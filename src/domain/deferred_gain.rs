//! The Subdivision 112-E deemed disposal and reacquisition at the 1 July 2027
//! boundary: one disposal allocation's gain split into a **deferred** pre-2027
//! component assessed under the old law and a **current** post-2027 component
//! assessed under the reform's indexation.
//!
//! # The law this encodes
//!
//! New Subdivision 112-E (ss 112-155, 112-160, 112-165, 112-170; EM
//! 1.107–1.131 and 1.194–1.211, mirrored in
//! [`docs/ato/cgt-reform-cgt-adjustments.md`]):
//!
//! 1. **A CGT asset held immediately before 1 July 2027** — acquired on or
//!    before 30 June 2027 and still held on 1 July 2027 — is deemed to be
//!    disposed of just before that day and reacquired just after it, at the
//!    asset's **market value** (s 112-155(2)/(3), s 112-165(2)/(3); EM 1.117,
//!    1.121). [`crate::domain::cgt_reform::DEEMED_DISPOSAL`] is the deemed
//!    disposal date and [`crate::domain::cgt_reform::COMMENCEMENT`] the deemed
//!    reacquisition.
//! 2. **The first component** — the *initial deferred gain or loss* — is the
//!    deemed disposal's gain computed under the law as it stood before the
//!    reform: capital proceeds of the market value, against the cost base the
//!    asset then carried. It is **disregarded** at the deemed disposal
//!    (s 112-160(1)/(2)) and instead included in the income year of the
//!    **real** realisation event, alongside the second component
//!    (s 112-160(3), EM 1.109, 1.113, 1.125–1.126). It keeps the old law's
//!    character: it "will be a discount capital gain if the initial deferred
//!    gain was a discount capital gain" (s 112-160(4), EM 1.110).
//! 3. **The second component** is the gain on the real disposal measured from
//!    the reacquired cost base, computed under the amended law — the
//!    reacquired market value indexed by the CPI movement from the quarter
//!    **beginning 1 July 2027** to the quarter of the event (s 960-275(1B);
//!    EM 1.72, 1.112, 1.129).
//! 4. **The 12-month ownership rule counts actual continuous ownership**: the
//!    deemed sale and reacquisition is the sixth exception to it (s 114-10(2),
//!    EM 1.56–1.57), so both components test ownership from the parcel's real
//!    acquisition date to the real event date — [`crate::domain::cgt_discount`]
//!    is the one clock for both this split and the ordinary discount.
//! 5. **The reductions still apply**: an AMIT (CGT event E10) or
//!    return-of-capital (G1) event arising **after** the boundary reduces the
//!    reacquired cost base at face value, exactly as it would have reduced any
//!    other cost base — the pre-boundary reductions already shaped the cost
//!    base the deferred component is measured against, and the market value
//!    replaces that figure rather than building on it.
//!
//! **What is deliberately not here.** The Minister's optional apportioning
//! method (s 112-185) is not modelled: the split takes the market value, which
//! is the default the section states, and the choice of any other method is the
//! taxpayer's own manual entry (`docs/API.md` Known limitations). Pre-CGT
//! assets are refused at the trade write path, so the s 112-175 arm is
//! unreachable. The deemed disposal is not computed *nowhere*: it is included
//! in the year of the real disposal, which is what this module returns.
//!
//! [`docs/ato/cgt-reform-cgt-adjustments.md`]: ../../docs/ato/cgt-reform-cgt-adjustments.md

use chrono::NaiveDate;
use rust_decimal::Decimal;

use crate::domain::cgt_discount;
use crate::domain::cgt_indexation::{self, CurrentCpiQuarters, ReformIndexation, Residency};
use crate::domain::cgt_reform::COMMENCEMENT;
use crate::domain::cost_base::CostBase;

