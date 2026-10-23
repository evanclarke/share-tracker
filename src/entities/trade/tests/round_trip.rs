//! A trade row written and read back: each type, precision, and the
//! currency foreign keys.

use super::*;

#[tokio::test]
async fn db_buy_trade_insert_and_retrieve() {
    let pool = test_pool().await;
    insert_test_listing(&pool).await;
    db_upsert(&pool, &buy_trade()).await.unwrap();
    let got = db_get(&pool, TradeId(1)).await.unwrap().unwrap();
    assert_eq!(got.trade_type, TradeType::Buy);
    assert_eq!(got.quantity, Decimal::from(10));
    assert_eq!(got.average_price, Decimal::from(100));
    assert_eq!(
        got.settlement_date,
        NaiveDate::from_ymd_opt(2024, 1, 17).unwrap()
    );
    assert_eq!(got.contract_note_ref, Some("CN001".to_owned()));
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
    bad_currency.currency = "ZZZ".into();
    bad_currency.brokerage_currency = "ZZZ".into();
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
    btc.id = TradeId(2);
    btc.listing_id = ListingId(2);
    btc.currency = "BTC".into();
    btc.brokerage_currency = "BTC".into();
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
    let got = db_get(&pool, TradeId(2)).await.unwrap().unwrap();
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
    let got = db_get(&pool, TradeId(3)).await.unwrap().unwrap();
    assert_eq!(got.trade_type, TradeType::DRP);
    assert_eq!(got.quantity, Decimal::from(2));
}

#[tokio::test]
async fn db_drp_residual_fields_round_trip_with_precision() {
    let pool = test_pool().await;
    insert_test_listing(&pool).await;
    let mut trade = buy_trade();
    trade.id = TradeId(7);
    trade.trade_type = TradeType::DRP;
    trade.residual_brought_forward = "1.234567890".parse().unwrap();
    trade.residual_carried_forward = "0.987654321".parse().unwrap();
    trade.residual_paid_out = "2.500000001".parse().unwrap();
    db_upsert(&pool, &trade).await.unwrap();
    let got = db_get(&pool, TradeId(7)).await.unwrap().unwrap();
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
    let got = db_get(&pool, TradeId(1)).await.unwrap().unwrap();
    assert_eq!(got.residual_brought_forward, Decimal::ZERO);
    assert_eq!(got.residual_carried_forward, Decimal::ZERO);
    assert_eq!(got.residual_paid_out, Decimal::ZERO);
}

#[tokio::test]
async fn db_get_missing_returns_none() {
    let pool = test_pool().await;
    assert!(db_get(&pool, TradeId(999)).await.unwrap().is_none());
}
