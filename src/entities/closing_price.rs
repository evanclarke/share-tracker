//! Daily closing-price history: one stored closing (or reference) price per
//! listing per trading day, plus the pluggable fetcher that collects them.
//!
//! # Provider: Yahoo Finance via the `yfinance-rs` crate
//!
//! Chosen 2026-06-07 (see TODO "Daily closing prices" clarifications): free and
//! keyless, and one provider covers all three held asset classes — NYSE tickers
//! plain (`ICE`), ASX via the `.AX` suffix (`BHP.AX`), crypto as
//! `<TICKER>-<quote currency>` (`BTC-AUD`). The endpoint is unofficial; the
//! `yfinance-rs` crate maintains the request format and the cookie/crumb
//! handling that bare HTTP clients trip over (a plain curl of the chart API
//! returns 429 without the crumb dance), and adds retries. Verified live
//! 2026-06-07 against BHP.AX / ICE / BTC-AUD: daily candles arrive with the
//! quote currency attached, crypto candles keyed on UTC midnight — exactly the
//! resolved crypto cut-off convention. Build note: `yfinance-rs` needs `protoc`
//! at build time (see Cargo.toml / ci.yml). The `PriceFetcher` trait is the
//! swap point if the provider breaks.
//!
//! # Conventions
//!
//! - A stored price is in the **listing's quote currency**, never AUD-converted
//!   (reports convert via the FX rules at read time). The provider's currency
//!   is cross-checked against the listing's; a mismatch is an errored row.
//! - A fetched row records the **provider symbol it was fetched under**
//!   ([`ClosingPrice::fetched_symbol`], migration 0038) — always, not only
//!   when it differs from the symbol the rename chain derives, so the question
//!   "what symbol produced this row?" has one answer rather than two readings
//!   of a null. It comes from the fetcher itself ([`PriceFetcher::symbol`]),
//!   so it is always in the namespace of the `source` beside it. A manual row
//!   carries none (nothing was fetched), and a row stored before 0038 carries
//!   none either — the symbol is not recoverable after the fact, and nothing
//!   invents one.
//! - `price_date` is the trading day in the exchange's timezone; for
//!   exchange-less (Crypto) listings it is the UTC date of the daily candle
//!   completing at 00:00 UTC at the end of that date (~10–11 am Sydney the
//!   next morning).
//! - A day's price is only collected once the exchange's `close_time` has
//!   passed in its timezone (crypto: once the UTC date has rolled over).
//! - Yahoo serves prices as float32-precision binary floats, so a raw value
//!   carries float noise (`62.4799995422363`); [`clean_price`] rounds to 7
//!   significant digits (counted from the first non-zero digit, so sub-$1
//!   token prices keep theirs) before storing.
//! - **A stored price is in its own trading day's unit basis** — the price the
//!   security actually traded at on `price_date`. The provider does not serve
//!   it that way: Yahoo restates a security's whole close series into the
//!   *current* basis the moment it splits (`auto_adjust(false)` turns off
//!   dividend adjustment only), so after a 10-for-1 it answers 120.888 for a
//!   day the security closed at 1208.88. The reports go the other way —
//!   `domain::open_parcels` re-bases parcel quantities into the snapshot
//!   date's own basis — so an unnormalised price was multiplied by units in a
//!   different basis and the product came out by the split ratio (SCENARIOS
//!   Q-14). Which basis a figure arrived in is fixed by *when it was
//!   observed*, and `fetched_at` dates that, so the row keeps the figure as
//!   observed and derives the stored one:
//!
//!       price = price_as_observed × the price re-basing ratio
//!                                   over (price_date, fetched_at]
//!
//!   Every restatement is therefore a recompute from the observation rather
//!   than a delta applied to an already-adjusted number
//!   ([`db_rebase_listing_prices`]): recording, editing or deleting a price
//!   re-basing action re-derives the same answer in any order, and a series
//!   collected day by day *before* one is left alone by it (its fetches
//!   predate the event, so its ratio is 1). The recovered figure carries only
//!   the provider's ~7 significant digits — see [`clean_price`] — so a
//!   re-fetch is no longer byte-identical to the provider's response.
//!
//! - **Which corporate actions restate the price series** — the set is a
//!   strict *superset* of the actions that re-base quantities
//!   (`corporate_action::adjustments`, whose module docs carry the same
//!   statement from the other side, and whose separate
//!   [`PriceBasisEvent`](crate::entities::corporate_action::PriceBasisEvent)
//!   type keeps the two apart at every call site):
//!
//!   - `ShareSplit` / `BonusIssue` restate it, by the same ratio they multiply
//!     the unit count by.
//!   - A **`Demerger`** restates it too — the provider applies a spin-off
//!     price-adjustment factor to the whole pre-demerger series exactly as it
//!     does for a split — while changing **no unit count** on this listing (it
//!     issues units of a *different* one). So there is no ratio to read: the
//!     factor is derived from the close the operator states the security
//!     actually traded at on the last pre-demerger trading day
//!     (`demerger_close_date` / `demerger_close_price`) divided by the
//!     provider's own adjusted figure for that same day
//!     ([`db_price_basis_events`]). Both sides are kept as facts and the
//!     quotient is computed at re-base time, so the close can be stated before
//!     the history is backfilled and re-derives itself if it is re-fetched.
//!     A demerger with no stated close restates nothing — its pre-demerger
//!     prices stay as the provider served them, which `GET /reports/health`
//!     reports as `demergers_missing_close`.
//!   - `ScripForScrip` and `WorthlessShares` do **not**: both end the original
//!     ticker, so the provider stops serving a series rather than restating
//!     one (the `listings.unpriced_from` case).
//!   - `ReturnOfCapital`, `RightsIssue` and `BuyBack` do **not**: a
//!     distribution goes through the provider's dividend-adjustment channel,
//!     which `auto_adjust(false)` turns off, and neither of the other two is
//!     in the provider's adjustment set at all.
//!
//!   The derived demerger factor is a `Decimal` division, so the recovered
//!   figures carry no more than the ~7 significant digits the provider gave
//!   (see [`clean_price`], which holds them to exactly that) *and* whatever
//!   the division itself rounds off — the price is recovered to about the
//!   accuracy of the stated close, not exactly.
//! - A failed fetch is stored as an errored row for that (listing, date) —
//!   never a silent zero or a skipped row — and is replaced by a later
//!   successful re-run.
//! - Only an **errored** row is deletable ([`db_delete`]): the acknowledgement
//!   that no price will ever exist for that day. An ok row is replaced by a
//!   re-fetch, never removed, so no valuation can lose a price it once had.
//!   The one relaxation, and its whole justification: a date inside the
//!   listing's `unpriced_before` span is **by declaration not valued at all**
//!   — the marker supersedes every stored row for the span and
//!   `reports::valuation` excludes the holding there rather than pricing it
//!   (migration 0037), and the carry-forward query is floored at the marker
//!   too — so there is no valued series to punch a hole in, and deleting is
//!   the acknowledgement that the stored figure never was a valuation. The
//!   span is the only place an ok row may be deleted, one date at a time or
//!   all at once ([`db_clear_unpriced_before`]). Note the asymmetry with
//!   `unpriced_from`: a date on or after **that** marker *is* valued — the
//!   last stored ok close is carried forward into it — so nothing is relaxed
//!   at that end.
//! - A day the provider cannot serve at all can be priced **by hand**
//!   (`PUT /closing_prices/{listing_id}/{price_date}`), recorded with where
//!   the figure was sourced from and why manual entry was needed
//!   ([`PriceOrigin::Manual`]). Valuation reads such a row exactly like a
//!   fetched one. The provider never takes the day back: collection and
//!   backfill skip it as an ok row, and an explicit re-fetch is refused — a
//!   manual price is changed only by entering another one. It is also
//!   contemporaneous **by declaration** — the operator states what the
//!   security traded at that day — so it is neither normalised on entry nor
//!   ever re-based: nothing rewrites a figure a person typed.

use crate::infra::http::ApiError;
use axum::{
    Extension, Json, Router,
    extract::{Path, Query, State},
    http::StatusCode,
    routing::{get, post, put},
};
use chrono::{DateTime, Datelike, Duration, NaiveDate, NaiveTime, Utc};
use chrono_tz::Tz;
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use sqlx::{QueryBuilder, Row, SqlitePool};
use std::{
    collections::{HashMap, HashSet},
    future::Future,
    pin::Pin,
    sync::Arc,
};

use crate::entities::{exchange, listing};
use crate::infra::decimal::{Money, OptMoney, parse_dec};

/// Whether a stored row carries a price or a fetch failure.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, sqlx::Type)]
#[sqlx(rename_all = "lowercase")]
#[serde(rename_all = "lowercase")]
pub enum PriceStatus {
    Ok,
    Error,
}

/// How a stored row came to be: fetched from the provider, or entered by hand
/// for a day the provider cannot serve.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, sqlx::Type)]
#[sqlx(rename_all = "lowercase")]
#[serde(rename_all = "lowercase")]
pub enum PriceOrigin {
    Fetched,
    Manual,
}

/// The `source` of a manually entered row — the provider slot, held in step
/// with `origin = "manual"` by a schema CHECK (0020).
pub const MANUAL_SOURCE: &str = "manual";

/// The [`ClosingPrice::id`] of a row built to be written: the surrogate key is
/// server-assigned, so [`db_store`] ignores the value and lets the database
/// assign a new id (or preserve the stored row's, on an upsert that updates).
pub const UNASSIGNED_ID: i64 = 0;

/// One stored closing price — or one recorded fetch failure (`status =
/// "error"`, `price` null, `error` set).
#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct ClosingPrice {
    /// Server-assigned surrogate key (0021): the row's identity for the audit
    /// trail (`row_history.row_id`, so `POST /reports/row_history` can be
    /// keyed on it). Writes address a row by its `(listing_id, price_date)`
    /// natural key, never by this — [`db_store`] ignores the value it is
    /// handed and lets the database assign or preserve it.
    #[serde(default)]
    pub id: i64,
    pub listing_id: i64,
    pub price_date: NaiveDate,
    /// Closing price in the listing's quote currency, in the unit basis in
    /// force on [`Self::price_date`]; None exactly when the fetch failed.
    #[sqlx(try_from = "OptMoney")]
    pub price: Option<Decimal>,
    /// The figure exactly as the provider served it (or as the operator
    /// entered it), in the unit basis in force when it was observed — which
    /// [`Self::fetched_at`] dates. [`Self::price`] is derived from it by the
    /// re-basing actions dated in `(price_date, fetched_at]`, so a split
    /// recorded, edited or deleted later restates the price from here rather
    /// than from itself (see the module docs). Equal to `price` for a manual
    /// row, and None exactly when the fetch failed.
    #[sqlx(try_from = "OptMoney")]
    pub price_as_observed: Option<Decimal>,
    /// Provider that produced the row, e.g. "yahoo" — [`MANUAL_SOURCE`]
    /// exactly when `origin` is `Manual`.
    pub source: String,
    /// RFC 3339 UTC timestamp of the fetch that produced the row — for a
    /// manual row, of the entry that recorded it.
    pub fetched_at: String,
    /// The provider symbol this row was fetched under (migration 0038), in
    /// the namespace of [`Self::source`] — recorded on every fetched row, so
    /// a backfill made with the one-off `symbol` override is afterwards
    /// distinguishable from an ordinary fetch. Informational: no calculation
    /// reads it; it is provenance, served by `GET /closing_prices`, shown on
    /// the Closing Prices screen and carried into `row_history`.
    ///
    /// None for a manual row (nothing was fetched — a schema CHECK pairs the
    /// two), for a row stored before 0038 (unrecorded, and not recoverable
    /// after the fact), and for the errored row a fetch stores when no symbol
    /// could be resolved at all — an exchange with no provider mapping, which
    /// the row's own `error` names.
    pub fetched_symbol: Option<String>,
    pub status: PriceStatus,
    /// Failure detail; None exactly when the fetch succeeded.
    pub error: Option<String>,
    pub origin: PriceOrigin,
    /// Where a manual price was sourced from; None exactly when `origin` is
    /// `Fetched`.
    pub sourced_from: Option<String>,
    /// Why manual entry was needed; None exactly when `origin` is `Fetched`.
    pub reason: Option<String>,
}

// ---------------------------------------------------------------------------
// Market context: a listing plus the exchange-calendar data price collection
// needs (timezone, close time, holidays).
//
// A listing that has been renamed traded under a different ticker — and, for
// an exchange-code change, on a different calendar — before the rename took
// effect (`listing_renames`, `domain::listing_identity`). So a market is not
// one exchange but a *timeline* of identities: resolving a historical date
// against today's identity asks the provider for a symbol that was never
// quoted then, and walks a calendar that was not in force. `exchange` is None
// exactly for an exchange-less (Crypto) span.
// ---------------------------------------------------------------------------

/// One span of a listing's history: the ticker and exchange in force from
/// [`MarketIdentity::from`] until the next span begins.
pub struct MarketIdentity {
    /// First date this identity was in effect; `None` for the earliest span,
    /// which reaches back indefinitely.
    pub from: Option<NaiveDate>,
    pub ticker: String,
    /// The MIC the security traded under over this span; `None` exactly for
    /// an exchange-less (Crypto) span. The provider-symbol mapping keys off
    /// this, so it answers even when the `exchanges` row itself is absent.
    pub exchange_mic: Option<String>,
    /// The `exchanges` row for `exchange_mic`, when there is one: the source
    /// of the timezone, close time and (with `holidays`) the trading calendar.
    pub exchange: Option<exchange::Exchange>,
    pub holidays: HashSet<NaiveDate>,
}

impl MarketIdentity {
    /// The timezone trading days are keyed on: the exchange's, or UTC for
    /// exchange-less (Crypto) listings, per the resolved cut-off convention.
    fn tz(&self) -> Result<Tz, String> {
        match &self.exchange {
            None => Ok(chrono_tz::UTC),
            Some(ex) => ex.timezone.parse().map_err(|_| {
                format!(
                    "exchange {} has unrecognised timezone {:?}",
                    ex.mic, ex.timezone
                )
            }),
        }
    }

    /// Whether `date` is a trading day under this identity: crypto trades
    /// every day; an exchange trades on weekdays that are not seeded public
    /// holidays.
    fn is_trading_day(&self, date: NaiveDate) -> bool {
        if self.exchange.is_none() {
            return true;
        }
        let weekday = date.weekday();
        weekday != chrono::Weekday::Sat
            && weekday != chrono::Weekday::Sun
            && !self.holidays.contains(&date)
    }
}

pub struct Market {
    pub listing: listing::Listing,
    /// The listing's identity spans, ascending by `from` and contiguous; the
    /// last is the one in effect now. Always non-empty — a listing with no
    /// recorded rename has exactly one open-ended span.
    identities: Vec<MarketIdentity>,
    /// One-off provider-symbol override for this fetch only — not persisted
    /// (contrast `listing.price_symbol`, which is stored). Set by the
    /// backfill endpoint's optional `symbol` for a provider spelling the
    /// rename chain doesn't record; `load_market` always leaves this `None`.
    pub symbol_override: Option<String>,
}

impl Market {
    /// The identity in effect on `date` — the latest span that had started by
    /// then, falling back to the earliest (which is open-ended, so this only
    /// matters for a date before a chain the listing somehow starts after).
    pub fn identity_at(&self, date: NaiveDate) -> &MarketIdentity {
        self.identities
            .iter()
            .rev()
            .find(|i| i.from.is_none_or(|from| from <= date))
            .unwrap_or_else(|| self.earliest())
    }

    fn earliest(&self) -> &MarketIdentity {
        self.identities.first().expect("always at least one span")
    }

    /// The identity in effect now — the one `now`-relative questions (the
    /// close time, the timezone the current date is read in) are answered
    /// against.
    pub fn current(&self) -> &MarketIdentity {
        self.identities.last().expect("always at least one span")
    }

    /// Split `from..=to` into maximal sub-ranges that each sit wholly inside
    /// one identity, ascending. One provider call is made per sub-range, so a
    /// range straddling a rename is fetched under each symbol that was
    /// actually quoted over it. An unrenamed listing yields exactly one.
    pub fn identity_segments(
        &self,
        from: NaiveDate,
        to: NaiveDate,
    ) -> Vec<(NaiveDate, NaiveDate, &MarketIdentity)> {
        let mut segments = Vec::new();
        if from > to {
            return segments;
        }
        let mut start = from;
        while start <= to {
            let identity = self.identity_at(start);
            // The segment runs until the next span begins, or to `to`.
            let next_start = self
                .identities
                .iter()
                .filter_map(|i| i.from)
                .filter(|f| *f > start)
                .min();
            let end = match next_start {
                Some(next) => to.min(next - Duration::days(1)),
                None => to,
            };
            segments.push((start, end, identity));
            start = end + Duration::days(1);
        }
        segments
    }

    /// Whether `date` is a trading day on the calendar that was in force then.
    fn is_trading_day(&self, date: NaiveDate) -> bool {
        self.identity_at(date).is_trading_day(date)
    }

    /// The market's nearest trading day at or before `date` (the day whose
    /// closing price values a holding on `date`). `None` only if no trading
    /// day exists in the year before `date` (a calendar misconfiguration,
    /// e.g. every day seeded as a holiday).
    pub fn latest_trading_day_on_or_before(&self, date: NaiveDate) -> Option<NaiveDate> {
        let mut candidate = date;
        for _ in 0..366 {
            if self.is_trading_day(candidate) {
                return Some(candidate);
            }
            candidate -= Duration::days(1);
        }
        None
    }

    /// The most recent trading day whose closing price is final at `now`.
    ///
    /// Exchange-listed: the current date in the exchange's timezone if its
    /// `close_time` has passed, else the previous date — then walked back to
    /// the nearest trading day. Crypto: yesterday's UTC date (the daily candle
    /// for date D completes at 00:00 UTC at the end of D); every day trades.
    /// Read against the *current* identity: "now" is by definition after any
    /// rename, so the close that matters is the one in force today.
    ///
    /// `None` only if no trading day exists in the past year (a calendar
    /// misconfiguration, e.g. every day seeded as a holiday).
    pub fn latest_complete_trading_day(
        &self,
        now: DateTime<Utc>,
    ) -> Result<Option<NaiveDate>, String> {
        let current = self.current();
        let candidate = match &current.exchange {
            None => now.date_naive() - Duration::days(1),
            Some(ex) => {
                let close = NaiveTime::parse_from_str(&ex.close_time, "%H:%M").map_err(|_| {
                    format!(
                        "exchange {} has malformed close_time {:?}",
                        ex.mic, ex.close_time
                    )
                })?;
                let now_local = now.with_timezone(&current.tz()?);
                if now_local.time() >= close {
                    now_local.date_naive()
                } else {
                    now_local.date_naive() - Duration::days(1)
                }
            }
        };
        Ok(self.latest_trading_day_on_or_before(candidate))
    }
}

#[cfg(test)]
impl Market {
    /// A market with an explicit identity timeline, for tests that exercise
    /// renames without going through the database.
    pub(crate) fn from_identities(
        listing: listing::Listing,
        identities: Vec<MarketIdentity>,
    ) -> Self {
        assert!(!identities.is_empty(), "a market needs at least one span");
        Market {
            listing,
            identities,
            symbol_override: None,
        }
    }

    /// A never-renamed market: one open-ended span taking its ticker and MIC
    /// from the listing row.
    pub(crate) fn unrenamed(
        listing: listing::Listing,
        exchange: Option<exchange::Exchange>,
        holidays: HashSet<NaiveDate>,
    ) -> Self {
        let identity = MarketIdentity {
            from: None,
            ticker: listing.ticker.clone(),
            exchange_mic: listing.exchange_mic.clone(),
            exchange,
            holidays,
        };
        Market::from_identities(listing, vec![identity])
    }
}

/// Load the market context for a listing — its identity timeline, with each
/// span's exchange and holiday calendar; None if the listing doesn't exist.
pub async fn load_market(
    pool: &SqlitePool,
    listing_id: i64,
) -> Result<Option<Market>, sqlx::Error> {
    let Some(listing) = listing::db_get(pool, listing_id).await? else {
        return Ok(None);
    };
    let mut conn = pool.acquire().await?;
    let renames = crate::domain::listing_identity::RenameHistory::load(&mut conn).await?;
    drop(conn);
    let spans = renames.identities(
        listing_id,
        crate::domain::listing_identity::Identity {
            from: None,
            ticker: listing.ticker.clone(),
            exchange_mic: listing.exchange_mic.clone(),
        },
    );

    // One exchange (and one holiday load) per distinct MIC across the
    // timeline, not one per span — a chain that renames within the same
    // exchange must not re-read its calendar for every link.
    let mut exchanges: HashMap<String, Option<exchange::Exchange>> = HashMap::new();
    let mut holidays: HashMap<String, HashSet<NaiveDate>> = HashMap::new();
    let mut identities = Vec::with_capacity(spans.len());
    for span in spans {
        let (exchange, holiday_dates) = match &span.exchange_mic {
            None => (None, HashSet::new()),
            Some(mic) => {
                if !exchanges.contains_key(mic) {
                    exchanges.insert(mic.clone(), exchange::db_get(pool, mic).await?);
                    holidays.insert(
                        mic.clone(),
                        crate::entities::exchange_holiday::db_holiday_dates_for(pool, mic).await?,
                    );
                }
                (
                    exchanges[mic].clone(),
                    holidays.get(mic).cloned().unwrap_or_default(),
                )
            }
        };
        identities.push(MarketIdentity {
            from: span.from,
            ticker: span.ticker,
            exchange_mic: span.exchange_mic,
            exchange,
            holidays: holiday_dates,
        });
    }

    Ok(Some(Market {
        listing,
        identities,
        symbol_override: None,
    }))
}

// ---------------------------------------------------------------------------
// The pluggable price fetcher
// ---------------------------------------------------------------------------

/// One daily close as returned by a provider, before it is checked and stored.
#[derive(Debug, Clone)]
pub struct FetchedClose {
    pub date: NaiveDate,
    pub price: Decimal,
    /// The quote currency the provider reports — cross-checked against the
    /// listing's currency before the price is stored.
    pub currency: String,
}

pub type FetchFuture<'a> =
    Pin<Box<dyn Future<Output = Result<Vec<FetchedClose>, String>> + Send + 'a>>;

/// The most recent available price for a listing, for on-demand live
/// valuation: the price in the listing's quote currency and the provider's
/// quote timestamp (the as-of moment the price was observed).
#[derive(Debug, Clone)]
pub struct LatestQuote {
    pub price: Decimal,
    /// The quote currency the provider reports — cross-checked against the
    /// listing's currency before the price is used.
    pub currency: String,
    /// The provider's quote timestamp (the as-of moment).
    pub as_of: DateTime<Utc>,
}

pub type QuoteFuture<'a> = Pin<Box<dyn Future<Output = Result<LatestQuote, String>> + Send + 'a>>;

/// A source of prices. Implementations do their own symbol mapping and
/// candle-timestamp→trading-date conversion (both are provider-specific); a
/// failure is an error result, never a silent zero or a skipped row.
pub trait PriceFetcher: Send + Sync {
    /// Identifier stored in each row's `source` column, e.g. "yahoo".
    fn source(&self) -> &'static str;

    /// The symbol this provider is asked for when quoting `market` as at
    /// `date` — the listing's rename chain resolved into the provider's own
    /// spelling, or whatever override is in force. `Err` when no symbol can
    /// be resolved at all (an exchange with no mapping), with the reason.
    ///
    /// The counterpart of [`Self::source`]: that names the provider, this
    /// names what was asked of it. [`fetch_and_store`] records the answer on
    /// every row it stores (`fetched_symbol`), which is why it is asked of
    /// the fetcher rather than derived beside it — the two columns must
    /// always be in the same namespace.
    fn symbol(&self, market: &Market, date: NaiveDate) -> Result<String, String>;

    /// Daily closes for the listing over `from..=to` (trading-day dates in the
    /// market's timezone convention). Non-trading days in the range simply
    /// have no entry.
    ///
    /// `from..=to` must sit wholly inside **one** of the market's identities
    /// (`Market::identity_segments` is what guarantees it): the provider is
    /// asked for a single symbol per call, and after a rename that symbol
    /// differs either side of the effective date. Implementations resolve the
    /// symbol at `from`.
    fn daily_closes<'a>(
        &'a self,
        market: &'a Market,
        from: NaiveDate,
        to: NaiveDate,
    ) -> FetchFuture<'a>;

    /// The most recent available price for the listing, with the provider's
    /// quote timestamp — for exchange-listed and exchange-less (Crypto)
    /// listings alike. Drives on-demand live valuation (the price-dependent
    /// reports). A failure is an error result, never a silent zero.
    fn latest_quote<'a>(&'a self, market: &'a Market) -> QuoteFuture<'a>;
}

/// The fetcher handlers receive via an axum `Extension` (so tests can inject a
/// stub instead of the live provider).
pub type SharedFetcher = Arc<dyn PriceFetcher>;

/// Round away the float noise in a provider price: Yahoo serves float32-
/// precision values (`62.48` arrives as `62.4799995422363`, which is exactly
/// `62.48f32`), so anything past ~7 significant digits is noise. Keep 7
/// significant digits — counted from the first non-zero digit, so sub-$1
/// token prices keep theirs — and drop trailing zeros.
pub fn clean_price(price: Decimal) -> Decimal {
    if price.is_zero() {
        return Decimal::ZERO;
    }
    // Order of magnitude from the mantissa/scale: 62.4799… has a 15-digit
    // mantissa at scale 13 → exponent 1 → round to 5 decimal places.
    let mantissa_digits = price.mantissa().abs().to_string().len() as i32;
    let exponent = mantissa_digits - 1 - price.scale() as i32;
    let dp = (6 - exponent).clamp(0, 28) as u32;
    price.round_dp(dp).normalize()
}

/// Yahoo Finance, via the `yfinance-rs` crate (see the module docs for the
/// provider decision and its verified behaviour).
#[derive(Default)]
pub struct YahooFetcher {
    client: yfinance_rs::YfClient,
}

