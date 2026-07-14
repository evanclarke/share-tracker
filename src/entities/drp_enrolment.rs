//! DRP (Dividend Reinvestment Plan) enrolment periods per holding.
//!
//! A holding's DRP participation is a series of dated periods: `enrolment_date`
//! (inclusive) to `unenrolment_date` (exclusive; `NULL` = open-ended, i.e.
//! currently enrolled). A holding can start unenrolled, enrol, unenrol, and
//! re-enrol; each period carries its own `residual_handling`, so a re-enrolment
//! may choose differently. A distribution reinvests when its ex date (or pay
//! date, when no ex date is recorded) falls inside a period — see
//! `entities::drp_reinvestment`. Partial participation is out of scope.
//!
//! Write-time invariants, validated atomically in a transaction (`422`):
//! periods for a listing must not overlap, and at most one may be open at a
//! time (an open period overlaps everything after its start, so non-overlap
//! implies it); a closed period must end after it starts.
//!
//! Closing a period settles its trailing residual: the registry refunds the
//! leftover cash at unenrolment, so the latest in-period DRP trade's
//! `residual_carried_forward` is moved to `residual_paid_out` in the same
//! transaction. Deleting a period record means "it never existed" and settles
//! nothing.

use crate::infra::decimal::parse_dec;
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
use sqlx::SqlitePool;

#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize, sqlx::Type)]
pub enum ResidualHandling {
    /// Leftover cash is carried forward and added to the next reinvestment.
    #[default]
    CarryForward,
    /// Leftover cash is paid out rather than carried.
    PayOut,
}

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct DrpEnrolment {
    pub id: i64,
    /// Listing this enrolment period applies to.
    pub listing_id: i64,
    /// The holding account the enrolment applies to: enrolment is per
    /// (listing, holding account), so the same listing may be enrolled in one
    /// account and unenrolled in another (e.g. an employer share-plan account
    /// that cannot DRP alongside an enrolled personal account). Defaults to
    /// the seeded default account when omitted from a request.
    pub holding_account_id: i64,
    /// First day of the period (inclusive).
    pub enrolment_date: NaiveDate,
    /// Day the unenrolment takes effect (exclusive): distributions with an ex
    /// date on or after it no longer reinvest. `None` = open-ended (currently
    /// enrolled).
    pub unenrolment_date: Option<NaiveDate>,
    pub residual_handling: ResidualHandling,
}

#[derive(Debug, Deserialize)]
pub struct DrpEnrolmentBody {
    pub listing_id: i64,
    /// Defaults to the seeded default holding account when omitted.
    #[serde(default = "crate::entities::holding_account::default_holding_account_id")]
    pub holding_account_id: i64,
    pub enrolment_date: NaiveDate,
    #[serde(default)]
    pub unenrolment_date: Option<NaiveDate>,
    #[serde(default)]
    pub residual_handling: ResidualHandling,
}

#[derive(Debug)]
pub enum UpsertError {
    /// The unenrolment date is not after the enrolment date.
    EmptyPeriod,
    /// The period overlaps another period for the same listing (two open
    /// periods always overlap, so this also enforces at-most-one-open).
    Overlap,
    Db(sqlx::Error),
}

impl From<sqlx::Error> for UpsertError {
    fn from(e: sqlx::Error) -> Self {
        UpsertError::Db(e)
    }
}

pub fn router() -> Router<SqlitePool> {
    Router::new().route("/drp_enrolments", get(list)).route(
        "/drp_enrolments/{id}",
        get(get_one).put(upsert).delete(delete),
    )
}

const COLUMNS: &str =
    "id, listing_id, holding_account_id, enrolment_date, unenrolment_date, residual_handling";

pub async fn db_list(pool: &SqlitePool) -> Result<Vec<DrpEnrolment>, sqlx::Error> {
    sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT {COLUMNS} FROM drp_enrolments \
         ORDER BY listing_id, holding_account_id, enrolment_date, id"
    )))
    .fetch_all(pool)
    .await
}

pub async fn db_get(pool: &SqlitePool, id: i64) -> Result<Option<DrpEnrolment>, sqlx::Error> {
    sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT {COLUMNS} FROM drp_enrolments WHERE id = ?"
    )))
    .bind(id)
    .fetch_optional(pool)
    .await
}

