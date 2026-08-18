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
//! - every listing with at least one errored closing-price row (a wrong,
//!   renamed, or delisted provider symbol otherwise only shows up
//!   indirectly, as a missing snapshot from the errored date onward —
//!   `reports::valuation` refuses to value a date with an errored price);
//! - every listing with a held day whose price was never even attempted —
//!   the missing-row counterpart of the errored list (see
//!   [`UnpricedListing`]);
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
//!   vesting the parcel twice (see [`DuplicateEssStatement`]).
//!
//! A database with no prices or FX rates at all reports `stale = false` for
//! that series: nothing has decayed — a fresh install shows no banner, and a
//! price/FX import that breaks before ever succeeding surfaces through
//! `failed_jobs` (and the Jobs page) instead.

use crate::domain::tax_year::tax_year_for;
use crate::entities::closing_price::{self, HeldTimeline};
use crate::entities::ess_statement::{self, EssStatement};
use crate::entities::income::Income;
use crate::entities::interest_income::InterestIncome;
use crate::entities::investment_expense::{ExpenseType, InvestmentExpense};
use crate::infra::http::ApiError;
use axum::{Json, Router, extract::State, routing::get};
use chrono::{DateTime, Datelike, Duration, NaiveDate, Utc, Weekday};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;
use std::collections::{BTreeSet, HashMap, HashSet};

/// Prices are stale once the latest ok closing price is more than this many
/// business days (Mon–Fri) old. The price-import job runs every weekday, so a
/// healthy database is at most 1–2 business days behind; 3 leaves headroom for
/// a long exchange-holiday weekend without a false alarm.
pub const PRICE_STALE_BUSINESS_DAYS: i64 = 3;

/// A job whose most recent recorded run failed.
#[derive(Debug, Serialize, Deserialize, sqlx::FromRow)]
pub struct FailedJob {
    pub name: String,
    pub finished_at: String,
    pub error: Option<String>,
}

/// A listing with one or more errored closing-price rows: a stuck symbol
/// (wrong, renamed, or delisted) would otherwise only show up indirectly, as
/// a missing snapshot from the errored date onward (`reports::valuation`
/// refuses to value a date with an errored price). Re-fetch it via
/// `POST /closing_prices/backfill` (or `/fetch` for a single date) once the
/// underlying symbol issue — see `latest_error` — is fixed (e.g. set
/// `listings.price_symbol`).
#[derive(Debug, Serialize, Deserialize, sqlx::FromRow)]
pub struct ErroredPriceListing {
    pub listing_id: i64,
    pub ticker: String,
    /// Count of errored rows for this listing (any date, not just recent).
    pub errored_days: i64,
    pub latest_errored_date: NaiveDate,
    pub latest_error: String,
}

/// A listing with a held day whose price was never stored at all — the
/// missing-row counterpart of [`ErroredPriceListing`]. An errored fetch at
/// least leaves a row to find; a day nobody ever asked for is silent and
/// permanent: it only shows up as a snapshot stuck stale, and by the time it
/// is noticed the provider may no longer serve that far back.
///
/// It happens whenever a trade is entered later than the price-import job's
/// lookback window on a listing not otherwise held — a batch of statements
/// entered years after the fact — so nothing ever attempted those days.
///
/// A day is unpriced when it is exactly what `reports::valuation` would ask
/// for and not find: the listing was held on some calendar date, that date's
/// valuation day (`Market::latest_trading_day_on_or_before`) has no
/// `closing_prices` row, and that day's close is already final. A day whose
/// row is errored belongs to `errored_prices` instead — the two lists
/// partition the problem. Close it with `POST /closing_prices/backfill`, or a
/// manual price for a day the provider can never serve.
#[derive(Debug, Serialize, Deserialize)]
pub struct UnpricedListing {
    pub listing_id: i64,
    pub ticker: String,
    /// Count of distinct valuation days with no stored row.
    pub unpriced_days: i64,
    pub earliest_date: NaiveDate,
    pub latest_date: NaiveDate,
}

/// More than one corporate action of the same type, on the same listing and
/// date. Two such rows are two independent events to every reader — the
/// cost-base pipeline sums both `ReturnOfCapital` reductions and multiplies
/// both `ShareSplit` ratios (SCENARIOS E-03, E-15) — so a re-submitted form or
/// a re-imported statement restates every cost base and quantity of the
/// listing with nothing to show for it.
///
/// Deliberately a **warning, not a constraint**: a genuine same-day pair
/// exists in principle (two tranches of one capital return), so the pair stays
/// enterable and this names it for the user to judge.
#[derive(Debug, Serialize, Deserialize)]
pub struct DuplicateAction {
    pub listing_id: i64,
    pub ticker: String,
    pub action_type: String,
    pub date: NaiveDate,
    /// How many actions share this (listing, type, date) — always ≥ 2.
    pub action_count: i64,
    /// The ids sharing it, ascending, so the surplus row can be found and
    /// deleted without a search.
    pub action_ids: Vec<i64>,
}

/// The grouped row behind [`DuplicateAction`]: SQLite returns the ids as one
/// `GROUP_CONCAT` string, split into the public struct's `Vec<i64>` by
/// [`db_duplicate_actions`].
#[derive(sqlx::FromRow)]
struct DuplicateActionRow {
    listing_id: i64,
    ticker: String,
    action_type: String,
    date: NaiveDate,
    action_count: i64,
    action_ids: String,
}

/// More than one AMMA statement for the same listing, financial year and
/// holding account. Every reader counts both: the tax summary's `amma_*`
/// lines and the net-capital-gain report's gain buckets each sum all
/// statements of the year, and — because the one-adjustment-per-parcel UNIQUE
/// index is per *statement* — each statement can also generate its own full
/// set of AMIT adjustments, reducing every parcel a second time
/// (SCENARIOS F-06). The usual cause is an amended statement entered as a new
/// row instead of over the original.
///
/// Deliberately a **warning, not a constraint**, the same call as
/// [`DuplicateAction`]: a genuine pair exists in principle (a registry change
/// mid-year, or a fund merger, leaves two part-year statements for one
/// account) and an amended statement is easier to check against the original
/// while both are enterable. The holding account is part of the key because
/// the statement is issued per holder account — a fund held in two accounts
/// legitimately has two statements for one year (SCENARIOS F-03).
#[derive(Debug, Serialize, Deserialize)]
pub struct DuplicateAmmaStatement {
    pub listing_id: i64,
    pub ticker: String,
    /// The financial year the statements attribute, identified by the
    /// calendar year of their shared 30 June end (`domain::tax_year`).
    pub tax_year: i32,
    pub holding_account_id: i64,
    /// How many statements share this (listing, year, account) — always ≥ 2.
    pub statement_count: i64,
    /// The ids sharing it, ascending, so the superseded row can be found and
    /// deleted without a search.
    pub statement_ids: Vec<i64>,
}

/// The grouped row behind [`DuplicateAmmaStatement`], shaped like
/// [`DuplicateActionRow`]: SQLite returns the ids as one `GROUP_CONCAT`
/// string, split into the public struct's `Vec<i64>` by
/// [`db_duplicate_amma_statements`].
#[derive(sqlx::FromRow)]
struct DuplicateAmmaStatementRow {
    listing_id: i64,
    ticker: String,
    tax_year_end_date: NaiveDate,
    holding_account_id: i64,
    statement_count: i64,
    statement_ids: String,
}

/// More than one income row for the same listing, holding account and payment
/// date, carrying **identical amounts**. Every reader counts both: the tax
/// summary's dividend lines, the franking credits behind them, the foreign
/// income and the FITO limit are each summed row by row, so a re-submitted
/// form or a re-imported statement declares the distribution twice
/// (SCENARIOS G-24).
///
/// Deliberately a **warning, not a constraint**, the same call as
/// [`DuplicateAction`] and [`DuplicateAmmaStatement`]: two dividends from one
/// company on one day are legitimate in principle (an ordinary and a special
/// dividend), so the pair stays enterable.
///
/// The amounts are part of the key for exactly that reason. (listing, account,
/// date) alone would flag that legitimate pair, which differs in what it pays;
/// requiring every money figure to match as well — and the currency they are
/// stated in — leaves only what is almost certainly one distribution entered
/// twice. Non-money differences (an `ex_date` filled in on one row only, a
/// trust flag) are ignored: they are how a re-entry usually differs from the
/// original, not evidence of a second payment.
#[derive(Debug, Serialize, Deserialize)]
pub struct DuplicateIncome {
    pub listing_id: i64,
    pub ticker: String,
    pub holding_account_id: i64,
    pub date_paid: NaiveDate,
    /// ISO 4217 currency the shared amounts are stated in (part of the key —
    /// two rows stating the same figures in different currencies are not the
    /// same payment).
    pub currency: String,
    /// The gross cash the duplicated rows each declare
    /// (`Income::gross_cash_income`, in `currency`), so the warning names the
    /// distribution rather than only its date.
    pub gross_amount: Decimal,
    /// How many rows share this (listing, account, date, amounts) — always ≥ 2.
    pub income_count: i64,
    /// The ids sharing it, ascending, so the surplus row can be found and
    /// deleted without a search.
    pub income_ids: Vec<i64>,
}

/// More than one interest-income row carrying identical figures on one date
/// from one source (SCENARIOS H-01). Interest has no listing to key on, so
/// `source` — the free-text payer, "ANZ savings account" — and the optional
/// holding account stand in for it: two $250 credits on one day from two
/// different banks are legitimate and stay unflagged, while the same credit
/// keyed twice doubles the year's `interest_income` line and any withholding
/// beside it.
///
/// Deliberately a **warning, not a constraint**, the same call as
/// [`DuplicateIncome`]: a payer really can credit the same amount twice in one
/// day (two term deposits of equal size maturing together), so the pair stays
/// enterable.
#[derive(Debug, Serialize, Deserialize)]
pub struct DuplicateInterest {
    pub date_paid: NaiveDate,
    /// ISO 4217 currency the shared amount is stated in (part of the key).
    pub currency: String,
    /// The gross interest each duplicated row declares, in `currency`.
    pub amount: Decimal,
    /// The free-text source the rows share (part of the key); `None` when none
    /// of them recorded one.
    pub source: Option<String>,
    /// The holding account the rows share (part of the key); `None` for
    /// interest from outside the portfolio's accounts.
    pub holding_account_id: Option<i64>,
    /// How many rows share the whole key — always ≥ 2.
    pub interest_count: i64,
    /// The ids sharing it, ascending, so the surplus row can be opened and
    /// deleted without a search.
    pub interest_ids: Vec<i64>,
}

/// More than one investment-expense row carrying identical figures on one date
/// (SCENARIOS H-06). The deduction is claimed once per row, so a re-submitted
/// form lifts the year's `deductions_*` line and lowers
/// `net_assessable_investment_income` by the same amount again.
///
/// The key is everything that identifies the expense: the date, the type, the
/// money figures (including the `gross_amount` / `deductible_percentage`
/// provenance pair), the currency, the free-text description, and both optional
/// attributions. Two advice fees of $200 on one day against two different
/// listings — or with different descriptions — are legitimate and stay
/// unflagged; the same invoice keyed twice is not.
///
/// Deliberately a **warning, not a constraint**, the same call as
/// [`DuplicateIncome`].
#[derive(Debug, Serialize, Deserialize)]
pub struct DuplicateExpense {
    pub date_incurred: NaiveDate,
    pub expense_type: ExpenseType,
    /// ISO 4217 currency the shared amount is stated in (part of the key).
    pub currency: String,
    /// The deductible amount each duplicated row claims, in `currency`.
    pub amount: Decimal,
    /// The free-text description the rows share (part of the key).
    pub description: Option<String>,
    /// The listing the rows are attributed to (part of the key); `None` for a
    /// portfolio-wide expense.
    pub listing_id: Option<i64>,
    /// That listing's ticker, so the warning names the holding rather than only
    /// its id; `None` for a portfolio-wide expense.
    pub ticker: Option<String>,
    /// The holding account the rows are attributed to (part of the key);
    /// `None` for a portfolio-wide expense.
    pub holding_account_id: Option<i64>,
    /// How many rows share the whole key — always ≥ 2.
    pub expense_count: i64,
    /// The ids sharing it, ascending, so the surplus row can be opened and
    /// deleted without a search.
    pub expense_ids: Vec<i64>,
}

