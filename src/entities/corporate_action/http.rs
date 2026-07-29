//! HTTP routes: list/get/upsert/delete over the corporate_actions table.

use super::db::db_upsert;
use super::model::{CorporateAction, CorporateActionBody};
use crate::infra::http::{self, ApiError};
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
            get(http::list_handler::<CorporateAction>),
        )
        .route(
            "/corporate_actions/{id}",
            get(http::get_handler::<CorporateAction>)
                .put(upsert)
                // Deleting an action still referenced by rights-exercise trades
                // violates the trades.rights_action_id FK → 422 (delete those
                // first).
                .delete(http::delete_handler::<CorporateAction>),
        )
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
