use chrono::{DateTime, Local};
use sqlx::{
    SqlitePool,
    sqlite::{SqliteConnectOptions, SqliteJournalMode},
};
use std::{path::Path, str::FromStr};

pub async fn init(db_path: &str) -> Result<SqlitePool, sqlx::Error> {
    let url = if db_path == ":memory:" {
        "sqlite::memory:".to_string()
    } else {
        format!("sqlite:{db_path}")
    };

    let mut opts = SqliteConnectOptions::from_str(&url)?.create_if_missing(true).foreign_keys(true);

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

/// Destination filename for a backup taken now: `<file>-YYYY-MM-DD-HHMMSS.db`.
/// The time component (down to the second) keeps each weekly backup distinct —
/// the backup job runs weekly, so a date-only name would collide across runs.
pub fn backup_path(db_path: &str) -> String {
    backup_path_at(db_path, Local::now())
}

fn backup_path_at(db_path: &str, at: DateTime<Local>) -> String {
    let ts = at.format("%Y-%m-%d-%H%M%S");
    let stem = db_path.strip_suffix(".db").unwrap_or(db_path);
    format!("{stem}-{ts}.db")
}

pub async fn backup(pool: &SqlitePool, db_path: &str) -> Result<(), sqlx::Error> {
    backup_to(pool, &backup_path(db_path)).await
}

/// Write a backup to a specific destination, skipping if it already exists. With
/// a per-second timestamped name a collision only happens for two runs in the
/// same second, so in practice each weekly run writes a fresh file.
async fn backup_to(pool: &SqlitePool, dest: &str) -> Result<(), sqlx::Error> {
    if Path::new(dest).exists() {
        tracing::debug!(path = dest, "backup already exists, skipping");
    } else {
        tracing::info!(path = dest, "starting backup");
        sqlx::query("VACUUM INTO ?").bind(dest).execute(pool).await?;
        tracing::info!(path = dest, "backup complete");
    }
    Ok(())
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
        let dest = backup_path(&db_path);
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
            backup_path_at("share-tracker.db", at),
            "share-tracker-2026-06-01-143005.db"
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
            .filter(|e| e.path().extension().map_or(false, |x| x == "sql"));

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
                        .trim()
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
            .filter(|e| e.path().extension().map_or(false, |x| x == "sql"));

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
        let dest = backup_path(&db_path);
        backup_to(&pool, &dest).await.unwrap();
        let mtime1 = std::fs::metadata(&dest).unwrap().modified().unwrap();

        backup_to(&pool, &dest).await.unwrap();
        let mtime2 = std::fs::metadata(&dest).unwrap().modified().unwrap();

        assert_eq!(mtime1, mtime2);
    }
}
