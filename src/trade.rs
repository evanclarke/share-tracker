use axum::{
    Json, Router,
    extract::{Path, State},
    http::StatusCode,
    routing::get,
};
use chrono::NaiveDate;
use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct Trade {
    pub id: i64,
    pub trade_type: String,
    pub date: NaiveDate,
    pub settlement_date: NaiveDate,
    pub listing_id: i64,
    pub average_price: f64,
    pub quantity: f64,
    pub currency: String,
    pub brokerage: f64,
    pub gst_on_brokerage: f64,
    pub brokerage_currency: String,
    pub fx_rate: f64,
    pub contract_note_ref: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct TradeBody {
    pub trade_type: String,
    pub date: NaiveDate,
    #[serde(default)]
    pub settlement_date: Option<NaiveDate>,
    pub listing_id: i64,
    pub average_price: f64,
    pub quantity: f64,
    pub currency: String,
    pub brokerage: f64,
    pub gst_on_brokerage: f64,
    pub brokerage_currency: String,
    pub fx_rate: f64,
    #[serde(default)]
    pub contract_note_ref: Option<String>,
}

pub fn router() -> Router<SqlitePool> {
    Router::new()
        .route("/trades", get(list))
        .route("/trades/{id}", get(get_one).put(upsert).delete(delete))
}

pub async fn db_list(pool: &SqlitePool) -> Result<Vec<Trade>, sqlx::Error> {
    sqlx::query_as(
        "SELECT id, trade_type, date, settlement_date, listing_id, average_price, quantity, \
         currency, brokerage, gst_on_brokerage, brokerage_currency, fx_rate, contract_note_ref \
         FROM trades ORDER BY date, id",
    )
    .fetch_all(pool)
    .await
}

pub async fn db_get(pool: &SqlitePool, id: i64) -> Result<Option<Trade>, sqlx::Error> {
    sqlx::query_as(
        "SELECT id, trade_type, date, settlement_date, listing_id, average_price, quantity, \
         currency, brokerage, gst_on_brokerage, brokerage_currency, fx_rate, contract_note_ref \
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
          currency, brokerage, gst_on_brokerage, brokerage_currency, fx_rate, contract_note_ref) \
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?) \
         ON CONFLICT(id) DO UPDATE SET \
             trade_type         = excluded.trade_type, \
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
    .bind(trade.id)
    .bind(&trade.trade_type)
    .bind(trade.date)
    .bind(trade.settlement_date)
    .bind(trade.listing_id)
    .bind(trade.average_price)
    .bind(trade.quantity)
    .bind(&trade.currency)
    .bind(trade.brokerage)
    .bind(trade.gst_on_brokerage)
    .bind(&trade.brokerage_currency)
    .bind(trade.fx_rate)
    .bind(&trade.contract_note_ref)
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

async fn settlement_days_for_listing(pool: &SqlitePool, listing_id: i64) -> Result<i64, sqlx::Error> {
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
    let settlement_date = match body.settlement_date {
        Some(d) => d,
        None => {
            let days = settlement_days_for_listing(&pool, body.listing_id)
                .await
                .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
            body.date + chrono::Duration::days(days)
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
    };
    db_upsert(&pool, &trade)
        .await
        .map(|_| StatusCode::NO_CONTENT)
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)
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
    use crate::{db, listing};
    use axum::{body::Body, http::Request};
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
                security_type: "ETF".to_string(),
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
            trade_type: "Buy".to_string(),
            date: NaiveDate::from_ymd_opt(2024, 1, 15).unwrap(),
            settlement_date: NaiveDate::from_ymd_opt(2024, 1, 17).unwrap(),
            listing_id: 1,
            average_price: 100.0,
            quantity: 10.0,
            currency: "AUD".to_string(),
            brokerage: 9.95,
            gst_on_brokerage: 0.995,
            brokerage_currency: "AUD".to_string(),
            fx_rate: 1.0,
            contract_note_ref: Some("CN001".to_string()),
        }
    }

    // DB-level tests

    #[tokio::test]
    async fn db_buy_trade_insert_and_retrieve() {
        let pool = test_pool().await;
        insert_test_listing(&pool).await;
        db_upsert(&pool, &buy_trade()).await.unwrap();
        let got = db_get(&pool, 1).await.unwrap().unwrap();
        assert_eq!(got.trade_type, "Buy");
        assert_eq!(got.quantity, 10.0);
        assert_eq!(got.average_price, 100.0);
        assert_eq!(got.settlement_date, NaiveDate::from_ymd_opt(2024, 1, 17).unwrap());
        assert_eq!(got.contract_note_ref, Some("CN001".to_string()));
    }

    #[tokio::test]
    async fn db_sell_trade_insert_and_retrieve() {
        let pool = test_pool().await;
        insert_test_listing(&pool).await;
        let trade = Trade {
            id: 2,
            trade_type: "Sell".to_string(),
            date: NaiveDate::from_ymd_opt(2024, 6, 1).unwrap(),
            settlement_date: NaiveDate::from_ymd_opt(2024, 6, 3).unwrap(),
            listing_id: 1,
            average_price: 120.0,
            quantity: 5.0,
            currency: "AUD".to_string(),
            brokerage: 9.95,
            gst_on_brokerage: 0.995,
            brokerage_currency: "AUD".to_string(),
            fx_rate: 1.0,
            contract_note_ref: None,
        };
        db_upsert(&pool, &trade).await.unwrap();
        let got = db_get(&pool, 2).await.unwrap().unwrap();
        assert_eq!(got.trade_type, "Sell");
        assert_eq!(got.quantity, 5.0);
    }

    #[tokio::test]
    async fn db_drp_trade_insert_and_retrieve() {
        let pool = test_pool().await;
        insert_test_listing(&pool).await;
        let trade = Trade {
            id: 3,
            trade_type: "DRP".to_string(),
            date: NaiveDate::from_ymd_opt(2024, 3, 15).unwrap(),
            settlement_date: NaiveDate::from_ymd_opt(2024, 3, 15).unwrap(),
            listing_id: 1,
            average_price: 95.0,
            quantity: 2.0,
            currency: "AUD".to_string(),
            brokerage: 0.0,
            gst_on_brokerage: 0.0,
            brokerage_currency: "AUD".to_string(),
            fx_rate: 1.0,
            contract_note_ref: None,
        };
        db_upsert(&pool, &trade).await.unwrap();
        let got = db_get(&pool, 3).await.unwrap().unwrap();
        assert_eq!(got.trade_type, "DRP");
        assert_eq!(got.quantity, 2.0);
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
        assert_eq!(trades[0].trade_type, "Buy");
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
        assert_eq!(t.trade_type, "Buy");
        assert_eq!(t.quantity, 10.0);
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
}
