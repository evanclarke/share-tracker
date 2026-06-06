use crate::infra::decimal::parse_dec;
use axum::{Json, Router, extract::State, http::StatusCode, routing::post};
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
    pub quantity: Decimal,
    pub avg_cost_base_per_unit: Decimal,
    pub total_cost_base: Decimal,
    pub current_price: Option<Decimal>,
    pub market_value: Option<Decimal>,
}

#[derive(Debug, Default, Deserialize)]
pub struct OverviewRequest {
    /// Current price per unit by listing id, expected in AUD so it lines up with
    /// the AUD-denominated cost base.
    #[serde(default)]
    pub prices: HashMap<i64, Decimal>,
}

pub fn router() -> Router<SqlitePool> {
    Router::new().route("/portfolio/overview", post(overview))
}

/// Returns open holdings per listing: quantity, cost base, and optional market value.
///
/// "Open" quantity for a parcel = trade.quantity − sum of parcel_allocations where purchase_trade_id = trade.id
/// (each allocation re-based to the parcel's as-acquired units — a post-split
/// sale allocates post-split units). The reported quantity is in *current*
/// units (after every recorded share split/consolidation) so it lines up with
/// a current market price; cost base totals are unaffected by splits
/// (TD 2000/10 — only the per-unit figure scales).
/// Cost base is pro-rated to remaining units and reduced by any AMIT adjustments
/// and return-of-capital payments (CGT event G1) received since acquisition.
pub async fn db_holdings(pool: &SqlitePool) -> Result<Vec<HoldingOverview>, sqlx::Error> {
    let trade_rows = sqlx::query(
        "SELECT id, listing_id, date, quantity, average_price, brokerage, gst_on_brokerage, \
         currency, fx_rate \
         FROM trades WHERE trade_type IN ('Buy', 'DRP')",
    )
    .fetch_all(pool)
    .await?;

    if trade_rows.is_empty() {
        return Ok(vec![]);
    }

    // units sold per purchase parcel, with each sale's date so the allocated
    // quantity (in sale-date units) can be re-based across splits
    let alloc_rows = sqlx::query(
        "SELECT pa.purchase_trade_id, pa.quantity_allocated, s.date AS sale_date \
         FROM parcel_allocations pa JOIN trades s ON s.id = pa.sale_trade_id",
    )
    .fetch_all(pool)
    .await?;

    let mut qty_sold: HashMap<i64, Vec<(NaiveDate, Decimal)>> = HashMap::new();
    for row in &alloc_rows {
        let tid: i64 = row.try_get("purchase_trade_id")?;
        qty_sold.entry(tid).or_default().push((
            row.try_get("sale_date")?,
            parse_dec("quantity_allocated", row.try_get("quantity_allocated")?)?,
        ));
    }

    // total AMIT cost base reduction per purchase parcel
    let cba_reduction = crate::entities::amit_adjustment::db_cost_base_reductions(pool).await?;
    // return-of-capital payments (CGT event G1) per listing
    let roc_events =
        crate::entities::corporate_action::db_return_of_capital_events(pool).await?;
    // share splits/consolidations per listing (quantity re-basing)
    let split_events = crate::entities::corporate_action::db_share_split_events(pool).await?;

    let mut listing_qty: HashMap<i64, Decimal> = HashMap::new();
    let mut listing_cost_base: HashMap<i64, Decimal> = HashMap::new();

    for row in &trade_rows {
        let trade_id: i64 = row.try_get("id")?;
        let listing_id: i64 = row.try_get("listing_id")?;
        let trade_date: NaiveDate = row.try_get("date")?;
        let qty = parse_dec("quantity", row.try_get("quantity")?)?;
        let price = parse_dec("average_price", row.try_get("average_price")?)?;
        let brok = parse_dec("brokerage", row.try_get("brokerage")?)?;
        let gst = parse_dec("gst_on_brokerage", row.try_get("gst_on_brokerage")?)?;
        let currency: String = row.try_get("currency")?;
        let fx_rate = parse_dec("fx_rate", row.try_get("fx_rate")?)?;

        let splits = split_events.get(&listing_id).map_or(&[][..], |v| v);
        // Internal cost-base arithmetic stays in the parcel's as-acquired units;
        // each sale's allocated quantity is re-based back across any splits.
        let sold = crate::entities::corporate_action::sold_in_acquired_units(
            qty_sold.get(&trade_id).map_or(&[][..], |v| v),
            splits,
            trade_date,
        );
        let remaining = qty - sold;
        if remaining <= Decimal::ZERO {
            continue;
        }

        let initial_cost = price * qty + brok + gst;
        let amit = *cba_reduction.get(&trade_id).unwrap_or(&Decimal::ZERO);
        // CGT event E10: an AMIT cost base reduction can only take the cost base to
        // nil, never negative. Any excess is a capital gain reported by the
        // net-capital-gain report, not a negative cost base here.
        let net_cost = (initial_cost - amit).max(Decimal::ZERO);
        // pro-rate cost base to remaining units, then net off the return-of-capital
        // payments received on those units (CGT event G1 also floors at nil — the
        // excess is a capital gain in the net-capital-gain report, never a
        // negative cost base here)
        let roc_per_unit = crate::entities::corporate_action::per_unit_reduction(
            roc_events.get(&listing_id).map_or(&[][..], |v| v),
            splits,
            &currency,
            trade_date,
            None,
        )?;
        let remaining_cost = if qty > Decimal::ZERO {
            (net_cost * remaining / qty - roc_per_unit * remaining).max(Decimal::ZERO)
        } else {
            Decimal::ZERO
        };
        // Convert the parcel's cost base to AUD (ATO rate for the buy month, else
        // the trade's manual fx_rate) so holdings aggregate in AUD.
        let remaining_cost =
            crate::infra::fx::to_aud(pool, remaining_cost, &currency, trade_date, Some(fx_rate))
                .await?;

        // The holding's quantity is reported in current units — after every
        // recorded split/consolidation — so market value uses today's price.
        let remaining_now = crate::entities::corporate_action::split_adjusted_quantity(
            remaining, splits, trade_date, None,
        );
        *listing_qty.entry(listing_id).or_insert(Decimal::ZERO) += remaining_now;
        *listing_cost_base.entry(listing_id).or_insert(Decimal::ZERO) += remaining_cost;
    }

    let mut result: Vec<HoldingOverview> = listing_qty
        .into_iter()
        .filter(|(_, qty)| *qty > Decimal::ZERO)
        .map(|(listing_id, qty)| {
            let cost_base = listing_cost_base.get(&listing_id).copied().unwrap_or(Decimal::ZERO);
            let avg = if qty > Decimal::ZERO { cost_base / qty } else { Decimal::ZERO };
            HoldingOverview {
                listing_id,
                quantity: qty,
                avg_cost_base_per_unit: avg,
                total_cost_base: cost_base,
                current_price: None,
                market_value: None,
            }
        })
        .collect();

    result.sort_by_key(|h| h.listing_id);
    Ok(result)
}

