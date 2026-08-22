//! Daily report snapshots: persisted results of the price-dependent reports
//! (portfolio overview, unrealised gains, performance), one stored row per
//! (report, date) in `report_snapshots`.
//!
//! # Semantics
//!
//! A snapshot for date D is the report run against the facts recorded *as at
//! D* (the reports' `as_of` filtering) and valued at D's stored closing
//! prices, each converted from the listing's quote currency to AUD at the
//! valuation FX rate (`infra::fx::resolve_valuation_rate`: the ATO monthly
//! rate when imported, else the bounded earlier-month fallback). Generation
//! refuses to run unless **every** listing held on D has a final, ok stored
//! price for its nearest trading day on or before D — a day whose price
//! fetches failed therefore has **no** snapshot (missing) until the price
//! re-run succeeds, which is distinguishable from a **stale** one.
//!
//! Staleness: recording a back-dated fact marks every snapshot dated on or
//! after the fact stale, atomically with the fact write — enforced by the
//! `*_stale_snapshots_*` triggers (0001_schema.sql), so no write path can
//! bypass it. A stale snapshot keeps showing its stored result (flagged)
//! until regenerated on demand via `POST /report_snapshots/generate`, which
//! re-runs the reports with the stored prices and the *new* facts. Which
//! tables must carry that trigger set is pinned by this module's
//! `every_table_is_classified_for_snapshot_staleness` test — the sibling of
//! `reports::row_history::AUDITED_TABLES`: every table in the live schema is
//! either listed with the staleness triggers it is required to carry, or
//! listed exempt with the reason a write to it can invalidate no snapshot, and
//! a table in neither list fails the test.
//!
//! Provisional (migration 0015, distinct from stale): a snapshot whose run
//! converted any price at a fallback-month FX rate — the valuation month's
//! rate was not published yet — is stored flagged `provisional`. Nothing
//! stales it when the real rate lands (FX imports fire no staleness
//! triggers); instead the RBA import true-up and the scheduled job's window
//! regenerate provisional snapshots, and a run whose conversions all used
//! real-month rates clears the flag.
//!
//! Carried-forward price (migration 0035, distinct from both): a listing the
//! price provider has stopped quoting (`listings.unpriced_from` — a delisting
//! or a long suspension) is valued at its last stored ok close rather than
//! blocking the whole portfolio's date forever, and the snapshot is stored
//! flagged `price_carried_forward` with the affected report rows carrying the
//! same flag. Deliberately **not** folded into `provisional`: that flag means
//! an interim FX rate a later import trues up, and the true-up runs target
//! provisional dates — a carried-forward price never clears, so conflating
//! the two would turn a bounded true-up into one that regenerates the same
//! dates forever. Nothing regenerates a carried-forward snapshot; clearing
//! `unpriced_from` (the security relists) stales every snapshot from that
//! date on, which is what puts the real prices back.
//!
//! Excluded holding (migration 0037, distinct from all three): a listing
//! dated before its `listings.unpriced_before` — the day the price
//! provider's series begins — has no obtainable price at all, so it is
//! **left out** of the date's totals rather than blocking them, and the
//! snapshot is stored flagged `holding_excluded` with `excluded_holdings`
//! naming each absent holding and why. This is a stronger statement than the
//! other two flags: they say the figure rests on an interim input, this one
//! says the figure is **missing a holding** — hence its own flag *and* its
//! own list. It is kept out of `provisional` for the same bounded-true-up
//! reason `price_carried_forward` is, and out of `price_carried_forward`
//! because the two mean different things to a reader. A date on which every
//! held listing is excluded is blocked, not stored empty.
//!
//! The scheduled `report-snapshot` job catches up instead of targeting one
//! date: each run generates every missing snapshot date in a bounded lookback
//! window (capped at [`CATCHUP_LOOKBACK_DAYS`], never reaching before the
//! series' first stored snapshot) up to the latest date every held listing
//! can be valued at with final prices, and regenerates stale or provisional
//! snapshots in the window. A date still blocked (missing/errored price) is
//! skipped with its blocker in the job failure detail and retried on later
//! runs — a late price delays that date's snapshot, never loses it. Past
//! dates outside the window are generated on demand via the same endpoint, or
//! in bulk via `POST /report_snapshots/regenerate_all` /
//! `POST /report_snapshots/regenerate_provisional`.

use crate::infra::http::ApiError;
use axum::{
    Json, Router,
    extract::{Path, Query, State},
    routing::{get, post},
};
use chrono::{DateTime, Duration, NaiveDate, Utc};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use sqlx::{QueryBuilder, Row, SqlitePool, sqlite::SqliteRow};
use std::collections::{BTreeSet, HashMap, HashSet};

use crate::entities::closing_price::{self, Market};
use crate::reports::{performance, portfolio, unrealised_gains, valuation};

/// The price-dependent reports that are snapshotted daily.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, sqlx::Type)]
#[sqlx(rename_all = "snake_case")]
#[serde(rename_all = "snake_case")]
pub enum ReportKind {
    PortfolioOverview,
    UnrealisedGains,
    Performance,
}

impl ReportKind {
    pub const ALL: [ReportKind; 3] = [
        ReportKind::PortfolioOverview,
        ReportKind::UnrealisedGains,
        ReportKind::Performance,
    ];

    pub fn slug(self) -> &'static str {
        match self {
            ReportKind::PortfolioOverview => "portfolio_overview",
            ReportKind::UnrealisedGains => "unrealised_gains",
            ReportKind::Performance => "performance",
        }
    }

    fn from_slug(slug: &str) -> Option<ReportKind> {
        ReportKind::ALL.into_iter().find(|k| k.slug() == slug)
    }
}

/// One stored snapshot's metadata (the result rows are fetched separately —
/// lists never carry the JSON payload).
#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct SnapshotMeta {
    pub report: ReportKind,
    pub snapshot_date: NaiveDate,
    /// RFC 3339 UTC timestamp of the run that produced the stored result.
    pub generated_at: String,
    /// A back-dated fact was recorded after generation: the stored result no
    /// longer reflects the books; regenerate on demand.
    pub stale: bool,
    /// The stored result was valued with a fallback-month FX rate (the
    /// valuation month's rate was not published at generation); regeneration
    /// with all real rates clears it.
    pub provisional: bool,
    /// A held listing was valued at a **carried-forward** close because the
    /// provider has stopped quoting it (`listings.unpriced_from`). Unlike
    /// `provisional` no later fact clears this, so nothing retries it
    /// (SCENARIOS Q-02).
    pub price_carried_forward: bool,
    /// The stored totals **omit** a held holding: no price is obtainable for
    /// it at this date (`listings.unpriced_before`). Like
    /// `price_carried_forward` nothing retries it; clearing the listing's
    /// marker stales these dates instead (migration 0037).
    pub holding_excluded: bool,
    /// Which holdings the totals omit, and why — the run's own resolution,
    /// stored with the result so it stays readable after the listing's marker
    /// moves. Empty unless `holding_excluded`.
    #[sqlx(try_from = "String")]
    pub excluded_holdings: ExcludedHoldings,
}

/// The stored `report_snapshots.excluded_holdings` JSON array, as the sqlx
/// `try_from` newtype the `FromRow` derives read it through (a `Vec` of a
/// local type cannot implement `TryFrom<String>` itself). Serialises as the
/// bare array, so the API shape is a list, not a wrapper.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ExcludedHoldings(pub Vec<valuation::ExcludedHolding>);

impl TryFrom<String> for ExcludedHoldings {
    type Error = serde_json::Error;

    fn try_from(json: String) -> Result<Self, Self::Error> {
        serde_json::from_str(&json).map(ExcludedHoldings)
    }
}

/// A full snapshot: metadata plus the report's stored response rows.
#[derive(Debug, Serialize, Deserialize)]
pub struct Snapshot {
    pub report: ReportKind,
    pub snapshot_date: NaiveDate,
    pub generated_at: String,
    pub stale: bool,
    pub provisional: bool,
    pub price_carried_forward: bool,
    pub holding_excluded: bool,
    pub excluded_holdings: ExcludedHoldings,
    /// The report's rows exactly as the live endpoint would have returned
    /// them at generation time (money values are Decimal strings).
    pub rows: serde_json::Value,
}

/// One point of the snapshot time series (from the unrealised-gains
/// snapshots): portfolio-total AUD figures for graphing market value and
/// unrealised gain over time.
#[derive(Debug, Serialize, Deserialize)]
pub struct SeriesPoint {
    pub snapshot_date: NaiveDate,
    pub stale: bool,
    pub provisional: bool,
    pub price_carried_forward: bool,
    /// This point's totals omit a held holding (`listings.unpriced_before`):
    /// the graph **steps** where the excluded listing's own series begins,
    /// and that step is a change in what is being measured, not in value —
    /// which is why the point carries the reason alongside the flag.
    pub holding_excluded: bool,
    pub excluded_holdings: ExcludedHoldings,
    pub market_value: Decimal,
    pub total_cost_base: Decimal,
    pub unrealised_gain: Decimal,
}

/// Why a snapshot could not be generated. `Unprocessable` carries the human
/// detail (which listing's price is missing/errored, an unconvertible
/// currency, a close that is not final yet) and maps to HTTP 422; anything
/// else is a 500.
#[derive(thiserror::Error, Debug)]
pub enum GenerateError {
    #[error("{0}")]
    Unprocessable(String),
    #[error("{0}")]
    Db(String),
}

/// The `Db` arm keeps the message rather than the `sqlx::Error` itself: the
/// same variant also carries generation failures with no `sqlx::Error` behind
/// them (a missing price row, a `ValuationError::Db`).
impl From<sqlx::Error> for GenerateError {
    fn from(e: sqlx::Error) -> Self {
        GenerateError::Db(e.to_string())
    }
}

impl From<valuation::ValuationError> for GenerateError {
    fn from(e: valuation::ValuationError) -> Self {
        match e {
            valuation::ValuationError::Unprocessable(msg) => GenerateError::Unprocessable(msg),
            valuation::ValuationError::Db(msg) => GenerateError::Db(msg),
        }
    }
}

// ---------------------------------------------------------------------------
// DB access
// ---------------------------------------------------------------------------

/// Read the `excluded_holdings` JSON column off a row, for the two reads
/// whose `FromRow` a derive cannot express (`db_get` attaches `rows_json`,
/// `db_series` computes its totals from it). Same codec as the derive's
/// `try_from`, so the two paths cannot drift.
fn excluded_holdings(row: &SqliteRow) -> Result<ExcludedHoldings, sqlx::Error> {
    let json: String = row.try_get("excluded_holdings")?;
    ExcludedHoldings::try_from(json)
        .map_err(|e| sqlx::Error::Decode(format!("malformed excluded_holdings: {e}").into()))
}

/// Stored snapshot metadata, oldest first, optionally filtered.
pub async fn db_list(
    pool: &SqlitePool,
    report: Option<ReportKind>,
    from: Option<NaiveDate>,
    to: Option<NaiveDate>,
) -> Result<Vec<SnapshotMeta>, sqlx::Error> {
    let mut qb = QueryBuilder::new(
        "SELECT report, snapshot_date, generated_at, stale, provisional, price_carried_forward, \
                holding_excluded, excluded_holdings \
         FROM report_snapshots WHERE 1=1",
    );
    if let Some(report) = report {
        qb.push(" AND report = ").push_bind(report);
    }
    if let Some(from) = from {
        qb.push(" AND snapshot_date >= ").push_bind(from);
    }
    if let Some(to) = to {
        qb.push(" AND snapshot_date <= ").push_bind(to);
    }
    qb.push(" ORDER BY snapshot_date, report");
    qb.build_query_as().fetch_all(pool).await
}

pub async fn db_get(
    pool: &SqlitePool,
    report: ReportKind,
    date: NaiveDate,
) -> Result<Option<Snapshot>, sqlx::Error> {
    let row = sqlx::query(
        "SELECT report, snapshot_date, generated_at, stale, provisional, price_carried_forward, \
                holding_excluded, excluded_holdings, rows_json \
         FROM report_snapshots WHERE report = ? AND snapshot_date = ?",
    )
    .bind(report)
    .bind(date)
    .fetch_optional(pool)
    .await?;
    row.map(|row| {
        let rows_json: String = row.try_get("rows_json")?;
        Ok(Snapshot {
            report: row.try_get("report")?,
            snapshot_date: row.try_get("snapshot_date")?,
            generated_at: row.try_get("generated_at")?,
            stale: row.try_get("stale")?,
            provisional: row.try_get("provisional")?,
            price_carried_forward: row.try_get("price_carried_forward")?,
            holding_excluded: row.try_get("holding_excluded")?,
            excluded_holdings: excluded_holdings(&row)?,
            rows: serde_json::from_str(&rows_json)
                .map_err(|e| sqlx::Error::Decode(format!("malformed rows_json: {e}").into()))?,
        })
    })
    .transpose()
}

/// The graphable time series: per snapshot date, the unrealised-gains
/// snapshot's portfolio totals (every held listing is priced at generation,
/// so the sums cover the whole portfolio). Oldest first.
pub async fn db_series(pool: &SqlitePool) -> Result<Vec<SeriesPoint>, sqlx::Error> {
    let rows = sqlx::query(
        "SELECT snapshot_date, stale, provisional, price_carried_forward, holding_excluded, \
                excluded_holdings, rows_json \
         FROM report_snapshots \
         WHERE report = 'unrealised_gains' ORDER BY snapshot_date",
    )
    .fetch_all(pool)
    .await?;

    let mut series = Vec::with_capacity(rows.len());
    for row in &rows {
        let rows_json: String = row.try_get("rows_json")?;
        let gains: Vec<unrealised_gains::UnrealisedGain> = serde_json::from_str(&rows_json)
            .map_err(|e| sqlx::Error::Decode(format!("malformed rows_json: {e}").into()))?;
        let mut point = SeriesPoint {
            snapshot_date: row.try_get("snapshot_date")?,
            stale: row.try_get("stale")?,
            provisional: row.try_get("provisional")?,
            price_carried_forward: row.try_get("price_carried_forward")?,
            holding_excluded: row.try_get("holding_excluded")?,
            excluded_holdings: excluded_holdings(row)?,
            market_value: Decimal::ZERO,
            total_cost_base: Decimal::ZERO,
            unrealised_gain: Decimal::ZERO,
        };
        for g in &gains {
            point.market_value += g.market_value.unwrap_or(Decimal::ZERO);
            point.total_cost_base += g.total_cost_base;
            point.unrealised_gain += g.unrealised_gain_loss.unwrap_or(Decimal::ZERO);
        }
        series.push(point);
    }
    Ok(series)
}

// ---------------------------------------------------------------------------
// Generation
// ---------------------------------------------------------------------------

