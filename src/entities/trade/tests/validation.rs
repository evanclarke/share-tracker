//! Write-time validation of a trade's figures and dates: degenerate amounts,
//! pre-CGT, future and non-trading-day dates, GST-inclusive brokerage, the
//! statement-total cross-check, and currency agreement.

use super::*;

/// Degenerate core figures are rejected with 422 per shape — a zero or
/// negative quantity, a negative price, negative brokerage or GST, a
/// non-positive `fx_rate`, or a settlement before the trade date — and
/// nothing is persisted. (2026-07-12 review: the plain CRUD path accepted
/// them all, silently corrupting every downstream report.)
#[tokio::test]
async fn api_degenerate_trade_amounts_are_rejected_per_shape() {
    let pool = test_pool().await;
    insert_test_listing(&pool).await;
    let base = serde_json::json!({
        "trade_type": "Buy",
        "date": "2024-01-15",
        "listing_id": 1,
        "average_price": "100",
        "quantity": "10",
        "currency": "AUD",
        "brokerage": "9.95",
        "gst_on_brokerage": "0.995",
        "brokerage_currency": "AUD",
        "fx_rate": "1"
    });
    for (field, value, expected) in [
        ("quantity", "0", "quantity must be positive"),
        ("quantity", "-5", "quantity must be positive"),
        ("average_price", "-1", "average_price cannot be negative"),
        ("brokerage", "-9.95", "brokerage cannot be negative"),
        (
            "gst_on_brokerage",
            "-0.995",
            "gst_on_brokerage cannot be negative",
        ),
        ("fx_rate", "0", "fx_rate must be a positive"),
        ("fx_rate", "-1.5", "fx_rate must be a positive"),
        (
            "settlement_date",
            "2024-01-14",
            "settlement_date cannot be before the trade date",
        ),
    ] {
        let mut body = base.clone();
        body[field] = value.into();
        let (status, detail) = put_trade_json(&pool, 1, body).await;
        assert_eq!(
            status,
            StatusCode::UNPROCESSABLE_ENTITY,
            "{field}={value} must be rejected"
        );
        assert!(
            detail.contains(expected),
            "{field}={value}: detail must explain the rule, got: {detail}"
        );
        assert!(
            db_get(&pool, TradeId(1)).await.unwrap().is_none(),
            "{field}={value}: nothing persisted"
        );
    }

    // Boundary values stay accepted: zero price, zero costs, a same-day
    // settlement, and a fractional quantity are all legitimate entries.
    let mut fine = base.clone();
    fine["average_price"] = "0".into();
    fine["brokerage"] = "0".into();
    fine["gst_on_brokerage"] = "0".into();
    fine["quantity"] = "0.00000001".into();
    fine["settlement_date"] = "2024-01-15".into();
    let (status, _) = put_trade_json(&pool, 1, fine).await;
    assert_eq!(status, StatusCode::CREATED);
}

/// A trade dated before the start of CGT (20 September 1985) is rejected
/// 422: a pre-CGT holding is outside CGT and not modelled, so recording
/// one would wrongly compute a capital gain or loss on it (REQUIREMENTS
/// 2026-07-13 — the former documentation-only Known limitation, enforced).
/// The first CGT day itself stays accepted.
#[tokio::test]
async fn api_pre_cgt_dated_trade_rejected_422() {
    let pool = test_pool().await;
    insert_test_listing(&pool).await;
    let base = serde_json::json!({
        "trade_type": "Buy",
        "date": "1985-09-19",
        "settlement_date": "1985-09-19",
        "listing_id": 1,
        "average_price": "100",
        "quantity": "10",
        "currency": "AUD",
        "brokerage": "0",
        "gst_on_brokerage": "0",
        "brokerage_currency": "AUD",
        "fx_rate": "1"
    });
    let (status, detail) = put_trade_json(&pool, 1, base.clone()).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert!(
        detail.contains("dated before 20 September 1985")
            && detail.contains("pre-CGT holding is outside CGT"),
        "detail must explain the pre-CGT rule, got: {detail}"
    );
    assert!(db_get(&pool, TradeId(1)).await.unwrap().is_none());

    // 20 September 1985 — the first day inside CGT — is accepted.
    let mut first_cgt_day = base;
    first_cgt_day["date"] = "1985-09-20".into();
    first_cgt_day["settlement_date"] = "1985-09-20".into();
    let (status, _) = put_trade_json(&pool, 1, first_cgt_day).await;
    assert_eq!(status, StatusCode::CREATED);
}

