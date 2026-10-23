//! Health / data-freshness report: the one read the web UI's cross-view
//! banner polls. Surfaces, in a single read transaction:
//!
//! - the latest stored ok closing-price date and whether it is stale
//!   (older than [`PRICE_STALE_BUSINESS_DAYS`] business days — a coarse
//!   Mon–Fri count, deliberately ignoring per-exchange holiday calendars:
//!   this is a freshness alarm across every exchange and crypto, not a
//!   settlement calculation);
//! - the latest imported RBA FX rate month and whether it is stale (the RBA
//!   publishes month M's F11 rates shortly after M ends, so anything older
//!   than the previous calendar month means the weekly import has stopped
//!   landing new months);
//! - every job whose most recent recorded run failed;
//! - every scheduled job that is **overdue**: the instant its own timer stored
//!   as its next run came and went, and nothing moved it on (see
//!   [`OverdueJob`]);
//! - every job whose most recent run has been `running` far longer than any run
//!   of it should take — a run the process did not survive rather than one in
//!   flight (see [`StalledJob`]);
//! - every listing with at least one errored closing-price row (a wrong,
//!   renamed, or delisted provider symbol otherwise only shows up
//!   indirectly, as a missing snapshot from the errored date onward —
//!   `reports::valuation` refuses to value a date with an errored price);
//! - every listing with a held day whose price was never even attempted —
//!   the missing-row counterpart of the errored list (see
//!   [`UnpricedListing`]);
//! - every recorded demerger whose **head listing looks like the entity the
//!   demerger created** rather than the one that continued — the two sides
//!   entered the wrong way round, which no tax figure and no write-time check
//!   can see, but which leaves the head parcel on a listing whose price series
//!   begins after it was bought (see [`DemergerHeadNotContinuing`]);
//! - every pair of listings holding the *same* closing price on a long run of
//!   consecutive trading days — one security's series stored under two
//!   listings, which no other list here can see because every row is `ok`
//!   (see [`DuplicatePriceSeries`]);
//! - every (listing, action type, date) carrying more than one corporate
//!   action — the double-entry that silently compounds (see
//!   [`DuplicateAction`]);
//! - every (listing, financial year, holding account) carrying more than one
//!   AMMA statement — the same double-entry on the attribution side, counted
//!   twice in the income, gains and cost-base figures alike (see
//!   [`DuplicateAmmaStatement`]);
//! - every (listing, holding account, payment date) carrying more than one
//!   income row of identical amounts — the same double-entry on the
//!   distribution side, doubling the dividend income and the franking credits
//!   (see [`DuplicateIncome`]);
//! - every identical pair of interest-income rows, and of investment-expense
//!   rows — the same double-entry on the two listing-less sides of the tax
//!   summary, doubling a year's interest or its deduction (see
//!   [`DuplicateInterest`], [`DuplicateExpense`]);
//! - every (listing, holding account, taxing point) carrying more than one ESS
//!   statement of identical figures — the same double-entry on the
//!   employee-share-scheme side, doubling the year's Item 12 discount and
//!   vesting the parcel twice (see [`DuplicateEssStatement`]);
//! - every (listing, holding account, date of death) carrying more than one
//!   inheritance of identical figures — the same double-entry on the
//!   deceased-estate side, and the one that doubles a *holding* rather than a
//!   year's income (see [`DuplicateInheritance`]);
//! - every disposal of ESS-vested shares within 30 days after the taxing
//!   point, where the ESS 30-day rule re-measures the discount, moves it into
//!   the disposal's year and cancels the capital gain — the one entry here
//!   that is wrong in two years at once (see [`EssThirtyDaySale`]);
//! - every listing whose `currency` is not the one its own exchange quotes in
//!   — a holding that can never be priced again and, once it has history,
//!   cannot be corrected in place either (see [`ExchangeCurrencyMismatch`]);
//! - every trade dated on a day its own exchange did not trade — the CGT
//!   event date on a day the market was shut (see [`NonTradingDayTrade`]);
//! - every disposal recorded at **nil proceeds** — where the market-value
//!   substitution rule may put the asset's market value in place of the
//!   nothing that was received, and entering the nothing fabricates a capital
//!   loss the size of the whole cost base (see [`NilProceedsDisposal`]).
//!
//! The non-trading-day entry is the safety net under a write-time rule rather
//! than a check of its own: `PUT /trades/:id` and `PUT /sells/:id` **refuse**
//! such a date outright (SCENARIOS S-08), but the derived write paths — an ESS
//! vest, an inherited parcel, a DRP reinvestment, a rights exercise, and every
//! parcel-substituting operation — insert their trade rows without that check,
//! deliberately, because a taxing point, a date of death or a corporate
//! action's own date is legitimately not a market day. So the alert reads
//! *every* trade, not only the ones the refusal would have caught, and stays
//! non-blocking.
//!
//! The ESS 30-day entry is the odd one out in kind: not a double entry but a
//! **date pattern**, advisory in the way `reports::wash_sales` is. It lives here
//! rather than in its own report because it needs no parameters and belongs on
//! the same cross-view banner — the point is to catch the case at entry time
//! rather than at return time.
//!
//! A database with no prices or FX rates at all reports `stale = false` for
//! that series: nothing has decayed — a fresh install shows no banner, and a
//! price/FX import that breaks before ever succeeding surfaces through
//! `failed_jobs` (and the Jobs page) instead.
//!
//! The checks live one family per submodule — `jobs`, `prices`,
//! `duplicates`, `trades` (the per-row trade and listing checks) and
//! `calendar` (the distribution calendar) — each a `db_*` read over the
//! connection of the one read transaction [`db_health`] begins, re-exported
//! here so every `reports::health::X` path is unchanged. This module is only
//! the report itself: the freshness figures, the assembly, and the route.

