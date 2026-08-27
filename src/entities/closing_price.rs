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
//! - A fetched row records the **provider symbol it was fetched under**
//!   ([`ClosingPrice::fetched_symbol`], migration 0038) — always, not only
//!   when it differs from the symbol the rename chain derives, so the question
//!   "what symbol produced this row?" has one answer rather than two readings
//!   of a null. It comes from the fetcher itself ([`PriceFetcher::symbol`]),
//!   so it is always in the namespace of the `source` beside it. A manual row
//!   carries none (nothing was fetched), and a row stored before 0038 carries
//!   none either — the symbol is not recoverable after the fact, and nothing
//!   invents one.
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
//! - **A stored price is in its own trading day's unit basis** — the price the
//!   security actually traded at on `price_date`. The provider does not serve
//!   it that way: Yahoo restates a security's whole close series into the
//!   *current* basis the moment it splits (`auto_adjust(false)` turns off
//!   dividend adjustment only), so after a 10-for-1 it answers 120.888 for a
//!   day the security closed at 1208.88. The reports go the other way —
//!   `domain::open_parcels` re-bases parcel quantities into the snapshot
//!   date's own basis — so an unnormalised price was multiplied by units in a
//!   different basis and the product came out by the split ratio (SCENARIOS
//!   Q-14). Which basis a figure arrived in is fixed by *when it was
//!   observed*, and `fetched_at` dates that, so the row keeps the figure as
//!   observed and derives the stored one:
//!
//!       price = price_as_observed × the price re-basing ratio
//!                                   over (price_date, fetched_at]
//!
//!   Every restatement is therefore a recompute from the observation rather
//!   than a delta applied to an already-adjusted number
//!   ([`db_rebase_listing_prices`]): recording, editing or deleting a price
//!   re-basing action re-derives the same answer in any order, and a series
//!   collected day by day *before* one is left alone by it (its fetches
//!   predate the event, so its ratio is 1). The recovered figure carries only
//!   the provider's ~7 significant digits — see [`clean_price`] — so a
//!   re-fetch is no longer byte-identical to the provider's response.
//!
//! - **Which corporate actions restate the price series** — the set is a
//!   strict *superset* of the actions that re-base quantities
//!   (`corporate_action::adjustments`, whose module docs carry the same
//!   statement from the other side, and whose separate
//!   [`PriceBasisEvent`](crate::entities::corporate_action::PriceBasisEvent)
//!   type keeps the two apart at every call site):
//!
//!   - `ShareSplit` / `BonusIssue` restate it, by the same ratio they multiply
//!     the unit count by.
//!   - A **`Demerger`** restates it too — the provider applies a spin-off
//!     price-adjustment factor to the whole pre-demerger series exactly as it
//!     does for a split — while changing **no unit count** on this listing (it
//!     issues units of a *different* one). So there is no ratio to read: the
//!     factor is derived from the close the operator states the security
//!     actually traded at on the last pre-demerger trading day
//!     (`demerger_close_date` / `demerger_close_price`) divided by the
//!     provider's own adjusted figure for that same day
//!     ([`db_price_basis_events`]). Both sides are kept as facts and the
//!     quotient is computed at re-base time, so the close can be stated before
//!     the history is backfilled and re-derives itself if it is re-fetched.
//!     A demerger with no stated close restates nothing — its pre-demerger
//!     prices stay as the provider served them, which `GET /reports/health`
//!     reports as `demergers_missing_close`.
//!   - `ScripForScrip` and `WorthlessShares` do **not**: both end the original
//!     ticker, so the provider stops serving a series rather than restating
//!     one (the `listings.unpriced_from` case).
//!   - `ReturnOfCapital`, `RightsIssue` and `BuyBack` do **not**: a
//!     distribution goes through the provider's dividend-adjustment channel,
//!     which `auto_adjust(false)` turns off, and neither of the other two is
//!     in the provider's adjustment set at all.
//!
//!   The derived demerger factor is a `Decimal` division, so the recovered
//!   figures carry no more than the ~7 significant digits the provider gave
//!   (see [`clean_price`], which holds them to exactly that) *and* whatever
//!   the division itself rounds off — the price is recovered to about the
//!   accuracy of the stated close, not exactly.
//! - A failed fetch is stored as an errored row for that (listing, date) —
//!   never a silent zero or a skipped row — and is replaced by a later
//!   successful re-run.
//! - Only an **errored** row is deletable ([`db_delete`]): the acknowledgement
//!   that no price will ever exist for that day. An ok row is replaced by a
//!   re-fetch, never removed, so no valuation can lose a price it once had.
//!   The one relaxation, and its whole justification: a date inside the
//!   listing's `unpriced_before` span is **by declaration not valued at all**
//!   — the marker supersedes every stored row for the span and
//!   `reports::valuation` excludes the holding there rather than pricing it
//!   (migration 0037), and the carry-forward query is floored at the marker
//!   too — so there is no valued series to punch a hole in, and deleting is
//!   the acknowledgement that the stored figure never was a valuation. The
//!   span is the only place an ok row may be deleted, one date at a time or
//!   all at once ([`db_clear_unpriced_before`]). Note the asymmetry with
//!   `unpriced_from`: a date on or after **that** marker *is* valued — the
//!   last stored ok close is carried forward into it — so nothing is relaxed
//!   at that end.
//! - A day the provider cannot serve at all can be priced **by hand**
//!   (`PUT /closing_prices/{listing_id}/{price_date}`), recorded with where
//!   the figure was sourced from and why manual entry was needed
//!   ([`PriceOrigin::Manual`]). Valuation reads such a row exactly like a
//!   fetched one. The provider never takes the day back: collection and
//!   backfill skip it as an ok row, and an explicit re-fetch is refused — a
//!   manual price is changed only by entering another one. It is also
//!   contemporaneous **by declaration** — the operator states what the
//!   security traded at that day — so it is neither normalised on entry nor
//!   ever re-based: nothing rewrites a figure a person typed.
//!
//! # Layout
//!
//! Split into focused units, in the order a price travels through them: the
//! stored row and its enums (`model`), the listing-plus-calendar context every
//! dated question is asked of (`market`), the provider abstraction and the
//! pieces built on it (`fetcher`) with the live Yahoo implementation beside it
//! (`yahoo`), persistence (`db`), the held-days timeline collection is scoped
//! to (`held`), the fetch-and-store path the scheduled job / manual re-fetch /
//! backfill all share (`collection`), on-demand live valuation (`live`), and
//! the HTTP handlers and router (`http`). Everything is re-exported here, so
//! the module's surface is unchanged from when it was one file — and the tests
//! below, which predate the split, are the behaviour lock proving it.

