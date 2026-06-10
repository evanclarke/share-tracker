//! Cron-driven scheduler for recurring maintenance jobs.
//!
//! Job *schedules* live in a declarative cron file (5-field Vixie cron:
//! `min hour dom mon dow`) rather than in code — see `schedule.cron`. This
//! module owns a registry mapping a job name to the work it performs, parses a
//! schedule, and spawns one background task per scheduled entry. Jobs fire only
//! at their cron times (no run-on-startup); any job can be run on demand via
//! `POST /jobs/{name}`.
//!
//! Each spawned task logs the next scheduled run at INFO after every run (and at
//! startup), so the live schedule is verifiable from logs without reading code.

use axum::{
    Extension, Json, Router,
    extract::{Path, State},
    http::StatusCode,
    routing::{get, post},
};
use chrono::{DateTime, Local};
use croner::Cron;
use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;
use std::{collections::HashMap, future::Future, pin::Pin, str::FromStr, sync::Arc, time::Duration};

/// A unit of scheduled work. Each call runs the job once, returning `Ok(())` on
/// success or a human-readable error. Jobs do their own detailed INFO logging.
type JobFuture = Pin<Box<dyn Future<Output = Result<(), String>> + Send>>;
type Job = Arc<dyn Fn() -> JobFuture + Send + Sync>;

/// Job-name → work. Shared between the spawned schedule tasks and the HTTP
/// trigger handler (injected as an axum `Extension`).
pub type JobRegistry = Arc<HashMap<String, Job>>;

/// A job's last run, surfaced by `GET /jobs` and the Jobs UI. Every registered
/// job appears in the list; the `last_*` fields are `None` until the job has run
/// at least once (no `job_runs` row yet).
#[derive(Debug, Serialize, Deserialize)]
pub struct JobStatus {
    pub name: String,
    pub last_started_at: Option<String>,
    pub last_finished_at: Option<String>,
    pub last_success: Option<bool>,
    pub last_error: Option<String>,
}

/// One persisted run record from `job_runs` (the last run of a single job).
#[derive(sqlx::FromRow)]
struct JobRun {
    name: String,
    started_at: String,
    finished_at: String,
    success: bool,
    error: Option<String>,
}

/// Upsert the last-run record for `name`. One row per job (keyed by name), so a
/// new run overwrites the previous record rather than accumulating history.
async fn db_record_run(
    pool: &SqlitePool,
    name: &str,
    started_at: &str,
    finished_at: &str,
    success: bool,
    error: Option<&str>,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO job_runs (name, started_at, finished_at, success, error) \
         VALUES (?, ?, ?, ?, ?) \
         ON CONFLICT(name) DO UPDATE SET \
             started_at = excluded.started_at, \
             finished_at = excluded.finished_at, \
             success = excluded.success, \
             error = excluded.error",
    )
    .bind(name)
    .bind(started_at)
    .bind(finished_at)
    .bind(success)
    .bind(error)
    .execute(pool)
    .await?;
    Ok(())
}

