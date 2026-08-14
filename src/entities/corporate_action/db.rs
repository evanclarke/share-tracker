//! Persistence: CRUD (`db_list`/`db_get`/`db_upsert`/`db_delete`), the
//! write-time invariant that an action referenced by exercise/participation/
//! exchange/demerger/recognise trades is frozen against edits ([`WriteError`]),
//! and the matching delete-time guard for the three types that re-base or
//! reduce parcels at read time instead of creating trades ([`DeleteError`]).

use super::model::{ActionKind, CorporateAction};
use crate::infra::decimal::OptMoney;
use crate::infra::http::{ApiError, CrudEntity};
use sqlx::SqlitePool;

const COLUMNS: &str = "id, action_type, listing_id, date, amount_per_unit, currency, \
                       split_new_units, split_old_units, bonus_units, bonus_held_units, \
                       rights_units, rights_held_units, exercise_price, \
                       buyback_price, buyback_dividend, buyback_franking_credit, \
                       buyback_market_value, scrip_listing_id, scrip_new_units, \
                       scrip_old_units, scrip_cash_per_unit, scrip_market_value, \
                       scrip_cash_currency, demerger_listing_id, demerger_new_units, \
                       demerger_held_units, demerger_cost_base_pct, worthless_event";

impl CrudEntity for CorporateAction {
    type Key = i64;
    const TABLE: &'static str = "corporate_actions";
    const COLUMNS: &'static str = COLUMNS;
    const NOUN: &'static str = "corporate action";
}

#[cfg(test)]
pub async fn db_list(pool: &SqlitePool) -> Result<Vec<CorporateAction>, sqlx::Error> {
    crate::infra::http::crud_list(pool).await
}

#[cfg(test)]
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

#[derive(thiserror::Error, Debug)]
pub enum WriteError {
    #[error("corporate action write failed: {0}")]
    Db(#[from] sqlx::Error),
    /// The action is referenced by rights-exercise, buy-back participation,
    /// scrip-for-scrip exchange, demerger, or worthless-shares recognise trades
    /// (`trades.rights_action_id` / `trades.buyback_action_id` /
    /// `trades.scrip_action_id` / `trades.demerger_action_id` /
    /// `trades.worthless_action_id`), or by rights sales
    /// (`rights_sales.rights_action_id`): editing it would retroactively change
    /// the terms those rows were created and validated against. Delete the
    /// referencing rows first. Mapped to `422`.
    #[error("this corporate action is referenced by trades or rights sales and cannot be edited")]
    ReferencedByTrade,
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

#[derive(thiserror::Error, Debug)]
pub enum DeleteError {
    #[error("corporate action delete failed: {0}")]
    Db(#[from] sqlx::Error),
    /// A `ShareSplit` or `BonusIssue` whose listing has a trade dated on or
    /// after the conversion/issue date. Those quantities are recorded in the
    /// post-action unit basis, and nothing materialises the re-base: every
    /// open-parcel quantity, allocation capacity check, and realised gain is
    /// computed from the action at read time. Deleting it re-reads them in the
    /// pre-action basis, which can leave a Sell's allocations exceeding the
    /// parcel they draw on — the state `PUT /sells/:id` itself refuses — and a
    /// generated AMIT adjustment covering more units than its parcel has.
    /// Mapped to `422`.
    #[error("this action re-bases parcels that later trades are recorded against")]
    RebasedTrades,
    /// A `ReturnOfCapital` whose listing has an acquisition dated on or before
    /// the payment date — a parcel the payment reduced. The reduction (and any
    /// CGT event G1 excess gain it produced, reported in the payment's income
    /// year) is computed from the action at read time, so deleting it restates
    /// a cost base and can drop an already-reported gain. Mapped to `422`.
    #[error("this return of capital reduced the cost base of parcels held at its date")]
    ReducedParcels,
}

impl From<DeleteError> for ApiError {
    fn from(e: DeleteError) -> Self {
        match e {
            DeleteError::RebasedTrades => ApiError::unprocessable(
                "this listing has a trade dated on or after this action — those quantities are \
                 recorded in the post-split unit basis, so deleting it would restate them \
                 (delete those trades first)",
            ),
            DeleteError::ReducedParcels => ApiError::unprocessable(
                "this payment reduced the cost base of parcels held at its date — deleting it \
                 would restate those parcels, and any capital gain already reported for the \
                 excess (delete the parcels first)",
            ),
            DeleteError::Db(err) => err.into(),
        }
    }
}

/// Delete an action, `Ok(false)` when no action has that id.
///
/// An action referenced by rights-exercise, buy-back participation,
/// scrip-for-scrip exchange, demerger, or recognise trades is protected by the
/// corresponding `trades.*_action_id` foreign key — the violation surfaces as a
/// database error the handler maps to `422`. The three types that create no
/// trades — `ShareSplit`, `BonusIssue`, `ReturnOfCapital` — have no such
/// reference to protect them, so their dependants are checked here, in the
/// delete's own transaction (see [`DeleteError`]).
pub async fn db_delete(pool: &SqlitePool, id: i64) -> Result<bool, DeleteError> {
    let mut tx = pool.begin().await?;
    let Some(action) = db_get_tx(&mut *tx, id).await? else {
        return Ok(false);
    };

    // The dependants each type leaves behind, in the direction its effect
    // runs: a split/bonus re-bases everything recorded *after* it, a return of
    // capital reduces the parcels held (so acquired) *before* it. The other
    // five types are frozen by their trade group's foreign key instead.
    let dependants = match &action.kind {
        ActionKind::ShareSplit { .. } | ActionKind::BonusIssue { .. } => Some((
            "SELECT EXISTS(SELECT 1 FROM trades WHERE listing_id = ? AND date >= ?)",
            DeleteError::RebasedTrades,
        )),
        ActionKind::ReturnOfCapital { .. } => Some((
            "SELECT EXISTS(SELECT 1 FROM trades \
                           WHERE listing_id = ? AND date <= ? AND trade_type IN ('Buy', 'DRP'))",
            DeleteError::ReducedParcels,
        )),
        ActionKind::RightsIssue { .. }
        | ActionKind::BuyBack { .. }
        | ActionKind::ScripForScrip { .. }
        | ActionKind::Demerger { .. }
        | ActionKind::WorthlessShares { .. } => None,
    };
    if let Some((query, err)) = dependants {
        let dependent: bool = sqlx::query_scalar(query)
            .bind(action.listing_id)
            .bind(action.date)
            .fetch_one(&mut *tx)
            .await?;
        if dependent {
            return Err(err);
        }
    }

    sqlx::query("DELETE FROM corporate_actions WHERE id = ?")
        .bind(id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(true)
}
