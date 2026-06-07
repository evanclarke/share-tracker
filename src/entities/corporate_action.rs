//! Corporate actions recorded against a listing.
//!
//! Three action types are modelled so far:
//!
//! **ReturnOfCapital** — a non-assessable payment from a company (a
//! shareholder-approved return of share capital, CGT event G1; see
//! `docs/cgt-non-assessable-payments.md`). The per-unit payment reduces the
//! cost base of every parcel of the listing held on the payment date (units
//! sold before the payment were not held for it, so they are unaffected).
//! Where cumulative payments exceed a parcel's per-unit cost base, the cost
//! base floors at nil and the excess is an immediate capital gain in the
//! payment's income year — G1 can never produce a capital loss — computed by
//! the net-capital-gain report (`g1_gains`). Distinct from the AMIT
//! tax-deferred regime (CGT event E10, `amit_adjustment`), which applies to
//! trust units, not company shares.
//!
//! **ShareSplit** — a share split or consolidation (TD 2000/10; see
//! `docs/share-splits-and-consolidations.md`): on the conversion date every
//! `split_old_units` units become `split_new_units` units (a 2-for-1 split is
//! new=2/old=1; a 1-for-10 consolidation is new=1/old=10). No CGT event
//! happens: the converted parcel keeps its total cost base and its original
//! acquisition date — only the unit count (and so the per-unit cost base)
//! changes. Trade rows keep the quantities as originally transacted; reports
//! and write-time allocation checks re-base quantities between unit bases via
//! [`split_ratio`] / [`split_adjusted_quantity`] / [`as_acquired_quantity`].
//! A trade dated on the conversion date is already in post-split units.
//!
//! **BonusIssue** — a non-assessable bonus share issue (the general
//! post-1 July 1998 case; see `docs/bonus-shares.md`): on the issue date
//! every `bonus_held_units` units held receive `bonus_units` additional
//! units. The ATO apportions the parcel's cost base over original + bonus
//! shares and the bonus shares take the original acquisition date — exactly
//! the quantity re-base `(held + bonus) / held` with total cost base and
//! acquisition date preserved, so a BonusIssue folds into the split-event
//! stream as its equivalent split (new = held + bonus, old = held) and every
//! report and write-time check inherits the treatment. A trade dated on the
//! issue date is ex-bonus. (Bonus shares chosen *in lieu of a dividend* are
//! assessed as a dividend — entered as a DRP trade, not as this action.)
//!
//! **RightsIssue** — rights to acquire new shares, issued free to existing
//! holders (the dominant retail case; see `docs/rights-issues.md`): on the
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
//! **BuyBack** — an off-market share buy-back (see `docs/share-buy-backs.md`):
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
//! **ScripForScrip** — a takeover or merger completed as an all-scrip
//! exchange with scrip-for-scrip rollover (Subdiv 124-M; see
//! `docs/takeovers-and-scrip-for-scrip.md`): on the exchange `date` every
//! `scrip_old_units` units of the original (target) listing become
//! `scrip_new_units` units of `scrip_listing_id` (the replacement listing).
//! Recording the action changes nothing by itself; exchanging (`POST
//! /corporate_actions/:id/exchange`, `entities::scrip_exchange`) atomically
//! creates a closing Sell on the original listing consuming every open
//! parcel — excluded from the realised-gains and net-capital-gain reports,
//! because the rollover disregards the capital gain — plus one replacement
//! Buy per consumed parcel carrying the parcel's remaining reduced cost base
//! and (as `trades.deemed_acquisition_date`) its acquisition date, the
//! rollover's combined-holding-period rule for the 12-month CGT discount.
//! An action referenced by exchange trades is frozen against edits.
//!
//! **Demerger** — an eligible demerger with the Div 125 rollover chosen (see
//! `docs/demergers.md`): on the demerger `date` every `demerger_held_units`
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
//! `ActionKind` is the extension point for future corporate actions, each
//! widening the enum and its CHECK.

use crate::infra::decimal::parse_dec;
use crate::infra::http::write_error_status;
use axum::{
    Json, Router,
    extract::{Path, State},
    http::StatusCode,
    routing::get,
};
use chrono::NaiveDate;
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use sqlx::{Row, SqlitePool};
use std::collections::HashMap;

/// The per-type payload of a corporate action. Each variant carries exactly
/// the fields its action type needs, so a mixed or partial payload (the
/// states the table CHECKs reject) is unrepresentable once constructed.
/// Internally tagged on `action_type` and flattened into [`CorporateAction`],
/// so the JSON wire shape stays flat: the tag plus the variant's own fields.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "action_type")]
pub enum ActionKind {
    ReturnOfCapital {
        /// Per-unit payment amount in `currency` (positive).
        amount_per_unit: Decimal,
        currency: String,
    },
    ShareSplit {
        /// Every `split_old_units` existing units become `split_new_units`
        /// units on the conversion date (both positive; a consolidation has
        /// new < old).
        split_new_units: Decimal,
        split_old_units: Decimal,
    },
    BonusIssue {
        /// Every `bonus_held_units` units held receive `bonus_units`
        /// additional units on the issue date (both positive; a 1-for-10
        /// bonus issue is bonus_units=1 / bonus_held_units=10).
        bonus_units: Decimal,
        bonus_held_units: Decimal,
    },
    RightsIssue {
        /// Every `rights_held_units` units held at the record date entitle
        /// the holder to acquire `rights_units` new units (both positive; a
        /// 1-for-4 rights issue is rights_units=1 / rights_held_units=4).
        rights_units: Decimal,
        rights_held_units: Decimal,
        /// Per-new-unit price paid on exercise, in `currency` (positive).
        exercise_price: Decimal,
        currency: String,
    },
    BuyBack {
        /// Per-unit buy-back price in `currency` (positive).
        buyback_price: Decimal,
        /// Per-unit dividend component of the price (non-negative, ≤ the
        /// price; 0 for a listed-company buy-back announced after
        /// 25 Oct 2022). Assessable income, excluded from capital proceeds.
        buyback_dividend: Decimal,
        /// Per-unit franking credit attached to the dividend component
        /// (non-negative; 0 when there is no dividend component).
        buyback_franking_credit: Decimal,
        /// Per-unit market value had the buy-back not been proposed
        /// (positive). Capital proceeds can't be less than it; `None` when
        /// the buy-back price is at or above market value (the price is used).
        buyback_market_value: Option<Decimal>,
        currency: String,
    },
    ScripForScrip {
        /// The replacement listing the original holding converts into (must
        /// differ from the action's own `listing_id`).
        scrip_listing_id: i64,
        /// Every `scrip_old_units` units of the original listing held at the
        /// exchange date become `scrip_new_units` units of the replacement
        /// listing (both positive).
        scrip_new_units: Decimal,
        scrip_old_units: Decimal,
    },
    Demerger {
        /// The demerged entity's listing (must differ from the action's own
        /// `listing_id`, the head entity).
        demerger_listing_id: i64,
        /// Every `demerger_held_units` units of the head entity held at the
        /// demerger date receive `demerger_new_units` units of the demerged
        /// entity (both positive; BHP Billiton's 1-for-5 demerger of BHP
        /// Steel is new=1 / held=5).
        demerger_new_units: Decimal,
        demerger_held_units: Decimal,
        /// Percentage of each parcel's cost base apportioned to the new
        /// interests in the demerged entity (0 < pct < 100; the
        /// head-entity-advised step 2 percentage — e.g. 5.063 for BHP
        /// Steel). The head parcels keep the remaining `100 − pct` percent.
        demerger_cost_base_pct: Decimal,
    },
}

