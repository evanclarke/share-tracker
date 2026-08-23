//! Investment performance (not tax): total return, money-weighted return, and
//! income yield, per holding (listing × holding account) and overall.
//!
//! The report is cash-flow based, valued at `as_of_date` with the supplied AUD
//! prices (trades and income dated after `as_of_date` are ignored). The flows:
//!
//! - **Out**: each Buy/DRP parcel's AUD cost (price × qty + brokerage + GST,
//!   converted at the acquisition month — the deemed acquisition month for a
//!   rollover-created parcel, preserving its AUD cost) on the trade date.
//! - **In**: each Sell's AUD net proceeds (price × qty − brokerage − GST,
//!   converted at the sale month) on the sale date; cash income (franked +
//!   unfranked + foreign source − foreign tax − TFN withholding — the cash
//!   actually received, excluding franking credits, same as the DRP
//!   reinvestable-cash definition) on the pay date; and the holding's market
//!   value (current units × supplied price) at `as_of_date`.
//!
//! Internal movements — a holding-account transfer, a scrip-for-scrip exchange,
//! or a demerger (`transfer_id` / `scrip_action_id` / `demerger_action_id`
//! groups) — are not external cash. Per holding they are valued **at the
//! carried cost**: the source holding's closing Sell counts as an inflow equal
//! to the cost base the replacement parcels carried away, and each replacement
//! parcel is an outflow of that carried cost — so a holding's return reflects
//! only its own period and any deferred gain shows up where the parcels now
//! sit. In the **OVERALL** row those internal legs are skipped entirely (they
//! net to zero), leaving only external cash, so portfolio-level figures are
//! unaffected by moving parcels around. The cash component of a
//! partial-rollover scrip exchange (the closing Sell's price × quantity) is
//! real external cash on top of the carried cost, and reaches OVERALL too.
//! A rights exercise and a buy-back participation are real cash and count as
//! ordinary flows. AMMA statements
//! attribute taxable income, not cash, and are excluded; a DRP reinvestment is
//! both the cash income and a same-sized purchase, so it nets out naturally.
//!
//! Per row: `total_return` (AUD: proceeds + income + market value − invested),
//! `total_return_pct` (of invested), `money_weighted_return_pct` (the
//! annualised internal rate of return over the dated flows, actual/365),
//! `income_yield_pct` (the trailing 12 months' income over market value). A
//! still-open holding with no supplied price reports `null` for the
//! market-dependent metrics rather than a silently wrong figure; the OVERALL
//! row does the same unless every open holding is priced.

use crate::domain::cost_base::ParcelRow;
use crate::domain::open_parcels;
use crate::entities::closing_price::{self, SharedFetcher};
use crate::entities::income::{Income, IncomeType};
use crate::entities::trade::TradeType;
use crate::infra::fx::{FxOverride, FxRates};
use crate::infra::http::ApiError;
use axum::{Extension, Json, Router, extract::State, routing::post};
use chrono::{Months, NaiveDate};
use rust_decimal::{Decimal, MathematicalOps};
use serde::{Deserialize, Serialize};
use sqlx::{Row, SqlitePool};
use std::collections::{BTreeMap, HashMap};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HoldingPerformance {
    /// `None` on the OVERALL row.
    pub listing_id: Option<i64>,
    /// The listing's ticker, or `"OVERALL"` on the portfolio-total row.
    pub ticker: String,
    /// `None` on the OVERALL row.
    pub holding_account_id: Option<i64>,
    /// Units held at `as_of_date` (in as-of units, after splits). `None` on
    /// the OVERALL row — units don't aggregate across listings.
    pub quantity_held: Option<Decimal>,
    /// Total AUD acquisition cost (per holding: including the carried cost of
    /// rollover/transfer-created parcels; OVERALL: external purchases only).
    pub invested: Decimal,
    /// Total AUD sale proceeds (per holding: internal exits at carried cost;
    /// OVERALL: real sales only).
    pub proceeds: Decimal,
    /// Total AUD cash income received (dividends/distributions net of foreign
    /// tax and TFN withholding; franking credits are not cash).
    pub income: Decimal,
    /// quantity_held × the supplied AUD price. `None` when open but unpriced
    /// (OVERALL: the sum, `None` unless every open holding is priced).
    pub market_value: Option<Decimal>,
    /// proceeds + income + market value − invested, AUD. `None` when the
    /// market value is unknown.
    pub total_return: Option<Decimal>,
    /// total_return / invested × 100. `None` when total_return is unknown or
    /// nothing was invested.
    pub total_return_pct: Option<Decimal>,
    /// Annualised money-weighted return (IRR over the dated flows + market
    /// value), percent p.a. `None` when the market value is unknown, all flows
    /// fall on one day, or the flows admit no rate (e.g. one-sided).
    pub money_weighted_return_pct: Option<Decimal>,
    /// Trailing 12 months' income / market value × 100. `None` when the
    /// market value is unknown or zero.
    pub income_yield_pct: Option<Decimal>,
    /// The price source's quote timestamp (the "as at" moment) when the market
    /// value came from a live fetch; absent on the OVERALL row, for an
    /// explicitly supplied price, or a stored snapshot.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub price_as_of: Option<String>,
    /// The AUD conversion of this row's price used an earlier month's FX rate
    /// because the valuation month's is not published yet
    /// (`infra::fx::resolve_valuation_rate`): the valuation is provisional.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub fx_provisional: bool,
    /// This row's price is not its valuation day's own close: the price
    /// provider has stopped quoting the security (`listings.unpriced_from`),
    /// so the last stored ok close was carried forward
    /// (`reports::valuation`, SCENARIOS Q-02). Unlike `fx_provisional` there
    /// is no later fact that clears it — the day is never going to be quoted
    /// — so a regeneration reproduces it.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub price_carried_forward: bool,
    /// Why a live price could not be obtained for an open holding: its
    /// market-dependent metrics are left unknown with the reason rather than
    /// silently zeroed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub price_unavailable: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PerformanceRequest {
    /// Current price per unit by listing id, expected in AUD so it lines up
    /// with the AUD-denominated flows. An explicit price overrides a
    /// live-fetched one.
    #[serde(
        default,
        deserialize_with = "crate::infra::decimal::strict_decimal_map"
    )]
    pub prices: HashMap<i64, Decimal>,
    /// Valuation date for the supplied prices; flows after it are ignored.
    /// Defaults to today.
    #[serde(default)]
    pub as_of_date: Option<NaiveDate>,
    /// Fetch the current price live from the price source for every open
    /// holding without an explicit price (off by default — see
    /// `portfolio::OverviewRequest::live`).
    #[serde(default)]
    pub live: bool,
}

