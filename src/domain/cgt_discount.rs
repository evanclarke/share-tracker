//! The 12-month CGT discount ownership rule, shared by every calculation that
//! classifies a gain as discountable — one definition so the same parcel and
//! the same event date can never be classified differently in different
//! reports.

use chrono::{Months, NaiveDate};

/// Whether a gain on units acquired on `acquired` and disposed of (or
/// otherwise crystallised) on `event_date` qualifies for the 50% CGT discount
/// on ownership grounds: the units must have been held for **more than** 12
/// months, exclusive of both the acquisition day and the event day
/// (`docs/ato/cgt-discount.md`). Held for exactly 12 months is *not* enough —
/// hence the strict `>`, the boundary this function exists to state once.
///
/// `acquired` is the *CGT* acquisition date, which for a rollover replacement
/// parcel is its deemed (carried) date rather than its trade date — callers
/// holding a parcel row pass [`crate::domain::cost_base::ParcelRow::acquired`]
/// (or its per-report equivalent), never the raw trade date.
///
/// Ownership is the only test applied here. Whether the amount is a *gain* at
/// all (a capital loss is never discounted), and whether the taxpayer is
/// otherwise eligible, stay with the caller.
pub fn discount_eligible(acquired: NaiveDate, event_date: NaiveDate) -> bool {
    event_date > acquired + Months::new(12)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The rule is exclusive of both days: a disposal exactly 12 months after
    /// acquisition misses the discount by one day. Every report that
    /// classifies a gain shares this boundary.
    #[test]
    fn exactly_twelve_months_is_not_eligible_but_a_day_later_is() {
        let d = |y, m, d| NaiveDate::from_ymd_opt(y, m, d).unwrap();
        assert!(!discount_eligible(d(2024, 1, 1), d(2025, 1, 1)));
        assert!(discount_eligible(d(2024, 1, 1), d(2025, 1, 2)));
        assert!(!discount_eligible(d(2024, 1, 1), d(2024, 12, 31)));
        // A same-day disposal, and a leap-day acquisition (Months arithmetic
        // clamps 29 Feb → 28 Feb, so the anniversary is 28 Feb 2025).
        assert!(!discount_eligible(d(2024, 6, 1), d(2024, 6, 1)));
        assert!(!discount_eligible(d(2024, 2, 29), d(2025, 2, 28)));
        assert!(discount_eligible(d(2024, 2, 29), d(2025, 3, 1)));
    }
}
