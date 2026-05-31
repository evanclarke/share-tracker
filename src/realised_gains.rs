use axum::{Json, Router, extract::State, http::StatusCode, routing::get};
use chrono::{Months, NaiveDate};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use sqlx::{Row, SqlitePool};
use std::collections::HashMap;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RealisedGainLoss {
    pub sale_trade_id: i64,
    pub listing_id: i64,
    pub sale_date: NaiveDate,
    /// Net proceeds from the allocated portion (sale price × qty − pro-rated brokerage).
    pub proceeds: Decimal,
    /// Adjusted cost base of the sold parcels (AMIT-reduced, pro-rated to allocated qty).
    pub cost_base: Decimal,
    /// proceeds − cost_base (positive = gain, negative = loss).
    pub capital_gain_loss: Decimal,
    /// Portion of the capital gain from parcels held strictly more than 12 months
    /// (eligible for the 50% CGT discount). Always ≥ 0; losses are excluded.
    pub discount_eligible_gain: Decimal,
}

pub fn router() -> Router<SqlitePool> {
    Router::new().route("/portfolio/realised-gains", get(realised_gains_handler))
}

pub async fn db_realised_gains(pool: &SqlitePool) -> Result<Vec<RealisedGainLoss>, sqlx::Error> {
    let sell_rows = sqlx::query(
        "SELECT id, listing_id, date, quantity, average_price, brokerage, gst_on_brokerage \
         FROM trades WHERE trade_type = 'Sell'",
    )
    .fetch_all(pool)
    .await?;

    if sell_rows.is_empty() {
        return Ok(vec![]);
    }

    let buy_rows = sqlx::query(
        "SELECT id, date, quantity, average_price, brokerage, gst_on_brokerage \
         FROM trades WHERE trade_type IN ('Buy', 'DRP')",
    )
    .fetch_all(pool)
    .await?;

    let alloc_rows = sqlx::query(
        "SELECT sale_trade_id, purchase_trade_id, quantity_allocated FROM parcel_allocations",
    )
    .fetch_all(pool)
    .await?;

    if alloc_rows.is_empty() {
        return Ok(vec![]);
    }

    let amit_rows = sqlx::query(
        "SELECT aa.trade_id, aa.quantity, a.cost_base_adjustment \
         FROM amit_adjustments aa \
         JOIN amma_statements a ON a.id = aa.amma_statement_id",
    )
    .fetch_all(pool)
    .await?;

    struct SellInfo {
        listing_id: i64,
        date: NaiveDate,
        quantity: Decimal,
        average_price: Decimal,
        brokerage: Decimal,
        gst_on_brokerage: Decimal,
    }

    struct BuyInfo {
        date: NaiveDate,
        quantity: Decimal,
        average_price: Decimal,
        brokerage: Decimal,
        gst_on_brokerage: Decimal,
    }

    let mut sell_map: HashMap<i64, SellInfo> = HashMap::new();
    for row in &sell_rows {
        let id: i64 = row.try_get("id")?;
        sell_map.insert(
            id,
            SellInfo {
                listing_id: row.try_get("listing_id")?,
                date: row.try_get("date")?,
                quantity: row
                    .try_get::<String, _>("quantity")?
                    .parse()
                    .unwrap_or(Decimal::ZERO),
                average_price: row
                    .try_get::<String, _>("average_price")?
                    .parse()
                    .unwrap_or(Decimal::ZERO),
                brokerage: row
                    .try_get::<String, _>("brokerage")?
                    .parse()
                    .unwrap_or(Decimal::ZERO),
                gst_on_brokerage: row
                    .try_get::<String, _>("gst_on_brokerage")?
                    .parse()
                    .unwrap_or(Decimal::ZERO),
            },
        );
    }

    let mut buy_map: HashMap<i64, BuyInfo> = HashMap::new();
    for row in &buy_rows {
        let id: i64 = row.try_get("id")?;
        buy_map.insert(
            id,
            BuyInfo {
                date: row.try_get("date")?,
                quantity: row
                    .try_get::<String, _>("quantity")?
                    .parse()
                    .unwrap_or(Decimal::ZERO),
                average_price: row
                    .try_get::<String, _>("average_price")?
                    .parse()
                    .unwrap_or(Decimal::ZERO),
                brokerage: row
                    .try_get::<String, _>("brokerage")?
                    .parse()
                    .unwrap_or(Decimal::ZERO),
                gst_on_brokerage: row
                    .try_get::<String, _>("gst_on_brokerage")?
                    .parse()
                    .unwrap_or(Decimal::ZERO),
            },
        );
    }

    let mut cba_reduction: HashMap<i64, Decimal> = HashMap::new();
    for row in &amit_rows {
        let tid: i64 = row.try_get("trade_id")?;
        let qty: Decimal = row
            .try_get::<String, _>("quantity")?
            .parse()
            .unwrap_or(Decimal::ZERO);
        let cba: Decimal = row
            .try_get::<String, _>("cost_base_adjustment")?
            .parse()
            .unwrap_or(Decimal::ZERO);
        *cba_reduction.entry(tid).or_insert(Decimal::ZERO) += qty * cba;
    }

    let mut sale_proceeds: HashMap<i64, Decimal> = HashMap::new();
    let mut sale_cost_base: HashMap<i64, Decimal> = HashMap::new();
    let mut sale_discount_gain: HashMap<i64, Decimal> = HashMap::new();

    for row in &alloc_rows {
        let sale_id: i64 = row.try_get("sale_trade_id")?;
        let buy_id: i64 = row.try_get("purchase_trade_id")?;
        let qty_alloc: Decimal = row
            .try_get::<String, _>("quantity_allocated")?
            .parse()
            .unwrap_or(Decimal::ZERO);

        let Some(sale) = sell_map.get(&sale_id) else {
            continue;
        };
        let Some(buy) = buy_map.get(&buy_id) else {
            continue;
        };

        // Proceeds for this allocation: pro-rate sale brokerage+gst by qty_alloc / sale_qty
        let sale_costs = sale.brokerage + sale.gst_on_brokerage;
        let alloc_proceeds = if sale.quantity > Decimal::ZERO {
            sale.average_price * qty_alloc - sale_costs * qty_alloc / sale.quantity
        } else {
            Decimal::ZERO
        };

        // Cost base for allocated portion of purchase parcel (AMIT-reduced, pro-rated)
        let buy_initial_cost =
            buy.average_price * buy.quantity + buy.brokerage + buy.gst_on_brokerage;
        let amit = *cba_reduction.get(&buy_id).unwrap_or(&Decimal::ZERO);
        let buy_net_cost = buy_initial_cost - amit;
        let alloc_cost = if buy.quantity > Decimal::ZERO {
            buy_net_cost * qty_alloc / buy.quantity
        } else {
            Decimal::ZERO
        };

        let alloc_gain = alloc_proceeds - alloc_cost;

        *sale_proceeds.entry(sale_id).or_insert(Decimal::ZERO) += alloc_proceeds;
        *sale_cost_base.entry(sale_id).or_insert(Decimal::ZERO) += alloc_cost;

        // CGT discount applies only to gains from parcels held strictly > 12 months
        if sale.date > buy.date + Months::new(12) && alloc_gain > Decimal::ZERO {
            *sale_discount_gain.entry(sale_id).or_insert(Decimal::ZERO) += alloc_gain;
        }
    }

    let mut result: Vec<RealisedGainLoss> = sale_proceeds
        .keys()
        .filter_map(|&sale_id| {
            let sale = sell_map.get(&sale_id)?;
            let proceeds = sale_proceeds[&sale_id];
            let cost_base = sale_cost_base[&sale_id];
            let discount_gain =
                sale_discount_gain.get(&sale_id).copied().unwrap_or(Decimal::ZERO);
            Some(RealisedGainLoss {
                sale_trade_id: sale_id,
                listing_id: sale.listing_id,
                sale_date: sale.date,
                proceeds,
                cost_base,
                capital_gain_loss: proceeds - cost_base,
                discount_eligible_gain: discount_gain,
            })
        })
        .collect();

    result.sort_by(|a, b| {
        a.sale_date
            .cmp(&b.sale_date)
            .then(a.sale_trade_id.cmp(&b.sale_trade_id))
    });
    Ok(result)
}