/// The latest calendar date the whole portfolio can be valued at with final
/// prices at `now`: starting from the most advanced market's latest complete
/// trading day, walk back until every held listing's valuation day (its
/// nearest trading day at or before the candidate) has a final close. `None`
/// when nothing is held.
pub async fn latest_snapshot_date(
    pool: &SqlitePool,
    now: DateTime<Utc>,
) -> Result<Option<NaiveDate>, GenerateError> {
    let markets = valuation::held_markets(pool, None).await?;
    let mut pairs: Vec<(&Market, NaiveDate)> = Vec::with_capacity(markets.len());
    for m in &markets {
        let latest = m
            .latest_complete_trading_day(now)
            .map_err(GenerateError::Db)?
            .ok_or_else(|| {
                GenerateError::Db(format!(
                    "listing {} has no trading day in the past year",
                    m.listing.ticker
                ))
            })?;
        pairs.push((m, latest));
    }
    let Some(&(_, mut candidate)) = pairs.iter().max_by_key(|(_, latest)| *latest) else {
        return Ok(None);
    };
    let floor = pairs
        .iter()
        .map(|&(_, latest)| latest)
        .min()
        .unwrap_or(candidate);
    while candidate > floor {
        let all_final = pairs.iter().all(|&(m, latest)| {
            m.latest_trading_day_on_or_before(candidate)
                .is_some_and(|t| t <= latest)
        });
        if all_final {
            return Ok(Some(candidate));
        }
        candidate -= Duration::days(1);
    }
    // The floor always qualifies: every market's valuation day for it is at
    // or before its own latest complete trading day.
    Ok(Some(floor))
}

/// The bulk-regeneration bounds to use when the caller gives none: the first
/// date anything was ever held (a Buy/DRP trade date — the same "held"
/// definition as [`closing_price::db_held_listing_ids`]) through the latest
/// date the portfolio can be valued at with final prices. Both `None` when
/// nothing has ever been held.
#[derive(Debug, Serialize, Deserialize)]
pub struct RegenerateRange {
    pub from: Option<NaiveDate>,
    pub to: Option<NaiveDate>,
}

pub async fn default_regenerate_range(
    pool: &SqlitePool,
    now: DateTime<Utc>,
) -> Result<RegenerateRange, GenerateError> {
    let from: Option<NaiveDate> =
        sqlx::query_scalar("SELECT MIN(date) FROM trades WHERE trade_type IN ('Buy', 'DRP')")
            .fetch_one(pool)
            .await
            .map_err(GenerateError::from)?;
    let to = latest_snapshot_date(pool, now).await?;
    Ok(RegenerateRange { from, to })
}

/// What one date's generation run resolved: the AUD price of every listing
/// held as at it (from `valuation::stored_valuations`), plus the two
/// per-listing caveats the stored snapshot and its rows must carry.
struct PricedListings {
    prices: HashMap<i64, Decimal>,
    /// Listings whose AUD conversion used a fallback-month rate — non-empty
    /// makes the snapshot `provisional`.
    fx_provisional: HashSet<i64>,
    /// Listings valued at a carried-forward close because the provider has
    /// stopped quoting them — non-empty makes the snapshot
    /// `price_carried_forward` (SCENARIOS Q-02). Kept apart from
    /// `fx_provisional` because only that one is ever trued up.
    carried_forward: HashSet<i64>,
    /// Held listings left out of the valuation entirely because no price is
    /// obtainable for them at this date (`listings.unpriced_before`) —
    /// non-empty makes the snapshot `holding_excluded` and is stored verbatim
    /// as its `excluded_holdings` (migration 0037).
    excluded: Vec<valuation::ExcludedHolding>,
}

async fn aud_prices_for(
    pool: &SqlitePool,
    date: NaiveDate,
    now: DateTime<Utc>,
) -> Result<PricedListings, GenerateError> {
    let resolved_valuations = valuation::stored_valuations(pool, date, now).await?;
    let mut resolved = PricedListings {
        prices: HashMap::new(),
        fx_provisional: HashSet::new(),
        carried_forward: HashSet::new(),
        excluded: resolved_valuations.excluded,
    };
    for v in resolved_valuations.valuations {
        resolved.prices.insert(v.listing_id, v.aud_price);
        if v.provisional {
            resolved.fx_provisional.insert(v.listing_id);
        }
        if v.price_carried_forward {
            resolved.carried_forward.insert(v.listing_id);
        }
    }
    Ok(resolved)
}

/// Generate (or regenerate) the three snapshots for `date` and store them in
/// one transaction, replacing any stored result and clearing its stale flag.
/// The stored `provisional` flag is set iff any price conversion in this run
/// used a fallback-month FX rate — so regenerating once the real rate is
/// imported clears it. The stored `price_carried_forward` flag is set iff any
/// held listing was valued at a carried-forward close (SCENARIOS Q-02);
/// nothing trues that one up, and a regeneration reproduces it until the
/// listing's `unpriced_from` is cleared. The stored `holding_excluded` flag
/// and its `excluded_holdings` list are set iff any held listing had no
/// obtainable price at all (`listings.unpriced_before`, migration 0037): the
/// totals omit it, its report rows carry the reason as `price_unavailable`,
/// and clearing the listing's marker is what brings it back.
pub async fn generate(
    pool: &SqlitePool,
    date: NaiveDate,
    now: DateTime<Utc>,
) -> Result<Vec<SnapshotMeta>, GenerateError> {
    let resolved = aud_prices_for(pool, date, now).await?;
    let prices = resolved.prices;
    let provisional = !resolved.fx_provisional.is_empty();
    let price_carried_forward = !resolved.carried_forward.is_empty();
    let excluded_holdings = ExcludedHoldings(resolved.excluded);
    let holding_excluded = !excluded_holdings.0.is_empty();
    // An excluded holding leaves its report rows unvalued carrying the
    // reason, exactly as a failed live quote does — the row-level counterpart
    // of the snapshot-level list, so a reader of the rows alone still sees
    // which holding is absent and why.
    let excluded_reason: HashMap<i64, &str> = excluded_holdings
        .0
        .iter()
        .map(|x| (x.listing_id, x.reason.as_str()))
        .collect();

    let mut overview = portfolio::db_holdings(pool, Some(date)).await?;
    for h in &mut overview {
        if let Some(&price) = prices.get(&h.listing_id) {
            h.current_price = Some(price);
            h.market_value = Some(h.quantity * price);
            h.fx_provisional = resolved.fx_provisional.contains(&h.listing_id);
            h.price_carried_forward = resolved.carried_forward.contains(&h.listing_id);
        } else if let Some(reason) = excluded_reason.get(&h.listing_id) {
            h.price_unavailable = Some((*reason).to_string());
        }
    }
    let mut gains = unrealised_gains::db_unrealised_gains(pool, date).await?;
    for g in &mut gains {
        if let Some(&price) = prices.get(&g.listing_id) {
            g.current_price = Some(price);
            g.market_value = Some(g.quantity * price);
            g.unrealised_gain_loss = Some(g.quantity * price - g.total_cost_base);
            g.fx_provisional = resolved.fx_provisional.contains(&g.listing_id);
            g.price_carried_forward = resolved.carried_forward.contains(&g.listing_id);
        } else if let Some(reason) = excluded_reason.get(&g.listing_id) {
            g.price_unavailable = Some((*reason).to_string());
        }
    }
    let mut perf = performance::db_performance(pool, &prices, date).await?;
    for row in &mut perf {
        if let Some(listing_id) = row.listing_id {
            row.fx_provisional = resolved.fx_provisional.contains(&listing_id);
            row.price_carried_forward = resolved.carried_forward.contains(&listing_id);
            if let Some(reason) = excluded_reason.get(&listing_id) {
                row.price_unavailable = Some((*reason).to_string());
            }
        }
    }

    let to_json = |kind: ReportKind, value: serde_json::Result<String>| {
        value
            .map(|json| (kind, json))
            .map_err(|e| GenerateError::Db(e.to_string()))
    };
    let payloads = [
        to_json(
            ReportKind::PortfolioOverview,
            serde_json::to_string(&overview),
        )?,
        to_json(ReportKind::UnrealisedGains, serde_json::to_string(&gains))?,
        to_json(ReportKind::Performance, serde_json::to_string(&perf))?,
    ];

    let generated_at = Utc::now().to_rfc3339();
    let excluded_json =
        serde_json::to_string(&excluded_holdings).map_err(|e| GenerateError::Db(e.to_string()))?;
    let mut tx = pool.begin().await?;
    for (kind, rows_json) in &payloads {
        sqlx::query(
            "INSERT INTO report_snapshots \
                 (report, snapshot_date, generated_at, stale, provisional, \
                  price_carried_forward, holding_excluded, excluded_holdings, rows_json) \
             VALUES (?, ?, ?, 0, ?, ?, ?, ?, ?) \
             ON CONFLICT(report, snapshot_date) DO UPDATE SET \
                 generated_at = excluded.generated_at, \
                 stale = 0, \
                 provisional = excluded.provisional, \
                 price_carried_forward = excluded.price_carried_forward, \
                 holding_excluded = excluded.holding_excluded, \
                 excluded_holdings = excluded.excluded_holdings, \
                 rows_json = excluded.rows_json",
        )
        .bind(kind)
        .bind(date)
        .bind(&generated_at)
        .bind(provisional)
        .bind(price_carried_forward)
        .bind(holding_excluded)
        .bind(&excluded_json)
        .bind(rows_json)
        .execute(&mut *tx)
        .await?;
    }
    tx.commit().await?;

    Ok(payloads
        .iter()
        .map(|&(report, _)| SnapshotMeta {
            report,
            snapshot_date: date,
            generated_at: generated_at.clone(),
            stale: false,
            provisional,
            price_carried_forward,
            holding_excluded,
            excluded_holdings: excluded_holdings.clone(),
        })
        .collect())
}

/// How far back one scheduled run reaches to backfill missing snapshot dates
/// and regenerate stale/provisional ones. Dates older than this are repaired
/// on demand (`generate`, `regenerate_all`) or by the RBA-import true-up
/// (`regenerate_provisional`), not by the daily job.
///
/// Deliberately the *same* number as price collection's lookback rather than
/// an independent one: a date this job keeps retrying but collection no longer
/// refills is a date that can never unblock itself, so the two windows are one
/// constant (`closing_price::COLLECTION_LOOKBACK_DAYS`).
pub const CATCHUP_LOOKBACK_DAYS: i64 = closing_price::COLLECTION_LOOKBACK_DAYS;

/// Whether the stored metadata for one date needs (re)generation: anything
/// short of all three reports stored fresh and final does.
///
/// `price_carried_forward` and `holding_excluded` are deliberately **not** in
/// the list: unlike a provisional FX rate, no later fact turns a
/// carried-forward close into a real one, and nothing ever makes a price
/// exist for a day before the provider's series begins — so retrying either
/// every run would regenerate the same figures forever. Clearing the
/// listing's `unpriced_from` / `unpriced_before` stales those dates instead,
/// which is what brings them back through here (SCENARIOS Q-02, migration
/// 0037).
fn needs_generation(metas: &[&SnapshotMeta]) -> bool {
    metas.len() < ReportKind::ALL.len() || metas.iter().any(|m| m.stale || m.provisional)
}

/// One scheduled run: catch up over a bounded window instead of targeting one
/// date. Every missing snapshot date in the window — including an interior
/// hole a blocked date left behind — is generated, and every stored snapshot
/// in it that is stale or provisional is regenerated. The window runs from
/// the first stored snapshot date, capped at [`CATCHUP_LOOKBACK_DAYS`] before
/// the latest fully-valuable date (a fresh database starts at the latest date
/// only), up to that latest date. A date still blocked (missing/errored
/// price, unconvertible currency) is skipped with its blocker in the job
/// failure detail — the other dates still generate — and retried on later
/// runs while it stays in the window. Nothing held (overall, or on an
/// individual window date) is a no-op, not an error.
pub async fn run_snapshot_job(pool: &SqlitePool, now: DateTime<Utc>) -> Result<(), String> {
    let latest = match latest_snapshot_date(pool, now).await {
        Ok(Some(date)) => date,
        Ok(None) => {
            tracing::info!("no holdings — no report snapshot to take");
            return Ok(());
        }
        Err(e) => return Err(e.to_string()),
    };
    let window_start = latest - Duration::days(CATCHUP_LOOKBACK_DAYS - 1);

    // Every date from the first stored snapshot in the window (a fresh
    // database starts at the latest date — the job never backfills before
    // the series began) up to the latest fully-valuable date is a candidate:
    // missing dates include interior holes a blocked date left behind, so a
    // hole keeps being retried while it stays in the window.
    let first_stored: Option<NaiveDate> =
        sqlx::query_scalar("SELECT MIN(snapshot_date) FROM report_snapshots")
            .fetch_one(pool)
            .await
            .map_err(|e| e.to_string())?;
    let candidates_from = match first_stored {
        None => latest,
        Some(first) => first.max(window_start),
    };
    let mut targets: BTreeSet<NaiveDate> = BTreeSet::new();
    let mut date = candidates_from;
    while date <= latest {
        targets.insert(date);
        date += Duration::days(1);
    }

    let metas = db_list(pool, None, Some(window_start), Some(latest))
        .await
        .map_err(|e| e.to_string())?;
    let mut by_date: HashMap<NaiveDate, Vec<&SnapshotMeta>> = HashMap::new();
    for m in &metas {
        by_date.entry(m.snapshot_date).or_default().push(m);
    }

    let mut generated: Vec<NaiveDate> = Vec::new();
    let mut blockers: Vec<String> = Vec::new();
    for date in targets {
        if by_date
            .get(&date)
            .is_some_and(|stored| !needs_generation(stored))
        {
            continue; // already stored fresh and final (e.g. a second run the same day)
        }
        // A window date before the first holding has nothing to snapshot —
        // skip it silently rather than reporting a permanent blocker.
        let held = closing_price::db_held_listing_ids(pool, Some(date))
            .await
            .map_err(|e| e.to_string())?;
        if held.is_empty() {
            continue;
        }
        match generate(pool, date, now).await {
            Ok(_) => generated.push(date),
            Err(GenerateError::Unprocessable(msg)) => blockers.push(format!("{date}: {msg}")),
            Err(GenerateError::Db(msg)) => return Err(format!("snapshot for {date}: {msg}")),
        }
    }

    if generated.is_empty() && blockers.is_empty() {
        tracing::info!(%latest, "report snapshots already stored fresh");
    } else {
        tracing::info!(
            ?generated,
            blocked = blockers.len(),
            "report snapshots stored"
        );
    }
    if blockers.is_empty() {
        Ok(())
    } else {
        Err(format!("blocked snapshot dates: {}", blockers.join("; ")))
    }
}

