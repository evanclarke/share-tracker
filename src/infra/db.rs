use chrono::{DateTime, Local, NaiveDateTime};
use sqlx::{
    Connection, SqlitePool,
    sqlite::{SqliteConnectOptions, SqliteConnection, SqliteJournalMode},
};
use std::{
    collections::HashSet,
    path::{Path, PathBuf},
    str::FromStr,
};

pub async fn init(db_path: &str) -> Result<SqlitePool, sqlx::Error> {
    let url = if db_path == ":memory:" {
        "sqlite::memory:".to_string()
    } else {
        format!("sqlite:{db_path}")
    };

    let mut opts = SqliteConnectOptions::from_str(&url)?
        .create_if_missing(true)
        .foreign_keys(true);

    if db_path != ":memory:" {
        opts = opts.journal_mode(SqliteJournalMode::Wal);
    }

    let pool = SqlitePool::connect_with(opts).await?;

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

/// Destination filename for a backup taken now: `<file>-YYYY-MM-DD-HHMMSS.db`,
/// placed in `backup_dir` when configured (so backups can land on another
/// volume) or beside the database file otherwise. The time component (down to
/// the second) keeps each weekly backup distinct — the backup job runs weekly,
/// so a date-only name would collide across runs.
pub fn backup_path(db_path: &str, backup_dir: Option<&str>) -> String {
    backup_path_at(db_path, backup_dir, Local::now())
}

fn backup_path_at(db_path: &str, backup_dir: Option<&str>, at: DateTime<Local>) -> String {
    let ts = at.format("%Y-%m-%d-%H%M%S");
    let stem = db_path.strip_suffix(".db").unwrap_or(db_path);
    match backup_dir {
        None => format!("{stem}-{ts}.db"),
        Some(dir) => {
            // Only the filename moves to the configured dir; the db's own
            // directory component is dropped.
            let name = Path::new(stem)
                .file_name()
                .map(|f| f.to_string_lossy().into_owned())
                .unwrap_or_else(|| stem.to_string());
            Path::new(dir)
                .join(format!("{name}-{ts}.db"))
                .to_string_lossy()
                .into_owned()
        }
    }
}

/// Why a backup run failed. `Verification` marks a produced file that is not a
/// restorable copy of the live database — the file has been quarantined
/// (renamed `<name>.bad`) so it can never be mistaken for a good backup.
#[derive(Debug)]
pub enum BackupError {
    Db(sqlx::Error),
    Io(std::io::Error),
    Verification { path: String, reason: String },
}

impl std::fmt::Display for BackupError {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        match self {
            BackupError::Db(e) => write!(f, "backup failed: {e}"),
            BackupError::Io(e) => write!(f, "backup failed: {e}"),
            BackupError::Verification { path, reason } => {
                write!(f, "backup verification failed for {path}: {reason}")
            }
        }
    }
}

impl std::error::Error for BackupError {}

impl From<sqlx::Error> for BackupError {
    fn from(e: sqlx::Error) -> Self {
        BackupError::Db(e)
    }
}

impl From<std::io::Error> for BackupError {
    fn from(e: std::io::Error) -> Self {
        BackupError::Io(e)
    }
}

pub async fn backup(
    pool: &SqlitePool,
    db_path: &str,
    backup_dir: Option<&str>,
) -> Result<(), BackupError> {
    // A configured backup dir may not exist yet (fresh volume / first run);
    // create it rather than failing the weekly job. The beside-the-DB default
    // needs no such step — the database file's directory already exists.
    if let Some(dir) = backup_dir {
        std::fs::create_dir_all(dir)?;
    }
    backup_to(pool, &backup_path(db_path, backup_dir)).await?;
    // Prune only after the fresh backup verified: a failed run must never
    // shrink the set of known-good backups.
    let deleted = prune_backups(db_path, backup_dir)?;
    if !deleted.is_empty() {
        tracing::info!(pruned = deleted.len(), "backup retention pruning complete");
    }
    Ok(())
}

