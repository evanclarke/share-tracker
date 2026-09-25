use crate::infra::db::write_tx;
use crate::infra::fetch::{FeedFetcher, LiveFeedFetcher, SharedFeedFetcher, fetch_feed};
use crate::infra::http::{self, ApiError, CrudEntity};
use axum::{
    Json, Router,
    extract::{Extension, State},
    routing::{get, post},
};
use chrono::NaiveDate;
use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;
use std::sync::Arc;

/// Source of the ISO 10383 Market Identifier Code registry: the official
/// ISO20022 published list. It carries no trading currency, timezone, or
/// settlement convention, so it is reference data only — used to validate that a
/// curated exchange's MIC is real and not expired, never to populate `exchanges`.
const MIC_REGISTRY_URL: &str =
    "https://www.iso20022.org/sites/default/files/ISO10383_MIC/ISO10383_MIC.csv";

/// The ISO STATUS of a MIC entry — a limited value set, so an enum with a DB
/// CHECK (0051) rather than free text.
#[derive(
    utoipa::ToSchema, Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, sqlx::Type,
)]
#[sqlx(rename_all = "UPPERCASE")]
#[serde(rename_all = "UPPERCASE")]
pub enum MicStatus {
    Active,
    Updated,
    Expired,
}

impl MicStatus {
    /// The feed's own STATUS string — the closed set the schema CHECK and the
    /// typed enum agree on. `None` for anything else, so an unknown value in
    /// the published registry fails the import loudly rather than being
    /// stored and silently failing the CHECK.
    fn from_iso(status: &str) -> Option<Self> {
        match status {
            "ACTIVE" => Some(Self::Active),
            "UPDATED" => Some(Self::Updated),
            "EXPIRED" => Some(Self::Expired),
            _ => None,
        }
    }
}

/// One ISO 10383 MIC. `status` is the ISO STATUS (`ACTIVE` | `UPDATED` |
/// `EXPIRED`); `expiry_date` is set only for expired entries. All fields are
/// surfaced by the read endpoints; `status`/`expiry_date` additionally drive the
/// exchange-MIC validation report.
#[derive(utoipa::ToSchema, Debug, Clone, PartialEq, Eq, Serialize, Deserialize, sqlx::FromRow)]
pub struct MicEntry {
    pub mic: String,
    pub operating_mic: String,
    pub name: String,
    pub country_code: String,
    pub city: Option<String>,
    pub status: MicStatus,
    pub expiry_date: Option<String>, // 'YYYY-MM-DD', present only when EXPIRED
}

