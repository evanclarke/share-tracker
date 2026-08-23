//! Costing a disposal that has **not been recorded yet** — the parcel
//! optimiser's and the pre-sale what-if's estimate of the very Sell they
//! rehearse.
//!
//! Both of those endpoints are decision support: the user reads a cost base
//! off them, picks a strategy, and then enters the Sell. The estimate is only
//! worth anything if it is the figure `reports::realised_gains` will report
//! once that Sell exists, so this module's single job is to produce **that**
//! figure — `domain::cost_base::adjusted_cost_base` fed exactly the inputs the
//! realised-gains report will feed it, one step in the future.
//!
//! Two of those inputs are not what an open-holdings read
//! ([`domain::open_parcels::load`](crate::domain::open_parcels::load)) hands
//! back, and getting either wrong is a silent divergence of the kind
//! `domain/` exists to prevent:
//!
//! 1. **The AMMA statements in scope.** `open_parcels::load` bounds them at
//!    its as-of date, because an adjustment arises at its statement's year
//!    end and a *valuation* as at an earlier day must not anticipate it. A
//!    disposal is not a valuation: s 104-107B makes the AMIT cost-base
//!    adjustment just before the end of the income year **or just before the
//!    time of a relevant CGT event** (LCR 2015/11 para 13), so a statement
//!    for the year the sale falls inside reaches the sold units even though
//!    its year end is still ahead. `realised_gains` reads its statements
//!    unbounded for exactly that reason, and so does [`Costing::load`].
//!
//! 2. **How many units the statement's year end saw disposed of.**
//!    [`AmitReductionEvent::disposed_by_year_end`] is read from the recorded
//!    allocations, and the contemplated sale is by definition not among them.
//!    Recording it would add its units to every statement whose year end is
//!    on or after the sale date — moving those units out of the row's
//!    still-held group and into its disposed-of group — which is the whole
//!    difference between the two branches of
//!    [`AmitReductionEvent::reduction_for_units`]. [`Costing`] therefore adds
//!    them itself, in [`events_with_disposal`].
//!
//! The second point is why an estimate cannot be assembled by costing a whole
//! parcel once and pro-rating: `disposed_by_year_end` moves with the *size* of
//! the contemplated allocation, so the pipeline is **not** linear in the units
//! being disposed of once a partly-covering AMIT row is in play. Every
//! allocation is costed at its own unit count here, which is also what the
//! recorded Sell does.

use std::collections::HashMap;

use chrono::NaiveDate;
use rust_decimal::Decimal;

use crate::domain::cost_base::{self, AmitReductionEvent, ParcelRow};
use crate::entities::amit_adjustment;
use crate::entities::corporate_action::{self, RocEvent, SplitEvent};
use crate::infra::fx::FxRates;

/// The reference data a contemplated disposal is costed against — the same
/// set `reports::realised_gains` loads for a recorded one, read once on the
/// caller's own connection so it joins that report's single-snapshot read
/// transaction.
#[derive(Debug)]
pub struct Costing {
    /// Every parcel's AMMA statements, **unbounded** by date (see the module
    /// doc): a statement for the year the contemplated sale falls inside
    /// still reaches the units it disposes of.
    amit: HashMap<i64, Vec<AmitReductionEvent>>,
    /// Return-of-capital payments (CGT event G1) per listing.
    roc: HashMap<i64, Vec<RocEvent>>,
    /// Share splits/consolidations per listing (quantity re-basing).
    splits: HashMap<i64, Vec<SplitEvent>>,
    /// Every imported ATO FX rate, so the per-parcel conversion is a map
    /// lookup rather than a round trip.
    fx: FxRates,
}

impl Costing {
    pub async fn load(conn: &mut sqlx::SqliteConnection) -> Result<Self, sqlx::Error> {
        Ok(Costing {
            amit: amit_adjustment::db_cost_base_reduction_events(&mut *conn, None).await?,
            roc: corporate_action::db_return_of_capital_events(&mut *conn).await?,
            splits: corporate_action::db_share_split_events(&mut *conn).await?,
            fx: FxRates::load(&mut *conn).await?,
        })
    }

