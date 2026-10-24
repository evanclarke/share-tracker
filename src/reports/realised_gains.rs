//! Realised capital gains and losses: one row per disposal (a Sell or a rights sale) with its
//! proceeds, the adjusted cost base of the parcels it consumed, the gain or loss in AUD and the
//! CGT-discount and indexation figures, plus the per-parcel detail behind each.

use crate::domain::adjustments::AcquiredUnits;
use crate::domain::cost_base::{self, ParcelRow};
use crate::domain::{cgt_discount, indexation, open_parcels};
use crate::entities::corporate_action;
use crate::entities::corporate_action::{RocEvent, SplitEvent, UnitsAt};
use crate::infra::currency::CurrencyCode;
use crate::infra::db;
use crate::infra::decimal::{Money, OptMoney, apportion, mul_div, to_cents};
use crate::infra::extract::Json;
use crate::infra::fx::{FxOverride, FxRates};
use crate::infra::http::ApiError;
use crate::infra::id::{HoldingAccountId, ListingId, RightsSaleId, TradeId};
use crate::reports::taxpayer;
use axum::extract::State;
use chrono::NaiveDate;
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;
use std::collections::HashMap;
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

/// Which disposal a realised row is, and the row that records it — a trade
/// id and a rights-sale id are different id spaces, so the variant is what
/// says which table `disposal_id` names. Serialised adjacently tagged and
/// flattened into its row: `"source": "Sell", "disposal_id": 12`.
///
/// Variant order is the report's tie-break on a shared sale date (Sells
/// first, then each by id), which the derived `Ord` gives.
#[derive(
    utoipa::ToSchema, Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize,
)]
#[serde(tag = "source", content = "disposal_id")]
pub enum DisposalId {
    /// An ordinary Sell trade (incl. buy-back and worthless-shares closing
    /// Sells): the `trades` row id.
    Sell(TradeId),
    /// A rights sale/lapse (`entities::rights_sale` — a disposal of the
    /// rights themselves; the share holding is untouched): the
    /// `rights_sales` row id.
    RightsSale(RightsSaleId),
}

impl DisposalId {
    /// The Sell trade, for a disposal that is one.
    pub fn sell(self) -> Option<TradeId> {
        match self {
            Self::Sell(id) => Some(id),
            Self::RightsSale(_) => None,
        }
    }
}

#[derive(utoipa::ToSchema, Debug, Clone, Serialize, Deserialize)]
pub struct RealisedGainLoss {
    /// The disposal: `source` (`Sell` or `RightsSale`) and `disposal_id`, its
    /// row id in that source's table (`trades` or `rights_sales`).
    #[serde(flatten)]
    pub disposal: DisposalId,
    pub listing_id: ListingId,
    /// The holding account the Sell happened in (the same taxpayer either
    /// way — totals are unchanged; rows identify the account).
    pub holding_account_id: HoldingAccountId,
    pub sale_date: NaiveDate,
    /// Net proceeds from the allocated portion (sale price × qty − pro-rated
    /// brokerage), converted to AUD at the sale's ATO FX rate.
    pub proceeds: Decimal,
    /// Adjusted cost base of the sold parcels (AMIT-reduced, pro-rated to allocated
    /// qty), converted to AUD at the purchase's ATO FX rate.
    pub cost_base: Decimal,
    /// proceeds − `cost_base` in AUD (positive = gain, negative = loss).
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
    /// Informational: the taxpayer assumption behind the hard-wired rates
    /// (always [`crate::reports::taxpayer::TAXPAYER_BASIS`]) — the 12-month split into
    /// `discount_eligible_gain` / `non_discountable_gain` reported here is the
    /// Australian-resident-individual 50% discount's; other entity types are
    /// not modelled. Stated per row, as the tax-summary and net-capital-gain
    /// rows state it.
    pub taxpayer_basis: String,
}

/// One parcel's share of a realised disposal — the per-allocation figures
/// `compute_realised_gains` sums into the disposal's totals, surfaced so a UI
/// can show which parcel contributed what. Mirrors
/// `parcel_optimiser::HypotheticalAllocation`'s field set (the hypothetical
/// equivalent), but for an actually-recorded allocation.
#[derive(utoipa::ToSchema, Debug, Clone, Serialize, Deserialize)]
pub struct ParcelDetail {
    /// The Buy/DRP trade (or, for a rights sale, the parcel that earned the
    /// rights) this allocation drew from.
    pub purchase_trade_id: TradeId,
    /// Original/deemed acquisition date — the discount-clock anchor
    /// (`ParcelRow::acquired()`).
    pub acquisition_date: NaiveDate,
    /// Units allocated from this parcel (sale-date unit basis).
    pub units: Decimal,
    /// Adjusted cost base of the allocated units, AUD.
    pub cost_base: Decimal,
    /// This allocation's share of the proceeds, AUD.
    pub proceeds: Decimal,
    /// proceeds − `cost_base` (positive = gain, negative = loss).
    pub capital_gain_loss: Decimal,
    /// Held strictly more than 12 months at the sale date (50% CGT discount).
    pub discount_eligible: bool,
    /// The parcel's cost was incurred by **21 September 1999**, so an
    /// individual could have chosen the **indexation method** on this
    /// allocation instead of the 50% discount
    /// (`docs/ato/indexing-the-cost-base.md`). Advisory: nothing this report
    /// or any other computes is changed by it — the 50% discount is applied
    /// throughout. Only a gain carries the flag; indexation cannot be used on
    /// a capital loss, so a loss allocation is `false` however old it is.
    ///
    /// The date tested is the parcel's own **trade date** — when the cost was
    /// actually incurred — not `acquisition_date`, which is the discount
    /// clock's anchor and may be a *deemed* date carried from a rollover or an
    /// inheritance. The two rules are about different things, and the deemed
    /// cases have their own indexation rules this system does not model (see
    /// the *Indexation method* Known limitation).
    pub indexation_eligible: bool,
    /// The indexed cost base of the allocated units, AUD — the alternative to
    /// `cost_base` under the indexation method, so the two methods can be
    /// read side by side. `None` whenever `indexation_eligible` is false.
    /// Reported, never summed: no total on this report or any other includes
    /// it (`reports::indexation_cross_check` is where the comparison is
    /// drawn).
    pub indexed_cost_base: Option<Decimal>,
}

// Per-sale identity: capital_gain_loss == discount_eligible_gain
//                                       + non_discountable_gain − capital_loss.

impl RealisedGainLoss {
    /// The disposal **as it is printed**: every money figure at the cent,
    /// rounded once per parcel, so each surface that shows a disposal — the
    /// Realised Gains response, the annual tax report's disposal schedule and
    /// the net-capital-gain worksheet's gains — reads the same cents and
    /// every printed row's working reaches the figure beside it. The pipeline
    /// itself ([`db_realised_gains_on`]) stays exact; this is the one place a
    /// disposal is rounded.
    ///
    /// - A parcel's **cost base** is its own fact and rounds on its own, so
    ///   the itemised adjustments the tax report prints under it can reach it.
    /// - Its **proceeds** are a share of the one contract-note figure, so they
    ///   round cumulatively in parcel order (each parcel takes the rounded
    ///   running total less the rounded total before it): the shares add up to
    ///   the disposal's own rounded proceeds, never a cent away from the note.
    /// - Its **gain** is that rounded proceeds less that rounded cost base,
    ///   and the disposal's gain and its three CGT buckets are the sums of the
    ///   parcels' — classified by each parcel's own rounded gain.
    ///
    /// Rounding the year's exact buckets once instead (the worksheet's rule
    /// before 2026-10-24) printed a CGT summary a cent away from the disposal
    /// schedule beside it: three BHP parcels' schedule total of 3,304.33
    /// against a long-term gain of 3,304.34. A disposal with no parcel detail
    /// rounds its own figures.
    pub fn at_the_cent(&self) -> Self {
        let mut out = self.clone();
        if out.parcels.is_empty() {
            out.proceeds = to_cents(self.proceeds);
            out.cost_base = to_cents(self.cost_base);
            out.capital_gain_loss = out.proceeds - out.cost_base;
            out.discount_eligible_gain = to_cents(self.discount_eligible_gain);
            out.non_discountable_gain = to_cents(self.non_discountable_gain);
            out.capital_loss = to_cents(self.capital_loss);
            return out;
        }
        let (rounded, totals) = parcels_at_the_cent(
            self.parcels
                .iter()
                .map(|p| (p.proceeds, p.cost_base, p.discount_eligible)),
        );
        for (parcel, (proceeds, cost_base)) in out.parcels.iter_mut().zip(rounded) {
            parcel.proceeds = proceeds;
            parcel.cost_base = cost_base;
            parcel.capital_gain_loss = proceeds - cost_base;
            parcel.indexed_cost_base = parcel.indexed_cost_base.map(to_cents);
        }
        out.proceeds = totals.proceeds;
        out.cost_base = totals.cost_base;
        out.capital_gain_loss = totals.proceeds - totals.cost_base;
        out.discount_eligible_gain = totals.discount_eligible_gain;
        out.non_discountable_gain = totals.non_discountable_gain;
        out.capital_loss = totals.capital_loss;
        out
    }
}

/// A disposal's totals at the cent — what [`parcels_at_the_cent`] sums its
/// rounded parcels to.
#[derive(Debug, Default, Clone, Copy, PartialEq)]
pub(crate) struct DisposalCents {
    pub proceeds: Decimal,
    pub cost_base: Decimal,
    pub discount_eligible_gain: Decimal,
    pub non_discountable_gain: Decimal,
    pub capital_loss: Decimal,
}

/// *The* per-parcel cent rule behind [`RealisedGainLoss::at_the_cent`], over
/// each parcel's exact `(proceeds, cost base, discount-eligible)`: the cost
/// base rounds on its own, the proceeds round cumulatively in the order
/// given, and each parcel's gain — that rounded proceeds less that rounded
/// cost base — lands in one bucket by its own sign and eligibility. Answers
/// each parcel's rounded `(proceeds, cost base)` and the disposal's totals.
/// `pub(crate)` so the net-capital-gain what-if previews a contemplated sale
/// by exactly the rule its recorded Sell will be netted by.
pub(crate) fn parcels_at_the_cent(
    parcels: impl IntoIterator<Item = (Decimal, Decimal, bool)>,
) -> (Vec<(Decimal, Decimal)>, DisposalCents) {
    let mut totals = DisposalCents::default();
    let (mut running, mut rounded_before) = (Decimal::ZERO, Decimal::ZERO);
    let rounded = parcels
        .into_iter()
        .map(|(proceeds, cost_base, discount_eligible)| {
            running += proceeds;
            let rounded_running = to_cents(running);
            let proceeds = rounded_running - rounded_before;
            rounded_before = rounded_running;
            let cost_base = to_cents(cost_base);
            let gain = proceeds - cost_base;
            if gain > Decimal::ZERO && discount_eligible {
                totals.discount_eligible_gain += gain;
            } else if gain > Decimal::ZERO {
                totals.non_discountable_gain += gain;
            } else {
                totals.capital_loss -= gain;
            }
            totals.proceeds += proceeds;
            totals.cost_base += cost_base;
            (proceeds, cost_base)
        })
        .collect();
    (rounded, totals)
}

pub fn router() -> OpenApiRouter<SqlitePool> {
    OpenApiRouter::new().routes(routes!(realised_gains_handler))
}

/// A realisable Sell as the report reads it from `trades`. Each trade carries
/// its own currency and manual `fx_rate` fallback so its amounts convert to
/// AUD via the ATO reference rate (see `infra::fx`); proceeds and cost base
/// convert independently — a buy and the sell that closes it may settle in
/// different months at different rates — so totals are never aggregated
/// across mixed currencies.
#[derive(sqlx::FromRow)]
struct SellInfo {
    id: TradeId,
    listing_id: ListingId,
    holding_account_id: HoldingAccountId,
    date: NaiveDate,
    #[sqlx(try_from = "Money")]
    quantity: Decimal,
    #[sqlx(try_from = "Money")]
    average_price: Decimal,
    #[sqlx(try_from = "Money")]
    brokerage: Decimal,
    #[sqlx(try_from = "Money")]
    gst_on_brokerage: Decimal,
    currency: CurrencyCode,
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
    /// [`scrip_cash_apportionment`], which the Annual Tax Report's worksheet
    /// shares. `None` for ordinary Sells.
    fn scrip_cash_apportionment(&self) -> Result<Option<(Decimal, Decimal)>, sqlx::Error> {
        scrip_cash_apportionment(
            self.id,
            self.scrip_cash_per_unit,
            self.scrip_market_value,
            self.scrip_new_units,
            self.scrip_old_units,
        )
    }
}

/// The `(numerator, denominator)` of a partial-rollover scrip closing Sell's
/// cash-side market-value share of each allocated parcel's reduced cost base —
/// `cash×old / (cash×old + mv×new)`, per
/// `docs/ato/takeovers-and-scrip-for-scrip.md` Example 27. The scrip side's
/// share rolled over into the replacement parcels at exchange time, so only
/// this share is realised against the cash proceeds. Kept as a pair and
/// multiplied before dividing so exact fractions (e.g. Gunther's 1/3) don't
/// round twice. `None` when the action carries no cash component (an ordinary
/// Sell, or an all-scrip rollover whose gain is disregarded).
///
/// A cash component with any of its companion terms missing is a corrupt
/// action row — `corporate_actions` CHECKs `scrip_market_value` and
/// `scrip_cash_per_unit` present together, and a `ScripForScrip` action always
/// carries its exchange ratio — so it is an error, never a silently
/// un-apportioned (and so overstated) cost base.
///
/// Free rather than a [`SellInfo`] method because the Annual Tax Report's
/// worksheet scales the parcel's initial cost base and each itemised
/// adjustment row by the identical fraction — the one `realised_gains` applied
/// to the adjusted figure printed beside them — so its
/// `initial − Σ adjustments = adjusted` identity survives a partial-rollover
/// scrip exchange. Reading the terms from the same
/// `trades ⋈ corporate_actions` join through this one function is what keeps
/// the two apportionments from ever diverging.
pub(crate) fn scrip_cash_apportionment(
    sale_id: TradeId,
    scrip_cash_per_unit: Option<Decimal>,
    scrip_market_value: Option<Decimal>,
    scrip_new_units: Option<Decimal>,
    scrip_old_units: Option<Decimal>,
) -> Result<Option<(Decimal, Decimal)>, sqlx::Error> {
    let Some(cash) = scrip_cash_per_unit else {
        return Ok(None);
    };
    let missing = |col: &str| {
        sqlx::Error::Decode(
            format!("scrip action of sell {sale_id} has a cash component but no {col}").into(),
        )
    };
    let mv = scrip_market_value.ok_or_else(|| missing("scrip_market_value"))?;
    let new = scrip_new_units.ok_or_else(|| missing("scrip_new_units"))?;
    let old = scrip_old_units.ok_or_else(|| missing("scrip_old_units"))?;
    Ok(Some((cash * old, cash * old + mv * new)))
}

/// One parcel-allocation row: `quantity_allocated` units of the sale (in the
/// sale date's unit basis) came from the purchase parcel.
#[derive(sqlx::FromRow)]
struct Allocation {
    /// `parcel_allocations.id`: the key of its as-acquired units in
    /// [`ReportData::acquired_units`].
    id: i64,
    sale_trade_id: TradeId,
    purchase_trade_id: TradeId,
    #[sqlx(try_from = "Money")]
    quantity_allocated: Decimal,
}

