//! Settlement-date derivation: T+n business-day arithmetic over the
//! exchange's seeded holiday calendar, with same-day settlement for
//! exchange-less (Crypto) listings and a coverage warning when the window
//! leaves the seeded holiday span.

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
    pool: &SqlitePool,
    listing_id: i64,
) -> Result<Option<i64>, sqlx::Error> {
    sqlx::query_scalar(
        "SELECT e.settlement_days FROM listings l \
         LEFT JOIN exchanges e ON e.mic = l.exchange_mic \
         WHERE l.id = ?",
    )
    .bind(listing_id)
    .fetch_one(pool)
    .await
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
    let Some(days) = settlement_days_for_listing(pool, listing_id).await? else {
        return Ok(date);
    };
    let holidays =
        crate::entities::exchange_holiday::exchange_holidays_for_listing(pool, listing_id).await?;
    let settlement = add_business_days(date, days, &holidays);
    warn_if_outside_holiday_coverage(trade_id, date, settlement, &holidays);
    Ok(settlement)
}
