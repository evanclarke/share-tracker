//! The CGT reform commencing 1 July 2027 — the commencement date, the four
//! gain categories and the reform predicates, defined once so no report can
//! disagree about where the old law ends.
//!
//! Sources (both mirrored in `docs/ato/`): the ATO's summary of the enacted
//! measure, *Tax reform – Boosting home ownership* (QC 107304,
//! `docs/ato/cgt-reform-boosting-home-ownership.md`), and Chapter 1 of the
//! Explanatory Memorandum to the Treasury Laws Amendment (Tax Reform No. 1)
//! Act 2026 (`docs/ato/cgt-reform-cgt-adjustments.md`). From 1 July 2027 the
//! reform replaces the 50 per cent CGT discount for Australian-resident
//! individuals and trusts with **cost base indexation** and imposes a **30 per
//! cent minimum tax on capital gains** (EM 1.23–1.24, Table 1.1).
//!
//! **The reform is implemented in part.** Cost base indexation for an
//! expenditure incurred on or after 1 July 2027 is `domain::cgt_indexation`,
//! and a disposal that draws only on such parcels is assessed under it by
//! `reports::realised_gains` (which flows into the net capital gain). The
//! Subdivision 112-E deemed disposal and reacquisition at the boundary is
//! `domain::deferred_gain`, and `reports::realised_gains` splits a disposal
//! that draws on an asset held across 30 June 2027 into its deferred old-law
//! component and its post-2027 component. What is *not* built, and what this
//! module's guards therefore still refuse:
//!
//! - a parcel whose cost was **carried** (a rollover replacement, an
//!   inheritance, a transfer-in) reaching a post-commencement disposal — its
//!   cost was incurred earlier than its own trade date, so neither its own
//!   quarter nor a plain boundary split costs it honestly
//!   ([`guard_deferred_split`]);
//! - the seven-step method statement (the existing loss chain already has the
//!   right shape for the non-residential, non-deferred gains this app holds,
//!   but the deferred categories do not exist yet);
//! - the 30 per cent minimum tax (Division 119); and
//! - the reform's treatment of non-disposal CGT events (E10/G1/C2), AMMA
//!   statements and rights sales, which [`guard_discount_events`] still
//!   refuses wholesale.
//!
//! A trade `date` is bounded above by today
//! (`entities::trade::checks::AmountsError::FutureDate`), so no CGT event on or
//! after [`COMMENCEMENT`] can be recorded until that date arrives — but that
//! ceiling is not a report's guarantee, so every report that would otherwise
//! apply repealed law guards itself here. A refusal reaches the HTTP layer as a
//! logged `500` naming the date; it is never a silent discount.

use chrono::NaiveDate;

/// The commencement date for this app's taxpayer: **1 July 2027**.
///
/// The ATO's summary states the changes "will apply from 1 July 2027"
/// (QC 107304), and the Explanatory Memorandum fixes the application date per
/// provision: Division 119 (the minimum tax) applies to capital gains from CGT
/// events happening on or after 1 July 2027 (EM 1.216), while all the other
/// amendments in Schedule 1 apply in relation to assessments for the income
/// year that includes 1 July 2027 and later income years (EM 1.219) — that
/// income year being 1 July 2027 – 30 June 2028, so a CGT event on or after
/// this date is the first the new law governs.
///
/// The commencement *table* itself (EM 1.214) is a machinery provision: it
/// starts the Act (excluding the minimum-tax provisions, which commence with
/// the Imposition Bill — EM 1.215) on the first 1 January, 1 April, 1 July or
/// 1 October after Royal Assent. Both Acts received Royal Assent in time for
/// the 1 July 2027 quarter, and no reform provision reaches a different date
/// for this app's single Australian-resident individual (the residency and
/// trust arms are separate scope decisions), so this is one definite date
/// rather than a provisional one.
pub const COMMENCEMENT: NaiveDate = match NaiveDate::from_ymd_opt(2027, 7, 1) {
    Some(d) => d,
    None => unreachable!(),
};

