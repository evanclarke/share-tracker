//! The annual tax report: a printable, per-year tax document meant to be
//! saved to PDF and archived — enough detail to hand-check every figure
//! against the source contract notes and statements. A presentation and
//! reconciliation layer only: nothing here computes a new tax figure. Every
//! number is sourced from the existing pipelines
//! ([`crate::domain::cost_base`], [`super::realised_gains`],
//! [`super::net_capital_gain`], [`super::tax_summary`]) — a second
//! implementation of a tax rule here would be a correctness bug, not a
//! feature. Distinct from the multi-year [`super::tax_summary`] screen, which
//! is unchanged and stays as the all-years/spreadsheet path.
//!
//! The core financial sections (the disposal schedule, the CGT summary, the
//! year's [`super::tax_summary::TaxYearSummary`] line) read on one
//! `pool.begin()` transaction, per the house rule for multi-query reports.
//! The completeness section's three existing cross-checks
//! ([`super::amit_cash_cross_check`], [`super::e4_cross_check`],
//! [`super::amit_adjustment_cross_check`]) and the
//! per-record income/franking detail deliberately read on their own
//! snapshots (their existing pool-based `db_*` functions) rather than folding
//! into that transaction: they are advisory notes and per-record detail rows
//! alongside a total computed elsewhere, so a rare interleaved write between
//! them and the main transaction could only change whether an advisory note
//! fires, never a reported dollar figure.

use crate::domain::cost_base::{self, CostBaseAdjustment, ParcelRow};
use crate::domain::listing_identity::RenameHistory;
use crate::domain::tax_year::tax_year_for;
use crate::entities::corporate_action::{RocEvent, SplitEvent};
use crate::entities::income::Income;
use crate::entities::trade::{Trade, TradeType};
use crate::infra::fx::FxRates;
use crate::infra::http::ApiError;
use crate::reports::realised_gains::DisposalSource;
use crate::reports::{
    activity, amit_adjustment_cross_check, amit_cash_cross_check, e4_cross_check, franking_at_risk,
    net_capital_gain, realised_gains, tax_summary,
};
use axum::{
    Json, Router,
    extract::State,
    routing::{get, post},
};
use chrono::{NaiveDate, Utc};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use sqlx::{Row, SqlitePool};
use std::collections::{HashMap, HashSet};

pub fn router() -> Router<SqlitePool> {
    Router::new()
        .route("/reports/tax-report/years", get(years_handler))
        .route("/reports/tax-report", post(tax_report_handler))
}

#[derive(Debug, Deserialize)]
pub struct TaxReportRequest {
    pub tax_year: i32,
}

// ---- meta ---------------------------------------------------------------

#[derive(Debug, Serialize)]
pub struct TaxReportMeta {
    pub tax_year: i32,
    pub period_start: NaiveDate,
    pub period_end: NaiveDate,
    pub generated_at: chrono::DateTime<Utc>,
    pub taxpayer_basis: String,
}

fn period_for(tax_year: i32) -> (NaiveDate, NaiveDate) {
    (
        NaiveDate::from_ymd_opt(tax_year - 1, 7, 1).expect("valid period start"),
        NaiveDate::from_ymd_opt(tax_year, 6, 30).expect("valid period end"),
    )
}

// ---- completeness ---------------------------------------------------------

#[derive(Debug, Serialize)]
pub struct AmmaMissingAlert {
    pub listing_id: i64,
    pub ticker: String,
}

#[derive(Debug, Serialize)]
pub struct Completeness {
    pub complete: bool,
    /// AMIT listings held at any point in the year with no AMMA statement
    /// covering it — holdings-based, so (unlike
    /// [`amit_cash_cross_check`](super::amit_cash_cross_check), whose own doc
    /// comment names the gap) this also catches a fund-year where no cash
    /// rows were entered at all.
    pub amma_missing: Vec<AmmaMissingAlert>,
    pub amit_cash_alerts: Vec<amit_cash_cross_check::AmitCashAlert>,
    pub e4_alerts: Vec<e4_cross_check::E4CrossCheckAlert>,
    /// AMMA statements for this year whose per-parcel AMIT adjustment set
    /// does not reconcile to the statement. An adjustment gap distorts the
    /// disposal schedule's cost base — this report's central figure — so it
    /// belongs to the gate the completeness section is.
    pub amit_adjustment_alerts: Vec<amit_adjustment_cross_check::AmitAdjustmentAlert>,
}

/// Every AMIT listing with a non-zero opening balance at the start of the
/// year, or any Buy/DRP trade dated within it — i.e. held at some point
/// during the year — that has no `amma_statements` row whose
/// `tax_year_end_date` falls in the year. A simple net-units walk (Buy/DRP
/// minus Sell quantities, not cost-base aware): good enough for a
/// held/not-held flag, not a financial figure.
async fn amma_missing(
    conn: &mut sqlx::SqliteConnection,
    tax_year: i32,
) -> Result<Vec<AmmaMissingAlert>, sqlx::Error> {
    let (start, end) = period_for(tax_year);

    let amit_listings: Vec<(i64, String)> =
        sqlx::query("SELECT id, ticker FROM listings WHERE amit")
            .fetch_all(&mut *conn)
            .await?
            .into_iter()
            .map(|r| Ok::<_, sqlx::Error>((r.try_get("id")?, r.try_get("ticker")?)))
            .collect::<Result<_, _>>()?;
    if amit_listings.is_empty() {
        return Ok(vec![]);
    }

    let trade_rows = sqlx::query(
        "SELECT listing_id, trade_type, date, quantity FROM trades \
         WHERE listing_id IN (SELECT id FROM listings WHERE amit) \
           AND trade_type IN ('Buy', 'DRP', 'Sell') \
         ORDER BY listing_id, date",
    )
    .fetch_all(&mut *conn)
    .await?;

    let mut opening: HashMap<i64, Decimal> = HashMap::new();
    let mut bought_in_year: HashSet<i64> = HashSet::new();
    for row in &trade_rows {
        let listing_id: i64 = row.try_get("listing_id")?;
        let trade_type: TradeType = row.try_get("trade_type")?;
        let date: NaiveDate = row.try_get("date")?;
        let qty = crate::infra::decimal::row_dec(row, "quantity")?;
        let signed = match trade_type {
            TradeType::Buy | TradeType::DRP => qty,
            TradeType::Sell => -qty,
        };
        if date < start {
            *opening.entry(listing_id).or_insert(Decimal::ZERO) += signed;
        } else if date <= end && trade_type.is_acquisition() {
            bought_in_year.insert(listing_id);
        }
    }

    let covered: HashSet<i64> = sqlx::query(
        "SELECT listing_id FROM amma_statements \
         WHERE tax_year_end_date BETWEEN ? AND ?",
    )
    .bind(start)
    .bind(end)
    .fetch_all(&mut *conn)
    .await?
    .into_iter()
    .map(|r| r.try_get("listing_id"))
    .collect::<Result<_, _>>()?;

    Ok(amit_listings
        .into_iter()
        .filter(|(id, _)| {
            let held =
                opening.get(id).is_some_and(|q| *q > Decimal::ZERO) || bought_in_year.contains(id);
            held && !covered.contains(id)
        })
        .map(|(listing_id, ticker)| AmmaMissingAlert { listing_id, ticker })
        .collect())
}

// ---- disposals ------------------------------------------------------------

#[derive(Debug, Default, Clone, Copy, Serialize)]
pub struct DisposalTotals {
    pub proceeds_aud: Decimal,
    pub cost_base_aud: Decimal,
    pub gain_loss_aud: Decimal,
    pub cgt_discount_amount_aud: Decimal,
    pub gain_after_discount_aud: Decimal,
}

impl DisposalTotals {
    fn add(&mut self, r: &DisposalParcelRow) {
        self.proceeds_aud += r.proceeds_aud;
        self.cost_base_aud += r.adjusted_cost_base_aud;
        self.gain_loss_aud += r.gain_loss_aud;
        self.cgt_discount_amount_aud += r.cgt_discount_amount_aud;
        self.gain_after_discount_aud += r.gain_after_discount_aud;
    }
}

