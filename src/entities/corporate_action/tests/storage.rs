//! Each action type written and read back through the database, the CHECKs
//! that refuse a malformed payload, and the per-listing event loads.

use super::*;

/// A `Demerger` carrying the stated pre-demerger close (and its
/// provenance) that the price re-base derives its factor from.
fn demerger_with_close(
    id: i64,
    listing_id: ListingId,
    demerger_listing_id: ListingId,
    date: NaiveDate,
    close_date: NaiveDate,
    close: &str,
) -> CorporateAction {
    let mut action = demerger(id, listing_id, demerger_listing_id, date, "1", "1", "36");
    if let ActionKind::Demerger {
        demerger_close_date,
        demerger_close_price,
        demerger_close_sourced_from,
        demerger_close_reason,
        ..
    } = &mut action.kind
    {
        *demerger_close_date = Some(close_date);
        *demerger_close_price = Some(close.parse().unwrap());
        *demerger_close_sourced_from = Some("nyse.com daily close".to_owned());
        *demerger_close_reason = Some("the provider adjusts the pre-demerger series".to_owned());
    }
    action
}

fn buyback(
    id: i64,
    listing_id: ListingId,
    date: NaiveDate,
    price: &str,
    dividend: &str,
    credit: &str,
    market_value: Option<&str>,
) -> CorporateAction {
    CorporateAction {
        id,
        listing_id,
        date,
        kind: ActionKind::BuyBack {
            buyback_price: price.parse().unwrap(),
            buyback_dividend: dividend.parse().unwrap(),
            buyback_franking_credit: credit.parse().unwrap(),
            buyback_market_value: market_value.map(|v| v.parse().unwrap()),
            currency: "AUD".into(),
        },
    }
}

#[tokio::test]
async fn db_insert_and_retrieve_preserves_precision() {
    let pool = test_pool().await;
    insert_listing(&pool, 1, "RAP").await;
    db_upsert(&pool, &roc(1, ListingId(1), d(2024, 11, 30), "0.505"))
        .await
        .unwrap();

    let got = db_get(&pool, 1).await.unwrap().unwrap();
    assert_eq!(got.listing_id, 1);
    assert_eq!(got.date, d(2024, 11, 30));
    assert_eq!(
        got.kind,
        ActionKind::ReturnOfCapital {
            amount_per_unit: "0.505".parse().unwrap(),
            currency: "AUD".into(),
            record_date: None,
        }
    );
}

#[tokio::test]
async fn db_insert_and_retrieve_share_split_preserves_ratio() {
    let pool = test_pool().await;
    insert_listing(&pool, 1, "SPL").await;
    // An uneven ratio (e.g. a 7-for-2 split) must round-trip exactly.
    db_upsert(&pool, &split(1, ListingId(1), d(2024, 11, 30), "7", "2"))
        .await
        .unwrap();

    let got = db_get(&pool, 1).await.unwrap().unwrap();
    assert_eq!(
        got.kind,
        ActionKind::ShareSplit {
            split_new_units: Decimal::from(7),
            split_old_units: Decimal::from(2),
        }
    );
}

#[tokio::test]
async fn db_insert_and_retrieve_bonus_issue_preserves_ratio() {
    let pool = test_pool().await;
    insert_listing(&pool, 1, "BON").await;
    // An uneven ratio (e.g. 3 bonus shares per 7 held) must round-trip exactly.
    db_upsert(&pool, &bonus(1, ListingId(1), d(2024, 11, 30), "3", "7"))
        .await
        .unwrap();

    let got = db_get(&pool, 1).await.unwrap().unwrap();
    assert_eq!(
        got.kind,
        ActionKind::BonusIssue {
            bonus_units: Decimal::from(3),
            bonus_held_units: Decimal::from(7),
        }
    );
}

#[tokio::test]
async fn db_insert_and_retrieve_rights_issue_preserves_terms() {
    let pool = test_pool().await;
    insert_listing(&pool, 1, "RTS").await;
    // An uneven ratio and a sub-cent price must round-trip exactly.
    db_upsert(
        &pool,
        &rights(1, ListingId(1), d(2024, 11, 30), "3", "7", "1.805"),
    )
    .await
    .unwrap();

    let got = db_get(&pool, 1).await.unwrap().unwrap();
    assert_eq!(
        got.kind,
        ActionKind::RightsIssue {
            rights_units: Decimal::from(3),
            rights_held_units: Decimal::from(7),
            exercise_price: "1.805".parse().unwrap(),
            currency: "AUD".into(),
            renounceable: true,
        }
    );
}

