use axum::{
    Json, Router,
    extract::{Path, State},
    http::StatusCode,
    routing::get,
};
use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, sqlx::Type)]
pub enum SecurityType {
    Share,
    ETF,
    LIC,
    Trust,
}

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct Listing {
    pub id: i64,
    pub exchange_mic: String,
    pub ticker: String,
    pub name: String,
    pub isin: Option<String>,
    pub security_type: SecurityType,
    pub currency: String,
    pub amit: bool,
}

#[derive(Debug, Deserialize)]
pub struct ListingBody {
    pub exchange_mic: String,
    pub ticker: String,
    pub name: String,
    pub isin: Option<String>,
    pub security_type: SecurityType,
    pub currency: String,
    pub amit: bool,
}

pub fn router() -> Router<SqlitePool> {
    Router::new()
        .route("/listings", get(list))
        .route("/listings/{id}", get(get_one).put(upsert).delete(delete))
}

pub async fn db_list(pool: &SqlitePool) -> Result<Vec<Listing>, sqlx::Error> {
    sqlx::query_as(
        "SELECT id, exchange_mic, ticker, name, isin, security_type, currency, amit \
         FROM listings ORDER BY exchange_mic, ticker",
    )
    .fetch_all(pool)
    .await
}

pub async fn db_get(pool: &SqlitePool, id: i64) -> Result<Option<Listing>, sqlx::Error> {
    sqlx::query_as(
        "SELECT id, exchange_mic, ticker, name, isin, security_type, currency, amit \
         FROM listings WHERE id = ?",
    )
    .bind(id)
    .fetch_optional(pool)
    .await
}

pub async fn db_upsert(pool: &SqlitePool, listing: &Listing) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO listings (id, exchange_mic, ticker, name, isin, security_type, currency, amit) \
         VALUES (?, ?, ?, ?, ?, ?, ?, ?) \
         ON CONFLICT(id) DO UPDATE SET \
             exchange_mic  = excluded.exchange_mic, \
             ticker        = excluded.ticker, \
             name          = excluded.name, \
             isin          = excluded.isin, \
             security_type = excluded.security_type, \
             currency      = excluded.currency, \
             amit          = excluded.amit",
    )
    .bind(listing.id)
    .bind(&listing.exchange_mic)
    .bind(&listing.ticker)
    .bind(&listing.name)
    .bind(&listing.isin)
    .bind(listing.security_type)
    .bind(listing.currency.as_str())
    .bind(listing.amit)
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn db_delete(pool: &SqlitePool, id: i64) -> Result<bool, sqlx::Error> {
    let result = sqlx::query("DELETE FROM listings WHERE id = ?")
        .bind(id)
        .execute(pool)
        .await?;
    Ok(result.rows_affected() > 0)
}

async fn list(State(pool): State<SqlitePool>) -> Result<Json<Vec<Listing>>, StatusCode> {
    db_list(&pool)
        .await
        .map(Json)
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)
}

async fn get_one(
    State(pool): State<SqlitePool>,
    Path(id): Path<i64>,
) -> Result<Json<Listing>, StatusCode> {
    db_get(&pool, id)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
        .map(Json)
        .ok_or(StatusCode::NOT_FOUND)
}

