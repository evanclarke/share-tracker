//! Atomic DRP reinvestment: turn a distribution into a linked DRP trade.
//!
//! Given a distribution (an `income` row) on a DRP-enrolled holding and the
//! reinvestment price, this creates the reinvestment Trade (type `DRP`) and
//! links it back to the distribution (`income.reinvestment_trade_id`) in one
//! transaction. By default the reinvestable cash plus any residual brought
//! forward from the holding's previous reinvestment is spent on whole shares;
//! the leftover is carried forward or paid out per the enrolment period's
//! residual handling.
//!
//! A broker plan that allots **fractional shares** (e.g. a US broker DRP)
//! states the allotted units on its statement and leaves no residual. For
//! those, the body's optional `units` is the broker's stated figure and is
//! authoritative: the trade takes exactly that quantity, cross-checked
//! against the available cash — `units × price` must agree with it to within
//! one unit-step at the units' stated precision (a figure stated to 3
//! decimals must be within 0.001 × price), which is the property any
//! broker-computed allotment has regardless of its rounding direction. The
//! sub-step difference is statement rounding, not cash: the residual columns
//! record zero (brought-forward cash, if any, is spent into the purchase).
//!
//! Enrolment is checked as at the distribution's ex date (registry practice:
//! DRP participation is fixed at the record date), falling back to the pay
//! date when no ex date is recorded. That date must fall inside one of the
//! enrolment periods *for the distribution's holding account*
//! (`entities::drp_enrolment` — enrolment is per (listing, holding account),
//! so an employer-plan account's distribution never reinvests off a personal
//! account's enrolment) — a distribution dated before enrolment, or in a gap
//! between unenrolment and re-enrolment, is rejected — and the matching
//! period's residual handling applies. The created DRP trade lands in the
//! distribution's holding account.
//!
//! The carried-forward residual is *not* stored as a separate running balance:
//! it lives on each DRP trade (`residual_carried_forward`), and "brought
//! forward" for the next reinvestment is read back from the most recent prior
//! DRP trade *within the same enrolment period*. That single source of truth
//! can't drift, and the chain never crosses periods — a period's trailing
//! residual is paid out at unenrolment (see `drp_enrolment::db_upsert`), not
//! picked up after re-enrolment.
//!
//! A distribution may be reinvested at most once — re-posting is rejected
//! rather than creating a second trade.
//!
//! The inverse operation, `DELETE /income/:id/reinvest`, undoes a
//! reinvestment: it deletes the DRP trade and clears the distribution's link
//! in one transaction (the only path that clears it — `PUT /income` never
//! touches the link, and `DELETE /income` is refused while it is set, so an
//! orphaned DRP trade can't exist). Refused while the trade is drawn on (a
//! Sell allocation or AMIT adjustment references it) or while a later DRP
//! trade exists for the same listing and holding account — the chain reads
//! residuals back from the most recent prior trade, so undo runs
//! last-in-first-out.

use crate::entities::{
    drp_enrolment::ResidualHandling,
    income::Income,
    trade::{self, Trade},
};
use crate::infra::decimal::parse_dec;
use crate::infra::http::ApiError;
use axum::{
    Json, Router,
    extract::{Path, State},
    http::StatusCode,
    routing::post,
};
use chrono::NaiveDate;
use rust_decimal::Decimal;
use serde::Deserialize;
use sqlx::SqlitePool;

#[derive(Debug, Deserialize)]
pub struct ReinvestBody {
    /// Per-share price the distribution is reinvested at.
    pub reinvestment_price: Decimal,
    /// Optional broker-stated fractional allotment. When present it is
    /// authoritative — the trade takes exactly this quantity — and
    /// `units × reinvestment_price` is cross-checked against the available
    /// cash to within one unit-step at the stated precision. Omitted: whole
    /// shares with residual carry (the registry default).
    #[serde(default)]
    pub units: Option<Decimal>,
    /// Optional foreign-per-AUD override for the created DRP trade (defaults to
    /// 1; reports prefer the ATO rate and fall back to this — see `infra::fx`).
    #[serde(default)]
    pub fx_rate: Option<Decimal>,
    /// Optional trade date; defaults to the distribution's `date_paid`.
    #[serde(default)]
    pub date: Option<NaiveDate>,
}

#[derive(Debug)]
pub enum ReinvestError {
    Db(sqlx::Error),
    /// No income row with that id.
    IncomeNotFound,
    /// No enrolment period covers the distribution's ex date (or pay date when
    /// no ex date is recorded): never enrolled, dated before enrolment, or in a
    /// gap between unenrolment and re-enrolment. Carries the account name,
    /// ticker, and date so the rejection can name them rather than raw ids.
    NotEnrolled {
        account: String,
        ticker: String,
        date: NaiveDate,
    },
    /// The distribution already has a reinvestment trade.
    AlreadyReinvested,
    /// The reinvestment price is not strictly positive.
    NonPositivePrice,
    /// The stated units are not strictly positive.
    NonPositiveUnits,
    /// The stated units don't spend the available cash at the given price:
    /// `units × price` differs from it by a full unit-step (at the units'
    /// stated precision) or more. Carries both figures for the rejection.
    UnitsCashMismatch {
        cost: Decimal,
        available: Decimal,
    },
    /// Undo requested on a distribution with no reinvestment trade.
    NotReinvested,
    /// Undo refused: the DRP trade is drawn on by a Sell allocation or an
    /// AMIT adjustment — deleting it would orphan those dependants. Remove
    /// them first (e.g. delete the Sell via `DELETE /sells/:id`).
    ReinvestmentConsumed,
    /// Undo refused: a later DRP trade exists for the same listing and
    /// holding account. Its `residual_brought_forward` was read from this
    /// chain, so removing a mid-chain trade would falsify it — undo the later
    /// reinvestments first (LIFO).
    ReinvestmentNotChainTail,
}

impl From<sqlx::Error> for ReinvestError {
    fn from(e: sqlx::Error) -> Self {
        ReinvestError::Db(e)
    }
}

impl From<ReinvestError> for ApiError {
    fn from(e: ReinvestError) -> Self {
        match e {
            ReinvestError::IncomeNotFound => ApiError::not_found("no distribution with that id"),
            ReinvestError::NotEnrolled {
                account,
                ticker,
                date,
            } => ApiError::unprocessable(format!(
                "account '{account}' is not enrolled in a DRP for {ticker} at {date} \
                     — enrol it on the DRP enrolments screen first"
            )),
            ReinvestError::AlreadyReinvested => ApiError::unprocessable(
                "this distribution already has a reinvestment trade — undo it first \
                 (DELETE /income/:id/reinvest) to redo it",
            ),
            ReinvestError::NonPositivePrice => {
                ApiError::unprocessable("the reinvestment price must be greater than zero")
            }
            ReinvestError::NonPositiveUnits => {
                ApiError::unprocessable("the stated units must be greater than zero")
            }
            ReinvestError::UnitsCashMismatch { cost, available } => {
                ApiError::unprocessable(format!(
                    "the stated units at the reinvestment price spend {cost}, but the \
                     reinvestable cash (including any residual brought forward) is {available} \
                     — they must agree to within one unit-step at the stated precision"
                ))
            }
            ReinvestError::NotReinvested => {
                ApiError::unprocessable("this distribution has no reinvestment trade to undo")
            }
            ReinvestError::ReinvestmentConsumed => ApiError::unprocessable(
                "the reinvestment trade is drawn on by a Sell allocation or AMIT adjustment \
                 — remove those first (e.g. delete the Sell via DELETE /sells/:id)",
            ),
            ReinvestError::ReinvestmentNotChainTail => ApiError::unprocessable(
                "a later DRP reinvestment for this listing and holding account brought this \
                 trade's residual forward — undo the later reinvestments first",
            ),
            ReinvestError::Db(err) => err.into(),
        }
    }
}

