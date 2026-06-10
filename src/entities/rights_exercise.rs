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
//! `trades.rights_action_id` — may not exceed it. To keep that check honest,
//! an exercise trade is immutable via `PUT /trades` (delete it and
//! re-exercise instead) and the action itself is frozen while exercise
//! trades reference it.
//!
//! Out of scope (documented in `docs/ato/rights-issues.md`): selling or lapsing
//! the rights themselves, pre-CGT originals, and retail premiums (entered as
//! unfranked dividend income).

use crate::entities::corporate_action::{
    self, ActionKind, as_acquired_quantity, split_adjusted_quantity,
};
use crate::entities::trade::{self, Trade, TradeType};
use crate::infra::decimal::parse_dec;
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
pub struct ExerciseBody {
    /// Exercise date — the new parcel's acquisition date. Must not precede
    /// the issue's record date.
    pub date: NaiveDate,
    /// New units acquired (strictly positive).
    pub units: Decimal,
    /// Total amount paid to acquire the exercised rights, in the action's
    /// currency (defaults to 0 — rights issued free have a nil cost base).
    #[serde(default)]
    pub rights_cost: Option<Decimal>,
    /// Optional foreign-per-AUD override for the created trade (defaults to
    /// 1; reports prefer the ATO rate and fall back to this — see `infra::fx`).
    #[serde(default)]
    pub fx_rate: Option<Decimal>,
    /// The holding account the exercised parcel lands in. Defaults to the
    /// seeded default account when omitted.
    #[serde(default = "crate::entities::holding_account::default_holding_account_id")]
    pub holding_account_id: i64,
}

#[derive(Debug)]
pub enum ExerciseError {
    Db(sqlx::Error),
    /// No corporate action with that id.
    ActionNotFound,
    /// The action is not a RightsIssue.
    NotARightsIssue,
    /// `units` is not strictly positive.
    NonPositiveUnits,
    /// `rights_cost` is negative.
    NegativeRightsCost,
    /// The exercise date precedes the issue's record date.
    BeforeRecordDate,
    /// Cumulative exercised units would exceed the entitlement earned by the
    /// units held at the record date.
    ExceedsEntitlement,
}

impl From<sqlx::Error> for ExerciseError {
    fn from(e: sqlx::Error) -> Self {
        ExerciseError::Db(e)
    }
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
            ExerciseError::BeforeRecordDate => {
                ApiError::unprocessable("the exercise date is before the issue's record date")
            }
            ExerciseError::ExceedsEntitlement => ApiError::unprocessable(
                "the units exercised exceed the entitlement earned by the holding at the record \
                 date",
            ),
            ExerciseError::Db(err) => err.into(),
        }
    }
}

