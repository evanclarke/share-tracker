//! Persistence: CRUD (`db_list`/`db_get`/`db_upsert`/`db_delete`) and the
//! write-time invariant that an action referenced by exercise/participation/
//! exchange/demerger/recognise trades is frozen against edits ([`WriteError`]).

use super::model::{ActionKind, CorporateAction};
use crate::infra::decimal::OptMoney;
use crate::infra::http::ApiError;
use sqlx::SqlitePool;

const COLUMNS: &str = "id, action_type, listing_id, date, amount_per_unit, currency, \
                       split_new_units, split_old_units, bonus_units, bonus_held_units, \
                       rights_units, rights_held_units, exercise_price, \
                       buyback_price, buyback_dividend, buyback_franking_credit, \
                       buyback_market_value, scrip_listing_id, scrip_new_units, \
                       scrip_old_units, scrip_cash_per_unit, scrip_market_value, \
                       scrip_cash_currency, demerger_listing_id, demerger_new_units, \
                       demerger_held_units, demerger_cost_base_pct, worthless_event";

pub async fn db_list(pool: &SqlitePool) -> Result<Vec<CorporateAction>, sqlx::Error> {
    sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT {COLUMNS} FROM corporate_actions ORDER BY id"
    )))
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
    sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT {COLUMNS} FROM corporate_actions WHERE id = ?"
    )))
    .bind(id)
    .fetch_optional(executor)
    .await
}

#[derive(Debug)]
pub enum WriteError {
    Db(sqlx::Error),
    /// The action is referenced by rights-exercise, buy-back participation,
    /// scrip-for-scrip exchange, demerger, or worthless-shares recognise trades
    /// (`trades.rights_action_id` / `trades.buyback_action_id` /
    /// `trades.scrip_action_id` / `trades.demerger_action_id` /
    /// `trades.worthless_action_id`), or by rights sales
    /// (`rights_sales.rights_action_id`): editing it would retroactively change
    /// the terms those rows were created and validated against. Delete the
    /// referencing rows first. Mapped to `422`.
    ReferencedByTrade,
}

impl From<sqlx::Error> for WriteError {
    fn from(e: sqlx::Error) -> Self {
        WriteError::Db(e)
    }
}

impl From<WriteError> for ApiError {
    fn from(e: WriteError) -> Self {
        match e {
            // Frozen while exercise/participation trades reference it → 422.
            WriteError::ReferencedByTrade => ApiError::unprocessable(
                "this corporate action is referenced by rights-exercise, buy-back, \
                 scrip-for-scrip, demerger, or worthless-shares trades or by rights sales \
                 and cannot be edited — delete those rows first",
            ),
            // Unknown listing/currency FK or enum CHECK violation → 422.
            WriteError::Db(err) => err.into(),
        }
    }
}