pub fn router() -> Router<SqlitePool> {
    Router::new().route("/portfolio/performance", post(performance_handler))
}

/// One holding's accumulated AUD figures and dated cash flows
/// (outflows negative, inflows positive).
#[derive(Default)]
struct Acc {
    invested: Decimal,
    proceeds: Decimal,
    income: Decimal,
    trailing_income: Decimal,
    quantity: Decimal,
    flows: Vec<(NaiveDate, Decimal)>,
}

/// Which internal-movement group a provenance-marked trade belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum GroupKind {
    Transfer,
    Scrip,
    Demerger,
}

struct TradeFlow {
    /// The trade's own columns, on the shared cost-base mapping — so its
    /// acquisition date, FX precedence and initial cost base are the parcel's
    /// definitions, not this report's restatements of them.
    parcel: ParcelRow,
    is_sell: bool,
    group: Option<(GroupKind, i64)>,
}

impl sqlx::FromRow<'_, sqlx::sqlite::SqliteRow> for TradeFlow {
    fn from_row(row: &sqlx::sqlite::SqliteRow) -> Result<Self, sqlx::Error> {
        // An internal-movement Sell and its replacement Buys share a group:
        // the carried cost flows between them instead of external cash.
        let transfer_id: Option<i64> = row.try_get("transfer_id")?;
        let scrip_id: Option<i64> = row.try_get("scrip_action_id")?;
        let demerger_id: Option<i64> = row.try_get("demerger_action_id")?;
        let group = transfer_id
            .map(|id| (GroupKind::Transfer, id))
            .or(scrip_id.map(|id| (GroupKind::Scrip, id)))
            .or(demerger_id.map(|id| (GroupKind::Demerger, id)));
        let trade_type: TradeType = row.try_get("trade_type")?;
        Ok(TradeFlow {
            parcel: ParcelRow::from_row(row)?,
            is_sell: trade_type == TradeType::Sell,
            group,
        })
    }
}

