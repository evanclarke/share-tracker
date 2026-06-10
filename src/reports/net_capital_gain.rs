//! Net capital gain / overall CGT position per Australian tax year.
//!
//! Combines the realised disposal gains (per the realised-gains report —
//! ordinary Sells plus rights sales/lapses, whose gains and losses enter the
//! same buckets) with the CGT components attributed on AMMA statements, then
//! computes the assessable net capital gain the ATO way:
//!
//!  1. Total the year's gross capital gains, split into:
//!     - discount-eligible gains (realised parcels held > 12 months, plus AMMA
//!       discount-method gains grossed up ×2 — the AMMA value is the already-halved
//!       "discounted capital gain" line, so doubling it restores the gross gain);
//!     - non-discountable gains (realised parcels held ≤ 12 months, plus AMMA
//!       indexation-method and other-method gains, neither of which gets the 50%
//!       discount).
//!  2. Total the year's capital losses: realised losses + AMMA capital losses
//!     applied, plus the net capital loss brought forward from earlier years
//!     (losses carry forward indefinitely, per `docs/ato/cgt-using-capital-losses.md`).
//!     The chain starts from the entered opening carried-forward loss in
//!     `cgt_settings` (losses from before the first year in the system).
//!  3. Apply losses against gains in the taxpayer-favourable order — non-discountable
//!     gains first, then discount-eligible gains — so the 50% discount falls on the
//!     largest possible remaining gain.
//!  4. Net capital gain = remaining non-discountable gain + 50% of the remaining
//!     discount-eligible gain. Any unused loss is carried forward into the next
//!     year in the series.

use crate::domain::tax_year::tax_year_for;
use crate::infra::decimal::parse_dec;
use crate::infra::fx::FxRates;
use crate::infra::http::ApiError;
use crate::reports::export;
use crate::reports::parcel_optimiser::{self, DisposalTotals, HypotheticalAllocation, Strategy};
use axum::{
    Json, Router,
    extract::State,
    response::Response,
    routing::{get, post},
};
use chrono::{Datelike, Months, NaiveDate};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use sqlx::{Row, SqlitePool};
use std::collections::HashMap;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NetCapitalGainYear {
    /// Australian tax year: the calendar year in which 30 June falls (e.g. 2024 = FY2023/24).
    pub tax_year: i32,
    /// Gross discount-eligible capital gains before the discount (realised parcels
    /// held > 12 months + AMMA discount-method gains grossed up ×2).
    pub discount_eligible_gains: Decimal,
    /// Gross non-discountable capital gains (realised parcels held ≤ 12 months +
    /// AMMA indexation-method + AMMA other-method gains).
    pub other_gains: Decimal,
    /// Capital losses arising this year (realised losses + AMMA capital losses
    /// applied), as a positive amount. Excludes the brought-forward balance.
    pub capital_losses: Decimal,
    /// Net capital loss brought forward from earlier years (unused losses chained
    /// from prior years in the series, seeded by the `cgt_settings` opening
    /// carried-forward loss), as a positive amount. Also offsets this year's gains.
    pub capital_loss_brought_forward: Decimal,
    /// Discount-eligible gain remaining after capital losses are applied (gross,
    /// before the 50% discount).
    pub net_discount_eligible_gain: Decimal,
    /// Non-discountable gain remaining after capital losses are applied.
    pub net_other_gain: Decimal,
    /// The 50% CGT discount amount removed from the remaining discount-eligible gain
    /// (= net_discount_eligible_gain / 2).
    pub cgt_discount: Decimal,
    /// Assessable net capital gain = net_other_gain + net_discount_eligible_gain / 2.
    pub net_capital_gain: Decimal,
    /// Capital losses left unused after offsetting all gains (from both this
    /// year's losses and the brought-forward balance), carried forward into the
    /// next year in the series.
    pub capital_loss_carried_forward: Decimal,
    /// Informational: gross CGT event E10 gains included in this year (the excess of
    /// AMIT cost base reductions over a parcel's cost base). Already counted within
    /// `discount_eligible_gains` / `other_gains` above per the holding period at the
    /// statement's year end; surfaced separately for transparency.
    pub cgt_event_e10_gain: Decimal,
    /// Informational: gross CGT event G1 gains included in this year (the excess of
    /// a company's return-of-capital payments over a parcel's cost base). Already
    /// counted within `discount_eligible_gains` / `other_gains` above per the
    /// holding period at the payment date; surfaced separately for transparency.
    pub cgt_event_g1_gain: Decimal,
    /// Informational: the taxpayer assumption behind the hard-wired rates
    /// (always [`crate::reports::TAXPAYER_BASIS`]) — the 50% discount applied
    /// here is the Australian-resident-individual rate; other entity types are
    /// not modelled.
    pub taxpayer_basis: String,
}

pub fn router() -> Router<SqlitePool> {
    Router::new()
        .route("/portfolio/net-capital-gain", get(net_capital_gain_handler))
        .route(
            "/portfolio/net-capital-gain/export",
            get(net_capital_gain_export_handler),
        )
        .route("/portfolio/net-capital-gain/what-if", post(what_if_handler))
}

/// CSV export columns — `NetCapitalGainYear`'s fields in declaration order. The
/// csv writer rejects a record whose length differs from this header (see
/// `reports::export`), so a drift between the two fails loudly.
const CSV_HEADER: &[&str] = &[
    "tax_year",
    "discount_eligible_gains",
    "other_gains",
    "capital_losses",
    "capital_loss_brought_forward",
    "net_discount_eligible_gain",
    "net_other_gain",
    "cgt_discount",
    "net_capital_gain",
    "capital_loss_carried_forward",
    "cgt_event_e10_gain",
    "cgt_event_g1_gain",
    "taxpayer_basis",
];

/// Gross gains and losses accumulated for one tax year before netting.
#[derive(Default, Clone)]
struct GrossBuckets {
    discount_eligible: Decimal,
    other: Decimal,
    losses: Decimal,
    /// Gross CGT event E10 gains folded into the buckets above (informational).
    e10: Decimal,
    /// Gross CGT event G1 gains folded into the buckets above (informational).
    g1: Decimal,
}

/// Read a TEXT decimal column and convert it to AUD via the pre-loaded ATO
/// rate for `currency` and the month of `date`. AMMA records carry no manual
/// fx override, so a non-AUD amount with no ATO rate fails loudly (the
/// `FxError` surfaces as a decode error).
fn aud_field(
    fx: &FxRates,
    row: &sqlx::sqlite::SqliteRow,
    field: &str,
    currency: &str,
    date: NaiveDate,
) -> Result<Decimal, sqlx::Error> {
    let value = parse_dec(field, row.try_get(field)?)?;
    Ok(fx.to_aud(value, currency, date, None)?)
}

/// CGT event E10 gains: when the cumulative AMIT cost base reductions applied to a
/// parcel exceed its cost base, the cost base is floored at nil and the excess is a
/// capital gain in the income year the reducing AMMA statement applies to (see
/// `docs/ato/amit-cost-base-adjustments.md`).
///
/// Returns `(tax_year, gross_gain_aud, discount_eligible)` for each AMMA statement
/// that pushes a parcel's cost base below nil. Adjustments are walked per parcel in
/// tax-year order, so the gain falls in the year the cost base is first exhausted —
/// and in every later year, since the cost base stays at nil (a later negative
/// adjustment, i.e. a cost base increase, restores it first). The excess is computed
/// in the parcel's native currency and converted to AUD at the parcel's buy-month ATO
/// rate (matching how the cost base itself is converted in the realised report), then
/// classified as discount-eligible when the units were held more than 12 months as at
/// the statement's `tax_year_end_date`.
async fn e10_gains(
    pool: &SqlitePool,
    fx: &FxRates,
) -> Result<Vec<(i32, Decimal, bool)>, sqlx::Error> {
    let rows = sqlx::query(
        "SELECT aa.trade_id, aa.quantity AS adj_qty, \
                t.date AS trade_date, t.deemed_acquisition_date, \
                t.quantity AS trade_qty, t.average_price, \
                t.brokerage, t.gst_on_brokerage, t.currency AS trade_currency, t.fx_rate, \
                a.cost_base_adjustment, a.tax_year_end_date \
         FROM amit_adjustments aa \
         JOIN trades t ON t.id = aa.trade_id \
         JOIN amma_statements a ON a.id = aa.amma_statement_id \
         ORDER BY aa.trade_id, a.tax_year_end_date, a.id",
    )
    .fetch_all(pool)
    .await?;

    let mut out = Vec::new();
    let mut i = 0;
    while i < rows.len() {
        let trade_id: i64 = rows[i].try_get("trade_id")?;
        // Parcel cost base in native currency (the trade columns repeat per row).
        let trade_qty = parse_dec("trade_qty", rows[i].try_get("trade_qty")?)?;
        let price = parse_dec("average_price", rows[i].try_get("average_price")?)?;
        let brok = parse_dec("brokerage", rows[i].try_get("brokerage")?)?;
        let gst = parse_dec("gst_on_brokerage", rows[i].try_get("gst_on_brokerage")?)?;
        let trade_date: NaiveDate = rows[i].try_get("trade_date")?;
        // A scrip-for-scrip replacement parcel's discount clock and AUD
        // translation month run from its deemed (carried) acquisition date.
        let deemed: Option<NaiveDate> = rows[i].try_get("deemed_acquisition_date")?;
        let acquired = deemed.unwrap_or(trade_date);
        let currency: String = rows[i].try_get("trade_currency")?;
        let fx_rate = parse_dec("fx_rate", rows[i].try_get("fx_rate")?)?;
        let mut remaining = price * trade_qty + brok + gst;

        while i < rows.len() && rows[i].try_get::<i64, _>("trade_id")? == trade_id {
            let adj_qty = parse_dec("adj_qty", rows[i].try_get("adj_qty")?)?;
            let cba = parse_dec(
                "cost_base_adjustment",
                rows[i].try_get("cost_base_adjustment")?,
            )?;
            let year_end: NaiveDate = rows[i].try_get("tax_year_end_date")?;
            let reduction = adj_qty * cba;
            if reduction > remaining {
                let excess = reduction - remaining;
                let excess_aud = fx.to_aud(excess, &currency, acquired, Some(fx_rate))?;
                let discount_eligible = year_end > acquired + Months::new(12);
                out.push((year_end.year(), excess_aud, discount_eligible));
                remaining = Decimal::ZERO;
            } else {
                remaining -= reduction;
            }
            i += 1;
        }
    }
    Ok(out)
}

