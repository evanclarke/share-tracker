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
//!    `sold_in_acquired_units` before calling, and per-unit payments are
//!    re-based inside `corporate_action::per_unit_reduction`.
//! 5. **AUD conversion at the acquisition month**
//!    ([`CostBase::into_aud_with`]) — reports take the Australian-tax view,
//!    so the cost base converts at the ATO reference rate for the parcel's
//!    (possibly deemed) acquisition month. A rollover replacement parcel
//!    converts at its *deemed* acquisition month, carrying the original AUD
//!    cost base over.

use chrono::NaiveDate;
use rust_decimal::Decimal;
use sqlx::{Row, sqlite::SqliteRow};

use crate::entities::corporate_action::{RocEvent, SplitEvent, per_unit_reduction};
use crate::infra::decimal::row_dec;
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

/// A Buy/DRP trade row as every cost-base report reads it — one `FromRow`
/// mapping (TEXT decimal columns via the `infra::decimal` helpers) instead of
/// a per-report field-by-field copy. Select [`ParcelRow::COLUMNS`] from
/// `trades`.
#[derive(Debug, Clone)]
pub struct ParcelRow {
    pub id: i64,
    pub listing_id: i64,
    pub holding_account_id: i64,
    /// The actual trade date — drives split and return-of-capital
    /// applicability (see [`Parcel::trade_date`]).
    pub date: NaiveDate,
    pub quantity: Decimal,
    pub average_price: Decimal,
    pub brokerage: Decimal,
    pub gst_on_brokerage: Decimal,
    pub currency: String,
    pub fx_rate: Decimal,
    /// Deliberate transaction-date spot-rate override: when set it wins over
    /// the ATO monthly rate (see `infra::fx::FxOverride`).
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

impl sqlx::FromRow<'_, SqliteRow> for ParcelRow {
    fn from_row(row: &SqliteRow) -> Result<Self, sqlx::Error> {
        Ok(ParcelRow {
            id: row.try_get("id")?,
            listing_id: row.try_get("listing_id")?,
            holding_account_id: row.try_get("holding_account_id")?,
            date: row.try_get("date")?,
            quantity: row_dec(row, "quantity")?,
            average_price: row_dec(row, "average_price")?,
            brokerage: row_dec(row, "brokerage")?,
            gst_on_brokerage: row_dec(row, "gst_on_brokerage")?,
            currency: row.try_get("currency")?,
            fx_rate: row_dec(row, "fx_rate")?,
            spot_fx_rate: crate::infra::decimal::row_opt_dec(row, "spot_fx_rate")?,
            deemed_acquisition_date: row.try_get("deemed_acquisition_date")?,
        })
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
    let initial_cost =
        parcel.average_price * parcel.quantity + parcel.brokerage + parcel.gst_on_brokerage;
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
    /// the breakdown stays internally consistent.
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