pub fn router() -> Router<SqlitePool> {
    Router::new().route("/corporate_actions/{id}/exercise", post(exercise))
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

    let mut tx = pool.begin().await?;

    let action = match corporate_action::db_get_tx(&mut *tx, action_id).await? {
        Some(a) => a,
        None => return Err(ExerciseError::ActionNotFound),
    };
    let (rights_units, rights_held_units, exercise_price, currency) = match &action.kind {
        ActionKind::RightsIssue {
            rights_units,
            rights_held_units,
            exercise_price,
            currency,
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

    // Entitlement: units held when the record date arrived. Trades dated
    // before the record date count (one dated on it is ex-rights — the same
    // half-open convention as splits/bonus issues); each quantity is re-based
    // to record-date units across any splits/consolidations so buys, sells,
    // prior exercises, and the cap all compare in one basis.
    let splits = corporate_action::db_splits_for_listing(&mut *tx, action.listing_id).await?;
    let held_rows = sqlx::query(
        "SELECT trade_type, date, quantity FROM trades \
         WHERE listing_id = ? AND date < ?",
    )
    .bind(action.listing_id)
    .bind(record_date)
    .fetch_all(&mut *tx)
    .await?;
    let mut held = Decimal::ZERO;
    for row in &held_rows {
        let trade_date: NaiveDate = row.try_get("date")?;
        let qty = parse_dec("quantity", row.try_get("quantity")?)?;
        let in_record_units = split_adjusted_quantity(qty, &splits, trade_date, Some(record_date));
        match row.try_get::<TradeType, _>("trade_type")? {
            TradeType::Buy | TradeType::DRP => held += in_record_units,
            TradeType::Sell => held -= in_record_units,
        }
    }

    // Fractional entitlements round up to a whole unit (registry practice),
    // so the cap is never tighter than the offer's own rounding.
    let entitled = (held.max(Decimal::ZERO) * rights_units / rights_held_units).ceil();

    // Prior exercises against this action, re-based to record-date units.
    let prior_rows = sqlx::query("SELECT date, quantity FROM trades WHERE rights_action_id = ?")
        .bind(action_id)
        .fetch_all(&mut *tx)
        .await?;
    let mut exercised = Decimal::ZERO;
    for row in &prior_rows {
        let trade_date: NaiveDate = row.try_get("date")?;
        let qty = parse_dec("quantity", row.try_get("quantity")?)?;
        exercised += as_acquired_quantity(qty, &splits, record_date, trade_date);
    }
    exercised += as_acquired_quantity(body.units, &splits, record_date, body.date);
    if exercised > entitled {
        return Err(ExerciseError::ExceedsEntitlement);
    }

    let fx_rate = body.fx_rate.unwrap_or(Decimal::ONE);
    let result = sqlx::query(
        "INSERT INTO trades \
         (trade_type, date, settlement_date, listing_id, average_price, quantity, currency, \
          brokerage, gst_on_brokerage, brokerage_currency, fx_rate, contract_note_ref, \
          residual_brought_forward, residual_carried_forward, residual_paid_out, \
          rights_action_id, holding_account_id) \
         VALUES ('Buy', ?, ?, ?, ?, ?, ?, ?, '0', ?, ?, NULL, '0', '0', '0', ?, ?)",
    )
    .bind(body.date)
    .bind(body.date)
    .bind(action.listing_id)
    .bind(exercise_price.to_string())
    .bind(body.units.to_string())
    .bind(&currency)
    .bind(rights_cost.to_string())
    .bind(&currency)
    .bind(fx_rate.to_string())
    .bind(action_id)
    .bind(body.holding_account_id)
    .execute(&mut *tx)
    .await?;
    let new_id = result.last_insert_rowid();

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
    use crate::infra::db;
    use axum::{body::Body, http::Request};
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    async fn test_pool() -> SqlitePool {
        db::init(":memory:").await.unwrap()
    }

    fn d(y: i32, m: u32, day: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, day).unwrap()
    }

    async fn insert_listing(pool: &SqlitePool, id: i64) {
        listing::db_upsert(
            pool,
            &listing::Listing {
                id,
                exchange_mic: Some("XASX".to_string()),
                ticker: "RTS".to_string(),
                name: "Rights Test Co".to_string(),
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

    async fn insert_buy(pool: &SqlitePool, id: i64, date: NaiveDate, qty: &str, price: &str) {
        trade::db_upsert(
            pool,
            &Trade {
                brokerage_includes_gst: false,
                statement_total: None,
                holding_account_id: 1,
                transfer_id: None,
                ess_statement_id: None,
                id,
                trade_type: TradeType::Buy,
                date,
                settlement_date: date,
                listing_id: 1,
                average_price: price.parse().unwrap(),
                quantity: qty.parse().unwrap(),
                currency: "AUD".to_string(),
                brokerage: Decimal::ZERO,
                gst_on_brokerage: Decimal::ZERO,
                brokerage_currency: "AUD".to_string(),
                fx_rate: Decimal::ONE,
                contract_note_ref: None,
                residual_brought_forward: Decimal::ZERO,
                residual_carried_forward: Decimal::ZERO,
                residual_paid_out: Decimal::ZERO,
                rights_action_id: None,
                buyback_action_id: None,
                scrip_action_id: None,
                demerger_action_id: None,
                worthless_action_id: None,
                deemed_acquisition_date: None,
            },
        )
        .await
        .unwrap();
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

        let resp = router()
            .with_state(pool.clone())
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/corporate_actions/10/exercise")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        serde_json::json!({
                            "date": "2024-08-01",
                            "units": "250",
                            "rights_cost": "10.50",
                        })
                        .to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::CREATED);
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let trade: Trade = serde_json::from_slice(&bytes).unwrap();
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
        let resp = router()
            .with_state(pool.clone())
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!("/corporate_actions/{action_id}/exercise"))
                    .header("content-type", "application/json")
                    .body(Body::from(body.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), expected);
        // Every client-error rejection carries a reason for the toast.
        if expected.is_client_error() {
            let bytes = BodyExt::collect(resp.into_body()).await.unwrap().to_bytes();
            assert!(
                !bytes.is_empty(),
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
        let app = crate::entities::router().with_state(pool.clone());
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("PUT")
                    .uri(format!("/trades/{trade_id}"))
                    .header("content-type", "application/json")
                    .body(Body::from(
                        serde_json::json!({
                            "trade_type": "Buy", "date": "2024-08-01",
                            "settlement_date": "2024-08-01", "listing_id": 1,
                            "average_price": "1.80", "quantity": "9999",
                            "currency": "AUD", "brokerage": "0",
                            "gst_on_brokerage": "0", "brokerage_currency": "AUD",
                            "fx_rate": "1",
                        })
                        .to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY);
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("PUT")
                    .uri("/corporate_actions/10")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        serde_json::json!({
                            "action_type": "RightsIssue", "listing_id": 1,
                            "date": "2024-07-01", "rights_units": "1",
                            "rights_held_units": "4", "exercise_price": "1.80",
                            "currency": "AUD",
                        })
                        .to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY);
        let resp = app
            .oneshot(
                Request::builder()
                    .method("DELETE")
                    .uri("/corporate_actions/10")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY);
    }
}
