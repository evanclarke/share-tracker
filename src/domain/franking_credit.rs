//! The maximum franking credit a company distribution can carry
//! (`docs/ato/allocating-franking-credits.md`, QC 47305).
//!
//! A corporate tax entity may attach at most
//! `frankable distribution × (1 ÷ gross-up rate)` to a distribution, the
//! gross-up rate being `(100% − corporate tax rate for imputation purposes) ÷
//! that rate`. At the standard 30% rate that is **franked amount × 30/70** —
//! the ratio in the fully franked worked example in
//! `docs/ato/you-and-your-shares-dividends.md` ($700 franked → $300 credit).
//! Every base-rate-entity rate (27.5%, 26%, 25%) yields a *smaller* maximum,
//! and a partly franked distribution smaller again, so the 30% figure is the
//! one ceiling that holds for every company distribution.
//!
//! What makes the ceiling worth enforcing rather than merely reporting is the
//! member's side of the same page: where a statement shows a credit above the
//! maximum, "the recipient is only entitled to a franking credit equal to the
//! maximum amount". A larger figure is therefore never claimable by this
//! system's taxpayer — it is a data-entry error (a transposed column, a
//! wrong statement line), and it inflates a *refundable* offset.
//!
//! Shared because two writes create franked income: an `income` row
//! (`entities::income::db_upsert`) and a buy-back's per-unit dividend terms
//! (`entities::corporate_action`, whose participation multiplies them into an
//! income row). They must not diverge on what is possible.

use crate::infra::decimal::mul_div;
use chrono::NaiveDate;
use rust_decimal::{Decimal, RoundingStrategy};

/// The first day the 30% corporate tax rate applied (the 2001–02 income year).
///
/// Distributions paid before it are out of the ceiling's scope: the imputation
/// rates in force earlier (34%, 36%, 39%) allowed proportionally *larger*
/// credits, so applying 30/70 to them would be a wrong rejection. The project
/// has no pre-2001 dividend data, so this is a scope cut, not an approximation
/// — the check simply doesn't run on such a row.
pub const THIRTY_PERCENT_RATE_FROM: NaiveDate = match NaiveDate::from_ymd_opt(2001, 7, 1) {
    Some(d) => d,
    None => unreachable!(),
};

/// Slack over the computed maximum, absorbing the rounding a statement applies
/// to the two figures it prints: the greater of one cent and **0.5%** of the
/// maximum.
///
/// A percentage rather than a fixed amount because real statements round more
/// loosely than the cent. The ATO's own Example 6
/// (`docs/ato/you-and-your-shares-dividends.md`) is the case in point: a fully
/// franked dividend of **$13,066** carrying **$5,600** of credits, where an
/// exact 30/70 would put the franked amount at $13,066.67 — the credit sits
/// 29 cents (0.005%) above the maximum computed from the rounded figure. The
/// cent floor covers the same rounding on a dividend too small for the
/// percentage to reach a cent.
///
/// The grace costs nothing in detection power: what this check exists to catch
/// — a credit keyed with no dividend behind it, or a transposed column
/// ($700 franked against $7,000 of credits) — is out by multiples, not by
/// fractions of a percent.
fn tolerance(maximum: Decimal) -> Decimal {
    (maximum * Decimal::new(5, 3)).max(Decimal::new(1, 2))
}

/// The largest franking credit a company may attach to `franked_amount`,
/// rounded to the cent the way a statement prints it (half away from zero,
/// matching `entities::income`'s per-share cross-check).
pub fn maximum_franking_credit(franked_amount: Decimal) -> Decimal {
    mul_div(&[franked_amount, Decimal::from(30)], Decimal::from(70))
        .round_dp_with_strategy(2, RoundingStrategy::MidpointAwayFromZero)
}

/// The ceiling `franking_credits` is checked against for a distribution paid
/// on `paid_on`: the maximum plus the rounding [`tolerance`], or `None` where
/// the check does not apply (a pre-2001 distribution — see
/// [`THIRTY_PERCENT_RATE_FROM`]).
///
/// The caller decides *whose* distribution this is: the ceiling holds for a
/// company's, not for a trust's, whose "franked distributions from trusts"
/// component can be reduced by the trust's own deductions while the member
/// still claims the full credit (`docs/ato/amma-statement-guidance-notes.md`,
/// Part B item 13Q).
pub fn credit_ceiling(franked_amount: Decimal, paid_on: NaiveDate) -> Option<Decimal> {
    (paid_on >= THIRTY_PERCENT_RATE_FROM).then(|| {
        let maximum = maximum_franking_credit(franked_amount);
        // Cent-rounded like the figures it is compared against, so a rejection
        // can quote it as an amount rather than a trailing fraction.
        (maximum + tolerance(maximum))
            .round_dp_with_strategy(2, RoundingStrategy::MidpointAwayFromZero)
    })
}

