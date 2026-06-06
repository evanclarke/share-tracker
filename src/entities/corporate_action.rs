//! Corporate actions recorded against a listing.
//!
//! One action type is modelled so far: **ReturnOfCapital** — a non-assessable
//! payment from a company (a shareholder-approved return of share capital, CGT
//! event G1; see `docs/cgt-non-assessable-payments.md`). The per-unit payment
//! reduces the cost base of every parcel of the listing held on the payment
//! date (units sold before the payment were not held for it, so they are
//! unaffected). Where cumulative payments exceed a parcel's per-unit cost base,
//! the cost base floors at nil and the excess is an immediate capital gain in
//! the payment's income year — G1 can never produce a capital loss — computed
//! by the net-capital-gain report (`g1_gains`). Distinct from the AMIT
//! tax-deferred regime (CGT event E10, `amit_adjustment`), which applies to
//! trust units, not company shares.
//!
//! `ActionType` is the extension point for future corporate actions (splits,
//! bonus shares, rights issues, ...), each widening the enum and its CHECK.

use crate::infra::decimal::parse_dec;
use crate::infra::http::write_error_status;
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
use std::collections::HashMap;

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, sqlx::Type)]
pub enum ActionType {
    ReturnOfCapital,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CorporateAction {
    pub id: i64,
    pub action_type: ActionType,
    pub listing_id: i64,
    /// Payment date: parcels acquired on/before this date are affected.
    pub date: NaiveDate,
    /// Per-unit payment amount in `currency` (must be positive).
    pub amount_per_unit: Decimal,
    pub currency: String,
}

impl<'r> sqlx::FromRow<'r, sqlx::sqlite::SqliteRow> for CorporateAction {
    fn from_row(row: &'r sqlx::sqlite::SqliteRow) -> Result<Self, sqlx::Error> {
        Ok(CorporateAction {
            id: row.try_get("id")?,
            action_type: row.try_get("action_type")?,
            listing_id: row.try_get("listing_id")?,
            date: row.try_get("date")?,
            amount_per_unit: parse_dec("amount_per_unit", row.try_get("amount_per_unit")?)?,
            currency: row.try_get("currency")?,
        })
    }
}

#[derive(Debug, Deserialize)]
pub struct CorporateActionBody {
    pub action_type: ActionType,
    pub listing_id: i64,
    pub date: NaiveDate,
    pub amount_per_unit: Decimal,
    pub currency: String,
}

pub fn router() -> Router<SqlitePool> {
    Router::new()
        .route("/corporate_actions", get(list))
        .route("/corporate_actions/{id}", get(get_one).put(upsert).delete(delete))
}

const COLUMNS: &str = "id, action_type, listing_id, date, amount_per_unit, currency";

pub async fn db_list(pool: &SqlitePool) -> Result<Vec<CorporateAction>, sqlx::Error> {
    sqlx::query_as(&format!("SELECT {COLUMNS} FROM corporate_actions ORDER BY id"))
        .fetch_all(pool)
        .await
}

pub async fn db_get(pool: &SqlitePool, id: i64) -> Result<Option<CorporateAction>, sqlx::Error> {
    sqlx::query_as(&format!("SELECT {COLUMNS} FROM corporate_actions WHERE id = ?"))
        .bind(id)
        .fetch_optional(pool)
        .await
}

pub async fn db_upsert(pool: &SqlitePool, action: &CorporateAction) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO corporate_actions (id, action_type, listing_id, date, amount_per_unit, currency) \
         VALUES (?, ?, ?, ?, ?, ?) \
         ON CONFLICT(id) DO UPDATE SET \
             action_type     = excluded.action_type, \
             listing_id      = excluded.listing_id, \
             date            = excluded.date, \
             amount_per_unit = excluded.amount_per_unit, \
             currency        = excluded.currency",
    )
    .bind(action.id)
    .bind(action.action_type)
    .bind(action.listing_id)
    .bind(action.date)
    .bind(action.amount_per_unit.to_string())
    .bind(&action.currency)
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn db_delete(pool: &SqlitePool, id: i64) -> Result<bool, sqlx::Error> {
    let result = sqlx::query("DELETE FROM corporate_actions WHERE id = ?")
        .bind(id)
        .execute(pool)
        .await?;
    Ok(result.rows_affected() > 0)
}

/// A return-of-capital payment, as consumed by the cost-base reports.
#[derive(Debug, Clone)]
pub struct RocEvent {
    pub date: NaiveDate,
    pub amount_per_unit: Decimal,
    pub currency: String,
}

