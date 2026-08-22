//! Settlement-date derivation: T+n business-day arithmetic over the
//! exchange's seeded holiday calendar, with same-day settlement for
//! exchange-less (Crypto) listings and a coverage warning when the window
//! leaves the seeded holiday span.
//!
//! Also the `settlement-recompute` maintenance job ([`run_recompute`]), which
//! re-derives the settlement dates this module computed once the calendar they
//! were computed against is completed (SCENARIOS S-04).

use super::model::SettlementDateSource;
use crate::infra::db::write_tx;
use chrono::{Datelike, NaiveDate};
use sqlx::SqlitePool;
use std::collections::HashSet;

/// Advance `date` by `business_days` trading days, skipping Saturdays, Sundays
/// and the exchange's public `holidays`.
///
/// Market settlement is quoted as T+n *business* days (e.g. ASX T+2), so a Thursday
/// trade settles the following Monday, not Saturday — and a settlement that would
/// land on a public holiday rolls forward to the next trading day. Pass the
/// exchange's holiday set (see `exchange_holiday::exchange_holidays_for_listing`);
/// an empty set degrades to weekend-only skipping.
pub(crate) fn add_business_days(
    date: NaiveDate,
    business_days: i64,
    holidays: &HashSet<NaiveDate>,
) -> NaiveDate {
    use chrono::Weekday;
    let mut result = date;
    let mut remaining = business_days;
    while remaining > 0 {
        result += chrono::Duration::days(1);
        let is_weekend = matches!(result.weekday(), Weekday::Sat | Weekday::Sun);
        if !is_weekend && !holidays.contains(&result) {
            remaining -= 1;
        }
    }
    result
}

/// Warn when an auto-computed settlement window falls outside the seeded
/// holiday coverage for the listing's exchange: `add_business_days` silently
/// degrades to weekend-only skipping there, so the date may be wrong if the
/// exchange observes a holiday in the window. Non-blocking — the write
/// proceeds; the settlement-holiday-coverage report
/// (`GET /reports/settlement_holiday_coverage`) flags the persisted trades.
pub(crate) fn warn_if_outside_holiday_coverage(
    trade_id: i64,
    date: NaiveDate,
    settlement_date: NaiveDate,
    holidays: &HashSet<NaiveDate>,
) {
    use crate::entities::exchange_holiday::{coverage_span, window_outside_coverage};
    if window_outside_coverage(date, settlement_date, coverage_span(holidays)) {
        tracing::warn!(
            trade_id,
            %date,
            %settlement_date,
            "settlement window outside seeded exchange-holiday coverage; computed skipping weekends only"
        );
    }
}

/// The listing's exchange T+n settlement period, or `None` for an
/// exchange-less (Crypto) listing — those settle same-day.
pub(crate) async fn settlement_days_for_listing(
    conn: &mut sqlx::SqliteConnection,
    listing_id: i64,
) -> Result<Option<i64>, sqlx::Error> {
    sqlx::query_scalar(
        "SELECT e.settlement_days FROM listings l \
         LEFT JOIN exchanges e ON e.mic = l.exchange_mic \
         WHERE l.id = ?",
    )
    .bind(listing_id)
    .fetch_one(&mut *conn)
    .await
}

/// A settlement date together with where it came from — what a write path
/// resolves before it stores either (SCENARIOS S-04).
///
/// The two travel together because they are one decision: a value the body
/// supplied is [`SettlementDateSource::Stated`] and is stored verbatim and
/// never rewritten, and an omitted one is [`SettlementDateSource::Computed`]
/// from the exchange's calendar and is the `settlement-recompute` job's to
/// re-derive. Splitting them across two arguments is what would let a caller
/// state a computed date, or claim a stated one was computed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Settlement {
    pub date: NaiveDate,
    pub source: SettlementDateSource,
}

