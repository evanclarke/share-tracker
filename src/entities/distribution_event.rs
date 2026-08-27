//! The distribution calendar: what each held listing's register says it paid,
//! and when — the external half of "was a distribution ever entered?".
//!
//! # What it is for, and what it is deliberately not
//!
//! Nothing in the system knew when a holding *should* have paid a
//! distribution. A dividend or trust distribution never entered — or entered
//! with a fat-fingered amount — is invisible: it misstates the year's income
//! and franking credits, and the AMIT cash cross-check can only compare
//! against rows that exist. This module collects the provider's own dividend
//! calendar per held listing, and `reports::health` reads it into two advisory
//! alerts (a known ex-date with units held and no income row; a matched income
//! row whose gross differs from per unit × units held).
//!
//! **Advisory, and never a tax gate.** No reported figure is computed from
//! this table. `reports::tax_report`'s `amma_missing` gate stays on recorded
//! facts alone and the advisory `amma_nothing_recorded` list is never resolved
//! from here — both stated in REQUIREMENTS' "Deliberately out of scope", the
//! reason being that a provider's coverage gap must never be able to retire a
//! real question. Nor is the feed a source of *amounts* for an income row: a
//! distribution's franking, foreign-source and cost-base components come from
//! the registry statement and nowhere else. The calendar answers "was there
//! one" and "does the total look right", never "what was in it".
//!
//! # Provider coverage, settled before this was built
//!
//! The whole feature rests on being able to read "the provider knows of no
//! ex-date" as "there was no distribution", so that was checked against an
//! issuer before any of it was written (REQUIREMENTS, "Coverage settled").
//! Yahoo returns 8 HNDQ events since the fund's 2020 inception where a
//! semi-annual payer would have 12; Betashares' own distribution table lists
//! 12 periods and prints a bare `-` in its amount column for exactly the four
//! Yahoo lacks. The fund distributed nothing on them, and the other eight
//! match Yahoo to 6 dp. Two limits are stated rather than assumed away: it is
//! one security's history, and the alerts are advisory precisely so that a
//! coverage hole in some other security degrades them rather than breaking
//! anything.
//!
//! # The stored date is the ex-date, and it is *not* the provider's own
//!
//! `yfinance-rs` collapses a corporate action's timestamp to a **UTC**
//! calendar date (`core::conversions::i64_to_date`), discarding the
//! `exchangeTimezoneName` the same response carries. Yahoo stamps the event at
//! the exchange's session start, so for an ASX security `Action::Dividend`'s
//! own date is the ex-date only in AEST; in AEDT (UTC+11, October–April) it is
//! **one day early**, where it then lands on a day the market was shut
//! (2025-01-01, 2024-01-01, a Sunday). Measured across HNDQ, BHP and VDHG
//! against issuer-published dates; the two offsets bracket the stamp hour at
//! ASX open.
//!
//! The crate exposes no route to the raw instant, so [`yahoo`] recovers the
//! date by **joining the event to the candle sharing its UTC date**:
//! `fetch_full()` returns the candles and the actions from one response,
//! `Candle::ts` keeps the instant the action lost, and both are stamped at
//! session start — so the event whose UTC date is `D` belongs to the candle
//! whose UTC `ts` date is `D`, and that candle's exchange-local date is the
//! ex-date. It assumes nothing about the stamp hour, and it is the same
//! candle-timestamp convention `closing_price::yahoo`'s `daily_closes` already
//! reads its trading days with. Verified 10 of 10 against issuer-published
//! dates.
//!
//! # Conventions
//!
//! - One row per (listing, ex-date). `amount_per_unit` is in the listing's
//!   **quote currency**, never AUD-converted — the same rule as a stored
//!   closing price, and for the same reason: the income row it is compared
//!   against records its gross in that currency too.
//! - `amount_per_unit` is in the **unit basis of its own `fetched_at`**, which
//!   is the provider's convention rather than a choice made here: Yahoo
//!   restates a whole dividend history into the current basis every time the
//!   security splits. It is stored as served and dated, never converted — see
//!   [`DistributionEvent::amount_per_unit`].
//! - The provider's currency is cross-checked against the listing's; a
//!   mismatch fails the listing's refresh rather than storing a silently
//!   mis-scaled expectation.
//! - A refresh **never deletes**. An event the provider stops serving stays
//!   stored: the alerts are about a distribution the books may have missed,
//!   and a provider that drops history must not be able to quietly retire one.
//!   An event whose amount the provider revises is updated in place, and the
//!   audit trail records what it said before (migration 0048).
//! - The fetch always passes an **explicit period**. `Range::Max` silently
//!   truncates the action stream — `VDHG.AX` returned 8 events over
//!   `Range::Max` against 28 for the same span requested as
//!   `between(start, end)` — so a max-range fetch would quietly lose most of
//!   the history this feature exists to check against.
//! - Provider-owned: the import job is the only writer. There is no `PUT` and
//!   no `DELETE` — a wrong row is corrected by fixing the listing's symbol
//!   mapping and re-running the job, not by hand-editing the provider's
//!   answer.

