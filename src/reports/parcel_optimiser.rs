//! Parcel-selection optimiser: candidate allocation strategies for a
//! contemplated sale.
//!
//! Which parcels a sale comes from is the taxpayer's *choice*
//! (`docs/ato/cgt-keeping-records-shares.md` — Boris picks the parcel that
//! realises a loss), and it is the largest legal CGT lever an individual has.
//! Given a listing, holding account, unit quantity, sale date, and a price
//! (live-fetched by default, per the live-valuation rules; an explicit price
//! wins), this report returns one candidate per strategy — minimise the
//! current-year assessable gain, maximise the discount-eligible proportion,
//! harvest losses first, FIFO as the baseline — each with its per-parcel
//! allocation and the resulting gross gain / discountable split, so the user
//! can pick allocations for the real Sell. Read-only: nothing is persisted.
//!
//! The candidate *set* is the shared open-parcels read **as at the
//! contemplated sale's date** (units in that date's basis) — the same parcels
//! a real Sell dated then could allocate. Their **cost bases** are not that
//! read's, though: an open-parcels row costs units still *held*, and this
//! report rehearses their disposal. Every figure here is instead costed
//! through `domain::contemplated_disposal`, which runs the shared pipeline
//! with the inputs the realised-gains report will use once the Sell exists —
//! so every cost-base rule (AMIT/E10, return of capital/G1, splits, rollover
//! carried dates and cost bases) flows through unchanged, *and* the estimate
//! is the figure the sale it rehearses will realise. Each allocation is
//! costed at its own unit count rather than pro-rated off the parcel,
//! because the pipeline is not linear in the disposed units where an AMMA
//! statement covers only part of a parcel (see that module).
//!
//! The hypothetical sale carries no brokerage (it isn't known yet), and the
//! 12-month discount clock is the shared ownership rule
//! (`domain::cgt_discount`), run from the parcel's (possibly deemed)
//! acquisition date to the sale date — the same rule, not a matching copy of
//! it, as the realised report applies.

use crate::domain::adjustments::UnitsAt;
use crate::domain::cgt_discount::discount_eligible;
use crate::domain::cost_base::ParcelRow;
use crate::domain::{contemplated_disposal, open_parcels};
use crate::entities::closing_price::{self, SharedFetcher};
use crate::infra::decimal::mul_div;
use crate::infra::extract::{Json, Query};
use crate::infra::http::ApiError;
use crate::infra::id::{HoldingAccountId, ListingId, TradeId};
use crate::infra::{date, db};
use crate::reports::{labels, taxpayer};
use axum::Extension;
use axum::extract::State;
use chrono::NaiveDate;
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;
use std::cmp::Ordering;
use std::collections::HashMap;
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

/// A parcel-selection strategy: the order in which open parcels are consumed
/// by a hypothetical sale.
#[derive(utoipa::ToSchema, Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Strategy {
    /// Oldest acquisition first — the no-choice baseline.
    Fifo,
    /// The allocation whose own assessable gain — losses netted against
    /// the non-discountable gains first, then the discount-eligible ones,
    /// which are then halved — is the least (`min_gain_picks`).
    MinGain,
    /// Discount-eligible gain parcels first, then losses, then
    /// non-discountable gains last — maximises the proportion of the
    /// realised gain that gets the 50% discount.
    MaxDiscount,
    /// Loss parcels first (largest per-unit loss first), then FIFO.
    HarvestLosses,
}

/// Every strategy, in the order the report presents them.
pub const ALL_STRATEGIES: [Strategy; 4] = [
    Strategy::Fifo,
    Strategy::MinGain,
    Strategy::MaxDiscount,
    Strategy::HarvestLosses,
];

/// An open parcel as the strategies see it: remaining units (current basis)
/// and their adjusted cost base in AUD, with the (possibly deemed)
/// acquisition date driving FIFO order and the 12-month discount clock.
#[derive(Debug, Clone)]
pub struct CandidateParcel {
    pub trade_id: TradeId,
    pub holding_account_id: HoldingAccountId,
    pub acquisition_date: NaiveDate,
    pub remaining_quantity: Decimal,
    /// Adjusted cost base of the remaining units, AUD — costed as **disposed
    /// of on the contemplated sale date**, so it is the figure the
    /// realised-gains report will show for a pick that takes the whole parcel
    /// (see [`Candidates`]). It is also the per-unit basis the strategies
    /// rank on; a pick smaller than the parcel is costed exactly rather than
    /// pro-rated off this figure, because the pipeline is not linear in the
    /// disposed units where an AMMA statement covers only part of a parcel
    /// (`domain::contemplated_disposal`).
    pub remaining_cost_base: Decimal,
}

/// The candidates for one contemplated sale: the open parcels it may draw on,
/// plus the reference data needed to cost **any** allocation of them the way
/// the recorded Sell will (`domain::contemplated_disposal`).
///
/// The two travel together because they answer one question between them, and
/// because they must be read from one snapshot: the parcels come from the
/// shared open-parcels loader as at the sale date, the costing from the same
/// connection. `sale_date` is held here too so [`disposal_figures`] cannot be
/// handed a different date from the one the candidates were assembled for.
pub struct Candidates {
    parcels: Vec<CandidateParcel>,
    /// The candidates' trade rows, keyed by trade id — [`CandidateParcel`]
    /// carries only what the strategies rank on, while costing a pick needs
    /// the whole parcel row.
    rows: HashMap<TradeId, ParcelRow>,
    costing: contemplated_disposal::Costing,
    sale_date: NaiveDate,
}

impl Candidates {
    /// The candidate parcels, FIFO-ordered (acquisition date, then trade id).
    pub fn parcels(&self) -> &[CandidateParcel] {
        &self.parcels
    }

    /// The contemplated sale's date — the date the candidates were assembled
    /// as at, and the one every figure derived from them is costed on.
    pub fn sale_date(&self) -> NaiveDate {
        self.sale_date
    }

    /// The AUD adjusted cost base of `units` (sale-date basis) out of the
    /// candidate parcel created by `trade_id`.
    fn cost_base(&self, trade_id: TradeId, units: Decimal) -> Result<Decimal, sqlx::Error> {
        let row = self
            .rows
            .get(&trade_id)
            .expect("every pick names a candidate parcel");
        self.costing.adjusted_cost_base_aud(
            row,
            UnitsAt::new(units, self.sale_date),
            self.sale_date,
        )
    }
}

/// The open parcels a hypothetical sale of `listing_id` **on `sale_date`** can
/// draw on, optionally restricted to one holding account (a real Sell's
/// allocations may only consume its own account's parcels), in FIFO order.
///
/// The candidates are the parcels open on the contemplated sale's own date,
/// which is exactly the set a real Sell dated then could allocate — a parcel
/// acquired after it does not exist yet (the Sell path refuses such an
/// allocation outright), and one sold since was still there to sell.
/// Quantities come back in that date's unit basis (`docs/API.md`'s As-at date
/// section), so the caller's `units` and per-unit price must be on that basis
/// too.
pub async fn db_candidate_parcels(
    pool: &SqlitePool,
    listing_id: ListingId,
    holding_account_id: Option<HoldingAccountId>,
    sale_date: NaiveDate,
) -> Result<Candidates, sqlx::Error> {
    let mut tx = db::read_tx(pool).await?;
    let candidates =
        db_candidate_parcels_on(&mut tx, listing_id, holding_account_id, sale_date).await?;
    tx.commit().await?;
    Ok(candidates)
}

/// The same candidates read on the caller's own connection, for the what-if,
/// which folds them into its wider single-snapshot read transaction.
pub async fn db_candidate_parcels_on(
    conn: &mut sqlx::SqliteConnection,
    listing_id: ListingId,
    holding_account_id: Option<HoldingAccountId>,
    sale_date: NaiveDate,
) -> Result<Candidates, sqlx::Error> {
    // The candidate *set* and the remaining quantities are the shared
    // open-holdings read, as at the sale date. Its cost bases are not used:
    // they are the cost of units still **held** on that date, and this report
    // rehearses their disposal (`domain::contemplated_disposal`).
    let open: Vec<_> = open_parcels::load(&mut *conn, Some(sale_date))
        .await?
        .into_iter()
        .filter(|p| {
            p.parcel.listing_id == listing_id
                && holding_account_id.is_none_or(|a| p.parcel.holding_account_id == a)
        })
        .collect();
    let costing = contemplated_disposal::Costing::load(&mut *conn).await?;

    let mut parcels = Vec::with_capacity(open.len());
    let mut rows = HashMap::with_capacity(open.len());
    for p in open {
        parcels.push(CandidateParcel {
            trade_id: p.parcel.id,
            holding_account_id: p.parcel.holding_account_id,
            // The (possibly deemed) acquisition date: a rollover replacement
            // parcel carries the consumed parcel's, which is what drives FIFO
            // order and the discount clock.
            acquisition_date: p.parcel.acquired(),
            remaining_quantity: p.remaining_as_of.quantity(),
            remaining_cost_base: costing.adjusted_cost_base_aud(
                &p.parcel,
                p.remaining_as_of,
                sale_date,
            )?,
        });
        rows.insert(p.parcel.id, p.parcel);
    }
    parcels.sort_by_key(|p| (p.acquisition_date, p.trade_id));
    Ok(Candidates {
        parcels,
        rows,
        costing,
        sale_date,
    })
}

/// Allocate `units` across the candidates' parcels in the strategy's
/// preference order, greedily consuming each parcel's remaining quantity.
/// `price` is the per-unit sale price in AUD (it drives the gain-based
/// orderings); the sale date is the one the candidates were assembled as at,
/// so the discount clock cannot be read on a different date from the costing.
pub fn allocate_strategy(
    candidates: &Candidates,
    units: Decimal,
    price: Decimal,
    strategy: Strategy,
) -> Vec<(TradeId, Decimal)> {
    allocate_parcels(
        candidates.parcels(),
        units,
        price,
        candidates.sale_date(),
        strategy,
    )
}

/// [`allocate_strategy`] over a bare slice, which it orders itself: every
/// strategy, FIFO included, sorts on its own key with FIFO (acquisition date,
/// then trade id) as the tie-break, so the result does not depend on the order
/// the slice arrives in.
fn allocate_parcels(
    parcels: &[CandidateParcel],
    units: Decimal,
    price: Decimal,
    sale_date: NaiveDate,
    strategy: Strategy,
) -> Vec<(TradeId, Decimal)> {
    let gain_pu = |p: &CandidateParcel| price - p.remaining_cost_base / p.remaining_quantity;

    let mut order: Vec<&CandidateParcel> = parcels.iter().collect();
    let fifo = |a: &CandidateParcel, b: &CandidateParcel| {
        (a.acquisition_date, a.trade_id).cmp(&(b.acquisition_date, b.trade_id))
    };
    match strategy {
        Strategy::Fifo => order.sort_by(|a, b| fifo(a, b)),
        Strategy::MinGain => return min_gain_picks(parcels, units, price, sale_date),
        Strategy::MaxDiscount => {
            // Discount-eligible gains keep the discountable proportion at
            // 100%; losses don't enter the gain at all; a non-eligible gain
            // dilutes it — so it goes last.
            let group = |p: &CandidateParcel| {
                let g = gain_pu(p);
                if g > Decimal::ZERO && discount_eligible(p.acquisition_date, sale_date) {
                    0
                } else if g <= Decimal::ZERO {
                    1
                } else {
                    2
                }
            };
            order.sort_by(|a, b| group(a).cmp(&group(b)).then(fifo(a, b)));
        }
        Strategy::HarvestLosses => {
            order.sort_by(|a, b| {
                let (ga, gb) = (gain_pu(a), gain_pu(b));
                match (ga < Decimal::ZERO, gb < Decimal::ZERO) {
                    // Both losses: largest per-unit loss first.
                    (true, true) => ga.cmp(&gb).then(fifo(a, b)),
                    (true, false) => Ordering::Less,
                    (false, true) => Ordering::Greater,
                    (false, false) => fifo(a, b),
                }
            });
        }
    }

    let mut left = units;
    let mut picks = Vec::new();
    for p in order {
        if left <= Decimal::ZERO {
            break;
        }
        let take = p.remaining_quantity.min(left);
        picks.push((p.trade_id, take));
        left -= take;
    }
    picks
}

