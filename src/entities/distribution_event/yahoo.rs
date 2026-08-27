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
//! date that may be wrong or silently dropped — but only if it was in scope to
//! begin with, the request being wider than the window the job asked about
//! (see [`place`]).

use super::{DistributionFetcher, DistributionFuture, FetchedDistribution, FetchedDistributions};
use crate::entities::closing_price::{self, Market};
use chrono::{Duration, NaiveDate};
use rust_decimal::Decimal;
use std::collections::HashMap;

/// The most re-reading a date in another timezone can move it: a UTC calendar
/// date and the exchange-local date of the same instant differ by at most one
/// day, in either direction.
///
/// [`place`] uses it as the slack on its scope test — the only bound available
/// there, since an event that cannot be candle-joined has nothing but the
/// provider's own date to be judged on.
const MAX_TIMEZONE_DATE_SHIFT_DAYS: i64 = 1;

/// Place each provider-dated action on the market's own calendar, keeping the
/// ones whose recovered ex-date lands in the requested `from..=to`.
///
/// Separated from the `fetch_full()` call and left pure so the two rules that
/// decide what a run reports — **scope first, then datability** — can be
/// exercised without a provider. `actions` carries the provider's own
/// (UTC-collapsed) date, the cleaned per-unit amount and the quote currency;
/// `local_by_utc` maps a candle's UTC date to the exchange-local trading day
/// it is.
fn place(
    actions: impl IntoIterator<Item = (NaiveDate, Decimal, String)>,
    local_by_utc: &HashMap<NaiveDate, NaiveDate>,
    from: NaiveDate,
    to: NaiveDate,
) -> FetchedDistributions {
    let slack = Duration::days(MAX_TIMEZONE_DATE_SHIFT_DAYS);
    let mut out = FetchedDistributions::default();
    for (provider_date, amount_per_unit, currency) in actions {
        // **Scope before datability.** The request is widened by
        // `CANDLE_JOIN_MARGIN_DAYS` at both ends so a boundary event can find
        // its own candle, which means the response also carries actions from
        // outside the window the job actually asked about. One of *those* that
        // cannot be candle-joined must not be reported as work left undone —
        // it was never in scope, and the run would be qualified with a note
        // about a distribution nobody asked for. Judged on the provider's own
        // date with a day of slack either way, that being the most the
        // recovery below can move it.
        if provider_date + slack < from || provider_date - slack > to {
            continue;
        }
        let Some(&ex_date) = local_by_utc.get(&provider_date) else {
            out.undatable.push(provider_date);
            continue;
        };
        if ex_date < from || ex_date > to {
            continue;
        }
        out.events.push(FetchedDistribution {
            ex_date,
            amount_per_unit,
            currency,
        });
    }
    out.events.sort_by_key(|e| e.ex_date);
    out.undatable.sort_unstable();
    out
}

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

            let dividends = response.actions.iter().filter_map(|action| match action {
                yfinance_rs::core::Action::Dividend { date, amount } => Some((
                    *date,
                    // Yahoo serves figures as float32-precision binary floats,
                    // so a per-unit amount carries the same noise a price does.
                    closing_price::clean_price(amount.amount()),
                    amount.currency().to_string(),
                )),
                _ => None,
            });
            Ok(place(dividends, &local_by_utc, from, to))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{dec, ymd};

    /// The join table a response would produce: each candle's UTC date to the
    /// exchange-local trading day it is. The AEDT case from the module docs —
    /// the UTC date is the day before the ex-date.
    fn candles(days: &[(NaiveDate, NaiveDate)]) -> HashMap<NaiveDate, NaiveDate> {
        days.iter().copied().collect()
    }

    #[test]
    fn an_event_is_dated_by_its_own_candle_and_kept_if_it_lands_in_the_window() {
        let joined = candles(&[(ymd(2024, 12, 31), ymd(2025, 1, 1))]);
        let out = place(
            [(ymd(2024, 12, 31), dec("0.0187"), "AUD".to_string())],
            &joined,
            ymd(2024, 1, 1),
            ymd(2025, 6, 30),
        );
        assert_eq!(out.events.len(), 1);
        assert_eq!(out.events[0].ex_date, ymd(2025, 1, 1));
        assert!(out.undatable.is_empty());
    }

    /// An event the response carries only because of the candle-join margin,
    /// and which cannot be joined, is **out of scope** rather than undone
    /// work. Reported, it would qualify a run with a note about a distribution
    /// nobody asked about — and the window test below the join could never
    /// reach it, because an undatable event never gets that far.
    #[test]
    fn an_unjoinable_event_outside_the_requested_window_is_not_reported() {
        let out = place(
            // Four days before the window opens: inside the five-day margin
            // the request was widened by, outside what the job asked for.
            [(ymd(2023, 12, 28), dec("0.10"), "AUD".to_string())],
            &candles(&[]),
            ymd(2024, 1, 1),
            ymd(2024, 12, 31),
        );
        assert!(out.events.is_empty());
        assert!(
            out.undatable.is_empty(),
            "it was never in scope: {:?}",
            out.undatable
        );
    }

    /// The control: an unjoinable event that **is** in scope is still counted,
    /// which is what stops "the provider knows of no ex-date" quietly covering
    /// a case where it did.
    #[test]
    fn an_unjoinable_event_inside_the_window_is_still_reported() {
        let out = place(
            [(ymd(2024, 6, 30), dec("0.10"), "AUD".to_string())],
            &candles(&[]),
            ymd(2024, 1, 1),
            ymd(2024, 12, 31),
        );
        assert!(out.events.is_empty());
        assert_eq!(out.undatable, vec![ymd(2024, 6, 30)]);
    }

    /// The slack on the scope test is a day either way, because that is how
    /// far recovering the date can move it: an event stamped the day before
    /// the window opens can still be dated into it.
    #[test]
    fn an_event_one_day_outside_the_window_is_still_in_scope() {
        let boundary = ymd(2023, 12, 31);
        assert_eq!(
            place(
                [(boundary, dec("0.10"), "AUD".to_string())],
                &candles(&[]),
                ymd(2024, 1, 1),
                ymd(2024, 12, 31),
            )
            .undatable,
            vec![boundary]
        );
    }

    /// A joinable event whose recovered ex-date falls outside the window is
    /// dropped silently — the margin widens what can be *placed*, never what
    /// is stored.
    #[test]
    fn a_joinable_event_dated_outside_the_window_is_dropped_silently() {
        let out = place(
            [(ymd(2023, 12, 31), dec("0.10"), "AUD".to_string())],
            &candles(&[(ymd(2023, 12, 31), ymd(2023, 12, 31))]),
            ymd(2024, 1, 1),
            ymd(2024, 12, 31),
        );
        assert!(out.events.is_empty());
        assert!(out.undatable.is_empty());
    }
}
