//! Interest income: bank / term-deposit / broker-cash interest (ATO question
//! 10 — Gross interest, docs/ato/tax-return-labels-2026.md). Interest has no
//! listing, so it is its own entity rather than an `income` row.
//!
//! `amount` is the gross interest including any TFN amount withheld (the 10L
//! convention); `tfn_withholding_tax` is the withheld amount (10M). The tax
//! summary (`reports::tax_summary`) totals the gross per Australian financial
//! year as its `interest_income` line, includes it in gross assessable
//! investment income, and joins the TFN amount to the combined withholding
//! line, converting a non-AUD amount to AUD via the ATO rate for the month of
//! `date_paid` (failing loudly when no rate exists).

use crate::infra::decimal::row_dec;
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
pub struct InterestIncome {
    pub id: i64,
    /// Date paid/credited: its month sets the ATO FX conversion month and the
    /// Australian financial year the interest is assessed in.
    pub date_paid: NaiveDate,
    /// Gross interest (including any TFN amount withheld), in `currency`.
    pub amount: Decimal,
    /// TFN amount withheld from the gross interest; joins the tax summary's
    /// combined TFN withholding line.
    pub tfn_withholding_tax: Decimal,
    /// ISO 4217 currency the amounts are denominated in. The tax summary
    /// converts a non-AUD amount to AUD via the ATO rate for this currency and
    /// the month of `date_paid` (see `infra::fx::to_aud`). Defaults to AUD.
    pub currency: String,
    /// Free-text source description (e.g. "ANZ savings account").
    /// Informational only — no calculation reads it.
    pub source: Option<String>,
    /// Optional link to the holding account the interest was paid on (e.g. a
    /// broker cash account); NULL for interest from outside the portfolio's
    /// accounts. Informational only — no calculation reads it.
    pub holding_account_id: Option<i64>,
}

impl<'r> sqlx::FromRow<'r, sqlx::sqlite::SqliteRow> for InterestIncome {
    fn from_row(row: &'r sqlx::sqlite::SqliteRow) -> Result<Self, sqlx::Error> {
        Ok(InterestIncome {
            id: row.try_get("id")?,
            date_paid: row.try_get("date_paid")?,
            amount: row_dec(row, "amount")?,
            tfn_withholding_tax: row_dec(row, "tfn_withholding_tax")?,
            currency: row.try_get("currency")?,
            source: row.try_get("source")?,
            holding_account_id: row.try_get("holding_account_id")?,
        })
    }
}

#[derive(Debug, Deserialize)]
pub struct InterestIncomeBody {
    pub date_paid: NaiveDate,
    #[serde(default)]
    pub amount: Decimal,
    #[serde(default)]
    pub tfn_withholding_tax: Decimal,
    #[serde(default = "default_currency")]
    pub currency: String,
    #[serde(default)]
    pub source: Option<String>,
    #[serde(default)]
    pub holding_account_id: Option<i64>,
}

fn default_currency() -> String {
    "AUD".to_string()
}

pub fn router() -> Router<SqlitePool> {
    Router::new().route("/interest_income", get(list)).route(
        "/interest_income/{id}",
        get(get_one).put(upsert).delete(delete),
    )
}

const COLUMNS: &str =
    "id, date_paid, amount, tfn_withholding_tax, currency, source, holding_account_id";

pub async fn db_list(pool: &SqlitePool) -> Result<Vec<InterestIncome>, sqlx::Error> {
    sqlx::query_as(&format!(
        "SELECT {COLUMNS} FROM interest_income ORDER BY date_paid, id"
    ))
    .fetch_all(pool)
    .await
}

pub async fn db_get(pool: &SqlitePool, id: i64) -> Result<Option<InterestIncome>, sqlx::Error> {
    sqlx::query_as(&format!(
        "SELECT {COLUMNS} FROM interest_income WHERE id = ?"
    ))
    .bind(id)
    .fetch_optional(pool)
    .await
}

