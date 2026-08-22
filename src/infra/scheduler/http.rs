//! Routes for inspecting (`GET /jobs`) and manually triggering
//! (`POST /jobs/{name}`) maintenance jobs.

use super::db::{JobRunRecord, db_run_histories};
use super::registry::{JobParams, JobRegistry, JobTrigger};
use super::run::run_job;
use axum::{
    Extension, Json, Router,
    extract::{Path, Query, State, rejection::QueryRejection},
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

/// Run one job now. Every failure answers with a plain-text body the Jobs
/// screen can toast — a bare status code left the operator reading "HTTP 404"
/// or "HTTP 500" with the reason nowhere in the response (SCENARIOS T-10).
async fn trigger(
    State(pool): State<SqlitePool>,
    Extension(registry): Extension<JobRegistry>,
    Path(name): Path<String>,
    // Taken as a `Result` so the extractor's own rejection — which `Query`
    // would otherwise answer itself, as a `400` in axum's wording — becomes a
    // `422` with a reason, the same shape as every other rejected write. That
    // is what `JobParams`' `deny_unknown_fields` surfaces as.
    params: Result<Query<JobParams>, QueryRejection>,
) -> Result<StatusCode, crate::infra::http::ApiError> {
    let Query(params) = params.map_err(|e| {
        // serde's own message ("unknown field `sufix`, expected `suffix`")
        // rather than axum's "Failed to deserialize query string: …" wrapper,
        // which prefixes it with framework jargon in a toast that has room
        // for one line.
        let detail = std::error::Error::source(&e)
            .map(|source| source.to_string())
            .unwrap_or_else(|| e.body_text());
        crate::infra::http::ApiError::unprocessable(format!(
            "cannot read the query string: {detail}"
        ))
    })?;
    // Reject an invalid suffix before the registry lookup or run_job: a
    // malformed request must not be recorded as a failed job run (only the
    // backup job reads it, but the query string is shared across all names).
    if let Some(suffix) = &params.suffix {
        crate::infra::db::validate_backup_suffix(suffix)
            .map_err(crate::infra::http::ApiError::unprocessable)?;
    }
    match registry.get(&name) {
        // The `deleted(found, noun)` convention, one endpoint over: name what
        // was missing, and — since the registry already holds every name and
        // `GET /jobs` is a screen away — what the caller could have asked for.
        None => {
            let mut known: Vec<&str> = registry.keys().map(String::as_str).collect();
            known.sort_unstable();
            Err(crate::infra::http::ApiError::not_found(format!(
                "no job named '{name}'; registered jobs are {}",
                known.join(", ")
            )))
        }
        Some(job) => match run_job(&pool, &name, job, params).await {
            Ok(()) => Ok(StatusCode::NO_CONTENT),
            // The job's own error text (what `job_runs.error` records) rides
            // out in the 500's body, so the toast says what went wrong.
            Err(e) => Err(crate::infra::http::ApiError::job_failed(&name, e)),
        },
    }
}

/// Routes for inspecting and manually triggering jobs. The `JobRegistry` is
/// supplied to handlers via an `Extension` layer (added in `main`), so this
/// router shares the common `SqlitePool` state and merges with the others.
pub fn router() -> Router<SqlitePool> {
    Router::new()
        .route("/jobs", get(list))
        .route("/jobs/{name}", post(trigger))
}
