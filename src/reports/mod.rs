//! Read-only reports over the entity tables: AUD-denominated aggregations
//! (portfolio, realised/unrealised gains, tax summary) plus reference-data
//! validation (exchange MIC validation). No writes.
use axum::Router;
use sqlx::SqlitePool;

pub mod franking;
pub mod mic_validation;
pub mod net_capital_gain;
pub mod portfolio;
pub mod realised_gains;
pub mod tax_summary;
pub mod unrealised_gains;

/// Merge every report's routes into a single router.
pub fn router() -> Router<SqlitePool> {
    portfolio::router()
        .merge(unrealised_gains::router())
        .merge(realised_gains::router())
        .merge(net_capital_gain::router())
        .merge(tax_summary::router())
        .merge(mic_validation::router())
}
