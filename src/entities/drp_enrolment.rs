//! DRP (Dividend Reinvestment Plan) enrolment per holding.
//!
//! At most one enrolment per listing (the table is keyed by `listing_id`); its
//! presence means the holding reinvests its distributions in full. Partial
//! participation is out of scope. `residual_handling` decides what happens to
//! the leftover cash that a reinvestment can't spend on whole shares — it is
//! either carried forward to the next reinvestment or paid out (see
//! `entities::drp_reinvestment`).

use axum::{
    Json, Router,
    extract::{Path, State},
    http::StatusCode,
    routing::get,
};
use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, sqlx::Type)]
pub enum ResidualHandling {
    /// Leftover cash is carried forward and added to the next reinvestment.
    CarryForward,
    /// Leftover cash is paid out rather than carried.
    PayOut,
}

impl Default for ResidualHandling {
    fn default() -> Self {
        ResidualHandling::CarryForward
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct DrpEnrolment {
    /// Holding this enrolment applies to; also the primary key (one per listing).
    pub listing_id: i64,
    pub residual_handling: ResidualHandling,
}

#[derive(Debug, Deserialize)]
pub struct DrpEnrolmentBody {
    #[serde(default)]
    pub residual_handling: ResidualHandling,
}

pub fn router() -> Router<SqlitePool> {
    Router::new()
        .route("/drp_enrolments", get(list))
        .route("/drp_enrolments/{listing_id}", get(get_one).put(upsert).delete(delete))
}

pub async fn db_list(pool: &SqlitePool) -> Result<Vec<DrpEnrolment>, sqlx::Error> {
    sqlx::query_as("SELECT listing_id, residual_handling FROM drp_enrolments ORDER BY listing_id")
        .fetch_all(pool)
        .await
}

pub async fn db_get(pool: &SqlitePool, listing_id: i64) -> Result<Option<DrpEnrolment>, sqlx::Error> {
    sqlx::query_as("SELECT listing_id, residual_handling FROM drp_enrolments WHERE listing_id = ?")
        .bind(listing_id)
        .fetch_optional(pool)
        .await
}

pub async fn db_upsert(pool: &SqlitePool, enrolment: &DrpEnrolment) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO drp_enrolments (listing_id, residual_handling) VALUES (?, ?) \
         ON CONFLICT(listing_id) DO UPDATE SET residual_handling = excluded.residual_handling",
    )
    .bind(enrolment.listing_id)
    .bind(enrolment.residual_handling)
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn db_delete(pool: &SqlitePool, listing_id: i64) -> Result<bool, sqlx::Error> {
    let result = sqlx::query("DELETE FROM drp_enrolments WHERE listing_id = ?")
        .bind(listing_id)
        .execute(pool)
        .await?;
    Ok(result.rows_affected() > 0)
}

async fn list(State(pool): State<SqlitePool>) -> Result<Json<Vec<DrpEnrolment>>, StatusCode> {
    db_list(&pool)
        .await
        .map(Json)
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)
}

async fn get_one(
    State(pool): State<SqlitePool>,
    Path(listing_id): Path<i64>,
) -> Result<Json<DrpEnrolment>, StatusCode> {
    db_get(&pool, listing_id)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
        .map(Json)
        .ok_or(StatusCode::NOT_FOUND)
}

async fn upsert(
    State(pool): State<SqlitePool>,
    Path(listing_id): Path<i64>,
    Json(body): Json<DrpEnrolmentBody>,
) -> Result<StatusCode, StatusCode> {
    let enrolment = DrpEnrolment { listing_id, residual_handling: body.residual_handling };
    db_upsert(&pool, &enrolment)
        .await
        .map(|_| StatusCode::NO_CONTENT)
        // A bad listing_id violates the FK to listings → not a server fault.
        .map_err(|_| StatusCode::UNPROCESSABLE_ENTITY)
}