/// Load every job's last run, keyed by job name.
async fn db_last_runs(pool: &SqlitePool) -> Result<HashMap<String, JobRun>, sqlx::Error> {
    let rows = sqlx::query_as::<_, JobRun>(
        "SELECT name, started_at, finished_at, success, error FROM job_runs",
    )
    .fetch_all(pool)
    .await?;
    Ok(rows.into_iter().map(|r| (r.name.clone(), r)).collect())
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
    fetcher: crate::entities::closing_price::SharedFetcher,
) -> JobRegistry {
    let mut jobs: HashMap<String, Job> = HashMap::new();

    let backup_pool = pool.clone();
    jobs.insert(
        "backup".to_string(),
        Arc::new(move || {
            let pool = backup_pool.clone();
            let db_path = db_path.clone();
            Box::pin(async move {
                crate::infra::db::backup(&pool, &db_path).await.map_err(|e| e.to_string())
            })
        }),
    );

    let mic_pool = pool.clone();
    jobs.insert(
        "mic-import".to_string(),
        Arc::new(move || {
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
        }),
    );

    let currency_pool = pool.clone();
    jobs.insert(
        "currency-import".to_string(),
        Arc::new(move || {
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
        }),
    );

    let price_pool = pool.clone();
    let price_fetcher = fetcher;
    jobs.insert(
        "price-import".to_string(),
        Arc::new(move || {
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
        }),
    );

    let snapshot_pool = pool.clone();
    jobs.insert(
        "report-snapshot".to_string(),
        Arc::new(move || {
            let pool = snapshot_pool.clone();
            Box::pin(async move {
                crate::reports::snapshot::run_snapshot_job(&pool, chrono::Utc::now()).await
            })
        }),
    );

    let fx_pool = pool.clone();
    jobs.insert(
        "rba-fx-import".to_string(),
        Arc::new(move || {
            let pool = fx_pool.clone();
            Box::pin(async move {
                match crate::entities::rba_fx_rate::run_import(&pool).await {
                    Ok(s) => {
                        tracing::info!(
                            inserted = s.inserted,
                            skipped = s.skipped,
                            "RBA FX rate import complete"
                        );
                        Ok(())
                    }
                    Err(e) => Err(format!("{e:?}")),
                }
            })
        }),
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
async fn run_job(pool: &SqlitePool, name: &str, job: &Job) -> Result<(), String> {
    let started_at = chrono::Utc::now().to_rfc3339();
    tracing::info!(job = %name, "job started");
    let result = job().await;
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

/// Parse a cron schedule file into `(cron, job_name)` entries. Lines are
/// `<min> <hour> <dom> <mon> <dow> <job-name>`; `#` starts a comment and blank
/// lines are ignored.
fn parse(schedule: &str) -> Result<Vec<(Cron, String)>, ScheduleError> {
    let mut entries = Vec::new();

    for (idx, raw) in schedule.lines().enumerate() {
        let line = idx + 1;
        let content = raw.split('#').next().unwrap_or("").trim();
        if content.is_empty() {
            continue;
        }

        let fields: Vec<&str> = content.split_whitespace().collect();
        if fields.len() < 6 {
            return Err(ScheduleError::Parse {
                line,
                msg: format!(
                    "expected 5 cron fields followed by a job name, got {} field(s)",
                    fields.len()
                ),
            });
        }

        let expr = fields[..5].join(" ");
        let name = fields[5].to_string();
        let cron = Cron::from_str(&expr)
            .map_err(|e| ScheduleError::Parse { line, msg: format!("invalid cron {expr:?}: {e}") })?;
        entries.push((cron, name));
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
    for (idx, (_, name)) in entries.iter().enumerate() {
        if !registry.contains_key(name) {
            return Err(ScheduleError::UnknownJob { line: idx + 1, name: name.clone() });
        }
    }

    // A registered job with no schedule line never runs automatically (only via
    // POST /jobs/{name}). That is usually an oversight, so warn rather than fail.
    for name in registry.keys() {
        if !entries.iter().any(|(_, scheduled)| scheduled == name) {
            tracing::warn!(
                job = %name,
                "registered job has no schedule entry; it will only run via POST /jobs/{name}"
            );
        }
    }

    for (cron, name) in entries {
        let job = registry[&name].clone();
        let pool = pool.clone();
        tokio::spawn(async move {
            loop {
                let (next, delay) = match next_run(&cron, Local::now()) {
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
                tokio::time::sleep(delay).await;
                if let Err(e) = run_job(&pool, &name, &job).await {
                    tracing::warn!(job = %name, "job failed: {e}");
                }
            }
        });
    }

    Ok(())
}

/// Compute the next scheduled fire time at or after `now` and the exact delay to
/// sleep until then. The delay keeps sub-second precision: truncating it (e.g.
/// via `num_seconds`) would make the timer wake *before* the target second, so
/// the recomputed next run would still be the same instant and the loop would
/// busy-spin until the clock crossed the boundary. Returns `None` only if the
/// cron pattern has no future occurrence.
fn next_run(cron: &Cron, now: DateTime<Local>) -> Option<(DateTime<Local>, Duration)> {
    let next = cron.find_next_occurrence(&now, false).ok()?;
    // `next` is strictly after `now`, so the difference is non-negative; the
    // fallback only guards against clock skew between this read and the diff.
    let delay = (next - now).to_std().unwrap_or(Duration::from_secs(1));
    Some((next, delay))
}

/// List every registered job (sorted) with its last run. Jobs that have never
/// run carry `None` in the `last_*` fields.
async fn list(
    State(pool): State<SqlitePool>,
    Extension(registry): Extension<JobRegistry>,
) -> Result<Json<Vec<JobStatus>>, crate::infra::http::ApiError> {
    let mut last = db_last_runs(&pool).await?;

    let mut names: Vec<String> = registry.keys().cloned().collect();
    names.sort();
    let statuses = names
        .into_iter()
        .map(|name| match last.remove(&name) {
            Some(run) => JobStatus {
                name,
                last_started_at: Some(run.started_at),
                last_finished_at: Some(run.finished_at),
                last_success: Some(run.success),
                last_error: run.error,
            },
            None => JobStatus {
                name,
                last_started_at: None,
                last_finished_at: None,
                last_success: None,
                last_error: None,
            },
        })
        .collect();
    Ok(Json(statuses))
}

async fn trigger(
    State(pool): State<SqlitePool>,
    Extension(registry): Extension<JobRegistry>,
    Path(name): Path<String>,
) -> StatusCode {
    match registry.get(&name) {
        None => StatusCode::NOT_FOUND,
        Some(job) => match run_job(&pool, &name, job).await {
            Ok(()) => StatusCode::NO_CONTENT,
            Err(e) => {
                tracing::warn!(job = %name, "manual job trigger failed: {e}");
                StatusCode::INTERNAL_SERVER_ERROR
            }
        },
    }
}

/// Routes for inspecting and manually triggering jobs. The `JobRegistry` is
/// supplied to handlers via an `Extension` layer (added in `main`), so this
/// router shares the common `SqlitePool` state and merges with the others.
pub fn router() -> Router<SqlitePool> {
    Router::new().route("/jobs", get(list)).route("/jobs/{name}", post(trigger))
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
        (registry(pool.clone(), db_path.clone(), stub_fetcher()), pool, dir, db_path)
    }

    #[test]
    fn parse_ignores_comments_and_blank_lines() {
        let schedule = "# a comment\n\n0 0 * * *   backup   # trailing comment\n";
        let entries = parse(schedule).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].1, "backup");
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
            assert_eq!(now + slept, next, "delay must land exactly on next, not before");
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
        let (cron, _) =
            entries.iter().find(|(_, name)| name == "backup").expect("backup must be scheduled");
        let from = Local.with_ymd_and_hms(2026, 6, 1, 12, 0, 0).unwrap();
        let first = cron.find_next_occurrence(&from, false).unwrap();
        let second = cron.find_next_occurrence(&first, false).unwrap();
        assert_eq!(second - first, chrono::Duration::days(7));
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
    async fn trigger_backup_runs_and_returns_204() {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("t.db").to_string_lossy().to_string();
        let pool = db::init(&db_path).await.unwrap();
        let reg = registry(pool.clone(), db_path.clone(), stub_fetcher());
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
        assert!(made_backup, "expected a timestamped backup file beside t.db");
    }

    #[tracing_test::traced_test]
    #[tokio::test]
    async fn run_job_logs_started_and_finished() {
        // The scheduled loop runs each job via run_job, so this covers the
        // scheduled path: a job must be bracketed by INFO start/finish lines.
        let (reg, pool, _dir, _path) = test_registry().await;
        let job = reg.get("backup").unwrap();
        run_job(&pool, "backup", job).await.unwrap();
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
        let reg = registry(pool.clone(), db_path, stub_fetcher());
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
        let reg = registry(pool.clone(), db_path, stub_fetcher());
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
        let reg = registry(pool.clone(), db_path, stub_fetcher());
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
        run_job(&pool, "backup", job).await.unwrap();

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
    async fn record_run_persists_failure_with_error() {
        // A failed run stores success = 0 and the error text; a later success for
        // the same job overwrites it (one row per job).
        let (_reg, pool, _dir, _path) = test_registry().await;
        db_record_run(&pool, "backup", "2026-06-01T00:00:00Z", "2026-06-01T00:00:01Z", false, Some("boom"))
            .await
            .unwrap();
        let runs = db_last_runs(&pool).await.unwrap();
        let run = runs.get("backup").unwrap();
        assert!(!run.success);
        assert_eq!(run.error.as_deref(), Some("boom"));

        db_record_run(&pool, "backup", "2026-06-02T00:00:00Z", "2026-06-02T00:00:01Z", true, None)
            .await
            .unwrap();
        let runs = db_last_runs(&pool).await.unwrap();
        let run = runs.get("backup").unwrap();
        assert!(run.success);
        assert!(run.error.is_none());
        assert_eq!(run.started_at, "2026-06-02T00:00:00Z");
    }
}
