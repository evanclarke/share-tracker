//! Persistence: CRUD (`db_list`/`db_get`/`db_upsert`/`db_delete`), the
//! write-time invariant that an action referenced by exercise/participation/
//! exchange/demerger/recognise trades is frozen against edits ([`WriteError`]),
//! and the matching delete-time guard for the three types that re-base or
//! reduce parcels at read time instead of creating trades ([`DeleteError`]).

use super::adjustments::{
    as_acquired_quantity, db_payment_currency_conflict, db_splits_for_listing,
};
use super::model::{ActionKind, CorporateAction};
use crate::infra::db::write_tx;
use crate::infra::decimal::{OptMoney, parse_dec};
use crate::infra::http::{self, ApiError, CrudEntity};
use chrono::NaiveDate;
use rust_decimal::Decimal;
use sqlx::{Row, SqlitePool};
use std::collections::HashMap;

const COLUMNS: &str = "id, action_type, listing_id, date, amount_per_unit, currency, \
                       split_new_units, split_old_units, bonus_units, bonus_held_units, \
                       rights_units, rights_held_units, exercise_price, \
                       buyback_price, buyback_dividend, buyback_franking_credit, \
                       buyback_market_value, scrip_listing_id, scrip_new_units, \
                       scrip_old_units, scrip_cash_per_unit, scrip_market_value, \
                       scrip_cash_currency, demerger_listing_id, demerger_new_units, \
                       demerger_held_units, demerger_cost_base_pct, worthless_event, \
                       record_date, demerger_close_date, demerger_close_price, \
                       demerger_close_sourced_from, demerger_close_reason, renounceable";

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
    ///
    /// The **one** exception is a `Demerger`'s stated pre-demerger close
    /// ([`stated_close_only`]): it is a *price* fact, not a term — no demerge
    /// trade was created or validated against it, and it moves no quantity or
    /// cost base — so adding, correcting or removing it on an
    /// already-demerged action is allowed. Without that exception the fix for
    /// the provider's spin-off price adjustment would be unreachable on
    /// exactly the demergers that have been run, which is all of them that
    /// have prices to correct.
    #[error("this corporate action is referenced by trades or rights sales and cannot be edited")]
    ReferencedByTrade,
    /// The terms as written would leave a Sell allocating more units out of a
    /// parcel than the parcel holds — the state `PUT /sells/:id` itself
    /// refuses. A `ShareSplit`/`BonusIssue` re-bases quantities at read time,
    /// so its ratio and date decide how many as-acquired units each sale's
    /// allocation consumes: shrinking a ratio, moving the event past a sale,
    /// re-typing the action, moving it to another listing, or recording a new
    /// consolidation over existing sales can each over-consume a parcel.
    /// Checked over the resulting state rather than by freezing the row, so a
    /// correction that breaks nothing still lands. Mapped to `422`.
    #[error("this action's terms leave a sale allocating more units than its parcel holds")]
    AllocationsExceedParcel,
    /// The terms as written re-base a quantity of this listing past what a
    /// `Decimal` can hold — a 1000-for-1 split over a holding of 1e27 units
    /// asks for 1e30 units. Unlike the operations, a `ShareSplit`/`BonusIssue`
    /// materialises nothing: the re-base is computed at *read* time, so such a
    /// row was accepted `204` and then killed every open-holdings report of the
    /// portfolio (a logged `500` with an empty body) until someone worked out
    /// which action did it — with several of the reports that would have found
    /// it among the ones that were down. Refused here instead, over the state
    /// the write leaves behind, so the same edit that brings the ratio back
    /// inside the range still lands. Mapped to `422`.
    #[error("this action re-bases a quantity beyond the representable range: {0}")]
    UnrepresentableRebasedQuantity(#[source] crate::domain::cost_base::UnrepresentableQuantity),
    /// A `ReturnOfCapital` recorded in one currency while a parcel it reduces
    /// is held in another. The payment reduces each parcel's cost base in the
    /// parcel's own currency and amounts are never netted across currencies,
    /// so the reports refuse to compute the pair
    /// (`RocEvent::per_unit_for` → `sqlx::Error::Decode` → `500`) — every
    /// cost-base report of the listing dies at read time on a state nothing
    /// checked at write time (SCENARIOS E-07, E-39). Refused here instead, so
    /// the state is unrepresentable. Mapped to `422`.
    #[error("this payment's currency differs from that of the parcels it reduces")]
    PaymentCurrencyMismatch {
        payment_currency: String,
        parcel_currency: String,
    },
    /// A `ReturnOfCapital` / `ShareSplit` / `BonusIssue` dated on or before a
    /// rollover of the same listing that has already run — a transfer, a
    /// scrip-for-scrip exchange, or a demerger. Those operations **store** each
    /// replacement parcel's carried cost base and quantity, computed from the
    /// facts as they stood when they ran, so an event back-dated over one
    /// restates the parcels it consumed (which the reports still walk) while the
    /// frozen replacement figures cannot move: the cost base is silently
    /// overstated, or — for a split — the source parcel reappears as an open
    /// holding beside the untouched replacement (SCENARIOS N-06, N-07). The
    /// recovery is the one `amit_adjustment`'s `UnitsCarriedIntoReplacement`
    /// already names: delete the operation, enter the event, run it again.
    /// Mapped to `422`.
    #[error("this event is dated on or before a rollover of the same listing that has already run")]
    BackDatedOverRollover {
        /// The rollover closing Sells the event would restate behind, newest
        /// first, as (date, what it was): named so the operation to redo is
        /// findable without a search.
        rollovers: Vec<(NaiveDate, String)>,
    },
    /// A `ReturnOfCapital` on a listing flagged `amit`. An AMIT's cost-base
    /// movement is its AMMA statement's per-unit `cost_base_adjustment`,
    /// entered as AMIT adjustments (CGT event E10) — the E4 tax-deferred
    /// mechanism a `ReturnOfCapital` models is for non-AMIT trusts. Nothing
    /// relates the two, so the same money entered both ways simply reduces
    /// the parcel twice and no cross-check sees it (SCENARIOS E-04). The
    /// income path already refuses the mirror image of this
    /// (`income::UpsertError::AmitTaxDeferred`); this closes the other door.
    /// Mapped to `422`.
    #[error("a return of capital does not apply to an AMIT")]
    ReturnOfCapitalOnAmit,
}

