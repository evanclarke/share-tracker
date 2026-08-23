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
//!  2. Total the year's capital losses: realised losses, plus the net capital
//!     loss brought forward from earlier years (losses carry forward
//!     indefinitely, per `docs/ato/cgt-using-capital-losses.md`). The chain
//!     starts from the entered opening carried-forward loss in `cgt_settings`
//!     (losses from before the first year in the system). An AMMA statement's
//!     `capital_losses_applied` is *not* a loss of the taxpayer's: trust-level
//!     losses are already netted inside the attributed gains and cannot flow
//!     to members (`docs/ato/amma-statement-guidance-notes.md`,
//!     `docs/ato/personal-investors-guide-managed-fund-distributions.md`
//!     Step 4 — only the investor's own losses enter the worksheet).
//!  3. Apply losses against gains in the taxpayer-favourable order — non-discountable
//!     gains first, then discount-eligible gains — so the 50% discount falls on the
//!     largest possible remaining gain.
//!  4. Net capital gain = remaining non-discountable gain + 50% of the remaining
//!     discount-eligible gain. Any unused loss is carried forward into the next
//!     year in the series.
//!
//! **The year record is a worksheet, so it is kept at the cent.** Its input
//! figures (the gross gains, the year's losses, the brought-forward balance)
//! are rounded to the cent — [`crate::infra::decimal::to_cents`], the one
//! rounding rule — and every dependent column is computed from those rounded
//! values, so the working the CSV export and the annual tax report print
//! reaches the figure printed beside it (SCENARIOS W-f). See [`net_years`].
//!
//! The series is one record per year with something to report: every year with
//! recorded activity, plus every quiet year up to the current one that carries
//! a capital loss forward (label 18V is reported until the loss is used, not
//! only in years with a CGT event — see [`net_years`]).

use crate::domain::cost_base::{self, ParcelRow};
use crate::domain::tax_year::tax_year_for;
use crate::entities::corporate_action::{self, RocEvent};
use crate::infra::decimal::{parse_dec, to_cents};
use crate::infra::fx::{FxOverride, FxRates};
use crate::infra::http::ApiError;
use crate::reports::export::{self, Cents};
use crate::reports::parcel_optimiser::{self, DisposalTotals, HypotheticalAllocation, Strategy};
use axum::{
    Json, Router,
    extract::State,
    response::Response,
    routing::{get, post},
};
use chrono::{Datelike, NaiveDate};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use sqlx::{FromRow, Row, SqlitePool};
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
    /// Capital losses arising this year (realised losses), as a positive
    /// amount. Excludes the brought-forward balance. An AMMA's
    /// `capital_losses_applied` is deliberately not counted: those losses were
    /// applied at the *trust* level before attribution — the statement's gains
    /// are already net of them, and a trust cannot distribute capital losses
    /// to members (`docs/ato/personal-investors-guide-managed-fund-distributions.md`).
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
    /// (= net_discount_eligible_gain / 2, to the cent).
    pub cgt_discount: Decimal,
    /// Assessable net capital gain = net_other_gain + (net_discount_eligible_gain
    /// − cgt_discount), i.e. what the worksheet leaves after the discount line
    /// printed above it — not a second halving (see [`net_years`]).
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
    /// Informational: gross CGT event C2 gains included in this year — a
    /// [return-of-capital](crate::entities::corporate_action) payment received
    /// on units entitled at the record date but *sold* before the payment
    /// date, which ends a right to receive rather than reducing any cost base
    /// (`docs/ato/return-of-capital-right-to-receive.md`). Already counted
    /// within `discount_eligible_gains` / `other_gains` above per the holding
    /// period at the payment date; surfaced separately for transparency.
    pub cgt_event_c2_gain: Decimal,
    /// Informational: the taxpayer assumption behind the hard-wired rates
    /// (always [`crate::reports::TAXPAYER_BASIS`]) — the 50% discount applied
    /// here is the Australian-resident-individual rate; other entity types are
    /// not modelled.
    pub taxpayer_basis: String,
    /// This tax year's realised disposals (ordinary Sells and rights
    /// sales/lapses), each carrying its own per-parcel breakdown — so a UI can
    /// drill from the year's totals down to the disposals and parcels behind
    /// them. Excludes the AMMA-attributed and E10/G1 gains folded into the
    /// totals above (they have no parcel-allocation record to drill into).
    /// Left empty on the what-if's scenario rows (`ScenarioYear`) — that
    /// drilldown belongs to this report, not the hypothetical dry-run. Not
    /// carried into the CSV export (`NetCapitalGainYearCsv` — nested rows
    /// don't fit a flat CSV record).
    #[serde(default)]
    pub disposals: Vec<super::realised_gains::RealisedGainLoss>,
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
    "cgt_event_c2_gain",
    "taxpayer_basis",
];

/// ATO tax-return label per `CSV_HEADER` column (same order), exported as the
/// second header row. Labels are from the **2026** individual tax return
/// (`docs/ato/tax-return-labels-2026.md` — re-verify when the form year
/// changes; the first cell names the form year). `18H (component)` = the two
/// gross-gain columns sum to label 18H; `18 (working)` = an intermediate step
/// of question 18's calculation with no label of its own; empty =
/// informational. The full mapping rationale is in `docs/API.md`.
const CSV_ATO_LABELS: &[&str] = &[
    export::ATO_LABELS_MARKER, // tax_year
    "18H (component)",         // discount_eligible_gains
    "18H (component)",         // other_gains
    "18 (working)",            // capital_losses
    "18V (prior year)",        // capital_loss_brought_forward
    "18 (working)",            // net_discount_eligible_gain
    "18 (working)",            // net_other_gain
    "18 (working)",            // cgt_discount
    "18A",                     // net_capital_gain
    "18V",                     // capital_loss_carried_forward
    "",                        // cgt_event_e10_gain (informational)
    "",                        // cgt_event_g1_gain (informational)
    "",                        // cgt_event_c2_gain (informational)
    "",                        // taxpayer_basis
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
    /// Gross CGT event C2 gains folded into the buckets above (informational).
    c2: Decimal,
    /// The AMMA discount-method distribution component of `discount_eligible`
    /// (grossed up ×2), tracked separately so the annual tax report can show
    /// it on its own ATO-worksheet line ("Discounted Capital Gain
    /// Distributions (Grossed Up)") — `discount_eligible` itself keeps
    /// merging it with realised long-term sells and any discount-eligible
    /// E10/G1 gain, since a capital loss is applied to the combined total
    /// either way.
    amma_discount_grossed_up: Decimal,
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
    Ok(fx.to_aud(value, currency, date, FxOverride::None)?)
}

/// One cost-base reduction against a parcel, **per as-acquired unit** and in
/// the parcel's native currency — the merged input to
/// [`non_disposal_gains`]'s reduction chain. Per-unit rather than
/// whole-parcel because neither kind necessarily reaches every unit of the
/// parcel: a payment reaches only the units still held for it, and an AMMA
/// statement's adjustment row reaches only the units it covers.
enum Reduction {
    /// An AMMA statement's AMIT cost-base decrease (CGT event E10), read
    /// through the same `amit_adjustment::db_cost_base_reduction_events` the
    /// cost-base pipeline reads — so this walk applies it to exactly the units
    /// the cost base it is walking down applies it to.
    Amit(cost_base::AmitReductionEvent),
    /// A return-of-capital payment (CGT event G1), the per as-acquired unit
    /// figure `RocEvent::per_unit_for` produced.
    Roc {
        date: NaiveDate,
        currency: String,
        per_unit: Decimal,
        /// The payment's record date where recorded — what decides whether
        /// units sold before the payment were nonetheless entitled to it, and
        /// so make a CGT event C2 gain instead of a G1 reduction.
        record_date: Option<NaiveDate>,
    },
}

impl Reduction {
    /// The date the reduction arises: an AMMA statement's year end, or the
    /// payment date. The chain is walked in this order.
    fn date(&self) -> NaiveDate {
        match self {
            Reduction::Amit(e) => e.tax_year_end_date,
            Reduction::Roc { date, .. } => *date,
        }
    }

    /// The reduction reaching `units` as-acquired units of a
    /// `parcel_quantity`-unit parcel disposed of on `disposed_on` (`None` =
    /// still held) — [`cost_base::AmitReductionEvent::reduction_for_units`]'s
    /// coverage rule for an AMMA statement, and "was it still held?" for a
    /// payment.
    fn reduction_for_units(
        &self,
        parcel_quantity: Decimal,
        units: Decimal,
        disposed_on: Option<NaiveDate>,
    ) -> Decimal {
        match self {
            Reduction::Amit(e) => e.reduction_for_units(parcel_quantity, units, disposed_on),
            Reduction::Roc { date, per_unit, .. } => {
                // A unit sold before the payment was not held for it — the
                // same boundary the cost-base pipeline's `up_to` bound draws
                // (a payment on the sale date itself still reaches it).
                if disposed_on.is_none_or(|d| d >= *date) {
                    *per_unit * units
                } else {
                    Decimal::ZERO
                }
            }
        }
    }

    /// Same-date tie-break: AMIT before return of capital — the arbitrary but
    /// deterministic order `domain::cost_base::adjustment_detail` already
    /// itemises same-date rows in, so the two presentations agree on which
    /// event exhausted the cost base.
    fn rank(&self) -> u8 {
        match self {
            Reduction::Amit(_) => 0,
            Reduction::Roc { .. } => 1,
        }
    }
}

/// The groups of a parcel's units that share an event history: one per sale
/// allocation (carrying that sale's date), plus whatever is still held. Every
/// reduction either reaches a whole group or none of it, so walking the
/// cost base down group by group is what lets the excess be computed against
/// the cost base those units actually carry.
///
/// Allocated quantities are in their own sale date's unit basis, so each is
/// re-based back to the parcel's as-acquired basis first. Groups with no units
/// left are dropped.
fn unit_cohorts(
    parcel: &ParcelRow,
    sales: &[(NaiveDate, Decimal)],
    splits: &[corporate_action::SplitEvent],
) -> Vec<(Option<NaiveDate>, Decimal)> {
    let mut cohorts: Vec<(Option<NaiveDate>, Decimal)> = sales
        .iter()
        .map(|&(sale_date, qty)| {
            (
                Some(sale_date),
                corporate_action::as_acquired_quantity(qty, splits, parcel.date, sale_date),
            )
        })
        .filter(|&(_, units)| units > Decimal::ZERO)
        .collect();
    let still_held = parcel.quantity - cohorts.iter().map(|&(_, u)| u).sum::<Decimal>();
    if still_held > Decimal::ZERO {
        cohorts.push((None, still_held));
    }
    cohorts
}

/// Which CGT event a gain arose under — the informational split the report
/// reports the same gain twice under (`cgt_event_e10_gain` /
/// `cgt_event_g1_gain` / `cgt_event_c2_gain`).
#[derive(PartialEq, Eq, Clone, Copy)]
enum CgtEventKind {
    E10,
    G1,
    C2,
}

/// One capital gain from a CGT event that is not a disposal of the shares
/// themselves — so it has no parcel-allocation record and the realised-gains
/// report never sees it.
struct EventGain {
    kind: CgtEventKind,
    tax_year: i32,
    /// Gross gain in AUD.
    amount: Decimal,
    discount_eligible: bool,
}

