//! Wash-sale alert report: the TR 2008/1 fact pattern, surfaced before the
//! ATO's data matching finds it.
//!
//! A wash sale is a disposal that realises a capital loss with no real change
//! in economic exposure — typically selling and re-acquiring the same asset a
//! short time apart — entered into to apply the loss against a gain. Part IVA
//! may cancel such a loss (`docs/ato/wash-sales.md`; TR 2008/1). There is no
//! statutory window: 24 hours apart failed (Example 2) while 3 days apart
//! survived where market-driven (Example 6) — so this report is advisory
//! only. Writes are never rejected, the flagged loss still counts in every
//! CGT report, and whether Part IVA could apply is a facts-and-circumstances
//! judgment the report leaves to the taxpayer.

use crate::infra::decimal::row_dec;
use crate::infra::http::ApiError;
use crate::reports::realised_gains::{self, DisposalSource};
use axum::{Json, Router, extract::State, routing::post};
use chrono::NaiveDate;
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use sqlx::{Row, SqlitePool};

/// Review window when none is requested: a Buy within 30 days either side of
/// the loss Sell is flagged. A convention for review, not an ATO bright line
/// (TR 2008/1 has no statutory window).
pub const DEFAULT_WINDOW_DAYS: i64 = 30;

/// One loss-Sell × re-acquisition pair inside the window. A Sell with several
/// nearby Buys yields one row per Buy.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WashSaleAlert {
    pub sale_trade_id: i64,
    pub listing_id: i64,
    pub ticker: String,
    pub sale_holding_account_id: i64,
    pub sale_date: NaiveDate,
    /// The sale's realised capital loss across its loss-making allocations
    /// (AUD, positive) — the figure Part IVA would cancel.
    pub capital_loss: Decimal,
    pub buy_trade_id: i64,
    /// May differ from the sale's account: a repurchase in another holding
    /// account of the same beneficial owner changes nothing economically.
    pub buy_holding_account_id: i64,
    pub buy_date: NaiveDate,
    pub buy_quantity: Decimal,
    /// buy_date − sale_date in days; negative = acquired before the sale
    /// (TR 2008/1 para 4(b)), zero or positive = at/after it (para 4(a)).
    pub days_apart: i64,
}

#[derive(Debug, Default, Deserialize)]
pub struct WashSalesRequest {
    /// Days either side of the sale to scan; defaults to
    /// [`DEFAULT_WINDOW_DAYS`].
    pub window_days: Option<i64>,
}

pub fn router() -> Router<SqlitePool> {
    Router::new().route("/reports/wash_sales", post(report))
}

/// Flag every loss-realising Sell (any allocation realised a capital loss,
/// per the realised-gains report — so the rollover/transfer exclusions and
/// the whole adjusted-cost-base pipeline apply unchanged) that has a Buy or
/// DRP of the same listing dated within `window_days` either side, across
/// all holding accounts. Provenance Buys that merely continue or relocate an
/// existing holding are not re-acquisitions and never match: holding-account
/// transfer-ins, scrip-for-scrip replacements, demerger Buys, and inheritance
/// Buys (no purposive re-acquisition by the taxpayer). Rights-exercise and
/// ESS Buys do match — they are genuine new acquisitions of the same listing.
/// Date-pattern only: the Buy a Sell drew its own allocations from can match
/// (the dates speak for themselves either way) — the report surfaces the
/// pattern, it does not judge purpose.
pub async fn db_wash_sales(
    pool: &SqlitePool,
    window_days: i64,
) -> Result<Vec<WashSaleAlert>, sqlx::Error> {
    let losses: Vec<_> = realised_gains::db_realised_gains(pool)
        .await?
        .into_iter()
        .filter(|r| r.source == DisposalSource::Sell && r.capital_loss > Decimal::ZERO)
        .collect();
    if losses.is_empty() {
        return Ok(Vec::new());
    }

    let buy_rows = sqlx::query(
        "SELECT t.id, t.listing_id, t.holding_account_id, t.date, t.quantity, l.ticker \
         FROM trades t JOIN listings l ON l.id = t.listing_id \
         WHERE t.trade_type IN ('Buy', 'DRP') \
           AND t.transfer_id IS NULL AND t.scrip_action_id IS NULL \
           AND t.demerger_action_id IS NULL AND t.inheritance_id IS NULL \
         ORDER BY t.date, t.id",
    )
    .fetch_all(pool)
    .await?;

    let mut alerts = Vec::new();
    for sale in &losses {
        for row in &buy_rows {
            let buy_listing_id: i64 = row.try_get("listing_id")?;
            if buy_listing_id != sale.listing_id {
                continue;
            }
            let buy_date: NaiveDate = row.try_get("date")?;
            let days_apart = (buy_date - sale.sale_date).num_days();
            if days_apart.abs() > window_days {
                continue;
            }
            alerts.push(WashSaleAlert {
                sale_trade_id: sale.sale_trade_id,
                listing_id: sale.listing_id,
                ticker: row.try_get("ticker")?,
                sale_holding_account_id: sale.holding_account_id,
                sale_date: sale.sale_date,
                capital_loss: sale.capital_loss,
                buy_trade_id: row.try_get("id")?,
                buy_holding_account_id: row.try_get("holding_account_id")?,
                buy_date,
                buy_quantity: row_dec(row, "quantity")?,
                days_apart,
            });
        }
    }
    // db_realised_gains is sale-date-ordered and buy_rows date-ordered, so
    // rows arrive grouped by sale in date order already; no re-sort needed.
    Ok(alerts)
}

