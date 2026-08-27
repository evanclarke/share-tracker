//! The pluggable price fetcher: the [`PriceFetcher`] trait every provider
//! implements, the types its two questions are asked and answered in, and the
//! two provider-agnostic pieces built on it — [`CachingFetcher`] (a decorator
//! that reuses a quote for a short window) and [`clean_price`] (rounding away
//! the float noise a provider's binary floats arrive with).
//!
//! The live implementation lives beside this in `yahoo`; nothing here knows
//! about any particular provider.

use super::market::Market;
use chrono::{DateTime, NaiveDate, Utc};
use rust_decimal::Decimal;
use std::{
    collections::HashMap,
    future::Future,
    pin::Pin,
    sync::{Arc, Mutex},
    time::Instant,
};

/// One daily close as returned by a provider, before it is checked and stored.
#[derive(Debug, Clone)]
pub struct FetchedClose {
    pub date: NaiveDate,
    pub price: Decimal,
    /// The quote currency the provider reports — cross-checked against the
    /// listing's currency before the price is stored.
    pub currency: String,
}

/// Why a [`PriceFetcher::daily_closes`] call failed, in the only distinction
/// [`fetch_and_store`] can act on: did the provider *positively answer* that
/// it has no such series, or did the call merely not succeed?
///
/// Only the first is evidence about the symbol. A rate limit, a 5xx, a
/// timeout or a connection failure says nothing about whether the symbol is
/// right, and diagnosing one of those as a dead symbol would send the
/// operator hunting for a rename that never happened — which is why the
/// classification is made by the provider adapter, where the provider's own
/// typed error is still in hand, rather than by string-matching the message
/// afterwards.
#[derive(thiserror::Error, Debug, Clone, PartialEq, Eq)]
pub enum FetchError {
    /// The provider answered that it serves no series under this symbol
    /// (Yahoo: `404 Not found`, or the `400` its chart API returns for an
    /// unknown/retired ticker). The classic wrong/renamed/delisted-symbol
    /// case, arriving as an error rather than as an empty window.
    #[error("{0}")]
    NoSuchSymbol(String),
    /// Anything else: an outage, a rate limit, a transport failure, or a
    /// local failure that never reached the provider (an unresolvable symbol
    /// or timezone). Carries no verdict on the symbol.
    #[error("{0}")]
    Other(String),
}

impl FetchError {
    /// The failure text as it would be shown or stored.
    pub fn message(&self) -> &str {
        match self {
            Self::NoSuchSymbol(m) | Self::Other(m) => m,
        }
    }
}

/// A failure raised *before* the provider was reached (symbol or timezone
/// resolution) is never evidence about the symbol's existence.
impl From<String> for FetchError {
    fn from(message: String) -> Self {
        Self::Other(message)
    }
}

pub type FetchFuture<'a> =
    Pin<Box<dyn Future<Output = Result<Vec<FetchedClose>, FetchError>> + Send + 'a>>;

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

/// One result per market asked for, positionally — see
/// [`PriceFetcher::latest_quotes`]. Every element is its own `Result`, so one
/// listing the provider cannot serve never costs the others their valuation.
pub type QuotesFuture<'a> =
    Pin<Box<dyn Future<Output = Vec<Result<LatestQuote, String>>> + Send + 'a>>;

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
    ///
    /// A failure is classified as it is raised ([`FetchError`]): an
    /// implementation answers `NoSuchSymbol` only where the provider itself
    /// said it has no such series, and `Other` for everything else.
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

    /// [`Self::latest_quote`] for **every** market at once, which is how live
    /// valuation actually asks: a price-dependent report values the whole
    /// portfolio, so the per-listing loop this replaces cost one provider
    /// round trip per held listing — the dominant cost of loading the
    /// Portfolio Overview screen (measured: ~2.5 s for five holdings against
    /// ~0.6 s for the same five in one request, and it grew with the
    /// portfolio). Providers that quote many symbols per request override
    /// this; the default is the honest sequential loop, so a fetcher that
    /// cannot batch stays correct by implementing nothing.
    ///
    /// The answer is **positional and total**: `out[i]` is `markets[i]`'s
    /// result and `out.len() == markets.len()`, so no listing can go missing
    /// from a valuation. A whole-batch failure is therefore reported as that
    /// failure against each market, never as a short answer.
    ///
    /// Takes **borrowed** markets so a caller holding only some of them — a
    /// cache passing on the ones it could not answer ([`CachingFetcher`]) —
    /// can forward a subset without copying a `Market` (each carries its
    /// exchange's whole holiday calendar).
    fn latest_quotes<'a>(&'a self, markets: &'a [&'a Market]) -> QuotesFuture<'a> {
        Box::pin(async move {
            let mut out = Vec::with_capacity(markets.len());
            for market in markets {
                out.push(self.latest_quote(market).await);
            }
            out
        })
    }
}

/// The fetcher handlers receive via an axum `Extension` (so tests can inject a
/// stub instead of the live provider).
pub type SharedFetcher = Arc<dyn PriceFetcher>;

