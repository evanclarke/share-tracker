//! Employee share scheme (ESS) statement: the income side of an ESS interest
//! (docs/ato/employee-share-schemes.md). One row captures one Employee share
//! scheme statement attributed to a (listing, holding account): the Item 12
//! discount labels, the foreign-source memo, the TFN withheld, the taxing-point
//! date, and the per-share market value and quantity that vest.
//!
//! The assessable discount it carries reaches the tax summary
//! (`reports::tax_summary`), which totals D + E + F + G net of the $1,000
//! taxed-upfront reduction per Australian financial year. The CGT side is tied
//! in by the vesting operation (`entities::ess_vest`): it creates the
//! cost-base-reset Buy (quantity vested, price = market value at the taxing
//! point) linked back via `trades.ess_statement_id`.
//!
//! Integrity mirrors the corporate-action groups: while a statement's vest Buy
//! exists the statement is **frozen** against edits (`PUT` → 422; delete the
//! vest first), and deleting the statement removes its vest Buy in the same
//! transaction — **refused** (422) while that Buy is drawn on by a Sell
//! allocation or AMIT adjustment.

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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EssStatement {
    pub id: i64,
    pub listing_id: i64,
    /// The account the ESS interests vest into (an employer-plan account,
    /// typically). Defaults to the seeded default account when omitted.
    pub holding_account_id: i64,
    /// The taxing point: the Australian financial year this date falls in is the
    /// assessable year, and the vest Buy's acquisition/settlement date.
    pub taxing_point_date: NaiveDate,
    /// Shares that vest at the taxing point and their per-share market value —
    /// together the cost-base-reset Buy (quantity, price) the vesting operation
    /// creates. Positive for a vest.
    pub quantity: Decimal,
    pub market_value_per_share: Decimal,
    /// Item 12 label D: taxed-upfront discount eligible for the $1,000 reduction.
    pub taxed_upfront_eligible: Decimal,
    /// Item 12 label E: taxed-upfront discount not eligible for the reduction.
    pub taxed_upfront_not_eligible: Decimal,
    /// Item 12 label F: deferral-scheme discount (the RSU case).
    pub deferral_discount: Decimal,
    /// Pre-1 July 2009 ESS interests whose cessation time falls in the year
    /// (assessable this year, the same as the other discount labels).
    pub pre_2009_cessation_discount: Decimal,
    /// Item 12 label A: the foreign-source portion of the above discounts — a
    /// memo already counted within the discount labels, surfaced separately by
    /// the tax summary for the foreign-income/FITO calculation. Not added on top.
    pub foreign_source_discount: Decimal,
    /// Item 12 label C: TFN amounts withheld from the discounts.
    pub tfn_withholding: Decimal,
    /// ISO 4217 currency the amounts are denominated in. The tax summary
    /// converts non-AUD amounts to AUD via the ATO rate for this currency and
    /// the month of `taxing_point_date` (see `infra::fx::to_aud`). Defaults to AUD.
    pub currency: String,
    /// Statement-AUD overrides, one per discount label: the employer's annual
    /// Employee share scheme statement (and the ATO prefill) states each label
    /// in AUD at the release-date spot rate, which differs from the RBA monthly
    /// rate. When present the tax summary reports the figure verbatim for that
    /// label; absent, the label converts via the RBA rate as usual. Only
    /// accepted on a non-AUD statement (422 otherwise — two AUD figures for the
    /// same label could silently disagree).
    pub aud_taxed_upfront_eligible: Option<Decimal>,
    pub aud_taxed_upfront_not_eligible: Option<Decimal>,
    pub aud_deferral_discount: Option<Decimal>,
    pub aud_pre_2009_cessation_discount: Option<Decimal>,
    pub aud_foreign_source_discount: Option<Decimal>,
}

