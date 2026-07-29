//! Persistence for trades: the `db_*` functions and the write-time
//! invariants they enforce in-transaction (dependant-capacity re-checks,
//! provenance immutability guards), plus the delete guards.

use super::checks::{
    AmountsCheck, AmountsError, SpotFxRateError, StatementTotalCheck, StatementTotalError,
    amounts_detail, check_amounts, check_statement_total, spot_fx_rate_detail,
    statement_total_detail, validate_spot_fx_rate,
};
use super::model::Trade;
use crate::infra::decimal::{Money, OptMoney, parse_dec};
use crate::infra::http::ApiError;
use rust_decimal::Decimal;
use sqlx::{Row, SqlitePool};

pub async fn db_list(pool: &SqlitePool) -> Result<Vec<Trade>, sqlx::Error> {
    sqlx::query_as(
        "SELECT id, trade_type, date, settlement_date, listing_id, average_price, quantity, \
         currency, brokerage, gst_on_brokerage, brokerage_includes_gst, brokerage_currency, \
         fx_rate, spot_fx_rate, contract_note_ref, statement_total, \
         residual_brought_forward, residual_carried_forward, residual_paid_out, rights_action_id, \
         buyback_action_id, scrip_action_id, demerger_action_id, deemed_acquisition_date, \
         holding_account_id, transfer_id, ess_statement_id, worthless_action_id, inheritance_id \
         FROM trades ORDER BY date, id",
    )
    .fetch_all(pool)
    .await
}

pub async fn db_get(pool: &SqlitePool, id: i64) -> Result<Option<Trade>, sqlx::Error> {
    sqlx::query_as(
        "SELECT id, trade_type, date, settlement_date, listing_id, average_price, quantity, \
         currency, brokerage, gst_on_brokerage, brokerage_includes_gst, brokerage_currency, \
         fx_rate, spot_fx_rate, contract_note_ref, statement_total, \
         residual_brought_forward, residual_carried_forward, residual_paid_out, rights_action_id, \
         buyback_action_id, scrip_action_id, demerger_action_id, deemed_acquisition_date, \
         holding_account_id, transfer_id, ess_statement_id, worthless_action_id, inheritance_id \
         FROM trades WHERE id = ?",
    )
    .bind(id)
    .fetch_optional(pool)
    .await
}