impl From<WriteError> for ApiError {
    fn from(e: WriteError) -> Self {
        match e {
            // Frozen while exercise/participation trades reference it → 422.
            WriteError::ReferencedByTrade => ApiError::unprocessable(
                "this corporate action is referenced by rights-exercise, buy-back, \
                 scrip-for-scrip, demerger, or worthless-shares trades or by rights sales \
                 and cannot be edited — delete those rows first (a demerger's stated \
                 pre-demerger close is the one exception: it is a price fact and stays \
                 editable, so change only those four fields)",
            ),
            // The written terms would leave an over-consumed parcel → 422.
            WriteError::AllocationsExceedParcel => ApiError::unprocessable(
                "these terms re-base parcel quantities so that a sale allocates more units than \
                 the parcel it draws on holds — correct or remove those allocations first",
            ),
            // The terms re-base a quantity past what a decimal can hold → 422
            // quoting the arithmetic, the same wording every
            // beyond-the-range refusal answers with.
            WriteError::UnrepresentableRebasedQuantity(e) => ApiError::Unprocessable(e.message()),
            // The payment and the parcels it reduces disagree on currency → 422
            // naming both, so the typo is findable without opening the trades.
            WriteError::PaymentCurrencyMismatch {
                payment_currency,
                parcel_currency,
            } => ApiError::Unprocessable(format!(
                "this return of capital is recorded in {payment_currency} while a parcel it \
                 reduces is held in {parcel_currency} — a payment reduces each parcel's cost \
                 base in the parcel's own currency, and amounts are never netted across \
                 currencies, so record it converted into {parcel_currency}"
            )),
            // Back-dated over a rollover whose carried figures are frozen →
            // 422 naming each operation and the delete-enter-redo recovery,
            // the same wording `amit_adjustment` uses for the same situation.
            WriteError::BackDatedOverRollover { rollovers } => {
                let named = rollovers
                    .iter()
                    .map(|(date, what)| format!("{what} on {date}"))
                    .collect::<Vec<_>>()
                    .join(", ");
                ApiError::Unprocessable(format!(
                    "this event is dated on or before a rollover of the same listing that has \
                     already run ({named}), which carried its parcels' cost base and quantity away \
                     as stored figures — recording it now would restate the parcels the operation \
                     consumed while leaving the replacement parcels untouched. Delete that \
                     operation, enter this event, then run it again, so the replacement parcels \
                     carry the restated figures forward"
                ))
            }
            // The AMIT's cost-base movement belongs on its AMMA statement →
            // 422 naming the field it belongs in, mirroring the income
            // path's `tax_deferred_amount` refusal.
            WriteError::ReturnOfCapitalOnAmit => ApiError::unprocessable(
                "a return of capital does not apply to an AMIT — its cost-base movement is the \
                 AMMA statement's cost_base_adjustment, entered as AMIT adjustments (CGT event \
                 E10), not an E4 return of capital",
            ),
            // Unknown listing/currency FK or enum CHECK violation → 422.
            WriteError::Db(err) => err.into(),
        }
    }
}