/// Annualised money-weighted rate of return (as a fraction, e.g. 0.10 = 10%
/// p.a.) over dated flows: the rate at which the flows' net present value is
/// zero, with time in years (actual/365) from the first flow. Solved by
/// bisection on [−99.99%, +1,000%]; `None` when no such rate exists there
/// (one-sided flows, all flows on one day, or no sign change).
pub(crate) fn money_weighted_annual_return(flows: &[(NaiveDate, Decimal)]) -> Option<Decimal> {
    let start = flows.iter().map(|&(d, _)| d).min()?;
    let end = flows.iter().map(|&(d, _)| d).max()?;
    if start == end
        || !flows.iter().any(|&(_, a)| a < Decimal::ZERO)
        || !flows.iter().any(|&(_, a)| a > Decimal::ZERO)
    {
        return None;
    }
    let year = Decimal::from(365);
    let timed: Vec<(Decimal, Decimal)> = flows
        .iter()
        .map(|&(d, a)| (Decimal::from((d - start).num_days()) / year, a))
        .collect();
    // The NPV's sign, evaluated at the *last* flow date rather than the
    // first: Σ aᵢ·base^(t_max−tᵢ) = NPV·base^t_max with base^t_max > 0, so the
    // zeroes and signs — all the bisection uses — are the same. Discounting to
    // t₀ divides by base^tᵢ, which at the −99.99% bound is 1e-28 for a flow 7
    // years old — non-zero, but small enough that the division overflows
    // Decimal (and underflows to a bare `None` just past it). Compounding
    // forward multiplies by factors that stay ≤ 1 at negative rates; an early
    // flow that underflows to zero is genuinely negligible there.
    let t_max = timed
        .iter()
        .map(|&(t, _)| t)
        .fold(Decimal::ZERO, Decimal::max);
    // For a fractional exponent, `base.checked_powd(exp)` is exactly
    // `(exp * base.ln()).checked_exp()` internally — so calling it once per
    // flow recomputes `base.ln()` from scratch for every flow, even though
    // `base` (the bisection's current candidate rate) is the same for all of
    // them. `base` is always positive here (rate is bounded within
    // [−99.99%, +1,000%], so base ∈ (0.0001, 11)), matching `checked_powd`'s
    // own positive-base path exactly, so hoisting the `ln` out and reusing it
    // is the identical computation for that path. But an exponent that lands
    // on an exact integer (flows spaced a whole number of years apart) takes
    // a *different*, exact path inside `checked_powd` — repeated squaring,
    // not ln/exp — which is both more precise and avoids ln/exp underflow at
    // the wide extremes this bisection's bounds can reach (a 20-year gap at
    // the −99.99% bound needs `ln(0.0001)·20 ≈ −184`, right at `checked_exp`'s
    // underflow edge); that path is preserved by falling back to
    // `checked_powd` whenever the exponent is a whole number.
    let npv = |rate: Decimal| -> Option<Decimal> {
        let base = Decimal::ONE + rate;
        let ln_base = base.checked_ln()?;
        let mut total = Decimal::ZERO;
        for &(t, amount) in &timed {
            let exp = t_max - t;
            let factor = if exp.normalize().scale() == 0 {
                base.checked_powd(exp)?
            } else {
                (ln_base * exp).checked_exp()?
            };
            total = total.checked_add(amount.checked_mul(factor)?)?;
        }
        Some(total)
    };

    let mut lo = Decimal::new(-9999, 4); // −99.99% p.a.
    let mut hi = Decimal::from(10); // +1,000% p.a.
    let npv_lo = npv(lo)?;
    let npv_hi = npv(hi)?;
    if npv_lo.is_zero() {
        return Some(lo);
    }
    if npv_hi.is_zero() {
        return Some(hi);
    }
    if (npv_lo > Decimal::ZERO) == (npv_hi > Decimal::ZERO) {
        return None;
    }
    let two = Decimal::from(2);
    // 40 halvings of the ~11-wide [−99.99%, +1,000%] bracket reach ~1e-8
    // precision — the result is rounded to 4 dp, so this already has margin
    // to spare over the 64 the bisection used to run (each extra iteration
    // costs one more `ln`/`exp` evaluation per flow, so trimming this
    // matters as much as the hoist above for a flow-heavy holding).
    for _ in 0..40 {
        let mid = (lo + hi) / two;
        let v = npv(mid)?;
        if v.is_zero() {
            return Some(mid);
        }
        if (v > Decimal::ZERO) == (npv_lo > Decimal::ZERO) {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    Some((lo + hi) / two)
}

/// Identifies which holding a performance row is for. The portfolio-wide
/// `OVERALL` row carries no listing or account.
struct RowKey {
    listing_id: Option<i64>,
    ticker: String,
    holding_account_id: Option<i64>,
}

/// Assemble one report row from accumulated figures. `open` = units are still
/// held, so the market value is required for the value-dependent metrics.
fn build_row(
    key: RowKey,
    quantity_held: Option<Decimal>,
    market_value: Option<Decimal>,
    open: bool,
    acc: &Acc,
    as_of: NaiveDate,
) -> HoldingPerformance {
    let hundred = Decimal::from(100);
    let valued = !open || market_value.is_some();
    let total_return = valued
        .then(|| acc.proceeds + acc.income + market_value.unwrap_or(Decimal::ZERO) - acc.invested);
    let total_return_pct = total_return.and_then(|tr| {
        (acc.invested > Decimal::ZERO).then(|| (tr / acc.invested * hundred).round_dp(4))
    });
    let money_weighted_return_pct = if valued {
        let mut flows = acc.flows.clone();
        if let Some(mv) = market_value {
            flows.push((as_of, mv));
        }
        money_weighted_annual_return(&flows).map(|r| (r * hundred).round_dp(4))
    } else {
        None
    };
    let income_yield_pct = market_value.and_then(|mv| {
        (mv > Decimal::ZERO).then(|| (acc.trailing_income / mv * hundred).round_dp(4))
    });
    HoldingPerformance {
        listing_id: key.listing_id,
        ticker: key.ticker,
        holding_account_id: key.holding_account_id,
        quantity_held,
        invested: acc.invested,
        proceeds: acc.proceeds,
        income: acc.income,
        market_value,
        total_return,
        total_return_pct,
        money_weighted_return_pct,
        income_yield_pct,
        price_as_of: None,
        fx_provisional: false,
        price_carried_forward: false,
        price_unavailable: None,
    }
}

/// The dated cash-flow accumulation shared by `db_performance` (which turns
/// it into report rows valued at `as_of`) and `overall_flows` (which
/// `period_performance` reuses to build a window-scoped money-weighted
/// return) — the trade/income processing (internal-movement exclusion,
/// AUD conversion, carried cost) needs to happen exactly once either way.
struct Accumulated {
    holdings: BTreeMap<(i64, i64), Acc>,
    overall: Acc,
    tickers: HashMap<i64, String>,
}

async fn accumulate(
    pool: &SqlitePool,
    as_of: NaiveDate,
) -> Result<Option<Accumulated>, sqlx::Error> {
    // One read transaction: every input below comes from the same snapshot.
    let mut tx = pool.begin().await?;
    let trades: Vec<TradeFlow> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT {}, trade_type, transfer_id, scrip_action_id, demerger_action_id \
         FROM trades WHERE date <= ?",
        ParcelRow::COLUMNS
    )))
    .bind(as_of)
    .fetch_all(&mut *tx)
    .await?;

    let income_rows: Vec<Income> = sqlx::query_as("SELECT * FROM income WHERE date_paid <= ?")
        .bind(as_of)
        .fetch_all(&mut *tx)
        .await?;

    if trades.is_empty() && income_rows.is_empty() {
        // Nothing to report; the dropped transaction was read-only.
        return Ok(None);
    }

    // Units sold out of each purchase parcel by as_of (with sale dates, so the
    // allocated quantity is re-based across splits like the other reports).
    // This report can't use `domain::open_parcels::load` itself — it walks
    // every trade including the Sells, and values acquisitions at their
    // initial cost rather than the adjusted cost base — but the allocations
    // read is the same one, so it comes from there.
    let qty_sold = open_parcels::db_units_sold(&mut tx, Some(as_of)).await?;

    let split_events = crate::entities::corporate_action::db_share_split_events(&mut *tx).await?;
    let ticker_rows = sqlx::query("SELECT id, ticker FROM listings")
        .fetch_all(&mut *tx)
        .await?;
    let mut tickers: HashMap<i64, String> = HashMap::new();
    for row in &ticker_rows {
        tickers.insert(row.try_get("id")?, row.try_get("ticker")?);
    }
    // every imported ATO FX rate — per-row conversions below are map lookups,
    // not one DB round-trip each
    let fx = FxRates::load(&mut *tx).await?;
    tx.commit().await?;

    let mut holdings: BTreeMap<(i64, i64), Acc> = BTreeMap::new();
    let mut overall = Acc::default();
    // Carried AUD cost per internal-movement group: what the replacement
    // parcels took with them, which is what the closing Sell is worth.
    let mut group_costs: HashMap<(GroupKind, i64), Decimal> = HashMap::new();

    // Pass 1 — acquisitions (also totals each internal group's carried cost,
    // which pass 2 needs for the closing Sells).
    for t in trades.iter().filter(|t| !t.is_sell) {
        let p = &t.parcel;
        // Convert at the acquisition month — the deemed acquisition month for a
        // rollover/transfer-created parcel, preserving the original AUD cost.
        let cost = fx.to_aud(
            p.parcel().initial_cost(),
            &p.currency,
            p.acquired(),
            p.fx_override(),
        )?;
        let acc = holdings
            .entry((p.listing_id, p.holding_account_id))
            .or_default();
        acc.invested += cost;
        acc.flows.push((p.date, -cost));
        if let Some(key) = t.group {
            *group_costs.entry(key).or_insert(Decimal::ZERO) += cost;
        } else {
            overall.invested += cost;
            overall.flows.push((p.date, -cost));
        }

        // Units of this parcel still open at as_of (in as-of units).
        let splits = split_events.get(&p.listing_id).map_or(&[][..], |v| v);
        let sold = crate::entities::corporate_action::sold_in_acquired_units(
            qty_sold.get(&p.id).map_or(&[][..], |v| v),
            splits,
            p.date,
        );
        let remaining = p.quantity - sold;
        if remaining > Decimal::ZERO {
            acc.quantity += crate::entities::corporate_action::split_adjusted_quantity(
                remaining,
                splits,
                p.date,
                Some(as_of),
            );
        }
    }

    // Pass 2 — disposals. An internal closing Sell is worth the carried cost
    // its replacement parcels took (no external cash moved) — plus, for a
    // partial-rollover scrip exchange, its cash component (price × quantity:
    // real external cash the holder received, so it also reaches OVERALL);
    // a real Sell is worth its net proceeds.
    for t in trades.iter().filter(|t| t.is_sell) {
        let p = &t.parcel;
        let acc_key = (p.listing_id, p.holding_account_id);
        if let Some(key) = t.group {
            let carried = group_costs.get(&key).copied().unwrap_or(Decimal::ZERO);
            let mut inflow = carried;
            if !p.average_price.is_zero() {
                let cash = fx.to_aud(
                    p.average_price * p.quantity,
                    &p.currency,
                    p.date,
                    p.fx_override(),
                )?;
                inflow += cash;
                overall.proceeds += cash;
                overall.flows.push((p.date, cash));
            }
            let acc = holdings.entry(acc_key).or_default();
            acc.proceeds += inflow;
            acc.flows.push((p.date, inflow));
        } else {
            let net = p.average_price * p.quantity - p.brokerage - p.gst_on_brokerage;
            let net = fx.to_aud(net, &p.currency, p.date, p.fx_override())?;
            let acc = holdings.entry(acc_key).or_default();
            acc.proceeds += net;
            acc.flows.push((p.date, net));
            overall.proceeds += net;
            overall.flows.push((p.date, net));
        }
    }

    // Cash income, on the model's own `net_cash_received` definition — the
    // same figure the DRP reinvests: franking credits are a tax offset, not
    // cash received. Timed by `date_paid` (when the cash landed) rather than
    // the tax assessment date: this is a return measure, not a tax figure.
    let trailing_start = as_of
        .checked_sub_months(Months::new(12))
        .unwrap_or(NaiveDate::MIN);
    for income in &income_rows {
        // Remuneration recorded against the holding (a dividend equivalent on
        // unvested RSUs — SCENARIOS J-10) is not a return *on* the holding: it
        // is paid for services, not by the shares, and counting it would
        // inflate the income yield of whatever listing it was recorded against.
        // An `OtherIncome` row is the opposite case and is deliberately *not*
        // skipped: a staking reward or an established-token airdrop is a
        // return the holding itself produced (SCENARIOS L-03), so it belongs
        // in the yield exactly as a distribution does.
        if income.income_type == IncomeType::EmploymentIncome {
            continue;
        }
        let date_paid = income.date_paid;
        // Income rows carry no manual fx override: a non-AUD amount with no
        // ATO rate fails loudly (same as the tax summary).
        let cash = fx.to_aud(
            income.net_cash_received(),
            &income.currency,
            date_paid,
            FxOverride::None,
        )?;
        let acc = holdings
            .entry((income.listing_id, income.holding_account_id))
            .or_default();
        acc.income += cash;
        acc.flows.push((date_paid, cash));
        overall.income += cash;
        overall.flows.push((date_paid, cash));
        if date_paid > trailing_start {
            acc.trailing_income += cash;
            overall.trailing_income += cash;
        }
    }

    Ok(Some(Accumulated {
        holdings,
        overall,
        tickers,
    }))
}

