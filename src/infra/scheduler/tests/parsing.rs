//! Parsing `schedule.cron`, computing each entry's next run (timezones and
//! DST included), and what `spawn` warns about at startup.

use super::*;

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
    assert_eq!(entries[0].tz, Some(America::New_York));
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
        other @ ScheduleError::UnknownJob { .. } => {
            panic!("expected Parse error, got {other:?}")
        }
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
        other @ ScheduleError::UnknownJob { .. } => {
            panic!("expected Parse error, got {other:?}")
        }
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
    spawn(reg, pool, include_str!("../../../../schedule.cron"))
        .await
        .unwrap();
}

#[test]
fn backup_is_scheduled_weekly() {
    // REQUIREMENTS specifies weekly backups: the committed schedule's backup
    // entry must parse and fire exactly 7 days apart.
    let entries = parse(include_str!("../../../../schedule.cron")).unwrap();
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
    let entries = parse(include_str!("../../../../schedule.cron")).unwrap();
    let zones: Vec<Option<Tz>> = entries
        .iter()
        .filter(|e| e.name == "price-import")
        .map(|e| e.tz)
        .collect();
    assert_eq!(zones.len(), 3, "ASX, NYSE, and crypto-UTC runs expected");
    assert!(zones.contains(&Some(Australia::Sydney)));
    assert!(zones.contains(&Some(America::New_York)));
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

#[test]
fn the_hour_before_a_dst_gap_fires_once_not_again_after_the_gap() {
    // Sydney springs forward on 2026-10-04 at 02:00. A daily 01:00 entry
    // fires at 01:00 AEST; croner then offers 03:00 AEDT — the first
    // instant after the gap — as the "next 01:00", which ran the job a
    // second time that night. The next fire is the following day's 01:00.
    let tz: Tz = "Australia/Sydney".parse().unwrap();
    let cron = Cron::from_str("0 1 * * *").unwrap();
    let now = tz.with_ymd_and_hms(2026, 10, 3, 12, 0, 0).unwrap();
    let (first, _) = next_run(&cron, now).unwrap();
    assert_eq!(first, tz.with_ymd_and_hms(2026, 10, 4, 1, 0, 0).unwrap());
    let (after, _) = next_run(&cron, first).unwrap();
    assert_eq!(after, tz.with_ymd_and_hms(2026, 10, 5, 1, 0, 0).unwrap());
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
    let pool = test_pool().await;
    pool.close().await;
    time::pause();
    let t0 = Utc.with_ymd_and_hms(2026, 6, 1, 0, 0, 0).unwrap();
    let start = Instant::now();
    let clock = move || {
        let elapsed = Instant::now() - start;
        let mut now = t0 + chrono::Duration::from_std(elapsed).unwrap();
        if elapsed >= Duration::from_secs(90 * 60) {
            now += chrono::Duration::hours(1); // the wall-clock shift
        }
        now
    };

    let fired_at = Arc::new(Mutex::new(Vec::<DateTime<Utc>>::new()));
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
        expr: "30 3 1 6 *".to_owned(),
        tz: None,
        name: "fake".to_owned(),
    };
    tokio::spawn(run_entry(pool, entry, job, clock));
    // Paused time auto-advances through the loop's sleeps; run well past
    // the target, then check the fire time against the shifted wall clock.
    time::sleep(Duration::from_secs(4 * 3600)).await;

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

/// Polls `cond` until it holds, up to a generous deadline, so a test can
/// wait on something a spawned task does without guessing how long the
/// executor needs. Panics rather than hanging if it never holds, so a real
/// regression is a failure with a name rather than a stuck suite.
async fn wait_until(mut cond: impl FnMut() -> bool, what: &str) {
    let deadline = StdInstant::now() + Duration::from_secs(30);
    while !cond() {
        assert!(
            StdInstant::now() < deadline,
            "waited 30s for {what} and it never happened"
        );
        time::sleep(Duration::from_millis(1)).await;
    }
}

#[tracing_test::traced_test]
#[tokio::test]
async fn next_run_log_shows_timezone() {
    let (reg, pool, _dir, _path) = test_registry().await;
    spawn(reg, pool, "0 0 * * *   Pacific/Auckland   backup\n")
        .await
        .unwrap();
    // Wait for the line itself, not for a guessed number of yields. A
    // fixed `yield_now()` count bounds how many times *this* task defers,
    // not whether the spawned one has been polled far enough to log, so
    // under CPU contention the assertion reads an empty buffer and the
    // test fails for reasons unrelated to the behaviour under test — it
    // went red in CI on 2026-08-22 at 1953 passed / 1 failed, on a run
    // whose suite took 140s. Waiting on the condition is also faster in
    // the common case: it returns on the first poll that sees the line.
    wait_until(|| logs_contain("next run scheduled"), "next run scheduled").await;
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
    spawn(reg, pool, include_str!("../../../../schedule.cron"))
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
    let entries = parse(include_str!("../../../../schedule.cron")).unwrap();
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
