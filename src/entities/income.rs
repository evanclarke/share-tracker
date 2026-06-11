use crate::infra::decimal::{row_dec, row_opt_dec};
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
use sqlx::{Row, SqlitePool};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Income {
    pub id: i64,
    pub listing_id: i64,
    pub date_paid: NaiveDate,
    pub ex_date: Option<NaiveDate>,
    pub franked_amount: Decimal,
    pub unfranked_amount: Decimal,
    pub foreign_source_income: Decimal,
    pub foreign_tax_paid: Decimal,
    pub tfn_withholding_tax: Decimal,
    pub franking_credits: Decimal,
    pub lic_capital_gain_deduction: Decimal,
    pub conduit_foreign_income: Decimal,
    pub trust_income: bool,
    /// Trust distributions only: the date the holder became presently entitled
    /// (in practice the distribution period's end, printed on the statement).
    /// Trust income is assessed in the year of present entitlement regardless
    /// of when it is paid (ATO QC 23087, `docs/ato/trust-income-timing.md`),
    /// so when set the tax summary attributes the row by this date instead of
    /// `date_paid`. Rejected (`422`) on a non-trust row — a dividend is always
    /// assessed by payment.
    pub entitlement_date: Option<NaiveDate>,
    pub reinvestment_trade_id: Option<i64>,
    /// ISO 4217 currency the amounts are denominated in. The tax summary converts
    /// non-AUD amounts to AUD via the ATO rate for this currency and the month of
    /// the assessment date — `date_paid`, or `entitlement_date` when that governs
    /// a trust row (see `infra::fx::to_aud`). Defaults to AUD.
    pub currency: String,
    /// Provenance link from a buy-back dividend-component row to the buy-back
    /// Sell trade it was created with (`None` for every other row). Set only
    /// by `POST /corporate_actions/:id/participate`
    /// (`entities::buyback_participation`). A row carrying it is managed by
    /// the participation: `PUT`/`DELETE /income` reject it (`422`), and it is
    /// removed together with the Sell by `DELETE /sells/:id`.
    pub buyback_trade_id: Option<i64>,
    /// The holding account the distribution was paid to (see
    /// `entities::holding_account`): decides whose DRP enrolment applies and
    /// which account a reinvestment trade lands in. Defaults to the seeded
    /// default account when omitted from a request.
    pub holding_account_id: i64,
    /// Optional per-share figure from the registry statement, supplied only
    /// together with `securities_held`: their product, cent-rounded, must
    /// equal the gross cash components (see `check_per_share`). Informational
    /// / validation-only — no report uses it (mirrors
    /// `trades.statement_total`).
    pub amount_per_security: Option<Decimal>,
    /// See `amount_per_security` — the statement's securities-held count.
    pub securities_held: Option<Decimal>,
    /// Non-AMIT trust statements only: the statement's "tax-deferred amount" —
    /// a non-assessable payment that is a CGT event E4 cost-base reduction
    /// (`docs/ato/cgt-non-assessable-payments.md`). Informational: the E4
    /// reduction itself is entered as a `ReturnOfCapital` corporate action and
    /// no calculation reads this figure — the E4 cross-check report
    /// (`reports::e4_cross_check`) flags a row whose non-zero amount has no
    /// matching same-FY action. Trust rows only, never negative (`422`
    /// otherwise, mirrored by a schema CHECK).
    pub tax_deferred_amount: Option<Decimal>,
}

impl<'r> sqlx::FromRow<'r, sqlx::sqlite::SqliteRow> for Income {
    fn from_row(row: &'r sqlx::sqlite::SqliteRow) -> Result<Self, sqlx::Error> {
        Ok(Income {
            id: row.try_get("id")?,
            listing_id: row.try_get("listing_id")?,
            date_paid: row.try_get("date_paid")?,
            ex_date: row.try_get("ex_date")?,
            franked_amount: row_dec(row, "franked_amount")?,
            unfranked_amount: row_dec(row, "unfranked_amount")?,
            foreign_source_income: row_dec(row, "foreign_source_income")?,
            foreign_tax_paid: row_dec(row, "foreign_tax_paid")?,
            tfn_withholding_tax: row_dec(row, "tfn_withholding_tax")?,
            franking_credits: row_dec(row, "franking_credits")?,
            lic_capital_gain_deduction: row_dec(row, "lic_capital_gain_deduction")?,
            conduit_foreign_income: row_dec(row, "conduit_foreign_income")?,
            trust_income: row.try_get("trust_income")?,
            entitlement_date: row.try_get("entitlement_date")?,
            reinvestment_trade_id: row.try_get("reinvestment_trade_id")?,
            currency: row.try_get("currency")?,
            buyback_trade_id: row.try_get("buyback_trade_id")?,
            holding_account_id: row.try_get("holding_account_id")?,
            amount_per_security: row_opt_dec(row, "amount_per_security")?,
            securities_held: row_opt_dec(row, "securities_held")?,
            tax_deferred_amount: row_opt_dec(row, "tax_deferred_amount")?,
        })
    }
}

