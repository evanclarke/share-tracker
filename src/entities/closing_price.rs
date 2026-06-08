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

use axum::{
    Extension, Json, Router,
    extract::{Query, State},
    http::StatusCode,
    routing::{get, post},
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

/// One stored closing price — or one recorded fetch failure (`status =
/// "error"`, `price` null, `error` set).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClosingPrice {
    pub listing_id: i64,
    pub price_date: NaiveDate,
    /// Closing price in the listing's quote currency; None exactly when the
    /// fetch failed.
    pub price: Option<Decimal>,
    /// Provider that produced the row, e.g. "yahoo".
    pub source: String,
    /// RFC 3339 UTC timestamp of the fetch that produced the row.
    pub fetched_at: String,
    pub status: PriceStatus,
    /// Failure detail; None exactly when the fetch succeeded.
    pub error: Option<String>,
}

impl<'r> sqlx::FromRow<'r, sqlx::sqlite::SqliteRow> for ClosingPrice {
    fn from_row(row: &'r sqlx::sqlite::SqliteRow) -> Result<Self, sqlx::Error> {
        let price: Option<String> = row.try_get("price")?;
        Ok(ClosingPrice {
            listing_id: row.try_get("listing_id")?,
            price_date: row.try_get("price_date")?,
            price: price.map(|p| parse_dec("price", p)).transpose()?,
            source: row.try_get("source")?,
            fetched_at: row.try_get("fetched_at")?,
            status: row.try_get("status")?,
            error: row.try_get("error")?,
        })
    }
}

// ---------------------------------------------------------------------------
// Market context: a listing plus the exchange-calendar data price collection
// needs (timezone, close time, holidays). `exchange` is None exactly for
// exchange-less (Crypto) listings.
// ---------------------------------------------------------------------------

pub struct Market {
    pub listing: listing::Listing,
    pub exchange: Option<exchange::Exchange>,
    pub holidays: HashSet<NaiveDate>,
}

impl Market {
    /// The timezone trading days are keyed on: the exchange's, or UTC for
    /// exchange-less (Crypto) listings, per the resolved cut-off convention.
    fn tz(&self) -> Result<Tz, String> {
        match &self.exchange {
            None => Ok(chrono_tz::UTC),
            Some(ex) => ex
                .timezone
                .parse()
                .map_err(|_| format!("exchange {} has unrecognised timezone {:?}", ex.mic, ex.timezone)),
        }
    }