use crate::infra::db;
use crate::infra::extract::Json;
use crate::infra::http::ApiError;
use axum::extract::State;
use chrono::{DateTime, Datelike, NaiveDate, Utc, Weekday};
use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

mod calendar;
mod duplicates;
mod jobs;
mod prices;
mod trades;

pub use calendar::*;
pub use duplicates::*;
pub use jobs::*;
pub use prices::*;
pub use trades::*;

/// Prices are stale once the latest ok closing price is more than this many
/// business days (Mon–Fri) old. The price-import job runs every weekday, so a
/// healthy database is at most 1–2 business days behind; 3 leaves headroom for
/// a long exchange-holiday weekend without a false alarm.
pub const PRICE_STALE_BUSINESS_DAYS: i64 = 3;

#[derive(utoipa::ToSchema, Debug, Serialize, Deserialize)]
pub struct HealthReport {
    /// Latest `closing_prices` date stored with status ok, across every
    /// listing; `None` when no price has ever been stored.
    pub latest_price_date: Option<NaiveDate>,
    pub prices_stale: bool,
    /// Latest `rba_fx_rates` month (`YYYY-MM`); `None` when none imported yet.
    pub latest_fx_month: Option<String>,
    pub fx_stale: bool,
    pub failed_jobs: Vec<FailedJob>,
    /// Every scheduled job whose stored next run is more than
    /// [`JOB_OVERDUE_GRACE_HOURS`] in the past, by name. Empty when every
    /// spawned schedule entry is still moving its row on — and on a database
    /// whose server has not yet run the scheduler at all, since the table is
    /// then empty and nothing has been promised.
    pub overdue_jobs: Vec<OverdueJob>,
    /// Every job whose most recent run has been open for more than
    /// [`JOB_RUNNING_STALE_HOURS`], by name. Empty when no run is stuck.
    pub stalled_jobs: Vec<StalledJob>,
    /// Listings with at least one errored closing-price row, newest error
    /// first. Empty when every stored price is ok.
    pub errored_prices: Vec<ErroredPriceListing>,
    /// Listings with a held day that has no stored price row at all, oldest
    /// hole first — the oldest is the least recoverable, since a provider
    /// stops serving history long before it stops serving last week.
    pub unpriced_days: Vec<UnpricedListing>,
    /// Every recorded demerger whose head listing holds pre-demerger prices
    /// the provider served after the demerger, and which carries no stated
    /// pre-demerger close to re-base them from, newest demerger first. Empty
    /// when every such demerger states its close (or has no affected price).
    /// Each row carries two figures: the fetched rows a stated close repairs,
    /// and the hand-entered rows in the same span that it does not.
    pub demergers_missing_close: Vec<DemergerMissingClose>,
    /// Every recorded demerger whose head listing looks like the entity the
    /// demerger *created* rather than the one that continued — no stored
    /// pre-demerger price of its own, a parcel held before that date, and a
    /// demerged listing carrying the provider series the head lacks. Newest
    /// demerger first; empty when no demerger's two sides are inverted.
    pub demergers_head_not_continuing: Vec<DemergerHeadNotContinuing>,
    /// Every pair of listings whose stored closing prices are identical across
    /// a run of at least [`DUPLICATE_PRICE_SERIES_RUN_DAYS`] consecutive
    /// shared trading days, newest run first — one security's series stored
    /// under two listings. Empty when no two listings track each other that
    /// closely.
    pub duplicate_price_series: Vec<DuplicatePriceSeries>,
    /// Every (listing, contract note reference) carrying more than one trade,
    /// newest first — one broker confirmation entered twice. Empty when no two
    /// trades of a listing quote the same reference; trades that record no
    /// reference (every derived one, and any hand entry that left it blank)
    /// are never in it.
    pub duplicate_trades: Vec<DuplicateTrade>,
    /// Every (listing, action type, date) carrying more than one corporate
    /// action, newest first. Empty when no two actions of a type share a
    /// listing and date.
    pub duplicate_actions: Vec<DuplicateAction>,
    /// Every (listing, financial year, holding account) carrying more than one
    /// AMMA statement, newest year first. Empty when no fund-year has two
    /// statements for one account.
    pub duplicate_amma_statements: Vec<DuplicateAmmaStatement>,
    /// Every (listing, holding account, payment date) carrying more than one
    /// income row of identical amounts, newest first. Empty when no two rows
    /// declare the same payment twice.
    pub duplicate_income: Vec<DuplicateIncome>,
    /// Every group of identical interest-income rows (same date, figures,
    /// currency, source and holding account), newest first.
    pub duplicate_interest: Vec<DuplicateInterest>,
    /// Every group of identical investment-expense rows (same date, type,
    /// figures, currency, description and attributions), newest first.
    pub duplicate_expenses: Vec<DuplicateExpense>,
    /// Every (listing, holding account, taxing point) carrying more than one
    /// ESS statement of identical figures, newest first. Empty when no vest is
    /// declared twice.
    pub duplicate_ess_statements: Vec<DuplicateEssStatement>,
    /// Every (listing, holding account, date of death) carrying more than one
    /// inheritance of identical figures, newest death first. Empty when no
    /// inherited parcel is recorded twice.
    pub duplicate_inheritances: Vec<DuplicateInheritance>,
    /// Every listing whose `currency` differs from its exchange's, by ticker.
    /// Empty when every listing agrees with the market it trades on;
    /// exchange-less (Crypto) listings never appear.
    pub exchange_currency_mismatches: Vec<ExchangeCurrencyMismatch>,
    /// Every disposal of ESS-vested shares within 30 days after the statement's
    /// taxing point, newest sale first — where the 30-day rule re-measures the
    /// discount and cancels the capital gain. Empty when no such sale exists.
    pub ess_30_day_rule: Vec<EssThirtyDaySale>,
    /// Every trade dated on a day its own exchange did not trade, newest
    /// first. Empty when every trade falls on a market day (which the trade
    /// and Sell write paths now enforce for the rows they see).
    pub non_trading_day_trades: Vec<NonTradingDayTrade>,
    /// Every disposal recorded at **nil proceeds**, newest first — where the
    /// market-value substitution rule may mean the proceeds are the asset's
    /// market value rather than the nothing that was received. Empty when
    /// every disposal records what it realised.
    pub nil_proceeds_disposals: Vec<NilProceedsDisposal>,
    /// Every distribution the calendar knows about that the books do not:
    /// a stored ex-date where the (listing, holding account) held units on the
    /// last cum-dividend day and no income row matches it, newest ex-date
    /// first. Empty when every known distribution has been entered — and, on a
    /// database whose `distribution-import` job has never run, empty because
    /// nothing is known.
    pub missing_dividend_entries: Vec<MissingDividendEntry>,
    /// Every entered distribution whose gross does not match the calendar's
    /// per-unit figure times the units held, newest ex-date first. The
    /// likelier of the two errors — a typo in a figure, against a distribution
    /// wholly forgotten — and the one the provider's 6 dp agreement with the
    /// registry shows it is accurate enough to catch.
    pub dividend_amount_mismatches: Vec<DividendAmountMismatch>,
}

