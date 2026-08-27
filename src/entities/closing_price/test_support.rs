//! Reusable price-fetcher stub for the report tests (the daily-close tests
//! in this entity's own `tests` module use their own richer stub). Returns a
//! canned latest quote per listing, or a blanket failure for every listing.

use super::*;

#[derive(Default)]
pub struct QuoteStub {
    quotes: HashMap<i64, LatestQuote>,
    /// Daily closes keyed by **provider symbol**, so a stub can model a
    /// provider that serves a security's history only under the symbol it
    /// was quoted as at the time — the shape a rename produces.
    closes: HashMap<String, Vec<FetchedClose>>,
    /// A blanket failure, classified the way a provider adapter classifies
    /// its own ([`FetchError`]) — so a stub can model "the provider says
    /// it has no such symbol" separately from "the provider is down".
    fail: Option<FetchError>,
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
            fail: Some(FetchError::Other(msg.to_string())),
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
            if let Some(failure) = &self.fail {
                return Err(failure.clone());
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
            if let Some(failure) = &self.fail {
                return Err(failure.message().to_string());
            }
            self.quotes
                .get(&market.listing.id)
                .cloned()
                .ok_or_else(|| format!("no stub quote for listing {}", market.listing.id))
        })
    }
}
