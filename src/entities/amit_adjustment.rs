use axum::{
    Json, Router,
    extract::{Path, State},
    http::StatusCode,
    routing::get,
};
use crate::infra::decimal::parse_dec;
use crate::infra::http::write_error_body;
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use sqlx::{Row, SqlitePool};
use std::collections::HashMap;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AmitAdjustment {
    pub id: i64,
    pub amma_statement_id: i64,
    pub trade_id: i64,
    /// Units of the parcel covered by the statement's per-unit adjustment,
    /// expressed in the parcel's *as-acquired* units (the same basis as
    /// `trade.quantity`, which caps it). If a share split/consolidation
    /// intervened before the statement, enter as-acquired units and scale the
    /// statement's per-unit figure accordingly.
    pub quantity: Decimal,
}

impl<'r> sqlx::FromRow<'r, sqlx::sqlite::SqliteRow> for AmitAdjustment {
    fn from_row(row: &'r sqlx::sqlite::SqliteRow) -> Result<Self, sqlx::Error> {
        fn dec(s: String) -> Result<Decimal, sqlx::Error> {
            s.parse().map_err(|e: rust_decimal::Error| sqlx::Error::Decode(Box::new(e)))
        }
        Ok(AmitAdjustment {
            id: row.try_get("id")?,
            amma_statement_id: row.try_get("amma_statement_id")?,
            trade_id: row.try_get("trade_id")?,
            quantity: dec(row.try_get("quantity")?)?,
        })
    }
}

#[derive(Debug, Deserialize)]
pub struct AmitAdjustmentBody {
    pub amma_statement_id: i64,
    pub trade_id: i64,
    pub quantity: Decimal,
}

pub fn router() -> Router<SqlitePool> {
    Router::new()
        .route("/amit_adjustments", get(list))
        .route("/amit_adjustments/{id}", get(get_one).put(upsert).delete(delete))
}

pub async fn db_list(pool: &SqlitePool) -> Result<Vec<AmitAdjustment>, sqlx::Error> {
    sqlx::query_as(
        "SELECT id, amma_statement_id, trade_id, quantity FROM amit_adjustments ORDER BY id",
    )
    .fetch_all(pool)
    .await
}

pub async fn db_get(pool: &SqlitePool, id: i64) -> Result<Option<AmitAdjustment>, sqlx::Error> {
    sqlx::query_as(
        "SELECT id, amma_statement_id, trade_id, quantity FROM amit_adjustments WHERE id = ?",
    )
    .bind(id)
    .fetch_optional(pool)
    .await
}

#[derive(Debug)]
pub enum UpsertError {
    Db(sqlx::Error),
    TradeNotBuyOrDrp,
    ListingMismatch,
    /// The trade sits in a different holding account from the AMMA
    /// statement's: a registry issues one statement per holder account, so a
    /// statement only ever adjusts its own account's parcels.
    HoldingAccountMismatch,
    QuantityExceedsTrade,
}

impl From<sqlx::Error> for UpsertError {
    fn from(e: sqlx::Error) -> Self {
        UpsertError::Db(e)
    }
}