/// Whether every parcel of `listing_id` still covers the sale allocations
/// drawn on it, read on the caller's own connection so a write can check the
/// state it is about to commit.
///
/// This is the listing-wide form of the per-parcel invariant the Sell and
/// trade write paths each uphold from their own side
/// (`sell::SellError::PurchaseQuantityExceeded`,
/// `trade::UpsertError::QuantityBelowAllocated`): a parcel's quantity is in
/// as-acquired units while each allocation is in its own sale date's units, so
/// allocations are re-based back across the listing's splits (TD 2000/10)
/// before comparing — the same [`as_acquired_quantity`] those paths use. A
/// corporate-action write is the third way the comparison can move: it changes
/// the split stream itself rather than either side of the sum.
async fn allocations_fit_parcels(
    conn: &mut sqlx::SqliteConnection,
    listing_id: i64,
) -> Result<bool, sqlx::Error> {
    let splits = db_splits_for_listing(&mut *conn, listing_id).await?;
    let rows = sqlx::query(
        "SELECT pa.purchase_trade_id, p.date AS acquired, p.quantity AS parcel_quantity, \
                s.date AS sale_date, pa.quantity_allocated \
         FROM parcel_allocations pa \
         JOIN trades p ON p.id = pa.purchase_trade_id \
         JOIN trades s ON s.id = pa.sale_trade_id \
         WHERE p.listing_id = ?",
    )
    .bind(listing_id)
    .fetch_all(&mut *conn)
    .await?;

    // Each parcel's (quantity, allocations consumed so far), in as-acquired
    // units.
    let mut consumed: HashMap<i64, (Decimal, Decimal)> = HashMap::new();
    for row in &rows {
        let parcel_id: i64 = row.try_get("purchase_trade_id")?;
        let acquired: NaiveDate = row.try_get("acquired")?;
        let parcel_quantity = parse_dec("parcel_quantity", row.try_get("parcel_quantity")?)?;
        let sale_date: NaiveDate = row.try_get("sale_date")?;
        let allocated = parse_dec("quantity_allocated", row.try_get("quantity_allocated")?)?;
        let entry = consumed
            .entry(parcel_id)
            .or_insert((parcel_quantity, Decimal::ZERO));
        entry.1 += as_acquired_quantity(allocated, &splits, acquired, sale_date);
    }
    Ok(consumed
        .values()
        .all(|&(quantity, total)| total <= quantity))
}

