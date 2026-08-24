//! The adjusted-cost-base pipeline for a Buy/DRP parcel — the single
//! implementation shared by every report and operation that costs parcel
//! units (portfolio, unrealised/realised gains, open parcels, scrip-for-scrip
//! exchanges, demergers, holding-account transfers). The steps, in ATO order:
//!
//! 1. **Initial cost base** — price × quantity + brokerage + GST
//!    (`docs/ato/cost-base.md`: acquisition cost plus incidental costs),
//!    pro-rated to the units being costed.
//! 2. **AMIT cost-base net reduction**, floored at nil — CGT event E10: an
//!    AMMA statement's downward adjustment can only take the cost base to
//!    nil, never negative; the excess is a capital gain reported by the
//!    net-capital-gain report (`docs/ato/amit-cost-base-adjustments.md`).
//!    Applied **per unit to the units the statement covers**
//!    ([`AmitReductionEvent::per_unit_for`]), not pooled across the parcel.
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
use crate::infra::decimal::{Money, OptMoney, mul_div};
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
    /// Set — to [`Self::trade_date`] — when a rollover created this parcel: the
    /// operation folded every return-of-capital payment up to **and including**
    /// its own date into the cost base this parcel carries, so those payments
    /// must not reduce it a second time here
    /// (`RocEvent::per_unit_for`, SCENARIOS N-06). `None` on an ordinary parcel,
    /// and on an inherited one, whose cost base is stated rather than carried.
    pub rolled_over_on: Option<NaiveDate>,
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

