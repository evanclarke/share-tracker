//! Atomic rights exercise: turn a `RightsIssue` corporate action into a new
//! Buy parcel (see `docs/ato/rights-issues.md`).
//!
//! Exercising rights is no CGT event. The shares acquired are a new parcel
//! **acquired on the exercise date** — the 12-month CGT discount clock runs
//! from exercise, not from the rights or the original shares — with a cost
//! base of the amount paid to exercise (`units × exercise_price`) plus any
//! amount paid to acquire the exercised rights (`rights_cost`, nil for rights
//! issued free). The created trade carries the exercise payment as
//! `quantity × average_price` and the rights cost in `brokerage` (both are
//! first-element/incidental components of the single cost base every report
//! computes, so the parcel's cost base is exact). Shares from a rights issue
//! are allotted by the company, not market-settled, so the settlement date is
//! the exercise date.
//!
//! The entitlement is capped at write time: units held when the issue's
//! record date arrives (trades dated before `date`, in record-date units
//! across any splits) earn `rights_units` new units per `rights_held_units`
//! held, rounded **up** to a whole unit (registry practice for fractional
//! entitlements). Cumulative exercises against the same action — linked via
//! `trades.rights_action_id` — **plus rights sold or lapsed against it**
//! (`rights_sales`, see `entities::rights_sale`) may not exceed it; the two
//! operations share [`db_rights_used`] so their combined usage is capped
//! once. To keep that check honest, an exercise trade is immutable via
//! `PUT /trades` (delete it and re-exercise instead) and the action itself
//! is frozen while exercise trades reference it.
//!
//! Out of scope (documented in `docs/ato/rights-issues.md`): pre-CGT
//! originals and non-renounceable-offer retail premiums (an unfranked
//! dividend — entered as income, see `docs/ato/retail-premiums.md`).

use crate::entities::corporate_action::{
    self, ActionKind, SplitEvent, as_acquired_quantity, split_adjusted_quantity,
};
use crate::entities::trade::{self, Trade, TradeType};
use crate::infra::db::write_tx;
use crate::infra::decimal::{Money, parse_dec};
use crate::infra::http::ApiError;
use axum::{
    Json, Router,
    extract::{Path, State},
    http::StatusCode,
    routing::post,
};
use chrono::NaiveDate;
use rust_decimal::Decimal;
use serde::Deserialize;
use sqlx::{Row, SqlitePool};

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExerciseBody {
    /// Exercise date — the new parcel's acquisition date. Must not precede
    /// the issue's record date.
    pub date: NaiveDate,
    /// New units acquired (strictly positive).
    #[serde(deserialize_with = "crate::infra::decimal::strict_decimal")]
    pub units: Decimal,
    /// Total amount paid to acquire the exercised rights, in the action's
    /// currency (defaults to 0 — rights issued free have a nil cost base).
    #[serde(
        default,
        deserialize_with = "crate::infra::decimal::strict_optional_decimal"
    )]
    pub rights_cost: Option<Decimal>,
    /// Optional foreign-per-AUD override for the created trade (defaults to
    /// 1; reports prefer the ATO rate and fall back to this — see `infra::fx`).
    #[serde(
        default,
        deserialize_with = "crate::infra::decimal::strict_optional_decimal"
    )]
    pub fx_rate: Option<Decimal>,
    /// The holding account the exercised parcel lands in. Defaults to the
    /// seeded default account when omitted.
    #[serde(default = "crate::entities::holding_account::default_holding_account_id")]
    pub holding_account_id: i64,
}