async fn upsert(
    State(pool): State<SqlitePool>,
    Path(id): Path<i64>,
    Json(body): Json<ListingBody>,
) -> Result<StatusCode, StatusCode> {
    let listing = Listing {
        id,
        exchange_mic: body.exchange_mic,
        ticker: body.ticker,
        name: body.name,
        isin: body.isin,
        security_type: body.security_type,
        currency: body.currency,
        amit: body.amit,
    };
    db_upsert(&pool, &listing)
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
    use crate::infra::db;
    use axum::{body::Body, http::Request};
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    async fn test_pool() -> SqlitePool {
        db::init(":memory:").await.unwrap()
    }

    fn xtest() -> Listing {
        Listing {
            id: 1,
            exchange_mic: "XASX".to_string(),
            ticker: "VAS".to_string(),
            name: "Vanguard Australian Shares ETF".to_string(),
            isin: Some("AU0000VASAU4".to_string()),
            security_type: SecurityType::ETF,
            currency: "AUD".to_string(),
            amit: true,
        }
    }

    // DB-level tests

    #[tokio::test]
    async fn db_insert_and_retrieve() {
        let pool = test_pool().await;
        db_upsert(&pool, &xtest()).await.unwrap();
        let got = db_get(&pool, 1).await.unwrap().unwrap();
        assert_eq!(got.ticker, "VAS");
        assert_eq!(got.exchange_mic, "XASX");
        assert_eq!(got.isin, Some("AU0000VASAU4".to_string()));
        assert!(got.amit);
    }

    #[tokio::test]
    async fn db_get_missing_returns_none() {
        let pool = test_pool().await;
        assert!(db_get(&pool, 999).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn db_upsert_updates_existing() {
        let pool = test_pool().await;
        db_upsert(&pool, &xtest()).await.unwrap();
        let mut updated = xtest();
        updated.name = "Updated ETF".to_string();
        updated.amit = false;
        db_upsert(&pool, &updated).await.unwrap();
        let got = db_get(&pool, 1).await.unwrap().unwrap();
        assert_eq!(got.name, "Updated ETF");
        assert!(!got.amit);
    }

    #[tokio::test]
    async fn db_delete_removes_listing() {
        let pool = test_pool().await;
        db_upsert(&pool, &xtest()).await.unwrap();
        assert!(db_delete(&pool, 1).await.unwrap());
        assert!(db_get(&pool, 1).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn db_delete_missing_returns_false() {
        let pool = test_pool().await;
        assert!(!db_delete(&pool, 999).await.unwrap());
    }

    #[tokio::test]
    async fn db_fk_constraint_rejects_unknown_exchange() {
        let pool = test_pool().await;
        let mut bad = xtest();
        bad.exchange_mic = "XXXX".to_string();
        let err = db_upsert(&pool, &bad).await.unwrap_err();
        assert!(
            err.to_string().contains("FOREIGN KEY"),
            "expected FK error, got: {err}"
        );
    }

    // API-level tests

    #[tokio::test]
    async fn api_list_returns_ok() {
        let pool = test_pool().await;
        db_upsert(&pool, &xtest()).await.unwrap();
        let resp = router()
            .with_state(pool)
            .oneshot(Request::builder().uri("/listings").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let listings: Vec<Listing> = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(listings.len(), 1);
        assert_eq!(listings[0].ticker, "VAS");
    }

    #[tokio::test]
    async fn api_get_existing_returns_listing() {
        let pool = test_pool().await;
        db_upsert(&pool, &xtest()).await.unwrap();
        let resp = router()
            .with_state(pool)
            .oneshot(Request::builder().uri("/listings/1").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let l: Listing = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(l.ticker, "VAS");
    }

    #[tokio::test]
    async fn api_get_missing_returns_404() {
        let pool = test_pool().await;
        let resp = router()
            .with_state(pool)
            .oneshot(Request::builder().uri("/listings/999").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn api_upsert_creates_listing() {
        let pool = test_pool().await;
        let body = serde_json::json!({
            "exchange_mic": "XASX",
            "ticker": "VAS",
            "name": "Vanguard Australian Shares ETF",
            "isin": null,
            "security_type": "ETF",
            "currency": "AUD",
            "amit": true
        });
        let resp = router()
            .with_state(pool.clone())
            .oneshot(
                Request::builder()
                    .method("PUT")
                    .uri("/listings/1")
                    .header("content-type", "application/json")
                    .body(Body::from(body.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NO_CONTENT);
        assert!(db_get(&pool, 1).await.unwrap().is_some());
    }

    #[tokio::test]
    async fn api_upsert_updates_listing() {
        let pool = test_pool().await;
        db_upsert(&pool, &xtest()).await.unwrap();
        let body = serde_json::json!({
            "exchange_mic": "XASX",
            "ticker": "VAS",
            "name": "Renamed ETF",
            "isin": null,
            "security_type": "ETF",
            "currency": "AUD",
            "amit": false
        });
        let resp = router()
            .with_state(pool.clone())
            .oneshot(
                Request::builder()
                    .method("PUT")
                    .uri("/listings/1")
                    .header("content-type", "application/json")
                    .body(Body::from(body.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NO_CONTENT);
        let got = db_get(&pool, 1).await.unwrap().unwrap();
        assert_eq!(got.name, "Renamed ETF");
        assert!(!got.amit);
    }

    #[tokio::test]
    async fn api_delete_existing_returns_no_content() {
        let pool = test_pool().await;
        db_upsert(&pool, &xtest()).await.unwrap();
        let resp = router()
            .with_state(pool)
            .oneshot(
                Request::builder()
                    .method("DELETE")
                    .uri("/listings/1")
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
                    .uri("/listings/999")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }
}
