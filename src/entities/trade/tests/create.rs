//! The create entry point (`POST /trades`), and the list's filters.

use super::*;
use crate::test_support::holding_account_row;

/// `POST /trades` allocates its own id — the create that makes the whole
/// `max(id) + 1` dance unnecessary (SCENARIOS U-a).
#[tokio::test]
async fn post_trade_creates_a_row_with_a_database_assigned_id() {
    let pool = test_pool().await;
    insert_test_listing(&pool).await;
    let client = client(&pool);

    let response = client
        .post("/trades", &auto_settled_buy(ymd(2024, 1, 15)))
        .await;
    let (status, text) = response.status_and_body();
    assert_eq!(status, StatusCode::CREATED, "body: {text:?}");

    let created: Trade = response.json();
    assert_ne!(created.id, 0, "a create must not land on id 0");
    assert_eq!(created.trade_type, TradeType::Buy);
    assert_eq!(created.quantity, Decimal::from(10));
    // The create resolves the omitted settlement date exactly as the
    // upsert does: XASX T+2 from Monday 2024-01-15 is Wednesday the 17th.
    assert_eq!(created.settlement_date, ymd(2024, 1, 17));
    assert_eq!(
        created.settlement_date_source,
        SettlementDateSource::Computed
    );

    // The returned row is the stored one, and it is listed under its own
    // id — so the caller can act on the id at once instead of re-reading
    // the collection to discover it.
    let listed = db_get(&pool, created.id).await.unwrap().unwrap();
    assert_eq!(listed.id, created.id);
    assert_eq!(listed.listing_id, 1);
    assert_eq!(listed.average_price, Decimal::from(100));
}

/// The bug this endpoint exists to kill: with an id-less create there is no
/// guessed id to collide with, so two creates are two rows — before, a
/// second `max(id) + 1`-style PUT on a taken id silently overwrote the
/// first through the upsert's `ON CONFLICT ... DO UPDATE`.
#[tokio::test]
async fn post_trade_twice_stores_two_distinct_rows() {
    let pool = test_pool().await;
    insert_test_listing(&pool).await;
    let client = client(&pool);

    let first: Trade = client
        .post("/trades", &auto_settled_buy(ymd(2024, 1, 15)))
        .await
        .expect_status(StatusCode::CREATED)
        .json();
    let second: Trade = client
        .post("/trades", &auto_settled_buy(ymd(2024, 1, 16)))
        .await
        .expect_status(StatusCode::CREATED)
        .json();
    assert_ne!(
        first.id, second.id,
        "each create must get its own id, not overwrite the previous row"
    );

    let all: Vec<Trade> = client.get_json("/trades").await;
    assert_eq!(all.len(), 2, "both creates are stored: {all:?}");
}

/// The property the whole `row_history` occupant marking rests on: an id
/// handed out twice inherits the previous occupant's trail. `AUTOINCREMENT`
/// guarantees a create never re-issues a deleted row's id only while the
/// INSERT omits the id column — which is exactly what the create path does
/// (SCENARIOS U-a, migration 0045).
#[tokio::test]
async fn post_trade_never_reuses_a_deleted_rows_id() {
    let pool = test_pool().await;
    insert_test_listing(&pool).await;
    let client = client(&pool);

    let first: Trade = client
        .post("/trades", &auto_settled_buy(ymd(2024, 1, 15)))
        .await
        .expect_status(StatusCode::CREATED)
        .json();

    // Edit it, so its trail is non-empty, then delete it — the id is now
    // free and carries history that must never be inherited.
    let mut edited = auto_settled_buy(ymd(2024, 1, 15));
    edited["quantity"] = serde_json::json!("20");
    client
        .put(&format!("/trades/{}", first.id), &edited)
        .await
        .expect_status(StatusCode::NO_CONTENT);
    client
        .delete(&format!("/trades/{}", first.id))
        .await
        .expect_status(StatusCode::NO_CONTENT);

    let second: Trade = client
        .post("/trades", &auto_settled_buy(ymd(2024, 1, 15)))
        .await
        .expect_status(StatusCode::CREATED)
        .json();
    assert_ne!(
        first.id, second.id,
        "the create took the deleted row's id back, inheriting its audit history"
    );
}

