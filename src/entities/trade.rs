//! The trade entity, split into focused units: the model and wire types
//! (`model`), the write-time invariant checks shared with the Sell path
//! (`checks`), settlement-date derivation (`settlement`), persistence and
//! its in-transaction invariants (`db`), and the HTTP handlers/router
//! (`http`). Everything is re-exported here, so the module's surface is
//! unchanged from when it was one file — and the tests (`tests/`, one file
//! per topic), which predate the split, are the behaviour lock proving it.

mod checks;
mod db;
mod http;
mod model;
mod new_parcel;
mod provenance;
mod settlement;

/// Referenced by name only from tests (its variants reach non-test callers
/// through `db_upsert`'s signature), so the re-export is test-gated to keep
/// the non-test build warning-free.
#[cfg(test)]
pub use db::UpsertError;
/// The unfiltered whole-table read: now that the route filters, only tests
/// name it, so it is re-exported only for them.
#[cfg(test)]
pub use db::db_list;
/// `db_upsert` writes a row's settlement provenance exactly as given, which
/// only the tests (and the fixture builders) need: the HTTP path resolves the
/// body's date on the write's own transaction
/// ([`db_upsert_resolving_settlement`]), so the raw writer is test-gated to
/// keep the non-test build warning-free.
#[cfg(test)]
pub use db::db_upsert;
pub use db::{
    DeleteOutcome, TradeListQuery, db_create, db_delete, db_get, db_list_filtered,
    db_upsert_resolving_settlement,
};
// The Sell path shares this DB-level rule, as it shares `check_amounts`.
pub(crate) use db::listing_currency_mismatch;
pub use http::router;
/// The provenance of a stored `settlement_date`. Named outside this module
/// only by tests (the write paths set it through [`Settlement`], and the
/// non-test build reaches the type through `Trade`'s own field), so the
/// re-export is test-gated to keep the non-test build warning-free.
#[cfg(test)]
pub use model::SettlementDateSource;
pub use model::{Trade, TradeBody, TradeType};
pub(crate) use new_parcel::{NewParcel, ParcelOrigin, Residuals};
/// The classified provenance-column list itself, named outside this module
/// only by `web::tests` (which pins the UI's `tradeOrigin` labelling against
/// it), so the re-export is test-gated to keep the non-test build
/// warning-free.
#[cfg(test)]
pub(crate) use provenance::TRADE_PROVENANCE;
/// The one place that answers "did an operation write this trade row?" — the
/// provenance-column list and the two SQL fragments over it, used by the
/// health report's write-path label and its nil-proceeds exclusion.
pub(crate) use provenance::{operation_written_sql, source_case_sql};

pub(crate) use checks::{
    AmountsCheck, AmountsError, CGT_START, SpotFxRateError, StatementTotalCheck,
    StatementTotalError, TradeAmounts, amounts_detail, check_amounts, check_statement_total,
    resolve_brokerage, spot_fx_rate_detail, statement_total_detail, validate_spot_fx_rate,
};
pub(crate) use settlement::Settlement;
/// Why a settlement date could not be derived (a checked date step that left
/// `NaiveDate`'s range, or a lookup failure). Named by the Sell path, which
/// wraps it in its own error enum, and by the tests.
pub(crate) use settlement::SettlementError;
/// Reached by name only from tests — the write paths go through
/// [`Settlement::resolve_on`], which is where the omitted-means-computed rule
/// lives — so the re-export is test-gated.
#[cfg(test)]
pub(crate) use settlement::auto_settlement_date;
/// A contemplated sale's settlement date, on the caller's connection — the
/// pre-sale what-if asks it for the register date of units not yet sold.
pub(crate) use settlement::auto_settlement_date_on;
/// The `settlement-recompute` maintenance job (SCENARIOS S-04), registered in
/// `infra::scheduler::registry` and deliberately unscheduled.
pub use settlement::run_recompute;

#[cfg(test)]
use crate::infra::id::HoldingAccountId;
#[cfg(test)]
use crate::infra::id::ListingId;
#[cfg(test)]
use crate::infra::id::TradeId;
#[cfg(test)]
use axum::http::StatusCode;
#[cfg(test)]
use checks::split_gst_inclusive;
#[cfg(test)]
use chrono::NaiveDate;
#[cfg(test)]
use settlement::add_business_days;
#[cfg(test)]
use sqlx::SqlitePool;

#[cfg(test)]
mod tests;
