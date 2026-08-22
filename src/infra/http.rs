//! HTTP helpers shared by entity and report handlers.

use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use sqlx::error::ErrorKind;
use sqlx::sqlite::SqliteRow;
use sqlx::{Row, Sqlite, SqlitePool};

/// A boxed error source carried by [`ApiError::Internal`].
pub type BoxError = Box<dyn std::error::Error + Send + Sync + 'static>;

/// The error type every handler returns: it implements `IntoResponse`, so a
/// handler is `Result<_, ApiError>` and propagates failures with `?` instead
/// of `map_err(|_| StatusCode::…)` — which silently discarded the error a
/// lower layer (e.g. `parse_dec`) carefully named.
///
/// - [`ApiError::Internal`] is an unexpected server fault: it responds `500`
///   with an empty body (internal details must never reach the client), and
///   the wrapped error is logged via `tracing::error!` when the response is
///   built — no failure is swallowed unlogged.
/// - [`ApiError::Unprocessable`] is the client's fault: `422` with a short,
///   plain-text body explaining the rejection, so the web UI can show *why*
///   a request failed instead of a bare "HTTP 422".
/// - [`ApiError::NotFound`] is a plain `404` with an empty body.
///
/// Per-entity error enums stay (they document each operation's failure modes
/// and keep DB-level tests precise) and convert via `impl From<EntityError>
/// for ApiError` next to the enum — the handler itself never matches.
#[derive(thiserror::Error, Debug)]
pub enum ApiError {
    /// 500 — logged at error level when the response is built; empty body.
    #[error("{0}")]
    Internal(#[source] BoxError),
    /// 422 with a plain-text explanation of what the request got wrong.
    #[error("{0}")]
    Unprocessable(String),
    /// 400 — the request itself is malformed (e.g. an unparseable date in
    /// the path), as opposed to well-formed data the model rejects (422).
    #[error("{0}")]
    BadRequest(String),
    /// 502 — an upstream feed (MIC registry, currency list, RBA rates)
    /// could not be fetched. The underlying fetch error is logged at error
    /// level when the response is built; the body carries the short
    /// user-facing explanation only.
    #[error("{body}: {source}")]
    BadGateway { body: String, source: BoxError },
    /// 413 — an upload exceeds the size ceiling.
    #[error("{0}")]
    PayloadTooLarge(String),
    /// 500 whose body carries the failure text — the manual job trigger
    /// (`POST /jobs/{name}`) alone.
    ///
    /// Deliberately unlike [`ApiError::Internal`], whose 500 is empty because
    /// an unexpected internal fault must not leak implementation detail: a
    /// job's failure is the *operator's own diagnostic*, the very string
    /// `job_runs.error` keeps and the Jobs screen's Error column already
    /// shows. Withholding it only meant the toast the operator reads first
    /// said "HTTP 500" while the reason sat one reload away (SCENARIOS T-10).
    /// The reason is logged with its job name when the response is built, so
    /// nothing is swallowed.
    #[error("{reason}")]
    JobFailed { job: String, reason: String },
    /// 404, empty body — entity GETs, where the URL itself names what is
    /// missing.
    #[error("not found")]
    NotFound,
    /// 404 with a plain-text reason for the UI toast — operation endpoints
    /// (exercise, reinvest, delete), where the missing prerequisite deserves
    /// naming. Construct via [`ApiError::not_found`].
    #[error("{0}")]
    NotFoundWithReason(String),
    /// 401 — no valid session cookie or bearer token, or a login attempt
    /// with the wrong credentials. Never logged as an error: an
    /// unauthenticated request is expected traffic (a browser without a
    /// session yet, a probe), not a server fault; the auth layer logs a
    /// failed login itself, naming the peer.
    #[error("{0}")]
    Unauthorized(String),
}

impl ApiError {
    /// A 422 with the given plain-text explanation.
    pub fn unprocessable(msg: impl Into<String>) -> Self {
        ApiError::Unprocessable(msg.into())
    }

