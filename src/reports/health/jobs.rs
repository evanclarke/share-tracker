//! Job-run checks: the failed, overdue and stalled jobs the health banner
//! names (see the parent module's list).

use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};

/// How far past its stored next run a scheduled job may go before it is
/// reported overdue (see [`OverdueJob`]).
///
/// The margin has to absorb three legitimate delays. A job's stored instant is
/// only moved on *after* its run returns, so the margin must exceed the longest
/// a scheduled job takes — minutes, for the price import walking a fortnight of
/// trading days per held listing, or the snapshot generation behind it. The
/// timer re-anchors to the wall clock in hourly chunks (`scheduler`'s
/// `MAX_SLEEP`), so a DST step or an NTP correction mid-wait can legitimately
/// hold a fire up to an hour past the instant computed before the shift. And a
/// server restarted moments before a due instant rewrites the row from the new
/// process, which takes as long as startup takes.
///
/// Six hours clears all three by an order of magnitude and still catches a dead
/// weekly task on the morning of the day it should have run — a quarter of a
/// day against a cadence of a week. It is deliberately *not* tuned per job: the
/// stored instant already carries each job's own cadence, which is the whole
/// point of storing it rather than declaring a maximum age per job.
pub const JOB_OVERDUE_GRACE_HOURS: i64 = 6;

/// How long a run may stay `running` before it is reported as stalled (see
/// [`StalledJob`]).
///
/// `job_runs` records a run from the moment it starts (SCENARIOS T-11), which
/// made an *interrupted* run visible — but left it looking exactly like one
/// genuinely in flight, since both are `running` with no finish time. That
/// write-up said telling them apart needs a notion of overdue; this is it, on
/// the other axis. No scheduled job here runs for hours: the longest is a price
/// import or a snapshot generation, both minutes. A row still open after six of
/// them is a dead process's debris — nothing ever finishes it, because the
/// process that would have is gone — and it is worth saying so, since the Jobs
/// screen would otherwise show `running` for ever.
///
/// Same figure as [`JOB_OVERDUE_GRACE_HOURS`] and deliberately its own
/// constant: the two answer different questions (how late a schedule may be
/// against how long a run may be open) and either could move without the other.
pub const JOB_RUNNING_STALE_HOURS: i64 = 6;

/// A job whose most recent recorded run failed. A run still in flight — or one
/// a restart interrupted, which `job_runs` now records as started and never
/// finished (SCENARIOS T-11) — is deliberately not a failure and does not
/// appear here.
#[derive(utoipa::ToSchema, Debug, Serialize, Deserialize, sqlx::FromRow)]
pub struct FailedJob {
    pub name: String,
    pub finished_at: String,
    pub error: Option<String>,
}

/// A scheduled job whose stored next run is in the past by more than
/// [`JOB_OVERDUE_GRACE_HOURS`] — its timer has stopped moving the row on.
///
/// This is the database-derived liveness signal the jobs without one were
/// missing. `failed_jobs` fires only when a job's most recent run *failed*; a
/// job that is not running at all records nothing, so it raised nothing, and
/// the last successful run stayed in place indefinitely. Prices and FX each
/// have their own freshness signal (`prices_stale`, `fx_stale`); the backup —
/// where it matters most, because nothing in the database changes when a backup
/// does or does not happen — had none, nor did `mic-import` or
/// `currency-import` (SCENARIOS T-11/T-02/T-12).
///
/// What it catches is a scheduler that has stopped **while the process is up**:
/// a cron pattern with no future occurrence (accepted at startup — the entry's
/// task logs one ERROR and exits), or a task that has otherwise gone. It is not
/// a record of missed runs: `job_schedule` is rebuilt at every startup from the
/// schedule the new process is running, so a run missed while the server was
/// down is not reported here — the schedule resumes, and the stored instant
/// moves forward with it.
///
/// A **manual-only** job never appears: it has no schedule line by design, so
/// it has no row, and treating "never ran" as "late" is exactly what the
/// `trigger` flag exists to prevent (SCENARIOS T-09/schedule). Neither does a
/// job whose schedule line was *removed* — the startup WARN reports that, and a
/// row kept for a job nobody schedules any more would be an alarm nobody could
/// clear.
#[derive(utoipa::ToSchema, Debug, Serialize, Deserialize)]
pub struct OverdueJob {
    pub name: String,
    /// The cron expression of the entry that is late, as written in the
    /// schedule file (a job may be scheduled on several lines — the price
    /// import has three, one per market — and each is its own row).
    pub cron: String,
    /// The entry's IANA timezone; `None` when the line carries none and the
    /// expression is in server-local time.
    pub timezone: Option<String>,
    /// The instant the scheduler last said this entry was next due, RFC 3339
    /// UTC.
    pub next_run_at: String,
    /// Whole hours between `next_run_at` and now — how long overdue it is.
    pub overdue_hours: i64,
}

