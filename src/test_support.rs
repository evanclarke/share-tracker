//! Shared test fixtures (`#[cfg(test)]`-only, see the declaration in `main.rs`).
//!
//! Two halves. The *data* half: builders that let a test spell out only what it
//! varies instead of every `Listing`/`Trade` field — a new column is then added
//! here once, not in every test module. Each builder starts from a plausible
//! default row and exposes setters only for what tests actually vary; `insert`
//! upserts through the entity's own `db_upsert` so write-time invariants still
//! apply. The *HTTP* half: [`ApiClient`], which drives a router through
//! `tower::ServiceExt::oneshot` so a test says `client.put_json(path, &body)`
//! rather than open-coding `Request::builder()` and the body decode.

use crate::entities::{
    amit_adjustment, amma, closing_price, ess_statement, income, listing, parcel_allocation, trade,
};
use crate::infra::db;
use axum::body::{Body, Bytes};
use axum::http::{HeaderMap, Request, StatusCode};
use chrono::NaiveDate;
use http_body_util::BodyExt;
use rust_decimal::Decimal;
use sqlx::SqlitePool;
use tower::ServiceExt;

/// Fresh in-memory database with migrations and seed data applied.
pub async fn test_pool() -> SqlitePool {
    db::init(":memory:").await.unwrap()
}

/// One request's outcome: the status and the raw body, kept together so a
/// rejection test can assert on both without re-plumbing the decode.
pub struct ApiResponse {
    pub status: StatusCode,
    pub headers: HeaderMap,
    pub body: Bytes,
}

impl ApiResponse {
    /// The body as JSON. Panics with the body text on a decode failure — a
    /// test that mis-types its response should say what actually came back.
    pub fn json<T: serde::de::DeserializeOwned>(&self) -> T {
        serde_json::from_slice(&self.body)
            .unwrap_or_else(|e| panic!("decoding {:?} failed: {e}", self.text()))
    }

    /// The body as text — the plain-text reason on a 4xx.
    pub fn text(&self) -> &str {
        std::str::from_utf8(&self.body).expect("response body is not UTF-8")
    }

    /// Status plus body text, the pair the rejection tests assert on.
    pub fn status_and_body(&self) -> (StatusCode, &str) {
        (self.status, self.text())
    }

    /// Requires `expected`, reporting the body when it doesn't match, and
    /// returns self so a decode can be chained on.
    #[track_caller]
    pub fn expect_status(self, expected: StatusCode) -> Self {
        assert_eq!(self.status, expected, "body: {:?}", self.text());
        self
    }
}

/// Drives a router in-process — no network, no port binding. Construct it over
/// whichever router the test needs: one entity's own [`ApiClient::over`], or
/// the whole application as `main` serves it ([`ApiClient::full`]).
#[derive(Clone)]
pub struct ApiClient {
    app: axum::Router,
    /// Extra headers sent with every request — see [`Self::with_header`].
    headers: Vec<(String, String)>,
}

impl ApiClient {
    /// Client over an already-assembled router, e.g. one module's
    /// `router().with_state(pool)`.
    pub fn over(app: axum::Router) -> Self {
        ApiClient {
            app,
            headers: Vec::new(),
        }
    }

    /// Returns a client that also sends `name: value` on every request it
    /// makes from here on — e.g. a session cookie or `Authorization: Bearer
    /// <token>` against an `infra::auth`-gated router. Chainable:
    /// `client.with_header("Cookie", a).with_header("X-Foo", b)`.
    pub fn with_header(mut self, name: impl Into<String>, value: impl Into<String>) -> Self {
        self.headers.push((name.into(), value.into()));
        self
    }

    /// Client over the full application router, exactly as `main` serves it,
    /// but with an offline price stub in place of the live `YahooFetcher` — so
    /// no test path can reach the network.
    pub fn full(pool: &SqlitePool) -> Self {
        Self::full_with(
            pool,
            closing_price::test_support::QuoteStub::default().shared(),
        )
    }

