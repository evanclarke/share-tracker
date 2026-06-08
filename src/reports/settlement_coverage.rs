use crate::entities::exchange_holiday::{coverage_span_for, window_outside_coverage};
use axum::{Json, Router, extract::State, http::StatusCode, routing::get};
use chrono::NaiveDate;
use serde::{Deserialize, Serialize};
use sqlx::{Row, SqlitePool};

/// A trade whose date or settlement window falls outside the seeded
/// exchange-holiday coverage for its exchange. Settlement-date calculation
/// silently degrades to weekend-only skipping there (`exchange_holidays` is
/// seeded only for the published calendar years), so the settlement date may
/// be wrong if the exchange observes a holiday in the window. Non-blocking:
/// trade writes are never rejected — this report only surfaces trades worth a
/// second look once the missing calendar years are entered.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SettlementCoverageAlert {
    pub trade_id: i64,
    pub listing_id: i64,
    pub ticker: String,
    pub mic: String,
    pub trade_type: String,
    pub date: NaiveDate,
    pub settlement_date: NaiveDate,
    /// `no_holiday_coverage` (the exchange has no seeded holidays at all) or
    /// `outside_holiday_coverage` (the window extends beyond the seeded years).
    pub coverage_status: String,
    /// The seeded coverage span for the exchange — 1 Jan of the earliest
    /// seeded holiday's year to 31 Dec of the latest's — or nulls when the
    /// exchange has no seeded holidays.
    pub coverage_start: Option<NaiveDate>,
    pub coverage_end: Option<NaiveDate>,
}

pub fn router() -> Router<SqlitePool> {
    Router::new().route("/reports/settlement_holiday_coverage", get(report))
}

/// Flag every trade whose `[date, settlement_date]` window is not fully inside
/// its exchange's seeded holiday coverage (the calendar-year span of the
/// exchange's `exchange_holidays` rows). Trades fully inside coverage are
/// omitted — an empty report means every settlement window was computed
/// against a complete calendar. Exchange-less (Crypto) listings settle
/// same-day with no holiday calendar at all, so their trades are skipped —
/// there is no coverage to be outside of.
pub async fn db_coverage_alerts(
    pool: &SqlitePool,
) -> Result<Vec<SettlementCoverageAlert>, sqlx::Error> {
    let rows = sqlx::query(
        "SELECT t.id AS trade_id, t.listing_id, l.ticker, l.exchange_mic AS mic, \
                t.trade_type, t.date, t.settlement_date, h.earliest, h.latest \
         FROM trades t \
         JOIN listings l ON l.id = t.listing_id \
         LEFT JOIN (SELECT mic, MIN(holiday_date) AS earliest, MAX(holiday_date) AS latest \
                    FROM exchange_holidays GROUP BY mic) h ON h.mic = l.exchange_mic \
         WHERE l.exchange_mic IS NOT NULL \
         ORDER BY l.ticker, t.date, t.id",
    )
    .fetch_all(pool)
    .await?;

    let mut alerts = Vec::new();
    for row in &rows {
        let date: NaiveDate = row.try_get("date")?;
        let settlement_date: NaiveDate = row.try_get("settlement_date")?;
        let earliest: Option<NaiveDate> = row.try_get("earliest")?;
        let latest: Option<NaiveDate> = row.try_get("latest")?;
        let span = match (earliest, latest) {
            (Some(e), Some(l)) => Some(coverage_span_for(e, l)),
            _ => None,
        };
        // A manual settlement_date override may precede the trade date, so
        // bound the window by min/max rather than assuming the order.
        if !window_outside_coverage(date.min(settlement_date), date.max(settlement_date), span) {
            continue;
        }
        alerts.push(SettlementCoverageAlert {
            trade_id: row.try_get("trade_id")?,
            listing_id: row.try_get("listing_id")?,
            ticker: row.try_get("ticker")?,
            mic: row.try_get("mic")?,
            trade_type: row.try_get("trade_type")?,
            date,
            settlement_date,
            coverage_status: if span.is_some() {
                "outside_holiday_coverage"
            } else {
                "no_holiday_coverage"
            }
            .to_string(),
            coverage_start: span.map(|(s, _)| s),
            coverage_end: span.map(|(_, e)| e),
        });
    }
    Ok(alerts)
}

