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
//! - A failed fetch is stored as an errored row for that (listing, date) —
//!   never a silent zero or a skipped row — and is replaced by a later
//!   successful re-run.
//! - Only an **errored** row is deletable ([`db_delete`]): the acknowledgement
//!   that no price will ever exist for that day. An ok row is replaced by a
//!   re-fetch, never removed, so no valuation can lose a price it once had.
//! - A day the provider cannot serve at all can be priced **by hand**
//!   (`PUT /closing_prices/{listing_id}/{price_date}`), recorded with where
//!   the figure was sourced from and why manual entry was needed
//!   ([`PriceOrigin::Manual`]). Valuation reads such a row exactly like a
//!   fetched one. The provider never takes the day back: collection and
//!   backfill skip it as an ok row, and an explicit re-fetch is refused — a
//!   manual price is changed only by entering another one.

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
use crate::infra::decimal::parse_dec;

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
#[derive(Debug, Clone, Serialize, Deserialize)]
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
    /// Closing price in the listing's quote currency; None exactly when the
    /// fetch failed.
    pub price: Option<Decimal>,
    /// Provider that produced the row, e.g. "yahoo" — [`MANUAL_SOURCE`]
    /// exactly when `origin` is `Manual`.
    pub source: String,
    /// RFC 3339 UTC timestamp of the fetch that produced the row — for a
    /// manual row, of the entry that recorded it.
    pub fetched_at: String,
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

impl<'r> sqlx::FromRow<'r, sqlx::sqlite::SqliteRow> for ClosingPrice {
    fn from_row(row: &'r sqlx::sqlite::SqliteRow) -> Result<Self, sqlx::Error> {
        let price: Option<String> = row.try_get("price")?;
        Ok(ClosingPrice {
            id: row.try_get("id")?,
            listing_id: row.try_get("listing_id")?,
            price_date: row.try_get("price_date")?,
            price: price.map(|p| parse_dec("price", p)).transpose()?,
            source: row.try_get("source")?,
            fetched_at: row.try_get("fetched_at")?,
            status: row.try_get("status")?,
            error: row.try_get("error")?,
            origin: row.try_get("origin")?,
            sourced_from: row.try_get("sourced_from")?,
            reason: row.try_get("reason")?,
        })
    }
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

    fn daily_closes<'a>(
        &'a self,
        market: &'a Market,
        from: NaiveDate,
        to: NaiveDate,
    ) -> FetchFuture<'a> {
        Box::pin(async move {
            // `from..=to` sits inside one identity by contract, so its symbol
            // and calendar answer for the whole call.
            let symbol = yahoo_symbol(market, from)?;
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
        "SELECT id, listing_id, price_date, price, source, fetched_at, status, error, \
                origin, sourced_from, reason \
         FROM closing_prices WHERE listing_id = ? AND price_date = ?",
    )
    .bind(listing_id)
    .bind(price_date)
    .fetch_optional(pool)
    .await
}

/// Stored prices, newest first, optionally filtered by listing and date range.
pub async fn db_list(
    pool: &SqlitePool,
    listing_id: Option<i64>,
    from: Option<NaiveDate>,
    to: Option<NaiveDate>,
) -> Result<Vec<ClosingPrice>, sqlx::Error> {
    let mut qb = QueryBuilder::new(
        "SELECT id, listing_id, price_date, price, source, fetched_at, status, error, \
                origin, sourced_from, reason \
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
             (listing_id, price_date, price, source, fetched_at, status, error, \
              origin, sourced_from, reason) \
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?) \
         ON CONFLICT(listing_id, price_date) DO UPDATE SET \
             price = excluded.price, \
             source = excluded.source, \
             fetched_at = excluded.fetched_at, \
             status = excluded.status, \
             error = excluded.error, \
             origin = excluded.origin, \
             sourced_from = excluded.sourced_from, \
             reason = excluded.reason",
    )
    .bind(row.listing_id)
    .bind(row.price_date)
    .bind(row.price.map(|p| p.to_string()))
    .bind(&row.source)
    .bind(&row.fetched_at)
    .bind(row.status)
    .bind(&row.error)
    .bind(row.origin)
    .bind(&row.sourced_from)
    .bind(&row.reason)
    .execute(pool)
    .await?;
    Ok(())
}

