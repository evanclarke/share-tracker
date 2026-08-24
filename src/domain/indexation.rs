//! The indexation method's arithmetic — the frozen ATO quarterly CPI series
//! and the indexation factor derived from it, stated once so no caller can
//! index a cost differently from another.
//!
//! For a CGT asset whose costs were incurred **by 21 September 1999** an
//! individual may index those costs for inflation *instead of* applying the
//! 50% CGT discount (`docs/ato/indexing-the-cost-base.md`, QC 66024). The
//! rules this module encodes:
//!
//! 1. The cost must have been incurred on or before **21 September 1999**
//!    ([`LAST_INDEXABLE_COST_DATE`]).
//! 2. Indexation is **frozen at the September 1999 quarter**, whose CPI
//!    (68.7) is the numerator of every factor.
//! 3. The **factor** is that numerator ÷ the CPI for the quarter in which the
//!    cost was incurred, *limited to 3 decimal places, rounding the fourth
//!    decimal up from 5* (1.4125 → 1.413) — [`indexation_factor`].
//! 4. The indexed cost base is the indexed costs plus any **non-indexable**
//!    costs; the third element of the cost base (costs of owning the asset)
//!    can never be indexed — not recordable here in any case (see the *Cost
//!    base elements* Known limitation).
//! 5. Indexation **cannot be used on a capital loss**, and can never be
//!    combined with the 50% discount.
//!
//! **What this module is not.** Nothing here changes a reported tax figure.
//! The net capital gain, the tax summary, the Annual Tax Report and every CSV
//! export apply the 50% discount throughout, exactly as they did before this
//! module existed. The indexed figure is advisory: it answers "which method
//! would have given the better result on this disposal", reported by
//! `reports::indexation_cross_check` and carried beside the discount figure
//! on the realised-gains parcel rows. Modelling the *election* — a per-parcel
//! choice, taken after capital losses are applied and interacting with the
//! carried-forward loss chain — is a deliberate scope cut (`docs/API.md`,
//! Known limitations).

use chrono::{Datelike, NaiveDate};
use rust_decimal::{Decimal, RoundingStrategy};
use sqlx::Row;

use crate::domain::cost_base::CostBase;

/// The last day a cost can have been incurred and still be indexable
/// (`docs/ato/indexing-the-cost-base.md`: "must have incurred the costs by
/// 21 September 1999"). A cost incurred on this day is eligible; the next day
/// is not.
pub const LAST_INDEXABLE_COST_DATE: NaiveDate = match NaiveDate::from_ymd_opt(1999, 9, 21) {
    Some(d) => d,
    None => unreachable!(),
};

/// The quarter indexation is frozen at: every factor's numerator is this
/// quarter's CPI, and no later quarter is stored (see
/// `migrations/0046_cpi_quarters.sql`).
pub const FROZEN_QUARTER_END: NaiveDate = match NaiveDate::from_ymd_opt(1999, 9, 30) {
    Some(d) => d,
    None => unreachable!(),
};

/// The indexation of one cost: which quarter's CPI was used, that CPI, and
/// the resulting factor. Returned together so a report can *show its working*
/// rather than presenting a factor the reader cannot check against the ATO's
/// own published table.
/// Deliberately not `Serialize`: a report that shows its working spells these
/// three out as its own named columns (`cpi_quarter_end`, `cpi`,
/// `indexation_factor`), so this never reaches the wire as a nested object
/// whose fields no display-kind map knows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Indexation {
    /// End date of the quarter the cost was incurred in — the row of the
    /// ATO's table (`docs/ato/consumer-price-index.md`) this reads.
    pub quarter_end: NaiveDate,
    /// That quarter's CPI, verbatim as published.
    pub cpi: Decimal,
    /// 68.7 ÷ `cpi`, limited to 3 decimal places (fourth decimal rounded up
    /// from 5).
    pub factor: Decimal,
}

/// The frozen quarterly CPI series, pre-loaded so a report loop's per-parcel
/// lookup is a map read rather than a DB round-trip — the same shape
/// `infra::fx::FxRates` takes for the same reason. 57 rows.
#[derive(Debug, Clone, Default)]
pub struct CpiQuarters {
    quarters: std::collections::HashMap<NaiveDate, Decimal>,
}

