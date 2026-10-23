//! `GET /jobs`' trigger classification, unknown-job refusals at spawn, and
//! the per-job lock that serialises concurrent runs.

use super::*;

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
    let schedule =
        "# weekly maintenance\n\n0 0 * * 0   backup\n# bad line below\n0 1 * * *   no-such-job\n";
    let err = spawn(reg, pool, schedule).await.unwrap_err();
    match err {
        ScheduleError::UnknownJob { line, name } => {
            assert_eq!(line, 5);
            assert_eq!(name, "no-such-job");
        }
        other @ ScheduleError::Parse { .. } => {
            panic!("expected UnknownJob error, got {other:?}")
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_runs_of_same_job_serialise() {
    // Two run_job calls for the same job (e.g. a manual trigger overlapping
    // the scheduled run) must never execute the work concurrently: the
    // second waits on the per-job lock and runs after the first finishes.
    //
    // Sequenced on events, not durations: the first run's work holds at a
    // gate the test opens only once the second run is observed blocked on
    // the lock (`blocked`), so the window in which an overlap would show
    // is the whole of the first run, however slow the machine.
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    let pool = test_pool().await;
    let active = Arc::new(AtomicUsize::new(0));
    let overlapped = Arc::new(AtomicBool::new(false));
    let started = Arc::new(AtomicUsize::new(0));
    let runs = Arc::new(AtomicUsize::new(0));
    let gate = Arc::new(Semaphore::new(0));
    let (a, o, s, r, g) = (
        active.clone(),
        overlapped.clone(),
        started.clone(),
        runs.clone(),
        gate.clone(),
    );
    let job = RegisteredJob::from_fn(JobTrigger::Scheduled, move |_| {
        let (a, o, s, r, g) = (a.clone(), o.clone(), s.clone(), r.clone(), g.clone());
        async move {
            s.fetch_add(1, Ordering::SeqCst);
            if a.fetch_add(1, Ordering::SeqCst) > 0 {
                o.store(true, Ordering::SeqCst);
            }
            g.acquire().await.expect("the gate stays open").forget();
            a.fetch_sub(1, Ordering::SeqCst);
            r.fetch_add(1, Ordering::SeqCst);
            Ok(None)
        }
    });
    let spawn_run = || {
        let (pool, job) = (pool.clone(), job.clone());
        tokio::spawn(async move { run_job(&pool, "same-job", &job, JobParams::default()).await })
    };

    let first = spawn_run();
    await_condition(|| started.load(Ordering::SeqCst) == 1).await;
    let second = spawn_run();
    await_condition(|| job.blocked.load(Ordering::SeqCst) == 1).await;
    assert_eq!(
        started.load(Ordering::SeqCst),
        1,
        "the second run is queued behind the first's lock, not running"
    );

    gate.add_permits(2);
    first.await.unwrap().unwrap();
    second.await.unwrap().unwrap();
    assert_eq!(runs.load(Ordering::SeqCst), 2, "both runs must complete");
    assert!(
        !overlapped.load(Ordering::SeqCst),
        "the second run must wait for the first, not execute concurrently"
    );
}
