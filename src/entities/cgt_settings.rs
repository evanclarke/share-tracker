//! CGT settings: the opening carried-forward capital loss.
//!
//! A singleton row (the table CHECKs `id = 1`) holding the net capital loss
//! carried forward from years before the first year recorded in the system, so a
//! user migrating mid-history doesn't have to re-enter pre-system loss years.
//! The net-capital-gain report uses it as the starting brought-forward balance
//! when chaining unused losses across its year series (losses carry forward
//! indefinitely, per `docs/ato/cgt-using-capital-losses.md`). Absent row = zero.

use crate::infra::decimal::Money;
use crate::infra::http::{self, ApiError, CrudEntity};
use axum::{
    Json, Router,
    extract::{Path, State},
    http::StatusCode,
    routing::get,
};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct CgtSettings {
    /// Always 1 — the table CHECKs it, so at most one settings row exists.
    pub id: i64,
    /// Net capital loss carried forward from before the first recorded year
    /// (a non-negative amount, AUD).
    #[sqlx(try_from = "Money")]
    pub opening_capital_loss: Decimal,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CgtSettingsBody {
    pub opening_capital_loss: Decimal,
}

impl CrudEntity for CgtSettings {
    type Key = i64;
    const TABLE: &'static str = "cgt_settings";
    const COLUMNS: &'static str = "id, opening_capital_loss";
    const NOUN: &'static str = "CGT settings row";
}

pub fn router() -> Router<SqlitePool> {
    Router::new()
        .route("/cgt_settings", get(http::list_handler::<CgtSettings>))
        .route(
            "/cgt_settings/{id}",
            get(http::get_handler::<CgtSettings>)
                .put(upsert)
                .delete(http::delete_handler::<CgtSettings>),
        )
}

#[cfg(test)]
pub async fn db_get(pool: &SqlitePool, id: i64) -> Result<Option<CgtSettings>, sqlx::Error> {
    http::crud_get(pool, id).await
}

pub async fn db_upsert(pool: &SqlitePool, settings: &CgtSettings) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO cgt_settings (id, opening_capital_loss) VALUES (?, ?) \
         ON CONFLICT(id) DO UPDATE SET opening_capital_loss = excluded.opening_capital_loss",
    )
    .bind(settings.id)
    .bind(Money(settings.opening_capital_loss))
    .execute(pool)
    .await?;
    Ok(())
}

/// The opening carried-forward capital loss, or zero when no settings row exists.
/// Used by the net-capital-gain report as the starting loss balance.
/// Executor-generic so it can run on a pool or inside the report's own read
/// transaction.
pub async fn db_opening_capital_loss<'e, E>(executor: E) -> Result<Decimal, sqlx::Error>
where
    E: sqlx::Executor<'e, Database = sqlx::Sqlite>,
{
    let settings: Option<CgtSettings> =
        sqlx::query_as("SELECT id, opening_capital_loss FROM cgt_settings WHERE id = 1")
            .fetch_optional(executor)
            .await?;
    Ok(settings.map_or(Decimal::ZERO, |s| s.opening_capital_loss))
}

