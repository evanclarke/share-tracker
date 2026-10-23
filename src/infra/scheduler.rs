//! Cron-driven scheduler for recurring maintenance jobs.
//!
//! Job *schedules* live in a declarative cron file (5-field Vixie cron:
//! `min hour dom mon dow`, optionally followed by an IANA timezone before the
//! job name) rather than in code — see `schedule.cron`. This module owns a
//! registry mapping a job name to the work it performs, parses a schedule, and
//! spawns one background task per scheduled entry. Jobs fire only at their cron
//! times (no run-on-startup); any job can be run on demand via
//! `POST /jobs/{name}`.
//!
//! Each spawned task logs the next scheduled run at INFO after every run (and at
//! startup), so the live schedule is verifiable from logs without reading code.
//!
//! Split into focused submodules, all re-exported here so `scheduler::X` paths
//! are unchanged for callers:
//! - [`registry`] — what each job name does, and the job/lock types
//! - [`schedule`] — parsing `schedule.cron` and spawning a task per entry
//! - [`run`] — the logged, lock-serialised single run and the per-entry timer loop
//! - [`db`] — the bounded `job_runs` history and the stored `job_schedule`
//! - [`http`] — `GET /jobs` and `POST /jobs/{name}`

mod db;
mod http;
mod registry;
mod run;
mod schedule;

pub use http::router;
/// What one run of a job returns. Named outside the scheduler by the two email
/// jobs, whose bodies live with the reports they compose
/// (`reports::weekly_summary`, `entities::price_alert`) rather than inline in
/// the registry — a job whose work is a whole report is too big to be an
/// argument to `register`.
pub use registry::{JobOutcome, JobRegistry, registry};
pub use schedule::spawn;

// Reached only by the tests (`tests/`); not part of the module's surface in
// the non-test build, so gated to keep that build warning-free.
#[cfg(test)]
pub use db::JOB_RUN_HISTORY_LIMIT;
#[cfg(test)]
pub use db::JobRunStatus;
#[cfg(test)]
use db::{db_record_run, db_run_histories, db_start_run};
pub use http::JobStatus;
#[cfg(test)]
pub use registry::{JobTrigger, RegisteredJob};
#[cfg(test)]
use run::{next_run, run_entry, run_job};
#[cfg(test)]
pub use schedule::ScheduleError;
#[cfg(test)]
use schedule::{ScheduleEntry, parse};

#[cfg(test)]
mod tests;
