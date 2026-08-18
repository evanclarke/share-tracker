//! Employee share scheme (ESS) statement: the income side of an ESS interest
//! (docs/ato/employee-share-schemes.md). One row captures one Employee share
//! scheme statement attributed to a (listing, holding account): the Item 12
//! discount labels, the foreign-source memo, the TFN withheld, the taxing-point
//! date, and the per-share market value and quantity that vest.
//!
//! The assessable discount it carries reaches the tax summary
//! (`reports::tax_summary`), which totals D + E + F + G net of the $1,000
//! taxed-upfront reduction per Australian financial year. The CGT side is tied
//! in by the vesting operation (`entities::ess_vest`): it creates the
//! cost-base-reset Buy (quantity vested, price = market value at the taxing
//! point) linked back via `trades.ess_statement_id`.
//!
//! Write-time rules on what a statement may say: its `currency` must be its
//! listing's (the per-share market value and the listed price are the same
//! money — the rule the DRP reinvest path makes about a distribution's cash),
//! and a stated `fx_rate` must be positive and is only accepted on a non-AUD
//! statement. Both are refused `422` rather than reaching a report.
//!
//! The amounts are checked the same way ([`validate`]): no negative figure, a
//! taxing point on or after the start of CGT, the label-A memo no larger than
//! the discounts it is a memo of, and the discount no larger than the market
//! value of the shares that vest. Each is refused `422` — an employer's
//! statement is the source of these figures, so a contradiction between them is
//! a transcription slip, and every one of them reaches the tax summary and the
//! printed annual document unchallenged otherwise.
//!
//! Integrity mirrors the corporate-action groups: while a statement's vest Buy
//! exists the statement is **frozen** against edits (`PUT` → 422; delete the
//! vest first), and deleting the statement removes its vest Buy in the same
//! transaction — **refused** (422) while that Buy is drawn on by a Sell
//! allocation or AMIT adjustment.

use crate::infra::decimal::{Money, OptMoney};
use crate::infra::http::{self, ApiError, CrudEntity};
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

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct EssStatement {
    pub id: i64,
    pub listing_id: i64,
    /// The account the ESS interests vest into (an employer-plan account,
    /// typically). Defaults to the seeded default account when omitted.
    pub holding_account_id: i64,
    /// The taxing point: the Australian financial year this date falls in is the
    /// assessable year, and the vest Buy's acquisition/settlement date.
    pub taxing_point_date: NaiveDate,
    /// Shares that vest at the taxing point and their per-share market value —
    /// together the cost-base-reset Buy (quantity, price) the vesting operation
    /// creates. Positive for a vest.
    #[sqlx(try_from = "Money")]
    pub quantity: Decimal,
    #[sqlx(try_from = "Money")]
    pub market_value_per_share: Decimal,
    /// Item 12 label D: taxed-upfront discount eligible for the $1,000 reduction.
    #[sqlx(try_from = "Money")]
    pub taxed_upfront_eligible: Decimal,
    /// Item 12 label E: taxed-upfront discount not eligible for the reduction.
    #[sqlx(try_from = "Money")]
    pub taxed_upfront_not_eligible: Decimal,
    /// Item 12 label F: deferral-scheme discount (the RSU case).
    #[sqlx(try_from = "Money")]
    pub deferral_discount: Decimal,
    /// Pre-1 July 2009 ESS interests whose cessation time falls in the year
    /// (assessable this year, the same as the other discount labels).
    #[sqlx(try_from = "Money")]
    pub pre_2009_cessation_discount: Decimal,
    /// Item 12 label A: the foreign-source portion of the above discounts — a
    /// memo already counted within the discount labels, surfaced separately by
    /// the tax summary for the foreign-income/FITO calculation. Not added on top.
    #[sqlx(try_from = "Money")]
    pub foreign_source_discount: Decimal,
    /// Item 12 label C: TFN amounts withheld from the discounts.
    #[sqlx(try_from = "Money")]
    pub tfn_withholding: Decimal,
    /// ISO 4217 currency the amounts are denominated in. The tax summary
    /// converts non-AUD amounts to AUD via the ATO rate for this currency and
    /// the month of `taxing_point_date` (see `infra::fx::to_aud`). Defaults to AUD.
    /// Must be the **listing's** currency: `market_value_per_share` is the market
    /// value of that listed share, so the two are the same money (422 otherwise).
    pub currency: String,
    /// The foreign-per-AUD rate the taxpayer states for this statement (same
    /// convention as `trades.fx_rate` / `inheritances.fx_rate`: AUD = foreign /
    /// rate), used as the **fallback** when no ATO monthly rate exists for the
    /// taxing point's month — on both sides: the vest Buy carries it, and the
    /// tax summary converts the discount labels through it. `None` means none
    /// stated, and then the vest resolves the month's ATO rate or refuses (422)
    /// rather than costing the parcel at parity. Only accepted on a non-AUD
    /// statement, and must be positive (422 otherwise).
    #[sqlx(try_from = "OptMoney")]
    pub fx_rate: Option<Decimal>,
    /// Statement-AUD overrides, one per discount label: the employer's annual
    /// Employee share scheme statement (and the ATO prefill) states each label
    /// in AUD at the release-date spot rate, which differs from the RBA monthly
    /// rate. When present the tax summary reports the figure verbatim for that
    /// label; absent, the label converts via the RBA rate as usual. Only
    /// accepted on a non-AUD statement (422 otherwise — two AUD figures for the
    /// same label could silently disagree).
    #[sqlx(try_from = "OptMoney")]
    pub aud_taxed_upfront_eligible: Option<Decimal>,
    #[sqlx(try_from = "OptMoney")]
    pub aud_taxed_upfront_not_eligible: Option<Decimal>,
    #[sqlx(try_from = "OptMoney")]
    pub aud_deferral_discount: Option<Decimal>,
    #[sqlx(try_from = "OptMoney")]
    pub aud_pre_2009_cessation_discount: Option<Decimal>,
    #[sqlx(try_from = "OptMoney")]
    pub aud_foreign_source_discount: Option<Decimal>,
    /// Read-only: the id of the statement's vest Buy (the trade whose
    /// `ess_statement_id` links back here), `None` while unvested. Derived on
    /// read — not a stored column, ignored on write — so the web UI can offer
    /// the Vest action only on unvested rows.
    #[serde(default)]
    pub vest_trade_id: Option<i64>,
}

