//! Recognised currencies reference table — fiat (ISO 4217) and digital tokens
//! (ISO 24165) in one list.
//!
//! Populated by the `currency-import` maintenance job from two official feeds:
//! the SIX Group "List One" XML (the ISO 4217 maintenance agency's machine-
//! readable currency list) and the DTIF registry JSON (the ISO 24165 registration
//! authority's digital token list). Reference data only, keyed by `code`; rows are
//! written by the import, so the resource is read-only over HTTP. The import is
//! idempotent — re-importing a feed upserts in place, never duplicating rows.
//!
//! `minor_units` is informational only: stored monetary amounts remain
//! arbitrary-precision Decimal and are never rounded to a currency's minor unit.

use crate::infra::http::ApiError;
use axum::{
    Json, Router,
    extract::{Path, State},
    routing::{get, post},
};
use quick_xml::events::Event;
use quick_xml::reader::Reader;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sqlx::SqlitePool;
use std::collections::HashSet;

/// SIX Group "List One" — the official machine-readable ISO 4217 currency list.
/// SIX is the ISO 4217 Maintenance Agency (on behalf of the SNV) and publishes
/// this free of charge.
const ISO_4217_URL: &str =
    "https://www.six-group.com/dam/download/financial-information/data-center/iso-currrency/lists/list-one.xml";

/// DTIF registry snapshot — the ISO 24165 Digital Token Identifier registry,
/// maintained by the DTI Foundation (the ISO 24165 Registration Authority) and
/// refreshed monthly. The download requires Basic-auth credentials, supplied via
/// the `DTI_REGISTRY_USER_ID` / `DTI_REGISTRY_PASSWORD` environment variables; the
/// scheduled fetch is skipped (fiat still imports) when they are not configured.
const ISO_24165_URL: &str = "https://download.dtif.org/data.json";

/// Whether a currency is a fiat currency (ISO 4217) or a digital token
/// (ISO 24165). Limited value set → enum + DB CHECK constraint.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, sqlx::Type)]
pub enum CurrencyKind {
    Fiat,
    DigitalToken,
}

/// Which feed a currency row came from. Limited value set → enum + DB CHECK
/// constraint.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, sqlx::Type)]
pub enum CurrencySource {
    Iso4217,
    Iso24165,
}

/// One recognised currency. `code` is the ISO 4217 alphabetic code for fiat or the
/// ISO 24165 Digital Token Identifier (DTI) for a token. `numeric_code` is the
/// ISO 4217 numeric code (fiat only). `minor_units` is informational only.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, sqlx::FromRow)]
pub struct Currency {
    pub code: String,
    pub kind: CurrencyKind,
    pub numeric_code: Option<String>,
    pub name: String,
    pub short_name: Option<String>,
    pub minor_units: Option<i64>,
    pub source: CurrencySource,
}

#[derive(Debug)]
pub enum ImportError {
    /// Could not retrieve a published feed (network / HTTP error).
    Fetch(String),
    /// A feed was not the expected ISO 4217 XML / ISO 24165 JSON shape.
    Parse(String),
    Db(sqlx::Error),
}

impl From<sqlx::Error> for ImportError {
    fn from(e: sqlx::Error) -> Self {
        ImportError::Db(e)
    }
}

/// Outcome of an import run: how many currency rows were written (inserted or
/// updated). Every row in the feed is upserted on every run.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ImportSummary {
    pub imported: usize,
}

pub fn router() -> Router<SqlitePool> {
    Router::new()
        .route("/currencies", get(list))
        .route("/currencies/{code}", get(get_one))
        // Manual trigger for retries / missed runs. Read-only for clients otherwise.
        .route("/currencies/import", post(import))
}

pub async fn db_list(pool: &SqlitePool) -> Result<Vec<Currency>, sqlx::Error> {
    sqlx::query_as(
        "SELECT code, kind, numeric_code, name, short_name, minor_units, source \
         FROM currencies ORDER BY code",
    )
    .fetch_all(pool)
    .await
}