/// More than one ESS statement for the same listing, holding account and
/// taxing point, carrying **identical figures** (SCENARIOS J-11). Every reader
/// counts both: the tax summary's Item 12 discount labels (and the $1,000
/// taxed-upfront reduction computed over them) sum statement by statement, and
/// each statement vests its **own** parcel — so a $1,000 grant of 100 shares
/// entered twice reports $2,000 of discount and 200 shares held.
///
/// The 30-day rule makes this the expected accident rather than a hypothetical:
/// an employer issues an **amended** statement for one vest
/// (`docs/ato/ess-30-day-rule.md`), and a user who enters both instead of
/// editing the original has exactly this shape.
///
/// Deliberately a **warning, not a constraint**, the same call as
/// [`DuplicateIncome`]: two vests on one date from different grants are
/// ordinary, so the pair stays enterable. The figures are part of the key for
/// exactly that reason — two same-date statements differing in quantity, market
/// value or any discount label are two grants, not one entered twice.
#[derive(Debug, Serialize, Deserialize)]
pub struct DuplicateEssStatement {
    pub listing_id: i64,
    pub ticker: String,
    pub holding_account_id: i64,
    pub taxing_point_date: NaiveDate,
    /// ISO 4217 currency the shared figures are stated in (part of the key).
    pub currency: String,
    /// The shares each duplicated statement vests, so the warning names the
    /// grant rather than only its date.
    pub quantity: Decimal,
    /// The total Item 12 discount each duplicated statement declares (D + E +
    /// F + the pre-2009 cessation label, in `currency`) — the figure that is
    /// counted once per statement.
    pub discount_total: Decimal,
    /// How many statements share the whole key — always ≥ 2.
    pub statement_count: i64,
    /// The ids sharing it, ascending, so the surplus row can be opened and
    /// deleted without a search.
    pub statement_ids: Vec<i64>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct HealthReport {
    /// Latest `closing_prices` date stored with status ok, across every
    /// listing; `None` when no price has ever been stored.
    pub latest_price_date: Option<NaiveDate>,
    pub prices_stale: bool,
    /// Latest `rba_fx_rates` month (`YYYY-MM`); `None` when none imported yet.
    pub latest_fx_month: Option<String>,
    pub fx_stale: bool,
    pub failed_jobs: Vec<FailedJob>,
    /// Listings with at least one errored closing-price row, newest error
    /// first. Empty when every stored price is ok.
    pub errored_prices: Vec<ErroredPriceListing>,
    /// Listings with a held day that has no stored price row at all, oldest
    /// hole first — the oldest is the least recoverable, since a provider
    /// stops serving history long before it stops serving last week.
    pub unpriced_days: Vec<UnpricedListing>,
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

/// Held days with no stored closing-price row at all, per listing (see
/// [`UnpricedListing`]).
///
/// Deliberately shaped as the exact question `reports::valuation` asks, so
/// there are no false positives: for every calendar date a listing was held,
/// the valuation day it resolves to must have a row. Days whose close is not
/// final yet (today's, an unsettled crypto candle) are out of scope — the
/// walk stops at each market's `latest_complete_trading_day`.
///
/// One holdings load and one stored-date query per listing, then an in-memory
/// walk: six years of history per listing is thousands of dates, so a per-day
/// round trip is not an option (the same pre-loading pattern as `FxRates` and
/// `RenameHistory`).
///
/// Not on the read transaction its caller uses: `load_market` is pool-based,
/// and this check tolerates a concurrent write far better than a financial
/// aggregation would — a hole is a hole whichever snapshot it is seen in.
async fn db_unpriced_days(
    pool: &SqlitePool,
    now: DateTime<Utc>,
) -> Result<Vec<UnpricedListing>, sqlx::Error> {
    let timeline = HeldTimeline::load(pool).await?;
    let mut listings = Vec::new();
    for listing_id in timeline.listing_ids() {
        let Some(market) = closing_price::load_market(pool, listing_id).await? else {
            continue;
        };
        // A calendar so misconfigured it has no trading day in the past year
        // has nothing this check can say about it; the price-import job fails
        // loudly on the same listing.
        let Some(final_day) = market
            .latest_complete_trading_day(now)
            .map_err(sqlx::Error::Protocol)?
        else {
            continue;
        };
        let spans = timeline.held_spans(listing_id, final_day);
        if spans.is_empty() {
            continue;
        }
        // Every stored date, ok or errored: an errored day is *not* unpriced
        // — it is reported by `errored_prices`.
        let stored: HashSet<NaiveDate> =
            sqlx::query_scalar("SELECT price_date FROM closing_prices WHERE listing_id = ?")
                .bind(listing_id)
                .fetch_all(pool)
                .await?
                .into_iter()
                .collect();

        // Distinct valuation days, not calendar days: a weekend and the
        // Friday it values at are one hole, not three.
        let mut missing: BTreeSet<NaiveDate> = BTreeSet::new();
        for (from, to) in spans {
            let mut date = from;
            while date <= to {
                if let Some(valuation_day) = market.latest_trading_day_on_or_before(date)
                    && !stored.contains(&valuation_day)
                {
                    missing.insert(valuation_day);
                }
                date += Duration::days(1);
            }
        }
        let (Some(&earliest_date), Some(&latest_date)) = (missing.first(), missing.last()) else {
            continue;
        };
        listings.push(UnpricedListing {
            listing_id,
            ticker: market.listing.ticker.clone(),
            unpriced_days: missing.len() as i64,
            earliest_date,
            latest_date,
        });
    }
    // Oldest hole first: the least recoverable reads first.
    listings.sort_by_key(|row| (row.earliest_date, row.listing_id));
    Ok(listings)
}

/// Corporate actions sharing a (listing, type, date) — see [`DuplicateAction`].
///
/// Grouped in SQL and read on the caller's transaction: it is one small
/// aggregate over `corporate_actions`, not a per-listing walk like
/// [`db_unpriced_days`].
async fn db_duplicate_actions(
    conn: &mut sqlx::SqliteConnection,
) -> Result<Vec<DuplicateAction>, sqlx::Error> {
    let rows = sqlx::query_as::<_, DuplicateActionRow>(
        "SELECT ca.listing_id AS listing_id, l.ticker AS ticker, \
                ca.action_type AS action_type, ca.date AS date, \
                COUNT(*) AS action_count, GROUP_CONCAT(ca.id) AS action_ids \
         FROM corporate_actions ca JOIN listings l ON l.id = ca.listing_id \
         GROUP BY ca.listing_id, ca.action_type, ca.date \
         HAVING COUNT(*) > 1 \
         ORDER BY ca.date DESC, l.ticker, ca.action_type",
    )
    .fetch_all(&mut *conn)
    .await?;
    rows.into_iter()
        .map(|row| {
            // GROUP_CONCAT's order is unspecified, so sort rather than trust it.
            let mut action_ids = row
                .action_ids
                .split(',')
                .map(|id| {
                    id.parse::<i64>().map_err(|e| {
                        sqlx::Error::Decode(format!("corporate action id {id}: {e}").into())
                    })
                })
                .collect::<Result<Vec<_>, _>>()?;
            action_ids.sort_unstable();
            Ok(DuplicateAction {
                listing_id: row.listing_id,
                ticker: row.ticker,
                action_type: row.action_type,
                date: row.date,
                action_count: row.action_count,
                action_ids,
            })
        })
        .collect()
}

/// AMMA statements sharing a (listing, financial year, holding account) — see
/// [`DuplicateAmmaStatement`].
///
/// Grouped in SQL on the caller's transaction, like [`db_duplicate_actions`]:
/// one small aggregate over `amma_statements`. The year is grouped on
/// `tax_year_end_date` itself rather than a derived year, which is the same
/// thing — the column is a 30 June date, enforced at write time
/// (`entities::amma::UpsertError::NotFinancialYearEnd`).
async fn db_duplicate_amma_statements(
    conn: &mut sqlx::SqliteConnection,
) -> Result<Vec<DuplicateAmmaStatement>, sqlx::Error> {
    let rows = sqlx::query_as::<_, DuplicateAmmaStatementRow>(
        "SELECT a.listing_id AS listing_id, l.ticker AS ticker, \
                a.tax_year_end_date AS tax_year_end_date, \
                a.holding_account_id AS holding_account_id, \
                COUNT(*) AS statement_count, GROUP_CONCAT(a.id) AS statement_ids \
         FROM amma_statements a JOIN listings l ON l.id = a.listing_id \
         GROUP BY a.listing_id, a.tax_year_end_date, a.holding_account_id \
         HAVING COUNT(*) > 1 \
         ORDER BY a.tax_year_end_date DESC, l.ticker, a.holding_account_id",
    )
    .fetch_all(&mut *conn)
    .await?;
    rows.into_iter()
        .map(|row| {
            // GROUP_CONCAT's order is unspecified, so sort rather than trust it.
            let mut statement_ids = row
                .statement_ids
                .split(',')
                .map(|id| {
                    id.parse::<i64>().map_err(|e| {
                        sqlx::Error::Decode(format!("AMMA statement id {id}: {e}").into())
                    })
                })
                .collect::<Result<Vec<_>, _>>()?;
            statement_ids.sort_unstable();
            Ok(DuplicateAmmaStatement {
                listing_id: row.listing_id,
                ticker: row.ticker,
                tax_year: tax_year_for(row.tax_year_end_date),
                holding_account_id: row.holding_account_id,
                statement_count: row.statement_count,
                statement_ids,
            })
        })
        .collect()
}

/// True when two income rows are the same declared payment: same listing,
/// holding account and payment date, and identical money figures stated in one
/// currency — the fingerprint behind [`DuplicateIncome`].
///
/// Compared as `Decimal`s rather than as the stored TEXT, so `"10.0"` and
/// `"10.00"` — the same dollars written by two different clients — are the
/// match they are, which is also why the grouping cannot be pushed into SQL.
/// Every money column of the row is compared, including the informational
/// ones: two rows agreeing on the assessable figures but disagreeing on, say,
/// `tax_deferred_amount` were entered from different statements.
fn same_income_entry(a: &Income, b: &Income) -> bool {
    a.listing_id == b.listing_id
        && a.holding_account_id == b.holding_account_id
        && a.date_paid == b.date_paid
        && a.currency == b.currency
        && a.franked_amount == b.franked_amount
        && a.unfranked_amount == b.unfranked_amount
        && a.foreign_source_income == b.foreign_source_income
        && a.foreign_tax_paid == b.foreign_tax_paid
        && a.tfn_withholding_tax == b.tfn_withholding_tax
        && a.franking_credits == b.franking_credits
        && a.lic_capital_gain_amount == b.lic_capital_gain_amount
        && a.conduit_foreign_income == b.conduit_foreign_income
        && a.amount_per_security == b.amount_per_security
        && a.securities_held == b.securities_held
        && a.tax_deferred_amount == b.tax_deferred_amount
}

/// Income rows declaring one payment twice — see [`DuplicateIncome`].
///
/// Read on the caller's transaction, but grouped in Rust rather than in SQL:
/// the amounts are part of the key and they are TEXT decimals, which SQL would
/// compare as strings (see [`same_income_entry`]). Only rows that already
/// share a (listing, account, date) with another row are read, so what is
/// carried into memory is at most the handful of same-day pairs a portfolio
/// has, not the income table.
async fn db_duplicate_income(
    conn: &mut sqlx::SqliteConnection,
) -> Result<Vec<DuplicateIncome>, sqlx::Error> {
    let rows: Vec<Income> = sqlx::query_as(
        "SELECT * FROM income i \
         WHERE EXISTS (SELECT 1 FROM income o \
                       WHERE o.listing_id = i.listing_id \
                         AND o.holding_account_id = i.holding_account_id \
                         AND o.date_paid = i.date_paid AND o.id <> i.id) \
         ORDER BY i.id",
    )
    .fetch_all(&mut *conn)
    .await?;
    if rows.is_empty() {
        return Ok(Vec::new());
    }
    let tickers: HashMap<i64, String> =
        sqlx::query_as::<_, (i64, String)>("SELECT id, ticker FROM listings")
            .fetch_all(&mut *conn)
            .await?
            .into_iter()
            .collect();

    // Grouped by scanning: the rows here are already narrowed to same-day
    // clusters, so the quadratic worst case is a handful of comparisons.
    // Read in id order, so each group's ids come out ascending.
    let mut groups: Vec<Vec<Income>> = Vec::new();
    for row in rows {
        match groups
            .iter_mut()
            .find(|group| same_income_entry(&group[0], &row))
        {
            Some(group) => group.push(row),
            None => groups.push(vec![row]),
        }
    }
    let mut duplicates: Vec<DuplicateIncome> = groups
        .into_iter()
        .filter(|group| group.len() > 1)
        .map(|group| {
            let first = &group[0];
            DuplicateIncome {
                listing_id: first.listing_id,
                ticker: tickers
                    .get(&first.listing_id)
                    .cloned()
                    // The FK guarantees the listing exists; a row read between
                    // the two queries is the only way here, and an empty
                    // ticker still names the ids to open.
                    .unwrap_or_default(),
                holding_account_id: first.holding_account_id,
                date_paid: first.date_paid,
                currency: first.currency.clone(),
                gross_amount: first.gross_cash_income(),
                income_count: group.len() as i64,
                income_ids: group.iter().map(|row| row.id).collect(),
            }
        })
        .collect();
    // Newest first, matching the other two duplicate lists.
    duplicates.sort_by(|a, b| {
        b.date_paid
            .cmp(&a.date_paid)
            .then_with(|| a.ticker.cmp(&b.ticker))
            .then_with(|| a.holding_account_id.cmp(&b.holding_account_id))
            .then_with(|| a.income_ids.cmp(&b.income_ids))
    });
    Ok(duplicates)
}

/// Whether two interest rows are the same credit entered twice — see
/// [`DuplicateInterest`]. Every stored field except the id is compared: the
/// money figures as `Decimal`s rather than as the stored TEXT (so `"250.0"`
/// and `"250.00"` match), and `source` / `holding_account_id` as the payer
/// identity that interest has instead of a listing.
fn same_interest_entry(a: &InterestIncome, b: &InterestIncome) -> bool {
    a.date_paid == b.date_paid
        && a.currency == b.currency
        && a.amount == b.amount
        && a.tfn_withholding_tax == b.tfn_withholding_tax
        && a.foreign_source == b.foreign_source
        && a.foreign_tax_paid == b.foreign_tax_paid
        && a.source == b.source
        && a.holding_account_id == b.holding_account_id
}

/// Whether two expense rows are the same invoice entered twice — see
/// [`DuplicateExpense`]. Every stored field except the id is compared,
/// including the optional provenance pair: two rows agreeing on what is claimed
/// but disagreeing on the gross they were apportioned from came off different
/// invoices.
fn same_expense_entry(a: &InvestmentExpense, b: &InvestmentExpense) -> bool {
    a.date_incurred == b.date_incurred
        && a.expense_type == b.expense_type
        && a.currency == b.currency
        && a.amount == b.amount
        && a.gross_amount == b.gross_amount
        && a.deductible_percentage == b.deductible_percentage
        && a.description == b.description
        && a.listing_id == b.listing_id
        && a.holding_account_id == b.holding_account_id
}

/// Interest rows declaring one credit twice — see [`DuplicateInterest`].
///
/// Read on the caller's transaction and grouped in Rust for the same reason as
/// [`db_duplicate_income`]: the amounts are part of the key and they are TEXT
/// decimals, which SQL would compare as strings. Only rows sharing a date with
/// another row are read — the pre-narrowing is on the date alone, since the
/// rest of the key is nullable (`source`, `holding_account_id`) and would need
/// null-safe comparison in SQL to no benefit: same-day interest rows are a
/// handful even in a busy year.
async fn db_duplicate_interest(
    conn: &mut sqlx::SqliteConnection,
) -> Result<Vec<DuplicateInterest>, sqlx::Error> {
    let rows: Vec<InterestIncome> = sqlx::query_as(
        "SELECT * FROM interest_income i \
         WHERE EXISTS (SELECT 1 FROM interest_income o \
                       WHERE o.date_paid = i.date_paid AND o.id <> i.id) \
         ORDER BY i.id",
    )
    .fetch_all(&mut *conn)
    .await?;

    // Grouped by scanning, read in id order so each group's ids come out
    // ascending (the same shape as `db_duplicate_income`).
    let mut groups: Vec<Vec<InterestIncome>> = Vec::new();
    for row in rows {
        match groups
            .iter_mut()
            .find(|group| same_interest_entry(&group[0], &row))
        {
            Some(group) => group.push(row),
            None => groups.push(vec![row]),
        }
    }
    let mut duplicates: Vec<DuplicateInterest> = groups
        .into_iter()
        .filter(|group| group.len() > 1)
        .map(|group| {
            let first = &group[0];
            DuplicateInterest {
                date_paid: first.date_paid,
                currency: first.currency.clone(),
                amount: first.amount,
                source: first.source.clone(),
                holding_account_id: first.holding_account_id,
                interest_count: group.len() as i64,
                interest_ids: group.iter().map(|row| row.id).collect(),
            }
        })
        .collect();
    // Newest first, matching the other duplicate lists.
    duplicates.sort_by(|a, b| {
        b.date_paid
            .cmp(&a.date_paid)
            .then_with(|| a.source.cmp(&b.source))
            .then_with(|| a.interest_ids.cmp(&b.interest_ids))
    });
    Ok(duplicates)
}

/// Investment-expense rows claiming one expense twice — see
/// [`DuplicateExpense`]. Same shape as [`db_duplicate_interest`], plus the
/// ticker lookup for a listing-attributed row.
async fn db_duplicate_expenses(
    conn: &mut sqlx::SqliteConnection,
) -> Result<Vec<DuplicateExpense>, sqlx::Error> {
    let rows: Vec<InvestmentExpense> = sqlx::query_as(
        "SELECT * FROM investment_expenses e \
         WHERE EXISTS (SELECT 1 FROM investment_expenses o \
                       WHERE o.date_incurred = e.date_incurred AND o.id <> e.id) \
         ORDER BY e.id",
    )
    .fetch_all(&mut *conn)
    .await?;
    if rows.is_empty() {
        return Ok(Vec::new());
    }
    let tickers: HashMap<i64, String> =
        sqlx::query_as::<_, (i64, String)>("SELECT id, ticker FROM listings")
            .fetch_all(&mut *conn)
            .await?
            .into_iter()
            .collect();

    let mut groups: Vec<Vec<InvestmentExpense>> = Vec::new();
    for row in rows {
        match groups
            .iter_mut()
            .find(|group| same_expense_entry(&group[0], &row))
        {
            Some(group) => group.push(row),
            None => groups.push(vec![row]),
        }
    }
    let mut duplicates: Vec<DuplicateExpense> = groups
        .into_iter()
        .filter(|group| group.len() > 1)
        .map(|group| {
            let first = &group[0];
            DuplicateExpense {
                date_incurred: first.date_incurred,
                expense_type: first.expense_type,
                currency: first.currency.clone(),
                amount: first.amount,
                description: first.description.clone(),
                listing_id: first.listing_id,
                // The FK guarantees a set listing exists; a row read between
                // the two queries is the only way to miss it, and the ids are
                // named either way.
                ticker: first.listing_id.and_then(|id| tickers.get(&id).cloned()),
                holding_account_id: first.holding_account_id,
                expense_count: group.len() as i64,
                expense_ids: group.iter().map(|row| row.id).collect(),
            }
        })
        .collect();
    duplicates.sort_by(|a, b| {
        b.date_incurred
            .cmp(&a.date_incurred)
            .then_with(|| a.ticker.cmp(&b.ticker))
            .then_with(|| a.expense_ids.cmp(&b.expense_ids))
    });
    Ok(duplicates)
}

/// Whether two ESS statements are one vest entered twice — see
/// [`DuplicateEssStatement`]. Every stored field except the id is compared: the
/// money figures as `Decimal`s rather than as the stored TEXT (so `"1000.0"`
/// and `"1000.00"` match), the statement-AUD overrides included — two rows
/// agreeing on the foreign labels but disagreeing on the AUD the employer
/// stated for them came off different statements. `vest_trade_id` is derived,
/// not stored, and is deliberately not part of the key: whether the surplus
/// statement has been vested yet says nothing about whether it is a duplicate.
fn same_ess_entry(a: &EssStatement, b: &EssStatement) -> bool {
    a.listing_id == b.listing_id
        && a.holding_account_id == b.holding_account_id
        && a.taxing_point_date == b.taxing_point_date
        && a.currency == b.currency
        && a.quantity == b.quantity
        && a.market_value_per_share == b.market_value_per_share
        && a.taxed_upfront_eligible == b.taxed_upfront_eligible
        && a.taxed_upfront_not_eligible == b.taxed_upfront_not_eligible
        && a.deferral_discount == b.deferral_discount
        && a.pre_2009_cessation_discount == b.pre_2009_cessation_discount
        && a.foreign_source_discount == b.foreign_source_discount
        && a.tfn_withholding == b.tfn_withholding
        && a.fx_rate == b.fx_rate
        && a.aud_taxed_upfront_eligible == b.aud_taxed_upfront_eligible
        && a.aud_taxed_upfront_not_eligible == b.aud_taxed_upfront_not_eligible
        && a.aud_deferral_discount == b.aud_deferral_discount
        && a.aud_pre_2009_cessation_discount == b.aud_pre_2009_cessation_discount
        && a.aud_foreign_source_discount == b.aud_foreign_source_discount
}

/// ESS statements declaring one vest twice — see [`DuplicateEssStatement`].
///
/// Read on the caller's transaction and grouped in Rust for the same reason as
/// [`db_duplicate_income`]: the figures are part of the key and they are TEXT
/// decimals, which SQL would compare as strings. Only rows already sharing a
/// (listing, account, taxing point) with another row are read, so what is
/// carried into memory is at most the handful of same-day vests a plan has.
/// Selects the entity's own `COLUMNS` rather than `*`, since `vest_trade_id` is
/// a derived back-link the row mapping requires.
async fn db_duplicate_ess_statements(
    conn: &mut sqlx::SqliteConnection,
) -> Result<Vec<DuplicateEssStatement>, sqlx::Error> {
    let rows: Vec<EssStatement> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT {} FROM ess_statements \
         WHERE EXISTS (SELECT 1 FROM ess_statements o \
                       WHERE o.listing_id = ess_statements.listing_id \
                         AND o.holding_account_id = ess_statements.holding_account_id \
                         AND o.taxing_point_date = ess_statements.taxing_point_date \
                         AND o.id <> ess_statements.id) \
         ORDER BY ess_statements.id",
        ess_statement::COLUMNS
    )))
    .fetch_all(&mut *conn)
    .await?;
    if rows.is_empty() {
        return Ok(Vec::new());
    }
    let tickers: HashMap<i64, String> =
        sqlx::query_as::<_, (i64, String)>("SELECT id, ticker FROM listings")
            .fetch_all(&mut *conn)
            .await?
            .into_iter()
            .collect();

    // Grouped by scanning, read in id order so each group's ids come out
    // ascending (the same shape as `db_duplicate_income`).
    let mut groups: Vec<Vec<EssStatement>> = Vec::new();
    for row in rows {
        match groups
            .iter_mut()
            .find(|group| same_ess_entry(&group[0], &row))
        {
            Some(group) => group.push(row),
            None => groups.push(vec![row]),
        }
    }
    let mut duplicates: Vec<DuplicateEssStatement> = groups
        .into_iter()
        .filter(|group| group.len() > 1)
        .map(|group| {
            let first = &group[0];
            DuplicateEssStatement {
                listing_id: first.listing_id,
                ticker: tickers
                    .get(&first.listing_id)
                    .cloned()
                    // The FK guarantees the listing exists; a row read between
                    // the two queries is the only way here, and an empty
                    // ticker still names the ids to open.
                    .unwrap_or_default(),
                holding_account_id: first.holding_account_id,
                taxing_point_date: first.taxing_point_date,
                currency: first.currency.clone(),
                quantity: first.quantity,
                discount_total: ess_statement::discount_labels(first),
                statement_count: group.len() as i64,
                statement_ids: group.iter().map(|row| row.id).collect(),
            }
        })
        .collect();
    // Newest first, matching the other duplicate lists.
    duplicates.sort_by(|a, b| {
        b.taxing_point_date
            .cmp(&a.taxing_point_date)
            .then_with(|| a.ticker.cmp(&b.ticker))
            .then_with(|| a.holding_account_id.cmp(&b.holding_account_id))
            .then_with(|| a.statement_ids.cmp(&b.statement_ids))
    });
    Ok(duplicates)
}

