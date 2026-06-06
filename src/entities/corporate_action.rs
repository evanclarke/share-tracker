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
//! `ActionType` is the extension point for future corporate actions (rights
//! issues, buy-backs, ...), each widening the enum and its CHECK.

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

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, sqlx::Type)]
pub enum ActionType {
    ReturnOfCapital,
    ShareSplit,
    BonusIssue,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CorporateAction {
    pub id: i64,
    pub action_type: ActionType,
    pub listing_id: i64,
    /// ReturnOfCapital: payment date — parcels acquired on/before it are
    /// affected. ShareSplit: conversion date — parcels acquired before it are
    /// converted (a trade dated on the conversion date is already in
    /// post-split units). BonusIssue: issue date — parcels acquired before it
    /// receive bonus units (a trade dated on the issue date is ex-bonus).
    pub date: NaiveDate,
    /// ReturnOfCapital only: per-unit payment amount in `currency` (positive).
    pub amount_per_unit: Option<Decimal>,
    /// ReturnOfCapital only: currency of `amount_per_unit`.
    pub currency: Option<String>,
    /// ShareSplit only: every `split_old_units` existing units become
    /// `split_new_units` units on the conversion date (both positive; a
    /// consolidation has new < old).
    pub split_new_units: Option<Decimal>,
    /// ShareSplit only: see `split_new_units`.
    pub split_old_units: Option<Decimal>,
    /// BonusIssue only: every `bonus_held_units` units held receive
    /// `bonus_units` additional units on the issue date (both positive; a
    /// 1-for-10 bonus issue is bonus_units=1 / bonus_held_units=10).
    pub bonus_units: Option<Decimal>,
    /// BonusIssue only: see `bonus_units`.
    pub bonus_held_units: Option<Decimal>,
}

fn opt_dec(row: &sqlx::sqlite::SqliteRow, column: &str) -> Result<Option<Decimal>, sqlx::Error> {
    row.try_get::<Option<String>, _>(column)?
        .map(|s| parse_dec(column, s))
        .transpose()
}

impl<'r> sqlx::FromRow<'r, sqlx::sqlite::SqliteRow> for CorporateAction {
    fn from_row(row: &'r sqlx::sqlite::SqliteRow) -> Result<Self, sqlx::Error> {
        Ok(CorporateAction {
            id: row.try_get("id")?,
            action_type: row.try_get("action_type")?,
            listing_id: row.try_get("listing_id")?,
            date: row.try_get("date")?,
            amount_per_unit: opt_dec(row, "amount_per_unit")?,
            currency: row.try_get("currency")?,
            split_new_units: opt_dec(row, "split_new_units")?,
            split_old_units: opt_dec(row, "split_old_units")?,
            bonus_units: opt_dec(row, "bonus_units")?,
            bonus_held_units: opt_dec(row, "bonus_held_units")?,
        })
    }
}

#[derive(Debug, Deserialize)]
pub struct CorporateActionBody {
    pub action_type: ActionType,
    pub listing_id: i64,
    pub date: NaiveDate,
    #[serde(default)]
    pub amount_per_unit: Option<Decimal>,
    #[serde(default)]
    pub currency: Option<String>,
    #[serde(default)]
    pub split_new_units: Option<Decimal>,
    #[serde(default)]
    pub split_old_units: Option<Decimal>,
    #[serde(default)]
    pub bonus_units: Option<Decimal>,
    #[serde(default)]
    pub bonus_held_units: Option<Decimal>,
}

/// Each action type carries exactly its own payload (mirrors the table
/// CHECKs, plus positivity): ReturnOfCapital needs a positive payment and a
/// currency; ShareSplit a positive conversion ratio; BonusIssue a positive
/// bonus ratio — each with every other type's fields absent. A zero/negative
/// payment would silently *increase* cost bases; a zero/negative ratio would
/// zero out or invert holdings.
fn body_is_valid(body: &CorporateActionBody) -> bool {
    let payment = body.amount_per_unit.is_some() || body.currency.is_some();
    let split = body.split_new_units.is_some() || body.split_old_units.is_some();
    let bonus = body.bonus_units.is_some() || body.bonus_held_units.is_some();
    match body.action_type {
        ActionType::ReturnOfCapital => {
            body.amount_per_unit.is_some_and(|a| a > Decimal::ZERO)
                && body.currency.is_some()
                && !split
                && !bonus
        }
        ActionType::ShareSplit => {
            body.split_new_units.is_some_and(|n| n > Decimal::ZERO)
                && body.split_old_units.is_some_and(|o| o > Decimal::ZERO)
                && !payment
                && !bonus
        }
        ActionType::BonusIssue => {
            body.bonus_units.is_some_and(|b| b > Decimal::ZERO)
                && body.bonus_held_units.is_some_and(|h| h > Decimal::ZERO)
                && !payment
                && !split
        }
    }
}

pub fn router() -> Router<SqlitePool> {
    Router::new()
        .route("/corporate_actions", get(list))
        .route("/corporate_actions/{id}", get(get_one).put(upsert).delete(delete))
}

const COLUMNS: &str = "id, action_type, listing_id, date, amount_per_unit, currency, \
                       split_new_units, split_old_units, bonus_units, bonus_held_units";

pub async fn db_list(pool: &SqlitePool) -> Result<Vec<CorporateAction>, sqlx::Error> {
    sqlx::query_as(&format!("SELECT {COLUMNS} FROM corporate_actions ORDER BY id"))
        .fetch_all(pool)
        .await
}

pub async fn db_get(pool: &SqlitePool, id: i64) -> Result<Option<CorporateAction>, sqlx::Error> {
    sqlx::query_as(&format!("SELECT {COLUMNS} FROM corporate_actions WHERE id = ?"))
        .bind(id)
        .fetch_optional(pool)
        .await
}

pub async fn db_upsert(pool: &SqlitePool, action: &CorporateAction) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO corporate_actions \
         (id, action_type, listing_id, date, amount_per_unit, currency, \
          split_new_units, split_old_units, bonus_units, bonus_held_units) \
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?) \
         ON CONFLICT(id) DO UPDATE SET \
             action_type      = excluded.action_type, \
             listing_id       = excluded.listing_id, \
             date             = excluded.date, \
             amount_per_unit  = excluded.amount_per_unit, \
             currency         = excluded.currency, \
             split_new_units  = excluded.split_new_units, \
             split_old_units  = excluded.split_old_units, \
             bonus_units      = excluded.bonus_units, \
             bonus_held_units = excluded.bonus_held_units",
    )
    .bind(action.id)
    .bind(action.action_type)
    .bind(action.listing_id)
    .bind(action.date)
    .bind(action.amount_per_unit.map(|d| d.to_string()))
    .bind(&action.currency)
    .bind(action.split_new_units.map(|d| d.to_string()))
    .bind(action.split_old_units.map(|d| d.to_string()))
    .bind(action.bonus_units.map(|d| d.to_string()))
    .bind(action.bonus_held_units.map(|d| d.to_string()))
    .execute(pool)
    .await?;
    Ok(())
}

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
    match row.try_get::<ActionType, _>("action_type")? {
        ActionType::BonusIssue => {
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
    if !body_is_valid(&body) {
        return Err(StatusCode::UNPROCESSABLE_ENTITY);
    }
    let action = CorporateAction {
        id,
        action_type: body.action_type,
        listing_id: body.listing_id,
        date: body.date,
        amount_per_unit: body.amount_per_unit,
        currency: body.currency,
        split_new_units: body.split_new_units,
        split_old_units: body.split_old_units,
        bonus_units: body.bonus_units,
        bonus_held_units: body.bonus_held_units,
    };
    db_upsert(&pool, &action)
        .await
        .map(|_| StatusCode::NO_CONTENT)
        // Unknown listing/currency FK or enum CHECK violation → 422.
        .map_err(|e| write_error_status(&e))
}

async fn delete(
    State(pool): State<SqlitePool>,
    Path(id): Path<i64>,
) -> Result<StatusCode, StatusCode> {
    db_delete(&pool, id)
        .await
        .map(|found| if found { StatusCode::NO_CONTENT } else { StatusCode::NOT_FOUND })
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)
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
            action_type: ActionType::ReturnOfCapital,
            listing_id,
            date,
            amount_per_unit: Some(amount.parse().unwrap()),
            currency: Some("AUD".to_string()),
            split_new_units: None,
            split_old_units: None,
            bonus_units: None,
            bonus_held_units: None,
        }
    }

    fn split(id: i64, listing_id: i64, date: NaiveDate, new: &str, old: &str) -> CorporateAction {
        CorporateAction {
            id,
            action_type: ActionType::ShareSplit,
            listing_id,
            date,
            amount_per_unit: None,
            currency: None,
            split_new_units: Some(new.parse().unwrap()),
            split_old_units: Some(old.parse().unwrap()),
            bonus_units: None,
            bonus_held_units: None,
        }
    }

    fn bonus(id: i64, listing_id: i64, date: NaiveDate, units: &str, held: &str) -> CorporateAction {
        CorporateAction {
            id,
            action_type: ActionType::BonusIssue,
            listing_id,
            date,
            amount_per_unit: None,
            currency: None,
            split_new_units: None,
            split_old_units: None,
            bonus_units: Some(units.parse().unwrap()),
            bonus_held_units: Some(held.parse().unwrap()),
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
        assert_eq!(got.action_type, ActionType::ReturnOfCapital);
        assert_eq!(got.listing_id, 1);
        assert_eq!(got.date, d(2024, 11, 30));
        assert_eq!(got.amount_per_unit, Some("0.505".parse::<Decimal>().unwrap()));
        assert_eq!(got.currency.as_deref(), Some("AUD"));
        assert_eq!(got.split_new_units, None);
        assert_eq!(got.split_old_units, None);
    }

    #[tokio::test]
    async fn db_insert_and_retrieve_share_split_preserves_ratio() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "SPL").await;
        // An uneven ratio (e.g. a 7-for-2 split) must round-trip exactly.
        db_upsert(&pool, &split(1, 1, d(2024, 11, 30), "7", "2")).await.unwrap();

        let got = db_get(&pool, 1).await.unwrap().unwrap();
        assert_eq!(got.action_type, ActionType::ShareSplit);
        assert_eq!(got.split_new_units, Some(Decimal::from(7)));
        assert_eq!(got.split_old_units, Some(Decimal::from(2)));
        assert_eq!(got.amount_per_unit, None);
        assert_eq!(got.currency, None);
    }

    #[tokio::test]
    async fn db_insert_and_retrieve_bonus_issue_preserves_ratio() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "BON").await;
        // An uneven ratio (e.g. 3 bonus shares per 7 held) must round-trip exactly.
        db_upsert(&pool, &bonus(1, 1, d(2024, 11, 30), "3", "7")).await.unwrap();

        let got = db_get(&pool, 1).await.unwrap().unwrap();
        assert_eq!(got.action_type, ActionType::BonusIssue);
        assert_eq!(got.bonus_units, Some(Decimal::from(3)));
        assert_eq!(got.bonus_held_units, Some(Decimal::from(7)));
        assert_eq!(got.amount_per_unit, None);
        assert_eq!(got.currency, None);
        assert_eq!(got.split_new_units, None);
        assert_eq!(got.split_old_units, None);
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
        assert_eq!(all[0].amount_per_unit, Some("0.75".parse::<Decimal>().unwrap()));
    }

    #[tokio::test]
    async fn db_listing_fk_enforced() {
        let pool = test_pool().await;
        let err = db_upsert(&pool, &roc(1, 999, d(2024, 11, 30), "0.50")).await;
        assert!(err.is_err(), "unknown listing FK should be rejected");
    }

    #[tokio::test]
    async fn db_check_rejects_mixed_payloads() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "RAP").await;
        // A ShareSplit carrying a payment (or a ReturnOfCapital carrying a
        // ratio) violates the per-type CHECK even when written directly.
        let mut bad_split = split(1, 1, d(2024, 11, 30), "2", "1");
        bad_split.amount_per_unit = Some("0.50".parse().unwrap());
        bad_split.currency = Some("AUD".to_string());
        assert!(db_upsert(&pool, &bad_split).await.is_err());

        let mut bad_roc = roc(1, 1, d(2024, 11, 30), "0.50");
        bad_roc.split_new_units = Some(Decimal::from(2));
        bad_roc.split_old_units = Some(Decimal::ONE);
        assert!(db_upsert(&pool, &bad_roc).await.is_err());

        // …and a BonusIssue carrying a split ratio, or a ShareSplit carrying a
        // bonus ratio.
        let mut bad_bonus = bonus(1, 1, d(2024, 11, 30), "1", "10");
        bad_bonus.split_new_units = Some(Decimal::from(2));
        bad_bonus.split_old_units = Some(Decimal::ONE);
        assert!(db_upsert(&pool, &bad_bonus).await.is_err());

        let mut split_with_bonus = split(1, 1, d(2024, 11, 30), "2", "1");
        split_with_bonus.bonus_units = Some(Decimal::ONE);
        split_with_bonus.bonus_held_units = Some(Decimal::from(10));
        assert!(db_upsert(&pool, &split_with_bonus).await.is_err());
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
        assert_eq!(got.amount_per_unit, Some("0.50".parse::<Decimal>().unwrap()));

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
        assert_eq!(got.action_type, ActionType::ShareSplit);
        assert_eq!(got.split_new_units, Some(Decimal::from(2)));
        assert_eq!(got.split_old_units, Some(Decimal::ONE));
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
        assert_eq!(got.action_type, ActionType::BonusIssue);
        assert_eq!(got.bonus_units, Some(Decimal::ONE));
        assert_eq!(got.bonus_held_units, Some(Decimal::from(10)));
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
                "action_type": "RightsIssue",
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
