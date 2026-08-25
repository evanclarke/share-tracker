use chrono::{DateTime, Local, NaiveDateTime};
use sqlx::{
    Connection, Sqlite, SqlitePool, Transaction,
    sqlite::{SqliteConnectOptions, SqliteConnection, SqliteJournalMode},
};
use std::{
    collections::HashSet,
    path::{Path, PathBuf},
    str::FromStr,
    time::Duration,
};

/// How long a queued writer waits for SQLite's single write lock before it
/// gives up and the request fails.
///
/// **Chosen, not inherited.** sqlx's default is 5 seconds
/// (`SqliteConnectOptions::default()`, `sqlx-sqlite` 0.9), applied by
/// `sqlite3_busy_timeout()` at connect — a number nobody here picked, and one
/// the application can now outlast. [`write_tx`]'s whole argument is that a
/// concurrent writer *waits its turn* rather than failing; a timeout shorter
/// than the longest write transaction the application can produce silently
/// takes that promise back and answers the waiter "database is locked"
/// instead (SCENARIOS X-b).
///
/// **Sized above the longest single write transaction, measured.** The two
/// longest are the whole-holding rollover operations (scrip exchange,
/// demerger, transfer), which walk every open parcel and INSERT a replacement
/// for each in one transaction, and report-snapshot generation — which since
/// SCENARIOS X-a takes every one of its input reads *inside* the transaction
/// that stores it, so the lock is held for the whole run. Both scale linearly
/// with the number of open parcels. Measured at the HTTP surface on throwaway
/// databases of one-unit Buy parcels (debug build, 2026-08-23):
///
/// | open parcels | scrip exchange | snapshot generation |
/// |--------------|---------------:|--------------------:|
/// | 30,000       |         4.80 s |     0.55 s – 6.53 s |
/// | 60,000       |         9.42 s |              1.10 s |
///
/// The rollover is the steadier of the two: ~157 µs per parcel, measured on
/// two separately built databases that agree to 2%. **Generation is the wilder
/// one, and it is what binds this number.** Its per-parcel cost spans 18 µs
/// (one database grown from 30,000 parcels to 60,000) to 77 µs (a differently
/// built database of the same size) to **218 µs** — 6.53 s over 30,000
/// parcels, reproduced three times on the database SCENARIOS X-b was found on,
/// which carries a second listing's parcels, ten days of prices, and the
/// income rows the performance half walks. A twelvefold spread at one parcel
/// count is the fact to carry: parcels alone do not predict the cost, so the
/// bound has to be taken from the worst rate seen, not the typical one. At
/// 218 µs, **30 seconds covers a generation of roughly 138,000 open parcels**
/// (and a rollover of roughly 190,000) — four times the largest database
/// either was measured on, and three orders of magnitude past Evan's real one,
/// which generates a snapshot in ~41 ms (SCENARIOS X-a). It is deliberately
/// not larger: a genuinely stuck writer (another process holding the lock, a
/// hung transaction) has to *report*, and a request that never returns is
/// worse than one that returns 503.
///
/// **It bounds one transaction, not a loop of them.** A bulk regeneration
/// (`POST /report_snapshots/regenerate_all`) opens a fresh write transaction
/// for the next date the moment it commits the last, and SQLite's busy handler
/// is not a queue: a waiter that has been waiting a while only re-tries every
/// 100 ms, so it loses the microsecond-wide gap between dates to the loop's
/// own already-awake thread, every time. Measured: a write stream beside a
/// 15-date regeneration over 60,000 parcels (70 s) had **every** write in that
/// window fail — 13 in a row, each after its full busy timeout, across 13
/// lock releases it never won (SCENARIOS X-b). No timeout worth choosing
/// survives that, so it is deliberately not chased here: the honest answer to
/// a bulk repair holding the lock is the `503` below, and `docs/API.md` says
/// so. The unattended bulk path (`regenerate_provisional`, inside the weekly
/// `rba-fx-import` job) is bounded to the provisional window — at most a
/// couple of months of dates — while the unbounded one is only ever started
/// by an operator who is watching it.
///
/// **It does not rescue a deferred `BEGIN`.** `sqlite3_busy_timeout()` never
/// retries a read-to-write upgrade — see [`write_tx`] — so no value here helps
/// a transaction begun with `pool.begin()`; that is why every write path uses
/// [`write_tx`]. [`tests::a_deferred_transaction_cannot_upgrade_after_a_concurrent_write`]
/// pins it, and stays fast because the failure is immediate at any timeout.
///
/// When the wait *does* expire the request no longer dies as an empty `500`:
/// `infra::http::ApiError`'s `From<sqlx::Error>` classifies the whole
/// `SQLITE_BUSY` family as a `503` saying the write can be sent again.
pub const BUSY_TIMEOUT: Duration = Duration::from_secs(30);

/// How every pool in this application connects: create-if-missing, foreign keys
/// enforced, an explicit [`BUSY_TIMEOUT`], and WAL for a file database (an
/// in-memory one has no journal to configure). Factored out of [`init`] only so
/// the test harness's cached-schema pool (`test_support::test_pool`) can open a
/// database on *exactly* these options and differ from production in nothing but
/// how the schema gets there.
fn connect_options(db_path: &str) -> Result<SqliteConnectOptions, sqlx::Error> {
    let url = if db_path == ":memory:" {
        "sqlite::memory:".to_string()
    } else {
        format!("sqlite:{db_path}")
    };

    let mut opts = SqliteConnectOptions::from_str(&url)?
        .create_if_missing(true)
        .foreign_keys(true)
        .busy_timeout(BUSY_TIMEOUT);

    if db_path != ":memory:" {
        opts = opts.journal_mode(SqliteJournalMode::Wal);
    }
    Ok(opts)
}

/// A pool on `db_path` with **no migrations run** — the connection half of
/// [`init`] on its own. Test-only: it exists for `test_support::test_pool`,
/// which replays a captured schema instead of the 45 migration files. Nothing
/// on the production path may use it; [`init`] is the only way a real database
/// is opened, and it always migrates.
#[cfg(test)]
pub async fn unmigrated_pool(db_path: &str) -> Result<SqlitePool, sqlx::Error> {
    SqlitePool::connect_with(connect_options(db_path)?).await
}

/// Begin a transaction that is going to **write**: `BEGIN IMMEDIATE`, which
/// takes SQLite's write lock at the `BEGIN` instead of at the first write.
///
/// Every write path starts here, and none of them may use `pool.begin()`.
/// `pool.begin()` issues a *deferred* `BEGIN`: the transaction starts as a
/// reader and tries to upgrade when it first writes. If another connection has
/// written in the meantime the upgrade fails **immediately** — `SQLITE_BUSY`
/// (5) against a held write lock, or `SQLITE_BUSY_SNAPSHOT` (517) when that
/// other connection has already committed past our read snapshot — and
/// `sqlite3_busy_timeout()` deliberately retries neither (waiting on an
/// upgrade could deadlock two readers that both want to write). So the busy
/// timeout does not cover it — not sqlx's 5-second default, and not
/// [`BUSY_TIMEOUT`] either — and the request dies at once with
/// "database is locked". That is what a `PUT` issued while the scheduler was
/// writing its startup `job_schedule` rows hit — 2 failures in 160 startups,
/// measured, and how CI first found it (a `ui-smoke.sh` fixture seed).
///
/// `BEGIN IMMEDIATE` puts the transaction in the writer queue from the start,
/// where the busy timeout *does* apply, so a concurrent writer waits its turn
/// rather than failing. The cost is that write transactions serialise against
/// each other, which they already did — SQLite has one writer at a time. That
/// promise is only as good as how long the waiter is allowed to wait, which is
/// why [`BUSY_TIMEOUT`] is chosen against a measurement of the longest write
/// transaction here rather than left at sqlx's 5-second default: past it, the
/// waiter fails after all (SCENARIOS X-b), now as a `503` naming the reason
/// rather than an empty `500`.
///
/// Read-only transactions stay on `pool.begin()` deliberately: they never
/// upgrade, so they cannot hit either error, and making them immediate would
/// serialise every report against every other for no reason. The split is
/// pinned by [`tests::write_side_modules_never_begin_a_deferred_transaction`],
/// whose allowlist names the read-only report files one by one.
///
/// Top level only. sqlx issues a `SAVEPOINT` for a nested transaction and
/// rejects a custom `BEGIN` statement there (`Error::InvalidSavePointStatement`),
/// so a transaction opened on an existing `&mut *tx` keeps using `.begin()`.
pub async fn write_tx(pool: &SqlitePool) -> Result<Transaction<'static, Sqlite>, sqlx::Error> {
    pool.begin_with("BEGIN IMMEDIATE").await
}

pub async fn init(db_path: &str) -> Result<SqlitePool, sqlx::Error> {
    let pool = SqlitePool::connect_with(connect_options(db_path)?).await?;

    let applied_before: Vec<i64> =
        sqlx::query_scalar("SELECT version FROM _sqlx_migrations WHERE success = TRUE")
            .fetch_all(&pool)
            .await
            .unwrap_or_default();

    sqlx::migrate!().run(&pool).await?;

    let applied_after: Vec<(i64, String)> = sqlx::query_as(
        "SELECT version, description FROM _sqlx_migrations WHERE success = TRUE ORDER BY version",
    )
    .fetch_all(&pool)
    .await?;

    let new_count = applied_after
        .iter()
        .filter(|(v, _)| !applied_before.contains(v))
        .inspect(|(version, description)| {
            tracing::info!(version, description, "migration applied");
        })
        .count();

    if new_count == 0 {
        tracing::debug!("no new migrations");
    }

    Ok(pool)
}

/// Destination filename for a backup taken now:
/// `<file>-YYYY-MM-DD-HHMMSS[-suffix].db`, placed in `backup_dir` when
/// configured (so backups can land on another volume) or beside the database
/// file otherwise. The time component (down to the second) keeps each weekly
/// backup distinct — the backup job runs weekly, so a date-only name would
/// collide across runs. `suffix` (validated by [`validate_backup_suffix`])
/// labels a one-off backup with why it was taken, e.g. `pre-0.5.1` for an
/// update.sh pre-upgrade snapshot — it does not change how the file is found
/// for pruning (see `backup_timestamp`).
pub fn backup_path(db_path: &str, backup_dir: Option<&str>, suffix: Option<&str>) -> String {
    backup_path_at(db_path, backup_dir, suffix, Local::now())
}

fn backup_path_at(
    db_path: &str,
    backup_dir: Option<&str>,
    suffix: Option<&str>,
    at: DateTime<Local>,
) -> String {
    let ts = at.format("%Y-%m-%d-%H%M%S");
    let suffix_part = suffix.map(|s| format!("-{s}")).unwrap_or_default();
    let stem = db_path.strip_suffix(".db").unwrap_or(db_path);
    match backup_dir {
        None => format!("{stem}-{ts}{suffix_part}.db"),
        Some(dir) => {
            // Only the filename moves to the configured dir; the db's own
            // directory component is dropped.
            let name = Path::new(stem)
                .file_name()
                .map(|f| f.to_string_lossy().into_owned())
                .unwrap_or_else(|| stem.to_string());
            Path::new(dir)
                .join(format!("{name}-{ts}{suffix_part}.db"))
                .to_string_lossy()
                .into_owned()
        }
    }
}

/// Longest accepted `suffix` for a one-off backup (see [`backup`]). The value
/// lands directly in a filename, so it is kept short and self-documenting
/// (e.g. `pre-0.5.1`) rather than free text.
pub const MAX_BACKUP_SUFFIX_LEN: usize = 40;

/// Validate a caller-supplied backup filename suffix. The value is appended
/// to the backup filename as `-<suffix>.db` (see `backup_path`), so this is
/// the one gate standing between an HTTP query param and the filesystem:
/// characters are limited to ASCII alphanumerics, `.`, `_`, and `-`, which
/// makes `/`, `..`, NUL, and path separators structurally impossible, and the
/// suffix must not itself start with `-` or `.` (keeps the joining hyphen
/// unambiguous and rules out a leading-dot hidden file). Returns the reason on
/// failure, suitable for a `422` response body.
pub fn validate_backup_suffix(suffix: &str) -> Result<(), String> {
    if suffix.is_empty() {
        return Err("suffix must not be empty".to_string());
    }
    if suffix.len() > MAX_BACKUP_SUFFIX_LEN {
        return Err(format!(
            "suffix must be at most {MAX_BACKUP_SUFFIX_LEN} characters"
        ));
    }
    if !suffix
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
    {
        return Err("suffix may only contain ASCII letters, digits, '.', '_', and '-'".to_string());
    }
    if suffix.starts_with('-') || suffix.starts_with('.') {
        return Err("suffix must not start with '-' or '.'".to_string());
    }
    Ok(())
}