/// SCENARIOS S-10: a trade dated after the server's current date is
/// rejected 422 — the upper twin of the pre-CGT floor. A trade records a
/// transaction that has already happened, so a future date is a typo (a
/// 2027-for-2026 slip on a July trade), and it put a financial year that
/// has not begun on the annual tax report's year picker. Today itself is
/// the boundary and stays accepted — with its *settlement* landing in the
/// future, which is deliberately not bounded (a T+2 settlement of a trade
/// dated today has not happened yet).
///
/// The boundary is checked on an exchange-less (Crypto) listing, which
/// trades every day: on a listed security the trading-day rule (SCENARIOS
/// S-08) refuses a trade dated today whenever the suite runs on a weekend
/// or an exchange holiday, and that would make this test's answer depend
/// on the day of the week. The settlement half is asserted separately, on
/// the listed security, from the most recent day its market was open —
/// T+2 business days from any such day is always after today.
#[tokio::test]
async fn api_future_dated_trade_rejected_422_and_today_accepted() {
    let pool = test_pool().await;
    insert_test_listing(&pool).await;
    test_support::listing(2)
        .crypto()
        .ticker("ETH")
        .name("Ether")
        .insert(&pool)
        .await;
    let today = date::today();
    let base = serde_json::json!({
        "trade_type": "Buy",
        "date": (today + chrono::Days::new(1)).to_string(),
        "listing_id": 1,
        "average_price": "100",
        "quantity": "10",
        "currency": "AUD",
        "brokerage": "0",
        "gst_on_brokerage": "0",
        "brokerage_currency": "AUD",
        "fx_rate": "1"
    });
    let (status, detail) = put_trade_json(&pool, 1, base.clone()).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert!(
        detail.contains("dated after today"),
        "detail must explain the rule, got: {detail}"
    );
    assert!(db_get(&pool, TradeId(1)).await.unwrap().is_none());

    // Today — the last day inside the window — is accepted.
    let mut boundary = base.clone();
    boundary["date"] = today.to_string().into();
    boundary["listing_id"] = 2.into();
    let (status, detail) = put_trade_json(&pool, 1, boundary).await;
    assert_eq!(status, StatusCode::CREATED, "detail: {detail}");
    let stored = db_get(&pool, TradeId(1)).await.unwrap().unwrap();
    assert_eq!(stored.date, today);

    // And the settlement date is allowed past it: a T+2 settlement of a
    // trade dated the market's most recent open day always falls after
    // today, whichever day of the week the suite runs on.
    let market = closing_price::load_market(&pool, ListingId(1))
        .await
        .unwrap()
        .unwrap();
    let last_open = market
        .latest_trading_day_on_or_before(today)
        .expect("XASX has an open day in the past year");
    let mut settled = base;
    settled["date"] = last_open.to_string().into();
    let (status, detail) = put_trade_json(&pool, 2, settled).await;
    assert_eq!(status, StatusCode::CREATED, "detail: {detail}");
    let stored = db_get(&pool, TradeId(2)).await.unwrap().unwrap();
    assert!(
        stored.settlement_date > today,
        "T+2 settlement of a trade dated {last_open} is legitimately in the future, got {}",
        stored.settlement_date
    );
}

