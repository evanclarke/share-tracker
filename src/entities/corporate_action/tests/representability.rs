//! A replacement quantity no `Decimal` can hold, from the re-basing actions.

use super::*;

/// A `ShareSplit` or `BonusIssue` materialises nothing: the re-base is
/// applied at *read* time, so terms that overflow were accepted `204` and
/// then killed every open-holdings report of the whole portfolio — a
/// logged `500` with an empty body — until someone worked out which action
/// did it, with several of the reports that would have found it among the
/// ones that were down. Refused at the write instead, `422` naming the
/// ratio and the holding, and nothing is persisted.
#[tokio::test]
async fn api_a_split_that_rebases_a_parcel_beyond_the_decimal_range_is_refused() {
    for (action, body) in [
        // 1000-for-1, and its bonus-issue equivalent (999 new units for
        // every 1 held is the same 1000-for-1 re-base).
        (
            split(10, ListingId(1), d(2024, 7, 1), "1000", "1"),
            serde_json::json!({"action_type": "ShareSplit", "listing_id": 1,
                               "date": "2024-07-01", "split_new_units": "1000",
                               "split_old_units": "1"}),
        ),
        (
            bonus(10, ListingId(1), d(2024, 7, 1), "999", "1"),
            serde_json::json!({"action_type": "BonusIssue", "listing_id": 1,
                               "date": "2024-07-01", "bonus_units": "999",
                               "bonus_held_units": "1"}),
        ),
    ] {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "BIG").await;
        // Nil-priced, so the parcel's own cost base is representable
        // (W-e's bound accepts it) and only the re-base is at the ceiling.
        test_support::buy(1, 1)
            .date(d(2020, 10, 1))
            .settlement(d(2020, 10, 1))
            .qty("1000000000000000000000000000".parse().unwrap())
            .price(Decimal::ZERO)
            .insert(&pool)
            .await;

        let err = db_upsert(&pool, &action).await.unwrap_err();
        assert!(
            matches!(err, WriteError::UnrepresentableRebasedQuantity(_)),
            "expected the unrepresentable-quantity refusal, got: {err:?}"
        );

        let response = client(&pool).put("/corporate_actions/10", &body).await;
        let (status, detail) = response.status_and_body();
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
        assert!(
            detail.contains("quantity 1000000000000000000000000000 × new units 1000 / old units 1"),
            "the ratio and the holding are not named: {detail}"
        );
        assert!(
            detail.contains(&Decimal::MAX.to_string()),
            "the limit is not named: {detail}"
        );

        // Nothing persisted, and the reports the row would have killed
        // still read.
        let stored: bool =
            sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM corporate_actions WHERE id = 10)")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert!(!stored);
        let client = ApiClient::full(&pool);
        let rows: Vec<serde_json::Value> = client.get_json("/portfolio/open_parcels").await;
        assert_eq!(rows.len(), 1);
        let overview = client
            .post("/portfolio/overview", &serde_json::json!({}))
            .await;
        assert_eq!(overview.status, StatusCode::OK);
    }
}

/// The other direction of the same re-base, and the reason it has to be
/// checked *before* `allocations_fit_parcels`: a **consolidation**
/// recorded over an existing sale multiplies that sale's allocated units
/// back up into the parcel's as-acquired basis, and the over-consumption
/// check computes exactly that figure — so it overflowed inside the check
/// that would otherwise have refused the write, answering a logged `500`
/// instead of its own `422`.
#[tokio::test]
async fn api_a_consolidation_that_rebases_an_allocation_beyond_the_range_is_refused() {
    let pool = test_pool().await;
    insert_listing(&pool, 1, "BIG").await;
    test_support::buy(1, 1)
        .date(d(2020, 10, 1))
        .settlement(d(2020, 10, 1))
        .qty("1000000000000000000000000000".parse().unwrap())
        .price(Decimal::ZERO)
        .insert(&pool)
        .await;
    // The whole holding sold at nil, allocated 1:1 while no split exists.
    sell::db_upsert_sell(
        &pool,
        TradeId(2),
        &SellBody {
            brokerage_includes_gst: false,
            statement_total: None,
            holding_account_id: HoldingAccountId(1),
            date: d(2024, 6, 3),
            settlement_date: Some(d(2024, 6, 5)),
            listing_id: ListingId(1),
            average_price: Decimal::ZERO,
            quantity: "1000000000000000000000000000".parse().unwrap(),
            currency: "AUD".into(),
            brokerage: Decimal::ZERO,
            gst_on_brokerage: Decimal::ZERO,
            brokerage_currency: "AUD".into(),
            fx_rate: Decimal::ONE,
            spot_fx_rate: None,
            contract_note_ref: None,
            allocations: vec![AllocationInput {
                purchase_trade_id: TradeId(1),
                quantity_allocated: "1000000000000000000000000000".parse().unwrap(),
            }],
        },
    )
    .await
    .unwrap();

    // A 1-for-1000 consolidation dated between the parcel and the sale.
    let action = split(10, ListingId(1), d(2021, 1, 1), "1", "1000");
    let err = db_upsert(&pool, &action).await.unwrap_err();
    assert!(
        matches!(err, WriteError::UnrepresentableRebasedQuantity(_)),
        "expected the unrepresentable-quantity refusal, got: {err:?}"
    );
    let response = client(&pool)
        .put(
            "/corporate_actions/10",
            &serde_json::json!({"action_type": "ShareSplit", "listing_id": 1,
                                "date": "2021-01-01", "split_new_units": "1",
                                "split_old_units": "1000"}),
        )
        .await;
    let (status, detail) = response.status_and_body();
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert!(
        detail.contains(
            "quantity_allocated 1000000000000000000000000000 × old units 1000 / new units 1"
        ),
        "the ratio and the allocation are not named: {detail}"
    );
}

