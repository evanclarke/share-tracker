use crate::infra::http::ApiError;
use axum::{
    Json, Router,
    extract::{Path, State},
    http::StatusCode,
    routing::get,
};
use chrono::NaiveDate;
use crate::infra::decimal::row_dec;
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use sqlx::{Row, SqlitePool};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AmmaStatement {
    pub id: i64,
    pub listing_id: i64,
    pub tax_year_end_date: NaiveDate,
    pub units_held: Decimal,
    pub date_received: NaiveDate,
    pub australian_interest: Decimal,
    pub australian_dividends_unfranked: Decimal,
    pub franked_dividends: Decimal,
    pub franking_credits: Decimal,
    pub net_rent: Decimal,
    pub foreign_income: Decimal,
    pub foreign_tax_credits: Decimal,
    pub other_income: Decimal,
    pub cgt_discount_gains: Decimal,
    pub cgt_indexation_gains: Decimal,
    pub cgt_other_gains: Decimal,
    pub capital_losses_applied: Decimal,
    /// Informational only. Tax-deferred amounts are a reported AMMA statement line, but
    /// they do NOT directly drive the member's cost base adjustment — the ATO's annual
    /// AMIT cost base net amount (`cost_base_adjustment` below) already reflects them.
    /// See `docs/ato/amit-cost-base-adjustments.md`. Not consumed by any calculation.
    pub tax_deferred_amount: Decimal,
    /// Informational only. As with `tax_deferred_amount`, tax-free amounts are reported
    /// on the statement but are not a direct cost-base driver; they are broadly reflected
    /// in `cost_base_adjustment`. See `docs/ato/amit-cost-base-adjustments.md`.
    pub tax_free_amount: Decimal,
    /// The AMIT cost base net amount **per unit** for the year — the sole driver of the
    /// cost base adjustment applied to affected parcels (see
    /// `amit_adjustment::db_cost_base_reductions`). A positive value reduces the cost base;
    /// a negative value increases it (upward adjustments are permitted under the AMIT
    /// regime). See `docs/ato/amit-cost-base-adjustments.md`.
    pub cost_base_adjustment: Decimal,
    pub tfn_withholding_tax: Decimal,
    /// ISO 4217 currency the attributed amounts are denominated in. The tax summary
    /// converts non-AUD amounts to AUD via the ATO rate for this currency and the
    /// month of `tax_year_end_date` (see `infra::fx::to_aud`). Defaults to AUD.
    pub currency: String,
    /// The holding account the statement covers (a registry issues one AMMA
    /// statement per holder account; see `entities::holding_account`).
    /// Defaults to the seeded default account when omitted from a request.
    pub holding_account_id: i64,
}

impl<'r> sqlx::FromRow<'r, sqlx::sqlite::SqliteRow> for AmmaStatement {
    fn from_row(row: &'r sqlx::sqlite::SqliteRow) -> Result<Self, sqlx::Error> {
        Ok(AmmaStatement {
            id: row.try_get("id")?,
            listing_id: row.try_get("listing_id")?,
            tax_year_end_date: row.try_get("tax_year_end_date")?,
            units_held: row_dec(row, "units_held")?,
            date_received: row.try_get("date_received")?,
            australian_interest: row_dec(row, "australian_interest")?,
            australian_dividends_unfranked: row_dec(row, "australian_dividends_unfranked")?,
            franked_dividends: row_dec(row, "franked_dividends")?,
            franking_credits: row_dec(row, "franking_credits")?,
            net_rent: row_dec(row, "net_rent")?,
            foreign_income: row_dec(row, "foreign_income")?,
            foreign_tax_credits: row_dec(row, "foreign_tax_credits")?,
            other_income: row_dec(row, "other_income")?,
            cgt_discount_gains: row_dec(row, "cgt_discount_gains")?,
            cgt_indexation_gains: row_dec(row, "cgt_indexation_gains")?,
            cgt_other_gains: row_dec(row, "cgt_other_gains")?,
            capital_losses_applied: row_dec(row, "capital_losses_applied")?,
            tax_deferred_amount: row_dec(row, "tax_deferred_amount")?,
            tax_free_amount: row_dec(row, "tax_free_amount")?,
            cost_base_adjustment: row_dec(row, "cost_base_adjustment")?,
            tfn_withholding_tax: row_dec(row, "tfn_withholding_tax")?,
            currency: row.try_get("currency")?,
            holding_account_id: row.try_get("holding_account_id")?,
        })
    }
}

