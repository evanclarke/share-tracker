//! HTTP helpers shared by entity handlers.

use axum::http::StatusCode;
use sqlx::error::ErrorKind;

/// Map a database error raised while writing a row to an HTTP status code and a
/// short, plain-text body explaining the rejection, so the web UI can show *why*
/// a write failed instead of a bare "HTTP 422".
///
/// A constraint violation (foreign key, check, unique, or not-null) means the
/// request referenced or supplied data the data model rejects — e.g. an
/// unrecognised currency code (FK to `currencies`), an unknown listing/exchange,
/// or a value outside an enum's `CHECK` set. That is the client's fault, so it
/// surfaces as `422 Unprocessable Entity`; the body surfaces the database's own
/// message (which names the offending constraint/column — e.g. `CHECK constraint
/// failed: security_type`), reworded so it reads as a sentence. It never contains
/// a raw foreign-key id. Anything else is an unexpected server fault and maps to
/// a `500` with no body — the underlying error is logged here, and internal
/// details must not reach the toast.
pub fn write_error_body(err: &sqlx::Error) -> (StatusCode, String) {
    if let Some(db) = err.as_database_error() {
        // SQLite's message, e.g. "UNIQUE constraint failed: listings.ticker" or
        // "FOREIGN KEY constraint failed". It names columns/constraints, never a
        // value the client supplied, so it is safe to surface.
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
            _ => {
                tracing::error!(error = %err, "database error serving write");
                return (StatusCode::INTERNAL_SERVER_ERROR, String::new());
            }
        };
        return (StatusCode::UNPROCESSABLE_ENTITY, body);
    }
    tracing::error!(error = %err, "non-database error serving write");
    (StatusCode::INTERNAL_SERVER_ERROR, String::new())
}