use crate::entities::closing_price::{self, FetchError, Market};
use crate::infra::decimal::Money;
use crate::infra::http::{self, ApiError, CrudEntity};
use axum::{Router, routing::get};
use chrono::{DateTime, NaiveDate, Utc};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;
use std::{future::Future, pin::Pin, sync::Arc};

mod yahoo;
pub use yahoo::YahooDistributionFetcher;

/// Reusable offline fetcher stub — see the module.
#[cfg(test)]
pub mod test_support;

/// One known distribution: what the provider says a listing paid per unit, and
/// the ex-date it paid it on.
#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct DistributionEvent {
    /// Server-assigned surrogate key: the row's identity for the audit trail
    /// (`row_history.row_id`). Writes address a row by its
    /// `(listing_id, ex_date)` natural key, never by this.
    pub id: i64,
    pub listing_id: i64,
    /// The ex-dividend date in the exchange's own timezone — the first day the
    /// security traded without entitlement to this distribution. See the
    /// module docs: this is the candle-joined date, not the provider's own.
    pub ex_date: NaiveDate,
    /// Distribution per unit in the listing's quote currency, **in the unit
    /// basis in force at [`Self::fetched_at`]** — not the basis of
    /// [`Self::ex_date`].
    ///
    /// That is the provider's own convention and it is not a rounding
    /// nicety: Yahoo restates a security's whole dividend history into the
    /// current basis, cumulatively, every time it splits. NVDA's pre-split
    /// dividends come back as `0.004` against a declared $0.04 (10-for-1,
    /// June 2024) and the ones before its 2021 4-for-1 come back as `0.004`
    /// too, against a declared $0.16 — measured 2026-08-27.
    ///
    /// The figure is stored exactly as served rather than converted back to
    /// the ex-date's basis, because a conversion could only use the splits
    /// recorded *at the time of the fetch* and would silently be wrong for
    /// any recorded later. Keeping the provider's figure and dating it with
    /// [`Self::fetched_at`] leaves the basis a fact rather than a guess — and
    /// a total is basis-independent, so a reader multiplies it by units in
    /// **this same** basis and gets the right dollars whichever basis it is
    /// (`closing_price::HeldTimeline::units_by_account_on` takes that basis as
    /// its own parameter for exactly this).
    #[sqlx(try_from = "Money")]
    pub amount_per_unit: Decimal,
    pub currency: String,
    /// Provider that produced the row, e.g. `"yahoo"`.
    pub source: String,
    /// The provider symbol the row was fetched under, in the namespace of
    /// [`Self::source`]. Informational: no calculation reads it; it is
    /// provenance, served by `GET /distribution_events`, shown on the
    /// Distribution Calendar screen and carried into `row_history`.
    pub fetched_symbol: String,
    /// RFC 3339 UTC timestamp of the fetch that produced the row.
    pub fetched_at: String,
}

impl CrudEntity for DistributionEvent {
    type Key = i64;
    const TABLE: &'static str = "distribution_events";
    const COLUMNS: &'static str =
        "id, listing_id, ex_date, amount_per_unit, currency, source, fetched_symbol, fetched_at";
    const ORDER_BY: &'static str = "ex_date DESC, listing_id, id";
    const NOUN: &'static str = "distribution event";
}

// ---------------------------------------------------------------------------
// The provider abstraction
// ---------------------------------------------------------------------------