impl<'r> sqlx::FromRow<'r, sqlx::sqlite::SqliteRow> for EssStatement {
    fn from_row(row: &'r sqlx::sqlite::SqliteRow) -> Result<Self, sqlx::Error> {
        Ok(EssStatement {
            id: row.try_get("id")?,
            listing_id: row.try_get("listing_id")?,
            holding_account_id: row.try_get("holding_account_id")?,
            taxing_point_date: row.try_get("taxing_point_date")?,
            quantity: row_dec(row, "quantity")?,
            market_value_per_share: row_dec(row, "market_value_per_share")?,
            taxed_upfront_eligible: row_dec(row, "taxed_upfront_eligible")?,
            taxed_upfront_not_eligible: row_dec(row, "taxed_upfront_not_eligible")?,
            deferral_discount: row_dec(row, "deferral_discount")?,
            pre_2009_cessation_discount: row_dec(row, "pre_2009_cessation_discount")?,
            foreign_source_discount: row_dec(row, "foreign_source_discount")?,
            tfn_withholding: row_dec(row, "tfn_withholding")?,
            currency: row.try_get("currency")?,
            aud_taxed_upfront_eligible: row_opt_dec(row, "aud_taxed_upfront_eligible")?,
            aud_taxed_upfront_not_eligible: row_opt_dec(row, "aud_taxed_upfront_not_eligible")?,
            aud_deferral_discount: row_opt_dec(row, "aud_deferral_discount")?,
            aud_pre_2009_cessation_discount: row_opt_dec(row, "aud_pre_2009_cessation_discount")?,
            aud_foreign_source_discount: row_opt_dec(row, "aud_foreign_source_discount")?,
        })
    }
}

#[derive(Debug, Deserialize)]
pub struct EssStatementBody {
    pub listing_id: i64,
    #[serde(default = "crate::entities::holding_account::default_holding_account_id")]
    pub holding_account_id: i64,
    pub taxing_point_date: NaiveDate,
    #[serde(default)]
    pub quantity: Decimal,
    #[serde(default)]
    pub market_value_per_share: Decimal,
    #[serde(default)]
    pub taxed_upfront_eligible: Decimal,
    #[serde(default)]
    pub taxed_upfront_not_eligible: Decimal,
    #[serde(default)]
    pub deferral_discount: Decimal,
    #[serde(default)]
    pub pre_2009_cessation_discount: Decimal,
    #[serde(default)]
    pub foreign_source_discount: Decimal,
    #[serde(default)]
    pub tfn_withholding: Decimal,
    #[serde(default = "default_currency")]
    pub currency: String,
    #[serde(default)]
    pub aud_taxed_upfront_eligible: Option<Decimal>,
    #[serde(default)]
    pub aud_taxed_upfront_not_eligible: Option<Decimal>,
    #[serde(default)]
    pub aud_deferral_discount: Option<Decimal>,
    #[serde(default)]
    pub aud_pre_2009_cessation_discount: Option<Decimal>,
    #[serde(default)]
    pub aud_foreign_source_discount: Option<Decimal>,
}

fn default_currency() -> String {
    "AUD".to_string()
}

pub fn router() -> Router<SqlitePool> {
    Router::new().route("/ess_statements", get(list)).route(
        "/ess_statements/{id}",
        get(get_one).put(upsert).delete(delete),
    )
}

const COLUMNS: &str = "id, listing_id, holding_account_id, taxing_point_date, quantity, \
     market_value_per_share, taxed_upfront_eligible, taxed_upfront_not_eligible, \
     deferral_discount, pre_2009_cessation_discount, foreign_source_discount, \
     tfn_withholding, currency, aud_taxed_upfront_eligible, aud_taxed_upfront_not_eligible, \
     aud_deferral_discount, aud_pre_2009_cessation_discount, aud_foreign_source_discount";

pub async fn db_list(pool: &SqlitePool) -> Result<Vec<EssStatement>, sqlx::Error> {
    sqlx::query_as(&format!(
        "SELECT {COLUMNS} FROM ess_statements ORDER BY taxing_point_date, id"
    ))
    .fetch_all(pool)
    .await
}

pub async fn db_get(pool: &SqlitePool, id: i64) -> Result<Option<EssStatement>, sqlx::Error> {
    sqlx::query_as(&format!(
        "SELECT {COLUMNS} FROM ess_statements WHERE id = ?"
    ))
    .bind(id)
    .fetch_optional(pool)
    .await
}

