use axum::{
    Json, Router,
    extract::{Path, State},
    http::StatusCode,
    routing::get,
};
use chrono::{Datelike, NaiveDate};
use crate::infra::decimal::parse_dec;
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use sqlx::{Row, SqlitePool};
use std::collections::HashSet;

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, sqlx::Type)]
pub enum TradeType {
    Buy,
    Sell,
    DRP,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Trade {
    pub id: i64,
    pub trade_type: TradeType,
    pub date: NaiveDate,
    pub settlement_date: NaiveDate,
    pub listing_id: i64,
    pub average_price: Decimal,
    pub quantity: Decimal,
    pub currency: String,
    /// Always stored ex-GST: when a request flags `brokerage_includes_gst`,
    /// the entered inclusive amount is split at write time (see
    /// `split_gst_inclusive`) before it lands here, so the cost-base
    /// arithmetic (`brokerage + gst_on_brokerage`) holds unconditionally.
    pub brokerage: Decimal,
    pub gst_on_brokerage: Decimal,
    /// Records that the brokerage amount was *entered* GST-inclusive and
    /// server-split. Persisted only so the entry form can round-trip the
    /// trade (re-presenting `brokerage + gst_on_brokerage` as one inclusive
    /// amount); the stored money columns are already split, so nothing else
    /// reads it.
    pub brokerage_includes_gst: bool,
    pub brokerage_currency: String,
    /// Manual foreign-per-AUD override (same convention as the ATO rate: AUD =
    /// foreign / fx_rate). Reports prefer the ATO RBA rate for the trade's month
    /// and fall back to this field only when no ATO rate exists (see `infra::fx`).
    /// 1.0 for AUD trades.
    pub fx_rate: Decimal,
    pub contract_note_ref: Option<String>,
    /// The broker statement's net transaction total in the brokerage
    /// currency, kept for cross-referencing against the contract note.
    /// Validated at write time (see `check_statement_total`) — a value that
    /// doesn't reconcile with quantity × price ± (brokerage + GST) is
    /// rejected — and informational-only after that: no report or
    /// calculation uses it.
    pub statement_total: Option<Decimal>,
    /// DRP reinvestment residual cash (DRP trades only; 0 for Buy/Sell). When a
    /// distribution doesn't divide evenly into whole shares, the leftover is
    /// carried forward to the next reinvestment or paid out. These are populated
    /// by the reinvestment operation (see `entities::drp_reinvestment`); a
    /// manually entered DRP trade leaves them 0.
    pub residual_brought_forward: Decimal,
    pub residual_carried_forward: Decimal,
    pub residual_paid_out: Decimal,
    /// Provenance link from a rights-exercise Buy back to its `RightsIssue`
    /// corporate action (`None` for every other trade). Set only by
    /// `POST /corporate_actions/:id/exercise` (`entities::rights_exercise`),
    /// which uses it to cap cumulative exercised units at the entitlement; a
    /// trade carrying it is rejected by `PUT /trades` (delete and re-exercise
    /// instead), and the action it references cannot be edited or deleted
    /// while the trade exists.
    pub rights_action_id: Option<i64>,
    /// Provenance link from a buy-back participation Sell back to its
    /// `BuyBack` corporate action (`None` for every other trade). Set only by
    /// `POST /corporate_actions/:id/participate`
    /// (`entities::buyback_participation`). A trade carrying it is rejected
    /// by `PUT /sells` (delete it — which also removes the linked
    /// dividend-component income row — and re-participate instead), and the
    /// action it references cannot be edited or deleted while the trade
    /// exists.
    pub buyback_action_id: Option<i64>,
    /// Provenance link from a scrip-for-scrip exchange trade — the closing
    /// Sell on the original listing or a replacement Buy on the new one —
    /// back to its `ScripForScrip` corporate action (`None` for every other
    /// trade). Set only by `POST /corporate_actions/:id/exchange`
    /// (`entities::scrip_exchange`). The trades carrying one action id form
    /// the exchange group: each is rejected by `PUT /sells` and
    /// `PUT`/`DELETE /trades`; `DELETE /sells` on the closing Sell removes
    /// the whole group; and the action cannot be edited or deleted while any
    /// exists.
    pub scrip_action_id: Option<i64>,
    /// Provenance link from a demerger trade — the closing Sell on the head
    /// listing, a head replacement Buy, or a demerged-entity Buy — back to
    /// its `Demerger` corporate action (`None` for every other trade). Set
    /// only by `POST /corporate_actions/:id/demerge` (`entities::demerger`).
    /// The trades carrying one action id form the demerger group: each is
    /// rejected by `PUT /sells` and `PUT`/`DELETE /trades`; `DELETE /sells`
    /// on the closing Sell removes the whole group; and the action cannot be
    /// edited or deleted while any exists.
    pub demerger_action_id: Option<i64>,
    /// The CGT acquisition date deemed for this parcel when it differs from
    /// `date`: set only on scrip-for-scrip replacement Buys and demerger
    /// head/demerged Buys, carrying the consumed parcel's acquisition date
    /// (the rollovers count the combined holding period — see
    /// `docs/ato/takeovers-and-scrip-for-scrip.md` and `docs/ato/demergers.md`).
    /// Drives the 12-month discount clock and the AUD translation month of
    /// the cost base in the reports; split/return-of-capital applicability
    /// stays on the actual `date` (the replacement shares only exist in
    /// their listing from the exchange/demerger on). `None` = the trade's
    /// own date.
    pub deemed_acquisition_date: Option<NaiveDate>,
    /// The holding account the trade sits in (see
    /// `entities::holding_account`): the same listing can be held in more
    /// than one account at once — e.g. RSU-vested shares in an employer plan
    /// account alongside DRP-enrolled shares in a personal broker account.
    /// Defaults to the seeded default account when omitted from a request.
    pub holding_account_id: i64,
    /// Provenance link from a transfer-out Sell / transfer-in Buy back to its
    /// holding-account transfer (`None` for every other trade). Set only by
    /// `PUT /transfers/:id` (`entities::transfer`). The trades carrying one
    /// transfer id form the transfer group: each is rejected by `PUT /sells`
    /// and `PUT`/`DELETE /trades`; `DELETE /transfers/:id` removes the whole
    /// group.
    pub transfer_id: Option<i64>,
}

impl<'r> sqlx::FromRow<'r, sqlx::sqlite::SqliteRow> for Trade {
    fn from_row(row: &'r sqlx::sqlite::SqliteRow) -> Result<Self, sqlx::Error> {
        fn dec(s: String) -> Result<Decimal, sqlx::Error> {
            s.parse().map_err(|e: rust_decimal::Error| sqlx::Error::Decode(Box::new(e)))
        }
        Ok(Trade {
            id: row.try_get("id")?,
            trade_type: row.try_get::<TradeType, _>("trade_type")?,
            date: row.try_get("date")?,
            settlement_date: row.try_get("settlement_date")?,
            listing_id: row.try_get("listing_id")?,
            average_price: dec(row.try_get("average_price")?)?,
            quantity: dec(row.try_get("quantity")?)?,
            currency: row.try_get("currency")?,
            brokerage: dec(row.try_get("brokerage")?)?,
            gst_on_brokerage: dec(row.try_get("gst_on_brokerage")?)?,
            brokerage_includes_gst: row.try_get("brokerage_includes_gst")?,
            brokerage_currency: row.try_get("brokerage_currency")?,
            fx_rate: dec(row.try_get("fx_rate")?)?,
            contract_note_ref: row.try_get("contract_note_ref")?,
            statement_total: row
                .try_get::<Option<String>, _>("statement_total")?
                .map(dec)
                .transpose()?,
            residual_brought_forward: dec(row.try_get("residual_brought_forward")?)?,
            residual_carried_forward: dec(row.try_get("residual_carried_forward")?)?,
            residual_paid_out: dec(row.try_get("residual_paid_out")?)?,
            rights_action_id: row.try_get("rights_action_id")?,
            buyback_action_id: row.try_get("buyback_action_id")?,
            scrip_action_id: row.try_get("scrip_action_id")?,
            demerger_action_id: row.try_get("demerger_action_id")?,
            deemed_acquisition_date: row.try_get("deemed_acquisition_date")?,
            holding_account_id: row.try_get("holding_account_id")?,
            transfer_id: row.try_get("transfer_id")?,
        })
    }
}

#[derive(Debug, Deserialize)]
pub struct TradeBody {
    pub trade_type: TradeType,
    pub date: NaiveDate,
    #[serde(default)]
    pub settlement_date: Option<NaiveDate>,
    pub listing_id: i64,
    pub average_price: Decimal,
    pub quantity: Decimal,
    pub currency: String,
    /// GST-inclusive when `brokerage_includes_gst` is set (the server splits
    /// it; any supplied `gst_on_brokerage` is ignored), ex-GST otherwise.
    pub brokerage: Decimal,
    #[serde(default)]
    pub gst_on_brokerage: Decimal,
    #[serde(default)]
    pub brokerage_includes_gst: bool,
    pub brokerage_currency: String,
    pub fx_rate: Decimal,
    #[serde(default)]
    pub contract_note_ref: Option<String>,
    /// Optional statement cross-check; see `Trade::statement_total`.
    #[serde(default)]
    pub statement_total: Option<Decimal>,
    #[serde(default)]
    pub residual_brought_forward: Decimal,
    #[serde(default)]
    pub residual_carried_forward: Decimal,
    #[serde(default)]
    pub residual_paid_out: Decimal,
    /// Defaults to the seeded default holding account when omitted, so
    /// single-account clients never see the dimension.
    #[serde(default = "crate::entities::holding_account::default_holding_account_id")]
    pub holding_account_id: i64,
}

pub fn router() -> Router<SqlitePool> {
    Router::new()
        .route("/trades", get(list))
        .route("/trades/{id}", get(get_one).put(upsert).delete(delete))
}

/// Split a GST-inclusive brokerage amount into its (ex-GST brokerage, GST)
/// components. Australian GST is 10%, so an inclusive amount carries 1/11
/// GST; the GST is rounded to the cent (half away from zero, matching how
/// broker statements quote it) and the brokerage keeps the exact remainder,
/// so the pair always sums back to the amount actually paid.
pub(crate) fn split_gst_inclusive(amount: Decimal) -> (Decimal, Decimal) {
    let gst = (amount / Decimal::from(11))
        .round_dp_with_strategy(2, rust_decimal::RoundingStrategy::MidpointAwayFromZero);
    (amount - gst, gst)
}

/// Resolve a request's brokerage pair: the server-side ÷11 split when the
/// amount was entered GST-inclusive (any supplied GST value is ignored —
/// deriving it is the point of the flag), or the values as entered otherwise.
pub(crate) fn resolve_brokerage(
    includes_gst: bool,
    brokerage: Decimal,
    gst_on_brokerage: Decimal,
) -> (Decimal, Decimal) {
    if includes_gst {
        split_gst_inclusive(brokerage)
    } else {
        (brokerage, gst_on_brokerage)
    }
}

/// Why a supplied statement total failed to reconcile (both map to 422).
#[derive(Debug, PartialEq)]
pub(crate) enum StatementTotalError {
    /// The trade and brokerage currencies differ, so no single-currency
    /// total exists to check against — supplying one is rejected rather
    /// than inventing an FX conversion.
    CurrencyMismatch,
    /// The supplied total does not equal the computed figure (carried so
    /// the rejection can say what the trade actually adds up to).
    TotalMismatch { expected: Decimal },
}

/// Cross-check an optionally supplied statement total against the trade's
/// own figures: quantity × price + brokerage + GST for a Buy/DRP (amount
/// payable), quantity × price − brokerage − GST for a Sell (net proceeds
/// receivable — the statement nets costs out). Comparison is numeric
/// (`Decimal` equality ignores trailing zeros: 1234.50 matches 1234.5).
/// `None` means the statement total wasn't recorded — nothing to check.
pub(crate) fn check_statement_total(
    statement_total: Option<Decimal>,
    trade_type: TradeType,
    quantity: Decimal,
    average_price: Decimal,
    brokerage: Decimal,
    gst_on_brokerage: Decimal,
    currency: &str,
    brokerage_currency: &str,
) -> Result<(), StatementTotalError> {
    let Some(total) = statement_total else {
        return Ok(());
    };
    if currency != brokerage_currency {
        return Err(StatementTotalError::CurrencyMismatch);
    }
    let costs = brokerage + gst_on_brokerage;
    let expected = match trade_type {
        TradeType::Buy | TradeType::DRP => quantity * average_price + costs,
        TradeType::Sell => quantity * average_price - costs,
    };
    if total != expected {
        return Err(StatementTotalError::TotalMismatch { expected });
    }
    Ok(())
}

pub async fn db_list(pool: &SqlitePool) -> Result<Vec<Trade>, sqlx::Error> {
    sqlx::query_as(
        "SELECT id, trade_type, date, settlement_date, listing_id, average_price, quantity, \
         currency, brokerage, gst_on_brokerage, brokerage_includes_gst, brokerage_currency, \
         fx_rate, contract_note_ref, statement_total, \
         residual_brought_forward, residual_carried_forward, residual_paid_out, rights_action_id, \
         buyback_action_id, scrip_action_id, demerger_action_id, deemed_acquisition_date, \
         holding_account_id, transfer_id \
         FROM trades ORDER BY date, id",
    )
    .fetch_all(pool)
    .await
}

pub async fn db_get(pool: &SqlitePool, id: i64) -> Result<Option<Trade>, sqlx::Error> {
    sqlx::query_as(
        "SELECT id, trade_type, date, settlement_date, listing_id, average_price, quantity, \
         currency, brokerage, gst_on_brokerage, brokerage_includes_gst, brokerage_currency, \
         fx_rate, contract_note_ref, statement_total, \
         residual_brought_forward, residual_carried_forward, residual_paid_out, rights_action_id, \
         buyback_action_id, scrip_action_id, demerger_action_id, deemed_acquisition_date, \
         holding_account_id, transfer_id \
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
    /// A supplied statement total failed the cross-check (see
    /// `check_statement_total`): it doesn't reconcile with the trade's own
    /// figures, or the trade and brokerage currencies differ so there is no
    /// single-currency total to check.
    StatementTotal(StatementTotalError),
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
    // The statement total (when recorded) must reconcile with the trade's own
    // figures — a mismatch is a data-entry error against the contract note,
    // caught before anything is written.
    check_statement_total(
        trade.statement_total,
        trade.trade_type,
        trade.quantity,
        trade.average_price,
        trade.brokerage,
        trade.gst_on_brokerage,
        &trade.currency,
        &trade.brokerage_currency,
    )
    .map_err(UpsertError::StatementTotal)?;

    let mut tx = pool.begin().await?;

    // A rights-exercise, buy-back participation, scrip-for-scrip exchange,
    // or demerger trade is immutable here: it was created against its
    // action's terms (entitlement cap / dividend-capital split / carried
    // cost base and deemed acquisition date), which an edit could silently
    // break. (The INSERT below never sets any provenance column, so a normal
    // trade can't become one either.)
    let existing_action: Option<(Option<i64>, Option<i64>, Option<i64>, Option<i64>, Option<i64>)> =
        sqlx::query_as(
            "SELECT rights_action_id, buyback_action_id, scrip_action_id, demerger_action_id, \
                    transfer_id \
             FROM trades WHERE id = ?",
        )
        .bind(trade.id)
        .fetch_optional(&mut *tx)
        .await?;
    if let Some((rights, buyback, scrip, demerger, transfer)) = existing_action {
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
          fx_rate, contract_note_ref, statement_total, \
          residual_brought_forward, residual_carried_forward, residual_paid_out, \
          holding_account_id) \
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?) \
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
    .bind(trade.average_price.to_string())
    .bind(trade.quantity.to_string())
    .bind(&trade.currency)
    .bind(trade.brokerage.to_string())
    .bind(trade.gst_on_brokerage.to_string())
    .bind(trade.brokerage_includes_gst)
    .bind(&trade.brokerage_currency)
    .bind(trade.fx_rate.to_string())
    .bind(&trade.contract_note_ref)
    .bind(trade.statement_total.map(|d| d.to_string()))
    .bind(trade.residual_brought_forward.to_string())
    .bind(trade.residual_carried_forward.to_string())
    .bind(trade.residual_paid_out.to_string())
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
    /// allocation (or a Sell with allocations), by an AMIT adjustment, or as a
    /// distribution's reinvestment trade — or it belongs to a scrip-for-scrip
    /// exchange or demerger group, which is only ever deleted as a whole (via
    /// `DELETE /sells` on the group's closing Sell). Deleting it would orphan
    /// those dependants or break the rollover's parcel substitution, so the
    /// request is refused (mapped to 422) rather than surfacing the SQLite FK
    /// error as a 500. Remove the dependants first (e.g. delete the Sell via
    /// `DELETE /sells/:id`).
    Referenced,
}

pub async fn db_delete(pool: &SqlitePool, id: i64) -> Result<DeleteOutcome, sqlx::Error> {
    let mut tx = pool.begin().await?;

    let exists: Option<(Option<i64>, Option<i64>, Option<i64>)> = sqlx::query_as(
        "SELECT scrip_action_id, demerger_action_id, transfer_id FROM trades WHERE id = ?",
    )
    .bind(id)
    .fetch_optional(&mut *tx)
    .await?;
    let Some((scrip_action, demerger_action, transfer)) = exists else {
        return Ok(DeleteOutcome::NotFound);
    };
    // A scrip-for-scrip exchange, demerger, or holding-account transfer trade
    // is never deleted individually — the group's closing Sell and
    // replacement Buys substitute the same parcels, so they are removed as a
    // whole via DELETE /sells on the closing Sell (or DELETE /transfers/:id).
    if scrip_action.is_some() || demerger_action.is_some() || transfer.is_some() {
        return Ok(DeleteOutcome::Referenced);
    }

    let referenced: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM parcel_allocations \
                       WHERE purchase_trade_id = ?1 OR sale_trade_id = ?1) \
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

/// Advance `date` by `business_days` trading days, skipping Saturdays, Sundays
/// and the exchange's public `holidays`.
///
/// Market settlement is quoted as T+n *business* days (e.g. ASX T+2), so a Thursday
/// trade settles the following Monday, not Saturday — and a settlement that would
/// land on a public holiday rolls forward to the next trading day. Pass the
/// exchange's holiday set (see `exchange_holiday::exchange_holidays_for_listing`);
/// an empty set degrades to weekend-only skipping.
pub(crate) fn add_business_days(
    date: NaiveDate,
    business_days: i64,
    holidays: &HashSet<NaiveDate>,
) -> NaiveDate {
    use chrono::Weekday;
    let mut result = date;
    let mut remaining = business_days;
    while remaining > 0 {
        result += chrono::Duration::days(1);
        let is_weekend = matches!(result.weekday(), Weekday::Sat | Weekday::Sun);
        if !is_weekend && !holidays.contains(&result) {
            remaining -= 1;
        }
    }
    result
}

/// Warn when an auto-computed settlement window falls outside the seeded
/// holiday coverage for the listing's exchange: `add_business_days` silently
/// degrades to weekend-only skipping there, so the date may be wrong if the
/// exchange observes a holiday in the window. Non-blocking — the write
/// proceeds; the settlement-holiday-coverage report
/// (`GET /reports/settlement_holiday_coverage`) flags the persisted trades.
pub(crate) fn warn_if_outside_holiday_coverage(
    trade_id: i64,
    date: NaiveDate,
    settlement_date: NaiveDate,
    holidays: &HashSet<NaiveDate>,
) {
    use crate::entities::exchange_holiday::{coverage_span, window_outside_coverage};
    if window_outside_coverage(date, settlement_date, coverage_span(holidays)) {
        tracing::warn!(
            trade_id,
            %date,
            %settlement_date,
            "settlement window outside seeded exchange-holiday coverage; computed skipping weekends only"
        );
    }
}

/// The listing's exchange T+n settlement period, or `None` for an
/// exchange-less (Crypto) listing — those settle same-day.
pub(crate) async fn settlement_days_for_listing(
    pool: &SqlitePool,
    listing_id: i64,
) -> Result<Option<i64>, sqlx::Error> {
    sqlx::query_scalar(
        "SELECT e.settlement_days FROM listings l \
         LEFT JOIN exchanges e ON e.mic = l.exchange_mic \
         WHERE l.id = ?",
    )
    .bind(listing_id)
    .fetch_one(pool)
    .await
}

/// Auto-populate a settlement date for a trade with none supplied. An
/// exchange-listed security settles T+n business days after the trade date,
/// skipping weekends and the exchange's seeded holidays (warning when the
/// window leaves seeded coverage). An exchange-less (Crypto) listing settles
/// same-day — no T+n, no holiday calendar, no coverage warning.
pub(crate) async fn auto_settlement_date(
    pool: &SqlitePool,
    trade_id: i64,
    listing_id: i64,
    date: NaiveDate,
) -> Result<NaiveDate, sqlx::Error> {
    let Some(days) = settlement_days_for_listing(pool, listing_id).await? else {
        return Ok(date);
    };
    let holidays =
        crate::entities::exchange_holiday::exchange_holidays_for_listing(pool, listing_id).await?;
    let settlement = add_business_days(date, days, &holidays);
    warn_if_outside_holiday_coverage(trade_id, date, settlement, &holidays);
    Ok(settlement)
}

async fn list(State(pool): State<SqlitePool>) -> Result<Json<Vec<Trade>>, StatusCode> {
    db_list(&pool)
        .await
        .map(Json)
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)
}