/// Upsert an enrolment period, enforcing the no-overlap invariant and settling
/// a closed period's trailing residual, all in one transaction.
pub async fn db_upsert(pool: &SqlitePool, period: &DrpEnrolment) -> Result<(), UpsertError> {
    if let Some(end) = period.unenrolment_date
        && end <= period.enrolment_date
    {
        return Err(UpsertError::EmptyPeriod);
    }

    let mut tx = pool.begin().await?;

    // Half-open [start, end) overlap test against the (listing, holding
    // account)'s other periods — the same listing's periods in *another*
    // account are independent: overlap iff new.start < other.end AND
    // other.start < new.end, where an open end is unbounded. Touching periods
    // (end == next start) are fine.
    let overlapping: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM drp_enrolments \
         WHERE listing_id = ? AND holding_account_id = ? AND id != ? \
           AND (unenrolment_date IS NULL OR ? < unenrolment_date) \
           AND (? IS NULL OR enrolment_date < ?)",
    )
    .bind(period.listing_id)
    .bind(period.holding_account_id)
    .bind(period.id)
    .bind(period.enrolment_date)
    .bind(period.unenrolment_date)
    .bind(period.unenrolment_date)
    .fetch_one(&mut *tx)
    .await?;
    if overlapping > 0 {
        return Err(UpsertError::Overlap);
    }

    sqlx::query(
        "INSERT INTO drp_enrolments \
         (id, listing_id, holding_account_id, enrolment_date, unenrolment_date, residual_handling) \
         VALUES (?, ?, ?, ?, ?, ?) \
         ON CONFLICT(id) DO UPDATE SET \
           listing_id = excluded.listing_id, \
           holding_account_id = excluded.holding_account_id, \
           enrolment_date = excluded.enrolment_date, \
           unenrolment_date = excluded.unenrolment_date, \
           residual_handling = excluded.residual_handling",
    )
    .bind(period.id)
    .bind(period.listing_id)
    .bind(period.holding_account_id)
    .bind(period.enrolment_date)
    .bind(period.unenrolment_date)
    .bind(period.residual_handling)
    .execute(&mut *tx)
    .await?;

    // Unenrolment pays out the period's trailing residual: the leftover the
    // latest in-period reinvestment carried forward has no successor to pick it
    // up, so the registry refunds it. Recorded on that DRP trade by moving
    // carried-forward to paid-out. Idempotent — once moved, carried is zero —
    // and re-saving a closed period re-settles after any backdated reinvestment.
    if let Some(end) = period.unenrolment_date {
        let trailing: Option<(i64, String, String)> = sqlx::query_as(
            "SELECT id, residual_carried_forward, residual_paid_out FROM trades \
             WHERE listing_id = ? AND holding_account_id = ? \
               AND trade_type = 'DRP' AND date >= ? AND date < ? \
             ORDER BY date DESC, id DESC LIMIT 1",
        )
        .bind(period.listing_id)
        .bind(period.holding_account_id)
        .bind(period.enrolment_date)
        .bind(end)
        .fetch_optional(&mut *tx)
        .await?;
        if let Some((trade_id, carried, paid)) = trailing {
            let carried = parse_dec("residual_carried_forward", carried)?;
            let paid = parse_dec("residual_paid_out", paid)?;
            if carried > Decimal::ZERO {
                sqlx::query(
                    "UPDATE trades SET residual_carried_forward = '0', residual_paid_out = ? \
                     WHERE id = ?",
                )
                .bind((paid + carried).to_string())
                .bind(trade_id)
                .execute(&mut *tx)
                .await?;
            }
        }
    }

    tx.commit().await?;
    Ok(())
}

pub async fn db_delete(pool: &SqlitePool, id: i64) -> Result<bool, sqlx::Error> {
    let result = sqlx::query("DELETE FROM drp_enrolments WHERE id = ?")
        .bind(id)
        .execute(pool)
        .await?;
    Ok(result.rows_affected() > 0)
}