/// The capital gains a parcel's AMIT adjustments and return-of-capital
/// payments produce without any disposal of the parcel itself: CGT events
/// **E10** and **G1** (a cost-base reduction running past nil, which the cost
/// base itself floors — `docs/ato/amit-cost-base-adjustments.md`,
/// `docs/ato/cgt-non-assessable-payments.md`) and CGT event **C2** (a payment
/// received on units already sold, which reduces no cost base at all —
/// `docs/ato/return-of-capital-right-to-receive.md`).
///
/// The first two both draw down cost base, so they share the walk below; C2
/// falls out of the same per-cohort pass because the same question — was this
/// group of units still held when the payment was made? — decides both which
/// units G1 reduces and which units C2 reaches instead.
///
/// Both reduction kinds draw down **one chain per parcel**, walked in the date
/// order the reductions arise in (an AMMA statement at its `tax_year_end_date`,
/// a return of capital at its payment date), mirroring the single running
/// balance `domain::cost_base::adjusted_cost_base` nets them against. Walking
/// them separately would report each excess as `own reductions − cost base`
/// where the truth is `all reductions − cost base`, understating the gain by
/// the cost base once whenever both kinds fire on the same parcel — and by the
/// whole excess when neither kind exceeds on its own. The combination is not
/// hypothetical: a non-AMIT trust's CGT event E4 tax-deferred reduction is
/// entered as a `ReturnOfCapital` action (see `entities::income`), so a fund
/// that converts to an AMIT mid-history carries both against the same parcel.
///
/// Each excess is attributed to the event that caused it, keeping that event's
/// own conventions:
///
/// - **E10** falls in the income year the reducing AMMA statement applies to,
///   is converted to AUD at the parcel's buy-month ATO rate (matching how the
///   cost base itself is converted in the realised report), and is
///   discount-eligible when the units were held more than 12 months as at the
///   statement's `tax_year_end_date`.
/// - **G1** falls in the payment's income year, covers only the units still
///   held at the payment date (units sold earlier were not held for it; the
///   whole-parcel totals here keep any division until that final pro-rating),
///   is converted at the payment month's ATO rate (no manual fallback — a
///   non-AUD payment with no rate fails loudly), and is discount-eligible when
///   the units were held more than 12 months at the payment date. G1 can never
///   produce a capital loss.
/// - **C2** falls in the payment's income year too, covers the units entitled
///   at the payment's **record date** but sold before the payment date, is the
///   whole payment on those units (the right to receive has a nil cost base
///   wherever the share's own was fully applied on disposal, which an ordinary
///   Sell always does), is converted at the payment month's rate like G1, and
///   is discount-eligible on the **share's** holding period to the payment
///   date — the same test G1 uses, per CR 2025/59 para 18, and not the right's
///   own record-date-to-payment life. It needs a recorded `record_date`: with
///   none, entitlement falls back to the payment date and a unit sold before
///   then is simply not entitled (`RocEvent::per_unit_for`).
///
/// Once the chain reaches nil it stays there, so every later reduction is an
/// excess in its own year — until an AMIT *increase* (a negative adjustment)
/// restores it.
async fn non_disposal_gains(
    conn: &mut sqlx::SqliteConnection,
    fx: &FxRates,
) -> Result<Vec<EventGain>, sqlx::Error> {
    // Share splits/consolidations per listing: a payment after a split is per
    // *post-split* unit, and sold units are re-based back to as-acquired
    // units, both via the same helpers the cost-base pipeline uses — so this
    // walk can never disagree with the cost base it is walking down.
    let splits = corporate_action::db_share_split_events(&mut *conn).await?;

    // The parcels carrying either reduction kind, each read once through the
    // shared `ParcelRow` mapping (its trade columns repeat per reduction row),
    // so acquisition date, FX precedence and initial cost base are the
    // parcel's own rather than re-derived here. A scrip-for-scrip replacement
    // parcel's discount clock runs from its deemed (carried) acquisition date,
    // while split/payment applicability stays on the actual trade date — the
    // two the row distinguishes as `acquired()` and `date`. `order` keeps the
    // walk deterministic despite the map.
    let mut parcels: HashMap<i64, ParcelRow> = HashMap::new();
    let mut order: Vec<i64> = Vec::new();
    let mut reductions: HashMap<i64, Vec<Reduction>> = HashMap::new();

    // The AMIT reductions themselves come from the shared loader, so the
    // per-unit figure, the covered units and the split re-basing behind them
    // are the cost-base pipeline's own; this query only reads the parcels they
    // adjust.
    let amit_parcels: Vec<ParcelRow> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT {} FROM trades \
         WHERE id IN (SELECT trade_id FROM amit_adjustments) ORDER BY id",
        ParcelRow::COLUMNS
    )))
    .fetch_all(&mut *conn)
    .await?;
    let amit_events =
        crate::entities::amit_adjustment::db_cost_base_reduction_events(&mut *conn, None).await?;

    for parcel in amit_parcels {
        let Some(events) = amit_events.get(&parcel.id) else {
            continue;
        };
        reductions
            .entry(parcel.id)
            .or_default()
            .extend(events.iter().copied().map(Reduction::Amit));
        order.push(parcel.id);
        parcels.insert(parcel.id, parcel);
    }

    let roc_rows = sqlx::query(sqlx::AssertSqlSafe(format!(
        "SELECT ca.date AS action_date, ca.amount_per_unit, ca.currency AS action_currency, \
                ca.record_date, {} \
         FROM corporate_actions ca \
         JOIN trades t ON t.listing_id = ca.listing_id \
                      AND t.trade_type IN ('Buy', 'DRP') \
                      AND t.date <= ca.date \
         WHERE ca.action_type = 'ReturnOfCapital' \
         ORDER BY t.id, ca.date, ca.id",
        ParcelRow::columns_qualified("t")
    )))
    .fetch_all(&mut *conn)
    .await?;

    for row in &roc_rows {
        let parcel = parcel_entry(row, &mut parcels, &mut order)?;
        let event = RocEvent {
            date: row.try_get("action_date")?,
            amount_per_unit: parse_dec("amount_per_unit", row.try_get("amount_per_unit")?)?,
            currency: row.try_get("action_currency")?,
            record_date: row.try_get("record_date")?,
        };
        // The payment's own reduction per as-acquired unit
        // (`RocEvent::per_unit_for`, which carries the entitlement test, the
        // currency guard, and the split re-basing — a payment is per unit *at
        // the payment date*, so a split between acquisition and the payment
        // multiplies the units receiving it). The join is only the coarse
        // payment-date bound; a parcel acquired inside the record-to-payment
        // window is ex-entitlement and declined here, exactly as the cost base
        // it would otherwise have to disagree with declines it.
        let Some(per_unit) = event.per_unit_for(
            splits_of(&splits, parcel.listing_id),
            &parcel.currency,
            parcel.date,
            // A payment already inside a replacement parcel's carried cost base
            // is not a G1 reduction of it (SCENARIOS N-06) — the same call the
            // cost-base pipeline makes, so this walk cannot describe a different
            // set of payments than the cost base it compares against.
            parcel.rolled_over_on(),
            None,
        )?
        else {
            continue;
        };
        reductions
            .entry(parcel.id)
            .or_default()
            .push(Reduction::Roc {
                date: event.date,
                currency: event.currency,
                per_unit,
                record_date: event.record_date,
            });
    }

    if order.is_empty() {
        return Ok(vec![]);
    }

    // Units sold out of each parcel, with the sale date — the split into
    // [`unit_cohorts`], and so into what each group of units carried and
    // received. The same allocations read the open-parcel loader does
    // (`domain::open_parcels::db_units_sold`); this walk needs the per-sale
    // dates rather than a single remainder, so only that read is shared.
    let sold = crate::domain::open_parcels::db_units_sold(&mut *conn, None).await?;

    let mut out = Vec::new();
    for trade_id in order {
        let parcel = &parcels[&trade_id];
        let Some(events) = reductions.get_mut(&trade_id) else {
            continue;
        };
        // Stable, so same-date reductions of the same kind keep their queries'
        // own order (statement year then id; payment date then id).
        events.sort_by_key(|e| (e.date(), e.rank()));

        let trade_qty = parcel.quantity;
        if trade_qty <= Decimal::ZERO {
            continue;
        }
        let acquired = parcel.acquired();
        let per_unit_cost = parcel.parcel().initial_cost() / trade_qty;

        // One chain per group of units that share an event history, rather
        // than one whole-parcel chain: a reduction reaching only some of the
        // parcel's units runs past nil exactly when it exhausts *those* units'
        // cost base, which a pooled chain cannot see. Where every reduction
        // reaches every unit — the ordinary case — the groups' chains are
        // proportional to each other and add back up to the pooled one.
        for (disposed_on, units) in unit_cohorts(
            parcel,
            sold.get(&trade_id).map_or(&[][..], |v| v),
            splits_of(&splits, parcel.listing_id),
        ) {
            let mut remaining = per_unit_cost * units;
            for event in events.iter() {
                // A payment these units were entitled to at its record date but
                // no longer held at its payment date reduces nothing — it ends
                // a *right to receive*, CGT event C2 on the payment date, whose
                // cost base is nil because the share's own was fully applied on
                // the disposal (`docs/ato/return-of-capital-right-to-receive.md`).
                // So the whole payment is the gain, and it is discountable on
                // the share's holding period exactly as a G1 gain would be.
                if let Reduction::Roc {
                    date,
                    currency,
                    per_unit,
                    record_date: Some(record_date),
                } = event
                    && disposed_on.is_some_and(|d| d >= *record_date && d < *date)
                {
                    out.push(EventGain {
                        kind: CgtEventKind::C2,
                        tax_year: tax_year_for(*date),
                        amount: fx.to_aud(*per_unit * units, currency, *date, FxOverride::None)?,
                        discount_eligible: crate::domain::cgt_discount::discount_eligible(
                            acquired, *date,
                        ),
                    });
                }
                let amount = event.reduction_for_units(trade_qty, units, disposed_on);
                if amount <= remaining {
                    remaining -= amount;
                    continue;
                }
                let excess = amount - remaining;
                remaining = Decimal::ZERO;
                match event {
                    Reduction::Amit(e) => out.push(EventGain {
                        kind: CgtEventKind::E10,
                        tax_year: e.tax_year_end_date.year(),
                        amount: fx.to_aud(
                            excess,
                            &parcel.currency,
                            acquired,
                            parcel.fx_override(),
                        )?,
                        discount_eligible: crate::domain::cgt_discount::discount_eligible(
                            acquired,
                            e.tax_year_end_date,
                        ),
                    }),
                    Reduction::Roc { date, currency, .. } => out.push(EventGain {
                        kind: CgtEventKind::G1,
                        tax_year: tax_year_for(*date),
                        amount: fx.to_aud(excess, currency, *date, FxOverride::None)?,
                        discount_eligible: crate::domain::cgt_discount::discount_eligible(
                            acquired, *date,
                        ),
                    }),
                }
            }
        }
    }
    Ok(out)
}

/// This listing's split/consolidation events, or an empty slice.
fn splits_of(
    splits: &HashMap<i64, Vec<corporate_action::SplitEvent>>,
    listing_id: i64,
) -> &[corporate_action::SplitEvent] {
    splits.get(&listing_id).map_or(&[][..], |v| v)
}

/// The `ParcelRow` for a reduction row's trade, mapped once per parcel however
/// many reduction rows repeat its trade columns (and recording first-seen order
/// so the walk over the map stays deterministic).
fn parcel_entry<'a>(
    row: &sqlx::sqlite::SqliteRow,
    parcels: &'a mut HashMap<i64, ParcelRow>,
    order: &mut Vec<i64>,
) -> Result<&'a ParcelRow, sqlx::Error> {
    let id: i64 = row.try_get("id")?;
    if let std::collections::hash_map::Entry::Vacant(slot) = parcels.entry(id) {
        slot.insert(ParcelRow::from_row(row)?);
        order.push(id);
    }
    Ok(&parcels[&id])
}

pub async fn db_net_capital_gain(
    pool: &SqlitePool,
) -> Result<Vec<NetCapitalGainYear>, sqlx::Error> {
    // One read transaction across every input — the realised rows, the
    // AMMA/E10/G1 walks, the FX table, and the opening loss — so the whole
    // report sees a single consistent snapshot (an interleaved write can't
    // e.g. land an AMMA row between the realised read and the E10 walk).
    let mut tx = pool.begin().await?;
    let realised = super::realised_gains::db_realised_gains_on(&mut tx).await?;
    let buckets = gross_buckets(&mut tx, &realised).await?;
    let opening = crate::entities::cgt_settings::db_opening_capital_loss(&mut *tx).await?;
    tx.commit().await?;

    // Group the already-fetched disposals by tax year so each year row can
    // carry the disposals (and, within them, the parcels) that its gross
    // gains were summed from — the drilldown data `gross_buckets` only reads
    // aggregated totals from.
    let mut disposals_by_year: HashMap<i32, Vec<super::realised_gains::RealisedGainLoss>> =
        HashMap::new();
    for r in realised {
        disposals_by_year
            .entry(tax_year_for(r.sale_date))
            .or_default()
            .push(r);
    }

    let mut years = net_years(buckets, opening, current_tax_year());
    for year in &mut years {
        year.disposals = disposals_by_year.remove(&year.tax_year).unwrap_or_default();
    }
    Ok(years)
}

/// Accumulate the gross per-year buckets from every recorded source:
/// realised disposals, AMMA CGT components, and the E10/G1 excess gains.
/// Shared by the report and the what-if (which injects a hypothetical
/// disposal's buckets before netting). `realised` is fetched once by the
/// caller — both callers also need the same rows for their own purposes (the
/// report to attach each year's `disposals`, the what-if's totals having
/// already come from `parcel_optimiser` directly). Runs on the caller's read
/// transaction, the same snapshot the realised rows came from.
async fn gross_buckets(
    conn: &mut sqlx::SqliteConnection,
    realised: &[super::realised_gains::RealisedGainLoss],
) -> Result<HashMap<i32, GrossBuckets>, sqlx::Error> {
    let mut buckets: HashMap<i32, GrossBuckets> = HashMap::new();

    // Every imported ATO FX rate — the AMMA/E10/G1 conversions below are map
    // lookups, not one DB round-trip each.
    let fx = FxRates::load(&mut *conn).await?;

    // Realised parcel gains (already AUD), bucketed by the sale's tax year.
    for r in realised {
        let b = buckets.entry(tax_year_for(r.sale_date)).or_default();
        b.discount_eligible += r.discount_eligible_gain;
        b.other += r.non_discountable_gain;
        b.losses += r.capital_loss;
    }

    // AMMA-attributed CGT components, converted to AUD via the ATO rate for the
    // month of tax_year_end_date (the statement's only period anchor). The
    // statement's `capital_losses_applied` is deliberately not read here: the
    // attributed gains are already net of the losses the trust applied at its
    // own level, and trust losses cannot flow to members — counting them again
    // would double-deduct (`docs/ato/amma-statement-guidance-notes.md`;
    // `docs/ato/personal-investors-guide-managed-fund-distributions.md` Step 4
    // applies only the investor's own losses).
    let amma_rows = sqlx::query(
        "SELECT tax_year_end_date, cgt_discount_gains, cgt_indexation_gains, \
         cgt_other_gains, currency \
         FROM amma_statements",
    )
    .fetch_all(&mut *conn)
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

        let b = buckets.entry(year_end.year()).or_default();
        let grossed_up = discount_net * Decimal::from(2);
        b.discount_eligible += grossed_up;
        b.amma_discount_grossed_up += grossed_up;
        b.other += indexation + other;
    }

    // CGT event E10, G1 and C2 gains — a parcel's AMIT and return-of-capital
    // events, which produce capital gains without any disposal of the parcel
    // itself — are ordinary capital gains: they enter the buckets
    // (discount-eligible or not, per the holding period at the event date), so
    // losses can offset them and the discount applies to the eligible portion.
    // They are also reported on their own informational line per event type.
    for gain in non_disposal_gains(&mut *conn, &fx).await? {
        let b = buckets.entry(gain.tax_year).or_default();
        if gain.discount_eligible {
            b.discount_eligible += gain.amount;
        } else {
            b.other += gain.amount;
        }
        match gain.kind {
            CgtEventKind::E10 => b.e10 += gain.amount,
            CgtEventKind::G1 => b.g1 += gain.amount,
            CgtEventKind::C2 => b.c2 += gain.amount,
        }
    }

    Ok(buckets)
}

/// The financial year in progress — [`net_years`]'s `through` bound for the
/// two report paths (the multi-year report and the annual tax report's CGT
/// summary). The FY bucketing is [`tax_year_for`]'s, never re-derived.
fn current_tax_year() -> i32 {
    tax_year_for(crate::infra::date::today())
}

/// Steps 3 and 4: walk the years in order, applying losses ATO-optimally and
/// chaining unused net capital losses forward — a year's leftover loss
/// becomes the next year's brought-forward balance (losses carry forward
/// indefinitely). The chain starts from `brought_forward`, the entered
/// opening carried-forward loss (pre-system loss years) in `cgt_settings`.
///
/// **Every column here is at the cent** (SCENARIOS W-f). The record's
/// *inputs* — the two gross-gain buckets, the year's own losses, the
/// brought-forward balance (which is the previous row's carried-forward
/// output, so only the entered opening loss is rounded as an input), and the
/// three informational CGT-event lines — are taken to the cent with
/// [`crate::infra::decimal::to_cents`]. The rest are *derived* from those
/// rounded inputs by exact arithmetic that cannot leave the cent
/// (`+`, `−`, `min`), except the 50% discount, which halves and so rounds
/// once itself. The row is what the CSV export, the JSON report and the
/// annual tax report's `cgt_summary` all print, so a worksheet whose
/// columns are rounded independently would print a working that does not
/// reach its own result: `discount_eligible_gains` of 100.01 halves to
/// 50.005, and 100.01 − 50.01 is 50.00, not the 50.01 an independently
/// rounded 18A would show. Deriving instead of re-rounding is what settles
/// all three surfaces on one set of figures; the cost is that a reported
/// figure can move by up to a cent from the exact arithmetic, which is the
/// accepted price of a document that adds up.
///
/// **A quiet year that carries a loss balance still gets a row.** Label 18V
/// (*net capital losses carried forward to later income years*) is reported
/// **every** year until the loss is used, not only in years with a CGT event
/// (`docs/ato/capital-gains-question-18.md`, step 11 / Kathleen's Example 6 —
/// the unused loss is carried forward with no gain to report it against), so
/// a year with no activity of its own but a non-zero brought-forward balance
/// is emitted with zero gains, zero current-year losses, and the balance on
/// both the brought-forward and carried-forward lines. A year with neither
/// activity nor a balance is still absent: the series stays sparse, an
/// activity list plus the years that actually owe an 18V figure.
///
/// `through` is the last year such a filler row may be emitted for — the
/// financial year *in progress* at the callers (`tax_year_for(today())`),
/// since that is the last year for which a return could be being prepared
/// and there is nothing beyond it to report. It bounds only the filler: a
/// year present in `buckets` is always emitted, however it is dated. The
/// series still *starts* at the earliest year in `buckets`, or at `through`
/// itself when there are none — an opening loss entered in `cgt_settings` is
/// a pre-system balance attributed to no year, so with no recorded facts at
/// all the only year that can carry it is the current one.
fn net_years(
    mut buckets: HashMap<i32, GrossBuckets>,
    brought_forward: Decimal,
    through: i32,
) -> Vec<NetCapitalGainYear> {
    let first = buckets.keys().copied().min().unwrap_or(through);
    let mut years: Vec<i32> = buckets.keys().copied().chain(first..=through).collect();
    years.sort_unstable();
    years.dedup();

    // The chain's **seed** — the entered `cgt_settings` opening loss — is the
    // one brought-forward figure this walk does not produce itself, so it is
    // taken to the cent here, once (SCENARIOS W-f). Every later value of
    // `brought_forward` is a row's own `carried_forward`, already at the cent.
    // Rounding it here rather than per row also keeps the quiet-year test
    // below ("does this year still carry a balance?") asking about the figure
    // that would be *reported*, so a sub-cent opening loss does not emit a
    // row of zeros every year until it is used.
    let mut brought_forward = to_cents(brought_forward);
    let two = Decimal::from(2);
    years
        .into_iter()
        .filter_map(|tax_year| {
            let b = match buckets.remove(&tax_year) {
                Some(b) => b,
                // A quiet year: reported only while it carries a balance.
                None if brought_forward != Decimal::ZERO => GrossBuckets::default(),
                None => return None,
            };
            // The worksheet's **input** figures, taken to the cent here
            // (SCENARIOS W-f): every dependent column below is then computed
            // from these rounded values, so the working printed on the CSV
            // export and the annual tax report reaches the figure printed
            // beside it. (`brought_forward` is already at the cent — see the
            // seed above.)
            let discount_eligible_gains = to_cents(b.discount_eligible);
            let other_gains = to_cents(b.other);
            let capital_losses = to_cents(b.losses);

            // Apply losses (this year's + brought forward — both offset gains before
            // the discount) to non-discountable gains first, then to discount-eligible
            // gains (taxpayer-favourable: the discount falls on the largest remainder).
            // Addition, subtraction and `min` over cent figures stay at the
            // cent, so none of these four needs rounding of its own.
            let available_losses = capital_losses + brought_forward;
            let loss_to_other = other_gains.min(available_losses);
            let net_other = other_gains - loss_to_other;
            let remaining_loss = available_losses - loss_to_other;

            let loss_to_discount = discount_eligible_gains.min(remaining_loss);
            let net_discount = discount_eligible_gains - loss_to_discount;
            let carried_forward = remaining_loss - loss_to_discount;

            // Halving is the one step that can leave the cent: an odd number
            // of cents of net discount-eligible gain halves onto a half cent
            // (the mechanism behind SCENARIOS W-d and W-f). The **discount**
            // is the figure rounded — it is the worksheet's own "less CGT
            // concession amount @ 50%" line — and the assessable gain is then
            // what the worksheet says is left after it, rather than a second
            // independent halving. So `net_discount − cgt_discount` is
            // exactly the discounted part of `net_capital_gain`, and (the
            // discount rounding half away from zero) the assessable figure
            // lands the taxpayer-favourable way on a half cent.
            let cgt_discount = to_cents(net_discount / two);
            let year = NetCapitalGainYear {
                tax_year,
                discount_eligible_gains,
                other_gains,
                capital_losses,
                capital_loss_brought_forward: brought_forward,
                net_discount_eligible_gain: net_discount,
                net_other_gain: net_other,
                cgt_discount,
                net_capital_gain: net_other + (net_discount - cgt_discount),
                capital_loss_carried_forward: carried_forward,
                cgt_event_e10_gain: to_cents(b.e10),
                cgt_event_g1_gain: to_cents(b.g1),
                cgt_event_c2_gain: to_cents(b.c2),
                taxpayer_basis: crate::reports::TAXPAYER_BASIS.to_string(),
                // Attached by the caller (`db_net_capital_gain` groups the
                // already-fetched realised rows by tax year); left empty here
                // and on every what-if scenario row.
                disposals: Vec::new(),
            };
            brought_forward = carried_forward;
            Some(year)
        })
        .collect()
}

