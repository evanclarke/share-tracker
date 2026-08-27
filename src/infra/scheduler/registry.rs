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

/// What one run of a job returns: `Ok(None)` for a run that did the whole of
/// its work, `Ok(Some(note))` for one that succeeded while doing **less** than
/// that and says which part it passed over, or `Err(message)` for a failure.
///
/// The middle case exists because a green Jobs screen is the operator's
/// evidence that a job's work is done, and one job's work is legitimately
/// partial: the currency import skips the credential-gated ISO 24165 half when
/// no DTIF credentials are configured, and reported that as an unqualified
/// success with the gap only in a WARN (SCENARIOS T-09). The note is recorded
/// against the run (`job_runs.note`) and shown beside its status — the run
/// stays a success, it just stops reading as a complete one. Failing such a run
/// instead was considered and rejected: the credentials are optional by design.
pub type JobOutcome = Result<Option<String>, String>;

/// A unit of scheduled work. Each call runs the job once, returning a
/// [`JobOutcome`]. Jobs do their own detailed INFO logging.
type JobFuture = Pin<Box<dyn Future<Output = JobOutcome> + Send>>;
type Job = Arc<dyn Fn(JobParams) -> JobFuture + Send + Sync>;

/// Caller-supplied parameters for a manual job trigger, taken from the
/// `POST /jobs/{name}` query string. Currently only the `backup` job reads
/// them — `suffix` and `skip_command` (see [`crate::infra::db::backup`]);
/// every other registered job ignores both. The scheduled loop always passes
/// [`JobParams::default`]: a label and a suppressed off-machine copy only make
/// sense for a deliberate one-off backup, never for the weekly scheduled run,
/// which is the run the off-machine copy exists for.
///
/// `skip_command` suppresses the configured post-backup command for that one
/// run. Its case is the pre-upgrade backup (`pkg/freebsd/update.sh`): that
/// backup is a local rollback point taken seconds before `pkg add`, and
/// shipping a full copy of the database off-machine — which is what the
/// command typically does — delays the upgrade for as long as the transfer
/// takes, for a file the weekly run will send anyway. Suppressing the command
/// is deliberately *per-run* and never sticky: the configuration is untouched,
/// so the next scheduled backup copies off-machine as configured.
///
/// `deny_unknown_fields` makes a misspelt parameter a rejection rather than a
/// silent no-op: without it `POST /jobs/backup?sufix=pre-0.5.1` answered `204`
/// and took an *unlabelled* backup, so the operator's one-off label was lost
/// with nothing said (SCENARIOS T-10). The rejection is turned into a `422`
/// with the reason by [`super::http`]'s trigger handler.
#[derive(Debug, Default, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct JobParams {
    pub suffix: Option<String>,
    #[serde(default, deserialize_with = "flag")]
    pub skip_command: bool,
}

/// Read a query-string boolean flag, accepting the bare `?skip_command` form
/// (an empty value) as `true` alongside the spelt-out `=true` / `=false`.
///
/// serde's own `bool` would reject the bare form — the same silent-loss shape
/// as the misspelt `suffix` above, only louder: a `422` for what every other
/// tool on the box treats as *the* way to write a flag. Anything that is
/// neither is still an error, so a typo can never read as "off" by accident.
fn flag<'de, D: serde::Deserializer<'de>>(deserializer: D) -> Result<bool, D::Error> {
    let raw = String::deserialize(deserializer)?;
    match raw.to_ascii_lowercase().as_str() {
        "" | "true" | "1" | "yes" | "on" => Ok(true),
        "false" | "0" | "no" | "off" => Ok(false),
        other => Err(serde::de::Error::custom(format!(
            "'{other}' is not a flag value (use true or false)"
        ))),
    }
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
        Fut: Future<Output = JobOutcome> + Send + 'static,
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
    Fut: Future<Output = JobOutcome> + Send + 'static,
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
    Fut: Future<Output = JobOutcome> + Send + 'static,
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
    distribution_fetcher: crate::entities::distribution_event::SharedDistributionFetcher,
) -> JobRegistry {
    let mut jobs: HashMap<String, Arc<RegisteredJob>> = HashMap::new();

    register(&mut jobs, "backup", {
        let pool = pool.clone();
        move |params: JobParams| {
            let (pool, db_path) = (pool.clone(), db_path.clone());
            let (backup_dir, backup_command) = (backup_dir.clone(), backup_command.clone());
            async move {
                // `?skip_command=true` drops the hook for this run only, by
                // handing `backup` the same `None` a host with no command
                // configured gets — so the skip cannot half-apply (the command
                // never runs, rather than running against nothing) and the
                // stored configuration is untouched.
                let skipped = params.skip_command && backup_command.is_some();
                if skipped {
                    tracing::info!("post-backup command skipped at the caller's request");
                }
                let backup_command = backup_command.filter(|_| !params.skip_command);
                crate::infra::db::backup(
                    &pool,
                    &db_path,
                    backup_dir.as_deref(),
                    backup_command.as_deref(),
                    params.suffix.as_deref(),
                )
                .await
                .map_err(|e| e.to_string())?;
                // A run that took the backup but deliberately did not copy it
                // off-machine is a success that did less than the whole of the
                // job's work — exactly what the note is for, so the Jobs screen
                // says so rather than showing an unqualified `ok` (SCENARIOS
                // T-09).
                Ok(skipped
                    .then(|| "post-backup command skipped at the caller's request".to_string()))
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
                Ok(None)
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
                // Per feed, not one total: an import that fetched only the free
                // ISO 4217 half is the ordinary state of a server without DTIF
                // credentials, and used to log a bare `imported = 178` that read
                // exactly like a complete run (SCENARIOS T-09).
                tracing::info!(
                    fiat = ?summary.fiat,
                    tokens = ?summary.tokens,
                    skipped = ?summary.skipped,
                    "currency import complete"
                );
                // A skipped feed qualifies the run's success, so the Jobs screen
                // shows a half-import as one instead of a clean `ok`.
                Ok(summary.incomplete_note())
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
                .map(|()| None)
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
            async move {
                crate::entities::closing_price::run_rebase(&pool)
                    .await
                    .map(|()| None)
            }
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
            async move {
                crate::entities::trade::run_recompute(&pool)
                    .await
                    .map(|()| None)
            }
        }
    });

    register(&mut jobs, "distribution-import", {
        let pool = pool.clone();
        move |_| {
            let (pool, fetcher) = (pool.clone(), distribution_fetcher.clone());
            async move {
                // Returns the run's note directly: a run that could not place
                // some provider event on its market's calendar succeeded while
                // doing less than the whole of its work, which is what the note
                // is for (SCENARIOS T-09).
                crate::entities::distribution_event::run_refresh(
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
            async move {
                crate::reports::snapshot::run_snapshot_job(&pool, chrono::Utc::now())
                    .await
                    .map(|()| None)
            }
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
                Ok(None) => Ok(None),
                Ok(Some(t)) if t.blocked.is_empty() => Ok(None),
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