async fn realised_gains_handler(
    State(pool): State<SqlitePool>,
) -> Result<Json<Vec<RealisedGainLoss>>, StatusCode> {
    db_realised_gains(&pool)
        .await
        .map(Json)
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{amma, amit_adjustment, db, listing, parcel_allocation, trade};
    use axum::{body::Body, http::Request};
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
            },
        )
        .await
        .unwrap();
    }

    async fn insert_buy(
        pool: &SqlitePool,
        id: i64,
        listing_id: i64,
        date: NaiveDate,
        qty: Decimal,
        price: Decimal,
    ) {
        trade::db_upsert(
            pool,
            &trade::Trade {
                id,
                trade_type: trade::TradeType::Buy,
                date,
                settlement_date: date + chrono::Duration::days(2),
                listing_id,
                average_price: price,
                quantity: qty,
                currency: "AUD".to_string(),
                brokerage: Decimal::ZERO,
                gst_on_brokerage: Decimal::ZERO,
                brokerage_currency: "AUD".to_string(),
                fx_rate: Decimal::ONE,
                contract_note_ref: None,
            },
        )
        .await
        .unwrap();
    }

    async fn insert_sell(
        pool: &SqlitePool,
        id: i64,
        listing_id: i64,
        date: NaiveDate,
        qty: Decimal,
        price: Decimal,
    ) {
        trade::db_upsert(
            pool,
            &trade::Trade {
                id,
                trade_type: trade::TradeType::Sell,
                date,
                settlement_date: date + chrono::Duration::days(2),
                listing_id,
                average_price: price,
                quantity: qty,
                currency: "AUD".to_string(),
                brokerage: Decimal::ZERO,
                gst_on_brokerage: Decimal::ZERO,
                brokerage_currency: "AUD".to_string(),
                fx_rate: Decimal::ONE,
                contract_note_ref: None,
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
        insert_sell(&pool, 2, 1, sell_date, Decimal::from(100), Decimal::from(15)).await;
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
        insert_sell(&pool, 2, 1, sell_date, Decimal::from(100), Decimal::from(15)).await;
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
        insert_sell(&pool, 2, 1, sell_date, Decimal::from(100), Decimal::from(10)).await;
        allocate(&pool, 1, 2, 1, Decimal::from(100)).await;

        let result = db_realised_gains(&pool).await.unwrap();
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].capital_gain_loss, Decimal::from(-500));
        assert_eq!(result[0].discount_eligible_gain, Decimal::ZERO);
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
        trade::db_upsert(
            &pool,
            &trade::Trade {
                id: 1,
                trade_type: trade::TradeType::Buy,
                date: buy_date,
                settlement_date: buy_date + chrono::Duration::days(2),
                listing_id: 1,
                average_price: Decimal::from(10),
                quantity: Decimal::from(100),
                currency: "AUD".to_string(),
                brokerage: "9.95".parse().unwrap(),
                gst_on_brokerage: "0.995".parse().unwrap(),
                brokerage_currency: "AUD".to_string(),
                fx_rate: Decimal::ONE,
                contract_note_ref: None,
            },
        )
        .await
        .unwrap();
        trade::db_upsert(
            &pool,
            &trade::Trade {
                id: 2,
                trade_type: trade::TradeType::Sell,
                date: sell_date,
                settlement_date: sell_date + chrono::Duration::days(2),
                listing_id: 1,
                average_price: Decimal::from(15),
                quantity: Decimal::from(100),
                currency: "AUD".to_string(),
                brokerage: "9.95".parse().unwrap(),
                gst_on_brokerage: "0.995".parse().unwrap(),
                brokerage_currency: "AUD".to_string(),
                fx_rate: Decimal::ONE,
                contract_note_ref: None,
            },
        )
        .await
        .unwrap();
        allocate(&pool, 1, 2, 1, Decimal::from(100)).await;

        let result = db_realised_gains(&pool).await.unwrap();
        assert_eq!(result.len(), 1);
        // proceeds = 15*100 - 10.945 = 1489.055
        assert_eq!(result[0].proceeds, "1489.055".parse::<Decimal>().unwrap());
        // cost_base = 10*100 + 10.945 = 1010.945
        assert_eq!(result[0].cost_base, "1010.945".parse::<Decimal>().unwrap());
        // gain = 1489.055 - 1010.945 = 478.110
        assert_eq!(result[0].capital_gain_loss, "478.110".parse::<Decimal>().unwrap());
    }

    #[tokio::test]
    async fn db_cgt_discount_eligible_after_12_months() {
        let pool = test_pool().await;
        let buy_date = NaiveDate::from_ymd_opt(2024, 1, 1).unwrap();
        let sell_date = NaiveDate::from_ymd_opt(2025, 1, 2).unwrap(); // strictly > 12 months
        insert_listing(&pool, 1, "VAS").await;
        insert_buy(&pool, 1, 1, buy_date, Decimal::from(100), Decimal::from(10)).await;
        insert_sell(&pool, 2, 1, sell_date, Decimal::from(100), Decimal::from(15)).await;
        allocate(&pool, 1, 2, 1, Decimal::from(100)).await;

        let result = db_realised_gains(&pool).await.unwrap();
        assert_eq!(result[0].capital_gain_loss, Decimal::from(500));
        assert_eq!(result[0].discount_eligible_gain, Decimal::from(500));
    }

    #[tokio::test]
    async fn db_cgt_not_eligible_exactly_12_months() {
        let pool = test_pool().await;
        let buy_date = NaiveDate::from_ymd_opt(2024, 1, 1).unwrap();
        let sell_date = NaiveDate::from_ymd_opt(2025, 1, 1).unwrap(); // exactly 12 months — not eligible
        insert_listing(&pool, 1, "VAS").await;
        insert_buy(&pool, 1, 1, buy_date, Decimal::from(100), Decimal::from(10)).await;
        insert_sell(&pool, 2, 1, sell_date, Decimal::from(100), Decimal::from(15)).await;
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
        insert_sell(&pool, 2, 1, sell_date, Decimal::from(100), Decimal::from(10)).await;
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
        insert_sell(&pool, 3, 1, sell_date, Decimal::from(150), Decimal::from(15)).await;
        allocate(&pool, 1, 3, 1, Decimal::from(100)).await;
        allocate(&pool, 2, 3, 2, Decimal::from(50)).await;

        let result = db_realised_gains(&pool).await.unwrap();
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].proceeds, Decimal::from(2250));
        assert_eq!(result[0].cost_base, Decimal::from(1500));
        assert_eq!(result[0].capital_gain_loss, Decimal::from(750));
        assert_eq!(result[0].discount_eligible_gain, Decimal::from(500));
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
        insert_sell(&pool, 2, 1, sell_date, Decimal::from(100), Decimal::from(15)).await;
        allocate(&pool, 1, 2, 1, Decimal::from(100)).await;

        amma::db_upsert(
            &pool,
            &amma::AmmaStatement {
                id: 1,
                listing_id: 1,
                tax_year_end_date: NaiveDate::from_ymd_opt(2024, 6, 30).unwrap(),
                units_held: Decimal::from(100),
                date_received: NaiveDate::from_ymd_opt(2024, 8, 15).unwrap(),
                cost_base_adjustment: "0.05".parse().unwrap(),
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
            },
        )
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
        insert_buy(&pool, 1, 1, d(2023, 1, 1), Decimal::from(100), Decimal::from(10)).await;
        insert_buy(&pool, 2, 1, d(2023, 1, 1), Decimal::from(100), Decimal::from(10)).await;
        // sell 2 comes before sell 3 by date but is inserted second
        insert_sell(&pool, 3, 1, d(2024, 6, 1), Decimal::from(50), Decimal::from(15)).await;
        insert_sell(&pool, 4, 1, d(2024, 3, 1), Decimal::from(50), Decimal::from(15)).await;
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
        insert_sell(&pool, 2, 1, sell_date, Decimal::from(100), Decimal::from(15)).await;
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
}
