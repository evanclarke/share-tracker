//! Atomic DRP reinvestment: turn a distribution into a linked DRP trade.
//!
//! Given a distribution (an `income` row) on a DRP-enrolled holding and the
//! reinvestment price, this creates the reinvestment Trade (type `DRP`) and
//! links it back to the distribution (`income.reinvestment_trade_id`) in one
//! transaction. The reinvestable cash plus any residual brought forward from
//! the holding's previous reinvestment is spent on whole shares; the leftover
//! is carried forward or paid out per the enrolment's residual handling.
//!
//! The carried-forward residual is *not* stored as a separate running balance:
//! it lives on each DRP trade (`residual_carried_forward`), and "brought
//! forward" for the next reinvestment is read back from the most recent prior
//! DRP trade for the holding. That single source of truth can't drift.
//!
//! A distribution may be reinvested at most once — re-posting is rejected
//! rather than creating a second trade.

use crate::entities::{
    drp_enrolment::ResidualHandling,
    trade::{self, Trade},
};
use crate::infra::decimal::parse_dec;
use axum::{
    Json, Router,
    extract::{Path, State},
    http::StatusCode,
    routing::post,
};
use chrono::NaiveDate;
use rust_decimal::Decimal;
use serde::Deserialize;
use sqlx::{Row, SqlitePool};

#[derive(Debug, Deserialize)]
pub struct ReinvestBody {
    /// Per-share price the distribution is reinvested at.
    pub reinvestment_price: Decimal,
    /// Optional foreign-per-AUD override for the created DRP trade (defaults to
    /// 1; reports prefer the ATO rate and fall back to this — see `infra::fx`).
    #[serde(default)]
    pub fx_rate: Option<Decimal>,
    /// Optional trade date; defaults to the distribution's `date_paid`.
    #[serde(default)]
    pub date: Option<NaiveDate>,
}

#[derive(Debug)]
pub enum ReinvestError {
    Db(sqlx::Error),
    /// No income row with that id.
    IncomeNotFound,
    /// The holding is not enrolled in a DRP.
    NotEnrolled,
    /// The distribution already has a reinvestment trade.
    AlreadyReinvested,
    /// The reinvestment price is not strictly positive.
    NonPositivePrice,
}

impl From<sqlx::Error> for ReinvestError {
    fn from(e: sqlx::Error) -> Self {
        ReinvestError::Db(e)
    }
}

pub fn router() -> Router<SqlitePool> {
    Router::new().route("/income/{id}/reinvest", post(reinvest))
}

/// Cash actually received from a distribution and therefore available to
/// reinvest. Franking credits are notional (not cash) and excluded; foreign
/// tax and TFN amounts withheld at source reduce the cash received.
fn reinvestable_cash(row: &sqlx::sqlite::SqliteRow) -> Result<Decimal, sqlx::Error> {
    let dec = |col: &str| -> Result<Decimal, sqlx::Error> { parse_dec(col, row.try_get(col)?) };
    Ok(dec("franked_amount")?
        + dec("unfranked_amount")?
        + dec("foreign_source_income")?
        - dec("foreign_tax_paid")?
        - dec("tfn_withholding_tax")?)
}

