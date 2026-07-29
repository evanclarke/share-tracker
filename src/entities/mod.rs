//! Domain entities: each module owns one table's model, CRUD endpoints, and
//! write-time invariants. Add a new entity by dropping a file here and adding
//! one `pub mod` line plus one `.merge` below — `main.rs` never changes.
use axum::Router;
use sqlx::SqlitePool;

pub mod amit_adjustment;
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
    use crate::test_support::test_pool;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use http_body_util::BodyExt;
    use tower::ServiceExt;

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
            let resp = router()
                .with_state(pool)
                .oneshot(
                    Request::builder()
                        .method("DELETE")
                        .uri(*uri)
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(resp.status(), StatusCode::NOT_FOUND, "DELETE {uri}");
            let body = resp.into_body().collect().await.unwrap().to_bytes();
            let body = String::from_utf8(body.to_vec()).unwrap();
            assert!(
                body.contains(noun),
                "DELETE {uri} answered 404 with a body that does not name the missing \
                 {noun}: {body:?}"
            );
        }
    }
}