#[derive(Debug, Serialize)]
pub struct DisposalParcelRow {
    pub source: DisposalSource,
    pub sale_trade_id: i64,
    pub purchase_trade_id: i64,
    pub holding_account_id: i64,

    // Acquisition / traceability
    pub acquisition_date: NaiveDate,
    /// The actual trade date, when it differs from `acquisition_date` (an
    /// inherited or rollover-replacement parcel's deemed acquisition date).
    pub trade_date: Option<NaiveDate>,
    pub acquisition_method: String,
    pub buy_contract_note_ref: Option<String>,
    pub sale_contract_note_ref: Option<String>,

    // Buy side (native currency unless suffixed _aud)
    pub units: Decimal,
    pub buy_price: Option<Decimal>,
    pub buy_brokerage: Option<Decimal>,
    pub buy_gst_on_brokerage: Option<Decimal>,
    pub initial_cost_base_aud: Decimal,
    pub cost_base_per_unit_aud: Decimal,

    // Itemised cost-base adjustments (AUD), under the cost base.
    pub adjustments: Vec<CostBaseAdjustment>,
    pub adjusted_cost_base_aud: Decimal,

    // Sale side
    pub sale_date: NaiveDate,
    pub sale_price: Option<Decimal>,
    pub proceeds_aud: Decimal,
    pub proceeds_per_unit_aud: Decimal,

    // Outcome
    pub gain_loss_aud: Decimal,
    pub days_held: i64,
    pub discount_eligible: bool,
    pub cgt_discount_amount_aud: Decimal,
    pub gain_after_discount_aud: Decimal,

    // FX detail — populated only for a non-AUD parcel.
    pub currency: String,
    pub buy_month_fx_rate: Option<Decimal>,
    pub sell_month_fx_rate: Option<Decimal>,
}

#[derive(Debug, Serialize)]
pub struct DisposalListingGroup {
    pub listing_id: i64,
    pub ticker: String,
    pub listing_name: String,
    pub parcels: Vec<DisposalParcelRow>,
    pub subtotal: DisposalTotals,
}

#[derive(Debug, Serialize)]
pub struct DisposalsSection {
    pub listings: Vec<DisposalListingGroup>,
    pub totals: DisposalTotals,
}

/// Everything [`disposal_parcel_rows`] needs, read once on the caller's
/// transaction.
struct DisposalInputs {
    buys: HashMap<i64, ParcelRow>,
    trades: HashMap<i64, Trade>,
    amit_events: HashMap<i64, Vec<cost_base::AmitReductionEvent>>,
    roc_events: HashMap<i64, Vec<RocEvent>>,
    split_events: HashMap<i64, Vec<SplitEvent>>,
    fee_sale_ids: HashSet<i64>,
    fx: FxRates,
}

async fn load_disposal_inputs(
    conn: &mut sqlx::SqliteConnection,
) -> Result<DisposalInputs, sqlx::Error> {
    let buys: Vec<ParcelRow> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT {} FROM trades WHERE trade_type IN ('Buy', 'DRP')",
        ParcelRow::COLUMNS
    )))
    .fetch_all(&mut *conn)
    .await?;

    let all_trades: Vec<Trade> = sqlx::query_as(
        "SELECT id, trade_type, date, settlement_date, listing_id, average_price, quantity, \
         currency, brokerage, gst_on_brokerage, brokerage_includes_gst, brokerage_currency, \
         fx_rate, spot_fx_rate, contract_note_ref, statement_total, \
         residual_brought_forward, residual_carried_forward, residual_paid_out, rights_action_id, \
         buyback_action_id, scrip_action_id, demerger_action_id, deemed_acquisition_date, \
         holding_account_id, transfer_id, ess_statement_id, worthless_action_id, inheritance_id \
         FROM trades",
    )
    .fetch_all(&mut *conn)
    .await?;

    // Transfer-out Sells that fund a crypto network fee (activity's "transfer
    // network fee" qualifier) — reused here so a disposed parcel's
    // acquisition label matches the Listing Activity ledger exactly.
    let fee_sale_ids: HashSet<i64> = sqlx::query_scalar(
        "SELECT fee_sale_trade_id FROM transfers WHERE fee_sale_trade_id IS NOT NULL",
    )
    .fetch_all(&mut *conn)
    .await?
    .into_iter()
    .collect();

    let amit_events =
        crate::entities::amit_adjustment::db_cost_base_reduction_events(&mut *conn, None).await?;
    let roc_events =
        crate::entities::corporate_action::db_return_of_capital_events(&mut *conn).await?;
    let split_events = crate::entities::corporate_action::db_share_split_events(&mut *conn).await?;
    let fx = FxRates::load(&mut *conn).await?;

    Ok(DisposalInputs {
        buys: buys.into_iter().map(|b| (b.id, b)).collect(),
        trades: all_trades.into_iter().map(|t| (t.id, t)).collect(),
        amit_events,
        roc_events,
        split_events,
        fee_sale_ids,
        fx,
    })
}

