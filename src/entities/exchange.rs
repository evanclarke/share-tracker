use axum::{
    Json, Router,
    extract::{Path, State},
    http::StatusCode,
    routing::get,
};
use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct Exchange {
    pub mic: String,
    pub name: String,
    pub country: String,
    pub currency: String,
    pub timezone: String,
    pub settlement_days: i64,
}

#[derive(Debug, Deserialize)]
pub struct ExchangeBody {
    pub name: String,
    pub country: String,
    pub currency: String,
    pub timezone: String,
    pub settlement_days: i64,
}

pub fn router() -> Router<SqlitePool> {
    Router::new()
        .route("/exchanges", get(list))
        .route("/exchanges/{mic}", get(get_one).put(upsert).delete(delete))
}

pub async fn db_list(pool: &SqlitePool) -> Result<Vec<Exchange>, sqlx::Error> {
    sqlx::query_as(
        "SELECT mic, name, country, currency, timezone, settlement_days \
         FROM exchanges ORDER BY mic",
    )
    .fetch_all(pool)
    .await
}

pub async fn db_get(pool: &SqlitePool, mic: &str) -> Result<Option<Exchange>, sqlx::Error> {
    sqlx::query_as(
        "SELECT mic, name, country, currency, timezone, settlement_days \
         FROM exchanges WHERE mic = ?",
    )
    .bind(mic)
    .fetch_optional(pool)
    .await
}

pub async fn db_upsert(pool: &SqlitePool, exchange: &Exchange) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO exchanges (mic, name, country, currency, timezone, settlement_days) \
         VALUES (?, ?, ?, ?, ?, ?) \
         ON CONFLICT(mic) DO UPDATE SET \
             name = excluded.name, \
             country = excluded.country, \
             currency = excluded.currency, \
             timezone = excluded.timezone, \
             settlement_days = excluded.settlement_days",
    )
    .bind(&exchange.mic)
    .bind(&exchange.name)
    .bind(&exchange.country)
    .bind(&exchange.currency)
    .bind(&exchange.timezone)
    .bind(exchange.settlement_days)
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn db_delete(pool: &SqlitePool, mic: &str) -> Result<bool, sqlx::Error> {
    let result = sqlx::query("DELETE FROM exchanges WHERE mic = ?")
        .bind(mic)
        .execute(pool)
        .await?;
    Ok(result.rows_affected() > 0)
}

async fn list(State(pool): State<SqlitePool>) -> Result<Json<Vec<Exchange>>, StatusCode> {
    db_list(&pool)
        .await
        .map(Json)
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)
}

async fn get_one(
    State(pool): State<SqlitePool>,
    Path(mic): Path<String>,
) -> Result<Json<Exchange>, StatusCode> {
    db_get(&pool, &mic)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
        .map(Json)
        .ok_or(StatusCode::NOT_FOUND)
}

async fn upsert(
    State(pool): State<SqlitePool>,
    Path(mic): Path<String>,
    Json(body): Json<ExchangeBody>,
) -> Result<StatusCode, StatusCode> {
    let exchange = Exchange {
        mic,
        name: body.name,
        country: body.country,
        currency: body.currency,
        timezone: body.timezone,
        settlement_days: body.settlement_days,
    };
    db_upsert(&pool, &exchange)
        .await
        .map(|_| StatusCode::NO_CONTENT)
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)
}