#[tokio::test]
async fn db_insert_and_retrieve_buy_back_preserves_terms() {
    let pool = test_pool().await;
    insert_listing(&pool, 1, "BBK").await;
    // Sub-cent per-unit components must round-trip exactly.
    db_upsert(
        &pool,
        &buyback(
            1,
            ListingId(1),
            d(2024, 11, 30),
            "9.60",
            "1.405",
            "0.605",
            Some("10.20"),
        ),
    )
    .await
    .unwrap();

    let got = db_get(&pool, 1).await.unwrap().unwrap();
    assert_eq!(
        got.kind,
        ActionKind::BuyBack {
            buyback_price: "9.60".parse().unwrap(),
            buyback_dividend: "1.405".parse().unwrap(),
            buyback_franking_credit: "0.605".parse().unwrap(),
            buyback_market_value: Some("10.20".parse().unwrap()),
            currency: "AUD".into(),
        }
    );

    // The market value is optional: absent round-trips as None.
    db_upsert(
        &pool,
        &buyback(2, ListingId(1), d(2024, 12, 31), "5.00", "0", "0", None),
    )
    .await
    .unwrap();
    let got = db_get(&pool, 2).await.unwrap().unwrap();
    assert!(matches!(
        got.kind,
        ActionKind::BuyBack {
            buyback_market_value: None,
            ..
        }
    ));
}

#[tokio::test]
async fn db_insert_and_retrieve_scrip_for_scrip_preserves_terms() {
    let pool = test_pool().await;
    insert_listing(&pool, 1, "OLD").await;
    insert_listing(&pool, 2, "NEW").await;
    // An uneven exchange ratio (e.g. 3 new shares per 7 old) must
    // round-trip exactly.
    db_upsert(
        &pool,
        &scrip(1, ListingId(1), ListingId(2), d(2024, 11, 30), "3", "7"),
    )
    .await
    .unwrap();

    let got = db_get(&pool, 1).await.unwrap().unwrap();
    assert_eq!(got.listing_id, 1);
    assert_eq!(
        got.kind,
        ActionKind::ScripForScrip {
            scrip_listing_id: ListingId(2),
            scrip_new_units: Decimal::from(3),
            scrip_old_units: Decimal::from(7),
            scrip_cash_per_unit: None,
            scrip_market_value: None,
            scrip_cash_currency: None,
        }
    );
}

#[tokio::test]
async fn db_insert_and_retrieve_demerger_preserves_terms() {
    let pool = test_pool().await;
    insert_listing(&pool, 1, "HEAD").await;
    insert_listing(&pool, 2, "DEM").await;
    // An uneven ratio and a sub-unit percentage (BHP Steel's 5.063%) must
    // round-trip exactly.
    db_upsert(
        &pool,
        &demerger(
            1,
            ListingId(1),
            ListingId(2),
            d(2024, 11, 30),
            "1",
            "5",
            "5.063",
        ),
    )
    .await
    .unwrap();

    let got = db_get(&pool, 1).await.unwrap().unwrap();
    assert_eq!(got.listing_id, 1);
    assert_eq!(
        got.kind,
        ActionKind::Demerger {
            demerger_listing_id: ListingId(2),
            demerger_new_units: Decimal::ONE,
            demerger_held_units: Decimal::from(5),
            demerger_cost_base_pct: "5.063".parse().unwrap(),
            demerger_close_date: None,
            demerger_close_price: None,
            demerger_close_sourced_from: None,
            demerger_close_reason: None,
        }
    );
}

