//! The weekly portfolio summary email: the Portfolio Overview's two panels for
//! the week just closed.
//!
//! The screen is where the week's movement lives, and it is only ever seen by
//! someone who opens the app. This composes the same two panels into a message
//! the `weekly-summary` job sends after the week's last close:
//!
//! * the **period-performance headline** (`reports::period_performance`) —
//!   opening and closing market value, the period return and its split into
//!   capital growth, FX movement and income, plus purchases, sale proceeds and
//!   the realised-capital-gain cross-check;
//! * **market value and unrealised gain over time** — the window's stored
//!   snapshot series, one row per date (the graph's own data; see
//!   `infra::email` for why it is a table and not a chart image);
//! * the **per-holding contributions** table, each row carrying the security's
//!   opening and closing unit price and the move between them — the figures
//!   behind the screen's sparkline.
//!
//! # The window is the stored snapshots, not the calendar
//!
//! `to` is the last stored snapshot on or before the run date, and `from` the
//! first on or after seven days before it — which is exactly how the screen
//! resolves a range preset onto real snapshot dates (`nearestSeriesDates` in
//! `app.js`). Resolving it any other way would state figures for a window the
//! screen cannot show, and the first thing a reader does with a surprising
//! email is open the app and compare. Because both read the same stored rows,
//! they agree by construction.
//!
//! A window with fewer than two stored snapshots sends nothing and says so in
//! the run's note: there is no movement to report, and a mail stating a zero
//! week would be wrong rather than empty.
//!
//! This module opens no transaction of its own — every figure comes from
//! `period_performance::compute` and the two `reports::snapshot` series reads,
//! each of which holds whatever consistency its own result needs, and the only
//! reads made here are the two identity lookups (a listing's ticker, an
//! account's name) that no figure depends on.

use crate::infra::email::{self, Document, Notifier, Section, Table};
use crate::infra::scheduler::JobOutcome;
use crate::reports::{period_performance, snapshot};
use chrono::{DateTime, Duration, NaiveDate, Utc};
use rust_decimal::Decimal;
use sqlx::{Row, SqlitePool};
use std::collections::HashMap;

/// How far back the summary looks, in calendar days, before snapping to stored
/// snapshot dates. A week — the job runs weekly, and a window that did not
/// reach the previous run would leave days in no summary at all.
pub const SUMMARY_WINDOW_DAYS: i64 = 7;

