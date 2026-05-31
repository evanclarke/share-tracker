use axum::{
    Json, Router,
    extract::{Path, State},
    http::StatusCode,
    routing::{get, post},
};
use chrono::NaiveDate;
use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;

/// Source of the ISO 10383 Market Identifier Code registry: the official
/// ISO20022 published list. It carries no trading currency, timezone, or
/// settlement convention, so it is reference data only — used to validate that a
/// curated exchange's MIC is real and not expired, never to populate `exchanges`.
const MIC_REGISTRY_URL: &str =
    "https://www.iso20022.org/sites/default/files/ISO10383_MIC/ISO10383_MIC.csv";

/// One ISO 10383 MIC. `status` is the ISO STATUS (`ACTIVE` | `UPDATED` |
/// `EXPIRED`); `expiry_date` is set only for expired entries. All fields are
/// surfaced by the read endpoints; `status`/`expiry_date` additionally drive the
/// exchange-MIC validation report.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, sqlx::FromRow)]
pub struct MicEntry {
    pub mic: String,
    pub operating_mic: String,
    pub name: String,
    pub country_code: String,
    pub city: Option<String>,
    pub status: String,
    pub expiry_date: Option<String>, // 'YYYY-MM-DD', present only when EXPIRED
}

#[derive(Debug)]
pub enum ImportError {
    /// Could not retrieve the published registry (network / HTTP error).
    Fetch(String),
    /// The feed was not the expected ISO10383_MIC shape (missing column, bad row).
    Parse(String),
    Db(sqlx::Error),
}

impl From<sqlx::Error> for ImportError {
    fn from(e: sqlx::Error) -> Self {
        ImportError::Db(e)
    }
}

/// Outcome of an import run: how many registry rows were written (inserted or
/// updated). The registry mirrors the latest ISO publication, so every row in the
/// feed is upserted on every run.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ImportSummary {
    pub imported: usize,
}

pub fn router() -> Router<SqlitePool> {
    Router::new()
        .route("/mic_registry", get(list))
        .route("/mic_registry/{mic}", get(get_one))
        // Manual trigger for retries / missed runs. Read-only for clients otherwise.
        .route("/mic_registry/import", post(import))
}

pub async fn db_list(pool: &SqlitePool) -> Result<Vec<MicEntry>, sqlx::Error> {
    sqlx::query_as(
        "SELECT mic, operating_mic, name, country_code, city, status, expiry_date \
         FROM mic_registry ORDER BY mic",
    )
    .fetch_all(pool)
    .await
}

pub async fn db_get(pool: &SqlitePool, mic: &str) -> Result<Option<MicEntry>, sqlx::Error> {
    sqlx::query_as(
        "SELECT mic, operating_mic, name, country_code, city, status, expiry_date \
         FROM mic_registry WHERE mic = ?",
    )
    .bind(mic)
    .fetch_optional(pool)
    .await
}

/// Insert or update a registry entry by MIC. Unlike the FX rates (which are
/// immutable once published), a MIC's status and expiry change over time, so the
/// registry tracks the latest ISO publication via `ON CONFLICT DO UPDATE`.
/// Generic over the executor so it runs against either the pool or an import
/// transaction.
pub async fn db_upsert<'e, E>(executor: E, entry: &MicEntry) -> Result<(), sqlx::Error>
where
    E: sqlx::Executor<'e, Database = sqlx::Sqlite>,
{
    sqlx::query(
        "INSERT INTO mic_registry (mic, operating_mic, name, country_code, city, status, expiry_date) \
         VALUES (?, ?, ?, ?, ?, ?, ?) \
         ON CONFLICT(mic) DO UPDATE SET \
             operating_mic = excluded.operating_mic, \
             name          = excluded.name, \
             country_code  = excluded.country_code, \
             city          = excluded.city, \
             status        = excluded.status, \
             expiry_date   = excluded.expiry_date",
    )
    .bind(&entry.mic)
    .bind(&entry.operating_mic)
    .bind(&entry.name)
    .bind(&entry.country_code)
    .bind(&entry.city)
    .bind(&entry.status)
    .bind(&entry.expiry_date)
    .execute(executor)
    .await?;
    Ok(())
}

