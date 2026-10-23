//! The plain CRUD surface: get, list, delete, and their 404/422 answers.

use super::*;

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
    assert_eq!(resp.status, StatusCode::CREATED);
    let trade = db_get(&pool, TradeId(1)).await.unwrap().unwrap();
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
    insert_sell_consuming(&pool, TradeId(2), TradeId(1), Decimal::from(5)).await;

    let resp = client(&pool).delete("/trades/1").await;
    assert_eq!(resp.status, StatusCode::UNPROCESSABLE_ENTITY);
    assert!(
        db_get(&pool, TradeId(1)).await.unwrap().is_some(),
        "consumed buy must remain"
    );
}

#[tokio::test]
async fn api_shrink_partly_sold_buy_returns_422() {
    let pool = test_pool().await;
    insert_test_listing(&pool).await;
    db_upsert(&pool, &buy_trade()).await.unwrap(); // qty 10
    insert_sell_consuming(&pool, TradeId(2), TradeId(1), Decimal::from(5)).await;

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
        db_get(&pool, TradeId(1)).await.unwrap().unwrap().quantity,
        Decimal::from(10)
    );
}

#[tokio::test]
async fn api_listing_change_on_consumed_parcel_returns_422_with_reason() {
    let pool = test_pool().await;
    insert_test_listing(&pool).await;
    test_support::listing(2).ticker("VGS").insert(&pool).await;
    db_upsert(&pool, &buy_trade()).await.unwrap(); // listing 1
    insert_sell_consuming(&pool, TradeId(2), TradeId(1), Decimal::from(5)).await;

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
    assert_eq!(
        db_get(&pool, TradeId(1)).await.unwrap().unwrap().listing_id,
        1
    );
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
    assert_eq!(resp.status, StatusCode::CREATED);
    let resp = client(&pool).get("/trades/1").await;
    let t: Trade = resp.json();
    assert_eq!(t.average_price, "99.9999999999".parse::<Decimal>().unwrap());
    assert_eq!(t.quantity, "10.5".parse::<Decimal>().unwrap());
    assert_eq!(t.brokerage, "9.95".parse::<Decimal>().unwrap());
}