impl Settlement {
    /// A date its writer asserts rather than derives — a `settlement_date`
    /// supplied in a PUT body, or the same-day settlement the derived
    /// operations write (a corporate action's date; see
    /// [`SettlementDateSource::Stated`]). Never rewritten by the
    /// `settlement-recompute` job.
    pub(crate) fn stated(date: NaiveDate) -> Self {
        Settlement {
            date,
            source: SettlementDateSource::Stated,
        }
    }

    /// Resolve a trade or Sell write's settlement date: the `supplied` value
    /// stated as given, else T+n computed over the exchange's calendar
    /// ([`auto_settlement_date`]). The one place that rule lives, so the trade
    /// and Sell endpoints cannot classify the same body differently.
    ///
    /// With one qualification, which is what keeps the provenance meaningful:
    /// **re-supplying the date already stored changes nothing**. A `GET` body
    /// PUT back verbatim — which is exactly what the web UI's edit form sends,
    /// every field included — is not an assertion about the settlement date,
    /// it is the value we handed out; treating it as one would quietly opt
    /// every edited trade out of the `settlement-recompute` job, and would
    /// upgrade a pre-0041 `Unrecorded` row into a claim nobody made. So a
    /// supplied date equal to the stored one keeps the recorded source, and
    /// only a *different* supplied date (or a trade being created) is
    /// [`SettlementDateSource::Stated`].
    pub(crate) async fn resolve(
        pool: &SqlitePool,
        trade_id: i64,
        listing_id: i64,
        date: NaiveDate,
        supplied: Option<NaiveDate>,
    ) -> Result<Self, sqlx::Error> {
        let Some(supplied) = supplied else {
            return Ok(Settlement {
                date: auto_settlement_date(pool, trade_id, listing_id, date).await?,
                source: SettlementDateSource::Computed,
            });
        };
        let stored: Option<StoredSettlement> = sqlx::query_as(
            "SELECT id, listing_id, date, settlement_date, settlement_date_source \
             FROM trades WHERE id = ?",
        )
        .bind(trade_id)
        .fetch_optional(pool)
        .await?;
        Ok(match stored {
            Some(stored) if stored.settlement_date == supplied => Settlement {
                date: supplied,
                source: stored.settlement_date_source,
            },
            _ => Settlement::stated(supplied),
        })
    }
}

/// Auto-populate a settlement date for a trade with none supplied. An
/// exchange-listed security settles T+n business days after the trade date,
/// skipping weekends and the exchange's seeded holidays (warning when the
/// window leaves seeded coverage). An exchange-less (Crypto) listing settles
/// same-day — no T+n, no holiday calendar, no coverage warning.
pub(crate) async fn auto_settlement_date(
    pool: &SqlitePool,
    trade_id: i64,
    listing_id: i64,
    date: NaiveDate,
) -> Result<NaiveDate, sqlx::Error> {
    let mut conn = pool.acquire().await?;
    auto_settlement_date_on(&mut conn, trade_id, listing_id, date).await
}

/// [`auto_settlement_date`] on the caller's own connection, so the
/// `settlement-recompute` job can re-derive a date on the transaction it
/// rewrites it in — one consistent read of the calendar for the whole run.
pub(crate) async fn auto_settlement_date_on(
    conn: &mut sqlx::SqliteConnection,
    trade_id: i64,
    listing_id: i64,
    date: NaiveDate,
) -> Result<NaiveDate, sqlx::Error> {
    let Some(days) = settlement_days_for_listing(&mut *conn, listing_id).await? else {
        return Ok(date);
    };
    let holidays =
        crate::entities::exchange_holiday::exchange_holidays_for_listing(&mut *conn, listing_id)
            .await?;
    let settlement = add_business_days(date, days, &holidays);
    warn_if_outside_holiday_coverage(trade_id, date, settlement, &holidays);
    Ok(settlement)
}

/// One trade the recompute pass considers: its stored settlement date, and
/// where that date came from.
#[derive(sqlx::FromRow)]
struct StoredSettlement {
    id: i64,
    listing_id: i64,
    date: NaiveDate,
    settlement_date: NaiveDate,
    settlement_date_source: SettlementDateSource,
}

