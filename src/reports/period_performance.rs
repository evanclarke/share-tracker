//! Period performance: how the portfolio's AUD value changed between two
//! dates, attributed to capital growth, FX movement, and income.
//!
//! The window is half-open `(from, to]` — a trade or income payment dated
//! `from` counts as already inside the opening value, matching the existing
//! cumulative-to-`as_of` convention in `reports::performance`. Per holding
//! `h` (listing × holding account):
//!
//! ```text
//! V0(h) = opening AUD market value (0 if not held at `from`)
//! V1(h) = closing AUD market value (0 if not held at `to`)
//! C(h)  = AUD cost of Buy/DRP trades in (from, to]
//! P(h)  = AUD net proceeds of Sells in (from, to]
//! I(h)  = AUD cash income received in (from, to]
//!
//! R(h)       = V1 + P + I - V0 - C            period return
//! FX(h)      = q1(h)*native_price1(h)*(1/r1 - 1/r0)   closing exposure
//!                                                       revalued at the
//!                                                       opening vs closing
//!                                                       month's FX rate
//! Income(h)  = I(h)
//! Capital(h) = R(h) - FX(h) - Income(h)       residual
//! ```
//!
//! `Capital + FX + Income = R` always, by construction — no split-factor
//! arithmetic is needed and a holding opened or closed mid-window, or split
//! inside it, needs no special case. `R(h)` is not recomputed from scratch:
//! it equals `reports::performance`'s cumulative total return at `to` minus
//! at `from` (that computation already handles internal movements —
//! transfers, scrip-for-scrip, demergers — correctly at portfolio level), so
//! this report runs it at both endpoints and subtracts. `FX` needs the
//! native-currency close and the rate actually used, which the stored-price
//! valuation path (`reports::valuation`) provides.
//!
//! `capital_growth` is an investment-performance figure, not the tax realised
//! capital gain — `realised_capital_gain` is reported alongside it,
//! separately, as an informational cross-check (see `docs/API.md`'s Known
//! limitations entry for this report).

use crate::infra::fx::{FxError, FxRates};
use crate::infra::http::ApiError;
use crate::reports::{performance, realised_gains, valuation};
use axum::{Json, Router, extract::State, routing::post};
use chrono::{DateTime, NaiveDate, Utc};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;
use std::collections::{BTreeMap, HashMap};

#[derive(Debug, Deserialize)]
pub struct PeriodRequest {
    pub from: NaiveDate,
    pub to: NaiveDate,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HoldingPeriod {
    pub listing_id: i64,
    pub holding_account_id: i64,
    pub opening_market_value: Decimal,
    pub closing_market_value: Decimal,
    pub purchases: Decimal,
    pub sale_proceeds: Decimal,
    pub income: Decimal,
    pub capital_growth: Decimal,
    pub fx_movement: Decimal,
    pub total_return: Decimal,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CurrencyFx {
    pub currency: String,
    pub fx_movement: Decimal,
    /// The opening/closing FX rates used, when every listing in this
    /// currency resolved the same pair — omitted (not zeroed) when they
    /// differed (listings valued at different months near a boundary).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rate_from: Option<Decimal>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rate_to: Option<Decimal>,
    pub provisional: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PeriodPerformance {
    pub from: NaiveDate,
    pub to: NaiveDate,
    pub opening_market_value: Decimal,
    pub closing_market_value: Decimal,
    pub purchases: Decimal,
    pub sale_proceeds: Decimal,
    pub income: Decimal,
    pub capital_growth: Decimal,
    pub fx_movement: Decimal,
    pub total_return: Decimal,
    /// `total_return / opening_market_value * 100`. `None` when the window
    /// opened with nothing held.
    pub total_return_pct: Option<Decimal>,
    /// The tax realised capital gain for disposals in the window (sum of
    /// `reports::realised_gains` rows with `sale_date` in `(from, to]`) —
    /// informational only, not part of the additive capital/FX/income split.
    pub realised_capital_gain: Decimal,
    /// Any conversion at either endpoint used a fallback-month FX rate
    /// (`infra::fx::resolve_valuation_rate`) — the period result is
    /// provisional, same convention as report snapshots.
    pub provisional: bool,
    pub holdings: Vec<HoldingPeriod>,
    pub fx_by_currency: Vec<CurrencyFx>,
}

/// Why a period performance figure could not be computed. `Unprocessable`
/// carries the human detail (invalid range, a blocked valuation) and maps to
/// HTTP 422; anything else is a 500.
#[derive(Debug)]
pub enum PeriodError {
    Unprocessable(String),
    Db(String),
}

impl std::fmt::Display for PeriodError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PeriodError::Unprocessable(msg) | PeriodError::Db(msg) => write!(f, "{msg}"),
        }
    }
}

