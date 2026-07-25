//! The stored-price valuation path: resolving every held listing's AUD price
//! at a date from its stored closing price, converted at the valuation FX
//! rate (`infra::fx::resolve_valuation_rate`).
//!
//! This is the one place "final stored price, valuation-month rate (or a
//! flagged fallback), fail loudly on any blocker" is implemented — report
//! snapshot generation (`reports::snapshot`) and the period performance
//! report (`reports::period_performance`) both value through it, so a
//! valuation-day or FX-fallback rule only needs to change in one place.

use chrono::{DateTime, NaiveDate, Utc};
use rust_decimal::Decimal;
use sqlx::SqlitePool;

use crate::entities::closing_price::{self, Market, PriceStatus};
use crate::infra::fx::FxRates;

/// Why valuation could not proceed for one or more held listings.
/// `Unprocessable` carries the joined per-listing blockers (missing/errored
/// price, a close not final yet, no FX rate available) and maps to HTTP 422.
#[derive(Debug)]
pub enum ValuationError {
    Unprocessable(String),
    Db(String),
}

impl std::fmt::Display for ValuationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ValuationError::Unprocessable(msg) | ValuationError::Db(msg) => write!(f, "{msg}"),
        }
    }
}

impl From<sqlx::Error> for ValuationError {
    fn from(e: sqlx::Error) -> Self {
        ValuationError::Db(e.to_string())
    }
}

/// One listing's resolved valuation at a date: its stored native-currency
/// close, the FX rate used to convert it, and the AUD result.
#[derive(Debug, Clone)]
pub struct ListingValuation {
    pub listing_id: i64,
    pub currency: String,
    /// The stored closing price, in the listing's quote currency (not AUD).
    pub native_price: Decimal,
    /// Foreign currency units per 1 AUD, at the valuation FX rate.
    pub rate: Decimal,
    /// The valuation month's ATO rate wasn't imported yet — a fallback
    /// earlier month's rate was substituted
    /// (`infra::fx::resolve_valuation_rate`).
    pub provisional: bool,
    /// `native_price` converted to AUD via `rate`.
    pub aud_price: Decimal,
}

/// The market contexts of the listings held as at `date` (live holdings when
/// `None`).
pub async fn held_markets(
    pool: &SqlitePool,
    as_of: Option<NaiveDate>,
) -> Result<Vec<Market>, ValuationError> {
    let ids = closing_price::db_held_listing_ids(pool, as_of).await?;
    let mut markets = Vec::with_capacity(ids.len());
    for id in ids {
        markets.push(
            closing_price::load_market(pool, id)
                .await?
                .ok_or_else(|| ValuationError::Db(format!("listing {id} disappeared")))?,
        );
    }
    Ok(markets)
}

/// Resolve every held listing's AUD valuation at `date` from stored closing
/// prices: each listing is valued at its nearest trading day on or before
/// `date`, whose stored price must be ok and final, converted at the
/// valuation FX rate. Fails with the full list of blockers otherwise — a
/// partly-priced day yields no result at all, never a partial one.
pub async fn stored_valuations(
    pool: &SqlitePool,
    date: NaiveDate,
    now: DateTime<Utc>,
) -> Result<Vec<ListingValuation>, ValuationError> {
    let markets = held_markets(pool, Some(date)).await?;
    if markets.is_empty() {
        return Err(ValuationError::Unprocessable(format!(
            "nothing was held on {date}"
        )));
    }

    let mut valuations = Vec::with_capacity(markets.len());
    let mut blockers: Vec<String> = Vec::new();
    // every imported ATO FX rate — per-listing conversions below are map
    // lookups, not one DB round-trip each
    let fx = FxRates::load(pool).await?;
    for market in &markets {
        let ticker = &market.listing.ticker;
        let Some(valuation_day) = market.latest_trading_day_on_or_before(date) else {
            blockers.push(format!(
                "{ticker}: no trading day in the year before {date}"
            ));
            continue;
        };
        let final_day = market
            .latest_complete_trading_day(now)
            .map_err(ValuationError::Db)?;
        if final_day.is_none_or(|f| valuation_day > f) {
            blockers.push(format!(
                "{ticker}: the close of {valuation_day} is not final yet"
            ));
            continue;
        }
        match closing_price::db_get_one(pool, market.listing.id, valuation_day).await? {
            Some(row) if row.status == PriceStatus::Ok => {
                let native_price = row.price.expect("ok row carries a price (schema CHECK)");
                match fx.resolve_valuation_rate(&market.listing.currency, valuation_day) {
                    Ok(vr) => valuations.push(ListingValuation {
                        listing_id: market.listing.id,
                        currency: market.listing.currency.clone(),
                        native_price,
                        rate: vr.rate,
                        provisional: vr.provisional,
                        aud_price: crate::infra::fx::apply_rate(native_price, vr.rate),
                    }),
                    Err(e) => blockers.push(format!("{ticker}: {e}")),
                }
            }
            Some(row) => blockers.push(format!(
                "{ticker}: the stored price for {valuation_day} is errored ({}) — re-fetch it",
                row.error.unwrap_or_default()
            )),
            None => blockers.push(format!(
                "{ticker}: no stored price for {valuation_day} — backfill it"
            )),
        }
    }
    if blockers.is_empty() {
        Ok(valuations)
    } else {
        Err(ValuationError::Unprocessable(blockers.join("; ")))
    }
}