/// Create the DRP trade for a distribution and link it, atomically.
pub async fn db_reinvest(
    pool: &SqlitePool,
    income_id: i64,
    body: &ReinvestBody,
) -> Result<Trade, ReinvestError> {
    if body.reinvestment_price <= Decimal::ZERO {
        return Err(ReinvestError::NonPositivePrice);
    }

    let mut tx = pool.begin().await?;

    // Load the distribution and its cash components.
    let income = sqlx::query(
        "SELECT listing_id, date_paid, reinvestment_trade_id, franked_amount, unfranked_amount, \
         foreign_source_income, foreign_tax_paid, tfn_withholding_tax FROM income WHERE id = ?",
    )
    .bind(income_id)
    .fetch_optional(&mut *tx)
    .await?;
    let income = match income {
        Some(r) => r,
        None => return Err(ReinvestError::IncomeNotFound),
    };

    let existing: Option<i64> = income.try_get("reinvestment_trade_id")?;
    if existing.is_some() {
        return Err(ReinvestError::AlreadyReinvested);
    }

    let listing_id: i64 = income.try_get("listing_id")?;
    let date_paid: NaiveDate = income.try_get("date_paid")?;
    let cash = reinvestable_cash(&income)?;

    // Must be enrolled, and the enrolment decides what happens to the leftover.
    let handling: Option<ResidualHandling> =
        sqlx::query_scalar("SELECT residual_handling FROM drp_enrolments WHERE listing_id = ?")
            .bind(listing_id)
            .fetch_optional(&mut *tx)
            .await?;
    let handling = match handling {
        Some(h) => h,
        None => return Err(ReinvestError::NotEnrolled),
    };

    // The DRP trade is denominated in the holding's currency.
    let currency: String = sqlx::query_scalar("SELECT currency FROM listings WHERE id = ?")
        .bind(listing_id)
        .fetch_one(&mut *tx)
        .await?;

    // Residual brought forward = the most recent prior DRP trade's carried-forward.
    let prior_cf: Option<String> = sqlx::query_scalar(
        "SELECT residual_carried_forward FROM trades \
         WHERE listing_id = ? AND trade_type = 'DRP' ORDER BY date DESC, id DESC LIMIT 1",
    )
    .bind(listing_id)
    .fetch_optional(&mut *tx)
    .await?;
    let residual_bf = match prior_cf {
        Some(s) => parse_dec("residual_carried_forward", s)?,
        None => Decimal::ZERO,
    };

    // Spend the available cash on whole shares; the leftover is carried or paid out.
    let available = cash + residual_bf;
    let quantity = (available / body.reinvestment_price).floor();
    let cost = quantity * body.reinvestment_price;
    let leftover = available - cost;
    let (carried, paid_out) = match handling {
        ResidualHandling::CarryForward => (leftover, Decimal::ZERO),
        ResidualHandling::PayOut => (Decimal::ZERO, leftover),
    };

    // DRP units are issued by the registry, not market-settled, so the
    // settlement date is the trade date.
    let date = body.date.unwrap_or(date_paid);
    let fx_rate = body.fx_rate.unwrap_or(Decimal::ONE);

    let result = sqlx::query(
        "INSERT INTO trades \
         (trade_type, date, settlement_date, listing_id, average_price, quantity, currency, \
          brokerage, gst_on_brokerage, brokerage_currency, fx_rate, contract_note_ref, \
          residual_brought_forward, residual_carried_forward, residual_paid_out) \
         VALUES ('DRP', ?, ?, ?, ?, ?, ?, '0', '0', ?, ?, NULL, ?, ?, ?)",
    )
    .bind(date)
    .bind(date)
    .bind(listing_id)
    .bind(body.reinvestment_price.to_string())
    .bind(quantity.to_string())
    .bind(&currency)
    .bind(&currency)
    .bind(fx_rate.to_string())
    .bind(residual_bf.to_string())
    .bind(carried.to_string())
    .bind(paid_out.to_string())
    .execute(&mut *tx)
    .await?;
    let new_id = result.last_insert_rowid();

    sqlx::query("UPDATE income SET reinvestment_trade_id = ? WHERE id = ?")
        .bind(new_id)
        .bind(income_id)
        .execute(&mut *tx)
        .await?;

    tx.commit().await?;

    // Read the freshly created trade back so the response is exactly what was stored.
    trade::db_get(pool, new_id)
        .await?
        .ok_or_else(|| ReinvestError::Db(sqlx::Error::RowNotFound))
}

