//! Every DELETE route: a missing row is a 404 naming what was missing, and a
//! row a dependant still references is a 422 naming the dependant.

use super::*;

/// Every entity DELETE route, with the exact 404 body it must answer.
///
/// A delete is fired from a list row, so its failure only ever reaches the
/// user as a toast — an empty-bodied 404 shows as a bare "HTTP 404". The
/// contract had drifted three ways (eight routes returned a bare
/// `StatusCode::NOT_FOUND`) before `infra::http::deleted` and the
/// `CrudEntity` delete handler made one wording the default; this table
/// keeps a new entity from drifting again.
///
/// The body is pinned whole rather than merely containing the noun: an
/// entity keyed on a natural key must name *its* key, not `id` — the URL
/// for `/exchanges/ZZZZ` never carries an id, so `no exchange with that
/// id` would send the user looking for a column that is not there.
const DELETE_ROUTES: &[(&str, &str)] = &[
    ("/amma_statements/9999", "no AMMA statement with that id"),
    ("/amit_adjustments/9999", "no AMIT adjustment with that id"),
    ("/attachments/9999", "no attachment with that id"),
    ("/cgt_settings/9999", "no CGT settings row with that id"),
    (
        "/closing_prices/9999/2024-01-02",
        "no stored price for that listing and date",
    ),
    (
        "/corporate_actions/9999",
        "no corporate action with that id",
    ),
    ("/drp_enrolments/9999", "no DRP enrolment with that id"),
    ("/ess_statements/9999", "no ESS statement with that id"),
    ("/exchanges/ZZZZ", "no exchange with that mic"),
    (
        "/exchange_holidays/ZZZZ/2024-01-02",
        "no exchange holiday on that date for that exchange",
    ),
    ("/holding_accounts/9999", "no holding account with that id"),
    (
        "/holding_unit_scales/9999",
        "no unit-scale override with that id",
    ),
    ("/income/9999", "no income with that id"),
    ("/income/9999/reinvest", "no distribution with that id"),
    ("/inheritances/9999", "no inheritance with that id"),
    ("/interest_income/9999", "no interest income with that id"),
    (
        "/investment_expenses/9999",
        "no investment expense with that id",
    ),
    ("/listings/9999", "no listing with that id"),
    ("/listings/9999/renames/9999", "no rename with that id"),
    ("/rights_sales/9999", "no rights sale with that id"),
    ("/sells/9999", "no sell with that id"),
    (
        "/tax_year_settings/2026",
        "no tax year settings row for that year",
    ),
    ("/trades/9999", "no trade with that id"),
    ("/transfers/9999", "no transfer with that id"),
];

#[tokio::test]
async fn deleting_a_missing_row_is_404_naming_what_was_missing() {
    for (uri, expected) in DELETE_ROUTES {
        let pool = test_pool().await;
        let resp = ApiClient::over(router().with_state(pool))
            .delete(*uri)
            .await;
        assert_eq!(resp.status, StatusCode::NOT_FOUND, "DELETE {uri}");
        assert_eq!(
            resp.text(),
            *expected,
            "DELETE {uri} answered 404 with the wrong body"
        );
    }
}

/// A natural-key entity must override [`CrudEntity::missing_row_body`],
/// whose default names a column its URL never carries.
///
/// The `DELETE_ROUTES` table above pins the exact body for every delete
/// route that exists; the entity-type half is pinned here, so a new
/// natural-key entity that forgets the override fails at the type rather
/// than only when someone remembers to add its route to that table. The
/// two below are the only keyed entities with a DELETE route
/// (`currencies`/`mic_registry` are keyed too but expose no delete).
#[test]
fn a_natural_key_entity_names_its_key_in_its_delete_404() {
    use crate::entities::exchange::Exchange;
    use crate::entities::listing::Listing;
    use crate::entities::tax_year_settings::TaxYearSettings;
    use crate::infra::http::CrudEntity;

    assert_ne!(Exchange::KEY_COLUMN, "id");
    assert_ne!(TaxYearSettings::KEY_COLUMN, "id");
    assert_eq!(Exchange::missing_row_body(), "no exchange with that mic");
    assert_eq!(
        TaxYearSettings::missing_row_body(),
        "no tax year settings row for that year"
    );
    // …and a rowid-keyed entity keeps the default wording.
    assert_eq!(Listing::KEY_COLUMN, "id");
    assert_eq!(Listing::missing_row_body(), "no listing with that id");
}