impl ActionKind {
    /// The `action_type` column value (the serde tag).
    fn type_str(&self) -> &'static str {
        match self {
            ActionKind::ReturnOfCapital { .. } => "ReturnOfCapital",
            ActionKind::ShareSplit { .. } => "ShareSplit",
            ActionKind::BonusIssue { .. } => "BonusIssue",
            ActionKind::RightsIssue { .. } => "RightsIssue",
            ActionKind::BuyBack { .. } => "BuyBack",
            ActionKind::ScripForScrip { .. } => "ScripForScrip",
            ActionKind::Demerger { .. } => "Demerger",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CorporateAction {
    pub id: i64,
    pub listing_id: i64,
    /// ReturnOfCapital: payment date — parcels acquired on/before it are
    /// affected. ShareSplit: conversion date — parcels acquired before it are
    /// converted (a trade dated on the conversion date is already in
    /// post-split units). BonusIssue: issue date — parcels acquired before it
    /// receive bonus units (a trade dated on the issue date is ex-bonus).
    /// RightsIssue: record date — units held before it earn the entitlement
    /// (a trade dated on it is ex-rights); exercises are dated on/after it.
    /// ScripForScrip: exchange date — every parcel still open on it is
    /// exchanged; the closing Sell and replacement Buys are dated on it.
    /// Demerger: demerger date — every head parcel still open on it
    /// participates; the closing Sell and the head/demerged Buys are dated
    /// on it.
    pub date: NaiveDate,
    #[serde(flatten)]
    pub kind: ActionKind,
}

/// A required `TEXT` decimal column: NULL or an unparsable value is a Decode
/// error — the table CHECKs guarantee the column is set for the row's type.
fn req_dec(row: &sqlx::sqlite::SqliteRow, column: &str) -> Result<Decimal, sqlx::Error> {
    parse_dec(column, row.try_get(column)?)
}

/// An optional `TEXT` decimal column: NULL is `None`, an unparsable value is
/// a Decode error.
fn opt_dec(row: &sqlx::sqlite::SqliteRow, column: &str) -> Result<Option<Decimal>, sqlx::Error> {
    match row.try_get::<Option<String>, _>(column)? {
        Some(s) => parse_dec(column, s).map(Some),
        None => Ok(None),
    }
}

fn kind_from_row(row: &sqlx::sqlite::SqliteRow) -> Result<ActionKind, sqlx::Error> {
    match row.try_get::<String, _>("action_type")?.as_str() {
        "ReturnOfCapital" => Ok(ActionKind::ReturnOfCapital {
            amount_per_unit: req_dec(row, "amount_per_unit")?,
            currency: row.try_get("currency")?,
        }),
        "ShareSplit" => Ok(ActionKind::ShareSplit {
            split_new_units: req_dec(row, "split_new_units")?,
            split_old_units: req_dec(row, "split_old_units")?,
        }),
        "BonusIssue" => Ok(ActionKind::BonusIssue {
            bonus_units: req_dec(row, "bonus_units")?,
            bonus_held_units: req_dec(row, "bonus_held_units")?,
        }),
        "RightsIssue" => Ok(ActionKind::RightsIssue {
            rights_units: req_dec(row, "rights_units")?,
            rights_held_units: req_dec(row, "rights_held_units")?,
            exercise_price: req_dec(row, "exercise_price")?,
            currency: row.try_get("currency")?,
        }),
        "BuyBack" => Ok(ActionKind::BuyBack {
            buyback_price: req_dec(row, "buyback_price")?,
            buyback_dividend: req_dec(row, "buyback_dividend")?,
            buyback_franking_credit: req_dec(row, "buyback_franking_credit")?,
            buyback_market_value: opt_dec(row, "buyback_market_value")?,
            currency: row.try_get("currency")?,
        }),
        "ScripForScrip" => Ok(ActionKind::ScripForScrip {
            scrip_listing_id: row.try_get("scrip_listing_id")?,
            scrip_new_units: req_dec(row, "scrip_new_units")?,
            scrip_old_units: req_dec(row, "scrip_old_units")?,
        }),
        "Demerger" => Ok(ActionKind::Demerger {
            demerger_listing_id: row.try_get("demerger_listing_id")?,
            demerger_new_units: req_dec(row, "demerger_new_units")?,
            demerger_held_units: req_dec(row, "demerger_held_units")?,
            demerger_cost_base_pct: req_dec(row, "demerger_cost_base_pct")?,
        }),
        other => Err(sqlx::Error::Decode(
            format!("unknown corporate action_type {other}").into(),
        )),
    }
}

impl<'r> sqlx::FromRow<'r, sqlx::sqlite::SqliteRow> for CorporateAction {
    fn from_row(row: &'r sqlx::sqlite::SqliteRow) -> Result<Self, sqlx::Error> {
        Ok(CorporateAction {
            id: row.try_get("id")?,
            listing_id: row.try_get("listing_id")?,
            date: row.try_get("date")?,
            kind: kind_from_row(row)?,
        })
    }
}

/// The PUT body's `action_type` tag. Deserialized separately from the
/// payload fields (unlike [`ActionKind`]) so a stray cross-type field is
/// *rejected*, not silently dropped as serde's tagged-enum decoding would.
#[derive(Debug, Clone, Copy, Deserialize)]
enum ActionType {
    ReturnOfCapital,
    ShareSplit,
    BonusIssue,
    RightsIssue,
    BuyBack,
    ScripForScrip,
    Demerger,
}

#[derive(Debug, Deserialize)]
pub struct CorporateActionBody {
    action_type: ActionType,
    pub listing_id: i64,
    pub date: NaiveDate,
    #[serde(default)]
    amount_per_unit: Option<Decimal>,
    #[serde(default)]
    currency: Option<String>,
    #[serde(default)]
    split_new_units: Option<Decimal>,
    #[serde(default)]
    split_old_units: Option<Decimal>,
    #[serde(default)]
    bonus_units: Option<Decimal>,
    #[serde(default)]
    bonus_held_units: Option<Decimal>,
    #[serde(default)]
    rights_units: Option<Decimal>,
    #[serde(default)]
    rights_held_units: Option<Decimal>,
    #[serde(default)]
    exercise_price: Option<Decimal>,
    #[serde(default)]
    buyback_price: Option<Decimal>,
    #[serde(default)]
    buyback_dividend: Option<Decimal>,
    #[serde(default)]
    buyback_franking_credit: Option<Decimal>,
    #[serde(default)]
    buyback_market_value: Option<Decimal>,
    #[serde(default)]
    scrip_listing_id: Option<i64>,
    #[serde(default)]
    scrip_new_units: Option<Decimal>,
    #[serde(default)]
    scrip_old_units: Option<Decimal>,
    #[serde(default)]
    demerger_listing_id: Option<i64>,
    #[serde(default)]
    demerger_new_units: Option<Decimal>,
    #[serde(default)]
    demerger_held_units: Option<Decimal>,
    #[serde(default)]
    demerger_cost_base_pct: Option<Decimal>,
}

impl CorporateActionBody {
    /// Each action type carries exactly its own payload (mirrors the table
    /// CHECKs, plus positivity): ReturnOfCapital needs a positive payment and
    /// a currency; ShareSplit a positive conversion ratio; BonusIssue a
    /// positive bonus ratio; RightsIssue a positive entitlement ratio,
    /// exercise price, and a currency; BuyBack a positive per-unit price and
    /// a currency (dividend/franking-credit components default to 0; the
    /// dividend may not exceed the price — it is part of it — and a credit
    /// needs a dividend to attach to; market value, when given, is positive)
    /// — each with every other type's fields absent (`None` otherwise;
    /// `currency` is shared by ReturnOfCapital, RightsIssue, and BuyBack but
    /// forbidden for the ratio-only types). A zero/negative payment would
    /// silently *increase* cost bases; a zero/negative ratio would zero out
    /// or invert holdings or entitlements. ScripForScrip needs a positive
    /// exchange ratio and a replacement listing different from the original —
    /// exchanging a listing into itself would consume its parcels and
    /// recreate them in place. Demerger needs a positive entitlement ratio, a
    /// demerged listing different from the head, and a cost-base percentage
    /// strictly between 0 and 100 — 0 or 100 would zero out one side's cost
    /// base entirely, and anything outside would make one side negative.
    fn kind(self) -> Option<ActionKind> {
        let payment = self.amount_per_unit.is_some();
        let split = self.split_new_units.is_some() || self.split_old_units.is_some();
        let bonus = self.bonus_units.is_some() || self.bonus_held_units.is_some();
        let rights = self.rights_units.is_some()
            || self.rights_held_units.is_some()
            || self.exercise_price.is_some();
        let buyback = self.buyback_price.is_some()
            || self.buyback_dividend.is_some()
            || self.buyback_franking_credit.is_some()
            || self.buyback_market_value.is_some();
        let scrip = self.scrip_listing_id.is_some()
            || self.scrip_new_units.is_some()
            || self.scrip_old_units.is_some();
        let demerger = self.demerger_listing_id.is_some()
            || self.demerger_new_units.is_some()
            || self.demerger_held_units.is_some()
            || self.demerger_cost_base_pct.is_some();
        let positive = |d: Option<Decimal>| d.filter(|v| *v > Decimal::ZERO);
        match self.action_type {
            ActionType::ReturnOfCapital
                if !split && !bonus && !rights && !buyback && !scrip && !demerger =>
            {
                Some(ActionKind::ReturnOfCapital {
                    amount_per_unit: positive(self.amount_per_unit)?,
                    currency: self.currency?,
                })
            }
            ActionType::ShareSplit
                if !payment && !bonus && !rights && !buyback && !scrip && !demerger
                    && self.currency.is_none() =>
            {
                Some(ActionKind::ShareSplit {
                    split_new_units: positive(self.split_new_units)?,
                    split_old_units: positive(self.split_old_units)?,
                })
            }
            ActionType::BonusIssue
                if !payment && !split && !rights && !buyback && !scrip && !demerger
                    && self.currency.is_none() =>
            {
                Some(ActionKind::BonusIssue {
                    bonus_units: positive(self.bonus_units)?,
                    bonus_held_units: positive(self.bonus_held_units)?,
                })
            }
            ActionType::RightsIssue
                if !payment && !split && !bonus && !buyback && !scrip && !demerger =>
            {
                Some(ActionKind::RightsIssue {
                    rights_units: positive(self.rights_units)?,
                    rights_held_units: positive(self.rights_held_units)?,
                    exercise_price: positive(self.exercise_price)?,
                    currency: self.currency?,
                })
            }
            ActionType::ScripForScrip
                if !payment && !split && !bonus && !rights && !buyback && !demerger
                    && self.currency.is_none() =>
            {
                let scrip_listing_id =
                    self.scrip_listing_id.filter(|&l| l != self.listing_id)?;
                Some(ActionKind::ScripForScrip {
                    scrip_listing_id,
                    scrip_new_units: positive(self.scrip_new_units)?,
                    scrip_old_units: positive(self.scrip_old_units)?,
                })
            }
            ActionType::Demerger
                if !payment && !split && !bonus && !rights && !buyback && !scrip
                    && self.currency.is_none() =>
            {
                let demerger_listing_id =
                    self.demerger_listing_id.filter(|&l| l != self.listing_id)?;
                let demerger_cost_base_pct = self
                    .demerger_cost_base_pct
                    .filter(|p| *p > Decimal::ZERO && *p < Decimal::ONE_HUNDRED)?;
                Some(ActionKind::Demerger {
                    demerger_listing_id,
                    demerger_new_units: positive(self.demerger_new_units)?,
                    demerger_held_units: positive(self.demerger_held_units)?,
                    demerger_cost_base_pct,
                })
            }
            ActionType::BuyBack
                if !payment && !split && !bonus && !rights && !scrip && !demerger =>
            {
                let buyback_price = positive(self.buyback_price)?;
                let buyback_dividend = self.buyback_dividend.unwrap_or(Decimal::ZERO);
                if buyback_dividend < Decimal::ZERO || buyback_dividend > buyback_price {
                    return None;
                }
                let buyback_franking_credit =
                    self.buyback_franking_credit.unwrap_or(Decimal::ZERO);
                if buyback_franking_credit < Decimal::ZERO
                    || (buyback_franking_credit > Decimal::ZERO
                        && buyback_dividend == Decimal::ZERO)
                {
                    return None;
                }
                let buyback_market_value = match self.buyback_market_value {
                    Some(mv) => Some(positive(Some(mv))?),
                    None => None,
                };
                Some(ActionKind::BuyBack {
                    buyback_price,
                    buyback_dividend,
                    buyback_franking_credit,
                    buyback_market_value,
                    currency: self.currency?,
                })
            }
            _ => None,
        }
    }
}

pub fn router() -> Router<SqlitePool> {
    Router::new()
        .route("/corporate_actions", get(list))
        .route("/corporate_actions/{id}", get(get_one).put(upsert).delete(delete))
}

const COLUMNS: &str = "id, action_type, listing_id, date, amount_per_unit, currency, \
                       split_new_units, split_old_units, bonus_units, bonus_held_units, \
                       rights_units, rights_held_units, exercise_price, \
                       buyback_price, buyback_dividend, buyback_franking_credit, \
                       buyback_market_value, scrip_listing_id, scrip_new_units, \
                       scrip_old_units, demerger_listing_id, demerger_new_units, \
                       demerger_held_units, demerger_cost_base_pct";

pub async fn db_list(pool: &SqlitePool) -> Result<Vec<CorporateAction>, sqlx::Error> {
    sqlx::query_as(&format!("SELECT {COLUMNS} FROM corporate_actions ORDER BY id"))
        .fetch_all(pool)
        .await
}

pub async fn db_get(pool: &SqlitePool, id: i64) -> Result<Option<CorporateAction>, sqlx::Error> {
    db_get_tx(pool, id).await
}

/// [`db_get`] generic over the executor, so an operation (the rights
/// exercise) can load the action inside its own transaction.
pub async fn db_get_tx<'e, E>(executor: E, id: i64) -> Result<Option<CorporateAction>, sqlx::Error>
where
    E: sqlx::Executor<'e, Database = sqlx::Sqlite>,
{
    sqlx::query_as(&format!("SELECT {COLUMNS} FROM corporate_actions WHERE id = ?"))
        .bind(id)
        .fetch_optional(executor)
        .await
}

#[derive(Debug)]
pub enum WriteError {
    Db(sqlx::Error),
    /// The action is referenced by rights-exercise, buy-back participation,
    /// scrip-for-scrip exchange, or demerger trades (`trades.rights_action_id`
    /// / `trades.buyback_action_id` / `trades.scrip_action_id` /
    /// `trades.demerger_action_id`): editing it would retroactively change
    /// the terms those trades were created and validated against. Delete the
    /// referencing trades first. Mapped to `422`.
    ReferencedByTrade,
}

impl From<sqlx::Error> for WriteError {
    fn from(e: sqlx::Error) -> Self {
        WriteError::Db(e)
    }
}

pub async fn db_upsert(pool: &SqlitePool, action: &CorporateAction) -> Result<(), WriteError> {
    // Spread the variant's payload over the per-type columns; the other
    // types' columns are NULL (the table CHECKs require exactly this shape).
    #[derive(Default)]
    struct Cols {
        amount_per_unit: Option<String>,
        currency: Option<String>,
        split_new_units: Option<String>,
        split_old_units: Option<String>,
        bonus_units: Option<String>,
        bonus_held_units: Option<String>,
        rights_units: Option<String>,
        rights_held_units: Option<String>,
        exercise_price: Option<String>,
        buyback_price: Option<String>,
        buyback_dividend: Option<String>,
        buyback_franking_credit: Option<String>,
        buyback_market_value: Option<String>,
        scrip_listing_id: Option<i64>,
        scrip_new_units: Option<String>,
        scrip_old_units: Option<String>,
        demerger_listing_id: Option<i64>,
        demerger_new_units: Option<String>,
        demerger_held_units: Option<String>,
        demerger_cost_base_pct: Option<String>,
    }
    let mut c = Cols::default();
    match &action.kind {
        ActionKind::ReturnOfCapital { amount_per_unit, currency } => {
            c.amount_per_unit = Some(amount_per_unit.to_string());
            c.currency = Some(currency.clone());
        }
        ActionKind::ShareSplit { split_new_units, split_old_units } => {
            c.split_new_units = Some(split_new_units.to_string());
            c.split_old_units = Some(split_old_units.to_string());
        }
        ActionKind::BonusIssue { bonus_units, bonus_held_units } => {
            c.bonus_units = Some(bonus_units.to_string());
            c.bonus_held_units = Some(bonus_held_units.to_string());
        }
        ActionKind::RightsIssue { rights_units, rights_held_units, exercise_price, currency } => {
            c.rights_units = Some(rights_units.to_string());
            c.rights_held_units = Some(rights_held_units.to_string());
            c.exercise_price = Some(exercise_price.to_string());
            c.currency = Some(currency.clone());
        }
        ActionKind::BuyBack {
            buyback_price,
            buyback_dividend,
            buyback_franking_credit,
            buyback_market_value,
            currency,
        } => {
            c.buyback_price = Some(buyback_price.to_string());
            c.buyback_dividend = Some(buyback_dividend.to_string());
            c.buyback_franking_credit = Some(buyback_franking_credit.to_string());
            c.buyback_market_value = buyback_market_value.map(|v| v.to_string());
            c.currency = Some(currency.clone());
        }
        ActionKind::ScripForScrip { scrip_listing_id, scrip_new_units, scrip_old_units } => {
            c.scrip_listing_id = Some(*scrip_listing_id);
            c.scrip_new_units = Some(scrip_new_units.to_string());
            c.scrip_old_units = Some(scrip_old_units.to_string());
        }
        ActionKind::Demerger {
            demerger_listing_id,
            demerger_new_units,
            demerger_held_units,
            demerger_cost_base_pct,
        } => {
            c.demerger_listing_id = Some(*demerger_listing_id);
            c.demerger_new_units = Some(demerger_new_units.to_string());
            c.demerger_held_units = Some(demerger_held_units.to_string());
            c.demerger_cost_base_pct = Some(demerger_cost_base_pct.to_string());
        }
    }

    let mut tx = pool.begin().await?;

    // An action that exercise, participation, exchange, or demerge trades
    // were validated against is frozen: editing its terms (or re-typing it)
    // would invalidate the checks those trades were created under. Checked
    // and written in one transaction.
    let referenced: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM trades \
                       WHERE rights_action_id = ?1 OR buyback_action_id = ?1 \
                          OR scrip_action_id = ?1 OR demerger_action_id = ?1)",
    )
    .bind(action.id)
    .fetch_one(&mut *tx)
    .await?;
    if referenced {
        return Err(WriteError::ReferencedByTrade);
    }

