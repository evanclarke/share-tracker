use crate::infra::decimal::Money;
#[cfg(test)]
use crate::infra::decimal::parse_dec;
use crate::infra::http::{self, CrudEntity};
use axum::{Router, routing::get};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;
// Only the test-fixture write path below reads columns off a raw row.
#[cfg(test)]
use sqlx::Row;

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct ParcelAllocation {
    pub id: i64,
    pub sale_trade_id: i64,
    pub purchase_trade_id: i64,
    #[sqlx(try_from = "Money")]
    pub quantity_allocated: Decimal,
}

/// Parcel allocations are read-only over HTTP. They are created and replaced
/// atomically together with their Sell trade via `PUT /sells/{id}` (see
/// `sell` module); allowing standalone writes here would let a Sell become
/// under-covered (e.g. deleting or shrinking an allocation), breaking the
/// invariant that every persisted Sell is fully allocated.
impl CrudEntity for ParcelAllocation {
    type Key = i64;
    const TABLE: &'static str = "parcel_allocations";
    const COLUMNS: &'static str = "id, sale_trade_id, purchase_trade_id, quantity_allocated";
    const NOUN: &'static str = "parcel allocation";
}

pub fn router() -> Router<SqlitePool> {
    Router::new()
        .route(
            "/parcel_allocations",
            get(http::list_handler::<ParcelAllocation>),
        )
        .route(
            "/parcel_allocations/{id}",
            get(http::get_handler::<ParcelAllocation>),
        )
}

#[cfg(test)]
pub async fn db_get(pool: &SqlitePool, id: i64) -> Result<Option<ParcelAllocation>, sqlx::Error> {
    http::crud_get(pool, id).await
}