#[derive(Debug, Deserialize)]
pub struct AmmaStatementBody {
    pub listing_id: i64,
    pub tax_year_end_date: NaiveDate,
    #[serde(default)]
    pub units_held: Decimal,
    pub date_received: NaiveDate,
    #[serde(default)]
    pub australian_interest: Decimal,
    #[serde(default)]
    pub australian_dividends_unfranked: Decimal,
    #[serde(default)]
    pub franked_dividends: Decimal,
    #[serde(default)]
    pub franking_credits: Decimal,
    #[serde(default)]
    pub net_rent: Decimal,
    #[serde(default)]
    pub foreign_income: Decimal,
    #[serde(default)]
    pub foreign_tax_credits: Decimal,
    #[serde(default)]
    pub other_income: Decimal,
    #[serde(default)]
    pub cgt_discount_gains: Decimal,
    #[serde(default)]
    pub cgt_indexation_gains: Decimal,
    #[serde(default)]
    pub cgt_other_gains: Decimal,
    #[serde(default)]
    pub capital_losses_applied: Decimal,
    #[serde(default)]
    pub tax_deferred_amount: Decimal,
    #[serde(default)]
    pub tax_free_amount: Decimal,
    #[serde(default)]
    pub cost_base_adjustment: Decimal,
    #[serde(default)]
    pub tfn_withholding_tax: Decimal,
    #[serde(default = "default_currency")]
    pub currency: String,
    /// Defaults to the seeded default holding account when omitted.
    #[serde(default = "crate::entities::holding_account::default_holding_account_id")]
    pub holding_account_id: i64,
}

fn default_currency() -> String {
    "AUD".to_string()
}

pub fn router() -> Router<SqlitePool> {
    Router::new()
        .route("/amma_statements", get(list))
        .route("/amma_statements/{id}", get(get_one).put(upsert).delete(delete))
}

pub async fn db_list(pool: &SqlitePool) -> Result<Vec<AmmaStatement>, sqlx::Error> {
    sqlx::query_as(
        "SELECT id, listing_id, tax_year_end_date, units_held, date_received, \
         australian_interest, australian_dividends_unfranked, franked_dividends, \
         franking_credits, net_rent, foreign_income, foreign_tax_credits, other_income, \
         cgt_discount_gains, cgt_indexation_gains, cgt_other_gains, capital_losses_applied, \
         tax_deferred_amount, tax_free_amount, cost_base_adjustment, tfn_withholding_tax, \
         currency, holding_account_id \
         FROM amma_statements ORDER BY tax_year_end_date, id",
    )
    .fetch_all(pool)
    .await
}

pub async fn db_get(pool: &SqlitePool, id: i64) -> Result<Option<AmmaStatement>, sqlx::Error> {
    sqlx::query_as(
        "SELECT id, listing_id, tax_year_end_date, units_held, date_received, \
         australian_interest, australian_dividends_unfranked, franked_dividends, \
         franking_credits, net_rent, foreign_income, foreign_tax_credits, other_income, \
         cgt_discount_gains, cgt_indexation_gains, cgt_other_gains, capital_losses_applied, \
         tax_deferred_amount, tax_free_amount, cost_base_adjustment, tfn_withholding_tax, \
         currency, holding_account_id \
         FROM amma_statements WHERE id = ?",
    )
    .bind(id)
    .fetch_optional(pool)
    .await
}

