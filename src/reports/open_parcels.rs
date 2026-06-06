use crate::infra::decimal::parse_dec;
use axum::{Json, Router, extract::State, http::StatusCode, routing::get};
use chrono::NaiveDate;
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use sqlx::{Row, SqlitePool};
use std::collections::HashMap;

/// One open (not fully sold) purchase parcel — the per-parcel schedule a user
/// reconciles against a broker statement. All monetary figures are AUD,
/// converted at the parcel's buy-month ATO rate (manual `fx_rate` fallback).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OpenParcel {
    /// The Buy/DRP trade that created the parcel.
    pub trade_id: i64,
    pub listing_id: i64,
    pub ticker: String,
    /// Original acquisition date — preserved across share splits/consolidations
    /// (TD 2000/10), so the 12-month discount clock keeps running from here.
    pub acquisition_date: NaiveDate,
    /// Units as originally transacted (pre-split basis where a split followed).
    pub original_quantity: Decimal,
    /// Units not yet allocated to a Sell, in *current* units — after every
    /// recorded share split/consolidation — so it reconciles with a broker
    /// statement.
    pub remaining_quantity: Decimal,
    /// Whole-parcel cost base as acquired (price × qty + brokerage + GST), AUD.
    pub original_cost_base: Decimal,
    /// Cumulative AMIT cost-base reductions applied to the parcel to date, AUD
    /// (the full reduction, even where CGT event E10 has floored the cost base).
    pub amit_cost_base_reduction: Decimal,
    /// Cumulative return-of-capital payments (CGT event G1) received on the
    /// remaining units since acquisition, AUD (the full amount, even where the
    /// cost base has been floored at nil).
    pub return_of_capital_reduction: Decimal,
    /// Adjusted cost base of the remaining units, AUD: max(original − AMIT, 0)
    /// pro-rated to the remaining quantity, less return-of-capital payments on
    /// those units (CGT events E10 and G1 both floor the cost base at nil).
    pub remaining_cost_base: Decimal,
}

pub fn router() -> Router<SqlitePool> {
    Router::new().route("/portfolio/open-parcels", get(open_parcels_handler))
}