    /// The AUD adjusted cost base of `units` of `parcel` disposed of on
    /// `sale_date` — the figure `reports::realised_gains` will report for that
    /// allocation once the Sell is entered.
    ///
    /// `units` is in `sale_date`'s unit basis (what the caller's candidate
    /// list and price are quoted in); the cost-base arithmetic runs in the
    /// parcel's as-acquired basis, so a split between the two is re-based
    /// away first — the same `as_acquired_quantity` step the realised-gains
    /// report applies to an allocation.
    pub fn adjusted_cost_base_aud(
        &self,
        parcel: &ParcelRow,
        units: Decimal,
        sale_date: NaiveDate,
    ) -> Result<Decimal, sqlx::Error> {
        let splits = self.splits.get(&parcel.listing_id).map_or(&[][..], |v| v);
        let units_acquired =
            corporate_action::as_acquired_quantity(units, splits, parcel.date, sale_date);
        let amit = events_with_disposal(
            self.amit.get(&parcel.id).map_or(&[][..], |v| v),
            units_acquired,
            sale_date,
        );
        Ok(cost_base::adjusted_cost_base(
            &parcel.parcel(),
            units_acquired,
            &amit,
            self.roc.get(&parcel.listing_id).map_or(&[][..], |v| v),
            splits,
            cost_base::Held::DisposedOn(sale_date),
        )?
        .into_aud_with(
            &self.fx,
            &parcel.currency,
            parcel.acquired(),
            parcel.fx_override(),
        )?
        .adjusted)
    }
}

/// A parcel's AMMA statements as they will read **once** a disposal of
/// `units_acquired` as-acquired units on `sale_date` has been recorded: the
/// units join `disposed_by_year_end` for every statement whose year end the
/// sale falls on or before, which is the same boundary
/// `amit_adjustment::db_cost_base_reduction_events` counts the recorded
/// allocations on (`sale_date <= tax_year_end_date`).
fn events_with_disposal(
    events: &[AmitReductionEvent],
    units_acquired: Decimal,
    sale_date: NaiveDate,
) -> Vec<AmitReductionEvent> {
    events
        .iter()
        .map(|e| {
            let mut e = *e;
            if sale_date <= e.tax_year_end_date {
                e.disposed_by_year_end += units_acquired;
            }
            e
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{dec, ymd};

    fn event(year_end: NaiveDate, disposed: Decimal) -> AmitReductionEvent {
        AmitReductionEvent {
            amma_statement_id: 1,
            tax_year_end_date: year_end,
            per_unit: dec("1.30"),
            covered: dec("100"),
            disposed_by_year_end: disposed,
        }
    }

    /// A sale on or before the statement's year end joins the units the year
    /// end saw disposed of — the fact that decides which of
    /// `reduction_for_units`' two branches the allocation takes.
    #[test]
    fn a_sale_inside_the_statement_year_joins_its_disposed_units() {
        let events = [event(ymd(2026, 6, 30), dec("10"))];
        let with = events_with_disposal(&events, dec("40"), ymd(2026, 3, 2));
        assert_eq!(with[0].disposed_by_year_end, dec("50"));
        // The year end itself counts as inside the year, exactly as
        // `db_cost_base_reduction_events` counts a recorded sale dated on it.
        let with = events_with_disposal(&events, dec("40"), ymd(2026, 6, 30));
        assert_eq!(with[0].disposed_by_year_end, dec("50"));
    }

    /// A sale after the year end changes nothing about that year's statement:
    /// the units were still held when it ended.
    #[test]
    fn a_sale_after_the_statement_year_leaves_it_alone() {
        let events = [event(ymd(2026, 6, 30), dec("10"))];
        let with = events_with_disposal(&events, dec("40"), ymd(2026, 7, 1));
        assert_eq!(with[0].disposed_by_year_end, dec("10"));
    }
}
