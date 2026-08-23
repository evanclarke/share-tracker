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

use crate::domain::cgt_discount::discount_eligible;
use crate::domain::contemplated_disposal;
use crate::domain::cost_base::ParcelRow;
use crate::domain::open_parcels;
use crate::entities::closing_price::{self, SharedFetcher};
use crate::infra::decimal::mul_div;
use crate::infra::http::ApiError;
use axum::{Extension, Json, Router, extract::State, routing::post};
use chrono::NaiveDate;
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;
use std::cmp::Ordering;
use std::collections::HashMap;

/// A parcel-selection strategy: the order in which open parcels are consumed
/// by a hypothetical sale.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Strategy {
    /// Oldest acquisition first — the no-choice baseline.
    Fifo,
    /// Smallest assessable contribution per unit first: losses (full
    /// negative), then discount-eligible gains at half weight, then
    /// non-discountable gains — minimises the current-year assessable gain.
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
    pub trade_id: i64,
    pub holding_account_id: i64,
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
    rows: HashMap<i64, ParcelRow>,
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
    fn cost_base(&self, trade_id: i64, units: Decimal) -> Result<Decimal, sqlx::Error> {
        let row = self
            .rows
            .get(&trade_id)
            .expect("every pick names a candidate parcel");
        self.costing
            .adjusted_cost_base_aud(row, units, self.sale_date)
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
    listing_id: i64,
    holding_account_id: Option<i64>,
    sale_date: NaiveDate,
) -> Result<Candidates, sqlx::Error> {
    let mut tx = pool.begin().await?;
    let candidates =
        db_candidate_parcels_on(&mut tx, listing_id, holding_account_id, sale_date).await?;
    tx.commit().await?;
    Ok(candidates)
}

