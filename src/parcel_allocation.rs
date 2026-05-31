use axum::{
    Json, Router,
    extract::{Path, State},
    http::StatusCode,
    routing::get,
};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use sqlx::{Row, SqlitePool};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ParcelAllocation {
    pub id: i64,
    pub sale_trade_id: i64,
    pub purchase_trade_id: i64,
    pub quantity_allocated: Decimal,
}

impl<'r> sqlx::FromRow<'r, sqlx::sqlite::SqliteRow> for ParcelAllocation {
    fn from_row(row: &'r sqlx::sqlite::SqliteRow) -> Result<Self, sqlx::Error> {
        fn dec(s: String) -> Result<Decimal, sqlx::Error> {
            s.parse().map_err(|e: rust_decimal::Error| sqlx::Error::Decode(Box::new(e)))
        }
        Ok(ParcelAllocation {
            id: row.try_get("id")?,
            sale_trade_id: row.try_get("sale_trade_id")?,
            purchase_trade_id: row.try_get("purchase_trade_id")?,
            quantity_allocated: dec(row.try_get("quantity_allocated")?)?,
        })
    }
}

#[derive(Debug, Deserialize)]
pub struct ParcelAllocationBody {
    pub sale_trade_id: i64,
    pub purchase_trade_id: i64,
    pub quantity_allocated: Decimal,
}

pub fn router() -> Router<SqlitePool> {
    Router::new()
        .route("/parcel_allocations", get(list))
        .route("/parcel_allocations/{id}", get(get_one).put(upsert).delete(delete))
}

pub async fn db_list(pool: &SqlitePool) -> Result<Vec<ParcelAllocation>, sqlx::Error> {
    sqlx::query_as(
        "SELECT id, sale_trade_id, purchase_trade_id, quantity_allocated \
         FROM parcel_allocations ORDER BY id",
    )
    .fetch_all(pool)
    .await
}

pub async fn db_get(pool: &SqlitePool, id: i64) -> Result<Option<ParcelAllocation>, sqlx::Error> {
    sqlx::query_as(
        "SELECT id, sale_trade_id, purchase_trade_id, quantity_allocated \
         FROM parcel_allocations WHERE id = ?",
    )
    .bind(id)
    .fetch_optional(pool)
    .await
}

#[derive(Debug)]
pub enum UpsertError {
    Db(sqlx::Error),
    PurchaseQuantityExceeded,
    SaleQuantityExceeded,
}

impl From<sqlx::Error> for UpsertError {
    fn from(e: sqlx::Error) -> Self {
        UpsertError::Db(e)
    }
}

