//! DB-level and API-level tests for the distribution calendar.

use super::test_support::DistributionStub;
use super::*;
use crate::test_support::{ApiClient, allocate, buy, dec, listing, sell, test_pool, ymd};

fn client(pool: &SqlitePool) -> ApiClient {
    ApiClient::over(router().with_state(pool.clone()))
}

/// A stub that answers for `listing_id` with one event, and the job run that
/// stores it.
async fn refresh(
    pool: &SqlitePool,
    fetcher: &dyn DistributionFetcher,
) -> Result<Option<String>, String> {
    run_refresh(
        pool,
        fetcher,
        ymd(2026, 8, 27).and_hms_opt(0, 0, 0).unwrap().and_utc(),
    )
    .await
}

#[tokio::test]
async fn a_refresh_stores_the_provider_events_for_a_held_listing() {
    let pool = test_pool().await;
    listing(1).insert(&pool).await;
    buy(1, 1)
        .date(ymd(2024, 1, 10))
        .qty(dec("100"))
        .insert(&pool)
        .await;

    let stub = DistributionStub::default()
        .with_event(1, ymd(2024, 7, 1), dec("0.726547"), "AUD")
        .with_event(1, ymd(2025, 1, 2), dec("0.018741"), "AUD");
    assert_eq!(refresh(&pool, &stub).await, Ok(None));

    let stored = db_list(&pool).await.unwrap();
    assert_eq!(stored.len(), 2);
    // Newest ex-date first, and every provenance column recorded.
    assert_eq!(stored[0].ex_date, ymd(2025, 1, 2));
    assert_eq!(stored[0].amount_per_unit, dec("0.018741"));
    assert_eq!(stored[0].currency, "AUD");
    assert_eq!(stored[0].source, "stub");
    assert_eq!(stored[0].fetched_symbol, "T1");
    assert!(stored[0].fetched_at.starts_with("2026-08-27T"));
    assert_eq!(stored[1].ex_date, ymd(2024, 7, 1));
}

#[tokio::test]
async fn a_listing_never_held_is_not_fetched_at_all() {
    let pool = test_pool().await;
    listing(1).insert(&pool).await;
    // No trade: the listing exists but was never held, so there is no span to
    // ask the provider about and no distribution anyone could have missed.
    let stub = DistributionStub::default().with_event(1, ymd(2024, 7, 1), dec("1"), "AUD");
    assert_eq!(refresh(&pool, &stub).await, Ok(None));
    assert!(db_list(&pool).await.unwrap().is_empty());
}

#[tokio::test]
async fn the_window_is_the_held_span_so_an_event_outside_it_is_not_stored() {
    let pool = test_pool().await;
    listing(1).insert(&pool).await;
    buy(1, 1)
        .date(ymd(2024, 1, 10))
        .qty(dec("100"))
        .insert(&pool)
        .await;
    sell(2, 1)
        .date(ymd(2024, 9, 30))
        .qty(dec("100"))
        .insert(&pool)
        .await;
    // The holding timeline is built from parcel allocations, not from a Sell's
    // own quantity — so the sale only closes the parcel once it is allocated.
    allocate(&pool, 1, 2, 1, dec("100")).await;

    let stub = DistributionStub::default()
        // Before the parcel was acquired, and after it was sold out.
        .with_event(1, ymd(2023, 7, 1), dec("1"), "AUD")
        .with_event(1, ymd(2024, 7, 1), dec("1"), "AUD")
        .with_event(1, ymd(2025, 7, 1), dec("1"), "AUD");
    refresh(&pool, &stub).await.unwrap();

    let stored = db_list(&pool).await.unwrap();
    assert_eq!(
        stored.iter().map(|e| e.ex_date).collect::<Vec<_>>(),
        vec![ymd(2024, 7, 1)]
    );
}

#[tokio::test]
async fn a_re_fetch_updates_the_amount_in_place_and_keeps_the_row_id() {
    let pool = test_pool().await;
    listing(1).insert(&pool).await;
    buy(1, 1)
        .date(ymd(2024, 1, 10))
        .qty(dec("100"))
        .insert(&pool)
        .await;

    refresh(
        &pool,
        &DistributionStub::default().with_event(1, ymd(2024, 7, 1), dec("0.70"), "AUD"),
    )
    .await
    .unwrap();
    let first = db_list(&pool).await.unwrap();
    assert_eq!(first.len(), 1);

    refresh(
        &pool,
        &DistributionStub::default().with_event(1, ymd(2024, 7, 1), dec("0.726547"), "AUD"),
    )
    .await
    .unwrap();
    let second = db_list(&pool).await.unwrap();
    assert_eq!(second.len(), 1, "the natural key is (listing, ex_date)");
    assert_eq!(second[0].amount_per_unit, dec("0.726547"));
    assert_eq!(
        second[0].id, first[0].id,
        "the surrogate id survives, so the row keeps its own audit trail"
    );

    // …and the trail records what it said before.
    let trail: Vec<(String, String)> = sqlx::query_as(
        "SELECT operation, old_row FROM row_history \
         WHERE table_name = 'distribution_events' ORDER BY id",
    )
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(trail.len(), 1);
    assert_eq!(trail[0].0, "UPDATE");
    assert!(trail[0].1.contains("0.70"));
}