/// Why a backup run failed. `Verification` marks a produced file that is not a
/// restorable copy of the live database — the file has been quarantined
/// (renamed `<name>.bad`) so it can never be mistaken for a good backup.
/// `Command` marks a configured `backup_command` that failed to run or exited
/// non-zero; the backup itself is already complete and verified by that point.
#[derive(thiserror::Error, Debug)]
pub enum BackupError {
    #[error("backup failed: {0}")]
    Db(#[from] sqlx::Error),
    #[error("backup failed: {0}")]
    Io(#[from] std::io::Error),
    #[error("backup verification failed for {path}: {reason}")]
    Verification { path: String, reason: String },
    #[error("post-backup command failed ({command}): {reason}")]
    Command { command: String, reason: String },
    #[error("invalid backup suffix '{suffix}': {reason}")]
    InvalidSuffix { suffix: String, reason: String },
}

pub async fn backup(
    pool: &SqlitePool,
    db_path: &str,
    backup_dir: Option<&str>,
    backup_command: Option<&str>,
    suffix: Option<&str>,
) -> Result<(), BackupError> {
    // Validated before any filesystem work, even though the HTTP handler
    // validates too (for a clean 422) — backup() is pub and must not be
    // bypassable by a caller that skips the handler.
    if let Some(s) = suffix {
        validate_backup_suffix(s).map_err(|reason| BackupError::InvalidSuffix {
            suffix: s.to_string(),
            reason,
        })?;
    }
    // A configured backup dir may not exist yet (fresh volume / first run);
    // create it rather than failing the weekly job. The beside-the-DB default
    // needs no such step — the database file's directory already exists.
    if let Some(dir) = backup_dir {
        std::fs::create_dir_all(dir)?;
    }
    let dest = backup_path(db_path, backup_dir, suffix);
    let created = backup_to(pool, &dest).await?;

    // Only run the hook for a backup actually produced by this run — not one
    // skipped because a same-second file already existed (its hook, if any,
    // already ran when that file was created).
    let command_result = match (created, backup_command) {
        (true, Some(command)) => run_backup_command(command, &dest).await,
        _ => Ok(()),
    };

    // Prune only after the fresh backup verified: a failed run must never
    // shrink the set of known-good backups. Pruning runs regardless of the
    // post-backup command's outcome — the fresh backup is always within the
    // retention window, and local retention shouldn't be held hostage to an
    // offsite copy failing — but the command's error still fails the job.
    let deleted = prune_backups(db_path, backup_dir)?;
    if !deleted.is_empty() {
        tracing::info!(pruned = deleted.len(), "backup retention pruning complete");
    }
    command_result
}

/// Suffix of the staging file every backup is written to. A file under this
/// name is a copy in progress: it does not match the backup naming pattern, so
/// pruning never counts it, it can never become a monthly keeper, and nothing
/// can pick it for a restore.
const STAGING_SUFFIX: &str = ".partial";

/// Suffix a file that failed verification is quarantined under (see
/// [`verify_or_quarantine`]). Also outside the backup naming pattern.
const QUARANTINE_SUFFIX: &str = ".bad";

/// Write a backup to a specific destination, skipping if it already exists. With
/// a per-second timestamped name a collision only happens for two runs in the
/// same second, so in practice each weekly run writes a fresh file. Returns
/// whether a fresh file was written (`false` when skipped because one already
/// existed).
///
/// The copy is written to `<dest>.partial` and moved onto `dest` only once it
/// has **verified** — write, verify, rename, in that order and never any other.
/// A rename within a directory is atomic, so `dest` either does not exist or is
/// a file that passed verification in this process; a run the process does not
/// survive (a restart landing on the weekly backup's own slot, a `SIGKILL`, a
/// power cut) leaves its half-written copy under the staging name, where nothing
/// mistakes it for a backup (SCENARIOS T-11). Before this, the copy was written
/// straight to `dest` and verified afterwards, so an interrupted run left an
/// unverified file carrying a backup's exact name — counted by pruning, able to
/// become a first-of-month keeper, and indistinguishable at restore time from a
/// verified one. Whether such a file is restorable is luck; nothing checked.
async fn backup_to(pool: &SqlitePool, dest: &str) -> Result<bool, BackupError> {
    if Path::new(dest).exists() {
        tracing::debug!(path = dest, "backup already exists, skipping");
        return Ok(false);
    }
    let staging = format!("{dest}{STAGING_SUFFIX}");
    // `VACUUM INTO` refuses an existing target, so a leftover from an
    // interrupted run in this same second would otherwise fail every retry.
    // Startup sweeps these too (`sweep_partial_backups`); this covers a
    // long-running process that has not restarted since.
    if Path::new(&staging).exists() {
        tracing::info!(path = staging, "removing a leftover partial backup");
        std::fs::remove_file(&staging)?;
    }
    tracing::info!(path = staging, backup = dest, "starting backup");
    sqlx::query("VACUUM INTO ?")
        .bind(&staging)
        .execute(pool)
        .await?;
    verify_or_quarantine(pool, &staging, dest).await?;
    std::fs::rename(&staging, dest)?;
    tracing::info!(path = dest, "backup complete and verified");
    Ok(true)
}

/// Run the operator-configured `backup_command` after a fresh, verified backup,
/// substituting the literal token `{BACKUP_FILE}` with the backup's absolute
/// path — e.g. `scp {BACKUP_FILE} user@host:/backups/`. Runs via `sh -c` so the
/// configured string can use ordinary shell syntax (multiple args, pipes,
/// redirection); stdout/stderr are captured and only surfaced in logs, keeping
/// the job's own INFO/ERROR lines the single place to look.
async fn run_backup_command(command: &str, dest: &str) -> Result<(), BackupError> {
    // Absolute so the hook works regardless of the server's working directory
    // (dest may be a relative path when no --backup-dir is configured).
    let abs_dest = std::fs::canonicalize(dest)
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_else(|_| dest.to_string());
    let substituted = command.replace("{BACKUP_FILE}", &abs_dest);

    tracing::info!(command = %substituted, "running post-backup command");
    let output = tokio::process::Command::new("sh")
        .arg("-c")
        .arg(&substituted)
        .output()
        .await
        .map_err(|e| BackupError::Command {
            command: substituted.clone(),
            reason: format!("failed to spawn: {e}"),
        })?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        tracing::error!(
            command = %substituted,
            status = %output.status,
            stderr,
            "post-backup command failed"
        );
        return Err(BackupError::Command {
            command: substituted,
            reason: format!("exited with {}: {stderr}", output.status),
        });
    }
    tracing::info!(command = %substituted, "post-backup command succeeded");
    Ok(())
}

/// Check that a freshly written backup is a restorable copy: it must open, pass
/// `PRAGMA integrity_check`, and carry exactly the successful migrations the
/// live database has. A truncated or corrupted copy fails here, on the machine
/// that still has the original — not at restore time, when it may not.
async fn verify_backup(pool: &SqlitePool, dest: &str) -> Result<(), String> {
    let opts = SqliteConnectOptions::from_str(&format!("sqlite:{dest}"))
        .map_err(|e| e.to_string())?
        .read_only(true);
    let mut conn = SqliteConnection::connect_with(&opts)
        .await
        .map_err(|e| format!("cannot open backup: {e}"))?;

    let checks: Vec<String> = sqlx::query_scalar("PRAGMA integrity_check")
        .fetch_all(&mut conn)
        .await
        .map_err(|e| format!("integrity check failed to run: {e}"))?;
    if checks != ["ok"] {
        return Err(format!("integrity check failed: {}", checks.join("; ")));
    }

    let migrations = "SELECT version FROM _sqlx_migrations WHERE success = TRUE ORDER BY version";
    let live: Vec<i64> = sqlx::query_scalar(migrations)
        .fetch_all(pool)
        .await
        .map_err(|e| format!("cannot read the live database's migrations: {e}"))?;
    let backed_up: Vec<i64> = sqlx::query_scalar(migrations)
        .fetch_all(&mut conn)
        .await
        .map_err(|e| format!("migrations table check failed: {e}"))?;
    if backed_up != live {
        return Err(format!(
            "migrations incomplete: backup has {backed_up:?}, live database has {live:?}"
        ));
    }
    Ok(())
}

/// Verify the file at `produced`; on failure quarantine it by renaming to
/// `<dest>.bad` — kept for diagnosis (bounded by [`KEEP_BAD`]) but never
/// matching the backup naming pattern, so it can neither be restored by mistake
/// nor counted by pruning — log at ERROR, and fail the backup.
///
/// `produced` is the staging file the copy was written to and `dest` the
/// backup name it would have been renamed onto: the quarantined file is named
/// after the *backup*, not after the staging path, so `<name>.db.bad` reads the
/// same as it always has and says which backup failed.
async fn verify_or_quarantine(
    pool: &SqlitePool,
    produced: &str,
    dest: &str,
) -> Result<(), BackupError> {
    let Err(reason) = verify_backup(pool, produced).await else {
        return Ok(());
    };
    let quarantined = format!("{dest}{QUARANTINE_SUFFIX}");
    match std::fs::rename(produced, &quarantined) {
        Ok(()) => tracing::error!(
            path = dest,
            quarantined,
            reason,
            "backup verification failed; file quarantined"
        ),
        Err(e) => tracing::error!(
            path = dest,
            reason,
            "backup verification failed; could not quarantine the bad file: {e}"
        ),
    }
    Err(BackupError::Verification {
        path: dest.to_string(),
        reason,
    })
}

/// Retention policy: the newest `KEEP_RECENT` backups always survive, and the
/// first backup taken in each calendar month survives for the `KEEP_MONTHLY`
/// most recent months that have one (long-lived monthly keepers). With the
/// weekly schedule that is roughly two months of every backup plus a year of
/// monthlies.
const KEEP_RECENT: usize = 8;
const KEEP_MONTHLY: usize = 12;

/// How many quarantined `<name>.db.bad` files survive pruning, newest first.
/// A quarantined file is kept for diagnosis, but the likely cause of a
/// verification failure is a failing disk — and a failing disk fails every
/// weekly run, so an unbounded set left one full-size copy of the database per
/// week until the volume filled, which is the same failure the backups exist to
/// survive (SCENARIOS T-11). Three is enough to see whether the failure is
/// intermittent or permanent, and to compare two bad copies; a fourth adds
/// nothing a third has not already shown.
const KEEP_BAD: usize = 3;

/// Fixed width of the `YYYY-MM-DD-HHMMSS` timestamp component embedded in a
/// backup filename.
const BACKUP_TIMESTAMP_LEN: usize = 17;

/// The timestamp embedded in a backup filename, if `name` matches this
/// database's backup naming pattern — `<stem>-YYYY-MM-DD-HHMMSS.db` or
/// `<stem>-YYYY-MM-DD-HHMMSS-<suffix>.db` (see `backup_path`) — exactly.
/// Pruning candidates are selected by this — anything else is never touched.
/// A suffixed backup is treated as an ordinary pruning candidate: it competes
/// in the same retention policy as any other backup of this database.
///
/// The trailing `.db` is required, so a staging (`.db.partial`) or quarantined
/// (`.db.bad`) file never matches: neither is a backup, and neither may be
/// counted by retention or picked for a restore. `backup_artefact_timestamp`
/// is how those two are matched, on purpose and by their own suffix.
fn backup_timestamp(name: &str, stem: &str) -> Option<NaiveDateTime> {
    let rest = name
        .strip_prefix(stem)?
        .strip_prefix('-')?
        .strip_suffix(".db")?;
    // `get` rather than slicing/`split_at`: a non-ASCII filename must yield
    // `None` here, never panic on a non-char-boundary index.
    let ts = rest.get(..BACKUP_TIMESTAMP_LEN)?;
    let after = rest.get(BACKUP_TIMESTAMP_LEN..)?;
    if !after.is_empty() && !after.starts_with('-') {
        return None;
    }
    NaiveDateTime::parse_from_str(ts, "%Y-%m-%d-%H%M%S").ok()
}

/// The timestamp embedded in the name of a backup *artefact* — a staging
/// (`<backup>.partial`) or quarantined (`<backup>.bad`) file — if `name` is
/// one of this database's, carrying `suffix` over an otherwise well-formed
/// backup name. Matching by the artefact's own suffix keeps the sweep and the
/// `.bad` bound as narrow as pruning is: nothing else in the directory can be
/// mistaken for one.
fn backup_artefact_timestamp(name: &str, stem: &str, suffix: &str) -> Option<NaiveDateTime> {
    backup_timestamp(name.strip_suffix(suffix)?, stem)
}

/// The directory backups of `db_path` are written to: `backup_dir` when
/// configured, otherwise the database file's own directory.
fn backup_destination(db_path: &str, backup_dir: Option<&str>) -> PathBuf {
    match backup_dir {
        Some(dir) => PathBuf::from(dir),
        None => match Path::new(db_path).parent() {
            Some(parent) if !parent.as_os_str().is_empty() => parent.to_path_buf(),
            _ => PathBuf::from("."),
        },
    }
}

/// The backup filename stem for `db_path` — the filename with any `.db`
/// suffix removed, which every backup name of this database starts with.
fn backup_stem(db_path: &str) -> String {
    let stem_path = db_path.strip_suffix(".db").unwrap_or(db_path);
    Path::new(stem_path)
        .file_name()
        .map(|f| f.to_string_lossy().into_owned())
        .unwrap_or_else(|| stem_path.to_string())
}

/// Delete leftover staging files (`<backup>.partial`) of this database,
/// returning the deleted paths. A backup is written to a staging name and
/// renamed into place only after it verifies, so a file still under that name
/// is the debris of a run the process did not survive — the copy was never
/// finished and never verified, and nothing will ever complete it. Called at
/// startup, which is exactly when the interrupting restart has just happened.
///
/// Only this database's staging files are candidates: the live database, its
/// sidecars, the backups themselves, quarantined `.bad` files, another
/// database's anything, and any file whose name does not parse as
/// `<stem>-YYYY-MM-DD-HHMMSS[-suffix].db.partial` are never touched.
pub fn sweep_partial_backups(
    db_path: &str,
    backup_dir: Option<&str>,
) -> Result<Vec<PathBuf>, std::io::Error> {
    let stem = backup_stem(db_path);
    let dir = backup_destination(db_path, backup_dir);
    if !dir.is_dir() {
        // A configured backup dir the first run has not created yet.
        return Ok(Vec::new());
    }
    let mut deleted = Vec::new();
    for entry in std::fs::read_dir(&dir)? {
        let entry = entry?;
        if !entry.file_type()?.is_file() {
            continue;
        }
        let name = entry.file_name().to_string_lossy().into_owned();
        if backup_artefact_timestamp(&name, &stem, STAGING_SUFFIX).is_none() {
            continue;
        }
        let path = entry.path();
        std::fs::remove_file(&path)?;
        tracing::info!(
            path = %path.display(),
            "removed an unfinished backup left by an interrupted run"
        );
        deleted.push(path);
    }
    Ok(deleted)
}

/// Delete backups of this database that fall outside the retention policy
/// (see `KEEP_RECENT` / `KEEP_MONTHLY`), and quarantined `.bad` files beyond
/// the newest [`KEEP_BAD`], returning the deleted paths. Only regular files
/// matching this database's backup naming pattern — or that pattern plus the
/// `.bad` suffix — in the backup destination are candidates: the live database,
/// its WAL sidecars, staging `.partial` files (swept at startup instead), and
/// any other file are never deleted.
fn prune_backups(db_path: &str, backup_dir: Option<&str>) -> Result<Vec<PathBuf>, std::io::Error> {
    let stem = backup_stem(db_path);
    let dir = backup_destination(db_path, backup_dir);

    let mut backups: Vec<(NaiveDateTime, PathBuf)> = Vec::new();
    let mut quarantined: Vec<(NaiveDateTime, PathBuf)> = Vec::new();
    for entry in std::fs::read_dir(&dir)? {
        let entry = entry?;
        if !entry.file_type()?.is_file() {
            continue;
        }
        let name = entry.file_name().to_string_lossy().into_owned();
        if let Some(ts) = backup_timestamp(&name, &stem) {
            backups.push((ts, entry.path()));
        } else if let Some(ts) = backup_artefact_timestamp(&name, &stem, QUARANTINE_SUFFIX) {
            quarantined.push((ts, entry.path()));
        }
    }
    backups.sort_by_key(|(ts, _)| std::cmp::Reverse(*ts)); // newest first
    quarantined.sort_by_key(|(ts, _)| std::cmp::Reverse(*ts)); // newest first

    let mut keep: HashSet<&Path> = backups
        .iter()
        .take(KEEP_RECENT)
        .map(|(_, path)| path.as_path())
        .collect();

    // Monthly keepers: the *first* backup of a month, so a keeper is stable —
    // later runs in the same month never displace it. Sorted newest-first the
    // months come out grouped, so dedup yields the distinct months in order.
    let mut months: Vec<String> = backups
        .iter()
        .map(|(ts, _)| ts.format("%Y-%m").to_string())
        .collect();
    months.dedup();
    months.truncate(KEEP_MONTHLY);
    for month in &months {
        // newest-first, so the rearmost match = oldest in the month
        let first_of_month = backups
            .iter()
            .rfind(|(ts, _)| ts.format("%Y-%m").to_string() == *month);
        if let Some((_, path)) = first_of_month {
            keep.insert(path.as_path());
        }
    }

    let mut deleted = Vec::new();
    for (_, path) in &backups {
        if keep.contains(path.as_path()) {
            continue;
        }
        std::fs::remove_file(path)?;
        tracing::info!(path = %path.display(), "pruned backup outside retention policy");
        deleted.push(path.clone());
    }
    // The quarantined set has its own, much smaller bound (see `KEEP_BAD`):
    // these are not backups and never age into a monthly keeper, so newest-few
    // is the whole policy.
    for (_, path) in quarantined.iter().skip(KEEP_BAD) {
        std::fs::remove_file(path)?;
        tracing::info!(path = %path.display(), "pruned quarantined backup beyond the newest few");
        deleted.push(path.clone());
    }
    Ok(deleted)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Files that may begin a **deferred** transaction (`pool.begin()`).
    /// Everything else under `src` must go through [`write_tx`], which begins
    /// `IMMEDIATE` — see its docs for why a deferred write transaction can fail
    /// outright with "database is locked".
    ///
    /// The read-only reports are the bulk of the list: a report reads all its
    /// inputs on one transaction for a consistent snapshot and never writes, so
    /// it can never hit the failed upgrade `write_tx` exists to avoid, and
    /// taking the write lock up front would serialise every report against
    /// every other for nothing. They are listed one file at a time, not as
    /// `src/reports/`, so that a *new* report is an offender until someone
    /// decides which side of the split it is on: `reports/snapshot.rs` is the
    /// one report that **writes** (it persists the price-dependent reports to
    /// `report_snapshots`) and is deliberately not here.
    const DEFERRED_BEGIN_ALLOWED: &[&str] = &[
        // This module's own tests drive a deferred BEGIN deliberately, to pin
        // the failure `write_tx` exists to avoid.
        "infra/db.rs",
        // Read-only reports, one transaction each, no writes.
        "reports/activity.rs",
        "reports/amit_adjustment_cross_check.rs",
        "reports/amit_cash_cross_check.rs",
        "reports/e4_cross_check.rs",
        "reports/franking.rs",
        "reports/franking_at_risk.rs",
        "reports/fx_coverage.rs",
        "reports/health.rs",
        "reports/indexation_cross_check.rs",
        "reports/net_capital_gain.rs",
        "reports/open_parcels.rs",
        "reports/parcel_optimiser.rs",
        "reports/performance.rs",
        "reports/period_performance.rs",
        "reports/portfolio.rs",
        "reports/realised_gains.rs",
        "reports/rollover_consistency.rs",
        "reports/row_history.rs",
        "reports/settlement_coverage.rs",
        "reports/tax_report.rs",
        "reports/tax_summary.rs",
        "reports/unrealised_gains.rs",
        "reports/wash_sales.rs",
    ];

    /// Report files that begin **no** transaction of their own, each with the
    /// reason it needs none. Everything under `src/reports/` must be either
    /// here or in [`DEFERRED_BEGIN_ALLOWED`] —
    /// [`every_report_file_is_classified_for_transaction_discipline`] fails an
    /// unclassified file, so a new report is an offender until someone decides
    /// which side it is on.
    const REPORTS_WITHOUT_A_READ_TRANSACTION: &[(&str, &str)] = &[
        (
            "reports/attachments.rs",
            "one SELECT — a single statement is its own consistent snapshot",
        ),
        (
            "reports/export.rs",
            "no database access: CSV projection and rendering helpers only",
        ),
        (
            "reports/mic_validation.rs",
            "one SELECT — a single statement is its own consistent snapshot",
        ),
        (
            "reports/mod.rs",
            "router assembly plus the two message-label helpers, one SELECT each — no \
             multi-read result to hold together",
        ),
        (
            "reports/snapshot.rs",
            "the one report that writes: its transaction is infra::db::write_tx, and its \
             reads run inside it via the _on helpers",
        ),
        (
            "reports/valuation.rs",
            "connection-taking helpers composed into the callers' own transactions \
             (snapshot generation's write_tx, period performance's read transaction); \
             the remaining pool forms acquire one connection for a single-read probe \
             (latest_snapshot_date's candidate scan) or a test",
        ),
    ];

    /// The convention `write_tx` exists to hold: no write path may begin a
    /// deferred transaction, because a deferred `BEGIN` that upgrades to a
    /// write after another connection has written fails immediately with
    /// "database is locked" — a 500 the busy timeout cannot prevent. Nothing in
    /// the type system stops a new entity from typing `pool.begin()`, so this
    /// scans for it, exactly as `decimal`'s scan pins `.bind(x.to_string())`
    /// out of the tree.
    #[test]
    fn write_side_modules_never_begin_a_deferred_transaction() {
        // Assembled so this test's own scan line is not itself a match.
        let deferred = format!(".{}()", "begin");
        let mut offenders = Vec::new();
        let mut seen_in_allowed: Vec<&str> = Vec::new();
        for (rel, body) in crate::test_support::rust_sources() {
            let allowed = DEFERRED_BEGIN_ALLOWED.contains(&rel.as_str());
            for (n, line) in body.lines().enumerate() {
                let code = line.trim_start();
                if code.starts_with("//") || !code.contains(&deferred) {
                    continue;
                }
                if allowed {
                    seen_in_allowed.push(
                        DEFERRED_BEGIN_ALLOWED
                            .iter()
                            .find(|a| **a == rel)
                            .expect("just matched"),
                    );
                } else {
                    offenders.push(format!("{rel}:{}: {code}", n + 1));
                }
            }
        }
        assert!(
            offenders.is_empty(),
            "a write transaction must be begun with infra::db::write_tx(pool) — a deferred \
             BEGIN fails with \"database is locked\" when another connection writes first. \
             If this is a read-only report, add it to DEFERRED_BEGIN_ALLOWED:\n{}",
            offenders.join("\n")
        );
        // …and the allowlist may not rot: an entry that no longer begins a
        // deferred transaction (or no longer exists) has to go, or the list
        // stops meaning anything.
        let stale: Vec<&&str> = DEFERRED_BEGIN_ALLOWED
            .iter()
            .filter(|a| !seen_in_allowed.contains(*a))
            .collect();
        assert!(
            stale.is_empty(),
            "DEFERRED_BEGIN_ALLOWED names files that no longer begin a deferred \
             transaction — drop them: {stale:?}"
        );
    }

    /// The companion the scan above needs to be airtight for reports: that
    /// scan only sees a file once it *types* `.begin()`, so a report reading
    /// straight off the pool — each query its own implicit transaction, no
    /// consistent snapshot — used to pass unclassified. That is exactly how
    /// `period_performance`'s multi-snapshot read hid (code review
    /// 2026-08-25). So the whole of `src/reports/` is classified, the way
    /// every table is classified for snapshot staleness: a report file is
    /// either in [`DEFERRED_BEGIN_ALLOWED`] — and the scan above holds it to
    /// actually beginning its read transaction — or in
    /// [`REPORTS_WITHOUT_A_READ_TRANSACTION`] with the reason it needs none.
    /// A new report in neither list fails here until someone decides.
    #[test]
    fn every_report_file_is_classified_for_transaction_discipline() {
        // Assembled so this test's own lines are not themselves matches.
        let deferred = format!(".{}()", "begin");
        let mut report_files: Vec<String> = Vec::new();
        let mut unclassified: Vec<String> = Vec::new();
        for (rel, body) in crate::test_support::rust_sources() {
            if !rel.starts_with("reports/") {
                continue;
            }
            report_files.push(rel.clone());
            let allowed = DEFERRED_BEGIN_ALLOWED.contains(&rel.as_str());
            let exempt = REPORTS_WITHOUT_A_READ_TRANSACTION
                .iter()
                .any(|(f, _)| *f == rel);
            assert!(
                !(allowed && exempt),
                "{rel} is in both DEFERRED_BEGIN_ALLOWED and \
                 REPORTS_WITHOUT_A_READ_TRANSACTION — pick the one that is true"
            );
            if !allowed && !exempt {
                unclassified.push(rel.clone());
                continue;
            }
            // An exempt entry's reason is "begins no transaction": the moment
            // the file grows one it belongs on the other list, where the scan
            // above polices it.
            if exempt {
                let begins: Vec<String> = body
                    .lines()
                    .enumerate()
                    .filter(|(_, line)| {
                        let code = line.trim_start();
                        !code.starts_with("//") && code.contains(&deferred)
                    })
                    .map(|(n, line)| format!("{rel}:{}: {}", n + 1, line.trim()))
                    .collect();
                assert!(
                    begins.is_empty(),
                    "listed in REPORTS_WITHOUT_A_READ_TRANSACTION but begins a \
                     transaction — move it to DEFERRED_BEGIN_ALLOWED:\n{}",
                    begins.join("\n")
                );
            }
        }
        assert!(
            unclassified.is_empty(),
            "every report file must hold its reads together: either it opens one \
             pool.begin() read transaction for all of them (add it to \
             DEFERRED_BEGIN_ALLOWED) or it genuinely needs none (add it to \
             REPORTS_WITHOUT_A_READ_TRANSACTION with the reason). Unclassified:\n{}",
            unclassified.join("\n")
        );
        // …and the exempt list may not rot: an entry that no longer names a
        // report file has to go, or the list stops meaning anything.
        let gone: Vec<&&str> = REPORTS_WITHOUT_A_READ_TRANSACTION
            .iter()
            .map(|(f, _)| f)
            .filter(|f| !report_files.iter().any(|r| r == **f))
            .collect();
        assert!(
            gone.is_empty(),
            "REPORTS_WITHOUT_A_READ_TRANSACTION names files that no longer exist — \
             drop them: {gone:?}"
        );
    }

    /// The premise `write_tx` rests on, as an executable fact rather than a
    /// citation: a **deferred** transaction that reads and then writes fails
    /// outright when another connection has written in between. The
    /// `busy_timeout` does not cover it at *any* value — SQLite will not retry
    /// an upgrade (two readers both waiting to write would deadlock), so the
    /// error comes back at once. This is the 500 the ui-smoke fixture seed hit.
    ///
    /// That is also why this test stayed fast when [`BUSY_TIMEOUT`] went from
    /// sqlx's 5-second default to 30 seconds: the 2-second bound below is not
    /// slack under the timeout, it is the assertion that the failure never
    /// waits for one. Were SQLite ever to start waiting, this test would fail
    /// loudly on that bound rather than quietly take 30 seconds.
    #[tokio::test]
    async fn a_deferred_transaction_cannot_upgrade_after_a_concurrent_write() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("deferred.db").to_string_lossy().to_string();
        let pool = init(&path).await.unwrap();

        // Deferred: this begins as a reader, and the read fixes its snapshot.
        let mut deferred = pool.begin().await.unwrap();
        let _: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM holding_accounts")
            .fetch_one(&mut *deferred)
            .await
            .unwrap();

        // Another connection writes and commits past that snapshot.
        sqlx::query("UPDATE holding_accounts SET name = 'Elsewhere' WHERE id = 1")
            .execute(&pool)
            .await
            .unwrap();

        // The upgrade. Bounded so a version of SQLite that *did* wait would
        // fail this test loudly rather than hanging it for the busy timeout.
        let attempt = sqlx::query("UPDATE holding_accounts SET name = 'Here' WHERE id = 1")
            .execute(&mut *deferred);
        let err = tokio::time::timeout(std::time::Duration::from_secs(2), attempt)
            .await
            .expect("the failed upgrade is immediate, not after the busy timeout")
            .expect_err("a deferred transaction cannot upgrade past another connection's write");

        let msg = err.to_string();
        // 5 is SQLITE_BUSY against the held write lock, 517 SQLITE_BUSY_SNAPSHOT
        // when the other connection has already committed; both are this race,
        // and both are what `write_tx` avoids.
        assert!(
            msg.contains("(code: 5)") || msg.contains("(code: 517)"),
            "expected a busy/snapshot failure, got: {msg}"
        );
        assert!(msg.contains("database is locked"), "{msg}");

        // Both codes are in the SQLITE_BUSY family, so even this one — which
        // `write_tx` is meant to make unreachable on a write path — answers the
        // bodied 503 rather than an empty 500 if it ever escapes that guard
        // (`infra::http::is_busy`).
        let api = crate::infra::http::ApiError::from(err);
        assert!(
            matches!(api, crate::infra::http::ApiError::Busy { .. }),
            "a failed upgrade is still a busy database, not an internal fault: {api:?}"
        );
    }

