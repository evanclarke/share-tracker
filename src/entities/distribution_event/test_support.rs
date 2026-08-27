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
use crate::entities::closing_price::Market;
use chrono::NaiveDate;
use rust_decimal::Decimal;
use std::{collections::HashMap, sync::Arc};

/// A canned distribution history, keyed by listing.
#[derive(Default, Clone)]
pub struct DistributionStub {
    events: HashMap<i64, Vec<FetchedDistribution>>,
    undatable: HashMap<i64, Vec<NaiveDate>>,
    /// When set, every call fails with this message — the provider-outage
    /// case, which must never read as "this listing paid nothing".
    failure: Option<String>,
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

    /// A provider that fails every call.
    pub fn failing(message: &str) -> Self {
        Self {
            failure: Some(message.to_string()),
            ..Self::default()
        }
    }

    pub fn shared(self) -> SharedDistributionFetcher {
        Arc::new(self)
    }
}

impl DistributionFetcher for DistributionStub {
    fn source(&self) -> &'static str {
        "stub"
    }

    fn symbol(&self, market: &Market, _date: NaiveDate) -> Result<String, String> {
        Ok(market.listing.ticker.clone())
    }

    fn distributions<'a>(
        &'a self,
        market: &'a Market,
        from: NaiveDate,
        to: NaiveDate,
    ) -> DistributionFuture<'a> {
        let listing_id = market.listing.id;
        Box::pin(async move {
            if let Some(message) = &self.failure {
                return Err(message.clone());
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
