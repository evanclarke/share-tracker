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
pub mod corporate_action;
pub mod currencies;
pub mod drp_enrolment;
pub mod drp_reinvestment;
pub mod exchange;
pub mod exchange_holiday;
pub mod income;
pub mod listing;
pub mod mic_registry;
pub mod parcel_allocation;
pub mod rba_fx_rate;
pub mod rights_exercise;
pub mod sell;
pub mod trade;

/// Merge every entity's routes into a single router.
pub fn router() -> Router<SqlitePool> {
    exchange::router()
        .merge(exchange_holiday::router())
        .merge(listing::router())
        .merge(currencies::router())
        .merge(mic_registry::router())
        .merge(rba_fx_rate::router())
        .merge(trade::router())
        .merge(income::router())
        .merge(amma::router())
        .merge(parcel_allocation::router())
        .merge(sell::router())
        .merge(amit_adjustment::router())
        .merge(drp_enrolment::router())
        .merge(cgt_settings::router())
        .merge(corporate_action::router())
        .merge(rights_exercise::router())
        .merge(buyback_participation::router())
        .merge(drp_reinvestment::router())
        .merge(attachment::router())
}