#[derive(Debug)]
pub enum UpsertError {
    Db(sqlx::Error),
    /// The statement already has a vest Buy (`trades.ess_statement_id`) and the
    /// edit changes a field that Buy was created from (listing, account, taxing
    /// point, quantity, market value, or currency), which would desync it.
    /// Income-side fields (the discount labels, TFN withheld, the statement-AUD
    /// overrides) stay editable — the employer's annual ESS statement arrives
    /// after the vest is recorded. Delete the statement (which removes the vest)
    /// and re-enter to change the vest side. Mapped to 422.
    Vested,
    /// A statement-AUD override was supplied on a statement already denominated
    /// in AUD — the label amount *is* the AUD figure, and a second one could
    /// silently disagree with it. Mapped to 422.
    AudOverrideOnAudStatement,
}

impl From<sqlx::Error> for UpsertError {
    fn from(e: sqlx::Error) -> Self {
        UpsertError::Db(e)
    }
}

pub async fn db_upsert(pool: &SqlitePool, s: &EssStatement) -> Result<(), UpsertError> {
    // A statement-AUD override restates a label the statement already gives in
    // AUD — reject the contradiction before touching the row.
    let has_override = s.aud_taxed_upfront_eligible.is_some()
        || s.aud_taxed_upfront_not_eligible.is_some()
        || s.aud_deferral_discount.is_some()
        || s.aud_pre_2009_cessation_discount.is_some()
        || s.aud_foreign_source_discount.is_some();
    if has_override && s.currency == "AUD" {
        return Err(UpsertError::AudOverrideOnAudStatement);
    }

    let mut tx = pool.begin().await?;

    // While its vest exists, the fields the Buy was created from (listing,
    // account, taxing point, quantity, market value, currency) are frozen —
    // editing them would desync the Buy. The income side (discount labels, TFN
    // withheld, statement-AUD overrides) stays editable: the employer's annual
    // ESS statement arrives after the vest is recorded. (A new id has no vest,
    // so an insert always passes.)
    let existing: Option<EssStatement> = {
        let vested: bool =
            sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM trades WHERE ess_statement_id = ?)")
                .bind(s.id)
                .fetch_one(&mut *tx)
                .await?;
        if vested {
            sqlx::query_as(&format!(
                "SELECT {COLUMNS} FROM ess_statements WHERE id = ?"
            ))
            .bind(s.id)
            .fetch_optional(&mut *tx)
            .await?
        } else {
            None
        }
    };
    if let Some(old) = existing {
        let vest_side_unchanged = old.listing_id == s.listing_id
            && old.holding_account_id == s.holding_account_id
            && old.taxing_point_date == s.taxing_point_date
            && old.quantity == s.quantity
            && old.market_value_per_share == s.market_value_per_share
            && old.currency == s.currency;
        if !vest_side_unchanged {
            return Err(UpsertError::Vested);
        }
    }

    sqlx::query(
        "INSERT INTO ess_statements \
         (id, listing_id, holding_account_id, taxing_point_date, quantity, \
          market_value_per_share, taxed_upfront_eligible, taxed_upfront_not_eligible, \
          deferral_discount, pre_2009_cessation_discount, foreign_source_discount, \
          tfn_withholding, currency, aud_taxed_upfront_eligible, \
          aud_taxed_upfront_not_eligible, aud_deferral_discount, \
          aud_pre_2009_cessation_discount, aud_foreign_source_discount) \
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?) \
         ON CONFLICT(id) DO UPDATE SET \
             listing_id                      = excluded.listing_id, \
             holding_account_id              = excluded.holding_account_id, \
             taxing_point_date               = excluded.taxing_point_date, \
             quantity                        = excluded.quantity, \
             market_value_per_share          = excluded.market_value_per_share, \
             taxed_upfront_eligible          = excluded.taxed_upfront_eligible, \
             taxed_upfront_not_eligible      = excluded.taxed_upfront_not_eligible, \
             deferral_discount               = excluded.deferral_discount, \
             pre_2009_cessation_discount     = excluded.pre_2009_cessation_discount, \
             foreign_source_discount         = excluded.foreign_source_discount, \
             tfn_withholding                 = excluded.tfn_withholding, \
             currency                        = excluded.currency, \
             aud_taxed_upfront_eligible      = excluded.aud_taxed_upfront_eligible, \
             aud_taxed_upfront_not_eligible  = excluded.aud_taxed_upfront_not_eligible, \
             aud_deferral_discount           = excluded.aud_deferral_discount, \
             aud_pre_2009_cessation_discount = excluded.aud_pre_2009_cessation_discount, \
             aud_foreign_source_discount     = excluded.aud_foreign_source_discount",
    )
    .bind(s.id)
    .bind(s.listing_id)
    .bind(s.holding_account_id)
    .bind(s.taxing_point_date)
    .bind(s.quantity.to_string())
    .bind(s.market_value_per_share.to_string())
    .bind(s.taxed_upfront_eligible.to_string())
    .bind(s.taxed_upfront_not_eligible.to_string())
    .bind(s.deferral_discount.to_string())
    .bind(s.pre_2009_cessation_discount.to_string())
    .bind(s.foreign_source_discount.to_string())
    .bind(s.tfn_withholding.to_string())
    .bind(&s.currency)
    .bind(s.aud_taxed_upfront_eligible.map(|d| d.to_string()))
    .bind(s.aud_taxed_upfront_not_eligible.map(|d| d.to_string()))
    .bind(s.aud_deferral_discount.map(|d| d.to_string()))
    .bind(s.aud_pre_2009_cessation_discount.map(|d| d.to_string()))
    .bind(s.aud_foreign_source_discount.map(|d| d.to_string()))
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(())
}

