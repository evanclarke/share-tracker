use crate::infra::http::ApiError;
use crate::domain::cost_base;
use crate::infra::decimal::parse_dec;
use axum::{Json, Router, extract::State, routing::get};
use chrono::{Months, NaiveDate};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use sqlx::{Row, SqlitePool};
use std::collections::HashMap;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RealisedGainLoss {
    pub sale_trade_id: i64,
    pub listing_id: i64,
    /// The holding account the Sell happened in (the same taxpayer either
    /// way — totals are unchanged; rows identify the account).
    pub holding_account_id: i64,
    pub sale_date: NaiveDate,
    /// Net proceeds from the allocated portion (sale price × qty − pro-rated
    /// brokerage), converted to AUD at the sale's ATO FX rate.
    pub proceeds: Decimal,
    /// Adjusted cost base of the sold parcels (AMIT-reduced, pro-rated to allocated
    /// qty), converted to AUD at the purchase's ATO FX rate.
    pub cost_base: Decimal,
    /// proceeds − cost_base in AUD (positive = gain, negative = loss).
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
}

// Per-sale identity: capital_gain_loss == discount_eligible_gain
//                                       + non_discountable_gain − capital_loss.

pub fn router() -> Router<SqlitePool> {
    Router::new().route("/portfolio/realised-gains", get(realised_gains_handler))
}

