//! The cost base indexation that replaces the 50% CGT discount from
//! 1 July 2027 — the factor's arithmetic and the rules that decide when it
//! applies, stated once so no report can index a cost differently from another.
//!
//! This is the **sibling** of [`crate::domain::indexation`], not a widening of
//! it. The frozen module answers the pre-21-September-1999 election: it
//! hard-wires 68.7 (the September 1999 CPI) as every factor's numerator and
//! refuses any cost incurred after that day. The reform's factor has a moving
//! numerator — the CPI of the quarter the **CGT event** happens in — and a
//! denominator that can only be a quarter from 1 July 2027 onward, so the two
//! cannot share an implementation without one of them stating a rule it does
//! not obey.
//!
//! # The law this encodes
//!
//! From 1 July 2027 an Australian-resident individual or trust working out a
//! capital gain indexes expenditure in each element of the cost base except the
//! third (new s 110-36(1A); ss 114-1 and 114-10; EM 1.33–1.74, mirrored in
//! [`docs/ato/cgt-reform-cgt-adjustments.md`]). The rules, each with its
//! provision:
//!
//! 1. **The factor** is the index number for the quarter in which the CGT
//!    event happens over that for the quarter in which the expenditure is
//!    incurred (s 960-275(1B)) — or, for the first element of the cost base of
//!    a **share or unit**, the quarter in which the amount was *paid*
//!    (s 960-275(1C)). Both are worked out **to 3 decimal places, rounding up
//!    if the fourth decimal place is 5 or more** (s 960-275(5), which applies
//!    to every indexation factor the section states; the Bill inserted
//!    (1B)/(1C) without amending it). This app's data model holds one
//!    expenditure date per parcel, so (1B) and (1C) coincide today — the
//!    separate first-element rule becomes reachable only with a call-payment
//!    fact, which does not exist (see the *Partly paid shares* gap).
//! 2. **Only expenditure incurred on or after 1 July 2027 is indexed**
//!    (s 960-275(1B)(b)). An asset held across the boundary is dealt with by
//!    the deemed disposal and reacquisition in Subdivision 112-E — its
//!    pre-2027 gain is a *deferred* component assessed under the old law — and
//!    the reacquired cost base is taken to have been incurred on 1 July 2027,
//!    so the earliest denominator is the quarter ending 30 September 2027
//!    (EM 1.72). This app has not built that split, so an allocation whose
//!    expenditure predates the reform is not indexed here and the report that
//!    would need the split refuses the event outright.
//! 3. **The asset must have been owned for at least 12 months** (s 114-10(1)).
//!    The one ownership test is [`crate::domain::cgt_discount::discount_eligible`],
//!    so a parcel's discount clock and its indexation clock cannot disagree;
//!    the deemed sale and reacquisition is **disregarded** for it (the sixth
//!    exception, s 114-10(2), EM 1.56–1.57/1.61), which is why callers pass the
//!    parcel's *actual* acquisition date rather than the deemed reacquisition.
//! 4. **A capital loss is never indexed** — the reduced cost base is not
//!    indexed (EM 1.38) — so a disposal whose proceeds do not exceed the cost
//!    base takes the unindexed figure.
//! 5. **The residency testing period** (s 114-25): indexation is unavailable
//!    where the individual was a foreign or temporary resident at any time
//!    between 1 July 2027 (or acquisition, if later) and the event. This app
//!    models one Australian-resident individual ([`crate::reports::TAXPAYER_BASIS`]),
//!    so production passes [`Residency::AustralianResidentThroughout`]; the
//!    foreign/temporary answer exists so the rule is stated and testable rather
//!    than omitted, and recording a real residency history is a separate scope
//!    decision (`docs/API.md` Known limitations).
//! 6. **The third element is never indexed** (s 960-275(4)). The app records no
//!    costs of owning an asset, so the rule is structural: a [`CostBase`]
//!    carries only the acquisition cost and the AMIT/return-of-capital
//!    *reductions*.
//! 7. **Currency**: indexation multiplies the amount in the cost base, and for
//!    a foreign-currency asset the elements of the cost base are AUD figures —
//!    the money paid is translated at the exchange rate when the expenditure is
//!    incurred (s 960-50; `docs/ato/forex-common-transactions.md`). The pipeline
//!    therefore converts to AUD first ([`CostBase::into_aud_with`]) and this
//!    module indexes the AUD figure, which is also the only observable choice:
//!    the pipeline fuses the conversion into one number.
//!
//! **Facially similar, deliberately not shared**: the AMIT (E10) and
//! return-of-capital (G1) *reductions* are applied to the indexed figure at
//! face value and the result floors at nil, exactly as the frozen module does —
//! [`crate::domain::indexation::indexed_cost_base`] is the one implementation
//! of that arithmetic, and this module calls it (see [`indexed_cost_base`]).
//!
//! [`docs/ato/cgt-reform-cgt-adjustments.md`]: ../../docs/ato/cgt-reform-cgt-adjustments.md
//! [`CostBase::into_aud_with`]: crate::domain::cost_base::CostBase::into_aud_with