/// A Demerger never appears in the split-event or return-of-capital
/// streams — recording one changes no existing parcel (the demerge
/// operation does the apportionment).
#[tokio::test]
async fn db_demerger_is_not_a_split_or_payment_event() {
    let pool = test_pool().await;
    insert_listing(&pool, 1, "HEAD").await;
    insert_listing(&pool, 2, "DEM").await;
    db_upsert(
        &pool,
        &demerger(
            1,
            ListingId(1),
            ListingId(2),
            d(2024, 11, 30),
            "1",
            "5",
            "5.063",
        ),
    )
    .await
    .unwrap();

    assert!(db_share_split_events(&pool).await.unwrap().is_empty());
    assert!(
        db_splits_for_listing(&pool, ListingId(1))
            .await
            .unwrap()
            .is_empty()
    );
    assert!(db_return_of_capital_events(&pool).await.unwrap().is_empty());
}

/// **The structural point.** A demerger's stated close belongs to the
/// *price* re-basing set only. Recording one must move no quantity, no
/// cost base and no allocation capacity — the three things `split_ratio`
/// decides, and which `domain::cost_base`, `domain::open_parcels`, the
/// AMIT re-basing and the Sell/trade write-time checks all read. If the
/// factor ever leaked into the quantity set this test would fail, which is
/// the whole reason the two event sets are separate types.
#[tokio::test]
async fn db_a_demergers_stated_close_moves_no_quantity_cost_base_or_capacity() {
    let pool = test_pool().await;
    insert_listing(&pool, 1, "LAC").await;
    insert_listing(&pool, 2, "LAR").await;
    test_support::buy(10, 1)
        .date(d(2021, 3, 25))
        .qty(Decimal::from(1049))
        .price(Decimal::from(20))
        .insert(&pool)
        .await;
    // A sale that consumes part of the parcel: its allocation capacity is
    // computed from the split stream at read time, so a demerger sneaking
    // into that stream would change what still fits.
    test_support::sell(11, 1)
        .date(d(2023, 5, 1))
        .qty(Decimal::from(49))
        .price(Decimal::from(30))
        .insert(&pool)
        .await;
    sqlx::query(
        "INSERT INTO parcel_allocations (sale_trade_id, purchase_trade_id, quantity_allocated) \
             VALUES (11, 10, '49')",
    )
    .execute(&pool)
    .await
    .unwrap();

    async fn open_position(pool: &SqlitePool) -> (Decimal, Decimal) {
        let mut conn = pool.acquire().await.unwrap();
        let parcels = open_parcels::load(&mut conn, None).await.unwrap();
        assert_eq!(parcels.len(), 1);
        (
            parcels[0].remaining_as_of.quantity(),
            parcels[0].cost_base.adjusted,
        )
    }

    let before = open_position(&pool).await;
    assert_eq!(before.0, Decimal::from(1000));

    db_upsert(
        &pool,
        &demerger_with_close(
            1,
            ListingId(1),
            ListingId(2),
            d(2023, 10, 3),
            d(2023, 10, 2),
            "24.90",
        ),
    )
    .await
    .unwrap();

    assert_eq!(
        open_position(&pool).await,
        before,
        "a demerger's stated close is a price fact — it must not restate a \
             single unit or a single dollar of cost base"
    );
    assert!(
        db_splits_for_listing(&pool, ListingId(1))
            .await
            .unwrap()
            .is_empty(),
        "the quantity re-basing set never sees a demerger"
    );
    assert!(db_share_split_events(&pool).await.unwrap().is_empty());
    assert_eq!(
        split_adjusted_quantity(
            AcquiredUnits::new(Decimal::from(1000)),
            &[],
            d(2021, 3, 25),
            None
        ),
        Decimal::from(1000),
        "and the conversion the capacity checks run is still the identity"
    );

    // The remaining 1000 units still fit a sale of exactly 1000 — the
    // allocation-capacity invariant a corporate-action write re-checks.
    let statements = db_demerger_price_statements(&pool, ListingId(1))
        .await
        .unwrap();
    assert_eq!(statements.len(), 1, "…while the price side does see it");
    assert_eq!(statements[0].close_date, d(2023, 10, 2));
    assert_eq!(statements[0].close_price, "24.90".parse().unwrap());
}