/// Parse the ISO10383_MIC CSV into registry entries.
///
/// The file is fully double-quoted (fields such as institution names contain
/// commas), so it must be parsed with a real CSV reader, not by splitting on `,`.
/// Columns are located by header name so a reordered feed still works; a missing
/// required column or a malformed expiry date fails loudly rather than silently
/// dropping an entry. The `EXPIRY DATE` is `YYYYMMDD` (empty unless EXPIRED) and
/// is normalised to `YYYY-MM-DD`.
pub fn parse_registry(content: &str) -> Result<Vec<MicEntry>, ImportError> {
    let content = content.strip_prefix('\u{feff}').unwrap_or(content);
    let mut reader = csv::ReaderBuilder::new()
        .has_headers(true)
        .flexible(true)
        .from_reader(content.as_bytes());

    let headers = reader.headers().map_err(|e| ImportError::Parse(e.to_string()))?.clone();
    let col = |name: &str| -> Result<usize, ImportError> {
        headers
            .iter()
            .position(|h| h == name)
            .ok_or_else(|| ImportError::Parse(format!("missing required column {name:?}")))
    };
    let (i_mic, i_oper, i_name, i_country, i_city, i_status, i_expiry) = (
        col("MIC")?,
        col("OPERATING MIC")?,
        col("MARKET NAME-INSTITUTION DESCRIPTION")?,
        col("ISO COUNTRY CODE (ISO 3166)")?,
        col("CITY")?,
        col("STATUS")?,
        col("EXPIRY DATE")?,
    );

    let field = |rec: &csv::StringRecord, idx: usize| rec.get(idx).unwrap_or("").trim().to_string();
    let opt = |s: String| if s.is_empty() { None } else { Some(s) };

    let mut out = Vec::new();
    for result in reader.records() {
        let rec = result.map_err(|e| ImportError::Parse(e.to_string()))?;
        let mic = field(&rec, i_mic);
        if mic.is_empty() {
            return Err(ImportError::Parse("data row with empty MIC".into()));
        }
        let expiry_date = match opt(field(&rec, i_expiry)) {
            None => None,
            Some(raw) => {
                let date = NaiveDate::parse_from_str(&raw, "%Y%m%d").map_err(|e| {
                    ImportError::Parse(format!("invalid expiry date {raw:?} for {mic}: {e}"))
                })?;
                Some(date.format("%Y-%m-%d").to_string())
            }
        };
        out.push(MicEntry {
            mic,
            operating_mic: field(&rec, i_oper),
            name: field(&rec, i_name),
            country_code: field(&rec, i_country),
            city: opt(field(&rec, i_city)),
            status: field(&rec, i_status),
            expiry_date,
        });
    }

    if out.is_empty() {
        return Err(ImportError::Parse("feed contained no MIC rows".into()));
    }
    Ok(out)
}

/// Parse the given feed content and upsert every entry in one transaction, so an
/// import either fully replaces the registry's view of the feed or makes no
/// change at all. Shared by the scheduled task and the manual-trigger endpoint.
pub async fn import_from_content(
    pool: &SqlitePool,
    content: &str,
) -> Result<ImportSummary, ImportError> {
    let entries = parse_registry(content)?;
    let mut tx = pool.begin().await?;
    for entry in &entries {
        db_upsert(&mut *tx, entry).await?;
    }
    tx.commit().await?;
    Ok(ImportSummary { imported: entries.len() })
}

/// Fetch the published registry from ISO and import it.
pub async fn run_import(pool: &SqlitePool) -> Result<ImportSummary, ImportError> {
    let content = fetch_registry(MIC_REGISTRY_URL).await?;
    import_from_content(pool, &content).await
}

async fn fetch_registry(url: &str) -> Result<String, ImportError> {
    let resp = reqwest::get(url)
        .await
        .map_err(|e| ImportError::Fetch(e.to_string()))?
        .error_for_status()
        .map_err(|e| ImportError::Fetch(e.to_string()))?;
    resp.text().await.map_err(|e| ImportError::Fetch(e.to_string()))
}

async fn list(State(pool): State<SqlitePool>) -> Result<Json<Vec<MicEntry>>, StatusCode> {
    db_list(&pool).await.map(Json).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)
}

async fn get_one(
    State(pool): State<SqlitePool>,
    Path(mic): Path<String>,
) -> Result<Json<MicEntry>, StatusCode> {
    db_get(&pool, &mic)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
        .map(Json)
        .ok_or(StatusCode::NOT_FOUND)
}

