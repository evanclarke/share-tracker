//! Schema invariants: the CHECK constraints the table is held to.

use super::*;

/// The schema pairs a manual row's provenance with its origin, so no
/// write path — not even raw SQL — can store a hand-entered price without
/// its sourcing and reason, or hang them on a fetched row.
#[tokio::test]
async fn db_check_constraints_pair_manual_provenance_with_the_origin() {
    let pool = test_pool().await;
    insert_listing(&pool, 1, "BHP", "XASX", "AUD").await;
    let insert = |columns: &'static str, values: &'static str| {
        let pool = pool.clone();
        async move {
            sqlx::query(sqlx::AssertSqlSafe(format!(
                "INSERT INTO closing_prices \
                     (listing_id, price_date, price, price_as_observed, fetched_at, status, \
                      error, {columns}) \
                 VALUES (1, '2026-06-05', '1.23', '1.23', 'now', 'ok', NULL, {values})"
            )))
            .execute(&pool)
            .await
        }
    };

    // manual without either provenance field
    assert!(
        insert("source, origin", "'manual', 'manual'")
            .await
            .is_err()
    );
    // manual with only one of them
    assert!(
        insert(
            "source, origin, sourced_from",
            "'manual', 'manual', 'asx.com.au'"
        )
        .await
        .is_err()
    );
    assert!(
        insert("source, origin, reason", "'manual', 'manual', 'no candle'")
            .await
            .is_err()
    );
    // a fetched row may not carry provenance meant for a manual one
    assert!(
        insert(
            "source, origin, sourced_from, reason",
            "'yahoo', 'fetched', 'asx.com.au', 'no candle'"
        )
        .await
        .is_err()
    );
    // the provider slot and the origin may not disagree, either way round
    assert!(
        insert(
            "source, origin, sourced_from, reason",
            "'yahoo', 'manual', 'asx.com.au', 'no candle'"
        )
        .await
        .is_err()
    );
    assert!(
        insert("source, origin", "'manual', 'fetched'")
            .await
            .is_err()
    );
    // an unknown origin is rejected by the enum CHECK
    assert!(
        insert(
            "source, origin, sourced_from, reason",
            "'manual', 'entered', 'asx.com.au', 'no candle'"
        )
        .await
        .is_err()
    );
    // …and the valid combination is accepted.
    assert!(
        insert(
            "source, origin, sourced_from, reason",
            "'manual', 'manual', 'asx.com.au', 'no candle'"
        )
        .await
        .is_ok()
    );
}

/// A manual row is always a price, never a recorded failure: there is no
/// such thing as a hand-entered fetch error.
#[tokio::test]
async fn db_check_constraint_forbids_an_errored_manual_row() {
    let pool = test_pool().await;
    insert_listing(&pool, 1, "BHP", "XASX", "AUD").await;
    let bad = sqlx::query(
        "INSERT INTO closing_prices \
             (listing_id, price_date, price, source, fetched_at, status, error, \
              origin, sourced_from, reason) \
         VALUES (1, '2026-06-05', NULL, 'manual', 'now', 'error', 'oops', \
                 'manual', 'asx.com.au', 'no candle')",
    )
    .execute(&pool)
    .await;
    assert!(bad.is_err());
}

/// A fetched row's provider slot is a closed set: the CHECK added in 0051
/// rejects any source the code cannot produce, so a typo cannot be stored and
/// silently mis-state a row's provenance. The two values the code does produce
/// — the provider's own and the manual slot paired with its origin — write.
#[tokio::test]
async fn db_check_constraint_rejects_an_unknown_source() {
    let pool = test_pool().await;
    insert_listing(&pool, 1, "BHP", "XASX", "AUD").await;
    let insert = |date: &'static str, columns: &'static str, values: &'static str| {
        let pool = pool.clone();
        async move {
            sqlx::query(sqlx::AssertSqlSafe(format!(
                "INSERT INTO closing_prices \
                     (listing_id, price_date, price, price_as_observed, fetched_at, status, \
                      error, {columns}) \
                 VALUES (1, {date}, '1.23', '1.23', 'now', 'ok', NULL, {values})"
            )))
            .execute(&pool)
            .await
        }
    };
    // A provider the code cannot produce is rejected.
    assert!(
        insert("'2026-06-05'", "source, origin", "'bloomberg', 'fetched'")
            .await
            .is_err()
    );
    // The fetched provider's own slot is accepted.
    assert!(
        insert("'2026-06-05'", "source, origin", "'yahoo', 'fetched'")
            .await
            .is_ok()
    );
    // …and so is the manual slot, on the hand-entered row it pairs with.
    assert!(
        insert(
            "'2026-06-08'",
            "source, origin, sourced_from, reason",
            "'manual', 'manual', 'asx.com.au', 'no candle'"
        )
        .await
        .is_ok()
    );
}

#[tokio::test]
async fn db_check_constraints_tie_price_and_error_to_status() {
    let pool = test_pool().await;
    insert_listing(&pool, 1, "BHP", "XASX", "AUD").await;
    // ok with no price violates the CHECK.
    let bad = sqlx::query(
        "INSERT INTO closing_prices (listing_id, price_date, price, source, fetched_at, status, error) \
         VALUES (1, '2026-06-05', NULL, 'yahoo', 'now', 'ok', NULL)",
    )
    .execute(&pool)
    .await;
    assert!(bad.is_err());
    // error with a price (and no error text) violates both CHECKs.
    let bad = sqlx::query(
        "INSERT INTO closing_prices (listing_id, price_date, price, source, fetched_at, status, error) \
         VALUES (1, '2026-06-05', '1.23', 'yahoo', 'now', 'error', NULL)",
    )
    .execute(&pool)
    .await;
    assert!(bad.is_err());
    // an unknown status is rejected by the enum CHECK.
    let bad = sqlx::query(
        "INSERT INTO closing_prices (listing_id, price_date, price, source, fetched_at, status, error) \
         VALUES (1, '2026-06-05', '1.23', 'yahoo', 'now', 'pending', NULL)",
    )
    .execute(&pool)
    .await;
    assert!(bad.is_err());
    // duplicate (listing, date) is rejected by the primary key.
    let market = load_market(&pool, 1).await.unwrap().unwrap();
    let ok = StubFetcher::default().with_close(1, ymd(2026, 6, 5), "62.48", "AUD");
    fetch_and_store(&pool, &ok, &market, &[ymd(2026, 6, 5)])
        .await
        .unwrap();
    let dup = sqlx::query(
        "INSERT INTO closing_prices (listing_id, price_date, price, source, fetched_at, status, error) \
         VALUES (1, '2026-06-05', '1.23', 'yahoo', 'now', 'ok', NULL)",
    )
    .execute(&pool)
    .await;
    assert!(dup.is_err());
}
