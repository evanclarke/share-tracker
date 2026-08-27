//! The contemporaneous basis (SCENARIOS Q-14).
//!
//! A stored price is the price the security traded at on its own date. The
//! provider restates its whole close series into the *current* basis the
//! moment a security splits, so the figure has to be restated back out of
//! whichever basis it arrived in — which is the basis in force when it was
//! observed, i.e. at `fetched_at`. These pin both halves: normalising on
//! the way in, and re-deriving stored rows when the action set changes.
//!
//! A demerger restates the price series too — and there is no ratio to read
//! (it changes no unit count on this listing), so the factor is derived
//! from the close the operator states the security actually traded at on
//! the last pre-demerger trading day. Evan's LAC history is the live case.

use super::*;
/// The stored price for a listing, and the figure it was observed as.
async fn stored(pool: &SqlitePool, date: NaiveDate) -> (String, String) {
    let row = db_get_one(pool, 1, date).await.unwrap().unwrap();
    (
        row.price.unwrap().normalize().to_string(),
        row.price_as_observed.unwrap().normalize().to_string(),
    )
}

/// A pre-split day fetched *after* the split is recorded arrives in the
/// post-split basis (Yahoo answers 120.888 for a day NVDA closed at
/// 1208.88) and is stored in the day's own basis, with the provider's
/// figure kept beside it.
#[tokio::test]
async fn db_a_price_fetched_after_a_split_is_stored_in_its_own_days_basis() {
    let pool = test_pool().await;
    insert_listing(&pool, 1, "BHP", "XASX", "AUD").await;
    insert_share_split(&pool, 1, ymd(2026, 6, 10), "10", "1").await;

    let market = load_market(&pool, 1).await.unwrap().unwrap();
    let stub = StubFetcher::default().with_close(1, ymd(2026, 6, 5), "120.888", "AUD");
    fetch_and_store(&pool, &stub, &market, &[ymd(2026, 6, 5)])
        .await
        .unwrap();

    assert_eq!(
        stored(&pool, ymd(2026, 6, 5)).await,
        ("1208.88".to_string(), "120.888".to_string()),
        "the provider's post-split figure is restated into the price date's own basis"
    );
}

/// The other half: a day collected *before* the split happened already
/// holds the contemporaneous close, and recording the split later must
/// leave it exactly as it is. This is the case the whole daily-collected
/// history sits in, so a blanket "multiply every earlier price by the
/// ratio" rule would corrupt years of correct prices at a stroke.
#[tokio::test]
async fn db_a_price_observed_before_the_split_is_untouched_when_it_is_recorded() {
    let pool = test_pool().await;
    insert_listing(&pool, 1, "BHP", "XASX", "AUD").await;
    crate::test_support::closing_price(1, ymd(2026, 6, 5))
        .price("1208.88")
        .fetched_at("2026-06-05T08:00:00Z")
        .insert(&pool)
        .await;

    insert_share_split(&pool, 1, ymd(2026, 6, 10), "10", "1").await;

    assert_eq!(
        stored(&pool, ymd(2026, 6, 5)).await,
        ("1208.88".to_string(), "1208.88".to_string()),
        "the fetch predates the split, so the figure was never restated"
    );
}

/// The property the whole design exists for: whichever order the split and
/// the fetch are entered in, the stored price is the same.
#[tokio::test]
async fn db_entry_order_of_the_split_and_the_fetch_does_not_change_the_price() {
    async fn fetch(pool: &SqlitePool) {
        let market = load_market(pool, 1).await.unwrap().unwrap();
        let stub = StubFetcher::default().with_close(1, ymd(2026, 6, 5), "120.888", "AUD");
        fetch_and_store(pool, &stub, &market, &[ymd(2026, 6, 5)])
            .await
            .unwrap();
    }

    // split first, then the fetch
    let a = test_pool().await;
    insert_listing(&a, 1, "BHP", "XASX", "AUD").await;
    insert_share_split(&a, 1, ymd(2026, 6, 10), "10", "1").await;
    fetch(&a).await;

    // the fetch first, then the split
    let b = test_pool().await;
    insert_listing(&b, 1, "BHP", "XASX", "AUD").await;
    fetch(&b).await;
    assert_eq!(
        stored(&b, ymd(2026, 6, 5)).await.0,
        "120.888",
        "with no split recorded there is nothing to restate out of"
    );
    insert_share_split(&b, 1, ymd(2026, 6, 10), "10", "1").await;

    assert_eq!(
        stored(&a, ymd(2026, 6, 5)).await,
        ("1208.88".to_string(), "120.888".to_string())
    );
    assert_eq!(
        stored(&b, ymd(2026, 6, 5)).await,
        stored(&a, ymd(2026, 6, 5)).await,
        "entry order cannot matter"
    );
}