/// A rights sale/lapse as the report reads it (`rights_sales` joined to its
/// `RightsIssue` action for the listing and currency): a disposal of the rights
/// themselves (see `entities::rights_sale`). Proceeds are
/// `units × proceeds_per_right`; the cost base is `rights_cost` (nil for
/// rights issued free), apportioned over the anchoring allocations. Both
/// convert to AUD at the sale month's ATO rate (manual `fx_rate` fallback) —
/// the rights' own purchase date isn't tracked, so the cost leg uses the sale
/// date too.
#[derive(sqlx::FromRow)]
struct RightsSaleInfo {
    id: RightsSaleId,
    listing_id: ListingId,
    holding_account_id: HoldingAccountId,
    date: NaiveDate,
    #[sqlx(try_from = "Money")]
    units: Decimal,
    #[sqlx(try_from = "Money")]
    proceeds_per_right: Decimal,
    #[sqlx(try_from = "Money")]
    rights_cost: Decimal,
    currency: CurrencyCode,
    #[sqlx(try_from = "Money")]
    fx_rate: Decimal,
}

/// One rights-sale anchoring row: `units` of the sale's rights were earned by
/// (and take the acquisition date of) the purchase parcel. Consumes nothing.
#[derive(sqlx::FromRow)]
struct RightsAllocation {
    rights_sale_id: RightsSaleId,
    purchase_trade_id: TradeId,
    #[sqlx(try_from = "Money")]
    units: Decimal,
}

/// Everything the realised-gains computation consumes, read in one go by
/// [`load_report_data`] so [`compute_realised_gains`] is a pure function over
/// in-memory data.
#[derive(Default)]
struct ReportData {
    sells: HashMap<TradeId, SellInfo>,
    buys: HashMap<TradeId, ParcelRow>,
    allocations: Vec<Allocation>,
    /// Rights sales/lapses (disposals of the rights themselves) and their
    /// parcel anchorings, surfaced as `source = RightsSale` rows.
    rights_sales: Vec<RightsSaleInfo>,
    rights_allocations: Vec<RightsAllocation>,
    /// AMIT cost-base reductions (CGT event E10) per purchase parcel, one per
    /// AMMA statement adjusting it.
    cba_reduction: HashMap<TradeId, Vec<cost_base::AmitReductionEvent>>,
    /// Return-of-capital payments (CGT event G1) per listing.
    roc_events: HashMap<ListingId, Vec<RocEvent>>,
    /// Share splits/consolidations per listing (quantity re-basing).
    split_events: HashMap<ListingId, Vec<SplitEvent>>,
    /// Each allocation's units in its parcel's as-acquired basis, by
    /// allocation id (`open_parcels::db_acquired_units_by_allocation`): the
    /// parcel's sales walked in date order, so the one emptying it is costed
    /// as exactly its remainder. An allocation missing from it (an in-memory
    /// fixture) is re-based on its own.
    acquired_units: HashMap<i64, AcquiredUnits>,
    /// Every imported ATO FX rate, so per-allocation conversions are map
    /// lookups instead of one DB round-trip each.
    fx: FxRates,
    /// The frozen ATO quarterly CPI series, so the advisory indexed cost base
    /// beside each pre-22-September-1999 allocation's own is a map lookup too.
    cpi: indexation::CpiQuarters,
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
        ParcelRow::columns()
    )))
    .fetch_all(&mut *conn)
    .await?;

    let allocations: Vec<Allocation> = sqlx::query_as(
        "SELECT id, sale_trade_id, purchase_trade_id, quantity_allocated FROM parcel_allocations",
    )
    .fetch_all(&mut *conn)
    .await?;

    let cba_reduction = cost_base::db_cost_base_reduction_events(&mut *conn, None).await?;
    let roc_events = corporate_action::db_return_of_capital_events(&mut *conn).await?;
    // share splits/consolidations per listing (quantity re-basing)
    let split_events = corporate_action::db_share_split_events(&mut *conn).await?;
    let acquired_units = open_parcels::db_acquired_units_by_allocation(&mut *conn)
        .await?
        .into_iter()
        .map(|a| (a.id, a.units))
        .collect();
    let fx = FxRates::load(&mut *conn).await?;
    let cpi = indexation::CpiQuarters::load(&mut *conn).await?;

    Ok(ReportData {
        sells: sells.into_iter().map(|s| (s.id, s)).collect(),
        buys: buys.into_iter().map(|b| (b.id, b)).collect(),
        allocations,
        rights_sales,
        rights_allocations,
        cba_reduction,
        roc_events,
        split_events,
        acquired_units,
        fx,
        cpi,
    })
}

pub async fn db_realised_gains(pool: &SqlitePool) -> Result<Vec<RealisedGainLoss>, sqlx::Error> {
    let mut tx = db::read_tx(pool).await?;
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

/// One allocation's share of a disposal-level total, apportioned by units as a
/// cumulative difference: the entitlement of every unit allocated *so far*,
/// less what earlier allocations already took.
///
/// Two properties have to hold at once, and this form gets both.
///
/// * **The shares add up to `total`.** The differences telescope, so after
///   any allocation the running figure is that allocation's cumulative
///   entitlement, not a sum of independently rounded shares — a division
///   remainder (a third of \$10) lands on the last allocation instead of
///   leaking. And the allocation that *completes* the disposal
///   (`units_so_far == units_total`) is handed the whole total with no
///   division at all, so not even the final `total × units / units` can
///   re-round it. The running figure is the returned entitlement, carried
///   forward as `entitled_before` — **not** a sum of the shares: shares of
///   different magnitude at full precision re-round when added (two
///   replacement parcels' shares summed to 21961.146…666 of a
///   …667 contract note), so a disposal's total is the final entitlement.
/// * **Every share is still the parcel's own pro-rata figure**, so the
///   per-parcel rows a UI drills into mean something on their own.
/// * **The shares add back to the total exactly.** Each running entitlement is
///   rounded onto one grid per total ([`share_grid_scale`]): the total's own
///   scale, widened to as many places as 27 significant digits of it leave.
///   Every entitlement, share and partial sum is then a whole number of that
///   grid step and in range, so `entitled − entitled_before` and the re-adding
///   of the shares are exact. Unrounded, an entitlement after a consolidation
///   could carry a place more than the next one, and three parcels' shares
///   of a 3468.9373 Sell summed to 3468.9373000000000000000000001.
///
/// Apply it to the exact *total* the report publishes (net proceeds, a rights
/// sale's cost), never to one component of it: a component apportioned and
/// then combined with an exact figure re-rounds in the combining step and the
/// telescoping is lost (SCENARIOS Z-a).
pub(crate) fn cumulative_share(
    total: Decimal,
    units_so_far: Decimal,
    units_total: Decimal,
    entitled_before: Decimal,
) -> Apportioned {
    let entitled = if units_so_far == units_total {
        total
    } else {
        mul_div(&[total, units_so_far], units_total).round_dp(share_grid_scale(total))
    };
    Apportioned {
        share: entitled - entitled_before,
        entitled,
    }
}

/// The decimal places [`cumulative_share`] rounds every entitlement of `total`
/// to: as many as 27 significant digits of `total` leave after its integer
/// digits (one short of `Decimal`'s 28, so a sum of shares cannot carry out of
/// range), and never fewer than `total`'s own scale, so the total itself lies
/// on the grid. That scale is the *value's* (trailing zeros normalised away):
/// the recorded Sell's AUD conversion and the what-if's figure write one total
/// with different trailing zeros, and must be handed the same grid.
fn share_grid_scale(total: Decimal) -> u32 {
    let mut integer_digits = 0u32;
    let mut whole = total.abs().trunc();
    while whole >= Decimal::ONE {
        whole = (whole / Decimal::TEN).trunc();
        integer_digits += 1;
    }
    27u32
        .saturating_sub(integer_digits)
        .max(total.normalize().scale())
        .min(Decimal::MAX_SCALE)
}

/// One step of [`cumulative_share`]: this allocation's share, and the
/// cumulative entitlement to carry to the next step (and, after the last, the
/// disposal's total).
#[derive(Debug, Clone, Copy)]
pub(crate) struct Apportioned {
    pub share: Decimal,
    pub entitled: Decimal,
}

/// The gain/loss computation: a pure function over [`ReportData`], so the
/// arithmetic is unit-testable without a database.
fn compute_realised_gains(data: &ReportData) -> Result<Vec<RealisedGainLoss>, sqlx::Error> {
    let mut sale_proceeds: HashMap<TradeId, Decimal> = HashMap::new();
    let mut sale_cost_base: HashMap<TradeId, Decimal> = HashMap::new();
    let mut sale_discount_gain: HashMap<TradeId, Decimal> = HashMap::new();
    let mut sale_non_discount_gain: HashMap<TradeId, Decimal> = HashMap::new();
    let mut sale_loss: HashMap<TradeId, Decimal> = HashMap::new();
    // Per-sale parcel breakdown: the same per-allocation figures computed
    // below, kept instead of only being summed into the totals above, so the
    // report can surface which parcel contributed what.
    let mut sale_parcels: HashMap<TradeId, Vec<ParcelDetail>> = HashMap::new();
    // Running (quantity, proceeds share) already handed to earlier
    // allocations of each sale, for the cumulative-difference apportionment
    // below.
    let mut sale_proceeds_assigned: HashMap<TradeId, (Decimal, Decimal)> = HashMap::new();

    for alloc in &data.allocations {
        let sale_id = alloc.sale_trade_id;
        let qty_alloc = alloc.quantity_allocated;

        let Some(sale) = data.sells.get(&sale_id) else {
            continue;
        };
        let Some(buy) = data.buys.get(&alloc.purchase_trade_id) else {
            continue;
        };

        // Proceeds for this allocation: its share of the sale's own net
        // proceeds, apportioned by qty_alloc / sale_qty via
        // `cumulative_share`.
        //
        // The sale's total is knowable exactly — average_price × quantity −
        // brokerage − gst — so it is computed as a whole and converted to AUD
        // *once*, at the sale's rate (the sale's deliberate spot override when
        // set, else the ATO rate for the sale month, else the sale's manual
        // fx_rate fallback). Every allocation of a sale converts at that one
        // rate, so converting the total once and apportioning the AUD figure
        // is the same money as converting each share — minus one rounded
        // division per allocation.
        //
        // Apportioning the whole figure is what keeps the sale's `proceeds`
        // exact. Pro-rating only the brokerage and then subtracting it from
        // `price × qty_alloc` (the previous form) made every share an exact
        // large number less a 28-significant-digit repeating one; each such
        // subtraction re-rounds to fit Decimal's mantissa, so the shares no
        // longer telescoped to the total (a 1551-unit 10-parcel sale reported
        // 69785.049999999999999999999999 for an exact 69785.05 — a cent low
        // once rounded, and a cent adrift from the same disposal's figure on
        // the Annual Tax Report).
        let alloc_proceeds = if sale.quantity > Decimal::ZERO {
            let sale_total = data.fx.to_aud(
                sale.average_price * sale.quantity - sale.brokerage - sale.gst_on_brokerage,
                &sale.currency,
                sale.date,
                sale.fx_override(),
            )?;
            let (qty_so_far, entitled) = sale_proceeds_assigned
                .entry(sale_id)
                .or_insert((Decimal::ZERO, Decimal::ZERO));
            *qty_so_far += qty_alloc;
            let step = cumulative_share(sale_total, *qty_so_far, sale.quantity, *entitled);
            *entitled = step.entitled;
            step.share
        } else {
            Decimal::ZERO
        };

        // The allocated quantity is in the *sale date's* unit basis; the
        // purchase parcel's quantity and per-unit cost are as transacted. A
        // share split/consolidation between them (TD 2000/10) re-bases the
        // allocation back to as-acquired units so the cost-base pro-rating
        // spreads the unchanged total over the converted unit count.
        let splits = data
            .split_events
            .get(&buy.listing_id)
            .map_or(&[][..], |v| v);
        let qty_alloc_acquired = match data.acquired_units.get(&alloc.id) {
            Some(units) => *units,
            None => corporate_action::as_acquired_quantity(
                UnitsAt::new(qty_alloc, sale.date),
                splits,
                buy.date,
            ),
        };

        // Adjusted cost base of the allocated portion via the shared pipeline
        // (`domain::cost_base`), converted to AUD at the (possibly deemed)
        // acquisition month. `up_to` is the sale date: return-of-capital
        // payments after the sale don't touch these units — they were no
        // longer held.
        let alloc_cost_base = cost_base::adjusted_cost_base(
            &buy.parcel(),
            qty_alloc_acquired,
            data.cba_reduction
                .get(&alloc.purchase_trade_id)
                .map_or(&[][..], |v| v),
            data.roc_events.get(&buy.listing_id).map_or(&[][..], |v| v),
            splits,
            cost_base::Held::DisposedOn(sale.date),
        )?
        .into_aud_with(&data.fx, &buy.currency, buy.acquired(), buy.fx_override())?;
        let alloc_cost = alloc_cost_base.adjusted;

        // A partial-rollover scrip closing Sell realises only the cash
        // side's market-value share of the parcel's reduced cost base; the
        // scrip side's share rolled over into the replacement parcels (the
        // exchange carried exactly the complement — the other half of the
        // same `apportion` — so the two sum to the full reduced cost base).
        let cash_apportionment = sale.scrip_cash_apportionment()?;
        let alloc_cost = match cash_apportionment {
            Some((num, den)) => apportion(alloc_cost, num, den).share,
            None => alloc_cost,
        };

        let alloc_gain = alloc_proceeds - alloc_cost;

        // The advisory indexation figure (`domain::indexation`): what this
        // allocation's cost base would have been under the indexation method,
        // for the cross-check report to set beside `alloc_cost`. Offered only
        // where the method is actually available — the cost was incurred by
        // 21 September 1999 *and* the allocation produced a gain, since
        // indexation can never be used on a capital loss. Nothing below sums
        // it: every total this report reports is the discount method's.
        let indexation = if alloc_gain > Decimal::ZERO {
            data.cpi.indexation_for(buy.date)
        } else {
            None
        };
        let indexed_cost_base = indexation.map(|ix| {
            let indexed = indexation::indexed_cost_base(&alloc_cost_base, ix.factor);
            // The cash apportionment is a share of the *whole* cost base, so
            // it applies to the indexed figure exactly as it did above.
            match cash_apportionment {
                Some((num, den)) => apportion(indexed, num, den).share,
                None => indexed,
            }
        });

        // The running entitlement, not a sum of the shares (see
        // `cumulative_share`): the contract-note total once every allocation
        // is in.
        sale_proceeds.insert(
            sale_id,
            sale_proceeds_assigned
                .get(&sale_id)
                .map_or(Decimal::ZERO, |&(_, entitled)| entitled),
        );
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
            indexation_eligible: indexed_cost_base.is_some(),
            indexed_cost_base,
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
                disposal: DisposalId::Sell(sale_id),
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
                taxpayer_basis: taxpayer::TAXPAYER_BASIS.to_owned(),
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
        // Both legs convert at the sale month (manual fx_rate fallback): the
        // rights' own purchase date isn't tracked, so the cost leg has no
        // earlier anchor. Both totals are exact — proceeds_per_right × units,
        // and the carried rights_cost as recorded — so each is converted once
        // and then apportioned by units via `cumulative_share`, exactly as the
        // Sell path apportions its net proceeds. Converting per allocation
        // instead would round each division separately, so a non-AUD rights
        // sale's shares need not have summed to the converted total.
        let fx = FxOverride::Fallback(sale.fx_rate);
        let total_proceeds = data.fx.to_aud(
            sale.proceeds_per_right * sale.units,
            &sale.currency,
            sale.date,
            fx,
        )?;
        let total_cost = data
            .fx
            .to_aud(sale.rights_cost, &sale.currency, sale.date, fx)?;
        let mut units_so_far = Decimal::ZERO;
        let mut cost_entitled = Decimal::ZERO;
        let mut proceeds_entitled = Decimal::ZERO;
        for alloc in data
            .rights_allocations
            .iter()
            .filter(|a| a.rights_sale_id == sale.id)
        {
            let Some(buy) = data.buys.get(&alloc.purchase_trade_id) else {
                continue;
            };
            units_so_far += alloc.units;
            let (alloc_proceeds, alloc_cost) = if sale.units > Decimal::ZERO {
                let p =
                    cumulative_share(total_proceeds, units_so_far, sale.units, proceeds_entitled);
                let c = cumulative_share(total_cost, units_so_far, sale.units, cost_entitled);
                proceeds_entitled = p.entitled;
                cost_entitled = c.entitled;
                (p.share, c.share)
            } else {
                (Decimal::ZERO, Decimal::ZERO)
            };

            let alloc_gain = alloc_proceeds - alloc_cost;
            // The running entitlements, not sums of the shares (see
            // `cumulative_share`).
            proceeds = proceeds_entitled;
            cost_base = cost_entitled;
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
                // A rights disposal is deliberately outside the indexation
                // comparison. What would be indexed is the *rights'* own cost
                // base — nil for the free rights this action models, and
                // indexing nil gives nil — and the anchoring parcel's date is
                // the discount clock's, not the date any cost of the rights
                // was incurred. There is no honest figure to put here, so
                // none is put.
                indexation_eligible: false,
                indexed_cost_base: None,
            });
        }
        parcels.sort_by(|a, b| {
            a.acquisition_date
                .cmp(&b.acquisition_date)
                .then(a.purchase_trade_id.cmp(&b.purchase_trade_id))
        });
        result.push(RealisedGainLoss {
            disposal: DisposalId::RightsSale(sale.id),
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
            taxpayer_basis: taxpayer::TAXPAYER_BASIS.to_owned(),
        });
    }

    result.sort_by(|a, b| {
        a.sale_date
            .cmp(&b.sale_date)
            .then(a.disposal.cmp(&b.disposal))
    });
    Ok(result)
}

