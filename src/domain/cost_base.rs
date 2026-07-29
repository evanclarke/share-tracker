//! The adjusted-cost-base pipeline for a Buy/DRP parcel — the single
//! implementation shared by every report and operation that costs parcel
//! units (portfolio, unrealised/realised gains, open parcels, scrip-for-scrip
//! exchanges, demergers, holding-account transfers). The steps, in ATO order:
//!
//! 1. **Initial cost base** — price × quantity + brokerage + GST
//!    (`docs/ato/cost-base.md`: acquisition cost plus incidental costs).
//! 2. **AMIT cost-base net reduction**, floored at nil — CGT event E10: an
//!    AMMA statement's downward adjustment can only take the cost base to
//!    nil, never negative; the excess is a capital gain reported by the
//!    net-capital-gain report (`docs/ato/amit-cost-base-adjustments.md`).
//! 3. **Return-of-capital payments** (CGT event G1) received while the costed
//!    units were held — from acquisition up to `up_to` — reduce the cost base
//!    per as-acquired unit, again flooring at nil with the excess a capital
//!    gain in the net-capital-gain report
//!    (`docs/ato/cgt-non-assessable-payments.md`).
//! 4. **Split re-basing** — a share split/consolidation or non-assessable
//!    bonus issue scales unit counts but never the parcel's total cost base
//!    or acquisition date (TD 2000/10,
//!    `docs/ato/share-splits-and-consolidations.md`,
//!    `docs/ato/bonus-shares.md`). The pipeline therefore works in the
//!    parcel's *as-acquired* units: callers re-base a sale-date quantity back
//!    via `corporate_action::as_acquired_quantity` /
//!    `sold_in_acquired_units` before calling, and each payment re-bases
//!    itself (`corporate_action::RocEvent::per_unit_for`).
//! 5. **AUD conversion at the acquisition month**
//!    ([`CostBase::into_aud_with`]) — reports take the Australian-tax view,
//!    so the cost base converts at the ATO reference rate for the parcel's
//!    (possibly deemed) acquisition month. A rollover replacement parcel
//!    converts at its *deemed* acquisition month, carrying the original AUD
//!    cost base over. The acquisition-month rate deliberately covers the
//!    whole breakdown, including the AMIT/ROC reductions from later rate
//!    months — a documented simplification of the s 960-50(6) per-transaction
//!    translation timing (`docs/ato/forex-common-transactions.md`, QC 18322;
//!    see Known limitations in `docs/API.md`), material only for a non-AUD
//!    holding receiving non-AUD reductions.

use chrono::NaiveDate;
use rust_decimal::Decimal;
use serde::Serialize;

use crate::entities::corporate_action::{RocEvent, SplitEvent, per_unit_reduction};
use crate::infra::decimal::{Money, OptMoney};
use crate::infra::fx;

/// The facts of a Buy/DRP parcel as transacted, in its native currency and
/// as-acquired unit basis — the inputs to [`adjusted_cost_base`].
#[derive(Debug, Clone, Copy)]
pub struct Parcel<'a> {
    /// Units as transacted (the parcel's own as-acquired unit basis).
    pub quantity: Decimal,
    pub average_price: Decimal,
    pub brokerage: Decimal,
    pub gst_on_brokerage: Decimal,
    /// The parcel's trade currency. Return-of-capital payments in a different
    /// currency fail loudly — amounts in different currencies are never
    /// netted against each other.
    pub currency: &'a str,
    /// The actual trade date — drives split and return-of-capital
    /// applicability. A deemed acquisition date (rollover replacement
    /// parcels), where different, only drives the discount clock and the AUD
    /// translation month, never which events touch the parcel.
    pub trade_date: NaiveDate,
}

impl Parcel<'_> {
    /// Step 1 of the pipeline: the whole parcel's initial cost base in its own
    /// currency — the acquisition cost plus the incidental costs of acquiring
    /// it (`docs/ato/cgt-cost-base.md`). Every later step reduces this figure,
    /// so a caller that walks the reductions itself (the net-capital-gain
    /// report's E10 and G1 excess walks) starts from the same definition
    /// [`adjusted_cost_base`] does rather than re-adding the three terms.
    pub fn initial_cost(&self) -> Decimal {
        self.average_price * self.quantity + self.brokerage + self.gst_on_brokerage
    }
}