/// Build one printable row per parcel allocation behind `disposal`, itemising
/// the cost-base adjustments via [`cost_base::adjustment_detail`] — the exact
/// same inputs `realised_gains` fed [`cost_base::adjusted_cost_base`] for
/// this allocation, so the totals can never disagree (a test pins it).
/// Rights-sale disposals (no Buy/DRP cost-base pipeline of their own — the
/// rights cost is a flat figure, not itemised) get a row with no adjustments.
fn disposal_parcel_rows(
    disposal: &realised_gains::RealisedGainLoss,
    inputs: &DisposalInputs,
) -> Vec<DisposalParcelRow> {
    disposal
        .parcels
        .iter()
        .map(|p| {
            let buy = inputs.buys.get(&p.purchase_trade_id);
            let buy_trade = inputs.trades.get(&p.purchase_trade_id);
            let sale_trade = if disposal.source == DisposalSource::Sell {
                inputs.trades.get(&disposal.sale_trade_id)
            } else {
                None
            };
            let currency = buy_trade
                .map(|t| t.currency.clone())
                .unwrap_or_else(|| "AUD".to_string());

            // Both calls below feed `cost_base::adjusted_cost_base` /
            // `adjustment_detail` the exact same inputs `realised_gains` used
            // for this allocation's `p.cost_base` (as-acquired units, the
            // parcel's cumulative AMIT total, the listing's ROC/split events,
            // `up_to` = the sale date) — a test pins the two to agree. Any FX
            // failure here is unreachable in practice: `realised_gains`
            // already resolved the same rate to produce `p.cost_base`, so a
            // resolution failure would have surfaced there first; the
            // fallback to `p.cost_base` just keeps this presentation-only
            // detail from panicking if that invariant is ever violated.
            let (adjustments, initial_cost_base_aud) = match (disposal.source, buy) {
                (DisposalSource::Sell, Some(buy_row)) => {
                    let splits = inputs
                        .split_events
                        .get(&buy_row.listing_id)
                        .map_or(&[][..], |v| v);
                    let units_acquired = crate::entities::corporate_action::as_acquired_quantity(
                        p.units,
                        splits,
                        buy_row.date,
                        disposal.sale_date,
                    );
                    let amit = inputs
                        .amit_events
                        .get(&p.purchase_trade_id)
                        .map_or(&[][..], |v| v);
                    let roc = inputs
                        .roc_events
                        .get(&buy_row.listing_id)
                        .map_or(&[][..], |v| v);
                    let rows = cost_base::adjustment_detail(
                        &buy_row.parcel(),
                        units_acquired,
                        amit,
                        roc,
                        splits,
                        cost_base::Held::DisposedOn(disposal.sale_date),
                    )
                    .unwrap_or_default();
                    let native = cost_base::adjusted_cost_base(
                        &buy_row.parcel(),
                        units_acquired,
                        amit,
                        roc,
                        splits,
                        cost_base::Held::DisposedOn(disposal.sale_date),
                    );
                    let rate = inputs
                        .fx
                        .resolve_rate(&buy_row.currency, buy_row.acquired(), buy_row.fx_override())
                        .unwrap_or(Decimal::ONE);
                    let aud_initial = native
                        .ok()
                        .and_then(|cb| {
                            cb.into_aud_with(
                                &inputs.fx,
                                &buy_row.currency,
                                buy_row.acquired(),
                                buy_row.fx_override(),
                            )
                            .ok()
                        })
                        .map(|cb| cb.initial_cost)
                        .unwrap_or(p.cost_base);
                    let aud_rows: Vec<CostBaseAdjustment> = rows
                        .into_iter()
                        .map(|mut r| {
                            if rate != Decimal::ONE {
                                r.amount /= rate;
                                r.per_unit = r.per_unit.map(|pu| pu / rate);
                            }
                            r
                        })
                        .collect();
                    (aud_rows, aud_initial)
                }
                _ => (Vec::new(), p.cost_base),
            };

            let acquisition_method = buy_trade
                .map(|t| activity::trade_event(t, &inputs.fee_sale_ids))
                .unwrap_or_else(|| "Buy".to_string());
            let trade_date = buy_trade
                .map(|t| t.date)
                .filter(|d| *d != p.acquisition_date);

            let discount_amount = if p.discount_eligible && p.capital_gain_loss > Decimal::ZERO {
                p.capital_gain_loss / Decimal::from(2)
            } else {
                Decimal::ZERO
            };
            let sell_rate = buy_trade.and_then(|_| {
                inputs
                    .fx
                    .resolve_rate(
                        &currency,
                        disposal.sale_date,
                        crate::infra::fx::FxOverride::None,
                    )
                    .ok()
            });
            let buy_rate = buy_trade.and_then(|bt| {
                inputs
                    .fx
                    .resolve_rate(&currency, p.acquisition_date, bt.fx_override())
                    .ok()
            });

            DisposalParcelRow {
                source: disposal.source,
                sale_trade_id: disposal.sale_trade_id,
                purchase_trade_id: p.purchase_trade_id,
                holding_account_id: disposal.holding_account_id,
                acquisition_date: p.acquisition_date,
                trade_date,
                acquisition_method,
                buy_contract_note_ref: buy_trade.and_then(|t| t.contract_note_ref.clone()),
                sale_contract_note_ref: sale_trade.and_then(|t| t.contract_note_ref.clone()),
                units: p.units,
                buy_price: buy_trade.map(|t| t.average_price),
                buy_brokerage: buy_trade.map(|t| t.brokerage),
                buy_gst_on_brokerage: buy_trade.map(|t| t.gst_on_brokerage),
                initial_cost_base_aud,
                cost_base_per_unit_aud: if p.units > Decimal::ZERO {
                    p.cost_base / p.units
                } else {
                    Decimal::ZERO
                },
                adjustments,
                adjusted_cost_base_aud: p.cost_base,
                sale_date: disposal.sale_date,
                sale_price: sale_trade.map(|t| t.average_price),
                proceeds_aud: p.proceeds,
                proceeds_per_unit_aud: if p.units > Decimal::ZERO {
                    p.proceeds / p.units
                } else {
                    Decimal::ZERO
                },
                gain_loss_aud: p.capital_gain_loss,
                days_held: (disposal.sale_date - p.acquisition_date).num_days(),
                discount_eligible: p.discount_eligible,
                cgt_discount_amount_aud: discount_amount,
                gain_after_discount_aud: p.capital_gain_loss - discount_amount,
                currency: currency.clone(),
                buy_month_fx_rate: if currency == "AUD" { None } else { buy_rate },
                sell_month_fx_rate: if currency == "AUD" { None } else { sell_rate },
            }
        })
        .collect()
}

async fn disposals_section(
    conn: &mut sqlx::SqliteConnection,
    tax_year: i32,
) -> Result<DisposalsSection, sqlx::Error> {
    let all = realised_gains::db_realised_gains_on(&mut *conn).await?;
    let year_disposals: Vec<_> = all
        .into_iter()
        .filter(|d| tax_year_for(d.sale_date) == tax_year)
        .collect();
    if year_disposals.is_empty() {
        return Ok(DisposalsSection {
            listings: Vec::new(),
            totals: DisposalTotals::default(),
        });
    }

    let inputs = load_disposal_inputs(conn).await?;
    let listing_names: HashMap<i64, (String, String)> =
        sqlx::query("SELECT id, ticker, name FROM listings")
            .fetch_all(&mut *conn)
            .await?
            .into_iter()
            .map(|r| {
                Ok::<_, sqlx::Error>((
                    r.try_get::<i64, _>("id")?,
                    (r.try_get("ticker")?, r.try_get("name")?),
                ))
            })
            .collect::<Result<_, _>>()?;
    let renames = RenameHistory::load(conn).await?;

    let mut by_listing: HashMap<i64, Vec<DisposalParcelRow>> = HashMap::new();
    for d in &year_disposals {
        by_listing
            .entry(d.listing_id)
            .or_default()
            .extend(disposal_parcel_rows(d, &inputs));
    }

    let mut totals = DisposalTotals::default();
    let mut listings: Vec<DisposalListingGroup> = by_listing
        .into_iter()
        .map(|(listing_id, mut parcels)| {
            parcels.sort_by(|a, b| {
                a.sale_date
                    .cmp(&b.sale_date)
                    .then(a.acquisition_date.cmp(&b.acquisition_date))
            });
            let mut subtotal = DisposalTotals::default();
            for r in &parcels {
                subtotal.add(r);
                totals.add(r);
            }
            let (current_ticker, listing_name) = listing_names
                .get(&listing_id)
                .cloned()
                .unwrap_or_else(|| (format!("listing {listing_id}"), String::new()));
            // The group heading names the ticker as at its most recent
            // disposal — the taxable event closest to "now" within this
            // printed group — so it reads the way the broker statement did
            // even if a rename landed partway through the year.
            let latest_sale_date = parcels
                .iter()
                .map(|p| p.sale_date)
                .max()
                .expect("a listing group always has at least one parcel");
            let ticker = renames.ticker_as_at(listing_id, latest_sale_date, &current_ticker);
            DisposalListingGroup {
                listing_id,
                ticker,
                listing_name,
                parcels,
                subtotal,
            }
        })
        .collect();
    listings.sort_by(|a, b| a.ticker.cmp(&b.ticker));

    Ok(DisposalsSection { listings, totals })
}

// ---- income -----------------------------------------------------------

#[derive(Debug, Serialize)]
pub struct TrustIncomeRow {
    pub income_id: i64,
    pub listing_id: i64,
    pub ticker: String,
    pub date_paid: NaiveDate,
    pub entitlement_date: Option<NaiveDate>,
    pub franked_amount_aud: Decimal,
    pub unfranked_amount_aud: Decimal,
    pub foreign_source_income_aud: Decimal,
    pub tax_deferred_amount: Option<Decimal>,
    pub franking_credits_aud: Decimal,
}

#[derive(Debug, Serialize)]
pub struct AmmaStatementRow {
    pub amma_statement_id: i64,
    pub listing_id: i64,
    pub ticker: String,
    pub tax_year_end_date: NaiveDate,
    pub australian_interest_aud: Decimal,
    pub australian_dividends_unfranked_aud: Decimal,
    pub franked_dividends_aud: Decimal,
    pub franking_credits_aud: Decimal,
    pub net_rent_aud: Decimal,
    pub foreign_income_aud: Decimal,
    pub foreign_tax_credits_aud: Decimal,
    pub other_income_aud: Decimal,
    pub cgt_discount_gains_aud: Decimal,
    pub cgt_indexation_gains_aud: Decimal,
    pub cgt_other_gains_aud: Decimal,
    pub capital_losses_applied_aud: Decimal,
    pub tax_deferred_amount: Decimal,
    pub tax_free_amount: Decimal,
    pub tfn_withholding_tax_aud: Decimal,
}

