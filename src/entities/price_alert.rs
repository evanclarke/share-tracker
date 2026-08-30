//! Price-change alerts: a held listing whose latest stored close moved more
//! than the configured threshold from the previous stored close, emailed once
//! per move.
//!
//! The `price-alert` job runs just after each market's `price-import` (see
//! `schedule.cron`). It walks **every** held listing on every run, because a
//! listing's market is a property of the listing and not of the cron line that
//! woke the job — so the three daily runs would each re-find the same movers.
//! [`price_alerts`](../../migrations/0049_price_alerts.sql) is what makes a
//! move alert exactly once: an alerted move is recorded, keyed
//! `(listing_id, price_date)`, and a recorded move is never re-sent. The send
//! log *is* the suppression, so "did it send, and what did it say" has one
//! answer rather than two that can drift.
//!
//! Two things the walk deliberately does not do:
//!
//! * It never converts to AUD. The alert is about the security's own move, and
//!   both figures come from `closing_prices.price`, which is in the listing's
//!   quote currency — converting would fold an FX movement into a price change.
//! * It never compares across a **price-basis event**. `closing_prices.price`
//!   is stored in the unit basis in force on its own date, so a pair straddling
//!   a share split, consolidation or demerger restatement is quoted in two
//!   different units and a 1-for-2 consolidation reads as a 50% crash. Such a
//!   pair is skipped and carried in the run's note — visible rather than
//!   silent, since a suppressed comparison is a comparison the reader did not
//!   get.
//!
//! No routes and no UI screen, by decision (see the migration): this is the
//! job's own operational state, written by one job and read by that job alone.
//! The emails are the surface it exists for.

use crate::entities::closing_price::{db_held_listing_ids, db_price_basis_events};
use crate::infra::db::write_tx;
use crate::infra::decimal::{Money, row_dec};
use crate::infra::email::{self, Document, Notifier, Section, Table};
use crate::infra::scheduler::JobOutcome;
use chrono::{DateTime, NaiveDate, Utc};
use rust_decimal::Decimal;
use sqlx::{Row, SqlitePool};

/// One recorded alert — a row of `price_alerts`, and exactly what the email
/// said about that listing.
///
/// Gated to the tests, and deliberately **not** `Serialize`: the table has no
/// read route (see the module docs), so nothing reads a row back in the
/// non-test build and nothing ever puts one on the wire. It is kept rather than
/// dropped because it is what the tests assert the recorded shape against,
/// which is the only check the send log's columns get.
#[cfg(test)]
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct PriceAlert {
    /// Never read — the send log is keyed by `(listing_id, price_date)` and
    /// has no route to fetch a row by id. Selected all the same so `SELECT *`
    /// maps onto this struct, which is what makes the tests' assertions cover
    /// the whole stored row rather than a chosen subset of it.
    #[expect(dead_code)]
    pub id: i64,
    pub listing_id: i64,
    pub price_date: NaiveDate,
    pub previous_date: NaiveDate,
    #[sqlx(try_from = "Money")]
    pub price: Decimal,
    #[sqlx(try_from = "Money")]
    pub previous_price: Decimal,
    #[sqlx(try_from = "Money")]
    pub change_pct: Decimal,
    #[sqlx(try_from = "Money")]
    pub threshold_pct: Decimal,
    pub sent_at: String,
}

/// One holding's move, with the identity the email needs to name it.
#[derive(Debug, Clone, PartialEq)]
pub struct Mover {
    pub listing_id: i64,
    pub ticker: String,
    pub name: String,
    pub currency: String,
    pub previous_date: NaiveDate,
    pub previous_price: Decimal,
    pub price_date: NaiveDate,
    pub price: Decimal,
    pub change_pct: Decimal,
}

impl Mover {
    pub fn change(&self) -> Decimal {
        self.price - self.previous_price
    }
}

/// What one walk found: the movers to alert on, and the comparisons it could
/// not make. The second is never dropped — the run's note carries it.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct Scan {
    pub movers: Vec<Mover>,
    pub skipped: Vec<String>,
}