/// A Buy/DRP trade row as every cost-base report reads it — one `FromRow`
/// mapping (TEXT decimal columns via `infra::decimal`'s [`Money`]/[`OptMoney`])
/// instead of a per-report field-by-field copy. Select [`ParcelRow::COLUMNS`]
/// from `trades`.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct ParcelRow {
    pub id: i64,
    pub listing_id: i64,
    pub holding_account_id: i64,
    /// The actual trade date — drives split and return-of-capital
    /// applicability (see [`Parcel::trade_date`]).
    pub date: NaiveDate,
    #[sqlx(try_from = "Money")]
    pub quantity: Decimal,
    #[sqlx(try_from = "Money")]
    pub average_price: Decimal,
    #[sqlx(try_from = "Money")]
    pub brokerage: Decimal,
    #[sqlx(try_from = "Money")]
    pub gst_on_brokerage: Decimal,
    pub currency: String,
    #[sqlx(try_from = "Money")]
    pub fx_rate: Decimal,
    /// Deliberate transaction-date spot-rate override: when set it wins over
    /// the ATO monthly rate (see `infra::fx::FxOverride`).
    #[sqlx(try_from = "OptMoney")]
    pub spot_fx_rate: Option<Decimal>,
    /// Set on a rollover replacement parcel (scrip-for-scrip, demerger): the
    /// consumed parcel's acquisition date, carried so the combined holding
    /// period drives the discount clock and the AUD translation month.
    pub deemed_acquisition_date: Option<NaiveDate>,
}

impl ParcelRow {
    /// The column list matching the `FromRow` mapping, for single-table
    /// queries against `trades`.
    pub const COLUMNS: &'static str = "id, listing_id, holding_account_id, date, quantity, \
         average_price, brokerage, gst_on_brokerage, currency, fx_rate, spot_fx_rate, \
         deemed_acquisition_date";

    /// [`Self::COLUMNS`] qualified by a table alias and re-aliased back to the
    /// plain names, for a query that joins `trades` to a table carrying
    /// columns of the same name — `quantity`, `date` and `currency` all recur
    /// elsewhere in the schema, and the `FromRow` mapping reads by name, so an
    /// unqualified `quantity` would be ambiguous. Same set as `COLUMNS`, so a
    /// column added to one is added to both.
    pub fn columns_qualified(alias: &str) -> String {
        Self::COLUMNS
            .split(',')
            .map(|column| {
                let column = column.trim();
                format!("{alias}.{column} AS {column}")
            })
            .collect::<Vec<_>>()
            .join(", ")
    }

    /// The CGT acquisition date: the deemed date where set (rollover
    /// replacement parcels), else the trade date. Drives the 12-month
    /// discount clock and the AUD translation month of the cost base.
    pub fn acquired(&self) -> NaiveDate {
        self.deemed_acquisition_date.unwrap_or(self.date)
    }

    /// The parcel's per-record FX rate: its deliberate spot override when
    /// set, else its `fx_rate` fallback (see [`fx::FxOverride`]).
    pub fn fx_override(&self) -> fx::FxOverride {
        fx::FxOverride::from_trade(self.fx_rate, self.spot_fx_rate)
    }

    /// The row as [`adjusted_cost_base`]'s input.
    pub fn parcel(&self) -> Parcel<'_> {
        Parcel {
            quantity: self.quantity,
            average_price: self.average_price,
            brokerage: self.brokerage,
            gst_on_brokerage: self.gst_on_brokerage,
            currency: &self.currency,
            trade_date: self.date,
        }
    }
}

