//! Atomic ESS vesting: tie an ESS statement's income and CGT sides together.
//!
//! Given an `ess_statements` row, this creates the **cost-base-reset Buy** for
//! the vested shares and links it back (`trades.ess_statement_id`) in one
//! transaction. At the taxing point the ESS interest's first-element cost base
//! is reset to its market value and it is taken to be re-acquired on that date
//! for CGT (docs/ato/employee-share-schemes.md), so the Buy is dated the
//! taxing-point date with `average_price` = the per-share market value, zero
//! brokerage, in the statement's currency. The 12-month CGT discount clock and
//! the cost base both run from the taxing point — no `deemed_acquisition_date`.
//!
//! The income side (the assessable discount) is already on the statement and
//! reaches the tax summary directly; the vest is purely the parcel side.
//!
//! A statement may be vested at most once — re-posting is rejected rather than
//! creating a second Buy. The created Buy is immutable (`PUT /trades` → 422) and
//! never deleted individually; `DELETE /ess_statements/:id` removes it.

use crate::entities::trade::{self, Trade};
use crate::infra::http::ApiError;
use axum::{
    Json, Router,
    extract::{Path, State},
    http::StatusCode,
    routing::post,
};
use rust_decimal::Decimal;
use sqlx::{Row, SqlitePool};

#[derive(Debug)]
pub enum VestError {
    Db(sqlx::Error),
    /// No ess_statements row with that id.
    StatementNotFound,
    /// The statement already has a vest Buy. Delete the statement to redo it.
    AlreadyVested,
    /// The statement's quantity or per-share market value is not positive —
    /// there is no parcel to create.
    NothingToVest,
}

impl From<sqlx::Error> for VestError {
    fn from(e: sqlx::Error) -> Self {
        VestError::Db(e)
    }
}

pub fn router() -> Router<SqlitePool> {
    Router::new().route("/ess_statements/{id}/vest", post(vest))
}

/// Create the statement's cost-base-reset Buy and link it, atomically.
pub async fn db_vest(pool: &SqlitePool, statement_id: i64) -> Result<Trade, VestError> {
    let mut tx = pool.begin().await?;

    let row = sqlx::query(
        "SELECT listing_id, holding_account_id, taxing_point_date, quantity, \
                market_value_per_share, currency \
         FROM ess_statements WHERE id = ?",
    )
    .bind(statement_id)
    .fetch_optional(&mut *tx)
    .await?;
    let row = match row {
        Some(r) => r,
        None => return Err(VestError::StatementNotFound),
    };

    let already: bool =
        sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM trades WHERE ess_statement_id = ?)")
            .bind(statement_id)
            .fetch_one(&mut *tx)
            .await?;
    if already {
        return Err(VestError::AlreadyVested);
    }

    let listing_id: i64 = row.try_get("listing_id")?;
    let holding_account_id: i64 = row.try_get("holding_account_id")?;
    let taxing_point_date: chrono::NaiveDate = row.try_get("taxing_point_date")?;
    let quantity = crate::infra::decimal::parse_dec("quantity", row.try_get("quantity")?)?;
    let price = crate::infra::decimal::parse_dec(
        "market_value_per_share",
        row.try_get("market_value_per_share")?,
    )?;
    let currency: String = row.try_get("currency")?;

    if quantity <= Decimal::ZERO || price <= Decimal::ZERO {
        return Err(VestError::NothingToVest);
    }

    // The cost-base-reset Buy: market value at the taxing point, dated and
    // settled on that date, in the statement's currency. ESS-vested units are
    // issued by the plan, not market-settled, so settlement is the trade date.
    let new_id: i64 = sqlx::query_scalar("SELECT COALESCE(MAX(id), 0) + 1 FROM trades")
        .fetch_one(&mut *tx)
        .await?;
    sqlx::query(
        "INSERT INTO trades \
         (id, trade_type, date, settlement_date, listing_id, average_price, quantity, \
          currency, brokerage, gst_on_brokerage, brokerage_currency, fx_rate, \
          holding_account_id, ess_statement_id) \
         VALUES (?, 'Buy', ?, ?, ?, ?, ?, ?, '0', '0', ?, '1', ?, ?)",
    )
    .bind(new_id)
    .bind(taxing_point_date)
    .bind(taxing_point_date)
    .bind(listing_id)
    .bind(price.to_string())
    .bind(quantity.to_string())
    .bind(&currency)
    .bind(&currency)
    .bind(holding_account_id)
    .bind(statement_id)
    .execute(&mut *tx)
    .await?;

    tx.commit().await?;

    trade::db_get(pool, new_id)
        .await?
        .ok_or_else(|| VestError::Db(sqlx::Error::RowNotFound))
}

