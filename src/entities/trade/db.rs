//! Persistence for trades: the `db_*` functions and the write-time
//! invariants they enforce in-transaction (dependant-capacity re-checks,
//! provenance immutability guards), plus the delete guards.

use super::checks::{
    AmountsCheck, AmountsError, SpotFxRateError, StatementTotalCheck, StatementTotalError,
    amounts_detail, check_amounts, check_statement_total, spot_fx_rate_detail,
    statement_total_detail, validate_spot_fx_rate,
};
use super::model::Trade;
use crate::infra::db::write_tx;
use crate::infra::decimal::{Money, OptMoney, parse_dec};
use crate::infra::http::{self, ApiError, CrudEntity};
use rust_decimal::Decimal;
use sqlx::{Row, SqlitePool};

/// The list and get *reads* are the plain single-table shape, so their SQL
/// comes from here; the routes still use hand-written handlers, because a
/// trade is presented through [`Trade::present`] and its delete has its own
/// [`DeleteOutcome`] guards.
impl CrudEntity for Trade {
    type Key = i64;
    const TABLE: &'static str = "trades";
    const COLUMNS: &'static str = "id, trade_type, date, settlement_date, settlement_date_source, \
         listing_id, average_price, quantity, \
         currency, brokerage, gst_on_brokerage, brokerage_includes_gst, brokerage_currency, \
         fx_rate, spot_fx_rate, contract_note_ref, statement_total, \
         residual_brought_forward, residual_carried_forward, residual_paid_out, rights_action_id, \
         buyback_action_id, scrip_action_id, demerger_action_id, deemed_acquisition_date, \
         holding_account_id, transfer_id, ess_statement_id, worthless_action_id, inheritance_id";
    const ORDER_BY: &'static str = "date, id";
    const NOUN: &'static str = "trade";
}

pub async fn db_list(pool: &SqlitePool) -> Result<Vec<Trade>, sqlx::Error> {
    http::crud_list(pool).await
}

pub async fn db_get(pool: &SqlitePool, id: i64) -> Result<Option<Trade>, sqlx::Error> {
    http::crud_get(pool, id).await
}

