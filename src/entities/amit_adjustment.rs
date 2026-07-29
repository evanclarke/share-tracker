use crate::infra::decimal::{Money, parse_dec};
use crate::infra::http::{self, ApiError, CrudEntity};
use axum::{
    Json, Router,
    extract::{Path, State},
    http::StatusCode,
    routing::get,
};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use sqlx::{Row, SqlitePool};
use std::collections::HashMap;

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct AmitAdjustment {
    pub id: i64,
    pub amma_statement_id: i64,
    pub trade_id: i64,
    /// Units of the parcel covered by the statement's per-unit adjustment,
    /// expressed in the parcel's *as-acquired* units (the same basis as
    /// `trade.quantity`, which caps it). If a share split/consolidation
    /// intervened before the statement, enter as-acquired units and scale the
    /// statement's per-unit figure accordingly.
    #[sqlx(try_from = "Money")]
    pub quantity: Decimal,
}

#[derive(Debug, Deserialize)]
pub struct AmitAdjustmentBody {
    pub amma_statement_id: i64,
    pub trade_id: i64,
    pub quantity: Decimal,
}

impl CrudEntity for AmitAdjustment {
    type Key = i64;
    const TABLE: &'static str = "amit_adjustments";
    const COLUMNS: &'static str = "id, amma_statement_id, trade_id, quantity";
    const NOUN: &'static str = "AMIT adjustment";
}

pub fn router() -> Router<SqlitePool> {
    Router::new()
        .route(
            "/amit_adjustments",
            get(http::list_handler::<AmitAdjustment>),
        )
        .route(
            "/amit_adjustments/{id}",
            get(http::get_handler::<AmitAdjustment>)
                .put(upsert)
                .delete(http::delete_handler::<AmitAdjustment>),
        )
}

#[cfg(test)]
pub async fn db_get(pool: &SqlitePool, id: i64) -> Result<Option<AmitAdjustment>, sqlx::Error> {
    http::crud_get(pool, id).await
}

