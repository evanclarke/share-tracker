//! Figures a `Decimal` must hold: a quantity sent as a JSON number, a cost
//! base that cannot be represented, and a quantity a recorded ratio re-bases
//! out of range.

use super::*;
use crate::entities::corporate_action::FractionRule;

// -----------------------------------------------------------------------
// A money/quantity figure sent as a JSON number (SCENARIOS W-a)
// -----------------------------------------------------------------------

/// A trade body with `quantity` and `average_price` written out verbatim,
/// so a test can send an unquoted JSON number the way a bulk import would.
fn trade_body_raw(quantity: &str, average_price: &str) -> String {
    format!(
        r#"{{"trade_type":"Buy","date":"2024-01-15","listing_id":1,
                 "average_price":{average_price},"quantity":{quantity},"currency":"AUD",
                 "brokerage":"0","brokerage_currency":"AUD","fx_rate":"1"}}"#
    )
}

/// `{"quantity": 100000000.00000001}` used to be accepted `204` and stored
/// as `100000000` — `serde_json` hands a JSON number over as an `f64`,
/// which keeps ~15 significant digits, so a satoshi went missing under a
/// success (SCENARIOS W-a). It is now refused, naming the field.
#[tokio::test]
async fn api_a_quantity_sent_as_a_json_number_is_refused_naming_the_field() {
    let pool = test_pool().await;
    insert_test_listing(&pool).await;

    for quantity in ["100000000.00000001", "99999999.87654321", "10", "10.0"] {
        let response = client(&pool)
            .put_raw("/trades/1", &trade_body_raw(quantity, "\"1\""))
            .await;
        let (status, body) = response.status_and_body();
        assert_eq!(
            status,
            StatusCode::UNPROCESSABLE_ENTITY,
            "quantity {quantity} as a JSON number: {body}"
        );
        assert!(body.contains("quantity"), "the field is not named: {body}");
        assert!(
            body.contains("as a decimal string"),
            "the remedy is not stated: {body}"
        );
    }

    // An unquoted `average_price` is refused the same way, naming itself.
    let response = client(&pool)
        .put_raw(
            "/trades/1",
            &trade_body_raw("\"1\"", "1234567890123456789.12"),
        )
        .await;
    let (status, body) = response.status_and_body();
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert!(body.contains("average_price"), "{body}");

    // Nothing was written by any of the refusals.
    assert!(db_get(&pool, TradeId(1)).await.unwrap().is_none());
}

/// The control the finding was bounded with: the very same figures sent as
/// **strings** are accepted and stored to the digit. The refusal above is
/// therefore about the JSON encoding, not about the values.
#[tokio::test]
async fn api_the_same_quantity_sent_as_a_string_is_stored_exactly() {
    let pool = test_pool().await;
    insert_test_listing(&pool).await;

    for (id, quantity) in [(1, "100000000.00000001"), (2, "99999999.87654321")] {
        let path = format!("/trades/{id}");
        let sent = trade_body_raw(&format!("\"{quantity}\""), "\"1\"");
        let response = client(&pool).put_raw(&path, &sent).await;
        let (status, body) = response.status_and_body();
        assert_eq!(status, StatusCode::CREATED, "{body}");

        // Straight off the column, so nothing on the read path can round it.
        let stored: String = sqlx::query_scalar("SELECT quantity FROM trades WHERE id = ?")
            .bind(id)
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(stored, quantity, "stored quantity lost a digit");
    }
}

// -----------------------------------------------------------------------
// A cost base that cannot be represented (SCENARIOS W-e)
// -----------------------------------------------------------------------

