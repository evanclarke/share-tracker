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

/// The panic twin of [`ApiError::Internal`], for
/// `tower_http::catch_panic::CatchPanicLayer` in [`crate::app::router`].
///
/// A handler that panics unwinds past every `Result<_, ApiError>` in the
/// tree, so without this the connection is simply dropped: the client sees no
/// status at all (curl reports `000`), and the web UI shows a bare network
/// error naming nothing — which is how a parcel whose cost-base arithmetic
/// overflowed took down every portfolio read invisibly (SCENARIOS W-b).
///
/// The response is deliberately identical to [`ApiError::Internal`]'s: `500`
/// with an **empty body**, the detail logged via `tracing::error!` and never
/// returned. A panic message can carry anything (a file path, a row's
/// contents), so it is exactly the kind of internal detail that convention
/// exists to keep out of a response.
pub fn panic_response(err: Box<dyn std::any::Any + Send + 'static>) -> Response {
    // The payload of `panic!("…")` / `unwrap()` on a `&str` or `String`; any
    // other payload type has no readable form, so it is named as such rather
    // than lost.
    let message = err
        .downcast_ref::<&str>()
        .map(|s| s.to_string())
        .or_else(|| err.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "panic with a non-string payload".to_string());
    tracing::error!(panic = %message, "request handler panicked");
    StatusCode::INTERNAL_SERVER_ERROR.into_response()
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

    // -----------------------------------------------------------------------
    // Request bodies deny unknown fields
    // -----------------------------------------------------------------------

    /// Request-shaped types that are deliberately **permissive**, each with
    /// the reason it must stay that way. A type belongs here only if denying
    /// unknown fields would break something real:
    ///
    /// - a struct deserialised from an **external feed** (a provider response,
    ///   the ISO registries) — the publisher adding a field must not fail the
    ///   import;
    /// - a struct that only ever decodes a **response** (a report row read
    ///   back out of `report_snapshots`, a summary a test decodes) — the
    ///   producer is this server, and an older stored payload may carry a
    ///   field the current struct has since dropped;
    /// - a struct with a `#[serde(flatten)]` field — serde rejects the
    ///   combination at compile time.
    ///
    /// It is empty, and that is the point: neither of the first two kinds is
    /// reachable from a handler's extractor (feeds arrive as a `String` body
    /// this server parses itself; response types are only ever serialised on
    /// the way out), and no request body flattens. An entry here is a claim
    /// that a *request* body may silently drop a misspelt field, which
    /// SCENARIOS V-a is about not doing.
    const UNKNOWN_FIELDS_ALLOWED: &[(&str, &str, &str)] = &[];

    /// One `Deserialize`-deriving type, as the source scan sees it.
    struct DeserializedType {
        /// Path relative to `src`, with `/` separators.
        file: String,
        name: String,
        line: usize,
        /// Carries `#[serde(deny_unknown_fields)]`.
        denies: bool,
        /// Whether it has named fields at all: a field-less enum
        /// (`TradeType`, `WorthlessEvent`) has nothing to deny, so the
        /// attribute would be a no-op on it.
        has_fields: bool,
        /// The type names its fields name, for the walk into nested bodies
        /// (a Sell body's `allocations`, a what-if request's rows).
        field_types: Vec<String>,
        /// Its named fields, in declaration order, each with the serde
        /// attributes written above it — what the money-field guard reads.
        fields: Vec<ScannedField>,
    }

    /// One named field of a [`DeserializedType`].
    struct ScannedField {
        name: String,
        /// The declared type, source order preserved and whitespace squeezed
        /// (`Option<Decimal>`, `Vec<AllocationInput>`).
        ty: String,
        /// The `#[…]` attribute lines written directly above it, joined.
        attrs: String,
    }

    /// Every `.rs` file under `src`, path relative to `src`.
    fn source_files() -> Vec<(String, String)> {
        let src = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut found = Vec::new();
        let mut walk = vec![src.clone()];
        while let Some(dir) = walk.pop() {
            for entry in std::fs::read_dir(&dir)
                .expect("src should be readable")
                .flatten()
            {
                let path = entry.path();
                if path.is_dir() {
                    walk.push(path);
                } else if path.extension().is_some_and(|x| x == "rs") {
                    let rel = path
                        .strip_prefix(&src)
                        .expect("under src")
                        .components()
                        .map(|c| c.as_os_str().to_string_lossy().into_owned())
                        .collect::<Vec<_>>()
                        .join("/");
                    let body = std::fs::read_to_string(&path).expect("source should be readable");
                    found.push((rel, body));
                }
            }
        }
        found.sort();
        found
    }

    /// The `struct`/`enum` a definition line declares, if it declares one.
    fn item_name(line: &str) -> Option<String> {
        let mut tokens = line.split_whitespace();
        while let Some(token) = tokens.next() {
            if token == "struct" || token == "enum" {
                let name: String = tokens
                    .next()?
                    .chars()
                    .take_while(|c| c.is_alphanumeric() || *c == '_')
                    .collect();
                return (!name.is_empty()).then_some(name);
            }
            if !token.starts_with("pub") {
                return None;
            }
        }
        None
    }

    /// The capitalised identifiers in `text` — the type names a field names.
    fn type_names_in(text: &str) -> Vec<String> {
        let mut names = Vec::new();
        let mut word = String::new();
        for c in text.chars() {
            if c.is_alphanumeric() || c == '_' {
                word.push(c);
            } else {
                if word.starts_with(char::is_uppercase) {
                    names.push(std::mem::take(&mut word));
                } else {
                    word.clear();
                }
            }
        }
        if word.starts_with(char::is_uppercase) {
            names.push(word);
        }
        names
    }

    /// Every type in `src` that derives `Deserialize`.
    fn deserialized_types() -> Vec<DeserializedType> {
        let mut types = Vec::new();
        for (file, body) in source_files() {
            let lines: Vec<&str> = body.lines().collect();
            for (n, line) in lines.iter().enumerate() {
                if !line.contains("derive(") || !line.contains("Deserialize") {
                    continue;
                }
                // The attributes between the derive and the item it decorates:
                // `deny_unknown_fields` is one of them.
                let Some(start) =
                    (n..lines.len().min(n + 15)).find(|i| item_name(lines[*i]).is_some())
                else {
                    continue;
                };
                let name = item_name(lines[start]).expect("just matched");
                let denies = lines[n..start]
                    .iter()
                    .any(|l| l.contains("deny_unknown_fields"));

                // The item's own braces bound its fields.
                let mut depth = 0i32;
                let mut opened = false;
                let mut fields = Vec::new();
                let mut named = Vec::new();
                let mut has_fields = false;
                // The `#[…]` lines seen since the last field, which are the
                // attributes of the next one. An attribute rustfmt has wrapped
                // over several lines is one attribute, so the `[` / `]` depth
                // says where it ends rather than the leading `#`.
                let mut pending = String::new();
                let mut in_attr = 0i32;
                for line in &lines[start..] {
                    let code = line.split("//").next().unwrap_or("");
                    depth += code.matches('{').count() as i32;
                    if code.contains('{') {
                        opened = true;
                    }
                    depth -= code.matches('}').count() as i32;
                    let trimmed = code.trim_start();
                    let attr_line = in_attr > 0 || trimmed.starts_with('#');
                    if attr_line {
                        pending.push_str(trimmed);
                        in_attr += code.matches('[').count() as i32;
                        in_attr -= code.matches(']').count() as i32;
                    }
                    // A named field: `name: Type`, not a `::` path segment.
                    if let Some(colon) = trimmed.find(':')
                        && !attr_line
                        && trimmed[colon..].starts_with(": ")
                    {
                        has_fields = true;
                        fields.extend(type_names_in(&trimmed[colon + 1..]));
                        let name: String = trimmed[..colon]
                            .rsplit(char::is_whitespace)
                            .next()
                            .unwrap_or("")
                            .to_string();
                        let ty: String = trimmed[colon + 1..]
                            .trim()
                            .trim_end_matches(',')
                            .chars()
                            .filter(|c| !c.is_whitespace())
                            .collect();
                        named.push(ScannedField {
                            name,
                            ty,
                            attrs: std::mem::take(&mut pending),
                        });
                    }
                    if opened && depth <= 0 {
                        break;
                    }
                    if !opened && code.trim_end().ends_with(';') {
                        break;
                    }
                }
                types.push(DeserializedType {
                    file: file.clone(),
                    name,
                    line: start + 1,
                    denies,
                    has_fields,
                    field_types: fields,
                    fields: named,
                });
            }
        }
        types
    }

    /// The types axum deserialises straight off an inbound request: whatever
    /// a handler takes as `Json<T>`, `Query<T>` or `Form<T>` in *parameter*
    /// position (the return side, after `->`, is the response).
    fn extractor_types() -> Vec<String> {
        let mut found = Vec::new();
        for (_, body) in source_files() {
            for line in body.lines() {
                let code = line.trim_start();
                if code.starts_with("//") {
                    continue;
                }
                // Only the parameter list: `-> Result<Json<Report>>` is output.
                let params = code.split("->").next().unwrap_or("");
                for extractor in ["Json<", "Query<", "Form<"] {
                    for (at, _) in params.match_indices(extractor) {
                        let rest = &params[at + extractor.len()..];
                        let name: String = rest
                            .chars()
                            .take_while(|c| c.is_alphanumeric() || *c == '_' || *c == ':')
                            .collect();
                        if let Some(name) = name.rsplit("::").next()
                            && !name.is_empty()
                        {
                            found.push(name.to_string());
                        }
                    }
                }
            }
        }
        found
    }

    /// Every type an HTTP request is deserialised into: reachable from a
    /// handler's `Json`/`Query`/`Form` extractor (transitively, so a Sell
    /// body's allocation rows count), or named like a request body even where
    /// the scan cannot see the handler that takes it. Only the ones with named
    /// fields — a field-less enum has nothing for either guard to police.
    ///
    /// Shared by the two guards below, so both police exactly the same set and
    /// neither can drift: a new request body is covered by both at once.
    fn request_body_types(types: &[DeserializedType]) -> Vec<&DeserializedType> {
        assert!(
            types.len() > 100,
            "the Deserialize scan found only {} types — it has stopped parsing the tree",
            types.len()
        );

        // Reachable from a handler: the extractor types, then whatever their
        // fields name (a Sell body's allocation rows, a what-if request's).
        let mut queue = extractor_types();
        let mut reachable: Vec<String> = Vec::new();
        while let Some(name) = queue.pop() {
            if reachable.contains(&name) {
                continue;
            }
            if !types.iter().any(|t| t.name == name) {
                continue;
            }
            reachable.push(name.clone());
            for found in types.iter().filter(|t| t.name == name) {
                queue.extend(found.field_types.iter().cloned());
            }
        }

        // A parse that quietly found nothing would pass every assertion in the
        // callers, so pin the shape of what it must have found.
        for expected in [
            "AmmaStatementBody",
            "TradeBody",
            "IncomeBody",
            "SellBody",
            "AllocationInput",
            "RowHistoryRequest",
            "ListQuery",
        ] {
            assert!(
                reachable.iter().any(|n| n == expected),
                "the handler scan did not reach {expected} — it has stopped finding extractors"
            );
        }
        assert!(
            reachable.len() >= 40,
            "only {} request types reached from a handler; there are more than that",
            reachable.len()
        );

        types
            .iter()
            .filter(|t| {
                t.has_fields
                    && (reachable.contains(&t.name)
                        || ["Body", "Request", "Params", "Query", "Form"]
                            .iter()
                            .any(|suffix| t.name.ends_with(suffix)))
            })
            .collect()
    }

    /// Every HTTP request body refuses a field it does not recognise, so a
    /// misspelt name is a `422` naming it instead of a silently-ignored
    /// default (SCENARIOS V-a: `frankingcredits` on an AMMA statement stored
    /// A$0 of franking credits under a `204`).
    ///
    /// Nothing in the type system asks for `#[serde(deny_unknown_fields)]`, so
    /// this enumerates the bodies **reachable from a handler** — every type a
    /// `Json<T>`/`Query<T>`/`Form<T>` extractor names, and transitively every
    /// type their fields name — and requires the attribute on each one that
    /// has fields to deny. A new request body is therefore covered without its
    /// author having to remember, and a body that must stay permissive says so
    /// in [`UNKNOWN_FIELDS_ALLOWED`] with its reason.
    ///
    /// It lives here, in the module that owns the HTTP request/response
    /// contract (`ApiError` and the wording every rejection reaches the user
    /// with), because that is what the attribute is part of: the rejection an
    /// unrecognised field earns.
    #[test]
    fn every_request_body_denies_unknown_fields() {
        let types = deserialized_types();
        let required = request_body_types(&types);

        let mut offenders = Vec::new();
        let mut excused: Vec<&str> = Vec::new();
        for found in &required {
            let allowed = UNKNOWN_FIELDS_ALLOWED
                .iter()
                .find(|(file, name, _)| *file == found.file && *name == found.name);
            match (found.denies, allowed) {
                (true, Some((file, name, _))) => offenders.push(format!(
                    "{file}:{}: {name} denies unknown fields and is also excused — drop the \
                     UNKNOWN_FIELDS_ALLOWED entry",
                    found.line
                )),
                (false, None) => offenders.push(format!(
                    "{}:{}: {} is deserialised from a request and does not deny unknown fields",
                    found.file, found.line, found.name
                )),
                (false, Some((_, name, _))) => excused.push(name),
                (true, None) => {}
            }
        }
        assert!(
            offenders.is_empty(),
            "add #[serde(deny_unknown_fields)] so a misspelt field is a 422 naming it rather \
             than a silently-ignored default — or, if the type must stay permissive, name it in \
             UNKNOWN_FIELDS_ALLOWED with the reason:\n{}",
            offenders.join("\n")
        );
        // …and the excuse list may not rot: an entry naming a type that is no
        // longer permissive, or no longer there, has to go.
        let stale: Vec<&str> = UNKNOWN_FIELDS_ALLOWED
            .iter()
            .filter(|(_, name, _)| !excused.contains(name))
            .map(|(_, name, _)| *name)
            .collect();
        assert!(
            stale.is_empty(),
            "UNKNOWN_FIELDS_ALLOWED names types that are no longer permissive request \
             bodies — drop them: {stale:?}"
        );
    }

    // -----------------------------------------------------------------------
    // Money request fields refuse a JSON number
    // -----------------------------------------------------------------------

    /// Request-body `Decimal` fields that may still be sent as a JSON number,
    /// each with the reason. Same shape and same discipline as
    /// [`UNKNOWN_FIELDS_ALLOWED`], and empty for the same reason: an entry here
    /// is a claim that one money or quantity figure may silently lose digits
    /// past the fifteenth significant one, which SCENARIOS W-a is about not
    /// doing.
    const JSON_NUMBER_ALLOWED: &[(&str, &str, &str)] = &[];

    /// Every money/quantity field of every HTTP request body refuses a JSON
    /// number, so `{"quantity": 100000000.00000001}` is a `422` naming
    /// `quantity` instead of a `204` storing `100000000` (SCENARIOS W-a).
    ///
    /// `rust_decimal`'s own `Deserialize` accepts a JSON number, and
    /// `serde_json` hands it over as an `f64` — ~15 significant digits — so the
    /// project's *"money and quantities are always `Decimal`, never `f64`"*
    /// rule, which holds everywhere else in the tree, had exactly one hole in
    /// it: the request deserialiser. `infra::decimal::strict_decimal` /
    /// `strict_optional_decimal` close it per field, and nothing in the type
    /// system asks for the attribute, so this walks the same set as
    /// [`every_request_body_denies_unknown_fields`] — every type reachable from
    /// a handler's extractor — and requires it on each `Decimal` /
    /// `Option<Decimal>` field. A new request body is therefore covered without
    /// its author having to remember.
    #[test]
    fn every_money_request_field_refuses_a_json_number() {
        /// The attribute a field of this type must carry.
        fn required_for(ty: &str) -> Option<&'static str> {
            match ty {
                "Decimal" => Some("strict_decimal"),
                "Option<Decimal>" => Some("strict_optional_decimal"),
                // The three portfolio reports' price-override maps.
                "HashMap<i64,Decimal>" => Some("strict_decimal_map"),
                // Any other shape nesting a decimal would need its own
                // `deserialize_with`, so fail rather than wave it through.
                other if other.contains("Decimal") => Some("«a strict decimal deserialiser»"),
                _ => None,
            }
        }

        let types = deserialized_types();
        let required = request_body_types(&types);

        let mut money_fields = 0usize;
        let mut offenders = Vec::new();
        let mut excused: Vec<&str> = Vec::new();
        for found in &required {
            for field in &found.fields {
                let Some(want) = required_for(&field.ty) else {
                    continue;
                };
                money_fields += 1;
                let allowed = JSON_NUMBER_ALLOWED.iter().find(|(file, type_name, _)| {
                    *file == found.file && *type_name == format!("{}.{}", found.name, field.name)
                });
                // `strict_optional_decimal` contains `strict_decimal`, so the
                // required-attribute check has to be the exact function name.
                let has = field
                    .attrs
                    .contains(&format!("crate::infra::decimal::{want}\""));
                match (has, allowed) {
                    (true, Some((file, name, _))) => offenders.push(format!(
                        "{file}: {name} both refuses a JSON number and is excused — drop the \
                         JSON_NUMBER_ALLOWED entry"
                    )),
                    (false, None) => offenders.push(format!(
                        "{}:{}: {}.{}: {} is deserialised from a request without \
                         #[serde(deserialize_with = \"crate::infra::decimal::{want}\")]",
                        found.file, found.line, found.name, field.name, field.ty
                    )),
                    (false, Some((_, name, _))) => excused.push(name),
                    (true, None) => {}
                }
            }
        }

        // A scan that stopped finding fields would pass every assertion above.
        assert!(
            money_fields > 100,
            "only {money_fields} money fields found across {} request bodies — the field scan \
             has stopped parsing",
            required.len()
        );
        assert!(
            offenders.is_empty(),
            "a JSON number is read as an f64 and silently loses digits past about the 15th \
             significant one, so a money or quantity field must accept only a decimal string — \
             add the attribute, or, if the field must keep taking a number, name it in \
             JSON_NUMBER_ALLOWED with the reason:\n{}",
            offenders.join("\n")
        );
        let stale: Vec<&str> = JSON_NUMBER_ALLOWED
            .iter()
            .filter(|(_, name, _)| !excused.contains(name))
            .map(|(_, name, _)| *name)
            .collect();
        assert!(
            stale.is_empty(),
            "JSON_NUMBER_ALLOWED names fields that are no longer permissive — drop them: {stale:?}"
        );
    }
}