/// SCENARIOS S-08: a trade dated on a day its exchange did not trade —
/// a Saturday, or a seeded `exchange_holidays` date — is rejected 422
/// naming the day and the exchange. The trade date is the CGT event date,
/// so it sets the 12-month discount clock, the financial year the gain
/// falls in and the day the T+n count starts from; a day the market was
/// shut is a data-entry error by construction. The calendar is the one
/// `PUT /closing_prices` already refuses a non-trading day on.
#[tokio::test]
async fn api_trade_on_a_non_trading_day_rejected_422() {
    let pool = test_pool().await;
    insert_test_listing(&pool).await;
    let base = serde_json::json!({
        "trade_type": "Buy",
        "date": "2024-01-15",
        "listing_id": 1,
        "average_price": "100",
        "quantity": "10",
        "currency": "AUD",
        "brokerage": "0",
        "gst_on_brokerage": "0",
        "brokerage_currency": "AUD",
        "fx_rate": "1"
    });

    // Saturday 2024-01-13.
    let mut saturday = base.clone();
    saturday["date"] = "2024-01-13".into();
    let (status, detail) = put_trade_json(&pool, 1, saturday).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert!(
        detail.contains("did not trade") && detail.contains("Saturday"),
        "the refusal must name the day, got: {detail}"
    );
    assert!(
        detail.contains("XASX"),
        "the refusal must name the exchange, got: {detail}"
    );
    assert!(db_get(&pool, TradeId(1)).await.unwrap().is_none());

    // Australia Day 2024-01-26 — a Friday, and a seeded XASX holiday.
    let mut holiday = base.clone();
    holiday["date"] = "2024-01-26".into();
    let (status, detail) = put_trade_json(&pool, 1, holiday).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert!(
        detail.contains("public holiday") && detail.contains("XASX"),
        "the refusal must name the holiday and the exchange, got: {detail}"
    );
    assert!(db_get(&pool, TradeId(1)).await.unwrap().is_none());

    // The Monday between them is an ordinary trading day.
    let (status, _) = put_trade_json(&pool, 1, base).await;
    assert_eq!(status, StatusCode::CREATED);
}

/// SCENARIOS S-08: the same Saturday is accepted for an exchange-less
/// (Crypto) listing — a crypto asset trades every day, which is why it
/// also settles same-day (the `L-15` shape). The calendar rule must never
/// reach it.
#[tokio::test]
async fn api_crypto_trade_on_a_saturday_is_accepted() {
    let pool = test_pool().await;
    test_support::listing(1)
        .crypto()
        .ticker("ETH")
        .name("Ether")
        .insert(&pool)
        .await;
    let body = serde_json::json!({
        "trade_type": "Buy",
        "date": "2024-01-13",
        "listing_id": 1,
        "average_price": "100",
        "quantity": "10",
        "currency": "AUD",
        "brokerage": "0",
        "gst_on_brokerage": "0",
        "brokerage_currency": "AUD",
        "fx_rate": "1"
    });
    let (status, detail) = put_trade_json(&pool, 1, body).await;
    assert_eq!(status, StatusCode::CREATED, "{detail}");
    let stored = db_get(&pool, TradeId(1)).await.unwrap().unwrap();
    assert_eq!(stored.date, ymd(2024, 1, 13));
    // Same-day settlement, unchanged by this rule.
    assert_eq!(stored.settlement_date, ymd(2024, 1, 13));
}

/// SCENARIOS S-08: `exchange_holidays` is seeded for 2019–2027 only, so a
/// year outside it has no holiday rows at all. A weekday there must stay
/// recordable — an unseeded year cannot become unenterable — while its
/// weekends are still refused, since a weekend needs no calendar.
#[tokio::test]
async fn api_trade_in_a_year_with_no_seeded_calendar_is_accepted_on_a_weekday() {
    let pool = test_pool().await;
    insert_test_listing(&pool).await;
    let base = serde_json::json!({
        "trade_type": "Buy",
        // Christmas Day 2018 — a Tuesday, and a real ASX holiday, but the
        // seeded calendar does not reach 2018, so nothing here knows that.
        "date": "2018-12-25",
        "listing_id": 1,
        "average_price": "100",
        "quantity": "10",
        "currency": "AUD",
        "brokerage": "0",
        "gst_on_brokerage": "0",
        "brokerage_currency": "AUD",
        "fx_rate": "1"
    });
    let (status, detail) = put_trade_json(&pool, 1, base.clone()).await;
    assert_eq!(status, StatusCode::CREATED, "{detail}");

    // The weekend either side of it still is: Saturday 2018-12-22.
    let mut saturday = base;
    saturday["date"] = "2018-12-22".into();
    let (status, detail) = put_trade_json(&pool, 2, saturday).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert!(detail.contains("Saturday"), "got: {detail}");
}