    sqlx::query(
        "INSERT INTO corporate_actions \
         (id, action_type, listing_id, date, amount_per_unit, currency, \
          split_new_units, split_old_units, bonus_units, bonus_held_units, \
          rights_units, rights_held_units, exercise_price, \
          buyback_price, buyback_dividend, buyback_franking_credit, buyback_market_value, \
          scrip_listing_id, scrip_new_units, scrip_old_units, \
          demerger_listing_id, demerger_new_units, demerger_held_units, \
          demerger_cost_base_pct) \
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?) \
         ON CONFLICT(id) DO UPDATE SET \
             action_type       = excluded.action_type, \
             listing_id        = excluded.listing_id, \
             date              = excluded.date, \
             amount_per_unit   = excluded.amount_per_unit, \
             currency          = excluded.currency, \
             split_new_units   = excluded.split_new_units, \
             split_old_units   = excluded.split_old_units, \
             bonus_units       = excluded.bonus_units, \
             bonus_held_units  = excluded.bonus_held_units, \
             rights_units      = excluded.rights_units, \
             rights_held_units = excluded.rights_held_units, \
             exercise_price    = excluded.exercise_price, \
             buyback_price           = excluded.buyback_price, \
             buyback_dividend        = excluded.buyback_dividend, \
             buyback_franking_credit = excluded.buyback_franking_credit, \
             buyback_market_value    = excluded.buyback_market_value, \
             scrip_listing_id  = excluded.scrip_listing_id, \
             scrip_new_units   = excluded.scrip_new_units, \
             scrip_old_units   = excluded.scrip_old_units, \
             demerger_listing_id    = excluded.demerger_listing_id, \
             demerger_new_units     = excluded.demerger_new_units, \
             demerger_held_units    = excluded.demerger_held_units, \
             demerger_cost_base_pct = excluded.demerger_cost_base_pct",
    )
    .bind(action.id)
    .bind(action.kind.type_str())
    .bind(action.listing_id)
    .bind(action.date)
    .bind(c.amount_per_unit)
    .bind(c.currency)
    .bind(c.split_new_units)
    .bind(c.split_old_units)
    .bind(c.bonus_units)
    .bind(c.bonus_held_units)
    .bind(c.rights_units)
    .bind(c.rights_held_units)
    .bind(c.exercise_price)
    .bind(c.buyback_price)
    .bind(c.buyback_dividend)
    .bind(c.buyback_franking_credit)
    .bind(c.buyback_market_value)
    .bind(c.scrip_listing_id)
    .bind(c.scrip_new_units)
    .bind(c.scrip_old_units)
    .bind(c.demerger_listing_id)
    .bind(c.demerger_new_units)
    .bind(c.demerger_held_units)
    .bind(c.demerger_cost_base_pct)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(())
}

/// Delete an action. An action referenced by rights-exercise, buy-back
/// participation, scrip-for-scrip exchange, or demerger trades is protected
/// by the corresponding `trades.*_action_id` foreign key — the violation
/// surfaces as a database error the handler maps to `422`.
pub async fn db_delete(pool: &SqlitePool, id: i64) -> Result<bool, sqlx::Error> {
    let result = sqlx::query("DELETE FROM corporate_actions WHERE id = ?")
        .bind(id)
        .execute(pool)
        .await?;
    Ok(result.rows_affected() > 0)
}

/// A return-of-capital payment, as consumed by the cost-base reports.
#[derive(Debug, Clone)]
pub struct RocEvent {
    pub date: NaiveDate,
    pub amount_per_unit: Decimal,
    pub currency: String,
}

/// All ReturnOfCapital actions keyed by listing, each list sorted by payment
/// date (then id). Shared by the portfolio/unrealised/realised/open-parcels
/// reports to reduce affected parcels' cost bases, and by the net-capital-gain
/// report's G1 walk.
pub async fn db_return_of_capital_events(
    pool: &SqlitePool,
) -> Result<HashMap<i64, Vec<RocEvent>>, sqlx::Error> {
    let rows = sqlx::query(
        "SELECT listing_id, date, amount_per_unit, currency FROM corporate_actions \
         WHERE action_type = 'ReturnOfCapital' ORDER BY listing_id, date, id",
    )
    .fetch_all(pool)
    .await?;

    let mut map: HashMap<i64, Vec<RocEvent>> = HashMap::new();
    for row in &rows {
        let listing_id: i64 = row.try_get("listing_id")?;
        map.entry(listing_id).or_default().push(RocEvent {
            date: row.try_get("date")?,
            amount_per_unit: parse_dec("amount_per_unit", row.try_get("amount_per_unit")?)?,
            currency: row.try_get("currency")?,
        });
    }
    Ok(map)
}

/// A quantity re-basing event, as consumed by the reports and write-time
/// checks: on `date`, every `old_units` existing units become `new_units`.
/// A ShareSplit (TD 2000/10) is stored as its ratio directly; a
/// non-assessable BonusIssue (`docs/bonus-shares.md`) is its equivalent
/// split — every `bonus_held_units` units become `bonus_held_units +
/// bonus_units` units — because both preserve the parcel's total cost base
/// and acquisition date while scaling the unit count.
#[derive(Debug, Clone)]
pub struct SplitEvent {
    pub date: NaiveDate,
    pub new_units: Decimal,
    pub old_units: Decimal,
}

fn split_event_from_row(row: &sqlx::sqlite::SqliteRow) -> Result<SplitEvent, sqlx::Error> {
    let date = row.try_get("date")?;
    match row.try_get::<String, _>("action_type")?.as_str() {
        "BonusIssue" => {
            let bonus = parse_dec("bonus_units", row.try_get("bonus_units")?)?;
            let held = parse_dec("bonus_held_units", row.try_get("bonus_held_units")?)?;
            Ok(SplitEvent { date, new_units: held + bonus, old_units: held })
        }
        _ => Ok(SplitEvent {
            date,
            new_units: parse_dec("split_new_units", row.try_get("split_new_units")?)?,
            old_units: parse_dec("split_old_units", row.try_get("split_old_units")?)?,
        }),
    }
}

/// All quantity re-basing actions (ShareSplit + BonusIssue, each expressed as
/// its equivalent split) keyed by listing, each list sorted by event date
/// (then id). The reports use these to re-base parcel quantities between unit
/// bases.
pub async fn db_share_split_events(
    pool: &SqlitePool,
) -> Result<HashMap<i64, Vec<SplitEvent>>, sqlx::Error> {
    let rows = sqlx::query(
        "SELECT listing_id, action_type, date, split_new_units, split_old_units, \
                bonus_units, bonus_held_units FROM corporate_actions \
         WHERE action_type IN ('ShareSplit', 'BonusIssue') ORDER BY listing_id, date, id",
    )
    .fetch_all(pool)
    .await?;

    let mut map: HashMap<i64, Vec<SplitEvent>> = HashMap::new();
    for row in &rows {
        let listing_id: i64 = row.try_get("listing_id")?;
        map.entry(listing_id).or_default().push(split_event_from_row(row)?);
    }
    Ok(map)
}

/// The quantity re-basing actions (ShareSplit + BonusIssue) for one listing,
/// sorted by event date (then id). Generic over the executor so write-time
/// validators (sells/trades) can run it inside their transaction.
pub async fn db_splits_for_listing<'e, E>(
    executor: E,
    listing_id: i64,
) -> Result<Vec<SplitEvent>, sqlx::Error>
where
    E: sqlx::Executor<'e, Database = sqlx::Sqlite>,
{
    let rows = sqlx::query(
        "SELECT action_type, date, split_new_units, split_old_units, \
                bonus_units, bonus_held_units FROM corporate_actions \
         WHERE action_type IN ('ShareSplit', 'BonusIssue') AND listing_id = ? \
         ORDER BY date, id",
    )
    .bind(listing_id)
    .fetch_all(executor)
    .await?;
    rows.iter().map(split_event_from_row).collect()
}

/// Cumulative conversion ratio `(new, old)` between unit bases: the product of
/// `new_units`/`old_units` over the splits dated in `(from, up_to]` (`None` =
/// every split after `from`). A holding of `q` units in the basis of `from` is
/// `q × new / old` units in the basis of `up_to`. The interval is half-open
/// because a trade dated on a conversion date is already in post-split units,
/// while a sale or payment dated on it is post-split too.
pub fn split_ratio(
    splits: &[SplitEvent],
    from: NaiveDate,
    up_to: Option<NaiveDate>,
) -> (Decimal, Decimal) {
    let mut new = Decimal::ONE;
    let mut old = Decimal::ONE;
    for s in splits {
        if s.date <= from || up_to.is_some_and(|d| s.date > d) {
            continue;
        }
        new *= s.new_units;
        old *= s.old_units;
    }
    (new, old)
}

