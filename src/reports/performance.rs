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
//! unaffected by moving parcels around. A rights exercise and a buy-back
//! participation are real cash and count as ordinary flows. AMMA statements
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

use crate::infra::decimal::parse_dec;
use crate::infra::fx::to_aud;
use axum::{Json, Router, extract::State, http::StatusCode, routing::post};
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
}

#[derive(Debug, Default, Deserialize)]
pub struct PerformanceRequest {
    /// Current price per unit by listing id, expected in AUD so it lines up
    /// with the AUD-denominated flows.
    #[serde(default)]
    pub prices: HashMap<i64, Decimal>,
    /// Valuation date for the supplied prices; flows after it are ignored.
    /// Defaults to today.
    #[serde(default)]
    pub as_of_date: Option<NaiveDate>,
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
    id: i64,
    listing_id: i64,
    account_id: i64,
    is_sell: bool,
    date: NaiveDate,
    quantity: Decimal,
    price: Decimal,
    brokerage: Decimal,
    gst: Decimal,
    currency: String,
    fx_rate: Decimal,
    deemed: Option<NaiveDate>,
    group: Option<(GroupKind, i64)>,
}

/// Annualised money-weighted rate of return (as a fraction, e.g. 0.10 = 10%
/// p.a.) over dated flows: the rate at which the flows' net present value is
/// zero, with time in years (actual/365) from the first flow. Solved by
/// bisection on [−99.99%, +1,000%]; `None` when no such rate exists there
/// (one-sided flows, all flows on one day, or no sign change).
fn money_weighted_annual_return(flows: &[(NaiveDate, Decimal)]) -> Option<Decimal> {
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
    let npv = |rate: Decimal| -> Option<Decimal> {
        let base = Decimal::ONE + rate;
        let mut total = Decimal::ZERO;
        for &(t, amount) in &timed {
            let discount = base.checked_powd(t)?;
            if discount.is_zero() {
                return None;
            }
            total += amount / discount;
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
    for _ in 0..64 {
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

/// Assemble one report row from accumulated figures. `open` = units are still
/// held, so the market value is required for the value-dependent metrics.
fn build_row(
    listing_id: Option<i64>,
    ticker: String,
    holding_account_id: Option<i64>,
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
        listing_id,
        ticker,
        holding_account_id,
        quantity_held,
        invested: acc.invested,
        proceeds: acc.proceeds,
        income: acc.income,
        market_value,
        total_return,
        total_return_pct,
        money_weighted_return_pct,
        income_yield_pct,
    }
}

pub async fn db_performance(
    pool: &SqlitePool,
    prices: &HashMap<i64, Decimal>,
    as_of: NaiveDate,
) -> Result<Vec<HoldingPerformance>, sqlx::Error> {
    let trade_rows = sqlx::query(
        "SELECT id, listing_id, holding_account_id, trade_type, date, quantity, \
         average_price, brokerage, gst_on_brokerage, currency, fx_rate, \
         deemed_acquisition_date, transfer_id, scrip_action_id, demerger_action_id \
         FROM trades WHERE date <= ?",
    )
    .bind(as_of)
    .fetch_all(pool)
    .await?;

    let income_rows = sqlx::query(
        "SELECT listing_id, holding_account_id, date_paid, franked_amount, \
         unfranked_amount, foreign_source_income, foreign_tax_paid, \
         tfn_withholding_tax, currency FROM income WHERE date_paid <= ?",
    )
    .bind(as_of)
    .fetch_all(pool)
    .await?;

    if trade_rows.is_empty() && income_rows.is_empty() {
        return Ok(vec![]);
    }

    let mut trades = Vec::with_capacity(trade_rows.len());
    for row in &trade_rows {
        let transfer_id: Option<i64> = row.try_get("transfer_id")?;
        let scrip_id: Option<i64> = row.try_get("scrip_action_id")?;
        let demerger_id: Option<i64> = row.try_get("demerger_action_id")?;
        let group = transfer_id
            .map(|id| (GroupKind::Transfer, id))
            .or(scrip_id.map(|id| (GroupKind::Scrip, id)))
            .or(demerger_id.map(|id| (GroupKind::Demerger, id)));
        let trade_type: String = row.try_get("trade_type")?;
        trades.push(TradeFlow {
            id: row.try_get("id")?,
            listing_id: row.try_get("listing_id")?,
            account_id: row.try_get("holding_account_id")?,
            is_sell: trade_type == "Sell",
            date: row.try_get("date")?,
            quantity: parse_dec("quantity", row.try_get("quantity")?)?,
            price: parse_dec("average_price", row.try_get("average_price")?)?,
            brokerage: parse_dec("brokerage", row.try_get("brokerage")?)?,
            gst: parse_dec("gst_on_brokerage", row.try_get("gst_on_brokerage")?)?,
            currency: row.try_get("currency")?,
            fx_rate: parse_dec("fx_rate", row.try_get("fx_rate")?)?,
            deemed: row.try_get("deemed_acquisition_date")?,
            group,
        });
    }

    // Units sold out of each purchase parcel by as_of (with sale dates, so the
    // allocated quantity is re-based across splits like the other reports).
    let alloc_rows = sqlx::query(
        "SELECT pa.purchase_trade_id, pa.quantity_allocated, s.date AS sale_date \
         FROM parcel_allocations pa JOIN trades s ON s.id = pa.sale_trade_id \
         WHERE s.date <= ?",
    )
    .bind(as_of)
    .fetch_all(pool)
    .await?;
    let mut qty_sold: HashMap<i64, Vec<(NaiveDate, Decimal)>> = HashMap::new();
    for row in &alloc_rows {
        let tid: i64 = row.try_get("purchase_trade_id")?;
        qty_sold.entry(tid).or_default().push((
            row.try_get("sale_date")?,
            parse_dec("quantity_allocated", row.try_get("quantity_allocated")?)?,
        ));
    }

    let split_events = crate::entities::corporate_action::db_share_split_events(pool).await?;
    let ticker_rows = sqlx::query("SELECT id, ticker FROM listings").fetch_all(pool).await?;
    let mut tickers: HashMap<i64, String> = HashMap::new();
    for row in &ticker_rows {
        tickers.insert(row.try_get("id")?, row.try_get("ticker")?);
    }

    let mut holdings: BTreeMap<(i64, i64), Acc> = BTreeMap::new();
    let mut overall = Acc::default();
    // Carried AUD cost per internal-movement group: what the replacement
    // parcels took with them, which is what the closing Sell is worth.
    let mut group_costs: HashMap<(GroupKind, i64), Decimal> = HashMap::new();

    // Pass 1 — acquisitions (also totals each internal group's carried cost,
    // which pass 2 needs for the closing Sells).
    for t in trades.iter().filter(|t| !t.is_sell) {
        let cost = t.price * t.quantity + t.brokerage + t.gst;
        // Convert at the acquisition month — the deemed acquisition month for a
        // rollover/transfer-created parcel, preserving the original AUD cost.
        let acquired = t.deemed.unwrap_or(t.date);
        let cost = to_aud(pool, cost, &t.currency, acquired, Some(t.fx_rate)).await?;
        let acc = holdings.entry((t.listing_id, t.account_id)).or_default();
        acc.invested += cost;
        acc.flows.push((t.date, -cost));
        if let Some(key) = t.group {
            *group_costs.entry(key).or_insert(Decimal::ZERO) += cost;
        } else {
            overall.invested += cost;
            overall.flows.push((t.date, -cost));
        }

        // Units of this parcel still open at as_of (in as-of units).
        let splits = split_events.get(&t.listing_id).map_or(&[][..], |v| v);
        let sold = crate::entities::corporate_action::sold_in_acquired_units(
            qty_sold.get(&t.id).map_or(&[][..], |v| v),
            splits,
            t.date,
        );
        let remaining = t.quantity - sold;
        if remaining > Decimal::ZERO {
            acc.quantity += crate::entities::corporate_action::split_adjusted_quantity(
                remaining,
                splits,
                t.date,
                Some(as_of),
            );
        }
    }

    // Pass 2 — disposals. An internal closing Sell is worth the carried cost
    // its replacement parcels took (no external cash moved); a real Sell is
    // worth its net proceeds.
    for t in trades.iter().filter(|t| t.is_sell) {
        if let Some(key) = t.group {
            let carried = group_costs.get(&key).copied().unwrap_or(Decimal::ZERO);
            let acc = holdings.entry((t.listing_id, t.account_id)).or_default();
            acc.proceeds += carried;
            acc.flows.push((t.date, carried));
        } else {
            let net = t.price * t.quantity - t.brokerage - t.gst;
            let net = to_aud(pool, net, &t.currency, t.date, Some(t.fx_rate)).await?;
            let acc = holdings.entry((t.listing_id, t.account_id)).or_default();
            acc.proceeds += net;
            acc.flows.push((t.date, net));
            overall.proceeds += net;
            overall.flows.push((t.date, net));
        }
    }

    // Cash income. Same cash definition as the DRP reinvestable amount:
    // franking credits are a tax offset, not cash received.
    let trailing_start = as_of
        .checked_sub_months(Months::new(12))
        .unwrap_or(NaiveDate::MIN);
    for row in &income_rows {
        let listing_id: i64 = row.try_get("listing_id")?;
        let account_id: i64 = row.try_get("holding_account_id")?;
        let date_paid: NaiveDate = row.try_get("date_paid")?;
        let cash = parse_dec("franked_amount", row.try_get("franked_amount")?)?
            + parse_dec("unfranked_amount", row.try_get("unfranked_amount")?)?
            + parse_dec("foreign_source_income", row.try_get("foreign_source_income")?)?
            - parse_dec("foreign_tax_paid", row.try_get("foreign_tax_paid")?)?
            - parse_dec("tfn_withholding_tax", row.try_get("tfn_withholding_tax")?)?;
        let currency: String = row.try_get("currency")?;
        // Income rows carry no manual fx override: a non-AUD amount with no
        // ATO rate fails loudly (same as the tax summary).
        let cash = to_aud(pool, cash, &currency, date_paid, None).await?;
        let acc = holdings.entry((listing_id, account_id)).or_default();
        acc.income += cash;
        acc.flows.push((date_paid, cash));
        overall.income += cash;
        overall.flows.push((date_paid, cash));
        if date_paid > trailing_start {
            acc.trailing_income += cash;
            overall.trailing_income += cash;
        }
    }

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
            Some(listing_id),
            ticker,
            Some(account_id),
            Some(acc.quantity),
            market_value,
            open,
            acc,
            as_of,
        ));
    }
    let overall_mv = if any_open { overall_mv } else { None };
    result.push(build_row(
        None,
        "OVERALL".to_string(),
        None,
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
    body: Option<Json<PerformanceRequest>>,
) -> Result<Json<Vec<HoldingPerformance>>, StatusCode> {
    let (prices, as_of_date) = body
        .map(|Json(req)| (req.prices, req.as_of_date))
        .unwrap_or_default();
    let as_of = as_of_date.unwrap_or_else(|| chrono::Local::now().date_naive());
    db_performance(&pool, &prices, as_of)
        .await
        .map(Json)
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::entities::{income, listing, parcel_allocation, trade};
    use crate::infra::db;
    use axum::{body::Body, http::Request};
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    async fn test_pool() -> SqlitePool {
        db::init(":memory:").await.unwrap()
    }

    fn d(y: i32, m: u32, day: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, day).unwrap()
    }

    async fn insert_listing(pool: &SqlitePool, id: i64, ticker: &str, mic: &str, currency: &str) {
        listing::db_upsert(
            pool,
            &listing::Listing {
                id,
                exchange_mic: Some(mic.to_string()),
                ticker: ticker.to_string(),
                name: ticker.to_string(),
                isin: None,
                security_type: listing::SecurityType::Share,
                currency: currency.to_string(),
                amit: false,
                preference: false,
            },
        )
        .await
        .unwrap();
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
        trade::db_upsert(
            pool,
            &trade::Trade {
                brokerage_includes_gst: false,
                statement_total: None,
                holding_account_id: account_id,
                transfer_id: None,
                id,
                trade_type,
                date,
                settlement_date: date + chrono::Duration::days(2),
                listing_id,
                average_price: price,
                quantity: qty,
                currency: currency.to_string(),
                brokerage: Decimal::ZERO,
                gst_on_brokerage: Decimal::ZERO,
                brokerage_currency: currency.to_string(),
                fx_rate,
                contract_note_ref: None,
                residual_brought_forward: Decimal::ZERO,
                residual_carried_forward: Decimal::ZERO,
                residual_paid_out: Decimal::ZERO,
                rights_action_id: None,
                buyback_action_id: None,
                scrip_action_id: None,
                demerger_action_id: None,
                deemed_acquisition_date: None,
            },
        )
        .await
        .unwrap();
    }

    async fn buy(pool: &SqlitePool, id: i64, listing: i64, date: NaiveDate, qty: i64, price: i64) {
        insert_trade(pool, id, trade::TradeType::Buy, listing, 1, date,
            Decimal::from(qty), Decimal::from(price), "AUD", Decimal::ONE).await;
    }

    async fn sell(pool: &SqlitePool, id: i64, listing: i64, date: NaiveDate, qty: i64, price: i64) {
        insert_trade(pool, id, trade::TradeType::Sell, listing, 1, date,
            Decimal::from(qty), Decimal::from(price), "AUD", Decimal::ONE).await;
    }

    async fn allocate(pool: &SqlitePool, id: i64, sale_id: i64, buy_id: i64, qty: i64) {
        parcel_allocation::db_upsert(
            pool,
            &parcel_allocation::ParcelAllocation {
                id,
                sale_trade_id: sale_id,
                purchase_trade_id: buy_id,
                quantity_allocated: Decimal::from(qty),
            },
        )
        .await
        .unwrap();
    }

    /// An unfranked cash distribution (everything else zero).
    async fn insert_income(
        pool: &SqlitePool,
        id: i64,
        listing_id: i64,
        date_paid: NaiveDate,
        amount: i64,
    ) {
        income::db_upsert(
            pool,
            &income::Income {
                id,
                listing_id,
                date_paid,
                ex_date: None,
                franked_amount: Decimal::ZERO,
                unfranked_amount: Decimal::from(amount),
                foreign_source_income: Decimal::ZERO,
                foreign_tax_paid: Decimal::ZERO,
                tfn_withholding_tax: Decimal::ZERO,
                franking_credits: Decimal::ZERO,
                lic_capital_gain_deduction: Decimal::ZERO,
                conduit_foreign_income: Decimal::ZERO,
                trust_income: false,
                reinvestment_trade_id: None,
                currency: "AUD".to_string(),
                buyback_trade_id: None,
                holding_account_id: 1,
            },
        )
        .await
        .unwrap();
    }

    fn price_map(listing_id: i64, price: &str) -> HashMap<i64, Decimal> {
        HashMap::from([(listing_id, price.parse().unwrap())])
    }

    #[tokio::test]
    async fn db_empty_returns_empty() {
        let pool = test_pool().await;
        let rows = db_performance(&pool, &HashMap::new(), d(2024, 12, 31)).await.unwrap();
        assert!(rows.is_empty());
    }

    /// One open holding with income and a price: invested, market value,
    /// total return (absolute + %), and the trailing-12-month income yield.
    #[tokio::test]
    async fn db_open_holding_reports_value_return_and_yield() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "VAS", "XASX", "AUD").await;
        buy(&pool, 1, 1, d(2024, 1, 1), 100, 10).await; // invested 1,000
        insert_income(&pool, 1, 1, d(2024, 6, 30), 50).await;

        let rows =
            db_performance(&pool, &price_map(1, "12"), d(2024, 12, 31)).await.unwrap();
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
        assert_eq!((o.listing_id, o.holding_account_id, o.quantity_held), (None, None, None));
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
        buy(&pool, 1, 1, d(2023, 1, 1), 100, 10).await;

        let rows = db_performance(&pool, &price_map(1, "11"), d(2024, 1, 1)).await.unwrap();
        assert_eq!(rows[0].money_weighted_return_pct, Some(Decimal::from(10)));
        assert_eq!(rows[1].money_weighted_return_pct, Some(Decimal::from(10)));
    }

    /// A fully sold holding needs no price: its return is realised, the
    /// market-value-dependent yield is null.
    #[tokio::test]
    async fn db_closed_holding_reports_realised_performance_without_prices() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "VAS", "XASX", "AUD").await;
        buy(&pool, 1, 1, d(2023, 1, 1), 100, 10).await;
        sell(&pool, 2, 1, d(2024, 1, 1), 100, 11).await;
        allocate(&pool, 1, 2, 1, 100).await;

        let rows = db_performance(&pool, &HashMap::new(), d(2024, 6, 30)).await.unwrap();
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
        buy(&pool, 1, 1, d(2024, 1, 1), 100, 10).await;

        let rows = db_performance(&pool, &HashMap::new(), d(2024, 12, 31)).await.unwrap();
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
        buy(&pool, 1, 1, d(2022, 1, 1), 100, 10).await;
        insert_income(&pool, 1, 1, d(2022, 6, 30), 40).await; // outside the window
        insert_income(&pool, 2, 1, d(2024, 6, 30), 60).await; // inside

        let rows =
            db_performance(&pool, &price_map(1, "12"), d(2024, 12, 31)).await.unwrap();
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
            &holding_account::HoldingAccount { id: 2, name: "Broker".to_string() },
        )
        .await
        .unwrap();
        buy(&pool, 1, 1, d(2023, 1, 1), 100, 10).await; // 1,000 into account 1
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
            },
        )
        .await
        .unwrap();

        let rows = db_performance(&pool, &price_map(1, "15"), d(2024, 1, 1)).await.unwrap();
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

    /// Non-AUD flows convert to AUD (here via the trade's manual fx fallback,
    /// foreign-per-AUD); supplied prices are AUD already.
    #[tokio::test]
    async fn db_non_aud_invested_converts_to_aud() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "ICE", "XNYS", "USD").await;
        // 100 × US$10 = US$1,000 at 2 USD/AUD → A$500 invested.
        insert_trade(&pool, 1, trade::TradeType::Buy, 1, 1, d(2024, 1, 1),
            Decimal::from(100), Decimal::from(10), "USD", Decimal::from(2)).await;

        let rows = db_performance(&pool, &price_map(1, "6"), d(2024, 12, 31)).await.unwrap();
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
        buy(&pool, 1, 1, d(2024, 1, 1), 100, 10).await;
        sell(&pool, 2, 1, d(2024, 6, 1), 100, 15).await;
        allocate(&pool, 1, 2, 1, 100).await;
        insert_income(&pool, 1, 1, d(2024, 5, 1), 50).await;

        // As at 31 Mar the sale and the May distribution haven't happened.
        let rows = db_performance(&pool, &price_map(1, "12"), d(2024, 3, 31)).await.unwrap();
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
        buy(&pool, 1, 1, d(2024, 1, 1), 100, 10).await;

        let body = serde_json::json!({
            "prices": { "1": "12" },
            "as_of_date": "2024-12-31",
        });
        let resp = router()
            .with_state(pool)
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/portfolio/performance")
                    .header("content-type", "application/json")
                    .body(Body::from(body.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let rows: Vec<HoldingPerformance> = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].ticker, "VAS");
        assert_eq!(rows[0].market_value, Some(Decimal::from(1200)));
        assert_eq!(rows.last().unwrap().ticker, "OVERALL");
    }

    #[tokio::test]
    async fn api_performance_without_body_defaults_to_today() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "VAS", "XASX", "AUD").await;
        buy(&pool, 1, 1, d(2024, 1, 1), 100, 10).await;

        let resp = router()
            .with_state(pool)
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/portfolio/performance")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let rows: Vec<HoldingPerformance> = serde_json::from_slice(&bytes).unwrap();
        // Unpriced open holding: figures present, market metrics null.
        assert_eq!(rows[0].invested, Decimal::from(1000));
        assert_eq!(rows[0].market_value, None);
    }
}
