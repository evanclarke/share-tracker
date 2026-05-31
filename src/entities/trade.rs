use axum::{
    Json, Router,
    extract::{Path, State},
    http::StatusCode,
    routing::get,
};
use chrono::{Datelike, NaiveDate};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use sqlx::{Row, SqlitePool};
use std::collections::HashSet;

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, sqlx::Type)]
pub enum TradeType {
    Buy,
    Sell,
    DRP,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Trade {
    pub id: i64,
    pub trade_type: TradeType,
    pub date: NaiveDate,
    pub settlement_date: NaiveDate,
    pub listing_id: i64,
    pub average_price: Decimal,
    pub quantity: Decimal,
    pub currency: String,
    pub brokerage: Decimal,
    pub gst_on_brokerage: Decimal,
    pub brokerage_currency: String,
    /// Manual foreign-per-AUD override (same convention as the ATO rate: AUD =
    /// foreign / fx_rate). Reports prefer the ATO RBA rate for the trade's month
    /// and fall back to this field only when no ATO rate exists (see `infra::fx`).
    /// 1.0 for AUD trades.
    pub fx_rate: Decimal,
    pub contract_note_ref: Option<String>,
    /// DRP reinvestment residual cash (DRP trades only; 0 for Buy/Sell). When a
    /// distribution doesn't divide evenly into whole shares, the leftover is
    /// carried forward to the next reinvestment or paid out. These are populated
    /// by the reinvestment operation (see `entities::drp_reinvestment`); a
    /// manually entered DRP trade leaves them 0.
    pub residual_brought_forward: Decimal,
    pub residual_carried_forward: Decimal,
    pub residual_paid_out: Decimal,
}

impl<'r> sqlx::FromRow<'r, sqlx::sqlite::SqliteRow> for Trade {
    fn from_row(row: &'r sqlx::sqlite::SqliteRow) -> Result<Self, sqlx::Error> {
        fn dec(s: String) -> Result<Decimal, sqlx::Error> {
            s.parse().map_err(|e: rust_decimal::Error| sqlx::Error::Decode(Box::new(e)))
        }
        Ok(Trade {
            id: row.try_get("id")?,
            trade_type: row.try_get::<TradeType, _>("trade_type")?,
            date: row.try_get("date")?,
            settlement_date: row.try_get("settlement_date")?,
            listing_id: row.try_get("listing_id")?,
            average_price: dec(row.try_get("average_price")?)?,
            quantity: dec(row.try_get("quantity")?)?,
            currency: row.try_get("currency")?,
            brokerage: dec(row.try_get("brokerage")?)?,
            gst_on_brokerage: dec(row.try_get("gst_on_brokerage")?)?,
            brokerage_currency: row.try_get("brokerage_currency")?,
            fx_rate: dec(row.try_get("fx_rate")?)?,
            contract_note_ref: row.try_get("contract_note_ref")?,
            residual_brought_forward: dec(row.try_get("residual_brought_forward")?)?,
            residual_carried_forward: dec(row.try_get("residual_carried_forward")?)?,
            residual_paid_out: dec(row.try_get("residual_paid_out")?)?,
        })
    }
}

#[derive(Debug, Deserialize)]
pub struct TradeBody {
    pub trade_type: TradeType,
    pub date: NaiveDate,
    #[serde(default)]
    pub settlement_date: Option<NaiveDate>,
    pub listing_id: i64,
    pub average_price: Decimal,
    pub quantity: Decimal,
    pub currency: String,
    pub brokerage: Decimal,
    pub gst_on_brokerage: Decimal,
    pub brokerage_currency: String,
    pub fx_rate: Decimal,
    #[serde(default)]
    pub contract_note_ref: Option<String>,
    #[serde(default)]
    pub residual_brought_forward: Decimal,
    #[serde(default)]
    pub residual_carried_forward: Decimal,
    #[serde(default)]
    pub residual_paid_out: Decimal,
}

pub fn router() -> Router<SqlitePool> {
    Router::new()
        .route("/trades", get(list))
        .route("/trades/{id}", get(get_one).put(upsert).delete(delete))
}

pub async fn db_list(pool: &SqlitePool) -> Result<Vec<Trade>, sqlx::Error> {
    sqlx::query_as(
        "SELECT id, trade_type, date, settlement_date, listing_id, average_price, quantity, \
         currency, brokerage, gst_on_brokerage, brokerage_currency, fx_rate, contract_note_ref, \
         residual_brought_forward, residual_carried_forward, residual_paid_out \
         FROM trades ORDER BY date, id",
    )
    .fetch_all(pool)
    .await
}

pub async fn db_get(pool: &SqlitePool, id: i64) -> Result<Option<Trade>, sqlx::Error> {
    sqlx::query_as(
        "SELECT id, trade_type, date, settlement_date, listing_id, average_price, quantity, \
         currency, brokerage, gst_on_brokerage, brokerage_currency, fx_rate, contract_note_ref, \
         residual_brought_forward, residual_carried_forward, residual_paid_out \
         FROM trades WHERE id = ?",
    )
    .bind(id)
    .fetch_optional(pool)
    .await
}

pub async fn db_upsert(pool: &SqlitePool, trade: &Trade) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO trades \
         (id, trade_type, date, settlement_date, listing_id, average_price, quantity, \
          currency, brokerage, gst_on_brokerage, brokerage_currency, fx_rate, contract_note_ref, \
          residual_brought_forward, residual_carried_forward, residual_paid_out) \
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?) \
         ON CONFLICT(id) DO UPDATE SET \
             trade_type               = excluded.trade_type, \
             date                     = excluded.date, \
             settlement_date          = excluded.settlement_date, \
             listing_id               = excluded.listing_id, \
             average_price            = excluded.average_price, \
             quantity                 = excluded.quantity, \
             currency                 = excluded.currency, \
             brokerage                = excluded.brokerage, \
             gst_on_brokerage         = excluded.gst_on_brokerage, \
             brokerage_currency       = excluded.brokerage_currency, \
             fx_rate                  = excluded.fx_rate, \
             contract_note_ref        = excluded.contract_note_ref, \
             residual_brought_forward = excluded.residual_brought_forward, \
             residual_carried_forward = excluded.residual_carried_forward, \
             residual_paid_out        = excluded.residual_paid_out",
    )
    .bind(trade.id)
    .bind(trade.trade_type)
    .bind(trade.date)
    .bind(trade.settlement_date)
    .bind(trade.listing_id)
    .bind(trade.average_price.to_string())
    .bind(trade.quantity.to_string())
    .bind(&trade.currency)
    .bind(trade.brokerage.to_string())
    .bind(trade.gst_on_brokerage.to_string())
    .bind(&trade.brokerage_currency)
    .bind(trade.fx_rate.to_string())
    .bind(&trade.contract_note_ref)
    .bind(trade.residual_brought_forward.to_string())
    .bind(trade.residual_carried_forward.to_string())
    .bind(trade.residual_paid_out.to_string())
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn db_delete(pool: &SqlitePool, id: i64) -> Result<bool, sqlx::Error> {
    let result = sqlx::query("DELETE FROM trades WHERE id = ?")
        .bind(id)
        .execute(pool)
        .await?;
    Ok(result.rows_affected() > 0)
}

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