pub fn router() -> Router<SqlitePool> {
    Router::new().route("/income/{id}/reinvest", post(reinvest).delete(unreinvest))
}

/// Create the DRP trade for a distribution and link it, atomically.
pub async fn db_reinvest(
    pool: &SqlitePool,
    income_id: i64,
    body: &ReinvestBody,
) -> Result<Trade, ReinvestError> {
    if body.reinvestment_price <= Decimal::ZERO {
        return Err(ReinvestError::NonPositivePrice);
    }
    if let Some(units) = body.units
        && units <= Decimal::ZERO
    {
        return Err(ReinvestError::NonPositiveUnits);
    }

    let mut tx = pool.begin().await?;

    // Load the distribution and its cash components.
    let income: Option<Income> = sqlx::query_as("SELECT * FROM income WHERE id = ?")
        .bind(income_id)
        .fetch_optional(&mut *tx)
        .await?;
    let income = match income {
        Some(r) => r,
        None => return Err(ReinvestError::IncomeNotFound),
    };

    if income.reinvestment_trade_id.is_some() {
        return Err(ReinvestError::AlreadyReinvested);
    }

    let Income {
        listing_id,
        holding_account_id,
        date_paid,
        ..
    } = income;
    let cash = income.net_cash_received();

    // Reinvestability is decided as at the ex date (DRP participation is fixed
    // at the record date), falling back to the pay date when not recorded —
    // the model's own `ex_or_pay_date`.
    let entitlement_date = income.ex_or_pay_date();

    // That date must fall inside an enrolment period *for the distribution's
    // holding account* — half-open [enrolment_date, unenrolment_date),
    // open-ended when NULL — and the matching period decides what happens to
    // the leftover. No match means that account's holding wasn't enrolled
    // when the distribution went ex (never enrolled, before enrolment, in an
    // unenrolment gap — or only ever enrolled in a different account, e.g. a
    // personal account's enrolment never reinvests an employer-plan
    // distribution).
    let matched: Option<(ResidualHandling, NaiveDate, Option<NaiveDate>)> = sqlx::query_as(
        "SELECT residual_handling, enrolment_date, unenrolment_date FROM drp_enrolments \
         WHERE listing_id = ? AND holding_account_id = ? AND enrolment_date <= ? \
           AND (unenrolment_date IS NULL OR ? < unenrolment_date)",
    )
    .bind(listing_id)
    .bind(holding_account_id)
    .bind(entitlement_date)
    .bind(entitlement_date)
    .fetch_optional(&mut *tx)
    .await?;
    let (handling, period_start, period_end) = match matched {
        Some(p) => p,
        None => {
            // Name the account and listing in the rejection so the user knows
            // exactly what to enrol — never echo the raw foreign-key ids.
            let account: String =
                sqlx::query_scalar("SELECT name FROM holding_accounts WHERE id = ?")
                    .bind(holding_account_id)
                    .fetch_one(&mut *tx)
                    .await?;
            let ticker: String = sqlx::query_scalar("SELECT ticker FROM listings WHERE id = ?")
                .bind(listing_id)
                .fetch_one(&mut *tx)
                .await?;
            return Err(ReinvestError::NotEnrolled {
                account,
                ticker,
                date: entitlement_date,
            });
        }
    };

    // The DRP trade is denominated in the holding's currency.
    let currency: String = sqlx::query_scalar("SELECT currency FROM listings WHERE id = ?")
        .bind(listing_id)
        .fetch_one(&mut *tx)
        .await?;

    // Residual brought forward = the most recent prior DRP trade's
    // carried-forward, *within the same enrolment period and holding
    // account*: an earlier period's trailing residual was paid out at its
    // unenrolment, and another account runs its own chain, so the chain never
    // crosses a period boundary or an account boundary.
    let prior_cf: Option<String> = sqlx::query_scalar(
        "SELECT residual_carried_forward FROM trades \
         WHERE listing_id = ? AND holding_account_id = ? AND trade_type = 'DRP' AND date >= ? \
           AND (? IS NULL OR date < ?) \
         ORDER BY date DESC, id DESC LIMIT 1",
    )
    .bind(listing_id)
    .bind(holding_account_id)
    .bind(period_start)
    .bind(period_end)
    .bind(period_end)
    .fetch_optional(&mut *tx)
    .await?;
    let residual_bf = match prior_cf {
        Some(s) => parse_dec("residual_carried_forward", s)?,
        None => Decimal::ZERO,
    };

    let available = cash + residual_bf;
    let (quantity, carried, paid_out) = match body.units {
        // Broker-stated fractional allotment: the statement's figure is
        // authoritative, cross-checked to within one unit-step at its stated
        // precision (any broker rounding direction lands inside that). The
        // sub-step difference is statement rounding, not a residual.
        Some(units) => {
            let cost = units * body.reinvestment_price;
            let step = Decimal::new(1, units.scale());
            if (available - cost).abs() >= step * body.reinvestment_price {
                return Err(ReinvestError::UnitsCashMismatch { cost, available });
            }
            (units, Decimal::ZERO, Decimal::ZERO)
        }
        // Registry default: spend the available cash on whole shares; the
        // leftover is carried or paid out per the period's handling.
        None => {
            let quantity = (available / body.reinvestment_price).floor();
            let leftover = available - quantity * body.reinvestment_price;
            match handling {
                ResidualHandling::CarryForward => (quantity, leftover, Decimal::ZERO),
                ResidualHandling::PayOut => (quantity, Decimal::ZERO, leftover),
            }
        }
    };

    // DRP units are issued by the registry, not market-settled, so the
    // settlement date is the trade date.
    let date = body.date.unwrap_or(date_paid);
    let fx_rate = body.fx_rate.unwrap_or(Decimal::ONE);

    let result = sqlx::query(
        "INSERT INTO trades \
         (trade_type, date, settlement_date, listing_id, average_price, quantity, currency, \
          brokerage, gst_on_brokerage, brokerage_currency, fx_rate, contract_note_ref, \
          residual_brought_forward, residual_carried_forward, residual_paid_out, \
          holding_account_id) \
         VALUES ('DRP', ?, ?, ?, ?, ?, ?, '0', '0', ?, ?, NULL, ?, ?, ?, ?)",
    )
    .bind(date)
    .bind(date)
    .bind(listing_id)
    .bind(body.reinvestment_price.to_string())
    .bind(quantity.to_string())
    .bind(&currency)
    .bind(&currency)
    .bind(fx_rate.to_string())
    .bind(residual_bf.to_string())
    .bind(carried.to_string())
    .bind(paid_out.to_string())
    .bind(holding_account_id)
    .execute(&mut *tx)
    .await?;
    let new_id = result.last_insert_rowid();

    sqlx::query("UPDATE income SET reinvestment_trade_id = ? WHERE id = ?")
        .bind(new_id)
        .bind(income_id)
        .execute(&mut *tx)
        .await?;

    tx.commit().await?;

    // Read the freshly created trade back so the response is exactly what was stored.
    trade::db_get(pool, new_id)
        .await?
        .ok_or_else(|| ReinvestError::Db(sqlx::Error::RowNotFound))
}