/// The finding's own trade: `1e15 × 1e15` is `1e30`, past
/// `rust_decimal`'s ~7.9228e28 ceiling. It used to be accepted `204` and
/// then killed `GET /portfolio/open_parcels`, `POST /portfolio/overview`
/// and `POST /portfolio/unrealised_gains` — a logged `500` with an empty
/// body since W-b's panic layer, a dropped connection before it — leaving
/// the offending row invisible in exactly the reports that would have
/// found it. Now refused at the write, naming the product **and** the
/// limit, which is what makes the ceiling defensible: it is `Decimal::MAX`
/// read off the type, not a policy number.
#[tokio::test]
async fn api_a_trade_whose_cost_base_cannot_be_represented_is_refused_naming_it() {
    let pool = test_pool().await;
    insert_test_listing(&pool).await;

    let response = client(&pool)
        .put(
            "/trades/9600",
            &serde_json::json!({
                "trade_type": "Buy",
                "date": "2023-03-15",
                "listing_id": 1,
                "average_price": "1000000000000000",
                "quantity": "1000000000000000",
                "currency": "AUD",
                "brokerage": "0",
                "gst_on_brokerage": "0",
                "brokerage_includes_gst": false,
                "brokerage_currency": "AUD",
                "fx_rate": "1",
                "holding_account_id": 1,
            }),
        )
        .await;
    let (status, body) = response.status_and_body();

    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    // The product, both of its factors, and the limit itself.
    assert!(
        body.contains("average_price 1000000000000000 × quantity 1000000000000000"),
        "the product is not named: {body}"
    );
    assert!(
        body.contains(&rust_decimal::Decimal::MAX.to_string()),
        "the limit is not named: {body}"
    );
    // Nothing was written, so the reports stay readable.
    assert!(db_get(&pool, TradeId(9600)).await.unwrap().is_none());
}

/// The control, measured before the refusal existed and unchanged by it: a
/// large-but-representable parcel (`price 1 × quantity 1e14`, a cost base
/// of 1e14) still writes `204` and still reads back through
/// `GET /portfolio/open_parcels` at that exact figure. The bound is the
/// type's, so everything the type can hold is still accepted.
#[tokio::test]
async fn api_a_large_but_representable_cost_base_still_writes_and_reads_back() {
    let pool = test_pool().await;
    insert_test_listing(&pool).await;

    client(&pool)
        .put_ok(
            "/trades/9601",
            &serde_json::json!({
                "trade_type": "Buy",
                "date": "2023-03-15",
                "listing_id": 1,
                "average_price": "1",
                "quantity": "100000000000000",
                "currency": "AUD",
                "brokerage": "0",
                "gst_on_brokerage": "0",
                "brokerage_includes_gst": false,
                "brokerage_currency": "AUD",
                "fx_rate": "1",
                "holding_account_id": 1,
            }),
        )
        .await;

    let rows: serde_json::Value = ApiClient::full(&pool)
        .get_json("/portfolio/open_parcels")
        .await;
    assert_eq!(rows[0]["trade_id"], 9601);
    assert_eq!(rows[0]["original_cost_base"], "100000000000000");
    assert_eq!(rows[0]["remaining_cost_base"], "100000000000000");
}

/// A database that already holds such a row — entered by a build that
/// predates the guard — must stay correctable, which is the trap W-d's
/// sibling rule fell into (its first draft refused edits as well as
/// creations). The refusal reads the figures *being written*, never the
/// stored ones, so an edit that brings the parcel back inside the range
/// lands; only an edit that leaves it outside is refused.
#[tokio::test]
async fn api_an_already_unrepresentable_parcel_can_still_be_corrected() {
    let pool = test_pool().await;
    insert_test_listing(&pool).await;
    test_support::insert_parcel_bypassing_checks(
        &pool,
        9600,
        1,
        ymd(2023, 3, 15),
        "1000000000000000",
        "1000000000000000",
    )
    .await;

    let correction = |price: &str, quantity: &str| {
        serde_json::json!({
            "trade_type": "Buy",
            "date": "2023-03-15",
            "listing_id": 1,
            "average_price": price,
            "quantity": quantity,
            "currency": "AUD",
            "brokerage": "0",
            "gst_on_brokerage": "0",
            "brokerage_includes_gst": false,
            "brokerage_currency": "AUD",
            "fx_rate": "1",
            "holding_account_id": 1,
        })
    };

    // The typo corrected: accepted, and the parcel is readable again.
    client(&pool)
        .put_ok("/trades/9600", &correction("10", "100"))
        .await;
    let rows: serde_json::Value = ApiClient::full(&pool)
        .get_json("/portfolio/open_parcels")
        .await;
    assert_eq!(rows[0]["original_cost_base"], "1000");

    // A "correction" whose own product is still unrepresentable is not a
    // correction — each field is representable on its own here, so it is
    // the product this refuses and not the decimal decoder.
    let response = client(&pool)
        .put(
            "/trades/9600",
            &correction("1000000000000000", "1000000000000000"),
        )
        .await;
    let (status, body) = response.status_and_body();
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert!(
        body.contains("multiply out beyond what can be recorded"),
        "{body}"
    );
    // …and the accepted correction is what is still stored.
    assert_eq!(
        db_get(&pool, TradeId(9600))
            .await
            .unwrap()
            .unwrap()
            .average_price,
        Decimal::from(10)
    );
}

