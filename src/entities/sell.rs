//! Atomic Sell + parcel-allocation creation.
//!
//! A Sell and the purchase parcels it consumes are created together in one
//! transaction so that an under- (or over-) allocated Sell can never be
//! persisted. This is the only write path for Sell trades and their
//! allocations — the standalone `parcel_allocations` write routes are disabled
//! (see `parcel_allocation::router`) so a partial state cannot be reintroduced
//! after the fact.
//!
//! `PUT /sells/{id}` is an upsert: it replaces the Sell trade row and *all* of
//! its parcel allocations with the submitted set.

use crate::entities::trade::{self, TradeType};
use axum::{
    Json, Router,
    extract::{Path, State},
    http::StatusCode,
    routing::put,
};
use chrono::NaiveDate;
use rust_decimal::Decimal;
use serde::Deserialize;
use sqlx::SqlitePool;

#[derive(Debug, Deserialize)]
pub struct AllocationInput {
    pub purchase_trade_id: i64,
    pub quantity_allocated: Decimal,
}

#[derive(Debug, Deserialize)]
pub struct SellBody {
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
    pub allocations: Vec<AllocationInput>,
}

#[derive(Debug)]
pub enum SellError {
    Db(sqlx::Error),
    /// Allocated quantities do not sum exactly to the sell quantity.
    AllocationMismatch,
    /// A referenced purchase trade does not exist.
    PurchaseParcelMissing,
    /// A referenced purchase trade is not a Buy or DRP.
    PurchaseTradeNotBuyOrDrp,
    /// Allocating these parcels would exceed a purchase parcel's quantity.
    PurchaseQuantityExceeded,
}

impl From<sqlx::Error> for SellError {
    fn from(e: sqlx::Error) -> Self {
        SellError::Db(e)
    }
}

pub fn router() -> Router<SqlitePool> {
    Router::new().route("/sells/{id}", put(upsert).delete(delete))
}

/// Outcome of a delete request, so the handler can map to the right status.
#[derive(Debug, PartialEq)]
pub enum DeleteOutcome {
    Deleted,
    NotFound,
    /// The id refers to a trade that is not a Sell — deletion is refused so a
    /// Buy/DRP parcel can't be removed through the sells endpoint.
    NotASell,
}

/// Delete a Sell trade and all of its parcel allocations in one transaction,
/// freeing the purchase parcels those allocations consumed.
pub async fn db_delete_sell(pool: &SqlitePool, id: i64) -> Result<DeleteOutcome, sqlx::Error> {
    let mut tx = pool.begin().await?;

    let trade_type: Option<TradeType> =
        sqlx::query_scalar("SELECT trade_type FROM trades WHERE id = ?")
            .bind(id)
            .fetch_optional(&mut *tx)
            .await?;
    match trade_type {
        None => return Ok(DeleteOutcome::NotFound),
        Some(t) if t != TradeType::Sell => return Ok(DeleteOutcome::NotASell),
        Some(_) => {}
    }

    sqlx::query("DELETE FROM parcel_allocations WHERE sale_trade_id = ?")
        .bind(id)
        .execute(&mut *tx)
        .await?;
    sqlx::query("DELETE FROM trades WHERE id = ?")
        .bind(id)
        .execute(&mut *tx)
        .await?;

    tx.commit().await?;
    Ok(DeleteOutcome::Deleted)
}

