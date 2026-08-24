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
use std::sync::Arc;
use tower::ServiceExt;

/// Fresh in-memory database with migrations and seed data applied.
///
/// The 46 migration files are replayed **once** per test process, into a
/// template database that is then dumped to a single SQL script (see
/// [`schema_template`]); every call after the first builds its database from
/// that script instead. Applying the migrations costs ~89 ms against the
/// script's ~4 ms, and the suite calls this ~1500 times — over three
/// CPU-minutes spent re-deriving one fixed schema, which the test parallelism
/// was merely hiding (23 s wall clock over 267 s of CPU).
///
/// The script is the *whole* database, not just its DDL: seed rows,
/// `_sqlx_migrations` (`infra::db`'s backup verification compares it against a
/// backup's), and `sqlite_sequence` (migration 0045 seeds it for the audited
/// `AUTOINCREMENT` tables, so a server-assigned id never reuses a deleted
/// row's). `cached_schema_matches_the_migrated_schema` pins the two databases
/// against each other, object by object and row by row.
pub async fn test_pool() -> SqlitePool {
    let pool = db::unmigrated_pool(":memory:").await.unwrap();
    sqlx::raw_sql(sqlx::AssertSqlSafe(schema_template().await))
        .execute(&pool)
        .await
        .expect("the captured schema replays");
    pool
}

/// A file-backed pool (WAL, migrations run) — what `main` opens, and the one
/// thing a **concurrency** test cannot take from [`test_pool`]. A `:memory:`
/// database is shared-cache (sqlx configures it so): several connections share
/// one database, but a reader on a second connection *blocks* on an open
/// writer there, so the read/write interleave such a test is about cannot arise
/// at all and it would pass against the very code it exists to refuse. Under
/// WAL a reader sees the snapshot it began with while another connection
/// commits past it, which is the real behaviour.
///
/// The caller owns the `TempDir` — hold it for the length of the test, since
/// dropping it deletes the database out from under the pool. Migrations are
/// run rather than replayed from the cached script: this is one pool per test,
/// not the ~1500 [`test_pool`] builds the cache exists for.
pub async fn race_pool(dir: &tempfile::TempDir) -> SqlitePool {
    let path = dir.path().join("race.db");
    db::init(&path.to_string_lossy())
        .await
        .expect("a file-backed pool")
}

/// The captured schema script, built on first use and shared by every later
/// [`test_pool`] call in this process.
///
/// Held as an `Arc<str>` because that is the one shape `sqlx::AssertSqlSafe`
/// takes without copying the whole script on every call.
static SCHEMA_TEMPLATE: tokio::sync::OnceCell<Arc<str>> = tokio::sync::OnceCell::const_new();

async fn schema_template() -> Arc<str> {
    SCHEMA_TEMPLATE
        .get_or_init(|| async {
            let pool = db::init(":memory:").await.unwrap();
            let script: Arc<str> = dump_database(&pool).await.into();
            pool.close().await;
            script
        })
        .await
        .clone()
}

/// Dump a whole SQLite database — schema *and* contents — as a replayable SQL
/// script, in the order `sqlite3 .dump` uses: every table's definition, then
/// every table's rows, then the indexes, triggers and views.
///
/// Creating the triggers only after the data is loaded is deliberate: the
/// audit-trail and snapshot-staleness triggers must not fire while the seed
/// rows are being replayed, or the rebuilt database would differ from the
/// migrated one it is standing in for.
///
/// `quote()` renders each value as the SQL literal that reads back identically
/// — NULLs, text needing escaping, and `_sqlx_migrations`' checksum blobs
/// included — so the script is exact rather than a formatted approximation.
async fn dump_database(pool: &SqlitePool) -> String {
    let mut out = String::new();

    let tables: Vec<(String, String)> = sqlx::query_as(
        "SELECT name, sql FROM sqlite_master \
         WHERE type = 'table' AND sql IS NOT NULL ORDER BY rowid",
    )
    .fetch_all(pool)
    .await
    .unwrap();

    for (name, sql) in &tables {
        // SQLite creates and owns this one; it appears the moment the first
        // AUTOINCREMENT table does, so replaying its CREATE would fail.
        if name != SEQUENCE_TABLE {
            out.push_str(sql);
            out.push_str(";\n");
        }
    }

    for (name, _) in tables.iter().filter(|(n, _)| n != SEQUENCE_TABLE) {
        push_rows(pool, name, &mut out).await;
    }
    // Last: loading the seed rows above bumps the sequence counters of the
    // AUTOINCREMENT tables among them, so the captured values are restored
    // over the top rather than beside them.
    if tables.iter().any(|(n, _)| n == SEQUENCE_TABLE) {
        out.push_str("DELETE FROM sqlite_sequence;\n");
        push_rows(pool, SEQUENCE_TABLE, &mut out).await;
    }

    let rest: Vec<String> = sqlx::query_scalar(
        "SELECT sql FROM sqlite_master \
         WHERE type IN ('index', 'trigger', 'view') AND sql IS NOT NULL ORDER BY rowid",
    )
    .fetch_all(pool)
    .await
    .unwrap();
    for sql in &rest {
        out.push_str(sql);
        out.push_str(";\n");
    }

    out
}