/// Delete one stored row, reporting whether one was there. Callers must have
/// established that the row is errored (the handler rejects an ok row): an
/// errored date is never valued — `reports::valuation` blocks it outright —
/// so removing the row cannot invalidate a stored snapshot. That is what lets
/// `closing_prices` keep its single `..._stale_snapshots_update` trigger
/// (0001_schema.sql) with no DELETE counterpart, unlike the fact tables.
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
async fn fetch_and_store(
    pool: &SqlitePool,
    fetcher: &dyn PriceFetcher,
    market: &Market,
    dates: &[NaiveDate],
) -> Result<(usize, usize), sqlx::Error> {
    let (Some(&overall_from), Some(&overall_to)) = (dates.iter().min(), dates.iter().max()) else {
        return Ok((0, 0));
    };

    // Per requested date: the fetch outcome for the segment it falls in.
    let mut outcome: HashMap<NaiveDate, Result<Decimal, String>> = HashMap::new();
    for (from, to, _identity) in market.identity_segments(overall_from, overall_to) {
        let wanted: Vec<NaiveDate> = dates
            .iter()
            .copied()
            .filter(|d| *d >= from && *d <= to)
            .collect();
        if wanted.is_empty() {
            continue; // a segment the caller asked for no days in
        }
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
            let symbol = yahoo_symbol(market, from).unwrap_or_else(|e| e);
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
            outcome.insert(date, result);
        }
    }

    let fetched_at = Utc::now().to_rfc3339();
    let (mut ok, mut errored) = (0, 0);
    for &date in dates {
        let result = outcome
            .remove(&date)
            .unwrap_or_else(|| Err("no identity span covers this date".to_string()));
        let row = match result {
            Ok(price) => {
                ok += 1;
                ClosingPrice {
                    id: UNASSIGNED_ID,
                    listing_id: market.listing.id,
                    price_date: date,
                    price: Some(price),
                    source: fetcher.source().to_string(),
                    fetched_at: fetched_at.clone(),
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
                    source: fetcher.source().to_string(),
                    fetched_at: fetched_at.clone(),
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
        source: MANUAL_SOURCE.to_string(),
        fetched_at: Utc::now().to_rfc3339(),
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
    let to = body.to.min(latest);

    let mut trading_days: Vec<NaiveDate> = Vec::new();
    let mut date = body.from;
    while date <= to {
        if market.is_trading_day(date) {
            trading_days.push(date);
        }
        date += Duration::days(1);
    }
    let stored_ok = db_ok_dates(&pool, body.listing_id, body.from, to)
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
async fn delete_one(
    State(pool): State<SqlitePool>,
    Path((listing_id, price_date)): Path<(i64, NaiveDate)>,
) -> Result<StatusCode, ApiError> {
    let row = db_get_one(&pool, listing_id, price_date)
        .await?
        .ok_or_else(|| ApiError::not_found("no stored price for that listing and date"))?;
    if row.status == PriceStatus::Ok {
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
                let symbol = yahoo_symbol(market, from)?;
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
    use crate::test_support::test_pool;
    use axum::{body::Body, http::Request, http::StatusCode};
    use http_body_util::BodyExt;
    use std::sync::Mutex;
    use tower::ServiceExt;

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
                    .push(yahoo_symbol(market, from).unwrap_or_default());
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

    fn full_router(pool: SqlitePool, fetcher: StubFetcher) -> axum::Router {
        let shared: SharedFetcher = Arc::new(fetcher);
        router().with_state(pool).layer(Extension(shared))
    }

    async fn post_json(
        app: &axum::Router,
        uri: &str,
        body: serde_json::Value,
    ) -> (StatusCode, axum::body::Bytes) {
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(uri)
                    .header("content-type", "application/json")
                    .body(Body::from(body.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        let status = resp.status();
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        (status, bytes)
    }

    async fn delete_req(app: &axum::Router, uri: &str) -> (StatusCode, axum::body::Bytes) {
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("DELETE")
                    .uri(uri)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let status = resp.status();
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
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

    // --- manual prices ---

    async fn put_json(
        app: &axum::Router,
        uri: &str,
        body: serde_json::Value,
    ) -> (StatusCode, axum::body::Bytes) {
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("PUT")
                    .uri(uri)
                    .header("content-type", "application/json")
                    .body(Body::from(body.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        let status = resp.status();
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
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
        assert_eq!(valuations.len(), 1);
        assert_eq!(valuations[0].native_price, "62.48".parse().unwrap());
        assert_eq!(valuations[0].aud_price, "62.48".parse().unwrap());
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
                         (listing_id, price_date, price, fetched_at, status, error, {columns}) \
                     VALUES (1, '2026-06-05', '1.23', 'now', 'ok', NULL, {values})"
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
        let app = router().with_state(pool.clone()).layer(Extension(shared));

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
                let resp = app
                    .oneshot(Request::builder().uri(uri).body(Body::empty()).unwrap())
                    .await
                    .unwrap();
                assert_eq!(resp.status(), StatusCode::OK);
                let bytes = resp.into_body().collect().await.unwrap().to_bytes();
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
        insert_listing(&pool, 1, "OLD", "XASX", "AUD").await;
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
        insert_listing(&pool, 1, "OLD", "XASX", "AUD").await;
        exchange_holiday::db_upsert(
            &pool,
            &exchange_holiday::ExchangeHoliday {
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
        let app = router().with_state(pool.clone()).layer(Extension(shared));
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
}