#[derive(Debug)]
pub enum UpsertError {
    Db(sqlx::Error),
    /// The new quantity falls below the total already allocated out of this
    /// parcel by Sell allocations — accepting it would leave those allocations
    /// drawing on units the parcel no longer has.
    QuantityBelowAllocated,
    /// The new quantity falls below a linked AMIT adjustment's covered
    /// quantity, breaking that adjustment's `quantity <= trade.quantity`
    /// invariant (see `amit_adjustment::db_upsert`).
    QuantityBelowAmitAdjustment,
    /// The edit changes the trade's `listing_id` while Sell allocations or
    /// AMIT adjustments draw on this parcel: accepting it would silently
    /// re-associate those dependants to the new listing, costing them
    /// cross-listing in every CGT report. Remove the dependants first (e.g.
    /// delete the Sell via `DELETE /sells/:id`).
    ListingChangeReferenced,
    /// The existing trade is a rights exercise (`rights_action_id` set): its
    /// figures were validated against the rights issue's entitlement, which a
    /// free-form edit could exceed. Delete it and re-exercise instead (see
    /// `entities::rights_exercise`).
    RightsExerciseTrade,
    /// The existing trade is a buy-back participation Sell
    /// (`buyback_action_id` set): its figures derive from the buy-back's
    /// terms and it carries a linked dividend-component income row. Delete it
    /// via `DELETE /sells` and re-participate instead (see
    /// `entities::buyback_participation`).
    BuyBackTrade,
    /// The existing trade belongs to a scrip-for-scrip exchange group
    /// (`scrip_action_id` set): its figures carry the rollover's cost base
    /// and deemed acquisition date, which a free-form edit would corrupt.
    /// Delete the group via `DELETE /sells` on the closing Sell and
    /// re-exchange instead (see `entities::scrip_exchange`).
    ScripExchangeTrade,
    /// The existing trade belongs to a demerger group (`demerger_action_id`
    /// set): its figures carry the rollover's apportioned cost base and
    /// deemed acquisition date, which a free-form edit would corrupt. Delete
    /// the group via `DELETE /sells` on the closing Sell and re-demerge
    /// instead (see `entities::demerger`).
    DemergerTrade,
    /// The existing trade belongs to a holding-account transfer group
    /// (`transfer_id` set): its figures carry the moved parcel's cost base
    /// and deemed acquisition date, which a free-form edit would corrupt.
    /// Delete the transfer via `DELETE /transfers/:id` and re-transfer
    /// instead (see `entities::transfer`).
    TransferTrade,
    /// The existing trade is a cost-base-reset ESS vest Buy
    /// (`ess_statement_id` set): its figures derive from the ESS statement's
    /// quantity and taxing-point market value. Delete the statement (which
    /// removes the vest) and re-vest instead (see `entities::ess_vest`).
    EssVestTrade,
    /// The existing trade is an inherited-parcel Buy (`inheritance_id` set):
    /// its figures carry the inheritance's cost base and s 115-30 discount
    /// clock, which a free-form edit would corrupt. Edit the inheritance
    /// (`PUT /inheritances/:id`) instead (see `entities::inheritance`).
    InheritedParcelTrade,
    /// The existing trade is a DRP reinvestment — a distribution links to it
    /// via `income.reinvestment_trade_id`. Its quantity, price, and residual
    /// columns were computed from that distribution's cash and the enrolment
    /// period's residual chain, and a free-form body (which carries zero
    /// residuals) would silently re-type it and wipe the chain while the
    /// income row keeps pointing at it. Undo the reinvestment via its
    /// distribution (`DELETE /income/:id/reinvest`) and re-reinvest instead.
    /// The provenance lives on the income row, not on the trade, so it is
    /// guarded by a lookup rather than a column (symmetric with `db_delete`'s
    /// reinvestment guard).
    ReinvestmentTrade,
    /// The existing trade is an original parcel anchoring a rights sale
    /// (`rights_sale_allocations.purchase_trade_id`): its date and quantity
    /// are what the sale's record-date anchoring caps were validated against,
    /// which a free-form edit could silently break. Delete the rights sale
    /// (`DELETE /rights_sales/:id`) and re-enter it after the edit (see
    /// `entities::rights_sale`).
    RightsAnchorParcel,
    /// A supplied statement total failed the cross-check (see
    /// `check_statement_total`): it doesn't reconcile with the trade's own
    /// figures, or the trade and brokerage currencies differ so there is no
    /// single-currency total to check.
    StatementTotal(StatementTotalError),
    /// A supplied spot-rate override was rejected (see
    /// `validate_spot_fx_rate`): non-positive, or on an AUD trade where it
    /// could never apply.
    SpotFxRate(SpotFxRateError),
    /// A degenerate core figure was rejected (see [`check_amounts`]):
    /// non-positive quantity or FX rate, negative price/brokerage/GST, or a
    /// settlement before the trade date.
    Amounts(AmountsError),
}

impl From<sqlx::Error> for UpsertError {
    fn from(e: sqlx::Error) -> Self {
        UpsertError::Db(e)
    }
}