/// The cost-base breakdown of some or all of a parcel's units, produced by
/// [`adjusted_cost_base`]. Native currency until [`CostBase::into_aud_with`].
#[derive(Debug, Clone, Copy)]
pub struct CostBase {
    /// Whole-parcel initial cost base: price × quantity + brokerage + GST.
    pub initial_cost: Decimal,
    /// Cumulative AMIT cost-base reduction applied to the whole parcel (the
    /// full amount, even where CGT event E10 has floored the cost base).
    pub amit_reduction: Decimal,
    /// Return-of-capital payments received on the costed units (the full
    /// amount, even where CGT event G1 has floored the cost base).
    pub roc_reduction: Decimal,
    /// Adjusted cost base of the costed units: max(initial − AMIT, 0)
    /// pro-rated to the costed units, less the return-of-capital payments on
    /// them, floored at nil (CGT events E10 and G1 both floor at nil — any
    /// excess is a capital gain in the net-capital-gain report, never a
    /// negative cost base).
    pub adjusted: Decimal,
}

/// Runs steps 1–4 of the pipeline (see the module doc) for `units`
/// as-acquired units of `parcel`.
///
/// `units` is the portion being costed — a sale allocation re-based to
/// as-acquired units, the unsold remainder, or the moved/exchanged quantity.
/// `amit_reduction` is the parcel's cumulative AMIT reduction
/// (`amit_adjustment::db_cost_base_reductions*`). `roc_events` and `splits`
/// are the parcel's listing's events (`corporate_action::
/// db_return_of_capital_events` / `db_share_split_events`). `up_to` bounds
/// which return-of-capital payments the costed units were held for: the sale
/// date for realised units, the as-of/operation date for point-in-time views,
/// `None` for the live open-holdings view.
pub fn adjusted_cost_base(
    parcel: &Parcel<'_>,
    units: Decimal,
    amit_reduction: Decimal,
    roc_events: &[RocEvent],
    splits: &[SplitEvent],
    up_to: Option<NaiveDate>,
) -> Result<CostBase, sqlx::Error> {
    let initial_cost = parcel.initial_cost();
    let net_cost = (initial_cost - amit_reduction).max(Decimal::ZERO);
    let roc_per_unit = per_unit_reduction(
        roc_events,
        splits,
        parcel.currency,
        parcel.trade_date,
        up_to,
    )?;
    let roc_reduction = roc_per_unit * units;
    let adjusted = if parcel.quantity > Decimal::ZERO {
        (net_cost * units / parcel.quantity - roc_reduction).max(Decimal::ZERO)
    } else {
        Decimal::ZERO
    };
    Ok(CostBase {
        initial_cost,
        amit_reduction,
        roc_reduction,
        adjusted,
    })
}

/// One AMMA statement's whole-parcel AMIT cost-base reduction
/// (`amit_adjustments.quantity × amma_statements.cost_base_adjustment`), the
/// itemised input [`adjustment_detail`] needs in place of
/// [`adjusted_cost_base`]'s single pre-summed `amit_reduction` total. Produced
/// by `entities::amit_adjustment::db_cost_base_reduction_detail`.
#[derive(Debug, Clone, Copy)]
pub struct AmitReductionEvent {
    pub amma_statement_id: i64,
    pub tax_year_end_date: NaiveDate,
    /// The whole-parcel reduction from this one statement (native currency).
    pub amount: Decimal,
}

/// Which step of the pipeline (see the module doc) a [`CostBaseAdjustment`]
/// row belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum AdjustmentKind {
    AmitCostBase,
    ReturnOfCapital,
    /// A split/consolidation re-bases the unit count but reduces nothing
    /// (TD 2000/10) — carries `amount: 0`; included purely so a reader can
    /// reconcile a changed unit count against the source documents.
    SplitRebase,
}