#[derive(Debug, Deserialize)]
pub struct EssStatementBody {
    pub listing_id: i64,
    #[serde(default = "crate::entities::holding_account::default_holding_account_id")]
    pub holding_account_id: i64,
    pub taxing_point_date: NaiveDate,
    #[serde(default)]
    pub quantity: Decimal,
    #[serde(default)]
    pub market_value_per_share: Decimal,
    #[serde(default)]
    pub taxed_upfront_eligible: Decimal,
    #[serde(default)]
    pub taxed_upfront_not_eligible: Decimal,
    #[serde(default)]
    pub deferral_discount: Decimal,
    #[serde(default)]
    pub pre_2009_cessation_discount: Decimal,
    #[serde(default)]
    pub foreign_source_discount: Decimal,
    #[serde(default)]
    pub tfn_withholding: Decimal,
    #[serde(default = "default_currency")]
    pub currency: String,
    /// Absent/null means none stated (the AUD case, and the non-AUD case that
    /// relies on the imported ATO rate).
    #[serde(default)]
    pub fx_rate: Option<Decimal>,
    #[serde(default)]
    pub aud_taxed_upfront_eligible: Option<Decimal>,
    #[serde(default)]
    pub aud_taxed_upfront_not_eligible: Option<Decimal>,
    #[serde(default)]
    pub aud_deferral_discount: Option<Decimal>,
    #[serde(default)]
    pub aud_pre_2009_cessation_discount: Option<Decimal>,
    #[serde(default)]
    pub aud_foreign_source_discount: Option<Decimal>,
}

fn default_currency() -> String {
    "AUD".to_string()
}

impl CrudEntity for EssStatement {
    type Key = i64;
    const TABLE: &'static str = "ess_statements";
    const COLUMNS: &'static str = COLUMNS;
    const ORDER_BY: &'static str = "taxing_point_date, id";
    const NOUN: &'static str = "ESS statement";
}

pub fn router() -> Router<SqlitePool> {
    Router::new()
        .route("/ess_statements", get(http::list_handler::<EssStatement>))
        .route(
            "/ess_statements/{id}",
            get(http::get_handler::<EssStatement>)
                .put(upsert)
                .delete(delete),
        )
}

/// The SELECT list `EssStatement::from_row` maps — includes the derived
/// `vest_trade_id` back-link, so any query producing an `EssStatement` must
/// select this, never `*`.
pub(crate) const COLUMNS: &str = "id, listing_id, holding_account_id, taxing_point_date, quantity, \
     market_value_per_share, taxed_upfront_eligible, taxed_upfront_not_eligible, \
     deferral_discount, pre_2009_cessation_discount, foreign_source_discount, \
     tfn_withholding, currency, fx_rate, aud_taxed_upfront_eligible, \
     aud_taxed_upfront_not_eligible, \
     aud_deferral_discount, aud_pre_2009_cessation_discount, aud_foreign_source_discount, \
     (SELECT id FROM trades WHERE trades.ess_statement_id = ess_statements.id) AS vest_trade_id";

#[cfg(test)]
pub async fn db_list(pool: &SqlitePool) -> Result<Vec<EssStatement>, sqlx::Error> {
    http::crud_list(pool).await
}

#[cfg(test)]
pub async fn db_get(pool: &SqlitePool, id: i64) -> Result<Option<EssStatement>, sqlx::Error> {
    http::crud_get(pool, id).await
}