/// The date the Subdivision 112-E deemed disposal is taken to happen: the last
/// day of the income year before [`COMMENCEMENT`]. New s 112-155(2) deems an
/// asset held immediately before 1 July 2027 to be disposed of "just before"
/// that day and reacquired just after it (EM 1.117), so the deemed disposal's
/// capital proceeds are the asset's market value at this date and the deemed
/// reacquisition's cost base is that same value taken to have been incurred on
/// [`COMMENCEMENT`] — the reason the indexation denominator for a
/// boundary-crossing asset is the quarter *starting* 1 July 2027 (EM 1.72).
pub const DEEMED_DISPOSAL: NaiveDate = match NaiveDate::from_ymd_opt(2027, 6, 30) {
    Some(d) => d,
    None => unreachable!(),
};

/// The four categories of capital gains the reformed net-capital-gain method
/// statement classifies a year's gains into, in the statutory order (EM 1.82,
/// new s 102-6):
///
/// - **deferred non-residential** — a pre-1 July 2027 gain on an asset that is
///   not a residential dwelling, crystallised by the deemed disposal in
///   Subdivision 112-E (EM 1.91–1.93);
/// - **deferred residential** — the same, to the extent a residential
///   dwelling was used or held to provide residential accommodation before
///   1 July 2027 (EM 1.88–1.90);
/// - **non-residential** — a post-1 July 2027 gain on an asset that is not a
///   residential dwelling (EM 1.86–1.87); and
/// - **residential** — a post-1 July 2027 gain on a residential dwelling used
///   or held to provide residential accommodation (EM 1.83–1.85).
///
/// The order is the order capital losses are applied against the categories
/// in steps 1 and 2 of the method statement (EM 1.94–1.99), so deriving `Ord`
/// from this declaration order is deliberate.
///
/// This project holds shares, units and crypto
/// (`listings.security_type IN ('Share','ETF','LIC','Trust','Crypto')`), so
/// every gain it can produce is **non-residential**: the residential
/// categories stay defined here so the implemented set matches the statute,
/// and a later section records why they are always nil.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum GainCategory {
    DeferredNonResidential,
    DeferredResidential,
    NonResidential,
    Residential,
}

impl GainCategory {
    /// The four categories in the method statement's order (EM 1.82).
    pub const ALL: [GainCategory; 4] = [
        GainCategory::DeferredNonResidential,
        GainCategory::DeferredResidential,
        GainCategory::NonResidential,
        GainCategory::Residential,
    ];

    /// The category's name as the Explanatory Memorandum and s 102-6 give it.
    pub fn label(self) -> &'static str {
        match self {
            GainCategory::DeferredNonResidential => "deferred non-residential capital gains",
            GainCategory::DeferredResidential => "deferred residential capital gains",
            GainCategory::NonResidential => "non-residential capital gains",
            GainCategory::Residential => "residential capital gains",
        }
    }
}

impl std::fmt::Display for GainCategory {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.label())
    }
}

/// Whether the reform governs a CGT event happening on `event_date`: the new
/// law applies to a CGT event on or after [`COMMENCEMENT`] (EM 1.216, 1.219),
/// and for such an event the 50 per cent discount is repealed (EM 1.33,
/// Table 1.1).
pub fn applies_to_event(event_date: NaiveDate) -> bool {
    event_date >= COMMENCEMENT
}

/// Whether the repealed 50 per cent discount may still be applied to a CGT
/// event happening on `event_date` — true up to and including 30 June 2027,
/// the boundary this module's guard tests.
pub fn discount_available(event_date: NaiveDate) -> bool {
    !applies_to_event(event_date)
}

