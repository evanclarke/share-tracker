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
//! The disposal schedule's money figures are **rounded to the cent** here,
//! and every subtotal and grand total is the sum of those rounded figures
//! (see [`DisposalParcelRow::round_money_to_cents`], SCENARIOS W-d) — this is
//! a document that gets printed and hand-checked, so a column has to add up
//! on the page. It is the one report that rounds its own figures; every other
//! one answers the exact decimal and lets the screen round it for display.
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
use crate::domain::deduction_destination::{DeductionDestination, DeductionRouting};
use crate::domain::listing_identity::RenameHistory;
use crate::domain::tax_year::tax_year_for;
use crate::entities::corporate_action::{RocEvent, SplitEvent};
use crate::entities::income::{Income, IncomeType};
use crate::entities::investment_expense::ExpenseType;
use crate::entities::listing;
use crate::entities::trade::{Trade, TradeType};
use crate::infra::decimal::{parse_dec, to_cents};
use crate::infra::fx::FxRates;
use crate::infra::http::ApiError;
use crate::reports::realised_gains::DisposalSource;
use crate::reports::{
    activity, amit_adjustment_cross_check, amit_cash_cross_check, e4_cross_check, franking_at_risk,
    net_capital_gain, realised_gains, rollover_consistency, tax_summary,
};
use axum::{
    Json, Router,
    extract::State,
    routing::{get, post},
};
use chrono::{NaiveDate, Utc};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use sqlx::{FromRow, Row, SqlitePool};
use std::collections::{HashMap, HashSet};

