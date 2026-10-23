//! The HTTP surface: each action type's round trip, and the `422` every
//! malformed payload answers.

use super::*;

#[tokio::test]
async fn api_put_get_list_delete_round_trip() {
    let pool = test_pool().await;
    insert_listing(&pool, 1, "RAP").await;
    let body = serde_json::json!({
        "action_type": "ReturnOfCapital",
        "listing_id": 1,
        "date": "2024-11-30",
        "amount_per_unit": "0.50",
        "currency": "AUD",
    });
    let resp = client(&pool).put("/corporate_actions/1", &body).await;
    assert_eq!(resp.status, StatusCode::CREATED);

    let resp = client(&pool).get("/corporate_actions/1").await;
    assert_eq!(resp.status, StatusCode::OK);
    let got: CorporateAction = resp.json();
    assert_eq!(got.kind, roc(1, ListingId(1), d(2024, 11, 30), "0.50").kind);

    let resp = client(&pool).get("/corporate_actions").await;
    assert_eq!(resp.status, StatusCode::OK);
    let items: Vec<CorporateAction> = resp.json();
    assert_eq!(items.len(), 1);

    let resp = client(&pool).delete("/corporate_actions/1").await;
    assert_eq!(resp.status, StatusCode::NO_CONTENT);
    assert!(db_get(&pool, 1).await.unwrap().is_none());
}

#[tokio::test]
async fn api_share_split_round_trip() {
    let pool = test_pool().await;
    insert_listing(&pool, 1, "SPL").await;
    api_put_expecting(
        &pool,
        serde_json::json!({
            "action_type": "ShareSplit",
            "listing_id": 1,
            "date": "2024-11-30",
            "split_new_units": "2",
            "split_old_units": "1",
        }),
        StatusCode::CREATED,
    )
    .await;
    let got = db_get(&pool, 1).await.unwrap().unwrap();
    assert_eq!(
        got.kind,
        ActionKind::ShareSplit {
            split_new_units: Decimal::from(2),
            split_old_units: Decimal::ONE,
        }
    );
}

#[tokio::test]
async fn api_bonus_issue_round_trip() {
    let pool = test_pool().await;
    insert_listing(&pool, 1, "BON").await;
    api_put_expecting(
        &pool,
        serde_json::json!({
            "action_type": "BonusIssue",
            "listing_id": 1,
            "date": "2024-11-30",
            "bonus_units": "1",
            "bonus_held_units": "10",
        }),
        StatusCode::CREATED,
    )
    .await;
    let got = db_get(&pool, 1).await.unwrap().unwrap();
    assert_eq!(
        got.kind,
        ActionKind::BonusIssue {
            bonus_units: Decimal::ONE,
            bonus_held_units: Decimal::from(10),
        }
    );
}

#[tokio::test]
async fn api_rights_issue_round_trip() {
    let pool = test_pool().await;
    insert_listing(&pool, 1, "RTS").await;
    api_put_expecting(
        &pool,
        serde_json::json!({
            "action_type": "RightsIssue",
            "listing_id": 1,
            "date": "2024-11-30",
            "rights_units": "1",
            "rights_held_units": "4",
            "exercise_price": "1.80",
            "currency": "AUD",
            "renounceable": true,
        }),
        StatusCode::CREATED,
    )
    .await;
    let got = db_get(&pool, 1).await.unwrap().unwrap();
    assert_eq!(
        got.kind,
        ActionKind::RightsIssue {
            rights_units: Decimal::ONE,
            rights_held_units: Decimal::from(4),
            exercise_price: "1.80".parse().unwrap(),
            currency: "AUD".into(),
            renounceable: true,
        }
    );
}

/// SCENARIOS AA-b. Whether the offer was renounceable is a term of the
/// action like any other: it round-trips through the PUT, it reads back on
/// the GET, and `false` — the offer whose retail premium is an unfranked
/// dividend rather than capital proceeds — is a state the row can hold.
#[tokio::test]
async fn api_non_renounceable_rights_issue_round_trip() {
    let pool = test_pool().await;
    insert_listing(&pool, 1, "RTS").await;
    api_put_expecting(
        &pool,
        serde_json::json!({
            "action_type": "RightsIssue",
            "listing_id": 1,
            "date": "2024-11-30",
            "rights_units": "1",
            "rights_held_units": "4",
            "exercise_price": "1.80",
            "currency": "AUD",
            "renounceable": false,
        }),
        StatusCode::CREATED,
    )
    .await;
    let got = db_get(&pool, 1).await.unwrap().unwrap();
    assert_eq!(
        got.kind,
        ActionKind::RightsIssue {
            rights_units: Decimal::ONE,
            rights_held_units: Decimal::from(4),
            exercise_price: "1.80".parse().unwrap(),
            currency: "AUD".into(),
            renounceable: false,
        }
    );
    // And it is on the JSON the UI reads back, not only in the row.
    let resp = client(&pool).get("/corporate_actions/1").await;
    let json: serde_json::Value = resp.json();
    assert_eq!(json["renounceable"], serde_json::json!(false));
}