#[test]
fn split_gst_inclusive_rounds_to_the_cent_and_sums_back_exactly() {
    // $9.95 incl.: 9.95/11 = 0.9045… → $0.90 GST, $9.05 ex-GST.
    assert_eq!(split_gst_inclusive(dec("9.95")), (dec("9.05"), dec("0.90")));
    // $10 incl.: 10/11 = 0.9090… → $0.91 GST (rounded up to the cent).
    assert_eq!(split_gst_inclusive(dec("10")), (dec("9.09"), dec("0.91")));
    // An exact half-cent rounds away from zero: 0.055/11 = 0.005 → $0.01.
    assert_eq!(
        split_gst_inclusive(dec("0.055")),
        (dec("0.045"), dec("0.01"))
    );
    // The pair always sums back to the amount paid.
    for amount in ["9.95", "10", "0.055", "19.99"] {
        let (brok, gst) = split_gst_inclusive(dec(amount));
        assert_eq!(brok + gst, dec(amount));
    }
}

/// A GST-inclusive entry is split by the server (any supplied GST value is
/// ignored), the flag round-trips, and an edit re-splits the new amount.
/// An unflagged entry keeps today's behaviour: both values stored as sent.
#[tokio::test]
async fn api_gst_inclusive_brokerage_is_split_and_round_trips() {
    let pool = test_pool().await;
    insert_test_listing(&pool).await;
    let body = serde_json::json!({
        "trade_type": "Buy",
        "date": "2024-01-15",
        "listing_id": 1,
        "average_price": "100",
        "quantity": "10",
        "currency": "AUD",
        "brokerage": "9.95",
        "gst_on_brokerage": "123",   // ignored: the server derives the split
        "brokerage_includes_gst": true,
        "brokerage_currency": "AUD",
        "fx_rate": "1"
    });
    let (status, _) = put_trade_json(&pool, 1, body).await;
    assert_eq!(status, StatusCode::CREATED);
    let t = db_get(&pool, TradeId(1)).await.unwrap().unwrap();
    assert_eq!(t.brokerage, dec("9.05"));
    assert_eq!(t.gst_on_brokerage, dec("0.90"));
    assert!(
        t.brokerage_includes_gst,
        "flag must round-trip for the entry form"
    );

    // Editing with a new inclusive amount re-splits it.
    let body = serde_json::json!({
        "trade_type": "Buy",
        "date": "2024-01-15",
        "listing_id": 1,
        "average_price": "100",
        "quantity": "10",
        "currency": "AUD",
        "brokerage": "11",
        "brokerage_includes_gst": true,
        "brokerage_currency": "AUD",
        "fx_rate": "1"
    });
    let (status, _) = put_trade_json(&pool, 1, body).await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let t = db_get(&pool, TradeId(1)).await.unwrap().unwrap();
    assert_eq!(t.brokerage, dec("10"));
    assert_eq!(t.gst_on_brokerage, dec("1"));

    // Unflagged: stored exactly as entered (ex-GST + manual GST).
    let body = serde_json::json!({
        "trade_type": "Buy",
        "date": "2024-01-15",
        "listing_id": 1,
        "average_price": "100",
        "quantity": "10",
        "currency": "AUD",
        "brokerage": "9.95",
        "gst_on_brokerage": "0.995",
        "brokerage_currency": "AUD",
        "fx_rate": "1"
    });
    let (status, _) = put_trade_json(&pool, 2, body).await;
    assert_eq!(status, StatusCode::CREATED);
    let t = db_get(&pool, TradeId(2)).await.unwrap().unwrap();
    assert_eq!(t.brokerage, dec("9.95"));
    assert_eq!(t.gst_on_brokerage, dec("0.995"));
    assert!(!t.brokerage_includes_gst);
    assert_eq!(t.statement_total, None);
}