/// One disposal allocation's Subdivision 112-E split: the deferred old-law
/// component and the current component's assessment of the reacquired cost
/// base.
///
/// Every figure is AUD. The two components are reported side by side rather
/// than netted, because they keep different characters: only the deferred one
/// can be a discount capital gain, and only the current one can be indexed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DeferredSplit {
    /// The costed units' market value at 30 June 2027, AUD — the deemed
    /// disposal's capital proceeds and the deemed reacquisition's cost base
    /// (both are the one value; EM 1.121).
    pub market_value: Decimal,
    /// The cost base the units carried into the boundary, AUD — the deferred
    /// component's other figure.
    pub boundary_cost_base: Decimal,
    /// The deferred component: `market_value − boundary_cost_base`, signed
    /// (positive = gain, the *initial deferred gain*; negative = the *initial
    /// deferred loss*). It is disregarded at the boundary and included in the
    /// year of the real disposal (s 112-160(1)–(3)).
    pub deferred_gain_loss: Decimal,
    /// Whether the deferred gain keeps the old law's discount character
    /// (s 112-160(4)): true only where it *is* a gain and the units were
    /// actually owned for more than 12 months at the real event — the deemed
    /// sale is disregarded for that rule (s 114-10(2), EM 1.56–1.57).
    pub deferred_discount_eligible: bool,
    /// The reacquired cost base as at 1 July 2027, AUD, after any post-boundary
    /// AMIT/ROC reductions — the *unindexed* figure the current component falls
    /// back to where indexation is unavailable.
    pub reacquired_cost_base: Decimal,
    /// The current component: the real disposal's proceeds against the
    /// reacquired cost base, indexed from the quarter beginning 1 July 2027
    /// ([`crate::domain::cgt_indexation::reform_indexation`] with
    /// [`COMMENCEMENT`] as the expenditure date — EM 1.72).
    pub current: ReformIndexation,
}

impl DeferredSplit {
    /// The deferred gain where it is one, else nil — the figure that enters the
    /// year's discount-eligible or non-discountable bucket.
    pub fn deferred_gain(&self) -> Decimal {
        self.deferred_gain_loss.max(Decimal::ZERO)
    }

    /// The deferred loss where it is one, else nil (a positive amount).
    pub fn deferred_loss(&self) -> Decimal {
        (-self.deferred_gain_loss).max(Decimal::ZERO)
    }

    /// The cost base the current component is assessed against: the indexed
    /// reacquired figure where indexation applies, else the unindexed one.
    pub fn current_cost_base(&self) -> Decimal {
        self.current.cost_base_or(self.reacquired_cost_base)
    }

    /// The current component's gain (positive) or loss (negative).
    pub fn current_gain_loss(&self, proceeds: Decimal) -> Decimal {
        proceeds - self.current_cost_base()
    }

    /// The two components' total gain or loss — the disposal's own assessed
    /// result. Deliberately *not* `proceeds − (boundary_cost_base + indexed
    /// reacquired cost base)`, which is the same money but counts the deemed
    /// proceeds twice; each component's gain is stated against its own
    /// proceeds.
    pub fn total_gain_loss(&self, proceeds: Decimal) -> Decimal {
        self.deferred_gain_loss + self.current_gain_loss(proceeds)
    }
}

/// The Subdivision 112-E split of one allocation, given the figures the caller
/// has already resolved:
///
/// - `market_value` — the costed units' market value at 30 June 2027, AUD. The
///   caller sources it from the listing's stored closing price for
///   [`crate::domain::cgt_reform::DEEMED_DISPOSAL`], in the boundary day's unit
///   basis, converted to AUD; it refuses the disposal where no such price is
///   recorded rather than defaulting to the cost base.
/// - `at_boundary` — the shared cost-base pipeline for the same units with
///   [`crate::domain::cost_base::Held::AsAt`] at the deemed disposal date,
///   converted to AUD at the parcel's acquisition month. `AsAt`, not
///   `DisposedOn`: the deemed disposal is a legal fiction, so an AMMA
///   statement whose year ends on 30 June 2027 still adjusts units that were
///   held on that day.
/// - `at_disposal` — the same pipeline with the real disposal date, used only
///   for the reductions arising **after** the boundary: their AUD face value is
///   the difference between the two walks, so the reacquired cost base is
///   reduced by the events the reacquired asset actually lived through and by
///   nothing that had already shaped the boundary cost base.
/// - `proceeds` — the allocation's AUD share of the real sale's proceeds.
/// - `acquired`, `event` — the parcel's real acquisition date and the real
///   event date, for the 12-month rule (s 114-10(2)).
/// - `cpi`, `residency` — the current ABS series and the s 114-25 answer, as
///   [`crate::domain::cgt_indexation::reform_indexation`] takes them.
//
// The figure count is the split's own: the two components' inputs, the real
// disposal's dates and proceeds, and the two lookup tables. Bundling them into
// a struct would only move the same list one level down.
#[allow(clippy::too_many_arguments)]
pub fn deferred_split(
    market_value: Decimal,
    at_boundary: &CostBase,
    at_disposal: &CostBase,
    proceeds: Decimal,
    acquired: NaiveDate,
    event: NaiveDate,
    cpi: &CurrentCpiQuarters,
    residency: Residency,
) -> DeferredSplit {
    // The reductions that arose after the boundary. The two walks share the
    // pre-boundary events, so differencing their totals isolates the later
    // ones without a second walk over the event lists.
    let post_amit = (at_disposal.amit_reduction - at_boundary.amit_reduction).max(Decimal::ZERO);
    let post_roc = (at_disposal.roc_reduction - at_boundary.roc_reduction).max(Decimal::ZERO);
    let reacquired_cost_base = (market_value - post_amit - post_roc).max(Decimal::ZERO);
    let reacquired = CostBase {
        initial_cost: market_value,
        costed_initial_cost: market_value,
        amit_reduction: post_amit,
        roc_reduction: post_roc,
        adjusted: reacquired_cost_base,
    };
    // The reacquired cost base is taken to have been incurred on 1 July 2027
    // (EM 1.72), so the factor's denominator quarter is the one beginning
    // then — the whole point of the boundary split for indexation.
    let current = cgt_indexation::reform_indexation(
        &reacquired,
        proceeds,
        COMMENCEMENT,
        acquired,
        event,
        cpi,
        residency,
    );
    let deferred_gain_loss = market_value - at_boundary.adjusted;
    DeferredSplit {
        market_value,
        boundary_cost_base: at_boundary.adjusted,
        deferred_gain_loss,
        deferred_discount_eligible: deferred_gain_loss > Decimal::ZERO
            && cgt_discount::discount_eligible(acquired, event),
        reacquired_cost_base,
        current,
    }
}