/// SQLite's own AUTOINCREMENT bookkeeping table.
const SEQUENCE_TABLE: &str = "sqlite_sequence";

/// Append `table`'s rows as `INSERT` statements, batched so a 160-row seed
/// calendar costs a handful of statements rather than 160.
async fn push_rows(pool: &SqlitePool, table: &str, out: &mut String) {
    let columns: Vec<String> = sqlx::query_scalar("SELECT name FROM pragma_table_info(?)")
        .bind(table)
        .fetch_all(pool)
        .await
        .unwrap();
    let quoted: Vec<String> = columns.iter().map(|c| quote_ident(c)).collect();
    let tuple = quoted
        .iter()
        .map(|c| format!("quote({c})"))
        .collect::<Vec<_>>()
        .join(" || ',' || ");
    // Row order is preserved (`rowid` ascending), so the rebuilt table lays its
    // rows out exactly as the migrated one did.
    let rows: Vec<String> = sqlx::query_scalar(sqlx::AssertSqlSafe(format!(
        "SELECT '(' || {tuple} || ')' FROM {table} ORDER BY rowid",
        table = quote_ident(table)
    )))
    .fetch_all(pool)
    .await
    .unwrap();

    const ROWS_PER_STATEMENT: usize = 128;
    for chunk in rows.chunks(ROWS_PER_STATEMENT) {
        out.push_str(&format!(
            "INSERT INTO {} ({}) VALUES {};\n",
            quote_ident(table),
            quoted.join(","),
            chunk.join(",")
        ));
    }
}