/// Outcome of a delete request, so the handler can map to the right status.
#[derive(Debug, PartialEq)]
pub enum DeleteOutcome {
    Deleted,
    NotFound,
    /// The statement has a vest Buy that is drawn on by a Sell allocation or an
    /// AMIT adjustment — removing the statement would have to remove that Buy,
    /// orphaning those dependants. Remove them first. Mapped to 422.
    VestDrawnOn,
}

/// Delete the statement and, if it was vested, its cost-base-reset Buy — in one
/// transaction. Refused while the vest Buy is drawn on.
pub async fn db_delete(pool: &SqlitePool, id: i64) -> Result<DeleteOutcome, sqlx::Error> {
    let mut tx = pool.begin().await?;

    let exists: bool =
        sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM ess_statements WHERE id = ?)")
            .bind(id)
            .fetch_one(&mut *tx)
            .await?;
    if !exists {
        return Ok(DeleteOutcome::NotFound);
    }

    // The linked vest Buy, if any. It is never deleted individually, so this is
    // the only path that removes it.
    let vest_id: Option<i64> =
        sqlx::query_scalar("SELECT id FROM trades WHERE ess_statement_id = ?")
            .bind(id)
            .fetch_optional(&mut *tx)
            .await?;
    if let Some(vest_id) = vest_id {
        let drawn_on: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM parcel_allocations \
                           WHERE purchase_trade_id = ?1 OR sale_trade_id = ?1) \
                 OR EXISTS(SELECT 1 FROM amit_adjustments WHERE trade_id = ?1)",
        )
        .bind(vest_id)
        .fetch_one(&mut *tx)
        .await?;
        if drawn_on {
            return Ok(DeleteOutcome::VestDrawnOn);
        }
        sqlx::query("DELETE FROM trades WHERE id = ?")
            .bind(vest_id)
            .execute(&mut *tx)
            .await?;
    }

    sqlx::query("DELETE FROM ess_statements WHERE id = ?")
        .bind(id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(DeleteOutcome::Deleted)
}

async fn list(State(pool): State<SqlitePool>) -> Result<Json<Vec<EssStatement>>, ApiError> {
    db_list(&pool).await.map(Json).map_err(ApiError::from)
}

async fn get_one(
    State(pool): State<SqlitePool>,
    Path(id): Path<i64>,
) -> Result<Json<EssStatement>, ApiError> {
    db_get(&pool, id)
        .await
        .map_err(ApiError::from)?
        .map(Json)
        .ok_or(ApiError::NotFound)
}