/// A CGT event the reform governs reached a report that still applies the
/// repealed 50 per cent discount. Refused loudly rather than answered: the
/// report cannot honestly assess a post-commencement event, and the write-time
/// date ceiling that normally keeps one out of the database is not a report's
/// guarantee.
#[derive(thiserror::Error, Debug, PartialEq, Eq)]
pub enum CgtReformError {
    /// `what` happened on `event_date`, which is on or after the reform's
    /// commencement, and `site` applies the repealed discount.
    #[error(
        "{what} dated {event_date} is on or after the CGT reform commencement \
         ({commencement}), which this app does not implement; {site} refuses to \
         assess it under the repealed 50% discount, whose replacement \
         classifies gains as {categories}"
    )]
    PostCommencementEvent {
        /// What the dated fact is, e.g. "a Sell" or "a CGT event G1".
        what: &'static str,
        /// The CGT event's own date.
        event_date: NaiveDate,
        /// [`COMMENCEMENT`], carried explicitly so the message always names it.
        commencement: NaiveDate,
        /// The report that refused.
        site: &'static str,
        /// The unimplemented regime's gain categories, so the refusal says what
        /// the event would have needed.
        categories: String,
    },
    /// A CGT event on or after the commencement disposes of an asset whose cost
    /// was not incurred at a date the report can index or split honestly: a
    /// parcel held immediately before 1 July 2027 whose cost was **carried**
    /// forward from an earlier holding (a rollover replacement, an inherited
    /// parcel, a transfer-in) rather than paid for at its own trade date, or a
    /// partial-rollover closing Sell's cash side drawing on such a parcel.
    ///
    /// The plain boundary-crossing case *is* assessed — `domain::deferred_gain`
    /// is the Subdivision 112-E split — but a carried cost needs the split run
    /// on the source parcel each rollover chain leads back to, which this app
    /// does not do. Refused rather than approximated from the replacement's own
    /// trade date, which would silently mis-index it.
    #[error(
        "{what} dated {event_date} disposes of an asset held immediately before the CGT \
         reform commencement ({commencement}; acquired {acquisition_date}); the reform \
         defers the pre-commencement gain and indexes only the post-commencement component \
         (Subdivision 112-E), a split this app does not compute, so {site} refuses to assess \
         the disposal"
    )]
    DeferredSplitRequired {
        /// What the dated fact is, e.g. "a Sell".
        what: &'static str,
        /// The CGT event's own date.
        event_date: NaiveDate,
        /// The parcel's CGT acquisition date, carried so the message names why
        /// the split is needed.
        acquisition_date: NaiveDate,
        /// [`COMMENCEMENT`], carried explicitly so the message always names it.
        commencement: NaiveDate,
        /// The report that refused.
        site: &'static str,
    },
    /// The Subdivision 112-E split needs the asset's market value at
    /// [`DEEMED_DISPOSAL`] — the deemed disposal's capital proceeds and the
    /// deemed reacquisition's cost base — and the listing has no stored closing
    /// price for that day.
    ///
    /// Refused rather than defaulted: taking the parcel's own cost base as the
    /// market value would silently zero the deferred gain, which is the one
    /// error that would go unnoticed in a tax figure.
    #[error(
        "the CGT reform's Subdivision 112-E split needs listing {listing_id}'s market value at \
         {boundary_date} (30 June 2027), and no closing price is recorded for that day — record \
         one (PUT /closing_prices/{listing_id}/{boundary_date}) or backfill the listing's price \
         series, then run the report again"
    )]
    BoundaryValueMissing {
        /// The listing whose boundary value is missing.
        listing_id: i64,
        /// [`DEEMED_DISPOSAL`], carried explicitly so the message names the day.
        boundary_date: NaiveDate,
    },
}

/// The four categories as one list, for [`CgtReformError::PostCommencementEvent`].
fn category_list() -> String {
    GainCategory::ALL
        .iter()
        .map(GainCategory::to_string)
        .collect::<Vec<_>>()
        .join(", ")
}

