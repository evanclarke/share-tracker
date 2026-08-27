//! On-demand live valuation: latest quote per listing, converted to AUD.
//!
//! What the three price-taking reports (overview, unrealised gains,
//! performance) use when asked for `live: true` — distinct from the stored
//! daily history in `db`, and never written to it.

use super::fetcher::PriceFetcher;
use super::market::{Market, load_market};
use rust_decimal::Decimal;
use sqlx::SqlitePool;
use std::collections::{HashMap, HashSet};

/// One held listing's live valuation: the latest provider quote converted to
/// AUD, with the provider's as-of time. Consumed by the price-dependent
/// reports for current valuation.
#[derive(Debug, Clone)]
pub struct LiveValuation {
    /// Price per unit in AUD (the quote currency converted via the FX rules).
    pub aud_price: Decimal,
    /// The provider's quote timestamp, RFC 3339 UTC.
    pub as_of: String,
    /// The AUD conversion used an earlier month's FX rate because the quote
    /// month's rate is not published yet (`infra::fx::resolve_valuation_rate`):
    /// the valuation is provisional and the reports annotate the row.
    pub fx_provisional: bool,
}

/// Fetch the latest live quote for each listing and convert it to AUD. Returns
/// one entry per listing id: `Ok` with the AUD price + as-of time, or
/// `Err(reason)` when the fetch, a currency mismatch, or the AUD conversion
/// failed — the caller surfaces the reason per holding and leaves it unvalued
/// (never a silent zero, per the never-silent-zero rule).
pub async fn fetch_live_aud_prices(
    pool: &SqlitePool,
    fetcher: &dyn PriceFetcher,
    listing_ids: &[i64],
) -> Result<HashMap<i64, Result<LiveValuation, String>>, sqlx::Error> {
    let mut out = HashMap::new();
    // Resolve every market before quoting any of them, so the provider is
    // asked **once** for the whole portfolio rather than once per holding
    // (`PriceFetcher::latest_quotes`). A listing that has since been deleted
    // is answered from here and never occupies a slot in the request.
    let mut quoted_ids = Vec::with_capacity(listing_ids.len());
    let mut markets = Vec::with_capacity(listing_ids.len());
    for &id in listing_ids {
        match load_market(pool, id).await? {
            Some(market) => {
                quoted_ids.push(id);
                markets.push(market);
            }
            None => {
                out.insert(id, Err(format!("listing {id} no longer exists")));
            }
        }
    }
    if markets.is_empty() {
        return Ok(out);
    }
    let borrowed: Vec<&Market> = markets.iter().collect();
    let mut quotes = fetcher.latest_quotes(&borrowed).await;
    // One result per market is the trait's contract; hold a misbehaving
    // fetcher to it here rather than letting `zip` drop the tail, which would
    // leave a held listing missing from the valuation with nothing said.
    quotes.resize_with(markets.len(), || {
        Err("price source returned no result for this listing".to_string())
    });
    for ((id, market), quote) in quoted_ids.into_iter().zip(&markets).zip(quotes) {
        let result = match quote {
            Err(e) => Err(e),
            Ok(quote) if quote.currency != market.listing.currency => Err(format!(
                "currency mismatch: provider quoted {}, listing is {}",
                quote.currency, market.listing.currency
            )),
            Ok(quote) => {
                // Convert the quote-currency price to AUD at the valuation
                // rate for the quote's month: the ATO monthly rate when
                // published, else the bounded earlier-month fallback flagged
                // provisional on the row (early in a month the rate cannot
                // exist yet — a flagged valuation beats an unvalued holding).
                // A gap beyond the fallback bound is surfaced as the row's
                // reason, never a silent or zeroed value.
                match crate::infra::fx::resolve_valuation_rate(
                    pool,
                    &quote.currency,
                    quote.as_of.date_naive(),
                )
                .await
                {
                    Ok(vr) => Ok(LiveValuation {
                        aud_price: crate::infra::fx::apply_rate(quote.price, vr.rate),
                        as_of: quote.as_of.to_rfc3339(),
                        fx_provisional: vr.provisional,
                    }),
                    Err(e) => Err(e.to_string()),
                }
            }
        };
        out.insert(id, result);
    }
    Ok(out)
}

/// Resolve live AUD prices for the price-dependent report handlers: when `live`
/// is set, fetch the latest quote for every listing in `listing_ids` that has
/// no explicit override (an explicit price always wins, so it is never
/// fetched). Off, or with no fetcher available, yields an empty map (no live
/// valuation). A live request with no fetcher marks each listing unavailable
/// rather than silently dropping the as-of contract.
pub async fn resolve_live_prices(
    pool: &SqlitePool,
    fetcher: Option<&dyn PriceFetcher>,
    live: bool,
    overrides: &HashMap<i64, Decimal>,
    listing_ids: impl IntoIterator<Item = i64>,
) -> Result<HashMap<i64, Result<LiveValuation, String>>, sqlx::Error> {
    if !live {
        return Ok(HashMap::new());
    }
    let ids: Vec<i64> = listing_ids
        .into_iter()
        .filter(|id| !overrides.contains_key(id))
        .collect::<HashSet<_>>()
        .into_iter()
        .collect();
    match fetcher {
        Some(fetcher) => fetch_live_aud_prices(pool, fetcher, &ids).await,
        None => Ok(ids
            .into_iter()
            .map(|id| (id, Err("live price source unavailable".to_string())))
            .collect()),
    }
}
