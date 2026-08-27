//! Yahoo Finance, via the `yfinance-rs` crate — the live
//! [`DistributionFetcher`].
//!
//! Everything provider-specific for the distribution calendar is here, so the
//! trait stays the swap point: the symbol mapping (shared with the price
//! fetcher, so a listing's prices and its distributions can never be asked for
//! under two different spellings), the explicit-period request, and — the part
//! that earns this file — the recovery of the real ex-date from a provider
//! date that is not one.
//!
//! # Why the provider's own date cannot be stored
//!
//! `yfinance-rs` converts a corporate action's timestamp with
//! `core::conversions::i64_to_date`, which is
//! `DateTime::from_timestamp(ts, 0).date_naive()` — a **UTC** calendar date —
//! discarding the `chart.meta.exchangeTimezoneName` the same response carries.
//! Yahoo stamps the event at the exchange's session start. For an exchange
//! *behind* UTC (NYSE, Nasdaq) session start is mid-afternoon UTC and the date
//! survives; for the ASX it survives only in **AEST** (UTC+10), and in
//! **AEDT** (UTC+11, October–April) the UTC date is the day *before* the
//! ex-date — where it then routinely lands on a day the market was shut
//! (2025-01-01, 2024-01-01, a Sunday, Easter Monday).
//!
//! Measured 2026-08-27 against issuer-published dates: HNDQ's four January
//! events all one day early and its four July events all exact (Betashares),
//! BHP's March 2025 interim one day early and its September 2025 final exact
//! (BHP's own notices), VDHG consistent with both. The two offsets bracket the
//! stamp hour in [10:00, 11:00) exchange-local, which is ASX open.
//!
//! # The correction: join the event to its own candle
//!
//! `Action::Dividend` carries only the collapsed `NaiveDate`, so the instant
//! is unrecoverable from the action — but `fetch_full()` returns the candles
//! *and* the actions from one response, and `Candle::ts` is a full
//! `DateTime<Utc>`. Daily candles are stamped at session start too, which is
//! what `closing_price::yahoo`'s `daily_closes` already relies on when it
//! reads a trading day as `c.ts.with_timezone(&tz).date_naive()`. So the event
//! whose UTC date is `D` belongs to the candle whose UTC `ts` date is `D`, and
//! **that candle's exchange-local date is the ex-date**.
//!
//! This assumes nothing about the stamp hour — both sides share one convention,
//! whatever it is — which is why it is preferred over adding a timezone offset
//! back by hand. Verified 10 of 10 against issuer-published dates.
//!
//! An event with no candle sharing its UTC date cannot be placed, and is
//! reported ([`FetchedDistributions::undatable`]) rather than stored under a
//! date that may be wrong or silently dropped.

use super::{DistributionFetcher, DistributionFuture, FetchedDistribution, FetchedDistributions};
use crate::entities::closing_price::{self, Market};
use chrono::{Duration, NaiveDate};
use std::collections::HashMap;

/// Yahoo Finance, via the `yfinance-rs` crate.
///
/// Its own client rather than the price fetcher's: the two are injected
/// separately (the price fetcher reaches the router as well as the scheduler,
/// this one only the scheduler), and a `YfClient` is a thin handle over its own
/// cookie/crumb state.
#[derive(Default)]
pub struct YahooDistributionFetcher {
    client: yfinance_rs::YfClient,
}

impl DistributionFetcher for YahooDistributionFetcher {
    fn source(&self) -> &'static str {
        "yahoo"
    }

    fn symbol(&self, market: &Market, date: NaiveDate) -> Result<String, String> {
        closing_price::yahoo_symbol(market, date)
    }

    fn distributions<'a>(
        &'a self,
        market: &'a Market,
        from: NaiveDate,
        to: NaiveDate,
    ) -> DistributionFuture<'a> {
        Box::pin(async move {
            let symbol = self.symbol(market, from)?;
            let tz = market.identity_at(from).tz()?;
            // Widened by the candle-join margin at both ends so a boundary
            // event's own candle is inside the window; the events are filtered
            // back to `from..=to` once they are dated.
            let margin = Duration::days(super::CANDLE_JOIN_MARGIN_DAYS);
            let start = closing_price::local_midnight_utc(from - margin, tz)?;
            let end = closing_price::local_midnight_utc(to + margin + Duration::days(1), tz)?;
            // An explicit period, never `Range::Max`, which silently truncates
            // the action stream (see the module docs on the entity).
            // `auto_adjust(false)` matches the price fetcher: nothing here
            // wants the provider's own adjustment applied on top.
            let response = yfinance_rs::HistoryBuilder::new(&self.client, &symbol)
                .between(start, end)
                .interval(yfinance_rs::Interval::D1)
                .auto_adjust(false)
                .actions(true)
                .fetch_full()
                .await
                // Classified from the provider's own **typed** error, not by
                // matching words in a message — the same call
                // `closing_price::yahoo` makes, so a retired ticker is
                // diagnosed identically on both paths.
                .map_err(|e| closing_price::classify_yahoo_failure(&symbol, e))?;

            // The join table: the UTC date of each candle's own instant, to
            // the exchange-local trading day it is.
            let local_by_utc: HashMap<NaiveDate, NaiveDate> = response
                .candles
                .iter()
                .map(|c| (c.ts.date_naive(), c.ts.with_timezone(&tz).date_naive()))
                .collect();

            let mut out = FetchedDistributions::default();
            for action in &response.actions {
                let yfinance_rs::core::Action::Dividend { date, amount } = action else {
                    continue;
                };
                let Some(&ex_date) = local_by_utc.get(date) else {
                    out.undatable.push(*date);
                    continue;
                };
                if ex_date < from || ex_date > to {
                    continue;
                }
                out.events.push(FetchedDistribution {
                    ex_date,
                    // Yahoo serves figures as float32-precision binary floats,
                    // so a per-unit amount carries the same noise a price does.
                    amount_per_unit: closing_price::clean_price(amount.amount()),
                    currency: amount.currency().to_string(),
                });
            }
            out.events.sort_by_key(|e| e.ex_date);
            out.undatable.sort_unstable();
            Ok(out)
        })
    }
}