/// The `min_gain` allocation: the one whose own assessable contribution,
/// netted the ATO way (`net_capital_gain::net_years` — losses against the
/// non-discountable gains first, the rest against the discount-eligible gains,
/// which are then halved), is the least.
///
/// Ranking parcels per unit (a loss in full, a discount-eligible gain at half,
/// a non-discountable gain in full) is *not* that: a loss that has a
/// non-discountable gain to absorb it saves the gain in full, so while the
/// disposal's losses outweigh its non-discountable gains, a non-discountable
/// gain costs only half of itself too — and the cheaper of two such parcels can
/// be the non-discountable one. The exact answer, for `L` dollars of losses,
/// `N` of non-discountable and `D` of discount-eligible gain, minimises
///
/// ```text
/// net = ½(N + D − L) + ½·max(0, N − L)        (clamped at nil)
/// ```
///
/// Every loss unit lowers it, so the losses are taken first (largest per unit
/// first). Of the rest, each class is taken cheapest per unit first, so the
/// only choice is `k`, how many non-discountable units: the cost is convex and
/// piecewise linear in `k`, and its least value lies on a breakpoint — a
/// parcel boundary of either class, or the `k` at which the non-discountable
/// gain overtakes the losses (taken at the quantities' own decimal places,
/// either side, since the exact crossing is rarely a representable quantity).
fn min_gain_picks(
    parcels: &[CandidateParcel],
    units: Decimal,
    price: Decimal,
    sale_date: NaiveDate,
) -> Vec<(TradeId, Decimal)> {
    let gain_pu = |p: &CandidateParcel| price - p.remaining_cost_base / p.remaining_quantity;
    let by_gain = |a: &&CandidateParcel, b: &&CandidateParcel| {
        gain_pu(a)
            .cmp(&gain_pu(b))
            .then((a.acquisition_date, a.trade_id).cmp(&(b.acquisition_date, b.trade_id)))
    };
    let (mut losses, mut other, mut discounted) = (Vec::new(), Vec::new(), Vec::new());
    for p in parcels {
        if gain_pu(p) <= Decimal::ZERO {
            losses.push(p);
        } else if discount_eligible(p.acquisition_date, sale_date) {
            discounted.push(p);
        } else {
            other.push(p);
        }
    }
    losses.sort_by(by_gain);
    other.sort_by(by_gain);
    discounted.sort_by(by_gain);

    let mut picks = Vec::new();
    let mut left = units;
    let mut loss = Decimal::ZERO;
    for p in losses {
        if left <= Decimal::ZERO {
            break;
        }
        let take = p.remaining_quantity.min(left);
        picks.push((p.trade_id, take));
        loss -= gain_pu(p) * take;
        left -= take;
    }
    if left <= Decimal::ZERO {
        return picks;
    }

    // The gain of the cheapest `k` units of a class.
    let prefix_gain = |class: &[&CandidateParcel], mut k: Decimal| {
        let mut gain = Decimal::ZERO;
        for p in class {
            let take = p.remaining_quantity.min(k);
            gain += gain_pu(p) * take;
            k -= take;
        }
        gain
    };
    let total =
        |class: &[&CandidateParcel]| class.iter().map(|p| p.remaining_quantity).sum::<Decimal>();
    let lo = (left - total(&discounted)).max(Decimal::ZERO);
    let hi = left.min(total(&other));
    let scale = parcels
        .iter()
        .map(|p| p.remaining_quantity.scale())
        .chain([units.scale()])
        .max()
        .unwrap_or(0);

    let mut breakpoints = vec![lo, hi];
    let (mut units_before, mut gain_before) = (Decimal::ZERO, Decimal::ZERO);
    for p in &other {
        let g = gain_pu(p);
        if gain_before < loss && loss <= gain_before + g * p.remaining_quantity {
            let crossing = units_before + (loss - gain_before) / g;
            breakpoints.push(
                crossing.round_dp_with_strategy(scale, rust_decimal::RoundingStrategy::ToZero),
            );
            breakpoints.push(
                crossing
                    .round_dp_with_strategy(scale, rust_decimal::RoundingStrategy::AwayFromZero),
            );
        }
        units_before += p.remaining_quantity;
        gain_before += g * p.remaining_quantity;
        breakpoints.push(units_before);
    }
    let mut d_units = Decimal::ZERO;
    for p in &discounted {
        d_units += p.remaining_quantity;
        breakpoints.push(left - d_units);
    }
    let cost = |k: Decimal| {
        let n = prefix_gain(&other, k);
        n + prefix_gain(&discounted, left - k) + (n - loss).max(Decimal::ZERO)
    };
    let k = breakpoints
        .into_iter()
        .filter(|k| (lo..=hi).contains(k))
        .map(|k| (cost(k), k))
        .min()
        .map(|(_, k)| k)
        .unwrap_or(lo);

    for (class, mut take_from) in [(&other, k), (&discounted, left - k)] {
        for p in class.iter() {
            if take_from <= Decimal::ZERO {
                break;
            }
            let take = p.remaining_quantity.min(take_from);
            picks.push((p.trade_id, take));
            take_from -= take;
        }
    }
    picks
}

/// One parcel's share of a hypothetical disposal.
#[derive(utoipa::ToSchema, Debug, Clone, Serialize, Deserialize)]
pub struct HypotheticalAllocation {
    pub purchase_trade_id: TradeId,
    pub holding_account_id: HoldingAccountId,
    pub acquisition_date: NaiveDate,
    pub units: Decimal,
    /// Adjusted cost base of the allocated units, AUD.
    pub cost_base: Decimal,
    /// This allocation's share of the proceeds, AUD.
    pub proceeds: Decimal,
    /// proceeds − `cost_base` (positive = gain, negative = loss).
    pub capital_gain_loss: Decimal,
    /// Held strictly more than 12 months at the sale date.
    pub discount_eligible: bool,
}

/// A hypothetical disposal's totals, in the realised-gains report's buckets:
/// `capital_gain_loss == discount_eligible_gain + non_discountable_gain − capital_loss`.
#[derive(utoipa::ToSchema, Debug, Clone, Default, Serialize, Deserialize)]
#[schema(as = OptimiserDisposalTotals)]
pub struct DisposalTotals {
    pub proceeds: Decimal,
    pub cost_base: Decimal,
    pub capital_gain_loss: Decimal,
    /// Gross gains from parcels held strictly more than 12 months. Always ≥ 0.
    pub discount_eligible_gain: Decimal,
    /// Gross gains from parcels held 12 months or less. Always ≥ 0.
    pub non_discountable_gain: Decimal,
    /// Losses from parcels sold below cost, as a positive amount. Always ≥ 0.
    pub capital_loss: Decimal,
}

/// Per-parcel figures and totals for a hypothetical disposal of
/// `total_units` for `total_proceeds` (AUD) via `picks`. Proceeds are
/// spread by quantity as a cumulative difference, so the shares sum exactly
/// to the total (any division remainder lands on the last allocation).
///
/// Each allocation's cost base is the shared pipeline run for **that many
/// units disposed of on the candidates' sale date** — not the parcel's cost
/// base scaled down — so the figure is the one `reports::realised_gains` will
/// report once the Sell is entered. The two differ whenever an AMMA statement
/// covers part of a parcel, because how much of it reaches the sold units
/// depends on how many units are sold (`domain::contemplated_disposal`).
pub fn disposal_figures(
    candidates: &Candidates,
    picks: &[(TradeId, Decimal)],
    total_proceeds: Decimal,
    total_units: Decimal,
) -> Result<(Vec<HypotheticalAllocation>, DisposalTotals), sqlx::Error> {
    let sale_date = candidates.sale_date();
    let by_id: HashMap<TradeId, &CandidateParcel> = candidates
        .parcels()
        .iter()
        .map(|p| (p.trade_id, p))
        .collect();
    let mut allocations = Vec::with_capacity(picks.len());
    let mut totals = DisposalTotals::default();
    let mut units_so_far = Decimal::ZERO;
    let mut proceeds_so_far = Decimal::ZERO;
    for &(trade_id, units) in picks {
        let p = by_id[&trade_id];
        units_so_far += units;
        let proceeds = mul_div(&[total_proceeds, units_so_far], total_units) - proceeds_so_far;
        proceeds_so_far += proceeds;
        let cost_base = candidates.cost_base(trade_id, units)?;
        let gain = proceeds - cost_base;
        let eligible = discount_eligible(p.acquisition_date, sale_date);
        if gain > Decimal::ZERO {
            if eligible {
                totals.discount_eligible_gain += gain;
            } else {
                totals.non_discountable_gain += gain;
            }
        } else {
            totals.capital_loss -= gain;
        }
        totals.proceeds += proceeds;
        totals.cost_base += cost_base;
        totals.capital_gain_loss += gain;
        allocations.push(HypotheticalAllocation {
            purchase_trade_id: trade_id,
            holding_account_id: p.holding_account_id,
            acquisition_date: p.acquisition_date,
            units,
            cost_base,
            proceeds,
            capital_gain_loss: gain,
            discount_eligible: eligible,
        });
    }
    Ok((allocations, totals))
}

// ---------------------------------------------------------------------------
// HTTP API
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize, utoipa::IntoParams)]
#[serde(deny_unknown_fields)]
pub struct OptimiserRequest {
    pub listing_id: ListingId,
    /// The account the contemplated Sell would happen in — a real Sell's
    /// allocations may only consume its own account's parcels.
    pub holding_account_id: HoldingAccountId,
    #[serde(deserialize_with = "crate::infra::decimal::strict_decimal")]
    pub units: Decimal,
    /// Defaults to today.
    #[serde(default)]
    pub sale_date: Option<NaiveDate>,
    /// Per-unit price in AUD. Absent → live-fetched from the price source
    /// (the live-valuation rules); an explicit price wins.
    #[serde(
        default,
        deserialize_with = "crate::infra::decimal::strict_optional_decimal"
    )]
    pub price: Option<Decimal>,
}

/// One strategy's candidate: its totals, with the per-parcel rows in the
/// response's flat `allocations` list (keyed back by `strategy`).
#[derive(utoipa::ToSchema, Debug, Serialize, Deserialize)]
pub struct StrategySummary {
    pub strategy: Strategy,
    #[serde(flatten)]
    pub totals: DisposalTotals,
}

#[derive(utoipa::ToSchema, Debug, Serialize, Deserialize)]
pub struct StrategyAllocation {
    pub strategy: Strategy,
    #[serde(flatten)]
    pub allocation: HypotheticalAllocation,
}

#[derive(utoipa::ToSchema, Debug, Serialize, Deserialize)]
pub struct OptimiserResponse {
    pub listing_id: ListingId,
    pub holding_account_id: HoldingAccountId,
    pub units: Decimal,
    pub sale_date: NaiveDate,
    /// The per-unit AUD price the candidates were valued at.
    pub price: Decimal,
    /// The price source's quote timestamp when `price` came from a live
    /// fetch; absent for an explicitly supplied price.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub price_as_of: Option<String>,
    pub strategies: Vec<StrategySummary>,
    pub allocations: Vec<StrategyAllocation>,
    /// Informational: the taxpayer assumption behind the hard-wired rates
    /// (always [`crate::reports::taxpayer::TAXPAYER_BASIS`]) — the 50% discount the
    /// strategies weight discount-eligible gains at is the
    /// Australian-resident-individual rate; other entity types are not
    /// modelled. Stated once for the whole response rather than repeated per
    /// strategy row because it is not a property of any one candidate: it is
    /// the basis the candidates are *ranked against each other* on, so on a
    /// different basis (a company, a super fund at 33⅓%, a non-resident) the
    /// recommendation itself — not merely a figure on it — would differ.
    pub taxpayer_basis: String,
}

pub fn router() -> OpenApiRouter<SqlitePool> {
    OpenApiRouter::new().routes(routes!(optimiser_handler))
}