#[derive(thiserror::Error, Debug)]
pub enum UpsertError {
    #[error("trade write failed: {0}")]
    Db(#[from] sqlx::Error),
    /// The new quantity falls below the total already allocated out of this
    /// parcel by Sell allocations — accepting it would leave those allocations
    /// drawing on units the parcel no longer has.
    #[error("the new quantity is below what Sell allocations already draw from this parcel")]
    QuantityBelowAllocated,
    /// The new quantity falls below a linked AMIT adjustment's covered
    /// quantity, breaking that adjustment's `quantity <= trade.quantity`
    /// invariant (see `amit_adjustment::db_upsert`).
    #[error("the new quantity is below a linked AMIT adjustment's covered quantity")]
    QuantityBelowAmitAdjustment,
    /// The edit changes the trade's `listing_id` while Sell allocations or
    /// AMIT adjustments draw on this parcel: accepting it would silently
    /// re-associate those dependants to the new listing, costing them
    /// cross-listing in every CGT report. Remove the dependants first (e.g.
    /// delete the Sell via `DELETE /sells/:id`).
    #[error(
        "the listing cannot be changed while Sell allocations or AMIT adjustments reference this parcel"
    )]
    ListingChangeReferenced,
    /// The edit moves the parcel's `date` after a Sell that allocates from
    /// it: units can't be sold before they were acquired, so the pair would
    /// be impossible — and the discount clock would run backwards in every
    /// CGT report. This is the parcel side of `sell::SellError::PurchaseAfterSale`,
    /// which the Sell path already refuses.
    #[error("the date cannot move after a Sell that allocates from this parcel")]
    DateAfterAllocatedSale,
    /// The edit changes the trade's `holding_account_id` while Sell
    /// allocations or AMIT adjustments draw on this parcel: a sale only
    /// disposes of units its own account holds and a statement only adjusts
    /// its own account's parcels, so accepting it would leave the parcel
    /// reported as held in one account while its realised gain (or cost-base
    /// adjustment) stays costed against it in another. This is the parcel
    /// side of `sell::SellError::PurchaseInDifferentAccount` and
    /// `amit_adjustment::UpsertError::HoldingAccountMismatch`.
    #[error(
        "the holding account cannot be changed while Sell allocations or AMIT adjustments reference this parcel"
    )]
    AccountChangeReferenced,
    /// The existing trade is a rights exercise (`rights_action_id` set): its
    /// figures were validated against the rights issue's entitlement, which a
    /// free-form edit could exceed. Delete it and re-exercise instead (see
    /// `entities::rights_exercise`).
    #[error("this trade is a rights exercise and cannot be edited")]
    RightsExerciseTrade,
    /// The existing trade is a buy-back participation Sell
    /// (`buyback_action_id` set): its figures derive from the buy-back's
    /// terms and it carries a linked dividend-component income row. Delete it
    /// via `DELETE /sells` and re-participate instead (see
    /// `entities::buyback_participation`).
    #[error("this trade is a buy-back participation and cannot be edited")]
    BuyBackTrade,
    /// The existing trade belongs to a scrip-for-scrip exchange group
    /// (`scrip_action_id` set): its figures carry the rollover's cost base
    /// and deemed acquisition date, which a free-form edit would corrupt.
    /// Delete the group via `DELETE /sells` on the closing Sell and
    /// re-exchange instead (see `entities::scrip_exchange`).
    #[error("this trade belongs to a scrip-for-scrip exchange and cannot be edited")]
    ScripExchangeTrade,
    /// The existing trade belongs to a demerger group (`demerger_action_id`
    /// set): its figures carry the rollover's apportioned cost base and
    /// deemed acquisition date, which a free-form edit would corrupt. Delete
    /// the group via `DELETE /sells` on the closing Sell and re-demerge
    /// instead (see `entities::demerger`).
    #[error("this trade belongs to a demerger and cannot be edited")]
    DemergerTrade,
    /// The existing trade belongs to a holding-account transfer group
    /// (`transfer_id` set): its figures carry the moved parcel's cost base
    /// and deemed acquisition date, which a free-form edit would corrupt.
    /// Delete the transfer via `DELETE /transfers/:id` and re-transfer
    /// instead (see `entities::transfer`).
    #[error("this trade belongs to a holding-account transfer and cannot be edited")]
    TransferTrade,
    /// The existing trade is a cost-base-reset ESS vest Buy
    /// (`ess_statement_id` set): its figures derive from the ESS statement's
    /// quantity and taxing-point market value. Delete the statement (which
    /// removes the vest) and re-vest instead (see `entities::ess_vest`).
    #[error("this trade is an ESS vest and cannot be edited")]
    EssVestTrade,
    /// The existing trade is an inherited-parcel Buy (`inheritance_id` set):
    /// its figures carry the inheritance's cost base and s 115-30 discount
    /// clock, which a free-form edit would corrupt. Edit the inheritance
    /// (`PUT /inheritances/:id`) instead (see `entities::inheritance`).
    #[error("this trade is an inherited parcel and cannot be edited here")]
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
    #[error("this trade is a DRP reinvestment and cannot be edited")]
    ReinvestmentTrade,
    /// The existing trade is an original parcel anchoring a rights sale
    /// (`rights_sale_allocations.purchase_trade_id`): its date and quantity
    /// are what the sale's record-date anchoring caps were validated against,
    /// which a free-form edit could silently break. Delete the rights sale
    /// (`DELETE /rights_sales/:id`) and re-enter it after the edit (see
    /// `entities::rights_sale`).
    #[error("this parcel anchors a rights sale and cannot be edited")]
    RightsAnchorParcel,
    /// A supplied statement total failed the cross-check (see
    /// `check_statement_total`): it doesn't reconcile with the trade's own
    /// figures.
    #[error("the statement total cross-check failed: {0}")]
    StatementTotal(#[source] StatementTotalError),
    /// A supplied spot-rate override was rejected (see
    /// `validate_spot_fx_rate`): non-positive, or on an AUD trade where it
    /// could never apply.
    #[error("the spot FX rate override was rejected: {0}")]
    SpotFxRate(#[source] SpotFxRateError),
    /// The Buy/DRP's currency differs from that of a return-of-capital
    /// payment recorded on its listing that reaches it. The payment reduces
    /// each parcel's cost base in the parcel's own currency and amounts are
    /// never netted across currencies, so every cost-base report of the
    /// listing would fail loudly at read time
    /// (`corporate_action::RocEvent::per_unit_for`). This is the parcel side
    /// of `corporate_action::WriteError::PaymentCurrencyMismatch`, which
    /// refuses the same pair from the payment's side.
    #[error("this parcel's currency differs from a return of capital recorded on its listing")]
    PaymentCurrencyMismatch {
        payment_date: chrono::NaiveDate,
        payment_currency: String,
        parcel_currency: String,
    },
    /// The trade's currency is not its listing's. `average_price` is the
    /// price of that listed security, so the two are the same money — the
    /// rule [`ess_statement`](crate::entities::ess_statement) applies to a
    /// statement's per-share market value and the
    /// [DRP reinvest](crate::entities::drp_reinvestment) path to a
    /// distribution's cash. Mapped to 422.
    #[error("the trade is in {trade} but its listing is quoted in {listing}")]
    CurrencyNotListings { trade: String, listing: String },
    /// A degenerate core figure was rejected (see [`check_amounts`]):
    /// non-positive quantity or FX rate, negative price/brokerage/GST, a
    /// brokerage currency differing from the trade's, or a settlement before
    /// the trade date.
    #[error("a core trade figure was rejected: {0}")]
    Amounts(#[source] AmountsError),
    /// The trade is dated on a day its exchange did not trade — a weekend, or
    /// a seeded public holiday on the calendar that was in force then
    /// (SCENARIOS S-08). The trade date is the CGT event date, so it sets the
    /// 12-month discount clock, the financial year the gain falls in and the
    /// day the T+n settlement count starts from; a day the market was shut is
    /// a data-entry error by construction. The same calendar already refuses
    /// a closing price on such a day
    /// (`closing_price::validate_complete_trading_day`), and it is read here
    /// through the very same helper, resolved **as at the trade date** so a
    /// listing that has since changed exchange is judged on the market it
    /// actually traded on. Exchange-less (Crypto) listings trade every day and
    /// never reach this.
    ///
    /// That as-at resolution is deliberately *not* shared with the settlement
    /// calculation on the next line, which joins the listing's **live**
    /// `exchange_mic` (`exchange_holiday::exchange_holidays_for_listing`) — the
    /// documented live-exchange limitation, docs/API.md Known limitations. On a
    /// listing that changed exchange the two can therefore read different
    /// calendars for one trade. Correct on both counts: "was this security's
    /// market open that day" can only be answered by the calendar in force
    /// then, and the settlement half is a stated scope cut with its own test
    /// (`doc_checks::known_limitations_document_exchange_change_recomputation`).
    ///
    /// Deliberately **not** in [`check_amounts`]: that check is shared with
    /// `sell::upsert_sell_in_tx`, which every parcel-substituting operation
    /// writes its closing Sell through, and a corporate action's own date is
    /// legitimately not a trading day. The derived paths are covered instead
    /// by `reports::health`'s non-blocking `non_trading_day_trades` alert.
    #[error("the trade is dated on a non-trading day: {0}")]
    NonTradingDay(String),
}

/// The stored row an edit is checked against: the three fields whose *change*
/// is what dependants care about (`listing_id`, `date`, `holding_account_id`)
/// plus the provenance links that freeze the trade outright. Read once by
/// [`db_upsert`] and mapped by column name via `FromRow`.
#[derive(sqlx::FromRow)]
struct ExistingTrade {
    listing_id: i64,
    date: chrono::NaiveDate,
    holding_account_id: i64,
    rights_action_id: Option<i64>,
    buyback_action_id: Option<i64>,
    scrip_action_id: Option<i64>,
    demerger_action_id: Option<i64>,
    transfer_id: Option<i64>,
    ess_statement_id: Option<i64>,
    inheritance_id: Option<i64>,
}

/// Create or update a trade. Validated and written in one transaction
/// (symmetric with the Sell-side invariants in `sell::db_upsert_sell`): an
/// edit may not shrink a Buy/DRP's quantity below what its dependants rely on
/// — the quantity already allocated to Sells, or any linked AMIT adjustment's
/// covered quantity.
/// The listing's own currency when it differs from `currency`, else `None` —
/// the shared half of the rule both write paths enforce (SCENARIOS M-08).
///
/// A trade's `average_price` **is** the listed security's price, so the trade
/// and the listing are quoting the same money: a Buy of a US-quoted share
/// recorded in AUD divides an AUD price by a USD rate in every cost-base
/// report, and its closing prices — collected from the exchange in the
/// listing's currency — value it against a parcel costed in another. The same
/// argument [`ess_statement`] makes about `market_value_per_share` and the
/// [DRP reinvest][drp] path makes about a distribution's cash.
///
/// An unknown `listing_id` returns `None`: it falls through to the foreign-key
/// rejection, which names the missing row better than a currency message could.
///
/// [`ess_statement`]: crate::entities::ess_statement
/// [drp]: crate::entities::drp_reinvestment
pub(crate) async fn listing_currency_mismatch(
    conn: &mut sqlx::SqliteConnection,
    listing_id: i64,
    currency: &str,
) -> Result<Option<String>, sqlx::Error> {
    let listing: Option<String> = sqlx::query_scalar("SELECT currency FROM listings WHERE id = ?")
        .bind(listing_id)
        .fetch_optional(conn)
        .await?;
    Ok(listing.filter(|l| l != currency))
}

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
        currency: &trade.currency,
        brokerage_currency: &trade.brokerage_currency,
    })
    .map_err(UpsertError::Amounts)?;
    // The statement total (when recorded) must reconcile with the trade's own
    // figures — a mismatch is a data-entry error against the contract note,
    // caught before anything is written.
    check_statement_total(StatementTotalCheck {
        statement_total: trade.statement_total,
        amounts: trade.amounts(),
    })
    .map_err(UpsertError::StatementTotal)?;
    // A deliberate spot-rate override must be usable: positive, and on a
    // trade whose amounts actually convert.
    validate_spot_fx_rate(&trade.currency, trade.spot_fx_rate).map_err(UpsertError::SpotFxRate)?;

    let mut tx = write_tx(pool).await?;

    // A rights-exercise, buy-back participation, scrip-for-scrip exchange,
    // or demerger trade is immutable here: it was created against its
    // action's terms (entitlement cap / dividend-capital split / carried
    // cost base and deemed acquisition date), which an edit could silently
    // break. (The INSERT below never sets any provenance column, so a normal
    // trade can't become one either.)
    let existing: Option<ExistingTrade> = sqlx::query_as(
        "SELECT listing_id, date, holding_account_id, \
                rights_action_id, buyback_action_id, scrip_action_id, \
                demerger_action_id, transfer_id, ess_statement_id, inheritance_id \
         FROM trades WHERE id = ?",
    )
    .bind(trade.id)
    .fetch_optional(&mut *tx)
    .await?;
    if let Some(existing) = &existing {
        if existing.rights_action_id.is_some() {
            return Err(UpsertError::RightsExerciseTrade);
        }
        if existing.buyback_action_id.is_some() {
            return Err(UpsertError::BuyBackTrade);
        }
        if existing.scrip_action_id.is_some() {
            return Err(UpsertError::ScripExchangeTrade);
        }
        if existing.demerger_action_id.is_some() {
            return Err(UpsertError::DemergerTrade);
        }
        if existing.transfer_id.is_some() {
            return Err(UpsertError::TransferTrade);
        }
        if existing.ess_statement_id.is_some() {
            return Err(UpsertError::EssVestTrade);
        }
        if existing.inheritance_id.is_some() {
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
    if existing
        .as_ref()
        .is_some_and(|e| e.listing_id != trade.listing_id)
    {
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

    // The holding account is frozen on the same terms: a Sell allocation only
    // ever draws on a parcel its own account holds (`sell::db_upsert_sell`)
    // and an AMMA statement only adjusts its own account's parcels
    // (`amit_adjustment::db_upsert_on`), so moving the parcel out from under
    // them would leave it reported as held in one account while its realised
    // gain / cost-base adjustment stays costed against it in another — a state
    // neither of those write paths would accept.
    if existing
        .as_ref()
        .is_some_and(|e| e.holding_account_id != trade.holding_account_id)
    {
        let referenced: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM parcel_allocations WHERE purchase_trade_id = ?1) \
                 OR EXISTS(SELECT 1 FROM amit_adjustments WHERE trade_id = ?1)",
        )
        .bind(trade.id)
        .fetch_one(&mut *tx)
        .await?;
        if referenced {
            return Err(UpsertError::AccountChangeReferenced);
        }
    }

    // Moving the parcel's date later must not carry it past a Sell that
    // allocates from it: units can't be sold before they were acquired, and
    // the resulting pair would run the CGT discount clock backwards. The Sell
    // path refuses the same pair from its side
    // (`sell::SellError::PurchaseAfterSale`); this is the parcel side of it.
    // (Only a *later* date can break it, so an earlier one is left alone —
    // and a fresh insert has no allocations to break.)
    if existing.as_ref().is_some_and(|e| trade.date > e.date) {
        let earliest_sale: Option<chrono::NaiveDate> = sqlx::query_scalar(
            "SELECT MIN(s.date) FROM parcel_allocations pa \
             JOIN trades s ON s.id = pa.sale_trade_id \
             WHERE pa.purchase_trade_id = ?",
        )
        .bind(trade.id)
        .fetch_one(&mut *tx)
        .await?;
        if earliest_sale.is_some_and(|sale| trade.date > sale) {
            return Err(UpsertError::DateAfterAllocatedSale);
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
         (id, trade_type, date, settlement_date, settlement_date_source, listing_id, \
          average_price, quantity, \
          currency, brokerage, gst_on_brokerage, brokerage_includes_gst, brokerage_currency, \
          fx_rate, spot_fx_rate, contract_note_ref, statement_total, \
          residual_brought_forward, residual_carried_forward, residual_paid_out, \
          holding_account_id) \
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?) \
         ON CONFLICT(id) DO UPDATE SET \
             trade_type               = excluded.trade_type, \
             date                     = excluded.date, \
             settlement_date          = excluded.settlement_date, \
             settlement_date_source   = excluded.settlement_date_source, \
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
    .bind(trade.settlement_date_source)
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

    // The trade's currency must be the listing's: `average_price` is the price
    // of that listed security, so the two are one money (SCENARIOS M-08).
    // Checked *after* the write, like the return-of-capital rule below, so an
    // unrecognised currency code still meets its own foreign-key rejection
    // first — "no such currency" is the better answer to `ZZZ` than "not the
    // listing's".
    if let Some(listing) =
        listing_currency_mismatch(&mut tx, trade.listing_id, &trade.currency).await?
    {
        return Err(UpsertError::CurrencyNotListings {
            trade: trade.currency.clone(),
            listing,
        });
    }

    // The trade date must be a day the listing's own market actually traded
    // (SCENARIOS S-08). Read on this transaction — the calendar is a DB read,
    // so it can't live in the pure `check_amounts` — and, like the currency
    // rule above, after the write so an unknown `listing_id` still meets its
    // foreign-key rejection first.
    if let Some(shut) =
        crate::entities::closing_price::db_non_trading_day(&mut tx, trade.listing_id, trade.date)
            .await?
    {
        return Err(UpsertError::NonTradingDay(shut.describe(trade.date)));
    }

    // A return of capital on this listing reduces the parcel's cost base in
    // the *parcel's* own currency, so a Buy/DRP recorded in another one is a
    // state the cost-base reports refuse to compute over. This is the parcel
    // side of the same invariant `corporate_action::db_upsert` enforces from
    // the payment's side; like it, the check runs over the written state
    // inside the write's own transaction. A Sell holds no cost base, so only
    // the parcel types can introduce the pair.
    if trade.trade_type != super::model::TradeType::Sell
        && let Some(conflict) = crate::entities::corporate_action::db_payment_currency_conflict(
            &mut *tx,
            trade.listing_id,
        )
        .await?
    {
        return Err(UpsertError::PaymentCurrencyMismatch {
            payment_date: conflict.payment_date,
            payment_currency: conflict.payment_currency,
            parcel_currency: conflict.parcel_currency,
        });
    }

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
    let mut tx = write_tx(pool).await?;

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
            UpsertError::NonTradingDay(what) => ApiError::unprocessable(format!(
                "the trade is dated on a day its exchange did not trade — {what}. The trade date \
                 is the CGT event date, so it sets the discount clock, the financial year and the \
                 settlement count; enter the day the trade actually executed"
            )),
            UpsertError::CurrencyNotListings { trade, listing } => {
                ApiError::unprocessable(format!(
                    "this trade is recorded in {trade} but its listing is quoted in {listing} — \
                     the price you paid and the listed price are the same money, so enter the \
                     trade in {listing} (a contract note in another currency is converted before \
                     entry, or the wrong listing was picked)"
                ))
            }
            // Names the payment and both currencies, so the disagreeing row is
            // findable without opening the listing's corporate actions.
            UpsertError::PaymentCurrencyMismatch {
                payment_date,
                payment_currency,
                parcel_currency,
            } => ApiError::Unprocessable(format!(
                "this parcel is held in {parcel_currency} while the return of capital dated \
                 {payment_date} on its listing is recorded in {payment_currency} — a payment \
                 reduces each parcel's cost base in the parcel's own currency, and amounts are \
                 never netted across currencies, so the two must agree"
            )),
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
            UpsertError::AccountChangeReferenced => ApiError::unprocessable(
                "the holding account cannot be changed while Sell allocations or AMIT \
                 adjustments reference this parcel — move it with a Transfer, or remove \
                 them first",
            ),
            UpsertError::DateAfterAllocatedSale => ApiError::unprocessable(
                "the date cannot move after a Sell that allocates from this parcel — units \
                 can't be sold before they were acquired",
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
