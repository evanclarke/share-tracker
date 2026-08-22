//! The maintenance-job registry: what each job name *does*.
//!
//! A job is an async closure taking [`JobParams`]; [`RegisteredJob::from_fn`]
//! wraps it into the boxed-future shape the scheduler stores and pairs it with
//! its serialising lock, so each entry in [`registry`] is just a name and a
//! body.
//!
//! A body reports a failure as its error's **`Display`** (`e.to_string()`),
//! never the derived `Debug` (`{e:?}`). That string is the whole of what the
//! operator ever sees — `job_runs.error`, the Jobs table's Error column, the
//! health banner — and every error enum in the tree carries an `#[error("…")]`
//! written to be exactly that, which the `Debug` form discards in favour of
//! Rust syntax (SCENARIOS T-06). `scheduler::tests::
//! no_registered_job_records_its_failure_as_a_debug_string` pins it.

use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;
use std::{collections::HashMap, future::Future, pin::Pin, sync::Arc};

/// A unit of scheduled work. Each call runs the job once, returning `Ok(())` on
/// success or a human-readable error. Jobs do their own detailed INFO logging.
type JobFuture = Pin<Box<dyn Future<Output = Result<(), String>> + Send>>;
type Job = Arc<dyn Fn(JobParams) -> JobFuture + Send + Sync>;

/// Caller-supplied parameters for a manual job trigger, taken from the
/// `POST /jobs/{name}` query string. Currently only the `backup` job reads
/// `suffix` (see [`crate::infra::db::backup`]); every other registered job
/// ignores it. The scheduled loop always passes [`JobParams::default`] — a
/// suffix only makes sense for a deliberately-labelled one-off backup, not
/// the weekly scheduled run.
#[derive(Debug, Default, Clone, Deserialize)]
pub struct JobParams {
    pub suffix: Option<String>,
}

/// How a registered job is meant to be started — the registry's record of
/// whether the job expects a line in `schedule.cron`.
///
/// This is what separates a *lost* schedule line from a deliberately absent
/// one: [`super::schedule::spawn`] warns about a [`JobTrigger::Scheduled`] job
/// with no entry (an oversight), and says nothing about a
/// [`JobTrigger::ManualOnly`] one (SCENARIOS T-09/schedule — two permanent WARN
/// lines every startup buried the one that would matter). It is carried on
/// `GET /jobs` too, so the Jobs screen can label a never-scheduled job as what
/// it is rather than leaving it looking overdue.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JobTrigger {
    /// Recurring work: it belongs on the schedule, so a missing line is a fault.
    Scheduled,
    /// Deliberately schedule-less — a one-off repair run via
    /// `POST /jobs/{name}` when the operator needs it, never on a timer.
    ManualOnly,
}

/// A registered job: the work, how it is meant to be triggered, and a per-job
/// lock serialising its execution. `run_job` holds the lock for the whole run,
/// so a manual trigger can never overlap the scheduled run (or a second
/// trigger) of the same job — a concurrent caller waits and runs after.
pub struct RegisteredJob {
    pub(super) work: Job,
    pub(super) trigger: JobTrigger,
    pub(super) lock: tokio::sync::Mutex<()>,
}

impl RegisteredJob {
    /// Build a job from an async closure: box its future into the stored [`Job`]
    /// shape and give it its own lock. Every job — registered below or synthetic
    /// in a test — is constructed here, so none can be created lockless.
    pub(super) fn from_fn<F, Fut>(trigger: JobTrigger, work: F) -> Arc<Self>
    where
        F: Fn(JobParams) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<(), String>> + Send + 'static,
    {
        Arc::new(Self {
            work: Arc::new(move |params| Box::pin(work(params))),
            trigger,
            lock: tokio::sync::Mutex::new(()),
        })
    }
}

/// Job-name → work. Shared between the spawned schedule tasks and the HTTP
/// trigger handler (injected as an axum `Extension`).
pub type JobRegistry = Arc<HashMap<String, Arc<RegisteredJob>>>;

/// Add one **scheduled** job to the map under `name`. Keeps [`registry`] a flat
/// list of name-plus-body pairs rather than repeating the wrapping at every
/// entry. A job registered this way expects a line in `schedule.cron`; without
/// one, startup warns.
fn register<F, Fut>(jobs: &mut HashMap<String, Arc<RegisteredJob>>, name: &str, work: F)
where
    F: Fn(JobParams) -> Fut + Send + Sync + 'static,
    Fut: Future<Output = Result<(), String>> + Send + 'static,
{
    jobs.insert(
        name.to_string(),
        RegisteredJob::from_fn(JobTrigger::Scheduled, work),
    );
}

/// Add one **deliberately schedule-less** job: a one-off repair that only ever
/// runs via `POST /jobs/{name}`. Identical to [`register`] except that startup
/// stays silent about the missing schedule line and `GET /jobs` reports the job
/// as manual-only, so the "no schedule entry" WARN means what it says.
fn register_manual<F, Fut>(jobs: &mut HashMap<String, Arc<RegisteredJob>>, name: &str, work: F)
where
    F: Fn(JobParams) -> Fut + Send + Sync + 'static,
    Fut: Future<Output = Result<(), String>> + Send + 'static,
{
    jobs.insert(
        name.to_string(),
        RegisteredJob::from_fn(JobTrigger::ManualOnly, work),
    );
}