/// The first quantity of `listing_id` that the recorded split stream re-bases
/// past `Decimal`'s range, if there is one — read on the caller's own
/// connection so a write can check the state it is about to commit.
///
/// The sibling of [`allocations_fit_parcels`] and checked at the same hook, for
/// the same reason: a `ShareSplit`/`BonusIssue` materialises nothing, so its
/// ratio and date are re-applied at *read* time to every quantity of the
/// listing, and terms that overflow there are accepted here and then kill the
/// reports. Both directions of the re-base are covered, because both are used:
///
/// * forward — a parcel's as-acquired quantity into a later unit basis
///   ([`split_adjusted_quantity`], the open-holdings reports, the rollovers'
///   parcel walk, and the rights issue's holding at its record date). Checked
///   at **every** split boundary after the parcel rather than only at the
///   cumulative end, since a split followed by a consolidation nets back to a
///   ratio that fits while the basis in between does not, and a report as at
///   that date would still read it;
/// * backward — a sale's allocated quantity into its parcel's as-acquired
///   basis ([`as_acquired_quantity`]), which a *consolidation* multiplies up.
///   That is the direction [`allocations_fit_parcels`] itself computes in, so
///   this must be checked first or that check overflows before it can answer.
///
/// The parcel's gross quantity is what is bounded, not the units still open:
/// the gross figure is what the rights issue's record-date holding and the
/// activity report's running balance re-base.
///
/// It is the **parcel-creating writes'** guard as well, and for the mirror of
/// its own reason. A ratio that fits every parcel of the listing when it is
/// written can be made unrepresentable afterwards by a parcel entered *behind*
/// it, which the action write cannot see coming, so each of those writes runs
/// this walk over the state it is about to commit (`domain::whole_holding`
/// enumerates the paths; a rollover runs it on the listing its **replacement**
/// parcels land on, which is not the listing the operation is about). The two
/// hooks together cover the cross product: the action write judges a new ratio
/// against every recorded quantity, and the parcel write judges a new quantity
/// against every recorded ratio.
///
/// Run **after** the write's own INSERT, over the resulting state, for the same
/// reason the action write does: a row already stored beyond the range — one a
/// build predating this rule wrote — is corrected by the very write that would
/// be refused if the walk ran first.
pub async fn rebased_quantity_beyond_range(
    conn: &mut sqlx::SqliteConnection,
    listing_id: i64,
) -> Result<Option<crate::domain::cost_base::UnrepresentableQuantity>, sqlx::Error> {
    let splits = db_splits_for_listing(&mut *conn, listing_id).await?;
    // Nothing re-bases quantities on this listing, so nothing can overflow.
    if splits.is_empty() {
        return Ok(None);
    }

    let parcels = sqlx::query(
        "SELECT date, quantity FROM trades          WHERE listing_id = ? AND trade_type IN ('Buy', 'DRP')",
    )
    .bind(listing_id)
    .fetch_all(&mut *conn)
    .await?;
    for row in &parcels {
        let acquired: NaiveDate = row.try_get("date")?;
        let quantity = parse_dec("quantity", row.try_get("quantity")?)?;
        let (mut new, mut old) = (Decimal::ONE, Decimal::ONE);
        for split in &splits {
            if split.date <= acquired {
                continue;
            }
            new *= split.new_units;
            old *= split.old_units;
            if let Err(e) = crate::domain::cost_base::checked_rebased_quantity(
                ("quantity", quantity),
                ("new units", new),
                ("old units", old),
            ) {
                return Ok(Some(e));
            }
        }
    }

    let allocations = sqlx::query(
        "SELECT pa.quantity_allocated, p.date AS acquired, s.date AS sale_date          FROM parcel_allocations pa          JOIN trades p ON p.id = pa.purchase_trade_id          JOIN trades s ON s.id = pa.sale_trade_id          WHERE p.listing_id = ?",
    )
    .bind(listing_id)
    .fetch_all(&mut *conn)
    .await?;
    for row in &allocations {
        let acquired: NaiveDate = row.try_get("acquired")?;
        let sale_date: NaiveDate = row.try_get("sale_date")?;
        let allocated = parse_dec("quantity_allocated", row.try_get("quantity_allocated")?)?;
        let (new, old) = super::adjustments::split_ratio(&splits, acquired, Some(sale_date));
        if let Err(e) = crate::domain::cost_base::checked_rebased_quantity(
            ("quantity_allocated", allocated),
            ("old units", old),
            ("new units", new),
        ) {
            return Ok(Some(e));
        }
    }
    Ok(None)
}

/// The rollover closing Sells of `listing_id` dated on or after `date` — the
/// operations an event dated then would restate behind — newest first, each as
/// (date, a human name for it). `exclude_action_id` drops the group the action
/// being written created itself.
///
/// Read on the caller's transaction, so the check and the write see one state.
async fn rollovers_after(
    conn: &mut sqlx::SqliteConnection,
    listing_id: i64,
    date: NaiveDate,
    exclude_action_id: i64,
) -> Result<Vec<(NaiveDate, String)>, sqlx::Error> {
    let rows = sqlx::query(
        "SELECT s.date AS date, s.transfer_id, s.scrip_action_id, s.demerger_action_id \
         FROM trades s \
         WHERE s.listing_id = ?1 AND s.trade_type = 'Sell' AND s.date >= ?2 \
           AND (s.transfer_id IS NOT NULL OR s.scrip_action_id IS NOT NULL \
                OR s.demerger_action_id IS NOT NULL) \
           AND COALESCE(s.scrip_action_id, s.demerger_action_id, -1) <> ?3 \
         ORDER BY s.date DESC, s.id DESC",
    )
    .bind(listing_id)
    .bind(date)
    .bind(exclude_action_id)
    .fetch_all(&mut *conn)
    .await?;
    rows.iter()
        .map(|row| {
            let date: NaiveDate = row.try_get("date")?;
            let what = if let Some(id) = row.try_get::<Option<i64>, _>("transfer_id")? {
                format!("holding-account transfer #{id}")
            } else if let Some(id) = row.try_get::<Option<i64>, _>("scrip_action_id")? {
                format!("scrip-for-scrip exchange of corporate action #{id}")
            } else {
                let id: Option<i64> = row.try_get("demerger_action_id")?;
                format!("demerger of corporate action #{}", id.unwrap_or_default())
            };
            Ok((date, what))
        })
        .collect()
}