pub async fn db_upsert(pool: &SqlitePool, stmt: &AmmaStatement) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO amma_statements \
         (id, listing_id, tax_year_end_date, units_held, date_received, \
          australian_interest, australian_dividends_unfranked, franked_dividends, \
          franking_credits, net_rent, foreign_income, foreign_tax_credits, other_income, \
          cgt_discount_gains, cgt_indexation_gains, cgt_other_gains, capital_losses_applied, \
          tax_deferred_amount, tax_free_amount, cost_base_adjustment, tfn_withholding_tax, \
          currency, holding_account_id) \
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?) \
         ON CONFLICT(id) DO UPDATE SET \
             listing_id                      = excluded.listing_id, \
             tax_year_end_date               = excluded.tax_year_end_date, \
             units_held                      = excluded.units_held, \
             date_received                   = excluded.date_received, \
             australian_interest             = excluded.australian_interest, \
             australian_dividends_unfranked  = excluded.australian_dividends_unfranked, \
             franked_dividends               = excluded.franked_dividends, \
             franking_credits                = excluded.franking_credits, \
             net_rent                        = excluded.net_rent, \
             foreign_income                  = excluded.foreign_income, \
             foreign_tax_credits             = excluded.foreign_tax_credits, \
             other_income                    = excluded.other_income, \
             cgt_discount_gains              = excluded.cgt_discount_gains, \
             cgt_indexation_gains            = excluded.cgt_indexation_gains, \
             cgt_other_gains                 = excluded.cgt_other_gains, \
             capital_losses_applied          = excluded.capital_losses_applied, \
             tax_deferred_amount             = excluded.tax_deferred_amount, \
             tax_free_amount                 = excluded.tax_free_amount, \
             cost_base_adjustment            = excluded.cost_base_adjustment, \
             tfn_withholding_tax             = excluded.tfn_withholding_tax, \
             currency                        = excluded.currency, \
             holding_account_id              = excluded.holding_account_id",
    )
    .bind(stmt.id)
    .bind(stmt.listing_id)
    .bind(stmt.tax_year_end_date)
    .bind(stmt.units_held.to_string())
    .bind(stmt.date_received)
    .bind(stmt.australian_interest.to_string())
    .bind(stmt.australian_dividends_unfranked.to_string())
    .bind(stmt.franked_dividends.to_string())
    .bind(stmt.franking_credits.to_string())
    .bind(stmt.net_rent.to_string())
    .bind(stmt.foreign_income.to_string())
    .bind(stmt.foreign_tax_credits.to_string())
    .bind(stmt.other_income.to_string())
    .bind(stmt.cgt_discount_gains.to_string())
    .bind(stmt.cgt_indexation_gains.to_string())
    .bind(stmt.cgt_other_gains.to_string())
    .bind(stmt.capital_losses_applied.to_string())
    .bind(stmt.tax_deferred_amount.to_string())
    .bind(stmt.tax_free_amount.to_string())
    .bind(stmt.cost_base_adjustment.to_string())
    .bind(stmt.tfn_withholding_tax.to_string())
    .bind(&stmt.currency)
    .bind(stmt.holding_account_id)
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn db_delete(pool: &SqlitePool, id: i64) -> Result<bool, sqlx::Error> {
    let result = sqlx::query("DELETE FROM amma_statements WHERE id = ?")
        .bind(id)
        .execute(pool)
        .await?;
    Ok(result.rows_affected() > 0)
}

async fn list(State(pool): State<SqlitePool>) -> Result<Json<Vec<AmmaStatement>>, ApiError> {
    db_list(&pool)
        .await
        .map(Json)
        .map_err(ApiError::from)
}

async fn get_one(
    State(pool): State<SqlitePool>,
    Path(id): Path<i64>,
) -> Result<Json<AmmaStatement>, ApiError> {
    db_get(&pool, id)
        .await
        .map_err(ApiError::from)?
        .map(Json)
        .ok_or(ApiError::NotFound)
}