#[utoipa::path(
    get,
    path = "/portfolio/realised_gains",
    summary = "Realised gains and losses per disposal."
)]
async fn realised_gains_handler(
    State(pool): State<SqlitePool>,
) -> Result<Json<Vec<RealisedGainLoss>>, ApiError> {
    // Served at the cent, as the tax report prints and the worksheet nets it.
    let rows = db_realised_gains(&pool).await?;
    Ok(Json(
        rows.iter().map(RealisedGainLoss::at_the_cent).collect(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::entities::demerger::Demerge;
    use crate::test_support::holding_account_row;

    use crate::entities::listing_rename::RenameBody;
    use crate::entities::scrip_exchange::Exchange;
    use crate::entities::sell::AllocationInput;
    use crate::entities::transfer::TransferBody;
    use crate::entities::{
        corporate_action, demerger, holding_account, listing, listing_rename, rba_fx_rate,
        scrip_exchange, trade, transfer,
    };
    use crate::infra::decimal;
    use crate::reports::open_parcels;
    use crate::test_support::{self, ApiClient, allocate, test_pool};
    use axum::http::StatusCode;

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
        listing_id: ListingId,
        date: NaiveDate,
        qty: Decimal,
        price: Decimal,
    ) {
        test_support::buy(id, listing_id.0)
            .date(date)
            .qty(qty)
            .price(price)
            .insert(pool)
            .await;
    }

    async fn insert_sell(
        pool: &SqlitePool,
        id: i64,
        listing_id: ListingId,
        date: NaiveDate,
        qty: Decimal,
        price: Decimal,
    ) {
        test_support::sell(id, listing_id.0)
            .date(date)
            .qty(qty)
            .price(price)
            .insert(pool)
            .await;
    }

    /// A decimal literal, for the exact-figure assertions.
    use crate::test_support::dec;

    /// One column of a disposal's per-parcel rows, summed — the property the
    /// sale's own totals have to equal exactly.
    fn parcel_sum(r: &RealisedGainLoss, f: impl Fn(&ParcelDetail) -> Decimal) -> Decimal {
        r.parcels.iter().map(f).sum()
    }

    /// The SCENARIOS Z-a shape at its simplest: three equal 517-unit parcels
    /// of a 1,551-unit sale, so each brokerage share is a repeating decimal.
    fn three_equal_parcel_sale(price: i64, brokerage: &str) -> Vec<RealisedGainLoss> {
        let d = |y, m, day| NaiveDate::from_ymd_opt(y, m, day).unwrap();
        let mut sell = mem_sell(9, d(2026, 5, 15), 1551, price, "AUD");
        sell.brokerage = dec(brokerage);
        let mut buys: HashMap<TradeId, ParcelRow> = HashMap::new();
        let mut allocations = Vec::new();
        for id in 1..=3 {
            buys.insert(TradeId(id), mem_buy(id, d(2015, 3, 4), 517, 1, "AUD"));
            allocations.push(mem_alloc(9, id, 517));
        }
        compute_realised_gains(&ReportData {
            sells: [(TradeId(9), sell)].into(),
            buys,
            allocations,
            ..ReportData::default()
        })
        .expect("no FX needed")
    }

    // Pure-computation tests: `compute_realised_gains` over in-memory
    // `ReportData` — no pool. The arithmetic seam the load/compute split
    // exists for.

    fn mem_sell(id: i64, date: NaiveDate, qty: i64, price: i64, currency: &str) -> SellInfo {
        SellInfo {
            id: TradeId(id),
            listing_id: ListingId(1),
            holding_account_id: HoldingAccountId(1),
            date,
            quantity: Decimal::from(qty),
            average_price: Decimal::from(price),
            brokerage: Decimal::ZERO,
            gst_on_brokerage: Decimal::ZERO,
            currency: currency.to_owned().into(),
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
            id: TradeId(id),
            listing_id: ListingId(1),
            holding_account_id: HoldingAccountId(1),
            date,
            settlement_date: date,
            quantity: AcquiredUnits::new(Decimal::from(qty)),
            average_price: Decimal::from(price),
            brokerage: Decimal::ZERO,
            gst_on_brokerage: Decimal::ZERO,
            currency: currency.to_owned().into(),
            fx_rate: Decimal::ONE,
            spot_fx_rate: None,
            deemed_acquisition_date: None,
            scrip_action_id: None,
            demerger_action_id: None,
            transfer_id: None,
            demerger_head_listing_id: None,
            registered_from: None,
        }
    }

    fn mem_alloc(sale: i64, buy: i64, qty: i64) -> Allocation {
        Allocation {
            id: sale * 1000 + buy,
            sale_trade_id: TradeId(sale),
            purchase_trade_id: TradeId(buy),
            quantity_allocated: Decimal::from(qty),
        }
    }

    #[test]
    fn pure_mixed_eligibility_and_identity_without_a_pool() {
        let d = |y, m, day| NaiveDate::from_ymd_opt(y, m, day).unwrap();
        // Old parcel (>12mo, gain 500), new parcel (≤12mo, gain 250) — same
        // facts as `db_two_parcels_mixed_eligibility`, straight from memory.
        let data = ReportData {
            sells: [(TradeId(3), mem_sell(3, d(2025, 6, 2), 150, 15, "AUD"))].into(),
            buys: [
                (TradeId(1), mem_buy(1, d(2023, 1, 3), 100, 10, "AUD")),
                (TradeId(2), mem_buy(2, d(2025, 1, 1), 50, 10, "AUD")),
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
            sells: [(TradeId(2), mem_sell(2, d(2025, 6, 2), 100, 15, "USD"))].into(),
            buys: [(TradeId(1), mem_buy(1, d(2024, 1, 15), 100, 10, "USD"))].into(),
            allocations: vec![mem_alloc(2, 1, 100)],
            fx: FxRates::from_rates([
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
        let mut sell = mem_sell(2, d(2025, 6, 2), 100, 15, "USD");
        sell.fx_rate = "0.60".parse().unwrap();
        let mut buy = mem_buy(1, d(2024, 1, 15), 100, 10, "USD");
        buy.fx_rate = "0.50".parse().unwrap();
        let data = ReportData {
            sells: [(TradeId(2), sell)].into(),
            buys: [(TradeId(1), buy)].into(),
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
        let mut sell = mem_sell(2, d(2025, 6, 2), 100, 15, "USD");
        sell.spot_fx_rate = Some("0.75".parse().unwrap());
        let mut buy = mem_buy(1, d(2024, 1, 15), 100, 10, "USD");
        buy.spot_fx_rate = Some("0.40".parse().unwrap());
        let data = ReportData {
            sells: [(TradeId(2), sell)].into(),
            buys: [(TradeId(1), buy)].into(),
            allocations: vec![mem_alloc(2, 1, 100)],
            fx: FxRates::from_rates([
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
        // would sum to 9.999…, not 10. The cumulative-difference
        // apportionment hands the remainder to the last allocation, so total
        // proceeds come out exactly 3 × $4 − $10 = $2.
        //
        // The $4 price used to matter: the shares were `price × qty − share`,
        // and a larger price pushed that subtraction past Decimal's 28-digit
        // mantissa, re-rounding it and masking what this test asserts
        // (SCENARIOS Z-a). The whole net figure is apportioned now, so the
        // price is arbitrary — `pure_equal_parcel_shares_sum_exactly_at_a_
        // large_price` is this same shape at $45.00 and 1,551 units.
        let mut sell = mem_sell(4, d(2025, 6, 2), 3, 4, "AUD");
        sell.brokerage = Decimal::from(10);
        let data = ReportData {
            sells: [(TradeId(4), sell)].into(),
            buys: [
                (TradeId(1), mem_buy(1, d(2024, 1, 1), 1, 1, "AUD")),
                (TradeId(2), mem_buy(2, d(2024, 2, 1), 1, 1, "AUD")),
                (TradeId(3), mem_buy(3, d(2024, 3, 1), 1, 1, "AUD")),
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

    /// A disposal's own proceeds are `price × quantity − brokerage − gst`,
    /// and the report must publish exactly that figure however many parcels
    /// it was allocated across (SCENARIOS Z-a).
    ///
    /// Three equal parcels of 517 units at $45.00 less $9.95 brokerage is
    /// exactly $69,785.05. Pro-rating only the brokerage and subtracting the
    /// share from `price × qty` reported 69,785.049999999999999999999999
    /// here: each share was an exact large number less a 28-significant-digit
    /// repeating one, and every such subtraction re-rounds, so the shares
    /// stopped telescoping to the total.
    #[test]
    fn pure_equal_parcel_shares_sum_exactly_at_a_large_price() {
        let r = &three_equal_parcel_sale(45, "9.95")[0];
        assert_eq!(r.proceeds, dec("69785.05"));
        assert_eq!(parcel_sum(r, |p| p.proceeds), r.proceeds);
    }

    /// The same drift at a small price, which is the control that says it is
    /// the apportionment and not the magnitude: 3 × 517 units at $4.00 less
    /// $9.95 is exactly $6,194.05, and used to report
    /// 6,194.0499999999999999999999999.
    #[test]
    fn pure_equal_parcel_shares_sum_exactly_at_a_small_price() {
        let r = &three_equal_parcel_sale(4, "9.95")[0];
        assert_eq!(r.proceeds, dec("6194.05"));
        assert_eq!(parcel_sum(r, |p| p.proceeds), r.proceeds);
    }

    /// Control: with nothing to apportion, the same three parcels were always
    /// exact — 3 × 517 × $45.00 = $69,795.00 — and still are.
    #[test]
    fn pure_equal_parcel_shares_sum_exactly_without_brokerage() {
        let r = &three_equal_parcel_sale(45, "0")[0];
        assert_eq!(r.proceeds, dec("69795.00"));
        assert_eq!(parcel_sum(r, |p| p.proceeds), r.proceeds);
    }

    /// Control: with nothing to apportion the brokerage *across*, the figure
    /// was always exact too — one 1,551-unit parcel takes the whole $9.95.
    #[test]
    fn pure_single_parcel_proceeds_are_exact() {
        let d = |y, m, day| NaiveDate::from_ymd_opt(y, m, day).unwrap();
        let mut sell = mem_sell(9, d(2026, 5, 15), 1551, 45, "AUD");
        sell.brokerage = dec("9.95");
        let data = ReportData {
            sells: [(TradeId(9), sell)].into(),
            buys: [(TradeId(1), mem_buy(1, d(2015, 3, 4), 1551, 25, "AUD"))].into(),
            allocations: vec![mem_alloc(9, 1, 1551)],
            ..ReportData::default()
        };
        let result = compute_realised_gains(&data).unwrap();
        assert_eq!(result[0].proceeds, dec("69785.05"));
    }

    /// SCENARIOS Z-a in full, on the disposal that found it: 1,551 units over
    /// **ten** parcels, sold at $45.00 less $9.95 brokerage.
    ///
    /// Proceeds are exactly $69,785.05 and the cost base exactly
    /// $39,139.975, so the gain is exactly $30,645.075 — a half cent, which
    /// is what made the drift visible on screen. Rounded for display that is
    /// $30,645.08, the figure the annual tax report and the net-capital-gain
    /// report print; the Realised Gains screen printed $30,645.07 from a
    /// gain of 30,645.074999999999999999999999, a cent adrift both from the
    /// other two screens and from its own discount-eligible +
    /// non-discountable columns beside it (W-d's "columns do not add up").
    ///
    /// The two properties that have to hold together are both asserted: the
    /// sale's totals are the exact arithmetic figures, *and* the ten
    /// per-parcel rows still sum to each of them exactly.
    #[test]
    fn pure_ten_parcel_disposal_reports_its_exact_proceeds_and_gain() {
        let d = |y, m, day| NaiveDate::from_ymd_opt(y, m, day).unwrap();
        let mut sell = mem_sell(99, d(2026, 5, 15), 1551, 45, "AUD");
        sell.brokerage = dec("9.95");

        // Ten parcels totalling 1,551 units for a $39,139.975 cost base:
        // eight of 155 units at $25.00 ($31,000.00), one of 156 at $27.00
        // plus $15.00 brokerage ($4,227.00) bought inside 12 months of the
        // sale — so its gain is the non-discountable one — and one of 155 at
        // $25.245 ($3,912.975), which is where the half cent comes from.
        let mut buys: HashMap<TradeId, ParcelRow> = HashMap::new();
        let mut allocations = Vec::new();
        for id in 1..=8 {
            buys.insert(TradeId(id), mem_buy(id, d(2015, 3, 4), 155, 25, "AUD"));
            allocations.push(mem_alloc(99, id, 155));
        }
        let mut recent = mem_buy(9, d(2025, 11, 3), 156, 27, "AUD");
        recent.brokerage = Decimal::from(15);
        buys.insert(TradeId(9), recent);
        allocations.push(mem_alloc(99, 9, 156));
        let mut half_cent = mem_buy(10, d(2016, 8, 9), 155, 25, "AUD");
        half_cent.average_price = dec("25.245");
        buys.insert(TradeId(10), half_cent);
        allocations.push(mem_alloc(99, 10, 155));

        let data = ReportData {
            sells: [(TradeId(99), sell)].into(),
            buys,
            allocations,
            ..ReportData::default()
        };
        let result = compute_realised_gains(&data).unwrap();
        assert_eq!(result.len(), 1);
        let r = &result[0];
        assert_eq!(r.parcels.len(), 10);

        // 1551 × 45.00 − 9.95, and 30,645.075 against the stated cost base.
        assert_eq!(r.proceeds, dec("69785.05"));
        assert_eq!(r.cost_base, dec("39139.975"));
        assert_eq!(r.capital_gain_loss, dec("30645.075"));
        // The cent the two screens disagreed on.
        assert_eq!(decimal::to_cents(r.capital_gain_loss), dec("30645.08"));

        // The per-parcel rows still add up to each of the sale's totals.
        assert_eq!(parcel_sum(r, |p| p.proceeds), r.proceeds);
        assert_eq!(parcel_sum(r, |p| p.cost_base), r.cost_base);
        assert_eq!(parcel_sum(r, |p| p.capital_gain_loss), r.capital_gain_loss);
        // ...and the row's own gain columns add up to its gain.
        assert_eq!(
            r.discount_eligible_gain + r.non_discountable_gain - r.capital_loss,
            r.capital_gain_loss
        );
        assert!(r.non_discountable_gain > Decimal::ZERO, "the 2025 parcel");
    }

    /// A non-AUD multi-parcel disposal: every allocation of a sale converts
    /// at the sale's one rate, so converting the exact total once and
    /// apportioning the AUD figure is the same money as converting each
    /// share — with one rounded division instead of one per allocation.
    /// 1,551 units at US$45.00 less US$9.95 is US$69,785.05, which at 0.50
    /// USD per AUD is exactly A$139,570.10.
    #[test]
    fn pure_non_aud_multi_parcel_proceeds_convert_the_exact_total_once() {
        let d = |y, m, day| NaiveDate::from_ymd_opt(y, m, day).unwrap();
        let mut sell = mem_sell(5, d(2026, 5, 15), 1551, 45, "USD");
        sell.brokerage = dec("9.95");
        let mut buys: HashMap<TradeId, ParcelRow> = HashMap::new();
        let mut allocations = Vec::new();
        for (i, qty) in [517i64, 517, 517].into_iter().enumerate() {
            let id = i as i64 + 1;
            let mut buy = mem_buy(id, d(2015, 3, 4), qty, 20, "USD");
            buy.currency = "USD".into();
            buys.insert(TradeId(id), buy);
            allocations.push(mem_alloc(5, id, qty));
        }
        let data = ReportData {
            sells: [(TradeId(5), sell)].into(),
            buys,
            allocations,
            fx: FxRates::from_rates([
                ("USD", "2015-03", dec("0.50")),
                ("USD", "2026-05", dec("0.50")),
            ]),
            ..ReportData::default()
        };
        let result = compute_realised_gains(&data).unwrap();
        let r = &result[0];
        // US$69,785.05 / 0.50, and 3 × 517 × US$20.00 / 0.50 = A$62,040.00.
        assert_eq!(r.proceeds, dec("139570.10"));
        assert_eq!(r.cost_base, dec("62040.00"));
        assert_eq!(r.capital_gain_loss, dec("77530.10"));
        assert_eq!(parcel_sum(r, |p| p.proceeds), r.proceeds);
        assert_eq!(parcel_sum(r, |p| p.cost_base), r.cost_base);
    }

    /// The finding's own two-screens comparison, end to end through the API:
    /// one disposal's rounded gain must read the same on Realised Gains
    /// (`GET /portfolio/realised_gains`, which answers the exact decimal the
    /// screen rounds) and on the [annual tax
    /// report](crate::reports::tax_report) (`GET /reports/tax_report`, which
    /// cent-rounds each parcel row and prints the sum of those, SCENARIOS
    /// W-d) — Z-a is those two printing 30,645.07 and 30,645.08 for one
    /// disposal.
    ///
    /// 1,551 units at $45.00 less $9.95 brokerage over three parcels is
    /// exactly $69,785.05; against a $38,803.175 cost base the gain is
    /// exactly $30,981.875 — a half cent, which is what makes a drift of
    /// 1e-24 visible at the cent. The pipeline's exact figure is now
    /// 30,981.875 (before the Z-a fix, 30,981.874999999999999999999999).
    /// Since 2026-10-24 both screens print the disposal at the cent, from
    /// one rule (`RealisedGainLoss::at_the_cent`): the cost base rounds per
    /// parcel to $38,803.18 and the gain is struck from the printed figures,
    /// $30,981.87 on both — where rounding the exact gain on its own printed
    /// $30,981.88 beside proceeds and a cost base that subtract to .87.
    #[tokio::test]
    async fn api_one_disposal_reports_one_rounded_gain_on_both_screens() {
        use crate::infra::decimal::to_cents;
        let d = |y, m, day| NaiveDate::from_ymd_opt(y, m, day).unwrap();
        let pool = test_pool().await;
        insert_listing(&pool, 1, "VDHG").await;
        // 200 + 115 + 1,236 = 1,551 units. The middle parcel's $25.245 price
        // is where the half cent comes from, and its 2025 date makes its gain
        // the non-discountable one.
        let parcels = [
            (1i64, 200i64, "25.00", d(2015, 3, 4)),
            (2, 115, "25.245", d(2025, 11, 3)),
            (3, 1236, "25.00", d(2016, 8, 9)),
        ];
        for (id, qty, price, date) in parcels {
            insert_buy(
                &pool,
                id,
                ListingId(1),
                date,
                Decimal::from(qty),
                dec(price),
            )
            .await;
        }
        test_support::sell(4, 1)
            .date(d(2026, 5, 15))
            .qty(Decimal::from(1551))
            .price(Decimal::from(45))
            .brokerage(dec("9.95"))
            .insert(&pool)
            .await;
        for (id, qty, _, _) in parcels {
            allocate(&pool, id, 4, id, Decimal::from(qty)).await;
        }

        // The pipeline is exact: a half-cent cost base and gain.
        let exact = db_realised_gains(&pool).await.unwrap();
        assert_eq!(exact[0].proceeds, dec("69785.05"));
        assert_eq!(exact[0].cost_base, dec("38803.175"));
        assert_eq!(exact[0].capital_gain_loss, dec("30981.875"));

        // The screen prints it at the cent, the cost base rounded per parcel
        // and the gain struck from the printed figures, so the row subtracts.
        let client = ApiClient::full(&pool);
        let rows: Vec<RealisedGainLoss> = client.get_json("/portfolio/realised_gains").await;
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].proceeds, dec("69785.05"));
        assert_eq!(rows[0].cost_base, dec("38803.18"));
        assert_eq!(rows[0].capital_gain_loss, dec("30981.87"));

        let report: serde_json::Value = client.get_json("/reports/tax_report?tax_year=2026").await;
        let subtotal = &report["disposals"]["listings"][0]["subtotal"];
        // The same disposal, so the two screens have to land on the same
        // cents — the tax report from the sum of its rounded rows, Realised
        // Gains from rounding the exact figure.
        for (column, screen) in [
            ("proceeds_aud", rows[0].proceeds),
            ("cost_base_aud", rows[0].cost_base),
            ("gain_loss_aud", rows[0].capital_gain_loss),
        ] {
            assert_eq!(
                subtotal[column],
                serde_json::json!(to_cents(screen).to_string()),
                "{column} must read the same on both screens"
            );
        }
        assert_eq!(subtotal["gain_loss_aud"], serde_json::json!("30981.87"));
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
        let buy_date = NaiveDate::from_ymd_opt(2024, 1, 2).unwrap();
        let sell_date = NaiveDate::from_ymd_opt(2025, 3, 3).unwrap();
        insert_listing(&pool, 1, "VAS").await;
        insert_buy(
            &pool,
            1,
            ListingId(1),
            buy_date,
            Decimal::from(100),
            Decimal::from(10),
        )
        .await;
        insert_sell(
            &pool,
            2,
            ListingId(1),
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
        let buy_date = NaiveDate::from_ymd_opt(2024, 1, 2).unwrap();
        let sell_date = NaiveDate::from_ymd_opt(2024, 6, 3).unwrap();
        insert_listing(&pool, 1, "VAS").await;
        insert_buy(
            &pool,
            1,
            ListingId(1),
            buy_date,
            Decimal::from(100),
            Decimal::from(10),
        )
        .await;
        insert_sell(
            &pool,
            2,
            ListingId(1),
            sell_date,
            Decimal::from(100),
            Decimal::from(15),
        )
        .await;
        allocate(&pool, 1, 2, 1, Decimal::from(100)).await;

        let result = db_realised_gains(&pool).await.unwrap();
        assert_eq!(result.len(), 1);
        let r = &result[0];
        assert_eq!(r.disposal, DisposalId::Sell(TradeId(2)));
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
        let buy_date = NaiveDate::from_ymd_opt(2024, 1, 2).unwrap();
        let sell_date = NaiveDate::from_ymd_opt(2024, 6, 3).unwrap();
        insert_listing(&pool, 1, "VAS").await;
        insert_buy(
            &pool,
            1,
            ListingId(1),
            buy_date,
            Decimal::from(100),
            Decimal::from(15),
        )
        .await;
        insert_sell(
            &pool,
            2,
            ListingId(1),
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
        let buy_date = NaiveDate::from_ymd_opt(2024, 1, 2).unwrap();
        let sell_date = NaiveDate::from_ymd_opt(2024, 6, 3).unwrap();
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
        let buy_date = NaiveDate::from_ymd_opt(2024, 1, 2).unwrap();
        let sell_date = NaiveDate::from_ymd_opt(2025, 1, 3).unwrap(); // strictly > 12 months
        insert_listing(&pool, 1, "VAS").await;
        insert_buy(
            &pool,
            1,
            ListingId(1),
            buy_date,
            Decimal::from(100),
            Decimal::from(10),
        )
        .await;
        insert_sell(
            &pool,
            2,
            ListingId(1),
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
        let buy_date = NaiveDate::from_ymd_opt(2024, 1, 2).unwrap();
        let sell_date = NaiveDate::from_ymd_opt(2025, 1, 3).unwrap(); // strictly > 12 months
        insert_listing(&pool, 1, "OLD").await;
        insert_buy(
            &pool,
            1,
            ListingId(1),
            buy_date,
            Decimal::from(100),
            Decimal::from(10),
        )
        .await;
        // The security is renamed before the sale: same listing id, new
        // ticker — recorded via the rename action (a bare PUT is refused
        // once the listing has a trade; see
        // entities::listing::UpsertError::IdentityChangeRequiresRename).
        listing_rename::db_rename(
            &pool,
            ListingId(1),
            &RenameBody {
                effective_date: NaiveDate::from_ymd_opt(2024, 6, 3).unwrap(),
                ticker: "NEW".to_owned(),
                exchange_mic: None,
                name: Some("NEW".to_owned()),
                price_symbol: None,
                note: None,
            },
        )
        .await
        .unwrap();
        insert_sell(
            &pool,
            2,
            ListingId(1),
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
        let sell_date = NaiveDate::from_ymd_opt(2025, 1, 3).unwrap(); // strictly > 12 months
        let qty: Decimal = "0.12345678".parse().unwrap();
        insert_buy(&pool, 1, ListingId(1), buy_date, qty, Decimal::from(60000)).await;
        insert_sell(
            &pool,
            2,
            ListingId(1),
            sell_date,
            qty,
            Decimal::from(100000),
        )
        .await;
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
        let buy_date = NaiveDate::from_ymd_opt(2024, 1, 2).unwrap();
        let sell_date = NaiveDate::from_ymd_opt(2025, 1, 2).unwrap(); // exactly 12 months — not eligible
        insert_listing(&pool, 1, "VAS").await;
        insert_buy(
            &pool,
            1,
            ListingId(1),
            buy_date,
            Decimal::from(100),
            Decimal::from(10),
        )
        .await;
        insert_sell(
            &pool,
            2,
            ListingId(1),
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

    /// A 29 February acquisition has no anniversary in the following non-leap
    /// year: excluding both the acquisition day and the event day, ownership
    /// to 28 February 2025 is 364 days, so the first discountable disposal
    /// day is 1 March. Both parcels are bought on the same leap day and sold
    /// a day apart, so only the boundary separates them (SCENARIOS C-02, the
    /// report-level half of `domain::cgt_discount`'s own boundary test).
    #[tokio::test]
    async fn db_leap_day_parcel_becomes_discountable_on_1_march() {
        let pool = test_pool().await;
        let leap_day = NaiveDate::from_ymd_opt(2024, 2, 29).unwrap();
        insert_listing(&pool, 1, "LEAP").await;
        insert_buy(
            &pool,
            1,
            ListingId(1),
            leap_day,
            Decimal::from(100),
            Decimal::from(10),
        )
        .await;
        insert_buy(
            &pool,
            2,
            ListingId(1),
            leap_day,
            Decimal::from(100),
            Decimal::from(10),
        )
        .await;
        // Sale 3: 28 Feb 2025 — a day short. Sale 4: 1 Mar 2025 — over the line.
        insert_sell(
            &pool,
            3,
            ListingId(1),
            NaiveDate::from_ymd_opt(2025, 2, 28).unwrap(),
            Decimal::from(100),
            Decimal::from(15),
        )
        .await;
        insert_sell(
            &pool,
            4,
            ListingId(1),
            NaiveDate::from_ymd_opt(2025, 3, 3).unwrap(),
            Decimal::from(100),
            Decimal::from(15),
        )
        .await;
        allocate(&pool, 1, 3, 1, Decimal::from(100)).await;
        allocate(&pool, 2, 4, 2, Decimal::from(100)).await;

        let result = db_realised_gains(&pool).await.unwrap();
        let by_sale = |id: i64| {
            result
                .iter()
                .find(|r| r.disposal == DisposalId::Sell(TradeId::from(id)))
                .unwrap_or_else(|| panic!("no row for sale {id}"))
        };
        assert_eq!(by_sale(3).capital_gain_loss, Decimal::from(500));
        assert_eq!(by_sale(3).discount_eligible_gain, Decimal::ZERO);
        assert_eq!(by_sale(3).non_discountable_gain, Decimal::from(500));
        assert_eq!(by_sale(4).capital_gain_loss, Decimal::from(500));
        assert_eq!(by_sale(4).discount_eligible_gain, Decimal::from(500));
        assert_eq!(by_sale(4).non_discountable_gain, Decimal::ZERO);
    }

    /// CGT event A1 happens on the **contract** date, not on settlement
    /// (`docs/ato/cgt-discount.md`, `docs/ato/cgt-event-timing.md`), and the
    /// clock likewise starts on the buy's contract date. Here both settlement
    /// dates sit on the far side of the 12-month line while both contract
    /// dates sit on or inside it: a report reading settlement dates would
    /// discount this gain, and the correct answer is that it does not
    /// (SCENARIOS C-03).
    #[tokio::test]
    async fn db_discount_clock_runs_on_contract_dates_not_settlement() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "VAS").await;
        // Contracted 2024-01-01, settled T+2.
        test_support::buy(1, 1)
            .date(NaiveDate::from_ymd_opt(2024, 1, 2).unwrap())
            .settlement(NaiveDate::from_ymd_opt(2024, 1, 3).unwrap())
            .qty(Decimal::from(100))
            .price(Decimal::from(10))
            .insert(&pool)
            .await;
        // Sold on the anniversary itself — not eligible — settling five days
        // later, which is past the line on any settlement-based reading
        // (settlement-to-settlement: 2024-01-03 → 2025-01-06).
        test_support::sell(2, 1)
            .date(NaiveDate::from_ymd_opt(2025, 1, 2).unwrap())
            .settlement(NaiveDate::from_ymd_opt(2025, 1, 6).unwrap())
            .qty(Decimal::from(100))
            .price(Decimal::from(15))
            .insert(&pool)
            .await;
        allocate(&pool, 1, 2, 1, Decimal::from(100)).await;

        let result = db_realised_gains(&pool).await.unwrap();
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].capital_gain_loss, Decimal::from(500));
        assert_eq!(result[0].discount_eligible_gain, Decimal::ZERO);
        assert_eq!(result[0].non_discountable_gain, Decimal::from(500));
        // The per-allocation row anchors on the parcel's contract date too.
        assert_eq!(
            result[0].parcels[0].acquisition_date,
            NaiveDate::from_ymd_opt(2024, 1, 2).unwrap()
        );
        assert!(!result[0].parcels[0].discount_eligible);
    }

    /// Crypto has no exchange calendar, so a parcel settles the day it is
    /// contracted — the clock is the same 12-month rule and the same
    /// boundary (`docs/ato/crypto-cgt.md`). Sold a day short of the
    /// anniversary the gain is non-discountable; the eligible side is
    /// `db_crypto_sale_keeps_satoshi_precision_and_discount` (SCENARIOS C-17).
    #[tokio::test]
    async fn db_crypto_sold_a_day_short_of_the_anniversary_is_not_discountable() {
        let pool = test_pool().await;
        test_support::listing(1)
            .crypto()
            .ticker("BTC")
            .name("Bitcoin")
            .insert(&pool)
            .await;
        let buy_date = NaiveDate::from_ymd_opt(2024, 1, 1).unwrap();
        let qty: Decimal = "0.5".parse().unwrap();
        test_support::buy(1, 1)
            .date(buy_date)
            .settlement(buy_date)
            .qty(qty)
            .price(Decimal::from(60000))
            .insert(&pool)
            .await;
        // 31 Dec 2024 — one day inside the 12 months, settling same-day.
        let sell_date = NaiveDate::from_ymd_opt(2024, 12, 31).unwrap();
        test_support::sell(2, 1)
            .date(sell_date)
            .settlement(sell_date)
            .qty(qty)
            .price(Decimal::from(100000))
            .insert(&pool)
            .await;
        allocate(&pool, 1, 2, 1, qty).await;

        let result = db_realised_gains(&pool).await.unwrap();
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].capital_gain_loss, Decimal::from(20000));
        assert_eq!(result[0].discount_eligible_gain, Decimal::ZERO);
        assert_eq!(result[0].non_discountable_gain, Decimal::from(20000));
    }

    #[tokio::test]
    async fn db_loss_not_included_in_discount_eligible() {
        let pool = test_pool().await;
        // parcel held > 12 months but sold at a loss → discount_eligible_gain stays 0
        let buy_date = NaiveDate::from_ymd_opt(2024, 1, 2).unwrap();
        let sell_date = NaiveDate::from_ymd_opt(2025, 6, 2).unwrap();
        insert_listing(&pool, 1, "VAS").await;
        insert_buy(
            &pool,
            1,
            ListingId(1),
            buy_date,
            Decimal::from(100),
            Decimal::from(15),
        )
        .await;
        insert_sell(
            &pool,
            2,
            ListingId(1),
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
        let old_date = NaiveDate::from_ymd_opt(2023, 1, 3).unwrap();
        let new_date = NaiveDate::from_ymd_opt(2025, 1, 2).unwrap();
        let sell_date = NaiveDate::from_ymd_opt(2025, 6, 2).unwrap();
        insert_listing(&pool, 1, "VAS").await;
        insert_buy(
            &pool,
            1,
            ListingId(1),
            old_date,
            Decimal::from(100),
            Decimal::from(10),
        )
        .await;
        insert_buy(
            &pool,
            2,
            ListingId(1),
            new_date,
            Decimal::from(50),
            Decimal::from(10),
        )
        .await;
        insert_sell(
            &pool,
            3,
            ListingId(1),
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
        let buy_date = NaiveDate::from_ymd_opt(2024, 1, 2).unwrap();
        let sell_date = NaiveDate::from_ymd_opt(2025, 6, 2).unwrap();
        insert_listing(&pool, 1, "VAF").await;
        insert_buy(
            &pool,
            1,
            ListingId(1),
            buy_date,
            Decimal::from(100),
            Decimal::from(10),
        )
        .await;
        insert_sell(
            &pool,
            2,
            ListingId(1),
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

    /// A transfer's zero-proceeds closing Sell is an account move, not a
    /// disposal: it must not count in the AMIT held/spill split and so must
    /// not retroactively change an *earlier* real Sell's cost base. The
    /// statement's partial-coverage row covers the 600 units still held at
    /// its year end (they are — in the replacement parcel), spilling nothing
    /// onto the 400 sold in March.
    #[tokio::test]
    async fn db_a_transfer_does_not_change_an_earlier_sells_realised_gain() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "VDHG").await;
        holding_account::db_upsert(&pool, &holding_account_row(2, "Broker"))
            .await
            .unwrap();
        let d = |y, m, day| NaiveDate::from_ymd_opt(y, m, day).unwrap();
        // 1,000 units at $10; a real Sell of 400 at $12 on 2025-03-03.
        insert_buy(
            &pool,
            1,
            ListingId(1),
            d(2024, 1, 10),
            dec("1000"),
            dec("10"),
        )
        .await;
        insert_sell(&pool, 2, ListingId(1), d(2025, 3, 3), dec("400"), dec("12")).await;
        allocate(&pool, 1, 2, 1, dec("400")).await;
        // FY2025 statement ($1/unit) covering the remaining 600 units.
        test_support::amma(6, 1)
            .units(dec("600"))
            .cost_base_adjustment(dec("1"))
            .with(|a| a.tax_year_end_date = d(2025, 6, 30))
            .insert(&pool)
            .await;
        test_support::amit_adjustment(&pool, 1, 6, 1, dec("600")).await;

        let march_gain = |rows: &[RealisedGainLoss]| {
            rows.iter()
                .find(|r| r.disposal == DisposalId::Sell(TradeId(2)))
                .expect("the March sale")
                .capital_gain_loss
        };
        let before = db_realised_gains(&pool).await.unwrap();
        // proceeds 4,800 − cost 4,000 (no spill: the row covers the held 600)
        assert_eq!(march_gain(&before), dec("800"));

        // Transferring the 600 held units to another account, dated before
        // the year end, changes nothing about the March disposal.
        transfer::db_transfer(
            &pool,
            1,
            &TransferBody {
                listing_id: ListingId(1),
                date: d(2025, 5, 1),
                from_account_id: HoldingAccountId(1),
                to_account_id: HoldingAccountId(2),
                allocations: vec![AllocationInput {
                    purchase_trade_id: TradeId(1),
                    quantity_allocated: dec("600"),
                }],
                fee_allocations: Vec::new(),
                fee_market_price: None,
                fee_fx_rate: None,
            },
        )
        .await
        .unwrap();
        let after = db_realised_gains(&pool).await.unwrap();
        assert_eq!(march_gain(&after), dec("800"));
        // The transfer's closing Sell itself still never appears as a
        // disposal.
        assert_eq!(after.len(), before.len());
    }

    #[tokio::test]
    async fn db_sorted_by_sale_date() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "VAS").await;
        let d = |y, m, d| NaiveDate::from_ymd_opt(y, m, d).unwrap();
        insert_buy(
            &pool,
            1,
            ListingId(1),
            d(2023, 1, 3),
            Decimal::from(100),
            Decimal::from(10),
        )
        .await;
        insert_buy(
            &pool,
            2,
            ListingId(1),
            d(2023, 1, 3),
            Decimal::from(100),
            Decimal::from(10),
        )
        .await;
        // sell 2 comes before sell 3 by date but is inserted second
        insert_sell(
            &pool,
            3,
            ListingId(1),
            d(2024, 6, 3),
            Decimal::from(50),
            Decimal::from(15),
        )
        .await;
        insert_sell(
            &pool,
            4,
            ListingId(1),
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
        assert_eq!(result[0].disposal, DisposalId::Sell(TradeId(4)));
        assert_eq!(result[1].disposal, DisposalId::Sell(TradeId(3)));
    }

    // API-level test

    #[tokio::test]
    async fn api_realised_gains_returns_json() {
        let pool = test_pool().await;
        let buy_date = NaiveDate::from_ymd_opt(2024, 1, 2).unwrap();
        let sell_date = NaiveDate::from_ymd_opt(2025, 6, 2).unwrap();
        insert_listing(&pool, 1, "VAS").await;
        insert_buy(
            &pool,
            1,
            ListingId(1),
            buy_date,
            Decimal::from(100),
            Decimal::from(10),
        )
        .await;
        insert_sell(
            &pool,
            2,
            ListingId(1),
            sell_date,
            Decimal::from(100),
            Decimal::from(15),
        )
        .await;
        allocate(&pool, 1, 2, 1, Decimal::from(100)).await;

        let resp = ApiClient::over(router().with_state(pool))
            .get("/portfolio/realised_gains")
            .await;
        assert_eq!(resp.status, StatusCode::OK);
        let result: Vec<RealisedGainLoss> = resp.json();
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].capital_gain_loss, Decimal::from(500));
        // held > 12 months
        assert_eq!(result[0].discount_eligible_gain, Decimal::from(500));
    }

    /// SCENARIOS P-12: the report splits every disposal's gain by 12-month
    /// discount eligibility, so each row states the taxpayer basis that split
    /// is for — the assumption is never left implicit (the tax-summary and
    /// net-capital-gain rows state theirs the same way).
    #[tokio::test]
    async fn api_realised_gains_states_the_taxpayer_basis() {
        let pool = test_pool().await;
        let buy_date = NaiveDate::from_ymd_opt(2024, 1, 2).unwrap();
        let sell_date = NaiveDate::from_ymd_opt(2025, 6, 2).unwrap();
        insert_listing(&pool, 1, "VAS").await;
        insert_buy(
            &pool,
            1,
            ListingId(1),
            buy_date,
            Decimal::from(100),
            Decimal::from(10),
        )
        .await;
        insert_sell(
            &pool,
            2,
            ListingId(1),
            sell_date,
            Decimal::from(100),
            Decimal::from(15),
        )
        .await;
        allocate(&pool, 1, 2, 1, Decimal::from(100)).await;

        let result: Vec<RealisedGainLoss> = ApiClient::over(router().with_state(pool))
            .get_json("/portfolio/realised_gains")
            .await;
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].taxpayer_basis, taxpayer::TAXPAYER_BASIS);
    }

    // Return of capital (CGT event G1)

    async fn apply_roc(
        pool: &SqlitePool,
        id: i64,
        listing_id: ListingId,
        date: NaiveDate,
        amount: &str,
    ) {
        apply_roc_with_record(pool, id, listing_id, date, amount, None).await;
    }

    /// [`apply_roc`] with the record date that fixed entitlement to the
    /// payment (`None` = not recorded, so the payment date decides).
    async fn apply_roc_with_record(
        pool: &SqlitePool,
        id: i64,
        listing_id: ListingId,
        date: NaiveDate,
        amount: &str,
        record_date: Option<NaiveDate>,
    ) {
        corporate_action::db_upsert(
            pool,
            &corporate_action::CorporateAction {
                id,
                listing_id,
                date,
                kind: corporate_action::ActionKind::ReturnOfCapital {
                    amount_per_unit: amount.parse().unwrap(),
                    currency: "AUD".into(),
                    record_date,
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
        listing_id: ListingId,
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
            ListingId(1),
            NaiveDate::from_ymd_opt(2024, 1, 2).unwrap(),
            Decimal::from(100),
            Decimal::from(10),
        )
        .await;
        // 2-for-1 split, then sell all 200 post-split units @ $6.
        apply_split(
            &pool,
            1,
            ListingId(1),
            NaiveDate::from_ymd_opt(2024, 3, 1).unwrap(),
            "2",
            "1",
        )
        .await;
        insert_sell(
            &pool,
            2,
            ListingId(1),
            NaiveDate::from_ymd_opt(2024, 6, 3).unwrap(),
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
            ListingId(1),
            NaiveDate::from_ymd_opt(2024, 1, 2).unwrap(),
            Decimal::from(100),
            Decimal::from(10),
        )
        .await;
        apply_split(
            &pool,
            1,
            ListingId(1),
            NaiveDate::from_ymd_opt(2024, 3, 1).unwrap(),
            "2",
            "1",
        )
        .await;
        insert_sell(
            &pool,
            2,
            ListingId(1),
            NaiveDate::from_ymd_opt(2024, 6, 3).unwrap(),
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
            ListingId(1),
            NaiveDate::from_ymd_opt(2024, 1, 2).unwrap(),
            Decimal::from(100),
            Decimal::from(10),
        )
        .await;
        // 1-for-10 bonus issue 16 months later: 100 → 110 units.
        corporate_action::db_upsert(
            &pool,
            &corporate_action::CorporateAction {
                id: 1,
                listing_id: ListingId(1),
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
            ListingId(1),
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
            ListingId(1),
            NaiveDate::from_ymd_opt(2024, 1, 2).unwrap(),
            Decimal::from(100),
            Decimal::from(10),
        )
        .await;
        apply_split(
            &pool,
            1,
            ListingId(1),
            NaiveDate::from_ymd_opt(2025, 5, 1).unwrap(),
            "2",
            "1",
        )
        .await;
        insert_sell(
            &pool,
            2,
            ListingId(1),
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
            ListingId(1),
            NaiveDate::from_ymd_opt(2024, 1, 2).unwrap(),
            Decimal::from(100),
            Decimal::from(10),
        )
        .await;
        apply_split(
            &pool,
            1,
            ListingId(1),
            NaiveDate::from_ymd_opt(2024, 3, 1).unwrap(),
            "2",
            "1",
        )
        .await;
        // 25c per post-split unit while all 200 are held.
        apply_roc(
            &pool,
            2,
            ListingId(1),
            NaiveDate::from_ymd_opt(2024, 4, 1).unwrap(),
            "0.25",
        )
        .await;
        insert_sell(
            &pool,
            2,
            ListingId(1),
            NaiveDate::from_ymd_opt(2024, 6, 3).unwrap(),
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
            ListingId(1),
            NaiveDate::from_ymd_opt(2024, 1, 2).unwrap(),
            Decimal::from(100),
            Decimal::from(10),
        )
        .await;
        apply_roc(
            &pool,
            1,
            ListingId(1),
            NaiveDate::from_ymd_opt(2024, 3, 1).unwrap(),
            "0.50",
        )
        .await;
        insert_sell(
            &pool,
            2,
            ListingId(1),
            NaiveDate::from_ymd_opt(2024, 6, 3).unwrap(),
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
            ListingId(1),
            NaiveDate::from_ymd_opt(2024, 1, 2).unwrap(),
            Decimal::from(100),
            Decimal::from(10),
        )
        .await;
        insert_sell(
            &pool,
            2,
            ListingId(1),
            NaiveDate::from_ymd_opt(2024, 6, 3).unwrap(),
            Decimal::from(100),
            Decimal::from(12),
        )
        .await;
        allocate(&pool, 1, 2, 1, Decimal::from(100)).await;
        // Payment after the sale.
        apply_roc(
            &pool,
            1,
            ListingId(1),
            NaiveDate::from_ymd_opt(2024, 7, 1).unwrap(),
            "0.50",
        )
        .await;

        let result = db_realised_gains(&pool).await.unwrap();
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].cost_base, Decimal::from(1000));
        assert_eq!(result[0].capital_gain_loss, Decimal::from(200));
    }

    /// The two ends of the window are independent (SCENARIOS B-09/B-10): a
    /// parcel entitled at the record date but *sold* before the payment is
    /// still unaffected — CGT event G1 adjusts the cost base of the shares
    /// owned at the time of the payment — while the parcel bought after the
    /// record date and held through the payment is unaffected for the other
    /// reason, having never been entitled to it.
    #[tokio::test]
    async fn db_return_of_capital_needs_both_entitlement_and_holding_at_payment() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "RAP").await;
        // Entitled (bought before the 2025-02-10 record date), sold inside the
        // window before the 2025-03-01 payment.
        insert_buy(
            &pool,
            1,
            ListingId(1),
            NaiveDate::from_ymd_opt(2025, 2, 3).unwrap(),
            Decimal::from(100),
            Decimal::from(10),
        )
        .await;
        insert_sell(
            &pool,
            2,
            ListingId(1),
            NaiveDate::from_ymd_opt(2025, 2, 20).unwrap(),
            Decimal::from(100),
            Decimal::from(12),
        )
        .await;
        allocate(&pool, 1, 2, 1, Decimal::from(100)).await;
        // Ex-entitlement (bought after the record date), held past the payment
        // and sold later.
        insert_buy(
            &pool,
            3,
            ListingId(1),
            NaiveDate::from_ymd_opt(2025, 2, 18).unwrap(),
            Decimal::from(100),
            Decimal::from(10),
        )
        .await;
        insert_sell(
            &pool,
            4,
            ListingId(1),
            NaiveDate::from_ymd_opt(2025, 6, 2).unwrap(),
            Decimal::from(100),
            Decimal::from(12),
        )
        .await;
        allocate(&pool, 3, 4, 3, Decimal::from(100)).await;
        apply_roc_with_record(
            &pool,
            1,
            ListingId(1),
            NaiveDate::from_ymd_opt(2025, 3, 1).unwrap(),
            "0.50",
            Some(NaiveDate::from_ymd_opt(2025, 2, 10).unwrap()),
        )
        .await;

        let result = db_realised_gains(&pool).await.unwrap();
        assert_eq!(result.len(), 2);
        for row in &result {
            assert_eq!(row.cost_base, Decimal::from(1000));
            assert_eq!(row.capital_gain_loss, Decimal::from(200));
        }
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
        let buy_date = NaiveDate::from_ymd_opt(2024, 1, 16).unwrap();
        let sell_date = NaiveDate::from_ymd_opt(2025, 6, 2).unwrap();
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
        let buy_date = NaiveDate::from_ymd_opt(2024, 1, 16).unwrap();
        let sell_date = NaiveDate::from_ymd_opt(2025, 6, 2).unwrap();
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

    /// A disposal whose settlement window crosses a rate month converts at the
    /// **contract** month's rate on both legs — the settlement date never
    /// enters an FX conversion (SCENARIOS M-09). The movement between the two
    /// months is the CGT event K10/K11 amount this system deliberately does
    /// not compute (`docs/API.md` Known limitations); pinning the contract
    /// month here keeps that omission a decision rather than a drift.
    #[tokio::test]
    async fn db_settlement_crossing_a_rate_month_converts_at_the_contract_month() {
        let pool = test_pool().await;
        insert_usd_listing(&pool, 1, "AAPL").await;
        for (month, rate) in [
            ("2024-01", "0.50"),
            ("2024-03", "0.60"),
            ("2024-04", "0.40"),
        ] {
            rba_fx_rate::db_import_rate(&pool, "USD", month, rate.parse().unwrap())
                .await
                .unwrap();
        }
        insert_usd_trade(
            &pool,
            1,
            trade::TradeType::Buy,
            NaiveDate::from_ymd_opt(2024, 1, 16).unwrap(),
            Decimal::from(100),
            Decimal::from(10),
        )
        .await;
        // Contracted 27 March, settled 2 April: the April rate (0.40) would
        // give A$3750 of proceeds against March's (0.60) A$2500.
        test_support::sell(2, 1)
            .date(NaiveDate::from_ymd_opt(2024, 3, 27).unwrap())
            .settlement(NaiveDate::from_ymd_opt(2024, 4, 2).unwrap())
            .qty(Decimal::from(100))
            .price(Decimal::from(15))
            .currency("USD")
            .fx_rate("0.99".parse().unwrap())
            .insert(&pool)
            .await;
        allocate(&pool, 1, 2, 1, Decimal::from(100)).await;

        let result = db_realised_gains(&pool).await.unwrap();
        assert_eq!(result.len(), 1);
        // US$1500 / 0.60 (March, the contract month) = A$2500 — not A$3750.
        assert_eq!(result[0].proceeds, Decimal::from(2500));
        assert_eq!(result[0].cost_base, Decimal::from(2000));
        assert_eq!(result[0].capital_gain_loss, Decimal::from(500));
    }

    /// Take over listing `from` with listing `to`, 2 new units per 1 old, on
    /// `date`, and run the exchange. Returns the created group.
    async fn exchange_two_for_one(
        pool: &SqlitePool,
        action_id: i64,
        from: i64,
        to: i64,
        date: NaiveDate,
    ) -> Exchange {
        corporate_action::db_upsert(
            pool,
            &corporate_action::CorporateAction {
                id: action_id,
                listing_id: ListingId(from),
                date,
                kind: corporate_action::ActionKind::ScripForScrip {
                    scrip_listing_id: ListingId(to),
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
        scrip_exchange::db_exchange(pool, action_id).await.unwrap()
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
            ListingId(1),
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
        let buy_date = NaiveDate::from_ymd_opt(2023, 1, 17).unwrap();
        // Gunther: 100 shares, $9 cost base each = $900.
        insert_buy(
            &pool,
            1,
            ListingId(1),
            buy_date,
            Decimal::from(100),
            Decimal::from(9),
        )
        .await;
        // 1-for-1 plus $10 cash per old share; new shares worth $20.
        corporate_action::db_upsert(
            &pool,
            &corporate_action::CorporateAction {
                id: 10,
                listing_id: ListingId(1),
                date: NaiveDate::from_ymd_opt(2024, 7, 10).unwrap(),
                kind: corporate_action::ActionKind::ScripForScrip {
                    scrip_listing_id: ListingId(2),
                    scrip_new_units: Decimal::ONE,
                    scrip_old_units: Decimal::ONE,
                    scrip_cash_per_unit: Some(Decimal::from(10)),
                    scrip_market_value: Some(Decimal::from(20)),
                    scrip_cash_currency: Some("AUD".into()),
                },
            },
        )
        .await
        .unwrap();
        scrip_exchange::db_exchange(&pool, 10).await.unwrap();

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
            sells: [(TradeId(2), sell)].into(),
            buys: [(TradeId(1), mem_buy(1, d(2023, 1, 15), 100, 9, "AUD"))].into(),
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
            ListingId(1),
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
        insert_sell(
            &pool,
            50,
            ListingId(2),
            sell_date,
            Decimal::from(2000),
            Decimal::ONE,
        )
        .await;
        allocate(&pool, 1, 50, ex.replacements[0].id.0, Decimal::from(2000)).await;

        let result = db_realised_gains(&pool).await.unwrap();
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].disposal, DisposalId::Sell(TradeId(50)));
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
        let sell_date = NaiveDate::from_ymd_opt(2024, 12, 2).unwrap();
        insert_listing(&pool, 1, "OLD").await;
        insert_listing(&pool, 2, "NEW").await;
        insert_buy(
            &pool,
            1,
            ListingId(1),
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

        insert_sell(
            &pool,
            50,
            ListingId(2),
            sell_date,
            Decimal::from(2000),
            Decimal::ONE,
        )
        .await;
        allocate(&pool, 1, 50, ex.replacements[0].id.0, Decimal::from(2000)).await;

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
        let buy_date = NaiveDate::from_ymd_opt(2024, 1, 16).unwrap();
        let sell_date = NaiveDate::from_ymd_opt(2025, 6, 2).unwrap();
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
        allocate(&pool, 1, 50, ex.replacements[0].id.0, Decimal::from(200)).await;

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
    ) -> Demerge {
        corporate_action::db_upsert(
            pool,
            &corporate_action::CorporateAction {
                id: action_id,
                listing_id: ListingId(from),
                date,
                kind: corporate_action::ActionKind::Demerger {
                    demerger_listing_id: ListingId(to),
                    demerger_new_units: Decimal::ONE,
                    demerger_held_units: Decimal::from(5),
                    demerger_cost_base_pct: Decimal::from(20),
                    demerger_close_date: None,
                    demerger_close_price: None,
                    demerger_close_sourced_from: None,
                    demerger_close_reason: None,
                },
            },
        )
        .await
        .unwrap();
        demerger::db_demerge(pool, action_id).await.unwrap()
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
            ListingId(1),
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
            ListingId(1),
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
            ListingId(1),
            sell_date,
            Decimal::from(1000),
            Decimal::from(2),
        )
        .await;
        allocate(
            &pool,
            1,
            50,
            dm.head_replacements[0].id.0,
            Decimal::from(1000),
        )
        .await;
        insert_sell(
            &pool,
            51,
            ListingId(2),
            sell_date,
            Decimal::from(200),
            Decimal::from(3),
        )
        .await;
        allocate(
            &pool,
            2,
            51,
            dm.demerged_replacements[0].id.0,
            Decimal::from(200),
        )
        .await;

        let mut result = db_realised_gains(&pool).await.unwrap();
        result.sort_by_key(|r| r.disposal);
        assert_eq!(result.len(), 2);
        // Head: $2,000 − $1,200 = $800, discount-eligible from Oct 2020.
        assert_eq!(result[0].disposal, DisposalId::Sell(TradeId(50)));
        assert_eq!(result[0].cost_base, Decimal::from(1200));
        assert_eq!(result[0].capital_gain_loss, Decimal::from(800));
        assert_eq!(result[0].discount_eligible_gain, Decimal::from(800));
        assert_eq!(result[0].non_discountable_gain, Decimal::ZERO);
        // Demerged: $600 − $300 = $300, also discount-eligible from Oct 2020.
        assert_eq!(result[1].disposal, DisposalId::Sell(TradeId(51)));
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
            id: RightsSaleId(id),
            listing_id: ListingId(1),
            holding_account_id: HoldingAccountId(1),
            date,
            units: Decimal::from(units),
            proceeds_per_right: proceeds_per_right.parse().unwrap(),
            rights_cost: rights_cost.parse().unwrap(),
            currency: "AUD".into(),
            fx_rate: Decimal::ONE,
        }
    }

    fn mem_rights_alloc(sale: i64, buy: i64, units: i64) -> RightsAllocation {
        RightsAllocation {
            rights_sale_id: RightsSaleId(sale),
            purchase_trade_id: TradeId(buy),
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
                (TradeId(1), mem_buy(1, d(2023, 1, 3), 1000, 2, "AUD")), // > 12 months
                (TradeId(2), mem_buy(2, d(2025, 3, 1), 200, 2, "AUD")),  // ≤ 12 months
            ]
            .into(),
            rights_sales: vec![mem_rights_sale(7, d(2025, 5, 1), 300, "0.20", "0")],
            rights_allocations: vec![mem_rights_alloc(7, 1, 250), mem_rights_alloc(7, 2, 50)],
            ..ReportData::default()
        };

        let result = compute_realised_gains(&data).unwrap();
        assert_eq!(result.len(), 1);
        let r = &result[0];
        assert_eq!(r.disposal, DisposalId::RightsSale(RightsSaleId(7)));
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

    /// A non-AUD rights sale's per-allocation shares sum exactly to the
    /// sale's converted totals (SCENARIOS Z-a's property, on the other
    /// disposal shape).
    ///
    /// The defect reached this path by a different route: nothing here is
    /// apportioned *and then* combined with an exact figure, but each
    /// allocation's leg used to be converted to AUD on its own, so every
    /// share carried its own rounded division and the three need not have
    /// summed to the converted total. US$0.20 × 3 rights at 0.60 USD per AUD
    /// is exactly A$1.00 and came out 0.9999999999999999999999999999; the
    /// US$10.00 carried cost is exactly 10/0.6 AUD and came out one ulp
    /// short. Both totals are converted once now, then apportioned by units.
    #[test]
    fn pure_non_aud_rights_sale_shares_sum_exactly_to_the_converted_totals() {
        let d = |y, m, day| NaiveDate::from_ymd_opt(y, m, day).unwrap();
        let mut sale = mem_rights_sale(7, d(2024, 8, 1), 3, "0.20", "10");
        sale.currency = "USD".into();
        let data = ReportData {
            buys: [
                (TradeId(1), mem_buy(1, d(2024, 1, 1), 100, 2, "AUD")),
                (TradeId(2), mem_buy(2, d(2024, 2, 1), 100, 2, "AUD")),
                (TradeId(3), mem_buy(3, d(2024, 3, 1), 100, 2, "AUD")),
            ]
            .into(),
            rights_sales: vec![sale],
            rights_allocations: vec![
                mem_rights_alloc(7, 1, 1),
                mem_rights_alloc(7, 2, 1),
                mem_rights_alloc(7, 3, 1),
            ],
            fx: FxRates::from_rates([("USD", "2024-08", dec("0.60"))]),
            ..ReportData::default()
        };

        let result = compute_realised_gains(&data).unwrap();
        let r = &result[0];
        assert_eq!(r.proceeds, Decimal::ONE);
        assert_eq!(r.cost_base, Decimal::from(10) / dec("0.60"));
        assert_eq!(r.capital_gain_loss, r.proceeds - r.cost_base);
        assert_eq!(parcel_sum(r, |p| p.proceeds), r.proceeds);
        assert_eq!(parcel_sum(r, |p| p.cost_base), r.cost_base);
        assert_eq!(r.capital_loss, r.cost_base - r.proceeds);
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
                (TradeId(1), mem_buy(1, d(2024, 1, 1), 100, 2, "AUD")),
                (TradeId(2), mem_buy(2, d(2024, 2, 1), 100, 2, "AUD")),
                (TradeId(3), mem_buy(3, d(2024, 3, 1), 100, 2, "AUD")),
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
            ListingId(1),
            NaiveDate::from_ymd_opt(2023, 1, 17).unwrap(),
            Decimal::from(1000),
            Decimal::from(2),
        )
        .await;
        corporate_action::db_upsert(
            &pool,
            &corporate_action::CorporateAction {
                id: 10,
                listing_id: ListingId(1),
                date: NaiveDate::from_ymd_opt(2024, 7, 1).unwrap(),
                kind: corporate_action::ActionKind::RightsIssue {
                    rights_units: Decimal::ONE,
                    rights_held_units: Decimal::from(4),
                    exercise_price: "1.80".parse().unwrap(),
                    currency: "AUD".into(),
                    renounceable: true,
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
                holding_account_id: HoldingAccountId(1),
                allocations: vec![rights_sale::AllocationInput {
                    purchase_trade_id: TradeId(1),
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
            ListingId(1),
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
            .find(|r| matches!(r.disposal, DisposalId::RightsSale(_)))
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
            NaiveDate::from_ymd_opt(2023, 1, 17).unwrap()
        );
        assert_eq!(rights.parcels[0].units, Decimal::from(250));
        assert_eq!(rights.parcels[0].proceeds, Decimal::from(50));
        assert_eq!(rights.parcels[0].cost_base, Decimal::ZERO);
        assert!(rights.parcels[0].discount_eligible);
        let sell = result
            .iter()
            .find(|r| matches!(r.disposal, DisposalId::Sell(_)))
            .unwrap();
        assert_eq!(sell.disposal, DisposalId::Sell(TradeId(2)));
        assert_eq!(sell.capital_gain_loss, Decimal::from(100));
        assert_eq!(sell.parcels.len(), 1);
        assert_eq!(sell.parcels[0].purchase_trade_id, 1);
        assert_eq!(sell.parcels[0].units, Decimal::from(100));
        assert_eq!(sell.parcels[0].capital_gain_loss, Decimal::from(100));

        // The rights sale is `rights_sales` row 1 and trade 1 is the Buy it
        // anchors to: the same integer in two id spaces. The id travels with
        // the variant naming its table, in the JSON as `source` +
        // `disposal_id`, and nothing answers a bare `sale_trade_id` that a
        // client could read as a trade.
        assert_eq!(rights.disposal, DisposalId::RightsSale(RightsSaleId(1)));
        let json = serde_json::to_value(&result).unwrap();
        let shapes: Vec<_> = json
            .as_array()
            .unwrap()
            .iter()
            .map(|r| {
                assert!(r.get("sale_trade_id").is_none(), "{r}");
                assert!(r.get("disposal").is_none(), "flattened: {r}");
                (r["source"].clone(), r["disposal_id"].clone())
            })
            .collect();
        assert_eq!(
            shapes,
            vec![
                (serde_json::json!("RightsSale"), serde_json::json!(1)),
                (serde_json::json!("Sell"), serde_json::json!(2)),
            ]
        );
        let back: Vec<RealisedGainLoss> = serde_json::from_value(json).unwrap();
        assert_eq!(back[0].disposal, rights.disposal);
        assert_eq!(back[1].disposal, sell.disposal);
    }

    // Sale scenarios (SCENARIOS.md section D).

    /// SCENARIOS D-09. Bought and sold on the one day: a real disposal with a
    /// real gain, assessed in full — the discount needs *more* than 12 months
    /// (`domain::cgt_discount`), so a zero-day holding is the "other" method.
    #[tokio::test]
    async fn db_same_day_buy_and_sell_is_assessed_without_the_discount() {
        let pool = test_pool().await;
        let day = NaiveDate::from_ymd_opt(2024, 6, 3).unwrap();
        insert_listing(&pool, 1, "SDY").await;
        insert_buy(
            &pool,
            1,
            ListingId(1),
            day,
            Decimal::from(100),
            Decimal::from(10),
        )
        .await;
        insert_sell(
            &pool,
            2,
            ListingId(1),
            day,
            Decimal::from(100),
            Decimal::from(15),
        )
        .await;
        allocate(&pool, 1, 2, 1, Decimal::from(100)).await;

        let result = db_realised_gains(&pool).await.unwrap();
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].capital_gain_loss, Decimal::from(500));
        assert_eq!(result[0].non_discountable_gain, Decimal::from(500));
        assert_eq!(result[0].discount_eligible_gain, Decimal::ZERO);
        assert!(!result[0].parcels[0].discount_eligible);
    }

    /// SCENARIOS D-13. An AMMA statement for the year the sale fell in adjusts
    /// the cost base of the parcel that sale disposed of, even though its year
    /// end is months after the sale: s 104-107B makes the annual adjustment
    /// "just before the end of the income year, **or just before the time of a
    /// relevant CGT event**" (LCR 2015/11 para 13,
    /// `docs/ato/amit-cost-base-adjustments.md`). Entering the statement
    /// afterwards restates the sale's gain — as it should.
    #[tokio::test]
    async fn db_amit_statement_for_the_sale_year_adjusts_the_parcel_already_sold() {
        let pool = test_pool().await;
        test_support::listing(1)
            .ticker("VDHG")
            .amit(true)
            .insert(&pool)
            .await;
        insert_buy(
            &pool,
            1,
            ListingId(1),
            NaiveDate::from_ymd_opt(2023, 1, 10).unwrap(),
            Decimal::from(100),
            Decimal::from(10),
        )
        .await;
        // Sold in October 2023 — inside the year ended 30 June 2024.
        insert_sell(
            &pool,
            2,
            ListingId(1),
            NaiveDate::from_ymd_opt(2023, 10, 3).unwrap(),
            Decimal::from(100),
            Decimal::from(15),
        )
        .await;
        allocate(&pool, 1, 2, 1, Decimal::from(100)).await;

        let before = db_realised_gains(&pool).await.unwrap();
        assert_eq!(before[0].cost_base, Decimal::from(1000));
        assert_eq!(before[0].capital_gain_loss, Decimal::from(500));

        test_support::amma(1, 1)
            .cost_base_adjustment("0.50".parse().unwrap())
            .insert(&pool)
            .await;
        test_support::amit_adjustment(&pool, 1, 1, 1, Decimal::from(100)).await;

        let after = db_realised_gains(&pool).await.unwrap();
        assert_eq!(after[0].cost_base, Decimal::from(950));
        assert_eq!(after[0].capital_gain_loss, Decimal::from(550));
    }

    /// SCENARIOS F-25 — "the most common ETF tax error": a fund held for
    /// several years, each year's AMMA reducing the parcel's cost base, then
    /// sold. The sale's cost base must carry **every** prior year's reduction,
    /// not just the latest, and the year-of-sale statement (entered months
    /// after the sale, covering units already gone) restates it once more.
    #[tokio::test]
    async fn db_a_multi_year_amit_holding_carries_every_years_reduction_into_the_sale() {
        let pool = test_pool().await;
        test_support::listing(1)
            .ticker("VDHG")
            .amit(true)
            .insert(&pool)
            .await;
        insert_buy(
            &pool,
            1,
            ListingId(1),
            NaiveDate::from_ymd_opt(2020, 8, 3).unwrap(),
            Decimal::from(1000),
            Decimal::from(50),
        )
        .await;
        // Four full years of attribution: 10c, 15c, 20c, 25c per unit.
        for (id, year, per_unit) in [
            (1, 2021, "0.10"),
            (2, 2022, "0.15"),
            (3, 2023, "0.20"),
            (4, 2024, "0.25"),
        ] {
            test_support::amma(id, 1)
                .units(Decimal::from(1000))
                .cost_base_adjustment(per_unit.parse().unwrap())
                .with(|a| {
                    a.tax_year_end_date = NaiveDate::from_ymd_opt(year, 6, 30).unwrap();
                    a.date_received = NaiveDate::from_ymd_opt(year, 8, 29).unwrap();
                })
                .insert(&pool)
                .await;
            test_support::amit_adjustment(&pool, id, id, 1, Decimal::from(1000)).await;
        }
        // Sold in November 2024, inside FY2025.
        insert_sell(
            &pool,
            2,
            ListingId(1),
            NaiveDate::from_ymd_opt(2024, 11, 1).unwrap(),
            Decimal::from(1000),
            Decimal::from(80),
        )
        .await;
        allocate(&pool, 1, 2, 1, Decimal::from(1000)).await;

        // 50,000 − 1000 × (0.10 + 0.15 + 0.20 + 0.25) = 49,300.
        let gains = db_realised_gains(&pool).await.unwrap();
        assert_eq!(gains.len(), 1);
        assert_eq!(gains[0].cost_base, Decimal::from(49300));
        assert_eq!(gains[0].capital_gain_loss, Decimal::from(30700));
        assert_eq!(gains[0].discount_eligible_gain, Decimal::from(30700));

        // The FY2025 statement lands in September 2025, after the sale: it
        // covers the units disposed of during its year (LCR 2015/11 para 13),
        // and the sale's cost base moves again.
        test_support::amma(5, 1)
            .units(Decimal::ZERO)
            .cost_base_adjustment("0.30".parse().unwrap())
            .with(|a| {
                a.tax_year_end_date = NaiveDate::from_ymd_opt(2025, 6, 30).unwrap();
                a.date_received = NaiveDate::from_ymd_opt(2025, 9, 1).unwrap();
            })
            .insert(&pool)
            .await;
        test_support::amit_adjustment(&pool, 5, 5, 1, Decimal::from(1000)).await;

        let after = db_realised_gains(&pool).await.unwrap();
        assert_eq!(after[0].cost_base, Decimal::from(49000));
        assert_eq!(after[0].capital_gain_loss, Decimal::from(31000));
    }

    /// SCENARIOS D-13. The other half of the rule above: an adjustment row
    /// covering only *part* of the parcel — which is what
    /// `amit_adjustment_generation` writes for every parcel partly sold during
    /// the year, since it covers the units still open at the year end — takes
    /// the statement's per-unit figure off those units and no others. The
    /// units already sold are not covered, so their cost base is untouched.
    ///
    /// Pooling the row's total across the whole parcel instead spread it over
    /// units the statement never reached: the sale's cost base came out at
    /// 388.00 (1000 − 30, pro-rated 40/100) and the remainder at 582.00, when
    /// the statement itself says 60 units × 50c.
    #[tokio::test]
    async fn db_a_partial_amit_row_leaves_the_units_it_does_not_cover_alone() {
        let pool = test_pool().await;
        test_support::listing(1)
            .ticker("VDHG")
            .amit(true)
            .insert(&pool)
            .await;
        insert_buy(
            &pool,
            1,
            ListingId(1),
            NaiveDate::from_ymd_opt(2022, 1, 10).unwrap(),
            Decimal::from(100),
            Decimal::from(10),
        )
        .await;
        // 40 units sold in March, inside the year ended 30 June 2024.
        insert_sell(
            &pool,
            2,
            ListingId(1),
            NaiveDate::from_ymd_opt(2024, 3, 1).unwrap(),
            Decimal::from(40),
            Decimal::from(15),
        )
        .await;
        allocate(&pool, 1, 2, 1, Decimal::from(40)).await;
        // The FY2024 statement reduces 50c/unit over the 60 units held at its
        // year end — the row generation itself would write.
        test_support::amma(1, 1)
            .cost_base_adjustment("0.50".parse().unwrap())
            .with(|a| a.units_held = Decimal::from(60))
            .insert(&pool)
            .await;
        test_support::amit_adjustment(&pool, 1, 1, 1, Decimal::from(60)).await;

        // The sold units keep their full share: 1000 × 40/100.
        let realised = db_realised_gains(&pool).await.unwrap();
        assert_eq!(realised.len(), 1);
        assert_eq!(realised[0].cost_base, Decimal::from(400));
        assert_eq!(realised[0].capital_gain_loss, Decimal::from(200));

        // The 60 units covered take the stated 50c each — 600 − 30.
        let open = open_parcels::db_open_parcels(&pool, None).await.unwrap();
        assert_eq!(open.len(), 1);
        assert_eq!(open[0].amit_cost_base_reduction, Decimal::from(30));
        assert_eq!(open[0].remaining_cost_base, Decimal::from(570));

        // Nothing is lost or invented in the split: the statement's whole
        // 30.00 is accounted for, once.
        assert_eq!(
            open[0].remaining_cost_base + realised[0].cost_base,
            Decimal::from(970)
        );
    }

    /// SCENARIOS D-15, D-20. A disposal for nothing — worthless shares, or a
    /// gift entered at the nil consideration that actually changed hands
    /// rather than the market value the substitution rule requires — realises
    /// the whole cost base as a loss, and a sale whose costs exceed its
    /// consideration reports proceeds *below* nil rather than flooring them
    /// (the sale-side cost convention: costs net off proceeds, `docs/API.md`).
    #[tokio::test]
    async fn db_nil_and_negative_net_proceeds_realise_the_full_loss() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "NIL").await;
        let bought = NaiveDate::from_ymd_opt(2023, 1, 10).unwrap();
        for id in 1..=2i64 {
            insert_buy(
                &pool,
                id,
                ListingId(1),
                bought,
                Decimal::from(100),
                Decimal::from(10),
            )
            .await;
        }
        insert_sell(
            &pool,
            3,
            ListingId(1),
            NaiveDate::from_ymd_opt(2024, 6, 3).unwrap(),
            Decimal::from(100),
            Decimal::ZERO,
        )
        .await;
        allocate(&pool, 1, 3, 1, Decimal::from(100)).await;
        // The second closes at nil consideration but still costs $20 in
        // brokerage to execute.
        test_support::sell(4, 1)
            .date(test_support::ymd(2024, 6, 5))
            .qty(Decimal::from(100))
            .price(Decimal::ZERO)
            .brokerage(Decimal::from(20))
            .insert(&pool)
            .await;
        allocate(&pool, 2, 4, 2, Decimal::from(100)).await;

        let result = db_realised_gains(&pool).await.unwrap();
        assert_eq!(result.len(), 2);
        assert_eq!(result[0].proceeds, Decimal::ZERO);
        assert_eq!(result[0].capital_loss, Decimal::from(1000));
        assert_eq!(result[1].proceeds, Decimal::from(-20));
        assert_eq!(result[1].capital_loss, Decimal::from(1020));
    }

    /// SCENARIOS W. The sale-cost spread — `sale_costs × qty_so_far ÷
    /// sale.quantity` — multiplied A$1e10 of brokerage by 1e19 units before
    /// dividing: 1e29, past `Decimal`'s ~7.9228e28 ceiling, though the
    /// allocation's share is simply the whole A$1e10. Nil-priced on both legs
    /// so nothing else in the walk is at the limit.
    #[tokio::test]
    async fn api_realised_gains_past_the_old_sale_cost_spread_ceiling_reports() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "BIG").await;
        insert_buy(
            &pool,
            1,
            ListingId(1),
            NaiveDate::from_ymd_opt(2023, 1, 17).unwrap(),
            "10000000000000000000".parse().unwrap(),
            Decimal::ZERO,
        )
        .await;
        test_support::sell(2, 1)
            .date(NaiveDate::from_ymd_opt(2024, 8, 1).unwrap())
            .qty("10000000000000000000".parse().unwrap())
            .price(Decimal::ZERO)
            .brokerage("10000000000".parse().unwrap())
            .insert(&pool)
            .await;
        allocate(&pool, 1, 2, 1, "10000000000000000000".parse().unwrap()).await;

        let rows: Vec<RealisedGainLoss> = ApiClient::over(router().with_state(pool.clone()))
            .get_json("/portfolio/realised_gains")
            .await;
        assert_eq!(rows.len(), 1);
        // Nil proceeds less the whole brokerage: a capital loss of A$1e10.
        assert_eq!(rows[0].proceeds, "-10000000000".parse::<Decimal>().unwrap());
        assert_eq!(rows[0].capital_loss, "10000000000".parse().unwrap());
    }

    /// SCENARIOS W. The read-side twin of `scrip_exchange`'s apportionment:
    /// the allocation's AUD cost base (1e26) times the cash numerator (1e12)
    /// is 1e38, past the ceiling, while the cash side's share (5e25) is
    /// representable. The whole realised-gains report died on it.
    #[tokio::test]
    async fn api_realised_gains_past_the_old_scrip_apportionment_ceiling_reports() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "OLD").await;
        insert_listing(&pool, 2, "NEW").await;
        insert_buy(
            &pool,
            1,
            ListingId(1),
            NaiveDate::from_ymd_opt(2020, 10, 1).unwrap(),
            "10000000000".parse().unwrap(),
            "10000000000000000".parse().unwrap(),
        )
        .await;
        corporate_action::db_upsert(
            &pool,
            &corporate_action::CorporateAction {
                id: 10,
                listing_id: ListingId(1),
                date: NaiveDate::from_ymd_opt(2024, 7, 1).unwrap(),
                kind: corporate_action::ActionKind::ScripForScrip {
                    scrip_listing_id: ListingId(2),
                    scrip_new_units: Decimal::ONE,
                    scrip_old_units: Decimal::from(100),
                    scrip_cash_per_unit: Some("10000000000".parse().unwrap()),
                    scrip_market_value: Some("1000000000000".parse().unwrap()),
                    scrip_cash_currency: Some("AUD".into()),
                },
            },
        )
        .await
        .unwrap();
        scrip_exchange::db_exchange(&pool, 10).await.unwrap();

        let rows: Vec<RealisedGainLoss> = ApiClient::over(router().with_state(pool.clone()))
            .get_json("/portfolio/realised_gains")
            .await;
        assert_eq!(rows.len(), 1);
        // Exactly half of the 1e26 reduced cost base is on the cash side.
        assert_eq!(
            rows[0].cost_base,
            "50000000000000000000000000".parse::<Decimal>().unwrap()
        );
        assert_eq!(
            rows[0].proceeds,
            "100000000000000000000".parse::<Decimal>().unwrap()
        );
    }

    /// SCENARIOS W. The rights-cost spread — `rights_cost × units_so_far ÷
    /// sale.units` — on 1e19 lapsed rights carrying A$1e10 of cost: 1e29,
    /// past the ceiling, while the allocation's share is the whole A$1e10.
    #[tokio::test]
    async fn api_realised_gains_past_the_old_rights_cost_spread_ceiling_reports() {
        use crate::entities::rights_sale;
        let pool = test_pool().await;
        insert_listing(&pool, 1, "RTS").await;
        insert_buy(
            &pool,
            1,
            ListingId(1),
            NaiveDate::from_ymd_opt(2023, 1, 17).unwrap(),
            "40000000000000000000".parse().unwrap(),
            Decimal::ZERO,
        )
        .await;
        corporate_action::db_upsert(
            &pool,
            &corporate_action::CorporateAction {
                id: 10,
                listing_id: ListingId(1),
                date: NaiveDate::from_ymd_opt(2024, 7, 1).unwrap(),
                kind: corporate_action::ActionKind::RightsIssue {
                    rights_units: Decimal::ONE,
                    rights_held_units: Decimal::from(4),
                    exercise_price: "1.80".parse().unwrap(),
                    currency: "AUD".into(),
                    renounceable: true,
                },
            },
        )
        .await
        .unwrap();
        // The whole entitlement lapses, carrying A$1e10 of cost.
        rights_sale::db_sell_rights(
            &pool,
            10,
            &rights_sale::SellRightsBody {
                date: NaiveDate::from_ymd_opt(2024, 7, 20).unwrap(),
                units: "10000000000000000000".parse().unwrap(),
                proceeds_per_right: Some(Decimal::ZERO),
                rights_cost: Some("10000000000".parse().unwrap()),
                fx_rate: None,
                holding_account_id: HoldingAccountId(1),
                allocations: vec![rights_sale::AllocationInput {
                    purchase_trade_id: TradeId(1),
                    units: "10000000000000000000".parse().unwrap(),
                }],
            },
        )
        .await
        .unwrap();

        let rows: Vec<RealisedGainLoss> = ApiClient::over(router().with_state(pool.clone()))
            .get_json("/portfolio/realised_gains")
            .await;
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].cost_base, "10000000000".parse().unwrap());
        assert_eq!(rows[0].capital_loss, "10000000000".parse().unwrap());
    }

    /// Writes through the whole application, as the web UI does.
    async fn through_the_api(
        app: &ApiClient,
        method: &str,
        path: &str,
        body: serde_json::Value,
    ) -> serde_json::Value {
        let resp = match method {
            "PUT" => app.put(path, &body).await,
            _ => app.post(path, &body).await,
        };
        assert_eq!(resp.status, StatusCode::CREATED, "{path}: {}", resp.text());
        resp.json()
    }

    fn sell_everything(
        listing: i64,
        date: &str,
        price: &str,
        parcels: &[open_parcels::OpenParcel],
    ) -> serde_json::Value {
        let allocations: Vec<_> = parcels
            .iter()
            .filter(|p| p.listing_id == listing)
            .map(|p| serde_json::json!({
                "purchase_trade_id": p.trade_id, "quantity_allocated": p.remaining_quantity.to_string(),
            }))
            .collect();
        let quantity: Decimal = parcels
            .iter()
            .filter(|p| p.listing_id == listing)
            .map(|p| p.remaining_quantity)
            .sum();
        serde_json::json!({
            "listing_id": listing, "holding_account_id": 1, "date": date,
            "quantity": quantity.to_string(), "average_price": price, "currency": "AUD",
            "brokerage": "0", "brokerage_currency": "AUD", "fx_rate": "1", "allocations": allocations,
        })
    }

    /// The Sell emptying a parcel whose remainder is non-terminating (67
    /// units, a 2-for-7 bonus issue, 8 sold) realises exactly the cost base
    /// the open-parcels schedule carried for it: the Sell check takes it as
    /// the remainder, and so now does the costing — re-based on its own it
    /// came back a digit off.
    #[tokio::test]
    async fn api_the_sale_emptying_a_parcel_realises_exactly_its_scheduled_cost_base() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "BON").await;
        let app = ApiClient::full(&pool);
        through_the_api(
            &app,
            "PUT",
            "/trades/1",
            serde_json::json!({
                "listing_id": 1, "holding_account_id": 1, "date": "2021-04-12", "trade_type": "Buy",
                "quantity": "67", "average_price": "22.47", "currency": "AUD",
                "brokerage": "9.58", "brokerage_currency": "AUD", "fx_rate": "1",
            }),
        )
        .await;
        through_the_api(
            &app,
            "PUT",
            "/corporate_actions/1",
            serde_json::json!({
                "listing_id": 1, "date": "2021-05-26", "action_type": "BonusIssue",
                "bonus_units": "2", "bonus_held_units": "7",
            }),
        )
        .await;
        through_the_api(&app, "POST", "/sells", serde_json::json!({
            "listing_id": 1, "holding_account_id": 1, "date": "2021-07-05", "quantity": "8",
            "average_price": "3.21", "currency": "AUD", "brokerage": "0", "brokerage_currency": "AUD",
            "fx_rate": "1", "allocations": [{ "purchase_trade_id": 1, "quantity_allocated": "8" }],
        }))
        .await;
        let schedule = open_parcels::db_open_parcels(
            &pool,
            Some(NaiveDate::from_ymd_opt(2021, 7, 6).unwrap()),
        )
        .await
        .unwrap();
        let sell = through_the_api(
            &app,
            "POST",
            "/sells",
            sell_everything(1, "2021-07-06", "0.01", &schedule),
        )
        .await;
        let realised = db_realised_gains(&pool).await.unwrap();
        let row = realised
            .iter()
            .find(|r| r.disposal.sell() == Some(TradeId(sell["id"].as_i64().unwrap())))
            .unwrap();
        assert_eq!(row.parcels[0].cost_base, schedule[0].remaining_cost_base);
    }

    /// A disposal's proceeds are its contract note's total, however its
    /// parcels' shares round: two replacement parcels' full-precision shares
    /// summed to 21961.146…666 of a …667 total.
    #[tokio::test]
    async fn api_a_disposals_proceeds_are_its_contract_notes_total() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "OLD").await;
        insert_listing(&pool, 2, "NEW").await;
        let app = ApiClient::full(&pool);
        through_the_api(
            &app,
            "PUT",
            "/trades/1",
            serde_json::json!({
                "listing_id": 1, "holding_account_id": 1, "date": "2021-04-12", "trade_type": "Buy",
                "quantity": "241", "average_price": "0.05", "currency": "AUD",
                "brokerage": "0", "brokerage_currency": "AUD", "fx_rate": "1",
            }),
        )
        .await;
        through_the_api(&app, "POST", "/sells", serde_json::json!({
            "listing_id": 1, "holding_account_id": 1, "date": "2021-05-24", "quantity": "168",
            "average_price": "3.21", "currency": "AUD", "brokerage": "0", "brokerage_currency": "AUD",
            "fx_rate": "1", "allocations": [{ "purchase_trade_id": 1, "quantity_allocated": "168" }],
        }))
        .await;
        through_the_api(
            &app,
            "PUT",
            "/corporate_actions/1",
            serde_json::json!({
                "listing_id": 1, "date": "2021-07-05", "action_type": "ShareSplit",
                "split_new_units": "2", "split_old_units": "3",
            }),
        )
        .await;
        through_the_api(
            &app,
            "PUT",
            "/trades/3",
            serde_json::json!({
                "listing_id": 1, "holding_account_id": 1, "date": "2021-08-16", "trade_type": "Buy",
                "quantity": "180", "average_price": "0.05", "currency": "AUD",
                "brokerage": "0", "brokerage_currency": "AUD", "fx_rate": "1",
            }),
        )
        .await;
        through_the_api(
            &app,
            "PUT",
            "/corporate_actions/2",
            serde_json::json!({
                "listing_id": 1, "date": "2021-09-27", "action_type": "ScripForScrip",
                "scrip_listing_id": 2, "scrip_new_units": "2", "scrip_old_units": "1",
            }),
        )
        .await;
        through_the_api(
            &app,
            "POST",
            "/corporate_actions/2/exchange",
            serde_json::json!(null),
        )
        .await;
        let schedule = open_parcels::db_open_parcels(
            &pool,
            Some(NaiveDate::from_ymd_opt(2021, 9, 28).unwrap()),
        )
        .await
        .unwrap();
        let body = sell_everything(2, "2021-09-28", "48.02", &schedule);
        let quantity: Decimal = body["quantity"].as_str().unwrap().parse().unwrap();
        let sell = through_the_api(&app, "POST", "/sells", body).await;
        let realised = db_realised_gains(&pool).await.unwrap();
        let row = realised
            .iter()
            .find(|r| r.disposal.sell() == Some(TradeId(sell["id"].as_i64().unwrap())))
            .unwrap();
        assert_eq!(row.parcels.len(), 2);
        assert_eq!(row.proceeds, dec("48.02") * quantity);
    }

    mod properties {
        use super::*;
        use crate::entities::sell::{self, AllocationInput, SellBody};
        use crate::reports::open_parcels;
        use crate::test_support::ymd;
        use chrono::{Datelike, Weekday};
        use proptest::prelude::*;
        use std::collections::HashMap;

        /// Wednesdays from August to November of `year` — trading days on
        /// every seeded ASX calendar, clear of its public holidays.
        fn trading_days(year: i32) -> Vec<NaiveDate> {
            NaiveDate::from_ymd_opt(year, 8, 1)
                .unwrap()
                .iter_days()
                .take_while(|d| d.month() <= 11)
                .filter(|d| d.weekday() == Weekday::Wed)
                .collect()
        }

        /// One parcel bought: units, cents per unit, brokerage cents.
        fn buys() -> impl Strategy<Value = Vec<(i64, i64, i64)>> {
            prop::collection::vec((1i64..=2_000, 1i64..=10_000, 0i64..=3_000), 1..=3)
        }

        /// One Sell: per parcel, the percentage of what remains of it to
        /// take (0 = none), and the sale price in cents.
        fn sells() -> impl Strategy<Value = Vec<(Vec<u32>, i64)>> {
            prop::collection::vec((prop::collection::vec(0u32..=100, 3), 1i64..=20_000), 1..=4)
        }

        proptest! {
            /// Whatever the total and however the units fall — including
            /// counts a non-terminating ratio leaves (n/3, n/7) — the shares
            /// `cumulative_share` hands out add back to the total exactly, and
            /// the final entitlement is the total itself.
            #[test]
            fn cumulative_shares_add_back_to_the_total_exactly(
                mantissa in -10_000_000_000_000i64..=10_000_000_000_000,
                scale in 0u32..=10,
                units in prop::collection::vec((1i64..=100_000, 1i64..=9), 1..=6),
            ) {
                let total = Decimal::new(mantissa, scale);
                let units: Vec<Decimal> =
                    units.into_iter().map(|(n, d)| Decimal::from(n) / Decimal::from(d)).collect();
                let units_total: Decimal = units.iter().sum();
                let mut so_far = Decimal::ZERO;
                let mut entitled = Decimal::ZERO;
                let mut shares = Decimal::ZERO;
                for u in &units {
                    so_far += *u;
                    let step = cumulative_share(total, so_far, units_total, entitled);
                    shares += step.share;
                    entitled = step.entitled;
                }
                prop_assert_eq!(entitled, total);
                prop_assert_eq!(shares, total);
            }
        }

        proptest! {
            #![proptest_config(test_support::db_proptest_config())]

            /// Whatever parcels are bought, split and sold, nothing is lost
            /// or double-counted between the two reports: each parcel's
            /// units and cost base are exactly what the realised-gains
            /// report charged its sales plus what the open-parcels report
            /// still holds, each disposal's proceeds are its net sale
            /// proceeds, and every row's buckets add up to its gain.
            #[test]
            fn realised_and_open_parcels_account_for_every_unit_and_dollar(
                buys in buys(),
                split in prop::option::of(prop::sample::select(vec![(2i64, 1i64), (3, 1), (1, 2), (5, 4)])),
                sells in sells(),
            ) {
                test_support::block_on(async move {
                    let pool = test_pool().await;
                    insert_listing(&pool, 1, "PRP").await;
                    let buy_days = trading_days(2020);
                    let mut bought = Vec::new();
                    for (i, (qty, cents, brokerage)) in buys.iter().enumerate() {
                        let (qty, price, brokerage) = (Decimal::from(*qty), Decimal::new(*cents, 2), Decimal::new(*brokerage, 2));
                        test_support::buy(i as i64 + 1, 1)
                            .date(buy_days[i * 3])
                            .qty(qty)
                            .price(price)
                            .brokerage(brokerage)
                            .insert(&pool)
                            .await;
                        bought.push((qty, qty * price + brokerage));
                    }
                    let ratio = match split {
                        Some((new, old)) => {
                            corporate_action::db_upsert(&pool, &corporate_action::CorporateAction {
                                id: 1,
                                listing_id: ListingId(1),
                                date: ymd(2021, 6, 2),
                                kind: corporate_action::ActionKind::ShareSplit {
                                    split_new_units: Decimal::from(new),
                                    split_old_units: Decimal::from(old),
                                },
                            })
                            .await
                            .unwrap();
                            Decimal::from(new) / Decimal::from(old)
                        }
                        None => Decimal::ONE,
                    };

                    // What remains of each parcel, in post-split units.
                    let mut remaining: Vec<Decimal> = bought.iter().map(|(q, _)| *q * ratio).collect();
                    let sale_days = trading_days(2022);
                    let mut net_proceeds: HashMap<TradeId, Decimal> = HashMap::new();
                    for (n, (takes, cents)) in sells.iter().enumerate() {
                        let allocations: Vec<AllocationInput> = remaining
                            .iter()
                            .zip(takes)
                            .enumerate()
                            .filter_map(|(p, (left, pct))| {
                                let units = (*left * Decimal::from(*pct) / Decimal::from(100)).round_dp(2).min(*left);
                                (units > Decimal::ZERO).then_some(AllocationInput {
                                    purchase_trade_id: TradeId(p as i64 + 1),
                                    quantity_allocated: units,
                                })
                            })
                            .collect();
                        if allocations.is_empty() {
                            continue;
                        }
                        let quantity: Decimal = allocations.iter().map(|a| a.quantity_allocated).sum();
                        let price = Decimal::new(*cents, 2);
                        let id = TradeId(100 + n as i64);
                        let body = SellBody {
                            brokerage_includes_gst: false,
                            statement_total: None,
                            holding_account_id: HoldingAccountId(1),
                            date: sale_days[n * 2],
                            settlement_date: None,
                            listing_id: ListingId(1),
                            average_price: price,
                            quantity,
                            currency: "AUD".into(),
                            brokerage: Decimal::from(10),
                            gst_on_brokerage: Decimal::ONE,
                            brokerage_currency: "AUD".into(),
                            fx_rate: Decimal::ONE,
                            spot_fx_rate: None,
                            contract_note_ref: None,
                            allocations,
                        };
                        if let Err(e) = sell::db_upsert_sell(&pool, id, &body).await {
                            return Err(TestCaseError::fail(format!("valid Sell {id:?} refused: {e}")));
                        }
                        for a in &body.allocations {
                            let parcel = usize::try_from(a.purchase_trade_id.0 - 1)
                                .expect("the strategy's parcel ids start at 1");
                            remaining[parcel] -= a.quantity_allocated;
                        }
                        net_proceeds.insert(id, price * quantity - Decimal::from(11));
                    }

                    let realised = db_realised_gains(&pool).await.unwrap();
                    let open = open_parcels::db_open_parcels(&pool, Some(ymd(2026, 1, 1))).await.unwrap();
                    let tolerance = Decimal::new(1, 18);
                    for row in &realised {
                        let id = row.disposal.sell().unwrap();
                        prop_assert_eq!(row.proceeds, net_proceeds[&id], "Sell {:?} proceeds", id);
                        prop_assert_eq!(row.capital_gain_loss, row.proceeds - row.cost_base);
                        prop_assert_eq!(row.proceeds, row.parcels.iter().map(|p| p.proceeds).sum::<Decimal>());
                        // The buckets and the cost base are sums of 28-digit
                        // per-parcel figures, and such a sum itself rounds.
                        prop_assert!(
                            (row.capital_gain_loss
                                - (row.discount_eligible_gain + row.non_discountable_gain - row.capital_loss))
                                .abs()
                                <= tolerance
                        );
                        prop_assert!(
                            (row.cost_base - row.parcels.iter().map(|p| p.cost_base).sum::<Decimal>()).abs() <= tolerance
                        );
                    }
                    prop_assert_eq!(realised.len(), net_proceeds.len());
                    for (p, (qty, cost)) in bought.iter().enumerate() {
                        let id = TradeId(p as i64 + 1);
                        let sold = realised.iter().flat_map(|r| &r.parcels).filter(|d| d.purchase_trade_id == id);
                        let (sold_units, sold_cost) = sold.fold((Decimal::ZERO, Decimal::ZERO), |(u, c), d| (u + d.units, c + d.cost_base));
                        let still_open = open.iter().find(|o| o.trade_id == id);
                        let open_units = still_open.map_or(Decimal::ZERO, |o| o.remaining_quantity);
                        let open_cost = still_open.map_or(Decimal::ZERO, |o| o.remaining_cost_base);
                        prop_assert_eq!(sold_units + open_units, *qty * ratio, "parcel {} units", p + 1);
                        prop_assert_eq!(open_units, remaining[p], "parcel {} open units", p + 1);
                        prop_assert!(
                            (sold_cost + open_cost - cost).abs() <= tolerance,
                            "parcel {} cost {} = sold {} + open {}", p + 1, cost, sold_cost, open_cost
                        );
                    }
                    Ok(())
                })?;
            }
        }
    }
}