#[tokio::test]
async fn a_refresh_never_deletes_an_event_the_provider_stops_serving() {
    let pool = test_pool().await;
    listing(1).insert(&pool).await;
    buy(1, 1)
        .date(ymd(2024, 1, 10))
        .qty(dec("100"))
        .insert(&pool)
        .await;

    refresh(
        &pool,
        &DistributionStub::default().with_event(1, ymd(2024, 7, 1), dec("0.70"), "AUD"),
    )
    .await
    .unwrap();
    // The provider now knows of nothing at all — an outage that answered
    // rather than failed, or a history it silently truncated.
    refresh(&pool, &DistributionStub::default()).await.unwrap();
    assert_eq!(db_list(&pool).await.unwrap().len(), 1);
}

#[tokio::test]
async fn a_provider_failure_fails_the_run_rather_than_reading_as_no_distribution() {
    let pool = test_pool().await;
    listing(1).insert(&pool).await;
    buy(1, 1)
        .date(ymd(2024, 1, 10))
        .qty(dec("100"))
        .insert(&pool)
        .await;

    let error = refresh(&pool, &DistributionStub::failing("upstream 503"))
        .await
        .unwrap_err();
    assert!(error.contains("T1 (1)"), "{error}");
    assert!(error.contains("upstream 503"), "{error}");
    assert!(db_list(&pool).await.unwrap().is_empty());
}

#[tokio::test]
async fn an_undatable_event_qualifies_the_run_instead_of_vanishing() {
    let pool = test_pool().await;
    listing(1).insert(&pool).await;
    buy(1, 1)
        .date(ymd(2024, 1, 10))
        .qty(dec("100"))
        .insert(&pool)
        .await;

    let stub = DistributionStub::default()
        .with_event(1, ymd(2024, 7, 1), dec("0.70"), "AUD")
        .with_undatable(1, ymd(2025, 1, 1));
    let note = refresh(&pool, &stub).await.unwrap().expect("a note");
    assert!(note.contains("could not be placed"), "{note}");
    assert!(note.contains("2025-01-01"), "{note}");
    // The run still succeeded and still stored what it could.
    assert_eq!(db_list(&pool).await.unwrap().len(), 1);
}

/// A ticker the provider has retired must not fail the weekly job **forever**.
///
/// The distinction is drawn from the provider's own typed error, so the
/// control is the other arm: an outage carries no verdict on the symbol and
/// still fails. Found against the real portfolio — LAR, whose whole held span
/// predates its 2025 rename from LAAC, a symbol Yahoo now 404s.
#[tokio::test]
async fn a_retired_ticker_notes_the_run_rather_than_failing_it_forever() {
    let pool = test_pool().await;
    listing(1).insert(&pool).await;
    buy(1, 1)
        .date(ymd(2024, 1, 10))
        .qty(dec("100"))
        .insert(&pool)
        .await;

    let note = refresh(&pool, &DistributionStub::retired("Not found at …/LAAC"))
        .await
        .expect("a retired ticker is not a failure")
        .expect("but it does qualify the run");
    assert!(note.contains("no calendar"), "{note}");
    assert!(note.contains("T1 (1)"), "{note}");
    assert!(
        note.contains("cannot speak for them"),
        "the note says what the alerts can no longer claim: {note}"
    );

    // The control: the *other* arm of the same classification still fails, so
    // this is the provider's verdict being read, not every failure going quiet.
    assert!(
        refresh(&pool, &DistributionStub::failing("upstream 503"))
            .await
            .is_err()
    );
}

#[tokio::test]
async fn a_currency_the_listing_does_not_trade_in_is_refused() {
    let pool = test_pool().await;
    listing(1).insert(&pool).await; // AUD
    buy(1, 1)
        .date(ymd(2024, 1, 10))
        .qty(dec("100"))
        .insert(&pool)
        .await;

    let stub = DistributionStub::default().with_event(1, ymd(2024, 7, 1), dec("0.70"), "USD");
    let error = refresh(&pool, &stub).await.unwrap_err();
    assert!(error.contains("USD"), "{error}");
    assert!(error.contains("AUD"), "{error}");
    assert!(
        db_list(&pool).await.unwrap().is_empty(),
        "the listing's whole set rolls back rather than landing half-stored"
    );
}