pub async fn db_get(pool: &SqlitePool, code: &str) -> Result<Option<Currency>, sqlx::Error> {
    sqlx::query_as(
        "SELECT code, kind, numeric_code, name, short_name, minor_units, source \
         FROM currencies WHERE code = ?",
    )
    .bind(code)
    .fetch_optional(pool)
    .await
}

/// Insert or update a currency by `code`. A currency's name / minor units can be
/// revised between publications, so the table tracks the latest feed via
/// `ON CONFLICT DO UPDATE`. Generic over the executor so it runs against either
/// the pool or an import transaction.
pub async fn db_upsert<'e, E>(executor: E, currency: &Currency) -> Result<(), sqlx::Error>
where
    E: sqlx::Executor<'e, Database = sqlx::Sqlite>,
{
    sqlx::query(
        "INSERT INTO currencies (code, kind, numeric_code, name, short_name, minor_units, source) \
         VALUES (?, ?, ?, ?, ?, ?, ?) \
         ON CONFLICT(code) DO UPDATE SET \
             kind         = excluded.kind, \
             numeric_code = excluded.numeric_code, \
             name         = excluded.name, \
             short_name   = excluded.short_name, \
             minor_units  = excluded.minor_units, \
             source       = excluded.source",
    )
    .bind(&currency.code)
    .bind(currency.kind)
    .bind(&currency.numeric_code)
    .bind(&currency.name)
    .bind(&currency.short_name)
    .bind(currency.minor_units)
    .bind(currency.source)
    .execute(executor)
    .await?;
    Ok(())
}

/// Parse the SIX Group ISO 4217 "List One" XML into fiat currency rows.
///
/// The document is `<ISO_4217><CcyTbl><CcyNtry>…`; each `<CcyNtry>` carries
/// `<CtryNm>` (country/entity), `<CcyNm>` (currency name), `<Ccy>` (alpha code),
/// `<CcyNbr>` (numeric code) and `<CcyMnrUnts>` (minor units). Entries with no
/// `<Ccy>` (e.g. ANTARCTICA) are skipped, and `<CcyMnrUnts>` of `N.A.` (gold, no
/// currency) becomes `None`. A code appears once per country (EUR many times), so
/// the first occurrence wins and the rest are deduplicated. A malformed minor-unit
/// value fails loudly rather than being silently dropped.
pub fn parse_iso4217(content: &str) -> Result<Vec<Currency>, ImportError> {
    let mut reader = Reader::from_str(content);
    let mut out = Vec::new();
    let mut seen = HashSet::new();

    let mut in_entry = false;
    let mut cur_tag: Option<Vec<u8>> = None;
    let (mut name, mut ccy, mut nbr, mut minor) =
        (None::<String>, None::<String>, None::<String>, None::<String>);

    loop {
        match reader.read_event() {
            Err(e) => return Err(ImportError::Parse(format!("XML error: {e}"))),
            Ok(Event::Eof) => break,
            Ok(Event::Start(e)) => {
                let qname = e.name();
                let tag = qname.as_ref();
                if tag == b"CcyNtry" {
                    in_entry = true;
                    cur_tag = None;
                    (name, ccy, nbr, minor) = (None, None, None, None);
                } else if in_entry {
                    cur_tag = Some(tag.to_vec());
                }
            }
            Ok(Event::Text(t)) if in_entry => {
                if let Some(tag) = &cur_tag {
                    let text =
                        t.unescape().map_err(|e| ImportError::Parse(e.to_string()))?.trim().to_string();
                    match tag.as_slice() {
                        b"CcyNm" => name = Some(text),
                        b"Ccy" => ccy = Some(text),
                        b"CcyNbr" => nbr = Some(text),
                        b"CcyMnrUnts" => minor = Some(text),
                        _ => {}
                    }
                }
            }
            Ok(Event::End(e)) => {
                let qname = e.name();
                let tag = qname.as_ref();
                if tag == b"CcyNtry" {
                    in_entry = false;
                    cur_tag = None;
                    if let Some(code) = ccy.take().filter(|c| !c.is_empty())
                        && seen.insert(code.clone())
                    {
                        let minor_units = match minor.take().as_deref() {
                            None | Some("") | Some("N.A.") => None,
                            Some(s) => Some(s.parse::<i64>().map_err(|e| {
                                ImportError::Parse(format!(
                                    "invalid minor units {s:?} for {code}: {e}"
                                ))
                            })?),
                        };
                        out.push(Currency {
                            code,
                            kind: CurrencyKind::Fiat,
                            numeric_code: nbr.take().filter(|c| !c.is_empty()),
                            name: name.take().unwrap_or_default(),
                            short_name: None,
                            minor_units,
                            source: CurrencySource::Iso4217,
                        });
                    }
                } else if cur_tag.as_deref() == Some(tag) {
                    cur_tag = None;
                }
            }
            _ => {}
        }
    }

    if out.is_empty() {
        return Err(ImportError::Parse("ISO 4217 feed contained no currency entries".into()));
    }
    Ok(out)
}

