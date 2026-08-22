//! The declarative schedule file: parsing `schedule.cron` into entries and
//! spawning one background timer task per entry.
//!
//! Lines are `<min> <hour> <dom> <mon> <dow> [timezone] <job-name>`; the
//! optional timezone is an IANA name pinning the cron expression to that zone.

use super::db::db_clear_schedule;
use super::registry::{JobRegistry, JobTrigger};
use super::run::run_entry;
use chrono::{Local, Utc};
use chrono_tz::Tz;
use croner::Cron;
use sqlx::SqlitePool;
use std::str::FromStr;

/// Why a schedule file was rejected. Carries the 1-based line number so a bad
/// `schedule.cron` is easy to fix.
#[derive(thiserror::Error, Debug)]
pub enum ScheduleError {
    /// A line was malformed: too few fields, or an unparseable cron expression.
    #[error("schedule line {line}: {msg}")]
    Parse { line: usize, msg: String },
    /// A line referenced a job name that is not in the registry.
    #[error("schedule line {line}: no such job {name:?}")]
    UnknownJob { line: usize, name: String },
}

/// One parsed schedule line: when to fire, in which timezone (`None` = the
/// server's local timezone), and which registered job to run. `line` is the
/// 1-based line number in the schedule file (comments and blank lines count),
/// so validation errors point at the real line.
///
/// `expr` is the cron expression as *written*, kept beside the parsed [`Cron`]
/// because that is what the stored schedule (`job_schedule`, migration 0043)
/// and the Jobs screen show an operator — `Cron` can compute the next
/// occurrence but cannot say what the line said.
#[derive(Debug)]
pub(super) struct ScheduleEntry {
    pub(super) line: usize,
    pub(super) cron: Cron,
    pub(super) expr: String,
    pub(super) tz: Option<Tz>,
    pub(super) name: String,
}

/// Parse a cron schedule file. Lines are
/// `<min> <hour> <dom> <mon> <dow> [timezone] <job-name>`; the optional
/// timezone is an IANA name (e.g. `America/New_York`) pinning the cron
/// expression to that zone — absent, the expression is server-local time.
/// `#` starts a comment and blank lines are ignored.
pub(super) fn parse(schedule: &str) -> Result<Vec<ScheduleEntry>, ScheduleError> {
    let mut entries = Vec::new();

    for (idx, raw) in schedule.lines().enumerate() {
        let line = idx + 1;
        let content = raw.split('#').next().unwrap_or("").trim();
        if content.is_empty() {
            continue;
        }

        let fields: Vec<&str> = content.split_whitespace().collect();
        let (tz, name) = match fields.len() {
            6 => (None, fields[5]),
            7 => {
                let tz = fields[5].parse::<Tz>().map_err(|_| ScheduleError::Parse {
                    line,
                    msg: format!(
                        "unknown timezone {:?} (expected an IANA name like Australia/Sydney)",
                        fields[5]
                    ),
                })?;
                (Some(tz), fields[6])
            }
            n => {
                return Err(ScheduleError::Parse {
                    line,
                    msg: format!(
                        "expected 5 cron fields, an optional IANA timezone, \
                         and a job name, got {n} field(s)"
                    ),
                });
            }
        };

        let expr = fields[..5].join(" ");
        let cron = Cron::from_str(&expr).map_err(|e| ScheduleError::Parse {
            line,
            msg: format!("invalid cron {expr:?}: {e}"),
        })?;
        entries.push(ScheduleEntry {
            line,
            cron,
            expr,
            tz,
            name: name.to_string(),
        });
    }

    Ok(entries)
}

/// Parse the schedule, validate every entry against the registry, and spawn one
/// background task per entry. Returns an error (without spawning anything) if
/// the schedule is malformed or names an unregistered job.
///
/// The stored schedule (`job_schedule`, migration 0043) is **cleared here**,
/// before any entry task is spawned, and rebuilt by the tasks themselves: the
/// table describes the schedule *this* process is running, so an entry removed
/// from `schedule.cron` — or a job removed from the registry — leaves no row
/// behind to be reported overdue for ever after. That case is a lost line, and
/// the startup WARN below is what reports it (SCENARIOS T-09/schedule); a
/// permanent alarm nobody could clear is the failure mode this project has had
/// to undo more than once. Async for that one write: clearing must complete
/// before the tasks start claiming rows, or a task's row could be deleted the
/// instant after it was written. A failure to clear is logged and startup
/// continues — the schedule itself still runs; only its stored shadow is off.
pub async fn spawn(
    registry: JobRegistry,
    pool: SqlitePool,
    schedule: &str,
) -> Result<(), ScheduleError> {
    let entries = parse(schedule)?;

    // Validate all names up front so a bad file fails fast at startup rather
    // than spawning a partial set of tasks.
    for entry in &entries {
        if !registry.contains_key(&entry.name) {
            return Err(ScheduleError::UnknownJob {
                line: entry.line,
                name: entry.name.clone(),
            });
        }
    }

    // A *scheduled* job with no schedule line never runs automatically (only via
    // POST /jobs/{name}). That is an oversight — a lost line — so warn rather
    // than fail. A job registered as `ManualOnly` (a one-off repair, registered
    // with `register_manual`) is deliberately schedule-less and says nothing:
    // warning about it every startup would bury the line above, which is the
    // only one that ever needs acting on (SCENARIOS T-09/schedule).
    for (name, job) in registry.iter() {
        if job.trigger == JobTrigger::ManualOnly {
            continue;
        }
        if !entries.iter().any(|entry| &entry.name == name) {
            tracing::warn!(
                job = %name,
                "registered job has no schedule entry; it will only run via POST /jobs/{name}"
            );
        }
    }

    if let Err(e) = db_clear_schedule(&pool).await {
        tracing::warn!("could not clear the stored job schedule: {e}");
    }

    for entry in entries {
        let job = registry[&entry.name].clone();
        let pool = pool.clone();
        // The two arms differ only in the timezone type the clock yields
        // (`DateTime<Tz>` vs `DateTime<Local>`), so each instantiates the same
        // generic loop.
        match entry.tz {
            Some(tz) => {
                tokio::spawn(run_entry(pool, entry, job, move || {
                    Utc::now().with_timezone(&tz)
                }));
            }
            None => {
                tokio::spawn(run_entry(pool, entry, job, Local::now));
            }
        }
    }

    Ok(())
}
