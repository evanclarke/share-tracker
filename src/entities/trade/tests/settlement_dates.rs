//! Settlement-date auto-population (T+n business days over the exchange's
//! holiday calendar, same-day for crypto) and its recompute.

use super::*;

#[tokio::test]
async fn api_settlement_date_auto_populated() {
    let pool = test_pool().await;
    insert_test_listing(&pool).await;
    // XASX has settlement_days = 2, so 2024-01-15 + 2 = 2024-01-17
    let body = serde_json::json!({
        "trade_type": "Buy",
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
    assert_eq!(resp.status, StatusCode::CREATED);
    let trade = db_get(&pool, TradeId(1)).await.unwrap().unwrap();
    assert_eq!(
        trade.settlement_date,
        NaiveDate::from_ymd_opt(2024, 1, 17).unwrap()
    );
}

/// An exchange-less (Crypto) listing settles same-day: the auto-populated
/// settlement date is the trade date itself — a Friday stays a Friday (no
/// T+n, no business-day skipping) — and no coverage warning fires (there
/// is no holiday calendar to be outside of).
#[tracing_test::traced_test]
#[tokio::test]
async fn api_settlement_date_same_day_for_crypto() {
    let pool = test_pool().await;
    test_support::listing(1)
        .crypto()
        .ticker("BTC")
        .name("Bitcoin")
        .insert(&pool)
        .await;
    // 2024-06-07 is a Friday: on a T+2 exchange listing this would settle
    // the following Wednesday (the weekend, then the King's Birthday
    // Monday), so a same-day settlement can only come from the
    // exchange-less path.
    let body = serde_json::json!({
        "trade_type": "Buy",
        "date": "2024-06-07",
        "listing_id": 1,
        "average_price": "65000",
        "quantity": "0.12345678",
        "currency": "AUD",
        "brokerage": "0",
        "gst_on_brokerage": "0",
        "brokerage_currency": "AUD",
        "fx_rate": "1"
    });
    let resp = client(&pool).put("/trades/1", &body).await;
    assert_eq!(resp.status, StatusCode::CREATED);
    let trade = db_get(&pool, TradeId(1)).await.unwrap().unwrap();
    assert_eq!(
        trade.settlement_date,
        NaiveDate::from_ymd_opt(2024, 6, 7).unwrap()
    );
    assert!(!logs_contain(
        "settlement window outside seeded exchange-holiday coverage"
    ));
}

#[tracing_test::traced_test]
#[tokio::test]
async fn api_settlement_beyond_holiday_coverage_logs_warning() {
    let pool = test_pool().await;
    insert_test_listing(&pool).await;
    // XASX holidays are seeded 2019–2027 only: a 2018 trade's settlement is
    // computed skipping weekends only, so the auto-population warns rather
    // than silently using the incomplete calendar. (The gap is probed
    // below the seeded span rather than above it because a trade dated
    // after today is refused outright — SCENARIOS S-10.)
    let body = serde_json::json!({
        "trade_type": "Buy",
        "date": "2018-06-04",
        "listing_id": 1,
        "average_price": "100.0",
        "quantity": "10.0",
        "currency": "AUD",
        "brokerage": "0.0",
        "gst_on_brokerage": "0.0",
        "brokerage_currency": "AUD",
        "fx_rate": "1.0"
    });
    let resp = client(&pool).put("/trades/1", &body).await;
    // Non-blocking: the write succeeds, the warning surfaces the gap.
    assert_eq!(resp.status, StatusCode::CREATED);
    assert!(logs_contain(
        "settlement window outside seeded exchange-holiday coverage"
    ));
}

#[tracing_test::traced_test]
#[tokio::test]
async fn api_settlement_inside_holiday_coverage_does_not_warn() {
    let pool = test_pool().await;
    insert_test_listing(&pool).await;
    let body = serde_json::json!({
        "trade_type": "Buy",
        "date": "2024-01-15",
        "listing_id": 1,
        "average_price": "100.0",
        "quantity": "10.0",
        "currency": "AUD",
        "brokerage": "0.0",
        "gst_on_brokerage": "0.0",
        "brokerage_currency": "AUD",
        "fx_rate": "1.0"
    });
    let resp = client(&pool).put("/trades/1", &body).await;
    assert_eq!(resp.status, StatusCode::CREATED);
    assert!(!logs_contain(
        "settlement window outside seeded exchange-holiday coverage"
    ));
}

/// A parcel is created by a Buy or a DRP reinvestment and consumed by a
/// Sell — the classification every "needs something to draw on" write-time
/// guard tests (Sell and rights-sale allocations, AMIT adjustments).
#[test]
fn only_buy_and_drp_are_acquisitions() {
    assert!(TradeType::Buy.is_acquisition());
    assert!(TradeType::DRP.is_acquisition());
    assert!(!TradeType::Sell.is_acquisition());
}

#[test]
fn add_business_days_skips_weekend() {
    let none = HashSet::new();
    // 2024-01-18 is a Thursday; T+2 business days settles Monday 2024-01-22,
    // skipping Sat 2024-01-20 and Sun 2024-01-21.
    let thursday = NaiveDate::from_ymd_opt(2024, 1, 18).unwrap();
    assert_eq!(
        add_business_days(thursday, 2, &none).unwrap(),
        NaiveDate::from_ymd_opt(2024, 1, 22).unwrap()
    );
    // 2024-01-15 is a Monday; T+2 stays within the week (Wednesday).
    let monday = NaiveDate::from_ymd_opt(2024, 1, 15).unwrap();
    assert_eq!(
        add_business_days(monday, 2, &none).unwrap(),
        NaiveDate::from_ymd_opt(2024, 1, 17).unwrap()
    );
}

#[test]
fn add_business_days_skips_public_holidays() {
    // Christmas Day (Wed) and Boxing Day (Thu) 2024 are public holidays.
    let holidays: HashSet<NaiveDate> = [
        NaiveDate::from_ymd_opt(2024, 12, 25).unwrap(),
        NaiveDate::from_ymd_opt(2024, 12, 26).unwrap(),
    ]
    .into_iter()
    .collect();
    // Tuesday 2024-12-24 + T+2: skip Wed 25 + Thu 26 (holidays), Fri 27 = 1,
    // skip the weekend, Mon 30 = 2 → settles 2024-12-30.
    let tuesday = NaiveDate::from_ymd_opt(2024, 12, 24).unwrap();
    assert_eq!(
        add_business_days(tuesday, 2, &holidays).unwrap(),
        NaiveDate::from_ymd_opt(2024, 12, 30).unwrap()
    );
    // Without the holiday set it would settle on Boxing Day (Thu 26).
    assert_eq!(
        add_business_days(tuesday, 2, &HashSet::new()).unwrap(),
        NaiveDate::from_ymd_opt(2024, 12, 26).unwrap()
    );
}

/// The old unchecked `result += Duration::days(1)` panicked with
/// `'NaiveDate + TimeDelta' overflowed` once the step left `NaiveDate`'s
/// range (the 2026-09-17 review's 10-second empty `500`). The step is
/// checked now, so the same call answers an error. `NaiveDate::MAX` is
/// 262143-12-31, so the very first business-day step is already off the end.
#[test]
fn add_business_days_at_the_end_of_the_date_range_is_an_error_not_a_panic() {
    let err = add_business_days(NaiveDate::MAX, 1, &HashSet::new()).unwrap_err();
    assert!(matches!(err, SettlementError::DateOverflow { .. }));
    // The last representable day is a valid *input*: a T+0 window is that
    // day itself, not an overflow.
    assert_eq!(
        add_business_days(NaiveDate::MAX, 0, &HashSet::new()).unwrap(),
        NaiveDate::MAX
    );
}

#[tokio::test]
async fn api_settlement_date_skips_public_holiday() {
    let pool = test_pool().await;
    insert_test_listing(&pool).await; // listing 1 trades on XASX
    // XASX is closed Christmas (Wed 2024-12-25) and Boxing Day (Thu 2024-12-26);
    // a Tuesday 2024-12-24 buy at T+2 settles Mon 2024-12-30, not Thu 2024-12-26.
    let body = serde_json::json!({
        "trade_type": "Buy",
        "date": "2024-12-24",
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
        NaiveDate::from_ymd_opt(2024, 12, 30).unwrap()
    );
}

#[tokio::test]
async fn api_settlement_date_auto_populated_skips_weekend() {
    let pool = test_pool().await;
    insert_test_listing(&pool).await;
    // Friday 2024-01-19 + T+2 business days = Tuesday 2024-01-23 (skips the weekend).
    let body = serde_json::json!({
        "trade_type": "Buy",
        "date": "2024-01-19",
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
        NaiveDate::from_ymd_opt(2024, 1, 23).unwrap()
    );
}

/// SCENARIOS S-05: the guarantee the *auto* path gives, asserted rather
/// than assumed. A supplied `settlement_date` is a deliberate override and
/// is never refused — the settlement-holiday-coverage report flags one
/// that lands on a closed day — but a settlement this code computes must
/// land on a trading day, because `add_business_days` skips weekends and
/// the exchange's seeded holidays by construction. Walked over every
/// trading day of both seeded calendars (XASX and XNYS, 2019–2027), each
/// settled through the real write path's helper and each result put back
/// to the very calendar the trading-day refusal reads.
///
/// Windows running past the end of coverage are skipped, not asserted:
/// there the calendar is *incomplete*, which is the one way the auto path
/// can still produce a closed day (SCENARIOS S-04) and is exactly what the
/// coverage report exists to say.
#[tokio::test]
async fn auto_settlement_never_lands_on_a_non_trading_day_under_a_complete_calendar() {
    use crate::entities::closing_price;
    let pool = test_pool().await;
    test_support::listing(1).mic("XASX").insert(&pool).await;
    test_support::listing(2)
        .mic("XNYS")
        .ticker("LAC")
        .currency("USD")
        .insert(&pool)
        .await;
    // The seeded calendars run 2019–2027 on both exchanges.
    let coverage_end = ymd(2027, 12, 31);
    let mut checked = 0;
    for listing_id in [1, 2] {
        let market = closing_price::load_market(&pool, ListingId(listing_id))
            .await
            .unwrap()
            .unwrap();
        let mut date = ymd(2019, 1, 1);
        while date <= coverage_end {
            if closing_price::non_trading_day(&market, date).is_none() {
                let settled = auto_settlement_date(&pool, TradeId(1), ListingId(listing_id), date)
                    .await
                    .unwrap();
                if settled <= coverage_end {
                    assert!(
                        closing_price::non_trading_day(&market, settled).is_none(),
                        "listing {listing_id}: {date} settled on {settled}, not a trading day"
                    );
                    checked += 1;
                }
            }
            date += chrono::Duration::days(1);
        }
    }
    // Not a vacuous pass: two exchanges' worth of trading days.
    assert!(checked > 4000, "only {checked} settlements were checked");
}

async fn seed_holiday(app: &ApiClient, date: NaiveDate, name: &str) {
    app.put_ok(
        format!("/exchange_holidays/XASX/{date}"),
        &serde_json::json!({ "name": name }),
    )
    .await;
}

/// SCENARIOS S-04's four-step reproduction, end to end through the API.
///
/// Transposed to the 2018 Easter, because the reproduction as driven
/// (2028) can no longer be entered: S-10 refuses a trade dated after
/// today. The shape is identical — a Thursday before a Good Friday in a
/// year with no seeded `exchange_holidays` rows — and 2018 sits before the
/// seeded 2019–2027 span, so the calendar is missing at the same end of it.
///
/// 1. The Buy auto-computes T+2 skipping weekends only and lands on the
///    Easter Monday nobody has entered; the coverage report says so.
/// 2. The user seeds the year the report asked for.
/// 3. The window is now inside coverage — the report's first question goes
///    quiet (here its second one still catches the row, because this
///    settlement happens to land on the seeded holiday itself; the
///    following test is the case where it does not).
/// 4. The stored settlement date is still the Easter Monday until the
///    `settlement-recompute` job re-derives it, and then the report is
///    empty because the date is right rather than because it is hidden.
#[tokio::test]
async fn seeding_a_missing_calendar_and_recomputing_corrects_the_settlement_it_left_wrong() {
    let pool = test_pool().await;
    test_support::listing(1).mic("XASX").insert(&pool).await;
    let app = ApiClient::full(&pool);

    // 1. Thursday 2018-03-29, T+2 over an empty 2018 calendar.
    app.put_ok("/trades/1", &auto_settled_buy(ymd(2018, 3, 29)))
        .await;
    let trade: Trade = app.get_json("/trades/1").await;
    assert_eq!(trade.settlement_date, ymd(2018, 4, 2)); // Easter Monday
    assert_eq!(trade.settlement_date_source, SettlementDateSource::Computed);
    let alerts: Vec<serde_json::Value> = app.get_json("/reports/settlement_holiday_coverage").await;
    assert_eq!(alerts.len(), 1);
    assert_eq!(alerts[0]["coverage_status"], "outside_holiday_coverage");

    // 2. The user seeds the 2018 calendar the report asked for.
    seed_holiday(&app, ymd(2018, 3, 30), "Good Friday").await;
    seed_holiday(&app, ymd(2018, 4, 2), "Easter Monday").await;

    // 3. The window is inside coverage now; only the trading-day question
    //    still holds the row — and 4. the stored date has not moved.
    let alerts: Vec<serde_json::Value> = app.get_json("/reports/settlement_holiday_coverage").await;
    assert_eq!(alerts.len(), 1);
    assert_eq!(alerts[0]["coverage_status"], "inside_holiday_coverage");
    assert_eq!(alerts[0]["settlement_non_trading_reason"], "holiday");
    let trade: Trade = app.get_json("/trades/1").await;
    assert_eq!(trade.settlement_date, ymd(2018, 4, 2));

    // The job is what repairs it: T+2 over the completed calendar skips
    // Good Friday, the weekend and Easter Monday.
    app.post_empty("/jobs/settlement-recompute")
        .await
        .expect_status(StatusCode::NO_CONTENT);
    let trade: Trade = app.get_json("/trades/1").await;
    assert_eq!(trade.settlement_date, ymd(2018, 4, 4));
    assert_eq!(
        trade.settlement_date_source,
        SettlementDateSource::Computed,
        "a recomputed date is still a computed one"
    );
    let alerts: Vec<serde_json::Value> = app.get_json("/reports/settlement_holiday_coverage").await;
    assert!(alerts.is_empty(), "still flagged: {alerts:?}");

    // trades is audited, and the job writes through the same triggers as
    // any other update: the superseded date is recoverable.
    let history: Vec<serde_json::Value> = app
        .get_json("/reports/row_history?table=trades&row_id=1")
        .await;
    assert_eq!(history.len(), 1);
    assert_eq!(history[0]["operation"], "UPDATE");
    assert_eq!(history[0]["settlement_date"], "2018-04-02");
}

/// SCENARIOS S-04's general case — the one the S-05 trading-day check
/// cannot see. Only the holiday *inside* the window is missing, so the
/// settlement lands one business day early on a perfectly good trading
/// day: seeding the calendar silences the coverage report completely, and
/// nothing but the recompute job would ever correct the stored date.
#[tokio::test]
async fn recompute_corrects_a_settlement_left_a_day_early_by_a_missing_holiday() {
    let pool = test_pool().await;
    test_support::listing(1).mic("XASX").insert(&pool).await;
    let app = ApiClient::full(&pool);

    app.put_ok("/trades/1", &auto_settled_buy(ymd(2018, 3, 29)))
        .await;
    // Good Friday alone: the settlement stays on Monday 2018-04-02, which
    // is an ordinary trading day while Easter Monday is unseeded.
    seed_holiday(&app, ymd(2018, 3, 30), "Good Friday").await;
    let alerts: Vec<serde_json::Value> = app.get_json("/reports/settlement_holiday_coverage").await;
    assert!(
        alerts.is_empty(),
        "the report has gone quiet, which is the finding: {alerts:?}"
    );
    let trade: Trade = app.get_json("/trades/1").await;
    assert_eq!(trade.settlement_date, ymd(2018, 4, 2));

    app.post_empty("/jobs/settlement-recompute")
        .await
        .expect_status(StatusCode::NO_CONTENT);
    let trade: Trade = app.get_json("/trades/1").await;
    assert_eq!(trade.settlement_date, ymd(2018, 4, 3));
}

/// SCENARIOS S-04: a stored settlement that already matches what the
/// current calendar computes is left exactly as it is — no write, so no
/// audit entry and no snapshot staled. That is also what makes the job
/// idempotent: the second run of the test above would do this.
#[tokio::test]
async fn recompute_leaves_a_settlement_that_already_matches_the_calendar_untouched() {
    let pool = test_pool().await;
    test_support::listing(1).mic("XASX").insert(&pool).await;
    let app = ApiClient::full(&pool);

    // 2024 is inside the seeded calendar: Monday 2024-01-15 settles T+2 on
    // the Wednesday, with nothing missing to correct.
    app.put_ok("/trades/1", &auto_settled_buy(ymd(2024, 1, 15)))
        .await;
    let before: Trade = app.get_json("/trades/1").await;
    assert_eq!(before.settlement_date, ymd(2024, 1, 17));

    app.post_empty("/jobs/settlement-recompute")
        .await
        .expect_status(StatusCode::NO_CONTENT);
    let after: Trade = app.get_json("/trades/1").await;
    assert_eq!(after.settlement_date, before.settlement_date);
    let history: Vec<serde_json::Value> = app
        .get_json("/reports/row_history?table=trades&row_id=1")
        .await;
    assert!(history.is_empty(), "nothing should have been written");
}

/// SCENARIOS S-04/S-05: a **hand-supplied** settlement date is the
/// taxpayer's own assertion and the job never touches it — reproducing the
/// live database's trade 9071 (LAC on XNYS, dated 2021-03-25 with an
/// explicit `settlement_date` of 2021-05-29, a Saturday two months later).
/// It stays exactly as entered, still flagged by the coverage report, and
/// re-saving it through the API (which sends the date back verbatim) does
/// not turn it into a computed one either.
#[tokio::test]
async fn recompute_leaves_a_hand_supplied_settlement_untouched() {
    let pool = test_pool().await;
    test_support::listing(1)
        .mic("XNYS")
        .ticker("LAC")
        .currency("USD")
        .insert(&pool)
        .await;
    let app = ApiClient::full(&pool);
    let mut body = auto_settled_buy(ymd(2021, 3, 25));
    body["currency"] = "USD".into();
    body["brokerage_currency"] = "USD".into();
    body["settlement_date"] = "2021-05-29".into();
    app.put_ok("/trades/9071", &body).await;
    let stored: Trade = app.get_json("/trades/9071").await;
    assert_eq!(stored.settlement_date, ymd(2021, 5, 29));
    assert_eq!(stored.settlement_date_source, SettlementDateSource::Stated);

    app.post_empty("/jobs/settlement-recompute")
        .await
        .expect_status(StatusCode::NO_CONTENT);
    let after: Trade = app.get_json("/trades/9071").await;
    assert_eq!(after.settlement_date, ymd(2021, 5, 29));
    assert_eq!(after.settlement_date_source, SettlementDateSource::Stated);
    let alerts: Vec<serde_json::Value> = app.get_json("/reports/settlement_holiday_coverage").await;
    assert_eq!(alerts.len(), 1);
    assert_eq!(alerts[0]["trade_id"], 9071);
    assert_eq!(alerts[0]["settlement_non_trading_reason"], "weekend");
    let history: Vec<serde_json::Value> = app
        .get_json("/reports/row_history?table=trades&row_id=9071")
        .await;
    assert!(
        history.is_empty(),
        "the assertion was rewritten: {history:?}"
    );
}

/// SCENARIOS S-04: a row written before the provenance column existed
/// (migration 0041's default, which every row in the live database takes)
/// records nothing about how its settlement date was arrived at, so the
/// job leaves it alone rather than guessing — it might be an assertion
/// like trade 9071's. A re-save through the API keeps it that way while
/// the date is unchanged; entering a different one is what makes it a
/// statement.
#[tokio::test]
async fn recompute_leaves_a_row_from_before_the_provenance_column_untouched() {
    let pool = test_pool().await;
    test_support::listing(1).mic("XASX").insert(&pool).await;
    test_support::buy(1, 1)
        .date(ymd(2024, 1, 15))
        .settlement(ymd(2024, 1, 18)) // a day later than T+2 computes
        .settlement_source(SettlementDateSource::Unrecorded)
        .insert(&pool)
        .await;
    let app = ApiClient::full(&pool);

    app.post_empty("/jobs/settlement-recompute")
        .await
        .expect_status(StatusCode::NO_CONTENT);
    let after: Trade = app.get_json("/trades/1").await;
    assert_eq!(after.settlement_date, ymd(2024, 1, 18));
    assert_eq!(
        after.settlement_date_source,
        SettlementDateSource::Unrecorded
    );

    // Re-saving the row verbatim is not an assertion about the date.
    let mut body = auto_settled_buy(ymd(2024, 1, 15));
    body["settlement_date"] = "2024-01-18".into();
    app.put_ok("/trades/1", &body).await;
    let after: Trade = app.get_json("/trades/1").await;
    assert_eq!(
        after.settlement_date_source,
        SettlementDateSource::Unrecorded
    );

    // Entering a different date is.
    body["settlement_date"] = "2024-01-19".into();
    app.put_ok("/trades/1", &body).await;
    let after: Trade = app.get_json("/trades/1").await;
    assert_eq!(after.settlement_date_source, SettlementDateSource::Stated);
}

/// `Settlement::resolve_on` classifies against the row **the caller's own
/// connection** sees, which is what lets the write paths resolve on their
/// write transaction (2026-09-17 review).
///
/// The probe is an uncommitted UPDATE on the transaction the resolution
/// runs on: the supplied date still matches the pool's committed row
/// (2024-01-17), but the transaction's own view has moved it to
/// 2024-01-18. Read on the pool — what the old `Settlement::resolve` did
/// before `write_tx` — the dates match and the stale `computed` source is
/// kept; read here, the supplied date differs from the stored one and is
/// the taxpayer's statement, which is the answer the row it lands on
/// warrants.
#[tokio::test]
async fn settlement_resolution_classifies_the_row_on_its_own_connection() {
    let pool = test_pool().await;
    insert_test_listing(&pool).await;
    test_support::buy(1, 1)
        .date(ymd(2024, 1, 15))
        .settlement(ymd(2024, 1, 17))
        .settlement_source(SettlementDateSource::Computed)
        .insert(&pool)
        .await;

    let mut tx = infra::db::write_tx(&pool).await.unwrap();
    sqlx::query("UPDATE trades SET settlement_date = '2024-01-18' WHERE id = 1")
        .execute(&mut *tx)
        .await
        .unwrap();

    let resolved = Settlement::resolve_on(
        &mut tx,
        Some(TradeId(1)),
        ListingId(1),
        ymd(2024, 1, 15),
        Some(ymd(2024, 1, 17)),
    )
    .await
    .unwrap();
    assert_eq!(resolved.date, ymd(2024, 1, 17));
    assert_eq!(
        resolved.source,
        SettlementDateSource::Stated,
        "the transaction's stored date has moved, so the re-supplied one is a statement"
    );

    // The probe was the transaction's own view, not the pool's.
    tx.rollback().await.unwrap();
    let stored = db_get(&pool, TradeId(1)).await.unwrap().unwrap();
    assert_eq!(stored.settlement_date, ymd(2024, 1, 17));
    assert_eq!(
        stored.settlement_date_source,
        SettlementDateSource::Computed
    );
}

/// A `PUT` of a stated settlement date races a recompute that has already
/// moved the stored date: the row the PUT lands on is not the row it
/// classified from, so it must not stamp the date it writes as `computed`.
///
/// The interleave is made deterministic rather than sampled. A recompute's
/// write is held open on another connection — `BEGIN IMMEDIATE`, so the
/// moved date is invisible to any pool reader and the PUT's own
/// `write_tx` cannot begin until it commits. Resolved on the pool before
/// `write_tx` (what the handler did until the 2026-09-17 review), the PUT
/// read the pre-recompute row, where its supplied 2024-01-17 equalled the
/// stored one, and so kept `computed`; that stamp is what lets the
/// `settlement-recompute` job silently re-derive a date the user asserted.
/// Resolved on the write's own transaction, the stored row already holds
/// the recompute's 2024-01-18, the supplied date differs from it, and the
/// date is recorded `stated`.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_recompute_interleaved_with_a_stated_date_write_cannot_mis_stamp_the_source() {
    let dir = tempfile::tempdir().unwrap();
    let (pool, busy) = test_support::race_pool_with_busy_probe(&dir).await;
    test_support::listing(1).mic("XASX").insert(&pool).await;
    // The row the PUT's GET was taken from: a computed settlement date.
    test_support::buy(1, 1)
        .date(ymd(2024, 1, 15))
        .settlement(ymd(2024, 1, 17))
        .settlement_source(SettlementDateSource::Computed)
        .insert(&pool)
        .await;

    // The recompute, in flight: it moves the stored date one day. Held
    // uncommitted, so a pool reader still sees the old date — the state the
    // old handler decided on.
    let mut recompute = infra::db::write_tx(&pool).await.unwrap();
    sqlx::query("UPDATE trades SET settlement_date = '2024-01-18' WHERE id = 1")
        .execute(&mut *recompute)
        .await
        .unwrap();

    // The PUT replaying the GET's stated date.
    let body = serde_json::json!({
        "trade_type": "Buy",
        "date": "2024-01-15",
        "listing_id": 1,
        "settlement_date": "2024-01-17",
        "average_price": "100.0",
        "quantity": "10.0",
        "currency": "AUD",
        "brokerage": "0",
        "gst_on_brokerage": "0",
        "brokerage_currency": "AUD",
        "fx_rate": "1.0"
    });
    let app = client(&pool);
    let putting = {
        let app = app.clone();
        tokio::spawn(async move { app.put("/trades/1", &body).await })
    };
    // An event, not a guessed duration: the PUT is blocked on the write
    // lock — which, under the old shape, it reached only after its pooled
    // resolve, so the stale read has certainly happened by now.
    busy.blocked().await;
    assert!(
        !putting.is_finished(),
        "the PUT waits for the write lock rather than deciding on a stale read"
    );

    recompute.commit().await.unwrap();
    let resp = putting.await.expect("the PUT task does not panic");
    assert_eq!(resp.status, StatusCode::NO_CONTENT);

    let stored = db_get(&pool, TradeId(1)).await.unwrap().unwrap();
    assert_eq!(
        stored.settlement_date,
        ymd(2024, 1, 17),
        "the PUT wrote the date it stated"
    );
    assert_eq!(
        stored.settlement_date_source,
        SettlementDateSource::Stated,
        "the row the PUT landed on held the recompute's date, so the supplied one is a \
             statement — stamping it `computed` would let the settlement-recompute job \
             silently re-derive a date the user asserted"
    );
}