pub async fn db_upsert(pool: &SqlitePool, adj: &AmitAdjustment) -> Result<(), UpsertError> {
    use crate::entities::trade::TradeType;

    let trade_type: TradeType = sqlx::query_scalar("SELECT trade_type FROM trades WHERE id = ?")
        .bind(adj.trade_id)
        .fetch_one(pool)
        .await?;
    if !matches!(trade_type, TradeType::Buy | TradeType::DRP) {
        return Err(UpsertError::TradeNotBuyOrDrp);
    }

    let (trade_listing_id, trade_account_id): (i64, i64) =
        sqlx::query_as("SELECT listing_id, holding_account_id FROM trades WHERE id = ?")
            .bind(adj.trade_id)
            .fetch_one(pool)
            .await?;
    let (amma_listing_id, amma_account_id): (i64, i64) =
        sqlx::query_as("SELECT listing_id, holding_account_id FROM amma_statements WHERE id = ?")
            .bind(adj.amma_statement_id)
            .fetch_one(pool)
            .await?;
    if trade_listing_id != amma_listing_id {
        return Err(UpsertError::ListingMismatch);
    }
    // A statement only ever adjusts parcels in its own holding account (the
    // registry issues one AMMA statement per holder account).
    if trade_account_id != amma_account_id {
        return Err(UpsertError::HoldingAccountMismatch);
    }

    let trade_qty: String = sqlx::query_scalar("SELECT quantity FROM trades WHERE id = ?")
        .bind(adj.trade_id)
        .fetch_one(pool)
        .await?;
    let trade_qty: Decimal = trade_qty
        .parse()
        .map_err(|_| UpsertError::Db(sqlx::Error::Decode("invalid trade quantity".into())))?;
    if adj.quantity > trade_qty {
        return Err(UpsertError::QuantityExceedsTrade);
    }

    sqlx::query(
        "INSERT INTO amit_adjustments (id, amma_statement_id, trade_id, quantity) \
         VALUES (?, ?, ?, ?) \
         ON CONFLICT(id) DO UPDATE SET \
             amma_statement_id = excluded.amma_statement_id, \
             trade_id          = excluded.trade_id, \
             quantity          = excluded.quantity",
    )
    .bind(adj.id)
    .bind(adj.amma_statement_id)
    .bind(adj.trade_id)
    .bind(adj.quantity.to_string())
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn db_delete(pool: &SqlitePool, id: i64) -> Result<bool, sqlx::Error> {
    let result = sqlx::query("DELETE FROM amit_adjustments WHERE id = ?")
        .bind(id)
        .execute(pool)
        .await?;
    Ok(result.rows_affected() > 0)
}

/// Returns the total AMIT cost base reduction per purchase trade, keyed by `trade_id`.
/// reduction = sum(adjustment.quantity * amma.cost_base_adjustment) across all linked adjustments.
/// Shared by the portfolio, realised, and unrealised reports to net AMIT tax-deferred
/// amounts off the cost base of affected parcels. Generic over the executor so the
/// scrip-for-scrip exchange (`entities::scrip_exchange`) can compute carried cost
/// bases inside its own transaction.
pub async fn db_cost_base_reductions<'e, E>(
    executor: E,
) -> Result<HashMap<i64, Decimal>, sqlx::Error>
where
    E: sqlx::Executor<'e, Database = sqlx::Sqlite>,
{
    db_cost_base_reductions_up_to(executor, None).await
}

/// [`db_cost_base_reductions`] bounded to statements for years ending on or
/// before `up_to`: an adjustment arises at its statement's year end, so a
/// report valued as at an earlier date (a snapshot of a past day) must not
/// include it. `None` = no bound.
pub async fn db_cost_base_reductions_up_to<'e, E>(
    executor: E,
    up_to: Option<chrono::NaiveDate>,
) -> Result<HashMap<i64, Decimal>, sqlx::Error>
where
    E: sqlx::Executor<'e, Database = sqlx::Sqlite>,
{
    let cutoff = up_to.unwrap_or_else(|| chrono::NaiveDate::from_ymd_opt(9999, 12, 31).unwrap());
    let rows = sqlx::query(
        "SELECT aa.trade_id, aa.quantity, a.cost_base_adjustment \
         FROM amit_adjustments aa \
         JOIN amma_statements a ON a.id = aa.amma_statement_id \
         WHERE a.tax_year_end_date <= ?",
    )
    .bind(cutoff)
    .fetch_all(executor)
    .await?;

    let mut map: HashMap<i64, Decimal> = HashMap::new();
    for row in &rows {
        let tid: i64 = row.try_get("trade_id")?;
        let qty = parse_dec("quantity", row.try_get("quantity")?)?;
        let cba = parse_dec("cost_base_adjustment", row.try_get("cost_base_adjustment")?)?;
        *map.entry(tid).or_insert(Decimal::ZERO) += qty * cba;
    }
    Ok(map)
}

async fn list(State(pool): State<SqlitePool>) -> Result<Json<Vec<AmitAdjustment>>, StatusCode> {
    db_list(&pool)
        .await
        .map(Json)
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)
}

async fn get_one(
    State(pool): State<SqlitePool>,
    Path(id): Path<i64>,
) -> Result<Json<AmitAdjustment>, StatusCode> {
    db_get(&pool, id)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
        .map(Json)
        .ok_or(StatusCode::NOT_FOUND)
}