async fn reinvest(
    State(pool): State<SqlitePool>,
    Path(income_id): Path<i64>,
    Json(body): Json<ReinvestBody>,
) -> Result<(StatusCode, Json<Trade>), StatusCode> {
    match db_reinvest(&pool, income_id, &body).await {
        Ok(trade) => Ok((StatusCode::CREATED, Json(trade))),
        Err(ReinvestError::IncomeNotFound) => Err(StatusCode::NOT_FOUND),
        Err(ReinvestError::NotEnrolled) => Err(StatusCode::UNPROCESSABLE_ENTITY),
        Err(ReinvestError::AlreadyReinvested) => Err(StatusCode::UNPROCESSABLE_ENTITY),
        Err(ReinvestError::NonPositivePrice) => Err(StatusCode::UNPROCESSABLE_ENTITY),
        Err(ReinvestError::Db(e)) => {
            tracing::error!(error = %e, "drp reinvestment failed");
            Err(StatusCode::INTERNAL_SERVER_ERROR)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::entities::{drp_enrolment, income, listing, trade::TradeType};
    use crate::infra::db;
    use axum::{body::Body, http::Request};
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    async fn test_pool() -> SqlitePool {
        db::init(":memory:").await.unwrap()
    }

    async fn insert_listing(pool: &SqlitePool, id: i64, currency: &str) {
        listing::db_upsert(
            pool,
            &listing::Listing {
                id,
                exchange_mic: "XASX".to_string(),
                ticker: format!("T{id}"),
                name: format!("Test {id}"),
                isin: None,
                security_type: listing::SecurityType::Trust,
                currency: currency.to_string(),
                amit: false,
            },
        )
        .await
        .unwrap();
    }

    async fn enrol(pool: &SqlitePool, listing_id: i64, handling: ResidualHandling) {
        drp_enrolment::db_upsert(
            pool,
            &drp_enrolment::DrpEnrolment { listing_id, residual_handling: handling },
        )
        .await
        .unwrap();
    }

    /// Insert a distribution paying `cash` as unfranked cash (the simplest cash
    /// component), with `franking` notional franking credits that must be ignored.
    async fn insert_distribution(pool: &SqlitePool, id: i64, listing_id: i64, cash: Decimal, franking: Decimal) {
        income::db_upsert(
            pool,
            &income::Income {
                id,
                listing_id,
                date_paid: NaiveDate::from_ymd_opt(2024, 3, 31).unwrap(),
                ex_date: None,
                franked_amount: Decimal::ZERO,
                unfranked_amount: cash,
                foreign_source_income: Decimal::ZERO,
                foreign_tax_paid: Decimal::ZERO,
                tfn_withholding_tax: Decimal::ZERO,
                franking_credits: franking,
                lic_capital_gain_deduction: Decimal::ZERO,
                conduit_foreign_income: Decimal::ZERO,
                trust_income: true,
                reinvestment_trade_id: None,
            },
        )
        .await
        .unwrap();
    }

    fn body(price: &str) -> ReinvestBody {
        ReinvestBody {
            reinvestment_price: price.parse().unwrap(),
            fx_rate: None,
            date: None,
        }
    }

    #[tokio::test]
    async fn carry_forward_buys_whole_shares_and_carries_leftover() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "AUD").await;
        enrol(&pool, 1, ResidualHandling::CarryForward).await;
        // $100 cash + $30 notional franking credits (must be ignored), price $9.
        insert_distribution(&pool, 1, 1, Decimal::from(100), Decimal::from(30)).await;

        let trade = db_reinvest(&pool, 1, &body("9")).await.unwrap();

        // floor(100 / 9) = 11 shares, cost 99, leftover 1 carried forward.
        assert_eq!(trade.trade_type, TradeType::DRP);
        assert_eq!(trade.quantity, Decimal::from(11));
        assert_eq!(trade.average_price, Decimal::from(9));
        assert_eq!(trade.residual_brought_forward, Decimal::ZERO);
        assert_eq!(trade.residual_carried_forward, Decimal::ONE);
        assert_eq!(trade.residual_paid_out, Decimal::ZERO);

        // The distribution is now linked to the new trade.
        let inc = income::db_get(&pool, 1).await.unwrap().unwrap();
        assert_eq!(inc.reinvestment_trade_id, Some(trade.id));
    }

    #[tokio::test]
    async fn carried_residual_is_picked_up_by_the_next_reinvestment() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "AUD").await;
        enrol(&pool, 1, ResidualHandling::CarryForward).await;

        // First: $100 at $9 → 11 shares, $1 carried.
        insert_distribution(&pool, 1, 1, Decimal::from(100), Decimal::ZERO).await;
        let first = db_reinvest(&pool, 1, &body("9")).await.unwrap();
        assert_eq!(first.residual_carried_forward, Decimal::ONE);

        // Second: $8 cash + $1 brought forward = $9 available at $9 → exactly 1 share, $0 leftover.
        insert_distribution(&pool, 2, 1, Decimal::from(8), Decimal::ZERO).await;
        let second = db_reinvest(&pool, 2, &body("9")).await.unwrap();
        assert_eq!(second.residual_brought_forward, Decimal::ONE);
        assert_eq!(second.quantity, Decimal::from(1));
        assert_eq!(second.residual_carried_forward, Decimal::ZERO);
    }

    #[tokio::test]
    async fn pay_out_records_leftover_as_paid_not_carried() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "AUD").await;
        enrol(&pool, 1, ResidualHandling::PayOut).await;
        insert_distribution(&pool, 1, 1, Decimal::from(100), Decimal::ZERO).await;

        let trade = db_reinvest(&pool, 1, &body("9")).await.unwrap();
        // 11 shares, $1 leftover paid out (not carried).
        assert_eq!(trade.quantity, Decimal::from(11));
        assert_eq!(trade.residual_paid_out, Decimal::ONE);
        assert_eq!(trade.residual_carried_forward, Decimal::ZERO);

        // A pay-out leaves no carried balance for the next reinvestment.
        insert_distribution(&pool, 2, 1, Decimal::from(8), Decimal::ZERO).await;
        let next = db_reinvest(&pool, 2, &body("9")).await.unwrap();
        assert_eq!(next.residual_brought_forward, Decimal::ZERO);
        assert_eq!(next.quantity, Decimal::ZERO); // 8 < 9, no whole share
    }

    #[tokio::test]
    async fn franking_credits_are_excluded_from_reinvestable_cash() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "AUD").await;
        enrol(&pool, 1, ResidualHandling::CarryForward).await;
        // $9 cash but $90 franking credits — only the $9 cash reinvests.
        insert_distribution(&pool, 1, 1, Decimal::from(9), Decimal::from(90)).await;

        let trade = db_reinvest(&pool, 1, &body("9")).await.unwrap();
        assert_eq!(trade.quantity, Decimal::from(1));
        assert_eq!(trade.residual_carried_forward, Decimal::ZERO);
    }

    #[tokio::test]
    async fn not_enrolled_is_rejected_and_nothing_persisted() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "AUD").await;
        insert_distribution(&pool, 1, 1, Decimal::from(100), Decimal::ZERO).await;

        let err = db_reinvest(&pool, 1, &body("9")).await.unwrap_err();
        assert!(matches!(err, ReinvestError::NotEnrolled));
        // No trade created, distribution unlinked.
        let n: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM trades").fetch_one(&pool).await.unwrap();
        assert_eq!(n, 0);
        assert!(income::db_get(&pool, 1).await.unwrap().unwrap().reinvestment_trade_id.is_none());
    }

    #[tokio::test]
    async fn already_reinvested_is_rejected() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "AUD").await;
        enrol(&pool, 1, ResidualHandling::CarryForward).await;
        insert_distribution(&pool, 1, 1, Decimal::from(100), Decimal::ZERO).await;

        db_reinvest(&pool, 1, &body("9")).await.unwrap();
        let err = db_reinvest(&pool, 1, &body("9")).await.unwrap_err();
        assert!(matches!(err, ReinvestError::AlreadyReinvested));
        // Still exactly one DRP trade.
        let n: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM trades").fetch_one(&pool).await.unwrap();
        assert_eq!(n, 1);
    }

    #[tokio::test]
    async fn missing_income_is_not_found() {
        let pool = test_pool().await;
        let err = db_reinvest(&pool, 99, &body("9")).await.unwrap_err();
        assert!(matches!(err, ReinvestError::IncomeNotFound));
    }

    #[tokio::test]
    async fn non_positive_price_is_rejected() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "AUD").await;
        enrol(&pool, 1, ResidualHandling::CarryForward).await;
        insert_distribution(&pool, 1, 1, Decimal::from(100), Decimal::ZERO).await;
        let err = db_reinvest(&pool, 1, &body("0")).await.unwrap_err();
        assert!(matches!(err, ReinvestError::NonPositivePrice));
    }

    #[tokio::test]
    async fn api_reinvest_returns_201_with_trade() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "AUD").await;
        enrol(&pool, 1, ResidualHandling::CarryForward).await;
        insert_distribution(&pool, 1, 1, Decimal::from(100), Decimal::ZERO).await;

        let resp = router()
            .with_state(pool.clone())
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/income/1/reinvest")
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"reinvestment_price":"9"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::CREATED);
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let trade: Trade = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(trade.trade_type, TradeType::DRP);
        assert_eq!(trade.quantity, Decimal::from(11));
    }

    #[tokio::test]
    async fn api_reinvest_not_enrolled_returns_422() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "AUD").await;
        insert_distribution(&pool, 1, 1, Decimal::from(100), Decimal::ZERO).await;
        let resp = router()
            .with_state(pool)
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/income/1/reinvest")
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"reinvestment_price":"9"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY);
    }

    #[tokio::test]
    async fn api_reinvest_missing_income_returns_404() {
        let pool = test_pool().await;
        let resp = router()
            .with_state(pool)
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/income/99/reinvest")
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"reinvestment_price":"9"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }
}