use chrono::NaiveDate;
use rust_decimal::{Decimal, RoundingStrategy};
use sqlx::Row;

use crate::domain::cgt_discount;
use crate::domain::cgt_reform::{self, COMMENCEMENT};
use crate::domain::cost_base::CostBase;
use crate::domain::indexation::quarter_end_for;

/// The residency answer the s 114-25 testing period needs.
///
/// It is an enum rather than a `bool` because the question the section asks is
/// about a *period* ("was the individual a foreign or temporary resident at any
/// time between … and the event?"), not a status on one day; stating it as a
/// yes/no answer keeps the period's construction with whoever records it, and
/// keeps the two possible answers named at every call site.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Residency {
    /// An Australian resident throughout the testing period.
    AustralianResidentThroughout,
    /// A foreign resident or temporary resident at some time inside it, which
    /// denies indexation for the whole disposal.
    ///
    /// Test-only until the app records a residency history: production's one
    /// taxpayer is the assumed resident ([`ASSUMED_RESIDENCY`]), so nothing in
    /// the server can construct this answer — but the rule it states is still
    /// live, and the test that a foreign or temporary period denies indexation
    /// is what keeps it from being an omission.
    #[cfg(test)]
    ForeignOrTemporaryAtSomeTime,
}

/// The app's hard-wired residency assumption: one Australian-resident
/// individual (`reports::TAXPAYER_BASIS`). Every production caller passes this,
/// because the app records no residency history; the foreign/temporary arm is
/// reachable only from a test until a per-period residency fact exists.
pub const ASSUMED_RESIDENCY: Residency = Residency::AustralianResidentThroughout;

/// Why a disposal's cost base is not indexed under the reform. Each is a rule
/// of the new law rather than a failure, and each is surfaced so a reader can
/// check the classification instead of inferring it from an absent factor.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NotIndexed {
    /// The CGT event is before 1 July 2027, so the old law governs it.
    PreCommencementEvent,
    /// The expenditure was incurred before 1 July 2027 (s 960-275(1B)(b)): an
    /// asset held across the boundary needs the Subdivision 112-E deferred-gain
    /// split, which this app does not compute.
    PreCommencementExpenditure,
    /// The taxpayer was a foreign or temporary resident at some time in the
    /// s 114-25 testing period.
    ResidencyTestingPeriod,
    /// The asset was owned for 12 months or less (s 114-10(1)).
    HeldTwelveMonthsOrLess,
    /// The disposal produced no gain, and a capital loss's reduced cost base is
    /// never indexed (EM 1.38).
    CapitalLoss,
    /// The CPI table carries no index number for the expenditure or the event
    /// quarter. Failing closed is deliberate: an absent index number must
    /// produce an unindexed cost base, never a factor of 1.
    NoCpiForQuarter,
}

impl std::fmt::Display for NotIndexed {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            NotIndexed::PreCommencementEvent => "the CGT event is before 1 July 2027",
            NotIndexed::PreCommencementExpenditure => {
                "the expenditure was incurred before 1 July 2027"
            }
            NotIndexed::ResidencyTestingPeriod => {
                "the taxpayer was a foreign or temporary resident in the testing period"
            }
            NotIndexed::HeldTwelveMonthsOrLess => "the asset was owned for 12 months or less",
            NotIndexed::CapitalLoss => "the disposal produced no gain, and a loss is not indexed",
            NotIndexed::NoCpiForQuarter => "the CPI table has no index number for a quarter",
        })
    }
}

