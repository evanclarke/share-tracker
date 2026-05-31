use axum::{
    Json, Router,
    extract::{Path, State},
    http::StatusCode,
    routing::{get, post},
};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use sqlx::{Row, SqlitePool};
use std::time::Duration;

use crate::decimal::parse_dec;

/// Source of the ATO's published monthly foreign exchange rates. The import
/// expects a CSV with one rate per line: `currency,YYYY-MM,rate`, where `rate`
/// is foreign currency units per 1 AUD (foreign-per-AUD). A leading header row
/// (first field `currency`, case-insensitive) and blank lines are ignored.
const ATO_FX_RATES_URL: &str =
    "https://data.gov.au/data/dataset/ato-foreign-exchange-rates/monthly-rates.csv";

/// One week between scheduled imports.
const IMPORT_INTERVAL: Duration = Duration::from_secs(7 * 24 * 60 * 60);

/// An ATO-published monthly foreign exchange rate. `rate` is foreign currency
/// units per 1 AUD (foreign-per-AUD), so AUD = foreign / rate.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AtoFxRate {
    pub id: i64,
    pub currency: String,
    pub month: String, // 'YYYY-MM'
    pub rate: Decimal,
}

impl<'r> sqlx::FromRow<'r, sqlx::sqlite::SqliteRow> for AtoFxRate {
    fn from_row(row: &'r sqlx::sqlite::SqliteRow) -> Result<Self, sqlx::Error> {
        Ok(AtoFxRate {
            id: row.try_get("id")?,
            currency: row.try_get("currency")?,
            month: row.try_get("month")?,
            rate: parse_dec("rate", row.try_get("rate")?)?,
        })
    }
}

#[derive(Debug)]
pub enum ImportError {
    /// Could not retrieve the published rates (network / HTTP error).
    Fetch(String),
    /// A line in the feed was not well-formed `currency,YYYY-MM,rate`.
    Parse(String),
    Db(sqlx::Error),
}

impl From<sqlx::Error> for ImportError {
    fn from(e: sqlx::Error) -> Self {
        ImportError::Db(e)
    }
}

/// Outcome of an import run: how many new rows were inserted vs already present.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ImportSummary {
    pub inserted: usize,
    pub skipped: usize,
}

pub fn router() -> Router<SqlitePool> {
    Router::new()
        .route("/ato_fx_rates", get(list))
        .route("/ato_fx_rates/{id}", get(get_one))
        // Manual trigger for retries / missed runs. Read-only for clients otherwise.
        .route("/ato_fx_rates/import", post(import))
}

pub async fn db_list(pool: &SqlitePool) -> Result<Vec<AtoFxRate>, sqlx::Error> {
    sqlx::query_as("SELECT id, currency, month, rate FROM ato_fx_rates ORDER BY currency, month")
        .fetch_all(pool)
        .await
}

pub async fn db_get(pool: &SqlitePool, id: i64) -> Result<Option<AtoFxRate>, sqlx::Error> {
    sqlx::query_as("SELECT id, currency, month, rate FROM ato_fx_rates WHERE id = ?")
        .bind(id)
        .fetch_optional(pool)
        .await
}

/// Insert a rate for a (currency, month) only if absent, leaving any existing row
/// unchanged. Returns `true` when a new row was inserted. The `UNIQUE(currency,
/// month)` constraint plus `DO NOTHING` make re-running the import idempotent.
pub async fn db_import_rate(
    pool: &SqlitePool,
    currency: &str,
    month: &str,
    rate: Decimal,
) -> Result<bool, sqlx::Error> {
    let result = sqlx::query(
        "INSERT INTO ato_fx_rates (currency, month, rate) VALUES (?, ?, ?) \
         ON CONFLICT(currency, month) DO NOTHING",
    )
    .bind(currency)
    .bind(month)
    .bind(rate.to_string())
    .execute(pool)
    .await?;
    Ok(result.rows_affected() > 0)
}

/// Parse the ATO CSV feed into `(currency, month, rate)` tuples. Fails loudly on a
/// malformed data row rather than silently dropping it — a missing rate would later
/// surface as a failed (un-substitutable) AUD conversion.
pub fn parse_rates(content: &str) -> Result<Vec<(String, String, Decimal)>, ImportError> {
    let mut out = Vec::new();
    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let fields: Vec<&str> = line.split(',').map(str::trim).collect();
        // Skip a header row.
        if fields[0].eq_ignore_ascii_case("currency") {
            continue;
        }
        if fields.len() != 3 {
            return Err(ImportError::Parse(format!(
                "expected `currency,YYYY-MM,rate`, got {line:?}"
            )));
        }
        let currency = fields[0].to_uppercase();
        if currency.len() != 3 || !currency.chars().all(|c| c.is_ascii_alphabetic()) {
            return Err(ImportError::Parse(format!("invalid ISO 4217 currency {:?}", fields[0])));
        }
        let month = fields[1];
        if month.len() != 7 || month.as_bytes()[4] != b'-' {
            return Err(ImportError::Parse(format!("invalid month {month:?}, expected YYYY-MM")));
        }
        let rate: Decimal = fields[2]
            .parse()
            .map_err(|e| ImportError::Parse(format!("invalid rate {:?}: {e}", fields[2])))?;
        out.push((currency, month.to_string(), rate));
    }
    Ok(out)
}