async fn overview(
    State(pool): State<SqlitePool>,
    body: Option<Json<OverviewRequest>>,
) -> Result<Json<Vec<HoldingOverview>>, StatusCode> {
    let prices = body.map(|Json(req)| req.prices).unwrap_or_default();
    let mut holdings = db_holdings(&pool)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    for h in &mut holdings {
        if let Some(&price) = prices.get(&h.listing_id) {
            h.current_price = Some(price);
            h.market_value = Some(h.quantity * price);
        }
    }

    Ok(Json(holdings))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{infra::db, entities::{amma, amit_adjustment, corporate_action, listing, parcel_allocation, trade}};
    use axum::{body::Body, http::Request};
    use chrono::NaiveDate;
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    async fn test_pool() -> SqlitePool {
        db::init(":memory:").await.unwrap()
    }

    async fn insert_listing(pool: &SqlitePool, id: i64, ticker: &str) {
        listing::db_upsert(
            pool,
            &listing::Listing {
                id,
                exchange_mic: "XASX".to_string(),
                ticker: ticker.to_string(),
                name: ticker.to_string(),
                isin: None,
                security_type: listing::SecurityType::ETF,
                currency: "AUD".to_string(),
                amit: false,
                preference: false,
            },
        )
        .await
        .unwrap();
    }

    async fn insert_buy(pool: &SqlitePool, id: i64, listing_id: i64, qty: Decimal, price: Decimal) {
        trade::db_upsert(
            pool,
            &trade::Trade {
                id,
                trade_type: trade::TradeType::Buy,
                date: NaiveDate::from_ymd_opt(2024, 1, 1).unwrap(),
                settlement_date: NaiveDate::from_ymd_opt(2024, 1, 3).unwrap(),
                listing_id,
                average_price: price,
                quantity: qty,
                currency: "AUD".to_string(),
                brokerage: "9.95".parse().unwrap(),
                gst_on_brokerage: "0.995".parse().unwrap(),
                brokerage_currency: "AUD".to_string(),
                fx_rate: Decimal::ONE,
                contract_note_ref: None,
                residual_brought_forward: Decimal::ZERO,
                residual_carried_forward: Decimal::ZERO,
                residual_paid_out: Decimal::ZERO,
                rights_action_id: None,
            },
        )
        .await
        .unwrap();
    }

    async fn insert_sell(pool: &SqlitePool, id: i64, listing_id: i64, qty: Decimal) {
        trade::db_upsert(
            pool,
            &trade::Trade {
                id,
                trade_type: trade::TradeType::Sell,
                date: NaiveDate::from_ymd_opt(2024, 6, 1).unwrap(),
                settlement_date: NaiveDate::from_ymd_opt(2024, 6, 3).unwrap(),
                listing_id,
                average_price: Decimal::from(120),
                quantity: qty,
                currency: "AUD".to_string(),
                brokerage: "9.95".parse().unwrap(),
                gst_on_brokerage: "0.995".parse().unwrap(),
                brokerage_currency: "AUD".to_string(),
                fx_rate: Decimal::ONE,
                contract_note_ref: None,
                residual_brought_forward: Decimal::ZERO,
                residual_carried_forward: Decimal::ZERO,
                residual_paid_out: Decimal::ZERO,
                rights_action_id: None,
            },
        )
        .await
        .unwrap();
    }

    async fn allocate(pool: &SqlitePool, id: i64, sale_id: i64, buy_id: i64, qty: Decimal) {
        parcel_allocation::db_upsert(
            pool,
            &parcel_allocation::ParcelAllocation {
                id,
                sale_trade_id: sale_id,
                purchase_trade_id: buy_id,
                quantity_allocated: qty,
            },
        )
        .await
        .unwrap();
    }

    fn make_amma(id: i64, listing_id: i64, cba: Decimal) -> amma::AmmaStatement {
        amma::AmmaStatement {
            id,
            listing_id,
            tax_year_end_date: NaiveDate::from_ymd_opt(2024, 6, 30).unwrap(),
            units_held: Decimal::from(100),
            date_received: NaiveDate::from_ymd_opt(2024, 8, 15).unwrap(),
            cost_base_adjustment: cba,
            australian_interest: Decimal::ZERO,
            australian_dividends_unfranked: Decimal::ZERO,
            franked_dividends: Decimal::ZERO,
            franking_credits: Decimal::ZERO,
            net_rent: Decimal::ZERO,
            foreign_income: Decimal::ZERO,
            foreign_tax_credits: Decimal::ZERO,
            other_income: Decimal::ZERO,
            cgt_discount_gains: Decimal::ZERO,
            cgt_indexation_gains: Decimal::ZERO,
            cgt_other_gains: Decimal::ZERO,
            capital_losses_applied: Decimal::ZERO,
            tax_deferred_amount: Decimal::ZERO,
            tax_free_amount: Decimal::ZERO,
            tfn_withholding_tax: Decimal::ZERO,
            currency: "AUD".to_string(),
        }
    }

    // DB-level tests

    #[tokio::test]
    async fn db_no_trades_returns_empty() {
        let pool = test_pool().await;
        let holdings = db_holdings(&pool).await.unwrap();
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
        assert!(db_holdings(&pool).await.is_err());
    }

    #[tokio::test]
    async fn db_single_buy_fully_held() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "VAS").await;
        insert_buy(&pool, 1, 1, Decimal::from(100), Decimal::from(10)).await;

        let holdings = db_holdings(&pool).await.unwrap();
        assert_eq!(holdings.len(), 1);
        let h = &holdings[0];
        assert_eq!(h.listing_id, 1);
        assert_eq!(h.quantity, Decimal::from(100));
        // cost = 10 * 100 + 9.95 + 0.995 = 1010.945
        assert_eq!(h.total_cost_base, "1010.945".parse::<Decimal>().unwrap());
        assert_eq!(h.avg_cost_base_per_unit, "10.10945".parse::<Decimal>().unwrap());
    }

    #[tokio::test]
    async fn db_partial_sell_reduces_holding() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "VAS").await;
        insert_buy(&pool, 1, 1, Decimal::from(100), Decimal::from(10)).await;
        insert_sell(&pool, 2, 1, Decimal::from(40)).await;
        allocate(&pool, 1, 2, 1, Decimal::from(40)).await;

        let holdings = db_holdings(&pool).await.unwrap();
        assert_eq!(holdings.len(), 1);
        let h = &holdings[0];
        assert_eq!(h.quantity, Decimal::from(60));
        // remaining_cost = 1010.945 * 60 / 100 = 606.567
        assert_eq!(h.total_cost_base, "606.567".parse::<Decimal>().unwrap());
        assert_eq!(h.avg_cost_base_per_unit, "10.10945".parse::<Decimal>().unwrap());
    }

    #[tokio::test]
    async fn db_fully_sold_listing_excluded() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "VAS").await;
        insert_buy(&pool, 1, 1, Decimal::from(100), Decimal::from(10)).await;
        insert_sell(&pool, 2, 1, Decimal::from(100)).await;
        allocate(&pool, 1, 2, 1, Decimal::from(100)).await;

        let holdings = db_holdings(&pool).await.unwrap();
        assert!(holdings.is_empty());
    }

    #[tokio::test]
    async fn db_amit_adjustment_reduces_cost_base() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "VAF").await;
        insert_buy(&pool, 1, 1, Decimal::from(100), Decimal::from(10)).await;
        amma::db_upsert(&pool, &make_amma(1, 1, "0.05".parse().unwrap())).await.unwrap();
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

        let holdings = db_holdings(&pool).await.unwrap();
        assert_eq!(holdings.len(), 1);
        let h = &holdings[0];
        // initial = 1010.945, AMIT = 100 * 0.05 = 5.00, net = 1005.945
        assert_eq!(h.total_cost_base, "1005.945".parse::<Decimal>().unwrap());
        assert_eq!(h.avg_cost_base_per_unit, "10.05945".parse::<Decimal>().unwrap());
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
        amma::db_upsert(&pool, &make_amma(1, 1, "1.50".parse().unwrap())).await.unwrap();
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

        let holdings = db_holdings(&pool).await.unwrap();
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

        let holdings = db_holdings(&pool).await.unwrap();
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

        let holdings = db_holdings(&pool).await.unwrap();
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

    /// TD 2000/10 (`docs/share-splits-and-consolidations.md`): a share split
    /// multiplies the unit count and leaves the total cost base unchanged, so
    /// the per-unit cost base scales down proportionately.
    #[tokio::test]
    async fn db_share_split_adjusts_quantity_and_preserves_cost_base() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "SPL").await;
        // Buy 100 @ $10 on 2024-01-01 → cost base 1010.945 (incl. brokerage).
        insert_buy(&pool, 1, 1, Decimal::from(100), Decimal::from(10)).await;
        // 2-for-1 split after acquisition.
        apply_split(&pool, 1, 1, NaiveDate::from_ymd_opt(2024, 3, 1).unwrap(), "2", "1").await;

        let holdings = db_holdings(&pool).await.unwrap();
        assert_eq!(holdings.len(), 1);
        let h = &holdings[0];
        assert_eq!(h.quantity, Decimal::from(200));
        assert_eq!(h.total_cost_base, "1010.945".parse::<Decimal>().unwrap());
        // per-unit cost base halves: 1010.945 / 200
        assert_eq!(h.avg_cost_base_per_unit, "5.054725".parse::<Decimal>().unwrap());
    }

    /// A non-assessable bonus issue (`docs/bonus-shares.md`) adds the bonus
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

        let holdings = db_holdings(&pool).await.unwrap();
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
        apply_split(&pool, 1, 1, NaiveDate::from_ymd_opt(2024, 3, 1).unwrap(), "1", "10").await;

        let holdings = db_holdings(&pool).await.unwrap();
        assert_eq!(holdings[0].quantity, Decimal::from(10));
        assert_eq!(holdings[0].total_cost_base, "1010.945".parse::<Decimal>().unwrap());
    }

    /// A sale entered after a split is in post-split units; the holding nets it
    /// off correctly against the pre-split parcel.
    #[tokio::test]
    async fn db_post_split_sell_nets_off_pre_split_parcel() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "SPL").await;
        insert_buy(&pool, 1, 1, Decimal::from(100), Decimal::from(10)).await; // 2024-01-01
        apply_split(&pool, 1, 1, NaiveDate::from_ymd_opt(2024, 3, 1).unwrap(), "2", "1").await;
        // Sell 80 post-split units (= 40 as-acquired) on 2024-06-01.
        insert_sell(&pool, 2, 1, Decimal::from(80)).await;
        allocate(&pool, 1, 2, 1, Decimal::from(80)).await;

        let holdings = db_holdings(&pool).await.unwrap();
        assert_eq!(holdings.len(), 1);
        // 200 post-split − 80 sold = 120 held now.
        assert_eq!(holdings[0].quantity, Decimal::from(120));
        // Cost base of the remaining 60 as-acquired units: 1010.945 × 60/100.
        assert_eq!(holdings[0].total_cost_base, "606.567".parse::<Decimal>().unwrap());
    }

    /// A split dated before acquisition does not touch the parcel — its trade
    /// quantity is already in post-split units.
    #[tokio::test]
    async fn db_split_before_acquisition_does_not_apply() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "SPL").await;
        apply_split(&pool, 1, 1, NaiveDate::from_ymd_opt(2023, 6, 1).unwrap(), "2", "1").await;
        insert_buy(&pool, 1, 1, Decimal::from(100), Decimal::from(10)).await; // 2024-01-01

        let holdings = db_holdings(&pool).await.unwrap();
        assert_eq!(holdings[0].quantity, Decimal::from(100));
    }

    /// A return of capital after a split is per post-split unit: the reduction
    /// applies to the multiplied unit count.
    #[tokio::test]
    async fn db_return_of_capital_after_split_scales_to_post_split_units() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "SPL").await;
        insert_buy(&pool, 1, 1, Decimal::from(100), Decimal::from(10)).await; // 2024-01-01
        apply_split(&pool, 1, 1, NaiveDate::from_ymd_opt(2024, 3, 1).unwrap(), "2", "1").await;
        // 25c/unit on the 200 post-split units → 50.00 off the cost base.
        apply_roc(&pool, 2, 1, NaiveDate::from_ymd_opt(2024, 6, 1).unwrap(), "0.25").await;

        let holdings = db_holdings(&pool).await.unwrap();
        // 1010.945 − 200 × 0.25 = 960.945
        assert_eq!(holdings[0].total_cost_base, "960.945".parse::<Decimal>().unwrap());
    }

    /// A return of capital (CGT event G1) reduces the holding's cost base by the
    /// per-unit payment for units held on the payment date
    /// (`docs/cgt-non-assessable-payments.md`).
    #[tokio::test]
    async fn db_return_of_capital_reduces_cost_base() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "RAP").await;
        // Buy 100 @ $10 on 2024-01-01 → cost base 1010.945 (incl. brokerage).
        insert_buy(&pool, 1, 1, Decimal::from(100), Decimal::from(10)).await;
        // 50c/unit return of capital while all 100 units are held.
        apply_roc(&pool, 1, 1, NaiveDate::from_ymd_opt(2024, 3, 1).unwrap(), "0.50").await;

        let holdings = db_holdings(&pool).await.unwrap();
        assert_eq!(holdings.len(), 1);
        // 1010.945 − 100 × 0.50 = 960.945
        assert_eq!(holdings[0].total_cost_base, "960.945".parse::<Decimal>().unwrap());
    }

    #[tokio::test]
    async fn db_return_of_capital_before_acquisition_does_not_apply() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "RAP").await;
        // Payment made before this parcel was acquired (2024-01-01): unaffected.
        apply_roc(&pool, 1, 1, NaiveDate::from_ymd_opt(2023, 11, 30).unwrap(), "0.50").await;
        insert_buy(&pool, 1, 1, Decimal::from(100), Decimal::from(10)).await;

        let holdings = db_holdings(&pool).await.unwrap();
        assert_eq!(holdings[0].total_cost_base, "1010.945".parse::<Decimal>().unwrap());
    }

    /// G1 can never push the cost base below nil: the excess over cost base is a
    /// capital gain in the net-capital-gain report, not a negative cost base here.
    #[tokio::test]
    async fn db_return_of_capital_floors_cost_base_at_nil() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "RAP").await;
        insert_buy(&pool, 1, 1, Decimal::from(100), Decimal::from(10)).await;
        // $11/unit × 100 = 1100 exceeds the 1010.945 cost base.
        apply_roc(&pool, 1, 1, NaiveDate::from_ymd_opt(2024, 3, 1).unwrap(), "11").await;

        let holdings = db_holdings(&pool).await.unwrap();
        assert_eq!(holdings[0].total_cost_base, Decimal::ZERO);
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
        assert_eq!(holdings[0].current_price, Some("120.50".parse::<Decimal>().unwrap()));
        // market_value = 100 * 120.50 = 12050.00
        assert_eq!(holdings[0].market_value, Some("12050.00".parse::<Decimal>().unwrap()));
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
}
