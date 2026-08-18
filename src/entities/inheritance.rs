//! Inherited share parcels — the beneficiary's entry path for a parcel
//! passing from a deceased estate (REQUIREMENTS 2026-06-10;
//! `docs/ato/inherited-assets-cost-base.md` QC 66053,
//! `docs/ato/inherited-assets-cgt-discount.md` QC 69713 / s 115-30).
//!
//! The transfer from the estate is not a CGT event. `PUT /inheritances/:id`
//! records the inheritance facts and creates the parcel in one transaction: a
//! provenance-linked Buy (`trades.inheritance_id`) dated the date of death,
//! carrying the whole cost base — the first element per the recorded QC 66053
//! rule plus any LPR expenditure — on the brokerage column with price 0 (the
//! rollover convention), so the parcel flows through every report and
//! write-time capacity check like any Buy. Settlement is the date of death:
//! an estate transmission is not market-settled.
//!
//! The 12-month discount clock follows s 115-30:
//! - [`CostBaseRule::DeceasedCostBase`] (the deceased acquired the asset on or
//!   after 20 September 1985): the first element is the deceased's cost base
//!   on the day they died, and the clock runs from the **deceased's
//!   acquisition date**, carried as the Buy's `deemed_acquisition_date`.
//! - [`CostBaseRule::MarketValueAtDeath`] (a pre-CGT asset in the deceased's
//!   hands): the first element is the user-supplied market value on the day
//!   the deceased died, and the clock runs from the **date of death** — the
//!   Buy's own date, no deemed date.
//!
//! The deemed acquisition date also picks the AUD translation month of a
//! non-AUD cost base (the standard `ParcelRow::acquired()` rule), mirroring
//! the rollover treatment: a carried deceased's cost base translates at the
//! deceased's acquisition month, a market-value-at-death figure at the death
//! month. LPR expenditure is folded into that same single-rate conversion,
//! which is why it is accepted **only on an AUD inheritance**
//! ([`UpsertError::LprExpenditureOnForeignParcel`]): the LPR incurs it after
//! the death, so on a foreign parcel it would translate at a month that can
//! predate the expense by decades (SCENARIOS K-04). Its incurral date is
//! provenance only — nothing reads it.
//! `fx_rate` is the *fallback* for that conversion — used only when no ATO
//! rate exists for the month — so a non-AUD inheritance that leaves it at its
//! default 1 with no rate imported for the month is refused rather than costed
//! at parity ([`UpsertError::MissingFxRate`]). The month is the deceased's
//! acquisition month under `DeceasedCostBase`, which is routinely decades
//! before the RBA import reaches, so the rate is usually the taxpayer's to
//! state.
//!
//! The linked Buy is immutable individually (`PUT`/`DELETE /trades` → 422):
//! editing and deleting go through the inheritance, and both are refused
//! while the parcel is drawn on by a Sell allocation or AMIT adjustment.
//!
//! # The trade write-time checks, and where each is satisfied
//!
//! The Buy is written with a raw `INSERT INTO trades`, not through
//! `trade::db_upsert`, so `trade::check_amounts` never runs over it (nor could
//! it: the trade write paths refuse an inheritance-linked Buy outright). That
//! is deliberate, and this is the list it rests on — every one of that check's
//! rejections is either impossible here or refused by this entity's own
//! [`validate`] (SCENARIOS K-01, K-02, K-04):
//!
//! - `QuantityNotPositive` — [`UpsertError::QuantityNotPositive`], the same
//!   rule on the inherited unit count the Buy's quantity is bound from.
//! - `PriceNegative` — `average_price` is written as the literal `'0'`: an
//!   inherited parcel has no per-unit price, only a carried cost base.
//! - `BrokerageNegative` / `GstNegative` — the GST is the literal `'0'` (an
//!   estate transmission is not a brokered trade), and the brokerage column
//!   carries the cost base, which [`UpsertError::NegativeAmount`] refuses to
//!   let either component be negative.
//! - `BrokerageCurrencyMismatch` — `brokerage_currency` is bound from the same
//!   `currency` value as the trade's own, in one statement.
//! - `FxRateNotPositive` — [`UpsertError::FxRateNotPositive`], the same rule.
//! - `SettlementBeforeTrade` — `settlement_date` is bound to the trade date
//!   itself (an estate transmission is not market-settled).
//! - `PreCgtDate` — [`UpsertError::DeathPreCgt`], refused on the date of death,
//!   which is what the Buy is dated.
//!
//! `trade::db_upsert` also cross-checks the written parcel against the
//! return-of-capital payments on its listing, since a payment reduces a cost
//! base in the parcel's own currency; that check runs here too, over this
//! write's own transaction ([`UpsertError::PaymentCurrencyMismatch`]).
//!
//! A new check added to `trade::check_amounts` therefore needs a line here, and
//! either an argument that the inheritance satisfies it or a guard that makes
//! it so.

use crate::infra::decimal::Money;
use crate::infra::http::ApiError;
use axum::{
    Json, Router,
    extract::{Path, State},
    http::StatusCode,
    routing::get,
};
use chrono::NaiveDate;
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;

/// Which QC 66053 rule produced the inheritance's first-element cost base.
/// Serialized verbatim to JSON and to the CHECK-constrained TEXT
/// `cost_base_rule` column.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, sqlx::Type)]
pub enum CostBaseRule {
    /// The deceased acquired the asset on or after 20 September 1985: the
    /// beneficiary's first element is the deceased's cost base on the day
    /// they died, and the discount clock runs from the deceased's
    /// acquisition (s 115-30).
    DeceasedCostBase,
    /// A pre-CGT asset in the deceased's hands: the first element is the
    /// asset's market value on the day the deceased died (user-supplied),
    /// and the discount clock runs from the date of death (s 115-30).
    MarketValueAtDeath,
}

/// Start of CGT: an asset the deceased acquired before this date is pre-CGT,
/// so the [`CostBaseRule::MarketValueAtDeath`] rule applies and their
/// acquisition date is not recorded. A death before it makes the parcel
/// pre-CGT in the *beneficiary's* hands too (the s 115-30 clock deems
/// acquisition at the death), which is not modelled. Shared with the trade
/// write paths, which reject any pre-CGT-dated trade.
use super::trade::CGT_START;

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct Inheritance {
    pub id: i64,
    pub listing_id: i64,
    pub holding_account_id: i64,
    /// Units inherited, in date-of-death terms.
    #[sqlx(try_from = "Money")]
    pub quantity: Decimal,
    pub date_of_death: NaiveDate,
    pub cost_base_rule: CostBaseRule,
    /// The whole-parcel first-element cost base per the rule, in `currency`.
    #[sqlx(try_from = "Money")]
    pub cost_base: Decimal,
    /// LPR expenditure the beneficiary may include in the cost base
    /// (QC 66053 — e.g. conveyancing on the transfer, legal costs of proving
    /// the will). Added to the linked Buy's cost base.
    #[sqlx(try_from = "Money")]
    pub lpr_expenditure: Decimal,
    /// When the LPR incurred the expenditure (on or after the date of death);
    /// recorded with the figure, provenance only.
    pub lpr_expenditure_date: Option<NaiveDate>,
    /// The deceased's acquisition date — required with
    /// [`CostBaseRule::DeceasedCostBase`] (it starts the discount clock),
    /// absent with [`CostBaseRule::MarketValueAtDeath`].
    pub deceased_acquisition_date: Option<NaiveDate>,
    pub currency: String,
    /// Manual foreign-per-AUD fallback rate (same convention as
    /// `trades.fx_rate`). 1 for AUD.
    #[sqlx(try_from = "Money")]
    pub fx_rate: Decimal,
}

#[derive(Debug, Deserialize)]
pub struct InheritanceBody {
    pub listing_id: i64,
    /// Defaults to the seeded default holding account when omitted.
    #[serde(default = "crate::entities::holding_account::default_holding_account_id")]
    pub holding_account_id: i64,
    pub quantity: Decimal,
    pub date_of_death: NaiveDate,
    pub cost_base_rule: CostBaseRule,
    pub cost_base: Decimal,
    /// Absent/null means no LPR expenditure.
    #[serde(default)]
    pub lpr_expenditure: Option<Decimal>,
    #[serde(default)]
    pub lpr_expenditure_date: Option<NaiveDate>,
    #[serde(default)]
    pub deceased_acquisition_date: Option<NaiveDate>,
    #[serde(default = "default_currency")]
    pub currency: String,
    /// Absent/null means 1 (the AUD case).
    #[serde(default)]
    pub fx_rate: Option<Decimal>,
}

