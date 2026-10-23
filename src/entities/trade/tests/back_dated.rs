//! SCENARIOS V-d: a parcel dated behind a whole-holding operation.

use super::*;

/// An OLD → NEW scrip-for-scrip exchange that has already run, plus the
/// listings it needs. Returns nothing: the ids are fixed (1 = OLD, 2 = NEW,
/// action 10, parcel 1).
async fn exchanged_listing(pool: &SqlitePool) {
    test_support::listing(1).ticker("OLD").insert(pool).await;
    test_support::listing(2).ticker("NEW").insert(pool).await;
    test_support::buy(1, 1)
        .date(ymd(2024, 1, 2))
        .insert(pool)
        .await;
    corporate_action::db_upsert(
        pool,
        &CorporateAction {
            id: 10,
            listing_id: ListingId(1),
            date: ymd(2024, 6, 10),
            kind: ActionKind::ScripForScrip {
                scrip_listing_id: ListingId(2),
                scrip_new_units: Decimal::ONE,
                scrip_old_units: Decimal::ONE,
                scrip_cash_per_unit: None,
                scrip_market_value: None,
                scrip_cash_currency: None,
            },
        },
    )
    .await
    .unwrap();
    scrip_exchange::db_exchange(pool, 10).await.unwrap();
}

/// A HEAD → SPIN demerger that has already run (1 = HEAD, 2 = SPIN,
/// action 10, parcel 1).
async fn demerged_listing(pool: &SqlitePool) {
    test_support::listing(1).ticker("HEAD").insert(pool).await;
    test_support::listing(2).ticker("SPIN").insert(pool).await;
    test_support::buy(1, 1)
        .date(ymd(2024, 1, 2))
        .insert(pool)
        .await;
    corporate_action::db_upsert(
        pool,
        &CorporateAction {
            id: 10,
            listing_id: ListingId(1),
            date: ymd(2024, 6, 11),
            kind: ActionKind::Demerger {
                demerger_listing_id: ListingId(2),
                demerger_new_units: Decimal::ONE,
                demerger_held_units: dec("5"),
                demerger_cost_base_pct: dec("10"),
                demerger_close_date: None,
                demerger_close_price: None,
                demerger_close_sourced_from: None,
                demerger_close_reason: None,
            },
        },
    )
    .await
    .unwrap();
    demerger::db_demerge(pool, 10).await.unwrap();
}

/// A Buy of the listing dated `date`, id 900.
fn back_dated_buy(listing_id: ListingId, date: NaiveDate) -> Trade {
    test_support::buy(900, listing_id.0)
        .date(date)
        .settlement(date)
        .build()
}

/// A scrip-for-scrip exchange consumed **every** open parcel of the
/// original listing as at its date, and cannot reach back for one entered
/// afterwards: those units would stay open on a security the exchange
/// replaced, with no replacement units issued for them (SCENARIOS V-d).
#[tokio::test]
async fn db_a_buy_dated_before_an_executed_exchange_is_refused() {
    let pool = test_pool().await;
    exchanged_listing(&pool).await;
    let err = db_upsert(&pool, &back_dated_buy(ListingId(1), ymd(2024, 2, 5)))
        .await
        .unwrap_err();
    assert!(
        matches!(err, UpsertError::BackDatedOverWholeHolding(_)),
        "expected the whole-holding refusal, got: {err:?}"
    );
}

/// The demerger case: the parcel would keep 100% of its cost base instead
/// of the head company's share, and no demerged units would be issued for
/// it.
#[tokio::test]
async fn db_a_buy_dated_before_an_executed_demerger_is_refused() {
    let pool = test_pool().await;
    demerged_listing(&pool).await;
    let err = db_upsert(&pool, &back_dated_buy(ListingId(1), ymd(2024, 3, 5)))
        .await
        .unwrap_err();
    assert!(
        matches!(err, UpsertError::BackDatedOverWholeHolding(_)),
        "expected the whole-holding refusal, got: {err:?}"
    );
}

/// The worthless-shares case: 40 units left open on a company already
/// written off, whose capital loss is never recognised.
#[tokio::test]
async fn db_a_buy_dated_before_an_executed_recognise_is_refused() {
    let pool = test_pool().await;
    test_support::recognised_worthless_listing(
        &pool,
        5,
        "DEAD",
        ymd(2024, 1, 2),
        90,
        ymd(2024, 6, 13),
    )
    .await;
    let err = db_upsert(&pool, &back_dated_buy(ListingId(5), ymd(2024, 3, 5)))
        .await
        .unwrap_err();
    assert!(
        matches!(err, UpsertError::BackDatedOverWholeHolding(_)),
        "expected the whole-holding refusal, got: {err:?}"
    );
}

/// The rejection is a `422` whose body names the operation, its date, and
/// the delete-enter-redo recovery — the same shape the sibling refusal on
/// a back-dated corporate action gives, so the web UI can show it as an
/// instruction rather than an error code.
#[tokio::test]
async fn api_a_back_dated_buy_is_422_naming_the_operation_and_the_recovery() {
    let pool = test_pool().await;
    exchanged_listing(&pool).await;
    let body = serde_json::json!({
        "trade_type": "Buy",
        "date": "2024-02-05",
        "listing_id": 1,
        "average_price": "10.0",
        "quantity": "50.0",
        "currency": "AUD",
        "brokerage": "0.0",
        "gst_on_brokerage": "0.0",
        "brokerage_currency": "AUD",
        "fx_rate": "1.0"
    });
    let response = client(&pool).put("/trades/900", &body).await;
    let (status, detail) = response.status_and_body();
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert!(detail.contains("scrip-for-scrip exchange"), "{detail}");
    assert!(detail.contains("corporate action #10"), "{detail}");
    assert!(detail.contains("2024-06-10"), "{detail}");
    assert!(
        detail.contains("Delete that operation, enter this parcel, then run it again"),
        "{detail}"
    );
    // Nothing was written.
    let exists: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM trades WHERE id = 900)")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert!(!exists);
}

/// A parcel dated **after** the operation is ordinary post-event activity
/// and lands normally — the head listing of a demerger keeps trading.
#[tokio::test]
async fn db_a_buy_dated_after_the_operation_is_accepted() {
    let pool = test_pool().await;
    demerged_listing(&pool).await;
    db_upsert(&pool, &back_dated_buy(ListingId(1), ymd(2024, 6, 12)))
        .await
        .unwrap();
}

/// Editing a source parcel the operation **did** consume stays allowed:
/// that is precisely the state `reports::rollover_consistency` documents
/// and surfaces, and refusing the edit would make it unfixable while
/// fixing nothing. Only a write that *newly* strands units is refused.
#[tokio::test]
async fn db_editing_a_consumed_source_parcel_is_still_allowed() {
    let pool = test_pool().await;
    exchanged_listing(&pool).await;
    let mut parcel = db_get(&pool, TradeId(1)).await.unwrap().unwrap();
    parcel.average_price = dec("12");
    db_upsert(&pool, &parcel).await.unwrap();
    assert_eq!(
        db_get(&pool, TradeId(1))
            .await
            .unwrap()
            .unwrap()
            .average_price,
        dec("12")
    );
}
