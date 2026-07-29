//! Running a job: the uniformly-logged, lock-serialised single run (`run_job`)
//! that both the scheduled loop and the manual trigger go through, plus the
//! per-entry timer loop (`run_entry`) that decides when to call it.

use super::db::db_record_run;
use super::registry::{JobParams, RegisteredJob};
use chrono::{DateTime, TimeZone};
use croner::Cron;
use sqlx::SqlitePool;
use std::{sync::Arc, time::Duration};

/// Run a single job once, bracketing it with an INFO `job started` line and an
/// INFO `job finished` line (the latter carries `ok` = whether it succeeded).
/// Both the scheduled loop and the manual trigger go through here so every job
/// logs start and finish uniformly, regardless of any per-job logging it does,
/// and so every run persists its last-run record (timestamps, success, error)
/// to `job_runs` for the Jobs UI. A failure to record the run is logged but does
/// not change the job's own result.
///
/// The per-job lock is held for the whole run, serialising executions of the
/// same job: a manual trigger overlapping the scheduled run (or another
/// trigger) waits for the in-flight run to finish instead of racing it.
pub(super) async fn run_job(
    pool: &SqlitePool,
    name: &str,
    job: &RegisteredJob,
    params: JobParams,
) -> Result<(), String> {
    let _running = job.lock.lock().await;
    let started_at = chrono::Utc::now().to_rfc3339();
    tracing::info!(job = %name, "job started");
    let result = (job.work)(params).await;
    let finished_at = chrono::Utc::now().to_rfc3339();
    tracing::info!(job = %name, ok = result.is_ok(), "job finished");

    let error = result.as_ref().err().map(String::as_str);
    if let Err(e) =
        db_record_run(pool, name, &started_at, &finished_at, result.is_ok(), error).await
    {
        tracing::warn!(job = %name, "failed to record job run: {e}");
    }
    result
}

/// Cap on any single timer sleep. Timer sleeps are monotonic, so a wall-clock
/// shift mid-sleep (a DST transition in the entry's timezone, an NTP step, a
/// laptop resume) would otherwise make the job fire offset from its wall-clock
/// target. Sleeping at most this long and recomputing the target after each
/// chunk re-anchors to the wall clock within an hour of any shift.
const MAX_SLEEP: Duration = Duration::from_secs(60 * 60);

/// The scheduled loop for one schedule entry: log the next run, sleep to it in
/// capped chunks (recomputing the target after each chunk — see `MAX_SLEEP`),
/// run the job, repeat. `now` supplies the current time in the entry's
/// timezone — a per-entry IANA zone, or `Local` for entries without one.
pub(super) async fn run_entry<Z>(
    pool: SqlitePool,
    name: String,
    job: Arc<RegisteredJob>,
    cron: Cron,
    now: impl Fn() -> DateTime<Z> + Send + 'static,
) where
    Z: TimeZone + Send + 'static,
    Z::Offset: std::fmt::Display + Send,
{
    loop {
        let (next, mut delay) = match next_run(&cron, now()) {
            Some(pair) => pair,
            None => {
                tracing::error!(job = %name, "cannot compute next run, stopping");
                return;
            }
        };
        tracing::info!(
            job = %name,
            next_run = %next.format("%Y-%m-%d %H:%M:%S %Z"),
            "next run scheduled"
        );
        while delay > MAX_SLEEP {
            tokio::time::sleep(MAX_SLEEP).await;
            delay = match next_run(&cron, now()) {
                Some((_, recomputed)) => recomputed,
                None => {
                    tracing::error!(job = %name, "cannot compute next run, stopping");
                    return;
                }
            };
        }
        tokio::time::sleep(delay).await;
        // A scheduled run always uses the default (unsuffixed) params — a
        // suffix labels a deliberate one-off backup, not the weekly run.
        if let Err(e) = run_job(&pool, &name, &job, JobParams::default()).await {
            tracing::warn!(job = %name, "job failed: {e}");
        }
    }
}

/// Compute the next scheduled fire time at or after `now` and the exact delay to
/// sleep until then. The delay keeps sub-second precision: truncating it (e.g.
/// via `num_seconds`) would make the timer wake *before* the target second, so
/// the recomputed next run would still be the same instant and the loop would
/// busy-spin until the clock crossed the boundary. Returns `None` only if the
/// cron pattern has no future occurrence.
pub(super) fn next_run<Z: TimeZone>(
    cron: &Cron,
    now: DateTime<Z>,
) -> Option<(DateTime<Z>, Duration)> {
    let next = cron.find_next_occurrence(&now, false).ok()?;
    // `next` is strictly after `now`, so the difference is non-negative; the
    // fallback only guards against clock skew between this read and the diff.
    let delay = (next.clone() - now)
        .to_std()
        .unwrap_or(Duration::from_secs(1));
    Some((next, delay))
}