/// The statement loader reads only demergers that actually carry a close,
/// on the listing asked for, in date order.
#[tokio::test]
async fn db_demerger_price_statements_skips_a_demerger_with_no_stated_close() {
    let pool = test_pool().await;
    insert_listing(&pool, 1, "HEAD").await;
    insert_listing(&pool, 2, "DEM").await;
    db_upsert(
        &pool,
        &demerger(
            1,
            ListingId(1),
            ListingId(2),
            d(2024, 11, 30),
            "1",
            "5",
            "5.063",
        ),
    )
    .await
    .unwrap();
    assert!(
        db_demerger_price_statements(&pool, ListingId(1))
            .await
            .unwrap()
            .is_empty()
    );

    db_upsert(
        &pool,
        &demerger_with_close(
            2,
            ListingId(1),
            ListingId(2),
            d(2025, 6, 2),
            d(2025, 5, 30),
            "12.5",
        ),
    )
    .await
    .unwrap();
    let statements = db_demerger_price_statements(&pool, ListingId(1))
        .await
        .unwrap();
    assert_eq!(statements.len(), 1);
    assert_eq!(statements[0].date, d(2025, 6, 2));
    assert!(
        db_demerger_price_statements(&pool, ListingId(2))
            .await
            .unwrap()
            .is_empty()
    );
}

/// The CHECK rejects a demerger of a listing into itself even on a raw
/// SQL write — the body validation is the first line of defence.
#[tokio::test]
async fn db_check_rejects_self_demerger() {
    let pool = test_pool().await;
    insert_listing(&pool, 1, "HEAD").await;
    let result = sqlx::query(
        "INSERT INTO corporate_actions \
             (id, action_type, listing_id, date, demerger_listing_id, demerger_new_units, \
              demerger_held_units, demerger_cost_base_pct) \
             VALUES (1, 'Demerger', 1, '2024-11-30', 1, '1', '5', '5.063')",
    )
    .execute(&pool)
    .await;
    assert!(
        result.is_err(),
        "demerger_listing_id == listing_id should violate the CHECK"
    );
}

/// A `ScripForScrip` never appears in the split-event or return-of-capital
/// streams — recording one changes no existing parcel (the exchange
/// operation does the substitution).
#[tokio::test]
async fn db_scrip_for_scrip_is_not_a_split_or_payment_event() {
    let pool = test_pool().await;
    insert_listing(&pool, 1, "OLD").await;
    insert_listing(&pool, 2, "NEW").await;
    db_upsert(
        &pool,
        &scrip(1, ListingId(1), ListingId(2), d(2024, 11, 30), "2", "1"),
    )
    .await
    .unwrap();

    assert!(db_share_split_events(&pool).await.unwrap().is_empty());
    assert!(
        db_splits_for_listing(&pool, ListingId(1))
            .await
            .unwrap()
            .is_empty()
    );
    assert!(db_return_of_capital_events(&pool).await.unwrap().is_empty());
}

/// The CHECK rejects an exchange of a listing into itself even on a raw
/// SQL write — the body validation is the first line of defence.
#[tokio::test]
async fn db_check_rejects_self_exchange() {
    let pool = test_pool().await;
    insert_listing(&pool, 1, "OLD").await;
    let result = sqlx::query(
        "INSERT INTO corporate_actions \
             (id, action_type, listing_id, date, scrip_listing_id, scrip_new_units, scrip_old_units) \
             VALUES (1, 'ScripForScrip', 1, '2024-11-30', 1, '2', '1')",
    )
    .execute(&pool)
    .await;
    assert!(
        result.is_err(),
        "scrip_listing_id == listing_id should violate the CHECK"
    );
}

/// A `BuyBack` never appears in the split-event or return-of-capital
/// streams — recording one changes no existing parcel.
#[tokio::test]
async fn db_buy_back_is_not_a_split_or_payment_event() {
    let pool = test_pool().await;
    insert_listing(&pool, 1, "BBK").await;
    db_upsert(
        &pool,
        &buyback(
            1,
            ListingId(1),
            d(2024, 11, 30),
            "9.60",
            "1.40",
            "0.60",
            Some("10.20"),
        ),
    )
    .await
    .unwrap();

    assert!(db_share_split_events(&pool).await.unwrap().is_empty());
    assert!(
        db_splits_for_listing(&pool, ListingId(1))
            .await
            .unwrap()
            .is_empty()
    );
    assert!(db_return_of_capital_events(&pool).await.unwrap().is_empty());
}