/// Create or replace a Sell trade together with its full set of parcel
/// allocations, atomically. Returns an error (mapped to 422) unless the
/// allocations sum exactly to the sell quantity and every parcel is a valid,
/// not-over-allocated Buy/DRP.
pub async fn db_upsert_sell(pool: &SqlitePool, id: i64, body: &SellBody) -> Result<(), SellError> {
    // Allocations must account for the whole sale — no more, no less.
    let allocated: Decimal = body
        .allocations
        .iter()
        .map(|a| a.quantity_allocated)
        .sum();
    if allocated != body.quantity {
        return Err(SellError::AllocationMismatch);
    }

    // Reference data (exchanges/listings) is not touched here, so resolving the
    // settlement date outside the write transaction is a consistent read.
    let settlement_date = match body.settlement_date {
        Some(d) => d,
        None => {
            let days = trade::settlement_days_for_listing(pool, body.listing_id).await?;
            let holidays =
                crate::entities::exchange_holiday::exchange_holidays_for_listing(pool, body.listing_id)
                    .await?;
            let settlement = trade::add_business_days(body.date, days, &holidays);
            trade::warn_if_outside_holiday_coverage(id, body.date, settlement, &holidays);
            settlement
        }
    };

    let mut tx = pool.begin().await?;

    // Upsert the Sell trade row.
    sqlx::query(
        "INSERT INTO trades \
         (id, trade_type, date, settlement_date, listing_id, average_price, quantity, \
          currency, brokerage, gst_on_brokerage, brokerage_currency, fx_rate, contract_note_ref) \
         VALUES (?, 'Sell', ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?) \
         ON CONFLICT(id) DO UPDATE SET \
             trade_type         = 'Sell', \
             date               = excluded.date, \
             settlement_date    = excluded.settlement_date, \
             listing_id         = excluded.listing_id, \
             average_price      = excluded.average_price, \
             quantity           = excluded.quantity, \
             currency           = excluded.currency, \
             brokerage          = excluded.brokerage, \
             gst_on_brokerage   = excluded.gst_on_brokerage, \
             brokerage_currency = excluded.brokerage_currency, \
             fx_rate            = excluded.fx_rate, \
             contract_note_ref  = excluded.contract_note_ref",
    )
    .bind(id)
    .bind(body.date)
    .bind(settlement_date)
    .bind(body.listing_id)
    .bind(body.average_price.to_string())
    .bind(body.quantity.to_string())
    .bind(&body.currency)
    .bind(body.brokerage.to_string())
    .bind(body.gst_on_brokerage.to_string())
    .bind(&body.brokerage_currency)
    .bind(body.fx_rate.to_string())
    .bind(&body.contract_note_ref)
    .execute(&mut *tx)
    .await?;

    // Replace this sale's allocations wholesale.
    sqlx::query("DELETE FROM parcel_allocations WHERE sale_trade_id = ?")
        .bind(id)
        .execute(&mut *tx)
        .await?;

    for alloc in &body.allocations {
        // Purchase parcel must exist and be a Buy/DRP.
        let purchase_type: Option<TradeType> =
            sqlx::query_scalar("SELECT trade_type FROM trades WHERE id = ?")
                .bind(alloc.purchase_trade_id)
                .fetch_optional(&mut *tx)
                .await?;
        match purchase_type {
            None => return Err(SellError::PurchaseParcelMissing),
            Some(t) if !matches!(t, TradeType::Buy | TradeType::DRP) => {
                return Err(SellError::PurchaseTradeNotBuyOrDrp);
            }
            Some(_) => {}
        }

        sqlx::query(
            "INSERT INTO parcel_allocations (sale_trade_id, purchase_trade_id, quantity_allocated) \
             VALUES (?, ?, ?)",
        )
        .bind(id)
        .bind(alloc.purchase_trade_id)
        .bind(alloc.quantity_allocated.to_string())
        .execute(&mut *tx)
        .await?;

        // After inserting, the total allocated against this parcel (across all
        // sales) must not exceed the parcel's quantity.
        let purchase_qty: String = sqlx::query_scalar("SELECT quantity FROM trades WHERE id = ?")
            .bind(alloc.purchase_trade_id)
            .fetch_one(&mut *tx)
            .await?;
        let purchase_qty: Decimal = purchase_qty
            .parse()
            .map_err(|_| SellError::Db(sqlx::Error::Decode("invalid purchase quantity".into())))?;

        let rows: Vec<String> = sqlx::query_scalar(
            "SELECT quantity_allocated FROM parcel_allocations WHERE purchase_trade_id = ?",
        )
        .bind(alloc.purchase_trade_id)
        .fetch_all(&mut *tx)
        .await?;
        let total: Decimal = rows
            .into_iter()
            .filter_map(|s| s.parse::<Decimal>().ok())
            .sum();
        if total > purchase_qty {
            return Err(SellError::PurchaseQuantityExceeded);
        }
    }

    tx.commit().await?;
    Ok(())
}

