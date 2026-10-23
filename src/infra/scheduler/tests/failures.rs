//! Failed, disconnected and panicking runs: what is recorded, and that the
//! lock is released and the schedule survives.

use super::*;

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
    let offenders: Vec<String> = include_str!("../registry.rs")
        .lines()
        .enumerate()
        .filter(|(_, line)| !line.trim_start().starts_with("//") && line.contains(&debug_format))
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
    let transport = reqwest::get(test_support::unreachable_url("f11-data.csv"))
        .await
        .expect_err("nothing is listening on that port");
    let import_error = ImportError::Fetch(fetch::cause_chain(&transport));
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

/// The latest `job_runs` row of `name`: its status and recorded error.
async fn latest_run(pool: &SqlitePool, name: &str) -> Option<(String, Option<String>)> {
    sqlx::query_as("SELECT status, error FROM job_runs WHERE name = ? ORDER BY id DESC LIMIT 1")
        .bind(name)
        .fetch_optional(pool)
        .await
        .unwrap()
}

/// 2026-10-10 review: `POST /jobs/{name}` awaited the run inline, so a
/// client that disconnected mid-job (closing the tab, Ctrl-C in curl) had
/// its request future dropped — cancelling the job between two commits and
/// leaving its row `running` for ever. Dropping the request now detaches
/// from the run, which finishes and records itself.
#[tokio::test]
async fn a_manual_run_survives_its_client_disconnecting() {
    let pool = test_pool().await;
    let (started_tx, started_rx) = oneshot::channel::<()>();
    let (release_tx, release_rx) = oneshot::channel::<()>();
    let channels = Arc::new(Mutex::new(Some((started_tx, release_rx))));
    let finished = Arc::new(AtomicBool::new(false));
    let done = finished.clone();
    let mut jobs: HashMap<String, Arc<RegisteredJob>> = HashMap::new();
    jobs.insert(
        "slow".to_owned(),
        RegisteredJob::from_fn(JobTrigger::ManualOnly, move |_| {
            let channels = channels.clone();
            let done = done.clone();
            async move {
                let (started, release) = channels.lock().unwrap().take().expect("runs once");
                started.send(()).unwrap();
                release.await.expect("released");
                done.store(true, atomic::Ordering::SeqCst);
                Ok(None)
            }
        }),
    );
    let reg: JobRegistry = Arc::new(jobs);
    let app = ApiClient::over(router().with_state(pool.clone()).layer(Extension(reg)));

    // Drop the request future once the job is under way — the disconnect.
    tokio::select! {
        resp = app.post_empty("/jobs/slow") => panic!("the job is parked, yet answered {}", resp.status),
        started = started_rx => started.expect("the job started"),
    }
    assert_eq!(
        latest_run(&pool, "slow").await.map(|r| r.0).as_deref(),
        Some("running")
    );

    release_tx.send(()).expect("the run outlived its request");
    let deadline = StdInstant::now() + Duration::from_secs(30);
    loop {
        if latest_run(&pool, "slow").await.map(|r| r.0).as_deref() == Some("ok") {
            break;
        }
        assert!(
            StdInstant::now() < deadline,
            "the detached run never recorded its finish"
        );
        task::yield_now().await;
    }
    assert!(finished.load(atomic::Ordering::SeqCst));
}

/// A job that panics is a failed run carrying the panic's message, and
/// releases its lock: the next run of the same job proceeds.
#[tokio::test]
async fn a_panicking_run_is_recorded_as_failed_and_releases_the_lock() {
    let pool = test_pool().await;
    let calls = Arc::new(AtomicUsize::new(0));
    let counted = calls.clone();
    let job = RegisteredJob::from_fn(JobTrigger::Scheduled, move |_| {
        let first = counted.fetch_add(1, atomic::Ordering::SeqCst) == 0;
        async move {
            if first {
                panic!("the feed parser hit an impossible row");
            }
            Ok(None)
        }
    });

    let err = run_job(&pool, "flaky", &job, JobParams::default())
        .await
        .expect_err("a panic is a failure");
    assert_eq!(err, "job panicked: the feed parser hit an impossible row");
    assert_eq!(
        latest_run(&pool, "flaky").await,
        Some(("failed".to_owned(), Some(err)))
    );

    // The lock was released, so this neither deadlocks nor fails.
    time::timeout(
        Duration::from_secs(30),
        run_job(&pool, "flaky", &job, JobParams::default()),
    )
    .await
    .expect("the panicking run released the job's lock")
    .expect("the second run succeeds");
    assert_eq!(
        latest_run(&pool, "flaky").await.map(|r| r.0).as_deref(),
        Some("ok")
    );
}

/// The manual trigger answers a panicking job like any other failure: a
/// `500` carrying the recorded message, not a dropped connection.
#[tokio::test]
async fn a_panicking_job_answers_500_carrying_the_panic() {
    let pool = test_pool().await;
    let mut jobs: HashMap<String, Arc<RegisteredJob>> = HashMap::new();
    jobs.insert(
        "panics".to_owned(),
        RegisteredJob::from_fn(JobTrigger::ManualOnly, |_| async {
            panic!("{}", format!("index {} out of range", 7))
        }),
    );
    let reg: JobRegistry = Arc::new(jobs);
    let app = ApiClient::over(router().with_state(pool.clone()).layer(Extension(reg)));

    let resp = app.post_empty("/jobs/panics").await;

    assert_eq!(resp.status, StatusCode::INTERNAL_SERVER_ERROR);
    assert_eq!(resp.text(), "job panicked: index 7 out of range");
}

/// 2026-10-10 review: `run_entry` had no guard against a panicking run, so
/// one panic unwound out of the timer loop and the job was never scheduled
/// again until a restart. A job that panics on its first scheduled run must
/// still fire on the next one. (Clock paused and pool closed for the reason
/// `capped_sleep_reanchors_after_wall_clock_shift` gives.)
#[tokio::test]
async fn a_panicking_scheduled_run_keeps_the_job_scheduled() {
    let pool = test_pool().await;
    pool.close().await;
    time::pause();
    let t0 = Utc.with_ymd_and_hms(2026, 6, 1, 0, 0, 30).unwrap();
    let start = Instant::now();
    let clock = move || t0 + chrono::Duration::from_std(Instant::now() - start).unwrap();

    let calls = Arc::new(AtomicUsize::new(0));
    let counted = calls.clone();
    let job = RegisteredJob::from_fn(JobTrigger::Scheduled, move |_| {
        let first = counted.fetch_add(1, atomic::Ordering::SeqCst) == 0;
        async move {
            if first {
                panic!("first run panics");
            }
            Ok(None)
        }
    });
    let entry = ScheduleEntry {
        line: 1,
        cron: Cron::from_str("* * * * *").unwrap(),
        expr: "* * * * *".to_owned(),
        tz: None,
        name: "flaky".to_owned(),
    };
    let looping = tokio::spawn(run_entry(pool, entry, job, clock));
    // Fires at 00:01 (panics) and 00:02, 00:03 (succeed).
    time::sleep(Duration::from_secs(3 * 60)).await;

    assert!(
        calls.load(atomic::Ordering::SeqCst) >= 3,
        "the job stopped being scheduled after its panic"
    );
    assert!(!looping.is_finished(), "the timer loop ended");
}
