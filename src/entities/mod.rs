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
}