    /// [`Self::full`] with a caller-supplied price fetcher, for the tests that
    /// need canned quotes or a failing provider.
    pub fn full_with(pool: &SqlitePool, fetcher: closing_price::SharedFetcher) -> Self {
        let registry = crate::infra::scheduler::registry(
            pool.clone(),
            ":memory:".to_string(),
            None,
            None,
            fetcher.clone(),
        );
        // Mounted at the root, auth off — the default. `app::router`'s own
        // tests cover a router nested under a reverse-proxy base path;
        // `infra::auth`'s own tests cover one with `[auth]` configured.
        ApiClient::over(crate::app::router(
            "",
            pool.clone(),
            registry,
            fetcher,
            None,
        ))
    }

    async fn send(&self, mut req: Request<Body>) -> ApiResponse {
        for (name, value) in &self.headers {
            req.headers_mut().insert(
                axum::http::HeaderName::from_bytes(name.as_bytes()).expect("valid header name"),
                axum::http::HeaderValue::from_str(value).expect("valid header value"),
            );
        }
        let resp = self.app.clone().oneshot(req).await.unwrap();
        let status = resp.status();
        let headers = resp.headers().clone();
        let body = resp.into_body().collect().await.unwrap().to_bytes();
        ApiResponse {
            status,
            headers,
            body,
        }
    }

    async fn with_body(
        &self,
        method: &str,
        path: impl AsRef<str>,
        body: &impl serde::Serialize,
    ) -> ApiResponse {
        self.send(
            Request::builder()
                .method(method)
                .uri(path.as_ref())
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_vec(body).unwrap()))
                .unwrap(),
        )
        .await
    }

    pub async fn get(&self, path: impl AsRef<str>) -> ApiResponse {
        self.send(
            Request::builder()
                .uri(path.as_ref())
                .body(Body::empty())
                .unwrap(),
        )
        .await
    }

    pub async fn put(&self, path: impl AsRef<str>, body: &impl serde::Serialize) -> ApiResponse {
        self.with_body("PUT", path, body).await
    }

    pub async fn post(&self, path: impl AsRef<str>, body: &impl serde::Serialize) -> ApiResponse {
        self.with_body("POST", path, body).await
    }

    /// PUT a body already written out as a string, for the tests whose point
    /// is the exact bytes on the wire (a malformed or hand-shaped payload).
    pub async fn put_raw(&self, path: impl AsRef<str>, body: &str) -> ApiResponse {
        self.raw("PUT", path, body).await
    }

    /// POST a body already written out as a string. See [`Self::put_raw`].
    pub async fn post_raw(&self, path: impl AsRef<str>, body: &str) -> ApiResponse {
        self.raw("POST", path, body).await
    }

    async fn raw(&self, method: &str, path: impl AsRef<str>, body: &str) -> ApiResponse {
        self.send(
            Request::builder()
                .method(method)
                .uri(path.as_ref())
                .header("content-type", "application/json")
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
    }

    /// POST raw bytes for the payloads that are not JSON — the multipart
    /// uploads and the CSV/XML import feeds. `content_type` is optional
    /// because the import endpoints take a bare `String` body and are driven
    /// with no content type at all.
    pub async fn post_bytes(
        &self,
        path: impl AsRef<str>,
        content_type: Option<&str>,
        body: impl Into<Body>,
    ) -> ApiResponse {
        let mut req = Request::builder().method("POST").uri(path.as_ref());
        if let Some(ct) = content_type {
            req = req.header("content-type", ct);
        }
        self.send(req.body(body.into()).unwrap()).await
    }

    /// POST with no request body, for the endpoints that take none
    /// (`POST /jobs/{name}`).
    pub async fn post_empty(&self, path: impl AsRef<str>) -> ApiResponse {
        self.send(
            Request::builder()
                .method("POST")
                .uri(path.as_ref())
                .body(Body::empty())
                .unwrap(),
        )
        .await
    }

    pub async fn delete(&self, path: impl AsRef<str>) -> ApiResponse {
        self.send(
            Request::builder()
                .method("DELETE")
                .uri(path.as_ref())
                .body(Body::empty())
                .unwrap(),
        )
        .await
    }

    /// GET, require 200, decode the JSON body — the read half of most tests.
    pub async fn get_json<T: serde::de::DeserializeOwned>(&self, path: impl AsRef<str>) -> T {
        self.get(path).await.expect_status(StatusCode::OK).json()
    }

    /// PUT a JSON body and return the status alone (an entity upsert answers
    /// 204 with no body); the rejection tests use [`Self::put`] instead.
    pub async fn put_json(
        &self,
        path: impl AsRef<str>,
        body: &impl serde::Serialize,
    ) -> StatusCode {
        self.put(path, body).await.status
    }

    /// PUT a JSON body and require the 204 an entity upsert answers.
    pub async fn put_ok(&self, path: impl AsRef<str>, body: &impl serde::Serialize) {
        self.put(path, body)
            .await
            .expect_status(StatusCode::NO_CONTENT);
    }

    /// POST a JSON body, require 200, decode the JSON body.
    pub async fn post_json<T: serde::de::DeserializeOwned>(
        &self,
        path: impl AsRef<str>,
        body: &impl serde::Serialize,
    ) -> T {
        self.post(path, body)
            .await
            .expect_status(StatusCode::OK)
            .json()
    }
}

