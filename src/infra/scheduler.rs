//! Cron-driven scheduler for recurring maintenance jobs.
//!
//! Job *schedules* live in a declarative cron file (5-field Vixie cron:
//! `min hour dom mon dow`, optionally followed by an IANA timezone before the
//! job name) rather than in code — see `schedule.cron`. This module owns a
//! registry mapping a job name to the work it performs, parses a schedule, and
//! spawns one background task per scheduled entry. Jobs fire only at their cron
//! times (no run-on-startup); any job can be run on demand via
//! `POST /jobs/{name}`.
//!
//! Each spawned task logs the next scheduled run at INFO after every run (and at
//! startup), so the live schedule is verifiable from logs without reading code.

use axum::{
    Extension, Json, Router,
    extract::{Path, Query, State},
    http::StatusCode,
    routing::{get, post},
};
use chrono::{DateTime, Local, TimeZone, Utc};
use chrono_tz::Tz;
use croner::Cron;
use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;
use std::{
    collections::HashMap, future::Future, pin::Pin, str::FromStr, sync::Arc, time::Duration,
};

/// A unit of scheduled work. Each call runs the job once, returning `Ok(())` on
/// success or a human-readable error. Jobs do their own detailed INFO logging.
type JobFuture = Pin<Box<dyn Future<Output = Result<(), String>> + Send>>;
type Job = Arc<dyn Fn(JobParams) -> JobFuture + Send + Sync>;

/// Caller-supplied parameters for a manual job trigger, taken from the
/// `POST /jobs/{name}` query string. Currently only the `backup` job reads
/// `suffix` (see [`crate::infra::db::backup`]); every other registered job
/// ignores it. The scheduled loop always passes [`JobParams::default`] — a
/// suffix only makes sense for a deliberately-labelled one-off backup, not
/// the weekly scheduled run.
#[derive(Debug, Default, Clone, Deserialize)]
pub struct JobParams {
    pub suffix: Option<String>,
}

/// A registered job: the work plus a per-job lock serialising its execution.
/// `run_job` holds the lock for the whole run, so a manual trigger can never
/// overlap the scheduled run (or a second trigger) of the same job — a
/// concurrent caller waits and runs after.
pub struct RegisteredJob {
    work: Job,
    lock: tokio::sync::Mutex<()>,
}

impl RegisteredJob {
    fn new(work: Job) -> Arc<Self> {
        Arc::new(Self {
            work,
            lock: tokio::sync::Mutex::new(()),
        })
    }
}

/// Job-name → work. Shared between the spawned schedule tasks and the HTTP
/// trigger handler (injected as an axum `Extension`).
pub type JobRegistry = Arc<HashMap<String, Arc<RegisteredJob>>>;