/// Business days (Mon–Fri) strictly after `from`, up to and including `today`.
/// Zero when `today <= from`.
fn business_days_since(from: NaiveDate, today: NaiveDate) -> i64 {
    let mut days = 0;
    let mut d = from;
    while d < today {
        d = d.succ_opt().expect("date within chrono range");
        if !matches!(d.weekday(), Weekday::Sat | Weekday::Sun) {
            days += 1;
        }
    }
    days
}

/// The calendar month before `today`'s, as the `YYYY-MM` key `rba_fx_rates`
/// uses. `YYYY-MM` strings compare correctly lexicographically.
fn previous_month(today: NaiveDate) -> String {
    let (year, month) = if today.month() == 1 {
        (today.year() - 1, 12)
    } else {
        (today.year(), today.month() - 1)
    };
    format!("{year:04}-{month:02}")
}

/// Read the freshness facts on one snapshot. `today` and `now` are parameters
/// so tests can pin the staleness thresholds and the "close is final yet"
/// cut-off to fixed dates.
pub async fn db_health(
    pool: &SqlitePool,
    today: NaiveDate,
    now: DateTime<Utc>,
) -> Result<HealthReport, sqlx::Error> {
    let mut tx = db::read_tx(pool).await?;
    let latest_price_date: Option<NaiveDate> =
        sqlx::query_scalar("SELECT MAX(price_date) FROM closing_prices WHERE status = 'ok'")
            .fetch_one(&mut *tx)
            .await?;
    let latest_fx_month: Option<String> = sqlx::query_scalar("SELECT MAX(month) FROM rba_fx_rates")
        .fetch_one(&mut *tx)
        .await?;
    let failed_jobs = sqlx::query_as::<_, FailedJob>(
        "SELECT name, finished_at, error FROM job_runs r \
         WHERE id = (SELECT MAX(id) FROM job_runs WHERE name = r.name) \
           AND status = 'failed' \
         ORDER BY name",
    )
    .fetch_all(&mut *tx)
    .await?;
    // Errored rows dated from a listing's `unpriced_from`, or before its
    // `unpriced_before`, are expected rather than a to-do: outside that span
    // the provider serves nothing and valuation carries the last close
    // forward (SCENARIOS Q-02) or leaves the holding out of the date's totals
    // (migration 0037), so nagging about them would be a permanent alarm
    // nobody can clear. Rows *inside* the span are real holes and still
    // reported — as is the whole listing while both markers are unset.
    let errored_prices = sqlx::query_as::<_, ErroredPriceListing>(
        "SELECT cp.listing_id AS listing_id, l.ticker AS ticker, \
                COUNT(*) AS errored_days, MAX(cp.price_date) AS latest_errored_date, \
                (SELECT cp2.error FROM closing_prices cp2 \
                 WHERE cp2.listing_id = cp.listing_id AND cp2.status = 'error' \
                   AND (l.unpriced_from IS NULL OR cp2.price_date < l.unpriced_from) \
                   AND (l.unpriced_before IS NULL OR cp2.price_date >= l.unpriced_before) \
                 ORDER BY cp2.price_date DESC LIMIT 1) AS latest_error \
         FROM closing_prices cp JOIN listings l ON l.id = cp.listing_id \
         WHERE cp.status = 'error' \
           AND (l.unpriced_from IS NULL OR cp.price_date < l.unpriced_from) \
           AND (l.unpriced_before IS NULL OR cp.price_date >= l.unpriced_before) \
         GROUP BY cp.listing_id \
         ORDER BY latest_errored_date DESC",
    )
    .fetch_all(&mut *tx)
    .await?;
    let overdue_jobs = db_overdue_jobs(&mut tx, now).await?;
    let stalled_jobs = db_stalled_jobs(&mut tx, now).await?;
    let demergers_missing_close = db_demergers_missing_close(&mut tx).await?;
    let demergers_head_not_continuing = db_demergers_head_not_continuing(&mut tx).await?;
    let duplicate_price_series = db_duplicate_price_series(&mut tx).await?;
    let Duplicates {
        trades: duplicate_trades,
        actions: duplicate_actions,
        amma_statements: duplicate_amma_statements,
        income: duplicate_income,
        interest: duplicate_interest,
        expenses: duplicate_expenses,
        ess_statements: duplicate_ess_statements,
        inheritances: duplicate_inheritances,
    } = db_duplicates(&mut tx).await?;
    let exchange_currency_mismatches = db_exchange_currency_mismatches(&mut tx).await?;
    let ess_30_day_rule = db_ess_30_day_rule(&mut tx).await?;
    let non_trading_day_trades = db_non_trading_day_trades(&mut tx).await?;
    let nil_proceeds_disposals = db_nil_proceeds_disposals(&mut tx).await?;
    let (missing_dividend_entries, dividend_amount_mismatches) =
        db_distribution_calendar(&mut tx, today).await?;
    let unpriced_days = db_unpriced_days(&mut tx, now).await?;
    tx.commit().await?;

    let prices_stale = latest_price_date
        .is_some_and(|d| business_days_since(d, today) > PRICE_STALE_BUSINESS_DAYS);
    let fx_stale = latest_fx_month
        .as_deref()
        .is_some_and(|m| m < previous_month(today).as_str());
    Ok(HealthReport {
        latest_price_date,
        prices_stale,
        latest_fx_month,
        fx_stale,
        failed_jobs,
        overdue_jobs,
        stalled_jobs,
        errored_prices,
        unpriced_days,
        demergers_missing_close,
        demergers_head_not_continuing,
        duplicate_price_series,
        duplicate_trades,
        duplicate_actions,
        duplicate_amma_statements,
        duplicate_income,
        duplicate_interest,
        duplicate_expenses,
        duplicate_ess_statements,
        duplicate_inheritances,
        exchange_currency_mismatches,
        ess_30_day_rule,
        non_trading_day_trades,
        nil_proceeds_disposals,
        missing_dividend_entries,
        dividend_amount_mismatches,
    })
}

#[utoipa::path(
    get,
    path = "/reports/health",
    summary = "The data-health report behind the UI's banner: stale prices and FX, failed and stalled jobs, unpriced days, duplicates and more."
)]
async fn report(State(pool): State<SqlitePool>) -> Result<Json<HealthReport>, ApiError> {
    let today = chrono::Local::now().date_naive();
    db_health(&pool, today, Utc::now())
        .await
        .map(Json)
        .map_err(ApiError::from)
}

pub fn router() -> OpenApiRouter<SqlitePool> {
    OpenApiRouter::new().routes(routes!(report))
}

#[cfg(test)]
mod tests;