/// A [`PriceFetcher`] that remembers each listing's latest quote for a short
/// window, so repeat valuations inside it are answered without reaching the
/// provider at all.
///
/// The Portfolio Overview is the app's home screen and three reports take
/// live prices (overview, unrealised gains, performance), so the same quotes
/// are asked for again on every visit, every reload, and every hop between
/// those three — each one a fresh round trip to Yahoo before this.
///
/// What is cached, and what deliberately is not:
///
/// - **Quotes only.** [`Self::daily_closes`] passes straight through: price
///   *history* is already persisted in `closing_prices`, and the
///   `price-import` job that collects it must never read a remembered answer.
/// - **The provider's quote, not the AUD conversion.** The conversion depends
///   on `rba_fx_rates`, which a rate import can change under us; caching the
///   raw quote leaves every valuation to convert against the database as it
///   is now. This is what keeps a cached price out of the FX rules.
/// - **Successes only.** A failed fetch is never remembered, so an outage or
///   a rate limit is retried on the next request rather than pinned for the
///   window — recovery is the behaviour worth having, and a failure costs the
///   provider nothing to re-ask.
///
/// Nothing here makes a stale price *look* current: a row's `price_as_of` is
/// the provider's own quote timestamp, carried through the cache untouched,
/// so a served-from-cache valuation reports exactly the moment it was
/// observed — which is what the UI's "Live prices as at …" line shows.
///
/// Keyed by listing id rather than provider symbol: it is the identity the
/// caller asks with, and it keeps this decorator free of any provider's
/// symbol conventions. That also bounds the map at one entry per listing —
/// a small reference table — so an expired entry is overwritten in place
/// rather than needing eviction. The cost is that re-pointing a listing at a
/// different symbol (a ticker rename, an edited `price_symbol`) keeps
/// answering from the old symbol's quote until the entry ages out — bounded
/// by the TTL, and the reason that window is a minute rather than an hour.
pub struct CachingFetcher {
    inner: SharedFetcher,
    ttl: std::time::Duration,
    /// listing id → when it was fetched, and what came back.
    cached: Mutex<HashMap<i64, (Instant, LatestQuote)>>,
}

impl CachingFetcher {
    pub fn new(inner: SharedFetcher, ttl: std::time::Duration) -> Self {
        Self {
            inner,
            ttl,
            cached: Mutex::new(HashMap::new()),
        }
    }

    /// The remembered quote for `listing_id`, if it is still inside the
    /// window. A zero TTL is therefore never a hit, which is how a caller
    /// turns the cache off.
    fn remembered(&self, listing_id: i64) -> Option<LatestQuote> {
        let cached = self.cached.lock().ok()?;
        let (fetched_at, quote) = cached.get(&listing_id)?;
        (fetched_at.elapsed() < self.ttl).then(|| quote.clone())
    }

    fn remember(&self, listing_id: i64, quote: &LatestQuote) {
        // A poisoned lock means another thread panicked mid-update. That is
        // not a reason to fail a valuation: the cache is an optimisation, and
        // the quote in hand is already correct.
        if let Ok(mut cached) = self.cached.lock() {
            cached.insert(listing_id, (Instant::now(), quote.clone()));
        }
    }
}

impl PriceFetcher for CachingFetcher {
    fn source(&self) -> &'static str {
        self.inner.source()
    }

    fn symbol(&self, market: &Market, date: NaiveDate) -> Result<String, String> {
        self.inner.symbol(market, date)
    }

    /// Never cached — see the type's docs.
    fn daily_closes<'a>(
        &'a self,
        market: &'a Market,
        from: NaiveDate,
        to: NaiveDate,
    ) -> FetchFuture<'a> {
        self.inner.daily_closes(market, from, to)
    }

    fn latest_quote<'a>(&'a self, market: &'a Market) -> QuoteFuture<'a> {
        Box::pin(async move {
            if let Some(quote) = self.remembered(market.listing.id) {
                return Ok(quote);
            }
            let quote = self.inner.latest_quote(market).await?;
            self.remember(market.listing.id, &quote);
            Ok(quote)
        })
    }

    /// The misses go to the provider as **one** batch, so the caching and the
    /// batching compose: a portfolio with one newly-held listing costs one
    /// request carrying one symbol, not one request per holding.
    fn latest_quotes<'a>(&'a self, markets: &'a [&'a Market]) -> QuotesFuture<'a> {
        Box::pin(async move {
            let remembered: Vec<Option<LatestQuote>> = markets
                .iter()
                .map(|m| self.remembered(m.listing.id))
                .collect();
            let misses: Vec<&Market> = markets
                .iter()
                .zip(&remembered)
                .filter(|(_, hit)| hit.is_none())
                .map(|(m, _)| *m)
                .collect();
            if misses.is_empty() {
                return remembered.into_iter().flatten().map(Ok).collect();
            }

            let mut fetched = self.inner.latest_quotes(&misses).await;
            // The positional contract again, one level down: hold the inner
            // fetcher to one result per market it was given, so a short answer
            // cannot slide the results onto the wrong listings below.
            fetched.resize_with(misses.len(), || {
                Err("price source returned no result for this listing".to_string())
            });
            for (market, result) in misses.iter().zip(&fetched) {
                if let Ok(quote) = result {
                    self.remember(market.listing.id, quote);
                }
            }

            let mut fetched = fetched.into_iter();
            remembered
                .into_iter()
                .map(|hit| match hit {
                    Some(quote) => Ok(quote),
                    // One `fetched` entry per `None`, in the order the misses
                    // were collected — which is the order of `markets`.
                    None => fetched
                        .next()
                        .unwrap_or_else(|| Err("price source skipped this listing".to_string())),
                })
                .collect()
        })
    }
}

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
