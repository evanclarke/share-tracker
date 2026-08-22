use crate::entities::closing_price::{self, NonTradingReason};
use crate::entities::exchange_holiday::{coverage_span_for, window_outside_coverage};
use crate::infra::http::ApiError;
use axum::{Json, Router, extract::State, routing::get};
use chrono::NaiveDate;
use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;
use std::collections::{HashMap, HashSet};

/// A trade whose settlement date cannot be trusted, for either of two
/// independent reasons — and a trade can carry both at once, which is why they
/// are two fields rather than two values of one:
///
/// - its `[date, settlement_date]` window falls outside the seeded
///   exchange-holiday coverage for its exchange ([`coverage_status`]), so
///   settlement-date calculation silently degraded to weekend-only skipping
///   there (`exchange_holidays` is seeded only for the published calendar
///   years) and the date may be wrong if the exchange observes a holiday in
///   the window;
/// - the stored `settlement_date` is not a trading day on the listing's own
///   calendar ([`SettlementCoverageAlert::settlement_non_trading_reason`]) — a
///   settlement on a Saturday or a public holiday, which is wrong by
///   construction whoever wrote it (SCENARIOS S-05).
///
/// Non-blocking on both counts: trade writes are never rejected — an explicit
/// `settlement_date` is a deliberate override the user is asserting, so it
/// stays writable and the row stays editable. This report only surfaces
/// settlement dates worth a second look.
///
/// [`coverage_status`]: SettlementCoverageAlert::coverage_status
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SettlementCoverageAlert {
    pub trade_id: i64,
    pub listing_id: i64,
    pub ticker: String,
    pub mic: String,
    pub trade_type: String,
    pub date: NaiveDate,
    pub settlement_date: NaiveDate,
    /// `no_holiday_coverage` (the exchange has no seeded holidays at all),
    /// `outside_holiday_coverage` (the window extends beyond the seeded
    /// years), or `inside_holiday_coverage` (the window was computed against a
    /// complete calendar — such a row is listed only because of
    /// `settlement_non_trading_reason`).
    pub coverage_status: String,
    /// The seeded coverage span for the exchange — 1 Jan of the earliest
    /// seeded holiday's year to 31 Dec of the latest's — or nulls when the
    /// exchange has no seeded holidays.
    pub coverage_start: Option<NaiveDate>,
    pub coverage_end: Option<NaiveDate>,
    /// Why the stored `settlement_date` is not a trading day on the calendar
    /// in force **on that date** (`weekend` / `holiday`), or null when it is a
    /// trading day — the row is then listed for its `coverage_status` alone.
    /// After an exchange change the calendar judged here is the one in force
    /// at settlement, which need not be the listing's `mic` today.
    pub settlement_non_trading_reason: Option<NonTradingReason>,
}

/// The joined trade row behind [`SettlementCoverageAlert`], before its window
/// is put to the exchange's coverage span and its settlement date to the
/// listing's calendar. Mapped by column name via `FromRow`.
#[derive(sqlx::FromRow)]
struct CoverageCandidate {
    trade_id: i64,
    listing_id: i64,
    ticker: String,
    mic: String,
    trade_type: String,
    date: NaiveDate,
    settlement_date: NaiveDate,
    earliest: Option<NaiveDate>,
    latest: Option<NaiveDate>,
}

pub fn router() -> Router<SqlitePool> {
    Router::new().route("/reports/settlement_holiday_coverage", get(report))
}

