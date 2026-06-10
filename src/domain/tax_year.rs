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
}