    /// Whether `date` is a trading day: crypto trades every day; an exchange
    /// trades on weekdays that are not seeded public holidays.
    fn is_trading_day(&self, date: NaiveDate) -> bool {
        if self.exchange.is_none() {
            return true;
        }
        let weekday = date.weekday();
        weekday != chrono::Weekday::Sat
            && weekday != chrono::Weekday::Sun
            && !self.holidays.contains(&date)
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
    ///
    /// `None` only if no trading day exists in the past year (a calendar
    /// misconfiguration, e.g. every day seeded as a holiday).
    pub fn latest_complete_trading_day(
        &self,
        now: DateTime<Utc>,
    ) -> Result<Option<NaiveDate>, String> {
        let candidate = match &self.exchange {
            None => now.date_naive() - Duration::days(1),
            Some(ex) => {
                let close = NaiveTime::parse_from_str(&ex.close_time, "%H:%M").map_err(|_| {
                    format!("exchange {} has malformed close_time {:?}", ex.mic, ex.close_time)
                })?;
                let now_local = now.with_timezone(&self.tz()?);
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

/// Load the market context for a listing; None if the listing doesn't exist.
pub async fn load_market(pool: &SqlitePool, listing_id: i64) -> Result<Option<Market>, sqlx::Error> {
    let Some(listing) = listing::db_get(pool, listing_id).await? else {
        return Ok(None);
    };
    let exchange = match &listing.exchange_mic {
        None => None,
        Some(mic) => exchange::db_get(pool, mic).await?,
    };
    let holidays =
        crate::entities::exchange_holiday::exchange_holidays_for_listing(pool, listing_id).await?;
    Ok(Some(Market { listing, exchange, holidays }))
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
    fn daily_closes<'a>(&'a self, market: &'a Market, from: NaiveDate, to: NaiveDate)
    -> FetchFuture<'a>;

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

/// The Yahoo symbol for a listing: ASX tickers carry `.AX`, NYSE/Nasdaq are
/// plain, crypto is `<TICKER>-<quote currency>` (so Yahoo quotes it in the
/// listing's own currency). Other exchanges need a mapping added here.
fn yahoo_symbol(listing: &listing::Listing) -> Result<String, String> {
    match listing.exchange_mic.as_deref() {
        None => Ok(format!("{}-{}", listing.ticker, listing.currency)),
        Some("XASX") => Ok(format!("{}.AX", listing.ticker)),
        Some("XNYS") | Some("XNAS") => Ok(listing.ticker.clone()),
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
            let symbol = yahoo_symbol(&market.listing)?;
            let tz = market.tz()?;
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
                .iter()
                .map(|c| FetchedClose {
                    date: c.ts.with_timezone(&tz).date_naive(),
                    price: clean_price(c.close.amount()),
                    currency: c.close.currency().to_string(),
                })
                .collect())
        })
    }

    fn latest_quote<'a>(&'a self, market: &'a Market) -> QuoteFuture<'a> {
        Box::pin(async move {
            let symbol = yahoo_symbol(&market.listing)?;
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
                price: clean_price(price.amount()),
                currency: price.currency().to_string(),
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
        "SELECT listing_id, price_date, price, source, fetched_at, status, error \
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
        "SELECT listing_id, price_date, price, source, fetched_at, status, error \
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
/// (listing, date) — in particular, a success replaces an errored row.
async fn db_store(pool: &SqlitePool, row: &ClosingPrice) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO closing_prices \
             (listing_id, price_date, price, source, fetched_at, status, error) \
         VALUES (?, ?, ?, ?, ?, ?, ?) \
         ON CONFLICT(listing_id, price_date) DO UPDATE SET \
             price = excluded.price, \
             source = excluded.source, \
             fetched_at = excluded.fetched_at, \
             status = excluded.status, \
             error = excluded.error",
    )
    .bind(row.listing_id)
    .bind(row.price_date)
    .bind(row.price.map(|p| p.to_string()))
    .bind(&row.source)
    .bind(&row.fetched_at)
    .bind(row.status)
    .bind(&row.error)
    .execute(pool)
    .await?;
    Ok(())
}

/// Listings with a non-zero holding: total Buy/DRP quantity minus the units
/// consumed by parcel allocations (in as-acquired units — splits only rescale,
/// so they can't change whether the result is positive). Decimal arithmetic in
/// Rust, never float SUM in SQL. With `as_of` the holding is taken as at that
/// date — trades and sales dated after it don't count (snapshot generation for
/// a past date values what was held then, not what is held now).
pub async fn db_held_listing_ids(
    pool: &SqlitePool,
    as_of: Option<NaiveDate>,
) -> Result<Vec<i64>, sqlx::Error> {
    let cutoff = crate::infra::date::as_of_or_open(as_of);
    let buys = sqlx::query(
        "SELECT id, listing_id, quantity FROM trades \
         WHERE trade_type IN ('Buy', 'DRP') AND date <= ?",
    )
    .bind(cutoff)
    .fetch_all(pool)
    .await?;
    let mut listing_of: HashMap<i64, i64> = HashMap::new();
    let mut held: HashMap<i64, Decimal> = HashMap::new();
    for row in &buys {
        let trade_id: i64 = row.try_get("id")?;
        let listing_id: i64 = row.try_get("listing_id")?;
        let qty = parse_dec("quantity", row.try_get("quantity")?)?;
        listing_of.insert(trade_id, listing_id);
        *held.entry(listing_id).or_default() += qty;
    }

    let allocs = sqlx::query(
        "SELECT pa.purchase_trade_id, pa.quantity_allocated \
         FROM parcel_allocations pa JOIN trades s ON s.id = pa.sale_trade_id \
         WHERE s.date <= ?",
    )
    .bind(cutoff)
    .fetch_all(pool)
    .await?;
    for row in &allocs {
        let trade_id: i64 = row.try_get("purchase_trade_id")?;
        let qty = parse_dec("quantity_allocated", row.try_get("quantity_allocated")?)?;
        if let Some(listing_id) = listing_of.get(&trade_id) {
            *held.entry(*listing_id).or_default() -= qty;
        }
    }

    let mut ids: Vec<i64> =
        held.into_iter().filter(|(_, qty)| *qty > Decimal::ZERO).map(|(id, _)| id).collect();
    ids.sort();
    Ok(ids)
}

// ---------------------------------------------------------------------------
// Fetch-and-store: shared by scheduled collection, manual re-fetch and backfill
// ---------------------------------------------------------------------------

/// Fetch the given trading days for a listing (one provider call spanning the
/// range) and store one row per requested date: an ok row for a returned
/// candle in the listing's currency, an errored row for a fetch failure, a
/// missing candle, or a currency mismatch. Returns (ok, errored) counts.
async fn fetch_and_store(
    pool: &SqlitePool,
    fetcher: &dyn PriceFetcher,
    market: &Market,
    dates: &[NaiveDate],
) -> Result<(usize, usize), sqlx::Error> {
    let (Some(&from), Some(&to)) = (dates.iter().min(), dates.iter().max()) else {
        return Ok((0, 0));
    };

    let fetched = fetcher.daily_closes(market, from, to).await;
    let by_date: Result<HashMap<NaiveDate, FetchedClose>, String> =
        fetched.map(|closes| closes.into_iter().map(|c| (c.date, c)).collect());

    let fetched_at = Utc::now().to_rfc3339();
    let (mut ok, mut errored) = (0, 0);
    for &date in dates {
        let result = match &by_date {
            Err(e) => Err(e.clone()),
            Ok(map) => match map.get(&date) {
                None => Err("provider returned no candle for an expected trading day".to_string()),
                Some(close) if close.currency != market.listing.currency => Err(format!(
                    "currency mismatch: provider quoted {}, listing is {}",
                    close.currency, market.listing.currency
                )),
                Some(close) => Ok(close.price),
            },
        };
        let row = match result {
            Ok(price) => {
                ok += 1;
                ClosingPrice {
                    listing_id: market.listing.id,
                    price_date: date,
                    price: Some(price),
                    source: fetcher.source().to_string(),
                    fetched_at: fetched_at.clone(),
                    status: PriceStatus::Ok,
                    error: None,
                }
            }
            Err(e) => {
                errored += 1;
                ClosingPrice {
                    listing_id: market.listing.id,
                    price_date: date,
                    price: None,
                    source: fetcher.source().to_string(),
                    fetched_at: fetched_at.clone(),
                    status: PriceStatus::Error,
                    error: Some(e),
                }
            }
        };
        db_store(pool, &row).await?;
    }
    Ok((ok, errored))
}

/// One scheduled collection run: for every listing with a non-zero holding,
/// store the latest complete trading day's closing price unless it is already
/// stored ok. A non-trading day stores no row and is not an error; a failed
/// fetch stores an errored row and fails the job (so the Jobs UI shows it),
/// without stopping the other listings.
pub async fn run_collection(
    pool: &SqlitePool,
    fetcher: &dyn PriceFetcher,
    now: DateTime<Utc>,
) -> Result<(), String> {
    let ids = db_held_listing_ids(pool, None).await.map_err(|e| e.to_string())?;

    let (mut stored, mut skipped) = (0, 0);
    let mut failures: Vec<String> = Vec::new();
    for listing_id in ids {
        let market = load_market(pool, listing_id)
            .await
            .map_err(|e| e.to_string())?
            .ok_or_else(|| format!("listing {listing_id} disappeared during collection"))?;

        let target = match market.latest_complete_trading_day(now) {
            Ok(Some(date)) => date,
            Ok(None) => continue,
            Err(e) => {
                failures.push(e);
                continue;
            }
        };
        let already_ok = db_get_one(pool, listing_id, target)
            .await
            .map_err(|e| e.to_string())?
            .is_some_and(|row| row.status == PriceStatus::Ok);
        if already_ok {
            skipped += 1;
            continue;
        }

        let (ok, errored) =
            fetch_and_store(pool, fetcher, &market, &[target]).await.map_err(|e| e.to_string())?;
        stored += ok;
        if errored > 0 {
            failures.push(format!(
                "{} ({}): fetch failed for {target}, errored row stored",
                market.listing.ticker, listing_id
            ));
        }
    }

    tracing::info!(stored, skipped, failed = failures.len(), "closing-price collection complete");
    if failures.is_empty() { Ok(()) } else { Err(failures.join("; ")) }
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
                // Convert the quote-currency price to AUD via the standard FX
                // rules (ATO monthly rate for the quote's month, no manual
                // override). A missing rate is surfaced as the row's reason,
                // never a silent or zeroed value.
                match crate::infra::fx::to_aud(
                    pool,
                    quote.price,
                    &quote.currency,
                    quote.as_of.date_naive(),
                    None,
                )
                .await
                {
                    Ok(aud_price) => {
                        Ok(LiveValuation { aud_price, as_of: quote.as_of.to_rfc3339() })
                    }
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
) -> Result<Json<Vec<ClosingPrice>>, StatusCode> {
    db_list(&pool, params.listing_id, params.from, params.to)
        .await
        .map(Json)
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)
}

/// Re-fetch one (listing, date) on demand — typically to replace an errored
/// row. Returns the freshly stored row (which itself is errored if the
/// provider failed again).
async fn fetch_one(
    State(pool): State<SqlitePool>,
    Extension(fetcher): Extension<SharedFetcher>,
    Json(body): Json<FetchBody>,
) -> Result<Json<ClosingPrice>, (StatusCode, String)> {
    let market = load_market(&pool, body.listing_id)
        .await
        .map_err(internal)?
        .ok_or((StatusCode::NOT_FOUND, "no such listing".to_string()))?;
    validate_complete_trading_day(&market, body.price_date)?;

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
/// an old trade): trading days only, skipping dates already stored ok, in one
/// provider call.
async fn backfill(
    State(pool): State<SqlitePool>,
    Extension(fetcher): Extension<SharedFetcher>,
    Json(body): Json<BackfillBody>,
) -> Result<Json<BackfillSummary>, (StatusCode, String)> {
    let market = load_market(&pool, body.listing_id)
        .await
        .map_err(internal)?
        .ok_or((StatusCode::NOT_FOUND, "no such listing".to_string()))?;
    if body.from > body.to {
        return Err((StatusCode::UNPROCESSABLE_ENTITY, "from is after to".to_string()));
    }
    // Clamp the range to days whose close is final.
    let latest = market
        .latest_complete_trading_day(Utc::now())
        .map_err(unprocessable)?
        .filter(|latest| *latest >= body.from)
        .ok_or((
            StatusCode::UNPROCESSABLE_ENTITY,
            "range contains no complete trading day".to_string(),
        ))?;
    let to = body.to.min(latest);

    let mut trading_days: Vec<NaiveDate> = Vec::new();
    let mut date = body.from;
    while date <= to {
        if market.is_trading_day(date) {
            trading_days.push(date);
        }
        date += Duration::days(1);
    }
    let stored_ok = db_ok_dates(&pool, body.listing_id, body.from, to).await.map_err(internal)?;
    let missing: Vec<NaiveDate> =
        trading_days.iter().copied().filter(|d| !stored_ok.contains(d)).collect();

    let (fetched_ok, errored) =
        fetch_and_store(&pool, fetcher.as_ref(), &market, &missing).await.map_err(internal)?;
    Ok(Json(BackfillSummary {
        trading_days: trading_days.len(),
        already_stored: trading_days.len() - missing.len(),
        fetched_ok,
        errored,
    }))
}

/// 422 unless `date` is a trading day whose close has passed.
fn validate_complete_trading_day(
    market: &Market,
    date: NaiveDate,
) -> Result<(), (StatusCode, String)> {
    let latest = market.latest_complete_trading_day(Utc::now()).map_err(unprocessable)?;
    if latest.is_none_or(|latest| date > latest) {
        return Err((
            StatusCode::UNPROCESSABLE_ENTITY,
            format!("the close of {date} is not final yet"),
        ));
    }
    if !market.is_trading_day(date) {
        return Err((StatusCode::UNPROCESSABLE_ENTITY, format!("{date} is not a trading day")));
    }
    Ok(())
}

fn internal(e: impl std::fmt::Display) -> (StatusCode, String) {
    tracing::error!("closing-price request failed: {e}");
    (StatusCode::INTERNAL_SERVER_ERROR, String::new())
}

fn unprocessable(e: impl std::fmt::Display) -> (StatusCode, String) {
    (StatusCode::UNPROCESSABLE_ENTITY, e.to_string())
}

pub fn router() -> Router<SqlitePool> {
    Router::new()
        .route("/closing_prices", get(list))
        .route("/closing_prices/fetch", post(fetch_one))
        .route("/closing_prices/backfill", post(backfill))
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
        fail: Option<String>,
    }

    impl QuoteStub {
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
            QuoteStub { fail: Some(msg.to_string()), ..Default::default() }
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
            _market: &'a Market,
            _from: NaiveDate,
            _to: NaiveDate,
        ) -> FetchFuture<'a> {
            Box::pin(async move { Ok(vec![]) })
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
    use crate::entities::{exchange_holiday, parcel_allocation, trade};
    use crate::infra::db;
    use axum::{body::Body, http::Request};
    use http_body_util::BodyExt;
    use std::sync::Mutex;
    use tower::ServiceExt;

    async fn test_pool() -> SqlitePool {
        db::init(":memory:").await.unwrap()
    }

    fn ymd(y: i32, m: u32, d: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, d).unwrap()
    }

    fn utc(y: i32, m: u32, d: u32, h: u32, min: u32) -> DateTime<Utc> {
        chrono::TimeZone::with_ymd_and_hms(&Utc, y, m, d, h, min, 0).unwrap()
    }

    async fn insert_listing(pool: &SqlitePool, id: i64, ticker: &str, mic: &str, currency: &str) {
        listing::db_upsert(
            pool,
            &listing::Listing {
                id,
                exchange_mic: Some(mic.to_string()),
                ticker: ticker.to_string(),
                name: ticker.to_string(),
                isin: None,
                security_type: listing::SecurityType::Share,
                currency: currency.to_string(),
                amit: false,
                preference: false,
            },
        )
        .await
        .unwrap();
    }

    async fn insert_crypto_listing(pool: &SqlitePool, id: i64, ticker: &str) {
        listing::db_upsert(
            pool,
            &listing::Listing {
                id,
                exchange_mic: None,
                ticker: ticker.to_string(),
                name: ticker.to_string(),
                isin: None,
                security_type: listing::SecurityType::Crypto,
                currency: "AUD".to_string(),
                amit: false,
                preference: false,
            },
        )
        .await
        .unwrap();
    }

    async fn insert_buy(pool: &SqlitePool, id: i64, listing_id: i64, qty: &str) {
        trade::db_upsert(
            pool,
            &trade::Trade {
                brokerage_includes_gst: false,
                statement_total: None,
                holding_account_id: 1,
                transfer_id: None,
                ess_statement_id: None,
                id,
                trade_type: trade::TradeType::Buy,
                date: ymd(2024, 1, 15),
                settlement_date: ymd(2024, 1, 17),
                listing_id,
                average_price: Decimal::from(10),
                quantity: qty.parse().unwrap(),
                currency: "AUD".to_string(),
                brokerage: Decimal::ZERO,
                gst_on_brokerage: Decimal::ZERO,
                brokerage_currency: "AUD".to_string(),
                fx_rate: Decimal::ONE,
                contract_note_ref: None,
                residual_brought_forward: Decimal::ZERO,
                residual_carried_forward: Decimal::ZERO,
                residual_paid_out: Decimal::ZERO,
                rights_action_id: None,
                buyback_action_id: None,
                scrip_action_id: None,
                demerger_action_id: None,
                worthless_action_id: None,
                deemed_acquisition_date: None,
            },
        )
        .await
        .unwrap();
    }

    async fn sell_everything(pool: &SqlitePool, sell_id: i64, buy_id: i64, listing_id: i64, qty: &str) {
        trade::db_upsert(
            pool,
            &trade::Trade {
                brokerage_includes_gst: false,
                statement_total: None,
                holding_account_id: 1,
                transfer_id: None,
                ess_statement_id: None,
                id: sell_id,
                trade_type: trade::TradeType::Sell,
                date: ymd(2024, 6, 3),
                settlement_date: ymd(2024, 6, 5),
                listing_id,
                average_price: Decimal::from(12),
                quantity: qty.parse().unwrap(),
                currency: "AUD".to_string(),
                brokerage: Decimal::ZERO,
                gst_on_brokerage: Decimal::ZERO,
                brokerage_currency: "AUD".to_string(),
                fx_rate: Decimal::ONE,
                contract_note_ref: None,
                residual_brought_forward: Decimal::ZERO,
                residual_carried_forward: Decimal::ZERO,
                residual_paid_out: Decimal::ZERO,
                rights_action_id: None,
                buyback_action_id: None,
                scrip_action_id: None,
                demerger_action_id: None,
                worthless_action_id: None,
                deemed_acquisition_date: None,
            },
        )
        .await
        .unwrap();
        parcel_allocation::db_upsert(
            pool,
            &parcel_allocation::ParcelAllocation {
                id: sell_id,
                sale_trade_id: sell_id,
                purchase_trade_id: buy_id,
                quantity_allocated: qty.parse().unwrap(),
            },
        )
        .await
        .unwrap();
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
    }

    impl StubFetcher {
        fn with_close(mut self, listing_id: i64, date: NaiveDate, price: &str, ccy: &str) -> Self {
            self.closes.entry(listing_id).or_default().push(FetchedClose {
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
                LatestQuote { price: price.parse().unwrap(), currency: ccy.to_string(), as_of },
            );
            self
        }

        fn failing(msg: &str) -> Self {
            StubFetcher { fail: Some(msg.to_string()), ..Default::default() }
        }

        fn calls(&self) -> Vec<(i64, NaiveDate, NaiveDate)> {
            self.calls.lock().unwrap().clone()
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
                self.calls.lock().unwrap().push((market.listing.id, from, to));
                if let Some(msg) = &self.fail {
                    return Err(msg.clone());
                }
                Ok(self
                    .closes
                    .get(&market.listing.id)
                    .map(|v| {
                        v.iter().filter(|c| c.date >= from && c.date <= to).cloned().collect()
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

    // --- clean_price ---

    #[test]
    fn clean_price_strips_float_noise_and_keeps_tiny_prices() {
        let cases = [
            ("62.4799995422363", "62.48"),  // 62.48f32 — the live BHP.AX shape
            ("99545.3515625", "99545.35"),  // 99545.35f32 — the live BTC-AUD shape
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
            market.latest_complete_trading_day(friday_evening_sydney()).unwrap(),
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
            market.latest_complete_trading_day(utc(2026, 6, 7, 8, 0)).unwrap(),
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
            market.latest_complete_trading_day(utc(2026, 6, 7, 8, 0)).unwrap(),
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
            market.latest_complete_trading_day(utc(2026, 6, 7, 1, 30)).unwrap(),
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
        assert_eq!(db_held_listing_ids(&pool, Some(ymd(2024, 5, 31))).await.unwrap(), vec![1, 2]);
        assert!(db_held_listing_ids(&pool, Some(ymd(2024, 1, 1))).await.unwrap().is_empty());
    }

    // --- scheduled collection ---

    #[tokio::test]
    async fn collection_stores_price_per_held_listing_and_skips_non_held() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "BHP", "XASX", "AUD").await;
        insert_listing(&pool, 2, "IDLE", "XASX", "AUD").await;
        insert_buy(&pool, 1, 1, "100").await;
        let fetcher = StubFetcher::default().with_close(1, ymd(2026, 6, 5), "62.48", "AUD");

        run_collection(&pool, &fetcher, friday_evening_sydney()).await.unwrap();

        let rows = db_list(&pool, None, None, None).await.unwrap();
        assert_eq!(rows.len(), 1, "only the held listing is collected");
        let row = &rows[0];
        assert_eq!(row.listing_id, 1);
        assert_eq!(row.price_date, ymd(2026, 6, 5));
        assert_eq!(row.price, Some("62.48".parse().unwrap()));
        assert_eq!(row.status, PriceStatus::Ok);
        assert_eq!(row.source, "stub");
        assert!(row.error.is_none());
    }

    #[tokio::test]
    async fn collection_skips_a_day_already_stored_ok() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "BHP", "XASX", "AUD").await;
        insert_buy(&pool, 1, 1, "100").await;
        let fetcher = StubFetcher::default().with_close(1, ymd(2026, 6, 5), "62.48", "AUD");
        run_collection(&pool, &fetcher, friday_evening_sydney()).await.unwrap();
        assert_eq!(fetcher.calls().len(), 1);

        // A second run (same evening) finds the ok row and does not re-fetch.
        run_collection(&pool, &fetcher, friday_evening_sydney()).await.unwrap();
        assert_eq!(fetcher.calls().len(), 1, "no second provider call");
        assert_eq!(db_list(&pool, None, None, None).await.unwrap().len(), 1);
    }

    #[tokio::test]
    async fn collection_failure_stores_errored_row_and_fails_the_job() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "BHP", "XASX", "AUD").await;
        insert_buy(&pool, 1, 1, "100").await;
        let fetcher = StubFetcher::failing("provider down");

        let err = run_collection(&pool, &fetcher, friday_evening_sydney()).await.unwrap_err();
        assert!(err.contains("BHP"), "job error names the listing: {err}");

        let rows = db_list(&pool, None, None, None).await.unwrap();
        assert_eq!(rows.len(), 1, "the failure is recorded, never silently missing");
        assert_eq!(rows[0].status, PriceStatus::Error);
        assert!(rows[0].price.is_none());
        assert!(rows[0].error.as_deref().unwrap().contains("provider down"));
    }

    #[tokio::test]
    async fn collection_replaces_an_errored_row_once_the_provider_recovers() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "BHP", "XASX", "AUD").await;
        insert_buy(&pool, 1, 1, "100").await;
        run_collection(&pool, &StubFetcher::failing("down"), friday_evening_sydney())
            .await
            .unwrap_err();

        let fetcher = StubFetcher::default().with_close(1, ymd(2026, 6, 5), "62.48", "AUD");
        run_collection(&pool, &fetcher, friday_evening_sydney()).await.unwrap();

        let row = db_get_one(&pool, 1, ymd(2026, 6, 5)).await.unwrap().unwrap();
        assert_eq!(row.status, PriceStatus::Ok);
        assert_eq!(row.price, Some("62.48".parse().unwrap()));
        assert!(row.error.is_none());
    }

    #[tokio::test]
    async fn collection_records_currency_mismatch_as_error() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "ICE", "XNYS", "USD").await;
        insert_buy(&pool, 1, 1, "10").await;
        // Provider quotes AUD for a USD listing — wrong symbol mapping; the
        // price must not be stored as if it were USD.
        let fetcher = StubFetcher::default().with_close(1, ymd(2026, 6, 5), "141.50", "AUD");

        // 21:00 UTC Friday = 17:00 New York, after the close.
        run_collection(&pool, &fetcher, utc(2026, 6, 5, 21, 0)).await.unwrap_err();
        let row = db_get_one(&pool, 1, ymd(2026, 6, 5)).await.unwrap().unwrap();
        assert_eq!(row.status, PriceStatus::Error);
        assert!(row.error.as_deref().unwrap().contains("currency mismatch"));
    }

    #[tokio::test]
    async fn collection_crypto_collected_daily_at_utc_cutoff() {
        let pool = test_pool().await;
        insert_crypto_listing(&pool, 1, "BTC").await;
        insert_buy(&pool, 1, 1, "0.5").await;
        let fetcher = StubFetcher::default().with_close(1, ymd(2026, 6, 6), "86378.35", "AUD");

        // Sunday 01:30 UTC: Saturday 2026-06-06 is a complete crypto day.
        run_collection(&pool, &fetcher, utc(2026, 6, 7, 1, 30)).await.unwrap();
        let row = db_get_one(&pool, 1, ymd(2026, 6, 6)).await.unwrap().unwrap();
        assert_eq!(row.status, PriceStatus::Ok);
        assert_eq!(row.price, Some("86378.35".parse().unwrap()));
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

    #[tokio::test]
    async fn api_backfill_fetches_only_missing_trading_days() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "BHP", "XASX", "AUD").await;
        insert_buy(&pool, 1, 1, "100").await;
        // Week of Mon 2026-06-01 .. Fri 2026-06-05; Wednesday already stored ok.
        let pre = StubFetcher::default().with_close(1, ymd(2026, 6, 3), "64.91", "AUD");
        let market = load_market(&pool, 1).await.unwrap().unwrap();
        fetch_and_store(&pool, &pre, &market, &[ymd(2026, 6, 3)]).await.unwrap();

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
        assert_eq!(status, StatusCode::OK, "{}", String::from_utf8_lossy(&bytes));
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
        let wed = db_get_one(&pool, 1, ymd(2026, 6, 3)).await.unwrap().unwrap();
        assert_eq!(wed.price, Some("64.91".parse().unwrap()));
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

        let row = db_get_one(&pool, 1, ymd(2026, 6, 2)).await.unwrap().unwrap();
        assert_eq!(row.status, PriceStatus::Error);
        assert!(row.error.as_deref().unwrap().contains("no candle"));
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
        run_collection(&pool, &StubFetcher::failing("down"), friday_evening_sydney())
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

        let stored = db_get_one(&pool, 1, ymd(2026, 6, 5)).await.unwrap().unwrap();
        assert_eq!(stored.status, PriceStatus::Ok, "the errored row was replaced");
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
        fetch_and_store(&pool, &ok, &market1, &[ymd(2026, 6, 4), ymd(2026, 6, 5)]).await.unwrap();
        fetch_and_store(&pool, &StubFetcher::failing("down"), &market2, &[ymd(2026, 6, 5)])
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

        assert_eq!(get("/closing_prices").await.len(), 3, "errored rows are listed too");
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
        sqlx::query("INSERT INTO rba_fx_rates (currency, month, rate) VALUES ('USD', '2026-06', '2')")
            .execute(&pool)
            .await
            .unwrap();
        let as_of = utc(2026, 6, 5, 6, 30);
        let fetcher = StubFetcher::default()
            .with_quote(1, "62.48", "AUD", as_of)
            .with_quote(2, "141.50", "USD", as_of);

        let prices = fetch_live_aud_prices(&pool, &fetcher, &[1, 2]).await.unwrap();
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
        let u = fetch_live_aud_prices(&pool, &unconvertible, &[3]).await.unwrap();
        assert!(u[&3].as_ref().unwrap_err().contains("no ATO FX rate"));
    }

