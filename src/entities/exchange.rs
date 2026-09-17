use crate::infra::http::{self, ApiError, CrudEntity};
use axum::{
    Json, Router,
    extract::{Path, State},
    http::StatusCode,
    routing::get,
};
use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct Exchange {
    pub mic: String,
    pub name: String,
    pub country: String,
    pub currency: String,
    pub timezone: String,
    pub settlement_days: i64,
    /// Local-time end of the regular trading session (`HH:MM`, in `timezone`).
    /// The price-import job only collects a day's closing price once this time
    /// has passed in the exchange's timezone.
    pub close_time: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExchangeBody {
    pub name: String,
    pub country: String,
    pub currency: String,
    pub timezone: String,
    pub settlement_days: i64,
    #[serde(default = "default_close_time")]
    pub close_time: String,
}

fn default_close_time() -> String {
    "16:00".to_string()
}

/// The most business days `settlement_days` may be.
///
/// `add_business_days` computes T+n over the exchange's seeded holiday
/// calendar, so the value has to stay inside the horizon that calendar covers.
/// The seed (`0001_schema.sql`) publishes whole calendar years — 2019–2027 for
/// both XASX and XNYS, nine years — so a window of one year's worth of
/// business days is the longest still shorter than the horizon itself and can
/// therefore be covered by it; anything above is a typo, not a settlement
/// period. Real markets settle in a handful of days (ASX and NYSE are both
/// T+2), so this is deliberately generous. It is also what keeps the checked
/// date step in `trade::settlement::add_business_days` far away from
/// `NaiveDate`'s end for any value written through the API.
pub const MAX_SETTLEMENT_DAYS: i64 = 365;

/// Why an exchange write was refused.
#[derive(thiserror::Error, Debug)]
pub enum UpsertError {
    #[error("exchange write failed: {0}")]
    Db(#[from] sqlx::Error),
    /// `settlement_days` below zero. A negative T+n settles every trade on (or
    /// before) its own date and is recorded `computed`, so the
    /// `settlement-recompute` job re-affirms it for ever. Mapped to `422`.
    #[error("settlement_days cannot be negative")]
    NegativeSettlementDays,
    /// `settlement_days` above [`MAX_SETTLEMENT_DAYS`] (carries the rejected
    /// value). Mapped to `422`.
    #[error("settlement_days {value} is above the maximum of {max}")]
    SettlementDaysAboveMax { value: i64, max: i64 },
}

impl From<UpsertError> for ApiError {
    fn from(e: UpsertError) -> Self {
        match e {
            UpsertError::NegativeSettlementDays => ApiError::unprocessable(
                "settlement_days cannot be negative — T+n counts business days forward from the \
                 trade date, so a negative value settles every trade on its own date",
            ),
            UpsertError::SettlementDaysAboveMax { value, max } => ApiError::unprocessable(format!(
                "settlement_days {value} is above the maximum of {max} — T+n is a market's \
                 settlement period (ASX and NYSE are T+2), and a window longer than one year of \
                 business days cannot be covered by the seeded holiday calendar"
            )),
            // An unknown currency (FK) surfaces as 422 with the offending
            // constraint named.
            UpsertError::Db(err) => err.into(),
        }
    }
}

impl CrudEntity for Exchange {
    /// Keyed by MIC, not a rowid.
    type Key = String;
    const TABLE: &'static str = "exchanges";
    const COLUMNS: &'static str =
        "mic, name, country, currency, timezone, settlement_days, close_time";
    const KEY_COLUMN: &'static str = "mic";
    const ORDER_BY: &'static str = "mic";
    const NOUN: &'static str = "exchange";
}

pub fn router() -> Router<SqlitePool> {
    Router::new()
        .route("/exchanges", get(http::list_handler::<Exchange>))
        .route(
            "/exchanges/{mic}",
            get(http::get_handler::<Exchange>)
                .put(upsert)
                // Deleting an exchange still referenced by listings/holidays
                // violates an FK → 422.
                .delete(http::delete_handler::<Exchange>),
        )
}

/// Executor-generic for the same reason [`listing::db_get`] is: the trading
/// calendar has to be readable on a write path's own transaction.
///
/// [`listing::db_get`]: crate::entities::listing::db_get
pub async fn db_get<'e, X>(executor: X, mic: &str) -> Result<Option<Exchange>, sqlx::Error>
where
    X: sqlx::Executor<'e, Database = sqlx::Sqlite>,
{
    http::crud_get(executor, mic.to_string()).await
}

pub async fn db_upsert(pool: &SqlitePool, exchange: &Exchange) -> Result<(), UpsertError> {
    // `settlement_days` drives T+n on every trade written against this
    // exchange, so it is validated here, at the one write path, rather than
    // trusted. Both bounds are what the unchecked date step in
    // `trade::settlement::add_business_days` used to panic past, and what a
    // negative value silently mis-computed.
    if exchange.settlement_days < 0 {
        return Err(UpsertError::NegativeSettlementDays);
    }
    if exchange.settlement_days > MAX_SETTLEMENT_DAYS {
        return Err(UpsertError::SettlementDaysAboveMax {
            value: exchange.settlement_days,
            max: MAX_SETTLEMENT_DAYS,
        });
    }
    sqlx::query(
        "INSERT INTO exchanges (mic, name, country, currency, timezone, settlement_days, close_time) \
         VALUES (?, ?, ?, ?, ?, ?, ?) \
         ON CONFLICT(mic) DO UPDATE SET \
             name = excluded.name, \
             country = excluded.country, \
             currency = excluded.currency, \
             timezone = excluded.timezone, \
             settlement_days = excluded.settlement_days, \
             close_time = excluded.close_time",
    )
    .bind(&exchange.mic)
    .bind(&exchange.name)
    .bind(&exchange.country)
    .bind(&exchange.currency)
    .bind(&exchange.timezone)
    .bind(exchange.settlement_days)
    .bind(&exchange.close_time)
    .execute(pool)
    .await?;
    Ok(())
}

#[cfg(test)]
pub async fn db_delete(pool: &SqlitePool, mic: &str) -> Result<bool, sqlx::Error> {
    http::crud_delete::<Exchange>(pool, mic.to_string()).await
}

async fn upsert(
    State(pool): State<SqlitePool>,
    Path(mic): Path<String>,
    Json(body): Json<ExchangeBody>,
) -> Result<StatusCode, ApiError> {
    let exchange = Exchange {
        mic,
        name: body.name,
        country: body.country,
        currency: body.currency,
        timezone: body.timezone,
        settlement_days: body.settlement_days,
        close_time: body.close_time,
    };
    db_upsert(&pool, &exchange)
        .await
        .map(|_| StatusCode::NO_CONTENT)
        .map_err(ApiError::from)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{ApiClient, test_pool};

    fn client(pool: &SqlitePool) -> ApiClient {
        ApiClient::over(router().with_state(pool.clone()))
    }

    fn xtest() -> Exchange {
        Exchange {
            mic: "XTES".to_string(),
            name: "Test Exchange".to_string(),
            country: "Testland".to_string(),
            currency: "AUD".to_string(),
            timezone: "UTC".to_string(),
            settlement_days: 2,
            close_time: "16:00".to_string(),
        }
    }

    // DB-level tests

    #[tokio::test]
    async fn db_insert_and_retrieve() {
        let pool = test_pool().await;
        db_upsert(&pool, &xtest()).await.unwrap();
        let got = db_get(&pool, "XTES").await.unwrap().unwrap();
        assert_eq!(got.name, "Test Exchange");
        assert_eq!(got.settlement_days, 2);
    }

    #[tokio::test]
    async fn db_get_missing_returns_none() {
        let pool = test_pool().await;
        assert!(db_get(&pool, "XXXX").await.unwrap().is_none());
    }

    #[tokio::test]
    async fn db_upsert_updates_existing() {
        let pool = test_pool().await;
        db_upsert(&pool, &xtest()).await.unwrap();
        let mut updated = xtest();
        updated.name = "Updated Exchange".to_string();
        db_upsert(&pool, &updated).await.unwrap();
        let got = db_get(&pool, "XTES").await.unwrap().unwrap();
        assert_eq!(got.name, "Updated Exchange");
    }

    #[tokio::test]
    async fn db_delete_removes_exchange() {
        let pool = test_pool().await;
        db_upsert(&pool, &xtest()).await.unwrap();
        assert!(db_delete(&pool, "XTES").await.unwrap());
        assert!(db_get(&pool, "XTES").await.unwrap().is_none());
    }

    #[tokio::test]
    async fn db_delete_missing_returns_false() {
        let pool = test_pool().await;
        assert!(!db_delete(&pool, "XXXX").await.unwrap());
    }

    #[tokio::test]
    async fn seed_data_has_xasx_and_xnys() {
        let pool = test_pool().await;
        let asx = db_get(&pool, "XASX").await.unwrap().unwrap();
        assert_eq!(asx.currency, "AUD");
        assert_eq!(asx.settlement_days, 2);
        assert!(db_get(&pool, "XNYS").await.unwrap().is_some());
    }

    // API-level tests

    #[tokio::test]
    async fn api_list_includes_seed_exchanges() {
        let pool = test_pool().await;
        let exchanges: Vec<Exchange> = client(&pool).get_json("/exchanges").await;
        assert!(exchanges.iter().any(|e| e.mic == "XASX"));
        assert!(exchanges.iter().any(|e| e.mic == "XNYS"));
    }

    #[tokio::test]
    async fn api_get_existing_returns_exchange() {
        let pool = test_pool().await;
        let ex: Exchange = client(&pool).get_json("/exchanges/XASX").await;
        assert_eq!(ex.mic, "XASX");
    }

    #[tokio::test]
    async fn api_get_missing_returns_404() {
        let pool = test_pool().await;
        let resp = client(&pool).get("/exchanges/XXXX").await;
        assert_eq!(resp.status, StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn api_upsert_creates_exchange() {
        let pool = test_pool().await;
        let body = serde_json::json!({
            "name": "Test Exchange",
            "country": "Testland",
            "currency": "AUD",
            "timezone": "UTC",
            "settlement_days": 2
        });
        client(&pool).put_ok("/exchanges/XTES", &body).await;
        assert!(db_get(&pool, "XTES").await.unwrap().is_some());
    }

    #[tokio::test]
    async fn api_upsert_updates_exchange() {
        let pool = test_pool().await;
        db_upsert(&pool, &xtest()).await.unwrap();
        let body = serde_json::json!({
            "name": "Renamed Exchange",
            "country": "Testland",
            "currency": "AUD",
            "timezone": "UTC",
            "settlement_days": 3
        });
        client(&pool).put_ok("/exchanges/XTES", &body).await;
        let got = db_get(&pool, "XTES").await.unwrap().unwrap();
        assert_eq!(got.name, "Renamed Exchange");
        assert_eq!(got.settlement_days, 3);
    }

    /// The 2026-09-17 review reproduced this end to end: `settlement_days`
    /// was bound with no check at all, so `-1` silently made every auto
    /// settlement the trade date and `100000000` panicked `add_business_days`
    /// on the *next* trade write (a burned worker and a slow empty `500`).
    /// Both ends are refused `422` naming the field, and neither is stored.
    #[tokio::test]
    async fn api_settlement_days_out_of_range_returns_422() {
        let pool = test_pool().await;
        for days in [-1_i64, -2, 99999, 100_000_000] {
            let body = serde_json::json!({
                "name": "Test Exchange",
                "country": "Testland",
                "currency": "AUD",
                "timezone": "UTC",
                "settlement_days": days
            });
            let resp = client(&pool).put("/exchanges/XTES", &body).await;
            let (status, detail) = resp.status_and_body();
            assert_eq!(
                status,
                StatusCode::UNPROCESSABLE_ENTITY,
                "settlement_days {days} must be refused 422"
            );
            assert!(
                detail.contains("settlement_days"),
                "the 422 must name the field, got: {detail}"
            );
            assert!(
                db_get(&pool, "XTES").await.unwrap().is_none(),
                "settlement_days {days} must not be stored"
            );
        }
        // A real T+n is still accepted, with the documented `204`.
        let body = serde_json::json!({
            "name": "Test Exchange",
            "country": "Testland",
            "currency": "AUD",
            "timezone": "UTC",
            "settlement_days": 2
        });
        client(&pool).put_ok("/exchanges/XTES", &body).await;
        assert_eq!(
            db_get(&pool, "XTES")
                .await
                .unwrap()
                .unwrap()
                .settlement_days,
            2
        );
    }

    /// The bound is inclusive: [`MAX_SETTLEMENT_DAYS`] itself is a legal T+n
    /// (generous, but inside the seeded calendar's horizon), so only a value
    /// above it is refused.
    #[tokio::test]
    async fn api_settlement_days_at_the_maximum_is_accepted() {
        let pool = test_pool().await;
        let body = serde_json::json!({
            "name": "Test Exchange",
            "country": "Testland",
            "currency": "AUD",
            "timezone": "UTC",
            "settlement_days": MAX_SETTLEMENT_DAYS
        });
        client(&pool).put_ok("/exchanges/XTES", &body).await;
        assert_eq!(
            db_get(&pool, "XTES")
                .await
                .unwrap()
                .unwrap()
                .settlement_days,
            MAX_SETTLEMENT_DAYS
        );
    }

    /// The validation lives in [`db_upsert`] — the one write path — not only in
    /// the handler, so a direct call is refused too, and a rejected write
    /// persists nothing.
    #[tokio::test]
    async fn db_upsert_rejects_out_of_range_settlement_days() {
        let pool = test_pool().await;
        let mut ex = xtest();
        ex.settlement_days = -1;
        assert!(matches!(
            db_upsert(&pool, &ex).await.unwrap_err(),
            UpsertError::NegativeSettlementDays
        ));
        ex.settlement_days = MAX_SETTLEMENT_DAYS + 1;
        assert!(matches!(
            db_upsert(&pool, &ex).await.unwrap_err(),
            UpsertError::SettlementDaysAboveMax { .. }
        ));
        assert!(db_get(&pool, "XTES").await.unwrap().is_none());
    }

    #[tokio::test]
    async fn api_delete_existing_returns_no_content() {
        let pool = test_pool().await;
        // Delete a fresh exchange with no dependents — the seeded XASX/XNYS now
        // have child rows in exchange_holidays, so their delete is FK-blocked.
        db_upsert(&pool, &xtest()).await.unwrap();
        let resp = client(&pool).delete("/exchanges/XTES").await;
        assert_eq!(resp.status, StatusCode::NO_CONTENT);
    }

    #[tokio::test]
    async fn api_delete_missing_returns_404() {
        let pool = test_pool().await;
        let resp = client(&pool).delete("/exchanges/XXXX").await;
        assert_eq!(resp.status, StatusCode::NOT_FOUND);
    }
}
