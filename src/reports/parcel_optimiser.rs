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
//! The candidate parcels are the open-parcels report's rows (current units,
//! adjusted AUD cost base), so every cost-base rule — AMIT/E10, return of
//! capital/G1, splits, rollover carried dates — flows through unchanged. The
//! hypothetical sale carries no brokerage (it isn't known yet), and the
//! 12-month discount clock is the realised report's rule: strictly more than
//! 12 months from the (possibly deemed) acquisition date to the sale date.

use crate::entities::closing_price::{self, SharedFetcher};
use crate::infra::http::ApiError;
use crate::reports::open_parcels::{self, OpenParcel};
use axum::{Extension, Json, Router, extract::State, routing::post};
use chrono::{Months, NaiveDate};
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

/// The open parcels a hypothetical sale of `listing_id` can draw on,
/// optionally restricted to one holding account (a real Sell's allocations
/// may only consume its own account's parcels), in FIFO order.
pub async fn db_candidate_parcels(
    pool: &SqlitePool,
    listing_id: i64,
    holding_account_id: Option<i64>,
) -> Result<Vec<CandidateParcel>, sqlx::Error> {
    let mut tx = pool.begin().await?;
    let parcels = db_candidate_parcels_on(&mut tx, listing_id, holding_account_id).await?;
    tx.commit().await?;
    Ok(parcels)
}

/// The same candidates read on the caller's own connection, for the what-if,
/// which folds them into its wider single-snapshot read transaction.
pub async fn db_candidate_parcels_on(
    conn: &mut sqlx::SqliteConnection,
    listing_id: i64,
    holding_account_id: Option<i64>,
) -> Result<Vec<CandidateParcel>, sqlx::Error> {
    let mut parcels: Vec<CandidateParcel> = open_parcels::db_open_parcels_on(conn)
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

/// 12-month CGT discount clock, as the realised-gains report applies it:
/// eligible when held *strictly* more than 12 months.
fn discount_eligible(acquired: NaiveDate, sale_date: NaiveDate) -> bool {
    sale_date > acquired + Months::new(12)
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
        let proceeds = total_proceeds * units_so_far / total_units - proceeds_so_far;
        proceeds_so_far += proceeds;
        let cost_base = p.remaining_cost_base * units / p.remaining_quantity;
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
pub struct OptimiserRequest {
    pub listing_id: i64,
    /// The account the contemplated Sell would happen in — a real Sell's
    /// allocations may only consume its own account's parcels.
    pub holding_account_id: i64,
    pub units: Decimal,
    /// Defaults to today.
    #[serde(default)]
    pub sale_date: Option<NaiveDate>,
    /// Per-unit price in AUD. Absent → live-fetched from the price source
    /// (the live-valuation rules); an explicit price wins.
    #[serde(default)]
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

    let parcels = db_candidate_parcels(&pool, req.listing_id, Some(req.holding_account_id))
        .await
        .map_err(ApiError::from)?;
    let open: Decimal = parcels.iter().map(|p| p.remaining_quantity).sum();
    if req.units > open {
        return Err(ApiError::Unprocessable(format!(
            "only {open} unit(s) of listing {} are open in holding account {}",
            req.listing_id, req.holding_account_id
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
                        "no live price for listing {}: {reason} — supply a price",
                        req.listing_id
                    )));
                }
                None => {
                    return Err(ApiError::Unprocessable(format!(
                        "no live price for listing {} — supply a price",
                        req.listing_id
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
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{self, allocate, dec, test_pool, ymd};
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
            (1, ymd(2023, 1, 1), "5"),
            (2, ymd(2024, 1, 1), "12"),
            (3, ymd(2025, 1, 1), "8"),
            (4, ymd(2026, 3, 1), "4"),
        ] {
            insert_buy(pool, id, 1, date, Decimal::from(100), dec(price)).await;
        }
    }

    fn fixture_sale() -> (Decimal, Decimal, NaiveDate) {
        (Decimal::from(150), Decimal::from(10), ymd(2026, 6, 15))
    }

    async fn fixture_picks(pool: &SqlitePool, strategy: Strategy) -> Vec<(i64, Decimal)> {
        let parcels = db_candidate_parcels(pool, 1, None).await.unwrap();
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

    /// The 12-month clock is strict: exactly 12 months is not eligible, one
    /// day more is (the realised-gains report's rule).
    #[test]
    fn discount_window_edge_is_strictly_more_than_12_months() {
        assert!(!discount_eligible(ymd(2025, 6, 15), ymd(2026, 6, 15)));
        assert!(discount_eligible(ymd(2025, 6, 15), ymd(2026, 6, 16)));
    }

    // ---- disposal figures ------------------------------------------------

    #[tokio::test]
    async fn disposal_totals_split_gains_losses_and_sum_exactly() {
        let pool = test_pool().await;
        strategy_fixture(&pool).await;
        let parcels = db_candidate_parcels(&pool, 1, None).await.unwrap();
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

    /// Proceeds spread by cumulative difference sum exactly to the total
    /// even when the division doesn't terminate (1000/3 per unit).
    #[test]
    fn disposal_proceeds_sum_exactly_to_the_total() {
        let parcels: Vec<CandidateParcel> = (1..=3)
            .map(|id| CandidateParcel {
                trade_id: id,
                holding_account_id: 1,
                acquisition_date: ymd(2023, 1, 1),
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
        insert_buy(&pool, 1, 1, ymd(2024, 1, 1), Decimal::from(100), dec("5")).await;
        insert_buy(&pool, 2, 2, ymd(2024, 1, 1), Decimal::from(100), dec("5")).await;
        // Fully sell parcel 1's sibling parcel 3 so it drops out.
        insert_buy(&pool, 3, 1, ymd(2024, 2, 1), Decimal::from(40), dec("5")).await;
        test_support::sell(4, 1)
            .date(ymd(2025, 6, 1))
            .qty(Decimal::from(40))
            .price(Decimal::from(6))
            .insert(&pool)
            .await;
        allocate(&pool, 1, 4, 3, Decimal::from(40)).await;

        let parcels = db_candidate_parcels(&pool, 1, Some(1)).await.unwrap();
        assert_eq!(parcels.len(), 1);
        assert_eq!(parcels[0].trade_id, 1);
        // Wrong account: nothing.
        let parcels = db_candidate_parcels(&pool, 1, Some(99)).await.unwrap();
        assert!(parcels.is_empty());
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
        let resp = router
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/portfolio/parcel-optimiser")
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
}
