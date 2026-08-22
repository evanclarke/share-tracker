//! Routes for inspecting (`GET /jobs`) and manually triggering
//! (`POST /jobs/{name}`) maintenance jobs.

use super::db::{JobRunRecord, db_run_histories};
use super::registry::{JobParams, JobRegistry, JobTrigger};
use super::run::run_job;
use axum::{
    Extension, Json, Router,
    extract::{Path, Query, State},
    http::StatusCode,
    routing::{get, post},
};
use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;

/// A job's status, surfaced by `GET /jobs` and the Jobs UI. Every registered
/// job appears in the list; the `last_*` fields are `None` (and `runs` empty)
/// until the job has run at least once. `runs` is the job's stored run history,
/// most recent first, bounded to [`super::db::JOB_RUN_HISTORY_LIMIT`] entries —
/// the `last_*` fields duplicate `runs[0]` for at-a-glance reading.
///
/// `trigger` is the registry's own record of whether the job belongs on the
/// schedule ([`JobTrigger`]), so a job that has never run and never will run on
/// a timer reads as *manual only* rather than as one that is somehow overdue.
#[derive(Debug, Serialize, Deserialize)]
pub struct JobStatus {
    pub name: String,
    pub trigger: JobTrigger,
    pub last_started_at: Option<String>,
    pub last_finished_at: Option<String>,
    pub last_success: Option<bool>,
    pub last_error: Option<String>,
    pub runs: Vec<JobRunRecord>,
}

/// List every registered job (sorted) with its bounded run history (most
/// recent first). Jobs that have never run carry `None` in the `last_*` fields
/// and an empty `runs`.
async fn list(
    State(pool): State<SqlitePool>,
    Extension(registry): Extension<JobRegistry>,
) -> Result<Json<Vec<JobStatus>>, crate::infra::http::ApiError> {
    let mut histories = db_run_histories(&pool).await?;

    let mut names: Vec<String> = registry.keys().cloned().collect();
    names.sort();
    let statuses = names
        .into_iter()
        .map(|name| {
            let runs = histories.remove(&name).unwrap_or_default();
            let last = runs.first();
            let trigger = registry[&name].trigger;
            JobStatus {
                name,
                trigger,
                last_started_at: last.map(|r| r.started_at.clone()),
                last_finished_at: last.map(|r| r.finished_at.clone()),
                last_success: last.map(|r| r.success),
                last_error: last.and_then(|r| r.error.clone()),
                runs,
            }
        })
        .collect();
    Ok(Json(statuses))
}

async fn trigger(
    State(pool): State<SqlitePool>,
    Extension(registry): Extension<JobRegistry>,
    Path(name): Path<String>,
    Query(params): Query<JobParams>,
) -> Result<StatusCode, crate::infra::http::ApiError> {
    // Reject an invalid suffix before the registry lookup or run_job: a
    // malformed request must not be recorded as a failed job run (only the
    // backup job reads it, but the query string is shared across all names).
    if let Some(suffix) = &params.suffix {
        crate::infra::db::validate_backup_suffix(suffix)
            .map_err(crate::infra::http::ApiError::unprocessable)?;
    }
    Ok(match registry.get(&name) {
        None => StatusCode::NOT_FOUND,
        Some(job) => match run_job(&pool, &name, job, params).await {
            Ok(()) => StatusCode::NO_CONTENT,
            Err(e) => {
                tracing::warn!(job = %name, "manual job trigger failed: {e}");
                StatusCode::INTERNAL_SERVER_ERROR
            }
        },
    })
}

/// Routes for inspecting and manually triggering jobs. The `JobRegistry` is
/// supplied to handlers via an `Extension` layer (added in `main`), so this
/// router shares the common `SqlitePool` state and merges with the others.
pub fn router() -> Router<SqlitePool> {
    Router::new()
        .route("/jobs", get(list))
        .route("/jobs/{name}", post(trigger))
}