/// Undo a reinvestment: delete the DRP trade and clear the distribution's
/// link, atomically. The inverse of [`db_reinvest`] — after it the
/// distribution can be reinvested again.
pub async fn db_unreinvest(pool: &SqlitePool, income_id: i64) -> Result<(), ReinvestError> {
    let mut tx = pool.begin().await?;

    let link: Option<Option<i64>> =
        sqlx::query_scalar("SELECT reinvestment_trade_id FROM income WHERE id = ?")
            .bind(income_id)
            .fetch_optional(&mut *tx)
            .await?;
    let trade_id = match link {
        None => return Err(ReinvestError::IncomeNotFound),
        Some(None) => return Err(ReinvestError::NotReinvested),
        Some(Some(id)) => id,
    };

    // The trade must not be drawn on: a Sell allocation or AMIT adjustment
    // referencing it would be orphaned by the delete (the same dependants
    // `trade::db_delete` guards).
    let consumed: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM parcel_allocations WHERE purchase_trade_id = ?1) \
             OR EXISTS(SELECT 1 FROM amit_adjustments WHERE trade_id = ?1)",
    )
    .bind(trade_id)
    .fetch_one(&mut *tx)
    .await?;
    if consumed {
        return Err(ReinvestError::ReinvestmentConsumed);
    }

    // Undo is last-in-first-out: a later DRP trade for the same listing and
    // account read its residual_brought_forward back from the chain this
    // trade is part of (see the module doc), so a mid-chain trade can't be
    // removed. Ordered by (date, id), matching db_reinvest's chain lookup.
    let (listing_id, holding_account_id, date): (i64, i64, NaiveDate) =
        sqlx::query_as("SELECT listing_id, holding_account_id, date FROM trades WHERE id = ?")
            .bind(trade_id)
            .fetch_one(&mut *tx)
            .await?;
    let has_later: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM trades \
                       WHERE listing_id = ? AND holding_account_id = ? AND trade_type = 'DRP' \
                         AND (date > ? OR (date = ? AND id > ?)))",
    )
    .bind(listing_id)
    .bind(holding_account_id)
    .bind(date)
    .bind(date)
    .bind(trade_id)
    .fetch_one(&mut *tx)
    .await?;
    if has_later {
        return Err(ReinvestError::ReinvestmentNotChainTail);
    }

    // Clear the link before deleting the trade so the FK never dangles.
    sqlx::query("UPDATE income SET reinvestment_trade_id = NULL WHERE id = ?")
        .bind(income_id)
        .execute(&mut *tx)
        .await?;
    sqlx::query("DELETE FROM trades WHERE id = ?")
        .bind(trade_id)
        .execute(&mut *tx)
        .await?;

    tx.commit().await?;
    Ok(())
}

async fn reinvest(
    State(pool): State<SqlitePool>,
    Path(income_id): Path<i64>,
    Json(body): Json<ReinvestBody>,
) -> Result<(StatusCode, Json<Trade>), ApiError> {
    let trade = db_reinvest(&pool, income_id, &body).await?;
    Ok((StatusCode::CREATED, Json(trade)))
}