impl From<sqlx::Error> for PeriodError {
    fn from(e: sqlx::Error) -> Self {
        PeriodError::Db(e.to_string())
    }
}

impl From<valuation::ValuationError> for PeriodError {
    fn from(e: valuation::ValuationError) -> Self {
        match e {
            valuation::ValuationError::Unprocessable(msg) => PeriodError::Unprocessable(msg),
            valuation::ValuationError::Db(msg) => PeriodError::Db(msg),
        }
    }
}

impl From<FxError> for PeriodError {
    fn from(e: FxError) -> Self {
        match e {
            FxError::MissingRate { .. } => PeriodError::Unprocessable(e.to_string()),
            FxError::Db(inner) => PeriodError::Db(inner.to_string()),
        }
    }
}

impl From<PeriodError> for ApiError {
    fn from(e: PeriodError) -> Self {
        match e {
            PeriodError::Unprocessable(msg) => ApiError::Unprocessable(msg),
            PeriodError::Db(msg) => ApiError::internal(msg),
        }
    }
}

/// Every held listing's stored valuation at `date`, or empty when nothing was
/// held then (an inception-style `from` before the first holding) — unlike
/// `valuation::stored_valuations`, which treats "nothing held" as a blocker
/// (the right behaviour for snapshot generation, where the caller already
/// knows something is held). A date with holdings that can't be fully valued
/// (missing/errored price) still fails loudly.
async fn valuations_or_empty(
    pool: &SqlitePool,
    date: NaiveDate,
    now: DateTime<Utc>,
) -> Result<Vec<valuation::ListingValuation>, PeriodError> {
    let markets = valuation::held_markets(pool, Some(date)).await?;
    if markets.is_empty() {
        return Ok(vec![]);
    }
    Ok(valuation::stored_valuations(pool, date, now).await?)
}

/// Per-currency FX accumulator: the running total, the (rate_from, rate_to)
/// pair while every listing seen so far agrees (cleared to `None` the moment
/// one disagrees), and whether any contributor was provisional.
#[derive(Default)]
struct CurrencyAcc {
    total: Decimal,
    rates: Option<(Decimal, Decimal)>,
    rates_set: bool,
    provisional: bool,
}

