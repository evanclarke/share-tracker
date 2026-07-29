use crate::domain::cgt_discount;
use crate::domain::cost_base::{self, ParcelRow};
use crate::entities::corporate_action::{RocEvent, SplitEvent};
use crate::infra::decimal::{Money, OptMoney};
use crate::infra::fx::{FxOverride, FxRates};
use crate::infra::http::ApiError;
use axum::{Json, Router, extract::State, routing::get};
use chrono::NaiveDate;
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;
use std::collections::HashMap;

/// Where a realised disposal came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum DisposalSource {
    /// An ordinary Sell trade (incl. buy-back and worthless-shares closing
    /// Sells) — `sale_trade_id` is the trade id.
    Sell,
    /// A rights sale/lapse (`entities::rights_sale` — a disposal of the
    /// rights themselves; the share holding is untouched) — `sale_trade_id`
    /// is the `rights_sales` row id.
    RightsSale,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RealisedGainLoss {
    pub source: DisposalSource,
    /// The disposal's row id in its `source`'s table (`trades` for `Sell`,
    /// `rights_sales` for `RightsSale`).
    pub sale_trade_id: i64,
    pub listing_id: i64,
    /// The holding account the Sell happened in (the same taxpayer either
    /// way — totals are unchanged; rows identify the account).
    pub holding_account_id: i64,
    pub sale_date: NaiveDate,
    /// Net proceeds from the allocated portion (sale price × qty − pro-rated
    /// brokerage), converted to AUD at the sale's ATO FX rate.
    pub proceeds: Decimal,
    /// Adjusted cost base of the sold parcels (AMIT-reduced, pro-rated to allocated
    /// qty), converted to AUD at the purchase's ATO FX rate.
    pub cost_base: Decimal,
    /// proceeds − cost_base in AUD (positive = gain, negative = loss).
    pub capital_gain_loss: Decimal,
    /// Portion of the capital gain from parcels held strictly more than 12 months
    /// (eligible for the 50% CGT discount). Always ≥ 0; losses are excluded.
    pub discount_eligible_gain: Decimal,
    /// Gross positive gains from parcels held 12 months or less — the "other"
    /// (non-discountable) method. Always ≥ 0; losses are excluded.
    pub non_discountable_gain: Decimal,
    /// Total capital losses from this sale's allocations (those whose proceeds fell
    /// below their cost base), as a positive amount. Always ≥ 0.
    pub capital_loss: Decimal,
    /// The individual parcel allocations behind this disposal's totals — the
    /// same per-allocation figures the totals above are summed from, so a UI
    /// can drill into which parcel contributed what. Sorted by acquisition
    /// date then purchase trade id.
    pub parcels: Vec<ParcelDetail>,
}

/// One parcel's share of a realised disposal — the per-allocation figures
/// `compute_realised_gains` sums into the disposal's totals, surfaced so a UI
/// can show which parcel contributed what. Mirrors
/// `parcel_optimiser::HypotheticalAllocation`'s field set (the hypothetical
/// equivalent), but for an actually-recorded allocation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ParcelDetail {
    /// The Buy/DRP trade (or, for a rights sale, the parcel that earned the
    /// rights) this allocation drew from.
    pub purchase_trade_id: i64,
    /// Original/deemed acquisition date — the discount-clock anchor
    /// (`ParcelRow::acquired()`).
    pub acquisition_date: NaiveDate,
    /// Units allocated from this parcel (sale-date unit basis).
    pub units: Decimal,
    /// Adjusted cost base of the allocated units, AUD.
    pub cost_base: Decimal,
    /// This allocation's share of the proceeds, AUD.
    pub proceeds: Decimal,
    /// proceeds − cost_base (positive = gain, negative = loss).
    pub capital_gain_loss: Decimal,
    /// Held strictly more than 12 months at the sale date (50% CGT discount).
    pub discount_eligible: bool,
}

// Per-sale identity: capital_gain_loss == discount_eligible_gain
//                                       + non_discountable_gain − capital_loss.

pub fn router() -> Router<SqlitePool> {
    Router::new().route("/portfolio/realised-gains", get(realised_gains_handler))
}

/// A realisable Sell as the report reads it from `trades`. Each trade carries
/// its own currency and manual `fx_rate` fallback so its amounts convert to
/// AUD via the ATO reference rate (see `infra::fx`); proceeds and cost base
/// convert independently — a buy and the sell that closes it may settle in
/// different months at different rates — so totals are never aggregated
/// across mixed currencies.
#[derive(sqlx::FromRow)]
struct SellInfo {
    id: i64,
    listing_id: i64,
    holding_account_id: i64,
    date: NaiveDate,
    #[sqlx(try_from = "Money")]
    quantity: Decimal,
    #[sqlx(try_from = "Money")]
    average_price: Decimal,
    #[sqlx(try_from = "Money")]
    brokerage: Decimal,
    #[sqlx(try_from = "Money")]
    gst_on_brokerage: Decimal,
    currency: String,
    #[sqlx(try_from = "Money")]
    fx_rate: Decimal,
    #[sqlx(try_from = "OptMoney")]
    spot_fx_rate: Option<Decimal>,
    /// The cash terms of the scrip-for-scrip action this Sell closes, joined
    /// in — all four are `None` for an ordinary Sell, and all four are set on
    /// a partial-rollover closing Sell (see [`SellInfo::scrip_cash_apportionment`]).
    #[sqlx(try_from = "OptMoney")]
    scrip_cash_per_unit: Option<Decimal>,
    #[sqlx(try_from = "OptMoney")]
    scrip_market_value: Option<Decimal>,
    #[sqlx(try_from = "OptMoney")]
    scrip_new_units: Option<Decimal>,
    #[sqlx(try_from = "OptMoney")]
    scrip_old_units: Option<Decimal>,
}

impl SellInfo {
    /// The sale's per-record FX rate: its deliberate spot override when set,
    /// else its `fx_rate` fallback (see `infra::fx::FxOverride`).
    fn fx_override(&self) -> FxOverride {
        FxOverride::from_trade(self.fx_rate, self.spot_fx_rate)
    }

    /// Set on a partial-rollover scrip-for-scrip closing Sell (its action has
    /// a cash component): the `(numerator, denominator)` of the cash side's
    /// market-value share of each allocated parcel's reduced cost base —
    /// cash×old / (cash×old + mv×new), per
    /// `docs/ato/takeovers-and-scrip-for-scrip.md` Example 27. The scrip
    /// side's share rolled over into the replacement parcels at exchange
    /// time, so only this share is realised against the cash proceeds. Kept
    /// as a pair and multiplied before dividing so exact fractions (e.g.
    /// Gunther's 1/3) don't round twice. `None` for ordinary Sells.
    ///
    /// A cash component with any of its companion terms missing is a corrupt
    /// action row — `corporate_actions` CHECKs `scrip_market_value` and
    /// `scrip_cash_per_unit` present together, and a ScripForScrip action
    /// always carries its exchange ratio — so it is an error, never a
    /// silently un-apportioned (and so overstated) cost base.
    fn scrip_cash_apportionment(&self) -> Result<Option<(Decimal, Decimal)>, sqlx::Error> {
        let Some(cash) = self.scrip_cash_per_unit else {
            return Ok(None);
        };
        let missing = |col: &str| {
            sqlx::Error::Decode(
                format!(
                    "scrip action of sell {} has a cash component but no {col}",
                    self.id
                )
                .into(),
            )
        };
        let mv = self
            .scrip_market_value
            .ok_or_else(|| missing("scrip_market_value"))?;
        let new = self
            .scrip_new_units
            .ok_or_else(|| missing("scrip_new_units"))?;
        let old = self
            .scrip_old_units
            .ok_or_else(|| missing("scrip_old_units"))?;
        Ok(Some((cash * old, cash * old + mv * new)))
    }
}

/// One parcel-allocation row: `quantity_allocated` units of the sale (in the
/// sale date's unit basis) came from the purchase parcel.
#[derive(sqlx::FromRow)]
struct Allocation {
    sale_trade_id: i64,
    purchase_trade_id: i64,
    #[sqlx(try_from = "Money")]
    quantity_allocated: Decimal,
}

/// A rights sale/lapse as the report reads it (`rights_sales` joined to its
/// RightsIssue action for the listing and currency): a disposal of the rights
/// themselves (see `entities::rights_sale`). Proceeds are
/// `units × proceeds_per_right`; the cost base is `rights_cost` (nil for
/// rights issued free), apportioned over the anchoring allocations. Both
/// convert to AUD at the sale month's ATO rate (manual `fx_rate` fallback) —
/// the rights' own purchase date isn't tracked, so the cost leg uses the sale
/// date too.
#[derive(sqlx::FromRow)]
struct RightsSaleInfo {
    id: i64,
    listing_id: i64,
    holding_account_id: i64,
    date: NaiveDate,
    #[sqlx(try_from = "Money")]
    units: Decimal,
    #[sqlx(try_from = "Money")]
    proceeds_per_right: Decimal,
    #[sqlx(try_from = "Money")]
    rights_cost: Decimal,
    currency: String,
    #[sqlx(try_from = "Money")]
    fx_rate: Decimal,
}

/// One rights-sale anchoring row: `units` of the sale's rights were earned by
/// (and take the acquisition date of) the purchase parcel. Consumes nothing.
#[derive(sqlx::FromRow)]
struct RightsAllocation {
    rights_sale_id: i64,
    purchase_trade_id: i64,
    #[sqlx(try_from = "Money")]
    units: Decimal,
}

/// Everything the realised-gains computation consumes, read in one go by
/// [`load_report_data`] so [`compute_realised_gains`] is a pure function over
/// in-memory data.
#[derive(Default)]
struct ReportData {
    sells: HashMap<i64, SellInfo>,
    buys: HashMap<i64, ParcelRow>,
    allocations: Vec<Allocation>,
    /// Rights sales/lapses (disposals of the rights themselves) and their
    /// parcel anchorings, surfaced as `source = RightsSale` rows.
    rights_sales: Vec<RightsSaleInfo>,
    rights_allocations: Vec<RightsAllocation>,
    /// Cumulative AMIT cost-base reduction per purchase parcel.
    cba_reduction: HashMap<i64, Decimal>,
    /// Return-of-capital payments (CGT event G1) per listing.
    roc_events: HashMap<i64, Vec<RocEvent>>,
    /// Share splits/consolidations per listing (quantity re-basing).
    split_events: HashMap<i64, Vec<SplitEvent>>,
    /// Every imported ATO FX rate, so per-allocation conversions are map
    /// lookups instead of one DB round-trip each.
    fx: FxRates,
}

