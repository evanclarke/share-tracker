use crate::infra::http::{self, ApiError, CrudEntity};
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
    /// Local-time end of the regular trading session (`HH:MM`, in `timezone`).
    /// The price-import job only collects a day's closing price once this time
    /// has passed in the exchange's timezone.
    pub close_time: String,
}

#[derive(Debug, Deserialize)]
pub struct ExchangeBody {
    pub name: String,
    pub country: String,
    pub currency: String,
    pub timezone: String,
    pub settlement_days: i64,
    #[serde(default = "default_close_time")]
    pub close_time: String,
}

fn default_close_time() -> String {
    "16:00".to_string()
}

impl CrudEntity for Exchange {
    /// Keyed by MIC, not a rowid.
    type Key = String;
    const TABLE: &'static str = "exchanges";
    const COLUMNS: &'static str =
        "mic, name, country, currency, timezone, settlement_days, close_time";
    const KEY_COLUMN: &'static str = "mic";
    const ORDER_BY: &'static str = "mic";
    const NOUN: &'static str = "exchange";
}

pub fn router() -> Router<SqlitePool> {
    Router::new()
        .route("/exchanges", get(http::list_handler::<Exchange>))
        .route(
            "/exchanges/{mic}",
            get(http::get_handler::<Exchange>)
                .put(upsert)
                // Deleting an exchange still referenced by listings/holidays
                // violates an FK → 422.
                .delete(http::delete_handler::<Exchange>),
        )
}

/// Executor-generic for the same reason [`listing::db_get`] is: the trading
/// calendar has to be readable on a write path's own transaction.
///
/// [`listing::db_get`]: crate::entities::listing::db_get
pub async fn db_get<'e, X>(executor: X, mic: &str) -> Result<Option<Exchange>, sqlx::Error>
where
    X: sqlx::Executor<'e, Database = sqlx::Sqlite>,
{
    http::crud_get(executor, mic.to_string()).await
}

pub async fn db_upsert(pool: &SqlitePool, exchange: &Exchange) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO exchanges (mic, name, country, currency, timezone, settlement_days, close_time) \
         VALUES (?, ?, ?, ?, ?, ?, ?) \
         ON CONFLICT(mic) DO UPDATE SET \
             name = excluded.name, \
             country = excluded.country, \
             currency = excluded.currency, \
             timezone = excluded.timezone, \
             settlement_days = excluded.settlement_days, \
             close_time = excluded.close_time",
    )
    .bind(&exchange.mic)
    .bind(&exchange.name)
    .bind(&exchange.country)
    .bind(&exchange.currency)
    .bind(&exchange.timezone)
    .bind(exchange.settlement_days)
    .bind(&exchange.close_time)
    .execute(pool)
    .await?;
    Ok(())
}

#[cfg(test)]
pub async fn db_delete(pool: &SqlitePool, mic: &str) -> Result<bool, sqlx::Error> {
    http::crud_delete::<Exchange>(pool, mic.to_string()).await
}

async fn upsert(
    State(pool): State<SqlitePool>,
    Path(mic): Path<String>,
    Json(body): Json<ExchangeBody>,
) -> Result<StatusCode, ApiError> {
    let exchange = Exchange {
        mic,
        name: body.name,
        country: body.country,
        currency: body.currency,
        timezone: body.timezone,
        settlement_days: body.settlement_days,
        close_time: body.close_time,
    };
    db_upsert(&pool, &exchange)
        .await
        .map(|_| StatusCode::NO_CONTENT)
        .map_err(ApiError::from)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{ApiClient, test_pool};

    fn client(pool: &SqlitePool) -> ApiClient {
        ApiClient::over(router().with_state(pool.clone()))
    }

    fn xtest() -> Exchange {
        Exchange {
            mic: "XTES".to_string(),
            name: "Test Exchange".to_string(),
            country: "Testland".to_string(),
            currency: "AUD".to_string(),
            timezone: "UTC".to_string(),
            settlement_days: 2,
            close_time: "16:00".to_string(),
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
        let exchanges: Vec<Exchange> = client(&pool).get_json("/exchanges").await;
        assert!(exchanges.iter().any(|e| e.mic == "XASX"));
        assert!(exchanges.iter().any(|e| e.mic == "XNYS"));
    }

    #[tokio::test]
    async fn api_get_existing_returns_exchange() {
        let pool = test_pool().await;
        let ex: Exchange = client(&pool).get_json("/exchanges/XASX").await;
        assert_eq!(ex.mic, "XASX");
    }

    #[tokio::test]
    async fn api_get_missing_returns_404() {
        let pool = test_pool().await;
        let resp = client(&pool).get("/exchanges/XXXX").await;
        assert_eq!(resp.status, StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn api_upsert_creates_exchange() {
        let pool = test_pool().await;
        let body = serde_json::json!({
            "name": "Test Exchange",
            "country": "Testland",
            "currency": "AUD",
            "timezone": "UTC",
            "settlement_days": 2
        });
        client(&pool).put_ok("/exchanges/XTES", &body).await;
        assert!(db_get(&pool, "XTES").await.unwrap().is_some());
    }

    #[tokio::test]
    async fn api_upsert_updates_exchange() {
        let pool = test_pool().await;
        db_upsert(&pool, &xtest()).await.unwrap();
        let body = serde_json::json!({
            "name": "Renamed Exchange",
            "country": "Testland",
            "currency": "AUD",
            "timezone": "UTC",
            "settlement_days": 3
        });
        client(&pool).put_ok("/exchanges/XTES", &body).await;
        let got = db_get(&pool, "XTES").await.unwrap().unwrap();
        assert_eq!(got.name, "Renamed Exchange");
        assert_eq!(got.settlement_days, 3);
    }

    #[tokio::test]
    async fn api_delete_existing_returns_no_content() {
        let pool = test_pool().await;
        // Delete a fresh exchange with no dependents — the seeded XASX/XNYS now
        // have child rows in exchange_holidays, so their delete is FK-blocked.
        db_upsert(&pool, &xtest()).await.unwrap();
        let resp = client(&pool).delete("/exchanges/XTES").await;
        assert_eq!(resp.status, StatusCode::NO_CONTENT);
    }

    #[tokio::test]
    async fn api_delete_missing_returns_404() {
        let pool = test_pool().await;
        let resp = client(&pool).delete("/exchanges/XXXX").await;
        assert_eq!(resp.status, StatusCode::NOT_FOUND);
    }
}