// The write path below is retained only as a test-fixture builder for the
// report modules (and this module's own validation tests). Allocations are no
// longer writable over HTTP — they are managed atomically via `PUT /sells/{id}`.
#[cfg(test)]
#[derive(thiserror::Error, Debug)]
pub enum UpsertError {
    #[error("parcel allocation write failed: {0}")]
    Db(#[from] sqlx::Error),
    #[error("the sale trade is not a Sell")]
    SaleTradeNotSell,
    #[error("the purchase trade is not a Buy or DRP")]
    PurchaseTradeNotBuyOrDrp,
    #[error("the allocation exceeds the purchase parcel's quantity")]
    PurchaseQuantityExceeded,
    #[error("the allocations exceed the sale's quantity")]
    SaleQuantityExceeded,
}

#[cfg(test)]
async fn sum_allocated(
    pool: &SqlitePool,
    column: &str,
    trade_id: i64,
    exclude_id: i64,
) -> Result<Decimal, sqlx::Error> {
    let rows: Vec<String> = sqlx::query_scalar(sqlx::AssertSqlSafe(format!(
        "SELECT quantity_allocated FROM parcel_allocations WHERE {column} = ? AND id != ?"
    )))
    .bind(trade_id)
    .bind(exclude_id)
    .fetch_all(pool)
    .await?;
    let total = rows
        .into_iter()
        .filter_map(|s| s.parse::<Decimal>().ok())
        .fold(Decimal::ZERO, |acc, v| acc + v);
    Ok(total)
}

#[cfg(test)]
pub async fn db_upsert(
    pool: &SqlitePool,
    allocation: &ParcelAllocation,
) -> Result<(), UpsertError> {
    use crate::entities::corporate_action;
    use crate::entities::trade::TradeType;
    use chrono::NaiveDate;

    let sale_row = sqlx::query("SELECT trade_type, date, quantity FROM trades WHERE id = ?")
        .bind(allocation.sale_trade_id)
        .fetch_one(pool)
        .await?;
    let sale_type: TradeType = sale_row.try_get("trade_type")?;
    if sale_type != TradeType::Sell {
        return Err(UpsertError::SaleTradeNotSell);
    }
    let sale_date: NaiveDate = sale_row.try_get("date")?;

    let purchase_row =
        sqlx::query("SELECT trade_type, date, listing_id, quantity FROM trades WHERE id = ?")
            .bind(allocation.purchase_trade_id)
            .fetch_one(pool)
            .await?;
    let purchase_type: TradeType = purchase_row.try_get("trade_type")?;
    if !purchase_type.is_acquisition() {
        return Err(UpsertError::PurchaseTradeNotBuyOrDrp);
    }
    let purchase_date: NaiveDate = purchase_row.try_get("date")?;
    let purchase_listing: i64 = purchase_row.try_get("listing_id")?;
    let purchase_qty: Decimal = parse_dec("quantity", purchase_row.try_get("quantity")?)?;

    // The parcel's quantity is in as-acquired units while the allocation is in
    // sale-date units: re-base across any share splits/consolidations between
    // them (TD 2000/10). Mirrors the live `PUT /sells` path's check.
    let splits = corporate_action::db_splits_for_listing(pool, purchase_listing).await?;
    let alloc_acquired = corporate_action::as_acquired_quantity(
        allocation.quantity_allocated,
        &splits,
        purchase_date,
        sale_date,
    );

    let already_purchase_allocated = {
        let rows = sqlx::query(
            "SELECT pa.quantity_allocated, s.date AS sale_date \
             FROM parcel_allocations pa JOIN trades s ON s.id = pa.sale_trade_id \
             WHERE pa.purchase_trade_id = ? AND pa.id != ?",
        )
        .bind(allocation.purchase_trade_id)
        .bind(allocation.id)
        .fetch_all(pool)
        .await?;
        let mut total = Decimal::ZERO;
        for row in &rows {
            let qty: Decimal = parse_dec("quantity_allocated", row.try_get("quantity_allocated")?)?;
            let d: NaiveDate = row.try_get("sale_date")?;
            total += corporate_action::as_acquired_quantity(qty, &splits, purchase_date, d);
        }
        total
    };
    if already_purchase_allocated + alloc_acquired > purchase_qty {
        return Err(UpsertError::PurchaseQuantityExceeded);
    }

    let sale_qty: String = sqlx::query_scalar("SELECT quantity FROM trades WHERE id = ?")
        .bind(allocation.sale_trade_id)
        .fetch_one(pool)
        .await?;
    let sale_qty: Decimal = parse_dec("quantity", sale_qty)?;

    let already_sale_allocated = sum_allocated(
        pool,
        "sale_trade_id",
        allocation.sale_trade_id,
        allocation.id,
    )
    .await?;
    if already_sale_allocated + allocation.quantity_allocated > sale_qty {
        return Err(UpsertError::SaleQuantityExceeded);
    }

    sqlx::query(
        "INSERT INTO parcel_allocations (id, sale_trade_id, purchase_trade_id, quantity_allocated) \
         VALUES (?, ?, ?, ?) \
         ON CONFLICT(id) DO UPDATE SET \
             sale_trade_id      = excluded.sale_trade_id, \
             purchase_trade_id  = excluded.purchase_trade_id, \
             quantity_allocated = excluded.quantity_allocated",
    )
    .bind(allocation.id)
    .bind(allocation.sale_trade_id)
    .bind(allocation.purchase_trade_id)
    .bind(Money(allocation.quantity_allocated))
    .execute(pool)
    .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{self, ApiClient, dec, test_pool, ymd};
    use axum::http::StatusCode;

    /// Client over this module's own routes.
    fn client(pool: &SqlitePool) -> ApiClient {
        ApiClient::over(router().with_state(pool.clone()))
    }

    async fn insert_test_listing(pool: &SqlitePool) {
        test_support::listing(1)
            .ticker("VAS")
            .name("Vanguard Australian Shares ETF")
            .insert(pool)
            .await;
    }

    async fn insert_buy_trade(pool: &SqlitePool, id: i64, quantity: Decimal) {
        test_support::buy(id, 1)
            .date(ymd(2024, 1, 15))
            .qty(quantity)
            .price(Decimal::from(100))
            .brokerage(dec("9.95"))
            .gst_on_brokerage(dec("0.995"))
            .insert(pool)
            .await;
    }

    async fn insert_drp_trade(pool: &SqlitePool, id: i64, quantity: Decimal) {
        test_support::drp(id, 1)
            .date(ymd(2024, 3, 15))
            .settlement(ymd(2024, 3, 15))
            .qty(quantity)
            .price(Decimal::from(95))
            .insert(pool)
            .await;
    }

    async fn insert_sell_trade(pool: &SqlitePool, id: i64, quantity: Decimal) {
        test_support::sell(id, 1)
            .date(ymd(2024, 6, 1))
            .qty(quantity)
            .price(Decimal::from(120))
            .brokerage(dec("9.95"))
            .gst_on_brokerage(dec("0.995"))
            .insert(pool)
            .await;
    }

    // DB-level tests

    #[tokio::test]
    async fn db_allocation_insert_and_retrieve() {
        let pool = test_pool().await;
        insert_test_listing(&pool).await;
        insert_buy_trade(&pool, 1, Decimal::from(10)).await;
        insert_sell_trade(&pool, 2, Decimal::from(5)).await;

        let alloc = ParcelAllocation {
            id: 1,
            sale_trade_id: 2,
            purchase_trade_id: 1,
            quantity_allocated: Decimal::from(5),
        };
        db_upsert(&pool, &alloc).await.unwrap();

        let got = db_get(&pool, 1).await.unwrap().unwrap();
        assert_eq!(got.sale_trade_id, 2);
        assert_eq!(got.purchase_trade_id, 1);
        assert_eq!(got.quantity_allocated, Decimal::from(5));
    }

    #[tokio::test]
    async fn db_over_allocation_on_purchase_rejected() {
        let pool = test_pool().await;
        insert_test_listing(&pool).await;
        insert_buy_trade(&pool, 1, Decimal::from(10)).await;
        insert_sell_trade(&pool, 2, Decimal::from(15)).await;

        let alloc = ParcelAllocation {
            id: 1,
            sale_trade_id: 2,
            purchase_trade_id: 1,
            quantity_allocated: Decimal::from(11),
        };
        let err = db_upsert(&pool, &alloc).await.unwrap_err();
        assert!(matches!(err, UpsertError::PurchaseQuantityExceeded));
    }

    #[tokio::test]
    async fn db_over_allocation_on_sale_rejected() {
        let pool = test_pool().await;
        insert_test_listing(&pool).await;
        insert_buy_trade(&pool, 1, Decimal::from(10)).await;
        insert_sell_trade(&pool, 2, Decimal::from(3)).await;

        let alloc = ParcelAllocation {
            id: 1,
            sale_trade_id: 2,
            purchase_trade_id: 1,
            quantity_allocated: Decimal::from(5),
        };
        let err = db_upsert(&pool, &alloc).await.unwrap_err();
        assert!(matches!(err, UpsertError::SaleQuantityExceeded));
    }

    #[tokio::test]
    async fn db_cumulative_purchase_over_allocation_rejected() {
        let pool = test_pool().await;
        insert_test_listing(&pool).await;
        insert_buy_trade(&pool, 1, Decimal::from(10)).await;
        insert_sell_trade(&pool, 2, Decimal::from(8)).await;
        insert_sell_trade(&pool, 3, Decimal::from(8)).await;

        // First allocation: 8 of 10 purchase units
        db_upsert(
            &pool,
            &ParcelAllocation {
                id: 1,
                sale_trade_id: 2,
                purchase_trade_id: 1,
                quantity_allocated: Decimal::from(8),
            },
        )
        .await
        .unwrap();

        // Second allocation: 3 more from same purchase would exceed 10
        let err = db_upsert(
            &pool,
            &ParcelAllocation {
                id: 2,
                sale_trade_id: 3,
                purchase_trade_id: 1,
                quantity_allocated: Decimal::from(3),
            },
        )
        .await
        .unwrap_err();
        assert!(matches!(err, UpsertError::PurchaseQuantityExceeded));
    }

    #[tokio::test]
    async fn db_cumulative_sale_over_allocation_rejected() {
        let pool = test_pool().await;
        insert_test_listing(&pool).await;
        insert_buy_trade(&pool, 1, Decimal::from(10)).await;
        insert_buy_trade(&pool, 2, Decimal::from(10)).await;
        insert_sell_trade(&pool, 3, Decimal::from(5)).await;

        db_upsert(
            &pool,
            &ParcelAllocation {
                id: 1,
                sale_trade_id: 3,
                purchase_trade_id: 1,
                quantity_allocated: Decimal::from(4),
            },
        )
        .await
        .unwrap();

        let err = db_upsert(
            &pool,
            &ParcelAllocation {
                id: 2,
                sale_trade_id: 3,
                purchase_trade_id: 2,
                quantity_allocated: Decimal::from(3),
            },
        )
        .await
        .unwrap_err();
        assert!(matches!(err, UpsertError::SaleQuantityExceeded));
    }

    #[tokio::test]
    async fn db_sale_trade_not_sell_rejected() {
        let pool = test_pool().await;
        insert_test_listing(&pool).await;
        insert_buy_trade(&pool, 1, Decimal::from(10)).await;
        insert_buy_trade(&pool, 2, Decimal::from(10)).await;

        let err = db_upsert(
            &pool,
            &ParcelAllocation {
                id: 1,
                sale_trade_id: 1, // Buy, not Sell
                purchase_trade_id: 2,
                quantity_allocated: Decimal::from(5),
            },
        )
        .await
        .unwrap_err();
        assert!(matches!(err, UpsertError::SaleTradeNotSell));
    }

    #[tokio::test]
    async fn db_purchase_trade_not_buy_or_drp_rejected() {
        let pool = test_pool().await;
        insert_test_listing(&pool).await;
        insert_sell_trade(&pool, 1, Decimal::from(10)).await;
        insert_sell_trade(&pool, 2, Decimal::from(10)).await;

        let err = db_upsert(
            &pool,
            &ParcelAllocation {
                id: 1,
                sale_trade_id: 1,
                purchase_trade_id: 2, // Sell, not Buy/DRP
                quantity_allocated: Decimal::from(5),
            },
        )
        .await
        .unwrap_err();
        assert!(matches!(err, UpsertError::PurchaseTradeNotBuyOrDrp));
    }

    #[tokio::test]
    async fn db_drp_trade_valid_as_purchase() {
        let pool = test_pool().await;
        insert_test_listing(&pool).await;
        insert_drp_trade(&pool, 1, Decimal::from(5)).await;
        insert_sell_trade(&pool, 2, Decimal::from(5)).await;

        db_upsert(
            &pool,
            &ParcelAllocation {
                id: 1,
                sale_trade_id: 2,
                purchase_trade_id: 1,
                quantity_allocated: Decimal::from(5),
            },
        )
        .await
        .unwrap();
    }

    // API-level tests

    #[tokio::test]
    async fn api_put_allocation_route_is_not_allowed() {
        // Allocations are read-only over HTTP; writes go through PUT /sells/{id}.
        let pool = test_pool().await;
        let body = serde_json::json!({
            "sale_trade_id": 2,
            "purchase_trade_id": 1,
            "quantity_allocated": "5"
        });
        let resp = client(&pool).put("/parcel_allocations/1", &body).await;
        assert_eq!(resp.status, StatusCode::METHOD_NOT_ALLOWED);
    }

    #[tokio::test]
    async fn api_delete_allocation_route_is_not_allowed() {
        let pool = test_pool().await;
        let resp = client(&pool).delete("/parcel_allocations/1").await;
        assert_eq!(resp.status, StatusCode::METHOD_NOT_ALLOWED);
    }

    #[tokio::test]
    async fn api_list_returns_ok() {
        let pool = test_pool().await;
        insert_test_listing(&pool).await;
        insert_buy_trade(&pool, 1, Decimal::from(10)).await;
        insert_sell_trade(&pool, 2, Decimal::from(5)).await;
        db_upsert(
            &pool,
            &ParcelAllocation {
                id: 1,
                sale_trade_id: 2,
                purchase_trade_id: 1,
                quantity_allocated: Decimal::from(5),
            },
        )
        .await
        .unwrap();

        let resp = client(&pool).get("/parcel_allocations").await;
        assert_eq!(resp.status, StatusCode::OK);
        let allocs: Vec<ParcelAllocation> = resp.json();
        assert_eq!(allocs.len(), 1);
        assert_eq!(allocs[0].quantity_allocated, Decimal::from(5));
    }

    #[tokio::test]
    async fn api_get_missing_returns_404() {
        let pool = test_pool().await;
        let resp = client(&pool).get("/parcel_allocations/999").await;
        assert_eq!(resp.status, StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn api_decimal_precision_round_trip() {
        let pool = test_pool().await;
        insert_test_listing(&pool).await;
        insert_buy_trade(&pool, 1, "10.5".parse().unwrap()).await;
        insert_sell_trade(&pool, 2, "10.5".parse().unwrap()).await;
        db_upsert(
            &pool,
            &ParcelAllocation {
                id: 1,
                sale_trade_id: 2,
                purchase_trade_id: 1,
                quantity_allocated: "10.5".parse().unwrap(),
            },
        )
        .await
        .unwrap();

        let resp = client(&pool).get("/parcel_allocations/1").await;
        let alloc: ParcelAllocation = resp.json();
        assert_eq!(alloc.quantity_allocated, "10.5".parse::<Decimal>().unwrap());
    }
}