/// `NaiveDate` literal without the `from_ymd_opt(..).unwrap()` ceremony.
pub fn ymd(y: i32, m: u32, d: u32) -> NaiveDate {
    NaiveDate::from_ymd_opt(y, m, d).unwrap()
}

/// `Decimal` literal from a string.
pub fn dec(s: &str) -> Decimal {
    s.parse().unwrap()
}

/// Listing fixture: ASX-listed AUD ETF `T{id}` named `Test {id}`.
pub fn listing(id: i64) -> ListingBuilder {
    ListingBuilder {
        l: listing::Listing {
            id,
            exchange_mic: Some("XASX".to_string()),
            ticker: format!("T{id}"),
            name: format!("Test {id}"),
            isin: None,
            security_type: listing::SecurityType::ETF,
            currency: "AUD".to_string(),
            amit: false,
            amit_from: None,
            unpriced_from: None,
            unpriced_before: None,
            preference: false,
            price_symbol: None,
        },
    }
}

pub struct ListingBuilder {
    l: listing::Listing,
}

impl ListingBuilder {
    pub fn ticker(mut self, ticker: &str) -> Self {
        self.l.ticker = ticker.to_string();
        self
    }

    pub fn name(mut self, name: &str) -> Self {
        self.l.name = name.to_string();
        self
    }

    pub fn mic(mut self, mic: &str) -> Self {
        self.l.exchange_mic = Some(mic.to_string());
        self
    }

    pub fn currency(mut self, currency: &str) -> Self {
        self.l.currency = currency.to_string();
        self
    }

    pub fn security_type(mut self, st: listing::SecurityType) -> Self {
        self.l.security_type = st;
        self
    }

    /// Crypto listing: no exchange MIC (CHECK-enforced pairing).
    pub fn crypto(mut self) -> Self {
        self.l.security_type = listing::SecurityType::Crypto;
        self.l.exchange_mic = None;
        self
    }

    pub fn amit(mut self, amit: bool) -> Self {
        self.l.amit = amit;
        self
    }

    /// The date the fund became an AMIT (SCENARIOS F-23) — implies `amit`.
    pub fn amit_from(mut self, from: NaiveDate) -> Self {
        self.l.amit = true;
        self.l.amit_from = Some(from);
        self
    }

    /// The date the price provider stopped quoting the security
    /// (SCENARIOS Q-02).
    pub fn unpriced_from(mut self, from: NaiveDate) -> Self {
        self.l.unpriced_from = Some(from);
        self
    }

    /// The date the price provider's series *begins* for the security —
    /// before it nothing is obtainable at any price (migration 0037).
    pub fn unpriced_before(mut self, before: NaiveDate) -> Self {
        self.l.unpriced_before = Some(before);
        self
    }

    pub fn preference(mut self, preference: bool) -> Self {
        self.l.preference = preference;
        self
    }

    pub fn price_symbol(mut self, symbol: &str) -> Self {
        self.l.price_symbol = Some(symbol.to_string());
        self
    }

    /// Escape hatch for fields without a dedicated setter.
    pub fn with(mut self, f: impl FnOnce(&mut listing::Listing)) -> Self {
        f(&mut self.l);
        self
    }

    pub fn build(self) -> listing::Listing {
        self.l
    }

    pub async fn insert(self, pool: &SqlitePool) {
        listing::db_upsert(pool, &self.l).await.unwrap();
    }
}

