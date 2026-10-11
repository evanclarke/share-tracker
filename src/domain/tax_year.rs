//! The Australian financial-year bucketing rule, shared by every report that
//! groups facts by FY (tax summary, net capital gain, the E4 cross-check) —
//! one definition so a date can never land in different years in different
//! reports.
use chrono::{Datelike, NaiveDate};

/// The Australian financial year `date` falls in, identified by the calendar
/// year of its 30 June end (the convention every FY-keyed report field uses):
/// a July–December date belongs to the FY ending the following June.
pub fn tax_year_for(date: NaiveDate) -> i32 {
    if date.month() >= 7 {
        date.year() + 1
    } else {
        date.year()
    }
}

/// The 1 July on which the financial year containing `date` began — the
/// window's lower bound wherever a fact must fall *within* a year (an AMMA
/// statement's income year, its cross-check's coverage band).
pub fn fy_start(date: NaiveDate) -> NaiveDate {
    NaiveDate::from_ymd_opt(tax_year_for(date) - 1, 7, 1).expect("1 July is a valid date")
}

/// Was this listing an AMIT in the Australian financial year `tax_year`
/// (identified by the calendar year of its 30 June end)?
///
/// *The* rule behind every reader of the flag — the income entity's
/// write-time checks, the [tax summary](crate::reports::tax_summary)'s
/// whole-row exclusion, the [AMIT cash
/// cross-check](crate::reports::amit_cash_cross_check), the annual tax
/// report's completeness section *and* its printed income rows, the
/// [franking](crate::reports::franking) loader's candidate dividends, and the
/// `ReturnOfCapital` refusal — so a fund that converted part-way through a
/// holding cannot be an AMIT to one reader and an ordinary trust to another
/// (SCENARIOS F-23, P-01/P-07).
///
/// `amit_from` is a 1 July date (write-time enforced), so this is a
/// whole-year comparison: AMIT status is elected for an income year
/// (`docs/ato/amit-reporting-requirements.md`), never for part of one. `None`
/// means the flag applies to the whole recorded history — the ordinary case,
/// a fund that has always been an AMIT.
pub fn amit_in_tax_year(amit: bool, amit_from: Option<NaiveDate>, tax_year: i32) -> bool {
    amit && amit_from.is_none_or(|from| tax_year_for(from) <= tax_year)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn july_starts_the_next_fy_and_june_ends_the_current_one() {
        let d = |y, m, d| NaiveDate::from_ymd_opt(y, m, d).unwrap();
        assert_eq!(tax_year_for(d(2024, 6, 30)), 2024);
        assert_eq!(tax_year_for(d(2024, 7, 1)), 2025);
        assert_eq!(tax_year_for(d(2024, 12, 31)), 2025);
        assert_eq!(tax_year_for(d(2025, 1, 1)), 2025);
    }

    #[test]
    fn a_financial_year_starts_on_the_1_july_before_its_30_june_end() {
        let d = |y, m, d| NaiveDate::from_ymd_opt(y, m, d).unwrap();
        assert_eq!(fy_start(d(2025, 6, 30)), d(2024, 7, 1));
        assert_eq!(fy_start(d(2024, 7, 1)), d(2024, 7, 1));
        assert_eq!(fy_start(d(2024, 12, 31)), d(2024, 7, 1));
    }

    /// The shared rule every reader of the flag asks: an undated flag covers
    /// the whole history, and a dated one turns on with its own financial
    /// year — 1 July 2023 makes FY2024 the first AMIT year.
    #[test]
    fn amit_in_tax_year_turns_on_with_the_dated_financial_year() {
        assert!(amit_in_tax_year(true, None, 2019));
        assert!(!amit_in_tax_year(false, None, 2019));

        let from = Some(NaiveDate::from_ymd_opt(2023, 7, 1).unwrap());
        assert!(!amit_in_tax_year(true, from, 2022));
        assert!(!amit_in_tax_year(true, from, 2023));
        assert!(amit_in_tax_year(true, from, 2024));
        assert!(amit_in_tax_year(true, from, 2025));
    }

    proptest::proptest! {
        /// Every date from 1900 to 2100 lies inside the year the rule names
        /// for it — on or after that year's 1 July start, on or before its
        /// 30 June end, a year apart — and the year changes between two
        /// consecutive days exactly when the second is 1 July.
        #[test]
        fn every_date_lies_in_the_financial_year_named_for_it(days in 0i64..73_000) {
            let date = NaiveDate::from_ymd_opt(1900, 1, 1).unwrap() + chrono::Duration::days(days);
            let start = fy_start(date);
            let end = NaiveDate::from_ymd_opt(tax_year_for(date), 6, 30).unwrap();
            proptest::prop_assert_eq!((start.month(), start.day()), (7, 1));
            proptest::prop_assert_eq!(start.year() + 1, end.year());
            proptest::prop_assert!(start <= date && date <= end, "{date} outside {start}..={end}");

            let next = date.succ_opt().unwrap();
            proptest::prop_assert_eq!(
                tax_year_for(next) != tax_year_for(date),
                (next.month(), next.day()) == (7, 1)
            );
            proptest::prop_assert!(tax_year_for(next) - tax_year_for(date) <= 1);
        }
    }
}