pub async fn compute(
    pool: &SqlitePool,
    from: NaiveDate,
    to: NaiveDate,
    now: DateTime<Utc>,
) -> Result<PeriodPerformance, PeriodError> {
    if from >= to {
        return Err(PeriodError::Unprocessable(format!(
            "the period must start before it ends (from {from}, to {to})"
        )));
    }

    let valuations_from = valuations_or_empty(pool, from, now).await?;
    let valuations_to = valuations_or_empty(pool, to, now).await?;
    let val_from_by_listing: HashMap<i64, &valuation::ListingValuation> =
        valuations_from.iter().map(|v| (v.listing_id, v)).collect();
    let val_to_by_listing: HashMap<i64, &valuation::ListingValuation> =
        valuations_to.iter().map(|v| (v.listing_id, v)).collect();
    let prices_from: HashMap<i64, Decimal> = valuations_from
        .iter()
        .map(|v| (v.listing_id, v.aud_price))
        .collect();
    let prices_to: HashMap<i64, Decimal> = valuations_to
        .iter()
        .map(|v| (v.listing_id, v.aud_price))
        .collect();
    let mut provisional = valuations_from.iter().any(|v| v.provisional)
        || valuations_to.iter().any(|v| v.provisional);

    let perf_from = performance::db_performance(pool, &prices_from, from).await?;
    let perf_to = performance::db_performance(pool, &prices_to, to).await?;

    // Index both endpoints by (listing_id, holding_account_id); the OVERALL
    // row (listing_id: None) is skipped — portfolio totals are summed from
    // the per-holding rows below, so they match the per-holding breakdown by
    // construction rather than needing to be cross-checked against it.
    type Endpoints<'a> = (
        Option<&'a performance::HoldingPerformance>,
        Option<&'a performance::HoldingPerformance>,
    );
    let mut by_key: BTreeMap<(i64, i64), Endpoints> = BTreeMap::new();
    for r in &perf_from {
        if let (Some(l), Some(a)) = (r.listing_id, r.holding_account_id) {
            by_key.entry((l, a)).or_default().0 = Some(r);
        }
    }
    for r in &perf_to {
        if let (Some(l), Some(a)) = (r.listing_id, r.holding_account_id) {
            by_key.entry((l, a)).or_default().1 = Some(r);
        }
    }

    // Only reached for a listing whose closing quantity is positive (else FX
    // is defined as zero — see the module docs on the closing-exposure
    // convention), so a rate lookup failure here is a real blocker.
    let fx = FxRates::load(pool).await?;

    let mut holdings = Vec::with_capacity(by_key.len());
    let mut currency_acc: BTreeMap<String, CurrencyAcc> = BTreeMap::new();
    let mut opening_total = Decimal::ZERO;
    let mut closing_total = Decimal::ZERO;
    let mut purchases_total = Decimal::ZERO;
    let mut proceeds_total = Decimal::ZERO;
    let mut income_total = Decimal::ZERO;
    let mut fx_total = Decimal::ZERO;
    let mut return_total = Decimal::ZERO;

    for (&(listing_id, account_id), (f, t)) in &by_key {
        // Every open (quantity > 0) holding at a date is guaranteed a price
        // by `valuations_or_empty`/`stored_valuations` (it fails loudly
        // otherwise), so a `None` market_value here only ever means the
        // holding was closed (quantity 0) at that date — safe to treat as 0.
        let v0 = f.and_then(|r| r.market_value).unwrap_or(Decimal::ZERO);
        let v1 = t.and_then(|r| r.market_value).unwrap_or(Decimal::ZERO);
        let purchases =
            t.map_or(Decimal::ZERO, |r| r.invested) - f.map_or(Decimal::ZERO, |r| r.invested);
        let sale_proceeds =
            t.map_or(Decimal::ZERO, |r| r.proceeds) - f.map_or(Decimal::ZERO, |r| r.proceeds);
        let income = t.map_or(Decimal::ZERO, |r| r.income) - f.map_or(Decimal::ZERO, |r| r.income);
        let total_return = (v1 - v0) + sale_proceeds + income - purchases;

        let qty1 = t.and_then(|r| r.quantity_held).unwrap_or(Decimal::ZERO);
        let fx_movement = if qty1.is_zero() {
            Decimal::ZERO
        } else if let Some(val_to) = val_to_by_listing.get(&listing_id) {
            if val_to.currency.eq_ignore_ascii_case("AUD") {
                Decimal::ZERO
            } else {
                let (rate_from, from_provisional) = match val_from_by_listing.get(&listing_id) {
                    Some(val_from) => (val_from.rate, val_from.provisional),
                    None => {
                        let vr = fx.resolve_valuation_rate(&val_to.currency, from)?;
                        (vr.rate, vr.provisional)
                    }
                };
                if from_provisional || val_to.provisional {
                    provisional = true;
                }
                let exposure = qty1 * val_to.native_price;
                let movement = exposure / val_to.rate - exposure / rate_from;
                let acc = currency_acc.entry(val_to.currency.clone()).or_default();
                acc.total += movement;
                if !acc.rates_set {
                    acc.rates = Some((rate_from, val_to.rate));
                    acc.rates_set = true;
                } else if acc.rates != Some((rate_from, val_to.rate)) {
                    acc.rates = None;
                }
                if from_provisional || val_to.provisional {
                    acc.provisional = true;
                }
                movement
            }
        } else {
            // Closed by `to` (not in val_to_by_listing despite qty1 > 0 is
            // impossible — qty1 comes from the same valuation run — so this
            // is unreachable in practice; kept as a safe zero rather than a
            // panic).
            Decimal::ZERO
        };

        let capital_growth = total_return - fx_movement - income;

        opening_total += v0;
        closing_total += v1;
        purchases_total += purchases;
        proceeds_total += sale_proceeds;
        income_total += income;
        fx_total += fx_movement;
        return_total += total_return;

        holdings.push(HoldingPeriod {
            listing_id,
            holding_account_id: account_id,
            opening_market_value: v0,
            closing_market_value: v1,
            purchases,
            sale_proceeds,
            income,
            capital_growth,
            fx_movement,
            total_return,
        });
    }

    let capital_total = return_total - fx_total - income_total;
    let total_return_pct = (opening_total > Decimal::ZERO)
        .then(|| (return_total / opening_total * Decimal::from(100)).round_dp(4));

    let realised_capital_gain = realised_gains::db_realised_gains(pool)
        .await?
        .into_iter()
        .filter(|g| g.sale_date > from && g.sale_date <= to)
        .map(|g| g.capital_gain_loss)
        .sum();

    let fx_by_currency = currency_acc
        .into_iter()
        .map(|(currency, acc)| CurrencyFx {
            currency,
            fx_movement: acc.total,
            rate_from: acc.rates.map(|(f, _)| f),
            rate_to: acc.rates.map(|(_, t)| t),
            provisional: acc.provisional,
        })
        .collect();

    Ok(PeriodPerformance {
        from,
        to,
        opening_market_value: opening_total,
        closing_market_value: closing_total,
        purchases: purchases_total,
        sale_proceeds: proceeds_total,
        income: income_total,
        capital_growth: capital_total,
        fx_movement: fx_total,
        total_return: return_total,
        total_return_pct,
        realised_capital_gain,
        provisional,
        holdings,
        fx_by_currency,
    })
}