/// A database may already hold such an action — this rule postdates them,
/// and the mirror rule that stops one arriving behind an in-range ratio
/// postdates it again (`trade::UpsertError::UnrepresentableRebasedQuantity`
/// and its seven siblings). The refusal is judged on the terms being
/// *written*, never on the stored ones, so the edit that brings the ratio
/// back inside the range still lands, and the row is still deletable.
#[tokio::test]
async fn api_an_already_unrepresentable_action_can_still_be_edited_back_into_range() {
    let pool = test_pool().await;
    insert_listing(&pool, 1, "BIG").await;
    test_support::buy(1, 1)
        .date(d(2020, 10, 1))
        .settlement(d(2020, 10, 1))
        .qty("1000000000000000000000000000".parse().unwrap())
        .price(Decimal::ZERO)
        .insert(&pool)
        .await;
    // Straight into SQLite, behind the guard: only a pre-guard database
    // can hold this, which is precisely what the test is about.
    sqlx::query(
        "INSERT INTO corporate_actions              (id, action_type, listing_id, date, split_new_units, split_old_units)              VALUES (10, 'ShareSplit', 1, '2024-07-01', '1000', '1')",
    )
    .execute(&pool)
    .await
    .unwrap();
    // The state the guard exists to prevent: the open-holdings reads are
    // a logged 500 while it stands.
    assert_eq!(
        ApiClient::full(&pool)
            .get("/portfolio/open_parcels")
            .await
            .status,
        StatusCode::INTERNAL_SERVER_ERROR
    );

    // The correction: the same action, re-termed 2-for-1, lands.
    let corrected = split(10, ListingId(1), d(2024, 7, 1), "2", "1");
    db_upsert(&pool, &corrected).await.unwrap();
    let rows: Vec<serde_json::Value> = ApiClient::full(&pool)
        .get_json("/portfolio/open_parcels")
        .await;
    assert_eq!(
        rows[0]["remaining_quantity"],
        "2000000000000000000000000000"
    );

    // And it is still deletable.
    assert_eq!(
        client(&pool).delete("/corporate_actions/10").await.status,
        StatusCode::NO_CONTENT
    );
}

/// SCENARIOS W. A consolidation re-basing a very large holding:
/// `split_adjusted_quantity` multiplies 1e27 units by the ratio's
/// numerator (1000) before dividing by its denominator (1e6), and 1e30 is
/// past `Decimal`'s ~7.9228e28 ceiling even though the answer — 1e24
/// post-consolidation units — is representable. Driven through the
/// open-parcels report, the shortest read that re-bases a parcel.
#[tokio::test]
async fn api_open_parcels_past_the_old_split_rebasing_ceiling_reports() {
    let pool = test_pool().await;
    insert_listing(&pool, 1, "BIG").await;
    // Nil-priced, so the parcel's own cost base is not what is at the
    // limit (that bound is W-e's); only the unit re-basing is.
    test_support::buy(1, 1)
        .date(d(2024, 1, 15))
        .settlement(d(2024, 1, 15))
        .qty("1000000000000000000000000000".parse().unwrap())
        .price(Decimal::ZERO)
        .insert(&pool)
        .await;
    db_upsert(
        &pool,
        &split(10, ListingId(1), d(2024, 6, 1), "1000", "1000000"),
    )
    .await
    .unwrap();

    let rows: Vec<serde_json::Value> = ApiClient::full(&pool)
        .get_json("/portfolio/open_parcels")
        .await;
    assert_eq!(rows.len(), 1);
    assert_eq!(
        rows[0]["remaining_quantity"], "1000000000000000000000000",
        "{rows:?}"
    );
}