async fn upsert(
    State(pool): State<SqlitePool>,
    Path(id): Path<i64>,
    Json(body): Json<AmmaStatementBody>,
) -> Result<StatusCode, ApiError> {
    let stmt = AmmaStatement {
        id,
        listing_id: body.listing_id,
        tax_year_end_date: body.tax_year_end_date,
        units_held: body.units_held,
        date_received: body.date_received,
        australian_interest: body.australian_interest,
        australian_dividends_unfranked: body.australian_dividends_unfranked,
        franked_dividends: body.franked_dividends,
        franking_credits: body.franking_credits,
        net_rent: body.net_rent,
        foreign_income: body.foreign_income,
        foreign_tax_credits: body.foreign_tax_credits,
        other_income: body.other_income,
        cgt_discount_gains: body.cgt_discount_gains,
        cgt_indexation_gains: body.cgt_indexation_gains,
        cgt_other_gains: body.cgt_other_gains,
        capital_losses_applied: body.capital_losses_applied,
        tax_deferred_amount: body.tax_deferred_amount,
        tax_free_amount: body.tax_free_amount,
        cost_base_adjustment: body.cost_base_adjustment,
        tfn_withholding_tax: body.tfn_withholding_tax,
        currency: body.currency,
        holding_account_id: body.holding_account_id,
    };
    db_upsert(&pool, &stmt)
        .await
        .map(|_| StatusCode::NO_CONTENT)
        .map_err(ApiError::from)
}