#[derive(thiserror::Error, Debug)]
pub enum UpsertError {
    #[error("ESS statement write failed: {0}")]
    Db(#[from] sqlx::Error),
    /// The statement already has a vest Buy (`trades.ess_statement_id`) and the
    /// edit changes a field that Buy was created from (listing, account, taxing
    /// point, quantity, market value, or currency), which would desync it.
    /// Income-side fields (the discount labels, TFN withheld, the statement-AUD
    /// overrides) stay editable — the employer's annual ESS statement arrives
    /// after the vest is recorded. Delete the statement (which removes the vest)
    /// and re-enter to change the vest side. Mapped to 422.
    #[error("this ESS statement is vested: the fields its vest Buy came from cannot change")]
    Vested,
    /// A statement-AUD override was supplied on a statement already denominated
    /// in AUD — the label amount *is* the AUD figure, and a second one could
    /// silently disagree with it. Mapped to 422.
    #[error("a statement-AUD override was supplied on a statement already denominated in AUD")]
    AudOverrideOnAudStatement,
    /// A stated `fx_rate` that is not positive — it divides the amounts
    /// (`AUD = foreign / rate`), so zero or negative is not a rate. Mapped to 422.
    #[error("the statement's fx_rate must be positive")]
    FxRateNotPositive,
    /// A stated `fx_rate` on an AUD statement, where no conversion ever
    /// happens. Mapped to 422.
    #[error("an fx_rate was supplied on a statement already denominated in AUD")]
    FxRateOnAudStatement,
    /// The statement's currency is not its listing's. `market_value_per_share`
    /// is the market value of that listed share, so the two are the same money
    /// — differing currencies are a data-entry slip (and the vest would copy
    /// the statement's currency onto a parcel of a listing priced in another).
    /// Mapped to 422. Same rule as the DRP reinvest path.
    #[error("the ESS statement is in {statement} but its listing is in {listing}")]
    CurrencyNotListings { statement: String, listing: String },
    /// A negative amount on the statement (carries the field name): an
    /// employer's statement reports positive (or zero) figures. A negative
    /// discount label nets against the year's other statements, a negative TFN
    /// amount withheld is a refund from nowhere, and a negative quantity or
    /// market value describes a parcel that cannot exist. Mapped to 422.
    #[error("{0} cannot be negative")]
    NegativeAmount(&'static str),
    /// The taxing point is before the start of CGT, 20 September 1985
    /// (`trade::CGT_START`). The vest Buy this statement creates is dated the
    /// taxing point, and `trade::db_upsert` refuses exactly that trade — a
    /// pre-CGT holding is outside CGT and not modelled — so the statement is
    /// refused at the earlier, better place: what the user typed. Nothing about
    /// an ESS interest can genuinely predate it (Division 83A dates from 2009,
    /// its predecessor from 1995), so this is a typo guard. Mapped to 422.
    #[error("the taxing point is before the start of CGT (20 September 1985)")]
    PreCgtTaxingPoint,
    /// The foreign-source memo (label A) exceeds the discount labels it is a
    /// memo *of* (D + E + F + G — see [`EssStatement::foreign_source_discount`]),
    /// so it claims more foreign-source income than there is assessable
    /// discount. Carries which label set was checked (the statement's own
    /// amounts, or the statement-AUD overrides) and both figures. Same shape as
    /// income's CFI-within-unfranked check. Mapped to 422.
    #[error("{label} {foreign} exceeds the {discounts} of discount it is a memo of")]
    ForeignSourceExceedsDiscounts {
        label: &'static str,
        foreign: Decimal,
        discounts: Decimal,
    },
    /// The discount labels total more than the market value of the shares that
    /// vest (`quantity × market_value_per_share`). The discount *is* market
    /// value less what the employee paid (docs/ato/employee-share-schemes.md),
    /// so a larger discount implies a negative payment — the shape a transposed
    /// column or a foreign-currency discount against an AUD market value makes.
    /// Only checked when both figures are positive (an income-only statement
    /// leaves them zero, which is legitimate). Mapped to 422.
    #[error("the discount labels total {discount}, above the {market_value} that vests")]
    DiscountExceedsMarketValue {
        discount: Decimal,
        market_value: Decimal,
    },
}

/// Round half away from zero to the cent, the way employer statements do.
fn to_cents(v: Decimal) -> Decimal {
    v.round_dp_with_strategy(2, rust_decimal::RoundingStrategy::MidpointAwayFromZero)
}

/// The statement's own discount labels (D + E + F + G) — what the tax summary
/// assesses, and what label A is a memo of. Also what `reports::health` names a
/// duplicated statement by, so the two agree on what "the discount" is.
pub(crate) fn discount_labels(s: &EssStatement) -> Decimal {
    s.taxed_upfront_eligible
        + s.taxed_upfront_not_eligible
        + s.deferral_discount
        + s.pre_2009_cessation_discount
}
/// The same four labels in AUD from the statement-AUD overrides, when that
/// total is *knowable*: an override stands for its label, and an absent one is
/// only known to be nil when the label it overrides is itself nil. A label with
/// an amount but no override converts at the RBA rate, which is not resolvable
/// at write time — so the total is `None` and the AUD-side memo check is
/// skipped rather than guessed at.
fn aud_discount_labels(s: &EssStatement) -> Option<Decimal> {
    [
        (s.aud_taxed_upfront_eligible, s.taxed_upfront_eligible),
        (
            s.aud_taxed_upfront_not_eligible,
            s.taxed_upfront_not_eligible,
        ),
        (s.aud_deferral_discount, s.deferral_discount),
        (
            s.aud_pre_2009_cessation_discount,
            s.pre_2009_cessation_discount,
        ),
    ]
    .into_iter()
    .try_fold(
        Decimal::ZERO,
        |sum, (override_aud, label)| match override_aud {
            Some(aud) => Some(sum + aud),
            None if label.is_zero() => Some(sum),
            None => None,
        },
    )
}

/// What the statement may say about itself, decided from the row alone (the
/// currency and vest-freeze rules that need the database stay in
/// [`db_upsert`]). Every figure here reaches the tax summary and the printed
/// annual document, so a contradiction between them is refused at write time
/// rather than reported.
fn validate(s: &EssStatement) -> Result<(), UpsertError> {
    // Negatives first, so a negative figure gets the message naming its field
    // rather than failing one of the cross-checks below in a confusing way.
    for (field, value) in [
        ("quantity", Some(s.quantity)),
        ("market_value_per_share", Some(s.market_value_per_share)),
        ("taxed_upfront_eligible", Some(s.taxed_upfront_eligible)),
        (
            "taxed_upfront_not_eligible",
            Some(s.taxed_upfront_not_eligible),
        ),
        ("deferral_discount", Some(s.deferral_discount)),
        (
            "pre_2009_cessation_discount",
            Some(s.pre_2009_cessation_discount),
        ),
        ("foreign_source_discount", Some(s.foreign_source_discount)),
        ("tfn_withholding", Some(s.tfn_withholding)),
        ("aud_taxed_upfront_eligible", s.aud_taxed_upfront_eligible),
        (
            "aud_taxed_upfront_not_eligible",
            s.aud_taxed_upfront_not_eligible,
        ),
        ("aud_deferral_discount", s.aud_deferral_discount),
        (
            "aud_pre_2009_cessation_discount",
            s.aud_pre_2009_cessation_discount,
        ),
        ("aud_foreign_source_discount", s.aud_foreign_source_discount),
    ] {
        if value.is_some_and(|v| v < Decimal::ZERO) {
            return Err(UpsertError::NegativeAmount(field));
        }
    }

    // The vest Buy is dated the taxing point, and `trade::db_upsert` refuses a
    // pre-CGT trade — the vest writes its Buy directly, so without this the
    // statement is the one door a parcel can enter below the CGT floor by.
    if s.taxing_point_date < crate::entities::trade::CGT_START {
        return Err(UpsertError::PreCgtTaxingPoint);
    }

    // Label A is a memo *within* the discount labels, not an amount of its own
    // (see `EssStatement::foreign_source_discount`), so it cannot exceed them —
    // on the statement's own figures, and on the statement-AUD overrides where
    // those pin the same total in AUD.
    let discounts = discount_labels(s);
    if s.foreign_source_discount > discounts {
        return Err(UpsertError::ForeignSourceExceedsDiscounts {
            label: "foreign_source_discount",
            foreign: s.foreign_source_discount,
            discounts,
        });
    }
    if let (Some(aud_foreign), Some(aud_discounts)) =
        (s.aud_foreign_source_discount, aud_discount_labels(s))
        && aud_foreign > aud_discounts
    {
        return Err(UpsertError::ForeignSourceExceedsDiscounts {
            label: "aud_foreign_source_discount",
            foreign: aud_foreign,
            discounts: aud_discounts,
        });
    }

    // The discount is the market value less what the employee paid, so it can
    // at most equal the market value of the shares that vest (an RSU acquired
    // for nil consideration — the equality case, which must stay accepted).
    // Both sides round to the cent, since a per-share market value can carry
    // sub-cent precision while the discount on the statement is cents. Only
    // when both figures are positive: an income-only statement (no vest
    // recorded) leaves them zero and is legitimate.
    if s.quantity > Decimal::ZERO && s.market_value_per_share > Decimal::ZERO {
        let market_value = s.quantity * s.market_value_per_share;
        if to_cents(discounts) > to_cents(market_value) {
            return Err(UpsertError::DiscountExceedsMarketValue {
                discount: discounts,
                market_value,
            });
        }
    }
    Ok(())
}

pub async fn db_upsert(pool: &SqlitePool, s: &EssStatement) -> Result<(), UpsertError> {
    validate(s)?;

    // A statement-AUD override restates a label the statement already gives in
    // AUD — reject the contradiction before touching the row.
    let has_override = s.aud_taxed_upfront_eligible.is_some()
        || s.aud_taxed_upfront_not_eligible.is_some()
        || s.aud_deferral_discount.is_some()
        || s.aud_pre_2009_cessation_discount.is_some()
        || s.aud_foreign_source_discount.is_some();
    if has_override && s.currency == "AUD" {
        return Err(UpsertError::AudOverrideOnAudStatement);
    }

    // The stated rate divides the statement's amounts, and an AUD statement
    // never converts — reject both nonsense forms before touching the row.
    if let Some(rate) = s.fx_rate {
        if rate <= Decimal::ZERO {
            return Err(UpsertError::FxRateNotPositive);
        }
        if s.currency == "AUD" {
            return Err(UpsertError::FxRateOnAudStatement);
        }
    }

    let mut tx = pool.begin().await?;

    // The statement's currency must be the listing's: `market_value_per_share`
    // is the market value of *that listed share*, so the price and the listed
    // price are one money. Otherwise the vest copies the statement's currency
    // onto a parcel of a security priced in another, and a later closing price
    // (from the exchange, in the listing's currency) values it — mixing
    // currencies in one calculation, which CLAUDE.md forbids. The same
    // argument the DRP reinvest path makes about a distribution's cash.
    // (An unknown listing_id falls through to the foreign-key rejection.)
    let listing_currency: Option<String> =
        sqlx::query_scalar("SELECT currency FROM listings WHERE id = ?")
            .bind(s.listing_id)
            .fetch_optional(&mut *tx)
            .await?;
    if let Some(listing) = listing_currency
        && listing != s.currency
    {
        return Err(UpsertError::CurrencyNotListings {
            statement: s.currency.clone(),
            listing,
        });
    }

    // While its vest exists, the fields the Buy was created from (listing,
    // account, taxing point, quantity, market value, currency, FX rate) are frozen —
    // editing them would desync the Buy. The income side (discount labels, TFN
    // withheld, statement-AUD overrides) stays editable: the employer's annual
    // ESS statement arrives after the vest is recorded. (A new id has no vest,
    // so an insert always passes.)
    let existing: Option<EssStatement> = {
        let vested: bool =
            sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM trades WHERE ess_statement_id = ?)")
                .bind(s.id)
                .fetch_one(&mut *tx)
                .await?;
        if vested {
            sqlx::query_as(sqlx::AssertSqlSafe(format!(
                "SELECT {COLUMNS} FROM ess_statements WHERE id = ?"
            )))
            .bind(s.id)
            .fetch_optional(&mut *tx)
            .await?
        } else {
            None
        }
    };
    if let Some(old) = existing {
        let vest_side_unchanged = old.listing_id == s.listing_id
            && old.holding_account_id == s.holding_account_id
            && old.taxing_point_date == s.taxing_point_date
            && old.quantity == s.quantity
            && old.market_value_per_share == s.market_value_per_share
            && old.currency == s.currency
            && old.fx_rate == s.fx_rate;
        if !vest_side_unchanged {
            return Err(UpsertError::Vested);
        }
    }

    sqlx::query(
        "INSERT INTO ess_statements \
         (id, listing_id, holding_account_id, taxing_point_date, quantity, \
          market_value_per_share, taxed_upfront_eligible, taxed_upfront_not_eligible, \
          deferral_discount, pre_2009_cessation_discount, foreign_source_discount, \
          tfn_withholding, currency, fx_rate, aud_taxed_upfront_eligible, \
          aud_taxed_upfront_not_eligible, aud_deferral_discount, \
          aud_pre_2009_cessation_discount, aud_foreign_source_discount) \
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?) \
         ON CONFLICT(id) DO UPDATE SET \
             listing_id                      = excluded.listing_id, \
             holding_account_id              = excluded.holding_account_id, \
             taxing_point_date               = excluded.taxing_point_date, \
             quantity                        = excluded.quantity, \
             market_value_per_share          = excluded.market_value_per_share, \
             taxed_upfront_eligible          = excluded.taxed_upfront_eligible, \
             taxed_upfront_not_eligible      = excluded.taxed_upfront_not_eligible, \
             deferral_discount               = excluded.deferral_discount, \
             pre_2009_cessation_discount     = excluded.pre_2009_cessation_discount, \
             foreign_source_discount         = excluded.foreign_source_discount, \
             tfn_withholding                 = excluded.tfn_withholding, \
             currency                        = excluded.currency, \
             fx_rate                         = excluded.fx_rate, \
             aud_taxed_upfront_eligible      = excluded.aud_taxed_upfront_eligible, \
             aud_taxed_upfront_not_eligible  = excluded.aud_taxed_upfront_not_eligible, \
             aud_deferral_discount           = excluded.aud_deferral_discount, \
             aud_pre_2009_cessation_discount = excluded.aud_pre_2009_cessation_discount, \
             aud_foreign_source_discount     = excluded.aud_foreign_source_discount",
    )
    .bind(s.id)
    .bind(s.listing_id)
    .bind(s.holding_account_id)
    .bind(s.taxing_point_date)
    .bind(Money(s.quantity))
    .bind(Money(s.market_value_per_share))
    .bind(Money(s.taxed_upfront_eligible))
    .bind(Money(s.taxed_upfront_not_eligible))
    .bind(Money(s.deferral_discount))
    .bind(Money(s.pre_2009_cessation_discount))
    .bind(Money(s.foreign_source_discount))
    .bind(Money(s.tfn_withholding))
    .bind(&s.currency)
    .bind(OptMoney(s.fx_rate))
    .bind(OptMoney(s.aud_taxed_upfront_eligible))
    .bind(OptMoney(s.aud_taxed_upfront_not_eligible))
    .bind(OptMoney(s.aud_deferral_discount))
    .bind(OptMoney(s.aud_pre_2009_cessation_discount))
    .bind(OptMoney(s.aud_foreign_source_discount))
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
    /// The statement has a vest Buy that is drawn on by a Sell allocation or an
    /// AMIT adjustment — removing the statement would have to remove that Buy,
    /// orphaning those dependants. Remove them first. Mapped to 422.
    VestDrawnOn,
}

/// Delete the statement and, if it was vested, its cost-base-reset Buy — in one
/// transaction. Refused while the vest Buy is drawn on.
pub async fn db_delete(pool: &SqlitePool, id: i64) -> Result<DeleteOutcome, sqlx::Error> {
    let mut tx = pool.begin().await?;

    let exists: bool =
        sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM ess_statements WHERE id = ?)")
            .bind(id)
            .fetch_one(&mut *tx)
            .await?;
    if !exists {
        return Ok(DeleteOutcome::NotFound);
    }