/// Lists every open parcel: a Buy/DRP trade whose quantity is not fully
/// consumed by parcel allocations. Same open-quantity and AMIT/E10 cost-base
/// rules as the portfolio/unrealised reports, but per parcel instead of
/// aggregated per listing.
pub async fn db_open_parcels(pool: &SqlitePool) -> Result<Vec<OpenParcel>, sqlx::Error> {
    let trade_rows = sqlx::query(
        "SELECT t.id, t.listing_id, l.ticker, t.date, t.quantity, t.average_price, \
         t.brokerage, t.gst_on_brokerage, t.currency, t.fx_rate \
         FROM trades t JOIN listings l ON l.id = t.listing_id \
         WHERE t.trade_type IN ('Buy', 'DRP')",
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

    let cba_reduction = crate::entities::amit_adjustment::db_cost_base_reductions(pool).await?;
    let roc_events =
        crate::entities::corporate_action::db_return_of_capital_events(pool).await?;
    // share splits/consolidations per listing (quantity re-basing)
    let split_events = crate::entities::corporate_action::db_share_split_events(pool).await?;

    let mut parcels = Vec::new();
    for row in &trade_rows {
        let trade_id: i64 = row.try_get("id")?;
        let listing_id: i64 = row.try_get("listing_id")?;
        let ticker: String = row.try_get("ticker")?;
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
        // CGT event E10: an AMIT cost base reduction can only take the cost base
        // to nil, never negative (the excess is a capital gain in the
        // net-capital-gain report).
        let net_cost = (initial_cost - amit).max(Decimal::ZERO);
        // Return-of-capital payments (CGT event G1) received on the remaining
        // units since acquisition; the cost base floors at nil (the excess is a
        // capital gain in the net-capital-gain report).
        let roc_per_unit = crate::entities::corporate_action::per_unit_reduction(
            roc_events.get(&listing_id).map_or(&[][..], |v| v),
            splits,
            &currency,
            trade_date,
            None,
        )?;
        let roc = roc_per_unit * remaining;
        let remaining_cost = if qty > Decimal::ZERO {
            (net_cost * remaining / qty - roc).max(Decimal::ZERO)
        } else {
            Decimal::ZERO
        };

        // Convert each figure to AUD at the parcel's buy-month ATO rate (the
        // trade's manual fx_rate as fallback) so the schedule is uniformly AUD.
        let original_cost_base =
            crate::infra::fx::to_aud(pool, initial_cost, &currency, trade_date, Some(fx_rate))
                .await?;
        let amit_cost_base_reduction =
            crate::infra::fx::to_aud(pool, amit, &currency, trade_date, Some(fx_rate)).await?;
        let return_of_capital_reduction =
            crate::infra::fx::to_aud(pool, roc, &currency, trade_date, Some(fx_rate)).await?;
        let remaining_cost_base =
            crate::infra::fx::to_aud(pool, remaining_cost, &currency, trade_date, Some(fx_rate))
                .await?;

        // The remaining quantity is reported in current units — after every
        // recorded split/consolidation — so it reconciles with a broker
        // statement; `original_quantity` stays as transacted.
        let remaining_now = crate::entities::corporate_action::split_adjusted_quantity(
            remaining, splits, trade_date, None,
        );

        parcels.push(OpenParcel {
            trade_id,
            listing_id,
            ticker,
            acquisition_date: trade_date,
            original_quantity: qty,
            remaining_quantity: remaining_now,
            original_cost_base,
            amit_cost_base_reduction,
            return_of_capital_reduction,
            remaining_cost_base,
        });
    }

    parcels.sort_by(|a, b| {
        (a.listing_id, a.acquisition_date, a.trade_id)
            .cmp(&(b.listing_id, b.acquisition_date, b.trade_id))
    });
    Ok(parcels)
}

async fn open_parcels_handler(
    State(pool): State<SqlitePool>,
) -> Result<Json<Vec<OpenParcel>>, StatusCode> {
    db_open_parcels(&pool)
        .await
        .map(Json)
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        entities::{amit_adjustment, amma, corporate_action, listing, parcel_allocation, trade},
        infra::db,
    };
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
        insert_buy_ccy(pool, id, listing_id, date, qty, price, "AUD").await;
    }

    async fn insert_buy_ccy(
        pool: &SqlitePool,
        id: i64,
        listing_id: i64,
        date: NaiveDate,
        qty: Decimal,
        price: Decimal,
        currency: &str,
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
                currency: currency.to_string(),
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
                date: NaiveDate::from_ymd_opt(2025, 6, 1).unwrap(),
                settlement_date: NaiveDate::from_ymd_opt(2025, 6, 3).unwrap(),
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
                buyback_action_id: None,
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

    /// AMMA statement carrying only a per-unit cost base adjustment, linked to
    /// `trade_id` over `qty` units.
    async fn apply_amit(
        pool: &SqlitePool,
        amma_id: i64,
        listing_id: i64,
        trade_id: i64,
        qty: Decimal,
        per_unit: Decimal,
    ) {
        amma::db_upsert(
            pool,
            &amma::AmmaStatement {
                id: amma_id,
                listing_id,
                tax_year_end_date: NaiveDate::from_ymd_opt(2024, 6, 30).unwrap(),
                units_held: qty,
                date_received: NaiveDate::from_ymd_opt(2024, 8, 15).unwrap(),
                cost_base_adjustment: per_unit,
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
            pool,
            &amit_adjustment::AmitAdjustment {
                id: amma_id,
                amma_statement_id: amma_id,
                trade_id,
                quantity: qty,
            },
        )
        .await
        .unwrap();
    }

    // DB-level tests

    #[tokio::test]
    async fn db_no_trades_returns_empty() {
        let pool = test_pool().await;
        let parcels = db_open_parcels(&pool).await.unwrap();
        assert!(parcels.is_empty());
    }

    #[tokio::test]
    async fn db_open_parcel_listed_with_original_figures() {
        let pool = test_pool().await;
        let buy_date = NaiveDate::from_ymd_opt(2024, 1, 1).unwrap();
        insert_listing(&pool, 1, "VAS").await;
        insert_buy(&pool, 1, 1, buy_date, Decimal::from(100), Decimal::from(10)).await;

        let parcels = db_open_parcels(&pool).await.unwrap();
        assert_eq!(parcels.len(), 1);
        let p = &parcels[0];
        assert_eq!(p.trade_id, 1);
        assert_eq!(p.listing_id, 1);
        assert_eq!(p.ticker, "VAS");
        assert_eq!(p.acquisition_date, buy_date);
        assert_eq!(p.original_quantity, Decimal::from(100));
        assert_eq!(p.remaining_quantity, Decimal::from(100));
        // cost = 10 * 100 + 9.95 + 0.995 = 1010.945
        assert_eq!(p.original_cost_base, "1010.945".parse::<Decimal>().unwrap());
        assert_eq!(p.amit_cost_base_reduction, Decimal::ZERO);
        assert_eq!(p.remaining_cost_base, "1010.945".parse::<Decimal>().unwrap());
    }

    /// Security identity continuity across a ticker/name change: a rename is an
    /// in-place edit to the listing (`PUT /listings/:id` with the same id), so
    /// every parcel stays attached — same listing id, new ticker, unchanged
    /// acquisition date and cost base. Nothing is keyed by ticker.
    #[tokio::test]
    async fn db_ticker_rename_keeps_parcels_attached_to_the_listing() {
        let pool = test_pool().await;
        let buy_date = NaiveDate::from_ymd_opt(2024, 1, 1).unwrap();
        insert_listing(&pool, 1, "OLD").await;
        insert_buy(&pool, 1, 1, buy_date, Decimal::from(100), Decimal::from(10)).await;

        // The security is renamed: same listing id, new ticker + name.
        insert_listing(&pool, 1, "NEW").await;

        let parcels = db_open_parcels(&pool).await.unwrap();
        assert_eq!(parcels.len(), 1);
        let p = &parcels[0];
        assert_eq!(p.listing_id, 1);
        assert_eq!(p.ticker, "NEW");
        assert_eq!(p.acquisition_date, buy_date);
        assert_eq!(p.remaining_quantity, Decimal::from(100));
        assert_eq!(p.remaining_cost_base, "1010.945".parse::<Decimal>().unwrap());
    }

    #[tokio::test]
    async fn db_partial_sell_pro_rates_remaining_cost_base() {
        let pool = test_pool().await;
        let buy_date = NaiveDate::from_ymd_opt(2024, 1, 1).unwrap();
        insert_listing(&pool, 1, "VAS").await;
        insert_buy(&pool, 1, 1, buy_date, Decimal::from(100), Decimal::from(10)).await;
        insert_sell(&pool, 2, 1, Decimal::from(40)).await;
        allocate(&pool, 1, 2, 1, Decimal::from(40)).await;

        let parcels = db_open_parcels(&pool).await.unwrap();
        assert_eq!(parcels.len(), 1);
        let p = &parcels[0];
        assert_eq!(p.original_quantity, Decimal::from(100));
        assert_eq!(p.remaining_quantity, Decimal::from(60));
        // original cost base reported for the whole parcel
        assert_eq!(p.original_cost_base, "1010.945".parse::<Decimal>().unwrap());
        // remaining = 1010.945 * 60 / 100 = 606.567
        assert_eq!(p.remaining_cost_base, "606.567".parse::<Decimal>().unwrap());
    }

    #[tokio::test]
    async fn db_fully_sold_parcel_excluded() {
        let pool = test_pool().await;
        let buy_date = NaiveDate::from_ymd_opt(2024, 1, 1).unwrap();
        insert_listing(&pool, 1, "VAS").await;
        insert_buy(&pool, 1, 1, buy_date, Decimal::from(100), Decimal::from(10)).await;
        insert_buy(&pool, 2, 1, buy_date, Decimal::from(50), Decimal::from(11)).await;
        insert_sell(&pool, 3, 1, Decimal::from(100)).await;
        allocate(&pool, 1, 3, 1, Decimal::from(100)).await;

        let parcels = db_open_parcels(&pool).await.unwrap();
        assert_eq!(parcels.len(), 1);
        assert_eq!(parcels[0].trade_id, 2);
        assert_eq!(parcels[0].remaining_quantity, Decimal::from(50));
    }

    #[tokio::test]
    async fn db_amit_reduction_reported_and_netted_off() {
        let pool = test_pool().await;
        let buy_date = NaiveDate::from_ymd_opt(2024, 1, 1).unwrap();
        insert_listing(&pool, 1, "VAF").await;
        insert_buy(&pool, 1, 1, buy_date, Decimal::from(100), Decimal::from(10)).await;
        // AMIT reduction = 100 * 0.05 = 5.00
        apply_amit(&pool, 1, 1, 1, Decimal::from(100), "0.05".parse().unwrap()).await;
        // Partial sell of 40 after the adjustment
        insert_sell(&pool, 2, 1, Decimal::from(40)).await;
        allocate(&pool, 1, 2, 1, Decimal::from(40)).await;

        let parcels = db_open_parcels(&pool).await.unwrap();
        assert_eq!(parcels.len(), 1);
        let p = &parcels[0];
        assert_eq!(p.remaining_quantity, Decimal::from(60));
        assert_eq!(p.original_cost_base, "1010.945".parse::<Decimal>().unwrap());
        assert_eq!(p.amit_cost_base_reduction, Decimal::from(5));
        // remaining = (1010.945 - 5.00) * 60 / 100 = 603.567
        assert_eq!(p.remaining_cost_base, "603.567".parse::<Decimal>().unwrap());
    }

    #[tokio::test]
    async fn db_e10_floors_remaining_cost_base_at_nil() {
        let pool = test_pool().await;
        let buy_date = NaiveDate::from_ymd_opt(2024, 1, 1).unwrap();
        insert_listing(&pool, 1, "VAF").await;
        insert_buy(&pool, 1, 1, buy_date, Decimal::from(100), Decimal::from(10)).await;
        // Reduction 100 * 11 = 1100 exceeds the 1010.945 cost base → E10 floor.
        apply_amit(&pool, 1, 1, 1, Decimal::from(100), Decimal::from(11)).await;

        let parcels = db_open_parcels(&pool).await.unwrap();
        assert_eq!(parcels.len(), 1);
        let p = &parcels[0];
        // The full cumulative reduction is still reported…
        assert_eq!(p.amit_cost_base_reduction, Decimal::from(1100));
        // …but the remaining cost base is floored at nil, never negative.
        assert_eq!(p.remaining_cost_base, Decimal::ZERO);
    }

    #[tokio::test]
    async fn db_non_aud_parcel_converted_to_aud() {
        let pool = test_pool().await;
        let buy_date = NaiveDate::from_ymd_opt(2024, 1, 10).unwrap();
        insert_listing(&pool, 1, "VTS").await;
        // A$1 = 0.50 USD for the buy month → AUD = USD / 0.50.
        crate::entities::rba_fx_rate::db_import_rate(
            &pool,
            "USD",
            "2024-01",
            "0.50".parse().unwrap(),
        )
        .await
        .unwrap();
        insert_buy_ccy(&pool, 1, 1, buy_date, Decimal::from(100), Decimal::from(10), "USD")
            .await;

        let parcels = db_open_parcels(&pool).await.unwrap();
        assert_eq!(parcels.len(), 1);
        // USD cost = 1010.945 → AUD 2021.89 at 0.50
        assert_eq!(
            parcels[0].original_cost_base,
            "2021.890".parse::<Decimal>().unwrap()
        );
        assert_eq!(
            parcels[0].remaining_cost_base,
            "2021.890".parse::<Decimal>().unwrap()
        );
    }

    #[tokio::test]
    async fn db_sorted_by_listing_then_acquisition_date() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "VAS").await;
        insert_listing(&pool, 2, "VGS").await;
        let d1 = NaiveDate::from_ymd_opt(2023, 5, 1).unwrap();
        let d2 = NaiveDate::from_ymd_opt(2024, 1, 1).unwrap();
        insert_buy(&pool, 1, 2, d1, Decimal::from(10), Decimal::from(10)).await;
        insert_buy(&pool, 2, 1, d2, Decimal::from(10), Decimal::from(10)).await;
        insert_buy(&pool, 3, 1, d1, Decimal::from(10), Decimal::from(10)).await;

        let parcels = db_open_parcels(&pool).await.unwrap();
        let order: Vec<(i64, NaiveDate)> =
            parcels.iter().map(|p| (p.listing_id, p.acquisition_date)).collect();
        assert_eq!(order, vec![(1, d1), (1, d2), (2, d1)]);
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

    /// TD 2000/10 (`docs/share-splits-and-consolidations.md`): the split parcel
    /// keeps its acquisition date and total cost base; the remaining quantity
    /// is reported in post-split units while original_quantity stays as
    /// transacted.
    #[tokio::test]
    async fn db_share_split_rebases_remaining_quantity_and_preserves_cost_base() {
        let pool = test_pool().await;
        let buy_date = NaiveDate::from_ymd_opt(2024, 1, 1).unwrap();
        insert_listing(&pool, 1, "SPL").await;
        insert_buy(&pool, 1, 1, buy_date, Decimal::from(100), Decimal::from(10)).await;
        apply_split(&pool, 1, 1, NaiveDate::from_ymd_opt(2024, 3, 1).unwrap(), "2", "1").await;
        // Sell 80 post-split units (= 40 as-acquired) after the split.
        insert_sell(&pool, 2, 1, Decimal::from(80)).await; // sale dated 2025-06-01
        allocate(&pool, 1, 2, 1, Decimal::from(80)).await;

        let parcels = db_open_parcels(&pool).await.unwrap();
        assert_eq!(parcels.len(), 1);
        let p = &parcels[0];
        assert_eq!(p.acquisition_date, buy_date);
        assert_eq!(p.original_quantity, Decimal::from(100));
        // 200 post-split − 80 sold = 120 in current units.
        assert_eq!(p.remaining_quantity, Decimal::from(120));
        assert_eq!(p.original_cost_base, "1010.945".parse::<Decimal>().unwrap());
        // 60 of the 100 as-acquired units remain: 1010.945 × 60/100.
        assert_eq!(p.remaining_cost_base, "606.567".parse::<Decimal>().unwrap());
    }

    /// A consolidation that doesn't divide the holding evenly keeps the exact
    /// fractional quantity (rounding/cash-in-lieu arrangements aren't modelled).
    #[tokio::test]
    async fn db_consolidation_rebases_remaining_quantity() {
        let pool = test_pool().await;
        let buy_date = NaiveDate::from_ymd_opt(2024, 1, 1).unwrap();
        insert_listing(&pool, 1, "CON").await;
        insert_buy(&pool, 1, 1, buy_date, Decimal::from(150), Decimal::from(10)).await;
        // 1-for-10 consolidation → 15 units.
        apply_split(&pool, 1, 1, NaiveDate::from_ymd_opt(2024, 3, 1).unwrap(), "1", "10").await;

        let parcels = db_open_parcels(&pool).await.unwrap();
        assert_eq!(parcels[0].remaining_quantity, Decimal::from(15));
        // Total cost base untouched: 10 × 150 + 9.95 + 0.995.
        assert_eq!(parcels[0].remaining_cost_base, "1510.945".parse::<Decimal>().unwrap());
    }

    /// A return of capital (CGT event G1) is reported per parcel and netted off
    /// the remaining cost base for the units still held
    /// (`docs/cgt-non-assessable-payments.md`).
    #[tokio::test]
    async fn db_return_of_capital_reduction_reported_and_netted_off() {
        let pool = test_pool().await;
        let buy_date = NaiveDate::from_ymd_opt(2024, 1, 1).unwrap();
        insert_listing(&pool, 1, "RAP").await;
        insert_buy(&pool, 1, 1, buy_date, Decimal::from(100), Decimal::from(10)).await;
        // Sell 40 first, then a 50c/unit payment on the 60 still held.
        insert_sell(&pool, 2, 1, Decimal::from(40)).await; // sale dated 2025-06-01
        allocate(&pool, 1, 2, 1, Decimal::from(40)).await;
        apply_roc(&pool, 1, 1, NaiveDate::from_ymd_opt(2025, 7, 1).unwrap(), "0.50").await;

        let parcels = db_open_parcels(&pool).await.unwrap();
        assert_eq!(parcels.len(), 1);
        let p = &parcels[0];
        assert_eq!(p.remaining_quantity, Decimal::from(60));
        // The payment lands only on the 60 remaining units: 60 × 0.50 = 30.
        assert_eq!(p.return_of_capital_reduction, Decimal::from(30));
        // remaining = 1010.945 × 60/100 − 30 = 606.567 − 30 = 576.567
        assert_eq!(p.remaining_cost_base, "576.567".parse::<Decimal>().unwrap());
    }

    /// G1 floors the remaining cost base at nil — the excess is a capital gain
    /// in the net-capital-gain report, never a negative cost base here.
    #[tokio::test]
    async fn db_return_of_capital_floors_remaining_cost_base_at_nil() {
        let pool = test_pool().await;
        let buy_date = NaiveDate::from_ymd_opt(2024, 1, 1).unwrap();
        insert_listing(&pool, 1, "RAP").await;
        insert_buy(&pool, 1, 1, buy_date, Decimal::from(100), Decimal::from(10)).await;
        // $11/unit × 100 = 1100 exceeds the 1010.945 cost base.
        apply_roc(&pool, 1, 1, NaiveDate::from_ymd_opt(2024, 3, 1).unwrap(), "11").await;

        let parcels = db_open_parcels(&pool).await.unwrap();
        assert_eq!(parcels.len(), 1);
        // The full payment is still reported…
        assert_eq!(parcels[0].return_of_capital_reduction, Decimal::from(1100));
        // …but the remaining cost base is floored at nil.
        assert_eq!(parcels[0].remaining_cost_base, Decimal::ZERO);
    }

    // API-level tests

    #[tokio::test]
    async fn api_get_open_parcels() {
        let pool = test_pool().await;
        let buy_date = NaiveDate::from_ymd_opt(2024, 1, 1).unwrap();
        insert_listing(&pool, 1, "VAS").await;
        insert_buy(&pool, 1, 1, buy_date, Decimal::from(100), Decimal::from(10)).await;
        insert_sell(&pool, 2, 1, Decimal::from(40)).await;
        allocate(&pool, 1, 2, 1, Decimal::from(40)).await;

        let resp = router()
            .with_state(pool)
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/portfolio/open-parcels")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let parcels: Vec<OpenParcel> = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(parcels.len(), 1);
        assert_eq!(parcels[0].ticker, "VAS");
        assert_eq!(parcels[0].remaining_quantity, Decimal::from(60));
        assert_eq!(
            parcels[0].remaining_cost_base,
            "606.567".parse::<Decimal>().unwrap()
        );
    }
}