/// Reads the report's inputs on the caller's connection. Callers run this
/// inside one read transaction (`db_realised_gains` opens its own; the
/// net-capital-gain report passes its wider snapshot through
/// [`db_realised_gains_on`]) so the whole report sees a single consistent
/// snapshot — an interleaved write can never yield an allocation whose sale
/// or parcel is missing from the same read.
async fn load_report_data(conn: &mut sqlx::SqliteConnection) -> Result<ReportData, sqlx::Error> {
    // An all-scrip scrip-for-scrip exchange or demerger closing Sell
    // (scrip_action_id / demerger_action_id set) is not a realised gain or
    // loss: the rollover disregards the gain on the original shares
    // (docs/ato/takeovers-and-scrip-for-scrip.md, docs/ato/demergers.md), and its
    // zero proceeds must never surface as a capital loss. A *partial-rollover*
    // scrip closing Sell (its action has a cash component) IS included: the
    // rollover applies only to the scrip portion, so the cash proceeds are
    // assessed now against the cash-apportioned cost-base share (Example 27)
    // — the join carries the action's cash terms in. A holding-account
    // transfer-out Sell (transfer_id set) is not even a disposal — the same
    // beneficial owner holds the shares before and after, so it is no CGT
    // event at all. Excluded sells' allocations are skipped with them (the
    // sale id is absent from `sells`).
    let sells: Vec<SellInfo> = sqlx::query_as(
        "SELECT t.id, t.listing_id, t.holding_account_id, t.date, t.quantity, t.average_price, \
         t.brokerage, t.gst_on_brokerage, t.currency, t.fx_rate, t.spot_fx_rate, \
         ca.scrip_cash_per_unit, ca.scrip_market_value, ca.scrip_new_units, ca.scrip_old_units \
         FROM trades t LEFT JOIN corporate_actions ca ON ca.id = t.scrip_action_id \
         WHERE t.trade_type = 'Sell' \
           AND (t.scrip_action_id IS NULL OR ca.scrip_cash_per_unit IS NOT NULL) \
           AND t.demerger_action_id IS NULL AND t.transfer_id IS NULL",
    )
    .fetch_all(&mut *conn)
    .await?;

    let rights_sales: Vec<RightsSaleInfo> = sqlx::query_as(
        "SELECT rs.id, ca.listing_id, rs.holding_account_id, rs.date, rs.units, \
                rs.proceeds_per_right, rs.rights_cost, ca.currency, rs.fx_rate \
         FROM rights_sales rs JOIN corporate_actions ca ON ca.id = rs.rights_action_id",
    )
    .fetch_all(&mut *conn)
    .await?;

    if sells.is_empty() && rights_sales.is_empty() {
        // Nothing to report.
        return Ok(ReportData::default());
    }

    let rights_allocations: Vec<RightsAllocation> = sqlx::query_as(
        "SELECT rights_sale_id, purchase_trade_id, units FROM rights_sale_allocations \
         ORDER BY id",
    )
    .fetch_all(&mut *conn)
    .await?;

    let buys: Vec<ParcelRow> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT {} FROM trades WHERE trade_type IN ('Buy', 'DRP')",
        ParcelRow::COLUMNS
    )))
    .fetch_all(&mut *conn)
    .await?;

    let allocations: Vec<Allocation> = sqlx::query_as(
        "SELECT sale_trade_id, purchase_trade_id, quantity_allocated FROM parcel_allocations",
    )
    .fetch_all(&mut *conn)
    .await?;

    let cba_reduction =
        crate::entities::amit_adjustment::db_cost_base_reductions(&mut *conn).await?;
    let roc_events =
        crate::entities::corporate_action::db_return_of_capital_events(&mut *conn).await?;
    // share splits/consolidations per listing (quantity re-basing)
    let split_events = crate::entities::corporate_action::db_share_split_events(&mut *conn).await?;
    let fx = FxRates::load(&mut *conn).await?;

    Ok(ReportData {
        sells: sells.into_iter().map(|s| (s.id, s)).collect(),
        buys: buys.into_iter().map(|b| (b.id, b)).collect(),
        allocations,
        rights_sales,
        rights_allocations,
        cba_reduction,
        roc_events,
        split_events,
        fx,
    })
}

pub async fn db_realised_gains(pool: &SqlitePool) -> Result<Vec<RealisedGainLoss>, sqlx::Error> {
    let mut tx = pool.begin().await?;
    let data = load_report_data(&mut tx).await?;
    tx.commit().await?;
    compute_realised_gains(&data)
}

/// The same report read on the caller's own connection, for reports (the
/// net-capital-gain report and its what-if) that fold the realised rows into
/// a wider single-snapshot read transaction.
pub async fn db_realised_gains_on(
    conn: &mut sqlx::SqliteConnection,
) -> Result<Vec<RealisedGainLoss>, sqlx::Error> {
    compute_realised_gains(&load_report_data(conn).await?)
}

/// The gain/loss computation: a pure function over [`ReportData`], so the
/// arithmetic is unit-testable without a database.
fn compute_realised_gains(data: &ReportData) -> Result<Vec<RealisedGainLoss>, sqlx::Error> {
    let mut sale_proceeds: HashMap<i64, Decimal> = HashMap::new();
    let mut sale_cost_base: HashMap<i64, Decimal> = HashMap::new();
    let mut sale_discount_gain: HashMap<i64, Decimal> = HashMap::new();
    let mut sale_non_discount_gain: HashMap<i64, Decimal> = HashMap::new();
    let mut sale_loss: HashMap<i64, Decimal> = HashMap::new();
    // Per-sale parcel breakdown: the same per-allocation figures computed
    // below, kept instead of only being summed into the totals above, so the
    // report can surface which parcel contributed what.
    let mut sale_parcels: HashMap<i64, Vec<ParcelDetail>> = HashMap::new();
    // Running (quantity, sale-costs share) already handed to earlier
    // allocations of each sale, for the cumulative-difference pro-rating
    // below.
    let mut sale_costs_assigned: HashMap<i64, (Decimal, Decimal)> = HashMap::new();

    for alloc in &data.allocations {
        let sale_id = alloc.sale_trade_id;
        let qty_alloc = alloc.quantity_allocated;

        let Some(sale) = data.sells.get(&sale_id) else {
            continue;
        };
        let Some(buy) = data.buys.get(&alloc.purchase_trade_id) else {
            continue;
        };

        // Proceeds for this allocation: pro-rate sale brokerage+gst by
        // qty_alloc / sale_qty, as a cumulative difference — each share is the
        // cumulative entitlement minus what earlier allocations already took —
        // so the shares sum exactly to the total: any division remainder
        // (e.g. a third of $10) lands on the sale's last allocation instead
        // of each share rounding independently.
        let sale_costs = sale.brokerage + sale.gst_on_brokerage;
        let alloc_proceeds = if sale.quantity > Decimal::ZERO {
            let (qty_so_far, costs_so_far) = sale_costs_assigned
                .entry(sale_id)
                .or_insert((Decimal::ZERO, Decimal::ZERO));
            *qty_so_far += qty_alloc;
            let alloc_costs = sale_costs * *qty_so_far / sale.quantity - *costs_so_far;
            *costs_so_far += alloc_costs;
            sale.average_price * qty_alloc - alloc_costs
        } else {
            Decimal::ZERO
        };
        // Convert to AUD at the sale's rate (the sale's deliberate spot
        // override when set, else the ATO rate for the sale month, else the
        // sale's manual fx_rate fallback) before aggregating.
        let alloc_proceeds = data.fx.to_aud(
            alloc_proceeds,
            &sale.currency,
            sale.date,
            sale.fx_override(),
        )?;

        // The allocated quantity is in the *sale date's* unit basis; the
        // purchase parcel's quantity and per-unit cost are as transacted. A
        // share split/consolidation between them (TD 2000/10) re-bases the
        // allocation back to as-acquired units so the cost-base pro-rating
        // spreads the unchanged total over the converted unit count.
        let splits = data
            .split_events
            .get(&buy.listing_id)
            .map_or(&[][..], |v| v);
        let qty_alloc_acquired = crate::entities::corporate_action::as_acquired_quantity(
            qty_alloc, splits, buy.date, sale.date,
        );

        // Adjusted cost base of the allocated portion via the shared pipeline
        // (`domain::cost_base`), converted to AUD at the (possibly deemed)
        // acquisition month. `up_to` is the sale date: return-of-capital
        // payments after the sale don't touch these units — they were no
        // longer held.
        let alloc_cost = cost_base::adjusted_cost_base(
            &buy.parcel(),
            qty_alloc_acquired,
            *data
                .cba_reduction
                .get(&alloc.purchase_trade_id)
                .unwrap_or(&Decimal::ZERO),
            data.roc_events.get(&buy.listing_id).map_or(&[][..], |v| v),
            splits,
            Some(sale.date),
        )?
        .into_aud_with(&data.fx, &buy.currency, buy.acquired(), buy.fx_override())?
        .adjusted;

        // A partial-rollover scrip closing Sell realises only the cash
        // side's market-value share of the parcel's reduced cost base; the
        // scrip side's share rolled over into the replacement parcels (the
        // exchange carried exactly the complement, so the two sum to the
        // full reduced cost base).
        let alloc_cost = match sale.scrip_cash_apportionment()? {
            Some((num, den)) => alloc_cost * num / den,
            None => alloc_cost,
        };

        let alloc_gain = alloc_proceeds - alloc_cost;

        *sale_proceeds.entry(sale_id).or_insert(Decimal::ZERO) += alloc_proceeds;
        *sale_cost_base.entry(sale_id).or_insert(Decimal::ZERO) += alloc_cost;

        // Classify each allocation's gain/loss for CGT: a gain from a parcel
        // held long enough (`domain::cgt_discount`, from the possibly deemed
        // acquisition date) is discount-eligible; a gain from a parcel held ≤
        // 12 months is non-discountable ("other" method); a negative result is
        // a capital loss (recorded as a positive amount). The net-capital-gain
        // report nets these buckets across sales and AMMA gains.
        let discount_eligible = cgt_discount::discount_eligible(buy.acquired(), sale.date);
        if alloc_gain > Decimal::ZERO {
            if discount_eligible {
                *sale_discount_gain.entry(sale_id).or_insert(Decimal::ZERO) += alloc_gain;
            } else {
                *sale_non_discount_gain
                    .entry(sale_id)
                    .or_insert(Decimal::ZERO) += alloc_gain;
            }
        } else if alloc_gain < Decimal::ZERO {
            *sale_loss.entry(sale_id).or_insert(Decimal::ZERO) += -alloc_gain;
        }

        sale_parcels.entry(sale_id).or_default().push(ParcelDetail {
            purchase_trade_id: alloc.purchase_trade_id,
            acquisition_date: buy.acquired(),
            units: qty_alloc,
            cost_base: alloc_cost,
            proceeds: alloc_proceeds,
            capital_gain_loss: alloc_gain,
            discount_eligible,
        });
    }

    let mut result: Vec<RealisedGainLoss> = sale_proceeds
        .keys()
        .filter_map(|&sale_id| {
            let sale = data.sells.get(&sale_id)?;
            let proceeds = sale_proceeds[&sale_id];
            let cost_base = sale_cost_base[&sale_id];
            let discount_gain = sale_discount_gain
                .get(&sale_id)
                .copied()
                .unwrap_or(Decimal::ZERO);
            let non_discount_gain = sale_non_discount_gain
                .get(&sale_id)
                .copied()
                .unwrap_or(Decimal::ZERO);
            let loss = sale_loss.get(&sale_id).copied().unwrap_or(Decimal::ZERO);
            let mut parcels = sale_parcels.get(&sale_id).cloned().unwrap_or_default();
            parcels.sort_by(|a, b| {
                a.acquisition_date
                    .cmp(&b.acquisition_date)
                    .then(a.purchase_trade_id.cmp(&b.purchase_trade_id))
            });
            Some(RealisedGainLoss {
                source: DisposalSource::Sell,
                sale_trade_id: sale_id,
                listing_id: sale.listing_id,
                holding_account_id: sale.holding_account_id,
                sale_date: sale.date,
                proceeds,
                cost_base,
                capital_gain_loss: proceeds - cost_base,
                discount_eligible_gain: discount_gain,
                non_discountable_gain: non_discount_gain,
                capital_loss: loss,
                parcels,
            })
        })
        .collect();

    // Rights sales/lapses: one row per sale, each anchoring allocation
    // classified against its parcel's (possibly deemed) acquisition date —
    // free rights are taken to have been acquired with the original shares
    // (docs/ato/rights-issues.md Example 39, docs/ato/retail-premiums.md), so
    // the 12-month discount clock runs from the parcel, unlike an exercise.
    for sale in &data.rights_sales {
        let mut proceeds = Decimal::ZERO;
        let mut cost_base = Decimal::ZERO;
        let mut discount_gain = Decimal::ZERO;
        let mut non_discount_gain = Decimal::ZERO;
        let mut loss = Decimal::ZERO;
        let mut parcels: Vec<ParcelDetail> = Vec::new();
        // rights_cost apportioned by units as a cumulative difference, so the
        // per-allocation shares sum exactly to the total (any division
        // remainder lands on the last allocation).
        let mut units_so_far = Decimal::ZERO;
        let mut cost_so_far = Decimal::ZERO;
        for alloc in data
            .rights_allocations
            .iter()
            .filter(|a| a.rights_sale_id == sale.id)
        {
            let Some(buy) = data.buys.get(&alloc.purchase_trade_id) else {
                continue;
            };
            units_so_far += alloc.units;
            let alloc_cost = if sale.units > Decimal::ZERO {
                sale.rights_cost * units_so_far / sale.units - cost_so_far
            } else {
                Decimal::ZERO
            };
            cost_so_far += alloc_cost;

            // Both legs convert at the sale month (manual fx_rate fallback):
            // the rights' own purchase date isn't tracked, so the cost leg
            // has no earlier anchor.
            let alloc_proceeds = data.fx.to_aud(
                sale.proceeds_per_right * alloc.units,
                &sale.currency,
                sale.date,
                FxOverride::Fallback(sale.fx_rate),
            )?;
            let alloc_cost = data.fx.to_aud(
                alloc_cost,
                &sale.currency,
                sale.date,
                FxOverride::Fallback(sale.fx_rate),
            )?;

            let alloc_gain = alloc_proceeds - alloc_cost;
            proceeds += alloc_proceeds;
            cost_base += alloc_cost;
            let discount_eligible = cgt_discount::discount_eligible(buy.acquired(), sale.date);
            if alloc_gain > Decimal::ZERO {
                if discount_eligible {
                    discount_gain += alloc_gain;
                } else {
                    non_discount_gain += alloc_gain;
                }
            } else if alloc_gain < Decimal::ZERO {
                loss += -alloc_gain;
            }
            parcels.push(ParcelDetail {
                purchase_trade_id: alloc.purchase_trade_id,
                acquisition_date: buy.acquired(),
                units: alloc.units,
                cost_base: alloc_cost,
                proceeds: alloc_proceeds,
                capital_gain_loss: alloc_gain,
                discount_eligible,
            });
        }
        parcels.sort_by(|a, b| {
            a.acquisition_date
                .cmp(&b.acquisition_date)
                .then(a.purchase_trade_id.cmp(&b.purchase_trade_id))
        });
        result.push(RealisedGainLoss {
            source: DisposalSource::RightsSale,
            sale_trade_id: sale.id,
            listing_id: sale.listing_id,
            holding_account_id: sale.holding_account_id,
            sale_date: sale.date,
            proceeds,
            cost_base,
            capital_gain_loss: proceeds - cost_base,
            discount_eligible_gain: discount_gain,
            non_discountable_gain: non_discount_gain,
            capital_loss: loss,
            parcels,
        });
    }

    result.sort_by(|a, b| {
        a.sale_date
            .cmp(&b.sale_date)
            .then(a.source.cmp(&b.source))
            .then(a.sale_trade_id.cmp(&b.sale_trade_id))
    });
    Ok(result)
}