/// One line of the reason the adjusted cost base is what it is — the
/// itemised detail behind [`CostBase::amit_reduction`] / `roc_reduction`,
/// which are cumulative totals. Reporting-only: never consumed by a
/// calculation, only rendered. Native currency, like [`CostBase`] itself,
/// until the caller converts (each row's `amount` divides by the same
/// acquisition-month rate as the parcel's other cost-base components, per
/// [`CostBase::into_aud_with`]'s step-5 simplification).
#[derive(Debug, Clone, Serialize)]
pub struct CostBaseAdjustment {
    pub kind: AdjustmentKind,
    /// The AMMA statement's year end / the return-of-capital payment date /
    /// the split's effective date.
    pub date: NaiveDate,
    /// Human-readable source of the row, e.g. "AMMA statement 12 (year ended
    /// 2025-06-30)", "Return of capital @ 0.15 AUD/unit", "2-for-1 split".
    pub reference: String,
    /// Native-currency reduction per as-acquired unit, where meaningful
    /// (`None` for a whole-parcel AMIT row or a split rebase).
    pub per_unit: Option<Decimal>,
    /// The reduction this row applies to the costed units, native currency —
    /// the *full* amount even where it (or a later row) pushes the running
    /// balance past nil; rows sum exactly to [`CostBase::amit_reduction`] (the
    /// [`AdjustmentKind::AmitCostBase`] rows) or `roc_reduction` (the
    /// [`AdjustmentKind::ReturnOfCapital`] rows).
    pub amount: Decimal,
    /// This row is the one that first drives the running balance to nil, or
    /// falls after that point (CGT event E10/G1) — the excess is a capital
    /// gain in the net-capital-gain report, not reflected in the cost base.
    pub capped: bool,
}

/// The itemised detail behind [`adjusted_cost_base`]'s AMIT and
/// return-of-capital reductions, plus informational split-rebase rows, for
/// `units` as-acquired units of `parcel`. Mirrors `adjusted_cost_base`'s
/// walk (steps 2–4 of the module doc) so the two can never disagree — a test
/// pins the rows summing to the same function's netted totals.
///
/// `amit_events` replaces `adjusted_cost_base`'s single pre-summed
/// `amit_reduction`: one [`AmitReductionEvent`] per AMMA statement affecting
/// this parcel (`entities::amit_adjustment::db_cost_base_reduction_detail`),
/// in `tax_year_end_date` order. `roc_events`, `splits` and `up_to` are the
/// same inputs as `adjusted_cost_base`.
pub fn adjustment_detail(
    parcel: &Parcel<'_>,
    units: Decimal,
    amit_events: &[AmitReductionEvent],
    roc_events: &[RocEvent],
    splits: &[SplitEvent],
    up_to: Option<NaiveDate>,
) -> Result<Vec<CostBaseAdjustment>, sqlx::Error> {
    let initial_cost = parcel.initial_cost();
    let mut rows = Vec::new();

    // AMIT reductions (CGT event E10): whole-parcel, in statement-year order.
    // The running balance floors at nil exactly like adjusted_cost_base's
    // single `(initial_cost - amit_reduction).max(0)` — flooring after each
    // step never loses value versus one accumulated subtraction, since every
    // step only ever subtracts a non-negative amount.
    let mut running = initial_cost;
    for e in amit_events {
        let before = running;
        running = (running - e.amount).max(Decimal::ZERO);
        let capped = before <= Decimal::ZERO || e.amount > before;
        let per_unit = (parcel.quantity != Decimal::ZERO).then(|| e.amount / parcel.quantity);
        rows.push(CostBaseAdjustment {
            kind: AdjustmentKind::AmitCostBase,
            date: e.tax_year_end_date,
            reference: format!(
                "AMMA statement {} (year ended {})",
                e.amma_statement_id, e.tax_year_end_date
            ),
            per_unit,
            amount: e.amount,
            capped,
        });
    }
    let net_cost = running;

    // Return-of-capital payments (CGT event G1), on the costed units only —
    // the pool they draw down is the costed units' share of the post-AMIT
    // cost base, mirroring adjusted_cost_base's
    // `net_cost * units / parcel.quantity` term.
    let costed_pool = if parcel.quantity > Decimal::ZERO {
        net_cost * units / parcel.quantity
    } else {
        Decimal::ZERO
    };
    let mut roc_running = costed_pool;
    for e in roc_events {
        // Applicability, the currency guard and the split re-basing are the
        // payment's own (`RocEvent::per_unit_for`) — the same call
        // `per_unit_reduction` sums, so the itemised rows can't describe a
        // different set of payments than the totals were computed from.
        let Some(per_unit) = e.per_unit_for(splits, parcel.currency, parcel.trade_date, up_to)?
        else {
            continue;
        };
        let amount = per_unit * units;
        let before = roc_running;
        roc_running = (roc_running - amount).max(Decimal::ZERO);
        let capped = before <= Decimal::ZERO || amount > before;
        rows.push(CostBaseAdjustment {
            kind: AdjustmentKind::ReturnOfCapital,
            date: e.date,
            reference: format!("Return of capital @ {per_unit} {}/unit", parcel.currency),
            per_unit: Some(per_unit),
            amount,
            capped,
        });
    }

    // Split/consolidation rebases: informational only (amount 0) — they
    // explain a changed unit count, never a cost-base reduction.
    for s in splits {
        if s.date < parcel.trade_date || up_to.is_some_and(|d| s.date > d) {
            continue;
        }
        rows.push(CostBaseAdjustment {
            kind: AdjustmentKind::SplitRebase,
            date: s.date,
            reference: format!("{}-for-{} split", s.new_units, s.old_units),
            per_unit: None,
            amount: Decimal::ZERO,
            capped: false,
        });
    }

    rows.sort_by(|a, b| {
        a.date
            .cmp(&b.date)
            .then(kind_rank(a.kind).cmp(&kind_rank(b.kind)))
    });
    Ok(rows)
}