/// A bonus issue re-bases units exactly as a split does (one new share for
/// each held doubles the count), so it halves the per-unit price the same
/// way — and the provider restates for it too.
#[tokio::test]
async fn db_a_bonus_issue_rebases_stored_prices_like_a_split() {
    let pool = test_pool().await;
    insert_listing(&pool, 1, "BHP", "XASX", "AUD").await;
    let market = load_market(&pool, 1).await.unwrap().unwrap();
    let stub = StubFetcher::default().with_close(1, ymd(2026, 6, 5), "30", "AUD");
    fetch_and_store(&pool, &stub, &market, &[ymd(2026, 6, 5)])
        .await
        .unwrap();

    crate::entities::corporate_action::db_upsert(
        &pool,
        &crate::entities::corporate_action::CorporateAction {
            id: 950,
            listing_id: 1,
            date: ymd(2026, 6, 10),
            kind: crate::entities::corporate_action::ActionKind::BonusIssue {
                bonus_units: Decimal::ONE,
                bonus_held_units: Decimal::ONE,
            },
        },
    )
    .await
    .unwrap();

    assert_eq!(
        stored(&pool, ymd(2026, 6, 5)).await.0,
        "60",
        "one bonus share per share held doubles the unit count, so the earlier day's own \
         price is twice the restated one"
    );
}

/// A consolidation (reverse split) runs the error the other way: the
/// provider's restated figure is *larger* than the contemporaneous one.
#[tokio::test]
async fn db_a_consolidation_rebases_stored_prices_the_other_way() {
    let pool = test_pool().await;
    insert_listing(&pool, 1, "BHP", "XASX", "AUD").await;
    insert_share_split(&pool, 1, ymd(2026, 6, 10), "1", "10").await;

    let market = load_market(&pool, 1).await.unwrap().unwrap();
    let stub = StubFetcher::default().with_close(1, ymd(2026, 6, 5), "12088.8", "AUD");
    fetch_and_store(&pool, &stub, &market, &[ymd(2026, 6, 5)])
        .await
        .unwrap();

    assert_eq!(
        stored(&pool, ymd(2026, 6, 5)).await,
        ("1208.88".to_string(), "12088.8".to_string()),
        "ten old units became one, so the pre-consolidation day's price is a tenth"
    );
}

/// A hand-entered price is contemporaneous by declaration: it is stored
/// exactly as typed even with a split already recorded after its date, and
/// recording another one never rewrites it.
#[tokio::test]
async fn api_a_manual_price_is_neither_normalised_on_entry_nor_rebased() {
    let pool = test_pool().await;
    insert_listing(&pool, 1, "BHP", "XASX", "AUD").await;
    insert_share_split(&pool, 1, ymd(2026, 6, 10), "10", "1").await;

    let client = ApiClient::over(router().with_state(pool.clone()));
    client
        .put(
            "/closing_prices/1/2026-06-05",
            &serde_json::json!({
                "price": "1208.88",
                "sourced_from": "asx.com.au closing report",
                "reason": "provider serves no candle for that day",
            }),
        )
        .await
        .expect_status(StatusCode::NO_CONTENT);

    assert_eq!(
        stored(&pool, ymd(2026, 6, 5)).await,
        ("1208.88".to_string(), "1208.88".to_string()),
        "the operator's figure is its own observation"
    );

    insert_share_split(&pool, 1, ymd(2026, 6, 20), "2", "1").await;
    assert_eq!(
        stored(&pool, ymd(2026, 6, 5)).await.0,
        "1208.88",
        "nothing rewrites a figure a person typed"
    );
}