/// The same candidates read on the caller's own connection, for the what-if,
/// which folds them into its wider single-snapshot read transaction.
pub async fn db_candidate_parcels_on(
    conn: &mut sqlx::SqliteConnection,
    listing_id: i64,
    holding_account_id: Option<i64>,
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
            remaining_quantity: p.remaining_as_of,
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

/// Allocate `units` across `parcels` in the strategy's preference order,
/// greedily consuming each parcel's remaining quantity. `price` is the
/// per-unit sale price in AUD (it drives the gain-based orderings).
/// `parcels` must already be FIFO-sorted (the tie-break order).
pub fn allocate_strategy(
    parcels: &[CandidateParcel],
    units: Decimal,
    price: Decimal,
    sale_date: NaiveDate,
    strategy: Strategy,
) -> Vec<(i64, Decimal)> {
    let gain_pu = |p: &CandidateParcel| price - p.remaining_cost_base / p.remaining_quantity;
    // Assessable contribution per unit: a loss offsets gains in full; a
    // discount-eligible gain counts at half after the 50% discount.
    let assessable_pu = |p: &CandidateParcel| {
        let g = gain_pu(p);
        if g > Decimal::ZERO && discount_eligible(p.acquisition_date, sale_date) {
            g / Decimal::from(2)
        } else {
            g
        }
    };

    let mut order: Vec<&CandidateParcel> = parcels.iter().collect();
    let fifo = |a: &CandidateParcel, b: &CandidateParcel| {
        (a.acquisition_date, a.trade_id).cmp(&(b.acquisition_date, b.trade_id))
    };
    match strategy {
        Strategy::Fifo => {} // already FIFO-sorted
        Strategy::MinGain => {
            order.sort_by(|a, b| assessable_pu(a).cmp(&assessable_pu(b)).then(fifo(a, b)));
        }
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

/// One parcel's share of a hypothetical disposal.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HypotheticalAllocation {
    pub purchase_trade_id: i64,
    pub holding_account_id: i64,
    pub acquisition_date: NaiveDate,
    pub units: Decimal,
    /// Adjusted cost base of the allocated units, AUD.
    pub cost_base: Decimal,
    /// This allocation's share of the proceeds, AUD.
    pub proceeds: Decimal,
    /// proceeds − cost_base (positive = gain, negative = loss).
    pub capital_gain_loss: Decimal,
    /// Held strictly more than 12 months at the sale date.
    pub discount_eligible: bool,
}

/// A hypothetical disposal's totals, in the realised-gains report's buckets:
/// `capital_gain_loss == discount_eligible_gain + non_discountable_gain − capital_loss`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
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
    picks: &[(i64, Decimal)],
    total_proceeds: Decimal,
    total_units: Decimal,
) -> Result<(Vec<HypotheticalAllocation>, DisposalTotals), sqlx::Error> {
    let sale_date = candidates.sale_date();
    let by_id: HashMap<i64, &CandidateParcel> = candidates
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

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OptimiserRequest {
    pub listing_id: i64,
    /// The account the contemplated Sell would happen in — a real Sell's
    /// allocations may only consume its own account's parcels.
    pub holding_account_id: i64,
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
#[derive(Debug, Serialize, Deserialize)]
pub struct StrategySummary {
    pub strategy: Strategy,
    #[serde(flatten)]
    pub totals: DisposalTotals,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct StrategyAllocation {
    pub strategy: Strategy,
    #[serde(flatten)]
    pub allocation: HypotheticalAllocation,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct OptimiserResponse {
    pub listing_id: i64,
    pub holding_account_id: i64,
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
    /// (always [`crate::reports::TAXPAYER_BASIS`]) — the 50% discount the
    /// strategies weight discount-eligible gains at is the
    /// Australian-resident-individual rate; other entity types are not
    /// modelled. Stated once for the whole response rather than repeated per
    /// strategy row because it is not a property of any one candidate: it is
    /// the basis the candidates are *ranked against each other* on, so on a
    /// different basis (a company, a super fund at 33⅓%, a non-resident) the
    /// recommendation itself — not merely a figure on it — would differ.
    pub taxpayer_basis: String,
}

pub fn router() -> Router<SqlitePool> {
    Router::new().route("/portfolio/parcel-optimiser", post(optimiser_handler))
}

async fn optimiser_handler(
    State(pool): State<SqlitePool>,
    fetcher: Option<Extension<SharedFetcher>>,
    Json(req): Json<OptimiserRequest>,
) -> Result<Json<OptimiserResponse>, ApiError> {
    if req.units <= Decimal::ZERO {
        return Err(ApiError::Unprocessable(
            "units must be positive".to_string(),
        ));
    }
    let sale_date = req
        .sale_date
        .unwrap_or_else(|| chrono::Local::now().date_naive());

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
            super::listing_label(&pool, req.listing_id).await?,
            super::account_label(&pool, req.holding_account_id).await?
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
                true,
                &HashMap::new(),
                [req.listing_id],
            )
            .await
            .map_err(ApiError::from)?;
            match live.get(&req.listing_id) {
                Some(Ok(v)) => (v.aud_price, Some(v.as_of.clone())),
                Some(Err(reason)) => {
                    return Err(ApiError::Unprocessable(format!(
                        "no live price for {}: {reason} — supply a price",
                        super::listing_label(&pool, req.listing_id).await?
                    )));
                }
                None => {
                    return Err(ApiError::Unprocessable(format!(
                        "no live price for {} — supply a price",
                        super::listing_label(&pool, req.listing_id).await?
                    )));
                }
            }
        }
    };

    let total_proceeds = price * req.units;
    let mut strategies = Vec::new();
    let mut allocations = Vec::new();
    for strategy in ALL_STRATEGIES {
        let picks = allocate_strategy(candidates.parcels(), req.units, price, sale_date, strategy);
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
        taxpayer_basis: crate::reports::TAXPAYER_BASIS.to_string(),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{self, ApiClient, allocate, dec, test_pool, ymd};
    use axum::http::StatusCode;

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
            insert_buy(pool, id, 1, date, Decimal::from(100), dec(price)).await;
        }
    }

    fn fixture_sale() -> (Decimal, Decimal, NaiveDate) {
        (Decimal::from(150), Decimal::from(10), ymd(2026, 6, 15))
    }

    /// The fixture's candidates as at the fixture sale's own date — the date
    /// every figure derived from them is costed on.
    async fn fixture_candidates(pool: &SqlitePool) -> Candidates {
        let (_, _, sale_date) = fixture_sale();
        db_candidate_parcels(pool, 1, None, sale_date)
            .await
            .unwrap()
    }

    async fn fixture_picks(pool: &SqlitePool, strategy: Strategy) -> Vec<(i64, Decimal)> {
        let candidates = fixture_candidates(pool).await;
        let (units, price, sale_date) = fixture_sale();
        allocate_strategy(candidates.parcels(), units, price, sale_date, strategy)
    }

    // ---- strategy allocation choices ------------------------------------

    #[tokio::test]
    async fn fifo_takes_oldest_parcels_first() {
        let pool = test_pool().await;
        strategy_fixture(&pool).await;
        let picks = fixture_picks(&pool, Strategy::Fifo).await;
        assert_eq!(picks, vec![(1, Decimal::from(100)), (2, Decimal::from(50))]);
    }

    #[tokio::test]
    async fn harvest_losses_takes_loss_parcels_then_fifo() {
        let pool = test_pool().await;
        strategy_fixture(&pool).await;
        // Parcel 2 is the only loss; the remainder follows FIFO (parcel 1).
        let picks = fixture_picks(&pool, Strategy::HarvestLosses).await;
        assert_eq!(picks, vec![(2, Decimal::from(100)), (1, Decimal::from(50))]);
    }

    #[tokio::test]
    async fn min_gain_orders_by_assessable_contribution() {
        let pool = test_pool().await;
        strategy_fixture(&pool).await;
        // Assessable per unit: parcel 2 (−2) < parcel 3 (+2 eligible → 1)
        // < parcel 1 (+5 eligible → 2.5) < parcel 4 (+6 not eligible → 6).
        let picks = fixture_picks(&pool, Strategy::MinGain).await;
        assert_eq!(picks, vec![(2, Decimal::from(100)), (3, Decimal::from(50))]);
    }

    #[tokio::test]
    async fn max_discount_takes_eligible_gain_parcels_first() {
        let pool = test_pool().await;
        strategy_fixture(&pool).await;
        // Eligible gain parcels in FIFO order: parcel 1 then parcel 3 —
        // unlike FIFO, the loss parcel 2 is passed over.
        let picks = fixture_picks(&pool, Strategy::MaxDiscount).await;
        assert_eq!(picks, vec![(1, Decimal::from(100)), (3, Decimal::from(50))]);
    }

    /// The optimiser classifies on the shared ownership rule, so the boundary
    /// it applies is `domain::cgt_discount`'s (pinned by its own test) rather
    /// than one of its own. Asserted here through `allocate_strategy` so the
    /// wiring — not just the rule — is covered: a parcel one day short of 12
    /// months is a non-discountable gain and so sorts last under MaxDiscount.
    #[test]
    fn max_discount_treats_a_parcel_one_day_short_as_non_discountable() {
        let parcel = |trade_id, acquired| CandidateParcel {
            trade_id,
            holding_account_id: 1,
            acquisition_date: acquired,
            remaining_quantity: Decimal::from(10),
            remaining_cost_base: Decimal::from(100),
        };
        // Both parcels stand at a gain (price 20 vs cost 10/unit); only the
        // older one has cleared 12 months as at the sale date.
        let sale_date = ymd(2026, 6, 15);
        let parcels = vec![parcel(1, ymd(2025, 6, 15)), parcel(2, ymd(2025, 6, 14))];
        let picks = allocate_strategy(
            &parcels,
            Decimal::from(20),
            Decimal::from(20),
            sale_date,
            Strategy::MaxDiscount,
        );
        assert_eq!(picks, vec![(2, Decimal::from(10)), (1, Decimal::from(10))]);
    }

    // ---- disposal figures ------------------------------------------------

    #[tokio::test]
    async fn disposal_totals_split_gains_losses_and_sum_exactly() {
        let pool = test_pool().await;
        strategy_fixture(&pool).await;
        let candidates = fixture_candidates(&pool).await;
        let (units, price, sale_date) = fixture_sale();
        let picks = allocate_strategy(
            candidates.parcels(),
            units,
            price,
            sale_date,
            Strategy::Fifo,
        );
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
            1,
            ymd(2023, 1, 3),
            Decimal::from(100),
            Decimal::ONE,
        )
        .await;
        insert_buy(
            &pool,
            2,
            1,
            ymd(2026, 3, 2),
            Decimal::from(100),
            Decimal::ONE,
        )
        .await;
        let candidates = db_candidate_parcels(&pool, 1, None, sale_date)
            .await
            .unwrap();
        let picks = vec![(1, Decimal::from(30)), (2, Decimal::from(20))];
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
                1,
                ymd(2023, 1, 3),
                Decimal::ONE,
                Decimal::from(100),
            )
            .await;
        }
        let candidates = db_candidate_parcels(&pool, 1, None, ymd(2026, 1, 1))
            .await
            .unwrap();
        let picks: Vec<(i64, Decimal)> = (1..=3).map(|id| (id, Decimal::ONE)).collect();
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
        insert_buy(&pool, 1, 1, ymd(2024, 1, 2), Decimal::from(100), dec("5")).await;
        insert_buy(&pool, 2, 2, ymd(2024, 1, 2), Decimal::from(100), dec("5")).await;
        // Fully sell parcel 1's sibling parcel 3 so it drops out.
        insert_buy(&pool, 3, 1, ymd(2024, 2, 1), Decimal::from(40), dec("5")).await;
        test_support::sell(4, 1)
            .date(ymd(2025, 6, 2))
            .qty(Decimal::from(40))
            .price(Decimal::from(6))
            .insert(&pool)
            .await;
        allocate(&pool, 1, 4, 3, Decimal::from(40)).await;

        let today = crate::infra::date::today();
        let parcels = db_candidate_parcels(&pool, 1, Some(1), today)
            .await
            .unwrap();
        assert_eq!(parcels.parcels().len(), 1);
        assert_eq!(parcels.parcels()[0].trade_id, 1);
        // Wrong account: nothing.
        let parcels = db_candidate_parcels(&pool, 1, Some(99), today)
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
            insert_buy(pool, id, 1, date, Decimal::from(100), dec(price)).await;
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
        let (status, body) = post_optimiser(
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

        let picks = |strategy: Strategy| -> Vec<(i64, Decimal)> {
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
            vec![(1, Decimal::from(100)), (2, Decimal::from(50))]
        );
        assert_eq!(
            picks(Strategy::MaxDiscount),
            vec![(2, Decimal::from(100)), (1, Decimal::from(50))]
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
        let (status, body) = post_optimiser(
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
                    a.allocation.purchase_trade_id,
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
        let (status, body) = post_optimiser(
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
        let (status, body) = post_optimiser(
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
        let live = db_candidate_parcels(&pool, 1, None, crate::infra::date::today())
            .await
            .unwrap();
        assert!(live.parcels().iter().all(|p| p.trade_id != 2));

        // …but it was open on 2023-06-15, so a request for that date has it.
        let (status, body) = post_optimiser(
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
        insert_buy(&pool, 1, 1, ymd(2020, 1, 2), Decimal::from(100), dec("5")).await;
        crate::entities::corporate_action::db_upsert(
            &pool,
            &crate::entities::corporate_action::CorporateAction {
                id: 1,
                listing_id: 1,
                date: ymd(2025, 1, 2),
                kind: crate::entities::corporate_action::ActionKind::ShareSplit {
                    split_new_units: Decimal::from(2),
                    split_old_units: Decimal::ONE,
                },
            },
        )
        .await
        .unwrap();

        let before = db_candidate_parcels(&pool, 1, None, ymd(2024, 6, 30))
            .await
            .unwrap();
        assert_eq!(before.parcels()[0].remaining_quantity, Decimal::from(100));
        let after = db_candidate_parcels(&pool, 1, None, crate::infra::date::today())
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
        let today = crate::infra::date::today();
        let later = today + chrono::Duration::days(400);
        let run = async |date: NaiveDate| {
            let (status, body) = post_optimiser(
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
            let ids: std::collections::BTreeSet<i64> = r
                .allocations
                .iter()
                .filter(|a| a.strategy == Strategy::Fifo)
                .map(|a| a.allocation.purchase_trade_id)
                .collect();
            assert_eq!(ids, (1..=4).collect::<std::collections::BTreeSet<i64>>());
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

    async fn post_optimiser(
        pool: SqlitePool,
        fetcher: Option<SharedFetcher>,
        body: serde_json::Value,
    ) -> (StatusCode, Vec<u8>) {
        let mut router = router().with_state(pool);
        if let Some(f) = fetcher {
            router = router.layer(Extension(f));
        }
        let resp = ApiClient::over(router)
            .post("/portfolio/parcel-optimiser", &body)
            .await;
        let status = resp.status;
        (status, resp.body.to_vec())
    }

    #[tokio::test]
    async fn api_explicit_price_returns_all_strategies() {
        let pool = test_pool().await;
        strategy_fixture(&pool).await;
        let (status, body) = post_optimiser(
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
        let (status, body) = post_optimiser(
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
        assert_eq!(r.taxpayer_basis, crate::reports::TAXPAYER_BASIS);
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
        let (status, body) = post_optimiser(
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
        let (status, body) = post_optimiser(
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
        let (status, body) = post_optimiser(
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
        let (status, body) = post_optimiser(
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

        let sells = ApiClient::over(crate::entities::sell::router().with_state(pool.clone()));
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
            assert_eq!(
                resp.status,
                StatusCode::NO_CONTENT,
                "{strategy:?}: {}",
                resp.text()
            );
        }
    }

    #[tokio::test]
    async fn api_non_positive_units_rejected() {
        let pool = test_pool().await;
        strategy_fixture(&pool).await;
        let (status, _) = post_optimiser(
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
            1,
            ymd(2023, 1, 3),
            dec("100000000000000"),
            Decimal::ZERO,
        )
        .await;

        let (status, body) = post_optimiser(
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
            1,
            ymd(2023, 1, 3),
            dec("100000000000000"),
            dec("10000000000"),
        )
        .await;

        let (status, body) = post_optimiser(
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
    /// twice over, once as a question (`POST /portfolio/parcel-optimiser`) and
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
    /// **exactly** the Sell it described and read `/portfolio/realised-gains`
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
            .post_json(
                "/portfolio/parcel-optimiser",
                &serde_json::json!({
                    "listing_id": 1,
                    "holding_account_id": 1,
                    "units": r.units.to_string(),
                    "sale_date": r.sale_date.to_string(),
                    "price": r.price.to_string(),
                }),
            )
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
                p.purchase_trade_id,
                p.units,
            )
            .await;
        }

        let realised: Vec<crate::reports::realised_gains::RealisedGainLoss> =
            client.get_json("/portfolio/realised-gains").await;
        let recorded = realised
            .iter()
            .find(|row| row.sale_trade_id == SELL_ID)
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
            insert_buy(pool, 1, 1, ymd(2024, 1, 2), dec("100"), dec("10")).await;
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
            insert_buy(pool, 1, 1, ymd(2024, 1, 15), dec("100"), dec("60")).await;
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
            insert_buy(pool, 1, 1, ymd(2024, 1, 15), dec("100"), dec("60")).await;
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
            insert_buy(pool, 1, 1, ymd(2024, 1, 15), dec("100"), dec("60")).await;
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
            insert_buy(pool, 1, 1, ymd(2024, 1, 15), dec("100"), dec("60")).await;
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
            insert_buy(pool, 1, 1, ymd(2024, 1, 15), dec("100"), dec("60")).await;
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
            insert_buy(pool, 1, 1, ymd(2024, 1, 2), dec("100"), dec("10")).await;
            crate::entities::corporate_action::db_upsert(
                pool,
                &crate::entities::corporate_action::CorporateAction {
                    id: 1,
                    listing_id: 1,
                    date: ymd(2025, 3, 3),
                    kind: crate::entities::corporate_action::ActionKind::ReturnOfCapital {
                        amount_per_unit: dec("0.15"),
                        currency: "AUD".to_string(),
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
            insert_buy(pool, 1, 1, ymd(2024, 1, 2), dec("100"), dec("10")).await;
            crate::entities::corporate_action::db_upsert(
                pool,
                &crate::entities::corporate_action::CorporateAction {
                    id: 1,
                    listing_id: 1,
                    date: ymd(2025, 1, 2),
                    kind: crate::entities::corporate_action::ActionKind::ShareSplit {
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
            insert_buy(pool, 1, 1, ymd(2023, 2, 1), dec("100"), dec("40")).await;
            insert_buy(pool, 2, 1, ymd(2024, 1, 15), dec("100"), dec("60")).await;
            crate::entities::corporate_action::db_upsert(
                pool,
                &crate::entities::corporate_action::CorporateAction {
                    id: 1,
                    listing_id: 1,
                    date: ymd(2025, 3, 3),
                    kind: crate::entities::corporate_action::ActionKind::ReturnOfCapital {
                        amount_per_unit: dec("0.25"),
                        currency: "AUD".to_string(),
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
            insert_buy(pool, 1, 1, ymd(2024, 1, 15), dec("100"), dec("60")).await;
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
        insert_buy(&pool, 1, 1, ymd(2023, 2, 1), dec("100"), dec("60.00")).await;
        insert_buy(&pool, 2, 1, ymd(2023, 2, 2), dec("100"), dec("59.90")).await;
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
            .post_json(
                "/portfolio/parcel-optimiser",
                &serde_json::json!({
                    "listing_id": 1, "holding_account_id": 1, "units": "100",
                    "sale_date": "2026-03-02", "price": "70"
                }),
            )
            .await;
        let min_gain: Vec<i64> = estimate
            .allocations
            .iter()
            .filter(|a| a.strategy == Strategy::MinGain)
            .map(|a| a.allocation.purchase_trade_id)
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
        insert_buy(&pool, 1, 1, ymd(2024, 1, 15), dec("100"), dec("60")).await;
        amma_with_generated_adjustments(&client, &pool, 1, ymd(2026, 6, 30), dec("100"), "1.30")
            .await;

        let what_if: crate::reports::net_capital_gain::WhatIfResponse = client
            .post_json(
                "/portfolio/net-capital-gain/what-if",
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
}