/// All ReturnOfCapital actions keyed by listing, each list sorted by payment
/// date (then id). Shared by the portfolio/unrealised/realised/open-parcels
/// reports to reduce affected parcels' cost bases, and by the net-capital-gain
/// report's G1 walk.
pub async fn db_return_of_capital_events(
    pool: &SqlitePool,
) -> Result<HashMap<i64, Vec<RocEvent>>, sqlx::Error> {
    let rows = sqlx::query(
        "SELECT listing_id, date, amount_per_unit, currency FROM corporate_actions \
         WHERE action_type = 'ReturnOfCapital' ORDER BY listing_id, date, id",
    )
    .fetch_all(pool)
    .await?;

    let mut map: HashMap<i64, Vec<RocEvent>> = HashMap::new();
    for row in &rows {
        let listing_id: i64 = row.try_get("listing_id")?;
        map.entry(listing_id).or_default().push(RocEvent {
            date: row.try_get("date")?,
            amount_per_unit: parse_dec("amount_per_unit", row.try_get("amount_per_unit")?)?,
            currency: row.try_get("currency")?,
        });
    }
    Ok(map)
}

/// Cumulative per-unit return-of-capital cost-base reduction borne by a unit
/// acquired on `acquired` and still held at `up_to` (or held today when `None`):
/// the sum of `amount_per_unit` over the listing's payments dated within
/// `[acquired, up_to]`. A unit sold before a payment was not held for it, so the
/// realised report bounds `up_to` at the sale date; the open-holdings reports
/// pass `None` (an unsold unit was held for every payment since acquisition).
///
/// Fails loudly when a payment's currency differs from the parcel's — amounts in
/// different currencies must never be netted against each other.
pub fn per_unit_reduction(
    events: &[RocEvent],
    trade_currency: &str,
    acquired: NaiveDate,
    up_to: Option<NaiveDate>,
) -> Result<Decimal, sqlx::Error> {
    let mut total = Decimal::ZERO;
    for e in events {
        if e.date < acquired || up_to.is_some_and(|d| e.date > d) {
            continue;
        }
        if e.currency != trade_currency {
            return Err(sqlx::Error::Decode(
                format!(
                    "return-of-capital currency {} differs from the parcel's currency {}",
                    e.currency, trade_currency
                )
                .into(),
            ));
        }
        total += e.amount_per_unit;
    }
    Ok(total)
}

async fn list(State(pool): State<SqlitePool>) -> Result<Json<Vec<CorporateAction>>, StatusCode> {
    db_list(&pool)
        .await
        .map(Json)
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)
}

async fn get_one(
    State(pool): State<SqlitePool>,
    Path(id): Path<i64>,
) -> Result<Json<CorporateAction>, StatusCode> {
    db_get(&pool, id)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
        .map(Json)
        .ok_or(StatusCode::NOT_FOUND)
}

async fn upsert(
    State(pool): State<SqlitePool>,
    Path(id): Path<i64>,
    Json(body): Json<CorporateActionBody>,
) -> Result<StatusCode, StatusCode> {
    // A return of capital is a payment received: zero/negative is meaningless
    // (and a negative amount would silently *increase* cost bases).
    if body.amount_per_unit <= Decimal::ZERO {
        return Err(StatusCode::UNPROCESSABLE_ENTITY);
    }
    let action = CorporateAction {
        id,
        action_type: body.action_type,
        listing_id: body.listing_id,
        date: body.date,
        amount_per_unit: body.amount_per_unit,
        currency: body.currency,
    };
    db_upsert(&pool, &action)
        .await
        .map(|_| StatusCode::NO_CONTENT)
        // Unknown listing/currency FK or enum CHECK violation → 422.
        .map_err(|e| write_error_status(&e))
}

