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
///
/// This is the *unbounded* reading of `None`, for reads whose job is "every
/// recorded fact" (the allocations behind an FY-keyed report, the listings a
/// price import should cover). A read presented to the user as the **live**
/// position uses [`as_of_or_today`] instead — see its note.
pub fn as_of_or_open(as_of: Option<NaiveDate>) -> NaiveDate {
    as_of.unwrap_or_else(open_ended_cutoff)
}

/// Today, in the server's local time zone — the cutoff a live holdings view
/// means by "now".
pub fn today() -> NaiveDate {
    chrono::Local::now().date_naive()
}

/// Resolve an optional as-of date for a *live* view: `Some(date)` passes
/// through; `None` means "as at today" ([`today`]), **not** the open-ended
/// cutoff.
///
/// A corporate action is normally recorded when its terms are announced,
/// weeks before it takes effect, so a live view resolved to
/// [`open_ended_cutoff`] applies a not-yet-effective split or return of
/// capital to today's holdings while the as-of-dated reports correctly ignore
/// it — two reports, one database, one day, two answers. Trades are bounded
/// the same way rather than carved out: a future-dated trade is nearly always
/// a typo, and it surfaces on its own date either way.
pub fn as_of_or_today(as_of: Option<NaiveDate>) -> NaiveDate {
    as_of.unwrap_or_else(today)
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
        assert_eq!(
            as_of_or_open(None),
            NaiveDate::from_ymd_opt(9999, 12, 31).unwrap()
        );
    }

    #[test]
    fn some_passes_through_the_live_resolver() {
        let d = NaiveDate::from_ymd_opt(2024, 6, 30).unwrap();
        assert_eq!(as_of_or_today(Some(d)), d);
    }

    /// The live view's `None` is *today*, not the open-ended cutoff — the two
    /// resolvers deliberately disagree, and a live holdings read must reach
    /// for this one (SCENARIOS E-14).
    #[test]
    fn none_is_today_for_a_live_view() {
        assert_eq!(as_of_or_today(None), today());
        assert_ne!(as_of_or_today(None), as_of_or_open(None));
    }

    #[test]
    fn cutoff_sorts_after_a_plausible_date_as_text() {
        // The whole point: lexicographic ordering on the ISO string must hold.
        let plausible = NaiveDate::from_ymd_opt(2100, 1, 1).unwrap().to_string();
        assert!(open_ended_cutoff().to_string() > plausible);
    }
}