    /// A 400 with the given plain-text explanation.
    pub fn bad_request(msg: impl Into<String>) -> Self {
        ApiError::BadRequest(msg.into())
    }

    /// A 500 wrapping any error source (logged when the response is built).
    pub fn internal(err: impl Into<BoxError>) -> Self {
        ApiError::Internal(err.into())
    }

    /// A 502 with a short user-facing body, wrapping the upstream fetch
    /// error (logged when the response is built).
    pub fn bad_gateway(body: impl Into<String>, source: impl Into<BoxError>) -> Self {
        ApiError::BadGateway {
            body: body.into(),
            source: source.into(),
        }
    }

    /// A 500 whose body carries a failed job's own error text (see
    /// [`ApiError::JobFailed`]).
    pub fn job_failed(job: impl Into<String>, reason: impl Into<String>) -> Self {
        ApiError::JobFailed {
            job: job.into(),
            reason: reason.into(),
        }
    }

    /// A 404 whose body names what was missing.
    pub fn not_found(msg: impl Into<String>) -> Self {
        ApiError::NotFoundWithReason(msg.into())
    }

    /// A 401 with the given plain-text explanation.
    pub fn unauthorized(msg: impl Into<String>) -> Self {
        ApiError::Unauthorized(msg.into())
    }
}

/// The shared outcome of a DELETE: `204` when a row was removed, `404` naming
/// the missing row when there was nothing to remove.
///
/// A DELETE is an operation endpoint — unlike a GET, whose URL is still on
/// screen when the 404 arrives, a delete is fired from a list row and its
/// failure surfaces only as a toast. So the body always names what was
/// missing ("no income with that id") rather than leaving the web UI to show
/// a bare "HTTP 404". Every entity delete goes through this helper, or (where
/// the delete has its own outcome enum for the referenced-row cases) returns
/// the same wording by hand.
pub fn deleted(found: bool, noun: &str) -> Result<StatusCode, ApiError> {
    if found {
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err(ApiError::not_found(format!("no {noun} with that id")))
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        match self {
            ApiError::Internal(err) => {
                tracing::error!(error = %err, "request failed with an internal error");
                StatusCode::INTERNAL_SERVER_ERROR.into_response()
            }
            ApiError::Unprocessable(body) => {
                (StatusCode::UNPROCESSABLE_ENTITY, body).into_response()
            }
            ApiError::BadRequest(body) => (StatusCode::BAD_REQUEST, body).into_response(),
            ApiError::PayloadTooLarge(body) => {
                (StatusCode::PAYLOAD_TOO_LARGE, body).into_response()
            }
            ApiError::BadGateway { body, source } => {
                tracing::error!(error = %source, "upstream feed fetch failed");
                (StatusCode::BAD_GATEWAY, body).into_response()
            }
            ApiError::JobFailed { job, reason } => {
                // Same wording the trigger handler used to log itself, kept
                // here so constructing the error is enough to log it.
                tracing::warn!(job = %job, "manual job trigger failed: {reason}");
                (StatusCode::INTERNAL_SERVER_ERROR, reason).into_response()
            }
            ApiError::NotFound => StatusCode::NOT_FOUND.into_response(),
            ApiError::NotFoundWithReason(body) => (StatusCode::NOT_FOUND, body).into_response(),
            ApiError::Unauthorized(body) => (StatusCode::UNAUTHORIZED, body).into_response(),
        }
    }
}

/// An entity whose list / get-one / delete are plain single-table operations
/// over one primary key, implemented once here instead of copied per module.
///
/// Before this trait, 19 entity modules carried a byte-identical `async fn
/// list` and near-identical `get_one`/`delete`, and the SELECT column list was
/// spelled out twice per module (list and get). An entity now declares its
/// table, columns, ordering and noun, and takes [`list_handler`],
/// [`get_handler`] and [`delete_handler`] for its routes.
///
/// Deliberately *not* covered: `db_upsert`. That is where each entity's
/// write-time invariants live (see CLAUDE.md, "Data integrity") and it must
/// stay hand-written per entity. An entity whose read or delete does more than
/// one table's worth of work — `rights_sale` (attaches allocations),
/// `attachment` (filtered list), the outcome-enum deletes (`income`, `sell`,
/// `trade`, …) — keeps that verb hand-written and adopts the trait for the
/// verbs that do fit.
///
/// An entity keeps its own `db_list`/`db_get`/`db_delete` (now one-line
/// delegations to [`crud_list`]/[`crud_get`]/[`crud_delete`]) because other
/// modules and the DB-level tests call them by name (CLAUDE.md, "Test
/// conventions"). Where only the tests do, the wrapper is `#[cfg(test)]` —
/// the routes reach the query through the handler instead, so an ungated
/// wrapper would be dead code in the non-test build.
pub trait CrudEntity:
    Sized + Send + Unpin + serde::Serialize + for<'r> sqlx::FromRow<'r, SqliteRow> + 'static
{
    /// The primary key as it appears in the URL path — `i64` for the
    /// rowid-keyed tables, `String` for the code-keyed ones (`exchanges`,
    /// `currencies`, `mic_registry`). `Clone` because a blocked delete binds
    /// it a second time, to count the rows that blocked it (see
    /// [`fk_dependants_message`]).
    type Key: serde::de::DeserializeOwned
        + for<'q> sqlx::Encode<'q, Sqlite>
        + sqlx::Type<Sqlite>
        + Clone
        + Send
        + 'static;

    /// The table the three operations read and delete from.
    const TABLE: &'static str;
    /// The SELECT list, in the model struct's field order.
    const COLUMNS: &'static str;
    /// The primary-key column [`Self::Key`] addresses.
    const KEY_COLUMN: &'static str = "id";
    /// The list's ORDER BY, always ending in a unique column so the order is
    /// total (a list whose order can change between identical requests makes
    /// the UI's row positions unstable).
    const ORDER_BY: &'static str = "id";
    /// What a 404 from [`delete_handler`] calls the missing row, e.g.
    /// `"AMMA statement"` → `no AMMA statement with that id`.
    const NOUN: &'static str;
}

/// Every row of `E`'s table, in `E::ORDER_BY` order.
pub async fn crud_list<E: CrudEntity>(pool: &SqlitePool) -> Result<Vec<E>, sqlx::Error> {
    sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT {} FROM {} ORDER BY {}",
        E::COLUMNS,
        E::TABLE,
        E::ORDER_BY
    )))
    .fetch_all(pool)
    .await
}

