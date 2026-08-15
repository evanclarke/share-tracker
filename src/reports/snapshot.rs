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
//! re-runs the reports with the stored prices and the *new* facts.
//!
//! Provisional (migration 0015, distinct from stale): a snapshot whose run
//! converted any price at a fallback-month FX rate — the valuation month's
//! rate was not published yet — is stored flagged `provisional`. Nothing
//! stales it when the real rate lands (FX imports fire no staleness
//! triggers); instead the RBA import true-up and the scheduled job's window
//! regenerate provisional snapshots, and a run whose conversions all used
//! real-month rates clears the flag.
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
use sqlx::{QueryBuilder, Row, SqlitePool};
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
}

/// A full snapshot: metadata plus the report's stored response rows.
#[derive(Debug, Serialize, Deserialize)]
pub struct Snapshot {
    pub report: ReportKind,
    pub snapshot_date: NaiveDate,
    pub generated_at: String,
    pub stale: bool,
    pub provisional: bool,
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

/// Stored snapshot metadata, oldest first, optionally filtered.
pub async fn db_list(
    pool: &SqlitePool,
    report: Option<ReportKind>,
    from: Option<NaiveDate>,
    to: Option<NaiveDate>,
) -> Result<Vec<SnapshotMeta>, sqlx::Error> {
    let mut qb = QueryBuilder::new(
        "SELECT report, snapshot_date, generated_at, stale, provisional \
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
        "SELECT report, snapshot_date, generated_at, stale, provisional, rows_json \
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
        "SELECT snapshot_date, stale, provisional, rows_json FROM report_snapshots \
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

/// AUD price per listing held as at `date`, from `valuation::stored_valuations`.
/// The second return value is the listings whose conversion used a
/// fallback-month rate — non-empty makes the snapshot provisional.
async fn aud_prices_for(
    pool: &SqlitePool,
    date: NaiveDate,
    now: DateTime<Utc>,
) -> Result<(HashMap<i64, Decimal>, HashSet<i64>), GenerateError> {
    let valuations = valuation::stored_valuations(pool, date, now).await?;
    let mut prices = HashMap::new();
    let mut provisional: HashSet<i64> = HashSet::new();
    for v in valuations {
        prices.insert(v.listing_id, v.aud_price);
        if v.provisional {
            provisional.insert(v.listing_id);
        }
    }
    Ok((prices, provisional))
}

/// Generate (or regenerate) the three snapshots for `date` and store them in
/// one transaction, replacing any stored result and clearing its stale flag.
/// The stored `provisional` flag is set iff any price conversion in this run
/// used a fallback-month FX rate — so regenerating once the real rate is
/// imported clears it.
pub async fn generate(
    pool: &SqlitePool,
    date: NaiveDate,
    now: DateTime<Utc>,
) -> Result<Vec<SnapshotMeta>, GenerateError> {
    let (prices, provisional_ids) = aud_prices_for(pool, date, now).await?;
    let provisional = !provisional_ids.is_empty();

    let mut overview = portfolio::db_holdings(pool, Some(date)).await?;
    for h in &mut overview {
        if let Some(&price) = prices.get(&h.listing_id) {
            h.current_price = Some(price);
            h.market_value = Some(h.quantity * price);
            h.fx_provisional = provisional_ids.contains(&h.listing_id);
        }
    }
    let mut gains = unrealised_gains::db_unrealised_gains(pool, date).await?;
    for g in &mut gains {
        if let Some(&price) = prices.get(&g.listing_id) {
            g.current_price = Some(price);
            g.market_value = Some(g.quantity * price);
            g.unrealised_gain_loss = Some(g.quantity * price - g.total_cost_base);
            g.fx_provisional = provisional_ids.contains(&g.listing_id);
        }
    }
    let mut perf = performance::db_performance(pool, &prices, date).await?;
    for row in &mut perf {
        if let Some(listing_id) = row.listing_id {
            row.fx_provisional = provisional_ids.contains(&listing_id);
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
    let mut tx = pool.begin().await?;
    for (kind, rows_json) in &payloads {
        sqlx::query(
            "INSERT INTO report_snapshots \
                 (report, snapshot_date, generated_at, stale, provisional, rows_json) \
             VALUES (?, ?, ?, 0, ?, ?) \
             ON CONFLICT(report, snapshot_date) DO UPDATE SET \
                 generated_at = excluded.generated_at, \
                 stale = 0, \
                 provisional = excluded.provisional, \
                 rows_json = excluded.rows_json",
        )
        .bind(kind)
        .bind(date)
        .bind(&generated_at)
        .bind(provisional)
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
        insert_buy(&pool, 1, 1, ymd(2024, 1, 15), "100", "10", "AUD").await;
        insert_buy(&pool, 2, 2, ymd(2024, 1, 15), "10", "100", "USD").await;
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
        insert_buy(&pool, 1, 1, ymd(2024, 1, 15), "100", "10", "AUD").await;
        insert_buy(&pool, 2, 2, ymd(2024, 1, 15), "0.5", "60000", "AUD").await;
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
        insert_buy(&pool, 1, 1, ymd(2024, 1, 15), "100", "10", "AUD").await;
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
        insert_buy(&pool, 1, 1, ymd(2024, 1, 15), "100", "10", "AUD").await;
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

    /// A day whose price fetch failed has no trustworthy snapshot: generation
    /// refuses (the job fails, naming the listing), nothing is stored —
    /// missing, not stale — and the day becomes generable once the price
    /// re-run succeeds.
    #[tokio::test]
    async fn db_failed_price_day_yields_no_snapshot_until_the_price_rerun_succeeds() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "BHP", Some("XASX"), "AUD").await;
        insert_buy(&pool, 1, 1, ymd(2024, 1, 15), "100", "10", "AUD").await;
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
        insert_buy(&pool, 1, 1, ymd(2024, 1, 15), "10", "100", "USD").await;
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
        insert_buy(&pool, 1, 1, ymd(2024, 1, 15), "10", "100", "USD").await;
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
        insert_buy(&pool, 1, 1, ymd(2024, 1, 15), "0.5", "60000", "AUD").await;
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
        insert_buy(&pool, 1, 1, ymd(2024, 1, 15), "0.5", "60000", "AUD").await;
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
        insert_buy(&pool, 1, 1, ymd(2024, 1, 15), "100", "10", "AUD").await;
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
        insert_buy(&pool, 1, 1, ymd(2024, 1, 15), "100", "10", "AUD").await;
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
        insert_buy(&pool, 1, 1, ymd(2024, 1, 15), "10", "100", "USD").await;
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
        insert_buy(&pool, 1, 1, ymd(2024, 1, 15), "100", "10", "AUD").await;
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
        insert_buy(&pool, 1, 1, ymd(2024, 1, 15), "100", "10", "USD").await;
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
        insert_buy(&pool, 1, 1, ymd(2024, 1, 15), "100", "10", "AUD").await;
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
            .date(ymd(2024, 6, 1))
            .settlement(ymd(2024, 6, 1))
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
}