/// The flag is **required** on a rights issue and forbidden on every other
/// type. Required because the whole finding was that the fact was never
/// asked for: a quiet default would leave the same assumption in place for
/// every new entry, and the offer document always states it.
#[tokio::test]
async fn api_a_rights_issue_without_the_renounceable_flag_is_422() {
    let pool = test_pool().await;
    insert_listing(&pool, 1, "RTS").await;
    api_put_expecting(
        &pool,
        serde_json::json!({
            "action_type": "RightsIssue", "listing_id": 1, "date": "2024-11-30",
            "rights_units": "1", "rights_held_units": "4", "exercise_price": "1.80",
            "currency": "AUD",
        }),
        StatusCode::UNPROCESSABLE_ENTITY,
    )
    .await;
    for stray in [
        serde_json::json!({
            "action_type": "ShareSplit", "listing_id": 1, "date": "2024-11-30",
            "split_new_units": "2", "split_old_units": "1", "renounceable": true,
        }),
        serde_json::json!({
            "action_type": "BonusIssue", "listing_id": 1, "date": "2024-11-30",
            "bonus_units": "1", "bonus_held_units": "10", "renounceable": false,
        }),
        serde_json::json!({
            "action_type": "ReturnOfCapital", "listing_id": 1, "date": "2024-11-30",
            "amount_per_unit": "0.50", "currency": "AUD", "renounceable": true,
        }),
    ] {
        api_put_expecting(&pool, stray, StatusCode::UNPROCESSABLE_ENTITY).await;
    }
    let n: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM corporate_actions")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(n, 0, "nothing was written");
}

#[tokio::test]
async fn api_invalid_rights_issue_payloads_return_422() {
    let pool = test_pool().await;
    insert_listing(&pool, 1, "RTS").await;
    // Missing terms, a missing currency, non-positive ratio/price, a
    // stray payment amount, a stray split ratio — and the ratio-only
    // types carrying rights fields or a currency.
    for body in [
        serde_json::json!({
            "action_type": "RightsIssue", "listing_id": 1, "date": "2024-11-30",
        }),
        serde_json::json!({
            "action_type": "RightsIssue", "listing_id": 1, "date": "2024-11-30",
            "rights_units": "1", "rights_held_units": "4", "exercise_price": "1.80",
        }),
        serde_json::json!({
            "action_type": "RightsIssue", "listing_id": 1, "date": "2024-11-30",
            "rights_units": "0", "rights_held_units": "4", "exercise_price": "1.80",
            "currency": "AUD", "renounceable": true,
        }),
        serde_json::json!({
            "action_type": "RightsIssue", "listing_id": 1, "date": "2024-11-30",
            "rights_units": "1", "rights_held_units": "-4", "exercise_price": "1.80",
            "currency": "AUD", "renounceable": true,
        }),
        serde_json::json!({
            "action_type": "RightsIssue", "listing_id": 1, "date": "2024-11-30",
            "rights_units": "1", "rights_held_units": "4", "exercise_price": "0",
            "currency": "AUD", "renounceable": true,
        }),
        serde_json::json!({
            "action_type": "RightsIssue", "listing_id": 1, "date": "2024-11-30",
            "rights_units": "1", "rights_held_units": "4", "exercise_price": "1.80",
            "currency": "AUD", "renounceable": true, "amount_per_unit": "0.50",
        }),
        serde_json::json!({
            "action_type": "RightsIssue", "listing_id": 1, "date": "2024-11-30",
            "rights_units": "1", "rights_held_units": "4", "exercise_price": "1.80",
            "currency": "AUD", "renounceable": true, "split_new_units": "2",
            "split_old_units": "1",
        }),
        serde_json::json!({
            "action_type": "ShareSplit", "listing_id": 1, "date": "2024-11-30",
            "split_new_units": "2", "split_old_units": "1",
            "rights_units": "1", "rights_held_units": "4", "exercise_price": "1.80",
            "currency": "AUD",
        }),
        serde_json::json!({
            "action_type": "BonusIssue", "listing_id": 1, "date": "2024-11-30",
            "bonus_units": "1", "bonus_held_units": "10", "currency": "AUD",
        }),
    ] {
        api_put_expecting(&pool, body, StatusCode::UNPROCESSABLE_ENTITY).await;
    }
}

#[tokio::test]
async fn api_buy_back_round_trip() {
    let pool = test_pool().await;
    insert_listing(&pool, 1, "BBK").await;
    api_put_expecting(
        &pool,
        serde_json::json!({
            "action_type": "BuyBack",
            "listing_id": 1,
            "date": "2024-11-30",
            "buyback_price": "9.60",
            "buyback_dividend": "1.40",
            "buyback_franking_credit": "0.60",
            "buyback_market_value": "10.20",
            "currency": "AUD",
        }),
        StatusCode::CREATED,
    )
    .await;
    let got = db_get(&pool, 1).await.unwrap().unwrap();
    assert_eq!(
        got.kind,
        ActionKind::BuyBack {
            buyback_price: "9.60".parse().unwrap(),
            buyback_dividend: "1.40".parse().unwrap(),
            buyback_franking_credit: "0.60".parse().unwrap(),
            buyback_market_value: Some("10.20".parse().unwrap()),
            currency: "AUD".into(),
        }
    );

    // The no-dividend (listed post-Oct-2022) shape: dividend/credit
    // default to 0 and the market value may be omitted.
    api_put_expecting(
        &pool,
        serde_json::json!({
            "action_type": "BuyBack",
            "listing_id": 1,
            "date": "2024-12-31",
            "buyback_price": "5.00",
            "currency": "AUD",
        }),
        // A second PUT to the same id replaces the stored row.
        StatusCode::NO_CONTENT,
    )
    .await;
    let got = db_get(&pool, 1).await.unwrap().unwrap();
    assert_eq!(
        got.kind,
        ActionKind::BuyBack {
            buyback_price: "5.00".parse().unwrap(),
            buyback_dividend: Decimal::ZERO,
            buyback_franking_credit: Decimal::ZERO,
            buyback_market_value: None,
            currency: "AUD".into(),
        }
    );
}