/// What a bulk regeneration did: the dates regenerated, and the dates that
/// could not be (with each one's blocker) — a blocked date never aborts the
/// others.
#[derive(Debug, Serialize, Deserialize)]
pub struct RegenerateSummary {
    pub regenerated: Vec<NaiveDate>,
    pub blocked: Vec<BlockedDate>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct BlockedDate {
    pub date: NaiveDate,
    pub reason: String,
}

/// Regenerate each of `dates` with the single-date semantics of [`generate`]
/// (as-at-date facts, stored prices; nothing stored for a still-blocked
/// date), collecting per-date blockers instead of aborting. Logs each date
/// as it completes (INFO on success, WARN when blocked) with a running
/// `done`/`total` count, so a long bulk run's progress is visible in the log
/// file rather than only in the final response.
async fn regenerate_dates(
    pool: &SqlitePool,
    dates: impl IntoIterator<Item = NaiveDate>,
    now: DateTime<Utc>,
) -> RegenerateSummary {
    let dates: Vec<NaiveDate> = dates.into_iter().collect();
    let total = dates.len();
    let mut summary = RegenerateSummary {
        regenerated: Vec::new(),
        blocked: Vec::new(),
    };
    for (i, date) in dates.into_iter().enumerate() {
        let done = i + 1;
        match generate(pool, date, now).await {
            Ok(_) => {
                tracing::info!(%date, done, total, "snapshot regenerated");
                summary.regenerated.push(date);
            }
            Err(e) => {
                tracing::warn!(%date, done, total, reason = %e, "snapshot regeneration blocked");
                summary.blocked.push(BlockedDate {
                    date,
                    reason: e.to_string(),
                });
            }
        }
    }
    summary
}

/// Regenerate every date in `[from, to]` the portfolio held anything on —
/// the bulk repair after back-dated edits, and a backfill for dates that
/// never had a snapshot stored (e.g. after backfilling old closing prices).
/// A missing bound defaults to [`default_regenerate_range`] (first-ever-held
/// date / latest fully-valuable date); `from` is then clamped up to the
/// first-held date so an earlier caller-given `from` can't spin through
/// years of no-op days. Dates with nothing held are skipped silently; dates
/// that are blocked (missing/errored price) are reported in the summary
/// rather than aborting the others. Unblocked dates still regenerate when
/// others are blocked.
pub async fn regenerate_all(
    pool: &SqlitePool,
    from: Option<NaiveDate>,
    to: Option<NaiveDate>,
    now: DateTime<Utc>,
) -> Result<RegenerateSummary, GenerateError> {
    let default_range = default_regenerate_range(pool, now).await?;
    let (Some(first_held), Some(default_to)) = (default_range.from, default_range.to) else {
        // Nothing has ever been held — nothing to regenerate.
        return Ok(RegenerateSummary {
            regenerated: Vec::new(),
            blocked: Vec::new(),
        });
    };
    let from = from.unwrap_or(first_held).max(first_held);
    let to = to.unwrap_or(default_to);
    if from > to {
        return Err(GenerateError::Unprocessable(format!(
            "the range start ({from}) is after its end ({to})"
        )));
    }

    let mut dates: Vec<NaiveDate> = Vec::new();
    let mut date = from;
    while date <= to {
        if !closing_price::db_held_listing_ids(pool, Some(date))
            .await
            .map_err(GenerateError::from)?
            .is_empty()
        {
            dates.push(date);
        }
        date += Duration::days(1);
    }
    Ok(regenerate_dates(pool, dates, now).await)
}

/// Regenerate every date with a provisional snapshot — the true-up run after
/// an FX import lands real rates (also exposed manually as
/// `POST /report_snapshots/regenerate_provisional`). A date whose real rate
/// has still not been imported regenerates at the same fallback rate and
/// simply stays provisional.
///
/// It selects on `provisional` alone, never on `price_carried_forward` or
/// `holding_excluded`: neither of those ever clears, so including them would
/// turn this bounded pass into one that regenerates the same dates on every
/// import forever.
pub async fn regenerate_provisional(
    pool: &SqlitePool,
    now: DateTime<Utc>,
) -> Result<RegenerateSummary, sqlx::Error> {
    let dates: Vec<NaiveDate> = sqlx::query_scalar(
        "SELECT DISTINCT snapshot_date FROM report_snapshots WHERE provisional = 1 \
         ORDER BY snapshot_date",
    )
    .fetch_all(pool)
    .await?;
    Ok(regenerate_dates(pool, dates, now).await)
}

// ---------------------------------------------------------------------------
// HTTP API
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct ListParams {
    report: Option<ReportKind>,
    from: Option<NaiveDate>,
    to: Option<NaiveDate>,
}

#[derive(Debug, Default, Deserialize)]
struct GenerateBody {
    /// The snapshot date; defaults to the latest fully-valuable date.
    date: Option<NaiveDate>,
}

async fn list(
    State(pool): State<SqlitePool>,
    Query(params): Query<ListParams>,
) -> Result<Json<Vec<SnapshotMeta>>, ApiError> {
    db_list(&pool, params.report, params.from, params.to)
        .await
        .map(Json)
        .map_err(ApiError::from)
}

async fn series(State(pool): State<SqlitePool>) -> Result<Json<Vec<SeriesPoint>>, ApiError> {
    db_series(&pool).await.map(Json).map_err(ApiError::from)
}

async fn get_one(
    State(pool): State<SqlitePool>,
    Path((report, date)): Path<(String, NaiveDate)>,
) -> Result<Json<Snapshot>, ApiError> {
    let report = ReportKind::from_slug(&report).ok_or(ApiError::NotFound)?;
    db_get(&pool, report, date)
        .await
        .map_err(ApiError::from)?
        .map(Json)
        .ok_or(ApiError::NotFound)
}

/// Generate (or regenerate a stale) day's snapshots on demand — e.g. a past
/// date after backfilling its prices, or today's after recording a back-dated
/// fact. 422 carries the blocker detail (missing/errored price, close not
/// final, nothing held).
async fn generate_handler(
    State(pool): State<SqlitePool>,
    body: Option<Json<GenerateBody>>,
) -> Result<Json<Vec<SnapshotMeta>>, ApiError> {
    let now = Utc::now();
    let date = match body.and_then(|Json(b)| b.date) {
        Some(date) => date,
        None => latest_snapshot_date(&pool, now)
            .await?
            .ok_or_else(|| ApiError::unprocessable("nothing is held"))?,
    };
    Ok(Json(generate(&pool, date, now).await?))
}

impl From<GenerateError> for ApiError {
    fn from(e: GenerateError) -> Self {
        match e {
            GenerateError::Unprocessable(msg) => ApiError::Unprocessable(msg),
            GenerateError::Db(msg) => ApiError::internal(msg),
        }
    }
}

/// The optional `{ "from", "to" }` body for `regenerate_all` — either or both
/// omitted default per [`default_regenerate_range`].
#[derive(Debug, Default, Deserialize)]
struct RegenerateBody {
    from: Option<NaiveDate>,
    to: Option<NaiveDate>,
}

/// The default bulk-regeneration bounds, for the UI to prefill the range
/// boxes before the user submits.
async fn regenerate_range_handler(
    State(pool): State<SqlitePool>,
) -> Result<Json<RegenerateRange>, ApiError> {
    Ok(Json(
        default_regenerate_range(&pool, Utc::now())
            .await
            .map_err(ApiError::from)?,
    ))
}

/// Bulk repair: regenerate every date in the range (default: first-ever-held
/// through the latest fully-valuable date) that anything was held on —
/// backfilling dates that never had a snapshot as well as re-running stored
/// ones. 200 with the summary — per-date blockers are reported in it, not as
/// an error, so unblocked dates still regenerate. 422 if `from` is after
/// `to`.
async fn regenerate_all_handler(
    State(pool): State<SqlitePool>,
    body: Option<Json<RegenerateBody>>,
) -> Result<Json<RegenerateSummary>, ApiError> {
    let RegenerateBody { from, to } = body.map(|Json(b)| b).unwrap_or_default();
    Ok(Json(
        regenerate_all(&pool, from, to, Utc::now())
            .await
            .map_err(ApiError::from)?,
    ))
}

/// The manual counterpart of the post-import true-up: regenerate only the
/// provisional snapshot dates. Same summary shape as regenerate-all.
async fn regenerate_provisional_handler(
    State(pool): State<SqlitePool>,
) -> Result<Json<RegenerateSummary>, ApiError> {
    Ok(Json(regenerate_provisional(&pool, Utc::now()).await?))
}

pub fn router() -> Router<SqlitePool> {
    Router::new()
        .route("/report_snapshots", get(list))
        .route("/report_snapshots/series", get(series))
        .route("/report_snapshots/generate", post(generate_handler))
        .route(
            "/report_snapshots/regenerate_all",
            post(regenerate_all_handler),
        )
        .route(
            "/report_snapshots/regenerate_range",
            get(regenerate_range_handler),
        )
        .route(
            "/report_snapshots/regenerate_provisional",
            post(regenerate_provisional_handler),
        )
        .route("/report_snapshots/{report}/{date}", get(get_one))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::entities::{corporate_action, listing};
    use crate::test_support::{self, ApiClient, ApiResponse, test_pool, ymd};
    use axum::http::StatusCode;

    // -----------------------------------------------------------------------
    // The staleness-trigger set, pinned against the live schema
    //
    // The sibling of `reports::row_history::AUDITED_TABLES`: that const pins
    // which tables the audit trail records, this pair pins which tables stale a
    // stored snapshot. Both carry a rule a migration can silently skip — and
    // this one was skipped three times running (`listings` until 0030,
    // `rba_fx_rates` until 0031, `exchange_holidays` until 0033) while it was a
    // convention plus a per-migration comment, which is why it is asserted here
    // (SCENARIOS Q-09).
    // -----------------------------------------------------------------------

    /// The tables whose writes must mark stored snapshots stale, each with the
    /// trigger operations it is *required* to carry — an extra or a missing one
    /// fails — and why that is the set.
    const STALENESS_TRIGGERED_TABLES: [(&str, &[&str], &str); 10] = [
        (
            "trades",
            &["insert", "update", "delete"],
            "the parcels every snapshotted report values; staled from the trade `date` \
             (an UPDATE from the earlier of the old and new dates)",
        ),
        (
            "parcel_allocations",
            &["insert", "update", "delete"],
            "what a Sell consumed, so it decides which parcels are still open; staled from \
             its sale trade's `date`",
        ),
        (
            "income",
            &["insert", "update", "delete"],
            "distributions are the performance report's cash flows; staled from `date_paid`",
        ),
        (
            "amma_statements",
            &["insert", "update", "delete"],
            "the AMIT cost-base adjustments hang off it; staled from `tax_year_end_date`",
        ),
        (
            "amit_adjustments",
            &["insert", "update", "delete"],
            "the per-unit cost-base reduction itself (CGT event E10); staled from its \
             statement's `tax_year_end_date`",
        ),
        (
            "corporate_actions",
            &["insert", "update", "delete"],
            "splits, bonus issues and returns of capital re-base units and cost base; staled \
             from the action `date`",
        ),
        (
            "exchange_holidays",
            &["insert", "update", "delete"],
            "the trading calendar `reports::valuation::stored_valuations` reads **live** when \
             it walks each holding back to the nearest trading day on or before the snapshot \
             date; staled from `holiday_date` (0033_exchange_holiday_stale_snapshots.sql, \
             SCENARIOS Q-05/Q-08)",
        ),
        (
            "closing_prices",
            &["update"],
            "UPDATE only, and only on an ok price changing: an INSERT prices a date that was \
             blocked for valuation and so has no snapshot to stale, and the deletes the API \
             allows are of rows no stored figure was valued at — an errored row, whose date \
             valuation blocks outright, and an ok row inside the listing's `unpriced_before` \
             span, where the marker supersedes the stored rows and the holding is excluded \
             from the date's totals rather than priced (setting or moving that marker is \
             itself what stales those snapshots, via `listings`) \
             (0001_schema.sql; `entities::closing_price::db_delete`)",
        ),
        (
            "listings",
            &["update"],
            "UPDATE only, narrowed to \
             `currency`/`security_type`/`unpriced_from`/`unpriced_before` — the columns \
             that change what a *stored* figure means (the currency denominating every price, \
             the security type deciding which days are valuable, and the two dates bounding the \
             span the price provider serves). The first two carry no date, so \
             they stale the whole series; `unpriced_from` stales from the earlier of its old \
             and new dates, so clearing it puts the real prices back, and `unpriced_before` — \
             the date the provider's series begins, before which the holding is excluded from \
             the totals — stales the mirror-image prefix, before the later of its old and new \
             dates. A listing with no trades \
             is held on no snapshot date and a delete is refused while anything references it \
             (0030_listing_stale_snapshots.sql, 0035_listing_unpriced_from.sql, \
             0037_listing_unpriced_before.sql, SCENARIOS M-08, Q-02)",
        ),
        (
            "rba_fx_rates",
            &["update"],
            "UPDATE only: correcting a stored rate re-values every snapshot from its month on, \
             while an INSERT filling a month that had no rate is the provisional true-up's \
             business (regenerated, not staled — only the rate was interim), and there is no \
             delete route (0031_audit_rba_fx_rates.sql, SCENARIOS M-13)",
        ),
    ];

    /// Every other table in the schema, each with the reason a write to it can
    /// invalidate no stored snapshot — the reason its own migration gives,
    /// collected here so an exemption is a decision on the record rather than
    /// an omission. The snapshotted reports are the price-dependent three
    /// ([`SnapshotReport`]); a table only the live-computed CGT reports or the
    /// (un-snapshotted) tax summary read is therefore exempt.
    const STALENESS_EXEMPT_TABLES: [(&str, &str); 19] = [
        (
            "attachments",
            "documents are provenance, not financial facts; no snapshotted report reads them \
             (0014_attachment_owner_expansion.sql)",
        ),
        (
            "cgt_settings",
            "the opening capital loss reaches the CGT reports, which are computed live and \
             never snapshotted",
        ),
        (
            "currencies",
            "the ISO code list every currency column is foreign-keyed to: an identity, not a \
             figure any report computes with",
        ),
        (
            "drp_enrolments",
            "an enrolment period decides whether a distribution reinvests; the reinvestment \
             itself is a DRP `trades` row written in the same transaction, and the trades \
             triggers cover it",
        ),
        (
            "ess_statements",
            "no snapshotted report reads them — the ESS discount reaches the tax summary, \
             which is not snapshotted — and the vest Buy the statement feeds is a `trades` row \
             (0009_ess_statement_aud_overrides.sql, 0026_ess_statement_fx_rate.sql)",
        ),
        (
            "exchanges",
            "`settlement_days` is consumed when a trade is written, and `timezone`/`close_time` \
             decide only which dates are *generable*, never what a stored snapshot says \
             (docs/SCHEMA.md; contrast `exchange_holidays`, whose calendar valuation reads live)",
        ),
        (
            "holding_accounts",
            "identity only: an account names where a parcel sits and carries no figure a report \
             computes with",
        ),
        (
            "inheritances",
            "provenance only — every write also writes the linked Buy in the same transaction, \
             firing the trades triggers (0005_inheritances.sql)",
        ),
        (
            "interest_income",
            "the only report reading it is the tax summary, which is not snapshotted \
             (0008_interest_income.sql, 0011_interest_income_foreign_source.sql)",
        ),
        (
            "investment_expenses",
            "a deduction the tax summary totals, and the tax summary is not snapshotted",
        ),
        (
            "job_runs",
            "operational metadata, not a financial fact table (0012_job_run_history.sql)",
        ),
        (
            "listing_renames",
            "a snapshot carries a ticker only as a display label over `listing_id`, never as a \
             computed figure, so a rename is display-only drift rather than a wrong figure \
             (0018_listing_renames.sql, SCENARIOS Q-15)",
        ),
        (
            "mic_registry",
            "the import-managed ISO 10383 validation list; no report reads it",
        ),
        (
            "report_snapshots",
            "the snapshot store itself — what the triggers write *to*, never a fact that stales \
             one",
        ),
        (
            "rights_sale_allocations",
            "the parcels a rights sale drew on; exempt for the same reason as `rights_sales` \
             (0006_rights_sales.sql)",
        ),
        (
            "rights_sales",
            "a rights sale changes no holding quantity and no parcel cost base, so no \
             snapshotted report reads it — its effect is confined to the live-computed CGT \
             reports (0006_rights_sales.sql)",
        ),
        (
            "row_history",
            "the append-only audit trail: derived state written by triggers, and its own guards \
             abort any UPDATE or DELETE of it (0013_row_history.sql)",
        ),
        (
            "tax_year_settings",
            "a taxpayer-level ESS eligibility fact read by the tax summary, which is not \
             snapshotted (0027_tax_year_settings.sql)",
        ),
        (
            "transfers",
            "provenance only — a transfer writes its closing Sell and the replacement Buys in \
             the same transaction, firing the trades triggers",
        ),
    ];

    /// Every table in the live schema is classified: it either carries exactly
    /// the staleness triggers [`STALENESS_TRIGGERED_TABLES`] requires of it, or
    /// it is listed in [`STALENESS_EXEMPT_TABLES`] with the reason it needs
    /// none — and then carries none at all. A table in **neither** list fails,
    /// which is the property the convention was missing: a new dated fact table
    /// cannot land without its author either giving it triggers or writing down
    /// why it has none. Modelled on
    /// `row_history::audited_tables_match_migration_check_and_triggers`, but
    /// asserted against `sqlite_master` rather than the migration text, so a
    /// later rebuild that drops a trigger set with its table fails here too.
    #[tokio::test]
    async fn every_table_is_classified_for_snapshot_staleness() {
        let pool = test_pool().await;

        let tables: Vec<String> = sqlx::query_scalar(
            "SELECT name FROM sqlite_master WHERE type = 'table' \
             AND name NOT LIKE 'sqlite_%' AND name <> '_sqlx_migrations' ORDER BY name",
        )
        .fetch_all(&pool)
        .await
        .unwrap();
        assert!(
            tables.len() > 20,
            "the test pool should hold the fully migrated schema, found {tables:?}"
        );

        let triggered: Vec<&str> = STALENESS_TRIGGERED_TABLES
            .iter()
            .map(|(table, ..)| *table)
            .collect();
        let exempt: Vec<&str> = STALENESS_EXEMPT_TABLES
            .iter()
            .map(|(table, _)| *table)
            .collect();

        for table in &tables {
            let listed = usize::from(triggered.contains(&table.as_str()))
                + usize::from(exempt.contains(&table.as_str()));
            assert_eq!(
                listed, 1,
                "`{table}` appears in {listed} of STALENESS_TRIGGERED_TABLES / \
                 STALENESS_EXEMPT_TABLES, must appear in exactly one. A write to a dated fact \
                 must mark every report snapshot on or after its date stale, in the write's own \
                 transaction (0001_schema.sql, \"Snapshot-staleness triggers\"): either give the \
                 table its `*_stale_snapshots_*` triggers in the migration and list it triggered \
                 here, or list it exempt with the reason no snapshotted report's figures can go \
                 stale from a write to it."
            );
        }
        for name in triggered.iter().chain(exempt.iter()) {
            assert!(
                tables.iter().any(|table| table == name),
                "`{name}` is classified here but is not a table in the schema"
            );
        }

        for (table, ops, reason) in STALENESS_TRIGGERED_TABLES {
            let mut expected: Vec<String> = ops
                .iter()
                .map(|op| format!("{table}_stale_snapshots_{op}"))
                .collect();
            expected.sort();
            assert_eq!(
                staleness_triggers(&pool, table).await,
                expected,
                "`{table}` must carry exactly these staleness triggers — {reason}"
            );
        }

        for (table, reason) in STALENESS_EXEMPT_TABLES {
            assert!(!reason.is_empty(), "`{table}` must record why it is exempt");
            assert!(
                staleness_triggers(&pool, table).await.is_empty(),
                "`{table}` is listed exempt ({reason}) but carries staleness triggers — \
                 move it to STALENESS_TRIGGERED_TABLES with the operations it needs"
            );
        }
    }

    /// The `*_stale_snapshots_*` trigger names the live schema carries on
    /// `table`, sorted — read from `sqlite_master` the way `infra::db`'s
    /// migration tests read theirs.
    async fn staleness_triggers(pool: &SqlitePool, table: &str) -> Vec<String> {
        sqlx::query_scalar(
            "SELECT name FROM sqlite_master WHERE type = 'trigger' AND tbl_name = ? \
             AND name LIKE '%\\_stale\\_snapshots\\_%' ESCAPE '\\' ORDER BY name",
        )
        .bind(table)
        .fetch_all(pool)
        .await
        .unwrap()
    }

    fn utc(y: i32, m: u32, d: u32, h: u32, min: u32) -> DateTime<Utc> {
        chrono::TimeZone::with_ymd_and_hms(&Utc, y, m, d, h, min, 0).unwrap()
    }

    // 2026-06-05 is a Friday; 08:00 UTC = 18:00 Sydney, after the ASX close.
    fn friday_evening_sydney() -> DateTime<Utc> {
        utc(2026, 6, 5, 8, 0)
    }

    async fn insert_listing(
        pool: &SqlitePool,
        id: i64,
        ticker: &str,
        mic: Option<&str>,
        ccy: &str,
    ) {
        let b = test_support::listing(id)
            .ticker(ticker)
            .name(ticker)
            .currency(ccy);
        match mic {
            Some(m) => {
                b.mic(m)
                    .security_type(listing::SecurityType::Share)
                    .insert(pool)
                    .await
            }
            None => b.crypto().insert(pool).await,
        }
    }

    async fn insert_buy(
        pool: &SqlitePool,
        id: i64,
        listing_id: i64,
        date: NaiveDate,
        qty: &str,
        price: &str,
        ccy: &str,
    ) {
        test_support::buy(id, listing_id)
            .date(date)
            .settlement(date)
            .qty(qty.parse().unwrap())
            .price(price.parse().unwrap())
            .currency(ccy)
            .insert(pool)
            .await;
    }

    async fn store_price(pool: &SqlitePool, listing_id: i64, date: NaiveDate, price: &str) {
        test_support::closing_price(listing_id, date)
            .price(price)
            .insert(pool)
            .await;
    }

    async fn store_errored_price(pool: &SqlitePool, listing_id: i64, date: NaiveDate, msg: &str) {
        test_support::closing_price(listing_id, date)
            .errored(msg)
            .insert(pool)
            .await;
    }

    async fn stale_flags(pool: &SqlitePool, date: NaiveDate) -> Vec<bool> {
        db_list(pool, None, Some(date), Some(date))
            .await
            .unwrap()
            .iter()
            .map(|m| m.stale)
            .collect()
    }

    /// The scheduled job stores all three reports' AUD results keyed by the
    /// latest fully-valuable date, and skips a date already stored fresh.
    #[tokio::test]
    async fn db_snapshot_job_persists_aud_report_results_keyed_by_date() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "BHP", Some("XASX"), "AUD").await;
        insert_listing(&pool, 2, "ICE", Some("XNYS"), "USD").await;
        insert_buy(&pool, 1, 1, ymd(2024, 1, 16), "100", "10", "AUD").await;
        insert_buy(&pool, 2, 2, ymd(2024, 1, 16), "10", "100", "USD").await;
        // RBA rates: 2 USD per AUD in both the acquisition and valuation months.
        for month in ["2024-01", "2026-06"] {
            sqlx::query("INSERT INTO rba_fx_rates (currency, month, rate) VALUES ('USD', ?, '2')")
                .bind(month)
                .execute(&pool)
                .await
                .unwrap();
        }
        // Friday evening Sydney: the NYSE close for Friday is hours away, so
        // the latest date the whole portfolio is final for is Thursday.
        let now = friday_evening_sydney();
        assert_eq!(
            latest_snapshot_date(&pool, now).await.unwrap(),
            Some(ymd(2026, 6, 4))
        );
        store_price(&pool, 1, ymd(2026, 6, 4), "62.48").await;
        store_price(&pool, 2, ymd(2026, 6, 4), "141.50").await;

