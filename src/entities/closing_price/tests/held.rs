//! The held timeline: which listings were held when, and that it agrees with
//! the holdings reports across a split or consolidation.

use super::*;

#[tokio::test]
async fn db_held_listings_excludes_fully_sold() {
    let pool = test_pool().await;
    insert_listing(&pool, 1, "BHP", "XASX", "AUD").await;
    insert_listing(&pool, 2, "SOLD", "XASX", "AUD").await;
    insert_listing(&pool, 3, "NEVER", "XASX", "AUD").await;
    insert_buy(&pool, 1, 1, "100").await;
    insert_buy(&pool, 2, 2, "50").await;
    sell_everything(&pool, 3, 2, 2, "50").await;

    assert_eq!(db_held_listing_ids(&pool, None).await.unwrap(), vec![1]);
    // As at a date before the sale, the sold listing still counts; before
    // any buys, nothing does.
    assert_eq!(
        db_held_listing_ids(&pool, Some(ymd(2024, 5, 31)))
            .await
            .unwrap(),
        vec![1, 2]
    );
    assert!(
        db_held_listing_ids(&pool, Some(ymd(2024, 1, 1)))
            .await
            .unwrap()
            .is_empty()
    );
}

/// A listing sold part-way through the lookback window is still collected
/// for the days it was held: `reports::valuation` values a snapshot date
/// against the listings held *on that date*, so dropping it the moment
/// the Sell lands leaves those dates permanently blocked.
#[tokio::test]
async fn collection_covers_a_listing_sold_inside_the_lookback_window() {
    let pool = test_pool().await;
    insert_listing(&pool, 1, "BHP", "XASX", "AUD").await;
    crate::test_support::buy(1, 1)
        .date(ymd(2024, 1, 16))
        .qty(Decimal::from(100))
        .price(Decimal::from(10))
        .insert(&pool)
        .await;
    sell_everything(&pool, 2, 1, 1, "100").await;
    sqlx::query("UPDATE trades SET date = '2026-06-03' WHERE id = 2")
        .execute(&pool)
        .await
        .unwrap();

    // Nothing is held now, but the listing was held for most of the window.
    assert!(db_held_listing_ids(&pool, None).await.unwrap().is_empty());

    let mut stub = StubFetcher::default();
    for &d in &asx_lookback_window() {
        stub = stub.with_close(1, d, "62.48", "AUD");
    }
    run_collection(&pool, &stub, friday_evening_sydney())
        .await
        .unwrap();

    let stored = db_list(&pool, Some(1), None, None).await.unwrap();
    assert!(
        !stored.is_empty(),
        "the sold listing is still collected for the window"
    );
    assert!(
        stored.iter().any(|r| r.price_date == ymd(2026, 6, 2)),
        "including the days before the sale"
    );
}

/// The collection window must reach at least as far back as the snapshot
/// catch-up window: a date the snapshot job retries but collection no
/// longer refills can never unblock itself.
#[test]
fn collection_window_covers_the_snapshot_catchup_window() {
    // Read through a runtime binding so this stays a real assertion if the
    // two constants are ever decoupled again.
    let catchup: i64 = crate::reports::snapshot::CATCHUP_LOOKBACK_DAYS;
    let collection: i64 = COLLECTION_LOOKBACK_DAYS;
    assert!(
        catchup <= collection,
        "snapshot catch-up ({catchup}) reaches further back than collection ({collection})"
    );
}

/// `db_held_listing_ids` and `reports::portfolio::db_holdings_on` must
/// agree about whether a listing is held: the price map is keyed off the
/// former and the snapshot rows off the latter, so a disagreement stores
/// a silently unvalued holding (or blocks a date on a security already
/// fully sold). A split between the Buy and the Sell is what used to
/// separate them — the allocation is in sale-date units, the parcel in
/// as-acquired ones.
async fn held_sets_agree(pool: &SqlitePool, as_of: NaiveDate) {
    let ids = db_held_listing_ids(pool, Some(as_of)).await.unwrap();
    let mut conn = pool.acquire().await.unwrap();
    let holdings = crate::reports::portfolio::db_holdings_on(&mut conn, Some(as_of))
        .await
        .unwrap();
    let mut from_report: Vec<i64> = holdings.iter().map(|h| h.listing_id).collect();
    from_report.sort();
    from_report.dedup();
    assert_eq!(ids, from_report, "as at {as_of}");
}

#[tokio::test]
async fn db_held_listings_match_the_holdings_report_across_a_split() {
    let pool = test_pool().await;
    insert_listing(&pool, 1, "BHP", "XASX", "AUD").await;
    // Buy 100, 2:1 split to 200 units, sell 150 of them.
    crate::test_support::buy(1, 1)
        .date(ymd(2024, 1, 16))
        .qty(Decimal::from(100))
        .price(Decimal::from(10))
        .insert(&pool)
        .await;
    insert_share_split(&pool, 1, ymd(2024, 3, 1), "2", "1").await;
    crate::test_support::sell(2, 1)
        .date(ymd(2024, 6, 3))
        .qty(Decimal::from(150))
        .price(Decimal::from(8))
        .insert(&pool)
        .await;
    crate::test_support::allocate(&pool, 2, 2, 1, Decimal::from(150)).await;

    // 50 of the 200 post-split units remain, so the listing is still held
    // — the raw subtraction (100 − 150) used to make it look fully sold.
    held_sets_agree(&pool, ymd(2024, 7, 1)).await;
    assert_eq!(
        db_held_listing_ids(&pool, Some(ymd(2024, 7, 1)))
            .await
            .unwrap(),
        vec![1]
    );
}

#[tokio::test]
async fn db_held_listings_match_the_holdings_report_across_a_consolidation() {
    let pool = test_pool().await;
    insert_listing(&pool, 1, "BHP", "XASX", "AUD").await;
    // Buy 1000, 1:10 consolidation to 100 units, sell all 100.
    crate::test_support::buy(1, 1)
        .date(ymd(2024, 1, 16))
        .qty(Decimal::from(1000))
        .price(Decimal::from(1))
        .insert(&pool)
        .await;
    insert_share_split(&pool, 1, ymd(2024, 3, 1), "1", "10").await;
    crate::test_support::sell(2, 1)
        .date(ymd(2024, 6, 3))
        .qty(Decimal::from(100))
        .price(Decimal::from(12))
        .insert(&pool)
        .await;
    crate::test_support::allocate(&pool, 2, 2, 1, Decimal::from(100)).await;

    // Fully sold — the raw subtraction (1000 − 100) used to leave 900
    // phantom units, blocking every later snapshot on a missing price.
    held_sets_agree(&pool, ymd(2024, 7, 1)).await;
    assert!(
        db_held_listing_ids(&pool, Some(ymd(2024, 7, 1)))
            .await
            .unwrap()
            .is_empty()
    );
}
