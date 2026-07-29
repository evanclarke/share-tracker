use crate::infra::http::{self, ApiError, CrudEntity};
use axum::{
    Json, Router,
    extract::State,
    routing::{get, post},
};
use chrono::NaiveDate;
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;

use crate::infra::decimal::Money;

/// Source of the official monthly foreign exchange rates used for AUD tax
/// conversion: the RBA's F11 "Exchange Rates" CSV (the rates the ATO directs
/// taxpayers to use). Each currency column is headed `A$1=<code>` and holds
/// foreign currency units per 1 AUD (foreign-per-AUD), so AUD = foreign / rate.
const RBA_FX_RATES_URL: &str = "https://www.rba.gov.au/statistics/tables/csv/f11-data.csv";

/// An official monthly foreign exchange rate. `rate` is foreign currency units
/// per 1 AUD (foreign-per-AUD), so AUD = foreign / rate.
#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct RbaFxRate {
    pub id: i64,
    pub currency: String,
    pub month: String, // 'YYYY-MM'
    #[sqlx(try_from = "Money")]
    pub rate: Decimal,
}

#[derive(thiserror::Error, Debug)]
pub enum ImportError {
    /// Could not retrieve the published rates (network / HTTP error).
    #[error("could not fetch the RBA FX rate feed: {0}")]
    Fetch(String),
    /// The feed was not the expected RBA F11 shape (missing header, bad rate).
    #[error("the RBA FX rate feed is malformed: {0}")]
    Parse(String),
    #[error("RBA FX rate import write failed: {0}")]
    Db(#[from] sqlx::Error),
}

/// Outcome of an import run: how many new rows were inserted vs already present.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ImportSummary {
    pub inserted: usize,
    pub skipped: usize,
}

/// What the manual import endpoint returns: the import summary plus the
/// provisional-snapshot true-up that followed it (absent when the import
/// added no new rows, so no snapshot could have improved).
#[derive(Debug, Serialize)]
pub struct ImportOutcome {
    #[serde(flatten)]
    pub summary: ImportSummary,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub snapshot_true_up: Option<crate::reports::snapshot::RegenerateSummary>,
}

/// After an import added new (currency, month) rows, regenerate the stored
/// provisional snapshots in the same run, so a newly published real rate
/// replaces the fallback valuations without waiting for the daily snapshot
/// job. Shared by the scheduled `rba-fx-import` job and the manual
/// `POST /rba_fx_rates/import`. `None` when nothing was inserted.
pub async fn true_up_provisional_snapshots(
    pool: &SqlitePool,
    summary: &ImportSummary,
) -> Result<Option<crate::reports::snapshot::RegenerateSummary>, sqlx::Error> {
    if summary.inserted == 0 {
        return Ok(None);
    }
    let true_up =
        crate::reports::snapshot::regenerate_provisional(pool, chrono::Utc::now()).await?;
    tracing::info!(
        regenerated = true_up.regenerated.len(),
        blocked = true_up.blocked.len(),
        "provisional-snapshot true-up after FX import"
    );
    Ok(Some(true_up))
}

impl CrudEntity for RbaFxRate {
    type Key = i64;
    const TABLE: &'static str = "rba_fx_rates";
    const COLUMNS: &'static str = "id, currency, month, rate";
    const ORDER_BY: &'static str = "currency, month";
    const NOUN: &'static str = "FX rate";
}

pub fn router() -> Router<SqlitePool> {
    Router::new()
        .route("/rba_fx_rates", get(http::list_handler::<RbaFxRate>))
        .route("/rba_fx_rates/{id}", get(http::get_handler::<RbaFxRate>))
        // Manual trigger for retries / missed runs. Read-only for clients otherwise.
        .route("/rba_fx_rates/import", post(import))
}

#[cfg(test)]
pub async fn db_list(pool: &SqlitePool) -> Result<Vec<RbaFxRate>, sqlx::Error> {
    http::crud_list(pool).await
}

