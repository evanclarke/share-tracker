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
//! Which reinvestments a period covers is decided by their distributions'
//! entitlement dates — the date participation was fixed on and eligibility
//! judged by — never by the DRP trades' own dates, which are payment dates and
//! can fall outside the period that authorised them (see
//! [`PERIOD_TRADES_FROM_WHERE`]).
//!
//! Where each of those reinvestments' leftover cash went is then *derived*
//! from the period rather than recorded when it closes: under `PayOut` the
//! registry refunds every leftover, under `CarryForward` each is picked up by
//! the next reinvestment except the last, which is refunded once the period
//! ends ([`recompute_residuals`], run in the same transaction as every period
//! write). Deriving it is what makes the period editable — clearing a mistaken
//! unenrolment date restores the carry it paid out, and moving the date moves
//! the settlement.
//!
//! Deleting a period record means "it never existed" and settles nothing — so
//! a period that already produced reinvestments cannot be deleted at all
//! (`422`, [`DeleteError::CoversReinvestment`]): unenrolling is how a period
//! ends.

use crate::entities::income::Income;
use crate::infra::db::write_tx;
use crate::infra::decimal::{Money, parse_dec};
use crate::infra::http::{self, ApiError, CrudEntity};
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
use std::sync::LazyLock;

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

#[derive(thiserror::Error, Debug)]
pub enum UpsertError {
    /// The unenrolment date is not after the enrolment date.
    #[error("the unenrolment date must be after the enrolment date")]
    EmptyPeriod,
    /// The period overlaps another period for the same listing (two open
    /// periods always overlap, so this also enforces at-most-one-open).
    #[error("this period overlaps an existing enrolment for the same listing and account")]
    Overlap,
    #[error("DRP enrolment write failed: {0}")]
    Db(#[from] sqlx::Error),
}

/// `FROM`/`WHERE` selecting **the DRP trades a period covers**, the one place
/// that question is decided — bind `(listing_id, holding_account_id,
/// enrolment_date, unenrolment_date, unenrolment_date)`, and read the trade as
/// `t`, its funding distribution as `i`.
///
/// A reinvestment belongs to the period that *authorised* it, which is the one
/// covering its distribution's entitlement date ([`Income::ex_or_pay_date`]) —
/// the same date `drp_reinvestment` decides eligibility on. It is deliberately
/// **not** the trade date: a plan ended between a distribution going ex and
/// its payment (the ordinary way a DRP is stopped) puts the trade outside the
/// period that produced it, and every trade-date answer then contradicts the
/// ex-date one — the leftover settles nowhere, or is carried into the next
/// period under that period's handling, and a period that plainly produced a
/// reinvestment becomes deletable (SCENARIOS I-01, I-02, I-04).
///
/// The join is total in practice: a DRP trade is created only by the reinvest
/// operation, which links it to its distribution in the same transaction, and
/// `PUT /trades` refuses the type outright.
pub(crate) static PERIOD_TRADES_FROM_WHERE: LazyLock<String> = LazyLock::new(|| {
    format!(
        "FROM trades t JOIN income i ON i.reinvestment_trade_id = t.id \
         WHERE t.listing_id = ? AND t.holding_account_id = ? AND t.trade_type = 'DRP' \
           AND {date} >= ? AND (? IS NULL OR {date} < ?)",
        date = Income::EX_OR_PAY_DATE_SQL
    )
});

impl CrudEntity for DrpEnrolment {
    type Key = i64;
    const TABLE: &'static str = "drp_enrolments";
    const COLUMNS: &'static str =
        "id, listing_id, holding_account_id, enrolment_date, unenrolment_date, residual_handling";
    const ORDER_BY: &'static str = "listing_id, holding_account_id, enrolment_date, id";
    const NOUN: &'static str = "DRP enrolment";
}

pub fn router() -> Router<SqlitePool> {
    Router::new()
        .route("/drp_enrolments", get(http::list_handler::<DrpEnrolment>))
        .route(
            "/drp_enrolments/{id}",
            get(http::get_handler::<DrpEnrolment>)
                .put(upsert)
                // Hand-written: a period covering a reinvestment is refused
                // rather than deleted (see `db_delete`).
                .delete(delete),
        )
}

#[cfg(test)]
pub async fn db_list(pool: &SqlitePool) -> Result<Vec<DrpEnrolment>, sqlx::Error> {
    http::crud_list(pool).await
}

#[cfg(test)]
pub async fn db_get(pool: &SqlitePool, id: i64) -> Result<Option<DrpEnrolment>, sqlx::Error> {
    http::crud_get(pool, id).await
}

/// Upsert an enrolment period, enforcing the no-overlap invariant and settling
/// a closed period's trailing residual, all in one transaction.
pub async fn db_upsert(pool: &SqlitePool, period: &DrpEnrolment) -> Result<(), UpsertError> {
    if let Some(end) = period.unenrolment_date
        && end <= period.enrolment_date
    {
        return Err(UpsertError::EmptyPeriod);
    }

    let mut tx = write_tx(pool).await?;

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

    recompute_residuals(&mut tx, period).await?;

    tx.commit().await?;
    Ok(())
}

/// Recompute the split of every leftover in the period between *carried
/// forward* and *paid out*, from the period as it now stands.
///
/// Each of the period's reinvestments left some cash over (its
/// `residual_carried_forward + residual_paid_out`, one of which is always
/// zero — the total is what the plan did not spend, and no edit here changes
/// it). Where that cash went is entirely a function of the period: under
/// `PayOut` the registry refunds every leftover as it arises; under
/// `CarryForward` each is picked up by the next reinvestment, except the last,
/// which has no successor — so it is refunded once the period ends, and is
/// still carried while the period is open.
///
/// Deriving it rather than recording it at the moment of unenrolment is what
/// makes an edit reversible (SCENARIOS I-01, I-03): clearing a mistaken
/// unenrolment date restores the carry the closure paid out, moving the date
/// moves the settlement to whichever reinvestment is trailing under the new
/// window, and a back-dated reinvestment hands the settlement on to itself.
/// The one-way write this replaced could only ever move carried → paid, so
/// every one of those left the chain short by a leftover and the next
/// reinvestment funded from zero.
///
/// Runs on the caller's transaction — the residual split and the period
/// deciding it are one write. `drp_reinvestment` calls it too, so a
/// reinvestment entered against an already-closed period settles at once
/// rather than waiting for the period to be saved again.
pub(crate) async fn recompute_residuals(
    conn: &mut sqlx::SqliteConnection,
    period: &DrpEnrolment,
) -> Result<(), sqlx::Error> {
    // In the order the cash moved — payment order, which is the trade date —
    // so "the last one" is the reinvestment with no successor to pick its
    // leftover up. Which trades are *in* the period is the entitlement-date
    // question `PERIOD_TRADES_FROM_WHERE` answers.
    let trades: Vec<(i64, String, String)> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT t.id, t.residual_carried_forward, t.residual_paid_out {} \
         ORDER BY t.date, t.id",
        *PERIOD_TRADES_FROM_WHERE
    )))
    .bind(period.listing_id)
    .bind(period.holding_account_id)
    .bind(period.enrolment_date)
    .bind(period.unenrolment_date)
    .bind(period.unenrolment_date)
    .fetch_all(&mut *conn)
    .await?;

    let last = trades.len().saturating_sub(1);
    for (index, (trade_id, carried, paid)) in trades.iter().enumerate() {
        let carried = parse_dec("residual_carried_forward", carried.clone())?;
        let paid = parse_dec("residual_paid_out", paid.clone())?;
        let leftover = carried + paid;
        let refunded = match period.residual_handling {
            ResidualHandling::PayOut => true,
            ResidualHandling::CarryForward => index == last && period.unenrolment_date.is_some(),
        };
        let (want_carried, want_paid) = match refunded {
            true => (Decimal::ZERO, leftover),
            false => (leftover, Decimal::ZERO),
        };
        if want_carried != carried || want_paid != paid {
            sqlx::query(
                "UPDATE trades SET residual_carried_forward = ?, residual_paid_out = ? WHERE id = ?",
            )
            .bind(Money(want_carried))
            .bind(Money(want_paid))
            .bind(trade_id)
            .execute(&mut *conn)
            .await?;
        }
    }
    Ok(())
}

