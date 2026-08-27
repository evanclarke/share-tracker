//! Yahoo Finance, via the `yfinance-rs` crate — the live [`PriceFetcher`]
//! (see the entity's module docs for the provider decision and its verified
//! behaviour).
//!
//! Everything provider-specific is here: the symbol mapping (ASX `.AX`, US
//! plain, crypto `<TICKER>-<quote currency>`) resolved against the identity in
//! force, the failure classification that decides what counts as "no such
//! series", the candle-timestamp→trading-day conversion, and the by-symbol
//! reading of a multi-symbol quote answer.

use super::fetcher::{
    FetchError, FetchFuture, FetchedClose, LatestQuote, PriceFetcher, QuoteFuture, QuotesFuture,
    clean_price,
};
use super::market::{Market, MarketIdentity};
use chrono::{DateTime, Duration, NaiveDate, Utc};
use chrono_tz::Tz;
use std::collections::{HashMap, HashSet};

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
pub(crate) fn yahoo_symbol(market: &Market, date: NaiveDate) -> Result<String, String> {
    yahoo_symbol_for(market, market.identity_at(date))
}

/// The Yahoo symbol for the identity in effect **now** — for a live quote,
/// which is always a question about today.
pub(super) fn yahoo_symbol_now(market: &Market) -> Result<String, String> {
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

/// Classify a `yfinance-rs` failure into the one distinction
/// [`fetch_and_store`] acts on, from the crate's **typed** error rather than
/// its rendered message — the status is a field on the variant, so nothing
/// here depends on the provider's or the crate's wording.
///
/// Yahoo has two ways of saying "no such series": `404` (the crate's
/// `NotFound`, rendered "Not found at …") and the `400` its chart API answers
/// for an unknown or retired ticker (`Status { status: 400 }`, rendered
/// "Unexpected response status: 400 at …") — measured against the live
/// provider for `ZZQQNOTREAL` and for `FB` after the FB → META rename
/// (SCENARIOS R-06). Everything else stays [`FetchError::Other`], deliberately
/// including the rest of the 4xx range: a `401`/`403` is a credential or
/// crumb problem and a `429` is a rate limit, neither of which is evidence
/// that the symbol is wrong, and misdiagnosing an outage as a dead symbol is
/// the failure mode this narrowness exists to prevent.
pub(super) fn classify_yahoo_failure(symbol: &str, error: yfinance_rs::YfError) -> FetchError {
    let message = format!("yahoo fetch for {symbol} failed: {error}");
    match error {
        yfinance_rs::YfError::NotFound { .. }
        | yfinance_rs::YfError::Status { status: 400, .. } => FetchError::NoSuchSymbol(message),
        _ => FetchError::Other(message),
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
                .map_err(|e| classify_yahoo_failure(&symbol, e))?;
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
            yahoo_quote_named(&quotes, &symbol)
        })
    }

    /// Yahoo's quote endpoint takes a symbol *list* and answers the lot in one
    /// request, so a portfolio is one round trip rather than one per holding
    /// (see the trait method's docs for the measurement).
    fn latest_quotes<'a>(&'a self, markets: &'a [&'a Market]) -> QuotesFuture<'a> {
        Box::pin(async move {
            // Symbols first: a market whose symbol cannot be resolved at all
            // (an exchange with no mapping) is its own failure and is never
            // put to the provider — but it still occupies its own slot in the
            // answer, so the positional contract holds.
            let symbols: Vec<Result<String, String>> =
                markets.iter().copied().map(yahoo_symbol_now).collect();
            // Deduplicated: two listings can resolve to one symbol, and asking
            // twice in the same request would be asking Yahoo to repeat itself.
            let mut wanted: Vec<&str> = symbols
                .iter()
                .filter_map(|s| s.as_deref().ok())
                .collect::<HashSet<_>>()
                .into_iter()
                .collect();
            // HashSet order is arbitrary; sorting makes the requests (and so
            // the failure messages below) reproducible.
            wanted.sort_unstable();

            let mut quotes: Vec<yfinance_rs::Quote> = Vec::with_capacity(wanted.len());
            // Which symbols their own request failed for, and why. Attributed
            // per chunk rather than per batch: a chunk that failed says
            // nothing about the symbols in a chunk that succeeded.
            // Owned keys: `wanted` borrows `symbols`, which is consumed
            // below to build the answer.
            let mut failures: HashMap<String, String> = HashMap::new();
            for chunk in wanted.chunks(QUOTE_BATCH_SYMBOLS) {
                match yfinance_rs::quotes(&self.client, chunk.iter().copied()).await {
                    Ok(answered) => quotes.extend(answered),
                    // One request failed for every symbol in it, so each of
                    // them carries that failure.
                    Err(e) => {
                        let message = format!("yahoo quote for {} failed: {e}", chunk.join(", "));
                        for &symbol in chunk {
                            failures.insert(symbol.to_string(), message.clone());
                        }
                    }
                }
            }

            symbols
                .into_iter()
                .map(|s| {
                    // A market that never reached the provider keeps its own
                    // failure; one whose request failed gets that request's.
                    let symbol = s?;
                    match failures.get(symbol.as_str()) {
                        Some(message) => Err(message.clone()),
                        None => yahoo_quote_named(&quotes, &symbol),
                    }
                })
                .collect()
        })
    }
}

/// How many symbols one quote request carries. Yahoo accepts a long list, but
/// not an unbounded one (a request past its limit — or past the URL length
/// the list is spelled into — fails as a whole), and a whole-portfolio fetch
/// is the caller here. Chunking bounds what a single over-long request could
/// cost: the failure is confined to its own chunk's symbols rather than
/// leaving every holding unvalued, which is the one way batching could have
/// come out worse than the per-listing loop it replaced. Well above any
/// portfolio this is built for, so in practice it stays a single request.
const QUOTE_BATCH_SYMBOLS: usize = 50;

/// The quote for `symbol` within a provider answer, as a [`LatestQuote`].
///
/// Found **by symbol**, never by position: Yahoo answers a batch in an order
/// of its own and simply omits a symbol it cannot serve, so position carries
/// no meaning across a multi-symbol request. Matched case-insensitively
/// because the crate canonicalises what it parses to uppercase, while the
/// symbol asked for came from a listing's own (already uppercase) ticker —
/// this way nothing depends on those two conventions staying in step.
pub(super) fn yahoo_quote_named(
    quotes: &[yfinance_rs::Quote],
    symbol: &str,
) -> Result<LatestQuote, String> {
    let quote = quotes
        .iter()
        .find(|q| q.instrument.symbol.as_str().eq_ignore_ascii_case(symbol))
        .ok_or_else(|| format!("yahoo returned no quote for {symbol}"))?;
    let price = quote
        .price
        .as_ref()
        .ok_or_else(|| format!("yahoo quote for {symbol} carries no price"))?;
    let as_of = quote
        .as_of
        .ok_or_else(|| format!("yahoo quote for {symbol} carries no timestamp"))?;
    Ok(LatestQuote {
        price: clean_price(*price.as_decimal()),
        currency: quote.currency.to_string(),
        as_of,
    })
}

/// Midnight at the start of `date` in `tz`, as a UTC instant (a DST gap at
/// midnight resolves to the earliest valid time).
pub(crate) fn local_midnight_utc(date: NaiveDate, tz: Tz) -> Result<DateTime<Utc>, String> {
    date.and_hms_opt(0, 0, 0)
        .and_then(|dt| dt.and_local_timezone(tz).earliest())
        .map(|dt| dt.with_timezone(&Utc))
        .ok_or_else(|| format!("cannot resolve midnight {date} in {tz}"))
}