async fn delete(
    State(pool): State<SqlitePool>,
    Path(listing_id): Path<i64>,
) -> Result<StatusCode, StatusCode> {
    db_delete(&pool, listing_id)
        .await
        .map(|found| if found { StatusCode::NO_CONTENT } else { StatusCode::NOT_FOUND })
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{entities::listing, infra::db};
    use axum::{body::Body, http::Request};
    use http_body_util::BodyExt;
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
                security_type: listing::SecurityType::Share,
                currency: "AUD".to_string(),
                amit: false,
                preference: false,
            },
        )
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn db_insert_and_retrieve() {
        let pool = test_pool().await;
        insert_listing(&pool, 1).await;
        db_upsert(&pool, &DrpEnrolment { listing_id: 1, residual_handling: ResidualHandling::PayOut })
            .await
            .unwrap();
        let got = db_get(&pool, 1).await.unwrap().unwrap();
        assert_eq!(got.listing_id, 1);
        assert_eq!(got.residual_handling, ResidualHandling::PayOut);
    }

    #[tokio::test]
    async fn db_one_enrolment_per_listing_upsert_updates() {
        let pool = test_pool().await;
        insert_listing(&pool, 1).await;
        db_upsert(&pool, &DrpEnrolment {
            listing_id: 1,
            residual_handling: ResidualHandling::CarryForward,
        })
        .await
        .unwrap();
        // A second upsert on the same holding updates the row, not duplicates it.
        db_upsert(&pool, &DrpEnrolment { listing_id: 1, residual_handling: ResidualHandling::PayOut })
            .await
            .unwrap();

        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM drp_enrolments WHERE listing_id = 1")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(count, 1);
        assert_eq!(db_get(&pool, 1).await.unwrap().unwrap().residual_handling, ResidualHandling::PayOut);
    }

    #[tokio::test]
    async fn db_residual_handling_enum_constraint_enforced() {
        let pool = test_pool().await;
        insert_listing(&pool, 1).await;
        let err = sqlx::query("INSERT INTO drp_enrolments (listing_id, residual_handling) VALUES (1, 'Bogus')")
            .execute(&pool)
            .await;
        assert!(err.is_err(), "CHECK constraint should reject an unknown residual_handling");
    }

    #[tokio::test]
    async fn db_listing_fk_enforced() {
        let pool = test_pool().await;
        // No listing 99 exists → FK violation.
        let err = db_upsert(&pool, &DrpEnrolment {
            listing_id: 99,
            residual_handling: ResidualHandling::CarryForward,
        })
        .await;
        assert!(err.is_err(), "FK to listings should reject an unknown listing_id");
    }

    #[tokio::test]
    async fn api_put_defaults_to_carry_forward() {
        let pool = test_pool().await;
        insert_listing(&pool, 1).await;
        // Empty body → default CarryForward.
        let resp = router()
            .with_state(pool.clone())
            .oneshot(
                Request::builder()
                    .method("PUT")
                    .uri("/drp_enrolments/1")
                    .header("content-type", "application/json")
                    .body(Body::from("{}"))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NO_CONTENT);
        assert_eq!(
            db_get(&pool, 1).await.unwrap().unwrap().residual_handling,
            ResidualHandling::CarryForward
        );
    }

    #[tokio::test]
    async fn api_put_bad_listing_returns_422() {
        let pool = test_pool().await;
        let resp = router()
            .with_state(pool)
            .oneshot(
                Request::builder()
                    .method("PUT")
                    .uri("/drp_enrolments/99")
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"residual_handling":"PayOut"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY);
    }

    #[tokio::test]
    async fn api_get_list_and_delete() {
        let pool = test_pool().await;
        insert_listing(&pool, 1).await;
        db_upsert(&pool, &DrpEnrolment {
            listing_id: 1,
            residual_handling: ResidualHandling::CarryForward,
        })
        .await
        .unwrap();

        let resp = router()
            .with_state(pool.clone())
            .oneshot(Request::builder().uri("/drp_enrolments").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let items: Vec<DrpEnrolment> = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(items.len(), 1);

        let resp = router()
            .with_state(pool.clone())
            .oneshot(
                Request::builder()
                    .method("DELETE")
                    .uri("/drp_enrolments/1")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NO_CONTENT);
        assert!(db_get(&pool, 1).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn api_get_missing_returns_404() {
        let pool = test_pool().await;
        let resp = router()
            .with_state(pool)
            .oneshot(Request::builder().uri("/drp_enrolments/1").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }
}