#[derive(Debug)]
pub enum UpsertError {
    Db(sqlx::Error),
    /// A negative `amount` or `tfn_withholding_tax` (carries the field name):
    /// interest figures are the statement's positive (or zero) amounts — a
    /// negative would silently reduce the year's gross-interest line. Mapped
    /// to `422`.
    NegativeAmount(&'static str),
}

impl From<sqlx::Error> for UpsertError {
    fn from(e: sqlx::Error) -> Self {
        UpsertError::Db(e)
    }
}

impl From<UpsertError> for ApiError {
    fn from(e: UpsertError) -> Self {
        match e {
            UpsertError::NegativeAmount(field) => ApiError::unprocessable(format!(
                "{field} cannot be negative — interest figures are the statement's own \
                 positive (or zero) amounts"
            )),
            // Unknown currency/account (FK) surfaces as 422 with the
            // offending constraint named.
            UpsertError::Db(err) => err.into(),
        }
    }
}

pub async fn db_upsert(pool: &SqlitePool, i: &InterestIncome) -> Result<(), UpsertError> {
    for (field, value) in [
        ("amount", i.amount),
        ("tfn_withholding_tax", i.tfn_withholding_tax),
    ] {
        if value < Decimal::ZERO {
            return Err(UpsertError::NegativeAmount(field));
        }
    }
    sqlx::query(
        "INSERT INTO interest_income \
         (id, date_paid, amount, tfn_withholding_tax, currency, source, holding_account_id) \
         VALUES (?, ?, ?, ?, ?, ?, ?) \
         ON CONFLICT(id) DO UPDATE SET \
             date_paid           = excluded.date_paid, \
             amount              = excluded.amount, \
             tfn_withholding_tax = excluded.tfn_withholding_tax, \
             currency            = excluded.currency, \
             source              = excluded.source, \
             holding_account_id  = excluded.holding_account_id",
    )
    .bind(i.id)
    .bind(i.date_paid)
    .bind(i.amount.to_string())
    .bind(i.tfn_withholding_tax.to_string())
    .bind(&i.currency)
    .bind(&i.source)
    .bind(i.holding_account_id)
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn db_delete(pool: &SqlitePool, id: i64) -> Result<bool, sqlx::Error> {
    let rows = sqlx::query("DELETE FROM interest_income WHERE id = ?")
        .bind(id)
        .execute(pool)
        .await?
        .rows_affected();
    Ok(rows > 0)
}

async fn list(State(pool): State<SqlitePool>) -> Result<Json<Vec<InterestIncome>>, ApiError> {
    db_list(&pool).await.map(Json).map_err(ApiError::from)
}

async fn get_one(
    State(pool): State<SqlitePool>,
    Path(id): Path<i64>,
) -> Result<Json<InterestIncome>, ApiError> {
    db_get(&pool, id)
        .await
        .map_err(ApiError::from)?
        .map(Json)
        .ok_or(ApiError::NotFound)
}

async fn upsert(
    State(pool): State<SqlitePool>,
    Path(id): Path<i64>,
    Json(body): Json<InterestIncomeBody>,
) -> Result<StatusCode, ApiError> {
    let i = InterestIncome {
        id,
        date_paid: body.date_paid,
        amount: body.amount,
        tfn_withholding_tax: body.tfn_withholding_tax,
        currency: body.currency,
        source: body.source,
        holding_account_id: body.holding_account_id,
    };
    db_upsert(&pool, &i)
        .await
        .map(|_| StatusCode::NO_CONTENT)
        .map_err(ApiError::from)
}

async fn delete(
    State(pool): State<SqlitePool>,
    Path(id): Path<i64>,
) -> Result<StatusCode, ApiError> {
    if db_delete(&pool, id).await? {
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err(ApiError::not_found("no interest income with that id"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::test_pool;
    use axum::{body::Body, http::Request};
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    fn sample(id: i64) -> InterestIncome {
        InterestIncome {
            id,
            date_paid: NaiveDate::from_ymd_opt(2024, 3, 15).unwrap(),
            amount: Decimal::from(250),
            tfn_withholding_tax: Decimal::ZERO,
            currency: "AUD".to_string(),
            source: Some("savings account".to_string()),
            holding_account_id: None,
        }
    }

    #[tokio::test]
    async fn db_round_trips_with_decimal_precision() {
        let pool = test_pool().await;
        let mut i = sample(1);
        i.amount = "612.345678900".parse().unwrap();
        i.tfn_withholding_tax = "287.802269083".parse().unwrap();
        db_upsert(&pool, &i).await.unwrap();
        let got = db_get(&pool, 1).await.unwrap().unwrap();
        assert_eq!(got.amount, "612.345678900".parse::<Decimal>().unwrap());
        assert_eq!(
            got.tfn_withholding_tax,
            "287.802269083".parse::<Decimal>().unwrap()
        );
        assert_eq!(got.date_paid, NaiveDate::from_ymd_opt(2024, 3, 15).unwrap());
        assert_eq!(got.source.as_deref(), Some("savings account"));
        assert_eq!(got.holding_account_id, None);
    }

    #[tokio::test]
    async fn db_optional_holding_account_round_trips() {
        let pool = test_pool().await;
        let mut i = sample(1);
        i.holding_account_id = Some(1); // the seeded default account
        db_upsert(&pool, &i).await.unwrap();
        let got = db_get(&pool, 1).await.unwrap().unwrap();
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
                    .uri(format!("/interest_income/{id}"))
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
                "date_paid": "2024-03-15",
                "amount": "250.75",
                "tfn_withholding_tax": "117.85",
                "source": "term deposit"
            }),
        )
        .await;
        assert_eq!(status, StatusCode::NO_CONTENT);
        let got = db_get(&pool, 1).await.unwrap().unwrap();
        assert_eq!(got.amount, "250.75".parse::<Decimal>().unwrap());
        assert_eq!(
            got.tfn_withholding_tax,
            "117.85".parse::<Decimal>().unwrap()
        );
        assert_eq!(got.currency, "AUD"); // defaulted
        assert_eq!(got.source.as_deref(), Some("term deposit"));
    }

    /// A negative gross amount or TFN withholding is rejected with 422 naming
    /// the field, and nothing is persisted (2026-07-12 review: negatives were
    /// accepted, silently reducing the year's gross-interest line). Zero
    /// stays fine (the withholding default).
    #[tokio::test]
    async fn api_negative_amounts_rejected_422() {
        let pool = test_pool().await;
        for (field, body) in [
            (
                "amount",
                serde_json::json!({ "date_paid": "2024-03-15", "amount": "-250" }),
            ),
            (
                "tfn_withholding_tax",
                serde_json::json!({
                    "date_paid": "2024-03-15",
                    "amount": "250",
                    "tfn_withholding_tax": "-1"
                }),
            ),
        ] {
            let resp = router()
                .with_state(pool.clone())
                .oneshot(
                    Request::builder()
                        .method("PUT")
                        .uri("/interest_income/1")
                        .header("content-type", "application/json")
                        .body(Body::from(body.to_string()))
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(
                resp.status(),
                StatusCode::UNPROCESSABLE_ENTITY,
                "negative {field} must be rejected"
            );
            let bytes = resp.into_body().collect().await.unwrap().to_bytes();
            let detail = String::from_utf8(bytes.to_vec()).unwrap();
            assert!(
                detail.contains(field) && detail.contains("cannot be negative"),
                "negative {field}: detail must name the field, got: {detail}"
            );
            assert!(
                db_get(&pool, 1).await.unwrap().is_none(),
                "negative {field}: nothing persisted"
            );
        }
    }

    #[tokio::test]
    async fn api_unknown_currency_rejected_422() {
        let pool = test_pool().await;
        let status = put(
            &pool,
            1,
            serde_json::json!({
                "date_paid": "2024-03-15",
                "amount": "100",
                "currency": "ZZZ"
            }),
        )
        .await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
        assert!(db_get(&pool, 1).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn api_unknown_holding_account_rejected_422() {
        let pool = test_pool().await;
        let status = put(
            &pool,
            1,
            serde_json::json!({
                "date_paid": "2024-03-15",
                "amount": "100",
                "holding_account_id": 999
            }),
        )
        .await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    }

    #[tokio::test]
    async fn api_list_returns_ok() {
        let pool = test_pool().await;
        db_upsert(&pool, &sample(1)).await.unwrap();
        let resp = router()
            .with_state(pool)
            .oneshot(
                Request::builder()
                    .uri("/interest_income")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let items: Vec<InterestIncome> = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(items.len(), 1);
    }

    #[tokio::test]
    async fn api_get_missing_returns_404() {
        let pool = test_pool().await;
        let resp = router()
            .with_state(pool)
            .oneshot(
                Request::builder()
                    .uri("/interest_income/99")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn api_delete_missing_returns_404() {
        let pool = test_pool().await;
        let resp = router()
            .with_state(pool)
            .oneshot(
                Request::builder()
                    .method("DELETE")
                    .uri("/interest_income/99")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }
}