#[derive(Debug, Serialize)]
pub struct DividendIncomeRow {
    pub income_id: i64,
    pub listing_id: i64,
    pub ticker: String,
    pub date_paid: NaiveDate,
    pub ex_date: Option<NaiveDate>,
    pub franked_amount_aud: Decimal,
    pub unfranked_amount_aud: Decimal,
    pub franking_credits_aud: Decimal,
    pub lic_capital_gain_deduction_aud: Decimal,
    pub tfn_withholding_tax_aud: Decimal,
    /// `entitled`, `denied`, or `exempt_small_shareholder` — from
    /// [`franking_at_risk`]; `entitled` when the row isn't in its alert list.
    pub franking_status: String,
    pub franking_credits_denied_aud: Decimal,
}

#[derive(Debug, Serialize)]
pub struct ForeignIncomeRow {
    pub kind: String,
    pub listing_id: Option<i64>,
    pub ticker: Option<String>,
    pub date: NaiveDate,
    pub amount_aud: Decimal,
    pub foreign_tax_paid_aud: Decimal,
}

#[derive(Debug, Serialize)]
pub struct InterestIncomeRow {
    pub interest_income_id: i64,
    pub date_paid: NaiveDate,
    pub source: Option<String>,
    pub amount_aud: Decimal,
    pub foreign_source: bool,
    pub foreign_tax_paid_aud: Decimal,
    pub tfn_withholding_tax_aud: Decimal,
}

#[derive(Debug, Serialize)]
pub struct EssIncomeRow {
    pub ess_statement_id: i64,
    pub listing_id: i64,
    pub ticker: String,
    pub taxing_point_date: NaiveDate,
    pub taxed_upfront_eligible_aud: Decimal,
    pub taxed_upfront_not_eligible_aud: Decimal,
    pub deferral_discount_aud: Decimal,
    pub pre_2009_cessation_discount_aud: Decimal,
    pub foreign_source_discount_aud: Decimal,
    pub tfn_withholding_aud: Decimal,
}

#[derive(Debug, Serialize)]
pub struct DeductionRow {
    pub investment_expense_id: i64,
    pub date_incurred: NaiveDate,
    pub expense_type: String,
    pub amount_aud: Decimal,
    pub listing_id: Option<i64>,
    pub description: Option<String>,
}

#[derive(Debug, Default, Serialize)]
pub struct IncomeSections {
    pub trust_income: Vec<TrustIncomeRow>,
    pub amma_statements: Vec<AmmaStatementRow>,
    /// Item 11 (Australian-company dividends) only — a non-trust income row
    /// with no franked/unfranked/franking-credit/LIC/TFN content (a foreign
    /// company's dividend, entered via `foreign_source_income` alone) is
    /// excluded rather than printing as an all-zero row; it still appears in
    /// `foreign_income` below.
    pub dividends: Vec<DividendIncomeRow>,
    pub foreign_income: Vec<ForeignIncomeRow>,
    pub interest: Vec<InterestIncomeRow>,
    pub ess: Vec<EssIncomeRow>,
    pub deductions: Vec<DeductionRow>,
}

impl IncomeSections {
    /// Every section reads chronologically.
    fn sort(&mut self) {
        self.trust_income.sort_by_key(|r| r.date_paid);
        self.dividends.sort_by_key(|r| r.date_paid);
        self.foreign_income.sort_by_key(|r| r.date);
        self.interest.sort_by_key(|r| r.date_paid);
        self.ess.sort_by_key(|r| r.taxing_point_date);
        self.deductions.sort_by_key(|r| r.date_incurred);
    }
}

/// What every income section needs: the year being reported, the FX table to
/// convert with, and the naming history to print each row's ticker as at its
/// own date.
struct IncomeContext {
    tax_year: i32,
    fx: FxRates,
    tickers: HashMap<i64, String>,
    renames: RenameHistory,
}

impl IncomeContext {
    async fn load(conn: &mut sqlx::SqliteConnection, tax_year: i32) -> Result<Self, sqlx::Error> {
        let fx = FxRates::load(&mut *conn).await?;
        let tickers: HashMap<i64, String> = sqlx::query("SELECT id, ticker FROM listings")
            .fetch_all(&mut *conn)
            .await?
            .into_iter()
            .map(|r| Ok::<_, sqlx::Error>((r.try_get("id")?, r.try_get("ticker")?)))
            .collect::<Result<_, _>>()?;
        let renames = RenameHistory::load(&mut *conn).await?;
        Ok(Self {
            tax_year,
            fx,
            tickers,
            renames,
        })
    }

    /// As at `date`: the current ticker unless renamed since, in which case
    /// the ticker in effect on `date` — so an income row prints the way the
    /// broker statement named the security when it was paid.
    fn ticker_as_at(&self, listing_id: i64, date: NaiveDate) -> String {
        let current = self
            .tickers
            .get(&listing_id)
            .map(String::as_str)
            .unwrap_or("");
        self.renames.ticker_as_at(listing_id, date, current)
    }
}

async fn income_section(
    conn: &mut sqlx::SqliteConnection,
    tax_year: i32,
    franking_alerts: &HashMap<i64, franking_at_risk::FrankingAtRiskAlert>,
) -> Result<IncomeSections, sqlx::Error> {
    let ctx = IncomeContext::load(&mut *conn, tax_year).await?;
    let mut out = IncomeSections::default();
    push_income_rows(&mut *conn, &ctx, franking_alerts, &mut out).await?;
    push_amma_rows(&mut *conn, &ctx, &mut out).await?;
    push_interest_rows(&mut *conn, &ctx, &mut out).await?;
    push_ess_rows(&mut *conn, &ctx, &mut out).await?;
    push_deduction_rows(&mut *conn, &ctx, &mut out).await?;
    out.sort();
    Ok(out)
}

/// Income rows (dividends + non-AMIT trust distributions), on the same
/// assessment-date rule the tax summary uses.
async fn push_income_rows(
    conn: &mut sqlx::SqliteConnection,
    ctx: &IncomeContext,
    franking_alerts: &HashMap<i64, franking_at_risk::FrankingAtRiskAlert>,
    out: &mut IncomeSections,
) -> Result<(), sqlx::Error> {
    let (tax_year, fx) = (ctx.tax_year, &ctx.fx);
    let income_rows: Vec<Income> = sqlx::query_as(
        "SELECT i.* FROM income i JOIN listings l ON l.id = i.listing_id \
         WHERE NOT l.amit",
    )
    .fetch_all(&mut *conn)
    .await?;

    for income in &income_rows {
        let Income {
            id: income_id,
            listing_id,
            date_paid,
            entitlement_date,
            trust_income,
            ..
        } = *income;
        let assessed = income.assessment_date();
        if tax_year_for(assessed) != tax_year {
            continue;
        }
        let ticker = ctx.ticker_as_at(listing_id, assessed);
        // Every figure converts exactly the way the tax summary's own totals
        // do, at the assessment date's month.
        let aud = |amount: Decimal| tax_summary::aud_value(fx, amount, &income.currency, assessed);
        let franked = aud(income.franked_amount)?;
        let unfranked = aud(income.unfranked_amount)?;
        let foreign = aud(income.foreign_source_income)?;
        let foreign_tax = aud(income.foreign_tax_paid)?;
        let fc = aud(income.franking_credits)?;
        let lic = aud(income.lic_capital_gain_deduction)?;
        let tfn = aud(income.tfn_withholding_tax)?;

        if trust_income {
            out.trust_income.push(TrustIncomeRow {
                income_id,
                listing_id,
                ticker,
                date_paid,
                entitlement_date,
                franked_amount_aud: franked,
                unfranked_amount_aud: unfranked,
                foreign_source_income_aud: foreign,
                tax_deferred_amount: income.tax_deferred_amount,
                franking_credits_aud: fc,
            });
        } else if !income.is_foreign_only() {
            // Item 11 (Dividends) is Australian-company dividends only — a
            // foreign company's dividend (e.g. a US-listed RSU holding) is
            // entered with only `foreign_source_income` set and reported
            // under Item 20 instead (the `foreign` push below); such a row
            // carries no Item-11 content, so it stays out of this table
            // rather than printing as an all-zero dividend line.
            let alert = franking_alerts.get(&income_id);
            out.dividends.push(DividendIncomeRow {
                income_id,
                listing_id,
                ticker,
                date_paid,
                ex_date: income.ex_date,
                franked_amount_aud: franked,
                unfranked_amount_aud: unfranked,
                franking_credits_aud: fc - alert.map_or(Decimal::ZERO, |a| a.credits_denied),
                lic_capital_gain_deduction_aud: lic,
                tfn_withholding_tax_aud: tfn,
                franking_status: alert.map_or("entitled", |a| a.status.as_str()).to_string(),
                franking_credits_denied_aud: alert.map_or(Decimal::ZERO, |a| a.credits_denied),
            });
        }
        if foreign > Decimal::ZERO || foreign_tax > Decimal::ZERO {
            out.foreign_income.push(ForeignIncomeRow {
                kind: "Dividend/trust foreign income".to_string(),
                listing_id: Some(listing_id),
                ticker: Some(ctx.ticker_as_at(listing_id, assessed)),
                date: assessed,
                amount_aud: foreign,
                foreign_tax_paid_aud: foreign_tax,
            });
        }
    }
    Ok(())
}

