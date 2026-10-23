//! A payment must be in the currency of the parcels it reduces.

use super::*;

// Currency agreement between a payment and the parcels it reduces
//
// A return of capital reduces each parcel's cost base in the *parcel's*
// own currency, and amounts are never netted across currencies, so
// `RocEvent::per_unit_for` refuses to compute the mismatched pair. Nothing
// checked it at write time, so the pair was accepted and every cost-base
// report of the listing then answered 500 with an empty body (SCENARIOS
// E-07, E-39).

/// E-07: the typo — an AUD holding, a payment keyed as USD.
#[tokio::test]
async fn api_payment_in_another_currency_than_its_parcels_returns_422() {
    let pool = test_pool().await;
    insert_listing(&pool, 1, "AAA").await;
    // 100 units @ $10, in the listing's own AUD.
    test_support::buy(1, 1)
        .date(d(2023, 1, 10))
        .insert(&pool)
        .await;

    let payment = |currency: &str| {
        serde_json::json!({
            "action_type": "ReturnOfCapital",
            "listing_id": 1,
            "date": "2024-05-01",
            "amount_per_unit": "0.50",
            "currency": currency,
        })
    };
    let resp = client(&pool)
        .put("/corporate_actions/1", &payment("USD"))
        .await;
    assert_eq!(resp.status, StatusCode::UNPROCESSABLE_ENTITY);
    let detail = resp.text().to_owned();
    assert!(
        detail.contains("recorded in USD") && detail.contains("held in AUD"),
        "detail must name both currencies: {detail}"
    );
    assert!(
        db_get(&pool, 1).await.unwrap().is_none(),
        "nothing persisted"
    );
    // The reports the accepted pair used to kill still answer, because the
    // state that killed them can no longer be written.
    let full = ApiClient::full(&pool);
    assert_eq!(
        full.get("/portfolio/open_parcels").await.status,
        StatusCode::OK
    );
    // The same payment in the parcels' own currency is accepted.
    api_put_expecting(&pool, payment("AUD"), StatusCode::CREATED).await;
}

/// Re-denominate a parcel the way a **rollover** does: a scrip-for-scrip
/// or demerger replacement Buy carries its consumed parcel's currency and
/// FX rate onto the new listing, so the carried cost base is unchanged by
/// the substitution (`entities::rollover::insert_replacement_buy`, which
/// writes the row itself). That is the one writer that can leave a parcel
/// in a currency other than its listing's — `trade::db_upsert` refuses the
/// pair (SCENARIOS M-08) — and it is exactly the state this check exists
/// for, so the tests reproduce it directly rather than driving a whole
/// exchange.
async fn carried_over_from_a_rollover(pool: &SqlitePool, trade_id: TradeId, currency: &str) {
    sqlx::query("UPDATE trades SET currency = ?, brokerage_currency = ? WHERE id = ?")
        .bind(currency)
        .bind(currency)
        .bind(trade_id)
        .execute(pool)
        .await
        .unwrap();
}

/// E-39: the same trap without a typo — a scrip-for-scrip replacement
/// parcel keeps the *original's* currency (docs/API.md), so a USD-listed
/// security can hold AUD parcels, and recording that listing's return of
/// capital in its own listed currency — the obvious entry — is what breaks
/// the reports. Refused with the same 422 naming both sides.
#[tokio::test]
async fn api_payment_in_the_listed_currency_of_a_carried_over_parcel_returns_422() {
    let pool = test_pool().await;
    test_support::listing(1)
        .ticker("USL")
        .name("USL")
        .currency("USD")
        .security_type(listing::SecurityType::Share)
        .insert(&pool)
        .await;
    // The replacement parcel a rollover leaves behind: AUD cost base
    // carried onto a USD-listed security.
    test_support::buy(1, 1)
        .date(d(2023, 1, 10))
        .insert(&pool)
        .await;
    carried_over_from_a_rollover(&pool, TradeId(1), "AUD").await;

    let resp = client(&pool)
        .put(
            "/corporate_actions/1",
            &serde_json::json!({
                "action_type": "ReturnOfCapital",
                "listing_id": 1,
                "date": "2024-05-01",
                "amount_per_unit": "0.50",
                "currency": "USD",
            }),
        )
        .await;
    assert_eq!(resp.status, StatusCode::UNPROCESSABLE_ENTITY);
    let detail = resp.text().to_owned();
    assert!(
        detail.contains("recorded in USD") && detail.contains("held in AUD"),
        "detail must name both currencies: {detail}"
    );
}

/// The check is scoped to the parcels the payment actually *reaches* — the
/// same entitlement test `RocEvent::per_unit_for` applies — so a parcel in
/// another currency that the payment never reduces is no obstacle.
#[tokio::test]
async fn api_payment_currency_check_covers_only_the_parcels_it_reaches() {
    let pool = test_pool().await;
    insert_listing(&pool, 1, "AAA").await;
    test_support::buy(1, 1)
        .date(d(2023, 1, 10))
        .insert(&pool)
        .await;
    test_support::buy(2, 1)
        .date(d(2025, 1, 6))
        .insert(&pool)
        .await;
    carried_over_from_a_rollover(&pool, TradeId(2), "USD").await;

    // Dated before the USD parcel was acquired: it was never entitled.
    api_put_expecting(
        &pool,
        serde_json::json!({
            "action_type": "ReturnOfCapital",
            "listing_id": 1,
            "date": "2024-06-01",
            "amount_per_unit": "0.50",
            "currency": "AUD",
        }),
        StatusCode::CREATED,
    )
    .await;
    // With a record date it is entitlement at *that* date that decides,
    // and a parcel joins the register when it *settles*: bought Mon 6 Jan
    // 2025 and settled Wed 8 Jan, it is ex-entitlement to a record date
    // of 7 Jan though traded before it …
    let payment = |record: &str| {
        serde_json::json!({
            "action_type": "ReturnOfCapital",
            "listing_id": 1,
            "date": "2025-06-01",
            "amount_per_unit": "0.50",
            "currency": "AUD",
            "record_date": record,
        })
    };
    let resp = client(&pool)
        .put("/corporate_actions/2", &payment("2025-01-07"))
        .await;
    assert_eq!(resp.status, StatusCode::CREATED);
    // … while with the record date on its settlement date it is entitled,
    // and the same edit is refused — the check runs over the state the
    // write would leave.
    let resp = client(&pool)
        .put("/corporate_actions/2", &payment("2025-01-08"))
        .await;
    assert_eq!(resp.status, StatusCode::UNPROCESSABLE_ENTITY);
    let stored = db_get(&pool, 2).await.unwrap().unwrap();
    assert_eq!(
        stored.kind,
        roc_with_record(2, ListingId(1), d(2025, 6, 1), "0.50", d(2025, 1, 7)).kind,
        "the refused edit left the stored terms untouched"
    );
}