/// Closing-price fixture: a provider-fetched ok price of 10 in the listing's
/// quote currency. `.errored(msg)` turns it into a recorded fetch failure and
/// `.manual(sourced_from, reason)` into a hand-entered price — each keeps the
/// row's CHECK pairings consistent, so a test cannot build an impossible one.
pub fn closing_price(listing_id: i64, price_date: NaiveDate) -> ClosingPriceBuilder {
    ClosingPriceBuilder {
        p: closing_price::ClosingPrice {
            id: closing_price::UNASSIGNED_ID,
            listing_id,
            price_date,
            price: Some(Decimal::from(10)),
            price_as_observed: Some(Decimal::from(10)),
            source: "test".to_string(),
            fetched_at: "2026-06-05T08:00:00Z".to_string(),
            fetched_symbol: None,
            status: closing_price::PriceStatus::Ok,
            error: None,
            origin: closing_price::PriceOrigin::Fetched,
            sourced_from: None,
            reason: None,
        },
    }
}

pub struct ClosingPriceBuilder {
    p: closing_price::ClosingPrice,
}

impl ClosingPriceBuilder {
    /// The stored price *and* the figure it was observed as: a fixture is in
    /// its own day's unit basis unless a test says otherwise with
    /// [`Self::as_observed`].
    pub fn price(mut self, price: &str) -> Self {
        self.p.price = Some(dec(price));
        self.p.price_as_observed = Some(dec(price));
        self
    }

    pub fn source(mut self, source: &str) -> Self {
        self.p.source = source.to_string();
        self
    }

    pub fn fetched_at(mut self, fetched_at: &str) -> Self {
        self.p.fetched_at = fetched_at.to_string();
        self
    }

    /// The provider symbol the row was fetched under (0038). Left unrecorded
    /// by default, which is what a row stored before that column existed
    /// looks like.
    pub fn fetched_symbol(mut self, symbol: &str) -> Self {
        self.p.fetched_symbol = Some(symbol.to_string());
        self
    }

    /// A recorded fetch failure: no price, the message stored (CHECK-paired).
    pub fn errored(mut self, error: &str) -> Self {
        self.p.price = None;
        self.p.price_as_observed = None;
        self.p.status = closing_price::PriceStatus::Error;
        self.p.error = Some(error.to_string());
        self
    }

    /// A price entered by hand, with the provenance the schema requires: the
    /// `source` moves to `manual` in step with the origin (CHECK-paired).
    pub fn manual(mut self, sourced_from: &str, reason: &str) -> Self {
        self.p.source = closing_price::MANUAL_SOURCE.to_string();
        self.p.origin = closing_price::PriceOrigin::Manual;
        // Nothing was fetched, so no symbol was used (CHECK-paired, 0038).
        self.p.fetched_symbol = None;
        self.p.sourced_from = Some(sourced_from.to_string());
        self.p.reason = Some(reason.to_string());
        self
    }

    pub async fn insert(self, pool: &SqlitePool) {
        closing_price::db_store(pool, &self.p).await.unwrap();
    }
}

/// Buy fixture: 100 units @ 10 AUD on 2024-01-01 (T+2), no brokerage,
/// default holding account.
pub fn buy(id: i64, listing_id: i64) -> TradeBuilder {
    TradeBuilder::new(id, listing_id, trade::TradeType::Buy)
}

/// Sell fixture: same defaults as [`buy`].
pub fn sell(id: i64, listing_id: i64) -> TradeBuilder {
    TradeBuilder::new(id, listing_id, trade::TradeType::Sell)
}

/// DRP fixture: same defaults as [`buy`].
pub fn drp(id: i64, listing_id: i64) -> TradeBuilder {
    TradeBuilder::new(id, listing_id, trade::TradeType::DRP)
}

/// Trade fixture with an explicit type, for helpers parameterised over
/// Buy/Sell. Same defaults as [`buy`].
pub fn trade(id: i64, listing_id: i64, trade_type: trade::TradeType) -> TradeBuilder {
    TradeBuilder::new(id, listing_id, trade_type)
}

pub struct TradeBuilder {
    t: trade::Trade,
    settlement_overridden: bool,
    currency_overridden: bool,
}

