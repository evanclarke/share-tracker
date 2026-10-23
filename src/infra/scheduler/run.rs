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
///
/// The run is **detached** from its caller: it executes on a task of its own
/// and the caller only awaits that task's handle. `POST /jobs/{name}` awaits
/// this inline, and axum drops a request's future when its client disconnects
/// — closing the tab or a Ctrl-C in curl used to cancel the job between two
/// of its commits and leave its `job_runs` row `running` for ever. Dropping
/// the handle now leaves the run going; it finishes and records itself.
///
/// A job that **panics** is recorded as a failed run carrying the panic's
/// message, rather than unwinding through here: the work runs on a task of
/// its own too, whose `JoinError` is the panic caught. On the scheduled path
/// the panic used to unwind out of `run_entry` and end that entry's timer
/// loop, so one panic stopped the job being scheduled until a restart.
pub(super) async fn run_job(
    pool: &SqlitePool,
    name: &str,
    job: &Arc<RegisteredJob>,
    params: JobParams,
) -> Result<(), String> {
    let (pool, name, job) = (pool.clone(), name.to_owned(), job.clone());
    tokio::spawn(async move { run_attached(&pool, &name, &job, params).await })
        .await
        .unwrap_or_else(|e| Err(join_failure(e)))
}

/// The message a run that ended in a `JoinError` records: the panic's own
/// payload when it carried a string (`panic!("…")`, `expect`, an index out of
/// bounds — every panic the code itself raises), else a plain statement that it
/// panicked. A cancelled task cannot reach here: nothing aborts a run.
fn join_failure(e: tokio::task::JoinError) -> String {
    if !e.is_panic() {
        return format!("job task ended unexpectedly: {e}");
    }
    let payload = e.into_panic();
    let message = payload
        .downcast_ref::<&str>()
        .map(|s| s.to_string())
        .or_else(|| payload.downcast_ref::<String>().cloned());
    match message {
        Some(m) => format!("job panicked: {m}"),
        None => "job panicked".to_owned(),
    }
}

/// The body of [`run_job`], on the task it detaches. The job's own work is
/// spawned once more so a panic in it is caught here — with the lock still
/// held and the opening row still to complete — and recorded as a failure.
async fn run_attached(
    pool: &SqlitePool,
    name: &str,
    job: &RegisteredJob,
    params: JobParams,
) -> Result<(), String> {
    let _running = job.serialise().await;
    let started_at = chrono::Utc::now().to_rfc3339();
    tracing::info!(job = %name, "job started");
    let run_id = match db_start_run(pool, name, &started_at).await {
        Ok(id) => Some(id),
        Err(e) => {
            tracing::warn!(job = %name, "failed to record the start of the job run: {e}");
            None
        }
    };
    let result = tokio::spawn((job.work)(params))
        .await
        .unwrap_or_else(|e| Err(join_failure(e)));
    let finished_at = chrono::Utc::now().to_rfc3339();
    tracing::info!(job = %name, ok = result.is_ok(), "job finished");

    if let Ok(Some(note)) = &result {
        tracing::info!(job = %name, "job succeeded incompletely: {note}");
    }
    let recorded = match run_id {
        Some(id) => db_finish_run(pool, id, &finished_at, &result).await,
        // No opening row to complete, so record the whole run in one write
        // rather than losing it.
        None => db_record_run(pool, name, &started_at, &finished_at, &result).await,
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
            timezone: entry.tz.map(|tz| tz.name().to_owned()),
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

/// Whether an occurrence croner returned is one the entry asked for.
///
/// croner moves a wall time a spring-forward gap skips (02:30 on the day
/// 02:00 becomes 03:00) to the first instant after the gap, which is wanted —
/// the day's run is neither skipped nor an error. But on that same day it also
/// answers that instant for an entry whose time lies *before* the gap: from a
/// 01:00 run it offers 03:00 as the next "01:00", so a daily 01:00 job ran
/// twice. An occurrence is genuine when its own wall time matches the
/// expression, or when the gap immediately before it skipped a wall time that
/// does.
fn is_genuine_occurrence<Z: TimeZone>(cron: &Cron, next: &DateTime<Z>) -> bool {
    let matches =
        |wall: chrono::NaiveDateTime| cron.is_time_matching(&wall.and_utc()).unwrap_or(false);
    let wall = next.naive_local();
    if matches(wall) {
        return true;
    }
    // The wall times between the instant before `next` and `next` itself are
    // the ones a gap skipped — none, unless `next` sits just after one.
    let mut skipped =
        (next.clone() - chrono::Duration::seconds(1)).naive_local() + chrono::Duration::seconds(1);
    while skipped < wall {
        if matches(skipped) {
            return true;
        }
        skipped += chrono::Duration::minutes(1);
    }
    false
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
    let mut from = now.clone();
    let next = loop {
        let next = cron.find_next_occurrence(&from, false).ok()?;
        if is_genuine_occurrence(cron, &next) {
            break next;
        }
        from = next;
    };
    // `next` is strictly after `now`, so the difference is non-negative; the
    // fallback only guards against clock skew between this read and the diff.
    let delay = (next.clone() - now)
        .to_std()
        .unwrap_or(Duration::from_secs(1));
    Some((next, delay))
}