/// Every external cash-flow event (Buy/DRP AUD cost, Sell AUD net proceeds,
/// cash income — dated, signed, internal-movement legs excluded) since
/// inception up to `as_of`, portfolio-wide — the same flows `db_performance`
/// feeds to `money_weighted_annual_return` for its OVERALL row, exposed for
/// `period_performance` to filter to a window and combine with the window's
/// opening/closing market value as boundary flows.
pub(crate) async fn overall_flows(
    pool: &SqlitePool,
    as_of: NaiveDate,
) -> Result<Vec<(NaiveDate, Decimal)>, sqlx::Error> {
    Ok(accumulate(pool, as_of)
        .await?
        .map_or_else(Vec::new, |a| a.overall.flows))
}

pub async fn db_performance(
    pool: &SqlitePool,
    prices: &HashMap<i64, Decimal>,
    as_of: NaiveDate,
) -> Result<Vec<HoldingPerformance>, sqlx::Error> {
    let Some(Accumulated {
        holdings,
        overall,
        tickers,
    }) = accumulate(pool, as_of).await?
    else {
        return Ok(vec![]);
    };

    // Assemble rows: one per holding, then the OVERALL row (external flows
    // only — the internal legs cancel — with the market value summed across
    // holdings, known only when every open holding is priced).
    let mut result = Vec::with_capacity(holdings.len() + 1);
    let mut overall_mv = Some(Decimal::ZERO);
    let mut any_open = false;
    for (&(listing_id, account_id), acc) in &holdings {
        let open = acc.quantity > Decimal::ZERO;
        let market_value = if open {
            prices.get(&listing_id).map(|&p| acc.quantity * p)
        } else {
            None
        };
        if open {
            any_open = true;
            overall_mv = match (overall_mv, market_value) {
                (Some(total), Some(mv)) => Some(total + mv),
                _ => None,
            };
        }
        let ticker = tickers.get(&listing_id).cloned().unwrap_or_default();
        result.push(build_row(
            RowKey {
                listing_id: Some(listing_id),
                ticker,
                holding_account_id: Some(account_id),
            },
            Some(acc.quantity),
            market_value,
            open,
            acc,
            as_of,
        ));
    }
    let overall_mv = if any_open { overall_mv } else { None };
    result.push(build_row(
        RowKey {
            listing_id: None,
            ticker: "OVERALL".to_string(),
            holding_account_id: None,
        },
        None,
        overall_mv,
        any_open,
        &overall,
        as_of,
    ));
    Ok(result)
}