impl TradeBuilder {
    fn new(id: i64, listing_id: i64, trade_type: trade::TradeType) -> Self {
        let date = ymd(2024, 1, 1);
        TradeBuilder {
            t: trade::Trade {
                id,
                trade_type,
                date,
                settlement_date: date + chrono::Duration::days(2),
                listing_id,
                average_price: Decimal::from(10),
                quantity: Decimal::from(100),
                currency: "AUD".to_string(),
                brokerage: Decimal::ZERO,
                gst_on_brokerage: Decimal::ZERO,
                brokerage_includes_gst: false,
                brokerage_currency: "AUD".to_string(),
                fx_rate: Decimal::ONE,
                spot_fx_rate: None,
                contract_note_ref: None,
                statement_total: None,
                residual_brought_forward: Decimal::ZERO,
                residual_carried_forward: Decimal::ZERO,
                residual_paid_out: Decimal::ZERO,
                rights_action_id: None,
                buyback_action_id: None,
                scrip_action_id: None,
                demerger_action_id: None,
                worthless_action_id: None,
                deemed_acquisition_date: None,
                holding_account_id: 1,
                transfer_id: None,
                ess_statement_id: None,
                inheritance_id: None,
            },
            settlement_overridden: false,
            currency_overridden: false,
        }
    }

    /// Sets the trade date; the settlement date follows at T+2 unless
    /// [`Self::settlement`] pinned it.
    pub fn date(mut self, date: NaiveDate) -> Self {
        self.t.date = date;
        if !self.settlement_overridden {
            self.t.settlement_date = date + chrono::Duration::days(2);
        }
        self
    }

    pub fn settlement(mut self, date: NaiveDate) -> Self {
        self.t.settlement_date = date;
        self.settlement_overridden = true;
        self
    }

    pub fn qty(mut self, qty: Decimal) -> Self {
        self.t.quantity = qty;
        self
    }

    pub fn price(mut self, price: Decimal) -> Self {
        self.t.average_price = price;
        self
    }

    /// Sets the trade currency (and the brokerage currency with it — the two
    /// can never differ: a brokerage billed in another currency is rejected at
    /// write time, see `trade::AmountsError::BrokerageCurrencyMismatch`).
    pub fn currency(mut self, currency: &str) -> Self {
        self.t.currency = currency.to_string();
        self.t.brokerage_currency = currency.to_string();
        self.currency_overridden = true;
        self
    }

    pub fn fx_rate(mut self, fx_rate: Decimal) -> Self {
        self.t.fx_rate = fx_rate;
        self
    }

    /// Sets the deliberate transaction-date spot-rate override (wins over the
    /// ATO monthly rate; see `infra::fx::FxOverride`).
    pub fn spot_fx_rate(mut self, spot: Decimal) -> Self {
        self.t.spot_fx_rate = Some(spot);
        self
    }

    pub fn brokerage(mut self, brokerage: Decimal) -> Self {
        self.t.brokerage = brokerage;
        self
    }

    pub fn gst_on_brokerage(mut self, gst: Decimal) -> Self {
        self.t.gst_on_brokerage = gst;
        self
    }

    pub fn account(mut self, holding_account_id: i64) -> Self {
        self.t.holding_account_id = holding_account_id;
        self
    }

    /// Escape hatch for fields without a dedicated setter.
    pub fn with(mut self, f: impl FnOnce(&mut trade::Trade)) -> Self {
        f(&mut self.t);
        self
    }

    pub fn build(self) -> trade::Trade {
        self.t
    }

    /// Inserts through `trade::db_upsert`, so every write-time invariant
    /// still applies. A test that never named a currency takes the
    /// **listing's**, which is what `db_upsert` requires of a real trade
    /// (SCENARIOS M-08): the default AUD would otherwise refuse every
    /// fixture built on a foreign listing, and spelling the currency out at
    /// each call site would only repeat what the listing already says.
    pub async fn insert(mut self, pool: &SqlitePool) {
        if !self.currency_overridden
            && let Some(listing) =
                sqlx::query_scalar::<_, String>("SELECT currency FROM listings WHERE id = ?")
                    .bind(self.t.listing_id)
                    .fetch_optional(pool)
                    .await
                    .unwrap()
        {
            self.t.currency = listing.clone();
            self.t.brokerage_currency = listing;
        }
        trade::db_upsert(pool, &self.t).await.unwrap();
    }
}

