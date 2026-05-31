//! Read-only reports: AUD-denominated aggregations over the entity tables
//! (portfolio, realised/unrealised gains, tax summary). No writes.
use axum::Router;
use sqlx::SqlitePool;

pub mod portfolio;
pub mod realised_gains;
pub mod tax_summary;
pub mod unrealised_gains;

/// Merge every report's routes into a single router.
pub fn router() -> Router<SqlitePool> {
    portfolio::router()
        .merge(unrealised_gains::router())
        .merge(realised_gains::router())
        .merge(tax_summary::router())
}