/// Create or update a trade. Validated and written in one transaction
/// (symmetric with the Sell-side invariants in `sell::db_upsert_sell`): an
/// edit may not shrink a Buy/DRP's quantity below what its dependants rely on
/// — the quantity already allocated to Sells, or any linked AMIT adjustment's
/// covered quantity.
pub async fn db_upsert(pool: &SqlitePool, trade: &Trade) -> Result<(), UpsertError> {
    // Degenerate figures (zero/negative quantity, negative costs, …) corrupt
    // every downstream report without failing anything — rejected before
    // anything else runs.
    check_amounts(&AmountsCheck {
        quantity: trade.quantity,
        average_price: trade.average_price,
        brokerage: trade.brokerage,
        gst_on_brokerage: trade.gst_on_brokerage,
        fx_rate: trade.fx_rate,
        date: trade.date,
        settlement_date: trade.settlement_date,
    })
    .map_err(UpsertError::Amounts)?;
    // The statement total (when recorded) must reconcile with the trade's own
    // figures — a mismatch is a data-entry error against the contract note,
    // caught before anything is written.
    check_statement_total(StatementTotalCheck {
        statement_total: trade.statement_total,
        amounts: trade.amounts(),
        currency: &trade.currency,
        brokerage_currency: &trade.brokerage_currency,
    })
    .map_err(UpsertError::StatementTotal)?;
    // A deliberate spot-rate override must be usable: positive, and on a
    // trade whose amounts actually convert.
    validate_spot_fx_rate(&trade.currency, trade.spot_fx_rate).map_err(UpsertError::SpotFxRate)?;

    let mut tx = pool.begin().await?;

    // A rights-exercise, buy-back participation, scrip-for-scrip exchange,
    // or demerger trade is immutable here: it was created against its
    // action's terms (entitlement cap / dividend-capital split / carried
    // cost base and deemed acquisition date), which an edit could silently
    // break. (The INSERT below never sets any provenance column, so a normal
    // trade can't become one either.)
    type ProvenanceRow = (
        i64,
        Option<i64>,
        Option<i64>,
        Option<i64>,
        Option<i64>,
        Option<i64>,
        Option<i64>,
        Option<i64>,
    );
    let existing_action: Option<ProvenanceRow> = sqlx::query_as(
        "SELECT listing_id, rights_action_id, buyback_action_id, scrip_action_id, \
                demerger_action_id, transfer_id, ess_statement_id, inheritance_id \
         FROM trades WHERE id = ?",
    )
    .bind(trade.id)
    .fetch_optional(&mut *tx)
    .await?;
    let existing_listing_id = existing_action.as_ref().map(|row| row.0);
    if let Some((_, rights, buyback, scrip, demerger, transfer, ess, inheritance)) = existing_action
    {
        if rights.is_some() {
            return Err(UpsertError::RightsExerciseTrade);
        }
        if buyback.is_some() {
            return Err(UpsertError::BuyBackTrade);
        }
        if scrip.is_some() {
            return Err(UpsertError::ScripExchangeTrade);
        }
        if demerger.is_some() {
            return Err(UpsertError::DemergerTrade);
        }
        if transfer.is_some() {
            return Err(UpsertError::TransferTrade);
        }
        if ess.is_some() {
            return Err(UpsertError::EssVestTrade);
        }
        if inheritance.is_some() {
            return Err(UpsertError::InheritedParcelTrade);
        }
    }
    // A parcel anchoring a rights sale is immutable too: the sale's
    // record-date anchoring caps were validated against this trade's date and
    // quantity. Its provenance lives on the allocation rows, so it is guarded
    // by a lookup rather than a column.
    let anchors_rights: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM rights_sale_allocations WHERE purchase_trade_id = ?)",
    )
    .bind(trade.id)
    .fetch_one(&mut *tx)
    .await?;
    if anchors_rights {
        return Err(UpsertError::RightsAnchorParcel);
    }
    // A transfer's network-fee disposal Sell is immutable here too: its
    // provenance lives on the transfer (transfers.fee_sale_trade_id), not on
    // the trade row, so it is guarded by a lookup rather than a column.
    let is_transfer_fee: bool =
        sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM transfers WHERE fee_sale_trade_id = ?)")
            .bind(trade.id)
            .fetch_one(&mut *tx)
            .await?;
    if is_transfer_fee {
        return Err(UpsertError::TransferTrade);
    }
    // A reinvest-created DRP is immutable here for the same reason: its link
    // lives on the income row (income.reinvestment_trade_id), which the
    // provenance-column check above can't see, so a Buy body would silently
    // re-type it and zero its residual chain. Guarded by lookup, like the
    // delete path.
    let is_reinvestment: bool =
        sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM income WHERE reinvestment_trade_id = ?)")
            .bind(trade.id)
            .fetch_one(&mut *tx)
            .await?;
    if is_reinvestment {
        return Err(UpsertError::ReinvestmentTrade);
    }

    // The listing is frozen while dependants draw on the parcel: a Sell
    // allocation or AMIT adjustment references this trade by id, so changing
    // its listing would silently re-associate them to the new security. (This
    // also keeps the capacity re-check below honest — its split re-basing
    // looks up the trade's listing.)
    if existing_listing_id.is_some_and(|existing| existing != trade.listing_id) {
        let referenced: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM parcel_allocations WHERE purchase_trade_id = ?1) \
                 OR EXISTS(SELECT 1 FROM amit_adjustments WHERE trade_id = ?1)",
        )
        .bind(trade.id)
        .fetch_one(&mut *tx)
        .await?;
        if referenced {
            return Err(UpsertError::ListingChangeReferenced);
        }
    }

    // Sum is computed in Decimal (the column is TEXT; SQL SUM would coerce to
    // float). For a new id both dependant sets are empty, so the checks pass.
    // Each allocation is in its own sale date's unit basis while the trade's
    // quantity is as-acquired, so allocations are re-based back across any
    // share splits/consolidations (TD 2000/10) before comparing.
    let splits =
        crate::entities::corporate_action::db_splits_for_listing(&mut *tx, trade.listing_id)
            .await?;
    let allocated = sqlx::query(
        "SELECT pa.quantity_allocated, s.date AS sale_date \
         FROM parcel_allocations pa JOIN trades s ON s.id = pa.sale_trade_id \
         WHERE pa.purchase_trade_id = ?",
    )
    .bind(trade.id)
    .fetch_all(&mut *tx)
    .await?;
    let mut allocated_total = Decimal::ZERO;
    for row in &allocated {
        let qty = parse_dec("quantity_allocated", row.try_get("quantity_allocated")?)?;
        let sale_date: chrono::NaiveDate = row.try_get("sale_date")?;
        allocated_total += crate::entities::corporate_action::as_acquired_quantity(
            qty, &splits, trade.date, sale_date,
        );
    }
    if trade.quantity < allocated_total {
        return Err(UpsertError::QuantityBelowAllocated);
    }

    let amit_quantities: Vec<String> =
        sqlx::query_scalar("SELECT quantity FROM amit_adjustments WHERE trade_id = ?")
            .bind(trade.id)
            .fetch_all(&mut *tx)
            .await?;
    for q in amit_quantities {
        if trade.quantity < parse_dec("quantity", q)? {
            return Err(UpsertError::QuantityBelowAmitAdjustment);
        }
    }

    sqlx::query(
        "INSERT INTO trades \
         (id, trade_type, date, settlement_date, listing_id, average_price, quantity, \
          currency, brokerage, gst_on_brokerage, brokerage_includes_gst, brokerage_currency, \
          fx_rate, spot_fx_rate, contract_note_ref, statement_total, \
          residual_brought_forward, residual_carried_forward, residual_paid_out, \
          holding_account_id) \
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?) \
         ON CONFLICT(id) DO UPDATE SET \
             trade_type               = excluded.trade_type, \
             date                     = excluded.date, \
             settlement_date          = excluded.settlement_date, \
             listing_id               = excluded.listing_id, \
             average_price            = excluded.average_price, \
             quantity                 = excluded.quantity, \
             currency                 = excluded.currency, \
             brokerage                = excluded.brokerage, \
             gst_on_brokerage         = excluded.gst_on_brokerage, \
             brokerage_includes_gst   = excluded.brokerage_includes_gst, \
             brokerage_currency       = excluded.brokerage_currency, \
             fx_rate                  = excluded.fx_rate, \
             spot_fx_rate             = excluded.spot_fx_rate, \
             contract_note_ref        = excluded.contract_note_ref, \
             statement_total          = excluded.statement_total, \
             residual_brought_forward = excluded.residual_brought_forward, \
             residual_carried_forward = excluded.residual_carried_forward, \
             residual_paid_out        = excluded.residual_paid_out, \
             holding_account_id       = excluded.holding_account_id",
    )
    .bind(trade.id)
    .bind(trade.trade_type)
    .bind(trade.date)
    .bind(trade.settlement_date)
    .bind(trade.listing_id)
    .bind(Money(trade.average_price))
    .bind(Money(trade.quantity))
    .bind(&trade.currency)
    .bind(Money(trade.brokerage))
    .bind(Money(trade.gst_on_brokerage))
    .bind(trade.brokerage_includes_gst)
    .bind(&trade.brokerage_currency)
    .bind(Money(trade.fx_rate))
    .bind(OptMoney(trade.spot_fx_rate))
    .bind(&trade.contract_note_ref)
    .bind(OptMoney(trade.statement_total))
    .bind(Money(trade.residual_brought_forward))
    .bind(Money(trade.residual_carried_forward))
    .bind(Money(trade.residual_paid_out))
    .bind(trade.holding_account_id)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(())
}