/// A job's status, surfaced by `GET /jobs` and the Jobs UI. Every registered
/// job appears in the list; the `last_*` fields are `None` (and `runs` empty)
/// until the job has run at least once. `runs` is the job's stored run history,
/// most recent first, bounded to [`JOB_RUN_HISTORY_LIMIT`] entries — the
/// `last_*` fields duplicate `runs[0]` for at-a-glance reading.
#[derive(Debug, Serialize, Deserialize)]
pub struct JobStatus {
    pub name: String,
    pub last_started_at: Option<String>,
    pub last_finished_at: Option<String>,
    pub last_success: Option<bool>,
    pub last_error: Option<String>,
    pub runs: Vec<JobRunRecord>,
}

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
async fn db_record_run(
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
async fn db_run_histories(
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

/// Why a schedule file was rejected. Carries the 1-based line number so a bad
/// `schedule.cron` is easy to fix.
#[derive(Debug)]
pub enum ScheduleError {
    /// A line was malformed: too few fields, or an unparseable cron expression.
    Parse { line: usize, msg: String },
    /// A line referenced a job name that is not in the registry.
    UnknownJob { line: usize, name: String },
}

impl std::fmt::Display for ScheduleError {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        match self {
            ScheduleError::Parse { line, msg } => write!(f, "schedule line {line}: {msg}"),
            ScheduleError::UnknownJob { line, name } => {
                write!(f, "schedule line {line}: no such job {name:?}")
            }
        }
    }
}

impl std::error::Error for ScheduleError {}

/// Build the registry of maintenance jobs, wiring each name to existing work
/// functions. Adding a future job is a new entry here plus a line in the
/// schedule file. The price source is injected (not constructed here) so the
/// live `YahooFetcher` only ever reaches the registry from `main`; tests pass a
/// stub and never touch the network.
pub fn registry(
    pool: SqlitePool,
    db_path: String,
    backup_dir: Option<String>,
    backup_command: Option<String>,
    fetcher: crate::entities::closing_price::SharedFetcher,
) -> JobRegistry {
    let mut jobs: HashMap<String, Arc<RegisteredJob>> = HashMap::new();

    let backup_pool = pool.clone();
    jobs.insert(
        "backup".to_string(),
        RegisteredJob::new(Arc::new(move |params: JobParams| {
            let pool = backup_pool.clone();
            let db_path = db_path.clone();
            let backup_dir = backup_dir.clone();
            let backup_command = backup_command.clone();
            Box::pin(async move {
                crate::infra::db::backup(
                    &pool,
                    &db_path,
                    backup_dir.as_deref(),
                    backup_command.as_deref(),
                    params.suffix.as_deref(),
                )
                .await
                .map_err(|e| e.to_string())
            })
        })),
    );

    let mic_pool = pool.clone();
    jobs.insert(
        "mic-import".to_string(),
        RegisteredJob::new(Arc::new(move |_params: JobParams| {
            let pool = mic_pool.clone();
            Box::pin(async move {
                match crate::entities::mic_registry::run_import(&pool).await {
                    Ok(s) => {
                        tracing::info!(imported = s.imported, "MIC registry import complete");
                        Ok(())
                    }
                    Err(e) => Err(format!("{e:?}")),
                }
            })
        })),
    );

    let currency_pool = pool.clone();
    jobs.insert(
        "currency-import".to_string(),
        RegisteredJob::new(Arc::new(move |_params: JobParams| {
            let pool = currency_pool.clone();
            Box::pin(async move {
                match crate::entities::currencies::run_import(&pool).await {
                    Ok(s) => {
                        tracing::info!(imported = s.imported, "currency import complete");
                        Ok(())
                    }
                    Err(e) => Err(format!("{e:?}")),
                }
            })
        })),
    );

    let price_pool = pool.clone();
    let price_fetcher = fetcher;
    jobs.insert(
        "price-import".to_string(),
        RegisteredJob::new(Arc::new(move |_params: JobParams| {
            let pool = price_pool.clone();
            let fetcher = price_fetcher.clone();
            Box::pin(async move {
                crate::entities::closing_price::run_collection(
                    &pool,
                    fetcher.as_ref(),
                    chrono::Utc::now(),
                )
                .await
            })
        })),
    );

    let snapshot_pool = pool.clone();
    jobs.insert(
        "report-snapshot".to_string(),
        RegisteredJob::new(Arc::new(move |_params: JobParams| {
            let pool = snapshot_pool.clone();
            Box::pin(async move {
                crate::reports::snapshot::run_snapshot_job(&pool, chrono::Utc::now()).await
            })
        })),
    );

    let fx_pool = pool.clone();
    jobs.insert(
        "rba-fx-import".to_string(),
        RegisteredJob::new(Arc::new(move |_params: JobParams| {
            let pool = fx_pool.clone();
            Box::pin(async move {
                let summary = match crate::entities::rba_fx_rate::run_import(&pool).await {
                    Ok(s) => {
                        tracing::info!(
                            inserted = s.inserted,
                            skipped = s.skipped,
                            "RBA FX rate import complete"
                        );
                        s
                    }
                    Err(e) => return Err(format!("{e:?}")),
                };
                // New rates may finalise provisional snapshots in this same
                // run; a blocked true-up date fails the job so the Jobs UI
                // surfaces it (the import itself succeeded and is idempotent).
                match crate::entities::rba_fx_rate::true_up_provisional_snapshots(&pool, &summary)
                    .await
                {
                    Ok(None) => Ok(()),
                    Ok(Some(t)) if t.blocked.is_empty() => Ok(()),
                    Ok(Some(t)) => Err(format!(
                        "import ok ({} new rates); provisional snapshot true-up blocked: {}",
                        summary.inserted,
                        t.blocked
                            .iter()
                            .map(|b| format!("{}: {}", b.date, b.reason))
                            .collect::<Vec<_>>()
                            .join("; ")
                    )),
                    Err(e) => Err(format!("provisional snapshot true-up failed: {e}")),
                }
            })
        })),
    );

    Arc::new(jobs)
}

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
async fn run_job(
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

/// One parsed schedule line: when to fire, in which timezone (`None` = the
/// server's local timezone), and which registered job to run. `line` is the
/// 1-based line number in the schedule file (comments and blank lines count),
/// so validation errors point at the real line.
#[derive(Debug)]
struct ScheduleEntry {
    line: usize,
    cron: Cron,
    tz: Option<Tz>,
    name: String,
}

/// Parse a cron schedule file. Lines are
/// `<min> <hour> <dom> <mon> <dow> [timezone] <job-name>`; the optional
/// timezone is an IANA name (e.g. `America/New_York`) pinning the cron
/// expression to that zone — absent, the expression is server-local time.
/// `#` starts a comment and blank lines are ignored.
fn parse(schedule: &str) -> Result<Vec<ScheduleEntry>, ScheduleError> {
    let mut entries = Vec::new();

    for (idx, raw) in schedule.lines().enumerate() {
        let line = idx + 1;
        let content = raw.split('#').next().unwrap_or("").trim();
        if content.is_empty() {
            continue;
        }

        let fields: Vec<&str> = content.split_whitespace().collect();
        let (tz, name) = match fields.len() {
            6 => (None, fields[5]),
            7 => {
                let tz = fields[5].parse::<Tz>().map_err(|_| ScheduleError::Parse {
                    line,
                    msg: format!(
                        "unknown timezone {:?} (expected an IANA name like Australia/Sydney)",
                        fields[5]
                    ),
                })?;
                (Some(tz), fields[6])
            }
            n => {
                return Err(ScheduleError::Parse {
                    line,
                    msg: format!(
                        "expected 5 cron fields, an optional IANA timezone, \
                         and a job name, got {n} field(s)"
                    ),
                });
            }
        };

        let expr = fields[..5].join(" ");
        let cron = Cron::from_str(&expr).map_err(|e| ScheduleError::Parse {
            line,
            msg: format!("invalid cron {expr:?}: {e}"),
        })?;
        entries.push(ScheduleEntry {
            line,
            cron,
            tz,
            name: name.to_string(),
        });
    }

    Ok(entries)
}

/// Parse the schedule, validate every entry against the registry, and spawn one
/// background task per entry. Returns an error (without spawning anything) if
/// the schedule is malformed or names an unregistered job.
pub fn spawn(registry: JobRegistry, pool: SqlitePool, schedule: &str) -> Result<(), ScheduleError> {
    let entries = parse(schedule)?;

    // Validate all names up front so a bad file fails fast at startup rather
    // than spawning a partial set of tasks.
    for entry in &entries {
        if !registry.contains_key(&entry.name) {
            return Err(ScheduleError::UnknownJob {
                line: entry.line,
                name: entry.name.clone(),
            });
        }
    }

    // A registered job with no schedule line never runs automatically (only via
    // POST /jobs/{name}). That is usually an oversight, so warn rather than fail.
    for name in registry.keys() {
        if !entries.iter().any(|entry| &entry.name == name) {
            tracing::warn!(
                job = %name,
                "registered job has no schedule entry; it will only run via POST /jobs/{name}"
            );
        }
    }

    for entry in entries {
        let job = registry[&entry.name].clone();
        let pool = pool.clone();
        // The two arms differ only in the timezone type the clock yields
        // (`DateTime<Tz>` vs `DateTime<Local>`), so each instantiates the same
        // generic loop.
        match entry.tz {
            Some(tz) => {
                tokio::spawn(run_entry(pool, entry.name, job, entry.cron, move || {
                    Utc::now().with_timezone(&tz)
                }));
            }
            None => {
                tokio::spawn(run_entry(pool, entry.name, job, entry.cron, Local::now));
            }
        }
    }

    Ok(())
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
async fn run_entry<Z>(
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
fn next_run<Z: TimeZone>(cron: &Cron, now: DateTime<Z>) -> Option<(DateTime<Z>, Duration)> {
    let next = cron.find_next_occurrence(&now, false).ok()?;
    // `next` is strictly after `now`, so the difference is non-negative; the
    // fallback only guards against clock skew between this read and the diff.
    let delay = (next.clone() - now)
        .to_std()
        .unwrap_or(Duration::from_secs(1));
    Some((next, delay))
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
            JobStatus {
                name,
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::infra::db;
    use axum::body::Body;
    use axum::http::Request;
    use chrono::TimeZone;
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    /// An offline price-source stub so building a registry never constructs the
    /// live `YahooFetcher` (these tests trigger only the `backup` job, but the
    /// stub guarantees no test path can reach the network).
    fn stub_fetcher() -> crate::entities::closing_price::SharedFetcher {
        crate::entities::closing_price::test_support::QuoteStub::default().shared()
    }

    async fn test_registry() -> (JobRegistry, SqlitePool, tempfile::TempDir, String) {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("test.db").to_string_lossy().to_string();
        let pool = db::init(&db_path).await.unwrap();
        (
            registry(pool.clone(), db_path.clone(), None, None, stub_fetcher()),
            pool,
            dir,
            db_path,
        )
    }

    #[test]
    fn parse_ignores_comments_and_blank_lines() {
        let schedule = "# a comment\n\n0 0 * * *   backup   # trailing comment\n";
        let entries = parse(schedule).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].name, "backup");
        assert_eq!(entries[0].tz, None);
    }

    #[test]
    fn parse_accepts_timezone_field() {
        let entries = parse("30 16 * * 1-5  America/New_York  price-import\n").unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].tz, Some(chrono_tz::America::New_York));
        assert_eq!(entries[0].name, "price-import");
    }

    #[test]
    fn parse_rejects_unknown_timezone() {
        // A non-IANA zone name (here a bare city) must fail at startup with the
        // line number, not silently fall back to local time.
        let err = parse("# comment\n0 0 * * *   Sydney   backup\n").unwrap_err();
        match err {
            ScheduleError::Parse { line, msg } => {
                assert_eq!(line, 2);
                assert!(msg.contains("unknown timezone"), "{msg}");
            }
            other => panic!("expected Parse error, got {other:?}"),
        }
    }

    #[test]
    fn parse_rejects_extra_fields() {
        let err = parse("0 0 * * *   UTC   backup   extra\n").unwrap_err();
        assert!(matches!(err, ScheduleError::Parse { line: 1, .. }));
    }

    #[test]
    fn parse_rejects_too_few_fields() {
        let err = parse("0 0 * * backup\n").unwrap_err();
        match err {
            ScheduleError::Parse { line, .. } => assert_eq!(line, 1),
            other => panic!("expected Parse error, got {other:?}"),
        }
    }

    #[test]
    fn parse_rejects_malformed_cron() {
        let err = parse("99 0 * * *   backup\n").unwrap_err();
        assert!(matches!(err, ScheduleError::Parse { line: 1, .. }));
    }

    #[test]
    fn daily_midnight_cron_computes_next_local_midnight() {
        let cron = Cron::from_str("0 0 * * *").unwrap();
        let from = Local.with_ymd_and_hms(2026, 5, 31, 9, 30, 0).unwrap();
        let next = cron.find_next_occurrence(&from, false).unwrap();
        let expected = Local.with_ymd_and_hms(2026, 6, 1, 0, 0, 0).unwrap();
        assert_eq!(next, expected);
    }

    #[test]
    fn next_run_delay_lands_exactly_on_next_no_truncation() {
        // Regression for a busy-spin: with a sub-second remainder, truncating the
        // delay to whole seconds (the old `num_seconds()`) makes the timer wake
        // *before* `next`, so the recomputed run is the same instant and the loop
        // spins. The exact delay must land precisely on `next`. Check both just
        // before and just after a minute boundary on a per-minute schedule.
        let cron = Cron::from_str("* * * * *").unwrap();
        for offset_ms in [400, 999, 10] {
            let now = Local.with_ymd_and_hms(2026, 5, 31, 22, 41, 59).unwrap()
                + chrono::Duration::milliseconds(offset_ms);
            let (next, delay) = next_run(&cron, now).unwrap();
            assert!(next > now, "next {next} must be strictly after now {now}");
            let slept = chrono::Duration::from_std(delay).unwrap();
            assert_eq!(
                now + slept,
                next,
                "delay must land exactly on next, not before"
            );
        }
    }

    #[tokio::test]
    async fn embedded_schedule_is_valid() {
        let (reg, pool, _dir, _path) = test_registry().await;
        // Guards the committed schedule.cron: every referenced job must exist.
        spawn(reg, pool, include_str!("../../schedule.cron")).unwrap();
    }

    #[test]
    fn backup_is_scheduled_weekly() {
        // REQUIREMENTS specifies weekly backups: the committed schedule's backup
        // entry must parse and fire exactly 7 days apart.
        let entries = parse(include_str!("../../schedule.cron")).unwrap();
        let entry = entries
            .iter()
            .find(|e| e.name == "backup")
            .expect("backup must be scheduled");
        let from = Local.with_ymd_and_hms(2026, 6, 1, 12, 0, 0).unwrap();
        let first = entry.cron.find_next_occurrence(&from, false).unwrap();
        let second = entry.cron.find_next_occurrence(&first, false).unwrap();
        assert_eq!(second - first, chrono::Duration::days(7));
    }

    #[test]
    fn price_imports_are_scheduled_in_market_timezones() {
        // Each committed price-import entry is pinned to its market's zone so a
        // DST transition at either end can't shift the run's margin over that
        // market's close.
        let entries = parse(include_str!("../../schedule.cron")).unwrap();
        let zones: Vec<Option<Tz>> = entries
            .iter()
            .filter(|e| e.name == "price-import")
            .map(|e| e.tz)
            .collect();
        assert_eq!(zones.len(), 3, "ASX, NYSE, and crypto-UTC runs expected");
        assert!(zones.contains(&Some(chrono_tz::Australia::Sydney)));
        assert!(zones.contains(&Some(chrono_tz::America::New_York)));
        assert!(zones.contains(&Some(chrono_tz::UTC)));
    }

    #[test]
    fn next_occurrence_computed_in_entry_timezone() {
        // 16:30 weekdays in New York, evaluated from a UTC instant: 2026-06-11
        // (a Thursday) 18:00 UTC is 14:00 EDT, so the next fire is 16:30 EDT
        // = 20:30 UTC the same day — not 16:30 in UTC or server-local time.
        let tz: Tz = "America/New_York".parse().unwrap();
        let cron = Cron::from_str("30 16 * * 1-5").unwrap();
        let now = Utc
            .with_ymd_and_hms(2026, 6, 11, 18, 0, 0)
            .unwrap()
            .with_timezone(&tz);
        let (next, delay) = next_run(&cron, now).unwrap();
        assert_eq!(next, Utc.with_ymd_and_hms(2026, 6, 11, 20, 30, 0).unwrap());
        assert_eq!(delay, Duration::from_secs(2 * 3600 + 30 * 60));
        // The "next run scheduled" log line formats this value with %Z, which
        // renders the entry zone's abbreviation.
        assert_eq!(
            next.format("%Y-%m-%d %H:%M:%S %Z").to_string(),
            "2026-06-11 16:30:00 EDT"
        );
    }

    #[test]
    fn dst_gap_fires_at_first_valid_instant_after_gap() {
        // Sydney springs forward on 2026-10-04: 02:00 AEST → 03:00 AEDT, so
        // 02:30 does not exist that day. The occurrence lands on the first
        // valid instant after the gap (03:00 AEDT) — the day is neither
        // skipped nor an error.
        let tz: Tz = "Australia/Sydney".parse().unwrap();
        let cron = Cron::from_str("30 2 * * *").unwrap();
        let now = tz.with_ymd_and_hms(2026, 10, 4, 1, 0, 0).unwrap();
        let (next, _) = next_run(&cron, now).unwrap();
        assert_eq!(next, tz.with_ymd_and_hms(2026, 10, 4, 3, 0, 0).unwrap());
        assert_eq!(next.format("%Z").to_string(), "AEDT");
        // The following day, 02:30 exists again.
        let (after, _) = next_run(&cron, next).unwrap();
        assert_eq!(after, tz.with_ymd_and_hms(2026, 10, 5, 2, 30, 0).unwrap());
    }

    #[test]
    fn dst_fold_fires_once_at_first_occurrence() {
        // Sydney falls back on 2026-04-05: 03:00 AEDT → 02:00 AEST, so 02:30
        // occurs twice. The job fires once, at the first (AEDT) occurrence,
        // and not again in the repeated hour.
        let tz: Tz = "Australia/Sydney".parse().unwrap();
        let cron = Cron::from_str("30 2 * * *").unwrap();
        let now = tz.with_ymd_and_hms(2026, 4, 5, 1, 0, 0).unwrap();
        let (next, _) = next_run(&cron, now).unwrap();
        let first = tz
            .with_ymd_and_hms(2026, 4, 5, 2, 30, 0)
            .earliest()
            .unwrap();
        assert_eq!(next, first);
        assert_eq!(next.format("%Z").to_string(), "AEDT");
        // After the first occurrence, the next fire is the following day —
        // not the second (AEST) 02:30 of the fold.
        let (after, _) = next_run(&cron, next).unwrap();
        assert_eq!(after, tz.with_ymd_and_hms(2026, 4, 6, 2, 30, 0).unwrap());
    }

    #[tokio::test]
    async fn capped_sleep_reanchors_after_wall_clock_shift() {
        // The wall-clock target is 03:30, three and a half hours of wall time
        // away — but the wall clock jumps +1h mid-sleep (as a DST
        // spring-forward or an NTP step would), so the target arrives after
        // only 2.5h of monotonic time. A single uncapped 3.5h monotonic sleep
        // would fire an hour late on the wall clock; the capped loop recomputes
        // every MAX_SLEEP and re-anchors, firing exactly at 03:30 wall time.
        //
        // The pool is created before pausing the clock: under the paused clock
        // tokio auto-advances past sqlx's acquire timeout while the sqlite
        // worker thread is still opening the database, failing pool init.
        let pool = db::init(":memory:").await.unwrap();
        tokio::time::pause();
        let t0 = Utc.with_ymd_and_hms(2026, 6, 1, 0, 0, 0).unwrap();
        let start = tokio::time::Instant::now();
        let clock = move || {
            let elapsed = tokio::time::Instant::now() - start;
            let mut now = t0 + chrono::Duration::from_std(elapsed).unwrap();
            if elapsed >= Duration::from_secs(90 * 60) {
                now += chrono::Duration::hours(1); // the wall-clock shift
            }
            now
        };

        let fired_at = Arc::new(std::sync::Mutex::new(Vec::<DateTime<Utc>>::new()));
        let fired = fired_at.clone();
        let job = RegisteredJob::new(Arc::new(move |_params: JobParams| {
            let fired = fired.clone();
            let now = clock();
            Box::pin(async move {
                fired.lock().unwrap().push(now);
                Ok(())
            })
        }));

        let cron = Cron::from_str("30 3 1 6 *").unwrap(); // 03:30 on 2026-06-01
        tokio::spawn(run_entry(pool, "fake".to_string(), job, cron, clock));
        // Paused time auto-advances through the loop's sleeps; run well past
        // the target, then check the fire time against the shifted wall clock.
        tokio::time::sleep(Duration::from_secs(4 * 3600)).await;

        let fired = fired_at.lock().unwrap();
        assert_eq!(fired.len(), 1, "job must fire exactly once");
        // Tokio's paused clock rounds each auto-advanced sleep up by 1ms, so
        // allow a few ms of slack — the failure mode being guarded against is
        // firing a whole hour late on the wall clock.
        let target = Utc.with_ymd_and_hms(2026, 6, 1, 3, 30, 0).unwrap();
        let off_by = (fired[0] - target).num_milliseconds().abs();
        assert!(
            off_by < 1000,
            "fired at {} — {off_by}ms from the wall-clock target {target}",
            fired[0]
        );
    }

    #[tracing_test::traced_test]
    #[tokio::test]
    async fn next_run_log_shows_timezone() {
        let (reg, pool, _dir, _path) = test_registry().await;
        spawn(reg, pool, "0 0 * * *   Pacific/Auckland   backup\n").unwrap();
        // The spawned task logs its first "next run scheduled" before any
        // await; yield so it gets to run.
        for _ in 0..50 {
            tokio::task::yield_now().await;
        }
        assert!(logs_contain("next run scheduled"));
        assert!(
            logs_contain("NZST") || logs_contain("NZDT"),
            "the next-run log line must carry the entry timezone's %Z name"
        );
    }

    #[tracing_test::traced_test]
    #[tokio::test]
    async fn spawn_warns_about_job_with_no_schedule_entry() {
        // Registry has `backup` and `rba-fx-import`; schedule only mentions backup.
        let (reg, pool, _dir, _path) = test_registry().await;
        spawn(reg, pool, "0 0 * * *   backup\n").unwrap();
        assert!(logs_contain("registered job has no schedule entry"));
        assert!(logs_contain("rba-fx-import"));
    }

    #[tokio::test]
    async fn spawn_rejects_unknown_job() {
        let (reg, pool, _dir, _path) = test_registry().await;
        let err = spawn(reg, pool, "0 0 * * *   no-such-job\n").unwrap_err();
        assert!(matches!(err, ScheduleError::UnknownJob { .. }));
    }

    #[tokio::test]
    async fn unknown_job_error_reports_file_line_not_entry_index() {
        // Comments and blank lines shift parsed-entry indexes away from file
        // lines: `no-such-job` is the 2nd parsed entry but sits on file line 5.
        // The error must point at line 5, where the user will look.
        let (reg, pool, _dir, _path) = test_registry().await;
        let schedule = "# weekly maintenance\n\n0 0 * * 0   backup\n# bad line below\n0 1 * * *   no-such-job\n";
        let err = spawn(reg, pool, schedule).unwrap_err();
        match err {
            ScheduleError::UnknownJob { line, name } => {
                assert_eq!(line, 5);
                assert_eq!(name, "no-such-job");
            }
            other => panic!("expected UnknownJob error, got {other:?}"),
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn concurrent_runs_of_same_job_serialise() {
        // Two run_job calls for the same job (e.g. a manual trigger overlapping
        // the scheduled run) must never execute the work concurrently: the
        // second waits on the per-job lock and runs after the first finishes.
        use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
        let pool = db::init(":memory:").await.unwrap();
        let active = Arc::new(AtomicUsize::new(0));
        let overlapped = Arc::new(AtomicBool::new(false));
        let runs = Arc::new(AtomicUsize::new(0));
        let (a, o, r) = (active.clone(), overlapped.clone(), runs.clone());
        let job = RegisteredJob::new(Arc::new(move |_params: JobParams| {
            let (a, o, r) = (a.clone(), o.clone(), r.clone());
            Box::pin(async move {
                if a.fetch_add(1, Ordering::SeqCst) > 0 {
                    o.store(true, Ordering::SeqCst);
                }
                tokio::time::sleep(Duration::from_millis(30)).await;
                a.fetch_sub(1, Ordering::SeqCst);
                r.fetch_add(1, Ordering::SeqCst);
                Ok(())
            })
        }));

        let (r1, r2) = tokio::join!(
            run_job(&pool, "same-job", &job, JobParams::default()),
            run_job(&pool, "same-job", &job, JobParams::default()),
        );
        r1.unwrap();
        r2.unwrap();
        assert_eq!(runs.load(Ordering::SeqCst), 2, "both runs must complete");
        assert!(
            !overlapped.load(Ordering::SeqCst),
            "the second run must wait for the first, not execute concurrently"
        );
    }

    #[tokio::test]
    async fn trigger_backup_runs_and_returns_204() {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("t.db").to_string_lossy().to_string();
        let pool = db::init(&db_path).await.unwrap();
        let reg = registry(pool.clone(), db_path.clone(), None, None, stub_fetcher());
        let app = router().with_state(pool).layer(Extension(reg));

        let resp = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/jobs/backup")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::NO_CONTENT);
        // The backup job derives its own timestamped name (`t-YYYY-MM-DD-HHMMSS.db`),
        // so re-deriving the path here could land in a later second and miss it.
        // Assert instead that a backup file for this DB was written to the dir.
        let made_backup = std::fs::read_dir(dir.path())
            .unwrap()
            .filter_map(|e| e.ok())
            .any(|e| {
                let name = e.file_name().to_string_lossy().into_owned();
                name.starts_with("t-") && name.ends_with(".db")
            });
        assert!(
            made_backup,
            "expected a timestamped backup file beside t.db"
        );
    }

    #[tokio::test]
    async fn trigger_backup_with_suffix_writes_suffixed_file() {
        // POST /jobs/backup?suffix=... (the update.sh pre-upgrade path) must
        // reach the backup job's suffix param end-to-end through the HTTP
        // layer, not just the db::backup unit tests.
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("t.db").to_string_lossy().to_string();
        let pool = db::init(&db_path).await.unwrap();
        let reg = registry(pool.clone(), db_path.clone(), None, None, stub_fetcher());
        let app = router().with_state(pool).layer(Extension(reg));

        let resp = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/jobs/backup?suffix=pre-0.5.1")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::NO_CONTENT);
        let made_backup = std::fs::read_dir(dir.path())
            .unwrap()
            .filter_map(|e| e.ok())
            .any(|e| {
                let name = e.file_name().to_string_lossy().into_owned();
                name.starts_with("t-") && name.ends_with("-pre-0.5.1.db")
            });
        assert!(made_backup, "expected a suffixed backup file beside t.db");
    }

    #[tokio::test]
    async fn trigger_with_invalid_suffix_returns_422_and_records_no_run() {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("t.db").to_string_lossy().to_string();
        let pool = db::init(&db_path).await.unwrap();
        let reg = registry(pool.clone(), db_path, None, None, stub_fetcher());
        let app = router().with_state(pool.clone()).layer(Extension(reg));

        let resp = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/jobs/backup?suffix=../etc/passwd")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY);
        // Beside "t.db" itself, WAL-mode sidecars (t.db-wal, t.db-shm) are
        // expected; only a backup-named file would indicate the rejected
        // suffix reached the filesystem.
        let no_backup = std::fs::read_dir(dir.path())
            .unwrap()
            .filter_map(|e| e.ok())
            .all(|e| {
                let name = e.file_name().to_string_lossy().into_owned();
                name == "t.db" || name.starts_with("t.db-")
            });
        assert!(
            no_backup,
            "an invalid suffix must never reach the filesystem"
        );

        // A rejected request never called run_job, so no run was recorded.
        let runs: Vec<(String,)> = sqlx::query_as("SELECT name FROM job_runs")
            .fetch_all(&pool)
            .await
            .unwrap();
        assert!(
            runs.is_empty(),
            "an invalid suffix must be rejected before run_job, recording no run"
        );
    }

    #[tokio::test]
    async fn scheduled_backup_takes_no_suffix() {
        // The scheduled loop always passes JobParams::default(): the weekly
        // run must keep producing the plain, unsuffixed name.
        let (reg, pool, dir, db_path) = test_registry().await;
        let job = reg.get("backup").unwrap();
        run_job(&pool, "backup", job, JobParams::default())
            .await
            .unwrap();

        let stem = std::path::Path::new(&db_path)
            .file_stem()
            .unwrap()
            .to_string_lossy()
            .into_owned();
        let has_suffix = std::fs::read_dir(dir.path())
            .unwrap()
            .filter_map(|e| e.ok())
            .any(|e| {
                let name = e.file_name().to_string_lossy().into_owned();
                name.starts_with(&format!("{stem}-")) && name.matches('-').count() > 4
            });
        assert!(
            !has_suffix,
            "the scheduled run must not append a suffix to the backup filename"
        );
    }

    #[tokio::test]
    async fn backup_job_honours_configured_backup_dir() {
        // The --backup-dir option must reach the scheduled backup job: with a
        // dir configured, the job writes there, not beside the database file.
        let db_dir = tempfile::tempdir().unwrap();
        let backup_dir = tempfile::tempdir().unwrap();
        let db_path = db_dir.path().join("t.db").to_string_lossy().to_string();
        let pool = db::init(&db_path).await.unwrap();
        let reg = registry(
            pool.clone(),
            db_path,
            Some(backup_dir.path().to_string_lossy().into_owned()),
            None,
            stub_fetcher(),
        );

        let job = reg.get("backup").unwrap();
        run_job(&pool, "backup", job, JobParams::default())
            .await
            .unwrap();

        let in_backup_dir = std::fs::read_dir(backup_dir.path())
            .unwrap()
            .filter_map(|e| e.ok())
            .any(|e| {
                let name = e.file_name().to_string_lossy().into_owned();
                name.starts_with("t-") && name.ends_with(".db")
            });
        assert!(in_backup_dir, "backup job must write to the configured dir");
        let beside_db = std::fs::read_dir(db_dir.path())
            .unwrap()
            .filter_map(|e| e.ok())
            .any(|e| e.file_name().to_string_lossy().starts_with("t-"));
        assert!(!beside_db, "backup must not also land beside the db");
    }

    #[tokio::test]
    async fn backup_job_honours_configured_backup_command() {
        // The --backup-command / backup_command config option must reach the
        // scheduled backup job: end-to-end through the registry, not just the
        // db::backup unit tests.
        let db_dir = tempfile::tempdir().unwrap();
        let db_path = db_dir.path().join("t.db").to_string_lossy().to_string();
        let pool = db::init(&db_path).await.unwrap();
        let marker = db_dir.path().join("hook-ran");
        let command = format!("touch {}", marker.to_string_lossy());
        let reg = registry(pool.clone(), db_path, None, Some(command), stub_fetcher());

        let job = reg.get("backup").unwrap();
        run_job(&pool, "backup", job, JobParams::default())
            .await
            .unwrap();

        assert!(
            marker.exists(),
            "the configured backup_command must have run"
        );
    }

    #[tracing_test::traced_test]
    #[tokio::test]
    async fn run_job_logs_started_and_finished() {
        // The scheduled loop runs each job via run_job, so this covers the
        // scheduled path: a job must be bracketed by INFO start/finish lines.
        let (reg, pool, _dir, _path) = test_registry().await;
        let job = reg.get("backup").unwrap();
        run_job(&pool, "backup", job, JobParams::default())
            .await
            .unwrap();
        assert!(logs_contain("job started"));
        assert!(logs_contain("job finished"));
    }

    #[tracing_test::traced_test]
    #[tokio::test]
    async fn triggered_job_logs_started_and_finished() {
        // The manual POST /jobs/{name} path also goes through run_job.
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("t.db").to_string_lossy().to_string();
        let pool = db::init(&db_path).await.unwrap();
        let reg = registry(pool.clone(), db_path, None, None, stub_fetcher());
        let app = router().with_state(pool).layer(Extension(reg));

        let resp = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/jobs/backup")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::NO_CONTENT);
        assert!(logs_contain("job started"));
        assert!(logs_contain("job finished"));
    }

    #[tokio::test]
    async fn trigger_unknown_job_returns_404() {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("t.db").to_string_lossy().to_string();
        let pool = db::init(&db_path).await.unwrap();
        let reg = registry(pool.clone(), db_path, None, None, stub_fetcher());
        let app = router().with_state(pool).layer(Extension(reg));

        let resp = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/jobs/does-not-exist")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn list_jobs_returns_registered_names() {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("t.db").to_string_lossy().to_string();
        let pool = db::init(&db_path).await.unwrap();
        let reg = registry(pool.clone(), db_path, None, None, stub_fetcher());
        let app = router().with_state(pool).layer(Extension(reg));

        let resp = app
            .oneshot(Request::builder().uri("/jobs").body(Body::empty()).unwrap())
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::OK);
        let body = resp.into_body().collect().await.unwrap().to_bytes();
        let statuses: Vec<JobStatus> = serde_json::from_slice(&body).unwrap();
        let names: Vec<&str> = statuses.iter().map(|s| s.name.as_str()).collect();
        assert!(names.contains(&"backup"));
        assert!(names.contains(&"rba-fx-import"));
        // A job that has never run reports no last-run details.
        let backup = statuses.iter().find(|s| s.name == "backup").unwrap();
        assert!(backup.last_started_at.is_none());
        assert!(backup.last_success.is_none());
    }

    #[tokio::test]
    async fn run_job_records_successful_last_run() {
        // After a successful run, GET /jobs surfaces the recorded last run with
        // success = true and no error.
        let (reg, pool, _dir, _path) = test_registry().await;
        let job = reg.get("backup").unwrap();
        run_job(&pool, "backup", job, JobParams::default())
            .await
            .unwrap();

        let app = router().with_state(pool).layer(Extension(reg));
        let resp = app
            .oneshot(Request::builder().uri("/jobs").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = resp.into_body().collect().await.unwrap().to_bytes();
        let statuses: Vec<JobStatus> = serde_json::from_slice(&body).unwrap();
        let backup = statuses.iter().find(|s| s.name == "backup").unwrap();
        assert!(backup.last_started_at.is_some());
        assert!(backup.last_finished_at.is_some());
        assert_eq!(backup.last_success, Some(true));
        assert!(backup.last_error.is_none());
    }

    #[tokio::test]
    async fn record_run_keeps_history_latest_first() {
        // A failed run stores success = 0 and the error text; a later success
        // for the same job becomes the latest run while the failure stays in
        // the history (an intermittent failure leaves a trace).
        let (_reg, pool, _dir, _path) = test_registry().await;
        db_record_run(
            &pool,
            "backup",
            "2026-06-01T00:00:00Z",
            "2026-06-01T00:00:01Z",
            false,
            Some("boom"),
        )
        .await
        .unwrap();
        db_record_run(
            &pool,
            "backup",
            "2026-06-02T00:00:00Z",
            "2026-06-02T00:00:01Z",
            true,
            None,
        )
        .await
        .unwrap();

        let histories = db_run_histories(&pool).await.unwrap();
        let runs = histories.get("backup").unwrap();
        assert_eq!(runs.len(), 2);
        assert!(runs[0].success);
        assert!(runs[0].error.is_none());
        assert_eq!(runs[0].started_at, "2026-06-02T00:00:00Z");
        assert!(!runs[1].success);
        assert_eq!(runs[1].error.as_deref(), Some("boom"));
    }

    #[tokio::test]
    async fn run_history_is_pruned_to_the_limit_per_job() {
        // Recording a run prunes that job's history to the newest
        // JOB_RUN_HISTORY_LIMIT rows in the same write; other jobs' histories
        // are untouched.
        let (_reg, pool, _dir, _path) = test_registry().await;
        db_record_run(
            &pool,
            "other",
            "2026-05-01T00:00:00Z",
            "2026-05-01T00:00:01Z",
            true,
            None,
        )
        .await
        .unwrap();
        let extra = 5;
        for i in 0..(JOB_RUN_HISTORY_LIMIT + extra) {
            let started = format!("2026-06-01T00:{i:02}:00Z");
            let finished = format!("2026-06-01T00:{i:02}:01Z");
            db_record_run(&pool, "backup", &started, &finished, true, None)
                .await
                .unwrap();
        }

        let histories = db_run_histories(&pool).await.unwrap();
        let runs = histories.get("backup").unwrap();
        assert_eq!(runs.len(), JOB_RUN_HISTORY_LIMIT as usize);
        // The newest runs survive; the oldest `extra` were pruned.
        assert_eq!(
            runs[0].started_at,
            format!("2026-06-01T00:{:02}:00Z", JOB_RUN_HISTORY_LIMIT + extra - 1)
        );
        assert_eq!(
            runs.last().unwrap().started_at,
            format!("2026-06-01T00:{extra:02}:00Z")
        );
        assert_eq!(histories.get("other").unwrap().len(), 1);
    }

    #[tokio::test]
    async fn list_jobs_exposes_run_history() {
        // GET /jobs carries each job's stored history (most recent first) in
        // `runs`, with the `last_*` fields mirroring the newest entry.
        let (reg, pool, _dir, _path) = test_registry().await;
        db_record_run(
            &pool,
            "backup",
            "2026-06-01T00:00:00Z",
            "2026-06-01T00:00:01Z",
            false,
            Some("boom"),
        )
        .await
        .unwrap();
        db_record_run(
            &pool,
            "backup",
            "2026-06-02T00:00:00Z",
            "2026-06-02T00:00:01Z",
            true,
            None,
        )
        .await
        .unwrap();

        let app = router().with_state(pool).layer(Extension(reg));
        let resp = app
            .oneshot(Request::builder().uri("/jobs").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = resp.into_body().collect().await.unwrap().to_bytes();
        let statuses: Vec<JobStatus> = serde_json::from_slice(&body).unwrap();
        let backup = statuses.iter().find(|s| s.name == "backup").unwrap();
        assert_eq!(backup.runs.len(), 2);
        assert!(backup.runs[0].success);
        assert_eq!(backup.runs[1].error.as_deref(), Some("boom"));
        assert_eq!(backup.last_success, Some(true));
        assert_eq!(
            backup.last_started_at.as_deref(),
            Some("2026-06-02T00:00:00Z")
        );
        // A never-run job has an empty history.
        let never = statuses.iter().find(|s| s.name == "rba-fx-import").unwrap();
        assert!(never.runs.is_empty());
    }

    #[tokio::test]
    async fn migration_0012_preserves_the_old_single_row_records() {
        // 0012 rebuilt job_runs from the one-row-per-job upsert shape into the
        // append-per-run history shape. Each job's previously stored last run
        // must survive as its first history row — recreate the old shape,
        // apply the migration file, and check nothing was dropped.
        use sqlx::Connection;
        let mut conn = sqlx::SqliteConnection::connect(":memory:").await.unwrap();
        sqlx::raw_sql(
            "CREATE TABLE job_runs (
                 name        TEXT PRIMARY KEY,
                 started_at  TEXT    NOT NULL,
                 finished_at TEXT    NOT NULL,
                 success     INTEGER NOT NULL,
                 error       TEXT
             );
             INSERT INTO job_runs VALUES
                 ('backup', '2026-06-01T00:00:00Z', '2026-06-01T00:00:01Z', 0, 'boom'),
                 ('price-import', '2026-06-02T00:00:00Z', '2026-06-02T00:00:01Z', 1, NULL);",
        )
        .execute(&mut conn)
        .await
        .unwrap();

        sqlx::raw_sql(include_str!("../../migrations/0012_job_run_history.sql"))
            .execute(&mut conn)
            .await
            .unwrap();

        let rows: Vec<(String, String, bool, Option<String>)> =
            sqlx::query_as("SELECT name, started_at, success, error FROM job_runs ORDER BY name")
                .fetch_all(&mut conn)
                .await
                .unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(
            rows[0],
            (
                "backup".to_string(),
                "2026-06-01T00:00:00Z".to_string(),
                false,
                Some("boom".to_string())
            )
        );
        assert_eq!(rows[1].0, "price-import");
        assert!(rows[1].2);
    }
}