#[tokio::test]
async fn a_non_positive_amount_is_refused() {
    let pool = test_pool().await;
    listing(1).insert(&pool).await;
    buy(1, 1)
        .date(ymd(2024, 1, 10))
        .qty(dec("100"))
        .insert(&pool)
        .await;

    let stub = DistributionStub::default().with_event(1, ymd(2024, 7, 1), dec("0"), "AUD");
    let error = refresh(&pool, &stub).await.unwrap_err();
    assert!(error.contains("not a positive amount"), "{error}");
}

#[tokio::test]
async fn the_list_and_get_routes_serve_the_stored_calendar() {
    let pool = test_pool().await;
    listing(1).insert(&pool).await;
    buy(1, 1)
        .date(ymd(2024, 1, 10))
        .qty(dec("100"))
        .insert(&pool)
        .await;
    refresh(
        &pool,
        &DistributionStub::default().with_event(1, ymd(2024, 7, 1), dec("0.726547"), "AUD"),
    )
    .await
    .unwrap();

    let api = client(&pool);
    let listed: Vec<DistributionEvent> = api.get_json("/distribution_events").await;
    assert_eq!(listed.len(), 1);
    let one: DistributionEvent = api
        .get_json(&format!("/distribution_events/{}", listed[0].id))
        .await;
    assert_eq!(one.ex_date, ymd(2024, 7, 1));
    assert_eq!(one.amount_per_unit, dec("0.726547"));

    // A `GET` of a missing row is the house's bare 404 — only a DELETE names
    // the noun, and this entity has no DELETE (the calendar is provider-owned).
    api.get("/distribution_events/9999")
        .await
        .expect_status(axum::http::StatusCode::NOT_FOUND);
}

/// Record a rename directly, the way `domain::listing_identity`'s own tests
/// do: the entity's operation rewrites the listing's ticker as a side effect,
/// and what this needs is only the chain the market timeline is built from.
async fn rename(
    pool: &SqlitePool,
    listing_id: i64,
    effective_date: NaiveDate,
    old_ticker: &str,
    new_ticker: &str,
) {
    sqlx::query(
        "INSERT INTO listing_renames (listing_id, effective_date, old_ticker, new_ticker) \
         VALUES (?, ?, ?, ?)",
    )
    .bind(listing_id)
    .bind(effective_date)
    .bind(old_ticker)
    .bind(new_ticker)
    .execute(pool)
    .await
    .unwrap();
}

/// A held span straddling a rename is fetched under **each** ticker that was
/// actually in force over it, not once under the one in force at its start.
///
/// The real case: LAR's held span begins before its 2025 rename from LAAC.
/// Asking for the whole span under LAAC gets Yahoo's `NoSuchSymbol` for a
/// ticker it retired, and before this the listing was left with no calendar at
/// all — both alerts silent for it forever — while its post-rename history sat
/// there to be collected under LAR.
#[tokio::test]
async fn a_span_straddling_a_rename_is_fetched_under_each_ticker_in_force() {
    let pool = test_pool().await;
    listing(1).ticker("LAR").insert(&pool).await;
    rename(&pool, 1, ymd(2025, 3, 1), "LAAC", "LAR").await;
    buy(1, 1)
        .date(ymd(2024, 1, 10))
        .qty(dec("100"))
        .insert(&pool)
        .await;

    // The provider serves the series only under the surviving ticker.
    let stub = DistributionStub::default().serving_only("LAR").with_event(
        1,
        ymd(2025, 7, 1),
        dec("0.31"),
        "AUD",
    );
    let note = refresh(&pool, &stub).await.expect("the run succeeds");

    // Two calls, split on the effective date, each under its own ticker.
    assert_eq!(
        stub.calls(),
        vec![
            ("LAAC".to_string(), ymd(2024, 1, 10), ymd(2025, 2, 28)),
            ("LAR".to_string(), ymd(2025, 3, 1), ymd(2026, 8, 27)),
        ]
    );
    // The post-rename half is collected rather than lost with the pre-rename
    // half — the whole point of segmenting.
    let stored = db_list(&pool).await.unwrap();
    assert_eq!(stored.len(), 1);
    assert_eq!(stored[0].ex_date, ymd(2025, 7, 1));
    assert_eq!(
        stored[0].fetched_symbol, "LAR",
        "the row records the symbol its own segment was fetched under"
    );
    // …and the span that genuinely has no calendar is still said out loud,
    // naming the symbol and the window rather than the listing as a whole.
    let note = note.expect("the retired span qualifies the run");
    assert!(note.contains("no calendar"), "{note}");
    assert!(note.contains("as LAAC"), "{note}");
    assert!(note.contains("2024-01-10..2025-02-28"), "{note}");
}

