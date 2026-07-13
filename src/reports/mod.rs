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
pub const TAXPAYER_BASIS: &str = "individual resident: 50% CGT discount; 50% LIC deduction";

pub mod activity;
pub mod amit_cash_cross_check;
pub mod e4_cross_check;
pub mod export;
pub mod franking;
pub mod franking_at_risk;
pub mod mic_validation;
pub mod net_capital_gain;
pub mod open_parcels;
pub mod parcel_optimiser;
pub mod performance;
pub mod portfolio;
pub mod realised_gains;
pub mod settlement_coverage;
pub mod snapshot;
pub mod tax_summary;
pub mod unrealised_gains;
pub mod wash_sales;

/// A listing's ticker for a rejection or detail message — the error-bodies
/// contract (API.md) names entities by ticker/name, never by raw foreign-key
/// id. Falls back to `listing <id>` when no such row exists (e.g. an unknown
/// id straight from the request).
pub(crate) async fn listing_label(
    pool: &SqlitePool,
    listing_id: i64,
) -> Result<String, sqlx::Error> {
    let ticker: Option<String> = sqlx::query_scalar("SELECT ticker FROM listings WHERE id = ?")
        .bind(listing_id)
        .fetch_optional(pool)
        .await?;
    Ok(ticker.unwrap_or_else(|| format!("listing {listing_id}")))
}

/// A holding account's quoted name for a message (`account 'Default'`),
/// falling back to `account <id>` when no such row exists.
pub(crate) async fn account_label(
    pool: &SqlitePool,
    account_id: i64,
) -> Result<String, sqlx::Error> {
    let name: Option<String> = sqlx::query_scalar("SELECT name FROM holding_accounts WHERE id = ?")
        .bind(account_id)
        .fetch_optional(pool)
        .await?;
    Ok(name.map_or_else(
        || format!("account {account_id}"),
        |n| format!("account '{n}'"),
    ))
}

/// Merge every report's routes into a single router.
pub fn router() -> Router<SqlitePool> {
    portfolio::router()
        .merge(activity::router())
        .merge(open_parcels::router())
        .merge(performance::router())
        .merge(unrealised_gains::router())
        .merge(realised_gains::router())
        .merge(net_capital_gain::router())
        .merge(parcel_optimiser::router())
        .merge(tax_summary::router())
        .merge(mic_validation::router())
        .merge(settlement_coverage::router())
        .merge(e4_cross_check::router())
        .merge(amit_cash_cross_check::router())
        .merge(wash_sales::router())
        .merge(franking_at_risk::router())
        .merge(snapshot::router())
}