/// Stable tie-break for same-date rows: AMIT, then return of capital, then
/// the split that made room for it — an arbitrary but deterministic order
/// (`adjustment_detail`'s doc comment).
fn kind_rank(kind: AdjustmentKind) -> u8 {
    match kind {
        AdjustmentKind::AmitCostBase => 0,
        AdjustmentKind::ReturnOfCapital => 1,
        AdjustmentKind::SplitRebase => 2,
    }
}

impl CostBase {
    /// Step 5 of the pipeline: convert every figure to AUD at the ATO
    /// reference rate for the parcel's acquisition month (`acquired` — the
    /// deemed acquisition date for a rollover replacement parcel, so the
    /// original AUD cost base carries over), arbitrated against the trade's
    /// per-record rate per `infra::fx::pick_rate` (a deliberate spot override
    /// wins; the `fx_rate` fallback applies only when no ATO rate exists).
    /// Takes pre-loaded rates ([`fx::FxRates`]) so
    /// a report loop's per-parcel conversion is a map lookup, not a DB
    /// round-trip. The rate is resolved once and applied to all components so
    /// the breakdown stays internally consistent — deliberately including the
    /// AMIT/ROC reductions from later rate months (the documented cost-base
    /// FX-timing simplification; see the module doc, step 5).
    pub fn into_aud_with(
        self,
        rates: &fx::FxRates,
        currency: &str,
        acquired: NaiveDate,
        fx_override: fx::FxOverride,
    ) -> Result<CostBase, fx::FxError> {
        let rate = rates.resolve_rate(currency, acquired, fx_override)?;
        Ok(self.at_rate(rate))
    }

    /// Apply a resolved foreign-per-AUD rate to every component (same
    /// pass-through-at-1 convention as `fx::apply_rate`) so the breakdown
    /// stays internally consistent.
    fn at_rate(self, rate: Decimal) -> CostBase {
        if rate == Decimal::ONE {
            return self;
        }
        CostBase {
            initial_cost: self.initial_cost / rate,
            amit_reduction: self.amit_reduction / rate,
            roc_reduction: self.roc_reduction / rate,
            adjusted: self.adjusted / rate,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::entities::rba_fx_rate;
    use crate::infra::db;

    fn date(y: i32, m: u32, d: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, d).unwrap()
    }

    fn parcel(qty: i64, price: i64) -> Parcel<'static> {
        Parcel {
            quantity: Decimal::from(qty),
            average_price: Decimal::from(price),
            brokerage: Decimal::ZERO,
            gst_on_brokerage: Decimal::ZERO,
            currency: "AUD",
            trade_date: date(2024, 1, 1),
        }
    }