#[tokio::test]
async fn api_invalid_buy_back_payloads_return_422() {
    let pool = test_pool().await;
    insert_listing(&pool, 1, "BBK").await;
    // Missing/non-positive price, a missing currency, a dividend
    // exceeding the price (it is a component of it), a negative
    // dividend, a credit without a dividend to attach to, a negative
    // credit, a non-positive market value, stray cross-type fields —
    // and the other types carrying buy-back fields.
    for body in [
        serde_json::json!({
            "action_type": "BuyBack", "listing_id": 1, "date": "2024-11-30",
            "currency": "AUD",
        }),
        serde_json::json!({
            "action_type": "BuyBack", "listing_id": 1, "date": "2024-11-30",
            "buyback_price": "0", "currency": "AUD",
        }),
        serde_json::json!({
            "action_type": "BuyBack", "listing_id": 1, "date": "2024-11-30",
            "buyback_price": "9.60",
        }),
        serde_json::json!({
            "action_type": "BuyBack", "listing_id": 1, "date": "2024-11-30",
            "buyback_price": "9.60", "buyback_dividend": "9.61", "currency": "AUD",
        }),
        serde_json::json!({
            "action_type": "BuyBack", "listing_id": 1, "date": "2024-11-30",
            "buyback_price": "9.60", "buyback_dividend": "-1.40", "currency": "AUD",
        }),
        serde_json::json!({
            "action_type": "BuyBack", "listing_id": 1, "date": "2024-11-30",
            "buyback_price": "9.60", "buyback_franking_credit": "0.60", "currency": "AUD",
        }),
        serde_json::json!({
            "action_type": "BuyBack", "listing_id": 1, "date": "2024-11-30",
            "buyback_price": "9.60", "buyback_dividend": "1.40",
            "buyback_franking_credit": "-0.60", "currency": "AUD",
        }),
        serde_json::json!({
            "action_type": "BuyBack", "listing_id": 1, "date": "2024-11-30",
            "buyback_price": "9.60", "buyback_market_value": "0", "currency": "AUD",
        }),
        serde_json::json!({
            "action_type": "BuyBack", "listing_id": 1, "date": "2024-11-30",
            "buyback_price": "9.60", "currency": "AUD",
            "split_new_units": "2", "split_old_units": "1",
        }),
        serde_json::json!({
            "action_type": "BuyBack", "listing_id": 1, "date": "2024-11-30",
            "buyback_price": "9.60", "currency": "AUD", "amount_per_unit": "0.50",
        }),
        serde_json::json!({
            "action_type": "ShareSplit", "listing_id": 1, "date": "2024-11-30",
            "split_new_units": "2", "split_old_units": "1", "buyback_price": "9.60",
        }),
        serde_json::json!({
            "action_type": "ReturnOfCapital", "listing_id": 1, "date": "2024-11-30",
            "amount_per_unit": "0.50", "currency": "AUD", "buyback_dividend": "1.40",
        }),
        serde_json::json!({
            "action_type": "RightsIssue", "listing_id": 1, "date": "2024-11-30",
            "rights_units": "1", "rights_held_units": "4", "exercise_price": "1.80",
            "currency": "AUD", "renounceable": true, "buyback_market_value": "10.20",
        }),
    ] {
        api_put_expecting(&pool, body, StatusCode::UNPROCESSABLE_ENTITY).await;
    }
}

#[tokio::test]
async fn api_invalid_bonus_issue_payloads_return_422() {
    let pool = test_pool().await;
    insert_listing(&pool, 1, "BON").await;
    // Missing ratio, non-positive ratio, a stray payment field, a stray
    // split ratio — and a ShareSplit carrying a bonus ratio.
    for body in [
        serde_json::json!({
            "action_type": "BonusIssue", "listing_id": 1, "date": "2024-11-30",
        }),
        serde_json::json!({
            "action_type": "BonusIssue", "listing_id": 1, "date": "2024-11-30",
            "bonus_units": "0", "bonus_held_units": "10",
        }),
        serde_json::json!({
            "action_type": "BonusIssue", "listing_id": 1, "date": "2024-11-30",
            "bonus_units": "1", "bonus_held_units": "-10",
        }),
        serde_json::json!({
            "action_type": "BonusIssue", "listing_id": 1, "date": "2024-11-30",
            "bonus_units": "1", "bonus_held_units": "10",
            "amount_per_unit": "0.50", "currency": "AUD",
        }),
        serde_json::json!({
            "action_type": "BonusIssue", "listing_id": 1, "date": "2024-11-30",
            "bonus_units": "1", "bonus_held_units": "10",
            "split_new_units": "2", "split_old_units": "1",
        }),
        serde_json::json!({
            "action_type": "ShareSplit", "listing_id": 1, "date": "2024-11-30",
            "split_new_units": "2", "split_old_units": "1",
            "bonus_units": "1", "bonus_held_units": "10",
        }),
    ] {
        api_put_expecting(&pool, body, StatusCode::UNPROCESSABLE_ENTITY).await;
    }
}