pub fn router() -> Router<SqlitePool> {
    Router::new()
        .route("/reports/tax-report/years", get(years_handler))
        .route("/reports/tax-report", post(tax_report_handler))
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
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

/// The `tax_year` values this report accepts, each naming the calendar year
/// its Australian financial year *ends* in (2026 = 1 July 2025 – 30 June 2026).
///
/// The bounds are deliberately far wider than anything anyone will ask for,
/// because refusing a legitimate year is the worse failure of the two: this
/// system holds parcels acquired decades ago (the ATO acceptance tests run
/// years dated 1998 and 2001), and it reports the financial year in progress
/// and — for a draft or a projection — years beyond it. So the floor sits well
/// below the first year CGT can reach (20 September 1985, inside FY1986; and
/// only *trade* dates are floored there — income, interest and expense rows
/// carry no such limit), and the ceiling a thousand years past the year in
/// progress. What the range does exclude is input that is not a financial year
/// at all: `0`, a negative year, or a value `chrono` cannot build a date from,
/// which used to panic the handler (SCENARIOS P-02). Both bounds are far
/// inside `chrono`'s own ±262,143-year limit, and that is what makes
/// [`period_for`] total.
const MIN_TAX_YEAR: i32 = 1900;
const MAX_TAX_YEAR: i32 = 2999;

/// A `tax_year` checked against [`MIN_TAX_YEAR`]..=[`MAX_TAX_YEAR`]. It is the
/// only way to reach [`period_for`], so a year `chrono` cannot represent can
/// never get there: the panic SCENARIOS P-02 found is unreachable by
/// construction rather than merely unlikely.
#[derive(Clone, Copy, Debug)]
struct TaxYear(i32);

impl TaxYear {
    fn new(tax_year: i32) -> Result<Self, ApiError> {
        if !(MIN_TAX_YEAR..=MAX_TAX_YEAR).contains(&tax_year) {
            return Err(ApiError::Unprocessable(format!(
                "tax_year {tax_year} is out of range — give the calendar year the Australian \
                 financial year ends in, between {MIN_TAX_YEAR} and {MAX_TAX_YEAR} \
                 (e.g. 2026 for 1 July 2025 – 30 June 2026)"
            )));
        }
        Ok(Self(tax_year))
    }

    fn year(self) -> i32 {
        self.0
    }
}

fn period_for(tax_year: TaxYear) -> (NaiveDate, NaiveDate) {
    let year = tax_year.year();
    // Infallible: a `TaxYear` cannot hold a year outside MIN_TAX_YEAR..=
    // MAX_TAX_YEAR, so both dates are well inside chrono's range and neither
    // `from_ymd_opt` can be `None` (SCENARIOS P-02).
    (
        NaiveDate::from_ymd_opt(year - 1, 7, 1).expect("in range by TaxYear::new"),
        NaiveDate::from_ymd_opt(year, 6, 30).expect("in range by TaxYear::new"),
    )
}

// ---- completeness ---------------------------------------------------------

#[derive(Debug, Serialize)]
pub struct AmmaMissingAlert {
    pub listing_id: i64,
    pub ticker: String,
    /// The holding account the units were held in. A registry issues one AMMA
    /// statement per holder account, so a fund held in two accounts needs two
    /// statements and each account is asked for its own (the same rule the
    /// [AMIT cash cross-check](super::amit_cash_cross_check) applies).
    pub holding_account_id: i64,
}

#[derive(Debug, Serialize)]
pub struct Completeness {
    pub complete: bool,
    /// AMIT listings held at any point in the year, per holding account, with
    /// no AMMA statement covering that account and year — holdings-based, so
    /// (unlike [`amit_cash_cross_check`](super::amit_cash_cross_check), whose
    /// own doc comment names the gap) this also catches a fund-year where no
    /// cash rows were entered at all.
    pub amma_missing: Vec<AmmaMissingAlert>,
    pub amit_cash_alerts: Vec<amit_cash_cross_check::AmitCashAlert>,
    pub e4_alerts: Vec<e4_cross_check::E4CrossCheckAlert>,
    /// AMMA statements for this year whose per-parcel AMIT adjustment set
    /// does not reconcile to the statement. An adjustment gap distorts the
    /// disposal schedule's cost base — this report's central figure — so it
    /// belongs to the gate the completeness section is.
    pub amit_adjustment_alerts: Vec<amit_adjustment_cross_check::AmitAdjustmentAlert>,
    /// Rollovers whose stored carried cost base no longer matches what the
    /// units they consumed are worth today, for the same reason: a replacement
    /// parcel's cost base *is* the disposal schedule's cost base for every unit
    /// still descending from it. **Not** filtered to the year — a rollover from
    /// an earlier year is exactly the one whose stale figure this year's
    /// disposals are costed on (SCENARIOS N-06).
    pub rollover_alerts: Vec<rollover_consistency::RolloverAlert>,
}

/// Every (AMIT listing, holding account) with a non-zero opening balance at
/// the start of the year, or any Buy/DRP trade dated within it — i.e. held at
/// some point during the year — that has no `amma_statements` row for that
/// account whose `tax_year_end_date` falls in the year. A simple net-units
/// walk (Buy/DRP minus Sell quantities, not cost-base aware): good enough for
/// a held/not-held flag, not a financial figure.
///
/// The account is part of the key because the statement is: a registry issues
/// one AMMA statement per holder account, and one account's statement
/// attributes only its own units — so a fund held in two accounts with a
/// statement for one of them is exactly the gap this section exists to catch
/// (SCENARIOS F-03, F-08).
async fn amma_missing(
    conn: &mut sqlx::SqliteConnection,
    tax_year: TaxYear,
) -> Result<Vec<AmmaMissingAlert>, sqlx::Error> {
    let (start, end) = period_for(tax_year);

    // Listings that are an AMIT *for this year*: one that converted part-way
    // through a holding has no AMMA statement for its earlier years and must
    // not be asked for one (SCENARIOS F-23).
    let tickers: HashMap<i64, String> =
        sqlx::query("SELECT id, ticker, amit, amit_from FROM listings WHERE amit")
            .fetch_all(&mut *conn)
            .await?
            .into_iter()
            .map(|r| {
                Ok::<_, sqlx::Error>((
                    r.try_get("id")?,
                    r.try_get("ticker")?,
                    listing::amit_in_tax_year(
                        r.try_get("amit")?,
                        r.try_get("amit_from")?,
                        tax_year.year(),
                    ),
                ))
            })
            .collect::<Result<Vec<_>, _>>()?
            .into_iter()
            .filter(|(_, _, in_year)| *in_year)
            .map(|(id, ticker, _)| (id, ticker))
            .collect();
    if tickers.is_empty() {
        return Ok(vec![]);
    }

    let trade_rows = sqlx::query(
        "SELECT listing_id, holding_account_id, trade_type, date, quantity FROM trades \
         WHERE listing_id IN (SELECT id FROM listings WHERE amit) \
           AND trade_type IN ('Buy', 'DRP', 'Sell') \
         ORDER BY listing_id, holding_account_id, date",
    )
    .fetch_all(&mut *conn)
    .await?;

    let mut opening: HashMap<(i64, i64), Decimal> = HashMap::new();
    let mut bought_in_year: HashSet<(i64, i64)> = HashSet::new();
    for row in &trade_rows {
        let key: (i64, i64) = (
            row.try_get("listing_id")?,
            row.try_get("holding_account_id")?,
        );
        // The SQL above matches every listing that is an AMIT *now*; only the
        // ones that were one in this year can be missing a statement for it
        // (SCENARIOS F-23), and `tickers` is that per-year set.
        if !tickers.contains_key(&key.0) {
            continue;
        }
        let trade_type: TradeType = row.try_get("trade_type")?;
        let date: NaiveDate = row.try_get("date")?;
        let qty = crate::infra::decimal::row_dec(row, "quantity")?;
        let signed = match trade_type {
            TradeType::Buy | TradeType::DRP => qty,
            TradeType::Sell => -qty,
        };
        if date < start {
            *opening.entry(key).or_insert(Decimal::ZERO) += signed;
        } else if date <= end && trade_type.is_acquisition() {
            bought_in_year.insert(key);
        }
    }

    let covered: HashSet<(i64, i64)> = sqlx::query(
        "SELECT listing_id, holding_account_id FROM amma_statements \
         WHERE tax_year_end_date BETWEEN ? AND ?",
    )
    .bind(start)
    .bind(end)
    .fetch_all(&mut *conn)
    .await?
    .into_iter()
    .map(|r| Ok::<_, sqlx::Error>((r.try_get("listing_id")?, r.try_get("holding_account_id")?)))
    .collect::<Result<_, _>>()?;

    // Every (listing, account) the walk saw, held at some point in the year
    // and not covered — sorted by ticker, then account, so the section reads
    // in the same order as the cross-check alerts beside it.
    let mut held: Vec<(i64, i64)> = opening
        .iter()
        .filter(|(_, q)| **q > Decimal::ZERO)
        .map(|(key, _)| *key)
        .chain(bought_in_year.iter().copied())
        .filter(|key| !covered.contains(key))
        .collect();
    held.sort_unstable_by_key(|(listing_id, account_id)| {
        (tickers.get(listing_id).cloned(), *account_id, *listing_id)
    });
    held.dedup();

    Ok(held
        .into_iter()
        .map(|(listing_id, holding_account_id)| AmmaMissingAlert {
            listing_id,
            ticker: tickers.get(&listing_id).cloned().unwrap_or_default(),
            holding_account_id,
        })
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
    /// Add one **already cent-rounded** parcel row (see
    /// [`DisposalParcelRow::round_money_to_cents`]) — which is what makes every
    /// subtotal and grand total the exact sum of the figures printed above it
    /// (SCENARIOS W-d). Nothing else may add to a `DisposalTotals`.
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

impl DisposalParcelRow {
    /// Round this row's money figures to the cent (SCENARIOS W-d).
    ///
    /// The disposal schedule is printed and hand-checked, so its columns have
    /// to add up on the page: every figure here is rounded once, at source,
    /// and [`DisposalTotals::add`] then sums the rounded figures — so the
    /// subtotal under a column is exactly what a reader gets adding the column
    /// up, and the JSON API says the same number the page prints. (Rounding
    /// the rows and the exact totals *independently*, which is what the UI's
    /// per-cell display rounding did on its own, is what left a three-parcel
    /// BHP disposal's discount column printing 1652.17 over rows summing to
    /// 1652.18.) This report computes nothing new and nothing downstream
    /// consumes these rows, so rounding them cannot reach a tax figure: the
    /// [realised gains](super::realised_gains) and [net capital
    /// gain](super::net_capital_gain) reports the numbers come from are
    /// untouched and stay exact.
    ///
    /// **Money, and so rounded:** the six AUD figures below plus each itemised
    /// adjustment's `amount` — every figure the printed schedule shows as an
    /// amount, and the five that are totalled.
    ///
    /// **Deliberately left verbatim**, because none is a derived AUD amount:
    /// - `cost_base_per_unit_aud`, `proceeds_per_unit_aud` and an adjustment's
    ///   `per_unit` — derived *per-unit* figures, which `docs/API.md`'s
    ///   "Amounts round, rates don't" shows at 4+ decimal places precisely so
    ///   they are not read as cent amounts;
    /// - `buy_price`, `sale_price` — per-unit prices, the same rule;
    /// - `buy_brokerage`, `buy_gst_on_brokerage` — the contract note's own
    ///   figures in the trade's native currency, transcribed for hand-checking
    ///   against it (GST of 99.5c on $9.95 of brokerage is genuinely
    ///   sub-cent); neither is totalled, and rounding an entered fact would
    ///   misreport the source document;
    /// - `units`, `days_held`, the two FX rates and `currency` — quantities,
    ///   counts and rates, never money.
    fn round_money_to_cents(&mut self) {
        for figure in [
            &mut self.initial_cost_base_aud,
            &mut self.adjusted_cost_base_aud,
            &mut self.proceeds_aud,
            &mut self.gain_loss_aud,
            &mut self.cgt_discount_amount_aud,
            &mut self.gain_after_discount_aud,
        ] {
            *figure = to_cents(*figure);
        }
        for adjustment in &mut self.adjustments {
            adjustment.amount = to_cents(adjustment.amount);
        }
    }
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
    /// Each rights sale's `(currency, fx_rate)` — the issue's currency and the
    /// row's manual fallback rate, which is what `realised_gains` converted
    /// its proceeds at. A rights sale is not a trade, so it is not in
    /// `trades`, and the printed sell-side rate has to come from here.
    rights_sales: HashMap<i64, (String, Decimal)>,
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

    // The entity's own column list (`CrudEntity::COLUMNS`), not a copy of it:
    // a new trade column belongs in one place, and a stale copy here fails at
    // runtime with a missing column rather than at compile time.
    let all_trades: Vec<Trade> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT {} FROM trades",
        <Trade as crate::infra::http::CrudEntity>::COLUMNS
    )))
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

    // The issue's currency and each sale's own fallback rate, the pair
    // `realised_gains` converts a rights sale's proceeds with.
    let rights_sales: Vec<(i64, String, String)> = sqlx::query_as(
        "SELECT rs.id, ca.currency, rs.fx_rate \
         FROM rights_sales rs JOIN corporate_actions ca ON ca.id = rs.rights_action_id",
    )
    .fetch_all(&mut *conn)
    .await?;
    let rights_sales = rights_sales
        .into_iter()
        .map(|(id, currency, fx_rate)| {
            Ok::<_, sqlx::Error>((id, (currency, parse_dec("fx_rate", fx_rate)?)))
        })
        .collect::<Result<HashMap<_, _>, _>>()?;

    let amit_events =
        crate::entities::amit_adjustment::db_cost_base_reduction_events(&mut *conn, None).await?;
    let roc_events =
        crate::entities::corporate_action::db_return_of_capital_events(&mut *conn).await?;
    let split_events = crate::entities::corporate_action::db_share_split_events(&mut *conn).await?;
    let fx = FxRates::load(&mut *conn).await?;

    Ok(DisposalInputs {
        buys: buys.into_iter().map(|b| (b.id, b)).collect(),
        trades: all_trades.into_iter().map(|t| (t.id, t)).collect(),
        rights_sales,
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
            // The rate the *proceeds* were actually converted at, not the
            // month's published rate: `realised_gains` converts a Sell's
            // proceeds at the sale's own override (its deliberate spot rate
            // when set, else its `fx_rate` fallback where the month has no
            // ATO rate) and a rights sale's at that row's fallback. Printing
            // the monthly rate instead left the document's own arithmetic
            // irreconcilable — proceeds of A$40,000 beside a rate computing
            // A$29,411 (SCENARIOS M-01). Mirrors `buy_rate` below, and a test
            // pins each against the figure it sits next to.
            let sell_rate = match disposal.source {
                DisposalSource::Sell => sale_trade.and_then(|st| {
                    inputs
                        .fx
                        .resolve_rate(&st.currency, st.date, st.fx_override())
                        .ok()
                }),
                DisposalSource::RightsSale => inputs
                    .rights_sales
                    .get(&disposal.sale_trade_id)
                    .and_then(|(rights_currency, fx_rate)| {
                        inputs
                            .fx
                            .resolve_rate(
                                rights_currency,
                                disposal.sale_date,
                                crate::infra::fx::FxOverride::Fallback(*fx_rate),
                            )
                            .ok()
                    }),
            };
            let buy_rate = buy_trade.and_then(|bt| {
                inputs
                    .fx
                    .resolve_rate(&currency, p.acquisition_date, bt.fx_override())
                    .ok()
            });

            let mut row = DisposalParcelRow {
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
            };
            // Once, here, so every figure printed *and* every figure summed
            // into a subtotal is the same rounded one (SCENARIOS W-d).
            row.round_money_to_cents();
            row
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
    /// Memo only — the part of `unfranked_amount_aud` the trust declared to be
    /// conduit foreign income, printed so the entered figure ties back to the
    /// statement's own CFI line. Never added to a total: it is assessable to a
    /// resident *through* the unfranked amount it sits inside (see
    /// [`crate::entities::income::Income::conduit_foreign_income`]).
    pub conduit_foreign_income_aud: Decimal,
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
    /// Part C's foreign tax on the statement's **capital gains**, as the
    /// trustee reports it (grossed up). The [tax summary](tax_summary)
    /// apportions it to the assessable part before claiming it — see there —
    /// so this is the statement's figure, not the claimable one.
    pub foreign_tax_credits_capital_gains_aud: Decimal,
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
    /// Memo only — see [`TrustIncomeRow::conduit_foreign_income_aud`].
    pub conduit_foreign_income_aud: Decimal,
    pub franking_credits_aud: Decimal,
    pub lic_capital_gain_deduction_aud: Decimal,
    pub tfn_withholding_tax_aud: Decimal,
    /// `entitled`, `denied`, or `exempt_small_shareholder` — from
    /// [`franking_at_risk`]; `entitled` when the row isn't in its alert list.
    pub franking_status: String,
    pub franking_credits_denied_aud: Decimal,
}

/// A non-dividend income row: remuneration recorded against the holding it was
/// calculated from — a dividend equivalent on unvested RSUs (TD 2017/26,
/// SCENARIOS J-10). Printed in its own table, never among the dividends, since
/// the whole point of the kind is that the payment is not one.
#[derive(Debug, Serialize)]
pub struct EmploymentIncomeRow {
    pub income_id: i64,
    pub listing_id: i64,
    pub ticker: String,
    pub date_paid: NaiveDate,
    pub amount_aud: Decimal,
}

/// An [`IncomeType::OtherIncome`] row: ordinary income produced by the holding
/// that is not a distribution of it — a crypto staking reward or an
/// established-token airdrop, assessable at the tokens' market value on
/// receipt (QC 69950, `docs/ato/crypto-staking-airdrops.md`, SCENARIOS
/// L-03/L-04). Printed in its own table against **item 24**, never among the
/// dividends.
#[derive(Debug, Serialize)]
pub struct OtherIncomeRow {
    pub income_id: i64,
    pub listing_id: i64,
    pub ticker: String,
    pub date_paid: NaiveDate,
    pub amount_aud: Decimal,
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
    /// The attributed listing's ticker as at `date_incurred` (the same
    /// as-at naming every other listing-bearing row prints), `None` for a
    /// portfolio-wide expense. Without it the printed document loses the
    /// attribution entirely — and after a rename, demerger, or worthless
    /// declaration a bare `listing_id` is the only trace of which holding
    /// the fee was for (SCENARIOS H-07).
    pub ticker: Option<String>,
    /// Which question this deduction is claimed at, from the holding it is
    /// attributed to (`domain::deduction_destination`, SCENARIOS P-08) — the
    /// deductible *amount* is one figure, but a fee earning a trust
    /// distribution or foreign income is not claimed at D7/D8. Printed so the
    /// archived document says where each figure goes, not only what it was.
    pub destination: DeductionDestination,
    /// [`Self::destination`]'s label on the form year the report targets
    /// (`docs/ato/tax-return-labels-2026.md`).
    pub ato_label: String,
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
    /// Cash recorded against a holding that is not income *of* the holding —
    /// see [`EmploymentIncomeRow`]. Reported at item 1/2, not at item 11, and
    /// so kept out of `dividends` and out of every investment-income total.
    pub employment_income: Vec<EmploymentIncomeRow>,
    /// Ordinary income of the holding that is not a distribution of it — see
    /// [`OtherIncomeRow`]. Reported at item 24, so it is out of `dividends`
    /// but, unlike `employment_income`, inside the year's assessable income.
    pub other_income: Vec<OtherIncomeRow>,
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
        self.employment_income.sort_by_key(|r| r.date_paid);
        self.other_income.sort_by_key(|r| r.date_paid);
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
    tax_year: TaxYear,
    fx: FxRates,
    tickers: HashMap<i64, String>,
    renames: RenameHistory,
}

impl IncomeContext {
    async fn load(
        conn: &mut sqlx::SqliteConnection,
        tax_year: TaxYear,
    ) -> Result<Self, sqlx::Error> {
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
    tax_year: TaxYear,
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
    let (tax_year, fx) = (ctx.tax_year.year(), &ctx.fx);
    // An AMIT listing's cash rows are excluded: for an AMIT the AMMA
    // attribution is the only assessable record, and it prints below.
    //
    // The exclusion is per *year*, not per listing: a fund that converted to
    // an AMIT part-way through a holding was an ordinary trust before its
    // first AMIT income year, and those years' distributions print here
    // exactly like any other trust's — dropping them because the fund is an
    // AMIT *now* would leave the year's tax-summary income total with no rows
    // behind it (SCENARIOS F-23). `listing::amit_in_tax_year` is the shared
    // rule, applied exactly as `tax_summary::db_tax_summary_on` applies it.
    let income_rows: Vec<(Income, bool, Option<NaiveDate>)> = sqlx::query(
        "SELECT i.*, l.amit AS listing_amit, l.amit_from AS listing_amit_from \
         FROM income i JOIN listings l ON l.id = i.listing_id",
    )
    .fetch_all(&mut *conn)
    .await?
    .iter()
    .map(|row| {
        Ok::<_, sqlx::Error>((
            Income::from_row(row)?,
            row.try_get("listing_amit")?,
            row.try_get("listing_amit_from")?,
        ))
    })
    .collect::<Result<_, _>>()?;

    for (income, amit, amit_from) in &income_rows {
        if listing::amit_in_tax_year(*amit, *amit_from, tax_year_for(income.assessment_date())) {
            continue;
        }
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
        // The claimable 50%, so the column sums to the D8 line.
        let lic = aud(income.lic_capital_gain_deduction())?;
        let tfn = aud(income.tfn_withholding_tax)?;
        // Memo figure: converted like the rest so it reads on the same basis,
        // but it is part of `unfranked` and never totalled separately.
        let cfi = aud(income.conduit_foreign_income)?;

        if income.income_type == IncomeType::EmploymentIncome {
            // Remuneration, not a payment of the holding: its own table, and
            // out of every Item 11/13/20 list below. The write-time rule
            // leaves such a row only the cash in `unfranked_amount`, so
            // `unfranked` is the whole payment.
            out.employment_income.push(EmploymentIncomeRow {
                income_id,
                listing_id,
                ticker,
                date_paid,
                amount_aud: unfranked,
            });
        } else if income.income_type == IncomeType::OtherIncome {
            // Ordinary income at item 24, not a dividend: its own table, and
            // like the employment kind the write-time rule leaves the row only
            // the cash in `unfranked_amount`.
            out.other_income.push(OtherIncomeRow {
                income_id,
                listing_id,
                ticker,
                date_paid,
                amount_aud: unfranked,
            });
        } else if trust_income {
            out.trust_income.push(TrustIncomeRow {
                income_id,
                listing_id,
                ticker,
                date_paid,
                entitlement_date,
                franked_amount_aud: franked,
                unfranked_amount_aud: unfranked,
                conduit_foreign_income_aud: cfi,
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
                conduit_foreign_income_aud: cfi,
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
                a.net_rent, a.foreign_income, a.foreign_tax_credits, \
                a.foreign_tax_credits_capital_gains, a.other_income, \
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
        foreign_tax_credits_capital_gains_aud: aud("foreign_tax_credits_capital_gains")?,
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
    let (tax_year, fx) = (ctx.tax_year.year(), &ctx.fx);
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
    let (tax_year, fx) = (ctx.tax_year.year(), &ctx.fx);
    let ess_rows = sqlx::query(
        "SELECT id, listing_id, taxing_point_date, taxed_upfront_eligible, \
                taxed_upfront_not_eligible, deferral_discount, pre_2009_cessation_discount, \
                foreign_source_discount, tfn_withholding, currency, fx_rate, \
                aud_taxed_upfront_eligible, \
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
        // `aud_*` columns), so they convert through `aud_label` — and through
        // the statement's own stated `fx_rate` where the taxing-point month has
        // no RBA rate, exactly as the tax summary's totals do.
        let over = tax_summary::ess_fx_override(row)?;
        let label =
            |column: &str| tax_summary::aud_label(fx, row, column, &currency, taxing_point, over);
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
            tfn_withholding_aud: tax_summary::aud_field_with(
                fx,
                row,
                "tfn_withholding",
                &currency,
                taxing_point,
                over,
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
    let (tax_year, fx) = (ctx.tax_year.year(), &ctx.fx);
    let expense_rows = sqlx::query(
        "SELECT id, date_incurred, expense_type, amount, currency, listing_id, description \
         FROM investment_expenses",
    )
    .fetch_all(&mut *conn)
    .await?;
    // The same routing the tax summary's per-destination lines are cut by, on
    // this report's own read transaction — so a row's printed destination and
    // the summary line it is inside can never disagree (SCENARIOS P-08).
    let routing = DeductionRouting::load(&mut *conn).await?;
    for row in &expense_rows {
        let date_incurred: NaiveDate = row.try_get("date_incurred")?;
        if tax_year_for(date_incurred) != tax_year {
            continue;
        }
        let currency: String = row.try_get("currency")?;
        let listing_id: Option<i64> = row.try_get("listing_id")?;
        let expense_type: ExpenseType = row.try_get("expense_type")?;
        let destination = routing.destination(listing_id, expense_type, tax_year);
        out.deductions.push(DeductionRow {
            investment_expense_id: row.try_get("id")?,
            date_incurred,
            expense_type: row.try_get("expense_type")?,
            // To the cent, as the tax summary's own deduction lines are —
            // that report rounds each expense at the row so its two cuts
            // (by kind, by destination) agree, and this is the same row
            // printed on the archived document. Rounding here too is what
            // keeps the section a drilldown: these rows sum to the summary
            // line exactly (SCENARIOS W-d/W-f).
            amount_aud: crate::infra::decimal::to_cents(tax_summary::aud_field(
                fx,
                row,
                "amount",
                &currency,
                date_incurred,
            )?),
            listing_id,
            ticker: listing_id.map(|id| ctx.ticker_as_at(id, date_incurred)),
            destination,
            ato_label: destination.ato_label().to_string(),
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

/// Rejects a `tax_year` outside [`MIN_TAX_YEAR`]..=[`MAX_TAX_YEAR`] with a
/// `422` naming the range, before any date is built from it (SCENARIOS P-02).
pub async fn db_tax_report(pool: &SqlitePool, tax_year: i32) -> Result<TaxReport, ApiError> {
    let year = TaxYear::new(tax_year)?;
    let (period_start, period_end) = period_for(year);

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
    let amma_missing_alerts = amma_missing(&mut tx, year).await?;
    let disposals = disposals_section(&mut tx, tax_year).await?;
    let cgt_summary = net_capital_gain::db_cgt_summary_year(&mut tx, tax_year).await?;
    let income = income_section(&mut tx, year, &franking_alerts).await?;
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
    // Unfiltered, unlike the three above: a rollover's stale carried figure is
    // the cost base of every unit still descending from it, however many years
    // later this report's disposals are.
    let rollover_alerts = rollover_consistency::db_rollover_alerts(pool).await?;

    let completeness = Completeness {
        complete: amma_missing_alerts.is_empty()
            && amit_cash_alerts.is_empty()
            && e4_alerts.is_empty()
            && amit_adjustment_alerts.is_empty()
            && rollover_alerts.is_empty(),
        amma_missing: amma_missing_alerts,
        amit_cash_alerts,
        e4_alerts,
        amit_adjustment_alerts,
        rollover_alerts,
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
    db_tax_report(&pool, req.tax_year).await.map(Json)
}

/// Every Australian financial year this report has content for — the union of
/// everything it can report on, ascending and deduped (SCENARIOS
/// P-02/P-03/P-04):
///
/// - the CGT side, as [`net_capital_gain::db_cgt_years`] — realised disposals,
///   rights sales, the AMMA components and the E10/G1/C2 events, plus every
///   quiet year still carrying a capital loss forward. Reusing that walk is
///   the point: a second derivation of "which years have CGT content" could
///   offer a year whose `cgt_summary` then comes back `null`, or hide one that
///   has a whole worksheet behind it (a G1 excess or a rights sale against a
///   parcel bought years earlier used to leave only the *purchase* year on the
///   list);
/// - income by its **assessment date**, not `date_paid` — a trust distribution
///   with a 30 June entitlement date paid in July is assessed in the FY just
///   ended ([`Income::assessment_date`], the rule the tax summary and this
///   report's own income tables both apply), so `date_paid` offered a year
///   the distribution does not belong to and hid the one it does. There is no
///   SQL twin of that rule (unlike `Income::EX_OR_PAY_DATE_SQL`) and this read
///   does not need one: the rows are decoded and asked, exactly as
///   [`tax_summary`] does;
/// - the remaining facts, each bucketed by the date column that *is* its
///   assessment date — trades, interest, AMMA/ESS statements and investment
///   expenses.
///
/// The trade-off is cost: the CGT walk is the realised-gains read plus the
/// AMMA/E10/G1/C2 walks — roughly one `cgt_summary`'s worth of work — where
/// the old list was a single six-way `UNION` of date columns. That is
/// deliberate. The picker is a closed `<select>` and the *only* way to reach
/// the report from the UI, so a year missing from it is a year that cannot be
/// generated at all; one report-sized query behind a dropdown that opens once
/// per session is the cheaper failure.
///
/// Bounded to [`MIN_TAX_YEAR`]..=[`MAX_TAX_YEAR`] so the list and
/// [`TaxYear::new`] agree: the list never offers a year `POST
/// /reports/tax-report` would refuse `422`.
///
/// And bounded above at the financial year *in progress*
/// (`tax_year_for(today())`) — the last year a return could be being prepared
/// for, and the same bound `net_capital_gain` puts on its quiet-carry-forward
/// filler years (SCENARIOS S-10). A trade can no longer be dated in the future
/// (`trade::AmountsError::FutureDate`), but this list is a union over every
/// dated fact — an interest payment, an AMMA or ESS statement, an investment
/// expense, a distribution, and the CGT walk's own buckets, which `net_years`
/// emits "however dated" — so the picker keeps its own ceiling rather than
/// inheriting one write path's. A year past it has not begun: offering it
/// would render an annual tax document for a financial year that does not
/// exist yet.
async fn db_tax_report_years(pool: &SqlitePool) -> Result<Vec<i32>, sqlx::Error> {
    // One read transaction over every input, per the house rule for
    // multi-query reports: an interleaved write can't land a fact between the
    // CGT walk and the date reads and leave the list internally inconsistent.
    let mut tx = pool.begin().await?;
    let dates: Vec<NaiveDate> = sqlx::query_scalar(
        "SELECT date FROM trades \
         UNION SELECT date_paid FROM interest_income \
         UNION SELECT tax_year_end_date FROM amma_statements \
         UNION SELECT taxing_point_date FROM ess_statements \
         UNION SELECT date_incurred FROM investment_expenses",
    )
    .fetch_all(&mut *tx)
    .await?;
    let income_rows: Vec<Income> = sqlx::query_as("SELECT * FROM income")
        .fetch_all(&mut *tx)
        .await?;
    let cgt_years = net_capital_gain::db_cgt_years(&mut tx).await?;
    tx.commit().await?;

    let current = tax_year_for(crate::infra::date::today());
    let mut years: Vec<i32> = dates
        .into_iter()
        .chain(income_rows.iter().map(Income::assessment_date))
        .map(tax_year_for)
        .chain(cgt_years)
        .filter(|y| (MIN_TAX_YEAR..=MAX_TAX_YEAR).contains(y) && *y <= current)
        .collect();
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
    use axum::http::StatusCode;

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

    /// SCENARIOS O-03/O-04: a year in which nothing happened, following years
    /// of losses. The document is otherwise zeroed, but it must still state
    /// the loss the return carries forward at label 18V — so `cgt_summary` is
    /// `Some`, all zeros but for the brought-forward/carried-forward pair
    /// (`net_capital_gain::net_years` emits the quiet year's row, and this
    /// report reads that same walk).
    #[tokio::test]
    async fn a_quiet_year_still_reports_its_carried_forward_loss() {
        let pool = test_support::test_pool().await;
        test_support::listing(1)
            .ticker("T1")
            .name("Test One")
            .insert(&pool)
            .await;
        // FY2025: a $4,000 capital loss, nothing to offset it.
        test_support::buy(1, 1)
            .date(ymd(2024, 8, 1))
            .qty(dec("100"))
            .price(dec("50"))
            .insert(&pool)
            .await;
        test_support::sell(2, 1)
            .date(ymd(2025, 5, 1))
            .qty(dec("100"))
            .price(dec("10"))
            .insert(&pool)
            .await;
        test_support::allocate(&pool, 1, 2, 1, dec("100")).await;

        let report = db_tax_report(&pool, 2026).await.unwrap();
        let summary = report
            .cgt_summary
            .expect("the quiet year still carries a loss forward");
        assert_eq!(summary.tax_year, 2026);
        assert_eq!(summary.short_term_gains, Decimal::ZERO);
        assert_eq!(summary.long_term_gains, Decimal::ZERO);
        assert_eq!(summary.capital_losses_this_year, Decimal::ZERO);
        assert_eq!(summary.net_capital_gain, Decimal::ZERO);
        assert_eq!(summary.capital_loss_brought_forward, dec("4000"));
        assert_eq!(summary.capital_loss_carried_forward, dec("4000"));
        // Over HTTP, the surface the archived document is printed from.
        let body: serde_json::Value = crate::test_support::ApiClient::full(&pool)
            .post_json(
                "/reports/tax-report",
                &serde_json::json!({"tax_year": 2026}),
            )
            .await;
        assert_eq!(
            body["cgt_summary"]["capital_loss_carried_forward"],
            serde_json::json!("4000")
        );
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
            .date(ymd(2023, 1, 3))
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
            .date(ymd(2023, 1, 3))
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
            .date(ymd(2023, 1, 3))
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
                // The LIC's advised attributable part; D8 gets 50% of it.
                i.lic_capital_gain_amount = dec("50");
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
        // The LIC column is the *deduction* (50% of the advised amount), so it
        // sums to the D8 line rather than to what was entered.
        let lic_total: Decimal = report
            .income
            .dividends
            .iter()
            .map(|r| r.lic_capital_gain_deduction_aud)
            .sum();
        assert_eq!(lic_total, summary.lic_capital_gain_deduction);
        assert_eq!(
            lic_total,
            dec("25"),
            "50% of the advised $50 attributable part"
        );
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

    /// SCENARIOS W-f, the drilldown under the rounding. The tax summary's
    /// three *total* columns are now sums of the cent-rounded lines, and its
    /// deduction lines are sums of expenses rounded at their own row — so
    /// this checks the promise above still holds exactly on facts that fall
    /// on half cents: the income rows keep full precision and still sum to
    /// their (unrounded) summary lines, and the deduction rows are rounded
    /// here too, so they sum to the (rounded) deduction line rather than
    /// half a cent away from it.
    #[tokio::test]
    async fn income_sections_still_sum_to_the_summary_line_on_half_cent_figures() {
        let pool = test_support::test_pool().await;
        test_support::listing(1)
            .ticker("T1")
            .name("Test One")
            .insert(&pool)
            .await;
        test_support::income(1, 1, ymd(2024, 2, 1))
            .with(|i| i.franked_amount = dec("10.005"))
            .insert(&pool)
            .await;
        test_support::income(2, 1, ymd(2024, 3, 1))
            .with(|i| i.franked_amount = dec("10.005"))
            .insert(&pool)
            .await;
        for (id, amount) in [(1, "10.005"), (2, "10.005")] {
            investment_expense::db_upsert(
                &pool,
                &investment_expense::InvestmentExpense {
                    id,
                    date_incurred: ymd(2024, 2, 1),
                    expense_type: investment_expense::ExpenseType::ManagementFee,
                    amount: dec(amount),
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
        }

        let summary_rows = crate::reports::tax_summary::db_tax_summary(&pool)
            .await
            .unwrap();
        let summary = summary_rows.iter().find(|s| s.tax_year == 2024).unwrap();
        let report = db_tax_report(&pool, 2024).await.unwrap();

        // The income line keeps the half cents, and its rows sum to it.
        assert_eq!(summary.dividends_assessable, dec("20.01"));
        let dividends_total: Decimal = report
            .income
            .dividends
            .iter()
            .map(|r| r.franked_amount_aud + r.unfranked_amount_aud)
            .sum();
        assert_eq!(dividends_total, summary.dividends_assessable);
        assert_eq!(report.income.dividends[0].franked_amount_aud, dec("10.005"));

        // The deduction rows are at the cent, and sum to the line — which is
        // 20.02, the sum of the rounded rows, not the 20.01 the exact total
        // would have rounded to.
        assert_eq!(summary.deductions_total, dec("20.02"));
        assert_eq!(report.income.deductions[0].amount_aud, dec("10.01"));
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

    /// The archived document is the copy an accountant reads, so a dividend
    /// equivalent must not print among the dividends with a franking status
    /// (SCENARIOS J-10): it gets its own table, and the ordinary dividend
    /// beside it is unaffected.
    #[tokio::test]
    async fn employment_income_prints_in_its_own_table_not_among_the_dividends() {
        let pool = test_support::test_pool().await;
        test_support::listing(1).ticker("EMPA").insert(&pool).await;
        test_support::income(1, 1, ymd(2024, 3, 31))
            .with(|i| {
                i.unfranked_amount = dec("250");
                i.income_type = IncomeType::EmploymentIncome;
            })
            .insert(&pool)
            .await;
        test_support::income(2, 1, ymd(2024, 3, 31))
            .with(|i| {
                i.franked_amount = dec("70");
                i.franking_credits = dec("30");
            })
            .insert(&pool)
            .await;

        let report = db_tax_report(&pool, 2024).await.unwrap();
        assert_eq!(report.income.dividends.len(), 1);
        assert_eq!(report.income.dividends[0].income_id, 2);
        assert_eq!(report.income.employment_income.len(), 1);
        assert_eq!(report.income.employment_income[0].income_id, 1);
        assert_eq!(report.income.employment_income[0].ticker, "EMPA");
        assert_eq!(report.income.employment_income[0].amount_aud, dec("250"));
        // …and it reaches no Item 20 list either.
        assert!(report.income.foreign_income.is_empty());
    }

    /// A staking reward prints in its own item-24 table, not among the
    /// dividends and not in the employment-income table either (SCENARIOS
    /// L-03/L-04) — the archived document has to name the label the amount
    /// belongs at.
    #[tokio::test]
    async fn other_income_prints_in_its_own_item_24_table() {
        let pool = test_support::test_pool().await;
        test_support::listing(1)
            .crypto()
            .ticker("ETH")
            .name("Ether")
            .insert(&pool)
            .await;
        test_support::listing(2).ticker("BHP").insert(&pool).await;
        test_support::income(1, 1, ymd(2024, 3, 31))
            .with(|i| {
                i.unfranked_amount = dec("2000");
                i.income_type = IncomeType::OtherIncome;
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
        assert_eq!(report.income.dividends[0].income_id, 2);
        assert!(report.income.employment_income.is_empty());
        assert_eq!(report.income.other_income.len(), 1);
        assert_eq!(report.income.other_income[0].income_id, 1);
        assert_eq!(report.income.other_income[0].ticker, "ETH");
        assert_eq!(report.income.other_income[0].amount_aud, dec("2000"));
        assert!(report.income.foreign_income.is_empty());
    }

    /// The conduit-foreign-income memo is printed on the dividend and trust
    /// rows it was entered on, converted like every other figure — so a
    /// CFI-carrying row is no longer a line the report can't show (SCENARIOS
    /// G-03), while still totalling only through the unfranked amount it sits
    /// inside: the year's `dividends_assessable` is the two unfranked amounts,
    /// not those plus the memo.
    #[tokio::test]
    async fn conduit_foreign_income_prints_as_a_memo_column_and_is_not_double_counted() {
        let pool = test_support::test_pool().await;
        test_support::listing(1).ticker("T1").insert(&pool).await;
        test_support::income(1, 1, ymd(2024, 3, 31))
            .with(|i| {
                i.unfranked_amount = dec("100");
                i.conduit_foreign_income = dec("40");
            })
            .insert(&pool)
            .await;
        test_support::income(2, 1, ymd(2024, 3, 31))
            .with(|i| {
                i.trust_income = true;
                i.unfranked_amount = dec("50");
                i.conduit_foreign_income = dec("20");
            })
            .insert(&pool)
            .await;

        let report = db_tax_report(&pool, 2024).await.unwrap();
        assert_eq!(report.income.dividends.len(), 1);
        assert_eq!(
            report.income.dividends[0].conduit_foreign_income_aud,
            dec("40")
        );
        assert_eq!(report.income.dividends[0].unfranked_amount_aud, dec("100"));
        assert_eq!(report.income.trust_income.len(), 1);
        assert_eq!(
            report.income.trust_income[0].conduit_foreign_income_aud,
            dec("20")
        );
        // Counted once, through the unfranked amounts — not 210 — on each
        // row's own summary line: the company dividend at 11S/11T, the trust
        // distribution at 13U (SCENARIOS Z-f).
        let line = |field: &str| {
            report
                .tax_summary
                .iter()
                .find(|l| l.field == field)
                .unwrap_or_else(|| panic!("the year has a {field} line"))
        };
        assert_eq!(line("dividends_assessable").value, serde_json::json!("100"));
        assert_eq!(
            line("trust_income_unfranked").value,
            serde_json::json!("50")
        );
        assert_eq!(
            line("gross_assessable_investment_income").value,
            serde_json::json!("150")
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
            .date(ymd(2023, 1, 3))
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

    /// SCENARIOS F-03/F-08: the fund held in two accounts needs a statement
    /// for each — a registry issues one per holder account — so the
    /// completeness check asks per account, and the account with no statement
    /// keeps the year incomplete.
    #[tokio::test]
    async fn amma_missing_asks_each_holding_account_for_its_own_statement() {
        let pool = test_support::test_pool().await;
        listing_amit(&pool, 1, "AMT").await;
        crate::entities::holding_account::db_upsert(
            &pool,
            &crate::entities::holding_account::HoldingAccount {
                id: 2,
                name: "Second".to_string(),
            },
        )
        .await
        .unwrap();
        test_support::buy(1, 1)
            .date(ymd(2023, 1, 3))
            .qty(dec("100"))
            .price(dec("10"))
            .insert(&pool)
            .await;
        test_support::buy(2, 1)
            .date(ymd(2023, 2, 1))
            .qty(dec("40"))
            .price(dec("11"))
            .account(2)
            .insert(&pool)
            .await;

        let report = db_tax_report(&pool, 2024).await.unwrap();
        assert_eq!(
            report
                .completeness
                .amma_missing
                .iter()
                .map(|a| (a.listing_id, a.holding_account_id))
                .collect::<Vec<_>>(),
            vec![(1, 1), (1, 2)]
        );

        // The first account's statement clears only its own row.
        test_support::amma(1, 1).insert(&pool).await;
        let partly = db_tax_report(&pool, 2024).await.unwrap();
        assert_eq!(
            partly
                .completeness
                .amma_missing
                .iter()
                .map(|a| a.holding_account_id)
                .collect::<Vec<_>>(),
            vec![2]
        );
        assert!(!partly.completeness.complete);

        test_support::amma(2, 1)
            .with(|a| a.holding_account_id = 2)
            .insert(&pool)
            .await;
        let cleared = db_tax_report(&pool, 2024).await.unwrap();
        assert!(cleared.completeness.amma_missing.is_empty());
    }

    /// SCENARIOS F-23: the completeness gate follows the conversion year. A
    /// fund that became an AMIT for FY2025 needs no AMMA statement for FY2024
    /// — that year it was an ordinary trust, reported from its income rows —
    /// so the earlier year is complete without one.
    #[tokio::test]
    async fn amma_missing_ignores_years_before_the_fund_became_an_amit() {
        let pool = test_support::test_pool().await;
        test_support::listing(1)
            .ticker("AMT")
            .amit_from(ymd(2024, 7, 1))
            .insert(&pool)
            .await;
        test_support::buy(1, 1)
            .date(ymd(2023, 1, 3))
            .qty(dec("100"))
            .price(dec("10"))
            .insert(&pool)
            .await;

        let before = db_tax_report(&pool, 2024).await.unwrap();
        assert!(before.completeness.amma_missing.is_empty());
        assert!(before.completeness.complete);

        // FY2025, the first AMIT year, does want one.
        let after = db_tax_report(&pool, 2025).await.unwrap();
        assert_eq!(after.completeness.amma_missing.len(), 1);
        assert_eq!(after.completeness.amma_missing[0].listing_id, 1);
    }

    /// The same rule, with a second AMIT fund in the year so the completeness
    /// section's early-out (no AMIT-in-year listings at all) cannot be what
    /// makes the pre-conversion year clean: the converted fund must be
    /// dropped from the net-units walk itself, not merely from the ticker
    /// map, or it is flagged as missing a statement it could never have
    /// (SCENARIOS F-23).
    #[tokio::test]
    async fn amma_missing_ignores_a_converted_fund_beside_a_lifelong_amit() {
        let pool = test_support::test_pool().await;
        test_support::listing(1)
            .ticker("CONV")
            .name("CONV")
            .amit_from(ymd(2024, 7, 1))
            .insert(&pool)
            .await;
        listing_amit(&pool, 2, "AMT").await;
        test_support::buy(1, 1)
            .date(ymd(2023, 1, 3))
            .qty(dec("100"))
            .price(dec("10"))
            .insert(&pool)
            .await;
        test_support::buy(2, 2)
            .date(ymd(2023, 1, 3))
            .qty(dec("100"))
            .price(dec("10"))
            .insert(&pool)
            .await;

        // FY2024: only the lifelong AMIT is asked for a statement.
        let before = db_tax_report(&pool, 2024).await.unwrap();
        assert_eq!(before.completeness.amma_missing.len(), 1);
        assert_eq!(before.completeness.amma_missing[0].listing_id, 2);
        assert_eq!(before.completeness.amma_missing[0].ticker, "AMT");

        // FY2025: both are AMITs for the year, so both are.
        let after = db_tax_report(&pool, 2025).await.unwrap();
        let flagged: Vec<i64> = after
            .completeness
            .amma_missing
            .iter()
            .map(|a| a.listing_id)
            .collect();
        assert_eq!(flagged, vec![2, 1]);
    }

    /// SCENARIOS P-01: the printed document must carry the rows behind every
    /// figure it totals. A fund that converted to an AMIT was an ordinary
    /// trust before its first AMIT income year, so the earlier years'
    /// distributions are assessable and the tax summary counts them — while
    /// this section used to drop them on a flat `NOT l.amit`, printing a
    /// four-figure income total with an empty table behind it. The AMIT
    /// year's own cash row stays excluded, as always.
    #[tokio::test]
    async fn a_converted_funds_pre_amit_income_prints_behind_its_tax_summary_line() {
        let pool = test_support::test_pool().await;
        test_support::listing(1)
            .ticker("VDHG")
            .name("VDHG")
            .amit_from(ymd(2024, 7, 1))
            .insert(&pool)
            .await;
        // FY2023, an ordinary trust distribution with its franking credits.
        test_support::income(1, 1, ymd(2023, 2, 15))
            .with(|i| {
                i.trust_income = true;
                i.franked_amount = dec("600");
                i.unfranked_amount = dec("400");
                i.franking_credits = dec("257.14");
            })
            .insert(&pool)
            .await;
        // FY2025, the first AMIT year: cash only, assessable via the AMMA.
        test_support::income(2, 1, ymd(2025, 2, 15))
            .with(|i| {
                i.trust_income = true;
                i.unfranked_amount = dec("400");
            })
            .insert(&pool)
            .await;

        let summary = |report: &TaxReport, field: &str| -> Decimal {
            report
                .tax_summary
                .iter()
                .find(|l| l.field == field)
                .unwrap_or_else(|| panic!("no {field} line"))
                .value
                .as_str()
                .expect("a decimal string")
                .parse()
                .expect("a decimal")
        };

        let before = db_tax_report(&pool, 2023).await.unwrap();
        assert_eq!(before.income.trust_income.len(), 1);
        let row = &before.income.trust_income[0];
        assert_eq!(row.income_id, 1);
        assert_eq!(row.franked_amount_aud, dec("600"));
        assert_eq!(row.unfranked_amount_aud, dec("400"));
        assert_eq!(row.franking_credits_aud, dec("257.14"));
        // The document's stated invariant: every income figure sums to its
        // tax-summary line — and for a trust row that line is the question-13
        // pair, never the company-dividend one (SCENARIOS Z-f).
        assert_eq!(
            row.franked_amount_aud,
            summary(&before, "trust_franked_distributions")
        );
        assert_eq!(
            row.unfranked_amount_aud,
            summary(&before, "trust_income_unfranked")
        );
        assert_eq!(summary(&before, "dividends_assessable"), Decimal::ZERO);
        assert_eq!(
            row.franking_credits_aud,
            summary(&before, "franking_credits")
        );

        // FY2025 is a real AMIT year: the cash row is excluded from the
        // printed tables, exactly as the tax summary excludes it.
        let after = db_tax_report(&pool, 2025).await.unwrap();
        assert!(after.income.trust_income.is_empty());
        assert!(after.income.dividends.is_empty());
    }

    /// A listing bought and fully sold before the requested year (nothing
    /// held during it) is not flagged, even with no AMMA statement at all.
    #[tokio::test]
    async fn amma_missing_ignores_a_listing_not_held_during_the_year() {
        let pool = test_support::test_pool().await;
        listing_amit(&pool, 1, "AMT").await;
        test_support::buy(1, 1)
            .date(ymd(2021, 1, 4))
            .qty(dec("100"))
            .price(dec("10"))
            .insert(&pool)
            .await;
        test_support::sell(2, 1)
            .date(ymd(2022, 1, 4))
            .qty(dec("100"))
            .price(dec("12"))
            .insert(&pool)
            .await;
        test_support::allocate(&pool, 1, 2, 1, dec("100")).await;

        let report = db_tax_report(&pool, 2024).await.unwrap();
        assert!(report.completeness.amma_missing.is_empty());
        assert!(report.completeness.complete);
    }

    /// SCENARIOS H-09, H-10: a year whose only activity is an investment
    /// expense — no income at all — still exists as far as the print document
    /// is concerned. It is offered in the year list, its deduction prints in
    /// the Deductions table, and the net assessable investment income line
    /// carries the negative position (a deduction larger than the year's
    /// income is an ordinary result: it reduces other assessable income, and
    /// nothing here quarantines or carries it forward).
    #[tokio::test]
    async fn a_year_with_only_an_expense_still_prints_its_deduction() {
        let pool = test_support::test_pool().await;
        investment_expense::db_upsert(
            &pool,
            &investment_expense::InvestmentExpense {
                id: 1,
                date_incurred: ymd(2026, 3, 15), // FY2026
                expense_type: investment_expense::ExpenseType::LoanInterest,
                amount: dec("450"),
                gross_amount: None,
                deductible_percentage: None,
                currency: "AUD".to_string(),
                description: Some("margin loan interest".to_string()),
                listing_id: None,
                holding_account_id: None,
            },
        )
        .await
        .unwrap();

        assert_eq!(db_tax_report_years(&pool).await.unwrap(), vec![2026]);

        let report = db_tax_report(&pool, 2026).await.unwrap();
        assert_eq!(report.income.deductions.len(), 1);
        let row = &report.income.deductions[0];
        assert_eq!(row.expense_type, "LoanInterest");
        assert_eq!(row.amount_aud, dec("450"));
        assert!(report.income.dividends.is_empty());
        assert!(report.income.interest.is_empty());

        let line = |field: &str| {
            report
                .tax_summary
                .iter()
                .find(|l| l.field == field)
                .unwrap_or_else(|| panic!("no {field} line"))
                .value
                .clone()
        };
        assert_eq!(line("deductions_loan_interest"), "450");
        assert_eq!(line("gross_assessable_investment_income"), "0");
        assert_eq!(line("net_assessable_investment_income"), "-450");
    }

    /// SCENARIOS P-08: each printed deduction says *where the figure goes* on
    /// the return, not only what it was for — a fee earning a trust or AMIT
    /// distribution is claimed at 13Y, one earning foreign-source income nets
    /// into 20M, a debt deduction against foreign income goes to D15
    /// (question 20's worksheet excludes it), and everything else is the
    /// ordinary D7/D8 case (docs/ato/tax-return-labels-2026.md). The printed
    /// rows must sum to the same year's tax-summary destination lines, the
    /// document's standing invariant.
    #[tokio::test]
    async fn deduction_rows_print_the_question_each_is_claimed_at() {
        let pool = test_support::test_pool().await;
        let march = ymd(2026, 3, 1);
        test_support::listing(1)
            .ticker("VDHG")
            .amit(true)
            .insert(&pool)
            .await;
        test_support::listing(2).ticker("BHP").insert(&pool).await;
        test_support::listing(3)
            .ticker("ICE")
            .currency("USD")
            .mic("XNYS")
            .insert(&pool)
            .await;
        crate::entities::rba_fx_rate::db_import_rate(&pool, "USD", "2026-03", dec("0.50"))
            .await
            .unwrap();
        let mut foreign = test_support::income(1, 3, march).build();
        foreign.currency = "USD".to_string();
        foreign.foreign_source_income = dec("100");
        crate::entities::income::db_upsert(&pool, &foreign)
            .await
            .unwrap();

        let expense = |id: i64, kind, amount: &str, listing_id: Option<i64>| {
            investment_expense::InvestmentExpense {
                id,
                date_incurred: march,
                expense_type: kind,
                amount: dec(amount),
                gross_amount: None,
                deductible_percentage: None,
                currency: "AUD".to_string(),
                description: None,
                listing_id,
                holding_account_id: None,
            }
        };
        for e in [
            expense(
                1,
                investment_expense::ExpenseType::ManagementFee,
                "20",
                Some(1),
            ),
            expense(2, investment_expense::ExpenseType::AdviceFee, "30", Some(2)),
            expense(
                3,
                investment_expense::ExpenseType::ManagementFee,
                "40",
                Some(3),
            ),
            expense(
                4,
                investment_expense::ExpenseType::LoanInterest,
                "50",
                Some(3),
            ),
            expense(5, investment_expense::ExpenseType::Subscription, "60", None),
        ] {
            investment_expense::db_upsert(&pool, &e).await.unwrap();
        }

        let report = db_tax_report(&pool, 2026).await.unwrap();
        let rows = &report.income.deductions;
        assert_eq!(rows.len(), 5);
        let printed: Vec<(Option<&str>, &str)> = rows
            .iter()
            .map(|r| (r.ticker.as_deref(), r.ato_label.as_str()))
            .collect();
        assert_eq!(
            printed,
            vec![
                (Some("VDHG"), "13Y"),
                (Some("BHP"), "D7 / D8"),
                (Some("ICE"), "20M"),
                (Some("ICE"), "D15"),
                (None, "D7 / D8"),
            ]
        );
        assert_eq!(
            rows[0].destination,
            crate::domain::deduction_destination::DeductionDestination::TrustDistributions
        );

        // Each destination's printed rows sum to that destination's line in
        // the same document's tax summary.
        let line = |field: &str| {
            report
                .tax_summary
                .iter()
                .find(|l| l.field == field)
                .unwrap_or_else(|| panic!("no {field} line"))
                .value
                .clone()
        };
        let total_at = |label: &str| -> Decimal {
            rows.iter()
                .filter(|r| r.ato_label == label)
                .map(|r| r.amount_aud)
                .sum()
        };
        assert_eq!(total_at("13Y"), dec("20"));
        assert_eq!(line("deductions_trust_distributions"), "20");
        assert_eq!(total_at("20M"), dec("40"));
        assert_eq!(line("deductions_foreign_income"), "40");
        assert_eq!(total_at("D15"), dec("50"));
        assert_eq!(line("deductions_foreign_debt"), "50");
        assert_eq!(total_at("D7 / D8"), dec("90"));
        assert_eq!(line("deductions_dividend_and_interest"), "90");
        assert_eq!(line("deductions_total"), "200");

        // Over HTTP, the surface the archived document is printed from.
        let body: serde_json::Value = crate::test_support::ApiClient::full(&pool)
            .post_json(
                "/reports/tax-report",
                &serde_json::json!({"tax_year": 2026}),
            )
            .await;
        assert_eq!(body["income"]["deductions"][0]["ato_label"], "13Y");
        assert_eq!(
            body["income"]["deductions"][0]["destination"],
            "TrustDistributions"
        );
        assert_eq!(body["income"]["deductions"][4]["ato_label"], "D7 / D8");
    }

    /// SCENARIOS H-07: a deduction attributed to a listing prints which
    /// holding it was for, and prints it the way the listing was named when
    /// the fee was incurred. Without the ticker the archived PDF carries no
    /// trace of the attribution at all — and after a rename the bare
    /// `listing_id` in the JSON would be the only one left. A portfolio-wide
    /// expense has no holding to name and prints blank.
    #[tokio::test]
    async fn a_listing_attributed_deduction_prints_its_ticker_as_at_its_own_date() {
        let pool = test_support::test_pool().await;
        test_support::listing(1)
            .ticker("LAAC")
            .name("Lithium Americas (Argentina) Corp")
            .insert(&pool)
            .await;
        let expense = |id: i64, date: NaiveDate, listing_id: Option<i64>| {
            investment_expense::InvestmentExpense {
                id,
                date_incurred: date,
                expense_type: investment_expense::ExpenseType::AdviceFee,
                amount: dec("200"),
                gross_amount: None,
                deductible_percentage: None,
                currency: "AUD".to_string(),
                description: None,
                listing_id,
                holding_account_id: None,
            }
        };
        // Incurred against the holding before the rename…
        investment_expense::db_upsert(&pool, &expense(1, ymd(2024, 1, 5), Some(1)))
            .await
            .unwrap();
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
        // …and again after it, plus a portfolio-wide fee attributed to nothing.
        investment_expense::db_upsert(&pool, &expense(2, ymd(2024, 5, 1), Some(1)))
            .await
            .unwrap();
        investment_expense::db_upsert(&pool, &expense(3, ymd(2024, 6, 1), None))
            .await
            .unwrap();

        let report = db_tax_report(&pool, 2024).await.unwrap();
        assert_eq!(report.income.deductions.len(), 3);
        let rows = &report.income.deductions;
        assert_eq!(rows[0].listing_id, Some(1));
        assert_eq!(rows[0].ticker.as_deref(), Some("LAAC"));
        assert_eq!(rows[1].ticker.as_deref(), Some("LAR"));
        assert_eq!(rows[2].listing_id, None);
        assert_eq!(rows[2].ticker, None);
    }

    /// SCENARIOS J-09, J-11, J-14: a year with two vests from different grants
    /// and a sale out of the first. The printed document must keep the two
    /// sides apart — each statement prints its own Item 12 labels (D/E/F/G, the
    /// TFN withheld with it) in taxing-point order, while the disposal is
    /// Item 18 capital gains against the reset cost base — and the summary
    /// carries the assessable discount **net of** the one $1,000 reduction the
    /// year allows across both statements (here capped at the $600 eligible
    /// discount), with the TFN amount joining the withholding line.
    #[tokio::test]
    async fn two_vests_and_a_sale_print_their_item_12_labels_and_summary_lines() {
        let pool = test_support::test_pool().await;
        test_support::listing(1)
            .ticker("ICE")
            .name("Intercontinental")
            .insert(&pool)
            .await;
        // A deferral vest in September 2024 with TFN withheld…
        test_support::ess_statement(1, 1, ymd(2024, 9, 2))
            .with(|s| {
                s.quantity = dec("100");
                s.market_value_per_share = dec("10");
                s.deferral_discount = dec("1000");
                s.tfn_withholding = dec("470");
            })
            .insert(&pool)
            .await;
        // …and a taxed-upfront eligible one from another grant in March 2025,
        // both in FY2025.
        test_support::ess_statement(2, 1, ymd(2025, 3, 3))
            .with(|s| {
                s.quantity = dec("50");
                s.market_value_per_share = dec("12");
                s.taxed_upfront_eligible = dec("600");
            })
            .insert(&pool)
            .await;
        let first = crate::entities::ess_vest::db_vest(&pool, 1).await.unwrap();
        crate::entities::ess_vest::db_vest(&pool, 2).await.unwrap();

        // The September parcel sold in May 2025 at $15: $1,500 − $1,000.
        crate::entities::sell::db_upsert_sell(
            &pool,
            50,
            &crate::entities::sell::SellBody {
                brokerage_includes_gst: false,
                statement_total: None,
                holding_account_id: 1,
                date: ymd(2025, 5, 1),
                settlement_date: None,
                listing_id: 1,
                average_price: dec("15"),
                quantity: dec("100"),
                currency: "AUD".to_string(),
                brokerage: Decimal::ZERO,
                gst_on_brokerage: Decimal::ZERO,
                brokerage_currency: "AUD".to_string(),
                fx_rate: Decimal::ONE,
                spot_fx_rate: None,
                contract_note_ref: None,
                allocations: vec![crate::entities::sell::AllocationInput {
                    purchase_trade_id: first.id,
                    quantity_allocated: dec("100"),
                }],
            },
        )
        .await
        .unwrap();

        let report = db_tax_report(&pool, 2025).await.unwrap();
        assert_eq!(report.income.ess.len(), 2);
        let deferral = &report.income.ess[0]; // taxing-point order
        assert_eq!(deferral.ess_statement_id, 1);
        assert_eq!(deferral.ticker, "ICE");
        assert_eq!(deferral.taxing_point_date, ymd(2024, 9, 2));
        assert_eq!(deferral.deferral_discount_aud, dec("1000"));
        assert_eq!(deferral.taxed_upfront_eligible_aud, Decimal::ZERO);
        assert_eq!(deferral.tfn_withholding_aud, dec("470"));
        let upfront = &report.income.ess[1];
        assert_eq!(upfront.ess_statement_id, 2);
        assert_eq!(upfront.taxed_upfront_eligible_aud, dec("600"));
        assert_eq!(upfront.deferral_discount_aud, Decimal::ZERO);

        // The disposal is the CGT side, against the reset cost base — the
        // discount is not proceeds and the proceeds are not income.
        assert_eq!(report.disposals.totals.gain_loss_aud, dec("500"));

        let line = |field: &str| {
            report
                .tax_summary
                .iter()
                .find(|l| l.field == field)
                .unwrap_or_else(|| panic!("no {field} line"))
        };
        assert_eq!(line("ess_discount_assessable").value, "1000"); // 1600 − 600
        assert_eq!(line("ess_discount_assessable").ato_label, "12B");
        assert_eq!(line("ess_taxed_upfront_reduction").value, "600");
        assert_eq!(line("ess_foreign_source_discount").value, "0");
        assert_eq!(line("tfn_withholding_tax").value, "470");
        assert_eq!(line("dividends_assessable").value, "0");
    }

    /// The year list over HTTP — the surface the UI's closed `<select>` is
    /// built from, so a year missing here cannot be generated at all.
    async fn listed_years(pool: &SqlitePool) -> Vec<i32> {
        test_support::ApiClient::full(pool)
            .get_json("/reports/tax-report/years")
            .await
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

        // Exactly the two years with a fact — nothing carries a loss forward
        // here, so the list stays sparse: no filler for the years between or
        // after them.
        assert_eq!(db_tax_report_years(&pool).await.unwrap(), vec![2023, 2024]);
        assert_eq!(listed_years(&pool).await, vec![2023, 2024]);
    }

    /// SCENARIOS S-10: the picker never offers a financial year beyond the one
    /// in progress. A trade can no longer be dated in the future
    /// (`trade::AmountsError::FutureDate`), but the list unions every dated
    /// fact, and the other write paths are not bounded that way — an interest
    /// payment dated two years out used to put a financial year that has not
    /// begun on the closed `<select>`, and `POST /reports/tax-report` would
    /// then render an annual document for it.
    #[tokio::test]
    async fn the_year_list_never_offers_a_year_beyond_the_one_in_progress() {
        let pool = test_support::test_pool().await;
        let current = tax_year_for(crate::infra::date::today());
        let beyond = crate::infra::date::today() + chrono::Duration::days(2 * 365);
        interest_income::db_upsert(
            &pool,
            &interest_income::InterestIncome {
                id: 1,
                date_paid: beyond,
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
        // …and a second one in a year that has actually happened, so the
        // ceiling is shown to remove only what is past it.
        interest_income::db_upsert(
            &pool,
            &interest_income::InterestIncome {
                id: 2,
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
        // The future fact is recorded, and its own year is past the bound.
        assert!(tax_year_for(beyond) > current);

        for years in [
            db_tax_report_years(&pool).await.unwrap(),
            listed_years(&pool).await,
        ] {
            assert_eq!(
                years,
                vec![2024],
                "the FY2024 interest is offered and nothing past {current} is"
            );
        }
    }

    /// SCENARIOS P-04: a trust distribution with a 30 June entitlement date
    /// paid in mid-July is assessed in the FY just ended
    /// (`Income::assessment_date`, the rule the tax summary and this report's
    /// own income tables apply). The year list buckets income by that date
    /// too, so the year the distribution belongs to is the year offered — it
    /// used to offer the *payment* year, which has nothing in it, and hide
    /// FY2025 entirely.
    #[tokio::test]
    async fn the_year_list_buckets_trust_income_by_its_assessment_date() {
        let pool = test_support::test_pool().await;
        test_support::listing(1)
            .ticker("VDHG")
            .name("Vanguard Diversified High Growth")
            .insert(&pool)
            .await;
        test_support::income(1, 1, ymd(2025, 7, 15))
            .with(|i| {
                i.trust_income = true;
                i.entitlement_date = Some(ymd(2025, 6, 30)); // FY2025
                i.unfranked_amount = dec("100");
            })
            .insert(&pool)
            .await;

        assert_eq!(listed_years(&pool).await, vec![2025]);
        // And the document for that year does carry the distribution.
        let report = db_tax_report(&pool, 2025).await.unwrap();
        assert_eq!(report.income.trust_income.len(), 1);
    }

    /// SCENARIOS P-03: a CGT event that is not a trade puts its year on the
    /// list. A return of capital above the parcel's cost base (CGT event G1)
    /// makes a capital gain in the payment's year with no disposal at all —
    /// the list used to offer only the *purchase* year, so the year with the
    /// gain in it could not be generated.
    #[tokio::test]
    async fn the_year_list_offers_a_g1_excess_year_with_no_trade_in_it() {
        use crate::entities::corporate_action;
        let pool = test_support::test_pool().await;
        test_support::listing(1)
            .ticker("RAP")
            .name("Return A Plenty")
            .insert(&pool)
            .await;
        // FY2023 purchase: 100 units @ $1 → cost base $100.
        test_support::buy(1, 1)
            .date(ymd(2022, 8, 1))
            .qty(dec("100"))
            .price(dec("1"))
            .insert(&pool)
            .await;
        // FY2026 return of capital of $3/unit → $200 excess over the cost base.
        corporate_action::db_upsert(
            &pool,
            &corporate_action::CorporateAction {
                id: 10,
                listing_id: 1,
                date: ymd(2025, 9, 15),
                kind: corporate_action::ActionKind::ReturnOfCapital {
                    amount_per_unit: dec("3"),
                    currency: "AUD".to_string(),
                    record_date: None,
                },
            },
        )
        .await
        .unwrap();

        let years = listed_years(&pool).await;
        assert!(years.contains(&2026), "{years:?} omits the G1 year");
        assert!(years.contains(&2023), "{years:?} omits the purchase year");
        // Honest in the other direction: FY2024 and FY2025 have nothing in
        // them and no loss balance to carry, so they stay off the list.
        assert!(!years.contains(&2024), "{years:?} invents FY2024");
        assert!(!years.contains(&2025), "{years:?} invents FY2025");

        let report = db_tax_report(&pool, 2026).await.unwrap();
        let summary = report.cgt_summary.expect("the G1 year has a worksheet");
        assert_eq!(summary.cgt_event_g1_gain, dec("200"));
    }

    /// SCENARIOS P-03: the same for a rights sale as a year's only fact —
    /// `rights_sales` is in no date union, and the parcel it is anchored to
    /// was bought years earlier.
    #[tokio::test]
    async fn the_year_list_offers_a_rights_sale_only_year() {
        use crate::entities::{corporate_action, rights_sale};
        let pool = test_support::test_pool().await;
        test_support::listing(1)
            .ticker("RTS")
            .name("Rights Co")
            .insert(&pool)
            .await;
        // FY2022 purchase.
        test_support::buy(1, 1)
            .date(ymd(2021, 9, 1))
            .qty(dec("1000"))
            .price(dec("2"))
            .insert(&pool)
            .await;
        corporate_action::db_upsert(
            &pool,
            &corporate_action::CorporateAction {
                id: 10,
                listing_id: 1,
                date: ymd(2024, 7, 1),
                kind: corporate_action::ActionKind::RightsIssue {
                    rights_units: dec("1"),
                    rights_held_units: dec("4"),
                    exercise_price: dec("1.80"),
                    currency: "AUD".to_string(),
                },
            },
        )
        .await
        .unwrap();
        // 250 rights sold at 20c in July 2024 → a $50 gross FY2025 gain,
        // $25 after the discount.
        rights_sale::db_sell_rights(
            &pool,
            10,
            &rights_sale::SellRightsBody {
                date: ymd(2024, 7, 20),
                units: dec("250"),
                proceeds_per_right: Some(dec("0.20")),
                rights_cost: None,
                fx_rate: None,
                holding_account_id: 1,
                allocations: vec![rights_sale::AllocationInput {
                    purchase_trade_id: 1,
                    units: dec("250"),
                }],
            },
        )
        .await
        .unwrap();

        let years = listed_years(&pool).await;
        assert!(
            years.contains(&2025),
            "{years:?} omits the rights-sale year"
        );
        let report = db_tax_report(&pool, 2025).await.unwrap();
        assert_eq!(
            report
                .cgt_summary
                .expect("the rights-sale year has a worksheet")
                .net_capital_gain,
            dec("25")
        );
    }

    /// SCENARIOS P-02 (and O-03/O-04): a quiet year carrying a capital loss
    /// forward is offered too. `a_quiet_year_still_reports_its_carried_forward_loss`
    /// pins that the document prints such a year's label 18V figure; this
    /// pins that the picker can actually reach it. Every listed year is one
    /// `POST /reports/tax-report` accepts, so the list and the `tax_year`
    /// range validator agree.
    #[tokio::test]
    async fn the_year_list_offers_a_quiet_carry_forward_year() {
        let pool = test_support::test_pool().await;
        test_support::listing(1)
            .ticker("T1")
            .name("Test One")
            .insert(&pool)
            .await;
        // FY2025: a $4,000 capital loss, nothing to offset it.
        test_support::buy(1, 1)
            .date(ymd(2024, 8, 1))
            .qty(dec("100"))
            .price(dec("50"))
            .insert(&pool)
            .await;
        test_support::sell(2, 1)
            .date(ymd(2025, 5, 1))
            .qty(dec("100"))
            .price(dec("10"))
            .insert(&pool)
            .await;
        test_support::allocate(&pool, 1, 2, 1, dec("100")).await;

        let years = listed_years(&pool).await;
        assert!(years.contains(&2025), "{years:?} omits the loss year");
        assert!(
            years.contains(&2026),
            "{years:?} omits the quiet year carrying the loss forward"
        );
        // The list never reaches back before the first fact.
        assert!(!years.contains(&2024), "{years:?} invents FY2024");
        // Every offered year is one the report accepts (TaxYear's range).
        let client = test_support::ApiClient::full(&pool);
        for year in years {
            client
                .post(
                    "/reports/tax-report",
                    &serde_json::json!({ "tax_year": year }),
                )
                .await
                .expect_status(StatusCode::OK);
        }
    }

    /// The printed FX rates are the rates the printed AUD figures were
    /// computed at, on both sides (SCENARIOS M-01). This is a print-to-PDF
    /// document whose arithmetic a reader checks, so a rate that does not
    /// reproduce the figure beside it is worse than none: a Sell carrying a
    /// deliberate `spot_fx_rate` used to print the month's published rate
    /// instead, and a Sell resting on its own `fx_rate` fallback printed no
    /// rate at all beside a figure derived from one.
    #[tokio::test]
    async fn a_disposals_printed_fx_rates_reproduce_its_printed_aud_figures() {
        for (label, spot, ato_sell_rate, expected_sell_rate) in [
            // A deliberate spot override wins over the month's published rate.
            ("spot override", Some("0.5000"), Some("0.6800"), "0.5000"),
            // No published rate for the sale month: the trade's own fallback.
            ("fx_rate fallback", None, None, "0.5500"),
        ] {
            let pool = test_support::test_pool().await;
            test_support::listing(1)
                .ticker("AAPL")
                .name("AAPL")
                .mic("XNYS")
                .currency("USD")
                .insert(&pool)
                .await;
            rba_fx_rate::db_import_rate(&pool, "USD", "2023-03", dec("0.6600"))
                .await
                .unwrap();
            if let Some(rate) = ato_sell_rate {
                rba_fx_rate::db_import_rate(&pool, "USD", "2024-05", dec(rate))
                    .await
                    .unwrap();
            }
            test_support::buy(1, 1)
                .date(ymd(2023, 3, 15))
                .qty(dec("100"))
                .price(dec("150"))
                .currency("USD")
                .fx_rate(dec("0.6600"))
                .insert(&pool)
                .await;
            let mut sell = test_support::sell(2, 1)
                .date(ymd(2024, 5, 20))
                .settlement(ymd(2024, 5, 22))
                .qty(dec("100"))
                .price(dec("200"))
                .currency("USD")
                .fx_rate(dec("0.5500"));
            if let Some(spot) = spot {
                sell = sell.spot_fx_rate(dec(spot));
            }
            sell.insert(&pool).await;
            test_support::allocate(&pool, 1, 2, 1, dec("100")).await;

            let report = db_tax_report(&pool, 2024).await.unwrap();
            let row = &report.disposals.listings[0].parcels[0];
            assert_eq!(row.currency, "USD", "{label}");
            // Buy side: US$15,000 at the March 2023 rate.
            assert_eq!(row.buy_month_fx_rate, Some(dec("0.6600")), "{label}");
            // To the cent, which is what the document prints (SCENARIOS
            // W-d): the printed rate still reproduces the printed figure,
            // which is the point of printing it.
            assert_eq!(
                row.initial_cost_base_aud,
                to_cents(dec("15000") / row.buy_month_fx_rate.unwrap()),
                "{label}"
            );
            // Sell side: US$20,000 at the rate the proceeds actually used.
            assert_eq!(
                row.sell_month_fx_rate,
                Some(dec(expected_sell_rate)),
                "{label}"
            );
            assert_eq!(
                row.proceeds_aud,
                to_cents(dec("20000") / row.sell_month_fx_rate.unwrap()),
                "{label}"
            );
        }
    }

    /// An AUD disposal prints neither rate — there is no conversion to show.
    #[tokio::test]
    async fn an_aud_disposal_prints_no_fx_rates() {
        let pool = test_support::test_pool().await;
        test_support::listing(1)
            .ticker("BHP")
            .name("BHP")
            .insert(&pool)
            .await;
        test_support::buy(1, 1)
            .date(ymd(2023, 3, 15))
            .qty(dec("100"))
            .price(dec("40"))
            .insert(&pool)
            .await;
        test_support::sell(2, 1)
            .date(ymd(2024, 5, 20))
            .qty(dec("100"))
            .price(dec("50"))
            .insert(&pool)
            .await;
        test_support::allocate(&pool, 1, 2, 1, dec("100")).await;

        let report = db_tax_report(&pool, 2024).await.unwrap();
        let row = &report.disposals.listings[0].parcels[0];
        assert_eq!(row.currency, "AUD");
        assert_eq!(row.buy_month_fx_rate, None);
        assert_eq!(row.sell_month_fx_rate, None);
    }

    /// SCENARIOS P-02 (boundary): a `tax_year` no date can be built from used
    /// to **panic** the handler at `period_for`'s `expect` — bypassing
    /// `infra::http`'s one-error-type contract entirely (no classified status,
    /// no logged cause, the connection simply dropped). It is now a `422`
    /// whose body names the accepted range, so the UI toast says what to do.
    #[tokio::test]
    async fn an_absurd_tax_year_is_refused_naming_the_range_not_panicking() {
        let pool = test_support::test_pool().await;
        let client = test_support::ApiClient::full(&pool);
        for year in [300_000, MAX_TAX_YEAR + 1, i32::MAX] {
            let resp = client
                .post(
                    "/reports/tax-report",
                    &serde_json::json!({"tax_year": year}),
                )
                .await;
            let (status, body) = resp.status_and_body();
            assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{year}: {body}");
            assert!(
                body.contains(&MIN_TAX_YEAR.to_string())
                    && body.contains(&MAX_TAX_YEAR.to_string()),
                "the refusal must name the accepted range: {body}"
            );
        }
    }

    /// SCENARIOS P-02 (boundary): below the range, nonsense was accepted
    /// *silently* — `tax_year: 0` answered `200` with `period_start:
    /// "-0001-07-01"`. A year that is not a financial year at all is refused
    /// the same way as one chrono cannot represent.
    #[tokio::test]
    async fn a_nonsense_low_tax_year_is_refused_rather_than_reported_on() {
        let pool = test_support::test_pool().await;
        let client = test_support::ApiClient::full(&pool);
        for year in [0, -1, MIN_TAX_YEAR - 1, i32::MIN] {
            let resp = client
                .post(
                    "/reports/tax-report",
                    &serde_json::json!({"tax_year": year}),
                )
                .await;
            let (status, body) = resp.status_and_body();
            assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{year}: {body}");
            assert!(
                body.contains(&MIN_TAX_YEAR.to_string())
                    && body.contains(&MAX_TAX_YEAR.to_string()),
                "the refusal must name the accepted range: {body}"
            );
        }
    }

    /// SCENARIOS P-02 (boundary): the range exists to refuse non-years, never
    /// a financial year someone could legitimately ask for. A parcel bought
    /// last century and sold in FY1999 still reports, and so does the year
    /// after the one in progress (a draft or a projection).
    #[tokio::test]
    async fn a_far_past_and_a_future_financial_year_still_report() {
        let pool = test_support::test_pool().await;
        test_support::listing(1)
            .ticker("T1")
            .name("Test One")
            .insert(&pool)
            .await;
        test_support::buy(1, 1)
            .date(ymd(1997, 3, 3))
            .qty(dec("100"))
            .price(dec("5"))
            .insert(&pool)
            .await;
        test_support::sell(2, 1)
            .date(ymd(1998, 12, 1))
            .qty(dec("100"))
            .price(dec("8"))
            .insert(&pool)
            .await;
        test_support::allocate(&pool, 1, 2, 1, dec("100")).await;
        let client = test_support::ApiClient::full(&pool);

        let body: serde_json::Value = client
            .post_json(
                "/reports/tax-report",
                &serde_json::json!({"tax_year": 1999}),
            )
            .await;
        assert_eq!(body["meta"]["period_start"], "1998-07-01");
        assert_eq!(body["meta"]["period_end"], "1999-06-30");
        assert_eq!(
            body["disposals"]["listings"]
                .as_array()
                .expect("the FY1999 disposal schedule")
                .len(),
            1
        );

        // The financial year after the one in progress: no activity, so a
        // zeroed document — but a document, not a refusal.
        let next_year = tax_year_for(Utc::now().date_naive()) + 1;
        let body: serde_json::Value = client
            .post_json(
                "/reports/tax-report",
                &serde_json::json!({"tax_year": next_year}),
            )
            .await;
        assert_eq!(body["meta"]["tax_year"], next_year);
        assert!(
            body["disposals"]["listings"]
                .as_array()
                .expect("a zeroed disposal schedule")
                .is_empty()
        );
    }

    // ---- SCENARIOS W-d: the printed columns add up -------------------------

    /// Each [`DisposalTotals`] field beside the parcel-row field it totals —
    /// the pairing [`DisposalTotals::add`] makes, which is only visible in
    /// JSON as a name (the two differ once: `cost_base_aud` totals
    /// `adjusted_cost_base_aud`). The test below asserts this covers the whole
    /// struct, so a sixth total cannot be added without being reconciled here.
    const TOTALLED_COLUMNS: [(&str, &str); 5] = [
        ("proceeds_aud", "proceeds_aud"),
        ("cost_base_aud", "adjusted_cost_base_aud"),
        ("gain_loss_aud", "gain_loss_aud"),
        ("cgt_discount_amount_aud", "cgt_discount_amount_aud"),
        ("gain_after_discount_aud", "gain_after_discount_aud"),
    ];

    /// A JSON decimal as its `Decimal` value, failing loudly on a shape that
    /// is not a decimal string (a silently skipped column would make the
    /// reconciliation below vacuous).
    fn json_dec(value: &serde_json::Value) -> Decimal {
        value
            .as_str()
            .unwrap_or_else(|| panic!("expected a decimal string, got {value}"))
            .parse()
            .unwrap_or_else(|e| panic!("expected a decimal, got {value}: {e}"))
    }

    fn decimal_places(value: &serde_json::Value) -> usize {
        let text = value.as_str().expect("a decimal string");
        text.split_once('.').map_or(0, |(_, frac)| frac.len())
    }

    /// The money columns of a parcel row, found by name rather than listed:
    /// an AUD figure that is an *amount*, so every `*_aud` field except the
    /// derived per-unit pair (`docs/API.md`'s "Amounts round, rates don't"
    /// shows those at 4+ places precisely so they are not read as cents).
    /// A newly added money column is therefore covered without anyone
    /// remembering to add it here.
    fn money_columns(parcel: &serde_json::Value) -> Vec<(&str, &serde_json::Value)> {
        parcel
            .as_object()
            .expect("a parcel row")
            .iter()
            .filter(|(name, _)| name.ends_with("_aud") && !name.ends_with("_per_unit_aud"))
            .map(|(name, value)| (name.as_str(), value))
            .collect()
    }

    /// The SCENARIOS W-d facts: three ordinary BHP buys — $9.95 brokerage plus
    /// 99.5c GST on each, so every parcel's cost base lands on a half-cent —
    /// all sold in one Sell more than twelve months later.
    async fn bhp_three_parcel_disposal(pool: &SqlitePool) {
        test_support::listing(1)
            .ticker("BHP")
            .name("BHP Group Limited")
            .insert(pool)
            .await;
        for (id, qty, price, date) in [
            (1, "111", "44.87", ymd(2022, 3, 15)),
            (2, "223", "41.33", ymd(2022, 6, 15)),
            (3, "333", "39.71", ymd(2022, 9, 15)),
        ] {
            test_support::buy(id, 1)
                .date(date)
                .qty(dec(qty))
                .price(dec(price))
                .brokerage(dec("9.95"))
                .gst_on_brokerage(dec("0.995"))
                .insert(pool)
                .await;
        }
        test_support::sell(4, 1)
            .date(ymd(2024, 2, 15))
            .qty(dec("667"))
            .price(dec("46.13"))
            .brokerage(dec("9.95"))
            .gst_on_brokerage(dec("0.995"))
            .insert(pool)
            .await;
        test_support::allocate(pool, 1, 4, 1, dec("111")).await;
        test_support::allocate(pool, 2, 4, 2, dec("223")).await;
        test_support::allocate(pool, 3, 4, 3, dec("333")).await;
    }

    /// SCENARIOS W-d, the regression: the document is printed and archived, so
    /// a reader must be able to add a column up. Three parcels whose cost
    /// bases each land on a half-cent used to print rows summing to 1652.18
    /// under a subtotal of 1652.17 (the exact sum, rounded) — and a cost-base
    /// column of 27453.44 under 27453.43. Every figure here is pinned: the
    /// rows are cent-rounded and the subtotal is their sum, not the rounded
    /// exact total.
    #[tokio::test]
    async fn api_a_disposal_columns_rows_add_up_to_its_printed_subtotal() {
        let pool = test_support::test_pool().await;
        bhp_three_parcel_disposal(&pool).await;

        let body: serde_json::Value = test_support::ApiClient::full(&pool)
            .post_json(
                "/reports/tax-report",
                &serde_json::json!({"tax_year": 2024}),
            )
            .await;
        let group = &body["disposals"]["listings"][0];
        let parcels = group["parcels"].as_array().expect("three parcels");
        assert_eq!(parcels.len(), 3);

        // Row by row, as printed: cost base, proceeds, gain, discount.
        let printed: Vec<Vec<&str>> = parcels
            .iter()
            .map(|p| {
                [
                    "adjusted_cost_base_aud",
                    "proceeds_aud",
                    "gain_loss_aud",
                    "cgt_discount_amount_aud",
                    "gain_after_discount_aud",
                ]
                .iter()
                .map(|c| p[*c].as_str().expect("a decimal string"))
                .collect()
            })
            .collect();
        assert_eq!(
            printed,
            vec![
                vec!["4991.52", "5118.61", "127.09", "63.55", "63.55"],
                vec!["9227.54", "10283.33", "1055.80", "527.90", "527.90"],
                vec!["13234.38", "15355.83", "2121.45", "1060.73", "1060.73"],
            ]
        );

        // 63.55 + 527.90 + 1060.73 = 1652.18 — what the page adds up to, where
        // the subtotal used to print the exact sum rounded (1652.17).
        let subtotal = &group["subtotal"];
        assert_eq!(subtotal["cost_base_aud"], serde_json::json!("27453.44"));
        assert_eq!(subtotal["proceeds_aud"], serde_json::json!("30757.77"));
        assert_eq!(subtotal["gain_loss_aud"], serde_json::json!("3304.34"));
        assert_eq!(
            subtotal["cgt_discount_amount_aud"],
            serde_json::json!("1652.18")
        );
        assert_eq!(
            subtotal["gain_after_discount_aud"],
            serde_json::json!("1652.18")
        );
        // One listing, so the grand total is the same figure again — and the
        // document's two levels of total have to agree with each other too.
        assert_eq!(body["disposals"]["totals"], *subtotal);
    }

    /// The general rule behind that regression, over a document with three
    /// listing groups — an ordinary AUD disposal, an AMIT parcel carrying
    /// itemised cost-base adjustments, and a USD parcel whose every figure is
    /// an FX conversion: **every** money column of the disposal schedule is
    /// cent-rounded, each subtotal is the exact sum of the rows above it, and
    /// the grand total the exact sum of the subtotals.
    ///
    /// Money columns are found by name (`*_aud`, excluding the derived
    /// per-unit pair) rather than listed, so a newly added one is covered
    /// here; and the pairing table is asserted to cover the whole
    /// `DisposalTotals` struct, so a newly added *total* fails until it is.
    #[tokio::test]
    async fn api_every_disposal_money_column_totals_the_rounded_rows_beneath_it() {
        let pool = test_support::test_pool().await;
        bhp_three_parcel_disposal(&pool).await;

        // An AMIT parcel: the AMMA statement's per-unit reduction is itemised
        // under the cost base, so the adjustment rows are money columns too.
        listing_amit(&pool, 2, "VDHG").await;
        test_support::buy(11, 2)
            .date(ymd(2022, 7, 4))
            .qty(dec("137"))
            .price(dec("58.115"))
            .brokerage(dec("9.95"))
            .gst_on_brokerage(dec("0.995"))
            .insert(&pool)
            .await;
        test_support::amma(11, 2)
            .units(dec("137"))
            .cost_base_adjustment(dec("0.1234567"))
            .with(|a| a.tax_year_end_date = ymd(2023, 6, 30))
            .insert(&pool)
            .await;
        crate::entities::amit_adjustment_generation::db_generate(
            &pool,
            11,
            &crate::entities::amit_adjustment_generation::GenerateBody::default(),
        )
        .await
        .unwrap();
        test_support::sell(12, 2)
            .date(ymd(2024, 4, 3))
            .qty(dec("137"))
            .price(dec("61.037"))
            .brokerage(dec("9.95"))
            .gst_on_brokerage(dec("0.995"))
            .insert(&pool)
            .await;
        test_support::allocate(&pool, 11, 12, 11, dec("137")).await;

        // A USD parcel: both sides convert at a published rate, so every AUD
        // figure on the row is a long decimal.
        test_support::listing(3)
            .ticker("MSFT")
            .name("Microsoft Corporation")
            .mic("XNYS")
            .currency("USD")
            .insert(&pool)
            .await;
        for (month, rate) in [("2022-11", "0.6714"), ("2024-05", "0.6623")] {
            rba_fx_rate::db_import_rate(&pool, "USD", month, dec(rate))
                .await
                .unwrap();
        }
        test_support::buy(21, 3)
            .date(ymd(2022, 11, 9))
            .qty(dec("19"))
            .price(dec("237.53"))
            .currency("USD")
            .brokerage(dec("11.95"))
            .insert(&pool)
            .await;
        test_support::sell(22, 3)
            .date(ymd(2024, 5, 21))
            .qty(dec("19"))
            .price(dec("429.04"))
            .currency("USD")
            .brokerage(dec("11.95"))
            .insert(&pool)
            .await;
        test_support::allocate(&pool, 21, 22, 21, dec("19")).await;

        let body: serde_json::Value = test_support::ApiClient::full(&pool)
            .post_json(
                "/reports/tax-report",
                &serde_json::json!({"tax_year": 2024}),
            )
            .await;
        let disposals = &body["disposals"];
        let groups = disposals["listings"].as_array().expect("three groups");
        assert_eq!(groups.len(), 3, "one group per listing disposed of");

        // The pairing table is the whole of `DisposalTotals` — a sixth total
        // added to the struct fails here until it is reconciled below.
        let total_columns: HashSet<&str> = disposals["totals"]
            .as_object()
            .expect("the grand total")
            .keys()
            .map(String::as_str)
            .collect();
        assert_eq!(
            total_columns,
            TOTALLED_COLUMNS.iter().map(|(t, _)| *t).collect(),
            "every disposal total must be reconciled to the parcel column it sums"
        );

        let mut totals: HashMap<&str, Decimal> = HashMap::new();
        let mut adjustment_rows = 0;
        for group in groups {
            let parcels = group["parcels"].as_array().expect("parcel rows");
            assert!(!parcels.is_empty());

            for parcel in parcels {
                // Every money column of the row is a cent figure...
                for (name, value) in money_columns(parcel) {
                    assert!(
                        decimal_places(value) <= 2,
                        "{name} is a money column and must print to the cent, got {value}"
                    );
                }
                // ...including each itemised cost-base adjustment's amount,
                // which the document prints under the parcel it reduces.
                for adjustment in parcel["adjustments"].as_array().expect("adjustments") {
                    adjustment_rows += 1;
                    assert!(
                        decimal_places(&adjustment["amount"]) <= 2,
                        "an adjustment amount must print to the cent, got {}",
                        adjustment["amount"]
                    );
                }
            }

            // ...and each subtotal is exactly the sum of the rows above it.
            for (total, column) in TOTALLED_COLUMNS {
                let summed: Decimal = parcels.iter().map(|p| json_dec(&p[column])).sum();
                assert_eq!(
                    json_dec(&group["subtotal"][total]),
                    summed,
                    "{} subtotal must be the sum of its printed {column} rows",
                    group["ticker"]
                );
                *totals.entry(total).or_default() += summed;
            }
        }
        assert!(
            adjustment_rows > 0,
            "the AMIT parcel must contribute itemised adjustment rows"
        );

        // The grand total is in turn the exact sum of the subtotals.
        for (total, _) in TOTALLED_COLUMNS {
            assert_eq!(
                json_dec(&disposals["totals"][total]),
                totals[total],
                "the {total} grand total must be the sum of the printed subtotals"
            );
        }
    }

    /// The control on the rule above: the columns that are *not* money keep
    /// their full precision. A derived per-unit figure shows at 4+ decimal
    /// places rather than cent-rounded (`docs/API.md`, "Amounts round, rates
    /// don't"), and the contract note's own brokerage and GST are transcribed
    /// exactly as entered — 99.5c of GST on $9.95 of brokerage is genuinely
    /// sub-cent, and neither figure is totalled anywhere.
    #[tokio::test]
    async fn api_the_per_unit_and_as_entered_disposal_columns_are_not_cent_rounded() {
        let pool = test_support::test_pool().await;
        bhp_three_parcel_disposal(&pool).await;

        let body: serde_json::Value = test_support::ApiClient::full(&pool)
            .post_json(
                "/reports/tax-report",
                &serde_json::json!({"tax_year": 2024}),
            )
            .await;
        let parcel = &body["disposals"]["listings"][0]["parcels"][0];
        assert_eq!(
            parcel["cost_base_per_unit_aud"],
            serde_json::json!("44.968603603603603603603603604")
        );
        assert_eq!(
            parcel["proceeds_per_unit_aud"],
            serde_json::json!("46.11359070464767616191904048")
        );
        assert_eq!(parcel["buy_gst_on_brokerage"], serde_json::json!("0.995"));
        assert_eq!(parcel["buy_brokerage"], serde_json::json!("9.95"));
        assert_eq!(parcel["buy_price"], serde_json::json!("44.87"));
        assert_eq!(parcel["sale_price"], serde_json::json!("46.13"));
        // The money figure beside them is the rounded one, so the two rules
        // are visibly different rather than a coincidence of these facts.
        assert_eq!(
            parcel["adjusted_cost_base_aud"],
            serde_json::json!("4991.52")
        );
    }
}
