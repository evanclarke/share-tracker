//! Running a job: the uniformly-logged, lock-serialised single run (`run_job`)
//! that both the scheduled loop and the manual trigger go through, plus the
//! per-entry timer loop (`run_entry`) that decides when to call it.

use super::db::{
    db_finish_run, db_insert_schedule, db_record_run, db_start_run, db_update_schedule,
};
use super::registry::{JobParams, RegisteredJob};
use super::schedule::ScheduleEntry;
use chrono::{DateTime, TimeZone, Utc};
use croner::Cron;
use sqlx::SqlitePool;
use std::{sync::Arc, time::Duration};

/// Run a single job once, bracketing it with an INFO `job started` line and an
/// INFO `job finished` line (the latter carries `ok` = whether it succeeded).
/// Both the scheduled loop and the manual trigger go through here so every job
/// logs start and finish uniformly, regardless of any per-job logging it does,
/// and so every run persists its record (timestamps, status, error, note) to
/// `job_runs` for the Jobs UI. A failure to record the run is logged but does
/// not change the job's own result.
///
/// A job that succeeded while doing **less** than the whole of its work returns
/// its own note ([`super::registry::JobOutcome`]); it is recorded against the
/// run and shown beside the status, so a half-import stops reading as a
/// complete one (SCENARIOS T-09). It is deliberately not folded into the error
/// column: the run succeeded, and this function's `Ok`/`Err` result — what
/// `POST /jobs/{name}` answers on — is unchanged by it.
///
/// The row is written **when the run starts** (`status = 'running'`,
/// `finished_at` NULL) and updated when the work returns. A run the process
/// does not survive — a restart landing on the weekly backup's own slot, a
/// `SIGKILL`, a power cut — therefore leaves a row that started and never
/// finished, instead of the nothing it used to leave, which was
/// indistinguishable from a run that never began (SCENARIOS T-11). Waiting for
/// the job on shutdown was considered and rejected: it covers none of those
/// three.
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
    let run_id = match db_start_run(pool, name, &started_at).await {
        Ok(id) => Some(id),
        Err(e) => {
            tracing::warn!(job = %name, "failed to record the start of the job run: {e}");
            None
        }
    };
    let result = (job.work)(params).await;
    let finished_at = chrono::Utc::now().to_rfc3339();
    tracing::info!(job = %name, ok = result.is_ok(), "job finished");

    let error = result.as_ref().err().map(String::as_str);
    let note = result.as_ref().ok().and_then(Option::as_deref);
    if let Some(note) = note {
        tracing::info!(job = %name, "job succeeded incompletely: {note}");
    }
    let recorded = match run_id {
        Some(id) => db_finish_run(pool, id, &finished_at, result.is_ok(), error, note).await,
        // No opening row to complete, so record the whole run in one write
        // rather than losing it.
        None => {
            db_record_run(
                pool,
                name,
                &started_at,
                &finished_at,
                result.is_ok(),
                error,
                note,
            )
            .await
        }
    };
    if let Err(e) = recorded {
        tracing::warn!(job = %name, "failed to record job run: {e}");
    }
    result.map(|_| ())
}

/// Cap on any single timer sleep. Timer sleeps are monotonic, so a wall-clock
/// shift mid-sleep (a DST transition in the entry's timezone, an NTP step, a
/// laptop resume) would otherwise make the job fire offset from its wall-clock
/// target. Sleeping at most this long and recomputing the target after each
/// chunk re-anchors to the wall clock within an hour of any shift.
const MAX_SLEEP: Duration = Duration::from_secs(60 * 60);

/// One entry's own row in `job_schedule` (migration 0043): the persisted twin
/// of the `next run scheduled` log line, so the instant the scheduler already
/// computes survives into a report that reads only the database.
///
/// The id is claimed lazily by the first successful write and reused after,
/// because the table is rebuilt at every startup — `spawn` clears it before any
/// task starts. A write that finds the row gone (a second `spawn` in the same
/// process) claims a fresh one rather than quietly ceasing to report, and a
/// write that fails is logged and retried on the next iteration: an entry that
/// cannot store its schedule must not take the job's own scheduling down with
/// it.
struct StoredSchedule {
    name: String,
    cron: String,
    timezone: Option<String>,
    id: Option<i64>,
}

