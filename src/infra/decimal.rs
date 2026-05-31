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

/// Re-render a stored TEXT decimal in `rust_decimal`'s canonical plain-decimal
/// form, returning `Some(canonical)` only when it differs from `value`.
///
/// Migration 0006 converted the REAL columns of migrations 0004/0005 to TEXT with
/// `CAST(REAL AS TEXT)`, which emits non-canonical text for some magnitudes — SQLite
/// renders a small float as scientific notation (e.g. `1.0e-08`), unlike the
/// plain-decimal strings the app writes today. `Decimal::to_string` never uses an
/// exponent, so parsing then re-rendering normalises the form without changing the
/// numeric value. A malformed value surfaces as a decode error rather than being
/// silently rewritten (see `parse_dec`).
pub fn canonicalize_decimal(column: &str, value: &str) -> Result<Option<String>, sqlx::Error> {
    let canonical = parse_dec(column, value.to_string())?.to_string();
    Ok((canonical != value).then_some(canonical))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::str::FromStr;

    #[test]
    fn canonicalize_expands_scientific_notation() {
        // SQLite's CAST(REAL AS TEXT) emits this for a tiny value; it must become
        // a plain decimal with the same numeric value.
        let canonical = canonicalize_decimal("quantity", "1.0e-08").unwrap();
        let canonical = canonical.expect("scientific notation is not canonical");
        assert!(!canonical.contains(['e', 'E']), "expected plain decimal, got {canonical:?}");
        assert_eq!(canonical.parse::<Decimal>().unwrap(), Decimal::from_str("0.00000001").unwrap());
    }

    #[test]
    fn canonicalize_expands_large_scientific_notation() {
        let canonical = canonicalize_decimal("average_price", "1.23e+20").unwrap();
        let canonical = canonical.expect("scientific notation is not canonical");
        assert_eq!(canonical, "123000000000000000000");
    }

    #[test]
    fn canonicalize_leaves_plain_decimals_untouched() {
        // Already-canonical values (including preserved scale and the '0' default)
        // produce no rewrite.
        assert_eq!(canonicalize_decimal("average_price", "19.99").unwrap(), None);
        assert_eq!(canonicalize_decimal("brokerage", "0").unwrap(), None);
        assert_eq!(canonicalize_decimal("quantity", "1.000").unwrap(), None);
    }

    #[test]
    fn canonicalize_propagates_malformed_value() {
        let err = canonicalize_decimal("quantity", "not-a-number").unwrap_err();
        assert!(matches!(err, sqlx::Error::Decode(_)));
    }
}