/// SCENARIOS W. The inverse re-basing, `as_acquired_quantity`, on the Sell
/// path: 1e24 post-consolidation units allocated back against the parcel
/// multiply by the denominator (1e6) first — 1e30 — before dividing by the
/// numerator, though the as-acquired figure (1e27) is exactly the parcel's
/// own quantity. A **write**, so the panic aborted the Sell.
#[tokio::test]
async fn api_sell_past_the_old_as_acquired_rebasing_ceiling_allocates() {
    let pool = test_pool().await;
    insert_listing(&pool, 1, "BIG").await;
    test_support::buy(1, 1)
        .date(d(2024, 1, 15))
        .settlement(d(2024, 1, 15))
        .qty("1000000000000000000000000000".parse().unwrap())
        .price(Decimal::ZERO)
        .insert(&pool)
        .await;
    db_upsert(
        &pool,
        &split(10, ListingId(1), d(2024, 6, 1), "1000", "1000000"),
    )
    .await
    .unwrap();

    // The whole holding, in post-consolidation units, sold at nil.
    sell::db_upsert_sell(
        &pool,
        TradeId(2),
        &SellBody {
            brokerage_includes_gst: false,
            statement_total: None,
            holding_account_id: HoldingAccountId(1),
            date: d(2024, 9, 2),
            settlement_date: Some(d(2024, 9, 2)),
            listing_id: ListingId(1),
            average_price: Decimal::ZERO,
            quantity: "1000000000000000000000000".parse().unwrap(),
            currency: "AUD".into(),
            brokerage: Decimal::ZERO,
            gst_on_brokerage: Decimal::ZERO,
            brokerage_currency: "AUD".into(),
            fx_rate: Decimal::ONE,
            spot_fx_rate: None,
            contract_note_ref: None,
            allocations: vec![AllocationInput {
                purchase_trade_id: TradeId(1),
                quantity_allocated: "1000000000000000000000000".parse().unwrap(),
            }],
        },
    )
    .await
    .unwrap();

    // The parcel is fully consumed, which is what says the allocation was
    // converted back to the parcel's own 1e27 as-acquired units.
    let rows: Vec<serde_json::Value> = ApiClient::full(&pool)
        .get_json("/portfolio/open_parcels")
        .await;
    assert!(rows.is_empty(), "{rows:?}");
}

/// SCENARIOS W. A return-of-capital payment re-based across a split it was
/// quoted after: `RocEvent::per_unit_for` multiplies the per-unit amount
/// (1e24) by the split's numerator (1e6) before dividing by its
/// denominator, and 1e30 is past `Decimal`'s ~7.9228e28 ceiling even
/// though the per-as-acquired-unit figure, 1e27, is representable.
#[tokio::test]
async fn api_open_parcels_past_the_old_payment_rebasing_ceiling_reports() {
    let pool = test_pool().await;
    insert_listing(&pool, 1, "BIG").await;
    // One nil-priced unit, so only the payment's re-basing is at the
    // ceiling — the parcel's own cost base is nil.
    test_support::buy(1, 1)
        .date(d(2024, 1, 15))
        .settlement(d(2024, 1, 15))
        .qty(Decimal::ONE)
        .price(Decimal::ZERO)
        .insert(&pool)
        .await;
    // A 1,000,000-for-1,000 split, i.e. 1000 new units for each old one.
    db_upsert(
        &pool,
        &split(10, ListingId(1), d(2024, 6, 3), "1000000", "1000"),
    )
    .await
    .unwrap();
    // …and a payment quoted per *post*-split unit.
    db_upsert(
        &pool,
        &roc(11, ListingId(1), d(2024, 7, 1), "1000000000000000000000000"),
    )
    .await
    .unwrap();

    let rows: Vec<serde_json::Value> = ApiClient::full(&pool)
        .get_json("/portfolio/open_parcels")
        .await;
    assert_eq!(rows.len(), 1);
    assert_eq!(
        rows[0]["return_of_capital_reduction"], "1000000000000000000000000000",
        "{rows:?}"
    );
}

/// SCENARIOS W. The price dual of the same re-basing: a figure observed
/// after a 1000-for-1 split, restated into its own trading day's basis,
/// multiplies by the numerator (1e6) first. A unit test rather than an
/// API one because the only production caller is the provider-price
/// re-basing pass, which would need a stubbed fetch and a stored
/// `price_as_observed` row to reach one multiplication.
#[test]
fn contemporaneous_price_past_the_old_ceiling_still_recovers_the_day() {
    let splits = [split_event(d(2024, 6, 3), "1000000", "1000")];
    let events: Vec<PriceBasisEvent> = splits.iter().map(PriceBasisEvent::from).collect();
    // 1e24 × 1e6 is 1e30 — past the ceiling — while the recovered
    // pre-split price, 1e27, is representable.
    assert_eq!(
        contemporaneous_price(
            "1000000000000000000000000".parse().unwrap(),
            &events,
            d(2024, 5, 1),
            d(2024, 7, 1),
        ),
        "1000000000000000000000000000".parse::<Decimal>().unwrap()
    );
}