/// One distribution as a provider returned it, before it is checked and
/// stored.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FetchedDistribution {
    /// The ex-date in the market's own timezone — the adapter's job, since
    /// recovering it is provider-specific (see the module docs).
    pub ex_date: NaiveDate,
    pub amount_per_unit: Decimal,
    /// The quote currency the provider reports — cross-checked against the
    /// listing's before the row is stored.
    pub currency: String,
}

/// What one provider call answered: the events it dated, and the ones it could
/// not.
///
/// The second half is not an error and not a silence. An event the adapter
/// cannot date is one the provider served without the context needed to place
/// it (for Yahoo, an action with no candle sharing its UTC date), and dropping
/// it quietly would let "the provider knows of no ex-date" — the reading the
/// missing-entry alert is built on — cover a case where it did. So it is
/// counted, and the job qualifies its own success with it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FetchedDistributions {
    pub events: Vec<FetchedDistribution>,
    /// The provider's own dates for events that could not be placed on the
    /// market's calendar, ascending.
    pub undatable: Vec<NaiveDate>,
}

pub type DistributionFuture<'a> =
    Pin<Box<dyn Future<Output = Result<FetchedDistributions, FetchError>> + Send + 'a>>;

/// A source of distribution history. Implementations do their own symbol
/// mapping and their own provider-date→ex-date recovery, both being
/// provider-specific; a failure is an error result, never an empty list, so a
/// provider outage can never read as "this listing paid nothing".
///
/// The failure is a [`FetchError`], shared with the price fetcher, so both
/// paths draw the one distinction that matters: did the provider **positively
/// answer** that it serves no such series ([`FetchError::NoSuchSymbol`]), or
/// did the call merely not succeed? A retired ticker is a standing fact about
/// the security and would otherwise fail the weekly job forever; an outage is a
/// reason to try again. See [`run_refresh`].
pub trait DistributionFetcher: Send + Sync {
    /// Identifier stored in each row's `source` column, e.g. `"yahoo"`.
    fn source(&self) -> &'static str;

    /// The symbol this provider is asked for when quoting `market` as at
    /// `date` — recorded on every row stored, so `source` and
    /// `fetched_symbol` are always in the same namespace.
    fn symbol(&self, market: &Market, date: NaiveDate) -> Result<String, String>;

    /// Every distribution with an ex-date in `from..=to`, ascending.
    fn distributions<'a>(
        &'a self,
        market: &'a Market,
        from: NaiveDate,
        to: NaiveDate,
    ) -> DistributionFuture<'a>;
}

/// The fetcher as it is injected: one shared instance for the whole process.
pub type SharedDistributionFetcher = Arc<dyn DistributionFetcher>;

// ---------------------------------------------------------------------------
// Persistence
// ---------------------------------------------------------------------------

#[derive(thiserror::Error, Debug)]
pub enum StoreError {
    /// A per-unit amount that is not a positive number. Enforced here rather
    /// than by a schema CHECK: the column is a TEXT decimal, and the only way
    /// a CHECK could compare it numerically is a `CAST(... AS REAL)` — exactly
    /// the float round-trip the money rules forbid near a stored figure.
    #[error(
        "distribution for listing {listing_id} on {ex_date} is not a positive amount: {amount}"
    )]
    NotPositive {
        listing_id: i64,
        ex_date: NaiveDate,
        amount: Decimal,
    },
    /// The provider quoted the distribution in a currency the listing does not
    /// trade in — the symbol resolved to some other security, or the provider
    /// changed its quote currency. Either way the per-unit figure cannot be
    /// multiplied by units and compared against an income row.
    #[error(
        "provider quoted listing {listing_id}'s {ex_date} distribution in {provider}, but the \
         listing trades in {listing}"
    )]
    CurrencyMismatch {
        listing_id: i64,
        ex_date: NaiveDate,
        provider: String,
        listing: String,
    },
    #[error("distribution event write failed: {0}")]
    Db(#[from] sqlx::Error),
}

impl From<StoreError> for ApiError {
    fn from(e: StoreError) -> Self {
        match e {
            StoreError::Db(e) => e.into(),
            other => ApiError::Unprocessable(other.to_string()),
        }
    }
}