/// Outcome of a delete request, so the handler can map to the right status.
#[derive(Debug, PartialEq)]
pub enum DeleteOutcome {
    Deleted,
    NotFound,
    /// The trade is still referenced — as the purchase parcel of a Sell's
    /// allocation (or a Sell with allocations), as a parcel anchoring a
    /// rights sale, by an AMIT adjustment, or as a
    /// distribution's reinvestment trade — or it belongs to a scrip-for-scrip
    /// exchange or demerger group, which is only ever deleted as a whole (via
    /// `DELETE /sells` on the group's closing Sell), or it is an ESS vest Buy
    /// (removed via `DELETE /ess_statements/:id`). Deleting it would orphan
    /// those dependants or break the rollover's parcel substitution, so the
    /// request is refused (mapped to 422) rather than surfacing the SQLite FK
    /// error as a 500. Remove the dependants first (e.g. delete the Sell via
    /// `DELETE /sells/:id`).
    Referenced,
}

/// The provenance links a trade may carry that block deleting it on its own.
/// Read by [`db_delete`] and mapped by column name via `FromRow`.
#[derive(sqlx::FromRow)]
struct DeleteGuard {
    scrip_action_id: Option<i64>,
    demerger_action_id: Option<i64>,
    transfer_id: Option<i64>,
    ess_statement_id: Option<i64>,
    worthless_action_id: Option<i64>,
    inheritance_id: Option<i64>,
}