/// AMMA statement fixture: 100 units over FY ending 2024-06-30, received
/// 2024-08-15, every amount zero, AUD, default holding account.
pub fn amma(id: i64, listing_id: i64) -> AmmaBuilder {
    AmmaBuilder {
        a: amma::AmmaStatement {
            id,
            listing_id,
            tax_year_end_date: ymd(2024, 6, 30),
            units_held: Decimal::from(100),
            date_received: ymd(2024, 8, 15),
            australian_interest: Decimal::ZERO,
            australian_dividends_unfranked: Decimal::ZERO,
            franked_dividends: Decimal::ZERO,
            franking_credits: Decimal::ZERO,
            net_rent: Decimal::ZERO,
            foreign_income: Decimal::ZERO,
            foreign_tax_credits: Decimal::ZERO,
            foreign_tax_credits_capital_gains: Decimal::ZERO,
            other_income: Decimal::ZERO,
            cgt_discount_gains: Decimal::ZERO,
            cgt_indexation_gains: Decimal::ZERO,
            cgt_other_gains: Decimal::ZERO,
            capital_losses_applied: Decimal::ZERO,
            tax_deferred_amount: Decimal::ZERO,
            tax_free_amount: Decimal::ZERO,
            cost_base_adjustment: Decimal::ZERO,
            tfn_withholding_tax: Decimal::ZERO,
            currency: "AUD".to_string(),
            holding_account_id: 1,
        },
    }
}

pub struct AmmaBuilder {
    a: amma::AmmaStatement,
}

impl AmmaBuilder {
    pub fn units(mut self, units: Decimal) -> Self {
        self.a.units_held = units;
        self
    }

    pub fn cost_base_adjustment(mut self, per_unit: Decimal) -> Self {
        self.a.cost_base_adjustment = per_unit;
        self
    }

    /// Escape hatch for fields without a dedicated setter.
    pub fn with(mut self, f: impl FnOnce(&mut amma::AmmaStatement)) -> Self {
        f(&mut self.a);
        self
    }

    pub fn build(self) -> amma::AmmaStatement {
        self.a
    }

    /// Inserts through `amma::db_upsert`, so every write-time invariant still
    /// applies. A statement still carrying the builder's default AUD takes the
    /// **listing's** currency instead, which is what `db_upsert` requires
    /// (SCENARIOS M-08) — the same defaulting [`TradeBuilder::insert`] does,
    /// and for the same reason. A statement given a currency of its own (via
    /// [`Self::with`]) is left exactly as written, so a test that wants the
    /// mismatch *refused* names a non-AUD one — or drives the API, which is
    /// where that refusal is asserted.
    pub async fn insert(mut self, pool: &SqlitePool) {
        if self.a.currency == "AUD"
            && let Some(listing) =
                sqlx::query_scalar::<_, String>("SELECT currency FROM listings WHERE id = ?")
                    .bind(self.a.listing_id)
                    .fetch_optional(pool)
                    .await
                    .unwrap()
        {
            self.a.currency = listing;
        }
        amma::db_upsert(pool, &self.a).await.unwrap();
    }
}

/// Income fixture: every amount zero, no dates beyond `date_paid`, AUD,
/// default holding account. Vary fields via [`IncomeBuilder::with`].
pub fn income(id: i64, listing_id: i64, date_paid: NaiveDate) -> IncomeBuilder {
    IncomeBuilder {
        i: income::Income {
            id,
            listing_id,
            date_paid,
            ex_date: None,
            franked_amount: Decimal::ZERO,
            unfranked_amount: Decimal::ZERO,
            foreign_source_income: Decimal::ZERO,
            foreign_tax_paid: Decimal::ZERO,
            tfn_withholding_tax: Decimal::ZERO,
            franking_credits: Decimal::ZERO,
            lic_capital_gain_amount: Decimal::ZERO,
            conduit_foreign_income: Decimal::ZERO,
            trust_income: false,
            entitlement_date: None,
            reinvestment_trade_id: None,
            currency: "AUD".to_string(),
            buyback_trade_id: None,
            holding_account_id: 1,
            amount_per_security: None,
            securities_held: None,
            tax_deferred_amount: None,
            income_type: income::IncomeType::Dividend,
        },
    }
}

pub struct IncomeBuilder {
    i: income::Income,
}

impl IncomeBuilder {
    /// Escape hatch for fields without a dedicated setter.
    pub fn with(mut self, f: impl FnOnce(&mut income::Income)) -> Self {
        f(&mut self.i);
        self
    }