pub(crate) async fn settlement_days_for_listing(
    pool: &SqlitePool,
    listing_id: i64,
) -> Result<i64, sqlx::Error> {
    sqlx::query_scalar(
        "SELECT e.settlement_days FROM listings l \
         JOIN exchanges e ON e.mic = l.exchange_mic \
         WHERE l.id = ?",
    )
    .bind(listing_id)
    .fetch_one(pool)
    .await
}

async fn list(State(pool): State<SqlitePool>) -> Result<Json<Vec<Trade>>, StatusCode> {
    db_list(&pool)
        .await
        .map(Json)
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)
}

async fn get_one(
    State(pool): State<SqlitePool>,
    Path(id): Path<i64>,
) -> Result<Json<Trade>, StatusCode> {
    db_get(&pool, id)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
        .map(Json)
        .ok_or(StatusCode::NOT_FOUND)
}

async fn upsert(
    State(pool): State<SqlitePool>,
    Path(id): Path<i64>,
    Json(body): Json<TradeBody>,
) -> Result<StatusCode, StatusCode> {
    // Sells must be created via PUT /sells/{id} so they are persisted together
    // with a full set of parcel allocations (no uncovered Sell can exist).
    if body.trade_type == TradeType::Sell {
        return Err(StatusCode::UNPROCESSABLE_ENTITY);
    }
    let settlement_date = match body.settlement_date {
        Some(d) => d,
        None => {
            let days = settlement_days_for_listing(&pool, body.listing_id)
                .await
                .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
            let holidays =
                crate::entities::exchange_holiday::exchange_holidays_for_listing(&pool, body.listing_id)
                    .await
                    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
            add_business_days(body.date, days, &holidays)
        }
    };
    let trade = Trade {
        id,
        trade_type: body.trade_type,
        date: body.date,
        settlement_date,
        listing_id: body.listing_id,
        average_price: body.average_price,
        quantity: body.quantity,
        currency: body.currency,
        brokerage: body.brokerage,
        gst_on_brokerage: body.gst_on_brokerage,
        brokerage_currency: body.brokerage_currency,
        fx_rate: body.fx_rate,
        contract_note_ref: body.contract_note_ref,
        residual_brought_forward: body.residual_brought_forward,
        residual_carried_forward: body.residual_carried_forward,
        residual_paid_out: body.residual_paid_out,
    };
    db_upsert(&pool, &trade)
        .await
        .map(|_| StatusCode::NO_CONTENT)
        .map_err(|e| crate::infra::http::write_error_status(&e))
}