/// The Yahoo symbol for a market **as at `date`** — the symbol the security
/// was actually quoted under then, so a historical fetch isn't asked for a
/// ticker that didn't exist yet.
///
/// Precedence: a one-off `symbol_override` (backfill's `symbol` param) wins
/// first; then the listing's stored `price_symbol`, but only for a date in
/// the *current* identity, since an override that matched the old ticker
/// rarely matches the new one (`listing::Listing::price_symbol`); then the
/// derived mapping over the identity in force on `date` — ASX tickers carry
/// `.AX`, NYSE/Nasdaq are plain, crypto is `<TICKER>-<quote currency>` (so
/// Yahoo quotes it in the listing's own currency). Other exchanges need a
/// mapping added here, or a `price_symbol` override on the listing.
fn yahoo_symbol(market: &Market, date: NaiveDate) -> Result<String, String> {
    yahoo_symbol_for(market, market.identity_at(date))
}

/// The Yahoo symbol for the identity in effect **now** — for a live quote,
/// which is always a question about today.
fn yahoo_symbol_now(market: &Market) -> Result<String, String> {
    yahoo_symbol_for(market, market.current())
}

fn yahoo_symbol_for(market: &Market, identity: &MarketIdentity) -> Result<String, String> {
    if let Some(symbol) = &market.symbol_override {
        return Ok(symbol.clone());
    }
    // Spans are unique by their start date, so this identifies the current one.
    if let Some(symbol) = &market.listing.price_symbol
        && identity.from == market.current().from
    {
        return Ok(symbol.clone());
    }
    let currency = &market.listing.currency;
    match identity.exchange_mic.as_deref() {
        None => Ok(format!("{}-{}", identity.ticker, currency)),
        Some("XASX") => Ok(format!("{}.AX", identity.ticker)),
        Some("XNYS") | Some("XNAS") => Ok(identity.ticker.clone()),
        Some(mic) => Err(format!("no Yahoo symbol mapping for exchange {mic}")),
    }
}

impl PriceFetcher for YahooFetcher {
    fn source(&self) -> &'static str {
        "yahoo"
    }

    fn symbol(&self, market: &Market, date: NaiveDate) -> Result<String, String> {
        yahoo_symbol(market, date)
    }

    fn daily_closes<'a>(
        &'a self,
        market: &'a Market,
        from: NaiveDate,
        to: NaiveDate,
    ) -> FetchFuture<'a> {
        Box::pin(async move {
            // `from..=to` sits inside one identity by contract, so its symbol
            // and calendar answer for the whole call.
            let symbol = self.symbol(market, from)?;
            let tz = market.identity_at(from).tz()?;
            // Daily candles are timestamped at session start, so the UTC
            // window [from 00:00, to+1 00:00) in the market's timezone covers
            // exactly the requested trading days.
            let start = local_midnight_utc(from, tz)?;
            let end = local_midnight_utc(to + Duration::days(1), tz)?;
            let candles = yfinance_rs::HistoryBuilder::new(&self.client, &symbol)
                .between(start, end)
                .interval(yfinance_rs::Interval::D1)
                .auto_adjust(false)
                .fetch()
                .await
                .map_err(|e| format!("yahoo fetch for {symbol} failed: {e}"))?;
            Ok(candles
                .into_iter()
                .map(|c| FetchedClose {
                    date: c.ts.with_timezone(&tz).date_naive(),
                    price: clean_price(c.ohlc.close.into_inner()),
                    currency: c.currency.to_string(),
                })
                .collect())
        })
    }

    fn latest_quote<'a>(&'a self, market: &'a Market) -> QuoteFuture<'a> {
        Box::pin(async move {
            // "Now" by definition, so the current identity's symbol — not
            // `symbol(market, today)`, which would resolve the same thing the
            // long way round.
            let symbol = yahoo_symbol_now(market)?;
            let quotes = yfinance_rs::quotes(&self.client, [symbol.clone()])
                .await
                .map_err(|e| format!("yahoo quote for {symbol} failed: {e}"))?;
            let quote = quotes
                .into_iter()
                .next()
                .ok_or_else(|| format!("yahoo returned no quote for {symbol}"))?;
            let price = quote
                .price
                .ok_or_else(|| format!("yahoo quote for {symbol} carries no price"))?;
            let as_of = quote
                .as_of
                .ok_or_else(|| format!("yahoo quote for {symbol} carries no timestamp"))?;
            Ok(LatestQuote {
                price: clean_price(price.into_inner()),
                currency: quote.currency.to_string(),
                as_of,
            })
        })
    }
}

/// Midnight at the start of `date` in `tz`, as a UTC instant (a DST gap at
/// midnight resolves to the earliest valid time).
fn local_midnight_utc(date: NaiveDate, tz: Tz) -> Result<DateTime<Utc>, String> {
    date.and_hms_opt(0, 0, 0)
        .and_then(|dt| dt.and_local_timezone(tz).earliest())
        .map(|dt| dt.with_timezone(&Utc))
        .ok_or_else(|| format!("cannot resolve midnight {date} in {tz}"))
}

// ---------------------------------------------------------------------------
// DB access
// ---------------------------------------------------------------------------

pub async fn db_get_one(
    pool: &SqlitePool,
    listing_id: i64,
    price_date: NaiveDate,
) -> Result<Option<ClosingPrice>, sqlx::Error> {
    sqlx::query_as(
        "SELECT id, listing_id, price_date, price, price_as_observed, source, fetched_at, \
                fetched_symbol, status, error, origin, sourced_from, reason \
         FROM closing_prices WHERE listing_id = ? AND price_date = ?",
    )
    .bind(listing_id)
    .bind(price_date)
    .fetch_optional(pool)
    .await
}

/// The listing's latest **ok** stored price at or before `on_or_before` and
/// not earlier than `not_before`, as `(price_date, price)`.
///
/// The carry-forward source for a listing the provider has stopped quoting
/// (`listings.unpriced_from`, SCENARIOS Q-02): `reports::valuation` reads it
/// when the valuation day itself has no ok price. It returns the *date* too,
/// so the caller can tell a genuinely contemporaneous price from a carried
/// one. A manual price entered during the unpriced run wins over an older
/// fetched one simply by being later.
///
/// `not_before` is the listing's `unpriced_before` (migration 0037), when it
/// has one: a row dated before the provider's series begins is not a price
/// for this security by the listing's own record, so it cannot be the figure
/// carried forward either. `None` means no floor.
pub async fn db_latest_ok_price_on_or_before(
    pool: &SqlitePool,
    listing_id: i64,
    on_or_before: NaiveDate,
    not_before: Option<NaiveDate>,
) -> Result<Option<(NaiveDate, Decimal)>, sqlx::Error> {
    let row: Option<(NaiveDate, Money)> = sqlx::query_as(
        "SELECT price_date, price FROM closing_prices \
         WHERE listing_id = ?1 AND status = 'ok' AND price_date <= ?2 \
           AND (?3 IS NULL OR price_date >= ?3) \
         ORDER BY price_date DESC LIMIT 1",
    )
    .bind(listing_id)
    .bind(on_or_before)
    .bind(not_before)
    .fetch_optional(pool)
    .await?;
    Ok(row.map(|(date, Money(price))| (date, price)))
}

/// Stored prices, newest first, optionally filtered by listing and date range.
pub async fn db_list(
    pool: &SqlitePool,
    listing_id: Option<i64>,
    from: Option<NaiveDate>,
    to: Option<NaiveDate>,
) -> Result<Vec<ClosingPrice>, sqlx::Error> {
    let mut qb = QueryBuilder::new(
        "SELECT id, listing_id, price_date, price, price_as_observed, source, fetched_at, \
                fetched_symbol, status, error, origin, sourced_from, reason \
         FROM closing_prices WHERE 1=1",
    );
    if let Some(id) = listing_id {
        qb.push(" AND listing_id = ").push_bind(id);
    }
    if let Some(from) = from {
        qb.push(" AND price_date >= ").push_bind(from);
    }
    if let Some(to) = to {
        qb.push(" AND price_date <= ").push_bind(to);
    }
    qb.push(" ORDER BY price_date DESC, listing_id");
    qb.build_query_as().fetch_all(pool).await
}

/// The dates in `from..=to` already stored with status ok for the listing
/// (so collection/backfill never re-fetches a good price).
async fn db_ok_dates(
    pool: &SqlitePool,
    listing_id: i64,
    from: NaiveDate,
    to: NaiveDate,
) -> Result<HashSet<NaiveDate>, sqlx::Error> {
    let dates: Vec<NaiveDate> = sqlx::query_scalar(
        "SELECT price_date FROM closing_prices \
         WHERE listing_id = ? AND status = 'ok' AND price_date BETWEEN ? AND ?",
    )
    .bind(listing_id)
    .bind(from)
    .bind(to)
    .fetch_all(pool)
    .await?;
    Ok(dates.into_iter().collect())
}

/// Upsert one row: a re-fetch replaces whatever is stored for the
/// (listing, date) — in particular, a success replaces an errored row — and a
/// manual entry replaces whatever was stored before it. Every column moves
/// together, so a row can never keep the origin of one write and the
/// provenance of another.
///
/// `row.id` is ignored (see [`UNASSIGNED_ID`]): the natural key is the conflict
/// target, so the database assigns a new surrogate id on an insert and keeps
/// the stored one when this updates — which is what lets the row's audit trail
/// span every version of it. A replacing write is an UPDATE, so the superseded
/// row (a manual price's own `sourced_from`/`reason` included) is recorded in
/// `row_history` by the 0021 trigger rather than lost.
pub(crate) async fn db_store(pool: &SqlitePool, row: &ClosingPrice) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO closing_prices \
             (listing_id, price_date, price, price_as_observed, source, fetched_at, \
              fetched_symbol, status, error, origin, sourced_from, reason) \
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?) \
         ON CONFLICT(listing_id, price_date) DO UPDATE SET \
             price = excluded.price, \
             price_as_observed = excluded.price_as_observed, \
             source = excluded.source, \
             fetched_at = excluded.fetched_at, \
             fetched_symbol = excluded.fetched_symbol, \
             status = excluded.status, \
             error = excluded.error, \
             origin = excluded.origin, \
             sourced_from = excluded.sourced_from, \
             reason = excluded.reason",
    )
    .bind(row.listing_id)
    .bind(row.price_date)
    .bind(OptMoney(row.price))
    .bind(OptMoney(row.price_as_observed))
    .bind(&row.source)
    .bind(&row.fetched_at)
    .bind(&row.fetched_symbol)
    .bind(row.status)
    .bind(&row.error)
    .bind(row.origin)
    .bind(&row.sourced_from)
    .bind(&row.reason)
    .execute(pool)
    .await?;
    Ok(())
}

/// A provider figure restated into its own trading day's unit basis, rounded
/// back to the provider's precision.
///
/// The arithmetic is `corporate_action::contemporaneous_price` — the shared
/// re-basing math, never re-derived here — and [`clean_price`] then holds the
/// result to 7 significant digits: the observation only ever carried that many
/// (Yahoo serves float32), so a ratio that does not divide out exactly must
/// not be written down as if it recovered more.
fn contemporaneous(
    as_observed: Decimal,
    events: &[crate::entities::corporate_action::PriceBasisEvent],
    price_date: NaiveDate,
    observed: NaiveDate,
) -> Decimal {
    clean_price(crate::entities::corporate_action::contemporaneous_price(
        as_observed,
        events,
        price_date,
        observed,
    ))
}

/// One stored provider figure, as the demerger factor's denominator: the
/// figure exactly as observed, and the UTC date it was observed on.
struct ObservedFigure {
    as_observed: Decimal,
    observed: NaiveDate,
}

/// The stored provider figure for one (listing, day), or `None` when there is
/// none to read. Manual rows are excluded: a hand-entered price is
/// contemporaneous by declaration, so it is not a *restated* figure and
/// dividing by it would only ever answer 1.
async fn db_observed_figure(
    conn: &mut sqlx::SqliteConnection,
    listing_id: i64,
    price_date: NaiveDate,
) -> Result<Option<ObservedFigure>, sqlx::Error> {
    let row: Option<(Money, String)> = sqlx::query_as(
        "SELECT price_as_observed, fetched_at FROM closing_prices \
         WHERE listing_id = ? AND price_date = ? AND status = 'ok' AND origin = 'fetched'",
    )
    .bind(listing_id)
    .bind(price_date)
    .fetch_optional(&mut *conn)
    .await?;
    row.map(|(price, fetched_at)| {
        Ok(ObservedFigure {
            as_observed: price.0,
            observed: observation_date(&fetched_at)?,
        })
    })
    .transpose()
}

/// The UTC date a row's `fetched_at` records — the date that fixes which unit
/// and price basis the figure arrived in (module docs).
fn observation_date(fetched_at: &str) -> Result<NaiveDate, sqlx::Error> {
    Ok(DateTime::parse_from_rfc3339(fetched_at)
        .map_err(|e| {
            sqlx::Error::Decode(
                format!("closing_prices.fetched_at {fetched_at:?} is not RFC 3339: {e}").into(),
            )
        })?
        .with_timezone(&Utc)
        .date_naive())
}

/// Every event that restated the provider's **price** series for one listing:
/// its `ShareSplit`/`BonusIssue` ratios, plus a derived factor for each
/// `Demerger` that carries a stated pre-demerger close (module docs, and
/// `corporate_action::adjustments` for why this is a different set from the
/// quantity one).
///
/// A split states its own factor. A demerger does not — the provider's
/// spin-off factor is set by the two entities' market values, which no term of
/// the action gives — so it is **derived**, here, from the two facts that
/// bracket it: what the operator states the security actually closed at on the
/// last pre-demerger trading day, over what the provider says about that same
/// day. The provider's side is read now rather than stored, so
///
/// - the close can be stated before any pre-demerger history exists (the
///   factor simply resolves to nothing until a figure is there to divide), and
/// - re-fetching that day re-derives the factor instead of leaving a stored
///   quotient stale.
///
/// The denominator is not the raw stored figure but what the walk **without
/// this demerger** would already make of it: a split dated between the close
/// date and the observation has restated that figure too, and the factor must
/// not absorb it a second time. That is also why the statements are resolved
/// latest-first — a *later* demerger restated the same figure and has to be
/// divided out first, while an earlier one is outside the half-open window
/// `(close_date, observed]` and cannot matter.
///
/// A demerger whose reference day was observed **before** it contributes
/// nothing: that figure is already contemporaneous, so there is no factor to
/// recover from it (and the rows it would apply to are exactly the ones the
/// half-open window already leaves alone).
pub async fn db_price_basis_events(
    conn: &mut sqlx::SqliteConnection,
    listing_id: i64,
) -> Result<Vec<crate::entities::corporate_action::PriceBasisEvent>, sqlx::Error> {
    use crate::entities::corporate_action::{self, PriceBasisEvent};

    let splits = corporate_action::db_splits_for_listing(&mut *conn, listing_id).await?;
    let mut events: Vec<PriceBasisEvent> = splits.iter().map(PriceBasisEvent::from).collect();

    let statements = corporate_action::db_demerger_price_statements(&mut *conn, listing_id).await?;
    for statement in statements.iter().rev() {
        let Some(reference) =
            db_observed_figure(&mut *conn, listing_id, statement.close_date).await?
        else {
            continue; // nothing of that day is stored yet
        };
        if reference.observed < statement.date {
            continue; // observed before the demerger: already contemporaneous
        }
        let partly = corporate_action::contemporaneous_price(
            reference.as_observed,
            &events,
            statement.close_date,
            reference.observed,
        );
        if partly <= Decimal::ZERO {
            continue; // no factor is recoverable from a non-positive figure
        }
        events.push(PriceBasisEvent {
            date: statement.date,
            recover_new: statement.close_price,
            recover_old: partly,
        });
    }
    Ok(events)
}

/// One stored ok, provider-fetched row, as the re-basing pass reads it.
#[derive(sqlx::FromRow)]
struct ObservedRow {
    id: i64,
    price_date: NaiveDate,
    fetched_at: String,
    #[sqlx(try_from = "Money")]
    price: Decimal,
    #[sqlx(try_from = "Money")]
    price_as_observed: Decimal,
}

/// Re-derive every stored provider price for one listing from the figure as
/// observed, over the listing's re-basing actions as they now stand. Returns
/// how many rows changed.
///
/// This is the other half of the basis invariant (module docs): normalising on
/// the way in fixes a price fetched *after* an event is recorded, and this
/// fixes one fetched before it. Because each price is recomputed from
/// `price_as_observed` rather than adjusted in place, the pass is idempotent
/// and order-free — it is equally the answer to a split or a demerger's stated
/// close being recorded, a ratio, close or date being edited, an action being
/// re-typed into another kind, and one being deleted.
/// `corporate_action::db_upsert`/`db_delete` run it on their own transaction so
/// the prices and the action can never be committed out of step, and the
/// `price-rebase` job runs it over every listing as the one-off repair of a
/// database that predates this rule.
///
/// The event set is [`db_price_basis_events`]', not the quantity re-basing one
/// — a demerger belongs to it and must never reach `split_ratio`.
///
/// An **empty** event set is not an early exit: it is the state a listing is
/// left in when its last re-basing action is deleted, or a demerger's stated
/// close removed, and the prices then have to come back to the figures as
/// observed. So the walk runs either way — over no events it re-derives each
/// price as `clean_price(price_as_observed)`, which is what a fetch with
/// nothing to restate would have stored, and writes nothing where that is
/// already the stored figure.
///
/// Manual rows are excluded: a hand-entered price is contemporaneous by
/// declaration and is never rewritten (module docs).
pub async fn db_rebase_listing_prices(
    conn: &mut sqlx::SqliteConnection,
    listing_id: i64,
) -> Result<usize, sqlx::Error> {
    let events = db_price_basis_events(&mut *conn, listing_id).await?;
    let rows: Vec<ObservedRow> = sqlx::query_as(
        "SELECT id, price_date, fetched_at, price, price_as_observed FROM closing_prices \
         WHERE listing_id = ? AND status = 'ok' AND origin = 'fetched' ORDER BY price_date",
    )
    .bind(listing_id)
    .fetch_all(&mut *conn)
    .await?;

    let mut changed = 0;
    for row in rows {
        let observed = observation_date(&row.fetched_at)?;
        let wanted = contemporaneous(row.price_as_observed, &events, row.price_date, observed);
        if wanted == row.price {
            continue;
        }
        sqlx::query("UPDATE closing_prices SET price = ? WHERE id = ?")
            .bind(Money(wanted))
            .bind(row.id)
            .execute(&mut *conn)
            .await?;
        changed += 1;
    }
    Ok(changed)
}

/// Re-base every listing that has a price re-basing action recorded against
/// it — a `ShareSplit`/`BonusIssue`, or a `Demerger` carrying a stated
/// pre-demerger close — as the `price-rebase` maintenance job, and the one-off
/// repair for a database whose prices were stored before the basis rule
/// existed (migrations 0034 and 0036). One transaction, so the whole repair
/// lands or none of it does; idempotent, so running it again is a no-op.
///
/// Only listings with such an action can have a price to correct, so those are
/// the only ones read. This stays the single repair path: a demerger's stated
/// close was folded into the same job rather than given one of its own.
pub async fn run_rebase(pool: &SqlitePool) -> Result<(), String> {
    let mut tx = pool.begin().await.map_err(|e| e.to_string())?;
    let listing_ids: Vec<i64> = sqlx::query_scalar(
        "SELECT DISTINCT listing_id FROM corporate_actions \
         WHERE action_type IN ('ShareSplit', 'BonusIssue') \
            OR (action_type = 'Demerger' AND demerger_close_date IS NOT NULL) \
         ORDER BY listing_id",
    )
    .fetch_all(&mut *tx)
    .await
    .map_err(|e| e.to_string())?;
    let mut changed = 0;
    for listing_id in &listing_ids {
        changed += db_rebase_listing_prices(&mut tx, *listing_id)
            .await
            .map_err(|e| e.to_string())?;
    }
    tx.commit().await.map_err(|e| e.to_string())?;
    tracing::info!(
        listings = listing_ids.len(),
        rebased = changed,
        "closing-price re-base complete"
    );
    Ok(())
}

/// Delete one stored row, reporting whether one was there. Callers must have
/// established that the row is one of the two kinds no snapshot was ever
/// valued at (the handler rejects any other):
///
/// * an **errored** row — `reports::valuation` blocks the date outright;
/// * an ok row dated **before the listing's `unpriced_before`** — the marker
///   supersedes the stored rows for that span, so valuation excludes the
///   holding from those dates instead of pricing it, and even the
///   `unpriced_from` carry-forward is floored at the marker
///   ([`db_latest_ok_price_on_or_before`]).
///
/// Either way removing the row cannot invalidate a stored snapshot figure:
/// no stored figure was computed from it. That is what lets `closing_prices`
/// keep its single `..._stale_snapshots_update` trigger (0001_schema.sql)
/// with no DELETE counterpart, unlike the fact tables. Setting or moving the
/// marker is itself what stales the affected snapshots (0037's
/// `listings_stale_snapshots_update` stales the prefix before the later of
/// the old and new dates), so a span whose rows are then cleared has already
/// been regenerated without them, and clearing or moving the marker back
/// later stales the prefix again — regeneration then reports the dates
/// blocked for want of a price, which is the truth once the rows are gone.
pub async fn db_delete(
    pool: &SqlitePool,
    listing_id: i64,
    price_date: NaiveDate,
) -> Result<bool, sqlx::Error> {
    let result = sqlx::query("DELETE FROM closing_prices WHERE listing_id = ? AND price_date = ?")
        .bind(listing_id)
        .bind(price_date)
        .execute(pool)
        .await?;
    Ok(result.rows_affected() > 0)
}

/// What [`db_clear_unpriced_before`] found to do.
#[derive(Debug, PartialEq, Eq)]
pub enum ClearOutcome {
    /// No such listing.
    NoListing,
    /// The listing declares no `unpriced_before`, so it has no superseded
    /// span and nothing here may be cleared in bulk.
    NoMarker,
    /// The span was cleared (possibly of nothing — the operation is
    /// idempotent).
    Cleared {
        unpriced_before: NaiveDate,
        deleted: u64,
    },
}

/// Clear every stored row a listing's `unpriced_before` marker supersedes —
/// the whole span before it, ok rows included — in one transaction.
///
/// The bulk form of the single-date delete, and deliberately the *only* bulk
/// form: the span it clears is not a caller-supplied date range but the
/// listing's own declaration, read from the `listings` row by the DELETE
/// itself, so this can never become a general bulk-delete of price history.
/// A listing with no marker deletes nothing ([`ClearOutcome::NoMarker`]).
/// Re-running it is a no-op that reports `deleted: 0`.
///
/// Why an ok row may go: see [`db_delete`] — inside the span no stored figure
/// is read by valuation, so none of it is a valuation to lose. Nothing is
/// destroyed either way: `closing_prices` is audited, and the per-row `AFTER
/// DELETE` trigger fires once per row of a multi-row DELETE, so every
/// cleared figure and its `sourced_from`/`reason` land in `row_history`.
///
/// It cannot break `unpriced_from`'s write-time pairing (a stored ok price
/// must exist *before* that marker to be carried forward), because that check
/// only ever looks at rows on or after `unpriced_before` — exactly the ones
/// this leaves alone.
pub async fn db_clear_unpriced_before(
    pool: &SqlitePool,
    listing_id: i64,
) -> Result<ClearOutcome, sqlx::Error> {
    let mut tx = pool.begin().await?;
    let found: Option<(Option<NaiveDate>,)> =
        sqlx::query_as("SELECT unpriced_before FROM listings WHERE id = ?")
            .bind(listing_id)
            .fetch_optional(&mut *tx)
            .await?;
    let Some((marker,)) = found else {
        return Ok(ClearOutcome::NoListing);
    };
    let Some(unpriced_before) = marker else {
        return Ok(ClearOutcome::NoMarker);
    };
    // The bound is the subquery, not the value read above: the rows deleted
    // are exactly the ones the listing's own row calls superseded at the
    // moment the statement runs.
    let deleted = sqlx::query(
        "DELETE FROM closing_prices \
         WHERE listing_id = ?1 \
           AND price_date < (SELECT unpriced_before FROM listings WHERE id = ?1)",
    )
    .bind(listing_id)
    .execute(&mut *tx)
    .await?
    .rows_affected();
    tx.commit().await?;
    Ok(ClearOutcome::Cleared {
        unpriced_before,
        deleted,
    })
}

// ---------------------------------------------------------------------------
// Held timeline: what was held, and when
// ---------------------------------------------------------------------------

/// One purchase parcel's contribution to a listing's holding over time: `qty`
/// units from `acquired`, less the units each sale allocated out of it from
/// that sale's date.
struct ParcelHolding {
    acquired: NaiveDate,
    qty: Decimal,
    /// `(sale date, units sold)`, each already re-based to this parcel's
    /// as-acquired unit basis.
    sales: Vec<(NaiveDate, Decimal)>,
}

impl ParcelHolding {
    /// Units of this parcel still held on `date`, floored at nil: an
    /// over-allocated parcel must not net off another parcel's remaining
    /// units.
    fn remaining_on(&self, date: NaiveDate) -> Decimal {
        if self.acquired > date {
            return Decimal::ZERO;
        }
        let sold: Decimal = self
            .sales
            .iter()
            .filter(|(sale_date, _)| *sale_date <= date)
            .map(|(_, qty)| *qty)
            .sum();
        (self.qty - sold).max(Decimal::ZERO)
    }
}

/// Every purchase parcel and the sales out of it, loaded once: *the* in-memory
/// model of what was held and when. Decimal arithmetic in Rust, never float
/// SUM in SQL.
///
/// Each sale's `quantity_allocated` is expressed in the unit basis of its own
/// sale date, so it is re-based back to the parcel's as-acquired units
/// (`corporate_action::as_acquired_quantity`) as it is loaded — exactly as
/// `reports::portfolio::db_holdings_on` does. Without that, a split between a
/// Buy and a Sell makes this and the holdings reports disagree about whether
/// the listing is held at all, and snapshot generation then stores a silently
/// unvalued row (a holding the price map has no entry for) or blocks a date on
/// a security already fully sold.
///
/// Three queries answer any number of dates, so a caller walking years of
/// history ([`HeldTimeline::held_spans`], the health report's unpriced-day
/// check) never makes a per-day round trip.
pub struct HeldTimeline {
    /// Purchase parcels per listing; a listing appears exactly if it was ever
    /// bought, whether or not it is still held.
    parcels: HashMap<i64, Vec<ParcelHolding>>,
}

