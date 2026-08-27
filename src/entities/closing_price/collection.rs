//! Fetch-and-store: shared by scheduled collection, manual re-fetch and
//! backfill.
//!
//! One path from "ask the provider for these days" to "the rows are stored",
//! including the identity-segmented calls a rename requires, the price-basis
//! normalisation, and the errored-row recording that makes a failed day
//! visible instead of silent.

use super::db::{
    contemporaneous, db_ok_dates, db_price_basis_events, db_rebase_listing_prices, db_store,
};
use super::fetcher::{FetchError, FetchedClose, PriceFetcher};
use super::held::db_held_listing_ids;
use super::market::{Market, load_market};
use super::model::{ClosingPrice, PriceOrigin, PriceStatus, UNASSIGNED_ID};
use chrono::{DateTime, Duration, NaiveDate, Utc};
use rust_decimal::Decimal;
use sqlx::SqlitePool;
use std::collections::HashMap;

/// What to advise when a segment's symbol looks wrong — the remedy that can
/// actually reach **that** segment.
///
/// `price_symbol` is consulted only for the listing's *current* identity
/// (`yahoo_symbol_for`, so that an override matching today's ticker is never
/// applied to a pre-rename date), which means naming it for an earlier span
/// would be advice that cannot work: setting it would change nothing about
/// the failing fetch. There, only the backfill `symbol` override reaches the
/// call. Spans are unique by their start date, so that comparison identifies
/// the current one.
fn dead_symbol_remedy(market: &Market, from: NaiveDate) -> &'static str {
    if market.identity_at(from).from == market.current().from {
        "set price_symbol on the listing or backfill with an explicit symbol"
    } else {
        "this range predates the listing's current ticker, and price_symbol applies to that \
         ticker only, so backfill this range with an explicit symbol"
    }
}