/// Write a backup to a specific destination, skipping if it already exists. With
/// a per-second timestamped name a collision only happens for two runs in the
/// same second, so in practice each weekly run writes a fresh file. A freshly
/// written file is verified before the backup counts as complete.
async fn backup_to(pool: &SqlitePool, dest: &str) -> Result<(), BackupError> {
    if Path::new(dest).exists() {
        tracing::debug!(path = dest, "backup already exists, skipping");
    } else {
        tracing::info!(path = dest, "starting backup");
        sqlx::query("VACUUM INTO ?")
            .bind(dest)
            .execute(pool)
            .await?;
        verify_or_quarantine(pool, dest).await?;
        tracing::info!(path = dest, "backup complete and verified");
    }
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

/// Verify `dest`; on failure quarantine it by renaming to `<dest>.bad` — kept
/// for diagnosis but no longer matching the backup naming pattern, so it can
/// neither be restored by mistake nor counted by pruning — log at ERROR, and
/// fail the backup.
async fn verify_or_quarantine(pool: &SqlitePool, dest: &str) -> Result<(), BackupError> {
    let Err(reason) = verify_backup(pool, dest).await else {
        return Ok(());
    };
    let quarantined = format!("{dest}.bad");
    match std::fs::rename(dest, &quarantined) {
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

/// The timestamp embedded in a backup filename, if `name` matches this
/// database's backup naming pattern (`<stem>-YYYY-MM-DD-HHMMSS.db`) exactly.
/// Pruning candidates are selected by this — anything else is never touched.
fn backup_timestamp(name: &str, stem: &str) -> Option<NaiveDateTime> {
    let ts = name
        .strip_prefix(stem)?
        .strip_prefix('-')?
        .strip_suffix(".db")?;
    NaiveDateTime::parse_from_str(ts, "%Y-%m-%d-%H%M%S").ok()
}

/// Delete backups of this database that fall outside the retention policy
/// (see `KEEP_RECENT` / `KEEP_MONTHLY`), returning the deleted paths. Only
/// regular files matching the backup naming pattern in the backup destination
/// are candidates: the live database, its WAL sidecars, quarantined `.bad`
/// files, and any other file are never deleted.
fn prune_backups(db_path: &str, backup_dir: Option<&str>) -> Result<Vec<PathBuf>, std::io::Error> {
    let stem_path = db_path.strip_suffix(".db").unwrap_or(db_path);
    let stem = Path::new(stem_path)
        .file_name()
        .map(|f| f.to_string_lossy().into_owned())
        .unwrap_or_else(|| stem_path.to_string());
    let dir = match backup_dir {
        Some(dir) => PathBuf::from(dir),
        None => match Path::new(db_path).parent() {
            Some(parent) if !parent.as_os_str().is_empty() => parent.to_path_buf(),
            _ => PathBuf::from("."),
        },
    };

    let mut backups: Vec<(NaiveDateTime, PathBuf)> = Vec::new();
    for entry in std::fs::read_dir(&dir)? {
        let entry = entry?;
        if !entry.file_type()?.is_file() {
            continue;
        }
        let name = entry.file_name().to_string_lossy().into_owned();
        if let Some(ts) = backup_timestamp(&name, &stem) {
            backups.push((ts, entry.path()));
        }
    }
    backups.sort_by(|a, b| b.0.cmp(&a.0)); // newest first

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
        let first_of_month = backups
            .iter()
            .filter(|(ts, _)| ts.format("%Y-%m").to_string() == *month)
            .last(); // newest-first, so last = oldest in the month
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
    Ok(deleted)
}

#[cfg(test)]
mod tests {
    use super::*;

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
        let dest = backup_path(&db_path, None);
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
            backup_path_at("share-tracker.db", None, at),
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
            backup_path_at("/data/share-tracker.db", Some("/mnt/backups"), at),
            "/mnt/backups/share-tracker-2026-06-01-143005.db"
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
        backup(&pool, &db_path, Some(&dest_dir)).await.unwrap();

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

        let dest = backup_path(&db_path, None);
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
        let dest = backup_path(&db_path, None);
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

        let dest = backup_path(&db_path, None);
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
        // backup.
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("test.db").to_string_lossy().to_string();
        let pool = init(&db_path).await.unwrap();

        let dest = dir
            .path()
            .join("test-2026-01-04-000000.db")
            .to_string_lossy()
            .to_string();
        std::fs::write(&dest, b"this is not a sqlite database at all").unwrap();

        let err = verify_or_quarantine(&pool, &dest).await.unwrap_err();
        assert!(
            matches!(err, BackupError::Verification { .. }),
            "expected a Verification error, got {err:?}"
        );
        assert!(
            !Path::new(&dest).exists(),
            "the bad file must not keep its backup name"
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

    /// Touch an empty file named as a backup of `test.db` taken at `ts`.
    fn fake_backup(dir: &Path, ts: &str) -> PathBuf {
        let path = dir.join(format!("test-{ts}.db"));
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
        backup(&pool, &db_path, Some(&dir_str)).await.unwrap();

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
        backup(&pool, &db_path, Some(&dir_str)).await.unwrap();
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
                    sqlx::query_scalar::<_, i64>(&sql)
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
}
