//! Shared helpers for reading TEXT-stored decimal columns.
//!
//! Financial values are persisted as TEXT to preserve arbitrary precision (see the
//! schema migration). When reading them back in report queries, a malformed value
//! must surface as an error rather than being silently coerced to zero — a wrong
//! number in a financial report is worse than a failed request.

use rust_decimal::Decimal;
use sqlx::{Row, sqlite::SqliteRow};

/// Parse a TEXT decimal column value, mapping a malformed value to a decode error
/// that names the offending column (so the failure is diagnosable in logs).
pub fn parse_dec(column: &str, value: String) -> Result<Decimal, sqlx::Error> {
    value.parse().map_err(|e: rust_decimal::Error| {
        sqlx::Error::Decode(format!("invalid decimal in column '{column}': {value:?} ({e})").into())
    })
}

/// Read a required TEXT decimal column straight off a row, propagating both a
/// missing/typed-wrong column and a malformed value as errors (the latter naming
/// the column via [`parse_dec`]). The shared helper for `FromRow` impls of
/// entities whose monetary columns are stored as TEXT.
pub fn row_dec(row: &SqliteRow, column: &str) -> Result<Decimal, sqlx::Error> {
    parse_dec(column, row.try_get(column)?)
}

/// Read a nullable TEXT decimal column: `NULL` maps to `None`, a present value is
/// parsed via [`parse_dec`] (so a malformed value is a column-named error, never a
/// silent `None`).
pub fn row_opt_dec(row: &SqliteRow, column: &str) -> Result<Option<Decimal>, sqlx::Error> {
    row.try_get::<Option<String>, _>(column)?
        .map(|s| parse_dec(column, s))
        .transpose()
}
