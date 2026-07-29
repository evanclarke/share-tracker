//! Holding accounts: the custody/location dimension within one taxpayer.
//!
//! The same listing can be held in more than one place at once with different
//! treatment — e.g. RSU-vested shares sitting in an employer share-plan
//! account (which cannot participate in the DRP) alongside DRP-enrolled
//! shares in the holder's own broker account. Trades, income, AMMA statements
//! and DRP enrolment periods each carry a `holding_account_id`; API writes
//! that omit it default to the seeded default account
//! (`DEFAULT_HOLDING_ACCOUNT_ID`), so single-account users never see the
//! dimension. Shares move between accounts via `entities::transfer`.
//!
//! This is *not* the planned taxpayer-level Accounts / ownership dimension:
//! every holding account belongs to the same taxpayer, so taxpayer-level
//! reports (tax summary, net capital gain) aggregate across all of them.

use crate::infra::http::{self, ApiError, CrudEntity};
use axum::{
    Json, Router,
    extract::{Path, State},
    http::StatusCode,
    routing::get,
};
use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;

/// The account every row migrated to and every write that omits
/// `holding_account_id` lands in (seeded as 'Default' by migration 0016).
pub const DEFAULT_HOLDING_ACCOUNT_ID: i64 = 1;

/// Serde default for `holding_account_id` body fields.
pub fn default_holding_account_id() -> i64 {
    DEFAULT_HOLDING_ACCOUNT_ID
}

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct HoldingAccount {
    pub id: i64,
    pub name: String,
}

#[derive(Debug, Deserialize)]
pub struct HoldingAccountBody {
    pub name: String,
}

impl CrudEntity for HoldingAccount {
    type Key = i64;
    const TABLE: &'static str = "holding_accounts";
    const COLUMNS: &'static str = "id, name";
    const NOUN: &'static str = "holding account";
}

pub fn router() -> Router<SqlitePool> {
    Router::new()
        .route(
            "/holding_accounts",
            get(http::list_handler::<HoldingAccount>),
        )
        .route(
            "/holding_accounts/{id}",
            get(http::get_handler::<HoldingAccount>)
                .put(upsert)
                .delete(delete),
        )
}

#[cfg(test)]
pub async fn db_list(pool: &SqlitePool) -> Result<Vec<HoldingAccount>, sqlx::Error> {
    http::crud_list(pool).await
}

#[cfg(test)]
pub async fn db_get(pool: &SqlitePool, id: i64) -> Result<Option<HoldingAccount>, sqlx::Error> {
    http::crud_get(pool, id).await
}

pub async fn db_upsert(pool: &SqlitePool, account: &HoldingAccount) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO holding_accounts (id, name) VALUES (?, ?) \
         ON CONFLICT(id) DO UPDATE SET name = excluded.name",
    )
    .bind(account.id)
    .bind(&account.name)
    .execute(pool)
    .await?;
    Ok(())
}

/// Outcome of a delete request, so the handler can map to the right status.
#[derive(Debug, PartialEq)]
pub enum DeleteOutcome {
    Deleted,
    NotFound,
    /// The account still holds data — trades, income, AMMA statements, DRP
    /// enrolment periods, or a transfer endpoint reference it — or it is the
    /// seeded default account (which writes that omit an account fall back
    /// to). Refused (mapped to 422) rather than surfacing the SQLite FK error
    /// as a 500; move or remove the data first.
    Referenced,
}

pub async fn db_delete(pool: &SqlitePool, id: i64) -> Result<DeleteOutcome, sqlx::Error> {
    let mut tx = pool.begin().await?;

    let exists: bool =
        sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM holding_accounts WHERE id = ?)")
            .bind(id)
            .fetch_one(&mut *tx)
            .await?;
    if !exists {
        return Ok(DeleteOutcome::NotFound);
    }
    // The seeded default account is the fallback for writes that omit an
    // account; deleting it would turn those into FK failures.
    if id == DEFAULT_HOLDING_ACCOUNT_ID {
        return Ok(DeleteOutcome::Referenced);
    }

    let referenced: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM trades WHERE holding_account_id = ?1) \
             OR EXISTS(SELECT 1 FROM income WHERE holding_account_id = ?1) \
             OR EXISTS(SELECT 1 FROM amma_statements WHERE holding_account_id = ?1) \
             OR EXISTS(SELECT 1 FROM drp_enrolments WHERE holding_account_id = ?1) \
             OR EXISTS(SELECT 1 FROM transfers \
                       WHERE from_account_id = ?1 OR to_account_id = ?1)",
    )
    .bind(id)
    .fetch_one(&mut *tx)
    .await?;
    if referenced {
        return Ok(DeleteOutcome::Referenced);
    }

    sqlx::query("DELETE FROM holding_accounts WHERE id = ?")
        .bind(id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(DeleteOutcome::Deleted)
}

async fn upsert(
    State(pool): State<SqlitePool>,
    Path(id): Path<i64>,
    Json(body): Json<HoldingAccountBody>,
) -> Result<StatusCode, ApiError> {
    db_upsert(
        &pool,
        &HoldingAccount {
            id,
            name: body.name,
        },
    )
    .await
    .map(|_| StatusCode::NO_CONTENT)
    // A duplicate name violates the UNIQUE constraint → 422.
    .map_err(ApiError::from)
}