/// Fetch the given trading days for a listing and store one row per requested
/// date: an ok row for a returned candle in the listing's currency, an errored
/// row for a fetch failure, a missing candle, or a currency mismatch. Returns
/// (ok, errored) counts.
///
/// The dates are split by [`Market::identity_segments`] and fetched with **one
/// provider call per identity** — a range straddling a rename is quoted under
/// the old symbol before the effective date and the new one after, so a
/// historical backfill recovers pre-rename history without the caller having
/// to supply the old symbol by hand.
///
/// Every stored row records the symbol its own segment was fetched under
/// ([`ClosingPrice::fetched_symbol`]), errored rows included — the symbol is
/// as much of the provenance of a failure as of a price.
pub(super) async fn fetch_and_store(
    pool: &SqlitePool,
    fetcher: &dyn PriceFetcher,
    market: &Market,
    dates: &[NaiveDate],
) -> Result<(usize, usize), sqlx::Error> {
    let (Some(&overall_from), Some(&overall_to)) = (dates.iter().min(), dates.iter().max()) else {
        return Ok((0, 0));
    };

    // Per requested date: the symbol its segment was fetched under (None only
    // when none could be resolved) and the segment's fetch outcome.
    let mut outcome: HashMap<NaiveDate, (Option<String>, Result<Decimal, String>)> = HashMap::new();
    for (from, to, _identity) in market.identity_segments(overall_from, overall_to) {
        let wanted: Vec<NaiveDate> = dates
            .iter()
            .copied()
            .filter(|d| *d >= from && *d <= to)
            .collect();
        if wanted.is_empty() {
            continue; // a segment the caller asked for no days in
        }
        // What the provider is actually asked for over this segment —
        // recorded on every row stored below, so a fetch made under a one-off
        // override is afterwards distinguishable from an ordinary one. Asked
        // of the fetcher, so it is always in the same namespace as the
        // `source` it is stored beside.
        let symbol = fetcher.symbol(market, from);
        let fetched = fetcher.daily_closes(market, from, to).await;
        let by_date: Result<HashMap<NaiveDate, FetchedClose>, FetchError> =
            fetched.map(|closes| closes.into_iter().map(|c| (c.date, c)).collect());

        // The provider says the symbol is wrong/renamed/delisted in two
        // different ways, and both get a message naming the symbol and the
        // remedy instead of a bare provider string:
        //
        //  - a **200 with zero candles** across the whole requested window (as
        //    opposed to a partial result with a data gap on one date), which
        //    would otherwise fall through to the day-by-day message below and
        //    read exactly like a transient outage; and
        //  - a **`NoSuchSymbol` failure** — the provider positively answering
        //    that it has no such series (`FetchError`). Yahoo answers that way
        //    for a ticker retired by a rename, so this is the path the
        //    pre-rename half of a straddling backfill actually takes.
        //
        // Anything else that failed keeps the provider's own words: an outage
        // is not evidence about the symbol. Judged per segment, so the message
        // names the symbol that actually failed and advises the remedy that
        // reaches *that* segment.
        let symbol_dead_or_wrong = matches!(&by_date, Ok(map) if map.is_empty());
        let named_symbol = || symbol.clone().unwrap_or_else(|e| e);
        let remedy = dead_symbol_remedy(market, from);
        let no_candles_message = || {
            format!(
                "provider returned no candles for {} over {from}..{to} — the symbol may be wrong, \
                 renamed, or delisted; {remedy}",
                named_symbol()
            )
        };
        let dead_symbol_message = |provider_error: &str| {
            format!(
                "{provider_error} — the provider serves no history for {} over {from}..{to}; the \
                 symbol may be wrong, renamed, or delisted; {remedy}",
                named_symbol()
            )
        };

        for date in wanted {
            let result = match &by_date {
                Err(FetchError::NoSuchSymbol(e)) => Err(dead_symbol_message(e)),
                Err(e) => Err(e.message().to_string()),
                Ok(_) if symbol_dead_or_wrong => Err(no_candles_message()),
                Ok(map) => match map.get(&date) {
                    None => {
                        Err("provider returned no candle for an expected trading day".to_string())
                    }
                    Some(close) if close.currency != market.listing.currency => Err(format!(
                        "currency mismatch: provider quoted {}, listing is {}",
                        close.currency, market.listing.currency
                    )),
                    Some(close) => Ok(close.price),
                },
            };
            outcome.insert(date, (symbol.as_ref().ok().cloned(), result));
        }
    }

    // The observation moment: what the row's `fetched_at` records, and what
    // dates the unit basis the provider's figures arrived in (module docs).
    let observed = Utc::now();
    let fetched_at = observed.to_rfc3339();
    // Scoped so the pooled connection is released before the writes below: an
    // in-memory pool holds a single connection, and keeping one here while
    // `db_store` asks for another would deadlock.
    let events = {
        let mut conn = pool.acquire().await?;
        db_price_basis_events(&mut conn, market.listing.id).await?
    };
    let (mut ok, mut errored) = (0, 0);
    for &date in dates {
        let (fetched_symbol, result) = outcome
            .remove(&date)
            .unwrap_or_else(|| (None, Err("no identity span covers this date".to_string())));
        let row = match result {
            Ok(as_observed) => {
                ok += 1;
                ClosingPrice {
                    id: UNASSIGNED_ID,
                    listing_id: market.listing.id,
                    price_date: date,
                    price: Some(contemporaneous(
                        as_observed,
                        &events,
                        date,
                        observed.date_naive(),
                    )),
                    price_as_observed: Some(as_observed),
                    source: fetcher.source().to_string(),
                    fetched_at: fetched_at.clone(),
                    fetched_symbol,
                    status: PriceStatus::Ok,
                    error: None,
                    origin: PriceOrigin::Fetched,
                    sourced_from: None,
                    reason: None,
                }
            }
            Err(e) => {
                errored += 1;
                ClosingPrice {
                    id: UNASSIGNED_ID,
                    listing_id: market.listing.id,
                    price_date: date,
                    price: None,
                    price_as_observed: None,
                    source: fetcher.source().to_string(),
                    fetched_at: fetched_at.clone(),
                    fetched_symbol,
                    status: PriceStatus::Error,
                    error: Some(e),
                    origin: PriceOrigin::Fetched,
                    sourced_from: None,
                    reason: None,
                }
            }
        };
        db_store(pool, &row).await?;
    }

    // The events were resolved from what was stored *before* this call, and a
    // demerger's factor is derived from one of these very rows — the provider's
    // figure for its stated close date. Backfilling a pre-demerger range
    // therefore has to look again once the range has landed, or a run that
    // fetched the reference day itself would store every other day of it in the
    // provider's adjusted basis. Re-deriving from `price_as_observed` is
    // idempotent, so this is a no-op (and writes nothing, so it stales no
    // snapshot and adds no audit row) in every case where the first pass was
    // already right.
    {
        let mut conn = pool.acquire().await?;
        db_rebase_listing_prices(&mut conn, market.listing.id).await?;
    }
    Ok((ok, errored))
}

/// How many **calendar** days back one collection run looks, so a day missed
/// outright (host down, provider outage) or stored errored is re-attempted by
/// the following runs instead of becoming a permanent hole. Ok rows are never
/// re-fetched, so the runs stay idempotent and the lookback costs nothing
/// once the window is filled.
///
/// `reports::snapshot::CATCHUP_LOOKBACK_DAYS` *is* this constant: the snapshot
/// job retries every blocked date in its window on every run, and a date it
/// retries but collection no longer refills is a date that can never be
/// unblocked without a manual backfill. Calendar days, not trading days, so
/// the two windows are directly comparable — seven trading days is only
/// nine-to-eleven calendar days, which used to leave the far end of the
/// snapshot window permanently unreachable.
pub const COLLECTION_LOOKBACK_DAYS: i64 = 14;