/// Parse the given feed content and idempotently upsert each rate. Shared by the
/// scheduled task and the manual-trigger endpoint.
pub async fn import_from_content(
    pool: &SqlitePool,
    content: &str,
) -> Result<ImportSummary, ImportError> {
    let rates = parse_rates(content)?;
    let mut summary = ImportSummary { inserted: 0, skipped: 0 };
    for (currency, month, rate) in rates {
        if db_import_rate(pool, &currency, &month, rate).await? {
            summary.inserted += 1;
        } else {
            summary.skipped += 1;
        }
    }
    Ok(summary)
}

/// Fetch the published rates from the ATO and import them.
pub async fn run_import(pool: &SqlitePool) -> Result<ImportSummary, ImportError> {
    let content = fetch_rates(ATO_FX_RATES_URL).await?;
    import_from_content(pool, &content).await
}

async fn fetch_rates(url: &str) -> Result<String, ImportError> {
    let resp = reqwest::get(url)
        .await
        .map_err(|e| ImportError::Fetch(e.to_string()))?
        .error_for_status()
        .map_err(|e| ImportError::Fetch(e.to_string()))?;
    resp.text().await.map_err(|e| ImportError::Fetch(e.to_string()))
}

/// Run the ATO FX rate import now, then once a week, alongside the daily backup.
pub fn spawn_weekly_import(pool: SqlitePool) {
    tokio::spawn(async move {
        loop {
            match run_import(&pool).await {
                Ok(s) => tracing::info!(
                    inserted = s.inserted,
                    skipped = s.skipped,
                    "ATO FX rate import complete"
                ),
                Err(e) => tracing::warn!("ATO FX rate import failed: {e:?}"),
            }
            tokio::time::sleep(IMPORT_INTERVAL).await;
        }
    });
}

async fn list(State(pool): State<SqlitePool>) -> Result<Json<Vec<AtoFxRate>>, StatusCode> {
    db_list(&pool).await.map(Json).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)
}

async fn get_one(
    State(pool): State<SqlitePool>,
    Path(id): Path<i64>,
) -> Result<Json<AtoFxRate>, StatusCode> {
    db_get(&pool, id)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
        .map(Json)
        .ok_or(StatusCode::NOT_FOUND)
}