/// Full AMMA statement component detail for the year.
async fn push_amma_rows(
    conn: &mut sqlx::SqliteConnection,
    ctx: &IncomeContext,
    out: &mut IncomeSections,
) -> Result<(), sqlx::Error> {
    let tax_year = ctx.tax_year;
    let amma_rows = sqlx::query(
        "SELECT a.id, a.listing_id, a.tax_year_end_date, a.australian_interest, \
                a.australian_dividends_unfranked, a.franked_dividends, a.franking_credits, \
                a.net_rent, a.foreign_income, a.foreign_tax_credits, a.other_income, \
                a.cgt_discount_gains, a.cgt_indexation_gains, a.cgt_other_gains, \
                a.capital_losses_applied, a.tax_deferred_amount, a.tax_free_amount, \
                a.tfn_withholding_tax, a.currency \
         FROM amma_statements a \
         WHERE a.tax_year_end_date BETWEEN ? AND ?",
    )
    .bind(period_for(tax_year).0)
    .bind(period_for(tax_year).1)
    .fetch_all(&mut *conn)
    .await?;
    for row in &amma_rows {
        let listing_id: i64 = row.try_get("listing_id")?;
        let d: NaiveDate = row.try_get("tax_year_end_date")?;
        let statement = amma_statement_row(ctx, row, listing_id, d)?;
        let (foreign_income, foreign_tax) = (
            statement.foreign_income_aud,
            statement.foreign_tax_credits_aud,
        );
        out.amma_statements.push(statement);
        if foreign_income > Decimal::ZERO || foreign_tax > Decimal::ZERO {
            out.foreign_income.push(ForeignIncomeRow {
                kind: "AMMA foreign income".to_string(),
                listing_id: Some(listing_id),
                ticker: Some(ctx.ticker_as_at(listing_id, d)),
                date: d,
                amount_aud: foreign_income,
                foreign_tax_paid_aud: foreign_tax,
            });
        }
    }
    Ok(())
}

/// One AMMA statement's components, each converted at the statement's own
/// tax-year-end month.
fn amma_statement_row(
    ctx: &IncomeContext,
    row: &sqlx::sqlite::SqliteRow,
    listing_id: i64,
    d: NaiveDate,
) -> Result<AmmaStatementRow, sqlx::Error> {
    let currency: String = row.try_get("currency")?;
    let fx = &ctx.fx;
    let aud = |column: &str| tax_summary::aud_field(fx, row, column, &currency, d);
    Ok(AmmaStatementRow {
        amma_statement_id: row.try_get("id")?,
        listing_id,
        ticker: ctx.ticker_as_at(listing_id, d),
        tax_year_end_date: d,
        australian_interest_aud: aud("australian_interest")?,
        australian_dividends_unfranked_aud: aud("australian_dividends_unfranked")?,
        franked_dividends_aud: aud("franked_dividends")?,
        franking_credits_aud: aud("franking_credits")?,
        net_rent_aud: aud("net_rent")?,
        foreign_income_aud: aud("foreign_income")?,
        foreign_tax_credits_aud: aud("foreign_tax_credits")?,
        other_income_aud: aud("other_income")?,
        cgt_discount_gains_aud: aud("cgt_discount_gains")?,
        cgt_indexation_gains_aud: aud("cgt_indexation_gains")?,
        cgt_other_gains_aud: aud("cgt_other_gains")?,
        capital_losses_applied_aud: aud("capital_losses_applied")?,
        tax_deferred_amount: crate::infra::decimal::row_dec(row, "tax_deferred_amount")?,
        tax_free_amount: crate::infra::decimal::row_dec(row, "tax_free_amount")?,
        tfn_withholding_tax_aud: aud("tfn_withholding_tax")?,
    })
}

async fn push_interest_rows(
    conn: &mut sqlx::SqliteConnection,
    ctx: &IncomeContext,
    out: &mut IncomeSections,
) -> Result<(), sqlx::Error> {
    let (tax_year, fx) = (ctx.tax_year, &ctx.fx);
    let interest_rows = sqlx::query(
        "SELECT id, date_paid, amount, tfn_withholding_tax, foreign_source, foreign_tax_paid, \
                currency, source \
         FROM interest_income",
    )
    .fetch_all(&mut *conn)
    .await?;
    for row in &interest_rows {
        let date_paid: NaiveDate = row.try_get("date_paid")?;
        if tax_year_for(date_paid) != tax_year {
            continue;
        }
        let currency: String = row.try_get("currency")?;
        let foreign_source: bool = row.try_get("foreign_source")?;
        let amount = tax_summary::aud_field(fx, row, "amount", &currency, date_paid)?;
        let foreign_tax =
            tax_summary::aud_field(fx, row, "foreign_tax_paid", &currency, date_paid)?;
        out.interest.push(InterestIncomeRow {
            interest_income_id: row.try_get("id")?,
            date_paid,
            source: row.try_get("source")?,
            amount_aud: amount,
            foreign_source,
            foreign_tax_paid_aud: foreign_tax,
            tfn_withholding_tax_aud: tax_summary::aud_field(
                fx,
                row,
                "tfn_withholding_tax",
                &currency,
                date_paid,
            )?,
        });
        if foreign_source {
            out.foreign_income.push(ForeignIncomeRow {
                kind: "Foreign interest".to_string(),
                listing_id: None,
                ticker: None,
                date: date_paid,
                amount_aud: amount,
                foreign_tax_paid_aud: foreign_tax,
            });
        }
    }
    Ok(())
}