/// Extra CGT-worksheet detail for one tax year, reproducing the ATO
/// worksheet layout (`docs/ato/personal-investors-guide-managed-fund-
/// distributions.md`) the annual tax report prints. Kept off
/// [`NetCapitalGainYear`] itself — unlike that struct this is never exported
/// as CSV — but every figure here is derived from the exact same
/// `gross_buckets`/`net_years` pipeline that struct comes from, never a
/// second implementation of the netting rule. It is therefore at the cent
/// and internally consistent for the same reason (SCENARIOS W-f): this is
/// the layout that is *printed*, and its lines subtract from one another.
#[derive(Debug, Clone, Serialize)]
pub(crate) struct CgtSummaryYear {
    pub tax_year: i32,
    /// "Capital Gains on shares applicable for 'Other' method (short term
    /// gains)" — `NetCapitalGainYear::other_gains` unchanged (there's no AMMA
    /// distribution split on this side: an AMMA's indexation/other-method
    /// gains are already the full untaxed amount, not a halved distribution
    /// figure needing a separate grossed-up line).
    pub short_term_gains: Decimal,
    /// "Capital Gains on shares applicable for 'Discount' method (long term
    /// gains)" — `discount_eligible_gains` less the AMMA distribution
    /// component below (realised long-term sells plus any discount-eligible
    /// E10/G1 gain).
    pub long_term_gains: Decimal,
    /// "Discounted Capital Gain Distributions (Grossed Up)".
    pub amma_discount_gains_grossed_up: Decimal,
    /// "less Capital losses available to be offset" (Other method side).
    pub losses_applied_other: Decimal,
    pub net_other_gain: Decimal,
    /// "less Capital losses available to be offset" (Discount method side).
    pub losses_applied_discount: Decimal,
    pub net_discount_eligible_gain: Decimal,
    /// "less CGT Concession Amount @ 50%".
    pub cgt_concession_amount: Decimal,
    /// "Capital Gain" — the final assessable net capital gain.
    pub net_capital_gain: Decimal,
    pub capital_losses_this_year: Decimal,
    pub capital_loss_brought_forward: Decimal,
    pub capital_loss_carried_forward: Decimal,
    pub cgt_event_e10_gain: Decimal,
    pub cgt_event_g1_gain: Decimal,
    pub cgt_event_c2_gain: Decimal,
    pub taxpayer_basis: String,
}

/// Every tax year this report emits a row for — the year list itself, with
/// no per-year figures attached.
///
/// *The* answer to "which years have CGT content", for the annual tax
/// report's year picker (`reports::tax_report::db_tax_report_years`, SCENARIOS
/// P-02/P-03/P-04). It runs the very same `gross_buckets`/[`net_years`]
/// pipeline [`db_net_capital_gain`] and [`db_cgt_summary_year`] do, so the
/// picker cannot offer a year the CGT summary would then answer `None` for,
/// nor omit one it would answer `Some` for. Deriving the years a second time
/// from the fact tables would have to re-derive realised disposals, rights
/// sales, the E10/G1/C2 walks *and* the loss-carry-forward chain — exactly the
/// divergence this shares the walk to avoid.
///
/// Runs on the caller's connection so it joins the caller's read transaction.
pub(crate) async fn db_cgt_years(
    conn: &mut sqlx::SqliteConnection,
) -> Result<Vec<i32>, sqlx::Error> {
    let realised = super::realised_gains::db_realised_gains_on(&mut *conn).await?;
    let buckets = gross_buckets(&mut *conn, &realised).await?;
    let opening = crate::entities::cgt_settings::db_opening_capital_loss(&mut *conn).await?;
    Ok(net_years(buckets, opening, current_tax_year())
        .into_iter()
        .map(|y| y.tax_year)
        .collect())
}

/// [`CgtSummaryYear`] for one tax year — `None` when the year has neither
/// recorded gain/loss activity nor a capital loss brought forward into it
/// (matches [`NetCapitalGainYear`]'s own behaviour of only emitting a row for
/// years with something to report; an out-of-range or otherwise empty year
/// has nothing to show). A *quiet* year that carries a loss balance does
/// answer `Some`, all zeros but for the brought-forward/carried-forward pair,
/// so the archived tax document still prints that year's label 18V. Runs the whole
/// loss chain from the first recorded year — the carried-forward balance can
/// only be computed by walking every prior year in order — then picks out
/// the requested one, the same full-history computation
/// [`db_net_capital_gain`] already does.
pub(crate) async fn db_cgt_summary_year(
    conn: &mut sqlx::SqliteConnection,
    tax_year: i32,
) -> Result<Option<CgtSummaryYear>, sqlx::Error> {
    let realised = super::realised_gains::db_realised_gains_on(&mut *conn).await?;
    let buckets = gross_buckets(&mut *conn, &realised).await?;
    let amma_grossed_up: HashMap<i32, Decimal> = buckets
        .iter()
        .map(|(y, b)| (*y, b.amma_discount_grossed_up))
        .collect();
    let opening = crate::entities::cgt_settings::db_opening_capital_loss(&mut *conn).await?;
    let years = net_years(buckets, opening, current_tax_year());
    Ok(years.into_iter().find(|y| y.tax_year == tax_year).map(|y| {
        // At the cent like every other figure on this worksheet, and for the
        // same reason (SCENARIOS W-f): the printed page adds the two gain
        // lines together, so `long_term_gains` is derived by *subtracting*
        // the rounded distribution component from the rounded
        // discount-eligible total. `to_cents` is monotonic and the component
        // is part of that total, so the remainder can never go negative.
        let amma = to_cents(
            amma_grossed_up
                .get(&tax_year)
                .copied()
                .unwrap_or(Decimal::ZERO),
        );
        CgtSummaryYear {
            tax_year: y.tax_year,
            short_term_gains: y.other_gains,
            long_term_gains: y.discount_eligible_gains - amma,
            amma_discount_gains_grossed_up: amma,
            losses_applied_other: y.other_gains - y.net_other_gain,
            net_other_gain: y.net_other_gain,
            losses_applied_discount: y.discount_eligible_gains - y.net_discount_eligible_gain,
            net_discount_eligible_gain: y.net_discount_eligible_gain,
            cgt_concession_amount: y.cgt_discount,
            net_capital_gain: y.net_capital_gain,
            capital_losses_this_year: y.capital_losses,
            capital_loss_brought_forward: y.capital_loss_brought_forward,
            capital_loss_carried_forward: y.capital_loss_carried_forward,
            cgt_event_e10_gain: y.cgt_event_e10_gain,
            cgt_event_g1_gain: y.cgt_event_g1_gain,
            cgt_event_c2_gain: y.cgt_event_c2_gain,
            taxpayer_basis: y.taxpayer_basis,
        }
    }))
}

async fn net_capital_gain_handler(
    State(pool): State<SqlitePool>,
) -> Result<Json<Vec<NetCapitalGainYear>>, ApiError> {
    db_net_capital_gain(&pool)
        .await
        .map(Json)
        .map_err(ApiError::from)
}

/// Flat CSV projection of [`NetCapitalGainYear`] — every field except
/// `disposals`, with every money field typed [`Cents`] so the export reads to
/// the cent like the screen it mirrors (the JSON report above keeps the exact
/// figure). The `csv` crate rejects a struct with a nested sequence field
/// (`Vec<RealisedGainLoss>`), so the JSON report's nested drilldown is dropped
/// here; the export stays exactly the flat per-year record it was before the
/// drilldown was added — same `CSV_HEADER`/`CSV_ATO_LABELS`, unchanged.
#[derive(Serialize)]
struct NetCapitalGainYearCsv {
    tax_year: i32,
    discount_eligible_gains: Cents,
    other_gains: Cents,
    capital_losses: Cents,
    capital_loss_brought_forward: Cents,
    net_discount_eligible_gain: Cents,
    net_other_gain: Cents,
    cgt_discount: Cents,
    net_capital_gain: Cents,
    capital_loss_carried_forward: Cents,
    cgt_event_e10_gain: Cents,
    cgt_event_g1_gain: Cents,
    cgt_event_c2_gain: Cents,
    taxpayer_basis: String,
}

impl From<&NetCapitalGainYear> for NetCapitalGainYearCsv {
    fn from(y: &NetCapitalGainYear) -> Self {
        NetCapitalGainYearCsv {
            tax_year: y.tax_year,
            discount_eligible_gains: y.discount_eligible_gains.into(),
            other_gains: y.other_gains.into(),
            capital_losses: y.capital_losses.into(),
            capital_loss_brought_forward: y.capital_loss_brought_forward.into(),
            net_discount_eligible_gain: y.net_discount_eligible_gain.into(),
            net_other_gain: y.net_other_gain.into(),
            cgt_discount: y.cgt_discount.into(),
            net_capital_gain: y.net_capital_gain.into(),
            capital_loss_carried_forward: y.capital_loss_carried_forward.into(),
            cgt_event_e10_gain: y.cgt_event_e10_gain.into(),
            cgt_event_g1_gain: y.cgt_event_g1_gain.into(),
            cgt_event_c2_gain: y.cgt_event_c2_gain.into(),
            taxpayer_basis: y.taxpayer_basis.clone(),
        }
    }
}

/// The same per-year rows as the JSON report, as a downloadable tax-return-ready CSV.
async fn net_capital_gain_export_handler(
    State(pool): State<SqlitePool>,
) -> Result<Response, ApiError> {
    let rows = db_net_capital_gain(&pool).await.map_err(ApiError::from)?;
    let csv_rows: Vec<NetCapitalGainYearCsv> = rows.iter().map(Into::into).collect();
    export::csv_response(
        "net-capital-gain.csv",
        CSV_HEADER,
        CSV_ATO_LABELS,
        &csv_rows,
    )
    .map_err(ApiError::from)
}

// ---------------------------------------------------------------------------
// Pre-sale what-if
// ---------------------------------------------------------------------------

