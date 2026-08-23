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
//! The candidate parcels are the open-parcels report's rows **as at the
//! contemplated sale's date** (units in that date's basis, adjusted AUD cost
//! base) — the same set a real Sell dated then could allocate — so every
//! cost-base rule — AMIT/E10, return of capital/G1, splits, rollover carried
//! dates — flows through unchanged. The
//! hypothetical sale carries no brokerage (it isn't known yet), and the
//! 12-month discount clock is the shared ownership rule
//! (`domain::cgt_discount`), run from the parcel's (possibly deemed)
//! acquisition date to the sale date — the same rule, not a matching copy of
//! it, as the realised report applies.

use crate::domain::cgt_discount::discount_eligible;
use crate::entities::closing_price::{self, SharedFetcher};
use crate::infra::decimal::mul_div;
use crate::infra::http::ApiError;
use crate::reports::open_parcels::{self, OpenParcel};
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
    /// Adjusted cost base of the remaining units, AUD.
    pub remaining_cost_base: Decimal,
}

impl CandidateParcel {
    fn from_open(p: &OpenParcel) -> Self {
        CandidateParcel {
            trade_id: p.trade_id,
            holding_account_id: p.holding_account_id,
            acquisition_date: p.acquisition_date,
            remaining_quantity: p.remaining_quantity,
            remaining_cost_base: p.remaining_cost_base,
        }
    }
}

/// The open parcels a hypothetical sale of `listing_id` **as at `as_of`** can
/// draw on, optionally restricted to one holding account (a real Sell's
/// allocations may only consume its own account's parcels), in FIFO order.
///
/// `as_of` is the contemplated sale's own date (`None` = the live view, as at
/// today): the candidates are the parcels open on that date, which is exactly
/// the set a real Sell dated then could allocate — a parcel acquired after it
/// does not exist yet (the Sell path refuses such an allocation outright), and
/// one sold since was still there to sell. Quantities come back in that date's
/// unit basis (`docs/API.md`'s As-at date section), so the caller's `units`
/// and per-unit price must be on that basis too.
pub async fn db_candidate_parcels(
    pool: &SqlitePool,
    listing_id: i64,
    holding_account_id: Option<i64>,
    as_of: Option<NaiveDate>,
) -> Result<Vec<CandidateParcel>, sqlx::Error> {
    let mut tx = pool.begin().await?;
    let parcels = db_candidate_parcels_on(&mut tx, listing_id, holding_account_id, as_of).await?;
    tx.commit().await?;
    Ok(parcels)
}