/// The row's stored columns, in `Inheritance`'s field order — the one SELECT
/// list every read of the table uses (`reports::health`'s duplicate check
/// included), so a new column is added in one place.
pub(crate) const COLUMNS: &str = "id, listing_id, holding_account_id, quantity, date_of_death, \
                                  cost_base_rule, cost_base, lpr_expenditure, \
                                  lpr_expenditure_date, deceased_acquisition_date, currency, \
                                  fx_rate";

fn default_currency() -> String {
    "AUD".to_string()
}

pub fn router() -> Router<SqlitePool> {
    Router::new().route("/inheritances", get(list)).route(
        "/inheritances/{id}",
        get(get_one).put(upsert).delete(delete),
    )
}

#[derive(thiserror::Error, Debug)]
pub enum UpsertError {
    #[error("inheritance write failed: {0}")]
    Db(#[from] sqlx::Error),
    /// The inherited unit count must be positive — there is no parcel
    /// otherwise.
    #[error("the inherited quantity must be greater than zero")]
    QuantityNotPositive,
    /// The cost base and LPR expenditure are amounts spent: negative values
    /// are data-entry errors.
    #[error("the cost base and LPR expenditure must not be negative")]
    NegativeAmount,
    /// `deceased_acquisition_date` must be present exactly when the rule is
    /// `DeceasedCostBase` (it starts the discount clock); a pre-CGT asset's
    /// clock runs from the date of death and the date is not recorded.
    #[error("the deceased's acquisition date belongs only with the DeceasedCostBase rule")]
    RuleAcquisitionDateMismatch,
    /// A `DeceasedCostBase` inheritance with the deceased's acquisition
    /// before 20 September 1985 — that is the pre-CGT case, which takes the
    /// `MarketValueAtDeath` rule instead.
    #[error("the deceased acquired the asset before 20 September 1985 (a pre-CGT asset)")]
    DeceasedAcquisitionPreCgt,
    /// A death before 20 September 1985: the beneficiary is treated as
    /// having owned the asset since (at latest) the death, so the parcel is
    /// pre-CGT in the beneficiary's own hands — outside CGT and not
    /// modelled.
    #[error("the date of death is before 20 September 1985, so the parcel is pre-CGT")]
    DeathPreCgt,
    /// The deceased cannot have acquired the asset after they died.
    #[error("the deceased's acquisition date cannot be after the date of death")]
    DeceasedAcquisitionAfterDeath,
    /// `lpr_expenditure_date` must be present exactly when a non-zero
    /// `lpr_expenditure` is recorded (the figure is dated when the LPR
    /// incurred it).
    #[error("a non-zero LPR expenditure needs the date the LPR incurred it, and vice versa")]
    LprExpenditureDateMismatch,
    /// LPR expenditure is incurred administering the estate — it cannot
    /// pre-date the death.
    #[error("the LPR expenditure date cannot be before the date of death")]
    LprExpenditureBeforeDeath,
    /// The linked Buy is drawn on by a Sell allocation or AMIT adjustment —
    /// editing the inheritance under it could invalidate those dependants.
    /// Remove them first.
    #[error("the inherited parcel is drawn on by a sale allocation or AMIT adjustment")]
    ParcelDrawnOn,
    /// Zero or negative fallback FX rate. The rate divides the amount
    /// (`AUD = foreign / rate`), so it can never be a real exchange rate —
    /// and a zero one is not merely wrong: `infra::fx::apply_rate` divides by
    /// it, so every cost-base report of the listing panics
    /// (`trade::checks::AmountsError::FxRateNotPositive`, the same rule).
    #[error("the fallback FX rate must be greater than zero")]
    FxRateNotPositive,
    /// A non-zero LPR expenditure on a non-AUD inheritance. The whole parcel
    /// converts to AUD at one rate — the (possibly deemed) acquisition
    /// month's — and the LPR incurs their expenditure *after* the death, so
    /// on a foreign parcel that rate can predate the expense by decades and
    /// the element would be translated at the wrong month entirely
    /// (SCENARIOS K-04). An LPR fee is an Australian estate-administration
    /// cost billed in AUD, so rather than report a figure known to be wrong,
    /// the pair is refused and documented as a limitation.
    #[error("LPR expenditure is only recordable on an AUD inheritance")]
    LprExpenditureOnForeignParcel,
    /// The inheritance's currency is not its listing's. The parcel is a
    /// holding of that listed security, priced by its exchange in the
    /// listing's currency, so the cost base and the market value it will be
    /// compared against must be the same money — and under
    /// [`CostBaseRule::MarketValueAtDeath`] the figure entered *is* a market
    /// value of that security. Mapped to 422. Same rule as the ESS statement
    /// and DRP reinvest paths.
    #[error("the inheritance is in {inheritance} but its listing is in {listing}")]
    CurrencyNotListings {
        inheritance: String,
        listing: String,
    },
    /// A non-AUD inheritance whose acquisition month has no imported ATO rate
    /// and that states no rate of its own — `fx_rate` still at its default 1.
    /// `fx_rate` is the *fallback* rate, applied exactly when no ATO rate
    /// exists for the month, so the default would become a real answer in
    /// precisely that case and cost the parcel at parity. Refused rather than
    /// answered wrongly (`ess_vest::VestError::MissingFxRate`, the same rule).
    #[error("no ATO FX rate for {currency} in {month} and the inheritance states none")]
    MissingFxRate { currency: String, month: String },
    /// The inherited parcel's currency differs from that of a
    /// return-of-capital payment on its listing that reaches it. The payment
    /// reduces each parcel's cost base in the parcel's own currency and
    /// amounts are never netted across currencies, so every cost-base report
    /// of the listing would fail loudly at read time. The inheritance side of
    /// `trade::UpsertError::PaymentCurrencyMismatch`, which refuses the same
    /// pair for an ordinary Buy.
    #[error("this parcel's currency differs from a return of capital recorded on its listing")]
    PaymentCurrencyMismatch {
        payment_date: NaiveDate,
        payment_currency: String,
        parcel_currency: String,
    },
}

impl From<UpsertError> for ApiError {
    fn from(e: UpsertError) -> Self {
        match e {
            UpsertError::QuantityNotPositive => {
                ApiError::unprocessable("the inherited quantity must be greater than zero")
            }
            UpsertError::NegativeAmount => {
                ApiError::unprocessable("the cost base and LPR expenditure must not be negative")
            }
            UpsertError::RuleAcquisitionDateMismatch => ApiError::unprocessable(
                "the deceased's acquisition date is required with the DeceasedCostBase rule \
                 (it starts the 12-month discount clock) and must be omitted with \
                 MarketValueAtDeath (a pre-CGT asset's clock runs from the date of death)",
            ),
            UpsertError::DeceasedAcquisitionPreCgt => ApiError::unprocessable(
                "the deceased acquired the asset before 20 September 1985 — that is a pre-CGT \
                 asset, so record it under the MarketValueAtDeath rule",
            ),
            UpsertError::DeceasedAcquisitionAfterDeath => ApiError::unprocessable(
                "the deceased's acquisition date cannot be after the date of death",
            ),
            UpsertError::DeathPreCgt => ApiError::unprocessable(
                "the date of death is before 20 September 1985 — the inherited parcel is \
                 pre-CGT in the beneficiary's own hands, which is outside CGT and not modelled",
            ),
            UpsertError::LprExpenditureDateMismatch => ApiError::unprocessable(
                "a non-zero LPR expenditure needs the date the LPR incurred it (and a date \
                 needs a non-zero expenditure)",
            ),
            UpsertError::LprExpenditureBeforeDeath => ApiError::unprocessable(
                "the LPR expenditure date cannot be before the date of death",
            ),
            UpsertError::ParcelDrawnOn => ApiError::unprocessable(
                "the inherited parcel is drawn on by a sale allocation or AMIT adjustment — \
                 remove those first",
            ),
            UpsertError::FxRateNotPositive => ApiError::unprocessable(
                "fx_rate must be a positive foreign-per-AUD rate (1 for an AUD inheritance)",
            ),
            UpsertError::LprExpenditureOnForeignParcel => ApiError::unprocessable(
                "LPR expenditure can only be recorded on an AUD inheritance — the whole parcel \
                 converts to AUD at one rate, its (deemed) acquisition month's, while the LPR \
                 incurs the expenditure after the death, so on a foreign parcel the figure would \
                 be translated at the wrong month (see Known limitations)",
            ),
            UpsertError::CurrencyNotListings {
                inheritance,
                listing,
            } => ApiError::Unprocessable(format!(
                "this inheritance is recorded in {inheritance} but its listing is quoted in \
                 {listing} — the parcel's cost base and the exchange's price for the same \
                 security are one money, so enter it in {listing} (an estate's figures in \
                 another currency are converted before entry, or the wrong listing was chosen)"
            )),
            UpsertError::MissingFxRate { currency, month } => ApiError::Unprocessable(format!(
                "this inheritance is in {currency} but no ATO/RBA rate has been imported for \
                 {currency} in {month} — the month the parcel's cost base converts at — and the \
                 inheritance states no fx_rate of its own — import that month's rates or record \
                 the rate to use; recording it without one would cost the parcel at parity \
                 (1 AUD per {currency})"
            )),
            // The same wording `PUT /trades` answers for the same pair: it
            // names the payment and both currencies, so the disagreeing row is
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
            UpsertError::Db(err) => err.into(),
        }
    }
}

pub async fn db_list(pool: &SqlitePool) -> Result<Vec<Inheritance>, sqlx::Error> {
    sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT {COLUMNS} FROM inheritances ORDER BY date_of_death, id"
    )))
    .fetch_all(pool)
    .await
}

