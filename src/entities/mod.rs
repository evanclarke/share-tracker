//! Domain entities: each module owns one table's model, CRUD endpoints, and
//! write-time invariants. Add a new entity by dropping a file here and adding
//! one `pub mod` line plus one `.merge` below — `main.rs` never changes.
use axum::Router;
use sqlx::SqlitePool;

pub mod amit_adjustment;
pub mod amit_adjustment_generation;
pub mod amma;
pub mod attachment;
pub mod buyback_participation;
pub mod cgt_settings;
pub mod closing_price;
pub mod corporate_action;
pub mod currencies;
pub mod demerger;
pub mod distribution_event;
pub mod drp_enrolment;
pub mod drp_reinvestment;
pub mod ess_statement;
pub mod ess_vest;
pub mod exchange;
pub mod exchange_holiday;
pub mod holding_account;
pub mod income;
pub mod inheritance;
pub mod interest_income;
pub mod investment_expense;
pub mod listing;
pub mod listing_rename;
pub mod mic_registry;
pub mod parcel_allocation;
/// The price-change alert's send log and detection walk. Deliberately routeless
/// — it is the `price-alert` job's own state, not an entity the UI edits (see
/// the module docs), so it has no `.merge` line below.
pub mod price_alert;
pub mod rba_fx_rate;
pub mod rights_exercise;
pub mod rights_sale;
pub mod scrip_exchange;
pub mod sell;
pub mod tax_year_settings;
pub mod trade;
pub mod transfer;
pub mod worthless;

/// Merge every entity's routes into a single router.
pub fn router() -> Router<SqlitePool> {
    exchange::router()
        .merge(exchange_holiday::router())
        .merge(listing::router())
        .merge(listing_rename::router())
        .merge(holding_account::router())
        .merge(currencies::router())
        .merge(mic_registry::router())
        .merge(rba_fx_rate::router())
        .merge(trade::router())
        .merge(income::router())
        .merge(interest_income::router())
        .merge(investment_expense::router())
        .merge(amma::router())
        .merge(parcel_allocation::router())
        .merge(sell::router())
        .merge(amit_adjustment::router())
        .merge(amit_adjustment_generation::router())
        .merge(drp_enrolment::router())
        .merge(cgt_settings::router())
        .merge(tax_year_settings::router())
        .merge(closing_price::router())
        .merge(distribution_event::router())
        .merge(corporate_action::router())
        .merge(rights_exercise::router())
        .merge(rights_sale::router())
        .merge(buyback_participation::router())
        .merge(scrip_exchange::router())
        .merge(demerger::router())
        .merge(drp_reinvestment::router())
        .merge(ess_statement::router())
        .merge(ess_vest::router())
        .merge(inheritance::router())
        .merge(transfer::router())
        .merge(worthless::router())
        .merge(attachment::router())
}

#[cfg(test)]
/// The success outcome a `PUT` route reports.
///
/// The convention is `201 Created` with the created row on a fresh id and
/// `204 No Content` on a replace; the two exceptions are the only routes
/// that can answer one status, each with the reason it must.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum PutOutcome {
    /// `201` with the created row on a create, `204` on a replace.
    CreateThenReplace,
    /// `PUT /transfers/{id}`: create-only, so always `201` with the
    /// executed group — a bare `204` would hide the created Sell/Buy ids.
    AlwaysCreatedGroup,
    /// `PUT /rba_fx_rates/{id}`: a correction of an existing row, so
    /// always `204`; it can never create one (that is the import route).
    NeverCreates,
}

#[cfg(test)]
/// Every `PUT` route `entities::router()` serves, classified by outcome.
///
/// A new `PUT` route fails `every_put_route_reports_create_then_replace`
/// until it is added here, so the create-vs-replace signal cannot be left
/// out by omission. `api_spec::tests::every_put_route_documents_its_outcome`
/// **reads this list** (rather than re-declaring its own copy) to check the
/// generated OpenAPI document records each route's statuses.
pub(crate) const PUT_ROUTES: &[(&str, PutOutcome)] = &[
    ("/exchanges/{mic}", PutOutcome::CreateThenReplace),
    (
        "/exchange_holidays/{mic}/{date}",
        PutOutcome::CreateThenReplace,
    ),
    ("/listings/{id}", PutOutcome::CreateThenReplace),
    ("/holding_accounts/{id}", PutOutcome::CreateThenReplace),
    ("/trades/{id}", PutOutcome::CreateThenReplace),
    ("/income/{id}", PutOutcome::CreateThenReplace),
    ("/interest_income/{id}", PutOutcome::CreateThenReplace),
    ("/investment_expenses/{id}", PutOutcome::CreateThenReplace),
    ("/amma_statements/{id}", PutOutcome::CreateThenReplace),
    ("/amit_adjustments/{id}", PutOutcome::CreateThenReplace),
    ("/drp_enrolments/{id}", PutOutcome::CreateThenReplace),
    ("/cgt_settings/{id}", PutOutcome::CreateThenReplace),
    (
        "/tax_year_settings/{tax_year}",
        PutOutcome::CreateThenReplace,
    ),
    ("/corporate_actions/{id}", PutOutcome::CreateThenReplace),
    ("/ess_statements/{id}", PutOutcome::CreateThenReplace),
    ("/inheritances/{id}", PutOutcome::CreateThenReplace),
    ("/sells/{id}", PutOutcome::CreateThenReplace),
    (
        "/closing_prices/{listing_id}/{price_date}",
        PutOutcome::CreateThenReplace,
    ),
    ("/transfers/{id}", PutOutcome::AlwaysCreatedGroup),
    ("/rba_fx_rates/{id}", PutOutcome::NeverCreates),
];