/// The working behind one indexed cost base: the two quarters' index numbers
/// and the factor the rounding rule produced. Returned together so a report can
/// show *which* quarters and figures produced the factor rather than presenting
/// a number the reader cannot check against the ABS's published series.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IndexationFactor {
    /// Quarter the expenditure was incurred in — the factor's denominator row.
    pub expenditure_quarter_end: NaiveDate,
    /// Quarter the CGT event happened in — the factor's numerator row.
    pub event_quarter_end: NaiveDate,
    /// That expenditure quarter's index number, verbatim as published.
    pub expenditure_cpi: Decimal,
    /// That event quarter's index number, verbatim as published.
    pub event_cpi: Decimal,
    /// `event_cpi ÷ expenditure_quarter_cpi`, worked out to 3 decimal places
    /// rounding the fourth up from 5 (s 960-275(5)).
    pub factor: Decimal,
}

impl IndexationFactor {
    /// The indexed cost base of the units `cost` was computed for, AUD: the
    /// costed initial cost multiplied by the factor, less the AMIT and
    /// return-of-capital reductions at face value, floored at nil.
    ///
    /// Delegates to [`crate::domain::indexation::indexed_cost_base`] — the one
    /// implementation of that step, so the frozen and current methods apply the
    /// reductions and the floor identically.
    pub fn apply(&self, cost: &CostBase) -> Decimal {
        crate::domain::indexation::indexed_cost_base(cost, self.factor)
    }
}

/// The current ABS quarterly CPI series, pre-loaded so a report loop's
/// per-parcel lookup is a map read rather than a DB round-trip — the same shape
/// [`crate::domain::indexation::CpiQuarters`] and `infra::fx::FxRates` take for
/// the same reason.
#[derive(Debug, Clone, Default)]
pub struct CurrentCpiQuarters {
    quarters: std::collections::HashMap<NaiveDate, Decimal>,
}

impl CurrentCpiQuarters {
    /// Read the whole table on the caller's connection, so it joins the
    /// report's own single-snapshot read transaction.
    pub async fn load(conn: &mut sqlx::SqliteConnection) -> Result<Self, sqlx::Error> {
        let rows = sqlx::query("SELECT quarter_end, cpi FROM current_cpi_quarters")
            .fetch_all(&mut *conn)
            .await?;
        let mut quarters = std::collections::HashMap::with_capacity(rows.len());
        for row in &rows {
            let quarter_end: NaiveDate = row.try_get("quarter_end")?;
            let cpi = crate::infra::decimal::row_dec(row, "cpi")?;
            quarters.insert(quarter_end, cpi);
        }
        Ok(Self { quarters })
    }

    /// The factor's working for an expenditure/event pair, or `None` when the
    /// table carries no index number for either quarter. Does **not** apply any
    /// of the availability rules — [`reform_indexation`] does that.
    pub fn factor_for(&self, expenditure: NaiveDate, event: NaiveDate) -> Option<IndexationFactor> {
        let expenditure_quarter_end = quarter_end_for(expenditure);
        let event_quarter_end = quarter_end_for(event);
        let expenditure_cpi = self.quarters.get(&expenditure_quarter_end).copied()?;
        let event_cpi = self.quarters.get(&event_quarter_end).copied()?;
        Some(IndexationFactor {
            expenditure_quarter_end,
            event_quarter_end,
            expenditure_cpi,
            event_cpi,
            factor: indexation_factor(event_cpi, expenditure_cpi)?,
        })
    }

    /// How many quarters the table carries — for a report's own sanity checks
    /// and the migration's classification test, never a calculation.
    #[cfg(test)]
    pub fn len(&self) -> usize {
        self.quarters.len()
    }
}

/// s 960-275(5): `event_cpi ÷ expenditure_cpi`, **worked out to 3 decimal
/// places, rounding up if the fourth decimal place is 5 or more** (the section
/// says exactly that; the same rule the frozen method states). `None` for a
/// non-positive denominator (unreachable against the published series; a
/// divide-by-zero must never surface as a panicked report).
pub fn indexation_factor(event_cpi: Decimal, expenditure_cpi: Decimal) -> Option<Decimal> {
    if expenditure_cpi <= Decimal::ZERO {
        return None;
    }
    Some(
        (event_cpi / expenditure_cpi)
            .round_dp_with_strategy(3, RoundingStrategy::MidpointAwayFromZero),
    )
}

/// The reform's answer for one disposal allocation: either the cost base to
/// assess the gain against, indexed, or the rule that left it unindexed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReformIndexation {
    /// Index the cost base to the event quarter.
    Indexed {
        /// The indexed cost base of the costed units, AUD.
        cost_base: Decimal,
        /// The factor and the two quarters it came from.
        factor: IndexationFactor,
    },
    /// The cost base stands unindexed, for this stated rule.
    NotIndexed(NotIndexed),
}

