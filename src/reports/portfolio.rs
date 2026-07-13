use crate::domain::cost_base::{self, ParcelRow};
use crate::entities::closing_price::{self, SharedFetcher};
use crate::infra::decimal::parse_dec;
use crate::infra::fx::FxRates;
use crate::infra::http::ApiError;
use axum::{Extension, Json, Router, extract::State, routing::post};
use chrono::NaiveDate;
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use sqlx::{Row, SqlitePool};
use std::collections::HashMap;

/// Cost base figures are in AUD (each parcel converted via the ATO FX rate). The
/// supplied `current_price` is taken as AUD too, so `market_value` is AUD.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HoldingOverview {
    pub listing_id: i64,
    /// The holding account the parcels sit in: the same listing held in two
    /// accounts (e.g. an employer share plan and a personal broker account)
    /// reports as two holdings.
    pub holding_account_id: i64,
    pub quantity: Decimal,
    pub avg_cost_base_per_unit: Decimal,
    pub total_cost_base: Decimal,
    pub current_price: Option<Decimal>,
    pub market_value: Option<Decimal>,
    /// The price source's quote timestamp (the "as at" moment) when
    /// `current_price` came from a live fetch; absent for an explicitly
    /// supplied price or a stored snapshot.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub price_as_of: Option<String>,
    /// Why a live price could not be obtained for this holding: it is left
    /// unvalued (no `current_price`/`market_value`) with the reason, rather
    /// than silently zeroed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub price_unavailable: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
pub struct OverviewRequest {
    /// Current price per unit by listing id, expected in AUD so it lines up with
    /// the AUD-denominated cost base. An explicit price always overrides a
    /// live-fetched one.
    #[serde(default)]
    pub prices: HashMap<i64, Decimal>,
    /// Fetch the current price live from the price source for every held
    /// listing without an explicit price above. Off by default so existing
    /// callers (and the deterministic ATO acceptance tests) never hit the
    /// network; the web UI sets it.
    #[serde(default)]
    pub live: bool,
}

pub fn router() -> Router<SqlitePool> {
    Router::new().route("/portfolio/overview", post(overview))
}

/// Returns open holdings per (listing, holding account): quantity, cost base,
/// and optional market value. The same listing held in two accounts reports
/// as two holdings.
///
/// "Open" quantity for a parcel = trade.quantity − sum of parcel_allocations where purchase_trade_id = trade.id
/// (each allocation re-based to the parcel's as-acquired units — a post-split
/// sale allocates post-split units). The reported quantity is in *current*
/// units (after every recorded share split/consolidation) so it lines up with
/// a current market price; cost base totals are unaffected by splits
/// (TD 2000/10 — only the per-unit figure scales).
/// Cost base is pro-rated to remaining units and reduced by any AMIT adjustments
/// and return-of-capital payments (CGT event G1) received since acquisition.
///
/// With `as_of` the holdings are taken as at that date: trades, sales,
/// corporate actions, and AMIT adjustments (by their statement's year end)
/// dated after it are excluded, and quantities are in that date's unit basis —
/// snapshot generation values a past day's actual position. `None` is the live
/// view (every recorded fact, current units).
pub async fn db_holdings(
    pool: &SqlitePool,
    as_of: Option<NaiveDate>,
) -> Result<Vec<HoldingOverview>, sqlx::Error> {
    // One read transaction: every input comes from the same snapshot, so an
    // interleaved write can't yield e.g. an allocation whose parcel is
    // missing from the same read.
    let mut tx = pool.begin().await?;
    let holdings = db_holdings_on(&mut tx, as_of).await?;
    tx.commit().await?;
    Ok(holdings)
}

