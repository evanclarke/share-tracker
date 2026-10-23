//! Health / data-freshness report: the one read the web UI's cross-view
//! banner polls. Surfaces, in a single read transaction:
//!
//! - the latest stored ok closing-price date and whether it is stale (older
//!   than [`PRICE_STALE_BUSINESS_DAYS`](report::PRICE_STALE_BUSINESS_DAYS)
//!   business days — a coarse Mon–Fri count, deliberately ignoring
//!   per-exchange holiday calendars: this is a freshness alarm across every
//!   exchange and crypto, not a settlement calculation);
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
//! connection of the one read transaction
//! [`db_health`](report::db_health) begins, re-exported here so every
//! `reports::health::X` path is unchanged. The report itself — the freshness
//! figures, the assembly, and the route — is `report`, likewise re-exported;
//! this module is only declarations and re-exports.

mod calendar;
mod duplicates;
mod jobs;
mod prices;
mod report;
mod trades;

pub use calendar::*;
pub use duplicates::*;
pub use jobs::*;
pub use prices::*;
pub use report::{HealthReport, router};
pub use trades::*;

// Reached only by the tests below (and through `reports::health::X` from other
// modules' tests); gated to keep the non-test build warning-free.
#[cfg(test)]
use chrono::{DateTime, NaiveDate, Utc};
#[cfg(test)]
pub use report::db_health;
#[cfg(test)]
use report::{business_days_since, previous_month};
#[cfg(test)]
use sqlx::SqlitePool;

#[cfg(test)]
mod tests;