pub async fn db_get(pool: &SqlitePool, id: i64) -> Result<Option<Inheritance>, sqlx::Error> {
    sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT {COLUMNS} FROM inheritances WHERE id = ?"
    )))
    .bind(id)
    .fetch_optional(pool)
    .await
}

/// The id of the inheritance's linked Buy (`trades.inheritance_id`), if the
/// inheritance has been recorded.
async fn linked_buy_id(
    tx: &mut sqlx::SqliteConnection,
    inheritance_id: i64,
) -> Result<Option<i64>, sqlx::Error> {
    sqlx::query_scalar("SELECT id FROM trades WHERE inheritance_id = ?")
        .bind(inheritance_id)
        .fetch_optional(tx)
        .await
}

/// Whether anything draws on the linked Buy: a Sell allocation consuming its
/// units or an AMIT adjustment covering them. While true, the inheritance is
/// frozen — an edit or delete could invalidate those dependants.
async fn buy_drawn_on(tx: &mut sqlx::SqliteConnection, buy_id: i64) -> Result<bool, sqlx::Error> {
    sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM parcel_allocations WHERE purchase_trade_id = ?1) \
             OR EXISTS(SELECT 1 FROM amit_adjustments WHERE trade_id = ?1)",
    )
    .bind(buy_id)
    .fetch_one(tx)
    .await
}

/// The month the parcel's cost base converts at — `ParcelRow::acquired()`'s
/// rule spelled out on the inheritance: the deceased's acquisition under
/// `DeceasedCostBase` (carried as the Buy's deemed date), else the death.
fn conversion_month(inh: &Inheritance) -> String {
    inh.deceased_acquisition_date
        .unwrap_or(inh.date_of_death)
        .format("%Y-%m")
        .to_string()
}

/// Refuse a non-AUD inheritance that has no rate to convert at. `fx_rate` is
/// the *fallback* (`infra::fx::pick_rate`), used exactly when the ATO rate for
/// the month is missing, and it defaults to 1 — so leaving it alone costs the
/// parcel at parity in precisely the case the fallback exists for. Worth its
/// own check here rather than at read time because the month in question is
/// the *deceased's* acquisition month, routinely decades before the RBA import
/// reaches (SCENARIOS K-01, K-04).
/// The inheritance's currency must be its listing's: the parcel is a holding
/// of that listed security, and the exchange prices it in the listing's
/// currency, so a cost base in another would be compared against a market
/// value in the first — mixing currencies in one calculation. Sharper still
/// under [`CostBaseRule::MarketValueAtDeath`], where the figure entered *is* a
/// market value of that security. The same rule `ess_statement::db_upsert` and
/// the DRP reinvest path apply (SCENARIOS K-01). An unknown `listing_id` falls
/// through to the foreign-key rejection.
async fn check_listing_currency(
    tx: &mut sqlx::SqliteConnection,
    inh: &Inheritance,
) -> Result<(), UpsertError> {
    let listing_currency: Option<String> =
        sqlx::query_scalar("SELECT currency FROM listings WHERE id = ?")
            .bind(inh.listing_id)
            .fetch_optional(tx)
            .await?;
    if let Some(listing) = listing_currency
        && listing != inh.currency
    {
        return Err(UpsertError::CurrencyNotListings {
            inheritance: inh.currency.clone(),
            listing,
        });
    }
    Ok(())
}

async fn check_convertible(
    tx: &mut sqlx::SqliteConnection,
    inh: &Inheritance,
) -> Result<(), UpsertError> {
    if inh.currency.eq_ignore_ascii_case("AUD") || inh.fx_rate != Decimal::ONE {
        return Ok(());
    }
    let month = conversion_month(inh);
    let ato_rate: Option<String> =
        sqlx::query_scalar("SELECT rate FROM rba_fx_rates WHERE currency = ? AND month = ?")
            .bind(&inh.currency)
            .bind(&month)
            .fetch_optional(tx)
            .await?;
    if ato_rate.is_none() {
        return Err(UpsertError::MissingFxRate {
            currency: inh.currency.clone(),
            month,
        });
    }
    Ok(())
}

fn validate(inh: &Inheritance) -> Result<(), UpsertError> {
    if inh.quantity <= Decimal::ZERO {
        return Err(UpsertError::QuantityNotPositive);
    }
    if inh.cost_base < Decimal::ZERO || inh.lpr_expenditure < Decimal::ZERO {
        return Err(UpsertError::NegativeAmount);
    }
    if inh.fx_rate <= Decimal::ZERO {
        return Err(UpsertError::FxRateNotPositive);
    }
    // Checked before the per-rule acquisition checks so a pre-CGT death gets
    // this message (not misdirected advice to switch cost-base rules).
    if inh.date_of_death < CGT_START {
        return Err(UpsertError::DeathPreCgt);
    }
    match (inh.cost_base_rule, inh.deceased_acquisition_date) {
        (CostBaseRule::DeceasedCostBase, Some(acquired)) => {
            if acquired < CGT_START {
                return Err(UpsertError::DeceasedAcquisitionPreCgt);
            }
            if acquired > inh.date_of_death {
                return Err(UpsertError::DeceasedAcquisitionAfterDeath);
            }
        }
        (CostBaseRule::MarketValueAtDeath, None) => {}
        _ => return Err(UpsertError::RuleAcquisitionDateMismatch),
    }
    if inh.lpr_expenditure != Decimal::ZERO && !inh.currency.eq_ignore_ascii_case("AUD") {
        return Err(UpsertError::LprExpenditureOnForeignParcel);
    }
    match inh.lpr_expenditure_date {
        Some(incurred) => {
            if inh.lpr_expenditure == Decimal::ZERO {
                return Err(UpsertError::LprExpenditureDateMismatch);
            }
            if incurred < inh.date_of_death {
                return Err(UpsertError::LprExpenditureBeforeDeath);
            }
        }
        None => {
            if inh.lpr_expenditure != Decimal::ZERO {
                return Err(UpsertError::LprExpenditureDateMismatch);
            }
        }
    }
    Ok(())
}

