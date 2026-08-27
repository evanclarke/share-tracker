//! The stored row and its two enums: what a `closing_prices` row is, and the
//! two closed sets it is constrained to — `status` (a price or a recorded
//! fetch failure) and `origin` (fetched from the provider, or entered by
//! hand for a day the provider cannot serve).

use crate::infra::decimal::OptMoney;
use chrono::NaiveDate;
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};

/// Whether a stored row carries a price or a fetch failure.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, sqlx::Type)]
#[sqlx(rename_all = "lowercase")]
#[serde(rename_all = "lowercase")]
pub enum PriceStatus {
    Ok,
    Error,
}

/// How a stored row came to be: fetched from the provider, or entered by hand
/// for a day the provider cannot serve.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, sqlx::Type)]
#[sqlx(rename_all = "lowercase")]
#[serde(rename_all = "lowercase")]
pub enum PriceOrigin {
    Fetched,
    Manual,
}

/// The `source` of a manually entered row — the provider slot, held in step
/// with `origin = "manual"` by a schema CHECK (0020).
pub const MANUAL_SOURCE: &str = "manual";

/// The [`ClosingPrice::id`] of a row built to be written: the surrogate key is
/// server-assigned, so [`db_store`] ignores the value and lets the database
/// assign a new id (or preserve the stored row's, on an upsert that updates).
pub const UNASSIGNED_ID: i64 = 0;

/// One stored closing price — or one recorded fetch failure (`status =
/// "error"`, `price` null, `error` set).
#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct ClosingPrice {
    /// Server-assigned surrogate key (0021): the row's identity for the audit
    /// trail (`row_history.row_id`, so `POST /reports/row_history` can be
    /// keyed on it). Writes address a row by its `(listing_id, price_date)`
    /// natural key, never by this — [`db_store`] ignores the value it is
    /// handed and lets the database assign or preserve it.
    #[serde(default)]
    pub id: i64,
    pub listing_id: i64,
    pub price_date: NaiveDate,
    /// Closing price in the listing's quote currency, in the unit basis in
    /// force on [`Self::price_date`]; None exactly when the fetch failed.
    #[sqlx(try_from = "OptMoney")]
    pub price: Option<Decimal>,
    /// The figure exactly as the provider served it (or as the operator
    /// entered it), in the unit basis in force when it was observed — which
    /// [`Self::fetched_at`] dates. [`Self::price`] is derived from it by the
    /// re-basing actions dated in `(price_date, fetched_at]`, so a split
    /// recorded, edited or deleted later restates the price from here rather
    /// than from itself (see the module docs). Equal to `price` for a manual
    /// row, and None exactly when the fetch failed.
    #[sqlx(try_from = "OptMoney")]
    pub price_as_observed: Option<Decimal>,
    /// Provider that produced the row, e.g. "yahoo" — [`MANUAL_SOURCE`]
    /// exactly when `origin` is `Manual`.
    pub source: String,
    /// RFC 3339 UTC timestamp of the fetch that produced the row — for a
    /// manual row, of the entry that recorded it.
    pub fetched_at: String,
    /// The provider symbol this row was fetched under (migration 0038), in
    /// the namespace of [`Self::source`] — recorded on every fetched row, so
    /// a backfill made with the one-off `symbol` override is afterwards
    /// distinguishable from an ordinary fetch. Informational: no calculation
    /// reads it; it is provenance, served by `GET /closing_prices`, shown on
    /// the Closing Prices screen and carried into `row_history`.
    ///
    /// None for a manual row (nothing was fetched — a schema CHECK pairs the
    /// two), for a row stored before 0038 (unrecorded, and not recoverable
    /// after the fact), and for the errored row a fetch stores when no symbol
    /// could be resolved at all — an exchange with no provider mapping, which
    /// the row's own `error` names.
    pub fetched_symbol: Option<String>,
    pub status: PriceStatus,
    /// Failure detail; None exactly when the fetch succeeded.
    pub error: Option<String>,
    pub origin: PriceOrigin,
    /// Where a manual price was sourced from; None exactly when `origin` is
    /// `Fetched`.
    pub sourced_from: Option<String>,
    /// Why manual entry was needed; None exactly when `origin` is `Fetched`.
    pub reason: Option<String>,
}
