//! Read-only reports over the entity tables: AUD-denominated aggregations
//! (portfolio, realised/unrealised gains, tax summary) plus reference-data
//! validation (exchange MIC validation, settlement-holiday coverage). The one
//! exception to "no writes" is `snapshot`, which persists the price-dependent
//! reports' daily results to `report_snapshots`.
use axum::Router;
use sqlx::SqlitePool;

/// Taxpayer assumption stated on every tax-report row: the rates are hard-wired
/// for an Australian-resident *individual* — the 50% CGT discount and the 50%
/// LIC capital gain deduction. Other entity types (SMSF/complying super 33⅓%,
/// company 0%, trust/partnership flow-through) are deliberately not modelled
/// (scope decision, 2026-06-07). Kept comma-free so it stays a single CSV field.
pub const TAXPAYER_BASIS: &str =
    "individual resident: 50% CGT discount; 50% LIC deduction";

pub mod export;
pub mod franking;
pub mod mic_validation;
pub mod net_capital_gain;
pub mod open_parcels;
pub mod performance;
pub mod portfolio;
pub mod realised_gains;
pub mod settlement_coverage;
pub mod snapshot;
pub mod tax_summary;
pub mod unrealised_gains;

/// Merge every report's routes into a single router.
pub fn router() -> Router<SqlitePool> {
    portfolio::router()
        .merge(open_parcels::router())
        .merge(performance::router())
        .merge(unrealised_gains::router())
        .merge(realised_gains::router())
        .merge(net_capital_gain::router())
        .merge(tax_summary::router())
        .merge(mic_validation::router())
        .merge(settlement_coverage::router())
        .merge(snapshot::router())
}