#[tokio::test]
async fn api_scrip_for_scrip_round_trip() {
    let pool = test_pool().await;
    insert_listing(&pool, 1, "OLD").await;
    insert_listing(&pool, 2, "NEW").await;
    api_put_expecting(
        &pool,
        serde_json::json!({
            "action_type": "ScripForScrip",
            "listing_id": 1,
            "date": "2024-11-30",
            "scrip_listing_id": 2,
            "scrip_new_units": "2",
            "scrip_old_units": "1",
        }),
        StatusCode::CREATED,
    )
    .await;
    let got = db_get(&pool, 1).await.unwrap().unwrap();
    assert_eq!(
        got.kind,
        ActionKind::ScripForScrip {
            scrip_listing_id: ListingId(2),
            scrip_new_units: Decimal::from(2),
            scrip_old_units: Decimal::ONE,
            scrip_cash_per_unit: None,
            scrip_market_value: None,
            scrip_cash_currency: None,
        }
    );
}

/// The optional cash component (partial rollover, Example 27) round-trips
/// with sub-cent precision.
#[tokio::test]
async fn api_scrip_for_scrip_cash_component_round_trip() {
    let pool = test_pool().await;
    insert_listing(&pool, 1, "OLD").await;
    insert_listing(&pool, 2, "NEW").await;
    api_put_expecting(
        &pool,
        serde_json::json!({
            "action_type": "ScripForScrip", "listing_id": 1, "date": "2024-11-30",
            "scrip_listing_id": 2, "scrip_new_units": "1", "scrip_old_units": "1",
            "scrip_cash_per_unit": "10.005", "scrip_market_value": "20.105",
            "scrip_cash_currency": "AUD",
        }),
        StatusCode::CREATED,
    )
    .await;
    let got = db_get(&pool, 1).await.unwrap().unwrap();
    assert_eq!(
        got.kind,
        ActionKind::ScripForScrip {
            scrip_listing_id: ListingId(2),
            scrip_new_units: Decimal::ONE,
            scrip_old_units: Decimal::ONE,
            scrip_cash_per_unit: Some("10.005".parse().unwrap()),
            scrip_market_value: Some("20.105".parse().unwrap()),
            scrip_cash_currency: Some("AUD".into()),
        }
    );
}

/// The cash component is all-or-none (cash, market value, currency) and
/// both amounts must be positive; cash fields on another action type are
/// stray.
#[tokio::test]
async fn api_invalid_scrip_cash_payloads_return_422() {
    let pool = test_pool().await;
    insert_listing(&pool, 1, "OLD").await;
    insert_listing(&pool, 2, "NEW").await;
    let scrip_base = serde_json::json!({
        "action_type": "ScripForScrip", "listing_id": 1, "date": "2024-11-30",
        "scrip_listing_id": 2, "scrip_new_units": "1", "scrip_old_units": "1",
    });
    let with = |extra: serde_json::Value| {
        let mut body = scrip_base.clone();
        body.as_object_mut()
            .unwrap()
            .extend(extra.as_object().unwrap().clone());
        body
    };
    for body in [
        // Partial sets.
        with(serde_json::json!({ "scrip_cash_per_unit": "10" })),
        with(serde_json::json!({ "scrip_cash_per_unit": "10", "scrip_market_value": "20" })),
        with(serde_json::json!({ "scrip_market_value": "20", "scrip_cash_currency": "AUD" })),
        with(serde_json::json!({ "scrip_cash_currency": "AUD" })),
        // Non-positive amounts.
        with(serde_json::json!({
            "scrip_cash_per_unit": "0", "scrip_market_value": "20",
            "scrip_cash_currency": "AUD",
        })),
        with(serde_json::json!({
            "scrip_cash_per_unit": "10", "scrip_market_value": "-20",
            "scrip_cash_currency": "AUD",
        })),
        // An unknown cash currency (FK).
        with(serde_json::json!({
            "scrip_cash_per_unit": "10", "scrip_market_value": "20",
            "scrip_cash_currency": "ZZZ",
        })),
        // The shared `currency` column stays forbidden for ScripForScrip.
        with(serde_json::json!({
            "scrip_cash_per_unit": "10", "scrip_market_value": "20",
            "scrip_cash_currency": "AUD", "currency": "AUD",
        })),
        // Cash fields are stray on another action type.
        serde_json::json!({
            "action_type": "ShareSplit", "listing_id": 1, "date": "2024-11-30",
            "split_new_units": "2", "split_old_units": "1",
            "scrip_cash_per_unit": "10", "scrip_market_value": "20",
            "scrip_cash_currency": "AUD",
        }),
    ] {
        api_put_expecting(&pool, body, StatusCode::UNPROCESSABLE_ENTITY).await;
    }
}