/// A listing's two most recent stored ok closes, newest first.
async fn db_latest_two_closes(
    pool: &SqlitePool,
    listing_id: i64,
) -> Result<Vec<(NaiveDate, Decimal)>, sqlx::Error> {
    let rows = sqlx::query(
        "SELECT price_date, price FROM closing_prices \
         WHERE listing_id = ? AND status = 'ok' AND price IS NOT NULL \
         ORDER BY price_date DESC LIMIT 2",
    )
    .bind(listing_id)
    .fetch_all(pool)
    .await?;
    rows.iter()
        .map(|row| Ok((row.try_get("price_date")?, row_dec(row, "price")?)))
        .collect()
}

/// The listing's display identity for the email.
async fn db_listing_identity(
    pool: &SqlitePool,
    listing_id: i64,
) -> Result<Option<(String, String, String)>, sqlx::Error> {
    let row = sqlx::query("SELECT ticker, name, currency FROM listings WHERE id = ?")
        .bind(listing_id)
        .fetch_optional(pool)
        .await?;
    row.map(|row| {
        Ok((
            row.try_get("ticker")?,
            row.try_get("name")?,
            row.try_get("currency")?,
        ))
    })
    .transpose()
}

/// Whether this listing's close has already been alerted on.
pub async fn db_already_alerted(
    pool: &SqlitePool,
    listing_id: i64,
    price_date: NaiveDate,
) -> Result<bool, sqlx::Error> {
    let found: Option<i64> =
        sqlx::query_scalar("SELECT id FROM price_alerts WHERE listing_id = ? AND price_date = ?")
            .bind(listing_id)
            .bind(price_date)
            .fetch_optional(pool)
            .await?;
    Ok(found.is_some())
}

/// Record every alert the message that just went out carried, in one
/// transaction. Written **after** the send, so a failed send leaves no row and
/// the next run retries the move rather than silently swallowing it.
pub async fn db_record(
    pool: &SqlitePool,
    movers: &[Mover],
    threshold_pct: Decimal,
    sent_at: &str,
) -> Result<(), sqlx::Error> {
    let mut tx = write_tx(pool).await?;
    for mover in movers {
        sqlx::query(
            "INSERT INTO price_alerts \
                 (listing_id, price_date, previous_date, price, previous_price, \
                  change_pct, threshold_pct, sent_at) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?) \
             ON CONFLICT(listing_id, price_date) DO NOTHING",
        )
        .bind(mover.listing_id)
        .bind(mover.price_date)
        .bind(mover.previous_date)
        .bind(Money(mover.price))
        .bind(Money(mover.previous_price))
        .bind(Money(mover.change_pct.round_dp(4)))
        .bind(Money(threshold_pct))
        .bind(sent_at)
        .execute(&mut *tx)
        .await?;
    }
    tx.commit().await
}

/// Walk every listing held at `as_of` and find the moves worth alerting on.
///
/// A listing is passed over — silently, because none of these is a fault — when
/// it has fewer than two stored ok closes, when the earlier of the two is zero
/// (no percentage is definable), when the move is inside the threshold, or when
/// this close has already been alerted. The one pass-over that *is* reported is
/// a pair straddling a price-basis event: there the reader would have expected
/// an alert and must be told why there is none.
pub async fn scan(
    pool: &SqlitePool,
    threshold_pct: Decimal,
    as_of: NaiveDate,
) -> Result<Scan, sqlx::Error> {
    let mut scan = Scan::default();
    let mut conn = pool.acquire().await?;
    let listing_ids = db_held_listing_ids(pool, Some(as_of)).await?;

    for listing_id in listing_ids {
        let closes = db_latest_two_closes(pool, listing_id).await?;
        let [(price_date, price), (previous_date, previous_price)] = closes.as_slice() else {
            continue; // never priced, or priced once — nothing to compare
        };
        let (price_date, price) = (*price_date, *price);
        let (previous_date, previous_price) = (*previous_date, *previous_price);
        let Some((ticker, name, currency)) = db_listing_identity(pool, listing_id).await? else {
            continue; // deleted between the held read and this one
        };

        // The two prices are quoted in the same unit only if nothing re-based
        // the listing between them. `db_price_basis_events` is the same event
        // set the stored prices are normalised over, so asking it is asking
        // exactly the right question.
        let events = db_price_basis_events(&mut conn, listing_id).await?;
        if let Some(event) = events
            .iter()
            .find(|e| e.date > previous_date && e.date <= price_date)
        {
            scan.skipped.push(format!(
                "{ticker}: {previous_date} and {price_date} are quoted in different units \
                 (a price-basis event on {}), so no change was compared",
                event.date
            ));
            continue;
        }

        if previous_price.is_zero() {
            continue; // no percentage is definable against a zero close
        }
        let change_pct = (price - previous_price) / previous_price * Decimal::ONE_HUNDRED;
        if change_pct.abs() < threshold_pct {
            continue;
        }
        if db_already_alerted(pool, listing_id, price_date).await? {
            continue;
        }
        scan.movers.push(Mover {
            listing_id,
            ticker,
            name,
            currency,
            previous_date,
            previous_price,
            price_date,
            price,
            change_pct,
        });
    }

    // Biggest move first, by size rather than direction: an 11% fall leads an
    // 6% rise, which is the order the reader wants to scan.
    scan.movers
        .sort_by_key(|m| std::cmp::Reverse(m.change_pct.abs()));
    Ok(scan)
}

