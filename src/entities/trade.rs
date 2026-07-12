use crate::infra::decimal::{parse_dec, row_dec, row_opt_dec};
use crate::infra::http::ApiError;
use axum::{
    Json, Router,
    extract::{Path, State},
    http::StatusCode,
    routing::get,
};
use chrono::{Datelike, NaiveDate};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use sqlx::{Row, SqlitePool};
use std::collections::HashSet;

// `DRP` is serialized verbatim to JSON and persisted to the TEXT `trade_type`
// column (matched by a CHECK constraint), so the acronym spelling is the
// wire/storage format and must not be camel-cased.
#[allow(clippy::upper_case_acronyms)]
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
    /// Deliberate transaction-date spot-rate override (same foreign-per-AUD
    /// convention): when set it wins over the ATO monthly rate everywhere
    /// this trade's amounts convert to AUD — QC 18020 says an average rate is
    /// not appropriate for a one-off purchase or sale of a large capital
    /// asset (see `infra::fx::FxOverride`). `None` keeps the unchanged
    /// default (monthly rate first, `fx_rate` fallback). Rejected at write
    /// time unless positive and on a non-AUD trade (where it could apply).
    pub spot_fx_rate: Option<Decimal>,
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
    /// Provenance link from a cost-base-reset ESS vest Buy back to its
    /// `ess_statements` row (`None` for every other trade). Set only by
    /// `POST /ess_statements/:id/vest` (`entities::ess_vest`). A trade carrying
    /// it is the statement's vest: it is rejected by `PUT /trades` (delete and
    /// re-vest instead), and never deleted individually — `DELETE
    /// /ess_statements/:id` removes it (refused while it is drawn on by a Sell
    /// allocation or AMIT adjustment), and the statement is frozen against
    /// edits while it exists.
    pub ess_statement_id: Option<i64>,
    /// Provenance link from a worthless-shares recognise closing Sell back to
    /// its `WorthlessShares` corporate action (`None` for every other trade).
    /// Set only by `POST /corporate_actions/:id/recognise`
    /// (`entities::worthless`). The Sell carrying it is rejected by `PUT /sells`
    /// and `PUT`/`DELETE /trades`; `DELETE /sells` removes it and restores the
    /// holding; and the action cannot be edited or deleted while it exists.
    /// Unlike the rollover provenance columns it does **not** exclude the Sell
    /// from the realised-gains report — its nil proceeds recognise the capital
    /// loss (see `docs/ato/worthless-shares.md`).
    pub worthless_action_id: Option<i64>,
    /// Provenance link from an inherited-parcel Buy back to its `inheritances`
    /// row (`None` for every other trade). Set only by `PUT /inheritances/:id`
    /// (`entities::inheritance`): the Buy carries the inheritance's cost base
    /// and s 115-30 discount clock, so it is rejected by `PUT`/`DELETE
    /// /trades` — edit or delete the inheritance instead (refused while the
    /// parcel is drawn on by a Sell allocation or AMIT adjustment).
    pub inheritance_id: Option<i64>,
}