async fn realised_gains_handler(
    State(pool): State<SqlitePool>,
) -> Result<Json<Vec<RealisedGainLoss>>, ApiError> {
    db_realised_gains(&pool)
        .await
        .map(Json)
        .map_err(ApiError::from)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::entities::{corporate_action, listing, rba_fx_rate, trade};
    use crate::test_support::{self, allocate, test_pool};
    use axum::http::StatusCode;
    use axum::{body::Body, http::Request};
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    async fn insert_listing(pool: &SqlitePool, id: i64, ticker: &str) {
        test_support::listing(id)
            .ticker(ticker)
            .name(ticker)
            .insert(pool)
            .await;
    }

    async fn insert_buy(
        pool: &SqlitePool,
        id: i64,
        listing_id: i64,
        date: NaiveDate,
        qty: Decimal,
        price: Decimal,
    ) {
        test_support::buy(id, listing_id)
            .date(date)
            .qty(qty)
            .price(price)
            .insert(pool)
            .await;
    }

    async fn insert_sell(
        pool: &SqlitePool,
        id: i64,
        listing_id: i64,
        date: NaiveDate,
        qty: Decimal,
        price: Decimal,
    ) {
        test_support::sell(id, listing_id)
            .date(date)
            .qty(qty)
            .price(price)
            .insert(pool)
            .await;
    }

    // Pure-computation tests: `compute_realised_gains` over in-memory
    // `ReportData` — no pool. The arithmetic seam the load/compute split
    // exists for.

    fn mem_sell(id: i64, date: NaiveDate, qty: i64, price: i64, currency: &str) -> SellInfo {
        SellInfo {
            id,
            listing_id: 1,
            holding_account_id: 1,
            date,
            quantity: Decimal::from(qty),
            average_price: Decimal::from(price),
            brokerage: Decimal::ZERO,
            gst_on_brokerage: Decimal::ZERO,
            currency: currency.to_string(),
            fx_rate: Decimal::ONE,
            spot_fx_rate: None,
            scrip_cash_per_unit: None,
            scrip_market_value: None,
            scrip_new_units: None,
            scrip_old_units: None,
        }
    }

    fn mem_buy(id: i64, date: NaiveDate, qty: i64, price: i64, currency: &str) -> ParcelRow {
        ParcelRow {
            id,
            listing_id: 1,
            holding_account_id: 1,
            date,
            quantity: Decimal::from(qty),
            average_price: Decimal::from(price),
            brokerage: Decimal::ZERO,
            gst_on_brokerage: Decimal::ZERO,
            currency: currency.to_string(),
            fx_rate: Decimal::ONE,
            spot_fx_rate: None,
            deemed_acquisition_date: None,
        }
    }

    fn mem_alloc(sale: i64, buy: i64, qty: i64) -> Allocation {
        Allocation {
            sale_trade_id: sale,
            purchase_trade_id: buy,
            quantity_allocated: Decimal::from(qty),
        }
    }

    #[test]
    fn pure_mixed_eligibility_and_identity_without_a_pool() {
        let d = |y, m, day| NaiveDate::from_ymd_opt(y, m, day).unwrap();
        // Old parcel (>12mo, gain 500), new parcel (≤12mo, gain 250) — same
        // facts as `db_two_parcels_mixed_eligibility`, straight from memory.
        let data = ReportData {
            sells: [(3, mem_sell(3, d(2025, 6, 1), 150, 15, "AUD"))].into(),
            buys: [
                (1, mem_buy(1, d(2023, 1, 1), 100, 10, "AUD")),
                (2, mem_buy(2, d(2025, 1, 1), 50, 10, "AUD")),
            ]
            .into(),
            allocations: vec![mem_alloc(3, 1, 100), mem_alloc(3, 2, 50)],
            ..ReportData::default()
        };

        let result = compute_realised_gains(&data).unwrap();
        assert_eq!(result.len(), 1);
        let r = &result[0];
        assert_eq!(r.proceeds, Decimal::from(2250));
        assert_eq!(r.cost_base, Decimal::from(1500));
        assert_eq!(r.capital_gain_loss, Decimal::from(750));
        assert_eq!(r.discount_eligible_gain, Decimal::from(500));
        assert_eq!(r.non_discountable_gain, Decimal::from(250));
        assert_eq!(r.capital_loss, Decimal::ZERO);
        assert_eq!(
            r.capital_gain_loss,
            r.discount_eligible_gain + r.non_discountable_gain - r.capital_loss
        );
    }

    #[test]
    fn pure_fx_conversion_uses_the_preloaded_rates() {
        let d = |y, m, day| NaiveDate::from_ymd_opt(y, m, day).unwrap();
        // USD buy at 0.50 (cost 1000 USD → 2000 AUD), USD sell at 0.60
        // (proceeds 1500 USD → 2500 AUD): gain 500 AUD, from the map alone.
        let data = ReportData {
            sells: [(2, mem_sell(2, d(2025, 6, 1), 100, 15, "USD"))].into(),
            buys: [(1, mem_buy(1, d(2024, 1, 15), 100, 10, "USD"))].into(),
            allocations: vec![mem_alloc(2, 1, 100)],
            fx: crate::infra::fx::FxRates::from_rates([
                ("USD", "2024-01", "0.50".parse().unwrap()),
                ("USD", "2025-06", "0.60".parse().unwrap()),
            ]),
            ..ReportData::default()
        };

        let result = compute_realised_gains(&data).unwrap();
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].cost_base, Decimal::from(2000));
        assert_eq!(result[0].proceeds, Decimal::from(2500));
        assert_eq!(result[0].capital_gain_loss, Decimal::from(500));
    }

    #[test]
    fn pure_manual_override_fallback_when_no_ato_rate() {
        let d = |y, m, day| NaiveDate::from_ymd_opt(y, m, day).unwrap();
        // With no ATO rate loaded for either month, both legs must fall back
        // to their trade's manual fx_rate (never an unconverted or zeroed
        // figure — the missing-rate-and-no-override case errors, asserted in
        // `infra::fx`'s tests).
        let mut sell = mem_sell(2, d(2025, 6, 1), 100, 15, "USD");
        sell.fx_rate = "0.60".parse().unwrap();
        let mut buy = mem_buy(1, d(2024, 1, 15), 100, 10, "USD");
        buy.fx_rate = "0.50".parse().unwrap();
        let data = ReportData {
            sells: [(2, sell)].into(),
            buys: [(1, buy)].into(),
            allocations: vec![mem_alloc(2, 1, 100)],
            ..ReportData::default()
        };
        // No ATO rates loaded: both legs use the trades' manual overrides.
        let result = compute_realised_gains(&data).unwrap();
        assert_eq!(result[0].cost_base, Decimal::from(2000));
        assert_eq!(result[0].proceeds, Decimal::from(2500));
    }

    #[test]
    fn pure_spot_override_wins_over_preloaded_rates() {
        let d = |y, m, day| NaiveDate::from_ymd_opt(y, m, day).unwrap();
        // Both legs carry deliberate spot rates (QC 18020); the loaded
        // monthly rates would give different figures and must lose: cost
        // 1000 USD / 0.40 = 2500 AUD, proceeds 1500 USD / 0.75 = 2000 AUD.
        let mut sell = mem_sell(2, d(2025, 6, 1), 100, 15, "USD");
        sell.spot_fx_rate = Some("0.75".parse().unwrap());
        let mut buy = mem_buy(1, d(2024, 1, 15), 100, 10, "USD");
        buy.spot_fx_rate = Some("0.40".parse().unwrap());
        let data = ReportData {
            sells: [(2, sell)].into(),
            buys: [(1, buy)].into(),
            allocations: vec![mem_alloc(2, 1, 100)],
            fx: crate::infra::fx::FxRates::from_rates([
                ("USD", "2024-01", "0.50".parse().unwrap()),
                ("USD", "2025-06", "0.60".parse().unwrap()),
            ]),
            ..ReportData::default()
        };

        let result = compute_realised_gains(&data).unwrap();
        assert_eq!(result[0].cost_base, Decimal::from(2500));
        assert_eq!(result[0].proceeds, Decimal::from(2000));
        assert_eq!(result[0].capital_gain_loss, Decimal::from(-500));
    }

    #[test]
    fn pure_brokerage_shares_sum_exactly_across_allocations() {
        let d = |y, m, day| NaiveDate::from_ymd_opt(y, m, day).unwrap();
        // $10 brokerage over three 1-unit allocations of a 3-unit sale: each
        // independent share would be the non-terminating 10/3, so the three
        // would sum to 9.999…, not 10. The cumulative-difference pro-rating
        // hands the remainder to the last allocation, so total proceeds come
        // out exactly 3 × $4 − $10 = $2. (The $4 price keeps each
        // `price × qty − share` subtraction inside Decimal's 28-digit
        // mantissa; a larger price would re-round there and mask what this
        // test asserts — the shares themselves summing exactly.)
        let mut sell = mem_sell(4, d(2025, 6, 1), 3, 4, "AUD");
        sell.brokerage = Decimal::from(10);
        let data = ReportData {
            sells: [(4, sell)].into(),
            buys: [
                (1, mem_buy(1, d(2024, 1, 1), 1, 1, "AUD")),
                (2, mem_buy(2, d(2024, 2, 1), 1, 1, "AUD")),
                (3, mem_buy(3, d(2024, 3, 1), 1, 1, "AUD")),
            ]
            .into(),
            allocations: vec![mem_alloc(4, 1, 1), mem_alloc(4, 2, 1), mem_alloc(4, 3, 1)],
            ..ReportData::default()
        };

        let result = compute_realised_gains(&data).unwrap();
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].proceeds, Decimal::from(2));
        assert_eq!(result[0].cost_base, Decimal::from(3));
        assert_eq!(result[0].capital_gain_loss, Decimal::from(-1));
        // Each allocation's net (4 − share) fell below its $1 cost: the three
        // per-allocation losses likewise sum exactly to the $1 total.
        assert_eq!(result[0].capital_loss, Decimal::ONE);
    }

    // DB-level tests

    #[tokio::test]
    async fn db_no_sells_returns_empty() {
        let pool = test_pool().await;
        let result = db_realised_gains(&pool).await.unwrap();
        assert!(result.is_empty());
    }

    #[tokio::test]
    async fn db_sell_without_allocations_returns_empty() {
        let pool = test_pool().await;
        let buy_date = NaiveDate::from_ymd_opt(2024, 1, 1).unwrap();
        let sell_date = NaiveDate::from_ymd_opt(2025, 3, 1).unwrap();
        insert_listing(&pool, 1, "VAS").await;
        insert_buy(&pool, 1, 1, buy_date, Decimal::from(100), Decimal::from(10)).await;
        insert_sell(
            &pool,
            2,
            1,
            sell_date,
            Decimal::from(100),
            Decimal::from(15),
        )
        .await;
        // no allocations
        let result = db_realised_gains(&pool).await.unwrap();
        assert!(result.is_empty());
    }

    #[tokio::test]
    async fn db_basic_gain() {
        let pool = test_pool().await;
        // buy 100 @ $10, sell 100 @ $15 — zero brokerage
        let buy_date = NaiveDate::from_ymd_opt(2024, 1, 1).unwrap();
        let sell_date = NaiveDate::from_ymd_opt(2024, 6, 1).unwrap();
        insert_listing(&pool, 1, "VAS").await;
        insert_buy(&pool, 1, 1, buy_date, Decimal::from(100), Decimal::from(10)).await;
        insert_sell(
            &pool,
            2,
            1,
            sell_date,
            Decimal::from(100),
            Decimal::from(15),
        )
        .await;
        allocate(&pool, 1, 2, 1, Decimal::from(100)).await;

        let result = db_realised_gains(&pool).await.unwrap();
        assert_eq!(result.len(), 1);
        let r = &result[0];
        assert_eq!(r.sale_trade_id, 2);
        assert_eq!(r.listing_id, 1);
        assert_eq!(r.sale_date, sell_date);
        assert_eq!(r.proceeds, Decimal::from(1500));
        assert_eq!(r.cost_base, Decimal::from(1000));
        assert_eq!(r.capital_gain_loss, Decimal::from(500));
    }

    #[tokio::test]
    async fn db_basic_loss() {
        let pool = test_pool().await;
        // buy 100 @ $15, sell 100 @ $10 — zero brokerage → loss of 500
        let buy_date = NaiveDate::from_ymd_opt(2024, 1, 1).unwrap();
        let sell_date = NaiveDate::from_ymd_opt(2024, 6, 1).unwrap();
        insert_listing(&pool, 1, "VAS").await;
        insert_buy(&pool, 1, 1, buy_date, Decimal::from(100), Decimal::from(15)).await;
        insert_sell(
            &pool,
            2,
            1,
            sell_date,
            Decimal::from(100),
            Decimal::from(10),
        )
        .await;
        allocate(&pool, 1, 2, 1, Decimal::from(100)).await;

        let result = db_realised_gains(&pool).await.unwrap();
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].capital_gain_loss, Decimal::from(-500));
        assert_eq!(result[0].discount_eligible_gain, Decimal::ZERO);
        // The loss is captured as a positive capital_loss; no gain buckets.
        assert_eq!(result[0].non_discountable_gain, Decimal::ZERO);
        assert_eq!(result[0].capital_loss, Decimal::from(500));
    }

    #[tokio::test]
    async fn db_brokerage_prorated_to_allocation() {
        let pool = test_pool().await;
        // buy 100 @ $10, brokerage $9.95 + gst $0.995 → cost = 1010.945
        // sell 100 @ $15, brokerage $9.95 + gst $0.995
        //   proceeds = 1500 - 10.945 = 1489.055
        //   gain = 1489.055 - 1010.945 = 478.110
        let buy_date = NaiveDate::from_ymd_opt(2024, 1, 1).unwrap();
        let sell_date = NaiveDate::from_ymd_opt(2024, 6, 1).unwrap();
        insert_listing(&pool, 1, "VAS").await;
        test_support::buy(1, 1)
            .date(buy_date)
            .price(Decimal::from(10))
            .brokerage("9.95".parse().unwrap())
            .gst_on_brokerage("0.995".parse().unwrap())
            .insert(&pool)
            .await;
        test_support::sell(2, 1)
            .date(sell_date)
            .price(Decimal::from(15))
            .brokerage("9.95".parse().unwrap())
            .gst_on_brokerage("0.995".parse().unwrap())
            .insert(&pool)
            .await;
        allocate(&pool, 1, 2, 1, Decimal::from(100)).await;

        let result = db_realised_gains(&pool).await.unwrap();
        assert_eq!(result.len(), 1);
        // proceeds = 15*100 - 10.945 = 1489.055
        assert_eq!(result[0].proceeds, "1489.055".parse::<Decimal>().unwrap());
        // cost_base = 10*100 + 10.945 = 1010.945
        assert_eq!(result[0].cost_base, "1010.945".parse::<Decimal>().unwrap());
        // gain = 1489.055 - 1010.945 = 478.110
        assert_eq!(
            result[0].capital_gain_loss,
            "478.110".parse::<Decimal>().unwrap()
        );
    }

    #[tokio::test]
    async fn db_cgt_discount_eligible_after_12_months() {
        let pool = test_pool().await;
        let buy_date = NaiveDate::from_ymd_opt(2024, 1, 1).unwrap();
        let sell_date = NaiveDate::from_ymd_opt(2025, 1, 2).unwrap(); // strictly > 12 months
        insert_listing(&pool, 1, "VAS").await;
        insert_buy(&pool, 1, 1, buy_date, Decimal::from(100), Decimal::from(10)).await;
        insert_sell(
            &pool,
            2,
            1,
            sell_date,
            Decimal::from(100),
            Decimal::from(15),
        )
        .await;
        allocate(&pool, 1, 2, 1, Decimal::from(100)).await;

        let result = db_realised_gains(&pool).await.unwrap();
        assert_eq!(result[0].capital_gain_loss, Decimal::from(500));
        assert_eq!(result[0].discount_eligible_gain, Decimal::from(500));
    }

    /// Security identity continuity across a ticker/name change: a rename is an
    /// in-place edit to the listing (same id), so a sale entered after the
    /// rename still allocates against the pre-rename parcel — original cost
    /// base, and the 12-month discount clock keeps running from the original
    /// acquisition date.
    #[tokio::test]
    async fn db_sale_after_ticker_rename_keeps_cost_base_and_discount_clock() {
        let pool = test_pool().await;
        let buy_date = NaiveDate::from_ymd_opt(2024, 1, 1).unwrap();
        let sell_date = NaiveDate::from_ymd_opt(2025, 1, 2).unwrap(); // strictly > 12 months
        insert_listing(&pool, 1, "OLD").await;
        insert_buy(&pool, 1, 1, buy_date, Decimal::from(100), Decimal::from(10)).await;
        // The security is renamed before the sale: same listing id, new
        // ticker — recorded via the rename action (a bare PUT is refused
        // once the listing has a trade; see
        // entities::listing::UpsertError::IdentityChangeRequiresRename).
        crate::entities::listing_rename::db_rename(
            &pool,
            1,
            &crate::entities::listing_rename::RenameBody {
                effective_date: NaiveDate::from_ymd_opt(2024, 6, 1).unwrap(),
                ticker: "NEW".to_string(),
                exchange_mic: None,
                name: Some("NEW".to_string()),
                price_symbol: None,
                note: None,
            },
        )
        .await
        .unwrap();
        insert_sell(
            &pool,
            2,
            1,
            sell_date,
            Decimal::from(100),
            Decimal::from(15),
        )
        .await;
        allocate(&pool, 1, 2, 1, Decimal::from(100)).await;

        let result = db_realised_gains(&pool).await.unwrap();
        assert_eq!(result.len(), 1);
        // Cost base from the pre-rename buy: 100 × $10 = $1,000; proceeds $1,500.
        assert_eq!(result[0].listing_id, 1);
        assert_eq!(result[0].cost_base, Decimal::from(1000));
        assert_eq!(result[0].capital_gain_loss, Decimal::from(500));
        // Held > 12 months from the original acquisition date → discount-eligible.
        assert_eq!(result[0].discount_eligible_gain, Decimal::from(500));
    }

    /// Crypto parcels flow through the report exactly like share parcels:
    /// satoshi-scale fractional quantities (8 decimal places) stay exact in
    /// the AUD cost base, proceeds, and gain, and a parcel held more than 12
    /// months is discount-eligible (docs/ato/crypto-cgt.md).
    #[tokio::test]
    async fn db_crypto_sale_keeps_satoshi_precision_and_discount() {
        let pool = test_pool().await;
        // An exchange-less Crypto listing (BTC is a seeded digital token).
        test_support::listing(1)
            .crypto()
            .ticker("BTC")
            .name("Bitcoin")
            .insert(&pool)
            .await;
        let buy_date = NaiveDate::from_ymd_opt(2024, 1, 1).unwrap();
        let sell_date = NaiveDate::from_ymd_opt(2025, 1, 2).unwrap(); // strictly > 12 months
        let qty: Decimal = "0.12345678".parse().unwrap();
        insert_buy(&pool, 1, 1, buy_date, qty, Decimal::from(60000)).await;
        insert_sell(&pool, 2, 1, sell_date, qty, Decimal::from(100000)).await;
        allocate(&pool, 1, 2, 1, qty).await;

        let result = db_realised_gains(&pool).await.unwrap();
        assert_eq!(result.len(), 1);
        // 0.12345678 × 60,000 / 100,000: every satoshi-scale digit preserved.
        assert_eq!(
            result[0].cost_base,
            "7407.40680000".parse::<Decimal>().unwrap()
        );
        assert_eq!(
            result[0].proceeds,
            "12345.67800000".parse::<Decimal>().unwrap()
        );
        assert_eq!(
            result[0].capital_gain_loss,
            "4938.27120000".parse::<Decimal>().unwrap()
        );
        // Held > 12 months → the whole gain is discount-eligible.
        assert_eq!(
            result[0].discount_eligible_gain,
            result[0].capital_gain_loss
        );
    }

    #[tokio::test]
    async fn db_cgt_not_eligible_exactly_12_months() {
        let pool = test_pool().await;
        let buy_date = NaiveDate::from_ymd_opt(2024, 1, 1).unwrap();
        let sell_date = NaiveDate::from_ymd_opt(2025, 1, 1).unwrap(); // exactly 12 months — not eligible
        insert_listing(&pool, 1, "VAS").await;
        insert_buy(&pool, 1, 1, buy_date, Decimal::from(100), Decimal::from(10)).await;
        insert_sell(
            &pool,
            2,
            1,
            sell_date,
            Decimal::from(100),
            Decimal::from(15),
        )
        .await;
        allocate(&pool, 1, 2, 1, Decimal::from(100)).await;

        let result = db_realised_gains(&pool).await.unwrap();
        assert_eq!(result[0].capital_gain_loss, Decimal::from(500));
        assert_eq!(result[0].discount_eligible_gain, Decimal::ZERO);
    }

    #[tokio::test]
    async fn db_loss_not_included_in_discount_eligible() {
        let pool = test_pool().await;
        // parcel held > 12 months but sold at a loss → discount_eligible_gain stays 0
        let buy_date = NaiveDate::from_ymd_opt(2024, 1, 1).unwrap();
        let sell_date = NaiveDate::from_ymd_opt(2025, 6, 1).unwrap();
        insert_listing(&pool, 1, "VAS").await;
        insert_buy(&pool, 1, 1, buy_date, Decimal::from(100), Decimal::from(15)).await;
        insert_sell(
            &pool,
            2,
            1,
            sell_date,
            Decimal::from(100),
            Decimal::from(10),
        )
        .await;
        allocate(&pool, 1, 2, 1, Decimal::from(100)).await;

        let result = db_realised_gains(&pool).await.unwrap();
        assert_eq!(result[0].capital_gain_loss, Decimal::from(-500));
        assert_eq!(result[0].discount_eligible_gain, Decimal::ZERO);
    }

    #[tokio::test]
    async fn db_two_parcels_mixed_eligibility() {
        let pool = test_pool().await;
        // old parcel: 100 units @ $10, bought 2023-01-01 (>12mo before sell)
        // new parcel: 50 units @ $10, bought 2025-01-01 (<12mo before sell)
        // sell 150 units @ $15 on 2025-06-01
        // gain per unit = 5, total gain = 750
        // only old parcel (100 units) is discount eligible → discount_eligible_gain = 500
        let old_date = NaiveDate::from_ymd_opt(2023, 1, 1).unwrap();
        let new_date = NaiveDate::from_ymd_opt(2025, 1, 1).unwrap();
        let sell_date = NaiveDate::from_ymd_opt(2025, 6, 1).unwrap();
        insert_listing(&pool, 1, "VAS").await;
        insert_buy(&pool, 1, 1, old_date, Decimal::from(100), Decimal::from(10)).await;
        insert_buy(&pool, 2, 1, new_date, Decimal::from(50), Decimal::from(10)).await;
        insert_sell(
            &pool,
            3,
            1,
            sell_date,
            Decimal::from(150),
            Decimal::from(15),
        )
        .await;
        allocate(&pool, 1, 3, 1, Decimal::from(100)).await;
        allocate(&pool, 2, 3, 2, Decimal::from(50)).await;

        let result = db_realised_gains(&pool).await.unwrap();
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].proceeds, Decimal::from(2250));
        assert_eq!(result[0].cost_base, Decimal::from(1500));
        assert_eq!(result[0].capital_gain_loss, Decimal::from(750));
        assert_eq!(result[0].discount_eligible_gain, Decimal::from(500));
        // The new (≤12mo) parcel's $250 gain is non-discountable; no losses.
        assert_eq!(result[0].non_discountable_gain, Decimal::from(250));
        assert_eq!(result[0].capital_loss, Decimal::ZERO);
        // Identity: capital_gain_loss == discount_eligible + non_discountable − loss.
        assert_eq!(
            result[0].capital_gain_loss,
            result[0].discount_eligible_gain + result[0].non_discountable_gain
                - result[0].capital_loss
        );

        // Per-parcel breakdown: one entry per allocation, sorted oldest first,
        // each carrying the same figures the totals above are summed from.
        assert_eq!(result[0].parcels.len(), 2);
        let old = &result[0].parcels[0];
        assert_eq!(old.purchase_trade_id, 1);
        assert_eq!(old.acquisition_date, old_date);
        assert_eq!(old.units, Decimal::from(100));
        assert_eq!(old.cost_base, Decimal::from(1000));
        assert_eq!(old.proceeds, Decimal::from(1500));
        assert_eq!(old.capital_gain_loss, Decimal::from(500));
        assert!(old.discount_eligible);
        let new = &result[0].parcels[1];
        assert_eq!(new.purchase_trade_id, 2);
        assert_eq!(new.acquisition_date, new_date);
        assert_eq!(new.units, Decimal::from(50));
        assert_eq!(new.cost_base, Decimal::from(500));
        assert_eq!(new.proceeds, Decimal::from(750));
        assert_eq!(new.capital_gain_loss, Decimal::from(250));
        assert!(!new.discount_eligible);

        // The parcels reconcile exactly to the disposal's totals.
        let parcel_cost_base: Decimal = result[0].parcels.iter().map(|p| p.cost_base).sum();
        let parcel_proceeds: Decimal = result[0].parcels.iter().map(|p| p.proceeds).sum();
        assert_eq!(parcel_cost_base, result[0].cost_base);
        assert_eq!(parcel_proceeds, result[0].proceeds);
        let discount_from_parcels: Decimal = result[0]
            .parcels
            .iter()
            .filter(|p| p.discount_eligible && p.capital_gain_loss > Decimal::ZERO)
            .map(|p| p.capital_gain_loss)
            .sum();
        assert_eq!(discount_from_parcels, result[0].discount_eligible_gain);
    }

    #[tokio::test]
    async fn db_amit_reduces_cost_base_increases_gain() {
        let pool = test_pool().await;
        // buy 100 @ $10 → cost = 1000, sell 100 @ $15 → proceeds = 1500
        // AMIT reduces cost by 100 * $0.05 = $5 → cost_base = 995, gain = 505
        let buy_date = NaiveDate::from_ymd_opt(2024, 1, 1).unwrap();
        let sell_date = NaiveDate::from_ymd_opt(2025, 6, 1).unwrap();
        insert_listing(&pool, 1, "VAF").await;
        insert_buy(&pool, 1, 1, buy_date, Decimal::from(100), Decimal::from(10)).await;
        insert_sell(
            &pool,
            2,
            1,
            sell_date,
            Decimal::from(100),
            Decimal::from(15),
        )
        .await;
        allocate(&pool, 1, 2, 1, Decimal::from(100)).await;

        test_support::amma(1, 1)
            .cost_base_adjustment("0.05".parse().unwrap())
            .insert(&pool)
            .await;
        test_support::amit_adjustment(&pool, 1, 1, 1, Decimal::from(100)).await;

        let result = db_realised_gains(&pool).await.unwrap();
        assert_eq!(result.len(), 1);
        // cost = 1000 - 100*0.05 = 995
        assert_eq!(result[0].cost_base, Decimal::from(995));
        // gain = 1500 - 995 = 505
        assert_eq!(result[0].capital_gain_loss, Decimal::from(505));
        // discount eligible: sell_date 2025-06-01 > buy_date 2024-01-01 + 12mo = 2025-01-01 ✓
        assert_eq!(result[0].discount_eligible_gain, Decimal::from(505));
    }

    #[tokio::test]
    async fn db_sorted_by_sale_date() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "VAS").await;
        let d = |y, m, d| NaiveDate::from_ymd_opt(y, m, d).unwrap();
        insert_buy(
            &pool,
            1,
            1,
            d(2023, 1, 1),
            Decimal::from(100),
            Decimal::from(10),
        )
        .await;
        insert_buy(
            &pool,
            2,
            1,
            d(2023, 1, 1),
            Decimal::from(100),
            Decimal::from(10),
        )
        .await;
        // sell 2 comes before sell 3 by date but is inserted second
        insert_sell(
            &pool,
            3,
            1,
            d(2024, 6, 1),
            Decimal::from(50),
            Decimal::from(15),
        )
        .await;
        insert_sell(
            &pool,
            4,
            1,
            d(2024, 3, 1),
            Decimal::from(50),
            Decimal::from(15),
        )
        .await;
        allocate(&pool, 1, 3, 1, Decimal::from(50)).await;
        allocate(&pool, 2, 4, 2, Decimal::from(50)).await;

        let result = db_realised_gains(&pool).await.unwrap();
        assert_eq!(result.len(), 2);
        assert!(result[0].sale_date <= result[1].sale_date);
        assert_eq!(result[0].sale_trade_id, 4);
        assert_eq!(result[1].sale_trade_id, 3);
    }

    // API-level test

    #[tokio::test]
    async fn api_realised_gains_returns_json() {
        let pool = test_pool().await;
        let buy_date = NaiveDate::from_ymd_opt(2024, 1, 1).unwrap();
        let sell_date = NaiveDate::from_ymd_opt(2025, 6, 1).unwrap();
        insert_listing(&pool, 1, "VAS").await;
        insert_buy(&pool, 1, 1, buy_date, Decimal::from(100), Decimal::from(10)).await;
        insert_sell(
            &pool,
            2,
            1,
            sell_date,
            Decimal::from(100),
            Decimal::from(15),
        )
        .await;
        allocate(&pool, 1, 2, 1, Decimal::from(100)).await;

        let resp = router()
            .with_state(pool)
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/portfolio/realised-gains")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let result: Vec<RealisedGainLoss> = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].capital_gain_loss, Decimal::from(500));
        // held > 12 months
        assert_eq!(result[0].discount_eligible_gain, Decimal::from(500));
    }

    // Return of capital (CGT event G1)

    async fn apply_roc(pool: &SqlitePool, id: i64, listing_id: i64, date: NaiveDate, amount: &str) {
        corporate_action::db_upsert(
            pool,
            &corporate_action::CorporateAction {
                id,
                listing_id,
                date,
                kind: corporate_action::ActionKind::ReturnOfCapital {
                    amount_per_unit: amount.parse().unwrap(),
                    currency: "AUD".to_string(),
                },
            },
        )
        .await
        .unwrap();
    }

    // Share splits / consolidations (TD 2000/10)

    async fn apply_split(
        pool: &SqlitePool,
        id: i64,
        listing_id: i64,
        date: NaiveDate,
        new: &str,
        old: &str,
    ) {
        corporate_action::db_upsert(
            pool,
            &corporate_action::CorporateAction {
                id,
                listing_id,
                date,
                kind: corporate_action::ActionKind::ShareSplit {
                    split_new_units: new.parse().unwrap(),
                    split_old_units: old.parse().unwrap(),
                },
            },
        )
        .await
        .unwrap();
    }

    /// TD 2000/10 (`docs/ato/share-splits-and-consolidations.md`): selling the whole
    /// post-split holding realises the parcel's full, unchanged cost base —
    /// the split itself is no CGT event and creates no gain.
    #[tokio::test]
    async fn db_post_split_sale_uses_unchanged_total_cost_base() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "SPL").await;
        // Buy 100 @ $10 (2024-01-01) → cost base 1000 (zero brokerage helper).
        insert_buy(
            &pool,
            1,
            1,
            NaiveDate::from_ymd_opt(2024, 1, 1).unwrap(),
            Decimal::from(100),
            Decimal::from(10),
        )
        .await;
        // 2-for-1 split, then sell all 200 post-split units @ $6.
        apply_split(
            &pool,
            1,
            1,
            NaiveDate::from_ymd_opt(2024, 3, 1).unwrap(),
            "2",
            "1",
        )
        .await;
        insert_sell(
            &pool,
            2,
            1,
            NaiveDate::from_ymd_opt(2024, 6, 1).unwrap(),
            Decimal::from(200),
            Decimal::from(6),
        )
        .await;
        allocate(&pool, 1, 2, 1, Decimal::from(200)).await;

        let result = db_realised_gains(&pool).await.unwrap();
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].proceeds, Decimal::from(1200));
        assert_eq!(result[0].cost_base, Decimal::from(1000));
        assert_eq!(result[0].capital_gain_loss, Decimal::from(200));
    }

    /// A partial post-split sale pro-rates the cost base over the converted
    /// unit count: 80 of 200 post-split units carry 40% of the parcel's cost.
    #[tokio::test]
    async fn db_partial_post_split_sale_pro_rates_cost_base() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "SPL").await;
        insert_buy(
            &pool,
            1,
            1,
            NaiveDate::from_ymd_opt(2024, 1, 1).unwrap(),
            Decimal::from(100),
            Decimal::from(10),
        )
        .await;
        apply_split(
            &pool,
            1,
            1,
            NaiveDate::from_ymd_opt(2024, 3, 1).unwrap(),
            "2",
            "1",
        )
        .await;
        insert_sell(
            &pool,
            2,
            1,
            NaiveDate::from_ymd_opt(2024, 6, 1).unwrap(),
            Decimal::from(80),
            Decimal::from(6),
        )
        .await;
        allocate(&pool, 1, 2, 1, Decimal::from(80)).await;

        let result = db_realised_gains(&pool).await.unwrap();
        assert_eq!(result.len(), 1);
        // cost base = 1000 × (80 post-split = 40 as-acquired) / 100 = 400
        assert_eq!(result[0].cost_base, Decimal::from(400));
        // proceeds 80 × 6 = 480 → gain 80
        assert_eq!(result[0].capital_gain_loss, Decimal::from(80));
    }

    /// A non-assessable bonus issue (`docs/ato/bonus-shares.md`) apportions the
    /// parcel's cost base over original + bonus shares, and the bonus shares
    /// keep the original acquisition date — so a partial post-bonus sale
    /// pro-rates the unchanged cost base and the 12-month discount clock runs
    /// from the original buy, not the issue date.
    #[tokio::test]
    async fn db_post_bonus_issue_sale_apportions_cost_base_and_keeps_discount() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "BON").await;
        insert_buy(
            &pool,
            1,
            1,
            NaiveDate::from_ymd_opt(2024, 1, 1).unwrap(),
            Decimal::from(100),
            Decimal::from(10),
        )
        .await;
        // 1-for-10 bonus issue 16 months later: 100 → 110 units.
        corporate_action::db_upsert(
            &pool,
            &corporate_action::CorporateAction {
                id: 1,
                listing_id: 1,
                date: NaiveDate::from_ymd_opt(2025, 5, 1).unwrap(),
                kind: corporate_action::ActionKind::BonusIssue {
                    bonus_units: Decimal::ONE,
                    bonus_held_units: Decimal::from(10),
                },
            },
        )
        .await
        .unwrap();
        // Sell 55 of the 110 post-bonus units (= 50 as-acquired) two months
        // after the issue, 18 months after the original buy.
        insert_sell(
            &pool,
            2,
            1,
            NaiveDate::from_ymd_opt(2025, 7, 1).unwrap(),
            Decimal::from(55),
            Decimal::from(12),
        )
        .await;
        allocate(&pool, 1, 2, 1, Decimal::from(55)).await;

        let result = db_realised_gains(&pool).await.unwrap();
        assert_eq!(result.len(), 1);
        // cost base = 1000 × (55 post-bonus = 50 as-acquired) / 100 = 500
        assert_eq!(result[0].cost_base, Decimal::from(500));
        // proceeds 55 × 12 = 660 → gain 160, discount-eligible from the
        // original 2024-01-01 acquisition (not the 2025 issue date).
        assert_eq!(result[0].capital_gain_loss, Decimal::from(160));
        assert_eq!(result[0].discount_eligible_gain, Decimal::from(160));
        assert_eq!(result[0].non_discountable_gain, Decimal::ZERO);
    }

    /// The converted shares keep the original acquisition date (TD 2000/10), so
    /// the 12-month discount clock runs from the original buy, not the split.
    #[tokio::test]
    async fn db_split_preserves_acquisition_date_for_discount() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "SPL").await;
        // Held 18 months from the original buy; the split happened 2 months
        // before the sale.
        insert_buy(
            &pool,
            1,
            1,
            NaiveDate::from_ymd_opt(2024, 1, 1).unwrap(),
            Decimal::from(100),
            Decimal::from(10),
        )
        .await;
        apply_split(
            &pool,
            1,
            1,
            NaiveDate::from_ymd_opt(2025, 5, 1).unwrap(),
            "2",
            "1",
        )
        .await;
        insert_sell(
            &pool,
            2,
            1,
            NaiveDate::from_ymd_opt(2025, 7, 1).unwrap(),
            Decimal::from(200),
            Decimal::from(8),
        )
        .await;
        allocate(&pool, 1, 2, 1, Decimal::from(200)).await;

        let result = db_realised_gains(&pool).await.unwrap();
        assert_eq!(result.len(), 1);
        // gain = 1600 − 1000 = 600, all discount-eligible.
        assert_eq!(result[0].capital_gain_loss, Decimal::from(600));
        assert_eq!(result[0].discount_eligible_gain, Decimal::from(600));
        assert_eq!(result[0].non_discountable_gain, Decimal::ZERO);
    }

    /// A return of capital received between a split and the sale is per
    /// post-split unit: the sold units' cost base drops by payment × post-split
    /// quantity.
    #[tokio::test]
    async fn db_return_of_capital_after_split_reduces_sold_cost_base() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "SPL").await;
        insert_buy(
            &pool,
            1,
            1,
            NaiveDate::from_ymd_opt(2024, 1, 1).unwrap(),
            Decimal::from(100),
            Decimal::from(10),
        )
        .await;
        apply_split(
            &pool,
            1,
            1,
            NaiveDate::from_ymd_opt(2024, 3, 1).unwrap(),
            "2",
            "1",
        )
        .await;
        // 25c per post-split unit while all 200 are held.
        apply_roc(
            &pool,
            2,
            1,
            NaiveDate::from_ymd_opt(2024, 4, 1).unwrap(),
            "0.25",
        )
        .await;
        insert_sell(
            &pool,
            2,
            1,
            NaiveDate::from_ymd_opt(2024, 6, 1).unwrap(),
            Decimal::from(200),
            Decimal::from(6),
        )
        .await;
        allocate(&pool, 1, 2, 1, Decimal::from(200)).await;

        let result = db_realised_gains(&pool).await.unwrap();
        assert_eq!(result.len(), 1);
        // cost base = 1000 − 200 × 0.25 = 950
        assert_eq!(result[0].cost_base, Decimal::from(950));
        assert_eq!(result[0].capital_gain_loss, Decimal::from(250));
    }

    /// A return of capital received while the sold units were held reduces their
    /// cost base, so the realised gain grows by the per-unit payment × quantity
    /// (`docs/ato/cgt-non-assessable-payments.md`).
    #[tokio::test]
    async fn db_return_of_capital_during_holding_reduces_cost_base() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "RAP").await;
        // Buy 100 @ $10 (2024-01-01), 50c/unit payment (2024-03-01),
        // sell 100 @ $12 (2024-06-01). Zero brokerage in these helpers.
        insert_buy(
            &pool,
            1,
            1,
            NaiveDate::from_ymd_opt(2024, 1, 1).unwrap(),
            Decimal::from(100),
            Decimal::from(10),
        )
        .await;
        apply_roc(
            &pool,
            1,
            1,
            NaiveDate::from_ymd_opt(2024, 3, 1).unwrap(),
            "0.50",
        )
        .await;
        insert_sell(
            &pool,
            2,
            1,
            NaiveDate::from_ymd_opt(2024, 6, 1).unwrap(),
            Decimal::from(100),
            Decimal::from(12),
        )
        .await;
        allocate(&pool, 1, 2, 1, Decimal::from(100)).await;

        let result = db_realised_gains(&pool).await.unwrap();
        assert_eq!(result.len(), 1);
        // cost base = 1000 − 100 × 0.50 = 950
        assert_eq!(result[0].cost_base, Decimal::from(950));
        // gain = 1200 − 950 = 250
        assert_eq!(result[0].capital_gain_loss, Decimal::from(250));
    }

    /// A payment made after the units were sold doesn't touch them: they were no
    /// longer held when it was received.
    #[tokio::test]
    async fn db_return_of_capital_after_sale_does_not_affect_cost_base() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "RAP").await;
        insert_buy(
            &pool,
            1,
            1,
            NaiveDate::from_ymd_opt(2024, 1, 1).unwrap(),
            Decimal::from(100),
            Decimal::from(10),
        )
        .await;
        insert_sell(
            &pool,
            2,
            1,
            NaiveDate::from_ymd_opt(2024, 6, 1).unwrap(),
            Decimal::from(100),
            Decimal::from(12),
        )
        .await;
        allocate(&pool, 1, 2, 1, Decimal::from(100)).await;
        // Payment after the sale.
        apply_roc(
            &pool,
            1,
            1,
            NaiveDate::from_ymd_opt(2024, 7, 1).unwrap(),
            "0.50",
        )
        .await;

        let result = db_realised_gains(&pool).await.unwrap();
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].cost_base, Decimal::from(1000));
        assert_eq!(result[0].capital_gain_loss, Decimal::from(200));
    }

    // FX conversion

    async fn insert_usd_listing(pool: &SqlitePool, id: i64, ticker: &str) {
        test_support::listing(id)
            .mic("XNYS")
            .ticker(ticker)
            .name(ticker)
            .security_type(listing::SecurityType::Share)
            .currency("USD")
            .insert(pool)
            .await;
    }

    async fn insert_usd_trade(
        pool: &SqlitePool,
        id: i64,
        trade_type: trade::TradeType,
        date: NaiveDate,
        qty: Decimal,
        price: Decimal,
    ) {
        test_support::trade(id, 1, trade_type)
            .date(date)
            .qty(qty)
            .price(price)
            .currency("USD")
            // A wrong manual override: the report must prefer the ATO rate.
            .fx_rate("0.99".parse().unwrap())
            .insert(pool)
            .await;
    }

    #[tokio::test]
    async fn db_usd_buy_sell_produces_aud_cost_base_and_gain_via_ato_rate() {
        let pool = test_pool().await;
        let buy_date = NaiveDate::from_ymd_opt(2024, 1, 15).unwrap();
        let sell_date = NaiveDate::from_ymd_opt(2025, 6, 1).unwrap();
        insert_usd_listing(&pool, 1, "AAPL").await;
        // ATO RBA rates (foreign-per-AUD): A$1 = 0.50 USD in Jan-2024, 0.60 in Jun-2025.
        rba_fx_rate::db_import_rate(&pool, "USD", "2024-01", "0.50".parse().unwrap())
            .await
            .unwrap();
        rba_fx_rate::db_import_rate(&pool, "USD", "2025-06", "0.60".parse().unwrap())
            .await
            .unwrap();

        // Buy 100 @ US$10 (US$1000), sell 100 @ US$15 (US$1500), zero brokerage.
        insert_usd_trade(
            &pool,
            1,
            trade::TradeType::Buy,
            buy_date,
            Decimal::from(100),
            Decimal::from(10),
        )
        .await;
        insert_usd_trade(
            &pool,
            2,
            trade::TradeType::Sell,
            sell_date,
            Decimal::from(100),
            Decimal::from(15),
        )
        .await;
        allocate(&pool, 1, 2, 1, Decimal::from(100)).await;

        let result = db_realised_gains(&pool).await.unwrap();
        assert_eq!(result.len(), 1);
        // cost = US$1000 / 0.50 = A$2000 (ATO rate, not the 0.99 override)
        assert_eq!(result[0].cost_base, Decimal::from(2000));
        // proceeds = US$1500 / 0.60 = A$2500
        assert_eq!(result[0].proceeds, Decimal::from(2500));
        // gain = 2500 - 2000 = A$500
        assert_eq!(result[0].capital_gain_loss, Decimal::from(500));
        // held > 12 months → fully discount-eligible (in AUD)
        assert_eq!(result[0].discount_eligible_gain, Decimal::from(500));
    }

    #[tokio::test]
    async fn db_usd_falls_back_to_manual_fx_rate_when_no_ato_rate() {
        let pool = test_pool().await;
        let buy_date = NaiveDate::from_ymd_opt(2024, 1, 15).unwrap();
        let sell_date = NaiveDate::from_ymd_opt(2025, 6, 1).unwrap();
        insert_usd_listing(&pool, 1, "AAPL").await;
        // No ATO rates imported → both trades fall back to their 0.99 manual override.
        insert_usd_trade(
            &pool,
            1,
            trade::TradeType::Buy,
            buy_date,
            Decimal::from(100),
            Decimal::from(10),
        )
        .await;
        insert_usd_trade(
            &pool,
            2,
            trade::TradeType::Sell,
            sell_date,
            Decimal::from(100),
            Decimal::from(15),
        )
        .await;
        allocate(&pool, 1, 2, 1, Decimal::from(100)).await;

        let result = db_realised_gains(&pool).await.unwrap();
        assert_eq!(result.len(), 1);
        // cost = US$1000 / 0.99, proceeds = US$1500 / 0.99
        assert_eq!(
            result[0].cost_base,
            Decimal::from(1000) / "0.99".parse::<Decimal>().unwrap()
        );
        assert_eq!(
            result[0].proceeds,
            Decimal::from(1500) / "0.99".parse::<Decimal>().unwrap()
        );
    }

    /// Take over listing `from` with listing `to`, 2 new units per 1 old, on
    /// `date`, and run the exchange. Returns the created group.
    async fn exchange_two_for_one(
        pool: &SqlitePool,
        action_id: i64,
        from: i64,
        to: i64,
        date: NaiveDate,
    ) -> crate::entities::scrip_exchange::Exchange {
        corporate_action::db_upsert(
            pool,
            &corporate_action::CorporateAction {
                id: action_id,
                listing_id: from,
                date,
                kind: corporate_action::ActionKind::ScripForScrip {
                    scrip_listing_id: to,
                    scrip_new_units: Decimal::from(2),
                    scrip_old_units: Decimal::ONE,
                    scrip_cash_per_unit: None,
                    scrip_market_value: None,
                    scrip_cash_currency: None,
                },
            },
        )
        .await
        .unwrap();
        crate::entities::scrip_exchange::db_exchange(pool, action_id)
            .await
            .unwrap()
    }

    /// The scrip-for-scrip rollover disregards the gain on the exchanged
    /// shares: the closing Sell never appears as a realised gain or loss —
    /// despite its zero proceeds — and the exchange year reports nothing.
    #[tokio::test]
    async fn db_scrip_exchange_closing_sell_is_excluded() {
        let pool = test_pool().await;
        let buy_date = NaiveDate::from_ymd_opt(2020, 10, 1).unwrap();
        insert_listing(&pool, 1, "OLD").await;
        insert_listing(&pool, 2, "NEW").await;
        insert_buy(
            &pool,
            1,
            1,
            buy_date,
            Decimal::from(1000),
            "1.50".parse().unwrap(),
        )
        .await;
        exchange_two_for_one(
            &pool,
            10,
            1,
            2,
            NaiveDate::from_ymd_opt(2024, 7, 1).unwrap(),
        )
        .await;

        let result = db_realised_gains(&pool).await.unwrap();
        assert!(
            result.is_empty(),
            "the rollover disposal is not a realised gain/loss"
        );
    }

    /// A partial-rollover exchange (cash component) realises the cash side:
    /// proceeds = cash × units, cost base = the market-value-apportioned
    /// share of the parcel's reduced cost base, discount-classified by the
    /// original holding period (docs/ato/takeovers-and-scrip-for-scrip.md
    /// Example 27 — Gunther's $700 gain).
    #[tokio::test]
    async fn db_partial_rollover_cash_component_realises_the_cash_side() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "WDR").await;
        insert_listing(&pool, 2, "RGL").await;
        let buy_date = NaiveDate::from_ymd_opt(2023, 1, 15).unwrap();
        // Gunther: 100 shares, $9 cost base each = $900.
        insert_buy(&pool, 1, 1, buy_date, Decimal::from(100), Decimal::from(9)).await;
        // 1-for-1 plus $10 cash per old share; new shares worth $20.
        corporate_action::db_upsert(
            &pool,
            &corporate_action::CorporateAction {
                id: 10,
                listing_id: 1,
                date: NaiveDate::from_ymd_opt(2024, 7, 10).unwrap(),
                kind: corporate_action::ActionKind::ScripForScrip {
                    scrip_listing_id: 2,
                    scrip_new_units: Decimal::ONE,
                    scrip_old_units: Decimal::ONE,
                    scrip_cash_per_unit: Some(Decimal::from(10)),
                    scrip_market_value: Some(Decimal::from(20)),
                    scrip_cash_currency: Some("AUD".to_string()),
                },
            },
        )
        .await
        .unwrap();
        crate::entities::scrip_exchange::db_exchange(&pool, 10)
            .await
            .unwrap();

        let result = db_realised_gains(&pool).await.unwrap();
        assert_eq!(result.len(), 1);
        // $1,000 cash proceeds − $300 (10/(10+20) of $900) = $700 gain,
        // discount-eligible from the original 2023 acquisition (> 12 months).
        assert_eq!(result[0].proceeds, Decimal::from(1000));
        assert_eq!(result[0].cost_base, Decimal::from(300));
        assert_eq!(result[0].capital_gain_loss, Decimal::from(700));
        assert_eq!(result[0].discount_eligible_gain, Decimal::from(700));
        assert_eq!(result[0].non_discountable_gain, Decimal::ZERO);
        assert_eq!(result[0].capital_loss, Decimal::ZERO);
    }

    /// The pure apportionment seam: a sell carrying a cash apportionment
    /// realises only that share of the cost base — exact fractions (1/3)
    /// divide once, after the multiplication.
    #[test]
    fn pure_scrip_cash_apportionment_scales_the_cost_base() {
        let d = |y, m, day| NaiveDate::from_ymd_opt(y, m, day).unwrap();
        // 100 units, $9 cost base each; $10 cash against a $30 per-unit total
        // consideration → apportionment (10, 30): 1-for-1 exchange, $20 market
        // value per new unit.
        let mut sell = mem_sell(2, d(2024, 7, 10), 100, 10, "AUD");
        sell.scrip_cash_per_unit = Some(Decimal::from(10));
        sell.scrip_market_value = Some(Decimal::from(20));
        sell.scrip_new_units = Some(Decimal::ONE);
        sell.scrip_old_units = Some(Decimal::ONE);
        assert_eq!(
            sell.scrip_cash_apportionment().unwrap(),
            Some((Decimal::from(10), Decimal::from(30)))
        );
        let data = ReportData {
            sells: [(2, sell)].into(),
            buys: [(1, mem_buy(1, d(2023, 1, 15), 100, 9, "AUD"))].into(),
            allocations: vec![mem_alloc(2, 1, 100)],
            ..ReportData::default()
        };

        let result = compute_realised_gains(&data).unwrap();
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].cost_base, Decimal::from(300));
        assert_eq!(result[0].capital_gain_loss, Decimal::from(700));
    }

    /// A later sale of the replacement parcel uses the carried cost base and
    /// the combined holding period: bought Oct 2020, exchanged Jul 2024, sold
    /// Oct 2024 — under 12 months after the exchange but over 12 months from
    /// the original acquisition, so the gain is discount-eligible.
    #[tokio::test]
    async fn db_sale_of_replacement_parcel_uses_carried_cost_base_and_combined_period() {
        let pool = test_pool().await;
        let buy_date = NaiveDate::from_ymd_opt(2020, 10, 1).unwrap();
        let sell_date = NaiveDate::from_ymd_opt(2024, 10, 1).unwrap();
        insert_listing(&pool, 1, "OLD").await;
        insert_listing(&pool, 2, "NEW").await;
        // 1,000 @ $1.50 = $1,500 cost base.
        insert_buy(
            &pool,
            1,
            1,
            buy_date,
            Decimal::from(1000),
            "1.50".parse().unwrap(),
        )
        .await;
        let ex = exchange_two_for_one(
            &pool,
            10,
            1,
            2,
            NaiveDate::from_ymd_opt(2024, 7, 1).unwrap(),
        )
        .await;

        // Sell all 2,000 replacement units at $1.00 → $2,000 proceeds.
        insert_sell(&pool, 50, 2, sell_date, Decimal::from(2000), Decimal::ONE).await;
        allocate(&pool, 1, 50, ex.replacements[0].id, Decimal::from(2000)).await;

        let result = db_realised_gains(&pool).await.unwrap();
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].sale_trade_id, 50);
        // Carried cost base, not the exchange-date figure.
        assert_eq!(result[0].cost_base, Decimal::from(1500));
        assert_eq!(result[0].capital_gain_loss, Decimal::from(500));
        // Combined period Oct 2020 → Oct 2024 exceeds 12 months.
        assert_eq!(result[0].discount_eligible_gain, Decimal::from(500));
        assert_eq!(result[0].non_discountable_gain, Decimal::ZERO);
    }

    /// The combined period is what counts: a parcel bought under 12 months
    /// before the sale stays non-discountable even though the exchange sits
    /// in between.
    #[tokio::test]
    async fn db_replacement_sale_within_combined_12_months_is_not_discounted() {
        let pool = test_pool().await;
        let buy_date = NaiveDate::from_ymd_opt(2024, 2, 1).unwrap();
        let sell_date = NaiveDate::from_ymd_opt(2024, 12, 1).unwrap();
        insert_listing(&pool, 1, "OLD").await;
        insert_listing(&pool, 2, "NEW").await;
        insert_buy(
            &pool,
            1,
            1,
            buy_date,
            Decimal::from(1000),
            "1.50".parse().unwrap(),
        )
        .await;
        let ex = exchange_two_for_one(
            &pool,
            10,
            1,
            2,
            NaiveDate::from_ymd_opt(2024, 7, 1).unwrap(),
        )
        .await;

        insert_sell(&pool, 50, 2, sell_date, Decimal::from(2000), Decimal::ONE).await;
        allocate(&pool, 1, 50, ex.replacements[0].id, Decimal::from(2000)).await;

        let result = db_realised_gains(&pool).await.unwrap();
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].discount_eligible_gain, Decimal::ZERO);
        assert_eq!(result[0].non_discountable_gain, Decimal::from(500));
    }

    /// A non-AUD replacement parcel's carried cost base converts at the
    /// *original* acquisition month's ATO rate (the rollover carries the AUD
    /// cost base over), not at the exchange month's.
    #[tokio::test]
    async fn db_usd_replacement_cost_base_converts_at_the_original_buy_month() {
        let pool = test_pool().await;
        let buy_date = NaiveDate::from_ymd_opt(2024, 1, 15).unwrap();
        let sell_date = NaiveDate::from_ymd_opt(2025, 6, 1).unwrap();
        insert_usd_listing(&pool, 1, "OLDQ").await;
        insert_usd_listing(&pool, 2, "NEWQ").await;
        // US$0.50/A$ in the buy month, 0.80 at the exchange, 0.60 at the sale.
        for (month, rate) in [
            ("2024-01", "0.50"),
            ("2024-07", "0.80"),
            ("2025-06", "0.60"),
        ] {
            rba_fx_rate::db_import_rate(&pool, "USD", month, rate.parse().unwrap())
                .await
                .unwrap();
        }
        // US$10 × 100 = US$1,000 cost base = A$2,000 at the buy month.
        insert_usd_trade(
            &pool,
            1,
            trade::TradeType::Buy,
            buy_date,
            Decimal::from(100),
            Decimal::from(10),
        )
        .await;
        let ex = exchange_two_for_one(
            &pool,
            10,
            1,
            2,
            NaiveDate::from_ymd_opt(2024, 7, 1).unwrap(),
        )
        .await;

        // Sell the 200 replacement units for US$7.50 each = US$1,500 = A$2,500.
        test_support::sell(50, 2)
            .date(sell_date)
            .qty(Decimal::from(200))
            .price("7.50".parse().unwrap())
            .currency("USD")
            .fx_rate("0.99".parse().unwrap())
            .insert(&pool)
            .await;
        allocate(&pool, 1, 50, ex.replacements[0].id, Decimal::from(200)).await;

        let result = db_realised_gains(&pool).await.unwrap();
        assert_eq!(result.len(), 1);
        // A$2,000 at the Jan-2024 rate — not US$1,000 / 0.80 = A$1,250.
        assert_eq!(result[0].cost_base, Decimal::from(2000));
        assert_eq!(result[0].proceeds, Decimal::from(2500));
    }

    /// Demerge listing `to` out of listing `from` (1 new unit per 5 held,
    /// 20% of the cost base to the demerged entity) on `date`.
    async fn demerge_one_for_five(
        pool: &SqlitePool,
        action_id: i64,
        from: i64,
        to: i64,
        date: NaiveDate,
    ) -> crate::entities::demerger::Demerge {
        corporate_action::db_upsert(
            pool,
            &corporate_action::CorporateAction {
                id: action_id,
                listing_id: from,
                date,
                kind: corporate_action::ActionKind::Demerger {
                    demerger_listing_id: to,
                    demerger_new_units: Decimal::ONE,
                    demerger_held_units: Decimal::from(5),
                    demerger_cost_base_pct: Decimal::from(20),
                },
            },
        )
        .await
        .unwrap();
        crate::entities::demerger::db_demerge(pool, action_id)
            .await
            .unwrap()
    }

    /// The demerger rollover disregards any gain or loss under the demerger:
    /// the closing Sell never appears as a realised gain or loss — despite
    /// its zero proceeds — and the demerger year reports nothing.
    #[tokio::test]
    async fn db_demerger_closing_sell_is_excluded() {
        let pool = test_pool().await;
        let buy_date = NaiveDate::from_ymd_opt(2020, 10, 1).unwrap();
        insert_listing(&pool, 1, "HEAD").await;
        insert_listing(&pool, 2, "DEM").await;
        insert_buy(
            &pool,
            1,
            1,
            buy_date,
            Decimal::from(1000),
            "1.50".parse().unwrap(),
        )
        .await;
        demerge_one_for_five(
            &pool,
            10,
            1,
            2,
            NaiveDate::from_ymd_opt(2024, 7, 1).unwrap(),
        )
        .await;

        let result = db_realised_gains(&pool).await.unwrap();
        assert!(
            result.is_empty(),
            "the rollover apportionment is not a realised gain/loss"
        );
    }

    /// Later sales on both sides of the demerger use the apportioned cost
    /// bases and the combined holding period: bought Oct 2020, demerged
    /// Jul 2024, sold Oct 2024 — under 12 months after the demerger but over
    /// 12 months from the original acquisition, so both gains are
    /// discount-eligible (the ATO's Example 32 rule for the new interests;
    /// the head interests' acquisition dates never changed).
    #[tokio::test]
    async fn db_post_demerger_sales_use_apportioned_cost_bases_and_combined_period() {
        let pool = test_pool().await;
        let buy_date = NaiveDate::from_ymd_opt(2020, 10, 1).unwrap();
        let sell_date = NaiveDate::from_ymd_opt(2024, 10, 1).unwrap();
        insert_listing(&pool, 1, "HEAD").await;
        insert_listing(&pool, 2, "DEM").await;
        // 1,000 @ $1.50 = $1,500 cost base → head $1,200 + demerged $300.
        insert_buy(
            &pool,
            1,
            1,
            buy_date,
            Decimal::from(1000),
            "1.50".parse().unwrap(),
        )
        .await;
        let dm = demerge_one_for_five(
            &pool,
            10,
            1,
            2,
            NaiveDate::from_ymd_opt(2024, 7, 1).unwrap(),
        )
        .await;

        // Sell the 1,000 head units at $2.00 and the 200 demerged units at
        // $3.00.
        insert_sell(
            &pool,
            50,
            1,
            sell_date,
            Decimal::from(1000),
            Decimal::from(2),
        )
        .await;
        allocate(
            &pool,
            1,
            50,
            dm.head_replacements[0].id,
            Decimal::from(1000),
        )
        .await;
        insert_sell(
            &pool,
            51,
            2,
            sell_date,
            Decimal::from(200),
            Decimal::from(3),
        )
        .await;
        allocate(
            &pool,
            2,
            51,
            dm.demerged_replacements[0].id,
            Decimal::from(200),
        )
        .await;

        let mut result = db_realised_gains(&pool).await.unwrap();
        result.sort_by_key(|r| r.sale_trade_id);
        assert_eq!(result.len(), 2);
        // Head: $2,000 − $1,200 = $800, discount-eligible from Oct 2020.
        assert_eq!(result[0].sale_trade_id, 50);
        assert_eq!(result[0].cost_base, Decimal::from(1200));
        assert_eq!(result[0].capital_gain_loss, Decimal::from(800));
        assert_eq!(result[0].discount_eligible_gain, Decimal::from(800));
        assert_eq!(result[0].non_discountable_gain, Decimal::ZERO);
        // Demerged: $600 − $300 = $300, also discount-eligible from Oct 2020.
        assert_eq!(result[1].sale_trade_id, 51);
        assert_eq!(result[1].cost_base, Decimal::from(300));
        assert_eq!(result[1].capital_gain_loss, Decimal::from(300));
        assert_eq!(result[1].discount_eligible_gain, Decimal::from(300));
        assert_eq!(result[1].non_discountable_gain, Decimal::ZERO);
    }

    // Rights sales/lapses (docs/ato/rights-issues.md Example 39,
    // docs/ato/retail-premiums.md)

    fn mem_rights_sale(
        id: i64,
        date: NaiveDate,
        units: i64,
        proceeds_per_right: &str,
        rights_cost: &str,
    ) -> RightsSaleInfo {
        RightsSaleInfo {
            id,
            listing_id: 1,
            holding_account_id: 1,
            date,
            units: Decimal::from(units),
            proceeds_per_right: proceeds_per_right.parse().unwrap(),
            rights_cost: rights_cost.parse().unwrap(),
            currency: "AUD".to_string(),
            fx_rate: Decimal::ONE,
        }
    }

    fn mem_rights_alloc(sale: i64, buy: i64, units: i64) -> RightsAllocation {
        RightsAllocation {
            rights_sale_id: sale,
            purchase_trade_id: buy,
            units: Decimal::from(units),
        }
    }

    /// Free rights sold take the **original parcel's** acquisition date for
    /// the discount — unlike an exercised parcel, whose clock restarts at the
    /// exercise. Sold weeks after the record date but >12 months after the
    /// anchoring buy → the whole (nil-cost-base) gain is discount-eligible.
    #[test]
    fn pure_rights_sale_discount_anchors_to_the_original_parcel() {
        let d = |y, m, day| NaiveDate::from_ymd_opt(y, m, day).unwrap();
        let data = ReportData {
            buys: [
                (1, mem_buy(1, d(2023, 1, 1), 1000, 2, "AUD")), // > 12 months
                (2, mem_buy(2, d(2025, 3, 1), 200, 2, "AUD")),  // ≤ 12 months
            ]
            .into(),
            rights_sales: vec![mem_rights_sale(7, d(2025, 5, 1), 300, "0.20", "0")],
            rights_allocations: vec![mem_rights_alloc(7, 1, 250), mem_rights_alloc(7, 2, 50)],
            ..ReportData::default()
        };

        let result = compute_realised_gains(&data).unwrap();
        assert_eq!(result.len(), 1);
        let r = &result[0];
        assert_eq!(r.source, DisposalSource::RightsSale);
        assert_eq!(r.sale_trade_id, 7);
        // Nil cost base: the gain is the whole proceeds, 300 × $0.20.
        assert_eq!(r.proceeds, Decimal::from(60));
        assert_eq!(r.cost_base, Decimal::ZERO);
        assert_eq!(r.capital_gain_loss, Decimal::from(60));
        // 250 rights anchored to the 2023 parcel are discount-eligible; the
        // 50 anchored to the 2025 parcel are not.
        assert_eq!(r.discount_eligible_gain, Decimal::from(50));
        assert_eq!(r.non_discountable_gain, Decimal::from(10));
        assert_eq!(r.capital_loss, Decimal::ZERO);
        assert_eq!(
            r.capital_gain_loss,
            r.discount_eligible_gain + r.non_discountable_gain - r.capital_loss
        );
    }

    /// A paid right that lapses (nil proceeds) realises a capital loss equal
    /// to its carried cost, and the cost shares sum exactly across
    /// allocations (cumulative-difference apportionment: $10 over three
    /// 1-right allocations has no third-of-a-cent leak).
    #[test]
    fn pure_lapsed_paid_rights_realise_the_carried_cost_as_a_loss() {
        let d = |y, m, day| NaiveDate::from_ymd_opt(y, m, day).unwrap();
        let data = ReportData {
            buys: [
                (1, mem_buy(1, d(2024, 1, 1), 100, 2, "AUD")),
                (2, mem_buy(2, d(2024, 2, 1), 100, 2, "AUD")),
                (3, mem_buy(3, d(2024, 3, 1), 100, 2, "AUD")),
            ]
            .into(),
            rights_sales: vec![mem_rights_sale(7, d(2024, 8, 1), 3, "0", "10")],
            rights_allocations: vec![
                mem_rights_alloc(7, 1, 1),
                mem_rights_alloc(7, 2, 1),
                mem_rights_alloc(7, 3, 1),
            ],
            ..ReportData::default()
        };

        let result = compute_realised_gains(&data).unwrap();
        assert_eq!(result.len(), 1);
        let r = &result[0];
        assert_eq!(r.proceeds, Decimal::ZERO);
        assert_eq!(r.cost_base, Decimal::from(10));
        assert_eq!(r.capital_gain_loss, Decimal::from(-10));
        assert_eq!(r.capital_loss, Decimal::from(10));
        assert_eq!(r.discount_eligible_gain, Decimal::ZERO);
        assert_eq!(r.non_discountable_gain, Decimal::ZERO);
    }

    /// End to end: a rights sale recorded through the operation reaches the
    /// report alongside ordinary Sells, AUD-converted via the ATO rate.
    #[tokio::test]
    async fn db_rights_sale_flows_into_the_report() {
        use crate::entities::rights_sale;
        let pool = test_pool().await;
        insert_listing(&pool, 1, "RTS").await;
        insert_buy(
            &pool,
            1,
            1,
            NaiveDate::from_ymd_opt(2023, 1, 15).unwrap(),
            Decimal::from(1000),
            Decimal::from(2),
        )
        .await;
        corporate_action::db_upsert(
            &pool,
            &corporate_action::CorporateAction {
                id: 10,
                listing_id: 1,
                date: NaiveDate::from_ymd_opt(2024, 7, 1).unwrap(),
                kind: corporate_action::ActionKind::RightsIssue {
                    rights_units: Decimal::ONE,
                    rights_held_units: Decimal::from(4),
                    exercise_price: "1.80".parse().unwrap(),
                    currency: "AUD".to_string(),
                },
            },
        )
        .await
        .unwrap();
        rights_sale::db_sell_rights(
            &pool,
            10,
            &rights_sale::SellRightsBody {
                date: NaiveDate::from_ymd_opt(2024, 7, 20).unwrap(),
                units: Decimal::from(250),
                proceeds_per_right: Some("0.20".parse().unwrap()),
                rights_cost: None,
                fx_rate: None,
                holding_account_id: 1,
                allocations: vec![rights_sale::AllocationInput {
                    purchase_trade_id: 1,
                    units: Decimal::from(250),
                }],
            },
        )
        .await
        .unwrap();
        // An ordinary Sell too, so both sources coexist in one report.
        insert_sell(
            &pool,
            2,
            1,
            NaiveDate::from_ymd_opt(2024, 8, 1).unwrap(),
            Decimal::from(100),
            Decimal::from(3),
        )
        .await;
        allocate(&pool, 1, 2, 1, Decimal::from(100)).await;

        let result = db_realised_gains(&pool).await.unwrap();
        assert_eq!(result.len(), 2);
        let rights = result
            .iter()
            .find(|r| r.source == DisposalSource::RightsSale)
            .unwrap();
        // 250 × $0.20 = $50 gain, nil cost base, discount-eligible from the
        // 2023 parcel — not from the 2024 record date.
        assert_eq!(rights.listing_id, 1);
        assert_eq!(rights.proceeds, Decimal::from(50));
        assert_eq!(rights.cost_base, Decimal::ZERO);
        assert_eq!(rights.capital_gain_loss, Decimal::from(50));
        assert_eq!(rights.discount_eligible_gain, Decimal::from(50));
        // The rights sale's single anchoring allocation surfaces as one
        // parcel, taking the anchor parcel's acquisition date (not the
        // rights issue's record date) as its discount-clock anchor.
        assert_eq!(rights.parcels.len(), 1);
        assert_eq!(rights.parcels[0].purchase_trade_id, 1);
        assert_eq!(
            rights.parcels[0].acquisition_date,
            NaiveDate::from_ymd_opt(2023, 1, 15).unwrap()
        );
        assert_eq!(rights.parcels[0].units, Decimal::from(250));
        assert_eq!(rights.parcels[0].proceeds, Decimal::from(50));
        assert_eq!(rights.parcels[0].cost_base, Decimal::ZERO);
        assert!(rights.parcels[0].discount_eligible);
        let sell = result
            .iter()
            .find(|r| r.source == DisposalSource::Sell)
            .unwrap();
        assert_eq!(sell.sale_trade_id, 2);
        assert_eq!(sell.capital_gain_loss, Decimal::from(100));
        assert_eq!(sell.parcels.len(), 1);
        assert_eq!(sell.parcels[0].purchase_trade_id, 1);
        assert_eq!(sell.parcels[0].units, Decimal::from(100));
        assert_eq!(sell.parcels[0].capital_gain_loss, Decimal::from(100));
    }
}