async fn upsert(
    State(pool): State<SqlitePool>,
    Path(id): Path<i64>,
    Json(body): Json<SellBody>,
) -> Result<StatusCode, StatusCode> {
    match db_upsert_sell(&pool, id, &body).await {
        Ok(()) => Ok(StatusCode::NO_CONTENT),
        Err(SellError::AllocationMismatch) => Err(StatusCode::UNPROCESSABLE_ENTITY),
        Err(SellError::PurchaseParcelMissing) => Err(StatusCode::UNPROCESSABLE_ENTITY),
        Err(SellError::PurchaseTradeNotBuyOrDrp) => Err(StatusCode::UNPROCESSABLE_ENTITY),
        Err(SellError::PurchaseQuantityExceeded) => Err(StatusCode::UNPROCESSABLE_ENTITY),
        Err(SellError::Db(e)) => {
            tracing::error!(error = %e, "sell upsert failed");
            Err(StatusCode::INTERNAL_SERVER_ERROR)
        }
    }
}

async fn delete(
    State(pool): State<SqlitePool>,
    Path(id): Path<i64>,
) -> Result<StatusCode, StatusCode> {
    match db_delete_sell(&pool, id).await {
        Ok(DeleteOutcome::Deleted) => Ok(StatusCode::NO_CONTENT),
        Ok(DeleteOutcome::NotFound) => Err(StatusCode::NOT_FOUND),
        Ok(DeleteOutcome::NotASell) => Err(StatusCode::UNPROCESSABLE_ENTITY),
        Err(e) => {
            tracing::error!(error = %e, "sell delete failed");
            Err(StatusCode::INTERNAL_SERVER_ERROR)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{infra::db, entities::{listing, trade}};
    use axum::{body::Body, http::Request};
    use tower::ServiceExt;

    async fn test_pool() -> SqlitePool {
        db::init(":memory:").await.unwrap()
    }

    async fn insert_listing(pool: &SqlitePool, id: i64) {
        listing::db_upsert(
            pool,
            &listing::Listing {
                id,
                exchange_mic: "XASX".to_string(),
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

    async fn insert_buy(pool: &SqlitePool, id: i64, listing_id: i64, qty: Decimal) {
        trade::db_upsert(
            pool,
            &trade::Trade {
                id,
                trade_type: trade::TradeType::Buy,
                date: NaiveDate::from_ymd_opt(2024, 1, 1).unwrap(),
                settlement_date: NaiveDate::from_ymd_opt(2024, 1, 3).unwrap(),
                listing_id,
                average_price: Decimal::from(10),
                quantity: qty,
                currency: "AUD".to_string(),
                brokerage: Decimal::ZERO,
                gst_on_brokerage: Decimal::ZERO,
                brokerage_currency: "AUD".to_string(),
                fx_rate: Decimal::ONE,
                contract_note_ref: None,
                residual_brought_forward: Decimal::ZERO,
                residual_carried_forward: Decimal::ZERO,
                residual_paid_out: Decimal::ZERO,
            },
        )
        .await
        .unwrap();
    }

    fn sell_body(qty: Decimal, allocations: Vec<AllocationInput>) -> SellBody {
        SellBody {
            date: NaiveDate::from_ymd_opt(2024, 6, 3).unwrap(),
            settlement_date: None,
            listing_id: 1,
            average_price: Decimal::from(15),
            quantity: qty,
            currency: "AUD".to_string(),
            brokerage: Decimal::ZERO,
            gst_on_brokerage: Decimal::ZERO,
            brokerage_currency: "AUD".to_string(),
            fx_rate: Decimal::ONE,
            contract_note_ref: None,
            allocations,
        }
    }

    async fn count_allocations(pool: &SqlitePool, sale_id: i64) -> i64 {
        sqlx::query_scalar("SELECT COUNT(*) FROM parcel_allocations WHERE sale_trade_id = ?")
            .bind(sale_id)
            .fetch_one(pool)
            .await
            .unwrap()
    }

    async fn trade_exists(pool: &SqlitePool, id: i64) -> bool {
        let n: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM trades WHERE id = ?")
            .bind(id)
            .fetch_one(pool)
            .await
            .unwrap();
        n > 0
    }

    #[tokio::test]
    async fn db_fully_allocated_sell_is_persisted() {
        let pool = test_pool().await;
        insert_listing(&pool, 1).await;
        insert_buy(&pool, 1, 1, Decimal::from(100)).await;

        let body = sell_body(
            Decimal::from(100),
            vec![AllocationInput { purchase_trade_id: 1, quantity_allocated: Decimal::from(100) }],
        );
        db_upsert_sell(&pool, 2, &body).await.unwrap();

        assert!(trade_exists(&pool, 2).await);
        assert_eq!(count_allocations(&pool, 2).await, 1);
    }

    #[tokio::test]
    async fn db_under_allocated_sell_is_rejected_and_rolled_back() {
        let pool = test_pool().await;
        insert_listing(&pool, 1).await;
        insert_buy(&pool, 1, 1, Decimal::from(100)).await;

        // sell 100 but only allocate 60
        let body = sell_body(
            Decimal::from(100),
            vec![AllocationInput { purchase_trade_id: 1, quantity_allocated: Decimal::from(60) }],
        );
        let err = db_upsert_sell(&pool, 2, &body).await.unwrap_err();
        assert!(matches!(err, SellError::AllocationMismatch));
        // nothing persisted
        assert!(!trade_exists(&pool, 2).await);
        assert_eq!(count_allocations(&pool, 2).await, 0);
    }

    #[tokio::test]
    async fn db_over_allocated_sell_is_rejected() {
        let pool = test_pool().await;
        insert_listing(&pool, 1).await;
        insert_buy(&pool, 1, 1, Decimal::from(100)).await;

        // allocations sum to 120 but sell quantity is 100
        let body = sell_body(
            Decimal::from(100),
            vec![AllocationInput { purchase_trade_id: 1, quantity_allocated: Decimal::from(120) }],
        );
        let err = db_upsert_sell(&pool, 2, &body).await.unwrap_err();
        assert!(matches!(err, SellError::AllocationMismatch));
    }

    #[tokio::test]
    async fn db_allocation_exceeding_parcel_is_rejected() {
        let pool = test_pool().await;
        insert_listing(&pool, 1).await;
        insert_buy(&pool, 1, 1, Decimal::from(50)).await; // only 50 available

        // sell 100, fully allocated against a 50-unit parcel -> exceeds parcel
        let body = sell_body(
            Decimal::from(100),
            vec![AllocationInput { purchase_trade_id: 1, quantity_allocated: Decimal::from(100) }],
        );
        let err = db_upsert_sell(&pool, 2, &body).await.unwrap_err();
        assert!(matches!(err, SellError::PurchaseQuantityExceeded));
        assert!(!trade_exists(&pool, 2).await);
    }

    #[tokio::test]
    async fn db_allocation_against_non_buy_is_rejected() {
        let pool = test_pool().await;
        insert_listing(&pool, 1).await;
        insert_buy(&pool, 1, 1, Decimal::from(100)).await;
        // trade 3 is itself a Sell (created via this endpoint)
        let prior = sell_body(
            Decimal::from(100),
            vec![AllocationInput { purchase_trade_id: 1, quantity_allocated: Decimal::from(100) }],
        );
        db_upsert_sell(&pool, 3, &prior).await.unwrap();

        // now try to allocate a new sell against the Sell trade 3
        let body = sell_body(
            Decimal::from(10),
            vec![AllocationInput { purchase_trade_id: 3, quantity_allocated: Decimal::from(10) }],
        );
        let err = db_upsert_sell(&pool, 4, &body).await.unwrap_err();
        assert!(matches!(err, SellError::PurchaseTradeNotBuyOrDrp));
    }

    #[tokio::test]
    async fn db_upsert_replaces_previous_allocations() {
        let pool = test_pool().await;
        insert_listing(&pool, 1).await;
        insert_buy(&pool, 1, 1, Decimal::from(100)).await;
        insert_buy(&pool, 2, 1, Decimal::from(100)).await;

        // first version: 100 from parcel 1
        let v1 = sell_body(
            Decimal::from(100),
            vec![AllocationInput { purchase_trade_id: 1, quantity_allocated: Decimal::from(100) }],
        );
        db_upsert_sell(&pool, 3, &v1).await.unwrap();
        assert_eq!(count_allocations(&pool, 3).await, 1);

        // revised: split across two parcels — old allocation should be gone
        let v2 = sell_body(
            Decimal::from(100),
            vec![
                AllocationInput { purchase_trade_id: 1, quantity_allocated: Decimal::from(40) },
                AllocationInput { purchase_trade_id: 2, quantity_allocated: Decimal::from(60) },
            ],
        );
        db_upsert_sell(&pool, 3, &v2).await.unwrap();
        assert_eq!(count_allocations(&pool, 3).await, 2);
    }

    #[tokio::test]
    async fn api_fully_allocated_sell_returns_204() {
        let pool = test_pool().await;
        insert_listing(&pool, 1).await;
        insert_buy(&pool, 1, 1, Decimal::from(100)).await;

        let body = serde_json::json!({
            "date": "2024-06-03",
            "listing_id": 1,
            "average_price": "15",
            "quantity": "100",
            "currency": "AUD",
            "brokerage": "0",
            "gst_on_brokerage": "0",
            "brokerage_currency": "AUD",
            "fx_rate": "1",
            "allocations": [ { "purchase_trade_id": 1, "quantity_allocated": "100" } ]
        });
        let resp = router()
            .with_state(pool)
            .oneshot(
                Request::builder()
                    .method("PUT")
                    .uri("/sells/2")
                    .header("content-type", "application/json")
                    .body(Body::from(body.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NO_CONTENT);
    }

    #[tokio::test]
    async fn db_delete_removes_sell_and_allocations() {
        let pool = test_pool().await;
        insert_listing(&pool, 1).await;
        insert_buy(&pool, 1, 1, Decimal::from(100)).await;
        let body = sell_body(
            Decimal::from(100),
            vec![AllocationInput { purchase_trade_id: 1, quantity_allocated: Decimal::from(100) }],
        );
        db_upsert_sell(&pool, 2, &body).await.unwrap();
        assert_eq!(count_allocations(&pool, 2).await, 1);

        let outcome = db_delete_sell(&pool, 2).await.unwrap();
        assert_eq!(outcome, DeleteOutcome::Deleted);
        assert!(!trade_exists(&pool, 2).await);
        assert_eq!(count_allocations(&pool, 2).await, 0);
        // the purchase parcel itself is untouched
        assert!(trade_exists(&pool, 1).await);
    }

    #[tokio::test]
    async fn db_delete_missing_sell_is_not_found() {
        let pool = test_pool().await;
        assert_eq!(db_delete_sell(&pool, 99).await.unwrap(), DeleteOutcome::NotFound);
    }

    #[tokio::test]
    async fn db_delete_non_sell_is_refused() {
        let pool = test_pool().await;
        insert_listing(&pool, 1).await;
        insert_buy(&pool, 1, 1, Decimal::from(100)).await;
        // trade 1 is a Buy — deleting it via the sells endpoint is refused
        assert_eq!(db_delete_sell(&pool, 1).await.unwrap(), DeleteOutcome::NotASell);
        assert!(trade_exists(&pool, 1).await);
    }

    #[tokio::test]
    async fn api_delete_sell_returns_204_then_404() {
        let pool = test_pool().await;
        insert_listing(&pool, 1).await;
        insert_buy(&pool, 1, 1, Decimal::from(100)).await;
        let body = sell_body(
            Decimal::from(100),
            vec![AllocationInput { purchase_trade_id: 1, quantity_allocated: Decimal::from(100) }],
        );
        db_upsert_sell(&pool, 2, &body).await.unwrap();

        let app = router().with_state(pool.clone());
        let resp = app
            .oneshot(
                Request::builder()
                    .method("DELETE")
                    .uri("/sells/2")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NO_CONTENT);

        // second delete: already gone
        let resp = router()
            .with_state(pool)
            .oneshot(
                Request::builder()
                    .method("DELETE")
                    .uri("/sells/2")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn api_under_allocated_sell_returns_422() {
        let pool = test_pool().await;
        insert_listing(&pool, 1).await;
        insert_buy(&pool, 1, 1, Decimal::from(100)).await;

        let body = serde_json::json!({
            "date": "2024-06-03",
            "listing_id": 1,
            "average_price": "15",
            "quantity": "100",
            "currency": "AUD",
            "brokerage": "0",
            "gst_on_brokerage": "0",
            "brokerage_currency": "AUD",
            "fx_rate": "1",
            "allocations": [ { "purchase_trade_id": 1, "quantity_allocated": "60" } ]
        });
        let resp = router()
            .with_state(pool)
            .oneshot(
                Request::builder()
                    .method("PUT")
                    .uri("/sells/2")
                    .header("content-type", "application/json")
                    .body(Body::from(body.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY);
    }
}