/// Editing the action re-derives the prices from the observation, and
/// deleting it puts them back — neither is a delta applied to an
/// already-adjusted number.
#[tokio::test]
async fn api_editing_or_deleting_the_split_re_derives_the_stored_prices() {
    let pool = test_pool().await;
    insert_listing(&pool, 1, "BHP", "XASX", "AUD").await;
    insert_share_split(&pool, 1, ymd(2026, 6, 10), "10", "1").await;
    let market = load_market(&pool, 1).await.unwrap().unwrap();
    let stub = StubFetcher::default().with_close(1, ymd(2026, 6, 5), "120.888", "AUD");
    fetch_and_store(&pool, &stub, &market, &[ymd(2026, 6, 5)])
        .await
        .unwrap();
    assert_eq!(stored(&pool, ymd(2026, 6, 5)).await.0, "1208.88");

    // A mis-keyed ratio, corrected in place.
    insert_share_split(&pool, 1, ymd(2026, 6, 10), "2", "1").await;
    assert_eq!(
        stored(&pool, ymd(2026, 6, 5)).await.0,
        "241.776",
        "the price follows the corrected ratio, from the observation"
    );

    // Moved to a date before the price: the price date is then already in
    // the post-split basis, so nothing is restated.
    insert_share_split(&pool, 1, ymd(2026, 6, 1), "2", "1").await;
    assert_eq!(
        stored(&pool, ymd(2026, 6, 5)).await.0,
        "120.888",
        "a split on or before the price date has already restated that day's close"
    );

    // …and deleting it altogether leaves the provider's figure standing.
    crate::entities::corporate_action::db_delete(&pool, 901)
        .await
        .unwrap();
    assert_eq!(stored(&pool, ymd(2026, 6, 5)).await.0, "120.888");
}

/// Deleting the *only* re-basing action leaves the listing with an empty
/// event set, and the prices have to come back to the figures as observed
/// — the case the walk must not short-circuit past.
#[tokio::test]
async fn db_deleting_the_last_split_puts_the_prices_back_to_the_observation() {
    let pool = test_pool().await;
    insert_listing(&pool, 1, "BHP", "XASX", "AUD").await;
    insert_share_split(&pool, 1, ymd(2026, 6, 10), "10", "1").await;
    let market = load_market(&pool, 1).await.unwrap().unwrap();
    let stub = StubFetcher::default().with_close(1, ymd(2026, 6, 5), "120.888", "AUD");
    fetch_and_store(&pool, &stub, &market, &[ymd(2026, 6, 5)])
        .await
        .unwrap();
    assert_eq!(stored(&pool, ymd(2026, 6, 5)).await.0, "1208.88");

    crate::entities::corporate_action::db_delete(&pool, 901)
        .await
        .unwrap();
    assert_eq!(
        stored(&pool, ymd(2026, 6, 5)).await,
        ("120.888".to_string(), "120.888".to_string()),
        "with the split gone there is nothing to restate out of any more"
    );
}

/// The one-off repair: a database whose prices were stored before this
/// rule existed holds the provider's restated figure with a split already
/// recorded. `run_rebase` (the `price-rebase` job) re-derives them, and is
/// idempotent.
#[tokio::test]
async fn db_the_rebase_job_repairs_prices_stored_before_the_rule_existed() {
    let pool = test_pool().await;
    insert_listing(&pool, 1, "BHP", "XASX", "AUD").await;
    insert_share_split(&pool, 1, ymd(2026, 6, 10), "10", "1").await;
    // Stored the way a pre-0034 row was: the provider's post-split figure,
    // observed after the split, with nothing restated out of it.
    crate::test_support::closing_price(1, ymd(2026, 6, 5))
        .price("120.888")
        .fetched_at("2026-06-15T08:00:00Z")
        .insert(&pool)
        .await;

    run_rebase(&pool).await.unwrap();
    assert_eq!(stored(&pool, ymd(2026, 6, 5)).await.0, "1208.88");

    run_rebase(&pool).await.unwrap();
    assert_eq!(
        stored(&pool, ymd(2026, 6, 5)).await.0,
        "1208.88",
        "re-deriving from the observation is idempotent"
    );
}