#[derive(thiserror::Error, Debug)]
pub enum DeleteError {
    /// The period covers a DRP trade — a reinvestment that happened *because*
    /// the holding was enrolled then. Deleting the period erases the record of
    /// why that trade exists (and the reinvestment cannot be re-created
    /// afterwards, since the distribution's ex date would no longer fall in any
    /// period), and it strands the trailing residual the last reinvestment
    /// carried forward: nothing settles it, and no later reinvestment can pick
    /// it up. Closing the period instead pays that residual out, exactly as the
    /// registry does. Mapped to `422`.
    #[error("this period covers a DRP reinvestment")]
    CoversReinvestment,
    #[error("DRP enrolment delete failed: {0}")]
    Db(#[from] sqlx::Error),
}

impl From<DeleteError> for ApiError {
    fn from(e: DeleteError) -> Self {
        match e {
            DeleteError::CoversReinvestment => ApiError::unprocessable(
                "this period covers a DRP reinvestment, so deleting it would erase the record of \
                 why that trade exists and strand the residual it carried forward — end the \
                 period by setting an unenrolment date instead (that pays the residual out), or \
                 delete the reinvestment first",
            ),
            DeleteError::Db(err) => err.into(),
        }
    }
}

/// Delete an enrolment period, `Ok(false)` when no period has that id.
///
/// Refused while the period covers a DRP trade of its own (listing, holding
/// account) — see [`DeleteError::CoversReinvestment`]. Nothing references
/// `drp_enrolments` by foreign key (a reinvestment is matched to its period by
/// date at read time), so the check is made here, in the delete's own
/// transaction.
pub async fn db_delete(pool: &SqlitePool, id: i64) -> Result<bool, DeleteError> {
    let mut tx = write_tx(pool).await?;
    let Some(period): Option<DrpEnrolment> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT {} FROM drp_enrolments WHERE id = ?",
        <DrpEnrolment as CrudEntity>::COLUMNS
    )))
    .bind(id)
    .fetch_optional(&mut *tx)
    .await?
    else {
        return Ok(false);
    };

    // The same trade set the settlement recomputes and the reinvestment chain
    // reads back from — matched on the distribution's entitlement date, not
    // the trade's own (see `PERIOD_TRADES_FROM_WHERE`), so a reinvestment paid
    // after the unenrolment still counts as the period's and this refusal
    // still fires.
    let covers_reinvestment: bool = sqlx::query_scalar(sqlx::AssertSqlSafe(format!(
        "SELECT EXISTS(SELECT 1 {})",
        *PERIOD_TRADES_FROM_WHERE
    )))
    .bind(period.listing_id)
    .bind(period.holding_account_id)
    .bind(period.enrolment_date)
    .bind(period.unenrolment_date)
    .bind(period.unenrolment_date)
    .fetch_one(&mut *tx)
    .await?;
    if covers_reinvestment {
        return Err(DeleteError::CoversReinvestment);
    }

    sqlx::query("DELETE FROM drp_enrolments WHERE id = ?")
        .bind(id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(true)
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

async fn delete(
    State(pool): State<SqlitePool>,
    Path(id): Path<i64>,
) -> Result<StatusCode, ApiError> {
    http::deleted(
        db_delete(&pool, id).await?,
        <DrpEnrolment as CrudEntity>::NOUN,
    )
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::entities::listing;
    use crate::test_support::{self, ApiClient, test_pool};

    /// Client over this module's own routes.
    fn client(pool: &SqlitePool) -> ApiClient {
        ApiClient::over(router().with_state(pool.clone()))
    }

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

    /// SCENARIOS I-03. Periods are half-open `[start, end)`, so re-enrolling
    /// on the very day an unenrolment took effect is a legitimate history —
    /// not an overlap. (A registry that stops and restarts a plan between two
    /// distributions leaves no gap to record.) The day itself belongs to the
    /// new period: nothing is covered twice.
    #[tokio::test]
    async fn db_touching_periods_are_allowed() {
        let pool = test_pool().await;
        insert_listing(&pool, 1).await;
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
            &period(2, 1, "2024-01-01", None, ResidualHandling::PayOut),
        )
        .await
        .unwrap();
        let all = db_list(&pool).await.unwrap();
        assert_eq!(all.len(), 2);
        assert_eq!(all[0].unenrolment_date, Some(d("2024-01-01")));
        assert_eq!(all[1].enrolment_date, d("2024-01-01"));
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
        // …together with the distribution that funded it: a DRP trade only
        // exists because a reinvestment created it, and which period it
        // belongs to is that distribution's entitlement-date question. The
        // fixture states the trade date as the pay date and no ex date, so
        // the two dates coincide unless a test says otherwise.
        link_funding_distribution(pool, id, listing_id, date, date).await;
    }

    /// Insert the income row a DRP trade was reinvested from, linked to it —
    /// what `drp_reinvestment::db_reinvest` writes in one transaction.
    async fn link_funding_distribution(
        pool: &SqlitePool,
        trade_id: i64,
        listing_id: i64,
        ex_date: &str,
        date_paid: &str,
    ) {
        let account: i64 = sqlx::query_scalar("SELECT holding_account_id FROM trades WHERE id = ?")
            .bind(trade_id)
            .fetch_one(pool)
            .await
            .unwrap();
        test_support::income(1000 + trade_id, listing_id, date_paid.parse().unwrap())
            .with(|i| {
                i.ex_date = Some(ex_date.parse().unwrap());
                i.unfranked_amount = Decimal::from(100);
                i.holding_account_id = account;
            })
            .insert(pool)
            .await;
        sqlx::query("UPDATE income SET reinvestment_trade_id = ? WHERE id = ?")
            .bind(trade_id)
            .bind(1000 + trade_id)
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
        // An in-period DRP trade, but in account 2 — as is the distribution
        // that funded it (a reinvestment lands in its distribution's account).
        insert_drp_trade(&pool, 10, 1, "2024-06-30", "5", "0").await;
        sqlx::query("UPDATE trades SET holding_account_id = 2 WHERE id = 10")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("UPDATE income SET holding_account_id = 2 WHERE reinvestment_trade_id = 10")
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

    /// SCENARIOS I-01, I-03. The settlement is derived from the period, not
    /// stamped on when it closes — so an edit of the unenrolment date is
    /// reversible. Clearing a mistaken one restores the carry it refunded (the
    /// one-way write this replaced left the chain short, and the next
    /// reinvestment funded from zero); moving it later hands the settlement to
    /// whichever reinvestment is trailing under the new window.
    #[tokio::test]
    async fn re_opening_or_moving_an_unenrolment_re_derives_the_settlement() {
        let pool = test_pool().await;
        insert_listing(&pool, 1).await;
        let open = period(1, 1, "2024-01-01", None, ResidualHandling::CarryForward);
        db_upsert(&pool, &open).await.unwrap();
        insert_drp_trade(&pool, 10, 1, "2024-03-31", "1", "0").await;
        insert_drp_trade(&pool, 11, 1, "2024-09-30", "2.50", "0").await;

        // Unenrol by mistake …
        let closed = period(
            1,
            1,
            "2024-01-01",
            Some("2024-10-01"),
            ResidualHandling::CarryForward,
        );
        db_upsert(&pool, &closed).await.unwrap();
        assert_eq!(
            residuals(&pool, 11).await,
            ("0".to_string(), "2.50".to_string())
        );

        // … and correct it: the leftover is carried again, not lost.
        db_upsert(&pool, &open).await.unwrap();
        assert_eq!(
            residuals(&pool, 11).await,
            ("2.50".to_string(), "0".to_string()),
            "re-opening the period restores the carry it refunded"
        );

        // Closing it *between* the two reinvestments instead: the March one is
        // now the period's last, so it settles and the September one — no
        // longer covered — is left as it stands.
        db_upsert(
            &pool,
            &period(
                1,
                1,
                "2024-01-01",
                Some("2024-06-30"),
                ResidualHandling::CarryForward,
            ),
        )
        .await
        .unwrap();
        assert_eq!(
            residuals(&pool, 10).await,
            ("0".to_string(), "1".to_string())
        );
        assert_eq!(
            residuals(&pool, 11).await,
            ("2.50".to_string(), "0".to_string())
        );
    }

    /// SCENARIOS I-01, I-02, I-04. Which period a reinvestment belongs to is
    /// its distribution's entitlement date, not the DRP trade's own date. A
    /// plan ended between a distribution going ex and its payment — the
    /// ordinary way a DRP is stopped — leaves the trade dated outside the
    /// period that authorised it, and every question about it must still find
    /// it: its leftover settles at the unenrolment, it does not leak into the
    /// next period's chain, and the period cannot be deleted out from under it.
    #[tokio::test]
    async fn a_reinvestment_paid_after_the_unenrolment_still_belongs_to_its_period() {
        let pool = test_pool().await;
        insert_listing(&pool, 1).await;
        db_upsert(
            &pool,
            &period(
                1,
                1,
                "2024-01-01",
                Some("2024-07-01"),
                ResidualHandling::CarryForward,
            ),
        )
        .await
        .unwrap();
        // Went ex 20 June (inside the period), paid — and so allotted — on
        // 15 July, after the unenrolment took effect.
        insert_drp_trade(&pool, 10, 1, "2024-07-15", "2", "0").await;
        sqlx::query("UPDATE income SET ex_date = '2024-06-20' WHERE reinvestment_trade_id = 10")
            .execute(&pool)
            .await
            .unwrap();

        // Re-saving the period settles it: the leftover is refunded, not
        // stranded on a trade no walk can see.
        db_upsert(
            &pool,
            &period(
                1,
                1,
                "2024-01-01",
                Some("2024-07-01"),
                ResidualHandling::CarryForward,
            ),
        )
        .await
        .unwrap();
        assert_eq!(
            residuals(&pool, 10).await,
            ("0".to_string(), "2".to_string())
        );

        // The period cannot be deleted: it plainly produced a reinvestment,
        // even though that trade is dated outside it.
        assert!(matches!(
            db_delete(&pool, 1).await,
            Err(DeleteError::CoversReinvestment)
        ));

        // And a period re-opened the same day does not adopt the trade: its
        // own settlement leaves the earlier period's reinvestment alone.
        db_upsert(
            &pool,
            &period(2, 1, "2024-07-01", None, ResidualHandling::PayOut),
        )
        .await
        .unwrap();
        assert_eq!(
            residuals(&pool, 10).await,
            ("0".to_string(), "2".to_string()),
            "the trade belongs to the period its distribution went ex in"
        );
    }

    /// The delete counterpart of `db_unenrolment_pays_out_trailing_carried_residual`:
    /// the same period, ended by deleting it instead of unenrolling, would
    /// strand the residual its last reinvestment carried forward — so the
    /// delete is refused and the trade is left exactly as it was.
    #[tokio::test]
    async fn db_delete_refused_while_the_period_covers_a_reinvestment() {
        let pool = test_pool().await;
        insert_listing(&pool, 1).await;
        db_upsert(
            &pool,
            &period(1, 1, "2024-01-01", None, ResidualHandling::CarryForward),
        )
        .await
        .unwrap();
        insert_drp_trade(&pool, 10, 1, "2024-09-30", "5.50", "0").await;

        assert!(matches!(
            db_delete(&pool, 1).await,
            Err(DeleteError::CoversReinvestment)
        ));
        assert!(
            db_get(&pool, 1).await.unwrap().is_some(),
            "the refused delete leaves the period in place"
        );
        assert_eq!(
            residuals(&pool, 10).await,
            ("5.50".to_string(), "0".to_string()),
            "and leaves the reinvestment's residual chain untouched"
        );

        // Unenrolling is the way out: it settles the trailing residual.
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
            ("0".to_string(), "5.50".to_string())
        );
        // Closed, but still covering the reinvestment — still refused.
        assert!(matches!(
            db_delete(&pool, 1).await,
            Err(DeleteError::CoversReinvestment)
        ));
    }

    /// The guard is scoped exactly like the period itself: a DRP trade outside
    /// its dates, or in another holding account, doesn't block the delete.
    #[tokio::test]
    async fn db_delete_allowed_when_no_reinvestment_falls_in_the_period() {
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
        // Before the period, after it, and inside it but in another account.
        insert_drp_trade(&pool, 10, 1, "2023-06-30", "1", "0").await;
        insert_drp_trade(&pool, 11, 1, "2025-06-30", "1", "0").await;
        insert_drp_trade(&pool, 12, 1, "2024-06-30", "1", "0").await;
        sqlx::query("UPDATE trades SET holding_account_id = 2 WHERE id = 12")
            .execute(&pool)
            .await
            .unwrap();

        assert!(db_delete(&pool, 1).await.unwrap());
        assert!(db_get(&pool, 1).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn api_delete_covering_period_returns_422_pointing_at_unenrolment() {
        let pool = test_pool().await;
        insert_listing(&pool, 1).await;
        db_upsert(
            &pool,
            &period(1, 1, "2024-01-01", None, ResidualHandling::CarryForward),
        )
        .await
        .unwrap();
        insert_drp_trade(&pool, 10, 1, "2024-09-30", "5.50", "0").await;

        let resp = client(&pool).delete("/drp_enrolments/1").await;
        let (status, body) = resp.status_and_body();
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
        assert!(body.contains("DRP reinvestment"), "body: {body}");
        assert!(body.contains("unenrolment date"), "body: {body}");
    }

    #[tokio::test]
    async fn api_put_get_delete_round_trip() {
        let pool = test_pool().await;
        insert_listing(&pool, 1).await;
        let resp = client(&pool)
            .put_raw(
                "/drp_enrolments/1",
                r#"{"listing_id":1,"enrolment_date":"2024-01-01"}"#,
            )
            .await;
        assert_eq!(resp.status, StatusCode::NO_CONTENT);

        let resp = client(&pool).get("/drp_enrolments").await;
        assert_eq!(resp.status, StatusCode::OK);
        let items: Vec<DrpEnrolment> = resp.json();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].residual_handling, ResidualHandling::CarryForward);
        assert_eq!(items[0].unenrolment_date, None);

        let resp = client(&pool).delete("/drp_enrolments/1").await;
        assert_eq!(resp.status, StatusCode::NO_CONTENT);
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
        let resp = client(&pool)
            .put_raw(
                "/drp_enrolments/2",
                r#"{"listing_id":1,"enrolment_date":"2025-01-01"}"#,
            )
            .await;
        assert_eq!(resp.status, StatusCode::UNPROCESSABLE_ENTITY);
        let detail = resp.text().to_string();
        assert!(detail.contains("overlaps"), "detail: {detail}");
    }

    #[tokio::test]
    async fn api_put_bad_listing_returns_422() {
        let pool = test_pool().await;
        let resp = client(&pool)
            .put_raw(
                "/drp_enrolments/1",
                r#"{"listing_id":99,"enrolment_date":"2024-01-01"}"#,
            )
            .await;
        assert_eq!(resp.status, StatusCode::UNPROCESSABLE_ENTITY);
    }

    #[tokio::test]
    async fn api_get_missing_returns_404() {
        let pool = test_pool().await;
        let resp = client(&pool).get("/drp_enrolments/1").await;
        assert_eq!(resp.status, StatusCode::NOT_FOUND);
    }
}