async fn delete(
    State(pool): State<SqlitePool>,
    Path(id): Path<i64>,
) -> Result<StatusCode, StatusCode> {
    db_delete(&pool, id)
        .await
        .map(|found| if found { StatusCode::NO_CONTENT } else { StatusCode::NOT_FOUND })
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{entities::listing, infra::db};
    use axum::{body::Body, http::Request};
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    async fn test_pool() -> SqlitePool {
        db::init(":memory:").await.unwrap()
    }

    async fn insert_listing(pool: &SqlitePool, id: i64, ticker: &str) {
        listing::db_upsert(
            pool,
            &listing::Listing {
                id,
                exchange_mic: "XASX".to_string(),
                ticker: ticker.to_string(),
                name: ticker.to_string(),
                isin: None,
                security_type: listing::SecurityType::Share,
                currency: "AUD".to_string(),
                amit: false,
                preference: false,
            },
        )
        .await
        .unwrap();
    }

    fn roc(id: i64, listing_id: i64, date: NaiveDate, amount: &str) -> CorporateAction {
        CorporateAction {
            id,
            action_type: ActionType::ReturnOfCapital,
            listing_id,
            date,
            amount_per_unit: amount.parse().unwrap(),
            currency: "AUD".to_string(),
        }
    }

    fn d(y: i32, m: u32, day: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, day).unwrap()
    }

    // DB-level tests

    #[tokio::test]
    async fn db_insert_and_retrieve_preserves_precision() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "RAP").await;
        db_upsert(&pool, &roc(1, 1, d(2024, 11, 30), "0.505")).await.unwrap();

        let got = db_get(&pool, 1).await.unwrap().unwrap();
        assert_eq!(got.action_type, ActionType::ReturnOfCapital);
        assert_eq!(got.listing_id, 1);
        assert_eq!(got.date, d(2024, 11, 30));
        assert_eq!(got.amount_per_unit, "0.505".parse::<Decimal>().unwrap());
        assert_eq!(got.currency, "AUD");
    }

    #[tokio::test]
    async fn db_upsert_updates_existing() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "RAP").await;
        db_upsert(&pool, &roc(1, 1, d(2024, 11, 30), "0.50")).await.unwrap();
        db_upsert(&pool, &roc(1, 1, d(2024, 12, 31), "0.75")).await.unwrap();

        let all = db_list(&pool).await.unwrap();
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].date, d(2024, 12, 31));
        assert_eq!(all[0].amount_per_unit, "0.75".parse::<Decimal>().unwrap());
    }

    #[tokio::test]
    async fn db_listing_fk_enforced() {
        let pool = test_pool().await;
        let err = db_upsert(&pool, &roc(1, 999, d(2024, 11, 30), "0.50")).await;
        assert!(err.is_err(), "unknown listing FK should be rejected");
    }

    #[tokio::test]
    async fn db_get_missing_returns_none() {
        let pool = test_pool().await;
        assert!(db_get(&pool, 999).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn db_events_grouped_by_listing_sorted_by_date() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "RAP").await;
        insert_listing(&pool, 2, "XYZ").await;
        db_upsert(&pool, &roc(1, 1, d(2025, 3, 1), "0.30")).await.unwrap();
        db_upsert(&pool, &roc(2, 1, d(2024, 11, 30), "0.50")).await.unwrap();
        db_upsert(&pool, &roc(3, 2, d(2024, 6, 1), "1.00")).await.unwrap();

        let events = db_return_of_capital_events(&pool).await.unwrap();
        assert_eq!(events.len(), 2);
        let l1: Vec<NaiveDate> = events[&1].iter().map(|e| e.date).collect();
        assert_eq!(l1, vec![d(2024, 11, 30), d(2025, 3, 1)]);
        assert_eq!(events[&2].len(), 1);
    }

    #[test]
    fn per_unit_reduction_sums_events_from_acquisition() {
        let events = vec![
            RocEvent { date: d(2024, 1, 1), amount_per_unit: "0.10".parse().unwrap(), currency: "AUD".into() },
            RocEvent { date: d(2024, 6, 1), amount_per_unit: "0.20".parse().unwrap(), currency: "AUD".into() },
            RocEvent { date: d(2025, 1, 1), amount_per_unit: "0.40".parse().unwrap(), currency: "AUD".into() },
        ];
        // Acquired between the first and second events: the first doesn't apply.
        let pu = per_unit_reduction(&events, "AUD", d(2024, 3, 1), None).unwrap();
        assert_eq!(pu, "0.60".parse::<Decimal>().unwrap());
        // Acquired on the event date: held on the payment date, so it applies.
        let pu = per_unit_reduction(&events, "AUD", d(2024, 6, 1), None).unwrap();
        assert_eq!(pu, "0.60".parse::<Decimal>().unwrap());
    }

    #[test]
    fn per_unit_reduction_bounds_at_sale_date() {
        let events = vec![
            RocEvent { date: d(2024, 6, 1), amount_per_unit: "0.20".parse().unwrap(), currency: "AUD".into() },
            RocEvent { date: d(2025, 1, 1), amount_per_unit: "0.40".parse().unwrap(), currency: "AUD".into() },
        ];
        // Sold between the events: only the payment received while held applies.
        let pu = per_unit_reduction(&events, "AUD", d(2024, 1, 1), Some(d(2024, 9, 1))).unwrap();
        assert_eq!(pu, "0.20".parse::<Decimal>().unwrap());
        // Sold on the payment date: still held at the payment, so it applies.
        let pu = per_unit_reduction(&events, "AUD", d(2024, 1, 1), Some(d(2025, 1, 1))).unwrap();
        assert_eq!(pu, "0.60".parse::<Decimal>().unwrap());
        // Sold before any payment: unaffected.
        let pu = per_unit_reduction(&events, "AUD", d(2024, 1, 1), Some(d(2024, 5, 1))).unwrap();
        assert_eq!(pu, Decimal::ZERO);
    }

    #[test]
    fn per_unit_reduction_rejects_currency_mismatch() {
        let events = vec![RocEvent {
            date: d(2024, 6, 1),
            amount_per_unit: "0.20".parse().unwrap(),
            currency: "USD".into(),
        }];
        // Never net amounts across currencies: fail loudly, don't skip or zero.
        assert!(per_unit_reduction(&events, "AUD", d(2024, 1, 1), None).is_err());
        // An out-of-range event in another currency is not an error — it doesn't
        // participate in the calculation at all.
        assert!(per_unit_reduction(&events, "AUD", d(2024, 7, 1), None).is_ok());
    }

    // API-level tests

    #[tokio::test]
    async fn api_put_get_list_delete_round_trip() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "RAP").await;
        let body = serde_json::json!({
            "action_type": "ReturnOfCapital",
            "listing_id": 1,
            "date": "2024-11-30",
            "amount_per_unit": "0.50",
            "currency": "AUD",
        });
        let resp = router()
            .with_state(pool.clone())
            .oneshot(
                Request::builder()
                    .method("PUT")
                    .uri("/corporate_actions/1")
                    .header("content-type", "application/json")
                    .body(Body::from(body.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NO_CONTENT);

        let resp = router()
            .with_state(pool.clone())
            .oneshot(Request::builder().uri("/corporate_actions/1").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let got: CorporateAction = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(got.amount_per_unit, "0.50".parse::<Decimal>().unwrap());

        let resp = router()
            .with_state(pool.clone())
            .oneshot(Request::builder().uri("/corporate_actions").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let items: Vec<CorporateAction> = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(items.len(), 1);

        let resp = router()
            .with_state(pool.clone())
            .oneshot(
                Request::builder()
                    .method("DELETE")
                    .uri("/corporate_actions/1")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NO_CONTENT);
        assert!(db_get(&pool, 1).await.unwrap().is_none());
    }

    async fn api_put_expecting(pool: &SqlitePool, body: serde_json::Value, expected: StatusCode) {
        let resp = router()
            .with_state(pool.clone())
            .oneshot(
                Request::builder()
                    .method("PUT")
                    .uri("/corporate_actions/1")
                    .header("content-type", "application/json")
                    .body(Body::from(body.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), expected);
    }

    #[tokio::test]
    async fn api_non_positive_amount_returns_422() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "RAP").await;
        for amount in ["0", "-0.50"] {
            api_put_expecting(
                &pool,
                serde_json::json!({
                    "action_type": "ReturnOfCapital",
                    "listing_id": 1,
                    "date": "2024-11-30",
                    "amount_per_unit": amount,
                    "currency": "AUD",
                }),
                StatusCode::UNPROCESSABLE_ENTITY,
            )
            .await;
        }
    }

    #[tokio::test]
    async fn api_unknown_listing_returns_422() {
        let pool = test_pool().await;
        api_put_expecting(
            &pool,
            serde_json::json!({
                "action_type": "ReturnOfCapital",
                "listing_id": 999,
                "date": "2024-11-30",
                "amount_per_unit": "0.50",
                "currency": "AUD",
            }),
            StatusCode::UNPROCESSABLE_ENTITY,
        )
        .await;
    }

    #[tokio::test]
    async fn api_unknown_currency_returns_422() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "RAP").await;
        api_put_expecting(
            &pool,
            serde_json::json!({
                "action_type": "ReturnOfCapital",
                "listing_id": 1,
                "date": "2024-11-30",
                "amount_per_unit": "0.50",
                "currency": "ZZZ",
            }),
            StatusCode::UNPROCESSABLE_ENTITY,
        )
        .await;
    }

    #[tokio::test]
    async fn api_unknown_action_type_rejected() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "RAP").await;
        // Serde rejects an unrecognised enum variant before it reaches the DB.
        api_put_expecting(
            &pool,
            serde_json::json!({
                "action_type": "ShareSplit",
                "listing_id": 1,
                "date": "2024-11-30",
                "amount_per_unit": "0.50",
                "currency": "AUD",
            }),
            StatusCode::UNPROCESSABLE_ENTITY,
        )
        .await;
    }

    #[tokio::test]
    async fn api_get_and_delete_missing_return_404() {
        let pool = test_pool().await;
        let resp = router()
            .with_state(pool.clone())
            .oneshot(Request::builder().uri("/corporate_actions/999").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);

        let resp = router()
            .with_state(pool)
            .oneshot(
                Request::builder()
                    .method("DELETE")
                    .uri("/corporate_actions/999")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }
}
