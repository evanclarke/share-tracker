//! Shared date helpers for the "as-of" reporting cutoff.

use chrono::NaiveDate;

/// The sentinel "no upper bound" cutoff for as-of queries.
///
/// Dates are stored and compared as `YYYY-MM-DD` TEXT, so the open-ended cutoff
/// has to be a real, lexicographically-maximal date: `9999-12-31` is the largest
/// four-digit-year date and sorts after every plausible trade/price date.
/// `NaiveDate::MAX` is deliberately *not* used — it is year 262143 and renders
/// as `+262143-12-31`, whose leading `+` sorts *before* the digits and would
/// break the string comparison the SQL relies on.
pub fn open_ended_cutoff() -> NaiveDate {
    NaiveDate::from_ymd_opt(9999, 12, 31).expect("9999-12-31 is a valid date")
}

/// Resolve an optional as-of date to a concrete cutoff: `Some(date)` passes
/// through; `None` means "no upper bound" and maps to [`open_ended_cutoff`].
pub fn as_of_or_open(as_of: Option<NaiveDate>) -> NaiveDate {
    as_of.unwrap_or_else(open_ended_cutoff)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn some_passes_through() {
        let d = NaiveDate::from_ymd_opt(2024, 6, 30).unwrap();
        assert_eq!(as_of_or_open(Some(d)), d);
    }

    #[test]
    fn none_is_the_open_ended_cutoff() {
        assert_eq!(as_of_or_open(None), NaiveDate::from_ymd_opt(9999, 12, 31).unwrap());
    }

    #[test]
    fn cutoff_sorts_after_a_plausible_date_as_text() {
        // The whole point: lexicographic ordering on the ISO string must hold.
        let plausible = NaiveDate::from_ymd_opt(2100, 1, 1).unwrap().to_string();
        assert!(open_ended_cutoff().to_string() > plausible);
    }
}