async fn delete(
    State(pool): State<SqlitePool>,
    Path(mic): Path<String>,
) -> Result<StatusCode, StatusCode> {
    db_delete(&pool, &mic)
        .await
        .map(|found| if found { StatusCode::NO_CONTENT } else { StatusCode::NOT_FOUND })
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::infra::db;
    use axum::{body::Body, http::Request};
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    async fn test_pool() -> SqlitePool {
        db::init(":memory:").await.unwrap()
    }

    fn xtest() -> Exchange {
        Exchange {
            mic: "XTES".to_string(),
            name: "Test Exchange".to_string(),
            country: "Testland".to_string(),
            currency: "TST".to_string(),
            timezone: "UTC".to_string(),
            settlement_days: 2,
        }
    }

    // DB-level tests

    #[tokio::test]
    async fn db_insert_and_retrieve() {
        let pool = test_pool().await;
        db_upsert(&pool, &xtest()).await.unwrap();
        let got = db_get(&pool, "XTES").await.unwrap().unwrap();
        assert_eq!(got.name, "Test Exchange");
        assert_eq!(got.settlement_days, 2);
    }

    #[tokio::test]
    async fn db_get_missing_returns_none() {
        let pool = test_pool().await;
        assert!(db_get(&pool, "XXXX").await.unwrap().is_none());
    }

    #[tokio::test]
    async fn db_upsert_updates_existing() {
        let pool = test_pool().await;
        db_upsert(&pool, &xtest()).await.unwrap();
        let mut updated = xtest();
        updated.name = "Updated Exchange".to_string();
        db_upsert(&pool, &updated).await.unwrap();
        let got = db_get(&pool, "XTES").await.unwrap().unwrap();
        assert_eq!(got.name, "Updated Exchange");
    }

    #[tokio::test]
    async fn db_delete_removes_exchange() {
        let pool = test_pool().await;
        db_upsert(&pool, &xtest()).await.unwrap();
        assert!(db_delete(&pool, "XTES").await.unwrap());
        assert!(db_get(&pool, "XTES").await.unwrap().is_none());
    }

    #[tokio::test]
    async fn db_delete_missing_returns_false() {
        let pool = test_pool().await;
        assert!(!db_delete(&pool, "XXXX").await.unwrap());
    }

    #[tokio::test]
    async fn seed_data_has_xasx_and_xnys() {
        let pool = test_pool().await;
        let asx = db_get(&pool, "XASX").await.unwrap().unwrap();
        assert_eq!(asx.currency, "AUD");
        assert_eq!(asx.settlement_days, 2);
        assert!(db_get(&pool, "XNYS").await.unwrap().is_some());
    }

    // API-level tests

    #[tokio::test]
    async fn api_list_includes_seed_exchanges() {
        let pool = test_pool().await;
        let resp = router()
            .with_state(pool)
            .oneshot(Request::builder().uri("/exchanges").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let exchanges: Vec<Exchange> = serde_json::from_slice(&bytes).unwrap();
        assert!(exchanges.iter().any(|e| e.mic == "XASX"));
        assert!(exchanges.iter().any(|e| e.mic == "XNYS"));
    }

    #[tokio::test]
    async fn api_get_existing_returns_exchange() {
        let pool = test_pool().await;
        let resp = router()
            .with_state(pool)
            .oneshot(Request::builder().uri("/exchanges/XASX").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let ex: Exchange = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(ex.mic, "XASX");
    }

    #[tokio::test]
    async fn api_get_missing_returns_404() {
        let pool = test_pool().await;
        let resp = router()
            .with_state(pool)
            .oneshot(Request::builder().uri("/exchanges/XXXX").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn api_upsert_creates_exchange() {
        let pool = test_pool().await;
        let body = serde_json::json!({
            "name": "Test Exchange",
            "country": "Testland",
            "currency": "TST",
            "timezone": "UTC",
            "settlement_days": 2
        });
        let resp = router()
            .with_state(pool.clone())
            .oneshot(
                Request::builder()
                    .method("PUT")
                    .uri("/exchanges/XTES")
                    .header("content-type", "application/json")
                    .body(Body::from(body.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NO_CONTENT);
        assert!(db_get(&pool, "XTES").await.unwrap().is_some());
    }

    #[tokio::test]
    async fn api_upsert_updates_exchange() {
        let pool = test_pool().await;
        db_upsert(&pool, &xtest()).await.unwrap();
        let body = serde_json::json!({
            "name": "Renamed Exchange",
            "country": "Testland",
            "currency": "TST",
            "timezone": "UTC",
            "settlement_days": 3
        });
        let resp = router()
            .with_state(pool.clone())
            .oneshot(
                Request::builder()
                    .method("PUT")
                    .uri("/exchanges/XTES")
                    .header("content-type", "application/json")
                    .body(Body::from(body.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NO_CONTENT);
        let got = db_get(&pool, "XTES").await.unwrap().unwrap();
        assert_eq!(got.name, "Renamed Exchange");
        assert_eq!(got.settlement_days, 3);
    }

    #[tokio::test]
    async fn api_delete_existing_returns_no_content() {
        let pool = test_pool().await;
        let resp = router()
            .with_state(pool)
            .oneshot(
                Request::builder()
                    .method("DELETE")
                    .uri("/exchanges/XASX")
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
                    .uri("/exchanges/XXXX")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }
}