mod collection;
mod db;
mod fetcher;
mod held;
mod http;
mod live;
mod market;
mod model;
mod yahoo;

pub use collection::{COLLECTION_LOOKBACK_DAYS, run_collection};
pub use db::{db_get_one, db_latest_ok_price_on_or_before, db_rebase_listing_prices, run_rebase};
/// Shared with the distribution calendar for the same reason: Yahoo serves
/// every figure as a float32-precision binary float, and a per-unit
/// distribution arrives with the same noise a price does.
pub(crate) use fetcher::clean_price;
pub use fetcher::{CachingFetcher, SharedFetcher};
pub use held::{HeldTimeline, db_held_listing_ids, db_held_listing_ids_on};
pub use http::router;
pub use live::resolve_live_prices;
pub use market::{Market, NonTradingReason, load_market};
/// Asked of a market on the caller's own connection: the trade and sell write
/// paths reject a trade dated on a day its exchange was shut, and the
/// settlement-coverage, health and valuation reports walk the same calendar.
pub(crate) use market::{db_non_trading_day, load_market_on, non_trading_day};
pub use model::{PriceOrigin, PriceStatus};
pub use yahoo::YahooFetcher;
/// The two provider-mapping pieces the **distribution calendar's** own Yahoo
/// adapter shares with this one (`entities::distribution_event::yahoo`): the
/// listing→Yahoo-symbol resolution, and the exchange-local window a dated
/// request is expressed in. Shared rather than copied because both adapters
/// must ask Yahoo for the same security over the same days — a second spelling
/// of either would be a silent divergence between a listing's prices and its
/// distributions.
pub(crate) use yahoo::{local_midnight_utc, yahoo_symbol};

// Everything below is reached by name only from `#[cfg(test)]` code — the
// tests in this file, and the report/web test modules that drive the price
// paths. Each is part of the module's surface all the same (a non-test caller
// reaches these types through the signatures above without naming them), so
// the re-export is gated rather than dropped: an ungated one would warn in the
// non-test build, which `cargo build` is required to be free of.
/// The fetch-and-store path itself: the tests drive it directly as well as
/// through the routes that call it.
#[cfg(test)]
use collection::fetch_and_store;
/// The row-level store, named by `test_support`'s closing-price fixture as
/// well as by the tests here.
#[cfg(test)]
pub(crate) use db::db_store;
#[cfg(test)]
pub use db::{ClearOutcome, db_clear_unpriced_before, db_list};
#[cfg(test)]
pub use fetcher::{
    FetchError, FetchFuture, FetchedClose, LatestQuote, PriceFetcher, QuoteFuture, QuotesFuture,
};
#[cfg(test)]
pub use http::{BackfillSummary, ClearSummary};
#[cfg(test)]
pub use live::fetch_live_aud_prices;
#[cfg(test)]
pub use model::{ClosingPrice, MANUAL_SOURCE, UNASSIGNED_ID};
/// Reached by name only from tests: the symbol resolution the live fetcher
/// does (so a stub's stored `fetched_symbol` is the symbol a real fetch would
/// have recorded), its provider-failure classification, and the by-symbol
/// reading of a batch answer.
#[cfg(test)]
use yahoo::{classify_yahoo_failure, yahoo_quote_named, yahoo_symbol_now};

// The test modules below were written against the single file's imports; these
// keep them compiling unchanged, which is what makes them a behaviour lock on
// the split rather than a rewrite of it.
#[cfg(test)]
use crate::entities::listing;
#[cfg(test)]
use axum::Extension;
#[cfg(test)]
use chrono::{DateTime, Duration, NaiveDate, Utc};
#[cfg(test)]
use rust_decimal::Decimal;
#[cfg(test)]
use sqlx::SqlitePool;
#[cfg(test)]
use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
};

/// Reusable price-fetcher stub for the report tests — see the module.
#[cfg(test)]
pub mod test_support;

#[cfg(test)]
mod tests;