/// A `RightsIssue` never appears in the split-event or return-of-capital
/// streams — recording one changes no existing parcel.
#[tokio::test]
async fn db_rights_issue_is_not_a_split_or_payment_event() {
    let pool = test_pool().await;
    insert_listing(&pool, 1, "RTS").await;
    db_upsert(
        &pool,
        &rights(1, ListingId(1), d(2024, 11, 30), "1", "4", "1.80"),
    )
    .await
    .unwrap();

    assert!(db_share_split_events(&pool).await.unwrap().is_empty());
    assert!(
        db_splits_for_listing(&pool, ListingId(1))
            .await
            .unwrap()
            .is_empty()
    );
    assert!(db_return_of_capital_events(&pool).await.unwrap().is_empty());
}

/// The record date round-trips, reaches the reports through the event
/// stream they read, and clears again — the payment-date fallback is a
/// state a correction can return to, not just the absence of a first write.
#[tokio::test]
async fn db_return_of_capital_record_date_round_trips() {
    let pool = test_pool().await;
    insert_listing(&pool, 1, "RAP").await;
    let action = roc_with_record(1, ListingId(1), d(2025, 3, 1), "0.50", d(2025, 2, 10));
    db_upsert(&pool, &action).await.unwrap();

    assert_eq!(db_get(&pool, 1).await.unwrap().unwrap().kind, action.kind);
    let events = db_return_of_capital_events(&pool).await.unwrap();
    assert_eq!(events[&ListingId(1)][0].record_date, Some(d(2025, 2, 10)));

    db_upsert(&pool, &roc(1, ListingId(1), d(2025, 3, 1), "0.50"))
        .await
        .unwrap();
    let events = db_return_of_capital_events(&pool).await.unwrap();
    assert_eq!(events[&ListingId(1)][0].record_date, None);
}

/// The record date fixes entitlement to *this* payment, so it can never
/// follow it, and no other action type has one (the CHECK of 0023 — the
/// last line of defence behind the body validation, reached only by a raw
/// SQL write).
#[tokio::test]
async fn db_check_rejects_an_impossible_record_date() {
    let pool = test_pool().await;
    insert_listing(&pool, 1, "RAP").await;
    db_upsert(&pool, &roc(1, ListingId(1), d(2025, 3, 1), "0.50"))
        .await
        .unwrap();
    db_upsert(&pool, &split(2, ListingId(1), d(2025, 3, 1), "2", "1"))
        .await
        .unwrap();

    for (id, record_date) in [(1, "2025-03-02"), (2, "2025-02-10")] {
        let result = sqlx::query(sqlx::AssertSqlSafe(format!(
            "UPDATE corporate_actions SET record_date = '{record_date}' WHERE id = {id}"
        )))
        .execute(&pool)
        .await;
        assert!(
            result.is_err(),
            "record_date {record_date} on action {id} should violate the CHECK"
        );
    }
    // …and the record date the payment does allow still writes.
    sqlx::query("UPDATE corporate_actions SET record_date = '2025-03-01' WHERE id = 1")
        .execute(&pool)
        .await
        .unwrap();
}

#[tokio::test]
async fn db_upsert_updates_existing() {
    let pool = test_pool().await;
    insert_listing(&pool, 1, "RAP").await;
    db_upsert(&pool, &roc(1, ListingId(1), d(2024, 11, 30), "0.50"))
        .await
        .unwrap();
    db_upsert(&pool, &roc(1, ListingId(1), d(2024, 12, 31), "0.75"))
        .await
        .unwrap();

    let all = db_list(&pool).await.unwrap();
    assert_eq!(all.len(), 1);
    assert_eq!(all[0].date, d(2024, 12, 31));
    assert_eq!(
        all[0].kind,
        roc(1, ListingId(1), d(2024, 12, 31), "0.75").kind
    );
}

#[tokio::test]
async fn db_listing_fk_enforced() {
    let pool = test_pool().await;
    let err = db_upsert(&pool, &roc(1, ListingId(999), d(2024, 11, 30), "0.50")).await;
    assert!(err.is_err(), "unknown listing FK should be rejected");
}