/// The control for the segmented fetch: an outage on **one** segment carries
/// no verdict on the symbol, so it fails the listing loudly rather than
/// quietly storing half a history.
#[tokio::test]
async fn an_outage_on_one_segment_still_fails_the_whole_listing() {
    let pool = test_pool().await;
    listing(1).ticker("LAR").insert(&pool).await;
    rename(&pool, 1, ymd(2025, 3, 1), "LAAC", "LAR").await;
    buy(1, 1)
        .date(ymd(2024, 1, 10))
        .qty(dec("100"))
        .insert(&pool)
        .await;

    let error = refresh(&pool, &DistributionStub::failing("upstream 503"))
        .await
        .unwrap_err();
    assert!(error.contains("LAR (1)"), "{error}");
    assert!(error.contains("upstream 503"), "{error}");
    assert!(db_list(&pool).await.unwrap().is_empty());
}

/// A weekly run over years of unchanged history must leave the audit trail
/// alone.
///
/// The job re-stores each listing's whole held span every run, so left to bump
/// `fetched_at` alone it would UPDATE every stored event every Monday — and
/// `row_history` is append-only with keep-forever retention, so the Row
/// History screen's browse mode would show nothing but refresh noise.
#[tokio::test]
async fn re_storing_an_unrevised_event_writes_nothing_at_all() {
    let pool = test_pool().await;
    listing(1).insert(&pool).await;
    buy(1, 1)
        .date(ymd(2024, 1, 10))
        .qty(dec("100"))
        .insert(&pool)
        .await;

    let stub = DistributionStub::default().with_event(1, ymd(2024, 7, 1), dec("0.726547"), "AUD");
    refresh(&pool, &stub).await.unwrap();
    let first = db_list(&pool).await.unwrap();

    // The next week's run, at a later instant, over the same answer.
    run_refresh(
        &pool,
        &stub,
        ymd(2026, 9, 3).and_hms_opt(0, 0, 0).unwrap().and_utc(),
    )
    .await
    .unwrap();

    let second = db_list(&pool).await.unwrap();
    assert_eq!(second.len(), 1);
    assert_eq!(
        second[0].fetched_at, first[0].fetched_at,
        "fetched_at dates the answer that last changed the row — and with it \
         the unit basis the amount is in, which an unrevised amount proves has \
         not moved"
    );
    let trail: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM row_history WHERE table_name = 'distribution_events'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(
        trail, 0,
        "nothing changed, so nothing is recorded as having"
    );
}

/// The control for the no-op guard: a re-fetch whose **symbol** differs is a
/// real change of provenance and is still written — the guard tests the whole
/// of the provider's answer, not just its amount.
#[tokio::test]
async fn a_re_fetch_under_a_different_symbol_is_still_recorded() {
    let pool = test_pool().await;
    listing(1).ticker("LAAC").insert(&pool).await;
    buy(1, 1)
        .date(ymd(2024, 1, 10))
        .qty(dec("100"))
        .insert(&pool)
        .await;
    let stub = DistributionStub::default().with_event(1, ymd(2025, 7, 1), dec("0.31"), "AUD");
    refresh(&pool, &stub).await.unwrap();
    assert_eq!(db_list(&pool).await.unwrap()[0].fetched_symbol, "LAAC");

    // The security is renamed with effect from before that ex-date, so the
    // event now falls in the segment quoted under the new ticker.
    rename(&pool, 1, ymd(2025, 3, 1), "LAAC", "LAR").await;
    sqlx::query("UPDATE listings SET ticker = 'LAR' WHERE id = 1")
        .execute(&pool)
        .await
        .unwrap();
    run_refresh(
        &pool,
        &stub,
        ymd(2026, 9, 3).and_hms_opt(0, 0, 0).unwrap().and_utc(),
    )
    .await
    .unwrap();

    let stored = db_list(&pool).await.unwrap();
    assert_eq!(stored.len(), 1);
    assert_eq!(stored[0].fetched_symbol, "LAR");
    assert!(
        stored[0].fetched_at.starts_with("2026-09-03T"),
        "a real change re-dates the answer: {}",
        stored[0].fetched_at
    );
    let trail: Vec<String> = sqlx::query_scalar(
        "SELECT old_row FROM row_history WHERE table_name = 'distribution_events'",
    )
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(trail.len(), 1, "the provenance change is recorded");
    assert!(trail[0].contains("LAAC"), "{}", trail[0]);
}
