//! The spot-rate override (QC 18020): write-time validation and round-trip.

use super::*;

#[tokio::test]
async fn db_spot_fx_rate_round_trips_with_precision() {
    let pool = test_pool().await;
    insert_usd_listing(&pool).await;
    test_support::buy(1, 1)
        .currency("USD")
        .spot_fx_rate("0.643215987".parse().unwrap())
        .insert(&pool)
        .await;
    let got = db_get(&pool, TradeId(1)).await.unwrap().unwrap();
    assert_eq!(
        got.spot_fx_rate,
        Some("0.643215987".parse::<Decimal>().unwrap())
    );
    // Absent an override the column stays NULL — the unchanged default.
    test_support::buy(2, 1).currency("USD").insert(&pool).await;
    assert_eq!(
        db_get(&pool, TradeId(2))
            .await
            .unwrap()
            .unwrap()
            .spot_fx_rate,
        None
    );
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
    assert!(db_get(&pool, TradeId(1)).await.unwrap().is_none());
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
    assert_eq!(resp.status, StatusCode::CREATED);
    let got = db_get(&pool, TradeId(1)).await.unwrap().unwrap();
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