/// A create shares every write-time validation with the upsert, so a bad
/// body is refused and nothing is stored — the create path is not a way
/// round the core-figure checks.
#[tokio::test]
async fn post_trade_rejects_like_the_upsert_and_stores_nothing() {
    let pool = test_pool().await;
    insert_test_listing(&pool).await;
    let client = client(&pool);

    let mut body = auto_settled_buy(ymd(2024, 1, 15));
    body["quantity"] = serde_json::json!("0");
    let response = client.post("/trades", &body).await;
    let (status, text) = response.status_and_body();
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "body: {text:?}");
    assert!(
        text.contains("quantity must be positive"),
        "the shared wording is reused: {text:?}"
    );

    let all: Vec<Trade> = client.get_json("/trades").await;
    assert!(all.is_empty(), "a rejected create stores nothing: {all:?}");
}

/// The two body-shape refusals the `PUT` path applies are shared with the
/// create, so `POST /trades` is not a second door onto a Sell or a DRP.
#[tokio::test]
async fn post_trade_refuses_sell_and_drp_bodies_like_the_upsert() {
    let pool = test_pool().await;
    insert_test_listing(&pool).await;
    let client = client(&pool);

    for (trade_type, expected) in [("Sell", "PUT /sells"), ("DRP", "reinvest")] {
        let mut body = auto_settled_buy(ymd(2024, 1, 15));
        body["trade_type"] = serde_json::json!(trade_type);
        let response = client.post("/trades", &body).await;
        let (status, text) = response.status_and_body();
        assert_eq!(
            status,
            StatusCode::UNPROCESSABLE_ENTITY,
            "{trade_type}: {text:?}"
        );
        assert!(text.contains(expected), "{trade_type}: {text:?}");
    }

    let all: Vec<Trade> = client.get_json("/trades").await;
    assert!(all.is_empty(), "a refused create stores nothing: {all:?}");
}

/// The `/trades` list narrows by `?listing_id=`, `?holding_account_id=`
/// and the `?from=`/`?to=` range over `date` (inclusive at both ends, the
/// convention `GET /closing_prices` set); combined filters AND, and a
/// filter matching nothing is an empty `200` array.
#[tokio::test]
async fn the_list_narrows_by_listing_account_and_date_range() {
    use crate::test_support::assert_list_filters;
    let pool = test_pool().await;
    for id in [1, 2] {
        test_support::listing(id)
            .ticker(&format!("TRD{id}"))
            .insert(&pool)
            .await;
    }
    holding_account::db_upsert(&pool, &holding_account_row(2, "Second"))
        .await
        .unwrap();

    for (id, listing_id, account, date) in [
        (1, 1, 1, ymd(2024, 6, 3)),
        (2, 1, 2, ymd(2024, 7, 3)),
        (3, 2, 1, ymd(2024, 8, 5)),
        (4, 2, 2, ymd(2024, 9, 4)),
    ] {
        test_support::buy(id, listing_id)
            .date(date)
            .settlement(date)
            .account(account)
            .insert(&pool)
            .await;
    }

    assert_list_filters(
        &client(&pool),
        "/trades",
        |t: &Trade| t.id.0,
        &[
            ("", &[1, 2, 3, 4]),
            ("?listing_id=1", &[1, 2]),
            ("?holding_account_id=1", &[1, 3]),
            ("?from=2024-08-01", &[3, 4]),
            ("?to=2024-07-31", &[1, 2]),
            ("?listing_id=2&holding_account_id=2", &[4]),
            ("?listing_id=99", &[]),
        ],
    )
    .await;
}