/// Mixed payloads are unrepresentable in [`ActionKind`], so a raw SQL
/// write is the only path that could produce one — the per-type table
/// CHECKs are the last line of defence and must still reject it.
#[tokio::test]
async fn db_check_rejects_mixed_payloads() {
    let pool = test_pool().await;
    insert_listing(&pool, 1, "RAP").await;
    insert_listing(&pool, 2, "NEW").await;
    // (action_type, the stray columns the CHECK must reject for it)
    for (action_type, stray_cols) in [
        // A ShareSplit carrying a payment, a bonus ratio, or rights terms…
        ("ShareSplit", "amount_per_unit = '0.50', currency = 'AUD'"),
        ("ShareSplit", "bonus_units = '1', bonus_held_units = '10'"),
        (
            "ShareSplit",
            "rights_units = '1', rights_held_units = '4', exercise_price = '1.80'",
        ),
        // …a ReturnOfCapital carrying a split ratio…
        (
            "ReturnOfCapital",
            "split_new_units = '2', split_old_units = '1'",
        ),
        // …a BonusIssue carrying a split ratio…
        ("BonusIssue", "split_new_units = '2', split_old_units = '1'"),
        // …a RightsIssue carrying a payment or a split ratio…
        ("RightsIssue", "amount_per_unit = '0.50'"),
        (
            "RightsIssue",
            "split_new_units = '2', split_old_units = '1'",
        ),
        // …a BuyBack carrying a payment, a split ratio, or rights terms…
        ("BuyBack", "amount_per_unit = '0.50'"),
        ("BuyBack", "split_new_units = '2', split_old_units = '1'"),
        (
            "BuyBack",
            "rights_units = '1', rights_held_units = '4', exercise_price = '1.80'",
        ),
        // …the other types carrying buy-back terms…
        (
            "ShareSplit",
            "buyback_price = '9.60', buyback_dividend = '0', buyback_franking_credit = '0'",
        ),
        (
            "ReturnOfCapital",
            "buyback_price = '9.60', buyback_dividend = '0', buyback_franking_credit = '0'",
        ),
        ("RightsIssue", "buyback_market_value = '10.20'"),
        // …a ScripForScrip carrying a payment, a split ratio, or buy-back
        // terms…
        (
            "ScripForScrip",
            "amount_per_unit = '0.50', currency = 'AUD'",
        ),
        (
            "ScripForScrip",
            "split_new_units = '2', split_old_units = '1'",
        ),
        (
            "ScripForScrip",
            "buyback_price = '9.60', buyback_dividend = '0', buyback_franking_credit = '0'",
        ),
        // …and the other types carrying scrip terms…
        (
            "ShareSplit",
            "scrip_listing_id = 2, scrip_new_units = '2', scrip_old_units = '1'",
        ),
        (
            "BuyBack",
            "scrip_listing_id = 2, scrip_new_units = '2', scrip_old_units = '1'",
        ),
        // …a ScripForScrip carrying a partial cash set (all-or-none) and
        // another type carrying a full cash set…
        ("ScripForScrip", "scrip_cash_per_unit = '10'"),
        (
            "ShareSplit",
            "scrip_cash_per_unit = '10', scrip_market_value = '20', \
                 scrip_cash_currency = 'AUD'",
        ),
        // …a Demerger carrying a payment, a split ratio, or scrip terms…
        ("Demerger", "amount_per_unit = '0.50', currency = 'AUD'"),
        ("Demerger", "split_new_units = '2', split_old_units = '1'"),
        (
            "Demerger",
            "scrip_listing_id = 2, scrip_new_units = '2', scrip_old_units = '1'",
        ),
        // …and the other types carrying demerger terms.
        (
            "ShareSplit",
            "demerger_listing_id = 2, demerger_new_units = '1', demerger_held_units = '5', demerger_cost_base_pct = '5.063'",
        ),
        (
            "ScripForScrip",
            "demerger_listing_id = 2, demerger_new_units = '1', demerger_held_units = '5', demerger_cost_base_pct = '5.063'",
        ),
    ] {
        let (base_cols, base_vals) = match action_type {
            "ShareSplit" => ("split_new_units, split_old_units", "'2', '1'"),
            "ReturnOfCapital" => ("amount_per_unit, currency", "'0.50', 'AUD'"),
            "RightsIssue" => (
                "rights_units, rights_held_units, exercise_price, currency",
                "'1', '4', '1.80', 'AUD'",
            ),
            "BuyBack" => (
                "buyback_price, buyback_dividend, buyback_franking_credit, currency",
                "'9.60', '1.40', '0.60', 'AUD'",
            ),
            "ScripForScrip" => (
                "scrip_listing_id, scrip_new_units, scrip_old_units",
                "2, '2', '1'",
            ),
            "Demerger" => (
                "demerger_listing_id, demerger_new_units, demerger_held_units, \
                     demerger_cost_base_pct",
                "2, '1', '5', '5.063'",
            ),
            _ => ("bonus_units, bonus_held_units", "'1', '10'"),
        };
        // Insert a valid row, then try to smuggle the stray columns in.
        sqlx::query(sqlx::AssertSqlSafe(format!(
            "INSERT INTO corporate_actions (id, action_type, listing_id, date, {base_cols}) \
                 VALUES (1, '{action_type}', 1, '2024-11-30', {base_vals})"
        )))
        .execute(&pool)
        .await
        .unwrap();
        let result = sqlx::query(sqlx::AssertSqlSafe(format!(
            "UPDATE corporate_actions SET {stray_cols} WHERE id = 1"
        )))
        .execute(&pool)
        .await;
        assert!(
            result.is_err(),
            "{action_type} + {stray_cols} should violate the CHECK"
        );
        sqlx::query("DELETE FROM corporate_actions")
            .execute(&pool)
            .await
            .unwrap();
    }
}

