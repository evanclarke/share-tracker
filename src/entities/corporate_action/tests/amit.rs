//! A return of capital is refused on an AMIT listing.

use super::*;

// A return of capital is refused on an AMIT
//
// An AMIT reduces its unit holders' cost base through its AMMA
// statement's per-unit `cost_base_adjustment` (CGT event E10); the E4
// mechanism a return of capital models is for non-AMIT trusts. Nothing
// relates the two, so the same money entered both ways used to reduce the
// parcel twice with no cross-check catching it (SCENARIOS E-04).

/// E-04: the double entry — the AMMA statement's 50c/unit generated onto
/// the parcel, then the same 50c entered again as a payment. The write is
/// refused, naming where the reduction belongs, and the parcel keeps the
/// one reduction it should have.
#[tokio::test]
async fn api_return_of_capital_on_an_amit_listing_returns_422() {
    let pool = test_pool().await;
    test_support::listing(1)
        .ticker("VDHG")
        .name("VDHG")
        .amit(true)
        .insert(&pool)
        .await;
    test_support::buy(1, 1)
        .date(d(2023, 1, 10))
        .qty("100".parse().unwrap())
        .price("10".parse().unwrap())
        .insert(&pool)
        .await;
    test_support::amma(1, 1)
        .cost_base_adjustment("0.50".parse().unwrap())
        .with(|a| a.tax_year_end_date = d(2024, 6, 30))
        .insert(&pool)
        .await;
    test_support::amit_adjustment(&pool, 1, 1, 1, "100".parse().unwrap()).await;

    let resp = client(&pool)
        .put(
            "/corporate_actions/1",
            &serde_json::json!({
                "action_type": "ReturnOfCapital",
                "listing_id": 1,
                "date": "2024-05-01",
                "amount_per_unit": "0.50",
                "currency": "AUD",
            }),
        )
        .await;
    assert_eq!(resp.status, StatusCode::UNPROCESSABLE_ENTITY);
    let detail = resp.text().to_owned();
    assert!(
        detail.contains("cost_base_adjustment") && detail.contains("E10"),
        "the 422 must name where the reduction belongs: {detail}"
    );
    assert!(
        db_get(&pool, 1).await.unwrap().is_none(),
        "nothing persisted"
    );

    // The parcel still carries the AMMA statement's reduction alone:
    // 1000 − 50, not the 900 the accepted double entry produced.
    let parcels = reports::open_parcels::db_open_parcels(&pool, None)
        .await
        .unwrap();
    assert_eq!(parcels.len(), 1);
    assert_eq!(
        parcels[0].amit_cost_base_reduction,
        "50".parse::<Decimal>().unwrap()
    );
    assert_eq!(
        parcels[0].return_of_capital_reduction,
        "0".parse::<Decimal>().unwrap()
    );
    assert_eq!(
        parcels[0].remaining_cost_base,
        "950".parse::<Decimal>().unwrap()
    );
}

/// SCENARIOS F-23: on a fund that *converted* to an AMIT, the refusal
/// follows the payment's own year. The pre-conversion years' tax-deferred
/// amounts were ordinary E4 reductions and stay both enterable and
/// editable after the flag goes on — the E4 cross-check asks for them, so
/// refusing them left a year that could not be completed at all — while a
/// payment dated in an AMIT year is refused as usual.
#[tokio::test]
async fn api_return_of_capital_on_a_converted_fund_follows_the_payments_year() {
    let pool = test_pool().await;
    test_support::listing(1)
        .ticker("VDHG")
        .name("VDHG")
        .amit_from(d(2024, 7, 1)) // first AMIT year: FY2025
        .insert(&pool)
        .await;

    let payment = |date: &str, amount: &str| {
        serde_json::json!({
            "action_type": "ReturnOfCapital",
            "listing_id": 1,
            "date": date,
            "amount_per_unit": amount,
            "currency": "AUD",
        })
    };
    let c = client(&pool);
    // FY2024, before the conversion: accepted (a create, so 201)…
    assert_eq!(
        c.put("/corporate_actions/1", &payment("2024-05-01", "0.50"))
            .await
            .status,
        StatusCode::CREATED
    );
    // …and still editable afterwards (correcting the amount years later).
    assert_eq!(
        c.put("/corporate_actions/1", &payment("2024-05-01", "0.55"))
            .await
            .status,
        StatusCode::NO_CONTENT
    );

    // FY2025, the first AMIT year: refused, pointing at the AMMA.
    let resp = c
        .put("/corporate_actions/2", &payment("2025-05-01", "0.50"))
        .await;
    assert_eq!(resp.status, StatusCode::UNPROCESSABLE_ENTITY);
    assert!(
        resp.text().contains("cost_base_adjustment"),
        "{}",
        resp.text()
    );
    assert!(db_get(&pool, 2).await.unwrap().is_none());
}

/// The refusal is keyed on the listing's `amit` flag, so the E4 path it
/// exists for is untouched: the same payment on an ordinary trust is
/// accepted, and *moving* an accepted one onto an AMIT is refused (the
/// check runs over the state the write would leave, like its neighbours).
#[tokio::test]
async fn api_return_of_capital_is_refused_only_where_the_listing_is_an_amit() {
    let pool = test_pool().await;
    insert_listing(&pool, 1, "TRU").await;
    test_support::listing(2)
        .ticker("VDHG")
        .name("VDHG")
        .amit(true)
        .insert(&pool)
        .await;

    let payment = |listing_id: ListingId| {
        serde_json::json!({
            "action_type": "ReturnOfCapital",
            "listing_id": listing_id,
            "date": "2024-05-01",
            "amount_per_unit": "0.50",
            "currency": "AUD",
        })
    };
    let resp = client(&pool)
        .put("/corporate_actions/1", &payment(ListingId(1)))
        .await;
    assert_eq!(
        resp.status,
        StatusCode::CREATED,
        "a non-AMIT trust's E4 reduction is exactly what this action is for"
    );

    let resp = client(&pool)
        .put("/corporate_actions/1", &payment(ListingId(2)))
        .await;
    assert_eq!(resp.status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(
        db_get(&pool, 1).await.unwrap().unwrap().listing_id,
        1,
        "the refused move left the stored row on its own listing"
    );

    // Every other action type is unaffected — only the payment carries a
    // cost-base reduction the AMMA statement already carries.
    let resp = client(&pool)
        .put(
            "/corporate_actions/2",
            &serde_json::json!({
                "action_type": "ShareSplit",
                "listing_id": 2,
                "date": "2024-05-01",
                "split_new_units": "2",
                "split_old_units": "1",
            }),
        )
        .await;
    assert_eq!(resp.status, StatusCode::CREATED);
}
