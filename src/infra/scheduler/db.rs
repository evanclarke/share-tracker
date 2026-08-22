//! Persistence for the per-job run history (`job_runs`) and the live schedule
//! (`job_schedule`), both read by `GET /jobs` and the health report.
//!
//! Every run goes through `run_job`, which records it here — the row is
//! inserted when the run *starts* and updated when it finishes, so an
//! interrupted run leaves a record rather than nothing. That history says
//! nothing at all about a job that has **stopped running**, which is what the
//! schedule half is for: each spawned entry stores the next instant it is due
//! and rewrites it every iteration, so a task that has died leaves a row that
//! ages (SCENARIOS T-11/T-02/T-12).

use crate::infra::db::write_tx;
use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;
use std::collections::HashMap;

/// The state of one recorded run, CHECK-constrained in the database
/// (migration 0042).
///
/// `Running` is written the moment the run starts and replaced when the work
/// returns. It is therefore also what a run *interrupted* by a restart, a
/// `SIGKILL` or a power cut leaves behind: a run that started and never
/// finished, which is neither a success nor a failure (SCENARIOS T-11). Before
/// 0042 the row was written only after the work returned, so such a run left no
/// trace at all and `GET /jobs` went on showing the previous run's result.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, sqlx::Type)]
#[sqlx(rename_all = "lowercase")]
#[serde(rename_all = "lowercase")]
pub enum JobRunStatus {
    Running,
    Ok,
    Failed,
}

impl JobRunStatus {
    /// The finished state for a run that returned `Ok`/`Err`.
    fn finished(success: bool) -> Self {
        if success {
            JobRunStatus::Ok
        } else {
            JobRunStatus::Failed
        }
    }
}

/// One stored run of a job, as exposed in a `JobStatus`'s `runs` history.
/// `finished_at` is `None` exactly when `status` is
/// [`JobRunStatus::Running`] — the schema holds the two in step.
///
/// `note` qualifies a **successful** run that did less than the whole of its
/// work — the currency import that skipped the credential-gated ISO 24165 half
/// (SCENARIOS T-09). It is separate from `error` on purpose: the run did not
/// fail, so its status stays `ok` and the Jobs screen's red/green reading stays
/// honest; the note is what stops it also reading as *complete*.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JobRunRecord {
    pub started_at: String,
    pub finished_at: Option<String>,
    pub status: JobRunStatus,
    pub error: Option<String>,
    pub note: Option<String>,
}

/// One persisted run row from `job_runs`.
#[derive(sqlx::FromRow)]
struct JobRun {
    name: String,
    started_at: String,
    finished_at: Option<String>,
    status: JobRunStatus,
    error: Option<String>,
    note: Option<String>,
}

/// Bound on the stored run history per job: starting a run prunes that job's
/// rows to the newest this-many in the same transaction, so an intermittent
/// (flapping) failure stays diagnosable from `GET /jobs` without the table
/// growing unboundedly.
pub const JOB_RUN_HISTORY_LIMIT: u32 = 20;

/// Record that a run of `name` has just begun, returning the row id its
/// completion updates. The insert prunes the job's history to the newest
/// [`JOB_RUN_HISTORY_LIMIT`] rows in the same transaction, exactly as the
/// old record-on-completion write did.
///
/// Ordering matters twice. The prune runs *after* the insert, so the bound is
/// on the table as it now stands (pruning first would leave the limit plus the
/// fresh row); and the fresh row can never be the row pruned, because it is the
/// newest of its name and `run_job` holds the per-job lock for the whole run —
/// no other run of this job can insert ahead of it before it finishes.
pub(super) async fn db_start_run(
    pool: &SqlitePool,
    name: &str,
    started_at: &str,
) -> Result<i64, sqlx::Error> {
    let mut tx = write_tx(pool).await?;
    let id: i64 = sqlx::query_scalar(
        "INSERT INTO job_runs (name, started_at, finished_at, status, error) \
         VALUES (?, ?, NULL, 'running', NULL) RETURNING id",
    )
    .bind(name)
    .bind(started_at)
    .fetch_one(&mut *tx)
    .await?;
    prune_history(&mut tx, name).await?;
    tx.commit().await?;
    Ok(id)
}

/// Complete the run row `db_start_run` opened. `WHERE status = 'running'`
/// keeps this an update of an in-flight run and nothing else; a missing row
/// (which the per-job lock and the history bound between them make
/// unreachable) surfaces as `RowNotFound` so `run_job` logs it rather than
/// silently recording nothing.
pub(super) async fn db_finish_run(
    pool: &SqlitePool,
    id: i64,
    finished_at: &str,
    success: bool,
    error: Option<&str>,
    note: Option<&str>,
) -> Result<(), sqlx::Error> {
    let updated = sqlx::query(
        "UPDATE job_runs SET finished_at = ?, status = ?, error = ?, note = ? \
         WHERE id = ? AND status = 'running'",
    )
    .bind(finished_at)
    .bind(JobRunStatus::finished(success))
    .bind(error)
    .bind(note)
    .bind(id)
    .execute(pool)
    .await?
    .rows_affected();
    if updated == 0 {
        return Err(sqlx::Error::RowNotFound);
    }
    Ok(())
}

