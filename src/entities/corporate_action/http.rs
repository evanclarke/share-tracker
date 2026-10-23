//! HTTP routes: list/get/upsert/delete over the `corporate_actions` table.

use super::CorporateActionListQuery;
use super::db::{db_create, db_delete, db_upsert};
use super::model::{CorporateAction, CorporateActionBody};
use crate::infra::extract::Json;
use crate::infra::http::{self, ApiError, UpsertResponse, Upserted};
use crate::infra::respond::{Created, NoContent};
use axum::extract::{Path, State};
use sqlx::SqlitePool;
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

http::crud_list_route!(
    list,
    CorporateAction,
    CorporateActionListQuery,
    "/corporate_actions",
    "List every corporate action, optionally narrowed by ?listing_id= and an inclusive ?from=/?to= over the action date."
);
http::crud_get_route!(
    get_one,
    CorporateAction,
    i64,
    "/corporate_actions/{id}",
    "Fetch one corporate action, or 404."
);

pub fn router() -> OpenApiRouter<SqlitePool> {
    OpenApiRouter::new()
        .routes(routes!(list, create))
        // Deleting an action still referenced by rights-exercise trades
        // violates the trades.rights_action_id FK → 422 (delete those
        // first); the types that create no trades carry their own
        // delete-time guard in `db_delete`.
        .routes(routes!(get_one, upsert, delete))
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

#[utoipa::path(
    put,
    path = "/corporate_actions/{id}",
    summary = "Create or replace the corporate action at this id."
)]
async fn upsert(
    State(pool): State<SqlitePool>,
    Path(id): Path<i64>,
    Json(body): Json<CorporateActionBody>,
) -> Result<UpsertResponse<CorporateAction>, ApiError> {
    let Upserted {
        outcome,
        created: row,
    } = db_upsert(&pool, &corporate_action_from_body(id, body)?).await?;
    http::upsert_response::<CorporateAction>(outcome, row)
}

/// `POST /corporate_actions` — create the action without naming an id. The
/// database assigns one (see `db::write`), and the created row is returned so
/// a client can act on its id immediately — participating, exercising,
/// demerging — instead of guessing `max(id) + 1` between the two calls.
#[utoipa::path(
    post,
    path = "/corporate_actions",
    summary = "Create a corporate action; the created row is returned."
)]
async fn create(
    State(pool): State<SqlitePool>,
    Json(body): Json<CorporateActionBody>,
) -> Result<Created<CorporateAction>, ApiError> {
    let created = db_create(&pool, &corporate_action_from_body(0, body)?).await?;
    Ok(Created(created))
}

#[utoipa::path(
    delete,
    path = "/corporate_actions/{id}",
    summary = "Delete the corporate action; 422 while a trade references it."
)]
async fn delete(
    State(pool): State<SqlitePool>,
    Path(id): Path<i64>,
) -> Result<NoContent, ApiError> {
    let found = db_delete(&pool, id).await?;
    http::deleted(found, "corporate action")
}