#[tokio::test]
async fn db_get_missing_returns_none() {
    let pool = test_pool().await;
    assert!(db_get(&pool, 999).await.unwrap().is_none());
}

#[tokio::test]
async fn db_events_grouped_by_listing_sorted_by_date() {
    let pool = test_pool().await;
    insert_listing(&pool, 1, "RAP").await;
    insert_listing(&pool, 2, "XYZ").await;
    db_upsert(&pool, &roc(1, ListingId(1), d(2025, 3, 1), "0.30"))
        .await
        .unwrap();
    db_upsert(&pool, &roc(2, ListingId(1), d(2024, 11, 30), "0.50"))
        .await
        .unwrap();
    db_upsert(&pool, &roc(3, ListingId(2), d(2024, 6, 1), "1.00"))
        .await
        .unwrap();

    let events = db_return_of_capital_events(&pool).await.unwrap();
    assert_eq!(events.len(), 2);
    let l1: Vec<NaiveDate> = events[&ListingId(1)].iter().map(|e| e.date).collect();
    assert_eq!(l1, vec![d(2024, 11, 30), d(2025, 3, 1)]);
    assert_eq!(events[&ListingId(2)].len(), 1);
}

#[tokio::test]
async fn db_split_events_grouped_by_listing_sorted_by_date() {
    let pool = test_pool().await;
    insert_listing(&pool, 1, "SPL").await;
    insert_listing(&pool, 2, "XYZ").await;
    db_upsert(&pool, &split(1, ListingId(1), d(2025, 3, 1), "2", "1"))
        .await
        .unwrap();
    db_upsert(&pool, &split(2, ListingId(1), d(2024, 11, 30), "1", "10"))
        .await
        .unwrap();
    db_upsert(&pool, &split(3, ListingId(2), d(2024, 6, 1), "3", "1"))
        .await
        .unwrap();
    // A ReturnOfCapital on the same listing must not appear as a split.
    db_upsert(&pool, &roc(4, ListingId(1), d(2024, 6, 1), "0.50"))
        .await
        .unwrap();

    let events = db_share_split_events(&pool).await.unwrap();
    assert_eq!(events.len(), 2);
    let l1: Vec<NaiveDate> = events[&ListingId(1)].iter().map(|e| e.date).collect();
    assert_eq!(l1, vec![d(2024, 11, 30), d(2025, 3, 1)]);
    assert_eq!(events[&ListingId(2)].len(), 1);

    let for_listing = db_splits_for_listing(&pool, ListingId(1)).await.unwrap();
    assert_eq!(for_listing.len(), 2);
    assert_eq!(for_listing[0].date, d(2024, 11, 30));
}