/// Flag every trade whose settlement date is not trustworthy: either its
/// `[date, settlement_date]` window is not fully inside its exchange's seeded
/// holiday coverage (the calendar-year span of the exchange's
/// `exchange_holidays` rows), or the stored `settlement_date` is not a trading
/// day on the listing's own calendar. Trades that are inside coverage *and*
/// settle on a trading day are omitted — an empty report means every
/// settlement date was computed against a complete calendar and lands on a day
/// the market was open. Exchange-less (Crypto) listings settle same-day with
/// no holiday calendar at all, so their trades are skipped — there is no
/// coverage to be outside of, and every day is a trading day.
///
/// Reads its inputs on one `pool.begin()` read transaction — a consistent
/// snapshot, so a holiday entered part-way through cannot make the coverage
/// span and the trading-day test disagree about the same calendar.
pub async fn db_coverage_alerts(
    pool: &SqlitePool,
) -> Result<Vec<SettlementCoverageAlert>, sqlx::Error> {
    let mut tx = pool.begin().await?;
    let candidates: Vec<CoverageCandidate> = sqlx::query_as(
        "SELECT t.id AS trade_id, t.listing_id, l.ticker, l.exchange_mic AS mic, \
                t.trade_type, t.date, t.settlement_date, h.earliest, h.latest \
         FROM trades t \
         JOIN listings l ON l.id = t.listing_id \
         LEFT JOIN (SELECT mic, MIN(holiday_date) AS earliest, MAX(holiday_date) AS latest \
                    FROM exchange_holidays GROUP BY mic) h ON h.mic = l.exchange_mic \
         WHERE l.exchange_mic IS NOT NULL \
         ORDER BY l.ticker, t.date, t.id",
    )
    .fetch_all(&mut *tx)
    .await?;

    // Every calendar first, one load per listing that has trades — not one per
    // trade: the `Market` behind it is four queries (the shape
    // `reports::health`'s non-trading-day alert uses).
    let listing_ids: HashSet<i64> = candidates.iter().map(|c| c.listing_id).collect();
    let mut markets: HashMap<i64, closing_price::Market> = HashMap::new();
    for listing_id in listing_ids {
        if let Some(market) = closing_price::load_market_on(&mut tx, listing_id).await? {
            markets.insert(listing_id, market);
        }
    }
    tx.commit().await?;

    let mut alerts = Vec::new();
    for c in candidates {
        let span = match (c.earliest, c.latest) {
            (Some(e), Some(l)) => Some(coverage_span_for(e, l)),
            _ => None,
        };
        // Every write path refuses a settlement date before the trade date
        // (`trade::check_amounts`'s `SettlementBeforeTrade`), so in a database
        // written only through the API the window is already ordered; the
        // min/max keeps this read honest for a row edited outside it — which
        // is exactly the kind of row this report exists to find.
        let outside = window_outside_coverage(
            c.date.min(c.settlement_date),
            c.date.max(c.settlement_date),
            span,
        );
        // The stored settlement date against the listing's own calendar, as it
        // stood on that date — the same helper the closing-price and trade
        // write paths refuse a non-trading day with.
        let settlement_non_trading_reason = markets
            .get(&c.listing_id)
            .and_then(|market| closing_price::non_trading_day(market, c.settlement_date))
            .map(|shut| shut.reason);
        if !outside && settlement_non_trading_reason.is_none() {
            continue;
        }
        alerts.push(SettlementCoverageAlert {
            trade_id: c.trade_id,
            listing_id: c.listing_id,
            ticker: c.ticker,
            mic: c.mic,
            trade_type: c.trade_type,
            date: c.date,
            settlement_date: c.settlement_date,
            coverage_status: match (outside, span.is_some()) {
                (false, _) => "inside_holiday_coverage",
                (true, true) => "outside_holiday_coverage",
                (true, false) => "no_holiday_coverage",
            }
            .to_string(),
            coverage_start: span.map(|(s, _)| s),
            coverage_end: span.map(|(_, e)| e),
            settlement_non_trading_reason,
        });
    }
    Ok(alerts)
}