/// Record a demerger of `listing_id` into `demerged_id`, optionally
/// carrying the stated pre-demerger close the price factor is derived
/// from, through the entity's own write path.
async fn insert_demerger(
    pool: &SqlitePool,
    listing_id: i64,
    demerged_id: i64,
    date: NaiveDate,
    stated_close: Option<(NaiveDate, &str)>,
) {
    crate::entities::corporate_action::db_upsert(
        pool,
        &crate::entities::corporate_action::CorporateAction {
            id: 800 + listing_id,
            listing_id,
            date,
            kind: crate::entities::corporate_action::ActionKind::Demerger {
                demerger_listing_id: demerged_id,
                demerger_new_units: Decimal::ONE,
                demerger_held_units: Decimal::ONE,
                demerger_cost_base_pct: Decimal::from(36),
                demerger_close_date: stated_close.map(|(d, _)| d),
                demerger_close_price: stated_close.map(|(_, p)| p.parse().unwrap()),
                demerger_close_sourced_from: stated_close
                    .map(|_| "nyse.com daily close".to_string()),
                demerger_close_reason: stated_close
                    .map(|_| "the provider adjusts the pre-demerger series".to_string()),
            },
        },
    )
    .await
    .unwrap();
}

/// The LAC reproduction. The provider serves the whole pre-demerger series
/// adjusted by its spin-off factor, so the day LAC actually closed at
/// US$24.90 comes back as 10.13. Stating that close derives the factor and
/// re-bases every pre-demerger day with it — the reference day back to
/// exactly the stated figure, the days around it in proportion.
#[tokio::test]
async fn db_a_stated_pre_demerger_close_restates_the_whole_pre_demerger_series() {
    let pool = test_pool().await;
    insert_listing(&pool, 1, "LAC", "XNYS", "USD").await;
    insert_listing(&pool, 2, "LAR", "XNYS", "USD").await;
    let market = load_market(&pool, 1).await.unwrap().unwrap();
    let stub = StubFetcher::default()
        .with_close(1, ymd(2023, 9, 29), "10.00", "USD")
        .with_close(1, ymd(2023, 10, 2), "10.13", "USD")
        .with_close(1, ymd(2023, 10, 4), "11.72", "USD");
    fetch_and_store(
        &pool,
        &stub,
        &market,
        &[ymd(2023, 9, 29), ymd(2023, 10, 2), ymd(2023, 10, 4)],
    )
    .await
    .unwrap();
    assert_eq!(
        stored(&pool, ymd(2023, 10, 2)).await.0,
        "10.13",
        "with no stated close there is nothing to restate out of"
    );

    insert_demerger(
        &pool,
        1,
        2,
        ymd(2023, 10, 3),
        Some((ymd(2023, 10, 2), "24.90")),
    )
    .await;

    assert_eq!(
        stored(&pool, ymd(2023, 10, 2)).await,
        ("24.9".to_string(), "10.13".to_string()),
        "the reference day comes back to exactly the close the operator stated"
    );
    assert_eq!(
        stored(&pool, ymd(2023, 9, 29)).await.0,
        // 10.00 × 24.90/10.13, held to the provider's 7 significant digits.
        "24.58045",
        "every other pre-demerger day moves by the same derived factor"
    );
    assert_eq!(
        stored(&pool, ymd(2023, 10, 4)).await.0,
        "11.72",
        "a post-demerger day was never restated by the provider"
    );
}