async fn vest(
    State(pool): State<SqlitePool>,
    Path(statement_id): Path<i64>,
) -> Result<(StatusCode, Json<Trade>), ApiError> {
    let trade = db_vest(&pool, statement_id).await?;
    Ok((StatusCode::CREATED, Json(trade)))
}

impl From<VestError> for ApiError {
    fn from(e: VestError) -> Self {
        match e {
            VestError::StatementNotFound => ApiError::not_found("no ESS statement with that id"),
            VestError::AlreadyVested => ApiError::unprocessable(
                "this ESS statement has already been vested — delete it first to redo it",
            ),
            VestError::NothingToVest => ApiError::unprocessable(
                "the ESS statement's quantity and per-share market value must both be greater \
                 than zero to create the vest parcel",
            ),
            VestError::Db(err) => err.into(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::entities::trade::TradeType;
    use crate::entities::{ess_statement, listing};
    use crate::test_support::{self, test_pool, ymd};
    use axum::{body::Body, http::Request};
    use chrono::NaiveDate;
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    async fn insert_listing(pool: &SqlitePool, id: i64, currency: &str) {
        test_support::listing(id)
            .ticker(&format!("ESS{id}"))
            .name(&format!("ESS {id}"))
            .security_type(listing::SecurityType::Share)
            .currency(currency)
            .insert(pool)
            .await;
    }

    async fn insert_statement(pool: &SqlitePool, id: i64, qty: &str, price: &str, currency: &str) {
        test_support::ess_statement(id, 1, ymd(2024, 9, 1))
            .with(|s| {
                s.quantity = qty.parse().unwrap();
                s.market_value_per_share = price.parse().unwrap();
                s.deferral_discount = Decimal::from(600);
                s.currency = currency.to_string();
            })
            .insert(pool)
            .await;
    }

    #[tokio::test]
    async fn vest_creates_the_cost_base_reset_buy() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "AUD").await;
        insert_statement(&pool, 1, "100", "6", "AUD").await;

        let trade = db_vest(&pool, 1).await.unwrap();
        assert_eq!(trade.trade_type, TradeType::Buy);
        assert_eq!(trade.listing_id, 1);
        assert_eq!(trade.quantity, Decimal::from(100));
        assert_eq!(trade.average_price, Decimal::from(6));
        assert_eq!(trade.date, NaiveDate::from_ymd_opt(2024, 9, 1).unwrap());
        assert_eq!(
            trade.settlement_date,
            NaiveDate::from_ymd_opt(2024, 9, 1).unwrap()
        );
        assert_eq!(trade.brokerage, Decimal::ZERO);
        assert_eq!(trade.deemed_acquisition_date, None);
        assert_eq!(trade.ess_statement_id, Some(1));
        assert_eq!(trade.holding_account_id, 1);
    }

    /// The cost base = quantity × market value (price × qty + 0 brokerage), so
    /// the parcel carries the full taxing-point market value as its cost base.
    #[tokio::test]
    async fn vest_buy_carries_market_value_as_cost_base() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "AUD").await;
        insert_statement(&pool, 1, "100", "6", "AUD").await;
        let trade = db_vest(&pool, 1).await.unwrap();
        assert_eq!(trade.average_price * trade.quantity, Decimal::from(600));
    }

    #[tokio::test]
    async fn vest_keeps_the_statements_currency() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "USD").await;
        insert_statement(&pool, 1, "100", "6", "USD").await;
        let trade = db_vest(&pool, 1).await.unwrap();
        assert_eq!(trade.currency, "USD");
        assert_eq!(trade.brokerage_currency, "USD");
        assert_eq!(trade.fx_rate, Decimal::ONE);
    }

    #[tokio::test]
    async fn missing_statement_is_not_found() {
        let pool = test_pool().await;
        assert!(matches!(
            db_vest(&pool, 99).await,
            Err(VestError::StatementNotFound)
        ));
    }

    #[tokio::test]
    async fn second_vest_is_rejected_and_only_one_buy_exists() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "AUD").await;
        insert_statement(&pool, 1, "100", "6", "AUD").await;
        db_vest(&pool, 1).await.unwrap();
        assert!(matches!(
            db_vest(&pool, 1).await,
            Err(VestError::AlreadyVested)
        ));
        let n: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM trades WHERE ess_statement_id = 1")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(n, 1);
    }

    #[tokio::test]
    async fn zero_quantity_is_rejected() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "AUD").await;
        insert_statement(&pool, 1, "0", "6", "AUD").await;
        assert!(matches!(
            db_vest(&pool, 1).await,
            Err(VestError::NothingToVest)
        ));
    }

    /// The vest Buy is immutable individually and only removed by deleting the
    /// statement; deleting the statement removes it (when not drawn on).
    #[tokio::test]
    async fn deleting_the_statement_removes_the_vest_buy() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "AUD").await;
        insert_statement(&pool, 1, "100", "6", "AUD").await;
        let trade = db_vest(&pool, 1).await.unwrap();

        // PUT /trades on the vest Buy → rejected.
        let mut edited = trade.clone();
        edited.quantity = Decimal::from(999);
        assert!(matches!(
            trade::db_upsert(&pool, &edited).await,
            Err(trade::UpsertError::EssVestTrade)
        ));
        // DELETE /trades on the vest Buy → refused.
        assert_eq!(
            trade::db_delete(&pool, trade.id).await.unwrap(),
            trade::DeleteOutcome::Referenced
        );
        // The vest-side fields are frozen while vested…
        let mut s = ess_statement::db_get(&pool, 1).await.unwrap().unwrap();
        s.quantity = Decimal::from(999);
        assert!(matches!(
            ess_statement::db_upsert(&pool, &s).await,
            Err(ess_statement::UpsertError::Vested)
        ));
        // …but the income side stays editable (the employer's annual ESS
        // statement arrives after the vest is recorded).
        let mut s = ess_statement::db_get(&pool, 1).await.unwrap().unwrap();
        s.deferral_discount = Decimal::from(700);
        ess_statement::db_upsert(&pool, &s).await.unwrap();

        // Deleting the statement removes the vest Buy too.
        assert_eq!(
            ess_statement::db_delete(&pool, 1).await.unwrap(),
            ess_statement::DeleteOutcome::Deleted
        );
        assert!(trade::db_get(&pool, trade.id).await.unwrap().is_none());
    }

    /// While the vest Buy is drawn on by a Sell, deleting the statement is
    /// refused (it would orphan the allocation).
    #[tokio::test]
    async fn statement_delete_refused_while_vest_is_sold() {
        use crate::entities::sell::{self, AllocationInput, SellBody};
        let pool = test_pool().await;
        insert_listing(&pool, 1, "AUD").await;
        insert_statement(&pool, 1, "100", "6", "AUD").await;
        let vest = db_vest(&pool, 1).await.unwrap();

        sell::db_upsert_sell(
            &pool,
            50,
            &SellBody {
                brokerage_includes_gst: false,
                statement_total: None,
                holding_account_id: 1,
                date: NaiveDate::from_ymd_opt(2025, 1, 10).unwrap(),
                settlement_date: Some(NaiveDate::from_ymd_opt(2025, 1, 12).unwrap()),
                listing_id: 1,
                average_price: Decimal::from(9),
                quantity: Decimal::from(40),
                currency: "AUD".to_string(),
                brokerage: Decimal::ZERO,
                gst_on_brokerage: Decimal::ZERO,
                brokerage_currency: "AUD".to_string(),
                fx_rate: Decimal::ONE,
                contract_note_ref: None,
                allocations: vec![AllocationInput {
                    purchase_trade_id: vest.id,
                    quantity_allocated: Decimal::from(40),
                }],
            },
        )
        .await
        .unwrap();

        assert_eq!(
            ess_statement::db_delete(&pool, 1).await.unwrap(),
            ess_statement::DeleteOutcome::VestDrawnOn
        );
        // Removing the sale frees the statement to delete.
        assert_eq!(
            sell::db_delete_sell(&pool, 50).await.unwrap(),
            sell::DeleteOutcome::Deleted
        );
        assert_eq!(
            ess_statement::db_delete(&pool, 1).await.unwrap(),
            ess_statement::DeleteOutcome::Deleted
        );
    }

    #[tokio::test]
    async fn api_vest_returns_201_with_trade() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "AUD").await;
        insert_statement(&pool, 1, "100", "6", "AUD").await;
        let resp = router()
            .with_state(pool)
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/ess_statements/1/vest")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::CREATED);
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let trade: Trade = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(trade.trade_type, TradeType::Buy);
        assert_eq!(trade.quantity, Decimal::from(100));
    }

    #[tokio::test]
    async fn api_vest_missing_statement_returns_404() {
        let pool = test_pool().await;
        let resp = router()
            .with_state(pool)
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/ess_statements/99/vest")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }
}