/// A parcel quantity as transacted at `acquired`, re-based to the unit basis
/// at `up_to` (`None` = after every recorded split). TD 2000/10: only the unit
/// count scales — the parcel's total cost base and original acquisition date
/// are untouched.
pub fn split_adjusted_quantity(
    qty: Decimal,
    splits: &[SplitEvent],
    acquired: NaiveDate,
    up_to: Option<NaiveDate>,
) -> Decimal {
    let (new, old) = split_ratio(splits, acquired, up_to);
    if new == old { qty } else { qty * new / old }
}

/// The inverse of [`split_adjusted_quantity`]: a quantity expressed in the
/// unit basis at `at` (e.g. a sale's allocated units) converted back to the
/// as-acquired units of a parcel bought at `acquired`.
pub fn as_acquired_quantity(
    qty: Decimal,
    splits: &[SplitEvent],
    acquired: NaiveDate,
    at: NaiveDate,
) -> Decimal {
    let (new, old) = split_ratio(splits, acquired, Some(at));
    if new == old { qty } else { qty * old / new }
}

/// Total units sold out of a parcel acquired at `acquired`, re-based to its
/// as-acquired units. Each `(sale_date, quantity_allocated)` is expressed in
/// the unit basis of its own sale date — a post-split sale allocates post-split
/// units against the pre-split parcel.
pub fn sold_in_acquired_units(
    sales: &[(NaiveDate, Decimal)],
    splits: &[SplitEvent],
    acquired: NaiveDate,
) -> Decimal {
    sales.iter().map(|&(date, qty)| as_acquired_quantity(qty, splits, acquired, date)).sum()
}

/// Cumulative return-of-capital cost-base reduction per *as-acquired* unit for
/// a unit acquired on `acquired` and still held at `up_to` (or held today when
/// `None`): the sum of `amount_per_unit` over the listing's payments dated
/// within `[acquired, up_to]`. A unit sold before a payment was not held for
/// it, so the realised report bounds `up_to` at the sale date; the
/// open-holdings reports pass `None` (an unsold unit was held for every
/// payment since acquisition).
///
/// Each payment is per unit *at the payment date*: a split between acquisition
/// and the payment multiplies the units receiving the per-unit amount, so the
/// payment is scaled by the split ratio over `(acquired, payment date]` to
/// express it per as-acquired unit.
///
/// Fails loudly when a payment's currency differs from the parcel's — amounts in
/// different currencies must never be netted against each other.
pub fn per_unit_reduction(
    events: &[RocEvent],
    splits: &[SplitEvent],
    trade_currency: &str,
    acquired: NaiveDate,
    up_to: Option<NaiveDate>,
) -> Result<Decimal, sqlx::Error> {
    let mut total = Decimal::ZERO;
    for e in events {
        if e.date < acquired || up_to.is_some_and(|d| e.date > d) {
            continue;
        }
        if e.currency != trade_currency {
            return Err(sqlx::Error::Decode(
                format!(
                    "return-of-capital currency {} differs from the parcel's currency {}",
                    e.currency, trade_currency
                )
                .into(),
            ));
        }
        let (new, old) = split_ratio(splits, acquired, Some(e.date));
        total += if new == old { e.amount_per_unit } else { e.amount_per_unit * new / old };
    }
    Ok(total)
}

async fn list(State(pool): State<SqlitePool>) -> Result<Json<Vec<CorporateAction>>, StatusCode> {
    db_list(&pool)
        .await
        .map(Json)
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)
}

async fn get_one(
    State(pool): State<SqlitePool>,
    Path(id): Path<i64>,
) -> Result<Json<CorporateAction>, StatusCode> {
    db_get(&pool, id)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
        .map(Json)
        .ok_or(StatusCode::NOT_FOUND)
}

async fn upsert(
    State(pool): State<SqlitePool>,
    Path(id): Path<i64>,
    Json(body): Json<CorporateActionBody>,
) -> Result<StatusCode, StatusCode> {
    let (listing_id, date) = (body.listing_id, body.date);
    let kind = body.kind().ok_or(StatusCode::UNPROCESSABLE_ENTITY)?;
    let action = CorporateAction { id, listing_id, date, kind };
    db_upsert(&pool, &action)
        .await
        .map(|_| StatusCode::NO_CONTENT)
        .map_err(|e| match e {
            // Unknown listing/currency FK or enum CHECK violation → 422.
            WriteError::Db(err) => write_error_status(&err),
            // Frozen while exercise/participation trades reference it → 422.
            WriteError::ReferencedByTrade => StatusCode::UNPROCESSABLE_ENTITY,
        })
}