impl HeldTimeline {
    pub async fn load(pool: &SqlitePool) -> Result<Self, sqlx::Error> {
        let buys = sqlx::query(
            "SELECT id, listing_id, date, quantity FROM trades \
             WHERE trade_type IN ('Buy', 'DRP')",
        )
        .fetch_all(pool)
        .await?;

        // sale-date-basis units allocated out of each purchase parcel
        let allocs = sqlx::query(
            "SELECT pa.purchase_trade_id, pa.quantity_allocated, s.date AS sale_date \
             FROM parcel_allocations pa JOIN trades s ON s.id = pa.sale_trade_id",
        )
        .fetch_all(pool)
        .await?;
        let mut qty_sold: HashMap<i64, Vec<(NaiveDate, Decimal)>> = HashMap::new();
        for row in &allocs {
            let trade_id: i64 = row.try_get("purchase_trade_id")?;
            qty_sold.entry(trade_id).or_default().push((
                row.try_get("sale_date")?,
                parse_dec("quantity_allocated", row.try_get("quantity_allocated")?)?,
            ));
        }

        let split_events = crate::entities::corporate_action::db_share_split_events(pool).await?;

        let mut parcels: HashMap<i64, Vec<ParcelHolding>> = HashMap::new();
        for row in &buys {
            let trade_id: i64 = row.try_get("id")?;
            let listing_id: i64 = row.try_get("listing_id")?;
            let acquired: NaiveDate = row.try_get("date")?;
            let qty = parse_dec("quantity", row.try_get("quantity")?)?;
            let splits = split_events.get(&listing_id).map_or(&[][..], |v| v);
            let sales = qty_sold
                .get(&trade_id)
                .map_or(&[][..], |v| v)
                .iter()
                .map(|&(sale_date, sold)| {
                    (
                        sale_date,
                        crate::entities::corporate_action::as_acquired_quantity(
                            sold, splits, acquired, sale_date,
                        ),
                    )
                })
                .collect();
            parcels.entry(listing_id).or_default().push(ParcelHolding {
                acquired,
                qty,
                sales,
            });
        }
        Ok(HeldTimeline { parcels })
    }

    /// Listings with a non-zero holding as at `as_of` (live holdings when
    /// `None`) — trades and sales dated after it don't count.
    pub fn held_listing_ids(&self, as_of: Option<NaiveDate>) -> Vec<i64> {
        let cutoff = crate::infra::date::as_of_or_open(as_of);
        let mut ids: Vec<i64> = self
            .parcels
            .iter()
            .filter(|(_, parcels)| {
                parcels
                    .iter()
                    .map(|p| p.remaining_on(cutoff))
                    .sum::<Decimal>()
                    > Decimal::ZERO
            })
            .map(|(listing_id, _)| *listing_id)
            .collect();
        ids.sort();
        ids
    }

    /// Every listing ever held, ascending — including ones since fully sold,
    /// whose held span is still history that needed pricing.
    pub fn listing_ids(&self) -> Vec<i64> {
        let mut ids: Vec<i64> = self.parcels.keys().copied().collect();
        ids.sort();
        ids
    }

    /// The listing's held spans as inclusive date ranges, ascending and
    /// non-adjacent, ending no later than `until`. A listing sold down to nil
    /// and later re-bought yields one span per holding period.
    ///
    /// A holding only changes on an acquisition or a sale date, so the
    /// quantity is evaluated at those dates alone and held constant in between
    /// — walking six years of calendar dates would be thousands of sums.
    pub fn held_spans(&self, listing_id: i64, until: NaiveDate) -> Vec<(NaiveDate, NaiveDate)> {
        let Some(parcels) = self.parcels.get(&listing_id) else {
            return Vec::new();
        };
        let mut events: Vec<NaiveDate> = parcels
            .iter()
            .flat_map(|p| std::iter::once(p.acquired).chain(p.sales.iter().map(|(date, _)| *date)))
            .filter(|date| *date <= until)
            .collect();
        events.sort();
        events.dedup();

        let mut spans: Vec<(NaiveDate, NaiveDate)> = Vec::new();
        for (i, &start) in events.iter().enumerate() {
            let held: Decimal = parcels.iter().map(|p| p.remaining_on(start)).sum();
            if held <= Decimal::ZERO {
                continue;
            }
            // The holding stands until the next event changes it, or to the
            // caller's bound when nothing else happens.
            let end = events
                .get(i + 1)
                .map_or(until, |next| *next - Duration::days(1));
            match spans.last_mut() {
                Some(last) if last.1 + Duration::days(1) == start => last.1 = end,
                _ => spans.push((start, end)),
            }
        }
        spans
    }
}

/// Listings with a non-zero holding. With `as_of` the holding is taken as at
/// that date — trades and sales dated after it don't count (snapshot
/// generation for a past date values what was held then, not what is held
/// now). A thin wrapper over [`HeldTimeline`], which documents the re-basing
/// rules; a caller asking about more than one date should load the timeline
/// once instead.
pub async fn db_held_listing_ids(
    pool: &SqlitePool,
    as_of: Option<NaiveDate>,
) -> Result<Vec<i64>, sqlx::Error> {
    Ok(HeldTimeline::load(pool).await?.held_listing_ids(as_of))
}