    /// The fix, from the other side: a transaction begun with [`write_tx`]
    /// holds the write lock from the `BEGIN`, so a concurrent writer **waits**
    /// (sqlx's busy timeout covers a queued writer) and this transaction's own
    /// write goes through instead of failing.
    ///
    /// Non-vacuous by construction: the assertion that the other writer is
    /// still blocked is exactly what a deferred `BEGIN` would fail — with one,
    /// it would sail past and this transaction's write below would be the one
    /// that returned "database is locked" (see the test above).
    #[tokio::test]
    async fn write_tx_holds_off_a_concurrent_writer_instead_of_failing_to_upgrade() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir
            .path()
            .join("immediate.db")
            .to_string_lossy()
            .to_string();
        let pool = init(&path).await.unwrap();

        // The shape every write path has: begin, read, decide, write, commit.
        let mut tx = write_tx(&pool).await.unwrap();
        let _: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM holding_accounts")
            .fetch_one(&mut *tx)
            .await
            .unwrap();

        let other = pool.clone();
        let mut concurrent = tokio::spawn(async move {
            sqlx::query("UPDATE holding_accounts SET name = 'Elsewhere' WHERE id = 1")
                .execute(&other)
                .await
        });

        let ran_anyway =
            tokio::time::timeout(std::time::Duration::from_millis(500), &mut concurrent).await;
        assert!(
            ran_anyway.is_err(),
            "the concurrent writer was not held off — this transaction did not take the \
             write lock at its BEGIN, so its own write is about to fail"
        );