async fn upsert(
    State(pool): State<SqlitePool>,
    Path(id): Path<i64>,
    Json(body): Json<EssStatementBody>,
) -> Result<StatusCode, ApiError> {
    let s = EssStatement {
        id,
        listing_id: body.listing_id,
        holding_account_id: body.holding_account_id,
        taxing_point_date: body.taxing_point_date,
        quantity: body.quantity,
        market_value_per_share: body.market_value_per_share,
        taxed_upfront_eligible: body.taxed_upfront_eligible,
        taxed_upfront_not_eligible: body.taxed_upfront_not_eligible,
        deferral_discount: body.deferral_discount,
        pre_2009_cessation_discount: body.pre_2009_cessation_discount,
        foreign_source_discount: body.foreign_source_discount,
        tfn_withholding: body.tfn_withholding,
        currency: body.currency,
        aud_taxed_upfront_eligible: body.aud_taxed_upfront_eligible,
        aud_taxed_upfront_not_eligible: body.aud_taxed_upfront_not_eligible,
        aud_deferral_discount: body.aud_deferral_discount,
        aud_pre_2009_cessation_discount: body.aud_pre_2009_cessation_discount,
        aud_foreign_source_discount: body.aud_foreign_source_discount,
    };
    db_upsert(&pool, &s).await?;
    Ok(StatusCode::NO_CONTENT)
}

impl From<UpsertError> for ApiError {
    fn from(e: UpsertError) -> Self {
        match e {
            UpsertError::Vested => ApiError::unprocessable(
                "this ESS statement has been vested: the fields its vest Buy was created \
                 from (listing, account, taxing point, quantity, market value, currency) \
                 cannot change — delete the statement (which removes the vest Buy) and \
                 re-enter instead; the discount labels and statement-AUD overrides stay \
                 editable",
            ),
            UpsertError::AudOverrideOnAudStatement => ApiError::unprocessable(
                "statement-AUD override amounts are only accepted on a non-AUD statement \
                 — an AUD statement's discount labels are already the AUD figures",
            ),
            UpsertError::Db(err) => err.into(),
        }
    }
}