#[cfg(test)]
/// How an entity list route reaches its rows.
#[derive(PartialEq, Eq)]
pub(crate) enum ListKind {
    /// Registered with the shared `http::list_handler`, so its filters are
    /// its `CrudEntity::Filter` and an unknown parameter is axum's `Query`
    /// rejection.
    Shared,
    /// A hand-written handler that decodes its own query string — the
    /// trades list (it presents the row), and the two shared lists that
    /// already had filters. It refuses an unknown parameter too.
    HandWrittenQuery,
    /// A hand-written list with no query decoding at all, left as it was
    /// by this item.
    HandWrittenIgnoringQuery,
}

#[cfg(test)]
/// One entity list route.
pub(crate) struct ListRoute {
    pub(crate) path: &'static str,
    /// The query parameters it accepts; `&[]` for none, which then
    /// requires a `reason`.
    pub(crate) filters: &'static [&'static str],
    pub(crate) kind: ListKind,
    /// Why an unfiltered list takes none — empty for a filtered one.
    pub(crate) reason: &'static str,
}

#[cfg(test)]
/// Every list route `entities::router` serves, classified. The filtered
/// ones carry the parameters this item added (or the pre-existing ones the
/// hand-written lists already took); the unfiltered ones say why.
///
/// `every_list_route_is_classified_for_filtering` scans the sources for
/// the shared `list_handler` registrations and fails on one missing here,
/// so a new entity list cannot ship without the filters its columns offer
/// (or a stated reason it takes none), and
/// `every_list_route_refuses_an_unknown_parameter` drives each classified
/// route so the refusal is real rather than documented.
pub(crate) const LIST_ROUTES: &[ListRoute] = &[
    // Filtered: the workhorse entity lists.
    ListRoute {
        path: "/listings",
        filters: &["exchange_mic", "security_type"],
        kind: ListKind::Shared,
        reason: "",
    },
    ListRoute {
        path: "/trades",
        filters: &["listing_id", "holding_account_id", "from", "to"],
        kind: ListKind::HandWrittenQuery,
        reason: "",
    },
    ListRoute {
        path: "/income",
        filters: &["listing_id", "holding_account_id", "from", "to"],
        kind: ListKind::Shared,
        reason: "",
    },
    ListRoute {
        path: "/interest_income",
        filters: &["holding_account_id", "from", "to"],
        kind: ListKind::Shared,
        reason: "",
    },
    ListRoute {
        path: "/investment_expenses",
        filters: &["listing_id", "holding_account_id", "from", "to"],
        kind: ListKind::Shared,
        reason: "",
    },
    ListRoute {
        path: "/amma_statements",
        filters: &["listing_id", "holding_account_id", "from", "to"],
        kind: ListKind::Shared,
        reason: "",
    },
    ListRoute {
        path: "/ess_statements",
        filters: &["listing_id", "holding_account_id", "from", "to"],
        kind: ListKind::Shared,
        reason: "",
    },
    ListRoute {
        path: "/inheritances",
        filters: &["listing_id", "holding_account_id", "from", "to"],
        kind: ListKind::Shared,
        reason: "",
    },
    ListRoute {
        path: "/corporate_actions",
        filters: &["listing_id", "from", "to"],
        kind: ListKind::Shared,
        reason: "",
    },
    ListRoute {
        path: "/transfers",
        filters: &["listing_id", "from", "to"],
        kind: ListKind::Shared,
        reason: "",
    },
    ListRoute {
        path: "/distribution_events",
        filters: &["listing_id", "from", "to"],
        kind: ListKind::Shared,
        reason: "",
    },
    ListRoute {
        path: "/drp_enrolments",
        filters: &["listing_id", "holding_account_id"],
        kind: ListKind::Shared,
        reason: "",
    },
    ListRoute {
        path: "/amit_adjustments",
        filters: &["amma_statement_id", "trade_id"],
        kind: ListKind::Shared,
        reason: "",
    },
    ListRoute {
        path: "/parcel_allocations",
        filters: &["sale_trade_id", "purchase_trade_id"],
        kind: ListKind::Shared,
        reason: "",
    },
    // Unfiltered entity lists: no filter this item's columns offer, or a
    // table only ever read whole.
    ListRoute {
        path: "/exchanges",
        filters: &[],
        kind: ListKind::Shared,
        reason: "Reference table: a handful of rows, read whole by every picker.",
    },
    ListRoute {
        path: "/currencies",
        filters: &[],
        kind: ListKind::Shared,
        reason: "Import-fed ISO reference table, read whole (a code-keyed GET-one exists).",
    },
    ListRoute {
        path: "/mic_registry",
        filters: &[],
        kind: ListKind::Shared,
        reason: "Import-fed ISO reference table, read whole (a MIC-keyed GET-one exists).",
    },
    ListRoute {
        path: "/rba_fx_rates",
        filters: &[],
        kind: ListKind::Shared,
        reason: "Import-fed reference table, read whole; its rows are corrected one at a time by id.",
    },
    ListRoute {
        path: "/holding_accounts",
        filters: &[],
        kind: ListKind::Shared,
        reason: "A handful of rows, read whole.",
    },
    ListRoute {
        path: "/cgt_settings",
        filters: &[],
        kind: ListKind::Shared,
        reason: "Singleton settings row.",
    },
    ListRoute {
        path: "/tax_year_settings",
        filters: &[],
        kind: ListKind::Shared,
        reason: "One row per financial year, read whole; the keyed GET-one is the narrowed read.",
    },
    // Hand-written lists outside this item's workhorse entity lists.
    // Their existing filters are unchanged; `/rights_sales` and
    // `/exchange_holidays` decode no query at all.
    ListRoute {
        path: "/closing_prices",
        filters: &["listing_id", "from", "to", "status"],
        kind: ListKind::HandWrittenQuery,
        reason: "",
    },
    ListRoute {
        path: "/attachments",
        filters: &[
            "trade_id",
            "income_id",
            "amma_statement_id",
            "ess_statement_id",
            "interest_income_id",
            "corporate_action_id",
            "include_linked",
        ],
        kind: ListKind::HandWrittenQuery,
        reason: "",
    },
    ListRoute {
        path: "/rights_sales",
        filters: &[],
        kind: ListKind::HandWrittenIgnoringQuery,
        reason: "Hand-written list outside this item's scope; it decodes no query string.",
    },
    ListRoute {
        path: "/exchange_holidays",
        filters: &[],
        kind: ListKind::HandWrittenIgnoringQuery,
        reason: "Hand-written list outside this item's scope; it decodes no query string.",
    },
    ListRoute {
        path: "/exchange_holidays/{mic}",
        filters: &[],
        kind: ListKind::HandWrittenIgnoringQuery,
        reason: "Path-narrowed to one exchange's calendar; outside this item's scope.",
    },
    ListRoute {
        path: "/listings/{id}/renames",
        filters: &[],
        kind: ListKind::HandWrittenIgnoringQuery,
        reason: "Path-narrowed to one listing's rename chain; outside this item's scope.",
    },
];

