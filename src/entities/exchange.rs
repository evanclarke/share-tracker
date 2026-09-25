use crate::infra::db::write_tx;
use crate::infra::http::{self, ApiError, CrudEntity, Upsert, UpsertResponse};
use axum::{
    Json, Router,
    extract::{Path, State},
    routing::get,
};
use chrono::NaiveTime;
use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;

#[derive(utoipa::ToSchema, Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
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

#[derive(utoipa::ToSchema, Debug, Deserialize)]
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
    /// `close_time` is not an `HH:MM` local time in range (carries the rejected
    /// value). The price-import job only collects a day's close once this time
    /// has passed in the exchange's `timezone`, and
    /// `closing_price::market::Market::latest_complete_trading_day` parses it
    /// with `%H:%M` — so a malformed value is a defect discovered downstream,
    /// not at the write. Mapped to `422`.
    #[error("close_time {0:?} is not HH:MM")]
    MalformedCloseTime(String),
    /// `timezone` is not a recognised IANA zone name (carries the rejected
    /// value). It is the local clock `close_time` is measured against, so an
    /// unknown zone breaks every market-close decision for the exchange and
    /// surfaces only as a downstream parse failure. Mapped to `422`.
    #[error("timezone {0:?} is not a recognised IANA zone")]
    UnknownTimezone(String),
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
            UpsertError::MalformedCloseTime(value) => ApiError::unprocessable(format!(
                "close_time {value:?} is not an HH:MM local time — enter the end of the regular \
                 session as a two-digit 24-hour hour and minute (e.g. 16:00) in the exchange's \
                 own timezone, which is when closing-price collection considers the day's close \
                 final"
            )),
            UpsertError::UnknownTimezone(value) => ApiError::unprocessable(format!(
                "timezone {value:?} is not a recognised IANA zone name — enter the exchange's own \
                 zone (e.g. Australia/Sydney, America/New_York), which is the local clock \
                 close_time is measured against"
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

    /// Keyed by MIC, so the default body's "with that id" would name a column
    /// the route's URL never carries.
    fn missing_row_body(_mic: &String) -> String {
        "no exchange with that mic".to_string()
    }
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

pub async fn db_upsert(pool: &SqlitePool, exchange: &Exchange) -> Result<Upsert, UpsertError> {
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
    // `close_time` and `timezone` are the other two fields a downstream
    // calculation reads and cannot cope with a bad value of: `close_time` is
    // parsed as `%H:%M` by `Market::latest_complete_trading_day`, and the
    // timezone is parsed by `Market::tz`. Both are validated here, at the one
    // write path, rather than discovered later (2026-09-17 review).
    if !is_hh_mm(&exchange.close_time) {
        return Err(UpsertError::MalformedCloseTime(exchange.close_time.clone()));
    }
    if exchange.timezone.parse::<chrono_tz::Tz>().is_err() {
        return Err(UpsertError::UnknownTimezone(exchange.timezone.clone()));
    }
    // The exists check decides create-vs-replace inside the write's own
    // `BEGIN IMMEDIATE` transaction (CLAUDE.md, "Data integrity").
    let mut tx = write_tx(pool).await?;
    let existed = http::crud_exists::<Exchange, _>(&mut *tx, exchange.mic.clone()).await?;
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
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(if existed {
        Upsert::Replaced
    } else {
        Upsert::Created
    })
}

/// Is `s` an `HH:MM` local time — exactly two digits, a colon, two digits, in
/// range? `NaiveTime`'s own parse accepts a one- or two-digit hour (`9:30`,
/// `16:00`), while the documented spelling and the stored form are strictly
/// `HH:MM`, so the canonical round trip is what pins it: anything that does not
/// format back to the input is refused.
fn is_hh_mm(s: &str) -> bool {
    NaiveTime::parse_from_str(s, "%H:%M")
        .map(|t| t.format("%H:%M").to_string() == s)
        .unwrap_or(false)
}

#[cfg(test)]
pub async fn db_delete(pool: &SqlitePool, mic: &str) -> Result<bool, sqlx::Error> {
    http::crud_delete::<Exchange>(pool, mic.to_string()).await
}

async fn upsert(
    State(pool): State<SqlitePool>,
    Path(mic): Path<String>,
    Json(body): Json<ExchangeBody>,
) -> Result<UpsertResponse<Exchange>, ApiError> {
    let exchange = Exchange {
        mic,
        name: body.name,
        country: body.country,
        currency: body.currency,
        timezone: body.timezone,
        settlement_days: body.settlement_days,
        close_time: body.close_time,
    };
    let key = exchange.mic.clone();
    let outcome = db_upsert(&pool, &exchange).await?;
    http::upsert_response::<Exchange>(&pool, outcome, key).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{ApiClient, test_pool};
    use axum::http::StatusCode;

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

    /// The 2026-09-17 review reproduced this end to end: `PUT /exchanges/XASX`
    /// with `close_time: "nonsense"` or `"99:99"` returned `204` — the docs
    /// call the field `HH:MM` local — and so did `timezone: "Mars/Olympus"`.
    /// Both drive market-close logic and a bad value surfaced only downstream,
    /// so each is refused `422` naming the field, with nothing stored, while a
    /// real pair still answers the documented `204` and round-trips.
    #[tokio::test]
    async fn api_malformed_close_time_and_unknown_timezone_are_refused_422() {
        let pool = test_pool().await;
        for close_time in ["nonsense", "99:99", "24:00", "16:60", "9:30", "1600", ""] {
            let body = serde_json::json!({
                "name": "Test Exchange",
                "country": "Testland",
                "currency": "AUD",
                "timezone": "UTC",
                "settlement_days": 2,
                "close_time": close_time
            });
            let resp = client(&pool).put("/exchanges/XTES", &body).await;
            let (status, detail) = resp.status_and_body();
            assert_eq!(
                status,
                StatusCode::UNPROCESSABLE_ENTITY,
                "close_time {close_time:?} must be refused 422"
            );
            assert!(
                detail.contains("close_time"),
                "the 422 must name the field, got: {detail}"
            );
            assert!(
                db_get(&pool, "XTES").await.unwrap().is_none(),
                "close_time {close_time:?} must not be stored"
            );
        }
        for timezone in ["Mars/Olympus", "Not/AZone", ""] {
            let body = serde_json::json!({
                "name": "Test Exchange",
                "country": "Testland",
                "currency": "AUD",
                "timezone": timezone,
                "settlement_days": 2,
                "close_time": "16:00"
            });
            let resp = client(&pool).put("/exchanges/XTES", &body).await;
            let (status, detail) = resp.status_and_body();
            assert_eq!(
                status,
                StatusCode::UNPROCESSABLE_ENTITY,
                "timezone {timezone:?} must be refused 422"
            );
            assert!(
                detail.contains("timezone"),
                "the 422 must name the field, got: {detail}"
            );
            assert!(
                db_get(&pool, "XTES").await.unwrap().is_none(),
                "timezone {timezone:?} must not be stored"
            );
        }

        let body = serde_json::json!({
            "name": "Test Exchange",
            "country": "Testland",
            "currency": "AUD",
            "timezone": "Australia/Sydney",
            "settlement_days": 2,
            "close_time": "16:10"
        });
        client(&pool).put_ok("/exchanges/XTES", &body).await;
        let got = db_get(&pool, "XTES").await.unwrap().unwrap();
        assert_eq!(got.close_time, "16:10");
        assert_eq!(got.timezone, "Australia/Sydney");
    }

    /// The two checks live in [`db_upsert`] — the one write path — so a direct
    /// call is refused too and persists nothing.
    #[tokio::test]
    async fn db_upsert_rejects_malformed_close_time_and_unknown_timezone() {
        let pool = test_pool().await;
        let mut ex = xtest();
        ex.close_time = "nonsense".to_string();
        assert!(matches!(
            db_upsert(&pool, &ex).await.unwrap_err(),
            UpsertError::MalformedCloseTime(v) if v == "nonsense"
        ));
        ex.close_time = "16:00".to_string();
        ex.timezone = "Mars/Olympus".to_string();
        assert!(matches!(
            db_upsert(&pool, &ex).await.unwrap_err(),
            UpsertError::UnknownTimezone(v) if v == "Mars/Olympus"
        ));
        assert!(db_get(&pool, "XTES").await.unwrap().is_none());

        // The ordinary seeded shapes are accepted: a real zone and `HH:MM`.
        ex.timezone = "UTC".to_string();
        db_upsert(&pool, &ex).await.unwrap();
        assert_eq!(
            db_get(&pool, "XTES").await.unwrap().unwrap().close_time,
            "16:00"
        );
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
