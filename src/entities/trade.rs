//! The trade entity, split into focused units: the model and wire types
//! (`model`), the write-time invariant checks shared with the Sell path
//! (`checks`), settlement-date derivation (`settlement`), persistence and
//! its in-transaction invariants (`db`), and the HTTP handlers/router
//! (`http`). Everything is re-exported here, so the module's surface is
//! unchanged from when it was one file — and the tests below, which predate
//! the split, are the behaviour lock proving it.

mod checks;
mod db;
mod http;
mod model;
mod settlement;

/// Referenced by name only from tests (its variants reach non-test callers
/// through `db_upsert`'s signature), so the re-export is test-gated to keep
/// the non-test build warning-free.
#[cfg(test)]
pub use db::UpsertError;
pub use db::{DeleteOutcome, db_delete, db_get, db_list, db_upsert};
pub use http::router;
pub use model::{Trade, TradeBody, TradeType};

pub(crate) use checks::{
    AmountsCheck, AmountsError, CGT_START, SpotFxRateError, StatementTotalCheck,
    StatementTotalError, TradeAmounts, amounts_detail, check_amounts, check_statement_total,
    resolve_brokerage, spot_fx_rate_detail, statement_total_detail, validate_spot_fx_rate,
};
pub(crate) use settlement::auto_settlement_date;