    #[test]
    fn whole_parcel_initial_cost_includes_brokerage_and_gst() {
        let p = Parcel {
            brokerage: "9.95".parse().unwrap(),
            gst_on_brokerage: "0.995".parse().unwrap(),
            ..parcel(100, 10)
        };
        let cb = adjusted_cost_base(&p, Decimal::from(100), Decimal::ZERO, &[], &[], None).unwrap();
        assert_eq!(cb.initial_cost, "1010.945".parse::<Decimal>().unwrap());
        assert_eq!(cb.adjusted, "1010.945".parse::<Decimal>().unwrap());
    }

    #[test]
    fn partial_units_pro_rate_the_cost_base() {
        let cb = adjusted_cost_base(
            &parcel(100, 10),
            Decimal::from(40),
            Decimal::ZERO,
            &[],
            &[],
            None,
        )
        .unwrap();
        assert_eq!(cb.adjusted, Decimal::from(400));
    }

    #[test]
    fn amit_reduction_nets_off_and_e10_floors_at_nil() {
        // Reduction within the cost base nets off…
        let cb = adjusted_cost_base(
            &parcel(100, 10),
            Decimal::from(100),
            Decimal::from(5),
            &[],
            &[],
            None,
        )
        .unwrap();
        assert_eq!(cb.amit_reduction, Decimal::from(5));
        assert_eq!(cb.adjusted, Decimal::from(995));
        // …and a reduction exceeding it floors at nil (CGT event E10), while
        // the full reduction is still reported.
        let cb = adjusted_cost_base(
            &parcel(100, 10),
            Decimal::from(100),
            Decimal::from(1100),
            &[],
            &[],
            None,
        )
        .unwrap();
        assert_eq!(cb.amit_reduction, Decimal::from(1100));
        assert_eq!(cb.adjusted, Decimal::ZERO);
    }

    /// `adjustment_detail`'s itemised AMIT rows must sum to exactly the same
    /// `amit_reduction` `adjusted_cost_base` reports — including the E10
    /// floored case, where the second statement is the one that pushes the
    /// running balance to nil.
    #[test]
    fn itemised_amit_rows_sum_to_the_netted_reduction_including_the_floored_case() {
        let amit = |id: i64, y: i32, amount: i64| AmitReductionEvent {
            amma_statement_id: id,
            tax_year_end_date: date(y, 6, 30),
            amount: Decimal::from(amount),
        };
        // Two statements, neither alone exceeding the $1000 cost base.
        let events = [amit(1, 2024, 500), amit(2, 2025, 600)];
        let cb = adjusted_cost_base(
            &parcel(100, 10),
            Decimal::from(100),
            events.iter().map(|e| e.amount).sum(),
            &[],
            &[],
            None,
        )
        .unwrap();
        let rows = adjustment_detail(
            &parcel(100, 10),
            Decimal::from(100),
            &events,
            &[],
            &[],
            None,
        )
        .unwrap();
        let amit_rows: Vec<_> = rows
            .iter()
            .filter(|r| r.kind == AdjustmentKind::AmitCostBase)
            .collect();
        assert_eq!(amit_rows.len(), 2);
        let total: Decimal = amit_rows.iter().map(|r| r.amount).sum();
        assert_eq!(total, cb.amit_reduction);
        assert_eq!(cb.amit_reduction, Decimal::from(1100));
        assert_eq!(cb.adjusted, Decimal::ZERO); // 1000 - 1100, floored at nil (E10)
        // The first statement (500 of 1000) doesn't exhaust the cost base;
        // the second (600, on a remaining balance of 500) does.
        assert!(!amit_rows[0].capped);
        assert!(amit_rows[1].capped);
        assert_eq!(amit_rows[0].date, date(2024, 6, 30));
        assert_eq!(amit_rows[1].date, date(2025, 6, 30));
    }