/// Whether `written` differs from `stored` **only** in a `Demerger`'s stated
/// pre-demerger close — the one part of an action that a referenced (already
/// demerged) row may still be edited on.
///
/// The stated close is a price fact: `entities::closing_price` derives the
/// demerger's price re-basing factor from it, and nothing else reads it. The
/// demerge operation's trades were created and validated against the
/// entitlement ratio and the cost-base percentage, neither of which this can
/// touch — the comparison below is over the whole row with just the close
/// blanked out, so any other difference (a ratio, the percentage, the date,
/// the listing, the action type) still meets the freeze.
///
/// An unchanged close is not "only the close": a write that moves nothing is
/// refused as before, so this widens the freeze by exactly one fact and no
/// more.
fn stated_close_only(stored: &CorporateAction, written: &CorporateAction) -> bool {
    let blank_close = |kind: &ActionKind| {
        let mut kind = kind.clone();
        if let ActionKind::Demerger {
            demerger_close_date,
            demerger_close_price,
            demerger_close_sourced_from,
            demerger_close_reason,
            ..
        } = &mut kind
        {
            *demerger_close_date = None;
            *demerger_close_price = None;
            *demerger_close_sourced_from = None;
            *demerger_close_reason = None;
        }
        kind
    };
    matches!(
        (&stored.kind, &written.kind),
        (ActionKind::Demerger { .. }, ActionKind::Demerger { .. })
    ) && stored.listing_id == written.listing_id
        && stored.date == written.date
        && stored.kind != written.kind
        && blank_close(&stored.kind) == blank_close(&written.kind)
}