/// Create or update an inheritance and its linked Buy, atomically. The Buy
/// carries the whole cost base (first element + LPR expenditure) on the
/// brokerage column with price 0, and the s 115-30 discount clock as its
/// deemed acquisition date (post-CGT rule only). An edit is refused while
/// the parcel is drawn on (the linked Buy keeps its id across edits).
pub async fn db_upsert(pool: &SqlitePool, inh: &Inheritance) -> Result<(), UpsertError> {
    validate(inh)?;

    let mut tx = pool.begin().await?;

    check_listing_currency(&mut tx, inh).await?;
    check_convertible(&mut tx, inh).await?;

    let existing_buy = linked_buy_id(&mut tx, inh.id).await?;
    if let Some(buy_id) = existing_buy
        && buy_drawn_on(&mut tx, buy_id).await?
    {
        return Err(UpsertError::ParcelDrawnOn);
    }

    sqlx::query(
        "INSERT INTO inheritances \
         (id, listing_id, holding_account_id, quantity, date_of_death, cost_base_rule, \
          cost_base, lpr_expenditure, lpr_expenditure_date, deceased_acquisition_date, \
          currency, fx_rate) \
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?) \
         ON CONFLICT(id) DO UPDATE SET \
             listing_id                = excluded.listing_id, \
             holding_account_id        = excluded.holding_account_id, \
             quantity                  = excluded.quantity, \
             date_of_death             = excluded.date_of_death, \
             cost_base_rule            = excluded.cost_base_rule, \
             cost_base                 = excluded.cost_base, \
             lpr_expenditure           = excluded.lpr_expenditure, \
             lpr_expenditure_date      = excluded.lpr_expenditure_date, \
             deceased_acquisition_date = excluded.deceased_acquisition_date, \
             currency                  = excluded.currency, \
             fx_rate                   = excluded.fx_rate",
    )
    .bind(inh.id)
    .bind(inh.listing_id)
    .bind(inh.holding_account_id)
    .bind(Money(inh.quantity))
    .bind(inh.date_of_death)
    .bind(inh.cost_base_rule)
    .bind(Money(inh.cost_base))
    .bind(Money(inh.lpr_expenditure))
    .bind(inh.lpr_expenditure_date)
    .bind(inh.deceased_acquisition_date)
    .bind(&inh.currency)
    .bind(Money(inh.fx_rate))
    .execute(&mut *tx)
    .await?;

    // The parcel Buy: dated and settled on the date of death, the whole cost
    // base (first element + LPR expenditure) on the brokerage column with
    // price 0, and the s 115-30 clock as the deemed acquisition date. The
    // deemed date equals the rule pairing validated above: the deceased's
    // acquisition under DeceasedCostBase, none (the death date itself) under
    // MarketValueAtDeath.
    let total_cost_base = inh.cost_base + inh.lpr_expenditure;
    let buy_id = match existing_buy {
        Some(id) => id,
        None => {
            sqlx::query_scalar("SELECT COALESCE(MAX(id), 0) + 1 FROM trades")
                .fetch_one(&mut *tx)
                .await?
        }
    };
    sqlx::query(
        "INSERT INTO trades \
         (id, trade_type, date, settlement_date, listing_id, average_price, quantity, \
          currency, brokerage, gst_on_brokerage, brokerage_currency, fx_rate, \
          holding_account_id, inheritance_id, deemed_acquisition_date) \
         VALUES (?, 'Buy', ?, ?, ?, '0', ?, ?, ?, '0', ?, ?, ?, ?, ?) \
         ON CONFLICT(id) DO UPDATE SET \
             date                    = excluded.date, \
             settlement_date         = excluded.settlement_date, \
             listing_id              = excluded.listing_id, \
             quantity                = excluded.quantity, \
             currency                = excluded.currency, \
             brokerage               = excluded.brokerage, \
             brokerage_currency      = excluded.brokerage_currency, \
             fx_rate                 = excluded.fx_rate, \
             holding_account_id      = excluded.holding_account_id, \
             deemed_acquisition_date = excluded.deemed_acquisition_date",
    )
    .bind(buy_id)
    .bind(inh.date_of_death)
    .bind(inh.date_of_death)
    .bind(inh.listing_id)
    .bind(Money(inh.quantity))
    .bind(&inh.currency)
    .bind(Money(total_cost_base))
    .bind(&inh.currency)
    .bind(Money(inh.fx_rate))
    .bind(inh.holding_account_id)
    .bind(inh.id)
    .bind(inh.deceased_acquisition_date)
    .execute(&mut *tx)
    .await?;

    // A return of capital on this listing reduces the parcel's cost base in
    // the *parcel's* own currency, so a parcel recorded in another one is a
    // state the cost-base reports refuse to compute over. `trade::db_upsert`
    // runs this over an ordinary Buy; the inheritance's Buy does not go
    // through it, so the same check runs here, over the written state inside
    // this write's own transaction.
    if let Some(conflict) =
        crate::entities::corporate_action::db_payment_currency_conflict(&mut *tx, inh.listing_id)
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
    /// The linked Buy is drawn on by a Sell allocation or AMIT adjustment —
    /// deleting the parcel would orphan those dependants. Remove them first
    /// (mapped to 422).
    ParcelDrawnOn,
}

/// Delete an inheritance and its linked Buy together, in one transaction.
pub async fn db_delete(pool: &SqlitePool, id: i64) -> Result<DeleteOutcome, sqlx::Error> {
    let mut tx = pool.begin().await?;

    let exists: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM inheritances WHERE id = ?)")
        .bind(id)
        .fetch_one(&mut *tx)
        .await?;
    if !exists {
        return Ok(DeleteOutcome::NotFound);
    }
    if let Some(buy_id) = linked_buy_id(&mut tx, id).await? {
        if buy_drawn_on(&mut tx, buy_id).await? {
            return Ok(DeleteOutcome::ParcelDrawnOn);
        }
        // The Buy goes first: it carries the FK to the inheritance.
        sqlx::query("DELETE FROM trades WHERE id = ?")
            .bind(buy_id)
            .execute(&mut *tx)
            .await?;
    }
    sqlx::query("DELETE FROM inheritances WHERE id = ?")
        .bind(id)
        .execute(&mut *tx)
        .await?;

    tx.commit().await?;
    Ok(DeleteOutcome::Deleted)
}

async fn list(State(pool): State<SqlitePool>) -> Result<Json<Vec<Inheritance>>, ApiError> {
    db_list(&pool).await.map(Json).map_err(ApiError::from)
}

async fn get_one(
    State(pool): State<SqlitePool>,
    Path(id): Path<i64>,
) -> Result<Json<Inheritance>, ApiError> {
    db_get(&pool, id)
        .await
        .map_err(ApiError::from)?
        .map(Json)
        .ok_or(ApiError::NotFound)
}