        sqlx::query("UPDATE holding_accounts SET name = 'Here' WHERE id = 1")
            .execute(&mut *tx)
            .await
            .expect("the write transaction owns the write lock");
        tx.commit().await.unwrap();

        // Released: the writer that waited now goes through, rather than either
        // side having failed.
        concurrent
            .await
            .expect("the concurrent writer task")
            .expect("a queued writer waits out the busy timeout and then writes");
        let name: String = sqlx::query_scalar("SELECT name FROM holding_accounts WHERE id = 1")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(name, "Elsewhere", "both writes landed, the queued one last");
    }

    /// [`BUSY_TIMEOUT`] is actually in force on the connections [`init`] hands
    /// out — not merely written down in [`connect_options`].
    ///
    /// sqlx applies it with `sqlite3_busy_timeout()` at connect rather than as
    /// a startup `PRAGMA`, and `PRAGMA busy_timeout` reads back exactly what
    /// that call set, so this is the value a waiting writer really gets. The
    /// test exists because the failure it guards is invisible: drop the
    /// `.busy_timeout()` line and everything still passes, everything still
    /// works, and the application silently goes back to failing a concurrent
    /// write after 5 seconds (SCENARIOS X-b). Both pool kinds are checked —
    /// a file database (what `main` opens) and `:memory:` (what
    /// `test_support::test_pool` builds on), which take different branches of
    /// [`connect_options`].
    #[tokio::test]
    async fn the_chosen_busy_timeout_is_in_force_on_every_connection() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("busy.db").to_string_lossy().to_string();
        let expected = i64::try_from(BUSY_TIMEOUT.as_millis()).unwrap();
        assert_ne!(
            expected, 5_000,
            "5,000 ms is sqlx's default — BUSY_TIMEOUT must be a chosen value, \
             and this test cannot tell the two apart at that number"
        );

        for path in [file.as_str(), ":memory:"] {
            let pool = init(path).await.unwrap();
            let ms: i64 = sqlx::query_scalar("PRAGMA busy_timeout")
                .fetch_one(&pool)
                .await
                .unwrap();
            assert_eq!(
                ms, expected,
                "{path}: the busy timeout is not the chosen one — a queued writer \
                 gives up after {ms} ms instead of {expected} ms"
            );
        }
    }

    #[tokio::test]
    async fn init_memory_pool() {
        let pool = init(":memory:").await.unwrap();
        let row: (i64,) = sqlx::query_as("SELECT 1").fetch_one(&pool).await.unwrap();
        assert_eq!(row.0, 1);
    }

    #[tokio::test]
    async fn init_file_db() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.db").to_string_lossy().to_string();
        let pool = init(&path).await.unwrap();
        let row: (i64,) = sqlx::query_as("SELECT 1").fetch_one(&pool).await.unwrap();
        assert_eq!(row.0, 1);
    }

    #[tokio::test]
    async fn backup_creates_file() {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("test.db").to_string_lossy().to_string();
        let pool = init(&db_path).await.unwrap();

        // Capture the destination before backing up: `backup` computes its own
        // timestamp internally, so re-deriving the path afterwards could land in
        // a later second and miss the file. Drive `backup_to` with the same dest.
        let dest = backup_path(&db_path, None, None);
        backup_to(&pool, &dest).await.unwrap();

        assert!(Path::new(&dest).exists());
    }

    #[test]
    fn backup_path_includes_date_and_time() {
        use chrono::TimeZone;
        let at = Local.with_ymd_and_hms(2026, 6, 1, 14, 30, 5).unwrap();
        // Date-only naming (`-2026-06-01.db`) would collide across weekly runs;
        // the filename must carry the time component down to the second.
        assert_eq!(
            backup_path_at("share-tracker.db", None, None, at),
            "share-tracker-2026-06-01-143005.db"
        );
    }

    #[test]
    fn backup_path_honours_backup_dir() {
        use chrono::TimeZone;
        let at = Local.with_ymd_and_hms(2026, 6, 1, 14, 30, 5).unwrap();
        // With a configured dir only the filename is kept — the db's own
        // directory component must not be re-rooted under the backup dir.
        assert_eq!(
            backup_path_at("/data/share-tracker.db", Some("/mnt/backups"), None, at),
            "/mnt/backups/share-tracker-2026-06-01-143005.db"
        );
    }

    #[test]
    fn backup_path_appends_suffix() {
        use chrono::TimeZone;
        let at = Local.with_ymd_and_hms(2026, 6, 1, 14, 30, 5).unwrap();
        assert_eq!(
            backup_path_at("share-tracker.db", None, Some("pre-0.5.1"), at),
            "share-tracker-2026-06-01-143005-pre-0.5.1.db"
        );
    }

    #[test]
    fn backup_path_with_suffix_honours_backup_dir() {
        use chrono::TimeZone;
        let at = Local.with_ymd_and_hms(2026, 6, 1, 14, 30, 5).unwrap();
        assert_eq!(
            backup_path_at(
                "/data/share-tracker.db",
                Some("/mnt/backups"),
                Some("pre-0.5.1"),
                at
            ),
            "/mnt/backups/share-tracker-2026-06-01-143005-pre-0.5.1.db"
        );
    }

    #[tokio::test]
    async fn backup_with_suffix_writes_suffixed_file() {
        // A real backup() call with a suffix: the file must exist, verify, and
        // carry the suffix — the one-off pre-upgrade backup that update.sh
        // takes before `pkg add` (see pkg/freebsd/update.sh).
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("test.db").to_string_lossy().to_string();
        let pool = init(&db_path).await.unwrap();

        backup(&pool, &db_path, None, None, Some("pre-0.5.1"))
            .await
            .unwrap();

        let made_backup = std::fs::read_dir(dir.path())
            .unwrap()
            .filter_map(|e| e.ok())
            .find(|e| {
                let name = e.file_name().to_string_lossy().into_owned();
                name.starts_with("test-") && name.ends_with("-pre-0.5.1.db")
            });
        assert!(
            made_backup.is_some(),
            "expected a suffixed backup in {}",
            dir.path().display()
        );
    }

    #[tokio::test]
    async fn invalid_suffix_is_rejected() {
        // The value lands directly in a filename; every rejected shape below
        // would otherwise let a query param write outside the backup dir (or
        // collide with the naming pattern pruning relies on). No file must be
        // written for any of them.
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("test.db").to_string_lossy().to_string();
        let pool = init(&db_path).await.unwrap();

        let cases: &[&str] = &[
            "",
            "a/b",
            "..",
            "../etc",
            "-leading",
            ".leading",
            "has space",
            "emoji-\u{1F600}",
            &"x".repeat(MAX_BACKUP_SUFFIX_LEN + 1),
        ];
        for suffix in cases {
            let err = backup(&pool, &db_path, None, None, Some(suffix))
                .await
                .unwrap_err();
            assert!(
                matches!(err, BackupError::InvalidSuffix { .. }),
                "suffix {suffix:?}: expected InvalidSuffix, got {err:?}"
            );
        }
        let files: Vec<_> = std::fs::read_dir(dir.path())
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| n != "test.db" && !n.starts_with("test.db-"))
            .collect();
        assert!(
            files.is_empty(),
            "no backup file must be written for an invalid suffix, found {files:?}"
        );
    }

    #[tokio::test]
    async fn backup_lands_in_configured_dir() {
        let db_dir = tempfile::tempdir().unwrap();
        let backup_dir = tempfile::tempdir().unwrap();
        let db_path = db_dir.path().join("test.db").to_string_lossy().to_string();
        let pool = init(&db_path).await.unwrap();

        // A not-yet-existing subdirectory must be created, not fail the job.
        let dest_dir = backup_dir
            .path()
            .join("weekly")
            .to_string_lossy()
            .to_string();
        backup(&pool, &db_path, Some(&dest_dir), None, None)
            .await
            .unwrap();

        let made_backup = std::fs::read_dir(&dest_dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .any(|e| {
                let name = e.file_name().to_string_lossy().into_owned();
                name.starts_with("test-") && name.ends_with(".db")
            });
        assert!(made_backup, "expected a timestamped backup in {dest_dir}");
        // And nothing beside the database file.
        let beside_db = std::fs::read_dir(db_dir.path())
            .unwrap()
            .filter_map(|e| e.ok())
            .any(|e| e.file_name().to_string_lossy().starts_with("test-"));
        assert!(!beside_db, "backup must not also land beside the db");
    }

    #[tokio::test]
    async fn restore_round_trip_recovers_pre_mutation_state() {
        // Proves the README's documented restore procedure: back up, mutate the
        // live db, stop the server, bring the backup into service as the
        // database, restart — the pre-mutation state is back. The procedure
        // replaces the db file in place with the server process exited; an
        // in-process test can't get that (sqlx's sqlite workers tear down
        // asynchronously after `close()` and their close-time WAL checkpoint
        // races a same-path copy), so the restored copy opens at a fresh path —
        // which still proves the substance: the backup is a complete, openable
        // database holding exactly the pre-mutation state.
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("test.db").to_string_lossy().to_string();
        let pool = init(&db_path).await.unwrap();
        sqlx::query("INSERT INTO holding_accounts (id, name) VALUES (100, 'keep')")
            .execute(&pool)
            .await
            .unwrap();

        let dest = backup_path(&db_path, None, None);
        backup_to(&pool, &dest).await.unwrap();

        // Mutate after the backup: this row must be gone after the restore.
        sqlx::query("INSERT INTO holding_accounts (id, name) VALUES (101, 'lose')")
            .execute(&pool)
            .await
            .unwrap();
        pool.close().await;

        // Restore: the backup becomes the database file and the server restarts
        // (init runs migrations against it, exactly like a normal startup).
        let restored_path = dir.path().join("restored.db").to_string_lossy().to_string();
        std::fs::copy(&dest, &restored_path).unwrap();
        let restored = init(&restored_path).await.unwrap();

        let names: Vec<String> =
            sqlx::query_scalar("SELECT name FROM holding_accounts WHERE name IN ('keep', 'lose')")
                .fetch_all(&restored)
                .await
                .unwrap();
        assert!(
            names.contains(&"keep".to_string()),
            "pre-backup row survives"
        );
        assert!(
            !names.contains(&"lose".to_string()),
            "post-backup mutation must be gone after restore"
        );
    }

    /// SCENARIOS X-08: a backup taken while another connection has a write
    /// transaction **open** is a *committed* state of the database — it
    /// verifies, and it does not contain that transaction's rows. `VACUUM
    /// INTO` reads the last committed snapshot, so a weekly backup that
    /// happens to fire in the middle of a long rollover or snapshot run
    /// copies the database as it was before it, never half of it. A backup
    /// holding half a write would be the worst kind: it verifies (integrity
    /// and the migration list are both fine) and is only discovered to be
    /// inconsistent when it is restored from.
    ///
    /// The second half is the control, and without it the test is vacuous —
    /// a backup of an empty database would satisfy "does not contain the
    /// uncommitted row" just as well. Once the transaction commits, a fresh
    /// backup does contain it.
    #[tokio::test]
    async fn a_backup_taken_mid_transaction_holds_a_committed_state() {
        /// The holding-account names in a backup file, read the way a restore
        /// would read it: as its own database.
        async fn names_in(path: &str) -> Vec<String> {
            let opts = SqliteConnectOptions::from_str(&format!("sqlite:{path}"))
                .unwrap()
                .read_only(true);
            let mut conn = SqliteConnection::connect_with(&opts).await.unwrap();
            sqlx::query_scalar(
                "SELECT name FROM holding_accounts WHERE id IN (100, 101) ORDER BY id",
            )
            .fetch_all(&mut conn)
            .await
            .unwrap()
        }

        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("test.db").to_string_lossy().to_string();
        let pool = init(&db_path).await.unwrap();
        sqlx::query("INSERT INTO holding_accounts (id, name) VALUES (100, 'committed')")
            .execute(&pool)
            .await
            .unwrap();

        // A write transaction under way on another connection: its row is
        // written, its COMMIT has not happened.
        let mut tx = write_tx(&pool).await.unwrap();
        sqlx::query("INSERT INTO holding_accounts (id, name) VALUES (101, 'in flight')")
            .execute(&mut *tx)
            .await
            .unwrap();

        // `backup_to` verifies (integrity check + migration list) and
        // quarantines a bad file, so reaching `unwrap` is itself the
        // verification assertion.
        let mid = dir.path().join("mid.db").to_string_lossy().to_string();
        backup_to(&pool, &mid)
            .await
            .expect("a backup taken mid-transaction verifies");
        assert_eq!(
            names_in(&mid).await,
            vec!["committed".to_string()],
            "a backup must hold a committed state, never an open transaction's rows"
        );

        tx.commit().await.unwrap();

        let after = dir.path().join("after.db").to_string_lossy().to_string();
        backup_to(&pool, &after).await.unwrap();
        assert_eq!(
            names_in(&after).await,
            vec!["committed".to_string(), "in flight".to_string()],
            "the control: once committed, the row is in the next backup — so its \
             absence above was the open transaction, not an empty database"
        );
    }

    #[tokio::test]
    async fn migrations_apply_on_fresh_db() {
        let pool = init(":memory:").await.unwrap();
        let (count,): (i64,) = sqlx::query_as("SELECT COUNT(*) FROM _sqlx_migrations")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert!(count > 0);
    }

    #[tokio::test]
    async fn migrations_are_idempotent() {
        let pool = init(":memory:").await.unwrap();
        // Running migrate again should be a no-op, not an error
        sqlx::migrate!().run(&pool).await.unwrap();
    }

    /// A pool migrated up to (but excluding) `version`, so a single migration
    /// can then be applied to data that predates it — the only way to exercise
    /// a rebuild migration's copy step, which `init` (which applies
    /// everything) cannot reach.
    async fn pool_migrated_below(version: i64) -> SqlitePool {
        let pool = SqlitePool::connect("sqlite::memory:").await.unwrap();
        for m in sqlx::migrate!().iter().filter(|m| m.version < version) {
            sqlx::raw_sql(sqlx::AssertSqlSafe(m.sql.as_str().to_string()))
                .execute(&pool)
                .await
                .unwrap();
        }
        pool
    }

    /// Apply one migration by version to an already-seeded pool.
    async fn apply_migration(pool: &SqlitePool, version: i64) {
        let m = sqlx::migrate!()
            .iter()
            .find(|m| m.version == version)
            .unwrap_or_else(|| panic!("migration {version} exists"));
        sqlx::raw_sql(sqlx::AssertSqlSafe(m.sql.as_str().to_string()))
            .execute(pool)
            .await
            .unwrap();
    }

    /// 0020 rebuilds `closing_prices` via the rename pattern to add the
    /// manual-price columns, and a rebuild is exactly where rows go missing.
    /// Every price stored before it must survive with its value intact and be
    /// stamped as provider-fetched — the migration is applied here on top of a
    /// database seeded through 0019, not through `init`, so the copy step is
    /// really exercised.
    #[tokio::test]
    async fn migration_0020_preserves_prices_and_stamps_them_fetched() {
        let pool = pool_migrated_below(20).await;

        sqlx::query(
            "INSERT INTO listings (id, exchange_mic, ticker, name, security_type, currency) \
             VALUES (1, 'XASX', 'BHP', 'BHP Group', 'Share', 'AUD')",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO closing_prices \
                 (listing_id, price_date, price, source, fetched_at, status, error) \
             VALUES (1, '2026-06-04', '62.4899995', 'yahoo', '2026-06-04T08:00:00Z', 'ok', NULL), \
                    (1, '2026-06-05', NULL, 'yahoo', '2026-06-05T08:00:00Z', 'error', 'no candle')",
        )
        .execute(&pool)
        .await
        .unwrap();

        apply_migration(&pool, 20).await;

        #[derive(sqlx::FromRow)]
        struct Row {
            price: Option<String>,
            source: String,
            origin: String,
            sourced_from: Option<String>,
            reason: Option<String>,
        }
        let rows: Vec<Row> = sqlx::query_as(
            "SELECT price, source, origin, sourced_from, reason \
             FROM closing_prices ORDER BY price_date",
        )
        .fetch_all(&pool)
        .await
        .unwrap();
        assert_eq!(rows.len(), 2, "both rows survive the rebuild");
        assert_eq!(
            rows[0].price.as_deref(),
            Some("62.4899995"),
            "value untouched"
        );
        assert_eq!(rows[1].price, None, "the errored row keeps its null price");
        for row in &rows {
            assert_eq!(row.source, "yahoo", "the provider slot is untouched");
            assert_eq!(row.origin, "fetched", "existing rows are provider-fetched");
            assert_eq!(row.sourced_from, None);
            assert_eq!(row.reason, None);
        }

        // The staleness trigger is re-created with the table, not lost with it.
        let triggers: Vec<(String,)> = sqlx::query_as(
            "SELECT name FROM sqlite_master WHERE type = 'trigger' AND tbl_name = 'closing_prices'",
        )
        .fetch_all(&pool)
        .await
        .unwrap();
        assert_eq!(
            triggers
                .iter()
                .map(|t| t.0.as_str())
                .collect::<Vec<_>>()
                .as_slice(),
            ["closing_prices_stale_snapshots_update"]
        );
    }

    /// 0021 rebuilds `closing_prices` again (a surrogate `id` so the audit
    /// trail can key on it) *and* `row_history` (to extend its `table_name`
    /// CHECK). Two rebuilds in one migration is the riskiest shape in this
    /// schema, so this pins all of it against data that predates it: prices
    /// survive with ids assigned oldest-first, their manual provenance
    /// intact, the natural key still unique; existing audit entries survive;
    /// the trail is still append-only; and closing_prices is now audited.
    #[tokio::test]
    async fn migration_0021_adds_the_surrogate_key_and_audits_closing_prices() {
        let pool = pool_migrated_below(21).await;
        sqlx::query(
            "INSERT INTO listings (id, exchange_mic, ticker, name, security_type, currency) \
             VALUES (1, 'XASX', 'BHP', 'BHP Group', 'Share', 'AUD')",
        )
        .execute(&pool)
        .await
        .unwrap();
        // A fetched price, an errored row, and a manual price with provenance.
        sqlx::query(
            "INSERT INTO closing_prices \
                 (listing_id, price_date, price, source, fetched_at, status, error, \
                  origin, sourced_from, reason) \
             VALUES (1, '2026-06-05', '62.48', 'yahoo', '2026-06-05T08:00:00Z', 'ok', NULL, \
                     'fetched', NULL, NULL), \
                    (1, '2026-06-08', NULL, 'yahoo', '2026-06-08T08:00:00Z', 'error', 'no candle', \
                     'fetched', NULL, NULL), \
                    (1, '2026-06-04', '41.25', 'manual', '2026-06-06T01:00:00Z', 'ok', NULL, \
                     'manual', 'asx.com.au', 'delisted symbol')",
        )
        .execute(&pool)
        .await
        .unwrap();
        // An existing audit entry, from a table audited before 0021.
        sqlx::query("UPDATE listings SET name = 'BHP Group Ltd' WHERE id = 1")
            .execute(&pool)
            .await
            .unwrap();
        let history_before: i64 =
            sqlx::query_scalar("SELECT count(*) FROM row_history WHERE table_name = 'listings'")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(history_before, 1, "setup: one pre-existing audit entry");

        apply_migration(&pool, 21).await;

        // Prices survive, with ids assigned oldest price first.
        #[derive(sqlx::FromRow)]
        struct Priced {
            id: i64,
            price_date: String,
            price: Option<String>,
            origin: String,
            reason: Option<String>,
        }
        let rows: Vec<Priced> = sqlx::query_as(
            "SELECT id, price_date, price, origin, reason FROM closing_prices ORDER BY id",
        )
        .fetch_all(&pool)
        .await
        .unwrap();
        assert_eq!(rows.len(), 3, "every price survives the rebuild");
        assert_eq!(
            rows.iter()
                .map(|r| (r.id, r.price_date.as_str()))
                .collect::<Vec<_>>()
                .as_slice(),
            [(1, "2026-06-04"), (2, "2026-06-05"), (3, "2026-06-08")],
            "ids ascend with the history they describe"
        );
        assert_eq!(rows[0].price.as_deref(), Some("41.25"), "value untouched");
        assert_eq!(rows[0].origin, "manual");
        assert_eq!(
            rows[0].reason.as_deref(),
            Some("delisted symbol"),
            "manual provenance survives"
        );

        // The former primary key is still enforced, now as a UNIQUE constraint.
        let dup = sqlx::query(
            "INSERT INTO closing_prices \
                 (listing_id, price_date, price, source, fetched_at, status, origin) \
             VALUES (1, '2026-06-05', '1.00', 'yahoo', 'now', 'ok', 'fetched')",
        )
        .execute(&pool)
        .await;
        assert!(dup.is_err(), "one price per (listing, day) still holds");

        // Pre-existing audit entries survive the row_history rebuild, and the
        // trail is still append-only.
        let history_after: i64 =
            sqlx::query_scalar("SELECT count(*) FROM row_history WHERE table_name = 'listings'")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(history_after, 1, "the audit trail is not truncated");
        assert!(
            sqlx::query("DELETE FROM row_history")
                .execute(&pool)
                .await
                .is_err(),
            "the append-only guards are re-created with the table"
        );

        // closing_prices is audited from here on: revising a price records the
        // superseded row, provenance included.
        sqlx::query("UPDATE closing_prices SET price = '42.00' WHERE id = 1")
            .execute(&pool)
            .await
            .unwrap();
        let old_row: String = sqlx::query_scalar(
            "SELECT old_row FROM row_history \
             WHERE table_name = 'closing_prices' AND row_id = 1",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert!(old_row.contains("\"price\":\"41.25\""), "{old_row}");
        assert!(
            old_row.contains("\"reason\":\"delisted symbol\""),
            "{old_row}"
        );
        assert!(
            old_row.contains("\"sourced_from\":\"asx.com.au\""),
            "{old_row}"
        );
    }

    /// 0034 rebuilds `closing_prices` a third time, adding the figure each
    /// price was *observed* as so a split recorded later can restate the
    /// stored price from source (SCENARIOS Q-14). Rebuild number three on an
    /// audited table, so this pins what a rebuild can silently break: rows and
    /// their ids survive (the ids key the audit trail already recorded against
    /// them), every existing figure is stamped as its own observation, the
    /// errored row keeps a null in both columns, the new nullability CHECK
    /// holds, and both row-history triggers come back naming the new column.
    #[tokio::test]
    async fn migration_0034_keeps_prices_and_stamps_them_as_their_own_observation() {
        let pool = pool_migrated_below(34).await;
        sqlx::query(
            "INSERT INTO listings (id, exchange_mic, ticker, name, security_type, currency) \
             VALUES (1, 'XASX', 'BHP', 'BHP Group', 'Share', 'AUD')",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO closing_prices \
                 (id, listing_id, price_date, price, source, fetched_at, status, error, \
                  origin, sourced_from, reason) \
             VALUES (7, 1, '2026-06-04', '41.25', 'manual', '2026-06-06T01:00:00Z', 'ok', NULL, \
                     'manual', 'asx.com.au', 'delisted symbol'), \
                    (9, 1, '2026-06-05', '62.4899995', 'yahoo', '2026-06-05T08:00:00Z', 'ok', \
                     NULL, 'fetched', NULL, NULL), \
                    (11, 1, '2026-06-08', NULL, 'yahoo', '2026-06-08T08:00:00Z', 'error', \
                     'no candle', 'fetched', NULL, NULL)",
        )
        .execute(&pool)
        .await
        .unwrap();

        apply_migration(&pool, 34).await;

        #[derive(sqlx::FromRow)]
        struct Row {
            id: i64,
            price: Option<String>,
            price_as_observed: Option<String>,
        }
        let rows: Vec<Row> =
            sqlx::query_as("SELECT id, price, price_as_observed FROM closing_prices ORDER BY id")
                .fetch_all(&pool)
                .await
                .unwrap();
        assert_eq!(
            rows.iter().map(|r| r.id).collect::<Vec<_>>(),
            [7, 9, 11],
            "ids are carried over, so each row keeps its audit trail"
        );
        for row in &rows {
            assert_eq!(
                row.price, row.price_as_observed,
                "every stored figure to date *is* the raw observation"
            );
        }
        assert_eq!(
            rows[1].price.as_deref(),
            Some("62.4899995"),
            "value untouched"
        );
        assert_eq!(rows[2].price, None, "the errored row keeps its nulls");

        // The new column is CHECK-paired with the status, so no ok row can
        // exist without the observation a re-base must re-derive it from.
        let bad = sqlx::query(
            "INSERT INTO closing_prices \
                 (listing_id, price_date, price, source, fetched_at, status, origin) \
             VALUES (1, '2026-06-09', '1.00', 'yahoo', 'now', 'ok', 'fetched')",
        )
        .execute(&pool)
        .await;
        assert!(bad.is_err(), "an ok row without its observation is refused");

        // Both audit triggers come back naming the new column.
        sqlx::query("UPDATE closing_prices SET price = '42.00' WHERE id = 7")
            .execute(&pool)
            .await
            .unwrap();
        let old_row: String = sqlx::query_scalar(
            "SELECT old_row FROM row_history \
             WHERE table_name = 'closing_prices' AND row_id = 7",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert!(
            old_row.contains("\"price_as_observed\":\"41.25\""),
            "{old_row}"
        );
        let triggers: Vec<String> = sqlx::query_scalar(
            "SELECT name FROM sqlite_master WHERE type = 'trigger' \
             AND tbl_name = 'closing_prices' ORDER BY name",
        )
        .fetch_all(&pool)
        .await
        .unwrap();
        assert_eq!(
            triggers,
            [
                "closing_prices_row_history_delete",
                "closing_prices_row_history_update",
                "closing_prices_stale_snapshots_update"
            ]
        );
    }

    /// 0038 adds the provider symbol each fetched price was fetched under —
    /// the provenance the `symbol`-override incident had none of. Unlike
    /// 0020/0021/0034 this one is an `ALTER TABLE ADD COLUMN`, so what needs
    /// pinning is different: existing rows must be left **unrecorded** (the
    /// symbol they were fetched under is not recoverable, and a migration
    /// that guessed it from the ticker would be inventing the fact), the
    /// CHECK pairing the column with the origin must hold, and the audited
    /// table's two row-history triggers must come back naming the new column
    /// while the staleness trigger survives untouched.
    #[tokio::test]
    async fn migration_0038_leaves_existing_rows_unrecorded_and_pairs_the_symbol_with_the_origin() {
        let pool = pool_migrated_below(38).await;
        sqlx::query(
            "INSERT INTO listings (id, exchange_mic, ticker, name, security_type, currency) \
             VALUES (1, 'XNYS', 'LAC', 'Lithium Americas', 'Share', 'USD')",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO closing_prices \
                 (id, listing_id, price_date, price, price_as_observed, source, fetched_at, \
                  status, error, origin, sourced_from, reason) \
             VALUES (7, 1, '2026-06-04', '41.25', '41.25', 'manual', '2026-06-06T01:00:00Z', \
                     'ok', NULL, 'manual', 'asx.com.au', 'delisted symbol'), \
                    (9, 1, '2026-06-05', '62.48', '62.48', 'yahoo', '2026-06-05T08:00:00Z', \
                     'ok', NULL, 'fetched', NULL, NULL)",
        )
        .execute(&pool)
        .await
        .unwrap();

        apply_migration(&pool, 38).await;

        let symbols: Vec<Option<String>> =
            sqlx::query_scalar("SELECT fetched_symbol FROM closing_prices ORDER BY id")
                .fetch_all(&pool)
                .await
                .unwrap();
        assert_eq!(
            symbols,
            [None, None],
            "a pre-existing row's symbol is unrecorded, never invented"
        );

        // The CHECK is one-directional: manual implies no symbol.
        let bad = sqlx::query("UPDATE closing_prices SET fetched_symbol = 'LAAC' WHERE id = 7")
            .execute(&pool)
            .await;
        assert!(bad.is_err(), "a manual row cannot claim a fetched symbol");
        sqlx::query("UPDATE closing_prices SET fetched_symbol = 'LAAC' WHERE id = 9")
            .execute(&pool)
            .await
            .expect("a fetched row records the symbol it came from");

        // Both audit triggers come back naming the new column — and the
        // superseded value travels with the row.
        let old_row: String = sqlx::query_scalar(
            "SELECT old_row FROM row_history \
             WHERE table_name = 'closing_prices' AND row_id = 9",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert!(old_row.contains("\"fetched_symbol\":null"), "{old_row}");
        sqlx::query("DELETE FROM closing_prices WHERE id = 9")
            .execute(&pool)
            .await
            .unwrap();
        let deleted: String = sqlx::query_scalar(
            "SELECT old_row FROM row_history \
             WHERE table_name = 'closing_prices' AND row_id = 9 AND operation = 'DELETE'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert!(deleted.contains("\"fetched_symbol\":\"LAAC\""), "{deleted}");

        // ADD COLUMN leaves triggers alone, so the staleness one is still
        // there beside the two re-created ones.
        let triggers: Vec<String> = sqlx::query_scalar(
            "SELECT name FROM sqlite_master WHERE type = 'trigger' \
             AND tbl_name = 'closing_prices' ORDER BY name",
        )
        .fetch_all(&pool)
        .await
        .unwrap();
        assert_eq!(
            triggers,
            [
                "closing_prices_row_history_delete",
                "closing_prices_row_history_update",
                "closing_prices_stale_snapshots_update"
            ]
        );
    }

    /// 0039 rebuilds `exchange_holidays` to give it the surrogate `id` the
    /// audit trail keys on, then rebuilds `row_history` to admit the table —
    /// 0021's two-rebuilds-in-one-migration shape, on the one table that is
    /// pure reference data. So this pins it against a calendar that predates
    /// it: every holiday survives with its values exactly, ids ascend with
    /// the calendar, the natural key is still unique, the `mic` foreign key
    /// still bites, the three 0033 staleness triggers come back with the
    /// rebuilt table, existing audit entries survive, and a correction and a
    /// deletion now both land in the trail.
    #[tokio::test]
    async fn migration_0039_keeps_every_holiday_and_audits_the_calendar() {
        let pool = pool_migrated_below(39).await;
        // A hand-added holiday on top of the seeded calendar, and an audit
        // entry from a table audited before 0039.
        sqlx::query(
            "INSERT INTO exchange_holidays (mic, holiday_date, name) \
             VALUES ('XASX', '2028-01-03', 'New Year''s Day')",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO listings (id, exchange_mic, ticker, name, security_type, currency) \
             VALUES (1, 'XASX', 'BHP', 'BHP Group', 'Share', 'AUD')",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query("UPDATE listings SET name = 'BHP Group Ltd' WHERE id = 1")
            .execute(&pool)
            .await
            .unwrap();

        let before: Vec<(String, String, String)> = sqlx::query_as(
            "SELECT mic, holiday_date, name FROM exchange_holidays ORDER BY holiday_date, mic",
        )
        .fetch_all(&pool)
        .await
        .unwrap();
        assert!(before.len() > 100, "setup: the seeded calendar is there");

        apply_migration(&pool, 39).await;

        // Every row survives, values byte-identical, ids ascending with the
        // calendar they describe.
        let after: Vec<(i64, String, String, String)> =
            sqlx::query_as("SELECT id, mic, holiday_date, name FROM exchange_holidays ORDER BY id")
                .fetch_all(&pool)
                .await
                .unwrap();
        assert_eq!(
            after
                .iter()
                .map(|(_, m, d, n)| (m.clone(), d.clone(), n.clone()))
                .collect::<Vec<_>>(),
            before,
            "no holiday is lost or altered by the rebuild"
        );
        assert_eq!(
            after.first().map(|r| r.0),
            Some(1),
            "ids are assigned from 1, earliest holiday first"
        );
        assert_eq!(
            after.last().map(|r| (r.0, r.2.as_str())),
            Some((after.len() as i64, "2028-01-03")),
            "the latest holiday takes the highest id"
        );

        // The former primary key is still enforced, now as a UNIQUE
        // constraint, and the mic foreign key survives the rebuild.
        assert!(
            sqlx::query(
                "INSERT INTO exchange_holidays (mic, holiday_date, name) \
                 VALUES ('XASX', '2028-01-03', 'Duplicate')"
            )
            .execute(&pool)
            .await
            .is_err(),
            "one holiday per (exchange, day) still holds"
        );
        assert!(
            sqlx::query(
                "INSERT INTO exchange_holidays (mic, holiday_date, name) \
                 VALUES ('ZZZZ', '2028-01-04', 'Nowhere')"
            )
            .execute(&pool)
            .await
            .is_err(),
            "the mic foreign key survives the rebuild"
        );

        // 0033's staleness triggers came back with the table, beside the new
        // audit pair.
        let triggers: Vec<String> = sqlx::query_scalar(
            "SELECT name FROM sqlite_master WHERE type = 'trigger' \
             AND tbl_name = 'exchange_holidays' ORDER BY name",
        )
        .fetch_all(&pool)
        .await
        .unwrap();
        assert_eq!(
            triggers,
            [
                "exchange_holidays_row_history_delete",
                "exchange_holidays_row_history_update",
                "exchange_holidays_stale_snapshots_delete",
                "exchange_holidays_stale_snapshots_insert",
                "exchange_holidays_stale_snapshots_update",
            ]
        );

        // Pre-existing audit entries survive the row_history rebuild, and the
        // trail is still append-only.
        let history_after: i64 =
            sqlx::query_scalar("SELECT count(*) FROM row_history WHERE table_name = 'listings'")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(history_after, 1, "the audit trail is not truncated");
        assert!(
            sqlx::query("DELETE FROM row_history")
                .execute(&pool)
                .await
                .is_err(),
            "the append-only guards are re-created with the table"
        );

        // The calendar is audited from here on: a correction and a deletion
        // both retain the row they replaced.
        let id: i64 = sqlx::query_scalar(
            "SELECT id FROM exchange_holidays WHERE mic = 'XASX' AND holiday_date = '2028-01-03'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        sqlx::query("UPDATE exchange_holidays SET name = 'New Year (observed)' WHERE id = ?")
            .bind(id)
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("DELETE FROM exchange_holidays WHERE id = ?")
            .bind(id)
            .execute(&pool)
            .await
            .unwrap();
        let trail: Vec<(String, String)> = sqlx::query_as(
            "SELECT operation, old_row FROM row_history \
             WHERE table_name = 'exchange_holidays' AND row_id = ? ORDER BY id",
        )
        .bind(id)
        .fetch_all(&pool)
        .await
        .unwrap();
        assert_eq!(trail.len(), 2);
        assert_eq!(trail[0].0, "UPDATE");
        assert!(
            trail[0].1.contains("\"name\":\"New Year's Day\""),
            "{:?}",
            trail[0].1
        );
        assert_eq!(trail[1].0, "DELETE");
        assert!(
            trail[1].1.contains("\"holiday_date\":\"2028-01-03\"")
                && trail[1].1.contains("\"mic\":\"XASX\""),
            "{:?}",
            trail[1].1
        );
    }

    /// 0040 records what a rename overwrote (`old_name`/`old_price_symbol`,
    /// SCENARIOS R-04/R-08). Like 0038 it is an `ALTER TABLE ADD COLUMN`, so
    /// what needs pinning is the meaning it gives NULL: a rename recorded
    /// before it kept neither value, and neither is recoverable, so its row
    /// must be left **unrecorded** rather than back-filled from the listing's
    /// current one — which would make an undo "restore" a listing to what it
    /// already is. `old_name` is the marker that says a row recorded
    /// anything at all (`listings.name` is NOT NULL, so a 0040-era row always
    /// has one), and the CHECK is what makes that reading enforceable.
    #[tokio::test]
    async fn migration_0040_leaves_existing_renames_unrecorded_and_pairs_the_two_columns() {
        let pool = pool_migrated_below(40).await;
        sqlx::query(
            "INSERT INTO listings (id, exchange_mic, ticker, name, security_type, currency, \
                                   price_symbol) \
             VALUES (1, 'XASX', 'NEWER', 'Newer Co', 'Share', 'AUD', 'NEWER.AX')",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO listing_renames \
                 (id, listing_id, effective_date, old_ticker, new_ticker, \
                  old_exchange_mic, new_exchange_mic) \
             VALUES (5, 1, '2024-06-01', 'OLD', 'NEWER', 'XASX', 'XASX')",
        )
        .execute(&pool)
        .await
        .unwrap();

        apply_migration(&pool, 40).await;

        let recorded: (Option<String>, Option<String>) =
            sqlx::query_as("SELECT old_name, old_price_symbol FROM listing_renames WHERE id = 5")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(
            recorded,
            (None, None),
            "an existing rename recorded neither, and neither is invented"
        );

        // The CHECK: no recorded name means no recorded symbol either, so a
        // bare NULL old_price_symbol can never be read as "it was NULL".
        let bad =
            sqlx::query("UPDATE listing_renames SET old_price_symbol = 'OLD.AX' WHERE id = 5")
                .execute(&pool)
                .await;
        assert!(
            bad.is_err(),
            "an unrecorded row cannot claim a recorded symbol"
        );
        sqlx::query(
            "UPDATE listing_renames SET old_name = 'Old Co', old_price_symbol = 'OLD.AX' \
             WHERE id = 5",
        )
        .execute(&pool)
        .await
        .expect("recording both together is the shape a rename writes");

        // Both audit triggers come back naming the new columns, carrying the
        // superseded version of the row with them.
        let trail: Vec<(String,)> = sqlx::query_as(
            "SELECT old_row FROM row_history \
             WHERE table_name = 'listing_renames' AND row_id = 5 ORDER BY id",
        )
        .fetch_all(&pool)
        .await
        .unwrap();
        assert!(
            trail.last().unwrap().0.contains("\"old_name\":null")
                && trail
                    .last()
                    .unwrap()
                    .0
                    .contains("\"old_price_symbol\":null"),
            "{:?}",
            trail.last()
        );
        sqlx::query("DELETE FROM listing_renames WHERE id = 5")
            .execute(&pool)
            .await
            .unwrap();
        let deleted: String = sqlx::query_scalar(
            "SELECT old_row FROM row_history \
             WHERE table_name = 'listing_renames' AND row_id = 5 AND operation = 'DELETE'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert!(
            deleted.contains("\"old_name\":\"Old Co\"")
                && deleted.contains("\"old_price_symbol\":\"OLD.AX\""),
            "{deleted}"
        );
    }

    /// 0025 renames `income.lic_capital_gain_deduction` to
    /// `lic_capital_gain_amount` — the LIC's advised attributable part, which
    /// the reports now halve for D8 — and so has to read existing rows forward
    /// by doubling the already-halved figure they hold (SCENARIOS G-04). The
    /// doubling is done on the decimal's own digits as an integer, never
    /// through REAL, so this pins that it is *exact* at every scale, that a
    /// zero row is left alone, and that the audited table's two row-history
    /// triggers come back naming the new column.
    #[tokio::test]
    async fn migration_0025_doubles_the_lic_deduction_into_the_advised_amount() {
        let pool = pool_migrated_below(25).await;
        sqlx::query(
            "INSERT INTO listings (id, exchange_mic, ticker, name, security_type, currency) \
             VALUES (1, 'XASX', 'AFI', 'Australian Foundation Investment', 'LIC', 'AUD')",
        )
        .execute(&pool)
        .await
        .unwrap();
        // Deductions as the old convention stored them: whole dollars, cents,
        // sub-cent precision, a leading-zero fraction, and the column default.
        for (id, deduction) in [
            (1, "25"),
            (2, "12.34"),
            (3, "0.07"),
            (4, "1234567.895"),
            (5, "0"),
        ] {
            sqlx::query(
                "INSERT INTO income (id, listing_id, date_paid, lic_capital_gain_deduction) \
                 VALUES (?, 1, '2025-02-21', ?)",
            )
            .bind(id)
            .bind(deduction)
            .execute(&pool)
            .await
            .unwrap();
        }

        apply_migration(&pool, 25).await;

        let amounts: Vec<(i64, String)> =
            sqlx::query_as("SELECT id, lic_capital_gain_amount FROM income ORDER BY id")
                .fetch_all(&pool)
                .await
                .unwrap();
        assert_eq!(
            amounts
                .iter()
                .map(|(id, v)| (*id, v.as_str()))
                .collect::<Vec<_>>()
                .as_slice(),
            [
                (1, "50"),
                (2, "24.68"),
                (3, "0.14"),
                (4, "2469135.790"),
                (5, "0"),
            ],
            "each stored deduction reads forward as the amount it was half of, exactly"
        );

        // The audit trail is live again and records the *new* column name, so an
        // entry can't say "deduction" while carrying the advised amount.
        sqlx::query("UPDATE income SET lic_capital_gain_amount = '60' WHERE id = 1")
            .execute(&pool)
            .await
            .unwrap();
        let old_row: String = sqlx::query_scalar(
            "SELECT old_row FROM row_history WHERE table_name = 'income' AND row_id = 1",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert!(
            old_row.contains("\"lic_capital_gain_amount\":\"50\""),
            "{old_row}"
        );
        let triggers: Vec<(String,)> = sqlx::query_as(
            "SELECT name FROM sqlite_master \
             WHERE type = 'trigger' AND name LIKE 'income_row_history%' ORDER BY name",
        )
        .fetch_all(&pool)
        .await
        .unwrap();
        assert_eq!(
            triggers
                .iter()
                .map(|t| t.0.as_str())
                .collect::<Vec<_>>()
                .as_slice(),
            ["income_row_history_delete", "income_row_history_update"],
            "both triggers are re-created, not left dropped"
        );
    }

    /// 0047 adds `corporate_actions.renounceable` (SCENARIOS AA-b). What needs
    /// pinning is the meaning it gives existing rows: every rights issue
    /// recorded before it was a **renounceable** offer — that is what the whole
    /// feature was built for and what its documentation says — so they are
    /// backfilled to 1 rather than left unknown, while every other action type
    /// stays NULL, the per-type payload shape this table has always had. The
    /// column's CHECK is what keeps that shape enforceable, and the audited
    /// table's two triggers must come back naming the new column.
    #[tokio::test]
    async fn migration_0047_backfills_existing_rights_issues_as_renounceable() {
        let pool = pool_migrated_below(47).await;
        sqlx::query(
            "INSERT INTO listings (id, exchange_mic, ticker, name, security_type, currency) \
             VALUES (1, 'XASX', 'RTS', 'Rights Test Co', 'Share', 'AUD')",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO corporate_actions \
                 (id, action_type, listing_id, date, rights_units, rights_held_units, \
                  exercise_price, currency) \
             VALUES (1, 'RightsIssue', 1, '2024-07-01', '1', '4', '1.80', 'AUD')",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO corporate_actions \
                 (id, action_type, listing_id, date, bonus_units, bonus_held_units) \
             VALUES (2, 'BonusIssue', 1, '2024-08-01', '1', '10')",
        )
        .execute(&pool)
        .await
        .unwrap();

        apply_migration(&pool, 47).await;

        let flags: Vec<(i64, Option<bool>)> =
            sqlx::query_as("SELECT id, renounceable FROM corporate_actions ORDER BY id")
                .fetch_all(&pool)
                .await
                .unwrap();
        assert_eq!(
            flags,
            [(1, Some(true)), (2, None)],
            "the rights issue is renounceable, the bonus issue carries no flag"
        );

        // And the row reads back through the model as a renounceable offer, so
        // an action entered before the column behaves exactly as it did.
        let action = crate::entities::corporate_action::db_get(&pool, 1)
            .await
            .unwrap()
            .unwrap();
        assert!(matches!(
            action.kind,
            crate::entities::corporate_action::ActionKind::RightsIssue {
                renounceable: true,
                ..
            }
        ));

        // The CHECK holds the shape: only a rights issue may carry the flag,
        // and only 0 or 1.
        for bad in [
            "INSERT INTO corporate_actions (id, action_type, listing_id, date, bonus_units, \
             bonus_held_units, renounceable) VALUES (3, 'BonusIssue', 1, '2024-08-01', '1', \
             '10', 1)",
            "INSERT INTO corporate_actions (id, action_type, listing_id, date, rights_units, \
             rights_held_units, exercise_price, currency, renounceable) \
             VALUES (4, 'RightsIssue', 1, '2024-07-01', '1', '4', '1.80', 'AUD', 2)",
        ] {
            assert!(
                sqlx::query(bad).execute(&pool).await.is_err(),
                "the CHECK must refuse: {bad}"
            );
        }

        // The audit trail is live again and records the new column.
        sqlx::query("UPDATE corporate_actions SET renounceable = 0 WHERE id = 1")
            .execute(&pool)
            .await
            .unwrap();
        let old_row: String = sqlx::query_scalar(
            "SELECT old_row FROM row_history \
             WHERE table_name = 'corporate_actions' AND row_id = 1",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert!(old_row.contains("\"renounceable\":1"), "{old_row}");
        let triggers: Vec<(String,)> = sqlx::query_as(
            "SELECT name FROM sqlite_master \
             WHERE type = 'trigger' AND name LIKE 'corporate_actions_row_history%' ORDER BY name",
        )
        .fetch_all(&pool)
        .await
        .unwrap();
        assert_eq!(
            triggers
                .iter()
                .map(|t| t.0.as_str())
                .collect::<Vec<_>>()
                .as_slice(),
            [
                "corporate_actions_row_history_delete",
                "corporate_actions_row_history_update"
            ],
            "both triggers are re-created, not left dropped"
        );
    }

    #[test]
    fn migrations_do_not_drop_tables_or_columns() {
        let migrations_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("migrations");
        let entries = std::fs::read_dir(&migrations_dir)
            .expect("migrations dir should exist")
            .filter_map(|e| e.ok())
            .filter(|e| e.path().extension().is_some_and(|x| x == "sql"));

        for entry in entries {
            let path = entry.path();
            let sql = std::fs::read_to_string(&path)
                .unwrap_or_else(|_| panic!("could not read {}", path.display()));

            for line in sql.lines() {
                let trimmed = line.trim();
                if trimmed.starts_with("--") {
                    continue;
                }
                let upper = trimmed.to_uppercase();

                assert!(
                    !upper.contains("DROP COLUMN"),
                    "{} contains DROP COLUMN: {}",
                    path.display(),
                    trimmed
                );

                if let Some(rest) = upper.strip_prefix("DROP TABLE") {
                    let after = rest.trim();
                    let table = after
                        .strip_prefix("IF EXISTS")
                        .unwrap_or(after)
                        .split_whitespace()
                        .next()
                        .unwrap_or("")
                        .trim_end_matches(';');
                    assert!(
                        table.ends_with("_OLD"),
                        "{}: DROP TABLE is only allowed on _old tables (rename pattern); got '{}'",
                        path.display(),
                        table.to_lowercase()
                    );
                }
            }
        }
    }

    #[test]
    fn migrations_store_decimals_as_text_never_real() {
        // The consolidated schema persists every monetary/quantity value as TEXT
        // (arbitrary-precision Decimal). Guard against a future migration
        // reintroducing a REAL column or a lossy CAST(... AS TEXT), which was the
        // source of the historical pre-0006 float-imprecision problem.
        let migrations_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("migrations");
        let entries = std::fs::read_dir(&migrations_dir)
            .expect("migrations dir should exist")
            .filter_map(|e| e.ok())
            .filter(|e| e.path().extension().is_some_and(|x| x == "sql"));

        for entry in entries {
            let path = entry.path();
            let sql = std::fs::read_to_string(&path)
                .unwrap_or_else(|_| panic!("could not read {}", path.display()));

            for line in sql.lines() {
                let trimmed = line.trim();
                if trimmed.starts_with("--") {
                    continue;
                }
                let upper = trimmed.to_uppercase();
                assert!(
                    !upper.contains(" REAL"),
                    "{}: decimal columns must be TEXT, not REAL: {}",
                    path.display(),
                    trimmed
                );
                assert!(
                    !upper.contains("CAST(") || !upper.contains("AS TEXT"),
                    "{}: avoid CAST(... AS TEXT) (float-imprecision risk): {}",
                    path.display(),
                    trimmed
                );
            }
        }
    }

    #[tokio::test]
    async fn backup_does_not_overwrite_existing() {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("test.db").to_string_lossy().to_string();
        let pool = init(&db_path).await.unwrap();

        // Fixed destination so this exercises the skip-if-exists guard directly
        // rather than depending on two `backup` calls landing in the same second.
        let dest = backup_path(&db_path, None, None);
        backup_to(&pool, &dest).await.unwrap();
        let mtime1 = std::fs::metadata(&dest).unwrap().modified().unwrap();

        backup_to(&pool, &dest).await.unwrap();
        let mtime2 = std::fs::metadata(&dest).unwrap().modified().unwrap();

        assert_eq!(mtime1, mtime2);
    }

    #[tokio::test]
    async fn fresh_backup_is_verified_in_place() {
        // The happy path: backup_to writes, verifies, and leaves the verified
        // file under its backup name — no quarantine artefact.
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("test.db").to_string_lossy().to_string();
        let pool = init(&db_path).await.unwrap();

        let dest = backup_path(&db_path, None, None);
        backup_to(&pool, &dest).await.unwrap();

        assert!(Path::new(&dest).exists());
        assert!(!Path::new(&format!("{dest}.bad")).exists());
        verify_backup(&pool, &dest).await.unwrap();
    }

    #[tokio::test]
    async fn verification_quarantines_corrupt_file() {
        // A produced file that is not a database (as a torn write / full disk
        // could leave) must fail the backup loudly and be renamed `<name>.bad`
        // so nothing — a human restore or the pruner — mistakes it for a good
        // backup. The copy verified is the staging file; what it is quarantined
        // as is named after the backup it would have become.
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("test.db").to_string_lossy().to_string();
        let pool = init(&db_path).await.unwrap();

        let dest = dir
            .path()
            .join("test-2026-01-04-000000.db")
            .to_string_lossy()
            .to_string();
        let staging = format!("{dest}{STAGING_SUFFIX}");
        std::fs::write(&staging, b"this is not a sqlite database at all").unwrap();

        let err = verify_or_quarantine(&pool, &staging, &dest)
            .await
            .unwrap_err();
        assert!(
            matches!(err, BackupError::Verification { .. }),
            "expected a Verification error, got {err:?}"
        );
        assert!(
            !Path::new(&dest).exists(),
            "the bad file must not keep its backup name"
        );
        assert!(
            !Path::new(&staging).exists(),
            "the staging file is moved aside, not left behind"
        );
        assert!(
            Path::new(&format!("{dest}.bad")).exists(),
            "the bad file is quarantined for diagnosis, not deleted"
        );
    }

    #[tokio::test]
    async fn verification_rejects_backup_missing_migrations() {
        // A structurally valid SQLite file that lacks the applied migrations is
        // not a restorable copy of this database.
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("test.db").to_string_lossy().to_string();
        let pool = init(&db_path).await.unwrap();

        let empty = dir.path().join("empty.db").to_string_lossy().to_string();
        let opts = SqliteConnectOptions::from_str(&format!("sqlite:{empty}"))
            .unwrap()
            .create_if_missing(true);
        let plain = SqlitePool::connect_with(opts).await.unwrap();
        sqlx::query("CREATE TABLE t (x INTEGER)")
            .execute(&plain)
            .await
            .unwrap();
        plain.close().await;

        let reason = verify_backup(&pool, &empty).await.unwrap_err();
        assert!(
            reason.contains("migrations"),
            "reason must name the migrations check: {reason}"
        );
    }

    #[tokio::test]
    async fn a_backup_is_written_under_a_staging_name_and_renamed_only_once_verified() {
        // SCENARIOS T-11. The happy path leaves exactly one file — the verified
        // backup — and no staging debris; and a leftover `.partial` from an
        // earlier interrupted run in the same second is replaced rather than
        // failing the run (`VACUUM INTO` refuses an existing target).
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("test.db").to_string_lossy().to_string();
        let pool = init(&db_path).await.unwrap();

        let dest = dir
            .path()
            .join("test-2026-01-04-000000.db")
            .to_string_lossy()
            .to_string();
        let staging = format!("{dest}{STAGING_SUFFIX}");
        std::fs::write(&staging, b"debris of an interrupted run").unwrap();

        assert!(backup_to(&pool, &dest).await.unwrap());

        assert!(Path::new(&dest).exists(), "the verified backup is in place");
        assert!(
            !Path::new(&staging).exists(),
            "nothing is left under the staging name"
        );
        assert!(!Path::new(&format!("{dest}.bad")).exists());
        verify_backup(&pool, &dest).await.unwrap();
    }

    #[tokio::test]
    async fn an_unverified_copy_never_carries_a_backup_name() {
        // The file an interrupted run leaves behind is the one under the
        // staging name, and nothing treats it as a backup: it does not parse as
        // one, pruning neither deletes it nor counts it against the retention
        // policy (so it can never displace a real first-of-month keeper), and
        // the startup sweep removes it. Before SCENARIOS T-11 the same
        // interruption left that copy under the backup name itself — unverified,
        // counted, and a restore candidate indistinguishable from a good one.
        let dir = tempfile::tempdir().unwrap();
        // One backup a month for 14 months, each on the 15th — old enough that
        // the middle ones survive only as their month's first-of-month keeper.
        for i in 0..14 {
            let month = format!("{}-{:02}", 2025 + (i / 12), 1 + (i % 12));
            fake_backup(dir.path(), &format!("{month}-15-000000"));
        }
        // Dated earlier in a month whose real backup is a keeper: if the
        // staging file were counted at all it would take that month's keeper
        // slot, and the real backup — outside the newest 8 — would be pruned.
        let partial = dir.path().join("test-2025-06-01-000000.db.partial");
        std::fs::write(&partial, b"half a database").unwrap();
        let keeper = dir.path().join("test-2025-06-15-000000.db");

        assert!(
            backup_timestamp("test-2025-06-01-000000.db.partial", "test").is_none(),
            "a staging file must not parse as a backup of this database"
        );

        let dir_str = dir.path().to_string_lossy().to_string();
        let deleted = prune_backups("test.db", Some(&dir_str)).unwrap();
        assert!(!deleted.contains(&partial), "pruning never touches it");
        assert!(partial.exists());
        assert!(
            keeper.exists(),
            "the real first backup of the month is still its keeper"
        );

        let swept = sweep_partial_backups("test.db", Some(&dir_str)).unwrap();
        assert_eq!(swept, vec![partial.clone()]);
        assert!(!partial.exists());
        assert!(keeper.exists(), "the sweep touches nothing else");
    }

    #[test]
    fn the_startup_sweep_only_removes_this_database_s_staging_files() {
        // The sweep runs at startup and deletes files, so its match is as narrow
        // as pruning's: this database's `<stem>-YYYY-MM-DD-HHMMSS[-suffix].db`
        // plus the staging suffix, nothing else in the directory.
        let dir = tempfile::tempdir().unwrap();
        let swept_names = [
            "test-2026-01-04-000000.db.partial",
            "test-2026-01-11-000000-pre-0.5.1.db.partial",
        ];
        let bystanders = [
            "test.db",
            "test.db-wal",
            "test.db-shm",
            "test-2026-01-04-000000.db",
            "test-2026-01-04-000000.db.bad",
            "other-2026-01-04-000000.db.partial",
            "test-garbage.db.partial",
            "test-2026-13-40-000000.db.partial", // month 13, day 40: not a timestamp
            "test.db.partial",
            "some-notes.partial",
        ];
        for name in swept_names.iter().chain(bystanders.iter()) {
            std::fs::write(dir.path().join(name), b"").unwrap();
        }

        let dir_str = dir.path().to_string_lossy().to_string();
        let mut swept = sweep_partial_backups("test.db", Some(&dir_str)).unwrap();
        swept.sort();

        let mut expected: Vec<PathBuf> = swept_names.iter().map(|n| dir.path().join(n)).collect();
        expected.sort();
        assert_eq!(swept, expected);
        for name in bystanders {
            assert!(dir.path().join(name).exists(), "{name} must never be swept");
        }
    }

    #[test]
    fn quarantined_files_are_bounded_to_the_newest_few() {
        // A quarantined `.bad` file is kept for diagnosis, but the likely cause
        // of a verification failure is a failing disk — which fails every weekly
        // run — so an unbounded set fills the volume with full-size copies
        // (SCENARIOS T-11). The newest KEEP_BAD survive; older ones are pruned,
        // and only this database's.
        let dir = tempfile::tempdir().unwrap();
        let bad: Vec<PathBuf> = (1..=6)
            .map(|week| {
                let path = dir
                    .path()
                    .join(format!("test-2026-01-{:02}-000000.db.bad", week * 4));
                std::fs::write(&path, b"").unwrap();
                path
            })
            .collect();
        let others = ["other-2026-01-04-000000.db.bad", "test-notes.db.bad"];
        for name in others {
            std::fs::write(dir.path().join(name), b"").unwrap();
        }
        // A live backup set alongside, so this is the pruner's ordinary run.
        fake_backup(dir.path(), "2026-02-01-000000");

        let dir_str = dir.path().to_string_lossy().to_string();
        let deleted = prune_backups("test.db", Some(&dir_str)).unwrap();

        for path in bad.iter().rev().take(KEEP_BAD) {
            assert!(path.exists(), "{} is one of the newest few", path.display());
            assert!(!deleted.contains(path));
        }
        for path in bad.iter().take(bad.len() - KEEP_BAD) {
            assert!(!path.exists(), "{} is beyond the bound", path.display());
            assert!(deleted.contains(path));
        }
        for name in others {
            assert!(
                dir.path().join(name).exists(),
                "{name} is not a quarantined backup of this database"
            );
        }
    }

    /// Touch an empty file named as a backup of `test.db` taken at `ts`.
    fn fake_backup(dir: &Path, ts: &str) -> PathBuf {
        let path = dir.join(format!("test-{ts}.db"));
        std::fs::write(&path, b"").unwrap();
        path
    }

    /// Touch an empty file named as a suffixed backup of `test.db` taken at
    /// `ts` (e.g. an update.sh pre-upgrade backup).
    fn fake_backup_suffixed(dir: &Path, ts: &str, suffix: &str) -> PathBuf {
        let path = dir.join(format!("test-{ts}-{suffix}.db"));
        std::fs::write(&path, b"").unwrap();
        path
    }

    #[test]
    fn prune_keeps_recent_and_first_of_month_keepers() {
        // Four weekly backups in each of Jan–May 2026. Newest KEEP_RECENT (8)
        // survive (all of May + April); each month's *first* backup survives as
        // its monthly keeper; every other file is pruned.
        let dir = tempfile::tempdir().unwrap();
        let mut all = Vec::new();
        for month in 1..=5 {
            for day in ["01", "08", "15", "22"] {
                all.push(fake_backup(
                    dir.path(),
                    &format!("2026-{month:02}-{day}-000000"),
                ));
            }
        }

        let dir_str = dir.path().to_string_lossy().to_string();
        let deleted = prune_backups("test.db", Some(&dir_str)).unwrap();

        let survives = |ts: &str| dir.path().join(format!("test-{ts}.db")).exists();
        // The newest 8: all four May runs and all four April runs.
        for day in ["01", "08", "15", "22"] {
            assert!(survives(&format!("2026-05-{day}-000000")));
            assert!(survives(&format!("2026-04-{day}-000000")));
        }
        // Monthly keepers for the older months: the first run of each month.
        for month in 1..=3 {
            assert!(survives(&format!("2026-{month:02}-01-000000")));
            for day in ["08", "15", "22"] {
                assert!(
                    !survives(&format!("2026-{month:02}-{day}-000000")),
                    "2026-{month:02}-{day} is neither recent nor a keeper"
                );
            }
        }
        assert_eq!(deleted.len(), 9, "3 old months × 3 non-keeper runs");
    }

    #[test]
    fn prune_drops_monthly_keepers_beyond_the_cap() {
        // One backup per month for 14 months: the 12 most recent months keep
        // their keeper, the 2 oldest are pruned even though each is its
        // month's first backup.
        let dir = tempfile::tempdir().unwrap();
        let months: Vec<String> = (0..14)
            .map(|i| format!("{}-{:02}", 2025 + (i / 12), 1 + (i % 12)))
            .collect();
        for month in &months {
            fake_backup(dir.path(), &format!("{month}-01-000000"));
        }

        let dir_str = dir.path().to_string_lossy().to_string();
        let deleted = prune_backups("test.db", Some(&dir_str)).unwrap();

        let survives = |month: &str| {
            dir.path()
                .join(format!("test-{month}-01-000000.db"))
                .exists()
        };
        assert!(!survives("2025-01"), "oldest month rolls off");
        assert!(!survives("2025-02"), "second-oldest month rolls off");
        for month in &months[2..] {
            assert!(survives(month), "{month} is within the 12 kept months");
        }
        assert_eq!(deleted.len(), 2);
    }

    #[test]
    fn prune_never_touches_non_matching_files() {
        // Alongside enough pattern-matched backups that pruning really deletes
        // something, every non-matching file — the live db, WAL sidecars, a
        // quarantined .bad file, another database's backups, a malformed
        // timestamp, a subdirectory — survives untouched.
        let dir = tempfile::tempdir().unwrap();
        for i in 0..14 {
            let month = format!("{}-{:02}", 2025 + (i / 12), 1 + (i % 12));
            fake_backup(dir.path(), &format!("{month}-01-000000"));
        }
        let bystanders = [
            "test.db",
            "test.db-wal",
            "test.db-shm",
            "other-2025-01-01-000000.db",
            "test-2025-01-01-000000.db.bad",
            // The staging name a copy in progress is written under: never a
            // backup, never a pruning candidate, and never a monthly keeper
            // (SCENARIOS T-11). Startup sweeps these, not the pruner.
            "test-2025-01-01-000000.db.partial",
            "test-garbage.db",
            "test-2025-13-40-000000.db", // month 13, day 40: not a timestamp
        ];
        for name in bystanders {
            std::fs::write(dir.path().join(name), b"").unwrap();
        }
        std::fs::create_dir(dir.path().join("test-2027-01-01-000000.db")).unwrap();

        let dir_str = dir.path().to_string_lossy().to_string();
        let deleted = prune_backups("test.db", Some(&dir_str)).unwrap();

        assert!(!deleted.is_empty(), "pruning must actually have run");
        for name in bystanders {
            assert!(
                dir.path().join(name).exists(),
                "{name} must never be pruned"
            );
        }
        assert!(
            dir.path().join("test-2027-01-01-000000.db").is_dir(),
            "a directory is never a pruning candidate, even name-matched"
        );
    }

    #[test]
    fn suffixed_backups_are_pruning_candidates() {
        // A suffixed one-off backup (e.g. update.sh's pre-upgrade snapshot)
        // must compete in the same retention policy as any other backup of
        // this database — never exempt, or every upgrade would leave a
        // permanent extra copy behind. One backup per month for 14 months (as
        // in `prune_drops_monthly_keepers_beyond_the_cap`), but the very
        // oldest — otherwise this month's sole, and so "first of month",
        // backup — is suffixed: it must still roll off once its month falls
        // outside the 12-month cap, exactly like an unsuffixed one would.
        let dir = tempfile::tempdir().unwrap();
        let months: Vec<String> = (0..14)
            .map(|i| format!("{}-{:02}", 2025 + (i / 12), 1 + (i % 12)))
            .collect();
        let oldest_suffixed =
            fake_backup_suffixed(dir.path(), &format!("{}-01-000000", months[0]), "pre-0.4.0");
        for month in &months[1..] {
            fake_backup(dir.path(), &format!("{month}-01-000000"));
        }

        let dir_str = dir.path().to_string_lossy().to_string();
        let deleted = prune_backups("test.db", Some(&dir_str)).unwrap();

        assert!(
            deleted.contains(&oldest_suffixed),
            "a suffixed backup outside the retention policy must be pruned like any other"
        );
        assert!(!oldest_suffixed.exists());

        // A fresh suffixed backup (within KEEP_RECENT) must survive pruning.
        let fresh_suffixed = fake_backup_suffixed(dir.path(), "2026-02-15-101500", "pre-0.5.1");
        let deleted2 = prune_backups("test.db", Some(&dir_str)).unwrap();
        assert!(
            !deleted2.contains(&fresh_suffixed),
            "a fresh suffixed backup within the retention window must survive"
        );
        assert!(fresh_suffixed.exists());
    }

    #[tokio::test]
    async fn prune_beside_db_spares_the_live_database() {
        // With no --backup-dir, backups (and so pruning) live beside the db
        // file: the pruner must work on the db's own directory and never touch
        // the live database or its sidecars.
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("test.db").to_string_lossy().to_string();
        let pool = init(&db_path).await.unwrap();
        for i in 0..14 {
            let month = format!("{}-{:02}", 2020 + (i / 12), 1 + (i % 12));
            fake_backup(dir.path(), &format!("{month}-01-000000"));
        }

        let deleted = prune_backups(&db_path, None).unwrap();

        assert_eq!(deleted.len(), 2, "the two oldest monthly keepers roll off");
        assert!(Path::new(&db_path).exists(), "live db must survive");
        let row: (i64,) = sqlx::query_as("SELECT 1").fetch_one(&pool).await.unwrap();
        assert_eq!(row.0, 1, "live db still serves queries after pruning");
    }

    #[tokio::test]
    async fn backup_job_prunes_old_backups() {
        // The full backup() path (as the scheduled/triggered job runs it):
        // after the fresh verified backup, files outside the retention policy
        // are pruned from the backup destination.
        let db_dir = tempfile::tempdir().unwrap();
        let backup_dir = tempfile::tempdir().unwrap();
        let db_path = db_dir.path().join("test.db").to_string_lossy().to_string();
        let pool = init(&db_path).await.unwrap();
        // 13 pre-existing monthly backups; with the fresh backup that is 14
        // distinct months, so the two oldest keepers roll off.
        for i in 0..13 {
            let month = format!("{}-{:02}", 2025 + (i / 12), 1 + (i % 12));
            fake_backup(backup_dir.path(), &format!("{month}-01-000000"));
        }

        let dir_str = backup_dir.path().to_string_lossy().to_string();
        backup(&pool, &db_path, Some(&dir_str), None, None)
            .await
            .unwrap();

        let survives = |month: &str| {
            backup_dir
                .path()
                .join(format!("test-{month}-01-000000.db"))
                .exists()
        };
        assert!(!survives("2025-01"));
        assert!(!survives("2025-02"));
        assert!(survives("2025-03"));
        let this_month = Local::now().format("%Y-%m").to_string();
        let fresh = std::fs::read_dir(backup_dir.path())
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| {
                let name = e.file_name().to_string_lossy().into_owned();
                backup_timestamp(&name, "test")
                    .is_some_and(|ts| ts.format("%Y-%m").to_string() == this_month)
            })
            .count();
        assert_eq!(fresh, 1, "the fresh verified backup survives pruning");
    }

    #[tokio::test]
    async fn restore_drill_backup_restores_with_matching_row_counts() {
        // The restore drill: a backup produced by the real job path (write +
        // verify + prune) restores into a working database whose every table
        // has the same row count as the source — proving the artefact actually
        // restores, not merely that a file exists.
        let db_dir = tempfile::tempdir().unwrap();
        let backup_dir = tempfile::tempdir().unwrap();
        let db_path = db_dir.path().join("test.db").to_string_lossy().to_string();
        let pool = init(&db_path).await.unwrap();
        for id in 100..110 {
            sqlx::query("INSERT INTO holding_accounts (id, name) VALUES (?, ?)")
                .bind(id)
                .bind(format!("account {id}"))
                .execute(&pool)
                .await
                .unwrap();
        }

        let dir_str = backup_dir.path().to_string_lossy().to_string();
        backup(&pool, &db_path, Some(&dir_str), None, None)
            .await
            .unwrap();
        let produced = std::fs::read_dir(backup_dir.path())
            .unwrap()
            .filter_map(|e| e.ok())
            .find(|e| backup_timestamp(&e.file_name().to_string_lossy(), "test").is_some())
            .expect("the job produced a backup")
            .path();

        // Restore per the README procedure (from a copy) and start up on it.
        let restored_path = db_dir
            .path()
            .join("restored.db")
            .to_string_lossy()
            .to_string();
        std::fs::copy(&produced, &restored_path).unwrap();
        let restored = init(&restored_path).await.unwrap();

        let tables: Vec<String> = sqlx::query_scalar(
            "SELECT name FROM sqlite_master WHERE type = 'table' AND name NOT LIKE 'sqlite_%'",
        )
        .fetch_all(&pool)
        .await
        .unwrap();
        assert!(!tables.is_empty());
        for table in &tables {
            let count = |p: &SqlitePool| {
                let sql = format!("SELECT COUNT(*) FROM \"{table}\"");
                let p = p.clone();
                async move {
                    sqlx::query_scalar::<_, i64>(sqlx::AssertSqlSafe(sql))
                        .fetch_one(&p)
                        .await
                        .unwrap()
                }
            };
            let (source, drilled) = (count(&pool).await, count(&restored).await);
            assert_eq!(source, drilled, "row count differs for table {table}");
        }
        let (accounts,): (i64,) =
            sqlx::query_as("SELECT COUNT(*) FROM holding_accounts WHERE id >= 100")
                .fetch_one(&restored)
                .await
                .unwrap();
        assert_eq!(accounts, 10, "the drill data made the round trip");
    }

    #[tokio::test]
    async fn backup_command_receives_the_backup_file_token() {
        // `{BACKUP_FILE}` in the configured command must be substituted with the
        // fresh backup's absolute path — proven here by having the command copy
        // it to a marker path and asserting the copy's content matches.
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("test.db").to_string_lossy().to_string();
        let pool = init(&db_path).await.unwrap();

        let marker = dir.path().join("offsite-copy.db");
        let command = format!("cp {{BACKUP_FILE}} {}", marker.to_string_lossy());
        backup(&pool, &db_path, None, Some(&command), None)
            .await
            .unwrap();

        assert!(
            marker.exists(),
            "the command must have run against the real backup path"
        );
    }

    #[tokio::test]
    async fn backup_command_failure_fails_the_job_but_still_prunes() {
        // A post-backup command that exits non-zero (e.g. a failed scp) must
        // surface as a job error — but must not prevent local pruning, since the
        // fresh backup is safely within the retention window regardless.
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("test.db").to_string_lossy().to_string();
        let pool = init(&db_path).await.unwrap();
        for i in 0..13 {
            let month = format!("{}-{:02}", 2025 + (i / 12), 1 + (i % 12));
            fake_backup(dir.path(), &format!("{month}-01-000000"));
        }

        let err = backup(&pool, &db_path, None, Some("exit 1"), None)
            .await
            .unwrap_err();
        assert!(
            matches!(err, BackupError::Command { .. }),
            "expected a Command error, got {err:?}"
        );
        assert!(
            !Path::new(&dir.path().join("test-2025-01-01-000000.db")).exists(),
            "pruning must still have run despite the command failing"
        );
    }

    #[tokio::test]
    async fn backup_command_is_not_run_when_the_backup_was_skipped() {
        // Re-running `backup_to` against an already-existing destination (the
        // same-second-collision skip path) must not re-fire the hook — it
        // already ran when that file was first created.
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("test.db").to_string_lossy().to_string();
        let pool = init(&db_path).await.unwrap();

        let dest = backup_path(&db_path, None, None);
        backup_to(&pool, &dest).await.unwrap();

        let marker = dir.path().join("should-not-exist");
        let command = format!("touch {}", marker.to_string_lossy());
        run_backup_command(&command, &dest).await.unwrap();
        assert!(marker.exists(), "sanity check: the command itself works");
        std::fs::remove_file(&marker).unwrap();

        // Second backup_to against the identical dest is the skip path.
        let created = backup_to(&pool, &dest).await.unwrap();
        assert!(!created, "sanity check: second call to the same dest skips");
    }

    #[tokio::test]
    async fn backup_command_substitution_uses_an_absolute_path() {
        // The configured working directory of the server process shouldn't
        // matter to the hook — the token must resolve to an absolute path even
        // when the backup itself is a relative one.
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("test.db").to_string_lossy().to_string();
        let pool = init(&db_path).await.unwrap();
        let dest = backup_path(&db_path, None, None);
        backup_to(&pool, &dest).await.unwrap();

        let marker = dir.path().join("path-seen.txt");
        let command = format!("echo {{BACKUP_FILE}} > {}", marker.to_string_lossy());
        run_backup_command(&command, &dest).await.unwrap();

        let seen = std::fs::read_to_string(&marker).unwrap();
        assert!(
            Path::new(seen.trim()).is_absolute(),
            "expected an absolute path, got {seen:?}"
        );
    }
}
