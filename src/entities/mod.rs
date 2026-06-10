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
pub mod investment_expense;
pub mod listing;
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
        .merge(holding_account::router())
        .merge(currencies::router())
        .merge(mic_registry::router())
        .merge(rba_fx_rate::router())
        .merge(trade::router())
        .merge(income::router())
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