/// A hypothetical disposal: `units` of `listing_id` sold on `date` for
/// `proceeds` (total capital proceeds, AUD), drawn from open parcels via
/// either explicit `allocations` or a named optimiser `strategy` — exactly
/// one of the two.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WhatIfRequest {
    pub listing_id: i64,
    /// Restricts strategy-derived (and validates explicit) allocations to
    /// one account's parcels; absent = parcels from any account.
    #[serde(default)]
    pub holding_account_id: Option<i64>,
    #[serde(deserialize_with = "crate::infra::decimal::strict_decimal")]
    pub units: Decimal,
    /// Total capital proceeds in AUD.
    #[serde(deserialize_with = "crate::infra::decimal::strict_decimal")]
    pub proceeds: Decimal,
    pub date: NaiveDate,
    #[serde(default)]
    pub allocations: Option<Vec<WhatIfAllocation>>,
    #[serde(default)]
    pub strategy: Option<Strategy>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WhatIfAllocation {
    pub purchase_trade_id: i64,
    #[serde(deserialize_with = "crate::infra::decimal::strict_decimal")]
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

    // One read transaction across every input — the candidate parcels, the
    // realised rows, the AMMA/E10/G1 walks, and the opening loss — so the
    // dry-run works from a single consistent snapshot, exactly like the
    // report proper. Everything after the commit is pure computation.
    let mut tx = pool.begin().await.map_err(ApiError::from)?;
    // Candidates as at the hypothetical disposal's own date, not today: a
    // parcel acquired after it can't be sold on it (the Sell path refuses
    // exactly that allocation), and one sold since was still there to sell.
    // `units` and the implied per-unit price `proceeds ÷ units` are read in
    // that date's unit basis — the basis the candidates come back in.
    let parcels = parcel_optimiser::db_candidate_parcels_on(
        &mut tx,
        req.listing_id,
        req.holding_account_id,
        Some(req.date),
    )
    .await
    .map_err(ApiError::from)?;
    let realised = super::realised_gains::db_realised_gains_on(&mut tx)
        .await
        .map_err(ApiError::from)?;
    let buckets = gross_buckets(&mut tx, &realised)
        .await
        .map_err(ApiError::from)?;
    let opening = crate::entities::cgt_settings::db_opening_capital_loss(&mut *tx)
        .await
        .map_err(ApiError::from)?;
    tx.commit().await.map_err(ApiError::from)?;

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
                    let in_account = match req.holding_account_id {
                        Some(h) => format!(" in {}", super::account_label(&pool, h).await?),
                        None => String::new(),
                    };
                    return Err(ApiError::Unprocessable(format!(
                        "parcel {} is not an open parcel of {}{in_account}",
                        a.purchase_trade_id,
                        super::listing_label(&pool, req.listing_id).await?
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
                // Name the account the request scoped the candidates to —
                // otherwise the refusal reads as a claim about every unit
                // held, which is false whenever another account holds more.
                // Same wording as the allocations branch above and the
                // optimiser's own over-request refusal.
                let in_account = match req.holding_account_id {
                    Some(h) => format!(" in {}", super::account_label(&pool, h).await?),
                    None => String::new(),
                };
                return Err(ApiError::Unprocessable(format!(
                    "only {open} unit(s) of {} are open{in_account}",
                    super::listing_label(&pool, req.listing_id).await?
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
        // The disposal year bounds the walk: the what-if answers for that year
        // alone, so the series need not run past it (the quiet-year filler
        // rows before it chain the balance through unchanged, exactly as the
        // report's own walk does, and are then discarded).
        year_row(net_years(without, opening, tax_year), "without"),
        year_row(net_years(with, opening, tax_year), "with"),
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
    use crate::test_support::{self, ApiClient, allocate, test_pool};
    use axum::http::StatusCode;

    /// Client over this module's own routes.
    fn client(pool: &SqlitePool) -> ApiClient {
        ApiClient::over(router().with_state(pool.clone()))
    }

    async fn insert_listing(pool: &SqlitePool, id: i64, ticker: &str) {
        test_support::listing(id)
            .ticker(ticker)
            .name(ticker)
            .insert(pool)
            .await;
    }

    /// A USD-quoted listing: a trade and an AMMA statement are both recorded
    /// in their listing's currency (`trade::UpsertError::CurrencyNotListings`).
    async fn insert_usd_listing(pool: &SqlitePool, id: i64, ticker: &str) {
        test_support::listing(id)
            .mic("XNYS")
            .ticker(ticker)
            .name(ticker)
            .currency("USD")
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

    /// The tax years the report emitted, in order — the series a test asserts
    /// the *shape* of (an activity year, plus every quiet year carrying a loss
    /// balance through to the current financial year).
    fn tax_years(rows: &[NetCapitalGainYear]) -> Vec<i32> {
        rows.iter().map(|y| y.tax_year).collect()
    }

    /// The one row for `fy`, panicking with the whole series when it's absent.
    fn row_for(rows: &[NetCapitalGainYear], fy: i32) -> &NetCapitalGainYear {
        rows.iter()
            .find(|y| y.tax_year == fy)
            .unwrap_or_else(|| panic!("no FY{fy} row in {:?}", tax_years(rows)))
    }

    /// A quiet year's row: no activity of its own, the balance held on both
    /// the brought-forward and carried-forward lines.
    fn assert_quiet_year(row: &NetCapitalGainYear, balance: Decimal) {
        assert_eq!(row.discount_eligible_gains, Decimal::ZERO);
        assert_eq!(row.other_gains, Decimal::ZERO);
        assert_eq!(row.capital_losses, Decimal::ZERO);
        assert_eq!(row.net_discount_eligible_gain, Decimal::ZERO);
        assert_eq!(row.net_other_gain, Decimal::ZERO);
        assert_eq!(row.cgt_discount, Decimal::ZERO);
        assert_eq!(row.net_capital_gain, Decimal::ZERO);
        assert_eq!(row.capital_loss_brought_forward, balance);
        assert_eq!(row.capital_loss_carried_forward, balance);
        assert!(row.disposals.is_empty());
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
            NaiveDate::from_ymd_opt(2024, 1, 2).unwrap(),
            Decimal::from(100),
            Decimal::from(10),
        )
        .await;
        insert_trade(
            &pool,
            2,
            trade::TradeType::Sell,
            1,
            NaiveDate::from_ymd_opt(2025, 6, 2).unwrap(),
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
            NaiveDate::from_ymd_opt(2024, 1, 2).unwrap(),
            Decimal::from(100),
            Decimal::from(10),
        )
        .await;
        insert_trade(
            &pool,
            2,
            trade::TradeType::Sell,
            1,
            NaiveDate::from_ymd_opt(2025, 6, 2).unwrap(),
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
            NaiveDate::from_ymd_opt(2024, 1, 2).unwrap(),
            Decimal::from(100),
            Decimal::from(10),
        )
        .await;
        insert_trade(
            &pool,
            2,
            trade::TradeType::Sell,
            1,
            NaiveDate::from_ymd_opt(2024, 6, 3).unwrap(),
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
            NaiveDate::from_ymd_opt(2024, 1, 2).unwrap(),
            Decimal::from(100),
            Decimal::from(10),
        )
        .await;
        insert_trade(
            &pool,
            2,
            trade::TradeType::Sell,
            1,
            NaiveDate::from_ymd_opt(2025, 6, 2).unwrap(),
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
            NaiveDate::from_ymd_opt(2025, 3, 3).unwrap(),
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
            NaiveDate::from_ymd_opt(2025, 3, 3).unwrap(),
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
            NaiveDate::from_ymd_opt(2024, 1, 2).unwrap(),
            Decimal::from(100),
            Decimal::from(10),
        )
        .await;
        insert_trade(
            &pool,
            2,
            trade::TradeType::Sell,
            1,
            NaiveDate::from_ymd_opt(2025, 6, 2).unwrap(),
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
            NaiveDate::from_ymd_opt(2025, 1, 2).unwrap(),
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
        // FY2025's activity, then a filler row per quiet year still carrying
        // the 200 balance, through to the financial year in progress.
        assert_eq!(
            tax_years(&r),
            (2025..=current_tax_year()).collect::<Vec<_>>()
        );
        let y = row_for(&r, 2025);
        assert_eq!(y.capital_losses, Decimal::from(700));
        assert_eq!(y.net_discount_eligible_gain, Decimal::ZERO);
        assert_eq!(y.net_capital_gain, Decimal::ZERO);
        assert_eq!(y.capital_loss_carried_forward, Decimal::from(200));
        for quiet in r.iter().filter(|y| y.tax_year > 2025) {
            assert_quiet_year(quiet, Decimal::from(200));
        }
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
            NaiveDate::from_ymd_opt(2023, 7, 3).unwrap(),
            Decimal::from(100),
            Decimal::from(10),
        )
        .await;
        insert_trade(
            &pool,
            2,
            trade::TradeType::Sell,
            1,
            NaiveDate::from_ymd_opt(2024, 6, 3).unwrap(),
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
            NaiveDate::from_ymd_opt(2026, 3, 2).unwrap(),
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
        // FY2025 has no activity of its own, but it carries the FY2024 loss:
        // it gets its own row between the two active years, and the series
        // stops at FY2026, which uses the balance up.
        assert_eq!(tax_years(&r), vec![2024, 2025, 2026]);
        // FY2024: the loss year.
        let loss_year = row_for(&r, 2024);
        assert_eq!(loss_year.capital_losses, Decimal::from(300));
        assert_eq!(loss_year.capital_loss_brought_forward, Decimal::ZERO);
        assert_eq!(loss_year.net_capital_gain, Decimal::ZERO);
        assert_eq!(loss_year.capital_loss_carried_forward, Decimal::from(300));
        // FY2025: the quiet year, reporting the balance it holds.
        assert_quiet_year(row_for(&r, 2025), Decimal::from(300));
        // FY2026: the chained loss offsets the gain before the discount.
        let gain_year = row_for(&r, 2026);
        assert_eq!(gain_year.capital_loss_brought_forward, Decimal::from(300));
        assert_eq!(gain_year.capital_losses, Decimal::ZERO);
        assert_eq!(gain_year.other_gains, Decimal::from(500));
        assert_eq!(gain_year.net_other_gain, Decimal::from(200));
        assert_eq!(gain_year.net_capital_gain, Decimal::from(200));
        assert_eq!(gain_year.capital_loss_carried_forward, Decimal::ZERO);
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
            NaiveDate::from_ymd_opt(2023, 7, 3).unwrap(),
            Decimal::from(100),
            Decimal::from(20),
        )
        .await;
        insert_trade(
            &pool,
            2,
            trade::TradeType::Sell,
            1,
            NaiveDate::from_ymd_opt(2024, 6, 3).unwrap(),
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
            NaiveDate::from_ymd_opt(2024, 1, 2).unwrap(),
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
            Decimal::from(15),
        )
        .await;
        allocate(&pool, 2, 4, 3, Decimal::from(100)).await;

        let r = db_net_capital_gain(&pool).await.unwrap();
        assert_eq!(
            tax_years(&r),
            (2024..=current_tax_year()).collect::<Vec<_>>()
        );
        assert_eq!(
            row_for(&r, 2024).capital_loss_carried_forward,
            Decimal::from(1000)
        );
        let y = row_for(&r, 2025);
        assert_eq!(y.capital_loss_brought_forward, Decimal::from(1000));
        assert_eq!(y.discount_eligible_gains, Decimal::from(500));
        assert_eq!(y.net_discount_eligible_gain, Decimal::ZERO);
        assert_eq!(y.cgt_discount, Decimal::ZERO);
        assert_eq!(y.net_capital_gain, Decimal::ZERO);
        assert_eq!(y.capital_loss_carried_forward, Decimal::from(500));
        for quiet in r.iter().filter(|y| y.tax_year > 2025) {
            assert_quiet_year(quiet, Decimal::from(500));
        }
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
            NaiveDate::from_ymd_opt(2024, 1, 2).unwrap(),
            Decimal::from(100),
            Decimal::from(10),
        )
        .await;
        insert_trade(
            &pool,
            2,
            trade::TradeType::Sell,
            1,
            NaiveDate::from_ymd_opt(2025, 6, 2).unwrap(),
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
            NaiveDate::from_ymd_opt(2023, 7, 3).unwrap(),
            Decimal::from(100),
            Decimal::from(10),
        )
        .await;
        insert_trade(
            &pool,
            2,
            trade::TradeType::Sell,
            1,
            NaiveDate::from_ymd_opt(2024, 6, 3).unwrap(),
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
            NaiveDate::from_ymd_opt(2025, 3, 3).unwrap(),
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
    async fn db_amma_indexation_and_other_gains_are_non_discountable() {
        let pool = test_pool().await;
        // Indexation 30 + other 20 = 50 non-discountable, taxed in full.
        insert_listing(&pool, 1, "VAF").await;
        let mut a = make_amma(1, 1, NaiveDate::from_ymd_opt(2024, 6, 30).unwrap());
        a.cgt_indexation_gains = Decimal::from(30);
        a.cgt_other_gains = Decimal::from(20);
        amma::db_upsert(&pool, &a).await.unwrap();

        let r = db_net_capital_gain(&pool).await.unwrap();
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].other_gains, Decimal::from(50));
        assert_eq!(r[0].net_other_gain, Decimal::from(50));
        assert_eq!(r[0].net_capital_gain, Decimal::from(50));
    }

    #[tokio::test]
    async fn db_amma_trust_level_losses_applied_never_enter_the_loss_pool() {
        let pool = test_pool().await;
        // The AMMA's capital_losses_applied is the trust's own netting,
        // disclosed for transparency — the attributed gains are already net of
        // it and a trust cannot distribute losses to members
        // (docs/ato/personal-investors-guide-managed-fund-distributions.md,
        // Step 4: only the investor's own losses are applied). It must offset
        // nothing: not the statement's own gains, not unrelated realised
        // gains, and it must not carry forward.
        insert_listing(&pool, 1, "VAF").await;
        let mut a = make_amma(1, 1, NaiveDate::from_ymd_opt(2024, 6, 30).unwrap());
        a.cgt_indexation_gains = Decimal::from(30);
        a.cgt_other_gains = Decimal::from(20);
        a.capital_losses_applied = Decimal::from(1000);
        amma::db_upsert(&pool, &a).await.unwrap();
        // An unrelated realised gain in the same year: buy 10 → sell 15.
        insert_trade(
            &pool,
            1,
            trade::TradeType::Buy,
            1,
            NaiveDate::from_ymd_opt(2024, 1, 2).unwrap(),
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

        let r = db_net_capital_gain(&pool).await.unwrap();
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].capital_losses, Decimal::ZERO);
        assert_eq!(r[0].other_gains, Decimal::from(550)); // 500 realised + 50 AMMA
        assert_eq!(r[0].net_other_gain, Decimal::from(550));
        assert_eq!(r[0].net_capital_gain, Decimal::from(550));
        assert_eq!(r[0].capital_loss_carried_forward, Decimal::ZERO);
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
            NaiveDate::from_ymd_opt(2023, 1, 3).unwrap(),
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

        // The year's `disposals` drilldown carries the one realised Sell (and
        // its parcel breakdown) that fed the discount-eligible bucket — the
        // AMMA-attributed $200 gross gain has no parcel-allocation record, so
        // it stays folded into the year's totals only.
        assert_eq!(r[0].disposals.len(), 1);
        assert_eq!(r[0].disposals[0].sale_trade_id, 2);
        assert_eq!(r[0].disposals[0].capital_gain_loss, Decimal::from(500));
        assert_eq!(r[0].disposals[0].parcels.len(), 1);
        assert_eq!(r[0].disposals[0].parcels[0].purchase_trade_id, 1);
        assert_eq!(
            r[0].disposals[0].parcels[0].capital_gain_loss,
            Decimal::from(500)
        );
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
            NaiveDate::from_ymd_opt(2023, 1, 17).unwrap(),
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
        insert_usd_listing(&pool, 1, "VAF").await;
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
        insert_usd_listing(&pool, 1, "VAF").await;
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

    /// SCENARIOS L-13. A crypto asset settles the day it is contracted (no
    /// T+n, no holiday calendar — `entities::trade`'s
    /// `api_settlement_date_same_day_for_crypto` pins the auto-population), so
    /// the financial year a disposal falls in rests on the contract date alone
    /// (`domain::tax_year`): bought 30 June and sold 1 July, the gain is the
    /// *later* year's and the parcel was held a single day.
    #[tokio::test]
    async fn db_crypto_bought_30_june_and_sold_1_july_is_the_later_year() {
        let pool = test_pool().await;
        test_support::listing(1)
            .crypto()
            .ticker("BTC")
            .name("Bitcoin")
            .insert(&pool)
            .await;
        let buy_date = NaiveDate::from_ymd_opt(2025, 6, 30).unwrap();
        let sell_date = NaiveDate::from_ymd_opt(2025, 7, 1).unwrap();
        test_support::buy(1, 1)
            .date(buy_date)
            .settlement(buy_date)
            .qty(Decimal::ONE)
            .price(Decimal::from(50000))
            .insert(&pool)
            .await;
        test_support::sell(2, 1)
            .date(sell_date)
            .settlement(sell_date)
            .qty(Decimal::ONE)
            .price(Decimal::from(60000))
            .insert(&pool)
            .await;
        allocate(&pool, 1, 2, 1, Decimal::ONE).await;

        let r = db_net_capital_gain(&pool).await.unwrap();
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].tax_year, 2026, "1 July is the next financial year");
        assert_eq!(r[0].other_gains, Decimal::from(10000));
        assert_eq!(r[0].discount_eligible_gains, Decimal::ZERO, "held one day");
        assert_eq!(r[0].net_capital_gain, Decimal::from(10000));
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
            NaiveDate::from_ymd_opt(2024, 1, 2).unwrap(),
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

    /// SCENARIOS D-13. The E10 walk runs down the cost base of the units a
    /// reduction actually reaches, not a pooled whole-parcel balance — because
    /// that is what the cost base it is walking down does. An adjustment row
    /// covering only the units still held exhausts *their* cost base while the
    /// parcel as a whole still has some, and the overrun is an E10 gain the
    /// pooled walk could not see.
    #[tokio::test]
    async fn db_e10_excess_is_measured_against_the_units_the_row_covers() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "VDHG").await;
        // 100 units at $10 = $1,000 ($10/unit), bought well over a year out.
        insert_trade(
            &pool,
            1,
            trade::TradeType::Buy,
            1,
            NaiveDate::from_ymd_opt(2022, 1, 10).unwrap(),
            Decimal::from(100),
            Decimal::from(10),
        )
        .await;
        insert_trade(
            &pool,
            2,
            trade::TradeType::Sell,
            1,
            NaiveDate::from_ymd_opt(2024, 3, 1).unwrap(),
            Decimal::from(40),
            Decimal::from(10),
        )
        .await;
        test_support::allocate(&pool, 1, 2, 1, Decimal::from(40)).await;
        // $12/unit over the 60 units still held at the year end: $720 against
        // the $600 those units carry — $120 past nil — while the parcel's
        // whole $1,000 would have absorbed it without a murmur.
        let mut a = make_amma(1, 1, NaiveDate::from_ymd_opt(2024, 6, 30).unwrap());
        a.cost_base_adjustment = Decimal::from(12);
        a.units_held = Decimal::from(60);
        amma::db_upsert(&pool, &a).await.unwrap();
        link_adjustment(&pool, 1, 1, 1, Decimal::from(60)).await;

        let r = db_net_capital_gain(&pool).await.unwrap();
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].tax_year, 2024);
        assert_eq!(r[0].cgt_event_e10_gain, Decimal::from(120));

        // And the cost base the walk is describing agrees: the covered units
        // are floored at nil, the sold ones untouched.
        let open = crate::reports::open_parcels::db_open_parcels(&pool)
            .await
            .unwrap();
        assert_eq!(open[0].remaining_cost_base, Decimal::ZERO);
        let realised = crate::reports::realised_gains::db_realised_gains(&pool)
            .await
            .unwrap();
        assert_eq!(realised[0].cost_base, Decimal::from(400));
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
            NaiveDate::from_ymd_opt(2023, 1, 3).unwrap(),
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
            NaiveDate::from_ymd_opt(2024, 1, 2).unwrap(),
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

    /// SCENARIOS B-24: the E10 walk reduces by the *year-end* unit basis, so a
    /// split between acquisition and the statement's year end doubles the
    /// reduction the fund's per-unit figure represents — and with it the
    /// excess over the cost base. Reduction and cost base must be walked on
    /// the same basis or the gain silently disappears.
    #[tokio::test]
    async fn db_e10_reduction_is_re_based_across_a_split() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "VAF").await;
        // Buy 100 @ $10 on 1 Aug 2023 → cost base $1,000; 2-for-1 split in
        // January, so the parcel is 200 units at the FY2024 year end.
        insert_trade(
            &pool,
            1,
            trade::TradeType::Buy,
            1,
            NaiveDate::from_ymd_opt(2023, 8, 1).unwrap(),
            Decimal::from(100),
            Decimal::from(10),
        )
        .await;
        apply_split(
            &pool,
            1,
            1,
            NaiveDate::from_ymd_opt(2024, 1, 15).unwrap(),
            "2",
            "1",
        )
        .await;
        // $6.00 per post-split unit × 200 = $1,200 against the $1,000 cost
        // base → nil cost base and a $200 E10 gain in FY2024.
        let mut a = make_amma(1, 1, NaiveDate::from_ymd_opt(2024, 6, 30).unwrap());
        a.cost_base_adjustment = "6.00".parse().unwrap();
        amma::db_upsert(&pool, &a).await.unwrap();
        link_adjustment(&pool, 1, 1, 1, Decimal::from(100)).await;

        let r = db_net_capital_gain(&pool).await.unwrap();
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].tax_year, 2024);
        assert_eq!(r[0].cgt_event_e10_gain, Decimal::from(200));
        // Held ≤ 12 months at the year end → non-discountable.
        assert_eq!(r[0].other_gains, Decimal::from(200));
        assert_eq!(r[0].net_capital_gain, Decimal::from(200));

        // And the cost base the open-parcels view reports is floored at nil,
        // not the $400 a naive `quantity × per-unit` multiplication leaves.
        let parcels = crate::reports::open_parcels::db_open_parcels(&pool)
            .await
            .unwrap();
        assert_eq!(parcels.len(), 1);
        assert_eq!(parcels[0].remaining_cost_base, Decimal::ZERO);
    }

    async fn apply_roc(pool: &SqlitePool, id: i64, listing_id: i64, date: NaiveDate, amount: &str) {
        apply_roc_with_record(pool, id, listing_id, date, amount, None).await;
    }

    /// [`apply_roc`] with the record date that fixed entitlement to the
    /// payment (`None` = not recorded, so the payment date decides).
    async fn apply_roc_with_record(
        pool: &SqlitePool,
        id: i64,
        listing_id: i64,
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
                    currency: "AUD".to_string(),
                    record_date,
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
            NaiveDate::from_ymd_opt(2024, 1, 2).unwrap(),
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
            NaiveDate::from_ymd_opt(2024, 1, 2).unwrap(),
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

    /// The G1 walk tests entitlement the way the cost base does (SCENARIOS
    /// B-09): a parcel bought after the record date received no payment, so it
    /// produces no excess gain — only the parcel that was actually paid does.
    #[tokio::test]
    async fn db_g1_skips_a_parcel_bought_after_the_record_date() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "RAP").await;
        // Two identical $100 parcels; only the first is held at the record date.
        for (id, date) in [
            (1, NaiveDate::from_ymd_opt(2025, 2, 3).unwrap()),
            (2, NaiveDate::from_ymd_opt(2025, 2, 18).unwrap()),
        ] {
            insert_trade(
                &pool,
                id,
                trade::TradeType::Buy,
                1,
                date,
                Decimal::from(100),
                Decimal::from(1),
            )
            .await;
        }
        // $1.50/unit → $50 excess over each entitled parcel's $100 cost base.
        apply_roc_with_record(
            &pool,
            1,
            1,
            NaiveDate::from_ymd_opt(2025, 3, 3).unwrap(),
            "1.50",
            Some(NaiveDate::from_ymd_opt(2025, 2, 10).unwrap()),
        )
        .await;

        let r = db_net_capital_gain(&pool).await.unwrap();
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].tax_year, 2025);
        assert_eq!(
            r[0].cgt_event_g1_gain,
            Decimal::from(50),
            "one parcel's excess, not both"
        );

        // Without the record date the payment date decides, and both parcels
        // are paid: the excess doubles.
        apply_roc_with_record(
            &pool,
            1,
            1,
            NaiveDate::from_ymd_opt(2025, 3, 3).unwrap(),
            "1.50",
            None,
        )
        .await;
        let r = db_net_capital_gain(&pool).await.unwrap();
        assert_eq!(r[0].cgt_event_g1_gain, Decimal::from(100));
    }

    /// SCENARIOS D-14. The mirror of the entitlement test above, at the other
    /// end of the window: units owned at the record date but **sold before the
    /// payment date** are still paid. No cost base is reduced (the units were
    /// gone, so CGT event G1 cannot apply, and the sale's own figures are
    /// untouched) — instead CGT event C2 ends the right to receive the payment
    /// on the payment date, for the whole payment: the right's cost base is nil
    /// because the share's own was fully applied on the disposal
    /// (`docs/ato/return-of-capital-right-to-receive.md`, CR 2025/59 paras
    /// 14–17). Before this the money was simply nowhere.
    #[tokio::test]
    async fn db_a_payment_on_units_sold_after_the_record_date_is_a_c2_gain() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "RAP").await;
        insert_trade(
            &pool,
            1,
            trade::TradeType::Buy,
            1,
            NaiveDate::from_ymd_opt(2023, 1, 10).unwrap(),
            Decimal::from(100),
            Decimal::from(10),
        )
        .await;
        // Sold 3 October, after the 25 September record date and before the
        // 1 November payment.
        insert_trade(
            &pool,
            2,
            trade::TradeType::Sell,
            1,
            NaiveDate::from_ymd_opt(2023, 10, 3).unwrap(),
            Decimal::from(100),
            Decimal::from(15),
        )
        .await;
        test_support::allocate(&pool, 1, 2, 1, Decimal::from(100)).await;
        apply_roc_with_record(
            &pool,
            1,
            1,
            NaiveDate::from_ymd_opt(2023, 11, 1).unwrap(),
            "0.50",
            Some(NaiveDate::from_ymd_opt(2023, 9, 25).unwrap()),
        )
        .await;

        let r = db_net_capital_gain(&pool).await.unwrap();
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].tax_year, 2024);
        // The whole $50 payment, in the payment's year, and no G1 (the units
        // were not held when it was made).
        assert_eq!(r[0].cgt_event_c2_gain, Decimal::from(50));
        assert_eq!(r[0].cgt_event_g1_gain, Decimal::ZERO);
        // The share was held under 12 months, so neither the sale's $500 nor
        // the C2 $50 is discountable: $550 assessable in full.
        assert_eq!(r[0].other_gains, Decimal::from(550));
        assert_eq!(r[0].discount_eligible_gains, Decimal::ZERO);
        assert_eq!(r[0].net_capital_gain, Decimal::from(550));

        // The sale itself is untouched: no cost-base reduction reached it.
        let realised = crate::reports::realised_gains::db_realised_gains(&pool)
            .await
            .unwrap();
        assert_eq!(realised[0].cost_base, Decimal::from(1000));
        assert_eq!(realised[0].capital_gain_loss, Decimal::from(500));
    }

    /// The C2 discount test is measured on the **share**, from its acquisition
    /// to the payment date — not on the right to receive, which only exists
    /// from the record date and so would never qualify. CR 2025/59 para 18
    /// puts G1 and C2 under the same test, and this is the same fixture as the
    /// test above with the parcel bought a year earlier.
    #[tokio::test]
    async fn db_c2_gain_is_discountable_on_the_shares_own_holding_period() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "RAP").await;
        insert_trade(
            &pool,
            1,
            trade::TradeType::Buy,
            1,
            NaiveDate::from_ymd_opt(2022, 1, 10).unwrap(),
            Decimal::from(100),
            Decimal::from(10),
        )
        .await;
        insert_trade(
            &pool,
            2,
            trade::TradeType::Sell,
            1,
            NaiveDate::from_ymd_opt(2023, 10, 3).unwrap(),
            Decimal::from(100),
            Decimal::from(15),
        )
        .await;
        test_support::allocate(&pool, 1, 2, 1, Decimal::from(100)).await;
        apply_roc_with_record(
            &pool,
            1,
            1,
            NaiveDate::from_ymd_opt(2023, 11, 1).unwrap(),
            "0.50",
            Some(NaiveDate::from_ymd_opt(2023, 9, 25).unwrap()),
        )
        .await;

        let r = db_net_capital_gain(&pool).await.unwrap();
        assert_eq!(r[0].cgt_event_c2_gain, Decimal::from(50));
        // The $50 joins the sale's discount-eligible $500 — $550 gross, $275
        // after the 50% discount. Measured from the record date instead it
        // would have been 37 days and fully assessable.
        assert_eq!(r[0].discount_eligible_gains, Decimal::from(550));
        assert_eq!(r[0].other_gains, Decimal::ZERO);
        assert_eq!(r[0].net_capital_gain, Decimal::from(275));
    }

    /// The C2 gain reaches only the units that were both entitled and sold
    /// inside the window. Units sold *before* the record date were never
    /// entitled; units still held at the payment date take the ordinary G1
    /// cost-base reduction instead — so a parcel split across all three
    /// outcomes is paid on exactly the units the record date names, once.
    #[tokio::test]
    async fn db_c2_covers_only_the_units_entitled_and_sold_before_payment() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "RAP").await;
        insert_trade(
            &pool,
            1,
            trade::TradeType::Buy,
            1,
            NaiveDate::from_ymd_opt(2023, 1, 10).unwrap(),
            Decimal::from(100),
            Decimal::from(10),
        )
        .await;
        // 30 sold before the record date (never entitled), 20 sold inside the
        // window (C2), 50 still held at the payment date (G1 reduction).
        insert_trade(
            &pool,
            2,
            trade::TradeType::Sell,
            1,
            NaiveDate::from_ymd_opt(2023, 9, 1).unwrap(),
            Decimal::from(30),
            Decimal::from(15),
        )
        .await;
        test_support::allocate(&pool, 1, 2, 1, Decimal::from(30)).await;
        insert_trade(
            &pool,
            3,
            trade::TradeType::Sell,
            1,
            NaiveDate::from_ymd_opt(2023, 10, 3).unwrap(),
            Decimal::from(20),
            Decimal::from(15),
        )
        .await;
        test_support::allocate(&pool, 2, 3, 1, Decimal::from(20)).await;
        apply_roc_with_record(
            &pool,
            1,
            1,
            NaiveDate::from_ymd_opt(2023, 11, 1).unwrap(),
            "0.50",
            Some(NaiveDate::from_ymd_opt(2023, 9, 25).unwrap()),
        )
        .await;

        let r = db_net_capital_gain(&pool).await.unwrap();
        // 20 units × 50c = $10 of C2 gain, and nothing for the 30 sold early.
        assert_eq!(r[0].cgt_event_c2_gain, Decimal::from(10));

        // The 50 units still held took the reduction instead: 500 − 25.
        let open = crate::reports::open_parcels::db_open_parcels(&pool)
            .await
            .unwrap();
        assert_eq!(open[0].return_of_capital_reduction, Decimal::from(25));
        assert_eq!(open[0].remaining_cost_base, Decimal::from(475));
    }

    /// Without a recorded record date entitlement falls back to the payment
    /// date, and a unit sold before then is simply not entitled — so there is
    /// no C2 gain to report. The remedy is to record the date, not to guess.
    #[tokio::test]
    async fn db_no_record_date_means_no_c2_gain() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "RAP").await;
        insert_trade(
            &pool,
            1,
            trade::TradeType::Buy,
            1,
            NaiveDate::from_ymd_opt(2023, 1, 10).unwrap(),
            Decimal::from(100),
            Decimal::from(10),
        )
        .await;
        insert_trade(
            &pool,
            2,
            trade::TradeType::Sell,
            1,
            NaiveDate::from_ymd_opt(2023, 10, 3).unwrap(),
            Decimal::from(100),
            Decimal::from(15),
        )
        .await;
        test_support::allocate(&pool, 1, 2, 1, Decimal::from(100)).await;
        apply_roc(
            &pool,
            1,
            1,
            NaiveDate::from_ymd_opt(2023, 11, 1).unwrap(),
            "0.50",
        )
        .await;

        let r = db_net_capital_gain(&pool).await.unwrap();
        assert_eq!(r[0].cgt_event_c2_gain, Decimal::ZERO);
        assert_eq!(r[0].cgt_event_g1_gain, Decimal::ZERO);

        // Adding the record date surfaces the payment.
        apply_roc_with_record(
            &pool,
            1,
            1,
            NaiveDate::from_ymd_opt(2023, 11, 1).unwrap(),
            "0.50",
            Some(NaiveDate::from_ymd_opt(2023, 9, 25).unwrap()),
        )
        .await;
        let r = db_net_capital_gain(&pool).await.unwrap();
        assert_eq!(r[0].cgt_event_c2_gain, Decimal::from(50));
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
            NaiveDate::from_ymd_opt(2024, 1, 2).unwrap(),
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
            NaiveDate::from_ymd_opt(2023, 1, 3).unwrap(),
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
            NaiveDate::from_ymd_opt(2024, 1, 2).unwrap(),
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
            NaiveDate::from_ymd_opt(2025, 6, 2).unwrap(),
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
            NaiveDate::from_ymd_opt(2024, 1, 2).unwrap(),
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

    /// A parcel carrying **both** reduction kinds draws them down one combined
    /// chain (SCENARIOS B-07, B-08). Neither the $600 return of capital nor the
    /// $600 AMIT decrease exceeds the $1,000 cost base on its own, so two
    /// independent walks each report nil and the $200 overrun is never
    /// reported at all.
    #[tokio::test]
    async fn db_amit_and_roc_on_one_parcel_share_one_reduction_chain() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "VDHG").await;
        // Buy 100 @ $10 (Jul 2024) → cost base $1,000.
        insert_trade(
            &pool,
            1,
            trade::TradeType::Buy,
            1,
            NaiveDate::from_ymd_opt(2024, 7, 1).unwrap(),
            Decimal::from(100),
            Decimal::from(10),
        )
        .await;
        // Return of capital $6/unit paid Sep 2024 → $600, leaving $400.
        apply_roc(
            &pool,
            1,
            1,
            NaiveDate::from_ymd_opt(2024, 9, 1).unwrap(),
            "6",
        )
        .await;
        // The FY2025 AMMA statement then reduces $6/unit × 100 = $600 against
        // that $400 remaining.
        let mut a = make_amma(1, 1, NaiveDate::from_ymd_opt(2025, 6, 30).unwrap());
        a.cost_base_adjustment = "6.00".parse().unwrap();
        amma::db_upsert(&pool, &a).await.unwrap();
        link_adjustment(&pool, 1, 1, 1, Decimal::from(100)).await;

        let r = db_net_capital_gain(&pool).await.unwrap();
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].tax_year, 2025);
        // $200 excess, attributed to the AMMA statement — the later of the two
        // reductions in the chain, and the one that drives it past nil.
        assert_eq!(r[0].cgt_event_e10_gain, Decimal::from(200));
        assert_eq!(r[0].cgt_event_g1_gain, Decimal::ZERO);
        // Held ≤ 12 months at the year end → non-discountable, fully assessable.
        assert_eq!(r[0].other_gains, Decimal::from(200));
        assert_eq!(r[0].discount_eligible_gains, Decimal::ZERO);
        assert_eq!(r[0].net_capital_gain, Decimal::from(200));

        // The cost base itself was always right: both reductions reported in
        // full, the parcel floored at nil.
        let parcels = crate::reports::open_parcels::db_open_parcels(&pool)
            .await
            .unwrap();
        assert_eq!(parcels.len(), 1);
        assert_eq!(parcels[0].amit_cost_base_reduction, Decimal::from(600));
        assert_eq!(parcels[0].return_of_capital_reduction, Decimal::from(600));
        assert_eq!(parcels[0].remaining_cost_base, Decimal::ZERO);
    }

    /// The chain sorts by event date, so it cannot depend on the order the two
    /// reductions were *recorded* in: entering the AMMA statement before the
    /// payment gives the figures the test above gets entering them the other
    /// way round.
    #[tokio::test]
    async fn db_combined_chain_is_independent_of_the_order_the_rows_were_entered() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "VDHG").await;
        insert_trade(
            &pool,
            1,
            trade::TradeType::Buy,
            1,
            NaiveDate::from_ymd_opt(2024, 7, 1).unwrap(),
            Decimal::from(100),
            Decimal::from(10),
        )
        .await;
        // AMMA statement entered first this time, the payment after it.
        let mut a = make_amma(1, 1, NaiveDate::from_ymd_opt(2025, 6, 30).unwrap());
        a.cost_base_adjustment = "6.00".parse().unwrap();
        amma::db_upsert(&pool, &a).await.unwrap();
        link_adjustment(&pool, 1, 1, 1, Decimal::from(100)).await;
        apply_roc(
            &pool,
            1,
            1,
            NaiveDate::from_ymd_opt(2024, 9, 1).unwrap(),
            "6",
        )
        .await;

        let r = db_net_capital_gain(&pool).await.unwrap();
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].tax_year, 2025);
        assert_eq!(r[0].cgt_event_e10_gain, Decimal::from(200));
        assert_eq!(r[0].cgt_event_g1_gain, Decimal::ZERO);
        assert_eq!(r[0].net_capital_gain, Decimal::from(200));
    }

    /// The combined chain's excess is reported once, in its own year — and the
    /// later sale of the now nil-cost-base parcel neither recovers it nor
    /// double-counts it. Gross gains across the two years total $1,700: the
    /// $200 overrun in FY2025 plus the $1,500 sale in FY2026.
    #[tokio::test]
    async fn db_combined_chain_excess_is_reported_once_across_the_years() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "VDHG").await;
        insert_trade(
            &pool,
            1,
            trade::TradeType::Buy,
            1,
            NaiveDate::from_ymd_opt(2024, 7, 1).unwrap(),
            Decimal::from(100),
            Decimal::from(10),
        )
        .await;
        apply_roc(
            &pool,
            1,
            1,
            NaiveDate::from_ymd_opt(2024, 9, 1).unwrap(),
            "6",
        )
        .await;
        let mut a = make_amma(1, 1, NaiveDate::from_ymd_opt(2025, 6, 30).unwrap());
        a.cost_base_adjustment = "6.00".parse().unwrap();
        amma::db_upsert(&pool, &a).await.unwrap();
        link_adjustment(&pool, 1, 1, 1, Decimal::from(100)).await;
        // Sold in FY2026 at $15 against the nil cost base.
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

        let r = db_net_capital_gain(&pool).await.unwrap();
        assert_eq!(r.len(), 2);
        assert_eq!(r[0].tax_year, 2025);
        assert_eq!(r[0].other_gains, Decimal::from(200));
        assert_eq!(r[1].tax_year, 2026);
        // Held > 12 months → discount-eligible; $1,500 gross, $750 assessable.
        assert_eq!(r[1].discount_eligible_gains, Decimal::from(1500));
        assert_eq!(r[1].net_capital_gain, Decimal::from(750));
        let gross: Decimal = r
            .iter()
            .map(|y| y.discount_eligible_gains + y.other_gains)
            .sum();
        assert_eq!(gross, Decimal::from(1700));
    }

    /// Attribution follows the event that drove the chain past nil: an AMMA
    /// statement that stays within the cost base, then a payment that overruns
    /// what is left, makes the excess a **G1** gain in the payment's year — the
    /// mirror of the E10 attribution above.
    #[tokio::test]
    async fn db_combined_chain_attributes_the_excess_to_the_event_that_caused_it() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "VDHG").await;
        // Buy 100 @ $10 (Jan 2023) → cost base $1,000.
        insert_trade(
            &pool,
            1,
            trade::TradeType::Buy,
            1,
            NaiveDate::from_ymd_opt(2023, 1, 3).unwrap(),
            Decimal::from(100),
            Decimal::from(10),
        )
        .await;
        // FY2023 AMMA: $7/unit × 100 = $700, leaving $300 — no excess yet.
        let mut a = make_amma(1, 1, NaiveDate::from_ymd_opt(2023, 6, 30).unwrap());
        a.cost_base_adjustment = "7.00".parse().unwrap();
        amma::db_upsert(&pool, &a).await.unwrap();
        link_adjustment(&pool, 1, 1, 1, Decimal::from(100)).await;
        // Then $5/unit × 100 = $500 against that $300 → $200 excess (G1, FY2024).
        apply_roc(
            &pool,
            1,
            1,
            NaiveDate::from_ymd_opt(2024, 3, 1).unwrap(),
            "5",
        )
        .await;

        let r = db_net_capital_gain(&pool).await.unwrap();
        assert_eq!(r.len(), 2);
        assert_eq!(r[0].tax_year, 2023);
        assert_eq!(r[0].cgt_event_e10_gain, Decimal::ZERO);
        assert_eq!(r[0].net_capital_gain, Decimal::ZERO);
        assert_eq!(r[1].tax_year, 2024);
        assert_eq!(r[1].cgt_event_g1_gain, Decimal::from(200));
        assert_eq!(r[1].cgt_event_e10_gain, Decimal::ZERO);
        // Held > 12 months at the payment → discount-eligible: $200/2 = $100.
        assert_eq!(r[1].discount_eligible_gains, Decimal::from(200));
        assert_eq!(r[1].net_capital_gain, Decimal::from(100));
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
            NaiveDate::from_ymd_opt(2024, 1, 2).unwrap(),
            Decimal::from(100),
            Decimal::from(10),
        )
        .await;
        insert_trade(
            &pool,
            2,
            trade::TradeType::Sell,
            1,
            NaiveDate::from_ymd_opt(2025, 6, 2).unwrap(),
            Decimal::from(100),
            Decimal::from(15),
        )
        .await;
        allocate(&pool, 1, 2, 1, Decimal::from(100)).await;

        let resp = client(&pool).get("/portfolio/net-capital-gain").await;
        assert_eq!(resp.status, StatusCode::OK);
        let result: Vec<NetCapitalGainYear> = resp.json();
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
            NaiveDate::from_ymd_opt(2024, 1, 2).unwrap(),
            Decimal::from(100),
            Decimal::from(10),
        )
        .await;
        insert_trade(
            &pool,
            2,
            trade::TradeType::Sell,
            1,
            NaiveDate::from_ymd_opt(2025, 6, 2).unwrap(),
            Decimal::from(100),
            Decimal::from(15),
        )
        .await;
        allocate(&pool, 1, 2, 1, Decimal::from(100)).await;

        let resp = client(&pool)
            .get("/portfolio/net-capital-gain/export")
            .await;
        assert_eq!(resp.status, StatusCode::OK);
        assert_eq!(
            resp.headers.get(axum::http::header::CONTENT_TYPE).unwrap(),
            "text/csv; charset=utf-8"
        );
        assert_eq!(
            resp.headers
                .get(axum::http::header::CONTENT_DISPOSITION)
                .unwrap(),
            "attachment; filename=\"net-capital-gain.csv\""
        );
        let csv = resp.text().to_string();
        let mut lines = csv.lines();
        // Header names every NetCapitalGainYear field, in declaration order.
        assert_eq!(lines.next().unwrap(), CSV_HEADER.join(","));
        // Second header row: the ATO tax-return label per column, first cell
        // naming the form year the mapping targets.
        let labels = lines.next().unwrap();
        assert_eq!(labels, CSV_ATO_LABELS.join(","));
        assert!(labels.starts_with(&format!("{},", export::ATO_LABELS_MARKER)));
        let fields: Vec<&str> = lines.next().unwrap().split(',').collect();
        assert_eq!(fields.len(), CSV_HEADER.len());
        assert_eq!(fields[0], "2025"); // tax_year
        assert_eq!(fields[1].parse::<Decimal>().unwrap(), Decimal::from(500)); // discount_eligible_gains
        assert_eq!(fields[7].parse::<Decimal>().unwrap(), Decimal::from(250)); // cgt_discount
        assert_eq!(fields[8].parse::<Decimal>().unwrap(), Decimal::from(250)); // net_capital_gain
        assert_eq!(lines.next(), None);
    }

    /// SCENARIOS W-c: brokerage and GST pro-rated across part of a parcel give
    /// a non-terminating quotient, and the CSV — a tax-return-ready document
    /// whose columns carry ATO labels (18A, 18V) — carries it at the cent, the
    /// way the screen it mirrors renders a money column.
    #[tokio::test]
    async fn api_export_rounds_money_columns_to_the_cent() {
        let pool = long_decimal_disposal().await;

        let csv = client(&pool)
            .get("/portfolio/net-capital-gain/export")
            .await
            .expect_status(StatusCode::OK)
            .text()
            .to_string();
        let mut lines = csv.lines();
        let header: Vec<&str> = lines.next().unwrap().split(',').collect();
        lines.next(); // the ATO label row
        let row: Vec<&str> = lines.next().expect("a record row").split(',').collect();
        let at = |col: &str| row[header.iter().position(|c| *c == col).unwrap()];

        // 3021.89 × 100/300 = 1007.296666… cost base against 1500 proceeds.
        assert_eq!(at("discount_eligible_gains"), "492.70");
        assert_eq!(at("net_discount_eligible_gain"), "492.70");
        assert_eq!(at("cgt_discount"), "246.35");
        assert_eq!(at("net_capital_gain"), "246.35"); // label 18A
        // A nil figure reads as a nil figure, not as twenty-four zeros.
        assert_eq!(at("capital_loss_carried_forward"), "0.00"); // label 18V
        assert_eq!(at("other_gains"), "0.00");
        // Not money, and untouched: the year and the taxpayer assumption.
        assert_eq!(at("tax_year"), "2024");
        assert_eq!(at("taxpayer_basis"), crate::reports::TAXPAYER_BASIS);
        // Every money cell is at the cent — none escaped the projection.
        for (i, cell) in row.iter().enumerate() {
            if header[i] == "tax_year" || header[i] == "taxpayer_basis" {
                continue;
            }
            let dp = cell.split_once('.').map(|(_, f)| f.len()).unwrap_or(0);
            assert_eq!(dp, 2, "{} exported as {cell}", header[i]);
        }
    }

    /// The counterpart of the test above, and **the one W-c control W-f
    /// reversed**: the JSON report now carries the very figures the export
    /// does. W-c rounded only the CSV projection, on the reasoning that the
    /// JSON should stay the exact figure; W-f moved the rounding into the
    /// shared year record instead, because a worksheet whose columns round
    /// independently prints a working that does not reach its own result —
    /// and the record is what the JSON report, the CSV export and the annual
    /// tax report's `cgt_summary` all read. The exact arithmetic is still
    /// there to check against: the gain is 1500 − 3021.89 ÷ 3, and the
    /// reported figure is that to the cent, no further away.
    #[tokio::test]
    async fn api_the_json_report_carries_the_same_cent_figures_as_the_export() {
        let pool = long_decimal_disposal().await;

        let years: Vec<NetCapitalGainYear> =
            client(&pool).get_json("/portfolio/net-capital-gain").await;
        let y = row_for(&years, 2024);
        let cost_base: Decimal = "3021.89".parse::<Decimal>().unwrap() / Decimal::from(3);
        let gain = Decimal::from(1500) - cost_base;
        // The unrounded gain really does have more than two decimal places,
        // so this is testing the rounding and not a coincidence.
        assert!(gain.scale() > 2, "{gain}");
        assert_eq!(y.discount_eligible_gains, "492.70".parse().unwrap());
        assert_eq!(y.cgt_discount, "246.35".parse().unwrap());
        assert_eq!(y.net_capital_gain, "246.35".parse().unwrap());
        // Every reported figure is within half a cent of the exact one.
        let half_cent: Decimal = "0.005".parse().unwrap();
        assert!((y.discount_eligible_gains - gain).abs() <= half_cent);
        assert!((y.net_capital_gain - gain / Decimal::from(2)).abs() <= half_cent);
        // The worksheet reconciles: the printed discount comes off the
        // printed net gain and reaches the printed 18A.
        assert_eq!(
            y.net_discount_eligible_gain - y.cgt_discount + y.net_other_gain,
            y.net_capital_gain
        );
    }

    /// **SCENARIOS W-f, the finding itself.** An entirely ordinary
    /// single-parcel disposal — 100 units bought at $10, sold at $11.0001,
    /// no brokerage — whose discount-eligible gain is an odd number of cents
    /// (100.01). Halved, that is 50.005: rounded independently, the export
    /// printed a working of `100.01 − 50.01` beside an 18A of `50.01`, which
    /// is 50.00 on the page. The discount is the figure that rounds (half
    /// away from zero, so 50.01) and 18A is what the worksheet leaves after
    /// it, so the working reads `100.01 − 50.01 = 50.00` and 18A **is**
    /// 50.00 — the assessable gain landing the taxpayer-favourable way.
    #[tokio::test]
    async fn api_export_the_printed_working_reaches_the_figure_it_works_to() {
        let pool = odd_cent_disposal().await;

        let csv = client(&pool)
            .get("/portfolio/net-capital-gain/export")
            .await
            .expect_status(StatusCode::OK)
            .text()
            .to_string();
        let mut lines = csv.lines();
        let header: Vec<&str> = lines.next().unwrap().split(',').collect();
        lines.next(); // the ATO label row
        let row: Vec<&str> = lines.next().expect("a record row").split(',').collect();
        let at = |col: &str| row[header.iter().position(|c| *c == col).unwrap()];
        let cell = |col: &str| at(col).parse::<Decimal>().unwrap();

        assert_eq!(at("discount_eligible_gains"), "100.01"); // 18H component
        assert_eq!(at("net_discount_eligible_gain"), "100.01"); // 18 working
        assert_eq!(at("cgt_discount"), "50.01"); // 18 working
        assert_eq!(at("net_capital_gain"), "50.00"); // 18A
        // The working, as a reader adds it up off the page.
        assert_eq!(
            cell("net_discount_eligible_gain") - cell("cgt_discount") + cell("net_other_gain"),
            cell("net_capital_gain"),
        );

        // The JSON report answers the same figures — one worksheet, not two.
        let years: Vec<NetCapitalGainYear> =
            client(&pool).get_json("/portfolio/net-capital-gain").await;
        let y = row_for(&years, 2024);
        assert_eq!(y.discount_eligible_gains, "100.01".parse().unwrap());
        assert_eq!(y.cgt_discount, "50.01".parse().unwrap());
        assert_eq!(y.net_capital_gain, "50.00".parse().unwrap());
    }

    /// SCENARIOS W-f: for **every** year row, every column is at the cent and
    /// every derived column is exactly the arithmetic of the rounded inputs —
    /// the netting order, the halving, and the loss chain from one year to the
    /// next. Written to cover a *new* column without anyone listing it: the
    /// record's whole field set must be classified as an input, a derived
    /// column, or not money, so an unclassified addition fails here.
    #[tokio::test]
    async fn api_every_derived_column_is_the_arithmetic_of_the_cent_rounded_inputs() {
        /// Figures the worksheet is computed *from*.
        const INPUTS: &[&str] = &[
            "discount_eligible_gains",
            "other_gains",
            "capital_losses",
            "capital_loss_brought_forward",
            // Informational, and part of no printed working — but money, and
            // so at the cent like every other money column.
            "cgt_event_e10_gain",
            "cgt_event_g1_gain",
            "cgt_event_c2_gain",
        ];
        /// Figures the worksheet computes, asserted below.
        const DERIVED: &[&str] = &[
            "net_discount_eligible_gain",
            "net_other_gain",
            "cgt_discount",
            "net_capital_gain",
            "capital_loss_carried_forward",
        ];
        /// Not money: the year, the taxpayer assumption, and the drilldown
        /// (whose per-disposal rows keep the realised report's own precision).
        const NOT_MONEY: &[&str] = &["tax_year", "taxpayer_basis", "disposals"];

        let pool = odd_cent_years().await;
        let years: Vec<NetCapitalGainYear> =
            client(&pool).get_json("/portfolio/net-capital-gain").await;
        assert!(
            years.len() >= 2,
            "the fixture spans a loss year and a gain year"
        );

        let mut previous_carried: Option<Decimal> = None;
        for y in &years {
            let value = serde_json::to_value(y).unwrap();
            let obj = value.as_object().unwrap();
            for column in obj.keys() {
                let c = column.as_str();
                assert!(
                    INPUTS.contains(&c) || DERIVED.contains(&c) || NOT_MONEY.contains(&c),
                    "{c} is a new column of the year record: classify it as an \
                     input, as derived from the rounded inputs, or as not money",
                );
            }
            for column in INPUTS.iter().chain(DERIVED) {
                let cell = obj
                    .get(*column)
                    .unwrap_or_else(|| panic!("{column} missing from the record"));
                let amount: Decimal = cell.as_str().unwrap().parse().unwrap();
                assert!(
                    amount.scale() <= 2,
                    "FY{} {column} is {amount}, not a figure at the cent",
                    y.tax_year,
                );
            }

            // The derived columns, from the rounded inputs: losses (this
            // year's plus the brought-forward balance) against the
            // non-discountable gains first, then the discount-eligible ones,
            // then the discount off what is left.
            let available = y.capital_losses + y.capital_loss_brought_forward;
            let to_other = y.other_gains.min(available);
            assert_eq!(y.net_other_gain, y.other_gains - to_other);
            let rest = available - to_other;
            let to_discount = y.discount_eligible_gains.min(rest);
            assert_eq!(
                y.net_discount_eligible_gain,
                y.discount_eligible_gains - to_discount
            );
            assert_eq!(y.capital_loss_carried_forward, rest - to_discount);
            assert_eq!(
                y.cgt_discount,
                crate::infra::decimal::to_cents(y.net_discount_eligible_gain / Decimal::TWO)
            );
            assert_eq!(
                y.net_capital_gain,
                y.net_other_gain + (y.net_discount_eligible_gain - y.cgt_discount)
            );

            // …and the chain between years is the same rounded figure.
            if let Some(carried) = previous_carried {
                assert_eq!(y.capital_loss_brought_forward, carried);
            }
            previous_carried = Some(y.capital_loss_carried_forward);
        }
    }

    /// SCENARIOS W-f: the annual tax report's printed CGT summary is the same
    /// worksheet, so it must agree with both the JSON report and the CSV
    /// export for the year — figure for figure, and adding up on the page.
    #[tokio::test]
    async fn api_the_annual_tax_reports_cgt_summary_agrees_with_the_json_and_the_csv() {
        let pool = odd_cent_disposal().await;
        let api = ApiClient::full(&pool);

        let years: Vec<NetCapitalGainYear> = api.get_json("/portfolio/net-capital-gain").await;
        let y = row_for(&years, 2024);

        let csv = api
            .get("/portfolio/net-capital-gain/export")
            .await
            .expect_status(StatusCode::OK)
            .text()
            .to_string();
        let mut lines = csv.lines();
        let header: Vec<&str> = lines.next().unwrap().split(',').collect();
        lines.next();
        let row: Vec<&str> = lines.next().expect("a record row").split(',').collect();
        let cell = |col: &str| {
            row[header.iter().position(|c| *c == col).unwrap()]
                .parse::<Decimal>()
                .unwrap()
        };

        let report: serde_json::Value = api
            .post_json(
                "/reports/tax-report",
                &serde_json::json!({ "tax_year": 2024 }),
            )
            .await;
        let summary = &report["cgt_summary"];
        let line = |name: &str| summary[name].as_str().unwrap().parse::<Decimal>().unwrap();

        assert_eq!(line("net_capital_gain"), y.net_capital_gain);
        assert_eq!(line("net_capital_gain"), cell("net_capital_gain"));
        assert_eq!(line("cgt_concession_amount"), y.cgt_discount);
        assert_eq!(line("cgt_concession_amount"), cell("cgt_discount"));
        assert_eq!(
            line("net_discount_eligible_gain"),
            cell("net_discount_eligible_gain")
        );
        assert_eq!(line("short_term_gains"), cell("other_gains"));
        assert_eq!(
            line("long_term_gains") + line("amma_discount_gains_grossed_up"),
            cell("discount_eligible_gains")
        );
        // The printed worksheet's own working, line by line.
        assert_eq!(
            line("long_term_gains") + line("amma_discount_gains_grossed_up")
                - line("losses_applied_discount"),
            line("net_discount_eligible_gain")
        );
        assert_eq!(
            line("short_term_gains") - line("losses_applied_other"),
            line("net_other_gain")
        );
        assert_eq!(
            line("net_discount_eligible_gain") - line("cgt_concession_amount")
                + line("net_other_gain"),
            line("net_capital_gain")
        );
    }

    /// The control for the three tests above: a year whose figures are
    /// already exact at the cent is untouched by any of the rounding — the
    /// same round numbers before and after, on the JSON and in the export.
    #[tokio::test]
    async fn api_a_year_already_exact_at_the_cent_is_unchanged() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "VAS").await;
        test_support::trade(1, 1, trade::TradeType::Buy)
            .date(NaiveDate::from_ymd_opt(2022, 1, 5).unwrap())
            .qty(Decimal::from(100))
            .price(Decimal::from(10))
            .insert(&pool)
            .await;
        test_support::trade(2, 1, trade::TradeType::Sell)
            .date(NaiveDate::from_ymd_opt(2024, 3, 15).unwrap())
            .qty(Decimal::from(100))
            .price(Decimal::from(15))
            .insert(&pool)
            .await;
        allocate(&pool, 1, 2, 1, Decimal::from(100)).await;

        let years: Vec<NetCapitalGainYear> =
            client(&pool).get_json("/portfolio/net-capital-gain").await;
        let y = row_for(&years, 2024);
        assert_eq!(y.discount_eligible_gains, Decimal::from(500));
        assert_eq!(y.net_discount_eligible_gain, Decimal::from(500));
        assert_eq!(y.cgt_discount, Decimal::from(250));
        assert_eq!(y.net_capital_gain, Decimal::from(250));

        let csv = client(&pool)
            .get("/portfolio/net-capital-gain/export")
            .await
            .expect_status(StatusCode::OK)
            .text()
            .to_string();
        let mut lines = csv.lines();
        let header: Vec<&str> = lines.next().unwrap().split(',').collect();
        lines.next();
        let row: Vec<&str> = lines.next().expect("a record row").split(',').collect();
        let at = |col: &str| row[header.iter().position(|c| *c == col).unwrap()];
        assert_eq!(at("discount_eligible_gains"), "500.00");
        assert_eq!(at("cgt_discount"), "250.00");
        assert_eq!(at("net_capital_gain"), "250.00");
    }

    /// The finding's own facts: 100 units bought 2022-01-05 at $10, sold
    /// 2024-03-15 at $11.0001, no brokerage — a gain of exactly 100.01, an
    /// odd number of cents, which is all it takes.
    async fn odd_cent_disposal() -> SqlitePool {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "VAS").await;
        test_support::trade(1, 1, trade::TradeType::Buy)
            .date(NaiveDate::from_ymd_opt(2022, 1, 5).unwrap())
            .qty(Decimal::from(100))
            .price(Decimal::from(10))
            .insert(&pool)
            .await;
        test_support::trade(2, 1, trade::TradeType::Sell)
            .date(NaiveDate::from_ymd_opt(2024, 3, 15).unwrap())
            .qty(Decimal::from(100))
            .price("11.0001".parse().unwrap())
            .insert(&pool)
            .await;
        allocate(&pool, 1, 2, 1, Decimal::from(100)).await;
        pool
    }

    /// Two years of long-decimal figures with a loss chaining between them:
    /// one parcel bought with brokerage + GST (so every pro-rated cost base
    /// is a non-terminating quotient), a third sold at a loss in FY2023 and a
    /// third at a gain in FY2024, over an entered opening loss that is itself
    /// a fraction of a cent.
    async fn odd_cent_years() -> SqlitePool {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "VAS").await;
        cgt_settings::db_upsert(
            &pool,
            &cgt_settings::CgtSettings {
                id: 1,
                opening_capital_loss: "10.0049".parse().unwrap(),
            },
        )
        .await
        .unwrap();
        test_support::trade(1, 1, trade::TradeType::Buy)
            .date(NaiveDate::from_ymd_opt(2021, 7, 1).unwrap())
            .qty(Decimal::from(300))
            .price(Decimal::from(10))
            .brokerage("19.90".parse().unwrap())
            .gst_on_brokerage("1.99".parse().unwrap())
            .insert(&pool)
            .await;
        test_support::trade(2, 1, trade::TradeType::Sell)
            .date(NaiveDate::from_ymd_opt(2023, 3, 15).unwrap())
            .qty(Decimal::from(100))
            .price("9.0003".parse().unwrap())
            .insert(&pool)
            .await;
        allocate(&pool, 1, 2, 1, Decimal::from(100)).await;
        test_support::trade(3, 1, trade::TradeType::Sell)
            .date(NaiveDate::from_ymd_opt(2024, 3, 15).unwrap())
            .qty(Decimal::from(100))
            .price("15.0007".parse().unwrap())
            .insert(&pool)
            .await;
        allocate(&pool, 2, 3, 1, Decimal::from(100)).await;
        pool
    }

    /// One parcel bought with brokerage + GST, a third of it sold at a gain:
    /// the pro-rate (`cost × units ÷ quantity`) is 3021.89 ÷ 3, which does not
    /// terminate. The ordinary shape behind the finding.
    async fn long_decimal_disposal() -> SqlitePool {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "VAS").await;
        test_support::trade(1, 1, trade::TradeType::Buy)
            .date(NaiveDate::from_ymd_opt(2022, 1, 5).unwrap())
            .qty(Decimal::from(300))
            .price(Decimal::from(10))
            .brokerage("19.90".parse().unwrap())
            .gst_on_brokerage("1.99".parse().unwrap())
            .insert(&pool)
            .await;
        test_support::trade(2, 1, trade::TradeType::Sell)
            .date(NaiveDate::from_ymd_opt(2024, 3, 15).unwrap())
            .qty(Decimal::from(100))
            .price(Decimal::from(15))
            .insert(&pool)
            .await;
        allocate(&pool, 1, 2, 1, Decimal::from(100)).await;
        pool
    }

    #[tokio::test]
    async fn api_export_of_empty_report_still_returns_header() {
        let pool = test_pool().await;
        let resp = client(&pool)
            .get("/portfolio/net-capital-gain/export")
            .await;
        assert_eq!(resp.status, StatusCode::OK);
        let csv = resp.text().to_string();
        assert_eq!(
            csv,
            CSV_HEADER.join(",") + "\n" + &CSV_ATO_LABELS.join(",") + "\n"
        );
    }

    /// SCENARIOS O-03/O-04: a database whose only content is an entered
    /// opening carried-forward loss. Nothing is bucketed — there is no
    /// recorded fact to bucket — and yet label 18V is $12,345 and has to be
    /// reported somewhere. The financial year in progress carries it, on both
    /// the JSON report and the CSV export (which used to be two header rows
    /// and nothing else).
    #[tokio::test]
    async fn api_an_opening_loss_alone_is_reported_in_the_current_year() {
        let pool = test_pool().await;
        let api = ApiClient::full(&pool);
        api.put_ok(
            "/cgt_settings/1",
            &serde_json::json!({ "opening_capital_loss": "12345" }),
        )
        .await;

        let years: Vec<NetCapitalGainYear> = api.get_json("/portfolio/net-capital-gain").await;
        assert_eq!(tax_years(&years), vec![current_tax_year()]);
        assert_quiet_year(&years[0], Decimal::from(12345));

        let csv = api
            .get("/portfolio/net-capital-gain/export")
            .await
            .expect_status(StatusCode::OK)
            .text()
            .to_string();
        let mut lines = csv.lines().skip(2); // past both header rows
        let fields: Vec<&str> = lines.next().expect("a record row").split(',').collect();
        assert_eq!(fields.len(), CSV_HEADER.len());
        assert_eq!(fields[0], current_tax_year().to_string()); // tax_year
        // capital_loss_brought_forward (18V prior year) and
        // capital_loss_carried_forward (18V) both carry the balance.
        assert_eq!(fields[4].parse::<Decimal>().unwrap(), Decimal::from(12345));
        assert_eq!(fields[9].parse::<Decimal>().unwrap(), Decimal::from(12345));
        assert_eq!(lines.next(), None);
    }

    /// SCENARIOS O-03/O-04, the ordinary form: losses in FY2023–FY2025 leave
    /// $4,000 carried forward and then nothing happens. Every quiet year from
    /// FY2026 to the year in progress reports the $4,000 it is still carrying,
    /// rather than the series simply stopping at the last year something
    /// happened in.
    #[tokio::test]
    async fn api_quiet_years_after_the_last_activity_still_report_the_balance() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "VAS").await;
        // Three loss years: 1,000 + 2,000 + 1,000 = 4,000, no gains anywhere.
        for (i, (fy, loss_per_unit)) in [(2023, 10), (2024, 20), (2025, 10)].iter().enumerate() {
            let buy = (i as i64) * 2 + 1;
            insert_trade(
                &pool,
                buy,
                trade::TradeType::Buy,
                1,
                NaiveDate::from_ymd_opt(fy - 1, 8, 1).unwrap(),
                Decimal::from(100),
                Decimal::from(*loss_per_unit + 10),
            )
            .await;
            insert_trade(
                &pool,
                buy + 1,
                trade::TradeType::Sell,
                1,
                NaiveDate::from_ymd_opt(*fy, 5, 1).unwrap(),
                Decimal::from(100),
                Decimal::from(10),
            )
            .await;
            allocate(&pool, buy, buy + 1, buy, Decimal::from(100)).await;
        }

        let years: Vec<NetCapitalGainYear> = ApiClient::full(&pool)
            .get_json("/portfolio/net-capital-gain")
            .await;
        assert_eq!(
            tax_years(&years),
            (2023..=current_tax_year()).collect::<Vec<_>>()
        );
        assert_eq!(
            row_for(&years, 2025).capital_loss_carried_forward,
            Decimal::from(4000)
        );
        // FY2026 onwards: nothing happened, the $4,000 is still carried.
        for quiet in years.iter().filter(|y| y.tax_year > 2025) {
            assert_quiet_year(quiet, Decimal::from(4000));
        }
    }

    /// The other half of the rule: a quiet year with **no** balance to carry
    /// is still absent. The series is an activity list plus the years that owe
    /// an 18V figure — never a row per year regardless.
    #[tokio::test]
    async fn db_a_quiet_year_with_no_balance_gets_no_row() {
        let pool = test_pool().await;
        // FY2024: a gain, fully assessable — nothing is carried out of it.
        insert_listing(&pool, 1, "VAS").await;
        insert_trade(
            &pool,
            1,
            trade::TradeType::Buy,
            1,
            NaiveDate::from_ymd_opt(2023, 8, 1).unwrap(),
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

        let r = db_net_capital_gain(&pool).await.unwrap();
        assert_eq!(tax_years(&r), vec![2024]);
        assert_eq!(r[0].net_capital_gain, Decimal::from(500));
    }

    /// Each exported column's tax-return label sits under its column (same
    /// index in both rows): the headline figures map per
    /// docs/ato/tax-return-labels-2026.md.
    #[tokio::test]
    async fn db_ato_labels_align_with_their_columns() {
        assert_eq!(CSV_HEADER.len(), CSV_ATO_LABELS.len());
        let label_of = |col: &str| {
            let i = CSV_HEADER.iter().position(|c| *c == col).unwrap();
            CSV_ATO_LABELS[i]
        };
        assert_eq!(label_of("net_capital_gain"), "18A");
        assert_eq!(label_of("capital_loss_carried_forward"), "18V");
        assert_eq!(label_of("capital_loss_brought_forward"), "18V (prior year)");
        // 18H (total current year capital gains) is the sum of the two gross
        // gain columns — both marked as its components.
        assert_eq!(label_of("discount_eligible_gains"), "18H (component)");
        assert_eq!(label_of("other_gains"), "18H (component)");
        // Informational columns report at no label.
        assert_eq!(label_of("cgt_event_e10_gain"), "");
        assert_eq!(label_of("cgt_event_g1_gain"), "");
        assert_eq!(label_of("taxpayer_basis"), "");
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
                    demerger_close_date: None,
                    demerger_close_price: None,
                    demerger_close_sourced_from: None,
                    demerger_close_reason: None,
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
        let resp = client(&pool)
            .post("/portfolio/net-capital-gain/what-if", &body)
            .await;
        let status = resp.status;
        (status, resp.body.to_vec())
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
            NaiveDate::from_ymd_opt(2024, 1, 2).unwrap(),
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
            NaiveDate::from_ymd_opt(2026, 1, 2).unwrap(),
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
            NaiveDate::from_ymd_opt(2023, 7, 3).unwrap(),
            Decimal::from(100),
            Decimal::from(10),
        )
        .await;
        insert_trade(
            &pool,
            2,
            trade::TradeType::Sell,
            1,
            NaiveDate::from_ymd_opt(2024, 6, 3).unwrap(),
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
            NaiveDate::from_ymd_opt(2024, 1, 2).unwrap(),
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
            NaiveDate::from_ymd_opt(2024, 1, 2).unwrap(),
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
        let msg = String::from_utf8(body).unwrap();
        assert!(msg.contains("99"), "{msg}");
        // The rejection names the listing by ticker, never a raw id.
        assert!(msg.contains("VAS"), "{msg}");

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

    /// A second holding account, so a listing can be held in two places at
    /// once.
    async fn insert_second_account(pool: &SqlitePool) {
        use crate::entities::holding_account::{self, HoldingAccount};
        holding_account::db_upsert(
            pool,
            &HoldingAccount {
                id: 2,
                name: "ICE Employee Plan".to_string(),
            },
        )
        .await
        .unwrap();
    }

    /// 2,000 units of TSTG open in the default account and 5,000 in a second
    /// one — the shape that makes an unqualified over-request refusal false.
    async fn two_account_fixture(pool: &SqlitePool) {
        insert_listing(pool, 1, "TSTG").await;
        insert_second_account(pool).await;
        for (id, account, qty) in [(1, 1, 2000), (2, 2, 5000)] {
            test_support::buy(id, 1)
                .account(account)
                .date(NaiveDate::from_ymd_opt(2020, 1, 2).unwrap())
                .qty(Decimal::from(qty))
                .price(Decimal::from(10))
                .insert(pool)
                .await;
        }
    }

    /// SCENARIOS O-16. The strategy branch's over-request refusal names the
    /// account the request scoped the candidates to. Without it, "only 2000
    /// unit(s) of TSTG are open" is simply false of the 7,000 units held —
    /// and the same endpoint's allocations branch, and the optimiser's own
    /// refusal, both name it.
    #[tokio::test]
    async fn api_what_if_over_request_names_the_account_it_was_scoped_to() {
        let pool = test_pool().await;
        two_account_fixture(&pool).await;

        let (status, body) = post_what_if(
            pool,
            serde_json::json!({
                "listing_id": 1, "holding_account_id": 1, "units": "3000",
                "proceeds": "30000", "date": "2026-06-15", "strategy": "fifo"
            }),
        )
        .await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
        let msg = String::from_utf8(body).unwrap();
        assert_eq!(
            msg,
            "only 2000 unit(s) of TSTG are open in account 'Default'"
        );
    }

    /// The other half of the same rule: an unscoped request bounds by every
    /// account's parcels, so there is no account to name and none is named.
    #[tokio::test]
    async fn api_what_if_unscoped_over_request_names_no_account() {
        let pool = test_pool().await;
        two_account_fixture(&pool).await;

        let (status, body) = post_what_if(
            pool,
            serde_json::json!({
                "listing_id": 1, "units": "8000", "proceeds": "80000",
                "date": "2026-06-15", "strategy": "fifo"
            }),
        )
        .await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
        let msg = String::from_utf8(body).unwrap();
        assert_eq!(msg, "only 7000 unit(s) of TSTG are open");
    }

    /// SCENARIOS O-14. The candidates are the parcels open **as at the
    /// disposal's date**: a parcel acquired after it cannot be sold on it, and
    /// the explicit allocation naming it is refused — the Sell path's own
    /// rule ("an allocated parcel is dated after the sale date"), which the
    /// what-if used to answer with figures for a sale that can never be
    /// recorded.
    #[tokio::test]
    async fn api_what_if_refuses_a_parcel_acquired_after_the_disposal_date() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "VAS").await;
        insert_trade(
            &pool,
            1,
            trade::TradeType::Buy,
            1,
            NaiveDate::from_ymd_opt(2022, 1, 4).unwrap(),
            Decimal::from(100),
            Decimal::from(10),
        )
        .await;

        let (status, body) = post_what_if(
            pool,
            serde_json::json!({
                "listing_id": 1, "units": "100", "proceeds": "11000",
                "date": "2021-12-31",
                "allocations": [ { "purchase_trade_id": 1, "units": "100" } ]
            }),
        )
        .await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
        let msg = String::from_utf8(body).unwrap();
        assert!(msg.contains("not an open parcel"), "{msg}");
        assert!(msg.contains("VAS"), "{msg}");
    }

    /// The boundary the Sell path draws, kept: a parcel acquired **on** the
    /// disposal date is a legitimate allocation, here and there.
    #[tokio::test]
    async fn api_what_if_accepts_a_parcel_acquired_on_the_disposal_date() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "VAS").await;
        insert_trade(
            &pool,
            1,
            trade::TradeType::Buy,
            1,
            NaiveDate::from_ymd_opt(2022, 1, 4).unwrap(),
            Decimal::from(100),
            Decimal::from(10),
        )
        .await;

        let (status, body) = post_what_if(
            pool,
            serde_json::json!({
                "listing_id": 1, "units": "100", "proceeds": "2000",
                "date": "2022-01-04",
                "allocations": [ { "purchase_trade_id": 1, "units": "100" } ]
            }),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{}", String::from_utf8_lossy(&body));
        let r: WhatIfResponse = serde_json::from_slice(&body).unwrap();
        assert_eq!(r.allocations.len(), 1);
        // Sold the day it was bought — a gain, and not a discountable one.
        assert!(!r.allocations[0].discount_eligible);
        assert_eq!(r.hypothetical.non_discountable_gain, Decimal::from(1000));
    }

    /// The strategy branch reads the same candidates, so a later parcel is
    /// neither allocated nor counted in the open quantity the over-request
    /// check bounds by.
    #[tokio::test]
    async fn api_what_if_strategy_ignores_parcels_acquired_after_the_disposal_date() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "VAS").await;
        // Each year's first trading day: 1 January is New Year's Day on the
        // seeded ASX calendar and a trade cannot be dated on a day the
        // exchange was shut (SCENARIOS S-08), so 2022's falls on the 4th.
        for (id, date) in [
            (1, NaiveDate::from_ymd_opt(2020, 1, 2).unwrap()),
            (2, NaiveDate::from_ymd_opt(2022, 1, 4).unwrap()),
        ] {
            insert_trade(
                &pool,
                id,
                trade::TradeType::Buy,
                1,
                date,
                Decimal::from(100),
                Decimal::from(10),
            )
            .await;
        }

        // 200 units are open today, but only 100 were open on 2021-06-30.
        let (status, body) = post_what_if(
            pool.clone(),
            serde_json::json!({
                "listing_id": 1, "units": "150", "proceeds": "3000",
                "date": "2021-06-30", "strategy": "fifo"
            }),
        )
        .await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
        assert!(
            String::from_utf8(body).unwrap().contains("only 100"),
            "the open quantity must be what was open at the disposal date"
        );

        // Within that bound, only the parcel that existed is allocated.
        let (status, body) = post_what_if(
            pool,
            serde_json::json!({
                "listing_id": 1, "units": "100", "proceeds": "2000",
                "date": "2021-06-30", "strategy": "fifo"
            }),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{}", String::from_utf8_lossy(&body));
        let r: WhatIfResponse = serde_json::from_slice(&body).unwrap();
        assert_eq!(r.allocations.len(), 1);
        assert_eq!(r.allocations[0].purchase_trade_id, 1);
        // Held from 2020-01-01 to 2021-06-30 — over 12 months.
        assert!(r.allocations[0].discount_eligible);
        assert_eq!(r.hypothetical.discount_eligible_gain, Decimal::from(1000));
    }

    /// The other direction of the same read: a parcel sold *since* the
    /// disposal date was there to be sold on it, so a past-dated what-if
    /// offers it rather than under-reporting what could have been sold.
    #[tokio::test]
    async fn api_what_if_offers_a_parcel_sold_since_the_disposal_date() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "VAS").await;
        insert_trade(
            &pool,
            1,
            trade::TradeType::Buy,
            1,
            NaiveDate::from_ymd_opt(2020, 1, 2).unwrap(),
            Decimal::from(100),
            Decimal::from(10),
        )
        .await;
        // Fully sold in 2024 — after the 2023-06-15 disposal being modelled.
        insert_trade(
            &pool,
            2,
            trade::TradeType::Sell,
            1,
            NaiveDate::from_ymd_opt(2024, 3, 1).unwrap(),
            Decimal::from(100),
            Decimal::from(12),
        )
        .await;
        allocate(&pool, 1, 2, 1, Decimal::from(100)).await;

        let (status, body) = post_what_if(
            pool,
            serde_json::json!({
                "listing_id": 1, "units": "100", "proceeds": "2000",
                "date": "2023-06-15", "strategy": "fifo"
            }),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{}", String::from_utf8_lossy(&body));
        let r: WhatIfResponse = serde_json::from_slice(&body).unwrap();
        assert_eq!(r.allocations.len(), 1);
        assert_eq!(r.allocations[0].purchase_trade_id, 1);
    }

    /// A future-dated disposal is unchanged: every parcel open today is a
    /// legitimate candidate for a sale contemplated later.
    #[tokio::test]
    async fn api_what_if_dated_in_the_future_still_sees_every_open_parcel() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "VAS").await;
        insert_trade(
            &pool,
            1,
            trade::TradeType::Buy,
            1,
            NaiveDate::from_ymd_opt(2024, 1, 2).unwrap(),
            Decimal::from(100),
            Decimal::from(10),
        )
        .await;
        let later = crate::infra::date::today() + chrono::Duration::days(400);

        let (status, body) = post_what_if(
            pool,
            serde_json::json!({
                "listing_id": 1, "units": "100", "proceeds": "2000",
                "date": later.to_string(), "strategy": "fifo"
            }),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{}", String::from_utf8_lossy(&body));
        let r: WhatIfResponse = serde_json::from_slice(&body).unwrap();
        assert_eq!(r.allocations.len(), 1);
        assert_eq!(r.allocations[0].purchase_trade_id, 1);
        assert!(r.allocations[0].discount_eligible);
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
            NaiveDate::from_ymd_opt(2026, 1, 2).unwrap(),
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
            NaiveDate::from_ymd_opt(2025, 1, 2).unwrap(),
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

    /// Three currencies disposed of in one financial year net to a single AUD
    /// figure, each leg converted at its own currency's month rate before it
    /// joins the total (SCENARIOS M-15). The three gains are deliberately
    /// equal in AUD and different in their native amounts, so a leg converted
    /// at the wrong currency's rate — or not at all — cannot coincidentally
    /// land on the same total.
    #[tokio::test]
    async fn db_three_currencies_in_one_year_net_to_one_aud_gain() {
        let pool = test_pool().await;
        for (id, ticker, currency) in [(1, "BHP", "AUD"), (2, "AAPL", "USD"), (3, "HSBA", "GBP")] {
            test_support::listing(id)
                .ticker(ticker)
                .name(ticker)
                .mic(if currency == "AUD" { "XASX" } else { "XNYS" })
                .currency(currency)
                .insert(&pool)
                .await;
        }
        // A$1 = 0.50 USD and 0.25 GBP in both the buy and the sell month, so
        // each leg's AUD figures are exact.
        for (currency, rate) in [("USD", "0.50"), ("GBP", "0.25")] {
            for month in ["2023-08", "2024-05"] {
                rba_fx_rate::db_import_rate(&pool, currency, month, rate.parse().unwrap())
                    .await
                    .unwrap();
            }
        }
        let buy_date = NaiveDate::from_ymd_opt(2023, 8, 10).unwrap();
        let sell_date = NaiveDate::from_ymd_opt(2024, 5, 20).unwrap();
        // Each holding: 100 units, cost A$2,000, proceeds A$3,000 → A$1,000
        // gain, held under 12 months so nothing is discounted away.
        for (listing_id, currency, buy_price, sell_price) in [
            (1, "AUD", "20", "30"),
            (2, "USD", "10", "15"),
            (3, "GBP", "5", "7.5"),
        ] {
            for (id, trade_type, price) in [
                (listing_id, trade::TradeType::Buy, buy_price),
                (listing_id + 10, trade::TradeType::Sell, sell_price),
            ] {
                test_support::trade(id, listing_id, trade_type)
                    .date(if trade_type == trade::TradeType::Buy {
                        buy_date
                    } else {
                        sell_date
                    })
                    .qty(Decimal::from(100))
                    .price(price.parse().unwrap())
                    .currency(currency)
                    .fx_rate(if currency == "AUD" {
                        Decimal::ONE
                    } else {
                        // A wrong fallback: the ATO rate must win.
                        "0.99".parse().unwrap()
                    })
                    .insert(&pool)
                    .await;
            }
            allocate(
                &pool,
                listing_id,
                listing_id + 10,
                listing_id,
                Decimal::from(100),
            )
            .await;
        }

        let years = db_net_capital_gain(&pool).await.unwrap();
        assert_eq!(years.len(), 1);
        let y = &years[0];
        assert_eq!(y.tax_year, 2024);
        assert_eq!(y.disposals.len(), 3);
        for d in &y.disposals {
            assert_eq!(d.cost_base, Decimal::from(2000));
            assert_eq!(d.proceeds, Decimal::from(3000));
            assert_eq!(d.capital_gain_loss, Decimal::from(1000));
        }
        // All three held under 12 months, so the whole A$3,000 is other gains.
        assert_eq!(y.other_gains, Decimal::from(3000));
        assert_eq!(y.discount_eligible_gains, Decimal::ZERO);
        assert_eq!(y.net_capital_gain, Decimal::from(3000));
    }
}