async fn delete(
    State(pool): State<SqlitePool>,
    Path(id): Path<i64>,
) -> Result<StatusCode, StatusCode> {
    db_delete(&pool, id)
        .await
        .map(|found| if found { StatusCode::NO_CONTENT } else { StatusCode::NOT_FOUND })
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{infra::db, entities::listing};
    use axum::{body::Body, http::Request};
    use http_body_util::BodyExt;
    use rust_decimal::Decimal;
    use std::collections::HashSet;
    use tower::ServiceExt;

    async fn test_pool() -> SqlitePool {
        db::init(":memory:").await.unwrap()
    }

    async fn insert_test_listing(pool: &SqlitePool) {
        listing::db_upsert(
            pool,
            &listing::Listing {
                id: 1,
                exchange_mic: "XASX".to_string(),
                ticker: "VAS".to_string(),
                name: "Vanguard Australian Shares ETF".to_string(),
                isin: None,
                security_type: listing::SecurityType::ETF,
                currency: "AUD".to_string(),
                amit: false,
            },
        )
        .await
        .unwrap();
    }

    fn buy_trade() -> Trade {
        Trade {
            id: 1,
            trade_type: TradeType::Buy,
            date: NaiveDate::from_ymd_opt(2024, 1, 15).unwrap(),
            settlement_date: NaiveDate::from_ymd_opt(2024, 1, 17).unwrap(),
            listing_id: 1,
            average_price: Decimal::from(100),
            quantity: Decimal::from(10),
            currency: "AUD".to_string(),
            brokerage: "9.95".parse().unwrap(),
            gst_on_brokerage: "0.995".parse().unwrap(),
            brokerage_currency: "AUD".to_string(),
            fx_rate: Decimal::ONE,
            contract_note_ref: Some("CN001".to_string()),
            residual_brought_forward: Decimal::ZERO,
            residual_carried_forward: Decimal::ZERO,
            residual_paid_out: Decimal::ZERO,
        }
    }

    // DB-level tests

    #[tokio::test]
    async fn db_buy_trade_insert_and_retrieve() {
        let pool = test_pool().await;
        insert_test_listing(&pool).await;
        db_upsert(&pool, &buy_trade()).await.unwrap();
        let got = db_get(&pool, 1).await.unwrap().unwrap();
        assert_eq!(got.trade_type, TradeType::Buy);
        assert_eq!(got.quantity, Decimal::from(10));
        assert_eq!(got.average_price, Decimal::from(100));
        assert_eq!(got.settlement_date, NaiveDate::from_ymd_opt(2024, 1, 17).unwrap());
        assert_eq!(got.contract_note_ref, Some("CN001".to_string()));
    }

    #[tokio::test]
    async fn db_unknown_currency_rejected_on_both_currency_columns() {
        let pool = test_pool().await;
        insert_test_listing(&pool).await;

        // 'ZZZ' is not a recognised currency → each currency column's FK rejects it.
        let mut bad_currency = buy_trade();
        bad_currency.currency = "ZZZ".to_string();
        let err = db_upsert(&pool, &bad_currency).await.unwrap_err();
        assert!(err.to_string().contains("FOREIGN KEY"), "expected currency FK error, got: {err}");

        let mut bad_brokerage = buy_trade();
        bad_brokerage.brokerage_currency = "ZZZ".to_string();
        let err = db_upsert(&pool, &bad_brokerage).await.unwrap_err();
        assert!(
            err.to_string().contains("FOREIGN KEY"),
            "expected brokerage_currency FK error, got: {err}"
        );

        // A seeded digital-token code (BTC) is a recognised currency and is accepted.
        let mut btc = buy_trade();
        btc.currency = "BTC".to_string();
        db_upsert(&pool, &btc).await.unwrap();
    }

    #[tokio::test]
    async fn db_sell_trade_insert_and_retrieve() {
        let pool = test_pool().await;
        insert_test_listing(&pool).await;
        let trade = Trade {
            id: 2,
            trade_type: TradeType::Sell,
            date: NaiveDate::from_ymd_opt(2024, 6, 1).unwrap(),
            settlement_date: NaiveDate::from_ymd_opt(2024, 6, 3).unwrap(),
            listing_id: 1,
            average_price: Decimal::from(120),
            quantity: Decimal::from(5),
            currency: "AUD".to_string(),
            brokerage: "9.95".parse().unwrap(),
            gst_on_brokerage: "0.995".parse().unwrap(),
            brokerage_currency: "AUD".to_string(),
            fx_rate: Decimal::ONE,
            contract_note_ref: None,
            residual_brought_forward: Decimal::ZERO,
            residual_carried_forward: Decimal::ZERO,
            residual_paid_out: Decimal::ZERO,
        };
        db_upsert(&pool, &trade).await.unwrap();
        let got = db_get(&pool, 2).await.unwrap().unwrap();
        assert_eq!(got.trade_type, TradeType::Sell);
        assert_eq!(got.quantity, Decimal::from(5));
    }

    #[tokio::test]
    async fn db_drp_trade_insert_and_retrieve() {
        let pool = test_pool().await;
        insert_test_listing(&pool).await;
        let trade = Trade {
            id: 3,
            trade_type: TradeType::DRP,
            date: NaiveDate::from_ymd_opt(2024, 3, 15).unwrap(),
            settlement_date: NaiveDate::from_ymd_opt(2024, 3, 15).unwrap(),
            listing_id: 1,
            average_price: Decimal::from(95),
            quantity: Decimal::from(2),
            currency: "AUD".to_string(),
            brokerage: Decimal::ZERO,
            gst_on_brokerage: Decimal::ZERO,
            brokerage_currency: "AUD".to_string(),
            fx_rate: Decimal::ONE,
            contract_note_ref: None,
            residual_brought_forward: Decimal::ZERO,
            residual_carried_forward: Decimal::ZERO,
            residual_paid_out: Decimal::ZERO,
        };
        db_upsert(&pool, &trade).await.unwrap();
        let got = db_get(&pool, 3).await.unwrap().unwrap();
        assert_eq!(got.trade_type, TradeType::DRP);
        assert_eq!(got.quantity, Decimal::from(2));
    }

    #[tokio::test]
    async fn db_drp_residual_fields_round_trip_with_precision() {
        let pool = test_pool().await;
        insert_test_listing(&pool).await;
        let mut trade = buy_trade();
        trade.id = 7;
        trade.trade_type = TradeType::DRP;
        trade.residual_brought_forward = "1.234567890".parse().unwrap();
        trade.residual_carried_forward = "0.987654321".parse().unwrap();
        trade.residual_paid_out = "2.500000001".parse().unwrap();
        db_upsert(&pool, &trade).await.unwrap();
        let got = db_get(&pool, 7).await.unwrap().unwrap();
        assert_eq!(got.residual_brought_forward, "1.234567890".parse::<Decimal>().unwrap());
        assert_eq!(got.residual_carried_forward, "0.987654321".parse::<Decimal>().unwrap());
        assert_eq!(got.residual_paid_out, "2.500000001".parse::<Decimal>().unwrap());
    }

    #[tokio::test]
    async fn db_non_drp_trade_defaults_residuals_to_zero() {
        // A plain Buy carries zero residuals (residuals are a DRP-only concept).
        let pool = test_pool().await;
        insert_test_listing(&pool).await;
        db_upsert(&pool, &buy_trade()).await.unwrap();
        let got = db_get(&pool, 1).await.unwrap().unwrap();
        assert_eq!(got.residual_brought_forward, Decimal::ZERO);
        assert_eq!(got.residual_carried_forward, Decimal::ZERO);
        assert_eq!(got.residual_paid_out, Decimal::ZERO);
    }

    #[tokio::test]
    async fn db_get_missing_returns_none() {
        let pool = test_pool().await;
        assert!(db_get(&pool, 999).await.unwrap().is_none());
    }

    // API-level tests

    #[tokio::test]
    async fn api_settlement_date_auto_populated() {
        let pool = test_pool().await;
        insert_test_listing(&pool).await;
        // XASX has settlement_days = 2, so 2024-01-15 + 2 = 2024-01-17
        let body = serde_json::json!({
            "trade_type": "Buy",
            "date": "2024-01-15",
            "listing_id": 1,
            "average_price": 100.0,
            "quantity": 10.0,
            "currency": "AUD",
            "brokerage": 9.95,
            "gst_on_brokerage": 0.995,
            "brokerage_currency": "AUD",
            "fx_rate": 1.0
        });
        let resp = router()
            .with_state(pool.clone())
            .oneshot(
                Request::builder()
                    .method("PUT")
                    .uri("/trades/1")
                    .header("content-type", "application/json")
                    .body(Body::from(body.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NO_CONTENT);
        let trade = db_get(&pool, 1).await.unwrap().unwrap();
        assert_eq!(trade.settlement_date, NaiveDate::from_ymd_opt(2024, 1, 17).unwrap());
    }

    #[test]
    fn add_business_days_skips_weekend() {
        let none = HashSet::new();
        // 2024-01-18 is a Thursday; T+2 business days settles Monday 2024-01-22,
        // skipping Sat 2024-01-20 and Sun 2024-01-21.
        let thursday = NaiveDate::from_ymd_opt(2024, 1, 18).unwrap();
        assert_eq!(
            add_business_days(thursday, 2, &none),
            NaiveDate::from_ymd_opt(2024, 1, 22).unwrap()
        );
        // 2024-01-15 is a Monday; T+2 stays within the week (Wednesday).
        let monday = NaiveDate::from_ymd_opt(2024, 1, 15).unwrap();
        assert_eq!(
            add_business_days(monday, 2, &none),
            NaiveDate::from_ymd_opt(2024, 1, 17).unwrap()
        );
    }

    #[test]
    fn add_business_days_skips_public_holidays() {
        // Christmas Day (Wed) and Boxing Day (Thu) 2024 are public holidays.
        let holidays: HashSet<NaiveDate> = [
            NaiveDate::from_ymd_opt(2024, 12, 25).unwrap(),
            NaiveDate::from_ymd_opt(2024, 12, 26).unwrap(),
        ]
        .into_iter()
        .collect();
        // Tuesday 2024-12-24 + T+2: skip Wed 25 + Thu 26 (holidays), Fri 27 = 1,
        // skip the weekend, Mon 30 = 2 → settles 2024-12-30.
        let tuesday = NaiveDate::from_ymd_opt(2024, 12, 24).unwrap();
        assert_eq!(
            add_business_days(tuesday, 2, &holidays),
            NaiveDate::from_ymd_opt(2024, 12, 30).unwrap()
        );
        // Without the holiday set it would settle on Boxing Day (Thu 26).
        assert_eq!(
            add_business_days(tuesday, 2, &HashSet::new()),
            NaiveDate::from_ymd_opt(2024, 12, 26).unwrap()
        );
    }

    #[tokio::test]
    async fn api_settlement_date_skips_public_holiday() {
        let pool = test_pool().await;
        insert_test_listing(&pool).await; // listing 1 trades on XASX
        // XASX is closed Christmas (Wed 2024-12-25) and Boxing Day (Thu 2024-12-26);
        // a Tuesday 2024-12-24 buy at T+2 settles Mon 2024-12-30, not Thu 2024-12-26.
        let body = serde_json::json!({
            "trade_type": "Buy",
            "date": "2024-12-24",
            "listing_id": 1,
            "average_price": 100.0,
            "quantity": 10.0,
            "currency": "AUD",
            "brokerage": 9.95,
            "gst_on_brokerage": 0.995,
            "brokerage_currency": "AUD",
            "fx_rate": 1.0
        });
        let resp = router()
            .with_state(pool.clone())
            .oneshot(
                Request::builder()
                    .method("PUT")
                    .uri("/trades/1")
                    .header("content-type", "application/json")
                    .body(Body::from(body.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NO_CONTENT);
        let trade = db_get(&pool, 1).await.unwrap().unwrap();
        assert_eq!(trade.settlement_date, NaiveDate::from_ymd_opt(2024, 12, 30).unwrap());
    }

    #[tokio::test]
    async fn api_settlement_date_auto_populated_skips_weekend() {
        let pool = test_pool().await;
        insert_test_listing(&pool).await;
        // Friday 2024-01-19 + T+2 business days = Tuesday 2024-01-23 (skips the weekend).
        let body = serde_json::json!({
            "trade_type": "Buy",
            "date": "2024-01-19",
            "listing_id": 1,
            "average_price": 100.0,
            "quantity": 10.0,
            "currency": "AUD",
            "brokerage": 9.95,
            "gst_on_brokerage": 0.995,
            "brokerage_currency": "AUD",
            "fx_rate": 1.0
        });
        let resp = router()
            .with_state(pool.clone())
            .oneshot(
                Request::builder()
                    .method("PUT")
                    .uri("/trades/1")
                    .header("content-type", "application/json")
                    .body(Body::from(body.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NO_CONTENT);
        let trade = db_get(&pool, 1).await.unwrap().unwrap();
        assert_eq!(trade.settlement_date, NaiveDate::from_ymd_opt(2024, 1, 23).unwrap());
    }

    #[tokio::test]
    async fn api_put_sell_trade_is_rejected() {
        // Sells must go through PUT /sells/{id}; the generic trade endpoint rejects them.
        let pool = test_pool().await;
        insert_test_listing(&pool).await;
        let body = serde_json::json!({
            "trade_type": "Sell",
            "date": "2024-01-15",
            "listing_id": 1,
            "average_price": 100.0,
            "quantity": 10.0,
            "currency": "AUD",
            "brokerage": 9.95,
            "gst_on_brokerage": 0.995,
            "brokerage_currency": "AUD",
            "fx_rate": 1.0
        });
        let resp = router()
            .with_state(pool)
            .oneshot(
                Request::builder()
                    .method("PUT")
                    .uri("/trades/1")
                    .header("content-type", "application/json")
                    .body(Body::from(body.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY);
    }

    #[tokio::test]
    async fn api_settlement_date_override() {
        let pool = test_pool().await;
        insert_test_listing(&pool).await;
        let body = serde_json::json!({
            "trade_type": "Buy",
            "date": "2024-01-15",
            "settlement_date": "2024-01-20",
            "listing_id": 1,
            "average_price": 100.0,
            "quantity": 10.0,
            "currency": "AUD",
            "brokerage": 9.95,
            "gst_on_brokerage": 0.995,
            "brokerage_currency": "AUD",
            "fx_rate": 1.0
        });
        let resp = router()
            .with_state(pool.clone())
            .oneshot(
                Request::builder()
                    .method("PUT")
                    .uri("/trades/1")
                    .header("content-type", "application/json")
                    .body(Body::from(body.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NO_CONTENT);
        let trade = db_get(&pool, 1).await.unwrap().unwrap();
        assert_eq!(trade.settlement_date, NaiveDate::from_ymd_opt(2024, 1, 20).unwrap());
    }

    #[tokio::test]
    async fn api_list_returns_ok() {
        let pool = test_pool().await;
        insert_test_listing(&pool).await;
        db_upsert(&pool, &buy_trade()).await.unwrap();
        let resp = router()
            .with_state(pool)
            .oneshot(Request::builder().uri("/trades").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let trades: Vec<Trade> = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(trades.len(), 1);
        assert_eq!(trades[0].trade_type, TradeType::Buy);
    }

    #[tokio::test]
    async fn api_get_existing_returns_trade() {
        let pool = test_pool().await;
        insert_test_listing(&pool).await;
        db_upsert(&pool, &buy_trade()).await.unwrap();
        let resp = router()
            .with_state(pool)
            .oneshot(Request::builder().uri("/trades/1").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let t: Trade = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(t.trade_type, TradeType::Buy);
        assert_eq!(t.quantity, Decimal::from(10));
    }

    #[tokio::test]
    async fn api_get_missing_returns_404() {
        let pool = test_pool().await;
        let resp = router()
            .with_state(pool)
            .oneshot(Request::builder().uri("/trades/999").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn api_delete_existing_returns_no_content() {
        let pool = test_pool().await;
        insert_test_listing(&pool).await;
        db_upsert(&pool, &buy_trade()).await.unwrap();
        let resp = router()
            .with_state(pool)
            .oneshot(
                Request::builder()
                    .method("DELETE")
                    .uri("/trades/1")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NO_CONTENT);
    }

    #[tokio::test]
    async fn api_delete_missing_returns_404() {
        let pool = test_pool().await;
        let resp = router()
            .with_state(pool)
            .oneshot(
                Request::builder()
                    .method("DELETE")
                    .uri("/trades/999")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn api_decimal_precision_round_trip() {
        let pool = test_pool().await;
        insert_test_listing(&pool).await;
        let body = serde_json::json!({
            "trade_type": "Buy",
            "date": "2024-01-15",
            "listing_id": 1,
            "average_price": "99.9999999999",
            "quantity": "10.5",
            "currency": "AUD",
            "brokerage": "9.95",
            "gst_on_brokerage": "0.995",
            "brokerage_currency": "AUD",
            "fx_rate": "1.0"
        });
        let resp = router()
            .with_state(pool.clone())
            .oneshot(
                Request::builder()
                    .method("PUT")
                    .uri("/trades/1")
                    .header("content-type", "application/json")
                    .body(Body::from(body.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NO_CONTENT);
        let resp = router()
            .with_state(pool)
            .oneshot(Request::builder().uri("/trades/1").body(Body::empty()).unwrap())
            .await
            .unwrap();
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let t: Trade = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(t.average_price, "99.9999999999".parse::<Decimal>().unwrap());
        assert_eq!(t.quantity, "10.5".parse::<Decimal>().unwrap());
        assert_eq!(t.brokerage, "9.95".parse::<Decimal>().unwrap());
    }
}