async fn delete(
    State(pool): State<SqlitePool>,
    Path(id): Path<i64>,
) -> Result<StatusCode, StatusCode> {
    db_delete(&pool, id)
        .await
        .map(|found| if found { StatusCode::NO_CONTENT } else { StatusCode::NOT_FOUND })
        // Deleting an action still referenced by rights-exercise trades
        // violates the trades.rights_action_id FK → 422 (delete those first).
        .map_err(|e| write_error_status(&e))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{entities::listing, infra::db};
    use axum::{body::Body, http::Request};
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    async fn test_pool() -> SqlitePool {
        db::init(":memory:").await.unwrap()
    }

    async fn insert_listing(pool: &SqlitePool, id: i64, ticker: &str) {
        listing::db_upsert(
            pool,
            &listing::Listing {
                id,
                exchange_mic: "XASX".to_string(),
                ticker: ticker.to_string(),
                name: ticker.to_string(),
                isin: None,
                security_type: listing::SecurityType::Share,
                currency: "AUD".to_string(),
                amit: false,
                preference: false,
            },
        )
        .await
        .unwrap();
    }

    fn roc(id: i64, listing_id: i64, date: NaiveDate, amount: &str) -> CorporateAction {
        CorporateAction {
            id,
            listing_id,
            date,
            kind: ActionKind::ReturnOfCapital {
                amount_per_unit: amount.parse().unwrap(),
                currency: "AUD".to_string(),
            },
        }
    }

    fn split(id: i64, listing_id: i64, date: NaiveDate, new: &str, old: &str) -> CorporateAction {
        CorporateAction {
            id,
            listing_id,
            date,
            kind: ActionKind::ShareSplit {
                split_new_units: new.parse().unwrap(),
                split_old_units: old.parse().unwrap(),
            },
        }
    }

    fn bonus(id: i64, listing_id: i64, date: NaiveDate, units: &str, held: &str) -> CorporateAction {
        CorporateAction {
            id,
            listing_id,
            date,
            kind: ActionKind::BonusIssue {
                bonus_units: units.parse().unwrap(),
                bonus_held_units: held.parse().unwrap(),
            },
        }
    }

    fn rights(
        id: i64,
        listing_id: i64,
        date: NaiveDate,
        units: &str,
        held: &str,
        price: &str,
    ) -> CorporateAction {
        CorporateAction {
            id,
            listing_id,
            date,
            kind: ActionKind::RightsIssue {
                rights_units: units.parse().unwrap(),
                rights_held_units: held.parse().unwrap(),
                exercise_price: price.parse().unwrap(),
                currency: "AUD".to_string(),
            },
        }
    }

    fn buyback(
        id: i64,
        listing_id: i64,
        date: NaiveDate,
        price: &str,
        dividend: &str,
        credit: &str,
        market_value: Option<&str>,
    ) -> CorporateAction {
        CorporateAction {
            id,
            listing_id,
            date,
            kind: ActionKind::BuyBack {
                buyback_price: price.parse().unwrap(),
                buyback_dividend: dividend.parse().unwrap(),
                buyback_franking_credit: credit.parse().unwrap(),
                buyback_market_value: market_value.map(|v| v.parse().unwrap()),
                currency: "AUD".to_string(),
            },
        }
    }

    fn scrip(
        id: i64,
        listing_id: i64,
        scrip_listing_id: i64,
        date: NaiveDate,
        new: &str,
        old: &str,
    ) -> CorporateAction {
        CorporateAction {
            id,
            listing_id,
            date,
            kind: ActionKind::ScripForScrip {
                scrip_listing_id,
                scrip_new_units: new.parse().unwrap(),
                scrip_old_units: old.parse().unwrap(),
            },
        }
    }

    fn demerger(
        id: i64,
        listing_id: i64,
        demerger_listing_id: i64,
        date: NaiveDate,
        new: &str,
        held: &str,
        pct: &str,
    ) -> CorporateAction {
        CorporateAction {
            id,
            listing_id,
            date,
            kind: ActionKind::Demerger {
                demerger_listing_id,
                demerger_new_units: new.parse().unwrap(),
                demerger_held_units: held.parse().unwrap(),
                demerger_cost_base_pct: pct.parse().unwrap(),
            },
        }
    }

    fn split_event(date: NaiveDate, new: &str, old: &str) -> SplitEvent {
        SplitEvent { date, new_units: new.parse().unwrap(), old_units: old.parse().unwrap() }
    }

    fn d(y: i32, m: u32, day: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, day).unwrap()
    }

    // DB-level tests

    #[tokio::test]
    async fn db_insert_and_retrieve_preserves_precision() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "RAP").await;
        db_upsert(&pool, &roc(1, 1, d(2024, 11, 30), "0.505")).await.unwrap();

        let got = db_get(&pool, 1).await.unwrap().unwrap();
        assert_eq!(got.listing_id, 1);
        assert_eq!(got.date, d(2024, 11, 30));
        assert_eq!(
            got.kind,
            ActionKind::ReturnOfCapital {
                amount_per_unit: "0.505".parse().unwrap(),
                currency: "AUD".to_string(),
            }
        );
    }

    #[tokio::test]
    async fn db_insert_and_retrieve_share_split_preserves_ratio() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "SPL").await;
        // An uneven ratio (e.g. a 7-for-2 split) must round-trip exactly.
        db_upsert(&pool, &split(1, 1, d(2024, 11, 30), "7", "2")).await.unwrap();

        let got = db_get(&pool, 1).await.unwrap().unwrap();
        assert_eq!(
            got.kind,
            ActionKind::ShareSplit {
                split_new_units: Decimal::from(7),
                split_old_units: Decimal::from(2),
            }
        );
    }

    #[tokio::test]
    async fn db_insert_and_retrieve_bonus_issue_preserves_ratio() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "BON").await;
        // An uneven ratio (e.g. 3 bonus shares per 7 held) must round-trip exactly.
        db_upsert(&pool, &bonus(1, 1, d(2024, 11, 30), "3", "7")).await.unwrap();

        let got = db_get(&pool, 1).await.unwrap().unwrap();
        assert_eq!(
            got.kind,
            ActionKind::BonusIssue {
                bonus_units: Decimal::from(3),
                bonus_held_units: Decimal::from(7),
            }
        );
    }

    #[tokio::test]
    async fn db_insert_and_retrieve_rights_issue_preserves_terms() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "RTS").await;
        // An uneven ratio and a sub-cent price must round-trip exactly.
        db_upsert(&pool, &rights(1, 1, d(2024, 11, 30), "3", "7", "1.805")).await.unwrap();

        let got = db_get(&pool, 1).await.unwrap().unwrap();
        assert_eq!(
            got.kind,
            ActionKind::RightsIssue {
                rights_units: Decimal::from(3),
                rights_held_units: Decimal::from(7),
                exercise_price: "1.805".parse().unwrap(),
                currency: "AUD".to_string(),
            }
        );
    }

    #[tokio::test]
    async fn db_insert_and_retrieve_buy_back_preserves_terms() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "BBK").await;
        // Sub-cent per-unit components must round-trip exactly.
        db_upsert(&pool, &buyback(1, 1, d(2024, 11, 30), "9.60", "1.405", "0.605", Some("10.20")))
            .await
            .unwrap();

        let got = db_get(&pool, 1).await.unwrap().unwrap();
        assert_eq!(
            got.kind,
            ActionKind::BuyBack {
                buyback_price: "9.60".parse().unwrap(),
                buyback_dividend: "1.405".parse().unwrap(),
                buyback_franking_credit: "0.605".parse().unwrap(),
                buyback_market_value: Some("10.20".parse().unwrap()),
                currency: "AUD".to_string(),
            }
        );

        // The market value is optional: absent round-trips as None.
        db_upsert(&pool, &buyback(2, 1, d(2024, 12, 31), "5.00", "0", "0", None)).await.unwrap();
        let got = db_get(&pool, 2).await.unwrap().unwrap();
        assert!(matches!(
            got.kind,
            ActionKind::BuyBack { buyback_market_value: None, .. }
        ));
    }

    #[tokio::test]
    async fn db_insert_and_retrieve_scrip_for_scrip_preserves_terms() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "OLD").await;
        insert_listing(&pool, 2, "NEW").await;
        // An uneven exchange ratio (e.g. 3 new shares per 7 old) must
        // round-trip exactly.
        db_upsert(&pool, &scrip(1, 1, 2, d(2024, 11, 30), "3", "7")).await.unwrap();

        let got = db_get(&pool, 1).await.unwrap().unwrap();
        assert_eq!(got.listing_id, 1);
        assert_eq!(
            got.kind,
            ActionKind::ScripForScrip {
                scrip_listing_id: 2,
                scrip_new_units: Decimal::from(3),
                scrip_old_units: Decimal::from(7),
            }
        );
    }

    #[tokio::test]
    async fn db_insert_and_retrieve_demerger_preserves_terms() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "HEAD").await;
        insert_listing(&pool, 2, "DEM").await;
        // An uneven ratio and a sub-unit percentage (BHP Steel's 5.063%) must
        // round-trip exactly.
        db_upsert(&pool, &demerger(1, 1, 2, d(2024, 11, 30), "1", "5", "5.063")).await.unwrap();

        let got = db_get(&pool, 1).await.unwrap().unwrap();
        assert_eq!(got.listing_id, 1);
        assert_eq!(
            got.kind,
            ActionKind::Demerger {
                demerger_listing_id: 2,
                demerger_new_units: Decimal::ONE,
                demerger_held_units: Decimal::from(5),
                demerger_cost_base_pct: "5.063".parse().unwrap(),
            }
        );
    }

    /// A Demerger never appears in the split-event or return-of-capital
    /// streams — recording one changes no existing parcel (the demerge
    /// operation does the apportionment).
    #[tokio::test]
    async fn db_demerger_is_not_a_split_or_payment_event() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "HEAD").await;
        insert_listing(&pool, 2, "DEM").await;
        db_upsert(&pool, &demerger(1, 1, 2, d(2024, 11, 30), "1", "5", "5.063")).await.unwrap();

        assert!(db_share_split_events(&pool).await.unwrap().is_empty());
        assert!(db_splits_for_listing(&pool, 1).await.unwrap().is_empty());
        assert!(db_return_of_capital_events(&pool).await.unwrap().is_empty());
    }

    /// The CHECK rejects a demerger of a listing into itself even on a raw
    /// SQL write — the body validation is the first line of defence.
    #[tokio::test]
    async fn db_check_rejects_self_demerger() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "HEAD").await;
        let result = sqlx::query(
            "INSERT INTO corporate_actions \
             (id, action_type, listing_id, date, demerger_listing_id, demerger_new_units, \
              demerger_held_units, demerger_cost_base_pct) \
             VALUES (1, 'Demerger', 1, '2024-11-30', 1, '1', '5', '5.063')",
        )
        .execute(&pool)
        .await;
        assert!(result.is_err(), "demerger_listing_id == listing_id should violate the CHECK");
    }

    /// A ScripForScrip never appears in the split-event or return-of-capital
    /// streams — recording one changes no existing parcel (the exchange
    /// operation does the substitution).
    #[tokio::test]
    async fn db_scrip_for_scrip_is_not_a_split_or_payment_event() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "OLD").await;
        insert_listing(&pool, 2, "NEW").await;
        db_upsert(&pool, &scrip(1, 1, 2, d(2024, 11, 30), "2", "1")).await.unwrap();

        assert!(db_share_split_events(&pool).await.unwrap().is_empty());
        assert!(db_splits_for_listing(&pool, 1).await.unwrap().is_empty());
        assert!(db_return_of_capital_events(&pool).await.unwrap().is_empty());
    }

    /// The CHECK rejects an exchange of a listing into itself even on a raw
    /// SQL write — the body validation is the first line of defence.
    #[tokio::test]
    async fn db_check_rejects_self_exchange() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "OLD").await;
        let result = sqlx::query(
            "INSERT INTO corporate_actions \
             (id, action_type, listing_id, date, scrip_listing_id, scrip_new_units, scrip_old_units) \
             VALUES (1, 'ScripForScrip', 1, '2024-11-30', 1, '2', '1')",
        )
        .execute(&pool)
        .await;
        assert!(result.is_err(), "scrip_listing_id == listing_id should violate the CHECK");
    }

    /// A BuyBack never appears in the split-event or return-of-capital
    /// streams — recording one changes no existing parcel.
    #[tokio::test]
    async fn db_buy_back_is_not_a_split_or_payment_event() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "BBK").await;
        db_upsert(&pool, &buyback(1, 1, d(2024, 11, 30), "9.60", "1.40", "0.60", Some("10.20")))
            .await
            .unwrap();

        assert!(db_share_split_events(&pool).await.unwrap().is_empty());
        assert!(db_splits_for_listing(&pool, 1).await.unwrap().is_empty());
        assert!(db_return_of_capital_events(&pool).await.unwrap().is_empty());
    }

    /// A RightsIssue never appears in the split-event or return-of-capital
    /// streams — recording one changes no existing parcel.
    #[tokio::test]
    async fn db_rights_issue_is_not_a_split_or_payment_event() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "RTS").await;
        db_upsert(&pool, &rights(1, 1, d(2024, 11, 30), "1", "4", "1.80")).await.unwrap();

        assert!(db_share_split_events(&pool).await.unwrap().is_empty());
        assert!(db_splits_for_listing(&pool, 1).await.unwrap().is_empty());
        assert!(db_return_of_capital_events(&pool).await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn db_upsert_updates_existing() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "RAP").await;
        db_upsert(&pool, &roc(1, 1, d(2024, 11, 30), "0.50")).await.unwrap();
        db_upsert(&pool, &roc(1, 1, d(2024, 12, 31), "0.75")).await.unwrap();

        let all = db_list(&pool).await.unwrap();
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].date, d(2024, 12, 31));
        assert_eq!(all[0].kind, roc(1, 1, d(2024, 12, 31), "0.75").kind);
    }

    #[tokio::test]
    async fn db_listing_fk_enforced() {
        let pool = test_pool().await;
        let err = db_upsert(&pool, &roc(1, 999, d(2024, 11, 30), "0.50")).await;
        assert!(err.is_err(), "unknown listing FK should be rejected");
    }

    /// Mixed payloads are unrepresentable in [`ActionKind`], so a raw SQL
    /// write is the only path that could produce one — the per-type table
    /// CHECKs are the last line of defence and must still reject it.
    #[tokio::test]
    async fn db_check_rejects_mixed_payloads() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "RAP").await;
        insert_listing(&pool, 2, "NEW").await;
        // (action_type, the stray columns the CHECK must reject for it)
        for (action_type, stray_cols) in [
            // A ShareSplit carrying a payment, a bonus ratio, or rights terms…
            ("ShareSplit", "amount_per_unit = '0.50', currency = 'AUD'"),
            ("ShareSplit", "bonus_units = '1', bonus_held_units = '10'"),
            ("ShareSplit", "rights_units = '1', rights_held_units = '4', exercise_price = '1.80'"),
            // …a ReturnOfCapital carrying a split ratio…
            ("ReturnOfCapital", "split_new_units = '2', split_old_units = '1'"),
            // …a BonusIssue carrying a split ratio…
            ("BonusIssue", "split_new_units = '2', split_old_units = '1'"),
            // …a RightsIssue carrying a payment or a split ratio…
            ("RightsIssue", "amount_per_unit = '0.50'"),
            ("RightsIssue", "split_new_units = '2', split_old_units = '1'"),
            // …a BuyBack carrying a payment, a split ratio, or rights terms…
            ("BuyBack", "amount_per_unit = '0.50'"),
            ("BuyBack", "split_new_units = '2', split_old_units = '1'"),
            ("BuyBack", "rights_units = '1', rights_held_units = '4', exercise_price = '1.80'"),
            // …the other types carrying buy-back terms…
            ("ShareSplit", "buyback_price = '9.60', buyback_dividend = '0', buyback_franking_credit = '0'"),
            ("ReturnOfCapital", "buyback_price = '9.60', buyback_dividend = '0', buyback_franking_credit = '0'"),
            ("RightsIssue", "buyback_market_value = '10.20'"),
            // …a ScripForScrip carrying a payment, a split ratio, or buy-back
            // terms…
            ("ScripForScrip", "amount_per_unit = '0.50', currency = 'AUD'"),
            ("ScripForScrip", "split_new_units = '2', split_old_units = '1'"),
            ("ScripForScrip", "buyback_price = '9.60', buyback_dividend = '0', buyback_franking_credit = '0'"),
            // …and the other types carrying scrip terms…
            ("ShareSplit", "scrip_listing_id = 2, scrip_new_units = '2', scrip_old_units = '1'"),
            ("BuyBack", "scrip_listing_id = 2, scrip_new_units = '2', scrip_old_units = '1'"),
            // …a Demerger carrying a payment, a split ratio, or scrip terms…
            ("Demerger", "amount_per_unit = '0.50', currency = 'AUD'"),
            ("Demerger", "split_new_units = '2', split_old_units = '1'"),
            ("Demerger", "scrip_listing_id = 2, scrip_new_units = '2', scrip_old_units = '1'"),
            // …and the other types carrying demerger terms.
            ("ShareSplit", "demerger_listing_id = 2, demerger_new_units = '1', demerger_held_units = '5', demerger_cost_base_pct = '5.063'"),
            ("ScripForScrip", "demerger_listing_id = 2, demerger_new_units = '1', demerger_held_units = '5', demerger_cost_base_pct = '5.063'"),
        ] {
            let (base_cols, base_vals) = match action_type {
                "ShareSplit" => ("split_new_units, split_old_units", "'2', '1'"),
                "ReturnOfCapital" => ("amount_per_unit, currency", "'0.50', 'AUD'"),
                "RightsIssue" => (
                    "rights_units, rights_held_units, exercise_price, currency",
                    "'1', '4', '1.80', 'AUD'",
                ),
                "BuyBack" => (
                    "buyback_price, buyback_dividend, buyback_franking_credit, currency",
                    "'9.60', '1.40', '0.60', 'AUD'",
                ),
                "ScripForScrip" => (
                    "scrip_listing_id, scrip_new_units, scrip_old_units",
                    "2, '2', '1'",
                ),
                "Demerger" => (
                    "demerger_listing_id, demerger_new_units, demerger_held_units, \
                     demerger_cost_base_pct",
                    "2, '1', '5', '5.063'",
                ),
                _ => ("bonus_units, bonus_held_units", "'1', '10'"),
            };
            // Insert a valid row, then try to smuggle the stray columns in.
            sqlx::query(&format!(
                "INSERT INTO corporate_actions (id, action_type, listing_id, date, {base_cols}) \
                 VALUES (1, '{action_type}', 1, '2024-11-30', {base_vals})"
            ))
            .execute(&pool)
            .await
            .unwrap();
            let result = sqlx::query(&format!(
                "UPDATE corporate_actions SET {stray_cols} WHERE id = 1"
            ))
            .execute(&pool)
            .await;
            assert!(result.is_err(), "{action_type} + {stray_cols} should violate the CHECK");
            sqlx::query("DELETE FROM corporate_actions").execute(&pool).await.unwrap();
        }
    }

    #[tokio::test]
    async fn db_get_missing_returns_none() {
        let pool = test_pool().await;
        assert!(db_get(&pool, 999).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn db_events_grouped_by_listing_sorted_by_date() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "RAP").await;
        insert_listing(&pool, 2, "XYZ").await;
        db_upsert(&pool, &roc(1, 1, d(2025, 3, 1), "0.30")).await.unwrap();
        db_upsert(&pool, &roc(2, 1, d(2024, 11, 30), "0.50")).await.unwrap();
        db_upsert(&pool, &roc(3, 2, d(2024, 6, 1), "1.00")).await.unwrap();

        let events = db_return_of_capital_events(&pool).await.unwrap();
        assert_eq!(events.len(), 2);
        let l1: Vec<NaiveDate> = events[&1].iter().map(|e| e.date).collect();
        assert_eq!(l1, vec![d(2024, 11, 30), d(2025, 3, 1)]);
        assert_eq!(events[&2].len(), 1);
    }

    #[tokio::test]
    async fn db_split_events_grouped_by_listing_sorted_by_date() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "SPL").await;
        insert_listing(&pool, 2, "XYZ").await;
        db_upsert(&pool, &split(1, 1, d(2025, 3, 1), "2", "1")).await.unwrap();
        db_upsert(&pool, &split(2, 1, d(2024, 11, 30), "1", "10")).await.unwrap();
        db_upsert(&pool, &split(3, 2, d(2024, 6, 1), "3", "1")).await.unwrap();
        // A ReturnOfCapital on the same listing must not appear as a split.
        db_upsert(&pool, &roc(4, 1, d(2024, 6, 1), "0.50")).await.unwrap();

        let events = db_share_split_events(&pool).await.unwrap();
        assert_eq!(events.len(), 2);
        let l1: Vec<NaiveDate> = events[&1].iter().map(|e| e.date).collect();
        assert_eq!(l1, vec![d(2024, 11, 30), d(2025, 3, 1)]);
        assert_eq!(events[&2].len(), 1);

        let for_listing = db_splits_for_listing(&pool, 1).await.unwrap();
        assert_eq!(for_listing.len(), 2);
        assert_eq!(for_listing[0].date, d(2024, 11, 30));
    }

    /// A BonusIssue is folded into the split-event stream as its equivalent
    /// split: every `bonus_held_units` units become `bonus_held_units +
    /// bonus_units` units (a 1-for-10 bonus issue re-bases 11-for-10).
    #[tokio::test]
    async fn db_split_events_include_bonus_issues_as_equivalent_splits() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "BON").await;
        db_upsert(&pool, &bonus(1, 1, d(2025, 3, 1), "1", "10")).await.unwrap();
        // A real split on the same listing interleaves in date order…
        db_upsert(&pool, &split(2, 1, d(2024, 11, 30), "2", "1")).await.unwrap();
        // …and a ReturnOfCapital never appears as a re-basing event.
        db_upsert(&pool, &roc(3, 1, d(2024, 6, 1), "0.50")).await.unwrap();

        let events = db_share_split_events(&pool).await.unwrap();
        let l1 = &events[&1];
        assert_eq!(l1.len(), 2);
        assert_eq!(l1[0].date, d(2024, 11, 30));
        assert_eq!((l1[0].new_units, l1[0].old_units), (Decimal::from(2), Decimal::ONE));
        assert_eq!(l1[1].date, d(2025, 3, 1));
        assert_eq!((l1[1].new_units, l1[1].old_units), (Decimal::from(11), Decimal::from(10)));

        let for_listing = db_splits_for_listing(&pool, 1).await.unwrap();
        assert_eq!(for_listing.len(), 2);
        assert_eq!(for_listing[1].new_units, Decimal::from(11));
        assert_eq!(for_listing[1].old_units, Decimal::from(10));
    }

    #[test]
    fn split_ratio_covers_half_open_interval() {
        let splits = vec![
            split_event(d(2024, 6, 1), "2", "1"),
            split_event(d(2025, 1, 1), "1", "10"),
        ];
        // Acquired before both: both apply → 2/10.
        assert_eq!(
            split_ratio(&splits, d(2024, 1, 1), None),
            (Decimal::from(2), Decimal::from(10))
        );
        // Acquired on the first conversion date: already post-split — only the
        // second applies.
        assert_eq!(
            split_ratio(&splits, d(2024, 6, 1), None),
            (Decimal::ONE, Decimal::from(10))
        );
        // Re-basing to a date on the second conversion: it applies (inclusive end).
        assert_eq!(
            split_ratio(&splits, d(2024, 1, 1), Some(d(2025, 1, 1))),
            (Decimal::from(2), Decimal::from(10))
        );
        // Re-basing to a date before the second conversion: only the first.
        assert_eq!(
            split_ratio(&splits, d(2024, 1, 1), Some(d(2024, 12, 31))),
            (Decimal::from(2), Decimal::ONE)
        );
    }

    #[test]
    fn split_adjusted_and_as_acquired_quantities_are_inverse() {
        let splits = vec![split_event(d(2024, 6, 1), "2", "1")];
        // 100 as-acquired units are 200 post-split units…
        assert_eq!(
            split_adjusted_quantity(Decimal::from(100), &splits, d(2024, 1, 1), None),
            Decimal::from(200)
        );
        // …and 80 post-split units sold come from 40 as-acquired units.
        assert_eq!(
            as_acquired_quantity(Decimal::from(80), &splits, d(2024, 1, 1), d(2024, 9, 1)),
            Decimal::from(40)
        );
        // A consolidation shrinks: 1-for-10 turns 100 into 10.
        let consol = vec![split_event(d(2024, 6, 1), "1", "10")];
        assert_eq!(
            split_adjusted_quantity(Decimal::from(100), &consol, d(2024, 1, 1), None),
            Decimal::from(10)
        );
    }

    #[test]
    fn per_unit_reduction_sums_events_from_acquisition() {
        let events = vec![
            RocEvent { date: d(2024, 1, 1), amount_per_unit: "0.10".parse().unwrap(), currency: "AUD".into() },
            RocEvent { date: d(2024, 6, 1), amount_per_unit: "0.20".parse().unwrap(), currency: "AUD".into() },
            RocEvent { date: d(2025, 1, 1), amount_per_unit: "0.40".parse().unwrap(), currency: "AUD".into() },
        ];
        // Acquired between the first and second events: the first doesn't apply.
        let pu = per_unit_reduction(&events, &[], "AUD", d(2024, 3, 1), None).unwrap();
        assert_eq!(pu, "0.60".parse::<Decimal>().unwrap());
        // Acquired on the event date: held on the payment date, so it applies.
        let pu = per_unit_reduction(&events, &[], "AUD", d(2024, 6, 1), None).unwrap();
        assert_eq!(pu, "0.60".parse::<Decimal>().unwrap());
    }

    #[test]
    fn per_unit_reduction_bounds_at_sale_date() {
        let events = vec![
            RocEvent { date: d(2024, 6, 1), amount_per_unit: "0.20".parse().unwrap(), currency: "AUD".into() },
            RocEvent { date: d(2025, 1, 1), amount_per_unit: "0.40".parse().unwrap(), currency: "AUD".into() },
        ];
        // Sold between the events: only the payment received while held applies.
        let pu = per_unit_reduction(&events, &[], "AUD", d(2024, 1, 1), Some(d(2024, 9, 1))).unwrap();
        assert_eq!(pu, "0.20".parse::<Decimal>().unwrap());
        // Sold on the payment date: still held at the payment, so it applies.
        let pu = per_unit_reduction(&events, &[], "AUD", d(2024, 1, 1), Some(d(2025, 1, 1))).unwrap();
        assert_eq!(pu, "0.60".parse::<Decimal>().unwrap());
        // Sold before any payment: unaffected.
        let pu = per_unit_reduction(&events, &[], "AUD", d(2024, 1, 1), Some(d(2024, 5, 1))).unwrap();
        assert_eq!(pu, Decimal::ZERO);
    }

    /// A payment after a split is per *post-split* unit: each as-acquired unit
    /// became `new/old` units, so the per-as-acquired-unit reduction scales by
    /// the split ratio.
    #[test]
    fn per_unit_reduction_scales_payments_across_a_split() {
        let events = vec![
            // Before the split: per as-acquired unit as-is.
            RocEvent { date: d(2024, 3, 1), amount_per_unit: "0.30".parse().unwrap(), currency: "AUD".into() },
            // After a 2-for-1 split: each as-acquired unit receives it twice.
            RocEvent { date: d(2024, 9, 1), amount_per_unit: "0.20".parse().unwrap(), currency: "AUD".into() },
        ];
        let splits = vec![split_event(d(2024, 6, 1), "2", "1")];
        let pu = per_unit_reduction(&events, &splits, "AUD", d(2024, 1, 1), None).unwrap();
        // 0.30 + 0.20 × 2 = 0.70 per as-acquired unit.
        assert_eq!(pu, "0.70".parse::<Decimal>().unwrap());

        // A parcel acquired after the split holds post-split units already:
        // the later payment applies unscaled.
        let pu = per_unit_reduction(&events, &splits, "AUD", d(2024, 7, 1), None).unwrap();
        assert_eq!(pu, "0.20".parse::<Decimal>().unwrap());
    }

    #[test]
    fn per_unit_reduction_rejects_currency_mismatch() {
        let events = vec![RocEvent {
            date: d(2024, 6, 1),
            amount_per_unit: "0.20".parse().unwrap(),
            currency: "USD".into(),
        }];
        // Never net amounts across currencies: fail loudly, don't skip or zero.
        assert!(per_unit_reduction(&events, &[], "AUD", d(2024, 1, 1), None).is_err());
        // An out-of-range event in another currency is not an error — it doesn't
        // participate in the calculation at all.
        assert!(per_unit_reduction(&events, &[], "AUD", d(2024, 7, 1), None).is_ok());
    }

    // API-level tests

    #[tokio::test]
    async fn api_put_get_list_delete_round_trip() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "RAP").await;
        let body = serde_json::json!({
            "action_type": "ReturnOfCapital",
            "listing_id": 1,
            "date": "2024-11-30",
            "amount_per_unit": "0.50",
            "currency": "AUD",
        });
        let resp = router()
            .with_state(pool.clone())
            .oneshot(
                Request::builder()
                    .method("PUT")
                    .uri("/corporate_actions/1")
                    .header("content-type", "application/json")
                    .body(Body::from(body.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NO_CONTENT);

        let resp = router()
            .with_state(pool.clone())
            .oneshot(Request::builder().uri("/corporate_actions/1").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let got: CorporateAction = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(got.kind, roc(1, 1, d(2024, 11, 30), "0.50").kind);

        let resp = router()
            .with_state(pool.clone())
            .oneshot(Request::builder().uri("/corporate_actions").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let items: Vec<CorporateAction> = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(items.len(), 1);

        let resp = router()
            .with_state(pool.clone())
            .oneshot(
                Request::builder()
                    .method("DELETE")
                    .uri("/corporate_actions/1")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NO_CONTENT);
        assert!(db_get(&pool, 1).await.unwrap().is_none());
    }

    async fn api_put_expecting(pool: &SqlitePool, body: serde_json::Value, expected: StatusCode) {
        let resp = router()
            .with_state(pool.clone())
            .oneshot(
                Request::builder()
                    .method("PUT")
                    .uri("/corporate_actions/1")
                    .header("content-type", "application/json")
                    .body(Body::from(body.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), expected);
    }

    #[tokio::test]
    async fn api_share_split_round_trip() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "SPL").await;
        api_put_expecting(
            &pool,
            serde_json::json!({
                "action_type": "ShareSplit",
                "listing_id": 1,
                "date": "2024-11-30",
                "split_new_units": "2",
                "split_old_units": "1",
            }),
            StatusCode::NO_CONTENT,
        )
        .await;
        let got = db_get(&pool, 1).await.unwrap().unwrap();
        assert_eq!(
            got.kind,
            ActionKind::ShareSplit {
                split_new_units: Decimal::from(2),
                split_old_units: Decimal::ONE,
            }
        );
    }

    #[tokio::test]
    async fn api_bonus_issue_round_trip() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "BON").await;
        api_put_expecting(
            &pool,
            serde_json::json!({
                "action_type": "BonusIssue",
                "listing_id": 1,
                "date": "2024-11-30",
                "bonus_units": "1",
                "bonus_held_units": "10",
            }),
            StatusCode::NO_CONTENT,
        )
        .await;
        let got = db_get(&pool, 1).await.unwrap().unwrap();
        assert_eq!(
            got.kind,
            ActionKind::BonusIssue {
                bonus_units: Decimal::ONE,
                bonus_held_units: Decimal::from(10),
            }
        );
    }

    #[tokio::test]
    async fn api_rights_issue_round_trip() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "RTS").await;
        api_put_expecting(
            &pool,
            serde_json::json!({
                "action_type": "RightsIssue",
                "listing_id": 1,
                "date": "2024-11-30",
                "rights_units": "1",
                "rights_held_units": "4",
                "exercise_price": "1.80",
                "currency": "AUD",
            }),
            StatusCode::NO_CONTENT,
        )
        .await;
        let got = db_get(&pool, 1).await.unwrap().unwrap();
        assert_eq!(
            got.kind,
            ActionKind::RightsIssue {
                rights_units: Decimal::ONE,
                rights_held_units: Decimal::from(4),
                exercise_price: "1.80".parse().unwrap(),
                currency: "AUD".to_string(),
            }
        );
    }

    #[tokio::test]
    async fn api_invalid_rights_issue_payloads_return_422() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "RTS").await;
        // Missing terms, a missing currency, non-positive ratio/price, a
        // stray payment amount, a stray split ratio — and the ratio-only
        // types carrying rights fields or a currency.
        for body in [
            serde_json::json!({
                "action_type": "RightsIssue", "listing_id": 1, "date": "2024-11-30",
            }),
            serde_json::json!({
                "action_type": "RightsIssue", "listing_id": 1, "date": "2024-11-30",
                "rights_units": "1", "rights_held_units": "4", "exercise_price": "1.80",
            }),
            serde_json::json!({
                "action_type": "RightsIssue", "listing_id": 1, "date": "2024-11-30",
                "rights_units": "0", "rights_held_units": "4", "exercise_price": "1.80",
                "currency": "AUD",
            }),
            serde_json::json!({
                "action_type": "RightsIssue", "listing_id": 1, "date": "2024-11-30",
                "rights_units": "1", "rights_held_units": "-4", "exercise_price": "1.80",
                "currency": "AUD",
            }),
            serde_json::json!({
                "action_type": "RightsIssue", "listing_id": 1, "date": "2024-11-30",
                "rights_units": "1", "rights_held_units": "4", "exercise_price": "0",
                "currency": "AUD",
            }),
            serde_json::json!({
                "action_type": "RightsIssue", "listing_id": 1, "date": "2024-11-30",
                "rights_units": "1", "rights_held_units": "4", "exercise_price": "1.80",
                "currency": "AUD", "amount_per_unit": "0.50",
            }),
            serde_json::json!({
                "action_type": "RightsIssue", "listing_id": 1, "date": "2024-11-30",
                "rights_units": "1", "rights_held_units": "4", "exercise_price": "1.80",
                "currency": "AUD", "split_new_units": "2", "split_old_units": "1",
            }),
            serde_json::json!({
                "action_type": "ShareSplit", "listing_id": 1, "date": "2024-11-30",
                "split_new_units": "2", "split_old_units": "1",
                "rights_units": "1", "rights_held_units": "4", "exercise_price": "1.80",
                "currency": "AUD",
            }),
            serde_json::json!({
                "action_type": "BonusIssue", "listing_id": 1, "date": "2024-11-30",
                "bonus_units": "1", "bonus_held_units": "10", "currency": "AUD",
            }),
        ] {
            api_put_expecting(&pool, body, StatusCode::UNPROCESSABLE_ENTITY).await;
        }
    }

    #[tokio::test]
    async fn api_buy_back_round_trip() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "BBK").await;
        api_put_expecting(
            &pool,
            serde_json::json!({
                "action_type": "BuyBack",
                "listing_id": 1,
                "date": "2024-11-30",
                "buyback_price": "9.60",
                "buyback_dividend": "1.40",
                "buyback_franking_credit": "0.60",
                "buyback_market_value": "10.20",
                "currency": "AUD",
            }),
            StatusCode::NO_CONTENT,
        )
        .await;
        let got = db_get(&pool, 1).await.unwrap().unwrap();
        assert_eq!(
            got.kind,
            ActionKind::BuyBack {
                buyback_price: "9.60".parse().unwrap(),
                buyback_dividend: "1.40".parse().unwrap(),
                buyback_franking_credit: "0.60".parse().unwrap(),
                buyback_market_value: Some("10.20".parse().unwrap()),
                currency: "AUD".to_string(),
            }
        );

        // The no-dividend (listed post-Oct-2022) shape: dividend/credit
        // default to 0 and the market value may be omitted.
        api_put_expecting(
            &pool,
            serde_json::json!({
                "action_type": "BuyBack",
                "listing_id": 1,
                "date": "2024-12-31",
                "buyback_price": "5.00",
                "currency": "AUD",
            }),
            StatusCode::NO_CONTENT,
        )
        .await;
        let got = db_get(&pool, 1).await.unwrap().unwrap();
        assert_eq!(
            got.kind,
            ActionKind::BuyBack {
                buyback_price: "5.00".parse().unwrap(),
                buyback_dividend: Decimal::ZERO,
                buyback_franking_credit: Decimal::ZERO,
                buyback_market_value: None,
                currency: "AUD".to_string(),
            }
        );
    }

    #[tokio::test]
    async fn api_invalid_buy_back_payloads_return_422() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "BBK").await;
        // Missing/non-positive price, a missing currency, a dividend
        // exceeding the price (it is a component of it), a negative
        // dividend, a credit without a dividend to attach to, a negative
        // credit, a non-positive market value, stray cross-type fields —
        // and the other types carrying buy-back fields.
        for body in [
            serde_json::json!({
                "action_type": "BuyBack", "listing_id": 1, "date": "2024-11-30",
                "currency": "AUD",
            }),
            serde_json::json!({
                "action_type": "BuyBack", "listing_id": 1, "date": "2024-11-30",
                "buyback_price": "0", "currency": "AUD",
            }),
            serde_json::json!({
                "action_type": "BuyBack", "listing_id": 1, "date": "2024-11-30",
                "buyback_price": "9.60",
            }),
            serde_json::json!({
                "action_type": "BuyBack", "listing_id": 1, "date": "2024-11-30",
                "buyback_price": "9.60", "buyback_dividend": "9.61", "currency": "AUD",
            }),
            serde_json::json!({
                "action_type": "BuyBack", "listing_id": 1, "date": "2024-11-30",
                "buyback_price": "9.60", "buyback_dividend": "-1.40", "currency": "AUD",
            }),
            serde_json::json!({
                "action_type": "BuyBack", "listing_id": 1, "date": "2024-11-30",
                "buyback_price": "9.60", "buyback_franking_credit": "0.60", "currency": "AUD",
            }),
            serde_json::json!({
                "action_type": "BuyBack", "listing_id": 1, "date": "2024-11-30",
                "buyback_price": "9.60", "buyback_dividend": "1.40",
                "buyback_franking_credit": "-0.60", "currency": "AUD",
            }),
            serde_json::json!({
                "action_type": "BuyBack", "listing_id": 1, "date": "2024-11-30",
                "buyback_price": "9.60", "buyback_market_value": "0", "currency": "AUD",
            }),
            serde_json::json!({
                "action_type": "BuyBack", "listing_id": 1, "date": "2024-11-30",
                "buyback_price": "9.60", "currency": "AUD",
                "split_new_units": "2", "split_old_units": "1",
            }),
            serde_json::json!({
                "action_type": "BuyBack", "listing_id": 1, "date": "2024-11-30",
                "buyback_price": "9.60", "currency": "AUD", "amount_per_unit": "0.50",
            }),
            serde_json::json!({
                "action_type": "ShareSplit", "listing_id": 1, "date": "2024-11-30",
                "split_new_units": "2", "split_old_units": "1", "buyback_price": "9.60",
            }),
            serde_json::json!({
                "action_type": "ReturnOfCapital", "listing_id": 1, "date": "2024-11-30",
                "amount_per_unit": "0.50", "currency": "AUD", "buyback_dividend": "1.40",
            }),
            serde_json::json!({
                "action_type": "RightsIssue", "listing_id": 1, "date": "2024-11-30",
                "rights_units": "1", "rights_held_units": "4", "exercise_price": "1.80",
                "currency": "AUD", "buyback_market_value": "10.20",
            }),
        ] {
            api_put_expecting(&pool, body, StatusCode::UNPROCESSABLE_ENTITY).await;
        }
    }

    #[tokio::test]
    async fn api_invalid_bonus_issue_payloads_return_422() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "BON").await;
        // Missing ratio, non-positive ratio, a stray payment field, a stray
        // split ratio — and a ShareSplit carrying a bonus ratio.
        for body in [
            serde_json::json!({
                "action_type": "BonusIssue", "listing_id": 1, "date": "2024-11-30",
            }),
            serde_json::json!({
                "action_type": "BonusIssue", "listing_id": 1, "date": "2024-11-30",
                "bonus_units": "0", "bonus_held_units": "10",
            }),
            serde_json::json!({
                "action_type": "BonusIssue", "listing_id": 1, "date": "2024-11-30",
                "bonus_units": "1", "bonus_held_units": "-10",
            }),
            serde_json::json!({
                "action_type": "BonusIssue", "listing_id": 1, "date": "2024-11-30",
                "bonus_units": "1", "bonus_held_units": "10",
                "amount_per_unit": "0.50", "currency": "AUD",
            }),
            serde_json::json!({
                "action_type": "BonusIssue", "listing_id": 1, "date": "2024-11-30",
                "bonus_units": "1", "bonus_held_units": "10",
                "split_new_units": "2", "split_old_units": "1",
            }),
            serde_json::json!({
                "action_type": "ShareSplit", "listing_id": 1, "date": "2024-11-30",
                "split_new_units": "2", "split_old_units": "1",
                "bonus_units": "1", "bonus_held_units": "10",
            }),
        ] {
            api_put_expecting(&pool, body, StatusCode::UNPROCESSABLE_ENTITY).await;
        }
    }

    #[tokio::test]
    async fn api_scrip_for_scrip_round_trip() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "OLD").await;
        insert_listing(&pool, 2, "NEW").await;
        api_put_expecting(
            &pool,
            serde_json::json!({
                "action_type": "ScripForScrip",
                "listing_id": 1,
                "date": "2024-11-30",
                "scrip_listing_id": 2,
                "scrip_new_units": "2",
                "scrip_old_units": "1",
            }),
            StatusCode::NO_CONTENT,
        )
        .await;
        let got = db_get(&pool, 1).await.unwrap().unwrap();
        assert_eq!(
            got.kind,
            ActionKind::ScripForScrip {
                scrip_listing_id: 2,
                scrip_new_units: Decimal::from(2),
                scrip_old_units: Decimal::ONE,
            }
        );
    }

    #[tokio::test]
    async fn api_invalid_scrip_for_scrip_payloads_return_422() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "OLD").await;
        insert_listing(&pool, 2, "NEW").await;
        // Missing terms, a non-positive ratio, the same listing on both
        // sides, an unknown replacement listing, a stray currency, stray
        // cross-type fields — and the other types carrying scrip fields.
        for body in [
            serde_json::json!({
                "action_type": "ScripForScrip", "listing_id": 1, "date": "2024-11-30",
            }),
            serde_json::json!({
                "action_type": "ScripForScrip", "listing_id": 1, "date": "2024-11-30",
                "scrip_listing_id": 2, "scrip_new_units": "0", "scrip_old_units": "1",
            }),
            serde_json::json!({
                "action_type": "ScripForScrip", "listing_id": 1, "date": "2024-11-30",
                "scrip_listing_id": 2, "scrip_new_units": "2", "scrip_old_units": "-1",
            }),
            serde_json::json!({
                "action_type": "ScripForScrip", "listing_id": 1, "date": "2024-11-30",
                "scrip_listing_id": 1, "scrip_new_units": "2", "scrip_old_units": "1",
            }),
            serde_json::json!({
                "action_type": "ScripForScrip", "listing_id": 1, "date": "2024-11-30",
                "scrip_listing_id": 999, "scrip_new_units": "2", "scrip_old_units": "1",
            }),
            serde_json::json!({
                "action_type": "ScripForScrip", "listing_id": 1, "date": "2024-11-30",
                "scrip_listing_id": 2, "scrip_new_units": "2", "scrip_old_units": "1",
                "currency": "AUD",
            }),
            serde_json::json!({
                "action_type": "ScripForScrip", "listing_id": 1, "date": "2024-11-30",
                "scrip_listing_id": 2, "scrip_new_units": "2", "scrip_old_units": "1",
                "amount_per_unit": "0.50",
            }),
            serde_json::json!({
                "action_type": "ScripForScrip", "listing_id": 1, "date": "2024-11-30",
                "scrip_listing_id": 2, "scrip_new_units": "2", "scrip_old_units": "1",
                "split_new_units": "2", "split_old_units": "1",
            }),
            serde_json::json!({
                "action_type": "ShareSplit", "listing_id": 1, "date": "2024-11-30",
                "split_new_units": "2", "split_old_units": "1",
                "scrip_listing_id": 2, "scrip_new_units": "2", "scrip_old_units": "1",
            }),
            serde_json::json!({
                "action_type": "BuyBack", "listing_id": 1, "date": "2024-11-30",
                "buyback_price": "9.60", "currency": "AUD", "scrip_listing_id": 2,
            }),
        ] {
            api_put_expecting(&pool, body, StatusCode::UNPROCESSABLE_ENTITY).await;
        }
    }

    #[tokio::test]
    async fn api_demerger_round_trip() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "HEAD").await;
        insert_listing(&pool, 2, "DEM").await;
        api_put_expecting(
            &pool,
            serde_json::json!({
                "action_type": "Demerger",
                "listing_id": 1,
                "date": "2024-11-30",
                "demerger_listing_id": 2,
                "demerger_new_units": "1",
                "demerger_held_units": "5",
                "demerger_cost_base_pct": "5.063",
            }),
            StatusCode::NO_CONTENT,
        )
        .await;
        let got = db_get(&pool, 1).await.unwrap().unwrap();
        assert_eq!(
            got.kind,
            ActionKind::Demerger {
                demerger_listing_id: 2,
                demerger_new_units: Decimal::ONE,
                demerger_held_units: Decimal::from(5),
                demerger_cost_base_pct: "5.063".parse().unwrap(),
            }
        );
    }

    #[tokio::test]
    async fn api_invalid_demerger_payloads_return_422() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "HEAD").await;
        insert_listing(&pool, 2, "DEM").await;
        // Missing terms, a non-positive ratio, a percentage at/outside the
        // (0, 100) bounds, the same listing on both sides, an unknown
        // demerged listing, a stray currency, stray cross-type fields — and
        // the other types carrying demerger fields.
        for body in [
            serde_json::json!({
                "action_type": "Demerger", "listing_id": 1, "date": "2024-11-30",
            }),
            serde_json::json!({
                "action_type": "Demerger", "listing_id": 1, "date": "2024-11-30",
                "demerger_listing_id": 2, "demerger_new_units": "0",
                "demerger_held_units": "5", "demerger_cost_base_pct": "5.063",
            }),
            serde_json::json!({
                "action_type": "Demerger", "listing_id": 1, "date": "2024-11-30",
                "demerger_listing_id": 2, "demerger_new_units": "1",
                "demerger_held_units": "-5", "demerger_cost_base_pct": "5.063",
            }),
            serde_json::json!({
                "action_type": "Demerger", "listing_id": 1, "date": "2024-11-30",
                "demerger_listing_id": 2, "demerger_new_units": "1",
                "demerger_held_units": "5", "demerger_cost_base_pct": "0",
            }),
            serde_json::json!({
                "action_type": "Demerger", "listing_id": 1, "date": "2024-11-30",
                "demerger_listing_id": 2, "demerger_new_units": "1",
                "demerger_held_units": "5", "demerger_cost_base_pct": "100",
            }),
            serde_json::json!({
                "action_type": "Demerger", "listing_id": 1, "date": "2024-11-30",
                "demerger_listing_id": 2, "demerger_new_units": "1",
                "demerger_held_units": "5", "demerger_cost_base_pct": "-5.063",
            }),
            serde_json::json!({
                "action_type": "Demerger", "listing_id": 1, "date": "2024-11-30",
                "demerger_listing_id": 1, "demerger_new_units": "1",
                "demerger_held_units": "5", "demerger_cost_base_pct": "5.063",
            }),
            serde_json::json!({
                "action_type": "Demerger", "listing_id": 1, "date": "2024-11-30",
                "demerger_listing_id": 999, "demerger_new_units": "1",
                "demerger_held_units": "5", "demerger_cost_base_pct": "5.063",
            }),
            serde_json::json!({
                "action_type": "Demerger", "listing_id": 1, "date": "2024-11-30",
                "demerger_listing_id": 2, "demerger_new_units": "1",
                "demerger_held_units": "5", "demerger_cost_base_pct": "5.063",
                "currency": "AUD",
            }),
            serde_json::json!({
                "action_type": "Demerger", "listing_id": 1, "date": "2024-11-30",
                "demerger_listing_id": 2, "demerger_new_units": "1",
                "demerger_held_units": "5", "demerger_cost_base_pct": "5.063",
                "split_new_units": "2", "split_old_units": "1",
            }),
            serde_json::json!({
                "action_type": "Demerger", "listing_id": 1, "date": "2024-11-30",
                "demerger_listing_id": 2, "demerger_new_units": "1",
                "demerger_held_units": "5", "demerger_cost_base_pct": "5.063",
                "scrip_listing_id": 2, "scrip_new_units": "2", "scrip_old_units": "1",
            }),
            serde_json::json!({
                "action_type": "ShareSplit", "listing_id": 1, "date": "2024-11-30",
                "split_new_units": "2", "split_old_units": "1",
                "demerger_listing_id": 2, "demerger_new_units": "1",
                "demerger_held_units": "5", "demerger_cost_base_pct": "5.063",
            }),
            serde_json::json!({
                "action_type": "ScripForScrip", "listing_id": 1, "date": "2024-11-30",
                "scrip_listing_id": 2, "scrip_new_units": "2", "scrip_old_units": "1",
                "demerger_cost_base_pct": "5.063",
            }),
        ] {
            api_put_expecting(&pool, body, StatusCode::UNPROCESSABLE_ENTITY).await;
        }
    }

    #[tokio::test]
    async fn api_non_positive_amount_returns_422() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "RAP").await;
        for amount in ["0", "-0.50"] {
            api_put_expecting(
                &pool,
                serde_json::json!({
                    "action_type": "ReturnOfCapital",
                    "listing_id": 1,
                    "date": "2024-11-30",
                    "amount_per_unit": amount,
                    "currency": "AUD",
                }),
                StatusCode::UNPROCESSABLE_ENTITY,
            )
            .await;
        }
    }

    #[tokio::test]
    async fn api_invalid_share_split_payloads_return_422() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "SPL").await;
        // Missing ratio, non-positive ratio, and a stray payment field.
        for body in [
            serde_json::json!({
                "action_type": "ShareSplit", "listing_id": 1, "date": "2024-11-30",
            }),
            serde_json::json!({
                "action_type": "ShareSplit", "listing_id": 1, "date": "2024-11-30",
                "split_new_units": "0", "split_old_units": "1",
            }),
            serde_json::json!({
                "action_type": "ShareSplit", "listing_id": 1, "date": "2024-11-30",
                "split_new_units": "2", "split_old_units": "-1",
            }),
            serde_json::json!({
                "action_type": "ShareSplit", "listing_id": 1, "date": "2024-11-30",
                "split_new_units": "2", "split_old_units": "1",
                "amount_per_unit": "0.50", "currency": "AUD",
            }),
            // …and a ReturnOfCapital carrying a split ratio.
            serde_json::json!({
                "action_type": "ReturnOfCapital", "listing_id": 1, "date": "2024-11-30",
                "amount_per_unit": "0.50", "currency": "AUD",
                "split_new_units": "2", "split_old_units": "1",
            }),
        ] {
            api_put_expecting(&pool, body, StatusCode::UNPROCESSABLE_ENTITY).await;
        }
    }

    #[tokio::test]
    async fn api_unknown_listing_returns_422() {
        let pool = test_pool().await;
        api_put_expecting(
            &pool,
            serde_json::json!({
                "action_type": "ReturnOfCapital",
                "listing_id": 999,
                "date": "2024-11-30",
                "amount_per_unit": "0.50",
                "currency": "AUD",
            }),
            StatusCode::UNPROCESSABLE_ENTITY,
        )
        .await;
    }

    #[tokio::test]
    async fn api_unknown_currency_returns_422() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "RAP").await;
        api_put_expecting(
            &pool,
            serde_json::json!({
                "action_type": "ReturnOfCapital",
                "listing_id": 1,
                "date": "2024-11-30",
                "amount_per_unit": "0.50",
                "currency": "ZZZ",
            }),
            StatusCode::UNPROCESSABLE_ENTITY,
        )
        .await;
    }

    #[tokio::test]
    async fn api_unknown_action_type_rejected() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "RAP").await;
        // Serde rejects an unrecognised enum variant before it reaches the DB.
        api_put_expecting(
            &pool,
            serde_json::json!({
                "action_type": "Merger",
                "listing_id": 1,
                "date": "2024-11-30",
                "amount_per_unit": "0.50",
                "currency": "AUD",
            }),
            StatusCode::UNPROCESSABLE_ENTITY,
        )
        .await;
    }

    #[tokio::test]
    async fn api_get_and_delete_missing_return_404() {
        let pool = test_pool().await;
        let resp = router()
            .with_state(pool.clone())
            .oneshot(Request::builder().uri("/corporate_actions/999").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);

        let resp = router()
            .with_state(pool)
            .oneshot(
                Request::builder()
                    .method("DELETE")
                    .uri("/corporate_actions/999")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }
}
