//! Buy-trade edit/delete integrity, symmetric with the Sell-side invariants:
//! a parcel an allocation, AMIT adjustment or reinvestment depends on cannot
//! be deleted, shrunk, or moved out from under it.

use super::*;
use crate::entities::corporate_action::FractionRule;
use crate::test_support::holding_account_row;

/// Link an AMIT adjustment covering `qty` units of trade `trade_id`
/// (listing 1), creating the AMMA statement it hangs off.
async fn insert_amit_adjustment_covering(pool: &SqlitePool, trade_id: TradeId, qty: Decimal) {
    test_support::amma(1, 1)
        .units(qty)
        .cost_base_adjustment(dec("0.05"))
        .insert(pool)
        .await;
    test_support::amit_adjustment(pool, 1, 1, trade_id.0, qty).await;
}

/// A second holding account, so a parcel has somewhere to be moved to.
async fn insert_second_account(pool: &SqlitePool) {
    use crate::entities::holding_account::{self};
    holding_account::db_upsert(pool, &holding_account_row(2, "ICE Employee Plan"))
        .await
        .unwrap();
}

#[tokio::test]
async fn db_delete_buy_consumed_by_allocation_is_refused() {
    let pool = test_pool().await;
    insert_test_listing(&pool).await;
    db_upsert(&pool, &buy_trade()).await.unwrap();
    insert_sell_consuming(&pool, TradeId(2), TradeId(1), Decimal::from(5)).await;

    assert_eq!(
        db_delete(&pool, TradeId(1)).await.unwrap(),
        DeleteOutcome::Referenced
    );
    assert!(
        db_get(&pool, TradeId(1)).await.unwrap().is_some(),
        "consumed buy must remain"
    );
}

