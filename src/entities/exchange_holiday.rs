//! Exchange public holidays (full-closure non-trading days).
//!
//! Settlement-date calculation advances by *business* days, skipping weekends
//! and the exchange's public holidays. This module owns the `exchange_holidays`
//! table (one row per `(mic, holiday_date)`) and exposes
//! [`exchange_holidays_for_listing`], which the trade/sell settlement logic uses
//! to look up the holiday set for a listing's exchange.

use crate::infra::http::ApiError;
use axum::{
    Json, Router,
    extract::{Path, State},
    http::StatusCode,
    routing::get,
};
use chrono::NaiveDate;
use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;
use std::collections::HashSet;

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct ExchangeHoliday {
    pub mic: String,
    pub holiday_date: NaiveDate,
    /// Holiday name; informational only (not used by any calculation).
    pub name: String,
}

#[derive(Debug, Deserialize)]
pub struct ExchangeHolidayBody {
    pub name: String,
}

pub fn router() -> Router<SqlitePool> {
    Router::new()
        .route("/exchange_holidays", get(list))
        .route("/exchange_holidays/{mic}", get(list_for_exchange))
        .route(
            "/exchange_holidays/{mic}/{date}",
            get(get_one).put(upsert).delete(delete),
        )
}

pub async fn db_list(pool: &SqlitePool) -> Result<Vec<ExchangeHoliday>, sqlx::Error> {
    sqlx::query_as(
        "SELECT mic, holiday_date, name FROM exchange_holidays ORDER BY mic, holiday_date",
    )
    .fetch_all(pool)
    .await
}

pub async fn db_list_for_exchange(
    pool: &SqlitePool,
    mic: &str,
) -> Result<Vec<ExchangeHoliday>, sqlx::Error> {
    sqlx::query_as(
        "SELECT mic, holiday_date, name FROM exchange_holidays WHERE mic = ? ORDER BY holiday_date",
    )
    .bind(mic)
    .fetch_all(pool)
    .await
}

pub async fn db_get(
    pool: &SqlitePool,
    mic: &str,
    date: NaiveDate,
) -> Result<Option<ExchangeHoliday>, sqlx::Error> {
    sqlx::query_as(
        "SELECT mic, holiday_date, name FROM exchange_holidays WHERE mic = ? AND holiday_date = ?",
    )
    .bind(mic)
    .bind(date)
    .fetch_optional(pool)
    .await
}

pub async fn db_upsert(pool: &SqlitePool, holiday: &ExchangeHoliday) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO exchange_holidays (mic, holiday_date, name) VALUES (?, ?, ?) \
         ON CONFLICT(mic, holiday_date) DO UPDATE SET name = excluded.name",
    )
    .bind(&holiday.mic)
    .bind(holiday.holiday_date)
    .bind(&holiday.name)
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn db_delete(pool: &SqlitePool, mic: &str, date: NaiveDate) -> Result<bool, sqlx::Error> {
    let result = sqlx::query("DELETE FROM exchange_holidays WHERE mic = ? AND holiday_date = ?")
        .bind(mic)
        .bind(date)
        .execute(pool)
        .await?;
    Ok(result.rows_affected() > 0)
}

/// The calendar-year span a seeded holiday set covers: a published calendar
/// covers its whole year, so coverage runs from 1 Jan of the earliest seeded
/// holiday's year to 31 Dec of the latest's. `None` when no holidays are seeded
/// (the exchange has no coverage at all).
pub(crate) fn coverage_span(holidays: &HashSet<NaiveDate>) -> Option<(NaiveDate, NaiveDate)> {
    let earliest = holidays.iter().min()?;
    let latest = holidays.iter().max()?;
    Some(coverage_span_for(*earliest, *latest))
}

/// The coverage span implied by the earliest and latest seeded holiday dates
/// (see [`coverage_span`]); shared with the settlement-holiday-coverage report,
/// which reads the bounds via SQL `MIN`/`MAX`.
pub(crate) fn coverage_span_for(earliest: NaiveDate, latest: NaiveDate) -> (NaiveDate, NaiveDate) {
    use chrono::Datelike;
    (
        NaiveDate::from_ymd_opt(earliest.year(), 1, 1).unwrap(),
        NaiveDate::from_ymd_opt(latest.year(), 12, 31).unwrap(),
    )
}

/// Whether any day of the `[start, end]` window falls outside the coverage
/// span. A `None` span (no seeded holidays) covers nothing, so any window is
/// outside it. Used to warn/flag when a settlement window is computed against
/// an incomplete holiday calendar rather than silently degrading to
/// weekend-only skipping.
pub(crate) fn window_outside_coverage(
    start: NaiveDate,
    end: NaiveDate,
    span: Option<(NaiveDate, NaiveDate)>,
) -> bool {
    match span {
        None => true,
        Some((covered_from, covered_to)) => start < covered_from || end > covered_to,
    }
}

