use crate::entities::corporate_action;
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
    /// `trade.quantity`, which caps it). A share split/consolidation between
    /// the parcel's acquisition and the statement's year end is handled by
    /// [`reduction_for`], which re-bases this quantity into the statement
    /// year's basis before multiplying — enter the statement's per-unit figure
    /// exactly as the fund states it.
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
    /// Another row already adjusts this parcel on this statement. Applying
    /// the statement's per-unit figure to the same parcel twice reduces its
    /// cost base twice, and CGT event E10's nil floor turns an over-reduction
    /// into a capital gain that was never made — so this is a data-model
    /// invariant, not an advisory cross-check. Also enforced by the
    /// `amit_adjustments_statement_trade` UNIQUE index (migration 0022).
    #[error("this parcel already has an adjustment on this AMMA statement")]
    DuplicateParcel,
}

/// [`db_upsert`] on a caller-supplied connection, so the AMMA statement's
/// `generate_adjustments` operation can write a whole generated set inside
/// one transaction and still go through the same per-row invariants (and the
/// same `row_history` audit trail) as a hand-entered row.
pub async fn db_upsert_on(
    conn: &mut sqlx::SqliteConnection,
    adj: &AmitAdjustment,
) -> Result<(), UpsertError> {
    use crate::entities::trade::TradeType;

    let trade_type: TradeType = sqlx::query_scalar("SELECT trade_type FROM trades WHERE id = ?")
        .bind(adj.trade_id)
        .fetch_one(&mut *conn)
        .await?;
    if !trade_type.is_acquisition() {
        return Err(UpsertError::TradeNotBuyOrDrp);
    }

    let (trade_listing_id, trade_account_id): (i64, i64) =
        sqlx::query_as("SELECT listing_id, holding_account_id FROM trades WHERE id = ?")
            .bind(adj.trade_id)
            .fetch_one(&mut *conn)
            .await?;
    let (amma_listing_id, amma_account_id): (i64, i64) =
        sqlx::query_as("SELECT listing_id, holding_account_id FROM amma_statements WHERE id = ?")
            .bind(adj.amma_statement_id)
            .fetch_one(&mut *conn)
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
        .fetch_one(&mut *conn)
        .await?;
    let trade_qty: Decimal = trade_qty
        .parse()
        .map_err(|_| UpsertError::Db(sqlx::Error::Decode("invalid trade quantity".into())))?;
    if adj.quantity > trade_qty {
        return Err(UpsertError::QuantityExceedsTrade);
    }

    // One adjustment per (statement, parcel): a second row for the same
    // parcel would apply the statement's per-unit figure to it twice. Checked
    // here so the rejection carries this module's own wording; the UNIQUE
    // index behind it (migration 0022) is the backstop for any other writer.
    let duplicate: Option<i64> = sqlx::query_scalar(
        "SELECT id FROM amit_adjustments \
         WHERE amma_statement_id = ? AND trade_id = ? AND id != ?",
    )
    .bind(adj.amma_statement_id)
    .bind(adj.trade_id)
    .bind(adj.id)
    .fetch_optional(&mut *conn)
    .await?;
    if duplicate.is_some() {
        return Err(UpsertError::DuplicateParcel);
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
    .execute(&mut *conn)
    .await?;
    Ok(())
}

pub async fn db_upsert(pool: &SqlitePool, adj: &AmitAdjustment) -> Result<(), UpsertError> {
    let mut conn = pool.acquire().await?;
    db_upsert_on(&mut conn, adj).await
}

/// The whole-parcel cost-base reduction one AMMA statement applies to one
/// parcel — *the* place the two figures are multiplied, so no caller can pair
/// them on mismatched unit bases.
///
/// `quantity` is the adjustment row's covered units in the parcel's
/// *as-acquired* basis (the basis `trades.quantity` caps), while the
/// statement's per-unit `cost_base_adjustment` is stated per unit as the
/// statement's own tax year saw them. A share split/consolidation or bonus
/// issue between the parcel's acquisition and `tax_year_end_date` leaves those
/// two on different bases, so the quantity is re-based into the year-end basis
/// before multiplying (TD 2000/10: a split scales the unit count, never the
/// parcel's cost base). This is the AMIT counterpart of
/// [`corporate_action::RocEvent::per_unit_for`]'s re-basing of a
/// return-of-capital payment.
pub fn reduction_for(
    quantity: Decimal,
    cost_base_adjustment: Decimal,
    splits: &[corporate_action::SplitEvent],
    acquired: chrono::NaiveDate,
    tax_year_end_date: chrono::NaiveDate,
) -> Decimal {
    corporate_action::split_adjusted_quantity(quantity, splits, acquired, Some(tax_year_end_date))
        * cost_base_adjustment
}

/// Returns the total AMIT cost base reduction per purchase trade, keyed by `trade_id`.
/// reduction = sum of [`reduction_for`] across all linked adjustments.
/// Shared by the portfolio, realised, and unrealised reports to net AMIT tax-deferred
/// amounts off the cost base of affected parcels. Takes the caller's own connection
/// (two reads — the adjustments and the listings' split events) so the scrip-for-scrip
/// exchange (`entities::scrip_exchange`) and every report can run it inside their own
/// transaction.
pub async fn db_cost_base_reductions(
    conn: &mut sqlx::SqliteConnection,
) -> Result<HashMap<i64, Decimal>, sqlx::Error> {
    db_cost_base_reductions_up_to(conn, None).await
}

/// [`db_cost_base_reductions`] bounded to statements for years ending on or
/// before `up_to`: an adjustment arises at its statement's year end, so a
/// report valued as at an earlier date (a snapshot of a past day) must not
/// include it. `None` = no bound.
pub async fn db_cost_base_reductions_up_to(
    conn: &mut sqlx::SqliteConnection,
    up_to: Option<chrono::NaiveDate>,
) -> Result<HashMap<i64, Decimal>, sqlx::Error> {
    let cutoff = crate::infra::date::as_of_or_open(up_to);
    let splits = corporate_action::db_share_split_events(&mut *conn).await?;
    let rows = sqlx::query(
        "SELECT aa.trade_id, aa.quantity, a.cost_base_adjustment, a.tax_year_end_date, \
                t.listing_id, t.date AS trade_date \
         FROM amit_adjustments aa \
         JOIN amma_statements a ON a.id = aa.amma_statement_id \
         JOIN trades t ON t.id = aa.trade_id \
         WHERE a.tax_year_end_date <= ?",
    )
    .bind(cutoff)
    .fetch_all(&mut *conn)
    .await?;

    let mut map: HashMap<i64, Decimal> = HashMap::new();
    for row in &rows {
        let tid: i64 = row.try_get("trade_id")?;
        let listing_id: i64 = row.try_get("listing_id")?;
        let qty = parse_dec("quantity", row.try_get("quantity")?)?;
        let cba = parse_dec("cost_base_adjustment", row.try_get("cost_base_adjustment")?)?;
        *map.entry(tid).or_insert(Decimal::ZERO) += reduction_for(
            qty,
            cba,
            splits.get(&listing_id).map_or(&[][..], |v| v),
            row.try_get("trade_date")?,
            row.try_get("tax_year_end_date")?,
        );
    }
    Ok(map)
}

/// The itemised detail behind [`db_cost_base_reductions`]: every AMMA
/// statement adjusting a purchase parcel, keyed by `trade_id`, each carrying
/// its own whole-parcel reduction ([`reduction_for`]) — the
/// [`crate::domain::cost_base::AmitReductionEvent`] input
/// `domain::cost_base::adjustment_detail` needs to itemise the E10 walk
/// instead of `db_cost_base_reductions`' single summed total. Sorted by
/// `tax_year_end_date` (then statement id) within each trade, matching the
/// year-order `adjustment_detail`'s running floor assumes. Takes the caller's
/// own connection (as [`db_cost_base_reductions`] does, and for the same
/// reason) so a report can read it on its own transaction.
pub async fn db_cost_base_reduction_detail(
    conn: &mut sqlx::SqliteConnection,
) -> Result<HashMap<i64, Vec<crate::domain::cost_base::AmitReductionEvent>>, sqlx::Error> {
    let splits = corporate_action::db_share_split_events(&mut *conn).await?;
    let rows = sqlx::query(
        "SELECT aa.trade_id, a.id AS amma_statement_id, a.tax_year_end_date, aa.quantity, \
         a.cost_base_adjustment, t.listing_id, t.date AS trade_date \
         FROM amit_adjustments aa \
         JOIN amma_statements a ON a.id = aa.amma_statement_id \
         JOIN trades t ON t.id = aa.trade_id \
         ORDER BY aa.trade_id, a.tax_year_end_date, a.id",
    )
    .fetch_all(&mut *conn)
    .await?;

    let mut map: HashMap<i64, Vec<crate::domain::cost_base::AmitReductionEvent>> = HashMap::new();
    for row in &rows {
        let trade_id: i64 = row.try_get("trade_id")?;
        let listing_id: i64 = row.try_get("listing_id")?;
        let amma_statement_id: i64 = row.try_get("amma_statement_id")?;
        let tax_year_end_date = row.try_get("tax_year_end_date")?;
        let qty = parse_dec("quantity", row.try_get("quantity")?)?;
        let cba = parse_dec("cost_base_adjustment", row.try_get("cost_base_adjustment")?)?;
        map.entry(trade_id)
            .or_default()
            .push(crate::domain::cost_base::AmitReductionEvent {
                amma_statement_id,
                tax_year_end_date,
                amount: reduction_for(
                    qty,
                    cba,
                    splits.get(&listing_id).map_or(&[][..], |v| v),
                    row.try_get("trade_date")?,
                    tax_year_end_date,
                ),
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
            UpsertError::DuplicateParcel => ApiError::unprocessable(
                "this parcel already has an adjustment on this AMMA statement — \
                 edit that row instead, or the statement's per-unit adjustment would \
                 reduce the parcel's cost base twice",
            ),
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

    /// [`db_cost_base_reductions`] over a pool-acquired connection (reports
    /// call it on their own read transaction).
    async fn reductions(pool: &SqlitePool) -> HashMap<i64, Decimal> {
        let mut conn = pool.acquire().await.unwrap();
        db_cost_base_reductions(&mut conn).await.unwrap()
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

    /// One adjustment per (statement, parcel): a second row for the same
    /// parcel would apply the statement's per-unit figure to it twice, and
    /// E10's nil floor can turn that over-reduction into a capital gain that
    /// was never made. Re-saving the *same* row (same id) is still an update.
    #[tokio::test]
    async fn db_duplicate_parcel_on_one_statement_rejected() {
        let pool = test_pool().await;
        insert_test_listing(&pool, 1, "XASX", "VAF").await;
        insert_buy_trade(&pool, 1, 1, Decimal::from(100)).await;
        insert_amma(&pool, 1, 1, "0.05".parse().unwrap()).await;
        insert_amma(&pool, 2, 1, "0.03".parse().unwrap()).await;

        let first = AmitAdjustment {
            id: 1,
            amma_statement_id: 1,
            trade_id: 1,
            quantity: Decimal::from(100),
        };
        db_upsert(&pool, &first).await.unwrap();

        let err = db_upsert(
            &pool,
            &AmitAdjustment {
                id: 2,
                ..first.clone()
            },
        )
        .await
        .unwrap_err();
        assert!(matches!(err, UpsertError::DuplicateParcel));

        // Updating the existing row is not a duplicate of itself.
        db_upsert(
            &pool,
            &AmitAdjustment {
                quantity: Decimal::from(80),
                ..first.clone()
            },
        )
        .await
        .unwrap();
        // Nor is the same parcel on a *different* statement — each year's
        // statement adjusts every parcel it covers.
        db_upsert(
            &pool,
            &AmitAdjustment {
                id: 2,
                amma_statement_id: 2,
                ..first
            },
        )
        .await
        .unwrap();
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

        let reductions = reductions(&pool).await;
        // 100 * 0.05 + 80 * 0.03 = 5.00 + 2.40 = 7.40
        assert_eq!(
            reductions.get(&1).copied(),
            Some("7.40".parse::<Decimal>().unwrap())
        );
    }

    /// SCENARIOS B-24: a share split between the parcel's acquisition and the
    /// statement's year end puts the two multiplicands on different unit
    /// bases. The stored `quantity` is as-acquired; the statement's per-unit
    /// figure is per unit *as the statement year saw them*, so the quantity is
    /// re-based to the year end before multiplying — 100 as-acquired units are
    /// 200 units at the year end, and the fund's 50c/unit is a $100 reduction,
    /// not $50.
    #[tokio::test]
    async fn db_cost_base_reduction_is_re_based_across_a_split() {
        use crate::entities::corporate_action::{ActionKind, CorporateAction};
        let pool = test_pool().await;
        insert_test_listing(&pool, 1, "XASX", "VAF").await;
        test_support::buy(1, 1)
            .date(ymd(2023, 8, 1))
            .qty(Decimal::from(100))
            .price(Decimal::from(10))
            .insert(&pool)
            .await;
        corporate_action::db_upsert(
            &pool,
            &CorporateAction {
                id: 1,
                listing_id: 1,
                date: ymd(2024, 1, 15),
                kind: ActionKind::ShareSplit {
                    split_new_units: Decimal::from(2),
                    split_old_units: Decimal::ONE,
                },
            },
        )
        .await
        .unwrap();
        // FY2024 statement (year ended 30 June 2024, after the split).
        insert_amma(&pool, 1, 1, dec("0.50")).await;
        test_support::amit_adjustment(&pool, 1, 1, 1, Decimal::from(100)).await;

        assert_eq!(reductions(&pool).await.get(&1).copied(), Some(dec("100")));

        // The itemised detail the E10 walk reads agrees, figure for figure.
        let mut conn = pool.acquire().await.unwrap();
        let detail = db_cost_base_reduction_detail(&mut conn).await.unwrap();
        assert_eq!(detail[&1].len(), 1);
        assert_eq!(detail[&1][0].amount, dec("100"));

        // A parcel bought *after* the split is already on the year end's
        // basis, so nothing is re-based: 50 × $0.50 = $25.
        test_support::buy(2, 1)
            .date(ymd(2024, 3, 1))
            .qty(Decimal::from(50))
            .price(Decimal::from(5))
            .insert(&pool)
            .await;
        test_support::amit_adjustment(&pool, 2, 1, 2, Decimal::from(50)).await;
        assert_eq!(reductions(&pool).await.get(&2).copied(), Some(dec("25")));
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

        let reductions = reductions(&pool).await;
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
    async fn api_duplicate_parcel_returns_422() {
        let pool = test_pool().await;
        insert_test_listing(&pool, 1, "XASX", "VAF").await;
        insert_buy_trade(&pool, 1, 1, Decimal::from(100)).await;
        insert_amma(&pool, 1, 1, "0.05".parse().unwrap()).await;

        let body = serde_json::json!({
            "amma_statement_id": 1,
            "trade_id": 1,
            "quantity": "100"
        });
        let c = client(&pool);
        assert_eq!(
            c.put("/amit_adjustments/1", &body).await.status,
            StatusCode::NO_CONTENT
        );
        let resp = c.put("/amit_adjustments/2", &body).await;
        assert_eq!(resp.status, StatusCode::UNPROCESSABLE_ENTITY);
        let detail = resp.text().to_string();
        assert!(detail.contains("already has an adjustment"), "{detail}");
        assert!(db_get(&pool, 2).await.unwrap().is_none());
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