// -----------------------------------------------------------------------
// A quantity a recorded ratio re-bases out of range
// ("A parcel entered behind a ratio that already fits")
// -----------------------------------------------------------------------

/// A `ShareSplit` on listing 1, so a parcel behind it is re-based at read
/// time by `new`-for-`old`.
async fn insert_split(pool: &SqlitePool, id: i64, date: NaiveDate, new: &str, old: &str) {
    corporate_action::db_upsert(
        pool,
        &CorporateAction {
            id,
            listing_id: ListingId(1),
            date,
            kind: ActionKind::ShareSplit {
                fraction_rule: FractionRule::Exact,
                split_new_units: dec(new),
                split_old_units: dec(old),
            },
        },
    )
    .await
    .unwrap();
}

/// A nil-priced Buy of `quantity` units on listing 1, dated 2024-01-15.
fn nil_priced_buy(quantity: &str) -> serde_json::Value {
    serde_json::json!({
        "trade_type": "Buy",
        "date": "2024-01-15",
        "settlement_date": "2024-01-15",
        "listing_id": 1,
        "average_price": "0",
        "quantity": quantity,
        "currency": "AUD",
        "brokerage": "0",
        "gst_on_brokerage": "0",
        "brokerage_includes_gst": false,
        "brokerage_currency": "AUD",
        "fx_rate": "1",
        "holding_account_id": 1,
    })
}

/// The finding, measured at the HTTP surface before it was fixed: a
/// 1000-for-1 split is recorded on a listing with **no holdings**, which is
/// correct and stays `204` (`corporate_action`'s own check has nothing to
/// refuse); then a nil-priced parcel of 1e27 units is entered behind it.
/// W-e's money bound is on `average_price × quantity`, which is nil here,
/// so nothing used to ask what the listing's recorded ratios do to the
/// *quantity* — the parcel landed `204` and `GET /portfolio/open_parcels`
/// and `POST /portfolio/overview` were both a logged `500` afterwards.
#[tokio::test]
async fn api_a_parcel_behind_a_ratio_that_already_fits_is_refused_naming_it() {
    let pool = test_pool().await;
    insert_test_listing(&pool).await;
    insert_split(&pool, 10, ymd(2024, 6, 1), "1000", "1").await;

    let response = client(&pool)
        .put(
            "/trades/900",
            &nil_priced_buy("1000000000000000000000000000"),
        )
        .await;
    let (status, detail) = response.status_and_body();
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{detail}");
    // The quantity, the ratio, and the limit — the one wording every
    // beyond-the-range refusal answers with.
    assert!(
        detail.contains("quantity 1000000000000000000000000000 × new units 1000 / old units 1"),
        "the quantity and the ratio are not named: {detail}"
    );
    assert!(
        detail.contains(&rust_decimal::Decimal::MAX.to_string()),
        "the limit is not named: {detail}"
    );
    assert!(
        detail.contains("the number of units the holding becomes"),
        "{detail}"
    );

    // Nothing was written, and the reads the state used to kill still answer.
    let exists: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM trades WHERE id = 900)")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert!(!exists);
    let rows: Vec<serde_json::Value> = ApiClient::full(&pool)
        .get_json("/portfolio/open_parcels")
        .await;
    assert!(rows.is_empty(), "{rows:?}");
}