/// GET the JSON body of /trades/{id} as raw bytes (the exact wire shape a
/// round-trip client would re-PUT) plus its parsed `Trade`.
async fn get_trade_raw(pool: &SqlitePool, id: i64) -> (Vec<u8>, Trade) {
    let resp = client(pool).get(format!("/trades/{id}")).await;
    assert_eq!(resp.status, StatusCode::OK);
    let trade = resp.json();
    (resp.body.to_vec(), trade)
}

/// Lossless GST-inclusive round-trip (REQUIREMENTS 2026-07-13): with the
/// flag set, `brokerage` on the wire is the same GST-inclusive amount on
/// reads and writes — GET re-presents the stored split recombined
/// (0.90 + 0.09 reads back as the 0.99 entered), and PUTting the response
/// body back verbatim re-splits it to the identical stored pair instead
/// of shrinking the brokerage by the GST each pass.
#[tokio::test]
async fn api_gst_inclusive_get_put_round_trip_is_lossless() {
    let pool = test_pool().await;
    insert_test_listing(&pool).await;
    let body = serde_json::json!({
        "trade_type": "Buy",
        "date": "2024-01-15",
        "listing_id": 1,
        "average_price": "100",
        "quantity": "10",
        "currency": "AUD",
        "brokerage": "0.99",
        "brokerage_includes_gst": true,
        "brokerage_currency": "AUD",
        "fx_rate": "1",
        // Round-trips too, and re-validates against the re-split figures.
        "statement_total": "1000.99"
    });
    let (status, _) = put_trade_json(&pool, 1, body).await;
    assert_eq!(status, StatusCode::CREATED);

    // Two full GET → PUT-verbatim passes: the stored split never moves.
    for pass in 1..=2 {
        let (raw, seen) = get_trade_raw(&pool, 1).await;
        assert_eq!(
            seen.brokerage,
            dec("0.99"),
            "read is inclusive (pass {pass})"
        );
        assert_eq!(
            seen.gst_on_brokerage,
            dec("0.09"),
            "derived GST (pass {pass})"
        );
        assert!(seen.brokerage_includes_gst);

        // Minus the columns a read carries that no write body owns —
        // the id and the trade's provenance/derived columns, which the
        // body denies rather than ignores (SCENARIOS V-a).
        let read: serde_json::Value = serde_json::from_slice(&raw).unwrap();
        let body = test_support::writable_body(&read, &[]);
        let (status, detail) = put_trade_json(&pool, 1, body).await;
        assert_eq!(status, StatusCode::NO_CONTENT, "pass {pass}: {detail}");
        let t = db_get(&pool, TradeId(1)).await.unwrap().unwrap();
        assert_eq!(t.brokerage, dec("0.90"), "stored ex-GST (pass {pass})");
        assert_eq!(t.gst_on_brokerage, dec("0.09"), "stored GST (pass {pass})");
        assert_eq!(t.statement_total, Some(dec("1000.99")));
    }

    // An unflagged trade reads back exactly as stored — no recombination.
    let body = serde_json::json!({
        "trade_type": "Buy",
        "date": "2024-01-15",
        "listing_id": 1,
        "average_price": "100",
        "quantity": "10",
        "currency": "AUD",
        "brokerage": "9.95",
        "gst_on_brokerage": "0.995",
        "brokerage_currency": "AUD",
        "fx_rate": "1"
    });
    let (status, _) = put_trade_json(&pool, 2, body).await;
    assert_eq!(status, StatusCode::CREATED);
    let (_, seen) = get_trade_raw(&pool, 2).await;
    assert_eq!(seen.brokerage, dec("9.95"));
    assert_eq!(seen.gst_on_brokerage, dec("0.995"));
}