impl CpiQuarters {
    /// Read the whole table on the caller's connection, so it joins the
    /// report's own single-snapshot read transaction.
    pub async fn load(conn: &mut sqlx::SqliteConnection) -> Result<Self, sqlx::Error> {
        let rows = sqlx::query("SELECT quarter_end, cpi FROM cpi_quarters")
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

    /// The CPI for the quarter `date` falls in, or `None` when the quarter is
    /// outside the seeded (indexable) range. Test-only: production callers
    /// want the whole [`Indexation`], never a bare CPI.
    #[cfg(test)]
    pub fn cpi_for(&self, date: NaiveDate) -> Option<Decimal> {
        self.quarters.get(&quarter_end_for(date)).copied()
    }

    /// The indexation of a cost incurred on `cost_incurred`, or `None` when
    /// the cost is not indexable: incurred after 21 September 1999, or in a
    /// quarter the table does not carry (which the seeded range makes
    /// equivalent to "before CGT existed"). Failing closed is deliberate —
    /// an absent CPI must produce no comparison at all, never a factor of 1.
    pub fn indexation_for(&self, cost_incurred: NaiveDate) -> Option<Indexation> {
        if cost_incurred > LAST_INDEXABLE_COST_DATE {
            return None;
        }
        let frozen = self.quarters.get(&FROZEN_QUARTER_END).copied()?;
        let quarter_end = quarter_end_for(cost_incurred);
        let cpi = self.quarters.get(&quarter_end).copied()?;
        Some(Indexation {
            quarter_end,
            cpi,
            factor: indexation_factor(frozen, cpi)?,
        })
    }
}

/// End date of the calendar quarter `date` falls in — the key of the ATO's
/// CPI table. 20 September 1985 is in the quarter ending 30 September 1985.
pub fn quarter_end_for(date: NaiveDate) -> NaiveDate {
    let (month, day) = match date.month() {
        1..=3 => (3, 31),
        4..=6 => (6, 30),
        7..=9 => (9, 30),
        _ => (12, 31),
    };
    NaiveDate::from_ymd_opt(date.year(), month, day).expect("a quarter end is a real date")
}

/// Step 3 of the method: `frozen_cpi ÷ cpi`, **limited to 3 decimal places,
/// rounding the fourth decimal up from 5** — the ATO's own words, and its own
/// example, 1.4125 → 1.413. `None` for a non-positive CPI (unreachable
/// against the seeded table; a divide-by-zero must never surface as a
/// panicked report).
pub fn indexation_factor(frozen_cpi: Decimal, cpi: Decimal) -> Option<Decimal> {
    if cpi <= Decimal::ZERO {
        return None;
    }
    Some((frozen_cpi / cpi).round_dp_with_strategy(3, RoundingStrategy::MidpointAwayFromZero))
}

/// The indexed cost base of the units `cost` was computed for.
///
/// `cost` is the shared pipeline's [`CostBase`] for those units, already
/// converted to AUD. Its components divide cleanly into the two halves step 5
/// of the method asks for:
///
/// - **Indexable**: [`CostBase::costed_initial_cost`] — price × quantity +
///   brokerage + GST, pro-rated to the costed units. Every part of it was
///   incurred at the parcel's own trade date, which is what `factor` is
///   derived from, so it indexes as one figure.
/// - **Not indexed**: the AMIT (CGT event E10) and return-of-capital (G1)
///   *reductions*. They are not costs at all — they come **off** the cost
///   base — and they arise from payments made years after the acquisition, so
///   they are applied to the indexed figure at face value. This is the
///   conservative direction: indexing a reduction would shrink the indexed
///   cost base further and overstate indexation's advantage.
///
/// Floored at nil for the same reason the pipeline's own `adjusted` is: E10
/// and G1 can take a cost base to nil, never below it.
pub fn indexed_cost_base(cost: &CostBase, factor: Decimal) -> Decimal {
    (cost.costed_initial_cost * factor - cost.amit_reduction - cost.roc_reduction)
        .max(Decimal::ZERO)
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

    /// The rounding rule is the ATO's own, stated with its own example:
    /// "limited to 3 decimal places (round the fourth decimal up from 5, e.g.
    /// 1.4125 → 1.413)".
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
        // A zero or negative CPI yields no factor rather than a panic.
        assert_eq!(indexation_factor(dec("68.7"), Decimal::ZERO), None);
    }

    /// A calendar date lands in the quarter whose end date the CPI table
    /// carries — including the boundary days at each end of a quarter.
    #[test]
    fn a_date_maps_to_its_own_quarter_end() {
        assert_eq!(quarter_end_for(d(1985, 9, 20)), d(1985, 9, 30));
        assert_eq!(quarter_end_for(d(1985, 7, 1)), d(1985, 9, 30));
        assert_eq!(quarter_end_for(d(1985, 9, 30)), d(1985, 9, 30));
        assert_eq!(quarter_end_for(d(1985, 10, 1)), d(1985, 12, 31));
        assert_eq!(quarter_end_for(d(1999, 1, 1)), d(1999, 3, 31));
        assert_eq!(quarter_end_for(d(1999, 6, 30)), d(1999, 6, 30));
    }