/// The control, and the figures are the ones this build answered *before*
/// the refusal existed: 7.9e25 units behind the same real 1000-for-1 split
/// re-base to 7.9e28, inside `Decimal`'s ~7.9228e28 ceiling, so the parcel
/// lands and both reads report it unchanged. The bound is the arithmetic's,
/// not a policy number.
#[tokio::test]
async fn api_a_large_parcel_a_recorded_ratio_still_fits_lands_and_reports() {
    let pool = test_pool().await;
    insert_test_listing(&pool).await;
    insert_split(&pool, 10, ymd(2024, 6, 1), "1000", "1").await;

    client(&pool)
        .put("/trades/900", &nil_priced_buy("79000000000000000000000000"))
        .await
        .expect_status(StatusCode::CREATED);

    let client = ApiClient::full(&pool);
    let rows: Vec<serde_json::Value> = client.get_json("/portfolio/open_parcels").await;
    assert_eq!(rows.len(), 1, "{rows:?}");
    assert_eq!(rows[0]["original_quantity"], "79000000000000000000000000");
    assert_eq!(
        rows[0]["remaining_quantity"],
        "79000000000000000000000000000"
    );
    let overview: Vec<serde_json::Value> = client
        .post_json("/portfolio/overview", &serde_json::json!({}))
        .await;
    assert_eq!(overview[0]["quantity"], "79000000000000000000000000000");
}

/// A database may already hold such a parcel — a build predating this rule
/// wrote it, and the state is otherwise unreachable — so the refusal is
/// judged over the state the write *leaves behind*, never over the stored
/// row. The correction that brings the quantity back inside the range
/// therefore lands, and the row is still deletable. Moving the check ahead
/// of the INSERT fails this test, which is what pins the ordering
/// (`fc1fd7b` and `71a26d6` each tripped on the same trap first).
#[tokio::test]
async fn api_an_already_unrepresentable_parcel_can_still_be_corrected_and_deleted() {
    let pool = test_pool().await;
    insert_test_listing(&pool).await;
    insert_split(&pool, 10, ymd(2024, 6, 1), "1000", "1").await;
    // Straight into SQLite, behind the guard: only a pre-guard database can
    // hold this, which is precisely what the test is about.
    sqlx::query(
        "INSERT INTO trades \
             (id, trade_type, date, settlement_date, settlement_date_source, listing_id, \
              average_price, quantity, currency, brokerage, gst_on_brokerage, \
              brokerage_currency, fx_rate, holding_account_id) \
             VALUES (900, 'Buy', '2024-01-15', '2024-01-15', 'stated', 1, '0', \
                     '1000000000000000000000000000', 'AUD', '0', '0', 'AUD', '1', 1)",
    )
    .execute(&pool)
    .await
    .unwrap();
    // The state the guard exists to prevent: the open-holdings reads are a
    // logged 500 while it stands.
    assert_eq!(
        ApiClient::full(&pool)
            .get("/portfolio/open_parcels")
            .await
            .status,
        StatusCode::INTERNAL_SERVER_ERROR
    );

    // The correction: the same parcel, re-quantified, lands.
    client(&pool)
        .put("/trades/900", &nil_priced_buy("79000000000000000000000000"))
        .await
        .expect_status(StatusCode::NO_CONTENT);
    let rows: Vec<serde_json::Value> = ApiClient::full(&pool)
        .get_json("/portfolio/open_parcels")
        .await;
    assert_eq!(
        rows[0]["remaining_quantity"],
        "79000000000000000000000000000"
    );

    // And it is still deletable.
    assert_eq!(
        client(&pool).delete("/trades/900").await.status,
        StatusCode::NO_CONTENT
    );
}

/// The walk is at **every** split boundary after the parcel, not only at
/// the cumulative end: a 1000-for-1 split followed by a 1-for-1000
/// consolidation nets back to a ratio that fits, while the unit basis
/// between the two does not — and a report as at that date reads it.
#[tokio::test]
async fn api_a_parcel_unrepresentable_only_between_two_ratios_is_refused() {
    let pool = test_pool().await;
    insert_test_listing(&pool).await;
    insert_split(&pool, 10, ymd(2024, 6, 1), "1000", "1").await;
    insert_split(&pool, 11, ymd(2024, 9, 1), "1", "1000").await;

    let response = client(&pool)
        .put(
            "/trades/900",
            &nil_priced_buy("1000000000000000000000000000"),
        )
        .await;
    let (status, detail) = response.status_and_body();
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{detail}");
    assert!(
        detail.contains("× new units 1000 / old units 1 exceeds"),
        "the boundary that overflows is not the one named: {detail}"
    );
}