async fn list(State(pool): State<SqlitePool>) -> Result<Json<Vec<DrpEnrolment>>, ApiError> {
    db_list(&pool).await.map(Json).map_err(ApiError::from)
}

async fn get_one(
    State(pool): State<SqlitePool>,
    Path(id): Path<i64>,
) -> Result<Json<DrpEnrolment>, ApiError> {
    db_get(&pool, id)
        .await
        .map_err(ApiError::from)?
        .map(Json)
        .ok_or(ApiError::NotFound)
}

async fn upsert(
    State(pool): State<SqlitePool>,
    Path(id): Path<i64>,
    Json(body): Json<DrpEnrolmentBody>,
) -> Result<StatusCode, ApiError> {
    let period = DrpEnrolment {
        id,
        listing_id: body.listing_id,
        holding_account_id: body.holding_account_id,
        enrolment_date: body.enrolment_date,
        unenrolment_date: body.unenrolment_date,
        residual_handling: body.residual_handling,
    };
    db_upsert(&pool, &period).await?;
    Ok(StatusCode::NO_CONTENT)
}

impl From<UpsertError> for ApiError {
    fn from(e: UpsertError) -> Self {
        match e {
            UpsertError::EmptyPeriod => {
                ApiError::unprocessable("the unenrolment date must be after the enrolment date")
            }
            UpsertError::Overlap => ApiError::unprocessable(
                "this period overlaps an existing enrolment for the same listing and account \
                 (a listing can have at most one open period per account)",
            ),
            // A bad listing_id violates the FK to listings → 422 via the shared map.
            UpsertError::Db(err) => err.into(),
        }
    }
}

