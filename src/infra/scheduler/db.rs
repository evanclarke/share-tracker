//! Persistence for the per-job run history (`job_runs`), read by `GET /jobs`
//! and the health report. Every run goes through `run_job`, which records it
//! here.

use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;
use std::collections::HashMap;

/// One stored run of a job, as exposed in a `JobStatus`'s `runs` history.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JobRunRecord {
    pub started_at: String,
    pub finished_at: String,
    pub success: bool,
    pub error: Option<String>,
}

/// One persisted run row from `job_runs`.
#[derive(sqlx::FromRow)]
struct JobRun {
    name: String,
    started_at: String,
    finished_at: String,
    success: bool,
    error: Option<String>,
}

/// Bound on the stored run history per job: recording a run prunes that job's
/// rows to the newest this-many in the same transaction, so an intermittent
/// (flapping) failure stays diagnosable from `GET /jobs` without the table
/// growing unboundedly.
pub const JOB_RUN_HISTORY_LIMIT: u32 = 20;

/// Append a run record for `name` and prune the job's history to the newest
/// [`JOB_RUN_HISTORY_LIMIT`] rows, atomically — history accumulates per run
/// (unlike the old one-row-per-job upsert) but stays bounded.
pub(super) async fn db_record_run(
    pool: &SqlitePool,
    name: &str,
    started_at: &str,
    finished_at: &str,
    success: bool,
    error: Option<&str>,
) -> Result<(), sqlx::Error> {
    let mut tx = pool.begin().await?;
    sqlx::query(
        "INSERT INTO job_runs (name, started_at, finished_at, success, error) \
         VALUES (?, ?, ?, ?, ?)",
    )
    .bind(name)
    .bind(started_at)
    .bind(finished_at)
    .bind(success)
    .bind(error)
    .execute(&mut *tx)
    .await?;
    sqlx::query(
        "DELETE FROM job_runs WHERE name = ?1 AND id NOT IN \
             (SELECT id FROM job_runs WHERE name = ?1 ORDER BY id DESC LIMIT ?2)",
    )
    .bind(name)
    .bind(JOB_RUN_HISTORY_LIMIT)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(())
}

/// Load every job's stored run history, keyed by job name, each job's runs
/// most recent first.
pub(super) async fn db_run_histories(
    pool: &SqlitePool,
) -> Result<HashMap<String, Vec<JobRunRecord>>, sqlx::Error> {
    let rows = sqlx::query_as::<_, JobRun>(
        "SELECT name, started_at, finished_at, success, error FROM job_runs ORDER BY id DESC",
    )
    .fetch_all(pool)
    .await?;
    let mut histories: HashMap<String, Vec<JobRunRecord>> = HashMap::new();
    for r in rows {
        histories.entry(r.name).or_default().push(JobRunRecord {
            started_at: r.started_at,
            finished_at: r.finished_at,
            success: r.success,
            error: r.error,
        });
    }
    Ok(histories)
}