async fn delete(
    State(pool): State<SqlitePool>,
    Path(id): Path<i64>,
) -> Result<StatusCode, ApiError> {
    match db_delete(&pool, id).await? {
        DeleteOutcome::Deleted => Ok(StatusCode::NO_CONTENT),
        DeleteOutcome::NotFound => Err(ApiError::not_found("no ESS statement with that id")),
        DeleteOutcome::VestDrawnOn => Err(ApiError::unprocessable(
            "this ESS statement's vest Buy is drawn on by a sale allocation or AMIT \
             adjustment — remove those first",
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::entities::listing;
    use crate::test_support::{self, test_pool, ymd};
    use axum::{body::Body, http::Request};
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    async fn insert_listing(pool: &SqlitePool, id: i64) {
        test_support::listing(id)
            .ticker(&format!("ESS{id}"))
            .name(&format!("ESS {id}"))
            .security_type(listing::SecurityType::Share)
            .insert(pool)
            .await;
    }

    fn sample(id: i64) -> EssStatement {
        test_support::ess_statement(id, 1, ymd(2024, 9, 1))
            .with(|s| {
                s.quantity = Decimal::from(100);
                s.market_value_per_share = Decimal::from(6);
                s.deferral_discount = Decimal::from(600);
            })
            .build()
    }

    #[tokio::test]
    async fn db_round_trips_with_decimal_precision() {
        let pool = test_pool().await;
        insert_listing(&pool, 1).await;
        let mut s = sample(1);
        s.market_value_per_share = "6.123456789".parse().unwrap();
        s.deferral_discount = "612.345678900".parse().unwrap();
        db_upsert(&pool, &s).await.unwrap();
        let got = db_get(&pool, 1).await.unwrap().unwrap();
        assert_eq!(
            got.market_value_per_share,
            "6.123456789".parse::<Decimal>().unwrap()
        );
        assert_eq!(
            got.deferral_discount,
            "612.345678900".parse::<Decimal>().unwrap()
        );
        assert_eq!(
            got.taxing_point_date,
            NaiveDate::from_ymd_opt(2024, 9, 1).unwrap()
        );
    }

    /// Statement-AUD overrides round-trip with precision and absent ones stay
    /// NULL/None.
    #[tokio::test]
    async fn db_round_trips_statement_aud_overrides() {
        let pool = test_pool().await;
        insert_listing(&pool, 1).await;
        let mut s = sample(1);
        s.currency = "USD".to_string();
        s.aud_deferral_discount = Some("10572.45".parse().unwrap());
        db_upsert(&pool, &s).await.unwrap();
        let got = db_get(&pool, 1).await.unwrap().unwrap();
        assert_eq!(
            got.aud_deferral_discount,
            Some("10572.45".parse::<Decimal>().unwrap())
        );
        assert_eq!(got.aud_taxed_upfront_eligible, None);
        assert_eq!(got.aud_foreign_source_discount, None);
    }

    /// An override on a statement already denominated in AUD is rejected — two
    /// AUD figures for the same label could silently disagree.
    #[tokio::test]
    async fn db_aud_override_on_aud_statement_rejected() {
        let pool = test_pool().await;
        insert_listing(&pool, 1).await;
        let mut s = sample(1); // currency AUD
        s.aud_deferral_discount = Some(Decimal::from(600));
        assert!(matches!(
            db_upsert(&pool, &s).await,
            Err(UpsertError::AudOverrideOnAudStatement)
        ));
        assert!(db_get(&pool, 1).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn db_get_missing_returns_none() {
        let pool = test_pool().await;
        assert!(db_get(&pool, 99).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn db_delete_without_vest_just_removes_the_statement() {
        let pool = test_pool().await;
        insert_listing(&pool, 1).await;
        db_upsert(&pool, &sample(1)).await.unwrap();
        assert_eq!(db_delete(&pool, 1).await.unwrap(), DeleteOutcome::Deleted);
        assert!(db_get(&pool, 1).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn db_delete_missing_is_not_found() {
        let pool = test_pool().await;
        assert_eq!(db_delete(&pool, 99).await.unwrap(), DeleteOutcome::NotFound);
    }

    #[tokio::test]
    async fn api_upsert_unknown_currency_rejected_422() {
        let pool = test_pool().await;
        insert_listing(&pool, 1).await;
        let body = serde_json::json!({
            "listing_id": 1,
            "taxing_point_date": "2024-09-01",
            "currency": "ZZZ"
        });
        let resp = router()
            .with_state(pool)
            .oneshot(
                Request::builder()
                    .method("PUT")
                    .uri("/ess_statements/1")
                    .header("content-type", "application/json")
                    .body(Body::from(body.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY);
    }

    #[tokio::test]
    async fn api_aud_override_on_aud_statement_rejected_422() {
        let pool = test_pool().await;
        insert_listing(&pool, 1).await;
        let body = serde_json::json!({
            "listing_id": 1,
            "taxing_point_date": "2024-09-01",
            "currency": "AUD",
            "aud_deferral_discount": "600"
        });
        let resp = router()
            .with_state(pool)
            .oneshot(
                Request::builder()
                    .method("PUT")
                    .uri("/ess_statements/1")
                    .header("content-type", "application/json")
                    .body(Body::from(body.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY);
    }

    #[tokio::test]
    async fn api_unknown_listing_rejected_422() {
        let pool = test_pool().await;
        let body = serde_json::json!({ "listing_id": 999, "taxing_point_date": "2024-09-01" });
        let resp = router()
            .with_state(pool)
            .oneshot(
                Request::builder()
                    .method("PUT")
                    .uri("/ess_statements/1")
                    .header("content-type", "application/json")
                    .body(Body::from(body.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY);
    }

    #[tokio::test]
    async fn api_list_and_get() {
        let pool = test_pool().await;
        insert_listing(&pool, 1).await;
        db_upsert(&pool, &sample(1)).await.unwrap();
        let resp = router()
            .with_state(pool.clone())
            .oneshot(
                Request::builder()
                    .uri("/ess_statements")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let items: Vec<EssStatement> = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].deferral_discount, Decimal::from(600));
    }

    #[tokio::test]
    async fn api_delete_missing_returns_404() {
        let pool = test_pool().await;
        let resp = router()
            .with_state(pool)
            .oneshot(
                Request::builder()
                    .method("DELETE")
                    .uri("/ess_statements/99")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }
}
