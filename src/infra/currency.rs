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

use rust_decimal::Decimal;
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

/// An amount known to be in Australian dollars — the reporting currency every
/// report aggregates in (CLAUDE.md, "Financial correctness"). A plain
/// `Decimal` says nothing about its currency, so a native-currency figure
/// could be added to an AUD one, or converted twice, and an AUD-only fixture
/// (rate 1) would never notice; an `Aud` is only made where an amount is
/// converted (`infra::fx`, `CostBase::into_aud_with`) or where the caller
/// states it is AUD already ([`Aud::stated`] — a request field documented as
/// AUD, a figure computed from AUD inputs).
///
/// Also the marker that says a [`CostBase`](crate::domain::cost_base::CostBase)
/// breakdown is in AUD (`CostBase<Aud>`), against [`Native`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct Aud(Decimal);

impl Aud {
    /// An amount the caller vouches is already in AUD.
    pub fn stated(amount: Decimal) -> Self {
        Self(amount)
    }

    /// The AUD figure, for arithmetic and output.
    pub fn amount(self) -> Decimal {
        self.0
    }
}

/// Marker for a figure still in its record's own trade currency — the
/// counterpart of [`Aud`] in `CostBase<Native>`. Uninhabited: it is only ever
/// a type parameter.
#[derive(Debug, Clone, Copy)]
pub enum Native {}

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

    /// Where production code may vouch that a bare `Decimal` is already AUD,
    /// with the reason each may. Everywhere else an [`Aud`] has to come out of
    /// a conversion — which is what makes the type worth having: an
    /// `Aud::stated` sprinkled wherever a `Decimal` will not type-check would
    /// launder a native-currency figure straight back into the AUD totals.
    const AUD_STATED_ALLOWED: &[(&str, &str)] = &[
        (
            "infra/fx.rs",
            "`apply_rate_to_aud`: the amount has just been divided by its \
             foreign-per-AUD rate",
        ),
        (
            "infra/decimal.rs",
            "`strict_aud_map`: the portfolio reports' `prices` request field, \
             documented as AUD per unit",
        ),
        (
            "reports/activity.rs",
            "the activity ledger's `price` request field, documented as AUD per unit",
        ),
    ];

    /// The [`AUD_STATED_ALLOWED`] scan: test modules, test-only files and
    /// comments are exempt; a call site anywhere else is an offender until it
    /// is classified, and an entry that no longer calls it is stale.
    #[test]
    fn aud_is_stated_only_where_the_amount_is_known_to_be_aud() {
        use crate::test_support::{code_only, rust_sources, strip_test_modules};
        let call = format!("Aud::{}(", "stated");
        let test_only = |rel: &str| {
            rel.contains("/tests/")
                || rel.ends_with("/tests.rs")
                || rel.starts_with("test_support")
                || ["ato_examples.rs", "doc_checks.rs"].contains(&rel)
        };
        let mut offenders = Vec::new();
        let mut seen: Vec<&str> = Vec::new();
        for (rel, body) in rust_sources() {
            if test_only(&rel) {
                continue;
            }
            let code = code_only(&strip_test_modules(&body));
            for (n, line) in code.lines().enumerate() {
                if !line.contains(&call) {
                    continue;
                }
                match AUD_STATED_ALLOWED.iter().find(|(f, _)| *f == rel) {
                    Some((file, _)) => seen.push(file),
                    None => offenders.push(format!("{rel}:{}: {}", n + 1, line.trim())),
                }
            }
        }
        assert!(
            offenders.is_empty(),
            "Aud::stated vouches that an amount is already AUD. Convert it instead \
             (fx::apply_rate_to_aud, CostBase::into_aud_with), or — if the amount \
             genuinely is AUD by contract — classify the file in AUD_STATED_ALLOWED \
             with the reason:\n{}",
            offenders.join("\n")
        );
        let stale: Vec<&str> = AUD_STATED_ALLOWED
            .iter()
            .map(|(f, _)| *f)
            .filter(|f| !seen.contains(f))
            .collect();
        assert!(
            stale.is_empty(),
            "AUD_STATED_ALLOWED names files that no longer call Aud::stated — drop \
             them: {stale:?}"
        );
    }

    #[test]
    fn it_serialises_as_the_bare_code() {
        let code = CurrencyCode::from("USD");
        assert_eq!(serde_json::to_string(&code).unwrap(), "\"USD\"");
        let back: CurrencyCode = serde_json::from_str("\"USD\"").unwrap();
        assert_eq!(back, code);
    }
}