#[tokio::test]
async fn api_invalid_scrip_for_scrip_payloads_return_422() {
    let pool = test_pool().await;
    insert_listing(&pool, 1, "OLD").await;
    insert_listing(&pool, 2, "NEW").await;
    // Missing terms, a non-positive ratio, the same listing on both
    // sides, an unknown replacement listing, a stray currency, stray
    // cross-type fields — and the other types carrying scrip fields.
    for body in [
        serde_json::json!({
            "action_type": "ScripForScrip", "listing_id": 1, "date": "2024-11-30",
        }),
        serde_json::json!({
            "action_type": "ScripForScrip", "listing_id": 1, "date": "2024-11-30",
            "scrip_listing_id": 2, "scrip_new_units": "0", "scrip_old_units": "1",
        }),
        serde_json::json!({
            "action_type": "ScripForScrip", "listing_id": 1, "date": "2024-11-30",
            "scrip_listing_id": 2, "scrip_new_units": "2", "scrip_old_units": "-1",
        }),
        serde_json::json!({
            "action_type": "ScripForScrip", "listing_id": 1, "date": "2024-11-30",
            "scrip_listing_id": 1, "scrip_new_units": "2", "scrip_old_units": "1",
        }),
        serde_json::json!({
            "action_type": "ScripForScrip", "listing_id": 1, "date": "2024-11-30",
            "scrip_listing_id": 999, "scrip_new_units": "2", "scrip_old_units": "1",
        }),
        serde_json::json!({
            "action_type": "ScripForScrip", "listing_id": 1, "date": "2024-11-30",
            "scrip_listing_id": 2, "scrip_new_units": "2", "scrip_old_units": "1",
            "currency": "AUD",
        }),
        serde_json::json!({
            "action_type": "ScripForScrip", "listing_id": 1, "date": "2024-11-30",
            "scrip_listing_id": 2, "scrip_new_units": "2", "scrip_old_units": "1",
            "amount_per_unit": "0.50",
        }),
        serde_json::json!({
            "action_type": "ScripForScrip", "listing_id": 1, "date": "2024-11-30",
            "scrip_listing_id": 2, "scrip_new_units": "2", "scrip_old_units": "1",
            "split_new_units": "2", "split_old_units": "1",
        }),
        serde_json::json!({
            "action_type": "ShareSplit", "listing_id": 1, "date": "2024-11-30",
            "split_new_units": "2", "split_old_units": "1",
            "scrip_listing_id": 2, "scrip_new_units": "2", "scrip_old_units": "1",
        }),
        serde_json::json!({
            "action_type": "BuyBack", "listing_id": 1, "date": "2024-11-30",
            "buyback_price": "9.60", "currency": "AUD", "scrip_listing_id": 2,
        }),
    ] {
        api_put_expecting(&pool, body, StatusCode::UNPROCESSABLE_ENTITY).await;
    }
}