async fn delete(
    State(pool): State<SqlitePool>,
    Path(id): Path<i64>,
) -> Result<StatusCode, ApiError> {
    match db_delete(&pool, id).await? {
        DeleteOutcome::Deleted => Ok(StatusCode::NO_CONTENT),
        DeleteOutcome::NotFound => Err(ApiError::not_found("no holding account with that id")),
        DeleteOutcome::Referenced => Err(ApiError::unprocessable(
            "this account still has trades, income, AMMA statements, DRP enrolments, or \
             transfers — reassign or delete those first (and the default account cannot be \
             deleted)",
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::test_pool;
    use axum::{body::Body, http::Request};
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    // DB-level tests

    #[tokio::test]
    async fn migration_seeds_the_default_account() {
        let pool = test_pool().await;
        let accounts = db_list(&pool).await.unwrap();
        assert_eq!(accounts.len(), 1);
        assert_eq!(accounts[0].id, DEFAULT_HOLDING_ACCOUNT_ID);
        assert_eq!(accounts[0].name, "Default");
    }

    #[tokio::test]
    async fn db_insert_retrieve_and_rename() {
        let pool = test_pool().await;
        db_upsert(
            &pool,
            &HoldingAccount {
                id: 2,
                name: "ICE Employee Plan".into(),
            },
        )
        .await
        .unwrap();
        assert_eq!(
            db_get(&pool, 2).await.unwrap().unwrap().name,
            "ICE Employee Plan"
        );

        db_upsert(
            &pool,
            &HoldingAccount {
                id: 2,
                name: "Personal CHESS".into(),
            },
        )
        .await
        .unwrap();
        assert_eq!(
            db_get(&pool, 2).await.unwrap().unwrap().name,
            "Personal CHESS"
        );
        assert_eq!(db_list(&pool).await.unwrap().len(), 2);
    }

    #[tokio::test]
    async fn db_duplicate_name_is_rejected() {
        let pool = test_pool().await;
        db_upsert(
            &pool,
            &HoldingAccount {
                id: 2,
                name: "Plan".into(),
            },
        )
        .await
        .unwrap();
        let err = db_upsert(
            &pool,
            &HoldingAccount {
                id: 3,
                name: "Plan".into(),
            },
        )
        .await
        .unwrap_err();
        assert!(matches!(
            crate::infra::http::ApiError::from(err),
            crate::infra::http::ApiError::Unprocessable(_)
        ));
    }

    #[tokio::test]
    async fn db_delete_unused_account_and_missing_account() {
        let pool = test_pool().await;
        db_upsert(
            &pool,
            &HoldingAccount {
                id: 2,
                name: "Plan".into(),
            },
        )
        .await
        .unwrap();
        assert_eq!(db_delete(&pool, 2).await.unwrap(), DeleteOutcome::Deleted);
        assert_eq!(db_delete(&pool, 2).await.unwrap(), DeleteOutcome::NotFound);
    }

    #[tokio::test]
    async fn db_delete_default_account_is_refused() {
        let pool = test_pool().await;
        assert_eq!(
            db_delete(&pool, DEFAULT_HOLDING_ACCOUNT_ID).await.unwrap(),
            DeleteOutcome::Referenced
        );
    }

    // API-level tests

    #[tokio::test]
    async fn api_crud_roundtrip() {
        let pool = test_pool().await;
        let app = || router().with_state(pool.clone());

        let resp = app()
            .oneshot(
                Request::builder()
                    .method("PUT")
                    .uri("/holding_accounts/2")
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"name":"ICE Employee Plan"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NO_CONTENT);

        let resp = app()
            .oneshot(
                Request::builder()
                    .uri("/holding_accounts")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let accounts: Vec<HoldingAccount> = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(accounts.len(), 2);
        assert_eq!(accounts[1].name, "ICE Employee Plan");

        let resp = app()
            .oneshot(
                Request::builder()
                    .method("DELETE")
                    .uri("/holding_accounts/2")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NO_CONTENT);

        let resp = app()
            .oneshot(
                Request::builder()
                    .uri("/holding_accounts/2")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn api_duplicate_name_returns_422_and_default_delete_refused() {
        let pool = test_pool().await;
        let app = || router().with_state(pool.clone());

        let resp = app()
            .oneshot(
                Request::builder()
                    .method("PUT")
                    .uri("/holding_accounts/2")
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"name":"Default"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY);
        let bytes = BodyExt::collect(resp.into_body()).await.unwrap().to_bytes();
        let detail = String::from_utf8(bytes.to_vec()).unwrap();
        assert!(detail.contains("already exists"), "detail: {detail}");

        let resp = app()
            .oneshot(
                Request::builder()
                    .method("DELETE")
                    .uri("/holding_accounts/1")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY);
        let bytes = BodyExt::collect(resp.into_body()).await.unwrap().to_bytes();
        let detail = String::from_utf8(bytes.to_vec()).unwrap();
        assert!(detail.contains("still has"), "detail: {detail}");
    }
}