/// Whether the asset was held at the boundary at all: acquired on or before
/// 30 June 2027 (s 112-155(1)(a)–(c); the deemed disposal is of an asset held
/// "immediately before 1 July 2027"). An asset acquired on or after
/// [`COMMENCEMENT`] has no deferred component — its whole gain is the current
/// one, indexed from its own expenditure quarter.
pub fn held_at_boundary(acquired: NaiveDate) -> bool {
    acquired < COMMENCEMENT
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::cgt_indexation::{ASSUMED_RESIDENCY, NotIndexed};
    use rust_decimal::Decimal;

    fn d(y: i32, m: u32, day: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, day).unwrap()
    }

    fn dec(s: &str) -> Decimal {
        s.parse().unwrap()
    }

    fn cost_base(initial: &str, adjusted: &str) -> CostBase {
        CostBase {
            initial_cost: dec(initial),
            costed_initial_cost: dec(initial),
            amit_reduction: Decimal::ZERO,
            roc_reduction: Decimal::ZERO,
            adjusted: dec(adjusted),
        }
    }

    /// The quarters a boundary-crossing disposal reads: the first the
    /// reacquired cost base can name (EM 1.72) and the event's own.
    fn cpi() -> CurrentCpiQuarters {
        CurrentCpiQuarters::from_quarters([
            (d(2027, 9, 30), dec("110.85")),
            (d(2028, 9, 30), dec("112.20")),
            (d(2029, 3, 31), dec("113.00")),
            (d(2029, 6, 30), dec("113.55")),
        ])
    }

    /// EM Example 1.11 (Otis): shares bought in January 2020 for \$100,000,
    /// worth \$120,000 at 30 June 2027 (a deferred gain of \$20,000), sold on
    /// 1 March 2029 for \$160,000. The current component's factor runs from the
    /// quarter **beginning** 1 July 2027 — 113.00 ÷ 110.85 = 1.019 — so the
    /// reacquired \$120,000 indexes to \$122,280.
    #[test]
    fn the_reacquisition_indexes_from_the_quarter_beginning_first_july_2027() {
        let split = deferred_split(
            dec("120000"),
            &cost_base("100000", "100000"),
            &cost_base("100000", "100000"),
            dec("160000"),
            d(2020, 1, 15),
            d(2029, 3, 1),
            &cpi(),
            ASSUMED_RESIDENCY,
        );
        assert_eq!(split.market_value, dec("120000"));
        assert_eq!(split.boundary_cost_base, dec("100000"));
        assert_eq!(split.deferred_gain_loss, dec("20000"));
        assert!(split.deferred_discount_eligible);
        assert_eq!(split.deferred_gain(), dec("20000"));
        assert_eq!(split.deferred_loss(), Decimal::ZERO);
        let ReformIndexation::Indexed { cost_base, factor } = split.current else {
            panic!("the reacquired cost base must index: {:?}", split.current);
        };
        // The denominator is the September 2027 quarter, not the January 2020
        // one: that is the boundary split's whole effect on the factor.
        assert_eq!(factor.expenditure_quarter_end, d(2027, 9, 30));
        assert_eq!(factor.expenditure_cpi, dec("110.85"));
        assert_eq!(factor.event_quarter_end, d(2029, 3, 31));
        assert_eq!(factor.factor, dec("1.019"));
        assert_eq!(cost_base, dec("122280.000"));
        assert_eq!(split.current_cost_base(), dec("122280.000"));
        assert_eq!(split.current_gain_loss(dec("160000")), dec("37720.000"));
        // Both components are included in the year of the real disposal.
        assert_eq!(split.total_gain_loss(dec("160000")), dec("57720.000"));
    }

    /// The old law's indexation election would have divided by the *January
    /// 2020* quarter's CPI; the reform's denominator is fixed at the boundary,
    /// so a parcel acquired decades earlier and one acquired the day before
    /// the boundary index identically on the reacquired figure. That is EM
    /// 1.72's rule, stated as the thing it is not.
    #[test]
    fn the_denominator_does_not_depend_on_when_the_parcel_was_acquired() {
        let old = deferred_split(
            dec("120000"),
            &cost_base("100000", "100000"),
            &cost_base("100000", "100000"),
            dec("160000"),
            d(1986, 9, 20),
            d(2029, 3, 1),
            &cpi(),
            ASSUMED_RESIDENCY,
        );
        let recent = deferred_split(
            dec("120000"),
            &cost_base("100000", "100000"),
            &cost_base("100000", "100000"),
            dec("160000"),
            d(2027, 6, 30),
            d(2029, 3, 1),
            &cpi(),
            ASSUMED_RESIDENCY,
        );
        assert_eq!(old.current.factor(), recent.current.factor());
        assert_eq!(
            old.current.factor().map(|f| f.expenditure_quarter_end),
            Some(d(2027, 9, 30))
        );
    }

    /// The 12-month rule counts actual continuous ownership across the deemed
    /// event (s 114-10(2), EM 1.56–1.57): a parcel bought on 1 August 2026 is
    /// only 11 months in at the boundary but 13 at the September 2027 event, so
    /// its deferred gain is a discount capital gain — the deemed sale does not
    /// restart the clock.
    #[test]
    fn the_deemed_event_does_not_restart_the_ownership_clock() {
        let split = deferred_split(
            dec("12000"),
            &cost_base("10000", "10000"),
            &cost_base("10000", "10000"),
            dec("13000"),
            d(2026, 8, 1),
            d(2027, 9, 15),
            &cpi(),
            ASSUMED_RESIDENCY,
        );
        assert_eq!(split.deferred_gain_loss, dec("2000"));
        assert!(split.deferred_discount_eligible);
        // The current component is indexable on the same clock.
        assert!(matches!(split.current, ReformIndexation::Indexed { .. }));
    }

    /// A loss is never indexed (EM 1.38), and a deferred *loss* is no discount
    /// gain: the current component falls back to the unindexed reacquired cost
    /// base, and the deferred component reduces the year's gains as a loss.
    #[test]
    fn a_current_loss_takes_the_unindexed_reacquired_cost_base() {
        let split = deferred_split(
            dec("120000"),
            &cost_base("100000", "100000"),
            &cost_base("100000", "100000"),
            dec("110000"),
            d(2020, 1, 15),
            d(2029, 3, 1),
            &cpi(),
            ASSUMED_RESIDENCY,
        );
        assert_eq!(split.deferred_gain_loss, dec("20000"));
        assert!(split.deferred_discount_eligible);
        assert_eq!(
            split.current,
            ReformIndexation::NotIndexed(NotIndexed::CapitalLoss)
        );
        assert_eq!(split.current_cost_base(), dec("120000"));
        assert_eq!(split.current_gain_loss(dec("110000")), dec("-10000"));
        assert_eq!(split.total_gain_loss(dec("110000")), dec("10000"));
    }

    /// A deferred loss is not a discount gain, and the loss is surfaced as a
    /// positive amount.
    #[test]
    fn a_deferred_loss_is_not_a_discount_gain() {
        let split = deferred_split(
            dec("90000"),
            &cost_base("100000", "100000"),
            &cost_base("100000", "100000"),
            dec("130000"),
            d(2020, 1, 15),
            d(2029, 3, 1),
            &cpi(),
            ASSUMED_RESIDENCY,
        );
        assert_eq!(split.deferred_gain_loss, dec("-10000"));
        assert!(!split.deferred_discount_eligible);
        assert_eq!(split.deferred_gain(), Decimal::ZERO);
        assert_eq!(split.deferred_loss(), dec("10000"));
        // 90,000 × 1.019 = 91,710, so the A$130,000 proceeds leave a current
        // gain of A$38,290 against the A$10,000 deferred loss.
        assert_eq!(split.current_gain_loss(dec("130000")), dec("38290.000"));
        assert_eq!(split.total_gain_loss(dec("130000")), dec("28290.000"));
    }

    /// The reductions that arose **after** the boundary come off the
    /// reacquired cost base at face value; the ones that shaped the boundary
    /// cost base do not, because the market value replaced that figure.
    #[test]
    fn only_post_boundary_reductions_reduce_the_reacquired_cost_base() {
        let at_boundary = CostBase {
            initial_cost: dec("100000"),
            costed_initial_cost: dec("100000"),
            amit_reduction: dec("5000"),
            roc_reduction: Decimal::ZERO,
            adjusted: dec("95000"),
        };
        // By the disposal, a further 5,000 of AMIT and 2,000 of ROC have
        // arisen: 10,000 total AMIT and 2,000 total ROC.
        let at_disposal = CostBase {
            initial_cost: dec("100000"),
            costed_initial_cost: dec("100000"),
            amit_reduction: dec("10000"),
            roc_reduction: dec("2000"),
            adjusted: dec("88000"),
        };
        let split = deferred_split(
            dec("120000"),
            &at_boundary,
            &at_disposal,
            dec("160000"),
            d(2020, 1, 15),
            d(2029, 3, 1),
            &cpi(),
            ASSUMED_RESIDENCY,
        );
        // The deferred component is the market value against the boundary cost
        // base — the pre-boundary 5,000 already inside it.
        assert_eq!(split.deferred_gain_loss, dec("25000"));
        // The reacquired base is the market value less only the 5,000 AMIT and
        // 2,000 ROC that arose after it.
        assert_eq!(split.reacquired_cost_base, dec("113000"));
        // The indexed figure is the market value indexed, less the post-boundary
        // reductions at face value: 120,000 × 1.019 − 5,000 − 2,000.
        assert_eq!(split.current_cost_base(), dec("115280.000"));
    }

    /// s 114-25: a foreign or temporary resident at some time in the testing
    /// period cannot index the current component — but the deferred component
    /// is old law and keeps its character.
    #[test]
    fn foreign_residency_denies_indexation_but_not_the_deferred_gain() {
        let split = deferred_split(
            dec("120000"),
            &cost_base("100000", "100000"),
            &cost_base("100000", "100000"),
            dec("160000"),
            d(2020, 1, 15),
            d(2029, 3, 1),
            &cpi(),
            Residency::ForeignOrTemporaryAtSomeTime,
        );
        assert_eq!(split.deferred_gain_loss, dec("20000"));
        assert!(split.deferred_discount_eligible);
        assert_eq!(
            split.current,
            ReformIndexation::NotIndexed(NotIndexed::ResidencyTestingPeriod)
        );
        assert_eq!(split.current_cost_base(), dec("120000"));
    }

    /// An absent CPI quarter fails closed — no factor of 1 — leaving the
    /// reacquired cost base unindexed.
    #[test]
    fn a_missing_event_quarter_leaves_the_reacquired_base_unindexed() {
        let split = deferred_split(
            dec("120000"),
            &cost_base("100000", "100000"),
            &cost_base("100000", "100000"),
            dec("160000"),
            d(2020, 1, 15),
            d(2031, 3, 1),
            &cpi(),
            ASSUMED_RESIDENCY,
        );
        assert_eq!(
            split.current,
            ReformIndexation::NotIndexed(NotIndexed::NoCpiForQuarter)
        );
        assert_eq!(split.current_cost_base(), dec("120000"));
    }

    /// A parcel acquired on or after the commencement was never held at the
    /// boundary: there is no deferred component, and the whole gain is the
    /// current one.
    #[test]
    fn a_post_commencement_parcel_has_no_boundary() {
        assert!(held_at_boundary(d(2027, 6, 30)));
        assert!(!held_at_boundary(d(2027, 7, 1)));
        assert!(!held_at_boundary(d(2028, 1, 1)));
    }
}