async fn report(
    State(pool): State<SqlitePool>,
) -> Result<Json<Vec<SettlementCoverageAlert>>, ApiError> {
    db_coverage_alerts(&pool)
        .await
        .map(Json)
        .map_err(ApiError::from)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::entities::exchange;
    use crate::test_support::{self, ApiClient, test_pool, ymd};
    use axum::http::StatusCode;
    use rust_decimal::Decimal;

    async fn insert_listing(pool: &SqlitePool, id: i64, mic: &str) {
        test_support::listing(id).mic(mic).insert(pool).await;
    }

    async fn insert_buy(
        pool: &SqlitePool,
        id: i64,
        listing_id: i64,
        date: NaiveDate,
        settlement: NaiveDate,
    ) {
        test_support::buy(id, listing_id)
            .date(date)
            .settlement(settlement)
            .qty(Decimal::from(10))
            .price(Decimal::from(100))
            .insert(pool)
            .await;
    }

    /// Seed holidays run 2019–2027 for XASX; a trade settling inside that
    /// span was computed against a complete calendar, so nothing is flagged.
    #[tokio::test]
    async fn db_trade_inside_coverage_is_not_flagged() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "XASX").await;
        insert_buy(&pool, 1, 1, ymd(2024, 1, 15), ymd(2024, 1, 17)).await;
        assert!(db_coverage_alerts(&pool).await.unwrap().is_empty());
    }

    /// A settlement window running beyond the seeded holiday range
    /// (2019–2027) is flagged, rather than the incomplete calendar being used
    /// silently. The *trade* can no longer be dated past the range at all
    /// (SCENARIOS S-10 refuses a trade dated after today), so the window
    /// reaches past it the way a real one does: a hand-entered settlement,
    /// which is exactly what this report exists to catch.
    #[tokio::test]
    async fn db_trade_beyond_seeded_holiday_range_is_flagged() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "XASX").await;
        insert_buy(&pool, 1, 1, ymd(2026, 8, 3), ymd(2028, 1, 4)).await;
        let alerts = db_coverage_alerts(&pool).await.unwrap();
        assert_eq!(alerts.len(), 1);
        let a = &alerts[0];
        assert_eq!(a.trade_id, 1);
        assert_eq!(a.mic, "XASX");
        assert_eq!(a.coverage_status, "outside_holiday_coverage");
        // Coverage span is whole calendar years of the seeded holidays.
        assert_eq!(a.coverage_start, Some(ymd(2019, 1, 1)));
        assert_eq!(a.coverage_end, Some(ymd(2027, 12, 31)));
    }

    /// A trade dated before the seeded range is just as uncovered as one after it.
    #[tokio::test]
    async fn db_trade_before_seeded_holiday_range_is_flagged() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "XASX").await;
        insert_buy(&pool, 1, 1, ymd(2018, 3, 6), ymd(2018, 3, 8)).await;
        let alerts = db_coverage_alerts(&pool).await.unwrap();
        assert_eq!(alerts.len(), 1);
        assert_eq!(alerts[0].coverage_status, "outside_holiday_coverage");
    }

    /// A settlement window straddling the *start* of coverage is flagged even
    /// though the settlement itself lands inside it: the uncovered head is
    /// where a missed holiday would have shifted the settlement. (The
    /// end-of-coverage straddle is the case above; it can only be reached
    /// through a hand-entered settlement now that a trade cannot be dated
    /// after today — SCENARIOS S-10.)
    #[tokio::test]
    async fn db_window_straddling_coverage_start_is_flagged() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "XASX").await;
        insert_buy(&pool, 1, 1, ymd(2018, 12, 28), ymd(2019, 1, 3)).await;
        let alerts = db_coverage_alerts(&pool).await.unwrap();
        assert_eq!(alerts.len(), 1);
        assert_eq!(alerts[0].coverage_status, "outside_holiday_coverage");
    }

    /// An exchange with no seeded holidays at all has no coverage: every one
    /// of its trades is flagged `no_holiday_coverage` (null span).
    #[tokio::test]
    async fn db_exchange_without_seeded_holidays_is_flagged_as_no_coverage() {
        let pool = test_pool().await;
        exchange::db_upsert(
            &pool,
            &exchange::Exchange {
                mic: "XLON".to_string(),
                name: "LSE".to_string(),
                country: "GB".to_string(),
                currency: "AUD".to_string(),
                timezone: "Europe/London".to_string(),
                settlement_days: 2,
                close_time: "16:00".to_string(),
            },
        )
        .await
        .unwrap();
        insert_listing(&pool, 1, "XLON").await;
        insert_buy(&pool, 1, 1, ymd(2024, 1, 15), ymd(2024, 1, 17)).await;
        let alerts = db_coverage_alerts(&pool).await.unwrap();
        assert_eq!(alerts.len(), 1);
        let a = &alerts[0];
        assert_eq!(a.coverage_status, "no_holiday_coverage");
        assert_eq!(a.coverage_start, None);
        assert_eq!(a.coverage_end, None);
    }

    /// An exchange-less (Crypto) listing settles same-day with no holiday
    /// calendar at all, so its trades are never flagged — even dated far
    /// outside every seeded calendar (2019–2027 for XASX).
    #[tokio::test]
    async fn db_crypto_trades_are_not_flagged() {
        let pool = test_pool().await;
        test_support::listing(1)
            .crypto()
            .ticker("BTC")
            .name("Bitcoin")
            .insert(&pool)
            .await;
        insert_buy(&pool, 1, 1, ymd(2018, 6, 4), ymd(2018, 6, 4)).await;
        assert!(db_coverage_alerts(&pool).await.unwrap().is_empty());
    }

    // A stored settlement date that is not a trading day (SCENARIOS S-05).

    /// SCENARIOS S-05, the live row this check exists for: trade 9071, LAC on
    /// XNYS, dated 2021-03-25 with an explicit `settlement_date` of
    /// **2021-05-29, a Saturday** two months later — hand-entered, since no
    /// T+n arithmetic produces it. The window sits comfortably inside XNYS's
    /// seeded coverage, so nothing flagged it before; it is now listed with
    /// the coverage question answered `inside_holiday_coverage` and the
    /// settlement question answered `weekend`, so both facts are legible on
    /// one row.
    #[tokio::test]
    async fn db_supplied_weekend_settlement_is_flagged_inside_coverage() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "XNYS").await;
        insert_buy(&pool, 1, 1, ymd(2021, 3, 25), ymd(2021, 5, 29)).await;
        let alerts = db_coverage_alerts(&pool).await.unwrap();
        assert_eq!(alerts.len(), 1);
        let a = &alerts[0];
        assert_eq!(a.trade_id, 1);
        assert_eq!(a.settlement_date, ymd(2021, 5, 29));
        assert_eq!(a.coverage_status, "inside_holiday_coverage");
        assert_eq!(
            a.settlement_non_trading_reason,
            Some(NonTradingReason::Weekend)
        );
        // The window is inside coverage, so the span is still reported.
        assert_eq!(a.coverage_start, Some(ymd(2019, 1, 1)));
        assert_eq!(a.coverage_end, Some(ymd(2027, 12, 31)));
    }

    /// SCENARIOS S-05: a supplied settlement on a seeded public holiday is
    /// flagged the same way, and told apart from a weekend. Good Friday
    /// 2026-04-03 is a seeded XASX holiday; the trade itself is dated the
    /// trading day before it.
    #[tokio::test]
    async fn db_supplied_holiday_settlement_is_flagged() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "XASX").await;
        insert_buy(&pool, 1, 1, ymd(2026, 4, 2), ymd(2026, 4, 3)).await;
        let alerts = db_coverage_alerts(&pool).await.unwrap();
        assert_eq!(alerts.len(), 1);
        assert_eq!(alerts[0].coverage_status, "inside_holiday_coverage");
        assert_eq!(
            alerts[0].settlement_non_trading_reason,
            Some(NonTradingReason::Holiday)
        );
    }

    /// SCENARIOS S-05: a Crypto asset settles **same-day, every day**, so a
    /// Saturday settlement on one is correct rather than suspect and is never
    /// flagged — there is no exchange calendar to put it to.
    #[tokio::test]
    async fn db_crypto_same_day_saturday_settlement_is_not_flagged() {
        let pool = test_pool().await;
        test_support::listing(1)
            .crypto()
            .ticker("BTC")
            .name("Bitcoin")
            .insert(&pool)
            .await;
        // Saturday 2026-03-14, traded and settled the same day.
        insert_buy(&pool, 1, 1, ymd(2026, 3, 14), ymd(2026, 3, 14)).await;
        assert!(db_coverage_alerts(&pool).await.unwrap().is_empty());
    }

    /// SCENARIOS S-05: the two questions are independent, and one row can
    /// answer both badly — a hand-entered settlement that is both past the
    /// seeded coverage (2019–2027) and a Saturday. Neither fact hides the
    /// other: `coverage_status` still says the window left the calendar, and
    /// `settlement_non_trading_reason` still says the day itself was shut.
    #[tokio::test]
    async fn db_settlement_both_outside_coverage_and_on_a_weekend_reports_both() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "XASX").await;
        // Monday 2026-08-03, settling Saturday 2028-01-08 — beyond the seeded
        // span and on a weekend.
        insert_buy(&pool, 1, 1, ymd(2026, 8, 3), ymd(2028, 1, 8)).await;
        let alerts = db_coverage_alerts(&pool).await.unwrap();
        assert_eq!(alerts.len(), 1);
        let a = &alerts[0];
        assert_eq!(a.coverage_status, "outside_holiday_coverage");
        assert_eq!(a.coverage_end, Some(ymd(2027, 12, 31)));
        assert_eq!(
            a.settlement_non_trading_reason,
            Some(NonTradingReason::Weekend)
        );
    }

    #[tokio::test]
    async fn api_get_settlement_holiday_coverage() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "XASX").await;
        insert_buy(&pool, 1, 1, ymd(2018, 3, 6), ymd(2018, 3, 8)).await;
        let resp = ApiClient::over(router().with_state(pool))
            .get("/reports/settlement_holiday_coverage")
            .await;
        assert_eq!(resp.status, StatusCode::OK);
        let alerts: Vec<SettlementCoverageAlert> = resp.json();
        assert_eq!(alerts.len(), 1);
        assert_eq!(alerts[0].trade_id, 1);
        assert_eq!(alerts[0].coverage_status, "outside_holiday_coverage");
        // The wire shape the web UI's table renders: the settlement question
        // is its own field, present (as null here) whatever the coverage
        // answer is.
        let raw: serde_json::Value = resp.json();
        assert_eq!(
            raw[0]["settlement_non_trading_reason"],
            serde_json::Value::Null
        );
    }
}