/// CGT event G1 gains: when a company's non-assessable (return-of-capital)
/// payments exceed a parcel's per-unit cost base, the cost base is floored at nil
/// and the excess is a capital gain in the payment's income year — G1 can never
/// produce a capital loss (see `docs/ato/cgt-non-assessable-payments.md`).
///
/// Returns `(tax_year, gross_gain_aud, discount_eligible)` for each payment that
/// pushes a parcel's per-unit cost base below nil. Payments are walked per parcel
/// in date order on a per-unit basis (the ATO compares the payment against the
/// cost base *of the shares* at the payment time), tracked here as whole-parcel
/// totals so no division precedes the final pro-rating. The gain covers only the
/// units still held at the payment date (units sold earlier were not held for
/// it); it is converted to AUD at the payment month's ATO rate (no manual
/// fallback — a non-AUD payment with no rate fails loudly) and classified
/// discount-eligible when the units were held more than 12 months at the payment
/// date. Independent of the AMIT E10 walk above: E10 applies to trust units, G1
/// to company shares, so the two reduction chains never share a parcel in
/// practice.
async fn g1_gains(
    pool: &SqlitePool,
    fx: &FxRates,
) -> Result<Vec<(i32, Decimal, bool)>, sqlx::Error> {
    let rows = sqlx::query(
        "SELECT ca.date AS action_date, ca.amount_per_unit, ca.currency AS action_currency, \
                ca.listing_id, \
                t.id AS trade_id, t.date AS trade_date, t.deemed_acquisition_date, \
                t.quantity AS trade_qty, \
                t.average_price, t.brokerage, t.gst_on_brokerage, \
                t.currency AS trade_currency \
         FROM corporate_actions ca \
         JOIN trades t ON t.listing_id = ca.listing_id \
                      AND t.trade_type IN ('Buy', 'DRP') \
                      AND t.date <= ca.date \
         WHERE ca.action_type = 'ReturnOfCapital' \
         ORDER BY t.id, ca.date, ca.id",
    )
    .fetch_all(pool)
    .await?;

    if rows.is_empty() {
        return Ok(vec![]);
    }

    // Share splits/consolidations per listing: a payment after a split is per
    // *post-split* unit, so its whole-parcel equivalent scales by the split
    // ratio between acquisition and the payment date (TD 2000/10); sold units
    // are re-based back to as-acquired units the same way.
    let split_events = crate::entities::corporate_action::db_share_split_events(pool).await?;

    // Units sold out of each parcel, with the sale date — a unit sold before a
    // payment was not held for it.
    let alloc_rows = sqlx::query(
        "SELECT pa.purchase_trade_id, pa.quantity_allocated, s.date AS sale_date \
         FROM parcel_allocations pa JOIN trades s ON s.id = pa.sale_trade_id",
    )
    .fetch_all(pool)
    .await?;
    let mut sold: HashMap<i64, Vec<(NaiveDate, Decimal)>> = HashMap::new();
    for row in &alloc_rows {
        let tid: i64 = row.try_get("purchase_trade_id")?;
        let qty = parse_dec("quantity_allocated", row.try_get("quantity_allocated")?)?;
        let sale_date: NaiveDate = row.try_get("sale_date")?;
        sold.entry(tid).or_default().push((sale_date, qty));
    }

    let mut out = Vec::new();
    let mut i = 0;
    while i < rows.len() {
        let trade_id: i64 = rows[i].try_get("trade_id")?;
        // Parcel cost base in native currency (the trade columns repeat per row).
        let trade_qty = parse_dec("trade_qty", rows[i].try_get("trade_qty")?)?;
        let price = parse_dec("average_price", rows[i].try_get("average_price")?)?;
        let brok = parse_dec("brokerage", rows[i].try_get("brokerage")?)?;
        let gst = parse_dec("gst_on_brokerage", rows[i].try_get("gst_on_brokerage")?)?;
        let trade_date: NaiveDate = rows[i].try_get("trade_date")?;
        // A scrip-for-scrip replacement parcel's discount clock runs from its
        // deemed (carried) acquisition date; split/payment applicability
        // stays on the actual trade date.
        let deemed: Option<NaiveDate> = rows[i].try_get("deemed_acquisition_date")?;
        let acquired = deemed.unwrap_or(trade_date);
        let trade_currency: String = rows[i].try_get("trade_currency")?;
        let listing_id: i64 = rows[i].try_get("listing_id")?;
        let splits = split_events.get(&listing_id).map_or(&[][..], |v| v);
        // Whole-parcel remaining cost base: remaining ÷ quantity is the per-unit
        // figure the payment is compared against, kept un-divided for precision.
        let mut remaining = price * trade_qty + brok + gst;

        while i < rows.len() && rows[i].try_get::<i64, _>("trade_id")? == trade_id {
            let action_date: NaiveDate = rows[i].try_get("action_date")?;
            let amount_pu = parse_dec("amount_per_unit", rows[i].try_get("amount_per_unit")?)?;
            let action_currency: String = rows[i].try_get("action_currency")?;
            if action_currency != trade_currency {
                return Err(sqlx::Error::Decode(
                    format!(
                        "return-of-capital currency {action_currency} differs from the \
                         parcel's currency {trade_currency}"
                    )
                    .into(),
                ));
            }
            // Whole-parcel equivalent of the per-unit payment. The payment is
            // per unit *at the payment date*: a split between acquisition and
            // the payment multiplies the units receiving it.
            let (new, old) = crate::entities::corporate_action::split_ratio(
                splits,
                trade_date,
                Some(action_date),
            );
            let payment = if new == old {
                amount_pu * trade_qty
            } else {
                amount_pu * trade_qty * new / old
            };
            if payment > remaining {
                let excess = payment - remaining;
                // Only the units still held at the payment date received it
                // (sold units re-based back to as-acquired units).
                let held = trade_qty
                    - sold.get(&trade_id).map_or(Decimal::ZERO, |sales| {
                        sales
                            .iter()
                            .filter(|(d, _)| *d < action_date)
                            .map(|&(d, q)| {
                                crate::entities::corporate_action::as_acquired_quantity(
                                    q, splits, trade_date, d,
                                )
                            })
                            .sum()
                    });
                if held > Decimal::ZERO && trade_qty > Decimal::ZERO {
                    let gain = excess * held / trade_qty;
                    let gain_aud = fx.to_aud(gain, &action_currency, action_date, None)?;
                    let discount_eligible = action_date > acquired + Months::new(12);
                    out.push((tax_year_for(action_date), gain_aud, discount_eligible));
                }
                remaining = Decimal::ZERO;
            } else {
                remaining -= payment;
            }
            i += 1;
        }
    }
    Ok(out)
}

pub async fn db_net_capital_gain(
    pool: &SqlitePool,
) -> Result<Vec<NetCapitalGainYear>, sqlx::Error> {
    let buckets = gross_buckets(pool).await?;
    let opening = crate::entities::cgt_settings::db_opening_capital_loss(pool).await?;
    Ok(net_years(buckets, opening))
}

/// Accumulate the gross per-year buckets from every recorded source:
/// realised disposals, AMMA CGT components, and the E10/G1 excess gains.
/// Shared by the report and the what-if (which injects a hypothetical
/// disposal's buckets before netting).
async fn gross_buckets(pool: &SqlitePool) -> Result<HashMap<i32, GrossBuckets>, sqlx::Error> {
    let mut buckets: HashMap<i32, GrossBuckets> = HashMap::new();

    // Every imported ATO FX rate — the AMMA/E10/G1 conversions below are map
    // lookups, not one DB round-trip each.
    let fx = FxRates::load(pool).await?;

    // Realised parcel gains (already AUD), bucketed by the sale's tax year.
    let realised = super::realised_gains::db_realised_gains(pool).await?;
    for r in &realised {
        let b = buckets.entry(tax_year_for(r.sale_date)).or_default();
        b.discount_eligible += r.discount_eligible_gain;
        b.other += r.non_discountable_gain;
        b.losses += r.capital_loss;
    }

    // AMMA-attributed CGT components, converted to AUD via the ATO rate for the
    // month of tax_year_end_date (the statement's only period anchor).
    let amma_rows = sqlx::query(
        "SELECT tax_year_end_date, cgt_discount_gains, cgt_indexation_gains, \
         cgt_other_gains, capital_losses_applied, currency \
         FROM amma_statements",
    )
    .fetch_all(pool)
    .await?;

    for row in &amma_rows {
        let year_end: NaiveDate = row.try_get("tax_year_end_date")?;
        let currency: String = row.try_get("currency")?;
        let d = year_end;
        // AMMA discount-method gains are the already-halved "discounted capital gain"
        // line; gross up ×2 to the pre-discount gain before netting losses.
        let discount_net = aud_field(&fx, row, "cgt_discount_gains", &currency, d)?;
        let indexation = aud_field(&fx, row, "cgt_indexation_gains", &currency, d)?;
        let other = aud_field(&fx, row, "cgt_other_gains", &currency, d)?;
        let losses = aud_field(&fx, row, "capital_losses_applied", &currency, d)?;

        let b = buckets.entry(year_end.year()).or_default();
        b.discount_eligible += discount_net * Decimal::from(2);
        b.other += indexation + other;
        b.losses += losses;
    }

    // CGT event E10 gains — excess AMIT cost base reductions over a parcel's cost
    // base — are ordinary capital gains: they enter the buckets (discount-eligible or
    // not, per the holding period at year end), so losses can offset them and the
    // discount applies to the eligible portion.
    for (tax_year, amount, discount_eligible) in e10_gains(pool, &fx).await? {
        let b = buckets.entry(tax_year).or_default();
        if discount_eligible {
            b.discount_eligible += amount;
        } else {
            b.other += amount;
        }
        b.e10 += amount;
    }

    // CGT event G1 gains — return-of-capital payments in excess of a parcel's
    // cost base — are likewise ordinary capital gains entering the buckets
    // (discount-eligible or not, per the holding period at the payment date).
    for (tax_year, amount, discount_eligible) in g1_gains(pool, &fx).await? {
        let b = buckets.entry(tax_year).or_default();
        if discount_eligible {
            b.discount_eligible += amount;
        } else {
            b.other += amount;
        }
        b.g1 += amount;
    }

    Ok(buckets)
}