#[cfg(test)]
use axum::http::StatusCode;
#[cfg(test)]
use checks::split_gst_inclusive;
#[cfg(test)]
use chrono::NaiveDate;
#[cfg(test)]
use settlement::add_business_days;
#[cfg(test)]
use sqlx::SqlitePool;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{self, ApiClient, dec, test_pool, ymd};
    use rust_decimal::Decimal;
    use std::collections::HashSet;

    /// Client over this module's own routes.
    fn client(pool: &SqlitePool) -> ApiClient {
        ApiClient::over(router().with_state(pool.clone()))
    }

    async fn insert_test_listing(pool: &SqlitePool) {
        test_support::listing(1)
            .ticker("VAS")
            .name("Vanguard Australian Shares ETF")
            .insert(pool)
            .await;
    }

    fn buy_trade() -> Trade {
        test_support::buy(1, 1)
            .date(ymd(2024, 1, 15))
            .qty(Decimal::from(10))
            .price(Decimal::from(100))
            .brokerage(dec("9.95"))
            .gst_on_brokerage(dec("0.995"))
            .with(|t| t.contract_note_ref = Some("CN001".to_string()))
            .build()
    }

    /// Sell `qty` units out of the Buy parcel `buy_id` (listing 1), via the
    /// atomic Sell + allocation path.
    async fn insert_sell_consuming(pool: &SqlitePool, sell_id: i64, buy_id: i64, qty: Decimal) {
        use crate::entities::sell;
        sell::db_upsert_sell(
            pool,
            sell_id,
            &sell::SellBody {
                brokerage_includes_gst: false,
                statement_total: None,
                holding_account_id: 1,
                date: NaiveDate::from_ymd_opt(2024, 6, 1).unwrap(),
                settlement_date: Some(NaiveDate::from_ymd_opt(2024, 6, 3).unwrap()),
                listing_id: 1,
                average_price: Decimal::from(120),
                quantity: qty,
                currency: "AUD".to_string(),
                brokerage: Decimal::ZERO,
                gst_on_brokerage: Decimal::ZERO,
                brokerage_currency: "AUD".to_string(),
                fx_rate: Decimal::ONE,
                spot_fx_rate: None,
                contract_note_ref: None,
                allocations: vec![sell::AllocationInput {
                    purchase_trade_id: buy_id,
                    quantity_allocated: qty,
                }],
            },
        )
        .await
        .unwrap();
    }

    /// Link an AMIT adjustment covering `qty` units of trade `trade_id`
    /// (listing 1), creating the AMMA statement it hangs off.
    async fn insert_amit_adjustment_covering(pool: &SqlitePool, trade_id: i64, qty: Decimal) {
        test_support::amma(1, 1)
            .units(qty)
            .cost_base_adjustment(dec("0.05"))
            .insert(pool)
            .await;
        test_support::amit_adjustment(pool, 1, 1, trade_id, qty).await;
    }

    // DB-level tests

    #[tokio::test]
    async fn db_buy_trade_insert_and_retrieve() {
        let pool = test_pool().await;
        insert_test_listing(&pool).await;
        db_upsert(&pool, &buy_trade()).await.unwrap();
        let got = db_get(&pool, 1).await.unwrap().unwrap();
        assert_eq!(got.trade_type, TradeType::Buy);
        assert_eq!(got.quantity, Decimal::from(10));
        assert_eq!(got.average_price, Decimal::from(100));
        assert_eq!(
            got.settlement_date,
            NaiveDate::from_ymd_opt(2024, 1, 17).unwrap()
        );
        assert_eq!(got.contract_note_ref, Some("CN001".to_string()));
    }

    #[tokio::test]
    async fn db_unknown_currency_rejected_on_both_currency_columns() {
        let pool = test_pool().await;
        insert_test_listing(&pool).await;

        fn assert_fk_error(err: UpsertError, column: &str) {
            match err {
                UpsertError::Db(e) => assert!(
                    e.to_string().contains("FOREIGN KEY"),
                    "expected {column} FK error, got: {e}"
                ),
                other => panic!("expected {column} FK error, got: {other:?}"),
            }
        }

        // 'ZZZ' is not a recognised currency → each currency column's FK rejects it.
        let mut bad_currency = buy_trade();
        bad_currency.currency = "ZZZ".to_string();
        assert_fk_error(
            db_upsert(&pool, &bad_currency).await.unwrap_err(),
            "currency",
        );

        let mut bad_brokerage = buy_trade();
        bad_brokerage.brokerage_currency = "ZZZ".to_string();
        assert_fk_error(
            db_upsert(&pool, &bad_brokerage).await.unwrap_err(),
            "brokerage_currency",
        );

        // A seeded digital-token code (BTC) is a recognised currency and is accepted.
        let mut btc = buy_trade();
        btc.currency = "BTC".to_string();
        db_upsert(&pool, &btc).await.unwrap();
    }

    #[tokio::test]
    async fn db_sell_trade_insert_and_retrieve() {
        let pool = test_pool().await;
        insert_test_listing(&pool).await;
        let trade = test_support::sell(2, 1)
            .date(ymd(2024, 6, 1))
            .qty(Decimal::from(5))
            .price(Decimal::from(120))
            .brokerage(dec("9.95"))
            .gst_on_brokerage(dec("0.995"))
            .build();
        db_upsert(&pool, &trade).await.unwrap();
        let got = db_get(&pool, 2).await.unwrap().unwrap();
        assert_eq!(got.trade_type, TradeType::Sell);
        assert_eq!(got.quantity, Decimal::from(5));
    }

    #[tokio::test]
    async fn db_drp_trade_insert_and_retrieve() {
        let pool = test_pool().await;
        insert_test_listing(&pool).await;
        let trade = test_support::drp(3, 1)
            .date(ymd(2024, 3, 15))
            .settlement(ymd(2024, 3, 15))
            .qty(Decimal::from(2))
            .price(Decimal::from(95))
            .build();
        db_upsert(&pool, &trade).await.unwrap();
        let got = db_get(&pool, 3).await.unwrap().unwrap();
        assert_eq!(got.trade_type, TradeType::DRP);
        assert_eq!(got.quantity, Decimal::from(2));
    }

    #[tokio::test]
    async fn db_drp_residual_fields_round_trip_with_precision() {
        let pool = test_pool().await;
        insert_test_listing(&pool).await;
        let mut trade = buy_trade();
        trade.id = 7;
        trade.trade_type = TradeType::DRP;
        trade.residual_brought_forward = "1.234567890".parse().unwrap();
        trade.residual_carried_forward = "0.987654321".parse().unwrap();
        trade.residual_paid_out = "2.500000001".parse().unwrap();
        db_upsert(&pool, &trade).await.unwrap();
        let got = db_get(&pool, 7).await.unwrap().unwrap();
        assert_eq!(
            got.residual_brought_forward,
            "1.234567890".parse::<Decimal>().unwrap()
        );
        assert_eq!(
            got.residual_carried_forward,
            "0.987654321".parse::<Decimal>().unwrap()
        );
        assert_eq!(
            got.residual_paid_out,
            "2.500000001".parse::<Decimal>().unwrap()
        );
    }

    #[tokio::test]
    async fn db_non_drp_trade_defaults_residuals_to_zero() {
        // A plain Buy carries zero residuals (residuals are a DRP-only concept).
        let pool = test_pool().await;
        insert_test_listing(&pool).await;
        db_upsert(&pool, &buy_trade()).await.unwrap();
        let got = db_get(&pool, 1).await.unwrap().unwrap();
        assert_eq!(got.residual_brought_forward, Decimal::ZERO);
        assert_eq!(got.residual_carried_forward, Decimal::ZERO);
        assert_eq!(got.residual_paid_out, Decimal::ZERO);
    }

    #[tokio::test]
    async fn db_get_missing_returns_none() {
        let pool = test_pool().await;
        assert!(db_get(&pool, 999).await.unwrap().is_none());
    }

    // Buy-trade edit/delete integrity (symmetric with the Sell-side invariants)

    #[tokio::test]
    async fn db_delete_buy_consumed_by_allocation_is_refused() {
        let pool = test_pool().await;
        insert_test_listing(&pool).await;
        db_upsert(&pool, &buy_trade()).await.unwrap();
        insert_sell_consuming(&pool, 2, 1, Decimal::from(5)).await;

        assert_eq!(
            db_delete(&pool, 1).await.unwrap(),
            DeleteOutcome::Referenced
        );
        assert!(
            db_get(&pool, 1).await.unwrap().is_some(),
            "consumed buy must remain"
        );
    }

    #[tokio::test]
    async fn db_delete_buy_covered_by_amit_adjustment_is_refused() {
        let pool = test_pool().await;
        insert_test_listing(&pool).await;
        db_upsert(&pool, &buy_trade()).await.unwrap();
        insert_amit_adjustment_covering(&pool, 1, Decimal::from(10)).await;

        assert_eq!(
            db_delete(&pool, 1).await.unwrap(),
            DeleteOutcome::Referenced
        );
        assert!(
            db_get(&pool, 1).await.unwrap().is_some(),
            "covered buy must remain"
        );
    }

    #[tokio::test]
    async fn db_delete_drp_linked_to_income_reinvestment_is_refused() {
        // A DRP trade recorded as a distribution's reinvestment is referenced by
        // income.reinvestment_trade_id — deleting it would orphan that link.
        let pool = test_pool().await;
        insert_test_listing(&pool).await;
        let mut drp = buy_trade();
        drp.trade_type = TradeType::DRP;
        db_upsert(&pool, &drp).await.unwrap();
        sqlx::query(
            "INSERT INTO income (id, listing_id, date_paid, reinvestment_trade_id) \
             VALUES (1, 1, '2024-03-15', 1)",
        )
        .execute(&pool)
        .await
        .unwrap();

        assert_eq!(
            db_delete(&pool, 1).await.unwrap(),
            DeleteOutcome::Referenced
        );
        assert!(
            db_get(&pool, 1).await.unwrap().is_some(),
            "reinvestment trade must remain"
        );
    }

    #[tokio::test]
    async fn db_upsert_over_reinvestment_trade_is_refused() {
        // A Buy body targeting a reinvest-created DRP would re-type it and
        // zero its residual chain while the income row keeps pointing at it —
        // the link lives on income.reinvestment_trade_id, which the
        // provenance-column check can't see, so it needs its own guard.
        let pool = test_pool().await;
        insert_test_listing(&pool).await;
        let mut drp = buy_trade();
        drp.trade_type = TradeType::DRP;
        drp.residual_carried_forward = dec("1.23");
        db_upsert(&pool, &drp).await.unwrap();
        sqlx::query(
            "INSERT INTO income (id, listing_id, date_paid, reinvestment_trade_id) \
             VALUES (1, 1, '2024-03-15', 1)",
        )
        .execute(&pool)
        .await
        .unwrap();

        let err = db_upsert(&pool, &buy_trade()).await.unwrap_err();
        assert!(
            matches!(err, UpsertError::ReinvestmentTrade),
            "expected ReinvestmentTrade, got: {err:?}"
        );
        let kept = db_get(&pool, 1).await.unwrap().unwrap();
        assert_eq!(kept.trade_type, TradeType::DRP, "trade must stay a DRP");
        assert_eq!(
            kept.residual_carried_forward,
            dec("1.23"),
            "residual chain must be untouched"
        );
    }

    #[tokio::test]
    async fn api_put_buy_body_over_reinvestment_drp_returns_422() {
        let pool = test_pool().await;
        insert_test_listing(&pool).await;
        let mut drp = buy_trade();
        drp.trade_type = TradeType::DRP;
        db_upsert(&pool, &drp).await.unwrap();
        sqlx::query(
            "INSERT INTO income (id, listing_id, date_paid, reinvestment_trade_id) \
             VALUES (1, 1, '2024-03-15', 1)",
        )
        .execute(&pool)
        .await
        .unwrap();

        let body = serde_json::json!({
            "trade_type": "Buy",
            "date": "2024-01-15",
            "listing_id": 1,
            "average_price": 100.0,
            "quantity": 10.0,
            "currency": "AUD",
            "brokerage": 9.95,
            "gst_on_brokerage": 0.995,
            "brokerage_currency": "AUD",
            "fx_rate": 1.0
        });
        let resp = client(&pool).put("/trades/1", &body).await;
        assert_eq!(resp.status, StatusCode::UNPROCESSABLE_ENTITY);
        let trade_type: String = sqlx::query_scalar("SELECT trade_type FROM trades WHERE id = 1")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(trade_type, "DRP", "the reinvestment DRP must be untouched");
    }

    #[tokio::test]
    async fn db_shrink_buy_below_allocated_quantity_is_refused() {
        let pool = test_pool().await;
        insert_test_listing(&pool).await;
        db_upsert(&pool, &buy_trade()).await.unwrap(); // qty 10
        insert_sell_consuming(&pool, 2, 1, Decimal::from(5)).await;

        // Shrinking below the 5 already allocated out is refused…
        let mut shrunk = buy_trade();
        shrunk.quantity = Decimal::from(4);
        let err = db_upsert(&pool, &shrunk).await.unwrap_err();
        assert!(
            matches!(err, UpsertError::QuantityBelowAllocated),
            "expected QuantityBelowAllocated, got: {err:?}"
        );
        assert_eq!(
            db_get(&pool, 1).await.unwrap().unwrap().quantity,
            Decimal::from(10)
        );

        // …but shrinking exactly to the allocated quantity is fine.
        let mut exact = buy_trade();
        exact.quantity = Decimal::from(5);
        db_upsert(&pool, &exact).await.unwrap();
        assert_eq!(
            db_get(&pool, 1).await.unwrap().unwrap().quantity,
            Decimal::from(5)
        );
    }

    /// With a 2-for-1 split (TD 2000/10) between the buy and the sale, the
    /// sale's allocation is in post-split units: a 10-unit parcel that had 10
    /// post-split units (= 5 as-acquired) sold out of it can still shrink to
    /// 5, but not 4.
    #[tokio::test]
    async fn db_shrink_check_rebases_post_split_allocations() {
        let pool = test_pool().await;
        insert_test_listing(&pool).await;
        db_upsert(&pool, &buy_trade()).await.unwrap(); // qty 10, 2024-01-15
        crate::entities::corporate_action::db_upsert(
            &pool,
            &crate::entities::corporate_action::CorporateAction {
                id: 1,
                listing_id: 1,
                date: NaiveDate::from_ymd_opt(2024, 3, 1).unwrap(),
                kind: crate::entities::corporate_action::ActionKind::ShareSplit {
                    split_new_units: Decimal::from(2),
                    split_old_units: Decimal::ONE,
                },
            },
        )
        .await
        .unwrap();
        // Sell 10 post-split units (= 5 as-acquired) on 2024-06-01.
        insert_sell_consuming(&pool, 2, 1, Decimal::from(10)).await;

        // 4 < the 5 as-acquired units allocated out → refused…
        let mut shrunk = buy_trade();
        shrunk.quantity = Decimal::from(4);
        let err = db_upsert(&pool, &shrunk).await.unwrap_err();
        assert!(
            matches!(err, UpsertError::QuantityBelowAllocated),
            "expected QuantityBelowAllocated, got: {err:?}"
        );

        // …but exactly the 5 as-acquired units is fine.
        let mut exact = buy_trade();
        exact.quantity = Decimal::from(5);
        db_upsert(&pool, &exact).await.unwrap();
        assert_eq!(
            db_get(&pool, 1).await.unwrap().unwrap().quantity,
            Decimal::from(5)
        );
    }

    #[tokio::test]
    async fn db_shrink_buy_below_amit_adjustment_quantity_is_refused() {
        let pool = test_pool().await;
        insert_test_listing(&pool).await;
        db_upsert(&pool, &buy_trade()).await.unwrap(); // qty 10
        insert_amit_adjustment_covering(&pool, 1, Decimal::from(8)).await;

        // Shrinking below the adjustment's 8 covered units is refused…
        let mut shrunk = buy_trade();
        shrunk.quantity = Decimal::from(7);
        let err = db_upsert(&pool, &shrunk).await.unwrap_err();
        assert!(
            matches!(err, UpsertError::QuantityBelowAmitAdjustment),
            "expected QuantityBelowAmitAdjustment, got: {err:?}"
        );
        assert_eq!(
            db_get(&pool, 1).await.unwrap().unwrap().quantity,
            Decimal::from(10)
        );

        // …but shrinking exactly to the covered quantity is fine.
        let mut exact = buy_trade();
        exact.quantity = Decimal::from(8);
        db_upsert(&pool, &exact).await.unwrap();
        assert_eq!(
            db_get(&pool, 1).await.unwrap().unwrap().quantity,
            Decimal::from(8)
        );
    }

    /// A Buy's listing is frozen while Sell allocations draw on the parcel:
    /// changing it would silently re-associate those allocations (and their
    /// CGT costing) to the new listing.
    #[tokio::test]
    async fn db_listing_change_on_allocated_parcel_is_refused() {
        let pool = test_pool().await;
        insert_test_listing(&pool).await;
        test_support::listing(2).ticker("VGS").insert(&pool).await;
        db_upsert(&pool, &buy_trade()).await.unwrap(); // listing 1
        insert_sell_consuming(&pool, 2, 1, Decimal::from(5)).await;

        let mut moved = buy_trade();
        moved.listing_id = 2;
        let err = db_upsert(&pool, &moved).await.unwrap_err();
        assert!(
            matches!(err, UpsertError::ListingChangeReferenced),
            "expected ListingChangeReferenced, got: {err:?}"
        );
        assert_eq!(db_get(&pool, 1).await.unwrap().unwrap().listing_id, 1);
    }

    /// The same freeze applies while an AMIT adjustment covers the parcel —
    /// and lifts once nothing references it.
    #[tokio::test]
    async fn db_listing_change_under_amit_adjustment_is_refused_until_unlinked() {
        let pool = test_pool().await;
        insert_test_listing(&pool).await;
        test_support::listing(2).ticker("VGS").insert(&pool).await;
        db_upsert(&pool, &buy_trade()).await.unwrap(); // listing 1
        insert_amit_adjustment_covering(&pool, 1, Decimal::from(8)).await;

        let mut moved = buy_trade();
        moved.listing_id = 2;
        let err = db_upsert(&pool, &moved).await.unwrap_err();
        assert!(
            matches!(err, UpsertError::ListingChangeReferenced),
            "expected ListingChangeReferenced, got: {err:?}"
        );
        assert_eq!(db_get(&pool, 1).await.unwrap().unwrap().listing_id, 1);

        // With the adjustment removed the listing edits freely again.
        sqlx::query("DELETE FROM amit_adjustments WHERE trade_id = 1")
            .execute(&pool)
            .await
            .unwrap();
        db_upsert(&pool, &moved).await.unwrap();
        assert_eq!(db_get(&pool, 1).await.unwrap().unwrap().listing_id, 2);
    }

    #[tokio::test]
    async fn db_unconsumed_buy_still_edits_and_deletes_freely() {
        let pool = test_pool().await;
        insert_test_listing(&pool).await;
        db_upsert(&pool, &buy_trade()).await.unwrap(); // qty 10

        let mut shrunk = buy_trade();
        shrunk.quantity = Decimal::ONE;
        db_upsert(&pool, &shrunk).await.unwrap();
        assert_eq!(
            db_get(&pool, 1).await.unwrap().unwrap().quantity,
            Decimal::ONE
        );

        assert_eq!(db_delete(&pool, 1).await.unwrap(), DeleteOutcome::Deleted);
        assert!(db_get(&pool, 1).await.unwrap().is_none());
    }

    // API-level tests

    #[tokio::test]
    async fn api_settlement_date_auto_populated() {
        let pool = test_pool().await;
        insert_test_listing(&pool).await;
        // XASX has settlement_days = 2, so 2024-01-15 + 2 = 2024-01-17
        let body = serde_json::json!({
            "trade_type": "Buy",
            "date": "2024-01-15",
            "listing_id": 1,
            "average_price": 100.0,
            "quantity": 10.0,
            "currency": "AUD",
            "brokerage": 9.95,
            "gst_on_brokerage": 0.995,
            "brokerage_currency": "AUD",
            "fx_rate": 1.0
        });
        let resp = client(&pool).put("/trades/1", &body).await;
        assert_eq!(resp.status, StatusCode::NO_CONTENT);
        let trade = db_get(&pool, 1).await.unwrap().unwrap();
        assert_eq!(
            trade.settlement_date,
            NaiveDate::from_ymd_opt(2024, 1, 17).unwrap()
        );
    }

    /// An exchange-less (Crypto) listing settles same-day: the auto-populated
    /// settlement date is the trade date itself — a Friday stays a Friday (no
    /// T+n, no business-day skipping) — and no coverage warning fires (there
    /// is no holiday calendar to be outside of).
    #[tracing_test::traced_test]
    #[tokio::test]
    async fn api_settlement_date_same_day_for_crypto() {
        let pool = test_pool().await;
        test_support::listing(1)
            .crypto()
            .ticker("BTC")
            .name("Bitcoin")
            .insert(&pool)
            .await;
        // 2030-06-07 is a Friday, far outside every seeded holiday calendar.
        let body = serde_json::json!({
            "trade_type": "Buy",
            "date": "2030-06-07",
            "listing_id": 1,
            "average_price": "65000",
            "quantity": "0.12345678",
            "currency": "AUD",
            "brokerage": "0",
            "gst_on_brokerage": "0",
            "brokerage_currency": "AUD",
            "fx_rate": "1"
        });
        let resp = client(&pool).put("/trades/1", &body).await;
        assert_eq!(resp.status, StatusCode::NO_CONTENT);
        let trade = db_get(&pool, 1).await.unwrap().unwrap();
        assert_eq!(
            trade.settlement_date,
            NaiveDate::from_ymd_opt(2030, 6, 7).unwrap()
        );
        assert!(!logs_contain(
            "settlement window outside seeded exchange-holiday coverage"
        ));
    }

    #[tracing_test::traced_test]
    #[tokio::test]
    async fn api_settlement_beyond_holiday_coverage_logs_warning() {
        let pool = test_pool().await;
        insert_test_listing(&pool).await;
        // XASX holidays are seeded 2019–2027 only: a 2030 trade's settlement is
        // computed skipping weekends only, so the auto-population warns rather
        // than silently using the incomplete calendar.
        let body = serde_json::json!({
            "trade_type": "Buy",
            "date": "2030-06-03",
            "listing_id": 1,
            "average_price": 100.0,
            "quantity": 10.0,
            "currency": "AUD",
            "brokerage": 0.0,
            "gst_on_brokerage": 0.0,
            "brokerage_currency": "AUD",
            "fx_rate": 1.0
        });
        let resp = client(&pool).put("/trades/1", &body).await;
        // Non-blocking: the write succeeds, the warning surfaces the gap.
        assert_eq!(resp.status, StatusCode::NO_CONTENT);
        assert!(logs_contain(
            "settlement window outside seeded exchange-holiday coverage"
        ));
    }

    #[tracing_test::traced_test]
    #[tokio::test]
    async fn api_settlement_inside_holiday_coverage_does_not_warn() {
        let pool = test_pool().await;
        insert_test_listing(&pool).await;
        let body = serde_json::json!({
            "trade_type": "Buy",
            "date": "2024-01-15",
            "listing_id": 1,
            "average_price": 100.0,
            "quantity": 10.0,
            "currency": "AUD",
            "brokerage": 0.0,
            "gst_on_brokerage": 0.0,
            "brokerage_currency": "AUD",
            "fx_rate": 1.0
        });
        let resp = client(&pool).put("/trades/1", &body).await;
        assert_eq!(resp.status, StatusCode::NO_CONTENT);
        assert!(!logs_contain(
            "settlement window outside seeded exchange-holiday coverage"
        ));
    }

    /// A parcel is created by a Buy or a DRP reinvestment and consumed by a
    /// Sell — the classification every "needs something to draw on" write-time
    /// guard tests (Sell and rights-sale allocations, AMIT adjustments).
    #[test]
    fn only_buy_and_drp_are_acquisitions() {
        assert!(TradeType::Buy.is_acquisition());
        assert!(TradeType::DRP.is_acquisition());
        assert!(!TradeType::Sell.is_acquisition());
    }

    #[test]
    fn add_business_days_skips_weekend() {
        let none = HashSet::new();
        // 2024-01-18 is a Thursday; T+2 business days settles Monday 2024-01-22,
        // skipping Sat 2024-01-20 and Sun 2024-01-21.
        let thursday = NaiveDate::from_ymd_opt(2024, 1, 18).unwrap();
        assert_eq!(
            add_business_days(thursday, 2, &none),
            NaiveDate::from_ymd_opt(2024, 1, 22).unwrap()
        );
        // 2024-01-15 is a Monday; T+2 stays within the week (Wednesday).
        let monday = NaiveDate::from_ymd_opt(2024, 1, 15).unwrap();
        assert_eq!(
            add_business_days(monday, 2, &none),
            NaiveDate::from_ymd_opt(2024, 1, 17).unwrap()
        );
    }

    #[test]
    fn add_business_days_skips_public_holidays() {
        // Christmas Day (Wed) and Boxing Day (Thu) 2024 are public holidays.
        let holidays: HashSet<NaiveDate> = [
            NaiveDate::from_ymd_opt(2024, 12, 25).unwrap(),
            NaiveDate::from_ymd_opt(2024, 12, 26).unwrap(),
        ]
        .into_iter()
        .collect();
        // Tuesday 2024-12-24 + T+2: skip Wed 25 + Thu 26 (holidays), Fri 27 = 1,
        // skip the weekend, Mon 30 = 2 → settles 2024-12-30.
        let tuesday = NaiveDate::from_ymd_opt(2024, 12, 24).unwrap();
        assert_eq!(
            add_business_days(tuesday, 2, &holidays),
            NaiveDate::from_ymd_opt(2024, 12, 30).unwrap()
        );
        // Without the holiday set it would settle on Boxing Day (Thu 26).
        assert_eq!(
            add_business_days(tuesday, 2, &HashSet::new()),
            NaiveDate::from_ymd_opt(2024, 12, 26).unwrap()
        );
    }

    #[tokio::test]
    async fn api_settlement_date_skips_public_holiday() {
        let pool = test_pool().await;
        insert_test_listing(&pool).await; // listing 1 trades on XASX
        // XASX is closed Christmas (Wed 2024-12-25) and Boxing Day (Thu 2024-12-26);
        // a Tuesday 2024-12-24 buy at T+2 settles Mon 2024-12-30, not Thu 2024-12-26.
        let body = serde_json::json!({
            "trade_type": "Buy",
            "date": "2024-12-24",
            "listing_id": 1,
            "average_price": 100.0,
            "quantity": 10.0,
            "currency": "AUD",
            "brokerage": 9.95,
            "gst_on_brokerage": 0.995,
            "brokerage_currency": "AUD",
            "fx_rate": 1.0
        });
        let resp = client(&pool).put("/trades/1", &body).await;
        assert_eq!(resp.status, StatusCode::NO_CONTENT);
        let trade = db_get(&pool, 1).await.unwrap().unwrap();
        assert_eq!(
            trade.settlement_date,
            NaiveDate::from_ymd_opt(2024, 12, 30).unwrap()
        );
    }

    #[tokio::test]
    async fn api_settlement_date_auto_populated_skips_weekend() {
        let pool = test_pool().await;
        insert_test_listing(&pool).await;
        // Friday 2024-01-19 + T+2 business days = Tuesday 2024-01-23 (skips the weekend).
        let body = serde_json::json!({
            "trade_type": "Buy",
            "date": "2024-01-19",
            "listing_id": 1,
            "average_price": 100.0,
            "quantity": 10.0,
            "currency": "AUD",
            "brokerage": 9.95,
            "gst_on_brokerage": 0.995,
            "brokerage_currency": "AUD",
            "fx_rate": 1.0
        });
        let resp = client(&pool).put("/trades/1", &body).await;
        assert_eq!(resp.status, StatusCode::NO_CONTENT);
        let trade = db_get(&pool, 1).await.unwrap().unwrap();
        assert_eq!(
            trade.settlement_date,
            NaiveDate::from_ymd_opt(2024, 1, 23).unwrap()
        );
    }

    #[tokio::test]
    async fn api_put_sell_trade_is_rejected() {
        // Sells must go through PUT /sells/{id}; the generic trade endpoint rejects them.
        let pool = test_pool().await;
        insert_test_listing(&pool).await;
        let body = serde_json::json!({
            "trade_type": "Sell",
            "date": "2024-01-15",
            "listing_id": 1,
            "average_price": 100.0,
            "quantity": 10.0,
            "currency": "AUD",
            "brokerage": 9.95,
            "gst_on_brokerage": 0.995,
            "brokerage_currency": "AUD",
            "fx_rate": 1.0
        });
        let resp = client(&pool).put("/trades/1", &body).await;
        assert_eq!(resp.status, StatusCode::UNPROCESSABLE_ENTITY);
    }

    #[tokio::test]
    async fn api_put_drp_trade_is_rejected() {
        // DRP trades are created only via POST /income/{id}/reinvest (which links
        // them to their distribution and threads the residual chain); the generic
        // trade endpoint rejects a free-form DRP, and nothing is persisted.
        let pool = test_pool().await;
        insert_test_listing(&pool).await;
        let body = serde_json::json!({
            "trade_type": "DRP",
            "date": "2024-03-15",
            "listing_id": 1,
            "average_price": 95.0,
            "quantity": 2.0,
            "currency": "AUD",
            "brokerage": 0.0,
            "gst_on_brokerage": 0.0,
            "brokerage_currency": "AUD",
            "fx_rate": 1.0
        });
        let resp = client(&pool).put("/trades/1", &body).await;
        assert_eq!(resp.status, StatusCode::UNPROCESSABLE_ENTITY);
        let n: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM trades")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(n, 0, "rejected DRP must not be persisted");
    }

    #[tokio::test]
    async fn api_settlement_date_override() {
        let pool = test_pool().await;
        insert_test_listing(&pool).await;
        let body = serde_json::json!({
            "trade_type": "Buy",
            "date": "2024-01-15",
            "settlement_date": "2024-01-20",
            "listing_id": 1,
            "average_price": 100.0,
            "quantity": 10.0,
            "currency": "AUD",
            "brokerage": 9.95,
            "gst_on_brokerage": 0.995,
            "brokerage_currency": "AUD",
            "fx_rate": 1.0
        });
        let resp = client(&pool).put("/trades/1", &body).await;
        assert_eq!(resp.status, StatusCode::NO_CONTENT);
        let trade = db_get(&pool, 1).await.unwrap().unwrap();
        assert_eq!(
            trade.settlement_date,
            NaiveDate::from_ymd_opt(2024, 1, 20).unwrap()
        );
    }

    #[tokio::test]
    async fn api_list_returns_ok() {
        let pool = test_pool().await;
        insert_test_listing(&pool).await;
        db_upsert(&pool, &buy_trade()).await.unwrap();
        let resp = client(&pool).get("/trades").await;
        assert_eq!(resp.status, StatusCode::OK);
        let trades: Vec<Trade> = resp.json();
        assert_eq!(trades.len(), 1);
        assert_eq!(trades[0].trade_type, TradeType::Buy);
    }

    #[tokio::test]
    async fn api_get_existing_returns_trade() {
        let pool = test_pool().await;
        insert_test_listing(&pool).await;
        db_upsert(&pool, &buy_trade()).await.unwrap();
        let resp = client(&pool).get("/trades/1").await;
        assert_eq!(resp.status, StatusCode::OK);
        let t: Trade = resp.json();
        assert_eq!(t.trade_type, TradeType::Buy);
        assert_eq!(t.quantity, Decimal::from(10));
    }

    #[tokio::test]
    async fn api_get_missing_returns_404() {
        let pool = test_pool().await;
        let resp = client(&pool).get("/trades/999").await;
        assert_eq!(resp.status, StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn api_delete_existing_returns_no_content() {
        let pool = test_pool().await;
        insert_test_listing(&pool).await;
        db_upsert(&pool, &buy_trade()).await.unwrap();
        let resp = client(&pool).delete("/trades/1").await;
        assert_eq!(resp.status, StatusCode::NO_CONTENT);
    }

    #[tokio::test]
    async fn api_delete_missing_returns_404() {
        let pool = test_pool().await;
        let resp = client(&pool).delete("/trades/999").await;
        assert_eq!(resp.status, StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn api_delete_consumed_buy_returns_422() {
        let pool = test_pool().await;
        insert_test_listing(&pool).await;
        db_upsert(&pool, &buy_trade()).await.unwrap();
        insert_sell_consuming(&pool, 2, 1, Decimal::from(5)).await;

        let resp = client(&pool).delete("/trades/1").await;
        assert_eq!(resp.status, StatusCode::UNPROCESSABLE_ENTITY);
        assert!(
            db_get(&pool, 1).await.unwrap().is_some(),
            "consumed buy must remain"
        );
    }

    #[tokio::test]
    async fn api_shrink_partly_sold_buy_returns_422() {
        let pool = test_pool().await;
        insert_test_listing(&pool).await;
        db_upsert(&pool, &buy_trade()).await.unwrap(); // qty 10
        insert_sell_consuming(&pool, 2, 1, Decimal::from(5)).await;

        // Editing the Buy down to 4 would leave the Sell's 5-unit allocation
        // drawing on units the parcel no longer has.
        let body = serde_json::json!({
            "trade_type": "Buy",
            "date": "2024-01-15",
            "settlement_date": "2024-01-17",
            "listing_id": 1,
            "average_price": "100",
            "quantity": "4",
            "currency": "AUD",
            "brokerage": "9.95",
            "gst_on_brokerage": "0.995",
            "brokerage_currency": "AUD",
            "fx_rate": "1"
        });
        let resp = client(&pool).put("/trades/1", &body).await;
        assert_eq!(resp.status, StatusCode::UNPROCESSABLE_ENTITY);
        assert_eq!(
            db_get(&pool, 1).await.unwrap().unwrap().quantity,
            Decimal::from(10)
        );
    }

    #[tokio::test]
    async fn api_listing_change_on_consumed_parcel_returns_422_with_reason() {
        let pool = test_pool().await;
        insert_test_listing(&pool).await;
        test_support::listing(2).ticker("VGS").insert(&pool).await;
        db_upsert(&pool, &buy_trade()).await.unwrap(); // listing 1
        insert_sell_consuming(&pool, 2, 1, Decimal::from(5)).await;

        let body = serde_json::json!({
            "trade_type": "Buy",
            "date": "2024-01-15",
            "settlement_date": "2024-01-17",
            "listing_id": 2,
            "average_price": "100",
            "quantity": "10",
            "currency": "AUD",
            "brokerage": "9.95",
            "gst_on_brokerage": "0.995",
            "brokerage_currency": "AUD",
            "fx_rate": "1"
        });
        let (status, detail) = put_trade_json(&pool, 1, body).await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
        assert!(
            detail.contains("listing cannot be changed"),
            "detail: {detail}"
        );
        assert_eq!(db_get(&pool, 1).await.unwrap().unwrap().listing_id, 1);
    }

    #[tokio::test]
    async fn api_decimal_precision_round_trip() {
        let pool = test_pool().await;
        insert_test_listing(&pool).await;
        let body = serde_json::json!({
            "trade_type": "Buy",
            "date": "2024-01-15",
            "listing_id": 1,
            "average_price": "99.9999999999",
            "quantity": "10.5",
            "currency": "AUD",
            "brokerage": "9.95",
            "gst_on_brokerage": "0.995",
            "brokerage_currency": "AUD",
            "fx_rate": "1.0"
        });
        let resp = client(&pool).put("/trades/1", &body).await;
        assert_eq!(resp.status, StatusCode::NO_CONTENT);
        let resp = client(&pool).get("/trades/1").await;
        let t: Trade = resp.json();
        assert_eq!(t.average_price, "99.9999999999".parse::<Decimal>().unwrap());
        assert_eq!(t.quantity, "10.5".parse::<Decimal>().unwrap());
        assert_eq!(t.brokerage, "9.95".parse::<Decimal>().unwrap());
    }

    fn d(s: &str) -> Decimal {
        s.parse().unwrap()
    }

    /// PUT the JSON body to /trades/{id}, returning the status and response
    /// body text (the statement-total 422 carries its detail there).
    async fn put_trade_json(
        pool: &SqlitePool,
        id: i64,
        body: serde_json::Value,
    ) -> (StatusCode, String) {
        let resp = client(pool).put(format!("/trades/{id}"), &body).await;
        let status = resp.status;
        (status, resp.text().to_string())
    }

    /// Degenerate core figures are rejected with 422 per shape — a zero or
    /// negative quantity, a negative price, negative brokerage or GST, a
    /// non-positive fx_rate, or a settlement before the trade date — and
    /// nothing is persisted. (2026-07-12 review: the plain CRUD path accepted
    /// them all, silently corrupting every downstream report.)
    #[tokio::test]
    async fn api_degenerate_trade_amounts_are_rejected_per_shape() {
        let pool = test_pool().await;
        insert_test_listing(&pool).await;
        let base = serde_json::json!({
            "trade_type": "Buy",
            "date": "2024-01-15",
            "listing_id": 1,
            "average_price": "100",
            "quantity": "10",
            "currency": "AUD",
            "brokerage": "9.95",
            "gst_on_brokerage": "0.995",
            "brokerage_currency": "AUD",
            "fx_rate": "1"
        });
        for (field, value, expected) in [
            ("quantity", "0", "quantity must be positive"),
            ("quantity", "-5", "quantity must be positive"),
            ("average_price", "-1", "average_price cannot be negative"),
            ("brokerage", "-9.95", "brokerage cannot be negative"),
            (
                "gst_on_brokerage",
                "-0.995",
                "gst_on_brokerage cannot be negative",
            ),
            ("fx_rate", "0", "fx_rate must be a positive"),
            ("fx_rate", "-1.5", "fx_rate must be a positive"),
            (
                "settlement_date",
                "2024-01-14",
                "settlement_date cannot be before the trade date",
            ),
        ] {
            let mut body = base.clone();
            body[field] = value.into();
            let (status, detail) = put_trade_json(&pool, 1, body).await;
            assert_eq!(
                status,
                StatusCode::UNPROCESSABLE_ENTITY,
                "{field}={value} must be rejected"
            );
            assert!(
                detail.contains(expected),
                "{field}={value}: detail must explain the rule, got: {detail}"
            );
            assert!(
                db_get(&pool, 1).await.unwrap().is_none(),
                "{field}={value}: nothing persisted"
            );
        }

        // Boundary values stay accepted: zero price, zero costs, a same-day
        // settlement, and a fractional quantity are all legitimate entries.
        let mut fine = base.clone();
        fine["average_price"] = "0".into();
        fine["brokerage"] = "0".into();
        fine["gst_on_brokerage"] = "0".into();
        fine["quantity"] = "0.00000001".into();
        fine["settlement_date"] = "2024-01-15".into();
        let (status, _) = put_trade_json(&pool, 1, fine).await;
        assert_eq!(status, StatusCode::NO_CONTENT);
    }

    /// A trade dated before the start of CGT (20 September 1985) is rejected
    /// 422: a pre-CGT holding is outside CGT and not modelled, so recording
    /// one would wrongly compute a capital gain or loss on it (REQUIREMENTS
    /// 2026-07-13 — the former documentation-only Known limitation, enforced).
    /// The first CGT day itself stays accepted.
    #[tokio::test]
    async fn api_pre_cgt_dated_trade_rejected_422() {
        let pool = test_pool().await;
        insert_test_listing(&pool).await;
        let base = serde_json::json!({
            "trade_type": "Buy",
            "date": "1985-09-19",
            "settlement_date": "1985-09-19",
            "listing_id": 1,
            "average_price": "100",
            "quantity": "10",
            "currency": "AUD",
            "brokerage": "0",
            "gst_on_brokerage": "0",
            "brokerage_currency": "AUD",
            "fx_rate": "1"
        });
        let (status, detail) = put_trade_json(&pool, 1, base.clone()).await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
        assert!(
            detail.contains("dated before 20 September 1985")
                && detail.contains("pre-CGT holding is outside CGT"),
            "detail must explain the pre-CGT rule, got: {detail}"
        );
        assert!(db_get(&pool, 1).await.unwrap().is_none());

        // 20 September 1985 — the first day inside CGT — is accepted.
        let mut first_cgt_day = base;
        first_cgt_day["date"] = "1985-09-20".into();
        first_cgt_day["settlement_date"] = "1985-09-20".into();
        let (status, _) = put_trade_json(&pool, 1, first_cgt_day).await;
        assert_eq!(status, StatusCode::NO_CONTENT);
    }

    #[test]
    fn split_gst_inclusive_rounds_to_the_cent_and_sums_back_exactly() {
        // $9.95 incl.: 9.95/11 = 0.9045… → $0.90 GST, $9.05 ex-GST.
        assert_eq!(split_gst_inclusive(d("9.95")), (d("9.05"), d("0.90")));
        // $10 incl.: 10/11 = 0.9090… → $0.91 GST (rounded up to the cent).
        assert_eq!(split_gst_inclusive(d("10")), (d("9.09"), d("0.91")));
        // An exact half-cent rounds away from zero: 0.055/11 = 0.005 → $0.01.
        assert_eq!(split_gst_inclusive(d("0.055")), (d("0.045"), d("0.01")));
        // The pair always sums back to the amount paid.
        for amount in ["9.95", "10", "0.055", "19.99"] {
            let (brok, gst) = split_gst_inclusive(d(amount));
            assert_eq!(brok + gst, d(amount));
        }
    }

    /// A GST-inclusive entry is split by the server (any supplied GST value is
    /// ignored), the flag round-trips, and an edit re-splits the new amount.
    /// An unflagged entry keeps today's behaviour: both values stored as sent.
    #[tokio::test]
    async fn api_gst_inclusive_brokerage_is_split_and_round_trips() {
        let pool = test_pool().await;
        insert_test_listing(&pool).await;
        let body = serde_json::json!({
            "trade_type": "Buy",
            "date": "2024-01-15",
            "listing_id": 1,
            "average_price": "100",
            "quantity": "10",
            "currency": "AUD",
            "brokerage": "9.95",
            "gst_on_brokerage": "123",   // ignored: the server derives the split
            "brokerage_includes_gst": true,
            "brokerage_currency": "AUD",
            "fx_rate": "1"
        });
        let (status, _) = put_trade_json(&pool, 1, body).await;
        assert_eq!(status, StatusCode::NO_CONTENT);
        let t = db_get(&pool, 1).await.unwrap().unwrap();
        assert_eq!(t.brokerage, d("9.05"));
        assert_eq!(t.gst_on_brokerage, d("0.90"));
        assert!(
            t.brokerage_includes_gst,
            "flag must round-trip for the entry form"
        );

        // Editing with a new inclusive amount re-splits it.
        let body = serde_json::json!({
            "trade_type": "Buy",
            "date": "2024-01-15",
            "listing_id": 1,
            "average_price": "100",
            "quantity": "10",
            "currency": "AUD",
            "brokerage": "11",
            "brokerage_includes_gst": true,
            "brokerage_currency": "AUD",
            "fx_rate": "1"
        });
        let (status, _) = put_trade_json(&pool, 1, body).await;
        assert_eq!(status, StatusCode::NO_CONTENT);
        let t = db_get(&pool, 1).await.unwrap().unwrap();
        assert_eq!(t.brokerage, d("10"));
        assert_eq!(t.gst_on_brokerage, d("1"));

        // Unflagged: stored exactly as entered (ex-GST + manual GST).
        let body = serde_json::json!({
            "trade_type": "Buy",
            "date": "2024-01-15",
            "listing_id": 1,
            "average_price": "100",
            "quantity": "10",
            "currency": "AUD",
            "brokerage": "9.95",
            "gst_on_brokerage": "0.995",
            "brokerage_currency": "AUD",
            "fx_rate": "1"
        });
        let (status, _) = put_trade_json(&pool, 2, body).await;
        assert_eq!(status, StatusCode::NO_CONTENT);
        let t = db_get(&pool, 2).await.unwrap().unwrap();
        assert_eq!(t.brokerage, d("9.95"));
        assert_eq!(t.gst_on_brokerage, d("0.995"));
        assert!(!t.brokerage_includes_gst);
        assert_eq!(t.statement_total, None);
    }

    /// GET the JSON body of /trades/{id} as raw bytes (the exact wire shape a
    /// round-trip client would re-PUT) plus its parsed `Trade`.
    async fn get_trade_raw(pool: &SqlitePool, id: i64) -> (Vec<u8>, Trade) {
        let resp = client(pool).get(format!("/trades/{id}")).await;
        assert_eq!(resp.status, StatusCode::OK);
        let trade = resp.json();
        (resp.body.to_vec(), trade)
    }

    /// Lossless GST-inclusive round-trip (REQUIREMENTS 2026-07-13): with the
    /// flag set, `brokerage` on the wire is the same GST-inclusive amount on
    /// reads and writes — GET re-presents the stored split recombined
    /// (0.90 + 0.09 reads back as the 0.99 entered), and PUTting the response
    /// body back verbatim re-splits it to the identical stored pair instead
    /// of shrinking the brokerage by the GST each pass.
    #[tokio::test]
    async fn api_gst_inclusive_get_put_round_trip_is_lossless() {
        let pool = test_pool().await;
        insert_test_listing(&pool).await;
        let body = serde_json::json!({
            "trade_type": "Buy",
            "date": "2024-01-15",
            "listing_id": 1,
            "average_price": "100",
            "quantity": "10",
            "currency": "AUD",
            "brokerage": "0.99",
            "brokerage_includes_gst": true,
            "brokerage_currency": "AUD",
            "fx_rate": "1",
            // Round-trips too, and re-validates against the re-split figures.
            "statement_total": "1000.99"
        });
        let (status, _) = put_trade_json(&pool, 1, body).await;
        assert_eq!(status, StatusCode::NO_CONTENT);

        // Two full GET → PUT-verbatim passes: the stored split never moves.
        for pass in 1..=2 {
            let (raw, seen) = get_trade_raw(&pool, 1).await;
            assert_eq!(seen.brokerage, d("0.99"), "read is inclusive (pass {pass})");
            assert_eq!(
                seen.gst_on_brokerage,
                d("0.09"),
                "derived GST (pass {pass})"
            );
            assert!(seen.brokerage_includes_gst);

            let body: serde_json::Value = serde_json::from_slice(&raw).unwrap();
            let (status, detail) = put_trade_json(&pool, 1, body).await;
            assert_eq!(status, StatusCode::NO_CONTENT, "pass {pass}: {detail}");
            let t = db_get(&pool, 1).await.unwrap().unwrap();
            assert_eq!(t.brokerage, d("0.90"), "stored ex-GST (pass {pass})");
            assert_eq!(t.gst_on_brokerage, d("0.09"), "stored GST (pass {pass})");
            assert_eq!(t.statement_total, Some(d("1000.99")));
        }

        // An unflagged trade reads back exactly as stored — no recombination.
        let body = serde_json::json!({
            "trade_type": "Buy",
            "date": "2024-01-15",
            "listing_id": 1,
            "average_price": "100",
            "quantity": "10",
            "currency": "AUD",
            "brokerage": "9.95",
            "gst_on_brokerage": "0.995",
            "brokerage_currency": "AUD",
            "fx_rate": "1"
        });
        let (status, _) = put_trade_json(&pool, 2, body).await;
        assert_eq!(status, StatusCode::NO_CONTENT);
        let (_, seen) = get_trade_raw(&pool, 2).await;
        assert_eq!(seen.brokerage, d("9.95"));
        assert_eq!(seen.gst_on_brokerage, d("0.995"));
    }

    /// The statement total must reconcile with quantity × price + brokerage +
    /// GST for a Buy: a matching figure (in any trailing-zero spelling) is
    /// accepted and stored; a mismatch is rejected with the computed figure in
    /// the 422 detail and nothing persisted.
    #[tokio::test]
    async fn api_statement_total_cross_check_on_buy() {
        let pool = test_pool().await;
        insert_test_listing(&pool).await;
        // 10 × 100 + 9.05 + 0.90 (from the 9.95 inclusive split) = 1009.95
        let body = serde_json::json!({
            "trade_type": "Buy",
            "date": "2024-01-15",
            "listing_id": 1,
            "average_price": "100",
            "quantity": "10",
            "currency": "AUD",
            "brokerage": "9.95",
            "brokerage_includes_gst": true,
            "brokerage_currency": "AUD",
            "fx_rate": "1",
            "statement_total": "1009.95"
        });
        let (status, _) = put_trade_json(&pool, 1, body.clone()).await;
        assert_eq!(status, StatusCode::NO_CONTENT);
        assert_eq!(
            db_get(&pool, 1).await.unwrap().unwrap().statement_total,
            Some(d("1009.95"))
        );

        // Numeric comparison: trailing zeros don't matter.
        let mut zeros = body.clone();
        zeros["statement_total"] = "1009.9500".into();
        let (status, _) = put_trade_json(&pool, 1, zeros).await;
        assert_eq!(status, StatusCode::NO_CONTENT);

        // A mismatch is rejected, says what the trade computes to, and
        // persists nothing.
        let mut wrong = body.clone();
        wrong["statement_total"] = "1010".into();
        let (status, detail) = put_trade_json(&pool, 2, wrong).await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
        assert!(
            detail.contains("1009.95"),
            "detail must carry the computed figure: {detail}"
        );
        assert!(
            db_get(&pool, 2).await.unwrap().is_none(),
            "nothing persisted"
        );
    }

    /// Contract notes print the consideration rounded to the cent, so a
    /// total equal to the computed figure cent-rounded (half away from
    /// zero) passes too. The figures are the three archive contract notes
    /// the exact comparison rejected (live trades 19, 16, 21):
    /// 1,302 × 37.585914 + 8.64 + 0.86 = 48,946.360028 → note 48,946.36;
    /// 562 × 73.259875 + 8.64 + 0.86 = 41,181.54975 → note 41,181.55;
    /// 0.02413796 × 3,983.77 + 3.84 = 100.000080… → note 100.00.
    /// A mismatch at the cent itself still rejects with the computed
    /// (unrounded) figure in the detail, and an exact .5-mil residue
    /// rounds away from zero, not to even.
    #[tokio::test]
    async fn api_statement_total_accepts_cent_rounded_contract_note_totals() {
        let pool = test_pool().await;
        insert_test_listing(&pool).await;
        let buy = |qty: &str, price: &str, brokerage: &str, gst: &str, total: &str| {
            serde_json::json!({
                "trade_type": "Buy",
                "date": "2024-03-01",
                "listing_id": 1,
                "average_price": price,
                "quantity": qty,
                "currency": "AUD",
                "brokerage": brokerage,
                "gst_on_brokerage": gst,
                "brokerage_currency": "AUD",
                "fx_rate": "1",
                "statement_total": total
            })
        };

        // Trade 19: HNDQ 1 Mar 2024 contract note 1404967.
        let hndq = buy("1302", "37.585914", "8.64", "0.86", "48946.36");
        let (status, _) = put_trade_json(&pool, 1, hndq).await;
        assert_eq!(status, StatusCode::NO_CONTENT);
        // Trade 16: VDHG 8 Apr 2026 contract note 4518597 (…54975 → .55).
        let vdhg = buy("562", "73.259875", "8.64", "0.86", "41181.55");
        let (status, _) = put_trade_json(&pool, 2, vdhg).await;
        assert_eq!(status, StatusCode::NO_CONTENT);
        // Trade 21: ETH 22 Sep 2021 card purchase (…0080… → 100.00).
        let eth = buy("0.02413796", "3983.77", "3.84", "0", "100.00");
        let (status, _) = put_trade_json(&pool, 3, eth).await;
        assert_eq!(status, StatusCode::NO_CONTENT);

        // An exact midpoint rounds half away from zero (100.005 → 100.01),
        // not banker's-to-even (100.00).
        let mid = buy("1", "100.005", "0", "0", "100.01");
        let (status, _) = put_trade_json(&pool, 4, mid).await;
        assert_eq!(status, StatusCode::NO_CONTENT);
        let even = buy("1", "100.005", "0", "0", "100.00");
        let (status, _) = put_trade_json(&pool, 5, even).await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);

        // Off by a whole cent still rejects, computed figure in the body.
        let wrong = buy("1302", "37.585914", "8.64", "0.86", "48946.37");
        let (status, detail) = put_trade_json(&pool, 6, wrong).await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
        assert!(
            detail.contains("48946.360028"),
            "detail must carry the computed figure: {detail}"
        );
        assert!(
            db_get(&pool, 6).await.unwrap().is_none(),
            "nothing persisted"
        );
    }

    /// A total can only be checked when the trade and brokerage currencies
    /// match — supplying one on a mixed-currency trade is rejected rather
    /// than inventing an FX conversion.
    #[tokio::test]
    async fn api_statement_total_on_mixed_currency_trade_returns_422() {
        let pool = test_pool().await;
        insert_test_listing(&pool).await;
        let body = serde_json::json!({
            "trade_type": "Buy",
            "date": "2024-01-15",
            "listing_id": 1,
            "average_price": "100",
            "quantity": "10",
            "currency": "USD",
            "brokerage": "9.95",
            "brokerage_currency": "AUD",
            "fx_rate": "1.5",
            "statement_total": "1009.95"
        });
        let (status, detail) = put_trade_json(&pool, 1, body).await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
        assert!(
            detail.contains("currencies"),
            "detail must explain the rejection: {detail}"
        );
        assert!(
            db_get(&pool, 1).await.unwrap().is_none(),
            "nothing persisted"
        );
    }

    /// The boolean column is CHECK-constrained to 0/1 in the database.
    #[tokio::test]
    async fn db_brokerage_includes_gst_check_constraint_enforced() {
        let pool = test_pool().await;
        insert_test_listing(&pool).await;
        db_upsert(&pool, &buy_trade()).await.unwrap();
        let err = sqlx::query("UPDATE trades SET brokerage_includes_gst = 2 WHERE id = 1")
            .execute(&pool)
            .await
            .unwrap_err();
        assert!(err.to_string().contains("CHECK"), "{err}");
    }

    // Spot-rate override (QC 18020): write-time validation and round-trip.

    #[tokio::test]
    async fn db_spot_fx_rate_round_trips_with_precision() {
        let pool = test_pool().await;
        insert_test_listing(&pool).await;
        test_support::buy(1, 1)
            .currency("USD")
            .spot_fx_rate("0.643215987".parse().unwrap())
            .insert(&pool)
            .await;
        let got = db_get(&pool, 1).await.unwrap().unwrap();
        assert_eq!(
            got.spot_fx_rate,
            Some("0.643215987".parse::<Decimal>().unwrap())
        );
        // Absent an override the column stays NULL — the unchanged default.
        test_support::buy(2, 1).currency("USD").insert(&pool).await;
        assert_eq!(db_get(&pool, 2).await.unwrap().unwrap().spot_fx_rate, None);
    }

    #[tokio::test]
    async fn db_spot_fx_rate_on_aud_trade_is_refused() {
        // An AUD amount never converts, so a spot override there could only
        // be a data-entry mistake silently ignored — rejected instead.
        let pool = test_pool().await;
        insert_test_listing(&pool).await;
        let trade = test_support::buy(1, 1)
            .spot_fx_rate("0.65".parse().unwrap())
            .build();
        let err = db_upsert(&pool, &trade).await.unwrap_err();
        assert!(
            matches!(err, UpsertError::SpotFxRate(SpotFxRateError::AudTrade)),
            "expected SpotFxRate(AudTrade), got: {err:?}"
        );
        assert!(db_get(&pool, 1).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn db_non_positive_spot_fx_rate_is_refused() {
        // The rate divides the amount (AUD = foreign / rate): zero or
        // negative can never be a real exchange rate.
        let pool = test_pool().await;
        insert_test_listing(&pool).await;
        for bad in ["0", "-0.5"] {
            let trade = test_support::buy(1, 1)
                .currency("USD")
                .spot_fx_rate(bad.parse().unwrap())
                .build();
            let err = db_upsert(&pool, &trade).await.unwrap_err();
            assert!(
                matches!(err, UpsertError::SpotFxRate(SpotFxRateError::NotPositive)),
                "expected SpotFxRate(NotPositive) for {bad}, got: {err:?}"
            );
        }
    }

    #[tokio::test]
    async fn api_put_trade_with_spot_fx_rate_persists_and_aud_is_422() {
        let pool = test_pool().await;
        insert_test_listing(&pool).await;
        let put = |pool: SqlitePool, body: serde_json::Value| async move {
            client(&pool).put("/trades/1", &body).await
        };
        // A USD trade with a deliberate spot rate persists it.
        let resp = put(
            pool.clone(),
            serde_json::json!({
                "trade_type": "Buy", "date": "2024-01-15", "listing_id": 1,
                "average_price": "100", "quantity": "10", "currency": "USD",
                "brokerage": "0", "brokerage_currency": "USD",
                "fx_rate": "0.70", "spot_fx_rate": "0.6543",
            }),
        )
        .await;
        assert_eq!(resp.status, StatusCode::NO_CONTENT);
        let got = db_get(&pool, 1).await.unwrap().unwrap();
        assert_eq!(got.spot_fx_rate, Some("0.6543".parse().unwrap()));

        // The same entry on an AUD trade is rejected with the reason.
        let resp = put(
            pool.clone(),
            serde_json::json!({
                "trade_type": "Buy", "date": "2024-01-15", "listing_id": 1,
                "average_price": "100", "quantity": "10", "currency": "AUD",
                "brokerage": "0", "brokerage_currency": "AUD",
                "fx_rate": "1", "spot_fx_rate": "0.6543",
            }),
        )
        .await;
        assert_eq!(resp.status, StatusCode::UNPROCESSABLE_ENTITY);
        let detail = resp.text();
        assert!(detail.contains("non-AUD"), "detail: {detail}");
    }
}
