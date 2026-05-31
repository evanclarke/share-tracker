//! HTTP helpers shared by entity handlers.

use axum::http::StatusCode;
use sqlx::error::ErrorKind;

/// Map a database error raised while writing a row to an HTTP status code.
///
/// A constraint violation (foreign key, check, unique, or not-null) means the
/// request referenced or supplied data the data model rejects — e.g. an
/// unrecognised currency code (FK to `currencies`), an unknown listing/exchange,
/// or a value outside an enum's `CHECK` set. That is the client's fault, so it
/// surfaces as `422 Unprocessable Entity`. Anything else is an unexpected server
/// fault and maps to `500 Internal Server Error`.
pub fn write_error_status(err: &sqlx::Error) -> StatusCode {
    if let Some(db) = err.as_database_error() {
        match db.kind() {
            ErrorKind::ForeignKeyViolation
            | ErrorKind::CheckViolation
            | ErrorKind::UniqueViolation
            | ErrorKind::NotNullViolation => return StatusCode::UNPROCESSABLE_ENTITY,
            _ => {}
        }
    }
    StatusCode::INTERNAL_SERVER_ERROR
}
