use chrono::{Local, TimeZone};
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

pub fn backup_path(db_path: &str) -> String {
    let date = Local::now().format("%Y-%m-%d");
    let stem = db_path.strip_suffix(".db").unwrap_or(db_path);
    format!("{stem}-{date}.db")
}

pub async fn backup(pool: &SqlitePool, db_path: &str) -> Result<(), sqlx::Error> {
    let dest = backup_path(db_path);
    if Path::new(&dest).exists() {
        tracing::debug!(path = dest, "backup already exists, skipping");
    } else {
        tracing::info!(path = dest, "starting backup");
        sqlx::query("VACUUM INTO ?").bind(&dest).execute(pool).await?;
        tracing::info!(path = dest, "backup complete");
    }
    Ok(())
}

pub fn spawn_daily_backup(pool: SqlitePool, db_path: String) {
    tokio::spawn(async move {
        loop {
            if let Err(e) = backup(&pool, &db_path).await {
                tracing::warn!("backup failed: {e}");
            }
            let now = Local::now();
            let next_midnight = Local
                .from_local_datetime(
                    &(now + chrono::Duration::days(1))
                        .date_naive()
                        .and_hms_opt(0, 0, 0)
                        .unwrap(),
                )
                .unwrap();
            let secs = (next_midnight - now).num_seconds().max(1) as u64;
            tracing::info!(
                next_run = %next_midnight.format("%Y-%m-%d %H:%M:%S %Z"),
                "next backup scheduled"
            );
            tokio::time::sleep(std::time::Duration::from_secs(secs)).await;
        }
    });
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

        backup(&pool, &db_path).await.unwrap();

        assert!(Path::new(&backup_path(&db_path)).exists());
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

    #[tokio::test]
    async fn backup_does_not_overwrite_existing() {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("test.db").to_string_lossy().to_string();
        let pool = init(&db_path).await.unwrap();

        backup(&pool, &db_path).await.unwrap();
        let mtime1 = std::fs::metadata(backup_path(&db_path)).unwrap().modified().unwrap();

        backup(&pool, &db_path).await.unwrap();
        let mtime2 = std::fs::metadata(backup_path(&db_path)).unwrap().modified().unwrap();

        assert_eq!(mtime1, mtime2);
    }
}
