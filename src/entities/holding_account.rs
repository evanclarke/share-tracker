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

use crate::infra::db::write_tx;
use crate::infra::http::{self, ApiError, CrudEntity, Upsert, UpsertResponse};
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

#[derive(utoipa::ToSchema, Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct HoldingAccount {
    pub id: i64,
    pub name: String,
}

#[derive(utoipa::ToSchema, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HoldingAccountBody {
    pub name: String,
}

impl CrudEntity for HoldingAccount {
    type Key = i64;
    type Filter = crate::infra::http::NoFilter;
    const TABLE: &'static str = "holding_accounts";
    const COLUMNS: &'static str = "id, name";
    const NOUN: &'static str = "holding account";
}

pub fn router() -> Router<SqlitePool> {
    Router::new()
        .route(
            "/holding_accounts",
            get(http::list_handler::<HoldingAccount>).post(create),
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

/// One-line delegation to the shared CRUD read, through which `db_create`
/// reads the created row back; the route reaches the same query through
/// `get_handler` (see CLAUDE.md's entity-module pattern).
pub async fn db_get(pool: &SqlitePool, id: i64) -> Result<Option<HoldingAccount>, sqlx::Error> {
    http::crud_get(pool, id).await
}

/// Why a holding-account write failed. The entity's only write-time invariant
/// is the UNIQUE `name`, which the database enforces, so the upsert path's
/// whole error set is the `sqlx::Error` its long-standing signature answers
/// with; `VanishedAfterCreate` is the create path's alone.
#[derive(thiserror::Error, Debug)]
pub enum UpsertError {
    #[error("holding account write failed: {0}")]
    Db(#[from] sqlx::Error),
    /// A create committed but the row could not be read back. Not reachable in
    /// practice — the row is committed and unread only if something deletes it
    /// in the same instant — but a create that answers with no row would be
    /// worse than a loud failure, so it is its own variant rather than an
    /// `unwrap`. Mapped to `500`.
    #[error("the holding account was created but could not be read back")]
    VanishedAfterCreate,
}

impl From<UpsertError> for ApiError {
    fn from(e: UpsertError) -> Self {
        match e {
            // A duplicate name violates the UNIQUE constraint → 422 with the
            // offending constraint named.
            UpsertError::Db(err) => err.into(),
            // A create that cannot read back its own committed row is a server
            // fault, not the caller's.
            err @ UpsertError::VanishedAfterCreate => ApiError::internal(err),
        }
    }
}

/// Write a holding account, allocating its id when `id` is `None`.
///
/// The one statement is shared by both callers: `id = Some` is the
/// long-standing `PUT /holding_accounts/:id` upsert, and `id = None` is the
/// `POST /holding_accounts` create, which leaves the id to the database. The
/// error is the plain `sqlx::Error` the upsert has always answered with, since
/// the UNIQUE `name` is the only validation and the database applies it.
async fn write(
    pool: &SqlitePool,
    id: Option<i64>,
    account: &HoldingAccount,
) -> Result<(i64, Upsert), sqlx::Error> {
    // A create (`id` is `None`, from `POST /holding_accounts`) omits the id
    // column altogether, so the database — not the caller — chooses the new
    // row's id (SCENARIOS U-a). The upsert branch is otherwise byte-identical,
    // so a `PUT` on an explicit id stays the upsert it has always been.
    //
    // `holding_accounts` is the one table here that is not `AUTOINCREMENT` (it
    // sits outside the audit scope), so a deleted *highest* id can be
    // re-issued; see
    // `post_holding_account_lands_on_an_unheld_id_and_never_overwrites`.
    let query = crate::insert_with_optional_id!(
        "INSERT INTO holding_accounts (id, name) VALUES (?, ?) \
         ON CONFLICT(id) DO UPDATE SET name = excluded.name",
        "INSERT INTO holding_accounts (name) VALUES (?) \
         ON CONFLICT(id) DO UPDATE SET name = excluded.name",
        id,
    );
    let mut tx = write_tx(pool).await?;
    // The create-vs-replace decision is made inside the write's own
    // `BEGIN IMMEDIATE` transaction (CLAUDE.md, "Data integrity").
    let existed = match id {
        Some(id) => http::crud_exists::<HoldingAccount, _>(&mut *tx, id).await?,
        None => false,
    };
    let result = query.bind(&account.name).execute(&mut *tx).await?;
    tx.commit().await?;
    // An id-less INSERT was given one by the database; an upsert wrote the
    // explicit one it was handed. Either way the caller gets the id the row
    // actually holds.
    let assigned_id = id.unwrap_or_else(|| result.last_insert_rowid());
    let outcome = if existed {
        Upsert::Replaced
    } else {
        Upsert::Created
    };
    Ok((assigned_id, outcome))
}

pub async fn db_upsert(pool: &SqlitePool, account: &HoldingAccount) -> Result<Upsert, sqlx::Error> {
    let (_, outcome) = write(pool, Some(account.id), account).await?;
    Ok(outcome)
}

/// `POST /holding_accounts` — create without naming an id, and return the row
/// the database assigned one to.
pub async fn db_create(
    pool: &SqlitePool,
    account: &HoldingAccount,
) -> Result<HoldingAccount, UpsertError> {
    let (id, _) = write(pool, None, account).await?;
    db_get(pool, id)
        .await?
        .ok_or(UpsertError::VanishedAfterCreate)
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
    let mut tx = write_tx(pool).await?;

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

/// The row a request body describes. `id` is the path's on an upsert and
/// ignored on a create (the database assigns one), so both entry points build
/// their `HoldingAccount` through this one mapping and cannot drift.
fn holding_account_from_body(id: i64, body: HoldingAccountBody) -> HoldingAccount {
    HoldingAccount {
        id,
        name: body.name,
    }
}

async fn upsert(
    State(pool): State<SqlitePool>,
    Path(id): Path<i64>,
    Json(body): Json<HoldingAccountBody>,
) -> Result<UpsertResponse<HoldingAccount>, ApiError> {
    // A duplicate name violates the UNIQUE constraint → 422.
    let outcome = db_upsert(&pool, &holding_account_from_body(id, body)).await?;
    http::upsert_response::<HoldingAccount>(&pool, outcome, id).await
}

/// `POST /holding_accounts` — create the account without naming an id. The
/// database assigns one (see [`write`]), and the created row is returned so the
/// caller can use its id immediately with no `max(id) + 1` guess between the
/// two calls.
async fn create(
    State(pool): State<SqlitePool>,
    Json(body): Json<HoldingAccountBody>,
) -> Result<(StatusCode, Json<HoldingAccount>), ApiError> {
    let created = db_create(&pool, &holding_account_from_body(0, body)).await?;
    Ok((StatusCode::CREATED, Json(created)))
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
    use crate::test_support::{ApiClient, test_pool};

    /// Client over this module's own routes.
    fn client(pool: &SqlitePool) -> ApiClient {
        ApiClient::over(router().with_state(pool.clone()))
    }

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

        let resp = ApiClient::over(app())
            .put_raw("/holding_accounts/2", r#"{"name":"ICE Employee Plan"}"#)
            .await;
        assert_eq!(resp.status, StatusCode::CREATED);

        let resp = ApiClient::over(app()).get("/holding_accounts").await;
        assert_eq!(resp.status, StatusCode::OK);
        let accounts: Vec<HoldingAccount> = resp.json();
        assert_eq!(accounts.len(), 2);
        assert_eq!(accounts[1].name, "ICE Employee Plan");

        let resp = ApiClient::over(app()).delete("/holding_accounts/2").await;
        assert_eq!(resp.status, StatusCode::NO_CONTENT);

        let resp = ApiClient::over(app()).get("/holding_accounts/2").await;
        assert_eq!(resp.status, StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn api_duplicate_name_returns_422_and_default_delete_refused() {
        let pool = test_pool().await;
        let app = || router().with_state(pool.clone());

        let resp = ApiClient::over(app())
            .put_raw("/holding_accounts/2", r#"{"name":"Default"}"#)
            .await;
        assert_eq!(resp.status, StatusCode::UNPROCESSABLE_ENTITY);
        let detail = resp.text().to_string();
        assert!(detail.contains("already exists"), "detail: {detail}");

        let resp = ApiClient::over(app()).delete("/holding_accounts/1").await;
        assert_eq!(resp.status, StatusCode::UNPROCESSABLE_ENTITY);
        let detail = resp.text().to_string();
        assert!(detail.contains("still has"), "detail: {detail}");
    }

    // Create (`POST /holding_accounts`) tests

    /// `POST /holding_accounts` allocates its own id — the create that makes
    /// the whole `max(id) + 1` dance unnecessary (SCENARIOS U-a).
    #[tokio::test]
    async fn post_holding_account_creates_a_row_with_a_database_assigned_id() {
        let pool = test_pool().await;
        let client = client(&pool);

        let response = client
            .post(
                "/holding_accounts",
                &serde_json::json!({ "name": "ICE Employee Plan" }),
            )
            .await;
        let (status, text) = response.status_and_body();
        assert_eq!(status, StatusCode::CREATED, "body: {text:?}");

        let created: HoldingAccount = response.json();
        assert_ne!(created.id, 0, "a create must not land on id 0");
        assert_eq!(created.name, "ICE Employee Plan");

        // The returned row is the stored one, so the caller can act on the id
        // at once instead of re-reading the collection to discover it.
        let listed = db_get(&pool, created.id).await.unwrap().unwrap();
        assert_eq!(listed.id, created.id);
        assert_eq!(listed.name, "ICE Employee Plan");
        // The seeded default account is untouched alongside it.
        assert_eq!(db_list(&pool).await.unwrap().len(), 2);
    }

    /// A create lands on an id no live row holds and never overwrites one.
    ///
    /// `holding_accounts` is the one entity this change covers whose table is
    /// **not** `AUTOINCREMENT`: it is identity-only and carries no
    /// `row_history` trail, so migration 0045 (which gave every audited table
    /// an AUTOINCREMENT id) left it a plain `INTEGER PRIMARY KEY` and SQLite
    /// assigns `max(rowid) + 1` over the rows that remain. A deleted *highest*
    /// id therefore can come back — pinned here is the property that holds
    /// either way, and is what actually matters: a create takes an id no live
    /// row holds and leaves every existing row alone. A re-issued number
    /// inherits nothing, because accounts are outside the audit scope, and an
    /// account anything references — or the seeded default — cannot be deleted
    /// at all (`db_delete`). Tightening this to a never-reissued id needs a
    /// migration adding AUTOINCREMENT (SCENARIOS U-a).
    #[tokio::test]
    async fn post_holding_account_lands_on_an_unheld_id_and_never_overwrites() {
        let pool = test_pool().await;
        let client = client(&pool);

        let first: HoldingAccount = client
            .post("/holding_accounts", &serde_json::json!({ "name": "Plan" }))
            .await
            .expect_status(StatusCode::CREATED)
            .json();
        let second: HoldingAccount = client
            .post(
                "/holding_accounts",
                &serde_json::json!({ "name": "Second" }),
            )
            .await
            .expect_status(StatusCode::CREATED)
            .json();
        assert_ne!(first.id, second.id);

        // Delete the lower id: its number is the one a naive `max(id) + 1`
        // client would hand back, and the next create must not take it.
        client
            .delete(&format!("/holding_accounts/{}", first.id))
            .await
            .expect_status(StatusCode::NO_CONTENT);

        let before: Vec<i64> = db_list(&pool).await.unwrap().iter().map(|a| a.id).collect();
        let third: HoldingAccount = client
            .post("/holding_accounts", &serde_json::json!({ "name": "Third" }))
            .await
            .expect_status(StatusCode::CREATED)
            .json();
        assert_ne!(
            third.id, first.id,
            "the create took a deleted id back: live ids were {before:?}"
        );
        assert!(
            !before.contains(&third.id),
            "the create landed on an id a live row already holds: {before:?}"
        );
        assert_eq!(third.name, "Third");

        // No existing row was replaced by the create.
        let second_after = db_get(&pool, second.id).await.unwrap().unwrap();
        assert_eq!(second_after.name, "Second");
        let default = db_get(&pool, DEFAULT_HOLDING_ACCOUNT_ID)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(default.name, "Default");
    }

    #[tokio::test]
    async fn post_holding_account_twice_stores_two_distinct_rows() {
        let pool = test_pool().await;
        let client = client(&pool);

        let first: HoldingAccount = client
            .post("/holding_accounts", &serde_json::json!({ "name": "Plan" }))
            .await
            .expect_status(StatusCode::CREATED)
            .json();
        let second: HoldingAccount = client
            .post(
                "/holding_accounts",
                &serde_json::json!({ "name": "Second" }),
            )
            .await
            .expect_status(StatusCode::CREATED)
            .json();
        assert_ne!(
            first.id, second.id,
            "each create must get its own id, not overwrite the previous row"
        );

        let all = db_list(&pool).await.unwrap();
        assert_eq!(all.len(), 3, "the seed plus both creates: {all:?}");
        let mut ids: Vec<i64> = all.iter().map(|a| a.id).collect();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), 3, "the ids are distinct: {all:?}");
    }

    /// A create shares the upsert's one write-time invariant — the UNIQUE
    /// `name`, enforced by the database — so a duplicate is refused the same
    /// way and nothing is stored.
    #[tokio::test]
    async fn post_holding_account_rejects_like_the_upsert_and_stores_nothing() {
        let pool = test_pool().await;
        let client = client(&pool);

        // "Default" is the seeded account's name.
        let response = client
            .post(
                "/holding_accounts",
                &serde_json::json!({ "name": "Default" }),
            )
            .await;
        let (status, text) = response.status_and_body();
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "body: {text:?}");
        assert!(
            text.contains("already exists"),
            "the shared wording is reused: {text:?}"
        );

        assert_eq!(
            db_list(&pool).await.unwrap().len(),
            1,
            "a rejected create stores nothing"
        );
    }
}
