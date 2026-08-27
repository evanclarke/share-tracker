//! Shared fixtures for the closing-price tests, and the topic modules over
//! them. Split out of the entity's single inline `mod tests` when the file
//! was split; every test moved verbatim, which is what makes the suite a
//! behaviour lock on both splits rather than a rewrite.
//!
//! What lives here is what more than one topic uses — the row/listing
//! builders, the `StubFetcher` provider double, and the router/JSON helpers.
//! A fixture only one topic calls lives in that topic's file.

mod collection;
mod delete;
mod fetch;
mod fetcher;
mod held;
mod live;
mod manual;
mod market;
mod price_basis;
mod schema;
mod unpriced;
mod yahoo;

use super::*;

use crate::entities::exchange_holiday;

use crate::test_support::{ApiClient, test_pool};

use axum::http::StatusCode;

use std::sync::Mutex;

fn ymd(y: i32, m: u32, d: u32) -> NaiveDate {
    NaiveDate::from_ymd_opt(y, m, d).unwrap()
}

fn utc(y: i32, m: u32, d: u32, h: u32, min: u32) -> DateTime<Utc> {
    chrono::TimeZone::with_ymd_and_hms(&Utc, y, m, d, h, min, 0).unwrap()
}

/// One entry of a provider answer in `yfinance-rs`'s own shape — what
/// `yahoo_quote_named` reads a batch out of.
fn yahoo_quote(symbol: &str, price: &str, as_of: DateTime<Utc>) -> yfinance_rs::Quote {
    let mut quote = yfinance_rs::Quote::new(
        yfinance_rs::Instrument::new(
            yfinance_rs::Symbol::new(symbol).unwrap(),
            yfinance_rs::AssetKind::Equity,
        ),
        yfinance_rs::Currency::Iso(yfinance_rs::IsoCurrency::AUD),
    );
    quote.price = Some(yfinance_rs::PriceAmount::new(price.parse().unwrap()));
    quote.as_of = Some(as_of);
    quote
}

async fn insert_listing(pool: &SqlitePool, id: i64, ticker: &str, mic: &str, currency: &str) {
    crate::test_support::listing(id)
        .ticker(ticker)
        .name(ticker)
        .mic(mic)
        .security_type(listing::SecurityType::Share)
        .currency(currency)
        .insert(pool)
        .await;
}

async fn insert_crypto_listing(pool: &SqlitePool, id: i64, ticker: &str) {
    crate::test_support::listing(id)
        .crypto()
        .ticker(ticker)
        .name(ticker)
        .insert(pool)
        .await;
}

async fn insert_buy(pool: &SqlitePool, id: i64, listing_id: i64, qty: &str) {
    crate::test_support::buy(id, listing_id)
        .date(ymd(2024, 1, 16))
        .qty(qty.parse().unwrap())
        .price(Decimal::from(10))
        .insert(pool)
        .await;
}

async fn sell_everything(pool: &SqlitePool, sell_id: i64, buy_id: i64, listing_id: i64, qty: &str) {
    crate::test_support::sell(sell_id, listing_id)
        .date(ymd(2024, 6, 3))
        .qty(qty.parse().unwrap())
        .price(Decimal::from(12))
        .insert(pool)
        .await;
    crate::test_support::allocate(pool, sell_id, sell_id, buy_id, qty.parse().unwrap()).await;
}

/// Stub provider: per-listing canned closes and latest quotes (keyed by
/// listing id), or a blanket failure. Records every (listing, from, to)
/// call.
#[derive(Default)]
struct StubFetcher {
    closes: HashMap<i64, Vec<FetchedClose>>,
    quotes: HashMap<i64, LatestQuote>,
    fail: Option<FetchError>,
    calls: Mutex<Vec<(i64, NaiveDate, NaiveDate)>>,
    /// The resolved symbol (`yahoo_symbol`'s output) each `daily_closes`
    /// call was made with — lets a test confirm a backfill `symbol`
    /// override actually reached the fetcher.
    symbols: Mutex<Vec<String>>,
    /// The listing ids of each `latest_quotes` call, one entry per call —
    /// so a test can assert live valuation asks the provider *once* for
    /// the whole portfolio rather than once per holding.
    quote_batches: Mutex<Vec<Vec<i64>>>,
}

impl StubFetcher {
    fn with_close(mut self, listing_id: i64, date: NaiveDate, price: &str, ccy: &str) -> Self {
        self.closes
            .entry(listing_id)
            .or_default()
            .push(FetchedClose {
                date,
                price: price.parse().unwrap(),
                currency: ccy.to_string(),
            });
        self
    }

    fn with_quote(mut self, listing_id: i64, price: &str, ccy: &str, as_of: DateTime<Utc>) -> Self {
        self.quotes.insert(
            listing_id,
            LatestQuote {
                price: price.parse().unwrap(),
                currency: ccy.to_string(),
                as_of,
            },
        );
        self
    }

    /// A blanket failure of the "not evidence about the symbol" kind —
    /// an outage. See [`StubFetcher::failing_no_such_symbol`] for the
    /// other kind.
    fn failing(msg: &str) -> Self {
        StubFetcher {
            fail: Some(FetchError::Other(msg.to_string())),
            ..Default::default()
        }
    }

    /// A provider that positively answers that it has no such series —
    /// what Yahoo does for a ticker retired by a rename.
    fn failing_no_such_symbol(msg: &str) -> Self {
        StubFetcher {
            fail: Some(FetchError::NoSuchSymbol(msg.to_string())),
            ..Default::default()
        }
    }

    fn calls(&self) -> Vec<(i64, NaiveDate, NaiveDate)> {
        self.calls.lock().unwrap().clone()
    }