    #[test]
    fn roc_payments_reduce_and_g1_floors_at_nil() {
        let roc = |amount: &str, d: NaiveDate| RocEvent {
            date: d,
            amount_per_unit: amount.parse().unwrap(),
            currency: "AUD".to_string(),
        };
        // 50c per unit on 100 units held: cost base 1000 → 950.
        let cb = adjusted_cost_base(
            &parcel(100, 10),
            Decimal::from(100),
            Decimal::ZERO,
            &[roc("0.50", date(2024, 3, 1))],
            &[],
            None,
        )
        .unwrap();
        assert_eq!(cb.roc_reduction, Decimal::from(50));
        assert_eq!(cb.adjusted, Decimal::from(950));
        // $11 per unit exceeds the $10 cost base: floored at nil (CGT event
        // G1), full payment still reported.
        let cb = adjusted_cost_base(
            &parcel(100, 10),
            Decimal::from(100),
            Decimal::ZERO,
            &[roc("11", date(2024, 3, 1))],
            &[],
            None,
        )
        .unwrap();
        assert_eq!(cb.roc_reduction, Decimal::from(1100));
        assert_eq!(cb.adjusted, Decimal::ZERO);
    }

    #[test]
    fn roc_payments_outside_the_holding_window_are_excluded() {
        let roc = RocEvent {
            date: date(2024, 7, 1),
            amount_per_unit: "0.50".parse().unwrap(),
            currency: "AUD".to_string(),
        };
        // Payment after the up_to (sale) date: the units were no longer held.
        let cb = adjusted_cost_base(
            &parcel(100, 10),
            Decimal::from(100),
            Decimal::ZERO,
            &[roc],
            &[],
            Some(date(2024, 6, 1)),
        )
        .unwrap();
        assert_eq!(cb.roc_reduction, Decimal::ZERO);
        assert_eq!(cb.adjusted, Decimal::from(1000));
    }

    #[test]
    fn roc_after_a_split_is_per_post_split_unit() {
        // 2-for-1 split, then 25c per post-split unit = 50c per as-acquired
        // unit: 100 as-acquired units lose 100 × 0.50 = 50.
        let split = SplitEvent {
            date: date(2024, 3, 1),
            new_units: Decimal::from(2),
            old_units: Decimal::ONE,
        };
        let roc = RocEvent {
            date: date(2024, 4, 1),
            amount_per_unit: "0.25".parse().unwrap(),
            currency: "AUD".to_string(),
        };
        let cb = adjusted_cost_base(
            &parcel(100, 10),
            Decimal::from(100),
            Decimal::ZERO,
            &[roc],
            &[split],
            None,
        )
        .unwrap();
        assert_eq!(cb.roc_reduction, Decimal::from(50));
        assert_eq!(cb.adjusted, Decimal::from(950));
    }

    /// `adjustment_detail`'s itemised ROC rows sum to exactly `roc_reduction`
    /// (including a floored G1 case), and a split within the holding window
    /// shows up as an informational nil-amount row in date order alongside
    /// them.
    #[test]
    fn itemised_roc_and_split_rows_sum_to_the_netted_reduction() {
        let split = SplitEvent {
            date: date(2024, 3, 1),
            new_units: Decimal::from(2),
            old_units: Decimal::ONE,
        };
        // Two ROC payments after the split: the second (per post-split unit)
        // exceeds what's left of the parcel's $1000 cost base and floors it.
        let roc = |amount: &str, d: NaiveDate| RocEvent {
            date: d,
            amount_per_unit: amount.parse().unwrap(),
            currency: "AUD".to_string(),
        };
        let events = [roc("0.25", date(2024, 4, 1)), roc("5", date(2024, 5, 1))];
        let cb = adjusted_cost_base(
            &parcel(100, 10),
            Decimal::from(100),
            Decimal::ZERO,
            &events,
            std::slice::from_ref(&split),
            None,
        )
        .unwrap();
        let rows = adjustment_detail(
            &parcel(100, 10),
            Decimal::from(100),
            &[],
            &events,
            &[split],
            None,
        )
        .unwrap();

        let roc_rows: Vec<_> = rows
            .iter()
            .filter(|r| r.kind == AdjustmentKind::ReturnOfCapital)
            .collect();
        assert_eq!(roc_rows.len(), 2);
        let total: Decimal = roc_rows.iter().map(|r| r.amount).sum();
        assert_eq!(total, cb.roc_reduction);
        assert_eq!(cb.adjusted, Decimal::ZERO); // G1-floored
        assert!(!roc_rows[0].capped);
        assert!(roc_rows[1].capped);

        let split_rows: Vec<_> = rows
            .iter()
            .filter(|r| r.kind == AdjustmentKind::SplitRebase)
            .collect();
        assert_eq!(split_rows.len(), 1);
        assert_eq!(split_rows[0].amount, Decimal::ZERO);
        assert_eq!(split_rows[0].date, date(2024, 3, 1));

        // Rows come back date-ordered: split, then the two ROC payments.
        assert_eq!(rows[0].kind, AdjustmentKind::SplitRebase);
        assert_eq!(rows[1].date, date(2024, 4, 1));
        assert_eq!(rows[2].date, date(2024, 5, 1));
    }

