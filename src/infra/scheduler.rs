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
//!
//! Split into focused submodules, all re-exported here so `scheduler::X` paths
//! are unchanged for callers:
//! - [`registry`] — what each job name does, and the job/lock types
//! - [`schedule`] — parsing `schedule.cron` and spawning a task per entry
//! - [`run`] — the logged, lock-serialised single run and the per-entry timer loop
//! - [`db`] — the bounded `job_runs` history
//! - [`http`] — `GET /jobs` and `POST /jobs/{name}`

mod db;
mod http;
mod registry;
mod run;
mod schedule;

pub use http::router;
pub use registry::{JobRegistry, registry};
pub use schedule::spawn;

// Reached only by the inline tests below; not part of the module's surface in
// the non-test build, so gated to keep that build warning-free.
#[cfg(test)]
pub use db::JOB_RUN_HISTORY_LIMIT;
#[cfg(test)]
use db::{db_record_run, db_run_histories};
#[cfg(test)]
pub use http::JobStatus;
#[cfg(test)]
pub use registry::{JobParams, RegisteredJob};
#[cfg(test)]
use run::{next_run, run_entry, run_job};
#[cfg(test)]
pub use schedule::ScheduleError;
#[cfg(test)]
use schedule::parse;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::infra::db;
    use crate::test_support::ApiClient;
    use axum::Extension;
    use axum::http::StatusCode;
    use chrono::{DateTime, Local, TimeZone, Utc};
    use chrono_tz::Tz;
    use croner::Cron;
    use sqlx::SqlitePool;
    use std::str::FromStr;
    use std::sync::Arc;
    use std::time::Duration;

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
        let job = RegisteredJob::from_fn(move |_| {
            let fired = fired.clone();
            let now = clock();
            async move {
                fired.lock().unwrap().push(now);
                Ok(())
            }
        });

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
        let job = RegisteredJob::from_fn(move |_| {
            let (a, o, r) = (a.clone(), o.clone(), r.clone());
            async move {
                if a.fetch_add(1, Ordering::SeqCst) > 0 {
                    o.store(true, Ordering::SeqCst);
                }
                tokio::time::sleep(Duration::from_millis(30)).await;
                a.fetch_sub(1, Ordering::SeqCst);
                r.fetch_add(1, Ordering::SeqCst);
                Ok(())
            }
        });

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
        let app = ApiClient::over(router().with_state(pool).layer(Extension(reg)));

        let resp = app.post_empty("/jobs/backup").await;

        assert_eq!(resp.status, StatusCode::NO_CONTENT);
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
        let app = ApiClient::over(router().with_state(pool).layer(Extension(reg)));

        let resp = app.post_empty("/jobs/backup?suffix=pre-0.5.1").await;

        assert_eq!(resp.status, StatusCode::NO_CONTENT);
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
        let app = ApiClient::over(router().with_state(pool.clone()).layer(Extension(reg)));

        let resp = app.post_empty("/jobs/backup?suffix=../etc/passwd").await;

        assert_eq!(resp.status, StatusCode::UNPROCESSABLE_ENTITY);
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
        let app = ApiClient::over(router().with_state(pool).layer(Extension(reg)));

        let resp = app.post_empty("/jobs/backup").await;

        assert_eq!(resp.status, StatusCode::NO_CONTENT);
        assert!(logs_contain("job started"));
        assert!(logs_contain("job finished"));
    }

    #[tokio::test]
    async fn trigger_unknown_job_returns_404() {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("t.db").to_string_lossy().to_string();
        let pool = db::init(&db_path).await.unwrap();
        let reg = registry(pool.clone(), db_path, None, None, stub_fetcher());
        let app = ApiClient::over(router().with_state(pool).layer(Extension(reg)));

        let resp = app.post_empty("/jobs/does-not-exist").await;

        assert_eq!(resp.status, StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn list_jobs_returns_registered_names() {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("t.db").to_string_lossy().to_string();
        let pool = db::init(&db_path).await.unwrap();
        let reg = registry(pool.clone(), db_path, None, None, stub_fetcher());
        let app = ApiClient::over(router().with_state(pool).layer(Extension(reg)));

        let resp = app.get("/jobs").await;

        assert_eq!(resp.status, StatusCode::OK);
        let statuses: Vec<JobStatus> = resp.json();
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

        let app = ApiClient::over(router().with_state(pool).layer(Extension(reg)));
        let resp = app.get("/jobs").await;
        assert_eq!(resp.status, StatusCode::OK);
        let statuses: Vec<JobStatus> = resp.json();
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

        let app = ApiClient::over(router().with_state(pool).layer(Extension(reg)));
        let resp = app.get("/jobs").await;
        assert_eq!(resp.status, StatusCode::OK);
        let statuses: Vec<JobStatus> = resp.json();
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

    /// SCENARIOS T-06: three registry entries recorded `format!("{e:?}")`, so
    /// what reached `job_runs.error` (and from there the Jobs table's Error
    /// column and the health banner) was Rust `Debug` syntax — `Fetch("…")` —
    /// with the enum's own `#[error]` wording thrown away. A job body reports
    /// its failure through `Display`; this pins the `Debug` form out of the
    /// registry the way `infra::decimal` pins stringified decimal binds out of
    /// the writes.
    #[test]
    fn no_registered_job_records_its_failure_as_a_debug_string() {
        // Assembled rather than written out, so this test and the doc comment
        // above it are not themselves matches.
        let debug_format = format!("{{{}:?}}", "e");
        let offenders: Vec<String> = include_str!("scheduler/registry.rs")
            .lines()
            .enumerate()
            .filter(|(_, line)| {
                !line.trim_start().starts_with("//") && line.contains(&debug_format)
            })
            .map(|(n, line)| format!("registry.rs:{}: {}", n + 1, line.trim()))
            .collect();
        assert!(
            offenders.is_empty(),
            "a job body must record its failure as the error's Display \
             (`e.to_string()`), which is its `#[error]` wording, not the \
             derived Debug:\n{}",
            offenders.join("\n")
        );
    }

    /// The other half of SCENARIOS T-06, end to end: what a failed job leaves
    /// in `job_runs.error` for the Jobs screen must name the failure *and* the
    /// cause under it. Driven with a real transport error — a refused loopback
    /// connection, no network — wrapped exactly as the `rba-fx-import` entry
    /// wraps the one it gets.
    #[tokio::test]
    async fn a_failed_job_records_its_message_and_its_cause() {
        let (reg, pool, _dir, _path) = test_registry().await;
        let transport = reqwest::get(crate::test_support::unreachable_url("f11-data.csv"))
            .await
            .expect_err("nothing is listening on that port");
        let import_error = crate::entities::rba_fx_rate::ImportError::Fetch(
            crate::infra::fetch::cause_chain(&transport),
        );
        let message = import_error.to_string();
        let failing = RegisteredJob::from_fn(move |_| {
            let message = message.clone();
            async move { Err(message) }
        });

        let returned = run_job(&pool, "rba-fx-import", &failing, JobParams::default())
            .await
            .expect_err("the job fails");

        let app = ApiClient::over(router().with_state(pool).layer(Extension(reg)));
        let statuses: Vec<JobStatus> = app.get("/jobs").await.json();
        let job = statuses
            .iter()
            .find(|s| s.name == "rba-fx-import")
            .expect("the job is registered");
        assert_eq!(job.last_success, Some(false));
        let recorded = job.last_error.clone().expect("a failed run records why");

        assert_eq!(
            recorded, returned,
            "job_runs.error is what the job returned"
        );
        assert!(
            recorded.starts_with("could not fetch the RBA FX rate feed: "),
            "the enum's own message is missing: {recorded}"
        );
        assert!(
            recorded.to_lowercase().contains("connect"),
            "the underlying cause is missing: {recorded}"
        );
        assert!(
            !recorded.starts_with("Fetch("),
            "recorded as a Rust Debug string: {recorded}"
        );
    }
}