pub async fn db_delete(pool: &SqlitePool, id: i64) -> Result<DeleteOutcome, sqlx::Error> {
    let mut tx = pool.begin().await?;

    let guard: Option<DeleteGuard> = sqlx::query_as(
        "SELECT scrip_action_id, demerger_action_id, transfer_id, ess_statement_id, \
                worthless_action_id, inheritance_id \
         FROM trades WHERE id = ?",
    )
    .bind(id)
    .fetch_optional(&mut *tx)
    .await?;
    let Some(guard) = guard else {
        return Ok(DeleteOutcome::NotFound);
    };
    // A scrip-for-scrip exchange, demerger, or holding-account transfer trade
    // is never deleted individually — the group's closing Sell and
    // replacement Buys substitute the same parcels, so they are removed as a
    // whole via DELETE /sells on the closing Sell (or DELETE /transfers/:id).
    // An ESS vest Buy is likewise only ever removed via DELETE
    // /ess_statements/:id (which deletes the statement and its vest together). A
    // worthless-shares recognise closing Sell is removed via DELETE /sells
    // (which restores the holding). An inherited-parcel Buy is removed via
    // DELETE /inheritances/:id (which deletes the inheritance and its Buy
    // together).
    if guard.scrip_action_id.is_some()
        || guard.demerger_action_id.is_some()
        || guard.transfer_id.is_some()
        || guard.ess_statement_id.is_some()
        || guard.worthless_action_id.is_some()
        || guard.inheritance_id.is_some()
    {
        return Ok(DeleteOutcome::Referenced);
    }

    let referenced: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM parcel_allocations \
                       WHERE purchase_trade_id = ?1 OR sale_trade_id = ?1) \
             OR EXISTS(SELECT 1 FROM rights_sale_allocations WHERE purchase_trade_id = ?1) \
             OR EXISTS(SELECT 1 FROM amit_adjustments WHERE trade_id = ?1) \
             OR EXISTS(SELECT 1 FROM income \
                       WHERE reinvestment_trade_id = ?1 OR buyback_trade_id = ?1)",
    )
    .bind(id)
    .fetch_one(&mut *tx)
    .await?;
    if referenced {
        return Ok(DeleteOutcome::Referenced);
    }

    sqlx::query("DELETE FROM trades WHERE id = ?")
        .bind(id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(DeleteOutcome::Deleted)
}

