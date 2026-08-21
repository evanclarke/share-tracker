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
//!   cannot be corrected in place either (see [`ExchangeCurrencyMismatch`]).
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

use crate::domain::rollover;
use crate::domain::tax_year::tax_year_for;
use crate::entities::closing_price::{self, HeldTimeline, PriceOrigin};
use crate::entities::ess_statement::{self, EssStatement};
use crate::entities::income::Income;
use crate::entities::inheritance::{self, Inheritance};
use crate::entities::interest_income::InterestIncome;
use crate::entities::investment_expense::{ExpenseType, InvestmentExpense};
use crate::infra::decimal::Money;
use crate::infra::http::ApiError;
use axum::{Json, Router, extract::State, routing::get};
use chrono::{DateTime, Datelike, Duration, NaiveDate, Utc, Weekday};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

/// Prices are stale once the latest ok closing price is more than this many
/// business days (Mon–Fri) old. The price-import job runs every weekday, so a
/// healthy database is at most 1–2 business days behind; 3 leaves headroom for
/// a long exchange-holiday weekend without a false alarm.
pub const PRICE_STALE_BUSINESS_DAYS: i64 = 3;

/// How many consecutive shared trading days of *identical* closes make two
/// listings one series stored twice — see [`DuplicatePriceSeries`]. Thirty is
/// about six weeks of trading. Two genuinely distinct securities do coincide
/// to the cent now and then (Evan's LAC and LAR both closed at 4.12 on
/// 2024-02-08, four months after their series diverged), but a scattered
/// coincidence is not a run: matching on thirty *consecutive* comparisons is
/// not something two real securities do, and the observed incident ran to 634.
pub const DUPLICATE_PRICE_SERIES_RUN_DAYS: i64 = 30;

/// The ESS 30-day rule's window: a disposal **within 30 days after** the
/// deferred taxing point moves the taxing point to the disposal date
/// (`docs/ato/ess-30-day-rule.md`, QC 23058 Example 11). Unlike the wash-sale
/// window this is statutory (ITAA 1997 s 83A-115(3)), not a review convention,
/// so it is a constant rather than a request parameter.
pub const ESS_THIRTY_DAY_WINDOW: i64 = 30;

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
///
/// Rows outside the span the provider serves the listing — dated from its
/// `unpriced_from`, or before its `unpriced_before` — are excluded: the
/// provider is recorded as serving nothing there, so they are expected rather
/// than a to-do, and valuation carries the last close forward instead of
/// blocking (SCENARIOS Q-02) or leaves the holding out of the date's totals
/// (migration 0037).
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
/// manual price for a day the provider can never serve. Days from the
/// listing's `unpriced_from`, or before its `unpriced_before`, are excluded
/// for the same reason [`ErroredPriceListing`] excludes them.
#[derive(Debug, Serialize, Deserialize)]
pub struct UnpricedListing {
    pub listing_id: i64,
    pub ticker: String,
    /// Count of distinct valuation days with no stored row.
    pub unpriced_days: i64,
    pub earliest_date: NaiveDate,
    pub latest_date: NaiveDate,
}

/// A recorded `Demerger` whose head listing has stored closing prices from
/// *before* the demerger that the provider served **after** it — figures the
/// provider restated by its spin-off price-adjustment factor, which nothing
/// can undo until the action carries a stated pre-demerger close.
///
/// The counterpart of the split case, which needs no warning because a split
/// states its own ratio: a demerger changes no unit count on the head listing,
/// so the factor has no term in the action to be read from and is unknowable
/// until the operator states what the security actually closed at on the last
/// pre-demerger trading day (`entities::closing_price` module docs). Until
/// then those prices are silently the *current* level — Evan's LAC history was
/// ~2.46x understated this way — and nothing else surfaces it: the rows are ok,
/// not errored, so `errored_prices` and `unpriced_days` both pass over them and
/// the only symptom is a valuation that looks plausible and is wrong.
///
/// Close it by adding `demerger_close_date` / `demerger_close_price` (and their
/// provenance) to the action; the write re-bases the listing's prices in its own
/// transaction.
///
/// Deliberately a **warning, not a constraint**: a provider that did not adjust
/// for a particular spin-off, or a series fetched entirely before it, needs no
/// statement at all, so the action stays enterable without one.
///
/// **Two figures, because the span holds two kinds of row and only one of them
/// a stated close repairs.** `adjusted_days` counts what the re-base walk will
/// touch (ok, `origin = 'fetched'`, observed on or after the demerger); the
/// `manual_*` figures count the hand-entered rows sitting in the same
/// pre-demerger span, which `db_rebase_listing_prices` skips by design — a
/// manual price is contemporaneous by declaration, so it is never restated.
/// Publishing only the first made the count read as the size of the problem
/// when it is not: on Evan's LAC the fetched half is 260 rows from 2022-09-20
/// while the affected span is 635 rows from 2021-03-25, the earlier 375 being
/// hand-entered copies of the *demerged* entity's series whose own `reason`
/// says they are "unblocked, not accurate". Reporting both says what a stated
/// close fixes and what it leaves behind.
///
/// The manual figures are **context on this warning, not a warning of their
/// own**: a demerger with only manual pre-demerger rows is not listed at all
/// (nothing needs re-basing), and stating the close clears the whole row while
/// those rows stay as entered. Judging a hand-entered price wrong is a
/// different check from this one.
#[derive(Debug, Serialize, Deserialize, sqlx::FromRow)]
pub struct DemergerMissingClose {
    pub action_id: i64,
    pub listing_id: i64,
    pub ticker: String,
    /// The demerger date — every stored price before it is suspect.
    pub demerger_date: NaiveDate,
    /// How many stored ok, provider-fetched rows dated before the demerger
    /// were observed on or after it: exactly the rows a stated close re-bases.
    /// Always ≥ 1 (a demerger with none is not reported), and **not** the size
    /// of the suspect span — hand-entered rows in it are counted separately by
    /// `manual_days`.
    pub adjusted_days: i64,
    /// Earliest and latest `price_date` of the `adjusted_days` rows — the
    /// fetched half of the span only, so the span as a whole starts at
    /// `min(earliest_date, manual_earliest_date)`.
    pub earliest_date: NaiveDate,
    pub latest_date: NaiveDate,
    /// How many stored ok, **hand-entered** (`origin = 'manual'`) rows are
    /// dated before the demerger, whenever they were entered — the rows in the
    /// same suspect span that a stated close does **not** repair, because the
    /// re-base walk skips manual rows by design. 0 when there are none.
    pub manual_days: i64,
    /// Earliest and latest `price_date` of the `manual_days` rows; `None`
    /// exactly when `manual_days` is 0.
    pub manual_earliest_date: Option<NaiveDate>,
    pub manual_latest_date: Option<NaiveDate>,
}

/// A recorded `Demerger` whose **head listing looks like the entity the
/// demerger created**, not the one that continued — the two sides recorded the
/// wrong way round.
///
/// A demerger has a continuing entity (the head, `listing_id`, which traded
/// before the event and goes on trading after it) and one it *creates*
/// (`demerger_listing_id`, whose first trading day is the demerger). Nothing in
/// the model records which side is which beyond the two columns themselves, so
/// entering them swapped is accepted in full: the apportionment percentages
/// attach to the tickers rather than to the roles, so no tax figure moves and
/// no write-time check can see it. What breaks is the *price* side — the head
/// parcel then sits on a listing that did not exist when it was bought, so the
/// provider serves no history for it and every valuation before the demerger
/// is unvaluable (Evan's LAC, invisible for three years: 921 snapshot dates
/// could not value the holding, and 635 prices were borrowed from the other
/// listing to paper over the gap).
///
/// **The predicate is an asymmetry, not an absence.** A head listing with no
/// prices before the demerger proves nothing on its own — a series nobody ever
/// collected looks exactly the same. What no correctly recorded demerger
/// produces is the *inversion*:
///
/// 1. the head listing holds **no ok closing price at all** dated before the
///    demerger — its series does not reach back past the event, which is what
///    a newly created entity looks like (and what [`unpriced_before`] records
///    explicitly once the operator has established it);
/// 2. the **demerged** listing holds provider-**fetched** ok prices dated
///    before it — the entity supposedly created by the demerger has a provider
///    series predating its own creation, which only the continuing entity has;
/// 3. the head listing was **acquired before** the demerger (a Buy or DRP
///    dated earlier), so the model really does claim to have held a security
///    on days its own provider says it did not exist.
///
/// Each clause kills a class of false positive. Without (2) a fresh database
/// where nothing has been collected lights up every demerger it holds; (2)
/// requires evidence from the provider on the *other* side, which an
/// uncollected database cannot produce, and requires it fetched rather than
/// hand-entered, since a hand-entered row is the operator's own assertion and
/// this check exists to question the operator's model. Without (3) a demerger
/// whose head history was simply never backfilled reads as a defect; (3) is
/// what makes the missing series a contradiction rather than a gap.
///
/// **Is there a legitimate demerger of this shape?** The continuing entity
/// traded before the event by definition, so a head with no earlier series is
/// already odd; and the demerged entity is created by it, so an earlier series
/// on *that* side is odder still. The one case where a demerged listing
/// genuinely predates the event is an in-specie distribution of an
/// already-listed holding — and there the parent kept trading throughout, so
/// clause (1) fails and nothing is reported. No arrangement was found in which
/// the continuing entity has no pre-event series while the entity it spun off
/// does; the shape is either a swap or a collection accident, and (3) rules the
/// accident out.
///
/// **What it does not catch, honestly.** It names the condition only once the
/// borrowed rows are gone (or were never added). On the live data as it stood
/// *before* the 2026-08-21 cleanup this check would have stayed silent: listing
/// 7 held 635 pre-demerger prices copied from listing 8, so clause (1) failed.
/// [`DuplicatePriceSeries`] is what names the borrowed rows while they are
/// there, and this check is what names the modelling error once they are
/// cleared — the two are complements, not alternatives, and between them the
/// incident is visible at both stages.
///
/// **Reported, never rejected**, the same call as the rest of this list: the
/// remedy is to re-record the action with the sides swapped (and to move the
/// parcel's Buy to the continuing listing), which is a data correction only the
/// operator can judge against the company's own demerger notice. It clears as
/// soon as the head listing is the one with the earlier series.
///
/// [`unpriced_before`]: crate::entities::listing::Listing::unpriced_before
#[derive(Debug, Serialize, Deserialize, sqlx::FromRow)]
pub struct DemergerHeadNotContinuing {
    pub action_id: i64,
    /// The head listing — the one recorded as continuing, which the price
    /// series says was created by the demerger.
    pub listing_id: i64,
    pub ticker: String,
    pub demerger_date: NaiveDate,
    /// The head listing's `unpriced_before`, when it carries one: the explicit
    /// record that the provider's series *begins* there, which is the modern
    /// statement of exactly what this check infers. `None` when unset — the
    /// marker corroborates the finding, it is not required for it.
    pub head_unpriced_before: Option<NaiveDate>,
    /// Earliest ok closing price stored for the head listing, of any origin —
    /// where its series actually begins. Always on or after `demerger_date`
    /// (the check requires nothing earlier), and `None` when it has no ok
    /// price at all.
    pub head_first_price_date: Option<NaiveDate>,
    /// Earliest Buy/DRP on the head listing dated before the demerger: the day
    /// the model starts claiming to hold a security whose own series had not
    /// begun.
    pub head_held_from: NaiveDate,
    /// The demerged listing — the one recorded as created, which the price
    /// series says was trading all along.
    pub demerger_listing_id: i64,
    pub demerger_ticker: String,
    /// How many ok, provider-fetched prices the demerged listing holds from
    /// before the demerger — the evidence that it, not the head, is the
    /// continuing entity. Always ≥ 1.
    pub demerged_priced_days: i64,
    /// Earliest and latest `price_date` of those rows.
    pub demerged_earliest_date: NaiveDate,
    pub demerged_latest_date: NaiveDate,
}

