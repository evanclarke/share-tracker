//! The stored `job_schedule`: each spawned entry records when it is next due,
//! and the overdue check reads it.

use super::*;

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
        time::sleep(Duration::from_millis(10)).await;
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
    // The row is claimed at the task's start instant *before* the first
    // occurrence is computed (see `run_entry`), so its being there is not
    // yet the occurrence: under load a read can land between the two
    // writes and see the claim. Wait for the one that stores a future run.
    let holds_a_future_run = |rows: &[(String, String, Option<String>, String)]| {
        rows.first()
            .is_some_and(|row| chrono::DateTime::parse_from_rfc3339(&row.3).unwrap() > Utc::now())
    };
    let mut rows = wait_for_schedule(&pool, 1).await;
    for _ in 0..300 {
        if holds_a_future_run(&rows) {
            break;
        }
        time::sleep(Duration::from_millis(10)).await;
        rows = stored_schedule(&pool).await;
    }

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
    let health = health::db_health(&pool, now.date_naive(), now)
        .await
        .unwrap();
    assert!(health.overdue_jobs.is_empty());

    // …and past it, the stopped scheduler is named.
    let later = now
        + chrono::Duration::hours(health::JOB_OVERDUE_GRACE_HOURS)
        + chrono::Duration::minutes(1);
    let health = health::db_health(&pool, later.date_naive(), later)
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
    let health = health::db_health(&pool, now.date_naive(), now)
        .await
        .unwrap();
    assert!(health.overdue_jobs.is_empty());
}

/// A request arriving while the scheduler is still storing its startup
/// schedule rows must be **served**, not answered `500 database is locked`.
/// Every spawned entry claims its `job_schedule` row the instant it starts,
/// so a `PUT` landing in that window is one writer racing another — which
/// is what a deferred `BEGIN` cannot survive (it fails the upgrade outright,
/// and SQLite's busy timeout does not retry that), and what
/// `infra::db::write_tx`'s `BEGIN IMMEDIATE` exists to hold off.
///
/// Deliberately amplified on both sides, because the live failure rate is
/// far too low for one attempt to catch a regression (2 startups in 160
/// measured): the real `schedule.cron` is repeated so ~64 entry tasks claim
/// rows at once rather than production's eight, the writes racing them are
/// issued concurrently rather than one at a time, and the startup is
/// repeated. Measured against a build with `write_tx` reverted to a
/// deferred `BEGIN`, this shape catches the regression **29 times in 30**
/// (`PUT /listings/… -> 500`), and passes 30 in 30 with the fix in place.
///
/// It is still a race rather than a scripted interleaving. `infra::db`'s
/// `write_tx_holds_off_a_concurrent_writer_instead_of_failing_to_upgrade`
/// is the deterministic half of the pair.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_write_arriving_during_scheduler_startup_is_served_not_locked_out() {
    let (reg, pool, _dir, _path) = test_registry().await;
    let schedule = include_str!("../../../../schedule.cron").repeat(8);
    let entries = parse(&schedule).unwrap().len();
    let app = ApiClient::full(&pool);

    // Five startups, because the window is narrow: the entry tasks claim
    // their rows within milliseconds, so only writes issued right behind
    // them are truly concurrent. A second `spawn` in the same process is a
    // supported path (it clears the table and the fresh tasks re-claim),
    // which is what makes repeating the startup honest here.
    for round in 0..5 {
        spawn(reg.clone(), pool.clone(), &schedule).await.unwrap();

        // No waiting, and all at once: the entry tasks are claiming their
        // rows right now, and requests overlap each other as they do on a
        // real server, so every write is racing writers on both sides.
        let mut writes = Vec::new();
        for id in 1..=15 {
            let app = app.clone();
            writes.push(tokio::spawn(async move {
                let body = serde_json::json!({
                    "ticker": format!("RACE{id}"),
                    "name": format!("Race {id} Ltd, round {round}"),
                    "exchange_mic": "XASX",
                    "currency": "AUD",
                    "security_type": "Share",
                    "amit": false,
                    "preference": false,
                });
                let resp = app.put(format!("/listings/{id}"), &body).await;
                (id, resp.status, resp.text().to_owned())
            }));
        }
        for write in writes {
            let (id, status, body) = write.await.expect("the request task");
            // Either outcome is a served write: round 0 creates each id
            // (201) and every later round replaces it (204). The point is
            // that it is not a 500 database-is-locked.
            assert!(
                status == StatusCode::CREATED || status == StatusCode::NO_CONTENT,
                "PUT /listings/{id} during scheduler startup answered {status}: {body}"
            );
        }
    }

    // …and the other side of the race really did write: every entry of the
    // last startup stored its row, so this was a concurrent-writer test and
    // not a vacuous one. At least, rather than exactly: a task from an
    // earlier round whose second write lands after a later round cleared
    // the table claims a fresh row, which is the documented behaviour of a
    // second `spawn` in one process, not a fault.
    let rows = wait_for_schedule(&pool, entries).await;
    assert!(
        rows.len() >= entries,
        "every schedule entry stores a row while the writes race it"
    );
}
