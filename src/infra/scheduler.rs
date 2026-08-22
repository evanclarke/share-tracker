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
//! - [`db`] — the bounded `job_runs` history and the stored `job_schedule`
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
pub use db::JobRunStatus;
#[cfg(test)]
use db::{db_record_run, db_run_histories, db_start_run};
#[cfg(test)]
pub use http::JobStatus;
#[cfg(test)]
pub use registry::{JobParams, JobTrigger, RegisteredJob};
#[cfg(test)]
use run::{next_run, run_entry, run_job};
#[cfg(test)]
pub use schedule::ScheduleError;
#[cfg(test)]
use schedule::{ScheduleEntry, parse};

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
    use std::collections::HashMap;
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
        spawn(reg, pool, include_str!("../../schedule.cron"))
            .await
            .unwrap();
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
        //
        // …and closed again before the loop starts, which this test needs and
        // nothing else does. `run_job` records the run's *start* before calling
        // the job (SCENARIOS T-11), and awaiting that write parks the runtime
        // on sqlx's sqlite worker thread; with the clock paused, tokio treats an
        // idle runtime as licence to jump to the next timer — sqlx's own 600s
        // pool maintenance tick — so the fake wall clock advanced ten minutes
        // between the timer firing and the job body reading it. A closed pool
        // fails that write without awaiting anything (`run_job` logs it and
        // carries on), which is fine here: what is under test is *when the timer
        // fires*, not what it records.
        let pool = db::init(":memory:").await.unwrap();
        pool.close().await;
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
        let job = RegisteredJob::from_fn(JobTrigger::Scheduled, move |_| {
            let fired = fired.clone();
            let now = clock();
            async move {
                fired.lock().unwrap().push(now);
                Ok(None)
            }
        });

        let cron = Cron::from_str("30 3 1 6 *").unwrap(); // 03:30 on 2026-06-01
        let entry = ScheduleEntry {
            line: 1,
            cron,
            expr: "30 3 1 6 *".to_string(),
            tz: None,
            name: "fake".to_string(),
        };
        tokio::spawn(run_entry(pool, entry, job, clock));
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
        spawn(reg, pool, "0 0 * * *   Pacific/Auckland   backup\n")
            .await
            .unwrap();
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
        spawn(reg, pool, "0 0 * * *   backup\n").await.unwrap();
        assert!(logs_contain("registered job has no schedule entry"));
        assert!(logs_contain("rba-fx-import"));
        // …and only about the jobs that expect a schedule: the two one-off
        // repairs registered with `register_manual` are deliberately
        // schedule-less, so a lost line is the only thing this WARN can mean
        // (SCENARIOS T-09/schedule).
        assert!(
            !logs_contain("price-rebase"),
            "a manual-only job must not be warned about"
        );
        assert!(
            !logs_contain("settlement-recompute"),
            "a manual-only job must not be warned about"
        );
    }

    #[tracing_test::traced_test]
    #[tokio::test]
    async fn committed_schedule_starts_without_a_single_missing_entry_warning() {
        // Startup on the shipped schedule must log *no* "no schedule entry"
        // WARN at all. Before the manual-only flag it logged two, every boot,
        // that nobody could ever clear — so a genuinely dropped schedule line
        // logged the identical line and was invisible (SCENARIOS T-09/schedule).
        let (reg, pool, _dir, _path) = test_registry().await;
        spawn(reg, pool, include_str!("../../schedule.cron"))
            .await
            .unwrap();
        assert!(!logs_contain("registered job has no schedule entry"));
    }

    #[tokio::test]
    async fn every_registered_job_is_scheduled_or_deliberately_manual() {
        // The registry's own record of intent must match the committed
        // schedule, in both directions: a `Scheduled` job has at least one
        // line, a `ManualOnly` job has none. This is what keeps the flag
        // honest — a new job added without a schedule line has to say which it
        // is rather than quietly reintroducing the permanent WARN.
        let (reg, _pool, _dir, _path) = test_registry().await;
        let entries = parse(include_str!("../../schedule.cron")).unwrap();
        let mut manual: Vec<&str> = Vec::new();
        for (name, job) in reg.iter() {
            let scheduled = entries.iter().any(|e| &e.name == name);
            match job.trigger {
                JobTrigger::Scheduled => assert!(
                    scheduled,
                    "{name} is registered as scheduled but has no schedule.cron line"
                ),
                JobTrigger::ManualOnly => {
                    assert!(
                        !scheduled,
                        "{name} is registered as manual-only but has a schedule.cron line"
                    );
                    manual.push(name);
                }
            }
        }
        manual.sort_unstable();
        assert_eq!(manual, ["price-rebase", "settlement-recompute"]);
    }

    /// Every stored `job_schedule` row: name, cron expression as written,
    /// timezone and next-run instant.
    async fn stored_schedule(pool: &SqlitePool) -> Vec<(String, String, Option<String>, String)> {
        sqlx::query_as("SELECT name, cron, timezone, next_run_at FROM job_schedule ORDER BY name")
            .fetch_all(pool)
            .await
            .unwrap()
    }

    /// Wait (briefly) for the spawned entry tasks to have claimed `expected`
    /// rows. The write goes through sqlx's worker thread, so yielding alone
    /// does not get there; a short poll does, and fails loudly rather than
    /// racing.
    async fn wait_for_schedule(
        pool: &SqlitePool,
        expected: usize,
    ) -> Vec<(String, String, Option<String>, String)> {
        for _ in 0..300 {
            let rows = stored_schedule(pool).await;
            if rows.len() >= expected {
                return rows;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        panic!("the spawned schedule entries never stored {expected} row(s)");
    }

    #[tokio::test]
    async fn a_spawned_entry_stores_when_it_is_next_due() {
        // The scheduler already computes this instant every iteration and logs
        // it; storing it is what lets a report that reads only the database say
        // whether a job is still scheduled and when it is due
        // (SCENARIOS T-11/T-02/T-12).
        let (reg, pool, _dir, _path) = test_registry().await;
        spawn(
            reg.clone(),
            pool.clone(),
            "0 0 * * 0   Pacific/Auckland   backup\n",
        )
        .await
        .unwrap();
        let rows = wait_for_schedule(&pool, 1).await;

        assert_eq!(rows.len(), 1, "one row per schedule entry");
        let (name, cron, tz, next_run_at) = &rows[0];
        assert_eq!(name, "backup");
        assert_eq!(cron, "0 0 * * 0", "the expression as the file wrote it");
        assert_eq!(tz.as_deref(), Some("Pacific/Auckland"));
        let due = chrono::DateTime::parse_from_rfc3339(next_run_at).unwrap();
        assert!(due > Utc::now(), "a live entry stores a future instant");

        // …and `GET /jobs` carries it, which is what the Jobs screen's "next
        // run" column reads. A manual-only job has no schedule by design, so it
        // has no row and no instant — never an overdue one.
        let app = ApiClient::over(router().with_state(pool).layer(Extension(reg)));
        let statuses: Vec<JobStatus> = app.get("/jobs").await.json();
        let job = |name: &str| statuses.iter().find(|s| s.name == name).unwrap();
        assert_eq!(job("backup").next_run_at.as_deref(), Some(&**next_run_at));
        assert!(job("price-rebase").next_run_at.is_none());
        // A *scheduled* job with no line of its own has none either — the
        // startup WARN is what reports that, not a stored instant.
        assert!(job("rba-fx-import").next_run_at.is_none());
    }

    #[tokio::test]
    async fn a_schedule_with_no_future_occurrence_is_reported_overdue() {
        // The finding, driven: `0 0 30 2 *` — 30 February — is accepted at
        // startup, the entry's task logs one ERROR and exits, and nothing ran
        // the backup again for the life of the process while `GET /jobs` went
        // on answering `ok`. The row claimed before the first occurrence is
        // computed is what ages instead.
        let (reg, pool, _dir, _path) = test_registry().await;
        spawn(reg, pool.clone(), "0 0 30 2 *   backup\n")
            .await
            .unwrap();
        let rows = wait_for_schedule(&pool, 1).await;
        assert_eq!(rows[0].0, "backup");
        assert_eq!(rows[0].1, "0 0 30 2 *");

        // Silent within the grace margin…
        let now = Utc::now();
        let health = crate::reports::health::db_health(&pool, now.date_naive(), now)
            .await
            .unwrap();
        assert!(health.overdue_jobs.is_empty());

        // …and past it, the stopped scheduler is named.
        let later = now
            + chrono::Duration::hours(crate::reports::health::JOB_OVERDUE_GRACE_HOURS)
            + chrono::Duration::minutes(1);
        let health = crate::reports::health::db_health(&pool, later.date_naive(), later)
            .await
            .unwrap();
        assert_eq!(health.overdue_jobs.len(), 1);
        assert_eq!(health.overdue_jobs[0].name, "backup");
        assert_eq!(health.overdue_jobs[0].cron, "0 0 30 2 *");
        // The run history says nothing at all: a job that never ran recorded
        // nothing to fail, which is the whole reason this check exists.
        assert!(health.failed_jobs.is_empty());
    }

    #[tokio::test]
    async fn spawn_forgets_a_schedule_entry_that_has_been_removed() {
        // A row left by a previous process for a line that has since been
        // deleted must not be reported overdue for ever after: the table is the
        // schedule *this* process is running, so startup clears it and the
        // spawned entries rebuild it. A lost line is the startup WARN's
        // business (SCENARIOS T-09/schedule), not a permanent alarm nobody can
        // clear.
        let (reg, pool, _dir, _path) = test_registry().await;
        sqlx::query(
            "INSERT INTO job_schedule (name, cron, timezone, next_run_at, updated_at) \
             VALUES ('backup', '0 0 * * 0', NULL, '2020-01-01T00:00:00+00:00', \
                     '2020-01-01T00:00:00+00:00')",
        )
        .execute(&pool)
        .await
        .unwrap();

        spawn(reg, pool.clone(), "0 2 * * 1   rba-fx-import\n")
            .await
            .unwrap();
        let rows = wait_for_schedule(&pool, 1).await;
        assert_eq!(
            rows.iter().map(|r| r.0.as_str()).collect::<Vec<_>>(),
            ["rba-fx-import"],
            "only the entries this process is running are stored"
        );

        let now = Utc::now();
        let health = crate::reports::health::db_health(&pool, now.date_naive(), now)
            .await
            .unwrap();
        assert!(health.overdue_jobs.is_empty());
    }

    #[tokio::test]
    async fn list_jobs_reports_how_each_job_is_triggered() {
        // `GET /jobs` carries the registry's intent, so the Jobs screen can say
        // "manual only" for a job that has no schedule at all — its `never`
        // last-run status is expected, not a missed run, and a later overdue
        // check has the one field it needs to leave such a job alone.
        let (reg, pool, _dir, _path) = test_registry().await;
        let app = ApiClient::over(router().with_state(pool).layer(Extension(reg)));

        let statuses: Vec<JobStatus> = app.get("/jobs").await.json();

        let trigger = |name: &str| {
            statuses
                .iter()
                .find(|s| s.name == name)
                .unwrap_or_else(|| panic!("{name} must be listed"))
                .trigger
        };
        assert_eq!(trigger("price-rebase"), JobTrigger::ManualOnly);
        assert_eq!(trigger("settlement-recompute"), JobTrigger::ManualOnly);
        assert_eq!(trigger("backup"), JobTrigger::Scheduled);
        assert_eq!(trigger("rba-fx-import"), JobTrigger::Scheduled);
        // A manual-only job has never run and never will run on a timer: the
        // flag is what says so, not an inference from the empty history.
        let rebase = statuses.iter().find(|s| s.name == "price-rebase").unwrap();
        assert!(rebase.last_started_at.is_none());
        assert!(rebase.runs.is_empty());
    }

    #[tokio::test]
    async fn manual_only_flag_is_serialised_as_snake_case() {
        // The wire value the Jobs screen matches on ('manual_only'), pinned so
        // renaming the Rust variant cannot silently change the JSON.
        let (reg, pool, _dir, _path) = test_registry().await;
        let app = ApiClient::over(router().with_state(pool).layer(Extension(reg)));

        let body: serde_json::Value = app.get("/jobs").await.json();

        let job = |name: &str| {
            body.as_array()
                .unwrap()
                .iter()
                .find(|j| j["name"] == name)
                .unwrap()
                .clone()
        };
        assert_eq!(job("price-rebase")["trigger"], "manual_only");
        assert_eq!(job("backup")["trigger"], "scheduled");
    }

    #[tokio::test]
    async fn spawn_rejects_unknown_job() {
        let (reg, pool, _dir, _path) = test_registry().await;
        let err = spawn(reg, pool, "0 0 * * *   no-such-job\n")
            .await
            .unwrap_err();
        assert!(matches!(err, ScheduleError::UnknownJob { .. }));
    }

    #[tokio::test]
    async fn unknown_job_error_reports_file_line_not_entry_index() {
        // Comments and blank lines shift parsed-entry indexes away from file
        // lines: `no-such-job` is the 2nd parsed entry but sits on file line 5.
        // The error must point at line 5, where the user will look.
        let (reg, pool, _dir, _path) = test_registry().await;
        let schedule = "# weekly maintenance\n\n0 0 * * 0   backup\n# bad line below\n0 1 * * *   no-such-job\n";
        let err = spawn(reg, pool, schedule).await.unwrap_err();
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
        let job = RegisteredJob::from_fn(JobTrigger::Scheduled, move |_| {
            let (a, o, r) = (a.clone(), o.clone(), r.clone());
            async move {
                if a.fetch_add(1, Ordering::SeqCst) > 0 {
                    o.store(true, Ordering::SeqCst);
                }
                tokio::time::sleep(Duration::from_millis(30)).await;
                a.fetch_sub(1, Ordering::SeqCst);
                r.fetch_add(1, Ordering::SeqCst);
                Ok(None)
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
    async fn trigger_unknown_job_404_names_the_job_and_the_registered_names() {
        // SCENARIOS T-10: a bare 404 reaches the Jobs screen as the toast
        // "HTTP 404". The body names what was asked for and what exists, the
        // same contract every entity DELETE keeps (`deleted(found, noun)`).
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("t.db").to_string_lossy().to_string();
        let pool = db::init(&db_path).await.unwrap();
        let reg = registry(pool.clone(), db_path, None, None, stub_fetcher());
        let app = ApiClient::over(router().with_state(pool).layer(Extension(reg)));

        let resp = app.post_empty("/jobs/does-not-exist").await;

        assert_eq!(resp.status, StatusCode::NOT_FOUND);
        let body = resp.text();
        assert!(
            body.contains("no job named 'does-not-exist'"),
            "the 404 must name the job asked for: {body}"
        );
        for name in ["backup", "rba-fx-import", "settlement-recompute"] {
            assert!(
                body.contains(name),
                "the 404 must list the registered names, missing {name}: {body}"
            );
        }
    }

    /// SCENARIOS T-10: a failed run answered `500` with an empty body, so the
    /// toast the operator reads first said "HTTP 500" while `run_job` had just
    /// handed back the reason. The body now carries exactly what `job_runs.error`
    /// records, and the reason still reaches the log.
    #[tracing_test::traced_test]
    #[tokio::test]
    async fn a_failing_job_answers_500_carrying_its_reason() {
        let pool = db::init(":memory:").await.unwrap();
        let mut jobs: HashMap<String, Arc<RegisteredJob>> = HashMap::new();
        jobs.insert(
            "always-fails".to_string(),
            RegisteredJob::from_fn(JobTrigger::ManualOnly, |_| async {
                Err("could not fetch the RBA FX rate feed: connection refused".to_string())
            }),
        );
        let reg: JobRegistry = Arc::new(jobs);
        let app = ApiClient::over(router().with_state(pool.clone()).layer(Extension(reg)));

        let resp = app.post_empty("/jobs/always-fails").await;

        assert_eq!(resp.status, StatusCode::INTERNAL_SERVER_ERROR);
        let body = resp.text();
        assert_eq!(
            body, "could not fetch the RBA FX rate feed: connection refused",
            "the 500 must carry the job's own error text"
        );
        // The same text the Jobs table's Error column shows, so the toast and
        // the row agree.
        let recorded: (Option<String>,) =
            sqlx::query_as("SELECT error FROM job_runs WHERE name = 'always-fails'")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(recorded.0.as_deref(), Some(body));
        // Returning the reason must not stop it being logged.
        assert!(logs_contain("manual job trigger failed"));
        assert!(logs_contain("connection refused"));
    }

    #[tokio::test]
    async fn trigger_with_a_misspelt_query_parameter_is_refused_not_ignored() {
        // SCENARIOS T-10: `?sufix=` used to answer 204 and take an *unlabelled*
        // backup — the operator's one-off label silently lost. It is now a 422
        // with a reason, rejected before any run is recorded.
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("t.db").to_string_lossy().to_string();
        let pool = db::init(&db_path).await.unwrap();
        let reg = registry(pool.clone(), db_path, None, None, stub_fetcher());
        let app = ApiClient::over(router().with_state(pool.clone()).layer(Extension(reg)));

        let resp = app.post_empty("/jobs/backup?sufix=pre-0.5.1").await;

        assert_eq!(resp.status, StatusCode::UNPROCESSABLE_ENTITY);
        let body = resp.text();
        assert!(
            body.contains("sufix") && body.contains("suffix"),
            "the 422 must name the parameter it did not understand: {body}"
        );
        // Nothing ran: no backup file, no recorded run.
        let no_backup = std::fs::read_dir(dir.path())
            .unwrap()
            .filter_map(|e| e.ok())
            .all(|e| {
                let name = e.file_name().to_string_lossy().into_owned();
                name == "t.db" || name.starts_with("t.db-")
            });
        assert!(
            no_backup,
            "a rejected query string must not take a backup at all"
        );
        let runs: Vec<(String,)> = sqlx::query_as("SELECT name FROM job_runs")
            .fetch_all(&pool)
            .await
            .unwrap();
        assert!(runs.is_empty(), "a rejected request records no run");
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
        assert!(backup.last_status.is_none());
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
        assert_eq!(backup.last_status, Some(JobRunStatus::Ok));
        assert!(backup.last_error.is_none());
    }

    /// SCENARIOS T-09: a run that succeeded while doing less than the whole of
    /// its work stays a **success** — the currency import's ISO 24165 half is
    /// credential-gated and optional by design — but the Jobs surface no longer
    /// reads as complete: the run's own note rides out on `GET /jobs` beside an
    /// `ok` status and an empty error, which is what the Jobs screen shows.
    #[tokio::test]
    async fn a_run_that_did_only_half_its_work_stays_ok_and_says_what_it_skipped() {
        let (reg, pool, _dir, _path) = test_registry().await;
        let note = "imported 178 ISO 4217 fiat currencies; ISO 24165 digital token feed skipped";
        let half = RegisteredJob::from_fn(JobTrigger::Scheduled, move |_| async move {
            Ok(Some(note.to_string()))
        });
        // The run itself succeeds: POST /jobs/{name} would answer 204.
        run_job(&pool, "currency-import", &half, JobParams::default())
            .await
            .expect("a skipped optional feed is not a failure");

        let app = ApiClient::over(router().with_state(pool).layer(Extension(reg)));
        let statuses: Vec<JobStatus> = app.get("/jobs").await.json();
        let job = statuses
            .iter()
            .find(|s| s.name == "currency-import")
            .unwrap();
        assert_eq!(job.last_status, Some(JobRunStatus::Ok), "still a success");
        assert!(job.last_error.is_none(), "and not an error");
        assert_eq!(
            job.last_note.as_deref(),
            Some(note),
            "but no longer reading as a complete run"
        );
        // The note is on the stored run too, so the expanded history shows it.
        assert_eq!(job.runs[0].note.as_deref(), Some(note));
    }

    /// The other side: an ordinary complete run carries no note at all, so the
    /// column stays empty except where something really was passed over.
    #[tokio::test]
    async fn a_complete_run_records_no_note() {
        let (reg, pool, _dir, _path) = test_registry().await;
        let job = reg.get("backup").unwrap();
        run_job(&pool, "backup", job, JobParams::default())
            .await
            .unwrap();

        let app = ApiClient::over(router().with_state(pool).layer(Extension(reg)));
        let statuses: Vec<JobStatus> = app.get("/jobs").await.json();
        let backup = statuses.iter().find(|s| s.name == "backup").unwrap();
        assert!(backup.last_note.is_none());
        assert!(backup.runs[0].note.is_none());
    }

    #[tokio::test]
    async fn record_run_keeps_history_latest_first() {
        // A failed run stores status 'failed' and the error text; a later success
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
            None,
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
            None,
        )
        .await
        .unwrap();

        let histories = db_run_histories(&pool).await.unwrap();
        let runs = histories.get("backup").unwrap();
        assert_eq!(runs.len(), 2);
        assert_eq!(runs[0].status, JobRunStatus::Ok);
        assert!(runs[0].error.is_none());
        assert_eq!(runs[0].started_at, "2026-06-02T00:00:00Z");
        assert_eq!(runs[1].status, JobRunStatus::Failed);
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
            None,
        )
        .await
        .unwrap();
        let extra = 5;
        for i in 0..(JOB_RUN_HISTORY_LIMIT + extra) {
            let started = format!("2026-06-01T00:{i:02}:00Z");
            let finished = format!("2026-06-01T00:{i:02}:01Z");
            db_record_run(&pool, "backup", &started, &finished, true, None, None)
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

    /// SCENARIOS T-11: the run row is written when the job **starts**, not when
    /// it returns. Driven with a job that parks until this test lets it go, so
    /// the assertions happen while the run really is in flight: `GET /jobs`
    /// must already show it, as `running` — not as the previous run's result,
    /// which is all it showed before, and not as a success or a failure.
    #[tokio::test]
    async fn a_run_is_visible_from_the_moment_it_starts() {
        let (reg, pool, _dir, _path) = test_registry().await;
        // An earlier, successful run: the thing `GET /jobs` used to keep
        // showing while a later run was in flight (or had been interrupted).
        db_record_run(
            &pool,
            "backup",
            "2026-06-01T00:00:00Z",
            "2026-06-01T00:00:01Z",
            true,
            None,
            None,
        )
        .await
        .unwrap();

        let (release, parked) = tokio::sync::oneshot::channel::<()>();
        let parked = Arc::new(tokio::sync::Mutex::new(Some(parked)));
        let job = Arc::new(RegisteredJob::from_fn(JobTrigger::Scheduled, move |_| {
            let parked = parked.clone();
            async move {
                let rx = parked.lock().await.take().expect("the job runs once");
                rx.await.expect("released");
                Ok(None)
            }
        }));

        let running = tokio::spawn({
            let pool = pool.clone();
            let job = job.clone();
            async move { run_job(&pool, "backup", &job, JobParams::default()).await }
        });

        let app = ApiClient::over(router().with_state(pool.clone()).layer(Extension(reg)));
        // Wait for the in-flight row rather than sleeping a guessed interval.
        let in_flight = loop {
            let statuses: Vec<JobStatus> = app.get("/jobs").await.json();
            let backup = statuses
                .into_iter()
                .find(|s| s.name == "backup")
                .expect("the job is registered");
            if backup.last_status == Some(JobRunStatus::Running) {
                break backup;
            }
            tokio::task::yield_now().await;
        };
        assert!(
            in_flight.last_finished_at.is_none(),
            "a run in flight has not finished"
        );
        assert!(in_flight.last_error.is_none(), "and has not failed");
        assert_eq!(
            in_flight.runs.len(),
            2,
            "the in-flight run is a history entry of its own, above the previous run"
        );
        assert_eq!(in_flight.runs[1].status, JobRunStatus::Ok);
        assert_eq!(
            in_flight.runs[1].finished_at.as_deref(),
            Some("2026-06-01T00:00:01Z"),
            "the previous run is still there, unchanged"
        );

        release.send(()).expect("the job is still parked");
        running.await.unwrap().unwrap();

        let statuses: Vec<JobStatus> = app.get("/jobs").await.json();
        let done = statuses
            .iter()
            .find(|s| s.name == "backup")
            .expect("the job is registered");
        assert_eq!(done.last_status, Some(JobRunStatus::Ok));
        assert!(done.last_finished_at.is_some());
        assert_eq!(
            done.runs.len(),
            2,
            "finishing updates the row the start opened; it does not append a second"
        );
    }

    /// The other half of SCENARIOS T-11: what a run the process did not survive
    /// leaves behind. The interruption itself is a `SIGTERM`/`SIGKILL`/power cut
    /// mid-run, which no in-process test can stage — but its *record* is exactly
    /// a started row that was never updated, which this stages directly. It must
    /// read as a run that started and never finished: not as a success, not as a
    /// failure, and — the actual defect — not as the previous run's result.
    #[tokio::test]
    async fn an_interrupted_run_reads_as_started_and_never_finished() {
        let (reg, pool, _dir, _path) = test_registry().await;
        db_record_run(
            &pool,
            "backup",
            "2026-06-01T00:00:00Z",
            "2026-06-01T00:00:01Z",
            true,
            None,
            None,
        )
        .await
        .unwrap();
        db_start_run(&pool, "backup", "2026-06-08T00:00:00Z")
            .await
            .unwrap();

        let app = ApiClient::over(router().with_state(pool.clone()).layer(Extension(reg)));
        let statuses: Vec<JobStatus> = app.get("/jobs").await.json();
        let backup = statuses
            .iter()
            .find(|s| s.name == "backup")
            .expect("the job is registered");
        assert_eq!(backup.last_status, Some(JobRunStatus::Running));
        assert_eq!(
            backup.last_started_at.as_deref(),
            Some("2026-06-08T00:00:00Z"),
            "the newest run is the interrupted one, not the week-old success"
        );
        assert!(backup.last_finished_at.is_none());

        // And the health report does not call it a failure: nothing failed.
        let health = crate::reports::health::db_health(
            &pool,
            chrono::NaiveDate::from_ymd_opt(2026, 6, 8).unwrap(),
            chrono::Utc::now(),
        )
        .await
        .unwrap();
        assert!(
            health.failed_jobs.is_empty(),
            "an unfinished run is not a failed one: {:?}",
            health.failed_jobs
        );
    }

    /// The history bound still holds when the row is written at the start — and
    /// the prune that enforces it can never take the row the in-flight run is
    /// about to update, because that row is the newest of its job and the
    /// per-job lock keeps any other run of it from inserting meanwhile.
    #[tokio::test]
    async fn starting_a_run_prunes_the_history_and_never_the_in_flight_row() {
        let (_reg, pool, _dir, _path) = test_registry().await;
        for i in 0..(JOB_RUN_HISTORY_LIMIT + 5) {
            let started = format!("2026-06-01T00:{i:02}:00Z");
            let finished = format!("2026-06-01T00:{i:02}:01Z");
            db_record_run(&pool, "backup", &started, &finished, true, None, None)
                .await
                .unwrap();
        }

        let id = db_start_run(&pool, "backup", "2026-06-02T00:00:00Z")
            .await
            .unwrap();

        let runs = db_run_histories(&pool).await.unwrap();
        let runs = runs.get("backup").unwrap();
        assert_eq!(runs.len(), JOB_RUN_HISTORY_LIMIT as usize);
        assert_eq!(runs[0].started_at, "2026-06-02T00:00:00Z");
        assert_eq!(runs[0].status, JobRunStatus::Running);

        // The row the start opened is still there to be completed.
        let still_there: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM job_runs WHERE id = ?")
            .bind(id)
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(still_there.0, 1);
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
            None,
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
        assert_eq!(backup.runs[0].status, JobRunStatus::Ok);
        assert_eq!(backup.runs[1].error.as_deref(), Some("boom"));
        assert_eq!(backup.last_status, Some(JobRunStatus::Ok));
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

    #[tokio::test]
    async fn migration_0042_carries_every_run_forward_as_ok_or_failed() {
        // 0042 relaxed finished_at to nullable and replaced the success boolean
        // with the three-valued status enum, which SQLite can only do by
        // rebuilding the table. Every recorded run must survive the rebuild with
        // its id, name, timestamps and error text intact and its success flag
        // translated — recreate the 0012 shape, apply the migration file, and
        // check nothing was dropped.
        use sqlx::Connection;
        let mut conn = sqlx::SqliteConnection::connect(":memory:").await.unwrap();
        sqlx::raw_sql(
            "CREATE TABLE job_runs (
                 id          INTEGER PRIMARY KEY AUTOINCREMENT,
                 name        TEXT    NOT NULL,
                 started_at  TEXT    NOT NULL,
                 finished_at TEXT    NOT NULL,
                 success     INTEGER NOT NULL,
                 error       TEXT
             );
             CREATE INDEX job_runs_name_id ON job_runs (name, id);
             INSERT INTO job_runs (id, name, started_at, finished_at, success, error) VALUES
                 (7, 'backup', '2026-06-01T00:00:00Z', '2026-06-01T00:00:01Z', 0, 'boom'),
                 (9, 'price-import', '2026-06-02T00:00:00Z', '2026-06-02T00:00:01Z', 1, NULL);",
        )
        .execute(&mut conn)
        .await
        .unwrap();

        sqlx::raw_sql(include_str!("../../migrations/0042_job_run_status.sql"))
            .execute(&mut conn)
            .await
            .unwrap();

        /// One migrated row, as the assertion below reads it.
        type MigratedRun = (i64, String, String, Option<String>, String, Option<String>);
        let rows: Vec<MigratedRun> = sqlx::query_as(
            "SELECT id, name, started_at, finished_at, status, error \
             FROM job_runs ORDER BY id",
        )
        .fetch_all(&mut conn)
        .await
        .unwrap();
        assert_eq!(
            rows,
            vec![
                (
                    7,
                    "backup".to_string(),
                    "2026-06-01T00:00:00Z".to_string(),
                    Some("2026-06-01T00:00:01Z".to_string()),
                    "failed".to_string(),
                    Some("boom".to_string()),
                ),
                (
                    9,
                    "price-import".to_string(),
                    "2026-06-02T00:00:00Z".to_string(),
                    Some("2026-06-02T00:00:01Z".to_string()),
                    "ok".to_string(),
                    None,
                ),
            ],
            "every run survives the rebuild, ids and all"
        );

        // The new shape accepts an unfinished run and refuses the two ways of
        // describing one incoherently.
        sqlx::query(
            "INSERT INTO job_runs (name, started_at, finished_at, status, error) \
             VALUES ('backup', '2026-06-08T00:00:00Z', NULL, 'running', NULL)",
        )
        .execute(&mut conn)
        .await
        .expect("an in-flight run has no finish time");
        for bad in [
            "INSERT INTO job_runs (name, started_at, finished_at, status) \
             VALUES ('backup', 'x', '2026-06-08T00:00:01Z', 'running')",
            "INSERT INTO job_runs (name, started_at, finished_at, status) \
             VALUES ('backup', 'x', NULL, 'ok')",
            "INSERT INTO job_runs (name, started_at, finished_at, status) \
             VALUES ('backup', 'x', '2026-06-08T00:00:01Z', 'maybe')",
        ] {
            sqlx::query(bad)
                .execute(&mut conn)
                .await
                .expect_err("the CHECKs hold status and finished_at in step");
        }
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
        let failing = RegisteredJob::from_fn(JobTrigger::Scheduled, move |_| {
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
        assert_eq!(job.last_status, Some(JobRunStatus::Failed));
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