#[tokio::test]
async fn db_delete_buy_covered_by_amit_adjustment_is_refused() {
    let pool = test_pool().await;
    insert_test_listing(&pool).await;
    db_upsert(&pool, &buy_trade()).await.unwrap();
    insert_amit_adjustment_covering(&pool, TradeId(1), Decimal::from(10)).await;

    assert_eq!(
        db_delete(&pool, TradeId(1)).await.unwrap(),
        DeleteOutcome::Referenced
    );
    assert!(
        db_get(&pool, TradeId(1)).await.unwrap().is_some(),
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
        db_delete(&pool, TradeId(1)).await.unwrap(),
        DeleteOutcome::Referenced
    );
    assert!(
        db_get(&pool, TradeId(1)).await.unwrap().is_some(),
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
    let kept = db_get(&pool, TradeId(1)).await.unwrap().unwrap();
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
    insert_sell_consuming(&pool, TradeId(2), TradeId(1), Decimal::from(5)).await;

    // Shrinking below the 5 already allocated out is refused…
    let mut shrunk = buy_trade();
    shrunk.quantity = Decimal::from(4);
    let err = db_upsert(&pool, &shrunk).await.unwrap_err();
    assert!(
        matches!(err, UpsertError::QuantityBelowAllocated),
        "expected QuantityBelowAllocated, got: {err:?}"
    );
    assert_eq!(
        db_get(&pool, TradeId(1)).await.unwrap().unwrap().quantity,
        Decimal::from(10)
    );

    // …but shrinking exactly to the allocated quantity is fine.
    let mut exact = buy_trade();
    exact.quantity = Decimal::from(5);
    db_upsert(&pool, &exact).await.unwrap();
    assert_eq!(
        db_get(&pool, TradeId(1)).await.unwrap().unwrap().quantity,
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
    corporate_action::db_upsert(
        &pool,
        &CorporateAction {
            id: 1,
            listing_id: ListingId(1),
            date: NaiveDate::from_ymd_opt(2024, 3, 1).unwrap(),
            kind: ActionKind::ShareSplit {
                fraction_rule: FractionRule::Exact,
                split_new_units: Decimal::from(2),
                split_old_units: Decimal::ONE,
            },
        },
    )
    .await
    .unwrap();
    // Sell 10 post-split units (= 5 as-acquired) on 2024-06-01.
    insert_sell_consuming(&pool, TradeId(2), TradeId(1), Decimal::from(10)).await;

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
        db_get(&pool, TradeId(1)).await.unwrap().unwrap().quantity,
        Decimal::from(5)
    );
}

#[tokio::test]
async fn db_shrink_buy_below_amit_adjustment_quantity_is_refused() {
    let pool = test_pool().await;
    insert_test_listing(&pool).await;
    db_upsert(&pool, &buy_trade()).await.unwrap(); // qty 10
    insert_amit_adjustment_covering(&pool, TradeId(1), Decimal::from(8)).await;

    // Shrinking below the adjustment's 8 covered units is refused…
    let mut shrunk = buy_trade();
    shrunk.quantity = Decimal::from(7);
    let err = db_upsert(&pool, &shrunk).await.unwrap_err();
    assert!(
        matches!(err, UpsertError::QuantityBelowAmitAdjustment),
        "expected QuantityBelowAmitAdjustment, got: {err:?}"
    );
    assert_eq!(
        db_get(&pool, TradeId(1)).await.unwrap().unwrap().quantity,
        Decimal::from(10)
    );

    // …but shrinking exactly to the covered quantity is fine.
    let mut exact = buy_trade();
    exact.quantity = Decimal::from(8);
    db_upsert(&pool, &exact).await.unwrap();
    assert_eq!(
        db_get(&pool, TradeId(1)).await.unwrap().unwrap().quantity,
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
    insert_sell_consuming(&pool, TradeId(2), TradeId(1), Decimal::from(5)).await;

    let mut moved = buy_trade();
    moved.listing_id = ListingId(2);
    let err = db_upsert(&pool, &moved).await.unwrap_err();
    assert!(
        matches!(err, UpsertError::ListingChangeReferenced),
        "expected ListingChangeReferenced, got: {err:?}"
    );
    assert_eq!(
        db_get(&pool, TradeId(1)).await.unwrap().unwrap().listing_id,
        1
    );
}

/// The same freeze applies while an AMIT adjustment covers the parcel —
/// and lifts once nothing references it.
#[tokio::test]
async fn db_listing_change_under_amit_adjustment_is_refused_until_unlinked() {
    let pool = test_pool().await;
    insert_test_listing(&pool).await;
    test_support::listing(2).ticker("VGS").insert(&pool).await;
    db_upsert(&pool, &buy_trade()).await.unwrap(); // listing 1
    insert_amit_adjustment_covering(&pool, TradeId(1), Decimal::from(8)).await;

    let mut moved = buy_trade();
    moved.listing_id = ListingId(2);
    let err = db_upsert(&pool, &moved).await.unwrap_err();
    assert!(
        matches!(err, UpsertError::ListingChangeReferenced),
        "expected ListingChangeReferenced, got: {err:?}"
    );
    assert_eq!(
        db_get(&pool, TradeId(1)).await.unwrap().unwrap().listing_id,
        1
    );

    // With the adjustment removed the listing edits freely again.
    sqlx::query("DELETE FROM amit_adjustments WHERE trade_id = 1")
        .execute(&pool)
        .await
        .unwrap();
    db_upsert(&pool, &moved).await.unwrap();
    assert_eq!(
        db_get(&pool, TradeId(1)).await.unwrap().unwrap().listing_id,
        2
    );
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
    insert_sell_consuming(&pool, TradeId(2), TradeId(1), Decimal::from(5)).await; // sale 2024-06-01

    let mut moved = buy_trade();
    moved.date = ymd(2024, 7, 1);
    moved.settlement_date = ymd(2024, 7, 3);
    let err = db_upsert(&pool, &moved).await.unwrap_err();
    assert!(
        matches!(err, UpsertError::DateAfterAllocatedSale),
        "expected DateAfterAllocatedSale, got: {err:?}"
    );
    assert_eq!(
        db_get(&pool, TradeId(1)).await.unwrap().unwrap().date,
        ymd(2024, 1, 15)
    );

    // Up to the sale date itself is fine (a same-day parcel is a valid
    // allocation on the Sell side too), as is moving earlier.
    let mut same_day = buy_trade();
    same_day.date = ymd(2024, 6, 3);
    same_day.settlement_date = ymd(2024, 6, 3);
    db_upsert(&pool, &same_day).await.unwrap();
    assert_eq!(
        db_get(&pool, TradeId(1)).await.unwrap().unwrap().date,
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
        db_get(&pool, TradeId(1)).await.unwrap().unwrap().date,
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
    insert_sell_consuming(&pool, TradeId(2), TradeId(1), Decimal::from(5)).await;

    let mut moved = buy_trade();
    moved.holding_account_id = HoldingAccountId(2);
    let err = db_upsert(&pool, &moved).await.unwrap_err();
    assert!(
        matches!(err, UpsertError::AccountChangeReferenced),
        "expected AccountChangeReferenced, got: {err:?}"
    );
    assert_eq!(
        db_get(&pool, TradeId(1))
            .await
            .unwrap()
            .unwrap()
            .holding_account_id,
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
    insert_amit_adjustment_covering(&pool, TradeId(1), Decimal::from(8)).await;

    let mut moved = buy_trade();
    moved.holding_account_id = HoldingAccountId(2);
    let err = db_upsert(&pool, &moved).await.unwrap_err();
    assert!(
        matches!(err, UpsertError::AccountChangeReferenced),
        "expected AccountChangeReferenced, got: {err:?}"
    );
    assert_eq!(
        db_get(&pool, TradeId(1))
            .await
            .unwrap()
            .unwrap()
            .holding_account_id,
        1
    );

    // With the adjustment removed the account moves freely again.
    sqlx::query("DELETE FROM amit_adjustments WHERE trade_id = 1")
        .execute(&pool)
        .await
        .unwrap();
    db_upsert(&pool, &moved).await.unwrap();
    assert_eq!(
        db_get(&pool, TradeId(1))
            .await
            .unwrap()
            .unwrap()
            .holding_account_id,
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
    insert_sell_consuming(&pool, TradeId(2), TradeId(1), Decimal::from(5)).await;

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

    let unchanged = db_get(&pool, TradeId(1)).await.unwrap().unwrap();
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
        db_get(&pool, TradeId(1)).await.unwrap().unwrap().quantity,
        Decimal::ONE
    );

    assert_eq!(
        db_delete(&pool, TradeId(1)).await.unwrap(),
        DeleteOutcome::Deleted
    );
    assert!(db_get(&pool, TradeId(1)).await.unwrap().is_none());
}