/// Manually trigger the import. With a non-empty request body, imports that body
/// (a downloaded feed — useful for retries when the ATO endpoint is unreachable);
/// with an empty body, fetches from the ATO. Both share `import_from_content`.
async fn import(
    State(pool): State<SqlitePool>,
    body: String,
) -> Result<Json<ImportSummary>, StatusCode> {
    let result = if body.trim().is_empty() {
        run_import(&pool).await
    } else {
        import_from_content(&pool, &body).await
    };
    result.map(Json).map_err(|e| match e {
        ImportError::Parse(msg) => {
            tracing::warn!(%msg, "ATO FX rate import rejected malformed feed");
            StatusCode::UNPROCESSABLE_ENTITY
        }
        ImportError::Fetch(msg) => {
            tracing::warn!(%msg, "ATO FX rate fetch failed");
            StatusCode::BAD_GATEWAY
        }
        ImportError::Db(e) => {
            tracing::error!(error = %e, "ATO FX rate import db error");
            StatusCode::INTERNAL_SERVER_ERROR
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db;
    use axum::{body::Body, http::Request};
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    async fn test_pool() -> SqlitePool {
        db::init(":memory:").await.unwrap()
    }

    const SAMPLE_CSV: &str = "currency,month,rate\nUSD,2024-01,1.5\nUSD,2024-02,1.6\nGBP,2024-01,1.9\n";

    // DB-level tests

    #[tokio::test]
    async fn db_insert_and_retrieve() {
        let pool = test_pool().await;
        assert!(db_import_rate(&pool, "USD", "2024-01", "1.5".parse().unwrap()).await.unwrap());
        let rows = db_list(&pool).await.unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].currency, "USD");
        assert_eq!(rows[0].month, "2024-01");
        assert_eq!(rows[0].rate, "1.5".parse::<Decimal>().unwrap());

        let got = db_get(&pool, rows[0].id).await.unwrap().unwrap();
        assert_eq!(got.currency, "USD");
    }

    #[tokio::test]
    async fn db_currency_month_uniqueness_enforced() {
        let pool = test_pool().await;
        assert!(db_import_rate(&pool, "USD", "2024-01", "1.5".parse().unwrap()).await.unwrap());
        // Same (currency, month): no new row, existing left unchanged.
        assert!(!db_import_rate(&pool, "USD", "2024-01", "1.6".parse().unwrap()).await.unwrap());

        let rows = db_list(&pool).await.unwrap();
        assert_eq!(rows.len(), 1, "(currency, month) must be unique");
        assert_eq!(rows[0].rate, "1.5".parse::<Decimal>().unwrap(), "existing row unchanged");

        // A different month is a distinct row.
        db_import_rate(&pool, "USD", "2024-02", "1.7".parse().unwrap()).await.unwrap();
        assert_eq!(db_list(&pool).await.unwrap().len(), 2);
    }

    #[tokio::test]
    async fn db_decimal_precision_preserved_in_round_trip() {
        let pool = test_pool().await;
        let rate = "0.67890123456789".parse::<Decimal>().unwrap();
        db_import_rate(&pool, "USD", "2024-03", rate).await.unwrap();
        let rows = db_list(&pool).await.unwrap();
        assert_eq!(rows[0].rate, rate);
    }

    // Parsing tests

    #[tokio::test]
    async fn parse_rates_parses_valid_csv_skipping_header_and_blanks() {
        let parsed = parse_rates(SAMPLE_CSV).unwrap();
        assert_eq!(
            parsed,
            vec![
                ("USD".to_string(), "2024-01".to_string(), "1.5".parse().unwrap()),
                ("USD".to_string(), "2024-02".to_string(), "1.6".parse().unwrap()),
                ("GBP".to_string(), "2024-01".to_string(), "1.9".parse().unwrap()),
            ]
        );
    }

    #[tokio::test]
    async fn parse_rates_rejects_malformed_rate() {
        let err = parse_rates("USD,2024-01,not-a-number").unwrap_err();
        assert!(matches!(err, ImportError::Parse(_)));
    }

    #[tokio::test]
    async fn parse_rates_rejects_bad_month() {
        let err = parse_rates("USD,2024/01,1.5").unwrap_err();
        assert!(matches!(err, ImportError::Parse(_)));
    }

    // Import idempotency

    #[tokio::test]
    async fn import_is_idempotent() {
        let pool = test_pool().await;

        let first = import_from_content(&pool, SAMPLE_CSV).await.unwrap();
        assert_eq!(first, ImportSummary { inserted: 3, skipped: 0 });

        // Re-running stores no duplicates and leaves existing rows unchanged, even
        // if the feed carries a different rate for an existing (currency, month).
        let altered = "USD,2024-01,9.99\nUSD,2024-02,1.6\nGBP,2024-01,1.9\n";
        let second = import_from_content(&pool, altered).await.unwrap();
        assert_eq!(second, ImportSummary { inserted: 0, skipped: 3 });

        let rows = db_list(&pool).await.unwrap();
        assert_eq!(rows.len(), 3, "no duplicates created");
        let usd_jan = rows.iter().find(|r| r.currency == "USD" && r.month == "2024-01").unwrap();
        assert_eq!(usd_jan.rate, "1.5".parse::<Decimal>().unwrap(), "existing row unchanged");
    }

    #[tokio::test]
    async fn import_adds_only_new_rows_on_rerun() {
        let pool = test_pool().await;
        import_from_content(&pool, SAMPLE_CSV).await.unwrap();
        // Feed now includes a new month alongside the existing rows.
        let extended = format!("{SAMPLE_CSV}GBP,2024-02,1.95\n");
        let summary = import_from_content(&pool, &extended).await.unwrap();
        assert_eq!(summary, ImportSummary { inserted: 1, skipped: 3 });
        assert_eq!(db_list(&pool).await.unwrap().len(), 4);
    }

    // API-level tests

    #[tokio::test]
    async fn api_list_returns_rates() {
        let pool = test_pool().await;
        db_import_rate(&pool, "USD", "2024-01", "1.5".parse().unwrap()).await.unwrap();
        let resp = router()
            .with_state(pool)
            .oneshot(Request::builder().uri("/ato_fx_rates").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let rates: Vec<AtoFxRate> = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(rates.len(), 1);
        assert_eq!(rates[0].currency, "USD");
    }

    #[tokio::test]
    async fn api_get_existing_returns_rate() {
        let pool = test_pool().await;
        db_import_rate(&pool, "USD", "2024-01", "1.5".parse().unwrap()).await.unwrap();
        let id = db_list(&pool).await.unwrap()[0].id;
        let resp = router()
            .with_state(pool)
            .oneshot(
                Request::builder().uri(format!("/ato_fx_rates/{id}")).body(Body::empty()).unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let rate: AtoFxRate = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(rate.currency, "USD");
        assert_eq!(rate.rate, "1.5".parse::<Decimal>().unwrap());
    }

    #[tokio::test]
    async fn api_get_missing_returns_404() {
        let pool = test_pool().await;
        let resp = router()
            .with_state(pool)
            .oneshot(Request::builder().uri("/ato_fx_rates/999").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn api_import_endpoint_invokes_import() {
        let pool = test_pool().await;
        let resp = router()
            .with_state(pool.clone())
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/ato_fx_rates/import")
                    .body(Body::from(SAMPLE_CSV))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let summary: ImportSummary = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(summary, ImportSummary { inserted: 3, skipped: 0 });
        assert_eq!(db_list(&pool).await.unwrap().len(), 3);
    }

    #[tokio::test]
    async fn api_import_endpoint_rejects_malformed_feed() {
        let pool = test_pool().await;
        let resp = router()
            .with_state(pool)
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/ato_fx_rates/import")
                    .body(Body::from("USD,2024-01,oops"))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY);
    }
}