#[derive(Debug, Deserialize)]
pub struct IncomeBody {
    pub listing_id: i64,
    pub date_paid: NaiveDate,
    #[serde(default)]
    pub ex_date: Option<NaiveDate>,
    #[serde(default)]
    pub franked_amount: Decimal,
    #[serde(default)]
    pub unfranked_amount: Decimal,
    #[serde(default)]
    pub foreign_source_income: Decimal,
    #[serde(default)]
    pub foreign_tax_paid: Decimal,
    #[serde(default)]
    pub tfn_withholding_tax: Decimal,
    #[serde(default)]
    pub franking_credits: Decimal,
    #[serde(default)]
    pub lic_capital_gain_deduction: Decimal,
    #[serde(default)]
    pub conduit_foreign_income: Decimal,
    #[serde(default)]
    pub trust_income: bool,
    /// See `Income::entitlement_date` — trust rows only.
    #[serde(default)]
    pub entitlement_date: Option<NaiveDate>,
    #[serde(default)]
    pub reinvestment_trade_id: Option<i64>,
    #[serde(default = "default_currency")]
    pub currency: String,
    /// Defaults to the seeded default holding account when omitted.
    #[serde(default = "crate::entities::holding_account::default_holding_account_id")]
    pub holding_account_id: i64,
    /// Optional statement cross-check; see `Income::amount_per_security`.
    #[serde(default)]
    pub amount_per_security: Option<Decimal>,
    #[serde(default)]
    pub securities_held: Option<Decimal>,
    /// See `Income::tax_deferred_amount` — trust rows only, ≥ 0.
    #[serde(default)]
    pub tax_deferred_amount: Option<Decimal>,
}

fn default_currency() -> String {
    "AUD".to_string()
}

pub fn router() -> Router<SqlitePool> {
    Router::new()
        .route("/income", get(list))
        .route("/income/{id}", get(get_one).put(upsert).delete(delete))
}

pub async fn db_list(pool: &SqlitePool) -> Result<Vec<Income>, sqlx::Error> {
    sqlx::query_as(
        "SELECT id, listing_id, date_paid, ex_date, franked_amount, unfranked_amount, \
         foreign_source_income, foreign_tax_paid, tfn_withholding_tax, franking_credits, \
         lic_capital_gain_deduction, conduit_foreign_income, trust_income, entitlement_date, \
         reinvestment_trade_id, currency, buyback_trade_id, holding_account_id, \
         amount_per_security, securities_held, tax_deferred_amount \
         FROM income ORDER BY date_paid, id",
    )
    .fetch_all(pool)
    .await
}

pub async fn db_get(pool: &SqlitePool, id: i64) -> Result<Option<Income>, sqlx::Error> {
    sqlx::query_as(
        "SELECT id, listing_id, date_paid, ex_date, franked_amount, unfranked_amount, \
         foreign_source_income, foreign_tax_paid, tfn_withholding_tax, franking_credits, \
         lic_capital_gain_deduction, conduit_foreign_income, trust_income, entitlement_date, \
         reinvestment_trade_id, currency, buyback_trade_id, holding_account_id, \
         amount_per_security, securities_held, tax_deferred_amount \
         FROM income WHERE id = ?",
    )
    .bind(id)
    .fetch_optional(pool)
    .await
}

