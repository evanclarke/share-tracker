//! Exchange public holidays (full-closure non-trading days).
//!
//! Settlement-date calculation advances by *business* days, skipping weekends
//! and the exchange's public holidays. This module owns the `exchange_holidays`
//! table (one row per `(mic, holiday_date)`, with a surrogate `id` since 0039
//! so the audit trail can key on it) and exposes
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
    /// Server-assigned surrogate key (0039): the row's identity for the audit
    /// trail (`row_history.row_id`, so `POST /reports/row_history` can be
    /// keyed on it). Writes address a holiday by its `(mic, holiday_date)`
    /// natural key, never by this — [`db_upsert`] ignores the value it is
    /// handed and lets the database assign or preserve it.
    #[serde(default)]
    pub id: i64,
    pub mic: String,
    pub holiday_date: NaiveDate,
    /// Holiday name; informational only (not used by any calculation).
    pub name: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
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
        "SELECT id, mic, holiday_date, name FROM exchange_holidays ORDER BY mic, holiday_date",
    )
    .fetch_all(pool)
    .await
}

pub async fn db_list_for_exchange(
    pool: &SqlitePool,
    mic: &str,
) -> Result<Vec<ExchangeHoliday>, sqlx::Error> {
    sqlx::query_as(
        "SELECT id, mic, holiday_date, name FROM exchange_holidays WHERE mic = ? ORDER BY holiday_date",
    )
    .bind(mic)
    .fetch_all(pool)
    .await
}

/// One exchange's public-holiday dates as a lookup set, keyed by MIC rather
/// than by listing. Price collection needs this form because a listing that
/// has moved exchange (`listing_renames`) trades on a *different* calendar
/// before and after the move, so its `Market` holds one holiday set per
/// identity — the listing-joined `exchange_holidays_for_listing` can only
/// ever answer for the exchange the listing records today.
///
/// Executor-generic so the calendar can be read on a caller's transaction:
/// the trade write path's non-trading-day refusal loads its `Market` inside
/// its own transaction.
pub(crate) async fn db_holiday_dates_for<'e, X>(
    executor: X,
    mic: &str,
) -> Result<HashSet<NaiveDate>, sqlx::Error>
where
    X: sqlx::Executor<'e, Database = sqlx::Sqlite>,
{
    let dates: Vec<NaiveDate> =
        sqlx::query_scalar("SELECT holiday_date FROM exchange_holidays WHERE mic = ?")
            .bind(mic)
            .fetch_all(executor)
            .await?;
    Ok(dates.into_iter().collect())
}