async fn unreinvest(
    State(pool): State<SqlitePool>,
    Path(income_id): Path<i64>,
) -> Result<StatusCode, ApiError> {
    db_unreinvest(&pool, income_id).await?;
    Ok(StatusCode::NO_CONTENT)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::entities::{drp_enrolment, income, listing, trade::TradeType};
    use crate::test_support::{self, test_pool};
    use axum::{body::Body, http::Request};
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    async fn insert_listing(pool: &SqlitePool, id: i64, currency: &str) {
        test_support::listing(id)
            .security_type(listing::SecurityType::Trust)
            .currency(currency)
            .insert(pool)
            .await;
    }

    /// Create an enrolment period `[from, to)`; `to = None` = open-ended.
    async fn enrol_period(
        pool: &SqlitePool,
        id: i64,
        listing_id: i64,
        from: &str,
        to: Option<&str>,
        handling: ResidualHandling,
    ) {
        drp_enrolment::db_upsert(
            pool,
            &drp_enrolment::DrpEnrolment {
                holding_account_id: 1,
                id,
                listing_id,
                enrolment_date: from.parse().unwrap(),
                unenrolment_date: to.map(|d| d.parse().unwrap()),
                residual_handling: handling,
            },
        )
        .await
        .unwrap();
    }

    /// Enrol open-ended from 2024-01-01, covering the default distribution date.
    async fn enrol(pool: &SqlitePool, listing_id: i64, handling: ResidualHandling) {
        enrol_period(pool, listing_id, listing_id, "2024-01-01", None, handling).await;
    }

    /// Insert a distribution paying `cash` as unfranked cash (the simplest cash
    /// component), with `franking` notional franking credits that must be ignored.
    async fn insert_distribution(
        pool: &SqlitePool,
        id: i64,
        listing_id: i64,
        cash: Decimal,
        franking: Decimal,
    ) {
        insert_distribution_dated(pool, id, listing_id, "2024-03-31", None, cash, franking).await;
    }

    async fn insert_distribution_dated(
        pool: &SqlitePool,
        id: i64,
        listing_id: i64,
        date_paid: &str,
        ex_date: Option<&str>,
        cash: Decimal,
        franking: Decimal,
    ) {
        test_support::income(id, listing_id, date_paid.parse().unwrap())
            .with(|i| {
                i.ex_date = ex_date.map(|d| d.parse().unwrap());
                i.unfranked_amount = cash;
                i.franking_credits = franking;
                i.trust_income = true;
            })
            .insert(pool)
            .await;
    }

    fn body(price: &str) -> ReinvestBody {
        ReinvestBody {
            reinvestment_price: price.parse().unwrap(),
            units: None,
            fx_rate: None,
            date: None,
        }
    }

    /// Body with the broker's stated fractional allotment.
    fn body_units(price: &str, units: &str) -> ReinvestBody {
        ReinvestBody {
            units: Some(units.parse().unwrap()),
            ..body(price)
        }
    }

    /// A second holding account (e.g. an employer share plan) for the
    /// per-account tests.
    async fn insert_account(pool: &SqlitePool, id: i64, name: &str) {
        crate::entities::holding_account::db_upsert(
            pool,
            &crate::entities::holding_account::HoldingAccount {
                id,
                name: name.to_string(),
            },
        )
        .await
        .unwrap();
    }

    /// Open-ended enrolment from 2024-01-01 in a specific holding account.
    async fn enrol_in_account(
        pool: &SqlitePool,
        id: i64,
        listing_id: i64,
        account_id: i64,
        handling: ResidualHandling,
    ) {
        drp_enrolment::db_upsert(
            pool,
            &drp_enrolment::DrpEnrolment {
                id,
                listing_id,
                holding_account_id: account_id,
                enrolment_date: "2024-01-01".parse().unwrap(),
                unenrolment_date: None,
                residual_handling: handling,
            },
        )
        .await
        .unwrap();
    }

    /// Distribution of `cash` paid to a specific holding account.
    async fn insert_distribution_in_account(
        pool: &SqlitePool,
        id: i64,
        listing_id: i64,
        account_id: i64,
        cash: Decimal,
    ) {
        test_support::income(id, listing_id, "2024-03-31".parse().unwrap())
            .with(|i| {
                i.holding_account_id = account_id;
                i.unfranked_amount = cash;
                i.trust_income = true;
            })
            .insert(pool)
            .await;
    }

    #[tokio::test]
    async fn carry_forward_buys_whole_shares_and_carries_leftover() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "AUD").await;
        enrol(&pool, 1, ResidualHandling::CarryForward).await;
        // $100 cash + $30 notional franking credits (must be ignored), price $9.
        insert_distribution(&pool, 1, 1, Decimal::from(100), Decimal::from(30)).await;

        let trade = db_reinvest(&pool, 1, &body("9")).await.unwrap();

        // floor(100 / 9) = 11 shares, cost 99, leftover 1 carried forward.
        assert_eq!(trade.trade_type, TradeType::DRP);
        assert_eq!(trade.quantity, Decimal::from(11));
        assert_eq!(trade.average_price, Decimal::from(9));
        assert_eq!(trade.residual_brought_forward, Decimal::ZERO);
        assert_eq!(trade.residual_carried_forward, Decimal::ONE);
        assert_eq!(trade.residual_paid_out, Decimal::ZERO);

        // The distribution is now linked to the new trade.
        let inc = income::db_get(&pool, 1).await.unwrap().unwrap();
        assert_eq!(inc.reinvestment_trade_id, Some(trade.id));
    }

    /// Operation-created trades take no part in GST-inclusive entry or the
    /// statement cross-check: a reinvestment trade reads back with the flag
    /// off and no statement total (the columns' defaults).
    #[tokio::test]
    async fn reinvestment_trade_is_not_gst_flagged_and_has_no_statement_total() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "AUD").await;
        enrol(&pool, 1, ResidualHandling::CarryForward).await;
        insert_distribution(&pool, 1, 1, Decimal::from(100), Decimal::ZERO).await;

        let trade = db_reinvest(&pool, 1, &body("9")).await.unwrap();
        let stored = crate::entities::trade::db_get(&pool, trade.id)
            .await
            .unwrap()
            .unwrap();
        assert!(!stored.brokerage_includes_gst);
        assert_eq!(stored.statement_total, None);
    }

    #[tokio::test]
    async fn carried_residual_is_picked_up_by_the_next_reinvestment() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "AUD").await;
        enrol(&pool, 1, ResidualHandling::CarryForward).await;

        // First: $100 at $9 → 11 shares, $1 carried.
        insert_distribution(&pool, 1, 1, Decimal::from(100), Decimal::ZERO).await;
        let first = db_reinvest(&pool, 1, &body("9")).await.unwrap();
        assert_eq!(first.residual_carried_forward, Decimal::ONE);

        // Second: $8 cash + $1 brought forward = $9 available at $9 → exactly 1 share, $0 leftover.
        insert_distribution(&pool, 2, 1, Decimal::from(8), Decimal::ZERO).await;
        let second = db_reinvest(&pool, 2, &body("9")).await.unwrap();
        assert_eq!(second.residual_brought_forward, Decimal::ONE);
        assert_eq!(second.quantity, Decimal::from(1));
        assert_eq!(second.residual_carried_forward, Decimal::ZERO);
    }

    #[tokio::test]
    async fn pay_out_records_leftover_as_paid_not_carried() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "AUD").await;
        enrol(&pool, 1, ResidualHandling::PayOut).await;
        insert_distribution(&pool, 1, 1, Decimal::from(100), Decimal::ZERO).await;

        let trade = db_reinvest(&pool, 1, &body("9")).await.unwrap();
        // 11 shares, $1 leftover paid out (not carried).
        assert_eq!(trade.quantity, Decimal::from(11));
        assert_eq!(trade.residual_paid_out, Decimal::ONE);
        assert_eq!(trade.residual_carried_forward, Decimal::ZERO);

        // A pay-out leaves no carried balance for the next reinvestment.
        insert_distribution(&pool, 2, 1, Decimal::from(8), Decimal::ZERO).await;
        let next = db_reinvest(&pool, 2, &body("9")).await.unwrap();
        assert_eq!(next.residual_brought_forward, Decimal::ZERO);
        assert_eq!(next.quantity, Decimal::ZERO); // 8 < 9, no whole share
    }

    #[tokio::test]
    async fn franking_credits_are_excluded_from_reinvestable_cash() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "AUD").await;
        enrol(&pool, 1, ResidualHandling::CarryForward).await;
        // $9 cash but $90 franking credits — only the $9 cash reinvests.
        insert_distribution(&pool, 1, 1, Decimal::from(9), Decimal::from(90)).await;

        let trade = db_reinvest(&pool, 1, &body("9")).await.unwrap();
        assert_eq!(trade.quantity, Decimal::from(1));
        assert_eq!(trade.residual_carried_forward, Decimal::ZERO);
    }

    /// Broker-stated fractional allotment: the statement's units are taken
    /// exactly — including trailing zeros, so the stored quantity reads back
    /// as stated — and the residual columns record zero.
    #[tokio::test]
    async fn explicit_units_take_the_statements_fractional_allotment() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "USD").await;
        enrol(&pool, 1, ResidualHandling::CarryForward).await;
        // $68.47 cash at $136.94 → the statement says 0.500 shares.
        insert_distribution(&pool, 1, 1, "68.47".parse().unwrap(), Decimal::ZERO).await;

        let trade = db_reinvest(&pool, 1, &body_units("136.94", "0.500"))
            .await
            .unwrap();
        assert_eq!(trade.trade_type, TradeType::DRP);
        assert_eq!(trade.quantity, "0.500".parse::<Decimal>().unwrap());
        assert_eq!(trade.average_price, "136.94".parse::<Decimal>().unwrap());
        assert_eq!(trade.residual_brought_forward, Decimal::ZERO);
        assert_eq!(trade.residual_carried_forward, Decimal::ZERO);
        assert_eq!(trade.residual_paid_out, Decimal::ZERO);

        // The stated figure is stored exactly as stated (scale preserved).
        let stored: String = sqlx::query_scalar("SELECT quantity FROM trades WHERE id = ?")
            .bind(trade.id)
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(stored, "0.500");

        let inc = income::db_get(&pool, 1).await.unwrap().unwrap();
        assert_eq!(inc.reinvestment_trade_id, Some(trade.id));
    }

    /// The cross-check tolerates the statement's own rounding: a real broker
    /// price (not the derived cash ÷ units) leaves `units × price` within one
    /// unit-step of the cash, and that passes.
    #[tokio::test]
    async fn explicit_units_tolerate_sub_step_statement_rounding() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "USD").await;
        enrol(&pool, 1, ResidualHandling::CarryForward).await;
        // 0.501 × 137.05 = 68.66205 vs $68.66 cash — off by $0.00205,
        // well inside one 0.001 unit-step (0.001 × 137.05 = $0.13705).
        insert_distribution(&pool, 1, 1, "68.66".parse().unwrap(), Decimal::ZERO).await;

        let trade = db_reinvest(&pool, 1, &body_units("137.05", "0.501"))
            .await
            .unwrap();
        assert_eq!(trade.quantity, "0.501".parse::<Decimal>().unwrap());
        assert_eq!(trade.residual_carried_forward, Decimal::ZERO);
        assert_eq!(trade.residual_paid_out, Decimal::ZERO);
    }

    /// Units that don't spend the cash are rejected and nothing persists: a
    /// full unit-step (at the stated precision) or more off is a data error,
    /// not rounding.
    #[tokio::test]
    async fn explicit_units_cash_mismatch_is_rejected() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "USD").await;
        enrol(&pool, 1, ResidualHandling::CarryForward).await;
        insert_distribution(&pool, 1, 1, "68.66".parse().unwrap(), Decimal::ZERO).await;

        // 0.600 × 137.05 = 82.23 — $13.57 off the $68.66 cash.
        let err = db_reinvest(&pool, 1, &body_units("137.05", "0.600"))
            .await
            .unwrap_err();
        assert!(matches!(err, ReinvestError::UnitsCashMismatch { .. }));
        let n: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM trades")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(n, 0);

        // The boundary is exclusive: exactly one unit-step off still rejects
        // (a broker-computed figure is always strictly inside the step)...
        insert_distribution(&pool, 2, 1, Decimal::from(60), Decimal::ZERO).await;
        let err = db_reinvest(&pool, 2, &body_units("100", "0.5")) // step 0.1 → tolerance $10
            .await
            .unwrap_err();
        assert!(matches!(err, ReinvestError::UnitsCashMismatch { .. }));
        // ...while just inside it passes (coarser stated precision, looser check).
        insert_distribution(&pool, 3, 1, "59.99".parse().unwrap(), Decimal::ZERO).await;
        let trade = db_reinvest(&pool, 3, &body_units("100", "0.5"))
            .await
            .unwrap();
        assert_eq!(trade.quantity, "0.5".parse::<Decimal>().unwrap());
    }

    #[tokio::test]
    async fn non_positive_units_are_rejected() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "USD").await;
        enrol(&pool, 1, ResidualHandling::CarryForward).await;
        insert_distribution(&pool, 1, 1, Decimal::from(100), Decimal::ZERO).await;
        for units in ["0", "-0.5"] {
            let err = db_reinvest(&pool, 1, &body_units("9", units))
                .await
                .unwrap_err();
            assert!(
                matches!(err, ReinvestError::NonPositiveUnits),
                "units {units}"
            );
        }
    }

    /// Explicit units go through the same enrolment gate as the default path.
    #[tokio::test]
    async fn explicit_units_still_require_enrolment() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "USD").await;
        insert_distribution(&pool, 1, 1, "68.47".parse().unwrap(), Decimal::ZERO).await;
        let err = db_reinvest(&pool, 1, &body_units("136.94", "0.500"))
            .await
            .unwrap_err();
        assert!(matches!(err, ReinvestError::NotEnrolled { .. }));
    }

    /// A residual carried forward by an earlier whole-share reinvestment in
    /// the period is part of the available cash an explicit-units allotment
    /// spends: it's recorded as brought forward, and nothing is carried on —
    /// the broker spent the lot.
    #[tokio::test]
    async fn explicit_units_spend_the_brought_forward_residual() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "AUD").await;
        enrol(&pool, 1, ResidualHandling::CarryForward).await;

        // Whole-share first: $100 at $9 → 11 shares, $1 carried.
        insert_distribution(&pool, 1, 1, Decimal::from(100), Decimal::ZERO).await;
        let first = db_reinvest(&pool, 1, &body("9")).await.unwrap();
        assert_eq!(first.residual_carried_forward, Decimal::ONE);

        // Fractional next: $8 cash + $1 brought forward = $9 = 0.5 × $18.
        insert_distribution(&pool, 2, 1, Decimal::from(8), Decimal::ZERO).await;
        let second = db_reinvest(&pool, 2, &body_units("18", "0.5"))
            .await
            .unwrap();
        assert_eq!(second.quantity, "0.5".parse::<Decimal>().unwrap());
        assert_eq!(second.residual_brought_forward, Decimal::ONE);
        assert_eq!(second.residual_carried_forward, Decimal::ZERO);
        assert_eq!(second.residual_paid_out, Decimal::ZERO);
    }

    /// Live-data check (REQUIREMENTS 2026-06-12): the nine Morgan Stanley ICE
    /// dividend reinvestments from the statement archive — entered as plain
    /// Buys priced net-cash ÷ units while reinvest was whole-share-only — go
    /// through the reinvest operation with the statements' exact fractional
    /// units. Figures are the live rows: foreign source income, US
    /// withholding, the stated units, and the derived per-share price.
    #[tokio::test]
    async fn morgan_stanley_ice_fractional_statements_reproduce() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "USD").await;
        enrol(&pool, 1, ResidualHandling::CarryForward).await;

        // (pay date, gross, US tax withheld, stated units, price)
        let statements = [
            ("2024-04-01", "80.55", "12.08", "0.500", "136.94000000"),
            ("2024-06-28", "80.78", "12.12", "0.501", "137.04590818"),
            ("2024-09-30", "81.00", "12.15", "0.434", "158.64055300"),
            ("2024-12-31", "81.20", "12.18", "0.465", "148.43010753"),
            ("2025-03-31", "111.31", "16.70", "0.539", "175.52875696"),
            ("2025-06-30", "111.57", "16.74", "0.522", "181.66666667"),
            ("2025-09-30", "111.82", "16.77", "0.565", "168.23008850"),
            ("2025-12-31", "112.09", "16.81", "0.582", "163.62542955"),
            ("2026-03-31", "148.78", "22.32", "0.811", "155.89395808"),
        ];
        for (i, (date, gross, withheld, units, price)) in statements.iter().enumerate() {
            let id = i as i64 + 1;
            test_support::income(id, 1, date.parse().unwrap())
                .with(|inc| {
                    inc.foreign_source_income = gross.parse().unwrap();
                    inc.foreign_tax_paid = withheld.parse().unwrap();
                })
                .insert(&pool)
                .await;
            let trade = db_reinvest(&pool, id, &body_units(price, units))
                .await
                .unwrap_or_else(|e| panic!("statement {date}: {e:?}"));
            assert_eq!(trade.trade_type, TradeType::DRP, "statement {date}");
            let stored: String = sqlx::query_scalar("SELECT quantity FROM trades WHERE id = ?")
                .bind(trade.id)
                .fetch_one(&pool)
                .await
                .unwrap();
            assert_eq!(&stored, units, "statement {date}: exact stated units");
            assert_eq!(trade.residual_carried_forward, Decimal::ZERO);
            assert_eq!(trade.residual_paid_out, Decimal::ZERO);
        }
    }

    #[tokio::test]
    async fn not_enrolled_is_rejected_and_nothing_persisted() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "AUD").await;
        insert_distribution(&pool, 1, 1, Decimal::from(100), Decimal::ZERO).await;

        let err = db_reinvest(&pool, 1, &body("9")).await.unwrap_err();
        assert!(matches!(err, ReinvestError::NotEnrolled { .. }));
        // No trade created, distribution unlinked.
        let n: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM trades")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(n, 0);
        assert!(
            income::db_get(&pool, 1)
                .await
                .unwrap()
                .unwrap()
                .reinvestment_trade_id
                .is_none()
        );
    }

    #[tokio::test]
    async fn already_reinvested_is_rejected() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "AUD").await;
        enrol(&pool, 1, ResidualHandling::CarryForward).await;
        insert_distribution(&pool, 1, 1, Decimal::from(100), Decimal::ZERO).await;

        db_reinvest(&pool, 1, &body("9")).await.unwrap();
        let err = db_reinvest(&pool, 1, &body("9")).await.unwrap_err();
        assert!(matches!(err, ReinvestError::AlreadyReinvested));
        // Still exactly one DRP trade.
        let n: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM trades")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(n, 1);
    }

    #[tokio::test]
    async fn missing_income_is_not_found() {
        let pool = test_pool().await;
        let err = db_reinvest(&pool, 99, &body("9")).await.unwrap_err();
        assert!(matches!(err, ReinvestError::IncomeNotFound));
    }

    #[tokio::test]
    async fn non_positive_price_is_rejected() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "AUD").await;
        enrol(&pool, 1, ResidualHandling::CarryForward).await;
        insert_distribution(&pool, 1, 1, Decimal::from(100), Decimal::ZERO).await;
        let err = db_reinvest(&pool, 1, &body("0")).await.unwrap_err();
        assert!(matches!(err, ReinvestError::NonPositivePrice));
    }

    #[tokio::test]
    async fn distribution_before_enrolment_is_rejected() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "AUD").await;
        // Enrolled only from June 2024; the distribution went ex in March.
        enrol_period(
            &pool,
            1,
            1,
            "2024-06-01",
            None,
            ResidualHandling::CarryForward,
        )
        .await;
        insert_distribution(&pool, 1, 1, Decimal::from(100), Decimal::ZERO).await; // 2024-03-31
        let err = db_reinvest(&pool, 1, &body("9")).await.unwrap_err();
        assert!(matches!(err, ReinvestError::NotEnrolled { .. }));
    }

    #[tokio::test]
    async fn distribution_in_unenrolment_gap_is_rejected() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "AUD").await;
        // Enrolled through 2023, re-enrolled from 2025 — 2024 is a gap.
        enrol_period(
            &pool,
            1,
            1,
            "2023-01-01",
            Some("2024-01-01"),
            ResidualHandling::CarryForward,
        )
        .await;
        enrol_period(
            &pool,
            2,
            1,
            "2025-01-01",
            None,
            ResidualHandling::CarryForward,
        )
        .await;
        insert_distribution(&pool, 1, 1, Decimal::from(100), Decimal::ZERO).await; // 2024-03-31
        let err = db_reinvest(&pool, 1, &body("9")).await.unwrap_err();
        assert!(matches!(err, ReinvestError::NotEnrolled { .. }));
    }

    #[tokio::test]
    async fn re_enrolment_after_unenrolment_uses_the_new_periods_handling() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "AUD").await;
        enrol_period(
            &pool,
            1,
            1,
            "2023-01-01",
            Some("2024-01-01"),
            ResidualHandling::CarryForward,
        )
        .await;
        enrol_period(&pool, 2, 1, "2025-01-01", None, ResidualHandling::PayOut).await;
        // A distribution inside the re-enrolment period reinvests, and its
        // leftover follows the *new* period's PayOut, not the old CarryForward.
        insert_distribution_dated(
            &pool,
            1,
            1,
            "2025-03-31",
            None,
            Decimal::from(100),
            Decimal::ZERO,
        )
        .await;
        let trade = db_reinvest(&pool, 1, &body("9")).await.unwrap();
        assert_eq!(trade.quantity, Decimal::from(11));
        assert_eq!(trade.residual_paid_out, Decimal::ONE);
        assert_eq!(trade.residual_carried_forward, Decimal::ZERO);
    }

    #[tokio::test]
    async fn reinvestability_is_decided_by_ex_date_not_pay_date() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "AUD").await;
        enrol_period(
            &pool,
            1,
            1,
            "2024-01-01",
            Some("2024-04-01"),
            ResidualHandling::CarryForward,
        )
        .await;

        // Ex inside the period, paid after the unenrolment took effect → the
        // participation was fixed at the ex date, so it still reinvests.
        insert_distribution_dated(
            &pool,
            1,
            1,
            "2024-04-10",
            Some("2024-03-15"),
            Decimal::from(100),
            Decimal::ZERO,
        )
        .await;
        let trade = db_reinvest(&pool, 1, &body("9")).await.unwrap();
        assert_eq!(trade.quantity, Decimal::from(11));

        // Ex before the period, paid inside it → was not enrolled at ex → rejected.
        insert_distribution_dated(
            &pool,
            2,
            1,
            "2024-02-01",
            Some("2023-12-15"),
            Decimal::from(100),
            Decimal::ZERO,
        )
        .await;
        let err = db_reinvest(&pool, 2, &body("9")).await.unwrap_err();
        assert!(matches!(err, ReinvestError::NotEnrolled { .. }));
    }

    #[tokio::test]
    async fn carried_residual_does_not_cross_an_unenrolment() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "AUD").await;
        enrol(&pool, 1, ResidualHandling::CarryForward).await; // open from 2024-01-01

        // First reinvestment carries $1 forward.
        insert_distribution(&pool, 1, 1, Decimal::from(100), Decimal::ZERO).await;
        let first = db_reinvest(&pool, 1, &body("9")).await.unwrap();
        assert_eq!(first.residual_carried_forward, Decimal::ONE);

        // Unenrol (close the period): the trailing $1 is paid out...
        enrol_period(
            &pool,
            1,
            1,
            "2024-01-01",
            Some("2024-06-01"),
            ResidualHandling::CarryForward,
        )
        .await;
        let settled = trade::db_get(&pool, first.id).await.unwrap().unwrap();
        assert_eq!(settled.residual_carried_forward, Decimal::ZERO);
        assert_eq!(settled.residual_paid_out, Decimal::ONE);

        // ...so a reinvestment in the re-enrolment period brings nothing forward.
        enrol_period(
            &pool,
            2,
            1,
            "2025-01-01",
            None,
            ResidualHandling::CarryForward,
        )
        .await;
        insert_distribution_dated(
            &pool,
            2,
            1,
            "2025-03-31",
            None,
            Decimal::from(8),
            Decimal::ZERO,
        )
        .await;
        let next = db_reinvest(&pool, 2, &body("9")).await.unwrap();
        assert_eq!(next.residual_brought_forward, Decimal::ZERO);
        assert_eq!(next.quantity, Decimal::ZERO); // 8 < 9, no whole share
    }

    /// The RSU scenario (REQUIREMENTS "Holding accounts"): the same listing
    /// held in two accounts at once, with the personal account DRP-enrolled
    /// and the employer-plan account not. A distribution paid to the enrolled
    /// account reinvests — and the DRP trade lands in that account — while
    /// the plan account's identical distribution is rejected: enrolment is
    /// per (listing, holding account), not per listing.
    #[tokio::test]
    async fn enrolment_is_per_holding_account() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "AUD").await;
        insert_account(&pool, 2, "ICE Employee Plan").await;
        // Only the default (personal) account is enrolled.
        enrol_in_account(&pool, 1, 1, 1, ResidualHandling::CarryForward).await;

        // Personal-account distribution reinvests, into the personal account.
        insert_distribution_in_account(&pool, 1, 1, 1, Decimal::from(100)).await;
        let trade = db_reinvest(&pool, 1, &body("9")).await.unwrap();
        assert_eq!(trade.quantity, Decimal::from(11));
        assert_eq!(trade.holding_account_id, 1);

        // The plan account's distribution on the same listing is rejected.
        insert_distribution_in_account(&pool, 2, 1, 2, Decimal::from(100)).await;
        let err = db_reinvest(&pool, 2, &body("9")).await.unwrap_err();
        assert!(matches!(err, ReinvestError::NotEnrolled { .. }));
    }

    /// The DRP trade is created in the distribution's holding account, not
    /// the default one.
    #[tokio::test]
    async fn drp_trade_lands_in_the_distributions_account() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "AUD").await;
        insert_account(&pool, 2, "Personal CHESS").await;
        enrol_in_account(&pool, 1, 1, 2, ResidualHandling::CarryForward).await;
        insert_distribution_in_account(&pool, 1, 1, 2, Decimal::from(100)).await;

        let trade = db_reinvest(&pool, 1, &body("9")).await.unwrap();
        assert_eq!(trade.holding_account_id, 2);
    }

    /// Each (listing, holding account) runs its own residual chain: a
    /// carried-forward leftover in one account is never brought forward by a
    /// reinvestment in another.
    #[tokio::test]
    async fn carried_residual_does_not_cross_accounts() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "AUD").await;
        insert_account(&pool, 2, "Personal CHESS").await;
        enrol_in_account(&pool, 1, 1, 1, ResidualHandling::CarryForward).await;
        enrol_in_account(&pool, 2, 1, 2, ResidualHandling::CarryForward).await;

        // Account 1: $100 at $9 → 11 shares, $1 carried forward.
        insert_distribution_in_account(&pool, 1, 1, 1, Decimal::from(100)).await;
        let first = db_reinvest(&pool, 1, &body("9")).await.unwrap();
        assert_eq!(first.residual_carried_forward, Decimal::ONE);

        // Account 2's next reinvestment brings nothing forward from account 1.
        insert_distribution_in_account(&pool, 2, 1, 2, Decimal::from(8)).await;
        let other = db_reinvest(&pool, 2, &body("9")).await.unwrap();
        assert_eq!(other.residual_brought_forward, Decimal::ZERO);
        assert_eq!(other.quantity, Decimal::ZERO); // 8 < 9, no whole share

        // Account 1's own chain still picks its $1 up.
        insert_distribution_in_account(&pool, 3, 1, 1, Decimal::from(8)).await;
        let next = db_reinvest(&pool, 3, &body("9")).await.unwrap();
        assert_eq!(next.residual_brought_forward, Decimal::ONE);
        assert_eq!(next.quantity, Decimal::from(1));
    }

    #[tokio::test]
    async fn api_reinvest_returns_201_with_trade() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "AUD").await;
        enrol(&pool, 1, ResidualHandling::CarryForward).await;
        insert_distribution(&pool, 1, 1, Decimal::from(100), Decimal::ZERO).await;

        let resp = router()
            .with_state(pool.clone())
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/income/1/reinvest")
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"reinvestment_price":"9"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::CREATED);
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let trade: Trade = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(trade.trade_type, TradeType::DRP);
        assert_eq!(trade.quantity, Decimal::from(11));
    }

    #[tokio::test]
    async fn api_reinvest_with_units_returns_201_with_fractional_trade() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "USD").await;
        enrol(&pool, 1, ResidualHandling::CarryForward).await;
        insert_distribution(&pool, 1, 1, "68.47".parse().unwrap(), Decimal::ZERO).await;

        let resp = router()
            .with_state(pool.clone())
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/income/1/reinvest")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        r#"{"reinvestment_price":"136.94","units":"0.500"}"#,
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::CREATED);
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let trade: Trade = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(trade.trade_type, TradeType::DRP);
        assert_eq!(trade.quantity, "0.500".parse::<Decimal>().unwrap());
    }

    /// The units/cash mismatch rejection carries both figures so the user can
    /// see what the entry computes to.
    #[tokio::test]
    async fn api_reinvest_units_mismatch_returns_422_with_figures() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "USD").await;
        enrol(&pool, 1, ResidualHandling::CarryForward).await;
        insert_distribution(&pool, 1, 1, "68.66".parse().unwrap(), Decimal::ZERO).await;
        let resp = router()
            .with_state(pool)
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/income/1/reinvest")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        r#"{"reinvestment_price":"137.05","units":"0.600"}"#,
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY);
        let body = resp.into_body().collect().await.unwrap().to_bytes();
        let text = String::from_utf8(body.to_vec()).unwrap();
        assert!(text.contains("82.23000"), "body: {text}"); // 0.600 × 137.05
        assert!(text.contains("68.66"), "body: {text}");
    }

    #[tokio::test]
    async fn api_reinvest_not_enrolled_returns_422() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "AUD").await;
        insert_distribution(&pool, 1, 1, Decimal::from(100), Decimal::ZERO).await;
        let resp = router()
            .with_state(pool)
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/income/1/reinvest")
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"reinvestment_price":"9"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY);
        let body = resp.into_body().collect().await.unwrap().to_bytes();
        let text = String::from_utf8(body.to_vec()).unwrap();
        // The rejection names the account and ticker, not raw ids.
        assert!(text.contains("Default"), "body: {text}");
        assert!(text.contains("T1"), "body: {text}");
        assert!(text.contains("not enrolled"), "body: {text}");
    }

    #[tokio::test]
    async fn api_reinvest_missing_income_returns_404() {
        let pool = test_pool().await;
        let resp = router()
            .with_state(pool)
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/income/99/reinvest")
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"reinvestment_price":"9"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    // ---- unreinvest (DELETE /income/:id/reinvest) ----

    #[tokio::test]
    async fn unreinvest_deletes_the_trade_clears_the_link_and_allows_redo() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "AUD").await;
        enrol(&pool, 1, ResidualHandling::CarryForward).await;
        insert_distribution(&pool, 1, 1, Decimal::from(100), Decimal::ZERO).await;
        let trade = db_reinvest(&pool, 1, &body("9")).await.unwrap();

        db_unreinvest(&pool, 1).await.unwrap();

        assert!(
            crate::entities::trade::db_get(&pool, trade.id)
                .await
                .unwrap()
                .is_none(),
            "DRP trade must be deleted"
        );
        let inc = income::db_get(&pool, 1).await.unwrap().unwrap();
        assert_eq!(inc.reinvestment_trade_id, None, "link must be cleared");

        // The undo is a true inverse: the distribution reinvests again, at a
        // corrected price this time.
        let redo = db_reinvest(&pool, 1, &body("10")).await.unwrap();
        assert_eq!(redo.quantity, Decimal::from(10));
        assert_eq!(
            income::db_get(&pool, 1)
                .await
                .unwrap()
                .unwrap()
                .reinvestment_trade_id,
            Some(redo.id)
        );
    }

    #[tokio::test]
    async fn unreinvest_without_a_reinvestment_is_rejected() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "AUD").await;
        insert_distribution(&pool, 1, 1, Decimal::from(100), Decimal::ZERO).await;
        let err = db_unreinvest(&pool, 1).await.unwrap_err();
        assert!(
            matches!(err, ReinvestError::NotReinvested),
            "expected NotReinvested, got: {err:?}"
        );
    }

    #[tokio::test]
    async fn unreinvest_missing_income_is_not_found() {
        let pool = test_pool().await;
        let err = db_unreinvest(&pool, 99).await.unwrap_err();
        assert!(
            matches!(err, ReinvestError::IncomeNotFound),
            "expected IncomeNotFound, got: {err:?}"
        );
    }

    #[tokio::test]
    async fn unreinvest_is_refused_while_the_trade_is_drawn_on() {
        // A Sell allocation consuming the DRP parcel would be orphaned by the
        // undo — refused, and nothing changes.
        let pool = test_pool().await;
        insert_listing(&pool, 1, "AUD").await;
        enrol(&pool, 1, ResidualHandling::CarryForward).await;
        insert_distribution(&pool, 1, 1, Decimal::from(100), Decimal::ZERO).await;
        let trade = db_reinvest(&pool, 1, &body("9")).await.unwrap();
        crate::entities::sell::db_upsert_sell(
            &pool,
            50,
            &crate::entities::sell::SellBody {
                date: "2024-06-03".parse().unwrap(),
                settlement_date: Some("2024-06-05".parse().unwrap()),
                listing_id: 1,
                average_price: Decimal::from(12),
                quantity: Decimal::from(5),
                currency: "AUD".to_string(),
                brokerage: Decimal::ZERO,
                gst_on_brokerage: Decimal::ZERO,
                brokerage_includes_gst: false,
                brokerage_currency: "AUD".to_string(),
                fx_rate: Decimal::ONE,
                spot_fx_rate: None,
                contract_note_ref: None,
                statement_total: None,
                holding_account_id: 1,
                allocations: vec![crate::entities::sell::AllocationInput {
                    purchase_trade_id: trade.id,
                    quantity_allocated: Decimal::from(5),
                }],
            },
        )
        .await
        .unwrap();

        let err = db_unreinvest(&pool, 1).await.unwrap_err();
        assert!(
            matches!(err, ReinvestError::ReinvestmentConsumed),
            "expected ReinvestmentConsumed, got: {err:?}"
        );
        assert!(
            crate::entities::trade::db_get(&pool, trade.id)
                .await
                .unwrap()
                .is_some(),
            "trade must remain"
        );
        assert_eq!(
            income::db_get(&pool, 1)
                .await
                .unwrap()
                .unwrap()
                .reinvestment_trade_id,
            Some(trade.id),
            "link must remain"
        );
    }

    #[tokio::test]
    async fn unreinvest_is_lifo_a_mid_chain_trade_is_refused() {
        // Reinvest twice: the second trade brought the first's carried
        // residual forward, so the first can only be undone after the second.
        let pool = test_pool().await;
        insert_listing(&pool, 1, "AUD").await;
        enrol(&pool, 1, ResidualHandling::CarryForward).await;
        insert_distribution_dated(
            &pool,
            1,
            1,
            "2024-03-31",
            None,
            Decimal::from(100),
            Decimal::ZERO,
        )
        .await;
        insert_distribution_dated(
            &pool,
            2,
            1,
            "2024-06-30",
            None,
            Decimal::from(100),
            Decimal::ZERO,
        )
        .await;
        db_reinvest(&pool, 1, &body("9")).await.unwrap();
        let second = db_reinvest(&pool, 2, &body("9")).await.unwrap();
        // The chain is real: the second reinvestment picked up the first's $1.
        assert_eq!(second.residual_brought_forward, Decimal::ONE);

        let err = db_unreinvest(&pool, 1).await.unwrap_err();
        assert!(
            matches!(err, ReinvestError::ReinvestmentNotChainTail),
            "expected ReinvestmentNotChainTail, got: {err:?}"
        );

        // Undoing in LIFO order works: the tail first, then the first one.
        db_unreinvest(&pool, 2).await.unwrap();
        db_unreinvest(&pool, 1).await.unwrap();
        let n: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM trades")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(n, 0, "both DRP trades undone");
    }

    #[tokio::test]
    async fn api_unreinvest_round_trip_and_rejections() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "AUD").await;
        enrol(&pool, 1, ResidualHandling::CarryForward).await;
        insert_distribution(&pool, 1, 1, Decimal::from(100), Decimal::ZERO).await;
        db_reinvest(&pool, 1, &body("9")).await.unwrap();

        let del = |uri: &str| {
            Request::builder()
                .method("DELETE")
                .uri(uri)
                .body(Body::empty())
                .unwrap()
        };
        let app = router().with_state(pool.clone());
        let resp = app
            .clone()
            .oneshot(del("/income/1/reinvest"))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NO_CONTENT);

        // Nothing left to undo → 422; unknown income → 404.
        let resp = app
            .clone()
            .oneshot(del("/income/1/reinvest"))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY);
        let resp = app.oneshot(del("/income/99/reinvest")).await.unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }
}