async fn upsert(
    State(pool): State<SqlitePool>,
    Path(id): Path<i64>,
    Json(body): Json<AmitAdjustmentBody>,
) -> Result<StatusCode, (StatusCode, String)> {
    let adj = AmitAdjustment {
        id,
        amma_statement_id: body.amma_statement_id,
        trade_id: body.trade_id,
        quantity: body.quantity,
    };
    let unprocessable = |msg: &str| Err((StatusCode::UNPROCESSABLE_ENTITY, msg.to_string()));
    match db_upsert(&pool, &adj).await {
        Ok(()) => Ok(StatusCode::NO_CONTENT),
        Err(UpsertError::TradeNotBuyOrDrp) => {
            unprocessable("the adjusted trade is not a Buy or DRP parcel")
        }
        Err(UpsertError::ListingMismatch) => {
            unprocessable("the trade's listing differs from the AMMA statement's listing")
        }
        Err(UpsertError::HoldingAccountMismatch) => unprocessable(
            "the trade sits in a different holding account from the AMMA statement — \
             a statement only adjusts its own account's parcels",
        ),
        Err(UpsertError::QuantityExceedsTrade) => {
            unprocessable("the adjusted quantity exceeds the trade's quantity")
        }
        Err(UpsertError::Db(e)) => Err(write_error_body(&e)),
    }
}