/// The refusal itself, built in one place so every caller's message names the
/// commencement date and the same category set.
fn refuse(what: &'static str, event_date: NaiveDate, site: &'static str) -> CgtReformError {
    CgtReformError::PostCommencementEvent {
        what,
        event_date,
        commencement: COMMENCEMENT,
        site,
        categories: category_list(),
    }
}

/// Refuse `event_date` if the reform governs it, or `Ok(())` if the repealed
/// discount may still be applied.
pub fn guard_discount_event(
    what: &'static str,
    event_date: NaiveDate,
    site: &'static str,
) -> Result<(), CgtReformError> {
    if discount_available(event_date) {
        Ok(())
    } else {
        Err(refuse(what, event_date, site))
    }
}

/// Refuse the earliest CGT event in `events` that the reform governs (or
/// `Ok(())` when none is). Each item is `(date, what-it-is)`; taking the
/// earliest keeps the refusal deterministic however the caller's rows are
/// ordered.
pub fn guard_discount_events(
    events: impl IntoIterator<Item = (NaiveDate, &'static str)>,
    site: &'static str,
) -> Result<(), CgtReformError> {
    match events
        .into_iter()
        .filter(|(date, _)| !discount_available(*date))
        .min_by_key(|(date, _)| *date)
    {
        Some((event_date, what)) => Err(refuse(what, event_date, site)),
        None => Ok(()),
    }
}

/// Refuse a CGT event the reform governs whose assessment needs the
/// Subdivision 112-E deferred-gain split (see
/// [`CgtReformError::DeferredSplitRequired`]). `Ok(())` for a
/// pre-commencement event, which the old law still governs.
pub fn guard_deferred_split(
    what: &'static str,
    event_date: NaiveDate,
    acquisition_date: NaiveDate,
    site: &'static str,
) -> Result<(), CgtReformError> {
    if discount_available(event_date) {
        return Ok(());
    }
    Err(CgtReformError::DeferredSplitRequired {
        what,
        event_date,
        acquisition_date,
        commencement: COMMENCEMENT,
        site,
    })
}

/// The refusal of a missing Subdivision 112-E boundary value (see
/// [`CgtReformError::BoundaryValueMissing`]): `listing_id`'s market value at
/// [`DEEMED_DISPOSAL`] is not recorded, and the split cannot be computed
/// without it.
pub fn boundary_value_missing(listing_id: i64) -> CgtReformError {
    CgtReformError::BoundaryValueMissing {
        listing_id,
        boundary_date: DEEMED_DISPOSAL,
    }
}

/// Surface the refusal through report code that returns `sqlx::Error` — the
/// decode-error bridge [`crate::infra::fx::FxError`] uses, so the failure
/// propagates to `ApiError::Internal` (a logged `500` naming the date) instead
/// of being swallowed. The `CgtReformError` itself is boxed, not stringified,
/// so the far end can still downcast it.
impl From<CgtReformError> for sqlx::Error {
    fn from(e: CgtReformError) -> Self {
        sqlx::Error::Decode(Box::new(e))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::ymd;

    /// The one date the whole section turns on. Pinned against the ATO's own
    /// statement (QC 107304) and the EM's application rules (1.216, 1.219) —
    /// see the constant's doc for why no provision reaches a different date
    /// for this app's taxpayer.
    #[test]
    fn commencement_is_one_july_2027() {
        assert_eq!(COMMENCEMENT, ymd(2027, 7, 1));
    }

    /// The boundary is the day before commencement: 30 June 2027 is old law,
    /// 1 July 2027 is the reform's first day.
    #[test]
    fn the_boundary_is_thirty_june_2027() {
        assert!(discount_available(ymd(2027, 6, 29)));
        assert!(discount_available(ymd(2027, 6, 30)));
        assert!(!discount_available(ymd(2027, 7, 1)));
        assert!(!discount_available(ymd(2028, 1, 1)));
        assert!(!applies_to_event(ymd(2027, 6, 30)));
        assert!(applies_to_event(ymd(2027, 7, 1)));
        // The deemed disposal is the day before the commencement it precedes
        // (EM 1.117), so the two constants can never drift apart.
        assert_eq!(DEEMED_DISPOSAL, ymd(2027, 6, 30));
        assert_eq!(DEEMED_DISPOSAL.succ_opt(), Some(COMMENCEMENT));
    }

    /// The guard passes a pre-commencement event and refuses a
    /// post-commencement one, always naming the commencement date.
    #[test]
    fn the_guard_passes_old_law_and_refuses_the_reform() {
        let site = "reports::realised_gains";
        assert!(guard_discount_event("a Sell", ymd(2027, 6, 30), site).is_ok());
        let err = guard_discount_event("a Sell", ymd(2027, 7, 1), site).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("2027-07-01"), "{msg}");
        assert!(msg.contains("a Sell"), "{msg}");
        assert!(msg.contains(site), "{msg}");
        // The refusal carries the structured fact, not just prose.
        assert_eq!(
            err,
            CgtReformError::PostCommencementEvent {
                what: "a Sell",
                event_date: ymd(2027, 7, 1),
                commencement: COMMENCEMENT,
                site,
                categories: category_list(),
            }
        );
    }

    /// The multi-event guard takes the earliest offending event, so a report
    /// refuses deterministically whatever order its rows come back in.
    #[test]
    fn the_guard_reports_the_earliest_offending_event() {
        let events = [
            (ymd(2027, 9, 1), "a rights sale"),
            (ymd(2027, 7, 1), "a Sell"),
            (ymd(2027, 8, 1), "an AMMA statement's year end"),
        ];
        let err = guard_discount_events(events, "reports::net_capital_gain").unwrap_err();
        assert!(err.to_string().contains("2027-07-01"), "{err}");
        assert!(err.to_string().contains("a Sell"), "{err}");
    }

    /// The four categories are held in the method statement's order, and the
    /// labels are the statute's own.
    #[test]
    fn the_four_gain_categories_are_in_statutory_order() {
        assert_eq!(
            GainCategory::ALL,
            [
                GainCategory::DeferredNonResidential,
                GainCategory::DeferredResidential,
                GainCategory::NonResidential,
                GainCategory::Residential,
            ]
        );
        // `Ord` follows the declaration order the loss steps consume.
        assert!(GainCategory::DeferredNonResidential < GainCategory::NonResidential);
        assert_eq!(
            GainCategory::Residential.to_string(),
            "residential capital gains"
        );
        assert_eq!(
            GainCategory::DeferredNonResidential.label(),
            "deferred non-residential capital gains"
        );
    }

    /// The deferred-split guard: a disposal on or after 1 July 2027 of an asset
    /// acquired before it is refused, naming both dates and the provision whose
    /// split is missing; a pre-commencement disposal passes.
    #[test]
    fn the_deferred_split_guard_refuses_a_boundary_crossing_disposal() {
        let site = "reports::realised_gains";
        assert!(guard_deferred_split("a Sell", ymd(2027, 6, 30), ymd(2019, 3, 1), site).is_ok());
        let err =
            guard_deferred_split("a Sell", ymd(2029, 9, 1), ymd(2019, 3, 1), site).unwrap_err();
        assert_eq!(
            err,
            CgtReformError::DeferredSplitRequired {
                what: "a Sell",
                event_date: ymd(2029, 9, 1),
                acquisition_date: ymd(2019, 3, 1),
                commencement: COMMENCEMENT,
                site,
            }
        );
        let msg = err.to_string();
        assert!(msg.contains("2029-09-01"), "{msg}");
        assert!(msg.contains("2019-03-01"), "{msg}");
        assert!(msg.contains("112-E"), "{msg}");
    }
}
