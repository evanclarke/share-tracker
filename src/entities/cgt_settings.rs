//! CGT settings: the opening carried-forward capital loss.
//!
//! A singleton row (the table CHECKs `id = 1`) holding the net capital loss
//! carried forward from years before the first year recorded in the system, so a
//! user migrating mid-history doesn't have to re-enter pre-system loss years.
//! The net-capital-gain report uses it as the starting brought-forward balance
//! when chaining unused losses across its year series (losses carry forward
//! indefinitely, per `docs/ato/cgt-using-capital-losses.md`). Absent row = zero.

use crate::infra::decimal::parse_dec;
use crate::infra::http::ApiError;
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
pub struct CgtSettings {
    /// Always 1 — the table CHECKs it, so at most one settings row exists.
    pub id: i64,
    /// Net capital loss carried forward from before the first recorded year
    /// (a non-negative amount, AUD).
    pub opening_capital_loss: Decimal,
}

impl<'r> sqlx::FromRow<'r, sqlx::sqlite::SqliteRow> for CgtSettings {
    fn from_row(row: &'r sqlx::sqlite::SqliteRow) -> Result<Self, sqlx::Error> {
        Ok(CgtSettings {
            id: row.try_get("id")?,
            opening_capital_loss: parse_dec(
                "opening_capital_loss",
                row.try_get("opening_capital_loss")?,
            )?,
        })
    }
}

#[derive(Debug, Deserialize)]
pub struct CgtSettingsBody {
    pub opening_capital_loss: Decimal,
}

pub fn router() -> Router<SqlitePool> {
    Router::new().route("/cgt_settings", get(list)).route(
        "/cgt_settings/{id}",
        get(get_one).put(upsert).delete(delete),
    )
}

pub async fn db_list(pool: &SqlitePool) -> Result<Vec<CgtSettings>, sqlx::Error> {
    sqlx::query_as("SELECT id, opening_capital_loss FROM cgt_settings ORDER BY id")
        .fetch_all(pool)
        .await
}

pub async fn db_get(pool: &SqlitePool, id: i64) -> Result<Option<CgtSettings>, sqlx::Error> {
    sqlx::query_as("SELECT id, opening_capital_loss FROM cgt_settings WHERE id = ?")
        .bind(id)
        .fetch_optional(pool)
        .await
}

pub async fn db_upsert(pool: &SqlitePool, settings: &CgtSettings) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO cgt_settings (id, opening_capital_loss) VALUES (?, ?) \
         ON CONFLICT(id) DO UPDATE SET opening_capital_loss = excluded.opening_capital_loss",
    )
    .bind(settings.id)
    .bind(settings.opening_capital_loss.to_string())
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn db_delete(pool: &SqlitePool, id: i64) -> Result<bool, sqlx::Error> {
    let result = sqlx::query("DELETE FROM cgt_settings WHERE id = ?")
        .bind(id)
        .execute(pool)
        .await?;
    Ok(result.rows_affected() > 0)
}

/// The opening carried-forward capital loss, or zero when no settings row exists.
/// Used by the net-capital-gain report as the starting loss balance.
pub async fn db_opening_capital_loss(pool: &SqlitePool) -> Result<Decimal, sqlx::Error> {
    Ok(db_get(pool, 1)
        .await?
        .map_or(Decimal::ZERO, |s| s.opening_capital_loss))
}

async fn list(State(pool): State<SqlitePool>) -> Result<Json<Vec<CgtSettings>>, ApiError> {
    db_list(&pool).await.map(Json).map_err(ApiError::from)
}

async fn get_one(
    State(pool): State<SqlitePool>,
    Path(id): Path<i64>,
) -> Result<Json<CgtSettings>, ApiError> {
    db_get(&pool, id)
        .await
        .map_err(ApiError::from)?
        .map(Json)
        .ok_or(ApiError::NotFound)
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

async fn delete(
    State(pool): State<SqlitePool>,
    Path(id): Path<i64>,
) -> Result<StatusCode, ApiError> {
    db_delete(&pool, id)
        .await
        .map(|found| {
            if found {
                StatusCode::NO_CONTENT
            } else {
                StatusCode::NOT_FOUND
            }
        })
        .map_err(ApiError::from)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::test_pool;
    use axum::{body::Body, http::Request};
    use http_body_util::BodyExt;
    use tower::ServiceExt;

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
        let resp = router()
            .with_state(pool.clone())
            .oneshot(
                Request::builder()
                    .method("PUT")
                    .uri("/cgt_settings/1")
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"opening_capital_loss":"1500.25"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NO_CONTENT);

        let resp = router()
            .with_state(pool.clone())
            .oneshot(
                Request::builder()
                    .uri("/cgt_settings/1")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let got: CgtSettings = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(
            got.opening_capital_loss,
            "1500.25".parse::<Decimal>().unwrap()
        );

        let resp = router()
            .with_state(pool.clone())
            .oneshot(
                Request::builder()
                    .uri("/cgt_settings")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let items: Vec<CgtSettings> = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(items.len(), 1);

        let resp = router()
            .with_state(pool.clone())
            .oneshot(
                Request::builder()
                    .method("DELETE")
                    .uri("/cgt_settings/1")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NO_CONTENT);
        assert!(db_get(&pool, 1).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn api_put_negative_loss_returns_422() {
        let pool = test_pool().await;
        let resp = router()
            .with_state(pool)
            .oneshot(
                Request::builder()
                    .method("PUT")
                    .uri("/cgt_settings/1")
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"opening_capital_loss":"-10"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY);
        let bytes = BodyExt::collect(resp.into_body()).await.unwrap().to_bytes();
        let detail = String::from_utf8(bytes.to_vec()).unwrap();
        assert!(detail.contains("cannot be negative"), "detail: {detail}");
    }

    #[tokio::test]
    async fn api_put_non_singleton_id_returns_422() {
        let pool = test_pool().await;
        let resp = router()
            .with_state(pool)
            .oneshot(
                Request::builder()
                    .method("PUT")
                    .uri("/cgt_settings/2")
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"opening_capital_loss":"10"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY);
    }

    #[tokio::test]
    async fn api_get_missing_returns_404() {
        let pool = test_pool().await;
        let resp = router()
            .with_state(pool.clone())
            .oneshot(
                Request::builder()
                    .uri("/cgt_settings/1")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);

        let resp = router()
            .with_state(pool)
            .oneshot(
                Request::builder()
                    .method("DELETE")
                    .uri("/cgt_settings/1")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }
}
