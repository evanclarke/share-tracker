//! The bounded `job_runs` history: what a run records, pruning, a run visible
//! from its start, and the migrations that carried old rows forward.

use super::*;

#[tokio::test]
async fn list_jobs_returns_registered_names() {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("t.db").to_string_lossy().to_string();
    let pool = file_pool(&db_path).await;
    let reg = registry(
        pool.clone(),
        db_path,
        None,
        None,
        stub_fetcher(),
        stub_distributions(),
        None,
    );
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
        Ok(Some(note.to_owned()))
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
        &Err("boom".to_owned()),
    )
    .await
    .unwrap();
    db_record_run(
        &pool,
        "backup",
        "2026-06-02T00:00:00Z",
        "2026-06-02T00:00:01Z",
        &Ok(None),
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
        &Ok(None),
    )
    .await
    .unwrap();
    let extra = 5;
    for i in 0..(JOB_RUN_HISTORY_LIMIT + extra) {
        let started = format!("2026-06-01T00:{i:02}:00Z");
        let finished = format!("2026-06-01T00:{i:02}:01Z");
        db_record_run(&pool, "backup", &started, &finished, &Ok(None))
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
        &Ok(None),
    )
    .await
    .unwrap();

    let (release, parked) = oneshot::channel::<()>();
    let parked = Arc::new(AsyncMutex::new(Some(parked)));
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
        task::yield_now().await;
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
        &Ok(None),
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
    let health = health::db_health(
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
        db_record_run(&pool, "backup", &started, &finished, &Ok(None))
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
        &Err("boom".to_owned()),
    )
    .await
    .unwrap();
    db_record_run(
        &pool,
        "backup",
        "2026-06-02T00:00:00Z",
        "2026-06-02T00:00:01Z",
        &Ok(None),
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

    sqlx::raw_sql(include_str!(
        "../../../../migrations/0012_job_run_history.sql"
    ))
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
            "backup".to_owned(),
            "2026-06-01T00:00:00Z".to_owned(),
            false,
            Some("boom".to_owned())
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

    sqlx::raw_sql(include_str!(
        "../../../../migrations/0042_job_run_status.sql"
    ))
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
                "backup".to_owned(),
                "2026-06-01T00:00:00Z".to_owned(),
                Some("2026-06-01T00:00:01Z".to_owned()),
                "failed".to_owned(),
                Some("boom".to_owned()),
            ),
            (
                9,
                "price-import".to_owned(),
                "2026-06-02T00:00:00Z".to_owned(),
                Some("2026-06-02T00:00:01Z".to_owned()),
                "ok".to_owned(),
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