async fn upsert(
    State(pool): State<SqlitePool>,
    Path(id): Path<i64>,
    Json(body): Json<CgtSettingsBody>,
) -> Result<StatusCode, ApiError> {
    // A negative opening loss is meaningless (losses are stored as positive
    // amounts); reject at write time so the report never consumes one.
    if body.opening_capital_loss < Decimal::ZERO {
        return Err(ApiError::unprocessable(
            "the opening capital loss cannot be negative (losses are stored as positive amounts)",
        ));
    }
    let settings = CgtSettings {
        id,
        opening_capital_loss: body.opening_capital_loss,
    };
    db_upsert(&pool, &settings)
        .await
        .map(|_| StatusCode::NO_CONTENT)
        // id != 1 violates the singleton CHECK → 422.
        .map_err(ApiError::from)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{ApiClient, test_pool};

    /// Client over this module's own routes.
    fn client(pool: &SqlitePool) -> ApiClient {
        ApiClient::over(router().with_state(pool.clone()))
    }

    #[tokio::test]
    async fn db_insert_and_retrieve_preserves_precision() {
        let pool = test_pool().await;
        let loss: Decimal = "1234.5678".parse().unwrap();
        db_upsert(
            &pool,
            &CgtSettings {
                id: 1,
                opening_capital_loss: loss,
            },
        )
        .await
        .unwrap();
        let got = db_get(&pool, 1).await.unwrap().unwrap();
        assert_eq!(got.id, 1);
        assert_eq!(got.opening_capital_loss, loss);
    }

    #[tokio::test]
    async fn db_upsert_updates_singleton_row() {
        let pool = test_pool().await;
        db_upsert(
            &pool,
            &CgtSettings {
                id: 1,
                opening_capital_loss: Decimal::from(100),
            },
        )
        .await
        .unwrap();
        db_upsert(
            &pool,
            &CgtSettings {
                id: 1,
                opening_capital_loss: Decimal::from(250),
            },
        )
        .await
        .unwrap();
        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM cgt_settings")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(count, 1);
        assert_eq!(
            db_opening_capital_loss(&pool).await.unwrap(),
            Decimal::from(250)
        );
    }

    #[tokio::test]
    async fn db_singleton_check_rejects_other_ids() {
        let pool = test_pool().await;
        let err = db_upsert(
            &pool,
            &CgtSettings {
                id: 2,
                opening_capital_loss: Decimal::ONE,
            },
        )
        .await;
        assert!(
            err.is_err(),
            "CHECK (id = 1) should reject a second settings row"
        );
    }

    #[tokio::test]
    async fn db_opening_loss_defaults_to_zero_when_unset() {
        let pool = test_pool().await;
        assert_eq!(db_opening_capital_loss(&pool).await.unwrap(), Decimal::ZERO);
    }

    #[tokio::test]
    async fn api_put_get_list_delete_round_trip() {
        let pool = test_pool().await;
        let resp = client(&pool)
            .put_raw("/cgt_settings/1", r#"{"opening_capital_loss":"1500.25"}"#)
            .await;
        assert_eq!(resp.status, StatusCode::NO_CONTENT);

        let resp = client(&pool).get("/cgt_settings/1").await;
        assert_eq!(resp.status, StatusCode::OK);
        let got: CgtSettings = resp.json();
        assert_eq!(
            got.opening_capital_loss,
            "1500.25".parse::<Decimal>().unwrap()
        );

        let resp = client(&pool).get("/cgt_settings").await;
        assert_eq!(resp.status, StatusCode::OK);
        let items: Vec<CgtSettings> = resp.json();
        assert_eq!(items.len(), 1);

        let resp = client(&pool).delete("/cgt_settings/1").await;
        assert_eq!(resp.status, StatusCode::NO_CONTENT);
        assert!(db_get(&pool, 1).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn api_put_negative_loss_returns_422() {
        let pool = test_pool().await;
        let resp = client(&pool)
            .put_raw("/cgt_settings/1", r#"{"opening_capital_loss":"-10"}"#)
            .await;
        assert_eq!(resp.status, StatusCode::UNPROCESSABLE_ENTITY);
        let detail = resp.text().to_string();
        assert!(detail.contains("cannot be negative"), "detail: {detail}");
    }

    #[tokio::test]
    async fn api_put_non_singleton_id_returns_422() {
        let pool = test_pool().await;
        let resp = client(&pool)
            .put_raw("/cgt_settings/2", r#"{"opening_capital_loss":"10"}"#)
            .await;
        assert_eq!(resp.status, StatusCode::UNPROCESSABLE_ENTITY);
    }

    #[tokio::test]
    async fn api_get_missing_returns_404() {
        let pool = test_pool().await;
        let resp = client(&pool).get("/cgt_settings/1").await;
        assert_eq!(resp.status, StatusCode::NOT_FOUND);

        let resp = client(&pool).delete("/cgt_settings/1").await;
        assert_eq!(resp.status, StatusCode::NOT_FOUND);
    }
}