async fn push_ess_rows(
    conn: &mut sqlx::SqliteConnection,
    ctx: &IncomeContext,
    out: &mut IncomeSections,
) -> Result<(), sqlx::Error> {
    let (tax_year, fx) = (ctx.tax_year, &ctx.fx);
    let ess_rows = sqlx::query(
        "SELECT id, listing_id, taxing_point_date, taxed_upfront_eligible, \
                taxed_upfront_not_eligible, deferral_discount, pre_2009_cessation_discount, \
                foreign_source_discount, tfn_withholding, currency, aud_taxed_upfront_eligible, \
                aud_taxed_upfront_not_eligible, aud_deferral_discount, \
                aud_pre_2009_cessation_discount, aud_foreign_source_discount \
         FROM ess_statements",
    )
    .fetch_all(&mut *conn)
    .await?;
    for row in &ess_rows {
        let taxing_point: NaiveDate = row.try_get("taxing_point_date")?;
        if tax_year_for(taxing_point) != tax_year {
            continue;
        }
        let currency: String = row.try_get("currency")?;
        let listing_id: i64 = row.try_get("listing_id")?;
        // The discount components carry their own stored AUD figures (the
        // `aud_*` columns), so they convert through `aud_label`.
        let label = |column: &str| tax_summary::aud_label(fx, row, column, &currency, taxing_point);
        let foreign = label("foreign_source_discount")?;
        out.ess.push(EssIncomeRow {
            ess_statement_id: row.try_get("id")?,
            listing_id,
            ticker: ctx.ticker_as_at(listing_id, taxing_point),
            taxing_point_date: taxing_point,
            taxed_upfront_eligible_aud: label("taxed_upfront_eligible")?,
            taxed_upfront_not_eligible_aud: label("taxed_upfront_not_eligible")?,
            deferral_discount_aud: label("deferral_discount")?,
            pre_2009_cessation_discount_aud: label("pre_2009_cessation_discount")?,
            foreign_source_discount_aud: foreign,
            tfn_withholding_aud: tax_summary::aud_field(
                fx,
                row,
                "tfn_withholding",
                &currency,
                taxing_point,
            )?,
        });
        if foreign > Decimal::ZERO {
            out.foreign_income.push(ForeignIncomeRow {
                kind: "ESS foreign-source discount (memo)".to_string(),
                listing_id: Some(listing_id),
                ticker: Some(ctx.ticker_as_at(listing_id, taxing_point)),
                date: taxing_point,
                amount_aud: foreign,
                foreign_tax_paid_aud: Decimal::ZERO,
            });
        }
    }
    Ok(())
}

/// Deductible investment expenses.
async fn push_deduction_rows(
    conn: &mut sqlx::SqliteConnection,
    ctx: &IncomeContext,
    out: &mut IncomeSections,
) -> Result<(), sqlx::Error> {
    let (tax_year, fx) = (ctx.tax_year, &ctx.fx);
    let expense_rows = sqlx::query(
        "SELECT id, date_incurred, expense_type, amount, currency, listing_id, description \
         FROM investment_expenses",
    )
    .fetch_all(&mut *conn)
    .await?;
    for row in &expense_rows {
        let date_incurred: NaiveDate = row.try_get("date_incurred")?;
        if tax_year_for(date_incurred) != tax_year {
            continue;
        }
        let currency: String = row.try_get("currency")?;
        out.deductions.push(DeductionRow {
            investment_expense_id: row.try_get("id")?,
            date_incurred,
            expense_type: row.try_get("expense_type")?,
            amount_aud: tax_summary::aud_field(fx, row, "amount", &currency, date_incurred)?,
            listing_id: row.try_get("listing_id")?,
            description: row.try_get("description")?,
        });
    }
    Ok(())
}

// ---- tax summary ------------------------------------------------------

#[derive(Debug, Serialize)]
pub struct TaxSummaryLine {
    pub field: String,
    pub ato_label: String,
    pub value: serde_json::Value,
}

// ---- top level ----------------------------------------------------------

#[derive(Debug, Serialize)]
pub struct TaxReport {
    pub meta: TaxReportMeta,
    pub completeness: Completeness,
    pub disposals: DisposalsSection,
    pub cgt_summary: Option<net_capital_gain::CgtSummaryYear>,
    pub income: IncomeSections,
    pub tax_summary: Vec<TaxSummaryLine>,
}

pub async fn db_tax_report(pool: &SqlitePool, tax_year: i32) -> Result<TaxReport, sqlx::Error> {
    let (period_start, period_end) = period_for(tax_year);

    // Every franked dividend's holding-period-walk result, on its own
    // snapshot (see the module doc's note on why) — read before the main
    // transaction so `income_section` can attach each dividend row's
    // franking entitlement/denial status.
    let franking_alerts: HashMap<i64, franking_at_risk::FrankingAtRiskAlert> =
        franking_at_risk::db_franking_at_risk(pool)
            .await?
            .into_iter()
            .map(|a| (a.income_id, a))
            .collect();

    let mut tx = pool.begin().await?;
    let amma_missing_alerts = amma_missing(&mut tx, tax_year).await?;
    let disposals = disposals_section(&mut tx, tax_year).await?;
    let cgt_summary = net_capital_gain::db_cgt_summary_year(&mut tx, tax_year).await?;
    let income = income_section(&mut tx, tax_year, &franking_alerts).await?;
    let all_years_summary = tax_summary::db_tax_summary_on(&mut tx).await?;
    tx.commit().await?;

    // Non-blocking advisory checks, filtered to the year — each on its own
    // snapshot (see the module doc's note on why).
    let amit_cash_alerts: Vec<_> = amit_cash_cross_check::db_amit_cash_alerts(pool)
        .await?
        .into_iter()
        .filter(|a| a.tax_year == tax_year)
        .collect();
    let e4_alerts: Vec<_> = e4_cross_check::db_e4_alerts(pool)
        .await?
        .into_iter()
        .filter(|a| a.tax_year == tax_year)
        .collect();
    let amit_adjustment_alerts: Vec<_> =
        amit_adjustment_cross_check::db_amit_adjustment_alerts(pool)
            .await?
            .into_iter()
            .filter(|a| a.tax_year == tax_year)
            .collect();

    let completeness = Completeness {
        complete: amma_missing_alerts.is_empty()
            && amit_cash_alerts.is_empty()
            && e4_alerts.is_empty()
            && amit_adjustment_alerts.is_empty(),
        amma_missing: amma_missing_alerts,
        amit_cash_alerts,
        e4_alerts,
        amit_adjustment_alerts,
    };

    let summary_row = all_years_summary
        .into_iter()
        .find(|s| s.tax_year == tax_year);
    let tax_summary_lines = match &summary_row {
        Some(row) => {
            let value = serde_json::to_value(row).map_err(|e| sqlx::Error::Decode(e.into()))?;
            let obj = value.as_object().cloned().unwrap_or_default();
            tax_summary::CSV_HEADER
                .iter()
                .zip(tax_summary::CSV_ATO_LABELS.iter())
                .map(|(field, label)| TaxSummaryLine {
                    field: field.to_string(),
                    ato_label: (*label).to_string(),
                    value: obj.get(*field).cloned().unwrap_or(serde_json::Value::Null),
                })
                .collect()
        }
        None => Vec::new(),
    };

    Ok(TaxReport {
        meta: TaxReportMeta {
            tax_year,
            period_start,
            period_end,
            generated_at: Utc::now(),
            taxpayer_basis: super::TAXPAYER_BASIS.to_string(),
        },
        completeness,
        disposals,
        cgt_summary,
        income,
        tax_summary: tax_summary_lines,
    })
}

async fn tax_report_handler(
    State(pool): State<SqlitePool>,
    Json(req): Json<TaxReportRequest>,
) -> Result<Json<TaxReport>, ApiError> {
    db_tax_report(&pool, req.tax_year)
        .await
        .map(Json)
        .map_err(ApiError::from)
}

/// Every Australian financial year with any recorded fact touching a tax
/// figure (trades, income, interest, AMMA/ESS statements, investment
/// expenses) — for the UI's year dropdown, cheaper than pulling a full
/// report per year.
async fn db_tax_report_years(pool: &SqlitePool) -> Result<Vec<i32>, sqlx::Error> {
    let dates: Vec<NaiveDate> = sqlx::query_scalar(
        "SELECT date FROM trades \
         UNION SELECT date_paid FROM income \
         UNION SELECT date_paid FROM interest_income \
         UNION SELECT tax_year_end_date FROM amma_statements \
         UNION SELECT taxing_point_date FROM ess_statements \
         UNION SELECT date_incurred FROM investment_expenses",
    )
    .fetch_all(pool)
    .await?;
    let mut years: Vec<i32> = dates.into_iter().map(tax_year_for).collect();
    years.sort_unstable();
    years.dedup();
    Ok(years)
}