/// Manually trigger the import. With a non-empty request body, imports that body
/// (a downloaded ISO10383_MIC CSV — useful for retries when ISO is unreachable);
/// with an empty body, fetches from ISO. Both share `import_from_content`.
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
            tracing::warn!(%msg, "MIC registry import rejected malformed feed");
            StatusCode::UNPROCESSABLE_ENTITY
        }
        ImportError::Fetch(msg) => {
            tracing::warn!(%msg, "MIC registry fetch failed");
            StatusCode::BAD_GATEWAY
        }
        ImportError::Db(e) => {
            tracing::error!(error = %e, "MIC registry import db error");
            StatusCode::INTERNAL_SERVER_ERROR
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::infra::db;
    use axum::{body::Body, http::Request};
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    async fn test_pool() -> SqlitePool {
        db::init(":memory:").await.unwrap()
    }

    /// A trimmed slice of the real ISO10383_MIC layout: a BOM, the full quoted
    /// header row, an ACTIVE operating MIC, an ACTIVE segment MIC with an empty
    /// CITY, and an EXPIRED entry with an expiry date. A field carries an embedded
    /// comma ("INTERCONTINENTAL EXCHANGE, INC.") to exercise quoted parsing.
    const SAMPLE_CSV: &str = "\u{feff}\"MIC\",\"OPERATING MIC\",\"OPRT/SGMT\",\"MARKET NAME-INSTITUTION DESCRIPTION\",\"LEGAL ENTITY NAME\",\"LEI\",\"MARKET CATEGORY CODE\",\"ACRONYM\",\"ISO COUNTRY CODE (ISO 3166)\",\"CITY\",\"WEBSITE\",\"STATUS\",\"CREATION DATE\",\"LAST UPDATE DATE\",\"LAST VALIDATION DATE\",\"EXPIRY DATE\",\"COMMENTS\"\n\
        \"XNYS\",\"XNYS\",\"OPRT\",\"NEW YORK STOCK EXCHANGE\",\"INTERCONTINENTAL EXCHANGE, INC.\",\"5493000F4ZO33MV32P92\",\"RMKT\",\"NYSE\",\"US\",\"NEW YORK\",\"WWW.NYSE.COM\",\"ACTIVE\",\"20050627\",\"20250428\",\"20250428\",\"\",\"\"\n\
        \"XASX\",\"XASX\",\"OPRT\",\"AUSTRALIAN SECURITIES EXCHANGE\",\"\",\"\",\"NSPD\",\"ASX\",\"AU\",\"\",\"WWW.ASX.COM.AU\",\"ACTIVE\",\"20070924\",\"20210927\",\"20210927\",\"\",\"\"\n\
        \"XOCH\",\"XOCH\",\"OPRT\",\"ONECHICAGO, LLC\",\"\",\"\",\"NSPD\",\"\",\"US\",\"CHICAGO\",\"WWW.ONECHICAGO.COM\",\"EXPIRED\",\"20050627\",\"20210823\",\"20210823\",\"20210823\",\"\"\n";

    fn sample_entry() -> MicEntry {
        MicEntry {
            mic: "XTES".to_string(),
            operating_mic: "XTES".to_string(),
            name: "Test Exchange".to_string(),
            country_code: "AU".to_string(),
            city: Some("Sydney".to_string()),
            status: "ACTIVE".to_string(),
            expiry_date: None,
        }
    }

    // DB-level tests

    #[tokio::test]
    async fn db_insert_and_retrieve() {
        let pool = test_pool().await;
        db_upsert(&pool, &sample_entry()).await.unwrap();
        let got = db_get(&pool, "XTES").await.unwrap().unwrap();
        assert_eq!(got, sample_entry());
    }

    #[tokio::test]
    async fn db_get_missing_returns_none() {
        let pool = test_pool().await;
        assert!(db_get(&pool, "XXXX").await.unwrap().is_none());
    }

    #[tokio::test]
    async fn db_upsert_updates_existing_status() {
        let pool = test_pool().await;
        db_upsert(&pool, &sample_entry()).await.unwrap();
        let mut updated = sample_entry();
        updated.status = "EXPIRED".to_string();
        updated.expiry_date = Some("2024-01-31".to_string());
        db_upsert(&pool, &updated).await.unwrap();
        let got = db_get(&pool, "XTES").await.unwrap().unwrap();
        assert_eq!(got.status, "EXPIRED");
        assert_eq!(got.expiry_date, Some("2024-01-31".to_string()));
    }

    // Parsing tests

    #[test]
    fn parse_registry_handles_quotes_empty_cells_and_expiry() {
        let parsed = parse_registry(SAMPLE_CSV).unwrap();
        assert_eq!(parsed.len(), 3);

        let nyse = &parsed[0];
        assert_eq!(nyse.mic, "XNYS");
        assert_eq!(nyse.name, "NEW YORK STOCK EXCHANGE");
        assert_eq!(nyse.country_code, "US");
        assert_eq!(nyse.city, Some("NEW YORK".to_string()));
        assert_eq!(nyse.status, "ACTIVE");
        assert_eq!(nyse.expiry_date, None);

        // Empty CITY becomes None.
        assert_eq!(parsed[1].mic, "XASX");
        assert_eq!(parsed[1].city, None);

        // EXPIRED entry keeps its normalised expiry date.
        let expired = &parsed[2];
        assert_eq!(expired.status, "EXPIRED");
        assert_eq!(expired.expiry_date, Some("2021-08-23".to_string()));
    }

    #[test]
    fn parse_registry_errors_on_missing_column() {
        let csv = "\"MIC\",\"STATUS\"\n\"XNYS\",\"ACTIVE\"\n";
        assert!(matches!(parse_registry(csv).unwrap_err(), ImportError::Parse(_)));
    }

    #[test]
    fn parse_registry_errors_on_malformed_expiry() {
        let csv = "\"MIC\",\"OPERATING MIC\",\"MARKET NAME-INSTITUTION DESCRIPTION\",\
            \"ISO COUNTRY CODE (ISO 3166)\",\"CITY\",\"STATUS\",\"EXPIRY DATE\"\n\
            \"XNYS\",\"XNYS\",\"NEW YORK STOCK EXCHANGE\",\"US\",\"NEW YORK\",\"EXPIRED\",\"not-a-date\"\n";
        assert!(matches!(parse_registry(csv).unwrap_err(), ImportError::Parse(_)));
    }

    // Import

    #[tokio::test]
    async fn import_inserts_all_rows_and_is_idempotent() {
        let pool = test_pool().await;

        let first = import_from_content(&pool, SAMPLE_CSV).await.unwrap();
        assert_eq!(first, ImportSummary { imported: 3 });
        assert_eq!(db_list(&pool).await.unwrap().len(), 3);

        // Re-running upserts the same rows: no duplicates, count unchanged.
        let second = import_from_content(&pool, SAMPLE_CSV).await.unwrap();
        assert_eq!(second, ImportSummary { imported: 3 });
        assert_eq!(db_list(&pool).await.unwrap().len(), 3);
    }

    #[tokio::test]
    async fn import_reflects_status_changes_on_rerun() {
        let pool = test_pool().await;
        import_from_content(&pool, SAMPLE_CSV).await.unwrap();
        // XNYS becomes EXPIRED in a later publication.
        let changed = SAMPLE_CSV.replace(
            "\"WWW.NYSE.COM\",\"ACTIVE\",\"20050627\",\"20250428\",\"20250428\",\"\"",
            "\"WWW.NYSE.COM\",\"EXPIRED\",\"20050627\",\"20250428\",\"20250428\",\"20250501\"",
        );
        import_from_content(&pool, &changed).await.unwrap();
        let nyse = db_get(&pool, "XNYS").await.unwrap().unwrap();
        assert_eq!(nyse.status, "EXPIRED");
        assert_eq!(nyse.expiry_date, Some("2025-05-01".to_string()));
    }

    // API-level tests

    #[tokio::test]
    async fn api_list_returns_entries() {
        let pool = test_pool().await;
        db_upsert(&pool, &sample_entry()).await.unwrap();
        let resp = router()
            .with_state(pool)
            .oneshot(Request::builder().uri("/mic_registry").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let entries: Vec<MicEntry> = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(entries, vec![sample_entry()]);
    }

    #[tokio::test]
    async fn api_get_existing_returns_entry() {
        let pool = test_pool().await;
        db_upsert(&pool, &sample_entry()).await.unwrap();
        let resp = router()
            .with_state(pool)
            .oneshot(Request::builder().uri("/mic_registry/XTES").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let entry: MicEntry = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(entry, sample_entry());
    }

    #[tokio::test]
    async fn api_get_missing_returns_404() {
        let pool = test_pool().await;
        let resp = router()
            .with_state(pool)
            .oneshot(Request::builder().uri("/mic_registry/XXXX").body(Body::empty()).unwrap())
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
                    .uri("/mic_registry/import")
                    .body(Body::from(SAMPLE_CSV))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let summary: ImportSummary = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(summary, ImportSummary { imported: 3 });
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
                    .uri("/mic_registry/import")
                    .body(Body::from("\"MIC\",\"STATUS\"\n\"XNYS\",\"ACTIVE\"\n"))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY);
    }
}
