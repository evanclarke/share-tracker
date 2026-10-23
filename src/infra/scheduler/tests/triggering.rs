//! Running a job: the started/finished log lines and the manual trigger's
//! answers (404, 500, a misspelt parameter).

use super::*;

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
    let pool = test_pool().await;
    let mut jobs: HashMap<String, Arc<RegisteredJob>> = HashMap::new();
    jobs.insert(
        "always-fails".to_owned(),
        RegisteredJob::from_fn(JobTrigger::ManualOnly, |_| async {
            Err("could not fetch the RBA FX rate feed: connection refused".to_owned())
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
    // backup — the operator's one-off label silently lost. It is now a 400
    // with a reason (the query-string shape every route answers), rejected
    // before any run is recorded.
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
    let app = ApiClient::over(router().with_state(pool.clone()).layer(Extension(reg)));

    let resp = app.post_empty("/jobs/backup?sufix=pre-0.5.1").await;

    assert_eq!(resp.status, StatusCode::BAD_REQUEST);
    let body = resp.text();
    assert!(
        body.contains("sufix") && body.contains("suffix"),
        "the 400 must name the parameter it did not understand: {body}"
    );
    // Nothing ran: no backup file, no recorded run.
    let no_backup = fs::read_dir(dir.path())
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