/// Steps 3 and 4: walk the years in order, applying losses ATO-optimally and
/// chaining unused net capital losses forward — a year's leftover loss
/// becomes the next year's brought-forward balance (losses carry forward
/// indefinitely). The chain starts from `brought_forward`, the entered
/// opening carried-forward loss (pre-system loss years) in `cgt_settings`.
fn net_years(
    buckets: HashMap<i32, GrossBuckets>,
    mut brought_forward: Decimal,
) -> Vec<NetCapitalGainYear> {
    let mut years: Vec<(i32, GrossBuckets)> = buckets.into_iter().collect();
    years.sort_by_key(|(tax_year, _)| *tax_year);

    let two = Decimal::from(2);
    years
        .into_iter()
        .map(|(tax_year, b)| {
            // Apply losses (this year's + brought forward — both offset gains before
            // the discount) to non-discountable gains first, then to discount-eligible
            // gains (taxpayer-favourable: the discount falls on the largest remainder).
            let available_losses = b.losses + brought_forward;
            let loss_to_other = b.other.min(available_losses);
            let net_other = b.other - loss_to_other;
            let remaining_loss = available_losses - loss_to_other;

            let loss_to_discount = b.discount_eligible.min(remaining_loss);
            let net_discount = b.discount_eligible - loss_to_discount;
            let carried_forward = remaining_loss - loss_to_discount;

            let cgt_discount = net_discount / two;
            let year = NetCapitalGainYear {
                tax_year,
                discount_eligible_gains: b.discount_eligible,
                other_gains: b.other,
                capital_losses: b.losses,
                capital_loss_brought_forward: brought_forward,
                net_discount_eligible_gain: net_discount,
                net_other_gain: net_other,
                cgt_discount,
                net_capital_gain: net_other + cgt_discount,
                capital_loss_carried_forward: carried_forward,
                cgt_event_e10_gain: b.e10,
                cgt_event_g1_gain: b.g1,
                taxpayer_basis: crate::reports::TAXPAYER_BASIS.to_string(),
            };
            brought_forward = carried_forward;
            year
        })
        .collect()
}

async fn net_capital_gain_handler(
    State(pool): State<SqlitePool>,
) -> Result<Json<Vec<NetCapitalGainYear>>, ApiError> {
    db_net_capital_gain(&pool)
        .await
        .map(Json)
        .map_err(ApiError::from)
}

/// The same per-year rows as the JSON report, as a downloadable tax-return-ready CSV.
async fn net_capital_gain_export_handler(
    State(pool): State<SqlitePool>,
) -> Result<Response, ApiError> {
    let rows = db_net_capital_gain(&pool).await.map_err(ApiError::from)?;
    export::csv_response("net-capital-gain.csv", CSV_HEADER, &rows).map_err(ApiError::from)
}

// ---------------------------------------------------------------------------
// Pre-sale what-if
// ---------------------------------------------------------------------------

/// A hypothetical disposal: `units` of `listing_id` sold on `date` for
/// `proceeds` (total capital proceeds, AUD), drawn from open parcels via
/// either explicit `allocations` or a named optimiser `strategy` — exactly
/// one of the two.
#[derive(Debug, Deserialize)]
pub struct WhatIfRequest {
    pub listing_id: i64,
    /// Restricts strategy-derived (and validates explicit) allocations to
    /// one account's parcels; absent = parcels from any account.
    #[serde(default)]
    pub holding_account_id: Option<i64>,
    pub units: Decimal,
    /// Total capital proceeds in AUD.
    pub proceeds: Decimal,
    pub date: NaiveDate,
    #[serde(default)]
    pub allocations: Option<Vec<WhatIfAllocation>>,
    #[serde(default)]
    pub strategy: Option<Strategy>,
}

#[derive(Debug, Deserialize)]
pub struct WhatIfAllocation {
    pub purchase_trade_id: i64,
    pub units: Decimal,
}

/// One year row labelled with its scenario (`without` / `with` the
/// hypothetical disposal).
#[derive(Debug, Serialize, Deserialize)]
pub struct ScenarioYear {
    pub scenario: String,
    #[serde(flatten)]
    pub year: NetCapitalGainYear,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct WhatIfResponse {
    /// The disposal's tax year — the year the two scenario rows describe.
    pub tax_year: i32,
    /// The strategy used to derive the allocations, when one was named.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub strategy: Option<Strategy>,
    /// The hypothetical disposal's own totals (the realised-gains buckets).
    pub hypothetical: DisposalTotals,
    /// Its per-parcel allocations (derived or as supplied).
    pub allocations: Vec<HypotheticalAllocation>,
    /// The disposal year's figures without and with the disposal, computed
    /// through the full loss-chaining walk (earlier years' carried-forward
    /// losses included).
    pub years: Vec<ScenarioYear>,
}

/// Dry-run a hypothetical disposal through the net-capital-gain computation:
/// the disposal's gain/loss buckets are injected into its tax year and the
/// netting walk re-run — nothing is written. The whole-of-income tax estimate
/// is out of scope; this is the CGT-side delta only.
async fn what_if_handler(
    State(pool): State<SqlitePool>,
    Json(req): Json<WhatIfRequest>,
) -> Result<Json<WhatIfResponse>, ApiError> {
    if req.units <= Decimal::ZERO {
        return Err(ApiError::Unprocessable(
            "units must be positive".to_string(),
        ));
    }
    if req.proceeds < Decimal::ZERO {
        return Err(ApiError::Unprocessable(
            "proceeds must not be negative".to_string(),
        ));
    }

    let parcels =
        parcel_optimiser::db_candidate_parcels(&pool, req.listing_id, req.holding_account_id)
            .await
            .map_err(ApiError::from)?;

    // Exactly one of explicit allocations / a named strategy.
    let picks: Vec<(i64, Decimal)> = match (&req.allocations, req.strategy) {
        (Some(_), Some(_)) | (None, None) => {
            return Err(ApiError::Unprocessable(
                "supply either allocations or a strategy (exactly one)".to_string(),
            ));
        }
        (Some(allocs), None) => {
            let mut remaining: HashMap<i64, Decimal> = parcels
                .iter()
                .map(|p| (p.trade_id, p.remaining_quantity))
                .collect();
            let mut picks = Vec::with_capacity(allocs.len());
            for a in allocs {
                if a.units <= Decimal::ZERO {
                    return Err(ApiError::Unprocessable(format!(
                        "allocation units for parcel {} must be positive",
                        a.purchase_trade_id
                    )));
                }
                let Some(left) = remaining.get_mut(&a.purchase_trade_id) else {
                    return Err(ApiError::Unprocessable(format!(
                        "parcel {} is not an open parcel of listing {}{}",
                        a.purchase_trade_id,
                        req.listing_id,
                        req.holding_account_id
                            .map(|h| format!(" in holding account {h}"))
                            .unwrap_or_default()
                    )));
                };
                if a.units > *left {
                    return Err(ApiError::Unprocessable(format!(
                        "parcel {} has only {left} unit(s) remaining",
                        a.purchase_trade_id
                    )));
                }
                *left -= a.units;
                picks.push((a.purchase_trade_id, a.units));
            }
            let total: Decimal = picks.iter().map(|&(_, q)| q).sum();
            if total != req.units {
                return Err(ApiError::Unprocessable(format!(
                    "the allocations sum to {total}, not the {} units sold",
                    req.units
                )));
            }
            picks
        }
        (None, Some(strategy)) => {
            let open: Decimal = parcels.iter().map(|p| p.remaining_quantity).sum();
            if req.units > open {
                return Err(ApiError::Unprocessable(format!(
                    "only {open} unit(s) of listing {} are open",
                    req.listing_id
                )));
            }
            // The per-unit price the strategy's gain orderings see.
            let price = req.proceeds / req.units;
            parcel_optimiser::allocate_strategy(&parcels, req.units, price, req.date, strategy)
        }
    };

    let (allocations, totals) =
        parcel_optimiser::disposal_figures(&parcels, &picks, req.proceeds, req.units, req.date);

    // Re-run the report's own computation with the hypothetical's buckets
    // injected into the disposal year — and the year ensured in both runs, so
    // a year with no recorded activity still yields a row (with the correct
    // brought-forward chain from earlier years).
    let tax_year = tax_year_for(req.date);
    let buckets = gross_buckets(&pool).await.map_err(ApiError::from)?;
    let opening = crate::entities::cgt_settings::db_opening_capital_loss(&pool)
        .await
        .map_err(ApiError::from)?;

    let mut without = buckets.clone();
    without.entry(tax_year).or_default();
    let mut with = buckets;
    let b = with.entry(tax_year).or_default();
    b.discount_eligible += totals.discount_eligible_gain;
    b.other += totals.non_discountable_gain;
    b.losses += totals.capital_loss;

    let year_row = |rows: Vec<NetCapitalGainYear>, scenario: &str| {
        rows.into_iter()
            .find(|y| y.tax_year == tax_year)
            .map(|year| ScenarioYear {
                scenario: scenario.to_string(),
                year,
            })
            .expect("the disposal year was ensured in the buckets")
    };
    let years = vec![
        year_row(net_years(without, opening), "without"),
        year_row(net_years(with, opening), "with"),
    ];

    Ok(Json(WhatIfResponse {
        tax_year,
        strategy: req.strategy,
        hypothetical: totals,
        allocations,
        years,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::entities::{amma, cgt_settings, corporate_action, rba_fx_rate, trade};
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

    async fn insert_trade(
        pool: &SqlitePool,
        id: i64,
        trade_type: trade::TradeType,
        listing_id: i64,
        date: NaiveDate,
        qty: Decimal,
        price: Decimal,
    ) {
        test_support::trade(id, listing_id, trade_type)
            .date(date)
            .qty(qty)
            .price(price)
            .insert(pool)
            .await;
    }

    async fn link_adjustment(
        pool: &SqlitePool,
        id: i64,
        amma_id: i64,
        trade_id: i64,
        qty: Decimal,
    ) {
        test_support::amit_adjustment(pool, id, amma_id, trade_id, qty).await;
    }

    fn make_amma(id: i64, listing_id: i64, year_end: NaiveDate) -> amma::AmmaStatement {
        test_support::amma(id, listing_id)
            .with(|a| {
                a.tax_year_end_date = year_end;
                a.date_received = year_end + chrono::Duration::days(60);
            })
            .build()
    }

    #[tokio::test]
    async fn db_empty_returns_empty() {
        let pool = test_pool().await;
        assert!(db_net_capital_gain(&pool).await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn db_discount_eligible_gain_is_halved() {
        let pool = test_pool().await;
        // Buy 100 @ $10 (Jan 2024), sell 100 @ $15 (Jun 2025) → held > 12 months.
        insert_listing(&pool, 1, "VAS").await;
        insert_trade(
            &pool,
            1,
            trade::TradeType::Buy,
            1,
            NaiveDate::from_ymd_opt(2024, 1, 1).unwrap(),
            Decimal::from(100),
            Decimal::from(10),
        )
        .await;
        insert_trade(
            &pool,
            2,
            trade::TradeType::Sell,
            1,
            NaiveDate::from_ymd_opt(2025, 6, 1).unwrap(),
            Decimal::from(100),
            Decimal::from(15),
        )
        .await;
        allocate(&pool, 1, 2, 1, Decimal::from(100)).await;

        let r = db_net_capital_gain(&pool).await.unwrap();
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].tax_year, 2025); // sale Jun 2025 → FY2025
        assert_eq!(r[0].discount_eligible_gains, Decimal::from(500));
        assert_eq!(r[0].other_gains, Decimal::ZERO);
        assert_eq!(r[0].capital_losses, Decimal::ZERO);
        assert_eq!(r[0].cgt_discount, Decimal::from(250));
        // Net capital gain = 500 × 50% = 250.
        assert_eq!(r[0].net_capital_gain, Decimal::from(250));
        assert_eq!(r[0].capital_loss_carried_forward, Decimal::ZERO);
    }

    /// The 50% rate is the Australian-resident-individual rate (other entity
    /// types are not modelled — scope decision 2026-06-07); every row states
    /// that assumption explicitly instead of leaving it implicit.
    #[tokio::test]
    async fn db_rows_state_the_individual_resident_basis() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "VAS").await;
        insert_trade(
            &pool,
            1,
            trade::TradeType::Buy,
            1,
            NaiveDate::from_ymd_opt(2024, 1, 1).unwrap(),
            Decimal::from(100),
            Decimal::from(10),
        )
        .await;
        insert_trade(
            &pool,
            2,
            trade::TradeType::Sell,
            1,
            NaiveDate::from_ymd_opt(2025, 6, 1).unwrap(),
            Decimal::from(100),
            Decimal::from(15),
        )
        .await;
        allocate(&pool, 1, 2, 1, Decimal::from(100)).await;

        let r = db_net_capital_gain(&pool).await.unwrap();
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].taxpayer_basis, crate::reports::TAXPAYER_BASIS);
        // The assumption ships in the CSV export too (CSV_HEADER names it); a
        // comma in the text would split it across CSV fields.
        assert!(CSV_HEADER.contains(&"taxpayer_basis"));
        assert!(!crate::reports::TAXPAYER_BASIS.contains(','));
    }