/// One row of `E`'s table by primary key, or `None`.
///
/// Executor-generic so the same read composes onto a caller's transaction:
/// a write-time invariant that has to see the row inside its own transaction
/// (`closing_price::load_market_on`, reached from the trade write path)
/// cannot go to the pool for it.
pub async fn crud_get<'e, E, X>(executor: X, key: E::Key) -> Result<Option<E>, sqlx::Error>
where
    E: CrudEntity,
    X: sqlx::Executor<'e, Database = sqlx::Sqlite>,
{
    sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT {} FROM {} WHERE {} = ?",
        E::COLUMNS,
        E::TABLE,
        E::KEY_COLUMN
    )))
    .bind(key)
    .fetch_optional(executor)
    .await
}

/// Delete one row of `E`'s table by primary key; `true` if a row went.
pub async fn crud_delete<E: CrudEntity>(
    pool: &SqlitePool,
    key: E::Key,
) -> Result<bool, sqlx::Error> {
    let result = sqlx::query(sqlx::AssertSqlSafe(format!(
        "DELETE FROM {} WHERE {} = ?",
        E::TABLE,
        E::KEY_COLUMN
    )))
    .bind(key)
    .execute(pool)
    .await?;
    Ok(result.rows_affected() > 0)
}

/// `GET /<entities>` → 200 with every row as JSON.
pub async fn list_handler<E: CrudEntity>(
    State(pool): State<SqlitePool>,
) -> Result<Json<Vec<E>>, ApiError> {
    crud_list::<E>(&pool)
        .await
        .map(Json)
        .map_err(ApiError::from)
}