async fn delete(
    State(pool): State<SqlitePool>,
    Path(id): Path<i64>,
) -> Result<StatusCode, ApiError> {
    db_delete(&pool, id)
        .await
        .map(|found| if found { StatusCode::NO_CONTENT } else { StatusCode::NOT_FOUND })
        // Deleting a statement still referenced by AMIT adjustments violates an FK → 422.
        .map_err(ApiError::from)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{infra::db, entities::listing};
    use axum::{body::Body, http::Request};
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    async fn test_pool() -> SqlitePool {
        db::init(":memory:").await.unwrap()
    }

    async fn insert_test_listing(pool: &SqlitePool) {
        listing::db_upsert(
            pool,
            &listing::Listing {
                id: 1,
                exchange_mic: Some("XASX".to_string()),
                ticker: "VAF".to_string(),
                name: "Vanguard Australian Fixed Interest ETF".to_string(),
                isin: None,
                security_type: listing::SecurityType::ETF,
                currency: "AUD".to_string(),
                amit: true,
                preference: false,
            },
        )
        .await
        .unwrap();
    }

    fn sample_amma() -> AmmaStatement {
        AmmaStatement {
            holding_account_id: 1,
            id: 1,
            listing_id: 1,
            tax_year_end_date: NaiveDate::from_ymd_opt(2024, 6, 30).unwrap(),
            units_held: "1000".parse().unwrap(),
            date_received: NaiveDate::from_ymd_opt(2024, 8, 15).unwrap(),
            australian_interest: "12.50".parse().unwrap(),
            australian_dividends_unfranked: "5.25".parse().unwrap(),
            franked_dividends: Decimal::ZERO,
            franking_credits: Decimal::ZERO,
            net_rent: Decimal::ZERO,
            foreign_income: Decimal::ZERO,
            foreign_tax_credits: Decimal::ZERO,
            other_income: Decimal::ZERO,
            cgt_discount_gains: Decimal::ZERO,
            cgt_indexation_gains: Decimal::ZERO,
            cgt_other_gains: Decimal::ZERO,
            capital_losses_applied: Decimal::ZERO,
            tax_deferred_amount: "2.30".parse().unwrap(),
            tax_free_amount: "1.10".parse().unwrap(),
            cost_base_adjustment: "0.0023".parse().unwrap(),
            tfn_withholding_tax: Decimal::ZERO,
            currency: "AUD".to_string(),
        }
    }

    // DB-level tests

    #[tokio::test]
    async fn db_insert_and_retrieve() {
        let pool = test_pool().await;
        insert_test_listing(&pool).await;
        db_upsert(&pool, &sample_amma()).await.unwrap();
        let got = db_get(&pool, 1).await.unwrap().unwrap();
        assert_eq!(got.listing_id, 1);
        assert_eq!(got.tax_year_end_date, NaiveDate::from_ymd_opt(2024, 6, 30).unwrap());
        assert_eq!(got.units_held, "1000".parse::<Decimal>().unwrap());
        assert_eq!(got.australian_interest, "12.50".parse::<Decimal>().unwrap());
        assert_eq!(got.australian_dividends_unfranked, "5.25".parse::<Decimal>().unwrap());
        assert_eq!(got.tax_deferred_amount, "2.30".parse::<Decimal>().unwrap());
        assert_eq!(got.tax_free_amount, "1.10".parse::<Decimal>().unwrap());
        assert_eq!(got.cost_base_adjustment, "0.0023".parse::<Decimal>().unwrap());
    }

    #[tokio::test]
    async fn db_cost_base_adjustment_calculation() {
        let pool = test_pool().await;
        insert_test_listing(&pool).await;
        db_upsert(&pool, &sample_amma()).await.unwrap();
        let got = db_get(&pool, 1).await.unwrap().unwrap();
        // total cost base reduction = cost_base_adjustment per unit * units_held
        let total_adjustment = got.cost_base_adjustment * got.units_held;
        assert_eq!(total_adjustment, "2.3".parse::<Decimal>().unwrap());
    }

    #[tokio::test]
    async fn db_get_missing_returns_none() {
        let pool = test_pool().await;
        assert!(db_get(&pool, 999).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn db_upsert_updates_existing() {
        let pool = test_pool().await;
        insert_test_listing(&pool).await;
        db_upsert(&pool, &sample_amma()).await.unwrap();
        let mut updated = sample_amma();
        updated.australian_interest = "99.99".parse().unwrap();
        db_upsert(&pool, &updated).await.unwrap();
        let got = db_get(&pool, 1).await.unwrap().unwrap();
        assert_eq!(got.australian_interest, "99.99".parse::<Decimal>().unwrap());
    }

    // API-level tests

    #[tokio::test]
    async fn api_upsert_and_get() {
        let pool = test_pool().await;
        insert_test_listing(&pool).await;
        let body = serde_json::json!({
            "listing_id": 1,
            "tax_year_end_date": "2024-06-30",
            "units_held": "1000",
            "date_received": "2024-08-15",
            "australian_interest": "12.50",
            "tax_deferred_amount": "2.30",
            "cost_base_adjustment": "0.0023"
        });
        let resp = router()
            .with_state(pool.clone())
            .oneshot(
                Request::builder()
                    .method("PUT")
                    .uri("/amma_statements/1")
                    .header("content-type", "application/json")
                    .body(Body::from(body.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NO_CONTENT);
        let got = db_get(&pool, 1).await.unwrap().unwrap();
        assert_eq!(got.australian_interest, "12.50".parse::<Decimal>().unwrap());
        assert_eq!(got.cost_base_adjustment, "0.0023".parse::<Decimal>().unwrap());
    }

    #[tokio::test]
    async fn api_list_returns_ok() {
        let pool = test_pool().await;
        insert_test_listing(&pool).await;
        db_upsert(&pool, &sample_amma()).await.unwrap();
        let resp = router()
            .with_state(pool)
            .oneshot(Request::builder().uri("/amma_statements").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let items: Vec<AmmaStatement> = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(items.len(), 1);
    }

    #[tokio::test]
    async fn api_get_missing_returns_404() {
        let pool = test_pool().await;
        let resp = router()
            .with_state(pool)
            .oneshot(
                Request::builder()
                    .uri("/amma_statements/999")
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
        db_upsert(&pool, &sample_amma()).await.unwrap();
        let resp = router()
            .with_state(pool)
            .oneshot(
                Request::builder()
                    .method("DELETE")
                    .uri("/amma_statements/1")
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
                    .uri("/amma_statements/999")
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
            "tax_year_end_date": "2024-06-30",
            "units_held": "1234.567890123",
            "date_received": "2024-08-15",
            "australian_interest": "9.876543210",
            "cost_base_adjustment": "0.001234567890"
        });
        let resp = router()
            .with_state(pool.clone())
            .oneshot(
                Request::builder()
                    .method("PUT")
                    .uri("/amma_statements/1")
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
                    .uri("/amma_statements/1")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let got: AmmaStatement = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(got.units_held, "1234.567890123".parse::<Decimal>().unwrap());
        assert_eq!(got.australian_interest, "9.876543210".parse::<Decimal>().unwrap());
        assert_eq!(got.cost_base_adjustment, "0.001234567890".parse::<Decimal>().unwrap());
    }
}