    // The linked vest Buy, if any. It is never deleted individually, so this is
    // the only path that removes it.
    let vest_id: Option<i64> =
        sqlx::query_scalar("SELECT id FROM trades WHERE ess_statement_id = ?")
            .bind(id)
            .fetch_optional(&mut *tx)
            .await?;
    if let Some(vest_id) = vest_id {
        let drawn_on: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM parcel_allocations \
                           WHERE purchase_trade_id = ?1 OR sale_trade_id = ?1) \
                 OR EXISTS(SELECT 1 FROM amit_adjustments WHERE trade_id = ?1)",
        )
        .bind(vest_id)
        .fetch_one(&mut *tx)
        .await?;
        if drawn_on {
            return Ok(DeleteOutcome::VestDrawnOn);
        }
        sqlx::query("DELETE FROM trades WHERE id = ?")
            .bind(vest_id)
            .execute(&mut *tx)
            .await?;
    }

    sqlx::query("DELETE FROM ess_statements WHERE id = ?")
        .bind(id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(DeleteOutcome::Deleted)
}

async fn upsert(
    State(pool): State<SqlitePool>,
    Path(id): Path<i64>,
    Json(body): Json<EssStatementBody>,
) -> Result<StatusCode, ApiError> {
    let s = EssStatement {
        id,
        listing_id: body.listing_id,
        holding_account_id: body.holding_account_id,
        taxing_point_date: body.taxing_point_date,
        quantity: body.quantity,
        market_value_per_share: body.market_value_per_share,
        taxed_upfront_eligible: body.taxed_upfront_eligible,
        taxed_upfront_not_eligible: body.taxed_upfront_not_eligible,
        deferral_discount: body.deferral_discount,
        pre_2009_cessation_discount: body.pre_2009_cessation_discount,
        foreign_source_discount: body.foreign_source_discount,
        tfn_withholding: body.tfn_withholding,
        currency: body.currency,
        fx_rate: body.fx_rate,
        aud_taxed_upfront_eligible: body.aud_taxed_upfront_eligible,
        aud_taxed_upfront_not_eligible: body.aud_taxed_upfront_not_eligible,
        aud_deferral_discount: body.aud_deferral_discount,
        aud_pre_2009_cessation_discount: body.aud_pre_2009_cessation_discount,
        aud_foreign_source_discount: body.aud_foreign_source_discount,
        vest_trade_id: None, // derived on read; never written
    };
    db_upsert(&pool, &s).await?;
    Ok(StatusCode::NO_CONTENT)
}