/// Read the freshness facts on one snapshot. `today` and `now` are parameters
/// so tests can pin the staleness thresholds and the "close is final yet"
/// cut-off to fixed dates.
pub async fn db_health(
    pool: &SqlitePool,
    today: NaiveDate,
    now: DateTime<Utc>,
) -> Result<HealthReport, sqlx::Error> {
    let mut tx = pool.begin().await?;
    let latest_price_date: Option<NaiveDate> =
        sqlx::query_scalar("SELECT MAX(price_date) FROM closing_prices WHERE status = 'ok'")
            .fetch_one(&mut *tx)
            .await?;
    let latest_fx_month: Option<String> = sqlx::query_scalar("SELECT MAX(month) FROM rba_fx_rates")
        .fetch_one(&mut *tx)
        .await?;
    let failed_jobs = sqlx::query_as::<_, FailedJob>(
        "SELECT name, finished_at, error FROM job_runs r \
         WHERE id = (SELECT MAX(id) FROM job_runs WHERE name = r.name) AND success = 0 \
         ORDER BY name",
    )
    .fetch_all(&mut *tx)
    .await?;
    let errored_prices = sqlx::query_as::<_, ErroredPriceListing>(
        "SELECT cp.listing_id AS listing_id, l.ticker AS ticker, \
                COUNT(*) AS errored_days, MAX(cp.price_date) AS latest_errored_date, \
                (SELECT cp2.error FROM closing_prices cp2 \
                 WHERE cp2.listing_id = cp.listing_id AND cp2.status = 'error' \
                 ORDER BY cp2.price_date DESC LIMIT 1) AS latest_error \
         FROM closing_prices cp JOIN listings l ON l.id = cp.listing_id \
         WHERE cp.status = 'error' \
         GROUP BY cp.listing_id \
         ORDER BY latest_errored_date DESC",
    )
    .fetch_all(&mut *tx)
    .await?;
    let duplicate_actions = db_duplicate_actions(&mut tx).await?;
    let duplicate_amma_statements = db_duplicate_amma_statements(&mut tx).await?;
    let duplicate_income = db_duplicate_income(&mut tx).await?;
    let duplicate_interest = db_duplicate_interest(&mut tx).await?;
    let duplicate_expenses = db_duplicate_expenses(&mut tx).await?;
    let duplicate_ess_statements = db_duplicate_ess_statements(&mut tx).await?;
    tx.commit().await?;
    let unpriced_days = db_unpriced_days(pool, now).await?;

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
        errored_prices,
        unpriced_days,
        duplicate_actions,
        duplicate_amma_statements,
        duplicate_income,
        duplicate_interest,
        duplicate_expenses,
        duplicate_ess_statements,
    })
}