async fn delete(
    State(pool): State<SqlitePool>,
    Path(id): Path<i64>,
) -> Result<StatusCode, (StatusCode, String)> {
    db_delete(&pool, id)
        .await
        .map(|found| if found { StatusCode::NO_CONTENT } else { StatusCode::NOT_FOUND })
        .map_err(|e| write_error_body(&e))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{infra::db, entities::{amma, listing, trade}};
    use axum::{body::Body, http::Request};
    use chrono::NaiveDate;
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    async fn test_pool() -> SqlitePool {
        db::init(":memory:").await.unwrap()
    }

    async fn insert_test_listing(pool: &SqlitePool, id: i64, exchange_mic: &str, ticker: &str) {
        listing::db_upsert(
            pool,
            &listing::Listing {
                id,
                exchange_mic: Some(exchange_mic.to_string()),
                ticker: ticker.to_string(),
                name: ticker.to_string(),
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

    async fn insert_buy_trade(pool: &SqlitePool, id: i64, listing_id: i64, quantity: Decimal) {
        trade::db_upsert(
            pool,
            &trade::Trade {
                brokerage_includes_gst: false,
                statement_total: None,
                holding_account_id: 1,
                transfer_id: None,
                ess_statement_id: None,
                id,
                trade_type: trade::TradeType::Buy,
                date: NaiveDate::from_ymd_opt(2024, 1, 15).unwrap(),
                settlement_date: NaiveDate::from_ymd_opt(2024, 1, 17).unwrap(),
                listing_id,
                average_price: Decimal::from(100),
                quantity,
                currency: "AUD".to_string(),
                brokerage: "9.95".parse().unwrap(),
                gst_on_brokerage: "0.995".parse().unwrap(),
                brokerage_currency: "AUD".to_string(),
                fx_rate: Decimal::ONE,
                contract_note_ref: None,
                residual_brought_forward: Decimal::ZERO,
                residual_carried_forward: Decimal::ZERO,
                residual_paid_out: Decimal::ZERO,
                rights_action_id: None,
                buyback_action_id: None,
                scrip_action_id: None,
                demerger_action_id: None,
                deemed_acquisition_date: None,
            },
        )
        .await
        .unwrap();
    }

    async fn insert_sell_trade(pool: &SqlitePool, id: i64, listing_id: i64, quantity: Decimal) {
        trade::db_upsert(
            pool,
            &trade::Trade {
                brokerage_includes_gst: false,
                statement_total: None,
                holding_account_id: 1,
                transfer_id: None,
                ess_statement_id: None,
                id,
                trade_type: trade::TradeType::Sell,
                date: NaiveDate::from_ymd_opt(2024, 6, 1).unwrap(),
                settlement_date: NaiveDate::from_ymd_opt(2024, 6, 3).unwrap(),
                listing_id,
                average_price: Decimal::from(120),
                quantity,
                currency: "AUD".to_string(),
                brokerage: "9.95".parse().unwrap(),
                gst_on_brokerage: "0.995".parse().unwrap(),
                brokerage_currency: "AUD".to_string(),
                fx_rate: Decimal::ONE,
                contract_note_ref: None,
                residual_brought_forward: Decimal::ZERO,
                residual_carried_forward: Decimal::ZERO,
                residual_paid_out: Decimal::ZERO,
                rights_action_id: None,
                buyback_action_id: None,
                scrip_action_id: None,
                demerger_action_id: None,
                deemed_acquisition_date: None,
            },
        )
        .await
        .unwrap();
    }

    async fn insert_amma(pool: &SqlitePool, id: i64, listing_id: i64, cost_base_adj: Decimal) {
        amma::db_upsert(
            pool,
            &amma::AmmaStatement {
                holding_account_id: 1,
                id,
                listing_id,
                tax_year_end_date: NaiveDate::from_ymd_opt(2024, 6, 30).unwrap(),
                units_held: Decimal::from(100),
                date_received: NaiveDate::from_ymd_opt(2024, 8, 15).unwrap(),
                australian_interest: Decimal::ZERO,
                australian_dividends_unfranked: Decimal::ZERO,
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
                tax_deferred_amount: Decimal::ZERO,
                tax_free_amount: Decimal::ZERO,
                cost_base_adjustment: cost_base_adj,
                tfn_withholding_tax: Decimal::ZERO,
                currency: "AUD".to_string(),
            },
        )
        .await
        .unwrap();
    }

    // DB-level tests

    #[tokio::test]
    async fn db_insert_and_retrieve() {
        let pool = test_pool().await;
        insert_test_listing(&pool, 1, "XASX", "VAF").await;
        insert_buy_trade(&pool, 1, 1, Decimal::from(100)).await;
        insert_amma(&pool, 1, 1, "0.05".parse().unwrap()).await;

        let adj = AmitAdjustment { id: 1, amma_statement_id: 1, trade_id: 1, quantity: Decimal::from(100) };
        db_upsert(&pool, &adj).await.unwrap();

        let got = db_get(&pool, 1).await.unwrap().unwrap();
        assert_eq!(got.amma_statement_id, 1);
        assert_eq!(got.trade_id, 1);
        assert_eq!(got.quantity, Decimal::from(100));
    }

    #[tokio::test]
    async fn db_sell_trade_rejected() {
        let pool = test_pool().await;
        insert_test_listing(&pool, 1, "XASX", "VAF").await;
        insert_sell_trade(&pool, 1, 1, Decimal::from(50)).await;
        insert_amma(&pool, 1, 1, "0.05".parse().unwrap()).await;

        let err = db_upsert(
            &pool,
            &AmitAdjustment { id: 1, amma_statement_id: 1, trade_id: 1, quantity: Decimal::from(50) },
        )
        .await
        .unwrap_err();
        assert!(matches!(err, UpsertError::TradeNotBuyOrDrp));
    }

    #[tokio::test]
    async fn db_listing_mismatch_rejected() {
        let pool = test_pool().await;
        insert_test_listing(&pool, 1, "XASX", "VAF").await;
        insert_test_listing(&pool, 2, "XASX", "VAS").await;
        insert_buy_trade(&pool, 1, 1, Decimal::from(100)).await;
        insert_amma(&pool, 1, 2, "0.05".parse().unwrap()).await; // different listing

        let err = db_upsert(
            &pool,
            &AmitAdjustment { id: 1, amma_statement_id: 1, trade_id: 1, quantity: Decimal::from(50) },
        )
        .await
        .unwrap_err();
        assert!(matches!(err, UpsertError::ListingMismatch));
    }

    /// A statement only adjusts parcels in its own holding account: the same
    /// listing's parcel in another account is rejected (the registry issues
    /// one AMMA statement per holder account).
    #[tokio::test]
    async fn db_holding_account_mismatch_rejected() {
        use crate::entities::holding_account::{self, HoldingAccount};
        let pool = test_pool().await;
        insert_test_listing(&pool, 1, "XASX", "VAF").await;
        holding_account::db_upsert(
            &pool,
            &HoldingAccount { id: 2, name: "ICE Employee Plan".to_string() },
        )
        .await
        .unwrap();
        insert_buy_trade(&pool, 1, 1, Decimal::from(100)).await;
        sqlx::query("UPDATE trades SET holding_account_id = 2 WHERE id = 1")
            .execute(&pool)
            .await
            .unwrap();
        insert_amma(&pool, 1, 1, "0.05".parse().unwrap()).await; // default account

        let err = db_upsert(
            &pool,
            &AmitAdjustment { id: 1, amma_statement_id: 1, trade_id: 1, quantity: Decimal::from(50) },
        )
        .await
        .unwrap_err();
        assert!(matches!(err, UpsertError::HoldingAccountMismatch));

        // Re-pointing the statement at the parcel's account fixes it.
        sqlx::query("UPDATE amma_statements SET holding_account_id = 2 WHERE id = 1")
            .execute(&pool)
            .await
            .unwrap();
        db_upsert(
            &pool,
            &AmitAdjustment { id: 1, amma_statement_id: 1, trade_id: 1, quantity: Decimal::from(50) },
        )
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn db_quantity_exceeds_trade_rejected() {
        let pool = test_pool().await;
        insert_test_listing(&pool, 1, "XASX", "VAF").await;
        insert_buy_trade(&pool, 1, 1, Decimal::from(100)).await;
        insert_amma(&pool, 1, 1, "0.05".parse().unwrap()).await;

        let err = db_upsert(
            &pool,
            &AmitAdjustment { id: 1, amma_statement_id: 1, trade_id: 1, quantity: Decimal::from(101) },
        )
        .await
        .unwrap_err();
        assert!(matches!(err, UpsertError::QuantityExceedsTrade));
    }

    #[tokio::test]
    async fn db_cost_base_reduction_calculation() {
        let pool = test_pool().await;
        insert_test_listing(&pool, 1, "XASX", "VAF").await;
        insert_buy_trade(&pool, 1, 1, Decimal::from(100)).await;
        // Two AMMA statements: $0.05/unit and $0.03/unit
        insert_amma(&pool, 1, 1, "0.05".parse().unwrap()).await;
        insert_amma(&pool, 2, 1, "0.03".parse().unwrap()).await;

        db_upsert(&pool, &AmitAdjustment { id: 1, amma_statement_id: 1, trade_id: 1, quantity: Decimal::from(100) })
            .await
            .unwrap();
        db_upsert(&pool, &AmitAdjustment { id: 2, amma_statement_id: 2, trade_id: 1, quantity: Decimal::from(80) })
            .await
            .unwrap();

        let reductions = db_cost_base_reductions(&pool).await.unwrap();
        // 100 * 0.05 + 80 * 0.03 = 5.00 + 2.40 = 7.40
        assert_eq!(reductions.get(&1).copied(), Some("7.40".parse::<Decimal>().unwrap()));
    }

    /// The cost base adjustment is driven solely by `cost_base_adjustment` (the per-unit
    /// AMIT cost base net amount), per ATO guidance (docs/ato/amit-cost-base-adjustments.md).
    /// `tax_deferred_amount` and `tax_free_amount` are informational-only and must NOT
    /// affect the reduction, even when large.
    #[tokio::test]
    async fn db_cost_base_reduction_ignores_tax_deferred_and_tax_free() {
        let pool = test_pool().await;
        insert_test_listing(&pool, 1, "XASX", "VAF").await;
        insert_buy_trade(&pool, 1, 1, Decimal::from(100)).await;

        // AMMA with cost_base_adjustment 0.05/unit but huge tax-deferred / tax-free lines.
        amma::db_upsert(
            &pool,
            &amma::AmmaStatement {
                holding_account_id: 1,
                id: 1,
                listing_id: 1,
                tax_year_end_date: NaiveDate::from_ymd_opt(2024, 6, 30).unwrap(),
                units_held: Decimal::from(100),
                date_received: NaiveDate::from_ymd_opt(2024, 8, 15).unwrap(),
                australian_interest: Decimal::ZERO,
                australian_dividends_unfranked: Decimal::ZERO,
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
                tax_deferred_amount: "999.99".parse().unwrap(),
                tax_free_amount: "888.88".parse().unwrap(),
                cost_base_adjustment: "0.05".parse().unwrap(),
                tfn_withholding_tax: Decimal::ZERO,
                currency: "AUD".to_string(),
            },
        )
        .await
        .unwrap();

        db_upsert(&pool, &AmitAdjustment { id: 1, amma_statement_id: 1, trade_id: 1, quantity: Decimal::from(100) })
            .await
            .unwrap();

        let reductions = db_cost_base_reductions(&pool).await.unwrap();
        // 100 * 0.05 = 5.00 — the tax-deferred/tax-free amounts are NOT added in.
        assert_eq!(reductions.get(&1).copied(), Some("5.00".parse::<Decimal>().unwrap()));
    }

    // API-level tests

    #[tokio::test]
    async fn api_upsert_and_get() {
        let pool = test_pool().await;
        insert_test_listing(&pool, 1, "XASX", "VAF").await;
        insert_buy_trade(&pool, 1, 1, Decimal::from(100)).await;
        insert_amma(&pool, 1, 1, "0.05".parse().unwrap()).await;

        let body = serde_json::json!({
            "amma_statement_id": 1,
            "trade_id": 1,
            "quantity": "100"
        });
        let resp = router()
            .with_state(pool.clone())
            .oneshot(
                Request::builder()
                    .method("PUT")
                    .uri("/amit_adjustments/1")
                    .header("content-type", "application/json")
                    .body(Body::from(body.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NO_CONTENT);

        let got = db_get(&pool, 1).await.unwrap().unwrap();
        assert_eq!(got.amma_statement_id, 1);
        assert_eq!(got.trade_id, 1);
        assert_eq!(got.quantity, Decimal::from(100));
    }

    #[tokio::test]
    async fn api_sell_trade_returns_422() {
        let pool = test_pool().await;
        insert_test_listing(&pool, 1, "XASX", "VAF").await;
        insert_sell_trade(&pool, 1, 1, Decimal::from(50)).await;
        insert_amma(&pool, 1, 1, "0.05".parse().unwrap()).await;

        let body = serde_json::json!({
            "amma_statement_id": 1,
            "trade_id": 1,
            "quantity": "50"
        });
        let resp = router()
            .with_state(pool)
            .oneshot(
                Request::builder()
                    .method("PUT")
                    .uri("/amit_adjustments/1")
                    .header("content-type", "application/json")
                    .body(Body::from(body.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY);
        let bytes = http_body_util::BodyExt::collect(resp.into_body()).await.unwrap().to_bytes();
        let detail = String::from_utf8(bytes.to_vec()).unwrap();
        assert!(detail.contains("not a Buy or DRP"), "detail: {detail}");
    }

    #[tokio::test]
    async fn api_listing_mismatch_returns_422() {
        let pool = test_pool().await;
        insert_test_listing(&pool, 1, "XASX", "VAF").await;
        insert_test_listing(&pool, 2, "XASX", "VAS").await;
        insert_buy_trade(&pool, 1, 1, Decimal::from(100)).await;
        insert_amma(&pool, 1, 2, "0.05".parse().unwrap()).await;

        let body = serde_json::json!({
            "amma_statement_id": 1,
            "trade_id": 1,
            "quantity": "50"
        });
        let resp = router()
            .with_state(pool)
            .oneshot(
                Request::builder()
                    .method("PUT")
                    .uri("/amit_adjustments/1")
                    .header("content-type", "application/json")
                    .body(Body::from(body.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY);
    }

    #[tokio::test]
    async fn api_list_returns_ok() {
        let pool = test_pool().await;
        insert_test_listing(&pool, 1, "XASX", "VAF").await;
        insert_buy_trade(&pool, 1, 1, Decimal::from(100)).await;
        insert_amma(&pool, 1, 1, "0.05".parse().unwrap()).await;
        db_upsert(&pool, &AmitAdjustment { id: 1, amma_statement_id: 1, trade_id: 1, quantity: Decimal::from(100) })
            .await
            .unwrap();

        let resp = router()
            .with_state(pool)
            .oneshot(Request::builder().uri("/amit_adjustments").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let items: Vec<AmitAdjustment> = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(items.len(), 1);
    }

    #[tokio::test]
    async fn api_get_missing_returns_404() {
        let pool = test_pool().await;
        let resp = router()
            .with_state(pool)
            .oneshot(Request::builder().uri("/amit_adjustments/999").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn api_delete_existing_returns_no_content() {
        let pool = test_pool().await;
        insert_test_listing(&pool, 1, "XASX", "VAF").await;
        insert_buy_trade(&pool, 1, 1, Decimal::from(100)).await;
        insert_amma(&pool, 1, 1, "0.05".parse().unwrap()).await;
        db_upsert(&pool, &AmitAdjustment { id: 1, amma_statement_id: 1, trade_id: 1, quantity: Decimal::from(100) })
            .await
            .unwrap();

        let resp = router()
            .with_state(pool)
            .oneshot(
                Request::builder()
                    .method("DELETE")
                    .uri("/amit_adjustments/1")
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
                    .uri("/amit_adjustments/999")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }
}