#[derive(thiserror::Error, Debug)]
pub enum UpsertError {
    #[error("AMIT adjustment write failed: {0}")]
    Db(#[from] sqlx::Error),
    #[error("the adjusted trade is not a Buy or DRP parcel")]
    TradeNotBuyOrDrp,
    #[error("the trade's listing differs from the AMMA statement's listing")]
    ListingMismatch,
    /// The trade sits in a different holding account from the AMMA
    /// statement's: a registry issues one statement per holder account, so a
    /// statement only ever adjusts its own account's parcels.
    #[error("the trade sits in a different holding account from the AMMA statement")]
    HoldingAccountMismatch,
    #[error("the adjusted quantity exceeds the trade's quantity")]
    QuantityExceedsTrade,
}

pub async fn db_upsert(pool: &SqlitePool, adj: &AmitAdjustment) -> Result<(), UpsertError> {
    use crate::entities::trade::TradeType;

    let trade_type: TradeType = sqlx::query_scalar("SELECT trade_type FROM trades WHERE id = ?")
        .bind(adj.trade_id)
        .fetch_one(pool)
        .await?;
    if !trade_type.is_acquisition() {
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
    .bind(Money(adj.quantity))
    .execute(pool)
    .await?;
    Ok(())
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
    let cutoff = crate::infra::date::as_of_or_open(up_to);
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

/// The itemised detail behind [`db_cost_base_reductions`]: every AMMA
/// statement adjusting a purchase parcel, keyed by `trade_id`, each carrying
/// its own whole-parcel reduction (`adjustment.quantity ×
/// amma.cost_base_adjustment`) — the [`crate::domain::cost_base::
/// AmitReductionEvent`] input `domain::cost_base::adjustment_detail` needs to
/// itemise the E10 walk instead of `db_cost_base_reductions`' single summed
/// total. Sorted by `tax_year_end_date` (then statement id) within each
/// trade, matching the year-order `adjustment_detail`'s running floor
/// assumes. Generic over the executor so a report can read it on its own
/// transaction.
pub async fn db_cost_base_reduction_detail<'e, E>(
    executor: E,
) -> Result<HashMap<i64, Vec<crate::domain::cost_base::AmitReductionEvent>>, sqlx::Error>
where
    E: sqlx::Executor<'e, Database = sqlx::Sqlite>,
{
    let rows = sqlx::query(
        "SELECT aa.trade_id, a.id AS amma_statement_id, a.tax_year_end_date, aa.quantity, \
         a.cost_base_adjustment \
         FROM amit_adjustments aa \
         JOIN amma_statements a ON a.id = aa.amma_statement_id \
         ORDER BY aa.trade_id, a.tax_year_end_date, a.id",
    )
    .fetch_all(executor)
    .await?;

    let mut map: HashMap<i64, Vec<crate::domain::cost_base::AmitReductionEvent>> = HashMap::new();
    for row in &rows {
        let trade_id: i64 = row.try_get("trade_id")?;
        let amma_statement_id: i64 = row.try_get("amma_statement_id")?;
        let tax_year_end_date = row.try_get("tax_year_end_date")?;
        let qty = parse_dec("quantity", row.try_get("quantity")?)?;
        let cba = parse_dec("cost_base_adjustment", row.try_get("cost_base_adjustment")?)?;
        map.entry(trade_id)
            .or_default()
            .push(crate::domain::cost_base::AmitReductionEvent {
                amma_statement_id,
                tax_year_end_date,
                amount: qty * cba,
            });
    }
    Ok(map)
}

async fn upsert(
    State(pool): State<SqlitePool>,
    Path(id): Path<i64>,
    Json(body): Json<AmitAdjustmentBody>,
) -> Result<StatusCode, ApiError> {
    let adj = AmitAdjustment {
        id,
        amma_statement_id: body.amma_statement_id,
        trade_id: body.trade_id,
        quantity: body.quantity,
    };
    db_upsert(&pool, &adj).await?;
    Ok(StatusCode::NO_CONTENT)
}

impl From<UpsertError> for ApiError {
    fn from(e: UpsertError) -> Self {
        match e {
            UpsertError::TradeNotBuyOrDrp => {
                ApiError::unprocessable("the adjusted trade is not a Buy or DRP parcel")
            }
            UpsertError::ListingMismatch => ApiError::unprocessable(
                "the trade's listing differs from the AMMA statement's listing",
            ),
            UpsertError::HoldingAccountMismatch => ApiError::unprocessable(
                "the trade sits in a different holding account from the AMMA statement — \
                 a statement only adjusts its own account's parcels",
            ),
            UpsertError::QuantityExceedsTrade => {
                ApiError::unprocessable("the adjusted quantity exceeds the trade's quantity")
            }
            UpsertError::Db(err) => err.into(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{self, ApiClient, dec, test_pool, ymd};

    /// Client over this module's own routes.
    fn client(pool: &SqlitePool) -> ApiClient {
        ApiClient::over(router().with_state(pool.clone()))
    }

    async fn insert_test_listing(pool: &SqlitePool, id: i64, exchange_mic: &str, ticker: &str) {
        test_support::listing(id)
            .mic(exchange_mic)
            .ticker(ticker)
            .name(ticker)
            .amit(true)
            .insert(pool)
            .await;
    }

    async fn insert_buy_trade(pool: &SqlitePool, id: i64, listing_id: i64, quantity: Decimal) {
        test_support::buy(id, listing_id)
            .date(ymd(2024, 1, 15))
            .qty(quantity)
            .price(Decimal::from(100))
            .brokerage(dec("9.95"))
            .gst_on_brokerage(dec("0.995"))
            .insert(pool)
            .await;
    }

    async fn insert_sell_trade(pool: &SqlitePool, id: i64, listing_id: i64, quantity: Decimal) {
        test_support::sell(id, listing_id)
            .date(ymd(2024, 6, 1))
            .qty(quantity)
            .price(Decimal::from(120))
            .brokerage(dec("9.95"))
            .gst_on_brokerage(dec("0.995"))
            .insert(pool)
            .await;
    }

    async fn insert_amma(pool: &SqlitePool, id: i64, listing_id: i64, cost_base_adj: Decimal) {
        test_support::amma(id, listing_id)
            .cost_base_adjustment(cost_base_adj)
            .insert(pool)
            .await;
    }

    // DB-level tests

    #[tokio::test]
    async fn db_insert_and_retrieve() {
        let pool = test_pool().await;
        insert_test_listing(&pool, 1, "XASX", "VAF").await;
        insert_buy_trade(&pool, 1, 1, Decimal::from(100)).await;
        insert_amma(&pool, 1, 1, "0.05".parse().unwrap()).await;

        let adj = AmitAdjustment {
            id: 1,
            amma_statement_id: 1,
            trade_id: 1,
            quantity: Decimal::from(100),
        };
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
            &AmitAdjustment {
                id: 1,
                amma_statement_id: 1,
                trade_id: 1,
                quantity: Decimal::from(50),
            },
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
            &AmitAdjustment {
                id: 1,
                amma_statement_id: 1,
                trade_id: 1,
                quantity: Decimal::from(50),
            },
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
            &HoldingAccount {
                id: 2,
                name: "ICE Employee Plan".to_string(),
            },
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
            &AmitAdjustment {
                id: 1,
                amma_statement_id: 1,
                trade_id: 1,
                quantity: Decimal::from(50),
            },
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
            &AmitAdjustment {
                id: 1,
                amma_statement_id: 1,
                trade_id: 1,
                quantity: Decimal::from(50),
            },
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
            &AmitAdjustment {
                id: 1,
                amma_statement_id: 1,
                trade_id: 1,
                quantity: Decimal::from(101),
            },
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

        db_upsert(
            &pool,
            &AmitAdjustment {
                id: 1,
                amma_statement_id: 1,
                trade_id: 1,
                quantity: Decimal::from(100),
            },
        )
        .await
        .unwrap();
        db_upsert(
            &pool,
            &AmitAdjustment {
                id: 2,
                amma_statement_id: 2,
                trade_id: 1,
                quantity: Decimal::from(80),
            },
        )
        .await
        .unwrap();

        let reductions = db_cost_base_reductions(&pool).await.unwrap();
        // 100 * 0.05 + 80 * 0.03 = 5.00 + 2.40 = 7.40
        assert_eq!(
            reductions.get(&1).copied(),
            Some("7.40".parse::<Decimal>().unwrap())
        );
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
        test_support::amma(1, 1)
            .cost_base_adjustment(dec("0.05"))
            .with(|a| {
                a.tax_deferred_amount = dec("999.99");
                a.tax_free_amount = dec("888.88");
            })
            .insert(&pool)
            .await;

        db_upsert(
            &pool,
            &AmitAdjustment {
                id: 1,
                amma_statement_id: 1,
                trade_id: 1,
                quantity: Decimal::from(100),
            },
        )
        .await
        .unwrap();

        let reductions = db_cost_base_reductions(&pool).await.unwrap();
        // 100 * 0.05 = 5.00 — the tax-deferred/tax-free amounts are NOT added in.
        assert_eq!(
            reductions.get(&1).copied(),
            Some("5.00".parse::<Decimal>().unwrap())
        );
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
        let resp = client(&pool).put("/amit_adjustments/1", &body).await;
        assert_eq!(resp.status, StatusCode::NO_CONTENT);

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
        let resp = client(&pool).put("/amit_adjustments/1", &body).await;
        assert_eq!(resp.status, StatusCode::UNPROCESSABLE_ENTITY);
        let detail = resp.text().to_string();
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
        let resp = client(&pool).put("/amit_adjustments/1", &body).await;
        assert_eq!(resp.status, StatusCode::UNPROCESSABLE_ENTITY);
    }

    #[tokio::test]
    async fn api_list_returns_ok() {
        let pool = test_pool().await;
        insert_test_listing(&pool, 1, "XASX", "VAF").await;
        insert_buy_trade(&pool, 1, 1, Decimal::from(100)).await;
        insert_amma(&pool, 1, 1, "0.05".parse().unwrap()).await;
        db_upsert(
            &pool,
            &AmitAdjustment {
                id: 1,
                amma_statement_id: 1,
                trade_id: 1,
                quantity: Decimal::from(100),
            },
        )
        .await
        .unwrap();

        let resp = client(&pool).get("/amit_adjustments").await;
        assert_eq!(resp.status, StatusCode::OK);
        let items: Vec<AmitAdjustment> = resp.json();
        assert_eq!(items.len(), 1);
    }

    #[tokio::test]
    async fn api_get_missing_returns_404() {
        let pool = test_pool().await;
        let resp = client(&pool).get("/amit_adjustments/999").await;
        assert_eq!(resp.status, StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn api_delete_existing_returns_no_content() {
        let pool = test_pool().await;
        insert_test_listing(&pool, 1, "XASX", "VAF").await;
        insert_buy_trade(&pool, 1, 1, Decimal::from(100)).await;
        insert_amma(&pool, 1, 1, "0.05".parse().unwrap()).await;
        db_upsert(
            &pool,
            &AmitAdjustment {
                id: 1,
                amma_statement_id: 1,
                trade_id: 1,
                quantity: Decimal::from(100),
            },
        )
        .await
        .unwrap();

        let resp = client(&pool).delete("/amit_adjustments/1").await;
        assert_eq!(resp.status, StatusCode::NO_CONTENT);
    }

    #[tokio::test]
    async fn api_delete_missing_returns_404() {
        let pool = test_pool().await;
        let resp = client(&pool).delete("/amit_adjustments/999").await;
        assert_eq!(resp.status, StatusCode::NOT_FOUND);
    }
}
