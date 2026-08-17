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
pub mod rba_fx_rate;
pub mod rights_exercise;
pub mod rights_sale;
pub mod scrip_exchange;
pub mod sell;
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
        .merge(closing_price::router())
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
mod tests {
    use super::*;
    use crate::test_support::{ApiClient, test_pool};
    use axum::http::StatusCode;

    /// Every entity DELETE route, with the noun its 404 body must name.
    ///
    /// A delete is fired from a list row, so its failure only ever reaches the
    /// user as a toast — an empty-bodied 404 shows as a bare "HTTP 404". The
    /// contract had drifted three ways (eight routes returned a bare
    /// `StatusCode::NOT_FOUND`) before `infra::http::deleted` and the
    /// `CrudEntity` delete handler made one wording the default; this table
    /// keeps a new entity from drifting again.
    const DELETE_ROUTES: &[(&str, &str)] = &[
        ("/amma_statements/9999", "AMMA statement"),
        ("/amit_adjustments/9999", "AMIT adjustment"),
        ("/attachments/9999", "attachment"),
        ("/cgt_settings/9999", "CGT settings row"),
        ("/closing_prices/9999/2024-01-02", "stored price"),
        ("/corporate_actions/9999", "corporate action"),
        ("/drp_enrolments/9999", "DRP enrolment"),
        ("/ess_statements/9999", "ESS statement"),
        ("/exchanges/ZZZZ", "exchange"),
        ("/exchange_holidays/ZZZZ/2024-01-02", "exchange holiday"),
        ("/holding_accounts/9999", "holding account"),
        ("/income/9999", "income"),
        ("/income/9999/reinvest", "distribution"),
        ("/inheritances/9999", "inheritance"),
        ("/interest_income/9999", "interest income"),
        ("/investment_expenses/9999", "investment expense"),
        ("/listings/9999", "listing"),
        ("/rights_sales/9999", "rights sale"),
        ("/sells/9999", "sell"),
        ("/trades/9999", "trade"),
        ("/transfers/9999", "transfer"),
    ];

    #[tokio::test]
    async fn deleting_a_missing_row_is_404_naming_what_was_missing() {
        for (uri, noun) in DELETE_ROUTES {
            let pool = test_pool().await;
            let resp = ApiClient::over(router().with_state(pool))
                .delete(*uri)
                .await;
            assert_eq!(resp.status, StatusCode::NOT_FOUND, "DELETE {uri}");
            let body = resp.text().to_string();
            assert!(
                body.contains(noun),
                "DELETE {uri} answered 404 with a body that does not name the missing \
                 {noun}: {body:?}"
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
    }

    impl RoundTrip {
        fn new(put_path: &'static str, create: serde_json::Value) -> Self {
            Self {
                put_path,
                get_path: None,
                create,
                graft: serde_json::Value::Null,
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
            ),
            RoundTrip::new(
                "/exchange_holidays/XASX/2024-12-27",
                json!({ "name": "Test Holiday" }),
            ),
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
            })),
        ]
    }

    /// What a GET hands back must be exactly what a PUT accepts, and storing
    /// it again must not move it.
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
                StatusCode::NO_CONTENT,
                "PUT {} did not create the row: {}",
                case.put_path,
                created.text()
            );
            let first: serde_json::Value = client.get_json(get_path).await;

            let mut replay = first.clone();
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
}