/// One term of an initial cost base, labelled as the write path that supplies
/// it names its own request field — so [`checked_cost_base`]'s refusal can
/// quote the arithmetic back in the user's own vocabulary rather than in the
/// `trades` columns it happens to be stored in (an inheritance states a
/// `cost_base` and an `lpr_expenditure`; they land on the `brokerage` column
/// with a zero price).
#[derive(Debug, Clone, Copy)]
pub enum Term<'a> {
    /// A per-unit figure times a unit count — the acquisition cost.
    Product {
        price: (&'a str, Decimal),
        units: (&'a str, Decimal),
    },
    /// A flat amount added to it — an incidental cost of acquiring.
    Amount(&'a str, Decimal),
}

impl Term<'_> {
    /// How the term reads inside [`UnrepresentableCost::expression`].
    fn written(&self) -> String {
        match self {
            Term::Product { price, units } => {
                format!("{} {} × {} {}", price.0, price.1, units.0, units.1)
            }
            Term::Amount(label, amount) => format!("{label} {amount}"),
        }
    }
}

/// The write-time refusal of a cost base that cannot be represented: the terms
/// [`checked_cost_base`] was given, written out, so the `422` can name the
/// product it could not compute (SCENARIOS W-e).
///
/// Carries the arithmetic rather than a code: every path that can reach it
/// names its figures differently, and the one thing they share is that the
/// product itself is the answer, so there is no lesser figure to record.
#[derive(thiserror::Error, Debug, Clone, PartialEq, Eq)]
#[error("the cost base {expression} exceeds the representable range")]
pub struct UnrepresentableCost {
    /// The terms as written, e.g.
    /// `average_price 1000 × quantity 2 + brokerage 19.95 + gst_on_brokerage 0`.
    pub expression: String,
}

impl UnrepresentableCost {
    /// The user-facing `422` body (shown by the web UI): the arithmetic that
    /// overflowed and the limit it overflowed — `Decimal::MAX` itself, read
    /// off the type rather than transcribed, so the bound can never drift
    /// from the one the arithmetic actually has. Lives here so the six
    /// refusals of the same fact cannot word it differently.
    pub fn message(&self) -> String {
        beyond_the_range(
            &self.expression,
            "the parcel's cost base itself — a sale's proceeds",
            "the entry",
        )
    }
}

/// The one wording for *this arithmetic overflows what a decimal can hold*,
/// shared by the cost-base ([`UnrepresentableCost`]) and unit-count
/// ([`UnrepresentableQuantity`]) refusals so the paths that answer it cannot
/// word it differently. `total_is` completes the sentence explaining why no
/// lesser figure can stand in, and `correct` names what the user has to fix.
///
/// `Decimal::MAX` is read off the type rather than transcribed, so the bound
/// quoted can never drift from the one the arithmetic actually has.
fn beyond_the_range(expression: &str, total_is: &str, correct: &str) -> String {
    format!(
        "these figures multiply out beyond what can be recorded: {expression} exceeds {}, the \
         largest amount that can be stored. That total is {total_is} — so no smaller figure could \
         stand in for it: correct {correct}, checking for a mistyped run of zeros",
        Decimal::MAX
    )
}

/// The write-time refusal of a **unit count** that cannot be represented: the
/// holding and the ratio applied to it, written out, so the `422` can name the
/// arithmetic it could not compute.
///
/// [`UnrepresentableCost`]'s sibling, and the same shape of fact: a
/// ratio-driven action (a scrip-for-scrip exchange, a demerger, a share split,
/// a bonus issue) turns `held` units into `held × new / old`, and where that
/// result is past `Decimal`'s range there is no lesser quantity to record
/// instead. The two share [`beyond_the_range`], so the wording lives once.
#[derive(thiserror::Error, Debug, Clone, PartialEq, Eq)]
#[error("the quantity {expression} exceeds the representable range")]
pub struct UnrepresentableQuantity {
    /// The arithmetic as written, e.g.
    /// `units held 1000000000000000000000000000 × scrip_new_units 1000 / scrip_old_units 1`.
    pub expression: String,
}

impl UnrepresentableQuantity {
    /// The user-facing `422` body (shown by the web UI).
    pub fn message(&self) -> String {
        beyond_the_range(
            &self.expression,
            "the number of units the holding becomes",
            "the ratio, or the holding it applies to",
        )
    }
}

/// `units × new / old` — a holding re-based by a ratio — or the refusal when
/// the **result** cannot be represented.
///
/// This is [`mul_div`]'s arithmetic in checked form, in the same order, so
/// everything `mul_div` can answer is accepted here unchanged: multiply first
/// (exact), and where that product alone overflows, divide first and multiply
/// after — the headroom `mul_div` exists for. Only when *both* orders overflow
/// is the result itself out of range, which is exactly where `mul_div` panics
/// and this returns the refusal instead.
///
/// Unlike a cost base, a re-based quantity has no lesser answer either: it is
/// the number of units the holding becomes, so a write that produces one has
/// to be refused rather than approximated (the section this closes;
/// `mul_div`'s own sweep deliberately left the decision open, since what is
/// unrepresentable here is the result and not the working).
///
/// A nil `old` cannot arise — every ratio denominator is validated positive
/// where the action is written — but it would report the same refusal rather
/// than divide by zero.
pub fn checked_rebased_quantity(
    units: (&str, Decimal),
    new: (&str, Decimal),
    old: (&str, Decimal),
) -> Result<Decimal, UnrepresentableQuantity> {
    let scaled = match units.1.checked_mul(new.1) {
        Some(product) => product.checked_div(old.1),
        None => units
            .1
            .checked_div(old.1)
            .and_then(|divided| divided.checked_mul(new.1)),
    };
    scaled.ok_or_else(|| UnrepresentableQuantity {
        expression: format!(
            "{} {} × {} {} / {} {}",
            units.0, units.1, new.0, new.1, old.0, old.1
        ),
    })
}

/// The cost base of `terms`, or the refusal when it cannot be represented.
///
/// The bound is `rust_decimal`'s own: [`Decimal::checked_mul`] and
/// [`Decimal::checked_add`] answer `None` at exactly the point the plain
/// operators panic, so what is refused is what could not have been stored —
/// not a policy ceiling anyone has to defend (SCENARIOS W-e, re-taking the
/// decision W-b recorded against a magnitude ceiling).
///
/// This is [`Parcel::initial_cost`]'s formula in checked form, and it is the
/// write-time half of it: with every parcel-creating path calling this first,
/// no row `initial_cost` later reads can overflow it. The read side stays
/// unchecked deliberately — a row that predates the guard must still fail
/// loudly (a logged `500`, via the panic layer) rather than quietly produce a
/// different number.
///
/// A Sell nets its costs out of the product rather than adding them, so
/// bounding it by the same sum is very slightly tighter than that trade needs.
/// Deliberate: the reports *sum* proceeds across sales, so a figure within a
/// brokerage of the limit is unusable either way, and one rule over both
/// directions is worth more than the last brokerage of range.
pub fn checked_cost_base(terms: &[Term<'_>]) -> Result<Decimal, UnrepresentableCost> {
    let overflowed = || UnrepresentableCost {
        expression: terms
            .iter()
            .map(Term::written)
            .collect::<Vec<_>>()
            .join(" + "),
    };
    let mut total = Decimal::ZERO;
    for term in terms {
        let value = match term {
            Term::Product { price, units } => {
                price.1.checked_mul(units.1).ok_or_else(overflowed)?
            }
            Term::Amount(_, amount) => *amount,
        };
        total = total.checked_add(value).ok_or_else(overflowed)?;
    }
    Ok(total)
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
    /// The operation that created this parcel, where one did
    /// (`domain::rollover`'s three): a scrip-for-scrip exchange, a demerger, or
    /// a holding-account transfer. Only one is ever set. They are read together
    /// as [`Self::rolled_over_on`] — an *inherited* parcel also carries a deemed
    /// acquisition date, so that field cannot stand in for this one.
    pub scrip_action_id: Option<i64>,
    pub demerger_action_id: Option<i64>,
    pub transfer_id: Option<i64>,
}

impl ParcelRow {
    /// The column list matching the `FromRow` mapping, for single-table
    /// queries against `trades`.
    pub const COLUMNS: &'static str = "id, listing_id, holding_account_id, date, quantity, \
         average_price, brokerage, gst_on_brokerage, currency, fx_rate, spot_fx_rate, \
         deemed_acquisition_date, scrip_action_id, demerger_action_id, transfer_id";

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

    /// The date a rollover carried this parcel's cost base forward on, when it
    /// is a replacement parcel — its own trade date, which is also the operation
    /// date. `None` for an ordinary (or inherited) parcel. See
    /// [`Parcel::rolled_over_on`].
    pub fn rolled_over_on(&self) -> Option<NaiveDate> {
        (self.scrip_action_id.is_some()
            || self.demerger_action_id.is_some()
            || self.transfer_id.is_some())
        .then_some(self.date)
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
            rolled_over_on: self.rolled_over_on(),
        }
    }
}

/// What became of the units being costed — the bound on which later events
/// reach them, and (for an AMIT statement covering only part of a parcel)
/// which side of the statement's year end they sit on.
///
/// The two cases are not interchangeable: a point-in-time view of units still
/// held (`AsAt(Some(date))`) bounds the events that have *happened yet*, while
/// a disposal (`DisposedOn(date)`) additionally says the units were gone
/// afterwards. Both answer [`Self::up_to`] the same way, which is why the two
/// used to be one `Option<NaiveDate>` — and why a partial AMIT adjustment
/// could not tell them apart.
#[derive(Debug, Clone, Copy)]
pub enum Held {
    /// Still held as at the given date (`None` = the live view of every
    /// recorded fact).
    AsAt(Option<NaiveDate>),
    /// Disposed of on this date (a sale allocation, or a parcel-substituting
    /// operation's closing Sell).
    DisposedOn(NaiveDate),
}

impl Held {
    /// The date bounding which events have reached these units: the as-of date
    /// for held units, the disposal date for sold ones (`None` = unbounded).
    pub fn up_to(self) -> Option<NaiveDate> {
        match self {
            Held::AsAt(date) => date,
            Held::DisposedOn(date) => Some(date),
        }
    }

    /// The disposal date, where these units were disposed of at all.
    pub fn disposed_on(self) -> Option<NaiveDate> {
        match self {
            Held::AsAt(_) => None,
            Held::DisposedOn(date) => Some(date),
        }
    }
}

/// One AMMA statement's AMIT cost-base reduction against one parcel, as
/// `entities::amit_adjustment::db_cost_base_reduction_events` reads it: the
/// statement's per-unit figure, the units of the parcel the adjustment row
/// covers, and how much of the parcel had already been disposed of by the
/// statement's year end — everything [`Self::per_unit_for`] needs to answer
/// *which* units the reduction reaches.
///
/// All three quantities are in the parcel's **as-acquired** unit basis; the
/// per-unit figure is likewise re-based into it, so a split between
/// acquisition and the statement's year end is already accounted for
/// (TD 2000/10 — see `entities::amit_adjustment::reduction_for`).
#[derive(Debug, Clone, Copy)]
pub struct AmitReductionEvent {
    pub amma_statement_id: i64,
    pub tax_year_end_date: NaiveDate,
    /// The statement's cost-base reduction per as-acquired unit.
    pub per_unit: Decimal,
    /// Units of the parcel this adjustment row covers.
    pub covered: Decimal,
    /// Units of the parcel disposed of on or before `tax_year_end_date`.
    pub disposed_by_year_end: Decimal,
}

impl AmitReductionEvent {
    /// The whole-parcel-equivalent reduction this row states: the covered
    /// units times the per-unit figure. This is the amount the row's own
    /// arithmetic produces, and the total [`Self::per_unit_for`] distributes —
    /// no more and no less, whichever units it reaches. (Only the tests pin
    /// that identity directly — every production caller goes through
    /// [`Self::per_unit_for`], which is the point.)
    #[cfg(test)]
    pub fn amount(&self) -> Decimal {
        self.per_unit * self.covered
    }

    /// The reduction reaching `units` as-acquired units of a
    /// `parcel_quantity`-unit parcel disposed of on `disposed_on` (`None` =
    /// still held).
    ///
    /// A row covering the whole parcel reaches every unit at the statement's
    /// full per-unit figure — including units already sold during the year,
    /// which is the fund attributing to units held *during* the year
    /// (s 104-107B: the adjustment is made just before the end of the income
    /// year, **or just before the time of a relevant CGT event** — LCR 2015/11
    /// para 13).
    ///
    /// A row covering less than the whole parcel covers the units still held
    /// at the year end **first** — that is what
    /// `entities::amit_adjustment_generation` means when it writes
    /// `quantity = remaining_as_of` — and only spills onto the units disposed
    /// of earlier once it covers more than those. Within each of the two
    /// groups the coverage is spread evenly, since units of one parcel are
    /// otherwise indistinguishable. The two rates always reconstruct
    /// [`Self::amount`] exactly, so which units a row reaches never changes
    /// how much it takes off in total. The single division comes last so that
    /// identity holds to the last decimal place even where the covered units
    /// don't divide evenly into the group — which is exactly what [`mul_div`]
    /// preserves, while giving an answer rather than a panic where one of the
    /// two products is too large for `rust_decimal` (a per-unit adjustment
    /// against a holding of 1e15 units).
    pub fn reduction_for_units(
        &self,
        parcel_quantity: Decimal,
        units: Decimal,
        disposed_on: Option<NaiveDate>,
    ) -> Decimal {
        let held = (parcel_quantity - self.disposed_by_year_end).max(Decimal::ZERO);
        let sold = parcel_quantity - held;
        // `disposed_on` on the year end itself counts as sold by it: the
        // statement's year-end position no longer includes those units, which
        // is the same boundary `disposed_by_year_end` was counted on.
        let still_held = disposed_on.is_none_or(|d| d > self.tax_year_end_date);
        if still_held {
            if held <= Decimal::ZERO {
                return Decimal::ZERO;
            }
            mul_div(&[self.per_unit, self.covered.min(held), units], held)
        } else {
            let spill = (self.covered - held).max(Decimal::ZERO);
            if sold <= Decimal::ZERO {
                return Decimal::ZERO;
            }
            mul_div(&[self.per_unit, spill, units], sold)
        }
    }

    /// [`Self::reduction_for_units`] for a single unit — the per-unit figure
    /// the itemised adjustment rows display. Presentation only: the amounts
    /// themselves go through `reduction_for_units`, which keeps its division
    /// to the end rather than rounding a rate and multiplying it back up.
    pub fn per_unit_for(
        &self,
        parcel_quantity: Decimal,
        disposed_on: Option<NaiveDate>,
    ) -> Decimal {
        self.reduction_for_units(parcel_quantity, Decimal::ONE, disposed_on)
    }
}

/// The AMIT cost-base reduction (CGT event E10) reaching `units` as-acquired
/// units of a `parcel_quantity`-unit parcel — [`AmitReductionEvent::per_unit_for`]
/// summed over the parcel's statements. Every caller of [`adjusted_cost_base`]
/// gets this for free; it is public for the net-capital-gain report's own E10
/// walk, which needs the per-statement figures rather than the total.
pub fn amit_reduction_for(
    events: &[AmitReductionEvent],
    parcel_quantity: Decimal,
    units: Decimal,
    disposed_on: Option<NaiveDate>,
) -> Decimal {
    events
        .iter()
        .map(|e| e.reduction_for_units(parcel_quantity, units, disposed_on))
        .sum()
}

/// The cost-base breakdown of some or all of a parcel's units, produced by
/// [`adjusted_cost_base`]. Native currency until [`CostBase::into_aud_with`].
#[derive(Debug, Clone, Copy)]
pub struct CostBase {
    /// Whole-parcel initial cost base: price × quantity + brokerage + GST.
    pub initial_cost: Decimal,
    /// AMIT cost-base reduction reaching the costed units (the full amount,
    /// even where CGT event E10 has floored the cost base).
    pub amit_reduction: Decimal,
    /// Return-of-capital payments received on the costed units (the full
    /// amount, even where CGT event G1 has floored the cost base).
    pub roc_reduction: Decimal,
    /// Adjusted cost base of the costed units: their share of the initial
    /// cost base, less the AMIT and return-of-capital reductions reaching
    /// them, floored at nil (CGT events E10 and G1 both floor at nil — any
    /// excess is a capital gain in the net-capital-gain report, never a
    /// negative cost base).
    pub adjusted: Decimal,
}

/// Step 1's pro-rate: the `units`-unit share of a `quantity`-unit parcel's
/// whole-parcel initial cost base. The one implementation both
/// [`adjusted_cost_base`] and [`adjustment_detail`] start their walks from,
/// so the totals and the itemised rows can never disagree about it.
///
/// `quantity` is non-zero — both callers guard — and the two cases are
/// tried in this order:
///
/// 1. **`units == quantity`** — the overwhelmingly common one (a parcel still
///    held in full, a sale that takes the lot, a rollover consuming a whole
///    parcel) — is the identity, so the figure comes back untouched rather
///    than being scaled up by the quantity and divided straight back down.
/// 2. **Otherwise [`mul_div`]**, which multiplies before it divides (the
///    order this has always used, and the more faithful one: `c × u / q`
///    rounds once, at the end, where `c / q × u` rounds a non-terminating
///    quotient to 28 significant digits first and then scales that error up
///    by `u` — a $39.95 cost base over 3 units, costing 2 of them, is
///    …33333333333333333333333 one way and …33333333333333333333334 the
///    other), and falls back to dividing first only where the product
///    overflows. That is what stopped a mistyped parcel taking down every
///    portfolio read with it (SCENARIOS W-b): `price 1 / quantity 1e15`
///    overflowed at `c × u = 1e30`, and dividing first raises the
///    partial-parcel ceiling by roughly fourteen orders of magnitude.
///
/// The whole-parcel product `price × quantity` inside
/// [`Parcel::initial_cost`] is a separate ceiling this cannot raise: that
/// figure is the cost base, so an unrepresentable one has no lesser answer to
/// give. It panics, and `app::router`'s panic layer turns that into a logged
/// `500` rather than a dropped connection.
fn prorated_initial_cost(initial_cost: Decimal, units: Decimal, quantity: Decimal) -> Decimal {
    if units == quantity {
        return initial_cost;
    }
    mul_div(&[initial_cost, units], quantity)
}

/// Runs steps 1–4 of the pipeline (see the module doc) for `units`
/// as-acquired units of `parcel`.
///
/// `units` is the portion being costed — a sale allocation re-based to
/// as-acquired units, the unsold remainder, or the moved/exchanged quantity.
/// `amit_events` are the parcel's AMMA statements
/// (`amit_adjustment::db_cost_base_reduction_events`); `roc_events` and
/// `splits` are the parcel's listing's events (`corporate_action::
/// db_return_of_capital_events` / `db_share_split_events`). `held` says what
/// became of the costed units — which bounds the return-of-capital payments
/// they were held for, and which side of an AMMA statement's year end they
/// sit on.
pub fn adjusted_cost_base(
    parcel: &Parcel<'_>,
    units: Decimal,
    amit_events: &[AmitReductionEvent],
    roc_events: &[RocEvent],
    splits: &[SplitEvent],
    held: Held,
) -> Result<CostBase, sqlx::Error> {
    let initial_cost = parcel.initial_cost();
    let amit_reduction =
        amit_reduction_for(amit_events, parcel.quantity, units, held.disposed_on());
    let roc_per_unit = per_unit_reduction(
        roc_events,
        splits,
        parcel.currency,
        parcel.trade_date,
        parcel.rolled_over_on,
        held.up_to(),
    )?;
    let roc_reduction = roc_per_unit * units;
    let adjusted = if parcel.quantity > Decimal::ZERO {
        // Both reductions are already stated for the costed units, so they
        // come off that share of the initial cost base directly — no second
        // pro-rating, and one floor covers both (each subtraction only ever
        // moves the balance the same way an earlier floor would have).
        (prorated_initial_cost(initial_cost, units, parcel.quantity)
            - amit_reduction
            - roc_reduction)
            .max(Decimal::ZERO)
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
    /// 2025-06-30)", "Return of capital @ 0.15 AUD/unit", "2-for-1 split",
    /// "1-for-2 consolidation", "1-for-10 bonus issue" (a re-basing event is
    /// named by its own kind and announced terms — see
    /// the event's own `RebaseTerms`).
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
/// `amit_events` are the same [`AmitReductionEvent`]s `adjusted_cost_base`
/// takes (`entities::amit_adjustment::db_cost_base_reduction_events`), in
/// `tax_year_end_date` order. `roc_events`, `splits` and `held` are its other
/// inputs unchanged. Every row describes the **costed units**, so the rows sum
/// to the same reductions that function reports for them.
pub fn adjustment_detail(
    parcel: &Parcel<'_>,
    units: Decimal,
    amit_events: &[AmitReductionEvent],
    roc_events: &[RocEvent],
    splits: &[SplitEvent],
    held: Held,
) -> Result<Vec<CostBaseAdjustment>, sqlx::Error> {
    let mut rows = Vec::new();

    // The costed units' share of the initial cost base: the pool both
    // reduction kinds draw down, the same [`prorated_initial_cost`] term
    // adjusted_cost_base starts from.
    let mut running = if parcel.quantity > Decimal::ZERO {
        prorated_initial_cost(parcel.initial_cost(), units, parcel.quantity)
    } else {
        Decimal::ZERO
    };

    // AMIT reductions (CGT event E10) on the costed units, in statement-year
    // order. The running balance floors at nil exactly like
    // adjusted_cost_base's single `.max(0)` — flooring after each step never
    // loses value versus one accumulated subtraction, since every step only
    // ever subtracts a non-negative amount.
    for e in amit_events {
        // The reduction reaching *these* units, which is the statement's own
        // per-unit figure only where its row covers them.
        let amount = e.reduction_for_units(parcel.quantity, units, held.disposed_on());
        let per_unit = e.per_unit_for(parcel.quantity, held.disposed_on());
        let before = running;
        running = (running - amount).max(Decimal::ZERO);
        let capped = before <= Decimal::ZERO || amount > before;
        rows.push(CostBaseAdjustment {
            kind: AdjustmentKind::AmitCostBase,
            date: e.tax_year_end_date,
            reference: format!(
                "AMMA statement {} (year ended {})",
                e.amma_statement_id, e.tax_year_end_date
            ),
            per_unit: Some(per_unit),
            amount,
            capped,
        });
    }

    // Return-of-capital payments (CGT event G1), on the costed units too —
    // drawing down what the AMIT rows left of the same pool.
    let mut roc_running = running;
    for e in roc_events {
        // Applicability, the currency guard and the split re-basing are the
        // payment's own (`RocEvent::per_unit_for`) — the same call
        // `per_unit_reduction` sums, so the itemised rows can't describe a
        // different set of payments than the totals were computed from.
        let Some(per_unit) = e.per_unit_for(
            splits,
            parcel.currency,
            parcel.trade_date,
            parcel.rolled_over_on,
            held.up_to(),
        )?
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
        if s.date < parcel.trade_date || held.up_to().is_some_and(|d| s.date > d) {
            continue;
        }
        rows.push(CostBaseAdjustment {
            kind: AdjustmentKind::SplitRebase,
            date: s.date,
            // Named from the action's own kind and announced terms
            // (`RebaseTerms::label`), never from the derived rebase factor:
            // this string is printed in an archived worksheet and reconciled
            // against the company's announcement, and a bonus issue's factor
            // (11/10 for a 1-for-10 issue) appears in no announcement.
            reference: s.terms.label(),
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
            rolled_over_on: None,
        }
    }

    /// One AMMA statement's adjustment row against a parcel: `per_unit` per
    /// as-acquired unit over `covered` units, `disposed` units of the parcel
    /// having already been sold when the statement's year (ending 30 June
    /// `year`) closed.
    fn amit(id: i64, year: i32, per_unit: &str, covered: i64, disposed: i64) -> AmitReductionEvent {
        AmitReductionEvent {
            amma_statement_id: id,
            tax_year_end_date: date(year, 6, 30),
            per_unit: per_unit.parse().unwrap(),
            covered: Decimal::from(covered),
            disposed_by_year_end: Decimal::from(disposed),
        }
    }

    /// The ordinary case: the row covers the whole parcel and none of it had
    /// been sold by the year end.
    fn whole(id: i64, year: i32, per_unit: &str, quantity: i64) -> AmitReductionEvent {
        amit(id, year, per_unit, quantity, 0)
    }

    #[test]
    fn whole_parcel_initial_cost_includes_brokerage_and_gst() {
        let p = Parcel {
            brokerage: "9.95".parse().unwrap(),
            gst_on_brokerage: "0.995".parse().unwrap(),
            ..parcel(100, 10)
        };
        let cb =
            adjusted_cost_base(&p, Decimal::from(100), &[], &[], &[], Held::AsAt(None)).unwrap();
        assert_eq!(cb.initial_cost, "1010.945".parse::<Decimal>().unwrap());
        assert_eq!(cb.adjusted, "1010.945".parse::<Decimal>().unwrap());
    }

    #[test]
    fn partial_units_pro_rate_the_cost_base() {
        let cb = adjusted_cost_base(
            &parcel(100, 10),
            Decimal::from(40),
            &[],
            &[],
            &[],
            Held::AsAt(None),
        )
        .unwrap();
        assert_eq!(cb.adjusted, Decimal::from(400));
    }

    /// SCENARIOS W-b. The pro-rate used to multiply before it divided, so a
    /// parcel whose `initial_cost × units` passed `rust_decimal`'s ~7.9228e28
    /// ceiling panicked — and, caught by nothing, dropped the connection of
    /// every portfolio read that touched it. The boundary measured before the
    /// fix: `price 1 / quantity 1e14` (product 1e28) costed fine, `price 1 /
    /// quantity 1e15` (1e30) did not, nor did `price 0.5 / quantity 4e14`
    /// (8e28).
    #[test]
    fn a_parcel_past_the_old_multiply_first_ceiling_still_costs() {
        let p = Parcel {
            quantity: "1000000000000000".parse().unwrap(),
            ..parcel(1, 1)
        };
        // The whole parcel: the pro-rate is the identity and never multiplies.
        let whole = adjusted_cost_base(&p, p.quantity, &[], &[], &[], Held::AsAt(None)).unwrap();
        assert_eq!(
            whole.adjusted,
            "1000000000000000".parse::<Decimal>().unwrap()
        );
        // A partial slice, where the identity does not apply: the product
        // 1e15 × 3e14 = 3e29 overflows, so it divides first instead.
        let part = adjusted_cost_base(
            &p,
            "300000000000000".parse().unwrap(),
            &[],
            &[],
            &[],
            Held::AsAt(None),
        )
        .unwrap();
        assert_eq!(part.adjusted, "300000000000000".parse::<Decimal>().unwrap());
        // The other measured overflow, price 0.5 / quantity 4e14.
        let half = Parcel {
            quantity: "400000000000000".parse().unwrap(),
            average_price: "0.5".parse().unwrap(),
            ..parcel(1, 1)
        };
        let part = adjusted_cost_base(
            &half,
            "100000000000000".parse().unwrap(),
            &[],
            &[],
            &[],
            Held::AsAt(None),
        )
        .unwrap();
        assert_eq!(part.adjusted, "50000000000000".parse::<Decimal>().unwrap());
        // The itemised detail walks the same term, so it survives too.
        let rows = adjustment_detail(
            &p,
            "300000000000000".parse().unwrap(),
            &[],
            &[],
            &[],
            Held::AsAt(None),
        )
        .unwrap();
        assert!(rows.is_empty());
    }

    /// The figures below the old ceiling are untouched by the reordering:
    /// where the product fits, it is still formed first and divided once, so
    /// nothing that worked before moves by a digit. `39.95` over 3 units,
    /// costing 2 of them, is the case that tells the two orders apart —
    /// `39.95 × 2 / 3` ends `…333`, `39.95 / 3 × 2` ends `…334`.
    #[test]
    fn pro_rating_keeps_the_multiply_first_figure_wherever_it_fits() {
        let p = Parcel {
            brokerage: "9.95".parse().unwrap(),
            ..parcel(3, 10)
        };
        let cb = adjusted_cost_base(&p, Decimal::from(2), &[], &[], &[], Held::AsAt(None)).unwrap();
        assert_eq!(
            cb.adjusted,
            "26.633333333333333333333333333".parse::<Decimal>().unwrap()
        );
        // …and the largest parcel that costed before the fix still gives what
        // it gave then, whole and pro-rated.
        let big = Parcel {
            quantity: "100000000000000".parse().unwrap(),
            ..parcel(1, 1)
        };
        let whole =
            adjusted_cost_base(&big, big.quantity, &[], &[], &[], Held::AsAt(None)).unwrap();
        assert_eq!(
            whole.adjusted,
            "100000000000000".parse::<Decimal>().unwrap()
        );
        let part = adjusted_cost_base(
            &big,
            "33333333333333".parse().unwrap(),
            &[],
            &[],
            &[],
            Held::AsAt(None),
        )
        .unwrap();
        assert_eq!(part.adjusted, "33333333333333".parse::<Decimal>().unwrap());
    }

    #[test]
    fn amit_reduction_nets_off_and_e10_floors_at_nil() {
        // Reduction within the cost base nets off…
        let cb = adjusted_cost_base(
            &parcel(100, 10),
            Decimal::from(100),
            &[whole(1, 2024, "0.05", 100)],
            &[],
            &[],
            Held::AsAt(None),
        )
        .unwrap();
        assert_eq!(cb.amit_reduction, Decimal::from(5));
        assert_eq!(cb.adjusted, Decimal::from(995));
        // …and a reduction exceeding it floors at nil (CGT event E10), while
        // the full reduction is still reported.
        let cb = adjusted_cost_base(
            &parcel(100, 10),
            Decimal::from(100),
            &[whole(1, 2024, "11", 100)],
            &[],
            &[],
            Held::AsAt(None),
        )
        .unwrap();
        assert_eq!(cb.amit_reduction, Decimal::from(1100));
        assert_eq!(cb.adjusted, Decimal::ZERO);
    }

    /// SCENARIOS D-13. A row covering only part of a parcel states its
    /// reduction for *those* units — so the units it covers each lose the
    /// statement's full per-unit figure, and the units sold before the year
    /// end (which the row does not cover) lose nothing. Pooling the row's
    /// total across the whole parcel instead moved reduction from the one to
    /// the other, understating the sale's cost base and overstating what the
    /// remainder carried forward.
    #[test]
    fn a_partial_row_reduces_only_the_units_it_covers() {
        // 100 units at $10; 40 sold in March; the AMMA statement for the year
        // ended 30 June covers the 60 still held at 50c/unit.
        let sold_on = date(2024, 3, 1);
        let events = [amit(1, 2024, "0.50", 60, 40)];

        let held = adjusted_cost_base(
            &parcel(100, 10),
            Decimal::from(60),
            &events,
            &[],
            &[],
            Held::AsAt(None),
        )
        .unwrap();
        // 60 units each reduced by the stated 50c: 600 − 30.
        assert_eq!(held.amit_reduction, Decimal::from(30));
        assert_eq!(held.adjusted, Decimal::from(570));

        let sold = adjusted_cost_base(
            &parcel(100, 10),
            Decimal::from(40),
            &events,
            &[],
            &[],
            Held::DisposedOn(sold_on),
        )
        .unwrap();
        // The row does not cover these units, so they keep their full share.
        assert_eq!(sold.amit_reduction, Decimal::ZERO);
        assert_eq!(sold.adjusted, Decimal::from(400));
    }

    /// The contrasting entry: a row covering the *whole* parcel after a
    /// mid-year disposal is the fund attributing to units held during the
    /// year (s 104-107B / LCR 2015/11 para 13), and must keep reaching the
    /// sold units — every unit takes the statement's full per-unit figure.
    #[test]
    fn a_whole_parcel_row_still_reaches_the_units_already_sold() {
        let events = [amit(1, 2024, "0.50", 100, 40)];

        let held = adjusted_cost_base(
            &parcel(100, 10),
            Decimal::from(60),
            &events,
            &[],
            &[],
            Held::AsAt(None),
        )
        .unwrap();
        assert_eq!(held.amit_reduction, Decimal::from(30));
        assert_eq!(held.adjusted, Decimal::from(570));

        let sold = adjusted_cost_base(
            &parcel(100, 10),
            Decimal::from(40),
            &events,
            &[],
            &[],
            Held::DisposedOn(date(2024, 3, 1)),
        )
        .unwrap();
        assert_eq!(sold.amit_reduction, Decimal::from(20));
        assert_eq!(sold.adjusted, Decimal::from(380));
    }

    /// Whichever units a row reaches, the total it takes off the parcel is
    /// exactly the amount its own arithmetic states (covered × per unit) —
    /// coverage decides the *split*, never the size.
    #[test]
    fn the_stated_reduction_is_conserved_however_it_is_split() {
        for covered in [0, 20, 40, 60, 100] {
            let e = amit(1, 2024, "0.50", covered, 40);
            let held = e.reduction_for_units(Decimal::from(100), Decimal::from(60), None);
            let sold = e.reduction_for_units(
                Decimal::from(100),
                Decimal::from(40),
                Some(date(2024, 3, 1)),
            );
            assert_eq!(held + sold, e.amount(), "covered {covered}");
        }
    }

    /// SCENARIOS W-b's residual, closed in the same treatment. The AMIT
    /// reduction pro-rates with **two** multiplications before its divide
    /// (`per_unit × covered × units / held`), so either of them could pass
    /// `rust_decimal`'s ~7.9228e28 ceiling and panic — measured before the
    /// fix: 1e15 units at 5c per unit survives (1.5e28), the same holding at
    /// 50c does not (5e29), and a per-unit figure of 1e15 overflows on the
    /// first multiplication alone (1e30). Every one of those answers is
    /// representable; only the working was not.
    #[test]
    fn an_amit_row_past_the_old_multiply_first_ceiling_still_reduces() {
        let units: Decimal = "1000000000000000".parse().unwrap();
        // The measured survivor, unchanged: 5c over the whole 1e15-unit parcel.
        let small = AmitReductionEvent {
            per_unit: "0.05".parse().unwrap(),
            covered: units,
            ..amit(1, 2024, "0.05", 0, 0)
        };
        assert_eq!(
            small.reduction_for_units(units, units, None),
            "50000000000000".parse::<Decimal>().unwrap()
        );
        // The second multiplication overflows (5e14 × 1e15 = 5e29).
        let big = AmitReductionEvent {
            per_unit: "0.5".parse().unwrap(),
            covered: units,
            ..amit(1, 2024, "0.5", 0, 0)
        };
        assert_eq!(
            big.reduction_for_units(units, units, None),
            "500000000000000".parse::<Decimal>().unwrap()
        );
        // The first one overflows on its own (1e15 × 1e15 = 1e30) — a
        // per-unit adjustment with a mistyped run of zeros.
        let huge = AmitReductionEvent {
            per_unit: units,
            covered: units,
            ..amit(1, 2024, "1", 0, 0)
        };
        assert_eq!(huge.per_unit_for(units, None), units);
        // The sold branch divides by its own denominator and overflows the
        // same way: the whole 1e15-unit parcel was sold during the year, so
        // all of the coverage spills onto it.
        let sold = AmitReductionEvent {
            per_unit: "0.5".parse().unwrap(),
            covered: units,
            disposed_by_year_end: units,
            ..amit(1, 2024, "0.5", 0, 0)
        };
        assert_eq!(
            sold.reduction_for_units(units, units, Some(date(2024, 3, 1))),
            "500000000000000".parse::<Decimal>().unwrap()
        );
    }

    /// The other half of that treatment: a reduction that fits today must not
    /// move by a digit. Both branches divide 39.95 × 2 by 3, whose quotient
    /// does not terminate — so the multiply-first figure and the divide-first
    /// one differ in the last place, and only the multiply-first one is this.
    #[test]
    fn a_reduction_that_fits_keeps_its_multiply_first_figure() {
        let held = amit(1, 2024, "39.95", 2, 0);
        assert_eq!(
            held.reduction_for_units(Decimal::from(3), Decimal::ONE, None),
            "26.633333333333333333333333333".parse::<Decimal>().unwrap()
        );
        // 5 units, 3 sold by the year end, 4 covered: 2 units of coverage
        // spill onto the 3 sold ones.
        let sold = amit(1, 2024, "39.95", 4, 3);
        assert_eq!(
            sold.reduction_for_units(Decimal::from(5), Decimal::ONE, Some(date(2024, 3, 1))),
            "26.633333333333333333333333333".parse::<Decimal>().unwrap()
        );
        // Divide-first would have given …334 in both.
        assert_ne!(
            "26.633333333333333333333333333".parse::<Decimal>().unwrap(),
            "39.95".parse::<Decimal>().unwrap() / Decimal::from(3) * Decimal::from(2)
        );
    }

    /// A row covering more units than are still held spills the excess evenly
    /// over the units sold during the year — the two groups' rates only
    /// coincide when the row covers the whole parcel.
    #[test]
    fn coverage_beyond_the_units_still_held_spills_onto_the_sold_ones() {
        // 80 of 100 units covered, 40 already sold: the 60 still held take the
        // full 50c, and the remaining 20 units of coverage spread over the 40
        // sold ones at 25c each.
        let e = amit(1, 2024, "0.50", 80, 40);
        assert_eq!(
            e.per_unit_for(Decimal::from(100), None),
            "0.50".parse().unwrap()
        );
        assert_eq!(
            e.per_unit_for(Decimal::from(100), Some(date(2024, 3, 1))),
            "0.25".parse().unwrap()
        );
    }

    /// A sale *on* the statement's year end is a disposal by it: those units
    /// are no longer in the year-end position the row's quantity describes.
    #[test]
    fn a_disposal_on_the_year_end_itself_counts_as_sold_by_it() {
        let e = amit(1, 2024, "0.50", 60, 40);
        assert_eq!(
            e.per_unit_for(Decimal::from(100), Some(date(2024, 6, 30))),
            Decimal::ZERO
        );
        // A sale the next day was still held at the year end.
        assert_eq!(
            e.per_unit_for(Decimal::from(100), Some(date(2024, 7, 1))),
            "0.50".parse().unwrap()
        );
    }

    /// `adjustment_detail`'s itemised AMIT rows must sum to exactly the same
    /// `amit_reduction` `adjusted_cost_base` reports — including the E10
    /// floored case, where the second statement is the one that pushes the
    /// running balance to nil.
    #[test]
    fn itemised_amit_rows_sum_to_the_netted_reduction_including_the_floored_case() {
        // Two statements, neither alone exceeding the $1000 cost base.
        let events = [whole(1, 2024, "5", 100), whole(2, 2025, "6", 100)];
        let cb = adjusted_cost_base(
            &parcel(100, 10),
            Decimal::from(100),
            &events,
            &[],
            &[],
            Held::AsAt(None),
        )
        .unwrap();
        let rows = adjustment_detail(
            &parcel(100, 10),
            Decimal::from(100),
            &events,
            &[],
            &[],
            Held::AsAt(None),
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

    /// The itemised rows describe the *costed* units, so a partial row shows
    /// the per-unit figure it actually applied to them — and still sums to
    /// what `adjusted_cost_base` reported.
    #[test]
    fn itemised_amit_rows_describe_the_costed_units_of_a_partial_row() {
        let events = [amit(1, 2024, "0.50", 60, 40)];
        let rows = adjustment_detail(
            &parcel(100, 10),
            Decimal::from(40),
            &events,
            &[],
            &[],
            Held::DisposedOn(date(2024, 3, 1)),
        )
        .unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].per_unit, Some(Decimal::ZERO));
        assert_eq!(rows[0].amount, Decimal::ZERO);

        let rows = adjustment_detail(
            &parcel(100, 10),
            Decimal::from(60),
            &events,
            &[],
            &[],
            Held::AsAt(None),
        )
        .unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].per_unit, Some("0.50".parse().unwrap()));
        assert_eq!(rows[0].amount, Decimal::from(30));
    }

    #[test]
    fn roc_payments_reduce_and_g1_floors_at_nil() {
        let roc = |amount: &str, d: NaiveDate| RocEvent {
            date: d,
            amount_per_unit: amount.parse().unwrap(),
            currency: "AUD".to_string(),
            record_date: None,
        };
        // 50c per unit on 100 units held: cost base 1000 → 950.
        let cb = adjusted_cost_base(
            &parcel(100, 10),
            Decimal::from(100),
            &[],
            &[roc("0.50", date(2024, 3, 1))],
            &[],
            Held::AsAt(None),
        )
        .unwrap();
        assert_eq!(cb.roc_reduction, Decimal::from(50));
        assert_eq!(cb.adjusted, Decimal::from(950));
        // $11 per unit exceeds the $10 cost base: floored at nil (CGT event
        // G1), full payment still reported.
        let cb = adjusted_cost_base(
            &parcel(100, 10),
            Decimal::from(100),
            &[],
            &[roc("11", date(2024, 3, 1))],
            &[],
            Held::AsAt(None),
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
            record_date: None,
        };
        // Payment after the sale date: the units were no longer held.
        let cb = adjusted_cost_base(
            &parcel(100, 10),
            Decimal::from(100),
            &[],
            &[roc],
            &[],
            Held::DisposedOn(date(2024, 6, 1)),
        )
        .unwrap();
        assert_eq!(cb.roc_reduction, Decimal::ZERO);
        assert_eq!(cb.adjusted, Decimal::from(1000));
    }

    #[test]
    fn roc_after_a_split_is_per_post_split_unit() {
        // 2-for-1 split, then 25c per post-split unit = 50c per as-acquired
        // unit: 100 as-acquired units lose 100 × 0.50 = 50.
        let split = SplitEvent::share_split(date(2024, 3, 1), Decimal::from(2), Decimal::ONE);
        let roc = RocEvent {
            date: date(2024, 4, 1),
            amount_per_unit: "0.25".parse().unwrap(),
            currency: "AUD".to_string(),
            record_date: None,
        };
        let cb = adjusted_cost_base(
            &parcel(100, 10),
            Decimal::from(100),
            &[],
            &[roc],
            &[split],
            Held::AsAt(None),
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
        let split = SplitEvent::share_split(date(2024, 3, 1), Decimal::from(2), Decimal::ONE);
        // Two ROC payments after the split: the second (per post-split unit)
        // exceeds what's left of the parcel's $1000 cost base and floors it.
        let roc = |amount: &str, d: NaiveDate| RocEvent {
            date: d,
            amount_per_unit: amount.parse().unwrap(),
            currency: "AUD".to_string(),
            record_date: None,
        };
        let events = [roc("0.25", date(2024, 4, 1)), roc("5", date(2024, 5, 1))];
        let cb = adjusted_cost_base(
            &parcel(100, 10),
            Decimal::from(100),
            &[],
            &events,
            std::slice::from_ref(&split),
            Held::AsAt(None),
        )
        .unwrap();
        let rows = adjustment_detail(
            &parcel(100, 10),
            Decimal::from(100),
            &[],
            &events,
            &[split],
            Held::AsAt(None),
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

    /// SCENARIOS Z-e: an informational re-basing row is named by the action
    /// it came from, at the ratio that action's own terms were stated in.
    /// The worksheet this reaches is archived and reconciled against the
    /// company's announcement, so the derived rebase factor (11/10 for a
    /// 1-for-10 bonus issue) is exactly the wrong thing to print, and
    /// "split" is the wrong word for a consolidation.
    #[test]
    fn rebase_rows_are_named_by_their_own_kind_and_announced_terms() {
        let events = [
            SplitEvent::share_split(date(2024, 3, 1), Decimal::from(2), Decimal::ONE),
            SplitEvent::share_split(date(2024, 4, 1), Decimal::ONE, Decimal::from(2)),
            SplitEvent::bonus_issue(date(2024, 5, 1), Decimal::ONE, Decimal::from(10)),
            // Terms typed with a scale were announced as "2-for-1".
            SplitEvent::share_split(
                date(2024, 6, 1),
                "2.00".parse().unwrap(),
                "1.00".parse().unwrap(),
            ),
            // The degenerate 1-for-1 ShareSplit: representable (both terms
            // positive is all the write path asks), re-bases nothing, and
            // keeps the action's own name — "consolidation" would claim a
            // unit count fell.
            SplitEvent::share_split(date(2024, 7, 1), Decimal::ONE, Decimal::ONE),
        ];
        let rows = adjustment_detail(
            &parcel(100, 10),
            Decimal::from(100),
            &[],
            &[],
            &events,
            Held::AsAt(None),
        )
        .unwrap();

        let references: Vec<&str> = rows.iter().map(|r| r.reference.as_str()).collect();
        assert_eq!(
            references,
            vec![
                "2-for-1 split",
                "1-for-2 consolidation",
                "1-for-10 bonus issue",
                "2-for-1 split",
                "1-for-1 split",
            ]
        );
        // Every one is still the informational nil-amount row it was.
        assert!(rows.iter().all(|r| r.kind == AdjustmentKind::SplitRebase
            && r.amount == Decimal::ZERO
            && r.per_unit.is_none()
            && !r.capped));
    }

    /// The label moved; the arithmetic did not. A return of capital after a
    /// bonus issue and a consolidation is still per unit *of the basis it was
    /// paid in*, re-based through both events' derived factors (11/10 then
    /// 1/2) onto the parcel's as-acquired units.
    #[test]
    fn a_bonus_issue_and_a_consolidation_rebase_by_their_derived_factors() {
        let events = [
            SplitEvent::bonus_issue(date(2024, 3, 1), Decimal::ONE, Decimal::from(10)),
            SplitEvent::share_split(date(2024, 5, 1), Decimal::ONE, Decimal::from(2)),
        ];
        let roc = |amount: &str, d: NaiveDate| RocEvent {
            date: d,
            amount_per_unit: amount.parse().unwrap(),
            currency: "AUD".to_string(),
            record_date: None,
        };
        // 100 as-acquired units → 110 after the bonus issue → 55 after the
        // consolidation. 10c per post-bonus unit is 11c per as-acquired unit;
        // 20c per post-consolidation unit is 11c per as-acquired unit too.
        let payments = [roc("0.10", date(2024, 4, 1)), roc("0.20", date(2024, 6, 1))];
        let cb = adjusted_cost_base(
            &parcel(100, 10),
            Decimal::from(100),
            &[],
            &payments,
            &events,
            Held::AsAt(None),
        )
        .unwrap();
        assert_eq!(cb.roc_reduction, Decimal::from(22));
        assert_eq!(cb.adjusted, Decimal::from(978));

        // The itemised rows agree with those totals, beside the two
        // informational rows naming the events that re-based the units.
        let rows = adjustment_detail(
            &parcel(100, 10),
            Decimal::from(100),
            &[],
            &payments,
            &events,
            Held::AsAt(None),
        )
        .unwrap();
        let roc_total: Decimal = rows
            .iter()
            .filter(|r| r.kind == AdjustmentKind::ReturnOfCapital)
            .map(|r| r.amount)
            .sum();
        assert_eq!(roc_total, cb.roc_reduction);
        let rebases: Vec<&str> = rows
            .iter()
            .filter(|r| r.kind == AdjustmentKind::SplitRebase)
            .map(|r| r.reference.as_str())
            .collect();
        assert_eq!(
            rebases,
            vec!["1-for-10 bonus issue", "1-for-2 consolidation"]
        );
    }

    #[test]
    fn roc_in_a_different_currency_fails_loudly() {
        let roc = RocEvent {
            date: date(2024, 3, 1),
            amount_per_unit: "0.50".parse().unwrap(),
            currency: "USD".to_string(),
            record_date: None,
        };
        let err = adjusted_cost_base(
            &parcel(100, 10),
            Decimal::from(100),
            &[],
            &[roc],
            &[],
            Held::AsAt(None),
        )
        .unwrap_err();
        assert!(err.to_string().contains("currency"));
    }

    #[test]
    fn zero_quantity_parcel_costs_nil() {
        let cb = adjusted_cost_base(
            &parcel(0, 10),
            Decimal::ZERO,
            &[],
            &[],
            &[],
            Held::AsAt(None),
        )
        .unwrap();
        assert_eq!(cb.adjusted, Decimal::ZERO);
    }

    #[test]
    fn into_aud_with_passes_aud_through_unchanged() {
        let cb = adjusted_cost_base(
            &parcel(100, 10),
            Decimal::from(100),
            &[],
            &[],
            &[],
            Held::AsAt(None),
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
            record_date: None,
        };
        let cb = adjusted_cost_base(
            &p,
            Decimal::from(100),
            &[whole(1, 2024, "0.05", 100)],
            &[roc],
            &[],
            Held::AsAt(None),
        )
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

    /// The documented asymmetry, pinned with *both* months imported so the
    /// choice is visible rather than incidental: a non-AUD parcel's AMIT
    /// (CGT event E10) and return-of-capital (G1) reductions convert at the
    /// **acquisition** month's rate, not at the rate of the month the
    /// reduction arose in (SCENARIOS M-10, `docs/API.md` Known limitations,
    /// "Cost-base FX timing"). That keeps `initial − reductions = adjusted`
    /// true in AUD exactly as in the native currency; strict s 960-50(6)
    /// translation would use each amount's own month and is not modelled.
    #[tokio::test]
    async fn a_reduction_converts_at_the_acquisition_month_not_its_own() {
        let pool = db::init(":memory:").await.unwrap();
        // A$1 = 0.50 USD when the parcel was acquired, 1.00 when the
        // reductions arose — a rate that halves the AUD reduction.
        rba_fx_rate::db_import_rate(&pool, "USD", "2024-01", "0.50".parse().unwrap())
            .await
            .unwrap();
        rba_fx_rate::db_import_rate(&pool, "USD", "2024-06", "1.00".parse().unwrap())
            .await
            .unwrap();
        let rates = fx::FxRates::load(&pool).await.unwrap();
        let p = Parcel {
            currency: "USD",
            ..parcel(100, 10)
        };
        let roc = RocEvent {
            date: date(2024, 6, 1),
            amount_per_unit: "0.50".parse().unwrap(),
            currency: "USD".to_string(),
            record_date: None,
        };
        let cb = adjusted_cost_base(
            &p,
            Decimal::from(100),
            &[whole(1, 2024, "0.05", 100)],
            &[roc],
            &[],
            Held::AsAt(None),
        )
        .unwrap();
        let aud = cb
            .into_aud_with(&rates, "USD", date(2024, 1, 15), fx::FxOverride::None)
            .unwrap();
        // Both reductions take the January rate (US$5 → A$10, US$50 → A$100),
        // not June's (which would give A$5 and A$50).
        assert_eq!(aud.amit_reduction, Decimal::from(10));
        assert_eq!(aud.roc_reduction, Decimal::from(100));
        // And the breakdown still adds up in AUD, which is what the single
        // rate buys: 2000 − 10 − 100 = 1890.
        assert_eq!(
            aud.adjusted,
            aud.initial_cost - aud.amit_reduction - aud.roc_reduction
        );
        assert_eq!(aud.adjusted, Decimal::from(1890));
    }

    // -----------------------------------------------------------------------
    // checked_cost_base (SCENARIOS W-e)
    // -----------------------------------------------------------------------

    fn dec(s: &str) -> Decimal {
        s.parse().unwrap()
    }

    /// The bound is the type's own: a product `Decimal` can hold is returned,
    /// the first one it cannot is refused, and the two are one step apart.
    #[test]
    fn the_bound_is_exactly_what_a_decimal_can_hold() {
        let terms = |price: &'static str, units: Decimal| {
            checked_cost_base(&[Term::Product {
                price: ("average_price", dec(price)),
                units: ("quantity", units),
            }])
        };
        // Decimal::MAX itself, reached as 1 × MAX.
        assert_eq!(terms("1", Decimal::MAX).unwrap(), Decimal::MAX);
        // One unit past it.
        assert!(terms("2", Decimal::MAX).is_err());
        // The finding's own figures.
        assert!(terms("1000000000000000", dec("1000000000000000")).is_err());
        // The control the finding was bounded with: 1e14 units at 1 is fine.
        assert_eq!(
            terms("1", dec("100000000000000")).unwrap(),
            dec("100000000000000")
        );
    }

    /// The additive terms are checked too — an inheritance has no product at
    /// all, just `cost_base + lpr_expenditure`, and the pair can overflow on
    /// its own.
    #[test]
    fn the_additive_terms_are_bounded_as_well_as_the_product() {
        // 3e28 + 4e28 fits; 4e28 + 4e28 does not.
        assert_eq!(
            checked_cost_base(&[
                Term::Amount("cost_base", dec("30000000000000000000000000000")),
                Term::Amount("lpr_expenditure", dec("40000000000000000000000000000")),
            ])
            .unwrap(),
            dec("70000000000000000000000000000")
        );
        assert!(
            checked_cost_base(&[
                Term::Amount("cost_base", dec("40000000000000000000000000000")),
                Term::Amount("lpr_expenditure", dec("40000000000000000000000000000")),
            ])
            .is_err()
        );
        // And a representable product with an unrepresentable cost on top.
        assert!(
            checked_cost_base(&[
                Term::Product {
                    price: ("exercise_price", Decimal::ONE),
                    units: ("units", Decimal::MAX),
                },
                Term::Amount("rights_cost", Decimal::ONE),
            ])
            .is_err()
        );
    }

    /// The refusal quotes the arithmetic back in the caller's own field names,
    /// and names the limit — the two things the `422` body has to carry.
    #[test]
    fn the_refusal_names_the_terms_and_the_limit() {
        let err = checked_cost_base(&[
            Term::Product {
                price: ("average_price", dec("1000000000000000")),
                units: ("quantity", dec("1000000000000000")),
            },
            Term::Amount("brokerage", dec("19.95")),
            Term::Amount("gst_on_brokerage", Decimal::ZERO),
        ])
        .unwrap_err();
        assert_eq!(
            err.expression,
            concat!(
                "average_price 1000000000000000 × quantity 1000000000000000",
                " + brokerage 19.95 + gst_on_brokerage 0"
            )
        );
        let message = err.message();
        assert!(message.contains(&err.expression), "{message}");
        assert!(message.contains(&Decimal::MAX.to_string()), "{message}");
    }

    /// It is [`Parcel::initial_cost`]'s formula, not a second one: wherever the
    /// parcel can answer, the two agree to the digit.
    #[test]
    fn it_is_the_same_formula_parcel_initial_cost_uses() {
        let p = Parcel {
            brokerage: dec("19.95"),
            gst_on_brokerage: dec("1.99"),
            ..parcel(300, 7)
        };
        let checked = checked_cost_base(&[
            Term::Product {
                price: ("average_price", p.average_price),
                units: ("quantity", p.quantity),
            },
            Term::Amount("brokerage", p.brokerage),
            Term::Amount("gst_on_brokerage", p.gst_on_brokerage),
        ])
        .unwrap();
        assert_eq!(checked, p.initial_cost());
    }

    // -----------------------------------------------------------------------
    // checked_rebased_quantity ("A replacement quantity no `Decimal` can hold")
    // -----------------------------------------------------------------------

    /// The bound is the type's own here too: the last re-based quantity a
    /// `Decimal` can hold is returned, the first one it cannot is refused.
    #[test]
    fn the_rebased_quantity_bound_is_exactly_what_a_decimal_can_hold() {
        let rebased = |units: Decimal, new: &'static str, old: &'static str| {
            checked_rebased_quantity(("units held", units), ("new", dec(new)), ("old", dec(old)))
        };
        assert_eq!(rebased(Decimal::MAX, "1", "1").unwrap(), Decimal::MAX);
        assert!(rebased(Decimal::MAX, "2", "1").is_err());
        // The finding's own figures: 1e27 units on a 1000-for-1 ratio wants
        // 1e30 units, past the ceiling however the arithmetic is ordered.
        assert!(rebased(dec("1e27"), "1000", "1").is_err());
        // The control it is bounded with: the same holding and the same
        // numerator over a wide enough denominator answers 1e24, and that is
        // the *result* being representable rather than the working.
        assert_eq!(
            rebased(dec("1e27"), "1000", "1000000").unwrap(),
            dec("1e24")
        );
    }

    /// It is [`mul_div`]'s arithmetic, not a second one: wherever `mul_div`
    /// can answer, the checked form answers the same thing — including the
    /// divide-early cases `mul_div` exists for, where the plain
    /// multiply-then-divide would overflow in the working.
    #[test]
    fn a_rebased_quantity_is_mul_divs_answer_wherever_that_fits() {
        for (units, new, old) in [
            ("100", "3", "7"),
            ("1e27", "1000", "1000000"),
            ("1e15", "1e15", "1e15"),
            ("0", "1000", "1"),
            ("123.456789", "7", "3"),
        ] {
            let (units, new, old) = (dec(units), dec(new), dec(old));
            assert_eq!(
                checked_rebased_quantity(("q", units), ("new", new), ("old", old)).unwrap(),
                mul_div(&[units, new], old),
                "{units} × {new} / {old}"
            );
        }
    }

    /// The refusal quotes the arithmetic back in the caller's own field names
    /// — the ratio *and* the holding that produced it — and names the limit.
    #[test]
    fn the_quantity_refusal_names_the_ratio_and_the_holding() {
        let err = checked_rebased_quantity(
            ("units held", dec("1e27")),
            ("scrip_new_units", dec("1000")),
            ("scrip_old_units", Decimal::ONE),
        )
        .unwrap_err();
        assert_eq!(
            err.expression,
            "units held 1000000000000000000000000000 × scrip_new_units 1000 / scrip_old_units 1"
        );
        let message = err.message();
        assert!(message.contains(&err.expression), "{message}");
        assert!(message.contains(&Decimal::MAX.to_string()), "{message}");
        // The same sentence the cost-base refusal uses, said once
        // (`beyond_the_range`) — only what the total *is* differs.
        assert!(
            message.contains("these figures multiply out beyond what can be recorded"),
            "{message}"
        );
        assert!(
            message.contains("the number of units the holding becomes"),
            "{message}"
        );
    }
}