/// Parse the DTIF ISO 24165 registry JSON into digital token rows.
///
/// The document is `{ "records": [ { "Header": { "DTI": … }, "Informative": {
/// "LongName": …, "ShortNames": [ … ] } }, … ] }`. Records without a `Header.DTI`
/// (template / metadata rows) are skipped, as the DTIF tooling does. A token's
/// `code` is its DTI; `name` is the long name (falling back to the DTI), and
/// `short_name` is the first listed short name. A missing `records` array fails
/// loudly.
pub fn parse_iso24165(content: &str) -> Result<Vec<Currency>, ImportError> {
    let root: Value = serde_json::from_str(content)
        .map_err(|e| ImportError::Parse(format!("invalid DTI JSON: {e}")))?;
    let records = root
        .get("records")
        .and_then(Value::as_array)
        .ok_or_else(|| ImportError::Parse("DTI feed missing \"records\" array".into()))?;

    let mut out = Vec::new();
    let mut seen = HashSet::new();
    for rec in records {
        let dti = match rec.get("Header").and_then(|h| h.get("DTI")).and_then(Value::as_str) {
            Some(d) if !d.is_empty() => d.to_string(),
            _ => continue, // not a token record (no DTI) — skip like the DTIF tooling
        };
        if !seen.insert(dti.clone()) {
            continue;
        }
        let informative = rec.get("Informative");
        let name = informative
            .and_then(|i| i.get("LongName"))
            .and_then(Value::as_str)
            .map(str::to_string)
            .unwrap_or_else(|| dti.clone());
        let short_name = informative.and_then(|i| i.get("ShortNames")).and_then(first_short_name);
        out.push(Currency {
            code: dti,
            kind: CurrencyKind::DigitalToken,
            numeric_code: None,
            name,
            short_name,
            minor_units: None,
            source: CurrencySource::Iso24165,
        });
    }

    if out.is_empty() {
        return Err(ImportError::Parse("DTI feed contained no token records".into()));
    }
    Ok(out)
}

/// Pull the first short name out of a DTI `ShortNames` value, which may be a list
/// of strings, a list of `{ "ShortName": "…" }` objects, a single such object, or
/// a bare string.
fn first_short_name(value: &Value) -> Option<String> {
    fn one(v: &Value) -> Option<String> {
        v.as_str()
            .map(str::to_string)
            .or_else(|| v.get("ShortName").and_then(Value::as_str).map(str::to_string))
    }
    match value {
        Value::Array(items) => items.iter().find_map(one),
        other => one(other),
    }
}