/// Build the registry of maintenance jobs, wiring each name to existing work
/// functions. Adding a future job is a new entry here plus a line in the
/// schedule file. The price source is injected (not constructed here) so the
/// live `YahooFetcher` only ever reaches the registry from `main`; tests pass a
/// stub and never touch the network.
pub fn registry(
    pool: SqlitePool,
    db_path: String,
    backup_dir: Option<String>,
    backup_command: Option<String>,
    fetcher: crate::entities::closing_price::SharedFetcher,
) -> JobRegistry {
    let mut jobs: HashMap<String, Arc<RegisteredJob>> = HashMap::new();

    register(&mut jobs, "backup", {
        let pool = pool.clone();
        move |params: JobParams| {
            let (pool, db_path) = (pool.clone(), db_path.clone());
            let (backup_dir, backup_command) = (backup_dir.clone(), backup_command.clone());
            async move {
                crate::infra::db::backup(
                    &pool,
                    &db_path,
                    backup_dir.as_deref(),
                    backup_command.as_deref(),
                    params.suffix.as_deref(),
                )
                .await
                .map_err(|e| e.to_string())
            }
        }
    });

    register(&mut jobs, "mic-import", {
        let pool = pool.clone();
        move |_| {
            let pool = pool.clone();
            async move {
                let summary = crate::entities::mic_registry::run_import(&pool)
                    .await
                    .map_err(|e| e.to_string())?;
                tracing::info!(imported = summary.imported, "MIC registry import complete");
                Ok(())
            }
        }
    });

    register(&mut jobs, "currency-import", {
        let pool = pool.clone();
        move |_| {
            let pool = pool.clone();
            async move {
                let summary = crate::entities::currencies::run_import(&pool)
                    .await
                    .map_err(|e| e.to_string())?;
                tracing::info!(imported = summary.imported, "currency import complete");
                Ok(())
            }
        }
    });

    register(&mut jobs, "price-import", {
        let pool = pool.clone();
        move |_| {
            let (pool, fetcher) = (pool.clone(), fetcher.clone());
            async move {
                crate::entities::closing_price::run_collection(
                    &pool,
                    fetcher.as_ref(),
                    chrono::Utc::now(),
                )
                .await
            }
        }
    });

    // Not on the schedule: a one-off repair, not recurring work. Recording a
    // corporate action already re-bases that listing's prices inside its own
    // transaction, so the only database this has anything to do is one whose
    // prices were stored before the basis rule existed (migration 0034).
    // Idempotent — it re-derives each price from the figure as observed — so
    // running it again changes nothing.
    register_manual(&mut jobs, "price-rebase", {
        let pool = pool.clone();
        move |_| {
            let pool = pool.clone();
            async move { crate::entities::closing_price::run_rebase(&pool).await }
        }
    });

    // Not on the schedule either, and for the same reason: a repair run after
    // the user does something, not recurring work. `exchange_holidays` is
    // seeded per published calendar year, so a settlement window running past
    // the last seeded year is computed skipping weekends only; seeding that
    // year makes `GET /reports/settlement_holiday_coverage` go quiet without
    // correcting the dates it flagged (SCENARIOS S-04). This re-derives them.
    // Only the dates the server computed itself are rewritten — a supplied
    // `settlement_date` is the taxpayer's own assertion (S-05) — and it is
    // idempotent, so running it again changes nothing.
    register_manual(&mut jobs, "settlement-recompute", {
        let pool = pool.clone();
        move |_| {
            let pool = pool.clone();
            async move { crate::entities::trade::run_recompute(&pool).await }
        }
    });

    register(&mut jobs, "report-snapshot", {
        let pool = pool.clone();
        move |_| {
            let pool = pool.clone();
            async move { crate::reports::snapshot::run_snapshot_job(&pool, chrono::Utc::now()).await }
        }
    });

    register(&mut jobs, "rba-fx-import", move |_| {
        let pool = pool.clone();
        async move {
            let summary = crate::entities::rba_fx_rate::run_import(&pool)
                .await
                .map_err(|e| e.to_string())?;
            tracing::info!(
                inserted = summary.inserted,
                skipped = summary.skipped,
                "RBA FX rate import complete"
            );
            // New rates may finalise provisional snapshots in this same run; a
            // blocked true-up date fails the job so the Jobs UI surfaces it (the
            // import itself succeeded and is idempotent).
            match crate::entities::rba_fx_rate::true_up_provisional_snapshots(&pool, &summary).await
            {
                Ok(None) => Ok(()),
                Ok(Some(t)) if t.blocked.is_empty() => Ok(()),
                Ok(Some(t)) => Err(format!(
                    "import ok ({} new rates); provisional snapshot true-up blocked: {}",
                    summary.inserted,
                    t.blocked
                        .iter()
                        .map(|b| format!("{}: {}", b.date, b.reason))
                        .collect::<Vec<_>>()
                        .join("; ")
                )),
                Err(e) => Err(format!("provisional snapshot true-up failed: {e}")),
            }
        }
    });

    Arc::new(jobs)
}