    #[test]
    fn roc_in_a_different_currency_fails_loudly() {
        let roc = RocEvent {
            date: date(2024, 3, 1),
            amount_per_unit: "0.50".parse().unwrap(),
            currency: "USD".to_string(),
        };
        let err = adjusted_cost_base(
            &parcel(100, 10),
            Decimal::from(100),
            Decimal::ZERO,
            &[roc],
            &[],
            None,
        )
        .unwrap_err();
        assert!(err.to_string().contains("currency"));
    }

    #[test]
    fn zero_quantity_parcel_costs_nil() {
        let cb = adjusted_cost_base(&parcel(0, 10), Decimal::ZERO, Decimal::ZERO, &[], &[], None)
            .unwrap();
        assert_eq!(cb.adjusted, Decimal::ZERO);
    }

    #[test]
    fn into_aud_with_passes_aud_through_unchanged() {
        let cb = adjusted_cost_base(
            &parcel(100, 10),
            Decimal::from(100),
            Decimal::ZERO,
            &[],
            &[],
            None,
        )
        .unwrap();
        let aud = cb
            .into_aud_with(
                &fx::FxRates::default(),
                "AUD",
                date(2024, 1, 1),
                fx::FxOverride::None,
            )
            .unwrap();
        assert_eq!(aud.adjusted, Decimal::from(1000));
        assert_eq!(aud.initial_cost, Decimal::from(1000));
    }

    /// The rates load from the `rba_fx_rates` table exactly as the async
    /// lookup path read them (same import, same month key).
    #[tokio::test]
    async fn into_aud_with_converts_every_component_at_the_acquisition_month_rate() {
        let pool = db::init(":memory:").await.unwrap();
        // A$1 = 0.50 USD in the acquisition month.
        rba_fx_rate::db_import_rate(&pool, "USD", "2024-01", "0.50".parse().unwrap())
            .await
            .unwrap();
        let rates = fx::FxRates::load(&pool).await.unwrap();
        let p = Parcel {
            currency: "USD",
            ..parcel(100, 10)
        };
        let roc = RocEvent {
            date: date(2024, 3, 1),
            amount_per_unit: "0.50".parse().unwrap(),
            currency: "USD".to_string(),
        };
        let cb = adjusted_cost_base(&p, Decimal::from(100), Decimal::from(5), &[roc], &[], None)
            .unwrap();
        let aud = cb
            .into_aud_with(&rates, "USD", date(2024, 1, 15), fx::FxOverride::None)
            .unwrap();
        // Every component is USD / 0.50: 1000 → 2000, 5 → 10, 50 → 100,
        // (1000 − 5 − 50) → 1890.
        assert_eq!(aud.initial_cost, Decimal::from(2000));
        assert_eq!(aud.amit_reduction, Decimal::from(10));
        assert_eq!(aud.roc_reduction, Decimal::from(100));
        assert_eq!(aud.adjusted, Decimal::from(1890));
        // A month with no rate falls back to the override; none means a loud
        // failure, never a silently unconverted figure.
        assert!(matches!(
            cb.into_aud_with(&rates, "USD", date(2025, 1, 15), fx::FxOverride::None),
            Err(fx::FxError::MissingRate { .. })
        ));
    }
}