/// Re-derive every **computed** settlement date from the exchange calendar as
/// it now stands — the `settlement-recompute` maintenance job
/// (`POST /jobs/settlement-recompute`), deliberately unscheduled, in the shape
/// of `price-rebase`.
///
/// Why it exists (SCENARIOS S-04): `exchange_holidays` is seeded per published
/// calendar year, and a trade whose settlement window runs past the last
/// seeded year is computed skipping weekends only — it can land on a holiday
/// that has not been entered yet, or one business day early because a holiday
/// inside the window was missing. `GET /reports/settlement_holiday_coverage`
/// flags exactly those trades; seeding the year it asks for extends the
/// coverage span, so the row goes quiet while the stored date stays wrong.
/// This job is what makes the seeding actually repair them, and the coverage
/// report's documented contract points at it.
///
/// **Only dates this server computed are rewritten** — `settlement_date_source
/// = 'computed'`, which is also all the pass reads. A stated date is the
/// taxpayer's own assertion (S-05: trade 9071 settles on a Saturday two months
/// after the trade, deliberately left as entered), and an unrecorded one —
/// every row written before migration 0041 — might be, so neither is touched.
///
/// Deliberately they are not *reported* either, tempting as it looks: most of
/// them are the derived paths' same-day settlements (an ESS vest, a DRP, a
/// transfer's Sell — 51 of the live database's 113 trades, all correct), so a
/// "today's calendar computes this differently" line over them would be noise
/// with the occasional real override buried in it. A stored date worth a
/// second look is the settlement-holiday-coverage report's business, which is
/// where S-05 put it.
///
/// The calendar recomputed against is the one the **write path** would use
/// today: [`auto_settlement_date_on`] itself, over the listing's live
/// `exchange_mic`. That is deliberate on both counts — the job's whole purpose
/// is to leave the stored date where a re-save would put it, and using a
/// different (say, as-at-settlement) resolution would make the job and the
/// write path disagree. It therefore inherits the documented live-exchange
/// limitation (docs/API.md, Known limitations): on a listing that has changed
/// exchange, a computed settlement is re-derived over the new exchange's
/// calendar, exactly as re-saving the trade already does.
///
/// One transaction, so the whole repair lands or none of it does, and every
/// date is judged against one consistent read of the calendar. Idempotent: a
/// second run recomputes the same answers and writes nothing. The UPDATEs are
/// ordinary writes of an audited table, so each superseded date stays
/// recoverable in `row_history`.
pub async fn run_recompute(pool: &SqlitePool) -> Result<(), String> {
    let mut tx = write_tx(pool).await.map_err(|e| e.to_string())?;
    let stored: Vec<StoredSettlement> = sqlx::query_as(
        "SELECT id, listing_id, date, settlement_date, settlement_date_source \
         FROM trades ORDER BY id",
    )
    .fetch_all(&mut *tx)
    .await
    .map_err(|e| e.to_string())?;

    let candidates: Vec<&StoredSettlement> = stored
        .iter()
        .filter(|t| t.settlement_date_source.is_recomputable())
        .collect();
    let mut recomputed = 0usize;
    for trade in &candidates {
        let wanted = auto_settlement_date_on(&mut tx, trade.id, trade.listing_id, trade.date)
            .await
            .map_err(|e| e.to_string())?;
        if wanted == trade.settlement_date {
            continue;
        }
        sqlx::query("UPDATE trades SET settlement_date = ? WHERE id = ?")
            .bind(wanted)
            .bind(trade.id)
            .execute(&mut *tx)
            .await
            .map_err(|e| e.to_string())?;
        tracing::info!(
            trade_id = trade.id,
            from = %trade.settlement_date,
            to = %wanted,
            "settlement date recomputed against the current calendar"
        );
        recomputed += 1;
    }
    tx.commit().await.map_err(|e| e.to_string())?;

    tracing::info!(
        trades = stored.len(),
        candidates = candidates.len(),
        recomputed,
        "settlement-date recompute complete"
    );
    Ok(())
}