/// The statement total must reconcile with quantity × price + brokerage +
/// GST for a Buy: a matching figure (in any trailing-zero spelling) is
/// accepted and stored; a mismatch is rejected with the computed figure in
/// the 422 detail and nothing persisted.
#[tokio::test]
async fn api_statement_total_cross_check_on_buy() {
    let pool = test_pool().await;
    insert_test_listing(&pool).await;
    // 10 × 100 + 9.05 + 0.90 (from the 9.95 inclusive split) = 1009.95
    let body = serde_json::json!({
        "trade_type": "Buy",
        "date": "2024-01-15",
        "listing_id": 1,
        "average_price": "100",
        "quantity": "10",
        "currency": "AUD",
        "brokerage": "9.95",
        "brokerage_includes_gst": true,
        "brokerage_currency": "AUD",
        "fx_rate": "1",
        "statement_total": "1009.95"
    });
    let (status, _) = put_trade_json(&pool, 1, body.clone()).await;
    assert_eq!(status, StatusCode::CREATED);
    assert_eq!(
        db_get(&pool, TradeId(1))
            .await
            .unwrap()
            .unwrap()
            .statement_total,
        Some(dec("1009.95"))
    );

    // Numeric comparison: trailing zeros don't matter.
    let mut zeros = body.clone();
    zeros["statement_total"] = "1009.9500".into();
    let (status, _) = put_trade_json(&pool, 1, zeros).await;
    assert_eq!(status, StatusCode::NO_CONTENT);

    // A mismatch is rejected, says what the trade computes to, and
    // persists nothing.
    let mut wrong = body.clone();
    wrong["statement_total"] = "1010".into();
    let (status, detail) = put_trade_json(&pool, 2, wrong).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert!(
        detail.contains("1009.95"),
        "detail must carry the computed figure: {detail}"
    );
    assert!(
        db_get(&pool, TradeId(2)).await.unwrap().is_none(),
        "nothing persisted"
    );
}

/// Contract notes print the consideration rounded to the cent, so a
/// total equal to the computed figure cent-rounded (half away from
/// zero) passes too. The figures are the three archive contract notes
/// the exact comparison rejected (live trades 19, 16, 21):
/// 1,302 × 37.585914 + 8.64 + 0.86 = 48,946.360028 → note 48,946.36;
/// 562 × 73.259875 + 8.64 + 0.86 = 41,181.54975 → note 41,181.55;
/// 0.02413796 × 3,983.77 + 3.84 = 100.000080… → note 100.00.
/// A mismatch at the cent itself still rejects with the computed
/// (unrounded) figure in the detail, and an exact .5-mil residue
/// rounds away from zero, not to even.
#[tokio::test]
async fn api_statement_total_accepts_cent_rounded_contract_note_totals() {
    let pool = test_pool().await;
    insert_test_listing(&pool).await;
    let buy = |qty: &str, price: &str, brokerage: &str, gst: &str, total: &str| {
        serde_json::json!({
            "trade_type": "Buy",
            "date": "2024-03-01",
            "listing_id": 1,
            "average_price": price,
            "quantity": qty,
            "currency": "AUD",
            "brokerage": brokerage,
            "gst_on_brokerage": gst,
            "brokerage_currency": "AUD",
            "fx_rate": "1",
            "statement_total": total
        })
    };

    // Trade 19: HNDQ 1 Mar 2024 contract note 1404967.
    let hndq = buy("1302", "37.585914", "8.64", "0.86", "48946.36");
    let (status, _) = put_trade_json(&pool, 1, hndq).await;
    assert_eq!(status, StatusCode::CREATED);
    // Trade 16: VDHG 8 Apr 2026 contract note 4518597 (…54975 → .55).
    let vdhg = buy("562", "73.259875", "8.64", "0.86", "41181.55");
    let (status, _) = put_trade_json(&pool, 2, vdhg).await;
    assert_eq!(status, StatusCode::CREATED);
    // Trade 21: ETH 22 Sep 2021 card purchase (…0080… → 100.00).
    let eth = buy("0.02413796", "3983.77", "3.84", "0", "100.00");
    let (status, _) = put_trade_json(&pool, 3, eth).await;
    assert_eq!(status, StatusCode::CREATED);

    // An exact midpoint rounds half away from zero (100.005 → 100.01),
    // not banker's-to-even (100.00).
    let mid = buy("1", "100.005", "0", "0", "100.01");
    let (status, _) = put_trade_json(&pool, 4, mid).await;
    assert_eq!(status, StatusCode::CREATED);
    let even = buy("1", "100.005", "0", "0", "100.00");
    let (status, _) = put_trade_json(&pool, 5, even).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);

    // Off by a whole cent still rejects, computed figure in the body.
    let wrong = buy("1302", "37.585914", "8.64", "0.86", "48946.37");
    let (status, detail) = put_trade_json(&pool, 6, wrong).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert!(
        detail.contains("48946.360028"),
        "detail must carry the computed figure: {detail}"
    );
    assert!(
        db_get(&pool, TradeId(6)).await.unwrap().is_none(),
        "nothing persisted"
    );
}

