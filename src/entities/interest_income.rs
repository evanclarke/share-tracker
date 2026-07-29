//! Interest income: bank / term-deposit / broker-cash interest
//! (docs/ato/tax-return-labels-2026.md). Interest has no listing, so it is
//! its own entity rather than an `income` row.
//!
//! `amount` is the gross interest including any amount withheld;
//! `foreign_source` classifies the row by payer. The tax summary
//! (`reports::tax_summary`) totals Australian-source gross per Australian
//! financial year as its `interest_income` line (question 10 label 10L, with
//! `tfn_withholding_tax` joining the combined withholding line, 10M), and
//! foreign-source gross as its `foreign_interest_income` line (question 20
//! label 20E — assessable foreign source income, with `foreign_tax_paid`
//! joining the FITO line, 20O). Both classifications count in gross
//! assessable investment income, converting a non-AUD amount to AUD via the
//! ATO rate for the month of `date_paid` (failing loudly when no rate
//! exists).

use crate::infra::decimal::Money;
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
pub struct InterestIncome {
    pub id: i64,
    /// Date paid/credited: its month sets the ATO FX conversion month and the
    /// Australian financial year the interest is assessed in.
    pub date_paid: NaiveDate,
    /// Gross interest (including any amount withheld), in `currency`.
    #[sqlx(try_from = "Money")]
    pub amount: Decimal,
    /// TFN amount withheld from the gross interest; joins the tax summary's
    /// combined TFN withholding line. Australian-source rows only — TFN
    /// amounts are withheld by Australian investment bodies.
    #[sqlx(try_from = "Money")]
    pub tfn_withholding_tax: Decimal,
    /// Whether the payer is foreign (e.g. a US broker's cash / money-market
    /// sweep fund): a foreign-source row is assessable foreign source income
    /// (label 20E) on the tax summary's `foreign_interest_income` line, not
    /// Australian gross interest (10L). Defaults to false (Australian).
    pub foreign_source: bool,
    /// Foreign tax withheld from the gross amount, in `currency`; joins the
    /// tax summary's FITO line (capped at the A$1,000 de-minimis,
    /// docs/ato/fito-limit.md). Foreign-source rows only.
    #[sqlx(try_from = "Money")]
    pub foreign_tax_paid: Decimal,
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

#[derive(Debug, Deserialize)]
pub struct InterestIncomeBody {
    pub date_paid: NaiveDate,
    #[serde(default)]
    pub amount: Decimal,
    #[serde(default)]
    pub tfn_withholding_tax: Decimal,
    #[serde(default)]
    pub foreign_source: bool,
    #[serde(default)]
    pub foreign_tax_paid: Decimal,
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

impl CrudEntity for InterestIncome {
    type Key = i64;
    const TABLE: &'static str = "interest_income";
    const COLUMNS: &'static str = "id, date_paid, amount, tfn_withholding_tax, foreign_source, \
     foreign_tax_paid, currency, source, holding_account_id";
    const ORDER_BY: &'static str = "date_paid, id";
    const NOUN: &'static str = "interest income";
}

pub fn router() -> Router<SqlitePool> {
    Router::new()
        .route(
            "/interest_income",
            get(http::list_handler::<InterestIncome>),
        )
        .route(
            "/interest_income/{id}",
            get(http::get_handler::<InterestIncome>)
                .put(upsert)
                .delete(http::delete_handler::<InterestIncome>),
        )
}

#[cfg(test)]
pub async fn db_get(pool: &SqlitePool, id: i64) -> Result<Option<InterestIncome>, sqlx::Error> {
    http::crud_get(pool, id).await
}

#[derive(thiserror::Error, Debug)]
pub enum UpsertError {
    #[error("interest income write failed: {0}")]
    Db(#[from] sqlx::Error),
    /// A negative `amount`, `tfn_withholding_tax`, or `foreign_tax_paid`
    /// (carries the field name): interest figures are the statement's
    /// positive (or zero) amounts — a negative would silently reduce the
    /// year's gross-interest line. Mapped to `422`.
    #[error("{0} cannot be negative")]
    NegativeAmount(&'static str),
    /// `foreign_tax_paid` on an Australian-source row: foreign tax is only
    /// withheld by a foreign payer, and silently accepting it would claim a
    /// FITO the row can't support. Mapped to `422`.
    #[error("foreign_tax_paid only applies to a foreign-source interest row")]
    ForeignTaxOnAustralianSource,
    /// `tfn_withholding_tax` on a foreign-source row: TFN amounts are
    /// withheld by Australian investment bodies — foreign withholding belongs
    /// in `foreign_tax_paid`. Mapped to `422`.
    #[error("tfn_withholding_tax only applies to an Australian-source interest row")]
    TfnWithholdingOnForeignSource,
}

impl From<UpsertError> for ApiError {
    fn from(e: UpsertError) -> Self {
        match e {
            UpsertError::NegativeAmount(field) => ApiError::unprocessable(format!(
                "{field} cannot be negative — interest figures are the statement's own \
                 positive (or zero) amounts"
            )),
            UpsertError::ForeignTaxOnAustralianSource => ApiError::unprocessable(
                "foreign_tax_paid only applies to a foreign-source interest row — mark the \
                 row foreign_source (an Australian payer withholds TFN amounts, entered as \
                 tfn_withholding_tax)",
            ),
            UpsertError::TfnWithholdingOnForeignSource => ApiError::unprocessable(
                "tfn_withholding_tax only applies to an Australian-source interest row — TFN \
                 amounts are withheld by Australian investment bodies; foreign withholding is \
                 entered as foreign_tax_paid",
            ),
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
        ("foreign_tax_paid", i.foreign_tax_paid),
    ] {
        if value < Decimal::ZERO {
            return Err(UpsertError::NegativeAmount(field));
        }
    }
    // Withholding must match the source classification, or the tax summary
    // would claim an offset the row can't support (FITO needs a foreign
    // payer; a TFN amount needs an Australian one).
    if !i.foreign_source && i.foreign_tax_paid > Decimal::ZERO {
        return Err(UpsertError::ForeignTaxOnAustralianSource);
    }
    if i.foreign_source && i.tfn_withholding_tax > Decimal::ZERO {
        return Err(UpsertError::TfnWithholdingOnForeignSource);
    }
    sqlx::query(
        "INSERT INTO interest_income \
         (id, date_paid, amount, tfn_withholding_tax, foreign_source, foreign_tax_paid, \
          currency, source, holding_account_id) \
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?) \
         ON CONFLICT(id) DO UPDATE SET \
             date_paid           = excluded.date_paid, \
             amount              = excluded.amount, \
             tfn_withholding_tax = excluded.tfn_withholding_tax, \
             foreign_source      = excluded.foreign_source, \
             foreign_tax_paid    = excluded.foreign_tax_paid, \
             currency            = excluded.currency, \
             source              = excluded.source, \
             holding_account_id  = excluded.holding_account_id",
    )
    .bind(i.id)
    .bind(i.date_paid)
    .bind(Money(i.amount))
    .bind(Money(i.tfn_withholding_tax))
    .bind(i.foreign_source)
    .bind(Money(i.foreign_tax_paid))
    .bind(&i.currency)
    .bind(&i.source)
    .bind(i.holding_account_id)
    .execute(pool)
    .await?;
    Ok(())
}

#[cfg(test)]
pub async fn db_delete(pool: &SqlitePool, id: i64) -> Result<bool, sqlx::Error> {
    http::crud_delete::<InterestIncome>(pool, id).await
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
        foreign_source: body.foreign_source,
        foreign_tax_paid: body.foreign_tax_paid,
        currency: body.currency,
        source: body.source,
        holding_account_id: body.holding_account_id,
    };
    db_upsert(&pool, &i)
        .await
        .map(|_| StatusCode::NO_CONTENT)
        .map_err(ApiError::from)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{ApiClient, test_pool};

    /// Client over this module's own routes.
    fn client(pool: &SqlitePool) -> ApiClient {
        ApiClient::over(router().with_state(pool.clone()))
    }

    fn sample(id: i64) -> InterestIncome {
        InterestIncome {
            id,
            date_paid: NaiveDate::from_ymd_opt(2024, 3, 15).unwrap(),
            amount: Decimal::from(250),
            tfn_withholding_tax: Decimal::ZERO,
            foreign_source: false,
            foreign_tax_paid: Decimal::ZERO,
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

    /// A foreign-source row (docs/ato/tax-return-labels-2026.md, question 20)
    /// round-trips its classification and foreign tax withheld — the inputs
    /// the tax summary's 20E / FITO routing reads.
    #[tokio::test]
    async fn db_foreign_source_round_trips() {
        let pool = test_pool().await;
        let mut i = sample(1);
        i.foreign_source = true;
        i.foreign_tax_paid = "37.50".parse().unwrap();
        i.currency = "USD".to_string();
        db_upsert(&pool, &i).await.unwrap();
        let got = db_get(&pool, 1).await.unwrap().unwrap();
        assert!(got.foreign_source);
        assert_eq!(got.foreign_tax_paid, "37.50".parse::<Decimal>().unwrap());
        // Existing rows / omitted fields stay Australian-source with no
        // foreign tax (the pre-migration behaviour).
        db_upsert(&pool, &sample(2)).await.unwrap();
        let got = db_get(&pool, 2).await.unwrap().unwrap();
        assert!(!got.foreign_source);
        assert_eq!(got.foreign_tax_paid, Decimal::ZERO);
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
        client(pool)
            .put(format!("/interest_income/{id}"), &body)
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
            (
                "foreign_tax_paid",
                serde_json::json!({
                    "date_paid": "2024-03-15",
                    "amount": "250",
                    "foreign_source": true,
                    "foreign_tax_paid": "-1"
                }),
            ),
        ] {
            let resp = client(&pool).put("/interest_income/1", &body).await;
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
    }

    /// Withholding must match the source classification: foreign tax needs a
    /// foreign-source row (else the FITO line would claim an offset the row
    /// can't support) and a TFN amount needs an Australian one. Both reject
    /// 422 with the correcting field named, and nothing is persisted.
    #[tokio::test]
    async fn api_withholding_source_mismatch_rejected_422() {
        let pool = test_pool().await;
        for (needle, body) in [
            (
                "foreign_tax_paid only applies to a foreign-source interest row",
                serde_json::json!({
                    "date_paid": "2024-03-15",
                    "amount": "250",
                    "foreign_tax_paid": "10"
                }),
            ),
            (
                "tfn_withholding_tax only applies to an Australian-source interest row",
                serde_json::json!({
                    "date_paid": "2024-03-15",
                    "amount": "250",
                    "foreign_source": true,
                    "tfn_withholding_tax": "10"
                }),
            ),
        ] {
            let resp = client(&pool).put("/interest_income/1", &body).await;
            assert_eq!(resp.status, StatusCode::UNPROCESSABLE_ENTITY);
            let detail = resp.text().to_string();
            assert!(
                detail.contains(needle),
                "expected '{needle}', got: {detail}"
            );
            assert!(db_get(&pool, 1).await.unwrap().is_none());
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
        let resp = client(&pool).get("/interest_income").await;
        assert_eq!(resp.status, StatusCode::OK);
        let items: Vec<InterestIncome> = resp.json();
        assert_eq!(items.len(), 1);
    }

    #[tokio::test]
    async fn api_get_missing_returns_404() {
        let pool = test_pool().await;
        let resp = client(&pool).get("/interest_income/99").await;
        assert_eq!(resp.status, StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn api_delete_missing_returns_404() {
        let pool = test_pool().await;
        let resp = client(&pool).delete("/interest_income/99").await;
        assert_eq!(resp.status, StatusCode::NOT_FOUND);
    }
}