async fn performance_handler(
    State(pool): State<SqlitePool>,
    fetcher: Option<Extension<SharedFetcher>>,
    body: Option<Json<PerformanceRequest>>,
) -> Result<Json<Vec<HoldingPerformance>>, ApiError> {
    let req = body.map(|Json(req)| req).unwrap_or_default();
    let as_of = req
        .as_of_date
        .unwrap_or_else(|| chrono::Local::now().date_naive());

    // Live-fetch a current price for each open held listing without an explicit
    // override (when requested); an explicit price always wins.
    let held = closing_price::db_held_listing_ids(&pool, Some(as_of))
        .await
        .map_err(ApiError::from)?;
    let live = closing_price::resolve_live_prices(
        &pool,
        fetcher.as_ref().map(|f| f.0.as_ref()),
        req.live,
        &req.prices,
        held,
    )
    .await
    .map_err(ApiError::from)?;

    // The effective AUD price per listing feeding the valuation: explicit
    // overrides plus the successful live quotes.
    let mut prices = req.prices.clone();
    for (id, result) in &live {
        if let Ok(v) = result {
            prices.insert(*id, v.aud_price);
        }
    }

    let mut rows = db_performance(&pool, &prices, as_of)
        .await
        .map_err(ApiError::from)?;

    // Carry each live price's as-of time, and surface a live-fetch failure on
    // an open holding (left unvalued by db_performance) as its reason.
    for row in &mut rows {
        let Some(listing_id) = row.listing_id else {
            continue;
        };
        match live.get(&listing_id) {
            Some(Ok(v)) => {
                row.price_as_of = Some(v.as_of.clone());
                row.fx_provisional = v.fx_provisional;
            }
            Some(Err(reason))
                if row.market_value.is_none()
                    && row.quantity_held.is_some_and(|q| q > Decimal::ZERO) =>
            {
                row.price_unavailable = Some(reason.clone());
            }
            _ => {}
        }
    }

    Ok(Json(rows))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::entities::{listing, trade};
    use crate::test_support::{self, ApiClient, test_pool};
    use axum::http::StatusCode;

    fn d(y: i32, m: u32, day: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, day).unwrap()
    }

    async fn insert_listing(pool: &SqlitePool, id: i64, ticker: &str, mic: &str, currency: &str) {
        test_support::listing(id)
            .ticker(ticker)
            .name(ticker)
            .mic(mic)
            .security_type(listing::SecurityType::Share)
            .currency(currency)
            .insert(pool)
            .await;
    }

    /// A trade with zero brokerage/GST so the flow figures stay round.
    #[allow(clippy::too_many_arguments)]
    async fn insert_trade(
        pool: &SqlitePool,
        id: i64,
        trade_type: trade::TradeType,
        listing_id: i64,
        account_id: i64,
        date: NaiveDate,
        qty: Decimal,
        price: Decimal,
        currency: &str,
        fx_rate: Decimal,
    ) {
        test_support::trade(id, listing_id, trade_type)
            .account(account_id)
            .date(date)
            .qty(qty)
            .price(price)
            .currency(currency)
            .fx_rate(fx_rate)
            .insert(pool)
            .await;
    }

    async fn buy(pool: &SqlitePool, id: i64, listing: i64, date: NaiveDate, qty: i64, price: i64) {
        insert_trade(
            pool,
            id,
            trade::TradeType::Buy,
            listing,
            1,
            date,
            Decimal::from(qty),
            Decimal::from(price),
            "AUD",
            Decimal::ONE,
        )
        .await;
    }

    async fn sell(pool: &SqlitePool, id: i64, listing: i64, date: NaiveDate, qty: i64, price: i64) {
        insert_trade(
            pool,
            id,
            trade::TradeType::Sell,
            listing,
            1,
            date,
            Decimal::from(qty),
            Decimal::from(price),
            "AUD",
            Decimal::ONE,
        )
        .await;
    }

    async fn allocate(pool: &SqlitePool, id: i64, sale_id: i64, buy_id: i64, qty: i64) {
        test_support::allocate(pool, id, sale_id, buy_id, Decimal::from(qty)).await;
    }

    /// An unfranked cash distribution (everything else zero).
    async fn insert_income(
        pool: &SqlitePool,
        id: i64,
        listing_id: i64,
        date_paid: NaiveDate,
        amount: i64,
    ) {
        test_support::income(id, listing_id, date_paid)
            .with(|i| i.unfranked_amount = Decimal::from(amount))
            .insert(pool)
            .await;
    }

    fn price_map(listing_id: i64, price: &str) -> HashMap<i64, Decimal> {
        HashMap::from([(listing_id, price.parse().unwrap())])
    }

    #[tokio::test]
    async fn db_empty_returns_empty() {
        let pool = test_pool().await;
        let rows = db_performance(&pool, &HashMap::new(), d(2024, 12, 31))
            .await
            .unwrap();
        assert!(rows.is_empty());
    }

    /// One open holding with income and a price: invested, market value,
    /// total return (absolute + %), and the trailing-12-month income yield.
    #[tokio::test]
    async fn db_open_holding_reports_value_return_and_yield() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "VAS", "XASX", "AUD").await;
        buy(&pool, 1, 1, d(2024, 1, 2), 100, 10).await; // invested 1,000
        insert_income(&pool, 1, 1, d(2024, 6, 30), 50).await;

        let rows = db_performance(&pool, &price_map(1, "12"), d(2024, 12, 31))
            .await
            .unwrap();
        assert_eq!(rows.len(), 2);
        let h = &rows[0];
        assert_eq!((h.listing_id, h.holding_account_id), (Some(1), Some(1)));
        assert_eq!(h.ticker, "VAS");
        assert_eq!(h.quantity_held, Some(Decimal::from(100)));
        assert_eq!(h.invested, Decimal::from(1000));
        assert_eq!(h.proceeds, Decimal::ZERO);
        assert_eq!(h.income, Decimal::from(50));
        assert_eq!(h.market_value, Some(Decimal::from(1200)));
        // 1,200 + 50 − 1,000 = 250 → 25% of invested.
        assert_eq!(h.total_return, Some(Decimal::from(250)));
        assert_eq!(h.total_return_pct, Some(Decimal::from(25)));
        // Trailing year's income 50 / market value 1,200 = 4.1667%.
        assert_eq!(h.income_yield_pct, Some("4.1667".parse().unwrap()));

        let o = &rows[1];
        assert_eq!(o.ticker, "OVERALL");
        assert_eq!(
            (o.listing_id, o.holding_account_id, o.quantity_held),
            (None, None, None)
        );
        assert_eq!(o.invested, Decimal::from(1000));
        assert_eq!(o.market_value, Some(Decimal::from(1200)));
        assert_eq!(o.total_return, Some(Decimal::from(250)));
    }

    /// A 1,000 → 1,100 value over exactly 365 days is a 10% p.a.
    /// money-weighted return.
    #[tokio::test]
    async fn db_money_weighted_return_of_a_one_year_gain_is_exact() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "VAS", "XASX", "AUD").await;
        buy(&pool, 1, 1, d(2023, 1, 3), 100, 10).await;

        let rows = db_performance(&pool, &price_map(1, "11"), d(2024, 1, 3))
            .await
            .unwrap();
        assert_eq!(rows[0].money_weighted_return_pct, Some(Decimal::from(10)));
        assert_eq!(rows[1].money_weighted_return_pct, Some(Decimal::from(10)));
    }

    /// Regression: a flow exactly 7 years (actual/365) before the valuation
    /// flow used to panic with "Division overflowed" — at the bisection's
    /// −99.99% bound the discount factor 0.0001⁷ = 1e-28 is non-zero, and
    /// dividing any amount above ~$7.92 by it exceeds `Decimal::MAX`.
    #[test]
    fn money_weighted_return_survives_a_seven_year_span() {
        let start = d(2019, 6, 26);
        let end = start + chrono::Duration::days(7 * 365);
        let r = money_weighted_annual_return(&[
            (start, Decimal::from(-10_000)),
            (end, Decimal::from(5_000)),
        ])
        .expect("a rate exists: (1+r)^7 = 0.5");
        // (1+r)^7 = 0.5 → r = 0.5^(1/7) − 1 ≈ −9.4276% p.a.
        assert_eq!(r.round_dp(6), "-0.094276".parse().unwrap());
    }

    /// Flows spanning two decades still get a rate (the old discount-to-t₀
    /// form underflowed to `None` beyond ~7 years even when it didn't panic).
    #[test]
    fn money_weighted_return_survives_a_twenty_year_span() {
        let start = d(2006, 7, 1);
        let end = start + chrono::Duration::days(20 * 365);
        let r = money_weighted_annual_return(&[
            (start, Decimal::from(-10_000)),
            (end, Decimal::from(5_000)),
        ])
        .expect("a rate exists: (1+r)^20 = 0.5");
        // (1+r)^20 = 0.5 → r = 0.5^(1/20) − 1 ≈ −3.4064% p.a.
        assert_eq!(r.round_dp(6), "-0.034064".parse().unwrap());
    }

    /// Regression for the `ln`-hoisting optimisation in `npv`'s inner loop:
    /// two flows a non-whole number of years apart take the `checked_exp`
    /// path (not the exact-integer-exponent `checked_powd` fallback the
    /// 7-year/20-year tests above exercise), so this is what most real
    /// portfolios' flows actually hit — hand-computed against a plain
    /// annualised rate, not just re-deriving whatever the code returns.
    #[test]
    fn money_weighted_return_of_a_fractional_year_gap_matches_hand_calc() {
        let start = d(2023, 1, 3);
        let end = start + chrono::Duration::days(200);
        let r = money_weighted_annual_return(&[
            (start, Decimal::from(-10_000)),
            (end, Decimal::from(11_000)),
        ])
        .expect("a rate exists: (1+r)^(200/365) = 1.1");
        // (1+r)^(200/365) = 1.1 → r = 1.1^(365/200) − 1 ≈ 18.9985% p.a.
        assert_eq!(r.round_dp(6), "0.189985".parse().unwrap());
    }

    /// A single call mixing a whole-number-of-years gap with a fractional
    /// one exercises both branches of the exponent dispatch together — the
    /// exact-integer `checked_powd` fallback for one flow and the hoisted
    /// `ln`/`checked_exp` path for the other, both discounted against the
    /// same bisected rate.
    #[test]
    fn money_weighted_return_mixes_integer_and_fractional_exponents() {
        let start = d(2020, 1, 1);
        let mid = start + chrono::Duration::days(365); // exactly 1 year: integer exponent
        let end = start + chrono::Duration::days(365 + 200); // +200 days: fractional exponent
        let r = money_weighted_annual_return(&[
            (start, Decimal::from(-10_000)),
            (mid, Decimal::from(-1_000)),
            (end, Decimal::from(12_500)),
        ]);
        assert!(r.is_some());
    }

    /// A fully sold holding needs no price: its return is realised, the
    /// market-value-dependent yield is null.
    #[tokio::test]
    async fn db_closed_holding_reports_realised_performance_without_prices() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "VAS", "XASX", "AUD").await;
        buy(&pool, 1, 1, d(2023, 1, 3), 100, 10).await;
        sell(&pool, 2, 1, d(2024, 1, 3), 100, 11).await;
        allocate(&pool, 1, 2, 1, 100).await;

        let rows = db_performance(&pool, &HashMap::new(), d(2024, 6, 30))
            .await
            .unwrap();
        let h = &rows[0];
        assert_eq!(h.quantity_held, Some(Decimal::ZERO));
        assert_eq!(h.market_value, None);
        assert_eq!(h.proceeds, Decimal::from(1100));
        assert_eq!(h.total_return, Some(Decimal::from(100)));
        assert_eq!(h.total_return_pct, Some(Decimal::from(10)));
        // Bought and sold 365 days apart → 10% p.a.
        assert_eq!(h.money_weighted_return_pct, Some(Decimal::from(10)));
        assert_eq!(h.income_yield_pct, None);
    }

    /// An open holding without a supplied price reports null for every
    /// market-dependent metric instead of a silently wrong figure — and so
    /// does the OVERALL row.
    #[tokio::test]
    async fn db_open_holding_without_price_has_unknown_market_metrics() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "VAS", "XASX", "AUD").await;
        buy(&pool, 1, 1, d(2024, 1, 2), 100, 10).await;

        let rows = db_performance(&pool, &HashMap::new(), d(2024, 12, 31))
            .await
            .unwrap();
        for row in &rows {
            assert_eq!(row.invested, Decimal::from(1000));
            assert_eq!(row.market_value, None);
            assert_eq!(row.total_return, None);
            assert_eq!(row.total_return_pct, None);
            assert_eq!(row.money_weighted_return_pct, None);
            assert_eq!(row.income_yield_pct, None);
        }
    }

    /// The yield window is the trailing 12 months; older income still counts
    /// in the lifetime `income` figure.
    #[tokio::test]
    async fn db_trailing_yield_counts_only_the_last_years_income() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "VAS", "XASX", "AUD").await;
        buy(&pool, 1, 1, d(2022, 1, 4), 100, 10).await;
        insert_income(&pool, 1, 1, d(2022, 6, 30), 40).await; // outside the window
        insert_income(&pool, 2, 1, d(2024, 6, 30), 60).await; // inside

        let rows = db_performance(&pool, &price_map(1, "12"), d(2024, 12, 31))
            .await
            .unwrap();
        let h = &rows[0];
        assert_eq!(h.income, Decimal::from(100));
        // 60 / 1,200 = 5%.
        assert_eq!(h.income_yield_pct, Some(Decimal::from(5)));
    }

    /// A holding-account transfer is an internal movement: the source holding
    /// exits at the carried cost (no gain), the destination carries it as its
    /// own invested base, and the OVERALL row sees neither leg — the
    /// portfolio's return is unchanged by moving parcels around.
    #[tokio::test]
    async fn db_transfer_is_internal_to_holdings_and_invisible_overall() {
        use crate::entities::{holding_account, sell as sell_mod, transfer};
        let pool = test_pool().await;
        insert_listing(&pool, 1, "ICE", "XASX", "AUD").await;
        holding_account::db_upsert(
            &pool,
            &holding_account::HoldingAccount {
                id: 2,
                name: "Broker".to_string(),
            },
        )
        .await
        .unwrap();
        buy(&pool, 1, 1, d(2023, 1, 3), 100, 10).await; // 1,000 into account 1
        transfer::db_transfer(
            &pool,
            1,
            &transfer::TransferBody {
                listing_id: 1,
                date: d(2023, 7, 1),
                from_account_id: 1,
                to_account_id: 2,
                allocations: vec![sell_mod::AllocationInput {
                    purchase_trade_id: 1,
                    quantity_allocated: Decimal::from(100),
                }],
                fee_allocations: Vec::new(),
                fee_market_price: None,
                fee_fx_rate: None,
            },
        )
        .await
        .unwrap();

        let rows = db_performance(&pool, &price_map(1, "15"), d(2024, 1, 2))
            .await
            .unwrap();
        assert_eq!(rows.len(), 3);

        // Source holding: closed at the carried cost — zero return.
        let src = &rows[0];
        assert_eq!((src.listing_id, src.holding_account_id), (Some(1), Some(1)));
        assert_eq!(src.quantity_held, Some(Decimal::ZERO));
        assert_eq!(src.invested, Decimal::from(1000));
        assert_eq!(src.proceeds, Decimal::from(1000));
        assert_eq!(src.total_return, Some(Decimal::ZERO));

        // Destination holding: carries the cost base and shows the gain.
        let dst = &rows[1];
        assert_eq!((dst.listing_id, dst.holding_account_id), (Some(1), Some(2)));
        assert_eq!(dst.quantity_held, Some(Decimal::from(100)));
        assert_eq!(dst.invested, Decimal::from(1000));
        assert_eq!(dst.market_value, Some(Decimal::from(1500)));
        assert_eq!(dst.total_return, Some(Decimal::from(500)));
        assert_eq!(dst.total_return_pct, Some(Decimal::from(50)));

        // OVERALL: only the external purchase and the terminal value.
        let o = &rows[2];
        assert_eq!(o.invested, Decimal::from(1000));
        assert_eq!(o.proceeds, Decimal::ZERO);
        assert_eq!(o.market_value, Some(Decimal::from(1500)));
        assert_eq!(o.total_return, Some(Decimal::from(500)));
        assert_eq!(o.total_return_pct, Some(Decimal::from(50)));
    }

    /// A partial-rollover scrip exchange's cash component is real external
    /// cash: the source holding's closing Sell is worth the carried cost
    /// plus the cash, and the cash (only) reaches the OVERALL row — the
    /// rolled-over cost stays internal.
    #[tokio::test]
    async fn db_scrip_exchange_cash_component_counts_as_external_cash() {
        use crate::entities::{corporate_action, scrip_exchange};
        let pool = test_pool().await;
        insert_listing(&pool, 1, "OLD", "XASX", "AUD").await;
        insert_listing(&pool, 2, "NEW", "XASX", "AUD").await;
        buy(&pool, 1, 1, d(2023, 1, 3), 100, 10).await; // invested 1,000
        // 1-for-1 with $2 cash per old unit; new shares worth $18 → the cash
        // side takes 2/20 = $100 of the cost base, $900 rolls over; $200
        // cash is received.
        corporate_action::db_upsert(
            &pool,
            &corporate_action::CorporateAction {
                id: 10,
                listing_id: 1,
                date: d(2023, 7, 1),
                kind: corporate_action::ActionKind::ScripForScrip {
                    scrip_listing_id: 2,
                    scrip_new_units: Decimal::ONE,
                    scrip_old_units: Decimal::ONE,
                    scrip_cash_per_unit: Some(Decimal::from(2)),
                    scrip_market_value: Some(Decimal::from(18)),
                    scrip_cash_currency: Some("AUD".to_string()),
                },
            },
        )
        .await
        .unwrap();
        scrip_exchange::db_exchange(&pool, 10).await.unwrap();

        let rows = db_performance(&pool, &price_map(2, "12"), d(2024, 1, 2))
            .await
            .unwrap();
        assert_eq!(rows.len(), 3);

        // Source holding: exits at the carried $900 plus the $200 cash.
        let src = &rows[0];
        assert_eq!((src.listing_id, src.holding_account_id), (Some(1), Some(1)));
        assert_eq!(src.invested, Decimal::from(1000));
        assert_eq!(src.proceeds, Decimal::from(1100));

        // Replacement holding: invested at the carried cost only.
        let dst = &rows[1];
        assert_eq!((dst.listing_id, dst.holding_account_id), (Some(2), Some(1)));
        assert_eq!(dst.invested, Decimal::from(900));
        assert_eq!(dst.market_value, Some(Decimal::from(1200)));

        // OVERALL: only external cash — $1,000 in, the $200 cash out; the
        // rolled-over legs net to nothing. 200 + 1,200 − 1,000 = 400.
        let o = rows.last().unwrap();
        assert_eq!(o.ticker, "OVERALL");
        assert_eq!(o.invested, Decimal::from(1000));
        assert_eq!(o.proceeds, Decimal::from(200));
        assert_eq!(o.market_value, Some(Decimal::from(1200)));
        assert_eq!(o.total_return, Some(Decimal::from(400)));
    }

    /// Non-AUD flows convert to AUD (here via the trade's manual fx fallback,
    /// foreign-per-AUD); supplied prices are AUD already.
    #[tokio::test]
    async fn db_non_aud_invested_converts_to_aud() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "ICE", "XNYS", "USD").await;
        // 100 × US$10 = US$1,000 at 2 USD/AUD → A$500 invested.
        insert_trade(
            &pool,
            1,
            trade::TradeType::Buy,
            1,
            1,
            d(2024, 1, 2),
            Decimal::from(100),
            Decimal::from(10),
            "USD",
            Decimal::from(2),
        )
        .await;

        let rows = db_performance(&pool, &price_map(1, "6"), d(2024, 12, 31))
            .await
            .unwrap();
        let h = &rows[0];
        assert_eq!(h.invested, Decimal::from(500));
        assert_eq!(h.market_value, Some(Decimal::from(600)));
        assert_eq!(h.total_return, Some(Decimal::from(100)));
        assert_eq!(h.total_return_pct, Some(Decimal::from(20)));
    }

    /// Trades and income after the valuation date are ignored — the report is
    /// the position as at `as_of_date`.
    #[tokio::test]
    async fn db_flows_after_as_of_are_excluded() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "VAS", "XASX", "AUD").await;
        buy(&pool, 1, 1, d(2024, 1, 2), 100, 10).await;
        sell(&pool, 2, 1, d(2024, 6, 3), 100, 15).await;
        allocate(&pool, 1, 2, 1, 100).await;
        insert_income(&pool, 1, 1, d(2024, 5, 1), 50).await;

        // As at 31 Mar the sale and the May distribution haven't happened.
        let rows = db_performance(&pool, &price_map(1, "12"), d(2024, 3, 31))
            .await
            .unwrap();
        let h = &rows[0];
        assert_eq!(h.quantity_held, Some(Decimal::from(100)));
        assert_eq!(h.proceeds, Decimal::ZERO);
        assert_eq!(h.income, Decimal::ZERO);
        assert_eq!(h.market_value, Some(Decimal::from(1200)));
        assert_eq!(h.total_return, Some(Decimal::from(200)));
    }

    // API-level tests

    #[tokio::test]
    async fn api_performance_with_prices_and_as_of_date() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "VAS", "XASX", "AUD").await;
        buy(&pool, 1, 1, d(2024, 1, 2), 100, 10).await;

        let body = serde_json::json!({
            "prices": { "1": "12" },
            "as_of_date": "2024-12-31",
        });
        let resp = ApiClient::over(router().with_state(pool))
            .post("/portfolio/performance", &body)
            .await;
        assert_eq!(resp.status, StatusCode::OK);
        let rows: Vec<HoldingPerformance> = resp.json();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].ticker, "VAS");
        assert_eq!(rows[0].market_value, Some(Decimal::from(1200)));
        assert_eq!(rows.last().unwrap().ticker, "OVERALL");
    }

    /// With `live`, an open holding is valued from the price source's latest
    /// quote: the market value and the as-of time come through, and the OVERALL
    /// row aggregates the live value. An explicit override still wins, and a
    /// per-listing failure leaves that holding's market metrics unknown with a
    /// reason (others still valued).
    #[tokio::test]
    async fn api_performance_live_fetch_with_as_of_override_and_failure() {
        use crate::entities::closing_price::test_support::QuoteStub;
        let pool = test_pool().await;
        insert_listing(&pool, 1, "VAS", "XASX", "AUD").await; // live-valued
        insert_listing(&pool, 2, "VAF", "XASX", "AUD").await; // explicit override
        insert_listing(&pool, 3, "VGS", "XASX", "AUD").await; // live fetch fails
        buy(&pool, 1, 1, d(2024, 1, 2), 100, 10).await; // invested 1,000
        buy(&pool, 2, 2, d(2024, 1, 2), 50, 12).await;
        buy(&pool, 3, 3, d(2024, 1, 2), 10, 5).await;
        let as_of =
            chrono::TimeZone::with_ymd_and_hms(&chrono::Utc, 2024, 12, 31, 6, 30, 0).unwrap();
        let fetcher = QuoteStub::default()
            .with_quote(1, "12", "AUD", as_of)
            .shared();

        let body = serde_json::json!({
            "live": true,
            "as_of_date": "2024-12-31",
            "prices": { "2": "20" },
        });
        let resp = ApiClient::over(router().with_state(pool).layer(axum::Extension(fetcher)))
            .post("/portfolio/performance", &body)
            .await;
        assert_eq!(resp.status, StatusCode::OK);
        let rows: Vec<HoldingPerformance> = resp.json();
        let by_id = |id: i64| rows.iter().find(|r| r.listing_id == Some(id)).unwrap();
        // Listing 1: live-valued at 12 → market value 1,200, as-of carried.
        assert_eq!(by_id(1).market_value, Some(Decimal::from(1200)));
        assert_eq!(
            by_id(1).price_as_of.as_deref(),
            Some(as_of.to_rfc3339().as_str())
        );
        // Listing 2: explicit override 20 → 50 × 20 = 1,000, no as-of.
        assert_eq!(by_id(2).market_value, Some(Decimal::from(1000)));
        assert!(by_id(2).price_as_of.is_none());
        // Listing 3: live fetch failed → market metrics unknown, with a reason.
        assert!(by_id(3).market_value.is_none());
        assert!(by_id(3).total_return.is_none());
        assert!(by_id(3).price_unavailable.is_some());
        // OVERALL is unknown while an open holding is unpriced.
        let overall = rows.iter().find(|r| r.ticker == "OVERALL").unwrap();
        assert!(overall.market_value.is_none());
    }

    #[tokio::test]
    async fn api_performance_without_body_defaults_to_today() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "VAS", "XASX", "AUD").await;
        buy(&pool, 1, 1, d(2024, 1, 2), 100, 10).await;

        let resp = ApiClient::over(router().with_state(pool))
            .post_empty("/portfolio/performance")
            .await;
        assert_eq!(resp.status, StatusCode::OK);
        let rows: Vec<HoldingPerformance> = resp.json();
        // Unpriced open holding: figures present, market metrics null.
        assert_eq!(rows[0].invested, Decimal::from(1000));
        assert_eq!(rows[0].market_value, None);
    }
}
