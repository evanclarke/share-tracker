use crate::domain::cost_base::{self, ParcelRow};
use crate::infra::decimal::parse_dec;
use crate::infra::fx::FxRates;
use crate::infra::http::ApiError;
use axum::{Json, Router, extract::State, routing::get};
use chrono::NaiveDate;
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use sqlx::{FromRow, Row, SqlitePool};
use std::collections::HashMap;

/// One open (not fully sold) purchase parcel — the per-parcel schedule a user
/// reconciles against a broker statement. All monetary figures are AUD,
/// converted at the parcel's buy-month ATO rate (manual `fx_rate` fallback).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OpenParcel {
    /// The Buy/DRP trade that created the parcel.
    pub trade_id: i64,
    pub listing_id: i64,
    /// The holding account the parcel sits in.
    pub holding_account_id: i64,
    pub ticker: String,
    /// Original acquisition date — preserved across share splits/consolidations
    /// (TD 2000/10), so the 12-month discount clock keeps running from here.
    /// A scrip-for-scrip replacement parcel reports the consumed parcel's
    /// acquisition date (its `deemed_acquisition_date` — the rollover's
    /// combined holding period), not the exchange date.
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
    // One read transaction: every input below comes from the same snapshot,
    // so an interleaved write can't yield e.g. an allocation whose parcel is
    // missing from the same read.
    let mut tx = pool.begin().await?;
    let raw_rows = sqlx::query(
        "SELECT t.id, t.listing_id, t.holding_account_id, l.ticker, t.date, t.quantity, \
         t.average_price, t.brokerage, t.gst_on_brokerage, t.currency, t.fx_rate, \
         t.spot_fx_rate, t.deemed_acquisition_date \
         FROM trades t JOIN listings l ON l.id = t.listing_id \
         WHERE t.trade_type IN ('Buy', 'DRP')",
    )
    .fetch_all(&mut *tx)
    .await?;
    // The shared trade-row mapping plus this report's joined ticker column.
    let trade_rows: Vec<(ParcelRow, String)> = raw_rows
        .iter()
        .map(|row| Ok((ParcelRow::from_row(row)?, row.try_get("ticker")?)))
        .collect::<Result<_, sqlx::Error>>()?;

    if trade_rows.is_empty() {
        // Nothing held; the dropped transaction was read-only.
        return Ok(vec![]);
    }

    // units sold per purchase parcel, with each sale's date so the allocated
    // quantity (in sale-date units) can be re-based across splits
    let alloc_rows = sqlx::query(
        "SELECT pa.purchase_trade_id, pa.quantity_allocated, s.date AS sale_date \
         FROM parcel_allocations pa JOIN trades s ON s.id = pa.sale_trade_id",
    )
    .fetch_all(&mut *tx)
    .await?;

    let mut qty_sold: HashMap<i64, Vec<(NaiveDate, Decimal)>> = HashMap::new();
    for row in &alloc_rows {
        let tid: i64 = row.try_get("purchase_trade_id")?;
        qty_sold.entry(tid).or_default().push((
            row.try_get("sale_date")?,
            parse_dec("quantity_allocated", row.try_get("quantity_allocated")?)?,
        ));
    }

    let cba_reduction = crate::entities::amit_adjustment::db_cost_base_reductions(&mut *tx).await?;
    let roc_events =
        crate::entities::corporate_action::db_return_of_capital_events(&mut *tx).await?;
    // share splits/consolidations per listing (quantity re-basing)
    let split_events = crate::entities::corporate_action::db_share_split_events(&mut *tx).await?;
    // every imported ATO FX rate — per-parcel conversions below are map
    // lookups, not one DB round-trip each
    let fx = FxRates::load(&mut *tx).await?;
    tx.commit().await?;

    let mut parcels = Vec::new();
    for (t, ticker) in &trade_rows {
        // A scrip-for-scrip replacement parcel carries the consumed parcel's
        // acquisition date (`t.acquired()`): it drives the reported
        // acquisition date and the AUD translation month (the rollover
        // carries the AUD cost base over); split/ROC applicability stays on
        // the actual trade date.
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

        // The shared pipeline (`domain::cost_base`) for the remaining units —
        // `up_to: None`: an unsold unit was held for every payment since
        // acquisition — with every figure converted to AUD at the parcel's
        // acquisition-month ATO rate (the trade's manual fx_rate as fallback)
        // so the schedule is uniformly AUD.
        let cb = cost_base::adjusted_cost_base(
            &t.parcel(),
            remaining,
            *cba_reduction.get(&t.id).unwrap_or(&Decimal::ZERO),
            roc_events.get(&t.listing_id).map_or(&[][..], |v| v),
            splits,
            None,
        )?
        .into_aud_with(&fx, &t.currency, t.acquired(), t.fx_override())?;

        // The remaining quantity is reported in current units — after every
        // recorded split/consolidation — so it reconciles with a broker
        // statement; `original_quantity` stays as transacted.
        let remaining_now = crate::entities::corporate_action::split_adjusted_quantity(
            remaining, splits, t.date, None,
        );

        parcels.push(OpenParcel {
            trade_id: t.id,
            listing_id: t.listing_id,
            holding_account_id: t.holding_account_id,
            ticker: ticker.clone(),
            acquisition_date: t.acquired(),
            original_quantity: t.quantity,
            remaining_quantity: remaining_now,
            original_cost_base: cb.initial_cost,
            amit_cost_base_reduction: cb.amit_reduction,
            return_of_capital_reduction: cb.roc_reduction,
            remaining_cost_base: cb.adjusted,
        });
    }

    parcels.sort_by(|a, b| {
        (
            a.listing_id,
            a.holding_account_id,
            a.acquisition_date,
            a.trade_id,
        )
            .cmp(&(
                b.listing_id,
                b.holding_account_id,
                b.acquisition_date,
                b.trade_id,
            ))
    });
    Ok(parcels)
}