/// Append a complete run record for `name` and prune the job's history to the
/// newest [`JOB_RUN_HISTORY_LIMIT`] rows, atomically. This is the fallback for
/// a run whose opening row never landed (a failed `db_start_run`): the run is
/// over and the whole of it is recorded in one write, rather than being lost.
pub(super) async fn db_record_run(
    pool: &SqlitePool,
    name: &str,
    started_at: &str,
    finished_at: &str,
    success: bool,
    error: Option<&str>,
    note: Option<&str>,
) -> Result<(), sqlx::Error> {
    let mut tx = write_tx(pool).await?;
    sqlx::query(
        "INSERT INTO job_runs (name, started_at, finished_at, status, error, note) \
         VALUES (?, ?, ?, ?, ?, ?)",
    )
    .bind(name)
    .bind(started_at)
    .bind(finished_at)
    .bind(JobRunStatus::finished(success))
    .bind(error)
    .bind(note)
    .execute(&mut *tx)
    .await?;
    prune_history(&mut tx, name).await?;
    tx.commit().await?;
    Ok(())
}

/// Drop everything but the newest [`JOB_RUN_HISTORY_LIMIT`] rows of one job,
/// on the caller's transaction.
async fn prune_history(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    name: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "DELETE FROM job_runs WHERE name = ?1 AND id NOT IN \
             (SELECT id FROM job_runs WHERE name = ?1 ORDER BY id DESC LIMIT ?2)",
    )
    .bind(name)
    .bind(JOB_RUN_HISTORY_LIMIT)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

/// Load every job's stored run history, keyed by job name, each job's runs
/// most recent first.
pub(super) async fn db_run_histories(
    pool: &SqlitePool,
) -> Result<HashMap<String, Vec<JobRunRecord>>, sqlx::Error> {
    let rows = sqlx::query_as::<_, JobRun>(
        "SELECT name, started_at, finished_at, status, error, note FROM job_runs ORDER BY id DESC",
    )
    .fetch_all(pool)
    .await?;
    let mut histories: HashMap<String, Vec<JobRunRecord>> = HashMap::new();
    for r in rows {
        histories.entry(r.name).or_default().push(JobRunRecord {
            started_at: r.started_at,
            finished_at: r.finished_at,
            status: r.status,
            error: r.error,
            note: r.note,
        });
    }
    Ok(histories)
}

/// The next scheduled run of one schedule entry, as the entry's own timer
/// computed it (`job_schedule`, migration 0043). Read by `GET /jobs` — which
/// folds the rows of a job with several lines into its earliest — and, on its
/// own read, by the health report's overdue check.
///
/// The table is **the schedule the running process is executing**: it is
/// cleared by [`super::schedule::spawn`] and rebuilt by the entry tasks, so a
/// job whose schedule line was removed leaves nothing behind to go stale. What
/// makes it useful is the opposite case — a task that has *stopped* stops
/// rewriting its row, which no `job_runs` record can show, because a job that
/// is not running records nothing at all.
#[derive(Debug, Clone, sqlx::FromRow)]
pub(super) struct JobScheduleRow {
    pub(super) name: String,
    pub(super) next_run_at: String,
}

/// Drop every stored schedule row. Called once at startup, before the entry
/// tasks are spawned, so the table only ever describes the schedule *this*
/// process is running: a line deleted from `schedule.cron` leaves no row to be
/// reported overdue for ever after (the lost-line case is what the startup
/// WARN is for — SCENARIOS T-09/schedule).
pub(super) async fn db_clear_schedule(pool: &SqlitePool) -> Result<(), sqlx::Error> {
    sqlx::query("DELETE FROM job_schedule")
        .execute(pool)
        .await?;
    Ok(())
}

/// Claim a row for one schedule entry, returning the id its later iterations
/// update. `next_run_at` is a UTC RFC 3339 instant.
pub(super) async fn db_insert_schedule(
    pool: &SqlitePool,
    name: &str,
    cron: &str,
    timezone: Option<&str>,
    next_run_at: &str,
) -> Result<i64, sqlx::Error> {
    let id: i64 = sqlx::query_scalar(
        "INSERT INTO job_schedule (name, cron, timezone, next_run_at, updated_at) \
         VALUES (?, ?, ?, ?, ?) RETURNING id",
    )
    .bind(name)
    .bind(cron)
    .bind(timezone)
    .bind(next_run_at)
    .bind(chrono::Utc::now().to_rfc3339())
    .fetch_one(pool)
    .await?;
    Ok(id)
}

/// Move one entry's stored next run forward. Answers whether the row was still
/// there: a `false` means the table was cleared underneath the task (a second
/// `spawn` in the same process), and the caller claims a fresh row rather than
/// silently ceasing to report.
pub(super) async fn db_update_schedule(
    pool: &SqlitePool,
    id: i64,
    next_run_at: &str,
) -> Result<bool, sqlx::Error> {
    let updated =
        sqlx::query("UPDATE job_schedule SET next_run_at = ?, updated_at = ? WHERE id = ?")
            .bind(next_run_at)
            .bind(chrono::Utc::now().to_rfc3339())
            .bind(id)
            .execute(pool)
            .await?
            .rows_affected();
    Ok(updated > 0)
}

/// Each job's next scheduled run — the **earliest** of its entries', for a job
/// scheduled on more than one line (`price-import` has three, one per market).
/// Jobs with no schedule entry are absent, which is what a manual-only job is.
pub(super) async fn db_next_runs(
    pool: &SqlitePool,
) -> Result<HashMap<String, String>, sqlx::Error> {
    let rows = sqlx::query_as::<_, JobScheduleRow>("SELECT name, next_run_at FROM job_schedule")
        .fetch_all(pool)
        .await?;
    let mut next: HashMap<String, String> = HashMap::new();
    for row in rows {
        next.entry(row.name)
            .and_modify(|held| {
                if row.next_run_at < *held {
                    held.clone_from(&row.next_run_at);
                }
            })
            .or_insert(row.next_run_at);
    }
    Ok(next)
}