async fn years_handler(State(pool): State<SqlitePool>) -> Result<Json<Vec<i32>>, ApiError> {
    db_tax_report_years(&pool)
        .await
        .map(Json)
        .map_err(ApiError::from)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::entities::{interest_income, investment_expense, rba_fx_rate};
    use crate::test_support::{self, dec, ymd};

    async fn listing_amit(pool: &SqlitePool, id: i64, ticker: &str) {
        test_support::listing(id)
            .ticker(ticker)
            .name(ticker)
            .amit(true)
            .insert(pool)
            .await;
    }

    #[tokio::test]
    async fn empty_year_returns_zeroed_document_not_error() {
        let pool = test_support::test_pool().await;
        let report = db_tax_report(&pool, 2099).await.unwrap();
        assert_eq!(report.meta.tax_year, 2099);
        assert!(report.disposals.listings.is_empty());
        assert_eq!(report.disposals.totals.gain_loss_aud, Decimal::ZERO);
        assert!(report.cgt_summary.is_none());
        assert!(report.tax_summary.is_empty());
        assert!(report.completeness.complete);
    }

    /// The tax report's disposal schedule must never disagree with the
    /// realised-gains report over the same allocations — both totals and the
    /// per-parcel figures — since it's presenting the same computation, not a
    /// second one.
    #[tokio::test]
    async fn disposal_figures_equal_realised_gains_for_the_same_year() {
        let pool = test_support::test_pool().await;
        test_support::listing(1)
            .ticker("T1")
            .name("Test One")
            .insert(&pool)
            .await;
        test_support::buy(1, 1)
            .date(ymd(2023, 1, 1))
            .qty(dec("100"))
            .price(dec("10"))
            .insert(&pool)
            .await;
        test_support::sell(2, 1)
            .date(ymd(2024, 3, 1))
            .qty(dec("40"))
            .price(dec("15"))
            .insert(&pool)
            .await;
        test_support::allocate(&pool, 1, 2, 1, dec("40")).await;

        let realised = crate::reports::realised_gains::db_realised_gains(&pool)
            .await
            .unwrap();
        let expected = realised.iter().find(|r| r.sale_trade_id == 2).unwrap();

        let report = db_tax_report(&pool, 2024).await.unwrap();
        assert_eq!(report.disposals.totals.proceeds_aud, expected.proceeds);
        assert_eq!(report.disposals.totals.cost_base_aud, expected.cost_base);
        assert_eq!(
            report.disposals.totals.gain_loss_aud,
            expected.capital_gain_loss
        );

        assert_eq!(report.disposals.listings.len(), 1);
        let group = &report.disposals.listings[0];
        assert_eq!(group.ticker, "T1");
        assert_eq!(group.parcels.len(), 1);
        let row = &group.parcels[0];
        // 100 units @ $10, 40 sold @ $15, no brokerage: cost base $400,
        // proceeds $600, gain $200, held > 12 months so fully discountable.
        assert_eq!(row.adjusted_cost_base_aud, dec("400"));
        assert_eq!(row.proceeds_aud, dec("600"));
        assert_eq!(row.gain_loss_aud, dec("200"));
        assert!(row.discount_eligible);
        assert_eq!(row.cgt_discount_amount_aud, dec("100"));
        assert_eq!(row.gain_after_discount_aud, dec("100"));
        assert_eq!(row.acquisition_method, "Buy");
    }

    /// A rename mid-year: the disposal group's heading names the ticker as
    /// at its most recent disposal, and an income row's ticker resolves at
    /// its own date — so a printed document keeps reading the way the
    /// broker statement did, before and after the rename.
    #[tokio::test]
    async fn tickers_resolve_as_at_the_taxable_events_own_date_across_a_rename() {
        let pool = test_support::test_pool().await;
        test_support::listing(1)
            .ticker("LAAC")
            .name("Lithium Americas (Argentina) Corp")
            .insert(&pool)
            .await;
        test_support::buy(1, 1)
            .date(ymd(2023, 1, 1))
            .qty(dec("100"))
            .price(dec("10"))
            .insert(&pool)
            .await;
        // A dividend paid before the rename.
        test_support::income(1, 1, ymd(2024, 1, 5))
            .with(|i| i.franked_amount = dec("50"))
            .insert(&pool)
            .await;
        // Renamed mid-year: LAAC -> LAR (the listing's current ticker).
        crate::entities::listing_rename::db_rename(
            &pool,
            1,
            &crate::entities::listing_rename::RenameBody {
                effective_date: ymd(2024, 3, 1),
                ticker: "LAR".to_string(),
                exchange_mic: None,
                name: None,
                price_symbol: None,
                note: None,
            },
        )
        .await
        .unwrap();
        // A sale after the rename.
        test_support::sell(2, 1)
            .date(ymd(2024, 5, 1))
            .qty(dec("10"))
            .price(dec("15"))
            .insert(&pool)
            .await;
        test_support::allocate(&pool, 1, 2, 1, dec("10")).await;

        let report = db_tax_report(&pool, 2024).await.unwrap();
        // The dividend, paid before the rename, prints the pre-rename ticker.
        assert_eq!(report.income.dividends.len(), 1);
        assert_eq!(report.income.dividends[0].ticker, "LAAC");
        // The disposal group, sold after the rename, prints the new ticker.
        assert_eq!(report.disposals.listings.len(), 1);
        assert_eq!(report.disposals.listings[0].ticker, "LAR");
    }

    /// The CGT-summary section's ATO-worksheet lines must reconcile back to
    /// `NetCapitalGainYear` for the same year, both directly (net capital
    /// gain, concession amount, carried-forward loss) and via the
    /// discount-eligible split (`long_term_gains` +
    /// `amma_discount_gains_grossed_up` == `discount_eligible_gains`).
    #[tokio::test]
    async fn cgt_summary_reconciles_to_net_capital_gain_year() {
        let pool = test_support::test_pool().await;
        test_support::listing(1)
            .ticker("T1")
            .name("Test One")
            .insert(&pool)
            .await;
        test_support::buy(1, 1)
            .date(ymd(2023, 1, 1))
            .qty(dec("100"))
            .price(dec("10"))
            .insert(&pool)
            .await;
        test_support::sell(2, 1)
            .date(ymd(2024, 3, 1))
            .qty(dec("40"))
            .price(dec("15"))
            .insert(&pool)
            .await;
        test_support::allocate(&pool, 1, 2, 1, dec("40")).await;

        let years = crate::reports::net_capital_gain::db_net_capital_gain(&pool)
            .await
            .unwrap();
        let expected = years.iter().find(|y| y.tax_year == 2024).unwrap();

        let report = db_tax_report(&pool, 2024).await.unwrap();
        let summary = report.cgt_summary.expect("the year has recorded activity");
        assert_eq!(summary.short_term_gains, expected.other_gains);
        assert_eq!(
            summary.long_term_gains + summary.amma_discount_gains_grossed_up,
            expected.discount_eligible_gains
        );
        assert_eq!(summary.net_capital_gain, expected.net_capital_gain);
        assert_eq!(summary.cgt_concession_amount, expected.cgt_discount);
        assert_eq!(
            summary.capital_loss_carried_forward,
            expected.capital_loss_carried_forward
        );
        assert_eq!(
            summary.capital_loss_brought_forward,
            expected.capital_loss_brought_forward
        );
    }

    /// Every AUD figure in the income section's per-record rows must sum to
    /// exactly the year's `TaxYearSummary` line — the income section is a
    /// drilldown into that total, never a second computation of it.
    #[tokio::test]
    async fn income_sections_sum_to_tax_year_summary() {
        let pool = test_support::test_pool().await;
        test_support::listing(1)
            .ticker("T1")
            .name("Test One")
            .insert(&pool)
            .await;
        test_support::income(1, 1, ymd(2024, 2, 1))
            .with(|i| {
                i.franked_amount = dec("100");
                i.unfranked_amount = dec("50");
            })
            .insert(&pool)
            .await;
        interest_income::db_upsert(
            &pool,
            &interest_income::InterestIncome {
                id: 1,
                date_paid: ymd(2024, 2, 1),
                amount: dec("20"),
                tfn_withholding_tax: Decimal::ZERO,
                foreign_source: false,
                foreign_tax_paid: Decimal::ZERO,
                currency: "AUD".to_string(),
                source: None,
                holding_account_id: None,
            },
        )
        .await
        .unwrap();
        investment_expense::db_upsert(
            &pool,
            &investment_expense::InvestmentExpense {
                id: 1,
                date_incurred: ymd(2024, 2, 1),
                expense_type: investment_expense::ExpenseType::ManagementFee,
                amount: dec("5"),
                gross_amount: None,
                deductible_percentage: None,
                currency: "AUD".to_string(),
                description: None,
                listing_id: None,
                holding_account_id: None,
            },
        )
        .await
        .unwrap();

        let summary_rows = crate::reports::tax_summary::db_tax_summary(&pool)
            .await
            .unwrap();
        let summary = summary_rows.iter().find(|s| s.tax_year == 2024).unwrap();

        let report = db_tax_report(&pool, 2024).await.unwrap();
        let dividends_total: Decimal = report
            .income
            .dividends
            .iter()
            .map(|r| r.franked_amount_aud + r.unfranked_amount_aud)
            .sum();
        assert_eq!(dividends_total, summary.dividends_assessable);
        let interest_total: Decimal = report
            .income
            .interest
            .iter()
            .filter(|r| !r.foreign_source)
            .map(|r| r.amount_aud)
            .sum();
        assert_eq!(interest_total, summary.interest_income);
        let deductions_total: Decimal = report.income.deductions.iter().map(|r| r.amount_aud).sum();
        assert_eq!(deductions_total, summary.deductions_total);
    }

    /// A foreign company's dividend (e.g. a US-listed RSU holding like ICE)
    /// is entered with only `foreign_source_income` set — no Item-11
    /// content — so it must not print as an all-zero row in the Dividend
    /// income table; it still appears in Foreign income, and a genuine
    /// (Australian, franked) dividend in the same year still prints.
    #[tokio::test]
    async fn foreign_only_income_row_excluded_from_dividends_table() {
        let pool = test_support::test_pool().await;
        test_support::listing(1)
            .ticker("ICE")
            .name("Intercontinental Exchange")
            .currency("USD")
            .insert(&pool)
            .await;
        test_support::listing(2)
            .ticker("T1")
            .name("Test One")
            .insert(&pool)
            .await;
        rba_fx_rate::db_import_rate(&pool, "USD", "2024-03", dec("0.65"))
            .await
            .unwrap();
        test_support::income(1, 1, ymd(2024, 3, 31))
            .with(|i| {
                i.currency = "USD".to_string();
                i.foreign_source_income = dec("22.42");
                i.foreign_tax_paid = dec("3.36");
            })
            .insert(&pool)
            .await;
        test_support::income(2, 2, ymd(2024, 3, 31))
            .with(|i| {
                i.franked_amount = dec("70");
                i.franking_credits = dec("30");
            })
            .insert(&pool)
            .await;

        let report = db_tax_report(&pool, 2024).await.unwrap();
        assert_eq!(report.income.dividends.len(), 1);
        assert_eq!(report.income.dividends[0].ticker, "T1");
        assert_eq!(report.income.foreign_income.len(), 1);
        assert_eq!(
            report.income.foreign_income[0].ticker.as_deref(),
            Some("ICE")
        );
    }

    /// The holdings-based completeness check must fire for an AMIT fund held
    /// all year with *no* cash rows at all — the gap the existing
    /// (cash-driven) `amit_cash_cross_check` documents it cannot catch — and
    /// clear once the statement is entered.
    #[tokio::test]
    async fn amma_missing_is_holdings_based_and_clears_once_entered() {
        let pool = test_support::test_pool().await;
        listing_amit(&pool, 1, "AMT").await;
        test_support::buy(1, 1)
            .date(ymd(2023, 1, 1))
            .qty(dec("100"))
            .price(dec("10"))
            .insert(&pool)
            .await;

        let report = db_tax_report(&pool, 2024).await.unwrap();
        assert!(!report.completeness.complete);
        assert_eq!(report.completeness.amma_missing.len(), 1);
        assert_eq!(report.completeness.amma_missing[0].listing_id, 1);
        // No cash rows at all, so the existing cross-check has nothing to
        // flag — this is exactly the gap the holdings-based check closes.
        assert!(report.completeness.amit_cash_alerts.is_empty());

        test_support::amma(1, 1).insert(&pool).await;
        let report2 = db_tax_report(&pool, 2024).await.unwrap();
        assert!(report2.completeness.amma_missing.is_empty());
        assert!(report2.completeness.complete);
    }

    /// An AMMA statement whose per-parcel AMIT adjustments are missing drops
    /// `complete` to false and is listed — the gap distorts the disposal
    /// schedule's cost base, the report's central figure — and generating
    /// them clears it. Filtered to the report's year like the other two
    /// cross-checks.
    #[tokio::test]
    async fn amit_adjustment_gap_is_flagged_and_clears_once_generated() {
        let pool = test_support::test_pool().await;
        listing_amit(&pool, 1, "AMT").await;
        test_support::buy(1, 1)
            .date(ymd(2023, 8, 1))
            .qty(dec("100"))
            .price(dec("10"))
            .insert(&pool)
            .await;
        test_support::amma(1, 1)
            .units(dec("100"))
            .cost_base_adjustment(dec("0.05"))
            .with(|a| a.tax_year_end_date = ymd(2024, 6, 30))
            .insert(&pool)
            .await;

        let report = db_tax_report(&pool, 2024).await.unwrap();
        assert!(!report.completeness.complete);
        assert_eq!(report.completeness.amit_adjustment_alerts.len(), 1);
        assert_eq!(
            report.completeness.amit_adjustment_alerts[0].amma_statement_id,
            1
        );
        // The AMMA statement itself is entered, so the other checks are quiet
        // — this is the gap only the set-level check sees.
        assert!(report.completeness.amma_missing.is_empty());

        // Another year's report doesn't carry it.
        let other = db_tax_report(&pool, 2025).await.unwrap();
        assert!(other.completeness.amit_adjustment_alerts.is_empty());

        crate::entities::amit_adjustment_generation::db_generate(
            &pool,
            1,
            &crate::entities::amit_adjustment_generation::GenerateBody::default(),
        )
        .await
        .unwrap();
        let cleared = db_tax_report(&pool, 2024).await.unwrap();
        assert!(cleared.completeness.amit_adjustment_alerts.is_empty());
        assert!(cleared.completeness.complete);
    }

    /// A listing bought and fully sold before the requested year (nothing
    /// held during it) is not flagged, even with no AMMA statement at all.
    #[tokio::test]
    async fn amma_missing_ignores_a_listing_not_held_during_the_year() {
        let pool = test_support::test_pool().await;
        listing_amit(&pool, 1, "AMT").await;
        test_support::buy(1, 1)
            .date(ymd(2021, 1, 1))
            .qty(dec("100"))
            .price(dec("10"))
            .insert(&pool)
            .await;
        test_support::sell(2, 1)
            .date(ymd(2022, 1, 1))
            .qty(dec("100"))
            .price(dec("12"))
            .insert(&pool)
            .await;
        test_support::allocate(&pool, 1, 2, 1, dec("100")).await;

        let report = db_tax_report(&pool, 2024).await.unwrap();
        assert!(report.completeness.amma_missing.is_empty());
        assert!(report.completeness.complete);
    }

    #[tokio::test]
    async fn years_handler_lists_every_year_with_a_recorded_fact() {
        let pool = test_support::test_pool().await;
        test_support::listing(1)
            .ticker("T1")
            .name("Test One")
            .insert(&pool)
            .await;
        test_support::buy(1, 1)
            .date(ymd(2022, 8, 1)) // FY2023
            .insert(&pool)
            .await;
        test_support::income(1, 1, ymd(2024, 3, 1)) // FY2024
            .insert(&pool)
            .await;

        let years = db_tax_report_years(&pool).await.unwrap();
        assert_eq!(years, vec![2023, 2024]);
    }
}