impl From<UpsertError> for ApiError {
    fn from(e: UpsertError) -> Self {
        match e {
            UpsertError::Vested => ApiError::unprocessable(
                "this ESS statement has been vested: the fields its vest Buy was created \
                 from (listing, account, taxing point, quantity, market value, currency, \
                 fx_rate) cannot change — delete the statement (which removes the vest \
                 Buy) and re-enter instead; the discount labels and statement-AUD \
                 overrides stay editable",
            ),
            UpsertError::AudOverrideOnAudStatement => ApiError::unprocessable(
                "statement-AUD override amounts are only accepted on a non-AUD statement \
                 — an AUD statement's discount labels are already the AUD figures",
            ),
            UpsertError::FxRateNotPositive => ApiError::unprocessable(
                "the statement's fx_rate must be a positive foreign-per-AUD rate — it \
                 divides the statement's amounts (AUD = foreign / rate)",
            ),
            UpsertError::FxRateOnAudStatement => ApiError::unprocessable(
                "an fx_rate is only accepted on a non-AUD statement — an AUD amount \
                 never converts",
            ),
            UpsertError::CurrencyNotListings { statement, listing } => {
                ApiError::unprocessable(format!(
                    "this ESS statement is recorded in {statement} but its listing is quoted \
                 in {listing} — the per-share market value and the listed price are the \
                 same money, so enter the statement in {listing} (an employer statement \
                 in another currency is converted before entry, or the wrong listing was \
                 chosen)"
                ))
            }
            UpsertError::NegativeAmount(field) => ApiError::unprocessable(format!(
                "{field} cannot be negative — an Employee share scheme statement reports the \
                 employer's positive (or zero) figures; a negative discount label nets against \
                 the year's other statements, a negative TFN amount withheld is a refund from \
                 nowhere, and a negative quantity or market value is not a parcel"
            )),
            UpsertError::PreCgtTaxingPoint => ApiError::unprocessable(
                "the taxing point is dated before 20 September 1985 — a pre-CGT holding is \
                 outside CGT and not modelled, and the vest Buy this statement creates would \
                 be refused for the same reason; no ESS interest can predate CGT in any case \
                 (Division 83A dates from 2009 and its predecessor from 1995), so check the \
                 date for a typo",
            ),
            UpsertError::ForeignSourceExceedsDiscounts {
                label,
                foreign,
                discounts,
            } => ApiError::unprocessable(format!(
                "{label} {foreign} cannot exceed the {discounts} of discount it is a memo of \
                 — the foreign-source figure (label A) is the foreign-sourced *portion* of \
                 the discount labels D + E + F + G, recorded within them rather than in \
                 addition to them (the tax summary surfaces it for the foreign income tax \
                 offset, never adds it on top); enter the discount labels in full, the \
                 foreign-source part included"
            )),
            UpsertError::DiscountExceedsMarketValue {
                discount,
                market_value,
            } => ApiError::unprocessable(format!(
                "the discount labels total {discount}, above the {market_value} market value \
                 of the shares that vest (quantity × market_value_per_share) — the discount \
                 is the market value less what the employee paid, so it can at most equal it \
                 (an RSU acquired for nil consideration); check for a transposed column, or a \
                 discount in another currency against this statement's market value"
            )),
            UpsertError::Db(err) => err.into(),
        }
    }
}

