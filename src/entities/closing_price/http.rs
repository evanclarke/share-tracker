//! The HTTP surface: the request bodies, the handlers, and the router.

use super::collection::fetch_and_store;
use super::db::{
    ClearOutcome, db_clear_unpriced_before, db_delete, db_get_one, db_list, db_ok_dates, db_store,
};
use super::fetcher::SharedFetcher;
use super::market::{Market, load_market};
use super::model::{ClosingPrice, MANUAL_SOURCE, PriceOrigin, PriceStatus, UNASSIGNED_ID};
use crate::entities::listing;
use crate::infra::http::ApiError;
use axum::{
    Extension, Json, Router,
    extract::{Path, Query, State},
    http::StatusCode,
    routing::{get, post, put},
};
use chrono::{Duration, NaiveDate, Utc};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ListParams {
    listing_id: Option<i64>,
    from: Option<NaiveDate>,
    to: Option<NaiveDate>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct FetchBody {
    listing_id: i64,
    price_date: NaiveDate,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct BackfillBody {
    listing_id: i64,
    from: NaiveDate,
    to: NaiveDate,
    /// One-off provider symbol for this fetch only (not persisted to
    /// `listings.price_symbol`) — recovers a pre-rename date range under the
    /// old symbol, when the provider no longer serves it under the current
    /// one. Omitted: the listing's stored `price_symbol` (if any) or the
    /// derived mapping, as for any other fetch.
    #[serde(default)]
    symbol: Option<String>,
}

/// A price entered by hand for a day the provider cannot serve, with the
/// provenance that makes the figure auditable later.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ManualPriceBody {
    /// Closing price in the listing's quote currency (never AUD).
    #[serde(deserialize_with = "crate::infra::decimal::strict_decimal")]
    price: Decimal,
    /// Where the figure came from, e.g. "asx.com.au closing report".
    sourced_from: String,
    /// Why manual entry was needed, e.g. "provider serves no candle since the
    /// delisting".
    reason: String,
}

/// Which listing's superseded price rows to clear. No date range: the span
/// is the listing's own `unpriced_before` declaration and nothing else.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ClearBody {
    listing_id: i64,
}

/// What a clear run did, returned to the caller.
#[derive(Debug, Serialize, Deserialize)]
pub struct ClearSummary {
    pub listing_id: i64,
    /// The marker that defined the cleared span, echoed back so the caller
    /// can see what it actually acted on.
    pub unpriced_before: NaiveDate,
    /// Rows removed. Zero on a re-run — the operation is idempotent.
    pub deleted: u64,
}

/// What a backfill run did, returned to the caller.
#[derive(Debug, Serialize, Deserialize)]
pub struct BackfillSummary {
    /// Trading days in the (clamped) range.
    pub trading_days: usize,
    /// Days skipped because an ok price was already stored.
    pub already_stored: usize,
    pub fetched_ok: usize,
    pub errored: usize,
}

async fn list(
    State(pool): State<SqlitePool>,
    Query(params): Query<ListParams>,
) -> Result<Json<Vec<ClosingPrice>>, ApiError> {
    db_list(&pool, params.listing_id, params.from, params.to)
        .await
        .map(Json)
        .map_err(ApiError::from)
}

