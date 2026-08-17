//! Deductible investment expense: the cost of earning assessable investment
//! income (docs/ato/investment-income-deductions.md +
//! dividend-income-deductions.md). One row is one expense — chiefly interest on
//! money borrowed to buy income-producing shares, plus management/adviser fees,
//! account-keeping fees, and subscriptions.
//!
//! `amount` is the **deductible amount** — post-apportionment, the figure that
//! goes on the return. The ATO's apportionment rules (joint accounts, private vs
//! income-producing use) are the user's determination, not computed here;
//! `gross_amount` and `deductible_percentage` are optional provenance only
//! (informational — no calculation reads them).
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
    /// Optional provenance (informational): the pre-apportionment gross expense.
    #[sqlx(try_from = "OptMoney")]
    pub gross_amount: Option<Decimal>,
    /// Optional provenance (informational): the percentage of `gross_amount` the
    /// user determined was deductible.
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

pub async fn db_upsert(pool: &SqlitePool, e: &InvestmentExpense) -> Result<(), sqlx::Error> {
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
        // Unknown currency/listing/account (FK) or a bad enum value (CHECK)
        // surface as 422 with the offending constraint named.
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