/// The set of public-holiday dates for the exchange a listing trades on. Used by
/// settlement-date calculation so a settlement never lands on a non-trading day.
pub(crate) async fn exchange_holidays_for_listing(
    pool: &SqlitePool,
    listing_id: i64,
) -> Result<HashSet<NaiveDate>, sqlx::Error> {
    let dates: Vec<NaiveDate> = sqlx::query_scalar(
        "SELECT eh.holiday_date FROM listings l \
         JOIN exchange_holidays eh ON eh.mic = l.exchange_mic \
         WHERE l.id = ?",
    )
    .bind(listing_id)
    .fetch_all(pool)
    .await?;
    Ok(dates.into_iter().collect())
}

async fn list(State(pool): State<SqlitePool>) -> Result<Json<Vec<ExchangeHoliday>>, ApiError> {
    db_list(&pool).await.map(Json).map_err(ApiError::from)
}

async fn list_for_exchange(
    State(pool): State<SqlitePool>,
    Path(mic): Path<String>,
) -> Result<Json<Vec<ExchangeHoliday>>, ApiError> {
    db_list_for_exchange(&pool, &mic)
        .await
        .map(Json)
        .map_err(ApiError::from)
}

async fn get_one(
    State(pool): State<SqlitePool>,
    Path((mic, date)): Path<(String, String)>,
) -> Result<Json<ExchangeHoliday>, ApiError> {
    let date: NaiveDate = date
        .parse()
        .map_err(|_| ApiError::bad_request("the holiday date is not a valid date"))?;
    db_get(&pool, &mic, date)
        .await
        .map_err(ApiError::from)?
        .map(Json)
        .ok_or(ApiError::NotFound)
}

async fn upsert(
    State(pool): State<SqlitePool>,
    Path((mic, date)): Path<(String, String)>,
    Json(body): Json<ExchangeHolidayBody>,
) -> Result<StatusCode, ApiError> {
    let holiday_date: NaiveDate = date
        .parse()
        .map_err(|_| ApiError::bad_request("the holiday date is not a valid date"))?;
    let holiday = ExchangeHoliday {
        mic,
        holiday_date,
        name: body.name,
    };
    db_upsert(&pool, &holiday)
        .await
        .map(|_| StatusCode::NO_CONTENT)
        .map_err(ApiError::from)
}

