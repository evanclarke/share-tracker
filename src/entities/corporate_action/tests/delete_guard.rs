//! The delete-time guard on the three types that create no trades, and
//! exact re-basing across a consolidation or a fine-grained payment.

use super::*;

// Delete-time guard: the three types that create no trades
//
// `ShareSplit`, `BonusIssue`, and `ReturnOfCapital` re-base or reduce
// parcels at read time, so — unlike the five types frozen by their trade
// group's foreign key — nothing stops a delete from restating figures the
// reports already produced (SCENARIOS A-06, A-20, A-21).

/// SCENARIOS E-09: a consolidation whose ratio does not divide the
/// holding evenly, followed by a sale of the whole of it. The reported
/// remaining quantity is the exact re-based figure (no rounding — company
/// rounding and cash-in-lieu are not modelled), and selling exactly that
/// is accepted and consumes the parcel to nothing: the re-basing and its
/// inverse agree, so no dust is left behind that could never be sold.
#[tokio::test]
async fn db_a_consolidation_that_does_not_divide_still_sells_out_exactly() {
    let pool = test_pool().await;
    insert_listing(&pool, 1, "CON").await;
    insert_parcel(&pool).await; // 100 units @ $10 on 2023-01-10
    db_upsert(
        &pool,
        &CorporateAction {
            id: 1,
            listing_id: ListingId(1),
            date: d(2024, 3, 1),
            kind: ActionKind::ShareSplit {
                split_new_units: "3".parse().unwrap(),
                split_old_units: "7".parse().unwrap(),
            },
        },
    )
    .await
    .unwrap();

    let parcels = reports::open_parcels::db_open_parcels(&pool, None)
        .await
        .unwrap();
    let remaining = parcels[0].remaining_quantity;
    assert_eq!(
        remaining,
        "42.857142857142857142857142857".parse::<Decimal>().unwrap()
    );

    insert_sell(&pool, d(2024, 4, 2), &remaining.to_string())
        .await
        .unwrap();
    assert!(
        reports::open_parcels::db_open_parcels(&pool, None)
            .await
            .unwrap()
            .is_empty()
    );
    // The whole $1,000 cost base is in the disposal, undiminished.
    let gains = realised_gains::db_realised_gains(&pool).await.unwrap();
    assert_eq!(gains.len(), 1);
    assert_eq!(gains[0].cost_base.round_dp(6), "1000".parse().unwrap());
}

/// SCENARIOS E-02: a per-unit payment carried to six decimal places (the
/// scale a registry states a small return of capital at). It survives the
/// round trip and the reduction is exact — `Decimal` all the way, never a
/// float and never rounded to cents on the way in.
#[tokio::test]
async fn db_a_six_decimal_place_payment_reduces_exactly() {
    let pool = test_pool().await;
    insert_listing(&pool, 1, "SIX").await;
    test_support::buy(1, 1)
        .date(d(2023, 1, 10))
        .qty("3333".parse().unwrap())
        .price(Decimal::ONE)
        .insert(&pool)
        .await;
    db_upsert(&pool, &roc(1, ListingId(1), d(2024, 3, 1), "0.123456"))
        .await
        .unwrap();

    assert_eq!(
        db_get(&pool, 1).await.unwrap().unwrap(),
        roc(1, ListingId(1), d(2024, 3, 1), "0.123456")
    );
    let parcels = reports::open_parcels::db_open_parcels(&pool, None)
        .await
        .unwrap();
    // 3,333 × 0.123456 = 411.478848, to the last digit.
    assert_eq!(
        parcels[0].return_of_capital_reduction,
        "411.478848".parse::<Decimal>().unwrap()
    );
    assert_eq!(
        parcels[0].remaining_cost_base,
        "2921.521152".parse::<Decimal>().unwrap()
    );
}

/// A-20: the split is what makes the 200-unit Sell fit the 100-unit
/// parcel, so deleting it would leave allocations the Sell's own write
/// path refuses. The delete is refused while that trade stands, and
/// allowed again once it is gone.
#[tokio::test]
async fn db_deleting_a_split_is_refused_while_a_later_trade_is_on_the_post_split_basis() {
    let pool = test_pool().await;
    insert_listing(&pool, 1, "SPL").await;
    insert_parcel(&pool).await;
    db_upsert(&pool, &split(10, ListingId(1), d(2023, 3, 1), "2", "1"))
        .await
        .unwrap();
    insert_sell(&pool, d(2023, 6, 1), "200").await.unwrap();

    let err = db_delete(&pool, 10).await.unwrap_err();
    assert!(matches!(err, DeleteError::RebasedTrades), "{err:?}");
    assert!(db_get(&pool, 10).await.unwrap().is_some(), "still there");

    // With the Sell gone the split deletes freely — and the identical Sell
    // is then refused, which is exactly the state the guard keeps the
    // delete from leaving behind.
    assert!(matches!(
        sell::db_delete_sell(&pool, TradeId(2)).await.unwrap(),
        sell::DeleteOutcome::Deleted
    ));
    assert!(db_delete(&pool, 10).await.unwrap());
    assert!(matches!(
        insert_sell(&pool, d(2023, 6, 1), "200").await,
        Err(SellError::PurchaseQuantityExceeded)
    ));
}

