//! Shared helpers for reading TEXT-stored decimal columns.
//!
//! Financial values are persisted as TEXT to preserve arbitrary precision (see
//! migration 0006). When reading them back in report queries, a malformed value
//! must surface as an error rather than being silently coerced to zero — a wrong
//! number in a financial report is worse than a failed request.

use rust_decimal::Decimal;

/// Parse a TEXT decimal column value, mapping a malformed value to a decode error
/// that names the offending column (so the failure is diagnosable in logs).
pub fn parse_dec(column: &str, value: String) -> Result<Decimal, sqlx::Error> {
    value.parse().map_err(|e: rust_decimal::Error| {
        sqlx::Error::Decode(format!("invalid decimal in column '{column}': {value:?} ({e})").into())
    })
}