/// The alert message for a set of movers.
pub fn message(movers: &[Mover], threshold_pct: Decimal, skipped: &[String]) -> Document {
    let threshold = format!("{}%", threshold_pct.normalize());
    let subject = match movers {
        [only] => format!(
            "Price alert: {} {}",
            only.ticker,
            email::signed_percent(only.change_pct)
        ),
        many => format!(
            "Price alert: {} holdings moved {threshold} or more",
            many.len()
        ),
    };

    let mut table = Table::new(&[
        ("Holding", false),
        ("Currency", false),
        ("Previous close", false),
        ("Previous price", true),
        ("Close", false),
        ("Price", true),
        ("Change", true),
        ("Change %", true),
    ]);
    for mover in movers {
        table.row(vec![
            format!("{} — {}", mover.ticker, mover.name),
            mover.currency.clone(),
            mover.previous_date.to_string(),
            email::unit_price(mover.previous_price),
            mover.price_date.to_string(),
            email::unit_price(mover.price),
            email::signed_money(mover.change()),
            email::signed_percent(mover.change_pct),
        ]);
    }

    let mut section = Section::new(format!("Moves of {threshold} or more")).table(table);
    section = section.note(
        "Prices are in each listing's own quote currency, exactly as stored — not converted to \
         AUD, so an FX movement is never folded into a price change.",
    );
    for skip in skipped {
        section = section.note(skip.clone());
    }

    Document {
        subject,
        title: format!("Price changes of {threshold} or more"),
        sections: vec![section],
    }
}

/// One `price-alert` run: scan, send if anything moved, then record what was
/// sent.
pub async fn run_alert(
    pool: &SqlitePool,
    notifier: Option<&Notifier>,
    now: DateTime<Utc>,
) -> JobOutcome {
    let Some(notifier) = notifier else {
        // Not a failure: email is optional, and a deployment that never asked
        // for it must not accumulate a failed run per close. The note is what
        // keeps the Jobs screen from reading as a complete run.
        return Ok(Some(
            "no [email] configured, so no price alert was sent".to_string(),
        ));
    };
    let threshold = notifier.price_alert_pct;
    let scan = scan(pool, threshold, now.date_naive())
        .await
        .map_err(|e| e.to_string())?;

    if scan.movers.is_empty() {
        tracing::info!(
            threshold = %threshold,
            skipped = scan.skipped.len(),
            "price alert: nothing moved past the threshold"
        );
        return Ok(note(&scan.skipped));
    }

    let document = message(&scan.movers, threshold, &scan.skipped);
    notifier
        .mailer
        .send(&document)
        .await
        .map_err(|e| e.to_string())?;
    let sent_at = now.to_rfc3339();
    db_record(pool, &scan.movers, threshold, &sent_at)
        .await
        .map_err(|e| e.to_string())?;
    tracing::info!(
        alerted = scan.movers.len(),
        skipped = scan.skipped.len(),
        threshold = %threshold,
        recipients = %notifier.mailer.describe(),
        "price alert sent"
    );
    Ok(note(&scan.skipped))
}