impl StoredSchedule {
    fn new(entry: &ScheduleEntry) -> Self {
        Self {
            name: entry.name.clone(),
            cron: entry.expr.clone(),
            timezone: entry.tz.map(|tz| tz.name().to_string()),
            id: None,
        }
    }

    /// Store `next` as this entry's next scheduled run. Held as a UTC instant
    /// whatever zone the entry is pinned to, so the overdue check compares it
    /// with `now` without re-deriving anyone's offset; the zone name travels in
    /// its own column, for display.
    async fn record(&mut self, pool: &SqlitePool, next: DateTime<Utc>) {
        let at = next.to_rfc3339();
        if let Some(id) = self.id {
            match db_update_schedule(pool, id, &at).await {
                Ok(true) => return,
                // The row is gone — fall through and claim another.
                Ok(false) => self.id = None,
                Err(e) => {
                    tracing::warn!(job = %self.name, "could not store the next scheduled run: {e}");
                    return;
                }
            }
        }
        match db_insert_schedule(pool, &self.name, &self.cron, self.timezone.as_deref(), &at).await
        {
            Ok(id) => self.id = Some(id),
            Err(e) => {
                tracing::warn!(job = %self.name, "could not store the next scheduled run: {e}")
            }
        }
    }
}

/// The scheduled loop for one schedule entry: store and log the next run, sleep
/// to it in capped chunks (recomputing the target after each chunk — see
/// `MAX_SLEEP`), run the job, repeat. `now` supplies the current time in the
/// entry's timezone — a per-entry IANA zone, or `Local` for entries without
/// one.
///
/// The stored next run (`job_schedule`) is what makes a scheduler that has
/// **stopped** visible: nothing else in the database changes when a job does
/// not run, so `job_runs` goes on showing the last successful run for ever and
/// `GET /jobs` keeps answering `ok` (SCENARIOS T-11/T-02/T-12). A row is
/// claimed at the instant this task starts, *before* the first occurrence is
/// computed, precisely so the one case that never reaches the loop body is
/// covered too: a cron pattern with no future occurrence at all (`0 0 30 2 *`,
/// 30 February) is accepted at startup, logs one ERROR here, and stops — and
/// the row it leaves, frozen at the moment the scheduler gave up, is what the
/// health report then reports overdue.
pub(super) async fn run_entry<Z>(
    pool: SqlitePool,
    entry: ScheduleEntry,
    job: Arc<RegisteredJob>,
    now: impl Fn() -> DateTime<Z> + Send + 'static,
) where
    Z: TimeZone + Send + 'static,
    Z::Offset: std::fmt::Display + Send,
{
    let ScheduleEntry { cron, name, .. } = &entry;
    let mut stored = StoredSchedule::new(&entry);
    stored.record(&pool, now().with_timezone(&Utc)).await;
    loop {
        let (next, mut delay) = match next_run(cron, now()) {
            Some(pair) => pair,
            None => {
                tracing::error!(job = %name, "cannot compute next run, stopping");
                return;
            }
        };
        stored.record(&pool, next.with_timezone(&Utc)).await;
        tracing::info!(
            job = %name,
            next_run = %next.format("%Y-%m-%d %H:%M:%S %Z"),
            "next run scheduled"
        );
        while delay > MAX_SLEEP {
            tokio::time::sleep(MAX_SLEEP).await;
            delay = match next_run(cron, now()) {
                // Re-stored as well as re-slept: a wall-clock shift mid-wait
                // moves the target, and a stored instant left at the old one
                // would read as overdue while the entry is waiting correctly.
                Some((recomputed_next, recomputed)) => {
                    stored
                        .record(&pool, recomputed_next.with_timezone(&Utc))
                        .await;
                    recomputed
                }
                None => {
                    tracing::error!(job = %name, "cannot compute next run, stopping");
                    return;
                }
            };
        }
        tokio::time::sleep(delay).await;
        // A scheduled run always uses the default (unsuffixed) params — a
        // suffix labels a deliberate one-off backup, not the weekly run.
        if let Err(e) = run_job(&pool, name, &job, JobParams::default()).await {
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