pub async fn db_realised_gains(pool: &SqlitePool) -> Result<Vec<RealisedGainLoss>, sqlx::Error> {
    // A scrip-for-scrip exchange or demerger closing Sell (scrip_action_id /
    // demerger_action_id set) is not a realised gain or loss: the rollover
    // disregards the gain on the original shares
    // (docs/ato/takeovers-and-scrip-for-scrip.md, docs/ato/demergers.md), and its
    // zero proceeds must never surface as a capital loss. A holding-account
    // transfer-out Sell (transfer_id set) is not even a disposal — the same
    // beneficial owner holds the shares before and after, so it is no CGT
    // event at all. Their allocations are skipped with them (the sale id is
    // absent from sell_map).
    let sell_rows = sqlx::query(
        "SELECT id, listing_id, holding_account_id, date, quantity, average_price, brokerage, \
         gst_on_brokerage, currency, fx_rate \
         FROM trades WHERE trade_type = 'Sell' \
           AND scrip_action_id IS NULL AND demerger_action_id IS NULL AND transfer_id IS NULL",
    )
    .fetch_all(pool)
    .await?;

    if sell_rows.is_empty() {
        return Ok(vec![]);
    }

    let buy_rows = sqlx::query(
        "SELECT id, listing_id, date, quantity, average_price, brokerage, gst_on_brokerage, \
         currency, fx_rate, deemed_acquisition_date \
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

    // Each trade carries its own currency, trade date, and manual `fx_rate`
    // fallback so its amounts can be converted to AUD via the ATO reference rate
    // (see `infra::fx`). Proceeds and cost base are converted independently — a
    // buy and the sell that closes it may settle in different months at different
    // rates — so totals are never aggregated across mixed currencies.
    struct SellInfo {
        listing_id: i64,
        holding_account_id: i64,
        date: NaiveDate,
        quantity: Decimal,
        average_price: Decimal,
        brokerage: Decimal,
        gst_on_brokerage: Decimal,
        currency: String,
        fx_rate: Decimal,
    }

    struct BuyInfo {
        listing_id: i64,
        date: NaiveDate,
        /// The CGT acquisition date: a scrip-for-scrip replacement parcel
        /// carries the consumed parcel's acquisition date (the rollover's
        /// combined holding period), every other parcel its own trade date.
        /// Drives the 12-month discount clock and the AUD translation month
        /// of the cost base; split/return-of-capital applicability stays on
        /// the actual `date`.
        acquired: NaiveDate,
        quantity: Decimal,
        average_price: Decimal,
        brokerage: Decimal,
        gst_on_brokerage: Decimal,
        currency: String,
        fx_rate: Decimal,
    }

    let mut sell_map: HashMap<i64, SellInfo> = HashMap::new();
    for row in &sell_rows {
        let id: i64 = row.try_get("id")?;
        sell_map.insert(
            id,
            SellInfo {
                listing_id: row.try_get("listing_id")?,
                holding_account_id: row.try_get("holding_account_id")?,
                date: row.try_get("date")?,
                quantity: parse_dec("quantity", row.try_get("quantity")?)?,
                average_price: parse_dec("average_price", row.try_get("average_price")?)?,
                brokerage: parse_dec("brokerage", row.try_get("brokerage")?)?,
                gst_on_brokerage: parse_dec("gst_on_brokerage", row.try_get("gst_on_brokerage")?)?,
                currency: row.try_get("currency")?,
                fx_rate: parse_dec("fx_rate", row.try_get("fx_rate")?)?,
            },
        );
    }

    let mut buy_map: HashMap<i64, BuyInfo> = HashMap::new();
    for row in &buy_rows {
        let id: i64 = row.try_get("id")?;
        let date: NaiveDate = row.try_get("date")?;
        let deemed: Option<NaiveDate> = row.try_get("deemed_acquisition_date")?;
        buy_map.insert(
            id,
            BuyInfo {
                listing_id: row.try_get("listing_id")?,
                date,
                acquired: deemed.unwrap_or(date),
                quantity: parse_dec("quantity", row.try_get("quantity")?)?,
                average_price: parse_dec("average_price", row.try_get("average_price")?)?,
                brokerage: parse_dec("brokerage", row.try_get("brokerage")?)?,
                gst_on_brokerage: parse_dec("gst_on_brokerage", row.try_get("gst_on_brokerage")?)?,
                currency: row.try_get("currency")?,
                fx_rate: parse_dec("fx_rate", row.try_get("fx_rate")?)?,
            },
        );
    }

    let cba_reduction = crate::entities::amit_adjustment::db_cost_base_reductions(pool).await?;
    let roc_events =
        crate::entities::corporate_action::db_return_of_capital_events(pool).await?;
    // share splits/consolidations per listing (quantity re-basing)
    let split_events = crate::entities::corporate_action::db_share_split_events(pool).await?;

    let mut sale_proceeds: HashMap<i64, Decimal> = HashMap::new();
    let mut sale_cost_base: HashMap<i64, Decimal> = HashMap::new();
    let mut sale_discount_gain: HashMap<i64, Decimal> = HashMap::new();
    let mut sale_non_discount_gain: HashMap<i64, Decimal> = HashMap::new();
    let mut sale_loss: HashMap<i64, Decimal> = HashMap::new();

    for row in &alloc_rows {
        let sale_id: i64 = row.try_get("sale_trade_id")?;
        let buy_id: i64 = row.try_get("purchase_trade_id")?;
        let qty_alloc = parse_dec("quantity_allocated", row.try_get("quantity_allocated")?)?;

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
        // Convert to AUD at the sale's rate (ATO rate for the sale month, else the
        // sale's manual fx_rate) before aggregating.
        let alloc_proceeds = crate::infra::fx::to_aud(
            pool,
            alloc_proceeds,
            &sale.currency,
            sale.date,
            Some(sale.fx_rate),
        )
        .await?;

        // The allocated quantity is in the *sale date's* unit basis; the
        // purchase parcel's quantity and per-unit cost are as transacted. A
        // share split/consolidation between them (TD 2000/10) re-bases the
        // allocation back to as-acquired units so the cost-base pro-rating
        // spreads the unchanged total over the converted unit count.
        let splits = split_events.get(&buy.listing_id).map_or(&[][..], |v| v);
        let qty_alloc_acquired = crate::entities::corporate_action::as_acquired_quantity(
            qty_alloc, splits, buy.date, sale.date,
        );

        // Adjusted cost base of the allocated portion via the shared pipeline
        // (`domain::cost_base`), converted to AUD at the (possibly deemed)
        // acquisition month. `up_to` is the sale date: return-of-capital
        // payments after the sale don't touch these units — they were no
        // longer held.
        let alloc_cost = cost_base::adjusted_cost_base(
            &cost_base::Parcel {
                quantity: buy.quantity,
                average_price: buy.average_price,
                brokerage: buy.brokerage,
                gst_on_brokerage: buy.gst_on_brokerage,
                currency: &buy.currency,
                trade_date: buy.date,
            },
            qty_alloc_acquired,
            *cba_reduction.get(&buy_id).unwrap_or(&Decimal::ZERO),
            roc_events.get(&buy.listing_id).map_or(&[][..], |v| v),
            splits,
            Some(sale.date),
        )?
        .into_aud(pool, &buy.currency, buy.acquired, Some(buy.fx_rate))
        .await?
        .adjusted;

        let alloc_gain = alloc_proceeds - alloc_cost;

        *sale_proceeds.entry(sale_id).or_insert(Decimal::ZERO) += alloc_proceeds;
        *sale_cost_base.entry(sale_id).or_insert(Decimal::ZERO) += alloc_cost;

        // Classify each allocation's gain/loss for CGT: a gain from a parcel held
        // strictly > 12 months — from the (possibly deemed) acquisition date — is
        // discount-eligible; a gain from a parcel held ≤ 12 months is
        // non-discountable ("other" method); a negative result is a capital loss
        // (recorded as a positive amount). The net-capital-gain report nets these
        // buckets across sales and AMMA gains.
        if alloc_gain > Decimal::ZERO {
            if sale.date > buy.acquired + Months::new(12) {
                *sale_discount_gain.entry(sale_id).or_insert(Decimal::ZERO) += alloc_gain;
            } else {
                *sale_non_discount_gain.entry(sale_id).or_insert(Decimal::ZERO) += alloc_gain;
            }
        } else if alloc_gain < Decimal::ZERO {
            *sale_loss.entry(sale_id).or_insert(Decimal::ZERO) += -alloc_gain;
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
            let non_discount_gain =
                sale_non_discount_gain.get(&sale_id).copied().unwrap_or(Decimal::ZERO);
            let loss = sale_loss.get(&sale_id).copied().unwrap_or(Decimal::ZERO);
            Some(RealisedGainLoss {
                sale_trade_id: sale_id,
                listing_id: sale.listing_id,
                holding_account_id: sale.holding_account_id,
                sale_date: sale.date,
                proceeds,
                cost_base,
                capital_gain_loss: proceeds - cost_base,
                discount_eligible_gain: discount_gain,
                non_discountable_gain: non_discount_gain,
                capital_loss: loss,
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
) -> Result<Json<Vec<RealisedGainLoss>>, ApiError> {
    db_realised_gains(&pool)
        .await
        .map(Json)
        .map_err(ApiError::from)
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::StatusCode;
    use crate::{infra::db, entities::{amma, amit_adjustment, corporate_action, listing, parcel_allocation, rba_fx_rate, trade}};
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
                exchange_mic: Some("XASX".to_string()),
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
                brokerage_includes_gst: false,
                statement_total: None,
                holding_account_id: 1,
                transfer_id: None,
                ess_statement_id: None,
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
                residual_brought_forward: Decimal::ZERO,
                residual_carried_forward: Decimal::ZERO,
                residual_paid_out: Decimal::ZERO,
                rights_action_id: None,
                buyback_action_id: None,
                scrip_action_id: None,
                demerger_action_id: None,
                worthless_action_id: None,
                deemed_acquisition_date: None,
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
                brokerage_includes_gst: false,
                statement_total: None,
                holding_account_id: 1,
                transfer_id: None,
                ess_statement_id: None,
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
                residual_brought_forward: Decimal::ZERO,
                residual_carried_forward: Decimal::ZERO,
                residual_paid_out: Decimal::ZERO,
                rights_action_id: None,
                buyback_action_id: None,
                scrip_action_id: None,
                demerger_action_id: None,
                worthless_action_id: None,
                deemed_acquisition_date: None,
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
        let buy_date = NaiveDate::from_ymd_opt(2024, 1, 1).unwrap();
        let sell_date = NaiveDate::from_ymd_opt(2024, 6, 1).unwrap();
        insert_listing(&pool, 1, "VAS").await;
        trade::db_upsert(
            &pool,
            &trade::Trade {
                brokerage_includes_gst: false,
                statement_total: None,
                holding_account_id: 1,
                transfer_id: None,
                ess_statement_id: None,
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
                residual_brought_forward: Decimal::ZERO,
                residual_carried_forward: Decimal::ZERO,
                residual_paid_out: Decimal::ZERO,
                rights_action_id: None,
                buyback_action_id: None,
                scrip_action_id: None,
                demerger_action_id: None,
                worthless_action_id: None,
                deemed_acquisition_date: None,
            },
        )
        .await
        .unwrap();
        trade::db_upsert(
            &pool,
            &trade::Trade {
                brokerage_includes_gst: false,
                statement_total: None,
                holding_account_id: 1,
                transfer_id: None,
                ess_statement_id: None,
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
                residual_brought_forward: Decimal::ZERO,
                residual_carried_forward: Decimal::ZERO,
                residual_paid_out: Decimal::ZERO,
                rights_action_id: None,
                buyback_action_id: None,
                scrip_action_id: None,
                demerger_action_id: None,
                worthless_action_id: None,
                deemed_acquisition_date: None,
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

    /// Security identity continuity across a ticker/name change: a rename is an
    /// in-place edit to the listing (same id), so a sale entered after the
    /// rename still allocates against the pre-rename parcel — original cost
    /// base, and the 12-month discount clock keeps running from the original
    /// acquisition date.
    #[tokio::test]
    async fn db_sale_after_ticker_rename_keeps_cost_base_and_discount_clock() {
        let pool = test_pool().await;
        let buy_date = NaiveDate::from_ymd_opt(2024, 1, 1).unwrap();
        let sell_date = NaiveDate::from_ymd_opt(2025, 1, 2).unwrap(); // strictly > 12 months
        insert_listing(&pool, 1, "OLD").await;
        insert_buy(&pool, 1, 1, buy_date, Decimal::from(100), Decimal::from(10)).await;
        // The security is renamed before the sale: same listing id, new ticker + name.
        insert_listing(&pool, 1, "NEW").await;
        insert_sell(&pool, 2, 1, sell_date, Decimal::from(100), Decimal::from(15)).await;
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
        listing::db_upsert(
            &pool,
            &listing::Listing {
                id: 1,
                exchange_mic: None,
                ticker: "BTC".to_string(),
                name: "Bitcoin".to_string(),
                isin: None,
                security_type: listing::SecurityType::Crypto,
                currency: "AUD".to_string(),
                amit: false,
                preference: false,
            },
        )
        .await
        .unwrap();
        let buy_date = NaiveDate::from_ymd_opt(2024, 1, 1).unwrap();
        let sell_date = NaiveDate::from_ymd_opt(2025, 1, 2).unwrap(); // strictly > 12 months
        let qty: Decimal = "0.12345678".parse().unwrap();
        insert_buy(&pool, 1, 1, buy_date, qty, Decimal::from(60000)).await;
        insert_sell(&pool, 2, 1, sell_date, qty, Decimal::from(100000)).await;
        allocate(&pool, 1, 2, 1, qty).await;

        let result = db_realised_gains(&pool).await.unwrap();
        assert_eq!(result.len(), 1);
        // 0.12345678 × 60,000 / 100,000: every satoshi-scale digit preserved.
        assert_eq!(result[0].cost_base, "7407.40680000".parse::<Decimal>().unwrap());
        assert_eq!(result[0].proceeds, "12345.67800000".parse::<Decimal>().unwrap());
        assert_eq!(result[0].capital_gain_loss, "4938.27120000".parse::<Decimal>().unwrap());
        // Held > 12 months → the whole gain is discount-eligible.
        assert_eq!(result[0].discount_eligible_gain, result[0].capital_gain_loss);
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
        // The new (≤12mo) parcel's $250 gain is non-discountable; no losses.
        assert_eq!(result[0].non_discountable_gain, Decimal::from(250));
        assert_eq!(result[0].capital_loss, Decimal::ZERO);
        // Identity: capital_gain_loss == discount_eligible + non_discountable − loss.
        assert_eq!(
            result[0].capital_gain_loss,
            result[0].discount_eligible_gain + result[0].non_discountable_gain
                - result[0].capital_loss
        );
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
                holding_account_id: 1,
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
                currency: "AUD".to_string(),
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

    // Return of capital (CGT event G1)

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

    // Share splits / consolidations (TD 2000/10)

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

    /// TD 2000/10 (`docs/ato/share-splits-and-consolidations.md`): selling the whole
    /// post-split holding realises the parcel's full, unchanged cost base —
    /// the split itself is no CGT event and creates no gain.
    #[tokio::test]
    async fn db_post_split_sale_uses_unchanged_total_cost_base() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "SPL").await;
        // Buy 100 @ $10 (2024-01-01) → cost base 1000 (zero brokerage helper).
        insert_buy(&pool, 1, 1, NaiveDate::from_ymd_opt(2024, 1, 1).unwrap(),
            Decimal::from(100), Decimal::from(10)).await;
        // 2-for-1 split, then sell all 200 post-split units @ $6.
        apply_split(&pool, 1, 1, NaiveDate::from_ymd_opt(2024, 3, 1).unwrap(), "2", "1").await;
        insert_sell(&pool, 2, 1, NaiveDate::from_ymd_opt(2024, 6, 1).unwrap(),
            Decimal::from(200), Decimal::from(6)).await;
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
        insert_buy(&pool, 1, 1, NaiveDate::from_ymd_opt(2024, 1, 1).unwrap(),
            Decimal::from(100), Decimal::from(10)).await;
        apply_split(&pool, 1, 1, NaiveDate::from_ymd_opt(2024, 3, 1).unwrap(), "2", "1").await;
        insert_sell(&pool, 2, 1, NaiveDate::from_ymd_opt(2024, 6, 1).unwrap(),
            Decimal::from(80), Decimal::from(6)).await;
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
        insert_buy(&pool, 1, 1, NaiveDate::from_ymd_opt(2024, 1, 1).unwrap(),
            Decimal::from(100), Decimal::from(10)).await;
        // 1-for-10 bonus issue 16 months later: 100 → 110 units.
        corporate_action::db_upsert(
            &pool,
            &corporate_action::CorporateAction {
                id: 1,
                listing_id: 1,
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
        insert_sell(&pool, 2, 1, NaiveDate::from_ymd_opt(2025, 7, 1).unwrap(),
            Decimal::from(55), Decimal::from(12)).await;
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
        insert_buy(&pool, 1, 1, NaiveDate::from_ymd_opt(2024, 1, 1).unwrap(),
            Decimal::from(100), Decimal::from(10)).await;
        apply_split(&pool, 1, 1, NaiveDate::from_ymd_opt(2025, 5, 1).unwrap(), "2", "1").await;
        insert_sell(&pool, 2, 1, NaiveDate::from_ymd_opt(2025, 7, 1).unwrap(),
            Decimal::from(200), Decimal::from(8)).await;
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
        insert_buy(&pool, 1, 1, NaiveDate::from_ymd_opt(2024, 1, 1).unwrap(),
            Decimal::from(100), Decimal::from(10)).await;
        apply_split(&pool, 1, 1, NaiveDate::from_ymd_opt(2024, 3, 1).unwrap(), "2", "1").await;
        // 25c per post-split unit while all 200 are held.
        apply_roc(&pool, 2, 1, NaiveDate::from_ymd_opt(2024, 4, 1).unwrap(), "0.25").await;
        insert_sell(&pool, 2, 1, NaiveDate::from_ymd_opt(2024, 6, 1).unwrap(),
            Decimal::from(200), Decimal::from(6)).await;
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
        insert_buy(&pool, 1, 1, NaiveDate::from_ymd_opt(2024, 1, 1).unwrap(),
            Decimal::from(100), Decimal::from(10)).await;
        apply_roc(&pool, 1, 1, NaiveDate::from_ymd_opt(2024, 3, 1).unwrap(), "0.50").await;
        insert_sell(&pool, 2, 1, NaiveDate::from_ymd_opt(2024, 6, 1).unwrap(),
            Decimal::from(100), Decimal::from(12)).await;
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
        insert_buy(&pool, 1, 1, NaiveDate::from_ymd_opt(2024, 1, 1).unwrap(),
            Decimal::from(100), Decimal::from(10)).await;
        insert_sell(&pool, 2, 1, NaiveDate::from_ymd_opt(2024, 6, 1).unwrap(),
            Decimal::from(100), Decimal::from(12)).await;
        allocate(&pool, 1, 2, 1, Decimal::from(100)).await;
        // Payment after the sale.
        apply_roc(&pool, 1, 1, NaiveDate::from_ymd_opt(2024, 7, 1).unwrap(), "0.50").await;

        let result = db_realised_gains(&pool).await.unwrap();
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].cost_base, Decimal::from(1000));
        assert_eq!(result[0].capital_gain_loss, Decimal::from(200));
    }

    // FX conversion

    async fn insert_usd_listing(pool: &SqlitePool, id: i64, ticker: &str) {
        listing::db_upsert(
            pool,
            &listing::Listing {
                id,
                exchange_mic: Some("XNYS".to_string()),
                ticker: ticker.to_string(),
                name: ticker.to_string(),
                isin: None,
                security_type: listing::SecurityType::Share,
                currency: "USD".to_string(),
                amit: false,
                preference: false,
            },
        )
        .await
        .unwrap();
    }

    async fn insert_usd_trade(
        pool: &SqlitePool,
        id: i64,
        trade_type: trade::TradeType,
        date: NaiveDate,
        qty: Decimal,
        price: Decimal,
    ) {
        trade::db_upsert(
            pool,
            &trade::Trade {
                brokerage_includes_gst: false,
                statement_total: None,
                holding_account_id: 1,
                transfer_id: None,
                ess_statement_id: None,
                id,
                trade_type,
                date,
                settlement_date: date + chrono::Duration::days(2),
                listing_id: 1,
                average_price: price,
                quantity: qty,
                currency: "USD".to_string(),
                brokerage: Decimal::ZERO,
                gst_on_brokerage: Decimal::ZERO,
                brokerage_currency: "USD".to_string(),
                // A wrong manual override: the report must prefer the ATO rate.
                fx_rate: "0.99".parse().unwrap(),
                contract_note_ref: None,
                residual_brought_forward: Decimal::ZERO,
                residual_carried_forward: Decimal::ZERO,
                residual_paid_out: Decimal::ZERO,
                rights_action_id: None,
                buyback_action_id: None,
                scrip_action_id: None,
                demerger_action_id: None,
                worthless_action_id: None,
                deemed_acquisition_date: None,
            },
        )
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn db_usd_buy_sell_produces_aud_cost_base_and_gain_via_ato_rate() {
        let pool = test_pool().await;
        let buy_date = NaiveDate::from_ymd_opt(2024, 1, 15).unwrap();
        let sell_date = NaiveDate::from_ymd_opt(2025, 6, 1).unwrap();
        insert_usd_listing(&pool, 1, "AAPL").await;
        // ATO RBA rates (foreign-per-AUD): A$1 = 0.50 USD in Jan-2024, 0.60 in Jun-2025.
        rba_fx_rate::db_import_rate(&pool, "USD", "2024-01", "0.50".parse().unwrap())
            .await
            .unwrap();
        rba_fx_rate::db_import_rate(&pool, "USD", "2025-06", "0.60".parse().unwrap())
            .await
            .unwrap();

        // Buy 100 @ US$10 (US$1000), sell 100 @ US$15 (US$1500), zero brokerage.
        insert_usd_trade(&pool, 1, trade::TradeType::Buy, buy_date, Decimal::from(100), Decimal::from(10)).await;
        insert_usd_trade(&pool, 2, trade::TradeType::Sell, sell_date, Decimal::from(100), Decimal::from(15)).await;
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
        let buy_date = NaiveDate::from_ymd_opt(2024, 1, 15).unwrap();
        let sell_date = NaiveDate::from_ymd_opt(2025, 6, 1).unwrap();
        insert_usd_listing(&pool, 1, "AAPL").await;
        // No ATO rates imported → both trades fall back to their 0.99 manual override.
        insert_usd_trade(&pool, 1, trade::TradeType::Buy, buy_date, Decimal::from(100), Decimal::from(10)).await;
        insert_usd_trade(&pool, 2, trade::TradeType::Sell, sell_date, Decimal::from(100), Decimal::from(15)).await;
        allocate(&pool, 1, 2, 1, Decimal::from(100)).await;

        let result = db_realised_gains(&pool).await.unwrap();
        assert_eq!(result.len(), 1);
        // cost = US$1000 / 0.99, proceeds = US$1500 / 0.99
        assert_eq!(result[0].cost_base, Decimal::from(1000) / "0.99".parse::<Decimal>().unwrap());
        assert_eq!(result[0].proceeds, Decimal::from(1500) / "0.99".parse::<Decimal>().unwrap());
    }

    /// Take over listing `from` with listing `to`, 2 new units per 1 old, on
    /// `date`, and run the exchange. Returns the created group.
    async fn exchange_two_for_one(
        pool: &SqlitePool,
        action_id: i64,
        from: i64,
        to: i64,
        date: NaiveDate,
    ) -> crate::entities::scrip_exchange::Exchange {
        corporate_action::db_upsert(
            pool,
            &corporate_action::CorporateAction {
                id: action_id,
                listing_id: from,
                date,
                kind: corporate_action::ActionKind::ScripForScrip {
                    scrip_listing_id: to,
                    scrip_new_units: Decimal::from(2),
                    scrip_old_units: Decimal::ONE,
                },
            },
        )
        .await
        .unwrap();
        crate::entities::scrip_exchange::db_exchange(pool, action_id).await.unwrap()
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
        insert_buy(&pool, 1, 1, buy_date, Decimal::from(1000), "1.50".parse().unwrap()).await;
        exchange_two_for_one(&pool, 10, 1, 2, NaiveDate::from_ymd_opt(2024, 7, 1).unwrap())
            .await;

        let result = db_realised_gains(&pool).await.unwrap();
        assert!(result.is_empty(), "the rollover disposal is not a realised gain/loss");
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
        insert_buy(&pool, 1, 1, buy_date, Decimal::from(1000), "1.50".parse().unwrap()).await;
        let ex = exchange_two_for_one(
            &pool,
            10,
            1,
            2,
            NaiveDate::from_ymd_opt(2024, 7, 1).unwrap(),
        )
        .await;

        // Sell all 2,000 replacement units at $1.00 → $2,000 proceeds.
        insert_sell(&pool, 50, 2, sell_date, Decimal::from(2000), Decimal::ONE).await;
        allocate(&pool, 1, 50, ex.replacements[0].id, Decimal::from(2000)).await;

        let result = db_realised_gains(&pool).await.unwrap();
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].sale_trade_id, 50);
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
        let sell_date = NaiveDate::from_ymd_opt(2024, 12, 1).unwrap();
        insert_listing(&pool, 1, "OLD").await;
        insert_listing(&pool, 2, "NEW").await;
        insert_buy(&pool, 1, 1, buy_date, Decimal::from(1000), "1.50".parse().unwrap()).await;
        let ex = exchange_two_for_one(
            &pool,
            10,
            1,
            2,
            NaiveDate::from_ymd_opt(2024, 7, 1).unwrap(),
        )
        .await;

        insert_sell(&pool, 50, 2, sell_date, Decimal::from(2000), Decimal::ONE).await;
        allocate(&pool, 1, 50, ex.replacements[0].id, Decimal::from(2000)).await;

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
        let buy_date = NaiveDate::from_ymd_opt(2024, 1, 15).unwrap();
        let sell_date = NaiveDate::from_ymd_opt(2025, 6, 1).unwrap();
        insert_usd_listing(&pool, 1, "OLDQ").await;
        insert_usd_listing(&pool, 2, "NEWQ").await;
        // US$0.50/A$ in the buy month, 0.80 at the exchange, 0.60 at the sale.
        for (month, rate) in [("2024-01", "0.50"), ("2024-07", "0.80"), ("2025-06", "0.60")] {
            rba_fx_rate::db_import_rate(&pool, "USD", month, rate.parse().unwrap())
                .await
                .unwrap();
        }
        // US$10 × 100 = US$1,000 cost base = A$2,000 at the buy month.
        insert_usd_trade(&pool, 1, trade::TradeType::Buy, buy_date, Decimal::from(100), Decimal::from(10)).await;
        let ex = exchange_two_for_one(
            &pool,
            10,
            1,
            2,
            NaiveDate::from_ymd_opt(2024, 7, 1).unwrap(),
        )
        .await;

        // Sell the 200 replacement units for US$7.50 each = US$1,500 = A$2,500.
        trade::db_upsert(
            &pool,
            &trade::Trade {
                brokerage_includes_gst: false,
                statement_total: None,
                holding_account_id: 1,
                transfer_id: None,
                ess_statement_id: None,
                id: 50,
                trade_type: trade::TradeType::Sell,
                date: sell_date,
                settlement_date: sell_date + chrono::Duration::days(2),
                listing_id: 2,
                average_price: "7.50".parse().unwrap(),
                quantity: Decimal::from(200),
                currency: "USD".to_string(),
                brokerage: Decimal::ZERO,
                gst_on_brokerage: Decimal::ZERO,
                brokerage_currency: "USD".to_string(),
                fx_rate: "0.99".parse().unwrap(),
                contract_note_ref: None,
                residual_brought_forward: Decimal::ZERO,
                residual_carried_forward: Decimal::ZERO,
                residual_paid_out: Decimal::ZERO,
                rights_action_id: None,
                buyback_action_id: None,
                scrip_action_id: None,
                demerger_action_id: None,
                worthless_action_id: None,
                deemed_acquisition_date: None,
            },
        )
        .await
        .unwrap();
        allocate(&pool, 1, 50, ex.replacements[0].id, Decimal::from(200)).await;

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
    ) -> crate::entities::demerger::Demerge {
        corporate_action::db_upsert(
            pool,
            &corporate_action::CorporateAction {
                id: action_id,
                listing_id: from,
                date,
                kind: corporate_action::ActionKind::Demerger {
                    demerger_listing_id: to,
                    demerger_new_units: Decimal::ONE,
                    demerger_held_units: Decimal::from(5),
                    demerger_cost_base_pct: Decimal::from(20),
                },
            },
        )
        .await
        .unwrap();
        crate::entities::demerger::db_demerge(pool, action_id).await.unwrap()
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
        insert_buy(&pool, 1, 1, buy_date, Decimal::from(1000), "1.50".parse().unwrap()).await;
        demerge_one_for_five(&pool, 10, 1, 2, NaiveDate::from_ymd_opt(2024, 7, 1).unwrap())
            .await;

        let result = db_realised_gains(&pool).await.unwrap();
        assert!(result.is_empty(), "the rollover apportionment is not a realised gain/loss");
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
        insert_buy(&pool, 1, 1, buy_date, Decimal::from(1000), "1.50".parse().unwrap()).await;
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
        insert_sell(&pool, 50, 1, sell_date, Decimal::from(1000), Decimal::from(2)).await;
        allocate(&pool, 1, 50, dm.head_replacements[0].id, Decimal::from(1000)).await;
        insert_sell(&pool, 51, 2, sell_date, Decimal::from(200), Decimal::from(3)).await;
        allocate(&pool, 2, 51, dm.demerged_replacements[0].id, Decimal::from(200)).await;

        let mut result = db_realised_gains(&pool).await.unwrap();
        result.sort_by_key(|r| r.sale_trade_id);
        assert_eq!(result.len(), 2);
        // Head: $2,000 − $1,200 = $800, discount-eligible from Oct 2020.
        assert_eq!(result[0].sale_trade_id, 50);
        assert_eq!(result[0].cost_base, Decimal::from(1200));
        assert_eq!(result[0].capital_gain_loss, Decimal::from(800));
        assert_eq!(result[0].discount_eligible_gain, Decimal::from(800));
        assert_eq!(result[0].non_discountable_gain, Decimal::ZERO);
        // Demerged: $600 − $300 = $300, also discount-eligible from Oct 2020.
        assert_eq!(result[1].sale_trade_id, 51);
        assert_eq!(result[1].cost_base, Decimal::from(300));
        assert_eq!(result[1].capital_gain_loss, Decimal::from(300));
        assert_eq!(result[1].discount_eligible_gain, Decimal::from(300));
        assert_eq!(result[1].non_discountable_gain, Decimal::ZERO);
    }
}