/// A job whose most recent run has been `running` for more than
/// [`JOB_RUNNING_STALE_HOURS`]: a run that started and, on the evidence, will
/// never finish.
///
/// The row is what a restart, a `SIGKILL` or a power cut mid-run leaves behind
/// (SCENARIOS T-11) — nothing completes it, because the process that would have
/// is gone. Reported separately from [`OverdueJob`] because the fault is a
/// different one: the schedule may be perfectly alive and still due on time,
/// while a run of it lies open for ever.
///
/// Only the *latest* run per job is considered, as with `failed_jobs`: once a
/// later run has started, the job is running again and the abandoned row is
/// history rather than a live fault.
#[derive(utoipa::ToSchema, Debug, Serialize, Deserialize)]
pub struct StalledJob {
    pub name: String,
    /// RFC 3339 timestamp the run began.
    pub started_at: String,
    /// Whole hours the run has been open.
    pub running_hours: i64,
}

/// One stored schedule row (`job_schedule`, migration 0043) — the next run of
/// one spawned schedule entry, as that entry's own timer last computed it.
#[derive(sqlx::FromRow)]
pub(super) struct JobScheduleRow {
    name: String,
    cron: String,
    timezone: Option<String>,
    next_run_at: String,
}

/// One `job_runs` row still open — a run that started and has not finished.
#[derive(sqlx::FromRow)]
pub(super) struct RunningJobRow {
    name: String,
    started_at: String,
}

/// An RFC 3339 timestamp column as an instant. A stored timestamp that will not
/// parse is a decode failure like any other, never a silently substituted
/// "now": a job's liveness must not be asserted from a value nobody could read.
pub(super) fn parse_instant(value: &str, column: &str) -> Result<DateTime<Utc>, sqlx::Error> {
    DateTime::parse_from_rfc3339(value)
        .map(|t| t.with_timezone(&Utc))
        .map_err(|e| {
            sqlx::Error::Decode(
                format!("{column} {value:?} is not an RFC 3339 timestamp: {e}").into(),
            )
        })
}

/// Scheduled entries whose stored next run is more than
/// [`JOB_OVERDUE_GRACE_HOURS`] in the past (see [`OverdueJob`]).
///
/// An empty table is silent, which is the state every upgraded database starts
/// in: the rows are written by the running scheduler, so until a server has
/// started with this schema there is nothing to be late for. Filtered in Rust
/// rather than SQL because the comparison is between instants, not strings —
/// the stored value is RFC 3339 and must be parsed to be compared honestly.
pub(super) async fn db_overdue_jobs(
    conn: &mut sqlx::SqliteConnection,
    now: DateTime<Utc>,
) -> Result<Vec<OverdueJob>, sqlx::Error> {
    let rows = sqlx::query_as::<_, JobScheduleRow>(
        "SELECT name, cron, timezone, next_run_at FROM job_schedule ORDER BY name, next_run_at",
    )
    .fetch_all(&mut *conn)
    .await?;
    let mut overdue = Vec::new();
    for row in rows {
        let due = parse_instant(&row.next_run_at, "job_schedule.next_run_at")?;
        let late = now - due;
        if late > Duration::hours(JOB_OVERDUE_GRACE_HOURS) {
            overdue.push(OverdueJob {
                name: row.name,
                cron: row.cron,
                timezone: row.timezone,
                next_run_at: row.next_run_at,
                overdue_hours: late.num_hours(),
            });
        }
    }
    Ok(overdue)
}

/// Jobs whose most recent run has been open for more than
/// [`JOB_RUNNING_STALE_HOURS`] (see [`StalledJob`]).
pub(super) async fn db_stalled_jobs(
    conn: &mut sqlx::SqliteConnection,
    now: DateTime<Utc>,
) -> Result<Vec<StalledJob>, sqlx::Error> {
    let rows = sqlx::query_as::<_, RunningJobRow>(
        "SELECT name, started_at FROM job_runs r \
         WHERE id = (SELECT MAX(id) FROM job_runs WHERE name = r.name) \
           AND status = 'running' \
         ORDER BY name",
    )
    .fetch_all(&mut *conn)
    .await?;
    let mut stalled = Vec::new();
    for row in rows {
        let started = parse_instant(&row.started_at, "job_runs.started_at")?;
        let open = now - started;
        if open > Duration::hours(JOB_RUNNING_STALE_HOURS) {
            stalled.push(StalledJob {
                name: row.name,
                started_at: row.started_at,
                running_hours: open.num_hours(),
            });
        }
    }
    Ok(stalled)
}