async fn delete(
    State(pool): State<SqlitePool>,
    Path((mic, date)): Path<(String, String)>,
) -> Result<StatusCode, ApiError> {
    let date: NaiveDate = date
        .parse()
        .map_err(|_| ApiError::bad_request("the holiday date is not a valid date"))?;
    db_delete(&pool, &mic, date)
        .await
        .map(|found| {
            if found {
                StatusCode::NO_CONTENT
            } else {
                StatusCode::NOT_FOUND
            }
        })
        .map_err(ApiError::from)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{self, test_pool, ymd};
    use axum::{body::Body, http::Request};
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    async fn insert_listing(pool: &SqlitePool, id: i64, mic: &str) {
        test_support::listing(id)
            .mic(mic)
            .currency(if mic == "XNYS" { "USD" } else { "AUD" })
            .insert(pool)
            .await;
    }

    // Coverage-span helpers

    #[test]
    fn coverage_span_spans_whole_calendar_years() {
        let holidays: HashSet<NaiveDate> = [ymd(2024, 4, 25), ymd(2026, 12, 25), ymd(2025, 1, 1)]
            .into_iter()
            .collect();
        // A published calendar covers its whole year, not just its listed days.
        assert_eq!(
            coverage_span(&holidays),
            Some((ymd(2024, 1, 1), ymd(2026, 12, 31)))
        );
        assert_eq!(coverage_span(&HashSet::new()), None);
    }

    #[test]
    fn window_outside_coverage_checks_both_ends_and_no_coverage() {
        let span = Some((ymd(2024, 1, 1), ymd(2027, 12, 31)));
        // Fully inside.
        assert!(!window_outside_coverage(
            ymd(2024, 1, 2),
            ymd(2024, 1, 4),
            span
        ));
        // The whole span is covered, boundaries included.
        assert!(!window_outside_coverage(
            ymd(2024, 1, 1),
            ymd(2027, 12, 31),
            span
        ));
        // Starts before coverage / ends after coverage (straddling counts).
        assert!(window_outside_coverage(
            ymd(2023, 12, 29),
            ymd(2024, 1, 2),
            span
        ));
        assert!(window_outside_coverage(
            ymd(2027, 12, 30),
            ymd(2028, 1, 4),
            span
        ));
        // No seeded holidays covers nothing.
        assert!(window_outside_coverage(
            ymd(2024, 1, 2),
            ymd(2024, 1, 4),
            None
        ));
    }

    // DB-level tests

    #[tokio::test]
    async fn db_insert_and_retrieve() {
        let pool = test_pool().await;
        let holiday = ExchangeHoliday {
            mic: "XASX".to_string(),
            holiday_date: ymd(2030, 1, 1),
            name: "New Year's Day".to_string(),
        };
        db_upsert(&pool, &holiday).await.unwrap();
        let got = db_get(&pool, "XASX", ymd(2030, 1, 1))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(got.name, "New Year's Day");
        assert_eq!(got.holiday_date, ymd(2030, 1, 1));
    }

    #[tokio::test]
    async fn db_upsert_updates_existing_name() {
        let pool = test_pool().await;
        let mut holiday = ExchangeHoliday {
            mic: "XASX".to_string(),
            holiday_date: ymd(2030, 1, 1),
            name: "New Year".to_string(),
        };
        db_upsert(&pool, &holiday).await.unwrap();
        holiday.name = "New Year's Day".to_string();
        db_upsert(&pool, &holiday).await.unwrap();
        let got = db_get(&pool, "XASX", ymd(2030, 1, 1))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(got.name, "New Year's Day");
    }

    #[tokio::test]
    async fn db_unknown_mic_is_rejected() {
        // The mic FK references exchanges(mic); an unknown exchange is rejected.
        let pool = test_pool().await;
        let holiday = ExchangeHoliday {
            mic: "ZZZZ".to_string(),
            holiday_date: ymd(2030, 1, 1),
            name: "Nope".to_string(),
        };
        let err = db_upsert(&pool, &holiday).await.unwrap_err();
        assert!(
            err.to_string().contains("FOREIGN KEY"),
            "expected mic FK error, got: {err}"
        );
    }

    #[tokio::test]
    async fn db_get_missing_returns_none() {
        let pool = test_pool().await;
        assert!(
            db_get(&pool, "XASX", ymd(1999, 1, 1))
                .await
                .unwrap()
                .is_none()
        );
    }

    #[tokio::test]
    async fn seed_data_has_asx_and_nyse_holidays() {
        let pool = test_pool().await;
        // ASX observes Anzac Day; NYSE observes Thanksgiving — neither is the other's.
        assert!(
            db_get(&pool, "XASX", ymd(2024, 4, 25))
                .await
                .unwrap()
                .is_some()
        );
        assert!(
            db_get(&pool, "XNYS", ymd(2024, 11, 28))
                .await
                .unwrap()
                .is_some()
        );
        // The NYSE Sunday→Monday Independence Day observance is seeded.
        assert!(
            db_get(&pool, "XNYS", ymd(2027, 7, 5))
                .await
                .unwrap()
                .is_some()
        );
    }

    #[tokio::test]
    async fn db_holidays_for_listing_returns_its_exchange_set() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "XASX").await;
        let asx = exchange_holidays_for_listing(&pool, 1).await.unwrap();
        assert!(asx.contains(&ymd(2024, 4, 25))); // Anzac Day (ASX)
        assert!(!asx.contains(&ymd(2024, 11, 28))); // Thanksgiving (NYSE only)

        insert_listing(&pool, 2, "XNYS").await;
        let nyse = exchange_holidays_for_listing(&pool, 2).await.unwrap();
        assert!(nyse.contains(&ymd(2024, 11, 28)));
        assert!(!nyse.contains(&ymd(2024, 4, 25)));
    }

    // API-level tests

    #[tokio::test]
    async fn api_list_for_exchange_returns_seeded_holidays() {
        let pool = test_pool().await;
        let resp = router()
            .with_state(pool)
            .oneshot(
                Request::builder()
                    .uri("/exchange_holidays/XASX")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let holidays: Vec<ExchangeHoliday> = serde_json::from_slice(&bytes).unwrap();
        assert!(holidays.iter().all(|h| h.mic == "XASX"));
        assert!(holidays.iter().any(|h| h.holiday_date == ymd(2024, 12, 25)));
    }

    #[tokio::test]
    async fn api_upsert_and_get() {
        let pool = test_pool().await;
        let body = serde_json::json!({ "name": "Test Holiday" });
        let resp = router()
            .with_state(pool.clone())
            .oneshot(
                Request::builder()
                    .method("PUT")
                    .uri("/exchange_holidays/XASX/2030-04-01")
                    .header("content-type", "application/json")
                    .body(Body::from(body.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NO_CONTENT);
        let got = db_get(&pool, "XASX", ymd(2030, 4, 1))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(got.name, "Test Holiday");
    }

    #[tokio::test]
    async fn api_upsert_unknown_exchange_returns_422() {
        let pool = test_pool().await;
        let body = serde_json::json!({ "name": "Test Holiday" });
        let resp = router()
            .with_state(pool)
            .oneshot(
                Request::builder()
                    .method("PUT")
                    .uri("/exchange_holidays/ZZZZ/2030-04-01")
                    .header("content-type", "application/json")
                    .body(Body::from(body.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY);
    }

    #[tokio::test]
    async fn api_delete_existing_then_404() {
        let pool = test_pool().await;
        let app = router().with_state(pool.clone());
        let resp = app
            .oneshot(
                Request::builder()
                    .method("DELETE")
                    .uri("/exchange_holidays/XASX/2024-12-25")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NO_CONTENT);

        let resp = router()
            .with_state(pool)
            .oneshot(
                Request::builder()
                    .method("DELETE")
                    .uri("/exchange_holidays/XASX/2024-12-25")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }
}