/// The same candidates read on the caller's own connection, for the what-if,
/// which folds them into its wider single-snapshot read transaction.
pub async fn db_candidate_parcels_on(
    conn: &mut sqlx::SqliteConnection,
    listing_id: i64,
    holding_account_id: Option<i64>,
    as_of: Option<NaiveDate>,
) -> Result<Vec<CandidateParcel>, sqlx::Error> {
    let mut parcels: Vec<CandidateParcel> = open_parcels::db_open_parcels_on(conn, as_of)
        .await?
        .iter()
        .filter(|p| {
            p.listing_id == listing_id
                && holding_account_id.is_none_or(|a| p.holding_account_id == a)
        })
        .map(CandidateParcel::from_open)
        .collect();
    parcels.sort_by_key(|p| (p.acquisition_date, p.trade_id));
    Ok(parcels)
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
pub fn disposal_figures(
    parcels: &[CandidateParcel],
    picks: &[(i64, Decimal)],
    total_proceeds: Decimal,
    total_units: Decimal,
    sale_date: NaiveDate,
) -> (Vec<HypotheticalAllocation>, DisposalTotals) {
    let by_id: HashMap<i64, &CandidateParcel> = parcels.iter().map(|p| (p.trade_id, p)).collect();
    let mut allocations = Vec::with_capacity(picks.len());
    let mut totals = DisposalTotals::default();
    let mut units_so_far = Decimal::ZERO;
    let mut proceeds_so_far = Decimal::ZERO;
    for &(trade_id, units) in picks {
        let p = by_id[&trade_id];
        units_so_far += units;
        let proceeds = mul_div(&[total_proceeds, units_so_far], total_units) - proceeds_so_far;
        proceeds_so_far += proceeds;
        let cost_base = mul_div(&[p.remaining_cost_base, units], p.remaining_quantity);
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
    (allocations, totals)
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
    let parcels = db_candidate_parcels(
        &pool,
        req.listing_id,
        Some(req.holding_account_id),
        Some(sale_date),
    )
    .await
    .map_err(ApiError::from)?;
    let open: Decimal = parcels.iter().map(|p| p.remaining_quantity).sum();
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
        let picks = allocate_strategy(&parcels, req.units, price, sale_date, strategy);
        let (allocs, totals) =
            disposal_figures(&parcels, &picks, total_proceeds, req.units, sale_date);
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

    async fn fixture_picks(pool: &SqlitePool, strategy: Strategy) -> Vec<(i64, Decimal)> {
        let parcels = db_candidate_parcels(pool, 1, None, None).await.unwrap();
        let (units, price, sale_date) = fixture_sale();
        allocate_strategy(&parcels, units, price, sale_date, strategy)
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
        let parcels = db_candidate_parcels(&pool, 1, None, None).await.unwrap();
        let (units, price, sale_date) = fixture_sale();
        let picks = allocate_strategy(&parcels, units, price, sale_date, Strategy::Fifo);
        let (allocs, totals) = disposal_figures(&parcels, &picks, price * units, units, sale_date);
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
    #[test]
    fn a_partly_consumed_parcel_carries_one_eligibility_flag() {
        let sale_date = ymd(2026, 6, 15);
        let parcel = |trade_id, acquired| CandidateParcel {
            trade_id,
            holding_account_id: 1,
            acquisition_date: acquired,
            remaining_quantity: Decimal::from(100),
            // $1/unit, so a $2/unit sale is a clean $1/unit gain.
            remaining_cost_base: Decimal::from(100),
        };
        let parcels = vec![
            parcel(1, ymd(2023, 1, 3)), // well over 12 months
            parcel(2, ymd(2026, 3, 2)), // under 12 months
        ];
        let picks = vec![(1, Decimal::from(30)), (2, Decimal::from(20))];
        let (allocs, totals) = disposal_figures(
            &parcels,
            &picks,
            Decimal::from(100), // 50 units at $2
            Decimal::from(50),
            sale_date,
        );

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
    #[test]
    fn disposal_proceeds_sum_exactly_to_the_total() {
        let parcels: Vec<CandidateParcel> = (1..=3)
            .map(|id| CandidateParcel {
                trade_id: id,
                holding_account_id: 1,
                acquisition_date: ymd(2023, 1, 3),
                remaining_quantity: Decimal::ONE,
                remaining_cost_base: Decimal::from(100),
            })
            .collect();
        let picks: Vec<(i64, Decimal)> = (1..=3).map(|id| (id, Decimal::ONE)).collect();
        let total = Decimal::from(1000);
        let (allocs, totals) =
            disposal_figures(&parcels, &picks, total, Decimal::from(3), ymd(2026, 1, 1));
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

        let parcels = db_candidate_parcels(&pool, 1, Some(1), None).await.unwrap();
        assert_eq!(parcels.len(), 1);
        assert_eq!(parcels[0].trade_id, 1);
        // Wrong account: nothing.
        let parcels = db_candidate_parcels(&pool, 1, Some(99), None)
            .await
            .unwrap();
        assert!(parcels.is_empty());
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
        let live = db_candidate_parcels(&pool, 1, None, None).await.unwrap();
        assert!(live.iter().all(|p| p.trade_id != 2));

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

        let before = db_candidate_parcels(&pool, 1, None, Some(ymd(2024, 6, 30)))
            .await
            .unwrap();
        assert_eq!(before[0].remaining_quantity, Decimal::from(100));
        let after = db_candidate_parcels(&pool, 1, None, None).await.unwrap();
        assert_eq!(after[0].remaining_quantity, Decimal::from(200));
        // The split never touches the cost base (TD 2000/10).
        assert_eq!(before[0].remaining_cost_base, after[0].remaining_cost_base);
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
}