async fn period_performance_handler(
    State(pool): State<SqlitePool>,
    Json(req): Json<PeriodRequest>,
) -> Result<Json<PeriodPerformance>, ApiError> {
    let result = compute(&pool, req.from, req.to, Utc::now()).await?;
    Ok(Json(result))
}

pub fn router() -> Router<SqlitePool> {
    Router::new().route(
        "/portfolio/period-performance",
        post(period_performance_handler),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{self, dec, test_pool, ymd};
    use axum::http::StatusCode;
    use axum::{body::Body, http::Request};
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    /// Two calendar days after `date` at 00:00 UTC — comfortably past the
    /// crypto valuation rule (`latest_complete_trading_day` = yesterday's UTC
    /// date), so a crypto listing's close on `date` is always final.
    fn now_after(date: NaiveDate) -> DateTime<Utc> {
        (date + chrono::Duration::days(2))
            .and_hms_opt(0, 0, 0)
            .unwrap()
            .and_utc()
    }

    /// A crypto listing (no exchange calendar — every day trades, sidestepping
    /// weekday/holiday bookkeeping in these tests) in `ccy`.
    async fn insert_listing(pool: &SqlitePool, id: i64, ticker: &str, ccy: &str) {
        test_support::listing(id)
            .ticker(ticker)
            .name(ticker)
            .currency(ccy)
            .crypto()
            .insert(pool)
            .await;
    }

    async fn store_price(pool: &SqlitePool, listing_id: i64, date: NaiveDate, price: &str) {
        sqlx::query(
            "INSERT INTO closing_prices (listing_id, price_date, price, source, fetched_at, status, error) \
             VALUES (?, ?, ?, 'test', '2026-01-01T00:00:00Z', 'ok', NULL)",
        )
        .bind(listing_id)
        .bind(date)
        .bind(price)
        .execute(pool)
        .await
        .unwrap();
    }

    async fn insert_fx_rate(pool: &SqlitePool, currency: &str, month: &str, rate: &str) {
        sqlx::query("INSERT INTO rba_fx_rates (currency, month, rate) VALUES (?, ?, ?)")
            .bind(currency)
            .bind(month)
            .bind(rate)
            .execute(pool)
            .await
            .unwrap();
    }

    /// AUD holding, no trades or income in the window, price only moves: the
    /// whole period return lands in capital growth and FX is exactly zero.
    #[tokio::test]
    async fn additivity_and_zero_fx_for_aud_holding() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "BTC", "AUD").await;
        test_support::buy(1, 1)
            .date(ymd(2024, 1, 1))
            .qty(dec("100"))
            .price(dec("10"))
            .currency("AUD")
            .insert(&pool)
            .await;
        let from = ymd(2026, 6, 1);
        let to = ymd(2026, 7, 1);
        store_price(&pool, 1, from, "12").await;
        store_price(&pool, 1, to, "15").await;

        let r = compute(&pool, from, to, now_after(to)).await.unwrap();
        assert_eq!(r.opening_market_value, dec("1200"));
        assert_eq!(r.closing_market_value, dec("1500"));
        assert_eq!(r.purchases, Decimal::ZERO);
        assert_eq!(r.sale_proceeds, Decimal::ZERO);
        assert_eq!(r.income, Decimal::ZERO);
        assert_eq!(r.fx_movement, Decimal::ZERO);
        assert_eq!(r.capital_growth, dec("300"));
        assert_eq!(r.total_return, dec("300"));
        assert_eq!(r.total_return_pct, Some(dec("25.0000")));
        assert!(!r.provisional);
        assert!(r.fx_by_currency.is_empty(), "AUD contributes no FX line");
        assert_eq!(r.holdings.len(), 1);
        assert_eq!(
            r.capital_growth + r.fx_movement + r.income,
            r.total_return,
            "capital + FX + income must sum exactly to the period return"
        );
    }

    /// A USD holding with an unchanged native price but a moved monthly ATO
    /// rate: the whole return is FX movement, hand-computed, and capital
    /// growth is exactly zero.
    #[tokio::test]
    async fn usd_fx_movement_hand_computed() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "BTC", "USD").await;
        test_support::buy(1, 1)
            .date(ymd(2024, 1, 1))
            .qty(dec("10"))
            .price(dec("100"))
            .currency("USD")
            .insert(&pool)
            .await;
        let from = ymd(2026, 6, 1);
        let to = ymd(2026, 7, 1);
        insert_fx_rate(&pool, "USD", "2026-06", "2").await;
        insert_fx_rate(&pool, "USD", "2026-07", "4").await;
        store_price(&pool, 1, from, "100").await;
        store_price(&pool, 1, to, "100").await;

        let r = compute(&pool, from, to, now_after(to)).await.unwrap();
        // V0 = 10*100/2 = 500, V1 = 10*100/4 = 250.
        assert_eq!(r.opening_market_value, dec("500"));
        assert_eq!(r.closing_market_value, dec("250"));
        assert_eq!(r.total_return, dec("-250"));
        // exposure(1000) * (1/4 - 1/2) = -250.
        assert_eq!(r.fx_movement, dec("-250"));
        assert_eq!(r.capital_growth, Decimal::ZERO);
        assert!(!r.provisional);
        assert_eq!(r.fx_by_currency.len(), 1);
        let usd = &r.fx_by_currency[0];
        assert_eq!(usd.currency, "USD");
        assert_eq!(usd.fx_movement, dec("-250"));
        assert_eq!(usd.rate_from, Some(dec("2")));
        assert_eq!(usd.rate_to, Some(dec("4")));
        assert!(!usd.provisional);
    }

    /// One holding opened entirely inside the window (opening value zero)
    /// and another fully closed inside it (closing value and FX zero,
    /// realised as a genuine capital gain on disposal).
    #[tokio::test]
    async fn holding_opened_and_closed_mid_window() {
        let pool = test_pool().await;
        let from = ymd(2026, 6, 1);
        let to = ymd(2026, 7, 1);

        // Opened mid-window: bought 2026-06-10, not held at `from`.
        insert_listing(&pool, 10, "BTC", "AUD").await;
        test_support::buy(10, 10)
            .date(ymd(2026, 6, 10))
            .qty(dec("50"))
            .price(dec("20"))
            .currency("AUD")
            .insert(&pool)
            .await;
        store_price(&pool, 10, to, "25").await;

        // Closed mid-window: bought before `from`, fully sold 2026-06-15.
        insert_listing(&pool, 11, "ETH", "AUD").await;
        test_support::buy(11, 11)
            .date(ymd(2024, 1, 1))
            .qty(dec("30"))
            .price(dec("10"))
            .currency("AUD")
            .insert(&pool)
            .await;
        test_support::sell(12, 11)
            .date(ymd(2026, 6, 15))
            .qty(dec("30"))
            .price(dec("15"))
            .currency("AUD")
            .insert(&pool)
            .await;
        test_support::allocate(&pool, 1, 12, 11, dec("30")).await;
        store_price(&pool, 11, from, "12").await;

        let r = compute(&pool, from, to, now_after(to)).await.unwrap();
        assert_eq!(r.holdings.len(), 2);
        let newco = r
            .holdings
            .iter()
            .find(|h| h.listing_id == 10)
            .expect("NEWCO row");
        assert_eq!(newco.opening_market_value, Decimal::ZERO);
        assert_eq!(newco.closing_market_value, dec("1250"));
        assert_eq!(newco.purchases, dec("1000"));
        assert_eq!(newco.capital_growth, dec("250"));
        assert_eq!(newco.fx_movement, Decimal::ZERO);

        let oldco = r
            .holdings
            .iter()
            .find(|h| h.listing_id == 11)
            .expect("OLDCO row");
        assert_eq!(oldco.opening_market_value, dec("360"));
        assert_eq!(oldco.closing_market_value, Decimal::ZERO);
        assert_eq!(oldco.sale_proceeds, dec("450"));
        assert_eq!(oldco.fx_movement, Decimal::ZERO);
        assert_eq!(oldco.capital_growth, dec("90"));

        assert_eq!(r.opening_market_value, dec("360"));
        assert_eq!(r.closing_market_value, dec("1250"));
        assert_eq!(r.total_return, dec("340"));
        assert_eq!(r.total_return_pct, Some(dec("94.4444")));
        // The disposal (proceeds 450, cost base 300) is a real realised gain.
        assert_eq!(r.realised_capital_gain, dec("150"));
        for h in &r.holdings {
            assert_eq!(
                h.capital_growth + h.fx_movement + h.income,
                h.total_return,
                "per-holding additivity for listing {}",
                h.listing_id
            );
        }
    }

    /// A window that opens with nothing held (the holding is bought entirely
    /// inside it) reports a null return percentage rather than dividing by
    /// zero.
    #[tokio::test]
    async fn total_return_pct_is_null_when_nothing_held_at_open() {
        let pool = test_pool().await;
        let from = ymd(2026, 6, 1);
        let to = ymd(2026, 7, 1);
        insert_listing(&pool, 1, "BTC", "AUD").await;
        test_support::buy(1, 1)
            .date(ymd(2026, 6, 10))
            .qty(dec("50"))
            .price(dec("20"))
            .currency("AUD")
            .insert(&pool)
            .await;
        store_price(&pool, 1, to, "25").await;

        let r = compute(&pool, from, to, now_after(to)).await.unwrap();
        assert_eq!(r.opening_market_value, Decimal::ZERO);
        assert_eq!(r.total_return_pct, None);
    }

    /// Cash income with no price movement: the whole period return lands in
    /// the income bucket and capital growth is exactly zero.
    #[tokio::test]
    async fn income_only_period_all_return_is_income() {
        let pool = test_pool().await;
        let from = ymd(2026, 6, 1);
        let to = ymd(2026, 7, 1);
        insert_listing(&pool, 1, "BTC", "AUD").await;
        test_support::buy(1, 1)
            .date(ymd(2024, 1, 1))
            .qty(dec("100"))
            .price(dec("10"))
            .currency("AUD")
            .insert(&pool)
            .await;
        store_price(&pool, 1, from, "12").await;
        store_price(&pool, 1, to, "12").await;
        test_support::income(1, 1, ymd(2026, 6, 20))
            .with(|i| i.unfranked_amount = dec("50"))
            .insert(&pool)
            .await;

        let r = compute(&pool, from, to, now_after(to)).await.unwrap();
        assert_eq!(r.income, dec("50"));
        assert_eq!(r.capital_growth, Decimal::ZERO);
        assert_eq!(r.total_return, dec("50"));
    }

    /// A `to`-month ATO rate not yet imported falls back to the previous
    /// month's, flagging the whole period result (and that currency's line)
    /// provisional.
    #[tokio::test]
    async fn provisional_rate_at_either_endpoint_propagates() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "BTC", "USD").await;
        test_support::buy(1, 1)
            .date(ymd(2024, 1, 1))
            .qty(dec("10"))
            .price(dec("100"))
            .currency("USD")
            .insert(&pool)
            .await;
        let from = ymd(2026, 5, 1);
        let to = ymd(2026, 7, 1);
        insert_fx_rate(&pool, "USD", "2026-05", "2").await;
        insert_fx_rate(&pool, "USD", "2026-06", "3").await; // no 2026-07 entry
        store_price(&pool, 1, from, "100").await;
        store_price(&pool, 1, to, "100").await;

        let r = compute(&pool, from, to, now_after(to)).await.unwrap();
        assert!(r.provisional);
        assert!(r.fx_by_currency[0].provisional);
        assert_eq!(r.fx_by_currency[0].rate_to, Some(dec("3")));
    }

    #[tokio::test]
    async fn invalid_range_is_422() {
        let pool = test_pool().await;
        let err = compute(
            &pool,
            ymd(2026, 7, 1),
            ymd(2026, 6, 1),
            now_after(ymd(2026, 7, 1)),
        )
        .await
        .unwrap_err();
        assert!(matches!(err, PeriodError::Unprocessable(_)));

        let err = compute(
            &pool,
            ymd(2026, 6, 1),
            ymd(2026, 6, 1),
            now_after(ymd(2026, 7, 1)),
        )
        .await
        .unwrap_err();
        assert!(matches!(err, PeriodError::Unprocessable(_)));
    }

    /// A held listing with no stored price at `to` blocks the whole period
    /// (the same "no partial result" rule as snapshot generation).
    #[tokio::test]
    async fn missing_price_at_endpoint_is_422() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "BTC", "AUD").await;
        test_support::buy(1, 1)
            .date(ymd(2024, 1, 1))
            .qty(dec("100"))
            .price(dec("10"))
            .currency("AUD")
            .insert(&pool)
            .await;
        let from = ymd(2026, 6, 1);
        let to = ymd(2026, 7, 1);
        store_price(&pool, 1, from, "12").await;
        // no price stored at `to`

        let err = compute(&pool, from, to, now_after(to)).await.unwrap_err();
        assert!(matches!(err, PeriodError::Unprocessable(_)));
    }

    // API-level tests

    #[tokio::test]
    async fn api_period_performance_ok() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "BTC", "AUD").await;
        test_support::buy(1, 1)
            .date(ymd(2024, 1, 1))
            .qty(dec("100"))
            .price(dec("10"))
            .currency("AUD")
            .insert(&pool)
            .await;
        let from = ymd(2026, 6, 1);
        let to = ymd(2026, 7, 1);
        store_price(&pool, 1, from, "12").await;
        store_price(&pool, 1, to, "15").await;

        let body = serde_json::json!({ "from": from, "to": to });
        let resp = router()
            .with_state(pool)
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/portfolio/period-performance")
                    .header("content-type", "application/json")
                    .body(Body::from(body.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let result: PeriodPerformance = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(result.capital_growth, dec("300"));
    }

    #[tokio::test]
    async fn api_period_performance_invalid_range_is_422() {
        let pool = test_pool().await;
        let body = serde_json::json!({ "from": "2026-07-01", "to": "2026-06-01" });
        let resp = router()
            .with_state(pool)
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/portfolio/period-performance")
                    .header("content-type", "application/json")
                    .body(Body::from(body.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY);
    }
}