async fn delete(
    State(pool): State<SqlitePool>,
    Path(id): Path<i64>,
) -> Result<StatusCode, ApiError> {
    db_delete(&pool, id)
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
    use crate::entities::listing;
    use crate::test_support::{self, test_pool};
    use axum::{body::Body, http::Request};
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    async fn insert_listing(pool: &SqlitePool, id: i64) {
        test_support::listing(id)
            .security_type(listing::SecurityType::Share)
            .insert(pool)
            .await;
    }

    fn d(s: &str) -> NaiveDate {
        s.parse().unwrap()
    }

    fn period(
        id: i64,
        listing_id: i64,
        from: &str,
        to: Option<&str>,
        handling: ResidualHandling,
    ) -> DrpEnrolment {
        DrpEnrolment {
            holding_account_id: 1,
            id,
            listing_id,
            enrolment_date: d(from),
            unenrolment_date: to.map(d),
            residual_handling: handling,
        }
    }

    #[tokio::test]
    async fn db_insert_and_retrieve_period() {
        let pool = test_pool().await;
        insert_listing(&pool, 1).await;
        db_upsert(
            &pool,
            &period(1, 1, "2024-01-01", None, ResidualHandling::PayOut),
        )
        .await
        .unwrap();
        let got = db_get(&pool, 1).await.unwrap().unwrap();
        assert_eq!(got.listing_id, 1);
        assert_eq!(got.enrolment_date, d("2024-01-01"));
        assert_eq!(
            got.unenrolment_date, None,
            "open-ended = currently enrolled"
        );
        assert_eq!(got.residual_handling, ResidualHandling::PayOut);
    }

    #[tokio::test]
    async fn db_re_enrolment_after_unenrolment_allowed() {
        let pool = test_pool().await;
        insert_listing(&pool, 1).await;
        // Enrolled, unenrolled, then re-enrolled open-ended with a different
        // residual handling — the exact history the period model exists for.
        db_upsert(
            &pool,
            &period(
                1,
                1,
                "2023-01-01",
                Some("2024-01-01"),
                ResidualHandling::CarryForward,
            ),
        )
        .await
        .unwrap();
        db_upsert(
            &pool,
            &period(2, 1, "2025-01-01", None, ResidualHandling::PayOut),
        )
        .await
        .unwrap();
        let all = db_list(&pool).await.unwrap();
        assert_eq!(all.len(), 2);
        assert_eq!(all[0].unenrolment_date, Some(d("2024-01-01")));
        assert_eq!(all[1].unenrolment_date, None);
        assert_eq!(all[1].residual_handling, ResidualHandling::PayOut);
    }

    #[tokio::test]
    async fn db_overlapping_periods_rejected() {
        let pool = test_pool().await;
        insert_listing(&pool, 1).await;
        db_upsert(
            &pool,
            &period(
                1,
                1,
                "2024-01-01",
                Some("2024-06-01"),
                ResidualHandling::CarryForward,
            ),
        )
        .await
        .unwrap();

        // Closed period starting inside the existing one.
        let err = db_upsert(
            &pool,
            &period(
                2,
                1,
                "2024-03-01",
                Some("2024-09-01"),
                ResidualHandling::CarryForward,
            ),
        )
        .await
        .unwrap_err();
        assert!(matches!(err, UpsertError::Overlap));

        // Open period starting before the existing one ends.
        let err = db_upsert(
            &pool,
            &period(2, 1, "2024-05-31", None, ResidualHandling::CarryForward),
        )
        .await
        .unwrap_err();
        assert!(matches!(err, UpsertError::Overlap));

        // Touching is not overlapping: the next period may start the day the
        // previous one's unenrolment takes effect.
        db_upsert(
            &pool,
            &period(2, 1, "2024-06-01", None, ResidualHandling::CarryForward),
        )
        .await
        .unwrap();

        // Overlap on a different listing is fine.
        insert_listing(&pool, 2).await;
        db_upsert(
            &pool,
            &period(3, 2, "2024-03-01", None, ResidualHandling::CarryForward),
        )
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn db_two_open_periods_rejected() {
        let pool = test_pool().await;
        insert_listing(&pool, 1).await;
        db_upsert(
            &pool,
            &period(1, 1, "2023-01-01", None, ResidualHandling::CarryForward),
        )
        .await
        .unwrap();
        // A second open period — even starting later — always overlaps an open one.
        let err = db_upsert(
            &pool,
            &period(2, 1, "2025-01-01", None, ResidualHandling::CarryForward),
        )
        .await
        .unwrap_err();
        assert!(matches!(err, UpsertError::Overlap));
        // And a later closed period overlaps the open one too.
        let err = db_upsert(
            &pool,
            &period(
                2,
                1,
                "2025-01-01",
                Some("2026-01-01"),
                ResidualHandling::CarryForward,
            ),
        )
        .await
        .unwrap_err();
        assert!(matches!(err, UpsertError::Overlap));
    }

    #[tokio::test]
    async fn db_empty_period_rejected() {
        let pool = test_pool().await;
        insert_listing(&pool, 1).await;
        for end in ["2024-01-01", "2023-12-31"] {
            let err = db_upsert(
                &pool,
                &period(
                    1,
                    1,
                    "2024-01-01",
                    Some(end),
                    ResidualHandling::CarryForward,
                ),
            )
            .await
            .unwrap_err();
            assert!(
                matches!(err, UpsertError::EmptyPeriod),
                "end {end} is not after start"
            );
        }
    }

    #[tokio::test]
    async fn db_listing_fk_enforced() {
        let pool = test_pool().await;
        // No listing 99 exists → FK violation.
        let err = db_upsert(
            &pool,
            &period(1, 99, "2024-01-01", None, ResidualHandling::CarryForward),
        )
        .await
        .unwrap_err();
        assert!(
            matches!(err, UpsertError::Db(_)),
            "FK to listings should reject an unknown listing_id"
        );
    }

    #[tokio::test]
    async fn db_residual_handling_enum_constraint_enforced() {
        let pool = test_pool().await;
        insert_listing(&pool, 1).await;
        let err = sqlx::query(
            "INSERT INTO drp_enrolments (listing_id, enrolment_date, residual_handling) \
             VALUES (1, '2024-01-01', 'Bogus')",
        )
        .execute(&pool)
        .await;
        assert!(
            err.is_err(),
            "CHECK constraint should reject an unknown residual_handling"
        );
    }

    /// Insert a DRP trade directly (the shape `drp_reinvestment` creates) so the
    /// unenrolment settlement has a residual chain to act on.
    async fn insert_drp_trade(
        pool: &SqlitePool,
        id: i64,
        listing_id: i64,
        date: &str,
        carried: &str,
        paid: &str,
    ) {
        sqlx::query(
            "INSERT INTO trades \
             (id, trade_type, date, settlement_date, listing_id, average_price, quantity, currency, \
              brokerage, gst_on_brokerage, brokerage_currency, fx_rate, contract_note_ref, \
              residual_brought_forward, residual_carried_forward, residual_paid_out) \
             VALUES (?, 'DRP', ?, ?, ?, '9', '11', 'AUD', '0', '0', 'AUD', '1', NULL, '0', ?, ?)",
        )
        .bind(id)
        .bind(date)
        .bind(date)
        .bind(listing_id)
        .bind(carried)
        .bind(paid)
        .execute(pool)
        .await
        .unwrap();
    }

    async fn residuals(pool: &SqlitePool, trade_id: i64) -> (String, String) {
        sqlx::query_as(
            "SELECT residual_carried_forward, residual_paid_out FROM trades WHERE id = ?",
        )
        .bind(trade_id)
        .fetch_one(pool)
        .await
        .unwrap()
    }

    #[tokio::test]
    async fn db_unenrolment_pays_out_trailing_carried_residual() {
        let pool = test_pool().await;
        insert_listing(&pool, 1).await;
        db_upsert(
            &pool,
            &period(1, 1, "2024-01-01", None, ResidualHandling::CarryForward),
        )
        .await
        .unwrap();
        // Two reinvestments in the period; only the latest carries the trailing residual.
        insert_drp_trade(&pool, 10, 1, "2024-03-31", "1", "0").await;
        insert_drp_trade(&pool, 11, 1, "2024-09-30", "2.50", "0").await;

        // Unenrol: the open period is closed and the trailing residual paid out.
        db_upsert(
            &pool,
            &period(
                1,
                1,
                "2024-01-01",
                Some("2025-01-01"),
                ResidualHandling::CarryForward,
            ),
        )
        .await
        .unwrap();

        let (carried, paid) = residuals(&pool, 11).await;
        assert_eq!(carried, "0", "the refunded residual is no longer carried");
        assert_eq!(
            paid, "2.50",
            "the registry refunds the leftover at unenrolment"
        );
        // The earlier reinvestment's historical snapshot is untouched.
        assert_eq!(
            residuals(&pool, 10).await,
            ("1".to_string(), "0".to_string())
        );

        // Re-saving the closed period is idempotent — nothing is paid twice.
        db_upsert(
            &pool,
            &period(
                1,
                1,
                "2024-01-01",
                Some("2025-01-01"),
                ResidualHandling::CarryForward,
            ),
        )
        .await
        .unwrap();
        assert_eq!(
            residuals(&pool, 11).await,
            ("0".to_string(), "2.50".to_string())
        );
    }

    #[tokio::test]
    async fn db_unenrolment_only_settles_trades_inside_the_period() {
        let pool = test_pool().await;
        insert_listing(&pool, 1).await;
        // A DRP trade from before the period must not be touched by its closure.
        insert_drp_trade(&pool, 10, 1, "2023-06-30", "5", "0").await;
        db_upsert(
            &pool,
            &period(
                1,
                1,
                "2024-01-01",
                Some("2025-01-01"),
                ResidualHandling::CarryForward,
            ),
        )
        .await
        .unwrap();
        assert_eq!(
            residuals(&pool, 10).await,
            ("5".to_string(), "0".to_string())
        );
    }

    /// Enrolment is per (listing, holding account): the same listing may hold
    /// an open-ended period in two accounts at once, while two open periods
    /// in one account still overlap.
    #[tokio::test]
    async fn db_same_listing_may_be_enrolled_per_account() {
        let pool = test_pool().await;
        insert_listing(&pool, 1).await;
        crate::entities::holding_account::db_upsert(
            &pool,
            &crate::entities::holding_account::HoldingAccount {
                id: 2,
                name: "ICE Employee Plan".to_string(),
            },
        )
        .await
        .unwrap();

        db_upsert(
            &pool,
            &period(1, 1, "2024-01-01", None, ResidualHandling::CarryForward),
        )
        .await
        .unwrap();

        // Same listing, other account: a coexisting open period is fine.
        let mut other_account = period(2, 1, "2024-01-01", None, ResidualHandling::PayOut);
        other_account.holding_account_id = 2;
        db_upsert(&pool, &other_account).await.unwrap();

        // A second open period within account 2 still overlaps.
        let mut doubly_open = period(3, 1, "2025-01-01", None, ResidualHandling::PayOut);
        doubly_open.holding_account_id = 2;
        assert!(matches!(
            db_upsert(&pool, &doubly_open).await,
            Err(UpsertError::Overlap)
        ));
    }

    /// Closing a period settles the trailing residual of *its* account's
    /// chain only — another account's DRP trades on the same listing are
    /// untouched.
    #[tokio::test]
    async fn db_unenrolment_only_settles_the_periods_accounts_trades() {
        let pool = test_pool().await;
        insert_listing(&pool, 1).await;
        crate::entities::holding_account::db_upsert(
            &pool,
            &crate::entities::holding_account::HoldingAccount {
                id: 2,
                name: "Personal CHESS".to_string(),
            },
        )
        .await
        .unwrap();
        // An in-period DRP trade, but in account 2.
        insert_drp_trade(&pool, 10, 1, "2024-06-30", "5", "0").await;
        sqlx::query("UPDATE trades SET holding_account_id = 2 WHERE id = 10")
            .execute(&pool)
            .await
            .unwrap();

        // Closing the account-1 period leaves account 2's chain alone.
        db_upsert(
            &pool,
            &period(
                1,
                1,
                "2024-01-01",
                Some("2025-01-01"),
                ResidualHandling::CarryForward,
            ),
        )
        .await
        .unwrap();
        assert_eq!(
            residuals(&pool, 10).await,
            ("5".to_string(), "0".to_string())
        );
    }

    #[tokio::test]
    async fn api_put_get_delete_round_trip() {
        let pool = test_pool().await;
        insert_listing(&pool, 1).await;
        let resp = router()
            .with_state(pool.clone())
            .oneshot(
                Request::builder()
                    .method("PUT")
                    .uri("/drp_enrolments/1")
                    .header("content-type", "application/json")
                    // No residual_handling → default CarryForward.
                    .body(Body::from(
                        r#"{"listing_id":1,"enrolment_date":"2024-01-01"}"#,
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NO_CONTENT);

        let resp = router()
            .with_state(pool.clone())
            .oneshot(
                Request::builder()
                    .uri("/drp_enrolments")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let items: Vec<DrpEnrolment> = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].residual_handling, ResidualHandling::CarryForward);
        assert_eq!(items[0].unenrolment_date, None);

        let resp = router()
            .with_state(pool.clone())
            .oneshot(
                Request::builder()
                    .method("DELETE")
                    .uri("/drp_enrolments/1")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NO_CONTENT);
        assert!(db_get(&pool, 1).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn api_put_overlapping_period_returns_422() {
        let pool = test_pool().await;
        insert_listing(&pool, 1).await;
        db_upsert(
            &pool,
            &period(1, 1, "2024-01-01", None, ResidualHandling::CarryForward),
        )
        .await
        .unwrap();
        let resp = router()
            .with_state(pool)
            .oneshot(
                Request::builder()
                    .method("PUT")
                    .uri("/drp_enrolments/2")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        r#"{"listing_id":1,"enrolment_date":"2025-01-01"}"#,
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY);
        let bytes = BodyExt::collect(resp.into_body()).await.unwrap().to_bytes();
        let detail = String::from_utf8(bytes.to_vec()).unwrap();
        assert!(detail.contains("overlaps"), "detail: {detail}");
    }

    #[tokio::test]
    async fn api_put_bad_listing_returns_422() {
        let pool = test_pool().await;
        let resp = router()
            .with_state(pool)
            .oneshot(
                Request::builder()
                    .method("PUT")
                    .uri("/drp_enrolments/1")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        r#"{"listing_id":99,"enrolment_date":"2024-01-01"}"#,
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY);
    }

    #[tokio::test]
    async fn api_get_missing_returns_404() {
        let pool = test_pool().await;
        let resp = router()
            .with_state(pool)
            .oneshot(
                Request::builder()
                    .uri("/drp_enrolments/1")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }
}