/// Two listings whose stored closing prices are *identical* across a long run
/// of consecutive trading days: one security's price series stored under both,
/// which is what a series fetched or copied under the wrong symbol looks like
/// afterwards.
///
/// This is the only alarm the case leaves once the fetch is over. Every row is
/// `ok`, so `errored_prices` and `unpriced_days` both pass over them; the
/// figures are plausible; and a row fetched under a one-off `symbol` override
/// before migration 0038 records no symbol at all, so nothing on the row
/// itself says where it came from. Evan's listing 7 (`LAC`) carries 634 such
/// rows — every one equal to listing 8 (`LAR`)'s close for the same day — and
/// 259 of them are exactly those provenance-less fetched rows.
///
/// **The predicate is a run, not a total.** Two distinct securities can close
/// at the same figure on scattered days by chance (LAC and LAR did, on
/// 2024-02-08, well after their series parted), so a count of matching days
/// over all history would report the accident and the incident alike. A run of
/// [`DUPLICATE_PRICE_SERIES_RUN_DAYS`] consecutive *comparisons* — days on
/// which both listings hold an ok price — is what no pair of real securities
/// produces. A day only one of them has a price for neither breaks nor extends
/// the run: it is not a comparison. A run whose closes never move is not
/// reported at all, however long: two instruments pinned to one figure (a pair
/// of stablecoins at 1.00, two funds at a constant unit price) match forever
/// without either series being a copy, and an alarm nobody can ever clear is
/// worse than none.
///
/// The per-origin split is the same call [`DemergerMissingClose`] made, and
/// for the same reason: the two halves need different remedies. Hand-entered
/// rows state where they came from and why (`sourced_from`/`reason`), so the
/// operator can read the intent off the row; fetched rows before 0038 state
/// nothing, which is the half this check exists for.
///
/// **Reported, never rejected, and with no way to silence it**: the check owns
/// no marker of its own, exactly like the rest of the `duplicate_*` family —
/// it clears when the borrowed rows go (`DELETE
/// /closing_prices/:listing_id/:price_date`, or the bulk
/// `POST /closing_prices/clear_unpriced_before`) or are replaced by the
/// listing's own series. Setting [`unpriced_before`] deliberately does *not*
/// quiet it: that marker says the provider serves nothing before the date,
/// which is a reason to delete the stored rows, not to accept them — unlike
/// `errored_prices`, where the marker explains an *absence*.
///
/// [`unpriced_before`]: crate::entities::listing::Listing::unpriced_before
#[derive(Debug, Serialize, Deserialize)]
pub struct DuplicatePriceSeries {
    /// The pair, ordered by id, so each pair is reported once.
    pub listing_id: i64,
    pub ticker: String,
    pub other_listing_id: i64,
    pub other_ticker: String,
    /// Length of the longest run of consecutive shared trading days on which
    /// the two listings' stored closes are equal — always ≥
    /// [`DUPLICATE_PRICE_SERIES_RUN_DAYS`], and **not** the number of matching
    /// days over all history (a later coincidence is not part of it).
    pub identical_days: i64,
    /// First and last `price_date` of that run.
    pub earliest_date: NaiveDate,
    pub latest_date: NaiveDate,
    /// How many of `listing_id`'s rows in the run were fetched from a provider
    /// and how many were entered by hand — the two halves sum to
    /// `identical_days`. A hand-entered row carries its own `sourced_from` /
    /// `reason`; a fetched one may carry nothing at all.
    pub fetched_days: i64,
    pub manual_days: i64,
    /// The same split for `other_listing_id`'s rows in the run.
    pub other_fetched_days: i64,
    pub other_manual_days: i64,
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

/// Two or more inheritances of one listing, in one holding account, from one
/// date of death, carrying **identical figures** (SCENARIOS K-09). Each one
/// creates its own parcel Buy, so a 100-unit holding entered twice is 200
/// units held at twice the cost base — every open-parcels, valuation and
/// realised-gains figure for the listing doubles, and unlike the income-side
/// duplicates there is no year to bound the error: it persists until the
/// parcel is sold.
///
/// Deliberately a **warning, not a constraint**, the same call as
/// [`DuplicateEssStatement`]: two inheritances of one listing from one death
/// are ordinary — two beneficiaries' shares are not modelled, but two holding
/// accounts, two estates, or a part interest recorded in stages all are. The
/// figures are part of the key for exactly that reason: two rows differing in
/// quantity, cost base or LPR expenditure are two inheritances, not one
/// entered twice.
#[derive(Debug, Serialize, Deserialize)]
pub struct DuplicateInheritance {
    pub listing_id: i64,
    pub ticker: String,
    pub holding_account_id: i64,
    pub date_of_death: NaiveDate,
    /// ISO 4217 currency the shared figures are stated in (part of the key).
    pub currency: String,
    /// The units each duplicated row inherits, so the warning names the parcel
    /// rather than only its date.
    pub quantity: Decimal,
    /// The whole cost base each duplicated row carries onto its parcel (first
    /// element + LPR expenditure, in `currency`) — the figure counted once per
    /// row.
    pub cost_base_total: Decimal,
    /// How many inheritances share the whole key — always ≥ 2.
    pub inheritance_count: i64,
    /// The ids sharing it, ascending, so the surplus row can be opened and
    /// deleted without a search.
    pub inheritance_ids: Vec<i64>,
}

/// A listing whose `currency` is not the currency its exchange quotes in
/// (SCENARIOS R-01) — an AUD listing sitting on a USD market, or the reverse.
///
/// The listing is unpriceable from that moment on: every fetch resolves the
/// exchange's symbol, and either the provider serves no such series (an
/// errored row) or the candle's currency is not the listing's, which
/// `entities::closing_price`'s cross-check refuses (also an errored row). And
/// it cannot be corrected in place, because `listings.currency` denominates
/// every stored closing price and is frozen once the listing has any history
/// (`entities::listing::UpsertError::CurrencyChangeWithHistory`) — the
/// documented remedy is a new listing in the right currency plus a
/// [transfer](crate::entities::transfer) of the parcels to it.
///
/// `POST /listings/:id/rename` refuses to *create* the state
/// (`entities::listing_rename::RenameError::ExchangeCurrencyMismatch`), but a
/// plain `PUT /listings/:id` can still enter a listing in it directly — no
/// write path compares the two columns — so it is reported here wherever it
/// came from. Deliberately a **warning, not a constraint** on `PUT`: a real
/// exchange can quote more than one currency (LSE quotes GBp, USD and EUR)
/// while `exchanges.currency` is a single column, so the pairing is a strong
/// smell rather than a certainty.
///
/// Exchange-less listings — `security_type = 'Crypto'`, NULL `exchange_mic` —
/// have no exchange to disagree with and are excluded (the SQL joins
/// `exchanges`, so they never appear).
#[derive(Debug, Serialize, Deserialize, sqlx::FromRow)]
pub struct ExchangeCurrencyMismatch {
    pub listing_id: i64,
    pub ticker: String,
    /// The listing's own `currency` — what every stored closing price for it
    /// is denominated in.
    pub currency: String,
    pub exchange_mic: String,
    /// The currency that exchange quotes in (`exchanges.currency`).
    pub exchange_currency: String,
}

/// A disposal of ESS-vested shares **within 30 days after** the statement's
/// taxing point, where the ESS 30-day rule moves the taxing point to the
/// disposal date (SCENARIOS J-04, `docs/ato/ess-30-day-rule.md`, QC 23058
/// Example 11 — ITAA 1997 s 83A-115(3)).
///
/// What the rule does, and why silence here is costly: the discount is
/// re-measured at what the disposal actually realised, and the CGT cost base
/// resets to that same figure on that same date — so **there is no separate
/// capital gain**, and the discount can move into the **next financial year**.
/// A user who enters the employer's *original* statement and then the sale gets
/// both figures wrong at once, in two different years, from an entry the system
/// accepts without comment: the original discount assessed in the original
/// year, plus a capital gain that does not exist.
///
/// Advisory only, the same call as `reports::wash_sales`: nothing is rejected
/// and no figure is rewritten. The correction is an **amended employer
/// statement** — the employer must issue one within 30 days of becoming aware
/// of the disposal — and the system cannot know whether one was issued, nor
/// perform a re-measurement the ATO puts on the employer. Enter the amended
/// statement over the original (taxing point = the disposal date, market value
/// = what the disposal realised) and the figures follow.
///
/// A disposal **on** the taxing point is never flagged: the rule's effect is a
/// no-op there — the taxing point is already the disposal date — so the
/// corrected entry (the amended statement, vested and sold the same day, as in
/// `ato_examples::ess_30_day_rule_example_11_wyatt_amended_statement`) must not
/// nag.
///
/// # What counts as a disposal (SCENARIOS N-08)
///
/// The rule turns on a *disposal*, so the Sells that are not one are not
/// candidates — `docs/ato/ess-takeovers-and-restructures.md` (ITAA 1997
/// s 83A-130) is the source for the two rollover cases:
///
/// - A **holding-account transfer**'s closing Sell (`trades.transfer_id`) is
///   excluded in SQL. Nothing is disposed of — the same beneficial owner holds
///   the same interests before and after, which is why no CGT event arises
///   either — and it is not a takeover or restructure, so s 83A-130 does not
///   even come into it. This is the RSU-plan-to-broker move `entities::transfer`
///   exists for, so leaving it in flagged the *ordinary* use of the feature and
///   invited an amended return over a change of custody.
/// - A **scrip-for-scrip exchange** or **demerger** closing Sell stays, carrying
///   [`EssDisposalKind::TakeoverOrRestructure`]: s 83A-130(2) treats matching
///   replacement interests as a continuation, so the taxing point normally does
///   *not* move — but that rests on facts this system does not record ((4)'s
///   ordinary-shares test, (9)'s continuing-employment and two 10% tests), and
///   (5) makes a partial-rollover's cash component a disposal to that extent.
///   Advisory is the honest answer there, not silence.
#[derive(Debug, Serialize, Deserialize)]
pub struct EssThirtyDaySale {
    /// The Sell whose allocation draws on the vest parcel.
    pub sale_trade_id: i64,
    pub listing_id: i64,
    pub ticker: String,
    pub sale_date: NaiveDate,
    /// Units of the vest parcel this sale allocated — the ESS interests
    /// disposed of, which need not be the whole vest.
    pub units_sold: Decimal,
    pub ess_statement_id: i64,
    /// The statement's vest Buy, i.e. the parcel allocated from.
    pub vest_trade_id: i64,
    pub taxing_point_date: NaiveDate,
    /// `sale_date − taxing_point_date` in days: always 1..=30
    /// ([`ESS_THIRTY_DAY_WINDOW`]).
    pub days_after: i64,
    /// ISO 4217 currency `statement_discount` is stated in (the statement's).
    pub currency: String,
    /// The discount the statement currently declares (D + E + F + the pre-2009
    /// cessation label, in `currency`) — the figure the rule re-measures.
    pub statement_discount: Decimal,
    /// The financial year the statement's taxing point falls in, which is where
    /// that discount is assessed today.
    pub statement_tax_year: i32,
    /// The financial year the **disposal** falls in — where the rule moves the
    /// discount to. Equal to `statement_tax_year` when the window does not
    /// cross a 30 June, and that is the *common* case: the years differing is
    /// the Example 11 shape, where the correction also moves a return.
    pub disposal_tax_year: i32,
    /// Whether the Sell is an ordinary disposal or a rollover operation's
    /// closing Sell, which changes what the row is asking the user to check.
    pub disposal_kind: EssDisposalKind,
}

/// What kind of Sell reached the 30-day-rule alert — see
/// [`EssThirtyDaySale`]'s "What counts as a disposal".
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum EssDisposalKind {
    /// An ordinary disposal: the rule applies, and the correction is an amended
    /// employer statement.
    Sale,
    /// A scrip-for-scrip exchange or demerger closing Sell. ITAA 1997
    /// s 83A-130(2) treats matching replacement interests as a **continuation**
    /// of the old ESS interests, so the deferred taxing point normally does not
    /// move — subject to (4)/(9), which this system cannot test, and to (5),
    /// which does treat a partial-rollover's cash component as a disposal.
    TakeoverOrRestructure,
}

/// The joined row behind [`EssThirtyDaySale`]: the statement's own figures are
/// attached from a second read, so the discount is summed by
/// `ess_statement::discount_labels` rather than re-added here.
#[derive(sqlx::FromRow)]
struct EssThirtyDaySaleRow {
    sale_trade_id: i64,
    listing_id: i64,
    ticker: String,
    sale_date: NaiveDate,
    #[sqlx(try_from = "Money")]
    units_sold: Decimal,
    /// The parcel the allocation consumed — a vest parcel, or a replacement one
    /// down its rollover chain; the statement and vest parcel are looked up
    /// from it.
    parcel_id: i64,
    /// Set when the Sell is a scrip-for-scrip exchange or demerger closing
    /// Sell (the transfer case never reaches here — it is filtered in SQL).
    rollover_action_id: Option<i64>,
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
        //
        // A day from the listing's `unpriced_from` on is never a hole: the
        // provider serves nothing there and valuation carries the last ok
        // close forward, so there is nothing to backfill (SCENARIOS Q-02).
        // The write-time rule that the marker needs an earlier ok price is
        // what makes that unconditional — a marked listing always has a close
        // to carry.
        // The mirror at the other end: a day before `unpriced_before` is
        // never a hole either — the provider's series has not begun, so
        // nothing can be backfilled and valuation excludes the holding from
        // that date's totals instead (migration 0037).
        let unpriced_from = market.listing.unpriced_from;
        let unpriced_before = market.listing.unpriced_before;
        let mut missing: BTreeSet<NaiveDate> = BTreeSet::new();
        for (from, to) in spans {
            let mut date = from;
            while date <= to {
                if let Some(valuation_day) = market.latest_trading_day_on_or_before(date)
                    && !stored.contains(&valuation_day)
                    && unpriced_from.is_none_or(|u| valuation_day < u)
                    && unpriced_before.is_none_or(|u| valuation_day >= u)
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
/// Demergers carrying no stated pre-demerger close, whose head listing has
/// stored provider prices from before the demerger that were observed on or
/// after it — see [`DemergerMissingClose`].
///
/// "Observed on or after" is the same test the re-base walk applies
/// (`entities::closing_price`, the half-open `(price_date, fetched_at]`
/// window): a figure observed before the demerger arrived in the
/// contemporaneous basis and is fine. `fetched_at` is written as a UTC RFC 3339
/// timestamp, so its first ten characters are the UTC date the re-base parses
/// out of it — compared as text here rather than through SQLite's `date()`,
/// which does not parse the nanosecond precision the timestamps carry.
///
/// One pass over the listing's pre-demerger ok rows produces both figures, the
/// two halves split by `FILTER`: the fetched rows the re-base will touch, and
/// the hand-entered rows in the same span that it skips. That observation test
/// is applied to the fetched half only — a manual row's `fetched_at` records
/// when it was *typed in*, which says nothing about the basis it was stated
/// in. `HAVING` keeps the row-existence rule the check has always had: a
/// demerger is listed because provider-adjusted prices need re-basing, so one
/// with only manual pre-demerger rows is not listed.
async fn db_demergers_missing_close(
    conn: &mut sqlx::SqliteConnection,
) -> Result<Vec<DemergerMissingClose>, sqlx::Error> {
    sqlx::query_as::<_, DemergerMissingClose>(
        "SELECT ca.id AS action_id, ca.listing_id AS listing_id, l.ticker AS ticker, \
                ca.date AS demerger_date, \
                COUNT(*) FILTER (WHERE cp.origin = 'fetched') AS adjusted_days, \
                MIN(cp.price_date) FILTER (WHERE cp.origin = 'fetched') AS earliest_date, \
                MAX(cp.price_date) FILTER (WHERE cp.origin = 'fetched') AS latest_date, \
                COUNT(*) FILTER (WHERE cp.origin = 'manual') AS manual_days, \
                MIN(cp.price_date) FILTER (WHERE cp.origin = 'manual') AS manual_earliest_date, \
                MAX(cp.price_date) FILTER (WHERE cp.origin = 'manual') AS manual_latest_date \
         FROM corporate_actions ca \
         JOIN listings l ON l.id = ca.listing_id \
         JOIN closing_prices cp ON cp.listing_id = ca.listing_id \
         WHERE ca.action_type = 'Demerger' AND ca.demerger_close_date IS NULL \
           AND cp.status = 'ok' AND cp.price_date < ca.date \
           AND (cp.origin = 'manual' OR substr(cp.fetched_at, 1, 10) >= ca.date) \
         GROUP BY ca.id \
         HAVING COUNT(*) FILTER (WHERE cp.origin = 'fetched') > 0 \
         ORDER BY ca.date DESC, ca.id DESC",
    )
    .fetch_all(conn)
    .await
}

/// Demergers recorded with the created entity as the head listing — see
/// [`DemergerHeadNotContinuing`] for the predicate and what each clause of it
/// rules out.
///
/// One aggregate on the caller's transaction. The `JOIN` onto the demerged
/// listing's pre-demerger fetched rows *is* clause (2) — no such row, no
/// group, so an uncollected database produces nothing — while the two
/// `NOT EXISTS`/`EXISTS` clauses are (1) and (3). Dates are TEXT and compare
/// lexicographically, which is the ordering they are stored in.
async fn db_demergers_head_not_continuing(
    conn: &mut sqlx::SqliteConnection,
) -> Result<Vec<DemergerHeadNotContinuing>, sqlx::Error> {
    sqlx::query_as::<_, DemergerHeadNotContinuing>(
        "SELECT ca.id AS action_id, ca.listing_id AS listing_id, l.ticker AS ticker, \
                ca.date AS demerger_date, \
                l.unpriced_before AS head_unpriced_before, \
                (SELECT MIN(hp.price_date) FROM closing_prices hp \
                  WHERE hp.listing_id = ca.listing_id AND hp.status = 'ok') \
                    AS head_first_price_date, \
                (SELECT MIN(t.date) FROM trades t \
                  WHERE t.listing_id = ca.listing_id AND t.trade_type IN ('Buy', 'DRP') \
                    AND t.date < ca.date) AS head_held_from, \
                ca.demerger_listing_id AS demerger_listing_id, dl.ticker AS demerger_ticker, \
                COUNT(*) AS demerged_priced_days, \
                MIN(dp.price_date) AS demerged_earliest_date, \
                MAX(dp.price_date) AS demerged_latest_date \
         FROM corporate_actions ca \
         JOIN listings l ON l.id = ca.listing_id \
         JOIN listings dl ON dl.id = ca.demerger_listing_id \
         JOIN closing_prices dp ON dp.listing_id = ca.demerger_listing_id \
              AND dp.status = 'ok' AND dp.origin = 'fetched' AND dp.price_date < ca.date \
         WHERE ca.action_type = 'Demerger' \
           AND NOT EXISTS (SELECT 1 FROM closing_prices hp \
                            WHERE hp.listing_id = ca.listing_id AND hp.status = 'ok' \
                              AND hp.price_date < ca.date) \
           AND EXISTS (SELECT 1 FROM trades t \
                        WHERE t.listing_id = ca.listing_id AND t.trade_type IN ('Buy', 'DRP') \
                          AND t.date < ca.date) \
         GROUP BY ca.id \
         ORDER BY ca.date DESC, ca.id DESC",
    )
    .fetch_all(conn)
    .await
}

/// One stored ok closing price, read for [`db_duplicate_price_series`].
#[derive(sqlx::FromRow)]
struct StoredClose {
    listing_id: i64,
    price_date: NaiveDate,
    #[sqlx(try_from = "Money")]
    price: Decimal,
    origin: PriceOrigin,
}

/// The longest run of identical closes between two listings, accumulated as
/// the two date-sorted series are walked together.
#[derive(Clone)]
struct IdenticalRun {
    length: i64,
    start: NaiveDate,
    end: NaiveDate,
    /// The run's first close, and whether any later day of it differs — a run
    /// that never moves is two pinned instruments, not a copied series.
    first_price: Decimal,
    varies: bool,
    manual_days: i64,
    other_manual_days: i64,
}

/// The longest run of consecutive shared trading days on which `a` and `b`
/// hold an equal close. Both series are sorted by date, so this is one merge
/// walk: a date only one side has is skipped without breaking the run (it is
/// not a comparison), a date both sides have and disagree on ends it.
///
/// Prices are compared as `Decimal`s, not as the stored TEXT, so `"10.5"` and
/// `"10.50"` are the same close — the same rule the rest of the `duplicate_*`
/// family applies to its amounts.
fn longest_identical_run(a: &[StoredClose], b: &[StoredClose]) -> Option<IdenticalRun> {
    let (mut i, mut j) = (0, 0);
    let mut current: Option<IdenticalRun> = None;
    let mut best: Option<IdenticalRun> = None;
    while i < a.len() && j < b.len() {
        match a[i].price_date.cmp(&b[j].price_date) {
            std::cmp::Ordering::Less => i += 1,
            std::cmp::Ordering::Greater => j += 1,
            std::cmp::Ordering::Equal => {
                if a[i].price == b[j].price {
                    let run = current.get_or_insert(IdenticalRun {
                        length: 0,
                        start: a[i].price_date,
                        end: a[i].price_date,
                        first_price: a[i].price,
                        varies: false,
                        manual_days: 0,
                        other_manual_days: 0,
                    });
                    run.length += 1;
                    run.end = a[i].price_date;
                    run.varies |= a[i].price != run.first_price;
                    run.manual_days += i64::from(a[i].origin == PriceOrigin::Manual);
                    run.other_manual_days += i64::from(b[j].origin == PriceOrigin::Manual);
                    if best.as_ref().is_none_or(|b| run.length > b.length) {
                        best = Some(run.clone());
                    }
                } else {
                    current = None;
                }
                i += 1;
                j += 1;
            }
        }
    }
    best
}

/// Pairs of listings holding one price series between them — see
/// [`DuplicatePriceSeries`].
///
/// Read on the caller's transaction but compared in Rust, for the same reason
/// as [`db_duplicate_income`]: the prices are the key and they are TEXT
/// decimals, which SQL would compare as strings. The whole ok-price table is
/// loaded in one query and grouped per listing — a personal portfolio's price
/// history is thousands of rows, and every pair has to be walked anyway, so a
/// per-pair round trip is not an option (the same pre-loading pattern as
/// [`db_unpriced_days`] and `FxRates`).
async fn db_duplicate_price_series(
    conn: &mut sqlx::SqliteConnection,
) -> Result<Vec<DuplicatePriceSeries>, sqlx::Error> {
    let rows = sqlx::query_as::<_, StoredClose>(
        "SELECT listing_id, price_date, price, origin FROM closing_prices \
         WHERE status = 'ok' ORDER BY listing_id, price_date",
    )
    .fetch_all(&mut *conn)
    .await?;
    if rows.is_empty() {
        return Ok(Vec::new());
    }
    let mut series: BTreeMap<i64, Vec<StoredClose>> = BTreeMap::new();
    for row in rows {
        series.entry(row.listing_id).or_default().push(row);
    }
    let tickers: HashMap<i64, String> =
        sqlx::query_as::<_, (i64, String)>("SELECT id, ticker FROM listings")
            .fetch_all(&mut *conn)
            .await?
            .into_iter()
            .collect();
    // The FK guarantees the listing exists; a row written between the two
    // queries is the only way to miss one, and an empty ticker still names the
    // ids to open.
    let ticker = |id: i64| tickers.get(&id).cloned().unwrap_or_default();

    let ids: Vec<i64> = series.keys().copied().collect();
    let mut duplicates = Vec::new();
    for (index, &listing_id) in ids.iter().enumerate() {
        for &other_listing_id in &ids[index + 1..] {
            let Some(run) = longest_identical_run(&series[&listing_id], &series[&other_listing_id])
            else {
                continue;
            };
            if run.length < DUPLICATE_PRICE_SERIES_RUN_DAYS || !run.varies {
                continue;
            }
            duplicates.push(DuplicatePriceSeries {
                listing_id,
                ticker: ticker(listing_id),
                other_listing_id,
                other_ticker: ticker(other_listing_id),
                identical_days: run.length,
                earliest_date: run.start,
                latest_date: run.end,
                fetched_days: run.length - run.manual_days,
                manual_days: run.manual_days,
                other_fetched_days: run.length - run.other_manual_days,
                other_manual_days: run.other_manual_days,
            });
        }
    }
    // Newest run first, matching the other lists here.
    duplicates.sort_by(|a, b| {
        b.latest_date
            .cmp(&a.latest_date)
            .then_with(|| a.listing_id.cmp(&b.listing_id))
            .then_with(|| a.other_listing_id.cmp(&b.other_listing_id))
    });
    Ok(duplicates)
}

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
/// `tax_deferred_amount` were entered from different statements. The row's
/// **kind** is part of it too — a dividend and a dividend equivalent of the
/// same amount on one day are two different payments, and the whole point of
/// the kind is that they are not the same thing (SCENARIOS J-10).
fn same_income_entry(a: &Income, b: &Income) -> bool {
    a.income_type == b.income_type
        && a.listing_id == b.listing_id
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

/// Whether two inheritances are one parcel entered twice — see
/// [`DuplicateInheritance`]. Every stored field except the id is compared, the
/// money figures as `Decimal`s rather than as the stored TEXT (so `"3000.0"`
/// and `"3000.00"` match). The cost-base *rule* is part of it too: the same
/// units and the same figure under `DeceasedCostBase` and `MarketValueAtDeath`
/// are two different claims about the same holding, which is a contradiction
/// worth showing rather than a duplicate to collapse.
fn same_inherited_parcel(a: &Inheritance, b: &Inheritance) -> bool {
    a.listing_id == b.listing_id
        && a.holding_account_id == b.holding_account_id
        && a.date_of_death == b.date_of_death
        && a.currency == b.currency
        && a.quantity == b.quantity
        && a.cost_base_rule == b.cost_base_rule
        && a.cost_base == b.cost_base
        && a.lpr_expenditure == b.lpr_expenditure
        && a.lpr_expenditure_date == b.lpr_expenditure_date
        && a.deceased_acquisition_date == b.deceased_acquisition_date
        && a.fx_rate == b.fx_rate
}

/// Inheritances recording one parcel twice — see [`DuplicateInheritance`].
///
/// Read on the caller's transaction and grouped in Rust for the same reason as
/// [`db_duplicate_ess_statements`]: the figures are part of the key and they
/// are TEXT decimals, which SQL would compare as strings. Only rows already
/// sharing a (listing, account, date of death) with another row are read, so a
/// portfolio of unrelated inheritances never reaches memory.
async fn db_duplicate_inheritances(
    conn: &mut sqlx::SqliteConnection,
) -> Result<Vec<DuplicateInheritance>, sqlx::Error> {
    let rows: Vec<Inheritance> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT {} FROM inheritances \
         WHERE EXISTS (SELECT 1 FROM inheritances o \
                       WHERE o.listing_id = inheritances.listing_id \
                         AND o.holding_account_id = inheritances.holding_account_id \
                         AND o.date_of_death = inheritances.date_of_death \
                         AND o.id <> inheritances.id) \
         ORDER BY inheritances.id",
        inheritance::COLUMNS
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

    let mut groups: Vec<Vec<Inheritance>> = Vec::new();
    for row in rows {
        match groups
            .iter_mut()
            .find(|group| same_inherited_parcel(&group[0], &row))
        {
            Some(group) => group.push(row),
            None => groups.push(vec![row]),
        }
    }
    let mut duplicates: Vec<DuplicateInheritance> = groups
        .into_iter()
        .filter(|group| group.len() > 1)
        .map(|group| {
            let first = &group[0];
            DuplicateInheritance {
                listing_id: first.listing_id,
                ticker: tickers.get(&first.listing_id).cloned().unwrap_or_default(),
                holding_account_id: first.holding_account_id,
                date_of_death: first.date_of_death,
                currency: first.currency.clone(),
                quantity: first.quantity,
                // What the parcel Buy carries: the first element plus the LPR
                // expenditure, the sum `inheritance::db_upsert` writes.
                cost_base_total: first.cost_base + first.lpr_expenditure,
                inheritance_count: group.len() as i64,
                inheritance_ids: group.iter().map(|row| row.id).collect(),
            }
        })
        .collect();
    // Newest first, matching the other duplicate lists.
    duplicates.sort_by(|a, b| {
        b.date_of_death
            .cmp(&a.date_of_death)
            .then_with(|| a.ticker.cmp(&b.ticker))
            .then_with(|| a.holding_account_id.cmp(&b.holding_account_id))
            .then_with(|| a.inheritance_ids.cmp(&b.inheritance_ids))
    });
    Ok(duplicates)
}

/// Disposals of ESS-vested shares inside the 30-day window — see
/// [`EssThirtyDaySale`].
///
/// Read on the caller's transaction, starting from the **vest parcels** rather
/// than from the allocations: each ESS statement's own vest Buy, plus every
/// parcel a rollover has since carried its units into
/// (`domain::rollover::replacement_descendants`). Following the chain is what
/// makes the alert work on the ordinary RSU path (SCENARIOS N-08) — vest into
/// the employer's plan account, move the shares to a personal broker account,
/// sell there — where the parcel actually sold carries no `ess_statement_id` of
/// its own, so joining `trades.ess_statement_id` to the allocation saw nothing
/// at all. Only the ESS parcels' own allocations are read, so an unrelated
/// disposal never reaches memory; the window itself is applied in Rust, since
/// the candidate set is now the small one.
///
/// The pairing is per **allocation × statement**: a Sell drawing on two vest
/// parcels inside their windows is two rows, since each statement would be
/// amended separately. Where one rollover moved several parcels at once, its
/// replacements descend from all of that group's sources (the data records no
/// finer link — see `replacement_descendants`), so such a sale can report
/// against more than one statement; the alert is advisory, and each row names
/// the vest parcel and statement it is about.
///
/// The statements are read separately rather than joined, so the discount is
/// summed by `ess_statement::discount_labels` — the tax summary's own definition
/// of the discount — instead of being re-added over TEXT columns here.
async fn db_ess_30_day_rule(
    conn: &mut sqlx::SqliteConnection,
) -> Result<Vec<EssThirtyDaySale>, sqlx::Error> {
    // Every vest parcel, and the statement behind it.
    let vests: Vec<(i64, i64)> = sqlx::query_as(
        "SELECT id, ess_statement_id FROM trades \
         WHERE ess_statement_id IS NOT NULL AND trade_type IN ('Buy', 'DRP') ORDER BY id",
    )
    .fetch_all(&mut *conn)
    .await?;
    if vests.is_empty() {
        return Ok(Vec::new());
    }
    // Which (vest parcel, statement) each parcel now holding ESS units answers
    // for — the vest parcel itself, and every replacement down the chain.
    let mut ess_parcels: HashMap<i64, Vec<(i64, i64)>> = HashMap::new();
    for (vest_trade_id, ess_statement_id) in vests {
        ess_parcels
            .entry(vest_trade_id)
            .or_default()
            .push((vest_trade_id, ess_statement_id));
        for descendant in rollover::replacement_descendants(&mut *conn, vest_trade_id).await? {
            ess_parcels
                .entry(descendant)
                .or_default()
                .push((vest_trade_id, ess_statement_id));
        }
    }

    // Their allocations, with the disposing Sell. A transfer-out Sell is not a
    // disposal at all and is excluded here (see `EssThirtyDaySale`); a
    // scrip-exchange/demerger closing Sell is kept and labelled.
    let placeholders = std::iter::repeat_n("?", ess_parcels.len())
        .collect::<Vec<_>>()
        .join(", ");
    let mut query = sqlx::query_as::<_, EssThirtyDaySaleRow>(sqlx::AssertSqlSafe(format!(
        "SELECT s.id AS sale_trade_id, s.listing_id AS listing_id, l.ticker AS ticker, \
                s.date AS sale_date, pa.quantity_allocated AS units_sold, \
                pa.purchase_trade_id AS parcel_id, \
                COALESCE(s.scrip_action_id, s.demerger_action_id) AS rollover_action_id \
         FROM parcel_allocations pa \
         JOIN trades s ON s.id = pa.sale_trade_id \
         JOIN listings l ON l.id = s.listing_id \
         WHERE pa.purchase_trade_id IN ({placeholders}) AND s.transfer_id IS NULL \
         ORDER BY s.date DESC, s.id"
    )));
    let mut parcel_ids: Vec<i64> = ess_parcels.keys().copied().collect();
    parcel_ids.sort_unstable();
    for id in &parcel_ids {
        query = query.bind(id);
    }
    let rows = query.fetch_all(&mut *conn).await?;
    if rows.is_empty() {
        return Ok(Vec::new());
    }
    // The whole (small) statement table, the same pre-loading call as the
    // ticker maps above: every candidate row needs its statement's figures.
    let statements: HashMap<i64, EssStatement> =
        sqlx::query_as::<_, EssStatement>(sqlx::AssertSqlSafe(format!(
            "SELECT {} FROM ess_statements",
            ess_statement::COLUMNS
        )))
        .fetch_all(&mut *conn)
        .await?
        .into_iter()
        .map(|s| (s.id, s))
        .collect();

    let mut alerts = Vec::new();
    for row in rows {
        // Every (vest parcel, statement) this parcel answers for; the map was
        // built from the same read, so the entry is always there.
        for &(vest_trade_id, ess_statement_id) in
            ess_parcels.get(&row.parcel_id).into_iter().flatten()
        {
            // A row read between the two queries is the only way to miss the
            // statement, and the alert would be unable to name what it is about.
            let statement = statements.get(&ess_statement_id).ok_or_else(|| {
                sqlx::Error::Decode(
                    format!("ESS statement {ess_statement_id} disappeared mid-read").into(),
                )
            })?;
            let days_after = (row.sale_date - statement.taxing_point_date).num_days();
            if !(1..=ESS_THIRTY_DAY_WINDOW).contains(&days_after) {
                continue;
            }
            alerts.push(EssThirtyDaySale {
                sale_trade_id: row.sale_trade_id,
                listing_id: row.listing_id,
                ticker: row.ticker.clone(),
                sale_date: row.sale_date,
                units_sold: row.units_sold,
                ess_statement_id,
                vest_trade_id,
                taxing_point_date: statement.taxing_point_date,
                days_after,
                currency: statement.currency.clone(),
                statement_discount: ess_statement::discount_labels(statement),
                statement_tax_year: tax_year_for(statement.taxing_point_date),
                disposal_tax_year: tax_year_for(row.sale_date),
                disposal_kind: match row.rollover_action_id {
                    Some(_) => EssDisposalKind::TakeoverOrRestructure,
                    None => EssDisposalKind::Sale,
                },
            });
        }
    }
    // Newest sale first, then the sale's own id, then the statement — the order
    // the SQL used to produce, now that the window is applied in Rust.
    alerts.sort_by(|a, b| {
        b.sale_date
            .cmp(&a.sale_date)
            .then_with(|| a.sale_trade_id.cmp(&b.sale_trade_id))
            .then_with(|| a.ess_statement_id.cmp(&b.ess_statement_id))
    });
    Ok(alerts)
}

/// Listings whose `currency` is not their exchange's — see
/// [`ExchangeCurrencyMismatch`].
///
/// One join on the caller's transaction, like the `duplicate_*` family. The
/// join to `exchanges` is what excludes exchange-less (Crypto) listings: they
/// have no exchange, so there is nothing for their currency to disagree with.
async fn db_exchange_currency_mismatches(
    conn: &mut sqlx::SqliteConnection,
) -> Result<Vec<ExchangeCurrencyMismatch>, sqlx::Error> {
    sqlx::query_as::<_, ExchangeCurrencyMismatch>(
        "SELECT l.id AS listing_id, l.ticker AS ticker, l.currency AS currency, \
                e.mic AS exchange_mic, e.currency AS exchange_currency \
         FROM listings l JOIN exchanges e ON e.mic = l.exchange_mic \
         WHERE l.currency <> e.currency \
         ORDER BY l.ticker, l.id",
    )
    .fetch_all(&mut *conn)
    .await
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
    let demergers_missing_close = db_demergers_missing_close(&mut tx).await?;
    let demergers_head_not_continuing = db_demergers_head_not_continuing(&mut tx).await?;
    let duplicate_price_series = db_duplicate_price_series(&mut tx).await?;
    let duplicate_actions = db_duplicate_actions(&mut tx).await?;
    let duplicate_amma_statements = db_duplicate_amma_statements(&mut tx).await?;
    let duplicate_income = db_duplicate_income(&mut tx).await?;
    let duplicate_interest = db_duplicate_interest(&mut tx).await?;
    let duplicate_expenses = db_duplicate_expenses(&mut tx).await?;
    let duplicate_ess_statements = db_duplicate_ess_statements(&mut tx).await?;
    let duplicate_inheritances = db_duplicate_inheritances(&mut tx).await?;
    let exchange_currency_mismatches = db_exchange_currency_mismatches(&mut tx).await?;
    let ess_30_day_rule = db_ess_30_day_rule(&mut tx).await?;
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
        demergers_missing_close,
        demergers_head_not_continuing,
        duplicate_price_series,
        duplicate_actions,
        duplicate_amma_statements,
        duplicate_income,
        duplicate_interest,
        duplicate_expenses,
        duplicate_ess_statements,
        duplicate_inheritances,
        exchange_currency_mismatches,
        ess_30_day_rule,
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
    use crate::entities::holding_account;
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
        assert!(h.duplicate_price_series.is_empty());
        assert!(h.duplicate_actions.is_empty());
        assert!(h.duplicate_amma_statements.is_empty());
        assert!(h.duplicate_income.is_empty());
        assert!(h.duplicate_interest.is_empty());
        assert!(h.duplicate_expenses.is_empty());
        assert!(h.duplicate_ess_statements.is_empty());
        assert!(h.ess_30_day_rule.is_empty());
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

    /// SCENARIOS Q-02: once a listing records the date the provider stopped
    /// quoting it, both lists go quiet from that date on — the errored rows
    /// and the never-fetched days after it are expected, not a to-do, and
    /// valuation carries the last close forward instead of blocking. Holes
    /// *before* the date are real and stay reported.
    #[tokio::test]
    async fn an_unpriced_listing_stops_being_nagged_about_from_its_date() {
        let pool = test_pool().await;
        test_support::listing(1).ticker("ATVI").insert(&pool).await;
        test_support::buy(1, 1)
            .date(ymd(2026, 7, 6))
            .insert(&pool)
            .await;
        insert_ok_price(&pool, 1, "2026-07-06").await;
        // A real hole before the delisting, then errored rows after it, then
        // days nothing ever fetched.
        insert_error_price(&pool, 1, "2026-07-07", "provider down").await;
        for day in ["2026-07-09", "2026-07-10"] {
            insert_error_price(&pool, 1, day, "yahoo fetch for ATVI failed: Not found").await;
        }

        let before = health(&pool, ymd(2026, 7, 13)).await;
        assert_eq!(before.errored_prices[0].errored_days, 3);
        assert_eq!(before.unpriced_days[0].unpriced_days, 2); // 8 and 13 July

        let marked = test_support::listing(1)
            .ticker("ATVI")
            .unpriced_from(ymd(2026, 7, 9))
            .build();
        crate::entities::listing::db_upsert(&pool, &marked)
            .await
            .unwrap();

        let after = health(&pool, ymd(2026, 7, 13)).await;
        assert_eq!(
            after.errored_prices.len(),
            1,
            "the 7 July hole predates the delisting and is still real"
        );
        assert_eq!(after.errored_prices[0].errored_days, 1);
        assert_eq!(after.errored_prices[0].latest_errored_date, ymd(2026, 7, 7));
        assert_eq!(after.errored_prices[0].latest_error, "provider down");
        assert_eq!(
            after.unpriced_days[0].unpriced_days, 1,
            "8 July stays a hole; 13 July is inside the unpriced run"
        );
        assert_eq!(after.unpriced_days[0].earliest_date, ymd(2026, 7, 8));
    }

    /// The mirror (migration 0037): once a listing records the date its
    /// provider series begins, both lists go quiet *before* it — nothing
    /// there can ever be fetched, and valuation leaves the holding out of
    /// those dates' totals instead of waiting. Holes from the date on are
    /// real and stay reported.
    #[tokio::test]
    async fn a_listing_is_not_nagged_about_before_its_series_begins() {
        let pool = test_pool().await;
        test_support::listing(1).ticker("LAC").insert(&pool).await;
        test_support::buy(1, 1)
            .date(ymd(2026, 7, 6))
            .insert(&pool)
            .await;
        // Errored rows on the days before the series begins, then a real hole
        // after it (10 July is never fetched).
        for day in ["2026-07-06", "2026-07-07"] {
            insert_error_price(&pool, 1, day, "yahoo fetch for LAC failed: 400").await;
        }
        insert_ok_price(&pool, 1, "2026-07-09").await;

        let before = health(&pool, ymd(2026, 7, 13)).await;
        assert_eq!(before.errored_prices[0].errored_days, 2);
        assert_eq!(before.unpriced_days[0].unpriced_days, 3); // 8, 10 and 13 July

        let marked = test_support::listing(1)
            .ticker("LAC")
            .unpriced_before(ymd(2026, 7, 9))
            .build();
        crate::entities::listing::db_upsert(&pool, &marked)
            .await
            .unwrap();

        let after = health(&pool, ymd(2026, 7, 13)).await;
        assert!(
            after.errored_prices.is_empty(),
            "both errored rows predate the series and are expected, not a to-do: {:?}",
            after.errored_prices
        );
        assert_eq!(
            after.unpriced_days[0].unpriced_days, 2,
            "8 July is before the series begins; 10 and 13 July are real holes"
        );
        assert_eq!(after.unpriced_days[0].earliest_date, ymd(2026, 7, 10));
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
    /// 2026-06-08) is not a trading day before the move, while the exchange
    /// moved to keeps no such holiday, so it is not its own hole.
    ///
    /// The destination is a second **AUD** market rather than the seeded XNYS
    /// because a rename across a currency boundary is refused
    /// (`RenameError::ExchangeCurrencyMismatch`, SCENARIOS R-01) — the
    /// calendar is what this test is about, not the currency.
    #[tokio::test]
    async fn a_hole_straddling_a_rename_uses_the_calendar_of_the_date() {
        let pool = test_pool().await;
        crate::entities::exchange::db_upsert(
            &pool,
            &crate::entities::exchange::Exchange {
                mic: "CXAX".to_string(),
                name: "Second AUD market".to_string(),
                country: "Australia".to_string(),
                currency: "AUD".to_string(),
                timezone: "Australia/Sydney".to_string(),
                settlement_days: 2,
                close_time: "16:00".to_string(),
            },
        )
        .await
        .unwrap();
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
                exchange_mic: Some("CXAX".to_string()),
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
        // Fri 06-05, Tue 06-09, then 06-10..06-12 under the new calendar —
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

    /// A demerger the price provider adjusted for, with no stated close to
    /// re-base its pre-demerger prices back: the rows are ok, so nothing else
    /// in health sees them, and a valuation of those dates is silently the
    /// current level (Evan's LAC history, ~2.46x understated). Adding the
    /// stated close both fixes the prices and clears the warning.
    #[tokio::test]
    async fn a_demerger_with_provider_adjusted_prices_and_no_stated_close_is_reported() {
        let pool = test_pool().await;
        test_support::listing(1).ticker("LAC").insert(&pool).await;
        test_support::listing(2).ticker("LAR").insert(&pool).await;
        // Two pre-demerger days served long after the demerger…
        for date in ["2023-09-29", "2023-10-02"] {
            test_support::closing_price(1, date.parse().unwrap())
                .price("10.13")
                .source("yahoo")
                .fetched_at("2026-07-26T07:44:56Z")
                .insert(&pool)
                .await;
        }
        // …one collected before it, which arrived contemporaneous…
        test_support::closing_price(1, "2023-09-28".parse().unwrap())
            .price("24.58")
            .source("yahoo")
            .fetched_at("2023-09-28T21:00:00Z")
            .insert(&pool)
            .await;
        // …and one after it, which the provider never restated.
        test_support::closing_price(1, "2023-10-04".parse().unwrap())
            .price("11.72")
            .source("yahoo")
            .fetched_at("2026-07-26T07:44:56Z")
            .insert(&pool)
            .await;
        insert_demerger(&pool, 1, 1, 2, ymd(2023, 10, 3), None).await;

        let h = health(&pool, ymd(2026, 7, 13)).await;
        assert_eq!(h.demergers_missing_close.len(), 1);
        let d = &h.demergers_missing_close[0];
        assert_eq!(d.action_id, 1);
        assert_eq!(d.ticker, "LAC");
        assert_eq!(d.demerger_date, ymd(2023, 10, 3));
        assert_eq!(
            d.adjusted_days, 2,
            "only pre-demerger days observed on or after the demerger are suspect"
        );
        assert_eq!(d.earliest_date, ymd(2023, 9, 29));
        assert_eq!(d.latest_date, ymd(2023, 10, 2));
        // No hand-entered row in the span, so the second figure is empty.
        assert_eq!(d.manual_days, 0);
        assert_eq!(d.manual_earliest_date, None);
        assert_eq!(d.manual_latest_date, None);
        // Nothing else in health notices: the rows are ok, not errored.
        assert!(h.errored_prices.is_empty());

        // Stating the close re-bases the prices and clears the alarm.
        insert_demerger(
            &pool,
            1,
            1,
            2,
            ymd(2023, 10, 3),
            Some((ymd(2023, 10, 2), "24.90")),
        )
        .await;
        let h = health(&pool, ymd(2026, 7, 13)).await;
        assert!(h.demergers_missing_close.is_empty());
    }

    /// The real shape of Evan's LAC history: the pre-demerger span holds both
    /// provider-fetched rows served after the demerger **and**, running back
    /// years earlier, hand-entered rows — copies of the demerged entity's
    /// series, entered to unblock stale snapshots, whose own `reason` says
    /// they are "unblocked, not accurate".
    ///
    /// The re-base walk skips manual rows by design, so counting only the
    /// fetched half published 260 where the affected span was 635 and made the
    /// alarm quietest about the rows already known to be wrong — and put the
    /// start of the exposure 18 months late. Both halves are reported, so the
    /// banner states what a stated close repairs and what it leaves.
    #[tokio::test]
    async fn a_demerger_reports_hand_entered_prices_in_the_span_separately() {
        let pool = test_pool().await;
        test_support::listing(1).ticker("LAC").insert(&pool).await;
        test_support::listing(2).ticker("LAR").insert(&pool).await;
        // The hand-entered half: the demerged entity's closes, copied in years
        // after the fact to unblock permanently stale snapshots.
        for date in ["2021-03-25", "2022-09-19"] {
            test_support::closing_price(1, date.parse().unwrap())
                .price("4.12")
                .fetched_at("2026-07-28T02:00:00Z")
                .manual(
                    "listing 8 (LAR) stored close for the same date",
                    "demerger-adjusted, so this period is unblocked, not accurate",
                )
                .insert(&pool)
                .await;
        }
        // The fetched half: what the provider served, after the demerger.
        for date in ["2022-09-20", "2023-10-02"] {
            test_support::closing_price(1, date.parse().unwrap())
                .price("10.13")
                .source("yahoo")
                .fetched_at("2026-07-26T07:44:56Z")
                .insert(&pool)
                .await;
        }
        insert_demerger(&pool, 1, 1, 2, ymd(2023, 10, 3), None).await;

        let resp = ApiClient::over(router().with_state(pool.clone()))
            .get("/reports/health")
            .await;
        assert_eq!(resp.status, StatusCode::OK);
        let body: serde_json::Value = resp.json();
        let d = &body["demergers_missing_close"][0];
        // What a stated close re-bases…
        assert_eq!(d["adjusted_days"], 2);
        assert_eq!(d["earliest_date"], "2022-09-20");
        assert_eq!(d["latest_date"], "2023-10-02");
        // …and what it does not, which starts 18 months earlier: the span the
        // operator has to deal with runs from the manual rows, not the fetched
        // ones.
        assert_eq!(d["manual_days"], 2);
        assert_eq!(d["manual_earliest_date"], "2021-03-25");
        assert_eq!(d["manual_latest_date"], "2022-09-19");

        let h = health(&pool, ymd(2026, 7, 13)).await;
        let d = &h.demergers_missing_close[0];
        assert_eq!(
            d.adjusted_days, 2,
            "the fetched figure counts only what the re-base walk touches"
        );
        assert_eq!(d.manual_earliest_date, Some(ymd(2021, 3, 25)));
        assert!(
            d.manual_earliest_date.unwrap() < d.earliest_date,
            "the suspect span starts at the earlier of the two halves"
        );
    }

    /// A demerger whose head listing has no provider-adjusted price is not a
    /// problem: nothing needs re-basing, so no statement is asked for — and a
    /// hand-entered pre-demerger row does not make one, since a stated close
    /// would not touch it. The manual figure is context on the warning, never
    /// a warning of its own.
    #[tokio::test]
    async fn a_demerger_with_no_adjusted_prices_is_not_reported() {
        let pool = test_pool().await;
        test_support::listing(1).ticker("LAC").insert(&pool).await;
        test_support::listing(2).ticker("LAR").insert(&pool).await;
        test_support::closing_price(1, "2023-09-29".parse().unwrap())
            .price("24.58")
            .source("yahoo")
            .fetched_at("2023-09-29T21:00:00Z")
            .insert(&pool)
            .await;
        test_support::closing_price(1, "2023-09-28".parse().unwrap())
            .price("24.10")
            .fetched_at("2026-07-28T02:00:00Z")
            .manual("the demerged entity's series", "no candle is served")
            .insert(&pool)
            .await;
        insert_demerger(&pool, 1, 1, 2, ymd(2023, 10, 3), None).await;

        let h = health(&pool, ymd(2026, 7, 13)).await;
        assert!(h.demergers_missing_close.is_empty());
    }

    /// A price the *demerged* listing was served by the provider, dated
    /// `date` — the evidence that it, not the head, traded before the
    /// demerger.
    async fn insert_fetched_price(pool: &SqlitePool, listing_id: i64, date: &str, price: &str) {
        test_support::closing_price(listing_id, date.parse().unwrap())
            .price(price)
            .source("yahoo")
            .fetched_at("2026-07-26T07:44:56Z")
            .insert(pool)
            .await;
    }

    /// Evan's LAC demerger as it was recorded for three years, reconstructed
    /// as it looks once the borrowed prices are cleared: the head listing is
    /// the entity the Arrangement **created** (a new corporation that took the
    /// original ticker) and the entity that actually **continued** is recorded
    /// as the one demerged from it.
    ///
    /// Nothing else can see this. No tax figure moves — the apportionment
    /// percentages attach to the tickers, not to the roles — and no write-time
    /// check has anything to test. The only trace is in the price series: the
    /// head parcel is held from 2021 on a listing whose provider series begins
    /// at the demerger, while the listing it supposedly created has history
    /// running back years before it. That inversion is the predicate.
    #[tokio::test]
    async fn a_demerger_recorded_with_the_created_entity_as_head_is_reported() {
        let pool = test_pool().await;
        // Listing 1 stands for LAC, the new corporation recorded as the head…
        test_support::listing(1)
            .ticker("LAC")
            .unpriced_before(ymd(2023, 10, 3))
            .insert(&pool)
            .await;
        // …listing 2 for the continuing entity, recorded as the demerged one.
        test_support::listing(2).ticker("LAR").insert(&pool).await;
        // The 2021 parcel, sitting on a listing that did not exist in 2021.
        test_support::buy(1, 1)
            .date(ymd(2021, 3, 25))
            .insert(&pool)
            .await;
        // The head's own series begins at the demerger, and not before it.
        insert_fetched_price(&pool, 1, "2023-10-03", "9.67").await;
        // The continuing entity's series runs back years before the event.
        for date in ["2021-03-25", "2022-09-19", "2023-10-02"] {
            insert_fetched_price(&pool, 2, date, "16.85").await;
        }
        insert_demerger(&pool, 1, 1, 2, ymd(2023, 10, 3), None).await;

        let h = health(&pool, ymd(2026, 7, 13)).await;
        assert_eq!(h.demergers_head_not_continuing.len(), 1);
        let d = &h.demergers_head_not_continuing[0];
        assert_eq!(d.action_id, 1);
        assert_eq!(d.listing_id, 1);
        assert_eq!(d.ticker, "LAC");
        assert_eq!(d.demerger_date, ymd(2023, 10, 3));
        assert_eq!(
            d.head_unpriced_before,
            Some(ymd(2023, 10, 3)),
            "the marker corroborates what the check infers"
        );
        assert_eq!(d.head_first_price_date, Some(ymd(2023, 10, 3)));
        assert_eq!(
            d.head_held_from,
            ymd(2021, 3, 25),
            "the day the model starts claiming a security its own series has not reached"
        );
        assert_eq!(d.demerger_listing_id, 2);
        assert_eq!(d.demerger_ticker, "LAR");
        assert_eq!(d.demerged_priced_days, 3);
        assert_eq!(d.demerged_earliest_date, ymd(2021, 3, 25));
        assert_eq!(d.demerged_latest_date, ymd(2023, 10, 2));

        // Re-modelled with the continuing entity as the head, the inversion is
        // gone: the head now holds the earlier series.
        insert_demerger(&pool, 1, 2, 1, ymd(2023, 10, 3), None).await;
        let h = health(&pool, ymd(2026, 7, 13)).await;
        assert!(h.demergers_head_not_continuing.is_empty());
    }

    /// The same finding over the HTTP surface the banner reads.
    #[tokio::test]
    async fn an_inverted_demerger_is_reported_over_http() {
        let pool = test_pool().await;
        test_support::listing(1).ticker("LAC").insert(&pool).await;
        test_support::listing(2).ticker("LAR").insert(&pool).await;
        test_support::buy(1, 1)
            .date(ymd(2021, 3, 25))
            .insert(&pool)
            .await;
        insert_fetched_price(&pool, 2, "2021-03-25", "16.85").await;
        insert_demerger(&pool, 1, 1, 2, ymd(2023, 10, 3), None).await;

        let body: serde_json::Value = ApiClient::over(router().with_state(pool.clone()))
            .get_json("/reports/health")
            .await;
        let d = &body["demergers_head_not_continuing"][0];
        assert_eq!(d["action_id"], 1);
        assert_eq!(d["ticker"], "LAC");
        assert_eq!(d["demerger_date"], "2023-10-03");
        assert_eq!(d["demerger_ticker"], "LAR");
        assert_eq!(d["demerged_priced_days"], 1);
        assert_eq!(d["head_held_from"], "2021-03-25");
        // No marker recorded, and no price of its own at all: both are
        // reported as absent rather than the row being withheld.
        assert_eq!(d["head_unpriced_before"], serde_json::Value::Null);
        assert_eq!(d["head_first_price_date"], serde_json::Value::Null);
    }

    /// The first false positive the predicate has to survive: a database where
    /// **no prices have been collected for anything**. Every head listing has
    /// no pre-demerger series there, so an absence-only check would light up
    /// on a fresh install. The demerged side's provider evidence is what stops
    /// it — nothing collected, nothing reported.
    #[tokio::test]
    async fn an_uncollected_database_reports_no_inverted_demerger() {
        let pool = test_pool().await;
        test_support::listing(1).ticker("LAC").insert(&pool).await;
        test_support::listing(2).ticker("LAR").insert(&pool).await;
        test_support::buy(1, 1)
            .date(ymd(2021, 3, 25))
            .insert(&pool)
            .await;
        insert_demerger(&pool, 1, 1, 2, ymd(2023, 10, 3), None).await;

        let h = health(&pool, ymd(2026, 7, 13)).await;
        assert!(h.demergers_head_not_continuing.is_empty());
    }

    /// A demerger recorded the right way round: the head is the entity that
    /// traded before the event, the demerged listing's series begins at it.
    /// This is the shape every correct demerger has, and it must never be
    /// reported.
    #[tokio::test]
    async fn a_correctly_modelled_demerger_is_not_reported() {
        let pool = test_pool().await;
        test_support::listing(1).ticker("LAR").insert(&pool).await;
        test_support::listing(2)
            .ticker("LAC")
            .unpriced_before(ymd(2023, 10, 3))
            .insert(&pool)
            .await;
        test_support::buy(1, 1)
            .date(ymd(2021, 3, 25))
            .insert(&pool)
            .await;
        for date in ["2021-03-25", "2023-10-02", "2023-10-03"] {
            insert_fetched_price(&pool, 1, date, "16.85").await;
        }
        insert_fetched_price(&pool, 2, "2023-10-03", "9.67").await;
        insert_demerger(&pool, 1, 1, 2, ymd(2023, 10, 3), None).await;

        let h = health(&pool, ymd(2026, 7, 13)).await;
        assert!(h.demergers_head_not_continuing.is_empty());
    }

    /// The second false positive: a demerger of a security whose pre-demerger
    /// history genuinely was **never backfilled**. Both listings' series begin
    /// at the demerger — an ordinary state, not a defect — so there is no
    /// asymmetry to report, however long the head has been held.
    #[tokio::test]
    async fn a_demerger_whose_history_was_never_backfilled_is_not_reported() {
        let pool = test_pool().await;
        test_support::listing(1).ticker("LAC").insert(&pool).await;
        test_support::listing(2).ticker("LAR").insert(&pool).await;
        test_support::buy(1, 1)
            .date(ymd(2021, 3, 25))
            .insert(&pool)
            .await;
        insert_fetched_price(&pool, 1, "2023-10-04", "9.67").await;
        insert_fetched_price(&pool, 2, "2023-10-04", "6.01").await;
        insert_demerger(&pool, 1, 1, 2, ymd(2023, 10, 3), None).await;

        let h = health(&pool, ymd(2026, 7, 13)).await;
        assert!(h.demergers_head_not_continuing.is_empty());
    }

    /// The third clause: a head listing with no pre-demerger series that was
    /// **not held** before the demerger either. Nothing is contradicted — the
    /// portfolio never claimed to own the security back there — so a listing
    /// whose history simply has not been collected is left alone.
    #[tokio::test]
    async fn a_head_listing_not_held_before_the_demerger_is_not_reported() {
        let pool = test_pool().await;
        test_support::listing(1).ticker("LAC").insert(&pool).await;
        test_support::listing(2).ticker("LAR").insert(&pool).await;
        // Acquired on the demerger date, not before it.
        test_support::buy(1, 1)
            .date(ymd(2023, 10, 3))
            .insert(&pool)
            .await;
        insert_fetched_price(&pool, 2, "2021-03-25", "16.85").await;
        insert_demerger(&pool, 1, 1, 2, ymd(2023, 10, 3), None).await;

        let h = health(&pool, ymd(2026, 7, 13)).await;
        assert!(h.demergers_head_not_continuing.is_empty());
    }

    /// The live data as it stood **before** the 2026-08-21 cleanup: the head
    /// listing's pre-demerger gap had been papered over with prices copied
    /// from the other listing. This check is silent there, and says so — a
    /// stored ok price before the demerger, however it got there, is an
    /// assertion that the security traded then.
    ///
    /// The two checks are complements, not alternatives, and this pins the
    /// hand-off: while the borrowed rows are there `duplicate_price_series`
    /// names them; once they are cleared this check names the modelling error
    /// underneath. Between them the incident is visible at both stages.
    #[tokio::test]
    async fn borrowed_pre_demerger_prices_are_left_to_the_duplicate_series_check() {
        let pool = test_pool().await;
        test_support::listing(1).ticker("LAC").insert(&pool).await;
        test_support::listing(2).ticker("LAR").insert(&pool).await;
        test_support::buy(1, 1)
            .date(ymd(2021, 3, 25))
            .insert(&pool)
            .await;
        // Thirty consecutive days of the continuing entity's series, fetched
        // under listing 2 and copied by hand onto listing 1 to unblock the
        // snapshots — exactly what was found on the live database.
        for (date, price) in moving_series(ymd(2023, 8, 1), 30, 1685) {
            test_support::closing_price(2, date)
                .price(&price)
                .source("yahoo")
                .fetched_at("2026-07-26T07:44:56Z")
                .insert(&pool)
                .await;
            test_support::closing_price(1, date)
                .price(&price)
                .fetched_at("2026-07-28T02:00:00Z")
                .manual(
                    "listing 2 (LAR) stored close for the same date",
                    "demerger-adjusted, so this period is unblocked, not accurate",
                )
                .insert(&pool)
                .await;
        }
        insert_demerger(&pool, 1, 1, 2, ymd(2023, 10, 3), None).await;

        let h = health(&pool, ymd(2026, 7, 13)).await;
        assert!(
            h.demergers_head_not_continuing.is_empty(),
            "a stored pre-demerger price, even a borrowed one, is an assertion the head traded then"
        );
        assert_eq!(
            h.duplicate_price_series.len(),
            1,
            "the borrowed rows are what the other check names while they are there"
        );

        // Clearing them — the runbook's own step — hands the incident over.
        sqlx::query("DELETE FROM closing_prices WHERE listing_id = 1")
            .execute(&pool)
            .await
            .unwrap();
        let h = health(&pool, ymd(2026, 7, 13)).await;
        assert!(h.duplicate_price_series.is_empty());
        assert_eq!(h.demergers_head_not_continuing.len(), 1);
        assert_eq!(h.demergers_head_not_continuing[0].ticker, "LAC");
    }

    /// A moving daily price series: `days` consecutive dates from `start`,
    /// the close stepping up a cent a day from `first_cents` — a series that
    /// *moves*, which is what the check requires before calling a run of
    /// matches a copied series.
    fn moving_series(start: NaiveDate, days: i64, first_cents: i64) -> Vec<(NaiveDate, String)> {
        (0..days)
            .map(|n| {
                let cents = first_cents + n;
                (
                    start + Duration::days(n),
                    format!("{}.{:02}", cents / 100, cents % 100),
                )
            })
            .collect()
    }

    /// Evan's LAC/LAR shape in miniature: listing 1 carries listing 2's price
    /// series, half of it hand-entered (copied, with the provenance saying so)
    /// and half fetched under a one-off symbol override that recorded nothing.
    /// Every row is `ok`, so no other list here sees any of it.
    #[tokio::test]
    async fn two_listings_holding_one_price_series_are_reported() {
        let pool = test_pool().await;
        test_support::listing(1).ticker("LAC").insert(&pool).await;
        test_support::listing(2).ticker("LAR").insert(&pool).await;
        let series = moving_series(ymd(2021, 3, 25), 40, 412);
        for (index, (date, price)) in series.iter().enumerate() {
            // The head listing's copy: the first 15 days typed in by hand…
            let row = test_support::closing_price(1, *date).price(price);
            if index < 15 {
                row.manual(
                    "listing 2 (LAR) stored close for the same date",
                    "the provider serves no candle here; unblocked, not accurate",
                )
                .insert(&pool)
                .await;
            } else {
                // …the rest fetched, with no record of the symbol used.
                row.source("yahoo").insert(&pool).await;
            }
            // The other listing's own series, all fetched.
            test_support::closing_price(2, *date)
                .price(price)
                .source("yahoo")
                .insert(&pool)
                .await;
        }
        // The day the two series part, and — months later — one genuine
        // coincidence: two securities really can close at the same figure.
        for (listing_id, price) in [(1, "20.00"), (2, "6.00")] {
            test_support::closing_price(listing_id, ymd(2021, 6, 1))
                .price(price)
                .source("yahoo")
                .insert(&pool)
                .await;
        }
        for listing_id in [1, 2] {
            test_support::closing_price(listing_id, ymd(2021, 7, 1))
                .price("4.12")
                .source("yahoo")
                .insert(&pool)
                .await;
        }

        let h = health(&pool, ymd(2026, 7, 13)).await;
        assert_eq!(h.duplicate_price_series.len(), 1);
        let d = &h.duplicate_price_series[0];
        assert_eq!((d.listing_id, d.ticker.as_str()), (1, "LAC"));
        assert_eq!((d.other_listing_id, d.other_ticker.as_str()), (2, "LAR"));
        assert_eq!(
            d.identical_days, 40,
            "the run, not the total: the later coincidence is not part of it"
        );
        assert_eq!(d.earliest_date, ymd(2021, 3, 25));
        assert_eq!(d.latest_date, ymd(2021, 3, 25) + Duration::days(39));
        // The two halves, which need different remedies: the hand-entered
        // rows say where they came from, the fetched ones say nothing.
        assert_eq!((d.manual_days, d.fetched_days), (15, 25));
        assert_eq!((d.other_manual_days, d.other_fetched_days), (0, 40));
        // Nothing else in health notices: every row is ok, not errored.
        assert!(h.errored_prices.is_empty());
        assert!(h.unpriced_days.is_empty());
    }

    /// The near-miss that must stay quiet: two distinct securities coinciding.
    /// Here they match on 49 days in all — more than the threshold — but never
    /// on 30 in a row, which is the only thing that is not a coincidence.
    #[tokio::test]
    async fn scattered_identical_closes_and_a_short_run_are_not_reported() {
        let pool = test_pool().await;
        test_support::listing(1).ticker("AAA").insert(&pool).await;
        test_support::listing(2).ticker("BBB").insert(&pool).await;
        // A run of 29 identical days — one short of the threshold.
        for (date, price) in moving_series(ymd(2021, 3, 25), 29, 412) {
            for listing_id in [1, 2] {
                test_support::closing_price(listing_id, date)
                    .price(&price)
                    .source("yahoo")
                    .insert(&pool)
                    .await;
            }
        }
        // Plus 60 later days on which every third close happens to agree: 20
        // more matches, none of them consecutive. The first of these days
        // disagrees, which is what ends the run above — a *disagreement* ends
        // a run, never a gap in the dates, since a day only one listing has a
        // price for is not a comparison at all.
        for (index, (date, price)) in moving_series(ymd(2022, 1, 1), 60, 500)
            .into_iter()
            .enumerate()
        {
            test_support::closing_price(1, date)
                .price(&price)
                .source("yahoo")
                .insert(&pool)
                .await;
            let other = if index % 3 == 1 {
                price
            } else {
                format!("9{price}")
            };
            test_support::closing_price(2, date)
                .price(&other)
                .source("yahoo")
                .insert(&pool)
                .await;
        }

        let h = health(&pool, ymd(2026, 7, 13)).await;
        assert!(
            h.duplicate_price_series.is_empty(),
            "49 matching days that never run 30 deep are a coincidence, not one series: {:?}",
            h.duplicate_price_series
        );
    }

    /// A day only one listing holds a price for is not a comparison: it
    /// neither breaks the run nor counts towards it.
    #[tokio::test]
    async fn a_day_only_one_listing_has_a_price_for_does_not_break_the_run() {
        let pool = test_pool().await;
        test_support::listing(1).ticker("LAC").insert(&pool).await;
        test_support::listing(2).ticker("LAR").insert(&pool).await;
        let series = moving_series(ymd(2021, 3, 25), 35, 412);
        let gap = series[17].0;
        for (date, price) in &series {
            test_support::closing_price(1, *date)
                .price(price)
                .source("yahoo")
                .insert(&pool)
                .await;
            if *date != gap {
                test_support::closing_price(2, *date)
                    .price(price)
                    .source("yahoo")
                    .insert(&pool)
                    .await;
            }
        }

        let h = health(&pool, ymd(2026, 7, 13)).await;
        assert_eq!(h.duplicate_price_series.len(), 1);
        let d = &h.duplicate_price_series[0];
        assert_eq!(
            d.identical_days, 34,
            "34 comparisons, the missing day not among them"
        );
        assert_eq!(d.earliest_date, series[0].0);
        assert_eq!(d.latest_date, series[34].0, "the run spans the gap");
    }

    /// Two instruments pinned to one figure — a pair of stablecoins at 1.00, two
    /// funds at a constant unit price — match forever without either series
    /// being a copy of the other. A run whose closes never move is not
    /// reported, because an alarm nobody can ever clear is worse than none.
    #[tokio::test]
    async fn two_listings_pinned_to_one_price_are_not_reported() {
        let pool = test_pool().await;
        test_support::listing(1).ticker("USDT").insert(&pool).await;
        test_support::listing(2).ticker("USDC").insert(&pool).await;
        let dates: Vec<NaiveDate> = (0..200)
            .map(|n| ymd(2021, 3, 25) + Duration::days(n))
            .collect();
        for date in &dates {
            for listing_id in [1, 2] {
                test_support::closing_price(listing_id, *date)
                    .price("1.00")
                    .source("yahoo")
                    .insert(&pool)
                    .await;
            }
        }
        let h = health(&pool, ymd(2026, 7, 13)).await;
        assert!(
            h.duplicate_price_series.is_empty(),
            "200 identical days that never move are two pinned instruments"
        );

        // The moment the shared series *moves* — and both still agree — it is
        // a series, not a peg, and the run is reported.
        for listing_id in [1, 2] {
            test_support::closing_price(listing_id, dates[100])
                .price("1.01")
                .source("yahoo")
                .insert(&pool)
                .await;
        }
        let h = health(&pool, ymd(2026, 7, 13)).await;
        assert_eq!(h.duplicate_price_series.len(), 1);
        assert_eq!(h.duplicate_price_series[0].identical_days, 200);
    }

    /// The check owns no marker of its own, like the rest of the `duplicate_*`
    /// family: `unpriced_before` does not silence it — that marker says the
    /// provider serves nothing before the date, which is a reason to *delete*
    /// the borrowed rows, not to accept them. Clearing them is what clears the
    /// warning, and it is served over the endpoint the banner polls.
    #[tokio::test]
    async fn api_the_warning_clears_when_the_borrowed_rows_go_not_when_the_marker_is_set() {
        let pool = test_pool().await;
        test_support::listing(1).ticker("LAC").insert(&pool).await;
        test_support::listing(2).ticker("LAR").insert(&pool).await;
        for (date, price) in moving_series(ymd(2021, 3, 25), 40, 412) {
            for listing_id in [1, 2] {
                test_support::closing_price(listing_id, date)
                    .price(&price)
                    .source("yahoo")
                    .insert(&pool)
                    .await;
            }
        }
        let client = ApiClient::over(router().with_state(pool.clone()));
        let body: serde_json::Value = client.get("/reports/health").await.json();
        let d = &body["duplicate_price_series"][0];
        assert_eq!(d["ticker"], "LAC");
        assert_eq!(d["other_ticker"], "LAR");
        assert_eq!(d["identical_days"], 40);
        assert_eq!(d["earliest_date"], "2021-03-25");
        assert_eq!(d["fetched_days"], 40);
        assert_eq!(d["manual_days"], 0);

        // Declaring the span unpriceable leaves the rows — and the warning.
        test_support::listing(1)
            .ticker("LAC")
            .with(|l| l.unpriced_before = Some(ymd(2021, 6, 1)))
            .insert(&pool)
            .await;
        let h = health(&pool, ymd(2026, 7, 13)).await;
        assert_eq!(
            h.duplicate_price_series.len(),
            1,
            "the marker explains an absence, not a stored figure that is another security's"
        );

        // Clearing the span is what clears it.
        assert_eq!(
            closing_price::db_clear_unpriced_before(&pool, 1)
                .await
                .unwrap(),
            closing_price::ClearOutcome::Cleared {
                unpriced_before: ymd(2021, 6, 1),
                deleted: 40,
            }
        );
        let h = health(&pool, ymd(2026, 7, 13)).await;
        assert!(h.duplicate_price_series.is_empty());
    }

    async fn insert_demerger(
        pool: &SqlitePool,
        id: i64,
        listing_id: i64,
        demerged_id: i64,
        date: NaiveDate,
        stated_close: Option<(NaiveDate, &str)>,
    ) {
        corporate_action::db_upsert(
            pool,
            &corporate_action::CorporateAction {
                id,
                listing_id,
                date,
                kind: corporate_action::ActionKind::Demerger {
                    demerger_listing_id: demerged_id,
                    demerger_new_units: dec("1"),
                    demerger_held_units: dec("1"),
                    demerger_cost_base_pct: dec("36"),
                    demerger_close_date: stated_close.map(|(d, _)| d),
                    demerger_close_price: stated_close.map(|(_, p)| dec(p)),
                    demerger_close_sourced_from: stated_close
                        .map(|_| "nyse.com daily close".to_string()),
                    demerger_close_reason: stated_close
                        .map(|_| "the provider adjusts the pre-demerger series".to_string()),
                },
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

    // The ESS 30-day rule (SCENARIOS J-04).

    /// Enters a 400-share statement worth $3.50 each (a $1,400 deferral
    /// discount — Example 11's figures) and vests it, answering the vest Buy a
    /// sale allocates from. Every vest a test needs is created **before** its
    /// sells: the vest Buy's id is assigned as max+1, so a sell inserted in
    /// between would have the next vest land on top of it.
    async fn ess_vest(
        pool: &SqlitePool,
        statement_id: i64,
        listing_id: i64,
        taxing_point: NaiveDate,
    ) -> i64 {
        test_support::ess_statement(statement_id, listing_id, taxing_point)
            .with(|s| {
                s.quantity = dec("400");
                s.market_value_per_share = dec("3.50");
                s.deferral_discount = dec("1400");
            })
            .insert(pool)
            .await;
        crate::entities::ess_vest::db_vest(pool, statement_id)
            .await
            .unwrap()
            .id
    }

    /// Sells `units` of a vest parcel on `sale_date`, allocating them to it.
    async fn sell_vest_parcel(
        pool: &SqlitePool,
        sale_id: i64,
        listing_id: i64,
        vest_trade_id: i64,
        sale_date: NaiveDate,
        units: Decimal,
    ) {
        sell_vest_parcel_in_account(
            pool,
            sale_id,
            listing_id,
            vest_trade_id,
            sale_date,
            units,
            1,
        )
        .await;
    }

    /// [`sell_vest_parcel`] in a named holding account — a Sell must sit in the
    /// account holding the parcel it allocates.
    async fn sell_vest_parcel_in_account(
        pool: &SqlitePool,
        sale_id: i64,
        listing_id: i64,
        vest_trade_id: i64,
        sale_date: NaiveDate,
        units: Decimal,
        account: i64,
    ) {
        test_support::sell(sale_id, listing_id)
            .date(sale_date)
            .qty(units)
            .price(dec("3.795"))
            .account(account)
            .insert(pool)
            .await;
        test_support::allocate(pool, sale_id, sale_id, vest_trade_id, units).await;
    }

    /// Example 11 itself, entered the way a user naturally would — the
    /// employer's *original* statement, then the sale 27 days later. Both the
    /// discount and its year are wrong, and a phantom capital gain appears;
    /// the alert names the sale, the statement it draws on, and the two years
    /// involved.
    #[tokio::test]
    async fn a_sale_inside_the_thirty_day_window_is_flagged_with_both_years() {
        let pool = test_pool().await;
        test_support::listing(1).ticker("PEPP").insert(&pool).await;
        let vest = ess_vest(&pool, 1, 1, ymd(2019, 6, 23)).await;
        sell_vest_parcel(&pool, 101, 1, vest, ymd(2019, 7, 20), dec("400")).await;

        let h = health(&pool, ymd(2019, 8, 1)).await;
        assert_eq!(h.ess_30_day_rule.len(), 1);
        let alert = &h.ess_30_day_rule[0];
        assert_eq!(alert.sale_trade_id, 101);
        assert_eq!(alert.ticker, "PEPP");
        assert_eq!(alert.sale_date, ymd(2019, 7, 20));
        assert_eq!(alert.units_sold, dec("400"));
        assert_eq!(alert.ess_statement_id, 1);
        assert_eq!(alert.taxing_point_date, ymd(2019, 6, 23));
        assert_eq!(alert.days_after, 27);
        assert_eq!(alert.currency, "AUD");
        assert_eq!(alert.statement_discount, dec("1400"));
        // The whole point of the rule: the discount is assessed in FY2019 as
        // entered, but belongs in FY2020 — two different returns.
        assert_eq!(alert.statement_tax_year, 2019);
        assert_eq!(alert.disposal_tax_year, 2020);
    }

    /// The window is 30 days **after** the taxing point, inclusive — ITAA 1997
    /// s 83A-115(3). Day 30 is inside it, day 31 is an ordinary post-vest sale
    /// whose gain is a real capital gain.
    #[tokio::test]
    async fn the_window_includes_day_thirty_and_excludes_day_thirty_one() {
        let pool = test_pool().await;
        test_support::listing(1).ticker("DAY30").insert(&pool).await;
        test_support::listing(2).ticker("DAY31").insert(&pool).await;
        let taxing_point = ymd(2026, 3, 10);
        let day30 = ess_vest(&pool, 1, 1, taxing_point).await;
        let day31 = ess_vest(&pool, 2, 2, taxing_point).await;
        sell_vest_parcel(&pool, 101, 1, day30, ymd(2026, 4, 9), dec("400")).await;
        sell_vest_parcel(&pool, 102, 2, day31, ymd(2026, 4, 10), dec("400")).await;

        let h = health(&pool, ymd(2026, 5, 1)).await;
        assert_eq!(h.ess_30_day_rule.len(), 1);
        assert_eq!(h.ess_30_day_rule[0].ticker, "DAY30");
        assert_eq!(h.ess_30_day_rule[0].days_after, 30);
        // Both sides of the boundary fall in one financial year here, which is
        // the ordinary case — the rule still applies, it just moves no return.
        assert_eq!(h.ess_30_day_rule[0].statement_tax_year, 2026);
        assert_eq!(h.ess_30_day_rule[0].disposal_tax_year, 2026);
    }

    /// The corrected entry must not nag: with the **amended** statement (taxing
    /// point = the disposal date) the rule's effect is a no-op, and that is
    /// exactly what `ato_examples::ess_30_day_rule_example_11_wyatt_amended_statement`
    /// enters. A sale of a non-ESS parcel is likewise none of this check's
    /// business.
    #[tokio::test]
    async fn a_same_day_sale_and_a_non_ess_parcel_are_not_flagged() {
        let pool = test_pool().await;
        test_support::listing(1).ticker("PEPP").insert(&pool).await;
        test_support::listing(2).ticker("ORD").insert(&pool).await;
        // The amended statement: taxing point *is* the disposal date.
        let vest = ess_vest(&pool, 1, 1, ymd(2019, 7, 20)).await;
        sell_vest_parcel(&pool, 101, 1, vest, ymd(2019, 7, 20), dec("400")).await;
        // An ordinary Buy sold the next day — no ESS statement behind it.
        test_support::buy(50, 2)
            .date(ymd(2019, 7, 20))
            .qty(dec("400"))
            .insert(&pool)
            .await;
        test_support::sell(51, 2)
            .date(ymd(2019, 7, 21))
            .qty(dec("400"))
            .insert(&pool)
            .await;
        test_support::allocate(&pool, 51, 51, 50, dec("400")).await;

        let h = health(&pool, ymd(2019, 8, 1)).await;
        assert!(h.ess_30_day_rule.is_empty());
    }

    /// One Sell drawing on two vest parcels inside their own windows is two
    /// alerts: each statement would be amended separately, so each is named.
    /// A partial disposal reports the units actually allocated, not the vest.
    #[tokio::test]
    async fn each_vest_parcel_a_sale_draws_on_is_named_separately() {
        let pool = test_pool().await;
        test_support::listing(1).ticker("EMPA").insert(&pool).await;
        let march2 = ess_vest(&pool, 1, 1, ymd(2026, 3, 2)).await;
        let march10 = ess_vest(&pool, 2, 1, ymd(2026, 3, 10)).await;
        test_support::sell(101, 1)
            .date(ymd(2026, 3, 20))
            .qty(dec("500"))
            .insert(&pool)
            .await;
        test_support::allocate(&pool, 1, 101, march2, dec("400")).await;
        test_support::allocate(&pool, 2, 101, march10, dec("100")).await;

        let h = health(&pool, ymd(2026, 5, 1)).await;
        assert_eq!(h.ess_30_day_rule.len(), 2);
        // Both sales are the same day, so the tie-break is the statement id.
        assert_eq!(h.ess_30_day_rule[0].ess_statement_id, 1);
        assert_eq!(h.ess_30_day_rule[0].days_after, 18);
        assert_eq!(h.ess_30_day_rule[0].units_sold, dec("400"));
        assert_eq!(h.ess_30_day_rule[1].ess_statement_id, 2);
        assert_eq!(h.ess_30_day_rule[1].days_after, 10);
        assert_eq!(h.ess_30_day_rule[1].units_sold, dec("100"));
    }

    /// SCENARIOS N-08: moving vested shares out of the employer's plan account
    /// into the holder's own broker account inside the window is **not** a
    /// disposal — the same beneficial owner holds the same interests, no CGT
    /// event arises, and s 83A-130 does not reach it either
    /// (`docs/ato/ess-takeovers-and-restructures.md`). It is also the move
    /// `entities::transfer` exists for, so flagging it nagged about the
    /// ordinary use of the feature and invited an amended return over a change
    /// of custody. A real sale in the same window still fires.
    #[tokio::test]
    async fn a_holding_account_transfer_inside_the_window_is_not_a_disposal() {
        let pool = test_pool().await;
        test_support::listing(1).ticker("ICE").insert(&pool).await;
        holding_account::db_upsert(
            &pool,
            &holding_account::HoldingAccount {
                id: 2,
                name: "ICE Employee Plan".to_string(),
            },
        )
        .await
        .unwrap();
        let taxing_point = ymd(2024, 3, 1);
        let vest = ess_vest(&pool, 1, 1, taxing_point).await;
        let group = crate::entities::transfer::db_transfer(
            &pool,
            1,
            &crate::entities::transfer::TransferBody {
                listing_id: 1,
                date: ymd(2024, 3, 11),
                from_account_id: 1,
                to_account_id: 2,
                allocations: vec![crate::entities::sell::AllocationInput {
                    purchase_trade_id: vest,
                    quantity_allocated: dec("400"),
                }],
                fee_allocations: Vec::new(),
                fee_market_price: None,
                fee_fx_rate: None,
            },
        )
        .await
        .unwrap();

        let h = health(&pool, ymd(2024, 4, 1)).await;
        assert!(
            h.ess_30_day_rule.is_empty(),
            "a transfer between the taxpayer's own accounts is not a disposal: {:?}",
            h.ess_30_day_rule
        );

        // A real sale of those same units after the move is a disposal, and is
        // reported as an ordinary one — reached through the transfer chain,
        // since the transferred-in parcel carries no `ess_statement_id` of its
        // own (SCENARIOS N-08).
        let transferred_in = group.transfer_ins[0].id;
        sell_vest_parcel_in_account(
            &pool,
            101,
            1,
            transferred_in,
            ymd(2024, 3, 20),
            dec("400"),
            2,
        )
        .await;
        let h = health(&pool, ymd(2024, 4, 1)).await;
        assert_eq!(h.ess_30_day_rule.len(), 1);
        assert_eq!(h.ess_30_day_rule[0].sale_trade_id, 101);
        assert_eq!(h.ess_30_day_rule[0].days_after, 19);
        assert_eq!(h.ess_30_day_rule[0].disposal_kind, EssDisposalKind::Sale);
        assert_eq!(h.ess_30_day_rule[0].vest_trade_id, vest);
    }

    /// A scrip-for-scrip exchange's closing Sell inside the window stays on the
    /// list, labelled: ITAA 1997 s 83A-130(2) normally treats the replacement
    /// interests as a continuation (so the taxing point does not move), but that
    /// rests on (4)/(9) facts this system does not record, and (5) makes a
    /// partial rollover's cash a disposal to that extent — advisory, not
    /// silence.
    #[tokio::test]
    async fn a_scrip_exchange_closing_sell_is_flagged_as_a_takeover_or_restructure() {
        let pool = test_pool().await;
        test_support::listing(1).ticker("OLD").insert(&pool).await;
        test_support::listing(2).ticker("NEW").insert(&pool).await;
        let taxing_point = ymd(2024, 3, 1);
        let vest = ess_vest(&pool, 1, 1, taxing_point).await;
        corporate_action::db_upsert(
            &pool,
            &corporate_action::CorporateAction {
                id: 1,
                listing_id: 1,
                date: ymd(2024, 3, 11),
                kind: corporate_action::ActionKind::ScripForScrip {
                    scrip_listing_id: 2,
                    scrip_new_units: Decimal::ONE,
                    scrip_old_units: Decimal::ONE,
                    scrip_cash_per_unit: None,
                    scrip_market_value: None,
                    scrip_cash_currency: None,
                },
            },
        )
        .await
        .unwrap();
        crate::entities::scrip_exchange::db_exchange(&pool, 1)
            .await
            .unwrap();

        let h = health(&pool, ymd(2024, 4, 1)).await;
        assert_eq!(h.ess_30_day_rule.len(), 1);
        let alert = &h.ess_30_day_rule[0];
        assert_eq!(alert.vest_trade_id, vest);
        assert_eq!(alert.days_after, 10);
        assert_eq!(alert.disposal_kind, EssDisposalKind::TakeoverOrRestructure);
    }

    // The deceased-estate side (SCENARIOS K-09).

    /// An inheritance of `quantity` units at a `cost_base`, all from the one
    /// 2015 acquisition and 2024 death.
    async fn insert_inheritance(
        pool: &SqlitePool,
        id: i64,
        listing_id: i64,
        account: i64,
        quantity: &str,
        cost_base: &str,
    ) {
        crate::entities::inheritance::db_upsert(
            pool,
            &Inheritance {
                id,
                listing_id,
                holding_account_id: account,
                quantity: dec(quantity),
                date_of_death: ymd(2024, 3, 1),
                cost_base_rule: crate::entities::inheritance::CostBaseRule::DeceasedCostBase,
                cost_base: dec(cost_base),
                lpr_expenditure: Decimal::ZERO,
                lpr_expenditure_date: None,
                deceased_acquisition_date: Some(ymd(2015, 5, 5)),
                currency: "AUD".to_string(),
                fx_rate: Decimal::ONE,
            },
        )
        .await
        .unwrap();
    }

    /// One inherited parcel entered twice: each row creates its own Buy, so
    /// the holding and its cost base are doubled with nothing else to notice.
    #[tokio::test]
    async fn one_inherited_parcel_entered_twice_is_reported() {
        let pool = test_pool().await;
        test_support::listing(1).ticker("INHA").insert(&pool).await;
        insert_inheritance(&pool, 1, 1, 1, "100", "3000").await;
        insert_inheritance(&pool, 2, 1, 1, "100", "3000").await;

        let h = health(&pool, ymd(2026, 7, 13)).await;
        assert_eq!(h.duplicate_inheritances.len(), 1);
        let d = &h.duplicate_inheritances[0];
        assert_eq!(d.ticker, "INHA");
        assert_eq!(d.date_of_death, ymd(2024, 3, 1));
        assert_eq!(d.quantity, dec("100"));
        assert_eq!(d.cost_base_total, dec("3000"));
        assert_eq!(d.inheritance_count, 2);
        assert_eq!(d.inheritance_ids, vec![1, 2]);
    }

    /// Two inheritances from one death that are not one entered twice: a
    /// different share of the estate, a different account, a different listing
    /// — and a different cost-base *rule*, which is a contradiction the
    /// warning does show.
    #[tokio::test]
    async fn inheritances_from_one_death_that_differ_are_not_reported() {
        let pool = test_pool().await;
        test_support::listing(1).ticker("INHA").insert(&pool).await;
        test_support::listing(2).ticker("INHB").insert(&pool).await;
        crate::entities::holding_account::db_upsert(
            &pool,
            &crate::entities::holding_account::HoldingAccount {
                id: 2,
                name: "Broker".to_string(),
            },
        )
        .await
        .unwrap();

        insert_inheritance(&pool, 1, 1, 1, "100", "3000").await;
        // A second parcel of the same listing from the same death, in a
        // different quantity — a part interest recorded in stages.
        insert_inheritance(&pool, 2, 1, 1, "60", "1800").await;
        // The same figures in another holding account, and on another listing.
        insert_inheritance(&pool, 3, 1, 2, "100", "3000").await;
        insert_inheritance(&pool, 4, 2, 1, "100", "3000").await;

        let h = health(&pool, ymd(2026, 7, 13)).await;
        assert!(
            h.duplicate_inheritances.is_empty(),
            "{:?}",
            h.duplicate_inheritances
        );
    }

    /// SCENARIOS R-01: a listing whose `currency` is not the currency its own
    /// exchange quotes in. The rename path refuses to create it, but a plain
    /// `PUT /listings/:id` compares the two columns nowhere, so a listing can
    /// be entered this way directly — and once it has history the freeze makes
    /// it permanent. Health names it wherever it came from.
    #[tokio::test]
    async fn a_listing_whose_currency_is_not_its_exchanges_is_reported() {
        let pool = test_pool().await;
        // Entered by a plain PUT: an AUD-quoting market holding a USD listing.
        test_support::listing(1)
            .ticker("CBA")
            .mic("XASX")
            .currency("USD")
            .insert(&pool)
            .await;

        let h = health(&pool, ymd(2026, 7, 13)).await;
        assert_eq!(h.exchange_currency_mismatches.len(), 1);
        let m = &h.exchange_currency_mismatches[0];
        assert_eq!(m.listing_id, 1);
        assert_eq!(m.ticker, "CBA");
        assert_eq!(m.currency, "USD");
        assert_eq!(m.exchange_mic, "XASX");
        assert_eq!(m.exchange_currency, "AUD");
    }

    /// A listing quoting the money its exchange does is silent, and so is a
    /// Crypto listing — it has no exchange for its currency to disagree with,
    /// which is the whole point of the exchange-less shape rather than a hole
    /// in the check.
    #[tokio::test]
    async fn matching_and_exchange_less_listings_are_not_reported() {
        let pool = test_pool().await;
        test_support::listing(1)
            .ticker("BHP")
            .mic("XASX")
            .currency("AUD")
            .insert(&pool)
            .await;
        test_support::listing(2)
            .ticker("MSFT")
            .mic("XNYS")
            .currency("USD")
            .insert(&pool)
            .await;
        // Exchange-less, and deliberately *not* denominated in the token: a
        // crypto holding is recorded in the fiat it is priced in.
        test_support::listing(3)
            .ticker("BTC")
            .security_type(crate::entities::listing::SecurityType::Crypto)
            .currency("AUD")
            .with(|l| l.exchange_mic = None)
            .insert(&pool)
            .await;

        let h = health(&pool, ymd(2026, 7, 13)).await;
        assert!(
            h.exchange_currency_mismatches.is_empty(),
            "{:?}",
            h.exchange_currency_mismatches
        );
    }

    /// Served over the endpoint the banner polls, since that is the only way
    /// the warning reaches anyone.
    #[tokio::test]
    async fn api_exchange_currency_mismatch_is_served_to_the_banner() {
        let pool = test_pool().await;
        test_support::listing(1)
            .ticker("CBA")
            .mic("XNYS")
            .currency("AUD")
            .insert(&pool)
            .await;
        let body: serde_json::Value = ApiClient::over(router().with_state(pool))
            .get("/reports/health")
            .await
            .json();
        let m = &body["exchange_currency_mismatches"][0];
        assert_eq!(m["ticker"], "CBA");
        assert_eq!(m["currency"], "AUD");
        assert_eq!(m["exchange_mic"], "XNYS");
        assert_eq!(m["exchange_currency"], "USD");
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