/// Parse the supplied feed content and upsert every row in one transaction, so an
/// import either fully applies the feed or makes no change at all. The feed format
/// is detected from its first non-space character: `<` → ISO 4217 XML, `{` →
/// ISO 24165 JSON. Shared by the scheduled task and the manual-trigger endpoint.
pub async fn import_from_content(
    pool: &SqlitePool,
    content: &str,
) -> Result<ImportSummary, ImportError> {
    let trimmed = content.trim_start();
    let currencies = if trimmed.starts_with('<') {
        parse_iso4217(content)?
    } else if trimmed.starts_with('{') {
        parse_iso24165(content)?
    } else {
        return Err(ImportError::Parse(
            "unrecognised currency feed (expected ISO 4217 XML or ISO 24165 JSON)".into(),
        ));
    };

    let mut tx = pool.begin().await?;
    for currency in &currencies {
        db_upsert(&mut *tx, currency).await?;
    }
    tx.commit().await?;
    Ok(ImportSummary { imported: currencies.len() })
}

/// Fetch and import both feeds: the ISO 4217 fiat list (free) always, and the
/// ISO 24165 digital token registry only when DTIF Basic-auth credentials are
/// configured (it is skipped with a warning otherwise, so a missing credential
/// never blocks the fiat import). Returns the combined number of rows written.
pub async fn run_import(pool: &SqlitePool) -> Result<ImportSummary, ImportError> {
    let fiat = fetch(ISO_4217_URL, None).await?;
    let mut summary = import_from_content(pool, &fiat).await?;

    match (std::env::var("DTI_REGISTRY_USER_ID"), std::env::var("DTI_REGISTRY_PASSWORD")) {
        (Ok(user), Ok(pass)) => {
            let tokens = fetch(ISO_24165_URL, Some((&user, &pass))).await?;
            let token_summary = import_from_content(pool, &tokens).await?;
            summary.imported += token_summary.imported;
        }
        _ => tracing::warn!(
            "DTI_REGISTRY_USER_ID / DTI_REGISTRY_PASSWORD not set; skipping ISO 24165 digital token import"
        ),
    }

    Ok(summary)
}

/// GET a feed URL, optionally with HTTP Basic auth (the DTIF download requires it).
async fn fetch(url: &str, basic_auth: Option<(&str, &str)>) -> Result<String, ImportError> {
    let mut req = reqwest::Client::new().get(url);
    if let Some((user, pass)) = basic_auth {
        req = req.basic_auth(user, Some(pass));
    }
    let resp = req
        .send()
        .await
        .map_err(|e| ImportError::Fetch(e.to_string()))?
        .error_for_status()
        .map_err(|e| ImportError::Fetch(e.to_string()))?;
    resp.text().await.map_err(|e| ImportError::Fetch(e.to_string()))
}

async fn list(State(pool): State<SqlitePool>) -> Result<Json<Vec<Currency>>, ApiError> {
    db_list(&pool).await.map(Json).map_err(ApiError::from)
}

async fn get_one(
    State(pool): State<SqlitePool>,
    Path(code): Path<String>,
) -> Result<Json<Currency>, ApiError> {
    db_get(&pool, &code)
        .await
        .map_err(ApiError::from)?
        .map(Json)
        .ok_or(ApiError::NotFound)
}

/// Manually trigger the import. With a non-empty request body, imports that body
/// (a downloaded ISO 4217 XML or ISO 24165 JSON — useful for retries, or to load
/// the DTIF snapshot without server credentials); with an empty body, fetches from
/// the live sources. Both share `import_from_content`.
async fn import(
    State(pool): State<SqlitePool>,
    body: String,
) -> Result<Json<ImportSummary>, ApiError> {
    let result = if body.trim().is_empty() {
        run_import(&pool).await
    } else {
        import_from_content(&pool, &body).await
    };
    Ok(Json(result?))
}