pub async fn db_upsert(pool: &SqlitePool, action: &CorporateAction) -> Result<(), WriteError> {
    // Spread the variant's payload over the per-type columns; the other
    // types' columns are NULL (the table CHECKs require exactly this shape).
    #[derive(Default)]
    struct Cols {
        amount_per_unit: OptMoney,
        currency: Option<String>,
        record_date: Option<NaiveDate>,
        split_new_units: OptMoney,
        split_old_units: OptMoney,
        bonus_units: OptMoney,
        bonus_held_units: OptMoney,
        rights_units: OptMoney,
        rights_held_units: OptMoney,
        exercise_price: OptMoney,
        renounceable: Option<bool>,
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
        demerger_close_date: Option<NaiveDate>,
        demerger_close_price: OptMoney,
        demerger_close_sourced_from: Option<String>,
        demerger_close_reason: Option<String>,
        worthless_event: Option<&'static str>,
    }
    let mut c = Cols::default();
    match &action.kind {
        ActionKind::ReturnOfCapital {
            amount_per_unit,
            currency,
            record_date,
        } => {
            c.amount_per_unit = OptMoney(Some(*amount_per_unit));
            c.currency = Some(currency.clone());
            c.record_date = *record_date;
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
            renounceable,
        } => {
            c.rights_units = OptMoney(Some(*rights_units));
            c.rights_held_units = OptMoney(Some(*rights_held_units));
            c.exercise_price = OptMoney(Some(*exercise_price));
            c.currency = Some(currency.clone());
            c.renounceable = Some(*renounceable);
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
            demerger_close_date,
            demerger_close_price,
            demerger_close_sourced_from,
            demerger_close_reason,
        } => {
            c.demerger_listing_id = Some(*demerger_listing_id);
            c.demerger_new_units = OptMoney(Some(*demerger_new_units));
            c.demerger_held_units = OptMoney(Some(*demerger_held_units));
            c.demerger_cost_base_pct = OptMoney(Some(*demerger_cost_base_pct));
            c.demerger_close_date = *demerger_close_date;
            c.demerger_close_price = OptMoney(*demerger_close_price);
            c.demerger_close_sourced_from = demerger_close_sourced_from.clone();
            c.demerger_close_reason = demerger_close_reason.clone();
        }
        ActionKind::WorthlessShares { worthless_event } => {
            c.worthless_event = Some(worthless_event.as_str());
        }
    }

    let mut tx = write_tx(pool).await?;

    // An action that exercise, participation, exchange, or demerge trades
    // were validated against is frozen: editing its terms (or re-typing it)
    // would invalidate the checks those trades were created under. Checked
    // and written in one transaction.
    // Loaded once: the row as it stands decides both whether a referenced
    // action is nonetheless writable (below) and which listing an edit is
    // moving it off (further down).
    let stored = db_get_tx(&mut *tx, action.id).await?;

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
    // Frozen against its *terms* — but a Demerger's stated pre-demerger close
    // is not one of them (see `WriteError::ReferencedByTrade`), so a write that
    // changes nothing else is let through. `stated_close_only` requires the
    // close to actually differ, so an otherwise identical re-`PUT` of a
    // referenced action is refused exactly as it always was.
    if referenced
        && !stored
            .as_ref()
            .is_some_and(|stored| stated_close_only(stored, action))
    {
        return Err(WriteError::ReferencedByTrade);
    }

    // An AMIT reduces its unit holders' cost base through its AMMA
    // statement's per-unit `cost_base_adjustment` (CGT event E10); the E4
    // mechanism a return of capital models is for non-AMIT trusts. Nothing
    // relates the two, so the same money entered both ways reduces the parcel
    // twice with no cross-check to catch it (SCENARIOS E-04) — refused here,
    // the mirror of the income path's `tax_deferred_amount` refusal. The
    // listing's flag is all it takes, so this is checked before the INSERT;
    // an unknown listing falls through to the FK violation as before.
    if matches!(action.kind, ActionKind::ReturnOfCapital { .. }) {
        let amit: Option<(bool, Option<chrono::NaiveDate>)> =
            sqlx::query_as("SELECT amit, amit_from FROM listings WHERE id = ?")
                .bind(action.listing_id)
                .fetch_optional(&mut *tx)
                .await?;
        // Dated, not absolute: a fund that converted to an AMIT part-way
        // through a holding paid its pre-conversion tax-deferred amounts as an
        // ordinary trust, and those years' E4 reductions must stay
        // recordable — and editable — after the flag goes on
        // (SCENARIOS F-23). `listing::amit_in_tax_year` is the shared rule.
        let amit_year = amit.is_some_and(|(amit, amit_from)| {
            crate::entities::listing::amit_in_tax_year(
                amit,
                amit_from,
                crate::domain::tax_year::tax_year_for(action.date),
            )
        });
        if amit_year {
            return Err(WriteError::ReturnOfCapitalOnAmit);
        }
    }

    // A rollover of this listing that has already run froze the cost base and
    // quantity its replacement parcels carry (`domain::rollover` stores both),
    // so an event dated on or before it can no longer reach them while it does
    // still restate the parcels the operation consumed. Refused for the three
    // action types that retroactively restate a parcel — a return of capital
    // reduces the cost base of parcels held at its date, and a split or bonus
    // issue re-bases their quantities — with the rollovers named and the
    // delete-enter-redo recovery spelled out (SCENARIOS N-06, N-07). The
    // remaining types either create their own trades (rights, buy-back) or
    // *are* the operation, so they have nothing to restate behind.
    //
    // The action's own group is excluded: a `ScripForScrip`/`Demerger` row is
    // frozen by `ReferencedByTrade` above once executed anyway, and this must
    // not refuse the very action that created the rollover.
    if matches!(
        action.kind,
        ActionKind::ReturnOfCapital { .. }
            | ActionKind::ShareSplit { .. }
            | ActionKind::BonusIssue { .. }
    ) {
        let rollovers = rollovers_after(&mut tx, action.listing_id, action.date, action.id).await?;
        if !rollovers.is_empty() {
            return Err(WriteError::BackDatedOverRollover { rollovers });
        }
    }

    // Which listing the row is leaving, when an edit moves it (the split
    // stream it is removed from has to hold up too). `None` for an insert.
    let previous_listing_id: Option<i64> = stored.as_ref().map(|stored| stored.listing_id);

    sqlx::query(
        "INSERT INTO corporate_actions \
         (id, action_type, listing_id, date, amount_per_unit, currency, \
          split_new_units, split_old_units, bonus_units, bonus_held_units, \
          rights_units, rights_held_units, exercise_price, \
          buyback_price, buyback_dividend, buyback_franking_credit, buyback_market_value, \
          scrip_listing_id, scrip_new_units, scrip_old_units, \
          scrip_cash_per_unit, scrip_market_value, scrip_cash_currency, \
          demerger_listing_id, demerger_new_units, demerger_held_units, \
          demerger_cost_base_pct, worthless_event, record_date, \
          demerger_close_date, demerger_close_price, demerger_close_sourced_from, \
          demerger_close_reason, renounceable) \
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, \
                 ?, ?, ?, ?, ?, ?) \
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
             worthless_event        = excluded.worthless_event, \
             record_date            = excluded.record_date, \
             demerger_close_date         = excluded.demerger_close_date, \
             demerger_close_price        = excluded.demerger_close_price, \
             demerger_close_sourced_from = excluded.demerger_close_sourced_from, \
             demerger_close_reason       = excluded.demerger_close_reason, \
             renounceable                = excluded.renounceable",
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
    .bind(c.record_date)
    .bind(c.demerger_close_date)
    .bind(c.demerger_close_price)
    .bind(c.demerger_close_sourced_from)
    .bind(c.demerger_close_reason)
    .bind(c.renounceable)
    .execute(&mut *tx)
    .await?;

    // Editing an action is deliberately *not* frozen the way deleting one is
    // (docs/API.md Known limitations: a mis-keyed ratio, date, or amount has
    // to stay correctable in place) — but the state the edit leaves behind
    // still has to be a legal one. The written row is checked, not the fields
    // that changed, so this equally covers a fresh consolidation recorded over
    // sales that already allocate in the pre-consolidation basis. Inside the
    // write's own transaction, so a refused write is never persisted.
    let mut listings = vec![action.listing_id];
    if let Some(previous) = previous_listing_id.filter(|p| *p != action.listing_id) {
        listings.push(previous);
    }
    for &listing_id in &listings {
        // Representability first: `allocations_fit_parcels` re-bases each
        // allocation itself, so an unrepresentable one overflows *inside* that
        // check before it can answer (`rebased_quantity_beyond_range`).
        if let Some(beyond) = rebased_quantity_beyond_range(&mut tx, listing_id).await? {
            return Err(WriteError::UnrepresentableRebasedQuantity(beyond));
        }
        if !allocations_fit_parcels(&mut tx, listing_id).await? {
            return Err(WriteError::AllocationsExceedParcel);
        }
    }

    // A stored closing price is in its own trading day's unit basis, derived
    // from the figure the provider served by the price re-basing events dated
    // between that day and the fetch (`entities::closing_price`). Recording a
    // ShareSplit/BonusIssue, or a Demerger's stated pre-demerger close — or
    // editing a ratio, a close, or a date, re-typing the action into something
    // else, or moving it to another listing — changes that derivation, so the
    // affected listings' prices are re-derived here, inside the write's own
    // transaction: the action and the prices it restates are committed
    // together, and the order the two were entered in cannot matter (SCENARIOS
    // Q-14, and the demerger finding that followed it). Run for every kind on
    // the written listings rather than only the re-basing ones, so a re-type
    // *away* from a split, or a stated close being removed, is covered by the
    // same call; the pass reads nothing at all for a listing with no price
    // re-basing event.
    for &listing_id in &listings {
        crate::entities::closing_price::db_rebase_listing_prices(&mut tx, listing_id).await?;
    }

    // A return of capital reduces each parcel's cost base in the *parcel's*
    // currency, so a payment recorded in another one is a state the cost-base
    // reports refuse to compute over — checked over the written state, on the
    // listing the payment now belongs to, inside the write's own transaction.
    // (Only a payment can introduce the pair from this side: re-typing a
    // ReturnOfCapital into another kind, or moving it off a listing, can only
    // remove one.)
    if matches!(action.kind, ActionKind::ReturnOfCapital { .. })
        && let Some(conflict) = db_payment_currency_conflict(&mut *tx, action.listing_id).await?
    {
        return Err(WriteError::PaymentCurrencyMismatch {
            payment_currency: conflict.payment_currency,
            parcel_currency: conflict.parcel_currency,
        });
    }

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
    /// A `ReturnOfCapital` whose listing has an acquisition the payment
    /// reached — dated before its record date, or on/before the payment date
    /// when it carries none. The reduction (and any
    /// CGT event G1 excess gain it produced, reported in the payment's income
    /// year) is computed from the action at read time, so deleting it restates
    /// a cost base and can drop an already-reported gain. Mapped to `422`.
    #[error("this return of capital reduced the cost base of parcels held at its date")]
    ReducedParcels,
    /// The action is frozen by its own trade group: the delete failed the
    /// `trades.*_action_id` (or `rights_sales.rights_action_id`) foreign key.
    /// The payload is the ready-made body naming the dependants, built by
    /// [`http::fk_dependants_message`] — the same wording every other blocked
    /// delete answers with. Mapped to `422`.
    #[error("this corporate action is still referenced ({0})")]
    StillReferenced(String),
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
            DeleteError::StillReferenced(body) => ApiError::Unprocessable(body),
            DeleteError::Db(err) => err.into(),
        }
    }
}

