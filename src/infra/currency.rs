//! [`CurrencyCode`]: the type every stored `currency` column is read into, so
//! "is this amount already in AUD?" is asked one way everywhere.
//!
//! Reports take the Australian-tax view — every non-AUD amount is converted
//! with its `fx_rate` before it is aggregated (CLAUDE.md, "Financial
//! correctness") — and the question that gates that conversion used to be a
//! `String` compared against `"AUD"` at each call site, some exactly and some
//! case-insensitively. A `CurrencyCode` deliberately has no `PartialEq<&str>`,
//! so `currency == "AUD"` does not compile; [`CurrencyCode::is_aud`] (over the
//! one rule, [`is_aud`]) is the only spelling.
//!
//! It is a code, not a validated ISO 4217 value: what a code may be is the
//! `currencies` reference table's business, enforced by every `currency`
//! column's foreign key. It derefs to `&str` so the FX lookups, keyed by code,
//! take it unchanged.

use serde::{Deserialize, Serialize};
use std::fmt;
use std::ops::Deref;

/// The code of the currency an amount is stated in — an ISO 4217 alphabetic
/// code for fiat, or an ISO 24165 Digital Token Identifier for a token (a row
/// of the `currencies` table).
#[derive(
    utoipa::ToSchema,
    Debug,
    Clone,
    PartialEq,
    Eq,
    Hash,
    PartialOrd,
    Ord,
    Serialize,
    Deserialize,
    sqlx::Type,
)]
#[serde(transparent)]
#[sqlx(transparent)]
pub struct CurrencyCode(String);

/// The one rule behind [`CurrencyCode::is_aud`], for the FX lookups that are
/// handed a bare code. Case-insensitive: the stored codes are upper case
/// (the foreign key sees to that), but a code arriving from a feed or a test
/// need not be.
pub fn is_aud(code: &str) -> bool {
    code.eq_ignore_ascii_case("AUD")
}

impl CurrencyCode {
    /// Australian dollars — the reporting currency, the one no `fx_rate`
    /// converts.
    pub fn aud() -> Self {
        Self("AUD".to_string())
    }

    /// Is an amount in this currency already in AUD?
    pub fn is_aud(&self) -> bool {
        is_aud(&self.0)
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl Deref for CurrencyCode {
    type Target = str;

    fn deref(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for CurrencyCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl From<&str> for CurrencyCode {
    fn from(code: &str) -> Self {
        Self(code.to_string())
    }
}

impl From<String> for CurrencyCode {
    fn from(code: String) -> Self {
        Self(code)
    }
}

/// Test-only, so an assertion can say `assert_eq!(row.currency, "AUD")`. The
/// non-test build has no such comparison, which is what keeps production code
/// on [`CurrencyCode::is_aud`]: CI and the pre-push hook build both.
#[cfg(test)]
impl PartialEq<&str> for CurrencyCode {
    fn eq(&self, other: &&str) -> bool {
        self.0 == *other
    }
}

#[cfg(test)]
impl PartialEq<str> for CurrencyCode {
    fn eq(&self, other: &str) -> bool {
        self.0 == other
    }
}

#[cfg(test)]
impl PartialEq<CurrencyCode> for &str {
    fn eq(&self, other: &CurrencyCode) -> bool {
        *self == other.0
    }
}

impl From<CurrencyCode> for String {
    fn from(code: CurrencyCode) -> Self {
        code.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn aud_is_recognised_in_any_case_and_nothing_else_is() {
        assert!(CurrencyCode::aud().is_aud());
        assert!(CurrencyCode::from("aud").is_aud());
        assert!(!CurrencyCode::from("USD").is_aud());
        assert!(!CurrencyCode::from("AUDX").is_aud());
    }

    #[test]
    fn it_serialises_as_the_bare_code() {
        let code = CurrencyCode::from("USD");
        assert_eq!(serde_json::to_string(&code).unwrap(), "\"USD\"");
        let back: CurrencyCode = serde_json::from_str("\"USD\"").unwrap();
        assert_eq!(back, code);
    }
}