/// The same guard for a `BonusIssue` (it folds into the split-event stream
/// as its equivalent re-base, and deleting it restates quantities the same
/// way), and a trade dated *before* the action does not block the delete.
#[tokio::test]
async fn db_deleting_a_bonus_issue_is_refused_the_same_way() {
    let pool = test_pool().await;
    insert_listing(&pool, 1, "BON").await;
    insert_parcel(&pool).await;
    db_upsert(&pool, &bonus(10, ListingId(1), d(2023, 3, 1), "1", "1"))
        .await
        .unwrap();

    // The parcel predates the issue: nothing is recorded on the post-issue
    // basis yet, so the delete stands.
    assert!(db_delete(&pool, 10).await.unwrap());

    db_upsert(&pool, &bonus(10, ListingId(1), d(2023, 3, 1), "1", "1"))
        .await
        .unwrap();
    insert_sell(&pool, d(2023, 6, 1), "200").await.unwrap();
    let err = db_delete(&pool, 10).await.unwrap_err();
    assert!(matches!(err, DeleteError::RebasedTrades), "{err:?}");
}

/// A-21: deleting a return of capital breaks no quantity invariant, but it
/// silently restores the cost base it reduced — and drops any CGT event G1
/// excess gain already reported for the payment's year. Refused while a
/// parcel it reduced exists; a payment before every acquisition (which
/// reduces nothing) still deletes.
#[tokio::test]
async fn db_deleting_a_return_of_capital_is_refused_while_it_reduced_a_parcel() {
    let pool = test_pool().await;
    insert_listing(&pool, 1, "RAP").await;
    insert_parcel(&pool).await;

    db_upsert(&pool, &roc(10, ListingId(1), d(2023, 6, 1), "0.50"))
        .await
        .unwrap();
    let err = db_delete(&pool, 10).await.unwrap_err();
    assert!(matches!(err, DeleteError::ReducedParcels), "{err:?}");

    // Dated before the parcel was acquired, it reduces nothing.
    db_upsert(&pool, &roc(11, ListingId(1), d(2022, 12, 1), "0.50"))
        .await
        .unwrap();
    assert!(db_delete(&pool, 11).await.unwrap());

    // Nor does a payment the parcel was never entitled to: its record date
    // (not the payment date) is what decides which parcels it reached, and
    // the parcel — bought 10 Jan, settled 12 Jan — was not yet registered
    // on 11 Jan, so the guard bounds the acquisitions it looks for by the
    // same settlement test the cost-base pipeline does.
    db_upsert(
        &pool,
        &roc_with_record(12, ListingId(1), d(2023, 6, 1), "0.50", d(2023, 1, 11)),
    )
    .await
    .unwrap();
    assert!(db_delete(&pool, 12).await.unwrap());

    // On its settlement date the parcel *is* entitled, and the guard holds
    // again.
    db_upsert(
        &pool,
        &roc_with_record(13, ListingId(1), d(2023, 6, 1), "0.50", d(2023, 1, 12)),
    )
    .await
    .unwrap();
    let err = db_delete(&pool, 13).await.unwrap_err();
    assert!(matches!(err, DeleteError::ReducedParcels), "{err:?}");
}

/// The five action types whose trades carry a `*_action_id` back-reference
/// keep the foreign key as their guard: with no such trade recorded, the
/// delete is not blocked by the new check.
#[tokio::test]
async fn db_deleting_an_unapplied_trade_creating_action_is_unaffected() {
    let pool = test_pool().await;
    insert_listing(&pool, 1, "RIT").await;
    insert_parcel(&pool).await;
    db_upsert(
        &pool,
        &rights(10, ListingId(1), d(2023, 6, 1), "1", "4", "2.00"),
    )
    .await
    .unwrap();
    assert!(db_delete(&pool, 10).await.unwrap());
}

#[tokio::test]
async fn api_deleting_a_depended_on_action_returns_422_naming_the_dependency() {
    let pool = test_pool().await;
    insert_listing(&pool, 1, "SPL").await;
    insert_parcel(&pool).await;
    db_upsert(&pool, &split(10, ListingId(1), d(2023, 3, 1), "2", "1"))
        .await
        .unwrap();
    insert_sell(&pool, d(2023, 6, 1), "200").await.unwrap();

    let resp = client(&pool).delete("/corporate_actions/10").await;
    let (status, body) = resp.status_and_body();
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert!(body.contains("trade dated on or after"), "{body}");

    db_upsert(&pool, &roc(11, ListingId(1), d(2023, 6, 1), "0.50"))
        .await
        .unwrap();
    let resp = client(&pool).delete("/corporate_actions/11").await;
    let (status, body) = resp.status_and_body();
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert!(body.contains("reduced the cost base"), "{body}");
}
