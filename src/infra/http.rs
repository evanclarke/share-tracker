//! HTTP helpers shared by entity and report handlers.

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use sqlx::error::ErrorKind;

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
#[derive(Debug)]
pub enum ApiError {
    /// 500 — logged at error level when the response is built; empty body.
    Internal(BoxError),
    /// 422 with a plain-text explanation of what the request got wrong.
    Unprocessable(String),
    /// 400 — the request itself is malformed (e.g. an unparseable date in
    /// the path), as opposed to well-formed data the model rejects (422).
    BadRequest(String),
    /// 502 — an upstream feed (MIC registry, currency list, RBA rates)
    /// could not be fetched. The underlying fetch error is logged at error
    /// level when the response is built; the body carries the short
    /// user-facing explanation only.
    BadGateway { body: String, source: BoxError },
    /// 413 — an upload exceeds the size ceiling.
    PayloadTooLarge(String),
    /// 404, empty body — entity GETs, where the URL itself names what is
    /// missing.
    NotFound,
    /// 404 with a plain-text reason for the UI toast — operation endpoints
    /// (exercise, reinvest, delete), where the missing prerequisite deserves
    /// naming. Construct via [`ApiError::not_found`].
    NotFoundWithReason(String),
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
        ApiError::BadGateway { body: body.into(), source: source.into() }
    }

    /// A 404 whose body names what was missing.
    pub fn not_found(msg: impl Into<String>) -> Self {
        ApiError::NotFoundWithReason(msg.into())
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
            ApiError::NotFound => StatusCode::NOT_FOUND.into_response(),
            ApiError::NotFoundWithReason(body) => {
                (StatusCode::NOT_FOUND, body).into_response()
            }
        }
    }
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
        ApiError::Internal(err.into())
    }
}

/// An FX failure surfaces like the `sqlx::Error` it converts to elsewhere: a
/// missing rate is an internal fault of the data (an unimported month), not a
/// client error — it must be logged, never silently zeroed.
impl From<crate::infra::fx::FxError> for ApiError {
    fn from(err: crate::infra::fx::FxError) -> Self {
        ApiError::Internal(err.into())
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

    #[tokio::test]
    async fn unprocessable_is_422_with_the_message_as_body() {
        let resp = ApiError::unprocessable("the date is before the listing existed")
            .into_response();
        assert_eq!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY);
        assert_eq!(body_of(resp).await, "the date is before the listing existed");
    }

    #[tokio::test]
    async fn not_found_is_404_with_an_empty_body() {
        let resp = ApiError::NotFound.into_response();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
        assert_eq!(body_of(resp).await, "");
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

    /// The whole point of the type: an internal failure reaches the logs.
    /// A buffer-backed tracing subscriber captures what `into_response`
    /// emits and the named decode failure must appear in it.
    #[tokio::test]
    async fn internal_logs_the_wrapped_error_at_error_level() {
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

        let err = ApiError::internal(sqlx::Error::Decode(
            "invalid decimal in column fx_rate: oops".into(),
        ));
        tracing::subscriber::with_default(subscriber, || {
            let _ = err.into_response();
        });

        let logged = String::from_utf8(buf.0.lock().unwrap().clone()).unwrap();
        assert!(logged.contains("ERROR"), "no error-level line logged: {logged}");
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
        let api: ApiError =
            sqlx::Error::Decode("invalid decimal in column quantity".into()).into();
        assert!(matches!(api, ApiError::Internal(_)));
    }
}