async fn report(State(pool): State<SqlitePool>) -> Result<Json<HealthReport>, ApiError> {
    let today = chrono::Local::now().date_naive();
    db_health(&pool, today, Utc::now())
        .await
        .map(Json)
        .map_err(ApiError::from)
}

pub fn router() -> Router<SqlitePool> {
    Router::new().route("/reports/health", get(report))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::entities::corporate_action;
    use crate::test_support::{self, ApiClient, dec, test_pool, ymd};
    use axum::http::StatusCode;

    /// The report as at `today`, read late enough in the day (22:00 Sydney /
    /// noon UTC) that `today`'s ASX close is final. Tests that care about the
    /// "not final yet" boundary call `db_health` directly with their own
    /// `now`.
    async fn health(pool: &SqlitePool, today: NaiveDate) -> HealthReport {
        db_health(pool, today, noon_utc(today)).await.unwrap()
    }

    fn noon_utc(date: NaiveDate) -> DateTime<Utc> {
        date.and_hms_opt(12, 0, 0).expect("valid time").and_utc()
    }

    async fn insert_ok_price(pool: &SqlitePool, listing_id: i64, date: &str) {
        test_support::closing_price(listing_id, date.parse().unwrap())
            .price("10.50")
            .source("yahoo")
            .fetched_at("2026-07-01T00:00:00Z")
            .insert(pool)
            .await;
    }

    async fn insert_error_price(pool: &SqlitePool, listing_id: i64, date: &str, error: &str) {
        test_support::closing_price(listing_id, date.parse().unwrap())
            .source("yahoo")
            .fetched_at("2026-07-01T00:00:00Z")
            .errored(error)
            .insert(pool)
            .await;
    }

    async fn insert_fx_month(pool: &SqlitePool, month: &str) {
        sqlx::query("INSERT INTO rba_fx_rates (currency, month, rate) VALUES ('USD', ?, '0.66')")
            .bind(month)
            .execute(pool)
            .await
            .unwrap();
    }

    async fn insert_job_run(pool: &SqlitePool, name: &str, finished_at: &str, error: Option<&str>) {
        sqlx::query(
            "INSERT INTO job_runs (name, started_at, finished_at, success, error) \
             VALUES (?, ?, ?, ?, ?)",
        )
        .bind(name)
        .bind(finished_at)
        .bind(finished_at)
        .bind(error.is_none())
        .bind(error)
        .execute(pool)
        .await
        .unwrap();
    }

    #[test]
    fn business_days_skip_weekends() {
        // Fri 2026-07-10 → Mon 2026-07-13 is one business day…
        assert_eq!(business_days_since(ymd(2026, 7, 10), ymd(2026, 7, 13)), 1);
        // …Mon → Fri of the same week is four…
        assert_eq!(business_days_since(ymd(2026, 7, 6), ymd(2026, 7, 10)), 4);
        // …and the same day (or a future `from`) is zero.
        assert_eq!(business_days_since(ymd(2026, 7, 13), ymd(2026, 7, 13)), 0);
        assert_eq!(business_days_since(ymd(2026, 7, 14), ymd(2026, 7, 13)), 0);
    }

    #[test]
    fn previous_month_wraps_the_year() {
        assert_eq!(previous_month(ymd(2026, 7, 13)), "2026-06");
        assert_eq!(previous_month(ymd(2026, 1, 5)), "2025-12");
    }

    #[tokio::test]
    async fn empty_database_reports_nothing_stale() {
        // A fresh install has nothing to have gone stale: no banner. A price
        // import that breaks before ever succeeding shows via failed_jobs.
        let pool = test_pool().await;
        let h = health(&pool, ymd(2026, 7, 13)).await;
        assert_eq!(h.latest_price_date, None);
        assert!(!h.prices_stale);
        assert_eq!(h.latest_fx_month, None);
        assert!(!h.fx_stale);
        assert!(h.failed_jobs.is_empty());
        assert!(h.errored_prices.is_empty());
        assert!(h.unpriced_days.is_empty());
        assert!(h.duplicate_actions.is_empty());
        assert!(h.duplicate_amma_statements.is_empty());
        assert!(h.duplicate_income.is_empty());
        assert!(h.duplicate_interest.is_empty());
        assert!(h.duplicate_expenses.is_empty());
        assert!(h.duplicate_ess_statements.is_empty());
    }

    #[tokio::test]
    async fn prices_within_threshold_are_fresh_older_are_stale() {
        let pool = test_pool().await;
        test_support::listing(1).insert(&pool).await;
        // Wed 2026-07-08 → Mon 2026-07-13 is exactly 3 business days: fresh.
        insert_ok_price(&pool, 1, "2026-07-08").await;
        let h = health(&pool, ymd(2026, 7, 13)).await;
        assert_eq!(h.latest_price_date, Some(ymd(2026, 7, 8)));
        assert!(!h.prices_stale);

        // One business day further out (Tue 2026-07-14) crosses the threshold.
        let h = health(&pool, ymd(2026, 7, 14)).await;
        assert!(h.prices_stale);
    }

    #[tokio::test]
    async fn only_ok_prices_count_towards_freshness() {
        // An errored fetch stores a row but no usable price — a run of errored
        // days must not make the data look fresh.
        let pool = test_pool().await;
        test_support::listing(1).insert(&pool).await;
        insert_ok_price(&pool, 1, "2026-07-01").await;
        insert_error_price(&pool, 1, "2026-07-10", "provider down").await;
        let h = health(&pool, ymd(2026, 7, 13)).await;
        assert_eq!(h.latest_price_date, Some(ymd(2026, 7, 1)));
        assert!(h.prices_stale);
    }

    /// A listing with errored closing-price rows is surfaced by ticker (not
    /// raw id), with the count and the most recent error message — the
    /// surface that stops a stuck symbol (renamed/delisted) from only
    /// showing up indirectly as a missing snapshot.
    #[tokio::test]
    async fn errored_price_listing_is_surfaced_with_ticker_count_and_latest_error() {
        let pool = test_pool().await;
        test_support::listing(1).ticker("LAR").insert(&pool).await;
        insert_error_price(
            &pool,
            1,
            "2026-07-01",
            "provider returned no candles for LAR",
        )
        .await;
        insert_error_price(
            &pool,
            1,
            "2026-07-02",
            "provider returned no candles for LAR",
        )
        .await;
        insert_error_price(
            &pool,
            1,
            "2026-07-03",
            "provider returned no candles for LAR (latest)",
        )
        .await;

        let h = health(&pool, ymd(2026, 7, 13)).await;
        assert_eq!(h.errored_prices.len(), 1);
        let row = &h.errored_prices[0];
        assert_eq!(row.listing_id, 1);
        assert_eq!(row.ticker, "LAR");
        assert_eq!(row.errored_days, 3);
        assert_eq!(row.latest_errored_date, ymd(2026, 7, 3));
        assert_eq!(
            row.latest_error,
            "provider returned no candles for LAR (latest)"
        );
    }

    /// Multiple affected listings are each their own row, newest error first
    /// — and an ok price for the same listing doesn't hide its errors.
    #[tokio::test]
    async fn errored_prices_are_grouped_per_listing_and_ordered_newest_first() {
        let pool = test_pool().await;
        test_support::listing(1).ticker("A").insert(&pool).await;
        test_support::listing(2).ticker("B").insert(&pool).await;
        insert_ok_price(&pool, 1, "2026-07-01").await;
        insert_error_price(&pool, 1, "2026-07-05", "err A").await;
        insert_error_price(&pool, 2, "2026-07-10", "err B").await;

        let h = health(&pool, ymd(2026, 7, 13)).await;
        assert_eq!(h.errored_prices.len(), 2);
        assert_eq!(h.errored_prices[0].ticker, "B"); // newest error first
        assert_eq!(h.errored_prices[1].ticker, "A");
    }

    #[tokio::test]
    async fn fx_fresh_with_previous_month_stale_when_older() {
        let pool = test_pool().await;
        // June is the month before July 2026: fresh.
        insert_fx_month(&pool, "2026-06").await;
        let h = health(&pool, ymd(2026, 7, 13)).await;
        assert_eq!(h.latest_fx_month.as_deref(), Some("2026-06"));
        assert!(!h.fx_stale);

        // Come September with nothing newer imported, June is stale.
        let h = health(&pool, ymd(2026, 9, 1)).await;
        assert!(h.fx_stale);
    }

    #[tokio::test]
    async fn fx_current_month_is_fresh() {
        let pool = test_pool().await;
        insert_fx_month(&pool, "2026-07").await;
        let h = health(&pool, ymd(2026, 7, 13)).await;
        assert!(!h.fx_stale);
    }

    #[tokio::test]
    async fn job_whose_latest_run_failed_is_surfaced() {
        let pool = test_pool().await;
        insert_job_run(
            &pool,
            "price-import",
            "2026-07-12T07:00:00Z",
            Some("yahoo 403"),
        )
        .await;
        let h = health(&pool, ymd(2026, 7, 13)).await;
        assert_eq!(h.failed_jobs.len(), 1);
        assert_eq!(h.failed_jobs[0].name, "price-import");
        assert_eq!(h.failed_jobs[0].error.as_deref(), Some("yahoo 403"));
    }

    #[tokio::test]
    async fn job_that_recovered_is_not_surfaced() {
        // Only the *latest* run per job counts: a failure followed by a
        // success is recovered, not failing.
        let pool = test_pool().await;
        insert_job_run(
            &pool,
            "price-import",
            "2026-07-12T07:00:00Z",
            Some("yahoo 403"),
        )
        .await;
        insert_job_run(&pool, "price-import", "2026-07-13T07:00:00Z", None).await;
        // And the reverse — success then failure — is failing.
        insert_job_run(&pool, "backup", "2026-07-12T00:00:00Z", None).await;
        insert_job_run(&pool, "backup", "2026-07-13T00:00:00Z", Some("disk full")).await;

        let h = health(&pool, ymd(2026, 7, 13)).await;
        assert_eq!(h.failed_jobs.len(), 1);
        assert_eq!(h.failed_jobs[0].name, "backup");
    }

    /// The case the errored list cannot catch: a held day nobody ever
    /// fetched, so there is no row to find. Wed 2026-07-08 is a trading day
    /// inside the held span with no stored row.
    #[tokio::test]
    async fn a_held_day_with_no_stored_row_is_reported() {
        let pool = test_pool().await;
        test_support::listing(1).ticker("BHP").insert(&pool).await;
        test_support::buy(1, 1)
            .date(ymd(2026, 7, 6))
            .insert(&pool)
            .await;
        for day in [
            "2026-07-06",
            "2026-07-07",
            "2026-07-09",
            "2026-07-10",
            "2026-07-13",
        ] {
            insert_ok_price(&pool, 1, day).await;
        }

        let h = health(&pool, ymd(2026, 7, 13)).await;
        assert_eq!(h.unpriced_days.len(), 1);
        let row = &h.unpriced_days[0];
        assert_eq!(row.listing_id, 1);
        assert_eq!(row.ticker, "BHP");
        assert_eq!(row.unpriced_days, 1);
        assert_eq!(row.earliest_date, ymd(2026, 7, 8));
        assert_eq!(row.latest_date, ymd(2026, 7, 8));
    }

    /// The two lists partition the problem: a day whose fetch failed has a
    /// row, so it is `errored_prices`' to report, never `unpriced_days`'.
    #[tokio::test]
    async fn an_errored_day_is_not_reported_as_unpriced() {
        let pool = test_pool().await;
        test_support::listing(1).ticker("BHP").insert(&pool).await;
        test_support::buy(1, 1)
            .date(ymd(2026, 7, 6))
            .insert(&pool)
            .await;
        for day in [
            "2026-07-06",
            "2026-07-07",
            "2026-07-09",
            "2026-07-10",
            "2026-07-13",
        ] {
            insert_ok_price(&pool, 1, day).await;
        }
        insert_error_price(&pool, 1, "2026-07-08", "provider down").await;

        let h = health(&pool, ymd(2026, 7, 13)).await;
        assert!(h.unpriced_days.is_empty());
        assert_eq!(h.errored_prices.len(), 1);
    }

    /// A day the market was shut is not a hole: the weekend and the ASX's
    /// King's Birthday (Mon 2026-06-08) all value at Fri 2026-06-05, which is
    /// priced.
    #[tokio::test]
    async fn non_trading_days_are_not_unpriced() {
        let pool = test_pool().await;
        test_support::listing(1).insert(&pool).await;
        test_support::buy(1, 1)
            .date(ymd(2026, 6, 5))
            .insert(&pool)
            .await;
        for day in ["2026-06-05", "2026-06-09", "2026-06-10"] {
            insert_ok_price(&pool, 1, day).await;
        }

        let h = health(&pool, ymd(2026, 6, 10)).await;
        assert!(h.unpriced_days.is_empty());
    }

    /// Today's close is not final until the exchange closes, so the day the
    /// price-import job has yet to collect is not reported as a hole — it
    /// becomes one only once the close has passed and nothing was stored.
    #[tokio::test]
    async fn a_close_that_is_not_final_yet_is_not_unpriced() {
        let pool = test_pool().await;
        test_support::listing(1).insert(&pool).await;
        test_support::buy(1, 1)
            .date(ymd(2026, 7, 6))
            .insert(&pool)
            .await;
        for day in [
            "2026-07-06",
            "2026-07-07",
            "2026-07-08",
            "2026-07-09",
            "2026-07-10",
        ] {
            insert_ok_price(&pool, 1, day).await;
        }

        // 11:00 Sydney on Mon 2026-07-13: the ASX has not closed yet.
        let before_close = "2026-07-13T01:00:00Z".parse::<DateTime<Utc>>().unwrap();
        let h = db_health(&pool, ymd(2026, 7, 13), before_close)
            .await
            .unwrap();
        assert!(h.unpriced_days.is_empty());

        // After the close, the still-unstored day is a hole.
        let h = health(&pool, ymd(2026, 7, 13)).await;
        assert_eq!(h.unpriced_days.len(), 1);
        assert_eq!(h.unpriced_days[0].latest_date, ymd(2026, 7, 13));
    }

    /// Nothing is held after the last unit is sold, so the span ends there —
    /// a listing sold out of the portfolio must not report every day since as
    /// a hole.
    #[tokio::test]
    async fn a_fully_sold_listing_is_not_reported_after_its_sale() {
        let pool = test_pool().await;
        test_support::listing(1).insert(&pool).await;
        test_support::buy(1, 1)
            .date(ymd(2026, 7, 6))
            .insert(&pool)
            .await;
        test_support::sell(2, 1)
            .date(ymd(2026, 7, 8))
            .insert(&pool)
            .await;
        test_support::allocate(&pool, 1, 2, 1, dec("100")).await;

        // Nothing was ever priced, so the whole held span is a hole: Mon
        // 2026-07-06 and Tue 07-07, and nothing from the sale date onward.
        let h = health(&pool, ymd(2026, 7, 13)).await;
        assert_eq!(h.unpriced_days.len(), 1);
        assert_eq!(h.unpriced_days[0].unpriced_days, 2);
        assert_eq!(h.unpriced_days[0].earliest_date, ymd(2026, 7, 6));
        assert_eq!(h.unpriced_days[0].latest_date, ymd(2026, 7, 7));
    }

    /// A hole spanning a ticker/exchange change is walked on the calendar
    /// that was in force at each date: the ASX's King's Birthday (Mon
    /// 2026-06-08) is not a trading day before the move to the NYSE, whose
    /// calendar has no such holiday, so it is not its own hole.
    #[tokio::test]
    async fn a_hole_straddling_a_rename_uses_the_calendar_of_the_date() {
        let pool = test_pool().await;
        test_support::listing(1).ticker("OLD").insert(&pool).await;
        test_support::buy(1, 1)
            .date(ymd(2026, 6, 5))
            .insert(&pool)
            .await;
        crate::entities::listing_rename::db_rename(
            &pool,
            1,
            &crate::entities::listing_rename::RenameBody {
                effective_date: ymd(2026, 6, 10),
                ticker: "NEW".to_string(),
                exchange_mic: Some("XNYS".to_string()),
                name: None,
                price_symbol: None,
                note: None,
            },
        )
        .await
        .unwrap();

        // 17:00 New York on Fri 2026-06-12: that day's close is final.
        let now = "2026-06-12T21:00:00Z".parse::<DateTime<Utc>>().unwrap();
        let h = db_health(&pool, ymd(2026, 6, 12), now).await.unwrap();
        assert_eq!(h.unpriced_days.len(), 1);
        let row = &h.unpriced_days[0];
        assert_eq!(row.ticker, "NEW");
        // Fri 06-05, Tue 06-09, then 06-10..06-12 under the NYSE calendar —
        // the ASX holiday of Mon 06-08 values at Fri 06-05 and is not a sixth.
        assert_eq!(row.unpriced_days, 5);
        assert_eq!(row.earliest_date, ymd(2026, 6, 5));
        assert_eq!(row.latest_date, ymd(2026, 6, 12));
    }

    #[tokio::test]
    async fn a_fully_priced_database_reports_no_unpriced_days() {
        let pool = test_pool().await;
        test_support::listing(1).insert(&pool).await;
        test_support::buy(1, 1)
            .date(ymd(2026, 7, 6))
            .insert(&pool)
            .await;
        for day in [
            "2026-07-06",
            "2026-07-07",
            "2026-07-08",
            "2026-07-09",
            "2026-07-10",
            "2026-07-13",
        ] {
            insert_ok_price(&pool, 1, day).await;
        }

        let h = health(&pool, ymd(2026, 7, 13)).await;
        assert!(h.unpriced_days.is_empty());
    }

    /// Oldest hole first: the further back it goes the less likely the
    /// provider will still serve it, so it is the one to act on.
    #[tokio::test]
    async fn unpriced_listings_are_ordered_oldest_hole_first() {
        let pool = test_pool().await;
        test_support::listing(1)
            .ticker("RECENT")
            .insert(&pool)
            .await;
        test_support::listing(2).ticker("OLD").insert(&pool).await;
        test_support::buy(1, 1)
            .date(ymd(2026, 7, 6))
            .insert(&pool)
            .await;
        test_support::buy(2, 2)
            .date(ymd(2026, 7, 2))
            .insert(&pool)
            .await;

        let h = health(&pool, ymd(2026, 7, 13)).await;
        assert_eq!(h.unpriced_days.len(), 2);
        assert_eq!(h.unpriced_days[0].ticker, "OLD");
        assert_eq!(h.unpriced_days[0].earliest_date, ymd(2026, 7, 2));
        assert_eq!(h.unpriced_days[1].ticker, "RECENT");
    }

    async fn insert_roc(pool: &SqlitePool, id: i64, listing_id: i64, date: NaiveDate) {
        insert_action(
            pool,
            id,
            listing_id,
            date,
            corporate_action::ActionKind::ReturnOfCapital {
                amount_per_unit: dec("0.50"),
                currency: "AUD".to_string(),
                record_date: None,
            },
        )
        .await;
    }

    async fn amit_listing(pool: &SqlitePool, id: i64, ticker: &str) {
        test_support::listing(id)
            .ticker(ticker)
            .amit(true)
            .insert(pool)
            .await;
    }

    /// An AMMA statement for `year_end`, in `holding_account_id`. Only the
    /// grouping key matters here, so the amounts stay at the fixture's zeros.
    async fn insert_amma(
        pool: &SqlitePool,
        id: i64,
        listing_id: i64,
        year_end: NaiveDate,
        holding_account_id: i64,
    ) {
        test_support::amma(id, listing_id)
            .with(|a| {
                a.tax_year_end_date = year_end;
                a.date_received = year_end + Duration::days(60);
                a.holding_account_id = holding_account_id;
            })
            .insert(pool)
            .await;
    }

    async fn insert_split(pool: &SqlitePool, id: i64, listing_id: i64, date: NaiveDate) {
        insert_action(
            pool,
            id,
            listing_id,
            date,
            corporate_action::ActionKind::ShareSplit {
                split_new_units: dec("2"),
                split_old_units: dec("1"),
            },
        )
        .await;
    }

    async fn insert_action(
        pool: &SqlitePool,
        id: i64,
        listing_id: i64,
        date: NaiveDate,
        kind: corporate_action::ActionKind,
    ) {
        corporate_action::db_upsert(
            pool,
            &corporate_action::CorporateAction {
                id,
                listing_id,
                date,
                kind,
            },
        )
        .await
        .unwrap();
    }

    /// SCENARIOS E-03 / E-15: a re-submitted form or a re-imported statement
    /// leaves two identical actions, and the cost-base pipeline reads them as
    /// two events — the return of capital reduces twice, the split multiplies
    /// twice. Nothing rejects the pair (a genuine same-day pair exists in
    /// principle), so health names it, with the ids to delete from.
    #[tokio::test]
    async fn duplicated_corporate_actions_are_reported_with_their_ids() {
        let pool = test_pool().await;
        test_support::listing(1).ticker("ROCC").insert(&pool).await;
        test_support::listing(2).ticker("SPLT").insert(&pool).await;
        insert_roc(&pool, 1, 1, ymd(2026, 3, 10)).await;
        insert_roc(&pool, 2, 1, ymd(2026, 3, 10)).await;
        insert_split(&pool, 3, 2, ymd(2026, 6, 1)).await;
        insert_split(&pool, 4, 2, ymd(2026, 6, 1)).await;

        let h = health(&pool, ymd(2026, 7, 13)).await;
        // Newest first: the split (June) before the capital return (March).
        assert_eq!(h.duplicate_actions.len(), 2);
        let split = &h.duplicate_actions[0];
        assert_eq!(split.ticker, "SPLT");
        assert_eq!(split.listing_id, 2);
        assert_eq!(split.action_type, "ShareSplit");
        assert_eq!(split.date, ymd(2026, 6, 1));
        assert_eq!(split.action_count, 2);
        assert_eq!(split.action_ids, vec![3, 4]);
        let roc = &h.duplicate_actions[1];
        assert_eq!(roc.ticker, "ROCC");
        assert_eq!(roc.action_type, "ReturnOfCapital");
        assert_eq!(roc.date, ymd(2026, 3, 10));
        assert_eq!(roc.action_ids, vec![1, 2]);
    }

    /// The warning is per (listing, action type, date): actions that differ in
    /// any of the three are ordinary independent events, however close
    /// together they fall.
    #[tokio::test]
    async fn actions_differing_in_listing_type_or_date_are_not_duplicates() {
        let pool = test_pool().await;
        test_support::listing(1).ticker("AAA").insert(&pool).await;
        test_support::listing(2).ticker("BBB").insert(&pool).await;
        // Same type and date, different listing.
        insert_roc(&pool, 1, 1, ymd(2026, 3, 10)).await;
        insert_roc(&pool, 2, 2, ymd(2026, 3, 10)).await;
        // Same listing and type, different date.
        insert_roc(&pool, 3, 1, ymd(2026, 9, 10)).await;
        // Same listing and date, different type.
        insert_split(&pool, 4, 1, ymd(2026, 3, 10)).await;

        let h = health(&pool, ymd(2026, 7, 13)).await;
        assert!(h.duplicate_actions.is_empty());
    }

    /// Three of a kind is one row, not three: the report answers "this
    /// (listing, type, date) is entered N times", listing every id.
    #[tokio::test]
    async fn three_identical_actions_are_one_row_counting_three() {
        let pool = test_pool().await;
        test_support::listing(1).ticker("TRIP").insert(&pool).await;
        insert_roc(&pool, 7, 1, ymd(2026, 3, 10)).await;
        insert_roc(&pool, 8, 1, ymd(2026, 3, 10)).await;
        insert_roc(&pool, 9, 1, ymd(2026, 3, 10)).await;

        let h = health(&pool, ymd(2026, 7, 13)).await;
        assert_eq!(h.duplicate_actions.len(), 1);
        assert_eq!(h.duplicate_actions[0].action_count, 3);
        assert_eq!(h.duplicate_actions[0].action_ids, vec![7, 8, 9]);
    }

    /// SCENARIOS F-06: an amended AMMA statement entered as a second row
    /// instead of over the original. Both are counted by every reader, so the
    /// pair is named — newest year first, with the ids ascending.
    #[tokio::test]
    async fn duplicated_amma_statements_are_reported_with_their_ids() {
        let pool = test_pool().await;
        amit_listing(&pool, 1, "VDHG").await;
        amit_listing(&pool, 2, "HNDQ").await;
        // VDHG FY2025: the original and its amendment.
        insert_amma(&pool, 1, 1, ymd(2025, 6, 30), 1).await;
        insert_amma(&pool, 2, 1, ymd(2025, 6, 30), 1).await;
        // HNDQ FY2024: the same mistake a year earlier.
        insert_amma(&pool, 3, 2, ymd(2024, 6, 30), 1).await;
        insert_amma(&pool, 4, 2, ymd(2024, 6, 30), 1).await;

        let h = health(&pool, ymd(2026, 7, 13)).await;
        assert_eq!(h.duplicate_amma_statements.len(), 2);
        let fy25 = &h.duplicate_amma_statements[0];
        assert_eq!(fy25.ticker, "VDHG");
        assert_eq!(fy25.listing_id, 1);
        assert_eq!(fy25.tax_year, 2025);
        assert_eq!(fy25.holding_account_id, 1);
        assert_eq!(fy25.statement_count, 2);
        assert_eq!(fy25.statement_ids, vec![1, 2]);
        let fy24 = &h.duplicate_amma_statements[1];
        assert_eq!(fy24.ticker, "HNDQ");
        assert_eq!(fy24.tax_year, 2024);
        assert_eq!(fy24.statement_ids, vec![3, 4]);
    }

    /// The key is all three parts. Two accounts of one fund-year are the
    /// legitimate case the warning must stay silent on (SCENARIOS F-03: a
    /// registry issues one statement per holder account), and two years or
    /// two listings are ordinary entry.
    #[tokio::test]
    async fn statements_differing_in_listing_year_or_account_are_not_duplicates() {
        let pool = test_pool().await;
        amit_listing(&pool, 1, "VDHG").await;
        amit_listing(&pool, 2, "HNDQ").await;
        crate::entities::holding_account::db_upsert(
            &pool,
            &crate::entities::holding_account::HoldingAccount {
                id: 2,
                name: "Second".to_string(),
            },
        )
        .await
        .unwrap();
        // Same listing and year, different holding account.
        insert_amma(&pool, 1, 1, ymd(2025, 6, 30), 1).await;
        insert_amma(&pool, 2, 1, ymd(2025, 6, 30), 2).await;
        // Same listing and account, different year.
        insert_amma(&pool, 3, 1, ymd(2024, 6, 30), 1).await;
        // Same year and account, different listing.
        insert_amma(&pool, 4, 2, ymd(2025, 6, 30), 1).await;

        let h = health(&pool, ymd(2026, 7, 13)).await;
        assert!(h.duplicate_amma_statements.is_empty());
    }

    /// Three of a kind is one row counting three, as on the actions side.
    #[tokio::test]
    async fn three_statements_for_one_fund_year_are_one_row_counting_three() {
        let pool = test_pool().await;
        amit_listing(&pool, 1, "VDHG").await;
        insert_amma(&pool, 7, 1, ymd(2025, 6, 30), 1).await;
        insert_amma(&pool, 8, 1, ymd(2025, 6, 30), 1).await;
        insert_amma(&pool, 9, 1, ymd(2025, 6, 30), 1).await;

        let h = health(&pool, ymd(2026, 7, 13)).await;
        assert_eq!(h.duplicate_amma_statements.len(), 1);
        assert_eq!(h.duplicate_amma_statements[0].statement_count, 3);
        assert_eq!(h.duplicate_amma_statements[0].statement_ids, vec![7, 8, 9]);
    }

    /// A fully franked dividend of `franked` gross, paid on `date` into
    /// `account` — the shape a duplicated statement entry takes.
    async fn insert_dividend(
        pool: &SqlitePool,
        id: i64,
        listing_id: i64,
        date: NaiveDate,
        account: i64,
        franked: &str,
    ) {
        insert_dividend_amount(pool, id, listing_id, date, account, dec(franked)).await;
    }

    /// As [`insert_dividend`], taking the franked amount as a `Decimal` so a
    /// test can control its scale (`10.0` vs `10.00`).
    async fn insert_dividend_amount(
        pool: &SqlitePool,
        id: i64,
        listing_id: i64,
        date: NaiveDate,
        account: i64,
        franked: Decimal,
    ) {
        test_support::income(id, listing_id, date)
            .with(|i| {
                i.holding_account_id = account;
                i.franked_amount = franked;
            })
            .insert(pool)
            .await;
    }

    /// SCENARIOS G-24: a re-submitted form or a re-imported statement leaves
    /// two identical dividends, and every reader counts both — twice the
    /// dividend income, twice the franking credits. Nothing rejects the pair
    /// (two dividends from one company on one day are legitimate in
    /// principle), so health names it, with the ids to delete from.
    #[tokio::test]
    async fn duplicated_income_rows_are_reported_with_their_ids() {
        let pool = test_pool().await;
        test_support::listing(1).ticker("DIVA").insert(&pool).await;
        test_support::listing(2).ticker("DIVB").insert(&pool).await;
        insert_dividend(&pool, 1, 1, ymd(2026, 3, 10), 1, "70").await;
        insert_dividend(&pool, 2, 1, ymd(2026, 3, 10), 1, "70").await;
        insert_dividend(&pool, 3, 2, ymd(2026, 6, 1), 1, "42.50").await;
        insert_dividend(&pool, 4, 2, ymd(2026, 6, 1), 1, "42.50").await;

        let h = health(&pool, ymd(2026, 7, 13)).await;
        // Newest first: June before March, as on the other two lists.
        assert_eq!(h.duplicate_income.len(), 2);
        let june = &h.duplicate_income[0];
        assert_eq!(june.ticker, "DIVB");
        assert_eq!(june.listing_id, 2);
        assert_eq!(june.holding_account_id, 1);
        assert_eq!(june.date_paid, ymd(2026, 6, 1));
        assert_eq!(june.currency, "AUD");
        assert_eq!(june.gross_amount, dec("42.50"));
        assert_eq!(june.income_count, 2);
        assert_eq!(june.income_ids, vec![3, 4]);
        let march = &h.duplicate_income[1];
        assert_eq!(march.ticker, "DIVA");
        assert_eq!(march.date_paid, ymd(2026, 3, 10));
        assert_eq!(march.gross_amount, dec("70"));
        assert_eq!(march.income_ids, vec![1, 2]);
    }

    /// The key is all four parts. Two payments differing in the amount are the
    /// legitimate case the warning must stay silent on — an ordinary and a
    /// special dividend paid the same day — and a different listing, account
    /// or date is ordinary entry.
    #[tokio::test]
    async fn income_differing_in_listing_account_date_or_amount_is_not_a_duplicate() {
        let pool = test_pool().await;
        test_support::listing(1).ticker("AAA").insert(&pool).await;
        test_support::listing(2).ticker("BBB").insert(&pool).await;
        crate::entities::holding_account::db_upsert(
            &pool,
            &crate::entities::holding_account::HoldingAccount {
                id: 2,
                name: "Second".to_string(),
            },
        )
        .await
        .unwrap();
        // Same account, date and amount, different listing.
        insert_dividend(&pool, 1, 1, ymd(2026, 3, 10), 1, "70").await;
        insert_dividend(&pool, 2, 2, ymd(2026, 3, 10), 1, "70").await;
        // Same listing, date and amount, different holding account.
        insert_dividend(&pool, 3, 1, ymd(2026, 3, 10), 2, "70").await;
        // Same listing, account and amount, different date.
        insert_dividend(&pool, 4, 1, ymd(2026, 9, 10), 1, "70").await;
        // Same listing, account and date, different amount: the ordinary +
        // special pair.
        insert_dividend(&pool, 5, 1, ymd(2026, 3, 10), 1, "12.34").await;

        let h = health(&pool, ymd(2026, 7, 13)).await;
        assert!(h.duplicate_income.is_empty());
    }

    /// The amounts are compared as decimals, not as the TEXT they are stored
    /// as: `70.0` and `70.00` are the same dollars entered twice, however the
    /// two clients wrote them.
    #[tokio::test]
    async fn amounts_equal_in_value_but_not_in_text_are_still_duplicates() {
        let pool = test_pool().await;
        test_support::listing(1).ticker("DIVA").insert(&pool).await;
        insert_dividend_amount(&pool, 1, 1, ymd(2026, 3, 10), 1, dec("70.0")).await;
        insert_dividend_amount(&pool, 2, 1, ymd(2026, 3, 10), 1, dec("70.00")).await;

        let h = health(&pool, ymd(2026, 7, 13)).await;
        assert_eq!(h.duplicate_income.len(), 1);
        assert_eq!(h.duplicate_income[0].income_ids, vec![1, 2]);
    }

    /// Three of a kind is one row counting three, as on the other two lists.
    #[tokio::test]
    async fn three_identical_income_rows_are_one_row_counting_three() {
        let pool = test_pool().await;
        test_support::listing(1).ticker("TRIP").insert(&pool).await;
        insert_dividend(&pool, 7, 1, ymd(2026, 3, 10), 1, "70").await;
        insert_dividend(&pool, 8, 1, ymd(2026, 3, 10), 1, "70").await;
        insert_dividend(&pool, 9, 1, ymd(2026, 3, 10), 1, "70").await;

        let h = health(&pool, ymd(2026, 7, 13)).await;
        assert_eq!(h.duplicate_income.len(), 1);
        assert_eq!(h.duplicate_income[0].income_count, 3);
        assert_eq!(h.duplicate_income[0].income_ids, vec![7, 8, 9]);
    }

    /// One same-day cluster can hold both a duplicated pair and an unrelated
    /// payment: the grouping is per amount fingerprint, not per day, so the
    /// third row neither joins the pair nor suppresses it.
    #[tokio::test]
    async fn a_duplicated_pair_is_reported_beside_a_genuine_second_dividend() {
        let pool = test_pool().await;
        test_support::listing(1).ticker("DIVA").insert(&pool).await;
        insert_dividend(&pool, 1, 1, ymd(2026, 3, 10), 1, "70").await;
        insert_dividend(&pool, 2, 1, ymd(2026, 3, 10), 1, "70").await;
        // The special dividend paid the same day.
        insert_dividend(&pool, 3, 1, ymd(2026, 3, 10), 1, "12.34").await;

        let h = health(&pool, ymd(2026, 7, 13)).await;
        assert_eq!(h.duplicate_income.len(), 1);
        assert_eq!(h.duplicate_income[0].gross_amount, dec("70"));
        assert_eq!(h.duplicate_income[0].income_ids, vec![1, 2]);
    }

    // The two listing-less sides of the tax summary (SCENARIOS H-01, H-06).

    async fn insert_interest(
        pool: &SqlitePool,
        id: i64,
        date: NaiveDate,
        amount: Decimal,
        source: Option<&str>,
    ) {
        crate::entities::interest_income::db_upsert(
            pool,
            &crate::entities::interest_income::InterestIncome {
                id,
                date_paid: date,
                amount,
                tfn_withholding_tax: Decimal::ZERO,
                foreign_source: false,
                foreign_tax_paid: Decimal::ZERO,
                currency: "AUD".to_string(),
                source: source.map(str::to_string),
                holding_account_id: None,
            },
        )
        .await
        .unwrap();
    }

    fn an_expense(
        id: i64,
        date: NaiveDate,
        expense_type: ExpenseType,
        amount: Decimal,
    ) -> InvestmentExpense {
        InvestmentExpense {
            id,
            date_incurred: date,
            expense_type,
            amount,
            gross_amount: None,
            deductible_percentage: None,
            currency: "AUD".to_string(),
            description: None,
            listing_id: None,
            holding_account_id: None,
        }
    }

    async fn insert_expense(pool: &SqlitePool, expense: &InvestmentExpense) {
        crate::entities::investment_expense::db_upsert(pool, expense)
            .await
            .unwrap();
    }

    /// A term-deposit credit keyed twice doubles the year's gross interest and
    /// nothing else notices, so the health report names the pair by id.
    #[tokio::test]
    async fn duplicated_interest_rows_are_reported_with_their_ids() {
        let pool = test_pool().await;
        insert_interest(&pool, 1, ymd(2026, 3, 10), dec("250"), Some("ANZ savings")).await;
        insert_interest(&pool, 2, ymd(2026, 3, 10), dec("250"), Some("ANZ savings")).await;
        insert_interest(&pool, 3, ymd(2026, 6, 30), dec("500"), Some("Term deposit")).await;
        insert_interest(&pool, 4, ymd(2026, 6, 30), dec("500"), Some("Term deposit")).await;

        let h = health(&pool, ymd(2026, 7, 13)).await;
        // Newest first, as on every other duplicate list.
        assert_eq!(h.duplicate_interest.len(), 2);
        let june = &h.duplicate_interest[0];
        assert_eq!(june.date_paid, ymd(2026, 6, 30));
        assert_eq!(june.amount, dec("500"));
        assert_eq!(june.currency, "AUD");
        assert_eq!(june.source.as_deref(), Some("Term deposit"));
        assert_eq!(june.holding_account_id, None);
        assert_eq!(june.interest_count, 2);
        assert_eq!(june.interest_ids, vec![3, 4]);
        let march = &h.duplicate_interest[1];
        assert_eq!(march.amount, dec("250"));
        assert_eq!(march.source.as_deref(), Some("ANZ savings"));
        assert_eq!(march.interest_ids, vec![1, 2]);
    }

    /// Interest has no listing, so `source` and the holding account carry the
    /// payer identity: two $250 credits on one day from different accounts are
    /// legitimate and stay unflagged, as do rows differing in date, amount, or
    /// any withholding figure.
    #[tokio::test]
    async fn interest_differing_in_any_key_field_is_not_a_duplicate() {
        let pool = test_pool().await;
        crate::entities::holding_account::db_upsert(
            &pool,
            &crate::entities::holding_account::HoldingAccount {
                id: 2,
                name: "Second".to_string(),
            },
        )
        .await
        .unwrap();
        // Same date and amount, different source: two banks, one day.
        insert_interest(&pool, 1, ymd(2026, 3, 10), dec("250"), Some("ANZ savings")).await;
        insert_interest(&pool, 2, ymd(2026, 3, 10), dec("250"), Some("CBA savings")).await;
        // Same source and amount, different date.
        insert_interest(&pool, 3, ymd(2026, 9, 10), dec("250"), Some("ANZ savings")).await;
        // Same source and date, different amount.
        insert_interest(
            &pool,
            4,
            ymd(2026, 3, 10),
            dec("12.34"),
            Some("ANZ savings"),
        )
        .await;
        // Same date, amount and source, different holding account.
        crate::entities::interest_income::db_upsert(
            &pool,
            &crate::entities::interest_income::InterestIncome {
                id: 5,
                date_paid: ymd(2026, 3, 10),
                amount: dec("250"),
                tfn_withholding_tax: Decimal::ZERO,
                foreign_source: false,
                foreign_tax_paid: Decimal::ZERO,
                currency: "AUD".to_string(),
                source: Some("ANZ savings".to_string()),
                holding_account_id: Some(2),
            },
        )
        .await
        .unwrap();
        // Same everything but the TFN amount withheld — one row was keyed from
        // a statement that showed the withholding, so they are different rows.
        crate::entities::interest_income::db_upsert(
            &pool,
            &crate::entities::interest_income::InterestIncome {
                id: 6,
                date_paid: ymd(2026, 3, 10),
                amount: dec("250"),
                tfn_withholding_tax: dec("117.50"),
                foreign_source: false,
                foreign_tax_paid: Decimal::ZERO,
                currency: "AUD".to_string(),
                source: Some("ANZ savings".to_string()),
                holding_account_id: None,
            },
        )
        .await
        .unwrap();

        let h = health(&pool, ymd(2026, 7, 13)).await;
        assert!(h.duplicate_interest.is_empty());
    }

    /// Grouped on decimal value, not on the stored TEXT — and three of a kind
    /// is one row counting three, as on every other list.
    #[tokio::test]
    async fn interest_amounts_equal_in_value_but_not_in_text_are_still_duplicates() {
        let pool = test_pool().await;
        insert_interest(&pool, 1, ymd(2026, 3, 10), dec("250.0"), Some("ANZ")).await;
        insert_interest(&pool, 2, ymd(2026, 3, 10), dec("250.00"), Some("ANZ")).await;
        insert_interest(&pool, 3, ymd(2026, 3, 10), dec("250.000"), Some("ANZ")).await;

        let h = health(&pool, ymd(2026, 7, 13)).await;
        assert_eq!(h.duplicate_interest.len(), 1);
        assert_eq!(h.duplicate_interest[0].interest_count, 3);
        assert_eq!(h.duplicate_interest[0].interest_ids, vec![1, 2, 3]);
    }

    /// A re-submitted expense form claims the deduction twice, lowering the
    /// year's net assessable investment income by the same amount again.
    #[tokio::test]
    async fn duplicated_expense_rows_are_reported_with_their_ids() {
        let pool = test_pool().await;
        test_support::listing(1).ticker("FEEA").insert(&pool).await;
        for id in [1, 2] {
            let mut e = an_expense(id, ymd(2026, 3, 10), ExpenseType::AdviceFee, dec("200"));
            e.listing_id = Some(1);
            e.description = Some("Annual advice fee".to_string());
            insert_expense(&pool, &e).await;
        }
        for id in [3, 4] {
            insert_expense(
                &pool,
                &an_expense(id, ymd(2026, 6, 1), ExpenseType::LoanInterest, dec("1500")),
            )
            .await;
        }

        let h = health(&pool, ymd(2026, 7, 13)).await;
        assert_eq!(h.duplicate_expenses.len(), 2);
        let june = &h.duplicate_expenses[0];
        assert_eq!(june.date_incurred, ymd(2026, 6, 1));
        assert_eq!(june.expense_type, ExpenseType::LoanInterest);
        assert_eq!(june.amount, dec("1500"));
        assert_eq!(june.currency, "AUD");
        // A portfolio-wide expense names no holding.
        assert_eq!(june.listing_id, None);
        assert_eq!(june.ticker, None);
        assert_eq!(june.expense_ids, vec![3, 4]);
        let march = &h.duplicate_expenses[1];
        assert_eq!(march.expense_type, ExpenseType::AdviceFee);
        assert_eq!(march.amount, dec("200"));
        assert_eq!(march.listing_id, Some(1));
        // …and a listing-attributed one is named by ticker, not only by id.
        assert_eq!(march.ticker.as_deref(), Some("FEEA"));
        assert_eq!(march.description.as_deref(), Some("Annual advice fee"));
        assert_eq!(march.expense_count, 2);
        assert_eq!(march.expense_ids, vec![1, 2]);
    }

    /// Everything that identifies the expense is in the key: two advice fees of
    /// the same amount on one day against different listings — or of different
    /// types, amounts, descriptions, or apportionment provenance — are ordinary
    /// entry, not a double claim.
    #[tokio::test]
    async fn expenses_differing_in_any_key_field_are_not_duplicates() {
        let pool = test_pool().await;
        test_support::listing(1).ticker("AAA").insert(&pool).await;
        test_support::listing(2).ticker("BBB").insert(&pool).await;
        let base = an_expense(0, ymd(2026, 3, 10), ExpenseType::AdviceFee, dec("200"));
        // Same date, type and amount, different listing.
        let mut a = base.clone();
        a.id = 1;
        a.listing_id = Some(1);
        insert_expense(&pool, &a).await;
        let mut b = base.clone();
        b.id = 2;
        b.listing_id = Some(2);
        insert_expense(&pool, &b).await;
        // Same listing-less row, different type.
        let mut c = base.clone();
        c.id = 3;
        c.expense_type = ExpenseType::ManagementFee;
        insert_expense(&pool, &c).await;
        // …different amount.
        let mut d = base.clone();
        d.id = 4;
        d.amount = dec("12.34");
        insert_expense(&pool, &d).await;
        // …different description: one invoice per quarter, keyed the same day.
        let mut e = base.clone();
        e.id = 5;
        e.description = Some("Q1".to_string());
        insert_expense(&pool, &e).await;
        let mut f = base.clone();
        f.id = 6;
        f.description = Some("Q2".to_string());
        insert_expense(&pool, &f).await;
        // …and one carrying the apportionment provenance the other lacks.
        let mut g = base.clone();
        g.id = 7;
        g.gross_amount = Some(dec("400"));
        g.deductible_percentage = Some(dec("50"));
        insert_expense(&pool, &g).await;
        // …plus one on another date entirely.
        let mut i = base.clone();
        i.id = 8;
        i.date_incurred = ymd(2026, 9, 10);
        insert_expense(&pool, &i).await;

        let h = health(&pool, ymd(2026, 7, 13)).await;
        assert!(h.duplicate_expenses.is_empty());
    }

    // The employee-share-scheme side (SCENARIOS J-11).

    /// A statement that vests `quantity` shares worth `market_value` each, all
    /// of it a deferral-scheme (label F) discount — the RSU shape.
    async fn insert_ess(
        pool: &SqlitePool,
        id: i64,
        listing_id: i64,
        taxing_point: NaiveDate,
        quantity: &str,
        market_value: &str,
    ) {
        test_support::ess_statement(id, listing_id, taxing_point)
            .with(|s| {
                s.quantity = dec(quantity);
                s.market_value_per_share = dec(market_value);
                s.deferral_discount = dec(quantity) * dec(market_value);
            })
            .insert(pool)
            .await;
    }

    /// The same statement entered twice — the accident the 30-day rule makes
    /// likely, an amended employer statement keyed as a new row — doubles the
    /// year's Item 12 discount and vests the parcel twice. Reported with both
    /// ids, newest taxing point first.
    #[tokio::test]
    async fn duplicated_ess_statements_are_reported_with_their_ids() {
        let pool = test_pool().await;
        test_support::listing(1).ticker("EMPA").insert(&pool).await;
        test_support::listing(2).ticker("EMPB").insert(&pool).await;
        insert_ess(&pool, 1, 1, ymd(2026, 3, 10), "100", "10").await;
        insert_ess(&pool, 2, 1, ymd(2026, 3, 10), "100", "10").await;
        insert_ess(&pool, 3, 2, ymd(2026, 6, 1), "50", "4.25").await;
        insert_ess(&pool, 4, 2, ymd(2026, 6, 1), "50", "4.25").await;

        let h = health(&pool, ymd(2026, 7, 13)).await;
        // Newest first, as on the other duplicate lists.
        assert_eq!(h.duplicate_ess_statements.len(), 2);
        let june = &h.duplicate_ess_statements[0];
        assert_eq!(june.ticker, "EMPB");
        assert_eq!(june.listing_id, 2);
        assert_eq!(june.holding_account_id, 1);
        assert_eq!(june.taxing_point_date, ymd(2026, 6, 1));
        assert_eq!(june.currency, "AUD");
        assert_eq!(june.quantity, dec("50"));
        assert_eq!(june.discount_total, dec("212.50"));
        assert_eq!(june.statement_count, 2);
        assert_eq!(june.statement_ids, vec![3, 4]);
        let march = &h.duplicate_ess_statements[1];
        assert_eq!(march.ticker, "EMPA");
        assert_eq!(march.taxing_point_date, ymd(2026, 3, 10));
        assert_eq!(march.discount_total, dec("1000"));
        assert_eq!(march.statement_ids, vec![1, 2]);
    }

    /// The legitimate case must stay silent: two vests on one date from
    /// different grants are ordinary, and so is a different listing, account or
    /// taxing point. The figures are part of the key for exactly that reason —
    /// a different quantity or a different discount is a second grant.
    #[tokio::test]
    async fn ess_statements_differing_in_any_key_field_are_not_duplicates() {
        let pool = test_pool().await;
        test_support::listing(1).ticker("AAA").insert(&pool).await;
        test_support::listing(2).ticker("BBB").insert(&pool).await;
        crate::entities::holding_account::db_upsert(
            &pool,
            &crate::entities::holding_account::HoldingAccount {
                id: 2,
                name: "Second".to_string(),
            },
        )
        .await
        .unwrap();
        insert_ess(&pool, 1, 1, ymd(2026, 3, 10), "100", "10").await;
        // Same account, taxing point and figures, different listing.
        insert_ess(&pool, 2, 2, ymd(2026, 3, 10), "100", "10").await;
        // Same listing, taxing point and figures, different holding account.
        test_support::ess_statement(3, 1, ymd(2026, 3, 10))
            .with(|s| {
                s.holding_account_id = 2;
                s.quantity = dec("100");
                s.market_value_per_share = dec("10");
                s.deferral_discount = dec("1000");
            })
            .insert(&pool)
            .await;
        // Same listing, account and figures, different taxing point.
        insert_ess(&pool, 4, 1, ymd(2026, 9, 10), "100", "10").await;
        // Same listing, account and taxing point, different quantity: a second
        // tranche vesting the same day.
        insert_ess(&pool, 5, 1, ymd(2026, 3, 10), "40", "10").await;
        // …and one of the same size whose discount differs (shares part-paid
        // for under the plan), which is likewise a second grant.
        test_support::ess_statement(6, 1, ymd(2026, 3, 10))
            .with(|s| {
                s.quantity = dec("100");
                s.market_value_per_share = dec("10");
                s.deferral_discount = dec("600");
            })
            .insert(&pool)
            .await;

        let h = health(&pool, ymd(2026, 7, 13)).await;
        assert!(h.duplicate_ess_statements.is_empty());
    }

    /// The figures are compared as decimals, not as the TEXT they are stored
    /// as: `1000.0` and `1000.00` are one grant entered twice.
    #[tokio::test]
    async fn ess_figures_equal_in_value_but_not_in_text_are_still_duplicates() {
        let pool = test_pool().await;
        test_support::listing(1).ticker("EMPA").insert(&pool).await;
        insert_ess(&pool, 1, 1, ymd(2026, 3, 10), "100.0", "10.0").await;
        insert_ess(&pool, 2, 1, ymd(2026, 3, 10), "100.00", "10.00").await;

        let h = health(&pool, ymd(2026, 7, 13)).await;
        assert_eq!(h.duplicate_ess_statements.len(), 1);
        assert_eq!(h.duplicate_ess_statements[0].statement_ids, vec![1, 2]);
    }

    /// One same-day cluster can hold both a duplicated pair and an unrelated
    /// grant: the grouping is per figure fingerprint, not per taxing point, so
    /// the third statement neither joins the pair nor suppresses it. The pair
    /// is still reported once it has been vested — `vest_trade_id` is derived,
    /// not part of the key.
    #[tokio::test]
    async fn a_duplicated_ess_pair_is_reported_beside_a_second_grant_and_after_vesting() {
        let pool = test_pool().await;
        test_support::listing(1).ticker("EMPA").insert(&pool).await;
        insert_ess(&pool, 1, 1, ymd(2026, 3, 10), "100", "10").await;
        insert_ess(&pool, 2, 1, ymd(2026, 3, 10), "100", "10").await;
        // The second tranche vesting the same day.
        insert_ess(&pool, 3, 1, ymd(2026, 3, 10), "40", "10").await;
        crate::entities::ess_vest::db_vest(&pool, 1).await.unwrap();

        let h = health(&pool, ymd(2026, 7, 13)).await;
        assert_eq!(h.duplicate_ess_statements.len(), 1);
        assert_eq!(h.duplicate_ess_statements[0].quantity, dec("100"));
        assert_eq!(h.duplicate_ess_statements[0].statement_ids, vec![1, 2]);
    }

    #[tokio::test]
    async fn api_get_health() {
        let pool = test_pool().await;
        insert_job_run(&pool, "backup", "2026-07-13T00:00:00Z", Some("disk full")).await;
        let resp = ApiClient::over(router().with_state(pool))
            .get("/reports/health")
            .await;
        assert_eq!(resp.status, StatusCode::OK);
        let h: HealthReport = resp.json();
        assert_eq!(h.failed_jobs.len(), 1);
        assert_eq!(h.failed_jobs[0].name, "backup");
    }
}