/// The other half, as for a split: a pre-demerger day collected *before*
/// the demerger already holds the contemporaneous close, and stating one
/// later must leave it exactly as it is.
#[tokio::test]
async fn db_a_pre_demerger_price_observed_before_the_demerger_is_untouched() {
    let pool = test_pool().await;
    insert_listing(&pool, 1, "LAC", "XNYS", "USD").await;
    insert_listing(&pool, 2, "LAR", "XNYS", "USD").await;
    // Observed the day it traded — before the demerger, so contemporaneous.
    crate::test_support::closing_price(1, ymd(2023, 9, 29))
        .price("24.58")
        .fetched_at("2023-09-29T21:00:00Z")
        .insert(&pool)
        .await;
    // …and the reference day, observed long after it, which is what the
    // factor is derived from.
    crate::test_support::closing_price(1, ymd(2023, 10, 2))
        .price("10.13")
        .fetched_at("2026-07-26T07:44:56Z")
        .insert(&pool)
        .await;

    insert_demerger(
        &pool,
        1,
        2,
        ymd(2023, 10, 3),
        Some((ymd(2023, 10, 2), "24.90")),
    )
    .await;

    assert_eq!(
        stored(&pool, ymd(2023, 9, 29)).await.0,
        "24.58",
        "the fetch predates the demerger, so the figure was never adjusted"
    );
    assert_eq!(stored(&pool, ymd(2023, 10, 2)).await.0, "24.9");
}

/// The entry-order property, both ways: state the close before the history
/// is backfilled, or backfill first and state it after. The reference row
/// the factor divides by is one of the rows being fetched, so the fetch
/// funnel re-derives once its range has landed.
#[tokio::test]
async fn db_entry_order_of_the_stated_close_and_the_backfill_does_not_change_the_price() {
    async fn backfill(pool: &SqlitePool) {
        let market = load_market(pool, 1).await.unwrap().unwrap();
        let stub = StubFetcher::default()
            .with_close(1, ymd(2023, 9, 29), "10.00", "USD")
            .with_close(1, ymd(2023, 10, 2), "10.13", "USD");
        fetch_and_store(pool, &stub, &market, &[ymd(2023, 9, 29), ymd(2023, 10, 2)])
            .await
            .unwrap();
    }
    async fn setup(pool: &SqlitePool) {
        insert_listing(pool, 1, "LAC", "XNYS", "USD").await;
        insert_listing(pool, 2, "LAR", "XNYS", "USD").await;
    }

    // The close stated first, then the history backfilled.
    let a = test_pool().await;
    setup(&a).await;
    insert_demerger(
        &a,
        1,
        2,
        ymd(2023, 10, 3),
        Some((ymd(2023, 10, 2), "24.90")),
    )
    .await;
    backfill(&a).await;

    // The history backfilled first, then the close stated.
    let b = test_pool().await;
    setup(&b).await;
    backfill(&b).await;
    insert_demerger(
        &b,
        1,
        2,
        ymd(2023, 10, 3),
        Some((ymd(2023, 10, 2), "24.90")),
    )
    .await;

    for date in [ymd(2023, 9, 29), ymd(2023, 10, 2)] {
        assert_eq!(
            stored(&a, date).await,
            stored(&b, date).await,
            "entry order cannot matter for {date}"
        );
    }
    assert_eq!(stored(&a, ymd(2023, 10, 2)).await.0, "24.9");
    assert_eq!(stored(&a, ymd(2023, 9, 29)).await.0, "24.58045");
}