        run_snapshot_job(&pool, now).await.unwrap();

        let metas = db_list(&pool, None, None, None).await.unwrap();
        assert_eq!(metas.len(), 3, "one snapshot per price-dependent report");
        assert!(
            metas
                .iter()
                .all(|m| m.snapshot_date == ymd(2026, 6, 4) && !m.stale)
        );

        // The unrealised-gains snapshot carries AUD market values: BHP at the
        // stored AUD price, ICE converted at 141.50 USD / 2 = 70.75 AUD.
        let snap = db_get(&pool, ReportKind::UnrealisedGains, ymd(2026, 6, 4))
            .await
            .unwrap()
            .unwrap();
        let gains: Vec<unrealised_gains::UnrealisedGain> =
            serde_json::from_value(snap.rows).unwrap();
        assert_eq!(gains.len(), 2);
        assert_eq!(gains[0].market_value, Some("6248.00".parse().unwrap())); // 100 × 62.48
        assert_eq!(gains[1].market_value, Some("707.50".parse().unwrap())); // 10 × 70.75
        let overview = db_get(&pool, ReportKind::PortfolioOverview, ymd(2026, 6, 4))
            .await
            .unwrap()
            .unwrap();
        let holdings: Vec<portfolio::HoldingOverview> =
            serde_json::from_value(overview.rows).unwrap();
        assert_eq!(holdings[0].market_value, Some("6248.00".parse().unwrap()));
        let perf = db_get(&pool, ReportKind::Performance, ymd(2026, 6, 4))
            .await
            .unwrap()
            .unwrap();
        let rows: Vec<performance::HoldingPerformance> = serde_json::from_value(perf.rows).unwrap();
        assert_eq!(rows.last().unwrap().ticker, "OVERALL");
        assert_eq!(
            rows.last().unwrap().market_value,
            Some("6955.50".parse().unwrap())
        );

