//! The write-time state check: an edit stays possible, but not into an
//! over-consumed parcel.

use super::*;
use crate::test_support::holding_account_row;

// Write-time state check: an edit stays possible, but not into an
// over-consumed parcel
//
// `PUT` is deliberately not frozen the way `DELETE` is — a mis-keyed
// ratio, date, or amount has to stay correctable — so the state the write
// leaves behind is checked instead: allocations must still fit the parcels
// they draw on, the same invariant the delete guard upholds.

/// The stored terms of the split entered by [`split`], for asserting a
/// refused write changed nothing.
async fn stored_split_terms(pool: &SqlitePool, id: i64) -> (NaiveDate, Decimal, Decimal) {
    let stored = db_get(pool, id).await.unwrap().unwrap();
    let ActionKind::ShareSplit {
        split_new_units,
        split_old_units,
    } = stored.kind
    else {
        panic!("not a share split");
    };
    (stored.date, split_new_units, split_old_units)
}

/// A-20 reached one verb over: the 2:1 split is what makes the 200-unit
/// Sell fit the 100-unit parcel, so re-terming it 1:1 — or moving it past
/// the Sell — would leave the allocations the Sell's own write path
/// refuses. Both are rejected, and the stored terms are untouched.
#[tokio::test]
async fn db_editing_a_split_into_an_over_consumed_parcel_is_refused() {
    let pool = test_pool().await;
    insert_listing(&pool, 1, "SPL").await;
    insert_parcel(&pool).await;
    db_upsert(&pool, &split(10, ListingId(1), d(2023, 3, 1), "2", "1"))
        .await
        .unwrap();
    insert_sell(&pool, d(2023, 6, 1), "200").await.unwrap();

    let err = db_upsert(&pool, &split(10, ListingId(1), d(2023, 3, 1), "1", "1"))
        .await
        .unwrap_err();
    assert!(
        matches!(err, WriteError::AllocationsExceedParcel),
        "{err:?}"
    );

    let err = db_upsert(&pool, &split(10, ListingId(1), d(2023, 7, 1), "2", "1"))
        .await
        .unwrap_err();
    assert!(
        matches!(err, WriteError::AllocationsExceedParcel),
        "{err:?}"
    );

    assert_eq!(
        stored_split_terms(&pool, 10).await,
        (d(2023, 3, 1), Decimal::from(2), Decimal::ONE),
        "a refused write must not have been persisted"
    );
}

/// The correction path the guard is deliberately not allowed to close: a
/// re-term that keeps every allocation covered still lands — a wider ratio
/// (3:1 leaves the parcel worth 300 post-split units), and a date move that
/// stays before the Sell.
#[tokio::test]
async fn db_editing_a_split_that_breaks_nothing_still_lands() {
    let pool = test_pool().await;
    insert_listing(&pool, 1, "SPL").await;
    insert_parcel(&pool).await;
    db_upsert(&pool, &split(10, ListingId(1), d(2023, 3, 1), "2", "1"))
        .await
        .unwrap();
    insert_sell(&pool, d(2023, 6, 1), "200").await.unwrap();

    db_upsert(&pool, &split(10, ListingId(1), d(2023, 2, 1), "3", "1"))
        .await
        .unwrap();
    assert_eq!(
        stored_split_terms(&pool, 10).await,
        (d(2023, 2, 1), Decimal::from(3), Decimal::ONE)
    );

    // A return of capital moves cost base, not quantities, so correcting
    // its per-unit amount over the same holding is unaffected by the check.
    db_upsert(&pool, &roc(11, ListingId(1), d(2023, 6, 1), "0.50"))
        .await
        .unwrap();
    db_upsert(&pool, &roc(11, ListingId(1), d(2023, 6, 1), "0.75"))
        .await
        .unwrap();
}

/// The check is over the written state, not the fields that changed, so it
/// equally covers a *new* consolidation recorded over sales already
/// allocated in the pre-consolidation basis — the same over-consumption
/// reached by inserting rather than editing.
#[tokio::test]
async fn db_recording_a_consolidation_over_existing_sales_is_refused() {
    let pool = test_pool().await;
    insert_listing(&pool, 1, "CON").await;
    insert_parcel(&pool).await;
    insert_sell(&pool, d(2023, 6, 1), "100").await.unwrap();

    // 1-for-2: the 100 units sold are 200 as-acquired units of a 100-unit
    // parcel.
    let err = db_upsert(&pool, &split(10, ListingId(1), d(2023, 3, 1), "1", "2"))
        .await
        .unwrap_err();
    assert!(
        matches!(err, WriteError::AllocationsExceedParcel),
        "{err:?}"
    );
    assert!(db_get(&pool, 10).await.unwrap().is_none(), "not persisted");

    // Dated after the Sell it re-bases nothing already recorded, so it
    // stands.
    db_upsert(&pool, &split(10, ListingId(1), d(2023, 8, 1), "1", "2"))
        .await
        .unwrap();
}

/// Moving a re-basing action to another listing has to leave *both*
/// listings legal: the one it lands on, and the one whose split stream it
/// is removed from.
#[tokio::test]
async fn db_moving_a_split_off_a_listing_that_needs_it_is_refused() {
    let pool = test_pool().await;
    insert_listing(&pool, 1, "SPL").await;
    insert_listing(&pool, 2, "OTH").await;
    insert_parcel(&pool).await;
    db_upsert(&pool, &split(10, ListingId(1), d(2023, 3, 1), "2", "1"))
        .await
        .unwrap();
    insert_sell(&pool, d(2023, 6, 1), "200").await.unwrap();

    let err = db_upsert(&pool, &split(10, ListingId(2), d(2023, 3, 1), "2", "1"))
        .await
        .unwrap_err();
    assert!(
        matches!(err, WriteError::AllocationsExceedParcel),
        "{err:?}"
    );
    assert_eq!(
        db_get(&pool, 10).await.unwrap().unwrap().listing_id,
        1,
        "a refused write must not have been persisted"
    );
}

