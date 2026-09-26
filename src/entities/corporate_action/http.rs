//! HTTP routes: list/get/upsert/delete over the corporate_actions table.

use super::db::{db_create, db_delete, db_upsert};
use super::model::{CorporateAction, CorporateActionBody};
use crate::infra::http::{self, ApiError, UpsertResponse};
use axum::{
    Json, Router,
    extract::{Path, State},
    http::StatusCode,
    routing::get,
};
use sqlx::SqlitePool;

pub fn router() -> Router<SqlitePool> {
    Router::new()
        .route(
            "/corporate_actions",
            get(http::list_handler::<CorporateAction>).post(create),
        )
        .route(
            "/corporate_actions/{id}",
            get(http::get_handler::<CorporateAction>)
                .put(upsert)
                // Deleting an action still referenced by rights-exercise trades
                // violates the trades.rights_action_id FK → 422 (delete those
                // first); the types that create no trades carry their own
                // delete-time guard in `db_delete`.
                .delete(delete),
        )
}

/// The action a request body describes. `id` is the path's on an upsert and
/// ignored on a create (the database assigns one), so both entry points build
/// their `CorporateAction` through this one mapping and cannot drift. The
/// per-type terms check is shared unchanged: a body whose fields do not match
/// its `action_type` is refused on either path.
fn corporate_action_from_body(
    id: i64,
    body: CorporateActionBody,
) -> Result<CorporateAction, ApiError> {
    let (listing_id, date) = (body.listing_id, body.date);
    let kind = body.kind().ok_or_else(|| {
        ApiError::unprocessable(
            "the corporate-action terms are missing or do not match the action type",
        )
    })?;
    Ok(CorporateAction {
        id,
        listing_id,
        date,
        kind,
    })
}

async fn upsert(
    State(pool): State<SqlitePool>,
    Path(id): Path<i64>,
    Json(body): Json<CorporateActionBody>,
) -> Result<UpsertResponse<CorporateAction>, ApiError> {
    let (outcome, row) = db_upsert(&pool, &corporate_action_from_body(id, body)?).await?;
    http::upsert_response::<CorporateAction>(outcome, row)
}

/// `POST /corporate_actions` — create the action without naming an id. The
/// database assigns one (see `db::write`), and the created row is returned so
/// a client can act on its id immediately — participating, exercising,
/// demerging — instead of guessing `max(id) + 1` between the two calls.
async fn create(
    State(pool): State<SqlitePool>,
    Json(body): Json<CorporateActionBody>,
) -> Result<(StatusCode, Json<CorporateAction>), ApiError> {
    let created = db_create(&pool, &corporate_action_from_body(0, body)?).await?;
    Ok((StatusCode::CREATED, Json(created)))
}

async fn delete(
    State(pool): State<SqlitePool>,
    Path(id): Path<i64>,
) -> Result<StatusCode, ApiError> {
    http::deleted(db_delete(&pool, id).await?, "corporate action")
}