async fn get_one(
    State(pool): State<SqlitePool>,
    Path(id): Path<i64>,
) -> Result<Json<Trade>, StatusCode> {
    db_get(&pool, id)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
        .map(Json)
        .ok_or(StatusCode::NOT_FOUND)
}

async fn upsert(
    State(pool): State<SqlitePool>,
    Path(id): Path<i64>,
    Json(body): Json<TradeBody>,
) -> Result<StatusCode, (StatusCode, String)> {
    // Sells must be created via PUT /sells/{id} so they are persisted together
    // with a full set of parcel allocations (no uncovered Sell can exist).
    if body.trade_type == TradeType::Sell {
        return Err((
            StatusCode::UNPROCESSABLE_ENTITY,
            "a Sell must be created via PUT /sells/:id so it carries its parcel allocations"
                .to_string(),
        ));
    }
    let settlement_date = match body.settlement_date {
        Some(d) => d,
        None => auto_settlement_date(&pool, id, body.listing_id, body.date)
            .await
            .map_err(|_| (StatusCode::INTERNAL_SERVER_ERROR, String::new()))?,
    };
    // A GST-inclusive brokerage entry is split here, at the API boundary, so
    // the stored columns (and `Trade` itself) are always ex-GST + GST.
    let (brokerage, gst_on_brokerage) =
        resolve_brokerage(body.brokerage_includes_gst, body.brokerage, body.gst_on_brokerage);
    let trade = Trade {
        id,
        trade_type: body.trade_type,
        date: body.date,
        settlement_date,
        listing_id: body.listing_id,
        average_price: body.average_price,
        quantity: body.quantity,
        currency: body.currency,
        brokerage,
        gst_on_brokerage,
        brokerage_includes_gst: body.brokerage_includes_gst,
        brokerage_currency: body.brokerage_currency,
        fx_rate: body.fx_rate,
        contract_note_ref: body.contract_note_ref,
        statement_total: body.statement_total,
        residual_brought_forward: body.residual_brought_forward,
        residual_carried_forward: body.residual_carried_forward,
        residual_paid_out: body.residual_paid_out,
        rights_action_id: None,
        buyback_action_id: None,
        scrip_action_id: None,
        demerger_action_id: None,
        deemed_acquisition_date: None,
        holding_account_id: body.holding_account_id,
        transfer_id: None,
    };
    db_upsert(&pool, &trade)
        .await
        .map(|_| StatusCode::NO_CONTENT)
        .map_err(|e| match e {
            UpsertError::Db(err) => crate::infra::http::write_error_body(&err),
            // The cross-check rejection says what the trade adds up to, so a
            // typo is findable without re-deriving the figure by hand.
            UpsertError::StatementTotal(detail) => {
                (StatusCode::UNPROCESSABLE_ENTITY, statement_total_detail(&detail))
            }
            UpsertError::QuantityBelowAllocated => (
                StatusCode::UNPROCESSABLE_ENTITY,
                "the new quantity is below what Sell allocations already draw from this parcel"
                    .to_string(),
            ),
            UpsertError::QuantityBelowAmitAdjustment => (
                StatusCode::UNPROCESSABLE_ENTITY,
                "the new quantity is below a linked AMIT adjustment's covered quantity".to_string(),
            ),
            UpsertError::RightsExerciseTrade => (
                StatusCode::UNPROCESSABLE_ENTITY,
                "this trade is a rights exercise and cannot be edited — delete it and \
                 re-exercise instead"
                    .to_string(),
            ),
            UpsertError::BuyBackTrade => (
                StatusCode::UNPROCESSABLE_ENTITY,
                "this trade is a buy-back participation and cannot be edited — delete it and \
                 re-participate instead"
                    .to_string(),
            ),
            UpsertError::ScripExchangeTrade => (
                StatusCode::UNPROCESSABLE_ENTITY,
                "this trade belongs to a scrip-for-scrip exchange and cannot be edited — \
                 delete the group and re-exchange instead"
                    .to_string(),
            ),
            UpsertError::DemergerTrade => (
                StatusCode::UNPROCESSABLE_ENTITY,
                "this trade belongs to a demerger and cannot be edited — delete the group and \
                 re-demerge instead"
                    .to_string(),
            ),
            UpsertError::TransferTrade => (
                StatusCode::UNPROCESSABLE_ENTITY,
                "this trade belongs to a holding-account transfer and cannot be edited — \
                 delete the transfer and re-transfer instead"
                    .to_string(),
            ),
        })
}