async fn sum_allocated(
    pool: &SqlitePool,
    column: &str,
    trade_id: i64,
    exclude_id: i64,
) -> Result<Decimal, sqlx::Error> {
    let rows: Vec<String> = sqlx::query_scalar(&format!(
        "SELECT quantity_allocated FROM parcel_allocations WHERE {column} = ? AND id != ?"
    ))
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

pub async fn db_upsert(pool: &SqlitePool, allocation: &ParcelAllocation) -> Result<(), UpsertError> {
    let purchase_qty: String = sqlx::query_scalar("SELECT quantity FROM trades WHERE id = ?")
        .bind(allocation.purchase_trade_id)
        .fetch_one(pool)
        .await?;
    let purchase_qty: Decimal = purchase_qty
        .parse()
        .map_err(|_| UpsertError::Db(sqlx::Error::Decode("invalid purchase quantity".into())))?;

    let already_purchase_allocated =
        sum_allocated(pool, "purchase_trade_id", allocation.purchase_trade_id, allocation.id).await?;
    if already_purchase_allocated + allocation.quantity_allocated > purchase_qty {
        return Err(UpsertError::PurchaseQuantityExceeded);
    }

    let sale_qty: String = sqlx::query_scalar("SELECT quantity FROM trades WHERE id = ?")
        .bind(allocation.sale_trade_id)
        .fetch_one(pool)
        .await?;
    let sale_qty: Decimal = sale_qty
        .parse()
        .map_err(|_| UpsertError::Db(sqlx::Error::Decode("invalid sale quantity".into())))?;

    let already_sale_allocated =
        sum_allocated(pool, "sale_trade_id", allocation.sale_trade_id, allocation.id).await?;
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
    .bind(allocation.quantity_allocated.to_string())
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn db_delete(pool: &SqlitePool, id: i64) -> Result<bool, sqlx::Error> {
    let result = sqlx::query("DELETE FROM parcel_allocations WHERE id = ?")
        .bind(id)
        .execute(pool)
        .await?;
    Ok(result.rows_affected() > 0)
}

async fn list(State(pool): State<SqlitePool>) -> Result<Json<Vec<ParcelAllocation>>, StatusCode> {
    db_list(&pool)
        .await
        .map(Json)
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)
}

async fn get_one(
    State(pool): State<SqlitePool>,
    Path(id): Path<i64>,
) -> Result<Json<ParcelAllocation>, StatusCode> {
    db_get(&pool, id)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
        .map(Json)
        .ok_or(StatusCode::NOT_FOUND)
}

async fn upsert(
    State(pool): State<SqlitePool>,
    Path(id): Path<i64>,
    Json(body): Json<ParcelAllocationBody>,
) -> Result<StatusCode, StatusCode> {
    let allocation = ParcelAllocation {
        id,
        sale_trade_id: body.sale_trade_id,
        purchase_trade_id: body.purchase_trade_id,
        quantity_allocated: body.quantity_allocated,
    };
    match db_upsert(&pool, &allocation).await {
        Ok(()) => Ok(StatusCode::NO_CONTENT),
        Err(UpsertError::PurchaseQuantityExceeded) => Err(StatusCode::UNPROCESSABLE_ENTITY),
        Err(UpsertError::SaleQuantityExceeded) => Err(StatusCode::UNPROCESSABLE_ENTITY),
        Err(UpsertError::Db(_)) => Err(StatusCode::INTERNAL_SERVER_ERROR),
    }
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
    use crate::{db, listing, trade};
    use axum::{body::Body, http::Request};
    use chrono::NaiveDate;
    use http_body_util::BodyExt;
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

    async fn insert_buy_trade(pool: &SqlitePool, id: i64, quantity: Decimal) {
        trade::db_upsert(
            pool,
            &trade::Trade {
                id,
                trade_type: trade::TradeType::Buy,
                date: NaiveDate::from_ymd_opt(2024, 1, 15).unwrap(),
                settlement_date: NaiveDate::from_ymd_opt(2024, 1, 17).unwrap(),
                listing_id: 1,
                average_price: Decimal::from(100),
                quantity,
                currency: "AUD".to_string(),
                brokerage: "9.95".parse().unwrap(),
                gst_on_brokerage: "0.995".parse().unwrap(),
                brokerage_currency: "AUD".to_string(),
                fx_rate: Decimal::ONE,
                contract_note_ref: None,
            },
        )
        .await
        .unwrap();
    }

    async fn insert_sell_trade(pool: &SqlitePool, id: i64, quantity: Decimal) {
        trade::db_upsert(
            pool,
            &trade::Trade {
                id,
                trade_type: trade::TradeType::Sell,
                date: NaiveDate::from_ymd_opt(2024, 6, 1).unwrap(),
                settlement_date: NaiveDate::from_ymd_opt(2024, 6, 3).unwrap(),
                listing_id: 1,
                average_price: Decimal::from(120),
                quantity,
                currency: "AUD".to_string(),
                brokerage: "9.95".parse().unwrap(),
                gst_on_brokerage: "0.995".parse().unwrap(),
                brokerage_currency: "AUD".to_string(),
                fx_rate: Decimal::ONE,
                contract_note_ref: None,
            },
        )
        .await
        .unwrap();
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
        db_upsert(&pool, &ParcelAllocation {
            id: 1,
            sale_trade_id: 2,
            purchase_trade_id: 1,
            quantity_allocated: Decimal::from(8),
        })
        .await
        .unwrap();

        // Second allocation: 3 more from same purchase would exceed 10
        let err = db_upsert(&pool, &ParcelAllocation {
            id: 2,
            sale_trade_id: 3,
            purchase_trade_id: 1,
            quantity_allocated: Decimal::from(3),
        })
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

        db_upsert(&pool, &ParcelAllocation {
            id: 1,
            sale_trade_id: 3,
            purchase_trade_id: 1,
            quantity_allocated: Decimal::from(4),
        })
        .await
        .unwrap();

        let err = db_upsert(&pool, &ParcelAllocation {
            id: 2,
            sale_trade_id: 3,
            purchase_trade_id: 2,
            quantity_allocated: Decimal::from(3),
        })
        .await
        .unwrap_err();
        assert!(matches!(err, UpsertError::SaleQuantityExceeded));
    }

    // API-level tests

    #[tokio::test]
    async fn api_allocation_creation() {
        let pool = test_pool().await;
        insert_test_listing(&pool).await;
        insert_buy_trade(&pool, 1, Decimal::from(10)).await;
        insert_sell_trade(&pool, 2, Decimal::from(5)).await;

        let body = serde_json::json!({
            "sale_trade_id": 2,
            "purchase_trade_id": 1,
            "quantity_allocated": "5"
        });
        let resp = router()
            .with_state(pool.clone())
            .oneshot(
                Request::builder()
                    .method("PUT")
                    .uri("/parcel_allocations/1")
                    .header("content-type", "application/json")
                    .body(Body::from(body.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NO_CONTENT);

        let got = db_get(&pool, 1).await.unwrap().unwrap();
        assert_eq!(got.quantity_allocated, Decimal::from(5));
    }

    #[tokio::test]
    async fn api_over_allocation_returns_422() {
        let pool = test_pool().await;
        insert_test_listing(&pool).await;
        insert_buy_trade(&pool, 1, Decimal::from(10)).await;
        insert_sell_trade(&pool, 2, Decimal::from(5)).await;

        let body = serde_json::json!({
            "sale_trade_id": 2,
            "purchase_trade_id": 1,
            "quantity_allocated": "11"
        });
        let resp = router()
            .with_state(pool)
            .oneshot(
                Request::builder()
                    .method("PUT")
                    .uri("/parcel_allocations/1")
                    .header("content-type", "application/json")
                    .body(Body::from(body.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY);
    }

    #[tokio::test]
    async fn api_list_returns_ok() {
        let pool = test_pool().await;
        insert_test_listing(&pool).await;
        insert_buy_trade(&pool, 1, Decimal::from(10)).await;
        insert_sell_trade(&pool, 2, Decimal::from(5)).await;
        db_upsert(&pool, &ParcelAllocation {
            id: 1,
            sale_trade_id: 2,
            purchase_trade_id: 1,
            quantity_allocated: Decimal::from(5),
        })
        .await
        .unwrap();

        let resp = router()
            .with_state(pool)
            .oneshot(Request::builder().uri("/parcel_allocations").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let allocs: Vec<ParcelAllocation> = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(allocs.len(), 1);
        assert_eq!(allocs[0].quantity_allocated, Decimal::from(5));
    }

    #[tokio::test]
    async fn api_get_missing_returns_404() {
        let pool = test_pool().await;
        let resp = router()
            .with_state(pool)
            .oneshot(Request::builder().uri("/parcel_allocations/999").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn api_delete_existing_returns_no_content() {
        let pool = test_pool().await;
        insert_test_listing(&pool).await;
        insert_buy_trade(&pool, 1, Decimal::from(10)).await;
        insert_sell_trade(&pool, 2, Decimal::from(5)).await;
        db_upsert(&pool, &ParcelAllocation {
            id: 1,
            sale_trade_id: 2,
            purchase_trade_id: 1,
            quantity_allocated: Decimal::from(5),
        })
        .await
        .unwrap();

        let resp = router()
            .with_state(pool)
            .oneshot(
                Request::builder()
                    .method("DELETE")
                    .uri("/parcel_allocations/1")
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
                    .uri("/parcel_allocations/999")
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
        insert_buy_trade(&pool, 1, "10.5".parse().unwrap()).await;
        insert_sell_trade(&pool, 2, "10.5".parse().unwrap()).await;

        let body = serde_json::json!({
            "sale_trade_id": 2,
            "purchase_trade_id": 1,
            "quantity_allocated": "10.5"
        });
        router()
            .with_state(pool.clone())
            .oneshot(
                Request::builder()
                    .method("PUT")
                    .uri("/parcel_allocations/1")
                    .header("content-type", "application/json")
                    .body(Body::from(body.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();

        let resp = router()
            .with_state(pool)
            .oneshot(Request::builder().uri("/parcel_allocations/1").body(Body::empty()).unwrap())
            .await
            .unwrap();
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let alloc: ParcelAllocation = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(alloc.quantity_allocated, "10.5".parse::<Decimal>().unwrap());
    }
}