async fn delete(
    State(pool): State<SqlitePool>,
    Path(id): Path<i64>,
) -> Result<StatusCode, ApiError> {
    match db_delete(&pool, id).await? {
        DeleteOutcome::Deleted => Ok(StatusCode::NO_CONTENT),
        DeleteOutcome::NotFound => Err(ApiError::not_found("no ESS statement with that id")),
        DeleteOutcome::VestDrawnOn => Err(ApiError::unprocessable(
            "this ESS statement's vest Buy is drawn on by a sale allocation or AMIT \
             adjustment — remove those first",
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::entities::listing;
    use crate::test_support::{self, ApiClient, test_pool, ymd};

    /// Client over this module's own routes.
    fn client(pool: &SqlitePool) -> ApiClient {
        ApiClient::over(router().with_state(pool.clone()))
    }

    async fn insert_listing(pool: &SqlitePool, id: i64) {
        insert_listing_in(pool, id, "AUD").await;
    }

    /// A listing quoted in `currency` — a statement is entered in its
    /// listing's currency, so the non-AUD cases need one to hang off.
    async fn insert_listing_in(pool: &SqlitePool, id: i64, currency: &str) {
        test_support::listing(id)
            .ticker(&format!("ESS{id}"))
            .name(&format!("ESS {id}"))
            .security_type(listing::SecurityType::Share)
            .currency(currency)
            .insert(pool)
            .await;
    }

    fn sample(id: i64) -> EssStatement {
        test_support::ess_statement(id, 1, ymd(2024, 9, 1))
            .with(|s| {
                s.quantity = Decimal::from(100);
                s.market_value_per_share = Decimal::from(6);
                s.deferral_discount = Decimal::from(600);
            })
            .build()
    }

    #[tokio::test]
    async fn db_round_trips_with_decimal_precision() {
        let pool = test_pool().await;
        insert_listing(&pool, 1).await;
        let mut s = sample(1);
        s.market_value_per_share = "6.123456789".parse().unwrap();
        s.deferral_discount = "612.345678900".parse().unwrap();
        db_upsert(&pool, &s).await.unwrap();
        let got = db_get(&pool, 1).await.unwrap().unwrap();
        assert_eq!(
            got.market_value_per_share,
            "6.123456789".parse::<Decimal>().unwrap()
        );
        assert_eq!(
            got.deferral_discount,
            "612.345678900".parse::<Decimal>().unwrap()
        );
        assert_eq!(
            got.taxing_point_date,
            NaiveDate::from_ymd_opt(2024, 9, 1).unwrap()
        );
    }

    /// Statement-AUD overrides round-trip with precision and absent ones stay
    /// NULL/None.
    #[tokio::test]
    async fn db_round_trips_statement_aud_overrides() {
        let pool = test_pool().await;
        insert_listing_in(&pool, 1, "USD").await;
        let mut s = sample(1);
        s.currency = "USD".to_string();
        s.aud_deferral_discount = Some("10572.45".parse().unwrap());
        db_upsert(&pool, &s).await.unwrap();
        let got = db_get(&pool, 1).await.unwrap().unwrap();
        assert_eq!(
            got.aud_deferral_discount,
            Some("10572.45".parse::<Decimal>().unwrap())
        );
        assert_eq!(got.aud_taxed_upfront_eligible, None);
        assert_eq!(got.aud_foreign_source_discount, None);
    }

    /// An override on a statement already denominated in AUD is rejected — two
    /// AUD figures for the same label could silently disagree.
    #[tokio::test]
    async fn db_aud_override_on_aud_statement_rejected() {
        let pool = test_pool().await;
        insert_listing(&pool, 1).await;
        let mut s = sample(1); // currency AUD
        s.aud_deferral_discount = Some(Decimal::from(600));
        assert!(matches!(
            db_upsert(&pool, &s).await,
            Err(UpsertError::AudOverrideOnAudStatement)
        ));
        assert!(db_get(&pool, 1).await.unwrap().is_none());
    }

    /// `vest_trade_id` is derived from the vest Buy's back-link: `None` while
    /// unvested, the Buy's id once vested — so the UI can gate the Vest action.
    #[tokio::test]
    async fn vest_trade_id_reflects_the_vest_buy() {
        let pool = test_pool().await;
        insert_listing(&pool, 1).await;
        db_upsert(&pool, &sample(1)).await.unwrap();
        assert_eq!(db_get(&pool, 1).await.unwrap().unwrap().vest_trade_id, None);

        let vest = crate::entities::ess_vest::db_vest(&pool, 1).await.unwrap();
        assert_eq!(
            db_get(&pool, 1).await.unwrap().unwrap().vest_trade_id,
            Some(vest.id)
        );
        let listed = db_list(&pool).await.unwrap();
        assert_eq!(listed[0].vest_trade_id, Some(vest.id));

        // The field is present in the JSON the list endpoint serves (the UI
        // reads it straight off the row).
        let resp = client(&pool).get("/ess_statements").await;
        let items: serde_json::Value = resp.json();
        assert_eq!(items[0]["vest_trade_id"], serde_json::json!(vest.id));
    }

    /// SCENARIOS J-08: a statement's currency must be its listing's. The
    /// per-share market value is the market value of *that listed share*, so
    /// the two are the same money — the I-06/I-08 argument the DRP reinvest
    /// path already makes about a distribution's cash. Otherwise the vest
    /// copies the statement's currency onto a parcel of an AUD-priced
    /// security, and the AUD closing price then values a USD-costed parcel.
    #[tokio::test]
    async fn a_statement_in_another_currency_than_its_listing_is_refused() {
        let pool = test_pool().await;
        insert_listing(&pool, 1).await; // AUD
        let mut s = sample(1);
        s.currency = "USD".to_string();
        assert!(matches!(
            db_upsert(&pool, &s).await,
            Err(UpsertError::CurrencyNotListings { ref statement, ref listing })
                if statement == "USD" && listing == "AUD"
        ));
        assert!(db_get(&pool, 1).await.unwrap().is_none());

        // The matching case is untouched: on a USD listing the same statement
        // is accepted.
        insert_listing_in(&pool, 2, "USD").await;
        s.listing_id = 2;
        db_upsert(&pool, &s).await.unwrap();
        assert_eq!(db_get(&pool, 1).await.unwrap().unwrap().currency, "USD");
    }

    #[tokio::test]
    async fn api_currency_not_the_listings_rejected_422_naming_both() {
        let pool = test_pool().await;
        insert_listing(&pool, 1).await; // AUD
        let body = serde_json::json!({
            "listing_id": 1,
            "taxing_point_date": "2024-09-01",
            "currency": "USD",
        });
        let (status, text) = {
            let resp = client(&pool).put("/ess_statements/1", &body).await;
            (resp.status, resp.text().to_string())
        };
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
        assert!(text.contains("USD") && text.contains("AUD"), "{text}");
    }

    /// The stated rate divides the statement's amounts, so zero or negative is
    /// not a rate; and an AUD statement never converts, so a rate on one is a
    /// figure that could never apply.
    #[tokio::test]
    async fn db_fx_rate_must_be_positive_and_only_on_a_non_aud_statement() {
        let pool = test_pool().await;
        insert_listing_in(&pool, 1, "USD").await;
        let mut s = sample(1);
        s.currency = "USD".to_string();
        s.fx_rate = Some(Decimal::ZERO);
        assert!(matches!(
            db_upsert(&pool, &s).await,
            Err(UpsertError::FxRateNotPositive)
        ));
        s.fx_rate = Some(Decimal::from(-1));
        assert!(matches!(
            db_upsert(&pool, &s).await,
            Err(UpsertError::FxRateNotPositive)
        ));

        // A positive rate on the non-AUD statement round-trips…
        s.fx_rate = Some("0.6543210987".parse().unwrap());
        db_upsert(&pool, &s).await.unwrap();
        assert_eq!(
            db_get(&pool, 1).await.unwrap().unwrap().fx_rate,
            Some("0.6543210987".parse::<Decimal>().unwrap())
        );

        // …but the same rate on an AUD statement is refused.
        insert_listing(&pool, 2).await; // AUD
        let mut aud = sample(2);
        aud.listing_id = 2;
        aud.fx_rate = Some(Decimal::from(1));
        assert!(matches!(
            db_upsert(&pool, &aud).await,
            Err(UpsertError::FxRateOnAudStatement)
        ));
    }

    /// The stated rate is a vest-side field — the vest Buy carries it — so it
    /// freezes with the rest of them once the statement is vested.
    #[tokio::test]
    async fn a_vested_statements_fx_rate_is_frozen() {
        let pool = test_pool().await;
        insert_listing_in(&pool, 1, "USD").await;
        let mut s = sample(1);
        s.currency = "USD".to_string();
        s.fx_rate = Some("0.65".parse().unwrap());
        db_upsert(&pool, &s).await.unwrap();
        crate::entities::ess_vest::db_vest(&pool, 1).await.unwrap();

        s.fx_rate = Some("0.70".parse().unwrap());
        assert!(matches!(
            db_upsert(&pool, &s).await,
            Err(UpsertError::Vested)
        ));
    }

    #[tokio::test]
    async fn api_fx_rate_on_an_aud_statement_rejected_422() {
        let pool = test_pool().await;
        insert_listing(&pool, 1).await;
        let body = serde_json::json!({
            "listing_id": 1,
            "taxing_point_date": "2024-09-01",
            "currency": "AUD",
            "fx_rate": "0.65",
        });
        let resp = client(&pool).put("/ess_statements/1", &body).await;
        assert_eq!(resp.status, StatusCode::UNPROCESSABLE_ENTITY);
    }

    /// SCENARIOS J-01/J-09: no figure on a statement may be negative — an
    /// employer's statement reports positive (or zero) amounts. A negative
    /// discount label silently nets against the year's other statements, a
    /// negative TFN amount withheld reports as withholding refunded from
    /// nowhere, and a negative quantity or market value describes a parcel that
    /// cannot exist. Each is refused 422 naming the field, and nothing is
    /// persisted.
    #[tokio::test]
    async fn db_negative_amounts_are_refused_naming_the_field() {
        let pool = test_pool().await;
        insert_listing_in(&pool, 1, "USD").await; // so the AUD overrides apply

        /// Puts the given amount in one field, so the sweep below can name
        /// every amount on the row once.
        type SetAmount = fn(&mut EssStatement, Decimal);

        let negative = Decimal::from(-50);
        let cases: [(&str, SetAmount); 13] = [
            ("quantity", |s, v| s.quantity = v),
            ("market_value_per_share", |s, v| {
                s.market_value_per_share = v
            }),
            ("taxed_upfront_eligible", |s, v| {
                s.taxed_upfront_eligible = v
            }),
            ("taxed_upfront_not_eligible", |s, v| {
                s.taxed_upfront_not_eligible = v
            }),
            ("deferral_discount", |s, v| s.deferral_discount = v),
            ("pre_2009_cessation_discount", |s, v| {
                s.pre_2009_cessation_discount = v
            }),
            ("foreign_source_discount", |s, v| {
                s.foreign_source_discount = v
            }),
            ("tfn_withholding", |s, v| s.tfn_withholding = v),
            ("aud_taxed_upfront_eligible", |s, v| {
                s.aud_taxed_upfront_eligible = Some(v)
            }),
            ("aud_taxed_upfront_not_eligible", |s, v| {
                s.aud_taxed_upfront_not_eligible = Some(v)
            }),
            ("aud_deferral_discount", |s, v| {
                s.aud_deferral_discount = Some(v)
            }),
            ("aud_pre_2009_cessation_discount", |s, v| {
                s.aud_pre_2009_cessation_discount = Some(v)
            }),
            ("aud_foreign_source_discount", |s, v| {
                s.aud_foreign_source_discount = Some(v)
            }),
        ];
        for (field, set) in cases {
            let mut s = sample(1);
            s.currency = "USD".to_string();
            set(&mut s, negative);
            assert!(
                matches!(db_upsert(&pool, &s).await, Err(UpsertError::NegativeAmount(f)) if f == field),
                "a negative {field} must be refused naming it"
            );
            assert!(db_get(&pool, 1).await.unwrap().is_none());
        }
    }

    /// The 422 body names the offending field, so the web UI can say which one.
    #[tokio::test]
    async fn api_negative_tfn_withholding_rejected_422() {
        let pool = test_pool().await;
        insert_listing(&pool, 1).await;
        let body = serde_json::json!({
            "listing_id": 1,
            "taxing_point_date": "2024-09-01",
            "deferral_discount": "-1000",
            "tfn_withholding": "-50",
        });
        let resp = client(&pool).put("/ess_statements/1", &body).await;
        assert_eq!(resp.status, StatusCode::UNPROCESSABLE_ENTITY);
        let text = resp.text().to_string();
        assert!(text.contains("deferral_discount"), "{text}");
    }

    /// SCENARIOS J-13: a taxing point before the start of CGT is refused. The
    /// vest writes its Buy with a raw INSERT, so without this the statement is
    /// the one door a parcel can enter below the CGT floor by — `PUT /trades`
    /// refuses precisely the Buy the vest would create. 20 September 1985
    /// itself is on the CGT side of the line and stays acceptable.
    #[tokio::test]
    async fn db_a_pre_cgt_taxing_point_is_refused_and_the_cutoff_day_accepted() {
        let pool = test_pool().await;
        insert_listing(&pool, 1).await;

        let mut s = sample(1);
        s.taxing_point_date = ymd(1985, 9, 19);
        assert!(matches!(
            db_upsert(&pool, &s).await,
            Err(UpsertError::PreCgtTaxingPoint)
        ));
        assert!(db_get(&pool, 1).await.unwrap().is_none());

        s.taxing_point_date = ymd(1985, 9, 20);
        db_upsert(&pool, &s).await.unwrap();
        assert_eq!(
            db_get(&pool, 1).await.unwrap().unwrap().taxing_point_date,
            ymd(1985, 9, 20)
        );
    }

    /// The refusal reaches the API as a 422 whose body says why, and no vest
    /// Buy can follow — the statement never exists to vest.
    #[tokio::test]
    async fn api_pre_cgt_taxing_point_rejected_422() {
        let pool = test_pool().await;
        insert_listing(&pool, 1).await;
        let body = serde_json::json!({
            "listing_id": 1,
            "taxing_point_date": "1985-01-01",
            "quantity": "100",
            "market_value_per_share": "10",
            "deferral_discount": "1000",
        });
        let resp = client(&pool).put("/ess_statements/1", &body).await;
        assert_eq!(resp.status, StatusCode::UNPROCESSABLE_ENTITY);
        let text = resp.text().to_string();
        assert!(text.contains("20 September 1985"), "{text}");
        assert!(db_get(&pool, 1).await.unwrap().is_none());
    }

    /// SCENARIOS J-11: label A is a memo *within* labels D + E + F + G, not an
    /// amount of its own, so a memo larger than what it is a memo of is a
    /// contradiction — the CFI-within-unfranked shape the income entity already
    /// enforces. Equality is the ordinary case (a wholly foreign-sourced
    /// discount) and stays accepted.
    #[tokio::test]
    async fn db_the_foreign_source_memo_cannot_exceed_the_discounts_it_memos() {
        let pool = test_pool().await;
        insert_listing(&pool, 1).await;

        let mut s = sample(1); // deferral_discount 600
        s.foreign_source_discount = Decimal::from(5000);
        assert!(matches!(
            db_upsert(&pool, &s).await,
            Err(UpsertError::ForeignSourceExceedsDiscounts { label, foreign, discounts })
                if label == "foreign_source_discount"
                    && foreign == Decimal::from(5000)
                    && discounts == Decimal::from(600)
        ));
        assert!(db_get(&pool, 1).await.unwrap().is_none());

        // The whole discount being foreign-sourced is ordinary.
        s.foreign_source_discount = Decimal::from(600);
        db_upsert(&pool, &s).await.unwrap();
    }

    /// The same rule on the statement-AUD overrides, which the tax summary
    /// reports verbatim: the label-A override cannot exceed the override total
    /// it is a memo of. Only checked when that total is knowable — a label with
    /// an amount but no override converts at the RBA rate, which no write-time
    /// check can resolve, so such a statement is left alone rather than guessed
    /// at.
    #[tokio::test]
    async fn db_the_aud_foreign_source_memo_is_checked_only_when_the_total_is_known() {
        let pool = test_pool().await;
        insert_listing_in(&pool, 1, "USD").await;

        let mut s = sample(1); // deferral_discount 600, the only non-zero label
        s.currency = "USD".to_string();
        s.aud_deferral_discount = Some(Decimal::from(900));
        s.foreign_source_discount = Decimal::from(600);
        s.aud_foreign_source_discount = Some(Decimal::from(1200));
        assert!(matches!(
            db_upsert(&pool, &s).await,
            Err(UpsertError::ForeignSourceExceedsDiscounts { label, foreign, discounts })
                if label == "aud_foreign_source_discount"
                    && foreign == Decimal::from(1200)
                    && discounts == Decimal::from(900)
        ));

        // Within the override total: accepted.
        s.aud_foreign_source_discount = Some(Decimal::from(900));
        db_upsert(&pool, &s).await.unwrap();

        // With no override on the label carrying the discount, the AUD total is
        // not knowable at write time (it converts at the RBA rate), so the memo
        // override passes unchecked rather than being compared against nothing.
        let mut unknowable = sample(2);
        unknowable.currency = "USD".to_string();
        unknowable.foreign_source_discount = Decimal::from(600);
        unknowable.aud_deferral_discount = None;
        unknowable.aud_foreign_source_discount = Some(Decimal::from(1200));
        db_upsert(&pool, &unknowable).await.unwrap();
    }

    /// SCENARIOS J-01: the discount *is* the market value less what the
    /// employee paid, so a discount above the market value of the shares that
    /// vest implies a negative payment — the shape a transposed column or a
    /// foreign-currency discount against an AUD market value makes. Exact
    /// equality is the RSU case (nil consideration) and must stay accepted.
    #[tokio::test]
    async fn db_the_discount_cannot_exceed_the_market_value_that_vests() {
        let pool = test_pool().await;
        insert_listing(&pool, 1).await;

        // 100 shares at $10 = $1,000 of market value, against a $15,000 label.
        let mut s = sample(1);
        s.quantity = Decimal::from(100);
        s.market_value_per_share = Decimal::from(10);
        s.deferral_discount = Decimal::from(15000);
        assert!(matches!(
            db_upsert(&pool, &s).await,
            Err(UpsertError::DiscountExceedsMarketValue { discount, market_value })
                if discount == Decimal::from(15000) && market_value == Decimal::from(1000)
        ));
        assert!(db_get(&pool, 1).await.unwrap().is_none());

        // Nil consideration: the discount is the whole market value.
        s.deferral_discount = Decimal::from(1000);
        db_upsert(&pool, &s).await.unwrap();

        // A per-share value carrying sub-cent precision still equals its own
        // total once both sides round to the cent (400 × 3.795 = 1518).
        let mut wyatt = sample(2);
        wyatt.quantity = Decimal::from(400);
        wyatt.market_value_per_share = "3.795".parse().unwrap();
        wyatt.deferral_discount = Decimal::from(1518);
        db_upsert(&pool, &wyatt).await.unwrap();
    }

    /// An income-only statement — a discount declared with no vest recorded
    /// against it — leaves quantity and market value zero, which is legitimate:
    /// the cross-check only applies when both are positive.
    #[tokio::test]
    async fn db_an_income_only_statement_keeps_its_discount() {
        let pool = test_pool().await;
        insert_listing(&pool, 1).await;
        let mut s = sample(1);
        s.quantity = Decimal::ZERO;
        s.market_value_per_share = Decimal::ZERO;
        s.deferral_discount = Decimal::from(5000);
        db_upsert(&pool, &s).await.unwrap();
        assert_eq!(
            db_get(&pool, 1).await.unwrap().unwrap().deferral_discount,
            Decimal::from(5000)
        );
    }

    #[tokio::test]
    async fn api_discount_above_the_market_value_rejected_422() {
        let pool = test_pool().await;
        insert_listing(&pool, 1).await;
        let body = serde_json::json!({
            "listing_id": 1,
            "taxing_point_date": "2024-09-01",
            "quantity": "100",
            "market_value_per_share": "10",
            "deferral_discount": "15000",
        });
        let resp = client(&pool).put("/ess_statements/1", &body).await;
        assert_eq!(resp.status, StatusCode::UNPROCESSABLE_ENTITY);
        let text = resp.text().to_string();
        assert!(text.contains("15000") && text.contains("1000"), "{text}");
    }

    #[tokio::test]
    async fn db_get_missing_returns_none() {
        let pool = test_pool().await;
        assert!(db_get(&pool, 99).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn db_delete_without_vest_just_removes_the_statement() {
        let pool = test_pool().await;
        insert_listing(&pool, 1).await;
        db_upsert(&pool, &sample(1)).await.unwrap();
        assert_eq!(db_delete(&pool, 1).await.unwrap(), DeleteOutcome::Deleted);
        assert!(db_get(&pool, 1).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn db_delete_missing_is_not_found() {
        let pool = test_pool().await;
        assert_eq!(db_delete(&pool, 99).await.unwrap(), DeleteOutcome::NotFound);
    }

    #[tokio::test]
    async fn api_upsert_unknown_currency_rejected_422() {
        let pool = test_pool().await;
        insert_listing(&pool, 1).await;
        let body = serde_json::json!({
            "listing_id": 1,
            "taxing_point_date": "2024-09-01",
            "currency": "ZZZ"
        });
        let resp = client(&pool).put("/ess_statements/1", &body).await;
        assert_eq!(resp.status, StatusCode::UNPROCESSABLE_ENTITY);
    }

    #[tokio::test]
    async fn api_aud_override_on_aud_statement_rejected_422() {
        let pool = test_pool().await;
        insert_listing(&pool, 1).await;
        let body = serde_json::json!({
            "listing_id": 1,
            "taxing_point_date": "2024-09-01",
            "currency": "AUD",
            "aud_deferral_discount": "600"
        });
        let resp = client(&pool).put("/ess_statements/1", &body).await;
        assert_eq!(resp.status, StatusCode::UNPROCESSABLE_ENTITY);
    }

    #[tokio::test]
    async fn api_unknown_listing_rejected_422() {
        let pool = test_pool().await;
        let body = serde_json::json!({ "listing_id": 999, "taxing_point_date": "2024-09-01" });
        let resp = client(&pool).put("/ess_statements/1", &body).await;
        assert_eq!(resp.status, StatusCode::UNPROCESSABLE_ENTITY);
    }

    #[tokio::test]
    async fn api_list_and_get() {
        let pool = test_pool().await;
        insert_listing(&pool, 1).await;
        db_upsert(&pool, &sample(1)).await.unwrap();
        let resp = client(&pool).get("/ess_statements").await;
        assert_eq!(resp.status, StatusCode::OK);
        let items: Vec<EssStatement> = resp.json();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].deferral_discount, Decimal::from(600));
    }

    #[tokio::test]
    async fn api_delete_missing_returns_404() {
        let pool = test_pool().await;
        let resp = client(&pool).delete("/ess_statements/99").await;
        assert_eq!(resp.status, StatusCode::NOT_FOUND);
    }
}