/// Whether `franking_credits` is impossible for a company distribution of
/// `franked_amount` paid on `paid_on`, and the ceiling it broke.
///
/// `None` means acceptable — including every pre-2001 distribution, which the
/// ceiling does not cover.
pub fn credit_above_ceiling(
    franked_amount: Decimal,
    franking_credits: Decimal,
    paid_on: NaiveDate,
) -> Option<Decimal> {
    credit_ceiling(franked_amount, paid_on).filter(|ceiling| franking_credits > *ceiling)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::ymd;

    fn dec(s: &str) -> Decimal {
        s.parse().unwrap()
    }

    /// The ATO's own fully franked figures: $700 franked carries exactly $300
    /// (`docs/ato/you-and-your-shares-dividends.md`, Example 2), and the
    /// project's PLS statement figure ($2,757.30 → $1,181.70) is likewise the
    /// maximum, not merely under it — the same 30/70 the income form's
    /// franking selector computes.
    #[test]
    fn the_maximum_is_the_fully_franked_figure_at_30_percent() {
        assert_eq!(maximum_franking_credit(dec("700")), dec("300"));
        assert_eq!(maximum_franking_credit(dec("2757.30")), dec("1181.70"));
        assert_eq!(maximum_franking_credit(Decimal::ZERO), Decimal::ZERO);
    }

    /// A base-rate entity's 25% dividend carries a *third* of the franked
    /// amount, comfortably under the 30% ceiling — the ceiling is a maximum
    /// over every corporate rate, not an assertion of the 30% one.
    #[test]
    fn a_base_rate_entity_dividend_is_under_the_ceiling() {
        let franked = dec("750");
        let base_rate_credit = dec("250"); // 750 × 25/75
        assert!(credit_above_ceiling(franked, base_rate_credit, ymd(2024, 3, 15)).is_none());
        assert_eq!(maximum_franking_credit(franked), dec("321.43"));
    }

    /// Statement rounding is what the tolerance is for, and the ATO's own
    /// Example 6 is the case that sets its size: $13,066 fully franked
    /// carrying $5,600, which an exact 30/70 puts 29 cents over. A fixed
    /// cent would reject the ATO's own worked example.
    #[test]
    fn the_atos_own_rounded_example_is_accepted() {
        let paid = ymd(2025, 4, 8);
        assert_eq!(maximum_franking_credit(dec("13066")), dec("5599.71"));
        assert!(credit_above_ceiling(dec("13066"), dec("5600"), paid).is_none());
    }

    /// The cent floor covers a dividend too small for the percentage to reach
    /// one ($10.00 franked → $4.2857…, printed $4.29).
    #[test]
    fn a_small_dividends_rounding_is_covered_by_the_cent_floor() {
        let paid = ymd(2024, 3, 15);
        assert_eq!(maximum_franking_credit(dec("10")), dec("4.29"));
        assert!(credit_above_ceiling(dec("10"), dec("4.30"), paid).is_none());
    }

    /// The transposed-column error the check exists for — out by multiples,
    /// far beyond any rounding — reported with the ceiling it broke.
    #[test]
    fn impossible_credits_are_reported_with_the_ceiling_they_broke() {
        let paid = ymd(2024, 3, 15);
        // $700 franked against $7,000 of credits: the ceiling is $301.50.
        assert_eq!(
            credit_above_ceiling(dec("700"), dec("7000"), paid),
            Some(dec("301.50"))
        );
        // Even a 30% overstatement is well outside the tolerance.
        assert!(credit_above_ceiling(dec("700"), dec("390"), paid).is_some());
    }

    /// Before 1 July 2001 the corporate rate was higher, so the 30/70 ceiling
    /// would reject legitimate credits: the check doesn't run at all, on
    /// either side of the boundary date.
    #[test]
    fn pre_2001_distributions_are_out_of_scope() {
        // 36% imputation: $640 franked carried $360 — over the 30/70 ceiling.
        assert!(credit_above_ceiling(dec("640"), dec("360"), ymd(2001, 6, 30)).is_none());
        assert!(credit_ceiling(dec("640"), ymd(2001, 6, 30)).is_none());
        // The first day of the 30% era is in scope.
        assert_eq!(
            credit_above_ceiling(dec("640"), dec("360"), ymd(2001, 7, 1)),
            Some(dec("275.66"))
        );
    }
}