/// `GET /<entities>/{id}` → 200 with the row, or an empty-bodied 404 (the URL
/// itself names what is missing).
pub async fn get_handler<E: CrudEntity>(
    State(pool): State<SqlitePool>,
    Path(key): Path<E::Key>,
) -> Result<Json<E>, ApiError> {
    crud_get::<E, _>(&pool, key)
        .await
        .map_err(ApiError::from)?
        .map(Json)
        .ok_or(ApiError::NotFound)
}

/// `DELETE /<entities>/{id}` → 204, or a 404 naming the missing row. A row
/// still referenced by another table fails its foreign key and surfaces as a
/// 422 naming the dependants (see [`fk_dependants_message`]).
pub async fn delete_handler<E: CrudEntity>(
    State(pool): State<SqlitePool>,
    Path(key): Path<E::Key>,
) -> Result<StatusCode, ApiError> {
    match crud_delete::<E>(&pool, key.clone()).await {
        Ok(found) => deleted(found, E::NOUN),
        Err(err) => {
            match fk_dependants_message(&pool, &err, E::NOUN, E::TABLE, E::KEY_COLUMN, key).await {
                Ok(Some(body)) => Err(ApiError::Unprocessable(body)),
                Ok(None) => Err(ApiError::from(err)),
                Err(scan_err) => Err(ApiError::internal(scan_err)),
            }
        }
    }
}

/// The plain-text `422` body for a DELETE a foreign key blocked, or `None`
/// when `err` is any other failure (the caller then classifies it the shared
/// way, via [`ApiError`]'s `From<sqlx::Error>`).
///
/// That shared classification reads a foreign-key violation as an *outgoing*
/// reference — "the request refers to a record that does not exist", true of a
/// write naming an unknown listing or currency code. On a delete the same
/// SQLite error kind means the exact opposite: the row is there, and something
/// still depends on it. Saying it does not exist states the reverse of the
/// truth and names nothing the user could act on.
///
/// SQLite's message carries no detail either (a bare "FOREIGN KEY constraint
/// failed"), so the dependants are *discovered* rather than parsed — see
/// [`fk_dependants`] — and named with their row counts.
pub async fn fk_dependants_message<K>(
    pool: &SqlitePool,
    err: &sqlx::Error,
    noun: &str,
    table: &str,
    key_column: &str,
    key: K,
) -> Result<Option<String>, sqlx::Error>
where
    K: for<'q> sqlx::Encode<'q, Sqlite> + sqlx::Type<Sqlite> + Clone + Send,
{
    if err.as_database_error().map(|db| db.kind()) != Some(ErrorKind::ForeignKeyViolation) {
        return Ok(None);
    }
    let dependants = fk_dependants(pool, table, key_column, key).await?;
    Ok(Some(still_referenced(noun, &dependants)))
}

/// The `422` body naming what still depends on the row. With `dependants`
/// empty — a foreign key blocked the delete but the scan matched nothing, e.g.
/// a composite key this walk does not model — it still says the row is
/// referenced, never that it does not exist.
fn still_referenced(noun: &str, dependants: &[(String, i64)]) -> String {
    if dependants.is_empty() {
        return format!("this {noun} is still referenced by another record — remove it first");
    }
    let named = dependants
        .iter()
        .map(|(table, rows)| format!("{} ({rows})", dependant_label(table)))
        .collect::<Vec<_>>()
        .join(", ");
    format!("this {noun} is still referenced by {named} — remove those records first")
}