#[derive(thiserror::Error, Debug)]
pub enum ImportError {
    /// Could not retrieve the published registry (network / HTTP error).
    #[error("could not fetch the MIC registry feed: {0}")]
    Fetch(String),
    /// The feed was not the expected ISO10383_MIC shape (missing column, bad row).
    #[error("the MIC registry feed is malformed: {0}")]
    Parse(String),
    #[error("MIC registry import write failed: {0}")]
    Db(#[from] sqlx::Error),
}

/// Outcome of an import run: how many registry rows were written (inserted or
/// updated). The registry mirrors the latest ISO publication, so every row in the
/// feed is upserted on every run.
#[derive(utoipa::ToSchema, Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[schema(as = MicImportSummary)]
pub struct ImportSummary {
    pub imported: usize,
}

impl CrudEntity for MicEntry {
    /// Keyed by MIC, not a rowid.
    type Key = String;
    const TABLE: &'static str = "mic_registry";
    const COLUMNS: &'static str =
        "mic, operating_mic, name, country_code, city, status, expiry_date";
    const KEY_COLUMN: &'static str = "mic";
    const ORDER_BY: &'static str = "mic";
    const NOUN: &'static str = "MIC registry entry";
}

pub fn router() -> Router<SqlitePool> {
    router_with(Arc::new(LiveFeedFetcher))
}

/// [`router`] over a caller-supplied transport. The production `router` is the
/// live one; the tests inject a stalled stub so the timeout path is exercised
/// with no socket in play.
fn router_with(fetcher: SharedFeedFetcher) -> Router<SqlitePool> {
    Router::new()
        .route("/mic_registry", get(http::list_handler::<MicEntry>))
        .route("/mic_registry/{mic}", get(http::get_handler::<MicEntry>))
        // Manual trigger for retries / missed runs. Read-only for clients otherwise.
        .route("/mic_registry/import", post(import))
        .layer(Extension(fetcher))
}

#[cfg(test)]
pub async fn db_list(pool: &SqlitePool) -> Result<Vec<MicEntry>, sqlx::Error> {
    http::crud_list(pool).await
}

#[cfg(test)]
pub async fn db_get(pool: &SqlitePool, mic: &str) -> Result<Option<MicEntry>, sqlx::Error> {
    http::crud_get(pool, mic.to_string()).await
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
    .bind(entry.status)
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

    let headers = reader
        .headers()
        .map_err(|e| ImportError::Parse(e.to_string()))?
        .clone();
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
        let raw_status = field(&rec, i_status);
        let status = MicStatus::from_iso(&raw_status).ok_or_else(|| {
            ImportError::Parse(format!("unknown STATUS {raw_status:?} for {mic}"))
        })?;
        out.push(MicEntry {
            mic,
            operating_mic: field(&rec, i_oper),
            name: field(&rec, i_name),
            country_code: field(&rec, i_country),
            city: opt(field(&rec, i_city)),
            status,
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
    let mut tx = write_tx(pool).await?;
    for entry in &entries {
        db_upsert(&mut *tx, entry).await?;
    }
    tx.commit().await?;
    Ok(ImportSummary {
        imported: entries.len(),
    })
}

/// Fetch the published registry from ISO and import it.
pub async fn run_import(pool: &SqlitePool) -> Result<ImportSummary, ImportError> {
    run_import_with(pool, &LiveFeedFetcher).await
}

/// [`run_import`] through a caller-supplied transport — the handler passes the
/// `Extension`'s fetcher, so a test can drive the timeout path.
async fn run_import_with(
    pool: &SqlitePool,
    fetcher: &dyn FeedFetcher,
) -> Result<ImportSummary, ImportError> {
    let content = fetch(fetcher, MIC_REGISTRY_URL).await?;
    import_from_content(pool, &content).await
}

/// The shared bounded fetch, mapped into this entity's error type — one
/// deadline and one body cap for every feed (see [`crate::infra::fetch`]).
async fn fetch(fetcher: &dyn FeedFetcher, url: &str) -> Result<String, ImportError> {
    fetch_feed(fetcher, url, None)
        .await
        .map_err(|e| ImportError::Fetch(e.to_string()))
}

/// Manually trigger the import. With a non-empty request body, imports that body
/// (a downloaded ISO10383_MIC CSV — useful for retries when ISO is unreachable);
/// with an empty body, fetches from ISO. Both share `import_from_content`.
async fn import(
    State(pool): State<SqlitePool>,
    Extension(fetcher): Extension<SharedFeedFetcher>,
    body: String,
) -> Result<Json<ImportSummary>, ApiError> {
    let result = if body.trim().is_empty() {
        run_import_with(&pool, fetcher.as_ref()).await
    } else {
        import_from_content(&pool, &body).await
    };
    Ok(Json(result?))
}

impl From<ImportError> for ApiError {
    fn from(e: ImportError) -> Self {
        match e {
            ImportError::Parse(msg) => {
                tracing::warn!(?msg, "MIC registry import rejected malformed feed");
                ApiError::unprocessable(format!("the MIC registry feed is malformed: {msg}"))
            }
            // The upstream fetch error is logged when the response is built.
            ImportError::Fetch(msg) => {
                ApiError::bad_gateway("could not fetch the MIC registry feed from its source", msg)
            }
            ImportError::Db(err) => err.into(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{ApiClient, test_pool};
    use axum::http::StatusCode;

    /// Client over this module's own routes.
    fn client(pool: &SqlitePool) -> ApiClient {
        ApiClient::over(router().with_state(pool.clone()))
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
            status: MicStatus::Active,
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

    /// `status` holds the ISO feed's closed set, so the CHECK added in 0051 —
    /// and the `MicStatus` enum the import parses it into — refuse a value the
    /// code cannot produce, while every value it does produce still writes.
    #[tokio::test]
    async fn db_check_constraint_rejects_an_unknown_status() {
        let pool = test_pool().await;
        let insert = |mic: &'static str, status: &'static str| {
            let pool = pool.clone();
            async move {
                sqlx::query(sqlx::AssertSqlSafe(format!(
                    "INSERT INTO mic_registry \
                         (mic, operating_mic, name, country_code, city, status, expiry_date) \
                     VALUES ({mic}, {mic}, 'Test Exchange', 'AU', 'Sydney', {status}, NULL)"
                )))
                .execute(&pool)
                .await
            }
        };
        // A STATUS the ISO publication does not define is rejected.
        assert!(insert("'XTES'", "'PENDING'").await.is_err());
        // Every value of the closed set is accepted.
        for (mic, status) in [
            ("'XAAA'", "'ACTIVE'"),
            ("'XBBB'", "'UPDATED'"),
            ("'XCCC'", "'EXPIRED'"),
        ] {
            assert!(
                insert(mic, status).await.is_ok(),
                "{status} must be an accepted status"
            );
        }
    }

    #[tokio::test]
    async fn db_upsert_updates_existing_status() {
        let pool = test_pool().await;
        db_upsert(&pool, &sample_entry()).await.unwrap();
        let mut updated = sample_entry();
        updated.status = MicStatus::Expired;
        updated.expiry_date = Some("2024-01-31".to_string());
        db_upsert(&pool, &updated).await.unwrap();
        let got = db_get(&pool, "XTES").await.unwrap().unwrap();
        assert_eq!(got.status, MicStatus::Expired);
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
        assert_eq!(nyse.status, MicStatus::Active);
        assert_eq!(nyse.expiry_date, None);

        // Empty CITY becomes None.
        assert_eq!(parsed[1].mic, "XASX");
        assert_eq!(parsed[1].city, None);

        // EXPIRED entry keeps its normalised expiry date.
        let expired = &parsed[2];
        assert_eq!(expired.status, MicStatus::Expired);
        assert_eq!(expired.expiry_date, Some("2021-08-23".to_string()));
    }

    #[test]
    fn parse_registry_errors_on_missing_column() {
        let csv = "\"MIC\",\"STATUS\"\n\"XNYS\",\"ACTIVE\"\n";
        assert!(matches!(
            parse_registry(csv).unwrap_err(),
            ImportError::Parse(_)
        ));
    }

    #[test]
    fn parse_registry_errors_on_malformed_expiry() {
        let csv = "\"MIC\",\"OPERATING MIC\",\"MARKET NAME-INSTITUTION DESCRIPTION\",\
            \"ISO COUNTRY CODE (ISO 3166)\",\"CITY\",\"STATUS\",\"EXPIRY DATE\"\n\
            \"XNYS\",\"XNYS\",\"NEW YORK STOCK EXCHANGE\",\"US\",\"NEW YORK\",\"EXPIRED\",\"not-a-date\"\n";
        assert!(matches!(
            parse_registry(csv).unwrap_err(),
            ImportError::Parse(_)
        ));
    }

    /// The STATUS is parsed into [`MicStatus`], so a value outside the closed
    /// set fails the import at parse time (naming the row) rather than being
    /// stored and rejected by the schema CHECK.
    #[test]
    fn parse_registry_rejects_an_unknown_status() {
        let csv = "\"MIC\",\"OPERATING MIC\",\"MARKET NAME-INSTITUTION DESCRIPTION\",\
            \"ISO COUNTRY CODE (ISO 3166)\",\"CITY\",\"STATUS\",\"EXPIRY DATE\"\n\
            \"XNYS\",\"XNYS\",\"NEW YORK STOCK EXCHANGE\",\"US\",\"NEW YORK\",\"PENDING\",\"\"\n";
        let error = parse_registry(csv).unwrap_err();
        assert!(
            error.to_string().contains("PENDING"),
            "the rejected STATUS is named: {error}"
        );
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
        assert_eq!(nyse.status, MicStatus::Expired);
        assert_eq!(nyse.expiry_date, Some("2025-05-01".to_string()));
    }

    // API-level tests

    #[tokio::test]
    async fn api_list_returns_entries() {
        let pool = test_pool().await;
        db_upsert(&pool, &sample_entry()).await.unwrap();
        let resp = client(&pool).get("/mic_registry").await;
        assert_eq!(resp.status, StatusCode::OK);
        let entries: Vec<MicEntry> = resp.json();
        assert_eq!(entries, vec![sample_entry()]);
    }

    #[tokio::test]
    async fn api_get_existing_returns_entry() {
        let pool = test_pool().await;
        db_upsert(&pool, &sample_entry()).await.unwrap();
        let resp = client(&pool).get("/mic_registry/XTES").await;
        assert_eq!(resp.status, StatusCode::OK);
        let entry: MicEntry = resp.json();
        assert_eq!(entry, sample_entry());
    }

    #[tokio::test]
    async fn api_get_missing_returns_404() {
        let pool = test_pool().await;
        let resp = client(&pool).get("/mic_registry/XXXX").await;
        assert_eq!(resp.status, StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn api_import_endpoint_invokes_import() {
        let pool = test_pool().await;
        let resp = client(&pool)
            .post_bytes("/mic_registry/import", None, SAMPLE_CSV)
            .await;
        assert_eq!(resp.status, StatusCode::OK);
        let summary: ImportSummary = resp.json();
        assert_eq!(summary, ImportSummary { imported: 3 });
        assert_eq!(db_list(&pool).await.unwrap().len(), 3);
    }

    #[tokio::test]
    async fn api_import_endpoint_rejects_malformed_feed() {
        let pool = test_pool().await;
        let resp = client(&pool)
            .post_raw(
                "/mic_registry/import",
                "\"MIC\",\"STATUS\"\n\"XNYS\",\"ACTIVE\"\n",
            )
            .await;
        assert_eq!(resp.status, StatusCode::UNPROCESSABLE_ENTITY);
    }

    /// SCENARIOS T-06, as for the RBA feed: the recorded string opens with the
    /// variant's own `#[error]` wording and carries the transport cause.
    #[tokio::test]
    async fn an_unreachable_feed_reports_the_feed_and_the_reason() {
        let url = crate::test_support::unreachable_url("ISO10383_MIC.csv");
        let error = fetch(&LiveFeedFetcher, &url)
            .await
            .expect_err("nothing is listening");
        let recorded = error.to_string();

        assert!(
            recorded.starts_with("could not fetch the MIC registry feed: "),
            "the variant's own message is missing: {recorded}"
        );
        assert!(recorded.contains(&url), "the feed is not named: {recorded}");
        assert!(
            recorded.to_lowercase().contains("connect"),
            "the underlying cause is missing: {recorded}"
        );
        assert!(
            !recorded.starts_with("Fetch("),
            "recorded as a Rust Debug string: {recorded}"
        );
    }

    /// The 2026-09-17 security finding end to end on this route: a stalled
    /// upstream answers the documented `502` rather than parking the request
    /// task. The stalled stub is the injection; the deadline under test is the
    /// one [`crate::infra::fetch::fetch_feed`] applies to every feed.
    ///
    /// The clock is paused so the stub's four-times-the-deadline sleep costs no
    /// real time — and so that, with the deadline removed, the stub completes
    /// and this fails on the parser's status instead of hanging.
    #[tokio::test]
    async fn a_stalled_feed_fetch_answers_the_documented_502() {
        use crate::infra::fetch::test_support::StalledFeedFetcher;

        let pool = test_pool().await;
        tokio::time::pause();
        let app = ApiClient::over(router_with(StalledFeedFetcher::shared()).with_state(pool));

        let resp = app.post_empty("/mic_registry/import").await;
        assert_eq!(resp.status, StatusCode::BAD_GATEWAY, "{}", resp.text());
        assert_eq!(
            resp.text(),
            "could not fetch the MIC registry feed from its source"
        );
    }
}