#[tokio::test]
async fn api_demerger_round_trip() {
    let pool = test_pool().await;
    insert_listing(&pool, 1, "HEAD").await;
    insert_listing(&pool, 2, "DEM").await;
    api_put_expecting(
        &pool,
        serde_json::json!({
            "action_type": "Demerger",
            "listing_id": 1,
            "date": "2024-11-30",
            "demerger_listing_id": 2,
            "demerger_new_units": "1",
            "demerger_held_units": "5",
            "demerger_cost_base_pct": "5.063",
        }),
        StatusCode::CREATED,
    )
    .await;
    let got = db_get(&pool, 1).await.unwrap().unwrap();
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

#[tokio::test]
async fn api_invalid_demerger_payloads_return_422() {
    let pool = test_pool().await;
    insert_listing(&pool, 1, "HEAD").await;
    insert_listing(&pool, 2, "DEM").await;
    // Missing terms, a non-positive ratio, a percentage at/outside the
    // (0, 100) bounds, the same listing on both sides, an unknown
    // demerged listing, a stray currency, stray cross-type fields — and
    // the other types carrying demerger fields.
    for body in [
        serde_json::json!({
            "action_type": "Demerger", "listing_id": 1, "date": "2024-11-30",
        }),
        serde_json::json!({
            "action_type": "Demerger", "listing_id": 1, "date": "2024-11-30",
            "demerger_listing_id": 2, "demerger_new_units": "0",
            "demerger_held_units": "5", "demerger_cost_base_pct": "5.063",
        }),
        serde_json::json!({
            "action_type": "Demerger", "listing_id": 1, "date": "2024-11-30",
            "demerger_listing_id": 2, "demerger_new_units": "1",
            "demerger_held_units": "-5", "demerger_cost_base_pct": "5.063",
        }),
        serde_json::json!({
            "action_type": "Demerger", "listing_id": 1, "date": "2024-11-30",
            "demerger_listing_id": 2, "demerger_new_units": "1",
            "demerger_held_units": "5", "demerger_cost_base_pct": "0",
        }),
        serde_json::json!({
            "action_type": "Demerger", "listing_id": 1, "date": "2024-11-30",
            "demerger_listing_id": 2, "demerger_new_units": "1",
            "demerger_held_units": "5", "demerger_cost_base_pct": "100",
        }),
        serde_json::json!({
            "action_type": "Demerger", "listing_id": 1, "date": "2024-11-30",
            "demerger_listing_id": 2, "demerger_new_units": "1",
            "demerger_held_units": "5", "demerger_cost_base_pct": "-5.063",
        }),
        serde_json::json!({
            "action_type": "Demerger", "listing_id": 1, "date": "2024-11-30",
            "demerger_listing_id": 1, "demerger_new_units": "1",
            "demerger_held_units": "5", "demerger_cost_base_pct": "5.063",
        }),
        serde_json::json!({
            "action_type": "Demerger", "listing_id": 1, "date": "2024-11-30",
            "demerger_listing_id": 999, "demerger_new_units": "1",
            "demerger_held_units": "5", "demerger_cost_base_pct": "5.063",
        }),
        serde_json::json!({
            "action_type": "Demerger", "listing_id": 1, "date": "2024-11-30",
            "demerger_listing_id": 2, "demerger_new_units": "1",
            "demerger_held_units": "5", "demerger_cost_base_pct": "5.063",
            "currency": "AUD",
        }),
        serde_json::json!({
            "action_type": "Demerger", "listing_id": 1, "date": "2024-11-30",
            "demerger_listing_id": 2, "demerger_new_units": "1",
            "demerger_held_units": "5", "demerger_cost_base_pct": "5.063",
            "split_new_units": "2", "split_old_units": "1",
        }),
        serde_json::json!({
            "action_type": "Demerger", "listing_id": 1, "date": "2024-11-30",
            "demerger_listing_id": 2, "demerger_new_units": "1",
            "demerger_held_units": "5", "demerger_cost_base_pct": "5.063",
            "scrip_listing_id": 2, "scrip_new_units": "2", "scrip_old_units": "1",
        }),
        serde_json::json!({
            "action_type": "ShareSplit", "listing_id": 1, "date": "2024-11-30",
            "split_new_units": "2", "split_old_units": "1",
            "demerger_listing_id": 2, "demerger_new_units": "1",
            "demerger_held_units": "5", "demerger_cost_base_pct": "5.063",
        }),
        serde_json::json!({
            "action_type": "ScripForScrip", "listing_id": 1, "date": "2024-11-30",
            "scrip_listing_id": 2, "scrip_new_units": "2", "scrip_old_units": "1",
            "demerger_cost_base_pct": "5.063",
        }),
        // A stated close on a type that has no price series to restate.
        serde_json::json!({
            "action_type": "ShareSplit", "listing_id": 1, "date": "2024-11-30",
            "split_new_units": "2", "split_old_units": "1",
            "demerger_close_date": "2024-11-29", "demerger_close_price": "24.90",
            "demerger_close_sourced_from": "nyse.com", "demerger_close_reason": "spin-off",
        }),
    ] {
        api_put_expecting(&pool, body, StatusCode::UNPROCESSABLE_ENTITY).await;
    }
}

/// The stated pre-demerger close round-trips with its provenance, and the
/// write-time refusals around it: it is all-or-none, the day must be
/// strictly before the demerger, the close positive, and neither
/// provenance field blank — the same shape `PUT /closing_prices/…` applies
/// to a hand-entered price, for the same reason.
#[tokio::test]
async fn api_demerger_stated_close_round_trips_and_rejects_a_partial_one() {
    let pool = test_pool().await;
    insert_listing(&pool, 1, "LAC").await;
    insert_listing(&pool, 2, "LAR").await;
    let complete = serde_json::json!({
        "action_type": "Demerger", "listing_id": 1, "date": "2023-10-03",
        "demerger_listing_id": 2, "demerger_new_units": "1",
        "demerger_held_units": "1", "demerger_cost_base_pct": "36",
        "demerger_close_date": "2023-10-02", "demerger_close_price": "24.90",
        "demerger_close_sourced_from": "  nyse.com daily close  ",
        "demerger_close_reason": "the provider adjusts the pre-demerger series",
    });
    api_put_expecting(&pool, complete.clone(), StatusCode::CREATED).await;
    assert_eq!(
        db_get(&pool, 1).await.unwrap().unwrap().kind,
        ActionKind::Demerger {
            demerger_listing_id: ListingId(2),
            demerger_new_units: Decimal::ONE,
            demerger_held_units: Decimal::ONE,
            demerger_cost_base_pct: Decimal::from(36),
            demerger_close_date: Some(d(2023, 10, 2)),
            demerger_close_price: Some("24.90".parse().unwrap()),
            demerger_close_sourced_from: Some("nyse.com daily close".to_owned()),
            demerger_close_reason: Some("the provider adjusts the pre-demerger series".to_owned()),
        },
        "the provenance is stored trimmed, beside the two sides of the factor"
    );

    let without = |field: &str| {
        let mut body = complete.clone();
        body.as_object_mut().unwrap().remove(field);
        body
    };
    let with = |field: &str, value: serde_json::Value| {
        let mut body = complete.clone();
        body[field] = value;
        body
    };
    for body in [
        // Each of the four missing on its own: a partial statement leaves
        // a factor with one side, or a figure with no recorded source.
        without("demerger_close_date"),
        without("demerger_close_price"),
        without("demerger_close_sourced_from"),
        without("demerger_close_reason"),
        // The close of the demerger date itself, or a later day, is
        // already in the post-demerger basis.
        with("demerger_close_date", serde_json::json!("2023-10-03")),
        with("demerger_close_date", serde_json::json!("2023-10-04")),
        // A non-positive close would make the factor zero or negative.
        with("demerger_close_price", serde_json::json!("0")),
        with("demerger_close_price", serde_json::json!("-24.90")),
        // Provenance that records nothing.
        with("demerger_close_sourced_from", serde_json::json!("   ")),
        with("demerger_close_reason", serde_json::json!("")),
    ] {
        api_put_expecting(&pool, body, StatusCode::UNPROCESSABLE_ENTITY).await;
    }
}

#[tokio::test]
async fn db_insert_and_retrieve_worthless_shares_preserves_event() {
    let pool = test_pool().await;
    insert_listing(&pool, 1, "DEAD").await;
    for (id, event) in [
        (1, WorthlessEvent::G3Declaration),
        (2, WorthlessEvent::C2Cancellation),
    ] {
        db_upsert(
            &pool,
            &CorporateAction {
                id,
                listing_id: ListingId(1),
                date: d(2025, 3, 31),
                kind: ActionKind::WorthlessShares {
                    worthless_event: event,
                },
            },
        )
        .await
        .unwrap();
        let got = db_get(&pool, id).await.unwrap().unwrap();
        assert_eq!(
            got.kind,
            ActionKind::WorthlessShares {
                worthless_event: event
            }
        );
    }
}

/// A WorthlessShares action never appears in the split-event or
/// return-of-capital streams — recording one changes no existing parcel
/// (the recognise operation closes the holding).
#[tokio::test]
async fn db_worthless_shares_is_not_a_split_or_payment_event() {
    let pool = test_pool().await;
    insert_listing(&pool, 1, "DEAD").await;
    db_upsert(
        &pool,
        &CorporateAction {
            id: 1,
            listing_id: ListingId(1),
            date: d(2025, 3, 31),
            kind: ActionKind::WorthlessShares {
                worthless_event: WorthlessEvent::G3Declaration,
            },
        },
    )
    .await
    .unwrap();
    assert!(db_share_split_events(&pool).await.unwrap().is_empty());
    assert!(db_return_of_capital_events(&pool).await.unwrap().is_empty());
}

#[tokio::test]
async fn api_worthless_shares_round_trip() {
    let pool = test_pool().await;
    insert_listing(&pool, 1, "DEAD").await;
    api_put_expecting(
        &pool,
        serde_json::json!({
            "action_type": "WorthlessShares",
            "listing_id": 1,
            "date": "2025-03-31",
            "worthless_event": "G3Declaration",
        }),
        StatusCode::CREATED,
    )
    .await;
    let got = db_get(&pool, 1).await.unwrap().unwrap();
    assert_eq!(
        got.kind,
        ActionKind::WorthlessShares {
            worthless_event: WorthlessEvent::G3Declaration
        }
    );
}

#[tokio::test]
async fn api_invalid_worthless_shares_payloads_return_422() {
    let pool = test_pool().await;
    insert_listing(&pool, 1, "DEAD").await;
    // Missing the event discriminator, an unknown event value, a stray
    // currency, stray cross-type payload fields — and another type
    // carrying a worthless_event.
    for body in [
        serde_json::json!({
            "action_type": "WorthlessShares", "listing_id": 1, "date": "2025-03-31",
        }),
        serde_json::json!({
            "action_type": "WorthlessShares", "listing_id": 1, "date": "2025-03-31",
            "worthless_event": "Nope",
        }),
        serde_json::json!({
            "action_type": "WorthlessShares", "listing_id": 1, "date": "2025-03-31",
            "worthless_event": "G3Declaration", "currency": "AUD",
        }),
        serde_json::json!({
            "action_type": "WorthlessShares", "listing_id": 1, "date": "2025-03-31",
            "worthless_event": "G3Declaration", "split_new_units": "2", "split_old_units": "1",
        }),
        serde_json::json!({
            "action_type": "ShareSplit", "listing_id": 1, "date": "2025-03-31",
            "split_new_units": "2", "split_old_units": "1",
            "worthless_event": "G3Declaration",
        }),
    ] {
        api_put_expecting(&pool, body, StatusCode::UNPROCESSABLE_ENTITY).await;
    }
}

#[tokio::test]
async fn api_non_positive_amount_returns_422() {
    let pool = test_pool().await;
    insert_listing(&pool, 1, "RAP").await;
    for amount in ["0", "-0.50"] {
        api_put_expecting(
            &pool,
            serde_json::json!({
                "action_type": "ReturnOfCapital",
                "listing_id": 1,
                "date": "2024-11-30",
                "amount_per_unit": amount,
                "currency": "AUD",
            }),
            StatusCode::UNPROCESSABLE_ENTITY,
        )
        .await;
    }
}

/// A return of capital may carry the record date that fixed entitlement to
/// it; it round-trips on the flat wire shape like any other field.
#[tokio::test]
async fn api_return_of_capital_record_date_round_trip() {
    let pool = test_pool().await;
    insert_listing(&pool, 1, "RAP").await;
    api_put_expecting(
        &pool,
        serde_json::json!({
            "action_type": "ReturnOfCapital",
            "listing_id": 1,
            "date": "2025-03-01",
            "amount_per_unit": "0.50",
            "currency": "AUD",
            "record_date": "2025-02-10",
        }),
        StatusCode::CREATED,
    )
    .await;

    let resp = client(&pool).get("/corporate_actions/1").await;
    let got: CorporateAction = resp.json();
    assert_eq!(
        got.kind,
        roc_with_record(1, ListingId(1), d(2025, 3, 1), "0.50", d(2025, 2, 10)).kind
    );
    let raw: serde_json::Value = client(&pool).get("/corporate_actions/1").await.json();
    assert_eq!(raw["record_date"], "2025-02-10");
}

/// A record date after the payment it entitles the holder to is a
/// contradiction, and no other action type has one at all — both rejected
/// rather than silently dropped.
#[tokio::test]
async fn api_invalid_record_dates_return_422() {
    let pool = test_pool().await;
    insert_listing(&pool, 1, "RAP").await;
    for body in [
        serde_json::json!({
            "action_type": "ReturnOfCapital",
            "listing_id": 1,
            "date": "2025-03-01",
            "amount_per_unit": "0.50",
            "currency": "AUD",
            "record_date": "2025-03-02",
        }),
        serde_json::json!({
            "action_type": "ShareSplit",
            "listing_id": 1,
            "date": "2025-03-01",
            "split_new_units": "2",
            "split_old_units": "1",
            "record_date": "2025-02-10",
        }),
        serde_json::json!({
            "action_type": "RightsIssue",
            "listing_id": 1,
            "date": "2025-03-01",
            "rights_units": "1",
            "rights_held_units": "4",
            "exercise_price": "1.80",
            "currency": "AUD",
            "renounceable": true,
            "record_date": "2025-02-10",
        }),
    ] {
        api_put_expecting(&pool, body, StatusCode::UNPROCESSABLE_ENTITY).await;
    }
    // The payment date itself is a legal record date (a same-day fixing).
    api_put_expecting(
        &pool,
        serde_json::json!({
            "action_type": "ReturnOfCapital",
            "listing_id": 1,
            "date": "2025-03-01",
            "amount_per_unit": "0.50",
            "currency": "AUD",
            "record_date": "2025-03-01",
        }),
        StatusCode::CREATED,
    )
    .await;
}

#[tokio::test]
async fn api_invalid_share_split_payloads_return_422() {
    let pool = test_pool().await;
    insert_listing(&pool, 1, "SPL").await;
    // Missing ratio, non-positive ratio, and a stray payment field.
    for body in [
        serde_json::json!({
            "action_type": "ShareSplit", "listing_id": 1, "date": "2024-11-30",
        }),
        serde_json::json!({
            "action_type": "ShareSplit", "listing_id": 1, "date": "2024-11-30",
            "split_new_units": "0", "split_old_units": "1",
        }),
        serde_json::json!({
            "action_type": "ShareSplit", "listing_id": 1, "date": "2024-11-30",
            "split_new_units": "2", "split_old_units": "-1",
        }),
        serde_json::json!({
            "action_type": "ShareSplit", "listing_id": 1, "date": "2024-11-30",
            "split_new_units": "2", "split_old_units": "1",
            "amount_per_unit": "0.50", "currency": "AUD",
        }),
        // …and a ReturnOfCapital carrying a split ratio.
        serde_json::json!({
            "action_type": "ReturnOfCapital", "listing_id": 1, "date": "2024-11-30",
            "amount_per_unit": "0.50", "currency": "AUD",
            "split_new_units": "2", "split_old_units": "1",
        }),
    ] {
        api_put_expecting(&pool, body, StatusCode::UNPROCESSABLE_ENTITY).await;
    }
}

#[tokio::test]
async fn api_unknown_listing_returns_422() {
    let pool = test_pool().await;
    api_put_expecting(
        &pool,
        serde_json::json!({
            "action_type": "ReturnOfCapital",
            "listing_id": 999,
            "date": "2024-11-30",
            "amount_per_unit": "0.50",
            "currency": "AUD",
        }),
        StatusCode::UNPROCESSABLE_ENTITY,
    )
    .await;
}

#[tokio::test]
async fn api_unknown_currency_returns_422() {
    let pool = test_pool().await;
    insert_listing(&pool, 1, "RAP").await;
    api_put_expecting(
        &pool,
        serde_json::json!({
            "action_type": "ReturnOfCapital",
            "listing_id": 1,
            "date": "2024-11-30",
            "amount_per_unit": "0.50",
            "currency": "ZZZ",
        }),
        StatusCode::UNPROCESSABLE_ENTITY,
    )
    .await;
}

#[tokio::test]
async fn api_unknown_action_type_rejected() {
    let pool = test_pool().await;
    insert_listing(&pool, 1, "RAP").await;
    // Serde rejects an unrecognised enum variant before it reaches the DB.
    api_put_expecting(
        &pool,
        serde_json::json!({
            "action_type": "Merger",
            "listing_id": 1,
            "date": "2024-11-30",
            "amount_per_unit": "0.50",
            "currency": "AUD",
        }),
        StatusCode::UNPROCESSABLE_ENTITY,
    )
    .await;
}
