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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InvestmentExpense {
    pub id: i64,
    /// Date incurred: its month sets the ATO FX conversion month and the
    /// Australian financial year the deduction is attributed to.
    pub date_incurred: NaiveDate,
    pub expense_type: ExpenseType,
    /// The deductible amount (post-apportionment), in `currency`. The figure the
    /// tax summary totals.
    pub amount: Decimal,
    /// Optional provenance (informational): the pre-apportionment gross expense.
    pub gross_amount: Option<Decimal>,
    /// Optional provenance (informational): the percentage of `gross_amount` the
    /// user determined was deductible.
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

impl<'r> sqlx::FromRow<'r, sqlx::sqlite::SqliteRow> for InvestmentExpense {
    fn from_row(row: &'r sqlx::sqlite::SqliteRow) -> Result<Self, sqlx::Error> {
        Ok(InvestmentExpense {
            id: row.try_get("id")?,
            date_incurred: row.try_get("date_incurred")?,
            expense_type: row.try_get("expense_type")?,
            amount: row_dec(row, "amount")?,
            gross_amount: row_opt_dec(row, "gross_amount")?,
            deductible_percentage: row_opt_dec(row, "deductible_percentage")?,
            currency: row.try_get("currency")?,
            description: row.try_get("description")?,
            listing_id: row.try_get("listing_id")?,
            holding_account_id: row.try_get("holding_account_id")?,
        })
    }
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

pub fn router() -> Router<SqlitePool> {
    Router::new()
        .route("/investment_expenses", get(list))
        .route(
            "/investment_expenses/{id}",
            get(get_one).put(upsert).delete(delete),
        )
}

const COLUMNS: &str = "id, date_incurred, expense_type, amount, gross_amount, \
     deductible_percentage, currency, description, listing_id, holding_account_id";

pub async fn db_list(pool: &SqlitePool) -> Result<Vec<InvestmentExpense>, sqlx::Error> {
    sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT {COLUMNS} FROM investment_expenses ORDER BY date_incurred, id"
    )))
    .fetch_all(pool)
    .await
}

pub async fn db_get(pool: &SqlitePool, id: i64) -> Result<Option<InvestmentExpense>, sqlx::Error> {
    sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT {COLUMNS} FROM investment_expenses WHERE id = ?"
    )))
    .bind(id)
    .fetch_optional(pool)
    .await
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
    .bind(e.amount.to_string())
    .bind(e.gross_amount.map(|d| d.to_string()))
    .bind(e.deductible_percentage.map(|d| d.to_string()))
    .bind(&e.currency)
    .bind(&e.description)
    .bind(e.listing_id)
    .bind(e.holding_account_id)
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn db_delete(pool: &SqlitePool, id: i64) -> Result<bool, sqlx::Error> {
    let rows = sqlx::query("DELETE FROM investment_expenses WHERE id = ?")
        .bind(id)
        .execute(pool)
        .await?
        .rows_affected();
    Ok(rows > 0)
}

async fn list(State(pool): State<SqlitePool>) -> Result<Json<Vec<InvestmentExpense>>, ApiError> {
    db_list(&pool).await.map(Json).map_err(ApiError::from)
}

async fn get_one(
    State(pool): State<SqlitePool>,
    Path(id): Path<i64>,
) -> Result<Json<InvestmentExpense>, ApiError> {
    db_get(&pool, id)
        .await
        .map_err(ApiError::from)?
        .map(Json)
        .ok_or(ApiError::NotFound)
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

async fn delete(
    State(pool): State<SqlitePool>,
    Path(id): Path<i64>,
) -> Result<StatusCode, ApiError> {
    if db_delete(&pool, id).await? {
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err(ApiError::not_found("no investment expense with that id"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::entities::listing;
    use crate::test_support::{self, test_pool};
    use axum::{body::Body, http::Request};
    use http_body_util::BodyExt;
    use tower::ServiceExt;

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
        router()
            .with_state(pool.clone())
            .oneshot(
                Request::builder()
                    .method("PUT")
                    .uri(format!("/investment_expenses/{id}"))
                    .header("content-type", "application/json")
                    .body(Body::from(body.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap()
            .status()
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

    #[tokio::test]
    async fn api_list_returns_ok() {
        let pool = test_pool().await;
        db_upsert(&pool, &sample(1)).await.unwrap();
        let resp = router()
            .with_state(pool)
            .oneshot(
                Request::builder()
                    .uri("/investment_expenses")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let items: Vec<InvestmentExpense> = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(items.len(), 1);
    }

    #[tokio::test]
    async fn api_delete_missing_returns_404() {
        let pool = test_pool().await;
        let resp = router()
            .with_state(pool)
            .oneshot(
                Request::builder()
                    .method("DELETE")
                    .uri("/investment_expenses/99")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }
}