#[derive(thiserror::Error, Debug)]
pub enum SummaryError {
    #[error("{0}")]
    Db(#[from] sqlx::Error),
    #[error("{0}")]
    Period(#[from] period_performance::PeriodError),
    #[error("{0}")]
    Mail(#[from] email::MailError),
}

/// The resolved window, plus the series points inside it — read once and used
/// by both the series section and the period request.
#[derive(Debug, Clone)]
pub struct Window {
    pub from: NaiveDate,
    pub to: NaiveDate,
    /// Every stored snapshot date in `[from, to]`, oldest first, with that
    /// date's portfolio totals.
    pub points: Vec<SeriesRow>,
}

/// One dated row of the market-value/unrealised-gain series. A flattening of
/// [`snapshot::SeriesPoint`] down to what the email states.
#[derive(Debug, Clone, PartialEq)]
pub struct SeriesRow {
    pub snapshot_date: NaiveDate,
    pub market_value: Decimal,
    pub unrealised_gain: Decimal,
}

/// Resolve the summary window against the stored snapshot series, the way the
/// Portfolio Overview screen resolves its range presets. `None` when fewer than
/// two stored snapshots fall in it — see the module docs.
pub async fn resolve_window(
    pool: &SqlitePool,
    run_date: NaiveDate,
) -> Result<Option<Window>, sqlx::Error> {
    let earliest = run_date - Duration::days(SUMMARY_WINDOW_DAYS);
    let points: Vec<SeriesRow> = snapshot::db_series(pool, None)
        .await?
        .into_iter()
        .filter(|p| p.snapshot_date >= earliest && p.snapshot_date <= run_date)
        .map(|p| SeriesRow {
            snapshot_date: p.snapshot_date,
            market_value: p.market_value,
            unrealised_gain: p.unrealised_gain,
        })
        .collect();
    let ([first, ..], Some(last)) = (points.as_slice(), points.last()) else {
        return Ok(None);
    };
    if first.snapshot_date >= last.snapshot_date {
        return Ok(None); // one stored date: nothing to measure a change against
    }
    Ok(Some(Window {
        from: first.snapshot_date,
        to: last.snapshot_date,
        points,
    }))
}

/// Listing tickers and holding-account names, for labelling contributions rows.
/// The whole of both identity tables — a portfolio has tens of each, and one
/// read is cheaper than a lookup per row.
async fn db_names(
    pool: &SqlitePool,
) -> Result<(HashMap<i64, String>, HashMap<i64, String>), sqlx::Error> {
    let listings = sqlx::query("SELECT id, ticker FROM listings")
        .fetch_all(pool)
        .await?
        .iter()
        .map(|row| Ok((row.try_get("id")?, row.try_get("ticker")?)))
        .collect::<Result<HashMap<i64, String>, sqlx::Error>>()?;
    let accounts = sqlx::query("SELECT id, name FROM holding_accounts")
        .fetch_all(pool)
        .await?
        .iter()
        .map(|row| Ok((row.try_get("id")?, row.try_get("name")?)))
        .collect::<Result<HashMap<i64, String>, sqlx::Error>>()?;
    Ok((listings, accounts))
}

/// The strict "no bearing on this period" predicate the Portfolio Overview's
/// *hide holdings with no activity* tick applies by default (`util.js`'s
/// `holdingHasActivity`): a row is inactive only when opening and closing
/// market value, purchases, sale proceeds and income are **all** exactly zero —
/// which forces the derived capital/FX/return figures to zero too. That is a
/// holding fully closed before the window began. A holding that was merely
/// flat still counts as active and is listed.
fn has_activity(h: &period_performance::HoldingPeriod) -> bool {
    ![
        h.opening_market_value,
        h.closing_market_value,
        h.purchases,
        h.sale_proceeds,
        h.income,
    ]
    .iter()
    .all(Decimal::is_zero)
}

/// Build the whole message for a resolved window.
pub async fn compose(
    pool: &SqlitePool,
    window: &Window,
    now: DateTime<Utc>,
) -> Result<Document, SummaryError> {
    let period = period_performance::compute(pool, window.from, window.to, now).await?;
    let trends = snapshot::db_holding_series(pool, Some(window.from), Some(window.to)).await?;
    let (tickers, accounts) = db_names(pool).await?;

    Ok(Document {
        subject: format!("Portfolio week to {}", window.to),
        title: format!("Portfolio week {} to {}", window.from, window.to),
        sections: vec![
            headline_section(&period),
            series_section(window),
            contributions_section(&period, &trends, &tickers, &accounts),
        ],
    })
}

/// The period-performance stat grid, with every advisory flag the report
/// carries stated above it. Surfacing them is not optional: a figure resting on
/// a fallback-month FX rate, a stale close, or a portfolio total that is
/// missing a holding must say so wherever it is shown.
fn headline_section(period: &period_performance::PeriodPerformance) -> Section {
    let mut section = Section::new("Summary");
    if period.provisional {
        section = section.note(
            "Provisional: a conversion at one end of this period used a fallback-month FX rate \
             (the real month's rate was not published yet). These figures will change once it \
             lands.",
        );
    }
    if period.price_carried_forward {
        section = section.note(
            "Carried-forward price: a holding at one end of this period is marked unpriced from \
             a date and was valued at its last stored close, so its capital growth over the \
             window is measured against a stale price.",
        );
    }
    if period.holding_excluded {
        let mut note = "Holding excluded: a holding at one end of this period has no obtainable \
                        price there, so that endpoint's total omits it."
            .to_string();
        for excluded in &period.excluded_holdings {
            note.push(' ');
            note.push_str(&excluded.reason);
        }
        section = section.note(note);
    }
    section
        .stat("Opening value", email::money(period.opening_market_value))
        .stat("Closing value", email::money(period.closing_market_value))
        .stat("Period return", email::signed_money(period.total_return))
        .stat(
            "Return %",
            period
                .total_return_pct
                .map_or_else(|| "—".to_string(), email::signed_percent),
        )
        .stat("Capital growth", email::signed_money(period.capital_growth))
        .stat("FX movement", email::signed_money(period.fx_movement))
        .stat("Income", email::money(period.income))
        .stat("Purchases", email::money(period.purchases))
        .stat("Sale proceeds", email::money(period.sale_proceeds))
        .stat(
            "Realised capital gain (tax)",
            email::money(period.realised_capital_gain),
        )
}

/// The graph's own data, dated. The two changes are stated as stats above the
/// table because they are what the shape of the line was for.
fn series_section(window: &Window) -> Section {
    let (first, last) = (
        window.points.first().expect("a resolved window has points"),
        window.points.last().expect("a resolved window has points"),
    );
    let mut table = Table::new(&[
        ("Date", false),
        ("Market value", true),
        ("Unrealised gain", true),
    ]);
    for point in &window.points {
        table.row(vec![
            point.snapshot_date.to_string(),
            email::money(point.market_value),
            email::money(point.unrealised_gain),
        ]);
    }
    Section::new("Market value and unrealised gain over time")
        .stat(
            "Market value change",
            email::signed_money(last.market_value - first.market_value),
        )
        .stat(
            "Unrealised gain change",
            email::signed_money(last.unrealised_gain - first.unrealised_gain),
        )
        .table(table)
}

/// One row per holding with any bearing on the window, carrying the screen's
/// own columns plus the unit price at each end — the figures its sparkline
/// draws, which a mail client cannot be relied on to render as a picture.
fn contributions_section(
    period: &period_performance::PeriodPerformance,
    trends: &[snapshot::HoldingSeries],
    tickers: &HashMap<i64, String>,
    accounts: &HashMap<i64, String>,
) -> Section {
    let mut table = Table::new(&[
        ("Holding", false),
        ("Opening value", true),
        ("Closing value", true),
        ("Unit price", false),
        ("Price move", true),
        ("Purchases", true),
        ("Sale proceeds", true),
        ("Income", true),
        ("Capital growth", true),
        ("FX movement", true),
        ("Total return", true),
    ]);

    let active: Vec<&period_performance::HoldingPeriod> =
        period.holdings.iter().filter(|h| has_activity(h)).collect();
    for holding in &active {
        let label = format!(
            "{} ({})",
            tickers
                .get(&holding.listing_id)
                .cloned()
                .unwrap_or_else(|| format!("listing {}", holding.listing_id)),
            accounts
                .get(&holding.holding_account_id)
                .cloned()
                .unwrap_or_else(|| format!("account {}", holding.holding_account_id)),
        );
        let points = trends
            .iter()
            .find(|t| {
                t.listing_id == holding.listing_id
                    && t.holding_account_id == holding.holding_account_id
            })
            .map(|t| t.points.as_slice())
            .unwrap_or_default();
        let (prices, move_pct) = unit_price_cells(points);
        table.row(vec![
            label,
            email::money(holding.opening_market_value),
            email::money(holding.closing_market_value),
            prices,
            move_pct,
            email::money(holding.purchases),
            email::money(holding.sale_proceeds),
            email::money(holding.income),
            email::signed_money(holding.capital_growth),
            email::signed_money(holding.fx_movement),
            email::signed_money(holding.total_return),
        ]);
    }

    let mut section = Section::new("Per-holding contributions");
    let hidden = period.holdings.len() - active.len();
    if active.is_empty() {
        section = section.note("No holding had any activity in this period.");
        return section;
    }
    if hidden > 0 {
        section = section.note(format!(
            "{hidden} {} with no activity in this period {} not listed.",
            if hidden == 1 { "holding" } else { "holdings" },
            if hidden == 1 { "is" } else { "are" },
        ));
    }
    section.table(table)
}

/// The window's opening and closing unit price, and the move between them.
///
/// Per **unit**, deliberately not the holding's market value, for the reason
/// the screen's sparkline plots the same figure: a purchase inside the window
/// raises market value on a day the security did not move at all, so a rise
/// drawn from it would hide a falling holding behind money added to it. The
/// row's own opening/closing value columns carry the position's size.
///
/// A holding with fewer than two stored points in the window has no move to
/// state — it was not held for enough of the window, or those days stored no
/// price — and gets a dash rather than a fabricated zero.
fn unit_price_cells(points: &[snapshot::HoldingSeriesPoint]) -> (String, String) {
    let ([first, ..], Some(last)) = (points, points.last()) else {
        return ("—".to_string(), "—".to_string());
    };
    if points.len() < 2 || first.unit_price.is_zero() {
        return (email::unit_price(last.unit_price), "—".to_string());
    }
    let move_pct = (last.unit_price - first.unit_price) / first.unit_price * Decimal::ONE_HUNDRED;
    (
        format!(
            "{} → {}",
            email::unit_price(first.unit_price),
            email::unit_price(last.unit_price)
        ),
        email::signed_percent(move_pct),
    )
}

/// One `weekly-summary` run.
pub async fn run_weekly_summary(
    pool: &SqlitePool,
    notifier: Option<&Notifier>,
    now: DateTime<Utc>,
) -> JobOutcome {
    let Some(notifier) = notifier else {
        return Ok(Some(
            "no [email] configured, so no weekly summary was sent".to_string(),
        ));
    };
    let Some(window) = resolve_window(pool, now.date_naive())
        .await
        .map_err(|e| e.to_string())?
    else {
        // A success that did less than the whole of its work: the run happened,
        // there was simply nothing to measure. Failing would be wrong — the
        // ordinary cause is a portfolio whose snapshots have not caught up yet.
        return Ok(Some(format!(
            "fewer than two stored report snapshots in the {SUMMARY_WINDOW_DAYS} days to {}, \
             so no weekly summary was sent",
            now.date_naive()
        )));
    };
    let document = compose(pool, &window, now)
        .await
        .map_err(|e| e.to_string())?;
    notifier
        .mailer
        .send(&document)
        .await
        .map_err(|e| e.to_string())?;
    tracing::info!(
        from = %window.from,
        to = %window.to,
        recipients = %notifier.mailer.describe(),
        "weekly portfolio summary sent"
    );
    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::entities::closing_price::test_support::QuoteStub;
    use crate::infra::email::test_support::Outbox;
    use crate::test_support::{self, dec, test_pool, ymd};

    fn now_on(date: NaiveDate) -> DateTime<Utc> {
        date.and_hms_opt(22, 0, 0).unwrap().and_utc()
    }

    /// An ASX-listed AUD holding bought in January and priced on each of the
    /// given dates. The window every test uses (2026-03-02 to 03-06) is that
    /// week's Monday to Friday, so every date is a trading day of the seeded
    /// calendar and every one of them is valuable.
    async fn holding(pool: &SqlitePool, id: i64, ticker: &str, prices: &[(NaiveDate, &str)]) {
        test_support::listing(id)
            .ticker(ticker)
            .name(&format!("{ticker} Holdings"))
            .insert(pool)
            .await;
        test_support::buy(id * 100, id)
            .date(ymd(2026, 1, 5))
            .qty(dec("10"))
            .price(dec("100"))
            .insert(pool)
            .await;
        for (date, price) in prices {
            test_support::closing_price(id, *date)
                .price(price)
                .insert(pool)
                .await;
        }
    }

    /// Generate the daily snapshots the summary reads, one per priced date.
    async fn snapshots(pool: &SqlitePool, dates: &[NaiveDate]) {
        for date in dates {
            snapshot::generate(pool, *date, now_on(*date + Duration::days(2)))
                .await
                .expect("the day is valuable");
        }
    }

    const WEEK: [NaiveDate; 5] = [
        NaiveDate::from_ymd_opt(2026, 3, 2).unwrap(),
        NaiveDate::from_ymd_opt(2026, 3, 3).unwrap(),
        NaiveDate::from_ymd_opt(2026, 3, 4).unwrap(),
        NaiveDate::from_ymd_opt(2026, 3, 5).unwrap(),
        NaiveDate::from_ymd_opt(2026, 3, 6).unwrap(),
    ];

    async fn a_priced_week(pool: &SqlitePool) {
        let prices: Vec<(NaiveDate, &str)> = WEEK
            .iter()
            .copied()
            .zip(["100", "102", "101", "104", "110"])
            .collect();
        holding(pool, 1, "AAA", &prices).await;
        snapshots(pool, &WEEK).await;
    }

    #[tokio::test]
    async fn the_window_snaps_to_the_stored_snapshot_dates() {
        let pool = test_pool().await;
        a_priced_week(&pool).await;
        let window = resolve_window(&pool, ymd(2026, 3, 6))
            .await
            .expect("resolves")
            .expect("a week of snapshots is a window");
        assert_eq!(window.from, ymd(2026, 3, 2));
        assert_eq!(window.to, ymd(2026, 3, 6));
        assert_eq!(window.points.len(), 5);
    }

    #[tokio::test]
    async fn one_stored_snapshot_is_no_window() {
        let pool = test_pool().await;
        holding(&pool, 1, "AAA", &[(ymd(2026, 3, 6), "100")]).await;
        snapshots(&pool, &[ymd(2026, 3, 6)]).await;
        assert!(
            resolve_window(&pool, ymd(2026, 3, 6))
                .await
                .expect("resolves")
                .is_none(),
            "there is nothing to measure a change against"
        );
    }

    #[tokio::test]
    async fn a_run_with_no_window_sends_nothing_and_says_why() {
        let pool = test_pool().await;
        let outbox = Outbox::new();
        let note = run_weekly_summary(
            &pool,
            Some(&outbox.notifier(dec("5"))),
            now_on(ymd(2026, 3, 6)),
        )
        .await
        .expect("an empty database is not a failure");
        let note = note.expect("the run says why it sent nothing");
        assert!(
            note.contains("fewer than two stored report snapshots"),
            "{note}"
        );
        assert!(outbox.sent().is_empty());
    }

    #[tokio::test]
    async fn the_summary_carries_the_headline_the_series_and_the_holdings() {
        let pool = test_pool().await;
        a_priced_week(&pool).await;
        let outbox = Outbox::new();
        let note = run_weekly_summary(
            &pool,
            Some(&outbox.notifier(dec("5"))),
            now_on(ymd(2026, 3, 6)),
        )
        .await
        .expect("the run succeeds");
        assert_eq!(note, None);

        let document = outbox.only();
        assert_eq!(document.subject, "Portfolio week to 2026-03-06");
        assert_eq!(document.title, "Portfolio week 2026-03-02 to 2026-03-06");
        let headings: Vec<&str> = document
            .sections
            .iter()
            .map(|s| s.heading.as_str())
            .collect();
        assert_eq!(
            headings,
            [
                "Summary",
                "Market value and unrealised gain over time",
                "Per-holding contributions"
            ]
        );

        let body = outbox.only_body();
        // Opening and closing portfolio value: 10 units at 100 and at 110.
        assert!(body.contains("1,000.00"), "{body}");
        assert!(body.contains("1,100.00"), "{body}");
        // Every stored date in the window is a row of the series table.
        for date in WEEK {
            assert!(body.contains(&date.to_string()), "missing {date}: {body}");
        }
        // The holding's row carries its ticker, its account, and the unit price
        // at each end of the window with the move between them.
        assert!(body.contains("AAA (Default)"), "{body}");
        assert!(body.contains("100.0000"), "{body}");
        assert!(body.contains("110.0000"), "{body}");
        assert!(body.contains("+10.00%"), "{body}");
    }

    #[tokio::test]
    async fn a_holding_closed_before_the_window_is_not_listed() {
        let pool = test_pool().await;
        a_priced_week(&pool).await;
        // A second listing bought and fully sold in January: every one of its
        // period figures is zero, so the screen hides it and so does this.
        holding(&pool, 2, "OLD", &[]).await;
        test_support::sell(701, 2)
            .date(ymd(2026, 1, 15))
            .qty(dec("10"))
            .insert(&pool)
            .await;
        // The holding is what the allocations say, not the Sell row's own
        // quantity (`HeldTimeline`), so the sale has to consume the parcel —
        // otherwise the listing is still held and its missing prices block the
        // window's valuation.
        crate::entities::parcel_allocation::db_upsert(
            &pool,
            &crate::entities::parcel_allocation::ParcelAllocation {
                id: 1,
                sale_trade_id: 701,
                purchase_trade_id: 200,
                quantity_allocated: dec("10"),
            },
        )
        .await
        .expect("the sale consumes the parcel");

        let outbox = Outbox::new();
        run_weekly_summary(
            &pool,
            Some(&outbox.notifier(dec("5"))),
            now_on(ymd(2026, 3, 6)),
        )
        .await
        .expect("the run succeeds");
        let body = outbox.only_body();
        assert!(body.contains("AAA"), "{body}");
        assert!(
            !body.contains("OLD ("),
            "a closed holding is not a row: {body}"
        );
        assert!(body.contains("no activity in this period"), "{body}");
    }

    #[tokio::test]
    async fn with_no_email_configured_the_run_notes_that_nothing_was_sent() {
        let pool = test_pool().await;
        a_priced_week(&pool).await;
        let note = run_weekly_summary(&pool, None, now_on(ymd(2026, 3, 6)))
            .await
            .expect("a deployment without email is not a failure");
        assert!(note.expect("a note").contains("[email]"));
    }

    #[tokio::test]
    async fn a_failed_send_fails_the_run_with_the_transport_message() {
        let pool = test_pool().await;
        a_priced_week(&pool).await;
        let broken = Outbox::failing("relay refused the connection");
        let err = run_weekly_summary(
            &pool,
            Some(&broken.notifier(dec("5"))),
            now_on(ymd(2026, 3, 6)),
        )
        .await
        .expect_err("a failed send fails the run");
        assert!(err.contains("relay refused the connection"), "{err}");
    }

    #[tokio::test]
    async fn a_holding_with_one_stored_point_states_no_move() {
        let points = [snapshot::HoldingSeriesPoint {
            snapshot_date: ymd(2026, 3, 6),
            unit_price: dec("12.5"),
        }];
        let (prices, move_pct) = unit_price_cells(&points);
        assert_eq!(prices, "12.5000");
        assert_eq!(move_pct, "—");
        // …and none at all is a dash on both.
        assert_eq!(unit_price_cells(&[]), ("—".to_string(), "—".to_string()));
    }

    // Keeps the offline quote stub named here: every test above values from
    // stored prices, and this asserts the module never reaches for a live one.
    #[tokio::test]
    async fn the_summary_values_from_stored_prices_only() {
        let pool = test_pool().await;
        a_priced_week(&pool).await;
        let _offline = QuoteStub::default();
        let outbox = Outbox::new();
        run_weekly_summary(
            &pool,
            Some(&outbox.notifier(dec("5"))),
            now_on(ymd(2026, 3, 6)),
        )
        .await
        .expect("the run succeeds without a price fetcher at all");
        assert_eq!(outbox.sent().len(), 1);
    }
}
