use super::decimal::canonicalize_decimal;
use chrono::Local;
use sqlx::{
    Row, SqlitePool,
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

    canonicalize_pre0006_decimals(&pool).await?;

    Ok(pool)
}

/// Decimal columns created as `REAL` in migrations 0004 (`trades`) and 0005
/// (`income`) and converted to TEXT by `CAST(REAL AS TEXT)` in 0006. Every other
/// decimal column was TEXT from creation, so only these can carry the cast's
/// non-canonical (e.g. scientific-notation) output for rows written before 0006.
const PRE0006_DECIMAL_COLUMNS: &[(&str, &[&str])] = &[
    ("trades", &["average_price", "quantity", "brokerage", "gst_on_brokerage", "fx_rate"]),
    (
        "income",
        &[
            "franked_amount",
            "unfranked_amount",
            "foreign_source_income",
            "foreign_tax_paid",
            "tfn_withholding_tax",
            "franking_credits",
            "lic_capital_gain_deduction",
            "conduit_foreign_income",
        ],
    ),
];

/// Repair the `CAST(REAL AS TEXT)` artefacts left in pre-0006 rows by rewriting each
/// affected cell in `rust_decimal`'s canonical plain-decimal form (see
/// `canonicalize_decimal`). Idempotent — rows already canonical are skipped — so it
/// is a cheap no-op on fresh databases and on every startup after the first repair.
/// Runs in one transaction and returns the number of cells rewritten. A malformed
/// stored value fails loudly (decode error) rather than being silently rewritten.
pub async fn canonicalize_pre0006_decimals(pool: &SqlitePool) -> Result<usize, sqlx::Error> {
    let mut tx = pool.begin().await?;
    let mut rewritten = 0usize;

    for (table, columns) in PRE0006_DECIMAL_COLUMNS {
        let rows =
            sqlx::query(&format!("SELECT id, {} FROM {table}", columns.join(", ")))
                .fetch_all(&mut *tx)
                .await?;

        for row in &rows {
            let id: i64 = row.try_get("id")?;
            for column in *columns {
                let stored: String = row.try_get(*column)?;
                if let Some(canonical) = canonicalize_decimal(column, &stored)? {
                    sqlx::query(&format!("UPDATE {table} SET {column} = ? WHERE id = ?"))
                        .bind(&canonical)
                        .bind(id)
                        .execute(&mut *tx)
                        .await?;
                    rewritten += 1;
                }
            }
        }
    }

    tx.commit().await?;

    if rewritten > 0 {
        tracing::info!(rewritten, "canonicalized pre-0006 decimal values");
    }

    Ok(rewritten)
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

    /// Insert a listing (XASX and AUD are seeded) so a trade's FKs resolve, then a
    /// trade whose decimal columns carry the kind of text `CAST(REAL AS TEXT)`
    /// produced for pre-0006 rows.
    async fn seed_trade_with_decimals(pool: &SqlitePool, average_price: &str, quantity: &str) {
        sqlx::query(
            "INSERT INTO listings (id, exchange_mic, ticker, name, security_type, currency, amit)
             VALUES (1, 'XASX', 'TST', 'Test', 'Share', 'AUD', 0)",
        )
        .execute(pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO trades
                (id, trade_type, date, settlement_date, listing_id,
                 average_price, quantity, currency, brokerage_currency)
             VALUES (1, 'Buy', '2020-01-01', '2020-01-03', 1, ?, ?, 'AUD', 'AUD')",
        )
        .bind(average_price)
        .bind(quantity)
        .execute(pool)
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn canonicalize_pre0006_rewrites_scientific_notation_and_is_idempotent() {
        let pool = init(":memory:").await.unwrap();
        // average_price is already canonical; quantity is the scientific-notation
        // form SQLite's CAST emits for a tiny value.
        seed_trade_with_decimals(&pool, "19.99", "1.0e-08").await;

        let rewritten = canonicalize_pre0006_decimals(&pool).await.unwrap();
        assert_eq!(rewritten, 1, "only the scientific-notation cell should be rewritten");

        let quantity: String =
            sqlx::query_scalar("SELECT quantity FROM trades WHERE id = 1").fetch_one(&pool).await.unwrap();
        assert!(!quantity.contains(['e', 'E']), "expected plain decimal, got {quantity:?}");
        assert_eq!(
            quantity.parse::<rust_decimal::Decimal>().unwrap(),
            "0.00000001".parse::<rust_decimal::Decimal>().unwrap(),
            "numeric value must be preserved"
        );

        let price: String =
            sqlx::query_scalar("SELECT average_price FROM trades WHERE id = 1").fetch_one(&pool).await.unwrap();
        assert_eq!(price, "19.99", "already-canonical cells are untouched");

        // Second pass finds nothing to fix.
        assert_eq!(canonicalize_pre0006_decimals(&pool).await.unwrap(), 0);
    }

    #[tokio::test]
    async fn canonicalize_pre0006_fails_loudly_on_malformed_value() {
        let pool = init(":memory:").await.unwrap();
        seed_trade_with_decimals(&pool, "not-a-number", "1").await;

        let err = canonicalize_pre0006_decimals(&pool).await.unwrap_err();
        assert!(matches!(err, sqlx::Error::Decode(_)), "got {err:?}");
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
