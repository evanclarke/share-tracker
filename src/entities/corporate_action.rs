//! Corporate actions recorded against a listing.
//!
//! The action types modelled so far:
//!
//! **`ReturnOfCapital`** — a non-assessable payment from a company (a
//! shareholder-approved return of share capital, CGT event G1; see
//! `docs/ato/cgt-non-assessable-payments.md`). The per-unit payment reduces the
//! cost base of every parcel of the listing held on the payment date (units
//! sold before the payment were not held for it, so they are unaffected).
//! Where cumulative payments exceed a parcel's per-unit cost base, the cost
//! base floors at nil and the excess is an immediate capital gain in the
//! payment's income year — G1 can never produce a capital loss — computed by
//! the net-capital-gain report (`g1_gains`). Distinct from the AMIT
//! tax-deferred regime (CGT event E10, `amit_adjustment`), which applies to
//! trust units, not company shares.
//!
//! **`ShareSplit`** — a share split or consolidation (TD 2000/10; see
//! `docs/ato/share-splits-and-consolidations.md`): on the conversion date every
//! `split_old_units` units become `split_new_units` units (a 2-for-1 split is
//! new=2/old=1; a 1-for-10 consolidation is new=1/old=10). No CGT event
//! happens: the converted parcel keeps its total cost base and its original
//! acquisition date — only the unit count (and so the per-unit cost base)
//! changes. Trade rows keep the quantities as originally transacted; reports
//! and write-time allocation checks re-base quantities between unit bases via
//! [`split_adjusted_quantity`] / [`as_acquired_quantity`].
//! A trade dated on the conversion date is already in post-split units.
//!
//! **`BonusIssue`** — a non-assessable bonus share issue (the general
//! post-1 July 1998 case; see `docs/ato/bonus-shares.md`): on the issue date
//! every `bonus_held_units` units held receive `bonus_units` additional
//! units. The ATO apportions the parcel's cost base over original + bonus
//! shares and the bonus shares take the original acquisition date — exactly
//! the quantity re-base `(held + bonus) / held` with total cost base and
//! acquisition date preserved, so a `BonusIssue` folds into the split-event
//! stream as its equivalent split (new = held + bonus, old = held) and every
//! report and write-time check inherits the treatment. A trade dated on the
//! issue date is ex-bonus. (Bonus shares chosen *in lieu of a dividend* are
//! assessed as a dividend — entered as a DRP trade, not as this action.)
//!
//! **`RightsIssue`** — rights to acquire new shares, issued free to existing
//! holders (the dominant retail case; see `docs/ato/rights-issues.md`): on the
//! record `date` every `rights_held_units` units held entitle the holder to
//! acquire `rights_units` new units at `exercise_price` per unit in
//! `currency` (a trade dated on the record date is ex-rights). Recording the
//! action changes nothing by itself — free rights are non-assessable
//! non-exempt income on issue. Exercising it (`POST
//! /corporate_actions/:id/exercise`, `entities::rights_exercise`) creates a
//! new Buy parcel dated the exercise date — no CGT event, the 12-month
//! discount clock runs from exercise — whose cost base is the amount paid to
//! exercise plus any amount paid to acquire the rights. Cumulative exercised
//! units are capped at the entitlement, so an action referenced by exercise
//! trades cannot be edited or deleted (delete the exercise trades first).
//! Selling or letting the rights themselves lapse is not modelled.
//!
//! **`BuyBack`** — an off-market share buy-back (see `docs/ato/share-buy-backs.md`):
//! the company offers to buy back shares directly from holders. The action
//! records the offer terms: on/after the buy-back `date`, each unit bought
//! back is paid `buyback_price` in `currency`, of which `buyback_dividend` is
//! an assessable franked dividend carrying `buyback_franking_credit` (both 0
//! for a listed-company buy-back announced after 7:30 pm AEDT 25 Oct 2022 —
//! no dividend component), and `buyback_market_value` is the per-unit market
//! value had the buy-back not been proposed (capital proceeds can't be less
//! than it; `None` when the price is at or above market value). Recording the
//! action changes nothing by itself; participating (`POST
//! /corporate_actions/:id/participate`, `entities::buyback_participation`)
//! atomically creates the Sell trade — per-unit price = capital proceeds per
//! unit = `max(price, market value) − dividend` — with its parcel
//! allocations, plus the dividend-component income row when there is one.
//! An action referenced by participation trades is frozen against edits.
//!
//! **`ScripForScrip`** — a takeover or merger completed as a scrip exchange
//! with scrip-for-scrip rollover (Subdiv 124-M; see
//! `docs/ato/takeovers-and-scrip-for-scrip.md`): on the exchange `date` every
//! `scrip_old_units` units of the original (target) listing become
//! `scrip_new_units` units of `scrip_listing_id` (the replacement listing),
//! plus — the partial-rollover case (Example 27) — an optional
//! `scrip_cash_per_unit` cash per old unit (with `scrip_market_value`, the
//! replacement unit's market value just after issue, and
//! `scrip_cash_currency`; the three are all present or all absent).
//! Recording the action changes nothing by itself; exchanging (`POST
//! /corporate_actions/:id/exchange`, `entities::scrip_exchange`) atomically
//! creates a closing Sell on the original listing consuming every open
//! parcel — at zero proceeds and excluded from the realised-gains and
//! net-capital-gain reports when all-scrip (the rollover disregards the
//! capital gain); at the cash per-unit price when there is a cash component,
//! whose gain over the cash-apportioned cost base those reports assess now —
//! plus one replacement Buy per consumed parcel carrying the parcel's
//! remaining reduced cost base (the scrip-apportioned share when there is
//! cash) and (as `trades.deemed_acquisition_date`) its acquisition date, the
//! rollover's combined-holding-period rule for the 12-month CGT discount.
//! An action referenced by exchange trades is frozen against edits.
//!
//! **Demerger** — an eligible demerger with the Div 125 rollover chosen (see
//! `docs/ato/demergers.md`): on the demerger `date` every `demerger_held_units`
//! units held in the head entity (the action's own `listing_id`) receive
//! `demerger_new_units` units of `demerger_listing_id` (the demerged
//! entity's listing), and `demerger_cost_base_pct` percent of each parcel's
//! cost base is apportioned to the new interests (the head-entity-advised
//! percentage; the head parcels keep the rest). Recording the action changes
//! nothing by itself; demerging (`POST /corporate_actions/:id/demerge`,
//! `entities::demerger`) atomically closes every open head parcel with a
//! zero-proceeds Sell — excluded from the realised-gains and net-capital-gain
//! reports, because the rollover disregards any gain — and recreates it as a
//! head replacement Buy plus a demerged-entity Buy splitting the parcel's
//! remaining reduced cost base by the percentage, both carrying the parcel's
//! acquisition date as `trades.deemed_acquisition_date` (the head dates are
//! unchanged by law; the new interests' 12-month discount clock runs from the
//! original acquisition). An action referenced by demerge trades is frozen
//! against edits.
//!
//! **`WorthlessShares`** — a capital loss on a failed company without an
//! ordinary sale (CGT events G3 and C2; see `docs/ato/worthless-shares.md`):
//! the action records, against the failed listing, which event the user is
//! invoking (`worthless_event`: `G3Declaration` — a liquidator's/administrator's
//! written declaration of no-likely-distribution, s 104-145/TD 2000/52; or
//! `C2Cancellation` — deregistration cancelling the shares, s 104-25/TD
//! 2000/7), the `date` being the declaration or cancellation date. Recording
//! the action changes nothing by itself; recognising (`POST
//! /corporate_actions/:id/recognise`, `entities::worthless`) atomically closes
//! every open parcel held at the event date through a provenance-marked Sell at
//! **nil proceeds**, each parcel producing a capital loss equal to its
//! remaining reduced cost base. Unlike the scrip-for-scrip and demerger closing
//! Sells, this Sell is **not** excluded from the realised-gains and
//! net-capital-gain reports — its nil proceeds against the cost base
//! *recognise* the loss (never income, never discounted). An action referenced
//! by a recognise Sell is frozen against edits.
//!
//! `ActionKind` is the extension point for future corporate actions, each
//! widening the enum and its CHECK.

