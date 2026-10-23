//! The `backup` job as triggered and scheduled: the suffix, the configured
//! directory and command, and skipping the hook.

use super::*;

#[tokio::test]
async fn trigger_backup_runs_and_returns_204() {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("t.db").to_string_lossy().to_string();
    let pool = file_pool(&db_path).await;
    let reg = registry(
        pool.clone(),
        db_path.clone(),
        None,
        None,
        stub_fetcher(),
        stub_distributions(),
        None,
    );
    let app = ApiClient::over(router().with_state(pool).layer(Extension(reg)));

    let resp = app.post_empty("/jobs/backup").await;

    assert_eq!(resp.status, StatusCode::NO_CONTENT);
    // The backup job derives its own timestamped name (`t-YYYY-MM-DD-HHMMSS.db`),
    // so re-deriving the path here could land in a later second and miss it.
    // Assert instead that a backup file for this DB was written to the dir.
    let made_backup = fs::read_dir(dir.path())
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
    let pool = file_pool(&db_path).await;
    let reg = registry(
        pool.clone(),
        db_path.clone(),
        None,
        None,
        stub_fetcher(),
        stub_distributions(),
        None,
    );
    let app = ApiClient::over(router().with_state(pool).layer(Extension(reg)));

    let resp = app.post_empty("/jobs/backup?suffix=pre-0.5.1").await;

    assert_eq!(resp.status, StatusCode::NO_CONTENT);
    let made_backup = fs::read_dir(dir.path())
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

    let resp = app.post_empty("/jobs/backup?suffix=../etc/passwd").await;

    assert_eq!(resp.status, StatusCode::UNPROCESSABLE_ENTITY);
    // Beside "t.db" itself, WAL-mode sidecars (t.db-wal, t.db-shm) are
    // expected; only a backup-named file would indicate the rejected
    // suffix reached the filesystem.
    let no_backup = fs::read_dir(dir.path())
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

    let stem = path::Path::new(&db_path)
        .file_stem()
        .unwrap()
        .to_string_lossy()
        .into_owned();
    let has_suffix = fs::read_dir(dir.path())
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
    let pool = file_pool(&db_path).await;
    let reg = registry(
        pool.clone(),
        db_path,
        Some(backup_dir.path().to_string_lossy().into_owned()),
        None,
        stub_fetcher(),
        stub_distributions(),
        None,
    );

    let job = reg.get("backup").unwrap();
    run_job(&pool, "backup", job, JobParams::default())
        .await
        .unwrap();

    let in_backup_dir = fs::read_dir(backup_dir.path())
        .unwrap()
        .filter_map(|e| e.ok())
        .any(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            name.starts_with("t-") && name.ends_with(".db")
        });
    assert!(in_backup_dir, "backup job must write to the configured dir");
    let beside_db = fs::read_dir(db_dir.path())
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
    let pool = file_pool(&db_path).await;
    let marker = db_dir.path().join("hook-ran");
    let command = format!("touch {}", marker.to_string_lossy());
    let reg = registry(
        pool.clone(),
        db_path,
        None,
        Some(command),
        stub_fetcher(),
        stub_distributions(),
        None,
    );

    let job = reg.get("backup").unwrap();
    run_job(&pool, "backup", job, JobParams::default())
        .await
        .unwrap();

    assert!(
        marker.exists(),
        "the configured backup_command must have run"
    );
}

/// A hook-appending registry plus the file it appends to: each run of the
/// backup job adds one line, so a test can count how many times the
/// post-backup command actually ran.
async fn registry_with_counting_hook() -> (JobRegistry, SqlitePool, tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("t.db").to_string_lossy().to_string();
    let pool = file_pool(&db_path).await;
    let log = dir.path().join("hook-runs");
    let command = format!("echo ran >> {}", log.to_string_lossy());
    let reg = registry(
        pool.clone(),
        db_path,
        None,
        Some(command),
        stub_fetcher(),
        stub_distributions(),
        None,
    );
    (reg, pool, dir, log)
}

/// How many times the counting hook has run.
fn hook_runs(log: &path::Path) -> usize {
    fs::read_to_string(log).map_or(0, |s| s.lines().count())
}

#[tokio::test]
async fn skip_command_takes_the_backup_without_running_the_hook() {
    // The pre-upgrade path (`pkg/freebsd/update.sh`): the backup must still
    // be taken — it is the rollback point — while the off-machine copy is
    // deliberately left to the weekly run.
    let (reg, pool, dir, log) = registry_with_counting_hook().await;
    let app = ApiClient::over(router().with_state(pool).layer(Extension(reg)));

    let resp = app
        .post_empty("/jobs/backup?suffix=pre-9.9.9&skip_command=true")
        .await;

    assert_eq!(resp.status, StatusCode::NO_CONTENT);
    let made_backup = fs::read_dir(dir.path())
        .unwrap()
        .filter_map(|e| e.ok())
        .any(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            name.starts_with("t-") && name.ends_with("-pre-9.9.9.db")
        });
    assert!(made_backup, "the backup itself must still be written");
    assert_eq!(hook_runs(&log), 0, "the post-backup command must not run");
}