impl From<ImportError> for ApiError {
    fn from(e: ImportError) -> Self {
        match e {
            ImportError::Parse(msg) => {
                tracing::warn!(%msg, "currency import rejected malformed feed");
                ApiError::unprocessable(format!("the currency feed is malformed: {msg}"))
            }
            // The upstream fetch error is logged when the response is built.
            ImportError::Fetch(msg) => ApiError::bad_gateway(
                "could not fetch the currency feed from its source",
                msg,
            ),
            ImportError::Db(err) => err.into(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::StatusCode;
    use crate::infra::db;
    use axum::{body::Body, http::Request};
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    async fn test_pool() -> SqlitePool {
        db::init(":memory:").await.unwrap()
    }

    /// A trimmed slice of the real List One layout: the `<ISO_4217>`/`<CcyTbl>`
    /// wrappers, AUD (minor units 2), a second EUR country row (same code → must
    /// deduplicate), gold XAU with `N.A.` minor units (→ None), and an entry with
    /// no `<Ccy>` (ANTARCTICA → skipped). `IsFund` on a `<CcyNm>` is ignored.
    const SAMPLE_XML: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<ISO_4217 Pblshd="2026-01-01">
  <CcyTbl>
    <CcyNtry>
      <CtryNm>AUSTRALIA</CtryNm>
      <CcyNm>Australian Dollar</CcyNm>
      <Ccy>AUD</Ccy>
      <CcyNbr>036</CcyNbr>
      <CcyMnrUnts>2</CcyMnrUnts>
    </CcyNtry>
    <CcyNtry>
      <CtryNm>GERMANY</CtryNm>
      <CcyNm>Euro</CcyNm>
      <Ccy>EUR</Ccy>
      <CcyNbr>978</CcyNbr>
      <CcyMnrUnts>2</CcyMnrUnts>
    </CcyNtry>
    <CcyNtry>
      <CtryNm>FRANCE</CtryNm>
      <CcyNm>Euro</CcyNm>
      <Ccy>EUR</Ccy>
      <CcyNbr>978</CcyNbr>
      <CcyMnrUnts>2</CcyMnrUnts>
    </CcyNtry>
    <CcyNtry>
      <CtryNm>ZZ08_Gold</CtryNm>
      <CcyNm>Gold</CcyNm>
      <Ccy>XAU</Ccy>
      <CcyNbr>959</CcyNbr>
      <CcyMnrUnts>N.A.</CcyMnrUnts>
    </CcyNtry>
    <CcyNtry>
      <CtryNm>ANTARCTICA</CtryNm>
      <CcyNm>No universal currency</CcyNm>
    </CcyNtry>
  </CcyTbl>
</ISO_4217>"#;

    /// A trimmed DTI registry: Bitcoin (ShortNames as `{ShortName}` objects), Ether
    /// (ShortNames as bare strings), and a metadata record with no `Header.DTI`
    /// (must be skipped).
    const SAMPLE_JSON: &str = r#"{
      "header": { "generated": "2026-05-01" },
      "records": [
        {
          "Header": { "DTI": "4H95J0R2X", "DTIType": 1 },
          "Informative": { "LongName": "Bitcoin", "ShortNames": [ { "ShortName": "BTC" }, { "ShortName": "XBT" } ] }
        },
        {
          "Header": { "DTI": "X9J9K872S", "DTIType": 1 },
          "Informative": { "LongName": "Ether", "ShortNames": [ "ETH" ] }
        },
        {
          "Header": { "templateVersion": "V1" }
        }
      ]
    }"#;

    fn aud() -> Currency {
        Currency {
            code: "AUD".to_string(),
            kind: CurrencyKind::Fiat,
            numeric_code: Some("036".to_string()),
            name: "Australian Dollar".to_string(),
            short_name: None,
            minor_units: Some(2),
            source: CurrencySource::Iso4217,
        }
    }

    // DB-level tests

    #[tokio::test]
    async fn db_insert_and_retrieve() {
        let pool = test_pool().await;
        db_upsert(&pool, &aud()).await.unwrap();
        let got = db_get(&pool, "AUD").await.unwrap().unwrap();
        assert_eq!(got, aud());
    }

    #[tokio::test]
    async fn db_get_missing_returns_none() {
        let pool = test_pool().await;
        assert!(db_get(&pool, "ZZZ").await.unwrap().is_none());
    }

    #[tokio::test]
    async fn db_upsert_updates_existing() {
        let pool = test_pool().await;
        db_upsert(&pool, &aud()).await.unwrap();
        let mut renamed = aud();
        renamed.name = "Aussie Dollar".to_string();
        db_upsert(&pool, &renamed).await.unwrap();
        let count: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM currencies WHERE code = 'AUD'")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(count, 1);
        assert_eq!(db_get(&pool, "AUD").await.unwrap().unwrap().name, "Aussie Dollar");
    }

    #[tokio::test]
    async fn db_kind_enum_constraint_enforced() {
        let pool = test_pool().await;
        let err = sqlx::query(
            "INSERT INTO currencies (code, kind, name, source) VALUES ('AUD', 'Bogus', 'x', 'Iso4217')",
        )
        .execute(&pool)
        .await;
        assert!(err.is_err(), "CHECK constraint should reject an unknown kind");
    }

    #[tokio::test]
    async fn db_source_enum_constraint_enforced() {
        let pool = test_pool().await;
        let err = sqlx::query(
            "INSERT INTO currencies (code, kind, name, source) VALUES ('AUD', 'Fiat', 'x', 'Bogus')",
        )
        .execute(&pool)
        .await;
        assert!(err.is_err(), "CHECK constraint should reject an unknown source");
    }

    // Parsing tests

    #[test]
    fn parse_iso4217_handles_minor_units_dedup_and_missing_code() {
        let parsed = parse_iso4217(SAMPLE_XML).unwrap();
        // AUD, EUR (deduplicated), XAU — ANTARCTICA (no Ccy) skipped.
        assert_eq!(parsed.len(), 3);

        assert_eq!(parsed[0], aud());

        let eur = &parsed[1];
        assert_eq!(eur.code, "EUR");
        assert_eq!(eur.numeric_code, Some("978".to_string()));

        // Gold: N.A. minor units → None.
        let xau = &parsed[2];
        assert_eq!(xau.code, "XAU");
        assert_eq!(xau.kind, CurrencyKind::Fiat);
        assert_eq!(xau.minor_units, None);
    }

    #[test]
    fn parse_iso4217_errors_on_malformed_minor_units() {
        let xml = "<ISO_4217><CcyTbl><CcyNtry><CcyNm>Bad</CcyNm><Ccy>BAD</Ccy>\
            <CcyNbr>999</CcyNbr><CcyMnrUnts>two</CcyMnrUnts></CcyNtry></CcyTbl></ISO_4217>";
        assert!(matches!(parse_iso4217(xml).unwrap_err(), ImportError::Parse(_)));
    }

    #[test]
    fn parse_iso24165_extracts_dti_names_and_skips_non_token_records() {
        let parsed = parse_iso24165(SAMPLE_JSON).unwrap();
        // Bitcoin + Ether; the metadata record (no DTI) is skipped.
        assert_eq!(parsed.len(), 2);

        let btc = &parsed[0];
        assert_eq!(btc.code, "4H95J0R2X");
        assert_eq!(btc.kind, CurrencyKind::DigitalToken);
        assert_eq!(btc.name, "Bitcoin");
        assert_eq!(btc.short_name, Some("BTC".to_string()));
        assert_eq!(btc.numeric_code, None);
        assert_eq!(btc.source, CurrencySource::Iso24165);

        // ShortNames given as bare strings still resolves.
        assert_eq!(parsed[1].short_name, Some("ETH".to_string()));
    }

    #[test]
    fn parse_iso24165_errors_when_records_missing() {
        assert!(matches!(parse_iso24165("{\"foo\": 1}").unwrap_err(), ImportError::Parse(_)));
    }

    // Import

    #[tokio::test]
    async fn import_iso4217_is_idempotent() {
        let pool = test_pool().await;

        // The DB is seeded with a baseline of currencies, so assert against the
        // change in row count rather than an absolute total.
        let first = import_from_content(&pool, SAMPLE_XML).await.unwrap();
        assert_eq!(first, ImportSummary { imported: 3 });
        let after_first = db_list(&pool).await.unwrap().len();
        assert!(db_get(&pool, "XAU").await.unwrap().is_some(), "import added the new XAU row");

        // Re-running upserts the same rows: no duplicates, count unchanged.
        let second = import_from_content(&pool, SAMPLE_XML).await.unwrap();
        assert_eq!(second, ImportSummary { imported: 3 });
        assert_eq!(db_list(&pool).await.unwrap().len(), after_first);
    }

    #[tokio::test]
    async fn import_iso24165_inserts_tokens() {
        let pool = test_pool().await;
        let summary = import_from_content(&pool, SAMPLE_JSON).await.unwrap();
        assert_eq!(summary, ImportSummary { imported: 2 });
        let btc = db_get(&pool, "4H95J0R2X").await.unwrap().unwrap();
        assert_eq!(btc.kind, CurrencyKind::DigitalToken);
        assert_eq!(btc.name, "Bitcoin");
    }

    #[tokio::test]
    async fn import_both_feeds_coexist_in_one_table() {
        let pool = test_pool().await;
        import_from_content(&pool, SAMPLE_XML).await.unwrap();
        import_from_content(&pool, SAMPLE_JSON).await.unwrap();
        // Fiat and tokens live in the same table, keyed by code.
        assert_eq!(db_get(&pool, "AUD").await.unwrap().unwrap().kind, CurrencyKind::Fiat);
        assert_eq!(db_get(&pool, "XAU").await.unwrap().unwrap().kind, CurrencyKind::Fiat);
        assert_eq!(
            db_get(&pool, "4H95J0R2X").await.unwrap().unwrap().kind,
            CurrencyKind::DigitalToken
        );
    }

    #[tokio::test]
    async fn import_rejects_unrecognised_feed() {
        let pool = test_pool().await;
        assert!(matches!(
            import_from_content(&pool, "code,name\nAUD,Dollar").await.unwrap_err(),
            ImportError::Parse(_)
        ));
    }

    // API-level tests

    #[tokio::test]
    async fn api_list_returns_currencies() {
        let pool = test_pool().await;
        db_upsert(&pool, &aud()).await.unwrap();
        let resp = router()
            .with_state(pool)
            .oneshot(Request::builder().uri("/currencies").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let currencies: Vec<Currency> = serde_json::from_slice(&bytes).unwrap();
        // The DB is seeded with a baseline; the inserted AUD must be among them.
        assert!(currencies.contains(&aud()));
    }

    #[tokio::test]
    async fn api_get_existing_returns_currency() {
        let pool = test_pool().await;
        db_upsert(&pool, &aud()).await.unwrap();
        let resp = router()
            .with_state(pool)
            .oneshot(Request::builder().uri("/currencies/AUD").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let currency: Currency = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(currency, aud());
    }

    #[tokio::test]
    async fn api_get_missing_returns_404() {
        let pool = test_pool().await;
        let resp = router()
            .with_state(pool)
            .oneshot(Request::builder().uri("/currencies/ZZZ").body(Body::empty()).unwrap())
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
                    .uri("/currencies/import")
                    .body(Body::from(SAMPLE_XML))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let summary: ImportSummary = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(summary, ImportSummary { imported: 3 });
        // Import ran against the seeded baseline: the new XAU row is now present.
        assert!(db_get(&pool, "XAU").await.unwrap().is_some());
    }

    #[tokio::test]
    async fn api_import_endpoint_rejects_malformed_feed() {
        let pool = test_pool().await;
        let resp = router()
            .with_state(pool)
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/currencies/import")
                    .body(Body::from("not a currency feed"))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY);
    }
}