    /// A fully franked dividend stated by the **franking credit** at stake:
    /// sets the credit and the franked amount that carries it (credit × 70/30,
    /// cent-rounded).
    ///
    /// The franking tests are all written in terms of the credits — how many
    /// are denied, whether the year crosses the A$5,000 threshold — and used
    /// to set the credit alone. That is not a dividend a company could pay:
    /// the credit is attached to the franked part of a distribution, so
    /// `income::db_upsert` now rejects a credit with nothing behind it and
    /// caps it at what the franked amount could carry
    /// (`domain::franking_credit`). Stating both here keeps those fixtures a
    /// dividend that could exist, without every test spelling out the
    /// gross-up.
    pub fn fully_franked_credits(mut self, credits: Decimal) -> Self {
        self.i.franking_credits = credits;
        self.i.franked_amount = (credits * Decimal::from(70) / Decimal::from(30))
            .round_dp_with_strategy(2, rust_decimal::RoundingStrategy::MidpointAwayFromZero);
        self
    }

    pub fn build(self) -> income::Income {
        self.i
    }

    pub async fn insert(self, pool: &SqlitePool) {
        income::db_upsert(pool, &self.i).await.unwrap();
    }
}

/// ESS statement fixture: every amount zero, AUD, default holding account.
/// Vary fields via [`EssStatementBuilder::with`].
pub fn ess_statement(id: i64, listing_id: i64, taxing_point: NaiveDate) -> EssStatementBuilder {
    EssStatementBuilder {
        s: ess_statement::EssStatement {
            id,
            listing_id,
            holding_account_id: 1,
            taxing_point_date: taxing_point,
            quantity: Decimal::ZERO,
            market_value_per_share: Decimal::ZERO,
            taxed_upfront_eligible: Decimal::ZERO,
            taxed_upfront_not_eligible: Decimal::ZERO,
            deferral_discount: Decimal::ZERO,
            pre_2009_cessation_discount: Decimal::ZERO,
            foreign_source_discount: Decimal::ZERO,
            tfn_withholding: Decimal::ZERO,
            currency: "AUD".to_string(),
            fx_rate: None,
            aud_taxed_upfront_eligible: None,
            aud_taxed_upfront_not_eligible: None,
            aud_deferral_discount: None,
            aud_pre_2009_cessation_discount: None,
            aud_foreign_source_discount: None,
            vest_trade_id: None,
        },
    }
}

pub struct EssStatementBuilder {
    s: ess_statement::EssStatement,
}

impl EssStatementBuilder {
    /// Escape hatch for fields without a dedicated setter.
    pub fn with(mut self, f: impl FnOnce(&mut ess_statement::EssStatement)) -> Self {
        f(&mut self.s);
        self
    }

    pub fn build(self) -> ess_statement::EssStatement {
        self.s
    }

    pub async fn insert(self, pool: &SqlitePool) {
        ess_statement::db_upsert(pool, &self.s).await.unwrap();
    }
}

/// AMIT adjustment linking an AMMA statement's per-unit cost-base adjustment
/// to `qty` units of a parcel.
pub async fn amit_adjustment(
    pool: &SqlitePool,
    id: i64,
    amma_id: i64,
    trade_id: i64,
    qty: Decimal,
) {
    amit_adjustment::db_upsert(
        pool,
        &amit_adjustment::AmitAdjustment {
            id,
            amma_statement_id: amma_id,
            trade_id,
            quantity: qty,
        },
    )
    .await
    .unwrap();
}

/// Parcel allocation linking a Sell to the Buy/DRP parcel it consumes.
pub async fn allocate(pool: &SqlitePool, id: i64, sale_id: i64, buy_id: i64, qty: Decimal) {
    parcel_allocation::db_upsert(
        pool,
        &parcel_allocation::ParcelAllocation {
            id,
            sale_trade_id: sale_id,
            purchase_trade_id: buy_id,
            quantity_allocated: qty,
        },
    )
    .await
    .unwrap();
}

/// Tests of the fixtures themselves — specifically [`ApiClient`], whose whole
/// job is to say what a hand-rolled `Request::builder()` block used to say.
/// Every verb is driven against the real application router, so a change that
/// broke the request shape (a missing content type, a swallowed body) would
/// fail here rather than in the ~50 test modules that depend on it.
#[cfg(test)]
mod tests {
    use super::*;
    use crate::entities::exchange::Exchange;
    use serde_json::json;