pub async fn db_get(
    pool: &SqlitePool,
    mic: &str,
    date: NaiveDate,
) -> Result<Option<ExchangeHoliday>, sqlx::Error> {
    sqlx::query_as(
        "SELECT id, mic, holiday_date, name FROM exchange_holidays WHERE mic = ? AND holiday_date = ?",
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
/// Takes the caller's own connection, so the `settlement-recompute` job can
/// load a calendar on the transaction it rewrites settlement dates in.
pub(crate) async fn exchange_holidays_for_listing(
    conn: &mut sqlx::SqliteConnection,
    listing_id: i64,
) -> Result<HashSet<NaiveDate>, sqlx::Error> {
    let dates: Vec<NaiveDate> = sqlx::query_scalar(
        "SELECT eh.holiday_date FROM listings l \
         JOIN exchange_holidays eh ON eh.mic = l.exchange_mic \
         WHERE l.id = ?",
    )
    .bind(listing_id)
    .fetch_all(&mut *conn)
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
        // Assigned by the database on insert, preserved on update; the upsert
        // below never binds it.
        id: 0,
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
    if db_delete(&pool, &mic, date).await? {
        Ok(StatusCode::NO_CONTENT)
    } else {
        // Keyed by exchange + date rather than an id, so this one names both
        // instead of going through `infra::http::deleted`.
        Err(ApiError::not_found(
            "no exchange holiday on that date for that exchange",
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{self, ApiClient, test_pool, ymd};

    /// Client over this module's own routes.
    fn client(pool: &SqlitePool) -> ApiClient {
        ApiClient::over(router().with_state(pool.clone()))
    }

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
            id: 0,
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
            id: 0,
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
            id: 0,
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
        insert_listing(&pool, 2, "XNYS").await;
        let mut conn = pool.acquire().await.unwrap();
        let asx = exchange_holidays_for_listing(&mut conn, 1).await.unwrap();
        assert!(asx.contains(&ymd(2024, 4, 25))); // Anzac Day (ASX)
        assert!(!asx.contains(&ymd(2024, 11, 28))); // Thanksgiving (NYSE only)

        let nyse = exchange_holidays_for_listing(&mut conn, 2).await.unwrap();
        assert!(nyse.contains(&ymd(2024, 11, 28)));
        assert!(!nyse.contains(&ymd(2024, 4, 25)));
    }

    // API-level tests

    #[tokio::test]
    async fn api_list_for_exchange_returns_seeded_holidays() {
        let pool = test_pool().await;
        let resp = client(&pool).get("/exchange_holidays/XASX").await;
        assert_eq!(resp.status, StatusCode::OK);
        let holidays: Vec<ExchangeHoliday> = resp.json();
        assert!(holidays.iter().all(|h| h.mic == "XASX"));
        assert!(holidays.iter().any(|h| h.holiday_date == ymd(2024, 12, 25)));
    }

    #[tokio::test]
    async fn api_upsert_and_get() {
        let pool = test_pool().await;
        let body = serde_json::json!({ "name": "Test Holiday" });
        let resp = client(&pool)
            .put("/exchange_holidays/XASX/2030-04-01", &body)
            .await;
        assert_eq!(resp.status, StatusCode::NO_CONTENT);
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
        let resp = client(&pool)
            .put("/exchange_holidays/ZZZZ/2030-04-01", &body)
            .await;
        assert_eq!(resp.status, StatusCode::UNPROCESSABLE_ENTITY);
    }

    /// A holiday write decides which day every held listing is *valued* at —
    /// `reports::valuation::stored_valuations` reads this calendar live on
    /// every snapshot generation — so an insert, a re-dating update and a
    /// delete each mark the snapshots dated on or after the holiday stale
    /// (migration 0033, SCENARIOS Q-05/Q-08). A name-only edit changes no
    /// stored figure and deliberately fires nothing: staling years of
    /// snapshots for a "Queen's Birthday" → "King's Birthday" correction
    /// would teach a reader to ignore the flag.
    #[tokio::test]
    async fn a_holiday_write_stales_snapshots_from_its_date() {
        let pool = test_pool().await;
        for date in ["2026-06-03", "2026-06-05", "2026-06-09"] {
            sqlx::query(
                "INSERT INTO report_snapshots \
                 (report, snapshot_date, generated_at, rows_json, stale) \
                 VALUES ('portfolio_overview', ?, '2026-06-10T00:00:00Z', '[]', 0)",
            )
            .bind(date)
            .execute(&pool)
            .await
            .unwrap();
        }
        let unstale = |pool: SqlitePool| async move {
            sqlx::query("UPDATE report_snapshots SET stale = 0")
                .execute(&pool)
                .await
                .unwrap();
        };
        let stale = |pool: SqlitePool| async move {
            sqlx::query_as::<_, (String, i64)>(
                "SELECT snapshot_date, stale FROM report_snapshots ORDER BY snapshot_date",
            )
            .fetch_all(&pool)
            .await
            .unwrap()
        };
        let flags = |d3: i64, d5: i64, d9: i64| {
            vec![
                ("2026-06-03".to_string(), d3),
                ("2026-06-05".to_string(), d5),
                ("2026-06-09".to_string(), d9),
            ]
        };

        // INSERT: the Friday turns out to have been a full closure, so every
        // snapshot from it on was valued on a day the market never traded.
        let mut holiday = ExchangeHoliday {
            id: 0,
            mic: "XASX".to_string(),
            holiday_date: ymd(2026, 6, 5),
            name: "Test Closure".to_string(),
        };
        db_upsert(&pool, &holiday).await.unwrap();
        assert_eq!(stale(pool.clone()).await, flags(0, 1, 1));

        // A name-only edit changes no figure.
        unstale(pool.clone()).await;
        holiday.name = "Test Closure (renamed)".to_string();
        db_upsert(&pool, &holiday).await.unwrap();
        assert_eq!(stale(pool.clone()).await, flags(0, 0, 0));

        // UPDATE: re-dating one in place stales from the earlier of the two
        // dates — both days' valuations move. (Not reachable through the API,
        // which re-dates by delete + insert; the trigger is what the schema's
        // rule rests on.)
        unstale(pool.clone()).await;
        sqlx::query("UPDATE exchange_holidays SET holiday_date = '2026-06-04' WHERE mic = 'XASX' AND holiday_date = '2026-06-05'")
            .execute(&pool)
            .await
            .unwrap();
        assert_eq!(stale(pool.clone()).await, flags(0, 1, 1));

        // DELETE: removing one makes the date a trading day again, which is
        // just as much a re-valuation.
        unstale(pool.clone()).await;
        assert!(db_delete(&pool, "XASX", ymd(2026, 6, 4)).await.unwrap());
        assert_eq!(stale(pool.clone()).await, flags(0, 1, 1));
    }

    /// The calendar joined the **audit trail** in 0039 (SCENARIOS Q-05/Q-08,
    /// decided 2026-08-21): it is hand-editable, there is no import to
    /// re-derive it from — the seed is a one-off in 0001_schema.sql — and a
    /// holiday changes a reported figure, since valuation reads the calendar
    /// live. So a correction records the superseded row, readable through
    /// `POST /reports/row_history` keyed on the surrogate `id` the rebuild
    /// gave the table.
    #[tokio::test]
    async fn correcting_a_holiday_records_the_superseded_row() {
        let pool = test_pool().await;
        let app = ApiClient::full(&pool);

        let before = db_get(&pool, "XNYS", ymd(2026, 2, 16))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(before.name, "Washington's Birthday", "seeded name");

        app.put_ok(
            "/exchange_holidays/XNYS/2026-02-16",
            &serde_json::json!({ "name": "Presidents' Day" }),
        )
        .await;

        let history: Vec<serde_json::Value> = app
            .post_json(
                "/reports/row_history",
                &serde_json::json!({ "table": "exchange_holidays", "row_id": before.id }),
            )
            .await;
        assert_eq!(history.len(), 1, "one entry for one correction");
        assert_eq!(history[0]["operation"], "UPDATE");
        assert_eq!(history[0]["mic"], "XNYS");
        assert_eq!(history[0]["holiday_date"], "2026-02-16");
        assert_eq!(
            history[0]["name"], "Washington's Birthday",
            "the trail holds what the row said before the write"
        );
        // The row itself now carries the correction, under the same id.
        let after = db_get(&pool, "XNYS", ymd(2026, 2, 16))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(after.id, before.id, "the upsert preserves the row identity");
        assert_eq!(after.name, "Presidents' Day");
    }

    /// The delete is the case the trail exists for: nothing else in the
    /// database could say a holiday was ever there, and removing one turns
    /// the date into a trading day that changes both recomputed settlement
    /// dates and every snapshot valuation from it.
    #[tokio::test]
    async fn deleting_a_holiday_keeps_it_recoverable_from_the_trail() {
        let pool = test_pool().await;
        let app = ApiClient::full(&pool);

        let holiday = db_get(&pool, "XASX", ymd(2026, 4, 3))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(holiday.name, "Good Friday", "seeded name");

        app.delete("/exchange_holidays/XASX/2026-04-03")
            .await
            .expect_status(StatusCode::NO_CONTENT);
        assert!(
            db_get(&pool, "XASX", ymd(2026, 4, 3))
                .await
                .unwrap()
                .is_none()
        );

        let history: Vec<serde_json::Value> = app
            .post_json(
                "/reports/row_history",
                &serde_json::json!({ "table": "exchange_holidays", "row_id": holiday.id }),
            )
            .await;
        assert_eq!(history.len(), 1);
        assert_eq!(history[0]["operation"], "DELETE");
        assert_eq!(history[0]["id"], holiday.id);
        assert_eq!(history[0]["mic"], "XASX");
        assert_eq!(history[0]["holiday_date"], "2026-04-03");
        assert_eq!(
            history[0]["name"], "Good Friday",
            "every column of the deleted holiday is retained"
        );
    }

    /// The surrogate id is server-assigned and never reused, so a trail can
    /// only ever belong to one holiday: re-adding a deleted date gets a fresh
    /// id rather than inheriting the deleted row's history (0039, 0021's
    /// AUTOINCREMENT reasoning).
    #[tokio::test]
    async fn a_re_added_holiday_does_not_inherit_the_deleted_one_s_trail() {
        let pool = test_pool().await;
        let holiday = db_get(&pool, "XASX", ymd(2026, 4, 3))
            .await
            .unwrap()
            .unwrap();
        assert!(db_delete(&pool, "XASX", ymd(2026, 4, 3)).await.unwrap());
        db_upsert(
            &pool,
            &ExchangeHoliday {
                id: 0,
                mic: "XASX".to_string(),
                holiday_date: ymd(2026, 4, 3),
                name: "Good Friday".to_string(),
            },
        )
        .await
        .unwrap();
        let re_added = db_get(&pool, "XASX", ymd(2026, 4, 3))
            .await
            .unwrap()
            .unwrap();
        assert_ne!(
            re_added.id, holiday.id,
            "AUTOINCREMENT never hands back a deleted row's id"
        );
    }

    #[tokio::test]
    async fn api_delete_existing_then_404() {
        let pool = test_pool().await;
        let app = client(&pool);
        let resp = app.delete("/exchange_holidays/XASX/2024-12-25").await;
        assert_eq!(resp.status, StatusCode::NO_CONTENT);

        let resp = client(&pool)
            .delete("/exchange_holidays/XASX/2024-12-25")
            .await;
        assert_eq!(resp.status, StatusCode::NOT_FOUND);
    }
}