/// Tables whose name humanises badly — acronyms the schema spells lower case.
/// Anything not listed reads fine with its underscores turned into spaces.
const DEPENDANT_LABELS: &[(&str, &str)] = &[
    ("amit_adjustments", "AMIT adjustments"),
    ("amma_statements", "AMMA statements"),
    ("drp_enrolments", "DRP enrolment periods"),
    ("ess_statements", "ESS statements"),
    ("ess_vests", "ESS vests"),
    ("rba_fx_rates", "RBA FX rates"),
    ("cgt_settings", "CGT settings"),
    ("mic_registry", "the MIC registry"),
];

fn dependant_label(table: &str) -> String {
    DEPENDANT_LABELS
        .iter()
        .find(|(name, _)| *name == table)
        .map(|(_, label)| (*label).to_string())
        .unwrap_or_else(|| table.replace('_', " "))
}

/// Every table that still references `key_column = key` in `table`, with how
/// many of its rows do, ordered by table name so the message is stable.
///
/// Discovered from the schema rather than maintained by hand: walk each
/// table's `PRAGMA foreign_key_list`, keep the foreign keys pointing at
/// `table` whose `on_delete` would actually block (`NO ACTION` / `RESTRICT` —
/// a `CASCADE` or `SET NULL` child goes with the row, so it is never the
/// blocker), and count the matching rows on each. A new table with a new
/// foreign key is therefore named without touching this code.
async fn fk_dependants<K>(
    pool: &SqlitePool,
    table: &str,
    key_column: &str,
    key: K,
) -> Result<Vec<(String, i64)>, sqlx::Error>
where
    K: for<'q> sqlx::Encode<'q, Sqlite> + sqlx::Type<Sqlite> + Clone + Send,
{
    let tables: Vec<String> =
        sqlx::query_scalar("SELECT name FROM sqlite_master WHERE type = 'table' ORDER BY name")
            .fetch_all(pool)
            .await?;

    let mut dependants = Vec::new();
    for child in tables {
        let keys = sqlx::query(sqlx::AssertSqlSafe(format!(
            "PRAGMA foreign_key_list(\"{child}\")"
        )))
        .fetch_all(pool)
        .await?;

        let mut clauses = Vec::new();
        for fk in keys {
            let parent: String = fk.try_get("table")?;
            if !parent.eq_ignore_ascii_case(table) {
                continue;
            }
            let on_delete: String = fk.try_get("on_delete")?;
            if !matches!(on_delete.as_str(), "NO ACTION" | "RESTRICT") {
                continue;
            }
            let from: String = fk.try_get("from")?;
            // `to` is NULL when the foreign key names no parent column, which
            // means the parent's primary key — the column the delete keyed on.
            let to: Option<String> = fk.try_get("to")?;
            let to = to.unwrap_or_else(|| key_column.to_string());
            clauses.push(format!(
                "\"{from}\" IN (SELECT \"{to}\" FROM \"{table}\" WHERE \"{key_column}\" = ?)"
            ));
        }
        if clauses.is_empty() {
            continue;
        }
        // One count over all of the child's foreign keys at once, not one per
        // key: a `listing_renames` row can name the same exchange as both its
        // old and its new one, and summing per key would report that single
        // row twice.
        let mut count = sqlx::query_scalar::<_, i64>(sqlx::AssertSqlSafe(format!(
            "SELECT COUNT(*) FROM \"{child}\" WHERE {}",
            clauses.join(" OR ")
        )));
        for _ in &clauses {
            count = count.bind(key.clone());
        }
        let rows_here = count.fetch_one(pool).await?;
        if rows_here > 0 {
            dependants.push((child, rows_here));
        }
    }
    Ok(dependants)
}