/// A brokerage billed in a currency other than the trade's is rejected at
/// write time (SCENARIOS B-02): the cost base, a Sell's net proceeds and
/// the activity ledger's transaction total are all single-currency sums,
/// so an AUD fee on a USD trade would be added at the USD scale and
/// silently mis-cost the parcel. Rejected with or without a statement
/// total — the total's own cross-check used to be the only thing reading
/// `brokerage_currency`, and now can never see a mixed-currency trade.
#[tokio::test]
async fn api_brokerage_in_another_currency_than_the_trade_returns_422() {
    let pool = test_pool().await;
    insert_usd_listing(&pool).await;
    let trade = |total: serde_json::Value| {
        serde_json::json!({
            "trade_type": "Buy",
            "date": "2024-01-16",
            "listing_id": 1,
            "average_price": "100",
            "quantity": "10",
            "currency": "USD",
            "brokerage": "30",
            "gst_on_brokerage": "3",
            "brokerage_currency": "AUD",
            "fx_rate": "1.5",
            "statement_total": total
        })
    };
    for (id, total) in [(1, serde_json::Value::Null), (2, serde_json::json!("1033"))] {
        let (status, detail) = put_trade_json(&pool, id, trade(total)).await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
        assert!(
            detail.contains("brokerage_currency must equal the trade's currency"),
            "detail must explain the rejection: {detail}"
        );
        assert!(
            db_get(&pool, TradeId(id)).await.unwrap().is_none(),
            "nothing persisted"
        );
    }
    // The same fee converted into the trade's own currency is accepted —
    // the documented way to record it (docs/API.md Known limitations).
    let (status, _) = put_trade_json(
        &pool,
        3,
        serde_json::json!({
            "trade_type": "Buy",
            "date": "2024-01-16",
            "listing_id": 1,
            "average_price": "100",
            "quantity": "10",
            "currency": "USD",
            "brokerage": "15",
            "gst_on_brokerage": "1.5",
            "brokerage_currency": "USD",
            "fx_rate": "1.5"
        }),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
}

/// The parcel side of the return-of-capital currency invariant (SCENARIOS
/// E-07/E-39): a payment reduces each parcel's cost base in the *parcel's*
/// own currency, so a Buy recorded in another currency than a payment that
/// reaches it is refused here too — otherwise the hole the
/// corporate-action write path closes simply reopens from the parcel side,
/// and every cost-base report of the listing dies at read time.
///
/// The reachable ordering is the payment first: with no parcels yet there
/// is nothing for the corporate-action side to disagree with, so a payment
/// in a currency other than the listing's is accepted — and every parcel
/// entered afterwards is in the listing's currency
/// (`UpsertError::CurrencyNotListings`), so this check is what catches the
/// pair.
#[tokio::test]
async fn api_buy_in_another_currency_than_a_payment_on_its_listing_returns_422() {
    use crate::entities::corporate_action::{ActionKind, CorporateAction};
    let pool = test_pool().await;
    insert_test_listing(&pool).await;
    corporate_action::db_upsert(
        &pool,
        &CorporateAction {
            id: 1,
            listing_id: ListingId(1),
            date: ymd(2024, 5, 1),
            kind: ActionKind::ReturnOfCapital {
                amount_per_unit: dec("0.50"),
                currency: "USD".into(),
                record_date: None,
            },
        },
    )
    .await
    .unwrap();

    let buy = |date: &str| {
        serde_json::json!({
            "trade_type": "Buy",
            "date": date,
            "listing_id": 1,
            "average_price": "10",
            "quantity": "100",
            "currency": "AUD",
            "brokerage": "0",
            "gst_on_brokerage": "0",
            "brokerage_currency": "AUD",
            "fx_rate": "1"
        })
    };
    // Acquired before the payment, so the payment reaches it: refused,
    // naming the payment and both currencies.
    let (status, detail) = put_trade_json(&pool, 1, buy("2024-01-15")).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert!(
        detail.contains("held in AUD")
            && detail.contains("2024-05-01")
            && detail.contains("recorded in USD"),
        "detail must name the payment and both currencies: {detail}"
    );
    assert!(
        db_get(&pool, TradeId(1)).await.unwrap().is_none(),
        "nothing persisted"
    );
    // A parcel acquired *after* the payment is fine: it was never entitled
    // to it, so nothing ever nets the two currencies.
    let (status, _) = put_trade_json(&pool, 2, buy("2024-06-03")).await;
    assert_eq!(status, StatusCode::CREATED);
}

/// A trade is recorded in its listing's own currency (SCENARIOS M-08).
/// `average_price` **is** the price of that listed security, so the two
/// are the same money — the rule ESS statements apply to a per-share
/// market value, inheritances to a parcel's cost base, and the DRP
/// reinvest path to a distribution's cash. Without it a US-quoted share
/// bought "in AUD" divides an AUD price by a USD rate in every cost-base
/// report, while its closing prices — collected from the exchange in the
/// listing's currency — value it against a parcel costed in another.
#[tokio::test]
async fn api_trade_currency_must_be_the_listings() {
    let pool = test_pool().await;
    insert_usd_listing(&pool).await;
    let buy = |currency: &str| {
        serde_json::json!({
            "trade_type": "Buy", "date": "2024-01-16", "listing_id": 1,
            "average_price": "100", "quantity": "10", "currency": currency,
            "brokerage": "0", "gst_on_brokerage": "0",
            "brokerage_currency": currency, "fx_rate": "1",
        })
    };
    let (status, detail) = put_trade_json(&pool, 1, buy("AUD")).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert!(
        detail.contains("recorded in AUD") && detail.contains("quoted in USD"),
        "the refusal names both currencies: {detail}"
    );
    assert!(
        db_get(&pool, TradeId(1)).await.unwrap().is_none(),
        "nothing written"
    );

    // In the listing's own currency it goes through.
    let (status, _) = put_trade_json(&pool, 1, buy("USD")).await;
    assert_eq!(status, StatusCode::CREATED);

    // A Sell of the same parcel meets the same rule on its own path.
    let sell = |currency: &str| {
        serde_json::json!({
            "date": "2024-06-03", "listing_id": 1, "average_price": "150",
            "quantity": "10", "currency": currency, "brokerage": "0",
            "gst_on_brokerage": "0", "brokerage_currency": currency,
            "fx_rate": "1",
            "allocations": [{ "purchase_trade_id": 1, "quantity_allocated": "10" }],
        })
    };
    let full = ApiClient::full(&pool);
    let resp = full.put("/sells/2", &sell("AUD")).await;
    let (status, detail) = resp.status_and_body();
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert!(
        detail.contains("recorded in AUD") && detail.contains("quoted in USD"),
        "the Sell refusal names both currencies: {detail}"
    );
    assert!(
        db_get(&pool, TradeId(2)).await.unwrap().is_none(),
        "nothing written"
    );
    full.put("/sells/2", &sell("USD"))
        .await
        .expect_status(StatusCode::CREATED);
}

/// The boolean column is CHECK-constrained to 0/1 in the database.
#[tokio::test]
async fn db_brokerage_includes_gst_check_constraint_enforced() {
    let pool = test_pool().await;
    insert_test_listing(&pool).await;
    db_upsert(&pool, &buy_trade()).await.unwrap();
    let err = sqlx::query("UPDATE trades SET brokerage_includes_gst = 2 WHERE id = 1")
        .execute(&pool)
        .await
        .unwrap_err();
    assert!(err.to_string().contains("CHECK"), "{err}");
}