pub async fn db_upsert(pool: &SqlitePool, action: &CorporateAction) -> Result<(), WriteError> {
    // Spread the variant's payload over the per-type columns; the other
    // types' columns are NULL (the table CHECKs require exactly this shape).
    #[derive(Default)]
    struct Cols {
        amount_per_unit: OptMoney,
        currency: Option<String>,
        split_new_units: OptMoney,
        split_old_units: OptMoney,
        bonus_units: OptMoney,
        bonus_held_units: OptMoney,
        rights_units: OptMoney,
        rights_held_units: OptMoney,
        exercise_price: OptMoney,
        buyback_price: OptMoney,
        buyback_dividend: OptMoney,
        buyback_franking_credit: OptMoney,
        buyback_market_value: OptMoney,
        scrip_listing_id: Option<i64>,
        scrip_new_units: OptMoney,
        scrip_old_units: OptMoney,
        scrip_cash_per_unit: OptMoney,
        scrip_market_value: OptMoney,
        scrip_cash_currency: Option<String>,
        demerger_listing_id: Option<i64>,
        demerger_new_units: OptMoney,
        demerger_held_units: OptMoney,
        demerger_cost_base_pct: OptMoney,
        worthless_event: Option<&'static str>,
    }
    let mut c = Cols::default();
    match &action.kind {
        ActionKind::ReturnOfCapital {
            amount_per_unit,
            currency,
        } => {
            c.amount_per_unit = OptMoney(Some(*amount_per_unit));
            c.currency = Some(currency.clone());
        }
        ActionKind::ShareSplit {
            split_new_units,
            split_old_units,
        } => {
            c.split_new_units = OptMoney(Some(*split_new_units));
            c.split_old_units = OptMoney(Some(*split_old_units));
        }
        ActionKind::BonusIssue {
            bonus_units,
            bonus_held_units,
        } => {
            c.bonus_units = OptMoney(Some(*bonus_units));
            c.bonus_held_units = OptMoney(Some(*bonus_held_units));
        }
        ActionKind::RightsIssue {
            rights_units,
            rights_held_units,
            exercise_price,
            currency,
        } => {
            c.rights_units = OptMoney(Some(*rights_units));
            c.rights_held_units = OptMoney(Some(*rights_held_units));
            c.exercise_price = OptMoney(Some(*exercise_price));
            c.currency = Some(currency.clone());
        }
        ActionKind::BuyBack {
            buyback_price,
            buyback_dividend,
            buyback_franking_credit,
            buyback_market_value,
            currency,
        } => {
            c.buyback_price = OptMoney(Some(*buyback_price));
            c.buyback_dividend = OptMoney(Some(*buyback_dividend));
            c.buyback_franking_credit = OptMoney(Some(*buyback_franking_credit));
            c.buyback_market_value = OptMoney(*buyback_market_value);
            c.currency = Some(currency.clone());
        }
        ActionKind::ScripForScrip {
            scrip_listing_id,
            scrip_new_units,
            scrip_old_units,
            scrip_cash_per_unit,
            scrip_market_value,
            scrip_cash_currency,
        } => {
            c.scrip_listing_id = Some(*scrip_listing_id);
            c.scrip_new_units = OptMoney(Some(*scrip_new_units));
            c.scrip_old_units = OptMoney(Some(*scrip_old_units));
            c.scrip_cash_per_unit = OptMoney(*scrip_cash_per_unit);
            c.scrip_market_value = OptMoney(*scrip_market_value);
            c.scrip_cash_currency = scrip_cash_currency.clone();
        }
        ActionKind::Demerger {
            demerger_listing_id,
            demerger_new_units,
            demerger_held_units,
            demerger_cost_base_pct,
        } => {
            c.demerger_listing_id = Some(*demerger_listing_id);
            c.demerger_new_units = OptMoney(Some(*demerger_new_units));
            c.demerger_held_units = OptMoney(Some(*demerger_held_units));
            c.demerger_cost_base_pct = OptMoney(Some(*demerger_cost_base_pct));
        }
        ActionKind::WorthlessShares { worthless_event } => {
            c.worthless_event = Some(worthless_event.as_str());
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
                          OR scrip_action_id = ?1 OR demerger_action_id = ?1 \
                          OR worthless_action_id = ?1) \
             OR EXISTS(SELECT 1 FROM rights_sales WHERE rights_action_id = ?1)",
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
          scrip_cash_per_unit, scrip_market_value, scrip_cash_currency, \
          demerger_listing_id, demerger_new_units, demerger_held_units, \
          demerger_cost_base_pct, worthless_event) \
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?) \
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
             scrip_cash_per_unit = excluded.scrip_cash_per_unit, \
             scrip_market_value  = excluded.scrip_market_value, \
             scrip_cash_currency = excluded.scrip_cash_currency, \
             demerger_listing_id    = excluded.demerger_listing_id, \
             demerger_new_units     = excluded.demerger_new_units, \
             demerger_held_units    = excluded.demerger_held_units, \
             demerger_cost_base_pct = excluded.demerger_cost_base_pct, \
             worthless_event        = excluded.worthless_event",
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
    .bind(c.scrip_cash_per_unit)
    .bind(c.scrip_market_value)
    .bind(c.scrip_cash_currency)
    .bind(c.demerger_listing_id)
    .bind(c.demerger_new_units)
    .bind(c.demerger_held_units)
    .bind(c.demerger_cost_base_pct)
    .bind(c.worthless_event)
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
