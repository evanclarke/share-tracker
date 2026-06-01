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

use axum::{Extension, Json, Router, extract::Path, http::StatusCode, routing::{get, post}};
use chrono::{DateTime, Local};
use croner::Cron;
use sqlx::SqlitePool;
use std::{collections::HashMap, future::Future, pin::Pin, str::FromStr, sync::Arc, time::Duration};

/// A unit of scheduled work. Each call runs the job once, returning `Ok(())` on
/// success or a human-readable error. Jobs do their own detailed INFO logging.
type JobFuture = Pin<Box<dyn Future<Output = Result<(), String>> + Send>>;
type Job = Arc<dyn Fn() -> JobFuture + Send + Sync>;

/// Job-name → work. Shared between the spawned schedule tasks and the HTTP
/// trigger handler (injected as an axum `Extension`).
pub type JobRegistry = Arc<HashMap<String, Job>>;

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
/// schedule file.
pub fn registry(pool: SqlitePool, db_path: String) -> JobRegistry {
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
/// logs start and finish uniformly, regardless of any per-job logging it does.
async fn run_job(name: &str, job: &Job) -> Result<(), String> {
    tracing::info!(job = %name, "job started");
    let result = job().await;
    tracing::info!(job = %name, ok = result.is_ok(), "job finished");
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
pub fn spawn(registry: JobRegistry, schedule: &str) -> Result<(), ScheduleError> {
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
                if let Err(e) = run_job(&name, &job).await {
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

async fn list(Extension(registry): Extension<JobRegistry>) -> Json<Vec<String>> {
    let mut names: Vec<String> = registry.keys().cloned().collect();
    names.sort();
    Json(names)
}

async fn trigger(
    Extension(registry): Extension<JobRegistry>,
    Path(name): Path<String>,
) -> StatusCode {
    match registry.get(&name) {
        None => StatusCode::NOT_FOUND,
        Some(job) => match run_job(&name, job).await {
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

    async fn test_registry() -> (JobRegistry, tempfile::TempDir, String) {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("test.db").to_string_lossy().to_string();
        let pool = db::init(&db_path).await.unwrap();
        (registry(pool, db_path.clone()), dir, db_path)
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
        let (reg, _dir, _path) = test_registry().await;
        // Guards the committed schedule.cron: every referenced job must exist.
        spawn(reg, include_str!("../../schedule.cron")).unwrap();
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
        let (reg, _dir, _path) = test_registry().await;
        spawn(reg, "0 0 * * *   backup\n").unwrap();
        assert!(logs_contain("registered job has no schedule entry"));
        assert!(logs_contain("rba-fx-import"));
    }

    #[tokio::test]
    async fn spawn_rejects_unknown_job() {
        let (reg, _dir, _path) = test_registry().await;
        let err = spawn(reg, "0 0 * * *   no-such-job\n").unwrap_err();
        assert!(matches!(err, ScheduleError::UnknownJob { .. }));
    }

    #[tokio::test]
    async fn trigger_backup_runs_and_returns_204() {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("t.db").to_string_lossy().to_string();
        let pool = db::init(&db_path).await.unwrap();
        let reg = registry(pool.clone(), db_path.clone());
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
        let (reg, _dir, _path) = test_registry().await;
        let job = reg.get("backup").unwrap();
        run_job("backup", job).await.unwrap();
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
        let reg = registry(pool.clone(), db_path);
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
        let reg = registry(pool.clone(), db_path);
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
        let reg = registry(pool.clone(), db_path);
        let app = router().with_state(pool).layer(Extension(reg));

        let resp = app
            .oneshot(Request::builder().uri("/jobs").body(Body::empty()).unwrap())
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::OK);
        let body = resp.into_body().collect().await.unwrap().to_bytes();
        let names: Vec<String> = serde_json::from_slice(&body).unwrap();
        assert!(names.contains(&"backup".to_string()));
        assert!(names.contains(&"rba-fx-import".to_string()));
    }
}
