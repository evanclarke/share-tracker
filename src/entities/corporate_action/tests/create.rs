//! The create entry point (`POST /corporate_actions`), and the list's filters.

use super::*;

// The `POST /corporate_actions` create — the second entry point beside the
// `PUT /corporate_actions/:id` upsert, mirroring income's four tests.

/// `POST /corporate_actions` allocates its own id — the create that makes
/// the whole `max(id) + 1` dance unnecessary (SCENARIOS U-a).
#[tokio::test]
async fn post_corporate_action_creates_a_row_with_a_database_assigned_id() {
    let pool = test_pool().await;
    insert_listing(&pool, 1, "RAP").await;
    let client = client(&pool);

    let body = serde_json::json!({
        "action_type": "ReturnOfCapital",
        "listing_id": 1,
        "date": "2024-11-30",
        "amount_per_unit": "0.50",
        "currency": "AUD"
    });
    let response = client.post("/corporate_actions", &body).await;
    let (status, text) = response.status_and_body();
    assert_eq!(status, StatusCode::CREATED, "body: {text:?}");

    let created: CorporateAction = response.json();
    assert_ne!(created.id, 0, "a create must not land on id 0");
    assert_eq!(
        created.kind,
        ActionKind::ReturnOfCapital {
            amount_per_unit: "0.50".parse().unwrap(),
            currency: "AUD".into(),
            record_date: None,
        }
    );

    // The returned row is the stored one.
    let stored = db_get(&pool, created.id).await.unwrap().unwrap();
    assert_eq!(stored, created);
}

/// The bug this endpoint exists to kill: with an id-less create there is no
/// guessed id to collide with, so two creates are two rows — before, a
/// second `max(id) + 1`-style PUT on a taken id silently overwrote the
/// first through the upsert's `ON CONFLICT ... DO UPDATE`.
#[tokio::test]
async fn post_corporate_action_twice_stores_two_distinct_rows() {
    let pool = test_pool().await;
    insert_listing(&pool, 1, "RAP").await;
    let client = client(&pool);

    let body = serde_json::json!({
        "action_type": "ReturnOfCapital",
        "listing_id": 1,
        "date": "2024-11-30",
        "amount_per_unit": "0.50",
        "currency": "AUD"
    });
    let first: CorporateAction = client
        .post("/corporate_actions", &body)
        .await
        .expect_status(StatusCode::CREATED)
        .json();
    let second: CorporateAction = client
        .post("/corporate_actions", &body)
        .await
        .expect_status(StatusCode::CREATED)
        .json();
    assert_ne!(
        first.id, second.id,
        "each create must get its own id, not overwrite the previous row"
    );

    let all: Vec<CorporateAction> = client.get_json("/corporate_actions").await;
    assert_eq!(all.len(), 2, "both creates are stored: {all:?}");
}

/// A create never re-issues an id a deleted row held — the property the
/// whole `row_history` occupant marking rests on. `AUTOINCREMENT`
/// guarantees it only while the INSERT omits the id column, which is
/// exactly what the create path does (SCENARIOS U-a, migration 0045).
#[tokio::test]
async fn post_corporate_action_never_reuses_a_deleted_rows_id() {
    let pool = test_pool().await;
    insert_listing(&pool, 1, "RAP").await;
    let client = client(&pool);

    let body = serde_json::json!({
        "action_type": "ReturnOfCapital",
        "listing_id": 1,
        "date": "2024-11-30",
        "amount_per_unit": "0.50",
        "currency": "AUD"
    });
    let first: CorporateAction = client
        .post("/corporate_actions", &body)
        .await
        .expect_status(StatusCode::CREATED)
        .json();

    // Edit it, so its trail is non-empty, then delete it — the id is now
    // free and carries history that must never be inherited.
    let edited = serde_json::json!({
        "action_type": "ReturnOfCapital",
        "listing_id": 1,
        "date": "2024-12-01",
        "amount_per_unit": "0.75",
        "currency": "AUD"
    });
    client
        .put_ok(&format!("/corporate_actions/{}", first.id), &edited)
        .await;
    client
        .delete(&format!("/corporate_actions/{}", first.id))
        .await
        .expect_status(StatusCode::NO_CONTENT);

    let second: CorporateAction = client
        .post("/corporate_actions", &body)
        .await
        .expect_status(StatusCode::CREATED)
        .json();
    assert_ne!(
        first.id, second.id,
        "the create took the deleted row's id back, inheriting its audit history"
    );
}

/// A create shares every write-time invariant with the upsert, so a return
/// of capital on an AMIT — the E4 mechanism is for non-AMIT trusts — is
/// refused and nothing is stored; the create path is no way round it.
#[tokio::test]
async fn post_corporate_action_rejects_like_the_upsert_and_stores_nothing() {
    let pool = test_pool().await;
    test_support::listing(1)
        .ticker("AMT")
        .name("AMT")
        .security_type(listing::SecurityType::Share)
        .amit(true)
        .insert(&pool)
        .await;

    let body = serde_json::json!({
        "action_type": "ReturnOfCapital",
        "listing_id": 1,
        "date": "2024-11-30",
        "amount_per_unit": "0.50",
        "currency": "AUD"
    });
    let response = client(&pool).post("/corporate_actions", &body).await;
    let (status, text) = response.status_and_body();
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "body: {text:?}");
    assert!(
        text.contains("does not apply to an AMIT"),
        "the shared AMIT wording is reused: {text:?}"
    );

    let all: Vec<CorporateAction> = client(&pool).get_json("/corporate_actions").await;
    assert!(all.is_empty(), "a rejected create stores nothing: {all:?}");
}

/// The `/corporate_actions` list narrows by `?listing_id=` and the
/// `?from=`/`?to=` range over the action `date` (inclusive); combined
/// filters AND.
#[tokio::test]
async fn the_list_narrows_by_listing_and_date_range() {
    use crate::test_support::{assert_list_filters, ymd};
    let pool = test_pool().await;
    insert_listing(&pool, 1, "CA1").await;
    insert_listing(&pool, 2, "CA2").await;
    for (id, listing_id, date) in [
        (1, 1, ymd(2024, 6, 3)),
        (2, 1, ymd(2024, 7, 3)),
        (3, 2, ymd(2024, 8, 5)),
        (4, 2, ymd(2024, 9, 4)),
    ] {
        db_upsert(&pool, &roc(id, ListingId(listing_id), date, "0.10"))
            .await
            .unwrap();
    }

    assert_list_filters(
        &client(&pool),
        "/corporate_actions",
        |a: &CorporateAction| a.id,
        &[
            ("", &[1, 2, 3, 4]),
            ("?listing_id=1", &[1, 2]),
            ("?from=2024-08-01", &[3, 4]),
            ("?to=2024-07-31", &[1, 2]),
            ("?listing_id=2&from=2024-09-01", &[4]),
            ("?listing_id=99", &[]),
        ],
    )
    .await;
}