// ---------------------------------------------------------------------------
// Fetch-and-store: shared by scheduled collection, manual re-fetch and backfill
// ---------------------------------------------------------------------------

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
async fn fetch_and_store(
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
        let by_date: Result<HashMap<NaiveDate, FetchedClose>, String> =
            fetched.map(|closes| closes.into_iter().map(|c| (c.date, c)).collect());

        // A provider call that returns *zero* candles across the whole
        // requested window (as opposed to a partial result with a data gap on
        // one date) is the classic wrong/renamed/delisted-symbol case, not a
        // transient outage — the day-by-day fallback message below is
        // indistinguishable from one, so every date gets a message that names
        // the symbol and points at the fix instead. Judged per segment, so the
        // message names the symbol that actually came back empty.
        let symbol_dead_or_wrong = matches!(&by_date, Ok(map) if map.is_empty());
        let no_candles_message = || {
            let symbol = symbol.clone().unwrap_or_else(|e| e);
            format!(
                "provider returned no candles for {symbol} over {from}..{to} — the symbol may be \
                 wrong, renamed, or delisted; set price_symbol on the listing or backfill with an \
                 explicit symbol"
            )
        };

        for date in wanted {
            let result = match &by_date {
                Err(e) => Err(e.clone()),
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

// ---------------------------------------------------------------------------
// On-demand live valuation: latest quote per listing, converted to AUD
// ---------------------------------------------------------------------------

/// One held listing's live valuation: the latest provider quote converted to
/// AUD, with the provider's as-of time. Consumed by the price-dependent
/// reports for current valuation.
#[derive(Debug, Clone)]
pub struct LiveValuation {
    /// Price per unit in AUD (the quote currency converted via the FX rules).
    pub aud_price: Decimal,
    /// The provider's quote timestamp, RFC 3339 UTC.
    pub as_of: String,
    /// The AUD conversion used an earlier month's FX rate because the quote
    /// month's rate is not published yet (`infra::fx::resolve_valuation_rate`):
    /// the valuation is provisional and the reports annotate the row.
    pub fx_provisional: bool,
}

/// Fetch the latest live quote for each listing and convert it to AUD. Returns
/// one entry per listing id: `Ok` with the AUD price + as-of time, or
/// `Err(reason)` when the fetch, a currency mismatch, or the AUD conversion
/// failed — the caller surfaces the reason per holding and leaves it unvalued
/// (never a silent zero, per the never-silent-zero rule).
pub async fn fetch_live_aud_prices(
    pool: &SqlitePool,
    fetcher: &dyn PriceFetcher,
    listing_ids: &[i64],
) -> Result<HashMap<i64, Result<LiveValuation, String>>, sqlx::Error> {
    let mut out = HashMap::new();
    for &id in listing_ids {
        let Some(market) = load_market(pool, id).await? else {
            out.insert(id, Err(format!("listing {id} no longer exists")));
            continue;
        };
        let result = match fetcher.latest_quote(&market).await {
            Err(e) => Err(e),
            Ok(quote) if quote.currency != market.listing.currency => Err(format!(
                "currency mismatch: provider quoted {}, listing is {}",
                quote.currency, market.listing.currency
            )),
            Ok(quote) => {
                // Convert the quote-currency price to AUD at the valuation
                // rate for the quote's month: the ATO monthly rate when
                // published, else the bounded earlier-month fallback flagged
                // provisional on the row (early in a month the rate cannot
                // exist yet — a flagged valuation beats an unvalued holding).
                // A gap beyond the fallback bound is surfaced as the row's
                // reason, never a silent or zeroed value.
                match crate::infra::fx::resolve_valuation_rate(
                    pool,
                    &quote.currency,
                    quote.as_of.date_naive(),
                )
                .await
                {
                    Ok(vr) => Ok(LiveValuation {
                        aud_price: crate::infra::fx::apply_rate(quote.price, vr.rate),
                        as_of: quote.as_of.to_rfc3339(),
                        fx_provisional: vr.provisional,
                    }),
                    Err(e) => Err(e.to_string()),
                }
            }
        };
        out.insert(id, result);
    }
    Ok(out)
}

/// Resolve live AUD prices for the price-dependent report handlers: when `live`
/// is set, fetch the latest quote for every listing in `listing_ids` that has
/// no explicit override (an explicit price always wins, so it is never
/// fetched). Off, or with no fetcher available, yields an empty map (no live
/// valuation). A live request with no fetcher marks each listing unavailable
/// rather than silently dropping the as-of contract.
pub async fn resolve_live_prices(
    pool: &SqlitePool,
    fetcher: Option<&dyn PriceFetcher>,
    live: bool,
    overrides: &HashMap<i64, Decimal>,
    listing_ids: impl IntoIterator<Item = i64>,
) -> Result<HashMap<i64, Result<LiveValuation, String>>, sqlx::Error> {
    if !live {
        return Ok(HashMap::new());
    }
    let ids: Vec<i64> = listing_ids
        .into_iter()
        .filter(|id| !overrides.contains_key(id))
        .collect::<HashSet<_>>()
        .into_iter()
        .collect();
    match fetcher {
        Some(fetcher) => fetch_live_aud_prices(pool, fetcher, &ids).await,
        None => Ok(ids
            .into_iter()
            .map(|id| (id, Err("live price source unavailable".to_string())))
            .collect()),
    }
}

// ---------------------------------------------------------------------------
// HTTP API
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct ListParams {
    listing_id: Option<i64>,
    from: Option<NaiveDate>,
    to: Option<NaiveDate>,
}

#[derive(Debug, Deserialize)]
struct FetchBody {
    listing_id: i64,
    price_date: NaiveDate,
}

#[derive(Debug, Deserialize)]
struct BackfillBody {
    listing_id: i64,
    from: NaiveDate,
    to: NaiveDate,
    /// One-off provider symbol for this fetch only (not persisted to
    /// `listings.price_symbol`) — recovers a pre-rename date range under the
    /// old symbol, when the provider no longer serves it under the current
    /// one. Omitted: the listing's stored `price_symbol` (if any) or the
    /// derived mapping, as for any other fetch.
    #[serde(default)]
    symbol: Option<String>,
}

/// A price entered by hand for a day the provider cannot serve, with the
/// provenance that makes the figure auditable later.
#[derive(Debug, Deserialize)]
struct ManualPriceBody {
    /// Closing price in the listing's quote currency (never AUD).
    price: Decimal,
    /// Where the figure came from, e.g. "asx.com.au closing report".
    sourced_from: String,
    /// Why manual entry was needed, e.g. "provider serves no candle since the
    /// delisting".
    reason: String,
}

/// Which listing's superseded price rows to clear. No date range: the span
/// is the listing's own `unpriced_before` declaration and nothing else.
#[derive(Debug, Deserialize)]
struct ClearBody {
    listing_id: i64,
}

/// What a clear run did, returned to the caller.
#[derive(Debug, Serialize, Deserialize)]
pub struct ClearSummary {
    pub listing_id: i64,
    /// The marker that defined the cleared span, echoed back so the caller
    /// can see what it actually acted on.
    pub unpriced_before: NaiveDate,
    /// Rows removed. Zero on a re-run — the operation is idempotent.
    pub deleted: u64,
}

/// What a backfill run did, returned to the caller.
#[derive(Debug, Serialize, Deserialize)]
pub struct BackfillSummary {
    /// Trading days in the (clamped) range.
    pub trading_days: usize,
    /// Days skipped because an ok price was already stored.
    pub already_stored: usize,
    pub fetched_ok: usize,
    pub errored: usize,
}

async fn list(
    State(pool): State<SqlitePool>,
    Query(params): Query<ListParams>,
) -> Result<Json<Vec<ClosingPrice>>, ApiError> {
    db_list(&pool, params.listing_id, params.from, params.to)
        .await
        .map(Json)
        .map_err(ApiError::from)
}

/// Store a price entered by hand for one (listing, day), with the provenance
/// that makes it auditable: where it was sourced from and why manual entry
/// was needed. This is the way out of a day the provider cannot serve — a
/// delisted or mis-served symbol, or a permanent hole in its series — which
/// `reports::valuation` otherwise blocks forever, taking the day's snapshots
/// with it.
///
/// The day must be a trading day whose close is final, exactly as for a
/// fetch: a price on any other date would never be read by valuation. A
/// manual price may deliberately replace a stored provider price that is
/// wrong; that is an ordinary UPDATE, so the staleness trigger regenerates
/// the snapshots that used the old figure.
async fn put_manual(
    State(pool): State<SqlitePool>,
    Path((listing_id, price_date)): Path<(i64, NaiveDate)>,
    Json(body): Json<ManualPriceBody>,
) -> Result<StatusCode, ApiError> {
    let market = load_market(&pool, listing_id)
        .await
        .map_err(internal)?
        .ok_or_else(|| ApiError::not_found("no such listing"))?;
    validate_complete_trading_day(&market, price_date)?;

    if body.price <= Decimal::ZERO {
        return Err(ApiError::unprocessable(format!(
            "the price must be positive, not {}",
            body.price
        )));
    }
    let sourced_from = body.sourced_from.trim();
    let reason = body.reason.trim();
    if sourced_from.is_empty() {
        return Err(ApiError::unprocessable(
            "sourced_from is required: record where the price was taken from",
        ));
    }
    if reason.is_empty() {
        return Err(ApiError::unprocessable(
            "reason is required: record why the price had to be entered by hand",
        ));
    }

    let row = ClosingPrice {
        id: UNASSIGNED_ID,
        listing_id,
        price_date,
        price: Some(body.price),
        // A hand-entered figure is contemporaneous by declaration — the
        // operator states what the security traded at that day — so it is
        // recorded as its own observation and no re-basing ever touches it.
        price_as_observed: Some(body.price),
        source: MANUAL_SOURCE.to_string(),
        fetched_at: Utc::now().to_rfc3339(),
        // Nothing was fetched, so there is no symbol to record (CHECK-paired
        // with the origin, migration 0038).
        fetched_symbol: None,
        status: PriceStatus::Ok,
        error: None,
        origin: PriceOrigin::Manual,
        sourced_from: Some(sourced_from.to_string()),
        reason: Some(reason.to_string()),
    };
    db_store(&pool, &row).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// Re-fetch one (listing, date) on demand — typically to replace an errored
/// row. Returns the freshly stored row (which itself is errored if the
/// provider failed again).
///
/// A **manual** row is rejected 422: a hand-entered price is a deliberate
/// correction for a day the provider got wrong or cannot serve at all, so the
/// provider never takes the day back — the price is changed by entering
/// another one.
async fn fetch_one(
    State(pool): State<SqlitePool>,
    Extension(fetcher): Extension<SharedFetcher>,
    Json(body): Json<FetchBody>,
) -> Result<Json<ClosingPrice>, ApiError> {
    let market = load_market(&pool, body.listing_id)
        .await
        .map_err(internal)?
        .ok_or_else(|| ApiError::not_found("no such listing"))?;
    validate_complete_trading_day(&market, body.price_date)?;
    reject_unpriced_date(&market, body.price_date)?;
    if let Some(stored) = db_get_one(&pool, body.listing_id, body.price_date).await?
        && stored.origin == PriceOrigin::Manual
    {
        return Err(ApiError::unprocessable(format!(
            "the stored price for {} was entered manually ({}) — re-enter it manually to \
             change it, the provider does not take the day back",
            body.price_date,
            stored.reason.unwrap_or_default()
        )));
    }

    fetch_and_store(&pool, fetcher.as_ref(), &market, &[body.price_date])
        .await
        .map_err(internal)?;
    let row = db_get_one(&pool, body.listing_id, body.price_date)
        .await
        .map_err(internal)?
        .ok_or_else(|| internal("stored row vanished"))?;
    Ok(Json(row))
}

/// Backfill a listing's price history over a date range (e.g. after importing
/// an old trade, or recovering pre-rename history under the old symbol via
/// the optional `symbol` override): trading days only, skipping dates
/// already stored ok, in one provider call.
async fn backfill(
    State(pool): State<SqlitePool>,
    Extension(fetcher): Extension<SharedFetcher>,
    Json(body): Json<BackfillBody>,
) -> Result<Json<BackfillSummary>, ApiError> {
    let mut market = load_market(&pool, body.listing_id)
        .await
        .map_err(internal)?
        .ok_or_else(|| ApiError::not_found("no such listing"))?;
    market.symbol_override = body.symbol.clone();
    if body.from > body.to {
        return Err(ApiError::unprocessable("from is after to"));
    }
    // Clamp the range to days whose close is final.
    let latest = market
        .latest_complete_trading_day(Utc::now())
        .map_err(unprocessable)?
        .filter(|latest| *latest >= body.from)
        .ok_or_else(|| ApiError::unprocessable("range contains no complete trading day"))?;
    let mut to = body.to.min(latest);
    // …and to days the provider still quotes: a listing marked unpriced from
    // a date has nothing to serve on or after it, so the range stops the day
    // before rather than storing a run of errored rows (SCENARIOS Q-02). A
    // range wholly inside the unpriced run is refused, naming the marker.
    if let Some(from) = market.listing.unpriced_from {
        reject_unpriced_date(&market, body.from)?;
        to = to.min(from.pred_opt().unwrap_or(from));
    }
    // The mirror: a listing marked unpriced *before* a date has nothing to
    // serve earlier than it, so the range starts at it rather than storing a
    // run of errored rows for a series that had not begun (migration 0037).
    // A range wholly before it is refused, naming the marker.
    let mut from = body.from;
    if let Some(before) = market.listing.unpriced_before {
        reject_unpriced_date(&market, body.to)?;
        from = from.max(before);
    }

    let mut trading_days: Vec<NaiveDate> = Vec::new();
    let mut date = from;
    while date <= to {
        if market.is_trading_day(date) {
            trading_days.push(date);
        }
        date += Duration::days(1);
    }
    let stored_ok = db_ok_dates(&pool, body.listing_id, from, to)
        .await
        .map_err(internal)?;
    let missing: Vec<NaiveDate> = trading_days
        .iter()
        .copied()
        .filter(|d| !stored_ok.contains(d))
        .collect();

    let (fetched_ok, errored) = fetch_and_store(&pool, fetcher.as_ref(), &market, &missing)
        .await
        .map_err(internal)?;
    Ok(Json(BackfillSummary {
        trading_days: trading_days.len(),
        already_stored: trading_days.len() - missing.len(),
        fetched_ok,
        errored,
    }))
}

/// Delete one **errored** row: the acknowledgement that no price will ever
/// exist for that (listing, day) — a date before the security's first trading
/// day, or a permanent hole in the provider's series — so it stops being
/// reported by `GET /reports/health`'s `errored_prices`, which otherwise nags
/// forever about a row no re-fetch can fix.
///
/// An **ok** row is rejected 422: real price data is replaced by a re-fetch
/// (`/fetch`, `/backfill`), never deleted, so this endpoint can never punch a
/// hole in a valued series. For a held listing, deleting an errored row does
/// not unblock its date — valuation still refuses it, now for want of any row
/// at all ("no stored price … backfill it") — it only clears the standing
/// alarm.
///
/// The one exception is a date inside the listing's **`unpriced_before`**
/// span, where an ok row is deletable whatever its origin. The rule protects
/// nothing there: the marker declares that no price is obtainable for the
/// security before that date, so valuation excludes the holding from those
/// dates rather than pricing it and the carry-forward is floored at the
/// marker — the stored figure is read by nothing, and deleting it is the
/// acknowledgement that it never was a valuation (migration 0037; the live
/// case is a span priced from another security's series). The mirror marker
/// `unpriced_from` gets **no** such relaxation: a date on or after it *is*
/// valued, at the last stored ok close carried forward, so a delete there
/// could remove the very figure being carried.
async fn delete_one(
    State(pool): State<SqlitePool>,
    Path((listing_id, price_date)): Path<(i64, NaiveDate)>,
) -> Result<StatusCode, ApiError> {
    let row = db_get_one(&pool, listing_id, price_date)
        .await?
        .ok_or_else(|| ApiError::not_found("no stored price for that listing and date"))?;
    let superseded = listing::db_get(&pool, listing_id)
        .await?
        .and_then(|l| l.unpriced_before)
        .is_some_and(|before| price_date < before);
    if row.status == PriceStatus::Ok && !superseded {
        let replacement = match row.origin {
            PriceOrigin::Manual => "enter another manual price to replace it",
            PriceOrigin::Fetched => "re-fetch it to replace it",
        };
        return Err(ApiError::unprocessable(format!(
            "the stored price for {price_date} is ok, not errored — {replacement} rather than \
             deleting it"
        )));
    }
    db_delete(&pool, listing_id, price_date).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// Clear a listing's whole superseded price span in one request: every row
/// dated before its `unpriced_before`, ok rows included, in one transaction,
/// answering how many were removed.
///
/// The bulk counterpart of the single-date delete above, for the case it
/// exists for — a span of hundreds of days priced from a source the listing
/// itself now says is not a price for this security. It takes **no date
/// range**: the span is read from the listing's own marker, so this cannot
/// become a general bulk-delete of price history. Safe to re-run (a second
/// call reports `deleted: 0`), and nothing is destroyed — every removed row
/// lands in `row_history` with its figure and provenance.
///
/// `404` for an unknown listing; `422` for a listing that declares no
/// `unpriced_before`, since without one there is no superseded span at all.
async fn clear_unpriced_before(
    State(pool): State<SqlitePool>,
    Json(body): Json<ClearBody>,
) -> Result<Json<ClearSummary>, ApiError> {
    let listing = listing::db_get(&pool, body.listing_id)
        .await?
        .ok_or_else(|| ApiError::not_found("no such listing"))?;
    match db_clear_unpriced_before(&pool, body.listing_id).await? {
        ClearOutcome::NoListing => Err(ApiError::not_found("no such listing")),
        ClearOutcome::NoMarker => Err(ApiError::unprocessable(format!(
            "{} has no unpriced_before, so no stored price of its is superseded — only the span \
             before that marker can be cleared in bulk. Set unpriced_before on the listing (PUT \
             /listings/:id) if the provider's series really does begin later than its stored \
             prices claim; otherwise a price is replaced by a re-fetch or another manual entry, \
             never deleted",
            listing.ticker
        ))),
        ClearOutcome::Cleared {
            unpriced_before,
            deleted,
        } => {
            tracing::info!(
                listing_id = body.listing_id,
                ticker = %listing.ticker,
                %unpriced_before,
                deleted,
                "cleared superseded closing prices"
            );
            Ok(Json(ClearSummary {
                listing_id: body.listing_id,
                unpriced_before,
                deleted,
            }))
        }
    }
}

/// 422 when `date` falls outside the span the provider serves this listing —
/// on or after `listings.unpriced_from`, or before `listings.unpriced_before`.
/// Either way the provider serves nothing there by the listing's own record,
/// so a fetch could only store another errored row. Each refusal names the
/// way back: enter the price by hand, or move/clear the marker.
fn reject_unpriced_date(market: &Market, date: NaiveDate) -> Result<(), ApiError> {
    if let Some(from) = market.listing.unpriced_from
        && date >= from
    {
        return Err(ApiError::unprocessable(format!(
            "{} is unpriced from {from} — the provider serves nothing for it from then on, so \
             valuation carries its last stored close forward instead. Enter a price by hand \
             (PUT /closing_prices/:listing_id/:price_date) if you have one, or clear \
             unpriced_from on the listing if the security is quoted again",
            market.listing.ticker
        )));
    }
    if let Some(before) = market.listing.unpriced_before
        && date < before
    {
        return Err(ApiError::unprocessable(format!(
            "{} is unpriced before {before} — the provider's series for it begins then, so \
             there is nothing to fetch earlier and valuation leaves the holding out of those \
             dates' totals instead. Enter a price by hand \
             (PUT /closing_prices/:listing_id/:price_date) if you have one, or move \
             unpriced_before back on the listing if the series reaches further than it says",
            market.listing.ticker
        )));
    }
    Ok(())
}

/// 422 unless `date` is a trading day whose close has passed.
fn validate_complete_trading_day(market: &Market, date: NaiveDate) -> Result<(), ApiError> {
    let latest = market
        .latest_complete_trading_day(Utc::now())
        .map_err(unprocessable)?;
    if latest.is_none_or(|latest| date > latest) {
        return Err(ApiError::unprocessable(format!(
            "the close of {date} is not final yet"
        )));
    }
    if !market.is_trading_day(date) {
        return Err(ApiError::unprocessable(format!(
            "{date} is not a trading day"
        )));
    }
    Ok(())
}

fn internal(e: impl std::fmt::Display) -> ApiError {
    ApiError::internal(e.to_string())
}

fn unprocessable(e: impl std::fmt::Display) -> ApiError {
    ApiError::unprocessable(e.to_string())
}

pub fn router() -> Router<SqlitePool> {
    Router::new()
        .route("/closing_prices", get(list))
        .route("/closing_prices/fetch", post(fetch_one))
        .route("/closing_prices/backfill", post(backfill))
        .route(
            "/closing_prices/clear_unpriced_before",
            post(clear_unpriced_before),
        )
        .route(
            "/closing_prices/{listing_id}/{price_date}",
            put(put_manual).delete(delete_one),
        )
}

/// Reusable price-fetcher stub for the report tests (the daily-close tests in
/// this module use their own richer stub). Returns a canned latest quote per
/// listing, or a blanket failure for every listing.
#[cfg(test)]
pub mod test_support {
    use super::*;

    #[derive(Default)]
    pub struct QuoteStub {
        quotes: HashMap<i64, LatestQuote>,
        /// Daily closes keyed by **provider symbol**, so a stub can model a
        /// provider that serves a security's history only under the symbol it
        /// was quoted as at the time — the shape a rename produces.
        closes: HashMap<String, Vec<FetchedClose>>,
        fail: Option<String>,
    }

    impl QuoteStub {
        /// Canned daily closes served for `symbol` only. A fetch whose
        /// resolved symbol has no entry gets no candles, exactly as a
        /// provider does for a symbol it no longer serves.
        pub fn with_symbol_closes(
            mut self,
            symbol: &str,
            currency: &str,
            closes: &[(NaiveDate, &str)],
        ) -> Self {
            self.closes.insert(
                symbol.to_string(),
                closes
                    .iter()
                    .map(|(date, price)| FetchedClose {
                        date: *date,
                        price: price.parse().unwrap(),
                        currency: currency.to_string(),
                    })
                    .collect(),
            );
            self
        }

        pub fn with_quote(
            mut self,
            listing_id: i64,
            price: &str,
            currency: &str,
            as_of: DateTime<Utc>,
        ) -> Self {
            self.quotes.insert(
                listing_id,
                LatestQuote {
                    price: price.parse().unwrap(),
                    currency: currency.to_string(),
                    as_of,
                },
            );
            self
        }

        pub fn failing(msg: &str) -> Self {
            QuoteStub {
                fail: Some(msg.to_string()),
                ..Default::default()
            }
        }

        /// As a `SharedFetcher` for layering onto a report router.
        pub fn shared(self) -> SharedFetcher {
            Arc::new(self)
        }
    }

    impl PriceFetcher for QuoteStub {
        fn source(&self) -> &'static str {
            "stub"
        }

        /// The same resolution the live fetcher does, so a stub's stored
        /// `fetched_symbol` is the symbol a real fetch would have recorded.
        fn symbol(&self, market: &Market, date: NaiveDate) -> Result<String, String> {
            yahoo_symbol(market, date)
        }

        fn daily_closes<'a>(
            &'a self,
            market: &'a Market,
            from: NaiveDate,
            to: NaiveDate,
        ) -> FetchFuture<'a> {
            Box::pin(async move {
                if let Some(msg) = &self.fail {
                    return Err(msg.clone());
                }
                let symbol = self.symbol(market, from)?;
                Ok(self
                    .closes
                    .get(&symbol)
                    .map(|v| {
                        v.iter()
                            .filter(|c| c.date >= from && c.date <= to)
                            .cloned()
                            .collect()
                    })
                    .unwrap_or_default())
            })
        }

        fn latest_quote<'a>(&'a self, market: &'a Market) -> QuoteFuture<'a> {
            Box::pin(async move {
                if let Some(msg) = &self.fail {
                    return Err(msg.clone());
                }
                self.quotes
                    .get(&market.listing.id)
                    .cloned()
                    .ok_or_else(|| format!("no stub quote for listing {}", market.listing.id))
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::entities::exchange_holiday;
    use crate::test_support::{ApiClient, test_pool};
    use axum::http::StatusCode;
    use std::sync::Mutex;

    fn ymd(y: i32, m: u32, d: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, d).unwrap()
    }

    fn utc(y: i32, m: u32, d: u32, h: u32, min: u32) -> DateTime<Utc> {
        chrono::TimeZone::with_ymd_and_hms(&Utc, y, m, d, h, min, 0).unwrap()
    }

    async fn insert_listing(pool: &SqlitePool, id: i64, ticker: &str, mic: &str, currency: &str) {
        crate::test_support::listing(id)
            .ticker(ticker)
            .name(ticker)
            .mic(mic)
            .security_type(listing::SecurityType::Share)
            .currency(currency)
            .insert(pool)
            .await;
    }

    async fn insert_crypto_listing(pool: &SqlitePool, id: i64, ticker: &str) {
        crate::test_support::listing(id)
            .crypto()
            .ticker(ticker)
            .name(ticker)
            .insert(pool)
            .await;
    }

    async fn insert_buy(pool: &SqlitePool, id: i64, listing_id: i64, qty: &str) {
        crate::test_support::buy(id, listing_id)
            .date(ymd(2024, 1, 15))
            .qty(qty.parse().unwrap())
            .price(Decimal::from(10))
            .insert(pool)
            .await;
    }

    async fn sell_everything(
        pool: &SqlitePool,
        sell_id: i64,
        buy_id: i64,
        listing_id: i64,
        qty: &str,
    ) {
        crate::test_support::sell(sell_id, listing_id)
            .date(ymd(2024, 6, 3))
            .qty(qty.parse().unwrap())
            .price(Decimal::from(12))
            .insert(pool)
            .await;
        crate::test_support::allocate(pool, sell_id, sell_id, buy_id, qty.parse().unwrap()).await;
    }

    /// Stub provider: per-listing canned closes and latest quotes (keyed by
    /// listing id), or a blanket failure. Records every (listing, from, to)
    /// call.
    #[derive(Default)]
    struct StubFetcher {
        closes: HashMap<i64, Vec<FetchedClose>>,
        quotes: HashMap<i64, LatestQuote>,
        fail: Option<String>,
        calls: Mutex<Vec<(i64, NaiveDate, NaiveDate)>>,
        /// The resolved symbol (`yahoo_symbol`'s output) each `daily_closes`
        /// call was made with — lets a test confirm a backfill `symbol`
        /// override actually reached the fetcher.
        symbols: Mutex<Vec<String>>,
    }

    impl StubFetcher {
        fn with_close(mut self, listing_id: i64, date: NaiveDate, price: &str, ccy: &str) -> Self {
            self.closes
                .entry(listing_id)
                .or_default()
                .push(FetchedClose {
                    date,
                    price: price.parse().unwrap(),
                    currency: ccy.to_string(),
                });
            self
        }

        fn with_quote(
            mut self,
            listing_id: i64,
            price: &str,
            ccy: &str,
            as_of: DateTime<Utc>,
        ) -> Self {
            self.quotes.insert(
                listing_id,
                LatestQuote {
                    price: price.parse().unwrap(),
                    currency: ccy.to_string(),
                    as_of,
                },
            );
            self
        }

        fn failing(msg: &str) -> Self {
            StubFetcher {
                fail: Some(msg.to_string()),
                ..Default::default()
            }
        }

        fn calls(&self) -> Vec<(i64, NaiveDate, NaiveDate)> {
            self.calls.lock().unwrap().clone()
        }

        fn symbols(&self) -> Vec<String> {
            self.symbols.lock().unwrap().clone()
        }
    }

    impl PriceFetcher for StubFetcher {
        fn source(&self) -> &'static str {
            "stub"
        }

        fn symbol(&self, market: &Market, date: NaiveDate) -> Result<String, String> {
            yahoo_symbol(market, date)
        }

        fn daily_closes<'a>(
            &'a self,
            market: &'a Market,
            from: NaiveDate,
            to: NaiveDate,
        ) -> FetchFuture<'a> {
            Box::pin(async move {
                self.calls
                    .lock()
                    .unwrap()
                    .push((market.listing.id, from, to));
                self.symbols
                    .lock()
                    .unwrap()
                    .push(self.symbol(market, from).unwrap_or_default());
                if let Some(msg) = &self.fail {
                    return Err(msg.clone());
                }
                Ok(self
                    .closes
                    .get(&market.listing.id)
                    .map(|v| {
                        v.iter()
                            .filter(|c| c.date >= from && c.date <= to)
                            .cloned()
                            .collect()
                    })
                    .unwrap_or_default())
            })
        }

        fn latest_quote<'a>(&'a self, market: &'a Market) -> QuoteFuture<'a> {
            Box::pin(async move {
                if let Some(msg) = &self.fail {
                    return Err(msg.clone());
                }
                self.quotes
                    .get(&market.listing.id)
                    .cloned()
                    .ok_or_else(|| format!("no stub quote for listing {}", market.listing.id))
            })
        }
    }

    // 2026-06-05 is a Friday; 2026-06-06/07 the weekend.
    // 08:00 UTC = 18:00 Sydney (AEST) — after the 16:00 ASX close.
    fn friday_evening_sydney() -> DateTime<Utc> {
        utc(2026, 6, 5, 8, 0)
    }

    async fn insert_share_split(
        pool: &SqlitePool,
        listing_id: i64,
        date: NaiveDate,
        new: &str,
        old: &str,
    ) {
        crate::entities::corporate_action::db_upsert(
            pool,
            &crate::entities::corporate_action::CorporateAction {
                id: 900 + listing_id,
                listing_id,
                date,
                kind: crate::entities::corporate_action::ActionKind::ShareSplit {
                    split_new_units: new.parse().unwrap(),
                    split_old_units: old.parse().unwrap(),
                },
            },
        )
        .await
        .unwrap();
    }

    /// Record a rename through the entity's own path, so the chain and the
    /// listing row move together exactly as `POST /listings/:id/rename` does.
    async fn rename_listing(
        pool: &SqlitePool,
        listing_id: i64,
        effective_date: NaiveDate,
        new_ticker: &str,
        new_mic: Option<&str>,
    ) {
        crate::entities::listing_rename::db_rename(
            pool,
            listing_id,
            &crate::entities::listing_rename::RenameBody {
                effective_date,
                ticker: new_ticker.to_string(),
                exchange_mic: new_mic.map(str::to_string),
                name: None,
                price_symbol: None,
                note: None,
            },
        )
        .await
        .unwrap();
    }

    // --- clean_price ---

    #[test]
    fn clean_price_strips_float_noise_and_keeps_tiny_prices() {
        let cases = [
            ("62.4799995422363", "62.48"), // 62.48f32 — the live BHP.AX shape
            ("99545.3515625", "99545.35"), // 99545.35f32 — the live BTC-AUD shape
            ("141.5", "141.5"),
            ("0.000012345678", "0.00001234568"), // sub-$1: significance starts at the 1
        ];
        for (input, expected) in cases {
            assert_eq!(
                clean_price(input.parse().unwrap()),
                expected.parse::<Decimal>().unwrap(),
                "clean_price({input})"
            );
        }
    }

    // --- latest complete trading day ---

    #[tokio::test]
    async fn db_close_time_gates_same_day_collection() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "BHP", "XASX", "AUD").await;
        let market = load_market(&pool, 1).await.unwrap().unwrap();

        // Friday 15:00 Sydney (05:00 UTC): before the 16:00 close → Thursday.
        let before_close = utc(2026, 6, 5, 5, 0);
        assert_eq!(
            market.latest_complete_trading_day(before_close).unwrap(),
            Some(ymd(2026, 6, 4))
        );
        // Friday 18:00 Sydney: after the close → Friday itself.
        assert_eq!(
            market
                .latest_complete_trading_day(friday_evening_sydney())
                .unwrap(),
            Some(ymd(2026, 6, 5))
        );
    }

    #[tokio::test]
    async fn db_weekends_and_holidays_walk_back_to_a_trading_day() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "BHP", "XASX", "AUD").await;
        // Sunday 18:00 Sydney → Friday (weekend skipped).
        let market = load_market(&pool, 1).await.unwrap().unwrap();
        assert_eq!(
            market
                .latest_complete_trading_day(utc(2026, 6, 7, 8, 0))
                .unwrap(),
            Some(ymd(2026, 6, 5))
        );

        // With Friday seeded as a holiday, the walk lands on Thursday.
        exchange_holiday::db_upsert(
            &pool,
            &exchange_holiday::ExchangeHoliday {
                id: 0,
                mic: "XASX".to_string(),
                holiday_date: ymd(2026, 6, 5),
                name: "Test Holiday".to_string(),
            },
        )
        .await
        .unwrap();
        let market = load_market(&pool, 1).await.unwrap().unwrap();
        assert_eq!(
            market
                .latest_complete_trading_day(utc(2026, 6, 7, 8, 0))
                .unwrap(),
            Some(ymd(2026, 6, 4))
        );
    }

    #[tokio::test]
    async fn db_crypto_cutoff_is_utc_midnight_with_no_holiday_calendar() {
        let pool = test_pool().await;
        insert_crypto_listing(&pool, 1, "BTC").await;
        let market = load_market(&pool, 1).await.unwrap().unwrap();
        // Sunday 01:30 UTC: Saturday's UTC candle is complete — weekends and
        // holiday calendars don't apply to a continuously-trading asset.
        assert_eq!(
            market
                .latest_complete_trading_day(utc(2026, 6, 7, 1, 30))
                .unwrap(),
            Some(ymd(2026, 6, 6))
        );
    }

    // --- held listings ---

    #[tokio::test]
    async fn db_held_listings_excludes_fully_sold() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "BHP", "XASX", "AUD").await;
        insert_listing(&pool, 2, "SOLD", "XASX", "AUD").await;
        insert_listing(&pool, 3, "NEVER", "XASX", "AUD").await;
        insert_buy(&pool, 1, 1, "100").await;
        insert_buy(&pool, 2, 2, "50").await;
        sell_everything(&pool, 3, 2, 2, "50").await;

        assert_eq!(db_held_listing_ids(&pool, None).await.unwrap(), vec![1]);
        // As at a date before the sale, the sold listing still counts; before
        // any buys, nothing does.
        assert_eq!(
            db_held_listing_ids(&pool, Some(ymd(2024, 5, 31)))
                .await
                .unwrap(),
            vec![1, 2]
        );
        assert!(
            db_held_listing_ids(&pool, Some(ymd(2024, 1, 1)))
                .await
                .unwrap()
                .is_empty()
        );
    }

    // --- scheduled collection ---

    /// The 7-trading-day lookback window ending Friday 2026-06-05 on the ASX
    /// calendar (no seeded holiday falls inside it), oldest first.
    /// The ASX trading days in the collection window ending Friday
    /// 2026-06-05 — the last [`COLLECTION_LOOKBACK_DAYS`] calendar days, so
    /// from Saturday 2026-05-23, whose first trading day is Monday 2026-05-25.
    fn asx_lookback_window() -> Vec<NaiveDate> {
        vec![
            ymd(2026, 5, 25),
            ymd(2026, 5, 26),
            ymd(2026, 5, 27),
            ymd(2026, 5, 28),
            ymd(2026, 5, 29),
            ymd(2026, 6, 1),
            ymd(2026, 6, 2),
            ymd(2026, 6, 3),
            ymd(2026, 6, 4),
            ymd(2026, 6, 5),
        ]
    }

    /// Store an ok row directly (as an earlier successful run would have).
    async fn seed_ok_price(pool: &SqlitePool, listing_id: i64, date: NaiveDate) {
        crate::test_support::closing_price(listing_id, date)
            .source("stub")
            .fetched_at("2026-06-01T00:00:00Z")
            .insert(pool)
            .await;
    }

    /// Store an errored row directly (as an earlier failed run would have).
    async fn seed_errored_price(pool: &SqlitePool, listing_id: i64, date: NaiveDate, msg: &str) {
        crate::test_support::closing_price(listing_id, date)
            .source("stub")
            .fetched_at("2026-06-03T08:00:00Z")
            .errored(msg)
            .insert(pool)
            .await;
    }

    /// Store a hand-entered price directly, as `PUT /closing_prices/…` does.
    async fn seed_manual_price(pool: &SqlitePool, listing_id: i64, date: NaiveDate, price: &str) {
        crate::test_support::closing_price(listing_id, date)
            .price(price)
            .manual("asx.com.au closing report", "provider serves no candle")
            .insert(pool)
            .await;
    }

    #[tokio::test]
    async fn collection_stores_price_per_held_listing_and_skips_non_held() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "BHP", "XASX", "AUD").await;
        insert_listing(&pool, 2, "IDLE", "XASX", "AUD").await;
        insert_buy(&pool, 1, 1, "100").await;
        // The earlier window days are already stored ok; only Friday is new.
        for &d in asx_lookback_window().iter().rev().skip(1) {
            seed_ok_price(&pool, 1, d).await;
        }
        let fetcher = StubFetcher::default().with_close(1, ymd(2026, 6, 5), "62.48", "AUD");

        run_collection(&pool, &fetcher, friday_evening_sydney())
            .await
            .unwrap();

        let row = db_get_one(&pool, 1, ymd(2026, 6, 5))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(row.price, Some("62.48".parse().unwrap()));
        assert_eq!(row.status, PriceStatus::Ok);
        assert_eq!(row.source, "stub");
        assert!(row.error.is_none());
        let rows = db_list(&pool, Some(2), None, None).await.unwrap();
        assert!(rows.is_empty(), "the non-held listing is not collected");
        assert_eq!(
            fetcher.calls(),
            vec![(1, ymd(2026, 6, 5), ymd(2026, 6, 5))],
            "only the missing day is fetched"
        );
    }

    #[tokio::test]
    async fn collection_skips_days_already_stored_ok() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "BHP", "XASX", "AUD").await;
        insert_buy(&pool, 1, 1, "100").await;
        let mut fetcher = StubFetcher::default();
        for &d in &asx_lookback_window() {
            fetcher = fetcher.with_close(1, d, "62.48", "AUD");
        }
        run_collection(&pool, &fetcher, friday_evening_sydney())
            .await
            .unwrap();
        assert_eq!(
            fetcher.calls().len(),
            1,
            "one provider call spans the window"
        );
        assert_eq!(db_list(&pool, None, None, None).await.unwrap().len(), 10);

        // A second run (same evening) finds every window day ok: no re-fetch.
        run_collection(&pool, &fetcher, friday_evening_sydney())
            .await
            .unwrap();
        assert_eq!(fetcher.calls().len(), 1, "no second provider call");
        assert_eq!(db_list(&pool, None, None, None).await.unwrap().len(), 10);
    }

    /// The lookback self-heals: a day stored errored (and a day missed
    /// outright) is re-attempted by the next run — with the days already ok
    /// never re-fetched — so the daily runs are each other's retries.
    #[tokio::test]
    async fn collection_backfills_missing_and_errored_days_in_the_lookback() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "BHP", "XASX", "AUD").await;
        insert_buy(&pool, 1, 1, "100").await;
        // Window state: Wed errored, Thu missing, Fri missing; the rest ok.
        for &d in &asx_lookback_window()[..7] {
            seed_ok_price(&pool, 1, d).await;
        }
        seed_errored_price(&pool, 1, ymd(2026, 6, 3), "provider down").await;

        let fetcher = StubFetcher::default()
            .with_close(1, ymd(2026, 6, 3), "64.91", "AUD")
            .with_close(1, ymd(2026, 6, 4), "63.10", "AUD")
            .with_close(1, ymd(2026, 6, 5), "62.48", "AUD");
        run_collection(&pool, &fetcher, friday_evening_sydney())
            .await
            .unwrap();

        // One call spanning exactly the days that needed work.
        assert_eq!(fetcher.calls(), vec![(1, ymd(2026, 6, 3), ymd(2026, 6, 5))]);
        for (d, price) in [
            (ymd(2026, 6, 3), "64.91"),
            (ymd(2026, 6, 4), "63.10"),
            (ymd(2026, 6, 5), "62.48"),
        ] {
            let row = db_get_one(&pool, 1, d).await.unwrap().unwrap();
            assert_eq!(row.status, PriceStatus::Ok, "{d}");
            assert_eq!(row.price, Some(price.parse().unwrap()), "{d}");
        }
    }

    #[tokio::test]
    async fn collection_failure_stores_errored_rows_and_fails_the_job() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "BHP", "XASX", "AUD").await;
        insert_buy(&pool, 1, 1, "100").await;
        let fetcher = StubFetcher::failing("provider down");

        let err = run_collection(&pool, &fetcher, friday_evening_sydney())
            .await
            .unwrap_err();
        assert!(err.contains("BHP"), "job error names the listing: {err}");

        let rows = db_list(&pool, None, None, None).await.unwrap();
        assert_eq!(
            rows.len(),
            asx_lookback_window().len(),
            "every attempted window day is recorded, never silently missing"
        );
        assert!(rows.iter().all(|r| r.status == PriceStatus::Error));
        assert!(rows.iter().all(|r| r.price.is_none()));
        assert!(rows[0].error.as_deref().unwrap().contains("provider down"));
    }

    #[tokio::test]
    async fn collection_replaces_errored_rows_once_the_provider_recovers() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "BHP", "XASX", "AUD").await;
        insert_buy(&pool, 1, 1, "100").await;
        run_collection(
            &pool,
            &StubFetcher::failing("down"),
            friday_evening_sydney(),
        )
        .await
        .unwrap_err();

        let mut fetcher = StubFetcher::default();
        for &d in &asx_lookback_window() {
            fetcher = fetcher.with_close(1, d, "62.48", "AUD");
        }
        run_collection(&pool, &fetcher, friday_evening_sydney())
            .await
            .unwrap();

        let rows = db_list(&pool, None, None, None).await.unwrap();
        assert_eq!(rows.len(), asx_lookback_window().len());
        assert!(rows.iter().all(|r| r.status == PriceStatus::Ok));
        assert!(rows.iter().all(|r| r.error.is_none()));
    }

    #[tokio::test]
    async fn collection_records_currency_mismatch_as_error() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "ICE", "XNYS", "USD").await;
        insert_buy(&pool, 1, 1, "10").await;
        // Only Friday is missing; the provider quotes AUD for a USD listing —
        // wrong symbol mapping; the price must not be stored as if it were USD.
        for &d in asx_lookback_window().iter().rev().skip(1) {
            seed_ok_price(&pool, 1, d).await;
        }
        let fetcher = StubFetcher::default().with_close(1, ymd(2026, 6, 5), "141.50", "AUD");

        // 21:00 UTC Friday = 17:00 New York, after the close.
        run_collection(&pool, &fetcher, utc(2026, 6, 5, 21, 0))
            .await
            .unwrap_err();
        let row = db_get_one(&pool, 1, ymd(2026, 6, 5))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(row.status, PriceStatus::Error);
        assert!(row.error.as_deref().unwrap().contains("currency mismatch"));
    }

    #[tokio::test]
    async fn collection_crypto_collected_daily_at_utc_cutoff() {
        let pool = test_pool().await;
        insert_crypto_listing(&pool, 1, "BTC").await;
        insert_buy(&pool, 1, 1, "0.5").await;
        // Crypto trades every day: the lookback is the COLLECTION_LOOKBACK_DAYS
        // calendar days ending Saturday 2026-06-06; all but Saturday are
        // already stored ok.
        for i in 1..COLLECTION_LOOKBACK_DAYS {
            seed_ok_price(&pool, 1, ymd(2026, 6, 6) - Duration::days(i)).await;
        }
        let fetcher = StubFetcher::default().with_close(1, ymd(2026, 6, 6), "86378.35", "AUD");

        // Sunday 01:30 UTC: Saturday 2026-06-06 is a complete crypto day.
        run_collection(&pool, &fetcher, utc(2026, 6, 7, 1, 30))
            .await
            .unwrap();
        let row = db_get_one(&pool, 1, ymd(2026, 6, 6))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(row.status, PriceStatus::Ok);
        assert_eq!(row.price, Some("86378.35".parse().unwrap()));
        assert_eq!(fetcher.calls(), vec![(1, ymd(2026, 6, 6), ymd(2026, 6, 6))]);
    }

    // --- backfill ---

    fn full_router(pool: SqlitePool, fetcher: StubFetcher) -> ApiClient {
        let shared: SharedFetcher = Arc::new(fetcher);
        ApiClient::over(router().with_state(pool).layer(Extension(shared)))
    }

    async fn post_json(
        app: &ApiClient,
        uri: &str,
        body: serde_json::Value,
    ) -> (StatusCode, axum::body::Bytes) {
        let resp = app.post(uri, &body).await;
        let status = resp.status;
        let bytes = resp.body.clone();
        (status, bytes)
    }

    async fn delete_req(app: &ApiClient, uri: &str) -> (StatusCode, axum::body::Bytes) {
        let resp = app.delete(uri).await;
        let status = resp.status;
        let bytes = resp.body.clone();
        (status, bytes)
    }

    /// Store one errored row for (listing 1, `date`) via the normal fetch
    /// path — a stub with no candle for the day.
    async fn store_errored(pool: &SqlitePool, date: NaiveDate) {
        let market = load_market(pool, 1).await.unwrap().unwrap();
        let (_, errored) = fetch_and_store(pool, &StubFetcher::default(), &market, &[date])
            .await
            .unwrap();
        assert_eq!(errored, 1);
    }

    // --- delete ---

    /// An errored row for a day that can never have a price (here: before the
    /// security's first trading day) is deletable, which is the only way to
    /// stop `reports::health` reporting it forever.
    #[tokio::test]
    async fn api_delete_removes_an_errored_row() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "HNDQ", "XASX", "AUD").await;
        insert_buy(&pool, 1, 1, "100").await;
        store_errored(&pool, ymd(2026, 6, 2)).await;
        let app = full_router(pool.clone(), StubFetcher::default());

        let (status, bytes) = delete_req(&app, "/closing_prices/1/2026-06-02").await;
        assert_eq!(
            status,
            StatusCode::NO_CONTENT,
            "{}",
            String::from_utf8_lossy(&bytes)
        );
        assert!(bytes.is_empty());
        assert!(
            db_get_one(&pool, 1, ymd(2026, 6, 2))
                .await
                .unwrap()
                .is_none()
        );
        // The health report's standing alarm is cleared with it.
        let health = crate::reports::health::db_health(&pool, ymd(2026, 6, 3), Utc::now())
            .await
            .unwrap();
        assert!(health.errored_prices.is_empty());
    }

    /// An ok row is never deletable: real price data is replaced by a
    /// re-fetch, so the endpoint cannot punch a hole in a valued series.
    #[tokio::test]
    async fn api_delete_rejects_an_ok_row() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "BHP", "XASX", "AUD").await;
        insert_buy(&pool, 1, 1, "100").await;
        let market = load_market(&pool, 1).await.unwrap().unwrap();
        let stub = StubFetcher::default().with_close(1, ymd(2026, 6, 2), "62.48", "AUD");
        fetch_and_store(&pool, &stub, &market, &[ymd(2026, 6, 2)])
            .await
            .unwrap();
        let app = full_router(pool.clone(), StubFetcher::default());

        let (status, bytes) = delete_req(&app, "/closing_prices/1/2026-06-02").await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
        let msg = String::from_utf8_lossy(&bytes);
        assert!(msg.contains("re-fetch it"), "points at the fix: {msg}");
        let row = db_get_one(&pool, 1, ymd(2026, 6, 2))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(row.status, PriceStatus::Ok, "the price is still stored");
    }

    // --- delete inside an `unpriced_before` span (migration 0037) ---

    /// Mark an already-inserted listing as unpriced before `before`: the
    /// provider's series for it begins then, so every stored row earlier than
    /// it is superseded by the listing's own declaration.
    async fn mark_unpriced_before(
        pool: &SqlitePool,
        id: i64,
        ticker: &str,
        before: NaiveDate,
    ) -> listing::Listing {
        let marked = crate::test_support::listing(id)
            .ticker(ticker)
            .name(ticker)
            .security_type(listing::SecurityType::Share)
            .unpriced_before(before)
            .build();
        listing::db_upsert(pool, &marked).await.unwrap();
        marked
    }

    /// The one relaxation of the ok-row rule, and the case it exists for: a
    /// span the listing itself declares unpriceable, stored from another
    /// security's series. Valuation excludes the holding from those dates
    /// rather than pricing it, so no stored figure was ever valued at these
    /// rows and deleting them punches no hole — whichever way they arrived.
    #[tokio::test]
    async fn api_delete_removes_an_ok_row_the_unpriced_before_marker_supersedes() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "LAC", "XASX", "AUD").await;
        insert_buy(&pool, 1, 1, "100").await;
        crate::test_support::closing_price(1, ymd(2026, 6, 2))
            .price("10.13")
            .insert(&pool)
            .await;
        seed_manual_price(&pool, 1, ymd(2026, 6, 3), "9.87").await;
        mark_unpriced_before(&pool, 1, "LAC", ymd(2026, 6, 4)).await;
        let app = full_router(pool.clone(), StubFetcher::default());

        for date in ["2026-06-02", "2026-06-03"] {
            let (status, bytes) = delete_req(&app, &format!("/closing_prices/1/{date}")).await;
            assert_eq!(
                status,
                StatusCode::NO_CONTENT,
                "{date}: {}",
                String::from_utf8_lossy(&bytes)
            );
        }
        assert!(
            db_get_one(&pool, 1, ymd(2026, 6, 2))
                .await
                .unwrap()
                .is_none(),
            "the fetched row is gone"
        );
        assert!(
            db_get_one(&pool, 1, ymd(2026, 6, 3))
                .await
                .unwrap()
                .is_none(),
            "the hand-entered row goes the same way — origin decides nothing here"
        );
    }

    /// The relaxation stops exactly at the marker: a row on the day the
    /// series begins, or after it, is an ordinary priced day again and the
    /// original refusal stands word for word.
    #[tokio::test]
    async fn api_delete_still_rejects_an_ok_row_on_or_after_unpriced_before() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "LAC", "XASX", "AUD").await;
        crate::test_support::closing_price(1, ymd(2026, 6, 4))
            .price("10.13")
            .insert(&pool)
            .await;
        crate::test_support::closing_price(1, ymd(2026, 6, 5))
            .price("10.50")
            .insert(&pool)
            .await;
        mark_unpriced_before(&pool, 1, "LAC", ymd(2026, 6, 4)).await;
        let app = full_router(pool.clone(), StubFetcher::default());

        for date in ["2026-06-04", "2026-06-05"] {
            let (status, bytes) = delete_req(&app, &format!("/closing_prices/1/{date}")).await;
            assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{date}");
            let msg = String::from_utf8_lossy(&bytes);
            assert!(msg.contains("is ok, not errored"), "{date}: {msg}");
            assert!(msg.contains("re-fetch it"), "{date}: {msg}");
            assert!(
                db_get_one(&pool, 1, date.parse().unwrap())
                    .await
                    .unwrap()
                    .is_some(),
                "{date} is still stored"
            );
        }
    }

    /// The two markers are **not** symmetric, and this is why the relaxation
    /// is only at one end. A date on or after `unpriced_from` *is* valued —
    /// `reports::valuation` carries the last stored ok close forward into it
    /// — so deleting a row there could remove the very figure being carried.
    /// The refusal stands.
    #[tokio::test]
    async fn api_delete_still_rejects_an_ok_row_inside_an_unpriced_from_run() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "SUSP", "XASX", "AUD").await;
        crate::test_support::closing_price(1, ymd(2026, 6, 2))
            .price("3.10")
            .insert(&pool)
            .await;
        seed_manual_price(&pool, 1, ymd(2026, 6, 4), "2.95").await;
        let marked = crate::test_support::listing(1)
            .ticker("SUSP")
            .name("SUSP")
            .security_type(listing::SecurityType::Share)
            .unpriced_from(ymd(2026, 6, 3))
            .build();
        listing::db_upsert(&pool, &marked).await.unwrap();
        let app = full_router(pool.clone(), StubFetcher::default());

        let (status, bytes) = delete_req(&app, "/closing_prices/1/2026-06-04").await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
        assert!(
            String::from_utf8_lossy(&bytes).contains("is ok, not errored"),
            "{}",
            String::from_utf8_lossy(&bytes)
        );
        // …and that row really is what a valuation of the unpriced run reads.
        assert_eq!(
            db_latest_ok_price_on_or_before(&pool, 1, ymd(2026, 6, 10), None)
                .await
                .unwrap(),
            Some((ymd(2026, 6, 4), "2.95".parse().unwrap())),
            "the refused row is the carried-forward figure"
        );
    }

    /// Nothing is destroyed: a superseded row's figure and the provenance
    /// that says what it was land in the audit trail, which is the property
    /// the whole cleanup rests on.
    #[tokio::test]
    async fn deleting_a_superseded_price_is_recorded_in_the_audit_trail() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "LAC", "XASX", "AUD").await;
        seed_manual_price(&pool, 1, ymd(2026, 6, 3), "9.87").await;
        let row = db_get_one(&pool, 1, ymd(2026, 6, 3))
            .await
            .unwrap()
            .unwrap();
        mark_unpriced_before(&pool, 1, "LAC", ymd(2026, 6, 4)).await;
        let app = full_router(pool.clone(), StubFetcher::default());

        let (status, _) = delete_req(&app, "/closing_prices/1/2026-06-03").await;
        assert_eq!(status, StatusCode::NO_CONTENT);

        let history = crate::reports::row_history::db_row_history(&pool, "closing_prices", row.id)
            .await
            .unwrap();
        assert_eq!(history.len(), 1);
        assert_eq!(history[0]["operation"], "DELETE");
        assert_eq!(history[0]["price"], "9.87");
        assert_eq!(history[0]["sourced_from"], "asx.com.au closing report");
        assert_eq!(history[0]["reason"], "provider serves no candle");
    }

    // --- clearing a whole superseded span ---

    /// The bulk form: hundreds of borrowed days are not a runbook one DELETE
    /// at a time. The span is the listing's own marker — never a caller's
    /// date range — so it clears exactly what the declaration supersedes,
    /// leaves the priced days alone, says how many rows went, and is safe to
    /// run again.
    #[tokio::test]
    async fn api_clear_unpriced_before_clears_exactly_the_superseded_span() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "LAC", "XASX", "AUD").await;
        insert_listing(&pool, 2, "BHP", "XASX", "AUD").await;
        insert_buy(&pool, 1, 1, "100").await;
        // Listing 1: two borrowed ok rows and an errored one before the
        // marker, one real row on the day the series begins.
        crate::test_support::closing_price(1, ymd(2026, 6, 1))
            .price("10.13")
            .insert(&pool)
            .await;
        seed_manual_price(&pool, 1, ymd(2026, 6, 2), "9.87").await;
        crate::test_support::closing_price(1, ymd(2026, 6, 3))
            .errored("no candle")
            .insert(&pool)
            .await;
        crate::test_support::closing_price(1, ymd(2026, 6, 4))
            .price("24.90")
            .insert(&pool)
            .await;
        // Another listing's row on a date inside the span stays put: the
        // marker is listing 1's declaration and nobody else's.
        crate::test_support::closing_price(2, ymd(2026, 6, 1))
            .price("62.48")
            .insert(&pool)
            .await;
        mark_unpriced_before(&pool, 1, "LAC", ymd(2026, 6, 4)).await;
        let app = full_router(pool.clone(), StubFetcher::default());

        let (status, bytes) = post_json(
            &app,
            "/closing_prices/clear_unpriced_before",
            serde_json::json!({ "listing_id": 1 }),
        )
        .await;
        assert_eq!(
            status,
            StatusCode::OK,
            "{}",
            String::from_utf8_lossy(&bytes)
        );
        let summary: ClearSummary = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(summary.listing_id, 1);
        assert_eq!(summary.unpriced_before, ymd(2026, 6, 4));
        assert_eq!(summary.deleted, 3, "both ok rows and the errored one");

        for gone in [ymd(2026, 6, 1), ymd(2026, 6, 2), ymd(2026, 6, 3)] {
            assert!(
                db_get_one(&pool, 1, gone).await.unwrap().is_none(),
                "{gone} was superseded"
            );
        }
        assert!(
            db_get_one(&pool, 1, ymd(2026, 6, 4))
                .await
                .unwrap()
                .is_some(),
            "the day the series begins is a real price"
        );
        assert!(
            db_get_one(&pool, 2, ymd(2026, 6, 1))
                .await
                .unwrap()
                .is_some(),
            "another listing's prices are not in this listing's span"
        );

        // Idempotent: re-running clears nothing and says so.
        let (status, bytes) = post_json(
            &app,
            "/closing_prices/clear_unpriced_before",
            serde_json::json!({ "listing_id": 1 }),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let again: ClearSummary = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(again.deleted, 0);
    }

    /// Without a marker there is no superseded span, so there is nothing this
    /// endpoint may clear — it must never become a bulk-delete of real price
    /// history. An unknown listing is the ordinary 404.
    #[tokio::test]
    async fn api_clear_unpriced_before_is_refused_without_a_marker() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "BHP", "XASX", "AUD").await;
        crate::test_support::closing_price(1, ymd(2026, 6, 2))
            .price("62.48")
            .insert(&pool)
            .await;
        let app = full_router(pool.clone(), StubFetcher::default());

        let (status, bytes) = post_json(
            &app,
            "/closing_prices/clear_unpriced_before",
            serde_json::json!({ "listing_id": 1 }),
        )
        .await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
        let msg = String::from_utf8_lossy(&bytes);
        assert!(msg.contains("BHP has no unpriced_before"), "{msg}");
        assert!(
            db_get_one(&pool, 1, ymd(2026, 6, 2))
                .await
                .unwrap()
                .is_some(),
            "nothing was cleared"
        );

        let (status, bytes) = post_json(
            &app,
            "/closing_prices/clear_unpriced_before",
            serde_json::json!({ "listing_id": 99 }),
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        assert_eq!(String::from_utf8_lossy(&bytes), "no such listing");
    }

    /// The audit trail is per row, not per statement: the `AFTER DELETE`
    /// trigger fires once for each row of the multi-row DELETE, so a cleared
    /// span leaves every figure and every `reason` recoverable — including
    /// the note explaining what the borrowed prices were.
    #[tokio::test]
    async fn clearing_a_span_records_every_row_in_the_audit_trail() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "LAC", "XASX", "AUD").await;
        crate::test_support::closing_price(1, ymd(2026, 6, 1))
            .price("10.13")
            .insert(&pool)
            .await;
        seed_manual_price(&pool, 1, ymd(2026, 6, 2), "9.87").await;
        let fetched = db_get_one(&pool, 1, ymd(2026, 6, 1))
            .await
            .unwrap()
            .unwrap();
        let manual = db_get_one(&pool, 1, ymd(2026, 6, 2))
            .await
            .unwrap()
            .unwrap();
        mark_unpriced_before(&pool, 1, "LAC", ymd(2026, 6, 4)).await;

        let cleared = db_clear_unpriced_before(&pool, 1).await.unwrap();
        assert_eq!(
            cleared,
            ClearOutcome::Cleared {
                unpriced_before: ymd(2026, 6, 4),
                deleted: 2,
            }
        );

        let history =
            crate::reports::row_history::db_row_history(&pool, "closing_prices", fetched.id)
                .await
                .unwrap();
        assert_eq!(history.len(), 1);
        assert_eq!(history[0]["operation"], "DELETE");
        assert_eq!(history[0]["price"], "10.13");

        let history =
            crate::reports::row_history::db_row_history(&pool, "closing_prices", manual.id)
                .await
                .unwrap();
        assert_eq!(history.len(), 1);
        assert_eq!(history[0]["operation"], "DELETE");
        assert_eq!(history[0]["price"], "9.87");
        assert_eq!(history[0]["reason"], "provider serves no candle");
    }

    /// Clearing the span cannot break the other marker's write-time pairing:
    /// `unpriced_from` needs a stored ok price *before* it to carry forward,
    /// and that check only ever looks at rows on or after `unpriced_before` —
    /// exactly the rows the clear leaves alone.
    #[tokio::test]
    async fn clearing_a_span_leaves_the_carry_forward_price_and_its_rule_intact() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "LAC", "XASX", "AUD").await;
        crate::test_support::closing_price(1, ymd(2026, 6, 1))
            .price("10.13")
            .insert(&pool)
            .await;
        crate::test_support::closing_price(1, ymd(2026, 6, 4))
            .price("24.90")
            .insert(&pool)
            .await;
        let marked = crate::test_support::listing(1)
            .ticker("LAC")
            .name("LAC")
            .security_type(listing::SecurityType::Share)
            .unpriced_before(ymd(2026, 6, 4))
            .unpriced_from(ymd(2026, 6, 5))
            .build();
        listing::db_upsert(&pool, &marked).await.unwrap();

        db_clear_unpriced_before(&pool, 1).await.unwrap();

        assert_eq!(
            db_latest_ok_price_on_or_before(&pool, 1, ymd(2026, 6, 9), Some(ymd(2026, 6, 4)))
                .await
                .unwrap(),
            Some((ymd(2026, 6, 4), "24.90".parse().unwrap())),
            "the figure the unpriced run carries forward is untouched"
        );
        // …and the pairing still accepts a re-save of the listing.
        listing::db_upsert(&pool, &marked).await.unwrap();
    }

    // --- manual prices ---

    async fn put_json(
        app: &ApiClient,
        uri: &str,
        body: serde_json::Value,
    ) -> (StatusCode, axum::body::Bytes) {
        let resp = app.put(uri, &body).await;
        let status = resp.status;
        let bytes = resp.body.clone();
        (status, bytes)
    }

    fn manual_body(price: &str) -> serde_json::Value {
        serde_json::json!({
            "price": price,
            "sourced_from": "asx.com.au closing report",
            "reason": "provider serves no candle since the delisting",
        })
    }

    /// A day the provider cannot serve is priced by hand, and the row records
    /// both halves of its provenance — where the figure came from and why it
    /// had to be entered — with the provider slot moved to `manual`.
    #[tokio::test]
    async fn api_manual_price_stores_the_price_with_its_provenance() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "BHP", "XASX", "AUD").await;
        insert_buy(&pool, 1, 1, "100").await;
        let app = full_router(pool.clone(), StubFetcher::default());

        let (status, bytes) =
            put_json(&app, "/closing_prices/1/2026-06-04", manual_body("62.48")).await;
        assert_eq!(
            status,
            StatusCode::NO_CONTENT,
            "{}",
            String::from_utf8_lossy(&bytes)
        );
        assert!(bytes.is_empty());

        let row = db_get_one(&pool, 1, ymd(2026, 6, 4))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(row.price, Some("62.48".parse().unwrap()));
        assert_eq!(row.status, PriceStatus::Ok);
        assert_eq!(row.origin, PriceOrigin::Manual);
        assert_eq!(row.source, "manual");
        assert_eq!(
            row.sourced_from.as_deref(),
            Some("asx.com.au closing report")
        );
        assert_eq!(
            row.reason.as_deref(),
            Some("provider serves no candle since the delisting")
        );
        assert!(row.error.is_none());
    }

    /// A manual price is read by valuation exactly like a fetched one: it is
    /// the way a date the provider blocked forever starts producing snapshots.
    #[tokio::test]
    async fn manual_price_unblocks_valuation_of_an_errored_day() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "BHP", "XASX", "AUD").await;
        insert_buy(&pool, 1, 1, "100").await;
        store_errored(&pool, ymd(2026, 6, 4)).await;
        let now = utc(2026, 6, 8, 9, 0);

        let blocked = crate::reports::valuation::stored_valuations(&pool, ymd(2026, 6, 4), now)
            .await
            .unwrap_err();
        assert!(
            blocked.to_string().contains("errored"),
            "setup: the day is blocked — {blocked}"
        );

        let app = full_router(pool.clone(), StubFetcher::default());
        let (status, bytes) =
            put_json(&app, "/closing_prices/1/2026-06-04", manual_body("62.48")).await;
        assert_eq!(status, StatusCode::NO_CONTENT, "{:?}", bytes);

        let valuations = crate::reports::valuation::stored_valuations(&pool, ymd(2026, 6, 4), now)
            .await
            .unwrap();
        assert_eq!(valuations.valuations.len(), 1);
        assert_eq!(
            valuations.valuations[0].native_price,
            "62.48".parse().unwrap()
        );
        assert_eq!(valuations.valuations[0].aud_price, "62.48".parse().unwrap());
    }

    /// Both provenance fields are required, and whitespace does not satisfy
    /// them: a hand-entered figure with no sourcing or reason is exactly the
    /// unauditable row the columns exist to prevent.
    #[tokio::test]
    async fn api_manual_price_requires_both_provenance_fields() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "BHP", "XASX", "AUD").await;
        let app = full_router(pool.clone(), StubFetcher::default());

        for (sourced_from, reason, expected) in [
            ("   ", "provider has no candle", "sourced_from is required"),
            ("asx.com.au", "  ", "reason is required"),
        ] {
            let body = serde_json::json!({
                "price": "62.48", "sourced_from": sourced_from, "reason": reason,
            });
            let (status, bytes) = put_json(&app, "/closing_prices/1/2026-06-04", body).await;
            assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
            let msg = String::from_utf8_lossy(&bytes);
            assert!(msg.contains(expected), "names the missing field: {msg}");
        }
        assert!(
            db_get_one(&pool, 1, ymd(2026, 6, 4))
                .await
                .unwrap()
                .is_none(),
            "nothing is stored for a rejected entry"
        );
    }

    /// A price that can never exist is refused rather than stored: zero or
    /// negative is a typo, not a close.
    #[tokio::test]
    async fn api_manual_price_rejects_a_non_positive_price() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "BHP", "XASX", "AUD").await;
        let app = full_router(pool.clone(), StubFetcher::default());

        for price in ["0", "-1.50"] {
            let (status, bytes) =
                put_json(&app, "/closing_prices/1/2026-06-04", manual_body(price)).await;
            assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{price}");
            let msg = String::from_utf8_lossy(&bytes);
            assert!(msg.contains("must be positive"), "{msg}");
        }
    }

    /// The same trading-day gate as a fetch: valuation only ever reads a
    /// trading day whose close is final, so a manual price on any other date
    /// would be a row nothing could use.
    #[tokio::test]
    async fn api_manual_price_rejects_non_trading_days_and_unfinished_closes() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "BHP", "XASX", "AUD").await;
        let app = full_router(pool.clone(), StubFetcher::default());

        // 2026-06-06 is a Saturday.
        let (status, bytes) =
            put_json(&app, "/closing_prices/1/2026-06-06", manual_body("62.48")).await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
        assert!(
            String::from_utf8_lossy(&bytes).contains("not a trading day"),
            "{}",
            String::from_utf8_lossy(&bytes)
        );

        // A date whose close cannot have happened yet.
        let future = (Utc::now() + Duration::days(30)).date_naive();
        let (status, bytes) = put_json(
            &app,
            &format!("/closing_prices/1/{future}"),
            manual_body("62.48"),
        )
        .await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
        assert!(
            String::from_utf8_lossy(&bytes).contains("not final yet"),
            "{}",
            String::from_utf8_lossy(&bytes)
        );
    }

    /// SCENARIOS L-15. A crypto asset trades every calendar day, so the
    /// trading-day gate that refuses a weekend price on an exchange listing
    /// must let the same Saturday through for an exchange-less one — otherwise
    /// the way out of a day the provider has no candle for (a hand-entered
    /// price) would be closed on two days in every seven.
    #[tokio::test]
    async fn api_manual_price_accepts_a_weekend_day_for_crypto_only() {
        let pool = test_pool().await;
        insert_crypto_listing(&pool, 1, "BTC").await;
        insert_listing(&pool, 2, "BHP", "XASX", "AUD").await;
        let app = full_router(pool.clone(), StubFetcher::default());

        // 2026-06-06 is a Saturday: a trading day for BTC, not for the ASX.
        let (status, bytes) =
            put_json(&app, "/closing_prices/1/2026-06-06", manual_body("91000")).await;
        assert_eq!(
            status,
            StatusCode::NO_CONTENT,
            "{}",
            String::from_utf8_lossy(&bytes)
        );
        let stored = db_get_one(&pool, 1, ymd(2026, 6, 6))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(stored.price, Some(Decimal::from(91000)));

        let (status, bytes) =
            put_json(&app, "/closing_prices/2/2026-06-06", manual_body("62.48")).await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
        assert!(
            String::from_utf8_lossy(&bytes).contains("not a trading day"),
            "{}",
            String::from_utf8_lossy(&bytes)
        );
    }

    #[tokio::test]
    async fn api_manual_price_unknown_listing_is_404() {
        let pool = test_pool().await;
        let app = full_router(pool.clone(), StubFetcher::default());

        let (status, _) =
            put_json(&app, "/closing_prices/9/2026-06-04", manual_body("62.48")).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
    }

    /// The provider never takes a hand-priced day back: an explicit re-fetch
    /// is refused, so a deliberate correction cannot be lost to a stray click
    /// — and the refusal quotes the reason so the user sees why it exists.
    #[tokio::test]
    async fn api_fetch_refuses_to_replace_a_manual_price() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "BHP", "XASX", "AUD").await;
        insert_buy(&pool, 1, 1, "100").await;
        seed_manual_price(&pool, 1, ymd(2026, 6, 4), "62.48").await;
        let stub = StubFetcher::default().with_close(1, ymd(2026, 6, 4), "99.99", "AUD");
        let app = full_router(pool.clone(), stub);

        let (status, bytes) = post_json(
            &app,
            "/closing_prices/fetch",
            serde_json::json!({ "listing_id": 1, "price_date": "2026-06-04" }),
        )
        .await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
        let msg = String::from_utf8_lossy(&bytes);
        assert!(msg.contains("entered manually"), "{msg}");
        assert!(
            msg.contains("provider serves no candle"),
            "quotes why: {msg}"
        );

        let row = db_get_one(&pool, 1, ymd(2026, 6, 4))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(row.price, Some("62.48".parse().unwrap()), "untouched");
        assert_eq!(row.origin, PriceOrigin::Manual);
    }

    /// SCENARIOS Q-02: a listing marked `unpriced_from` is not fetched from
    /// that date on — every call would only store another errored row, fail
    /// the job, and nag from health forever. The days *before* it are still
    /// collected.
    #[tokio::test]
    async fn collection_skips_a_listing_from_its_unpriced_from_date() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "ATVI", "XASX", "AUD").await;
        insert_buy(&pool, 1, 1, "100").await;
        for &d in &asx_lookback_window() {
            if d < ymd(2026, 6, 3) {
                seed_ok_price(&pool, 1, d).await;
            }
        }
        let marked = crate::test_support::listing(1)
            .ticker("ATVI")
            .name("ATVI")
            .mic("XASX")
            .security_type(listing::SecurityType::Share)
            .unpriced_from(ymd(2026, 6, 3))
            .build();
        listing::db_upsert(&pool, &marked).await.unwrap();

        let fetcher = StubFetcher::default();
        run_collection(&pool, &fetcher, friday_evening_sydney())
            .await
            .unwrap();
        assert!(
            fetcher.calls().is_empty(),
            "nothing left to fetch before the date, nothing fetched after it: {:?}",
            fetcher.calls()
        );
        assert!(
            db_get_one(&pool, 1, ymd(2026, 6, 5))
                .await
                .unwrap()
                .is_none(),
            "no errored row is stored for a day the provider cannot serve"
        );
    }

    /// The explicit paths refuse the same dates: a single re-fetch is `422`
    /// naming the marker, and a backfill crossing it fills the priced part
    /// and stops.
    #[tokio::test]
    async fn api_fetch_and_backfill_stop_at_unpriced_from() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "ATVI", "XASX", "AUD").await;
        insert_buy(&pool, 1, 1, "100").await;
        seed_ok_price(&pool, 1, ymd(2026, 6, 1)).await;
        let marked = crate::test_support::listing(1)
            .ticker("ATVI")
            .name("ATVI")
            .mic("XASX")
            .security_type(listing::SecurityType::Share)
            .unpriced_from(ymd(2026, 6, 3))
            .build();
        listing::db_upsert(&pool, &marked).await.unwrap();

        let mut stub = StubFetcher::default();
        for d in [ymd(2026, 6, 2), ymd(2026, 6, 3), ymd(2026, 6, 4)] {
            stub = stub.with_close(1, d, "94.42", "AUD");
        }
        let app = full_router(pool.clone(), stub);

        let (status, bytes) = post_json(
            &app,
            "/closing_prices/fetch",
            serde_json::json!({ "listing_id": 1, "price_date": "2026-06-04" }),
        )
        .await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
        let msg = String::from_utf8_lossy(&bytes);
        assert!(msg.contains("unpriced from 2026-06-03"), "{msg}");

        let (status, bytes) = post_json(
            &app,
            "/closing_prices/backfill",
            serde_json::json!({ "listing_id": 1, "from": "2026-06-01", "to": "2026-06-04" }),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let summary: BackfillSummary = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(summary.trading_days, 2, "1 and 2 June, not 3 or 4");
        assert_eq!(summary.already_stored, 1);
        assert_eq!(summary.fetched_ok, 1);
        assert_eq!(summary.errored, 0);
        assert!(
            db_get_one(&pool, 1, ymd(2026, 6, 3))
                .await
                .unwrap()
                .is_none()
        );

        // A range wholly inside the unpriced run is refused outright.
        let (status, bytes) = post_json(
            &app,
            "/closing_prices/backfill",
            serde_json::json!({ "listing_id": 1, "from": "2026-06-03", "to": "2026-06-04" }),
        )
        .await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
        assert!(
            String::from_utf8_lossy(&bytes).contains("unpriced from 2026-06-03"),
            "{}",
            String::from_utf8_lossy(&bytes)
        );
    }

    /// Migration 0037, the mirror: a listing marked `unpriced_before` is not
    /// fetched *earlier* than that date — the provider's series has not begun
    /// and every call would only store an errored row. The days from it on
    /// are still collected.
    #[tokio::test]
    async fn collection_skips_a_listing_before_its_unpriced_before_date() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "LAC", "XASX", "AUD").await;
        insert_buy(&pool, 1, 1, "100").await;
        let marked = crate::test_support::listing(1)
            .ticker("LAC")
            .name("LAC")
            .mic("XASX")
            .security_type(listing::SecurityType::Share)
            .unpriced_before(ymd(2026, 6, 4))
            .build();
        listing::db_upsert(&pool, &marked).await.unwrap();

        let mut stub = StubFetcher::default();
        for &d in &asx_lookback_window() {
            stub = stub.with_close(1, d, "24.90", "AUD");
        }
        run_collection(&pool, &stub, friday_evening_sydney())
            .await
            .unwrap();

        for &d in &asx_lookback_window() {
            let stored = db_get_one(&pool, 1, d).await.unwrap();
            if d < ymd(2026, 6, 4) {
                assert!(
                    stored.is_none(),
                    "nothing is fetched or stored before the series begins ({d})"
                );
            } else {
                assert!(stored.is_some(), "the days from it on are collected ({d})");
            }
        }
    }

    /// The explicit paths refuse the same days: a single fetch before the
    /// date is `422` naming the marker, and a backfill crossing it starts at
    /// the date instead of storing a run of errored rows.
    #[tokio::test]
    async fn api_fetch_and_backfill_start_at_unpriced_before() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "LAC", "XASX", "AUD").await;
        insert_buy(&pool, 1, 1, "100").await;
        let marked = crate::test_support::listing(1)
            .ticker("LAC")
            .name("LAC")
            .mic("XASX")
            .security_type(listing::SecurityType::Share)
            .unpriced_before(ymd(2026, 6, 4))
            .build();
        listing::db_upsert(&pool, &marked).await.unwrap();

        let mut stub = StubFetcher::default();
        for d in [
            ymd(2026, 6, 2),
            ymd(2026, 6, 3),
            ymd(2026, 6, 4),
            ymd(2026, 6, 5),
        ] {
            stub = stub.with_close(1, d, "24.90", "AUD");
        }
        let app = full_router(pool.clone(), stub);

        let (status, bytes) = post_json(
            &app,
            "/closing_prices/fetch",
            serde_json::json!({ "listing_id": 1, "price_date": "2026-06-03" }),
        )
        .await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
        let msg = String::from_utf8_lossy(&bytes);
        assert!(msg.contains("unpriced before 2026-06-04"), "{msg}");

        let (status, bytes) = post_json(
            &app,
            "/closing_prices/backfill",
            serde_json::json!({ "listing_id": 1, "from": "2026-06-02", "to": "2026-06-05" }),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let summary: BackfillSummary = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(summary.trading_days, 2, "4 and 5 June, not 2 or 3");
        assert_eq!(summary.fetched_ok, 2);
        assert_eq!(summary.errored, 0);
        assert!(
            db_get_one(&pool, 1, ymd(2026, 6, 3))
                .await
                .unwrap()
                .is_none()
        );

        // A range wholly before the date is refused outright.
        let (status, bytes) = post_json(
            &app,
            "/closing_prices/backfill",
            serde_json::json!({ "listing_id": 1, "from": "2026-06-01", "to": "2026-06-03" }),
        )
        .await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
        assert!(
            String::from_utf8_lossy(&bytes).contains("unpriced before 2026-06-04"),
            "{}",
            String::from_utf8_lossy(&bytes)
        );
    }

    /// Nor is a manual price deletable — it is an ok row, so the same rule
    /// that stops a fetched price being deleted applies, and the message
    /// points at the only way to change it.
    #[tokio::test]
    async fn api_delete_rejects_a_manual_price() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "BHP", "XASX", "AUD").await;
        seed_manual_price(&pool, 1, ymd(2026, 6, 4), "62.48").await;
        let app = full_router(pool.clone(), StubFetcher::default());

        let (status, bytes) = delete_req(&app, "/closing_prices/1/2026-06-04").await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
        let msg = String::from_utf8_lossy(&bytes);
        assert!(msg.contains("enter another manual price"), "{msg}");
        assert!(
            db_get_one(&pool, 1, ymd(2026, 6, 4))
                .await
                .unwrap()
                .is_some()
        );
    }

    /// Neither the scheduled run nor a backfill over the range clobbers a
    /// hand-entered price: both skip every date already stored ok, which a
    /// manual row is.
    #[tokio::test]
    async fn collection_and_backfill_leave_a_manual_price_alone() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "BHP", "XASX", "AUD").await;
        insert_buy(&pool, 1, 1, "100").await;
        seed_manual_price(&pool, 1, ymd(2026, 6, 4), "62.48").await;

        let week = asx_lookback_window();
        let mut stub = StubFetcher::default();
        for &d in &week {
            stub = stub.with_close(1, d, "99.99", "AUD");
        }
        run_collection(&pool, &stub, friday_evening_sydney())
            .await
            .unwrap();
        let row = db_get_one(&pool, 1, ymd(2026, 6, 4))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(row.price, Some("62.48".parse().unwrap()), "not re-fetched");
        assert_eq!(row.origin, PriceOrigin::Manual);
        // The other days of the window were collected normally.
        let other = db_get_one(&pool, 1, ymd(2026, 6, 5))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(other.origin, PriceOrigin::Fetched);

        let app = full_router(pool.clone(), StubFetcher::default());
        let (status, bytes) = post_json(
            &app,
            "/closing_prices/backfill",
            serde_json::json!({ "listing_id": 1, "from": "2026-06-01", "to": "2026-06-05" }),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{:?}", bytes);
        let row = db_get_one(&pool, 1, ymd(2026, 6, 4))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(row.price, Some("62.48".parse().unwrap()), "still manual");
        assert_eq!(row.origin, PriceOrigin::Manual);
    }

    /// Correcting a manual price keeps the superseded one: the upsert is an
    /// UPDATE, so the audit trail (0021) holds the old figure *and* the
    /// sourcing and reason given for it. Without that, re-entering a price
    /// would quietly destroy the record of why the first one was entered —
    /// which is what made auditing this table worth the surrogate key.
    #[tokio::test]
    async fn revising_a_manual_price_retains_the_superseded_provenance() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "BHP", "XASX", "AUD").await;
        let app = full_router(pool.clone(), StubFetcher::default());

        let first = serde_json::json!({
            "price": "62.48",
            "sourced_from": "asx.com.au closing report",
            "reason": "provider serves no candle since the delisting",
        });
        let (status, _) = put_json(&app, "/closing_prices/1/2026-06-04", first).await;
        assert_eq!(status, StatusCode::NO_CONTENT);
        let stored = db_get_one(&pool, 1, ymd(2026, 6, 4))
            .await
            .unwrap()
            .unwrap();

        let corrected = serde_json::json!({
            "price": "64.28",
            "sourced_from": "the registry's own statement",
            "reason": "the first entry transposed two digits",
        });
        let (status, _) = put_json(&app, "/closing_prices/1/2026-06-04", corrected).await;
        assert_eq!(status, StatusCode::NO_CONTENT);

        // The row keeps its identity across the correction — one audit trail,
        // not two rows.
        let now = db_get_one(&pool, 1, ymd(2026, 6, 4))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(now.id, stored.id, "the surrogate key survives an upsert");
        assert_eq!(now.price, Some("64.28".parse().unwrap()));

        let history = crate::reports::row_history::db_row_history(&pool, "closing_prices", now.id)
            .await
            .unwrap();
        assert_eq!(history.len(), 1, "one recorded prior version");
        let prior = &history[0];
        assert_eq!(prior["operation"], "UPDATE");
        assert_eq!(prior["price"], "62.48");
        assert_eq!(prior["sourced_from"], "asx.com.au closing report");
        assert_eq!(
            prior["reason"],
            "provider serves no candle since the delisting"
        );
    }

    /// Discarding an errored row is recorded too — the trail keeps the
    /// acknowledgement that a day was written off, and the message it carried.
    #[tokio::test]
    async fn discarding_an_errored_row_is_recorded_in_the_audit_trail() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "BHP", "XASX", "AUD").await;
        insert_buy(&pool, 1, 1, "100").await;
        store_errored(&pool, ymd(2026, 6, 2)).await;
        let row = db_get_one(&pool, 1, ymd(2026, 6, 2))
            .await
            .unwrap()
            .unwrap();
        let app = full_router(pool.clone(), StubFetcher::default());

        let (status, _) = delete_req(&app, "/closing_prices/1/2026-06-02").await;
        assert_eq!(status, StatusCode::NO_CONTENT);

        let history = crate::reports::row_history::db_row_history(&pool, "closing_prices", row.id)
            .await
            .unwrap();
        assert_eq!(history.len(), 1);
        assert_eq!(history[0]["operation"], "DELETE");
        assert_eq!(history[0]["status"], "error");
        assert!(
            history[0]["error"].as_str().is_some_and(|e| !e.is_empty()),
            "the failure the day was written off for is kept"
        );
    }

    /// The schema pairs a manual row's provenance with its origin, so no
    /// write path — not even raw SQL — can store a hand-entered price without
    /// its sourcing and reason, or hang them on a fetched row.
    #[tokio::test]
    async fn db_check_constraints_pair_manual_provenance_with_the_origin() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "BHP", "XASX", "AUD").await;
        let insert = |columns: &'static str, values: &'static str| {
            let pool = pool.clone();
            async move {
                sqlx::query(sqlx::AssertSqlSafe(format!(
                    "INSERT INTO closing_prices \
                         (listing_id, price_date, price, price_as_observed, fetched_at, status, \
                          error, {columns}) \
                     VALUES (1, '2026-06-05', '1.23', '1.23', 'now', 'ok', NULL, {values})"
                )))
                .execute(&pool)
                .await
            }
        };

        // manual without either provenance field
        assert!(
            insert("source, origin", "'manual', 'manual'")
                .await
                .is_err()
        );
        // manual with only one of them
        assert!(
            insert(
                "source, origin, sourced_from",
                "'manual', 'manual', 'asx.com.au'"
            )
            .await
            .is_err()
        );
        assert!(
            insert("source, origin, reason", "'manual', 'manual', 'no candle'")
                .await
                .is_err()
        );
        // a fetched row may not carry provenance meant for a manual one
        assert!(
            insert(
                "source, origin, sourced_from, reason",
                "'yahoo', 'fetched', 'asx.com.au', 'no candle'"
            )
            .await
            .is_err()
        );
        // the provider slot and the origin may not disagree, either way round
        assert!(
            insert(
                "source, origin, sourced_from, reason",
                "'yahoo', 'manual', 'asx.com.au', 'no candle'"
            )
            .await
            .is_err()
        );
        assert!(
            insert("source, origin", "'manual', 'fetched'")
                .await
                .is_err()
        );
        // an unknown origin is rejected by the enum CHECK
        assert!(
            insert(
                "source, origin, sourced_from, reason",
                "'manual', 'entered', 'asx.com.au', 'no candle'"
            )
            .await
            .is_err()
        );
        // …and the valid combination is accepted.
        assert!(
            insert(
                "source, origin, sourced_from, reason",
                "'manual', 'manual', 'asx.com.au', 'no candle'"
            )
            .await
            .is_ok()
        );
    }

    /// A manual row is always a price, never a recorded failure: there is no
    /// such thing as a hand-entered fetch error.
    #[tokio::test]
    async fn db_check_constraint_forbids_an_errored_manual_row() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "BHP", "XASX", "AUD").await;
        let bad = sqlx::query(
            "INSERT INTO closing_prices \
                 (listing_id, price_date, price, source, fetched_at, status, error, \
                  origin, sourced_from, reason) \
             VALUES (1, '2026-06-05', NULL, 'manual', 'now', 'error', 'oops', \
                     'manual', 'asx.com.au', 'no candle')",
        )
        .execute(&pool)
        .await;
        assert!(bad.is_err());
    }

    #[tokio::test]
    async fn api_delete_unknown_row_is_404() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "BHP", "XASX", "AUD").await;
        let app = full_router(pool.clone(), StubFetcher::default());

        let (status, _) = delete_req(&app, "/closing_prices/1/2026-06-02").await;
        assert_eq!(status, StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn api_backfill_fetches_only_missing_trading_days() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "BHP", "XASX", "AUD").await;
        insert_buy(&pool, 1, 1, "100").await;
        // Week of Mon 2026-06-01 .. Fri 2026-06-05; Wednesday already stored ok.
        let pre = StubFetcher::default().with_close(1, ymd(2026, 6, 3), "64.91", "AUD");
        let market = load_market(&pool, 1).await.unwrap().unwrap();
        fetch_and_store(&pool, &pre, &market, &[ymd(2026, 6, 3)])
            .await
            .unwrap();

        let fetcher = StubFetcher::default()
            .with_close(1, ymd(2026, 6, 1), "62.48", "AUD")
            .with_close(1, ymd(2026, 6, 2), "63.37", "AUD")
            .with_close(1, ymd(2026, 6, 4), "62.80", "AUD")
            .with_close(1, ymd(2026, 6, 5), "61.24", "AUD");
        let app = full_router(pool.clone(), fetcher);

        // Sat..Sat range: weekends are not trading days, Wednesday is skipped.
        let (status, bytes) = post_json(
            &app,
            "/closing_prices/backfill",
            serde_json::json!({ "listing_id": 1, "from": "2026-05-30", "to": "2026-06-06" }),
        )
        .await;
        assert_eq!(
            status,
            StatusCode::OK,
            "{}",
            String::from_utf8_lossy(&bytes)
        );
        let summary: BackfillSummary = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(summary.trading_days, 5);
        assert_eq!(summary.already_stored, 1);
        assert_eq!(summary.fetched_ok, 4);
        assert_eq!(summary.errored, 0);

        let rows = db_list(&pool, Some(1), None, None).await.unwrap();
        assert_eq!(rows.len(), 5);
        assert!(rows.iter().all(|r| r.status == PriceStatus::Ok));
        // Wednesday kept its original fetch (source "stub" both ways, but the
        // pre-stored price is unchanged).
        let wed = db_get_one(&pool, 1, ymd(2026, 6, 3))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(wed.price, Some("64.91".parse().unwrap()));
    }

    /// The backfill body's optional `symbol` reaches the fetcher as a one-off
    /// override — recovering a pre-rename date range under the old symbol
    /// without touching `listings.price_symbol`.
    #[tokio::test]
    async fn api_backfill_symbol_override_reaches_the_fetcher() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "LAR", "XNYS", "USD").await;
        insert_buy(&pool, 1, 1, "100").await;

        let fetcher = Arc::new(StubFetcher::default().with_close(1, ymd(2026, 6, 1), "10", "USD"));
        let shared: SharedFetcher = fetcher.clone();
        let app = ApiClient::over(router().with_state(pool.clone()).layer(Extension(shared)));

        let (status, bytes) = post_json(
            &app,
            "/closing_prices/backfill",
            serde_json::json!({
                "listing_id": 1, "from": "2026-06-01", "to": "2026-06-01",
                "symbol": "LAAC-OLD"
            }),
        )
        .await;
        assert_eq!(
            status,
            StatusCode::OK,
            "{}",
            String::from_utf8_lossy(&bytes)
        );
        assert_eq!(fetcher.symbols(), vec!["LAAC-OLD".to_string()]);
        // The listing's own stored symbol is untouched by the one-off override.
        assert_eq!(
            listing::db_get(&pool, 1)
                .await
                .unwrap()
                .unwrap()
                .price_symbol,
            None
        );
    }

    /// Every fetched row records the provider symbol it was fetched under —
    /// on an ordinary fetch too, not only an overridden one, so the stored
    /// answer to "what symbol produced this row?" is never a null that has to
    /// be interpreted (migration 0038).
    #[tokio::test]
    async fn db_a_fetched_row_records_the_symbol_it_was_fetched_under() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "BHP", "XASX", "AUD").await;
        let market = load_market(&pool, 1).await.unwrap().unwrap();
        let stub = StubFetcher::default().with_close(1, ymd(2026, 6, 4), "62.80", "AUD");

        fetch_and_store(&pool, &stub, &market, &[ymd(2026, 6, 4)])
            .await
            .unwrap();

        let row = db_get_one(&pool, 1, ymd(2026, 6, 4))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(row.fetched_symbol.as_deref(), Some("BHP.AX"));
    }

    /// A failed fetch records the symbol it was *attempted* under: the symbol
    /// is as much of the provenance of a failure as of a price, and a wrong
    /// one is the usual reason for the failure.
    #[tokio::test]
    async fn db_a_failed_fetch_records_the_symbol_it_was_attempted_under() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "LAAC", "XNYS", "USD").await;
        let market = load_market(&pool, 1).await.unwrap().unwrap();

        fetch_and_store(&pool, &StubFetcher::default(), &market, &[ymd(2026, 6, 4)])
            .await
            .unwrap();

        let row = db_get_one(&pool, 1, ymd(2026, 6, 4))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(row.status, PriceStatus::Error);
        assert_eq!(row.fetched_symbol.as_deref(), Some("LAAC"));
    }

    /// A range straddling a rename is fetched under one symbol per identity,
    /// and each stored row records *its own* segment's symbol — not one
    /// symbol for the lot.
    #[tokio::test]
    async fn db_each_row_records_its_own_segments_symbol_across_a_rename() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "LAAC", "XNYS", "USD").await;
        rename_listing(&pool, 1, ymd(2026, 6, 3), "LAR", None).await;

        let market = load_market(&pool, 1).await.unwrap().unwrap();
        let days = [ymd(2026, 6, 2), ymd(2026, 6, 4)];
        let mut stub = StubFetcher::default();
        for &d in &days {
            stub = stub.with_close(1, d, "2.77", "USD");
        }
        fetch_and_store(&pool, &stub, &market, &days).await.unwrap();

        let mut symbols = Vec::new();
        for &d in &days {
            symbols.push(
                db_get_one(&pool, 1, d)
                    .await
                    .unwrap()
                    .unwrap()
                    .fetched_symbol,
            );
        }
        assert_eq!(
            symbols,
            vec![Some("LAAC".to_string()), Some("LAR".to_string())]
        );
    }

    /// The incident this column exists for (TODO "LAC's whole pre-demerger
    /// price history is LAR's series"): a backfill run with the one-off
    /// `symbol` override stored 260 rows of another security's series under
    /// the listing's own id, and nothing recorded which symbol produced them.
    /// Now every such row names it on its face — and a later re-fetch under
    /// the ordinary symbol *replaces* the record rather than leaving the row
    /// asserting a symbol it no longer came from.
    #[tokio::test]
    async fn api_backfill_records_the_overriding_symbol_on_every_stored_row() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "LAC", "XNYS", "USD").await;
        insert_buy(&pool, 1, 1, "100").await;
        let days = [ymd(2026, 6, 1), ymd(2026, 6, 2)];
        let mut stub = StubFetcher::default();
        for &d in &days {
            stub = stub.with_close(1, d, "10", "USD");
        }
        let app = full_router(pool.clone(), stub);

        let (status, bytes) = post_json(
            &app,
            "/closing_prices/backfill",
            serde_json::json!({
                "listing_id": 1, "from": "2026-06-01", "to": "2026-06-02",
                "symbol": "LAAC"
            }),
        )
        .await;
        assert_eq!(
            status,
            StatusCode::OK,
            "{}",
            String::from_utf8_lossy(&bytes)
        );
        for &d in &days {
            let row = db_get_one(&pool, 1, d).await.unwrap().unwrap();
            assert_eq!(
                row.fetched_symbol.as_deref(),
                Some("LAAC"),
                "the row names the symbol that produced it, not the listing's own"
            );
            assert_ne!(row.fetched_symbol.as_deref(), Some("LAC"));
        }

        // Re-fetching without the override moves the record with the figure:
        // a row must never keep the symbol of a write it is no longer from.
        let (status, bytes) = post_json(
            &app,
            "/closing_prices/fetch",
            serde_json::json!({ "listing_id": 1, "price_date": "2026-06-01" }),
        )
        .await;
        assert_eq!(
            status,
            StatusCode::OK,
            "{}",
            String::from_utf8_lossy(&bytes)
        );
        let row = db_get_one(&pool, 1, days[0]).await.unwrap().unwrap();
        assert_eq!(row.fetched_symbol.as_deref(), Some("LAC"));
    }

    /// The recorded symbol is served by `GET /closing_prices` — the column is
    /// provenance for a person to read, so it has to reach the list the
    /// Closing Prices screen renders, not just the row.
    #[tokio::test]
    async fn api_list_serves_the_symbol_a_row_was_fetched_under() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "LAC", "XNYS", "USD").await;
        crate::test_support::closing_price(1, ymd(2026, 6, 1))
            .fetched_symbol("LAAC")
            .insert(&pool)
            .await;
        let app = full_router(pool.clone(), StubFetcher::default());

        let rows: Vec<serde_json::Value> = app.get_json("/closing_prices").await;
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0]["fetched_symbol"], "LAAC");
    }

    /// A hand-entered price is fetched under no symbol at all, so it records
    /// none — the column is CHECK-paired with the origin (0038), the way
    /// `sourced_from`/`reason` are paired the other way round.
    #[tokio::test]
    async fn api_a_manual_price_records_no_fetched_symbol() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "BHP", "XASX", "AUD").await;
        let app = full_router(pool.clone(), StubFetcher::default());

        let resp = app
            .put(
                "/closing_prices/1/2026-06-04",
                &serde_json::json!({
                    "price": "62.48",
                    "sourced_from": "asx.com.au closing report",
                    "reason": "provider serves no candle for the day"
                }),
            )
            .await;
        assert_eq!(resp.status, StatusCode::NO_CONTENT);

        let row = db_get_one(&pool, 1, ymd(2026, 6, 4))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(row.origin, PriceOrigin::Manual);
        assert_eq!(row.fetched_symbol, None);
    }

    /// The cheap cross-check on top of recording the symbol: whatever symbol
    /// the provider was asked for, the currency it answers in must be the
    /// listing's. A mismatch is an errored row for the day — the same
    /// treatment as any other provider failure, so the wrong figure is never
    /// stored and the reason is on the record — and the row still names the
    /// overriding symbol that produced it.
    #[tokio::test]
    async fn api_backfill_under_an_override_stores_a_currency_mismatch_as_an_error() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "LAC", "XNYS", "USD").await;
        insert_buy(&pool, 1, 1, "100").await;
        // The override reaches a security quoted in another currency — the
        // clearest evidence a symbol names a different security altogether.
        let stub = StubFetcher::default().with_close(1, ymd(2026, 6, 1), "10", "AUD");
        let app = full_router(pool.clone(), stub);

        let (status, bytes) = post_json(
            &app,
            "/closing_prices/backfill",
            serde_json::json!({
                "listing_id": 1, "from": "2026-06-01", "to": "2026-06-01",
                "symbol": "LAAC.AX"
            }),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let summary: BackfillSummary = serde_json::from_slice(&bytes).unwrap();
        assert_eq!((summary.fetched_ok, summary.errored), (0, 1));

        let row = db_get_one(&pool, 1, ymd(2026, 6, 1))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(row.status, PriceStatus::Error);
        assert_eq!(row.price, None, "no figure is stored from a foreign series");
        let msg = row.error.unwrap();
        assert!(msg.contains("currency mismatch"), "{msg}");
        assert!(msg.contains("AUD") && msg.contains("USD"), "{msg}");
        assert_eq!(row.fetched_symbol.as_deref(), Some("LAAC.AX"));
    }

    #[tokio::test]
    async fn api_backfill_records_missing_candles_as_errors() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "BHP", "XASX", "AUD").await;
        insert_buy(&pool, 1, 1, "100").await;
        // Provider has Thu+Fri but nothing for Mon-Wed (e.g. an unseeded
        // historical holiday or missing provider data) — those days must be
        // recorded as errored rows, never silently missing.
        let fetcher = StubFetcher::default()
            .with_close(1, ymd(2026, 6, 4), "62.80", "AUD")
            .with_close(1, ymd(2026, 6, 5), "61.24", "AUD");
        let app = full_router(pool.clone(), fetcher);

        let (status, bytes) = post_json(
            &app,
            "/closing_prices/backfill",
            serde_json::json!({ "listing_id": 1, "from": "2026-06-01", "to": "2026-06-05" }),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let summary: BackfillSummary = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(summary.fetched_ok, 2);
        assert_eq!(summary.errored, 3);

        let row = db_get_one(&pool, 1, ymd(2026, 6, 2))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(row.status, PriceStatus::Error);
        assert!(row.error.as_deref().unwrap().contains("no candle"));
    }

    /// A provider call that returns *zero* candles across the whole
    /// requested window (as opposed to a data gap on one date among others)
    /// is the classic wrong/renamed/delisted-symbol case — every date's
    /// errored row names the symbol and points at the fix, instead of the
    /// generic per-day message that's indistinguishable from a transient
    /// outage.
    #[tokio::test]
    async fn fetch_and_store_names_the_symbol_when_the_whole_window_returns_no_candles() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "LAAC", "XNYS", "USD").await;
        insert_buy(&pool, 1, 1, "100").await;
        let market = load_market(&pool, 1).await.unwrap().unwrap();

        // Fetcher has no data for this listing at all — Ok(vec![]).
        let empty = StubFetcher::default();
        let dates = [ymd(2026, 6, 1), ymd(2026, 6, 2)];
        let (ok, errored) = fetch_and_store(&pool, &empty, &market, &dates)
            .await
            .unwrap();
        assert_eq!(ok, 0);
        assert_eq!(errored, 2);

        for date in dates {
            let row = db_get_one(&pool, 1, date).await.unwrap().unwrap();
            assert_eq!(row.status, PriceStatus::Error);
            let msg = row.error.unwrap();
            assert!(msg.contains("LAAC"), "names the symbol: {msg}");
            assert!(msg.contains("renamed"), "points at the cause: {msg}");
            assert!(msg.contains("price_symbol"), "points at the fix: {msg}");
        }
    }

    #[tokio::test]
    async fn api_backfill_unknown_listing_404_and_bad_range_422() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "BHP", "XASX", "AUD").await;
        let app = full_router(pool, StubFetcher::default());

        let (status, _) = post_json(
            &app,
            "/closing_prices/backfill",
            serde_json::json!({ "listing_id": 99, "from": "2026-06-01", "to": "2026-06-05" }),
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND);

        let (status, _) = post_json(
            &app,
            "/closing_prices/backfill",
            serde_json::json!({ "listing_id": 1, "from": "2026-06-05", "to": "2026-06-01" }),
        )
        .await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    }

    // --- manual re-fetch ---

    #[tokio::test]
    async fn api_fetch_replaces_errored_row_and_returns_it() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "BHP", "XASX", "AUD").await;
        insert_buy(&pool, 1, 1, "100").await;
        run_collection(
            &pool,
            &StubFetcher::failing("down"),
            friday_evening_sydney(),
        )
        .await
        .unwrap_err();

        let fetcher = StubFetcher::default().with_close(1, ymd(2026, 6, 5), "62.48", "AUD");
        let app = full_router(pool.clone(), fetcher);
        let (status, bytes) = post_json(
            &app,
            "/closing_prices/fetch",
            serde_json::json!({ "listing_id": 1, "price_date": "2026-06-05" }),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let row: ClosingPrice = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(row.status, PriceStatus::Ok);
        assert_eq!(row.price, Some("62.48".parse().unwrap()));

        let stored = db_get_one(&pool, 1, ymd(2026, 6, 5))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            stored.status,
            PriceStatus::Ok,
            "the errored row was replaced"
        );
    }

    #[tokio::test]
    async fn api_fetch_rejects_incomplete_and_non_trading_days() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "BHP", "XASX", "AUD").await;
        let app = full_router(pool, StubFetcher::default());

        // Far future: the close cannot be final.
        let (status, bytes) = post_json(
            &app,
            "/closing_prices/fetch",
            serde_json::json!({ "listing_id": 1, "price_date": "2099-01-04" }),
        )
        .await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
        assert!(String::from_utf8_lossy(&bytes).contains("not final"));

        // A Saturday well in the past: not a trading day.
        let (status, bytes) = post_json(
            &app,
            "/closing_prices/fetch",
            serde_json::json!({ "listing_id": 1, "price_date": "2024-01-06" }),
        )
        .await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
        assert!(String::from_utf8_lossy(&bytes).contains("not a trading day"));

        // Unknown listing.
        let (status, _) = post_json(
            &app,
            "/closing_prices/fetch",
            serde_json::json!({ "listing_id": 99, "price_date": "2024-01-05" }),
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND);
    }

    // --- list endpoint ---

    #[tokio::test]
    async fn api_list_filters_by_listing_and_date_range_including_errors() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "BHP", "XASX", "AUD").await;
        insert_listing(&pool, 2, "ICE", "XNYS", "USD").await;
        let market1 = load_market(&pool, 1).await.unwrap().unwrap();
        let market2 = load_market(&pool, 2).await.unwrap().unwrap();
        let ok = StubFetcher::default()
            .with_close(1, ymd(2026, 6, 4), "62.80", "AUD")
            .with_close(1, ymd(2026, 6, 5), "61.24", "AUD");
        fetch_and_store(&pool, &ok, &market1, &[ymd(2026, 6, 4), ymd(2026, 6, 5)])
            .await
            .unwrap();
        fetch_and_store(
            &pool,
            &StubFetcher::failing("down"),
            &market2,
            &[ymd(2026, 6, 5)],
        )
        .await
        .unwrap();

        let app = full_router(pool, StubFetcher::default());
        let get = |uri: &str| {
            let app = app.clone();
            let uri = uri.to_string();
            async move {
                let resp = app.get(uri).await;
                assert_eq!(resp.status, StatusCode::OK);
                let bytes = resp.body.clone();
                serde_json::from_slice::<Vec<ClosingPrice>>(&bytes).unwrap()
            }
        };

        assert_eq!(
            get("/closing_prices").await.len(),
            3,
            "errored rows are listed too"
        );
        assert_eq!(get("/closing_prices?listing_id=1").await.len(), 2);
        let one_day = get("/closing_prices?from=2026-06-05&to=2026-06-05").await;
        assert_eq!(one_day.len(), 2);
        assert!(one_day.iter().any(|r| r.status == PriceStatus::Error));
    }

    // --- live quote / valuation ---

    #[tokio::test]
    async fn live_aud_prices_converts_quote_currency_and_carries_as_of() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "BHP", "XASX", "AUD").await;
        insert_listing(&pool, 2, "ICE", "XNYS", "USD").await;
        // 2 USD per AUD for June 2026 → US$141.50 = A$70.75.
        sqlx::query(
            "INSERT INTO rba_fx_rates (currency, month, rate) VALUES ('USD', '2026-06', '2')",
        )
        .execute(&pool)
        .await
        .unwrap();
        let as_of = utc(2026, 6, 5, 6, 30);
        let fetcher = StubFetcher::default()
            .with_quote(1, "62.48", "AUD", as_of)
            .with_quote(2, "141.50", "USD", as_of);

        let prices = fetch_live_aud_prices(&pool, &fetcher, &[1, 2])
            .await
            .unwrap();
        let bhp = prices[&1].as_ref().unwrap();
        assert_eq!(bhp.aud_price, "62.48".parse::<Decimal>().unwrap());
        assert_eq!(bhp.as_of, as_of.to_rfc3339());
        let ice = prices[&2].as_ref().unwrap();
        assert_eq!(ice.aud_price, "70.75".parse::<Decimal>().unwrap());
    }

    #[tokio::test]
    async fn live_aud_prices_surface_failures_instead_of_zeroing() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "BHP", "XASX", "AUD").await; // provider down
        insert_listing(&pool, 2, "ICE", "XNYS", "USD").await; // currency mismatch
        insert_listing(&pool, 3, "VAS", "XASX", "USD").await; // no ATO rate for the quote month

        // Listing 1: blanket failure.
        let down = fetch_live_aud_prices(&pool, &StubFetcher::failing("provider down"), &[1])
            .await
            .unwrap();
        assert!(down[&1].as_ref().unwrap_err().contains("provider down"));

        let as_of = utc(2026, 6, 5, 6, 30);
        // Listing 2: provider quotes AUD for a USD listing.
        let mismatch = StubFetcher::default().with_quote(2, "141.50", "AUD", as_of);
        let m = fetch_live_aud_prices(&pool, &mismatch, &[2]).await.unwrap();
        assert!(m[&2].as_ref().unwrap_err().contains("currency mismatch"));

        // Listing 3: USD quote but no ATO rate imported for the quote month.
        let unconvertible = StubFetcher::default().with_quote(3, "10.00", "USD", as_of);
        let u = fetch_live_aud_prices(&pool, &unconvertible, &[3])
            .await
            .unwrap();
        assert!(u[&3].as_ref().unwrap_err().contains("no ATO FX rate"));
    }

    #[tokio::test]
    async fn resolve_live_prices_skips_overridden_and_respects_the_flag() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "BHP", "XASX", "AUD").await;
        insert_listing(&pool, 2, "WBC", "XASX", "AUD").await;
        let as_of = utc(2026, 6, 5, 6, 30);
        let fetcher = StubFetcher::default()
            .with_quote(1, "62.48", "AUD", as_of)
            .with_quote(2, "30", "AUD", as_of);

        // live = false → nothing fetched.
        let off = resolve_live_prices(&pool, Some(&fetcher), false, &HashMap::new(), [1, 2])
            .await
            .unwrap();
        assert!(off.is_empty());

        // live = true, listing 1 overridden → only listing 2 is fetched.
        let overrides = HashMap::from([(1i64, "99".parse::<Decimal>().unwrap())]);
        let on = resolve_live_prices(&pool, Some(&fetcher), true, &overrides, [1, 2])
            .await
            .unwrap();
        assert!(!on.contains_key(&1), "overridden listing is never fetched");
        assert_eq!(on[&2].as_ref().unwrap().aud_price, Decimal::from(30));

        // live = true with no fetcher → each listing marked unavailable.
        let none = resolve_live_prices(&pool, None, true, &HashMap::new(), [1])
            .await
            .unwrap();
        assert!(none[&1].as_ref().unwrap_err().contains("unavailable"));
    }

    // --- schema invariants ---

    #[tokio::test]
    async fn db_check_constraints_tie_price_and_error_to_status() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "BHP", "XASX", "AUD").await;
        // ok with no price violates the CHECK.
        let bad = sqlx::query(
            "INSERT INTO closing_prices (listing_id, price_date, price, source, fetched_at, status, error) \
             VALUES (1, '2026-06-05', NULL, 'stub', 'now', 'ok', NULL)",
        )
        .execute(&pool)
        .await;
        assert!(bad.is_err());
        // error with a price (and no error text) violates both CHECKs.
        let bad = sqlx::query(
            "INSERT INTO closing_prices (listing_id, price_date, price, source, fetched_at, status, error) \
             VALUES (1, '2026-06-05', '1.23', 'stub', 'now', 'error', NULL)",
        )
        .execute(&pool)
        .await;
        assert!(bad.is_err());
        // an unknown status is rejected by the enum CHECK.
        let bad = sqlx::query(
            "INSERT INTO closing_prices (listing_id, price_date, price, source, fetched_at, status, error) \
             VALUES (1, '2026-06-05', '1.23', 'stub', 'now', 'pending', NULL)",
        )
        .execute(&pool)
        .await;
        assert!(bad.is_err());
        // duplicate (listing, date) is rejected by the primary key.
        let market = load_market(&pool, 1).await.unwrap().unwrap();
        let ok = StubFetcher::default().with_close(1, ymd(2026, 6, 5), "62.48", "AUD");
        fetch_and_store(&pool, &ok, &market, &[ymd(2026, 6, 5)])
            .await
            .unwrap();
        let dup = sqlx::query(
            "INSERT INTO closing_prices (listing_id, price_date, price, source, fetched_at, status, error) \
             VALUES (1, '2026-06-05', '1.23', 'stub', 'now', 'ok', NULL)",
        )
        .execute(&pool)
        .await;
        assert!(dup.is_err());
    }

    // --- yahoo symbol mapping ---

    #[test]
    fn yahoo_symbols_cover_asx_us_and_crypto() {
        let mk = |mic: Option<&str>, ticker: &str, ccy: &str| {
            let b = crate::test_support::listing(1)
                .ticker(ticker)
                .name(ticker)
                .currency(ccy);
            let listing = match mic {
                Some(m) => b.mic(m).security_type(listing::SecurityType::Share),
                None => b.crypto(),
            }
            .build();
            Market::unrenamed(listing, None, HashSet::new())
        };
        let d = ymd(2024, 6, 3);
        assert_eq!(
            yahoo_symbol(&mk(Some("XASX"), "BHP", "AUD"), d).unwrap(),
            "BHP.AX"
        );
        assert_eq!(
            yahoo_symbol(&mk(Some("XNYS"), "ICE", "USD"), d).unwrap(),
            "ICE"
        );
        assert_eq!(yahoo_symbol(&mk(None, "BTC", "AUD"), d).unwrap(), "BTC-AUD");
        assert!(
            yahoo_symbol(&mk(Some("XLON"), "BARC", "GBP"), d)
                .unwrap_err()
                .contains("XLON")
        );
    }

    /// `listings.price_symbol` overrides the derived mapping (a symbol the
    /// provider spells differently, or an exchange with no mapping at all).
    #[test]
    fn yahoo_symbol_prefers_the_listings_stored_price_symbol_override() {
        let mut market = Market::unrenamed(
            crate::test_support::listing(1)
                .mic("XLON")
                .security_type(listing::SecurityType::Share)
                .ticker("BARC")
                .build(),
            None,
            HashSet::new(),
        );
        let d = ymd(2024, 6, 3);
        // XLON has no derived mapping, so without an override it errors...
        assert!(yahoo_symbol(&market, d).is_err());
        // ...but a stored price_symbol resolves it.
        market.listing.price_symbol = Some("BARC.L".to_string());
        assert_eq!(yahoo_symbol(&market, d).unwrap(), "BARC.L");
    }

    /// A one-off `symbol_override` (backfill's `symbol` param) wins over even
    /// a stored `price_symbol` — it's for a single deliberate fetch, e.g.
    /// recovering pre-rename history under the old symbol.
    #[test]
    fn yahoo_symbol_override_wins_over_the_stored_price_symbol() {
        let mut market = Market::unrenamed(
            crate::test_support::listing(1)
                .mic("XNYS")
                .security_type(listing::SecurityType::Share)
                .ticker("LAR")
                .price_symbol("LAR-CURRENT")
                .build(),
            None,
            HashSet::new(),
        );
        market.symbol_override = Some("LAAC-OLD".to_string());
        assert_eq!(yahoo_symbol(&market, ymd(2024, 6, 3)).unwrap(), "LAAC-OLD");
    }

    // --- as-at identity: the symbol and the calendar follow the date ---

    /// The prompting case (LAAC → LAR): a fetch of a date *before* the rename
    /// asks the provider for the symbol the security was actually quoted
    /// under then, with no `symbol` override supplied by the caller.
    #[tokio::test]
    async fn db_yahoo_symbol_resolves_as_at_the_date_across_a_rename() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "LAAC", "XNYS", "USD").await;
        rename_listing(&pool, 1, ymd(2025, 1, 27), "LAR", None).await;

        let market = load_market(&pool, 1).await.unwrap().unwrap();
        assert_eq!(yahoo_symbol(&market, ymd(2025, 1, 26)).unwrap(), "LAAC");
        // The effective date itself is already the new identity.
        assert_eq!(yahoo_symbol(&market, ymd(2025, 1, 27)).unwrap(), "LAR");
        assert_eq!(yahoo_symbol(&market, ymd(2025, 6, 1)).unwrap(), "LAR");
        // A live quote is always a question about today.
        assert_eq!(yahoo_symbol_now(&market).unwrap(), "LAR");
    }

    /// An exchange change moves the derived suffix too: the same security is
    /// `OLD.AX` before it moved and plain `NEW` after.
    #[tokio::test]
    async fn db_yahoo_symbol_follows_the_exchange_in_force_on_the_date() {
        let pool = test_pool().await;
        // Quoted in USD from the start, so the move to the NYSE crosses no
        // currency boundary (a rename that did is refused — SCENARIOS R-01);
        // the symbol the *date* resolves to is what this test is about.
        insert_listing(&pool, 1, "OLD", "XASX", "USD").await;
        rename_listing(&pool, 1, ymd(2025, 3, 10), "NEW", Some("XNYS")).await;

        let market = load_market(&pool, 1).await.unwrap().unwrap();
        assert_eq!(yahoo_symbol(&market, ymd(2025, 3, 7)).unwrap(), "OLD.AX");
        assert_eq!(yahoo_symbol(&market, ymd(2025, 3, 10)).unwrap(), "NEW");
    }

    /// `listings.price_symbol` is the *current* provider spelling, so it must
    /// not be applied to a pre-rename date — an override that matched the new
    /// ticker would otherwise silently re-label the old identity's history.
    #[tokio::test]
    async fn db_price_symbol_applies_to_the_current_identity_only() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "LAAC", "XNYS", "USD").await;
        rename_listing(&pool, 1, ymd(2025, 1, 27), "LAR", None).await;
        sqlx::query("UPDATE listings SET price_symbol = 'LAR-CURRENT' WHERE id = 1")
            .execute(&pool)
            .await
            .unwrap();

        let market = load_market(&pool, 1).await.unwrap().unwrap();
        assert_eq!(yahoo_symbol(&market, ymd(2025, 1, 26)).unwrap(), "LAAC");
        assert_eq!(
            yahoo_symbol(&market, ymd(2025, 2, 3)).unwrap(),
            "LAR-CURRENT"
        );
    }

    /// A trading-day question about a pre-rename date is answered by the
    /// exchange that was actually open then. 2025-01-27 is Australia Day
    /// (an ASX holiday, seeded) and an ordinary NYSE trading day, so the two
    /// calendars disagree on exactly that date.
    #[tokio::test]
    async fn db_trading_days_follow_the_exchange_calendar_in_force_then() {
        let pool = test_pool().await;
        // USD from the start, for the reason
        // `db_yahoo_symbol_follows_the_exchange_in_force_on_the_date` gives:
        // the calendars are what this test is about, not the currency.
        insert_listing(&pool, 1, "OLD", "XASX", "USD").await;
        exchange_holiday::db_upsert(
            &pool,
            &exchange_holiday::ExchangeHoliday {
                id: 0,
                mic: "XASX".to_string(),
                holiday_date: ymd(2025, 1, 27),
                name: "Australia Day".to_string(),
            },
        )
        .await
        .unwrap();
        rename_listing(&pool, 1, ymd(2025, 6, 2), "NEW", Some("XNYS")).await;

        let market = load_market(&pool, 1).await.unwrap().unwrap();
        // Before the move the ASX calendar applies, so the holiday is closed
        // and valuation falls back to the previous trading day.
        assert_eq!(
            market.latest_trading_day_on_or_before(ymd(2025, 1, 27)),
            Some(ymd(2025, 1, 24))
        );
        // After the move, NYSE's calendar — which has no such holiday — is
        // what a date is tested against.
        assert_eq!(
            market.latest_trading_day_on_or_before(ymd(2025, 6, 3)),
            Some(ymd(2025, 6, 3))
        );
    }

    /// A fetch range straddling a rename is one call per identity, each under
    /// the symbol quoted over its own span — never one call for the lot under
    /// today's symbol.
    #[tokio::test]
    async fn db_fetch_straddling_a_rename_calls_the_provider_once_per_identity() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "LAAC", "XNYS", "USD").await;
        rename_listing(&pool, 1, ymd(2026, 6, 3), "LAR", None).await;

        let market = load_market(&pool, 1).await.unwrap().unwrap();
        let days = [
            ymd(2026, 6, 1),
            ymd(2026, 6, 2),
            ymd(2026, 6, 3),
            ymd(2026, 6, 4),
        ];
        let mut stub = StubFetcher::default();
        for &d in &days {
            stub = stub.with_close(1, d, "2.77", "USD");
        }
        let (ok, errored) = fetch_and_store(&pool, &stub, &market, &days).await.unwrap();
        assert_eq!((ok, errored), (4, 0));

        assert_eq!(
            stub.calls(),
            vec![
                (1, ymd(2026, 6, 1), ymd(2026, 6, 2)),
                (1, ymd(2026, 6, 3), ymd(2026, 6, 4)),
            ],
            "the range splits at the effective date"
        );
        assert_eq!(stub.symbols(), vec!["LAAC".to_string(), "LAR".to_string()]);
    }

    /// A wholly pre-rename backfill is self-healing: the operator supplies no
    /// `symbol`, and the old one is read off the rename chain.
    #[tokio::test]
    async fn api_backfill_before_a_rename_uses_the_old_symbol_without_an_override() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "LAAC", "XNYS", "USD").await;
        rename_listing(&pool, 1, ymd(2026, 6, 3), "LAR", None).await;

        let stub = Arc::new(StubFetcher::default().with_close(1, ymd(2026, 6, 1), "2.77", "USD"));
        let shared: SharedFetcher = stub.clone();
        let app = ApiClient::over(router().with_state(pool.clone()).layer(Extension(shared)));
        let (status, bytes) = post_json(
            &app,
            "/closing_prices/backfill",
            serde_json::json!({ "listing_id": 1, "from": "2026-06-01", "to": "2026-06-01" }),
        )
        .await;
        assert_eq!(
            status,
            StatusCode::OK,
            "{}",
            String::from_utf8_lossy(&bytes)
        );

        assert_eq!(stub.symbols(), vec!["LAAC".to_string()]);
        let row = db_get_one(&pool, 1, ymd(2026, 6, 1))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(row.status, PriceStatus::Ok);
        assert_eq!(row.price, Some("2.77".parse().unwrap()));
    }

    /// The zero-candle message is judged per segment, so it names the symbol
    /// that actually came back empty rather than today's — and the segment
    /// that *did* return candles still stores its ok rows.
    #[tokio::test]
    async fn db_a_dead_segment_errors_alone_and_names_its_own_symbol() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "LAAC", "XNYS", "USD").await;
        rename_listing(&pool, 1, ymd(2026, 6, 3), "LAR", None).await;

        let market = load_market(&pool, 1).await.unwrap().unwrap();
        // Only the post-rename day has a candle; the old symbol serves none.
        let stub = StubFetcher::default().with_close(1, ymd(2026, 6, 3), "2.77", "USD");
        let days = [ymd(2026, 6, 2), ymd(2026, 6, 3)];
        let (ok, errored) = fetch_and_store(&pool, &stub, &market, &days).await.unwrap();
        assert_eq!((ok, errored), (1, 1));

        let dead = db_get_one(&pool, 1, ymd(2026, 6, 2))
            .await
            .unwrap()
            .unwrap();
        let msg = dead.error.unwrap();
        assert!(
            msg.contains("LAAC"),
            "names the dead segment's symbol: {msg}"
        );
        assert!(!msg.contains("LAR"), "not the current symbol: {msg}");
        let good = db_get_one(&pool, 1, ymd(2026, 6, 3))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(good.status, PriceStatus::Ok);
    }

    // --- collection's held-set and window ---

    /// A listing sold part-way through the lookback window is still collected
    /// for the days it was held: `reports::valuation` values a snapshot date
    /// against the listings held *on that date*, so dropping it the moment
    /// the Sell lands leaves those dates permanently blocked.
    #[tokio::test]
    async fn collection_covers_a_listing_sold_inside_the_lookback_window() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "BHP", "XASX", "AUD").await;
        crate::test_support::buy(1, 1)
            .date(ymd(2024, 1, 15))
            .qty(Decimal::from(100))
            .price(Decimal::from(10))
            .insert(&pool)
            .await;
        sell_everything(&pool, 2, 1, 1, "100").await;
        sqlx::query("UPDATE trades SET date = '2026-06-03' WHERE id = 2")
            .execute(&pool)
            .await
            .unwrap();

        // Nothing is held now, but the listing was held for most of the window.
        assert!(db_held_listing_ids(&pool, None).await.unwrap().is_empty());

        let mut stub = StubFetcher::default();
        for &d in &asx_lookback_window() {
            stub = stub.with_close(1, d, "62.48", "AUD");
        }
        run_collection(&pool, &stub, friday_evening_sydney())
            .await
            .unwrap();

        let stored = db_list(&pool, Some(1), None, None).await.unwrap();
        assert!(
            !stored.is_empty(),
            "the sold listing is still collected for the window"
        );
        assert!(
            stored.iter().any(|r| r.price_date == ymd(2026, 6, 2)),
            "including the days before the sale"
        );
    }

    /// The collection window must reach at least as far back as the snapshot
    /// catch-up window: a date the snapshot job retries but collection no
    /// longer refills can never unblock itself.
    #[test]
    fn collection_window_covers_the_snapshot_catchup_window() {
        // Read through a runtime binding so this stays a real assertion if the
        // two constants are ever decoupled again.
        let catchup: i64 = crate::reports::snapshot::CATCHUP_LOOKBACK_DAYS;
        let collection: i64 = COLLECTION_LOOKBACK_DAYS;
        assert!(
            catchup <= collection,
            "snapshot catch-up ({catchup}) reaches further back than collection ({collection})"
        );
    }

    // --- the held-set agrees with the holdings reports across a split ---

    /// `db_held_listing_ids` and `reports::portfolio::db_holdings_on` must
    /// agree about whether a listing is held: the price map is keyed off the
    /// former and the snapshot rows off the latter, so a disagreement stores
    /// a silently unvalued holding (or blocks a date on a security already
    /// fully sold). A split between the Buy and the Sell is what used to
    /// separate them — the allocation is in sale-date units, the parcel in
    /// as-acquired ones.
    async fn held_sets_agree(pool: &SqlitePool, as_of: NaiveDate) {
        let ids = db_held_listing_ids(pool, Some(as_of)).await.unwrap();
        let mut conn = pool.acquire().await.unwrap();
        let holdings = crate::reports::portfolio::db_holdings_on(&mut conn, Some(as_of))
            .await
            .unwrap();
        let mut from_report: Vec<i64> = holdings.iter().map(|h| h.listing_id).collect();
        from_report.sort();
        from_report.dedup();
        assert_eq!(ids, from_report, "as at {as_of}");
    }

    #[tokio::test]
    async fn db_held_listings_match_the_holdings_report_across_a_split() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "BHP", "XASX", "AUD").await;
        // Buy 100, 2:1 split to 200 units, sell 150 of them.
        crate::test_support::buy(1, 1)
            .date(ymd(2024, 1, 15))
            .qty(Decimal::from(100))
            .price(Decimal::from(10))
            .insert(&pool)
            .await;
        insert_share_split(&pool, 1, ymd(2024, 3, 1), "2", "1").await;
        crate::test_support::sell(2, 1)
            .date(ymd(2024, 6, 1))
            .qty(Decimal::from(150))
            .price(Decimal::from(8))
            .insert(&pool)
            .await;
        crate::test_support::allocate(&pool, 2, 2, 1, Decimal::from(150)).await;

        // 50 of the 200 post-split units remain, so the listing is still held
        // — the raw subtraction (100 − 150) used to make it look fully sold.
        held_sets_agree(&pool, ymd(2024, 7, 1)).await;
        assert_eq!(
            db_held_listing_ids(&pool, Some(ymd(2024, 7, 1)))
                .await
                .unwrap(),
            vec![1]
        );
    }

    #[tokio::test]
    async fn db_held_listings_match_the_holdings_report_across_a_consolidation() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "BHP", "XASX", "AUD").await;
        // Buy 1000, 1:10 consolidation to 100 units, sell all 100.
        crate::test_support::buy(1, 1)
            .date(ymd(2024, 1, 15))
            .qty(Decimal::from(1000))
            .price(Decimal::from(1))
            .insert(&pool)
            .await;
        insert_share_split(&pool, 1, ymd(2024, 3, 1), "1", "10").await;
        crate::test_support::sell(2, 1)
            .date(ymd(2024, 6, 1))
            .qty(Decimal::from(100))
            .price(Decimal::from(12))
            .insert(&pool)
            .await;
        crate::test_support::allocate(&pool, 2, 2, 1, Decimal::from(100)).await;

        // Fully sold — the raw subtraction (1000 − 100) used to leave 900
        // phantom units, blocking every later snapshot on a missing price.
        held_sets_agree(&pool, ymd(2024, 7, 1)).await;
        assert!(
            db_held_listing_ids(&pool, Some(ymd(2024, 7, 1)))
                .await
                .unwrap()
                .is_empty()
        );
    }
    // -----------------------------------------------------------------------
    // The contemporaneous basis (SCENARIOS Q-14)
    //
    // A stored price is the price the security traded at on its own date. The
    // provider restates its whole close series into the *current* basis the
    // moment a security splits, so the figure has to be restated back out of
    // whichever basis it arrived in — which is the basis in force when it was
    // observed, i.e. at `fetched_at`. These pin both halves: normalising on
    // the way in, and re-deriving stored rows when the action set changes.
    // -----------------------------------------------------------------------

    /// The stored price for a listing, and the figure it was observed as.
    async fn stored(pool: &SqlitePool, date: NaiveDate) -> (String, String) {
        let row = db_get_one(pool, 1, date).await.unwrap().unwrap();
        (
            row.price.unwrap().normalize().to_string(),
            row.price_as_observed.unwrap().normalize().to_string(),
        )
    }

    /// A pre-split day fetched *after* the split is recorded arrives in the
    /// post-split basis (Yahoo answers 120.888 for a day NVDA closed at
    /// 1208.88) and is stored in the day's own basis, with the provider's
    /// figure kept beside it.
    #[tokio::test]
    async fn db_a_price_fetched_after_a_split_is_stored_in_its_own_days_basis() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "BHP", "XASX", "AUD").await;
        insert_share_split(&pool, 1, ymd(2026, 6, 10), "10", "1").await;

        let market = load_market(&pool, 1).await.unwrap().unwrap();
        let stub = StubFetcher::default().with_close(1, ymd(2026, 6, 5), "120.888", "AUD");
        fetch_and_store(&pool, &stub, &market, &[ymd(2026, 6, 5)])
            .await
            .unwrap();

        assert_eq!(
            stored(&pool, ymd(2026, 6, 5)).await,
            ("1208.88".to_string(), "120.888".to_string()),
            "the provider's post-split figure is restated into the price date's own basis"
        );
    }

    /// The other half: a day collected *before* the split happened already
    /// holds the contemporaneous close, and recording the split later must
    /// leave it exactly as it is. This is the case the whole daily-collected
    /// history sits in, so a blanket "multiply every earlier price by the
    /// ratio" rule would corrupt years of correct prices at a stroke.
    #[tokio::test]
    async fn db_a_price_observed_before_the_split_is_untouched_when_it_is_recorded() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "BHP", "XASX", "AUD").await;
        crate::test_support::closing_price(1, ymd(2026, 6, 5))
            .price("1208.88")
            .fetched_at("2026-06-05T08:00:00Z")
            .insert(&pool)
            .await;

        insert_share_split(&pool, 1, ymd(2026, 6, 10), "10", "1").await;

        assert_eq!(
            stored(&pool, ymd(2026, 6, 5)).await,
            ("1208.88".to_string(), "1208.88".to_string()),
            "the fetch predates the split, so the figure was never restated"
        );
    }

    /// The property the whole design exists for: whichever order the split and
    /// the fetch are entered in, the stored price is the same.
    #[tokio::test]
    async fn db_entry_order_of_the_split_and_the_fetch_does_not_change_the_price() {
        async fn fetch(pool: &SqlitePool) {
            let market = load_market(pool, 1).await.unwrap().unwrap();
            let stub = StubFetcher::default().with_close(1, ymd(2026, 6, 5), "120.888", "AUD");
            fetch_and_store(pool, &stub, &market, &[ymd(2026, 6, 5)])
                .await
                .unwrap();
        }

        // split first, then the fetch
        let a = test_pool().await;
        insert_listing(&a, 1, "BHP", "XASX", "AUD").await;
        insert_share_split(&a, 1, ymd(2026, 6, 10), "10", "1").await;
        fetch(&a).await;

        // the fetch first, then the split
        let b = test_pool().await;
        insert_listing(&b, 1, "BHP", "XASX", "AUD").await;
        fetch(&b).await;
        assert_eq!(
            stored(&b, ymd(2026, 6, 5)).await.0,
            "120.888",
            "with no split recorded there is nothing to restate out of"
        );
        insert_share_split(&b, 1, ymd(2026, 6, 10), "10", "1").await;

        assert_eq!(
            stored(&a, ymd(2026, 6, 5)).await,
            ("1208.88".to_string(), "120.888".to_string())
        );
        assert_eq!(
            stored(&b, ymd(2026, 6, 5)).await,
            stored(&a, ymd(2026, 6, 5)).await,
            "entry order cannot matter"
        );
    }

    /// A bonus issue re-bases units exactly as a split does (one new share for
    /// each held doubles the count), so it halves the per-unit price the same
    /// way — and the provider restates for it too.
    #[tokio::test]
    async fn db_a_bonus_issue_rebases_stored_prices_like_a_split() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "BHP", "XASX", "AUD").await;
        let market = load_market(&pool, 1).await.unwrap().unwrap();
        let stub = StubFetcher::default().with_close(1, ymd(2026, 6, 5), "30", "AUD");
        fetch_and_store(&pool, &stub, &market, &[ymd(2026, 6, 5)])
            .await
            .unwrap();

        crate::entities::corporate_action::db_upsert(
            &pool,
            &crate::entities::corporate_action::CorporateAction {
                id: 950,
                listing_id: 1,
                date: ymd(2026, 6, 10),
                kind: crate::entities::corporate_action::ActionKind::BonusIssue {
                    bonus_units: Decimal::ONE,
                    bonus_held_units: Decimal::ONE,
                },
            },
        )
        .await
        .unwrap();

        assert_eq!(
            stored(&pool, ymd(2026, 6, 5)).await.0,
            "60",
            "one bonus share per share held doubles the unit count, so the earlier day's own \
             price is twice the restated one"
        );
    }

    /// A consolidation (reverse split) runs the error the other way: the
    /// provider's restated figure is *larger* than the contemporaneous one.
    #[tokio::test]
    async fn db_a_consolidation_rebases_stored_prices_the_other_way() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "BHP", "XASX", "AUD").await;
        insert_share_split(&pool, 1, ymd(2026, 6, 10), "1", "10").await;

        let market = load_market(&pool, 1).await.unwrap().unwrap();
        let stub = StubFetcher::default().with_close(1, ymd(2026, 6, 5), "12088.8", "AUD");
        fetch_and_store(&pool, &stub, &market, &[ymd(2026, 6, 5)])
            .await
            .unwrap();

        assert_eq!(
            stored(&pool, ymd(2026, 6, 5)).await,
            ("1208.88".to_string(), "12088.8".to_string()),
            "ten old units became one, so the pre-consolidation day's price is a tenth"
        );
    }

    /// A hand-entered price is contemporaneous by declaration: it is stored
    /// exactly as typed even with a split already recorded after its date, and
    /// recording another one never rewrites it.
    #[tokio::test]
    async fn api_a_manual_price_is_neither_normalised_on_entry_nor_rebased() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "BHP", "XASX", "AUD").await;
        insert_share_split(&pool, 1, ymd(2026, 6, 10), "10", "1").await;

        let client = ApiClient::over(router().with_state(pool.clone()));
        client
            .put(
                "/closing_prices/1/2026-06-05",
                &serde_json::json!({
                    "price": "1208.88",
                    "sourced_from": "asx.com.au closing report",
                    "reason": "provider serves no candle for that day",
                }),
            )
            .await
            .expect_status(StatusCode::NO_CONTENT);

        assert_eq!(
            stored(&pool, ymd(2026, 6, 5)).await,
            ("1208.88".to_string(), "1208.88".to_string()),
            "the operator's figure is its own observation"
        );

        insert_share_split(&pool, 1, ymd(2026, 6, 20), "2", "1").await;
        assert_eq!(
            stored(&pool, ymd(2026, 6, 5)).await.0,
            "1208.88",
            "nothing rewrites a figure a person typed"
        );
    }

    /// Editing the action re-derives the prices from the observation, and
    /// deleting it puts them back — neither is a delta applied to an
    /// already-adjusted number.
    #[tokio::test]
    async fn api_editing_or_deleting_the_split_re_derives_the_stored_prices() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "BHP", "XASX", "AUD").await;
        insert_share_split(&pool, 1, ymd(2026, 6, 10), "10", "1").await;
        let market = load_market(&pool, 1).await.unwrap().unwrap();
        let stub = StubFetcher::default().with_close(1, ymd(2026, 6, 5), "120.888", "AUD");
        fetch_and_store(&pool, &stub, &market, &[ymd(2026, 6, 5)])
            .await
            .unwrap();
        assert_eq!(stored(&pool, ymd(2026, 6, 5)).await.0, "1208.88");

        // A mis-keyed ratio, corrected in place.
        insert_share_split(&pool, 1, ymd(2026, 6, 10), "2", "1").await;
        assert_eq!(
            stored(&pool, ymd(2026, 6, 5)).await.0,
            "241.776",
            "the price follows the corrected ratio, from the observation"
        );

        // Moved to a date before the price: the price date is then already in
        // the post-split basis, so nothing is restated.
        insert_share_split(&pool, 1, ymd(2026, 6, 1), "2", "1").await;
        assert_eq!(
            stored(&pool, ymd(2026, 6, 5)).await.0,
            "120.888",
            "a split on or before the price date has already restated that day's close"
        );

        // …and deleting it altogether leaves the provider's figure standing.
        crate::entities::corporate_action::db_delete(&pool, 901)
            .await
            .unwrap();
        assert_eq!(stored(&pool, ymd(2026, 6, 5)).await.0, "120.888");
    }

    /// Deleting the *only* re-basing action leaves the listing with an empty
    /// event set, and the prices have to come back to the figures as observed
    /// — the case the walk must not short-circuit past.
    #[tokio::test]
    async fn db_deleting_the_last_split_puts_the_prices_back_to_the_observation() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "BHP", "XASX", "AUD").await;
        insert_share_split(&pool, 1, ymd(2026, 6, 10), "10", "1").await;
        let market = load_market(&pool, 1).await.unwrap().unwrap();
        let stub = StubFetcher::default().with_close(1, ymd(2026, 6, 5), "120.888", "AUD");
        fetch_and_store(&pool, &stub, &market, &[ymd(2026, 6, 5)])
            .await
            .unwrap();
        assert_eq!(stored(&pool, ymd(2026, 6, 5)).await.0, "1208.88");

        crate::entities::corporate_action::db_delete(&pool, 901)
            .await
            .unwrap();
        assert_eq!(
            stored(&pool, ymd(2026, 6, 5)).await,
            ("120.888".to_string(), "120.888".to_string()),
            "with the split gone there is nothing to restate out of any more"
        );
    }

    /// The one-off repair: a database whose prices were stored before this
    /// rule existed holds the provider's restated figure with a split already
    /// recorded. `run_rebase` (the `price-rebase` job) re-derives them, and is
    /// idempotent.
    #[tokio::test]
    async fn db_the_rebase_job_repairs_prices_stored_before_the_rule_existed() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "BHP", "XASX", "AUD").await;
        insert_share_split(&pool, 1, ymd(2026, 6, 10), "10", "1").await;
        // Stored the way a pre-0034 row was: the provider's post-split figure,
        // observed after the split, with nothing restated out of it.
        crate::test_support::closing_price(1, ymd(2026, 6, 5))
            .price("120.888")
            .fetched_at("2026-06-15T08:00:00Z")
            .insert(&pool)
            .await;

        run_rebase(&pool).await.unwrap();
        assert_eq!(stored(&pool, ymd(2026, 6, 5)).await.0, "1208.88");

        run_rebase(&pool).await.unwrap();
        assert_eq!(
            stored(&pool, ymd(2026, 6, 5)).await.0,
            "1208.88",
            "re-deriving from the observation is idempotent"
        );
    }

    // -----------------------------------------------------------------------
    // A demerger restates the price series too — and there is no ratio to read
    // (it changes no unit count on this listing), so the factor is derived
    // from the close the operator states the security actually traded at on
    // the last pre-demerger trading day. Evan's LAC history is the live case.
    // -----------------------------------------------------------------------

    /// Record a demerger of `listing_id` into `demerged_id`, optionally
    /// carrying the stated pre-demerger close the price factor is derived
    /// from, through the entity's own write path.
    async fn insert_demerger(
        pool: &SqlitePool,
        listing_id: i64,
        demerged_id: i64,
        date: NaiveDate,
        stated_close: Option<(NaiveDate, &str)>,
    ) {
        crate::entities::corporate_action::db_upsert(
            pool,
            &crate::entities::corporate_action::CorporateAction {
                id: 800 + listing_id,
                listing_id,
                date,
                kind: crate::entities::corporate_action::ActionKind::Demerger {
                    demerger_listing_id: demerged_id,
                    demerger_new_units: Decimal::ONE,
                    demerger_held_units: Decimal::ONE,
                    demerger_cost_base_pct: Decimal::from(36),
                    demerger_close_date: stated_close.map(|(d, _)| d),
                    demerger_close_price: stated_close.map(|(_, p)| p.parse().unwrap()),
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

    /// The LAC reproduction. The provider serves the whole pre-demerger series
    /// adjusted by its spin-off factor, so the day LAC actually closed at
    /// US$24.90 comes back as 10.13. Stating that close derives the factor and
    /// re-bases every pre-demerger day with it — the reference day back to
    /// exactly the stated figure, the days around it in proportion.
    #[tokio::test]
    async fn db_a_stated_pre_demerger_close_restates_the_whole_pre_demerger_series() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "LAC", "XNYS", "USD").await;
        insert_listing(&pool, 2, "LAR", "XNYS", "USD").await;
        let market = load_market(&pool, 1).await.unwrap().unwrap();
        let stub = StubFetcher::default()
            .with_close(1, ymd(2023, 9, 29), "10.00", "USD")
            .with_close(1, ymd(2023, 10, 2), "10.13", "USD")
            .with_close(1, ymd(2023, 10, 4), "11.72", "USD");
        fetch_and_store(
            &pool,
            &stub,
            &market,
            &[ymd(2023, 9, 29), ymd(2023, 10, 2), ymd(2023, 10, 4)],
        )
        .await
        .unwrap();
        assert_eq!(
            stored(&pool, ymd(2023, 10, 2)).await.0,
            "10.13",
            "with no stated close there is nothing to restate out of"
        );

        insert_demerger(
            &pool,
            1,
            2,
            ymd(2023, 10, 3),
            Some((ymd(2023, 10, 2), "24.90")),
        )
        .await;

        assert_eq!(
            stored(&pool, ymd(2023, 10, 2)).await,
            ("24.9".to_string(), "10.13".to_string()),
            "the reference day comes back to exactly the close the operator stated"
        );
        assert_eq!(
            stored(&pool, ymd(2023, 9, 29)).await.0,
            // 10.00 × 24.90/10.13, held to the provider's 7 significant digits.
            "24.58045",
            "every other pre-demerger day moves by the same derived factor"
        );
        assert_eq!(
            stored(&pool, ymd(2023, 10, 4)).await.0,
            "11.72",
            "a post-demerger day was never restated by the provider"
        );
    }

    /// The other half, as for a split: a pre-demerger day collected *before*
    /// the demerger already holds the contemporaneous close, and stating one
    /// later must leave it exactly as it is.
    #[tokio::test]
    async fn db_a_pre_demerger_price_observed_before_the_demerger_is_untouched() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "LAC", "XNYS", "USD").await;
        insert_listing(&pool, 2, "LAR", "XNYS", "USD").await;
        // Observed the day it traded — before the demerger, so contemporaneous.
        crate::test_support::closing_price(1, ymd(2023, 9, 29))
            .price("24.58")
            .fetched_at("2023-09-29T21:00:00Z")
            .insert(&pool)
            .await;
        // …and the reference day, observed long after it, which is what the
        // factor is derived from.
        crate::test_support::closing_price(1, ymd(2023, 10, 2))
            .price("10.13")
            .fetched_at("2026-07-26T07:44:56Z")
            .insert(&pool)
            .await;

        insert_demerger(
            &pool,
            1,
            2,
            ymd(2023, 10, 3),
            Some((ymd(2023, 10, 2), "24.90")),
        )
        .await;

        assert_eq!(
            stored(&pool, ymd(2023, 9, 29)).await.0,
            "24.58",
            "the fetch predates the demerger, so the figure was never adjusted"
        );
        assert_eq!(stored(&pool, ymd(2023, 10, 2)).await.0, "24.9");
    }

    /// The entry-order property, both ways: state the close before the history
    /// is backfilled, or backfill first and state it after. The reference row
    /// the factor divides by is one of the rows being fetched, so the fetch
    /// funnel re-derives once its range has landed.
    #[tokio::test]
    async fn db_entry_order_of_the_stated_close_and_the_backfill_does_not_change_the_price() {
        async fn backfill(pool: &SqlitePool) {
            let market = load_market(pool, 1).await.unwrap().unwrap();
            let stub = StubFetcher::default()
                .with_close(1, ymd(2023, 9, 29), "10.00", "USD")
                .with_close(1, ymd(2023, 10, 2), "10.13", "USD");
            fetch_and_store(pool, &stub, &market, &[ymd(2023, 9, 29), ymd(2023, 10, 2)])
                .await
                .unwrap();
        }
        async fn setup(pool: &SqlitePool) {
            insert_listing(pool, 1, "LAC", "XNYS", "USD").await;
            insert_listing(pool, 2, "LAR", "XNYS", "USD").await;
        }

        // The close stated first, then the history backfilled.
        let a = test_pool().await;
        setup(&a).await;
        insert_demerger(
            &a,
            1,
            2,
            ymd(2023, 10, 3),
            Some((ymd(2023, 10, 2), "24.90")),
        )
        .await;
        backfill(&a).await;

        // The history backfilled first, then the close stated.
        let b = test_pool().await;
        setup(&b).await;
        backfill(&b).await;
        insert_demerger(
            &b,
            1,
            2,
            ymd(2023, 10, 3),
            Some((ymd(2023, 10, 2), "24.90")),
        )
        .await;

        for date in [ymd(2023, 9, 29), ymd(2023, 10, 2)] {
            assert_eq!(
                stored(&a, date).await,
                stored(&b, date).await,
                "entry order cannot matter for {date}"
            );
        }
        assert_eq!(stored(&a, ymd(2023, 10, 2)).await.0, "24.9");
        assert_eq!(stored(&a, ymd(2023, 9, 29)).await.0, "24.58045");
    }

    /// A demerger and a split on the same listing compose: the split restated
    /// the reference figure too, so the derived demerger factor must divide it
    /// out rather than absorb it — otherwise the split would be applied twice
    /// to every pre-demerger day.
    #[tokio::test]
    async fn db_a_demerger_and_a_later_split_compose_without_double_counting() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "LAC", "XNYS", "USD").await;
        insert_listing(&pool, 2, "LAR", "XNYS", "USD").await;
        // A 2-for-1 split after the demerger: the provider halves everything
        // before it, on top of the spin-off adjustment.
        insert_share_split(&pool, 1, ymd(2024, 5, 1), "2", "1").await;
        insert_demerger(
            &pool,
            1,
            2,
            ymd(2023, 10, 3),
            Some((ymd(2023, 10, 2), "24.90")),
        )
        .await;

        let market = load_market(&pool, 1).await.unwrap().unwrap();
        // What the provider serves today: 24.90 × (10.13/24.90 spin-off) × 1/2.
        let stub = StubFetcher::default()
            .with_close(1, ymd(2023, 10, 2), "5.065", "USD")
            .with_close(1, ymd(2024, 6, 3), "7.50", "USD");
        fetch_and_store(&pool, &stub, &market, &[ymd(2023, 10, 2), ymd(2024, 6, 3)])
            .await
            .unwrap();

        assert_eq!(
            stored(&pool, ymd(2023, 10, 2)).await.0,
            "24.9",
            "the split is undone once and the spin-off once — not the split twice"
        );
        assert_eq!(
            stored(&pool, ymd(2024, 6, 3)).await.0,
            "7.5",
            "a day after both events is served in its own basis already"
        );
    }

    /// Editing the stated close re-derives the prices from the observation,
    /// removing it puts them back, and deleting the whole demerger does too —
    /// none of them a delta applied to an already-adjusted number.
    #[tokio::test]
    async fn db_editing_or_removing_the_stated_close_re_derives_the_prices() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "LAC", "XNYS", "USD").await;
        insert_listing(&pool, 2, "LAR", "XNYS", "USD").await;
        let market = load_market(&pool, 1).await.unwrap().unwrap();
        let stub = StubFetcher::default().with_close(1, ymd(2023, 10, 2), "10.13", "USD");
        fetch_and_store(&pool, &stub, &market, &[ymd(2023, 10, 2)])
            .await
            .unwrap();

        insert_demerger(
            &pool,
            1,
            2,
            ymd(2023, 10, 3),
            Some((ymd(2023, 10, 2), "24.90")),
        )
        .await;
        assert_eq!(stored(&pool, ymd(2023, 10, 2)).await.0, "24.9");

        // A mis-keyed close, corrected in place.
        insert_demerger(
            &pool,
            1,
            2,
            ymd(2023, 10, 3),
            Some((ymd(2023, 10, 2), "24.95")),
        )
        .await;
        assert_eq!(stored(&pool, ymd(2023, 10, 2)).await.0, "24.95");

        // Removing the statement altogether leaves the provider's figure.
        insert_demerger(&pool, 1, 2, ymd(2023, 10, 3), None).await;
        assert_eq!(stored(&pool, ymd(2023, 10, 2)).await.0, "10.13");

        // …as does deleting the demerger, once it is stated again.
        insert_demerger(
            &pool,
            1,
            2,
            ymd(2023, 10, 3),
            Some((ymd(2023, 10, 2), "24.90")),
        )
        .await;
        assert_eq!(stored(&pool, ymd(2023, 10, 2)).await.0, "24.9");
        crate::entities::corporate_action::db_delete(&pool, 801)
            .await
            .unwrap();
        assert_eq!(stored(&pool, ymd(2023, 10, 2)).await.0, "10.13");
    }

    /// The one-off repair path is the existing `price-rebase` job, extended
    /// rather than duplicated: a database whose pre-demerger prices were
    /// stored before the demerger's close was stated is repaired by it, and
    /// running it again is a no-op.
    #[tokio::test]
    async fn db_the_rebase_job_repairs_prices_a_demerger_restated() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "LAC", "XNYS", "USD").await;
        insert_listing(&pool, 2, "LAR", "XNYS", "USD").await;
        // Stored the way the live rows are: the provider's adjusted figure,
        // observed years after the demerger, with nothing taken out of it.
        for (date, price) in [(ymd(2023, 9, 29), "10.00"), (ymd(2023, 10, 2), "10.13")] {
            crate::test_support::closing_price(1, date)
                .price(price)
                .fetched_at("2026-07-26T07:44:56Z")
                .insert(&pool)
                .await;
        }
        // Written straight to the table, as a database predating the column
        // would have had it re-entered afterwards.
        sqlx::query("UPDATE closing_prices SET price = price_as_observed WHERE listing_id = 1")
            .execute(&pool)
            .await
            .unwrap();
        insert_demerger(
            &pool,
            1,
            2,
            ymd(2023, 10, 3),
            Some((ymd(2023, 10, 2), "24.90")),
        )
        .await;

        run_rebase(&pool).await.unwrap();
        assert_eq!(stored(&pool, ymd(2023, 10, 2)).await.0, "24.9");
        assert_eq!(stored(&pool, ymd(2023, 9, 29)).await.0, "24.58045");

        run_rebase(&pool).await.unwrap();
        assert_eq!(
            stored(&pool, ymd(2023, 10, 2)).await.0,
            "24.9",
            "re-deriving from the observation is idempotent"
        );
        assert_eq!(stored(&pool, ymd(2023, 9, 29)).await.0, "24.58045");
    }

    /// A hand-entered pre-demerger price is contemporaneous by declaration, so
    /// a stated close never rewrites it — the same one-way rule a split obeys.
    #[tokio::test]
    async fn api_a_manual_pre_demerger_price_is_never_rebased_by_a_stated_close() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "LAC", "XNYS", "USD").await;
        insert_listing(&pool, 2, "LAR", "XNYS", "USD").await;
        crate::test_support::closing_price(1, ymd(2023, 9, 29))
            .price("24.58")
            .fetched_at("2026-07-26T07:44:56Z")
            .manual("nyse.com", "provider adjusts the pre-demerger series")
            .insert(&pool)
            .await;
        crate::test_support::closing_price(1, ymd(2023, 10, 2))
            .price("10.13")
            .fetched_at("2026-07-26T07:44:56Z")
            .insert(&pool)
            .await;

        insert_demerger(
            &pool,
            1,
            2,
            ymd(2023, 10, 3),
            Some((ymd(2023, 10, 2), "24.90")),
        )
        .await;

        assert_eq!(
            stored(&pool, ymd(2023, 9, 29)).await.0,
            "24.58",
            "nothing rewrites a figure a person typed"
        );
        assert_eq!(stored(&pool, ymd(2023, 10, 2)).await.0, "24.9");
    }

    /// A re-base is an UPDATE of an audited table, so the superseded figure is
    /// recoverable — and it stales the snapshots that were valued at it.
    #[tokio::test]
    async fn db_a_rebase_is_audited_and_stales_the_snapshots_it_moves() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "BHP", "XASX", "AUD").await;
        let market = load_market(&pool, 1).await.unwrap().unwrap();
        let stub = StubFetcher::default().with_close(1, ymd(2026, 6, 5), "120.888", "AUD");
        fetch_and_store(&pool, &stub, &market, &[ymd(2026, 6, 5)])
            .await
            .unwrap();
        sqlx::query(
            "INSERT INTO report_snapshots (report, snapshot_date, generated_at, stale, rows_json) \
             VALUES ('portfolio_overview', '2026-06-05', '2026-06-06T00:00:00Z', 0, '[]')",
        )
        .execute(&pool)
        .await
        .unwrap();

        insert_share_split(&pool, 1, ymd(2026, 6, 10), "10", "1").await;

        let old: Vec<String> = sqlx::query_scalar(
            "SELECT json_extract(old_row, '$.price') FROM row_history \
             WHERE table_name = 'closing_prices' ORDER BY id",
        )
        .fetch_all(&pool)
        .await
        .unwrap();
        assert_eq!(
            old,
            vec!["120.888".to_string()],
            "the superseded figure is retained"
        );

        let stale: i64 = sqlx::query_scalar(
            "SELECT stale FROM report_snapshots WHERE snapshot_date = '2026-06-05'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(
            stale, 1,
            "the valuation that used the old figure regenerates"
        );
    }
}