mod db;
mod http;
mod model;

/// The cost-base/quantity adjustment events derived from corporate actions
/// live in `domain::adjustments` — the cost-base pipeline reads them, and
/// `domain` depends on nothing in `entities` — and are re-exported here so the
/// entity's callers keep one path to everything about a corporate action.
pub use crate::domain::adjustments::{
    AcquiredUnits, PriceBasisEvent, RocEvent, SplitEvent, UnitsAt, acquired_units_of_sales,
    as_acquired_quantity, checked_as_acquired_quantity, contemporaneous_price,
    db_demerger_price_statements, db_payment_currency_conflict, db_return_of_capital_events,
    db_share_split_events, db_splits_for_listing, sold_in_acquired_units, split_adjusted_quantity,
};
/// The list route's filter type — `api_spec` reads its fields through
/// `utoipa::IntoParams` so the OpenAPI document's query parameters come from
/// what axum decodes rather than from prose.
pub use db::{CorporateActionListQuery, db_get_tx, rebased_quantity_beyond_range};
pub use http::router;
pub use model::{
    ActionKind, CorporateAction, CorporateActionBody, NOTHING_PAID_FOR_NON_RENOUNCEABLE_RIGHTS,
    WorthlessEvent,
};

/// Referenced by name only from other modules' tests (production code calls
/// these through the HTTP routes in `http.rs`, not this re-export), so the
/// re-export is test-gated to keep the non-test build warning-free — same
/// reasoning as `trade.rs`'s `UpsertError` re-export.
#[cfg(test)]
pub use db::{DeleteError, WriteError, db_delete, db_get, db_list, db_upsert};

/// The raw conversion ratio is now used only *inside* `adjustments` — every
/// caller outside it asks a higher-level question instead
/// ([`split_adjusted_quantity`] / [`as_acquired_quantity`] for a quantity,
/// [`RocEvent::per_unit_for`] for a payment) — so the re-export is test-gated
/// for its own unit tests, keeping the non-test build warning-free.
#[cfg(test)]
pub use crate::domain::adjustments::split_ratio;

#[cfg(test)]
use crate::infra::http::Upsert;
#[cfg(test)]
use crate::infra::id::HoldingAccountId;
#[cfg(test)]
use crate::infra::id::ListingId;
#[cfg(test)]
use crate::infra::id::TradeId;
#[cfg(test)]
use axum::http::StatusCode;
#[cfg(test)]
use chrono::NaiveDate;
#[cfg(test)]
use rust_decimal::Decimal;
#[cfg(test)]
use sqlx::SqlitePool;

#[cfg(test)]
mod tests;