/// Classify a database error: a constraint violation (foreign key, check,
/// unique, or not-null) means the request referenced or supplied data the
/// data model rejects — e.g. an unrecognised currency code (FK to
/// `currencies`), an unknown listing/exchange, or a value outside an enum's
/// `CHECK` set. That is the client's fault, so it surfaces as `422` with the
/// database's own message (which names the offending constraint/column —
/// e.g. `CHECK constraint failed: security_type`), reworded so it reads as a
/// sentence; it never contains a raw foreign-key id. Anything else —
/// including a `Decode` failure from a corrupt stored decimal — is an
/// unexpected server fault: `Internal`, logged when the response is built.
///
/// The foreign-key wording is the *write* direction — a body naming a row
/// that is not there. A DELETE fails the same foreign key for the opposite
/// reason (the row is there and something depends on it) and must not reach
/// this arm: deletes classify the violation themselves through
/// [`fk_dependants_message`], which names the dependants.
impl From<sqlx::Error> for ApiError {
    fn from(err: sqlx::Error) -> Self {
        if let Some(db) = err.as_database_error() {
            // SQLite's message, e.g. "UNIQUE constraint failed:
            // listings.ticker". It names columns/constraints, never a value
            // the client supplied, so it is safe to surface.
            let detail = db.message();
            let body = match db.kind() {
                ErrorKind::ForeignKeyViolation => {
                    "the request refers to a record that does not exist".to_string()
                }
                ErrorKind::CheckViolation => {
                    format!("a value falls outside its allowed set ({detail})")
                }
                ErrorKind::UniqueViolation => {
                    format!("a record with these key values already exists ({detail})")
                }
                ErrorKind::NotNullViolation => {
                    format!("a required field is missing ({detail})")
                }
                _ => return ApiError::Internal(err.into()),
            };
            return ApiError::Unprocessable(body);
        }
        // An FX gap travels through report code as a decode error (see
        // `impl From<FxError> for sqlx::Error`), carrying the `FxError`
        // itself so it can be recovered here rather than answered as an
        // opaque 500 the user can do nothing with (SCENARIOS M-04).
        if let sqlx::Error::Decode(source) = &err
            && let Some(fx) = source.downcast_ref::<crate::infra::fx::FxError>()
            && let Some(response) = missing_rate_unprocessable(fx)
        {
            return response;
        }
        ApiError::Internal(err.into())
    }
}

/// The `422` a missing ATO rate deserves: it is a gap in imported reference
/// data the user closes by running the RBA import, not an internal fault they
/// can do nothing about, so the body names the currency and the month — the
/// same answer [report-snapshot generation] already gives for the same gap,
/// where every other report answered a bare `500` with an empty body
/// (SCENARIOS M-04, M-07). `None` for any other `FxError`: a failed rate
/// *lookup* is a genuine server fault and stays a `500`.
///
/// [report-snapshot generation]: crate::reports::snapshot
fn missing_rate_unprocessable(err: &crate::infra::fx::FxError) -> Option<ApiError> {
    let crate::infra::fx::FxError::MissingRate { currency, month } = err else {
        return None;
    };
    tracing::warn!(%currency, %month, "report blocked by a missing ATO FX rate");
    Some(ApiError::Unprocessable(format!(
        "{err} — import that month's rates with POST /rba_fx_rates/import"
    )))
}

/// An FX failure raised directly (not through a `sqlx::Error`) classifies the
/// same way: a missing rate is the user's to close, anything else is a fault.
impl From<crate::infra::fx::FxError> for ApiError {
    fn from(err: crate::infra::fx::FxError) -> Self {
        missing_rate_unprocessable(&err).unwrap_or_else(|| ApiError::Internal(err.into()))
    }
}

