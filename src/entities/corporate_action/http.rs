//! HTTP routes: list/get/upsert/delete over the corporate_actions table.

use super::db::{db_delete, db_get, db_list, db_upsert};
use super::model::{CorporateAction, CorporateActionBody};
use crate::infra::http::ApiError;
use axum::{
    Json, Router,
    extract::{Path, State},
    http::StatusCode,
    routing::get,
};
use sqlx::SqlitePool;

pub fn router() -> Router<SqlitePool> {
    Router::new().route("/corporate_actions", get(list)).route(
        "/corporate_actions/{id}",
        get(get_one).put(upsert).delete(delete),
    )
}

async fn list(State(pool): State<SqlitePool>) -> Result<Json<Vec<CorporateAction>>, ApiError> {
    db_list(&pool).await.map(Json).map_err(ApiError::from)
}

async fn get_one(
    State(pool): State<SqlitePool>,
    Path(id): Path<i64>,
) -> Result<Json<CorporateAction>, ApiError> {
    db_get(&pool, id)
        .await
        .map_err(ApiError::from)?
        .map(Json)
        .ok_or(ApiError::NotFound)
}

async fn upsert(
    State(pool): State<SqlitePool>,
    Path(id): Path<i64>,
    Json(body): Json<CorporateActionBody>,
) -> Result<StatusCode, ApiError> {
    let (listing_id, date) = (body.listing_id, body.date);
    let kind = body.kind().ok_or_else(|| {
        ApiError::unprocessable(
            "the corporate-action terms are missing or do not match the action type",
        )
    })?;
    let action = CorporateAction {
        id,
        listing_id,
        date,
        kind,
    };
    db_upsert(&pool, &action).await?;
    Ok(StatusCode::NO_CONTENT)
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
        // Deleting an action still referenced by rights-exercise trades
        // violates the trades.rights_action_id FK → 422 (delete those first).
        .map_err(ApiError::from)
}