#[utoipa::path(
    get,
    path = "/portfolio/parcel_optimiser",
    summary = "Which parcels to sell for a target, under a chosen strategy; ?listing_id=, ?units=, ?sale_date=, ?price= and ?holding_account_id= drive it.",
    params(OptimiserRequest)
)]
async fn optimiser_handler(
    State(pool): State<SqlitePool>,
    fetcher: Option<Extension<SharedFetcher>>,
    Query(req): Query<OptimiserRequest>,
) -> Result<Json<OptimiserResponse>, ApiError> {
    if req.units <= Decimal::ZERO {
        return Err(ApiError::Unprocessable("units must be positive".to_owned()));
    }
    // The contemplated sale's own date, defaulting to today through the shared
    // resolver rather than a fourth inline `Local::now()` (the date-scoped twin
    // of `infra::date::as_of_or_open`).
    let sale_date = date::as_of_or_today(req.sale_date);

    // Candidates as at the contemplated sale's own date, not today: a parcel
    // acquired after it can't be sold on it, and one sold since was still
    // there to sell. `units` and `price` are read in that date's unit basis,
    // which is the basis the candidates come back in.
    let candidates = db_candidate_parcels(
        &pool,
        req.listing_id,
        Some(req.holding_account_id),
        sale_date,
    )
    .await
    .map_err(ApiError::from)?;
    let open: Decimal = candidates
        .parcels()
        .iter()
        .map(|p| p.remaining_quantity)
        .sum();
    if req.units > open {
        return Err(ApiError::Unprocessable(format!(
            "only {open} unit(s) of {} are open in {}",
            labels::listing_label(&pool, req.listing_id).await?,
            labels::account_label(&pool, req.holding_account_id).await?
        )));
    }

    // Price: explicit wins; otherwise the live-valuation rules (latest quote
    // converted to AUD). No price means the candidates can't be valued, so —
    // unlike the valuation reports, which leave a row unvalued — this report
    // rejects with the reason.
    let (price, price_as_of) = match req.price {
        Some(p) => (p, None),
        None => {
            let live = closing_price::resolve_live_prices(
                &pool,
                fetcher.as_ref().map(|f| f.0.as_ref()),
                closing_price::LiveQuotes::Fetch,
                &HashMap::new(),
                [req.listing_id],
            )
            .await
            .map_err(ApiError::from)?;
            match live.get(&req.listing_id) {
                Some(Ok(v)) => (v.aud_price.amount(), Some(v.as_of.clone())),
                Some(Err(reason)) => {
                    return Err(ApiError::Unprocessable(format!(
                        "no live price for {}: {reason} — supply a price",
                        labels::listing_label(&pool, req.listing_id).await?
                    )));
                }
                None => {
                    return Err(ApiError::Unprocessable(format!(
                        "no live price for {} — supply a price",
                        labels::listing_label(&pool, req.listing_id).await?
                    )));
                }
            }
        }
    };

    let total_proceeds = price * req.units;
    let mut strategies = Vec::new();
    let mut allocations = Vec::new();
    for strategy in ALL_STRATEGIES {
        let picks = allocate_strategy(&candidates, req.units, price, strategy);
        let (allocs, totals) = disposal_figures(&candidates, &picks, total_proceeds, req.units)
            .map_err(ApiError::from)?;
        strategies.push(StrategySummary { strategy, totals });
        allocations.extend(allocs.into_iter().map(|allocation| StrategyAllocation {
            strategy,
            allocation,
        }));
    }

    Ok(Json(OptimiserResponse {
        listing_id: req.listing_id,
        holding_account_id: req.holding_account_id,
        units: req.units,
        sale_date,
        price,
        price_as_of,
        strategies,
        allocations,
        taxpayer_basis: taxpayer::TAXPAYER_BASIS.to_owned(),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::entities::corporate_action::{ActionKind, CorporateAction};
    use crate::entities::{corporate_action, sell};
    use crate::infra::date;
    use crate::reports::net_capital_gain::WhatIfResponse;
    use crate::reports::realised_gains::{self, DisposalId, RealisedGainLoss};
    use crate::test_support::{self, ApiClient, allocate, dec, test_pool, ymd};
    use axum::http::StatusCode;
    use std::collections::BTreeSet;

    async fn insert_listing(pool: &SqlitePool, id: i64, ticker: &str) {
        test_support::listing(id)
            .ticker(ticker)
            .name(ticker)
            .insert(pool)
            .await;
    }

    /// Brokerage-free Buy so the per-unit cost base is exactly the price.
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

    /// The four-strategies fixture (sale 2026-06-15 at $10/unit):
    ///  - parcel 1: acq 2023-01-01, 100 u @ $5  → gain +5/u, eligible (assessable 2.5)
    ///  - parcel 2: acq 2024-01-01, 100 u @ $12 → loss −2/u
    ///  - parcel 3: acq 2025-01-01, 100 u @ $8  → gain +2/u, eligible (assessable 1)
    ///  - parcel 4: acq 2026-03-01, 100 u @ $4  → gain +6/u, NOT eligible (assessable 6)
    async fn strategy_fixture(pool: &SqlitePool) {
        insert_listing(pool, 1, "OPT").await;
        for (id, date, price) in [
            (1, ymd(2023, 1, 3), "5"),
            (2, ymd(2024, 1, 2), "12"),
            (3, ymd(2025, 1, 2), "8"),
            (4, ymd(2026, 3, 2), "4"),
        ] {
            insert_buy(pool, id, ListingId(1), date, Decimal::from(100), dec(price)).await;
        }
    }

    fn fixture_sale() -> (Decimal, Decimal, NaiveDate) {
        (Decimal::from(150), Decimal::from(10), ymd(2026, 6, 15))
    }

    /// The fixture's candidates as at the fixture sale's own date — the date
    /// every figure derived from them is costed on.
    async fn fixture_candidates(pool: &SqlitePool) -> Candidates {
        let (_, _, sale_date) = fixture_sale();
        db_candidate_parcels(pool, ListingId(1), None, sale_date)
            .await
            .unwrap()
    }

    async fn fixture_picks(pool: &SqlitePool, strategy: Strategy) -> Vec<(TradeId, Decimal)> {
        let candidates = fixture_candidates(pool).await;
        let (units, price, _) = fixture_sale();
        allocate_strategy(&candidates, units, price, strategy)
    }

    // ---- strategy allocation choices ------------------------------------

    #[tokio::test]
    async fn fifo_takes_oldest_parcels_first() {
        let pool = test_pool().await;
        strategy_fixture(&pool).await;
        let picks = fixture_picks(&pool, Strategy::Fifo).await;
        assert_eq!(
            picks,
            vec![
                (TradeId(1), Decimal::from(100)),
                (TradeId(2), Decimal::from(50))
            ]
        );
    }

    #[tokio::test]
    async fn harvest_losses_takes_loss_parcels_then_fifo() {
        let pool = test_pool().await;
        strategy_fixture(&pool).await;
        // Parcel 2 is the only loss; the remainder follows FIFO (parcel 1).
        let picks = fixture_picks(&pool, Strategy::HarvestLosses).await;
        assert_eq!(
            picks,
            vec![
                (TradeId(2), Decimal::from(100)),
                (TradeId(1), Decimal::from(50))
            ]
        );
    }

    #[tokio::test]
    async fn min_gain_orders_by_assessable_contribution() {
        let pool = test_pool().await;
        strategy_fixture(&pool).await;
        // Assessable per unit: parcel 2 (−2) < parcel 3 (+2 eligible → 1)
        // < parcel 1 (+5 eligible → 2.5) < parcel 4 (+6 not eligible → 6).
        let picks = fixture_picks(&pool, Strategy::MinGain).await;
        assert_eq!(
            picks,
            vec![
                (TradeId(2), Decimal::from(100)),
                (TradeId(3), Decimal::from(50))
            ]
        );
    }

    #[tokio::test]
    async fn max_discount_takes_eligible_gain_parcels_first() {
        let pool = test_pool().await;
        strategy_fixture(&pool).await;
        // Eligible gain parcels in FIFO order: parcel 1 then parcel 3 —
        // unlike FIFO, the loss parcel 2 is passed over.
        let picks = fixture_picks(&pool, Strategy::MaxDiscount).await;
        assert_eq!(
            picks,
            vec![
                (TradeId(1), Decimal::from(100)),
                (TradeId(3), Decimal::from(50))
            ]
        );
    }

    /// The optimiser classifies on the shared ownership rule, so the boundary
    /// it applies is `domain::cgt_discount`'s (pinned by its own test) rather
    /// than one of its own. Asserted here through `allocate_parcels` so the
    /// wiring — not just the rule — is covered: a parcel one day short of 12
    /// months is a non-discountable gain and so sorts last under `MaxDiscount`.
    #[test]
    fn max_discount_treats_a_parcel_one_day_short_as_non_discountable() {
        let parcel = |trade_id, acquired| CandidateParcel {
            trade_id,
            holding_account_id: HoldingAccountId(1),
            acquisition_date: acquired,
            remaining_quantity: Decimal::from(10),
            remaining_cost_base: Decimal::from(100),
        };
        // Both parcels stand at a gain (price 20 vs cost 10/unit); only the
        // older one has cleared 12 months as at the sale date.
        let sale_date = ymd(2026, 6, 15);
        let parcels = vec![
            parcel(TradeId(1), ymd(2025, 6, 15)),
            parcel(TradeId(2), ymd(2025, 6, 14)),
        ];
        let picks = allocate_parcels(
            &parcels,
            Decimal::from(20),
            Decimal::from(20),
            sale_date,
            Strategy::MaxDiscount,
        );
        assert_eq!(
            picks,
            vec![
                (TradeId(2), Decimal::from(10)),
                (TradeId(1), Decimal::from(10))
            ]
        );
    }

    /// The allocation orders the parcels itself, so no caller has to hand it
    /// a FIFO-sorted slice: parcels arriving newest-first are allocated
    /// exactly as the same parcels arriving oldest-first, under every
    /// strategy — including where the strategy's own key ties and FIFO
    /// (acquisition date, then trade id) is what decides.
    #[test]
    fn allocation_does_not_depend_on_the_order_the_parcels_arrive_in() {
        let parcel = |trade_id, acquired, cost| CandidateParcel {
            trade_id: TradeId(trade_id),
            holding_account_id: HoldingAccountId(1),
            acquisition_date: acquired,
            remaining_quantity: Decimal::from(10),
            remaining_cost_base: Decimal::from(cost),
        };
        let sale_date = ymd(2026, 6, 15);
        // Two same-cost gains (one discountable, one not), two equal losses
        // on the same date, and a gain identical to the first but younger.
        let fifo_sorted = vec![
            parcel(1, ymd(2020, 1, 1), 100),
            parcel(2, ymd(2021, 3, 1), 300),
            parcel(3, ymd(2021, 3, 1), 300),
            parcel(4, ymd(2022, 1, 1), 100),
            parcel(5, ymd(2026, 1, 1), 100),
        ];
        let reversed: Vec<_> = fifo_sorted.iter().rev().cloned().collect();
        for strategy in ALL_STRATEGIES {
            let (units, price) = (Decimal::from(35), Decimal::from(20));
            let expected = allocate_parcels(&fifo_sorted, units, price, sale_date, strategy);
            assert_eq!(
                allocate_parcels(&reversed, units, price, sale_date, strategy),
                expected,
                "{strategy:?}"
            );
        }
        assert_eq!(
            allocate_parcels(
                &reversed,
                Decimal::from(15),
                Decimal::from(20),
                sale_date,
                Strategy::Fifo
            ),
            vec![
                (TradeId(1), Decimal::from(10)),
                (TradeId(2), Decimal::from(5))
            ]
        );
    }

    // ---- disposal figures ------------------------------------------------

    #[tokio::test]
    async fn disposal_totals_split_gains_losses_and_sum_exactly() {
        let pool = test_pool().await;
        strategy_fixture(&pool).await;
        let candidates = fixture_candidates(&pool).await;
        let (units, price, _) = fixture_sale();
        let picks = allocate_strategy(&candidates, units, price, Strategy::Fifo);
        let (allocs, totals) = disposal_figures(&candidates, &picks, price * units, units).unwrap();
        // Parcel 1: 100 u, proceeds 1000, cost 500 → +500 eligible.
        // Parcel 2: 50 u, proceeds 500, cost 600 → −100 loss.
        assert_eq!(allocs.len(), 2);
        assert_eq!(allocs[0].proceeds, Decimal::from(1000));
        assert_eq!(allocs[0].cost_base, Decimal::from(500));
        assert_eq!(allocs[0].capital_gain_loss, Decimal::from(500));
        assert!(allocs[0].discount_eligible);
        assert_eq!(allocs[1].capital_gain_loss, Decimal::from(-100));
        assert!(allocs[1].discount_eligible); // held > 12mo — a loss is just never discounted
        assert_eq!(totals.proceeds, Decimal::from(1500));
        assert_eq!(totals.cost_base, Decimal::from(1100));
        assert_eq!(totals.capital_gain_loss, Decimal::from(400));
        assert_eq!(totals.discount_eligible_gain, Decimal::from(500));
        assert_eq!(totals.non_discountable_gain, Decimal::ZERO);
        assert_eq!(totals.capital_loss, Decimal::from(100));
    }

    /// Discount eligibility is a property of the *parcel*, so it can never
    /// vary within one: every unit of a parcel shares its acquisition date.
    /// Splitting an allocation — taking 30 of a 100-unit eligible parcel and
    /// 20 of a 100-unit ineligible one — therefore yields one flag per
    /// allocation and puts each allocation's whole gain in a single bucket,
    /// with nothing pro-rated across the boundary (SCENARIOS C-16).
    #[tokio::test]
    async fn a_partly_consumed_parcel_carries_one_eligibility_flag() {
        let sale_date = ymd(2026, 6, 15);
        let pool = test_pool().await;
        insert_listing(&pool, 1, "OPT").await;
        // $1/unit, so a $2/unit sale is a clean $1/unit gain.
        insert_buy(
            &pool,
            1,
            ListingId(1),
            ymd(2023, 1, 3),
            Decimal::from(100),
            Decimal::ONE,
        )
        .await;
        insert_buy(
            &pool,
            2,
            ListingId(1),
            ymd(2026, 3, 2),
            Decimal::from(100),
            Decimal::ONE,
        )
        .await;
        let candidates = db_candidate_parcels(&pool, ListingId(1), None, sale_date)
            .await
            .unwrap();
        let picks = vec![
            (TradeId(1), Decimal::from(30)),
            (TradeId(2), Decimal::from(20)),
        ];
        let (allocs, totals) = disposal_figures(
            &candidates,
            &picks,
            Decimal::from(100), // 50 units at $2
            Decimal::from(50),
        )
        .unwrap();

        assert_eq!(allocs.len(), 2, "one allocation per parcel, not per unit");
        assert!(allocs[0].discount_eligible);
        assert_eq!(allocs[0].capital_gain_loss, Decimal::from(30));
        assert!(!allocs[1].discount_eligible);
        assert_eq!(allocs[1].capital_gain_loss, Decimal::from(20));
        // Each parcel's gain lands whole in its own bucket.
        assert_eq!(totals.discount_eligible_gain, Decimal::from(30));
        assert_eq!(totals.non_discountable_gain, Decimal::from(20));
        assert_eq!(totals.capital_loss, Decimal::ZERO);
        assert_eq!(totals.capital_gain_loss, Decimal::from(50));
    }

    /// Proceeds spread by cumulative difference sum exactly to the total
    /// even when the division doesn't terminate (1000/3 per unit).
    #[tokio::test]
    async fn disposal_proceeds_sum_exactly_to_the_total() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "OPT").await;
        for id in 1..=3 {
            insert_buy(
                &pool,
                id,
                ListingId(1),
                ymd(2023, 1, 3),
                Decimal::ONE,
                Decimal::from(100),
            )
            .await;
        }
        let candidates = db_candidate_parcels(&pool, ListingId(1), None, ymd(2026, 1, 1))
            .await
            .unwrap();
        let picks: Vec<(TradeId, Decimal)> =
            (1..=3).map(|id| (TradeId(id), Decimal::ONE)).collect();
        let total = Decimal::from(1000);
        let (allocs, totals) =
            disposal_figures(&candidates, &picks, total, Decimal::from(3)).unwrap();
        let sum: Decimal = allocs.iter().map(|a| a.proceeds).sum();
        assert_eq!(sum, total);
        assert_eq!(totals.proceeds, total);
    }

    // ---- candidate parcels -------------------------------------------------

    #[tokio::test]
    async fn candidates_filter_by_listing_and_account_and_skip_sold_parcels() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "AAA").await;
        insert_listing(&pool, 2, "BBB").await;
        insert_buy(
            &pool,
            1,
            ListingId(1),
            ymd(2024, 1, 2),
            Decimal::from(100),
            dec("5"),
        )
        .await;
        insert_buy(
            &pool,
            2,
            ListingId(2),
            ymd(2024, 1, 2),
            Decimal::from(100),
            dec("5"),
        )
        .await;
        // Fully sell parcel 1's sibling parcel 3 so it drops out.
        insert_buy(
            &pool,
            3,
            ListingId(1),
            ymd(2024, 2, 1),
            Decimal::from(40),
            dec("5"),
        )
        .await;
        test_support::sell(4, 1)
            .date(ymd(2025, 6, 2))
            .qty(Decimal::from(40))
            .price(Decimal::from(6))
            .insert(&pool)
            .await;
        allocate(&pool, 1, 4, 3, Decimal::from(40)).await;

        let today = date::today();
        let parcels = db_candidate_parcels(&pool, ListingId(1), Some(HoldingAccountId(1)), today)
            .await
            .unwrap();
        assert_eq!(parcels.parcels().len(), 1);
        assert_eq!(parcels.parcels()[0].trade_id, 1);
        // Wrong account: nothing.
        let parcels = db_candidate_parcels(&pool, ListingId(1), Some(HoldingAccountId(99)), today)
            .await
            .unwrap();
        assert!(parcels.parcels().is_empty());
    }

    /// The as-at fixture (sale at $10/unit), spanning a past sale date of
    /// 2023-06-15:
    ///  - parcel 1: acq 2020-01-01, 100 u @ $12 → loss −2/u
    ///  - parcel 2: acq 2021-01-01, 100 u @ $5  → gain +5/u, eligible then
    ///  - parcel 3: acq 2023-01-01, 100 u @ $4  → gain +6/u, NOT eligible then
    ///  - parcel 4: acq 2025-01-01, 100 u @ $8  → acquired *after* that date
    async fn as_at_fixture(pool: &SqlitePool) {
        insert_listing(pool, 1, "AST").await;
        for (id, date, price) in [
            (1, ymd(2020, 1, 2), "12"),
            (2, ymd(2021, 1, 4), "5"),
            (3, ymd(2023, 1, 3), "4"),
            (4, ymd(2025, 1, 2), "8"),
        ] {
            insert_buy(pool, id, ListingId(1), date, Decimal::from(100), dec(price)).await;
        }
    }

    /// SCENARIOS O-15. The candidates are the parcels open **as at the sale
    /// date**, not today: a parcel acquired after the contemplated sale can't
    /// be sold on it (the Sell path refuses exactly that allocation), so it is
    /// not offered — and with it gone the discount clock runs forward again
    /// and the four strategies stop collapsing onto one answer.
    #[tokio::test]
    async fn api_past_dated_request_excludes_parcels_acquired_after_it() {
        let pool = test_pool().await;
        as_at_fixture(&pool).await;
        let (status, body) = get_optimiser(
            pool,
            None,
            serde_json::json!({
                "listing_id": 1, "holding_account_id": 1, "units": "150",
                "sale_date": "2023-06-15", "price": "10"
            }),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let r: OptimiserResponse = serde_json::from_slice(&body).unwrap();

        // Parcel 4 (acquired 2025-01-01) did not exist on 2023-06-15.
        assert!(
            r.allocations
                .iter()
                .all(|a| a.allocation.purchase_trade_id != 4),
            "a parcel acquired after the sale date was offered"
        );

        let picks = |strategy: Strategy| -> Vec<(TradeId, Decimal)> {
            r.allocations
                .iter()
                .filter(|a| a.strategy == strategy)
                .map(|a| (a.allocation.purchase_trade_id, a.allocation.units))
                .collect()
        };
        // FIFO opens on the oldest parcel (the loss); max_discount takes the
        // discount-eligible *gain* parcel first — the two candidates differ,
        // where reading today's parcels made every strategy identical.
        assert_eq!(
            picks(Strategy::Fifo),
            vec![
                (TradeId(1), Decimal::from(100)),
                (TradeId(2), Decimal::from(50))
            ]
        );
        assert_eq!(
            picks(Strategy::MaxDiscount),
            vec![
                (TradeId(2), Decimal::from(100)),
                (TradeId(1), Decimal::from(50))
            ]
        );

        // Held from 2021-01-01 to 2023-06-15 — over 12 months, so the gain is
        // discountable. The bug classified every parcel non-discountable.
        let eligible = r
            .allocations
            .iter()
            .find(|a| a.strategy == Strategy::MaxDiscount && a.allocation.purchase_trade_id == 2)
            .unwrap();
        assert!(eligible.allocation.discount_eligible);
        let max_discount = r
            .strategies
            .iter()
            .find(|s| s.strategy == Strategy::MaxDiscount)
            .unwrap();
        assert_eq!(
            max_discount.totals.discount_eligible_gain,
            Decimal::from(500)
        );
        assert_eq!(max_discount.totals.non_discountable_gain, Decimal::ZERO);
    }

    /// Parcel 3 was acquired 2023-01-01, less than 12 months before the
    /// 2023-06-15 sale date: an as-at read must still classify it as the
    /// non-discountable gain it is, not drop the distinction.
    #[tokio::test]
    async fn api_past_dated_request_classifies_a_short_held_parcel_as_non_discountable() {
        let pool = test_pool().await;
        as_at_fixture(&pool).await;
        let (status, body) = get_optimiser(
            pool,
            None,
            serde_json::json!({
                "listing_id": 1, "holding_account_id": 1, "units": "300",
                "sale_date": "2023-06-15", "price": "10"
            }),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let r: OptimiserResponse = serde_json::from_slice(&body).unwrap();
        let flags: HashMap<i64, bool> = r
            .allocations
            .iter()
            .filter(|a| a.strategy == Strategy::Fifo)
            .map(|a| {
                (
                    a.allocation.purchase_trade_id.0,
                    a.allocation.discount_eligible,
                )
            })
            .collect();
        assert_eq!(flags.get(&1), Some(&true));
        assert_eq!(flags.get(&2), Some(&true));
        assert_eq!(flags.get(&3), Some(&false));
        assert_eq!(flags.get(&4), None);
    }

    /// The over-request bound moves with the date: only 300 of the 400 units
    /// held today were open on 2023-06-15.
    #[tokio::test]
    async fn api_past_dated_open_quantity_is_what_was_open_then() {
        let pool = test_pool().await;
        as_at_fixture(&pool).await;
        let (status, body) = get_optimiser(
            pool,
            None,
            serde_json::json!({
                "listing_id": 1, "holding_account_id": 1, "units": "301",
                "sale_date": "2023-06-15", "price": "10"
            }),
        )
        .await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
        let msg = String::from_utf8(body).unwrap();
        assert!(msg.contains("only 300"), "{msg}");
    }

    /// The boundary the Sell path draws: a parcel acquired **on** the sale
    /// date is legitimate, and stays a candidate.
    #[tokio::test]
    async fn api_a_parcel_acquired_on_the_sale_date_is_still_a_candidate() {
        let pool = test_pool().await;
        as_at_fixture(&pool).await;
        let (status, body) = get_optimiser(
            pool,
            None,
            serde_json::json!({
                "listing_id": 1, "holding_account_id": 1, "units": "400",
                "sale_date": "2025-01-02", "price": "10"
            }),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let r: OptimiserResponse = serde_json::from_slice(&body).unwrap();
        assert!(
            r.allocations
                .iter()
                .any(|a| a.allocation.purchase_trade_id == 4),
            "the same-day parcel was dropped"
        );
    }

    /// The other direction of the same read: a parcel sold *since* the sale
    /// date was there to be sold on it, so a past-dated request offers it.
    #[tokio::test]
    async fn api_past_dated_request_offers_a_parcel_sold_since() {
        let pool = test_pool().await;
        as_at_fixture(&pool).await;
        // Parcel 2 is fully sold in 2024 — after the 2023-06-15 sale date.
        test_support::sell(5, 1)
            .date(ymd(2024, 3, 1))
            .qty(Decimal::from(100))
            .price(Decimal::from(11))
            .insert(&pool)
            .await;
        allocate(&pool, 1, 5, 2, Decimal::from(100)).await;

        // Today it is gone…
        let live = db_candidate_parcels(&pool, ListingId(1), None, date::today())
            .await
            .unwrap();
        assert!(live.parcels().iter().all(|p| p.trade_id != 2));

        // …but it was open on 2023-06-15, so a request for that date has it.
        let (status, body) = get_optimiser(
            pool,
            None,
            serde_json::json!({
                "listing_id": 1, "holding_account_id": 1, "units": "300",
                "sale_date": "2023-06-15", "price": "10"
            }),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let r: OptimiserResponse = serde_json::from_slice(&body).unwrap();
        assert!(
            r.allocations
                .iter()
                .any(|a| a.allocation.purchase_trade_id == 2),
            "a parcel open at the sale date but sold since was under-reported"
        );
    }

    /// Quantities come back in the unit basis of the sale date — the project's
    /// as-at convention (`docs/API.md`) — so the units the caller states and
    /// the units the candidates report are on one basis: a 2-for-1 split in
    /// 2025 does not restate what was held in 2024.
    #[tokio::test]
    async fn api_past_dated_candidates_are_in_that_dates_unit_basis() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "SPL").await;
        insert_buy(
            &pool,
            1,
            ListingId(1),
            ymd(2020, 1, 2),
            Decimal::from(100),
            dec("5"),
        )
        .await;
        corporate_action::db_upsert(
            &pool,
            &CorporateAction {
                id: 1,
                listing_id: ListingId(1),
                date: ymd(2025, 1, 2),
                kind: ActionKind::ShareSplit {
                    split_new_units: Decimal::from(2),
                    split_old_units: Decimal::ONE,
                },
            },
        )
        .await
        .unwrap();

        let before = db_candidate_parcels(&pool, ListingId(1), None, ymd(2024, 6, 30))
            .await
            .unwrap();
        assert_eq!(before.parcels()[0].remaining_quantity, Decimal::from(100));
        let after = db_candidate_parcels(&pool, ListingId(1), None, date::today())
            .await
            .unwrap();
        assert_eq!(after.parcels()[0].remaining_quantity, Decimal::from(200));
        // The split never touches the cost base (TD 2000/10).
        assert_eq!(
            before.parcels()[0].remaining_cost_base,
            after.parcels()[0].remaining_cost_base
        );
    }

    /// A future-dated request is unchanged: every parcel open today is a
    /// legitimate candidate for a sale contemplated later, and the answer
    /// matches the same request run for today.
    #[tokio::test]
    async fn api_future_dated_request_still_sees_every_open_parcel() {
        let pool = test_pool().await;
        as_at_fixture(&pool).await;
        let today = date::today();
        let later = today + chrono::Duration::days(400);
        let run = async |date: NaiveDate| {
            let (status, body) = get_optimiser(
                pool.clone(),
                None,
                serde_json::json!({
                    "listing_id": 1, "holding_account_id": 1, "units": "400",
                    "sale_date": date.to_string(), "price": "10"
                }),
            )
            .await;
            assert_eq!(status, StatusCode::OK);
            serde_json::from_slice::<OptimiserResponse>(&body).unwrap()
        };
        let now = run(today).await;
        let future = run(later).await;
        // All four parcels, both times — the whole 400 units held today.
        for r in [&now, &future] {
            let ids: BTreeSet<i64> = r
                .allocations
                .iter()
                .filter(|a| a.strategy == Strategy::Fifo)
                .map(|a| a.allocation.purchase_trade_id.0)
                .collect();
            assert_eq!(ids, (1..=4).collect::<BTreeSet<i64>>());
        }
        let totals = |r: &OptimiserResponse| {
            r.strategies
                .iter()
                .map(|s| (s.totals.cost_base, s.totals.capital_gain_loss))
                .collect::<Vec<_>>()
        };
        assert_eq!(totals(&now), totals(&future));
    }

    // ---- API ---------------------------------------------------------------

    /// The report's request is a `GET`+query, so a test's parameter object
    /// maps one-for-one onto `?key=value`. Every value is a string or a
    /// number; a string is used verbatim, so a decimal like `"150"` stays the
    /// decimal string the request parses rather than becoming a JSON number.
    fn query_string(body: &serde_json::Value) -> String {
        body.as_object()
            .expect("a query-parameter object")
            .iter()
            .map(|(k, v)| match v {
                serde_json::Value::String(s) => format!("{k}={s}"),
                other => format!("{k}={other}"),
            })
            .collect::<Vec<_>>()
            .join("&")
    }

    async fn get_optimiser(
        pool: SqlitePool,
        fetcher: Option<SharedFetcher>,
        body: serde_json::Value,
    ) -> (StatusCode, Vec<u8>) {
        let mut router = router().with_state(pool);
        if let Some(f) = fetcher {
            router = router.layer(Extension(f));
        }
        let resp = ApiClient::over(router)
            .get(format!(
                "/portfolio/parcel_optimiser?{}",
                query_string(&body)
            ))
            .await;
        let status = resp.status;
        (status, resp.body.to_vec())
    }

    #[tokio::test]
    async fn api_explicit_price_returns_all_strategies() {
        let pool = test_pool().await;
        strategy_fixture(&pool).await;
        let (status, body) = get_optimiser(
            pool,
            None,
            serde_json::json!({
                "listing_id": 1, "holding_account_id": 1, "units": "150",
                "sale_date": "2026-06-15", "price": "10"
            }),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let r: OptimiserResponse = serde_json::from_slice(&body).unwrap();
        assert_eq!(r.price, Decimal::from(10));
        assert!(r.price_as_of.is_none());
        assert_eq!(r.strategies.len(), 4);
        // The harvest-losses candidate realises the parcel-2 loss.
        let harvest = r
            .strategies
            .iter()
            .find(|s| s.strategy == Strategy::HarvestLosses)
            .unwrap();
        assert_eq!(harvest.totals.capital_loss, Decimal::from(200));
        assert_eq!(harvest.totals.discount_eligible_gain, Decimal::from(250));
        // Each strategy's allocations are keyed back by strategy and sum to the units.
        for s in ALL_STRATEGIES {
            let total: Decimal = r
                .allocations
                .iter()
                .filter(|a| a.strategy == s)
                .map(|a| a.allocation.units)
                .sum();
            assert_eq!(total, Decimal::from(150));
        }
    }

    /// SCENARIOS P-12: the strategies are *ranked against each other* on the
    /// 50% discount, so the response states the taxpayer basis that ranking
    /// assumes — once for the whole response, since it governs the comparison
    /// between the candidates rather than any one of them.
    #[tokio::test]
    async fn api_optimiser_states_the_taxpayer_basis_behind_its_ranking() {
        let pool = test_pool().await;
        strategy_fixture(&pool).await;
        let (status, body) = get_optimiser(
            pool,
            None,
            serde_json::json!({
                "listing_id": 1, "holding_account_id": 1, "units": "150",
                "sale_date": "2026-06-15", "price": "10"
            }),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let r: OptimiserResponse = serde_json::from_slice(&body).unwrap();
        assert_eq!(r.taxpayer_basis, taxpayer::TAXPAYER_BASIS);
    }

    #[tokio::test]
    async fn api_live_price_used_when_none_supplied() {
        use crate::entities::closing_price::test_support::QuoteStub;
        let pool = test_pool().await;
        strategy_fixture(&pool).await;
        let as_of = chrono::TimeZone::with_ymd_and_hms(&chrono::Utc, 2026, 6, 5, 6, 30, 0).unwrap();
        let fetcher = QuoteStub::default()
            .with_quote(1, "10.00", "AUD", as_of)
            .shared();
        let (status, body) = get_optimiser(
            pool,
            Some(fetcher),
            serde_json::json!({
                "listing_id": 1, "holding_account_id": 1, "units": "150",
                "sale_date": "2026-06-15"
            }),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let r: OptimiserResponse = serde_json::from_slice(&body).unwrap();
        assert_eq!(r.price, dec("10.00"));
        assert_eq!(r.price_as_of.as_deref(), Some(as_of.to_rfc3339().as_str()));
    }

    #[tokio::test]
    async fn api_no_price_obtainable_rejected_with_reason() {
        use crate::entities::closing_price::test_support::QuoteStub;
        let pool = test_pool().await;
        strategy_fixture(&pool).await;
        let fetcher = QuoteStub::failing("provider down").shared();
        let (status, body) = get_optimiser(
            pool,
            Some(fetcher),
            serde_json::json!({ "listing_id": 1, "holding_account_id": 1, "units": "10" }),
        )
        .await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
        let msg = String::from_utf8(body).unwrap();
        assert!(msg.contains("provider down"), "{msg}");
    }

    #[tokio::test]
    async fn api_units_exceeding_open_quantity_rejected() {
        let pool = test_pool().await;
        strategy_fixture(&pool).await;
        let (status, body) = get_optimiser(
            pool,
            None,
            serde_json::json!({
                "listing_id": 1, "holding_account_id": 1, "units": "401", "price": "10"
            }),
        )
        .await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
        let msg = String::from_utf8(body).unwrap();
        assert!(msg.contains("only 400"), "{msg}");
        // The rejection names the listing and account, never raw ids.
        assert!(msg.contains("OPT"), "{msg}");
        assert!(msg.contains("account 'Default'"), "{msg}");
    }

    /// SCENARIOS D-18. What the optimiser proposes, the Sell endpoint must
    /// accept: each strategy's allocations, submitted verbatim as a Sell's
    /// `allocations`, pass every write-time invariant — so the report can be
    /// acted on by copying it, not by re-deriving it.
    #[tokio::test]
    async fn every_strategys_allocations_are_accepted_verbatim_by_the_sell_endpoint() {
        let pool = test_pool().await;
        strategy_fixture(&pool).await;
        let (status, body) = get_optimiser(
            pool.clone(),
            None,
            serde_json::json!({
                "listing_id": 1, "holding_account_id": 1, "units": "150",
                "sale_date": "2026-06-15", "price": "10"
            }),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let resp: OptimiserResponse = serde_json::from_slice(&body).unwrap();

        let sells = ApiClient::over(sell::router().with_state(pool.clone()));
        for strategy in ALL_STRATEGIES {
            let allocations: Vec<_> = resp
                .allocations
                .iter()
                .filter(|a| a.strategy == strategy)
                .map(|a| {
                    serde_json::json!({
                        "purchase_trade_id": a.allocation.purchase_trade_id,
                        "quantity_allocated": a.allocation.units,
                    })
                })
                .collect();
            assert!(!allocations.is_empty(), "{strategy:?} proposed nothing");
            let resp = sells
                .put(
                    "/sells/50",
                    &serde_json::json!({
                        "date": "2026-06-15",
                        "listing_id": 1,
                        "average_price": "10",
                        "quantity": "150",
                        "currency": "AUD",
                        "brokerage": "0",
                        "brokerage_currency": "AUD",
                        "fx_rate": "1",
                        "holding_account_id": 1,
                        "allocations": allocations,
                    }),
                )
                .await;
            // The same id is re-PUT per strategy: the first creates it (201,
            // with the created row) and every later one replaces it (204).
            assert!(
                resp.status == StatusCode::CREATED || resp.status == StatusCode::NO_CONTENT,
                "{strategy:?}: {} {}",
                resp.status,
                resp.text()
            );
        }
    }

    #[tokio::test]
    async fn api_non_positive_units_rejected() {
        let pool = test_pool().await;
        strategy_fixture(&pool).await;
        let (status, _) = get_optimiser(
            pool,
            None,
            serde_json::json!({
                "listing_id": 1, "holding_account_id": 1, "units": "0", "price": "10"
            }),
        )
        .await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    }

    /// SCENARIOS W. The proceeds spread — `total_proceeds × units_so_far ÷
    /// total_units` — multiplied A$1e24 of proceeds by 1e14 units before
    /// dividing: 1e38, far past `Decimal`'s ~7.9228e28 ceiling, though the
    /// allocation's share is simply the whole A$1e24. The parcel is nil-priced
    /// so the cost-base pro-rate beside it stays under the ceiling and this
    /// test names one expression.
    #[tokio::test]
    async fn api_optimiser_past_the_old_proceeds_spread_ceiling_reports() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "OPT").await;
        insert_buy(
            &pool,
            1,
            ListingId(1),
            ymd(2023, 1, 3),
            dec("100000000000000"),
            Decimal::ZERO,
        )
        .await;

        let (status, body) = get_optimiser(
            pool,
            None,
            serde_json::json!({
                "listing_id": 1, "holding_account_id": 1,
                "units": "100000000000000", "sale_date": "2026-06-15",
                "price": "10000000000"
            }),
        )
        .await;
        let text = String::from_utf8(body).unwrap();
        assert_eq!(status, StatusCode::OK, "{text}");
        let r: OptimiserResponse = serde_json::from_str(&text).unwrap();
        for s in &r.strategies {
            assert_eq!(s.totals.proceeds, dec("1000000000000000000000000"));
        }
    }

    /// SCENARIOS W. The cost-base pro-rate beside it — `remaining_cost_base ×
    /// units ÷ remaining_quantity` — on a parcel costed at A$1e24 across 1e14
    /// units: 1e38 again. Here the sale price is $1, so the proceeds spread
    /// above stays well under the ceiling.
    #[tokio::test]
    async fn api_optimiser_past_the_old_cost_base_prorate_ceiling_reports() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "OPT").await;
        insert_buy(
            &pool,
            1,
            ListingId(1),
            ymd(2023, 1, 3),
            dec("100000000000000"),
            dec("10000000000"),
        )
        .await;

        let (status, body) = get_optimiser(
            pool,
            None,
            serde_json::json!({
                "listing_id": 1, "holding_account_id": 1,
                "units": "100000000000000", "sale_date": "2026-06-15",
                "price": "1"
            }),
        )
        .await;
        let text = String::from_utf8(body).unwrap();
        assert_eq!(status, StatusCode::OK, "{text}");
        let r: OptimiserResponse = serde_json::from_str(&text).unwrap();
        for s in &r.strategies {
            assert_eq!(s.totals.cost_base, dec("1000000000000000000000000"));
        }
    }

    // ---- agreeing with the Sell the estimate rehearses ----------------------

    /// The sale a matrix cell asks the optimiser to rehearse: it is entered
    /// twice over, once as a question (`GET /portfolio/parcel_optimiser`) and
    /// once as the recorded fact the question was about.
    struct Rehearsal {
        units: Decimal,
        sale_date: NaiveDate,
        price: Decimal,
    }

    /// An AMMA statement for `listing 1` with a per-unit cost-base adjustment,
    /// with its `amit_adjustments` rows generated the way the UI's
    /// generate-adjustments action does — so the row covers whatever was open
    /// at the statement's year end, not necessarily the whole parcel.
    async fn amma_with_generated_adjustments(
        client: &ApiClient,
        pool: &SqlitePool,
        id: i64,
        year_end: NaiveDate,
        units_held: Decimal,
        per_unit: &str,
    ) {
        test_support::amma(id, 1)
            .units(units_held)
            .cost_base_adjustment(dec(per_unit))
            .with(|a| {
                a.tax_year_end_date = year_end;
                a.date_received = year_end + chrono::Days::new(45);
            })
            .insert(pool)
            .await;
        client
            .post(
                format!("/amma_statements/{id}/generate_adjustments"),
                &serde_json::json!({}),
            )
            .await
            .expect_status(StatusCode::CREATED);
    }

    /// One cell of the agreement matrix — SCENARIOS section O's pattern, *diff
    /// a decision-support endpoint against the write path it rehearses*.
    ///
    /// Ask the optimiser what a contemplated sale would cost, then record
    /// **exactly** the Sell it described and read `/portfolio/realised_gains`
    /// back. The two are separate implementations of one rule
    /// (`domain::cost_base` reached by two different loaders), so the
    /// assertion is that they agree to the last decimal place — per allocation
    /// and in total — not that either equals a number worked out here. A
    /// cost-base event that reaches only one of them then fails this without
    /// anyone having had to predict its effect.
    ///
    /// Returns the agreed cost base, for a cell that also wants to pin the
    /// figure.
    async fn optimiser_agrees_with_the_recorded_sell(
        setup: impl AsyncFnOnce(&SqlitePool, &ApiClient) -> Rehearsal,
    ) -> Decimal {
        let pool = test_pool().await;
        let client = ApiClient::full(&pool);
        let r = setup(&pool, &client).await;

        let estimate: OptimiserResponse = client
            .get_json(format!(
                "/portfolio/parcel_optimiser?listing_id=1&holding_account_id=1&units={}&sale_date={}&price={}",
                r.units, r.sale_date, r.price
            ))
            .await;
        // FIFO is the cell's allocation: it is the one strategy whose picks
        // are fixed by the fixture rather than by the very cost bases under
        // test, so the recorded Sell consumes the same parcels whether or not
        // the estimate is right.
        let picks: Vec<HypotheticalAllocation> = estimate
            .allocations
            .iter()
            .filter(|a| a.strategy == Strategy::Fifo)
            .map(|a| a.allocation.clone())
            .collect();
        assert!(!picks.is_empty(), "the optimiser proposed no allocation");

        // The same disposal as a recorded fact. The hypothetical carries no
        // brokerage (it isn't known yet), so neither does this Sell.
        const SELL_ID: i64 = 9001;
        test_support::sell(SELL_ID, 1)
            .date(r.sale_date)
            .qty(r.units)
            .price(r.price)
            .brokerage(Decimal::ZERO)
            .insert(&pool)
            .await;
        for (i, p) in picks.iter().enumerate() {
            allocate(
                &pool,
                SELL_ID + i as i64,
                SELL_ID,
                p.purchase_trade_id.0,
                p.units,
            )
            .await;
        }

        // The exact pipeline the optimiser rehearses — the Realised Gains
        // screen prints the same disposal at the cent.
        let realised: Vec<RealisedGainLoss> =
            realised_gains::db_realised_gains(&pool).await.unwrap();
        let recorded = realised
            .iter()
            .find(|row| row.disposal == DisposalId::Sell(TradeId::from(SELL_ID)))
            .expect("the recorded Sell reaches the realised-gains report");

        // Per allocation first: a cell whose parcels err in opposite
        // directions would still reconcile in total.
        for p in &picks {
            let actual = recorded
                .parcels
                .iter()
                .find(|d| d.purchase_trade_id == p.purchase_trade_id)
                .unwrap_or_else(|| panic!("parcel {} was not allocated", p.purchase_trade_id));
            assert_eq!(
                p.units, actual.units,
                "parcel {} units",
                p.purchase_trade_id
            );
            assert_eq!(
                p.cost_base, actual.cost_base,
                "parcel {} cost base: the optimiser estimated {} and the Sell it \
                 rehearses realised {}",
                p.purchase_trade_id, p.cost_base, actual.cost_base
            );
        }
        let fifo = estimate
            .strategies
            .iter()
            .find(|s| s.strategy == Strategy::Fifo)
            .expect("every strategy is reported");
        assert_eq!(fifo.totals.cost_base, recorded.cost_base, "total cost base");
        assert_eq!(fifo.totals.proceeds, recorded.proceeds, "total proceeds");
        assert_eq!(
            fifo.totals.capital_gain_loss, recorded.capital_gain_loss,
            "capital gain/loss"
        );
        assert_eq!(
            fifo.totals.discount_eligible_gain, recorded.discount_eligible_gain,
            "discount-eligible gain"
        );
        recorded.cost_base
    }

    /// The baseline: no cost-base events at all.
    #[tokio::test]
    async fn agreement_on_a_plain_partial_sale() {
        let cost = optimiser_agrees_with_the_recorded_sell(async |pool, _client| {
            insert_listing(pool, 1, "VAS").await;
            insert_buy(
                pool,
                1,
                ListingId(1),
                ymd(2024, 1, 2),
                dec("100"),
                dec("10"),
            )
            .await;
            Rehearsal {
                units: dec("40"),
                sale_date: ymd(2026, 6, 15),
                price: dec("15"),
            }
        })
        .await;
        assert_eq!(cost, dec("400"));
    }

    /// A pick that takes the whole parcel — the case the old pro-rate was
    /// exact for, kept as a control.
    #[tokio::test]
    async fn agreement_on_a_whole_parcel_pick() {
        optimiser_agrees_with_the_recorded_sell(async |pool, client| {
            insert_listing(pool, 1, "VDHG").await;
            insert_buy(
                pool,
                1,
                ListingId(1),
                ymd(2024, 1, 15),
                dec("100"),
                dec("60"),
            )
            .await;
            amma_with_generated_adjustments(client, pool, 1, ymd(2026, 6, 30), dec("100"), "1.30")
                .await;
            Rehearsal {
                units: dec("100"),
                sale_date: ymd(2026, 3, 2),
                price: dec("70"),
            }
        })
        .await;
    }

    /// The finding: an AMMA statement whose tax year end falls **after** the
    /// contemplated sale. The adjustment is made just before the CGT event
    /// (s 104-107B / LCR 2015/11 para 13), so it reaches the disposed units —
    /// but an open-holdings read excludes a statement for a year that has not
    /// ended, and the estimate used to apply none of it.
    #[tokio::test]
    async fn agreement_when_the_amma_year_end_falls_after_the_sale() {
        optimiser_agrees_with_the_recorded_sell(async |pool, client| {
            insert_listing(pool, 1, "VDHG").await;
            insert_buy(
                pool,
                1,
                ListingId(1),
                ymd(2024, 1, 15),
                dec("100"),
                dec("60"),
            )
            .await;
            amma_with_generated_adjustments(client, pool, 1, ymd(2026, 6, 30), dec("100"), "1.30")
                .await;
            Rehearsal {
                units: dec("40"),
                sale_date: ymd(2026, 3, 2),
                price: dec("70"),
            }
        })
        .await;
    }

    /// The control the finding named: a statement whose year end is already
    /// past when the sale is contemplated always did agree, and must go on
    /// agreeing.
    #[tokio::test]
    async fn agreement_when_the_amma_year_end_falls_before_the_sale() {
        optimiser_agrees_with_the_recorded_sell(async |pool, client| {
            insert_listing(pool, 1, "VDHG").await;
            insert_buy(
                pool,
                1,
                ListingId(1),
                ymd(2024, 1, 15),
                dec("100"),
                dec("60"),
            )
            .await;
            amma_with_generated_adjustments(client, pool, 1, ymd(2025, 6, 30), dec("100"), "1.30")
                .await;
            Rehearsal {
                units: dec("40"),
                sale_date: ymd(2026, 3, 2),
                price: dec("70"),
            }
        })
        .await;
    }

    /// An AMIT row covering **less** than the parcel — 30 of the 100 units
    /// were already sold when the statement's year ended, so the generated row
    /// covers the 70 still held. This is the cell that says the pipeline is
    /// not linear in the disposed units: how much of the row spills onto the
    /// sold units depends on how many are sold, so the estimate has to be
    /// costed at the picked quantity rather than pro-rated off the parcel.
    #[tokio::test]
    async fn agreement_when_the_amit_row_covers_less_than_the_parcel() {
        optimiser_agrees_with_the_recorded_sell(async |pool, client| {
            insert_listing(pool, 1, "VDHG").await;
            insert_buy(
                pool,
                1,
                ListingId(1),
                ymd(2024, 1, 15),
                dec("100"),
                dec("60"),
            )
            .await;
            test_support::sell(2, 1)
                .date(ymd(2025, 5, 1))
                .qty(dec("30"))
                .price(dec("65"))
                .insert(pool)
                .await;
            allocate(pool, 1, 2, 1, dec("30")).await;
            // Generated after that sale, so `quantity` is the 70 still open at
            // the year end.
            amma_with_generated_adjustments(client, pool, 1, ymd(2026, 6, 30), dec("70"), "1.30")
                .await;
            Rehearsal {
                units: dec("40"),
                sale_date: ymd(2026, 3, 2),
                price: dec("70"),
            }
        })
        .await;
    }

    /// The same partly-covering row taken in full — the disposal consumes
    /// every unit the statement's year end saw still held.
    #[tokio::test]
    async fn agreement_when_a_partly_covering_amit_row_is_taken_in_full() {
        optimiser_agrees_with_the_recorded_sell(async |pool, client| {
            insert_listing(pool, 1, "VDHG").await;
            insert_buy(
                pool,
                1,
                ListingId(1),
                ymd(2024, 1, 15),
                dec("100"),
                dec("60"),
            )
            .await;
            test_support::sell(2, 1)
                .date(ymd(2025, 5, 1))
                .qty(dec("30"))
                .price(dec("65"))
                .insert(pool)
                .await;
            allocate(pool, 1, 2, 1, dec("30")).await;
            amma_with_generated_adjustments(client, pool, 1, ymd(2026, 6, 30), dec("70"), "1.30")
                .await;
            Rehearsal {
                units: dec("70"),
                sale_date: ymd(2026, 3, 2),
                price: dec("70"),
            }
        })
        .await;
    }

    /// A return of capital (CGT event G1) between acquisition and the sale.
    #[tokio::test]
    async fn agreement_on_a_return_of_capital() {
        optimiser_agrees_with_the_recorded_sell(async |pool, _client| {
            insert_listing(pool, 1, "TLS").await;
            insert_buy(
                pool,
                1,
                ListingId(1),
                ymd(2024, 1, 2),
                dec("100"),
                dec("10"),
            )
            .await;
            corporate_action::db_upsert(
                pool,
                &CorporateAction {
                    id: 1,
                    listing_id: ListingId(1),
                    date: ymd(2025, 3, 3),
                    kind: ActionKind::ReturnOfCapital {
                        amount_per_unit: dec("0.15"),
                        currency: "AUD".into(),
                        record_date: None,
                    },
                },
            )
            .await
            .unwrap();
            Rehearsal {
                units: dec("40"),
                sale_date: ymd(2026, 6, 15),
                price: dec("12"),
            }
        })
        .await;
    }

    /// A share split between acquisition and the sale: the candidates, the
    /// contemplated units and the recorded Sell are all in the sale date's
    /// unit basis, while the cost base stays in the as-acquired one
    /// (TD 2000/10).
    #[tokio::test]
    async fn agreement_across_a_split_between_acquisition_and_the_sale() {
        optimiser_agrees_with_the_recorded_sell(async |pool, _client| {
            insert_listing(pool, 1, "SPL").await;
            insert_buy(
                pool,
                1,
                ListingId(1),
                ymd(2024, 1, 2),
                dec("100"),
                dec("10"),
            )
            .await;
            corporate_action::db_upsert(
                pool,
                &CorporateAction {
                    id: 1,
                    listing_id: ListingId(1),
                    date: ymd(2025, 1, 2),
                    kind: ActionKind::ShareSplit {
                        split_new_units: dec("3"),
                        split_old_units: Decimal::ONE,
                    },
                },
            )
            .await
            .unwrap();
            // 300 units in the sale date's basis; 100 of them is a third of
            // the parcel, which the pro-rate cannot divide evenly.
            Rehearsal {
                units: dec("100"),
                sale_date: ymd(2026, 6, 15),
                price: dec("5"),
            }
        })
        .await;
    }

    /// Every cost-base event at once, over two parcels the pick spans — the
    /// cell that would catch a fix that only holds one event at a time.
    #[tokio::test]
    async fn agreement_across_two_parcels_with_every_cost_base_event() {
        optimiser_agrees_with_the_recorded_sell(async |pool, client| {
            insert_listing(pool, 1, "VDHG").await;
            insert_buy(
                pool,
                1,
                ListingId(1),
                ymd(2023, 2, 1),
                dec("100"),
                dec("40"),
            )
            .await;
            insert_buy(
                pool,
                2,
                ListingId(1),
                ymd(2024, 1, 15),
                dec("100"),
                dec("60"),
            )
            .await;
            corporate_action::db_upsert(
                pool,
                &CorporateAction {
                    id: 1,
                    listing_id: ListingId(1),
                    date: ymd(2025, 3, 3),
                    kind: ActionKind::ReturnOfCapital {
                        amount_per_unit: dec("0.25"),
                        currency: "AUD".into(),
                        record_date: None,
                    },
                },
            )
            .await
            .unwrap();
            // Two statements, one already past and one for the year the
            // contemplated sale falls inside.
            amma_with_generated_adjustments(client, pool, 1, ymd(2025, 6, 30), dec("200"), "0.70")
                .await;
            amma_with_generated_adjustments(client, pool, 2, ymd(2026, 6, 30), dec("200"), "1.30")
                .await;
            Rehearsal {
                units: dec("150"),
                sale_date: ymd(2026, 3, 2),
                price: dec("70"),
            }
        })
        .await;
    }

    /// The finding's own reproduction, with its figures: a A$52.00 gap — the
    /// statement's whole per-unit adjustment over the 40 disposed units — that
    /// the estimate used to leave on the table, reporting the *higher* cost
    /// base and so under-stating the gain.
    #[tokio::test]
    async fn the_amma_reduction_reaches_the_estimate_of_a_sale_inside_its_year() {
        let cost = optimiser_agrees_with_the_recorded_sell(async |pool, client| {
            insert_listing(pool, 1, "VDHG").await;
            insert_buy(
                pool,
                1,
                ListingId(1),
                ymd(2024, 1, 15),
                dec("100"),
                dec("60"),
            )
            .await;
            amma_with_generated_adjustments(client, pool, 1, ymd(2026, 6, 30), dec("100"), "1.30")
                .await;
            Rehearsal {
                units: dec("40"),
                sale_date: ymd(2026, 3, 2),
                price: dec("70"),
            }
        })
        .await;
        // 40 × ($60.00 − $1.30), not the 40 × $60.00 = $2,400.00 the estimate
        // reported while the statement reached only the recorded Sell.
        assert_eq!(cost, dec("2348.00"));
    }

    /// The failure that matters is not the figure but the *ranking*: a
    /// statement covering one candidate parcel and not another moves one
    /// parcel's per-unit gain past the other's, so min-gain picks a different
    /// parcel once the adjustment reaches the estimate.
    #[tokio::test]
    async fn an_amma_inside_the_sale_year_reorders_the_strategies() {
        let pool = test_pool().await;
        let client = ApiClient::full(&pool);
        insert_listing(&pool, 1, "VDHG").await;
        // Two parcels ten cents apart per unit before the statement — parcel
        // 1 is the dearer, so on its own it is the smaller gain…
        insert_buy(
            &pool,
            1,
            ListingId(1),
            ymd(2023, 2, 1),
            dec("100"),
            dec("60.00"),
        )
        .await;
        insert_buy(
            &pool,
            2,
            ListingId(1),
            ymd(2023, 2, 2),
            dec("100"),
            dec("59.90"),
        )
        .await;
        // …and a statement that reduces only the first, by $1.30/unit, which
        // takes its cost base under the other's and so reverses the order.
        test_support::amma(1, 1)
            .units(dec("100"))
            .cost_base_adjustment(dec("1.30"))
            .with(|a| {
                a.tax_year_end_date = ymd(2026, 6, 30);
                a.date_received = ymd(2026, 8, 14);
            })
            .insert(&pool)
            .await;
        test_support::amit_adjustment(&pool, 1, 1, 1, dec("100")).await;

        let estimate: OptimiserResponse = client
            .get_json(
                "/portfolio/parcel_optimiser?listing_id=1&holding_account_id=1&units=100&sale_date=2026-03-02&price=70",
            )
            .await;
        let min_gain: Vec<i64> = estimate
            .allocations
            .iter()
            .filter(|a| a.strategy == Strategy::MinGain)
            .map(|a| a.allocation.purchase_trade_id.0)
            .collect();
        // Once the statement reaches the estimate, parcel 1 is costed at
        // $58.70/unit against parcel 2's $59.90 — so parcel 2 is now the
        // smaller gain and min-gain takes it. Before this fix the statement
        // reached neither, parcel 1 stood at $60.00, and the report advised
        // selling parcel 1: a different parcel, not merely a different figure.
        assert_eq!(min_gain, vec![2]);
    }

    /// The pre-sale what-if reads its candidates through the very same loader,
    /// so it must report the same hypothetical cost base as the optimiser —
    /// and therefore the same one the recorded Sell realises.
    #[tokio::test]
    async fn the_what_if_costs_the_same_disposal_the_same_way() {
        let pool = test_pool().await;
        let client = ApiClient::full(&pool);
        insert_listing(&pool, 1, "VDHG").await;
        insert_buy(
            &pool,
            1,
            ListingId(1),
            ymd(2024, 1, 15),
            dec("100"),
            dec("60"),
        )
        .await;
        amma_with_generated_adjustments(&client, &pool, 1, ymd(2026, 6, 30), dec("100"), "1.30")
            .await;

        let what_if: WhatIfResponse = client
            .post_json(
                "/portfolio/net_capital_gain/what_if",
                &serde_json::json!({
                    "listing_id": 1, "holding_account_id": 1, "units": "40",
                    "proceeds": "2800", "date": "2026-03-02", "strategy": "fifo"
                }),
            )
            .await;
        assert_eq!(what_if.hypothetical.cost_base, dec("2348.00"));
        assert_eq!(what_if.allocations.len(), 1);
        assert_eq!(what_if.allocations[0].cost_base, dec("2348.00"));
    }

    /// A loss offsets non-discountable gains before discount-eligible ones,
    /// so a $4/unit short-term gain the disposal's $50 loss absorbs whole
    /// is cheaper than a $7/unit long-term one it does not — though ranked
    /// per unit at half weight, the long-term gain looks cheaper (3.50 < 4).
    #[test]
    fn min_gain_takes_the_short_term_gain_a_loss_absorbs_over_a_cheaper_looking_discounted_one() {
        let parcel = |id: i64, acquired: NaiveDate, cost: i64| CandidateParcel {
            trade_id: TradeId(id),
            holding_account_id: HoldingAccountId(1),
            acquisition_date: acquired,
            remaining_quantity: Decimal::from(10),
            remaining_cost_base: Decimal::from(cost * 10),
        };
        let parcels = [
            parcel(1, ymd(2025, 9, 1), 15), // loss $5/unit
            parcel(2, ymd(2026, 3, 2), 6),  // short-term gain $4/unit
            parcel(3, ymd(2024, 1, 2), 3),  // long-term gain $7/unit
        ];
        let picks = allocate_parcels(
            &parcels,
            Decimal::from(20),
            Decimal::from(10),
            ymd(2026, 6, 15),
            Strategy::MinGain,
        );
        // Loss 50 against gain 40 nets to nil (the old ranking's 1 + 3 left 10).
        assert_eq!(
            picks,
            vec![
                (TradeId(1), Decimal::from(10)),
                (TradeId(2), Decimal::from(10))
            ]
        );
    }

    mod properties {
        use super::*;
        use crate::entities::corporate_action;
        use crate::entities::corporate_action::{ActionKind, CorporateAction};
        use proptest::prelude::{any, prop, prop_assert, prop_assert_eq, prop_assume, proptest};
        use proptest::strategy;
        use proptest::strategy::Strategy as _;
        use std::collections::HashSet;

        /// Up to six open parcels: an acquisition date within three years
        /// of the 2026-06-15 sale, so either side of the 12-month discount
        /// clock, and a quantity and per-unit cost either side of the $10
        /// sale price.
        fn parcels() -> impl strategy::Strategy<Value = Vec<CandidateParcel>> {
            prop::collection::vec((0i64..1100, 1i64..=1_000, 1i64..=2_000), 1..=6).prop_map(
                |rows| {
                    rows.into_iter()
                        .enumerate()
                        .map(|(i, (days_before, qty, cents))| CandidateParcel {
                            trade_id: TradeId(i as i64 + 1),
                            holding_account_id: HoldingAccountId(1),
                            acquisition_date: ymd(2026, 6, 15)
                                - chrono::Duration::days(days_before),
                            remaining_quantity: Decimal::from(qty),
                            remaining_cost_base: Decimal::new(cents * qty, 2),
                        })
                        .collect()
                },
            )
        }

        /// The disposal's own assessable contribution, netted the ATO way
        /// (`net_capital_gain::net_years`): losses against the
        /// non-discountable gains first, then the discount-eligible ones,
        /// which are then halved.
        fn assessable(
            parcels: &[CandidateParcel],
            picks: &[(TradeId, Decimal)],
            price: Decimal,
        ) -> Decimal {
            let (mut other, mut discounted, mut losses) =
                (Decimal::ZERO, Decimal::ZERO, Decimal::ZERO);
            for (id, units) in picks {
                let p = parcels.iter().find(|p| p.trade_id == *id).unwrap();
                let gain = *units * (price - p.remaining_cost_base / p.remaining_quantity);
                if gain <= Decimal::ZERO {
                    losses -= gain;
                } else if discount_eligible(p.acquisition_date, ymd(2026, 6, 15)) {
                    discounted += gain;
                } else {
                    other += gain;
                }
            }
            let to_other = other.min(losses);
            let to_discounted = discounted.min(losses - to_other);
            other - to_other + (discounted - to_discounted) / Decimal::from(2)
        }

        /// Heap-free permutation walk: calls `visit` with every ordering of
        /// `items[k..]` behind the fixed prefix.
        fn permutations<'a>(
            items: &mut Vec<&'a CandidateParcel>,
            k: usize,
            visit: &mut impl FnMut(&[&'a CandidateParcel]),
        ) {
            if k == items.len() {
                visit(items);
                return;
            }
            for i in k..items.len() {
                items.swap(k, i);
                permutations(items, k + 1, visit);
                items.swap(k, i);
            }
        }

        proptest! {
            #![proptest_config(test_support::db_proptest_config())]

            /// The optimiser costs each pick "the way the recorded Sell
            /// will", so recording the Sell it proposes must make the
            /// realised-gains report show exactly its figures — per parcel
            /// and in total — whatever AMIT adjustment, return of capital and
            /// earlier sales the parcels carry.
            #[test]
            fn a_recorded_sell_realises_exactly_what_the_optimiser_previewed(
                parcels in prop::collection::vec((10i64..=500, 100i64..=3_000), 1..=4),
                amit_cents in -300i64..=300,
                roc_cents in prop::option::of(1i64..=400),
                sold_before in prop::collection::vec(0u32..=100, 4),
                units_pct in 1u32..=100,
                strategy in prop::sample::select(ALL_STRATEGIES.to_vec()),
                price_cents in 1i64..=4_000,
            ) {
                use crate::entities::amit_adjustment_generation::{self as generation, GenerateBody};
                use crate::entities::sell::{self, AllocationInput, SellBody};
                use crate::infra::db::WriteMode;
                use crate::reports::realised_gains;

                test_support::block_on(async move {
                    let pool = test_pool().await;
                    // An AMIT trust's cost base moves by its AMMA statement; a
                    // company's by a return of capital — never both.
                    test_support::listing(1).ticker("OPT").amit(roc_cents.is_none()).insert(&pool).await;
                    let buy_dates = [ymd(2023, 8, 2), ymd(2023, 9, 13), ymd(2023, 10, 11), ymd(2024, 2, 14)];
                    let mut open = Vec::new();
                    for (i, (qty, cents)) in parcels.iter().enumerate() {
                        insert_buy(&pool, i as i64 + 1, ListingId(1), buy_dates[i], Decimal::from(*qty), Decimal::new(*cents, 2)).await;
                        open.push(Decimal::from(*qty));
                    }
                    if let Some(cents) = roc_cents {
                        corporate_action::db_upsert(&pool, &CorporateAction {
                            id: 1,
                            listing_id: ListingId(1),
                            date: ymd(2024, 3, 13),
                            kind: ActionKind::ReturnOfCapital {
                                amount_per_unit: Decimal::new(cents, 2),
                                currency: "AUD".into(),
                                record_date: None,
                            },
                        })
                        .await
                        .unwrap();
                    }
                    if roc_cents.is_none() {
                        let held: Decimal = open.iter().sum();
                        test_support::amma(1, 1)
                            .units(held)
                            .cost_base_adjustment(Decimal::new(amit_cents, 2))
                            .with(|a| {
                                a.tax_year_end_date = ymd(2024, 6, 30);
                                a.date_received = ymd(2024, 9, 10);
                            })
                            .insert(&pool)
                            .await;
                        generation::db_generate(&pool, 1, &GenerateBody::default()).await.unwrap();
                    }

                    // An earlier Sell after the year end takes part of each parcel.
                    let earlier: Vec<AllocationInput> = open
                        .iter_mut()
                        .zip(&sold_before)
                        .enumerate()
                        .filter_map(|(i, (left, pct))| {
                            let take = (*left * Decimal::from(*pct) / Decimal::ONE_HUNDRED).floor();
                            (take > Decimal::ZERO).then(|| {
                                *left -= take;
                                AllocationInput { purchase_trade_id: TradeId(i as i64 + 1), quantity_allocated: take }
                            })
                        })
                        .collect();
                    let sell_body = |date, price: Decimal, allocations: Vec<AllocationInput>| SellBody {
                        date,
                        settlement_date: None,
                        listing_id: ListingId(1),
                        average_price: price,
                        quantity: allocations.iter().map(|a| a.quantity_allocated).sum(),
                        currency: "AUD".into(),
                        brokerage: Decimal::ZERO,
                        gst_on_brokerage: Decimal::ZERO,
                        brokerage_includes_gst: false,
                        brokerage_currency: "AUD".into(),
                        fx_rate: Decimal::ONE,
                        spot_fx_rate: None,
                        contract_note_ref: None,
                        statement_total: None,
                        holding_account_id: HoldingAccountId(1),
                        allocations,
                    };
                    if !earlier.is_empty() {
                        sell::db_create_sell(&pool, &sell_body(ymd(2024, 8, 14), Decimal::from(9), earlier), WriteMode::Commit)
                            .await
                            .unwrap();
                    }
                    let still_open: Decimal = open.iter().sum();
                    prop_assume!(still_open > Decimal::ZERO);

                    let sale_date = ymd(2025, 3, 12);
                    let units = (still_open * Decimal::from(units_pct) / Decimal::ONE_HUNDRED).ceil();
                    let price = Decimal::new(price_cents, 2);
                    let candidates = db_candidate_parcels(&pool, ListingId(1), Some(HoldingAccountId(1)), sale_date).await.unwrap();
                    let picks = allocate_strategy(&candidates, units, price, strategy);
                    let (previewed, totals) = disposal_figures(&candidates, &picks, price * units, units).unwrap();

                    let recorded = sell::db_create_sell(
                        &pool,
                        &sell_body(
                            sale_date,
                            price,
                            picks.iter().map(|(id, q)| AllocationInput { purchase_trade_id: *id, quantity_allocated: *q }).collect(),
                        ),
                        WriteMode::Commit,
                    )
                    .await
                    .unwrap();
                    let realised = realised_gains::db_realised_gains(&pool).await.unwrap();
                    let row = realised.iter().find(|r| r.disposal.sell() == Some(recorded.id)).unwrap();
                    for p in &previewed {
                        let actual = row.parcels.iter().find(|d| d.purchase_trade_id == p.purchase_trade_id).unwrap();
                        prop_assert_eq!(actual.units, p.units);
                        prop_assert_eq!(actual.cost_base, p.cost_base, "parcel {:?} cost base", p.purchase_trade_id);
                        prop_assert_eq!(actual.proceeds, p.proceeds, "parcel {:?} proceeds", p.purchase_trade_id);
                        prop_assert_eq!(actual.discount_eligible, p.discount_eligible);
                    }
                    prop_assert_eq!(row.parcels.len(), previewed.len());
                    prop_assert_eq!(row.proceeds, totals.proceeds);
                    prop_assert_eq!(row.cost_base, totals.cost_base);
                    prop_assert_eq!(row.capital_loss, totals.capital_loss);
                    prop_assert_eq!(row.discount_eligible_gain, totals.discount_eligible_gain);
                    prop_assert_eq!(row.non_discountable_gain, totals.non_discountable_gain);
                    Ok(())
                })?;
            }
        }

        proptest! {
            /// Every strategy draws exactly the units asked for (or every
            /// unit open, if fewer), never more of a parcel than remains
            /// and never the same parcel twice, whatever order the
            /// candidates arrive in.
            #[test]
            fn every_strategy_allocates_exactly_the_units_and_ignores_input_order(
                parcels in parcels(),
                units in 1i64..=7_000,
                shuffle in any::<usize>(),
            ) {
                let units = Decimal::from(units);
                let price = Decimal::from(10);
                let open: Decimal = parcels.iter().map(|p| p.remaining_quantity).sum();
                let mut shuffled = parcels.clone();
                let n = shuffled.len();
                shuffled.rotate_left(shuffle % n);
                if shuffle % 2 == 1 {
                    shuffled.reverse();
                }
                for strategy in ALL_STRATEGIES {
                    let picks = allocate_parcels(&parcels, units, price, ymd(2026, 6, 15), strategy);
                    prop_assert_eq!(picks.iter().map(|(_, u)| *u).sum::<Decimal>(), units.min(open));
                    let mut seen = HashSet::new();
                    for (id, take) in &picks {
                        prop_assert!(seen.insert(*id), "{:?} drew parcel {:?} twice", strategy, id);
                        let p = parcels.iter().find(|p| p.trade_id == *id).unwrap();
                        prop_assert!(*take > Decimal::ZERO && *take <= p.remaining_quantity);
                    }
                    prop_assert_eq!(
                        &picks,
                        &allocate_parcels(&shuffled, units, price, ymd(2026, 6, 15), strategy),
                        "{:?} depends on the candidates' order", strategy
                    );
                }
            }

            /// `min_gain` is documented as minimising the disposal's
            /// assessable gain, so no other order of consuming the same
            /// parcels — every strategy's, and every permutation's — may
            /// net to less.
            #[test]
            fn min_gain_nets_no_more_than_any_other_order(
                parcels in parcels(),
                units in 1i64..=7_000,
            ) {
                let units = Decimal::from(units);
                let price = Decimal::from(10);
                let sale = ymd(2026, 6, 15);
                let min_gain = assessable(&parcels, &allocate_parcels(&parcels, units, price, sale, Strategy::MinGain), price);
                let greedy = |order: &[&CandidateParcel]| {
                    let mut left = units;
                    let mut picks = Vec::new();
                    for p in order {
                        let take = p.remaining_quantity.min(left);
                        if take > Decimal::ZERO {
                            picks.push((p.trade_id, take));
                            left -= take;
                        }
                    }
                    picks
                };
                let mut order: Vec<&CandidateParcel> = parcels.iter().collect();
                permutations(&mut order, 0, &mut |order| {
                    let other = assessable(&parcels, &greedy(order), price);
                    assert!(min_gain <= other, "min_gain nets {min_gain} but {:?} nets {other}",
                        order.iter().map(|p| p.trade_id).collect::<Vec<_>>());
                });
            }
        }
    }
}