/// A CSV serialisation failure while building an export is a server fault.
impl From<csv::Error> for ApiError {
    fn from(err: csv::Error) -> Self {
        ApiError::Internal(err.into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use http_body_util::BodyExt;
    use std::sync::{Arc, Mutex};

    async fn body_of(resp: Response) -> String {
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        String::from_utf8(bytes.to_vec()).unwrap()
    }

    /// A missing ATO rate is a data gap the user closes, so it answers `422`
    /// naming the currency and month wherever it surfaces — raised directly
    /// or carried through a report's `sqlx::Error` — while a genuine decode
    /// failure stays the `500` it should be (SCENARIOS M-04).
    #[tokio::test]
    async fn a_missing_fx_rate_is_a_422_naming_the_currency_and_month() {
        let missing = || crate::infra::fx::FxError::MissingRate {
            currency: "USD".to_string(),
            month: "2023-05".to_string(),
        };
        for (label, error) in [
            ("raised directly", ApiError::from(missing())),
            (
                "through a report",
                ApiError::from(sqlx::Error::from(missing())),
            ),
        ] {
            let resp = error.into_response();
            assert_eq!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY, "{label}");
            let body = body_of(resp).await;
            assert!(body.contains("USD"), "{label}: {body}");
            assert!(body.contains("2023-05"), "{label}: {body}");
            assert!(body.contains("/rba_fx_rates/import"), "{label}: {body}");
        }
        // A decode failure that is not an FX gap — a malformed stored decimal —
        // is a server fault and must not be reclassified with it.
        let resp = ApiError::from(sqlx::Error::Decode("not a decimal".into())).into_response();
        assert_eq!(resp.status(), StatusCode::INTERNAL_SERVER_ERROR);
        assert_eq!(body_of(resp).await, "");
    }

    /// Table names reach the user, so the ones the schema spells in lower-case
    /// acronyms are relabelled and the rest are humanised — never printed raw.
    #[test]
    fn a_dependant_is_named_the_way_the_screen_names_it() {
        assert_eq!(dependant_label("amit_adjustments"), "AMIT adjustments");
        assert_eq!(dependant_label("closing_prices"), "closing prices");
        assert_eq!(dependant_label("exchange_holidays"), "exchange holidays");
        assert_eq!(
            still_referenced(
                "listing",
                &[("closing_prices".to_string(), 2), ("trades".to_string(), 1)]
            ),
            "this listing is still referenced by closing prices (2), trades (1) — remove those \
             records first"
        );
    }

    /// The scan can come up empty — a composite-key foreign key this walk does
    /// not model, say. The message still has to be true: the row is there and
    /// something depends on it. Falling back to `ApiError`'s write-direction
    /// wording would state the reverse.
    #[test]
    fn a_blocked_delete_with_nothing_matched_still_says_the_row_is_referenced() {
        assert_eq!(
            still_referenced("exchange", &[]),
            "this exchange is still referenced by another record — remove it first"
        );
    }

    /// Only a foreign-key violation is re-read as a blocked delete; every other
    /// failure keeps the shared classification (a `CHECK` violation is still a
    /// 422 quoting the constraint, a decode failure is still a 500).
    #[tokio::test]
    async fn only_a_foreign_key_failure_is_treated_as_a_blocked_delete() {
        let pool = crate::test_support::test_pool().await;
        let err = sqlx::Error::Decode("invalid decimal in column quantity".into());
        let message = fk_dependants_message(&pool, &err, "listing", "listings", "id", 1_i64)
            .await
            .unwrap();
        assert_eq!(message, None);
    }

    #[tokio::test]
    async fn unprocessable_is_422_with_the_message_as_body() {
        let resp =
            ApiError::unprocessable("the date is before the listing existed").into_response();
        assert_eq!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY);
        assert_eq!(
            body_of(resp).await,
            "the date is before the listing existed"
        );
    }

    #[tokio::test]
    async fn not_found_is_404_with_an_empty_body() {
        let resp = ApiError::NotFound.into_response();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
        assert_eq!(body_of(resp).await, "");
    }

    #[tokio::test]
    async fn unauthorized_is_401_with_the_message_as_body() {
        let resp = ApiError::unauthorized("no session cookie or bearer token").into_response();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
        assert_eq!(body_of(resp).await, "no session cookie or bearer token");
    }

    #[tokio::test]
    async fn internal_is_500_with_an_empty_body_never_the_error_text() {
        let resp = ApiError::internal(sqlx::Error::Decode(
            "invalid decimal in column quantity".into(),
        ))
        .into_response();
        assert_eq!(resp.status(), StatusCode::INTERNAL_SERVER_ERROR);
        assert_eq!(body_of(resp).await, "");
    }

