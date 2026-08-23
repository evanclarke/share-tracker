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
// The Sell path shares this DB-level rule, as it shares `check_amounts`.
pub(crate) use db::listing_currency_mismatch;
pub use http::router;
/// The provenance of a stored `settlement_date`. Named outside this module
/// only by tests (the write paths set it through [`Settlement`], and the
/// non-test build reaches the type through `Trade`'s own field), so the
/// re-export is test-gated to keep the non-test build warning-free.
#[cfg(test)]
pub use model::SettlementDateSource;
pub use model::{Trade, TradeBody, TradeType};

pub(crate) use checks::{
    AmountsCheck, AmountsError, CGT_START, SpotFxRateError, StatementTotalCheck,
    StatementTotalError, TradeAmounts, amounts_detail, check_amounts, check_statement_total,
    resolve_brokerage, spot_fx_rate_detail, statement_total_detail, validate_spot_fx_rate,
};
pub(crate) use settlement::Settlement;
/// Reached by name only from tests — the write paths go through
/// [`Settlement::resolve`], which is where the omitted-means-computed rule
/// lives — so the re-export is test-gated.
#[cfg(test)]
pub(crate) use settlement::auto_settlement_date;
/// The `settlement-recompute` maintenance job (SCENARIOS S-04), registered in
/// `infra::scheduler::registry` and deliberately unscheduled.
pub use settlement::run_recompute;

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

    /// A USD-quoted listing, for the cases about a foreign trade: a trade is
    /// recorded in its listing's currency (`UpsertError::CurrencyNotListings`).
    async fn insert_usd_listing(pool: &SqlitePool) {
        test_support::listing(1)
            .mic("XNYS")
            .ticker("VTS")
            .name("Vanguard US Total Market Shares Index ETF")
            .currency("USD")
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
                date: NaiveDate::from_ymd_opt(2024, 6, 3).unwrap(),
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

    /// A second holding account, so a parcel has somewhere to be moved to.
    async fn insert_second_account(pool: &SqlitePool) {
        use crate::entities::holding_account::{self, HoldingAccount};
        holding_account::db_upsert(
            pool,
            &HoldingAccount {
                id: 2,
                name: "ICE Employee Plan".to_string(),
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

        // 'ZZZ' is not a recognised currency → the currency columns' FK
        // rejects it. Both columns carry it: the write path requires the pair
        // to match (SCENARIOS B-02), so an unrecognised code can only ever
        // reach the database on both at once.
        let mut bad_currency = buy_trade();
        bad_currency.currency = "ZZZ".to_string();
        bad_currency.brokerage_currency = "ZZZ".to_string();
        assert_fk_error(
            db_upsert(&pool, &bad_currency).await.unwrap_err(),
            "currency",
        );

        // `brokerage_currency` carries its own FK all the same — pinned
        // directly, since no write path can reach it alone.
        db_upsert(&pool, &buy_trade()).await.unwrap();
        let err = sqlx::query("UPDATE trades SET brokerage_currency = 'ZZZ' WHERE id = 1")
            .execute(&pool)
            .await
            .unwrap_err();
        assert!(err.to_string().contains("FOREIGN KEY"), "{err}");

        // A seeded digital-token code (BTC) is a recognised currency and is
        // accepted — on a listing quoted in it, since a trade is recorded in
        // its listing's currency (`UpsertError::CurrencyNotListings`). An
        // ETH/BTC pair is the ordinary case.
        test_support::listing(2)
            .crypto()
            .ticker("ETH")
            .name("Ether")
            .currency("BTC")
            .insert(&pool)
            .await;
        let mut btc = buy_trade();
        btc.id = 2;
        btc.listing_id = 2;
        btc.currency = "BTC".to_string();
        btc.brokerage_currency = "BTC".to_string();
        db_upsert(&pool, &btc).await.unwrap();
    }

    #[tokio::test]
    async fn db_sell_trade_insert_and_retrieve() {
        let pool = test_pool().await;
        insert_test_listing(&pool).await;
        let trade = test_support::sell(2, 1)
            .date(ymd(2024, 6, 3))
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
            "average_price": "100.0",
            "quantity": "10.0",
            "currency": "AUD",
            "brokerage": "9.95",
            "gst_on_brokerage": "0.995",
            "brokerage_currency": "AUD",
            "fx_rate": "1.0"
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

    /// A Buy's date may not move past a Sell that allocates from it: the
    /// parcel side of `sell::SellError::PurchaseAfterSale`, which the Sell
    /// path refuses from its own end. Without it the sale is costed against a
    /// parcel acquired after it and the discount clock runs backwards
    /// (SCENARIOS A-09).
    #[tokio::test]
    async fn db_date_move_past_an_allocating_sell_is_refused() {
        let pool = test_pool().await;
        insert_test_listing(&pool).await;
        db_upsert(&pool, &buy_trade()).await.unwrap(); // 2024-01-15
        insert_sell_consuming(&pool, 2, 1, Decimal::from(5)).await; // sale 2024-06-01

        let mut moved = buy_trade();
        moved.date = ymd(2024, 7, 1);
        moved.settlement_date = ymd(2024, 7, 3);
        let err = db_upsert(&pool, &moved).await.unwrap_err();
        assert!(
            matches!(err, UpsertError::DateAfterAllocatedSale),
            "expected DateAfterAllocatedSale, got: {err:?}"
        );
        assert_eq!(
            db_get(&pool, 1).await.unwrap().unwrap().date,
            ymd(2024, 1, 15)
        );

        // Up to the sale date itself is fine (a same-day parcel is a valid
        // allocation on the Sell side too), as is moving earlier.
        let mut same_day = buy_trade();
        same_day.date = ymd(2024, 6, 3);
        same_day.settlement_date = ymd(2024, 6, 3);
        db_upsert(&pool, &same_day).await.unwrap();
        assert_eq!(
            db_get(&pool, 1).await.unwrap().unwrap().date,
            ymd(2024, 6, 3)
        );
    }

    /// The date freeze lifts once nothing allocates from the parcel.
    #[tokio::test]
    async fn db_date_move_is_free_while_nothing_allocates() {
        let pool = test_pool().await;
        insert_test_listing(&pool).await;
        db_upsert(&pool, &buy_trade()).await.unwrap();

        let mut moved = buy_trade();
        moved.date = ymd(2024, 7, 1);
        moved.settlement_date = ymd(2024, 7, 3);
        db_upsert(&pool, &moved).await.unwrap();
        assert_eq!(
            db_get(&pool, 1).await.unwrap().unwrap().date,
            ymd(2024, 7, 1)
        );
    }

    /// A Buy's holding account is frozen while a Sell allocates from it: a
    /// sale only disposes of units its own account holds, so moving the parcel
    /// away would report it held in one account while the realised gain stays
    /// costed against it in another (SCENARIOS A-13).
    #[tokio::test]
    async fn db_account_change_on_allocated_parcel_is_refused() {
        let pool = test_pool().await;
        insert_test_listing(&pool).await;
        insert_second_account(&pool).await;
        db_upsert(&pool, &buy_trade()).await.unwrap(); // account 1
        insert_sell_consuming(&pool, 2, 1, Decimal::from(5)).await;

        let mut moved = buy_trade();
        moved.holding_account_id = 2;
        let err = db_upsert(&pool, &moved).await.unwrap_err();
        assert!(
            matches!(err, UpsertError::AccountChangeReferenced),
            "expected AccountChangeReferenced, got: {err:?}"
        );
        assert_eq!(
            db_get(&pool, 1).await.unwrap().unwrap().holding_account_id,
            1
        );
    }

    /// The same freeze applies while an AMIT adjustment covers the parcel (a
    /// statement only adjusts its own account's parcels) — and lifts once
    /// nothing references it.
    #[tokio::test]
    async fn db_account_change_under_amit_adjustment_is_refused_until_unlinked() {
        let pool = test_pool().await;
        insert_test_listing(&pool).await;
        insert_second_account(&pool).await;
        db_upsert(&pool, &buy_trade()).await.unwrap();
        insert_amit_adjustment_covering(&pool, 1, Decimal::from(8)).await;

        let mut moved = buy_trade();
        moved.holding_account_id = 2;
        let err = db_upsert(&pool, &moved).await.unwrap_err();
        assert!(
            matches!(err, UpsertError::AccountChangeReferenced),
            "expected AccountChangeReferenced, got: {err:?}"
        );
        assert_eq!(
            db_get(&pool, 1).await.unwrap().unwrap().holding_account_id,
            1
        );

        // With the adjustment removed the account moves freely again.
        sqlx::query("DELETE FROM amit_adjustments WHERE trade_id = 1")
            .execute(&pool)
            .await
            .unwrap();
        db_upsert(&pool, &moved).await.unwrap();
        assert_eq!(
            db_get(&pool, 1).await.unwrap().unwrap().holding_account_id,
            2
        );
    }

    /// Both refusals reach the API as a 422 naming the rule, with nothing
    /// persisted — the states `PUT /sells/:id` itself refuses.
    #[tokio::test]
    async fn api_put_trade_moving_date_or_account_off_an_allocating_sell_returns_422() {
        let pool = test_pool().await;
        insert_test_listing(&pool).await;
        insert_second_account(&pool).await;
        db_upsert(&pool, &buy_trade()).await.unwrap();
        insert_sell_consuming(&pool, 2, 1, Decimal::from(5)).await;

        let body = |date: &str, account: i64| {
            serde_json::json!({
                "trade_type": "Buy",
                "date": date,
                "listing_id": 1,
                "average_price": "100.0",
                "quantity": "10.0",
                "currency": "AUD",
                "brokerage": "9.95",
                "gst_on_brokerage": "0.995",
                "brokerage_currency": "AUD",
                "fx_rate": "1.0",
                "holding_account_id": account
            })
        };

        let moved_date = client(&pool).put("/trades/1", &body("2024-07-01", 1)).await;
        let (status, detail) = moved_date.status_and_body();
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
        assert!(
            detail.contains("date cannot move after a Sell"),
            "expected the date rule, got: {detail}"
        );

        let moved_account = client(&pool).put("/trades/1", &body("2024-01-15", 2)).await;
        let (status, detail) = moved_account.status_and_body();
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
        assert!(
            detail.contains("holding account cannot be changed"),
            "expected the holding-account rule, got: {detail}"
        );

        let unchanged = db_get(&pool, 1).await.unwrap().unwrap();
        assert_eq!(unchanged.date, ymd(2024, 1, 15));
        assert_eq!(unchanged.holding_account_id, 1);
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
            "average_price": "100.0",
            "quantity": "10.0",
            "currency": "AUD",
            "brokerage": "9.95",
            "gst_on_brokerage": "0.995",
            "brokerage_currency": "AUD",
            "fx_rate": "1.0"
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
        // 2024-06-07 is a Friday: on a T+2 exchange listing this would settle
        // the following Wednesday (the weekend, then the King's Birthday
        // Monday), so a same-day settlement can only come from the
        // exchange-less path.
        let body = serde_json::json!({
            "trade_type": "Buy",
            "date": "2024-06-07",
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
            NaiveDate::from_ymd_opt(2024, 6, 7).unwrap()
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
        // XASX holidays are seeded 2019–2027 only: a 2018 trade's settlement is
        // computed skipping weekends only, so the auto-population warns rather
        // than silently using the incomplete calendar. (The gap is probed
        // below the seeded span rather than above it because a trade dated
        // after today is refused outright — SCENARIOS S-10.)
        let body = serde_json::json!({
            "trade_type": "Buy",
            "date": "2018-06-04",
            "listing_id": 1,
            "average_price": "100.0",
            "quantity": "10.0",
            "currency": "AUD",
            "brokerage": "0.0",
            "gst_on_brokerage": "0.0",
            "brokerage_currency": "AUD",
            "fx_rate": "1.0"
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
            "average_price": "100.0",
            "quantity": "10.0",
            "currency": "AUD",
            "brokerage": "0.0",
            "gst_on_brokerage": "0.0",
            "brokerage_currency": "AUD",
            "fx_rate": "1.0"
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
            "average_price": "100.0",
            "quantity": "10.0",
            "currency": "AUD",
            "brokerage": "9.95",
            "gst_on_brokerage": "0.995",
            "brokerage_currency": "AUD",
            "fx_rate": "1.0"
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
            "average_price": "100.0",
            "quantity": "10.0",
            "currency": "AUD",
            "brokerage": "9.95",
            "gst_on_brokerage": "0.995",
            "brokerage_currency": "AUD",
            "fx_rate": "1.0"
        });
        let resp = client(&pool).put("/trades/1", &body).await;
        assert_eq!(resp.status, StatusCode::NO_CONTENT);
        let trade = db_get(&pool, 1).await.unwrap().unwrap();
        assert_eq!(
            trade.settlement_date,
            NaiveDate::from_ymd_opt(2024, 1, 23).unwrap()
        );
    }

    /// SCENARIOS S-05: the guarantee the *auto* path gives, asserted rather
    /// than assumed. A supplied `settlement_date` is a deliberate override and
    /// is never refused — the settlement-holiday-coverage report flags one
    /// that lands on a closed day — but a settlement this code computes must
    /// land on a trading day, because `add_business_days` skips weekends and
    /// the exchange's seeded holidays by construction. Walked over every
    /// trading day of both seeded calendars (XASX and XNYS, 2019–2027), each
    /// settled through the real write path's helper and each result put back
    /// to the very calendar the trading-day refusal reads.
    ///
    /// Windows running past the end of coverage are skipped, not asserted:
    /// there the calendar is *incomplete*, which is the one way the auto path
    /// can still produce a closed day (SCENARIOS S-04) and is exactly what the
    /// coverage report exists to say.
    #[tokio::test]
    async fn auto_settlement_never_lands_on_a_non_trading_day_under_a_complete_calendar() {
        use crate::entities::closing_price;
        let pool = test_pool().await;
        test_support::listing(1).mic("XASX").insert(&pool).await;
        test_support::listing(2)
            .mic("XNYS")
            .ticker("LAC")
            .currency("USD")
            .insert(&pool)
            .await;
        // The seeded calendars run 2019–2027 on both exchanges.
        let coverage_end = ymd(2027, 12, 31);
        let mut checked = 0;
        for listing_id in [1, 2] {
            let market = closing_price::load_market(&pool, listing_id)
                .await
                .unwrap()
                .unwrap();
            let mut date = ymd(2019, 1, 1);
            while date <= coverage_end {
                if closing_price::non_trading_day(&market, date).is_none() {
                    let settled = auto_settlement_date(&pool, 1, listing_id, date)
                        .await
                        .unwrap();
                    if settled <= coverage_end {
                        assert!(
                            closing_price::non_trading_day(&market, settled).is_none(),
                            "listing {listing_id}: {date} settled on {settled}, not a trading day"
                        );
                        checked += 1;
                    }
                }
                date += chrono::Duration::days(1);
            }
        }
        // Not a vacuous pass: two exchanges' worth of trading days.
        assert!(checked > 4000, "only {checked} settlements were checked");
    }

    /// A Buy body with `settlement_date` omitted, so the server computes it —
    /// the only way a trade's settlement date is recorded as `computed`, and
    /// so the only shape the `settlement-recompute` job will ever rewrite.
    fn auto_settled_buy(date: NaiveDate) -> serde_json::Value {
        serde_json::json!({
            "trade_type": "Buy",
            "date": date,
            "listing_id": 1,
            "average_price": "100.0",
            "quantity": "10.0",
            "currency": "AUD",
            "brokerage": "0",
            "gst_on_brokerage": "0",
            "brokerage_currency": "AUD",
            "fx_rate": "1.0"
        })
    }

    async fn seed_holiday(app: &ApiClient, date: NaiveDate, name: &str) {
        app.put_ok(
            format!("/exchange_holidays/XASX/{date}"),
            &serde_json::json!({ "name": name }),
        )
        .await;
    }

    /// SCENARIOS S-04's four-step reproduction, end to end through the API.
    ///
    /// Transposed to the 2018 Easter, because the reproduction as driven
    /// (2028) can no longer be entered: S-10 refuses a trade dated after
    /// today. The shape is identical — a Thursday before a Good Friday in a
    /// year with no seeded `exchange_holidays` rows — and 2018 sits before the
    /// seeded 2019–2027 span, so the calendar is missing at the same end of it.
    ///
    /// 1. The Buy auto-computes T+2 skipping weekends only and lands on the
    ///    Easter Monday nobody has entered; the coverage report says so.
    /// 2. The user seeds the year the report asked for.
    /// 3. The window is now inside coverage — the report's first question goes
    ///    quiet (here its second one still catches the row, because this
    ///    settlement happens to land on the seeded holiday itself; the
    ///    following test is the case where it does not).
    /// 4. The stored settlement date is still the Easter Monday until the
    ///    `settlement-recompute` job re-derives it, and then the report is
    ///    empty because the date is right rather than because it is hidden.
    #[tokio::test]
    async fn seeding_a_missing_calendar_and_recomputing_corrects_the_settlement_it_left_wrong() {
        let pool = test_pool().await;
        test_support::listing(1).mic("XASX").insert(&pool).await;
        let app = ApiClient::full(&pool);

        // 1. Thursday 2018-03-29, T+2 over an empty 2018 calendar.
        app.put_ok("/trades/1", &auto_settled_buy(ymd(2018, 3, 29)))
            .await;
        let trade: Trade = app.get_json("/trades/1").await;
        assert_eq!(trade.settlement_date, ymd(2018, 4, 2)); // Easter Monday
        assert_eq!(trade.settlement_date_source, SettlementDateSource::Computed);
        let alerts: Vec<serde_json::Value> =
            app.get_json("/reports/settlement_holiday_coverage").await;
        assert_eq!(alerts.len(), 1);
        assert_eq!(alerts[0]["coverage_status"], "outside_holiday_coverage");

        // 2. The user seeds the 2018 calendar the report asked for.
        seed_holiday(&app, ymd(2018, 3, 30), "Good Friday").await;
        seed_holiday(&app, ymd(2018, 4, 2), "Easter Monday").await;

        // 3. The window is inside coverage now; only the trading-day question
        //    still holds the row — and 4. the stored date has not moved.
        let alerts: Vec<serde_json::Value> =
            app.get_json("/reports/settlement_holiday_coverage").await;
        assert_eq!(alerts.len(), 1);
        assert_eq!(alerts[0]["coverage_status"], "inside_holiday_coverage");
        assert_eq!(alerts[0]["settlement_non_trading_reason"], "holiday");
        let trade: Trade = app.get_json("/trades/1").await;
        assert_eq!(trade.settlement_date, ymd(2018, 4, 2));

        // The job is what repairs it: T+2 over the completed calendar skips
        // Good Friday, the weekend and Easter Monday.
        app.post_empty("/jobs/settlement-recompute")
            .await
            .expect_status(StatusCode::NO_CONTENT);
        let trade: Trade = app.get_json("/trades/1").await;
        assert_eq!(trade.settlement_date, ymd(2018, 4, 4));
        assert_eq!(
            trade.settlement_date_source,
            SettlementDateSource::Computed,
            "a recomputed date is still a computed one"
        );
        let alerts: Vec<serde_json::Value> =
            app.get_json("/reports/settlement_holiday_coverage").await;
        assert!(alerts.is_empty(), "still flagged: {alerts:?}");

        // trades is audited, and the job writes through the same triggers as
        // any other update: the superseded date is recoverable.
        let history: Vec<serde_json::Value> = app
            .post_json(
                "/reports/row_history",
                &serde_json::json!({ "table": "trades", "row_id": 1 }),
            )
            .await;
        assert_eq!(history.len(), 1);
        assert_eq!(history[0]["operation"], "UPDATE");
        assert_eq!(history[0]["settlement_date"], "2018-04-02");
    }

    /// SCENARIOS S-04's general case — the one the S-05 trading-day check
    /// cannot see. Only the holiday *inside* the window is missing, so the
    /// settlement lands one business day early on a perfectly good trading
    /// day: seeding the calendar silences the coverage report completely, and
    /// nothing but the recompute job would ever correct the stored date.
    #[tokio::test]
    async fn recompute_corrects_a_settlement_left_a_day_early_by_a_missing_holiday() {
        let pool = test_pool().await;
        test_support::listing(1).mic("XASX").insert(&pool).await;
        let app = ApiClient::full(&pool);

        app.put_ok("/trades/1", &auto_settled_buy(ymd(2018, 3, 29)))
            .await;
        // Good Friday alone: the settlement stays on Monday 2018-04-02, which
        // is an ordinary trading day while Easter Monday is unseeded.
        seed_holiday(&app, ymd(2018, 3, 30), "Good Friday").await;
        let alerts: Vec<serde_json::Value> =
            app.get_json("/reports/settlement_holiday_coverage").await;
        assert!(
            alerts.is_empty(),
            "the report has gone quiet, which is the finding: {alerts:?}"
        );
        let trade: Trade = app.get_json("/trades/1").await;
        assert_eq!(trade.settlement_date, ymd(2018, 4, 2));

        app.post_empty("/jobs/settlement-recompute")
            .await
            .expect_status(StatusCode::NO_CONTENT);
        let trade: Trade = app.get_json("/trades/1").await;
        assert_eq!(trade.settlement_date, ymd(2018, 4, 3));
    }

    /// SCENARIOS S-04: a stored settlement that already matches what the
    /// current calendar computes is left exactly as it is — no write, so no
    /// audit entry and no snapshot staled. That is also what makes the job
    /// idempotent: the second run of the test above would do this.
    #[tokio::test]
    async fn recompute_leaves_a_settlement_that_already_matches_the_calendar_untouched() {
        let pool = test_pool().await;
        test_support::listing(1).mic("XASX").insert(&pool).await;
        let app = ApiClient::full(&pool);

        // 2024 is inside the seeded calendar: Monday 2024-01-15 settles T+2 on
        // the Wednesday, with nothing missing to correct.
        app.put_ok("/trades/1", &auto_settled_buy(ymd(2024, 1, 15)))
            .await;
        let before: Trade = app.get_json("/trades/1").await;
        assert_eq!(before.settlement_date, ymd(2024, 1, 17));

        app.post_empty("/jobs/settlement-recompute")
            .await
            .expect_status(StatusCode::NO_CONTENT);
        let after: Trade = app.get_json("/trades/1").await;
        assert_eq!(after.settlement_date, before.settlement_date);
        let history: Vec<serde_json::Value> = app
            .post_json(
                "/reports/row_history",
                &serde_json::json!({ "table": "trades", "row_id": 1 }),
            )
            .await;
        assert!(history.is_empty(), "nothing should have been written");
    }

    /// SCENARIOS S-04/S-05: a **hand-supplied** settlement date is the
    /// taxpayer's own assertion and the job never touches it — reproducing the
    /// live database's trade 9071 (LAC on XNYS, dated 2021-03-25 with an
    /// explicit `settlement_date` of 2021-05-29, a Saturday two months later).
    /// It stays exactly as entered, still flagged by the coverage report, and
    /// re-saving it through the API (which sends the date back verbatim) does
    /// not turn it into a computed one either.
    #[tokio::test]
    async fn recompute_leaves_a_hand_supplied_settlement_untouched() {
        let pool = test_pool().await;
        test_support::listing(1)
            .mic("XNYS")
            .ticker("LAC")
            .currency("USD")
            .insert(&pool)
            .await;
        let app = ApiClient::full(&pool);
        let mut body = auto_settled_buy(ymd(2021, 3, 25));
        body["currency"] = "USD".into();
        body["brokerage_currency"] = "USD".into();
        body["settlement_date"] = "2021-05-29".into();
        app.put_ok("/trades/9071", &body).await;
        let stored: Trade = app.get_json("/trades/9071").await;
        assert_eq!(stored.settlement_date, ymd(2021, 5, 29));
        assert_eq!(stored.settlement_date_source, SettlementDateSource::Stated);

        app.post_empty("/jobs/settlement-recompute")
            .await
            .expect_status(StatusCode::NO_CONTENT);
        let after: Trade = app.get_json("/trades/9071").await;
        assert_eq!(after.settlement_date, ymd(2021, 5, 29));
        assert_eq!(after.settlement_date_source, SettlementDateSource::Stated);
        let alerts: Vec<serde_json::Value> =
            app.get_json("/reports/settlement_holiday_coverage").await;
        assert_eq!(alerts.len(), 1);
        assert_eq!(alerts[0]["trade_id"], 9071);
        assert_eq!(alerts[0]["settlement_non_trading_reason"], "weekend");
        let history: Vec<serde_json::Value> = app
            .post_json(
                "/reports/row_history",
                &serde_json::json!({ "table": "trades", "row_id": 9071 }),
            )
            .await;
        assert!(
            history.is_empty(),
            "the assertion was rewritten: {history:?}"
        );
    }

    /// SCENARIOS S-04: a row written before the provenance column existed
    /// (migration 0041's default, which every row in the live database takes)
    /// records nothing about how its settlement date was arrived at, so the
    /// job leaves it alone rather than guessing — it might be an assertion
    /// like trade 9071's. A re-save through the API keeps it that way while
    /// the date is unchanged; entering a different one is what makes it a
    /// statement.
    #[tokio::test]
    async fn recompute_leaves_a_row_from_before_the_provenance_column_untouched() {
        let pool = test_pool().await;
        test_support::listing(1).mic("XASX").insert(&pool).await;
        test_support::buy(1, 1)
            .date(ymd(2024, 1, 15))
            .settlement(ymd(2024, 1, 18)) // a day later than T+2 computes
            .settlement_source(SettlementDateSource::Unrecorded)
            .insert(&pool)
            .await;
        let app = ApiClient::full(&pool);

        app.post_empty("/jobs/settlement-recompute")
            .await
            .expect_status(StatusCode::NO_CONTENT);
        let after: Trade = app.get_json("/trades/1").await;
        assert_eq!(after.settlement_date, ymd(2024, 1, 18));
        assert_eq!(
            after.settlement_date_source,
            SettlementDateSource::Unrecorded
        );

        // Re-saving the row verbatim is not an assertion about the date.
        let mut body = auto_settled_buy(ymd(2024, 1, 15));
        body["settlement_date"] = "2024-01-18".into();
        app.put_ok("/trades/1", &body).await;
        let after: Trade = app.get_json("/trades/1").await;
        assert_eq!(
            after.settlement_date_source,
            SettlementDateSource::Unrecorded
        );

        // Entering a different date is.
        body["settlement_date"] = "2024-01-19".into();
        app.put_ok("/trades/1", &body).await;
        let after: Trade = app.get_json("/trades/1").await;
        assert_eq!(after.settlement_date_source, SettlementDateSource::Stated);
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
            "average_price": "100.0",
            "quantity": "10.0",
            "currency": "AUD",
            "brokerage": "9.95",
            "gst_on_brokerage": "0.995",
            "brokerage_currency": "AUD",
            "fx_rate": "1.0"
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
            "average_price": "95.0",
            "quantity": "2.0",
            "currency": "AUD",
            "brokerage": "0.0",
            "gst_on_brokerage": "0.0",
            "brokerage_currency": "AUD",
            "fx_rate": "1.0"
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
            "average_price": "100.0",
            "quantity": "10.0",
            "currency": "AUD",
            "brokerage": "9.95",
            "gst_on_brokerage": "0.995",
            "brokerage_currency": "AUD",
            "fx_rate": "1.0"
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

    /// SCENARIOS S-10: a trade dated after the server's current date is
    /// rejected 422 — the upper twin of the pre-CGT floor. A trade records a
    /// transaction that has already happened, so a future date is a typo (a
    /// 2027-for-2026 slip on a July trade), and it put a financial year that
    /// has not begun on the annual tax report's year picker. Today itself is
    /// the boundary and stays accepted — with its *settlement* landing in the
    /// future, which is deliberately not bounded (a T+2 settlement of a trade
    /// dated today has not happened yet).
    ///
    /// The boundary is checked on an exchange-less (Crypto) listing, which
    /// trades every day: on a listed security the trading-day rule (SCENARIOS
    /// S-08) refuses a trade dated today whenever the suite runs on a weekend
    /// or an exchange holiday, and that would make this test's answer depend
    /// on the day of the week. The settlement half is asserted separately, on
    /// the listed security, from the most recent day its market was open —
    /// T+2 business days from any such day is always after today.
    #[tokio::test]
    async fn api_future_dated_trade_rejected_422_and_today_accepted() {
        let pool = test_pool().await;
        insert_test_listing(&pool).await;
        test_support::listing(2)
            .crypto()
            .ticker("ETH")
            .name("Ether")
            .insert(&pool)
            .await;
        let today = crate::infra::date::today();
        let base = serde_json::json!({
            "trade_type": "Buy",
            "date": (today + chrono::Days::new(1)).to_string(),
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
            detail.contains("dated after today"),
            "detail must explain the rule, got: {detail}"
        );
        assert!(db_get(&pool, 1).await.unwrap().is_none());

        // Today — the last day inside the window — is accepted.
        let mut boundary = base.clone();
        boundary["date"] = today.to_string().into();
        boundary["listing_id"] = 2.into();
        let (status, detail) = put_trade_json(&pool, 1, boundary).await;
        assert_eq!(status, StatusCode::NO_CONTENT, "detail: {detail}");
        let stored = db_get(&pool, 1).await.unwrap().unwrap();
        assert_eq!(stored.date, today);

        // And the settlement date is allowed past it: a T+2 settlement of a
        // trade dated the market's most recent open day always falls after
        // today, whichever day of the week the suite runs on.
        let market = crate::entities::closing_price::load_market(&pool, 1)
            .await
            .unwrap()
            .unwrap();
        let last_open = market
            .latest_trading_day_on_or_before(today)
            .expect("XASX has an open day in the past year");
        let mut settled = base;
        settled["date"] = last_open.to_string().into();
        let (status, detail) = put_trade_json(&pool, 2, settled).await;
        assert_eq!(status, StatusCode::NO_CONTENT, "detail: {detail}");
        let stored = db_get(&pool, 2).await.unwrap().unwrap();
        assert!(
            stored.settlement_date > today,
            "T+2 settlement of a trade dated {last_open} is legitimately in the future, got {}",
            stored.settlement_date
        );
    }

    /// SCENARIOS S-08: a trade dated on a day its exchange did not trade —
    /// a Saturday, or a seeded `exchange_holidays` date — is rejected 422
    /// naming the day and the exchange. The trade date is the CGT event date,
    /// so it sets the 12-month discount clock, the financial year the gain
    /// falls in and the day the T+n count starts from; a day the market was
    /// shut is a data-entry error by construction. The calendar is the one
    /// `PUT /closing_prices` already refuses a non-trading day on.
    #[tokio::test]
    async fn api_trade_on_a_non_trading_day_rejected_422() {
        let pool = test_pool().await;
        insert_test_listing(&pool).await;
        let base = serde_json::json!({
            "trade_type": "Buy",
            "date": "2024-01-15",
            "listing_id": 1,
            "average_price": "100",
            "quantity": "10",
            "currency": "AUD",
            "brokerage": "0",
            "gst_on_brokerage": "0",
            "brokerage_currency": "AUD",
            "fx_rate": "1"
        });

        // Saturday 2024-01-13.
        let mut saturday = base.clone();
        saturday["date"] = "2024-01-13".into();
        let (status, detail) = put_trade_json(&pool, 1, saturday).await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
        assert!(
            detail.contains("did not trade") && detail.contains("Saturday"),
            "the refusal must name the day, got: {detail}"
        );
        assert!(
            detail.contains("XASX"),
            "the refusal must name the exchange, got: {detail}"
        );
        assert!(db_get(&pool, 1).await.unwrap().is_none());

        // Australia Day 2024-01-26 — a Friday, and a seeded XASX holiday.
        let mut holiday = base.clone();
        holiday["date"] = "2024-01-26".into();
        let (status, detail) = put_trade_json(&pool, 1, holiday).await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
        assert!(
            detail.contains("public holiday") && detail.contains("XASX"),
            "the refusal must name the holiday and the exchange, got: {detail}"
        );
        assert!(db_get(&pool, 1).await.unwrap().is_none());

        // The Monday between them is an ordinary trading day.
        let (status, _) = put_trade_json(&pool, 1, base).await;
        assert_eq!(status, StatusCode::NO_CONTENT);
    }

    /// SCENARIOS S-08: the same Saturday is accepted for an exchange-less
    /// (Crypto) listing — a crypto asset trades every day, which is why it
    /// also settles same-day (the `L-15` shape). The calendar rule must never
    /// reach it.
    #[tokio::test]
    async fn api_crypto_trade_on_a_saturday_is_accepted() {
        let pool = test_pool().await;
        test_support::listing(1)
            .crypto()
            .ticker("ETH")
            .name("Ether")
            .insert(&pool)
            .await;
        let body = serde_json::json!({
            "trade_type": "Buy",
            "date": "2024-01-13",
            "listing_id": 1,
            "average_price": "100",
            "quantity": "10",
            "currency": "AUD",
            "brokerage": "0",
            "gst_on_brokerage": "0",
            "brokerage_currency": "AUD",
            "fx_rate": "1"
        });
        let (status, detail) = put_trade_json(&pool, 1, body).await;
        assert_eq!(status, StatusCode::NO_CONTENT, "{detail}");
        let stored = db_get(&pool, 1).await.unwrap().unwrap();
        assert_eq!(stored.date, ymd(2024, 1, 13));
        // Same-day settlement, unchanged by this rule.
        assert_eq!(stored.settlement_date, ymd(2024, 1, 13));
    }

    /// SCENARIOS S-08: `exchange_holidays` is seeded for 2019–2027 only, so a
    /// year outside it has no holiday rows at all. A weekday there must stay
    /// recordable — an unseeded year cannot become unenterable — while its
    /// weekends are still refused, since a weekend needs no calendar.
    #[tokio::test]
    async fn api_trade_in_a_year_with_no_seeded_calendar_is_accepted_on_a_weekday() {
        let pool = test_pool().await;
        insert_test_listing(&pool).await;
        let base = serde_json::json!({
            "trade_type": "Buy",
            // Christmas Day 2018 — a Tuesday, and a real ASX holiday, but the
            // seeded calendar does not reach 2018, so nothing here knows that.
            "date": "2018-12-25",
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
        assert_eq!(status, StatusCode::NO_CONTENT, "{detail}");

        // The weekend either side of it still is: Saturday 2018-12-22.
        let mut saturday = base;
        saturday["date"] = "2018-12-22".into();
        let (status, detail) = put_trade_json(&pool, 2, saturday).await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
        assert!(detail.contains("Saturday"), "got: {detail}");
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

            // Minus the columns a read carries that no write body owns —
            // the id and the trade's provenance/derived columns, which the
            // body denies rather than ignores (SCENARIOS V-a).
            let read: serde_json::Value = serde_json::from_slice(&raw).unwrap();
            let body = crate::test_support::writable_body(&read, &[]);
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

    /// A brokerage billed in a currency other than the trade's is rejected at
    /// write time (SCENARIOS B-02): the cost base, a Sell's net proceeds and
    /// the activity ledger's transaction total are all single-currency sums,
    /// so an AUD fee on a USD trade would be added at the USD scale and
    /// silently mis-cost the parcel. Rejected with or without a statement
    /// total — the total's own cross-check used to be the only thing reading
    /// `brokerage_currency`, and now can never see a mixed-currency trade.
    #[tokio::test]
    async fn api_brokerage_in_another_currency_than_the_trade_returns_422() {
        let pool = test_pool().await;
        insert_usd_listing(&pool).await;
        let trade = |total: serde_json::Value| {
            serde_json::json!({
                "trade_type": "Buy",
                "date": "2024-01-16",
                "listing_id": 1,
                "average_price": "100",
                "quantity": "10",
                "currency": "USD",
                "brokerage": "30",
                "gst_on_brokerage": "3",
                "brokerage_currency": "AUD",
                "fx_rate": "1.5",
                "statement_total": total
            })
        };
        for (id, total) in [(1, serde_json::Value::Null), (2, serde_json::json!("1033"))] {
            let (status, detail) = put_trade_json(&pool, id, trade(total)).await;
            assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
            assert!(
                detail.contains("brokerage_currency must equal the trade's currency"),
                "detail must explain the rejection: {detail}"
            );
            assert!(
                db_get(&pool, id).await.unwrap().is_none(),
                "nothing persisted"
            );
        }
        // The same fee converted into the trade's own currency is accepted —
        // the documented way to record it (docs/API.md Known limitations).
        let (status, _) = put_trade_json(
            &pool,
            3,
            serde_json::json!({
                "trade_type": "Buy",
                "date": "2024-01-16",
                "listing_id": 1,
                "average_price": "100",
                "quantity": "10",
                "currency": "USD",
                "brokerage": "15",
                "gst_on_brokerage": "1.5",
                "brokerage_currency": "USD",
                "fx_rate": "1.5"
            }),
        )
        .await;
        assert_eq!(status, StatusCode::NO_CONTENT);
    }

    /// The parcel side of the return-of-capital currency invariant (SCENARIOS
    /// E-07/E-39): a payment reduces each parcel's cost base in the *parcel's*
    /// own currency, so a Buy recorded in another currency than a payment that
    /// reaches it is refused here too — otherwise the hole the
    /// corporate-action write path closes simply reopens from the parcel side,
    /// and every cost-base report of the listing dies at read time.
    ///
    /// The reachable ordering is the payment first: with no parcels yet there
    /// is nothing for the corporate-action side to disagree with, so a payment
    /// in a currency other than the listing's is accepted — and every parcel
    /// entered afterwards is in the listing's currency
    /// (`UpsertError::CurrencyNotListings`), so this check is what catches the
    /// pair.
    #[tokio::test]
    async fn api_buy_in_another_currency_than_a_payment_on_its_listing_returns_422() {
        use crate::entities::corporate_action::{ActionKind, CorporateAction};
        let pool = test_pool().await;
        insert_test_listing(&pool).await;
        crate::entities::corporate_action::db_upsert(
            &pool,
            &CorporateAction {
                id: 1,
                listing_id: 1,
                date: ymd(2024, 5, 1),
                kind: ActionKind::ReturnOfCapital {
                    amount_per_unit: dec("0.50"),
                    currency: "USD".to_string(),
                    record_date: None,
                },
            },
        )
        .await
        .unwrap();

        let buy = |date: &str| {
            serde_json::json!({
                "trade_type": "Buy",
                "date": date,
                "listing_id": 1,
                "average_price": "10",
                "quantity": "100",
                "currency": "AUD",
                "brokerage": "0",
                "gst_on_brokerage": "0",
                "brokerage_currency": "AUD",
                "fx_rate": "1"
            })
        };
        // Acquired before the payment, so the payment reaches it: refused,
        // naming the payment and both currencies.
        let (status, detail) = put_trade_json(&pool, 1, buy("2024-01-15")).await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
        assert!(
            detail.contains("held in AUD")
                && detail.contains("2024-05-01")
                && detail.contains("recorded in USD"),
            "detail must name the payment and both currencies: {detail}"
        );
        assert!(
            db_get(&pool, 1).await.unwrap().is_none(),
            "nothing persisted"
        );
        // A parcel acquired *after* the payment is fine: it was never entitled
        // to it, so nothing ever nets the two currencies.
        let (status, _) = put_trade_json(&pool, 2, buy("2024-06-03")).await;
        assert_eq!(status, StatusCode::NO_CONTENT);
    }

    /// A trade is recorded in its listing's own currency (SCENARIOS M-08).
    /// `average_price` **is** the price of that listed security, so the two
    /// are the same money — the rule ESS statements apply to a per-share
    /// market value, inheritances to a parcel's cost base, and the DRP
    /// reinvest path to a distribution's cash. Without it a US-quoted share
    /// bought "in AUD" divides an AUD price by a USD rate in every cost-base
    /// report, while its closing prices — collected from the exchange in the
    /// listing's currency — value it against a parcel costed in another.
    #[tokio::test]
    async fn api_trade_currency_must_be_the_listings() {
        let pool = test_pool().await;
        insert_usd_listing(&pool).await;
        let buy = |currency: &str| {
            serde_json::json!({
                "trade_type": "Buy", "date": "2024-01-16", "listing_id": 1,
                "average_price": "100", "quantity": "10", "currency": currency,
                "brokerage": "0", "gst_on_brokerage": "0",
                "brokerage_currency": currency, "fx_rate": "1",
            })
        };
        let (status, detail) = put_trade_json(&pool, 1, buy("AUD")).await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
        assert!(
            detail.contains("recorded in AUD") && detail.contains("quoted in USD"),
            "the refusal names both currencies: {detail}"
        );
        assert!(db_get(&pool, 1).await.unwrap().is_none(), "nothing written");

        // In the listing's own currency it goes through.
        let (status, _) = put_trade_json(&pool, 1, buy("USD")).await;
        assert_eq!(status, StatusCode::NO_CONTENT);

        // A Sell of the same parcel meets the same rule on its own path.
        let sell = |currency: &str| {
            serde_json::json!({
                "date": "2024-06-03", "listing_id": 1, "average_price": "150",
                "quantity": "10", "currency": currency, "brokerage": "0",
                "gst_on_brokerage": "0", "brokerage_currency": currency,
                "fx_rate": "1",
                "allocations": [{ "purchase_trade_id": 1, "quantity_allocated": "10" }],
            })
        };
        let full = ApiClient::full(&pool);
        let resp = full.put("/sells/2", &sell("AUD")).await;
        let (status, detail) = resp.status_and_body();
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
        assert!(
            detail.contains("recorded in AUD") && detail.contains("quoted in USD"),
            "the Sell refusal names both currencies: {detail}"
        );
        assert!(db_get(&pool, 2).await.unwrap().is_none(), "nothing written");
        full.put("/sells/2", &sell("USD"))
            .await
            .expect_status(StatusCode::NO_CONTENT);
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
        insert_usd_listing(&pool).await;
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
        // One listing per currency: a trade is recorded in its listing's own
        // currency, so the USD and AUD halves need a listing each.
        insert_usd_listing(&pool).await;
        test_support::listing(2)
            .ticker("VAS")
            .name("Vanguard Australian Shares ETF")
            .insert(&pool)
            .await;
        let put = |pool: SqlitePool, body: serde_json::Value| async move {
            client(&pool).put("/trades/1", &body).await
        };
        // A USD trade with a deliberate spot rate persists it.
        let resp = put(
            pool.clone(),
            serde_json::json!({
                "trade_type": "Buy", "date": "2024-01-16", "listing_id": 1,
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
                "trade_type": "Buy", "date": "2024-01-16", "listing_id": 2,
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

    // ---- SCENARIOS V-d: a parcel dated behind a whole-holding operation ----

    /// An OLD → NEW scrip-for-scrip exchange that has already run, plus the
    /// listings it needs. Returns nothing: the ids are fixed (1 = OLD, 2 = NEW,
    /// action 10, parcel 1).
    async fn exchanged_listing(pool: &SqlitePool) {
        test_support::listing(1).ticker("OLD").insert(pool).await;
        test_support::listing(2).ticker("NEW").insert(pool).await;
        test_support::buy(1, 1)
            .date(ymd(2024, 1, 2))
            .insert(pool)
            .await;
        crate::entities::corporate_action::db_upsert(
            pool,
            &crate::entities::corporate_action::CorporateAction {
                id: 10,
                listing_id: 1,
                date: ymd(2024, 6, 10),
                kind: crate::entities::corporate_action::ActionKind::ScripForScrip {
                    scrip_listing_id: 2,
                    scrip_new_units: Decimal::ONE,
                    scrip_old_units: Decimal::ONE,
                    scrip_cash_per_unit: None,
                    scrip_market_value: None,
                    scrip_cash_currency: None,
                },
            },
        )
        .await
        .unwrap();
        crate::entities::scrip_exchange::db_exchange(pool, 10)
            .await
            .unwrap();
    }

    /// A HEAD → SPIN demerger that has already run (1 = HEAD, 2 = SPIN,
    /// action 10, parcel 1).
    async fn demerged_listing(pool: &SqlitePool) {
        test_support::listing(1).ticker("HEAD").insert(pool).await;
        test_support::listing(2).ticker("SPIN").insert(pool).await;
        test_support::buy(1, 1)
            .date(ymd(2024, 1, 2))
            .insert(pool)
            .await;
        crate::entities::corporate_action::db_upsert(
            pool,
            &crate::entities::corporate_action::CorporateAction {
                id: 10,
                listing_id: 1,
                date: ymd(2024, 6, 11),
                kind: crate::entities::corporate_action::ActionKind::Demerger {
                    demerger_listing_id: 2,
                    demerger_new_units: Decimal::ONE,
                    demerger_held_units: dec("5"),
                    demerger_cost_base_pct: dec("10"),
                    demerger_close_date: None,
                    demerger_close_price: None,
                    demerger_close_sourced_from: None,
                    demerger_close_reason: None,
                },
            },
        )
        .await
        .unwrap();
        crate::entities::demerger::db_demerge(pool, 10)
            .await
            .unwrap();
    }

    /// A Buy of the listing dated `date`, id 900.
    fn back_dated_buy(listing_id: i64, date: NaiveDate) -> Trade {
        test_support::buy(900, listing_id)
            .date(date)
            .settlement(date)
            .build()
    }

    /// A scrip-for-scrip exchange consumed **every** open parcel of the
    /// original listing as at its date, and cannot reach back for one entered
    /// afterwards: those units would stay open on a security the exchange
    /// replaced, with no replacement units issued for them (SCENARIOS V-d).
    #[tokio::test]
    async fn db_a_buy_dated_before_an_executed_exchange_is_refused() {
        let pool = test_pool().await;
        exchanged_listing(&pool).await;
        let err = db_upsert(&pool, &back_dated_buy(1, ymd(2024, 2, 5)))
            .await
            .unwrap_err();
        assert!(
            matches!(err, UpsertError::BackDatedOverWholeHolding(_)),
            "expected the whole-holding refusal, got: {err:?}"
        );
    }

    /// The demerger case: the parcel would keep 100% of its cost base instead
    /// of the head company's share, and no demerged units would be issued for
    /// it.
    #[tokio::test]
    async fn db_a_buy_dated_before_an_executed_demerger_is_refused() {
        let pool = test_pool().await;
        demerged_listing(&pool).await;
        let err = db_upsert(&pool, &back_dated_buy(1, ymd(2024, 3, 5)))
            .await
            .unwrap_err();
        assert!(
            matches!(err, UpsertError::BackDatedOverWholeHolding(_)),
            "expected the whole-holding refusal, got: {err:?}"
        );
    }

    /// The worthless-shares case: 40 units left open on a company already
    /// written off, whose capital loss is never recognised.
    #[tokio::test]
    async fn db_a_buy_dated_before_an_executed_recognise_is_refused() {
        let pool = test_pool().await;
        test_support::recognised_worthless_listing(
            &pool,
            5,
            "DEAD",
            ymd(2024, 1, 2),
            90,
            ymd(2024, 6, 13),
        )
        .await;
        let err = db_upsert(&pool, &back_dated_buy(5, ymd(2024, 3, 5)))
            .await
            .unwrap_err();
        assert!(
            matches!(err, UpsertError::BackDatedOverWholeHolding(_)),
            "expected the whole-holding refusal, got: {err:?}"
        );
    }

    /// The rejection is a `422` whose body names the operation, its date, and
    /// the delete-enter-redo recovery — the same shape the sibling refusal on
    /// a back-dated corporate action gives, so the web UI can show it as an
    /// instruction rather than an error code.
    #[tokio::test]
    async fn api_a_back_dated_buy_is_422_naming_the_operation_and_the_recovery() {
        let pool = test_pool().await;
        exchanged_listing(&pool).await;
        let body = serde_json::json!({
            "trade_type": "Buy",
            "date": "2024-02-05",
            "listing_id": 1,
            "average_price": "10.0",
            "quantity": "50.0",
            "currency": "AUD",
            "brokerage": "0.0",
            "gst_on_brokerage": "0.0",
            "brokerage_currency": "AUD",
            "fx_rate": "1.0"
        });
        let response = client(&pool).put("/trades/900", &body).await;
        let (status, detail) = response.status_and_body();
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
        assert!(detail.contains("scrip-for-scrip exchange"), "{detail}");
        assert!(detail.contains("corporate action #10"), "{detail}");
        assert!(detail.contains("2024-06-10"), "{detail}");
        assert!(
            detail.contains("Delete that operation, enter this parcel, then run it again"),
            "{detail}"
        );
        // Nothing was written.
        let exists: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM trades WHERE id = 900)")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert!(!exists);
    }

    /// A parcel dated **after** the operation is ordinary post-event activity
    /// and lands normally — the head listing of a demerger keeps trading.
    #[tokio::test]
    async fn db_a_buy_dated_after_the_operation_is_accepted() {
        let pool = test_pool().await;
        demerged_listing(&pool).await;
        db_upsert(&pool, &back_dated_buy(1, ymd(2024, 6, 12)))
            .await
            .unwrap();
    }

    /// Editing a source parcel the operation **did** consume stays allowed:
    /// that is precisely the state `reports::rollover_consistency` documents
    /// and surfaces, and refusing the edit would make it unfixable while
    /// fixing nothing. Only a write that *newly* strands units is refused.
    #[tokio::test]
    async fn db_editing_a_consumed_source_parcel_is_still_allowed() {
        let pool = test_pool().await;
        exchanged_listing(&pool).await;
        let mut parcel = db_get(&pool, 1).await.unwrap().unwrap();
        parcel.average_price = dec("12");
        db_upsert(&pool, &parcel).await.unwrap();
        assert_eq!(
            db_get(&pool, 1).await.unwrap().unwrap().average_price,
            dec("12")
        );
    }

    // -----------------------------------------------------------------------
    // A money/quantity figure sent as a JSON number (SCENARIOS W-a)
    // -----------------------------------------------------------------------

    /// A trade body with `quantity` and `average_price` written out verbatim,
    /// so a test can send an unquoted JSON number the way a bulk import would.
    fn trade_body_raw(quantity: &str, average_price: &str) -> String {
        format!(
            r#"{{"trade_type":"Buy","date":"2024-01-15","listing_id":1,
                 "average_price":{average_price},"quantity":{quantity},"currency":"AUD",
                 "brokerage":"0","brokerage_currency":"AUD","fx_rate":"1"}}"#
        )
    }

    /// `{"quantity": 100000000.00000001}` used to be accepted `204` and stored
    /// as `100000000` — `serde_json` hands a JSON number over as an `f64`,
    /// which keeps ~15 significant digits, so a satoshi went missing under a
    /// success (SCENARIOS W-a). It is now refused, naming the field.
    #[tokio::test]
    async fn api_a_quantity_sent_as_a_json_number_is_refused_naming_the_field() {
        let pool = test_pool().await;
        insert_test_listing(&pool).await;

        for quantity in ["100000000.00000001", "99999999.87654321", "10", "10.0"] {
            let response = client(&pool)
                .put_raw("/trades/1", &trade_body_raw(quantity, "\"1\""))
                .await;
            let (status, body) = response.status_and_body();
            assert_eq!(
                status,
                StatusCode::UNPROCESSABLE_ENTITY,
                "quantity {quantity} as a JSON number: {body}"
            );
            assert!(body.contains("quantity"), "the field is not named: {body}");
            assert!(
                body.contains("as a decimal string"),
                "the remedy is not stated: {body}"
            );
        }

        // An unquoted `average_price` is refused the same way, naming itself.
        let response = client(&pool)
            .put_raw(
                "/trades/1",
                &trade_body_raw("\"1\"", "1234567890123456789.12"),
            )
            .await;
        let (status, body) = response.status_and_body();
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
        assert!(body.contains("average_price"), "{body}");

        // Nothing was written by any of the refusals.
        assert!(db_get(&pool, 1).await.unwrap().is_none());
    }

    /// The control the finding was bounded with: the very same figures sent as
    /// **strings** are accepted and stored to the digit. The refusal above is
    /// therefore about the JSON encoding, not about the values.
    #[tokio::test]
    async fn api_the_same_quantity_sent_as_a_string_is_stored_exactly() {
        let pool = test_pool().await;
        insert_test_listing(&pool).await;

        for (id, quantity) in [(1, "100000000.00000001"), (2, "99999999.87654321")] {
            let path = format!("/trades/{id}");
            let sent = trade_body_raw(&format!("\"{quantity}\""), "\"1\"");
            let response = client(&pool).put_raw(&path, &sent).await;
            let (status, body) = response.status_and_body();
            assert_eq!(status, StatusCode::NO_CONTENT, "{body}");

            // Straight off the column, so nothing on the read path can round it.
            let stored: String = sqlx::query_scalar("SELECT quantity FROM trades WHERE id = ?")
                .bind(id)
                .fetch_one(&pool)
                .await
                .unwrap();
            assert_eq!(stored, quantity, "stored quantity lost a digit");
        }
    }
}
