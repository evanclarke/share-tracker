//! The maintenance-job registry: what each job name *does*.
//!
//! A job is an async closure taking [`JobParams`]; [`RegisteredJob::from_fn`]
//! wraps it into the boxed-future shape the scheduler stores and pairs it with
//! its serialising lock, so each entry in [`registry`] is just a name and a
//! body.

use serde::Deserialize;
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

/// A registered job: the work plus a per-job lock serialising its execution.
/// `run_job` holds the lock for the whole run, so a manual trigger can never
/// overlap the scheduled run (or a second trigger) of the same job — a
/// concurrent caller waits and runs after.
pub struct RegisteredJob {
    pub(super) work: Job,
    pub(super) lock: tokio::sync::Mutex<()>,
}

impl RegisteredJob {
    /// Build a job from an async closure: box its future into the stored [`Job`]
    /// shape and give it its own lock. Every job — registered below or synthetic
    /// in a test — is constructed here, so none can be created lockless.
    pub(super) fn from_fn<F, Fut>(work: F) -> Arc<Self>
    where
        F: Fn(JobParams) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<(), String>> + Send + 'static,
    {
        Arc::new(Self {
            work: Arc::new(move |params| Box::pin(work(params))),
            lock: tokio::sync::Mutex::new(()),
        })
    }
}

/// Job-name → work. Shared between the spawned schedule tasks and the HTTP
/// trigger handler (injected as an axum `Extension`).
pub type JobRegistry = Arc<HashMap<String, Arc<RegisteredJob>>>;

/// Add one job to the map under `name`. Keeps [`registry`] a flat list of
/// name-plus-body pairs rather than repeating the wrapping at every entry.
fn register<F, Fut>(jobs: &mut HashMap<String, Arc<RegisteredJob>>, name: &str, work: F)
where
    F: Fn(JobParams) -> Fut + Send + Sync + 'static,
    Fut: Future<Output = Result<(), String>> + Send + 'static,
{
    jobs.insert(name.to_string(), RegisteredJob::from_fn(work));
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
                    .map_err(|e| format!("{e:?}"))?;
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
                    .map_err(|e| format!("{e:?}"))?;
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
                .map_err(|e| format!("{e:?}"))?;
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