/// The listing's trading days over the last [`COLLECTION_LOOKBACK_DAYS`]
/// calendar days ending at its latest complete trading day at `now`, oldest
/// first. `None` when the market has no complete trading day (calendar
/// misconfiguration). Each day is tested against the calendar in force *then*
/// (`Market::identity_at`), so a window spanning an exchange change mixes both
/// exchanges' calendars correctly.
fn lookback_trading_days(
    market: &Market,
    now: DateTime<Utc>,
) -> Result<Option<Vec<NaiveDate>>, String> {
    let Some(latest) = market.latest_complete_trading_day(now)? else {
        return Ok(None);
    };
    let earliest = latest - Duration::days(COLLECTION_LOOKBACK_DAYS - 1);
    let mut days = Vec::new();
    let mut candidate = earliest;
    while candidate <= latest {
        if market.is_trading_day(candidate) {
            days.push(candidate);
        }
        candidate += Duration::days(1);
    }
    Ok(Some(days))
}

/// The listings held at any point over `from..=to` — the union of the holdings
/// as at each day in the window.
///
/// Collection needs this, not "held now": `reports::valuation` values a
/// snapshot date against the listings held *on that date*, so a listing sold
/// part-way through the window is still required to have prices for the days
/// before the sale. Taking only the live holdings dropped it from collection
/// the moment the Sell was entered — and with trades entered retroactively
/// from statements, that is the ordinary case, not an edge one.
async fn db_listing_ids_held_between(
    pool: &SqlitePool,
    from: NaiveDate,
    to: NaiveDate,
) -> Result<Vec<i64>, sqlx::Error> {
    let mut ids: std::collections::BTreeSet<i64> = std::collections::BTreeSet::new();
    let mut date = from;
    while date <= to {
        ids.extend(db_held_listing_ids(pool, Some(date)).await?);
        date += Duration::days(1);
    }
    Ok(ids.into_iter().collect())
}

/// One scheduled collection run: for every listing held at any point in the
/// lookback window, store the closing price of every trading day in that
/// window whose stored row is missing or errored (one provider call per
/// identity span; days already stored ok are never re-fetched). A non-trading
/// day stores no row and is not an error; a failed fetch stores an errored row
/// and fails the job (so the Jobs UI shows it), without stopping the other
/// listings — and is re-attempted by later runs while it stays in the window.
pub async fn run_collection(
    pool: &SqlitePool,
    fetcher: &dyn PriceFetcher,
    now: DateTime<Utc>,
) -> Result<(), String> {
    // The window is bounded by calendar dates, so one span over all listings
    // covers every market's own lookback regardless of its exchange calendar.
    let today = now.date_naive();
    let ids = db_listing_ids_held_between(
        pool,
        today - Duration::days(COLLECTION_LOOKBACK_DAYS),
        today,
    )
    .await
    .map_err(|e| e.to_string())?;

    let (mut stored, mut skipped) = (0, 0);
    let mut failures: Vec<String> = Vec::new();
    for listing_id in ids {
        let market = load_market(pool, listing_id)
            .await
            .map_err(|e| e.to_string())?
            .ok_or_else(|| format!("listing {listing_id} disappeared during collection"))?;

        let days = match lookback_trading_days(&market, now) {
            Ok(Some(days)) => days,
            Ok(None) => continue,
            Err(e) => {
                failures.push(e);
                continue;
            }
        };
        // A listing the provider has stopped quoting is not fetched from its
        // `unpriced_from` date on: every call would only store another
        // errored row, fail the job, and nag from `GET /reports/health`
        // forever (SCENARIOS Q-02). Valuation carries its last ok close
        // forward instead.
        let days: Vec<NaiveDate> = match market.listing.unpriced_from {
            Some(from) => days.into_iter().filter(|d| *d < from).collect(),
            None => days,
        };
        // …and the mirror at the other end: nothing is obtainable before the
        // provider's series begins (`listings.unpriced_before`, migration
        // 0037), so those days are not fetched either. Valuation excludes the
        // holding on them instead of waiting for a price that cannot arrive.
        let days: Vec<NaiveDate> = match market.listing.unpriced_before {
            Some(before) => days.into_iter().filter(|d| *d >= before).collect(),
            None => days,
        };
        let (Some(&from), Some(&to)) = (days.first(), days.last()) else {
            continue;
        };
        let already_ok = db_ok_dates(pool, listing_id, from, to)
            .await
            .map_err(|e| e.to_string())?;
        let needed: Vec<NaiveDate> = days
            .into_iter()
            .filter(|d| !already_ok.contains(d))
            .collect();
        if needed.is_empty() {
            skipped += 1;
            continue;
        }

        let (ok, errored) = fetch_and_store(pool, fetcher, &market, &needed)
            .await
            .map_err(|e| e.to_string())?;
        stored += ok;
        if errored > 0 {
            failures.push(format!(
                "{} ({}): fetch failed for {errored} day(s) in {from}..{to}, errored rows stored",
                market.listing.ticker, listing_id
            ));
        }
    }

    tracing::info!(
        stored,
        skipped,
        failed = failures.len(),
        "closing-price collection complete"
    );
    if failures.is_empty() {
        Ok(())
    } else {
        Err(failures.join("; "))
    }
}