/// Store a price entered by hand for one (listing, day), with the provenance
/// that makes it auditable: where it was sourced from and why manual entry
/// was needed. This is the way out of a day the provider cannot serve — a
/// delisted or mis-served symbol, or a permanent hole in its series — which
/// `reports::valuation` otherwise blocks forever, taking the day's snapshots
/// with it.
///
/// The day must be a trading day whose close is final, exactly as for a
/// fetch: a price on any other date would never be read by valuation. A
/// manual price may deliberately replace a stored provider price that is
/// wrong; that is an ordinary UPDATE, so the staleness trigger regenerates
/// the snapshots that used the old figure.
async fn put_manual(
    State(pool): State<SqlitePool>,
    Path((listing_id, price_date)): Path<(i64, NaiveDate)>,
    Json(body): Json<ManualPriceBody>,
) -> Result<StatusCode, ApiError> {
    let market = load_market(&pool, listing_id)
        .await
        .map_err(internal)?
        .ok_or_else(|| ApiError::not_found("no such listing"))?;
    validate_complete_trading_day(&market, price_date)?;

    if body.price <= Decimal::ZERO {
        return Err(ApiError::unprocessable(format!(
            "the price must be positive, not {}",
            body.price
        )));
    }
    let sourced_from = body.sourced_from.trim();
    let reason = body.reason.trim();
    if sourced_from.is_empty() {
        return Err(ApiError::unprocessable(
            "sourced_from is required: record where the price was taken from",
        ));
    }
    if reason.is_empty() {
        return Err(ApiError::unprocessable(
            "reason is required: record why the price had to be entered by hand",
        ));
    }

    let row = ClosingPrice {
        id: UNASSIGNED_ID,
        listing_id,
        price_date,
        price: Some(body.price),
        // A hand-entered figure is contemporaneous by declaration — the
        // operator states what the security traded at that day — so it is
        // recorded as its own observation and no re-basing ever touches it.
        price_as_observed: Some(body.price),
        source: MANUAL_SOURCE.to_string(),
        fetched_at: Utc::now().to_rfc3339(),
        // Nothing was fetched, so there is no symbol to record (CHECK-paired
        // with the origin, migration 0038).
        fetched_symbol: None,
        status: PriceStatus::Ok,
        error: None,
        origin: PriceOrigin::Manual,
        sourced_from: Some(sourced_from.to_string()),
        reason: Some(reason.to_string()),
    };
    db_store(&pool, &row).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// Re-fetch one (listing, date) on demand — typically to replace an errored
/// row. Returns the freshly stored row (which itself is errored if the
/// provider failed again).
///
/// A **manual** row is rejected 422: a hand-entered price is a deliberate
/// correction for a day the provider got wrong or cannot serve at all, so the
/// provider never takes the day back — the price is changed by entering
/// another one.
async fn fetch_one(
    State(pool): State<SqlitePool>,
    Extension(fetcher): Extension<SharedFetcher>,
    Json(body): Json<FetchBody>,
) -> Result<Json<ClosingPrice>, ApiError> {
    let market = load_market(&pool, body.listing_id)
        .await
        .map_err(internal)?
        .ok_or_else(|| ApiError::not_found("no such listing"))?;
    validate_complete_trading_day(&market, body.price_date)?;
    reject_unpriced_date(&market, body.price_date)?;
    if let Some(stored) = db_get_one(&pool, body.listing_id, body.price_date).await?
        && stored.origin == PriceOrigin::Manual
    {
        return Err(ApiError::unprocessable(format!(
            "the stored price for {} was entered manually ({}) — re-enter it manually to \
             change it, the provider does not take the day back",
            body.price_date,
            stored.reason.unwrap_or_default()
        )));
    }

    fetch_and_store(&pool, fetcher.as_ref(), &market, &[body.price_date])
        .await
        .map_err(internal)?;
    let row = db_get_one(&pool, body.listing_id, body.price_date)
        .await
        .map_err(internal)?
        .ok_or_else(|| internal("stored row vanished"))?;
    Ok(Json(row))
}

/// Backfill a listing's price history over a date range (e.g. after importing
/// an old trade, or recovering pre-rename history under the old symbol via
/// the optional `symbol` override): trading days only, skipping dates
/// already stored ok, in one provider call.
async fn backfill(
    State(pool): State<SqlitePool>,
    Extension(fetcher): Extension<SharedFetcher>,
    Json(body): Json<BackfillBody>,
) -> Result<Json<BackfillSummary>, ApiError> {
    let mut market = load_market(&pool, body.listing_id)
        .await
        .map_err(internal)?
        .ok_or_else(|| ApiError::not_found("no such listing"))?;
    market.symbol_override = body.symbol.clone();
    if body.from > body.to {
        return Err(ApiError::unprocessable("from is after to"));
    }
    // Clamp the range to days whose close is final.
    let latest = market
        .latest_complete_trading_day(Utc::now())
        .map_err(unprocessable)?
        .filter(|latest| *latest >= body.from)
        .ok_or_else(|| ApiError::unprocessable("range contains no complete trading day"))?;
    let mut to = body.to.min(latest);
    // …and to days the provider still quotes: a listing marked unpriced from
    // a date has nothing to serve on or after it, so the range stops the day
    // before rather than storing a run of errored rows (SCENARIOS Q-02). A
    // range wholly inside the unpriced run is refused, naming the marker.
    if let Some(from) = market.listing.unpriced_from {
        reject_unpriced_date(&market, body.from)?;
        to = to.min(from.pred_opt().unwrap_or(from));
    }
    // The mirror: a listing marked unpriced *before* a date has nothing to
    // serve earlier than it, so the range starts at it rather than storing a
    // run of errored rows for a series that had not begun (migration 0037).
    // A range wholly before it is refused, naming the marker.
    let mut from = body.from;
    if let Some(before) = market.listing.unpriced_before {
        reject_unpriced_date(&market, body.to)?;
        from = from.max(before);
    }

    let mut trading_days: Vec<NaiveDate> = Vec::new();
    let mut date = from;
    while date <= to {
        if market.is_trading_day(date) {
            trading_days.push(date);
        }
        date += Duration::days(1);
    }
    let stored_ok = db_ok_dates(&pool, body.listing_id, from, to)
        .await
        .map_err(internal)?;
    let missing: Vec<NaiveDate> = trading_days
        .iter()
        .copied()
        .filter(|d| !stored_ok.contains(d))
        .collect();

    let (fetched_ok, errored) = fetch_and_store(&pool, fetcher.as_ref(), &market, &missing)
        .await
        .map_err(internal)?;
    Ok(Json(BackfillSummary {
        trading_days: trading_days.len(),
        already_stored: trading_days.len() - missing.len(),
        fetched_ok,
        errored,
    }))
}

/// Delete one **errored** row: the acknowledgement that no price will ever
/// exist for that (listing, day) — a date before the security's first trading
/// day, or a permanent hole in the provider's series — so it stops being
/// reported by `GET /reports/health`'s `errored_prices`, which otherwise nags
/// forever about a row no re-fetch can fix.
///
/// An **ok** row is rejected 422: real price data is replaced by a re-fetch
/// (`/fetch`, `/backfill`), never deleted, so this endpoint can never punch a
/// hole in a valued series. For a held listing, deleting an errored row does
/// not unblock its date — valuation still refuses it, now for want of any row
/// at all ("no stored price … backfill it") — it only clears the standing
/// alarm.
///
/// The one exception is a date inside the listing's **`unpriced_before`**
/// span, where an ok row is deletable whatever its origin. The rule protects
/// nothing there: the marker declares that no price is obtainable for the
/// security before that date, so valuation excludes the holding from those
/// dates rather than pricing it and the carry-forward is floored at the
/// marker — the stored figure is read by nothing, and deleting it is the
/// acknowledgement that it never was a valuation (migration 0037; the live
/// case is a span priced from another security's series). The mirror marker
/// `unpriced_from` gets **no** such relaxation: a date on or after it *is*
/// valued, at the last stored ok close carried forward, so a delete there
/// could remove the very figure being carried.
async fn delete_one(
    State(pool): State<SqlitePool>,
    Path((listing_id, price_date)): Path<(i64, NaiveDate)>,
) -> Result<StatusCode, ApiError> {
    let row = db_get_one(&pool, listing_id, price_date)
        .await?
        .ok_or_else(|| ApiError::not_found("no stored price for that listing and date"))?;
    let superseded = listing::db_get(&pool, listing_id)
        .await?
        .and_then(|l| l.unpriced_before)
        .is_some_and(|before| price_date < before);
    if row.status == PriceStatus::Ok && !superseded {
        let replacement = match row.origin {
            PriceOrigin::Manual => "enter another manual price to replace it",
            PriceOrigin::Fetched => "re-fetch it to replace it",
        };
        return Err(ApiError::unprocessable(format!(
            "the stored price for {price_date} is ok, not errored — {replacement} rather than \
             deleting it"
        )));
    }
    db_delete(&pool, listing_id, price_date).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// Clear a listing's whole superseded price span in one request: every row
/// dated before its `unpriced_before`, ok rows included, in one transaction,
/// answering how many were removed.
///
/// The bulk counterpart of the single-date delete above, for the case it
/// exists for — a span of hundreds of days priced from a source the listing
/// itself now says is not a price for this security. It takes **no date
/// range**: the span is read from the listing's own marker, so this cannot
/// become a general bulk-delete of price history. Safe to re-run (a second
/// call reports `deleted: 0`), and nothing is destroyed — every removed row
/// lands in `row_history` with its figure and provenance.
///
/// `404` for an unknown listing; `422` for a listing that declares no
/// `unpriced_before`, since without one there is no superseded span at all.
async fn clear_unpriced_before(
    State(pool): State<SqlitePool>,
    Json(body): Json<ClearBody>,
) -> Result<Json<ClearSummary>, ApiError> {
    let listing = listing::db_get(&pool, body.listing_id)
        .await?
        .ok_or_else(|| ApiError::not_found("no such listing"))?;
    match db_clear_unpriced_before(&pool, body.listing_id).await? {
        ClearOutcome::NoListing => Err(ApiError::not_found("no such listing")),
        ClearOutcome::NoMarker => Err(ApiError::unprocessable(format!(
            "{} has no unpriced_before, so no stored price of its is superseded — only the span \
             before that marker can be cleared in bulk. Set unpriced_before on the listing (PUT \
             /listings/:id) if the provider's series really does begin later than its stored \
             prices claim; otherwise a price is replaced by a re-fetch or another manual entry, \
             never deleted",
            listing.ticker
        ))),
        ClearOutcome::Cleared {
            unpriced_before,
            deleted,
        } => {
            tracing::info!(
                listing_id = body.listing_id,
                ticker = %listing.ticker,
                %unpriced_before,
                deleted,
                "cleared superseded closing prices"
            );
            Ok(Json(ClearSummary {
                listing_id: body.listing_id,
                unpriced_before,
                deleted,
            }))
        }
    }
}

/// 422 when `date` falls outside the span the provider serves this listing —
/// on or after `listings.unpriced_from`, or before `listings.unpriced_before`.
/// Either way the provider serves nothing there by the listing's own record,
/// so a fetch could only store another errored row. Each refusal names the
/// way back: enter the price by hand, or move/clear the marker.
fn reject_unpriced_date(market: &Market, date: NaiveDate) -> Result<(), ApiError> {
    if let Some(from) = market.listing.unpriced_from
        && date >= from
    {
        return Err(ApiError::unprocessable(format!(
            "{} is unpriced from {from} — the provider serves nothing for it from then on, so \
             valuation carries its last stored close forward instead. Enter a price by hand \
             (PUT /closing_prices/:listing_id/:price_date) if you have one, or clear \
             unpriced_from on the listing if the security is quoted again",
            market.listing.ticker
        )));
    }
    if let Some(before) = market.listing.unpriced_before
        && date < before
    {
        return Err(ApiError::unprocessable(format!(
            "{} is unpriced before {before} — the provider's series for it begins then, so \
             there is nothing to fetch earlier and valuation leaves the holding out of those \
             dates' totals instead. Enter a price by hand \
             (PUT /closing_prices/:listing_id/:price_date) if you have one, or move \
             unpriced_before back on the listing if the series reaches further than it says",
            market.listing.ticker
        )));
    }
    Ok(())
}

/// 422 unless `date` is a trading day whose close has passed.
fn validate_complete_trading_day(market: &Market, date: NaiveDate) -> Result<(), ApiError> {
    let latest = market
        .latest_complete_trading_day(Utc::now())
        .map_err(unprocessable)?;
    if latest.is_none_or(|latest| date > latest) {
        return Err(ApiError::unprocessable(format!(
            "the close of {date} is not final yet"
        )));
    }
    if !market.is_trading_day(date) {
        return Err(ApiError::unprocessable(format!(
            "{date} is not a trading day"
        )));
    }
    Ok(())
}

fn internal(e: impl std::fmt::Display) -> ApiError {
    ApiError::internal(e.to_string())
}

fn unprocessable(e: impl std::fmt::Display) -> ApiError {
    ApiError::unprocessable(e.to_string())
}

pub fn router() -> Router<SqlitePool> {
    Router::new()
        .route("/closing_prices", get(list))
        .route("/closing_prices/fetch", post(fetch_one))
        .route("/closing_prices/backfill", post(backfill))
        .route(
            "/closing_prices/clear_unpriced_before",
            post(clear_unpriced_before),
        )
        .route(
            "/closing_prices/{listing_id}/{price_date}",
            put(put_manual).delete(delete_one),
        )
}
