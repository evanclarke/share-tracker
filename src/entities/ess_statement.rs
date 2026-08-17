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
}

pub async fn db_upsert(pool: &SqlitePool, s: &EssStatement) -> Result<(), UpsertError> {
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