#[tokio::test]
async fn a_skipped_hook_is_a_success_that_says_what_it_skipped() {
    // Not an unqualified `ok`: the run did less than the whole of the job's
    // work, so the Jobs screen shows the reason beside the success
    // (SCENARIOS T-09), the same as the credential-gated currency import.
    let (reg, pool, _dir, _log) = registry_with_counting_hook().await;
    let job = reg.get("backup").unwrap();
    run_job(
        &pool,
        "backup",
        job,
        JobParams {
            skip_command: true,
            ..JobParams::default()
        },
    )
    .await
    .expect("a deliberately skipped hook is not a failure");

    let app = ApiClient::over(router().with_state(pool).layer(Extension(reg)));
    let statuses: Vec<JobStatus> = app.get("/jobs").await.json();
    let backup = statuses.iter().find(|s| s.name == "backup").unwrap();
    assert_eq!(backup.last_status, Some(JobRunStatus::Ok));
    assert!(backup.last_error.is_none());
    assert_eq!(
        backup.last_note.as_deref(),
        Some("post-backup command skipped at the caller's request")
    );
}

#[tokio::test]
async fn skipping_the_hook_applies_to_that_run_only() {
    // The flag suppresses the command, it does not unconfigure it: the very
    // next run — the weekly scheduled one, which is what the off-machine
    // copy exists for — must copy off-machine as configured, and record no
    // note.
    let (reg, pool, _dir, log) = registry_with_counting_hook().await;
    let job = reg.get("backup").unwrap();
    run_job(
        &pool,
        "backup",
        job,
        JobParams {
            suffix: Some("pre-9.9.9".to_owned()),
            skip_command: true,
        },
    )
    .await
    .unwrap();
    assert_eq!(hook_runs(&log), 0);

    run_job(&pool, "backup", job, JobParams::default())
        .await
        .unwrap();

    assert_eq!(hook_runs(&log), 1, "the next run must run the hook again");
    let app = ApiClient::over(router().with_state(pool).layer(Extension(reg)));
    let statuses: Vec<JobStatus> = app.get("/jobs").await.json();
    let backup = statuses.iter().find(|s| s.name == "backup").unwrap();
    assert!(backup.last_note.is_none(), "a complete run carries no note");
}

#[tokio::test]
async fn skip_command_with_no_hook_configured_is_an_ordinary_complete_run() {
    // Nothing was passed over on a host with no backup_command, so the run
    // must not claim it skipped anything.
    let (reg, pool, _dir, _path) = test_registry().await;
    let job = reg.get("backup").unwrap();
    run_job(
        &pool,
        "backup",
        job,
        JobParams {
            skip_command: true,
            ..JobParams::default()
        },
    )
    .await
    .unwrap();

    let app = ApiClient::over(router().with_state(pool).layer(Extension(reg)));
    let statuses: Vec<JobStatus> = app.get("/jobs").await.json();
    let backup = statuses.iter().find(|s| s.name == "backup").unwrap();
    assert_eq!(backup.last_status, Some(JobRunStatus::Ok));
    assert!(backup.last_note.is_none());
}

#[tokio::test]
async fn skip_command_accepts_the_bare_flag_form() {
    // `?skip_command` with no value is how a flag is written everywhere
    // else on the box; rejecting it would be a 422 for a request whose
    // meaning is unambiguous.
    let (reg, pool, _dir, log) = registry_with_counting_hook().await;
    let app = ApiClient::over(router().with_state(pool).layer(Extension(reg)));

    let resp = app.post_empty("/jobs/backup?skip_command").await;

    assert_eq!(resp.status, StatusCode::NO_CONTENT);
    assert_eq!(hook_runs(&log), 0);
}

#[tokio::test]
async fn skip_command_false_still_runs_the_hook() {
    let (reg, pool, _dir, log) = registry_with_counting_hook().await;
    let app = ApiClient::over(router().with_state(pool).layer(Extension(reg)));

    let resp = app.post_empty("/jobs/backup?skip_command=false").await;

    assert_eq!(resp.status, StatusCode::NO_CONTENT);
    assert_eq!(hook_runs(&log), 1);
}

#[tokio::test]
async fn an_unreadable_skip_command_value_is_400_and_runs_nothing() {
    // The `sufix` lesson (SCENARIOS T-10) applied to the flag: a value that
    // is neither true nor false must not quietly read as "off" and copy a
    // pre-upgrade backup off-machine after all.
    let (reg, pool, _dir, log) = registry_with_counting_hook().await;
    let app = ApiClient::over(router().with_state(pool).layer(Extension(reg)));

    let resp = app.post_empty("/jobs/backup?skip_command=maybe").await;
    let (status, body) = resp.status_and_body();

    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(
        body.contains("maybe") && body.contains("true or false"),
        "the reason must name the bad value: {body}"
    );
    assert_eq!(hook_runs(&log), 0, "and nothing must have run");
}