/// The same holdings read on the caller's own connection, for reports (the
/// listing activity ledger) that fold the overview into their wider
/// single-snapshot read transaction.
pub async fn db_holdings_on(
    conn: &mut sqlx::SqliteConnection,
    as_of: Option<NaiveDate>,
) -> Result<Vec<HoldingOverview>, sqlx::Error> {
    let cutoff = crate::infra::date::as_of_or_open(as_of);
    let trade_rows: Vec<ParcelRow> = sqlx::query_as(&format!(
        "SELECT {} FROM trades WHERE trade_type IN ('Buy', 'DRP') AND date <= ?",
        ParcelRow::COLUMNS
    ))
    .bind(cutoff)
    .fetch_all(&mut *conn)
    .await?;

    if trade_rows.is_empty() {
        // Nothing held.
        return Ok(vec![]);
    }

    // units sold per purchase parcel, with each sale's date so the allocated
    // quantity (in sale-date units) can be re-based across splits
    let alloc_rows = sqlx::query(
        "SELECT pa.purchase_trade_id, pa.quantity_allocated, s.date AS sale_date \
         FROM parcel_allocations pa JOIN trades s ON s.id = pa.sale_trade_id \
         WHERE s.date <= ?",
    )
    .bind(cutoff)
    .fetch_all(&mut *conn)
    .await?;

    let mut qty_sold: HashMap<i64, Vec<(NaiveDate, Decimal)>> = HashMap::new();
    for row in &alloc_rows {
        let tid: i64 = row.try_get("purchase_trade_id")?;
        qty_sold.entry(tid).or_default().push((
            row.try_get("sale_date")?,
            parse_dec("quantity_allocated", row.try_get("quantity_allocated")?)?,
        ));
    }

    // total AMIT cost base reduction per purchase parcel (statements for
    // years ending after `as_of` excluded)
    let cba_reduction =
        crate::entities::amit_adjustment::db_cost_base_reductions_up_to(&mut *conn, as_of).await?;
    // return-of-capital payments (CGT event G1) per listing
    let roc_events =
        crate::entities::corporate_action::db_return_of_capital_events(&mut *conn).await?;
    // share splits/consolidations per listing (quantity re-basing)
    let split_events = crate::entities::corporate_action::db_share_split_events(&mut *conn).await?;
    // every imported ATO FX rate — per-parcel conversions below are map
    // lookups, not one DB round-trip each
    let fx = FxRates::load(&mut *conn).await?;

    let mut holding_qty: HashMap<(i64, i64), Decimal> = HashMap::new();
    let mut holding_cost_base: HashMap<(i64, i64), Decimal> = HashMap::new();

    for t in &trade_rows {
        let splits = split_events.get(&t.listing_id).map_or(&[][..], |v| v);
        // Internal cost-base arithmetic stays in the parcel's as-acquired units;
        // each sale's allocated quantity is re-based back across any splits.
        let sold = crate::entities::corporate_action::sold_in_acquired_units(
            qty_sold.get(&t.id).map_or(&[][..], |v| v),
            splits,
            t.date,
        );
        let remaining = t.quantity - sold;
        if remaining <= Decimal::ZERO {
            continue;
        }

        // Adjusted cost base of the remaining units via the shared pipeline
        // (`domain::cost_base`), converted to AUD at the (possibly deemed)
        // acquisition month so holdings aggregate in AUD. `up_to` is the
        // report's as-of date: payments after it haven't happened yet in
        // this view.
        let remaining_cost = cost_base::adjusted_cost_base(
            &t.parcel(),
            remaining,
            *cba_reduction.get(&t.id).unwrap_or(&Decimal::ZERO),
            roc_events.get(&t.listing_id).map_or(&[][..], |v| v),
            splits,
            as_of,
        )?
        .into_aud_with(&fx, &t.currency, t.acquired(), t.fx_override())?
        .adjusted;

        // The holding's quantity is reported in the unit basis of `as_of`
        // (live view: current units, after every recorded split) so market
        // value lines up with a price as of that date.
        let remaining_now = crate::entities::corporate_action::split_adjusted_quantity(
            remaining, splits, t.date, as_of,
        );
        *holding_qty
            .entry((t.listing_id, t.holding_account_id))
            .or_insert(Decimal::ZERO) += remaining_now;
        *holding_cost_base
            .entry((t.listing_id, t.holding_account_id))
            .or_insert(Decimal::ZERO) += remaining_cost;
    }

    let mut result: Vec<HoldingOverview> = holding_qty
        .into_iter()
        .filter(|(_, qty)| *qty > Decimal::ZERO)
        .map(|((listing_id, holding_account_id), qty)| {
            let cost_base = holding_cost_base
                .get(&(listing_id, holding_account_id))
                .copied()
                .unwrap_or(Decimal::ZERO);
            let avg = if qty > Decimal::ZERO {
                cost_base / qty
            } else {
                Decimal::ZERO
            };
            HoldingOverview {
                listing_id,
                holding_account_id,
                quantity: qty,
                avg_cost_base_per_unit: avg,
                total_cost_base: cost_base,
                current_price: None,
                market_value: None,
                price_as_of: None,
                price_unavailable: None,
            }
        })
        .collect();

    result.sort_by_key(|h| (h.listing_id, h.holding_account_id));
    Ok(result)
}