/// A demerger and a split on the same listing compose: the split restated
/// the reference figure too, so the derived demerger factor must divide it
/// out rather than absorb it — otherwise the split would be applied twice
/// to every pre-demerger day.
#[tokio::test]
async fn db_a_demerger_and_a_later_split_compose_without_double_counting() {
    let pool = test_pool().await;
    insert_listing(&pool, 1, "LAC", "XNYS", "USD").await;
    insert_listing(&pool, 2, "LAR", "XNYS", "USD").await;
    // A 2-for-1 split after the demerger: the provider halves everything
    // before it, on top of the spin-off adjustment.
    insert_share_split(&pool, 1, ymd(2024, 5, 1), "2", "1").await;
    insert_demerger(
        &pool,
        1,
        2,
        ymd(2023, 10, 3),
        Some((ymd(2023, 10, 2), "24.90")),
    )
    .await;

    let market = load_market(&pool, 1).await.unwrap().unwrap();
    // What the provider serves today: 24.90 × (10.13/24.90 spin-off) × 1/2.
    let stub = StubFetcher::default()
        .with_close(1, ymd(2023, 10, 2), "5.065", "USD")
        .with_close(1, ymd(2024, 6, 3), "7.50", "USD");
    fetch_and_store(&pool, &stub, &market, &[ymd(2023, 10, 2), ymd(2024, 6, 3)])
        .await
        .unwrap();

    assert_eq!(
        stored(&pool, ymd(2023, 10, 2)).await.0,
        "24.9",
        "the split is undone once and the spin-off once — not the split twice"
    );
    assert_eq!(
        stored(&pool, ymd(2024, 6, 3)).await.0,
        "7.5",
        "a day after both events is served in its own basis already"
    );
}

/// Editing the stated close re-derives the prices from the observation,
/// removing it puts them back, and deleting the whole demerger does too —
/// none of them a delta applied to an already-adjusted number.
#[tokio::test]
async fn db_editing_or_removing_the_stated_close_re_derives_the_prices() {
    let pool = test_pool().await;
    insert_listing(&pool, 1, "LAC", "XNYS", "USD").await;
    insert_listing(&pool, 2, "LAR", "XNYS", "USD").await;
    let market = load_market(&pool, 1).await.unwrap().unwrap();
    let stub = StubFetcher::default().with_close(1, ymd(2023, 10, 2), "10.13", "USD");
    fetch_and_store(&pool, &stub, &market, &[ymd(2023, 10, 2)])
        .await
        .unwrap();

    insert_demerger(
        &pool,
        1,
        2,
        ymd(2023, 10, 3),
        Some((ymd(2023, 10, 2), "24.90")),
    )
    .await;
    assert_eq!(stored(&pool, ymd(2023, 10, 2)).await.0, "24.9");

    // A mis-keyed close, corrected in place.
    insert_demerger(
        &pool,
        1,
        2,
        ymd(2023, 10, 3),
        Some((ymd(2023, 10, 2), "24.95")),
    )
    .await;
    assert_eq!(stored(&pool, ymd(2023, 10, 2)).await.0, "24.95");

    // Removing the statement altogether leaves the provider's figure.
    insert_demerger(&pool, 1, 2, ymd(2023, 10, 3), None).await;
    assert_eq!(stored(&pool, ymd(2023, 10, 2)).await.0, "10.13");

    // …as does deleting the demerger, once it is stated again.
    insert_demerger(
        &pool,
        1,
        2,
        ymd(2023, 10, 3),
        Some((ymd(2023, 10, 2), "24.90")),
    )
    .await;
    assert_eq!(stored(&pool, ymd(2023, 10, 2)).await.0, "24.9");
    crate::entities::corporate_action::db_delete(&pool, 801)
        .await
        .unwrap();
    assert_eq!(stored(&pool, ymd(2023, 10, 2)).await.0, "10.13");
}