/// Delete an action, `Ok(false)` when no action has that id.
///
/// An action referenced by rights-exercise, buy-back participation,
/// scrip-for-scrip exchange, demerger, or recognise trades is protected by the
/// corresponding `trades.*_action_id` foreign key — the violation is turned
/// into a `422` naming those dependants ([`DeleteError::StillReferenced`]).
/// The three types that create no trades — `ShareSplit`, `BonusIssue`,
/// `ReturnOfCapital` — have no such reference to protect them, so their
/// dependants are checked here, in the delete's own transaction (see
/// [`DeleteError`]).
pub async fn db_delete(pool: &SqlitePool, id: i64) -> Result<bool, DeleteError> {
    let mut tx = write_tx(pool).await?;
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
            action.date,
            DeleteError::RebasedTrades,
        )),
        // Which acquisitions the payment actually reached: those held before
        // its record date, or — with none recorded — those made on or before
        // the payment date, the same entitlement test the cost-base pipeline
        // applies (`RocEvent::per_unit_for`). A parcel bought ex-entitlement
        // was never reduced, so deleting the action restates nothing about it.
        ActionKind::ReturnOfCapital {
            record_date: Some(record_date),
            ..
        } => Some((
            "SELECT EXISTS(SELECT 1 FROM trades \
                           WHERE listing_id = ? AND date < ? AND trade_type IN ('Buy', 'DRP'))",
            *record_date,
            DeleteError::ReducedParcels,
        )),
        ActionKind::ReturnOfCapital {
            record_date: None, ..
        } => Some((
            "SELECT EXISTS(SELECT 1 FROM trades \
                           WHERE listing_id = ? AND date <= ? AND trade_type IN ('Buy', 'DRP'))",
            action.date,
            DeleteError::ReducedParcels,
        )),
        ActionKind::RightsIssue { .. }
        | ActionKind::BuyBack { .. }
        | ActionKind::ScripForScrip { .. }
        | ActionKind::Demerger { .. }
        | ActionKind::WorthlessShares { .. } => None,
    };
    if let Some((query, cutoff, err)) = dependants {
        let dependent: bool = sqlx::query_scalar(query)
            .bind(action.listing_id)
            .bind(cutoff)
            .fetch_one(&mut *tx)
            .await?;
        if dependent {
            return Err(err);
        }
    }

    if let Err(err) = sqlx::query("DELETE FROM corporate_actions WHERE id = ?")
        .bind(id)
        .execute(&mut *tx)
        .await
    {
        // Roll back before the scan: it counts the blocking rows on the pool,
        // which the open transaction would otherwise be holding a write lock
        // against.
        drop(tx);
        let noun = <CorporateAction as CrudEntity>::NOUN;
        return match http::fk_dependants_message(pool, &err, noun, "corporate_actions", "id", id)
            .await?
        {
            Some(body) => Err(DeleteError::StillReferenced(body)),
            None => Err(DeleteError::Db(err)),
        };
    }
    // Deleting a price re-basing action — a split, a bonus issue, or a
    // demerger carrying a stated close — restates the listing's stored closing
    // prices exactly as recording one does, so the same pass runs in the
    // delete's own transaction; see `db_upsert` above.
    crate::entities::closing_price::db_rebase_listing_prices(&mut tx, action.listing_id).await?;

    tx.commit().await?;
    Ok(true)
}