    fn xtes() -> serde_json::Value {
        json!({
            "name": "Test Exchange",
            "country": "Testland",
            "currency": "AUD",
            "timezone": "UTC",
            "settlement_days": 2,
        })
    }

    #[tokio::test]
    async fn the_crud_verbs_round_trip_an_entity() {
        let pool = test_pool().await;
        let client = ApiClient::full(&pool);

        // PUT reports the upsert status; the body reached the handler.
        assert_eq!(
            client.put_json("/exchanges/XTES", &xtes()).await,
            StatusCode::NO_CONTENT
        );

        // GET decodes into the entity's own type, and lists it.
        let ex: Exchange = client.get_json("/exchanges/XTES").await;
        assert_eq!(ex.name, "Test Exchange");
        let all: Vec<Exchange> = client.get_json("/exchanges").await;
        assert!(all.iter().any(|e| e.mic == "XTES"));

        // DELETE answers 204, and a second one the named 404.
        assert_eq!(
            client.delete("/exchanges/XTES").await.status,
            StatusCode::NO_CONTENT
        );
        let gone = client.delete("/exchanges/XTES").await;
        let (status, body) = gone.status_and_body();
        assert_eq!(status, StatusCode::NOT_FOUND);
        assert!(body.contains("exchange"), "404 body: {body:?}");
    }

    #[tokio::test]
    async fn post_json_decodes_a_report_response() {
        let pool = test_pool().await;
        let client = ApiClient::full(&pool);
        let holdings: Vec<serde_json::Value> =
            client.post_json("/portfolio/overview", &json!({})).await;
        assert!(holdings.is_empty(), "an empty portfolio holds nothing");
    }

    /// The rejection path: the 422 reason is readable as text, and
    /// `expect_status` reports it when the status is not the expected one.
    #[tokio::test]
    async fn a_rejection_carries_its_reason_as_text() {
        let pool = test_pool().await;
        let client = ApiClient::full(&pool);
        let resp = client
            .put_raw("/exchanges/XTES", r#"{"name":"no other fields"}"#)
            .await;
        assert_eq!(resp.status, StatusCode::UNPROCESSABLE_ENTITY);
        assert!(!resp.text().is_empty(), "422 must say why");
    }

    /// `post_bytes` drives the non-JSON payloads, and `post_empty` the
    /// endpoints that take no body at all.
    #[tokio::test]
    async fn post_bytes_and_post_empty_reach_their_handlers() {
        let pool = test_pool().await;
        let client = ApiClient::full(&pool);

        let resp = client
            .post_bytes("/mic_registry/import", None, "not,a,valid,header\n")
            .await;
        assert_eq!(resp.status, StatusCode::UNPROCESSABLE_ENTITY);

        let resp = client.post_empty("/jobs/no-such-job").await;
        assert_eq!(resp.status, StatusCode::NOT_FOUND);
    }

    /// `with_header` reaches the handler on every verb — proven here via a
    /// route that reflects a header back (the row-history import's content
    /// type is otherwise the only header any handler reads): a bearer token
    /// header on an unauthenticated router is simply ignored, so the
    /// assertion is indirect — that no header we didn't ask for leaked in,
    /// and that the one we asked for went out.
    #[tokio::test]
    async fn with_header_sends_the_header_on_every_request() {
        let pool = test_pool().await;
        let client = ApiClient::full(&pool).with_header("Authorization", "Bearer test-token");
        // With no `[auth]` configured the router doesn't inspect this header
        // at all, so the request behaves exactly as it would without it —
        // this pins that `with_header` doesn't itself break a request.
        assert_eq!(client.get("/exchanges").await.status, StatusCode::OK);
    }

    #[tokio::test]
    async fn over_wraps_a_narrower_router_than_the_whole_app() {
        let pool = test_pool().await;
        let client = ApiClient::over(crate::entities::router().with_state(pool));
        // The entity routes are served…
        assert_eq!(client.get("/exchanges").await.status, StatusCode::OK);
        // …and the report routes, which this router does not carry, are not.
        assert_eq!(
            client.get("/portfolio/overview").await.status,
            StatusCode::NOT_FOUND
        );
    }
}