async fn report(
    State(pool): State<SqlitePool>,
    Json(req): Json<WashSalesRequest>,
) -> Result<Json<Vec<WashSaleAlert>>, ApiError> {
    let window_days = req.window_days.unwrap_or(DEFAULT_WINDOW_DAYS);
    if window_days < 1 {
        return Err(ApiError::Unprocessable(
            "window_days must be at least 1".to_string(),
        ));
    }
    db_wash_sales(&pool, window_days)
        .await
        .map(Json)
        .map_err(ApiError::from)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::entities::holding_account;
    use crate::test_support::{self, allocate, test_pool, ymd};
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

    /// A realised loss: buy 100 @ $15 (parcel), sell 100 @ $10 → loss $500.
    async fn loss_sale_fixture(pool: &SqlitePool, sell_date: NaiveDate) {
        insert_listing(pool, 1, "LOSS").await;
        test_support::buy(1, 1)
            .date(ymd(2023, 1, 10))
            .qty(Decimal::from(100))
            .price(Decimal::from(15))
            .insert(pool)
            .await;
        test_support::sell(2, 1)
            .date(sell_date)
            .qty(Decimal::from(100))
            .price(Decimal::from(10))
            .insert(pool)
            .await;
        allocate(pool, 1, 2, 1, Decimal::from(100)).await;
    }

    #[tokio::test]
    async fn db_loss_sell_with_repurchase_inside_window_is_flagged() {
        let pool = test_pool().await;
        loss_sale_fixture(&pool, ymd(2025, 6, 10)).await;
        // Re-buy the same listing 5 days later.
        test_support::buy(3, 1)
            .date(ymd(2025, 6, 15))
            .qty(Decimal::from(100))
            .price(Decimal::from(10))
            .insert(&pool)
            .await;

        let alerts = db_wash_sales(&pool, DEFAULT_WINDOW_DAYS).await.unwrap();
        assert_eq!(alerts.len(), 1);
        let a = &alerts[0];
        assert_eq!(a.sale_trade_id, 2);
        assert_eq!(a.ticker, "LOSS");
        assert_eq!(a.capital_loss, Decimal::from(500));
        assert_eq!(a.buy_trade_id, 3);
        assert_eq!(a.days_apart, 5);
    }

    /// Window edges, both sides: a Buy exactly `window` days away (before or
    /// after) is flagged; one day further is not.
    #[tokio::test]
    async fn db_window_edges_inclusive_either_side() {
        for (buy_date, expect) in [
            (ymd(2025, 7, 10), true),  // +30 days
            (ymd(2025, 7, 11), false), // +31 days
            (ymd(2025, 5, 11), true),  // −30 days
            (ymd(2025, 5, 10), false), // −31 days
        ] {
            let pool = test_pool().await;
            loss_sale_fixture(&pool, ymd(2025, 6, 10)).await;
            test_support::buy(3, 1)
                .date(buy_date)
                .qty(Decimal::from(50))
                .price(Decimal::from(9))
                .insert(&pool)
                .await;
            let alerts = db_wash_sales(&pool, 30).await.unwrap();
            assert_eq!(
                alerts.iter().any(|a| a.buy_trade_id == 3),
                expect,
                "buy on {buy_date} within ±30 days of 2025-06-10 should flag={expect}"
            );
        }
    }

    /// The window is configurable: a 10-days-later Buy escapes a 5-day window
    /// and is caught by a 15-day one.
    #[tokio::test]
    async fn db_window_is_configurable() {
        let pool = test_pool().await;
        loss_sale_fixture(&pool, ymd(2025, 6, 10)).await;
        test_support::buy(3, 1)
            .date(ymd(2025, 6, 20))
            .qty(Decimal::from(100))
            .price(Decimal::from(10))
            .insert(&pool)
            .await;
        assert!(db_wash_sales(&pool, 5).await.unwrap().is_empty());
        assert_eq!(db_wash_sales(&pool, 15).await.unwrap().len(), 1);
    }

    /// A repurchase in a *different* holding account is the same beneficial
    /// owner re-instating exposure — flagged all the same.
    #[tokio::test]
    async fn db_repurchase_in_another_account_is_flagged() {
        let pool = test_pool().await;
        loss_sale_fixture(&pool, ymd(2025, 6, 10)).await;
        holding_account::db_upsert(
            &pool,
            &holding_account::HoldingAccount {
                id: 2,
                name: "Second broker".to_string(),
            },
        )
        .await
        .unwrap();
        test_support::buy(3, 1)
            .date(ymd(2025, 6, 15))
            .account(2)
            .qty(Decimal::from(100))
            .price(Decimal::from(10))
            .insert(&pool)
            .await;

        let alerts = db_wash_sales(&pool, DEFAULT_WINDOW_DAYS).await.unwrap();
        assert_eq!(alerts.len(), 1);
        assert_eq!(alerts[0].sale_holding_account_id, 1);
        assert_eq!(alerts[0].buy_holding_account_id, 2);
    }

    /// A gain-realising Sell is not a wash-sale candidate, however close the
    /// repurchase: there is no loss for Part IVA to cancel.
    #[tokio::test]
    async fn db_gain_sell_is_not_flagged() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "GAIN").await;
        test_support::buy(1, 1)
            .date(ymd(2023, 1, 10))
            .qty(Decimal::from(100))
            .price(Decimal::from(10))
            .insert(&pool)
            .await;
        test_support::sell(2, 1)
            .date(ymd(2025, 6, 10))
            .qty(Decimal::from(100))
            .price(Decimal::from(15))
            .insert(&pool)
            .await;
        allocate(&pool, 1, 2, 1, Decimal::from(100)).await;
        test_support::buy(3, 1)
            .date(ymd(2025, 6, 11))
            .qty(Decimal::from(100))
            .price(Decimal::from(15))
            .insert(&pool)
            .await;

        assert!(
            db_wash_sales(&pool, DEFAULT_WINDOW_DAYS)
                .await
                .unwrap()
                .is_empty()
        );
    }

    /// A Sell that nets a gain overall but realises a loss on one allocation
    /// is still loss-realising — each parcel is a separate CGT asset, and the
    /// loss allocation is what a wash-sale arrangement harvests.
    #[tokio::test]
    async fn db_mixed_sell_with_a_loss_allocation_is_flagged() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "MIX").await;
        // Parcel 1 gains $500; parcel 2 loses $200 → net +$300, loss $200.
        test_support::buy(1, 1)
            .date(ymd(2023, 1, 10))
            .qty(Decimal::from(100))
            .price(Decimal::from(10))
            .insert(&pool)
            .await;
        test_support::buy(2, 1)
            .date(ymd(2024, 1, 10))
            .qty(Decimal::from(100))
            .price(Decimal::from(17))
            .insert(&pool)
            .await;
        test_support::sell(3, 1)
            .date(ymd(2025, 6, 10))
            .qty(Decimal::from(200))
            .price(Decimal::from(15))
            .insert(&pool)
            .await;
        allocate(&pool, 1, 3, 1, Decimal::from(100)).await;
        allocate(&pool, 2, 3, 2, Decimal::from(100)).await;
        test_support::buy(4, 1)
            .date(ymd(2025, 6, 20))
            .qty(Decimal::from(100))
            .price(Decimal::from(15))
            .insert(&pool)
            .await;

        let alerts = db_wash_sales(&pool, DEFAULT_WINDOW_DAYS).await.unwrap();
        assert_eq!(alerts.len(), 1);
        assert_eq!(alerts[0].capital_loss, Decimal::from(200));
    }

    /// A Buy of a *different* listing inside the window is not a re-acquisition
    /// of the same asset (TR 2008/1 para 6: shares in another company are not
    /// "substantially the same") — never flagged.
    #[tokio::test]
    async fn db_buy_of_other_listing_is_not_flagged() {
        let pool = test_pool().await;
        loss_sale_fixture(&pool, ymd(2025, 6, 10)).await;
        insert_listing(&pool, 2, "OTHER").await;
        test_support::buy(3, 2)
            .date(ymd(2025, 6, 15))
            .qty(Decimal::from(100))
            .price(Decimal::from(10))
            .insert(&pool)
            .await;

        assert!(
            db_wash_sales(&pool, DEFAULT_WINDOW_DAYS)
                .await
                .unwrap()
                .is_empty()
        );
    }

    /// A holding-account transfer creates paired Sell/Buy trades, but nothing
    /// is disposed of or re-acquired — the transfer-out Sell realises no loss
    /// (excluded by the realised-gains report) and the transfer-in Buys are
    /// not re-acquisitions, so a transfer near a genuine loss Sell of another
    /// parcel must not flag the transfer-in Buy.
    #[tokio::test]
    async fn db_transfer_in_buy_is_not_a_reacquisition() {
        let pool = test_pool().await;
        loss_sale_fixture(&pool, ymd(2025, 6, 10)).await;
        holding_account::db_upsert(
            &pool,
            &holding_account::HoldingAccount {
                id: 2,
                name: "Second broker".to_string(),
            },
        )
        .await
        .unwrap();
        // A second parcel, transferred to account 2 five days after the loss
        // Sell: the transfer-in Buy lands inside the window.
        test_support::buy(3, 1)
            .date(ymd(2024, 1, 10))
            .qty(Decimal::from(50))
            .price(Decimal::from(12))
            .insert(&pool)
            .await;
        crate::entities::transfer::db_transfer(
            &pool,
            1,
            &crate::entities::transfer::TransferBody {
                listing_id: 1,
                date: ymd(2025, 6, 15),
                from_account_id: 1,
                to_account_id: 2,
                allocations: vec![crate::entities::sell::AllocationInput {
                    purchase_trade_id: 3,
                    quantity_allocated: Decimal::from(50),
                }],
                fee_allocations: vec![],
                fee_market_price: None,
                fee_fx_rate: None,
            },
        )
        .await
        .unwrap();

        assert!(
            db_wash_sales(&pool, DEFAULT_WINDOW_DAYS)
                .await
                .unwrap()
                .is_empty()
        );
    }

    #[tokio::test]
    async fn api_post_wash_sales_defaults_and_custom_window() {
        let pool = test_pool().await;
        loss_sale_fixture(&pool, ymd(2025, 6, 10)).await;
        test_support::buy(3, 1)
            .date(ymd(2025, 6, 20))
            .qty(Decimal::from(100))
            .price(Decimal::from(10))
            .insert(&pool)
            .await;

        let post = |body: &'static str| {
            let pool = pool.clone();
            async move {
                router()
                    .with_state(pool)
                    .oneshot(
                        Request::builder()
                            .method("POST")
                            .uri("/reports/wash_sales")
                            .header("content-type", "application/json")
                            .body(Body::from(body))
                            .unwrap(),
                    )
                    .await
                    .unwrap()
            }
        };

        // Empty body → default 30-day window: the 10-day repurchase flags.
        let resp = post("{}").await;
        assert_eq!(resp.status(), StatusCode::OK);
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let alerts: Vec<WashSaleAlert> = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(alerts.len(), 1);
        assert_eq!(alerts[0].days_apart, 10);

        // An explicit 5-day window excludes it.
        let resp = post(r#"{"window_days": 5}"#).await;
        assert_eq!(resp.status(), StatusCode::OK);
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let alerts: Vec<WashSaleAlert> = serde_json::from_slice(&bytes).unwrap();
        assert!(alerts.is_empty());

        // A non-positive window is rejected, not silently defaulted.
        let resp = post(r#"{"window_days": 0}"#).await;
        assert_eq!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY);
    }
}