        // A second run the same evening finds the date stored fresh and skips.
        let generated_at = metas[0].generated_at.clone();
        run_snapshot_job(&pool, now).await.unwrap();
        let metas = db_list(&pool, None, None, None).await.unwrap();
        assert_eq!(metas.len(), 3);
        assert_eq!(
            metas[0].generated_at, generated_at,
            "fresh snapshots are not regenerated"
        );
    }

    /// A weekend (or holiday) date values each listing at its nearest earlier
    /// trading day; crypto trades every day, so a mixed portfolio's Saturday
    /// snapshot uses Friday's ASX close and Saturday's crypto cut-off price.
    #[tokio::test]
    async fn db_weekend_snapshot_walks_back_to_each_markets_trading_day() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "BHP", Some("XASX"), "AUD").await;
        insert_listing(&pool, 2, "BTC", None, "AUD").await;
        insert_buy(&pool, 1, 1, ymd(2024, 1, 16), "100", "10", "AUD").await;
        insert_buy(&pool, 2, 2, ymd(2024, 1, 16), "0.5", "60000", "AUD").await;
        store_price(&pool, 1, ymd(2026, 6, 5), "62.48").await; // Friday
        store_price(&pool, 2, ymd(2026, 6, 6), "99545.35").await; // Saturday (UTC day)

        // Sunday 01:30 UTC: Saturday's crypto candle is complete, Friday's ASX
        // close long final → the portfolio is valuable as at Saturday.
        let now = utc(2026, 6, 7, 1, 30);
        assert_eq!(
            latest_snapshot_date(&pool, now).await.unwrap(),
            Some(ymd(2026, 6, 6))
        );

        generate(&pool, ymd(2026, 6, 6), now).await.unwrap();
        let snap = db_get(&pool, ReportKind::UnrealisedGains, ymd(2026, 6, 6))
            .await
            .unwrap()
            .unwrap();
        let gains: Vec<unrealised_gains::UnrealisedGain> =
            serde_json::from_value(snap.rows).unwrap();
        assert_eq!(gains[0].market_value, Some("6248.00".parse().unwrap()));
        assert_eq!(gains[1].market_value, Some("49772.675".parse().unwrap())); // 0.5 × 99545.35
    }

    /// Correcting a stored provider price by hand is an ordinary UPDATE, so
    /// the staleness trigger catches it exactly as it catches a re-fetch: the
    /// snapshots that were valued at the wrong figure regenerate at the
    /// manual one, and earlier snapshots are untouched.
    #[tokio::test]
    async fn db_manual_price_over_a_stored_price_stales_on_or_after_snapshots() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "BHP", Some("XASX"), "AUD").await;
        insert_buy(&pool, 1, 1, ymd(2024, 1, 16), "100", "10", "AUD").await;
        store_price(&pool, 1, ymd(2026, 6, 3), "64.91").await; // Wednesday
        store_price(&pool, 1, ymd(2026, 6, 5), "62.48").await; // Friday
        let now = friday_evening_sydney();
        generate(&pool, ymd(2026, 6, 3), now).await.unwrap();
        generate(&pool, ymd(2026, 6, 5), now).await.unwrap();
        assert_eq!(stale_flags(&pool, ymd(2026, 6, 5)).await, vec![false; 3]);

        // Friday's close was wrong; the corrected figure is entered by hand.
        test_support::closing_price(1, ymd(2026, 6, 5))
            .price("60.00")
            .manual(
                "asx.com.au closing report",
                "provider quoted the wrong close",
            )
            .insert(&pool)
            .await;
        assert_eq!(stale_flags(&pool, ymd(2026, 6, 5)).await, vec![true; 3]);
        assert_eq!(stale_flags(&pool, ymd(2026, 6, 3)).await, vec![false; 3]);

        generate(&pool, ymd(2026, 6, 5), now).await.unwrap();
        assert_eq!(stale_flags(&pool, ymd(2026, 6, 5)).await, vec![false; 3]);
        let snap = db_get(&pool, ReportKind::UnrealisedGains, ymd(2026, 6, 5))
            .await
            .unwrap()
            .unwrap();
        let gains: Vec<unrealised_gains::UnrealisedGain> =
            serde_json::from_value(snap.rows).unwrap();
        assert_eq!(
            gains[0].market_value,
            Some("6000.00".parse().unwrap()),
            "regenerated at the hand-entered price"
        );
    }

    /// A back-dated fact (here: a trade, an income row, and a corporate action,
    /// each written through its normal entity path) marks every snapshot dated
    /// on or after it stale — in the same transaction, via the 0019 triggers —
    /// and leaves earlier snapshots alone. Regenerating re-runs the reports
    /// with the new facts and clears the flag.
    #[tokio::test]
    async fn db_back_dated_fact_stales_on_or_after_snapshots_and_regeneration_clears() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "BHP", Some("XASX"), "AUD").await;
        insert_buy(&pool, 1, 1, ymd(2024, 1, 16), "100", "10", "AUD").await;
        store_price(&pool, 1, ymd(2026, 6, 3), "64.91").await; // Wednesday
        store_price(&pool, 1, ymd(2026, 6, 5), "62.48").await; // Friday
        let now = friday_evening_sydney();
        generate(&pool, ymd(2026, 6, 3), now).await.unwrap();
        generate(&pool, ymd(2026, 6, 5), now).await.unwrap();

        // A Buy back-dated to Thursday: Friday's snapshots stale, Wednesday's untouched.
        insert_buy(&pool, 2, 1, ymd(2026, 6, 4), "50", "63", "AUD").await;
        assert_eq!(stale_flags(&pool, ymd(2026, 6, 3)).await, vec![false; 3]);
        assert_eq!(stale_flags(&pool, ymd(2026, 6, 5)).await, vec![true; 3]);

        // Regeneration re-runs with the new facts and clears the flag.
        generate(&pool, ymd(2026, 6, 5), now).await.unwrap();
        assert_eq!(stale_flags(&pool, ymd(2026, 6, 5)).await, vec![false; 3]);
        let snap = db_get(&pool, ReportKind::UnrealisedGains, ymd(2026, 6, 5))
            .await
            .unwrap()
            .unwrap();
        let gains: Vec<unrealised_gains::UnrealisedGain> =
            serde_json::from_value(snap.rows).unwrap();
        assert_eq!(
            gains[0].quantity,
            Decimal::from(150),
            "regenerated with the new parcel"
        );
        // …while Wednesday's regenerated result would still exclude the
        // Thursday parcel (the reports are as-at-date), keeping the series
        // consistent. Its stored rows already reflect that.
        let snap = db_get(&pool, ReportKind::UnrealisedGains, ymd(2026, 6, 3))
            .await
            .unwrap()
            .unwrap();
        let gains: Vec<unrealised_gains::UnrealisedGain> =
            serde_json::from_value(snap.rows).unwrap();
        assert_eq!(gains[0].quantity, Decimal::from(100));

        // The other fact paths invalidate the same way: a back-dated income
        // row and corporate action each re-stale Friday.
        test_support::income(1, 1, ymd(2026, 6, 4))
            .with(|i| {
                i.franked_amount = Decimal::from(70);
                i.franking_credits = Decimal::from(30);
            })
            .insert(&pool)
            .await;
        assert_eq!(stale_flags(&pool, ymd(2026, 6, 5)).await, vec![true; 3]);
        assert_eq!(stale_flags(&pool, ymd(2026, 6, 3)).await, vec![false; 3]);

        generate(&pool, ymd(2026, 6, 5), now).await.unwrap();
        // On a second, untraded listing: the delete leg below would otherwise
        // be refused, since a return of capital is frozen once it has reduced
        // a parcel held at its date.
        insert_listing(&pool, 2, "RIO", Some("XASX"), "AUD").await;
        corporate_action::db_upsert(
            &pool,
            &corporate_action::CorporateAction {
                id: 1,
                listing_id: 2,
                date: ymd(2026, 6, 4),
                kind: corporate_action::ActionKind::ReturnOfCapital {
                    amount_per_unit: "0.50".parse().unwrap(),
                    currency: "AUD".to_string(),
                    record_date: None,
                },
            },
        )
        .await
        .unwrap();
        assert_eq!(stale_flags(&pool, ymd(2026, 6, 5)).await, vec![true; 3]);

        // Deleting a back-dated fact invalidates the same way.
        generate(&pool, ymd(2026, 6, 5), now).await.unwrap();
        corporate_action::db_delete(&pool, 1).await.unwrap();
        assert_eq!(stale_flags(&pool, ymd(2026, 6, 5)).await, vec![true; 3]);
    }

    /// SCENARIOS Q-05/Q-08 end to end: the exchange holiday calendar is not a
    /// write-time input to anything — `valuation::stored_valuations` reads it
    /// **live**, valuing each holding at its nearest trading day on or before
    /// the snapshot date — so a holiday write re-values stored snapshots. Both
    /// directions are covered by the 0033 triggers: seeding one moves the
    /// valuation back to the prior close (12.4% here), and deleting a seeded
    /// one makes the date a trading day whose price was never collected, so
    /// the stored figure rests on a valuation day that no longer exists.
    /// Before 0033 both stood indefinitely as `stale: false`.
    #[tokio::test]
    async fn db_an_exchange_holiday_write_stales_the_snapshots_it_re_values() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "BHP", Some("XASX"), "AUD").await;
        insert_buy(&pool, 1, 1, ymd(2024, 1, 16), "100", "10", "AUD").await;
        store_price(&pool, 1, ymd(2026, 6, 4), "44.4308").await; // Thursday
        store_price(&pool, 1, ymd(2026, 6, 5), "50.7308").await; // Friday
        let now = friday_evening_sydney();
        generate(&pool, ymd(2026, 6, 4), now).await.unwrap();
        generate(&pool, ymd(2026, 6, 5), now).await.unwrap();
        let series = db_series(&pool).await.unwrap();
        assert_eq!(series[1].market_value, "5073.08".parse().unwrap());
        assert!(!series[1].stale);

        // The ASX turns out to have been closed that Friday.
        ApiClient::full(&pool)
            .put(
                "/exchange_holidays/XASX/2026-06-05",
                &serde_json::json!({ "name": "Test Closure" }),
            )
            .await
            .expect_status(StatusCode::NO_CONTENT);
        assert_eq!(stale_flags(&pool, ymd(2026, 6, 5)).await, vec![true; 3]);
        assert_eq!(
            stale_flags(&pool, ymd(2026, 6, 4)).await,
            vec![false; 3],
            "an earlier snapshot can never have been valued at the holiday"
        );

        // Regenerating values the Friday at Thursday's close instead.
        generate(&pool, ymd(2026, 6, 5), now).await.unwrap();
        let series = db_series(&pool).await.unwrap();
        assert_eq!(series[1].market_value, "4443.08".parse().unwrap());
        assert!(!series[1].stale);

        // The other direction. The following Monday is a seeded ASX holiday
        // (King's Birthday), so its snapshot is valued at the last open day.
        let next_friday_evening = utc(2026, 6, 12, 8, 0);
        generate(&pool, ymd(2026, 6, 8), next_friday_evening)
            .await
            .unwrap();
        assert_eq!(
            db_series(&pool).await.unwrap()[2].market_value,
            "4443.08".parse::<Decimal>().unwrap()
        );

        // Removing it makes 8 June a trading day — one no price was ever
        // collected for, so the stored figure now rests on a valuation day
        // that does not exist. The snapshot is staled, and its regeneration
        // is blocked until the price is backfilled.
        ApiClient::full(&pool)
            .delete("/exchange_holidays/XASX/2026-06-08")
            .await
            .expect_status(StatusCode::NO_CONTENT);
        assert_eq!(stale_flags(&pool, ymd(2026, 6, 8)).await, vec![true; 3]);
        let err = generate(&pool, ymd(2026, 6, 8), next_friday_evening)
            .await
            .unwrap_err();
        assert!(
            matches!(err, GenerateError::Unprocessable(ref msg)
                if msg.contains("no stored price for 2026-06-08")),
            "{err}"
        );
    }

    /// A day whose price fetch failed has no trustworthy snapshot: generation
    /// refuses (the job fails, naming the listing), nothing is stored —
    /// missing, not stale — and the day becomes generable once the price
    /// re-run succeeds.
    #[tokio::test]
    async fn db_failed_price_day_yields_no_snapshot_until_the_price_rerun_succeeds() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "BHP", Some("XASX"), "AUD").await;
        insert_buy(&pool, 1, 1, ymd(2024, 1, 16), "100", "10", "AUD").await;
        store_errored_price(&pool, 1, ymd(2026, 6, 5), "provider down").await;

        let err = run_snapshot_job(&pool, friday_evening_sydney())
            .await
            .unwrap_err();
        assert!(
            err.contains("BHP"),
            "job error names the blocking listing: {err}"
        );
        assert!(err.contains("errored"), "and the errored price: {err}");
        assert!(
            db_list(&pool, None, None, None).await.unwrap().is_empty(),
            "nothing stored: the day is missing, not stale"
        );

        // A missing (never fetched) price blocks the same way.
        let err = generate(&pool, ymd(2026, 6, 4), friday_evening_sydney())
            .await
            .unwrap_err();
        assert!(matches!(err, GenerateError::Unprocessable(ref msg) if msg.contains("backfill")));

        // Once the re-fetch succeeds, the job stores the snapshot.
        store_price(&pool, 1, ymd(2026, 6, 5), "62.48").await;
        run_snapshot_job(&pool, friday_evening_sydney())
            .await
            .unwrap();
        assert_eq!(db_list(&pool, None, None, None).await.unwrap().len(), 3);
    }

    async fn carried_flags(pool: &SqlitePool, date: NaiveDate) -> Vec<bool> {
        db_list(pool, None, Some(date), Some(date))
            .await
            .unwrap()
            .iter()
            .map(|m| m.price_carried_forward)
            .collect()
    }

    /// SCENARIOS Q-02: a still-held listing the provider has stopped quoting
    /// used to block the **whole** portfolio's snapshot for every date after
    /// its last quote, indefinitely. Marking it `unpriced_from` values it at
    /// its last stored ok close instead — flagged on the snapshot and on the
    /// rows, never silently — while the rest of the portfolio prices as
    /// usual.
    #[tokio::test]
    async fn db_an_unpriced_listing_is_valued_at_its_carried_forward_close() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "BHP", Some("XASX"), "AUD").await;
        insert_listing(&pool, 2, "ATVI", Some("XASX"), "AUD").await;
        insert_buy(&pool, 1, 1, ymd(2024, 1, 16), "100", "10", "AUD").await;
        insert_buy(&pool, 2, 2, ymd(2024, 1, 16), "50", "20", "AUD").await;
        store_price(&pool, 1, ymd(2026, 6, 4), "62.48").await;
        // ATVI's last quote was two days earlier; the provider has served
        // nothing since.
        store_price(&pool, 2, ymd(2026, 6, 2), "94.42").await;
        store_errored_price(
            &pool,
            2,
            ymd(2026, 6, 3),
            "yahoo fetch for ATVI failed: Not found",
        )
        .await;
        store_errored_price(
            &pool,
            2,
            ymd(2026, 6, 4),
            "yahoo fetch for ATVI failed: Not found",
        )
        .await;

        let now = friday_evening_sydney();
        let err = generate(&pool, ymd(2026, 6, 4), now).await.unwrap_err();
        assert!(
            matches!(err, GenerateError::Unprocessable(ref msg) if msg.contains("ATVI")),
            "one dead symbol blocks the whole date until it is marked: {err}"
        );

        let marked = test_support::listing(2)
            .ticker("ATVI")
            .name("ATVI")
            .unpriced_from(ymd(2026, 6, 3))
            .build();
        listing::db_upsert(&pool, &marked).await.unwrap();

        let metas = generate(&pool, ymd(2026, 6, 4), now).await.unwrap();
        assert!(
            metas
                .iter()
                .all(|m| m.price_carried_forward && !m.provisional)
        );
        assert_eq!(carried_flags(&pool, ymd(2026, 6, 4)).await, vec![true; 3]);

        let snap = db_get(&pool, ReportKind::UnrealisedGains, ymd(2026, 6, 4))
            .await
            .unwrap()
            .unwrap();
        assert!(snap.price_carried_forward);
        let gains: Vec<unrealised_gains::UnrealisedGain> =
            serde_json::from_value(snap.rows).unwrap();
        // BHP prices at the day's own close and carries no flag; ATVI at the
        // last stored ok close (94.42, not the errored days) and does.
        assert_eq!(gains[0].market_value, Some("6248.00".parse().unwrap()));
        assert!(!gains[0].price_carried_forward);
        assert_eq!(gains[1].market_value, Some("4721.00".parse().unwrap())); // 50 × 94.42
        assert!(gains[1].price_carried_forward);

        // The series point carries it too, so the graph can mark it.
        let series = db_series(&pool).await.unwrap();
        assert_eq!(series.len(), 1);
        assert!(series[0].price_carried_forward);

        // Nothing trues it up: the true-up run targets provisional dates, and
        // a carried-forward price never becomes a real one.
        let summary = regenerate_provisional(&pool, now).await.unwrap();
        assert!(
            summary.regenerated.is_empty() && summary.blocked.is_empty(),
            "a carried-forward date is not a provisional one: {summary:?}"
        );
    }

    /// The way *out*: the security is quoted again, the marker is cleared,
    /// and every snapshot from its date on is staled — so regeneration
    /// replaces the flat carried-forward line with the real prices.
    #[tokio::test]
    async fn db_clearing_unpriced_from_stales_the_carried_forward_snapshots() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "ATVI", Some("XASX"), "AUD").await;
        insert_buy(&pool, 1, 1, ymd(2024, 1, 16), "50", "20", "AUD").await;
        store_price(&pool, 1, ymd(2026, 6, 2), "94.42").await;
        let marked = test_support::listing(1)
            .ticker("ATVI")
            .name("ATVI")
            .unpriced_from(ymd(2026, 6, 3))
            .build();
        listing::db_upsert(&pool, &marked).await.unwrap();

        let now = friday_evening_sydney();
        generate(&pool, ymd(2026, 6, 4), now).await.unwrap();
        assert_eq!(carried_flags(&pool, ymd(2026, 6, 4)).await, vec![true; 3]);
        assert_eq!(stale_flags(&pool, ymd(2026, 6, 4)).await, vec![false; 3]);

        // Quoted again: clear the marker and store the real close.
        let cleared = test_support::listing(1).ticker("ATVI").name("ATVI").build();
        listing::db_upsert(&pool, &cleared).await.unwrap();
        assert_eq!(
            stale_flags(&pool, ymd(2026, 6, 4)).await,
            vec![true; 3],
            "clearing the marker stales every snapshot from its date on"
        );
        store_price(&pool, 1, ymd(2026, 6, 4), "60.00").await;

        generate(&pool, ymd(2026, 6, 4), now).await.unwrap();
        assert_eq!(carried_flags(&pool, ymd(2026, 6, 4)).await, vec![false; 3]);
        assert_eq!(stale_flags(&pool, ymd(2026, 6, 4)).await, vec![false; 3]);
        let snap = db_get(&pool, ReportKind::UnrealisedGains, ymd(2026, 6, 4))
            .await
            .unwrap()
            .unwrap();
        let gains: Vec<unrealised_gains::UnrealisedGain> =
            serde_json::from_value(snap.rows).unwrap();
        assert_eq!(gains[0].market_value, Some("3000.00".parse().unwrap())); // 50 × 60
        assert!(!gains[0].price_carried_forward);
    }

    /// A price entered **by hand** for a day inside the unpriced run wins
    /// over the carried-forward close: it is the day's own price, so the row
    /// is not flagged.
    #[tokio::test]
    async fn db_a_manual_price_inside_the_unpriced_run_beats_the_carried_close() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "ATVI", Some("XASX"), "AUD").await;
        insert_buy(&pool, 1, 1, ymd(2024, 1, 16), "50", "20", "AUD").await;
        store_price(&pool, 1, ymd(2026, 6, 2), "94.42").await;
        test_support::closing_price(1, ymd(2026, 6, 4))
            .price("80.00")
            .manual("administrator valuation", "suspended, no provider candle")
            .insert(&pool)
            .await;
        let marked = test_support::listing(1)
            .ticker("ATVI")
            .name("ATVI")
            .unpriced_from(ymd(2026, 6, 3))
            .build();
        listing::db_upsert(&pool, &marked).await.unwrap();

        generate(&pool, ymd(2026, 6, 4), friday_evening_sydney())
            .await
            .unwrap();
        assert_eq!(carried_flags(&pool, ymd(2026, 6, 4)).await, vec![false; 3]);
        let snap = db_get(&pool, ReportKind::UnrealisedGains, ymd(2026, 6, 4))
            .await
            .unwrap()
            .unwrap();
        let gains: Vec<unrealised_gains::UnrealisedGain> =
            serde_json::from_value(snap.rows).unwrap();
        assert_eq!(gains[0].market_value, Some("4000.00".parse().unwrap())); // 50 × 80
    }

    async fn excluded_flags(pool: &SqlitePool, date: NaiveDate) -> Vec<bool> {
        db_list(pool, None, Some(date), Some(date))
            .await
            .unwrap()
            .iter()
            .map(|m| m.holding_excluded)
            .collect()
    }

    /// Migration 0037, the mirror image of `unpriced_from`. A holding whose
    /// provider series *begins* mid-holding cannot be valued before that day
    /// at any price — the LAC shape. Rather than blocking the date (which
    /// produced 375 hand-entered, knowingly-wrong prices in the live
    /// database) the holding **leaves the total** and the snapshot says which
    /// one left and why. The wrong stored price for the excluded day is
    /// superseded by the marker, which is what lets such rows be retired.
    #[tokio::test]
    async fn db_a_holding_with_no_obtainable_price_leaves_the_total_and_says_so() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "BHP", Some("XASX"), "AUD").await;
        insert_listing(&pool, 2, "LAC", Some("XASX"), "AUD").await;
        insert_buy(&pool, 1, 1, ymd(2024, 1, 16), "100", "10", "AUD").await;
        insert_buy(&pool, 2, 2, ymd(2024, 1, 16), "50", "20", "AUD").await;
        for d in [ymd(2026, 6, 3), ymd(2026, 6, 4)] {
            store_price(&pool, 1, d, "62.48").await;
        }
        // LAC's own series begins on the 4th. The 3rd carries a stored close
        // all the same — in the live case another listing's series, copied in
        // to unblock the date.
        store_price(&pool, 2, ymd(2026, 6, 3), "10.13").await;
        store_price(&pool, 2, ymd(2026, 6, 4), "24.90").await;

        let now = friday_evening_sydney();
        let marked = test_support::listing(2)
            .ticker("LAC")
            .name("LAC")
            .unpriced_before(ymd(2026, 6, 4))
            .build();
        listing::db_upsert(&pool, &marked).await.unwrap();

        generate(&pool, ymd(2026, 6, 3), now).await.unwrap();
        let metas = db_list(&pool, None, Some(ymd(2026, 6, 3)), Some(ymd(2026, 6, 3)))
            .await
            .unwrap();
        assert_eq!(excluded_flags(&pool, ymd(2026, 6, 3)).await, vec![true; 3]);
        assert_eq!(
            carried_flags(&pool, ymd(2026, 6, 3)).await,
            vec![false; 3],
            "an excluded holding is not a carried-forward price — nothing was substituted"
        );
        assert_eq!(
            provisional_flags(&pool, ymd(2026, 6, 3)).await,
            vec![false; 3]
        );
        let listed = &metas[0].excluded_holdings.0;
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].listing_id, 2);
        assert_eq!(listed[0].ticker, "LAC");
        assert!(listed[0].reason.contains("before 2026-06-04"), "{listed:?}");

        // The total is smaller by exactly the excluded holding: BHP alone,
        // never LAC valued at the 10.13 row the marker supersedes.
        let snap = db_get(&pool, ReportKind::UnrealisedGains, ymd(2026, 6, 3))
            .await
            .unwrap()
            .unwrap();
        assert!(snap.holding_excluded);
        let gains: Vec<unrealised_gains::UnrealisedGain> =
            serde_json::from_value(snap.rows).unwrap();
        assert_eq!(gains[0].market_value, Some("6248.00".parse().unwrap()));
        assert_eq!(gains[1].listing_id, 2);
        assert_eq!(gains[1].market_value, None, "the holding is not valued");
        assert!(
            gains[1]
                .price_unavailable
                .as_deref()
                .is_some_and(|r| r.contains("LAC")),
            "the row says why it is absent: {:?}",
            gains[1].price_unavailable
        );

        // The series point carries both the flag and the list, so the graph
        // can mark the step where LAC's own series begins.
        let series = db_series(&pool).await.unwrap();
        assert_eq!(series[0].snapshot_date, ymd(2026, 6, 3));
        assert!(series[0].holding_excluded);
        assert_eq!(series[0].excluded_holdings.0[0].ticker, "LAC");
        assert_eq!(series[0].market_value, "6248.00".parse().unwrap());

        // The day the series begins values both holdings, and the graph steps
        // up by the holding that rejoined.
        generate(&pool, ymd(2026, 6, 4), now).await.unwrap();
        assert_eq!(excluded_flags(&pool, ymd(2026, 6, 4)).await, vec![false; 3]);
        let series = db_series(&pool).await.unwrap();
        assert_eq!(series[1].market_value, "7493.00".parse().unwrap()); // + 50 × 24.90
        assert!(series[1].excluded_holdings.0.is_empty());
    }

    /// The unbounded-loop trap the flag exists to avoid. An excluded holding
    /// never clears, so neither true-up pass may select on it: the FX true-up
    /// targets `provisional` dates only, and the scheduled job's
    /// `needs_generation` ignores it exactly as it ignores
    /// `price_carried_forward`. Otherwise every run would regenerate the same
    /// dates forever.
    #[tokio::test]
    async fn db_an_excluded_holding_is_never_retried_by_a_true_up_pass() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "BHP", Some("XASX"), "AUD").await;
        insert_listing(&pool, 2, "LAC", Some("XASX"), "AUD").await;
        insert_buy(&pool, 1, 1, ymd(2024, 1, 16), "100", "10", "AUD").await;
        insert_buy(&pool, 2, 2, ymd(2024, 1, 16), "50", "20", "AUD").await;
        store_price(&pool, 1, ymd(2026, 6, 3), "62.48").await;
        let marked = test_support::listing(2)
            .ticker("LAC")
            .name("LAC")
            .unpriced_before(ymd(2026, 6, 4))
            .build();
        listing::db_upsert(&pool, &marked).await.unwrap();

        let now = friday_evening_sydney();
        generate(&pool, ymd(2026, 6, 3), now).await.unwrap();
        assert_eq!(excluded_flags(&pool, ymd(2026, 6, 3)).await, vec![true; 3]);

        let summary = regenerate_provisional(&pool, now).await.unwrap();
        assert!(
            summary.regenerated.is_empty() && summary.blocked.is_empty(),
            "an excluded holding is not a provisional FX rate: {summary:?}"
        );
        let metas = db_list(&pool, None, Some(ymd(2026, 6, 3)), Some(ymd(2026, 6, 3)))
            .await
            .unwrap();
        assert!(
            !needs_generation(&metas.iter().collect::<Vec<_>>()),
            "the catch-up job must leave an excluded-holding date alone"
        );
    }

    /// The way *out*, and the mirror of clearing `unpriced_from`: the price
    /// becomes obtainable, the marker is cleared, and every snapshot *before*
    /// its date is staled — so regeneration puts the holding back into the
    /// totals at real prices.
    #[tokio::test]
    async fn db_clearing_unpriced_before_stales_and_regenerates_at_real_prices() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "LAC", Some("XASX"), "AUD").await;
        insert_listing(&pool, 2, "BHP", Some("XASX"), "AUD").await;
        insert_buy(&pool, 1, 1, ymd(2024, 1, 16), "50", "20", "AUD").await;
        insert_buy(&pool, 2, 2, ymd(2024, 1, 16), "100", "10", "AUD").await;
        store_price(&pool, 2, ymd(2026, 6, 3), "62.48").await;
        let marked = test_support::listing(1)
            .ticker("LAC")
            .name("LAC")
            .unpriced_before(ymd(2026, 6, 4))
            .build();
        listing::db_upsert(&pool, &marked).await.unwrap();

        let now = friday_evening_sydney();
        generate(&pool, ymd(2026, 6, 3), now).await.unwrap();
        assert_eq!(excluded_flags(&pool, ymd(2026, 6, 3)).await, vec![true; 3]);
        assert_eq!(stale_flags(&pool, ymd(2026, 6, 3)).await, vec![false; 3]);

        // A real price for the day turns up (a statement, a second provider):
        // enter it and clear the marker.
        test_support::closing_price(1, ymd(2026, 6, 3))
            .price("24.90")
            .manual(
                "broker statement",
                "the provider serves no candle this early",
            )
            .insert(&pool)
            .await;
        let cleared = test_support::listing(1).ticker("LAC").name("LAC").build();
        listing::db_upsert(&pool, &cleared).await.unwrap();
        assert_eq!(
            stale_flags(&pool, ymd(2026, 6, 3)).await,
            vec![true; 3],
            "clearing the marker stales every snapshot before its date"
        );

        generate(&pool, ymd(2026, 6, 3), now).await.unwrap();
        assert_eq!(excluded_flags(&pool, ymd(2026, 6, 3)).await, vec![false; 3]);
        assert_eq!(stale_flags(&pool, ymd(2026, 6, 3)).await, vec![false; 3]);
        let series = db_series(&pool).await.unwrap();
        assert_eq!(series[0].market_value, "7493.00".parse().unwrap()); // 6248 + 50 × 24.90
    }

    /// Why deleting a superseded price needs no staleness handling of its
    /// own. Setting the marker is what stales the prefix; regeneration then
    /// leaves the holding out, so the rows the marker supersedes are read by
    /// nothing and clearing them moves no stored figure and stales nothing.
    /// The case that must not be missed is the marker being cleared
    /// afterwards: the prefix stales again, and regeneration reports the date
    /// blocked for want of a price — the truth once the rows are gone, and
    /// not a silently wrong total.
    #[tokio::test]
    async fn db_clearing_the_superseded_prices_changes_no_stored_snapshot() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "LAC", Some("XASX"), "AUD").await;
        insert_listing(&pool, 2, "BHP", Some("XASX"), "AUD").await;
        insert_buy(&pool, 1, 1, ymd(2024, 1, 16), "50", "20", "AUD").await;
        insert_buy(&pool, 2, 2, ymd(2024, 1, 16), "100", "10", "AUD").await;
        // LAC's row for the day is another security's price — the live shape.
        store_price(&pool, 1, ymd(2026, 6, 3), "10.13").await;
        store_price(&pool, 2, ymd(2026, 6, 3), "62.48").await;
        let marked = test_support::listing(1)
            .ticker("LAC")
            .name("LAC")
            .unpriced_before(ymd(2026, 6, 4))
            .build();
        listing::db_upsert(&pool, &marked).await.unwrap();

        let now = friday_evening_sydney();
        generate(&pool, ymd(2026, 6, 3), now).await.unwrap();
        assert_eq!(excluded_flags(&pool, ymd(2026, 6, 3)).await, vec![true; 3]);
        let before = db_series(&pool).await.unwrap()[0].market_value;
        assert_eq!(before, "6248.00".parse().unwrap());

        let cleared = crate::entities::closing_price::db_clear_unpriced_before(&pool, 1)
            .await
            .unwrap();
        assert!(matches!(
            cleared,
            crate::entities::closing_price::ClearOutcome::Cleared { deleted: 1, .. }
        ));
        assert_eq!(
            stale_flags(&pool, ymd(2026, 6, 3)).await,
            vec![false; 3],
            "no stored figure was valued at the cleared row, so none is stale"
        );
        generate(&pool, ymd(2026, 6, 3), now).await.unwrap();
        assert_eq!(db_series(&pool).await.unwrap()[0].market_value, before);
        assert_eq!(excluded_flags(&pool, ymd(2026, 6, 3)).await, vec![true; 3]);

        // Clearing the marker afterwards stales the prefix, and regeneration
        // now blocks the date rather than valuing it at the rows that are gone.
        let unmarked = test_support::listing(1).ticker("LAC").name("LAC").build();
        listing::db_upsert(&pool, &unmarked).await.unwrap();
        assert_eq!(stale_flags(&pool, ymd(2026, 6, 3)).await, vec![true; 3]);
        let err = generate(&pool, ymd(2026, 6, 3), now).await.unwrap_err();
        assert!(
            matches!(err, GenerateError::Unprocessable(ref msg)
                if msg.contains("no stored price for 2026-06-03")),
            "{err}"
        );
    }

    /// Zero of zero is not a portfolio total: a date on which *every* held
    /// listing is excluded is blocked, not stored as an empty-but-flagged
    /// snapshot that would draw a false floor through the graph.
    #[tokio::test]
    async fn db_a_date_where_every_holding_is_excluded_is_blocked() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "LAC", Some("XASX"), "AUD").await;
        insert_buy(&pool, 1, 1, ymd(2024, 1, 16), "50", "20", "AUD").await;
        let marked = test_support::listing(1)
            .ticker("LAC")
            .name("LAC")
            .unpriced_before(ymd(2026, 6, 4))
            .build();
        listing::db_upsert(&pool, &marked).await.unwrap();

        let err = generate(&pool, ymd(2026, 6, 3), friday_evening_sydney())
            .await
            .unwrap_err();
        assert!(
            matches!(err, GenerateError::Unprocessable(ref msg)
                if msg.contains("no held listing can be valued on 2026-06-03")
                    && msg.contains("LAC")),
            "{err}"
        );
        assert!(
            db_list(&pool, None, None, None).await.unwrap().is_empty(),
            "nothing is stored for a date with no valuable holding"
        );
    }

    async fn import_rate(pool: &SqlitePool, currency: &str, month: &str, rate: &str) {
        crate::entities::rba_fx_rate::db_import_rate(pool, currency, month, rate.parse().unwrap())
            .await
            .unwrap();
    }

    async fn provisional_flags(pool: &SqlitePool, date: NaiveDate) -> Vec<bool> {
        db_list(pool, None, Some(date), Some(date))
            .await
            .unwrap()
            .iter()
            .map(|m| m.provisional)
            .collect()
    }

    /// A snapshot valued while the month's FX rate is unpublished uses the
    /// fallback-month rate and is stored provisional (with the affected rows
    /// annotated); once the real rate is imported, the scheduled job
    /// regenerates it in its window and the flag clears. Distinct from stale:
    /// importing the rate fires no staleness trigger.
    #[tokio::test]
    async fn db_missing_month_rate_makes_snapshot_provisional_until_regenerated() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "ICE", Some("XNYS"), "USD").await;
        insert_buy(&pool, 1, 1, ymd(2024, 1, 16), "10", "100", "USD").await;
        import_rate(&pool, "USD", "2024-01", "2").await;
        // May's rate exists, June's is not published yet.
        import_rate(&pool, "USD", "2026-05", "2").await;
        store_price(&pool, 1, ymd(2026, 6, 4), "141.50").await;

        let now = friday_evening_sydney();
        let metas = generate(&pool, ymd(2026, 6, 4), now).await.unwrap();
        assert!(metas.iter().all(|m| m.provisional && !m.stale));
        assert_eq!(
            provisional_flags(&pool, ymd(2026, 6, 4)).await,
            vec![true; 3]
        );

        // The stored rows carry the AUD value converted at May's fallback
        // rate, annotated per row.
        let snap = db_get(&pool, ReportKind::UnrealisedGains, ymd(2026, 6, 4))
            .await
            .unwrap()
            .unwrap();
        assert!(snap.provisional);
        let gains: Vec<unrealised_gains::UnrealisedGain> =
            serde_json::from_value(snap.rows).unwrap();
        assert_eq!(gains[0].market_value, Some("707.50".parse().unwrap())); // 10 × 141.50 / 2
        assert!(gains[0].fx_provisional);

        // The series marks the point provisional for the graph.
        let series = db_series(&pool).await.unwrap();
        assert!(series[0].provisional);

        // June's real rate lands (no staleness trigger fires) — the next job
        // run regenerates the provisional date and finalises it.
        assert_eq!(stale_flags(&pool, ymd(2026, 6, 4)).await, vec![false; 3]);
        import_rate(&pool, "USD", "2026-06", "2.5").await;
        run_snapshot_job(&pool, now).await.unwrap();
        assert_eq!(
            provisional_flags(&pool, ymd(2026, 6, 4)).await,
            vec![false; 3]
        );
        let snap = db_get(&pool, ReportKind::UnrealisedGains, ymd(2026, 6, 4))
            .await
            .unwrap()
            .unwrap();
        let gains: Vec<unrealised_gains::UnrealisedGain> =
            serde_json::from_value(snap.rows).unwrap();
        assert_eq!(gains[0].market_value, Some("566.00".parse().unwrap())); // 10 × 141.50 / 2.5
        assert!(!gains[0].fx_provisional);
    }

    /// A rate gap beyond the two-month fallback bound still blocks generation
    /// loudly — a "provisional" value that old would be meaningless.
    #[tokio::test]
    async fn db_rate_gap_beyond_fallback_bound_still_blocks() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "ICE", Some("XNYS"), "USD").await;
        insert_buy(&pool, 1, 1, ymd(2024, 1, 16), "10", "100", "USD").await;
        import_rate(&pool, "USD", "2026-03", "2").await; // 3 months before June
        store_price(&pool, 1, ymd(2026, 6, 4), "141.50").await;

        let err = generate(&pool, ymd(2026, 6, 4), friday_evening_sydney())
            .await
            .unwrap_err();
        assert!(
            matches!(err, GenerateError::Unprocessable(ref msg) if msg.contains("no ATO FX rate")),
            "{err}"
        );
        assert!(db_list(&pool, None, None, None).await.unwrap().is_empty());
    }

    /// One scheduled run backfills every missing date since the last stored
    /// snapshot (a crypto holding trades daily, so each calendar day is a
    /// snapshot date), and a blocked date is skipped — reported, the others
    /// still stored — then filled by a later run once its price exists.
    #[tokio::test]
    async fn db_job_catches_up_missing_dates_and_retries_blocked_ones() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "BTC", None, "AUD").await;
        insert_buy(&pool, 1, 1, ymd(2024, 1, 16), "0.5", "60000", "AUD").await;
        store_price(&pool, 1, ymd(2026, 6, 3), "99000").await;
        let now = utc(2026, 6, 7, 1, 30); // latest complete crypto day: 6/6
        generate(&pool, ymd(2026, 6, 3), now).await.unwrap();

        // Prices exist for 6/4 and 6/6 but 6/5 is missing: the run stores the
        // two generable dates and fails naming the blocked one.
        store_price(&pool, 1, ymd(2026, 6, 4), "99100").await;
        store_price(&pool, 1, ymd(2026, 6, 6), "99545.35").await;
        let err = run_snapshot_job(&pool, now).await.unwrap_err();
        assert!(err.contains("2026-06-05"), "{err}");
        assert!(err.contains("backfill"), "{err}");
        let dates: Vec<NaiveDate> = db_list(&pool, Some(ReportKind::UnrealisedGains), None, None)
            .await
            .unwrap()
            .iter()
            .map(|m| m.snapshot_date)
            .collect();
        assert_eq!(
            dates,
            vec![ymd(2026, 6, 3), ymd(2026, 6, 4), ymd(2026, 6, 6)],
            "unblocked dates stored; the blocked one is missing, not stale"
        );

        // The late price lands: the next run fills the hole and succeeds.
        store_price(&pool, 1, ymd(2026, 6, 5), "99200").await;
        run_snapshot_job(&pool, now).await.unwrap();
        assert_eq!(
            db_list(&pool, Some(ReportKind::UnrealisedGains), None, None)
                .await
                .unwrap()
                .len(),
            4
        );
    }

    /// The catch-up window is bounded: a gap older than the lookback is not
    /// backfilled by the job (bulk repair is the regenerate-all endpoint's
    /// job), and a stale snapshot inside the window is regenerated.
    #[tokio::test]
    async fn db_job_lookback_window_is_capped_and_regenerates_stale_in_window() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "BTC", None, "AUD").await;
        insert_buy(&pool, 1, 1, ymd(2024, 1, 16), "0.5", "60000", "AUD").await;
        let now = utc(2026, 6, 7, 1, 30); // latest complete crypto day: 6/6
        let latest = ymd(2026, 6, 6);
        let window_start = latest - Duration::days(CATCHUP_LOOKBACK_DAYS - 1); // 5/24

        // Last stored snapshot is a month old; prices exist for every day.
        store_price(&pool, 1, ymd(2026, 5, 6), "90000").await;
        let mut d = window_start - Duration::days(3);
        while d <= latest {
            store_price(&pool, 1, d, "99000").await;
            d += Duration::days(1);
        }
        generate(&pool, ymd(2026, 5, 6), now).await.unwrap();

        run_snapshot_job(&pool, now).await.unwrap();
        let dates: Vec<NaiveDate> = db_list(&pool, Some(ReportKind::UnrealisedGains), None, None)
            .await
            .unwrap()
            .iter()
            .map(|m| m.snapshot_date)
            .collect();
        let mut expected = vec![ymd(2026, 5, 6)];
        let mut d = window_start;
        while d <= latest {
            expected.push(d);
            d += Duration::days(1);
        }
        assert_eq!(dates, expected, "backfill starts at the window cap");

        // A back-dated fact stales snapshots inside the window; the next run
        // regenerates them without being asked.
        insert_buy(&pool, 2, 1, ymd(2026, 6, 1), "0.1", "95000", "AUD").await;
        assert_eq!(stale_flags(&pool, ymd(2026, 6, 6)).await, vec![true; 3]);
        run_snapshot_job(&pool, now).await.unwrap();
        assert_eq!(stale_flags(&pool, ymd(2026, 6, 6)).await, vec![false; 3]);
        assert_eq!(
            stale_flags(&pool, ymd(2026, 6, 1)).await,
            vec![false; 3],
            "every stale window date regenerated"
        );
    }

    /// Nothing held is a job no-op, and on-demand generation refuses a date
    /// whose close is not final yet (prices cannot exist for it).
    #[tokio::test]
    async fn db_job_skips_when_nothing_held_and_generate_rejects_unfinal_dates() {
        let pool = test_pool().await;
        run_snapshot_job(&pool, friday_evening_sydney())
            .await
            .unwrap();
        assert!(db_list(&pool, None, None, None).await.unwrap().is_empty());

        insert_listing(&pool, 1, "BHP", Some("XASX"), "AUD").await;
        insert_buy(&pool, 1, 1, ymd(2024, 1, 16), "100", "10", "AUD").await;
        // Friday 15:00 Sydney: Friday's close is not final yet.
        let err = generate(&pool, ymd(2026, 6, 5), utc(2026, 6, 5, 5, 0))
            .await
            .unwrap_err();
        assert!(matches!(err, GenerateError::Unprocessable(ref msg) if msg.contains("not final")));
    }

    // --- HTTP API ---

    fn app(pool: SqlitePool) -> ApiClient {
        ApiClient::over(router().with_state(pool))
    }

    async fn body_json(resp: ApiResponse) -> serde_json::Value {
        resp.json()
    }

    #[tokio::test]
    async fn api_generate_list_get_and_series() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "BHP", Some("XASX"), "AUD").await;
        insert_buy(&pool, 1, 1, ymd(2024, 1, 16), "100", "10", "AUD").await;
        store_price(&pool, 1, ymd(2026, 6, 4), "64.91").await;
        store_price(&pool, 1, ymd(2026, 6, 5), "62.48").await;
        let app = app(pool.clone());

        // Generate two days on demand (a backfilled past date + the latest).
        for date in ["2026-06-04", "2026-06-05"] {
            let resp = app
                .post_raw(
                    "/report_snapshots/generate",
                    &format!("{{\"date\":\"{date}\"}}"),
                )
                .await;
            assert_eq!(resp.status, StatusCode::OK);
            let metas = body_json(resp).await;
            assert_eq!(metas.as_array().unwrap().len(), 3);
        }

        // List, filterable by report and date.
        let resp = app
            .get("/report_snapshots?report=unrealised_gains&from=2026-06-05")
            .await;
        assert_eq!(resp.status, StatusCode::OK);
        let metas = body_json(resp).await;
        assert_eq!(metas.as_array().unwrap().len(), 1);
        assert_eq!(metas[0]["report"], "unrealised_gains");
        assert_eq!(metas[0]["stale"], false);
        assert_eq!(
            metas[0]["provisional"], false,
            "flag present in list metadata"
        );

        // Get one snapshot's stored rows.
        let resp = app
            .get("/report_snapshots/unrealised_gains/2026-06-05")
            .await;
        assert_eq!(resp.status, StatusCode::OK);
        let snap = body_json(resp).await;
        assert_eq!(snap["rows"][0]["market_value"], "6248.00");
        assert_eq!(
            snap["provisional"], false,
            "flag present on the full snapshot"
        );

        // The series feeds the graph: one point per snapshot date, with the
        // portfolio's AUD totals.
        let resp = app.get("/report_snapshots/series").await;
        assert_eq!(resp.status, StatusCode::OK);
        let series = body_json(resp).await;
        let points = series.as_array().unwrap();
        assert_eq!(points.len(), 2);
        assert_eq!(points[0]["snapshot_date"], "2026-06-04");
        assert_eq!(points[0]["market_value"], "6491.00");
        assert_eq!(points[1]["market_value"], "6248.00");
        assert_eq!(points[1]["total_cost_base"], "1000");
        assert_eq!(points[1]["unrealised_gain"], "5248.00");
        assert_eq!(
            points[0]["provisional"], false,
            "flag present on series points"
        );

        // Unknown report or date → 404.
        for uri in [
            "/report_snapshots/no_such_report/2026-06-05",
            "/report_snapshots/performance/2020-01-01",
        ] {
            let resp = app.get(uri).await;
            assert_eq!(resp.status, StatusCode::NOT_FOUND, "{uri}");
        }
    }

    /// The bulk repair endpoints: regenerate-all re-runs every stored date
    /// (reporting a blocked one without aborting the rest); regenerate-
    /// provisional touches only the provisional dates.
    #[tokio::test]
    async fn api_regenerate_all_and_provisional() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "ICE", Some("XNYS"), "USD").await;
        insert_buy(&pool, 1, 1, ymd(2024, 1, 16), "10", "100", "USD").await;
        import_rate(&pool, "USD", "2024-01", "2").await;
        import_rate(&pool, "USD", "2026-05", "2").await;
        let now = friday_evening_sydney();
        // 6/3 finalised with May's real rate; 6/4 provisional (June missing).
        store_price(&pool, 1, ymd(2026, 5, 29), "140").await;
        store_price(&pool, 1, ymd(2026, 6, 4), "141.50").await;
        generate(&pool, ymd(2026, 5, 29), now).await.unwrap();
        generate(&pool, ymd(2026, 6, 4), now).await.unwrap();
        assert_eq!(
            provisional_flags(&pool, ymd(2026, 5, 29)).await,
            vec![false; 3]
        );
        assert_eq!(
            provisional_flags(&pool, ymd(2026, 6, 4)).await,
            vec![true; 3]
        );
        let app = app(pool.clone());

        // Regenerate-provisional touches only the provisional date; June's
        // rate is still missing, so it regenerates and stays provisional.
        let resp = app
            .post_empty("/report_snapshots/regenerate_provisional")
            .await;
        assert_eq!(resp.status, StatusCode::OK);
        let summary = body_json(resp).await;
        assert_eq!(summary["regenerated"], serde_json::json!(["2026-06-04"]));
        assert_eq!(summary["blocked"].as_array().unwrap().len(), 0);
        assert_eq!(
            provisional_flags(&pool, ymd(2026, 6, 4)).await,
            vec![true; 3]
        );

        // June's rate lands; regenerate-all (narrowed to just these two
        // dates — the default range is exercised separately) re-runs both
        // and finalises the provisional one.
        import_rate(&pool, "USD", "2026-06", "2.5").await;
        let resp = app
            .post_raw(
                "/report_snapshots/regenerate_all",
                r#"{"from":"2026-05-29","to":"2026-06-04"}"#,
            )
            .await;
        assert_eq!(resp.status, StatusCode::OK);
        let summary = body_json(resp).await;
        // The range walks every calendar day 5/29..6/4: the weekend right
        // after 5/29 (Sat/Sun) walks back to Friday's price and succeeds
        // too; the weekdays 6/1-6/3 have no stored price and land in
        // `blocked` (not asserted here — the next block covers a blocked
        // date's shape).
        assert_eq!(
            summary["regenerated"],
            serde_json::json!(["2026-05-29", "2026-05-30", "2026-05-31", "2026-06-04"])
        );
        assert_eq!(
            provisional_flags(&pool, ymd(2026, 6, 4)).await,
            vec![false; 3]
        );

        // A date whose price row disappears is reported blocked; the other
        // date still regenerates.
        sqlx::query("DELETE FROM closing_prices WHERE price_date = '2026-05-29'")
            .execute(&pool)
            .await
            .unwrap();
        let resp = app
            .post_raw(
                "/report_snapshots/regenerate_all",
                r#"{"from":"2026-05-29","to":"2026-06-04"}"#,
            )
            .await;
        assert_eq!(resp.status, StatusCode::OK);
        let summary = body_json(resp).await;
        assert_eq!(summary["regenerated"], serde_json::json!(["2026-06-04"]));
        assert_eq!(summary["blocked"][0]["date"], "2026-05-29");
        assert!(
            summary["blocked"][0]["reason"]
                .as_str()
                .unwrap()
                .contains("backfill")
        );
    }

    /// Unlike a plain re-run of stored dates, a range can cover dates that
    /// never had a snapshot at all (e.g. after backfilling old closing
    /// prices) — those are generated, not just re-run. A `from` earlier than
    /// the first Buy/DRP is clamped up to it, so an over-wide caller-given
    /// range can't spin through years of no-op days; a date with nothing
    /// held is skipped rather than reported blocked.
    #[tokio::test]
    async fn db_regenerate_all_over_a_range_backfills_missing_dates() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "BHP", Some("XASX"), "AUD").await;
        insert_buy(&pool, 1, 1, ymd(2026, 6, 2), "100", "10", "AUD").await;
        for (date, price) in [
            (ymd(2026, 6, 2), "60.00"),
            (ymd(2026, 6, 3), "61.00"),
            (ymd(2026, 6, 4), "62.48"),
        ] {
            store_price(&pool, 1, date, price).await;
        }
        let now = friday_evening_sydney();
        assert!(
            db_list(&pool, None, None, None).await.unwrap().is_empty(),
            "nothing stored yet"
        );

        // A range starting well before the first Buy is clamped up to it;
        // the pre-holding days it would otherwise cover are skipped, not
        // reported blocked.
        let summary = regenerate_all(&pool, Some(ymd(2026, 5, 20)), Some(ymd(2026, 6, 4)), now)
            .await
            .unwrap();
        assert_eq!(
            summary.regenerated,
            vec![ymd(2026, 6, 2), ymd(2026, 6, 3), ymd(2026, 6, 4)]
        );
        assert!(summary.blocked.is_empty());
        let stored = db_list(&pool, None, None, None).await.unwrap();
        assert_eq!(stored.len(), 9, "3 reports x 3 dates");
    }

    /// A bulk regeneration logs each date's outcome as it completes — INFO on
    /// success, WARN when blocked — with a running done/total count, so
    /// progress on a long run is visible in the log file rather than only in
    /// the final response.
    #[tracing_test::traced_test]
    #[tokio::test]
    async fn db_regenerate_all_logs_progress_per_date() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "BHP", Some("XASX"), "AUD").await;
        insert_buy(&pool, 1, 1, ymd(2026, 6, 2), "100", "10", "AUD").await;
        store_price(&pool, 1, ymd(2026, 6, 2), "60.00").await;
        store_price(&pool, 1, ymd(2026, 6, 3), "61.00").await;
        // 6/4 has no stored price, so it stays blocked — exercising both the
        // success and blocked log lines in one range.
        let now = friday_evening_sydney();

        regenerate_all(&pool, Some(ymd(2026, 6, 2)), Some(ymd(2026, 6, 4)), now)
            .await
            .unwrap();

        assert!(logs_contain("snapshot regenerated"));
        assert!(logs_contain("done=1"));
        assert!(logs_contain("done=2"));
        assert!(logs_contain("total=3"));
        assert!(logs_contain("snapshot regeneration blocked"));
        assert!(logs_contain("done=3"));
    }

    /// `regenerate_all`'s defaults span the whole history — first-ever-held
    /// through the latest fully-valuable date — and are what
    /// `GET /report_snapshots/regenerate_range` reports for the UI to
    /// prefill; an explicit range narrows it, and a backwards range is
    /// rejected.
    #[tokio::test]
    async fn api_regenerate_all_accepts_a_date_range() {
        let pool = test_pool().await;
        // Before anything is held, the range is all-null.
        let app0 = app(pool.clone());
        let resp = app0.get("/report_snapshots/regenerate_range").await;
        assert_eq!(resp.status, StatusCode::OK);
        let range = body_json(resp).await;
        assert_eq!(range["from"], serde_json::Value::Null);
        assert_eq!(range["to"], serde_json::Value::Null);

        insert_listing(&pool, 1, "BHP", Some("XASX"), "AUD").await;
        insert_buy(&pool, 1, 1, ymd(2026, 6, 1), "100", "10", "AUD").await;
        store_price(&pool, 1, ymd(2026, 6, 2), "60.00").await;
        store_price(&pool, 1, ymd(2026, 6, 3), "61.00").await;
        store_price(&pool, 1, ymd(2026, 6, 4), "62.48").await;
        let app = app(pool.clone());

        let resp = app.get("/report_snapshots/regenerate_range").await;
        assert_eq!(resp.status, StatusCode::OK);
        let range = body_json(resp).await;
        assert_eq!(range["from"], "2026-06-01");
        // "to" is the real latest fully-valuable date (the handler uses the
        // real clock, unlike the fixed `now` the other tests inject) —
        // just assert it resolved to something on/after the stored prices.
        assert!(range["to"].as_str().unwrap() >= "2026-06-04");

        // Bodyless POST defaults to that full range and backfills it.
        let resp = app.post_empty("/report_snapshots/regenerate_all").await;
        assert_eq!(resp.status, StatusCode::OK);
        let summary = body_json(resp).await;
        assert_eq!(
            summary["regenerated"],
            serde_json::json!(["2026-06-02", "2026-06-03", "2026-06-04"])
        );

        // A backwards range is rejected.
        let resp = app
            .post_raw(
                "/report_snapshots/regenerate_all",
                r#"{"from":"2026-06-04","to":"2026-06-01"}"#,
            )
            .await;
        assert_eq!(resp.status, StatusCode::UNPROCESSABLE_ENTITY);
    }

    #[tokio::test]
    async fn api_generate_blocked_day_returns_422_with_detail() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "BHP", Some("XASX"), "AUD").await;
        insert_buy(&pool, 1, 1, ymd(2024, 1, 16), "100", "10", "AUD").await;
        let app = app(pool);

        let resp = app
            .post_raw("/report_snapshots/generate", "{\"date\":\"2026-06-03\"}")
            .await;
        assert_eq!(resp.status, StatusCode::UNPROCESSABLE_ENTITY);
        let detail = resp.text();
        assert!(
            detail.contains("BHP") && detail.contains("backfill"),
            "{detail}"
        );
    }

    // --- price collection and snapshot valuation agree ---

    /// End-to-end for the prompting case (LAAC → LAR): the scheduled
    /// collection run fills the window under the symbol in force on each
    /// date, and a snapshot dated *before* the rename then generates instead
    /// of blocking on a missing price.
    #[tokio::test]
    async fn db_a_pre_rename_date_is_valued_from_prices_the_collection_run_filled() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "LAAC", Some("XNYS"), "USD").await;
        insert_buy(&pool, 1, 1, ymd(2024, 1, 16), "100", "10", "USD").await;
        import_rate(&pool, "USD", "2024-01", "0.65").await;
        import_rate(&pool, "USD", "2026-06", "0.65").await;
        crate::entities::listing_rename::db_rename(
            &pool,
            1,
            &crate::entities::listing_rename::RenameBody {
                effective_date: ymd(2026, 6, 3),
                ticker: "LAR".to_string(),
                exchange_mic: None,
                name: None,
                price_symbol: None,
                note: None,
            },
        )
        .await
        .unwrap();

        // The provider serves the pre-rename days only under the old symbol
        // and the later ones only under the new one — as Yahoo does.
        let fetcher = closing_price::test_support::QuoteStub::default()
            .with_symbol_closes(
                "LAAC",
                "USD",
                &[(ymd(2026, 6, 1), "2.80"), (ymd(2026, 6, 2), "2.77")],
            )
            .with_symbol_closes(
                "LAR",
                "USD",
                &[
                    (ymd(2026, 6, 3), "2.73"),
                    (ymd(2026, 6, 4), "2.63"),
                    (ymd(2026, 6, 5), "2.60"),
                ],
            );
        closing_price::run_collection(&pool, &fetcher, friday_evening_sydney())
            .await
            .unwrap_err(); // the days outside the stub's range error, as expected

        // 2026-06-02 is inside the catch-up window and before the rename.
        let metas = generate(&pool, ymd(2026, 6, 2), friday_evening_sydney())
            .await
            .unwrap();
        assert_eq!(metas.len(), ReportKind::ALL.len());
        let overview = db_get(&pool, ReportKind::PortfolioOverview, ymd(2026, 6, 2))
            .await
            .unwrap()
            .unwrap();
        let rows = overview.rows.as_array().expect("rows are an array");
        assert_eq!(rows.len(), 1);
        assert!(
            !rows[0]["market_value"].is_null(),
            "the holding is valued, not silently unpriced: {}",
            overview.rows
        );
    }

    /// A split between a Buy and a Sell used to make `db_held_listing_ids`
    /// and the holdings reports disagree, so `generate` stored a row whose
    /// `market_value` was null — a holding silently missing from the totals.
    /// Every row in a stored snapshot must be priced.
    #[tokio::test]
    async fn db_a_split_holding_is_never_stored_unvalued() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "SPL", Some("XASX"), "AUD").await;
        insert_buy(&pool, 1, 1, ymd(2024, 1, 16), "100", "10", "AUD").await;
        corporate_action::db_upsert(
            &pool,
            &corporate_action::CorporateAction {
                id: 1,
                listing_id: 1,
                date: ymd(2024, 3, 1),
                kind: corporate_action::ActionKind::ShareSplit {
                    split_new_units: "2".parse().unwrap(),
                    split_old_units: "1".parse().unwrap(),
                },
            },
        )
        .await
        .unwrap();
        // 150 of the 200 post-split units sold; 50 remain.
        test_support::sell(2, 1)
            .date(ymd(2024, 6, 3))
            .settlement(ymd(2024, 6, 3))
            .qty("150".parse().unwrap())
            .price("8".parse().unwrap())
            .insert(&pool)
            .await;
        test_support::allocate(&pool, 2, 2, 1, "150".parse().unwrap()).await;
        store_price(&pool, 1, ymd(2026, 6, 5), "62.48").await;

        generate(&pool, ymd(2026, 6, 5), friday_evening_sydney())
            .await
            .unwrap();

        for kind in [ReportKind::PortfolioOverview, ReportKind::UnrealisedGains] {
            let stored = db_get(&pool, kind, ymd(2026, 6, 5)).await.unwrap().unwrap();
            let rows = stored.rows.as_array().expect("rows are an array");
            assert_eq!(rows.len(), 1, "{kind:?}");
            assert!(
                !rows[0]["market_value"].is_null(),
                "{kind:?} stored an unvalued holding: {}",
                stored.rows
            );
        }
    }
    /// SCENARIOS Q-14, end to end: the provider serves a split-adjusted
    /// history, so a day fetched after the split arrives in the post-split
    /// basis while `domain::open_parcels` re-bases the units into the
    /// snapshot date's own — and the series used to step by the split ratio
    /// at the split date (a 10-for-1 turned a holding that was up into an
    /// 89.5% "unrealised loss" the day before). Prices are now stored in the
    /// price date's own basis, so the two sides line up again.
    #[tokio::test]
    async fn db_the_valuation_series_does_not_step_at_a_split() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "NVD", Some("XASX"), "AUD").await;
        insert_buy(&pool, 1, 1, ymd(2026, 6, 1), "100", "115", "AUD").await;
        corporate_action::db_upsert(
            &pool,
            &corporate_action::CorporateAction {
                id: 1,
                listing_id: 1,
                date: ymd(2026, 6, 10),
                kind: corporate_action::ActionKind::ShareSplit {
                    split_new_units: "10".parse().unwrap(),
                    split_old_units: "1".parse().unwrap(),
                },
            },
        )
        .await
        .unwrap();

        // What the provider actually serves once the split has happened: the
        // whole series restated into the post-split basis.
        let stub = crate::entities::closing_price::test_support::QuoteStub::default()
            .with_symbol_closes(
                "NVD.AX",
                "AUD",
                &[(ymd(2026, 6, 5), "12.0888"), (ymd(2026, 6, 11), "12.10")],
            );
        let client = ApiClient::full_with(&pool, std::sync::Arc::new(stub));
        for date in ["2026-06-05", "2026-06-11"] {
            client
                .post(
                    "/closing_prices/fetch",
                    &serde_json::json!({"listing_id": 1, "price_date": date}),
                )
                .await
                .expect_status(StatusCode::OK);
        }

        let now = utc(2026, 6, 12, 8, 0);
        generate(&pool, ymd(2026, 6, 5), now).await.unwrap();
        generate(&pool, ymd(2026, 6, 11), now).await.unwrap();

        let series = db_series(&pool).await.unwrap();
        let value = |date: NaiveDate| {
            series
                .iter()
                .find(|p| p.snapshot_date == date)
                .expect("a snapshot for that date")
                .market_value
        };
        // 100 units in the 5 June basis at that day's own close of 120.888;
        // 1000 units in the 11 June basis at 12.10.
        assert_eq!(value(ymd(2026, 6, 5)), "12088.8".parse().unwrap());
        assert_eq!(value(ymd(2026, 6, 11)), "12100".parse().unwrap());

        // And the holding reads as a gain either side, not a 90% loss on the
        // day before the split.
        let overview = db_get(&pool, ReportKind::PortfolioOverview, ymd(2026, 6, 5))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(overview.rows[0]["total_cost_base"], "11500");
        assert_eq!(overview.rows[0]["market_value"], "12088.800");
    }
}