async fn report(
    State(pool): State<SqlitePool>,
) -> Result<Json<Vec<SettlementCoverageAlert>>, StatusCode> {
    db_coverage_alerts(&pool).await.map(Json).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::entities::{exchange, listing, trade};
    use crate::infra::db;
    use axum::{body::Body, http::Request};
    use http_body_util::BodyExt;
    use rust_decimal::Decimal;
    use tower::ServiceExt;

    async fn test_pool() -> SqlitePool {
        db::init(":memory:").await.unwrap()
    }

    fn ymd(y: i32, m: u32, d: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, d).unwrap()
    }

    async fn insert_listing(pool: &SqlitePool, id: i64, mic: &str) {
        listing::db_upsert(
            pool,
            &listing::Listing {
                id,
                exchange_mic: Some(mic.to_string()),
                ticker: format!("T{id}"),
                name: format!("Test {id}"),
                isin: None,
                security_type: listing::SecurityType::ETF,
                currency: "AUD".to_string(),
                amit: false,
                preference: false,
            },
        )
        .await
        .unwrap();
    }

    async fn insert_buy(pool: &SqlitePool, id: i64, listing_id: i64, date: NaiveDate, settlement: NaiveDate) {
        trade::db_upsert(
            pool,
            &trade::Trade {
                brokerage_includes_gst: false,
                statement_total: None,
                holding_account_id: 1,
                transfer_id: None,
                ess_statement_id: None,
                id,
                trade_type: trade::TradeType::Buy,
                date,
                settlement_date: settlement,
                listing_id,
                average_price: Decimal::from(100),
                quantity: Decimal::from(10),
                currency: "AUD".to_string(),
                brokerage: Decimal::ZERO,
                gst_on_brokerage: Decimal::ZERO,
                brokerage_currency: "AUD".to_string(),
                fx_rate: Decimal::ONE,
                contract_note_ref: None,
                residual_brought_forward: Decimal::ZERO,
                residual_carried_forward: Decimal::ZERO,
                residual_paid_out: Decimal::ZERO,
                rights_action_id: None,
                buyback_action_id: None,
                scrip_action_id: None,
                demerger_action_id: None,
                worthless_action_id: None,
                deemed_acquisition_date: None,
            },
        )
        .await
        .unwrap();
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

    /// A trade dated beyond the seeded holiday range (2019–2027) is flagged,
    /// rather than the incomplete calendar being used silently.
    #[tokio::test]
    async fn db_trade_beyond_seeded_holiday_range_is_flagged() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "XASX").await;
        insert_buy(&pool, 1, 1, ymd(2030, 6, 3), ymd(2030, 6, 5)).await;
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

    /// A settlement window straddling the end of coverage is flagged even
    /// though the trade date itself is covered: the uncovered tail is where a
    /// missed holiday would shift the settlement.
    #[tokio::test]
    async fn db_window_straddling_coverage_end_is_flagged() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "XASX").await;
        insert_buy(&pool, 1, 1, ymd(2027, 12, 30), ymd(2028, 1, 4)).await;
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
    /// outside every seeded calendar.
    #[tokio::test]
    async fn db_crypto_trades_are_not_flagged() {
        let pool = test_pool().await;
        listing::db_upsert(
            &pool,
            &listing::Listing {
                id: 1,
                exchange_mic: None,
                ticker: "BTC".to_string(),
                name: "Bitcoin".to_string(),
                isin: None,
                security_type: listing::SecurityType::Crypto,
                currency: "AUD".to_string(),
                amit: false,
                preference: false,
            },
        )
        .await
        .unwrap();
        insert_buy(&pool, 1, 1, ymd(2030, 6, 3), ymd(2030, 6, 3)).await;
        assert!(db_coverage_alerts(&pool).await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn api_get_settlement_holiday_coverage() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "XASX").await;
        insert_buy(&pool, 1, 1, ymd(2030, 6, 3), ymd(2030, 6, 5)).await;
        let resp = router()
            .with_state(pool)
            .oneshot(
                Request::builder()
                    .uri("/reports/settlement_holiday_coverage")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let alerts: Vec<SettlementCoverageAlert> = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(alerts.len(), 1);
        assert_eq!(alerts[0].trade_id, 1);
        assert_eq!(alerts[0].coverage_status, "outside_holiday_coverage");
    }
}
