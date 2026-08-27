//! An offline [`DistributionFetcher`] for tests — the distribution-calendar
//! counterpart of `closing_price::test_support::QuoteStub`.
//!
//! Every router built by `test_support::ApiClient::full` and every test job
//! registry gets one of these, so no test path can reach the network: the
//! default stub knows of no distribution at all, which is also the state a
//! database starts in.

use super::{
    DistributionFetcher, DistributionFuture, FetchedDistribution, FetchedDistributions,
    SharedDistributionFetcher,
};
use crate::entities::closing_price::{FetchError, Market};
use chrono::NaiveDate;
use rust_decimal::Decimal;
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};

/// A canned distribution history, keyed by listing.
#[derive(Default, Clone)]
pub struct DistributionStub {
    events: HashMap<i64, Vec<FetchedDistribution>>,
    undatable: HashMap<i64, Vec<NaiveDate>>,
    /// When set, every call fails this way — the provider-outage case, which
    /// must never read as "this listing paid nothing", and its opposite, the
    /// retired ticker, which must never fail the run forever.
    failure: Option<FetchError>,
    /// When set, the provider serves its series **only** under this symbol and
    /// answers [`FetchError::NoSuchSymbol`] for any other — the retired-ticker
    /// half of a rename, as Yahoo actually behaves for LAAC/LAR. What makes a
    /// call's symbol observable at all, since a stub that answers whatever it
    /// is asked cannot tell a segmented fetch from an unsegmented one.
    only_symbol: Option<String>,
    /// Every `(symbol, from, to)` the stub was asked for, in call order.
    calls: Arc<Mutex<Vec<(String, NaiveDate, NaiveDate)>>>,
}

impl DistributionStub {
    /// One distribution the provider will report for `listing_id`. Chainable.
    pub fn with_event(
        mut self,
        listing_id: i64,
        ex_date: NaiveDate,
        amount_per_unit: Decimal,
        currency: &str,
    ) -> Self {
        self.events
            .entry(listing_id)
            .or_default()
            .push(FetchedDistribution {
                ex_date,
                amount_per_unit,
                currency: currency.to_string(),
            });
        self
    }

    /// An event the adapter could not place on the market's calendar — the
    /// candle-join miss, reported rather than stored.
    pub fn with_undatable(mut self, listing_id: i64, provider_date: NaiveDate) -> Self {
        self.undatable
            .entry(listing_id)
            .or_default()
            .push(provider_date);
        self
    }

    /// A provider that fails every call for a reason that carries **no
    /// verdict on the symbol** — an outage, a rate limit, a transport failure.
    pub fn failing(message: &str) -> Self {
        Self {
            failure: Some(FetchError::Other(message.to_string())),
            ..Self::default()
        }
    }

    /// A provider that positively answers that it serves no such series — the
    /// retired ticker, a standing fact rather than a transient failure.
    pub fn retired(message: &str) -> Self {
        Self {
            failure: Some(FetchError::NoSuchSymbol(message.to_string())),
            ..Self::default()
        }
    }

    /// The provider serves its series only under `symbol`; any other symbol
    /// gets the retired-ticker answer. Chainable.
    pub fn serving_only(mut self, symbol: &str) -> Self {
        self.only_symbol = Some(symbol.to_string());
        self
    }

    /// Every `(symbol, from, to)` the stub has been asked for, in call order —
    /// how a test pins that a rename was fetched under both of its tickers.
    pub fn calls(&self) -> Vec<(String, NaiveDate, NaiveDate)> {
        self.calls.lock().expect("stub call log").clone()
    }

    pub fn shared(self) -> SharedDistributionFetcher {
        Arc::new(self)
    }
}

impl DistributionFetcher for DistributionStub {
    fn source(&self) -> &'static str {
        "stub"
    }

    /// The ticker in force on `date`, not the listing's current one — the
    /// same question `closing_price::yahoo_symbol` answers, so a renamed
    /// listing resolves to two different symbols across its held span.
    fn symbol(&self, market: &Market, date: NaiveDate) -> Result<String, String> {
        Ok(market.identity_at(date).ticker.clone())
    }

    fn distributions<'a>(
        &'a self,
        market: &'a Market,
        symbol: &'a str,
        from: NaiveDate,
        to: NaiveDate,
    ) -> DistributionFuture<'a> {
        let listing_id = market.listing.id;
        let symbol = symbol.to_string();
        Box::pin(async move {
            self.calls
                .lock()
                .expect("stub call log")
                .push((symbol.clone(), from, to));
            if let Some(message) = &self.failure {
                return Err(message.clone());
            }
            if self
                .only_symbol
                .as_ref()
                .is_some_and(|only| *only != symbol)
            {
                return Err(FetchError::NoSuchSymbol(format!(
                    "no series served under {symbol}"
                )));
            }
            // Windowed like the real adapter, so a test can pin that the job
            // asks for the held span and nothing outside it.
            let mut events: Vec<FetchedDistribution> = self
                .events
                .get(&listing_id)
                .map_or(&[][..], |v| v)
                .iter()
                .filter(|e| e.ex_date >= from && e.ex_date <= to)
                .cloned()
                .collect();
            events.sort_by_key(|e| e.ex_date);
            Ok(FetchedDistributions {
                events,
                undatable: self.undatable.get(&listing_id).cloned().unwrap_or_default(),
            })
        })
    }
}