/// A SQLite identifier as a double-quoted literal.
fn quote_ident(name: &str) -> String {
    format!("\"{}\"", name.replace('"', "\"\""))
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

/// A URL nothing is listening on, for driving a feed fetch's failure path: an
/// ephemeral loopback port is bound to learn a free number and dropped again,
/// so connecting to it is refused at once.
///
/// Loopback only — no test reaches the network (see CLAUDE.md's test
/// conventions). Used by the import tests that pin what an unreachable feed
/// records (SCENARIOS T-06).
pub fn unreachable_url(path: &str) -> String {
    let port = std::net::TcpListener::bind("127.0.0.1:0")
        .expect("an ephemeral loopback port should bind")
        .local_addr()
        .expect("a bound listener should have an address")
        .port();
    format!("http://127.0.0.1:{port}/{path}")
}

/// The columns a `GET` hands back that no request body accepts: the row's own
/// id, and the server-owned provenance and derived columns (which operation
/// created the trade, how its settlement date was arrived at, which trade a
/// distribution was reinvested into). Every request body denies unknown
/// fields (SCENARIOS V-a), so a read body is now *refused* — naming the
/// field — rather than silently accepted with these ignored.
///
/// Listed here once because three tests replay a read as a write
/// (`entities::tests::what_a_get_returns_can_be_put_back_unchanged` and the
/// two GST-inclusive round trips), and because the list is the answer to
/// "what does the write not own": a new server-owned column belongs here, and
/// a read field that is *not* server-owned must never be added — the round
/// trip would then stop noticing that the write rejects it.
pub const NOT_CLIENT_WRITABLE: &[&str] = &[
    // The key: the URL carries it.
    "id",
    // Trade provenance — which operation created the parcel or closing Sell.
    "inheritance_id",
    "ess_statement_id",
    "transfer_id",
    "rights_action_id",
    "buyback_action_id",
    "scrip_action_id",
    "demerger_action_id",
    "worthless_action_id",
    // Derived on the trade: how the settlement date was arrived at, and the
    // pre-CGT-testing date an inherited parcel inherits from the deceased.
    "settlement_date_source",
    "deemed_acquisition_date",
    // Links written by the operation that created them, never by a body.
    "reinvestment_trade_id",
    "buyback_trade_id",
    "vest_trade_id",
];

/// A read body with [`NOT_CLIENT_WRITABLE`] (plus `also`, for the key columns
/// an entity spells differently and the fields one route's body does not
/// share with another's) removed — what a `PUT` of that read accepts.
pub fn writable_body(read: &serde_json::Value, also: &[&str]) -> serde_json::Value {
    let mut body = read.clone();
    let object = body.as_object_mut().expect("a read body is a JSON object");
    for field in NOT_CLIENT_WRITABLE.iter().chain(also.iter()) {
        object.remove(*field);
    }
    body
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

/// A listing whose **whole holding** has been closed by an executed
/// worthless-shares recognise — the cheapest of the three whole-holding
/// operations to stand up, and the fixture the parcel-creating write paths'
/// back-dating guard tests hang off (`domain::whole_holding`, SCENARIOS V-d).
///
/// Creates listing `listing_id` (ticker `ticker`) with one 100-unit Buy dated
/// `parcel_date`, a `WorthlessShares` corporate action `action_id` dated
/// `event_date`, and runs `POST /corporate_actions/:id/recognise` over them, so
/// afterwards every parcel of the listing is consumed as at `event_date` and a
/// parcel dated on or before it can never be.
pub async fn recognised_worthless_listing(
    pool: &SqlitePool,
    listing_id: i64,
    ticker: &str,
    parcel_date: NaiveDate,
    action_id: i64,
    event_date: NaiveDate,
) {
    listing(listing_id)
        .ticker(ticker)
        .security_type(crate::entities::listing::SecurityType::Share)
        .insert(pool)
        .await;
    buy(listing_id * 1000, listing_id)
        .date(parcel_date)
        .settlement(parcel_date)
        .insert(pool)
        .await;
    crate::entities::corporate_action::db_upsert(
        pool,
        &crate::entities::corporate_action::CorporateAction {
            id: action_id,
            listing_id,
            date: event_date,
            kind: crate::entities::corporate_action::ActionKind::WorthlessShares {
                worthless_event: crate::entities::corporate_action::WorthlessEvent::C2Cancellation,
            },
        },
    )
    .await
    .unwrap();
    crate::entities::worthless::db_recognise(pool, action_id)
        .await
        .unwrap();
}

/// A Buy written **straight into `trades`**, bypassing `trade::db_upsert` and
/// therefore its write-time checks.
///
/// The one thing it is for: standing up a state a *write-time guard* has since
/// made unreachable, so the report or the panic layer that exists to cope with
/// a database already in it can still be tested. Two such states so far — the
/// parcel dated on or before an executed whole-holding operation that
/// `reports::rollover_consistency`'s *unconsumed parcel* problem reports
/// (SCENARIOS V-d), and the parcel whose `average_price × quantity` cannot be
/// represented, which `trade::check_amounts` now refuses and the panic layer
/// answers a logged `500` for where it survives from an older build
/// (SCENARIOS W-e). Neither is reachable through any write path any more, so
/// the only way to reproduce one is to write the row the way that older build
/// did. Use [`buy`] for everything else; a fixture that skips the invariants
/// is a fixture that can lie.
pub async fn insert_parcel_bypassing_checks(
    pool: &SqlitePool,
    id: i64,
    listing_id: i64,
    date: NaiveDate,
    quantity: &str,
    price: &str,
) {
    sqlx::query(
        "INSERT INTO trades \
         (id, trade_type, date, settlement_date, settlement_date_source, listing_id, \
          average_price, quantity, currency, brokerage, gst_on_brokerage, \
          brokerage_currency, fx_rate, holding_account_id) \
         VALUES (?, 'Buy', ?, ?, 'stated', ?, ?, ?, 'AUD', '0', '0', 'AUD', '1', 1)",
    )
    .bind(id)
    .bind(date)
    .bind(date)
    .bind(listing_id)
    .bind(price)
    .bind(quantity)
    .execute(pool)
    .await
    .unwrap();
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

/// Buy fixture: 100 units @ 10 AUD on 2024-01-02 (T+2), no brokerage,
/// default holding account.
///
/// The date is a Tuesday and not a seeded exchange holiday: since SCENARIOS
/// S-08 the write path refuses a trade dated on a day its exchange was shut,
/// and 2024-01-01 — the old default — is New Year's Day on both seeded
/// calendars.
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
        let date = ymd(2024, 1, 2);
        TradeBuilder {
            t: trade::Trade {
                id,
                trade_type,
                date,
                settlement_date: date + chrono::Duration::days(2),
                // A fixture spells its settlement date out, so it is a stated
                // one — and stated dates are what the `settlement-recompute`
                // job leaves alone, so no unrelated fixture can be rewritten
                // by a job run under test. Use [`TradeBuilder::settlement_source`]
                // for the cases that are about the provenance itself.
                settlement_date_source: trade::SettlementDateSource::Stated,
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

    /// Sets how the settlement date came to be (default: `Stated`) — for the
    /// tests about the `settlement-recompute` job, which rewrites only
    /// `Computed` ones, and for pinning what a pre-0041 row (`Unrecorded`)
    /// does.
    pub fn settlement_source(mut self, source: trade::SettlementDateSource) -> Self {
        self.t.settlement_date_source = source;
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

/// Every `.rs` file under `src`, with its path relative to `src` using `/`
/// separators (`reports/tax_summary.rs`) — the form the source-scanning tests
/// write their allowlists in.
///
/// The tree has several tests that pin a convention nothing in the type system
/// can (`infra::db`'s deferred-`BEGIN` scan, `infra::decimal`'s
/// stringified-bind scan, `web`'s display-kind scan); they share this walk
/// rather than each carrying its own copy of it.
pub fn rust_sources() -> Vec<(String, String)> {
    let src = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut found = Vec::new();
    let mut walk = vec![src.clone()];
    while let Some(dir) = walk.pop() {
        for entry in std::fs::read_dir(&dir)
            .expect("src should be readable")
            .flatten()
        {
            let path = entry.path();
            if path.is_dir() {
                walk.push(path);
            } else if path.extension().is_some_and(|x| x == "rs") {
                let rel = path
                    .strip_prefix(&src)
                    .expect("under src")
                    .components()
                    .map(|c| c.as_os_str().to_string_lossy().into_owned())
                    .collect::<Vec<_>>()
                    .join("/");
                let body = std::fs::read_to_string(&path).expect("source should be readable");
                found.push((rel, body));
            }
        }
    }
    found.sort();
    found
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

    /// The cached schema `test_pool` builds every database from must be the
    /// database the 46 migrations produce — not approximately, exactly.
    ///
    /// So this builds one of each and compares the **whole** of `sqlite_master`:
    /// every table, index, trigger and view, by name and by definition
    /// (normalised for comments and whitespace). Object type matters as much as
    /// object count here — this schema's correctness leans on its triggers (the
    /// `row_history` audit pair on every audited table, and the
    /// `*_stale_snapshots_*` sets), and a caching scheme that quietly dropped a
    /// trigger would switch the audit trail off in every test while the suite
    /// stayed green.
    ///
    /// Then it compares the two databases' full contents, which is what pins
    /// the parts of the seed a schema comparison cannot see: the seeded
    /// exchanges, holding accounts, currencies and exchange holidays,
    /// `_sqlx_migrations` (`infra::db`'s backup verification compares a
    /// backup's against the live database's), and `sqlite_sequence` (0045 seeds
    /// it so a server-assigned id never reuses a deleted row's).
    #[tokio::test]
    async fn cached_schema_matches_the_migrated_schema() {
        let cached = test_pool().await;
        let migrated = db::init(":memory:").await.unwrap();

        let cached_objects = schema_objects(&cached).await;
        let migrated_objects = schema_objects(&migrated).await;

        // Floors, so a comparison of two empty — or trigger-less — sets can
        // never pass by vacuity. Deliberately well under the real counts
        // (~40 tables, ~50 indexes, ~75 triggers as of migration 0045): the
        // schema only ever grows, and this is a guard, not an inventory.
        for (kind, least) in [("table", 30), ("index", 30), ("trigger", 50)] {
            let found = migrated_objects.iter().filter(|o| o.0 == kind).count();
            assert!(
                found >= least,
                "expected at least {least} {kind}s, got {found}"
            );
        }

        let missing: Vec<_> = migrated_objects
            .iter()
            .filter(|o| !cached_objects.contains(o))
            .collect();
        let extra: Vec<_> = cached_objects
            .iter()
            .filter(|o| !migrated_objects.contains(o))
            .collect();
        assert!(
            missing.is_empty() && extra.is_empty(),
            "cached schema differs from the migrated one\nmissing: {missing:#?}\nextra: {extra:#?}"
        );
        assert_eq!(
            cached_objects.len(),
            migrated_objects.len(),
            "same number of schema objects"
        );

        // …and the contents, row for row, table by table.
        let tables: Vec<String> =
            sqlx::query_scalar("SELECT name FROM sqlite_master WHERE type = 'table' ORDER BY name")
                .fetch_all(&migrated)
                .await
                .unwrap();
        assert!(tables.contains(&"exchange_holidays".to_string()));
        for table in &tables {
            // `_sqlx_migrations` is compared below on the columns that are not
            // per-run measurements.
            if table == "_sqlx_migrations" {
                continue;
            }
            let mut from_cache = String::new();
            let mut from_migrations = String::new();
            push_rows(&cached, table, &mut from_cache).await;
            push_rows(&migrated, table, &mut from_migrations).await;
            assert_eq!(
                from_cache, from_migrations,
                "table {table} differs between the cached and the migrated database"
            );
        }

        // The migrations sqlx recorded must read back the same — same versions,
        // same descriptions, same checksums, all successful. `installed_on` and
        // `execution_time` are excluded on purpose: they are measurements of the
        // run that applied them, and the cached database faithfully carries the
        // template run's rather than inventing new ones.
        let recorded = "SELECT version, description, success, hex(checksum) \
                        FROM _sqlx_migrations ORDER BY version";
        let from_cache: Vec<(i64, String, bool, String)> =
            sqlx::query_as(recorded).fetch_all(&cached).await.unwrap();
        let from_migrations: Vec<(i64, String, bool, String)> =
            sqlx::query_as(recorded).fetch_all(&migrated).await.unwrap();
        assert_eq!(from_cache, from_migrations, "_sqlx_migrations differs");
        assert_eq!(from_cache.len(), 47, "every migration is recorded");

        // Spelled out separately because it is the one piece of state a
        // schema-only cache would silently lose, and two tests in
        // `reports::row_history` depend on it.
        let sequences: Vec<(String, i64)> =
            sqlx::query_as("SELECT name, seq FROM sqlite_sequence ORDER BY name")
                .fetch_all(&cached)
                .await
                .unwrap();
        assert!(
            sequences.len() >= 17,
            "0045 seeds sqlite_sequence for the audited tables, got {sequences:?}"
        );
    }

    /// Every row of `sqlite_master` as `(type, name, tbl_name, normalised sql)`,
    /// sorted so the two databases can be compared as sets. `rootpage` is left
    /// out — it is a physical page number, not part of the schema. Entries with
    /// no `sql` (SQLite's own `sqlite_autoindex_*` behind a UNIQUE/PRIMARY KEY)
    /// are kept: losing a uniqueness constraint would show up here and nowhere
    /// else.
    async fn schema_objects(pool: &SqlitePool) -> Vec<(String, String, String, String)> {
        let rows: Vec<(String, String, String, Option<String>)> =
            sqlx::query_as("SELECT type, name, tbl_name, sql FROM sqlite_master")
                .fetch_all(pool)
                .await
                .unwrap();
        let mut objects: Vec<(String, String, String, String)> = rows
            .into_iter()
            .map(|(t, n, tbl, sql)| {
                (
                    t,
                    n,
                    tbl,
                    sql.as_deref().map(normalise_sql).unwrap_or_default(),
                )
            })
            .collect();
        objects.sort();
        objects
    }

    /// A DDL statement with its comments removed and its whitespace collapsed,
    /// so two spellings of the same definition compare equal. (String literals
    /// containing `--` would be mangled too, but identically on both sides, so
    /// the comparison stays sound.)
    fn normalise_sql(sql: &str) -> String {
        let mut out = String::with_capacity(sql.len());
        let mut rest = sql;
        while let Some(cut) = rest.find("--").or_else(|| rest.find("/*")) {
            out.push_str(&rest[..cut]);
            out.push(' ');
            rest = if rest[cut..].starts_with("--") {
                rest[cut..].find('\n').map_or("", |e| &rest[cut + e..])
            } else {
                rest[cut..].find("*/").map_or("", |e| &rest[cut + e + 2..])
            };
        }
        out.push_str(rest);
        out.split_whitespace().collect::<Vec<_>>().join(" ")
    }

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