/// A `BonusIssue` is folded into the split-event stream as its equivalent
/// split: every `bonus_held_units` units become `bonus_held_units +
/// bonus_units` units (a 1-for-10 bonus issue re-bases 11-for-10).
#[tokio::test]
async fn db_split_events_include_bonus_issues_as_equivalent_splits() {
    let pool = test_pool().await;
    insert_listing(&pool, 1, "BON").await;
    db_upsert(&pool, &bonus(1, ListingId(1), d(2025, 3, 1), "1", "10"))
        .await
        .unwrap();
    // A real split on the same listing interleaves in date order…
    db_upsert(&pool, &split(2, ListingId(1), d(2024, 11, 30), "2", "1"))
        .await
        .unwrap();
    // …and a ReturnOfCapital never appears as a re-basing event.
    db_upsert(&pool, &roc(3, ListingId(1), d(2024, 6, 1), "0.50"))
        .await
        .unwrap();

    let events = db_share_split_events(&pool).await.unwrap();
    let l1 = &events[&ListingId(1)];
    assert_eq!(l1.len(), 2);
    assert_eq!(l1[0].date, d(2024, 11, 30));
    assert_eq!(
        (l1[0].new_units, l1[0].old_units),
        (Decimal::from(2), Decimal::ONE)
    );
    assert_eq!(l1[1].date, d(2025, 3, 1));
    assert_eq!(
        (l1[1].new_units, l1[1].old_units),
        (Decimal::from(11), Decimal::from(10))
    );

    let for_listing = db_splits_for_listing(&pool, ListingId(1)).await.unwrap();
    assert_eq!(for_listing.len(), 2);
    assert_eq!(for_listing[1].new_units, Decimal::from(11));
    assert_eq!(for_listing[1].old_units, Decimal::from(10));
}

/// SCENARIOS Z-e: the loaders carry each action's **announced terms**
/// alongside the derived rebase factor, so a human-readable surface can
/// name the event the way the company announced it. The factor is what it
/// always was — a bonus issue still re-bases 11-for-10 — and only the
/// label reads from the terms.
#[tokio::test]
async fn db_split_events_carry_each_actions_announced_terms() {
    let pool = test_pool().await;
    insert_listing(&pool, 1, "BON").await;
    db_upsert(&pool, &split(1, ListingId(1), d(2024, 11, 30), "2", "1"))
        .await
        .unwrap();
    db_upsert(&pool, &bonus(2, ListingId(1), d(2025, 3, 1), "1", "10"))
        .await
        .unwrap();
    // new < old: a consolidation, announced in the same new-for-old terms.
    db_upsert(&pool, &split(3, ListingId(1), d(2025, 6, 1), "1", "2"))
        .await
        .unwrap();
    // Terms typed with a scale were still announced as "2-for-1".
    db_upsert(
        &pool,
        &split(4, ListingId(1), d(2025, 9, 1), "2.00", "1.00"),
    )
    .await
    .unwrap();

    for events in [
        db_splits_for_listing(&pool, ListingId(1)).await.unwrap(),
        db_share_split_events(&pool).await.unwrap()[&ListingId(1)].clone(),
    ] {
        let labels: Vec<String> = events.iter().map(|e| e.terms.label()).collect();
        assert_eq!(
            labels,
            vec![
                "2-for-1 split",
                "1-for-10 bonus issue",
                "1-for-2 consolidation",
                "2-for-1 split",
            ]
        );
        // The re-basing arithmetic is untouched by the terms travelling
        // with the event: the bonus issue is still its equivalent split.
        let factors: Vec<(Decimal, Decimal)> =
            events.iter().map(|e| (e.new_units, e.old_units)).collect();
        assert_eq!(
            factors,
            vec![
                (Decimal::from(2), Decimal::ONE),
                (Decimal::from(11), Decimal::from(10)),
                (Decimal::ONE, Decimal::from(2)),
                ("2.00".parse().unwrap(), "1.00".parse().unwrap()),
            ]
        );
        // 100 units → 200 → 220 → 110 → 220, over the same walk as before.
        assert_eq!(
            split_adjusted_quantity(
                AcquiredUnits::new(Decimal::from(100)),
                &events,
                d(2024, 1, 1),
                None
            ),
            Decimal::from(220)
        );
    }
}
