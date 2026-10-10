//! Price-series checks: errored and never-attempted closing prices, the two
//! demerger price-series checks, and one security's series stored under two
//! listings.

use crate::entities::closing_price::{self, HeldTimeline, PriceOrigin};
use crate::infra::decimal::Money;
use chrono::{DateTime, Duration, NaiveDate, Utc};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;
use std::collections::{BTreeMap, BTreeSet, HashSet};

/// How many consecutive shared trading days of *identical* closes make two
/// listings one series stored twice — see [`DuplicatePriceSeries`]. Thirty is
/// about six weeks of trading. Two genuinely distinct securities do coincide
/// to the cent now and then (Evan's LAC and LAR both closed at 4.12 on
/// 2024-02-08, four months after their series diverged), but a scattered
/// coincidence is not a run: matching on thirty *consecutive* comparisons is
/// not something two real securities do, and the observed incident ran to 634.
pub const DUPLICATE_PRICE_SERIES_RUN_DAYS: i64 = 30;

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
#[derive(utoipa::ToSchema, Debug, Serialize, Deserialize, sqlx::FromRow)]
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
#[derive(utoipa::ToSchema, Debug, Serialize, Deserialize)]
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
#[derive(utoipa::ToSchema, Debug, Serialize, Deserialize, sqlx::FromRow)]
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
#[derive(utoipa::ToSchema, Debug, Serialize, Deserialize, sqlx::FromRow)]
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
#[derive(utoipa::ToSchema, Debug, Serialize, Deserialize)]
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

/// Held days with no stored closing-price row at all, per listing (see
/// [`UnpricedListing`]).
///
/// Deliberately shaped as the exact question `reports::valuation` asks, so
/// there are no false positives: for every calendar date a listing was held,
/// the valuation day it resolves to must have a row. Days whose close is not
/// final yet (today's, an unsettled crypto candle) or final but not yet
/// published by the provider are out of scope — the walk stops at each
/// market's `latest_published_trading_day`, the same day collection fetches up
/// to, so a candle the next run will collect is never reported as a hole.
///
/// One holdings load and one stored-date query per listing, then an in-memory
/// walk: six years of history per listing is thousands of dates, so a per-day
/// round trip is not an option (the same pre-loading pattern as `FxRates` and
/// `RenameHistory`).
///
/// Not on the read transaction its caller uses: `load_market` is pool-based,
/// and this check tolerates a concurrent write far better than a financial
/// aggregation would — a hole is a hole whichever snapshot it is seen in.
pub(super) async fn db_unpriced_days(
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
            .latest_published_trading_day(now)
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
pub(super) async fn db_demergers_missing_close(
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
pub(super) async fn db_demergers_head_not_continuing(
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
pub(super) struct StoredClose {
    listing_id: i64,
    price_date: NaiveDate,
    #[sqlx(try_from = "Money")]
    price: Decimal,
    origin: PriceOrigin,
}

/// The longest run of identical closes between two listings, accumulated as
/// the two date-sorted series are walked together.
#[derive(Clone)]
pub(super) struct IdenticalRun {
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
pub(super) fn longest_identical_run(a: &[StoredClose], b: &[StoredClose]) -> Option<IdenticalRun> {
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
/// as the `duplicate_*` family's grouping (`duplicates::DuplicateCheck`): the prices are the key and they are TEXT
/// decimals, which SQL would compare as strings. The whole ok-price table is
/// loaded in one query and grouped per listing — a personal portfolio's price
/// history is thousands of rows, and every pair has to be walked anyway, so a
/// per-pair round trip is not an option (the same pre-loading pattern as
/// [`db_unpriced_days`] and `FxRates`).
pub(super) async fn db_duplicate_price_series(
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
    let tickers = crate::entities::listing::db_tickers(&mut *conn).await?;
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
