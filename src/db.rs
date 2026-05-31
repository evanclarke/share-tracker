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

    SqlitePool::connect_with(opts).await
}

pub fn backup_path(db_path: &str) -> String {
    let date = Local::now().format("%Y-%m-%d");
    let stem = db_path.strip_suffix(".db").unwrap_or(db_path);
    format!("{stem}-{date}.db")
}

pub async fn backup(pool: &SqlitePool, db_path: &str) -> Result<(), sqlx::Error> {
    let dest = backup_path(db_path);
    if !Path::new(&dest).exists() {
        sqlx::query("VACUUM INTO ?").bind(&dest).execute(pool).await?;
    }
    Ok(())
}

pub fn spawn_daily_backup(pool: SqlitePool, db_path: String) {
    tokio::spawn(async move {
        loop {
            if let Err(e) = backup(&pool, &db_path).await {
                eprintln!("warning: backup failed: {e}");
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