#[tokio::test]
async fn api_editing_a_split_into_an_over_consumed_parcel_returns_422() {
    let pool = test_pool().await;
    insert_listing(&pool, 1, "SPL").await;
    insert_parcel(&pool).await;
    db_upsert(&pool, &split(10, ListingId(1), d(2023, 3, 1), "2", "1"))
        .await
        .unwrap();
    insert_sell(&pool, d(2023, 6, 1), "200").await.unwrap();

    let resp = client(&pool)
        .put(
            "/corporate_actions/10",
            &serde_json::json!({
                "action_type": "ShareSplit",
                "listing_id": 1,
                "date": "2023-03-01",
                "split_new_units": "1",
                "split_old_units": "1",
            }),
        )
        .await;
    let (status, body) = resp.status_and_body();
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert!(
        body.contains("more units than the parcel it draws on holds"),
        "{body}"
    );

    // The same edit against a listing whose allocations still fit lands.
    let resp = client(&pool)
        .put(
            "/corporate_actions/10",
            &serde_json::json!({
                "action_type": "ShareSplit",
                "listing_id": 1,
                "date": "2023-03-01",
                "split_new_units": "4",
                "split_old_units": "1",
            }),
        )
        .await;
    assert_eq!(resp.status, StatusCode::NO_CONTENT);
}

/// SCENARIOS N-06, N-07: a rollover stores the cost base and quantity its
/// replacement parcels carry, so an event back-dated over one restates only
/// the parcels it consumed. Entering the *same* return of capital before the
/// transfer reported a $400 cost base; entering it afterwards reported $500
/// — a $100 understated gain from nothing but the order of entry — and a
/// back-dated split left the source parcel open again beside the untouched
/// replacement, reporting 200 units and $750 for a $500 holding of 200. Both
/// are refused now, naming the operation and the delete-enter-redo recovery.
#[tokio::test]
async fn events_back_dated_over_a_rollover_are_refused_naming_it() {
    let pool = test_pool().await;
    insert_listing(&pool, 1, "AAA").await;
    holding_account::db_upsert(&pool, &holding_account_row(2, "Broker"))
        .await
        .unwrap();
    test_support::buy(1, 1)
        .date(d(2023, 1, 10))
        .qty(Decimal::from(100))
        .price(Decimal::from(5))
        .insert(&pool)
        .await;
    transfer::db_transfer(
        &pool,
        1,
        &TransferBody {
            listing_id: ListingId(1),
            date: d(2023, 8, 1),
            from_account_id: HoldingAccountId(1),
            to_account_id: HoldingAccountId(2),
            allocations: vec![AllocationInput {
                purchase_trade_id: TradeId(1),
                quantity_allocated: Decimal::from(100),
            }],
            fee_allocations: Vec::new(),
            fee_market_price: None,
            fee_fx_rate: None,
        },
    )
    .await
    .unwrap();

    // A payment before the move, a payment on the day of it, and a
    // back-dated split — each restates parcels the transfer consumed.
    for action in [
        roc(10, ListingId(1), d(2023, 5, 1), "1"),
        roc(11, ListingId(1), d(2023, 8, 1), "1"),
        split(12, ListingId(1), d(2023, 5, 1), "2", "1"),
        bonus(13, ListingId(1), d(2023, 5, 1), "1", "10"),
    ] {
        let err = db_upsert(&pool, &action).await.expect_err("refused");
        assert!(
            matches!(err, WriteError::BackDatedOverRollover { .. }),
            "{action:?} → {err:?}"
        );
    }

    // The 422 the web UI shows names the operation to redo and the recovery.
    let resp = client(&pool)
        .put(
            "/corporate_actions/10",
            &serde_json::json!({
                "action_type": "ReturnOfCapital",
                "listing_id": 1,
                "date": "2023-05-01",
                "amount_per_unit": "1",
                "currency": "AUD",
            }),
        )
        .await;
    assert_eq!(resp.status, StatusCode::UNPROCESSABLE_ENTITY);
    let detail = resp.text().to_owned();
    assert!(
        detail.contains("holding-account transfer #1 on 2023-08-01"),
        "detail: {detail}"
    );
    assert!(detail.contains("Delete that operation"), "detail: {detail}");

    // After the move is fine — the replacement parcel receives it — and so
    // is a rights issue, which creates its own trades rather than restating
    // anything.
    db_upsert(&pool, &roc(14, ListingId(1), d(2023, 8, 2), "1"))
        .await
        .unwrap();

    // And nothing of another listing is affected.
    insert_listing(&pool, 2, "BBB").await;
    db_upsert(&pool, &roc(15, ListingId(2), d(2023, 5, 1), "1"))
        .await
        .unwrap();
}

#[tokio::test]
async fn api_get_and_delete_missing_return_404() {
    let pool = test_pool().await;
    let resp = client(&pool).get("/corporate_actions/999").await;
    assert_eq!(resp.status, StatusCode::NOT_FOUND);

    let resp = client(&pool).delete("/corporate_actions/999").await;
    assert_eq!(resp.status, StatusCode::NOT_FOUND);
}
