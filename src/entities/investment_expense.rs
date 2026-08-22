//! Deductible investment expense: the cost of earning assessable investment
//! income (docs/ato/investment-income-deductions.md +
//! dividend-income-deductions.md). One row is one expense — chiefly interest on
//! money borrowed to buy income-producing shares, plus management/adviser fees,
//! account-keeping fees, and subscriptions.
//!
//! `amount` is the **deductible amount** — post-apportionment, the figure that
//! goes on the return. The ATO's apportionment rules (joint accounts, private vs
//! income-producing use) are the user's determination, not computed here;
//! `gross_amount` and `deductible_percentage` are optional provenance
//! (no calculation reads them), but when both are supplied they are
//! cross-checked against `amount` at write time — see `check_apportionment`.
//!
//! The tax summary (`reports::tax_summary`) totals these by expense type and
//! overall per Australian financial year and nets them against gross assessable
//! investment income, converting a non-AUD amount to AUD via the ATO rate for the
//! month of `date_incurred` (failing loudly when no rate exists).

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

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, sqlx::Type)]
pub enum ExpenseType {
    /// Interest on money borrowed to buy income-producing shares/investments.
    LoanInterest,
    /// Ongoing investment management fees.
    ManagementFee,
    /// Financial-advice fees about an existing investment mix.
    AdviceFee,
    /// Investment-account keeping fees.
    AccountKeepingFee,
    /// Specialist investment journals / subscriptions.
    Subscription,
    /// Any other deductible investment expense.
    Other,
}

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct InvestmentExpense {
    pub id: i64,
    /// Date incurred: its month sets the ATO FX conversion month and the
    /// Australian financial year the deduction is attributed to.
    pub date_incurred: NaiveDate,
    pub expense_type: ExpenseType,
    /// The deductible amount (post-apportionment), in `currency`. The figure the
    /// tax summary totals.
    #[sqlx(try_from = "Money")]
    pub amount: Decimal,
    /// Optional provenance: the pre-apportionment gross expense. No calculation
    /// reads it, but supplied alongside `deductible_percentage` it is
    /// cross-checked against `amount` at write time.
    #[sqlx(try_from = "OptMoney")]
    pub gross_amount: Option<Decimal>,
    /// Optional provenance: the percentage of `gross_amount` the user
    /// determined was deductible (0–100). No calculation reads it, but supplied
    /// alongside `gross_amount` it is cross-checked against `amount` at write
    /// time.
    #[sqlx(try_from = "OptMoney")]
    pub deductible_percentage: Option<Decimal>,
    /// ISO 4217 currency the amount is denominated in. The tax summary converts a
    /// non-AUD amount to AUD via the ATO rate for this currency and the month of
    /// `date_incurred` (see `infra::fx::to_aud`). Defaults to AUD.
    pub currency: String,
    /// Free-text note.
    pub description: Option<String>,
    /// Optional link to the listing the expense relates to (NULL = portfolio-wide).
    pub listing_id: Option<i64>,
    /// Optional link to the holding account the expense relates to
    /// (NULL = portfolio-wide).
    pub holding_account_id: Option<i64>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InvestmentExpenseBody {
    pub date_incurred: NaiveDate,
    pub expense_type: ExpenseType,
    #[serde(default)]
    pub amount: Decimal,
    #[serde(default)]
    pub gross_amount: Option<Decimal>,
    #[serde(default)]
    pub deductible_percentage: Option<Decimal>,
    #[serde(default = "default_currency")]
    pub currency: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub listing_id: Option<i64>,
    #[serde(default)]
    pub holding_account_id: Option<i64>,
}

fn default_currency() -> String {
    "AUD".to_string()
}

impl CrudEntity for InvestmentExpense {
    type Key = i64;
    const TABLE: &'static str = "investment_expenses";
    const COLUMNS: &'static str = "id, date_incurred, expense_type, amount, gross_amount, \
     deductible_percentage, currency, description, listing_id, holding_account_id";
    const ORDER_BY: &'static str = "date_incurred, id";
    const NOUN: &'static str = "investment expense";
}

pub fn router() -> Router<SqlitePool> {
    Router::new()
        .route(
            "/investment_expenses",
            get(http::list_handler::<InvestmentExpense>),
        )
        .route(
            "/investment_expenses/{id}",
            get(http::get_handler::<InvestmentExpense>)
                .put(upsert)
                .delete(http::delete_handler::<InvestmentExpense>),
        )
}

#[cfg(test)]
pub async fn db_get(pool: &SqlitePool, id: i64) -> Result<Option<InvestmentExpense>, sqlx::Error> {
    http::crud_get(pool, id).await
}

#[derive(thiserror::Error, Debug)]
pub enum UpsertError {
    #[error("investment expense write failed: {0}")]
    Db(#[from] sqlx::Error),
    /// A negative `amount` or `gross_amount` (carries the field name): an
    /// expense is the invoice's own positive (or zero) figure, and a negative
    /// deduction is arithmetically income — it lifts the tax summary's net
    /// assessable line above its gross. Mapped to `422`.
    #[error("{0} cannot be negative")]
    NegativeAmount(&'static str),
    /// `deductible_percentage` outside 0–100 (carries the rejected value): a
    /// percentage outside that range is not a percentage. Mapped to `422`.
    #[error("deductible_percentage {0} is outside 0–100")]
    PercentageOutOfRange(Decimal),
    /// `gross_amount × deductible_percentage`, cent-rounded, does not equal
    /// `amount` (carries the computed figure, so the rejection can say what
    /// the provenance pair actually apportions to). Mapped to `422`.
    #[error("the apportionment figures compute to {product}, which is not the deductible amount")]
    ApportionmentMismatch { product: Decimal },
}

impl From<UpsertError> for ApiError {
    fn from(e: UpsertError) -> Self {
        match e {
            UpsertError::NegativeAmount(field) => ApiError::unprocessable(format!(
                "{field} cannot be negative — an investment expense is the invoice's own \
                 positive (or zero) amount; a negative deduction adds to assessable income"
            )),
            UpsertError::PercentageOutOfRange(pct) => ApiError::unprocessable(format!(
                "deductible_percentage {pct} is outside 0–100 — it is the percentage of \
                 gross_amount you determined was deductible"
            )),
            UpsertError::ApportionmentMismatch { product } => ApiError::unprocessable(format!(
                "apportionment figures do not reconcile: gross_amount × deductible_percentage \
                 computes to {product}, which must equal amount (the deductible figure)"
            )),
            // Unknown currency/listing/account (FK) or a bad enum value (CHECK)
            // surfaces as 422 with the offending constraint named.
            UpsertError::Db(err) => err.into(),
        }
    }
}

/// Round half away from zero to the cent, the way statements do.
fn to_cents(v: Decimal) -> Decimal {
    v.round_dp_with_strategy(2, rust_decimal::RoundingStrategy::MidpointAwayFromZero)
}

/// Cross-check the optional apportionment provenance against the deductible
/// amount: `gross_amount × deductible_percentage / 100`, to the cent, must
/// equal `amount` to the cent. Both figures are rounded because either may be
/// carried at sub-cent precision (a fee stated to more decimals, a percentage
/// that doesn't divide evenly) while the money that reaches the return is
/// cents. Only checked when **both** are supplied — either alone records less
/// than a determination (there is nothing to reconcile), and neither means the
/// apportionment simply wasn't recorded. The same shape as `income`'s
/// `amount_per_security × securities_held` reconciliation.
fn check_apportionment(e: &InvestmentExpense) -> Result<(), UpsertError> {
    let (Some(gross), Some(pct)) = (e.gross_amount, e.deductible_percentage) else {
        return Ok(());
    };
    let product = to_cents(gross * pct / Decimal::ONE_HUNDRED);
    if product != to_cents(e.amount) {
        return Err(UpsertError::ApportionmentMismatch { product });
    }
    Ok(())
}

pub async fn db_upsert(pool: &SqlitePool, e: &InvestmentExpense) -> Result<(), UpsertError> {
    // A negative expense is not an expense: it would reduce the year's
    // deduction total, and — since the tax summary subtracts that total from
    // gross assessable investment income — lift the net line above the gross.
    for (field, value) in [("amount", Some(e.amount)), ("gross_amount", e.gross_amount)] {
        if value.is_some_and(|v| v < Decimal::ZERO) {
            return Err(UpsertError::NegativeAmount(field));
        }
    }
    if let Some(pct) = e.deductible_percentage
        && (pct < Decimal::ZERO || pct > Decimal::ONE_HUNDRED)
    {
        return Err(UpsertError::PercentageOutOfRange(pct));
    }
    check_apportionment(e)?;
    sqlx::query(
        "INSERT INTO investment_expenses \
         (id, date_incurred, expense_type, amount, gross_amount, deductible_percentage, \
          currency, description, listing_id, holding_account_id) \
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?) \
         ON CONFLICT(id) DO UPDATE SET \
             date_incurred         = excluded.date_incurred, \
             expense_type          = excluded.expense_type, \
             amount                = excluded.amount, \
             gross_amount          = excluded.gross_amount, \
             deductible_percentage = excluded.deductible_percentage, \
             currency              = excluded.currency, \
             description           = excluded.description, \
             listing_id            = excluded.listing_id, \
             holding_account_id    = excluded.holding_account_id",
    )
    .bind(e.id)
    .bind(e.date_incurred)
    .bind(e.expense_type)
    .bind(Money(e.amount))
    .bind(OptMoney(e.gross_amount))
    .bind(OptMoney(e.deductible_percentage))
    .bind(&e.currency)
    .bind(&e.description)
    .bind(e.listing_id)
    .bind(e.holding_account_id)
    .execute(pool)
    .await?;
    Ok(())
}

#[cfg(test)]
pub async fn db_delete(pool: &SqlitePool, id: i64) -> Result<bool, sqlx::Error> {
    http::crud_delete::<InvestmentExpense>(pool, id).await
}

async fn upsert(
    State(pool): State<SqlitePool>,
    Path(id): Path<i64>,
    Json(body): Json<InvestmentExpenseBody>,
) -> Result<StatusCode, ApiError> {
    let e = InvestmentExpense {
        id,
        date_incurred: body.date_incurred,
        expense_type: body.expense_type,
        amount: body.amount,
        gross_amount: body.gross_amount,
        deductible_percentage: body.deductible_percentage,
        currency: body.currency,
        description: body.description,
        listing_id: body.listing_id,
        holding_account_id: body.holding_account_id,
    };
    db_upsert(&pool, &e)
        .await
        .map(|_| StatusCode::NO_CONTENT)
        .map_err(ApiError::from)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::entities::listing;
    use crate::test_support::{self, ApiClient, test_pool};

    /// Client over this module's own routes.
    fn client(pool: &SqlitePool) -> ApiClient {
        ApiClient::over(router().with_state(pool.clone()))
    }

    async fn insert_listing(pool: &SqlitePool, id: i64) {
        test_support::listing(id)
            .ticker(&format!("EXP{id}"))
            .name(&format!("Expense listing {id}"))
            .security_type(listing::SecurityType::Share)
            .insert(pool)
            .await;
    }

    fn sample(id: i64) -> InvestmentExpense {
        InvestmentExpense {
            id,
            date_incurred: NaiveDate::from_ymd_opt(2024, 3, 15).unwrap(),
            expense_type: ExpenseType::LoanInterest,
            amount: Decimal::from(500),
            gross_amount: None,
            deductible_percentage: None,
            currency: "AUD".to_string(),
            description: Some("margin loan interest".to_string()),
            listing_id: None,
            holding_account_id: None,
        }
    }

    #[tokio::test]
    async fn db_round_trips_with_decimal_precision_and_provenance() {
        let pool = test_pool().await;
        let mut e = sample(1);
        e.amount = "612.345678900".parse().unwrap();
        e.gross_amount = Some("816.460905200".parse().unwrap());
        e.deductible_percentage = Some("75".parse().unwrap());
        db_upsert(&pool, &e).await.unwrap();
        let got = db_get(&pool, 1).await.unwrap().unwrap();
        assert_eq!(got.amount, "612.345678900".parse::<Decimal>().unwrap());
        assert_eq!(
            got.gross_amount,
            Some("816.460905200".parse::<Decimal>().unwrap())
        );
        assert_eq!(got.deductible_percentage, Some(Decimal::from(75)));
        assert_eq!(got.expense_type, ExpenseType::LoanInterest);
        assert_eq!(got.description.as_deref(), Some("margin loan interest"));
    }

    #[tokio::test]
    async fn db_optional_links_round_trip() {
        let pool = test_pool().await;
        insert_listing(&pool, 1).await;
        let mut e = sample(1);
        e.listing_id = Some(1);
        e.holding_account_id = Some(1);
        db_upsert(&pool, &e).await.unwrap();
        let got = db_get(&pool, 1).await.unwrap().unwrap();
        assert_eq!(got.listing_id, Some(1));
        assert_eq!(got.holding_account_id, Some(1));
    }

    #[tokio::test]
    async fn db_get_missing_returns_none() {
        let pool = test_pool().await;
        assert!(db_get(&pool, 99).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn db_delete_removes_and_reports() {
        let pool = test_pool().await;
        db_upsert(&pool, &sample(1)).await.unwrap();
        assert!(db_delete(&pool, 1).await.unwrap());
        assert!(!db_delete(&pool, 1).await.unwrap());
        assert!(db_get(&pool, 1).await.unwrap().is_none());
    }

    async fn put(pool: &SqlitePool, id: i64, body: serde_json::Value) -> StatusCode {
        client(pool)
            .put(format!("/investment_expenses/{id}"), &body)
            .await
            .status
    }

    #[tokio::test]
    async fn api_upsert_and_get() {
        let pool = test_pool().await;
        let status = put(
            &pool,
            1,
            serde_json::json!({
                "date_incurred": "2024-03-15",
                "expense_type": "ManagementFee",
                "amount": "120.50"
            }),
        )
        .await;
        assert_eq!(status, StatusCode::NO_CONTENT);
        let got = db_get(&pool, 1).await.unwrap().unwrap();
        assert_eq!(got.expense_type, ExpenseType::ManagementFee);
        assert_eq!(got.amount, "120.50".parse::<Decimal>().unwrap());
    }

    /// SCENARIOS H-06/H-09: a negative `amount` or `gross_amount` is rejected
    /// with 422 naming the field, and nothing is persisted. A negative
    /// deduction is arithmetically income — it lifts the tax summary's net
    /// assessable line above its gross (see
    /// `a_deduction_alone_cannot_lift_the_net_line_above_the_gross`). Zero
    /// stays acceptable: a nil-cost expense is legitimate.
    #[tokio::test]
    async fn api_negative_amounts_rejected_422() {
        let pool = test_pool().await;
        for (field, body) in [
            (
                "amount",
                serde_json::json!({
                    "date_incurred": "2024-03-15",
                    "expense_type": "Other",
                    "amount": "-500"
                }),
            ),
            (
                "gross_amount",
                serde_json::json!({
                    "date_incurred": "2024-03-15",
                    "expense_type": "Other",
                    "amount": "100",
                    "gross_amount": "-100"
                }),
            ),
        ] {
            let resp = client(&pool).put("/investment_expenses/1", &body).await;
            assert_eq!(
                resp.status,
                StatusCode::UNPROCESSABLE_ENTITY,
                "negative {field} must be rejected"
            );
            let detail = resp.text().to_string();
            assert!(
                detail.contains(field) && detail.contains("cannot be negative"),
                "negative {field}: detail must name the field, got: {detail}"
            );
            assert!(
                db_get(&pool, 1).await.unwrap().is_none(),
                "negative {field}: nothing persisted"
            );
        }
        // Zero is fine on both.
        let status = put(
            &pool,
            1,
            serde_json::json!({
                "date_incurred": "2024-03-15",
                "expense_type": "Other",
                "amount": "0",
                "gross_amount": "0"
            }),
        )
        .await;
        assert_eq!(status, StatusCode::NO_CONTENT);
    }

    /// A `deductible_percentage` outside 0–100 is not a percentage: both ends
    /// are refused 422 naming the field and the rejected value, with nothing
    /// persisted, while the boundaries 0 and 100 are accepted.
    #[tokio::test]
    async fn api_percentage_outside_0_100_rejected_422() {
        let pool = test_pool().await;
        for pct in ["150", "-10"] {
            let resp = client(&pool)
                .put(
                    "/investment_expenses/1",
                    &serde_json::json!({
                        "date_incurred": "2024-03-15",
                        "expense_type": "AdviceFee",
                        "amount": "100",
                        "deductible_percentage": pct
                    }),
                )
                .await;
            assert_eq!(
                resp.status,
                StatusCode::UNPROCESSABLE_ENTITY,
                "deductible_percentage {pct} must be rejected"
            );
            let detail = resp.text().to_string();
            assert!(
                detail.contains("deductible_percentage") && detail.contains(pct),
                "deductible_percentage {pct}: detail must name field and value, got: {detail}"
            );
            assert!(db_get(&pool, 1).await.unwrap().is_none());
        }
        for (pct, gross, amount) in [("0", "1000", "0"), ("100", "100", "100")] {
            let status = put(
                &pool,
                1,
                serde_json::json!({
                    "date_incurred": "2024-03-15",
                    "expense_type": "AdviceFee",
                    "amount": amount,
                    "gross_amount": gross,
                    "deductible_percentage": pct
                }),
            )
            .await;
            assert_eq!(status, StatusCode::NO_CONTENT, "{pct}% must be accepted");
        }
    }

    /// SCENARIOS H-06: when both apportionment provenance figures are supplied
    /// they must reconcile with the deductible amount — `gross × pct`,
    /// cent-rounded, equals `amount`. The scenario's own case (gross 100 at
    /// 50% claimed as 900) is refused with the computed figure in the body;
    /// the consistent case, the exactly-100% case, and the cases where the
    /// pair is incomplete (nothing to reconcile) are accepted.
    #[tokio::test]
    async fn api_apportionment_provenance_must_reconcile() {
        let pool = test_pool().await;
        let resp = client(&pool)
            .put(
                "/investment_expenses/1",
                &serde_json::json!({
                    "date_incurred": "2024-03-15",
                    "expense_type": "AdviceFee",
                    "amount": "900",
                    "gross_amount": "100",
                    "deductible_percentage": "50"
                }),
            )
            .await;
        assert_eq!(resp.status, StatusCode::UNPROCESSABLE_ENTITY);
        let detail = resp.text().to_string();
        assert!(
            detail.contains("computes to 50,") && detail.contains("do not reconcile"),
            "the refusal must carry the computed figure, got: {detail}"
        );
        assert!(db_get(&pool, 1).await.unwrap().is_none());

        for (label, body) in [
            (
                "consistent pair",
                serde_json::json!({
                    "date_incurred": "2024-03-15", "expense_type": "AdviceFee",
                    "amount": "50", "gross_amount": "100",
                    "deductible_percentage": "50"
                }),
            ),
            (
                "exactly 100%",
                serde_json::json!({
                    "date_incurred": "2024-03-15", "expense_type": "AdviceFee",
                    "amount": "100", "gross_amount": "100",
                    "deductible_percentage": "100"
                }),
            ),
            (
                "no percentage",
                serde_json::json!({
                    "date_incurred": "2024-03-15", "expense_type": "AdviceFee",
                    "amount": "900", "gross_amount": "100"
                }),
            ),
            (
                "no gross amount",
                serde_json::json!({
                    "date_incurred": "2024-03-15", "expense_type": "AdviceFee",
                    "amount": "900", "deductible_percentage": "50"
                }),
            ),
            (
                "neither",
                serde_json::json!({
                    "date_incurred": "2024-03-15", "expense_type": "AdviceFee",
                    "amount": "900"
                }),
            ),
            (
                // Sub-cent precision on both sides: 1000 × 33.3333% is
                // 333.333, which is 333.33 in cents, as is the amount.
                "reconciles to the cent",
                serde_json::json!({
                    "date_incurred": "2024-03-15", "expense_type": "AdviceFee",
                    "amount": "333.3330", "gross_amount": "1000",
                    "deductible_percentage": "33.3333"
                }),
            ),
        ] {
            let status = put(&pool, 2, body).await;
            assert_eq!(status, StatusCode::NO_CONTENT, "{label} must be accepted");
        }
    }

    #[tokio::test]
    async fn api_unknown_currency_rejected_422() {
        let pool = test_pool().await;
        let status = put(
            &pool,
            1,
            serde_json::json!({
                "date_incurred": "2024-03-15",
                "expense_type": "LoanInterest",
                "amount": "100",
                "currency": "ZZZ"
            }),
        )
        .await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
        assert!(db_get(&pool, 1).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn api_unknown_listing_rejected_422() {
        let pool = test_pool().await;
        let status = put(
            &pool,
            1,
            serde_json::json!({
                "date_incurred": "2024-03-15",
                "expense_type": "LoanInterest",
                "amount": "100",
                "listing_id": 999
            }),
        )
        .await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    }

    #[tokio::test]
    async fn api_unknown_holding_account_rejected_422() {
        let pool = test_pool().await;
        let status = put(
            &pool,
            1,
            serde_json::json!({
                "date_incurred": "2024-03-15",
                "expense_type": "LoanInterest",
                "amount": "100",
                "holding_account_id": 999
            }),
        )
        .await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    }

    #[tokio::test]
    async fn api_unknown_expense_type_rejected() {
        let pool = test_pool().await;
        // An enum value outside the set fails to deserialize (4xx), never persists.
        let status = put(
            &pool,
            1,
            serde_json::json!({
                "date_incurred": "2024-03-15",
                "expense_type": "Bribe",
                "amount": "100"
            }),
        )
        .await;
        assert!(status.is_client_error(), "expected 4xx, got {status}");
        assert!(db_get(&pool, 1).await.unwrap().is_none());
    }

    /// SCENARIOS H-07: an expense attributed to a listing survives that
    /// listing's later life. A rename is the listing's own event — the row
    /// keeps its `id`, so the expense still names it, in its own financial
    /// year — and the listing can't be deleted out from under the expense:
    /// the refusal names the investment expenses that still draw on it (the
    /// inbound-foreign-key wording, section A) rather than denying the
    /// listing exists.
    #[tokio::test]
    async fn api_expense_survives_a_rename_and_blocks_deleting_its_listing() {
        let pool = test_pool().await;
        test_support::listing(1).ticker("OLD").insert(&pool).await;
        let client = ApiClient::full(&pool);
        client
            .put_ok(
                "/investment_expenses/1",
                &serde_json::json!({
                    "date_incurred": "2026-03-15",
                    "expense_type": "AdviceFee",
                    "amount": "100",
                    "listing_id": 1,
                    "description": "portfolio advice"
                }),
            )
            .await;
        client
            .post(
                "/listings/1/rename",
                &serde_json::json!({ "effective_date": "2026-04-01", "ticker": "NEW" }),
            )
            .await
            .expect_status(StatusCode::CREATED);

        let got = db_get(&pool, 1).await.unwrap().unwrap();
        assert_eq!(got.listing_id, Some(1), "the rename keeps the same listing");
        assert_eq!(
            got.date_incurred,
            NaiveDate::from_ymd_opt(2026, 3, 15).unwrap()
        );
        assert_eq!(got.amount, Decimal::from(100));

        let resp = client.delete("/listings/1").await;
        assert_eq!(resp.status, StatusCode::UNPROCESSABLE_ENTITY);
        let detail = resp.text().to_string();
        assert!(
            detail.contains("investment expenses"),
            "the refusal must name the expense drawing on the listing, got: {detail}"
        );
        assert!(db_get(&pool, 1).await.unwrap().is_some());
    }

    #[tokio::test]
    async fn api_list_returns_ok() {
        let pool = test_pool().await;
        db_upsert(&pool, &sample(1)).await.unwrap();
        let resp = client(&pool).get("/investment_expenses").await;
        assert_eq!(resp.status, StatusCode::OK);
        let items: Vec<InvestmentExpense> = resp.json();
        assert_eq!(items.len(), 1);
    }

    #[tokio::test]
    async fn api_delete_missing_returns_404() {
        let pool = test_pool().await;
        let resp = client(&pool).delete("/investment_expenses/99").await;
        assert_eq!(resp.status, StatusCode::NOT_FOUND);
    }
}