/// A run that could not compare some pair did less than the whole of its work
/// and says so — the `JobOutcome` note convention (see
/// `infra::scheduler::registry`).
fn note(skipped: &[String]) -> Option<String> {
    (!skipped.is_empty()).then(|| skipped.join("; "))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::infra::email::test_support::Outbox;
    use crate::test_support::{dec, test_pool, ymd};
    use sqlx::SqlitePool;

    fn now_on(date: NaiveDate) -> DateTime<Utc> {
        date.and_hms_opt(6, 0, 0).unwrap().and_utc()
    }

    /// An ASX-listed AUD holding, bought on a trading day in January. The dates
    /// every test below uses (2026-03-02 to 03-06) are that week's Monday to
    /// Friday, so nothing here lands on a weekend or a seeded holiday.
    async fn held_listing(pool: &SqlitePool, id: i64, ticker: &str) {
        crate::test_support::listing(id)
            .ticker(ticker)
            .name(&format!("{ticker} Holdings"))
            .insert(pool)
            .await;
        crate::test_support::buy(id * 100, id)
            .date(ymd(2026, 1, 5))
            .qty(dec("10"))
            .insert(pool)
            .await;
    }

    async fn close(pool: &SqlitePool, listing_id: i64, date: NaiveDate, price: &str) {
        crate::test_support::closing_price(listing_id, date)
            .price(price)
            .insert(pool)
            .await;
    }

    #[tokio::test]
    async fn a_move_past_the_threshold_is_alerted_once_and_never_again() {
        let pool = test_pool().await;
        held_listing(&pool, 1, "AAA").await;
        close(&pool, 1, ymd(2026, 3, 2), "100").await;
        close(&pool, 1, ymd(2026, 3, 3), "108").await;

        let outbox = Outbox::new();
        let notifier = outbox.notifier(dec("5"));
        let note = run_alert(&pool, Some(&notifier), now_on(ymd(2026, 3, 3)))
            .await
            .expect("the run succeeds");
        assert_eq!(note, None, "nothing was passed over");

        let body = outbox.only_body();
        assert!(body.contains("AAA"), "{body}");
        assert!(body.contains("+8.00%"), "{body}");
        assert!(body.contains("108.0000"), "{body}");
        assert_eq!(outbox.only().subject, "Price alert: AAA +8.00%");

        // The second close of the day — the next market's run — re-finds the
        // same move and must not re-send it.
        run_alert(&pool, Some(&notifier), now_on(ymd(2026, 3, 3)))
            .await
            .expect("the run succeeds");
        assert_eq!(outbox.sent().len(), 1, "the same move is alerted once");
    }

    #[tokio::test]
    async fn a_fall_alerts_like_a_rise() {
        let pool = test_pool().await;
        held_listing(&pool, 1, "AAA").await;
        close(&pool, 1, ymd(2026, 3, 2), "100").await;
        close(&pool, 1, ymd(2026, 3, 3), "91").await;

        let outbox = Outbox::new();
        run_alert(
            &pool,
            Some(&outbox.notifier(dec("5"))),
            now_on(ymd(2026, 3, 3)),
        )
        .await
        .expect("the run succeeds");
        let body = outbox.only_body();
        assert!(body.contains("-9.00%"), "{body}");
    }

    #[tokio::test]
    async fn a_move_inside_the_threshold_sends_nothing() {
        let pool = test_pool().await;
        held_listing(&pool, 1, "AAA").await;
        close(&pool, 1, ymd(2026, 3, 2), "100").await;
        close(&pool, 1, ymd(2026, 3, 3), "104.99").await;

        let outbox = Outbox::new();
        let note = run_alert(
            &pool,
            Some(&outbox.notifier(dec("5"))),
            now_on(ymd(2026, 3, 3)),
        )
        .await
        .expect("the run succeeds");
        assert_eq!(note, None);
        assert!(outbox.sent().is_empty(), "nothing to say, nothing sent");
        // …and nothing was recorded, so a later 5% move is still alertable.
        assert!(!db_already_alerted(&pool, 1, ymd(2026, 3, 3)).await.unwrap());
    }

    #[tokio::test]
    async fn the_threshold_is_configurable() {
        let pool = test_pool().await;
        held_listing(&pool, 1, "AAA").await;
        close(&pool, 1, ymd(2026, 3, 2), "100").await;
        close(&pool, 1, ymd(2026, 3, 3), "103").await;

        let quiet = Outbox::new();
        run_alert(
            &pool,
            Some(&quiet.notifier(dec("5"))),
            now_on(ymd(2026, 3, 3)),
        )
        .await
        .expect("the run succeeds");
        assert!(quiet.sent().is_empty(), "3% is inside a 5% threshold");

        let loud = Outbox::new();
        run_alert(
            &pool,
            Some(&loud.notifier(dec("2.5"))),
            now_on(ymd(2026, 3, 3)),
        )
        .await
        .expect("the run succeeds");
        assert!(loud.only_body().contains("+3.00%"));
    }

    #[tokio::test]
    async fn a_pair_straddling_a_split_is_skipped_and_reported() {
        let pool = test_pool().await;
        held_listing(&pool, 1, "AAA").await;
        close(&pool, 1, ymd(2026, 3, 2), "100").await;
        // A 1-for-2 consolidation between the two closes: the stored prices are
        // in different units, so the apparent 50% fall is a change of unit.
        crate::entities::corporate_action::db_upsert(
            &pool,
            &crate::entities::corporate_action::CorporateAction {
                id: 1,
                listing_id: 1,
                date: ymd(2026, 3, 3),
                kind: crate::entities::corporate_action::ActionKind::ShareSplit {
                    split_new_units: dec("1"),
                    split_old_units: dec("2"),
                },
            },
        )
        .await
        .expect("the consolidation records");
        close(&pool, 1, ymd(2026, 3, 3), "50").await;

        let outbox = Outbox::new();
        let note = run_alert(
            &pool,
            Some(&outbox.notifier(dec("5"))),
            now_on(ymd(2026, 3, 3)),
        )
        .await
        .expect("the run succeeds");
        assert!(outbox.sent().is_empty(), "a change of unit is not a move");
        let note = note.expect("the suppressed comparison is on the record");
        assert!(note.contains("AAA"), "{note}");
        assert!(note.contains("different units"), "{note}");
    }

    #[tokio::test]
    async fn a_listing_with_one_close_is_passed_over() {
        let pool = test_pool().await;
        held_listing(&pool, 1, "AAA").await;
        close(&pool, 1, ymd(2026, 3, 3), "100").await;

        let outbox = Outbox::new();
        run_alert(
            &pool,
            Some(&outbox.notifier(dec("5"))),
            now_on(ymd(2026, 3, 3)),
        )
        .await
        .expect("the run succeeds");
        assert!(outbox.sent().is_empty());
    }

    #[tokio::test]
    async fn a_listing_no_longer_held_is_not_alerted() {
        let pool = test_pool().await;
        held_listing(&pool, 1, "AAA").await;
        crate::test_support::sell(700, 1)
            .date(ymd(2026, 2, 2))
            .qty(dec("10"))
            .insert(&pool)
            .await;
        // The holding is what the *allocations* say, not what a Sell row's
        // quantity says (`HeldTimeline`), so the sale has to consume the parcel.
        crate::entities::parcel_allocation::db_upsert(
            &pool,
            &crate::entities::parcel_allocation::ParcelAllocation {
                id: 1,
                sale_trade_id: 700,
                purchase_trade_id: 100,
                quantity_allocated: dec("10"),
            },
        )
        .await
        .expect("the sale consumes the parcel");
        close(&pool, 1, ymd(2026, 3, 2), "100").await;
        close(&pool, 1, ymd(2026, 3, 3), "150").await;

        let outbox = Outbox::new();
        run_alert(
            &pool,
            Some(&outbox.notifier(dec("5"))),
            now_on(ymd(2026, 3, 3)),
        )
        .await
        .expect("the run succeeds");
        assert!(outbox.sent().is_empty(), "nothing held, nothing to alert");
    }

    #[tokio::test]
    async fn movers_are_listed_biggest_move_first() {
        let pool = test_pool().await;
        for (id, ticker, close_price) in [(1, "AAA", "106"), (2, "BBB", "88"), (3, "CCC", "120")] {
            held_listing(&pool, id, ticker).await;
            close(&pool, id, ymd(2026, 3, 2), "100").await;
            close(&pool, id, ymd(2026, 3, 3), close_price).await;
        }
        let outbox = Outbox::new();
        run_alert(
            &pool,
            Some(&outbox.notifier(dec("5"))),
            now_on(ymd(2026, 3, 3)),
        )
        .await
        .expect("the run succeeds");
        let document = outbox.only();
        assert_eq!(document.subject, "Price alert: 3 holdings moved 5% or more");
        let table = document.sections[0].table.as_ref().expect("a table");
        let order: Vec<&str> = table.rows.iter().map(|r| r[0].as_str()).collect();
        assert!(order[0].starts_with("CCC"), "{order:?}"); // +20%
        assert!(order[1].starts_with("BBB"), "{order:?}"); // -12%
        assert!(order[2].starts_with("AAA"), "{order:?}"); // +6%
    }

    #[tokio::test]
    async fn a_failed_send_records_nothing_so_the_move_is_retried() {
        let pool = test_pool().await;
        held_listing(&pool, 1, "AAA").await;
        close(&pool, 1, ymd(2026, 3, 2), "100").await;
        close(&pool, 1, ymd(2026, 3, 3), "108").await;

        let broken = Outbox::failing("relay refused the connection");
        let err = run_alert(
            &pool,
            Some(&broken.notifier(dec("5"))),
            now_on(ymd(2026, 3, 3)),
        )
        .await
        .expect_err("a failed send fails the run");
        assert!(err.contains("could not send"), "{err}");
        assert!(
            !db_already_alerted(&pool, 1, ymd(2026, 3, 3)).await.unwrap(),
            "a move that never went out must stay alertable"
        );

        // The next run — the next market's close — sends it.
        let outbox = Outbox::new();
        run_alert(
            &pool,
            Some(&outbox.notifier(dec("5"))),
            now_on(ymd(2026, 3, 3)),
        )
        .await
        .expect("the run succeeds");
        assert_eq!(outbox.sent().len(), 1);
    }

    #[tokio::test]
    async fn with_no_email_configured_the_run_notes_that_nothing_was_sent() {
        let pool = test_pool().await;
        held_listing(&pool, 1, "AAA").await;
        close(&pool, 1, ymd(2026, 3, 2), "100").await;
        close(&pool, 1, ymd(2026, 3, 3), "150").await;

        let note = run_alert(&pool, None, now_on(ymd(2026, 3, 3)))
            .await
            .expect("a deployment without email is not a failure");
        let note = note.expect("the run says it sent nothing");
        assert!(note.contains("[email]"), "{note}");
    }

    #[tokio::test]
    async fn the_recorded_alert_carries_both_prices_and_the_threshold() {
        let pool = test_pool().await;
        held_listing(&pool, 1, "AAA").await;
        close(&pool, 1, ymd(2026, 3, 2), "100").await;
        close(&pool, 1, ymd(2026, 3, 3), "108").await;

        let outbox = Outbox::new();
        run_alert(
            &pool,
            Some(&outbox.notifier(dec("5"))),
            now_on(ymd(2026, 3, 3)),
        )
        .await
        .expect("the run succeeds");

        let alert: PriceAlert = sqlx::query_as("SELECT * FROM price_alerts")
            .fetch_one(&pool)
            .await
            .expect("one recorded alert");
        assert_eq!(alert.listing_id, 1);
        assert_eq!(alert.price_date, ymd(2026, 3, 3));
        assert_eq!(alert.previous_date, ymd(2026, 3, 2));
        assert_eq!(alert.price, dec("108"));
        assert_eq!(alert.previous_price, dec("100"));
        assert_eq!(alert.change_pct, dec("8"));
        assert_eq!(alert.threshold_pct, dec("5"));
        assert!(
            alert.sent_at.starts_with("2026-03-03T"),
            "{}",
            alert.sent_at
        );
    }
}