impl From<UpsertError> for ApiError {
    fn from(e: UpsertError) -> Self {
        match e {
            // The cross-check rejection says what the trade adds up to, so a
            // typo is findable without re-deriving the figure by hand.
            UpsertError::StatementTotal(detail) => {
                ApiError::unprocessable(statement_total_detail(&detail))
            }
            UpsertError::SpotFxRate(detail) => {
                ApiError::unprocessable(spot_fx_rate_detail(&detail))
            }
            UpsertError::Amounts(detail) => ApiError::unprocessable(amounts_detail(&detail)),
            UpsertError::QuantityBelowAllocated => ApiError::unprocessable(
                "the new quantity is below what Sell allocations already draw from this parcel",
            ),
            UpsertError::QuantityBelowAmitAdjustment => ApiError::unprocessable(
                "the new quantity is below a linked AMIT adjustment's covered quantity",
            ),
            UpsertError::ListingChangeReferenced => ApiError::unprocessable(
                "the listing cannot be changed while Sell allocations or AMIT adjustments \
                 reference this parcel — remove them first",
            ),
            UpsertError::RightsExerciseTrade => ApiError::unprocessable(
                "this trade is a rights exercise and cannot be edited — delete it and \
                 re-exercise instead",
            ),
            UpsertError::BuyBackTrade => ApiError::unprocessable(
                "this trade is a buy-back participation and cannot be edited — delete it and \
                 re-participate instead",
            ),
            UpsertError::ScripExchangeTrade => ApiError::unprocessable(
                "this trade belongs to a scrip-for-scrip exchange and cannot be edited — \
                 delete the group and re-exchange instead",
            ),
            UpsertError::DemergerTrade => ApiError::unprocessable(
                "this trade belongs to a demerger and cannot be edited — delete the group and \
                 re-demerge instead",
            ),
            UpsertError::TransferTrade => ApiError::unprocessable(
                "this trade belongs to a holding-account transfer and cannot be edited — \
                 delete the transfer and re-transfer instead",
            ),
            UpsertError::EssVestTrade => ApiError::unprocessable(
                "this trade is an ESS vest and cannot be edited — delete the ESS statement \
                 and re-vest instead",
            ),
            UpsertError::InheritedParcelTrade => ApiError::unprocessable(
                "this trade is an inherited parcel and cannot be edited here — edit its \
                 inheritance (PUT /inheritances/:id) instead",
            ),
            UpsertError::ReinvestmentTrade => ApiError::unprocessable(
                "this trade is a DRP reinvestment and cannot be edited — undo the \
                 reinvestment via its distribution (DELETE /income/:id/reinvest) and \
                 re-reinvest instead",
            ),
            UpsertError::RightsAnchorParcel => ApiError::unprocessable(
                "this parcel anchors a rights sale and cannot be edited — delete the rights \
                 sale, edit, then re-enter it",
            ),
            UpsertError::Db(err) => err.into(),
        }
    }
}