async fn open_parcels_handler(
    State(pool): State<SqlitePool>,
) -> Result<Json<Vec<OpenParcel>>, ApiError> {
    db_open_parcels(&pool)
        .await
        .map(Json)
        .map_err(ApiError::from)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::entities::corporate_action;
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
        test_support::buy(id, listing_id)
            .date(date)
            .qty(qty)
            .price(price)
            .currency(currency)
            .with(|t| t.brokerage_currency = "AUD".to_string())
            .brokerage(dec("9.95"))
            .gst_on_brokerage(dec("0.995"))
            .insert(pool)
            .await;
    }

    async fn insert_sell(pool: &SqlitePool, id: i64, listing_id: i64, qty: Decimal) {
        test_support::sell(id, listing_id)
            .date(ymd(2025, 6, 1))
            .qty(qty)
            .price(Decimal::from(120))
            .brokerage(dec("9.95"))
            .gst_on_brokerage(dec("0.995"))
            .insert(pool)
            .await;
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
        test_support::amma(amma_id, listing_id)
            .units(qty)
            .cost_base_adjustment(per_unit)
            .insert(pool)
            .await;
        test_support::amit_adjustment(pool, amma_id, amma_id, trade_id, qty).await;
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
        assert_eq!(
            p.remaining_cost_base,
            "1010.945".parse::<Decimal>().unwrap()
        );
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
        assert_eq!(
            p.remaining_cost_base,
            "1010.945".parse::<Decimal>().unwrap()
        );
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
        insert_buy_ccy(
            &pool,
            1,
            1,
            buy_date,
            Decimal::from(100),
            Decimal::from(10),
            "USD",
        )
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
    async fn db_spot_fx_rate_wins_over_monthly_rate() {
        let pool = test_pool().await;
        let buy_date = NaiveDate::from_ymd_opt(2024, 1, 10).unwrap();
        insert_listing(&pool, 1, "VTS").await;
        // A monthly rate exists (0.50), but the parcel carries a deliberate
        // transaction-date spot rate (0.40) that must win (QC 18020):
        // USD 1000 / 0.40 = AUD 2500, not / 0.50 = 2000.
        crate::entities::rba_fx_rate::db_import_rate(
            &pool,
            "USD",
            "2024-01",
            "0.50".parse().unwrap(),
        )
        .await
        .unwrap();
        test_support::buy(1, 1)
            .date(buy_date)
            .qty(Decimal::from(100))
            .price(Decimal::from(10))
            .currency("USD")
            .spot_fx_rate("0.40".parse().unwrap())
            .insert(&pool)
            .await;

        let parcels = db_open_parcels(&pool).await.unwrap();
        assert_eq!(parcels.len(), 1);
        assert_eq!(
            parcels[0].remaining_cost_base,
            Decimal::from(2500),
            "the deliberate spot rate converts, not the monthly rate"
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
        let order: Vec<(i64, NaiveDate)> = parcels
            .iter()
            .map(|p| (p.listing_id, p.acquisition_date))
            .collect();
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

    /// TD 2000/10 (`docs/ato/share-splits-and-consolidations.md`): the split parcel
    /// keeps its acquisition date and total cost base; the remaining quantity
    /// is reported in post-split units while original_quantity stays as
    /// transacted.
    #[tokio::test]
    async fn db_share_split_rebases_remaining_quantity_and_preserves_cost_base() {
        let pool = test_pool().await;
        let buy_date = NaiveDate::from_ymd_opt(2024, 1, 1).unwrap();
        insert_listing(&pool, 1, "SPL").await;
        insert_buy(&pool, 1, 1, buy_date, Decimal::from(100), Decimal::from(10)).await;
        apply_split(
            &pool,
            1,
            1,
            NaiveDate::from_ymd_opt(2024, 3, 1).unwrap(),
            "2",
            "1",
        )
        .await;
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
        apply_split(
            &pool,
            1,
            1,
            NaiveDate::from_ymd_opt(2024, 3, 1).unwrap(),
            "1",
            "10",
        )
        .await;

        let parcels = db_open_parcels(&pool).await.unwrap();
        assert_eq!(parcels[0].remaining_quantity, Decimal::from(15));
        // Total cost base untouched: 10 × 150 + 9.95 + 0.995.
        assert_eq!(
            parcels[0].remaining_cost_base,
            "1510.945".parse::<Decimal>().unwrap()
        );
    }

    /// A return of capital (CGT event G1) is reported per parcel and netted off
    /// the remaining cost base for the units still held
    /// (`docs/ato/cgt-non-assessable-payments.md`).
    #[tokio::test]
    async fn db_return_of_capital_reduction_reported_and_netted_off() {
        let pool = test_pool().await;
        let buy_date = NaiveDate::from_ymd_opt(2024, 1, 1).unwrap();
        insert_listing(&pool, 1, "RAP").await;
        insert_buy(&pool, 1, 1, buy_date, Decimal::from(100), Decimal::from(10)).await;
        // Sell 40 first, then a 50c/unit payment on the 60 still held.
        insert_sell(&pool, 2, 1, Decimal::from(40)).await; // sale dated 2025-06-01
        allocate(&pool, 1, 2, 1, Decimal::from(40)).await;
        apply_roc(
            &pool,
            1,
            1,
            NaiveDate::from_ymd_opt(2025, 7, 1).unwrap(),
            "0.50",
        )
        .await;

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
        apply_roc(
            &pool,
            1,
            1,
            NaiveDate::from_ymd_opt(2024, 3, 1).unwrap(),
            "11",
        )
        .await;

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

    /// A scrip-for-scrip replacement parcel is an ordinary open parcel, but
    /// reports the consumed parcel's acquisition date (the rollover's
    /// combined holding period) and its carried cost base — and a non-AUD
    /// carried cost base converts at the *original* buy month's ATO rate, so
    /// the AUD figure is unchanged by the exchange.
    #[tokio::test]
    async fn db_scrip_replacement_parcel_reports_carried_date_and_cost_base() {
        let pool = test_pool().await;
        let buy_date = NaiveDate::from_ymd_opt(2024, 1, 10).unwrap();
        insert_listing(&pool, 1, "OLD").await;
        insert_listing(&pool, 2, "NEW").await;
        // A$1 = 0.50 USD in the buy month, 0.80 in the exchange month.
        for (month, rate) in [("2024-01", "0.50"), ("2024-07", "0.80")] {
            crate::entities::rba_fx_rate::db_import_rate(
                &pool,
                "USD",
                month,
                rate.parse().unwrap(),
            )
            .await
            .unwrap();
        }
        // US$10 × 100 + US$9.95 + US$0.995 = US$1,010.945 = A$2,021.89.
        insert_buy_ccy(
            &pool,
            1,
            1,
            buy_date,
            Decimal::from(100),
            Decimal::from(10),
            "USD",
        )
        .await;
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

        let parcels = db_open_parcels(&pool).await.unwrap();
        // The original parcel is fully consumed; only the replacement is open.
        assert_eq!(parcels.len(), 1);
        let p = &parcels[0];
        assert_eq!(p.listing_id, 2);
        assert_eq!(p.acquisition_date, buy_date);
        assert_eq!(p.remaining_quantity, Decimal::from(200));
        // A$2,021.89 at the Jan-2024 rate — not US$1,010.945 / 0.80.
        assert_eq!(p.original_cost_base, "2021.890".parse::<Decimal>().unwrap());
        assert_eq!(
            p.remaining_cost_base,
            "2021.890".parse::<Decimal>().unwrap()
        );
    }

    /// Demerger head and demerged parcels are ordinary open parcels, each
    /// reporting the consumed parcel's acquisition date (the head dates are
    /// unchanged by law; the new interests' discount clock runs from the
    /// original acquisition) and its share of the apportioned cost base.
    #[tokio::test]
    async fn db_demerger_parcels_report_carried_date_and_apportioned_cost_base() {
        let pool = test_pool().await;
        let buy_date = NaiveDate::from_ymd_opt(2024, 1, 10).unwrap();
        insert_listing(&pool, 1, "HEAD").await;
        insert_listing(&pool, 2, "DEM").await;
        // 100 @ $10 + $9.95 + $0.995 = $1,010.945 cost base.
        insert_buy(&pool, 1, 1, buy_date, Decimal::from(100), Decimal::from(10)).await;
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

        let mut parcels = db_open_parcels(&pool).await.unwrap();
        // The original parcel is fully consumed; both replacement legs are
        // open and keep the original acquisition date.
        parcels.sort_by_key(|p| p.listing_id);
        assert_eq!(parcels.len(), 2);
        let head = &parcels[0];
        assert_eq!(head.listing_id, 1);
        assert_eq!(head.acquisition_date, buy_date);
        assert_eq!(head.remaining_quantity, Decimal::from(100));
        // 80% of $1,010.945 = $808.756.
        assert_eq!(
            head.remaining_cost_base,
            "808.756".parse::<Decimal>().unwrap()
        );
        let demerged = &parcels[1];
        assert_eq!(demerged.listing_id, 2);
        assert_eq!(demerged.acquisition_date, buy_date);
        assert_eq!(demerged.remaining_quantity, Decimal::from(20));
        // 20% of $1,010.945 = $202.189.
        assert_eq!(
            demerged.remaining_cost_base,
            "202.189".parse::<Decimal>().unwrap()
        );
    }
}
