//! The declarative schedule file: parsing `schedule.cron` into entries and
//! spawning one background timer task per entry.
//!
//! Lines are `<min> <hour> <dom> <mon> <dow> [timezone] <job-name>`; the
//! optional timezone is an IANA name pinning the cron expression to that zone.

use super::registry::JobRegistry;
use super::run::run_entry;
use chrono::{Local, Utc};
use chrono_tz::Tz;
use croner::Cron;
use sqlx::SqlitePool;
use std::str::FromStr;

/// Why a schedule file was rejected. Carries the 1-based line number so a bad
/// `schedule.cron` is easy to fix.
#[derive(Debug)]
pub enum ScheduleError {
    /// A line was malformed: too few fields, or an unparseable cron expression.
    Parse { line: usize, msg: String },
    /// A line referenced a job name that is not in the registry.
    UnknownJob { line: usize, name: String },
}

impl std::fmt::Display for ScheduleError {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        match self {
            ScheduleError::Parse { line, msg } => write!(f, "schedule line {line}: {msg}"),
            ScheduleError::UnknownJob { line, name } => {
                write!(f, "schedule line {line}: no such job {name:?}")
            }
        }
    }
}

impl std::error::Error for ScheduleError {}

/// One parsed schedule line: when to fire, in which timezone (`None` = the
/// server's local timezone), and which registered job to run. `line` is the
/// 1-based line number in the schedule file (comments and blank lines count),
/// so validation errors point at the real line.
#[derive(Debug)]
pub(super) struct ScheduleEntry {
    pub(super) line: usize,
    pub(super) cron: Cron,
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
            tz,
            name: name.to_string(),
        });
    }

    Ok(entries)
}

/// Parse the schedule, validate every entry against the registry, and spawn one
/// background task per entry. Returns an error (without spawning anything) if
/// the schedule is malformed or names an unregistered job.
pub fn spawn(registry: JobRegistry, pool: SqlitePool, schedule: &str) -> Result<(), ScheduleError> {
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

    // A registered job with no schedule line never runs automatically (only via
    // POST /jobs/{name}). That is usually an oversight, so warn rather than fail.
    for name in registry.keys() {
        if !entries.iter().any(|entry| &entry.name == name) {
            tracing::warn!(
                job = %name,
                "registered job has no schedule entry; it will only run via POST /jobs/{name}"
            );
        }
    }

    for entry in entries {
        let job = registry[&entry.name].clone();
        let pool = pool.clone();
        // The two arms differ only in the timezone type the clock yields
        // (`DateTime<Tz>` vs `DateTime<Local>`), so each instantiates the same
        // generic loop.
        match entry.tz {
            Some(tz) => {
                tokio::spawn(run_entry(pool, entry.name, job, entry.cron, move || {
                    Utc::now().with_timezone(&tz)
                }));
            }
            None => {
                tokio::spawn(run_entry(pool, entry.name, job, entry.cron, Local::now));
            }
        }
    }

    Ok(())
}