/// The one-off repair path is the existing `price-rebase` job, extended
/// rather than duplicated: a database whose pre-demerger prices were
/// stored before the demerger's close was stated is repaired by it, and
/// running it again is a no-op.
#[tokio::test]
async fn db_the_rebase_job_repairs_prices_a_demerger_restated() {
    let pool = test_pool().await;
    insert_listing(&pool, 1, "LAC", "XNYS", "USD").await;
    insert_listing(&pool, 2, "LAR", "XNYS", "USD").await;
    // Stored the way the live rows are: the provider's adjusted figure,
    // observed years after the demerger, with nothing taken out of it.
    for (date, price) in [(ymd(2023, 9, 29), "10.00"), (ymd(2023, 10, 2), "10.13")] {
        crate::test_support::closing_price(1, date)
            .price(price)
            .fetched_at("2026-07-26T07:44:56Z")
            .insert(&pool)
            .await;
    }
    // Written straight to the table, as a database predating the column
    // would have had it re-entered afterwards.
    sqlx::query("UPDATE closing_prices SET price = price_as_observed WHERE listing_id = 1")
        .execute(&pool)
        .await
        .unwrap();
    insert_demerger(
        &pool,
        1,
        2,
        ymd(2023, 10, 3),
        Some((ymd(2023, 10, 2), "24.90")),
    )
    .await;

    run_rebase(&pool).await.unwrap();
    assert_eq!(stored(&pool, ymd(2023, 10, 2)).await.0, "24.9");
    assert_eq!(stored(&pool, ymd(2023, 9, 29)).await.0, "24.58045");

    run_rebase(&pool).await.unwrap();
    assert_eq!(
        stored(&pool, ymd(2023, 10, 2)).await.0,
        "24.9",
        "re-deriving from the observation is idempotent"
    );
    assert_eq!(stored(&pool, ymd(2023, 9, 29)).await.0, "24.58045");
}

/// A hand-entered pre-demerger price is contemporaneous by declaration, so
/// a stated close never rewrites it — the same one-way rule a split obeys.
#[tokio::test]
async fn api_a_manual_pre_demerger_price_is_never_rebased_by_a_stated_close() {
    let pool = test_pool().await;
    insert_listing(&pool, 1, "LAC", "XNYS", "USD").await;
    insert_listing(&pool, 2, "LAR", "XNYS", "USD").await;
    crate::test_support::closing_price(1, ymd(2023, 9, 29))
        .price("24.58")
        .fetched_at("2026-07-26T07:44:56Z")
        .manual("nyse.com", "provider adjusts the pre-demerger series")
        .insert(&pool)
        .await;
    crate::test_support::closing_price(1, ymd(2023, 10, 2))
        .price("10.13")
        .fetched_at("2026-07-26T07:44:56Z")
        .insert(&pool)
        .await;

    insert_demerger(
        &pool,
        1,
        2,
        ymd(2023, 10, 3),
        Some((ymd(2023, 10, 2), "24.90")),
    )
    .await;

    assert_eq!(
        stored(&pool, ymd(2023, 9, 29)).await.0,
        "24.58",
        "nothing rewrites a figure a person typed"
    );
    assert_eq!(stored(&pool, ymd(2023, 10, 2)).await.0, "24.9");
}

/// A re-base is an UPDATE of an audited table, so the superseded figure is
/// recoverable — and it stales the snapshots that were valued at it.
#[tokio::test]
async fn db_a_rebase_is_audited_and_stales_the_snapshots_it_moves() {
    let pool = test_pool().await;
    insert_listing(&pool, 1, "BHP", "XASX", "AUD").await;
    let market = load_market(&pool, 1).await.unwrap().unwrap();
    let stub = StubFetcher::default().with_close(1, ymd(2026, 6, 5), "120.888", "AUD");
    fetch_and_store(&pool, &stub, &market, &[ymd(2026, 6, 5)])
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO report_snapshots (report, snapshot_date, generated_at, stale, rows_json) \
         VALUES ('portfolio_overview', '2026-06-05', '2026-06-06T00:00:00Z', 0, '[]')",
    )
    .execute(&pool)
    .await
    .unwrap();

    insert_share_split(&pool, 1, ymd(2026, 6, 10), "10", "1").await;

    let old: Vec<String> = sqlx::query_scalar(
        "SELECT json_extract(old_row, '$.price') FROM row_history \
         WHERE table_name = 'closing_prices' ORDER BY id",
    )
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(
        old,
        vec!["120.888".to_string()],
        "the superseded figure is retained"
    );

    let stale: i64 =
        sqlx::query_scalar("SELECT stale FROM report_snapshots WHERE snapshot_date = '2026-06-05'")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(
        stale, 1,
        "the valuation that used the old figure regenerates"
    );
}