async fn upsert(
    State(pool): State<SqlitePool>,
    Path(id): Path<i64>,
    Json(body): Json<InheritanceBody>,
) -> Result<StatusCode, ApiError> {
    let inh = Inheritance {
        id,
        listing_id: body.listing_id,
        holding_account_id: body.holding_account_id,
        quantity: body.quantity,
        date_of_death: body.date_of_death,
        cost_base_rule: body.cost_base_rule,
        cost_base: body.cost_base,
        lpr_expenditure: body.lpr_expenditure.unwrap_or(Decimal::ZERO),
        lpr_expenditure_date: body.lpr_expenditure_date,
        deceased_acquisition_date: body.deceased_acquisition_date,
        currency: body.currency,
        fx_rate: body.fx_rate.unwrap_or(Decimal::ONE),
    };
    db_upsert(&pool, &inh).await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn delete(
    State(pool): State<SqlitePool>,
    Path(id): Path<i64>,
) -> Result<StatusCode, ApiError> {
    match db_delete(&pool, id).await? {
        DeleteOutcome::Deleted => Ok(StatusCode::NO_CONTENT),
        DeleteOutcome::NotFound => Err(ApiError::not_found("no inheritance with that id")),
        DeleteOutcome::ParcelDrawnOn => Err(ApiError::unprocessable(
            "the inherited parcel is drawn on by a sale allocation or AMIT adjustment — \
             remove those first",
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::entities::rba_fx_rate;
    use crate::entities::sell::{self, AllocationInput, SellBody};
    use crate::entities::trade::{self, TradeType};
    use crate::test_support::{self, ApiClient, dec, test_pool, ymd};

    async fn insert_listing(pool: &SqlitePool, id: i64, currency: &str) {
        test_support::listing(id)
            .ticker(&format!("INH{id}"))
            .name(&format!("Inherited {id}"))
            .currency(currency)
            .insert(pool)
            .await;
    }

    /// A post-CGT inheritance: the deceased acquired 2020-02-01, died
    /// 2025-01-10, their $3,000 cost base carries over, plus $200 of LPR
    /// conveyancing incurred 2025-03-01.
    fn post_cgt(id: i64) -> Inheritance {
        Inheritance {
            id,
            listing_id: 1,
            holding_account_id: 1,
            quantity: dec("100"),
            date_of_death: ymd(2025, 1, 10),
            cost_base_rule: CostBaseRule::DeceasedCostBase,
            cost_base: dec("3000"),
            lpr_expenditure: dec("200"),
            lpr_expenditure_date: Some(ymd(2025, 3, 1)),
            deceased_acquisition_date: Some(ymd(2020, 2, 1)),
            currency: "AUD".to_string(),
            fx_rate: Decimal::ONE,
        }
    }

    /// A pre-CGT inheritance: the deceased held since before 20 Sep 1985, so
    /// the first element is the (user-supplied) market value at death and no
    /// acquisition date is recorded.
    fn pre_cgt(id: i64) -> Inheritance {
        Inheritance {
            cost_base_rule: CostBaseRule::MarketValueAtDeath,
            cost_base: dec("5000"),
            lpr_expenditure: Decimal::ZERO,
            lpr_expenditure_date: None,
            deceased_acquisition_date: None,
            ..post_cgt(id)
        }
    }

    /// [`post_cgt`] in a foreign currency — no LPR expenditure, which is only
    /// recordable on an AUD inheritance (the LPR incurs it after the death,
    /// while the parcel converts at its acquisition month).
    fn post_cgt_usd(id: i64) -> Inheritance {
        Inheritance {
            currency: "USD".to_string(),
            lpr_expenditure: Decimal::ZERO,
            lpr_expenditure_date: None,
            ..post_cgt(id)
        }
    }

    async fn linked_buy(pool: &SqlitePool, inheritance_id: i64) -> trade::Trade {
        let id: i64 = sqlx::query_scalar("SELECT id FROM trades WHERE inheritance_id = ?")
            .bind(inheritance_id)
            .fetch_one(pool)
            .await
            .unwrap();
        trade::db_get(pool, id).await.unwrap().unwrap()
    }

    /// An AUD Sell of `qty` units of listing 1 at `price`, fully allocated to
    /// the `buy_id` parcel.
    fn sell_body(buy_id: i64, date: NaiveDate, qty: &str, price: &str) -> SellBody {
        SellBody {
            brokerage_includes_gst: false,
            statement_total: None,
            date,
            settlement_date: Some(date),
            listing_id: 1,
            average_price: dec(price),
            quantity: dec(qty),
            currency: "AUD".to_string(),
            brokerage: Decimal::ZERO,
            gst_on_brokerage: Decimal::ZERO,
            brokerage_currency: "AUD".to_string(),
            fx_rate: Decimal::ONE,
            spot_fx_rate: None,
            contract_note_ref: None,
            holding_account_id: 1,
            allocations: vec![AllocationInput {
                purchase_trade_id: buy_id,
                quantity_allocated: dec(qty),
            }],
        }
    }

    // DB-level tests

    /// The post-CGT entry path: one Buy dated (and settled) the date of
    /// death, price 0, the whole cost base (deceased's $3,000 + $200 LPR) on
    /// the brokerage column, and the discount clock anchored to the
    /// deceased's acquisition (s 115-30).
    #[tokio::test]
    async fn post_cgt_inheritance_creates_the_parcel_buy() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "AUD").await;

        db_upsert(&pool, &post_cgt(1)).await.unwrap();

        let buy = linked_buy(&pool, 1).await;
        assert_eq!(buy.trade_type, TradeType::Buy);
        assert_eq!(buy.date, ymd(2025, 1, 10));
        assert_eq!(buy.settlement_date, ymd(2025, 1, 10), "not market-settled");
        assert_eq!(buy.listing_id, 1);
        assert_eq!(buy.quantity, dec("100"));
        assert_eq!(buy.average_price, Decimal::ZERO);
        assert_eq!(
            buy.brokerage,
            dec("3200"),
            "first element + LPR expenditure"
        );
        assert_eq!(buy.gst_on_brokerage, Decimal::ZERO);
        assert_eq!(buy.deemed_acquisition_date, Some(ymd(2020, 2, 1)));
        assert_eq!(buy.inheritance_id, Some(1));
        assert_eq!(buy.holding_account_id, 1);
    }

    /// The pre-CGT entry path: the market value at death is the cost base and
    /// the clock runs from the date of death — the Buy's own date, no deemed
    /// date.
    #[tokio::test]
    async fn pre_cgt_inheritance_clock_runs_from_death() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "AUD").await;

        db_upsert(&pool, &pre_cgt(1)).await.unwrap();

        let buy = linked_buy(&pool, 1).await;
        assert_eq!(buy.date, ymd(2025, 1, 10));
        assert_eq!(buy.deemed_acquisition_date, None);
        assert_eq!(buy.brokerage, dec("5000"));
    }

    /// A non-AUD inheritance keeps its currency and manual FX fallback on the
    /// Buy, like every other parcel.
    #[tokio::test]
    async fn foreign_currency_inheritance_carries_currency_and_fx() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "USD").await;

        db_upsert(
            &pool,
            &Inheritance {
                fx_rate: dec("1.5"),
                ..post_cgt_usd(1)
            },
        )
        .await
        .unwrap();

        let buy = linked_buy(&pool, 1).await;
        assert_eq!(buy.currency, "USD");
        assert_eq!(buy.brokerage_currency, "USD");
        assert_eq!(buy.fx_rate, dec("1.5"));
    }

    #[tokio::test]
    async fn invalid_inheritances_are_rejected_and_nothing_persisted() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "AUD").await;

        // Zero quantity.
        let mut inh = post_cgt(1);
        inh.quantity = Decimal::ZERO;
        assert!(matches!(
            db_upsert(&pool, &inh).await,
            Err(UpsertError::QuantityNotPositive)
        ));

        // Negative cost base.
        let mut inh = post_cgt(1);
        inh.cost_base = dec("-1");
        assert!(matches!(
            db_upsert(&pool, &inh).await,
            Err(UpsertError::NegativeAmount)
        ));

        // DeceasedCostBase without the deceased's acquisition date.
        let mut inh = post_cgt(1);
        inh.deceased_acquisition_date = None;
        assert!(matches!(
            db_upsert(&pool, &inh).await,
            Err(UpsertError::RuleAcquisitionDateMismatch)
        ));

        // MarketValueAtDeath with one.
        let mut inh = pre_cgt(1);
        inh.deceased_acquisition_date = Some(ymd(1980, 1, 1));
        assert!(matches!(
            db_upsert(&pool, &inh).await,
            Err(UpsertError::RuleAcquisitionDateMismatch)
        ));

        // A pre-20-Sep-1985 acquisition under the post-CGT rule.
        let mut inh = post_cgt(1);
        inh.deceased_acquisition_date = Some(ymd(1985, 9, 19));
        assert!(matches!(
            db_upsert(&pool, &inh).await,
            Err(UpsertError::DeceasedAcquisitionPreCgt)
        ));

        // Acquired after death.
        let mut inh = post_cgt(1);
        inh.deceased_acquisition_date = Some(ymd(2025, 1, 11));
        assert!(matches!(
            db_upsert(&pool, &inh).await,
            Err(UpsertError::DeceasedAcquisitionAfterDeath)
        ));

        // A death before 20 Sep 1985 (REQUIREMENTS 2026-07-13): the parcel
        // would be pre-CGT in the beneficiary's own hands (the s 115-30
        // clock deems acquisition at the death at latest) — outside CGT and
        // not modelled, whichever cost-base rule was chosen.
        let mut inh = pre_cgt(1);
        inh.date_of_death = ymd(1985, 9, 19);
        assert!(matches!(
            db_upsert(&pool, &inh).await,
            Err(UpsertError::DeathPreCgt)
        ));
        let mut inh = post_cgt(1);
        inh.date_of_death = ymd(1985, 9, 19);
        inh.lpr_expenditure_date = Some(ymd(1985, 10, 1));
        assert!(matches!(
            db_upsert(&pool, &inh).await,
            Err(UpsertError::DeathPreCgt)
        ));

        // LPR expenditure without its date, and a date without expenditure.
        let mut inh = post_cgt(1);
        inh.lpr_expenditure_date = None;
        assert!(matches!(
            db_upsert(&pool, &inh).await,
            Err(UpsertError::LprExpenditureDateMismatch)
        ));
        let mut inh = post_cgt(1);
        inh.lpr_expenditure = Decimal::ZERO;
        assert!(matches!(
            db_upsert(&pool, &inh).await,
            Err(UpsertError::LprExpenditureDateMismatch)
        ));

        // LPR expenditure dated before the death.
        let mut inh = post_cgt(1);
        inh.lpr_expenditure_date = Some(ymd(2024, 12, 31));
        assert!(matches!(
            db_upsert(&pool, &inh).await,
            Err(UpsertError::LprExpenditureBeforeDeath)
        ));

        let inheritances: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM inheritances")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(inheritances, 0);
        let trades: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM trades")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(trades, 0, "no parcel Buy persisted");
    }

    /// Editing an undrawn inheritance updates the linked Buy in place (same
    /// trade id, no duplicate parcel).
    #[tokio::test]
    async fn edit_updates_the_linked_buy_in_place() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "AUD").await;
        db_upsert(&pool, &post_cgt(1)).await.unwrap();
        let before = linked_buy(&pool, 1).await;

        let mut edited = post_cgt(1);
        edited.quantity = dec("150");
        edited.cost_base = dec("4000");
        db_upsert(&pool, &edited).await.unwrap();

        let after = linked_buy(&pool, 1).await;
        assert_eq!(after.id, before.id, "the Buy keeps its id");
        assert_eq!(after.quantity, dec("150"));
        assert_eq!(after.brokerage, dec("4200"));
        let buys: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM trades WHERE inheritance_id IS NOT NULL")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(buys, 1, "no duplicate parcel");
    }

    /// Sell some of the parcel: the inheritance is frozen (edit and delete
    /// both refused) until the sale is removed.
    #[tokio::test]
    async fn drawn_on_parcel_freezes_edit_and_delete() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "AUD").await;
        db_upsert(&pool, &post_cgt(1)).await.unwrap();
        let buy = linked_buy(&pool, 1).await;

        sell::db_upsert_sell(&pool, 50, &sell_body(buy.id, ymd(2025, 6, 1), "30", "40"))
            .await
            .unwrap();

        assert!(matches!(
            db_upsert(&pool, &post_cgt(1)).await,
            Err(UpsertError::ParcelDrawnOn)
        ));
        assert_eq!(
            db_delete(&pool, 1).await.unwrap(),
            DeleteOutcome::ParcelDrawnOn
        );

        // Removing the sale unfreezes both.
        assert_eq!(
            sell::db_delete_sell(&pool, 50).await.unwrap(),
            sell::DeleteOutcome::Deleted
        );
        db_upsert(&pool, &post_cgt(1)).await.unwrap();
        assert_eq!(db_delete(&pool, 1).await.unwrap(), DeleteOutcome::Deleted);
        assert!(db_get(&pool, 1).await.unwrap().is_none());
        assert!(
            trade::db_get(&pool, buy.id).await.unwrap().is_none(),
            "the parcel Buy goes with the inheritance"
        );
    }

    /// The linked Buy is immutable individually — PUT /trades rejects it and
    /// DELETE /trades refuses it; the inheritance is the write path.
    #[tokio::test]
    async fn linked_buy_is_immutable_individually() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "AUD").await;
        db_upsert(&pool, &post_cgt(1)).await.unwrap();
        let buy = linked_buy(&pool, 1).await;

        let mut edited = buy.clone();
        edited.quantity = dec("999");
        assert!(matches!(
            trade::db_upsert(&pool, &edited).await,
            Err(trade::UpsertError::InheritedParcelTrade)
        ));
        assert_eq!(
            trade::db_delete(&pool, buy.id).await.unwrap(),
            trade::DeleteOutcome::Referenced
        );
    }

    /// The parcel is subject to the same write-time capacity check as any
    /// Buy: a Sell drawing more than the inherited units is rejected.
    #[tokio::test]
    async fn capacity_check_applies_like_any_buy() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "AUD").await;
        db_upsert(&pool, &post_cgt(1)).await.unwrap();
        let buy = linked_buy(&pool, 1).await;

        let result =
            sell::db_upsert_sell(&pool, 50, &sell_body(buy.id, ymd(2025, 6, 1), "101", "40")).await;
        assert!(matches!(
            result,
            Err(sell::SellError::PurchaseQuantityExceeded)
        ));
    }

    // Report flow-through

    /// The parcel flows through the open-parcels report like any Buy: the
    /// whole cost base (first element + LPR expenditure) and the s 115-30
    /// acquisition date (the deceased's, under the post-CGT rule).
    #[tokio::test]
    async fn parcel_flows_through_open_parcels() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "AUD").await;
        db_upsert(&pool, &post_cgt(1)).await.unwrap();

        let parcels = crate::reports::open_parcels::db_open_parcels(&pool)
            .await
            .unwrap();
        assert_eq!(parcels.len(), 1);
        let p = &parcels[0];
        assert_eq!(
            p.acquisition_date,
            ymd(2020, 2, 1),
            "the deceased's acquisition starts the clock"
        );
        assert_eq!(p.remaining_quantity, dec("100"));
        assert_eq!(p.original_cost_base, dec("3200"));
    }

    /// s 115-30 post-CGT case: a sale within 12 months of the death but more
    /// than 12 months after the deceased's acquisition is discount-eligible —
    /// the beneficiary is treated as having owned the asset since the
    /// deceased acquired it (docs/ato/inherited-assets-cgt-discount.md).
    #[tokio::test]
    async fn post_cgt_gain_discounts_off_the_deceaseds_acquisition() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "AUD").await;
        db_upsert(&pool, &post_cgt(1)).await.unwrap();
        let buy = linked_buy(&pool, 1).await;

        // 2025-06-01: under 5 months after the 2025-01-10 death, over 5 years
        // after the deceased's 2020-02-01 acquisition. 100 × $40 = $4,000
        // proceeds against the $3,200 inherited cost base.
        sell::db_upsert_sell(&pool, 50, &sell_body(buy.id, ymd(2025, 6, 1), "100", "40"))
            .await
            .unwrap();

        let realised = crate::reports::realised_gains::db_realised_gains(&pool)
            .await
            .unwrap();
        assert_eq!(realised.len(), 1);
        let g = &realised[0];
        assert_eq!(g.cost_base, dec("3200"));
        assert_eq!(g.capital_gain_loss, dec("800"));
        assert_eq!(g.discount_eligible_gain, dec("800"));
        assert_eq!(g.non_discountable_gain, Decimal::ZERO);
    }

    /// s 115-30 pre-CGT case: the clock runs from the date of death — a sale
    /// within 12 months of the death is non-discountable, one after is
    /// discount-eligible.
    #[tokio::test]
    async fn pre_cgt_gain_discounts_off_the_death() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "AUD").await;
        db_upsert(&pool, &pre_cgt(1)).await.unwrap();
        let buy = linked_buy(&pool, 1).await;

        // $5,000 market value at death over 100 units = $50/unit cost base.
        // 30 units at $60 on 2025-06-01 (< 12 months after the 2025-01-10
        // death): $300 gain, non-discountable.
        sell::db_upsert_sell(&pool, 50, &sell_body(buy.id, ymd(2025, 6, 1), "30", "60"))
            .await
            .unwrap();
        // 30 more on 2026-02-01 (> 12 months after the death): discountable.
        sell::db_upsert_sell(&pool, 51, &sell_body(buy.id, ymd(2026, 2, 1), "30", "60"))
            .await
            .unwrap();

        let realised = crate::reports::realised_gains::db_realised_gains(&pool)
            .await
            .unwrap();
        assert_eq!(realised.len(), 2);
        let within = realised.iter().find(|g| g.sale_trade_id == 50).unwrap();
        assert_eq!(within.capital_gain_loss, dec("300"));
        assert_eq!(within.non_discountable_gain, dec("300"));
        assert_eq!(within.discount_eligible_gain, Decimal::ZERO);
        let after = realised.iter().find(|g| g.sale_trade_id == 51).unwrap();
        assert_eq!(after.capital_gain_loss, dec("300"));
        assert_eq!(after.discount_eligible_gain, dec("300"));
        assert_eq!(after.non_discountable_gain, Decimal::ZERO);
    }

    /// The two `trade::check_amounts` rules the inheritance's own validation
    /// did not cover — a non-positive fallback FX rate, and a parcel currency
    /// a return of capital on the listing contradicts — are refused here
    /// rather than left to fail as a `500` from every cost-base report
    /// (SCENARIOS K-01, K-02, K-04). Both are exactly what `PUT /trades`
    /// answers for an ordinary Buy.
    #[tokio::test]
    async fn the_parcel_buys_trade_checks_are_enforced_here() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "USD").await;
        let api = ApiClient::full(&pool);

        // `apply_rate` divides by this, so a zero rate is a panicking report,
        // not merely a wrong figure.
        for rate in ["0", "-0.65"] {
            let bad = Inheritance {
                fx_rate: dec(rate),
                ..post_cgt_usd(1)
            };
            let err = db_upsert(&pool, &bad).await.unwrap_err();
            assert!(
                matches!(err, UpsertError::FxRateNotPositive),
                "rate {rate}: {err:?}"
            );
        }
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM trades")
                .fetch_one(&pool)
                .await
                .unwrap(),
            0,
            "nothing persisted"
        );

        // An AUD return of capital on a USD listing: `PUT /trades` refuses a
        // USD Buy of it, and so must the inheritance.
        api.put_ok(
            "/corporate_actions/1",
            &serde_json::json!({
                "listing_id": 1, "date": "2025-06-01", "action_type": "ReturnOfCapital",
                "amount_per_unit": "2.00", "currency": "AUD",
            }),
        )
        .await;
        let usd = Inheritance {
            fx_rate: dec("0.65"),
            ..post_cgt_usd(1)
        };
        let err = db_upsert(&pool, &usd).await.unwrap_err();
        let detail = ApiError::from(err);
        let ApiError::Unprocessable(body) = detail else {
            panic!("expected 422");
        };
        assert!(body.contains("held in USD"), "body: {body}");
        assert!(body.contains("recorded in AUD"), "body: {body}");

        // And the report the bad state used to break still answers.
        api.get("/portfolio/open-parcels")
            .await
            .expect_status(StatusCode::OK);
    }

    /// LPR expenditure is only recordable on an AUD inheritance (SCENARIOS
    /// K-04). The whole parcel converts at one rate — its (deemed)
    /// acquisition month's — while the LPR incurs the expenditure *after* the
    /// death, so on a foreign parcel the element would translate at a month
    /// that can predate the expense by decades. Refused rather than reported
    /// wrongly; the AUD case, where the conversion is the identity, is
    /// untouched.
    #[tokio::test]
    async fn lpr_expenditure_is_refused_on_a_foreign_parcel() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "USD").await;
        insert_listing(&pool, 2, "AUD").await;
        rba_fx_rate::db_import_rate(&pool, "USD", "2020-02", dec("0.5"))
            .await
            .unwrap();

        // The deceased's US$3,000 cost base is fine on its own…
        db_upsert(&pool, &post_cgt_usd(1)).await.unwrap();
        // …but a US$1,000 LPR fee incurred in 2025 would be translated at the
        // deceased's 2020-02 rate along with it.
        let with_lpr = Inheritance {
            lpr_expenditure: dec("1000"),
            lpr_expenditure_date: Some(ymd(2025, 3, 1)),
            ..post_cgt_usd(1)
        };
        let err = db_upsert(&pool, &with_lpr).await.unwrap_err();
        assert!(
            matches!(err, UpsertError::LprExpenditureOnForeignParcel),
            "{err:?}"
        );
        // The accepted row is untouched by the rejected write.
        let parcels = crate::domain::open_parcels::load(&mut pool.acquire().await.unwrap(), None)
            .await
            .unwrap();
        assert_eq!(parcels[0].cost_base.adjusted, dec("6000"));

        // An explicit zero is not "a non-zero expenditure", so it passes; and
        // the AUD parcel takes its LPR fee as before.
        db_upsert(
            &pool,
            &Inheritance {
                id: 2,
                listing_id: 2,
                currency: "AUD".to_string(),
                fx_rate: Decimal::ONE,
                ..post_cgt(2)
            },
        )
        .await
        .unwrap();
        let buy = linked_buy(&pool, 2).await;
        assert_eq!(buy.brokerage, dec("3200"), "$3,000 + the $200 LPR fee");
    }

    /// An inheritance's currency is its listing's, or it is refused: the
    /// parcel is a holding of that listed security and the exchange prices it
    /// in the listing's own currency (SCENARIOS K-01).
    #[tokio::test]
    async fn an_inheritance_in_another_currency_than_its_listings_is_refused() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "AUD").await;
        insert_listing(&pool, 2, "USD").await;

        let mismatched = Inheritance {
            fx_rate: dec("0.65"),
            ..post_cgt_usd(1)
        };
        let err = db_upsert(&pool, &mismatched).await.unwrap_err();
        assert!(
            matches!(&err, UpsertError::CurrencyNotListings { inheritance, listing }
                     if inheritance == "USD" && listing == "AUD"),
            "{err:?}"
        );
        let ApiError::Unprocessable(body) = ApiError::from(err) else {
            panic!("expected 422");
        };
        assert!(body.contains("recorded in USD"), "body: {body}");
        assert!(body.contains("quoted in AUD"), "body: {body}");
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM trades")
                .fetch_one(&pool)
                .await
                .unwrap(),
            0,
            "nothing persisted"
        );

        // The matching pair is unaffected, either way round.
        db_upsert(&pool, &post_cgt(1)).await.unwrap();
        db_upsert(
            &pool,
            &Inheritance {
                id: 2,
                listing_id: 2,
                ..mismatched
            },
        )
        .await
        .unwrap();
    }

    /// A non-AUD inheritance whose conversion month has no imported ATO rate
    /// and that states no rate of its own is refused, not costed at parity —
    /// and the month is the *deceased's* acquisition month, so it is the old
    /// one, not the death's (SCENARIOS K-01, K-04).
    #[tokio::test]
    async fn a_non_aud_inheritance_with_no_rate_is_refused_not_costed_at_parity() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "USD").await;
        // The deceased acquired 2020-02 and died 2025-01: importing the
        // *death* month is not enough, because the cost base converts at the
        // deceased's acquisition month.
        rba_fx_rate::db_import_rate(&pool, "USD", "2025-01", dec("0.62"))
            .await
            .unwrap();
        let usd = post_cgt_usd(1);

        let err = db_upsert(&pool, &usd).await.unwrap_err();
        assert!(
            matches!(&err, UpsertError::MissingFxRate { currency, month }
                     if currency == "USD" && month == "2020-02"),
            "{err:?}"
        );
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM trades")
                .fetch_one(&pool)
                .await
                .unwrap(),
            0,
            "nothing persisted"
        );

        // Either half of the pair is enough: the taxpayer's own rate…
        let stated = Inheritance {
            fx_rate: dec("0.75"),
            ..usd.clone()
        };
        db_upsert(&pool, &stated).await.unwrap();
        let parcels = crate::domain::open_parcels::load(&mut pool.acquire().await.unwrap(), None)
            .await
            .unwrap();
        // US$3,000 at the stated 0.75, not at parity.
        assert_eq!(parcels[0].cost_base.adjusted, dec("4000"));

        // …or the acquisition month's ATO rate, which then outranks it.
        rba_fx_rate::db_import_rate(&pool, "USD", "2020-02", dec("0.80"))
            .await
            .unwrap();
        db_upsert(&pool, &usd).await.unwrap();
        let parcels = crate::domain::open_parcels::load(&mut pool.acquire().await.unwrap(), None)
            .await
            .unwrap();
        assert_eq!(parcels[0].cost_base.adjusted, dec("3750"));
    }

    /// The inherited parcel's two dates do different jobs, and a corporate
    /// action must read the *death*, not the deceased's acquisition
    /// (SCENARIOS K-01, K-04). A return of capital paid while the deceased
    /// still held the units was received by them — it is already inside the
    /// cost base at death that carries over (QC 66053), so it must not
    /// reduce the beneficiary's parcel a second time; one paid after the
    /// death reaches it as CGT event G1 like any parcel's.
    #[tokio::test]
    async fn a_payment_before_the_death_does_not_reduce_the_inherited_parcel() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "AUD").await;
        // Deceased acquired 2020-02-01, died 2025-01-10, $3,200 cost base.
        db_upsert(&pool, &post_cgt(1)).await.unwrap();
        let api = ApiClient::full(&pool);

        // $1.00/unit paid while the deceased held the units, and $2.00/unit
        // paid after the beneficiary inherited them.
        for (id, date, amount) in [(1, "2022-06-01", "1.00"), (2, "2025-06-01", "2.00")] {
            api.put_ok(
                &format!("/corporate_actions/{id}"),
                &serde_json::json!({
                    "listing_id": 1, "date": date, "action_type": "ReturnOfCapital",
                    "amount_per_unit": amount, "currency": "AUD",
                }),
            )
            .await;
        }

        let parcels = crate::domain::open_parcels::load(&mut pool.acquire().await.unwrap(), None)
            .await
            .unwrap();
        assert_eq!(parcels.len(), 1);
        // Only the post-death payment: 100 × $2.00, not 100 × $3.00.
        assert_eq!(parcels[0].cost_base.roc_reduction, dec("200"));
        assert_eq!(parcels[0].cost_base.adjusted, dec("3000"));
    }

    /// The same division of labour for a split: the inherited quantity is
    /// stated in date-of-death terms, so a split *before* the death is
    /// already in it and must not re-base the parcel — even though the
    /// discount clock reaches back past it to the deceased's acquisition
    /// (SCENARIOS K-01).
    #[tokio::test]
    async fn a_split_before_the_death_does_not_rebase_the_inherited_quantity() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "AUD").await;
        db_upsert(&pool, &post_cgt(1)).await.unwrap();
        let api = ApiClient::full(&pool);

        // A 2-for-1 split after the deceased acquired but before they died.
        api.put_ok(
            "/corporate_actions/1",
            &serde_json::json!({
                "listing_id": 1, "date": "2022-06-01", "action_type": "ShareSplit",
                "split_new_units": "2", "split_old_units": "1",
            }),
        )
        .await;

        let parcels = crate::domain::open_parcels::load(&mut pool.acquire().await.unwrap(), None)
            .await
            .unwrap();
        assert_eq!(parcels.len(), 1);
        assert_eq!(
            parcels[0].remaining_as_of,
            dec("100"),
            "not re-based to 200"
        );
        assert_eq!(parcels[0].parcel.acquired(), ymd(2020, 2, 1));
    }

    /// K-08: an inherited parcel that is subsequently demerged. The rollover
    /// carries the *deceased's* acquisition date into both the head and the
    /// demerged replacement parcels, so each still discounts off s 115-30 and
    /// not off the demerger date, and the cost base splits by the action's
    /// percentage.
    #[tokio::test]
    async fn a_demerged_inherited_parcel_carries_the_deceaseds_clock() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "AUD").await;
        insert_listing(&pool, 2, "AUD").await;
        db_upsert(&pool, &post_cgt(1)).await.unwrap();
        let api = ApiClient::full(&pool);

        api.put_ok(
            "/corporate_actions/1",
            &serde_json::json!({
                "listing_id": 1, "date": "2025-09-01", "action_type": "Demerger",
                "demerger_listing_id": 2, "demerger_new_units": "1",
                "demerger_held_units": "5", "demerger_cost_base_pct": "10",
            }),
        )
        .await;
        api.post("/corporate_actions/1/demerge", &serde_json::json!({}))
            .await
            .expect_status(StatusCode::CREATED);

        let parcels = crate::domain::open_parcels::load(&mut pool.acquire().await.unwrap(), None)
            .await
            .unwrap();
        let head = parcels
            .iter()
            .find(|p| p.parcel.listing_id == 1)
            .expect("head parcel");
        let demerged = parcels
            .iter()
            .find(|p| p.parcel.listing_id == 2)
            .expect("demerged parcel");
        assert_eq!(head.parcel.acquired(), ymd(2020, 2, 1));
        assert_eq!(demerged.parcel.acquired(), ymd(2020, 2, 1));
        assert_eq!(head.cost_base.adjusted, dec("2880"), "90% of $3,200");
        assert_eq!(demerged.cost_base.adjusted, dec("320"), "10% of $3,200");
        assert_eq!(demerged.remaining_as_of, dec("20"), "1 new unit per 5 held");

        // The closing Sell now draws on the inherited parcel, so the
        // inheritance itself is frozen.
        let refused = api.delete("/inheritances/1").await;
        let (status, body) = refused.status_and_body();
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
        assert!(body.contains("drawn on"), "body: {body}");
    }

    /// K-07: an inherited parcel of an AMIT fund, with the fund's AMMA
    /// statement for the year of death. The statement's units are the ones
    /// the beneficiary held at year end, so generation covers the inherited
    /// parcel and its per-unit CGT event E10 reduction reaches the carried
    /// cost base.
    #[tokio::test]
    async fn an_amma_statement_for_the_year_of_death_adjusts_the_inherited_parcel() {
        let pool = test_pool().await;
        test_support::listing(1)
            .ticker("VDHG")
            .name("Vanguard Diversified High Growth")
            .amit(true)
            .amit_from(ymd(2015, 7, 1))
            .insert(&pool)
            .await;
        // Died 2024-11-15, mid-FY2025: 1,000 units at a $50,000 cost base.
        let inherited = Inheritance {
            quantity: dec("1000"),
            date_of_death: ymd(2024, 11, 15),
            cost_base: dec("50000"),
            lpr_expenditure: Decimal::ZERO,
            lpr_expenditure_date: None,
            deceased_acquisition_date: Some(ymd(2018, 3, 1)),
            ..post_cgt(1)
        };
        db_upsert(&pool, &inherited).await.unwrap();
        let api = ApiClient::full(&pool);

        api.put_ok(
            "/amma_statements/1",
            &serde_json::json!({
                "listing_id": 1, "tax_year_end_date": "2025-06-30", "units_held": "1000",
                "date_received": "2025-08-01", "australian_dividends_unfranked": "300",
                "cost_base_adjustment": "0.25", "holding_account_id": 1,
            }),
        )
        .await;
        api.post(
            "/amma_statements/1/generate_adjustments",
            &serde_json::json!({}),
        )
        .await
        .expect_status(StatusCode::CREATED);

        let parcels = crate::domain::open_parcels::load(&mut pool.acquire().await.unwrap(), None)
            .await
            .unwrap();
        assert_eq!(parcels.len(), 1);
        // 1,000 × $0.25 off the carried $50,000, per CGT event E10.
        assert_eq!(parcels[0].cost_base.amit_reduction, dec("250.00"));
        assert_eq!(parcels[0].cost_base.adjusted, dec("49750.00"));
        assert_eq!(parcels[0].parcel.acquired(), ymd(2018, 3, 1));

        // Nothing is left unexplained: the statement's units and the units
        // adjusted agree, so the cross-check is silent.
        let problems: Vec<serde_json::Value> =
            api.get_json("/reports/amit_adjustment_cross_check").await;
        assert!(problems.is_empty(), "cross-check: {problems:?}");
    }

    // API-level tests

    #[tokio::test]
    async fn api_round_trip() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "AUD").await;
        let app = || router().with_state(pool.clone());

        let body = serde_json::json!({
            "listing_id": 1,
            "quantity": "100",
            "date_of_death": "2025-01-10",
            "cost_base_rule": "DeceasedCostBase",
            "cost_base": "3000",
            "lpr_expenditure": "200",
            "lpr_expenditure_date": "2025-03-01",
            "deceased_acquisition_date": "2020-02-01"
        });
        let resp = ApiClient::over(app()).put("/inheritances/1", &body).await;
        assert_eq!(resp.status, StatusCode::NO_CONTENT);

        // The omitted fields took their defaults (account 1, AUD, fx 1).
        let resp = ApiClient::over(app()).get("/inheritances/1").await;
        assert_eq!(resp.status, StatusCode::OK);
        let got: Inheritance = resp.json();
        assert_eq!(got.holding_account_id, 1);
        assert_eq!(got.currency, "AUD");
        assert_eq!(got.fx_rate, Decimal::ONE);
        assert_eq!(got.cost_base_rule, CostBaseRule::DeceasedCostBase);

        // List.
        let resp = ApiClient::over(app()).get("/inheritances").await;
        assert_eq!(resp.status, StatusCode::OK);

        // DELETE removes it and its Buy; a second DELETE is 404.
        let resp = ApiClient::over(app()).delete("/inheritances/1").await;
        assert_eq!(resp.status, StatusCode::NO_CONTENT);
        let resp = ApiClient::over(app()).delete("/inheritances/1").await;
        assert_eq!(resp.status, StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn api_invalid_inheritance_returns_422_with_reason() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "AUD").await;

        // The post-CGT rule without the deceased's acquisition date.
        let body = serde_json::json!({
            "listing_id": 1,
            "quantity": "100",
            "date_of_death": "2025-01-10",
            "cost_base_rule": "DeceasedCostBase",
            "cost_base": "3000"
        });
        let resp = ApiClient::over(router().with_state(pool))
            .put("/inheritances/1", &body)
            .await;
        assert_eq!(resp.status, StatusCode::UNPROCESSABLE_ENTITY);
        let detail = resp.text().to_string();
        assert!(detail.contains("acquisition date"), "detail: {detail}");
    }
}