#[cfg(test)]
pub async fn db_get(pool: &SqlitePool, id: i64) -> Result<Option<RbaFxRate>, sqlx::Error> {
    http::crud_get(pool, id).await
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
        "INSERT INTO rba_fx_rates (currency, month, rate) VALUES (?, ?, ?) \
         ON CONFLICT(currency, month) DO NOTHING",
    )
    .bind(currency)
    .bind(month)
    .bind(Money(rate))
    .execute(pool)
    .await?;
    Ok(result.rows_affected() > 0)
}

/// Parse the RBA F11 "Exchange Rates" CSV into `(currency, month, rate)` tuples.
///
/// The file has a leading BOM, a `Title` row whose columns are `A$1=<code>` (and
/// a non-currency trade-weighted-index column), several other metadata rows, then
/// monthly data rows keyed by an end-of-month date (`DD-Mon-YYYY`). For each data
/// row we emit one tuple per currency column that has a value; the month is the
/// date's year-month. Fails loudly on a malformed rate rather than dropping it —
/// a missing rate would later surface as an un-substitutable AUD conversion.
pub fn parse_rates(content: &str) -> Result<Vec<(String, String, Decimal)>, ImportError> {
    let content = content.strip_prefix('\u{feff}').unwrap_or(content);

    // Currency code per column, aligned to fields[1..] (None for non-currency columns).
    let mut currencies: Option<Vec<Option<String>>> = None;
    let mut out = Vec::new();

    for line in content.lines() {
        let line = line.trim_end();
        if line.is_empty() {
            continue;
        }
        let fields: Vec<&str> = line.split(',').map(str::trim).collect();

        if fields[0].eq_ignore_ascii_case("Title") {
            currencies = Some(
                fields[1..]
                    .iter()
                    .map(|h| h.strip_prefix("A$1=").map(|c| c.to_uppercase()))
                    .collect(),
            );
            continue;
        }

        // Data rows are keyed by an end-of-month date; every other metadata row
        // (Description, Units, Source, …) fails this parse and is skipped.
        let Ok(date) = NaiveDate::parse_from_str(fields[0], "%d-%b-%Y") else {
            continue;
        };
        let currencies = currencies.as_ref().ok_or_else(|| {
            ImportError::Parse("data row encountered before the Title header row".into())
        })?;
        let month = date.format("%Y-%m").to_string();

        for (col, currency) in currencies.iter().enumerate() {
            let Some(currency) = currency else { continue };
            let Some(value) = fields.get(col + 1) else {
                continue;
            };
            if value.is_empty() {
                continue;
            }
            let rate: Decimal = value.parse().map_err(|e| {
                ImportError::Parse(format!(
                    "invalid {currency} rate {value:?} for {month}: {e}"
                ))
            })?;
            out.push((currency.clone(), month.clone(), rate));
        }
    }

    if currencies.is_none() {
        return Err(ImportError::Parse(
            "no `Title` header row found in feed".into(),
        ));
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
    let mut summary = ImportSummary {
        inserted: 0,
        skipped: 0,
    };
    for (currency, month, rate) in rates {
        if db_import_rate(pool, &currency, &month, rate).await? {
            summary.inserted += 1;
        } else {
            summary.skipped += 1;
        }
    }
    Ok(summary)
}

/// Fetch the published rates from the RBA and import them.
pub async fn run_import(pool: &SqlitePool) -> Result<ImportSummary, ImportError> {
    let content = fetch_rates(RBA_FX_RATES_URL).await?;
    import_from_content(pool, &content).await
}

async fn fetch_rates(url: &str) -> Result<String, ImportError> {
    let resp = reqwest::get(url)
        .await
        .map_err(|e| ImportError::Fetch(e.to_string()))?
        .error_for_status()
        .map_err(|e| ImportError::Fetch(e.to_string()))?;
    resp.text()
        .await
        .map_err(|e| ImportError::Fetch(e.to_string()))
}

/// Manually trigger the import. With a non-empty request body, imports that body
/// (a downloaded F11 CSV — useful for retries when the RBA endpoint is
/// unreachable); with an empty body, fetches from the RBA. Both share
/// `import_from_content`, and both true up provisional snapshots when new
/// rates landed.
async fn import(
    State(pool): State<SqlitePool>,
    body: String,
) -> Result<Json<ImportOutcome>, ApiError> {
    let result = if body.trim().is_empty() {
        run_import(&pool).await
    } else {
        import_from_content(&pool, &body).await
    };
    let summary = result?;
    let snapshot_true_up = true_up_provisional_snapshots(&pool, &summary).await?;
    Ok(Json(ImportOutcome {
        summary,
        snapshot_true_up,
    }))
}

impl From<ImportError> for ApiError {
    fn from(e: ImportError) -> Self {
        match e {
            ImportError::Parse(msg) => {
                tracing::warn!(%msg, "RBA FX rate import rejected malformed feed");
                ApiError::unprocessable(format!("the RBA FX rate feed is malformed: {msg}"))
            }
            // The upstream fetch error is logged when the response is built.
            ImportError::Fetch(msg) => {
                ApiError::bad_gateway("could not fetch the RBA FX rate feed from its source", msg)
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

    /// A trimmed slice of the real RBA F11 layout: BOM, metadata rows, a Title row
    /// with a non-currency Index column to skip, and two monthly data rows (the
    /// first has an empty PHP cell to skip).
    const SAMPLE_CSV: &str = "\u{feff}F11 EXCHANGE RATES\n\
        Title,A$1=USD,Trade-weighted Index May 1970 = 100,A$1=GBP,A$1=PHP\n\
        Description,AUD/USD,Index,AUD/GBP,AUD/PHP\n\
        Units,USD,Index,GBP,PHP\n\
        Series ID,FXRUSD,FXRTWI,FXRUKPS,FXRPHP\n\
        \n\
        29-Jan-2010,0.8909,69.2,0.5523,\n\
        26-Feb-2010,0.8899,69.5,0.5826,40.10\n";

    // DB-level tests

    #[tokio::test]
    async fn db_insert_and_retrieve() {
        let pool = test_pool().await;
        assert!(
            db_import_rate(&pool, "USD", "2024-01", "1.5".parse().unwrap())
                .await
                .unwrap()
        );
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
        assert!(
            db_import_rate(&pool, "USD", "2024-01", "1.5".parse().unwrap())
                .await
                .unwrap()
        );
        // Same (currency, month): no new row, existing left unchanged.
        assert!(
            !db_import_rate(&pool, "USD", "2024-01", "1.6".parse().unwrap())
                .await
                .unwrap()
        );

        let rows = db_list(&pool).await.unwrap();
        assert_eq!(rows.len(), 1, "(currency, month) must be unique");
        assert_eq!(
            rows[0].rate,
            "1.5".parse::<Decimal>().unwrap(),
            "existing row unchanged"
        );

        // A different month is a distinct row.
        db_import_rate(&pool, "USD", "2024-02", "1.7".parse().unwrap())
            .await
            .unwrap();
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
    async fn parse_rates_parses_f11_skipping_index_and_empty_cells() {
        let parsed = parse_rates(SAMPLE_CSV).unwrap();
        // Per row, per currency column with a value. The Index column is dropped,
        // and Jan's empty PHP cell is skipped.
        assert_eq!(
            parsed,
            vec![
                (
                    "USD".to_string(),
                    "2010-01".to_string(),
                    "0.8909".parse().unwrap()
                ),
                (
                    "GBP".to_string(),
                    "2010-01".to_string(),
                    "0.5523".parse().unwrap()
                ),
                (
                    "USD".to_string(),
                    "2010-02".to_string(),
                    "0.8899".parse().unwrap()
                ),
                (
                    "GBP".to_string(),
                    "2010-02".to_string(),
                    "0.5826".parse().unwrap()
                ),
                (
                    "PHP".to_string(),
                    "2010-02".to_string(),
                    "40.10".parse().unwrap()
                ),
            ]
        );
    }

    #[tokio::test]
    async fn parse_rates_rejects_malformed_rate() {
        let csv = "Title,A$1=USD\n29-Jan-2010,not-a-number\n";
        assert!(matches!(
            parse_rates(csv).unwrap_err(),
            ImportError::Parse(_)
        ));
    }

    #[tokio::test]
    async fn parse_rates_errors_without_title_header() {
        let csv = "F11 EXCHANGE RATES\nDescription,foo\n";
        assert!(matches!(
            parse_rates(csv).unwrap_err(),
            ImportError::Parse(_)
        ));
    }

    // Import idempotency

    #[tokio::test]
    async fn import_is_idempotent() {
        let pool = test_pool().await;

        let first = import_from_content(&pool, SAMPLE_CSV).await.unwrap();
        assert_eq!(
            first,
            ImportSummary {
                inserted: 5,
                skipped: 0
            }
        );

        // Re-running stores no duplicates and leaves existing rows unchanged, even
        // if the feed carries a different rate for an existing (currency, month).
        let altered = SAMPLE_CSV.replace("0.8909", "9.9999");
        let second = import_from_content(&pool, &altered).await.unwrap();
        assert_eq!(
            second,
            ImportSummary {
                inserted: 0,
                skipped: 5
            }
        );

        let rows = db_list(&pool).await.unwrap();
        assert_eq!(rows.len(), 5, "no duplicates created");
        let usd_jan = rows
            .iter()
            .find(|r| r.currency == "USD" && r.month == "2010-01")
            .unwrap();
        assert_eq!(
            usd_jan.rate,
            "0.8909".parse::<Decimal>().unwrap(),
            "existing row unchanged"
        );
    }

    #[tokio::test]
    async fn import_adds_only_new_rows_on_rerun() {
        let pool = test_pool().await;
        import_from_content(&pool, SAMPLE_CSV).await.unwrap();
        // Feed now includes a new month alongside the existing rows.
        let extended = format!("{SAMPLE_CSV}31-Mar-2010,0.9159,71.7,0.6072,42.00\n");
        let summary = import_from_content(&pool, &extended).await.unwrap();
        assert_eq!(
            summary,
            ImportSummary {
                inserted: 3,
                skipped: 5
            }
        );
        assert_eq!(db_list(&pool).await.unwrap().len(), 8);
    }

    // API-level tests

    #[tokio::test]
    async fn api_list_returns_rates() {
        let pool = test_pool().await;
        db_import_rate(&pool, "USD", "2024-01", "1.5".parse().unwrap())
            .await
            .unwrap();
        let resp = client(&pool).get("/rba_fx_rates").await;
        assert_eq!(resp.status, StatusCode::OK);
        let rates: Vec<RbaFxRate> = resp.json();
        assert_eq!(rates.len(), 1);
        assert_eq!(rates[0].currency, "USD");
    }

    #[tokio::test]
    async fn api_get_existing_returns_rate() {
        let pool = test_pool().await;
        db_import_rate(&pool, "USD", "2024-01", "1.5".parse().unwrap())
            .await
            .unwrap();
        let id = db_list(&pool).await.unwrap()[0].id;
        let resp = client(&pool).get(format!("/rba_fx_rates/{id}")).await;
        assert_eq!(resp.status, StatusCode::OK);
        let rate: RbaFxRate = resp.json();
        assert_eq!(rate.currency, "USD");
        assert_eq!(rate.rate, "1.5".parse::<Decimal>().unwrap());
    }

    #[tokio::test]
    async fn api_get_missing_returns_404() {
        let pool = test_pool().await;
        let resp = client(&pool).get("/rba_fx_rates/999").await;
        assert_eq!(resp.status, StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn api_import_endpoint_invokes_import() {
        let pool = test_pool().await;
        let resp = client(&pool)
            .post_bytes("/rba_fx_rates/import", None, SAMPLE_CSV)
            .await;
        assert_eq!(resp.status, StatusCode::OK);
        let summary: ImportSummary = resp.json();
        assert_eq!(
            summary,
            ImportSummary {
                inserted: 5,
                skipped: 0
            }
        );
        assert_eq!(db_list(&pool).await.unwrap().len(), 5);
    }

    /// A successful import that lands new rates regenerates the stored
    /// provisional snapshots in the same run (the manual endpoint here; the
    /// weekly `rba-fx-import` job shares `true_up_provisional_snapshots`), so
    /// a fallback-valued snapshot is finalised the moment its real rate
    /// arrives.
    #[tokio::test]
    async fn api_import_regenerates_provisional_snapshots_in_the_same_run() {
        let pool = test_pool().await;
        let june4 = chrono::NaiveDate::from_ymd_opt(2026, 6, 4).unwrap();
        // A USD holding valued for 2026-06-04 while only May's rate exists:
        // the snapshot generates provisional.
        crate::test_support::listing(1)
            .ticker("ICE")
            .name("ICE")
            .mic("XNYS")
            .security_type(crate::entities::listing::SecurityType::Share)
            .currency("USD")
            .insert(&pool)
            .await;
        crate::test_support::buy(1, 1)
            .date(chrono::NaiveDate::from_ymd_opt(2024, 1, 15).unwrap())
            .settlement(chrono::NaiveDate::from_ymd_opt(2024, 1, 17).unwrap())
            .qty("10".parse().unwrap())
            .price("100".parse().unwrap())
            .currency("USD")
            .insert(&pool)
            .await;
        db_import_rate(&pool, "USD", "2024-01", "2".parse().unwrap())
            .await
            .unwrap();
        db_import_rate(&pool, "USD", "2026-05", "2".parse().unwrap())
            .await
            .unwrap();
        crate::test_support::closing_price(1, june4)
            .price("141.50")
            .insert(&pool)
            .await;
        crate::reports::snapshot::generate(&pool, june4, chrono::Utc::now())
            .await
            .unwrap();
        let metas = crate::reports::snapshot::db_list(&pool, None, None, None)
            .await
            .unwrap();
        assert!(metas.iter().all(|m| m.provisional), "setup: provisional");

        // The import lands June's real rate; the response reports the true-up
        // and the stored snapshots are final afterwards.
        let resp = client(&pool)
            .post_raw("/rba_fx_rates/import", "Title,A$1=USD\n30-Jun-2026,2.5\n")
            .await;
        assert_eq!(resp.status, StatusCode::OK);
        let outcome: serde_json::Value = resp.json();
        assert_eq!(outcome["inserted"], 1);
        assert_eq!(
            outcome["snapshot_true_up"]["regenerated"],
            serde_json::json!(["2026-06-04"])
        );
        let metas = crate::reports::snapshot::db_list(&pool, None, None, None)
            .await
            .unwrap();
        assert!(
            metas.iter().all(|m| !m.provisional),
            "finalised by the true-up"
        );

        // A re-import with nothing new performs no true-up.
        let resp = client(&pool)
            .post_raw("/rba_fx_rates/import", "Title,A$1=USD\n30-Jun-2026,2.5\n")
            .await;
        let outcome: serde_json::Value = resp.json();
        assert_eq!(outcome["inserted"], 0);
        assert!(
            outcome.get("snapshot_true_up").is_none(),
            "no new rates, no true-up"
        );
    }

    #[tokio::test]
    async fn api_import_endpoint_rejects_malformed_feed() {
        let pool = test_pool().await;
        let resp = client(&pool)
            .post_raw("/rba_fx_rates/import", "Title,A$1=USD\n29-Jan-2010,oops\n")
            .await;
        assert_eq!(resp.status, StatusCode::UNPROCESSABLE_ENTITY);
    }
}