/// Human-readable body for a statement-total 422 (shown by the web UI).
pub(crate) fn statement_total_detail(e: &StatementTotalError) -> String {
    match e {
        StatementTotalError::CurrencyMismatch => {
            "statement_total can only be checked when the trade and brokerage \
             currencies match — omit it for mixed-currency trades"
                .to_string()
        }
        StatementTotalError::TotalMismatch { expected } => {
            format!("statement_total does not reconcile: the trade computes to {expected}")
        }
    }
}

async fn delete(
    State(pool): State<SqlitePool>,
    Path(id): Path<i64>,
) -> Result<StatusCode, (StatusCode, String)> {
    match db_delete(&pool, id).await {
        Ok(DeleteOutcome::Deleted) => Ok(StatusCode::NO_CONTENT),
        Ok(DeleteOutcome::NotFound) => {
            Err((StatusCode::NOT_FOUND, "no trade with that id".to_string()))
        }
        Ok(DeleteOutcome::Referenced) => Err((
            StatusCode::UNPROCESSABLE_ENTITY,
            "this trade is referenced by a sale allocation, AMIT adjustment, reinvestment, or \
             a scrip-for-scrip/demerger group — remove those first (e.g. delete the Sell)"
                .to_string(),
        )),
        Err(_) => Err((StatusCode::INTERNAL_SERVER_ERROR, String::new())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{infra::db, entities::listing};
    use axum::{body::Body, http::Request};
    use http_body_util::BodyExt;
    use rust_decimal::Decimal;
    use std::collections::HashSet;
    use tower::ServiceExt;

    async fn test_pool() -> SqlitePool {
        db::init(":memory:").await.unwrap()
    }

    async fn insert_test_listing(pool: &SqlitePool) {
        listing::db_upsert(
            pool,
            &listing::Listing {
                id: 1,
                exchange_mic: Some("XASX".to_string()),
                ticker: "VAS".to_string(),
                name: "Vanguard Australian Shares ETF".to_string(),
                isin: None,
                security_type: listing::SecurityType::ETF,
                currency: "AUD".to_string(),
                amit: false,
                preference: false,
            },
        )
        .await
        .unwrap();
    }

    fn buy_trade() -> Trade {
        Trade {
            brokerage_includes_gst: false,
            statement_total: None,
            holding_account_id: 1,
            transfer_id: None,
            id: 1,
            trade_type: TradeType::Buy,
            date: NaiveDate::from_ymd_opt(2024, 1, 15).unwrap(),
            settlement_date: NaiveDate::from_ymd_opt(2024, 1, 17).unwrap(),
            listing_id: 1,
            average_price: Decimal::from(100),
            quantity: Decimal::from(10),
            currency: "AUD".to_string(),
            brokerage: "9.95".parse().unwrap(),
            gst_on_brokerage: "0.995".parse().unwrap(),
            brokerage_currency: "AUD".to_string(),
            fx_rate: Decimal::ONE,
            contract_note_ref: Some("CN001".to_string()),
            residual_brought_forward: Decimal::ZERO,
            residual_carried_forward: Decimal::ZERO,
            residual_paid_out: Decimal::ZERO,
            rights_action_id: None,
            buyback_action_id: None,
            scrip_action_id: None,
            demerger_action_id: None,
            deemed_acquisition_date: None,
        }
    }

    /// Sell `qty` units out of the Buy parcel `buy_id` (listing 1), via the
    /// atomic Sell + allocation path.
    async fn insert_sell_consuming(pool: &SqlitePool, sell_id: i64, buy_id: i64, qty: Decimal) {
        use crate::entities::sell;
        sell::db_upsert_sell(
            pool,
            sell_id,
            &sell::SellBody {
                brokerage_includes_gst: false,
                statement_total: None,
                holding_account_id: 1,
                date: NaiveDate::from_ymd_opt(2024, 6, 1).unwrap(),
                settlement_date: Some(NaiveDate::from_ymd_opt(2024, 6, 3).unwrap()),
                listing_id: 1,
                average_price: Decimal::from(120),
                quantity: qty,
                currency: "AUD".to_string(),
                brokerage: Decimal::ZERO,
                gst_on_brokerage: Decimal::ZERO,
                brokerage_currency: "AUD".to_string(),
                fx_rate: Decimal::ONE,
                contract_note_ref: None,
                allocations: vec![sell::AllocationInput {
                    purchase_trade_id: buy_id,
                    quantity_allocated: qty,
                }],
            },
        )
        .await
        .unwrap();
    }

    /// Link an AMIT adjustment covering `qty` units of trade `trade_id`
    /// (listing 1), creating the AMMA statement it hangs off.
    async fn insert_amit_adjustment_covering(pool: &SqlitePool, trade_id: i64, qty: Decimal) {
        use crate::entities::{amit_adjustment, amma};
        amma::db_upsert(
            pool,
            &amma::AmmaStatement {
                holding_account_id: 1,
                id: 1,
                listing_id: 1,
                tax_year_end_date: NaiveDate::from_ymd_opt(2024, 6, 30).unwrap(),
                units_held: qty,
                date_received: NaiveDate::from_ymd_opt(2024, 8, 15).unwrap(),
                australian_interest: Decimal::ZERO,
                australian_dividends_unfranked: Decimal::ZERO,
                franked_dividends: Decimal::ZERO,
                franking_credits: Decimal::ZERO,
                net_rent: Decimal::ZERO,
                foreign_income: Decimal::ZERO,
                foreign_tax_credits: Decimal::ZERO,
                other_income: Decimal::ZERO,
                cgt_discount_gains: Decimal::ZERO,
                cgt_indexation_gains: Decimal::ZERO,
                cgt_other_gains: Decimal::ZERO,
                capital_losses_applied: Decimal::ZERO,
                tax_deferred_amount: Decimal::ZERO,
                tax_free_amount: Decimal::ZERO,
                cost_base_adjustment: "0.05".parse().unwrap(),
                tfn_withholding_tax: Decimal::ZERO,
                currency: "AUD".to_string(),
            },
        )
        .await
        .unwrap();
        amit_adjustment::db_upsert(
            pool,
            &amit_adjustment::AmitAdjustment {
                id: 1,
                amma_statement_id: 1,
                trade_id,
                quantity: qty,
            },
        )
        .await
        .unwrap();
    }

    // DB-level tests

    #[tokio::test]
    async fn db_buy_trade_insert_and_retrieve() {
        let pool = test_pool().await;
        insert_test_listing(&pool).await;
        db_upsert(&pool, &buy_trade()).await.unwrap();
        let got = db_get(&pool, 1).await.unwrap().unwrap();
        assert_eq!(got.trade_type, TradeType::Buy);
        assert_eq!(got.quantity, Decimal::from(10));
        assert_eq!(got.average_price, Decimal::from(100));
        assert_eq!(got.settlement_date, NaiveDate::from_ymd_opt(2024, 1, 17).unwrap());
        assert_eq!(got.contract_note_ref, Some("CN001".to_string()));
    }

    #[tokio::test]
    async fn db_unknown_currency_rejected_on_both_currency_columns() {
        let pool = test_pool().await;
        insert_test_listing(&pool).await;

        fn assert_fk_error(err: UpsertError, column: &str) {
            match err {
                UpsertError::Db(e) => assert!(
                    e.to_string().contains("FOREIGN KEY"),
                    "expected {column} FK error, got: {e}"
                ),
                other => panic!("expected {column} FK error, got: {other:?}"),
            }
        }

        // 'ZZZ' is not a recognised currency → each currency column's FK rejects it.
        let mut bad_currency = buy_trade();
        bad_currency.currency = "ZZZ".to_string();
        assert_fk_error(db_upsert(&pool, &bad_currency).await.unwrap_err(), "currency");

        let mut bad_brokerage = buy_trade();
        bad_brokerage.brokerage_currency = "ZZZ".to_string();
        assert_fk_error(
            db_upsert(&pool, &bad_brokerage).await.unwrap_err(),
            "brokerage_currency",
        );

        // A seeded digital-token code (BTC) is a recognised currency and is accepted.
        let mut btc = buy_trade();
        btc.currency = "BTC".to_string();
        db_upsert(&pool, &btc).await.unwrap();
    }

    #[tokio::test]
    async fn db_sell_trade_insert_and_retrieve() {
        let pool = test_pool().await;
        insert_test_listing(&pool).await;
        let trade = Trade {
            brokerage_includes_gst: false,
            statement_total: None,
            holding_account_id: 1,
            transfer_id: None,
            id: 2,
            trade_type: TradeType::Sell,
            date: NaiveDate::from_ymd_opt(2024, 6, 1).unwrap(),
            settlement_date: NaiveDate::from_ymd_opt(2024, 6, 3).unwrap(),
            listing_id: 1,
            average_price: Decimal::from(120),
            quantity: Decimal::from(5),
            currency: "AUD".to_string(),
            brokerage: "9.95".parse().unwrap(),
            gst_on_brokerage: "0.995".parse().unwrap(),
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
            deemed_acquisition_date: None,
        };
        db_upsert(&pool, &trade).await.unwrap();
        let got = db_get(&pool, 2).await.unwrap().unwrap();
        assert_eq!(got.trade_type, TradeType::Sell);
        assert_eq!(got.quantity, Decimal::from(5));
    }

    #[tokio::test]
    async fn db_drp_trade_insert_and_retrieve() {
        let pool = test_pool().await;
        insert_test_listing(&pool).await;
        let trade = Trade {
            brokerage_includes_gst: false,
            statement_total: None,
            holding_account_id: 1,
            transfer_id: None,
            id: 3,
            trade_type: TradeType::DRP,
            date: NaiveDate::from_ymd_opt(2024, 3, 15).unwrap(),
            settlement_date: NaiveDate::from_ymd_opt(2024, 3, 15).unwrap(),
            listing_id: 1,
            average_price: Decimal::from(95),
            quantity: Decimal::from(2),
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
            deemed_acquisition_date: None,
        };
        db_upsert(&pool, &trade).await.unwrap();
        let got = db_get(&pool, 3).await.unwrap().unwrap();
        assert_eq!(got.trade_type, TradeType::DRP);
        assert_eq!(got.quantity, Decimal::from(2));
    }

    #[tokio::test]
    async fn db_drp_residual_fields_round_trip_with_precision() {
        let pool = test_pool().await;
        insert_test_listing(&pool).await;
        let mut trade = buy_trade();
        trade.id = 7;
        trade.trade_type = TradeType::DRP;
        trade.residual_brought_forward = "1.234567890".parse().unwrap();
        trade.residual_carried_forward = "0.987654321".parse().unwrap();
        trade.residual_paid_out = "2.500000001".parse().unwrap();
        db_upsert(&pool, &trade).await.unwrap();
        let got = db_get(&pool, 7).await.unwrap().unwrap();
        assert_eq!(got.residual_brought_forward, "1.234567890".parse::<Decimal>().unwrap());
        assert_eq!(got.residual_carried_forward, "0.987654321".parse::<Decimal>().unwrap());
        assert_eq!(got.residual_paid_out, "2.500000001".parse::<Decimal>().unwrap());
    }

    #[tokio::test]
    async fn db_non_drp_trade_defaults_residuals_to_zero() {
        // A plain Buy carries zero residuals (residuals are a DRP-only concept).
        let pool = test_pool().await;
        insert_test_listing(&pool).await;
        db_upsert(&pool, &buy_trade()).await.unwrap();
        let got = db_get(&pool, 1).await.unwrap().unwrap();
        assert_eq!(got.residual_brought_forward, Decimal::ZERO);
        assert_eq!(got.residual_carried_forward, Decimal::ZERO);
        assert_eq!(got.residual_paid_out, Decimal::ZERO);
    }

    #[tokio::test]
    async fn db_get_missing_returns_none() {
        let pool = test_pool().await;
        assert!(db_get(&pool, 999).await.unwrap().is_none());
    }

    // Buy-trade edit/delete integrity (symmetric with the Sell-side invariants)

    #[tokio::test]
    async fn db_delete_buy_consumed_by_allocation_is_refused() {
        let pool = test_pool().await;
        insert_test_listing(&pool).await;
        db_upsert(&pool, &buy_trade()).await.unwrap();
        insert_sell_consuming(&pool, 2, 1, Decimal::from(5)).await;

        assert_eq!(db_delete(&pool, 1).await.unwrap(), DeleteOutcome::Referenced);
        assert!(db_get(&pool, 1).await.unwrap().is_some(), "consumed buy must remain");
    }

    #[tokio::test]
    async fn db_delete_buy_covered_by_amit_adjustment_is_refused() {
        let pool = test_pool().await;
        insert_test_listing(&pool).await;
        db_upsert(&pool, &buy_trade()).await.unwrap();
        insert_amit_adjustment_covering(&pool, 1, Decimal::from(10)).await;

        assert_eq!(db_delete(&pool, 1).await.unwrap(), DeleteOutcome::Referenced);
        assert!(db_get(&pool, 1).await.unwrap().is_some(), "covered buy must remain");
    }

    #[tokio::test]
    async fn db_delete_drp_linked_to_income_reinvestment_is_refused() {
        // A DRP trade recorded as a distribution's reinvestment is referenced by
        // income.reinvestment_trade_id — deleting it would orphan that link.
        let pool = test_pool().await;
        insert_test_listing(&pool).await;
        let mut drp = buy_trade();
        drp.trade_type = TradeType::DRP;
        db_upsert(&pool, &drp).await.unwrap();
        sqlx::query(
            "INSERT INTO income (id, listing_id, date_paid, reinvestment_trade_id) \
             VALUES (1, 1, '2024-03-15', 1)",
        )
        .execute(&pool)
        .await
        .unwrap();

        assert_eq!(db_delete(&pool, 1).await.unwrap(), DeleteOutcome::Referenced);
        assert!(db_get(&pool, 1).await.unwrap().is_some(), "reinvestment trade must remain");
    }

    #[tokio::test]
    async fn db_shrink_buy_below_allocated_quantity_is_refused() {
        let pool = test_pool().await;
        insert_test_listing(&pool).await;
        db_upsert(&pool, &buy_trade()).await.unwrap(); // qty 10
        insert_sell_consuming(&pool, 2, 1, Decimal::from(5)).await;

        // Shrinking below the 5 already allocated out is refused…
        let mut shrunk = buy_trade();
        shrunk.quantity = Decimal::from(4);
        let err = db_upsert(&pool, &shrunk).await.unwrap_err();
        assert!(
            matches!(err, UpsertError::QuantityBelowAllocated),
            "expected QuantityBelowAllocated, got: {err:?}"
        );
        assert_eq!(db_get(&pool, 1).await.unwrap().unwrap().quantity, Decimal::from(10));

        // …but shrinking exactly to the allocated quantity is fine.
        let mut exact = buy_trade();
        exact.quantity = Decimal::from(5);
        db_upsert(&pool, &exact).await.unwrap();
        assert_eq!(db_get(&pool, 1).await.unwrap().unwrap().quantity, Decimal::from(5));
    }

    /// With a 2-for-1 split (TD 2000/10) between the buy and the sale, the
    /// sale's allocation is in post-split units: a 10-unit parcel that had 10
    /// post-split units (= 5 as-acquired) sold out of it can still shrink to
    /// 5, but not 4.
    #[tokio::test]
    async fn db_shrink_check_rebases_post_split_allocations() {
        let pool = test_pool().await;
        insert_test_listing(&pool).await;
        db_upsert(&pool, &buy_trade()).await.unwrap(); // qty 10, 2024-01-15
        crate::entities::corporate_action::db_upsert(
            &pool,
            &crate::entities::corporate_action::CorporateAction {
                id: 1,
                listing_id: 1,
                date: NaiveDate::from_ymd_opt(2024, 3, 1).unwrap(),
                kind: crate::entities::corporate_action::ActionKind::ShareSplit {
                    split_new_units: Decimal::from(2),
                    split_old_units: Decimal::ONE,
                },
            },
        )
        .await
        .unwrap();
        // Sell 10 post-split units (= 5 as-acquired) on 2024-06-01.
        insert_sell_consuming(&pool, 2, 1, Decimal::from(10)).await;

        // 4 < the 5 as-acquired units allocated out → refused…
        let mut shrunk = buy_trade();
        shrunk.quantity = Decimal::from(4);
        let err = db_upsert(&pool, &shrunk).await.unwrap_err();
        assert!(
            matches!(err, UpsertError::QuantityBelowAllocated),
            "expected QuantityBelowAllocated, got: {err:?}"
        );

        // …but exactly the 5 as-acquired units is fine.
        let mut exact = buy_trade();
        exact.quantity = Decimal::from(5);
        db_upsert(&pool, &exact).await.unwrap();
        assert_eq!(db_get(&pool, 1).await.unwrap().unwrap().quantity, Decimal::from(5));
    }

    #[tokio::test]
    async fn db_shrink_buy_below_amit_adjustment_quantity_is_refused() {
        let pool = test_pool().await;
        insert_test_listing(&pool).await;
        db_upsert(&pool, &buy_trade()).await.unwrap(); // qty 10
        insert_amit_adjustment_covering(&pool, 1, Decimal::from(8)).await;

        // Shrinking below the adjustment's 8 covered units is refused…
        let mut shrunk = buy_trade();
        shrunk.quantity = Decimal::from(7);
        let err = db_upsert(&pool, &shrunk).await.unwrap_err();
        assert!(
            matches!(err, UpsertError::QuantityBelowAmitAdjustment),
            "expected QuantityBelowAmitAdjustment, got: {err:?}"
        );
        assert_eq!(db_get(&pool, 1).await.unwrap().unwrap().quantity, Decimal::from(10));

        // …but shrinking exactly to the covered quantity is fine.
        let mut exact = buy_trade();
        exact.quantity = Decimal::from(8);
        db_upsert(&pool, &exact).await.unwrap();
        assert_eq!(db_get(&pool, 1).await.unwrap().unwrap().quantity, Decimal::from(8));
    }

    #[tokio::test]
    async fn db_unconsumed_buy_still_edits_and_deletes_freely() {
        let pool = test_pool().await;
        insert_test_listing(&pool).await;
        db_upsert(&pool, &buy_trade()).await.unwrap(); // qty 10

        let mut shrunk = buy_trade();
        shrunk.quantity = Decimal::ONE;
        db_upsert(&pool, &shrunk).await.unwrap();
        assert_eq!(db_get(&pool, 1).await.unwrap().unwrap().quantity, Decimal::ONE);

        assert_eq!(db_delete(&pool, 1).await.unwrap(), DeleteOutcome::Deleted);
        assert!(db_get(&pool, 1).await.unwrap().is_none());
    }

    // API-level tests

    #[tokio::test]
    async fn api_settlement_date_auto_populated() {
        let pool = test_pool().await;
        insert_test_listing(&pool).await;
        // XASX has settlement_days = 2, so 2024-01-15 + 2 = 2024-01-17
        let body = serde_json::json!({
            "trade_type": "Buy",
            "date": "2024-01-15",
            "listing_id": 1,
            "average_price": 100.0,
            "quantity": 10.0,
            "currency": "AUD",
            "brokerage": 9.95,
            "gst_on_brokerage": 0.995,
            "brokerage_currency": "AUD",
            "fx_rate": 1.0
        });
        let resp = router()
            .with_state(pool.clone())
            .oneshot(
                Request::builder()
                    .method("PUT")
                    .uri("/trades/1")
                    .header("content-type", "application/json")
                    .body(Body::from(body.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NO_CONTENT);
        let trade = db_get(&pool, 1).await.unwrap().unwrap();
        assert_eq!(trade.settlement_date, NaiveDate::from_ymd_opt(2024, 1, 17).unwrap());
    }

    /// An exchange-less (Crypto) listing settles same-day: the auto-populated
    /// settlement date is the trade date itself — a Friday stays a Friday (no
    /// T+n, no business-day skipping) — and no coverage warning fires (there
    /// is no holiday calendar to be outside of).
    #[tracing_test::traced_test]
    #[tokio::test]
    async fn api_settlement_date_same_day_for_crypto() {
        let pool = test_pool().await;
        listing::db_upsert(
            &pool,
            &listing::Listing {
                id: 1,
                exchange_mic: None,
                ticker: "BTC".to_string(),
                name: "Bitcoin".to_string(),
                isin: None,
                security_type: listing::SecurityType::Crypto,
                currency: "AUD".to_string(),
                amit: false,
                preference: false,
            },
        )
        .await
        .unwrap();
        // 2030-06-07 is a Friday, far outside every seeded holiday calendar.
        let body = serde_json::json!({
            "trade_type": "Buy",
            "date": "2030-06-07",
            "listing_id": 1,
            "average_price": "65000",
            "quantity": "0.12345678",
            "currency": "AUD",
            "brokerage": "0",
            "gst_on_brokerage": "0",
            "brokerage_currency": "AUD",
            "fx_rate": "1"
        });
        let resp = router()
            .with_state(pool.clone())
            .oneshot(
                Request::builder()
                    .method("PUT")
                    .uri("/trades/1")
                    .header("content-type", "application/json")
                    .body(Body::from(body.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NO_CONTENT);
        let trade = db_get(&pool, 1).await.unwrap().unwrap();
        assert_eq!(trade.settlement_date, NaiveDate::from_ymd_opt(2030, 6, 7).unwrap());
        assert!(!logs_contain("settlement window outside seeded exchange-holiday coverage"));
    }

    #[tracing_test::traced_test]
    #[tokio::test]
    async fn api_settlement_beyond_holiday_coverage_logs_warning() {
        let pool = test_pool().await;
        insert_test_listing(&pool).await;
        // XASX holidays are seeded 2024–2027 only: a 2030 trade's settlement is
        // computed skipping weekends only, so the auto-population warns rather
        // than silently using the incomplete calendar.
        let body = serde_json::json!({
            "trade_type": "Buy",
            "date": "2030-06-03",
            "listing_id": 1,
            "average_price": 100.0,
            "quantity": 10.0,
            "currency": "AUD",
            "brokerage": 0.0,
            "gst_on_brokerage": 0.0,
            "brokerage_currency": "AUD",
            "fx_rate": 1.0
        });
        let resp = router()
            .with_state(pool.clone())
            .oneshot(
                Request::builder()
                    .method("PUT")
                    .uri("/trades/1")
                    .header("content-type", "application/json")
                    .body(Body::from(body.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        // Non-blocking: the write succeeds, the warning surfaces the gap.
        assert_eq!(resp.status(), StatusCode::NO_CONTENT);
        assert!(logs_contain("settlement window outside seeded exchange-holiday coverage"));
    }

    #[tracing_test::traced_test]
    #[tokio::test]
    async fn api_settlement_inside_holiday_coverage_does_not_warn() {
        let pool = test_pool().await;
        insert_test_listing(&pool).await;
        let body = serde_json::json!({
            "trade_type": "Buy",
            "date": "2024-01-15",
            "listing_id": 1,
            "average_price": 100.0,
            "quantity": 10.0,
            "currency": "AUD",
            "brokerage": 0.0,
            "gst_on_brokerage": 0.0,
            "brokerage_currency": "AUD",
            "fx_rate": 1.0
        });
        let resp = router()
            .with_state(pool.clone())
            .oneshot(
                Request::builder()
                    .method("PUT")
                    .uri("/trades/1")
                    .header("content-type", "application/json")
                    .body(Body::from(body.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NO_CONTENT);
        assert!(!logs_contain("settlement window outside seeded exchange-holiday coverage"));
    }

    #[test]
    fn add_business_days_skips_weekend() {
        let none = HashSet::new();
        // 2024-01-18 is a Thursday; T+2 business days settles Monday 2024-01-22,
        // skipping Sat 2024-01-20 and Sun 2024-01-21.
        let thursday = NaiveDate::from_ymd_opt(2024, 1, 18).unwrap();
        assert_eq!(
            add_business_days(thursday, 2, &none),
            NaiveDate::from_ymd_opt(2024, 1, 22).unwrap()
        );
        // 2024-01-15 is a Monday; T+2 stays within the week (Wednesday).
        let monday = NaiveDate::from_ymd_opt(2024, 1, 15).unwrap();
        assert_eq!(
            add_business_days(monday, 2, &none),
            NaiveDate::from_ymd_opt(2024, 1, 17).unwrap()
        );
    }

    #[test]
    fn add_business_days_skips_public_holidays() {
        // Christmas Day (Wed) and Boxing Day (Thu) 2024 are public holidays.
        let holidays: HashSet<NaiveDate> = [
            NaiveDate::from_ymd_opt(2024, 12, 25).unwrap(),
            NaiveDate::from_ymd_opt(2024, 12, 26).unwrap(),
        ]
        .into_iter()
        .collect();
        // Tuesday 2024-12-24 + T+2: skip Wed 25 + Thu 26 (holidays), Fri 27 = 1,
        // skip the weekend, Mon 30 = 2 → settles 2024-12-30.
        let tuesday = NaiveDate::from_ymd_opt(2024, 12, 24).unwrap();
        assert_eq!(
            add_business_days(tuesday, 2, &holidays),
            NaiveDate::from_ymd_opt(2024, 12, 30).unwrap()
        );
        // Without the holiday set it would settle on Boxing Day (Thu 26).
        assert_eq!(
            add_business_days(tuesday, 2, &HashSet::new()),
            NaiveDate::from_ymd_opt(2024, 12, 26).unwrap()
        );
    }

    #[tokio::test]
    async fn api_settlement_date_skips_public_holiday() {
        let pool = test_pool().await;
        insert_test_listing(&pool).await; // listing 1 trades on XASX
        // XASX is closed Christmas (Wed 2024-12-25) and Boxing Day (Thu 2024-12-26);
        // a Tuesday 2024-12-24 buy at T+2 settles Mon 2024-12-30, not Thu 2024-12-26.
        let body = serde_json::json!({
            "trade_type": "Buy",
            "date": "2024-12-24",
            "listing_id": 1,
            "average_price": 100.0,
            "quantity": 10.0,
            "currency": "AUD",
            "brokerage": 9.95,
            "gst_on_brokerage": 0.995,
            "brokerage_currency": "AUD",
            "fx_rate": 1.0
        });
        let resp = router()
            .with_state(pool.clone())
            .oneshot(
                Request::builder()
                    .method("PUT")
                    .uri("/trades/1")
                    .header("content-type", "application/json")
                    .body(Body::from(body.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NO_CONTENT);
        let trade = db_get(&pool, 1).await.unwrap().unwrap();
        assert_eq!(trade.settlement_date, NaiveDate::from_ymd_opt(2024, 12, 30).unwrap());
    }

    #[tokio::test]
    async fn api_settlement_date_auto_populated_skips_weekend() {
        let pool = test_pool().await;
        insert_test_listing(&pool).await;
        // Friday 2024-01-19 + T+2 business days = Tuesday 2024-01-23 (skips the weekend).
        let body = serde_json::json!({
            "trade_type": "Buy",
            "date": "2024-01-19",
            "listing_id": 1,
            "average_price": 100.0,
            "quantity": 10.0,
            "currency": "AUD",
            "brokerage": 9.95,
            "gst_on_brokerage": 0.995,
            "brokerage_currency": "AUD",
            "fx_rate": 1.0
        });
        let resp = router()
            .with_state(pool.clone())
            .oneshot(
                Request::builder()
                    .method("PUT")
                    .uri("/trades/1")
                    .header("content-type", "application/json")
                    .body(Body::from(body.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NO_CONTENT);
        let trade = db_get(&pool, 1).await.unwrap().unwrap();
        assert_eq!(trade.settlement_date, NaiveDate::from_ymd_opt(2024, 1, 23).unwrap());
    }

    #[tokio::test]
    async fn api_put_sell_trade_is_rejected() {
        // Sells must go through PUT /sells/{id}; the generic trade endpoint rejects them.
        let pool = test_pool().await;
        insert_test_listing(&pool).await;
        let body = serde_json::json!({
            "trade_type": "Sell",
            "date": "2024-01-15",
            "listing_id": 1,
            "average_price": 100.0,
            "quantity": 10.0,
            "currency": "AUD",
            "brokerage": 9.95,
            "gst_on_brokerage": 0.995,
            "brokerage_currency": "AUD",
            "fx_rate": 1.0
        });
        let resp = router()
            .with_state(pool)
            .oneshot(
                Request::builder()
                    .method("PUT")
                    .uri("/trades/1")
                    .header("content-type", "application/json")
                    .body(Body::from(body.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY);
    }

    #[tokio::test]
    async fn api_settlement_date_override() {
        let pool = test_pool().await;
        insert_test_listing(&pool).await;
        let body = serde_json::json!({
            "trade_type": "Buy",
            "date": "2024-01-15",
            "settlement_date": "2024-01-20",
            "listing_id": 1,
            "average_price": 100.0,
            "quantity": 10.0,
            "currency": "AUD",
            "brokerage": 9.95,
            "gst_on_brokerage": 0.995,
            "brokerage_currency": "AUD",
            "fx_rate": 1.0
        });
        let resp = router()
            .with_state(pool.clone())
            .oneshot(
                Request::builder()
                    .method("PUT")
                    .uri("/trades/1")
                    .header("content-type", "application/json")
                    .body(Body::from(body.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NO_CONTENT);
        let trade = db_get(&pool, 1).await.unwrap().unwrap();
        assert_eq!(trade.settlement_date, NaiveDate::from_ymd_opt(2024, 1, 20).unwrap());
    }

    #[tokio::test]
    async fn api_list_returns_ok() {
        let pool = test_pool().await;
        insert_test_listing(&pool).await;
        db_upsert(&pool, &buy_trade()).await.unwrap();
        let resp = router()
            .with_state(pool)
            .oneshot(Request::builder().uri("/trades").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let trades: Vec<Trade> = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(trades.len(), 1);
        assert_eq!(trades[0].trade_type, TradeType::Buy);
    }

    #[tokio::test]
    async fn api_get_existing_returns_trade() {
        let pool = test_pool().await;
        insert_test_listing(&pool).await;
        db_upsert(&pool, &buy_trade()).await.unwrap();
        let resp = router()
            .with_state(pool)
            .oneshot(Request::builder().uri("/trades/1").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let t: Trade = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(t.trade_type, TradeType::Buy);
        assert_eq!(t.quantity, Decimal::from(10));
    }

    #[tokio::test]
    async fn api_get_missing_returns_404() {
        let pool = test_pool().await;
        let resp = router()
            .with_state(pool)
            .oneshot(Request::builder().uri("/trades/999").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn api_delete_existing_returns_no_content() {
        let pool = test_pool().await;
        insert_test_listing(&pool).await;
        db_upsert(&pool, &buy_trade()).await.unwrap();
        let resp = router()
            .with_state(pool)
            .oneshot(
                Request::builder()
                    .method("DELETE")
                    .uri("/trades/1")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NO_CONTENT);
    }

    #[tokio::test]
    async fn api_delete_missing_returns_404() {
        let pool = test_pool().await;
        let resp = router()
            .with_state(pool)
            .oneshot(
                Request::builder()
                    .method("DELETE")
                    .uri("/trades/999")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn api_delete_consumed_buy_returns_422() {
        let pool = test_pool().await;
        insert_test_listing(&pool).await;
        db_upsert(&pool, &buy_trade()).await.unwrap();
        insert_sell_consuming(&pool, 2, 1, Decimal::from(5)).await;

        let resp = router()
            .with_state(pool.clone())
            .oneshot(
                Request::builder()
                    .method("DELETE")
                    .uri("/trades/1")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY);
        assert!(db_get(&pool, 1).await.unwrap().is_some(), "consumed buy must remain");
    }

    #[tokio::test]
    async fn api_shrink_partly_sold_buy_returns_422() {
        let pool = test_pool().await;
        insert_test_listing(&pool).await;
        db_upsert(&pool, &buy_trade()).await.unwrap(); // qty 10
        insert_sell_consuming(&pool, 2, 1, Decimal::from(5)).await;

        // Editing the Buy down to 4 would leave the Sell's 5-unit allocation
        // drawing on units the parcel no longer has.
        let body = serde_json::json!({
            "trade_type": "Buy",
            "date": "2024-01-15",
            "settlement_date": "2024-01-17",
            "listing_id": 1,
            "average_price": "100",
            "quantity": "4",
            "currency": "AUD",
            "brokerage": "9.95",
            "gst_on_brokerage": "0.995",
            "brokerage_currency": "AUD",
            "fx_rate": "1"
        });
        let resp = router()
            .with_state(pool.clone())
            .oneshot(
                Request::builder()
                    .method("PUT")
                    .uri("/trades/1")
                    .header("content-type", "application/json")
                    .body(Body::from(body.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY);
        assert_eq!(db_get(&pool, 1).await.unwrap().unwrap().quantity, Decimal::from(10));
    }

    #[tokio::test]
    async fn api_decimal_precision_round_trip() {
        let pool = test_pool().await;
        insert_test_listing(&pool).await;
        let body = serde_json::json!({
            "trade_type": "Buy",
            "date": "2024-01-15",
            "listing_id": 1,
            "average_price": "99.9999999999",
            "quantity": "10.5",
            "currency": "AUD",
            "brokerage": "9.95",
            "gst_on_brokerage": "0.995",
            "brokerage_currency": "AUD",
            "fx_rate": "1.0"
        });
        let resp = router()
            .with_state(pool.clone())
            .oneshot(
                Request::builder()
                    .method("PUT")
                    .uri("/trades/1")
                    .header("content-type", "application/json")
                    .body(Body::from(body.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NO_CONTENT);
        let resp = router()
            .with_state(pool)
            .oneshot(Request::builder().uri("/trades/1").body(Body::empty()).unwrap())
            .await
            .unwrap();
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let t: Trade = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(t.average_price, "99.9999999999".parse::<Decimal>().unwrap());
        assert_eq!(t.quantity, "10.5".parse::<Decimal>().unwrap());
        assert_eq!(t.brokerage, "9.95".parse::<Decimal>().unwrap());
    }

    fn d(s: &str) -> Decimal {
        s.parse().unwrap()
    }

    /// PUT the JSON body to /trades/{id}, returning the status and response
    /// body text (the statement-total 422 carries its detail there).
    async fn put_trade_json(
        pool: &SqlitePool,
        id: i64,
        body: serde_json::Value,
    ) -> (StatusCode, String) {
        let resp = router()
            .with_state(pool.clone())
            .oneshot(
                Request::builder()
                    .method("PUT")
                    .uri(format!("/trades/{id}"))
                    .header("content-type", "application/json")
                    .body(Body::from(body.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        let status = resp.status();
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        (status, String::from_utf8(bytes.to_vec()).unwrap())
    }

    #[test]
    fn split_gst_inclusive_rounds_to_the_cent_and_sums_back_exactly() {
        // $9.95 incl.: 9.95/11 = 0.9045… → $0.90 GST, $9.05 ex-GST.
        assert_eq!(split_gst_inclusive(d("9.95")), (d("9.05"), d("0.90")));
        // $10 incl.: 10/11 = 0.9090… → $0.91 GST (rounded up to the cent).
        assert_eq!(split_gst_inclusive(d("10")), (d("9.09"), d("0.91")));
        // An exact half-cent rounds away from zero: 0.055/11 = 0.005 → $0.01.
        assert_eq!(split_gst_inclusive(d("0.055")), (d("0.045"), d("0.01")));
        // The pair always sums back to the amount paid.
        for amount in ["9.95", "10", "0.055", "19.99"] {
            let (brok, gst) = split_gst_inclusive(d(amount));
            assert_eq!(brok + gst, d(amount));
        }
    }

    /// A GST-inclusive entry is split by the server (any supplied GST value is
    /// ignored), the flag round-trips, and an edit re-splits the new amount.
    /// An unflagged entry keeps today's behaviour: both values stored as sent.
    #[tokio::test]
    async fn api_gst_inclusive_brokerage_is_split_and_round_trips() {
        let pool = test_pool().await;
        insert_test_listing(&pool).await;
        let body = serde_json::json!({
            "trade_type": "Buy",
            "date": "2024-01-15",
            "listing_id": 1,
            "average_price": "100",
            "quantity": "10",
            "currency": "AUD",
            "brokerage": "9.95",
            "gst_on_brokerage": "123",   // ignored: the server derives the split
            "brokerage_includes_gst": true,
            "brokerage_currency": "AUD",
            "fx_rate": "1"
        });
        let (status, _) = put_trade_json(&pool, 1, body).await;
        assert_eq!(status, StatusCode::NO_CONTENT);
        let t = db_get(&pool, 1).await.unwrap().unwrap();
        assert_eq!(t.brokerage, d("9.05"));
        assert_eq!(t.gst_on_brokerage, d("0.90"));
        assert!(t.brokerage_includes_gst, "flag must round-trip for the entry form");

        // Editing with a new inclusive amount re-splits it.
        let body = serde_json::json!({
            "trade_type": "Buy",
            "date": "2024-01-15",
            "listing_id": 1,
            "average_price": "100",
            "quantity": "10",
            "currency": "AUD",
            "brokerage": "11",
            "brokerage_includes_gst": true,
            "brokerage_currency": "AUD",
            "fx_rate": "1"
        });
        let (status, _) = put_trade_json(&pool, 1, body).await;
        assert_eq!(status, StatusCode::NO_CONTENT);
        let t = db_get(&pool, 1).await.unwrap().unwrap();
        assert_eq!(t.brokerage, d("10"));
        assert_eq!(t.gst_on_brokerage, d("1"));

        // Unflagged: stored exactly as entered (ex-GST + manual GST).
        let body = serde_json::json!({
            "trade_type": "Buy",
            "date": "2024-01-15",
            "listing_id": 1,
            "average_price": "100",
            "quantity": "10",
            "currency": "AUD",
            "brokerage": "9.95",
            "gst_on_brokerage": "0.995",
            "brokerage_currency": "AUD",
            "fx_rate": "1"
        });
        let (status, _) = put_trade_json(&pool, 2, body).await;
        assert_eq!(status, StatusCode::NO_CONTENT);
        let t = db_get(&pool, 2).await.unwrap().unwrap();
        assert_eq!(t.brokerage, d("9.95"));
        assert_eq!(t.gst_on_brokerage, d("0.995"));
        assert!(!t.brokerage_includes_gst);
        assert_eq!(t.statement_total, None);
    }

    /// The statement total must reconcile with quantity × price + brokerage +
    /// GST for a Buy: a matching figure (in any trailing-zero spelling) is
    /// accepted and stored; a mismatch is rejected with the computed figure in
    /// the 422 detail and nothing persisted.
    #[tokio::test]
    async fn api_statement_total_cross_check_on_buy() {
        let pool = test_pool().await;
        insert_test_listing(&pool).await;
        // 10 × 100 + 9.05 + 0.90 (from the 9.95 inclusive split) = 1009.95
        let body = serde_json::json!({
            "trade_type": "Buy",
            "date": "2024-01-15",
            "listing_id": 1,
            "average_price": "100",
            "quantity": "10",
            "currency": "AUD",
            "brokerage": "9.95",
            "brokerage_includes_gst": true,
            "brokerage_currency": "AUD",
            "fx_rate": "1",
            "statement_total": "1009.95"
        });
        let (status, _) = put_trade_json(&pool, 1, body.clone()).await;
        assert_eq!(status, StatusCode::NO_CONTENT);
        assert_eq!(db_get(&pool, 1).await.unwrap().unwrap().statement_total, Some(d("1009.95")));

        // Numeric comparison: trailing zeros don't matter.
        let mut zeros = body.clone();
        zeros["statement_total"] = "1009.9500".into();
        let (status, _) = put_trade_json(&pool, 1, zeros).await;
        assert_eq!(status, StatusCode::NO_CONTENT);

        // A mismatch is rejected, says what the trade computes to, and
        // persists nothing.
        let mut wrong = body.clone();
        wrong["statement_total"] = "1010".into();
        let (status, detail) = put_trade_json(&pool, 2, wrong).await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
        assert!(detail.contains("1009.95"), "detail must carry the computed figure: {detail}");
        assert!(db_get(&pool, 2).await.unwrap().is_none(), "nothing persisted");
    }

    /// A total can only be checked when the trade and brokerage currencies
    /// match — supplying one on a mixed-currency trade is rejected rather
    /// than inventing an FX conversion.
    #[tokio::test]
    async fn api_statement_total_on_mixed_currency_trade_returns_422() {
        let pool = test_pool().await;
        insert_test_listing(&pool).await;
        let body = serde_json::json!({
            "trade_type": "Buy",
            "date": "2024-01-15",
            "listing_id": 1,
            "average_price": "100",
            "quantity": "10",
            "currency": "USD",
            "brokerage": "9.95",
            "brokerage_currency": "AUD",
            "fx_rate": "1.5",
            "statement_total": "1009.95"
        });
        let (status, detail) = put_trade_json(&pool, 1, body).await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
        assert!(detail.contains("currencies"), "detail must explain the rejection: {detail}");
        assert!(db_get(&pool, 1).await.unwrap().is_none(), "nothing persisted");
    }

    /// The boolean column is CHECK-constrained to 0/1 in the database.
    #[tokio::test]
    async fn db_brokerage_includes_gst_check_constraint_enforced() {
        let pool = test_pool().await;
        insert_test_listing(&pool).await;
        db_upsert(&pool, &buy_trade()).await.unwrap();
        let err = sqlx::query("UPDATE trades SET brokerage_includes_gst = 2 WHERE id = 1")
            .execute(&pool)
            .await
            .unwrap_err();
        assert!(err.to_string().contains("CHECK"), "{err}");
    }
}