    /// Build the response under a buffer-backed tracing subscriber and return
    /// everything it emitted, so a test can assert on what reached the logs.
    fn logs_of(err: ApiError) -> String {
        #[derive(Clone, Default)]
        struct Buf(Arc<Mutex<Vec<u8>>>);
        impl std::io::Write for Buf {
            fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
                self.0.lock().unwrap().extend_from_slice(buf);
                Ok(buf.len())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        let buf = Buf::default();
        let writer = buf.clone();
        let subscriber = tracing_subscriber::fmt()
            .with_writer(move || writer.clone())
            .with_max_level(tracing::Level::ERROR)
            .finish();

        tracing::subscriber::with_default(subscriber, || {
            let _ = err.into_response();
        });

        String::from_utf8(buf.0.lock().unwrap().clone()).unwrap()
    }

    /// The whole point of the type: an internal failure reaches the logs.
    /// A buffer-backed tracing subscriber captures what `into_response`
    /// emits and the named decode failure must appear in it.
    #[tokio::test]
    async fn internal_logs_the_wrapped_error_at_error_level() {
        let logged = logs_of(ApiError::internal(sqlx::Error::Decode(
            "invalid decimal in column fx_rate: oops".into(),
        )));
        assert!(
            logged.contains("ERROR"),
            "no error-level line logged: {logged}"
        );
        assert!(
            logged.contains("invalid decimal in column fx_rate: oops"),
            "the wrapped error's message is missing from the log: {logged}"
        );
    }

    /// The same guarantee one layer up, which is what the per-entity enums'
    /// `#[derive(thiserror::Error)]` buys: a `sqlx::Error` that a `db_*`
    /// function absorbed into its own error enum via `#[from]` keeps the
    /// database's own message in the enum's `Display` — so wrapping *that*
    /// in `Internal` still logs what actually went wrong, not just a variant
    /// name — and `source()` still reaches the `sqlx::Error` itself.
    #[tokio::test]
    async fn an_entity_enum_keeps_the_wrapped_sqlx_error_in_its_message_and_source() {
        use crate::entities::listing::UpsertError;
        use std::error::Error as _;

        let entity_err: UpsertError =
            sqlx::Error::Decode("invalid decimal in column fx_rate: oops".into()).into();

        assert!(
            entity_err
                .to_string()
                .contains("invalid decimal in column fx_rate: oops"),
            "the enum's Display dropped the wrapped error: {entity_err}"
        );
        assert!(
            entity_err
                .source()
                .is_some_and(|source| source.is::<sqlx::Error>()),
            "source() does not chain back to the sqlx::Error: {entity_err}"
        );

        let logged = logs_of(ApiError::internal(entity_err));
        assert!(
            logged.contains("ERROR"),
            "no error-level line logged: {logged}"
        );
        assert!(
            logged.contains("invalid decimal in column fx_rate: oops"),
            "the wrapped error's message is missing from the log: {logged}"
        );
    }

    #[tokio::test]
    async fn db_constraint_violations_classify_as_422_with_detail() {
        let pool = crate::infra::db::init(":memory:").await.unwrap();
        sqlx::query("CREATE TABLE check_demo (v TEXT CHECK(v IN ('a', 'b')))")
            .execute(&pool)
            .await
            .unwrap();
        let err = sqlx::query("INSERT INTO check_demo (v) VALUES ('z')")
            .execute(&pool)
            .await
            .unwrap_err();
        let api: ApiError = err.into();
        match api {
            ApiError::Unprocessable(body) => {
                assert!(body.contains("allowed set"), "unexpected body: {body}")
            }
            other => panic!("expected Unprocessable, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn non_constraint_db_errors_classify_as_internal() {
        let api: ApiError = sqlx::Error::Decode("invalid decimal in column quantity".into()).into();
        assert!(matches!(api, ApiError::Internal(_)));
    }
}
