//! The trade endpoints take Buys only: a Sell or DRP body is refused, and no
//! write can change a stored row's type.

use super::*;

#[tokio::test]
async fn api_put_sell_trade_is_rejected() {
    // Sells must go through PUT /sells/{id}; the generic trade endpoint rejects them.
    let pool = test_pool().await;
    insert_test_listing(&pool).await;
    let body = serde_json::json!({
        "trade_type": "Sell",
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
}

#[tokio::test]
async fn api_put_drp_trade_is_rejected() {
    // DRP trades are created only via POST /income/{id}/reinvest (which links
    // them to their distribution and threads the residual chain); the generic
    // trade endpoint rejects a free-form DRP, and nothing is persisted.
    let pool = test_pool().await;
    insert_test_listing(&pool).await;
    let body = serde_json::json!({
        "trade_type": "DRP",
        "date": "2024-03-15",
        "listing_id": 1,
        "average_price": "95.0",
        "quantity": "2.0",
        "currency": "AUD",
        "brokerage": "0.0",
        "gst_on_brokerage": "0.0",
        "brokerage_currency": "AUD",
        "fx_rate": "1.0"
    });
    let resp = client(&pool).put("/trades/1", &body).await;
    assert_eq!(resp.status, StatusCode::UNPROCESSABLE_ENTITY);
    let n: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM trades")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(n, 0, "rejected DRP must not be persisted");
}

/// The stored `parcel_allocations` row of the Sell `insert_sell_consuming`
/// wrote: `(purchase_trade_id, sale_trade_id, quantity_allocated)`.
async fn allocation_rows(pool: &SqlitePool) -> Vec<(i64, i64, String)> {
    sqlx::query_as(
        "SELECT purchase_trade_id, sale_trade_id, quantity_allocated \
             FROM parcel_allocations ORDER BY id",
    )
    .fetch_all(pool)
    .await
    .unwrap()
}

/// A Buy body at an id that already holds a plain Sell is refused: an
/// existing trade's type is part of its identity (SCENARIOS Z-c).
/// Accepted, it rewrote the disposal as a parcel in place — leaving the
/// Sell's allocations naming a Buy as their sale, un-consuming the units
/// they drew, and inventing an open parcel that was never bought, none of
/// which any report surfaces.
#[tokio::test]
async fn api_a_buy_body_cannot_rewrite_a_stored_sell() {
    let pool = test_pool().await;
    insert_test_listing(&pool).await;
    db_upsert(&pool, &buy_trade()).await.unwrap();
    insert_sell_consuming(&pool, TradeId(2), TradeId(1), Decimal::from(4)).await;
    let before = db_get(&pool, TradeId(2)).await.unwrap().unwrap();
    let allocations_before = allocation_rows(&pool).await;

    let body = serde_json::json!({
        "trade_type": "Buy",
        "date": "2024-06-03",
        "listing_id": 1,
        "average_price": "100.0",
        "quantity": "100.0",
        "currency": "AUD",
        "brokerage": "0.0",
        "gst_on_brokerage": "0.0",
        "brokerage_currency": "AUD",
        "fx_rate": "1.0"
    });
    let resp = client(&pool).put("/trades/2", &body).await;
    let (status, detail) = resp.status_and_body();
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    // Names what the id actually holds, and where a Sell is written.
    assert!(detail.contains("trade 2 is a Sell, not a Buy"), "{detail}");
    assert!(detail.contains("PUT /sells/2"), "{detail}");

    // The stored row is untouched — still the disposal it was.
    let after = db_get(&pool, TradeId(2)).await.unwrap().unwrap();
    assert_eq!(after.trade_type, TradeType::Sell);
    assert_eq!(after.quantity, before.quantity);
    assert_eq!(after.average_price, before.average_price);
    // And so are its allocations: this is the actual harm — an allocation
    // whose `sale_trade_id` names a Buy is a sale that is not a sale.
    assert_eq!(allocation_rows(&pool).await, allocations_before);
    assert_eq!(allocations_before.len(), 1);
    assert_eq!(allocations_before[0].0, 1);
    assert_eq!(allocations_before[0].1, 2);
}

/// The same at an id holding a plain Sell with a **DRP** body. Over HTTP
/// the body guard refuses it first (a DRP is only ever created by
/// reinvesting a distribution), so the endpoint answers 422 either way;
/// `db_upsert_cannot_change_an_existing_trades_type` pins the stored-row
/// rule underneath it.
#[tokio::test]
async fn api_a_drp_body_cannot_rewrite_a_stored_sell() {
    let pool = test_pool().await;
    insert_test_listing(&pool).await;
    db_upsert(&pool, &buy_trade()).await.unwrap();
    insert_sell_consuming(&pool, TradeId(2), TradeId(1), Decimal::from(4)).await;
    let allocations_before = allocation_rows(&pool).await;

    let body = serde_json::json!({
        "trade_type": "DRP",
        "date": "2024-06-03",
        "listing_id": 1,
        "average_price": "100.0",
        "quantity": "100.0",
        "currency": "AUD",
        "brokerage": "0.0",
        "gst_on_brokerage": "0.0",
        "brokerage_currency": "AUD",
        "fx_rate": "1.0"
    });
    let resp = client(&pool).put("/trades/2", &body).await;
    assert_eq!(resp.status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(
        db_get(&pool, TradeId(2)).await.unwrap().unwrap().trade_type,
        TradeType::Sell
    );
    assert_eq!(allocation_rows(&pool).await, allocations_before);
}

/// The rule is about the *stored* row's type, not the endpoint: at the DB
/// level every pair that would move a trade's type is refused, in both
/// directions.
#[tokio::test]
async fn db_upsert_cannot_change_an_existing_trades_type() {
    let pool = test_pool().await;
    insert_test_listing(&pool).await;
    db_upsert(&pool, &buy_trade()).await.unwrap();
    insert_sell_consuming(&pool, TradeId(2), TradeId(1), Decimal::from(4)).await;

    // Sell → Buy, and Sell → DRP.
    let stored_sell = db_get(&pool, TradeId(2)).await.unwrap().unwrap();
    for requested in [TradeType::Buy, TradeType::DRP] {
        let mut retyped = stored_sell.clone();
        retyped.trade_type = requested;
        assert!(
            matches!(
                db_upsert(&pool, &retyped).await,
                Err(UpsertError::TradeTypeChange {
                    id: TradeId(2),
                    existing: TradeType::Sell,
                    requested: got,
                }) if got == requested
            ),
            "expected a type-change refusal for {requested:?}"
        );
    }

    // Buy → Sell, the direction `PUT /sells/:id` refuses from its own side
    // (`sell::SellError::NotASell`, SCENARIOS Z-b).
    let mut retyped = db_get(&pool, TradeId(1)).await.unwrap().unwrap();
    retyped.trade_type = TradeType::Sell;
    assert!(matches!(
        db_upsert(&pool, &retyped).await,
        Err(UpsertError::TradeTypeChange {
            id: TradeId(1),
            existing: TradeType::Buy,
            requested: TradeType::Sell,
        })
    ));

    // Nothing moved.
    assert_eq!(
        db_get(&pool, TradeId(1)).await.unwrap().unwrap().trade_type,
        TradeType::Buy
    );
    assert_eq!(
        db_get(&pool, TradeId(2)).await.unwrap().unwrap().trade_type,
        TradeType::Sell
    );
}

/// The controls: the rule only bites on a type *change*. Editing an
/// existing Buy as a Buy still succeeds, and a free id still creates a
/// trade.
#[tokio::test]
async fn api_same_type_edits_and_free_ids_are_unaffected() {
    let pool = test_pool().await;
    insert_test_listing(&pool).await;
    db_upsert(&pool, &buy_trade()).await.unwrap();

    let body = |quantity: &str| {
        serde_json::json!({
            "trade_type": "Buy",
            "date": "2024-01-15",
            "listing_id": 1,
            "average_price": "100.0",
            "quantity": quantity,
            "currency": "AUD",
            "brokerage": "9.95",
            "gst_on_brokerage": "0.995",
            "brokerage_currency": "AUD",
            "fx_rate": "1.0"
        })
    };

    // An ordinary edit of the stored Buy.
    let resp = client(&pool).put("/trades/1", &body("25")).await;
    assert_eq!(resp.status, StatusCode::NO_CONTENT);
    assert_eq!(
        db_get(&pool, TradeId(1)).await.unwrap().unwrap().quantity,
        Decimal::from(25)
    );

    // A new, free id.
    let resp = client(&pool).put("/trades/7", &body("5")).await;
    assert_eq!(resp.status, StatusCode::CREATED);
    let created = db_get(&pool, TradeId(7)).await.unwrap().unwrap();
    assert_eq!(created.trade_type, TradeType::Buy);
    assert_eq!(created.quantity, Decimal::from(5));
}