async fn overview(
    State(pool): State<SqlitePool>,
    fetcher: Option<Extension<SharedFetcher>>,
    body: Option<Json<OverviewRequest>>,
) -> Result<Json<Vec<HoldingOverview>>, ApiError> {
    let req = body.map(|Json(req)| req).unwrap_or_default();
    let mut holdings = db_holdings(&pool, None).await.map_err(ApiError::from)?;

    // Live-fetch a current price for every held listing without an explicit
    // override (when requested); an explicit price always wins.
    let live = closing_price::resolve_live_prices(
        &pool,
        fetcher.as_ref().map(|f| f.0.as_ref()),
        req.live,
        &req.prices,
        holdings.iter().map(|h| h.listing_id),
    )
    .await
    .map_err(ApiError::from)?;

    for h in &mut holdings {
        if let Some(&price) = req.prices.get(&h.listing_id) {
            h.current_price = Some(price);
            h.market_value = Some(h.quantity * price);
        } else if let Some(result) = live.get(&h.listing_id) {
            match result {
                Ok(v) => {
                    h.current_price = Some(v.aud_price);
                    h.market_value = Some(h.quantity * v.aud_price);
                    h.price_as_of = Some(v.as_of.clone());
                }
                Err(reason) => h.price_unavailable = Some(reason.clone()),
            }
        }
    }

    Ok(Json(holdings))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::entities::{amit_adjustment, amma, corporate_action, trade};
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

    async fn insert_buy(pool: &SqlitePool, id: i64, listing_id: i64, qty: Decimal, price: Decimal) {
        test_support::buy(id, listing_id)
            .qty(qty)
            .price(price)
            .brokerage(dec("9.95"))
            .gst_on_brokerage(dec("0.995"))
            .insert(pool)
            .await;
    }

    async fn insert_sell(pool: &SqlitePool, id: i64, listing_id: i64, qty: Decimal) {
        test_support::sell(id, listing_id)
            .date(ymd(2024, 6, 1))
            .qty(qty)
            .price(Decimal::from(120))
            .brokerage(dec("9.95"))
            .gst_on_brokerage(dec("0.995"))
            .insert(pool)
            .await;
    }

    fn make_amma(id: i64, listing_id: i64, cba: Decimal) -> amma::AmmaStatement {
        test_support::amma(id, listing_id)
            .cost_base_adjustment(cba)
            .build()
    }

    // DB-level tests

    #[tokio::test]
    async fn db_no_trades_returns_empty() {
        let pool = test_pool().await;
        let holdings = db_holdings(&pool, None).await.unwrap();
        assert!(holdings.is_empty());
    }

    #[tokio::test]
    async fn db_malformed_decimal_is_an_error_not_zero() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "VAS").await;
        // Inject a row with a non-numeric average_price directly, bypassing the typed upsert.
        sqlx::query(
            "INSERT INTO trades (id, trade_type, date, settlement_date, listing_id, \
             average_price, quantity, currency, brokerage, gst_on_brokerage, \
             brokerage_currency, fx_rate) \
             VALUES (1, 'Buy', '2024-01-01', '2024-01-03', 1, \
             'not-a-number', '100', 'AUD', '0', '0', 'AUD', '1')",
        )
        .execute(&pool)
        .await
        .unwrap();

        // The malformed value must surface as an error rather than being read as zero.
        assert!(db_holdings(&pool, None).await.is_err());
    }

    #[tokio::test]
    async fn db_single_buy_fully_held() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "VAS").await;
        insert_buy(&pool, 1, 1, Decimal::from(100), Decimal::from(10)).await;

        let holdings = db_holdings(&pool, None).await.unwrap();
        assert_eq!(holdings.len(), 1);
        let h = &holdings[0];
        assert_eq!(h.listing_id, 1);
        assert_eq!(h.quantity, Decimal::from(100));
        // cost = 10 * 100 + 9.95 + 0.995 = 1010.945
        assert_eq!(h.total_cost_base, "1010.945".parse::<Decimal>().unwrap());
        assert_eq!(
            h.avg_cost_base_per_unit,
            "10.10945".parse::<Decimal>().unwrap()
        );
    }

    /// A Buy entered with GST-inclusive brokerage costs exactly what was paid:
    /// the server splits $9.95 incl. into $9.05 + $0.90 at write time
    /// (entered here through the trades API so the split path is exercised),
    /// and the report's cost base — brokerage + GST on top of price × qty —
    /// sums back to the inclusive amount.
    #[tokio::test]
    async fn db_cost_base_of_gst_inclusive_buy_equals_amount_paid() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "VAS").await;
        let body = serde_json::json!({
            "trade_type": "Buy",
            "date": "2024-01-15",
            "listing_id": 1,
            "average_price": "10",
            "quantity": "100",
            "currency": "AUD",
            "brokerage": "9.95",
            "brokerage_includes_gst": true,
            "brokerage_currency": "AUD",
            "fx_rate": "1"
        });
        let resp = trade::router()
            .with_state(pool.clone())
            .oneshot(
                Request::builder()
                    .method("PUT")
                    .uri("/trades/1")
                    .header("content-type", "application/json")
                    .body(Body::from(body.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NO_CONTENT);

        let holdings = db_holdings(&pool, None).await.unwrap();
        // cost = 10 × 100 + 9.95 (the inclusive amount paid) = 1009.95
        assert_eq!(
            holdings[0].total_cost_base,
            "1009.95".parse::<Decimal>().unwrap()
        );
    }

    #[tokio::test]
    async fn db_partial_sell_reduces_holding() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "VAS").await;
        insert_buy(&pool, 1, 1, Decimal::from(100), Decimal::from(10)).await;
        insert_sell(&pool, 2, 1, Decimal::from(40)).await;
        allocate(&pool, 1, 2, 1, Decimal::from(40)).await;

        let holdings = db_holdings(&pool, None).await.unwrap();
        assert_eq!(holdings.len(), 1);
        let h = &holdings[0];
        assert_eq!(h.quantity, Decimal::from(60));
        // remaining_cost = 1010.945 * 60 / 100 = 606.567
        assert_eq!(h.total_cost_base, "606.567".parse::<Decimal>().unwrap());
        assert_eq!(
            h.avg_cost_base_per_unit,
            "10.10945".parse::<Decimal>().unwrap()
        );
    }

    #[tokio::test]
    async fn db_fully_sold_listing_excluded() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "VAS").await;
        insert_buy(&pool, 1, 1, Decimal::from(100), Decimal::from(10)).await;
        insert_sell(&pool, 2, 1, Decimal::from(100)).await;
        allocate(&pool, 1, 2, 1, Decimal::from(100)).await;

        let holdings = db_holdings(&pool, None).await.unwrap();
        assert!(holdings.is_empty());
    }

    #[tokio::test]
    async fn db_amit_adjustment_reduces_cost_base() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "VAF").await;
        insert_buy(&pool, 1, 1, Decimal::from(100), Decimal::from(10)).await;
        amma::db_upsert(&pool, &make_amma(1, 1, "0.05".parse().unwrap()))
            .await
            .unwrap();
        amit_adjustment::db_upsert(
            &pool,
            &amit_adjustment::AmitAdjustment {
                id: 1,
                amma_statement_id: 1,
                trade_id: 1,
                quantity: Decimal::from(100),
            },
        )
        .await
        .unwrap();

        let holdings = db_holdings(&pool, None).await.unwrap();
        assert_eq!(holdings.len(), 1);
        let h = &holdings[0];
        // initial = 1010.945, AMIT = 100 * 0.05 = 5.00, net = 1005.945
        assert_eq!(h.total_cost_base, "1005.945".parse::<Decimal>().unwrap());
        assert_eq!(
            h.avg_cost_base_per_unit,
            "10.05945".parse::<Decimal>().unwrap()
        );
    }

    #[tokio::test]
    async fn db_amit_reduction_capped_at_nil_cost_base() {
        // CGT event E10: a reduction larger than the parcel's cost base floors the
        // cost base at nil rather than going negative (the excess is a capital gain
        // surfaced by the net-capital-gain report).
        let pool = test_pool().await;
        insert_listing(&pool, 1, "VAF").await;
        // initial cost = 1*100 + 9.95 + 0.995 = 110.945
        insert_buy(&pool, 1, 1, Decimal::from(100), Decimal::from(1)).await;
        // reduction = 100 * 1.50 = 150 > 110.945 → cost base floored at 0
        amma::db_upsert(&pool, &make_amma(1, 1, "1.50".parse().unwrap()))
            .await
            .unwrap();
        amit_adjustment::db_upsert(
            &pool,
            &amit_adjustment::AmitAdjustment {
                id: 1,
                amma_statement_id: 1,
                trade_id: 1,
                quantity: Decimal::from(100),
            },
        )
        .await
        .unwrap();

        let holdings = db_holdings(&pool, None).await.unwrap();
        assert_eq!(holdings.len(), 1);
        assert_eq!(holdings[0].total_cost_base, Decimal::ZERO);
        assert_eq!(holdings[0].avg_cost_base_per_unit, Decimal::ZERO);
    }

    #[tokio::test]
    async fn db_multiple_listings_aggregated_separately() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "VAS").await;
        insert_listing(&pool, 2, "VAF").await;
        insert_buy(&pool, 1, 1, Decimal::from(50), Decimal::from(10)).await;
        insert_buy(&pool, 2, 2, Decimal::from(200), Decimal::from(5)).await;

        let holdings = db_holdings(&pool, None).await.unwrap();
        assert_eq!(holdings.len(), 2);
        assert_eq!(holdings[0].listing_id, 1);
        assert_eq!(holdings[0].quantity, Decimal::from(50));
        assert_eq!(holdings[1].listing_id, 2);
        assert_eq!(holdings[1].quantity, Decimal::from(200));
    }

    #[tokio::test]
    async fn db_multiple_parcels_same_listing_aggregated() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "VAS").await;
        // Two buy parcels for the same listing
        insert_buy(&pool, 1, 1, Decimal::from(100), Decimal::from(10)).await;
        insert_buy(&pool, 2, 1, Decimal::from(50), Decimal::from(12)).await;

        let holdings = db_holdings(&pool, None).await.unwrap();
        assert_eq!(holdings.len(), 1);
        let h = &holdings[0];
        assert_eq!(h.quantity, Decimal::from(150));
        // parcel 1: 10*100 + 9.95 + 0.995 = 1010.945
        // parcel 2: 12*50  + 9.95 + 0.995 = 610.945
        // total = 1621.890
        assert_eq!(h.total_cost_base, "1621.890".parse::<Decimal>().unwrap());
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

    /// TD 2000/10 (`docs/ato/share-splits-and-consolidations.md`): a share split
    /// multiplies the unit count and leaves the total cost base unchanged, so
    /// the per-unit cost base scales down proportionately.
    #[tokio::test]
    async fn db_share_split_adjusts_quantity_and_preserves_cost_base() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "SPL").await;
        // Buy 100 @ $10 on 2024-01-01 → cost base 1010.945 (incl. brokerage).
        insert_buy(&pool, 1, 1, Decimal::from(100), Decimal::from(10)).await;
        // 2-for-1 split after acquisition.
        apply_split(
            &pool,
            1,
            1,
            NaiveDate::from_ymd_opt(2024, 3, 1).unwrap(),
            "2",
            "1",
        )
        .await;

        let holdings = db_holdings(&pool, None).await.unwrap();
        assert_eq!(holdings.len(), 1);
        let h = &holdings[0];
        assert_eq!(h.quantity, Decimal::from(200));
        assert_eq!(h.total_cost_base, "1010.945".parse::<Decimal>().unwrap());
        // per-unit cost base halves: 1010.945 / 200
        assert_eq!(
            h.avg_cost_base_per_unit,
            "5.054725".parse::<Decimal>().unwrap()
        );
    }

    /// A non-assessable bonus issue (`docs/ato/bonus-shares.md`) adds the bonus
    /// units and apportions the unchanged cost base over original + bonus
    /// shares — the same re-base as its equivalent split.
    #[tokio::test]
    async fn db_bonus_issue_adds_units_and_apportions_cost_base() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "BON").await;
        // Buy 100 @ $10 on 2024-01-01 → cost base 1010.945 (incl. brokerage).
        insert_buy(&pool, 1, 1, Decimal::from(100), Decimal::from(10)).await;
        // 1-for-10 bonus issue after acquisition: 100 held → 10 bonus units.
        corporate_action::db_upsert(
            &pool,
            &corporate_action::CorporateAction {
                id: 1,
                listing_id: 1,
                date: NaiveDate::from_ymd_opt(2024, 3, 1).unwrap(),
                kind: corporate_action::ActionKind::BonusIssue {
                    bonus_units: Decimal::ONE,
                    bonus_held_units: Decimal::from(10),
                },
            },
        )
        .await
        .unwrap();

        let holdings = db_holdings(&pool, None).await.unwrap();
        assert_eq!(holdings.len(), 1);
        let h = &holdings[0];
        assert_eq!(h.quantity, Decimal::from(110));
        assert_eq!(h.total_cost_base, "1010.945".parse::<Decimal>().unwrap());
        // per-unit cost base apportioned over 110 units: 1010.945 / 110
        assert_eq!(
            h.avg_cost_base_per_unit,
            "1010.945".parse::<Decimal>().unwrap() / Decimal::from(110)
        );
    }

    /// A consolidation is the same action with new < old (TD 2000/10 Example 2).
    #[tokio::test]
    async fn db_consolidation_shrinks_quantity_and_preserves_cost_base() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "CON").await;
        insert_buy(&pool, 1, 1, Decimal::from(100), Decimal::from(10)).await;
        // 1-for-10 consolidation.
        apply_split(
            &pool,
            1,
            1,
            NaiveDate::from_ymd_opt(2024, 3, 1).unwrap(),
            "1",
            "10",
        )
        .await;

        let holdings = db_holdings(&pool, None).await.unwrap();
        assert_eq!(holdings[0].quantity, Decimal::from(10));
        assert_eq!(
            holdings[0].total_cost_base,
            "1010.945".parse::<Decimal>().unwrap()
        );
    }

    /// A sale entered after a split is in post-split units; the holding nets it
    /// off correctly against the pre-split parcel.
    #[tokio::test]
    async fn db_post_split_sell_nets_off_pre_split_parcel() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "SPL").await;
        insert_buy(&pool, 1, 1, Decimal::from(100), Decimal::from(10)).await; // 2024-01-01
        apply_split(
            &pool,
            1,
            1,
            NaiveDate::from_ymd_opt(2024, 3, 1).unwrap(),
            "2",
            "1",
        )
        .await;
        // Sell 80 post-split units (= 40 as-acquired) on 2024-06-01.
        insert_sell(&pool, 2, 1, Decimal::from(80)).await;
        allocate(&pool, 1, 2, 1, Decimal::from(80)).await;

        let holdings = db_holdings(&pool, None).await.unwrap();
        assert_eq!(holdings.len(), 1);
        // 200 post-split − 80 sold = 120 held now.
        assert_eq!(holdings[0].quantity, Decimal::from(120));
        // Cost base of the remaining 60 as-acquired units: 1010.945 × 60/100.
        assert_eq!(
            holdings[0].total_cost_base,
            "606.567".parse::<Decimal>().unwrap()
        );
    }

    /// A split dated before acquisition does not touch the parcel — its trade
    /// quantity is already in post-split units.
    #[tokio::test]
    async fn db_split_before_acquisition_does_not_apply() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "SPL").await;
        apply_split(
            &pool,
            1,
            1,
            NaiveDate::from_ymd_opt(2023, 6, 1).unwrap(),
            "2",
            "1",
        )
        .await;
        insert_buy(&pool, 1, 1, Decimal::from(100), Decimal::from(10)).await; // 2024-01-01

        let holdings = db_holdings(&pool, None).await.unwrap();
        assert_eq!(holdings[0].quantity, Decimal::from(100));
    }

    /// A return of capital after a split is per post-split unit: the reduction
    /// applies to the multiplied unit count.
    #[tokio::test]
    async fn db_return_of_capital_after_split_scales_to_post_split_units() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "SPL").await;
        insert_buy(&pool, 1, 1, Decimal::from(100), Decimal::from(10)).await; // 2024-01-01
        apply_split(
            &pool,
            1,
            1,
            NaiveDate::from_ymd_opt(2024, 3, 1).unwrap(),
            "2",
            "1",
        )
        .await;
        // 25c/unit on the 200 post-split units → 50.00 off the cost base.
        apply_roc(
            &pool,
            2,
            1,
            NaiveDate::from_ymd_opt(2024, 6, 1).unwrap(),
            "0.25",
        )
        .await;

        let holdings = db_holdings(&pool, None).await.unwrap();
        // 1010.945 − 200 × 0.25 = 960.945
        assert_eq!(
            holdings[0].total_cost_base,
            "960.945".parse::<Decimal>().unwrap()
        );
    }

    /// A return of capital (CGT event G1) reduces the holding's cost base by the
    /// per-unit payment for units held on the payment date
    /// (`docs/ato/cgt-non-assessable-payments.md`).
    #[tokio::test]
    async fn db_return_of_capital_reduces_cost_base() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "RAP").await;
        // Buy 100 @ $10 on 2024-01-01 → cost base 1010.945 (incl. brokerage).
        insert_buy(&pool, 1, 1, Decimal::from(100), Decimal::from(10)).await;
        // 50c/unit return of capital while all 100 units are held.
        apply_roc(
            &pool,
            1,
            1,
            NaiveDate::from_ymd_opt(2024, 3, 1).unwrap(),
            "0.50",
        )
        .await;

        let holdings = db_holdings(&pool, None).await.unwrap();
        assert_eq!(holdings.len(), 1);
        // 1010.945 − 100 × 0.50 = 960.945
        assert_eq!(
            holdings[0].total_cost_base,
            "960.945".parse::<Decimal>().unwrap()
        );
    }

    #[tokio::test]
    async fn db_return_of_capital_before_acquisition_does_not_apply() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "RAP").await;
        // Payment made before this parcel was acquired (2024-01-01): unaffected.
        apply_roc(
            &pool,
            1,
            1,
            NaiveDate::from_ymd_opt(2023, 11, 30).unwrap(),
            "0.50",
        )
        .await;
        insert_buy(&pool, 1, 1, Decimal::from(100), Decimal::from(10)).await;

        let holdings = db_holdings(&pool, None).await.unwrap();
        assert_eq!(
            holdings[0].total_cost_base,
            "1010.945".parse::<Decimal>().unwrap()
        );
    }

    /// G1 can never push the cost base below nil: the excess over cost base is a
    /// capital gain in the net-capital-gain report, not a negative cost base here.
    #[tokio::test]
    async fn db_return_of_capital_floors_cost_base_at_nil() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "RAP").await;
        insert_buy(&pool, 1, 1, Decimal::from(100), Decimal::from(10)).await;
        // $11/unit × 100 = 1100 exceeds the 1010.945 cost base.
        apply_roc(
            &pool,
            1,
            1,
            NaiveDate::from_ymd_opt(2024, 3, 1).unwrap(),
            "11",
        )
        .await;

        let holdings = db_holdings(&pool, None).await.unwrap();
        assert_eq!(holdings[0].total_cost_base, Decimal::ZERO);
    }

    /// With `as_of` the overview is the position at that date: later sales
    /// (and purchases) are excluded; `None` is the live view.
    #[tokio::test]
    async fn db_holdings_as_of_a_past_date_excludes_later_facts() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "VAS").await;
        insert_buy(&pool, 1, 1, Decimal::from(100), Decimal::from(10)).await; // 2024-01-01
        insert_sell(&pool, 2, 1, Decimal::from(40)).await; // 2024-06-01
        allocate(&pool, 1, 2, 1, Decimal::from(40)).await;

        let live = db_holdings(&pool, None).await.unwrap();
        assert_eq!(live[0].quantity, Decimal::from(60));
        let as_at = db_holdings(&pool, Some(NaiveDate::from_ymd_opt(2024, 3, 1).unwrap()))
            .await
            .unwrap();
        assert_eq!(
            as_at[0].quantity,
            Decimal::from(100),
            "the June sale hasn't happened yet"
        );
    }

    // API-level tests

    #[tokio::test]
    async fn api_overview_without_prices() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "VAS").await;
        insert_buy(&pool, 1, 1, Decimal::from(100), Decimal::from(10)).await;

        let resp = router()
            .with_state(pool)
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/portfolio/overview")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let holdings: Vec<HoldingOverview> = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(holdings.len(), 1);
        assert_eq!(holdings[0].quantity, Decimal::from(100));
        assert!(holdings[0].current_price.is_none());
        assert!(holdings[0].market_value.is_none());
    }

    #[tokio::test]
    async fn api_overview_with_prices() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "VAS").await;
        insert_buy(&pool, 1, 1, Decimal::from(100), Decimal::from(10)).await;

        let body = serde_json::json!({ "prices": { "1": "120.50" } });
        let resp = router()
            .with_state(pool)
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/portfolio/overview")
                    .header("content-type", "application/json")
                    .body(Body::from(body.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let holdings: Vec<HoldingOverview> = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(holdings.len(), 1);
        assert_eq!(
            holdings[0].current_price,
            Some("120.50".parse::<Decimal>().unwrap())
        );
        // market_value = 100 * 120.50 = 12050.00
        assert_eq!(
            holdings[0].market_value,
            Some("12050.00".parse::<Decimal>().unwrap())
        );
    }

    #[tokio::test]
    async fn api_overview_unknown_listing_price_ignored() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "VAS").await;
        insert_buy(&pool, 1, 1, Decimal::from(100), Decimal::from(10)).await;

        // price for listing 99 (doesn't exist) — should be silently ignored
        let body = serde_json::json!({ "prices": { "99": "50.00" } });
        let resp = router()
            .with_state(pool)
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/portfolio/overview")
                    .header("content-type", "application/json")
                    .body(Body::from(body.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let holdings: Vec<HoldingOverview> = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(holdings.len(), 1);
        assert!(holdings[0].market_value.is_none());
    }

    /// With `live` set and no explicit price, each held listing is valued from
    /// the price source's latest quote, converted to AUD, and the quote's
    /// as-of time rides through to the row.
    #[tokio::test]
    async fn api_overview_live_fetches_prices_and_carries_as_of() {
        use crate::entities::closing_price::test_support::QuoteStub;
        let pool = test_pool().await;
        insert_listing(&pool, 1, "VAS").await;
        insert_buy(&pool, 1, 1, Decimal::from(100), Decimal::from(10)).await;
        let as_of = chrono::TimeZone::with_ymd_and_hms(&chrono::Utc, 2026, 6, 5, 6, 30, 0).unwrap();
        let fetcher = QuoteStub::default()
            .with_quote(1, "12.50", "AUD", as_of)
            .shared();

        let body = serde_json::json!({ "live": true });
        let resp = router()
            .with_state(pool)
            .layer(axum::Extension(fetcher))
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/portfolio/overview")
                    .header("content-type", "application/json")
                    .body(Body::from(body.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let holdings: Vec<HoldingOverview> = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(holdings[0].current_price, Some("12.50".parse().unwrap()));
        assert_eq!(holdings[0].market_value, Some(Decimal::from(1250)));
        assert_eq!(
            holdings[0].price_as_of.as_deref(),
            Some(as_of.to_rfc3339().as_str())
        );
        assert!(holdings[0].price_unavailable.is_none());
    }

    /// An explicit supplied price wins over the live fetch (and is never
    /// fetched); a per-listing live-fetch failure leaves that holding unvalued
    /// with a reason while the rest of the report still values.
    #[tokio::test]
    async fn api_overview_override_wins_and_failure_degrades_gracefully() {
        use crate::entities::closing_price::test_support::QuoteStub;
        let pool = test_pool().await;
        insert_listing(&pool, 1, "VAS").await; // priced live
        insert_listing(&pool, 2, "VAF").await; // explicit override
        insert_listing(&pool, 3, "VGS").await; // live fetch fails (no stub quote)
        insert_buy(&pool, 1, 1, Decimal::from(100), Decimal::from(10)).await;
        insert_buy(&pool, 2, 2, Decimal::from(50), Decimal::from(12)).await;
        insert_buy(&pool, 3, 3, Decimal::from(10), Decimal::from(5)).await;
        let as_of = chrono::TimeZone::with_ymd_and_hms(&chrono::Utc, 2026, 6, 5, 6, 30, 0).unwrap();
        // Stub only quotes listing 1; listing 3 has no quote → graceful failure.
        let fetcher = QuoteStub::default()
            .with_quote(1, "20", "AUD", as_of)
            .shared();

        let body = serde_json::json!({ "live": true, "prices": { "2": "99" } });
        let resp = router()
            .with_state(pool)
            .layer(axum::Extension(fetcher))
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/portfolio/overview")
                    .header("content-type", "application/json")
                    .body(Body::from(body.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let holdings: Vec<HoldingOverview> = serde_json::from_slice(&bytes).unwrap();
        // Listing 1: live-valued.
        assert_eq!(holdings[0].current_price, Some(Decimal::from(20)));
        assert!(holdings[0].price_as_of.is_some());
        // Listing 2: the explicit override, no as-of time.
        assert_eq!(holdings[1].current_price, Some(Decimal::from(99)));
        assert!(holdings[1].price_as_of.is_none());
        // Listing 3: unvalued, with a reason — never a silent zero.
        assert!(holdings[2].current_price.is_none());
        assert!(holdings[2].market_value.is_none());
        assert!(holdings[2].price_unavailable.is_some());
    }

    /// The same listing held in two holding accounts reports as two holdings
    /// (REQUIREMENTS "Holding accounts" — the RSU scenario: vested shares in
    /// the employer plan account alongside shares in the personal account),
    /// each with its own quantity and cost base.
    #[tokio::test]
    async fn db_same_listing_in_two_accounts_reports_as_two_holdings() {
        use crate::entities::holding_account::{self, HoldingAccount};
        let pool = test_pool().await;
        insert_listing(&pool, 1, "ICE").await;
        holding_account::db_upsert(
            &pool,
            &HoldingAccount {
                id: 2,
                name: "ICE Employee Plan".to_string(),
            },
        )
        .await
        .unwrap();
        // 100 @ $10 in the default account, 50 @ $12 in the plan account.
        insert_buy(&pool, 1, 1, Decimal::from(100), Decimal::from(10)).await;
        insert_buy(&pool, 2, 1, Decimal::from(50), Decimal::from(12)).await;
        sqlx::query("UPDATE trades SET holding_account_id = 2 WHERE id = 2")
            .execute(&pool)
            .await
            .unwrap();

        let holdings = db_holdings(&pool, None).await.unwrap();
        assert_eq!(holdings.len(), 2);
        assert_eq!(
            (holdings[0].listing_id, holdings[0].holding_account_id),
            (1, 1)
        );
        assert_eq!(holdings[0].quantity, Decimal::from(100));
        assert_eq!(
            (holdings[1].listing_id, holdings[1].holding_account_id),
            (1, 2)
        );
        assert_eq!(holdings[1].quantity, Decimal::from(50));
        assert!(holdings[1].total_cost_base > Decimal::ZERO);
        assert_ne!(holdings[0].total_cost_base, holdings[1].total_cost_base);
    }

    /// A scrip-for-scrip exchange moves the holding to the replacement
    /// listing: the original listing drops out of the overview and the
    /// replacement appears with the ratio-scaled quantity and the unchanged
    /// total cost base (per-unit average scales inversely).
    #[tokio::test]
    async fn db_scrip_exchange_moves_holding_to_replacement_listing() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "OLD").await;
        insert_listing(&pool, 2, "NEW").await;
        // 100 @ $10 + $9.95 + $0.995 = $1,010.945 cost base.
        insert_buy(&pool, 1, 1, Decimal::from(100), Decimal::from(10)).await;
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
        crate::entities::scrip_exchange::db_exchange(&pool, 10)
            .await
            .unwrap();

        let holdings = db_holdings(&pool, None).await.unwrap();
        assert_eq!(holdings.len(), 1);
        assert_eq!(holdings[0].listing_id, 2);
        assert_eq!(holdings[0].quantity, Decimal::from(200));
        assert_eq!(
            holdings[0].total_cost_base,
            "1010.945".parse::<Decimal>().unwrap()
        );
    }

    /// A demerger splits the holding across both listings: the head listing
    /// keeps its units with the head share of the cost base, the demerged
    /// listing appears with the entitlement units and the rest — total cost
    /// base across the two unchanged.
    #[tokio::test]
    async fn db_demerger_splits_holding_across_listings() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "HEAD").await;
        insert_listing(&pool, 2, "DEM").await;
        // 100 @ $10 + $9.95 + $0.995 = $1,010.945 cost base.
        insert_buy(&pool, 1, 1, Decimal::from(100), Decimal::from(10)).await;
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
        crate::entities::demerger::db_demerge(&pool, 10)
            .await
            .unwrap();

        let mut holdings = db_holdings(&pool, None).await.unwrap();
        holdings.sort_by_key(|h| h.listing_id);
        assert_eq!(holdings.len(), 2);
        assert_eq!(holdings[0].listing_id, 1);
        assert_eq!(holdings[0].quantity, Decimal::from(100));
        // 80% of $1,010.945 = $808.756.
        assert_eq!(
            holdings[0].total_cost_base,
            "808.756".parse::<Decimal>().unwrap()
        );
        assert_eq!(holdings[1].listing_id, 2);
        assert_eq!(holdings[1].quantity, Decimal::from(20));
        // 20% of $1,010.945 = $202.189.
        assert_eq!(
            holdings[1].total_cost_base,
            "202.189".parse::<Decimal>().unwrap()
        );
    }
}