/// A DELETE blocked by an *inbound* foreign key must say so — the row is
/// there and something depends on it.
///
/// `ApiError`'s shared `From<sqlx::Error>` reads the same SQLite error kind
/// as an *outbound* reference ("the request refers to a record that does
/// not exist"), which is right for a write naming an unknown listing and
/// exactly backwards for a delete: it denied the row's existence and named
/// nothing to clear. Each case below is one of the reproductions in
/// SCENARIOS.md section A.
#[tokio::test]
async fn a_delete_blocked_by_a_dependant_names_it_rather_than_denying_the_row_exists() {
    use crate::entities::corporate_action::{ActionKind, CorporateAction};
    use crate::test_support::{amma, buy, closing_price, dec, listing, ymd};
    use serde_json::json;

    // A-18/A-19: an AMMA statement whose generated AMIT adjustments must
    // go first — and the old message never said there were any.
    let pool = test_pool().await;
    listing(1).amit(true).insert(&pool).await;
    buy(1, 1).insert(&pool).await;
    amma(1, 1).insert(&pool).await;
    let client = ApiClient::over(router().with_state(pool));
    client
        .put_ok(
            "/amit_adjustments/1",
            &json!({ "amma_statement_id": 1, "trade_id": 1, "quantity": "100" }),
        )
        .await;
    let (status, body) = {
        let resp = client.delete("/amma_statements/1").await;
        (resp.status, resp.text().to_owned())
    };
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(
        body,
        "this AMMA statement is still referenced by AMIT adjustments (1) — remove those \
             records first"
    );

    // A-23: a listing whose only dependant is stored price history.
    let pool = test_pool().await;
    listing(1).insert(&pool).await;
    closing_price(1, ymd(2024, 1, 2)).insert(&pool).await;
    closing_price(1, ymd(2024, 1, 3)).insert(&pool).await;
    let client = ApiClient::over(router().with_state(pool));
    let (status, body) = {
        let resp = client.delete("/listings/1").await;
        (resp.status, resp.text().to_owned())
    };
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(
        body,
        "this listing is still referenced by closing prices (2) — remove those records first"
    );

    // A-41: an exchange a listing (and the seeded holiday calendar) hangs
    // off — every blocking table is named, with its row count.
    let pool = test_pool().await;
    listing(1).mic("XASX").insert(&pool).await;
    let client = ApiClient::over(router().with_state(pool));
    // A ticker-only rename names XASX as both its old *and* its new
    // exchange — one row, two foreign keys, and it must be counted once.
    client
        .post(
            "/listings/1/renames",
            &json!({ "effective_date": "2024-06-01", "ticker": "NEW" }),
        )
        .await
        .expect_status(StatusCode::CREATED);
    let (status, body) = {
        let resp = client.delete("/exchanges/XASX").await;
        (resp.status, resp.text().to_owned())
    };
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert!(
        body.starts_with("this exchange is still referenced by ")
            && body.contains("listing renames (1)")
            && body.contains("listings (1)")
            && body.contains("exchange holidays ("),
        "{body:?}"
    );

    // A-38: a corporate action frozen by its own trade group — the one
    // blocked delete that already answered 422, with the wrong reason.
    let pool = test_pool().await;
    listing(1).insert(&pool).await;
    corporate_action::db_upsert(
        &pool,
        &CorporateAction {
            id: 1,
            listing_id: ListingId(1),
            date: ymd(2024, 3, 1),
            kind: ActionKind::RightsIssue {
                rights_units: dec("1"),
                rights_held_units: dec("4"),
                exercise_price: dec("1.80"),
                currency: "AUD".into(),
                renounceable: true,
            },
        },
    )
    .await
    .unwrap();
    buy(1, 1)
        .date(ymd(2024, 1, 10))
        .settlement(ymd(2024, 1, 12))
        .qty(dec("400"))
        .insert(&pool)
        .await;
    let client = ApiClient::over(router().with_state(pool));
    client
        .post(
            "/corporate_actions/1/exercise",
            &json!({ "date": "2024-03-05", "units": "100" }),
        )
        .await
        .expect_status(StatusCode::CREATED);
    let (status, body) = {
        let resp = client.delete("/corporate_actions/1").await;
        (resp.status, resp.text().to_owned())
    };
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(
        body,
        "this corporate action is still referenced by trades (1) — remove those records first"
    );
}