#[derive(Debug)]
pub enum UpsertError {
    Db(sqlx::Error),
    /// The existing row is a buy-back dividend component (`buyback_trade_id`
    /// set): its figures derive from the buy-back's terms, so free-form edits
    /// are rejected. Delete the buy-back Sell via `DELETE /sells/:id` (which
    /// removes this row too) and re-participate instead. Mapped to `422`.
    BuyBackIncome,
    /// The supplied per-share figures failed the cross-check. Mapped to `422`.
    PerShare(PerShareError),
    /// An `entitlement_date` was supplied on a non-trust row. A dividend is
    /// assessed when paid or credited — present entitlement only shifts the
    /// assessment year of trust distributions (`docs/ato/trust-income-timing.md`).
    /// Mapped to `422`.
    EntitlementDateOnNonTrust,
    /// A `tax_deferred_amount` was supplied on a non-trust row. Tax-deferred
    /// amounts are a unit-trust statement concept (CGT event E4,
    /// `docs/ato/cgt-non-assessable-payments.md`) — a company's equivalent is
    /// a return of capital, entered as the corporate action directly. Mapped
    /// to `422`.
    TaxDeferredOnNonTrust,
    /// A negative `tax_deferred_amount` — the statement figure is a payment
    /// received, never below zero. Mapped to `422`.
    TaxDeferredNegative,
    /// The row's listing is an AMIT (`listings.amit`) but `trust_income` is
    /// false. An AMIT is an attribution managed investment *trust* — its cash
    /// distribution advice is entered as a trust row (cash-only: the AMMA
    /// statement is the assessable record). Mapped to `422`.
    AmitNonTrust,
    /// A non-zero notional tax component (`franking_credits`,
    /// `lic_capital_gain_deduction`, or `conduit_foreign_income`) on an AMIT
    /// listing's row. An AMIT cash advice is not a tax document — the fund's
    /// attribution (credits, LIC deduction, CFI) is reported by its AMMA
    /// statement, and the tax summary reads it from there alone; a value here
    /// would be stored but never used. Carries the offending field name.
    /// Mapped to `422`.
    AmitNotionalComponent(&'static str),
    /// A `tax_deferred_amount` on an AMIT listing's row. An AMIT's cost-base
    /// movement is the AMMA statement's `cost_base_adjustment` (entered as
    /// AMIT adjustments, CGT event E10) — the E4 tax-deferred mechanism is
    /// for non-AMIT trusts. Mapped to `422`.
    AmitTaxDeferred,
}

impl From<sqlx::Error> for UpsertError {
    fn from(e: sqlx::Error) -> Self {
        UpsertError::Db(e)
    }
}

/// Why the supplied per-share figures failed to reconcile (both map to 422).
#[derive(Debug, PartialEq)]
pub enum PerShareError {
    /// Exactly one of `amount_per_security` / `securities_held` was supplied —
    /// the cross-check needs both (or neither).
    SuppliedAlone,
    /// amount_per_security × securities_held, cent-rounded, does not equal
    /// the gross cash components (carried so the rejection can say what the
    /// statement figures actually multiply to).
    ProductMismatch { product: Decimal },
}

/// Cross-check the optional per-share statement figures against the entered
/// amounts: amount_per_security × securities_held, rounded to the cent (half
/// away from zero, matching statements), must equal the gross cash components
/// `franked + unfranked + foreign_source_income` — franking credits are
/// notional and TFN withholding is deducted from (not part of) the gross.
/// Comparison is numeric (`Decimal` equality ignores trailing zeros). Neither
/// supplied means the figures weren't recorded — nothing to check.
fn check_per_share(income: &Income) -> Result<(), PerShareError> {
    let (aps, held) = match (income.amount_per_security, income.securities_held) {
        (None, None) => return Ok(()),
        (Some(aps), Some(held)) => (aps, held),
        _ => return Err(PerShareError::SuppliedAlone),
    };
    let product = (aps * held)
        .round_dp_with_strategy(2, rust_decimal::RoundingStrategy::MidpointAwayFromZero);
    let gross = income.franked_amount + income.unfranked_amount + income.foreign_source_income;
    if product != gross {
        return Err(PerShareError::ProductMismatch { product });
    }
    Ok(())
}

/// Human-readable body for a per-share 422 (shown by the web UI).
pub(crate) fn per_share_detail(e: &PerShareError) -> String {
    match e {
        PerShareError::SuppliedAlone => {
            "amount_per_security and securities_held must be supplied together \
             — provide both or neither"
                .to_string()
        }
        PerShareError::ProductMismatch { product } => {
            format!(
                "per-share figures do not reconcile: amount_per_security × \
                 securities_held computes to {product}, which must equal \
                 franked + unfranked + foreign source income"
            )
        }
    }
}

pub async fn db_upsert(pool: &SqlitePool, income: &Income) -> Result<(), UpsertError> {
    check_per_share(income).map_err(UpsertError::PerShare)?;
    if income.entitlement_date.is_some() && !income.trust_income {
        return Err(UpsertError::EntitlementDateOnNonTrust);
    }
    if let Some(td) = income.tax_deferred_amount {
        if !income.trust_income {
            return Err(UpsertError::TaxDeferredOnNonTrust);
        }
        if td < Decimal::ZERO {
            return Err(UpsertError::TaxDeferredNegative);
        }
    }

    let mut tx = pool.begin().await?;

    // A buy-back dividend-component row is immutable here: it was created
    // from its action's terms by the participation operation. (The INSERT
    // below never sets buyback_trade_id, so a normal row can't become one
    // either.)
    let existing_buyback: Option<Option<i64>> =
        sqlx::query_scalar("SELECT buyback_trade_id FROM income WHERE id = ?")
            .bind(income.id)
            .fetch_optional(&mut *tx)
            .await?;
    if existing_buyback.flatten().is_some() {
        return Err(UpsertError::BuyBackIncome);
    }

    // AMIT listings take cash-only income rows: the row funds the DRP chain
    // (cash components, source withholding) but the AMMA statement is the only
    // assessable record, so the notional tax components must be entered there
    // — a value here would be stored and silently never reported. An unknown
    // listing falls through to the INSERT's FK rejection.
    let listing_amit: Option<bool> = sqlx::query_scalar("SELECT amit FROM listings WHERE id = ?")
        .bind(income.listing_id)
        .fetch_optional(&mut *tx)
        .await?;
    if listing_amit == Some(true) {
        if !income.trust_income {
            return Err(UpsertError::AmitNonTrust);
        }
        for (field, value) in [
            ("franking_credits", income.franking_credits),
            (
                "lic_capital_gain_deduction",
                income.lic_capital_gain_deduction,
            ),
            ("conduit_foreign_income", income.conduit_foreign_income),
        ] {
            if value != Decimal::ZERO {
                return Err(UpsertError::AmitNotionalComponent(field));
            }
        }
        if income.tax_deferred_amount.is_some() {
            return Err(UpsertError::AmitTaxDeferred);
        }
    }

    sqlx::query(
        "INSERT INTO income \
         (id, listing_id, date_paid, ex_date, franked_amount, unfranked_amount, \
          foreign_source_income, foreign_tax_paid, tfn_withholding_tax, franking_credits, \
          lic_capital_gain_deduction, conduit_foreign_income, trust_income, entitlement_date, \
          reinvestment_trade_id, currency, holding_account_id, amount_per_security, \
          securities_held, tax_deferred_amount) \
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?) \
         ON CONFLICT(id) DO UPDATE SET \
             listing_id                 = excluded.listing_id, \
             date_paid                  = excluded.date_paid, \
             ex_date                    = excluded.ex_date, \
             franked_amount             = excluded.franked_amount, \
             unfranked_amount           = excluded.unfranked_amount, \
             foreign_source_income      = excluded.foreign_source_income, \
             foreign_tax_paid           = excluded.foreign_tax_paid, \
             tfn_withholding_tax        = excluded.tfn_withholding_tax, \
             franking_credits           = excluded.franking_credits, \
             lic_capital_gain_deduction = excluded.lic_capital_gain_deduction, \
             conduit_foreign_income     = excluded.conduit_foreign_income, \
             trust_income               = excluded.trust_income, \
             entitlement_date           = excluded.entitlement_date, \
             reinvestment_trade_id      = excluded.reinvestment_trade_id, \
             currency                   = excluded.currency, \
             holding_account_id         = excluded.holding_account_id, \
             amount_per_security        = excluded.amount_per_security, \
             securities_held            = excluded.securities_held, \
             tax_deferred_amount        = excluded.tax_deferred_amount",
    )
    .bind(income.id)
    .bind(income.listing_id)
    .bind(income.date_paid)
    .bind(income.ex_date)
    .bind(income.franked_amount.to_string())
    .bind(income.unfranked_amount.to_string())
    .bind(income.foreign_source_income.to_string())
    .bind(income.foreign_tax_paid.to_string())
    .bind(income.tfn_withholding_tax.to_string())
    .bind(income.franking_credits.to_string())
    .bind(income.lic_capital_gain_deduction.to_string())
    .bind(income.conduit_foreign_income.to_string())
    .bind(income.trust_income)
    .bind(income.entitlement_date)
    .bind(income.reinvestment_trade_id)
    .bind(&income.currency)
    .bind(income.holding_account_id)
    .bind(income.amount_per_security.map(|d| d.to_string()))
    .bind(income.securities_held.map(|d| d.to_string()))
    .bind(income.tax_deferred_amount.map(|d| d.to_string()))
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
    /// The row is a buy-back dividend component (`buyback_trade_id` set) —
    /// deleting it alone would leave the buy-back Sell without its dividend
    /// side. Delete the Sell via `DELETE /sells/:id` instead (which removes
    /// this row too). Mapped to `422`.
    BuyBackIncome,
}

pub async fn db_delete(pool: &SqlitePool, id: i64) -> Result<DeleteOutcome, sqlx::Error> {
    let mut tx = pool.begin().await?;

    let buyback: Option<Option<i64>> =
        sqlx::query_scalar("SELECT buyback_trade_id FROM income WHERE id = ?")
            .bind(id)
            .fetch_optional(&mut *tx)
            .await?;
    match buyback {
        None => return Ok(DeleteOutcome::NotFound),
        Some(Some(_)) => return Ok(DeleteOutcome::BuyBackIncome),
        Some(None) => {}
    }

    sqlx::query("DELETE FROM income WHERE id = ?")
        .bind(id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(DeleteOutcome::Deleted)
}

async fn list(State(pool): State<SqlitePool>) -> Result<Json<Vec<Income>>, ApiError> {
    db_list(&pool).await.map(Json).map_err(ApiError::from)
}

async fn get_one(
    State(pool): State<SqlitePool>,
    Path(id): Path<i64>,
) -> Result<Json<Income>, ApiError> {
    db_get(&pool, id)
        .await
        .map_err(ApiError::from)?
        .map(Json)
        .ok_or(ApiError::NotFound)
}

async fn upsert(
    State(pool): State<SqlitePool>,
    Path(id): Path<i64>,
    Json(body): Json<IncomeBody>,
) -> Result<StatusCode, ApiError> {
    let income = Income {
        id,
        listing_id: body.listing_id,
        date_paid: body.date_paid,
        ex_date: body.ex_date,
        franked_amount: body.franked_amount,
        unfranked_amount: body.unfranked_amount,
        foreign_source_income: body.foreign_source_income,
        foreign_tax_paid: body.foreign_tax_paid,
        tfn_withholding_tax: body.tfn_withholding_tax,
        franking_credits: body.franking_credits,
        lic_capital_gain_deduction: body.lic_capital_gain_deduction,
        conduit_foreign_income: body.conduit_foreign_income,
        trust_income: body.trust_income,
        entitlement_date: body.entitlement_date,
        reinvestment_trade_id: body.reinvestment_trade_id,
        currency: body.currency,
        buyback_trade_id: None,
        holding_account_id: body.holding_account_id,
        amount_per_security: body.amount_per_security,
        securities_held: body.securities_held,
        tax_deferred_amount: body.tax_deferred_amount,
    };
    db_upsert(&pool, &income).await?;
    Ok(StatusCode::NO_CONTENT)
}

impl From<UpsertError> for ApiError {
    fn from(e: UpsertError) -> Self {
        match e {
            // Managed by the buy-back participation → 422.
            UpsertError::BuyBackIncome => ApiError::unprocessable(
                "this income row is a buy-back dividend component and cannot be edited — \
                 it is managed by the buy-back participation",
            ),
            // The cross-check rejection says what the statement figures
            // multiply to, so a typo is findable without a calculator.
            UpsertError::PerShare(detail) => ApiError::unprocessable(per_share_detail(&detail)),
            UpsertError::EntitlementDateOnNonTrust => ApiError::unprocessable(
                "entitlement_date only applies to trust distributions — a dividend is \
                 assessed when paid; tick trust income or clear the entitlement date",
            ),
            UpsertError::TaxDeferredOnNonTrust => ApiError::unprocessable(
                "tax_deferred_amount only applies to trust distributions — a company's \
                 non-assessable payment is entered as a ReturnOfCapital corporate action \
                 instead; tick trust income or clear the tax-deferred amount",
            ),
            UpsertError::TaxDeferredNegative => ApiError::unprocessable(
                "tax_deferred_amount cannot be negative — it is a payment received per \
                 the trust's statement",
            ),
            UpsertError::AmitNonTrust => ApiError::unprocessable(
                "this listing is an AMIT — its distributions are trust income; tick \
                 trust income (the cash row funds DRP reinvestment, while the fund's \
                 AMMA statement carries the assessable figures)",
            ),
            UpsertError::AmitNotionalComponent(field) => ApiError::unprocessable(format!(
                "{field} cannot be entered on an AMIT distribution — the cash advice \
                 is not a tax record; the fund's attributed components belong on its \
                 AMMA statement, which the tax summary reads instead"
            )),
            UpsertError::AmitTaxDeferred => ApiError::unprocessable(
                "tax_deferred_amount does not apply to an AMIT — its cost-base \
                 movement is the AMMA statement's cost_base_adjustment, entered as \
                 AMIT adjustments (CGT event E10), not an E4 tax-deferred amount",
            ),
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
        DeleteOutcome::NotFound => Err(ApiError::not_found("no income with that id")),
        // Managed by the buy-back participation → 422.
        DeleteOutcome::BuyBackIncome => Err(ApiError::unprocessable(
            "this income row is a buy-back dividend component — delete the buy-back Sell \
             instead, which removes it too",
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{self, test_pool, ymd};
    use axum::{body::Body, http::Request};
    use http_body_util::BodyExt;
    use rust_decimal::Decimal;
    use tower::ServiceExt;

    async fn insert_test_listing(pool: &SqlitePool) {
        test_support::listing(1)
            .ticker("VAS")
            .name("Vanguard Australian Shares ETF")
            .insert(pool)
            .await;
    }

    async fn insert_test_trade(pool: &SqlitePool) -> i64 {
        test_support::drp(1, 1)
            .date(ymd(2024, 3, 15))
            .settlement(ymd(2024, 3, 15))
            .qty(Decimal::from(2))
            .price(Decimal::from(95))
            .insert(pool)
            .await;
        1
    }

    fn dividend_income() -> Income {
        test_support::income(1, 1, ymd(2024, 3, 15))
            .with(|i| {
                i.ex_date = Some(ymd(2024, 3, 1));
                i.franked_amount = Decimal::from(70);
                i.unfranked_amount = Decimal::from(30);
                i.franking_credits = Decimal::from(30);
            })
            .build()
    }

    // DB-level tests

    #[tokio::test]
    async fn db_dividend_income_insert_and_retrieve() {
        let pool = test_pool().await;
        insert_test_listing(&pool).await;
        db_upsert(&pool, &dividend_income()).await.unwrap();
        let got = db_get(&pool, 1).await.unwrap().unwrap();
        assert_eq!(got.listing_id, 1);
        assert_eq!(got.franked_amount, Decimal::from(70));
        assert_eq!(got.unfranked_amount, Decimal::from(30));
        assert_eq!(got.franking_credits, Decimal::from(30));
        assert_eq!(
            got.ex_date,
            Some(NaiveDate::from_ymd_opt(2024, 3, 1).unwrap())
        );
        assert!(!got.trust_income);
        assert!(got.reinvestment_trade_id.is_none());
    }

    #[tokio::test]
    async fn db_trust_distribution_insert_and_retrieve() {
        let pool = test_pool().await;
        insert_test_listing(&pool).await;
        let dist = Income {
            holding_account_id: 1,
            id: 2,
            listing_id: 1,
            date_paid: NaiveDate::from_ymd_opt(2024, 6, 30).unwrap(),
            ex_date: None,
            franked_amount: Decimal::ZERO,
            unfranked_amount: Decimal::from(50),
            foreign_source_income: Decimal::from(10),
            foreign_tax_paid: "1.5".parse().unwrap(),
            tfn_withholding_tax: Decimal::ZERO,
            franking_credits: Decimal::ZERO,
            lic_capital_gain_deduction: Decimal::from(5),
            conduit_foreign_income: Decimal::from(3),
            trust_income: true,
            entitlement_date: None,
            reinvestment_trade_id: None,
            currency: "AUD".to_string(),
            buyback_trade_id: None,
            amount_per_security: None,
            securities_held: None,
            tax_deferred_amount: None,
        };
        db_upsert(&pool, &dist).await.unwrap();
        let got = db_get(&pool, 2).await.unwrap().unwrap();
        assert!(got.trust_income);
        assert_eq!(got.foreign_source_income, Decimal::from(10));
        assert_eq!(got.conduit_foreign_income, Decimal::from(3));
        assert_eq!(got.lic_capital_gain_deduction, Decimal::from(5));
    }

    #[tokio::test]
    async fn db_drp_reinvestment_linkage() {
        let pool = test_pool().await;
        insert_test_listing(&pool).await;
        let trade_id = insert_test_trade(&pool).await;
        let inc = Income {
            holding_account_id: 1,
            id: 3,
            listing_id: 1,
            date_paid: NaiveDate::from_ymd_opt(2024, 3, 15).unwrap(),
            ex_date: None,
            franked_amount: Decimal::from(140),
            unfranked_amount: Decimal::from(60),
            foreign_source_income: Decimal::ZERO,
            foreign_tax_paid: Decimal::ZERO,
            tfn_withholding_tax: Decimal::ZERO,
            franking_credits: Decimal::from(60),
            lic_capital_gain_deduction: Decimal::ZERO,
            conduit_foreign_income: Decimal::ZERO,
            trust_income: false,
            entitlement_date: None,
            reinvestment_trade_id: Some(trade_id),
            currency: "AUD".to_string(),
            buyback_trade_id: None,
            amount_per_security: None,
            securities_held: None,
            tax_deferred_amount: None,
        };
        db_upsert(&pool, &inc).await.unwrap();
        let got = db_get(&pool, 3).await.unwrap().unwrap();
        assert_eq!(got.reinvestment_trade_id, Some(trade_id));
        assert_eq!(got.franked_amount, Decimal::from(140));
    }

    #[tokio::test]
    async fn db_get_missing_returns_none() {
        let pool = test_pool().await;
        assert!(db_get(&pool, 999).await.unwrap().is_none());
    }

    // API-level tests

    #[tokio::test]
    async fn api_upsert_and_get() {
        let pool = test_pool().await;
        insert_test_listing(&pool).await;
        let body = serde_json::json!({
            "listing_id": 1,
            "date_paid": "2024-03-15",
            "ex_date": "2024-03-01",
            "franked_amount": 70.0,
            "unfranked_amount": 30.0,
            "franking_credits": 30.0
        });
        let resp = router()
            .with_state(pool.clone())
            .oneshot(
                Request::builder()
                    .method("PUT")
                    .uri("/income/1")
                    .header("content-type", "application/json")
                    .body(Body::from(body.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NO_CONTENT);
        let got = db_get(&pool, 1).await.unwrap().unwrap();
        assert_eq!(got.franked_amount, Decimal::from(70));
    }

    #[tokio::test]
    async fn api_list_returns_ok() {
        let pool = test_pool().await;
        insert_test_listing(&pool).await;
        db_upsert(&pool, &dividend_income()).await.unwrap();
        let resp = router()
            .with_state(pool)
            .oneshot(
                Request::builder()
                    .uri("/income")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let items: Vec<Income> = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(items.len(), 1);
    }

    #[tokio::test]
    async fn api_get_missing_returns_404() {
        let pool = test_pool().await;
        let resp = router()
            .with_state(pool)
            .oneshot(
                Request::builder()
                    .uri("/income/999")
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
        db_upsert(&pool, &dividend_income()).await.unwrap();
        let resp = router()
            .with_state(pool)
            .oneshot(
                Request::builder()
                    .method("DELETE")
                    .uri("/income/1")
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
                    .uri("/income/999")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn api_decimal_precision_round_trip() {
        let pool = test_pool().await;
        insert_test_listing(&pool).await;
        let body = serde_json::json!({
            "listing_id": 1,
            "date_paid": "2024-03-15",
            "franked_amount": "70.123456789",
            "unfranked_amount": "29.876543211",
            "franking_credits": "30.052631578"
        });
        let resp = router()
            .with_state(pool.clone())
            .oneshot(
                Request::builder()
                    .method("PUT")
                    .uri("/income/1")
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
                    .uri("/income/1")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let inc: Income = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(
            inc.franked_amount,
            "70.123456789".parse::<Decimal>().unwrap()
        );
        assert_eq!(
            inc.unfranked_amount,
            "29.876543211".parse::<Decimal>().unwrap()
        );
        assert_eq!(
            inc.franking_credits,
            "30.052631578".parse::<Decimal>().unwrap()
        );
    }

    // Per-share cross-check tests

    async fn put_income(
        pool: &SqlitePool,
        id: i64,
        body: serde_json::Value,
    ) -> (StatusCode, String) {
        let resp = router()
            .with_state(pool.clone())
            .oneshot(
                Request::builder()
                    .method("PUT")
                    .uri(format!("/income/{id}"))
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

    /// PLS 2023 final dividend payment advice: 14 cents per share × 19,695
    /// shares = $2,757.30, 100% franked, franking credit $1,181.70.
    #[tokio::test]
    async fn api_per_share_figures_reconcile_fully_franked_dividend() {
        let pool = test_pool().await;
        insert_test_listing(&pool).await;
        let (status, _) = put_income(
            &pool,
            1,
            serde_json::json!({
                "listing_id": 1,
                "date_paid": "2023-09-27",
                "franked_amount": "2757.30",
                "franking_credits": "1181.70",
                "amount_per_security": "0.14",
                "securities_held": "19695"
            }),
        )
        .await;
        assert_eq!(status, StatusCode::NO_CONTENT);
        let got = db_get(&pool, 1).await.unwrap().unwrap();
        assert_eq!(got.amount_per_security, Some("0.14".parse().unwrap()));
        assert_eq!(got.securities_held, Some(Decimal::from(19695)));
    }

    /// VDHG 2020-10 distribution advice: $0.89891492 per security × 866
    /// securities = $778.4603… — the statement's gross is the cent-rounded
    /// $778.46, so the check must round the product before comparing.
    #[tokio::test]
    async fn api_per_share_product_is_cent_rounded_before_comparison() {
        let pool = test_pool().await;
        insert_test_listing(&pool).await;
        let (status, _) = put_income(
            &pool,
            1,
            serde_json::json!({
                "listing_id": 1,
                "date_paid": "2020-10-16",
                "unfranked_amount": "778.46",
                "trust_income": true,
                "amount_per_security": "0.89891492",
                "securities_held": "866"
            }),
        )
        .await;
        assert_eq!(status, StatusCode::NO_CONTENT);
    }

    #[tokio::test]
    async fn api_per_share_mismatch_returns_422_with_detail_and_persists_nothing() {
        let pool = test_pool().await;
        insert_test_listing(&pool).await;
        let (status, detail) = put_income(
            &pool,
            1,
            serde_json::json!({
                "listing_id": 1,
                "date_paid": "2023-09-27",
                "franked_amount": "2757.30",
                // Typo'd per-share rate: 0.15 × 19,695 = 2,954.25 ≠ 2,757.30.
                "amount_per_security": "0.15",
                "securities_held": "19695"
            }),
        )
        .await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
        // The rejection carries the computed product so the typo is findable.
        assert!(
            detail.contains("2954.25"),
            "detail should cite the product: {detail}"
        );
        assert!(db_get(&pool, 1).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn api_per_share_field_supplied_alone_returns_422() {
        let pool = test_pool().await;
        insert_test_listing(&pool).await;
        for body in [
            serde_json::json!({
                "listing_id": 1, "date_paid": "2024-03-15",
                "unfranked_amount": "100", "amount_per_security": "0.14"
            }),
            serde_json::json!({
                "listing_id": 1, "date_paid": "2024-03-15",
                "unfranked_amount": "100", "securities_held": "19695"
            }),
        ] {
            let (status, detail) = put_income(&pool, 1, body).await;
            assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
            assert!(detail.contains("together"), "detail: {detail}");
        }
        assert!(db_get(&pool, 1).await.unwrap().is_none());
    }

    /// The gross the product must match includes foreign source income but
    /// not franking credits or TFN withholding (notional / deducted-from).
    #[tokio::test]
    async fn api_per_share_gross_includes_foreign_income_not_credits_or_withholding() {
        let pool = test_pool().await;
        insert_test_listing(&pool).await;
        // 1.00 × 100 = 100 = franked 60 + unfranked 30 + foreign 10; the
        // franking credits and TFN withholding must not disturb the check.
        let (status, _) = put_income(
            &pool,
            1,
            serde_json::json!({
                "listing_id": 1,
                "date_paid": "2024-03-15",
                "franked_amount": "60",
                "unfranked_amount": "30",
                "foreign_source_income": "10",
                "franking_credits": "25.71",
                "tfn_withholding_tax": "47",
                "amount_per_security": "1.00",
                "securities_held": "100"
            }),
        )
        .await;
        assert_eq!(status, StatusCode::NO_CONTENT);
    }

    /// Both omitted = no check (existing clients unchanged), and the columns
    /// stay NULL.
    #[tokio::test]
    async fn api_omitted_per_share_pair_skips_the_check() {
        let pool = test_pool().await;
        insert_test_listing(&pool).await;
        let (status, _) = put_income(
            &pool,
            1,
            serde_json::json!({
                "listing_id": 1,
                "date_paid": "2024-03-15",
                "franked_amount": "70"
            }),
        )
        .await;
        assert_eq!(status, StatusCode::NO_CONTENT);
        let got = db_get(&pool, 1).await.unwrap().unwrap();
        assert_eq!(got.amount_per_security, None);
        assert_eq!(got.securities_held, None);
    }

    #[tokio::test]
    async fn api_per_share_decimal_precision_round_trips() {
        let pool = test_pool().await;
        insert_test_listing(&pool).await;
        let (status, _) = put_income(
            &pool,
            1,
            serde_json::json!({
                "listing_id": 1,
                "date_paid": "2020-10-16",
                "unfranked_amount": "778.46",
                "amount_per_security": "0.89891492",
                "securities_held": "866"
            }),
        )
        .await;
        assert_eq!(status, StatusCode::NO_CONTENT);
        let resp = router()
            .with_state(pool)
            .oneshot(
                Request::builder()
                    .uri("/income/1")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let inc: Income = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(
            inc.amount_per_security,
            Some("0.89891492".parse::<Decimal>().unwrap())
        );
        assert_eq!(inc.securities_held, Some(Decimal::from(866)));
    }

    // Entitlement date (trust present-entitlement timing,
    // docs/ato/trust-income-timing.md): trust rows only.

    #[tokio::test]
    async fn db_entitlement_date_round_trips_on_trust_row() {
        let pool = test_pool().await;
        insert_test_listing(&pool).await;
        let mut dist = dividend_income();
        dist.trust_income = true;
        dist.entitlement_date = Some(NaiveDate::from_ymd_opt(2026, 6, 30).unwrap());
        db_upsert(&pool, &dist).await.unwrap();
        let got = db_get(&pool, 1).await.unwrap().unwrap();
        assert_eq!(
            got.entitlement_date,
            Some(NaiveDate::from_ymd_opt(2026, 6, 30).unwrap())
        );
    }

    #[tokio::test]
    async fn db_entitlement_date_on_non_trust_rejected() {
        let pool = test_pool().await;
        insert_test_listing(&pool).await;
        let mut div = dividend_income();
        div.entitlement_date = Some(NaiveDate::from_ymd_opt(2026, 6, 30).unwrap());
        let err = db_upsert(&pool, &div).await.unwrap_err();
        assert!(matches!(err, UpsertError::EntitlementDateOnNonTrust));
        assert!(db_get(&pool, 1).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn api_entitlement_date_on_dividend_returns_422_with_detail() {
        let pool = test_pool().await;
        insert_test_listing(&pool).await;
        let (status, detail) = put_income(
            &pool,
            1,
            serde_json::json!({
                "listing_id": 1,
                "date_paid": "2026-07-15",
                "franked_amount": "70",
                "entitlement_date": "2026-06-30"
            }),
        )
        .await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
        assert!(detail.contains("trust distributions"), "detail: {detail}");
        assert!(db_get(&pool, 1).await.unwrap().is_none());
    }

    // Tax-deferred amount (CGT event E4 cross-check,
    // docs/ato/cgt-non-assessable-payments.md): trust rows only, ≥ 0.

    #[tokio::test]
    async fn db_tax_deferred_amount_round_trips_on_trust_row() {
        let pool = test_pool().await;
        insert_test_listing(&pool).await;
        let mut dist = dividend_income();
        dist.trust_income = true;
        dist.tax_deferred_amount = Some("120.505".parse().unwrap());
        db_upsert(&pool, &dist).await.unwrap();
        let got = db_get(&pool, 1).await.unwrap().unwrap();
        assert_eq!(got.tax_deferred_amount, Some("120.505".parse().unwrap()));
    }

    #[tokio::test]
    async fn db_tax_deferred_amount_on_non_trust_rejected() {
        let pool = test_pool().await;
        insert_test_listing(&pool).await;
        let mut div = dividend_income();
        div.tax_deferred_amount = Some("50".parse().unwrap());
        let err = db_upsert(&pool, &div).await.unwrap_err();
        assert!(matches!(err, UpsertError::TaxDeferredOnNonTrust));
        assert!(db_get(&pool, 1).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn db_negative_tax_deferred_amount_rejected() {
        let pool = test_pool().await;
        insert_test_listing(&pool).await;
        let mut dist = dividend_income();
        dist.trust_income = true;
        dist.tax_deferred_amount = Some("-1".parse().unwrap());
        let err = db_upsert(&pool, &dist).await.unwrap_err();
        assert!(matches!(err, UpsertError::TaxDeferredNegative));
        assert!(db_get(&pool, 1).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn api_tax_deferred_amount_on_dividend_returns_422_with_detail() {
        let pool = test_pool().await;
        insert_test_listing(&pool).await;
        let (status, detail) = put_income(
            &pool,
            1,
            serde_json::json!({
                "listing_id": 1,
                "date_paid": "2025-03-15",
                "franked_amount": "70",
                "tax_deferred_amount": "50"
            }),
        )
        .await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
        assert!(detail.contains("trust distributions"), "detail: {detail}");
        assert!(db_get(&pool, 1).await.unwrap().is_none());
    }

    /// Omitted = NULL — the statement didn't report one, nothing to check.
    #[tokio::test]
    async fn api_omitted_tax_deferred_amount_stays_null() {
        let pool = test_pool().await;
        insert_test_listing(&pool).await;
        let (status, _) = put_income(
            &pool,
            1,
            serde_json::json!({
                "listing_id": 1,
                "date_paid": "2025-03-15",
                "unfranked_amount": "100",
                "trust_income": true
            }),
        )
        .await;
        assert_eq!(status, StatusCode::NO_CONTENT);
        let got = db_get(&pool, 1).await.unwrap().unwrap();
        assert_eq!(got.tax_deferred_amount, None);
    }

    // AMIT cash-only rows (REQUIREMENTS 2026-06-12): an AMIT listing's income
    // row funds the DRP chain but the AMMA statement is the only assessable
    // record — notional tax components are rejected at write time so they
    // can't be stored and silently never reported.

    async fn insert_amit_listing(pool: &SqlitePool) {
        test_support::listing(1)
            .ticker("VDHG")
            .amit(true)
            .insert(pool)
            .await;
    }

    /// The cash side stays fully recordable: gross components, source
    /// withholding (both reduce DRP-reinvestable cash), ex date, and the
    /// per-share cross-check pair.
    #[tokio::test]
    async fn db_amit_cash_only_row_accepted() {
        let pool = test_pool().await;
        insert_amit_listing(&pool).await;
        let mut dist = dividend_income();
        dist.trust_income = true;
        dist.franking_credits = Decimal::ZERO;
        dist.foreign_source_income = Decimal::from(10);
        dist.foreign_tax_paid = Decimal::from(2);
        dist.tfn_withholding_tax = Decimal::from(1);
        db_upsert(&pool, &dist).await.unwrap();
        let got = db_get(&pool, 1).await.unwrap().unwrap();
        assert_eq!(got.foreign_tax_paid, Decimal::from(2));
        assert_eq!(got.tfn_withholding_tax, Decimal::from(1));
    }

    #[tokio::test]
    async fn db_amit_non_trust_row_rejected() {
        let pool = test_pool().await;
        insert_amit_listing(&pool).await;
        let mut div = dividend_income();
        div.franking_credits = Decimal::ZERO;
        let err = db_upsert(&pool, &div).await.unwrap_err();
        assert!(matches!(err, UpsertError::AmitNonTrust));
        assert!(db_get(&pool, 1).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn db_amit_notional_components_rejected() {
        let pool = test_pool().await;
        insert_amit_listing(&pool).await;
        let base = || {
            let mut d = dividend_income();
            d.trust_income = true;
            d.franking_credits = Decimal::ZERO;
            d
        };
        let mut with_credits = base();
        with_credits.franking_credits = Decimal::from(30);
        let mut with_lic = base();
        with_lic.lic_capital_gain_deduction = Decimal::from(5);
        let mut with_cfi = base();
        with_cfi.conduit_foreign_income = Decimal::from(3);
        let cases = [
            ("franking_credits", with_credits),
            ("lic_capital_gain_deduction", with_lic),
            ("conduit_foreign_income", with_cfi),
        ];
        for (field, dist) in cases {
            let err = db_upsert(&pool, &dist).await.unwrap_err();
            assert!(
                matches!(err, UpsertError::AmitNotionalComponent(f) if f == field),
                "expected AmitNotionalComponent({field}), got {err:?}"
            );
        }
        assert!(db_get(&pool, 1).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn db_amit_tax_deferred_amount_rejected() {
        let pool = test_pool().await;
        insert_amit_listing(&pool).await;
        let mut dist = dividend_income();
        dist.trust_income = true;
        dist.franking_credits = Decimal::ZERO;
        dist.tax_deferred_amount = Some("50".parse().unwrap());
        let err = db_upsert(&pool, &dist).await.unwrap_err();
        assert!(matches!(err, UpsertError::AmitTaxDeferred));
        assert!(db_get(&pool, 1).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn api_amit_franking_credits_return_422_with_detail() {
        let pool = test_pool().await;
        insert_amit_listing(&pool).await;
        let (status, detail) = put_income(
            &pool,
            1,
            serde_json::json!({
                "listing_id": 1,
                "date_paid": "2024-03-15",
                "unfranked_amount": "100",
                "franking_credits": "30",
                "trust_income": true
            }),
        )
        .await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
        assert!(detail.contains("AMMA"), "detail: {detail}");
        assert!(db_get(&pool, 1).await.unwrap().is_none());
    }

    /// Non-AMIT listings are untouched by the AMIT validation: a dividend with
    /// credits and a trust row with a tax-deferred amount both still pass.
    #[tokio::test]
    async fn db_non_amit_rows_unaffected_by_amit_validation() {
        let pool = test_pool().await;
        insert_test_listing(&pool).await;
        db_upsert(&pool, &dividend_income()).await.unwrap();
        let mut trust = dividend_income();
        trust.id = 2;
        trust.trust_income = true;
        trust.tax_deferred_amount = Some("40".parse().unwrap());
        db_upsert(&pool, &trust).await.unwrap();
        assert!(db_get(&pool, 2).await.unwrap().is_some());
    }

    #[tokio::test]
    async fn api_trust_entitlement_date_round_trips() {
        let pool = test_pool().await;
        insert_test_listing(&pool).await;
        let (status, _) = put_income(
            &pool,
            1,
            serde_json::json!({
                "listing_id": 1,
                "date_paid": "2026-07-15",
                "unfranked_amount": "100",
                "trust_income": true,
                "entitlement_date": "2026-06-30"
            }),
        )
        .await;
        assert_eq!(status, StatusCode::NO_CONTENT);
        let resp = router()
            .with_state(pool)
            .oneshot(
                Request::builder()
                    .uri("/income/1")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let inc: Income = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(
            inc.entitlement_date,
            Some(NaiveDate::from_ymd_opt(2026, 6, 30).unwrap())
        );
        assert!(inc.trust_income);
    }
}