    #[tokio::test]
    async fn db_short_term_gain_not_discounted() {
        let pool = test_pool().await;
        // Held ≤ 12 months → non-discountable; full gain assessable.
        insert_listing(&pool, 1, "VAS").await;
        insert_trade(
            &pool,
            1,
            trade::TradeType::Buy,
            1,
            NaiveDate::from_ymd_opt(2024, 1, 1).unwrap(),
            Decimal::from(100),
            Decimal::from(10),
        )
        .await;
        insert_trade(
            &pool,
            2,
            trade::TradeType::Sell,
            1,
            NaiveDate::from_ymd_opt(2024, 6, 1).unwrap(),
            Decimal::from(100),
            Decimal::from(15),
        )
        .await;
        allocate(&pool, 1, 2, 1, Decimal::from(100)).await;

        let r = db_net_capital_gain(&pool).await.unwrap();
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].other_gains, Decimal::from(500));
        assert_eq!(r[0].discount_eligible_gains, Decimal::ZERO);
        assert_eq!(r[0].cgt_discount, Decimal::ZERO);
        assert_eq!(r[0].net_capital_gain, Decimal::from(500));
    }

    #[tokio::test]
    async fn db_losses_applied_to_non_discount_gains_first() {
        let pool = test_pool().await;
        // FY2025: a discount-eligible gain of 500 and a non-discountable gain of 200,
        // plus a 100 capital loss. ATO-optimal: loss hits the non-discountable gain
        // first → net_other = 100, net_discount = 500, NCG = 100 + 250 = 350.
        insert_listing(&pool, 1, "VAS").await;
        // Discount-eligible: buy Jan 2024, sell Jun 2025 (>12mo), gain 500.
        insert_trade(
            &pool,
            1,
            trade::TradeType::Buy,
            1,
            NaiveDate::from_ymd_opt(2024, 1, 1).unwrap(),
            Decimal::from(100),
            Decimal::from(10),
        )
        .await;
        insert_trade(
            &pool,
            2,
            trade::TradeType::Sell,
            1,
            NaiveDate::from_ymd_opt(2025, 6, 1).unwrap(),
            Decimal::from(100),
            Decimal::from(15),
        )
        .await;
        allocate(&pool, 1, 2, 1, Decimal::from(100)).await;
        // Non-discountable: buy Mar 2025, sell Jun 2025 (≤12mo), gain 200.
        insert_trade(
            &pool,
            3,
            trade::TradeType::Buy,
            1,
            NaiveDate::from_ymd_opt(2025, 3, 1).unwrap(),
            Decimal::from(100),
            Decimal::from(10),
        )
        .await;
        insert_trade(
            &pool,
            4,
            trade::TradeType::Sell,
            1,
            NaiveDate::from_ymd_opt(2025, 6, 2).unwrap(),
            Decimal::from(100),
            Decimal::from(12),
        )
        .await;
        allocate(&pool, 2, 4, 3, Decimal::from(100)).await;
        // Capital loss of 100: buy Mar 2025 @ $10, sell Jun 2025 @ $9.
        insert_trade(
            &pool,
            5,
            trade::TradeType::Buy,
            1,
            NaiveDate::from_ymd_opt(2025, 3, 1).unwrap(),
            Decimal::from(100),
            Decimal::from(10),
        )
        .await;
        insert_trade(
            &pool,
            6,
            trade::TradeType::Sell,
            1,
            NaiveDate::from_ymd_opt(2025, 6, 3).unwrap(),
            Decimal::from(100),
            Decimal::from(9),
        )
        .await;
        allocate(&pool, 3, 6, 5, Decimal::from(100)).await;

        let r = db_net_capital_gain(&pool).await.unwrap();
        assert_eq!(r.len(), 1);
        let y = &r[0];
        assert_eq!(y.tax_year, 2025);
        assert_eq!(y.discount_eligible_gains, Decimal::from(500));
        assert_eq!(y.other_gains, Decimal::from(200));
        assert_eq!(y.capital_losses, Decimal::from(100));
        assert_eq!(y.net_other_gain, Decimal::from(100)); // 200 − 100
        assert_eq!(y.net_discount_eligible_gain, Decimal::from(500)); // untouched
        assert_eq!(y.net_capital_gain, Decimal::from(350)); // 100 + 500/2
        assert_eq!(y.capital_loss_carried_forward, Decimal::ZERO);
    }

    #[tokio::test]
    async fn db_losses_spill_into_discount_gains_then_carry_forward() {
        let pool = test_pool().await;
        // Discount-eligible gain 500, no other gains, capital loss 700.
        // Loss exhausts other (0), then reduces discount gain to 0 (uses 500),
        // leaving 200 carried forward. NCG = 0.
        insert_listing(&pool, 1, "VAS").await;
        insert_trade(
            &pool,
            1,
            trade::TradeType::Buy,
            1,
            NaiveDate::from_ymd_opt(2024, 1, 1).unwrap(),
            Decimal::from(100),
            Decimal::from(10),
        )
        .await;
        insert_trade(
            &pool,
            2,
            trade::TradeType::Sell,
            1,
            NaiveDate::from_ymd_opt(2025, 6, 1).unwrap(),
            Decimal::from(100),
            Decimal::from(15),
        )
        .await;
        allocate(&pool, 1, 2, 1, Decimal::from(100)).await;
        // Loss of 700: buy 100 @ $17, sell 100 @ $10 (Jun 2025).
        insert_trade(
            &pool,
            3,
            trade::TradeType::Buy,
            1,
            NaiveDate::from_ymd_opt(2025, 1, 1).unwrap(),
            Decimal::from(100),
            Decimal::from(17),
        )
        .await;
        insert_trade(
            &pool,
            4,
            trade::TradeType::Sell,
            1,
            NaiveDate::from_ymd_opt(2025, 6, 2).unwrap(),
            Decimal::from(100),
            Decimal::from(10),
        )
        .await;
        allocate(&pool, 2, 4, 3, Decimal::from(100)).await;

        let r = db_net_capital_gain(&pool).await.unwrap();
        assert_eq!(r.len(), 1);
        let y = &r[0];
        assert_eq!(y.capital_losses, Decimal::from(700));
        assert_eq!(y.net_discount_eligible_gain, Decimal::ZERO);
        assert_eq!(y.net_capital_gain, Decimal::ZERO);
        assert_eq!(y.capital_loss_carried_forward, Decimal::from(200));
    }

    #[tokio::test]
    async fn db_earlier_year_loss_reduces_later_year_gain() {
        let pool = test_pool().await;
        // FY2024: capital loss 300, no gains → carried forward 300.
        // FY2026: non-discountable gain 500 → brought-forward 300 applied → NCG 200.
        insert_listing(&pool, 1, "VAS").await;
        // Loss of 300: buy 100 @ $10 (Jul 2023), sell 100 @ $7 (Jun 2024).
        insert_trade(
            &pool,
            1,
            trade::TradeType::Buy,
            1,
            NaiveDate::from_ymd_opt(2023, 7, 1).unwrap(),
            Decimal::from(100),
            Decimal::from(10),
        )
        .await;
        insert_trade(
            &pool,
            2,
            trade::TradeType::Sell,
            1,
            NaiveDate::from_ymd_opt(2024, 6, 1).unwrap(),
            Decimal::from(100),
            Decimal::from(7),
        )
        .await;
        allocate(&pool, 1, 2, 1, Decimal::from(100)).await;
        // Gain of 500 two FYs later (≤12mo → non-discountable): buy Mar 2026, sell Jun 2026.
        insert_trade(
            &pool,
            3,
            trade::TradeType::Buy,
            1,
            NaiveDate::from_ymd_opt(2026, 3, 1).unwrap(),
            Decimal::from(100),
            Decimal::from(10),
        )
        .await;
        insert_trade(
            &pool,
            4,
            trade::TradeType::Sell,
            1,
            NaiveDate::from_ymd_opt(2026, 6, 1).unwrap(),
            Decimal::from(100),
            Decimal::from(15),
        )
        .await;
        allocate(&pool, 2, 4, 3, Decimal::from(100)).await;

        let r = db_net_capital_gain(&pool).await.unwrap();
        assert_eq!(r.len(), 2);
        // FY2024: the loss year.
        assert_eq!(r[0].tax_year, 2024);
        assert_eq!(r[0].capital_losses, Decimal::from(300));
        assert_eq!(r[0].capital_loss_brought_forward, Decimal::ZERO);
        assert_eq!(r[0].net_capital_gain, Decimal::ZERO);
        assert_eq!(r[0].capital_loss_carried_forward, Decimal::from(300));
        // FY2026: the chained loss offsets the gain before the discount.
        assert_eq!(r[1].tax_year, 2026);
        assert_eq!(r[1].capital_loss_brought_forward, Decimal::from(300));
        assert_eq!(r[1].capital_losses, Decimal::ZERO);
        assert_eq!(r[1].other_gains, Decimal::from(500));
        assert_eq!(r[1].net_other_gain, Decimal::from(200));
        assert_eq!(r[1].net_capital_gain, Decimal::from(200));
        assert_eq!(r[1].capital_loss_carried_forward, Decimal::ZERO);
    }

    #[tokio::test]
    async fn db_loss_absorbing_later_gains_leaves_zero_and_carries_remainder() {
        let pool = test_pool().await;
        // FY2024: capital loss 1000. FY2025: discount-eligible gain 500.
        // Brought-forward 1000 absorbs the full gain → NCG 0, 500 carried on.
        insert_listing(&pool, 1, "VAS").await;
        insert_trade(
            &pool,
            1,
            trade::TradeType::Buy,
            1,
            NaiveDate::from_ymd_opt(2023, 7, 1).unwrap(),
            Decimal::from(100),
            Decimal::from(20),
        )
        .await;
        insert_trade(
            &pool,
            2,
            trade::TradeType::Sell,
            1,
            NaiveDate::from_ymd_opt(2024, 6, 1).unwrap(),
            Decimal::from(100),
            Decimal::from(10),
        )
        .await;
        allocate(&pool, 1, 2, 1, Decimal::from(100)).await;
        // Discount-eligible gain 500: buy Jan 2024, sell Jun 2025 (>12mo).
        insert_trade(
            &pool,
            3,
            trade::TradeType::Buy,
            1,
            NaiveDate::from_ymd_opt(2024, 1, 1).unwrap(),
            Decimal::from(100),
            Decimal::from(10),
        )
        .await;
        insert_trade(
            &pool,
            4,
            trade::TradeType::Sell,
            1,
            NaiveDate::from_ymd_opt(2025, 6, 1).unwrap(),
            Decimal::from(100),
            Decimal::from(15),
        )
        .await;
        allocate(&pool, 2, 4, 3, Decimal::from(100)).await;

        let r = db_net_capital_gain(&pool).await.unwrap();
        assert_eq!(r.len(), 2);
        assert_eq!(r[0].capital_loss_carried_forward, Decimal::from(1000));
        assert_eq!(r[1].tax_year, 2025);
        assert_eq!(r[1].capital_loss_brought_forward, Decimal::from(1000));
        assert_eq!(r[1].discount_eligible_gains, Decimal::from(500));
        assert_eq!(r[1].net_discount_eligible_gain, Decimal::ZERO);
        assert_eq!(r[1].cgt_discount, Decimal::ZERO);
        assert_eq!(r[1].net_capital_gain, Decimal::ZERO);
        assert_eq!(r[1].capital_loss_carried_forward, Decimal::from(500));
    }

    #[tokio::test]
    async fn db_opening_capital_loss_is_applied_as_starting_balance() {
        let pool = test_pool().await;
        // Entered opening carried-forward loss of 400 (pre-system years), then a
        // FY2025 discount-eligible gain of 500 → 100 remains, halved → NCG 50.
        cgt_settings::db_upsert(
            &pool,
            &cgt_settings::CgtSettings {
                id: 1,
                opening_capital_loss: Decimal::from(400),
            },
        )
        .await
        .unwrap();
        insert_listing(&pool, 1, "VAS").await;
        insert_trade(
            &pool,
            1,
            trade::TradeType::Buy,
            1,
            NaiveDate::from_ymd_opt(2024, 1, 1).unwrap(),
            Decimal::from(100),
            Decimal::from(10),
        )
        .await;
        insert_trade(
            &pool,
            2,
            trade::TradeType::Sell,
            1,
            NaiveDate::from_ymd_opt(2025, 6, 1).unwrap(),
            Decimal::from(100),
            Decimal::from(15),
        )
        .await;
        allocate(&pool, 1, 2, 1, Decimal::from(100)).await;

        let r = db_net_capital_gain(&pool).await.unwrap();
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].tax_year, 2025);
        assert_eq!(r[0].capital_loss_brought_forward, Decimal::from(400));
        assert_eq!(r[0].discount_eligible_gains, Decimal::from(500));
        assert_eq!(r[0].net_discount_eligible_gain, Decimal::from(100));
        assert_eq!(r[0].cgt_discount, Decimal::from(50));
        assert_eq!(r[0].net_capital_gain, Decimal::from(50));
        assert_eq!(r[0].capital_loss_carried_forward, Decimal::ZERO);
    }

    #[tokio::test]
    async fn db_opening_loss_chains_through_a_loss_year_in_order() {
        let pool = test_pool().await;
        // Opening 100 + FY2024 loss 300 = 400 carried into FY2025's gain of 500
        // (≤12mo, non-discountable) → NCG 100.
        cgt_settings::db_upsert(
            &pool,
            &cgt_settings::CgtSettings {
                id: 1,
                opening_capital_loss: Decimal::from(100),
            },
        )
        .await
        .unwrap();
        insert_listing(&pool, 1, "VAS").await;
        // FY2024 loss of 300.
        insert_trade(
            &pool,
            1,
            trade::TradeType::Buy,
            1,
            NaiveDate::from_ymd_opt(2023, 7, 1).unwrap(),
            Decimal::from(100),
            Decimal::from(10),
        )
        .await;
        insert_trade(
            &pool,
            2,
            trade::TradeType::Sell,
            1,
            NaiveDate::from_ymd_opt(2024, 6, 1).unwrap(),
            Decimal::from(100),
            Decimal::from(7),
        )
        .await;
        allocate(&pool, 1, 2, 1, Decimal::from(100)).await;
        // FY2025 non-discountable gain of 500.
        insert_trade(
            &pool,
            3,
            trade::TradeType::Buy,
            1,
            NaiveDate::from_ymd_opt(2025, 3, 1).unwrap(),
            Decimal::from(100),
            Decimal::from(10),
        )
        .await;
        insert_trade(
            &pool,
            4,
            trade::TradeType::Sell,
            1,
            NaiveDate::from_ymd_opt(2025, 6, 1).unwrap(),
            Decimal::from(100),
            Decimal::from(15),
        )
        .await;
        allocate(&pool, 2, 4, 3, Decimal::from(100)).await;

        let r = db_net_capital_gain(&pool).await.unwrap();
        assert_eq!(r.len(), 2);
        assert_eq!(r[0].tax_year, 2024);
        assert_eq!(r[0].capital_loss_brought_forward, Decimal::from(100));
        assert_eq!(r[0].capital_loss_carried_forward, Decimal::from(400));
        assert_eq!(r[1].tax_year, 2025);
        assert_eq!(r[1].capital_loss_brought_forward, Decimal::from(400));
        assert_eq!(r[1].net_capital_gain, Decimal::from(100));
        assert_eq!(r[1].capital_loss_carried_forward, Decimal::ZERO);
    }

    #[tokio::test]
    async fn db_amma_discount_gains_grossed_up_then_halved() {
        let pool = test_pool().await;
        // AMMA discount-method gain stored as the net (already-halved) $100.
        // Grossed up ×2 = 200 discount-eligible; net capital gain = 200/2 = 100.
        insert_listing(&pool, 1, "VAF").await;
        let mut a = make_amma(1, 1, NaiveDate::from_ymd_opt(2024, 6, 30).unwrap());
        a.cgt_discount_gains = Decimal::from(100);
        amma::db_upsert(&pool, &a).await.unwrap();

        let r = db_net_capital_gain(&pool).await.unwrap();
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].tax_year, 2024);
        assert_eq!(r[0].discount_eligible_gains, Decimal::from(200));
        assert_eq!(r[0].cgt_discount, Decimal::from(100));
        assert_eq!(r[0].net_capital_gain, Decimal::from(100));
    }

    #[tokio::test]
    async fn db_amma_indexation_other_gains_and_losses() {
        let pool = test_pool().await;
        // Indexation 30 + other 20 = 50 non-discountable; capital losses applied 10.
        // Loss hits non-discountable first → net_other = 40, NCG = 40.
        insert_listing(&pool, 1, "VAF").await;
        let mut a = make_amma(1, 1, NaiveDate::from_ymd_opt(2024, 6, 30).unwrap());
        a.cgt_indexation_gains = Decimal::from(30);
        a.cgt_other_gains = Decimal::from(20);
        a.capital_losses_applied = Decimal::from(10);
        amma::db_upsert(&pool, &a).await.unwrap();

        let r = db_net_capital_gain(&pool).await.unwrap();
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].other_gains, Decimal::from(50));
        assert_eq!(r[0].capital_losses, Decimal::from(10));
        assert_eq!(r[0].net_other_gain, Decimal::from(40));
        assert_eq!(r[0].net_capital_gain, Decimal::from(40));
    }

    #[tokio::test]
    async fn db_realised_and_amma_combined_in_one_year() {
        let pool = test_pool().await;
        // FY2024: realised discount-eligible gain 500 (sale May 2024) + AMMA discount
        // gain net 100 (gross 200) → discount-eligible 700; NCG = 700/2 = 350.
        insert_listing(&pool, 1, "VAF").await;
        insert_trade(
            &pool,
            1,
            trade::TradeType::Buy,
            1,
            NaiveDate::from_ymd_opt(2023, 1, 1).unwrap(),
            Decimal::from(100),
            Decimal::from(10),
        )
        .await;
        insert_trade(
            &pool,
            2,
            trade::TradeType::Sell,
            1,
            NaiveDate::from_ymd_opt(2024, 5, 1).unwrap(),
            Decimal::from(100),
            Decimal::from(15),
        )
        .await;
        allocate(&pool, 1, 2, 1, Decimal::from(100)).await;
        let mut a = make_amma(1, 1, NaiveDate::from_ymd_opt(2024, 6, 30).unwrap());
        a.cgt_discount_gains = Decimal::from(100);
        amma::db_upsert(&pool, &a).await.unwrap();

        let r = db_net_capital_gain(&pool).await.unwrap();
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].tax_year, 2024);
        assert_eq!(r[0].discount_eligible_gains, Decimal::from(700));
        assert_eq!(r[0].net_capital_gain, Decimal::from(350));
    }

    /// A rights sale's gain enters the year's buckets through the realised
    /// report: anchored to a >12-month parcel it is discount-eligible, so the
    /// net capital gain halves it (docs/ato/rights-issues.md Example 39).
    #[tokio::test]
    async fn db_rights_sale_gain_enters_the_year_buckets() {
        use crate::entities::{corporate_action, rights_sale};
        let pool = test_pool().await;
        insert_listing(&pool, 1, "RTS").await;
        insert_trade(
            &pool,
            1,
            trade::TradeType::Buy,
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
        // 250 rights sold at 20c in July 2024 → FY2025, $50 discount-eligible.
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

        let r = db_net_capital_gain(&pool).await.unwrap();
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].tax_year, 2025);
        assert_eq!(r[0].discount_eligible_gains, Decimal::from(50));
        assert_eq!(r[0].net_capital_gain, Decimal::from(25));
    }

    #[tokio::test]
    async fn db_amma_non_aud_converted_via_ato_rate() {
        let pool = test_pool().await;
        // USD AMMA discount gain net US$50 with A$1 = 0.50 USD (Jun 2024).
        // AUD net = 100, gross ×2 = 200, NCG = 100.
        insert_listing(&pool, 1, "VAF").await;
        rba_fx_rate::db_import_rate(&pool, "USD", "2024-06", "0.50".parse().unwrap())
            .await
            .unwrap();
        let mut a = make_amma(1, 1, NaiveDate::from_ymd_opt(2024, 6, 30).unwrap());
        a.currency = "USD".to_string();
        a.cgt_discount_gains = Decimal::from(50);
        amma::db_upsert(&pool, &a).await.unwrap();

        let r = db_net_capital_gain(&pool).await.unwrap();
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].discount_eligible_gains, Decimal::from(200));
        assert_eq!(r[0].net_capital_gain, Decimal::from(100));
    }

    #[tokio::test]
    async fn db_amma_non_aud_without_rate_fails_loudly() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "VAF").await;
        let mut a = make_amma(1, 1, NaiveDate::from_ymd_opt(2024, 6, 30).unwrap());
        a.currency = "USD".to_string();
        a.cgt_discount_gains = Decimal::from(50);
        amma::db_upsert(&pool, &a).await.unwrap();

        assert!(db_net_capital_gain(&pool).await.is_err());
    }

    #[tokio::test]
    async fn db_sorted_by_tax_year() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "VAF").await;
        let mut a1 = make_amma(1, 1, NaiveDate::from_ymd_opt(2025, 6, 30).unwrap());
        a1.cgt_other_gains = Decimal::from(10);
        amma::db_upsert(&pool, &a1).await.unwrap();
        let mut a2 = make_amma(2, 1, NaiveDate::from_ymd_opt(2024, 6, 30).unwrap());
        a2.cgt_other_gains = Decimal::from(20);
        amma::db_upsert(&pool, &a2).await.unwrap();

        let r = db_net_capital_gain(&pool).await.unwrap();
        assert_eq!(r.len(), 2);
        assert_eq!(r[0].tax_year, 2024);
        assert_eq!(r[1].tax_year, 2025);
    }

    #[tokio::test]
    async fn db_e10_excess_reduction_becomes_capital_gain() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "VAF").await;
        // Buy 100 @ $1 → cost base $100; held ~6 months at the 30 Jun 2024 year end.
        insert_trade(
            &pool,
            1,
            trade::TradeType::Buy,
            1,
            NaiveDate::from_ymd_opt(2024, 1, 1).unwrap(),
            Decimal::from(100),
            Decimal::from(1),
        )
        .await;
        // AMMA reduces cost base by $1.50/unit × 100 = $150 → $50 excess over the $100 base.
        let mut a = make_amma(1, 1, NaiveDate::from_ymd_opt(2024, 6, 30).unwrap());
        a.cost_base_adjustment = "1.50".parse().unwrap();
        amma::db_upsert(&pool, &a).await.unwrap();
        link_adjustment(&pool, 1, 1, 1, Decimal::from(100)).await;

        let r = db_net_capital_gain(&pool).await.unwrap();
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].tax_year, 2024);
        assert_eq!(r[0].cgt_event_e10_gain, Decimal::from(50));
        // Held ≤ 12 months as at the year end → non-discountable; fully assessable.
        assert_eq!(r[0].other_gains, Decimal::from(50));
        assert_eq!(r[0].discount_eligible_gains, Decimal::ZERO);
        assert_eq!(r[0].net_capital_gain, Decimal::from(50));
    }

    #[tokio::test]
    async fn db_e10_gain_discount_eligible_when_held_over_12_months() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "VAF").await;
        // Bought Jan 2023 → held > 12 months at the 30 Jun 2024 year end.
        insert_trade(
            &pool,
            1,
            trade::TradeType::Buy,
            1,
            NaiveDate::from_ymd_opt(2023, 1, 1).unwrap(),
            Decimal::from(100),
            Decimal::from(1),
        )
        .await;
        let mut a = make_amma(1, 1, NaiveDate::from_ymd_opt(2024, 6, 30).unwrap());
        a.cost_base_adjustment = "1.50".parse().unwrap();
        amma::db_upsert(&pool, &a).await.unwrap();
        link_adjustment(&pool, 1, 1, 1, Decimal::from(100)).await;

        let r = db_net_capital_gain(&pool).await.unwrap();
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].cgt_event_e10_gain, Decimal::from(50));
        // Discount-eligible → halved.
        assert_eq!(r[0].discount_eligible_gains, Decimal::from(50));
        assert_eq!(r[0].other_gains, Decimal::ZERO);
        assert_eq!(r[0].cgt_discount, Decimal::from(25));
        assert_eq!(r[0].net_capital_gain, Decimal::from(25));
    }

    #[tokio::test]
    async fn db_e10_accumulates_across_years_fires_when_cost_base_exhausted() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "VAF").await;
        // Buy 100 @ $1 → cost base $100, bought Jan 2024.
        insert_trade(
            &pool,
            1,
            trade::TradeType::Buy,
            1,
            NaiveDate::from_ymd_opt(2024, 1, 1).unwrap(),
            Decimal::from(100),
            Decimal::from(1),
        )
        .await;
        // FY2024: reduce $0.60/unit × 100 = $60 → cost base $40 remaining, no excess.
        let mut a1 = make_amma(1, 1, NaiveDate::from_ymd_opt(2024, 6, 30).unwrap());
        a1.cost_base_adjustment = "0.60".parse().unwrap();
        amma::db_upsert(&pool, &a1).await.unwrap();
        link_adjustment(&pool, 1, 1, 1, Decimal::from(100)).await;
        // FY2025: reduce $0.70/unit × 100 = $70 > $40 remaining → $30 excess (E10) in FY2025.
        let mut a2 = make_amma(2, 1, NaiveDate::from_ymd_opt(2025, 6, 30).unwrap());
        a2.cost_base_adjustment = "0.70".parse().unwrap();
        amma::db_upsert(&pool, &a2).await.unwrap();
        link_adjustment(&pool, 2, 2, 1, Decimal::from(100)).await;

        let r = db_net_capital_gain(&pool).await.unwrap();
        // Both AMMA statements create a year bucket; only FY2025 carries the E10 gain.
        assert_eq!(r.len(), 2);
        assert_eq!(r[0].tax_year, 2024);
        assert_eq!(r[0].cgt_event_e10_gain, Decimal::ZERO);
        assert_eq!(r[0].net_capital_gain, Decimal::ZERO);
        assert_eq!(r[1].tax_year, 2025);
        assert_eq!(r[1].cgt_event_e10_gain, Decimal::from(30));
        // Held > 12 months at the FY2025 year end → discount-eligible → $30/2 = $15.
        assert_eq!(r[1].discount_eligible_gains, Decimal::from(30));
        assert_eq!(r[1].net_capital_gain, Decimal::from(15));
    }

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

    /// A payment after a 2-for-1 split is per *post-split* unit, so the parcel
    /// receives it on twice the units; the G1 excess reflects that (TD 2000/10
    /// re-basing in `docs/ato/share-splits-and-consolidations.md`).
    #[tokio::test]
    async fn db_g1_payment_after_split_scales_to_post_split_units() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "SPL").await;
        // Buy 100 @ $1 → cost base $100 (Jan 2024); 2-for-1 split in March.
        insert_trade(
            &pool,
            1,
            trade::TradeType::Buy,
            1,
            NaiveDate::from_ymd_opt(2024, 1, 1).unwrap(),
            Decimal::from(100),
            Decimal::from(1),
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
        // 75c per post-split unit × 200 units = $150 → $50 excess over the
        // unchanged $100 cost base.
        apply_roc(
            &pool,
            2,
            1,
            NaiveDate::from_ymd_opt(2024, 6, 1).unwrap(),
            "0.75",
        )
        .await;

        let r = db_net_capital_gain(&pool).await.unwrap();
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].tax_year, 2024);
        assert_eq!(r[0].cgt_event_g1_gain, Decimal::from(50));
        assert_eq!(r[0].net_capital_gain, Decimal::from(50));
    }

    /// CGT event G1: a return-of-capital payment exceeding the parcel's cost
    /// base produces a capital gain equal to the excess, in the payment's income
    /// year (`docs/ato/cgt-non-assessable-payments.md`).
    #[tokio::test]
    async fn db_g1_excess_payment_becomes_capital_gain() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "RAP").await;
        // Buy 100 @ $1 → cost base $100; held ~5 months at the payment date.
        insert_trade(
            &pool,
            1,
            trade::TradeType::Buy,
            1,
            NaiveDate::from_ymd_opt(2024, 1, 1).unwrap(),
            Decimal::from(100),
            Decimal::from(1),
        )
        .await;
        // $1.50/unit × 100 = $150 payment → $50 excess over the $100 cost base.
        apply_roc(
            &pool,
            1,
            1,
            NaiveDate::from_ymd_opt(2024, 6, 1).unwrap(),
            "1.50",
        )
        .await;

        let r = db_net_capital_gain(&pool).await.unwrap();
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].tax_year, 2024);
        assert_eq!(r[0].cgt_event_g1_gain, Decimal::from(50));
        // Held ≤ 12 months at the payment date → non-discountable; fully assessable.
        assert_eq!(r[0].other_gains, Decimal::from(50));
        assert_eq!(r[0].discount_eligible_gains, Decimal::ZERO);
        assert_eq!(r[0].net_capital_gain, Decimal::from(50));
    }

    /// A payment within the cost base produces no gain at all (and G1 can never
    /// produce a capital loss) — Rob's Example 45 shape.
    #[tokio::test]
    async fn db_g1_payment_within_cost_base_produces_no_gain() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "RAP").await;
        insert_trade(
            &pool,
            1,
            trade::TradeType::Buy,
            1,
            NaiveDate::from_ymd_opt(2024, 1, 1).unwrap(),
            Decimal::from(1500),
            Decimal::from(5),
        )
        .await;
        apply_roc(
            &pool,
            1,
            1,
            NaiveDate::from_ymd_opt(2024, 11, 30).unwrap(),
            "0.50",
        )
        .await;

        let r = db_net_capital_gain(&pool).await.unwrap();
        assert!(
            r.iter().all(|y| y.net_capital_gain == Decimal::ZERO
                && y.cgt_event_g1_gain == Decimal::ZERO
                && y.capital_losses == Decimal::ZERO),
            "payment not more than cost base → no gain, and never a loss"
        );
    }

    #[tokio::test]
    async fn db_g1_gain_discount_eligible_when_held_over_12_months() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "RAP").await;
        // Bought Jan 2023 → held > 12 months at the Jun 2024 payment date.
        insert_trade(
            &pool,
            1,
            trade::TradeType::Buy,
            1,
            NaiveDate::from_ymd_opt(2023, 1, 1).unwrap(),
            Decimal::from(100),
            Decimal::from(1),
        )
        .await;
        apply_roc(
            &pool,
            1,
            1,
            NaiveDate::from_ymd_opt(2024, 6, 1).unwrap(),
            "1.50",
        )
        .await;

        let r = db_net_capital_gain(&pool).await.unwrap();
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].cgt_event_g1_gain, Decimal::from(50));
        // Discount-eligible → halved.
        assert_eq!(r[0].discount_eligible_gains, Decimal::from(50));
        assert_eq!(r[0].other_gains, Decimal::ZERO);
        assert_eq!(r[0].cgt_discount, Decimal::from(25));
        assert_eq!(r[0].net_capital_gain, Decimal::from(25));
    }

    #[tokio::test]
    async fn db_g1_accumulates_across_payments_fires_when_cost_base_exhausted() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "RAP").await;
        // Buy 100 @ $1 → cost base $100, bought Jan 2024.
        insert_trade(
            &pool,
            1,
            trade::TradeType::Buy,
            1,
            NaiveDate::from_ymd_opt(2024, 1, 1).unwrap(),
            Decimal::from(100),
            Decimal::from(1),
        )
        .await;
        // FY2024: 60c/unit × 100 = $60 → $40 cost base remains, no excess.
        apply_roc(
            &pool,
            1,
            1,
            NaiveDate::from_ymd_opt(2024, 6, 1).unwrap(),
            "0.60",
        )
        .await;
        // FY2025: 70c/unit × 100 = $70 > $40 remaining → $30 excess (G1) in FY2025.
        apply_roc(
            &pool,
            2,
            1,
            NaiveDate::from_ymd_opt(2025, 6, 1).unwrap(),
            "0.70",
        )
        .await;

        let r = db_net_capital_gain(&pool).await.unwrap();
        // Only FY2025 carries a gain (FY2024's payment stayed within cost base).
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].tax_year, 2025);
        assert_eq!(r[0].cgt_event_g1_gain, Decimal::from(30));
        // Held > 12 months at the second payment → discount-eligible → $30/2 = $15.
        assert_eq!(r[0].discount_eligible_gains, Decimal::from(30));
        assert_eq!(r[0].net_capital_gain, Decimal::from(15));
    }

    /// Only units still held at the payment date received it: a parcel partly
    /// sold before the payment realises only the held share of the excess.
    #[tokio::test]
    async fn db_g1_gain_scales_to_units_held_at_payment() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "RAP").await;
        // Buy 100 @ $1 (Jan 2024); sell 40 @ $1 (Mar 2024, no gain); then a
        // $1.50/unit payment lands on the 60 still held.
        insert_trade(
            &pool,
            1,
            trade::TradeType::Buy,
            1,
            NaiveDate::from_ymd_opt(2024, 1, 1).unwrap(),
            Decimal::from(100),
            Decimal::from(1),
        )
        .await;
        insert_trade(
            &pool,
            2,
            trade::TradeType::Sell,
            1,
            NaiveDate::from_ymd_opt(2024, 3, 1).unwrap(),
            Decimal::from(40),
            Decimal::from(1),
        )
        .await;
        allocate(&pool, 1, 2, 1, Decimal::from(40)).await;
        apply_roc(
            &pool,
            1,
            1,
            NaiveDate::from_ymd_opt(2024, 6, 1).unwrap(),
            "1.50",
        )
        .await;

        let r = db_net_capital_gain(&pool).await.unwrap();
        assert_eq!(r.len(), 1);
        // Per-unit excess 50c × 60 held units = $30 (not the whole-parcel $50).
        assert_eq!(r[0].cgt_event_g1_gain, Decimal::from(30));
        assert_eq!(r[0].net_capital_gain, Decimal::from(30));
    }

    #[tokio::test]
    async fn api_net_capital_gain_returns_json() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "VAS").await;
        insert_trade(
            &pool,
            1,
            trade::TradeType::Buy,
            1,
            NaiveDate::from_ymd_opt(2024, 1, 1).unwrap(),
            Decimal::from(100),
            Decimal::from(10),
        )
        .await;
        insert_trade(
            &pool,
            2,
            trade::TradeType::Sell,
            1,
            NaiveDate::from_ymd_opt(2025, 6, 1).unwrap(),
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
                    .uri("/portfolio/net-capital-gain")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let result: Vec<NetCapitalGainYear> = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].net_capital_gain, Decimal::from(250));
    }

    #[tokio::test]
    async fn api_export_returns_csv_with_expected_columns() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "VAS").await;
        // Held > 12 months: $500 gross gain → discounted to a $250 net capital gain.
        insert_trade(
            &pool,
            1,
            trade::TradeType::Buy,
            1,
            NaiveDate::from_ymd_opt(2024, 1, 1).unwrap(),
            Decimal::from(100),
            Decimal::from(10),
        )
        .await;
        insert_trade(
            &pool,
            2,
            trade::TradeType::Sell,
            1,
            NaiveDate::from_ymd_opt(2025, 6, 1).unwrap(),
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
                    .uri("/portfolio/net-capital-gain/export")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        assert_eq!(
            resp.headers()
                .get(axum::http::header::CONTENT_TYPE)
                .unwrap(),
            "text/csv; charset=utf-8"
        );
        assert_eq!(
            resp.headers()
                .get(axum::http::header::CONTENT_DISPOSITION)
                .unwrap(),
            "attachment; filename=\"net-capital-gain.csv\""
        );
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let csv = String::from_utf8(bytes.to_vec()).unwrap();
        let mut lines = csv.lines();
        // Header names every NetCapitalGainYear field, in declaration order.
        assert_eq!(lines.next().unwrap(), CSV_HEADER.join(","));
        let fields: Vec<&str> = lines.next().unwrap().split(',').collect();
        assert_eq!(fields.len(), CSV_HEADER.len());
        assert_eq!(fields[0], "2025"); // tax_year
        assert_eq!(fields[1].parse::<Decimal>().unwrap(), Decimal::from(500)); // discount_eligible_gains
        assert_eq!(fields[7].parse::<Decimal>().unwrap(), Decimal::from(250)); // cgt_discount
        assert_eq!(fields[8].parse::<Decimal>().unwrap(), Decimal::from(250)); // net_capital_gain
        assert_eq!(lines.next(), None);
    }

    #[tokio::test]
    async fn api_export_of_empty_report_still_returns_header() {
        let pool = test_pool().await;
        let resp = router()
            .with_state(pool)
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/portfolio/net-capital-gain/export")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let csv = String::from_utf8(bytes.to_vec()).unwrap();
        assert_eq!(csv, CSV_HEADER.join(",") + "\n");
    }

    /// A scrip-for-scrip rollover produces no net capital gain in the
    /// exchange year (the gain is disregarded), and a later sale of the
    /// replacement parcel is taxed on the carried cost base with the
    /// combined-period discount.
    #[tokio::test]
    async fn db_scrip_rollover_disregards_the_exchange_and_taxes_the_later_sale() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "OLD").await;
        insert_listing(&pool, 2, "NEW").await;
        // FY2021: buy 1,000 @ $1.50 = $1,500.
        insert_trade(
            &pool,
            1,
            trade::TradeType::Buy,
            1,
            NaiveDate::from_ymd_opt(2020, 10, 1).unwrap(),
            Decimal::from(1000),
            "1.50".parse().unwrap(),
        )
        .await;
        // FY2025: 2-for-1 takeover with rollover.
        corporate_action::db_upsert(
            &pool,
            &corporate_action::CorporateAction {
                id: 10,
                listing_id: 1,
                date: NaiveDate::from_ymd_opt(2024, 7, 1).unwrap(),
                kind: corporate_action::ActionKind::ScripForScrip {
                    scrip_listing_id: 2,
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
        let ex = crate::entities::scrip_exchange::db_exchange(&pool, 10)
            .await
            .unwrap();

        // The exchange alone: no tax year reports any gain or loss.
        let years = db_net_capital_gain(&pool).await.unwrap();
        assert!(years.is_empty(), "the rollover is disregarded: {years:?}");

        // FY2025 sale of the replacement: 2,000 units @ $1.00 = $2,000
        // proceeds − $1,500 carried cost base = $500, halved via the
        // combined-period discount → $250 net capital gain.
        insert_trade(
            &pool,
            50,
            trade::TradeType::Sell,
            2,
            NaiveDate::from_ymd_opt(2024, 10, 1).unwrap(),
            Decimal::from(2000),
            Decimal::ONE,
        )
        .await;
        allocate(&pool, 1, 50, ex.replacements[0].id, Decimal::from(2000)).await;

        let years = db_net_capital_gain(&pool).await.unwrap();
        assert_eq!(years.len(), 1);
        assert_eq!(years[0].tax_year, 2025);
        assert_eq!(years[0].discount_eligible_gains, Decimal::from(500));
        assert_eq!(years[0].cgt_discount, Decimal::from(250));
        assert_eq!(years[0].net_capital_gain, Decimal::from(250));
    }

    /// The demerger rollover (Div 125): the apportionment reports nothing in
    /// the demerger year (any gain is disregarded), and later sales on both
    /// sides are taxed on the apportioned cost bases with the
    /// combined-period discount.
    #[tokio::test]
    async fn db_demerger_rollover_disregards_the_demerge_and_taxes_the_later_sales() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "HEAD").await;
        insert_listing(&pool, 2, "DEM").await;
        // FY2021: buy 1,000 @ $1.50 = $1,500.
        insert_trade(
            &pool,
            1,
            trade::TradeType::Buy,
            1,
            NaiveDate::from_ymd_opt(2020, 10, 1).unwrap(),
            Decimal::from(1000),
            "1.50".parse().unwrap(),
        )
        .await;
        // FY2025: 1-for-5 demerger with rollover, 20% of cost base demerged.
        corporate_action::db_upsert(
            &pool,
            &corporate_action::CorporateAction {
                id: 10,
                listing_id: 1,
                date: NaiveDate::from_ymd_opt(2024, 7, 1).unwrap(),
                kind: corporate_action::ActionKind::Demerger {
                    demerger_listing_id: 2,
                    demerger_new_units: Decimal::ONE,
                    demerger_held_units: Decimal::from(5),
                    demerger_cost_base_pct: Decimal::from(20),
                },
            },
        )
        .await
        .unwrap();
        let dm = crate::entities::demerger::db_demerge(&pool, 10)
            .await
            .unwrap();

        // The demerge alone: no tax year reports any gain or loss.
        let years = db_net_capital_gain(&pool).await.unwrap();
        assert!(years.is_empty(), "the rollover is disregarded: {years:?}");

        // FY2025 sales of both sides: 1,000 head units @ $2.00 = $2,000 −
        // $1,200 head cost base = $800; 200 demerged units @ $3.00 = $600 −
        // $300 = $300. Both held since Oct 2020 (combined period) → $1,100
        // halved → $550 net capital gain.
        insert_trade(
            &pool,
            50,
            trade::TradeType::Sell,
            1,
            NaiveDate::from_ymd_opt(2024, 10, 1).unwrap(),
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
        insert_trade(
            &pool,
            51,
            trade::TradeType::Sell,
            2,
            NaiveDate::from_ymd_opt(2024, 10, 1).unwrap(),
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

        let years = db_net_capital_gain(&pool).await.unwrap();
        assert_eq!(years.len(), 1);
        assert_eq!(years[0].tax_year, 2025);
        assert_eq!(years[0].discount_eligible_gains, Decimal::from(1100));
        assert_eq!(years[0].cgt_discount, Decimal::from(550));
        assert_eq!(years[0].net_capital_gain, Decimal::from(550));
    }

    // ---- pre-sale what-if -------------------------------------------------

    async fn post_what_if(pool: SqlitePool, body: serde_json::Value) -> (StatusCode, Vec<u8>) {
        let resp = router()
            .with_state(pool)
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/portfolio/net-capital-gain/what-if")
                    .header("content-type", "application/json")
                    .body(Body::from(body.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        let status = resp.status();
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        (status, bytes.to_vec())
    }

    /// FY2026 already holds a realised non-discountable gain of 500; the
    /// hypothetical sale of the >12-month parcel adds a discount-eligible
    /// 1,000. `without` matches the plain report; `with` adds the disposal —
    /// and the database is untouched (a dry run).
    #[tokio::test]
    async fn api_what_if_reports_the_year_with_and_without_the_disposal() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "VAS").await;
        // Open parcel: 200 @ $10, bought Jan 2024 (>12mo by Jun 2026).
        insert_trade(
            &pool,
            1,
            trade::TradeType::Buy,
            1,
            NaiveDate::from_ymd_opt(2024, 1, 1).unwrap(),
            Decimal::from(200),
            Decimal::from(10),
        )
        .await;
        // Recorded FY2026 non-discountable gain of 500 on another parcel.
        insert_trade(
            &pool,
            2,
            trade::TradeType::Buy,
            1,
            NaiveDate::from_ymd_opt(2026, 1, 1).unwrap(),
            Decimal::from(100),
            Decimal::from(10),
        )
        .await;
        insert_trade(
            &pool,
            3,
            trade::TradeType::Sell,
            1,
            NaiveDate::from_ymd_opt(2026, 6, 1).unwrap(),
            Decimal::from(100),
            Decimal::from(15),
        )
        .await;
        allocate(&pool, 1, 3, 2, Decimal::from(100)).await;

        let baseline = db_net_capital_gain(&pool).await.unwrap();

        // Hypothetical: sell 100 of parcel 1 on 2026-06-15 for $2,000
        // (cost base $1,000 → discount-eligible gain $1,000).
        let (status, body) = post_what_if(
            pool.clone(),
            serde_json::json!({
                "listing_id": 1, "units": "100", "proceeds": "2000",
                "date": "2026-06-15",
                "allocations": [ { "purchase_trade_id": 1, "units": "100" } ]
            }),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let r: WhatIfResponse = serde_json::from_slice(&body).unwrap();
        assert_eq!(r.tax_year, 2026);
        assert_eq!(r.hypothetical.discount_eligible_gain, Decimal::from(1000));
        assert_eq!(r.hypothetical.capital_loss, Decimal::ZERO);
        assert_eq!(r.allocations.len(), 1);
        assert!(r.allocations[0].discount_eligible);

        assert_eq!(r.years.len(), 2);
        let without = &r.years[0];
        let with = &r.years[1];
        assert_eq!(without.scenario, "without");
        assert_eq!(without.year.net_capital_gain, Decimal::from(500));
        assert_eq!(with.scenario, "with");
        assert_eq!(with.year.discount_eligible_gains, Decimal::from(1000));
        // 500 non-discountable + 1,000/2 discounted = 1,000.
        assert_eq!(with.year.net_capital_gain, Decimal::from(1000));

        // Dry run: the stored report is unchanged, and no rows were written.
        let after = db_net_capital_gain(&pool).await.unwrap();
        assert_eq!(after.len(), baseline.len());
        assert_eq!(after[0].net_capital_gain, baseline[0].net_capital_gain);
        let trades: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM trades")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(trades, 3, "the what-if must not write trades");
        let allocs: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM parcel_allocations")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(allocs, 1, "the what-if must not write allocations");
    }

    /// A year with no recorded activity still yields both scenario rows, with
    /// the brought-forward chain from earlier years intact: a FY2024 loss of
    /// 300 offsets the hypothetical FY2026 gain before the discount.
    #[tokio::test]
    async fn api_what_if_on_an_empty_year_chains_earlier_losses() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "VAS").await;
        // FY2024 realised loss of 300.
        insert_trade(
            &pool,
            1,
            trade::TradeType::Buy,
            1,
            NaiveDate::from_ymd_opt(2023, 7, 1).unwrap(),
            Decimal::from(100),
            Decimal::from(10),
        )
        .await;
        insert_trade(
            &pool,
            2,
            trade::TradeType::Sell,
            1,
            NaiveDate::from_ymd_opt(2024, 6, 1).unwrap(),
            Decimal::from(100),
            Decimal::from(7),
        )
        .await;
        allocate(&pool, 1, 2, 1, Decimal::from(100)).await;
        // Open parcel bought Jan 2024.
        insert_trade(
            &pool,
            3,
            trade::TradeType::Buy,
            1,
            NaiveDate::from_ymd_opt(2024, 1, 1).unwrap(),
            Decimal::from(100),
            Decimal::from(10),
        )
        .await;

        // Sell all 100 on 2026-06-15 for 1,500 → gain 500, eligible.
        let (status, body) = post_what_if(
            pool,
            serde_json::json!({
                "listing_id": 1, "units": "100", "proceeds": "1500",
                "date": "2026-06-15", "strategy": "fifo"
            }),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let r: WhatIfResponse = serde_json::from_slice(&body).unwrap();
        assert_eq!(r.strategy, Some(Strategy::Fifo));
        // The FIFO strategy drew on the open parcel (parcel 1 is fully sold).
        assert_eq!(r.allocations.len(), 1);
        assert_eq!(r.allocations[0].purchase_trade_id, 3);
        // Without: an all-zero FY2026 row carrying the 300 loss forward.
        let without = &r.years[0];
        assert_eq!(without.year.tax_year, 2026);
        assert_eq!(
            without.year.capital_loss_brought_forward,
            Decimal::from(300)
        );
        assert_eq!(without.year.net_capital_gain, Decimal::ZERO);
        assert_eq!(
            without.year.capital_loss_carried_forward,
            Decimal::from(300)
        );
        // With: (500 − 300) / 2 = 100.
        let with = &r.years[1];
        assert_eq!(with.year.discount_eligible_gains, Decimal::from(500));
        assert_eq!(with.year.capital_loss_brought_forward, Decimal::from(300));
        assert_eq!(with.year.net_capital_gain, Decimal::from(100));
        assert_eq!(with.year.capital_loss_carried_forward, Decimal::ZERO);
    }

    #[tokio::test]
    async fn api_what_if_rejects_bad_allocations_and_modes() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "VAS").await;
        insert_trade(
            &pool,
            1,
            trade::TradeType::Buy,
            1,
            NaiveDate::from_ymd_opt(2024, 1, 1).unwrap(),
            Decimal::from(100),
            Decimal::from(10),
        )
        .await;

        // Neither allocations nor strategy.
        let (status, _) = post_what_if(
            pool.clone(),
            serde_json::json!({
                "listing_id": 1, "units": "10", "proceeds": "100", "date": "2026-06-15"
            }),
        )
        .await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);

        // Both at once.
        let (status, _) = post_what_if(
            pool.clone(),
            serde_json::json!({
                "listing_id": 1, "units": "10", "proceeds": "100", "date": "2026-06-15",
                "strategy": "fifo",
                "allocations": [ { "purchase_trade_id": 1, "units": "10" } ]
            }),
        )
        .await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);

        // Allocations not summing to the units.
        let (status, body) = post_what_if(
            pool.clone(),
            serde_json::json!({
                "listing_id": 1, "units": "10", "proceeds": "100", "date": "2026-06-15",
                "allocations": [ { "purchase_trade_id": 1, "units": "5" } ]
            }),
        )
        .await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
        assert!(String::from_utf8(body).unwrap().contains("sum to 5"));

        // Over-allocating the parcel.
        let (status, body) = post_what_if(
            pool.clone(),
            serde_json::json!({
                "listing_id": 1, "units": "150", "proceeds": "100", "date": "2026-06-15",
                "allocations": [ { "purchase_trade_id": 1, "units": "150" } ]
            }),
        )
        .await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
        assert!(String::from_utf8(body).unwrap().contains("only 100"));

        // A parcel that isn't an open parcel of the listing.
        let (status, body) = post_what_if(
            pool.clone(),
            serde_json::json!({
                "listing_id": 1, "units": "10", "proceeds": "100", "date": "2026-06-15",
                "allocations": [ { "purchase_trade_id": 99, "units": "10" } ]
            }),
        )
        .await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
        assert!(String::from_utf8(body).unwrap().contains("99"));

        // Strategy mode with more units than are open.
        let (status, _) = post_what_if(
            pool,
            serde_json::json!({
                "listing_id": 1, "units": "150", "proceeds": "100", "date": "2026-06-15",
                "strategy": "min_gain"
            }),
        )
        .await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    }

    /// A hypothetical loss enters the year's loss pool: it offsets the
    /// recorded non-discountable gain first, ATO-optimally.
    #[tokio::test]
    async fn api_what_if_loss_offsets_recorded_gains() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "VAS").await;
        // Recorded FY2026 non-discountable gain of 500.
        insert_trade(
            &pool,
            1,
            trade::TradeType::Buy,
            1,
            NaiveDate::from_ymd_opt(2026, 1, 1).unwrap(),
            Decimal::from(100),
            Decimal::from(10),
        )
        .await;
        insert_trade(
            &pool,
            2,
            trade::TradeType::Sell,
            1,
            NaiveDate::from_ymd_opt(2026, 6, 1).unwrap(),
            Decimal::from(100),
            Decimal::from(15),
        )
        .await;
        allocate(&pool, 1, 2, 1, Decimal::from(100)).await;
        // Open parcel at $10; hypothetically sold at $8 → loss 200.
        insert_trade(
            &pool,
            3,
            trade::TradeType::Buy,
            1,
            NaiveDate::from_ymd_opt(2025, 1, 1).unwrap(),
            Decimal::from(100),
            Decimal::from(10),
        )
        .await;

        let (status, body) = post_what_if(
            pool,
            serde_json::json!({
                "listing_id": 1, "units": "100", "proceeds": "800",
                "date": "2026-06-15", "strategy": "harvest_losses"
            }),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let r: WhatIfResponse = serde_json::from_slice(&body).unwrap();
        assert_eq!(r.hypothetical.capital_loss, Decimal::from(200));
        assert_eq!(r.years[0].year.net_capital_gain, Decimal::from(500));
        assert_eq!(r.years[1].year.capital_losses, Decimal::from(200));
        assert_eq!(r.years[1].year.net_capital_gain, Decimal::from(300));
    }
}