/// Store one fetched distribution, inserting it or updating the stored row for
/// the same `(listing_id, ex_date)`.
///
/// An update rewrites the amount **and** its provenance: a re-fetch is a fresh
/// observation, and a stored row that kept an old `fetched_at` beside a new
/// figure would misdate the provider's own revision. The audit trail records
/// what the row said before (migration 0048), which is where a revised amount
/// stays visible.
pub async fn db_store(
    conn: &mut sqlx::SqliteConnection,
    listing_id: i64,
    event: &FetchedDistribution,
    listing_currency: &str,
    source: &str,
    fetched_symbol: &str,
    fetched_at: &str,
) -> Result<(), StoreError> {
    if event.amount_per_unit <= Decimal::ZERO {
        return Err(StoreError::NotPositive {
            listing_id,
            ex_date: event.ex_date,
            amount: event.amount_per_unit,
        });
    }
    if !event.currency.eq_ignore_ascii_case(listing_currency) {
        return Err(StoreError::CurrencyMismatch {
            listing_id,
            ex_date: event.ex_date,
            provider: event.currency.clone(),
            listing: listing_currency.to_string(),
        });
    }
    // ON CONFLICT over the natural key, so the surrogate id — and with it the
    // row's audit trail — survives a re-fetch.
    sqlx::query(
        "INSERT INTO distribution_events \
             (listing_id, ex_date, amount_per_unit, currency, source, fetched_symbol, fetched_at) \
         VALUES (?, ?, ?, ?, ?, ?, ?) \
         ON CONFLICT (listing_id, ex_date) DO UPDATE SET \
             amount_per_unit = excluded.amount_per_unit, \
             currency = excluded.currency, \
             source = excluded.source, \
             fetched_symbol = excluded.fetched_symbol, \
             fetched_at = excluded.fetched_at",
    )
    .bind(listing_id)
    .bind(event.ex_date)
    .bind(Money(event.amount_per_unit))
    .bind(&event.currency)
    .bind(source)
    .bind(fetched_symbol)
    .bind(fetched_at)
    .execute(&mut *conn)
    .await?;
    Ok(())
}

#[cfg(test)]
pub async fn db_list(pool: &SqlitePool) -> Result<Vec<DistributionEvent>, sqlx::Error> {
    http::crud_list::<DistributionEvent>(pool).await
}

// ---------------------------------------------------------------------------
// The refresh job
// ---------------------------------------------------------------------------

/// How far either side of the requested window the adapter may look for the
/// candle that dates a boundary event.
///
/// The candle join needs the event's *own* candle, and an event on the first
/// or last day of the window has it right at the edge — where a public holiday
/// or a weekend can push it outside a window cut to the day. Five days clears
/// the longest ASX closure. Events are filtered back to the requested window
/// after they are dated, so the margin widens what can be *placed*, never what
/// is stored.
pub const CANDLE_JOIN_MARGIN_DAYS: i64 = 5;