    fn symbols(&self) -> Vec<String> {
        self.symbols.lock().unwrap().clone()
    }

    fn quote_batches(&self) -> Vec<Vec<i64>> {
        self.quote_batches.lock().unwrap().clone()
    }
}

impl PriceFetcher for StubFetcher {
    fn source(&self) -> &'static str {
        "stub"
    }

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
            self.calls
                .lock()
                .unwrap()
                .push((market.listing.id, from, to));
            self.symbols
                .lock()
                .unwrap()
                .push(self.symbol(market, from).unwrap_or_default());
            if let Some(failure) = &self.fail {
                return Err(failure.clone());
            }
            Ok(self
                .closes
                .get(&market.listing.id)
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

    /// Records the batch, then answers it the way the trait's default
    /// does. The recording is the point: it is what lets a test see how
    /// many times the provider was asked, which is the whole subject of
    /// the batching.
    fn latest_quotes<'a>(&'a self, markets: &'a [&'a Market]) -> QuotesFuture<'a> {
        Box::pin(async move {
            self.quote_batches
                .lock()
                .unwrap()
                .push(markets.iter().map(|m| m.listing.id).collect());
            let mut out = Vec::with_capacity(markets.len());
            for market in markets {
                out.push(self.latest_quote(market).await);
            }
            out
        })
    }
}

// 2026-06-05 is a Friday; 2026-06-06/07 the weekend.
// 08:00 UTC = 18:00 Sydney (AEST) — after the 16:00 ASX close.
fn friday_evening_sydney() -> DateTime<Utc> {
    utc(2026, 6, 5, 8, 0)
}

async fn insert_share_split(
    pool: &SqlitePool,
    listing_id: i64,
    date: NaiveDate,
    new: &str,
    old: &str,
) {
    crate::entities::corporate_action::db_upsert(
        pool,
        &crate::entities::corporate_action::CorporateAction {
            id: 900 + listing_id,
            listing_id,
            date,
            kind: crate::entities::corporate_action::ActionKind::ShareSplit {
                split_new_units: new.parse().unwrap(),
                split_old_units: old.parse().unwrap(),
            },
        },
    )
    .await
    .unwrap();
}

/// Record a rename through the entity's own path, so the chain and the
/// listing row move together exactly as `POST /listings/:id/rename` does.
async fn rename_listing(
    pool: &SqlitePool,
    listing_id: i64,
    effective_date: NaiveDate,
    new_ticker: &str,
    new_mic: Option<&str>,
) {
    crate::entities::listing_rename::db_rename(
        pool,
        listing_id,
        &crate::entities::listing_rename::RenameBody {
            effective_date,
            ticker: new_ticker.to_string(),
            exchange_mic: new_mic.map(str::to_string),
            name: None,
            price_symbol: None,
            note: None,
        },
    )
    .await
    .unwrap();
}

/// The 7-trading-day lookback window ending Friday 2026-06-05 on the ASX
/// calendar (no seeded holiday falls inside it), oldest first.
/// The ASX trading days in the collection window ending Friday
/// 2026-06-05 — the last [`COLLECTION_LOOKBACK_DAYS`] calendar days, so
/// from Saturday 2026-05-23, whose first trading day is Monday 2026-05-25.
fn asx_lookback_window() -> Vec<NaiveDate> {
    vec![
        ymd(2026, 5, 25),
        ymd(2026, 5, 26),
        ymd(2026, 5, 27),
        ymd(2026, 5, 28),
        ymd(2026, 5, 29),
        ymd(2026, 6, 1),
        ymd(2026, 6, 2),
        ymd(2026, 6, 3),
        ymd(2026, 6, 4),
        ymd(2026, 6, 5),
    ]
}

/// Store an ok row directly (as an earlier successful run would have).
async fn seed_ok_price(pool: &SqlitePool, listing_id: i64, date: NaiveDate) {
    crate::test_support::closing_price(listing_id, date)
        .source("stub")
        .fetched_at("2026-06-01T00:00:00Z")
        .insert(pool)
        .await;
}

/// Store a hand-entered price directly, as `PUT /closing_prices/…` does.
async fn seed_manual_price(pool: &SqlitePool, listing_id: i64, date: NaiveDate, price: &str) {
    crate::test_support::closing_price(listing_id, date)
        .price(price)
        .manual("asx.com.au closing report", "provider serves no candle")
        .insert(pool)
        .await;
}

fn full_router(pool: SqlitePool, fetcher: StubFetcher) -> ApiClient {
    let shared: SharedFetcher = Arc::new(fetcher);
    ApiClient::over(router().with_state(pool).layer(Extension(shared)))
}

async fn post_json(
    app: &ApiClient,
    uri: &str,
    body: serde_json::Value,
) -> (StatusCode, axum::body::Bytes) {
    let resp = app.post(uri, &body).await;
    let status = resp.status;
    let bytes = resp.body.clone();
    (status, bytes)
}

async fn delete_req(app: &ApiClient, uri: &str) -> (StatusCode, axum::body::Bytes) {
    let resp = app.delete(uri).await;
    let status = resp.status;
    let bytes = resp.body.clone();
    (status, bytes)
}

/// Store one errored row for (listing 1, `date`) via the normal fetch
/// path — a stub with no candle for the day.
async fn store_errored(pool: &SqlitePool, date: NaiveDate) {
    let market = load_market(pool, 1).await.unwrap().unwrap();
    let (_, errored) = fetch_and_store(pool, &StubFetcher::default(), &market, &[date])
        .await
        .unwrap();
    assert_eq!(errored, 1);
}