    #[tokio::test]
    async fn resolve_live_prices_skips_overridden_and_respects_the_flag() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "BHP", "XASX", "AUD").await;
        insert_listing(&pool, 2, "WBC", "XASX", "AUD").await;
        let as_of = utc(2026, 6, 5, 6, 30);
        let fetcher =
            StubFetcher::default().with_quote(1, "62.48", "AUD", as_of).with_quote(2, "30", "AUD", as_of);

        // live = false → nothing fetched.
        let off = resolve_live_prices(&pool, Some(&fetcher), false, &HashMap::new(), [1, 2])
            .await
            .unwrap();
        assert!(off.is_empty());

        // live = true, listing 1 overridden → only listing 2 is fetched.
        let overrides = HashMap::from([(1i64, "99".parse::<Decimal>().unwrap())]);
        let on = resolve_live_prices(&pool, Some(&fetcher), true, &overrides, [1, 2]).await.unwrap();
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
        fetch_and_store(&pool, &ok, &market, &[ymd(2026, 6, 5)]).await.unwrap();
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
        let mk = |mic: Option<&str>, ticker: &str, ccy: &str| listing::Listing {
            id: 1,
            exchange_mic: mic.map(str::to_string),
            ticker: ticker.to_string(),
            name: ticker.to_string(),
            isin: None,
            security_type: if mic.is_none() {
                listing::SecurityType::Crypto
            } else {
                listing::SecurityType::Share
            },
            currency: ccy.to_string(),
            amit: false,
            preference: false,
        };
        assert_eq!(yahoo_symbol(&mk(Some("XASX"), "BHP", "AUD")).unwrap(), "BHP.AX");
        assert_eq!(yahoo_symbol(&mk(Some("XNYS"), "ICE", "USD")).unwrap(), "ICE");
        assert_eq!(yahoo_symbol(&mk(None, "BTC", "AUD")).unwrap(), "BTC-AUD");
        assert!(yahoo_symbol(&mk(Some("XLON"), "BARC", "GBP")).unwrap_err().contains("XLON"));
    }
}
