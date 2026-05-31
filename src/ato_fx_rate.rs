use axum::{
    Json, Router,
    extract::{Path, State},
    http::StatusCode,
    routing::get,
};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use sqlx::{Row, SqlitePool};

use crate::decimal::parse_dec;

/// An ATO-published monthly foreign exchange rate. `rate` is foreign currency
/// units per 1 AUD (foreign-per-AUD), so AUD = foreign / rate.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AtoFxRate {
    pub id: i64,
    pub currency: String,
    pub month: String, // 'YYYY-MM'
    pub rate: Decimal,
}

impl<'r> sqlx::FromRow<'r, sqlx::sqlite::SqliteRow> for AtoFxRate {
    fn from_row(row: &'r sqlx::sqlite::SqliteRow) -> Result<Self, sqlx::Error> {
        Ok(AtoFxRate {
            id: row.try_get("id")?,
            currency: row.try_get("currency")?,
            month: row.try_get("month")?,
            rate: parse_dec("rate", row.try_get("rate")?)?,
        })
    }
}

pub fn router() -> Router<SqlitePool> {
    // Read-only over HTTP: rows are written by the ATO FX rate import, not by clients.
    Router::new()
        .route("/ato_fx_rates", get(list))
        .route("/ato_fx_rates/{id}", get(get_one))
}

pub async fn db_list(pool: &SqlitePool) -> Result<Vec<AtoFxRate>, sqlx::Error> {
    sqlx::query_as(
        "SELECT id, currency, month, rate FROM ato_fx_rates ORDER BY currency, month",
    )
    .fetch_all(pool)
    .await
}

pub async fn db_get(pool: &SqlitePool, id: i64) -> Result<Option<AtoFxRate>, sqlx::Error> {
    sqlx::query_as("SELECT id, currency, month, rate FROM ato_fx_rates WHERE id = ?")
        .bind(id)
        .fetch_optional(pool)
        .await
}

/// Idempotently upsert a rate for a (currency, month). The UNIQUE(currency, month)
/// constraint means re-running never creates a duplicate row.
// Gated to tests until the ATO FX rate import (which will be its sole caller) lands.
#[cfg(test)]
pub async fn db_upsert(
    pool: &SqlitePool,
    currency: &str,
    month: &str,
    rate: Decimal,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO ato_fx_rates (currency, month, rate) VALUES (?, ?, ?) \
         ON CONFLICT(currency, month) DO UPDATE SET rate = excluded.rate",
    )
    .bind(currency)
    .bind(month)
    .bind(rate.to_string())
    .execute(pool)
    .await?;
    Ok(())
}

async fn list(State(pool): State<SqlitePool>) -> Result<Json<Vec<AtoFxRate>>, StatusCode> {
    db_list(&pool)
        .await
        .map(Json)
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)
}

async fn get_one(
    State(pool): State<SqlitePool>,
    Path(id): Path<i64>,
) -> Result<Json<AtoFxRate>, StatusCode> {
    db_get(&pool, id)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
        .map(Json)
        .ok_or(StatusCode::NOT_FOUND)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db;
    use axum::{body::Body, http::Request};
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    async fn test_pool() -> SqlitePool {
        db::init(":memory:").await.unwrap()
    }

    // DB-level tests

    #[tokio::test]
    async fn db_insert_and_retrieve() {
        let pool = test_pool().await;
        db_upsert(&pool, "USD", "2024-01", "1.5".parse().unwrap()).await.unwrap();
        let rows = db_list(&pool).await.unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].currency, "USD");
        assert_eq!(rows[0].month, "2024-01");
        assert_eq!(rows[0].rate, "1.5".parse::<Decimal>().unwrap());

        let got = db_get(&pool, rows[0].id).await.unwrap().unwrap();
        assert_eq!(got.currency, "USD");
    }

    #[tokio::test]
    async fn db_currency_month_uniqueness_enforced() {
        let pool = test_pool().await;
        db_upsert(&pool, "USD", "2024-01", "1.5".parse().unwrap()).await.unwrap();
        // Same (currency, month): must update in place, not create a duplicate.
        db_upsert(&pool, "USD", "2024-01", "1.6".parse().unwrap()).await.unwrap();

        let rows = db_list(&pool).await.unwrap();
        assert_eq!(rows.len(), 1, "(currency, month) must be unique");
        assert_eq!(rows[0].rate, "1.6".parse::<Decimal>().unwrap());

        // A different month is a distinct row.
        db_upsert(&pool, "USD", "2024-02", "1.7".parse().unwrap()).await.unwrap();
        assert_eq!(db_list(&pool).await.unwrap().len(), 2);
    }

    #[tokio::test]
    async fn db_decimal_precision_preserved_in_round_trip() {
        let pool = test_pool().await;
        let rate = "0.67890123456789".parse::<Decimal>().unwrap();
        db_upsert(&pool, "USD", "2024-03", rate).await.unwrap();
        let rows = db_list(&pool).await.unwrap();
        assert_eq!(rows[0].rate, rate);
    }

    // API-level tests

    #[tokio::test]
    async fn api_list_returns_rates() {
        let pool = test_pool().await;
        db_upsert(&pool, "USD", "2024-01", "1.5".parse().unwrap()).await.unwrap();
        let resp = router()
            .with_state(pool)
            .oneshot(Request::builder().uri("/ato_fx_rates").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let rates: Vec<AtoFxRate> = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(rates.len(), 1);
        assert_eq!(rates[0].currency, "USD");
    }

    #[tokio::test]
    async fn api_get_existing_returns_rate() {
        let pool = test_pool().await;
        db_upsert(&pool, "USD", "2024-01", "1.5".parse().unwrap()).await.unwrap();
        let id = db_list(&pool).await.unwrap()[0].id;
        let resp = router()
            .with_state(pool)
            .oneshot(
                Request::builder()
                    .uri(format!("/ato_fx_rates/{id}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let rate: AtoFxRate = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(rate.currency, "USD");
        assert_eq!(rate.rate, "1.5".parse::<Decimal>().unwrap());
    }

    #[tokio::test]
    async fn api_get_missing_returns_404() {
        let pool = test_pool().await;
        let resp = router()
            .with_state(pool)
            .oneshot(Request::builder().uri("/ato_fx_rates/999").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }
}