#[cfg(test)]
/// The filtered entity list routes as `(path, query parameters)`, read by
/// `api_spec` and `doc_checks` so the per-route filter documentation is derived
/// from the one classification table rather than transcribed beside it.
pub(crate) fn filtered_list_routes() -> Vec<(&'static str, &'static [&'static str])> {
    LIST_ROUTES
        .iter()
        .filter(|route| !route.filters.is_empty())
        .map(|route| (route.path, route.filters))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{ApiClient, test_pool};
    use axum::http::StatusCode;

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
            (resp.status, resp.text().to_string())
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
            (resp.status, resp.text().to_string())
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
                "/listings/1/rename",
                &json!({ "effective_date": "2024-06-01", "ticker": "NEW" }),
            )
            .await
            .expect_status(StatusCode::CREATED);
        let (status, body) = {
            let resp = client.delete("/exchanges/XASX").await;
            (resp.status, resp.text().to_string())
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
                listing_id: 1,
                date: ymd(2024, 3, 1),
                kind: ActionKind::RightsIssue {
                    rights_units: dec("1"),
                    rights_held_units: dec("4"),
                    exercise_price: dec("1.80"),
                    currency: "AUD".to_string(),
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
            (resp.status, resp.text().to_string())
        };
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
        assert_eq!(
            body,
            "this corporate action is still referenced by trades (1) — remove those records first"
        );
    }

    /// The other half of the contract: a *write* naming a row that really is
    /// missing keeps the outbound wording, so fixing the delete direction did
    /// not cost the write direction its message.
    #[tokio::test]
    async fn a_write_naming_an_unknown_row_still_says_the_record_does_not_exist() {
        let pool = test_pool().await;
        let client = ApiClient::over(router().with_state(pool));
        let resp = client
            .put(
                "/exchange_holidays/ZZZZ/2024-01-02",
                &serde_json::json!({ "name": "Nowhere Day" }),
            )
            .await;
        assert_eq!(
            resp.status_and_body(),
            (
                StatusCode::UNPROCESSABLE_ENTITY,
                "the request refers to a record that does not exist"
            )
        );
    }

    /// One entity's GET → PUT round trip.
    struct RoundTrip {
        /// The upsert path. Also the read path unless `get_path` differs — a
        /// Sell is written to `/sells/{id}` but read back as a trade.
        put_path: &'static str,
        get_path: Option<&'static str>,
        /// The body that creates the row.
        create: serde_json::Value,
        /// Fields merged into the read body before it is re-PUT: the child
        /// rows a PUT requires that the matching GET does not return (a Sell's
        /// parcel allocations). Null for the plain CRUD entities, which re-PUT
        /// what they read verbatim.
        graft: serde_json::Value,
        /// Read fields the write body does not own, *beyond* the shared
        /// [`test_support::NOT_CLIENT_WRITABLE`] set: the key columns an
        /// entity spells other than `id`, and the trade columns `/sells`
        /// does not share with `/trades`.
        read_only: &'static [&'static str],
    }

    impl RoundTrip {
        fn new(put_path: &'static str, create: serde_json::Value) -> Self {
            Self {
                put_path,
                get_path: None,
                create,
                graft: serde_json::Value::Null,
                read_only: &[],
            }
        }

        fn read_at(mut self, get_path: &'static str) -> Self {
            self.get_path = Some(get_path);
            self
        }

        fn grafting(mut self, graft: serde_json::Value) -> Self {
            self.graft = graft;
            self
        }

        fn read_only(mut self, fields: &'static [&'static str]) -> Self {
            self.read_only = fields;
            self
        }
    }

    /// Prerequisite rows every case's own row can hang off: an AMIT-bearing
    /// listing, a plain one, one open Buy parcel, and an AMMA statement.
    async fn seed_round_trip_fixtures(pool: &SqlitePool) {
        use crate::test_support::{amma, buy, dec, listing, ymd};

        listing(1).amit(true).insert(pool).await;
        // Franking credits belong to a non-AMIT listing, so the income case
        // has one of its own to hang off.
        listing(3)
            .ticker("NAM")
            .name("Non AMIT Ltd")
            .insert(pool)
            .await;
        // A USD listing for the ESS-statement case: a statement is entered in
        // its listing's currency (the per-share market value and the listed
        // price are the same money), so a foreign-currency statement needs a
        // foreign-currency listing to hang off.
        listing(4)
            .ticker("USDL")
            .name("US Listed Inc")
            .currency("USD")
            .insert(pool)
            .await;
        buy(1, 1)
            .qty(dec("100"))
            .date(ymd(2024, 1, 10))
            .settlement(ymd(2024, 1, 12))
            .insert(pool)
            .await;
        amma(1, 1).insert(pool).await;
    }

    /// Every entity whose GET-one output is meant to be PUT-able.
    ///
    /// Deliberately absent, each for a reason that means there is no read →
    /// write cycle to be lossy:
    ///
    /// - Stored closing prices, rights sales and parcel allocations — no
    ///   GET-one route to read a row back from (their reads are list-only).
    /// - Currencies, the MIC registry and RBA FX rates — import-fed, so the
    ///   write is a POST import rather than an upsert of a read body.
    /// - Transfers — created once and immutable (a re-PUT is refused; the way
    ///   to change one is delete and re-transfer), so a read is never written
    ///   back. Their allocations round-trip through the Sell case below,
    ///   which shares the `allocationEditor` shape.
    fn round_trip_cases() -> Vec<RoundTrip> {
        use serde_json::json;

        vec![
            RoundTrip::new(
                "/exchanges/XZZZ",
                json!({
                    "name": "Test Exchange", "country": "AU", "currency": "AUD",
                    "timezone": "Australia/Sydney", "settlement_days": 2,
                    "close_time": "16:00",
                }),
            )
            .read_only(&["mic"]),
            RoundTrip::new(
                "/exchange_holidays/XASX/2024-12-27",
                json!({ "name": "Test Holiday" }),
            )
            .read_only(&["mic", "holiday_date"]),
            RoundTrip::new(
                "/listings/2",
                json!({
                    "exchange_mic": "XASX", "ticker": "RTT", "name": "Round Trip Ltd",
                    "isin": "AU000000RTT1", "security_type": "Trust", "currency": "AUD",
                    "amit": true, "preference": false, "price_symbol": "RTT.AX",
                }),
            ),
            RoundTrip::new("/holding_accounts/3", json!({ "name": "Third" })),
            RoundTrip::new(
                "/trades/10",
                json!({
                    "trade_type": "Buy", "date": "2024-03-04", "listing_id": 1,
                    "average_price": "12.345678", "quantity": "100.123456",
                    "currency": "AUD", "brokerage": "19.95",
                    "brokerage_includes_gst": true, "brokerage_currency": "AUD",
                    "fx_rate": "1", "contract_note_ref": "CN-1",
                }),
            ),
            RoundTrip::new(
                "/income/10",
                json!({
                    "listing_id": 3, "date_paid": "2024-03-04", "ex_date": "2024-02-20",
                    "franked_amount": "123.46", "franking_credits": "52.910048",
                    "unfranked_amount": "1.05", "trust_income": true,
                    "entitlement_date": "2024-02-20", "amount_per_security": "0.1245068",
                    "securities_held": "1000", "tax_deferred_amount": "3.21",
                    "currency": "AUD",
                }),
            ),
            RoundTrip::new(
                "/interest_income/10",
                json!({
                    "date_paid": "2024-03-04", "amount": "45.678901",
                    "foreign_source": true,
                    "foreign_tax_paid": "1.234567", "currency": "USD",
                    "source": "Some Bank",
                }),
            ),
            RoundTrip::new(
                "/investment_expenses/10",
                json!({
                    "date_incurred": "2024-03-04", "expense_type": "ManagementFee",
                    // gross × pct reconciles to the cent (87.90790081 → 87.91),
                    // which the write-time apportionment check requires.
                    "amount": "87.907901", "gross_amount": "97.135802",
                    "deductible_percentage": "90.5", "currency": "AUD",
                    "description": "Annual fee", "listing_id": 1,
                }),
            ),
            RoundTrip::new(
                "/amma_statements/10",
                json!({
                    "listing_id": 1, "tax_year_end_date": "2024-06-30",
                    "date_received": "2024-08-15", "units_held": "1000.123456",
                    "franked_dividends": "12.345678", "franking_credits": "5.291005",
                    "cgt_discount_gains": "7.654321",
                    "cost_base_adjustment": "0.1234567890", "currency": "AUD",
                }),
            ),
            RoundTrip::new(
                "/amit_adjustments/10",
                json!({ "amma_statement_id": 1, "trade_id": 1, "quantity": "100" }),
            ),
            RoundTrip::new(
                "/drp_enrolments/10",
                json!({
                    "listing_id": 1, "enrolment_date": "2024-02-01",
                    "unenrolment_date": "2024-09-01", "residual_handling": "PayOut",
                }),
            ),
            RoundTrip::new(
                "/cgt_settings/1",
                json!({ "opening_capital_loss": "1234.567891" }),
            ),
            // On the non-AMIT listing: a return of capital is the E4
            // mechanism for a non-AMIT trust, and is refused on an AMIT
            // (whose cost-base movement is its AMMA statement's).
            RoundTrip::new(
                "/corporate_actions/10",
                json!({
                    "action_type": "ReturnOfCapital", "listing_id": 3, "date": "2024-05-01",
                    "amount_per_unit": "0.123456", "currency": "AUD",
                    "record_date": "2024-04-20",
                }),
            ),
            RoundTrip::new(
                "/ess_statements/10",
                json!({
                    "listing_id": 4, "taxing_point_date": "2024-03-04",
                    "quantity": "250.5", "market_value_per_share": "12.345678",
                    "deferral_discount": "3091.591239", "currency": "USD",
                    "fx_rate": "0.6666666667",
                    "aud_deferral_discount": "4637.386859",
                }),
            ),
            RoundTrip::new(
                "/inheritances/10",
                json!({
                    "listing_id": 1, "quantity": "500.123456",
                    "date_of_death": "2024-02-14", "cost_base_rule": "MarketValueAtDeath",
                    "cost_base": "6172.839012", "lpr_expenditure": "150.75",
                    "lpr_expenditure_date": "2024-03-01", "currency": "AUD",
                    "fx_rate": "1",
                }),
            ),
            // A Sell is written to /sells/{id} and read as a trade. Its parcel
            // allocations are child rows the GET does not return and the PUT
            // requires, so they are grafted back on.
            RoundTrip::new(
                "/sells/10",
                json!({
                    "date": "2024-06-03", "listing_id": 1, "average_price": "15.678901",
                    "quantity": "60", "currency": "AUD", "brokerage": "9.95",
                    "brokerage_includes_gst": true, "brokerage_currency": "AUD",
                    "fx_rate": "1",
                    "allocations": [{ "purchase_trade_id": 1, "quantity_allocated": "60" }],
                }),
            )
            .read_at("/trades/10")
            .grafting(json!({
                "allocations": [{ "purchase_trade_id": 1, "quantity_allocated": "60" }],
            }))
            // A trade read carries three columns `/sells` does not accept:
            // the type (the route says Sell) and the DRP residual chain.
            .read_only(&[
                "trade_type",
                "residual_brought_forward",
                "residual_carried_forward",
                "residual_paid_out",
            ]),
        ]
    }

    /// What a GET hands back must be exactly what a PUT accepts — bar the
    /// columns the write does not own — and storing it again must not move it.
    ///
    /// This is the one bug class the `db_*` tests structurally cannot reach:
    /// they build the body struct in Rust and never cross the JSON boundary,
    /// so a field the read renames, a `Decimal` that loses digits through
    /// serialisation, or a read shape the write rejects all pass at the DB
    /// level. It is not hypothetical — the GST-inclusive brokerage round trip
    /// was lossy in exactly this way (a read returned the ex-GST figure a
    /// re-PUT then split *again*), and it reached recorded data before anyone
    /// noticed. `sell.rs` pins that entity; this pins every other one, so a
    /// new entity is covered without its author having to remember.
    #[tokio::test]
    async fn what_a_get_returns_can_be_put_back_unchanged() {
        for case in round_trip_cases() {
            let get_path = case.get_path.unwrap_or(case.put_path);
            let pool = test_pool().await;
            seed_round_trip_fixtures(&pool).await;
            let client = ApiClient::over(router().with_state(pool));

            let created = client.put(case.put_path, &case.create).await;
            assert_eq!(
                created.status,
                StatusCode::CREATED,
                "PUT {} did not create the row: {}",
                case.put_path,
                created.text()
            );
            let first: serde_json::Value = client.get_json(get_path).await;
            // The 201 carries the created row: exactly what the following GET
            // answers, so a client never needs the extra read.
            assert_eq!(
                created.json::<serde_json::Value>(),
                first,
                "PUT {} answered 201 with a body the GET does not match",
                case.put_path
            );

            // The read carries what the write does not own — the key in the
            // URL, and the server-owned provenance/derived columns. Since
            // every request body denies unknown fields (SCENARIOS V-a) those
            // are refused, naming the first of them, rather than accepted and
            // ignored: a client that thinks it is editing provenance is told
            // otherwise.
            let verbatim = client.put(case.put_path, &first).await;
            let (status, body) = verbatim.status_and_body();
            assert_eq!(
                status,
                StatusCode::UNPROCESSABLE_ENTITY,
                "PUT {} accepted a read body carrying columns it does not own",
                case.put_path
            );
            assert!(
                body.contains("unknown field"),
                "PUT {} rejected the read body for another reason: {body}",
                case.put_path
            );

            let mut replay = crate::test_support::writable_body(&first, case.read_only);
            if let Some(graft) = case.graft.as_object() {
                let replay = replay.as_object_mut().expect("read body is a JSON object");
                for (key, value) in graft {
                    replay.insert(key.clone(), value.clone());
                }
            }
            let stored = client.put(case.put_path, &replay).await;
            assert_eq!(
                stored.status,
                StatusCode::NO_CONTENT,
                "PUT {} rejected the body GET {get_path} returned: {}",
                case.put_path,
                stored.text()
            );

            let second: serde_json::Value = client.get_json(get_path).await;
            assert_eq!(
                first, second,
                "GET {get_path} changed after its own body was PUT back"
            );
        }
    }

    /// Every `PUT` route on a fresh id answers `201 Created` with the created
    /// row, and the same `PUT` again answers `204 No Content`.
    ///
    /// `PUT` used to answer a bare `204` either way, so a stale or mistaken
    /// write clobbered a record with no signal that it had replaced anything.
    /// This is the round-trip contract's status half: the body half (the `201`
    /// equals the following `GET`, and a re-PUT of that body does not move the
    /// row) is `what_a_get_returns_can_be_put_back_unchanged` above. It drives
    /// [`PUT_ROUTES`] rather than a hand-picked list, so a new entity is
    /// covered without its author having to remember, and the two exceptions
    /// are named with the reason they differ.
    #[tokio::test]
    async fn every_put_route_reports_create_then_replace() {
        /// The [`PUT_ROUTES`] template a concrete request path instantiates, so
        /// the concrete paths the round-trip table carries can be compared with
        /// the templates the classification table names.
        fn template_for(path: &str) -> &'static str {
            let segments: Vec<&str> = path.split('/').collect();
            PUT_ROUTES
                .iter()
                .map(|(template, _)| *template)
                .find(|template| {
                    let pattern: Vec<&str> = template.split('/').collect();
                    pattern.len() == segments.len()
                        && pattern
                            .iter()
                            .zip(&segments)
                            .all(|(p, s)| p.starts_with('{') || p == s)
                })
                .unwrap_or_else(|| panic!("no PUT_ROUTES template matches {path}"))
        }

        let mut checked: Vec<&str> = Vec::new();

        // The sixteen upserts whose create body `round_trip_cases` supplies:
        // PUT the same body twice — 201 then 204.
        for case in round_trip_cases() {
            let pool = test_pool().await;
            seed_round_trip_fixtures(&pool).await;
            let client = ApiClient::over(router().with_state(pool));

            let created = client.put(case.put_path, &case.create).await;
            assert_eq!(
                created.status,
                StatusCode::CREATED,
                "PUT {} on a fresh id did not answer 201: {}",
                case.put_path,
                created.text()
            );
            let replaced = client.put(case.put_path, &case.create).await;
            assert_eq!(
                replaced.status,
                StatusCode::NO_CONTENT,
                "PUT {} replacing an existing row did not answer 204: {}",
                case.put_path,
                replaced.text()
            );
            checked.push(template_for(case.put_path));
        }

        // The tax-year settings upsert (no GET-one round-trip case, because it
        // is not one of the entities the read-body round trip covers).
        let pool = test_pool().await;
        let client = ApiClient::over(router().with_state(pool));
        let settings = serde_json::json!({ "ess_taxed_upfront_reduction_eligible": false });
        assert_eq!(
            client
                .put("/tax_year_settings/2026", &settings)
                .await
                .status,
            StatusCode::CREATED
        );
        assert_eq!(
            client
                .put("/tax_year_settings/2026", &settings)
                .await
                .status,
            StatusCode::NO_CONTENT
        );
        checked.push("/tax_year_settings/{tax_year}");

        // The manual closing price: 201 with the freshly stored row (the body
        // POST /closing_prices/fetch answers), then 204.
        let pool = test_pool().await;
        crate::test_support::listing(1).insert(&pool).await;
        let client = ApiClient::over(router().with_state(pool));
        let price = serde_json::json!({
            "price": "62.48",
            "sourced_from": "asx.com.au closing report",
            "reason": "provider serves no candle since the delisting",
        });
        let created = client.put("/closing_prices/1/2026-06-04", &price).await;
        assert_eq!(
            created.status,
            StatusCode::CREATED,
            "the first manual price did not answer 201: {}",
            created.text()
        );
        assert_eq!(
            created.json::<serde_json::Value>()["price"],
            "62.48",
            "the 201 must carry the stored row"
        );
        assert_eq!(
            client
                .put("/closing_prices/1/2026-06-04", &price)
                .await
                .status,
            StatusCode::NO_CONTENT
        );
        checked.push("/closing_prices/{listing_id}/{price_date}");

        // Exception 1: the RBA rate correction only ever replaces an existing
        // row, so it answers 204 — and a missing id is 404, never a create.
        let pool = test_pool().await;
        crate::entities::rba_fx_rate::db_import_rate(
            &pool,
            "USD",
            "2024-01",
            crate::test_support::dec("1.5"),
        )
        .await
        .unwrap();
        let id: i64 = sqlx::query_scalar("SELECT id FROM rba_fx_rates")
            .fetch_one(&pool)
            .await
            .unwrap();
        let client = ApiClient::over(router().with_state(pool));
        let correction = serde_json::json!({ "rate": "1.6" });
        assert_eq!(
            client
                .put(format!("/rba_fx_rates/{id}"), &correction)
                .await
                .status,
            StatusCode::NO_CONTENT
        );
        assert_eq!(
            client.put("/rba_fx_rates/9999", &correction).await.status,
            StatusCode::NOT_FOUND,
            "the correction cannot create a row"
        );

        // Exception 2: `PUT /transfers/{id}` is create-only and always 201
        // with the executed group — `transfer.rs`'s own tests cover the
        // execution and the refusal of a re-PUT (the group's trade ids are what
        // a 204 would hide), so only the classification is pinned here.

        // The classification is exhaustive: every route exercised above is a
        // CreateThenReplace entry, and the exceptions are the only others.
        let create_then_replace: Vec<&str> = PUT_ROUTES
            .iter()
            .filter(|(_, outcome)| *outcome == PutOutcome::CreateThenReplace)
            .map(|(path, _)| *path)
            .collect();
        let exceptions: Vec<&str> = PUT_ROUTES
            .iter()
            .filter(|(_, outcome)| *outcome != PutOutcome::CreateThenReplace)
            .map(|(path, _)| *path)
            .collect();
        checked.sort_unstable();
        let mut expected = create_then_replace.clone();
        expected.sort_unstable();
        assert_eq!(
            checked, expected,
            "every CreateThenReplace PUT route must be exercised here, and vice versa"
        );
        assert_eq!(
            exceptions,
            vec!["/transfers/{id}", "/rba_fx_rates/{id}"],
            "the only PUT routes that report a single outcome"
        );
        assert_eq!(
            PUT_ROUTES.len(),
            20,
            "the table must carry every PUT route entities::router serves"
        );
    }

    // -----------------------------------------------------------------------
    // Every entity list route is classified for filtering
    // -----------------------------------------------------------------------

    /// The value each filter is driven with — the shape the parameter takes,
    /// not a row that has to exist (every route answers an empty `200` for a
    /// filter that matches nothing).
    ///
    /// `route` is passed so a parameter whose valid values are route-specific
    /// can be classified per route and **fail closed**: `status` is driven at
    /// `/closing_prices` only, and a future list with its own `status` value set
    /// panics here until its sample is added rather than being handed `ok` and
    /// silently left untested.
    fn sample_filter_value(route: &str, param: &str) -> &'static str {
        match param {
            "from" | "to" => "2024-01-02",
            "exchange_mic" => "XASX",
            "security_type" => "Share",
            "include_linked" => "false",
            "status" => {
                assert_eq!(
                    route, "/closing_prices",
                    "a `status` filter on {route} needs its own sample value classified here"
                );
                "ok"
            }
            _ => "1",
        }
    }

    /// The `/<path>` literals `entities/` registers with the shared
    /// `http::list_handler`, read out of the sources rather than transcribed.
    ///
    /// The needle is the **handler name alone** (`list_handler::<`), not the
    /// fully-qualified `get(http::list_handler::<`: an entity that imports the
    /// handler (`use crate::infra::http::list_handler;`) and registers
    /// `get(list_handler::<X>)` is the same route written differently, and the
    /// longer needle made it invisible to the scan *and* absent from
    /// `LIST_ROUTES`, so the both-ways assertion passed while a list shipped
    /// unclassified. A comment line is skipped so a doc mention cannot be read
    /// as a registration.
    fn shared_list_route_paths() -> Vec<String> {
        let mut paths: Vec<String> = Vec::new();
        for (file, body) in crate::test_support::rust_sources() {
            // `entities/mod.rs` is this file — the scan's own needle appears in
            // it, and every router it merges is defined in the module beside it.
            if !file.starts_with("entities/") || file == "entities/mod.rs" {
                continue;
            }
            for (at, _) in body.match_indices("list_handler::<") {
                let before = &body[..at];
                let line_start = before.rfind('\n').map_or(0, |i| i + 1);
                if before[line_start..].trim_start().starts_with("//") {
                    continue;
                }
                let route_at = before
                    .rfind(".route(")
                    .expect("a list_handler is registered by a `.route(…)` call");
                let rest = before[route_at + ".route(".len()..].trim_start();
                let path = rest
                    .strip_prefix('"')
                    .expect("the route path is a string literal");
                paths.push(path.split('"').next().unwrap().to_string());
            }
        }
        paths.sort();
        paths.dedup();
        paths
    }

    /// Every entity list route is classified in [`LIST_ROUTES`], and every
    /// shared `list_handler` registration in the sources is one of them — so a
    /// new entity list cannot quietly ship without the filters its columns
    /// offer, or without a stated reason it takes none.
    #[test]
    fn every_list_route_is_classified_for_filtering() {
        let scanned = shared_list_route_paths();
        assert!(
            scanned.len() >= 20,
            "the scan found only {} shared list routes — it has stopped parsing",
            scanned.len()
        );

        let mut shared: Vec<String> = LIST_ROUTES
            .iter()
            .filter(|r| r.kind == ListKind::Shared)
            .map(|r| r.path.to_string())
            .collect();
        shared.sort();
        assert_eq!(
            shared, scanned,
            "every `http::list_handler` route in the sources must be classified in LIST_ROUTES \
             (and vice versa)"
        );

        let mut seen: Vec<&str> = LIST_ROUTES.iter().map(|r| r.path).collect();
        seen.sort_unstable();
        let before = seen.len();
        seen.dedup();
        assert_eq!(before, seen.len(), "LIST_ROUTES names a path twice");

        for route in LIST_ROUTES {
            if route.filters.is_empty() {
                assert!(
                    !route.reason.is_empty(),
                    "{} takes no filter and must say why in its reason",
                    route.path
                );
            } else {
                assert!(
                    route.reason.is_empty(),
                    "{} is filtered; drop its reason",
                    route.path
                );
            }
        }
    }

    /// Every entity list route the classification table knows is named in
    /// **exactly one** row of `docs/API.md`'s ordering table.
    ///
    /// The ordering pin in `doc_checks` asserts each row's endpoints were typed
    /// in as they are; nothing there reads `LIST_ROUTES`, so a new entity list
    /// shipped without any ordering row failed nothing — which is how
    /// `/drp_enrolments` came to be filed under a key it does not have. This
    /// closes the completeness half from the table's own source.
    #[test]
    fn every_classified_list_route_is_in_the_documented_ordering_table() {
        let md = include_str!("../../docs/API.md");
        let section = md
            .split("## Reading a list")
            .nth(1)
            .expect("docs/API.md has a Reading a list section")
            .split("\n## ")
            .next()
            .expect("split always yields at least one part");
        let rows: Vec<&str> = section
            .lines()
            .filter(|line| line.starts_with("| ascending") || line.starts_with("| **descending**"))
            .collect();

        for route in LIST_ROUTES {
            // A path-narrowed variant (`/exchange_holidays/{mic}`) is documented
            // with its parent collection and has no row of its own.
            if route.path.contains('{') {
                continue;
            }
            let needle = format!("`{}`", route.path);
            let count = rows.iter().filter(|row| row.contains(&needle)).count();
            assert_eq!(
                count, 1,
                "`{}` must appear in exactly one ordering-table row (found {count}): {rows:?}",
                route.path
            );
        }
        // The non-entity list surfaces the table also names, so a row dropped
        // for one of those fails here too.
        for path in ["/report_snapshots", "/jobs", "/reports/row_history"] {
            let needle = format!("`{path}`");
            let count = rows.iter().filter(|row| row.contains(&needle)).count();
            assert_eq!(
                count, 1,
                "`{path}` must appear in exactly one ordering-table row (found {count})"
            );
        }
    }

    /// Every classified list route that decodes a query string refuses an
    /// unrecognised parameter with a `400` naming it — the shared generic
    /// lists (filtered and `NoFilter` alike), the trades list, and the two
    /// hand-written filtered lists. An unfiltered `NoFilter` route is the
    /// point of the last case: the empty braced struct does decode the empty
    /// query string a bare `GET` sends, while any parameter at all is the
    /// unknown-field rejection.
    #[tokio::test]
    async fn every_list_route_refuses_an_unknown_parameter() {
        let pool = test_pool().await;
        let client = ApiClient::full(&pool);
        let mut checked = 0;
        for route in LIST_ROUTES {
            if route.kind == ListKind::HandWrittenIgnoringQuery || route.path.contains('{') {
                continue;
            }
            let resp = client.get(route.path).await;
            assert_eq!(
                resp.status,
                StatusCode::OK,
                "GET {} must list without a query string",
                route.path
            );
            for filter in route.filters {
                let query = format!(
                    "{}?{}={}",
                    route.path,
                    filter,
                    sample_filter_value(route.path, filter)
                );
                let resp = client.get(&query).await;
                assert_eq!(resp.status, StatusCode::OK, "GET {query}");
            }
            let query = format!("{}?zzz_unrecognised=1", route.path);
            let resp = client.get(&query).await;
            let (status, body) = resp.status_and_body();
            assert_eq!(
                status,
                StatusCode::BAD_REQUEST,
                "GET {query} must be refused, not ignored: {body}"
            );
            assert!(
                body.contains("zzz_unrecognised"),
                "GET {query} must name the parameter: {body}"
            );
            checked += 1;
        }
        assert_eq!(
            checked, 23,
            "every query-decoding list route must be driven here"
        );
    }
}