impl ReformIndexation {
    /// The cost base to assess the disposal against, AUD — the indexed figure
    /// where indexation applies, otherwise the caller's own adjusted cost base.
    pub fn cost_base_or(&self, unindexed: Decimal) -> Decimal {
        match self {
            ReformIndexation::Indexed { cost_base, .. } => *cost_base,
            ReformIndexation::NotIndexed(_) => unindexed,
        }
    }

    /// The factor applied, where one was.
    pub fn factor(&self) -> Option<IndexationFactor> {
        match self {
            ReformIndexation::Indexed { factor, .. } => Some(*factor),
            ReformIndexation::NotIndexed(_) => None,
        }
    }
}

/// Whether the reform's indexation applies to one disposal allocation, and the
/// indexed cost base if it does.
///
/// Every input is the *allocation's own*:
///
/// - `cost` — the shared pipeline's AUD [`CostBase`] for the costed units,
///   already reduced by any AMIT (E10) and return-of-capital (G1) events up to
///   the disposal ([`crate::domain::cost_base::adjusted_cost_base`] then
///   [`CostBase::into_aud_with`]).
/// - `proceeds` — the allocation's AUD share of the sale proceeds.
/// - `expenditure` — when the cost was **incurred**. For an ordinary parcel
///   that is its trade date; a parcel whose cost was carried forward by a
///   rollover (or an inheritance) incurred it earlier, and callers must not
///   pass such a parcel's own trade date. This app assesses only plain parcels
///   under the new law and refuses the rest, so that rule is enforced by the
///   caller, not guessed here.
/// - `acquired` — the parcel's CGT acquisition date, deemed-aware
///   ([`crate::domain::cost_base::ParcelRow::acquired`]), for the 12-month
///   ownership rule. The deemed 1 July 2027 reacquisition is *not* passed here
///   — it is disregarded for that rule (s 114-10(2)).
/// - `event` — the CGT event's date.
/// - `cpi` — the pre-loaded current CPI series.
/// - `residency` — the s 114-25 answer.
pub fn reform_indexation(
    cost: &CostBase,
    proceeds: Decimal,
    expenditure: NaiveDate,
    acquired: NaiveDate,
    event: NaiveDate,
    cpi: &CurrentCpiQuarters,
    residency: Residency,
) -> ReformIndexation {
    use ReformIndexation::{Indexed, NotIndexed as No};

    if !cgt_reform::applies_to_event(event) {
        return No(NotIndexed::PreCommencementEvent);
    }
    if expenditure < COMMENCEMENT {
        return No(NotIndexed::PreCommencementExpenditure);
    }
    if residency != Residency::AustralianResidentThroughout {
        return No(NotIndexed::ResidencyTestingPeriod);
    }
    if !cgt_discount::discount_eligible(acquired, event) {
        return No(NotIndexed::HeldTwelveMonthsOrLess);
    }
    // A loss's reduced cost base is not indexed; the test is the gain the
    // unindexed cost base produces, because indexing can only ever increase the
    // cost base and so can never turn a loss into a gain.
    if proceeds <= cost.adjusted {
        return No(NotIndexed::CapitalLoss);
    }
    let Some(factor) = cpi.factor_for(expenditure, event) else {
        return No(NotIndexed::NoCpiForQuarter);
    };
    Indexed {
        cost_base: factor.apply(cost),
        factor,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d(y: i32, m: u32, day: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, day).unwrap()
    }

    fn dec(s: &str) -> Decimal {
        s.parse().unwrap()
    }

    fn cost(initial: &str) -> CostBase {
        CostBase {
            initial_cost: dec(initial),
            costed_initial_cost: dec(initial),
            amit_reduction: Decimal::ZERO,
            roc_reduction: Decimal::ZERO,
            adjusted: dec(initial),
        }
    }

    /// A table carrying the quarters a post-reform disposal needs, matching the
    /// RBA G1 publication the import reads.
    fn cpi() -> CurrentCpiQuarters {
        let mut quarters = std::collections::HashMap::new();
        quarters.insert(d(2027, 9, 30), dec("110.85"));
        quarters.insert(d(2027, 12, 31), dec("111.40"));
        quarters.insert(d(2028, 3, 31), dec("112.10"));
        quarters.insert(d(2028, 6, 30), dec("113.00"));
        quarters.insert(d(2028, 9, 30), dec("112.20"));
        CurrentCpiQuarters { quarters }
    }

    /// The factor is the event quarter's index number over the expenditure
    /// quarter's, at the section's own rounding (s 960-275(5)).
    #[test]
    fn the_factor_is_the_event_quarter_over_the_expenditure_quarter() {
        let f = cpi().factor_for(d(2027, 9, 30), d(2027, 12, 31)).unwrap();
        assert_eq!(f.expenditure_quarter_end, d(2027, 9, 30));
        assert_eq!(f.event_quarter_end, d(2027, 12, 31));
        assert_eq!(f.expenditure_cpi, dec("110.85"));
        assert_eq!(f.event_cpi, dec("111.40"));
        // 111.40 ÷ 110.85 = 1.004961…, whose fourth decimal is a 9.
        assert_eq!(f.factor, dec("1.005"));
    }

    /// The rounding rule is s 960-275(5)'s own words, with its own example
    /// shape: the fourth decimal place of 5 or more rounds the third up.
    #[test]
    fn the_factor_rounds_the_fourth_decimal_up_from_five() {
        assert_eq!(
            indexation_factor(dec("1.4125"), Decimal::ONE),
            Some(dec("1.413"))
        );
        assert_eq!(
            indexation_factor(dec("1.4124"), Decimal::ONE),
            Some(dec("1.412"))
        );
        assert_eq!(indexation_factor(dec("100"), Decimal::ZERO), None);
    }

    /// The reform's first possible disposal: a parcel acquired on 1 July 2027
    /// and sold in the September 2028 quarter (over 12 months, as indexation
    /// requires) indexes from the September 2027 quarter, and the AMIT/ROC
    /// reductions come off the indexed figure at face value with the result
    /// floored at nil.
    #[test]
    fn a_post_reform_gain_indexes_from_its_own_expenditure_quarter() {
        let cost = CostBase {
            initial_cost: dec("10000"),
            costed_initial_cost: dec("10000"),
            amit_reduction: dec("300"),
            roc_reduction: dec("200"),
            adjusted: dec("9500"),
        };
        let answer = reform_indexation(
            &cost,
            dec("12000"),
            d(2027, 7, 1),
            d(2027, 7, 1),
            d(2028, 9, 15),
            &cpi(),
            ASSUMED_RESIDENCY,
        );
        let ReformIndexation::Indexed { cost_base, factor } = answer else {
            panic!("a post-reform gain held over 12 months must index: {answer:?}");
        };
        // 112.20 ÷ 110.85 = 1.012179…, whose fourth decimal is a 1.
        assert_eq!(factor.factor, dec("1.012"));
        // 10,000 × 1.012 = 10,120, less the 500 of reductions at face value.
        assert_eq!(cost_base, dec("9620.000"));
    }

    /// EM 1.56–1.57 / Example 1.1: the 12-month rule counts *actual* continuous
    /// ownership, so a parcel acquired well before the reform and sold after it
    /// is over 12 months. This app does not yet compute the deferred split, so
    /// the engine refuses indexation for the pre-reform expenditure — the
    /// ownership clock is not what stopped it.
    #[test]
    fn pre_reform_expenditure_is_not_indexed() {
        let answer = reform_indexation(
            &cost("10000"),
            dec("12000"),
            d(2025, 8, 11),
            d(2025, 8, 11),
            d(2028, 1, 2),
            &cpi(),
            ASSUMED_RESIDENCY,
        );
        assert_eq!(
            answer,
            ReformIndexation::NotIndexed(NotIndexed::PreCommencementExpenditure)
        );
        assert_eq!(answer.cost_base_or(dec("10000")), dec("10000"));
    }

    /// The 12-month ownership rule is exactly the discount's, so the two clocks
    /// cannot disagree: exactly 12 months misses, a day more qualifies.
    #[test]
    fn the_ownership_rule_is_the_discount_clock() {
        let at = |event: NaiveDate| {
            reform_indexation(
                &cost("10000"),
                dec("12000"),
                d(2027, 7, 1),
                d(2027, 7, 1),
                event,
                &cpi(),
                ASSUMED_RESIDENCY,
            )
        };
        assert_eq!(
            at(d(2028, 7, 1)),
            ReformIndexation::NotIndexed(NotIndexed::HeldTwelveMonthsOrLess)
        );
        assert!(matches!(
            at(d(2028, 9, 15)),
            ReformIndexation::Indexed { .. }
        ));
    }

    /// Never index a capital loss: a disposal whose proceeds do not exceed the
    /// cost base takes the unindexed figure, however old the parcel is.
    #[test]
    fn a_capital_loss_is_not_indexed() {
        let answer = reform_indexation(
            &cost("10000"),
            dec("9000"),
            d(2027, 7, 1),
            d(2027, 7, 1),
            d(2028, 8, 1),
            &cpi(),
            ASSUMED_RESIDENCY,
        );
        assert_eq!(
            answer,
            ReformIndexation::NotIndexed(NotIndexed::CapitalLoss)
        );
        // Proceeds exactly equal to the cost base is not a gain either.
        let flat = reform_indexation(
            &cost("10000"),
            dec("10000"),
            d(2027, 7, 1),
            d(2027, 7, 1),
            d(2028, 8, 1),
            &cpi(),
            ASSUMED_RESIDENCY,
        );
        assert_eq!(flat, ReformIndexation::NotIndexed(NotIndexed::CapitalLoss));
    }

    /// s 114-25: a foreign or temporary resident at any time in the testing
    /// period cannot index.
    #[test]
    fn a_foreign_or_temporary_resident_cannot_index() {
        let answer = reform_indexation(
            &cost("10000"),
            dec("12000"),
            d(2027, 7, 1),
            d(2027, 7, 1),
            d(2028, 8, 1),
            &cpi(),
            Residency::ForeignOrTemporaryAtSomeTime,
        );
        assert_eq!(
            answer,
            ReformIndexation::NotIndexed(NotIndexed::ResidencyTestingPeriod)
        );
    }

    /// An event before the commencement is not the reform's at all, whatever
    /// else is true.
    #[test]
    fn a_pre_commencement_event_is_old_law() {
        let answer = reform_indexation(
            &cost("10000"),
            dec("12000"),
            d(2027, 7, 1),
            d(2027, 7, 1),
            d(2027, 6, 30),
            &cpi(),
            ASSUMED_RESIDENCY,
        );
        assert_eq!(
            answer,
            ReformIndexation::NotIndexed(NotIndexed::PreCommencementEvent)
        );
    }

    /// An absent index number fails closed: no factor of 1, no indexation.
    #[test]
    fn a_missing_quarter_yields_no_indexation() {
        let answer = reform_indexation(
            &cost("10000"),
            dec("12000"),
            d(2027, 7, 1),
            d(2027, 7, 1),
            d(2029, 1, 1), // the March 2029 quarter is not in the table
            &cpi(),
            ASSUMED_RESIDENCY,
        );
        assert_eq!(
            answer,
            ReformIndexation::NotIndexed(NotIndexed::NoCpiForQuarter)
        );
    }

    /// The current series lives in its own table, on its own range: the frozen
    /// `cpi_quarters` still ends at the September 1999 quarter, and this table
    /// starts at the September 2027 one.
    #[tokio::test]
    async fn the_current_table_is_the_reform_range_only() {
        let pool = crate::test_support::test_pool().await;
        let mut conn = pool.acquire().await.unwrap();
        let current = CurrentCpiQuarters::load(&mut conn).await.unwrap();
        assert_eq!(current.len(), 0);
        assert!(current.factor_for(d(2027, 7, 1), d(2027, 12, 20)).is_none());

        sqlx::query(
            "INSERT INTO current_cpi_quarters (quarter_end, cpi) VALUES ('2027-09-30', '110.85')",
        )
        .execute(&mut *conn)
        .await
        .unwrap();
        let loaded = CurrentCpiQuarters::load(&mut conn).await.unwrap();
        assert_eq!(loaded.len(), 1);
        assert_eq!(
            loaded.factor_for(d(2027, 7, 1), d(2027, 9, 30)),
            Some(IndexationFactor {
                expenditure_quarter_end: d(2027, 9, 30),
                event_quarter_end: d(2027, 9, 30),
                expenditure_cpi: dec("110.85"),
                event_cpi: dec("110.85"),
                factor: Decimal::ONE,
            })
        );
    }

    /// The table refuses a pre-reform quarter, so the old method's range cannot
    /// leak into the new series (or the reverse) through the database.
    #[tokio::test]
    async fn a_pre_reform_quarter_is_refused_by_the_table() {
        let pool = crate::test_support::test_pool().await;
        let err = sqlx::query(
            "INSERT INTO current_cpi_quarters (quarter_end, cpi) VALUES ('2027-06-30', '110.00')",
        )
        .execute(&pool)
        .await
        .unwrap_err();
        assert!(err.to_string().to_lowercase().contains("check"), "{err}");
    }
}