#[derive(thiserror::Error, Debug)]
pub enum ExerciseError {
    #[error("rights exercise write failed: {0}")]
    Db(#[from] sqlx::Error),
    /// No corporate action with that id.
    #[error("no corporate action with that id")]
    ActionNotFound,
    /// The action is not a RightsIssue.
    #[error("that corporate action is not a rights issue")]
    NotARightsIssue,
    /// `units` is not strictly positive.
    #[error("the number of units exercised must be greater than zero")]
    NonPositiveUnits,
    /// `rights_cost` is negative.
    #[error("the rights cost cannot be negative")]
    NegativeRightsCost,
    /// `exercise_price × units + rights_cost` — the exercised parcel's whole
    /// cost base, which the Buy carries as `average_price × quantity +
    /// brokerage` — cannot be represented (SCENARIOS W-e). Nothing multiplies
    /// these two at write time, so the row was accepted `201` and it was the
    /// portfolio and gains reports that died on it. The bound and its wording
    /// are `domain::cost_base`'s, shared with `trade::check_amounts` — which
    /// this path does not go through, since the Buy is written directly.
    #[error("the exercised parcel's cost base is not representable: {0}")]
    UnrepresentableCostBase(#[source] crate::domain::cost_base::UnrepresentableCost),
    /// The exercise date precedes the issue's record date.
    #[error("the exercise date is before the issue's record date")]
    BeforeRecordDate,
    /// Cumulative exercised units would exceed the entitlement earned by the
    /// units held at the record date.
    #[error("the units exercised exceed the entitlement earned at the record date")]
    ExceedsEntitlement,
    /// The exercise Buy is dated on or before an executed whole-holding
    /// operation of its listing — a scrip-for-scrip exchange, a demerger, or a
    /// worthless-shares recognise. Each consumed every parcel open at its own
    /// date, so an exercise behind one can never be consumed and stays open
    /// forever (SCENARIOS V-d). The rights side of
    /// `trade::UpsertError::BackDatedOverWholeHolding`; wording and recovery in
    /// `domain::whole_holding`. Mapped to 422.
    #[error("this parcel is dated behind a whole-holding operation: {0}")]
    BackDatedOverWholeHolding(#[source] crate::domain::whole_holding::BackDatedParcel),
    /// The exercised quantity is one the listing's recorded splits and bonus
    /// issues re-base past what a `Decimal` can hold. The rights side of
    /// `trade::UpsertError::UnrepresentableRebasedQuantity`, and not the same
    /// bound as [`UnrepresentableCostBase`](ExerciseError::UnrepresentableCostBase)
    /// above it: that one bounds `exercise_price × units`, which a nil-ish
    /// exercise price leaves free to be any unit count at all. Same walk, same
    /// wording (`corporate_action::rebased_quantity_beyond_range`). Mapped to
    /// 422.
    #[error("this parcel's quantity re-bases beyond the representable range: {0}")]
    UnrepresentableRebasedQuantity(#[source] crate::domain::cost_base::UnrepresentableQuantity),
}

impl From<ExerciseError> for ApiError {
    fn from(e: ExerciseError) -> Self {
        match e {
            ExerciseError::ActionNotFound => {
                ApiError::not_found("no corporate action with that id")
            }
            ExerciseError::NotARightsIssue => {
                ApiError::unprocessable("that corporate action is not a rights issue")
            }
            ExerciseError::NonPositiveUnits => {
                ApiError::unprocessable("the number of units exercised must be greater than zero")
            }
            ExerciseError::NegativeRightsCost => {
                ApiError::unprocessable("the rights cost cannot be negative")
            }
            ExerciseError::UnrepresentableCostBase(e) => ApiError::unprocessable(e.message()),
            ExerciseError::BeforeRecordDate => {
                ApiError::unprocessable("the exercise date is before the issue's record date")
            }
            ExerciseError::ExceedsEntitlement => ApiError::unprocessable(
                "the units exercised exceed the entitlement earned by the holding at the record \
                 date",
            ),
            // The same body every parcel-creating path answers for this fact.
            ExerciseError::BackDatedOverWholeHolding(e) => ApiError::Unprocessable(e.message()),
            ExerciseError::UnrepresentableRebasedQuantity(e) => {
                ApiError::Unprocessable(e.message())
            }
            ExerciseError::Db(err) => err.into(),
        }
    }
}

pub fn router() -> Router<SqlitePool> {
    Router::new().route("/corporate_actions/{id}/exercise", post(exercise))
}

/// Units of the listing held when the issue's record date arrived, in
/// record-date units. Trades dated before the record date count (one dated on
/// it is ex-rights — the same half-open convention as splits/bonus issues);
/// each quantity is re-based to record-date units across any
/// splits/consolidations so buys, sells, prior usage, and the cap all
/// compare in one basis.
pub(crate) async fn db_held_at_record_date(
    conn: &mut sqlx::SqliteConnection,
    listing_id: i64,
    record_date: NaiveDate,
    splits: &[SplitEvent],
) -> Result<Decimal, sqlx::Error> {
    let held_rows = sqlx::query(
        "SELECT trade_type, date, quantity FROM trades \
         WHERE listing_id = ? AND date < ?",
    )
    .bind(listing_id)
    .bind(record_date)
    .fetch_all(&mut *conn)
    .await?;
    let mut held = Decimal::ZERO;
    for row in &held_rows {
        let trade_date: NaiveDate = row.try_get("date")?;
        let qty = parse_dec("quantity", row.try_get("quantity")?)?;
        let in_record_units = split_adjusted_quantity(qty, splits, trade_date, Some(record_date));
        match row.try_get::<TradeType, _>("trade_type")? {
            TradeType::Buy | TradeType::DRP => held += in_record_units,
            TradeType::Sell => held -= in_record_units,
        }
    }
    Ok(held)
}

/// The entitlement a holding earns under the issue's terms. Fractional
/// entitlements round up to a whole unit (registry practice), so the cap is
/// never tighter than the offer's own rounding.
///
/// `None` where the entitlement itself is past `Decimal`'s range — a 1000-for-1
/// issue over a holding of 1e27 units earns 1e30 rights. That is deliberately
/// *not* a refusal, unlike the replacement quantity a scrip exchange or
/// demerger would have to store: this figure is never written anywhere, and
/// exists only to answer *has the holder used more rights than they were
/// entitled to?* — to which an unrepresentable cap gives an exact answer, since
/// every representable request is below it. Refusing here would deny a
/// perfectly ordinary 100-unit exercise because of arithmetic the user never
/// asked for. Callers therefore read `None` as *no representable request can
/// exceed this*.
pub(crate) fn entitled_units(
    held: Decimal,
    rights_units: Decimal,
    rights_held_units: Decimal,
) -> Option<Decimal> {
    Some(
        crate::domain::cost_base::checked_rebased_quantity(
            ("units held", held.max(Decimal::ZERO)),
            ("rights_units", rights_units),
            ("rights_held_units", rights_held_units),
        )
        .ok()?
        .ceil(),
    )
}

/// Rights already used against the action, in record-date units: exercised
/// units (trades linked via `rights_action_id`, re-based across any splits
/// between the record date and each exercise) plus rights sold or lapsed
/// (`rights_sales.units`, stored in record-date rights units). The exercise
/// and sell-rights operations both validate against this, so their combined
/// usage can never exceed the entitlement.
pub(crate) async fn db_rights_used(
    conn: &mut sqlx::SqliteConnection,
    action_id: i64,
    record_date: NaiveDate,
    splits: &[SplitEvent],
) -> Result<Decimal, sqlx::Error> {
    let prior_rows = sqlx::query("SELECT date, quantity FROM trades WHERE rights_action_id = ?")
        .bind(action_id)
        .fetch_all(&mut *conn)
        .await?;
    let mut used = Decimal::ZERO;
    for row in &prior_rows {
        let trade_date: NaiveDate = row.try_get("date")?;
        let qty = parse_dec("quantity", row.try_get("quantity")?)?;
        used += as_acquired_quantity(qty, splits, record_date, trade_date);
    }
    let sold_units: Vec<String> =
        sqlx::query_scalar("SELECT units FROM rights_sales WHERE rights_action_id = ?")
            .bind(action_id)
            .fetch_all(&mut *conn)
            .await?;
    for units in sold_units {
        used += parse_dec("units", units)?;
    }
    Ok(used)
}

/// Create the Buy trade for a rights exercise, atomically.
pub async fn db_exercise(
    pool: &SqlitePool,
    action_id: i64,
    body: &ExerciseBody,
) -> Result<Trade, ExerciseError> {
    if body.units <= Decimal::ZERO {
        return Err(ExerciseError::NonPositiveUnits);
    }
    let rights_cost = body.rights_cost.unwrap_or(Decimal::ZERO);
    if rights_cost < Decimal::ZERO {
        return Err(ExerciseError::NegativeRightsCost);
    }

    let mut tx = write_tx(pool).await?;

    let action = match corporate_action::db_get_tx(&mut *tx, action_id).await? {
        Some(a) => a,
        None => return Err(ExerciseError::ActionNotFound),
    };
    let (rights_units, rights_held_units, exercise_price, currency) = match &action.kind {
        // The offer's renounceability is deliberately not read here:
        // exercising is identical under both (`docs/ato/rights-issues.md` —
        // the exercise rules turn on how the rights were acquired and on the
        // original shares' pre/post-CGT status, never on renounceability), so
        // a non-renounceable entitlement offer is exercised exactly like any
        // other. It is the *disposal* path that turns on it
        // (`entities::rights_sale`).
        ActionKind::RightsIssue {
            rights_units,
            rights_held_units,
            exercise_price,
            currency,
            ..
        } => (
            *rights_units,
            *rights_held_units,
            *exercise_price,
            currency.clone(),
        ),
        _ => return Err(ExerciseError::NotARightsIssue),
    };
    let record_date = action.date;
    if body.date < record_date {
        return Err(ExerciseError::BeforeRecordDate);
    }

    // A whole-holding operation of this listing that has already run consumed
    // every parcel open at its own date and cannot reach back for this one, so
    // an exercise Buy dated on or before it would stay open forever
    // (SCENARIOS V-d). Compared on the Buy's own date, the exercise date
    // (`domain::whole_holding`).
    if let Some(back_dated) = crate::domain::whole_holding::db_back_dated_parcel(
        &mut tx,
        action.listing_id,
        body.date,
        None,
    )
    .await?
    {
        return Err(ExerciseError::BackDatedOverWholeHolding(back_dated));
    }

    let splits = corporate_action::db_splits_for_listing(&mut *tx, action.listing_id).await?;
    let held = db_held_at_record_date(&mut tx, action.listing_id, record_date, &splits).await?;
    let entitled = entitled_units(held, rights_units, rights_held_units);

    // Rights already used against this action (prior exercises + rights
    // sales) plus this exercise, re-based to record-date units.
    let mut used = db_rights_used(&mut tx, action_id, record_date, &splits).await?;
    used += as_acquired_quantity(body.units, &splits, record_date, body.date);
    // `None` means the entitlement is past `Decimal`'s range, so nothing the
    // request can name reaches it (`entitled_units`).
    if entitled.is_some_and(|entitled| used > entitled) {
        return Err(ExerciseError::ExceedsEntitlement);
    }

    // What the exercise costs — the parcel's cost base, stored as the Buy's
    // price × quantity plus the rights cost on its `brokerage` column. Nothing
    // multiplies the pair on the way in, so without this an unrepresentable
    // one is stored happily and every report that costs the parcel answers a
    // logged `500` (SCENARIOS W-e). Same bound as `trade::check_amounts`,
    // which this path does not go through.
    crate::domain::cost_base::checked_cost_base(&[
        crate::domain::cost_base::Term::Product {
            price: ("exercise_price", exercise_price),
            units: ("units", body.units),
        },
        crate::domain::cost_base::Term::Amount("rights_cost", rights_cost),
    ])
    .map_err(ExerciseError::UnrepresentableCostBase)?;

    let fx_rate = body.fx_rate.unwrap_or(Decimal::ONE);
    let result = sqlx::query(
        "INSERT INTO trades \
         (trade_type, date, settlement_date, settlement_date_source, listing_id, \
          average_price, quantity, currency, \
          brokerage, gst_on_brokerage, brokerage_currency, fx_rate, contract_note_ref, \
          residual_brought_forward, residual_carried_forward, residual_paid_out, \
          rights_action_id, holding_account_id) \
         VALUES ('Buy', ?, ?, 'stated', ?, ?, ?, ?, ?, '0', ?, ?, NULL, '0', '0', '0', ?, ?)",
    )
    .bind(body.date)
    .bind(body.date)
    .bind(action.listing_id)
    .bind(Money(exercise_price))
    .bind(Money(body.units))
    .bind(&currency)
    .bind(Money(rights_cost))
    .bind(&currency)
    .bind(Money(fx_rate))
    .bind(action_id)
    .bind(body.holding_account_id)
    .execute(&mut *tx)
    .await?;
    let new_id = result.last_insert_rowid();

    // The listing's recorded splits and bonus issues are re-applied at *read*
    // time, so an exercised quantity they push past `Decimal`'s range is
    // accepted here and then answers a logged `500` from every open-holdings
    // report of the whole portfolio. Checked over the state this write leaves
    // behind, like every other parcel-creating path
    // (`corporate_action::rebased_quantity_beyond_range`).
    if let Some(beyond) =
        crate::entities::corporate_action::rebased_quantity_beyond_range(&mut tx, action.listing_id)
            .await?
    {
        return Err(ExerciseError::UnrepresentableRebasedQuantity(beyond));
    }

    tx.commit().await?;

    // Read the freshly created trade back so the response is exactly what was stored.
    trade::db_get(pool, new_id)
        .await?
        .ok_or_else(|| ExerciseError::Db(sqlx::Error::RowNotFound))
}

async fn exercise(
    State(pool): State<SqlitePool>,
    Path(action_id): Path<i64>,
    Json(body): Json<ExerciseBody>,
) -> Result<(StatusCode, Json<Trade>), ApiError> {
    let trade = db_exercise(&pool, action_id, &body).await?;
    Ok((StatusCode::CREATED, Json(trade)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::entities::{corporate_action::CorporateAction, listing, sell};
    use crate::test_support::{self, ApiClient, test_pool};

    fn d(y: i32, m: u32, day: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, day).unwrap()
    }

    async fn insert_listing(pool: &SqlitePool, id: i64) {
        test_support::listing(id)
            .ticker("RTS")
            .name("Rights Test Co")
            .security_type(listing::SecurityType::Share)
            .insert(pool)
            .await;
    }

    async fn insert_buy(pool: &SqlitePool, id: i64, date: NaiveDate, qty: &str, price: &str) {
        test_support::buy(id, 1)
            .date(date)
            .settlement(date)
            .qty(qty.parse().unwrap())
            .price(price.parse().unwrap())
            .insert(pool)
            .await;
    }

    async fn insert_sell(
        pool: &SqlitePool,
        id: i64,
        date: NaiveDate,
        qty: &str,
        parcel_trade_id: i64,
    ) {
        sell::db_upsert_sell(
            pool,
            id,
            &sell::SellBody {
                brokerage_includes_gst: false,
                statement_total: None,
                holding_account_id: 1,
                date,
                settlement_date: Some(date),
                listing_id: 1,
                average_price: "2.00".parse().unwrap(),
                quantity: qty.parse().unwrap(),
                currency: "AUD".to_string(),
                brokerage: Decimal::ZERO,
                gst_on_brokerage: Decimal::ZERO,
                brokerage_currency: "AUD".to_string(),
                fx_rate: Decimal::ONE,
                spot_fx_rate: None,
                contract_note_ref: None,
                allocations: vec![sell::AllocationInput {
                    purchase_trade_id: parcel_trade_id,
                    quantity_allocated: qty.parse().unwrap(),
                }],
            },
        )
        .await
        .unwrap();
    }

    /// A 1-for-4 rights issue at $1.80, record date `date`.
    async fn insert_rights_issue(pool: &SqlitePool, id: i64, date: NaiveDate) {
        corporate_action::db_upsert(
            pool,
            &CorporateAction {
                id,
                listing_id: 1,
                date,
                kind: ActionKind::RightsIssue {
                    rights_units: Decimal::ONE,
                    rights_held_units: Decimal::from(4),
                    exercise_price: "1.80".parse().unwrap(),
                    currency: "AUD".to_string(),
                    renounceable: true,
                },
            },
        )
        .await
        .unwrap();
    }

    fn body(date: NaiveDate, units: &str) -> ExerciseBody {
        ExerciseBody {
            holding_account_id: 1,
            date,
            units: units.parse().unwrap(),
            rights_cost: None,
            fx_rate: None,
        }
    }

    // DB-level tests

    #[tokio::test]
    async fn exercise_creates_a_buy_parcel_at_the_exercise_date() {
        let pool = test_pool().await;
        insert_listing(&pool, 1).await;
        insert_buy(&pool, 1, d(2024, 1, 15), "1000", "2.00").await;
        insert_rights_issue(&pool, 10, d(2024, 7, 1)).await;

        let trade = db_exercise(&pool, 10, &body(d(2024, 8, 1), "250"))
            .await
            .unwrap();
        assert_eq!(trade.trade_type, TradeType::Buy);
        // Acquired on the exercise date (the discount clock runs from here),
        // allotted by the company so settlement is the same day.
        assert_eq!(trade.date, d(2024, 8, 1));
        assert_eq!(trade.settlement_date, d(2024, 8, 1));
        assert_eq!(trade.listing_id, 1);
        assert_eq!(trade.quantity, Decimal::from(250));
        assert_eq!(trade.average_price, "1.80".parse::<Decimal>().unwrap());
        assert_eq!(trade.brokerage, Decimal::ZERO);
        assert_eq!(trade.currency, "AUD");
        assert_eq!(trade.fx_rate, Decimal::ONE);
        assert_eq!(trade.rights_action_id, Some(10));

        // The parcel's cost base is exactly the amount paid to exercise.
        let parcels = crate::reports::open_parcels::db_open_parcels(&pool)
            .await
            .unwrap();
        let parcel = parcels.iter().find(|p| p.trade_id == trade.id).unwrap();
        assert_eq!(parcel.acquisition_date, d(2024, 8, 1));
        assert_eq!(parcel.original_cost_base, Decimal::from(450)); // 250 × 1.80
    }

    /// An amount paid to acquire the rights (the purchased-rights case) is
    /// the rights' cost base at exercise — part of the new parcel's first
    /// element, carried on the trade's brokerage column.
    #[tokio::test]
    async fn rights_cost_is_part_of_the_new_parcels_cost_base() {
        let pool = test_pool().await;
        insert_listing(&pool, 1).await;
        insert_buy(&pool, 1, d(2024, 1, 15), "1000", "2.00").await;
        insert_rights_issue(&pool, 10, d(2024, 7, 1)).await;

        let exercise = ExerciseBody {
            holding_account_id: 1,
            date: d(2024, 8, 1),
            units: "250".parse().unwrap(),
            rights_cost: Some("50.05".parse().unwrap()),
            fx_rate: None,
        };
        let trade = db_exercise(&pool, 10, &exercise).await.unwrap();
        assert_eq!(trade.brokerage, "50.05".parse::<Decimal>().unwrap());

        let parcels = crate::reports::open_parcels::db_open_parcels(&pool)
            .await
            .unwrap();
        let parcel = parcels.iter().find(|p| p.trade_id == trade.id).unwrap();
        // 250 × 1.80 + 50.05
        assert_eq!(
            parcel.original_cost_base,
            "500.05".parse::<Decimal>().unwrap()
        );
    }

    #[tokio::test]
    async fn cumulative_exercises_cannot_exceed_the_entitlement() {
        let pool = test_pool().await;
        insert_listing(&pool, 1).await;
        insert_buy(&pool, 1, d(2024, 1, 15), "1000", "2.00").await;
        insert_rights_issue(&pool, 10, d(2024, 7, 1)).await; // entitled to 250

        db_exercise(&pool, 10, &body(d(2024, 7, 10), "100"))
            .await
            .unwrap();
        db_exercise(&pool, 10, &body(d(2024, 8, 1), "150"))
            .await
            .unwrap();
        let err = db_exercise(&pool, 10, &body(d(2024, 8, 2), "1")).await;
        assert!(matches!(err, Err(ExerciseError::ExceedsEntitlement)));

        // Only the two valid exercises persisted.
        let n: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM trades WHERE rights_action_id = 10")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(n, 2);
    }

    /// Entitlement counts units held when the record date arrives: a sale
    /// before it reduces the holding, and a buy dated *on* the record date is
    /// ex-rights (the same half-open convention as splits/bonus issues).
    #[tokio::test]
    async fn entitlement_reflects_holdings_at_the_record_date() {
        let pool = test_pool().await;
        insert_listing(&pool, 1).await;
        insert_buy(&pool, 1, d(2024, 1, 15), "1000", "2.00").await;
        insert_sell(&pool, 2, d(2024, 5, 1), "600", 1).await;
        insert_buy(&pool, 3, d(2024, 7, 1), "400", "2.00").await; // ex-rights
        insert_rights_issue(&pool, 10, d(2024, 7, 1)).await;

        // Held at the record date = 1000 − 600 = 400 → entitled to 100.
        let err = db_exercise(&pool, 10, &body(d(2024, 8, 1), "101")).await;
        assert!(matches!(err, Err(ExerciseError::ExceedsEntitlement)));
        db_exercise(&pool, 10, &body(d(2024, 8, 1), "100"))
            .await
            .unwrap();
    }

    /// A split between acquisition and the record date re-bases the holding
    /// into record-date units before the entitlement ratio applies.
    #[tokio::test]
    async fn split_before_the_record_date_rebases_the_entitlement() {
        let pool = test_pool().await;
        insert_listing(&pool, 1).await;
        insert_buy(&pool, 1, d(2024, 1, 15), "100", "2.00").await;
        corporate_action::db_upsert(
            &pool,
            &CorporateAction {
                id: 5,
                listing_id: 1,
                date: d(2024, 4, 1),
                kind: ActionKind::ShareSplit {
                    split_new_units: Decimal::from(2),
                    split_old_units: Decimal::ONE,
                },
            },
        )
        .await
        .unwrap();
        insert_rights_issue(&pool, 10, d(2024, 7, 1)).await;

        // 100 as-acquired units are 200 record-date units → entitled to 50.
        let err = db_exercise(&pool, 10, &body(d(2024, 8, 1), "51")).await;
        assert!(matches!(err, Err(ExerciseError::ExceedsEntitlement)));
        db_exercise(&pool, 10, &body(d(2024, 8, 1), "50"))
            .await
            .unwrap();
    }

    /// SCENARIOS E-12: a split between the record date and the exercise. The
    /// entitlement is fixed in record-date units, and each exercise is
    /// re-based back into that basis before the cap is applied — so the
    /// 25-right entitlement is exercised as 50 post-split units, and not one
    /// unit more.
    #[tokio::test]
    async fn split_after_the_record_date_rebases_each_exercise() {
        let pool = test_pool().await;
        insert_listing(&pool, 1).await;
        insert_buy(&pool, 1, d(2024, 1, 15), "100", "2.00").await;
        insert_rights_issue(&pool, 10, d(2024, 7, 1)).await; // 100 units → 25 rights
        corporate_action::db_upsert(
            &pool,
            &CorporateAction {
                id: 5,
                listing_id: 1,
                date: d(2024, 7, 15),
                kind: ActionKind::ShareSplit {
                    split_new_units: Decimal::from(2),
                    split_old_units: Decimal::ONE,
                },
            },
        )
        .await
        .unwrap();

        // Half the entitlement, then the rest, then one unit too many.
        db_exercise(&pool, 10, &body(d(2024, 8, 1), "20"))
            .await
            .unwrap();
        db_exercise(&pool, 10, &body(d(2024, 8, 2), "30"))
            .await
            .unwrap();
        let err = db_exercise(&pool, 10, &body(d(2024, 8, 3), "1")).await;
        assert!(matches!(err, Err(ExerciseError::ExceedsEntitlement)));
    }

    /// A fractional entitlement rounds up to a whole unit, so the cap is
    /// never tighter than the offer's own rounding.
    #[tokio::test]
    async fn fractional_entitlements_round_up() {
        let pool = test_pool().await;
        insert_listing(&pool, 1).await;
        insert_buy(&pool, 1, d(2024, 1, 15), "1000", "2.00").await;
        // 3-for-7: 1000 × 3 / 7 = 428.57… → entitled to 429.
        corporate_action::db_upsert(
            &pool,
            &CorporateAction {
                id: 10,
                listing_id: 1,
                date: d(2024, 7, 1),
                kind: ActionKind::RightsIssue {
                    rights_units: Decimal::from(3),
                    rights_held_units: Decimal::from(7),
                    exercise_price: "1.80".parse().unwrap(),
                    currency: "AUD".to_string(),
                    renounceable: true,
                },
            },
        )
        .await
        .unwrap();

        let err = db_exercise(&pool, 10, &body(d(2024, 8, 1), "430")).await;
        assert!(matches!(err, Err(ExerciseError::ExceedsEntitlement)));
        db_exercise(&pool, 10, &body(d(2024, 8, 1), "429"))
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn invalid_exercises_are_rejected_and_nothing_persisted() {
        let pool = test_pool().await;
        insert_listing(&pool, 1).await;
        insert_buy(&pool, 1, d(2024, 1, 15), "1000", "2.00").await;
        insert_rights_issue(&pool, 10, d(2024, 7, 1)).await;
        // A non-rights action on the same listing.
        corporate_action::db_upsert(
            &pool,
            &CorporateAction {
                id: 11,
                listing_id: 1,
                date: d(2024, 7, 1),
                kind: ActionKind::ShareSplit {
                    split_new_units: Decimal::from(2),
                    split_old_units: Decimal::ONE,
                },
            },
        )
        .await
        .unwrap();

        let err = db_exercise(&pool, 999, &body(d(2024, 8, 1), "10")).await;
        assert!(matches!(err, Err(ExerciseError::ActionNotFound)));
        let err = db_exercise(&pool, 11, &body(d(2024, 8, 1), "10")).await;
        assert!(matches!(err, Err(ExerciseError::NotARightsIssue)));
        let err = db_exercise(&pool, 10, &body(d(2024, 6, 30), "10")).await;
        assert!(matches!(err, Err(ExerciseError::BeforeRecordDate)));
        let err = db_exercise(&pool, 10, &body(d(2024, 8, 1), "0")).await;
        assert!(matches!(err, Err(ExerciseError::NonPositiveUnits)));
        let err = db_exercise(&pool, 10, &body(d(2024, 8, 1), "-5")).await;
        assert!(matches!(err, Err(ExerciseError::NonPositiveUnits)));
        let exercise = ExerciseBody {
            holding_account_id: 1,
            date: d(2024, 8, 1),
            units: "10".parse().unwrap(),
            rights_cost: Some("-1".parse().unwrap()),
            fx_rate: None,
        };
        let err = db_exercise(&pool, 10, &exercise).await;
        assert!(matches!(err, Err(ExerciseError::NegativeRightsCost)));

        let n: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM trades WHERE rights_action_id IS NOT NULL")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(n, 0, "no rejected exercise may persist a trade");
    }

    /// An exercise trade was validated against the entitlement, so free-form
    /// edits are rejected; deleting it frees the entitlement again.
    #[tokio::test]
    async fn exercise_trade_is_immutable_via_put_trades_but_deletable() {
        let pool = test_pool().await;
        insert_listing(&pool, 1).await;
        insert_buy(&pool, 1, d(2024, 1, 15), "1000", "2.00").await;
        insert_rights_issue(&pool, 10, d(2024, 7, 1)).await;
        let created = db_exercise(&pool, 10, &body(d(2024, 8, 1), "250"))
            .await
            .unwrap();

        // Any edit — even one keeping the same figures — is rejected.
        let mut edited = created.clone();
        edited.quantity = Decimal::from(9999);
        let err = trade::db_upsert(&pool, &edited).await;
        assert!(matches!(err, Err(trade::UpsertError::RightsExerciseTrade)));
        let err = trade::db_upsert(&pool, &created).await;
        assert!(matches!(err, Err(trade::UpsertError::RightsExerciseTrade)));

        // Deleting the trade frees the entitlement: a fresh exercise works.
        assert_eq!(
            trade::db_delete(&pool, created.id).await.unwrap(),
            trade::DeleteOutcome::Deleted
        );
        db_exercise(&pool, 10, &body(d(2024, 8, 2), "250"))
            .await
            .unwrap();
    }

    /// The action the exercises were validated against is frozen while they
    /// reference it: editing or deleting it is rejected until they are gone.
    #[tokio::test]
    async fn referenced_action_cannot_be_edited_or_deleted() {
        let pool = test_pool().await;
        insert_listing(&pool, 1).await;
        insert_buy(&pool, 1, d(2024, 1, 15), "1000", "2.00").await;
        insert_rights_issue(&pool, 10, d(2024, 7, 1)).await;
        let created = db_exercise(&pool, 10, &body(d(2024, 8, 1), "250"))
            .await
            .unwrap();

        let action = corporate_action::db_get(&pool, 10).await.unwrap().unwrap();
        let err = corporate_action::db_upsert(&pool, &action).await;
        assert!(matches!(
            err,
            Err(corporate_action::WriteError::ReferencedByTrade)
        ));
        let err = corporate_action::db_delete(&pool, 10).await;
        assert!(
            err.is_err(),
            "the trades.rights_action_id FK must block the delete"
        );

        // Removing the exercise trade unfreezes the action.
        trade::db_delete(&pool, created.id).await.unwrap();
        corporate_action::db_upsert(&pool, &action).await.unwrap();
        assert!(corporate_action::db_delete(&pool, 10).await.unwrap());
    }

    /// The 12-month discount clock runs from the exercise date, not from the
    /// original shares or the rights (docs/ato/rights-issues.md): a sale more
    /// than 12 months after the original buy but within 12 months of the
    /// exercise is non-discountable.
    #[tokio::test]
    async fn discount_clock_runs_from_the_exercise_date() {
        let pool = test_pool().await;
        insert_listing(&pool, 1).await;
        insert_buy(&pool, 1, d(2023, 1, 10), "1000", "2.00").await;
        insert_rights_issue(&pool, 10, d(2024, 7, 1)).await;
        let parcel = db_exercise(&pool, 10, &body(d(2024, 8, 1), "250"))
            .await
            .unwrap();

        // Sold at a gain ~5 months after exercise (~2 years after the
        // original buy that earned the rights).
        insert_sell(&pool, 50, d(2025, 1, 10), "250", parcel.id).await;

        let gains = crate::reports::realised_gains::db_realised_gains(&pool)
            .await
            .unwrap();
        let sale = gains.iter().find(|g| g.sale_trade_id == 50).unwrap();
        assert!(sale.capital_gain_loss > Decimal::ZERO);
        assert_eq!(sale.discount_eligible_gain, Decimal::ZERO);
        assert_eq!(sale.non_discountable_gain, sale.capital_gain_loss);
    }

    /// The exercise says which holding account it acts in: the exercised
    /// parcel lands there, not in the default account.
    #[tokio::test]
    async fn exercise_lands_in_the_chosen_holding_account() {
        use crate::entities::holding_account::{self, HoldingAccount};
        let pool = test_pool().await;
        insert_listing(&pool, 1).await;
        holding_account::db_upsert(
            &pool,
            &HoldingAccount {
                id: 2,
                name: "Personal CHESS".to_string(),
            },
        )
        .await
        .unwrap();
        insert_buy(&pool, 1, d(2024, 1, 10), "1000", "8.00").await;
        insert_rights_issue(&pool, 10, d(2024, 3, 1)).await;

        let mut exercise = body(d(2024, 4, 1), "100");
        exercise.holding_account_id = 2;
        let trade = db_exercise(&pool, 10, &exercise).await.unwrap();
        assert_eq!(trade.holding_account_id, 2);
    }

    // API-level tests

    #[tokio::test]
    async fn api_exercise_returns_201_with_the_created_trade() {
        let pool = test_pool().await;
        insert_listing(&pool, 1).await;
        insert_buy(&pool, 1, d(2024, 1, 15), "1000", "2.00").await;
        insert_rights_issue(&pool, 10, d(2024, 7, 1)).await;

        let resp = ApiClient::over(router().with_state(pool.clone()))
            .post(
                "/corporate_actions/10/exercise",
                &serde_json::json!({
                    "date": "2024-08-01",
                    "units": "250",
                    "rights_cost": "10.50",
                }),
            )
            .await;
        assert_eq!(resp.status, StatusCode::CREATED);
        let trade: Trade = resp.json();
        assert_eq!(trade.quantity, Decimal::from(250));
        assert_eq!(trade.brokerage, "10.50".parse::<Decimal>().unwrap());
        assert_eq!(trade.rights_action_id, Some(10));
        assert!(trade::db_get(&pool, trade.id).await.unwrap().is_some());
    }

    async fn api_exercise_expecting(
        pool: &SqlitePool,
        action_id: i64,
        body: serde_json::Value,
        expected: StatusCode,
    ) {
        let resp = ApiClient::over(router().with_state(pool.clone()))
            .post(format!("/corporate_actions/{action_id}/exercise"), &body)
            .await;
        assert_eq!(resp.status, expected);
        // Every client-error rejection carries a reason for the toast.
        if expected.is_client_error() {
            assert!(
                !resp.body.is_empty(),
                "a {expected} rejection must carry a reason body"
            );
        }
    }

    #[tokio::test]
    async fn api_invalid_exercises_return_404_or_422() {
        let pool = test_pool().await;
        insert_listing(&pool, 1).await;
        insert_buy(&pool, 1, d(2024, 1, 15), "1000", "2.00").await;
        insert_rights_issue(&pool, 10, d(2024, 7, 1)).await;

        let ok = serde_json::json!({ "date": "2024-08-01", "units": "250" });
        api_exercise_expecting(&pool, 999, ok.clone(), StatusCode::NOT_FOUND).await;
        for body in [
            serde_json::json!({ "date": "2024-08-01", "units": "251" }),
            serde_json::json!({ "date": "2024-08-01", "units": "0" }),
            serde_json::json!({ "date": "2024-06-30", "units": "250" }),
            serde_json::json!({ "date": "2024-08-01", "units": "250", "rights_cost": "-1" }),
        ] {
            api_exercise_expecting(&pool, 10, body, StatusCode::UNPROCESSABLE_ENTITY).await;
        }

        // PUT /trades on the exercise trade → 422 (immutable); the frozen
        // action → 422 on PUT and DELETE.
        api_exercise_expecting(&pool, 10, ok, StatusCode::CREATED).await;
        let trade_id: i64 = sqlx::query_scalar("SELECT id FROM trades WHERE rights_action_id = 10")
            .fetch_one(&pool)
            .await
            .unwrap();
        let app = ApiClient::over(crate::entities::router().with_state(pool.clone()));
        let resp = app
            .put(
                format!("/trades/{trade_id}"),
                &serde_json::json!({
                    "trade_type": "Buy", "date": "2024-08-01",
                    "settlement_date": "2024-08-01", "listing_id": 1,
                    "average_price": "1.80", "quantity": "9999",
                    "currency": "AUD", "brokerage": "0",
                    "gst_on_brokerage": "0", "brokerage_currency": "AUD",
                    "fx_rate": "1",
                }),
            )
            .await;
        assert_eq!(resp.status, StatusCode::UNPROCESSABLE_ENTITY);
        let resp = app
            .put(
                "/corporate_actions/10",
                &serde_json::json!({
                    "action_type": "RightsIssue", "listing_id": 1,
                    "date": "2024-07-01", "rights_units": "1",
                    "rights_held_units": "4", "exercise_price": "1.80",
                    "currency": "AUD", "renounceable": true,
                }),
            )
            .await;
        assert_eq!(resp.status, StatusCode::UNPROCESSABLE_ENTITY);
        let resp = app.delete("/corporate_actions/10").await;
        assert_eq!(resp.status, StatusCode::UNPROCESSABLE_ENTITY);
    }

    /// SCENARIOS V-d: an exercise Buy is dated by the body, so it can land
    /// behind a whole-holding operation of the listing that has already run —
    /// units the operation could never consume. Refused with the same `422`
    /// every parcel-creating path answers, and nothing is written.
    #[tokio::test]
    async fn exercise_dated_before_an_executed_recognise_is_refused() {
        let pool = test_pool().await;
        test_support::recognised_worthless_listing(
            &pool,
            1,
            "DEAD",
            d(2024, 1, 2),
            90,
            d(2024, 12, 2),
        )
        .await;
        // A rights issue announced while the company was still alive.
        corporate_action::db_upsert(
            &pool,
            &CorporateAction {
                id: 91,
                listing_id: 1,
                date: d(2024, 2, 6),
                kind: ActionKind::RightsIssue {
                    rights_units: Decimal::ONE,
                    rights_held_units: Decimal::ONE,
                    exercise_price: Decimal::ONE,
                    currency: "AUD".to_string(),
                    renounceable: true,
                },
            },
        )
        .await
        .unwrap();

        let body = ExerciseBody {
            date: d(2024, 3, 5),
            units: Decimal::from(10),
            rights_cost: None,
            fx_rate: None,
            holding_account_id: 1,
        };
        let err = db_exercise(&pool, 91, &body).await.unwrap_err();
        assert!(
            matches!(err, ExerciseError::BackDatedOverWholeHolding(_)),
            "expected the whole-holding refusal, got: {err:?}"
        );

        let response = ApiClient::over(router().with_state(pool.clone()))
            .post(
                "/corporate_actions/91/exercise",
                &serde_json::json!({"date": "2024-03-05", "units": "10"}),
            )
            .await;
        let (status, detail) = response.status_and_body();
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
        assert!(detail.contains("worthless-shares recognise"), "{detail}");
        let exercised: bool =
            sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM trades WHERE rights_action_id = 91)")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert!(!exercised);
    }

    /// The exercised parcel's cost base is `exercise_price × units +
    /// rights_cost`, and nothing multiplies the pair at write time — so an
    /// unrepresentable one was stored under a `201` and killed every report
    /// that costs the parcel. Reachable at any scale the entitlement allows:
    /// a nil-priced parcel of 1e15 units is a legitimate holding, and a 1-for-1
    /// issue entitles all of it. Refused now, naming the product and the limit
    /// (SCENARIOS W-e).
    #[tokio::test]
    async fn an_unrepresentable_exercised_cost_base_is_refused_naming_it() {
        let pool = test_pool().await;
        insert_listing(&pool, 1).await;
        // A nil-priced parcel: representable itself, so it is the exercise
        // that overflows and not the holding behind it.
        insert_buy(&pool, 1, d(2024, 1, 10), "1000000000000000", "0").await;
        // 1-for-1 at a fat-fingered $1e15 exercise price.
        corporate_action::db_upsert(
            &pool,
            &CorporateAction {
                id: 10,
                listing_id: 1,
                date: d(2024, 7, 1),
                kind: ActionKind::RightsIssue {
                    rights_units: Decimal::ONE,
                    rights_held_units: Decimal::ONE,
                    exercise_price: "1000000000000000".parse().unwrap(),
                    currency: "AUD".to_string(),
                    renounceable: true,
                },
            },
        )
        .await
        .unwrap();

        let response = ApiClient::over(router().with_state(pool.clone()))
            .post(
                "/corporate_actions/10/exercise",
                &serde_json::json!({"date": "2024-08-01", "units": "1000000000000000"}),
            )
            .await;
        let (status, detail) = response.status_and_body();
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{detail}");
        assert!(
            detail.contains(concat!(
                "exercise_price 1000000000000000 × units 1000000000000000",
                " + rights_cost 0"
            )),
            "the product is not named: {detail}"
        );
        assert!(
            detail.contains(&Decimal::MAX.to_string()),
            "the limit is not named: {detail}"
        );
        let exercised: bool =
            sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM trades WHERE rights_action_id = 10)")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert!(!exercised);

        // The control: a slice of the same entitlement whose cost base *is*
        // representable (1e13 × 1e15 = 1e28) still lands, so the bound is the
        // type's and nothing narrower.
        let response = ApiClient::over(router().with_state(pool.clone()))
            .post(
                "/corporate_actions/10/exercise",
                &serde_json::json!({"date": "2024-08-01", "units": "10000000000000"}),
            )
            .await;
        let (status, detail) = response.status_and_body();
        assert_eq!(status, StatusCode::CREATED, "{detail}");
    }

    /// "A replacement quantity no `Decimal` can hold" — and the one path in
    /// that section that is deliberately **not** a refusal.
    ///
    /// A 1000-for-1 issue over a holding of 1e27 units earns 1e30 rights,
    /// which no `Decimal` can hold, and the cap arithmetic panicked: the user
    /// asked to exercise **100** units and got a logged `500` with an empty
    /// body, for a figure they never named and that nothing would have stored.
    /// Unlike a scrip exchange's replacement quantity, this figure is never
    /// written: it exists only to answer *have more rights been used than were
    /// earned?*, and an unrepresentable cap answers that exactly — nothing
    /// representable reaches it. So the cap saturates to "unbounded" and the
    /// ordinary exercise lands.
    #[tokio::test]
    async fn a_modest_exercise_against_an_unrepresentable_entitlement_cap_still_lands() {
        let pool = test_pool().await;
        insert_listing(&pool, 1).await;
        // Nil-priced, so the holding itself is representable (W-e).
        insert_buy(
            &pool,
            1,
            d(2024, 1, 15),
            "1000000000000000000000000000",
            "0",
        )
        .await;
        // 1000-for-1 at $1: the entitlement is 1e30 rights.
        corporate_action::db_upsert(
            &pool,
            &CorporateAction {
                id: 10,
                listing_id: 1,
                date: d(2024, 7, 1),
                kind: ActionKind::RightsIssue {
                    rights_units: Decimal::from(1000),
                    rights_held_units: Decimal::ONE,
                    exercise_price: Decimal::ONE,
                    currency: "AUD".to_string(),
                    renounceable: true,
                },
            },
        )
        .await
        .unwrap();

        // The cap itself: past the range, so no representable request reaches
        // it.
        assert_eq!(
            entitled_units(
                "1000000000000000000000000000".parse().unwrap(),
                Decimal::from(1000),
                Decimal::ONE
            ),
            None
        );

        let response = ApiClient::over(router().with_state(pool.clone()))
            .post(
                "/corporate_actions/10/exercise",
                &serde_json::json!({"date": "2024-08-01", "units": "100"}),
            )
            .await;
        let (status, detail) = response.status_and_body();
        assert_eq!(status, StatusCode::CREATED, "{detail}");
        let v: serde_json::Value = serde_json::from_str(detail).unwrap();
        assert_eq!(v["quantity"], "100");

        // The cap still bites wherever it *is* representable: the same
        // holding under a 1-for-1e27 issue earns exactly one right, and a
        // second is refused.
        corporate_action::db_upsert(
            &pool,
            &CorporateAction {
                id: 11,
                listing_id: 1,
                date: d(2024, 7, 1),
                kind: ActionKind::RightsIssue {
                    rights_units: Decimal::ONE,
                    rights_held_units: "1000000000000000000000000000".parse().unwrap(),
                    exercise_price: Decimal::ONE,
                    currency: "AUD".to_string(),
                    renounceable: true,
                },
            },
        )
        .await
        .unwrap();
        let response = ApiClient::over(router().with_state(pool.clone()))
            .post(
                "/corporate_actions/11/exercise",
                &serde_json::json!({"date": "2024-08-01", "units": "2"}),
            )
            .await;
        assert_eq!(response.status, StatusCode::UNPROCESSABLE_ENTITY);
    }

    /// SCENARIOS W. The entitlement ratio applied to a very large holding:
    /// 1e27 units × 1000 is 1e30, past `Decimal`'s ~7.9228e28 ceiling, while
    /// the entitlement itself (1e27 × 1000 / 1e6 = 1e24 rights) is perfectly
    /// representable. Before `mul_div` the exercise panicked in
    /// `entitled_units` before it could get anywhere near its own cost-base
    /// bound.
    #[tokio::test]
    async fn api_exercise_past_the_old_entitlement_ceiling_completes() {
        let pool = test_pool().await;
        insert_listing(&pool, 1).await;
        // Nil-priced, so the holding itself is representable (W-e) and only
        // the entitlement arithmetic is at the ceiling.
        insert_buy(
            &pool,
            1,
            d(2024, 1, 15),
            "1000000000000000000000000000",
            "0",
        )
        .await;
        // 1000-for-1,000,000 at $1.
        corporate_action::db_upsert(
            &pool,
            &CorporateAction {
                id: 10,
                listing_id: 1,
                date: d(2024, 7, 1),
                kind: ActionKind::RightsIssue {
                    rights_units: Decimal::from(1000),
                    rights_held_units: Decimal::from(1_000_000),
                    exercise_price: Decimal::ONE,
                    currency: "AUD".to_string(),
                    renounceable: true,
                },
            },
        )
        .await
        .unwrap();

        let response = ApiClient::over(router().with_state(pool.clone()))
            .post(
                "/corporate_actions/10/exercise",
                &serde_json::json!({"date": "2024-08-01", "units": "250"}),
            )
            .await;
        let (status, detail) = response.status_and_body();
        assert_eq!(status, StatusCode::CREATED, "{detail}");
        // The entitlement it was checked against.
        assert_eq!(
            entitled_units(
                "1000000000000000000000000000".parse().unwrap(),
                Decimal::from(1000),
                Decimal::from(1_000_000)
            ),
            Some("1000000000000000000000000".parse::<Decimal>().unwrap())
        );
    }

    /// A near-nil exercise price and an unbounded entitlement, so the only
    /// thing left bounding the exercised unit count is the listing's own
    /// ratios — plus the 1000-for-1 split those ratios are.
    async fn nil_priced_rights_issue_behind_a_split(pool: &SqlitePool) {
        insert_listing(pool, 1).await;
        insert_buy(pool, 1, d(2024, 1, 15), "100", "1").await;
        corporate_action::db_upsert(
            pool,
            &CorporateAction {
                id: 10,
                listing_id: 1,
                date: d(2024, 2, 1),
                kind: ActionKind::RightsIssue {
                    rights_units: "1e27".parse().unwrap(),
                    rights_held_units: Decimal::ONE,
                    exercise_price: "0.0000000000000000000000000001".parse().unwrap(),
                    currency: "AUD".to_string(),
                    renounceable: true,
                },
            },
        )
        .await
        .unwrap();
        corporate_action::db_upsert(
            pool,
            &CorporateAction {
                id: 11,
                listing_id: 1,
                date: d(2024, 6, 1),
                kind: ActionKind::ShareSplit {
                    split_new_units: Decimal::from(1000),
                    split_old_units: Decimal::ONE,
                },
            },
        )
        .await
        .unwrap();
    }

    /// W-e's bound on this path is `exercise_price × units + rights_cost`,
    /// which a near-nil exercise price satisfies at any unit count at all — so
    /// nothing asked what the listing's recorded 1000-for-1 split does to 1e27
    /// exercised units. The exercise answered `201` and then killed every
    /// open-holdings read of the whole portfolio. Refused now, naming the
    /// quantity and the ratio, with nothing written.
    #[tokio::test]
    async fn api_an_exercised_quantity_a_recorded_ratio_rebases_out_of_range_is_refused() {
        let pool = test_pool().await;
        nil_priced_rights_issue_behind_a_split(&pool).await;

        let response = ApiClient::over(router().with_state(pool.clone()))
            .post(
                "/corporate_actions/10/exercise",
                &serde_json::json!({"date": "2024-02-15",
                                    "units": "1000000000000000000000000000"}),
            )
            .await;
        let (status, detail) = response.status_and_body();
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{detail}");
        assert!(
            detail.contains("quantity 1000000000000000000000000000 × new units 1000 / old units 1"),
            "the quantity and the ratio are not named: {detail}"
        );
        let exercised: bool =
            sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM trades WHERE rights_action_id = 10)")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert!(!exercised);
        // The read the state used to kill still answers.
        ApiClient::full(&pool)
            .get("/portfolio/open-parcels")
            .await
            .expect_status(StatusCode::OK);
    }

    /// The control, pinned at the figures this build answered before the
    /// refusal existed: 7.9e25 exercised units behind the same real 1000-for-1
    /// split re-base to 7.9e28, inside the range, so the exercise lands and the
    /// parcel reports.
    #[tokio::test]
    async fn api_a_large_exercised_quantity_a_recorded_ratio_still_fits_lands_and_reports() {
        let pool = test_pool().await;
        nil_priced_rights_issue_behind_a_split(&pool).await;

        let response = ApiClient::over(router().with_state(pool.clone()))
            .post(
                "/corporate_actions/10/exercise",
                &serde_json::json!({"date": "2024-02-15",
                                    "units": "79000000000000000000000000"}),
            )
            .await;
        let (status, detail) = response.status_and_body();
        assert_eq!(status, StatusCode::CREATED, "{detail}");

        let rows: Vec<serde_json::Value> = ApiClient::full(&pool)
            .get_json("/portfolio/open-parcels")
            .await;
        let exercised = rows
            .iter()
            .find(|r| r["acquisition_date"] == "2024-02-15")
            .unwrap_or_else(|| panic!("{rows:?}"));
        assert_eq!(exercised["original_quantity"], "79000000000000000000000000");
        assert_eq!(
            exercised["remaining_quantity"],
            "79000000000000000000000000000"
        );
        assert_eq!(
            exercised["original_cost_base"],
            "0.0079000000000000000000000000"
        );
    }
}