/// One refresh run: for every listing ever held, store the provider's
/// distribution history over the span it was held.
///
/// The window is the listing's own held span rather than a rolling lookback,
/// because the question the alerts ask is retrospective — "is there a
/// distribution from any year we held this that was never entered?" — and a
/// lookback would answer it only for the weeks since the feature was built.
/// One provider call per listing per run, which is why this is weekly work
/// rather than daily.
///
/// A listing whose fetch fails does not stop the others, and the run fails
/// (so the Jobs screen shows it) naming each failure. Events the adapter could
/// not date are not a failure — the run succeeded, it just did less than the
/// whole of its work — so they qualify the run with a note instead (the
/// `job_runs.note` shape SCENARIOS T-09 introduced).
pub async fn run_refresh(
    pool: &SqlitePool,
    fetcher: &dyn DistributionFetcher,
    now: DateTime<Utc>,
) -> Result<Option<String>, String> {
    let today = now.date_naive();
    let fetched_at = now.to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
    let timeline = closing_price::HeldTimeline::load(pool)
        .await
        .map_err(|e| e.to_string())?;

    let (mut stored, mut listings) = (0usize, 0usize);
    let mut failures: Vec<String> = Vec::new();
    let mut undated: Vec<String> = Vec::new();
    let mut retired: Vec<String> = Vec::new();
    for listing_id in timeline.listing_ids() {
        let Some(market) = closing_price::load_market(pool, listing_id)
            .await
            .map_err(|e| e.to_string())?
        else {
            continue;
        };
        // The whole span the listing was held, from the first acquisition to
        // the last day it was still held (capped at today — a distribution
        // cannot be missing from a year that has not happened).
        let spans = timeline.held_spans(listing_id, today);
        let (Some(first), Some(last)) = (spans.first(), spans.last()) else {
            continue;
        };
        let (from, to) = (first.0, last.1.min(today));
        if from > to {
            continue;
        }

        let symbol = match fetcher.symbol(&market, from) {
            Ok(symbol) => symbol,
            Err(e) => {
                failures.push(format!("{} ({listing_id}): {e}", market.listing.ticker));
                continue;
            }
        };
        let answer = match fetcher.distributions(&market, from, to).await {
            Ok(answer) => answer,
            // The provider positively answers that it serves no such series:
            // the security's ticker was retired, and asking again next week
            // will get the same answer forever. Failing the run on it would
            // leave a red Jobs screen nothing can clear — the shape SCENARIOS
            // Q-02 fixed for prices — while the honest consequence is only
            // that this listing has no calendar, which the alerts degrade
            // gracefully around (an alert not firing was never proof).
            Err(FetchError::NoSuchSymbol(message)) => {
                retired.push(format!(
                    "{} ({listing_id}): {message}",
                    market.listing.ticker
                ));
                continue;
            }
            // Anything else — an outage, a rate limit, a transport failure —
            // carries no verdict on the symbol and must fail loudly, or a
            // provider having a bad morning would read as "nothing to report".
            Err(FetchError::Other(message)) => {
                failures.push(format!(
                    "{} ({listing_id}): {message}",
                    market.listing.ticker
                ));
                continue;
            }
        };
        if !answer.undatable.is_empty() {
            undated.push(format!(
                "{} ({listing_id}): {}",
                market.listing.ticker,
                answer
                    .undatable
                    .iter()
                    .map(ToString::to_string)
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }

        listings += 1;
        // One transaction per listing: a provider that answers for six
        // listings and fails on the seventh still leaves those six refreshed,
        // and each listing's own set lands atomically.
        let mut tx = crate::infra::db::write_tx(pool)
            .await
            .map_err(|e| e.to_string())?;
        let mut failed = None;
        for event in &answer.events {
            if let Err(e) = db_store(
                &mut tx,
                listing_id,
                event,
                &market.listing.currency,
                fetcher.source(),
                &symbol,
                &fetched_at,
            )
            .await
            {
                failed = Some(format!("{} ({listing_id}): {e}", market.listing.ticker));
                break;
            }
        }
        match failed {
            Some(message) => {
                tx.rollback().await.map_err(|e| e.to_string())?;
                failures.push(message);
            }
            None => {
                tx.commit().await.map_err(|e| e.to_string())?;
                stored += answer.events.len();
            }
        }
    }

    tracing::info!(
        listings,
        stored,
        failed = failures.len(),
        undated = undated.len(),
        retired = retired.len(),
        "distribution calendar refresh complete"
    );
    if !failures.is_empty() {
        return Err(failures.join("; "));
    }
    // Both notes qualify a run that succeeded while doing less than the whole
    // of its work, and both are permanent facts rather than transient ones —
    // so they are said on every run, where the Jobs screen shows them beside
    // an `ok` status (SCENARIOS T-09).
    let mut notes: Vec<String> = Vec::new();
    if !retired.is_empty() {
        notes.push(format!(
            "{} listing(s) have no calendar: the provider serves no series under the symbol they \
             were quoted under, so nothing can be collected for them and the missing-distribution \
             alert cannot speak for them: {}",
            retired.len(),
            retired.join("; ")
        ));
    }
    if !undated.is_empty() {
        notes.push(format!(
            "{} provider event(s) could not be placed on their market's calendar and were not \
             stored: {}",
            undated.len(),
            undated.join("; ")
        ));
    }
    Ok((!notes.is_empty()).then(|| notes.join(" ")))
}

// ---------------------------------------------------------------------------
// Routes
// ---------------------------------------------------------------------------

pub fn router() -> Router<SqlitePool> {
    Router::new()
        .route(
            "/distribution_events",
            get(http::list_handler::<DistributionEvent>),
        )
        .route(
            "/distribution_events/{id}",
            get(http::get_handler::<DistributionEvent>),
        )
}

#[cfg(test)]
mod tests;