impl<'r> sqlx::FromRow<'r, sqlx::sqlite::SqliteRow> for Trade {
    fn from_row(row: &'r sqlx::sqlite::SqliteRow) -> Result<Self, sqlx::Error> {
        Ok(Trade {
            id: row.try_get("id")?,
            trade_type: row.try_get::<TradeType, _>("trade_type")?,
            date: row.try_get("date")?,
            settlement_date: row.try_get("settlement_date")?,
            listing_id: row.try_get("listing_id")?,
            average_price: row_dec(row, "average_price")?,
            quantity: row_dec(row, "quantity")?,
            currency: row.try_get("currency")?,
            brokerage: row_dec(row, "brokerage")?,
            gst_on_brokerage: row_dec(row, "gst_on_brokerage")?,
            brokerage_includes_gst: row.try_get("brokerage_includes_gst")?,
            brokerage_currency: row.try_get("brokerage_currency")?,
            fx_rate: row_dec(row, "fx_rate")?,
            spot_fx_rate: row_opt_dec(row, "spot_fx_rate")?,
            contract_note_ref: row.try_get("contract_note_ref")?,
            statement_total: row_opt_dec(row, "statement_total")?,
            residual_brought_forward: row_dec(row, "residual_brought_forward")?,
            residual_carried_forward: row_dec(row, "residual_carried_forward")?,
            residual_paid_out: row_dec(row, "residual_paid_out")?,
            rights_action_id: row.try_get("rights_action_id")?,
            buyback_action_id: row.try_get("buyback_action_id")?,
            scrip_action_id: row.try_get("scrip_action_id")?,
            demerger_action_id: row.try_get("demerger_action_id")?,
            deemed_acquisition_date: row.try_get("deemed_acquisition_date")?,
            holding_account_id: row.try_get("holding_account_id")?,
            transfer_id: row.try_get("transfer_id")?,
            ess_statement_id: row.try_get("ess_statement_id")?,
            worthless_action_id: row.try_get("worthless_action_id")?,
            inheritance_id: row.try_get("inheritance_id")?,
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
    /// Optional deliberate spot-rate override; see `Trade::spot_fx_rate`.
    #[serde(default)]
    pub spot_fx_rate: Option<Decimal>,
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

/// Why a supplied spot-rate override was rejected (both map to 422).
#[derive(Debug, PartialEq)]
pub(crate) enum SpotFxRateError {
    /// Zero or negative: the rate divides the amount (AUD = foreign / rate).
    NotPositive,
    /// On an AUD trade nothing converts, so the override could never apply —
    /// accepting it would silently ignore a deliberate entry.
    AudTrade,
}

/// Validate an optional deliberate spot-rate override against the trade's
/// currency. `None` — no override — is always fine (the unchanged default
/// conversion behaviour). Shared by the trade and Sell write paths so the two
/// can't drift.
pub(crate) fn validate_spot_fx_rate(
    currency: &str,
    spot_fx_rate: Option<Decimal>,
) -> Result<(), SpotFxRateError> {
    let Some(rate) = spot_fx_rate else {
        return Ok(());
    };
    if rate <= Decimal::ZERO {
        return Err(SpotFxRateError::NotPositive);
    }
    if currency.eq_ignore_ascii_case("AUD") {
        return Err(SpotFxRateError::AudTrade);
    }
    Ok(())
}

/// Human-readable body for a spot-rate 422 (shown by the web UI).
pub(crate) fn spot_fx_rate_detail(e: &SpotFxRateError) -> &'static str {
    match e {
        SpotFxRateError::NotPositive => "spot_fx_rate must be a positive foreign-per-AUD rate",
        SpotFxRateError::AudTrade => {
            "spot_fx_rate only applies to a non-AUD trade — an AUD amount never converts"
        }
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

/// The figures a recorded statement total is cross-checked against, gathered
/// for [`check_statement_total`]. Named fields keep the four adjacent `Decimal`
/// amounts from being transposed at the call site.
pub(crate) struct StatementTotalCheck<'a> {
    pub statement_total: Option<Decimal>,
    pub trade_type: TradeType,
    pub quantity: Decimal,
    pub average_price: Decimal,
    pub brokerage: Decimal,
    pub gst_on_brokerage: Decimal,
    pub currency: &'a str,
    pub brokerage_currency: &'a str,
}

/// Cross-check an optionally supplied statement total against the trade's
/// own figures: quantity × price + brokerage + GST for a Buy/DRP (amount
/// payable), quantity × price − brokerage − GST for a Sell (net proceeds
/// receivable — the statement nets costs out). Contract notes print the
/// consideration rounded to the cent, so the total also passes when it
/// equals the computed figure rounded to 2 dp (half away from zero, as
/// statements round). Comparison is numeric (`Decimal` equality ignores
/// trailing zeros: 1234.50 matches 1234.5). `None` means the statement
/// total wasn't recorded — nothing to check.
pub(crate) fn check_statement_total(c: StatementTotalCheck) -> Result<(), StatementTotalError> {
    let Some(total) = c.statement_total else {
        return Ok(());
    };
    if c.currency != c.brokerage_currency {
        return Err(StatementTotalError::CurrencyMismatch);
    }
    let costs = c.brokerage + c.gst_on_brokerage;
    let expected = match c.trade_type {
        TradeType::Buy | TradeType::DRP => c.quantity * c.average_price + costs,
        TradeType::Sell => c.quantity * c.average_price - costs,
    };
    let cent_rounded =
        expected.round_dp_with_strategy(2, rust_decimal::RoundingStrategy::MidpointAwayFromZero);
    if total != expected && total != cent_rounded {
        return Err(StatementTotalError::TotalMismatch { expected });
    }
    Ok(())
}

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
    check_statement_total(StatementTotalCheck {
        statement_total: trade.statement_total,
        trade_type: trade.trade_type,
        quantity: trade.quantity,
        average_price: trade.average_price,
        brokerage: trade.brokerage,
        gst_on_brokerage: trade.gst_on_brokerage,
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
    .bind(trade.average_price.to_string())
    .bind(trade.quantity.to_string())
    .bind(&trade.currency)
    .bind(trade.brokerage.to_string())
    .bind(trade.gst_on_brokerage.to_string())
    .bind(trade.brokerage_includes_gst)
    .bind(&trade.brokerage_currency)
    .bind(trade.fx_rate.to_string())
    .bind(trade.spot_fx_rate.map(|d| d.to_string()))
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

async fn list(State(pool): State<SqlitePool>) -> Result<Json<Vec<Trade>>, ApiError> {
    db_list(&pool).await.map(Json).map_err(ApiError::from)
}

async fn get_one(
    State(pool): State<SqlitePool>,
    Path(id): Path<i64>,
) -> Result<Json<Trade>, ApiError> {
    db_get(&pool, id)
        .await
        .map_err(ApiError::from)?
        .map(Json)
        .ok_or(ApiError::NotFound)
}

async fn upsert(
    State(pool): State<SqlitePool>,
    Path(id): Path<i64>,
    Json(body): Json<TradeBody>,
) -> Result<StatusCode, ApiError> {
    // Sells must be created via PUT /sells/{id} so they are persisted together
    // with a full set of parcel allocations (no uncovered Sell can exist).
    if body.trade_type == TradeType::Sell {
        return Err(ApiError::unprocessable(
            "a Sell must be created via PUT /sells/:id so it carries its parcel allocations",
        ));
    }
    // DRP trades are only ever created via POST /income/:id/reinvest, which
    // links the shares back to their funding distribution and threads the
    // residual carry-forward chain. A free-form DRP here would be an orphan
    // parcel (no income link, zero residuals) that could shadow that chain —
    // and editing a reinvest-created DRP through this endpoint would silently
    // zero its residual columns (the form doesn't carry them). Reject both.
    if body.trade_type == TradeType::DRP {
        return Err(ApiError::unprocessable(
            "a DRP trade is created via POST /income/:id/reinvest so it stays linked to its \
             distribution and residual chain",
        ));
    }
    let settlement_date = match body.settlement_date {
        Some(d) => d,
        None => auto_settlement_date(&pool, id, body.listing_id, body.date).await?,
    };
    // A GST-inclusive brokerage entry is split here, at the API boundary, so
    // the stored columns (and `Trade` itself) are always ex-GST + GST.
    let (brokerage, gst_on_brokerage) = resolve_brokerage(
        body.brokerage_includes_gst,
        body.brokerage,
        body.gst_on_brokerage,
    );
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
        spot_fx_rate: body.spot_fx_rate,
        contract_note_ref: body.contract_note_ref,
        statement_total: body.statement_total,
        residual_brought_forward: body.residual_brought_forward,
        residual_carried_forward: body.residual_carried_forward,
        residual_paid_out: body.residual_paid_out,
        rights_action_id: None,
        buyback_action_id: None,
        scrip_action_id: None,
        demerger_action_id: None,
        worthless_action_id: None,
        deemed_acquisition_date: None,
        holding_account_id: body.holding_account_id,
        transfer_id: None,
        ess_statement_id: None,
        inheritance_id: None,
    };
    db_upsert(&pool, &trade).await?;
    Ok(StatusCode::NO_CONTENT)
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
) -> Result<StatusCode, ApiError> {
    match db_delete(&pool, id).await? {
        DeleteOutcome::Deleted => Ok(StatusCode::NO_CONTENT),
        DeleteOutcome::NotFound => Err(ApiError::not_found("no trade with that id")),
        DeleteOutcome::Referenced => Err(ApiError::unprocessable(
            "this trade is referenced by a sale allocation, AMIT adjustment, reinvestment, or \
             a scrip-for-scrip/demerger group — remove those first (e.g. delete the Sell)",
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{self, dec, test_pool, ymd};
    use axum::{body::Body, http::Request};
    use http_body_util::BodyExt;
    use rust_decimal::Decimal;
    use std::collections::HashSet;
    use tower::ServiceExt;

    async fn insert_test_listing(pool: &SqlitePool) {
        test_support::listing(1)
            .ticker("VAS")
            .name("Vanguard Australian Shares ETF")
            .insert(pool)
            .await;
    }

    fn buy_trade() -> Trade {
        test_support::buy(1, 1)
            .date(ymd(2024, 1, 15))
            .qty(Decimal::from(10))
            .price(Decimal::from(100))
            .brokerage(dec("9.95"))
            .gst_on_brokerage(dec("0.995"))
            .with(|t| t.contract_note_ref = Some("CN001".to_string()))
            .build()
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
                spot_fx_rate: None,
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
        test_support::amma(1, 1)
            .units(qty)
            .cost_base_adjustment(dec("0.05"))
            .insert(pool)
            .await;
        test_support::amit_adjustment(pool, 1, 1, trade_id, qty).await;
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
        assert_eq!(
            got.settlement_date,
            NaiveDate::from_ymd_opt(2024, 1, 17).unwrap()
        );
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
        assert_fk_error(
            db_upsert(&pool, &bad_currency).await.unwrap_err(),
            "currency",
        );

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
        let trade = test_support::sell(2, 1)
            .date(ymd(2024, 6, 1))
            .qty(Decimal::from(5))
            .price(Decimal::from(120))
            .brokerage(dec("9.95"))
            .gst_on_brokerage(dec("0.995"))
            .build();
        db_upsert(&pool, &trade).await.unwrap();
        let got = db_get(&pool, 2).await.unwrap().unwrap();
        assert_eq!(got.trade_type, TradeType::Sell);
        assert_eq!(got.quantity, Decimal::from(5));
    }

    #[tokio::test]
    async fn db_drp_trade_insert_and_retrieve() {
        let pool = test_pool().await;
        insert_test_listing(&pool).await;
        let trade = test_support::drp(3, 1)
            .date(ymd(2024, 3, 15))
            .settlement(ymd(2024, 3, 15))
            .qty(Decimal::from(2))
            .price(Decimal::from(95))
            .build();
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
        assert_eq!(
            got.residual_brought_forward,
            "1.234567890".parse::<Decimal>().unwrap()
        );
        assert_eq!(
            got.residual_carried_forward,
            "0.987654321".parse::<Decimal>().unwrap()
        );
        assert_eq!(
            got.residual_paid_out,
            "2.500000001".parse::<Decimal>().unwrap()
        );
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

        assert_eq!(
            db_delete(&pool, 1).await.unwrap(),
            DeleteOutcome::Referenced
        );
        assert!(
            db_get(&pool, 1).await.unwrap().is_some(),
            "consumed buy must remain"
        );
    }

    #[tokio::test]
    async fn db_delete_buy_covered_by_amit_adjustment_is_refused() {
        let pool = test_pool().await;
        insert_test_listing(&pool).await;
        db_upsert(&pool, &buy_trade()).await.unwrap();
        insert_amit_adjustment_covering(&pool, 1, Decimal::from(10)).await;

        assert_eq!(
            db_delete(&pool, 1).await.unwrap(),
            DeleteOutcome::Referenced
        );
        assert!(
            db_get(&pool, 1).await.unwrap().is_some(),
            "covered buy must remain"
        );
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

        assert_eq!(
            db_delete(&pool, 1).await.unwrap(),
            DeleteOutcome::Referenced
        );
        assert!(
            db_get(&pool, 1).await.unwrap().is_some(),
            "reinvestment trade must remain"
        );
    }

    #[tokio::test]
    async fn db_upsert_over_reinvestment_trade_is_refused() {
        // A Buy body targeting a reinvest-created DRP would re-type it and
        // zero its residual chain while the income row keeps pointing at it —
        // the link lives on income.reinvestment_trade_id, which the
        // provenance-column check can't see, so it needs its own guard.
        let pool = test_pool().await;
        insert_test_listing(&pool).await;
        let mut drp = buy_trade();
        drp.trade_type = TradeType::DRP;
        drp.residual_carried_forward = dec("1.23");
        db_upsert(&pool, &drp).await.unwrap();
        sqlx::query(
            "INSERT INTO income (id, listing_id, date_paid, reinvestment_trade_id) \
             VALUES (1, 1, '2024-03-15', 1)",
        )
        .execute(&pool)
        .await
        .unwrap();

        let err = db_upsert(&pool, &buy_trade()).await.unwrap_err();
        assert!(
            matches!(err, UpsertError::ReinvestmentTrade),
            "expected ReinvestmentTrade, got: {err:?}"
        );
        let kept = db_get(&pool, 1).await.unwrap().unwrap();
        assert_eq!(kept.trade_type, TradeType::DRP, "trade must stay a DRP");
        assert_eq!(
            kept.residual_carried_forward,
            dec("1.23"),
            "residual chain must be untouched"
        );
    }

    #[tokio::test]
    async fn api_put_buy_body_over_reinvestment_drp_returns_422() {
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
        assert_eq!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY);
        let trade_type: String = sqlx::query_scalar("SELECT trade_type FROM trades WHERE id = 1")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(trade_type, "DRP", "the reinvestment DRP must be untouched");
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
        assert_eq!(
            db_get(&pool, 1).await.unwrap().unwrap().quantity,
            Decimal::from(10)
        );

        // …but shrinking exactly to the allocated quantity is fine.
        let mut exact = buy_trade();
        exact.quantity = Decimal::from(5);
        db_upsert(&pool, &exact).await.unwrap();
        assert_eq!(
            db_get(&pool, 1).await.unwrap().unwrap().quantity,
            Decimal::from(5)
        );
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
        assert_eq!(
            db_get(&pool, 1).await.unwrap().unwrap().quantity,
            Decimal::from(5)
        );
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
        assert_eq!(
            db_get(&pool, 1).await.unwrap().unwrap().quantity,
            Decimal::from(10)
        );

        // …but shrinking exactly to the covered quantity is fine.
        let mut exact = buy_trade();
        exact.quantity = Decimal::from(8);
        db_upsert(&pool, &exact).await.unwrap();
        assert_eq!(
            db_get(&pool, 1).await.unwrap().unwrap().quantity,
            Decimal::from(8)
        );
    }

    /// A Buy's listing is frozen while Sell allocations draw on the parcel:
    /// changing it would silently re-associate those allocations (and their
    /// CGT costing) to the new listing.
    #[tokio::test]
    async fn db_listing_change_on_allocated_parcel_is_refused() {
        let pool = test_pool().await;
        insert_test_listing(&pool).await;
        test_support::listing(2).ticker("VGS").insert(&pool).await;
        db_upsert(&pool, &buy_trade()).await.unwrap(); // listing 1
        insert_sell_consuming(&pool, 2, 1, Decimal::from(5)).await;

        let mut moved = buy_trade();
        moved.listing_id = 2;
        let err = db_upsert(&pool, &moved).await.unwrap_err();
        assert!(
            matches!(err, UpsertError::ListingChangeReferenced),
            "expected ListingChangeReferenced, got: {err:?}"
        );
        assert_eq!(db_get(&pool, 1).await.unwrap().unwrap().listing_id, 1);
    }

    /// The same freeze applies while an AMIT adjustment covers the parcel —
    /// and lifts once nothing references it.
    #[tokio::test]
    async fn db_listing_change_under_amit_adjustment_is_refused_until_unlinked() {
        let pool = test_pool().await;
        insert_test_listing(&pool).await;
        test_support::listing(2).ticker("VGS").insert(&pool).await;
        db_upsert(&pool, &buy_trade()).await.unwrap(); // listing 1
        insert_amit_adjustment_covering(&pool, 1, Decimal::from(8)).await;

        let mut moved = buy_trade();
        moved.listing_id = 2;
        let err = db_upsert(&pool, &moved).await.unwrap_err();
        assert!(
            matches!(err, UpsertError::ListingChangeReferenced),
            "expected ListingChangeReferenced, got: {err:?}"
        );
        assert_eq!(db_get(&pool, 1).await.unwrap().unwrap().listing_id, 1);

        // With the adjustment removed the listing edits freely again.
        sqlx::query("DELETE FROM amit_adjustments WHERE trade_id = 1")
            .execute(&pool)
            .await
            .unwrap();
        db_upsert(&pool, &moved).await.unwrap();
        assert_eq!(db_get(&pool, 1).await.unwrap().unwrap().listing_id, 2);
    }

    #[tokio::test]
    async fn db_unconsumed_buy_still_edits_and_deletes_freely() {
        let pool = test_pool().await;
        insert_test_listing(&pool).await;
        db_upsert(&pool, &buy_trade()).await.unwrap(); // qty 10

        let mut shrunk = buy_trade();
        shrunk.quantity = Decimal::ONE;
        db_upsert(&pool, &shrunk).await.unwrap();
        assert_eq!(
            db_get(&pool, 1).await.unwrap().unwrap().quantity,
            Decimal::ONE
        );

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
        assert_eq!(
            trade.settlement_date,
            NaiveDate::from_ymd_opt(2024, 1, 17).unwrap()
        );
    }

    /// An exchange-less (Crypto) listing settles same-day: the auto-populated
    /// settlement date is the trade date itself — a Friday stays a Friday (no
    /// T+n, no business-day skipping) — and no coverage warning fires (there
    /// is no holiday calendar to be outside of).
    #[tracing_test::traced_test]
    #[tokio::test]
    async fn api_settlement_date_same_day_for_crypto() {
        let pool = test_pool().await;
        test_support::listing(1)
            .crypto()
            .ticker("BTC")
            .name("Bitcoin")
            .insert(&pool)
            .await;
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
        assert_eq!(
            trade.settlement_date,
            NaiveDate::from_ymd_opt(2030, 6, 7).unwrap()
        );
        assert!(!logs_contain(
            "settlement window outside seeded exchange-holiday coverage"
        ));
    }

    #[tracing_test::traced_test]
    #[tokio::test]
    async fn api_settlement_beyond_holiday_coverage_logs_warning() {
        let pool = test_pool().await;
        insert_test_listing(&pool).await;
        // XASX holidays are seeded 2019–2027 only: a 2030 trade's settlement is
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
        assert!(logs_contain(
            "settlement window outside seeded exchange-holiday coverage"
        ));
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
        assert!(!logs_contain(
            "settlement window outside seeded exchange-holiday coverage"
        ));
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
        assert_eq!(
            trade.settlement_date,
            NaiveDate::from_ymd_opt(2024, 12, 30).unwrap()
        );
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
        assert_eq!(
            trade.settlement_date,
            NaiveDate::from_ymd_opt(2024, 1, 23).unwrap()
        );
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
    async fn api_put_drp_trade_is_rejected() {
        // DRP trades are created only via POST /income/{id}/reinvest (which links
        // them to their distribution and threads the residual chain); the generic
        // trade endpoint rejects a free-form DRP, and nothing is persisted.
        let pool = test_pool().await;
        insert_test_listing(&pool).await;
        let body = serde_json::json!({
            "trade_type": "DRP",
            "date": "2024-03-15",
            "listing_id": 1,
            "average_price": 95.0,
            "quantity": 2.0,
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
        assert_eq!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY);
        let n: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM trades")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(n, 0, "rejected DRP must not be persisted");
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
        assert_eq!(
            trade.settlement_date,
            NaiveDate::from_ymd_opt(2024, 1, 20).unwrap()
        );
    }

    #[tokio::test]
    async fn api_list_returns_ok() {
        let pool = test_pool().await;
        insert_test_listing(&pool).await;
        db_upsert(&pool, &buy_trade()).await.unwrap();
        let resp = router()
            .with_state(pool)
            .oneshot(
                Request::builder()
                    .uri("/trades")
                    .body(Body::empty())
                    .unwrap(),
            )
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
            .oneshot(
                Request::builder()
                    .uri("/trades/1")
                    .body(Body::empty())
                    .unwrap(),
            )
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
            .oneshot(
                Request::builder()
                    .uri("/trades/999")
                    .body(Body::empty())
                    .unwrap(),
            )
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
        assert!(
            db_get(&pool, 1).await.unwrap().is_some(),
            "consumed buy must remain"
        );
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
        assert_eq!(
            db_get(&pool, 1).await.unwrap().unwrap().quantity,
            Decimal::from(10)
        );
    }

    #[tokio::test]
    async fn api_listing_change_on_consumed_parcel_returns_422_with_reason() {
        let pool = test_pool().await;
        insert_test_listing(&pool).await;
        test_support::listing(2).ticker("VGS").insert(&pool).await;
        db_upsert(&pool, &buy_trade()).await.unwrap(); // listing 1
        insert_sell_consuming(&pool, 2, 1, Decimal::from(5)).await;

        let body = serde_json::json!({
            "trade_type": "Buy",
            "date": "2024-01-15",
            "settlement_date": "2024-01-17",
            "listing_id": 2,
            "average_price": "100",
            "quantity": "10",
            "currency": "AUD",
            "brokerage": "9.95",
            "gst_on_brokerage": "0.995",
            "brokerage_currency": "AUD",
            "fx_rate": "1"
        });
        let (status, detail) = put_trade_json(&pool, 1, body).await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
        assert!(
            detail.contains("listing cannot be changed"),
            "detail: {detail}"
        );
        assert_eq!(db_get(&pool, 1).await.unwrap().unwrap().listing_id, 1);
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
            .oneshot(
                Request::builder()
                    .uri("/trades/1")
                    .body(Body::empty())
                    .unwrap(),
            )
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
        assert!(
            t.brokerage_includes_gst,
            "flag must round-trip for the entry form"
        );

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
        assert_eq!(
            db_get(&pool, 1).await.unwrap().unwrap().statement_total,
            Some(d("1009.95"))
        );

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
        assert!(
            detail.contains("1009.95"),
            "detail must carry the computed figure: {detail}"
        );
        assert!(
            db_get(&pool, 2).await.unwrap().is_none(),
            "nothing persisted"
        );
    }

    /// Contract notes print the consideration rounded to the cent, so a
    /// total equal to the computed figure cent-rounded (half away from
    /// zero) passes too. The figures are the three archive contract notes
    /// the exact comparison rejected (live trades 19, 16, 21):
    /// 1,302 × 37.585914 + 8.64 + 0.86 = 48,946.360028 → note 48,946.36;
    /// 562 × 73.259875 + 8.64 + 0.86 = 41,181.54975 → note 41,181.55;
    /// 0.02413796 × 3,983.77 + 3.84 = 100.000080… → note 100.00.
    /// A mismatch at the cent itself still rejects with the computed
    /// (unrounded) figure in the detail, and an exact .5-mil residue
    /// rounds away from zero, not to even.
    #[tokio::test]
    async fn api_statement_total_accepts_cent_rounded_contract_note_totals() {
        let pool = test_pool().await;
        insert_test_listing(&pool).await;
        let buy = |qty: &str, price: &str, brokerage: &str, gst: &str, total: &str| {
            serde_json::json!({
                "trade_type": "Buy",
                "date": "2024-03-01",
                "listing_id": 1,
                "average_price": price,
                "quantity": qty,
                "currency": "AUD",
                "brokerage": brokerage,
                "gst_on_brokerage": gst,
                "brokerage_currency": "AUD",
                "fx_rate": "1",
                "statement_total": total
            })
        };

        // Trade 19: HNDQ 1 Mar 2024 contract note 1404967.
        let hndq = buy("1302", "37.585914", "8.64", "0.86", "48946.36");
        let (status, _) = put_trade_json(&pool, 1, hndq).await;
        assert_eq!(status, StatusCode::NO_CONTENT);
        // Trade 16: VDHG 8 Apr 2026 contract note 4518597 (…54975 → .55).
        let vdhg = buy("562", "73.259875", "8.64", "0.86", "41181.55");
        let (status, _) = put_trade_json(&pool, 2, vdhg).await;
        assert_eq!(status, StatusCode::NO_CONTENT);
        // Trade 21: ETH 22 Sep 2021 card purchase (…0080… → 100.00).
        let eth = buy("0.02413796", "3983.77", "3.84", "0", "100.00");
        let (status, _) = put_trade_json(&pool, 3, eth).await;
        assert_eq!(status, StatusCode::NO_CONTENT);

        // An exact midpoint rounds half away from zero (100.005 → 100.01),
        // not banker's-to-even (100.00).
        let mid = buy("1", "100.005", "0", "0", "100.01");
        let (status, _) = put_trade_json(&pool, 4, mid).await;
        assert_eq!(status, StatusCode::NO_CONTENT);
        let even = buy("1", "100.005", "0", "0", "100.00");
        let (status, _) = put_trade_json(&pool, 5, even).await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);

        // Off by a whole cent still rejects, computed figure in the body.
        let wrong = buy("1302", "37.585914", "8.64", "0.86", "48946.37");
        let (status, detail) = put_trade_json(&pool, 6, wrong).await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
        assert!(
            detail.contains("48946.360028"),
            "detail must carry the computed figure: {detail}"
        );
        assert!(
            db_get(&pool, 6).await.unwrap().is_none(),
            "nothing persisted"
        );
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
        assert!(
            detail.contains("currencies"),
            "detail must explain the rejection: {detail}"
        );
        assert!(
            db_get(&pool, 1).await.unwrap().is_none(),
            "nothing persisted"
        );
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

    // Spot-rate override (QC 18020): write-time validation and round-trip.

    #[tokio::test]
    async fn db_spot_fx_rate_round_trips_with_precision() {
        let pool = test_pool().await;
        insert_test_listing(&pool).await;
        test_support::buy(1, 1)
            .currency("USD")
            .spot_fx_rate("0.643215987".parse().unwrap())
            .insert(&pool)
            .await;
        let got = db_get(&pool, 1).await.unwrap().unwrap();
        assert_eq!(
            got.spot_fx_rate,
            Some("0.643215987".parse::<Decimal>().unwrap())
        );
        // Absent an override the column stays NULL — the unchanged default.
        test_support::buy(2, 1).currency("USD").insert(&pool).await;
        assert_eq!(db_get(&pool, 2).await.unwrap().unwrap().spot_fx_rate, None);
    }

    #[tokio::test]
    async fn db_spot_fx_rate_on_aud_trade_is_refused() {
        // An AUD amount never converts, so a spot override there could only
        // be a data-entry mistake silently ignored — rejected instead.
        let pool = test_pool().await;
        insert_test_listing(&pool).await;
        let trade = test_support::buy(1, 1)
            .spot_fx_rate("0.65".parse().unwrap())
            .build();
        let err = db_upsert(&pool, &trade).await.unwrap_err();
        assert!(
            matches!(err, UpsertError::SpotFxRate(SpotFxRateError::AudTrade)),
            "expected SpotFxRate(AudTrade), got: {err:?}"
        );
        assert!(db_get(&pool, 1).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn db_non_positive_spot_fx_rate_is_refused() {
        // The rate divides the amount (AUD = foreign / rate): zero or
        // negative can never be a real exchange rate.
        let pool = test_pool().await;
        insert_test_listing(&pool).await;
        for bad in ["0", "-0.5"] {
            let trade = test_support::buy(1, 1)
                .currency("USD")
                .spot_fx_rate(bad.parse().unwrap())
                .build();
            let err = db_upsert(&pool, &trade).await.unwrap_err();
            assert!(
                matches!(err, UpsertError::SpotFxRate(SpotFxRateError::NotPositive)),
                "expected SpotFxRate(NotPositive) for {bad}, got: {err:?}"
            );
        }
    }

    #[tokio::test]
    async fn api_put_trade_with_spot_fx_rate_persists_and_aud_is_422() {
        let pool = test_pool().await;
        insert_test_listing(&pool).await;
        let put = |pool: SqlitePool, body: serde_json::Value| async move {
            router()
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
                .unwrap()
        };
        // A USD trade with a deliberate spot rate persists it.
        let resp = put(
            pool.clone(),
            serde_json::json!({
                "trade_type": "Buy", "date": "2024-01-15", "listing_id": 1,
                "average_price": "100", "quantity": "10", "currency": "USD",
                "brokerage": "0", "brokerage_currency": "USD",
                "fx_rate": "0.70", "spot_fx_rate": "0.6543",
            }),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::NO_CONTENT);
        let got = db_get(&pool, 1).await.unwrap().unwrap();
        assert_eq!(got.spot_fx_rate, Some("0.6543".parse().unwrap()));

        // The same entry on an AUD trade is rejected with the reason.
        let resp = put(
            pool.clone(),
            serde_json::json!({
                "trade_type": "Buy", "date": "2024-01-15", "listing_id": 1,
                "average_price": "100", "quantity": "10", "currency": "AUD",
                "brokerage": "0", "brokerage_currency": "AUD",
                "fx_rate": "1", "spot_fx_rate": "0.6543",
            }),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY);
        let detail = String::from_utf8(
            resp.into_body()
                .collect()
                .await
                .unwrap()
                .to_bytes()
                .to_vec(),
        )
        .unwrap();
        assert!(detail.contains("non-AUD"), "detail: {detail}");
    }
}