    /// The seeded table is the ATO's Appendix 2 verbatim: 57 quarters, the
    /// September 1985 quarter through the September 1999 freeze, and nothing
    /// after it. The two figures the whole method turns on are pinned here.
    #[tokio::test]
    async fn the_seeded_series_is_the_ato_appendix() {
        let pool = crate::test_support::test_pool().await;
        let mut conn = pool.acquire().await.unwrap();
        let cpi = CpiQuarters::load(&mut conn).await.unwrap();
        assert_eq!(cpi.quarters.len(), 57);
        assert_eq!(cpi.cpi_for(d(1985, 9, 20)), Some(dec("39.7")));
        assert_eq!(cpi.cpi_for(d(1999, 9, 21)), Some(dec("68.7")));
        // Nothing after the freeze, and nothing before CGT.
        assert_eq!(cpi.cpi_for(d(1999, 12, 1)), None);
        assert_eq!(cpi.cpi_for(d(1985, 6, 30)), None);
    }

    /// The finding's own parcel (SCENARIOS AA-a): a cost incurred
    /// 20 September 1985 indexes at 68.7 ÷ 39.7 = **1.730**, not the 1.731
    /// the finding's write-up quoted — 1.730478… rounds *down* at the third
    /// decimal, because the fourth is a 4. The write-up's figure is the one
    /// the superseded 1989-90-base series gives (123.4 ÷ 71.3 = 1.731), which
    /// the ATO says can no longer be used for tax purposes.
    #[tokio::test]
    async fn the_earliest_enterable_acquisition_indexes_at_1_730() {
        let pool = crate::test_support::test_pool().await;
        let mut conn = pool.acquire().await.unwrap();
        let cpi = CpiQuarters::load(&mut conn).await.unwrap();
        let ix = cpi.indexation_for(d(1985, 9, 20)).unwrap();
        assert_eq!(ix.quarter_end, d(1985, 9, 30));
        assert_eq!(ix.cpi, dec("39.7"));
        assert_eq!(ix.factor, dec("1.730"));
        // A$10,000 of cost indexes to A$17,300, so a A$20,000 disposal shows
        // a A$2,700 gain under indexation against A$5,000 under the discount
        // — and the crossover is 2.460 × cost, not "almost always" the
        // discount.
        assert_eq!(dec("10000") * ix.factor, dec("17300.000"));
    }

    /// The ATO's own worked example (Val, `docs/ato/indexing-the-cost-base.md`)
    /// reproduces exactly against the seeded table: the June 1991 quarter
    /// gives 1.164 and the September 1991 quarter 1.159. Two independently
    /// published factors agreeing is what says the *table* and the *rounding
    /// rule* are both right.
    #[tokio::test]
    async fn the_ato_worked_examples_factors_reproduce() {
        let pool = crate::test_support::test_pool().await;
        let mut conn = pool.acquire().await.unwrap();
        let cpi = CpiQuarters::load(&mut conn).await.unwrap();
        assert_eq!(
            cpi.indexation_for(d(1991, 6, 24)).unwrap().factor,
            dec("1.164")
        );
        assert_eq!(
            cpi.indexation_for(d(1991, 7, 20)).unwrap().factor,
            dec("1.159")
        );
        assert_eq!(
            cpi.indexation_for(d(1991, 8, 5)).unwrap().factor,
            dec("1.159")
        );
    }

    /// The 21 September 1999 boundary is exact, and the frozen quarter itself
    /// indexes at 1.000 — a cost incurred inside it is already at the frozen
    /// CPI, so indexation can add nothing.
    #[tokio::test]
    async fn the_twenty_first_of_september_1999_is_the_last_indexable_day() {
        let pool = crate::test_support::test_pool().await;
        let mut conn = pool.acquire().await.unwrap();
        let cpi = CpiQuarters::load(&mut conn).await.unwrap();
        assert_eq!(
            cpi.indexation_for(d(1999, 9, 21)).map(|i| i.factor),
            Some(Decimal::ONE)
        );
        assert_eq!(cpi.indexation_for(d(1999, 9, 22)), None);
    }

    /// The reductions come off the indexed figure at face value, and the
    /// result floors at nil exactly as the pipeline's own `adjusted` does.
    #[test]
    fn reductions_are_not_indexed_and_the_result_floors_at_nil() {
        let cost = CostBase {
            initial_cost: dec("10000"),
            costed_initial_cost: dec("10000"),
            amit_reduction: dec("300"),
            roc_reduction: dec("200"),
            adjusted: dec("9500"),
        };
        // 10,000 × 1.730 = 17,300, less the 500 of reductions at face value.
        assert_eq!(indexed_cost_base(&cost, dec("1.730")), dec("16800.000"));
        let wiped = CostBase {
            amit_reduction: dec("40000"),
            ..cost
        };
        assert_eq!(indexed_cost_base(&wiped, dec("1.730")), Decimal::ZERO);
    }
}
