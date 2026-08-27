//! Market context: a listing plus the exchange-calendar data price collection
//! needs (timezone, close time, holidays).
//!
//! A listing that has been renamed traded under a different ticker — and, for
//! an exchange-code change, on a different calendar — before the rename took
//! effect (`listing_renames`, `domain::listing_identity`). So a market is not
//! one exchange but a *timeline* of identities: resolving a historical date
//! against today's identity asks the provider for a symbol that was never
//! quoted then, and walks a calendar that was not in force. `exchange` is None
//! exactly for an exchange-less (Crypto) span.

use crate::entities::{exchange, listing};
use chrono::{DateTime, Datelike, Duration, NaiveDate, NaiveTime, Utc};
use chrono_tz::Tz;
use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;
use std::collections::{HashMap, HashSet};

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
    /// `pub(crate)` rather than `pub(super)`: the distribution calendar's own
    /// Yahoo adapter asks the same question of the same identity, and the
    /// answer must be the one this module gives (see
    /// `entities::distribution_event::yahoo`).
    pub(crate) fn tz(&self) -> Result<Tz, String> {
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
    pub(super) fn is_trading_day(&self, date: NaiveDate) -> bool {
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
    let mut conn = pool.acquire().await?;
    load_market_on(&mut conn, listing_id).await
}

/// [`load_market`] on a caller's own connection, so a write path can read the
/// calendar **inside its own transaction** — the trade and Sell write paths'
/// non-trading-day refusal ([`db_non_trading_day`]) does exactly that, the way
/// `trade::db::listing_currency_mismatch` reads the listing's currency there.
pub(crate) async fn load_market_on(
    conn: &mut sqlx::SqliteConnection,
    listing_id: i64,
) -> Result<Option<Market>, sqlx::Error> {
    let Some(listing) = listing::db_get(&mut *conn, listing_id).await? else {
        return Ok(None);
    };
    let renames = crate::domain::listing_identity::RenameHistory::load(&mut *conn).await?;
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
                    exchanges.insert(mic.clone(), exchange::db_get(&mut *conn, mic).await?);
                    holidays.insert(
                        mic.clone(),
                        crate::entities::exchange_holiday::db_holiday_dates_for(&mut *conn, mic)
                            .await?,
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

/// Why a date is not on a listing's trading calendar — the only two ways
/// `MarketIdentity::is_trading_day` can answer no.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NonTradingReason {
    /// A Saturday or a Sunday.
    Weekend,
    /// A weekday seeded in `exchange_holidays` for the exchange in force then.
    Holiday,
}

impl NonTradingReason {
    /// The reason as it reads in a rejection or an alert, e.g. "a Saturday"
    /// or "a public holiday".
    pub(crate) fn describe(self, date: NaiveDate) -> String {
        match self {
            NonTradingReason::Weekend => format!("a {}", date.format("%A")),
            NonTradingReason::Holiday => "a public holiday".to_string(),
        }
    }
}

/// A date that falls outside a listing's own trading calendar, as the calendar
/// stood **on that date** (so a listing that changed exchange is judged on the
/// market it actually traded on then, not today's).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NonTradingDay {
    /// The MIC whose calendar was in force on the date.
    pub exchange_mic: String,
    pub reason: NonTradingReason,
}

impl NonTradingDay {
    /// The whole fact as one clause: `2026-05-16 is a Saturday on XASX`.
    pub(crate) fn describe(&self, date: NaiveDate) -> String {
        format!(
            "{date} is {} on {}",
            self.reason.describe(date),
            self.exchange_mic
        )
    }
}

/// Whether `date` was a trading day for `market`, and if not, why. `None`
/// means it was — including for an exchange-less (Crypto) listing, which
/// trades every day, and for a listing whose `exchanges` row is missing (the
/// calendar is unknown, so nothing can be asserted about it).
///
/// One question, one implementation: the closing-price write path
/// ([`validate_complete_trading_day`]), the trade and Sell write paths'
/// refusal, and `reports::health`'s alert all read this same calendar rather
/// than each re-deriving weekends and holidays.
pub(crate) fn non_trading_day(market: &Market, date: NaiveDate) -> Option<NonTradingDay> {
    let identity = market.identity_at(date);
    let exchange = identity.exchange.as_ref()?;
    if identity.is_trading_day(date) {
        return None;
    }
    let reason = match date.weekday() {
        chrono::Weekday::Sat | chrono::Weekday::Sun => NonTradingReason::Weekend,
        _ => NonTradingReason::Holiday,
    };
    Some(NonTradingDay {
        exchange_mic: exchange.mic.clone(),
        reason,
    })
}

/// [`non_trading_day`] for one listing, on the caller's own transaction.
/// `None` for a listing that doesn't exist — the write path that asks is
/// about to meet (or has just met) the foreign-key rejection, which names the
/// missing listing better than a calendar message could.
pub(crate) async fn db_non_trading_day(
    conn: &mut sqlx::SqliteConnection,
    listing_id: i64,
    date: NaiveDate,
) -> Result<Option<NonTradingDay>, sqlx::Error> {
    let Some(market) = load_market_on(conn, listing_id).await? else {
        return Ok(None);
    };
    Ok(non_trading_day(&market, date))
}
