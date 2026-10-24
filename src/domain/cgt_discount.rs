//! The 12-month CGT discount ownership rule, shared by every calculation that
//! classifies a gain as discountable — one definition so the same parcel and
//! the same event date can never be classified differently in different
//! reports.

use chrono::{Months, NaiveDate};

/// Whether a gain on units acquired on `acquired` and disposed of (or
/// otherwise crystallised) on `event_date` qualifies for the 50% CGT discount
/// on ownership grounds: the units must have been owned for at least 12
/// months, excluding both the acquisition day and the event day
/// (`docs/ato/cgt-discount.md`) — so the owned days run from the day after
/// acquisition to the day before the event, and must span 12 whole months.
/// That is: the day 12 months before the event is still after the
/// acquisition — hence the strict `>`, the boundary this function exists to
/// state once.
///
/// The anniversary is counted back from the event, not forward from the
/// acquisition, because month arithmetic clamps a day the target month lacks:
/// 28 February 2023 + 12 months is 28 February 2024, which would make a
/// disposal on 29 February 2024 eligible after owning 1 March 2023 – 28
/// February 2024, a day short of 12 months. Counted back, 29 February 2024 −
/// 12 months clamps to 28 February 2023, which is not after the acquisition.
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
    event_date - Months::new(12) > acquired
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Datelike;

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

    /// Regression: bought on 28 February the year before a leap year, sold on
    /// 29 February, the owned days (1 March – 28 February) are a day short of
    /// 12 months. `acquired + 12 months` clamped to 28 February and called it
    /// eligible.
    #[test]
    fn the_leap_day_after_a_28_february_purchase_is_a_day_short() {
        let d = |y, m, d| NaiveDate::from_ymd_opt(y, m, d).unwrap();
        assert!(!discount_eligible(d(2023, 2, 28), d(2024, 2, 29)));
        assert!(discount_eligible(d(2023, 2, 28), d(2024, 3, 1)));
        assert!(discount_eligible(d(2023, 2, 27), d(2024, 2, 29)));
    }

    /// Every acquisition day from 1985 to 2030, against every event day within
    /// a fortnight of its first anniversary, agrees with counting the rule
    /// out in days: the owned days run from the day after acquisition to the
    /// day before the event, and a period of 12 months starting on a day ends
    /// the day before that day's anniversary — the same month and day a year
    /// on, or 1 March for a period starting on 29 February.
    #[test]
    fn the_rule_agrees_with_counting_the_owned_days() {
        fn anniversary(first: NaiveDate) -> NaiveDate {
            NaiveDate::from_ymd_opt(first.year() + 1, first.month(), first.day())
                .unwrap_or_else(|| NaiveDate::from_ymd_opt(first.year() + 1, 3, 1).unwrap())
        }
        let mut acquired = NaiveDate::from_ymd_opt(1985, 9, 20).unwrap();
        let last = NaiveDate::from_ymd_opt(2030, 12, 31).unwrap();
        while acquired <= last {
            let first_owned = acquired.succ_opt().unwrap();
            // Owned through the day before the event, so eligible from the
            // anniversary of the first owned day.
            let eligible_from = anniversary(first_owned);
            for offset in -14..=14 {
                let event = eligible_from + chrono::Duration::days(offset);
                assert_eq!(
                    discount_eligible(acquired, event),
                    event >= eligible_from,
                    "acquired {acquired}, event {event}"
                );
            }
            acquired = first_owned;
        }
    }
}
