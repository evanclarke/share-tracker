use crate::domain::tax_year::tax_year_for;
use crate::entities::income::Income;
use crate::entities::listing;
use crate::infra::decimal::{parse_dec, row_opt_dec};
use crate::infra::fx::{FxOverride, FxRates};
use crate::infra::http::ApiError;
use crate::reports::{export, franking};
use axum::{Json, Router, extract::State, response::Response, routing::get};
use chrono::{Datelike, NaiveDate};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use sqlx::{FromRow, Row, SqlitePool};
use std::collections::HashMap;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaxYearSummary {
    /// Australian tax year: the calendar year in which June 30 falls (e.g. 2024 = FY2023/24).
    pub tax_year: i32,
    /// Assessable dividend income: franked_amount + unfranked_amount from
    /// income records. Rows on an AMIT listing are excluded entirely (every
    /// component): an AMIT's cash advice only funds the DRP chain — the AMMA
    /// statement is the assessable record, reported on the `amma_*` lines.
    pub dividends_assessable: Decimal,
    /// Gross Australian-source interest from interest-income records
    /// (question 10 label L — includes any TFN amount withheld; the withheld
    /// amount itself joins `tfn_withholding_tax`).
    pub interest_income: Decimal,
    /// Gross foreign-source interest from interest-income records marked
    /// `foreign_source` (e.g. a foreign broker's cash / money-market sweep
    /// fund): assessable foreign source income (question 20 label E), kept
    /// off the 10L line; the row's `foreign_tax_paid` joins
    /// `foreign_tax_offsets` (docs/ato/tax-return-labels-2026.md).
    pub foreign_interest_income: Decimal,
    /// Assessable foreign source income. Conduit foreign income is not part of
    /// it: despite the name, an Australian company's dividend declared to be
    /// CFI is Australian-sourced unfranked income, counted in
    /// `dividends_assessable` through `unfranked_amount` (see
    /// [`crate::entities::income::Income::conduit_foreign_income`]).
    pub foreign_source_income: Decimal,
    /// LIC capital gain deduction from income records (question D8): **50%**
    /// of each dividend's advised `lic_capital_gain_amount`, per
    /// [`Income::lic_capital_gain_deduction`](crate::entities::income::Income::lic_capital_gain_deduction).
    pub lic_capital_gain_deduction: Decimal,
    /// AMMA attributed Australian interest.
    pub amma_australian_interest: Decimal,
    /// AMMA attributed Australian dividends (unfranked).
    pub amma_dividends_unfranked: Decimal,
    /// AMMA attributed franked dividends.
    pub amma_franked_dividends: Decimal,
    /// AMMA attributed net rent.
    pub amma_net_rent: Decimal,
    /// AMMA attributed foreign income.
    pub amma_foreign_income: Decimal,
    /// AMMA attributed other income.
    pub amma_other_income: Decimal,
    /// AMMA attributed CGT discount gains.
    pub amma_cgt_discount_gains: Decimal,
    /// AMMA attributed CGT indexation gains.
    pub amma_cgt_indexation_gains: Decimal,
    /// AMMA attributed CGT other gains.
    pub amma_cgt_other_gains: Decimal,
    /// Informational: capital losses the trust applied *at its own level*
    /// before attributing the CGT amounts above (disclosed on the AMMA per the
    /// trustee guidance notes). Not a loss of the taxpayer's — the attributed
    /// gains are already net of it, and trust losses cannot flow to members —
    /// so no calculation reads it
    /// (`docs/ato/personal-investors-guide-managed-fund-distributions.md`).
    pub amma_capital_losses_applied: Decimal,
    /// Claimable franking credits (income + AMMA). Credits attached to a
    /// dividend whose shares fail the 45-day at-risk holding-period rule (90
    /// days for preference shares) are excluded, unless the small-shareholder
    /// exemption applies — total attached credits in the year below A$5,000
    /// (see `reports::franking`). The walk is anchored on the date the units
    /// went ex (`Income::ex_or_pay_date`); a dividend with no such date
    /// recorded falls back to the payment date, which cannot see a disposal
    /// made before it — `reports::franking_at_risk` lists those as
    /// `untested_no_ex_date` rather than leaving the gap silent.
    pub franking_credits: Decimal,
    /// Franking credits attached but denied by the holding-period rule (the
    /// amount excluded from `franking_credits`).
    pub franking_credits_denied: Decimal,
    /// Claimable foreign income tax offset (income foreign_tax_paid + AMMA
    /// foreign_tax_credits), capped at the A$1,000 FITO de-minimis: above that
    /// the ATO requires the offset-limit calculation, which needs the
    /// taxpayer's full income-tax position and is outside this system's data
    /// (see `docs/ato/fito-limit.md`).
    pub foreign_tax_offsets: Decimal,
    /// Foreign tax paid above the A$1,000 de-minimis (the amount excluded from
    /// `foreign_tax_offsets`). Claimable only to the extent the taxpayer's own
    /// offset-limit calculation supports it.
    pub foreign_tax_offset_excess: Decimal,
    /// Total TFN withholding tax (income + AMMA + ESS discounts).
    pub tfn_withholding_tax: Decimal,
    /// Assessable employee-share-scheme discount (Item 12 label B): the sum of
    /// the taxed-upfront (eligible + not eligible), deferral, and pre-2009
    /// cessation discounts, **net of** the applied $1,000 taxed-upfront
    /// reduction. Reported separately from dividend/trust income, in AUD
    /// (foreign-currency statements converted via the ATO rate for the
    /// taxing-point month). See `docs/ato/employee-share-schemes.md`.
    pub ess_discount_assessable: Decimal,
    /// The taxed-upfront $1,000 reduction actually applied this year
    /// (`min($1,000, total taxed-upfront-eligible discount)`). Surfaced like the
    /// FITO de-minimis: the tool applies the cap but the ≤A$180,000
    /// adjusted-taxable-income eligibility test needs the taxpayer's whole
    /// income position, so **confirming eligibility is the user's
    /// responsibility** (an ineligible taxpayer must add this amount back).
    pub ess_taxed_upfront_reduction: Decimal,
    /// Informational (Item 12 label A): the foreign-source portion of the ESS
    /// discounts, already counted within `ess_discount_assessable`, surfaced
    /// separately for the foreign-income / FITO calculation. Not added on top.
    pub ess_foreign_source_discount: Decimal,
    /// Gross assessable investment income for the year (AUD): the sum of the
    /// report's existing assessable income lines — `dividends_assessable`
    /// (franked + unfranked) + `interest_income` + `foreign_interest_income` +
    /// `foreign_source_income` + the six AMMA income components
    /// (`amma_australian_interest`, `amma_dividends_unfranked`,
    /// `amma_franked_dividends`, `amma_net_rent`, `amma_foreign_income`,
    /// `amma_other_income`). It deliberately excludes the franking-credit
    /// gross-up and FITO (carried as offset lines), the recorded conduit
    /// foreign income memo (already inside `dividends_assessable`, as part of
    /// `unfranked_amount`), the ESS discount (employment income, Item 12), and capital gains
    /// (the net-capital-gain report). `net_assessable_investment_income`
    /// subtracts the investment-expense deductions from this.
    pub gross_assessable_investment_income: Decimal,
    /// Deductible investment-expense total for the year, by expense type (AUD;
    /// see `entities::investment_expense`, docs/ato/investment-income-deductions.md).
    /// Each is the post-apportionment deductible amount the user recorded.
    pub deductions_loan_interest: Decimal,
    pub deductions_management_fee: Decimal,
    pub deductions_advice_fee: Decimal,
    pub deductions_account_keeping_fee: Decimal,
    pub deductions_subscription: Decimal,
    pub deductions_other: Decimal,
    /// Total deductible investment expenses for the year (sum of the per-type
    /// lines above), in AUD.
    pub deductions_total: Decimal,
    /// Net assessable investment income: `gross_assessable_investment_income −
    /// deductions_total` (AUD). The LIC capital gain deduction, franking-credit
    /// gross-up, and FITO are distinct and tracked on their own lines, so they
    /// are not folded in here.
    pub net_assessable_investment_income: Decimal,
    /// Informational: the taxpayer assumption behind the hard-wired rates
    /// (always [`crate::reports::TAXPAYER_BASIS`]) — the LIC capital gain
    /// deduction passed through here is the Australian-resident-individual 50%
    /// figure from the income record; other entity types are not modelled.
    pub taxpayer_basis: String,
}

pub fn router() -> Router<SqlitePool> {
    Router::new()
        .route("/portfolio/tax-summary", get(tax_summary_handler))
        .route(
            "/portfolio/tax-summary/export",
            get(tax_summary_export_handler),
        )
}

/// CSV export columns — `TaxYearSummary`'s fields in declaration order. The csv
/// writer rejects a record whose length differs from this header (see
/// `reports::export`), so a drift between the two fails loudly.
pub(crate) const CSV_HEADER: &[&str] = &[
    "tax_year",
    "dividends_assessable",
    "interest_income",
    "foreign_interest_income",
    "foreign_source_income",
    "lic_capital_gain_deduction",
    "amma_australian_interest",
    "amma_dividends_unfranked",
    "amma_franked_dividends",
    "amma_net_rent",
    "amma_foreign_income",
    "amma_other_income",
    "amma_cgt_discount_gains",
    "amma_cgt_indexation_gains",
    "amma_cgt_other_gains",
    "amma_capital_losses_applied",
    "franking_credits",
    "franking_credits_denied",
    "foreign_tax_offsets",
    "foreign_tax_offset_excess",
    "tfn_withholding_tax",
    "ess_discount_assessable",
    "ess_taxed_upfront_reduction",
    "ess_foreign_source_discount",
    "gross_assessable_investment_income",
    "deductions_loan_interest",
    "deductions_management_fee",
    "deductions_advice_fee",
    "deductions_account_keeping_fee",
    "deductions_subscription",
    "deductions_other",
    "deductions_total",
    "net_assessable_investment_income",
    "taxpayer_basis",
];

/// ATO tax-return label per `CSV_HEADER` column (same order), exported as the
/// second header row. Labels are from the **2026** individual tax return
/// (`docs/ato/tax-return-labels-2026.md` — re-verify when the form year
/// changes; the first cell names the form year). Empty = the column reports at
/// no label (informational or a derived total); `18 (working)` = an input to
/// question 18's net-capital-gain calculation, whose final 18H/18A/18V figures
/// the net-capital-gain export carries. The full mapping rationale is in
/// `docs/API.md`.
pub(crate) const CSV_ATO_LABELS: &[&str] = &[
    export::ATO_LABELS_MARKER, // tax_year
    "11S + 11T",               // dividends_assessable (unfranked + franked)
    "10L",                     // interest_income (Australian gross, incl. TFN withheld)
    "20E + 20M",               // foreign_interest_income
    "20E + 20M",               // foreign_source_income
    "D8",                      // lic_capital_gain_deduction (claimed at D8)
    "13U",                     // amma_australian_interest
    "13U",                     // amma_dividends_unfranked
    "13C",                     // amma_franked_dividends
    "13U",                     // amma_net_rent
    "20E + 20M",               // amma_foreign_income
    "13U",                     // amma_other_income
    "18 (working)",            // amma_cgt_discount_gains
    "18 (working)",            // amma_cgt_indexation_gains
    "18 (working)",            // amma_cgt_other_gains
    "",                        // amma_capital_losses_applied (trust-level, informational)
    "11U / 13Q",               // franking_credits
    "",                        // franking_credits_denied (informational)
    "20O",                     // foreign_tax_offsets
    "",                        // foreign_tax_offset_excess (informational)
    "10M / 11V / 13R / 12C",   // tfn_withholding_tax
    "12B",                     // ess_discount_assessable
    "",                        // ess_taxed_upfront_reduction (inside 12B vs 12D)
    "12A",                     // ess_foreign_source_discount
    "",                        // gross_assessable_investment_income (derived)
    "D7 / D8",                 // deductions_loan_interest
    "D7 / D8",                 // deductions_management_fee
    "D7 / D8",                 // deductions_advice_fee
    "D7 / D8",                 // deductions_account_keeping_fee
    "D7 / D8",                 // deductions_subscription
    "D7 / D8",                 // deductions_other
    "D7 / D8",                 // deductions_total
    "",                        // net_assessable_investment_income (derived)
    "",                        // taxpayer_basis
];

fn zero_summary(tax_year: i32) -> TaxYearSummary {
    TaxYearSummary {
        tax_year,
        dividends_assessable: Decimal::ZERO,
        interest_income: Decimal::ZERO,
        foreign_interest_income: Decimal::ZERO,
        foreign_source_income: Decimal::ZERO,
        lic_capital_gain_deduction: Decimal::ZERO,
        amma_australian_interest: Decimal::ZERO,
        amma_dividends_unfranked: Decimal::ZERO,
        amma_franked_dividends: Decimal::ZERO,
        amma_net_rent: Decimal::ZERO,
        amma_foreign_income: Decimal::ZERO,
        amma_other_income: Decimal::ZERO,
        amma_cgt_discount_gains: Decimal::ZERO,
        amma_cgt_indexation_gains: Decimal::ZERO,
        amma_cgt_other_gains: Decimal::ZERO,
        amma_capital_losses_applied: Decimal::ZERO,
        franking_credits: Decimal::ZERO,
        franking_credits_denied: Decimal::ZERO,
        foreign_tax_offsets: Decimal::ZERO,
        foreign_tax_offset_excess: Decimal::ZERO,
        tfn_withholding_tax: Decimal::ZERO,
        ess_discount_assessable: Decimal::ZERO,
        ess_taxed_upfront_reduction: Decimal::ZERO,
        ess_foreign_source_discount: Decimal::ZERO,
        gross_assessable_investment_income: Decimal::ZERO,
        deductions_loan_interest: Decimal::ZERO,
        deductions_management_fee: Decimal::ZERO,
        deductions_advice_fee: Decimal::ZERO,
        deductions_account_keeping_fee: Decimal::ZERO,
        deductions_subscription: Decimal::ZERO,
        deductions_other: Decimal::ZERO,
        deductions_total: Decimal::ZERO,
        net_assessable_investment_income: Decimal::ZERO,
        taxpayer_basis: crate::reports::TAXPAYER_BASIS.to_string(),
    }
}

/// Taxed-upfront eligible discounts are reducible by up to this de-minimis per
/// year (docs/ato/employee-share-schemes.md); the ≤A$180,000 income-test
/// eligibility is the user's responsibility (mirrors the FITO de-minimis).
fn ess_reduction_cap_aud() -> Decimal {
    Decimal::from(1000)
}

/// FITO de-minimis (docs/ato/fito-limit.md): up to A$1,000 of foreign income tax
/// paid in a year is claimable without working out the offset limit.
fn fito_de_minimis_aud() -> Decimal {
    Decimal::from(1000)
}

/// Read a TEXT decimal column from `row` and convert it to AUD via the
/// pre-loaded ATO rate for `currency` and the month of `date`. Income and
/// AMMA records carry no manual fx override, so a non-AUD amount with no ATO
/// rate fails loudly (the `FxError` surfaces as a decode error) rather than
/// being passed through or zeroed. `pub(crate)` so the annual tax report's
/// per-record income detail converts every figure exactly the same way this
/// report's totals do.
pub(crate) fn aud_field(
    fx: &FxRates,
    row: &sqlx::sqlite::SqliteRow,
    field: &str,
    currency: &str,
    date: NaiveDate,
) -> Result<Decimal, sqlx::Error> {
    let value = parse_dec(field, row.try_get(field)?)?;
    aud_value(fx, value, currency, date)
}

/// [`aud_field`] for a figure already decoded off a model struct rather than
/// read column-by-column off a row — same rate resolution, same loud failure
/// when a non-AUD amount has no ATO rate.
pub(crate) fn aud_value(
    fx: &FxRates,
    value: Decimal,
    currency: &str,
    date: NaiveDate,
) -> Result<Decimal, sqlx::Error> {
    Ok(fx.to_aud(value, currency, date, FxOverride::None)?)
}

/// AUD figure for an ESS discount label: the statement-AUD override column
/// (`aud_<field>` — the employer's stated AUD figure, used verbatim) when
/// present, otherwise the label converted like any other field.
pub(crate) fn aud_label(
    fx: &FxRates,
    row: &sqlx::sqlite::SqliteRow,
    field: &str,
    currency: &str,
    date: NaiveDate,
) -> Result<Decimal, sqlx::Error> {
    match row_opt_dec(row, &format!("aud_{field}"))? {
        Some(stated) => Ok(stated),
        None => aud_field(fx, row, field, currency, date),
    }
}

pub async fn db_tax_summary(pool: &SqlitePool) -> Result<Vec<TaxYearSummary>, sqlx::Error> {
    // One read transaction for the income-side inputs (and the FX rates that
    // convert them), so they come from a single consistent snapshot.
    let mut tx = pool.begin().await?;
    let result = db_tax_summary_on(&mut tx).await?;
    tx.commit().await?;
    Ok(result)
}

/// [`db_tax_summary`] on the caller's own connection, for callers (the
/// annual tax report) folding a single year's summary into a wider
/// single-snapshot read transaction instead of re-running this whole
/// multi-year aggregation on its own transaction.
pub(crate) async fn db_tax_summary_on(
    tx: &mut sqlx::SqliteConnection,
) -> Result<Vec<TaxYearSummary>, sqlx::Error> {
    // An AMIT listing's cash rows are excluded outright: for an AMIT the AMMA
    // attribution is the only assessable record — the cash advice exists to
    // drive the DRP chain, and counting its cash alongside the AMMA components
    // would double the year's income (write-time validation already keeps the
    // notional components off these rows, `entities::income`).
    //
    // The exclusion is per *year*, not per listing: a fund that converted to
    // an AMIT part-way through a holding was an ordinary trust before its
    // first AMIT income year, and those years' distributions are assessable
    // here exactly like any other trust's — dropping them because the fund is
    // an AMIT *now* would silently delete a year of income from the return
    // (SCENARIOS F-23). `listing::amit_in_tax_year` is the shared rule.
    let income_rows: Vec<(Income, bool, Option<NaiveDate>)> = sqlx::query(
        "SELECT i.*, l.amit AS listing_amit, l.amit_from AS listing_amit_from \
         FROM income i JOIN listings l ON l.id = i.listing_id",
    )
    .fetch_all(&mut *tx)
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
    let income_rows: Vec<&Income> = income_rows
        .iter()
        .filter(|(income, amit, amit_from)| {
            !listing::amit_in_tax_year(*amit, *amit_from, tax_year_for(income.assessment_date()))
        })
        .map(|(income, _, _)| income)
        .collect();

    let interest_rows = sqlx::query(
        "SELECT date_paid, amount, tfn_withholding_tax, foreign_source, foreign_tax_paid, \
         currency FROM interest_income",
    )
    .fetch_all(&mut *tx)
    .await?;

    let amma_rows = sqlx::query(
        "SELECT tax_year_end_date, australian_interest, australian_dividends_unfranked, \
         franked_dividends, franking_credits, net_rent, foreign_income, foreign_tax_credits, \
         other_income, cgt_discount_gains, cgt_indexation_gains, cgt_other_gains, \
         capital_losses_applied, tfn_withholding_tax, currency \
         FROM amma_statements",
    )
    .fetch_all(&mut *tx)
    .await?;

    let ess_rows = sqlx::query(
        "SELECT taxing_point_date, taxed_upfront_eligible, taxed_upfront_not_eligible, \
         deferral_discount, pre_2009_cessation_discount, foreign_source_discount, \
         tfn_withholding, currency, aud_taxed_upfront_eligible, \
         aud_taxed_upfront_not_eligible, aud_deferral_discount, \
         aud_pre_2009_cessation_discount, aud_foreign_source_discount \
         FROM ess_statements",
    )
    .fetch_all(&mut *tx)
    .await?;

    let expense_rows = sqlx::query(
        "SELECT date_incurred, expense_type, amount, currency FROM investment_expenses",
    )
    .fetch_all(&mut *tx)
    .await?;

    // every imported ATO FX rate — per-field conversions below are map
    // lookups, not one DB round-trip each
    let fx = FxRates::load(&mut *tx).await?;

    // Per-dividend candidates for the franking holding-period rule, and the
    // year's total attached credits (income + AMMA, AUD) for the
    // small-shareholder exemption test — the loader shared with the franking
    // at-risk report, so the two reports can never disagree about them.
    let (franked_dividends, attached_credits_by_year) =
        franking::db_franked_dividends(tx, &fx).await?;
    // The holding-period walks' inputs (listing preference, trades, splits)
    // load on the same transaction, so the denials below are computed from the
    // same snapshot as every other line — a trade committed after this point
    // can't change the outcome — and each dividend's walk below is a pure
    // in-memory pass, not more queries.
    let walks = franking::HoldingWalks::load(tx).await?;

    let mut map: HashMap<i32, TaxYearSummary> = HashMap::new();

    for income in &income_rows {
        // `Income::assessment_date`: trust income is assessed in the year of
        // *present entitlement*, not of payment (ATO QC 23087,
        // docs/ato/trust-income-timing.md) — a June trust distribution paid in
        // mid-July belongs to the FY just ended, and every component of the
        // row is attributed by that date; dividends go by date_paid.
        let assessed = income.assessment_date();
        let tax_year = tax_year_for(assessed);

        // Amounts are denominated in the record's currency; convert to AUD via the
        // ATO rate for the month of the assessment date (the entitlement date when
        // it governs, otherwise date_paid) before aggregating.
        let aud = |amount: Decimal| aud_value(&fx, amount, &income.currency, assessed);
        let franked = aud(income.franked_amount)?;
        let unfranked = aud(income.unfranked_amount)?;
        let foreign_income = aud(income.foreign_source_income)?;
        let foreign_tax = aud(income.foreign_tax_paid)?;
        let tfn_wht = aud(income.tfn_withholding_tax)?;
        let fc = aud(income.franking_credits)?;
        // 50% of the statement's LIC capital gain amount, not the amount
        // itself (`Income::lic_capital_gain_deduction`, the shared halving).
        let lic = aud(income.lic_capital_gain_deduction())?;

        let s = map
            .entry(tax_year)
            .or_insert_with(|| zero_summary(tax_year));
        s.dividends_assessable += franked + unfranked;
        s.foreign_source_income += foreign_income;
        s.lic_capital_gain_deduction += lic;
        s.franking_credits += fc;
        s.foreign_tax_offsets += foreign_tax;
        s.tfn_withholding_tax += tfn_wht;
    }

    // Interest (docs/ato/tax-return-labels-2026.md): assessed when
    // paid/credited. An Australian-source row's gross amount (question 10,
    // 10L) is its own line, its TFN amount withheld (10M) joining the
    // combined withholding line; a foreign-source row (a foreign broker's
    // cash / money-market fund) is instead assessable foreign source income
    // (question 20, 20E), its foreign tax withheld joining the FITO line —
    // write-time validation keeps each withholding kind on the matching
    // source (`entities::interest_income`).
    for row in &interest_rows {
        let date_paid: NaiveDate = row.try_get("date_paid")?;
        let tax_year = tax_year_for(date_paid);
        let currency: String = row.try_get("currency")?;
        let foreign_source: bool = row.try_get("foreign_source")?;
        let amount = aud_field(&fx, row, "amount", &currency, date_paid)?;
        let tfn_wht = aud_field(&fx, row, "tfn_withholding_tax", &currency, date_paid)?;
        let foreign_tax = aud_field(&fx, row, "foreign_tax_paid", &currency, date_paid)?;

        let s = map
            .entry(tax_year)
            .or_insert_with(|| zero_summary(tax_year));
        if foreign_source {
            s.foreign_interest_income += amount;
            s.foreign_tax_offsets += foreign_tax;
        } else {
            s.interest_income += amount;
        }
        s.tfn_withholding_tax += tfn_wht;
    }

    for row in &amma_rows {
        let tax_year_end_date: NaiveDate = row.try_get("tax_year_end_date")?;
        let tax_year = tax_year_end_date.year();

        // Convert to AUD via the ATO rate for the month of tax_year_end_date (the
        // statement's only period anchor) before aggregating.
        let currency: String = row.try_get("currency")?;
        let d = tax_year_end_date;
        let interest = aud_field(&fx, row, "australian_interest", &currency, d)?;
        let div_unfranked = aud_field(&fx, row, "australian_dividends_unfranked", &currency, d)?;
        let franked_div = aud_field(&fx, row, "franked_dividends", &currency, d)?;
        let fc = aud_field(&fx, row, "franking_credits", &currency, d)?;
        let rent = aud_field(&fx, row, "net_rent", &currency, d)?;
        let foreign_inc = aud_field(&fx, row, "foreign_income", &currency, d)?;
        let foreign_tax = aud_field(&fx, row, "foreign_tax_credits", &currency, d)?;
        let other = aud_field(&fx, row, "other_income", &currency, d)?;
        let cgt_disc = aud_field(&fx, row, "cgt_discount_gains", &currency, d)?;
        let cgt_idx = aud_field(&fx, row, "cgt_indexation_gains", &currency, d)?;
        let cgt_other = aud_field(&fx, row, "cgt_other_gains", &currency, d)?;
        let cap_losses = aud_field(&fx, row, "capital_losses_applied", &currency, d)?;
        let tfn_wht = aud_field(&fx, row, "tfn_withholding_tax", &currency, d)?;

        let s = map
            .entry(tax_year)
            .or_insert_with(|| zero_summary(tax_year));
        s.amma_australian_interest += interest;
        s.amma_dividends_unfranked += div_unfranked;
        s.amma_franked_dividends += franked_div;
        s.amma_net_rent += rent;
        s.amma_foreign_income += foreign_inc;
        s.amma_other_income += other;
        s.amma_cgt_discount_gains += cgt_disc;
        s.amma_cgt_indexation_gains += cgt_idx;
        s.amma_cgt_other_gains += cgt_other;
        s.amma_capital_losses_applied += cap_losses;
        s.franking_credits += fc;
        s.foreign_tax_offsets += foreign_tax;
        s.tfn_withholding_tax += tfn_wht;
        // AMMA credits count toward the small-shareholder threshold (in
        // `attached_credits_by_year`, via `db_franked_dividends`) but are
        // never themselves denied: the holding-period rule needs a
        // per-distribution ex-date, which an annual AMMA statement doesn't
        // carry.
    }

    // ESS discounts (docs/ato/employee-share-schemes.md): the assessable
    // discount (labels D + E + F + G) is declared in the year of the taxing
    // point, separately from dividend/trust income; the taxed-upfront-eligible
    // (D) total is reducible by up to A$1,000 per year. Accumulate the raw
    // discount and the eligible total here, then net the reduction below.
    let mut ess_eligible_by_year: HashMap<i32, Decimal> = HashMap::new();
    for row in &ess_rows {
        let taxing_point: NaiveDate = row.try_get("taxing_point_date")?;
        let tax_year = tax_year_for(taxing_point);

        let currency: String = row.try_get("currency")?;
        let d = taxing_point;
        // Each discount label prefers the statement-AUD override (the
        // employer's stated AUD figure, converted at the release-date spot
        // rate — what the ATO prefill carries) and falls back to the RBA
        // monthly conversion when absent.
        let eligible = aud_label(&fx, row, "taxed_upfront_eligible", &currency, d)?;
        let not_eligible = aud_label(&fx, row, "taxed_upfront_not_eligible", &currency, d)?;
        let deferral = aud_label(&fx, row, "deferral_discount", &currency, d)?;
        let pre_2009 = aud_label(&fx, row, "pre_2009_cessation_discount", &currency, d)?;
        let foreign = aud_label(&fx, row, "foreign_source_discount", &currency, d)?;
        let tfn = aud_field(&fx, row, "tfn_withholding", &currency, d)?;

        let s = map
            .entry(tax_year)
            .or_insert_with(|| zero_summary(tax_year));
        // Raw discount; the $1,000 reduction is subtracted per year below.
        s.ess_discount_assessable += eligible + not_eligible + deferral + pre_2009;
        s.ess_foreign_source_discount += foreign;
        s.tfn_withholding_tax += tfn;
        *ess_eligible_by_year.entry(tax_year).or_default() += eligible;
    }
    // Apply the taxed-upfront $1,000 reduction per year: reduce the assessable
    // discount by min(A$1,000, the year's eligible discount), and surface the
    // amount applied (the ≤A$180,000 income test is the user's responsibility).
    for (tax_year, eligible_total) in ess_eligible_by_year {
        let reduction = eligible_total.min(ess_reduction_cap_aud());
        if reduction > Decimal::ZERO {
            let s = map
                .get_mut(&tax_year)
                .expect("year inserted with the ESS row");
            s.ess_taxed_upfront_reduction = reduction;
            s.ess_discount_assessable -= reduction;
        }
    }

    // Deductible investment expenses (docs/ato/investment-income-deductions.md,
    // dividend-income-deductions.md): the cost of earning assessable investment
    // income, recorded as the post-apportionment deductible amount. Total by
    // expense type per Australian financial year; a non-AUD amount converts to
    // AUD via the ATO rate for the month incurred (fails loudly with no rate). An
    // expense in a year with no income still creates that year's row so the
    // deduction is visible.
    for row in &expense_rows {
        let date_incurred: NaiveDate = row.try_get("date_incurred")?;
        let tax_year = tax_year_for(date_incurred);
        let currency: String = row.try_get("currency")?;
        let amount = aud_field(&fx, row, "amount", &currency, date_incurred)?;
        let expense_type: String = row.try_get("expense_type")?;

        let s = map
            .entry(tax_year)
            .or_insert_with(|| zero_summary(tax_year));
        let line = match expense_type.as_str() {
            "LoanInterest" => &mut s.deductions_loan_interest,
            "ManagementFee" => &mut s.deductions_management_fee,
            "AdviceFee" => &mut s.deductions_advice_fee,
            "AccountKeepingFee" => &mut s.deductions_account_keeping_fee,
            "Subscription" => &mut s.deductions_subscription,
            "Other" => &mut s.deductions_other,
            // The column is CHECK-constrained to the set above; an unknown value
            // means the data model was bypassed — fail loudly rather than drop it.
            other => {
                return Err(sqlx::Error::Decode(
                    format!("unknown investment expense_type '{other}'").into(),
                ));
            }
        };
        *line += amount;
        s.deductions_total += amount;
    }

    // Franking-credit entitlement (docs/ato/you-and-your-shares-dividends.md): in a
    // year with A$5,000 or more of attached credits the small-shareholder
    // exemption doesn't apply, so each dividend's shares must pass the at-risk
    // holding-period test; the credits on units that fail it are denied.
    for div in &franked_dividends {
        let attached = attached_credits_by_year[&div.tax_year];
        if attached < franking::small_shareholder_threshold_aud() {
            continue;
        }
        let test = walks.test(div.listing_id, div.ex_date);
        let denied = test.denied(div.credits_aud);
        if denied > Decimal::ZERO {
            let s = map
                .get_mut(&div.tax_year)
                .expect("year inserted with the income row");
            s.franking_credits -= denied;
            s.franking_credits_denied += denied;
        }
    }

    // FITO de-minimis (docs/ato/fito-limit.md): a year's foreign tax offset over
    // A$1,000 needs the offset-limit calculation, which is outside this
    // system's data — cap the claimable offset and surface the excess.
    for s in map.values_mut() {
        let limit = fito_de_minimis_aud();
        if s.foreign_tax_offsets > limit {
            s.foreign_tax_offset_excess = s.foreign_tax_offsets - limit;
            s.foreign_tax_offsets = limit;
        }
    }

    // Gross assessable investment income (the report's existing assessable income
    // lines) and the net position after the investment-expense deductions. Done
    // last so every income, AMMA and deduction line is already aggregated.
    for s in map.values_mut() {
        s.gross_assessable_investment_income = s.dividends_assessable
            + s.interest_income
            + s.foreign_interest_income
            + s.foreign_source_income
            + s.amma_australian_interest
            + s.amma_dividends_unfranked
            + s.amma_franked_dividends
            + s.amma_net_rent
            + s.amma_foreign_income
            + s.amma_other_income;
        s.net_assessable_investment_income =
            s.gross_assessable_investment_income - s.deductions_total;
    }

    let mut result: Vec<TaxYearSummary> = map.into_values().collect();
    result.sort_by_key(|s| s.tax_year);
    Ok(result)
}

async fn tax_summary_handler(
    State(pool): State<SqlitePool>,
) -> Result<Json<Vec<TaxYearSummary>>, ApiError> {
    db_tax_summary(&pool)
        .await
        .map(Json)
        .map_err(ApiError::from)
}

/// The same per-year rows as the JSON report, as a downloadable tax-return-ready CSV.
async fn tax_summary_export_handler(State(pool): State<SqlitePool>) -> Result<Response, ApiError> {
    let rows = db_tax_summary(&pool).await.map_err(ApiError::from)?;
    export::csv_response("tax-summary.csv", CSV_HEADER, CSV_ATO_LABELS, &rows)
        .map_err(ApiError::from)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::ApiClient;
    use crate::{
        entities::{
            amma, ess_statement, income, interest_income, investment_expense, listing, rba_fx_rate,
            trade,
        },
        test_support::{self, test_pool, ymd},
    };
    use axum::http::StatusCode;

    /// Client over this module's own routes.
    fn client(pool: &SqlitePool) -> ApiClient {
        ApiClient::over(router().with_state(pool.clone()))
    }

    async fn insert_listing(pool: &SqlitePool, id: i64) {
        test_support::listing(id)
            .ticker(&format!("TST{id}"))
            .insert(pool)
            .await;
    }

    async fn insert_trade(
        pool: &SqlitePool,
        id: i64,
        listing_id: i64,
        trade_type: trade::TradeType,
        date: NaiveDate,
        qty: i64,
    ) {
        test_support::trade(id, listing_id, trade_type)
            .date(date)
            .qty(Decimal::from(qty))
            .price(Decimal::ONE)
            .insert(pool)
            .await;
    }

    fn make_income(id: i64, listing_id: i64, date: NaiveDate) -> income::Income {
        test_support::income(id, listing_id, date).build()
    }

    fn make_amma(id: i64, listing_id: i64, year_end: NaiveDate) -> amma::AmmaStatement {
        test_support::amma(id, listing_id)
            .with(|a| {
                a.tax_year_end_date = year_end;
                a.date_received = year_end + chrono::Duration::days(60);
            })
            .build()
    }

    fn make_ess(id: i64, listing_id: i64, taxing_point: NaiveDate) -> ess_statement::EssStatement {
        test_support::ess_statement(id, listing_id, taxing_point).build()
    }

    fn make_expense(
        id: i64,
        date: NaiveDate,
        expense_type: investment_expense::ExpenseType,
        amount: Decimal,
    ) -> investment_expense::InvestmentExpense {
        investment_expense::InvestmentExpense {
            id,
            date_incurred: date,
            expense_type,
            amount,
            gross_amount: None,
            deductible_percentage: None,
            currency: "AUD".to_string(),
            description: None,
            listing_id: None,
            holding_account_id: None,
        }
    }

    // DB-level tests

    #[tokio::test]
    async fn db_empty_returns_empty() {
        let pool = test_pool().await;
        let result = db_tax_summary(&pool).await.unwrap();
        assert!(result.is_empty());
    }

    #[tokio::test]
    async fn db_dividend_income_aggregated_by_tax_year() {
        let pool = test_pool().await;
        insert_listing(&pool, 1).await;
        // Jan 2024 → FY2024 (July 2023 – June 2024)
        let mut inc = make_income(1, 1, NaiveDate::from_ymd_opt(2024, 1, 15).unwrap());
        inc.franked_amount = Decimal::from(70);
        inc.unfranked_amount = Decimal::from(30);
        inc.franking_credits = Decimal::from(30);
        income::db_upsert(&pool, &inc).await.unwrap();

        let result = db_tax_summary(&pool).await.unwrap();
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].tax_year, 2024);
        assert_eq!(result[0].dividends_assessable, Decimal::from(100));
        assert_eq!(result[0].franking_credits, Decimal::from(30));
    }

    /// The LIC deduction (and the discount in the companion net-capital-gain
    /// report) is the Australian-resident-individual 50% rate; every row states
    /// that assumption explicitly (scope decision 2026-06-07: entity types are
    /// not modelled).
    #[tokio::test]
    async fn db_rows_state_the_individual_resident_basis() {
        let pool = test_pool().await;
        insert_listing(&pool, 1).await;
        let mut inc = make_income(1, 1, NaiveDate::from_ymd_opt(2024, 1, 15).unwrap());
        inc.franked_amount = Decimal::from(70);
        income::db_upsert(&pool, &inc).await.unwrap();

        let result = db_tax_summary(&pool).await.unwrap();
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].taxpayer_basis, crate::reports::TAXPAYER_BASIS);
        // The assumption ships in the CSV export too (CSV_HEADER names it).
        assert!(CSV_HEADER.contains(&"taxpayer_basis"));
    }

    #[tokio::test]
    async fn db_july_date_belongs_to_next_tax_year() {
        let pool = test_pool().await;
        insert_listing(&pool, 1).await;
        // July 1 2023 → FY2024 (July 2023 – June 2024)
        let mut inc = make_income(1, 1, NaiveDate::from_ymd_opt(2023, 7, 1).unwrap());
        inc.unfranked_amount = Decimal::from(50);
        income::db_upsert(&pool, &inc).await.unwrap();

        let result = db_tax_summary(&pool).await.unwrap();
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].tax_year, 2024);
        assert_eq!(result[0].dividends_assessable, Decimal::from(50));
    }

    /// Trust income is assessed in the year of present entitlement, not
    /// payment (ATO QC 23087, docs/ato/trust-income-timing.md): a June trust
    /// distribution paid in mid-July belongs to the FY just ended, while a
    /// dividend paid the same day stays in the FY of payment.
    #[tokio::test]
    async fn db_trust_distribution_assessed_by_entitlement_date_not_payment() {
        let pool = test_pool().await;
        insert_listing(&pool, 1).await;
        // June 2026 trust distribution, paid 15 July 2026 → FY2026.
        let mut trust = make_income(1, 1, NaiveDate::from_ymd_opt(2026, 7, 15).unwrap());
        trust.trust_income = true;
        trust.entitlement_date = Some(NaiveDate::from_ymd_opt(2026, 6, 30).unwrap());
        trust.unfranked_amount = Decimal::from(100);
        trust.tfn_withholding_tax = Decimal::from(10);
        income::db_upsert(&pool, &trust).await.unwrap();
        // A dividend paid the same day is assessed when paid → FY2027.
        let mut div = make_income(2, 1, NaiveDate::from_ymd_opt(2026, 7, 15).unwrap());
        div.unfranked_amount = Decimal::from(50);
        income::db_upsert(&pool, &div).await.unwrap();

        let result = db_tax_summary(&pool).await.unwrap();
        assert_eq!(result.len(), 2);
        assert_eq!(result[0].tax_year, 2026);
        // Every component follows the entitlement date, not just the income line.
        assert_eq!(result[0].dividends_assessable, Decimal::from(100));
        assert_eq!(result[0].tfn_withholding_tax, Decimal::from(10));
        assert_eq!(result[1].tax_year, 2027);
        assert_eq!(result[1].dividends_assessable, Decimal::from(50));
    }

    /// Without an entitlement date a trust row keeps the date_paid attribution
    /// (existing rows are unaffected by the new column).
    #[tokio::test]
    async fn db_trust_without_entitlement_date_assessed_by_date_paid() {
        let pool = test_pool().await;
        insert_listing(&pool, 1).await;
        let mut trust = make_income(1, 1, NaiveDate::from_ymd_opt(2026, 7, 15).unwrap());
        trust.trust_income = true;
        trust.unfranked_amount = Decimal::from(100);
        income::db_upsert(&pool, &trust).await.unwrap();

        let result = db_tax_summary(&pool).await.unwrap();
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].tax_year, 2027);
    }

    /// The AUD conversion month follows the assessment date too: a USD trust
    /// row entitled in June converts at the June rate (a July-keyed lookup
    /// would find no rate and fail loudly).
    #[tokio::test]
    async fn db_trust_entitlement_date_drives_fx_month() {
        let pool = test_pool().await;
        insert_listing(&pool, 1).await;
        // A$1 = 0.50 USD for June 2026 only — no July rate exists.
        rba_fx_rate::db_import_rate(&pool, "USD", "2026-06", "0.50".parse().unwrap())
            .await
            .unwrap();
        let mut trust = make_income(1, 1, NaiveDate::from_ymd_opt(2026, 7, 15).unwrap());
        trust.trust_income = true;
        trust.entitlement_date = Some(NaiveDate::from_ymd_opt(2026, 6, 30).unwrap());
        trust.currency = "USD".to_string();
        trust.unfranked_amount = Decimal::from(100);
        income::db_upsert(&pool, &trust).await.unwrap();

        let result = db_tax_summary(&pool).await.unwrap();
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].tax_year, 2026);
        assert_eq!(result[0].dividends_assessable, Decimal::from(200)); // 100 / 0.50
    }

    /// The A$5,000 small-shareholder threshold groups attached credits by the
    /// assessment year, so July-paid June trust credits count toward the
    /// entitlement year's total.
    #[tokio::test]
    async fn db_franking_threshold_year_follows_entitlement_date() {
        let pool = test_pool().await;
        insert_listing(&pool, 1).await;
        // Long-held parcel so the 45-day at-risk test passes once the
        // threshold is crossed.
        insert_trade(
            &pool,
            1,
            1,
            trade::TradeType::Buy,
            NaiveDate::from_ymd_opt(2024, 1, 10).unwrap(),
            1000,
        )
        .await;
        // FY2026 dividend credits just below the threshold on their own.
        let mut div = make_income(1, 1, NaiveDate::from_ymd_opt(2026, 1, 15).unwrap());
        div.ex_date = Some(NaiveDate::from_ymd_opt(2026, 1, 2).unwrap());
        div.franked_amount = "11643.33".parse().unwrap();
        div.franking_credits = Decimal::from(4990);
        income::db_upsert(&pool, &div).await.unwrap();
        // July-paid June trust credits tip FY2026 over A$5,000.
        let mut trust = make_income(2, 1, NaiveDate::from_ymd_opt(2026, 7, 15).unwrap());
        trust.trust_income = true;
        trust.entitlement_date = Some(NaiveDate::from_ymd_opt(2026, 6, 15).unwrap());
        trust.unfranked_amount = Decimal::from(100);
        trust.franking_credits = Decimal::from(20);
        income::db_upsert(&pool, &trust).await.unwrap();

        let result = db_tax_summary(&pool).await.unwrap();
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].tax_year, 2026);
        // Threshold crossed (4,990 + 20 ≥ 5,000) → the at-risk walk runs; the
        // long-held parcel passes it, so all credits stay claimable in FY2026.
        assert_eq!(result[0].franking_credits, Decimal::from(5010));
        assert_eq!(result[0].franking_credits_denied, Decimal::ZERO);
    }

    /// Conduit foreign income is a **memo inside `unfranked_amount`**, not an
    /// amount of its own (SCENARIOS G-03): for the Australian-resident
    /// individual this report is written for, an unfranked dividend declared to
    /// be CFI is assessable — it is NANE only to a foreign resident
    /// (Subdiv 802-A; `docs/ato/amma-statement-guidance-notes.md` Part B item
    /// 13U puts it in the non-primary production income). So the resident's
    /// assessable figure is the whole unfranked amount, counted once: the CFI
    /// column is neither added on top nor netted off, and despite its name it
    /// is Australian-sourced, so it stays out of `foreign_source_income` too.
    #[tokio::test]
    async fn db_conduit_foreign_income_is_assessable_within_the_unfranked_amount() {
        let pool = test_pool().await;
        insert_listing(&pool, 1).await;
        let mut inc = make_income(1, 1, NaiveDate::from_ymd_opt(2024, 3, 15).unwrap());
        inc.unfranked_amount = Decimal::from(100); // includes the CFI portion
        inc.conduit_foreign_income = Decimal::from(40); // memo subset of the above
        inc.foreign_source_income = Decimal::from(100);
        income::db_upsert(&pool, &inc).await.unwrap();

        let result = db_tax_summary(&pool).await.unwrap();
        assert_eq!(result.len(), 1);
        // The full unfranked amount is assessable — not 60, and not 140.
        assert_eq!(result[0].dividends_assessable, Decimal::from(100));
        // The CFI memo doesn't leak into the foreign total either.
        assert_eq!(result[0].foreign_source_income, Decimal::from(100));
        assert_eq!(
            result[0].gross_assessable_investment_income,
            Decimal::from(200)
        );
    }

    #[tokio::test]
    async fn db_amma_components_attributed_to_tax_year() {
        let pool = test_pool().await;
        insert_listing(&pool, 1).await;
        let year_end = NaiveDate::from_ymd_opt(2024, 6, 30).unwrap();
        let mut a = make_amma(1, 1, year_end);
        a.australian_interest = Decimal::from(10);
        a.franked_dividends = Decimal::from(20);
        a.franking_credits = Decimal::from(8);
        a.foreign_income = Decimal::from(5);
        a.foreign_tax_credits = Decimal::from(2);
        a.cgt_discount_gains = Decimal::from(50);
        a.tfn_withholding_tax = Decimal::from(3);
        amma::db_upsert(&pool, &a).await.unwrap();

        let result = db_tax_summary(&pool).await.unwrap();
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].tax_year, 2024);
        assert_eq!(result[0].amma_australian_interest, Decimal::from(10));
        assert_eq!(result[0].amma_franked_dividends, Decimal::from(20));
        assert_eq!(result[0].franking_credits, Decimal::from(8));
        assert_eq!(result[0].amma_foreign_income, Decimal::from(5));
        assert_eq!(result[0].foreign_tax_offsets, Decimal::from(2));
        assert_eq!(result[0].amma_cgt_discount_gains, Decimal::from(50));
        assert_eq!(result[0].tfn_withholding_tax, Decimal::from(3));
    }

    #[tokio::test]
    async fn db_income_spanning_two_tax_years() {
        let pool = test_pool().await;
        insert_listing(&pool, 1).await;
        // FY2024: Jan 2024
        let mut inc1 = make_income(1, 1, NaiveDate::from_ymd_opt(2024, 1, 15).unwrap());
        inc1.unfranked_amount = Decimal::from(100);
        income::db_upsert(&pool, &inc1).await.unwrap();
        // FY2025: Sep 2024
        let mut inc2 = make_income(2, 1, NaiveDate::from_ymd_opt(2024, 9, 15).unwrap());
        inc2.unfranked_amount = Decimal::from(200);
        income::db_upsert(&pool, &inc2).await.unwrap();

        let result = db_tax_summary(&pool).await.unwrap();
        assert_eq!(result.len(), 2);
        assert_eq!(result[0].tax_year, 2024);
        assert_eq!(result[0].dividends_assessable, Decimal::from(100));
        assert_eq!(result[1].tax_year, 2025);
        assert_eq!(result[1].dividends_assessable, Decimal::from(200));
    }

    #[tokio::test]
    async fn db_income_and_amma_franking_credits_combined() {
        let pool = test_pool().await;
        insert_listing(&pool, 1).await;
        // Income with $30 franking credits in FY2024
        let mut inc = make_income(1, 1, NaiveDate::from_ymd_opt(2024, 3, 15).unwrap());
        inc.franked_amount = Decimal::from(70); // the dividend the $30 is attached to
        inc.franking_credits = Decimal::from(30);
        income::db_upsert(&pool, &inc).await.unwrap();
        // AMMA with $8 franking credits for FY2024
        let mut a = make_amma(1, 1, NaiveDate::from_ymd_opt(2024, 6, 30).unwrap());
        a.franking_credits = Decimal::from(8);
        amma::db_upsert(&pool, &a).await.unwrap();

        let result = db_tax_summary(&pool).await.unwrap();
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].tax_year, 2024);
        assert_eq!(result[0].franking_credits, Decimal::from(38));
    }

    #[tokio::test]
    async fn db_lic_deduction_is_half_the_advised_amount() {
        let pool = test_pool().await;
        insert_listing(&pool, 1).await;
        let mut inc = make_income(1, 1, NaiveDate::from_ymd_opt(2024, 3, 15).unwrap());
        // The statement's advised LIC capital gain amount (the attributable
        // part); the individual deducts 50% of it at D8.
        inc.lic_capital_gain_amount = Decimal::from(15);
        income::db_upsert(&pool, &inc).await.unwrap();

        let result = db_tax_summary(&pool).await.unwrap();
        assert_eq!(
            result[0].lic_capital_gain_deduction,
            Decimal::new(75, 1),
            "D8 is half the advised attributable part"
        );
    }

    #[tokio::test]
    async fn db_full_year_mixed_income_types() {
        let pool = test_pool().await;
        insert_listing(&pool, 1).await;
        insert_listing(&pool, 2).await;

        // Dividend income FY2024
        let mut div = make_income(1, 1, NaiveDate::from_ymd_opt(2024, 3, 15).unwrap());
        div.franked_amount = Decimal::from(140);
        div.unfranked_amount = Decimal::from(60);
        div.franking_credits = Decimal::from(60);
        div.foreign_tax_paid = Decimal::from(0);
        div.tfn_withholding_tax = Decimal::from(5);
        income::db_upsert(&pool, &div).await.unwrap();

        // Trust distribution FY2024 whose unfranked amount includes a
        // declared conduit-foreign-income portion (a memo within it, not a
        // component beside it).
        let mut trust = make_income(2, 2, NaiveDate::from_ymd_opt(2024, 6, 30).unwrap());
        trust.foreign_source_income = Decimal::from(30);
        trust.foreign_tax_paid = Decimal::from(9);
        trust.unfranked_amount = Decimal::from(40);
        trust.conduit_foreign_income = Decimal::from(10); // 10 of the 40 above
        trust.lic_capital_gain_amount = Decimal::from(5);
        trust.trust_income = true;
        income::db_upsert(&pool, &trust).await.unwrap();

        // AMMA statement FY2024
        let mut a = make_amma(1, 2, NaiveDate::from_ymd_opt(2024, 6, 30).unwrap());
        a.australian_interest = Decimal::from(8);
        a.cgt_discount_gains = Decimal::from(100);
        a.foreign_tax_credits = Decimal::from(3);
        a.tfn_withholding_tax = Decimal::from(2);
        amma::db_upsert(&pool, &a).await.unwrap();

        let result = db_tax_summary(&pool).await.unwrap();
        assert_eq!(result.len(), 1);
        let s = &result[0];
        assert_eq!(s.tax_year, 2024);
        // 140 + 60 dividend, + the trust's 40 unfranked (its 10 of CFI is
        // inside that 40, counted once — never added again, never netted off).
        assert_eq!(s.dividends_assessable, Decimal::from(240));
        // Despite the name, CFI is Australian-sourced: it stays out of this.
        assert_eq!(s.foreign_source_income, Decimal::from(30));
        assert_eq!(s.lic_capital_gain_deduction, Decimal::new(25, 1)); // 50% of the advised 5
        assert_eq!(s.franking_credits, Decimal::from(60)); // only from income (amma.franking_credits = 0)
        assert_eq!(s.foreign_tax_offsets, Decimal::from(12)); // 9 income + 3 amma
        assert_eq!(s.tfn_withholding_tax, Decimal::from(7)); // 5 income + 2 amma
        assert_eq!(s.amma_australian_interest, Decimal::from(8));
        assert_eq!(s.amma_cgt_discount_gains, Decimal::from(100));
    }

    // AMIT cash distributions (REQUIREMENTS 2026-06-12): cash rows on an AMIT
    // listing fund the DRP chain only — the AMMA statement is the assessable
    // record, so every component of the cash row is excluded from the summary.

    /// An AMIT cash row contributes nothing to any income line, while the
    /// fund's AMMA components and a non-AMIT dividend in the same year report
    /// as before.
    /// SCENARIOS F-23: a fund that converted to an AMIT for FY2025. The
    /// exclusion follows the year, not the listing — the pre-conversion
    /// year's distribution is ordinary trust income and is still reported in
    /// full, while the AMIT year's cash row is excluded as usual. Flipping
    /// the flag used to delete the earlier year from the return outright.
    #[tokio::test]
    async fn db_a_converted_funds_pre_amit_years_are_still_reported() {
        let pool = test_pool().await;
        test_support::listing(1)
            .ticker("VDHG")
            .amit_from(NaiveDate::from_ymd_opt(2024, 7, 1).unwrap())
            .insert(&pool)
            .await;

        // FY2024, an ordinary trust distribution with its franking credits.
        let mut before = make_income(1, 1, NaiveDate::from_ymd_opt(2024, 2, 15).unwrap());
        before.trust_income = true;
        before.franked_amount = Decimal::from(200);
        before.unfranked_amount = Decimal::from(300);
        before.franking_credits = Decimal::from(85);
        income::db_upsert(&pool, &before).await.unwrap();

        // FY2025, the first AMIT year: cash-only, excluded, with the AMMA
        // carrying the assessable figures.
        let mut cash = make_income(2, 1, NaiveDate::from_ymd_opt(2025, 2, 15).unwrap());
        cash.trust_income = true;
        cash.unfranked_amount = Decimal::from(400);
        income::db_upsert(&pool, &cash).await.unwrap();
        let mut a = make_amma(1, 1, NaiveDate::from_ymd_opt(2025, 6, 30).unwrap());
        a.other_income = Decimal::from(450);
        amma::db_upsert(&pool, &a).await.unwrap();

        let result = db_tax_summary(&pool).await.unwrap();
        let fy24 = result.iter().find(|s| s.tax_year == 2024).expect("FY2024");
        assert_eq!(fy24.dividends_assessable, Decimal::from(500));
        assert_eq!(fy24.franking_credits, Decimal::from(85));
        let fy25 = result.iter().find(|s| s.tax_year == 2025).expect("FY2025");
        assert_eq!(fy25.dividends_assessable, Decimal::ZERO);
        assert_eq!(fy25.amma_other_income, Decimal::from(450));
    }

    #[tokio::test]
    async fn db_amit_cash_rows_excluded_from_every_income_line() {
        let pool = test_pool().await;
        test_support::listing(1)
            .ticker("VDHG")
            .amit(true)
            .insert(&pool)
            .await;
        insert_listing(&pool, 2).await;

        // AMIT quarterly cash distribution: gross cash + source withholding.
        let mut cash = make_income(1, 1, NaiveDate::from_ymd_opt(2024, 1, 15).unwrap());
        cash.trust_income = true;
        cash.unfranked_amount = Decimal::from(1000);
        cash.foreign_source_income = Decimal::from(10);
        cash.foreign_tax_paid = Decimal::from(2);
        cash.tfn_withholding_tax = Decimal::from(5);
        income::db_upsert(&pool, &cash).await.unwrap();

        // The fund's AMMA attribution for the same FY.
        let mut a = make_amma(1, 1, NaiveDate::from_ymd_opt(2024, 6, 30).unwrap());
        a.franked_dividends = Decimal::from(300);
        a.franking_credits = Decimal::from(120);
        a.foreign_income = Decimal::from(40);
        a.foreign_tax_credits = Decimal::from(6);
        amma::db_upsert(&pool, &a).await.unwrap();

        // A non-AMIT franked dividend in the same FY still counts in full.
        let mut div = make_income(2, 2, NaiveDate::from_ymd_opt(2024, 3, 15).unwrap());
        div.franked_amount = Decimal::from(70);
        div.franking_credits = Decimal::from(30);
        income::db_upsert(&pool, &div).await.unwrap();

        let result = db_tax_summary(&pool).await.unwrap();
        assert_eq!(result.len(), 1);
        let s = &result[0];
        assert_eq!(s.tax_year, 2024);
        // Only the PLS-style ordinary dividend; the $1,010 AMIT cash is gone.
        assert_eq!(s.dividends_assessable, Decimal::from(70));
        assert_eq!(s.foreign_source_income, Decimal::ZERO);
        // Credits/offsets/withholding come from the dividend and the AMMA only.
        assert_eq!(s.franking_credits, Decimal::from(150)); // 30 + 120
        assert_eq!(s.foreign_tax_offsets, Decimal::from(6)); // AMMA only
        assert_eq!(s.tfn_withholding_tax, Decimal::ZERO);
        // The AMMA attribution is unchanged by the exclusion.
        assert_eq!(s.amma_franked_dividends, Decimal::from(300));
        assert_eq!(s.amma_foreign_income, Decimal::from(40));
        // Gross assessable = dividend + AMMA components, no cash.
        assert_eq!(
            s.gross_assessable_investment_income,
            Decimal::from(70 + 300 + 40)
        );
    }

    /// Rows entered before the write-time validation existed may carry
    /// notional components (simulated by flipping the listing to AMIT after
    /// the insert): the report-level exclusion drops the whole row, so legacy
    /// credits can neither be claimed nor count toward the small-shareholder
    /// threshold.
    #[tokio::test]
    async fn db_legacy_amit_rows_with_components_fully_excluded() {
        let pool = test_pool().await;
        insert_listing(&pool, 1).await;
        let mut cash = make_income(1, 1, NaiveDate::from_ymd_opt(2024, 1, 15).unwrap());
        cash.trust_income = true;
        cash.unfranked_amount = Decimal::from(1000);
        cash.franking_credits = Decimal::from(6000); // over the $5,000 threshold
        income::db_upsert(&pool, &cash).await.unwrap();
        let mut l = test_support::listing(1).ticker("TST1").build();
        l.amit = true;
        listing::db_upsert(&pool, &l).await.unwrap();

        let result = db_tax_summary(&pool).await.unwrap();
        assert!(result.is_empty());
    }

    /// Legacy AMIT credits don't count toward the A$5,000 small-shareholder
    /// threshold either: a short-held non-AMIT dividend under the threshold
    /// on its own keeps its credits even with a 6,000-credit AMIT row in the
    /// same year.
    #[tokio::test]
    async fn db_amit_credits_do_not_count_toward_franking_threshold() {
        let pool = test_pool().await;
        insert_listing(&pool, 1).await;
        insert_listing(&pool, 2).await;
        // Short at-risk holding: bought and sold around the ex-date.
        insert_trade(
            &pool,
            1,
            2,
            trade::TradeType::Buy,
            NaiveDate::from_ymd_opt(2025, 3, 1).unwrap(),
            1000,
        )
        .await;
        insert_trade(
            &pool,
            2,
            2,
            trade::TradeType::Sell,
            NaiveDate::from_ymd_opt(2025, 4, 10).unwrap(),
            1000,
        )
        .await;
        let mut div = make_income(1, 2, NaiveDate::from_ymd_opt(2025, 4, 8).unwrap());
        div.ex_date = Some(NaiveDate::from_ymd_opt(2025, 3, 14).unwrap());
        div.franked_amount = Decimal::from(7000);
        div.franking_credits = Decimal::from(3000);
        income::db_upsert(&pool, &div).await.unwrap();
        // Legacy AMIT row with credits (listing flipped to AMIT post-insert).
        let mut cash = make_income(2, 1, NaiveDate::from_ymd_opt(2025, 1, 15).unwrap());
        cash.trust_income = true;
        cash.unfranked_amount = Decimal::from(1000);
        cash.franking_credits = Decimal::from(6000);
        income::db_upsert(&pool, &cash).await.unwrap();
        let mut l = test_support::listing(1).ticker("TST1").build();
        l.amit = true;
        listing::db_upsert(&pool, &l).await.unwrap();

        let result = db_tax_summary(&pool).await.unwrap();
        assert_eq!(result.len(), 1);
        // Under the threshold without the AMIT credits → exemption holds.
        assert_eq!(result[0].franking_credits, Decimal::from(3000));
        assert_eq!(result[0].franking_credits_denied, Decimal::ZERO);
    }

    #[tokio::test]
    async fn db_usd_income_converted_to_aud_via_ato_rate() {
        let pool = test_pool().await;
        insert_listing(&pool, 1).await;
        // A$1 = 0.50 USD for Jan 2024 → AUD = USD / 0.50.
        rba_fx_rate::db_import_rate(&pool, "USD", "2024-01", "0.50".parse().unwrap())
            .await
            .unwrap();
        let mut inc = make_income(1, 1, NaiveDate::from_ymd_opt(2024, 1, 15).unwrap());
        inc.currency = "USD".to_string();
        inc.franked_amount = Decimal::from(70);
        inc.unfranked_amount = Decimal::from(30);
        inc.franking_credits = Decimal::from(30);
        income::db_upsert(&pool, &inc).await.unwrap();

        let result = db_tax_summary(&pool).await.unwrap();
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].tax_year, 2024);
        // (70 + 30) / 0.50 = 200 AUD; 30 / 0.50 = 60 AUD.
        assert_eq!(result[0].dividends_assessable, Decimal::from(200));
        assert_eq!(result[0].franking_credits, Decimal::from(60));
    }

    #[tokio::test]
    async fn db_usd_amma_converted_to_aud_via_ato_rate() {
        let pool = test_pool().await;
        insert_listing(&pool, 1).await;
        // Rate for the month of tax_year_end_date (June 2024).
        rba_fx_rate::db_import_rate(&pool, "USD", "2024-06", "0.50".parse().unwrap())
            .await
            .unwrap();
        let mut a = make_amma(1, 1, NaiveDate::from_ymd_opt(2024, 6, 30).unwrap());
        a.currency = "USD".to_string();
        a.foreign_income = Decimal::from(5);
        a.foreign_tax_credits = Decimal::from(2);
        a.cgt_discount_gains = Decimal::from(50);
        amma::db_upsert(&pool, &a).await.unwrap();

        let result = db_tax_summary(&pool).await.unwrap();
        assert_eq!(result.len(), 1);
        // 5 / 0.50 = 10; 2 / 0.50 = 4; 50 / 0.50 = 100.
        assert_eq!(result[0].amma_foreign_income, Decimal::from(10));
        assert_eq!(result[0].foreign_tax_offsets, Decimal::from(4));
        assert_eq!(result[0].amma_cgt_discount_gains, Decimal::from(100));
    }

    #[tokio::test]
    async fn db_non_aud_without_ato_rate_fails_loudly() {
        let pool = test_pool().await;
        insert_listing(&pool, 1).await;
        // No USD rate imported for the month → conversion must fail, not zero/pass through.
        let mut inc = make_income(1, 1, NaiveDate::from_ymd_opt(2024, 1, 15).unwrap());
        inc.currency = "USD".to_string();
        inc.unfranked_amount = Decimal::from(100);
        income::db_upsert(&pool, &inc).await.unwrap();

        assert!(db_tax_summary(&pool).await.is_err());
    }

    // Franking-credit entitlement (45-day holding-period rule + small-shareholder
    // exemption — docs/ato/you-and-your-shares-dividends.md, reports::franking).

    /// Matthew-shaped facts: credits over $5,000 and the parcel held at risk
    /// under 45 days, so the credits are denied but the dividend stays assessable.
    #[tokio::test]
    async fn db_franking_credits_denied_when_held_under_45_days() {
        let pool = test_pool().await;
        insert_listing(&pool, 1).await;
        insert_trade(
            &pool,
            1,
            1,
            trade::TradeType::Buy,
            NaiveDate::from_ymd_opt(2025, 3, 1).unwrap(),
            1000,
        )
        .await;
        insert_trade(
            &pool,
            2,
            1,
            trade::TradeType::Sell,
            NaiveDate::from_ymd_opt(2025, 4, 10).unwrap(),
            1000,
        )
        .await;
        let mut inc = make_income(1, 1, NaiveDate::from_ymd_opt(2025, 4, 8).unwrap());
        inc.ex_date = Some(NaiveDate::from_ymd_opt(2025, 3, 14).unwrap());
        inc.franked_amount = Decimal::from(13066);
        inc.franking_credits = Decimal::from(5600);
        income::db_upsert(&pool, &inc).await.unwrap();

        let result = db_tax_summary(&pool).await.unwrap();
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].tax_year, 2025);
        assert_eq!(result[0].dividends_assessable, Decimal::from(13066));
        assert_eq!(result[0].franking_credits, Decimal::ZERO);
        assert_eq!(result[0].franking_credits_denied, Decimal::from(5600));
    }

    /// Two franked dividends on one listing in one year: each gets its own
    /// walk over the single pre-loaded input set. The March dividend loses
    /// the credits on the short-held parcel while the August dividend (whose
    /// qualification window the sale post-dates) keeps every credit.
    #[tokio::test]
    async fn db_two_dividends_on_one_listing_denied_independently() {
        let pool = test_pool().await;
        insert_listing(&pool, 1).await;
        insert_trade(&pool, 1, 1, trade::TradeType::Buy, ymd(2024, 3, 14), 10000).await;
        insert_trade(&pool, 2, 1, trade::TradeType::Buy, ymd(2025, 3, 4), 4000).await;
        insert_trade(&pool, 3, 1, trade::TradeType::Sell, ymd(2025, 4, 3), 4000).await;

        // Ex 14 Aug 2024: its qualification window closed months before the
        // April 2025 sale — nothing denied.
        let mut aug = make_income(1, 1, ymd(2024, 8, 28));
        aug.ex_date = Some(ymd(2024, 8, 14));
        aug.franked_amount = Decimal::from(7000);
        aug.franking_credits = Decimal::from(3000);
        income::db_upsert(&pool, &aug).await.unwrap();
        // Ex 14 Mar 2025: LIFO deems the recent 4,000 units sold at 29
        // at-risk days — 4,000/14,000 of its 7,000 credits denied.
        let mut mar = make_income(2, 1, ymd(2025, 3, 28));
        mar.ex_date = Some(ymd(2025, 3, 14));
        mar.franked_amount = "16333.33".parse().unwrap();
        mar.franking_credits = Decimal::from(7000);
        income::db_upsert(&pool, &mar).await.unwrap();

        let result = db_tax_summary(&pool).await.unwrap();
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].tax_year, 2025);
        // The year's attached A$10,000 is over the small-shareholder
        // threshold, so the rule applies to both — only March's walk denies.
        assert_eq!(result[0].franking_credits_denied, Decimal::from(2000));
        assert_eq!(result[0].franking_credits, Decimal::from(8000));
    }

    /// Same short holding, but total attached credits under $5,000: the
    /// small-shareholder exemption keeps them claimable.
    #[tokio::test]
    async fn db_small_shareholder_exemption_keeps_credits_below_5000() {
        let pool = test_pool().await;
        insert_listing(&pool, 1).await;
        insert_trade(
            &pool,
            1,
            1,
            trade::TradeType::Buy,
            NaiveDate::from_ymd_opt(2025, 3, 1).unwrap(),
            1000,
        )
        .await;
        insert_trade(
            &pool,
            2,
            1,
            trade::TradeType::Sell,
            NaiveDate::from_ymd_opt(2025, 4, 10).unwrap(),
            1000,
        )
        .await;
        let mut inc = make_income(1, 1, NaiveDate::from_ymd_opt(2025, 4, 8).unwrap());
        inc.ex_date = Some(NaiveDate::from_ymd_opt(2025, 3, 14).unwrap());
        inc.franked_amount = Decimal::from(7000);
        inc.franking_credits = Decimal::from(3000);
        income::db_upsert(&pool, &inc).await.unwrap();

        let result = db_tax_summary(&pool).await.unwrap();
        assert_eq!(result[0].franking_credits, Decimal::from(3000));
        assert_eq!(result[0].franking_credits_denied, Decimal::ZERO);
    }

    /// The exemption needs the year's credits to be *below* $5,000 — exactly
    /// $5,000 is not exempt.
    #[tokio::test]
    async fn db_exactly_5000_attached_credits_is_not_exempt() {
        let pool = test_pool().await;
        insert_listing(&pool, 1).await;
        insert_trade(
            &pool,
            1,
            1,
            trade::TradeType::Buy,
            NaiveDate::from_ymd_opt(2025, 3, 1).unwrap(),
            1000,
        )
        .await;
        insert_trade(
            &pool,
            2,
            1,
            trade::TradeType::Sell,
            NaiveDate::from_ymd_opt(2025, 4, 10).unwrap(),
            1000,
        )
        .await;
        let mut inc = make_income(1, 1, NaiveDate::from_ymd_opt(2025, 4, 8).unwrap());
        inc.ex_date = Some(NaiveDate::from_ymd_opt(2025, 3, 14).unwrap());
        inc.franked_amount = "11666.67".parse().unwrap();
        inc.franking_credits = Decimal::from(5000);
        income::db_upsert(&pool, &inc).await.unwrap();

        let result = db_tax_summary(&pool).await.unwrap();
        assert_eq!(result[0].franking_credits, Decimal::ZERO);
        assert_eq!(result[0].franking_credits_denied, Decimal::from(5000));
    }

    /// AMMA-attributed credits push the year over the $5,000 threshold (so a
    /// short-held dividend's credits are denied) but are never denied themselves.
    #[tokio::test]
    async fn db_amma_credits_count_toward_small_shareholder_threshold() {
        let pool = test_pool().await;
        insert_listing(&pool, 1).await;
        insert_listing(&pool, 2).await;
        insert_trade(
            &pool,
            1,
            1,
            trade::TradeType::Buy,
            NaiveDate::from_ymd_opt(2025, 3, 1).unwrap(),
            1000,
        )
        .await;
        insert_trade(
            &pool,
            2,
            1,
            trade::TradeType::Sell,
            NaiveDate::from_ymd_opt(2025, 4, 10).unwrap(),
            1000,
        )
        .await;
        // $3,000 income credits alone would be exempt…
        let mut inc = make_income(1, 1, NaiveDate::from_ymd_opt(2025, 4, 8).unwrap());
        inc.ex_date = Some(NaiveDate::from_ymd_opt(2025, 3, 14).unwrap());
        inc.franked_amount = Decimal::from(7000);
        inc.franking_credits = Decimal::from(3000);
        income::db_upsert(&pool, &inc).await.unwrap();
        // …but $2,500 AMMA credits take the year's total to $5,500.
        let mut a = make_amma(1, 2, NaiveDate::from_ymd_opt(2025, 6, 30).unwrap());
        a.franking_credits = Decimal::from(2500);
        amma::db_upsert(&pool, &a).await.unwrap();

        let result = db_tax_summary(&pool).await.unwrap();
        assert_eq!(result.len(), 1);
        // The income credits are denied; the AMMA credits remain claimable.
        assert_eq!(result[0].franking_credits, Decimal::from(2500));
        assert_eq!(result[0].franking_credits_denied, Decimal::from(3000));
    }

    /// Without a recorded ex-date the test anchors on the payment date.
    #[tokio::test]
    async fn db_missing_ex_date_falls_back_to_date_paid() {
        let pool = test_pool().await;
        insert_listing(&pool, 1).await;
        insert_trade(
            &pool,
            1,
            1,
            trade::TradeType::Buy,
            NaiveDate::from_ymd_opt(2025, 4, 1).unwrap(),
            1000,
        )
        .await;
        insert_trade(
            &pool,
            2,
            1,
            trade::TradeType::Sell,
            NaiveDate::from_ymd_opt(2025, 4, 20).unwrap(),
            1000,
        )
        .await;
        let mut inc = make_income(1, 1, NaiveDate::from_ymd_opt(2025, 4, 8).unwrap());
        inc.ex_date = None;
        inc.franked_amount = Decimal::from(14000);
        inc.franking_credits = Decimal::from(6000);
        income::db_upsert(&pool, &inc).await.unwrap();

        let result = db_tax_summary(&pool).await.unwrap();
        assert_eq!(result[0].franking_credits, Decimal::ZERO);
        assert_eq!(result[0].franking_credits_denied, Decimal::from(6000));
    }

    /// A long-held parcel's credits are untouched by the rule even in a
    /// non-exempt year.
    #[tokio::test]
    async fn db_long_held_parcel_keeps_credits_in_non_exempt_year() {
        let pool = test_pool().await;
        insert_listing(&pool, 1).await;
        insert_trade(
            &pool,
            1,
            1,
            trade::TradeType::Buy,
            NaiveDate::from_ymd_opt(2023, 1, 10).unwrap(),
            1000,
        )
        .await;
        let mut inc = make_income(1, 1, NaiveDate::from_ymd_opt(2025, 4, 8).unwrap());
        inc.ex_date = Some(NaiveDate::from_ymd_opt(2025, 3, 14).unwrap());
        inc.franked_amount = Decimal::from(14000);
        inc.franking_credits = Decimal::from(6000);
        income::db_upsert(&pool, &inc).await.unwrap();

        let result = db_tax_summary(&pool).await.unwrap();
        assert_eq!(result[0].franking_credits, Decimal::from(6000));
        assert_eq!(result[0].franking_credits_denied, Decimal::ZERO);
    }

    // FITO de-minimis cap (docs/ato/fito-limit.md): up to A$1,000 of foreign tax
    // is claimable as-is; above that the offset-limit calculation is required,
    // so the claimable offset is capped and the excess surfaced.

    #[tokio::test]
    async fn db_foreign_tax_under_1000_passes_through() {
        let pool = test_pool().await;
        insert_listing(&pool, 1).await;
        let mut inc = make_income(1, 1, NaiveDate::from_ymd_opt(2024, 3, 15).unwrap());
        inc.foreign_source_income = Decimal::from(3000);
        inc.foreign_tax_paid = Decimal::from(999);
        income::db_upsert(&pool, &inc).await.unwrap();

        let result = db_tax_summary(&pool).await.unwrap();
        assert_eq!(result[0].foreign_tax_offsets, Decimal::from(999));
        assert_eq!(result[0].foreign_tax_offset_excess, Decimal::ZERO);
    }

    /// "Up to $1,000" is claimable without the limit calculation — exactly
    /// $1,000 is not capped.
    #[tokio::test]
    async fn db_foreign_tax_exactly_1000_is_not_capped() {
        let pool = test_pool().await;
        insert_listing(&pool, 1).await;
        let mut inc = make_income(1, 1, NaiveDate::from_ymd_opt(2024, 3, 15).unwrap());
        inc.foreign_tax_paid = Decimal::from(1000);
        income::db_upsert(&pool, &inc).await.unwrap();

        let result = db_tax_summary(&pool).await.unwrap();
        assert_eq!(result[0].foreign_tax_offsets, Decimal::from(1000));
        assert_eq!(result[0].foreign_tax_offset_excess, Decimal::ZERO);
    }

    #[tokio::test]
    async fn db_foreign_tax_above_1000_is_capped_with_excess_surfaced() {
        let pool = test_pool().await;
        insert_listing(&pool, 1).await;
        // Anna-shaped total (docs/ato/fito-limit.md Example 16 pays A$3,400 foreign
        // tax; her computed limit is outside this system's data, so only the
        // A$1,000 de-minimis is claimable here).
        let mut inc = make_income(1, 1, NaiveDate::from_ymd_opt(2025, 3, 15).unwrap());
        inc.foreign_source_income = Decimal::from(12000);
        inc.foreign_tax_paid = Decimal::from(3400);
        income::db_upsert(&pool, &inc).await.unwrap();

        let result = db_tax_summary(&pool).await.unwrap();
        assert_eq!(result[0].foreign_tax_offsets, Decimal::from(1000));
        assert_eq!(result[0].foreign_tax_offset_excess, Decimal::from(2400));
    }

    /// The cap is a per-year total across sources: income foreign_tax_paid and
    /// AMMA foreign_tax_credits combine before the A$1,000 test, and each year
    /// is capped independently.
    #[tokio::test]
    async fn db_fito_cap_combines_income_and_amma_per_year() {
        let pool = test_pool().await;
        insert_listing(&pool, 1).await;
        insert_listing(&pool, 2).await;
        // FY2024: 600 (income) + 700 (AMMA) = 1300 → capped at 1000, excess 300.
        let mut inc = make_income(1, 1, NaiveDate::from_ymd_opt(2024, 3, 15).unwrap());
        inc.foreign_tax_paid = Decimal::from(600);
        income::db_upsert(&pool, &inc).await.unwrap();
        let mut a = make_amma(1, 2, NaiveDate::from_ymd_opt(2024, 6, 30).unwrap());
        a.foreign_tax_credits = Decimal::from(700);
        amma::db_upsert(&pool, &a).await.unwrap();
        // FY2025: 400 alone → under the cap, untouched.
        let mut inc2 = make_income(2, 1, NaiveDate::from_ymd_opt(2024, 9, 15).unwrap());
        inc2.foreign_tax_paid = Decimal::from(400);
        income::db_upsert(&pool, &inc2).await.unwrap();

        let result = db_tax_summary(&pool).await.unwrap();
        assert_eq!(result.len(), 2);
        assert_eq!(result[0].tax_year, 2024);
        assert_eq!(result[0].foreign_tax_offsets, Decimal::from(1000));
        assert_eq!(result[0].foreign_tax_offset_excess, Decimal::from(300));
        assert_eq!(result[1].tax_year, 2025);
        assert_eq!(result[1].foreign_tax_offsets, Decimal::from(400));
        assert_eq!(result[1].foreign_tax_offset_excess, Decimal::ZERO);
    }

    // API-level test

    #[tokio::test]
    async fn api_tax_summary_returns_json() {
        let pool = test_pool().await;
        insert_listing(&pool, 1).await;
        let mut inc = make_income(1, 1, NaiveDate::from_ymd_opt(2024, 3, 15).unwrap());
        inc.franked_amount = Decimal::from(70);
        inc.unfranked_amount = Decimal::from(30);
        inc.franking_credits = Decimal::from(30);
        income::db_upsert(&pool, &inc).await.unwrap();

        let resp = client(&pool).get("/portfolio/tax-summary").await;
        assert_eq!(resp.status, StatusCode::OK);
        let result: Vec<TaxYearSummary> = resp.json();
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].tax_year, 2024);
        assert_eq!(result[0].dividends_assessable, Decimal::from(100));
        assert_eq!(result[0].franking_credits, Decimal::from(30));
    }

    #[tokio::test]
    async fn api_export_returns_csv_with_expected_columns() {
        let pool = test_pool().await;
        insert_listing(&pool, 1).await;
        let mut inc = make_income(1, 1, NaiveDate::from_ymd_opt(2024, 3, 15).unwrap());
        inc.franked_amount = Decimal::from(71);
        inc.unfranked_amount = Decimal::from(29);
        inc.franking_credits = "30.50".parse().unwrap();
        income::db_upsert(&pool, &inc).await.unwrap();

        let resp = client(&pool).get("/portfolio/tax-summary/export").await;
        assert_eq!(resp.status, StatusCode::OK);
        assert_eq!(
            resp.headers.get(axum::http::header::CONTENT_TYPE).unwrap(),
            "text/csv; charset=utf-8"
        );
        assert_eq!(
            resp.headers
                .get(axum::http::header::CONTENT_DISPOSITION)
                .unwrap(),
            "attachment; filename=\"tax-summary.csv\""
        );
        let csv = resp.text().to_string();
        let mut lines = csv.lines();
        // Header names every TaxYearSummary field, in declaration order.
        assert_eq!(lines.next().unwrap(), CSV_HEADER.join(","));
        // Second header row: the ATO tax-return label per column, first cell
        // naming the form year the mapping targets.
        let labels = lines.next().unwrap();
        assert_eq!(labels, CSV_ATO_LABELS.join(","));
        assert!(labels.starts_with(&format!("{},", export::ATO_LABELS_MARKER)));
        // One record per tax year, decimal figures rendered exactly.
        let row = lines.next().unwrap();
        assert!(row.starts_with("2024,100,"));
        assert!(row.contains(",30.50,")); // franking_credits keeps its precision
        assert_eq!(lines.next(), None);
    }

    /// Each exported column's tax-return label sits under its column (same
    /// index in both rows): the headline figures map per
    /// docs/ato/tax-return-labels-2026.md.
    #[tokio::test]
    async fn db_ato_labels_align_with_their_columns() {
        assert_eq!(CSV_HEADER.len(), CSV_ATO_LABELS.len());
        let label_of = |col: &str| {
            let i = CSV_HEADER.iter().position(|c| *c == col).unwrap();
            CSV_ATO_LABELS[i]
        };
        assert_eq!(label_of("dividends_assessable"), "11S + 11T");
        assert_eq!(label_of("franking_credits"), "11U / 13Q");
        assert_eq!(label_of("amma_franked_dividends"), "13C");
        assert_eq!(label_of("amma_australian_interest"), "13U");
        assert_eq!(label_of("foreign_source_income"), "20E + 20M");
        assert_eq!(label_of("foreign_tax_offsets"), "20O");
        assert_eq!(label_of("interest_income"), "10L");
        assert_eq!(label_of("foreign_interest_income"), "20E + 20M");
        assert_eq!(label_of("tfn_withholding_tax"), "10M / 11V / 13R / 12C");
        assert_eq!(label_of("ess_discount_assessable"), "12B");
        assert_eq!(label_of("ess_foreign_source_discount"), "12A");
        assert_eq!(label_of("lic_capital_gain_deduction"), "D8");
        assert_eq!(label_of("deductions_total"), "D7 / D8");
        // Informational/derived columns report at no label.
        assert_eq!(label_of("franking_credits_denied"), "");
        assert_eq!(label_of("net_assessable_investment_income"), "");
        // Trust-level losses the AMMA's gains are already net of — not the
        // taxpayer's loss, so it feeds no question-18 figure
        // (docs/ato/personal-investors-guide-managed-fund-distributions.md).
        assert_eq!(label_of("amma_capital_losses_applied"), "");
        assert_eq!(label_of("taxpayer_basis"), "");
    }

    // ESS discount (docs/ato/employee-share-schemes.md): the assessable
    // discount is declared in the year of the taxing point, separately from
    // dividend income; the taxed-upfront-eligible total is reducible by up to
    // A$1,000 per year; the ESS TFN withheld joins the existing TFN line.

    /// Labels D + E + F + G total the assessable discount; with the eligible
    /// total over $1,000 the reduction is capped at $1,000 and surfaced.
    #[tokio::test]
    async fn db_ess_discount_totals_labels_net_of_1000_reduction() {
        let pool = test_pool().await;
        insert_listing(&pool, 1).await;
        // Taxing point Sep 2024 → FY2025.
        let mut e = make_ess(1, 1, NaiveDate::from_ymd_opt(2024, 9, 1).unwrap());
        e.taxed_upfront_eligible = Decimal::from(2400); // D
        e.taxed_upfront_not_eligible = Decimal::from(100); // E
        e.deferral_discount = Decimal::from(500); // F
        e.pre_2009_cessation_discount = Decimal::from(50); // G
        e.tfn_withholding = Decimal::from(30);
        ess_statement::db_upsert(&pool, &e).await.unwrap();

        let result = db_tax_summary(&pool).await.unwrap();
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].tax_year, 2025);
        // 2400 + 100 + 500 + 50 − 1000 = 2050.
        assert_eq!(result[0].ess_discount_assessable, Decimal::from(2050));
        assert_eq!(result[0].ess_taxed_upfront_reduction, Decimal::from(1000));
        // The ESS discount is not lumped into dividend income.
        assert_eq!(result[0].dividends_assessable, Decimal::ZERO);
        // ESS TFN joins the existing TFN line.
        assert_eq!(result[0].tfn_withholding_tax, Decimal::from(30));
    }

    /// An eligible discount of $1,000 or less is reduced by the whole of it (not
    /// a flat $1,000) — the reduction caps at the eligible total.
    #[tokio::test]
    async fn db_ess_reduction_caps_at_the_eligible_discount() {
        let pool = test_pool().await;
        insert_listing(&pool, 1).await;
        let mut e = make_ess(1, 1, NaiveDate::from_ymd_opt(2024, 9, 1).unwrap());
        e.taxed_upfront_eligible = Decimal::from(600); // D ≤ 1000
        e.deferral_discount = Decimal::from(900); // F
        ess_statement::db_upsert(&pool, &e).await.unwrap();

        let result = db_tax_summary(&pool).await.unwrap();
        // 600 + 900 − 600 = 900 (only the eligible 600 is removed).
        assert_eq!(result[0].ess_discount_assessable, Decimal::from(900));
        assert_eq!(result[0].ess_taxed_upfront_reduction, Decimal::from(600));
    }

    /// With no taxed-upfront-eligible discount there is no reduction; a pure
    /// deferral (RSU) statement is assessable in full.
    #[tokio::test]
    async fn db_ess_deferral_only_has_no_reduction() {
        let pool = test_pool().await;
        insert_listing(&pool, 1).await;
        let mut e = make_ess(1, 1, NaiveDate::from_ymd_opt(2024, 9, 1).unwrap());
        e.deferral_discount = Decimal::from(5000);
        ess_statement::db_upsert(&pool, &e).await.unwrap();

        let result = db_tax_summary(&pool).await.unwrap();
        assert_eq!(result[0].ess_discount_assessable, Decimal::from(5000));
        assert_eq!(result[0].ess_taxed_upfront_reduction, Decimal::ZERO);
    }

    /// The reduction is a per-year total across statements: two eligible
    /// discounts in the same year share one $1,000 cap.
    #[tokio::test]
    async fn db_ess_reduction_is_one_cap_per_year_across_statements() {
        let pool = test_pool().await;
        insert_listing(&pool, 1).await;
        let mut e1 = make_ess(1, 1, NaiveDate::from_ymd_opt(2024, 9, 1).unwrap());
        e1.taxed_upfront_eligible = Decimal::from(800);
        ess_statement::db_upsert(&pool, &e1).await.unwrap();
        let mut e2 = make_ess(2, 1, NaiveDate::from_ymd_opt(2025, 2, 1).unwrap());
        e2.taxed_upfront_eligible = Decimal::from(800);
        ess_statement::db_upsert(&pool, &e2).await.unwrap();

        let result = db_tax_summary(&pool).await.unwrap();
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].tax_year, 2025);
        // 1600 eligible − 1000 cap = 600.
        assert_eq!(result[0].ess_discount_assessable, Decimal::from(600));
        assert_eq!(result[0].ess_taxed_upfront_reduction, Decimal::from(1000));
    }

    /// The foreign-source discount is a memo already within the assessable
    /// total — surfaced separately, never added on top. Non-AUD statements
    /// convert via the ATO rate for the taxing-point month.
    #[tokio::test]
    async fn db_ess_foreign_source_converted_and_not_double_counted() {
        let pool = test_pool().await;
        insert_listing(&pool, 1).await;
        // A$1 = 0.50 USD for Sep 2024 → AUD = USD / 0.50.
        rba_fx_rate::db_import_rate(&pool, "USD", "2024-09", "0.50".parse().unwrap())
            .await
            .unwrap();
        let mut e = make_ess(1, 1, NaiveDate::from_ymd_opt(2024, 9, 1).unwrap());
        e.currency = "USD".to_string();
        e.deferral_discount = Decimal::from(1000); // USD → 2000 AUD
        e.foreign_source_discount = Decimal::from(1000); // the whole discount is foreign
        ess_statement::db_upsert(&pool, &e).await.unwrap();

        let result = db_tax_summary(&pool).await.unwrap();
        // 1000 / 0.50 = 2000 assessable; foreign memo 2000; not 4000.
        assert_eq!(result[0].ess_discount_assessable, Decimal::from(2000));
        assert_eq!(result[0].ess_foreign_source_discount, Decimal::from(2000));
    }

    /// A statement-AUD override is reported verbatim — the employer's stated
    /// AUD figure (release-date spot), not the RBA monthly conversion — while
    /// labels without an override keep converting via the RBA rate.
    #[tokio::test]
    async fn db_ess_statement_aud_override_reported_verbatim() {
        let pool = test_pool().await;
        insert_listing(&pool, 1).await;
        // A$1 = 0.50 USD for Sep 2024 → RBA conversion would double the figure.
        rba_fx_rate::db_import_rate(&pool, "USD", "2024-09", "0.50".parse().unwrap())
            .await
            .unwrap();
        let mut e = make_ess(1, 1, NaiveDate::from_ymd_opt(2024, 9, 1).unwrap());
        e.currency = "USD".to_string();
        e.deferral_discount = Decimal::from(1000); // USD; RBA would say 2000 AUD
        e.aud_deferral_discount = Some("1403.40".parse().unwrap()); // employer's AUD figure
        e.foreign_source_discount = Decimal::from(1000); // no override → RBA converts
        ess_statement::db_upsert(&pool, &e).await.unwrap();

        let result = db_tax_summary(&pool).await.unwrap();
        // The override verbatim, not 2000; the un-overridden memo still converts.
        assert_eq!(
            result[0].ess_discount_assessable,
            "1403.40".parse::<Decimal>().unwrap()
        );
        assert_eq!(result[0].ess_foreign_source_discount, Decimal::from(2000));
    }

    /// The $1,000 taxed-upfront reduction is computed on the overridden AUD
    /// eligible figure when one is stated.
    #[tokio::test]
    async fn db_ess_aud_override_drives_the_taxed_upfront_reduction() {
        let pool = test_pool().await;
        insert_listing(&pool, 1).await;
        rba_fx_rate::db_import_rate(&pool, "USD", "2024-09", "0.50".parse().unwrap())
            .await
            .unwrap();
        let mut e = make_ess(1, 1, NaiveDate::from_ymd_opt(2024, 9, 1).unwrap());
        e.currency = "USD".to_string();
        e.taxed_upfront_eligible = Decimal::from(1000); // USD; RBA would say 2000 AUD
        e.aud_taxed_upfront_eligible = Some(Decimal::from(800)); // employer's AUD figure
        ess_statement::db_upsert(&pool, &e).await.unwrap();

        let result = db_tax_summary(&pool).await.unwrap();
        // Eligible = 800 (the override), under the cap → reduced by all of it
        // (the RBA figure of 2000 would have capped the reduction at 1,000).
        assert_eq!(result[0].ess_taxed_upfront_reduction, Decimal::from(800));
        assert_eq!(result[0].ess_discount_assessable, Decimal::ZERO);
    }

    /// Live-data acceptance (REQUIREMENTS 2026-06-12, ESS statement AUD
    /// override): the real RSU release facts with the employer's stated AUD
    /// figures entered as statement-AUD overrides reproduce the ATO ESS
    /// statements exactly — FY2022 10,572; FY2023 9,443; FY2024 11,731;
    /// FY2025 13,526 (the figures the prefilled return carries) — while the
    /// year whose annual employer statement hasn't been issued yet (FY2026)
    /// keeps the RBA monthly conversion.
    #[tokio::test]
    async fn db_ess_aud_overrides_reproduce_the_employer_ess_statements() {
        let pool = test_pool().await;
        insert_listing(&pool, 1).await;
        // (taxing point, USD deferral discount, employer-stated AUD figure)
        let releases: [(NaiveDate, &str, Option<&str>); 5] = [
            (ymd(2022, 2, 7), "7533.12", Some("10572")),
            (ymd(2023, 2, 6), "6499.20", Some("9443")),
            (ymd(2024, 2, 5), "7605.00", Some("11731")),
            (ymd(2025, 2, 12), "8513.94", Some("13526")),
            (ymd(2026, 2, 12), "7903.48", None), // annual statement not yet issued
        ];
        for (i, (taxing_point, usd, aud)) in releases.iter().enumerate() {
            let month = format!("{}", taxing_point.format("%Y-%m"));
            rba_fx_rate::db_import_rate(&pool, "USD", &month, "0.64".parse().unwrap())
                .await
                .unwrap();
            let mut e = make_ess(i as i64 + 1, 1, *taxing_point);
            e.currency = "USD".to_string();
            e.deferral_discount = usd.parse().unwrap();
            e.aud_deferral_discount = aud.map(|a| a.parse().unwrap());
            ess_statement::db_upsert(&pool, &e).await.unwrap();
        }

        let result = db_tax_summary(&pool).await.unwrap();
        let by_year: HashMap<i32, &TaxYearSummary> =
            result.iter().map(|s| (s.tax_year, s)).collect();
        for (fy, expected) in [
            (2022, "10572"),
            (2023, "9443"),
            (2024, "11731"),
            (2025, "13526"),
        ] {
            assert_eq!(
                by_year[&fy].ess_discount_assessable,
                expected.parse::<Decimal>().unwrap(),
                "FY{fy} must equal the ATO ESS statement verbatim"
            );
        }
        // FY2026 has no override → RBA conversion (7903.48 / 0.64).
        assert_eq!(
            by_year[&2026].ess_discount_assessable,
            "12349.1875".parse::<Decimal>().unwrap()
        );
    }

    /// A non-AUD ESS statement with no ATO rate fails loudly (no silent zero).
    #[tokio::test]
    async fn db_ess_non_aud_without_rate_fails_loudly() {
        let pool = test_pool().await;
        insert_listing(&pool, 1).await;
        let mut e = make_ess(1, 1, NaiveDate::from_ymd_opt(2024, 9, 1).unwrap());
        e.currency = "USD".to_string();
        e.deferral_discount = Decimal::from(1000);
        ess_statement::db_upsert(&pool, &e).await.unwrap();
        assert!(db_tax_summary(&pool).await.is_err());
    }

    // Deductible investment expenses (docs/ato/investment-income-deductions.md,
    // dividend-income-deductions.md): netted against gross assessable investment
    // income per Australian financial year, totalled by expense type and overall.

    use investment_expense::ExpenseType;

    /// Gross assessable investment income sums the existing assessable lines;
    /// the deductions net it to a separate "net assessable" figure, by type and
    /// overall, with the gross figures retained.
    #[tokio::test]
    async fn db_deductions_net_gross_assessable_investment_income() {
        let pool = test_pool().await;
        insert_listing(&pool, 1).await;
        // FY2024 dividend income: 140 franked + 60 unfranked = 200 assessable.
        let mut div = make_income(1, 1, NaiveDate::from_ymd_opt(2024, 3, 15).unwrap());
        div.franked_amount = Decimal::from(140);
        div.unfranked_amount = Decimal::from(60);
        div.franking_credits = Decimal::from(60); // an offset line, not in gross
        income::db_upsert(&pool, &div).await.unwrap();
        // Loan interest 500 + management fee 120 = 620 deductions.
        investment_expense::db_upsert(
            &pool,
            &make_expense(
                1,
                NaiveDate::from_ymd_opt(2024, 2, 1).unwrap(),
                ExpenseType::LoanInterest,
                Decimal::from(500),
            ),
        )
        .await
        .unwrap();
        investment_expense::db_upsert(
            &pool,
            &make_expense(
                2,
                NaiveDate::from_ymd_opt(2024, 5, 1).unwrap(),
                ExpenseType::ManagementFee,
                Decimal::from(120),
            ),
        )
        .await
        .unwrap();

        let result = db_tax_summary(&pool).await.unwrap();
        assert_eq!(result.len(), 1);
        let s = &result[0];
        assert_eq!(s.tax_year, 2024);
        // Gross is the cash assessable lines only (franking credits excluded).
        assert_eq!(s.gross_assessable_investment_income, Decimal::from(200));
        assert_eq!(s.deductions_loan_interest, Decimal::from(500));
        assert_eq!(s.deductions_management_fee, Decimal::from(120));
        assert_eq!(s.deductions_total, Decimal::from(620));
        // Net = 200 − 620 = −420 (deductions can exceed income → a loss).
        assert_eq!(s.net_assessable_investment_income, Decimal::from(-420));
        // The gross dividend line is retained unchanged.
        assert_eq!(s.dividends_assessable, Decimal::from(200));
    }

    /// Gross assessable investment income includes the foreign-source income and
    /// the AMMA income components, but not conduit foreign income (NANE), the
    /// franking-credit gross-up, the ESS discount, or capital gains.
    #[tokio::test]
    async fn db_gross_assessable_spans_income_and_amma_excludes_nane_and_cgt() {
        let pool = test_pool().await;
        insert_listing(&pool, 1).await;
        let mut inc = make_income(1, 1, NaiveDate::from_ymd_opt(2024, 3, 15).unwrap());
        inc.unfranked_amount = Decimal::from(100);
        inc.foreign_source_income = Decimal::from(50);
        inc.conduit_foreign_income = Decimal::from(40); // NANE, excluded
        income::db_upsert(&pool, &inc).await.unwrap();
        let mut a = make_amma(1, 1, NaiveDate::from_ymd_opt(2024, 6, 30).unwrap());
        a.australian_interest = Decimal::from(10);
        a.franked_dividends = Decimal::from(20);
        a.foreign_income = Decimal::from(5);
        a.other_income = Decimal::from(3);
        a.cgt_discount_gains = Decimal::from(1000); // a capital gain, excluded
        amma::db_upsert(&pool, &a).await.unwrap();

        let result = db_tax_summary(&pool).await.unwrap();
        // 100 + 50 + 10 + 20 + 5 + 3 = 188 (conduit and CGT excluded).
        assert_eq!(
            result[0].gross_assessable_investment_income,
            Decimal::from(188)
        );
        // No deductions → net equals gross.
        assert_eq!(
            result[0].net_assessable_investment_income,
            Decimal::from(188)
        );
        assert_eq!(result[0].deductions_total, Decimal::ZERO);
    }

    /// Each expense type totals into its own line, and into the overall total.
    #[tokio::test]
    async fn db_deductions_totalled_by_type() {
        let pool = test_pool().await;
        let d = NaiveDate::from_ymd_opt(2024, 3, 1).unwrap();
        let kinds = [
            (ExpenseType::LoanInterest, 100),
            (ExpenseType::ManagementFee, 200),
            (ExpenseType::AdviceFee, 300),
            (ExpenseType::AccountKeepingFee, 40),
            (ExpenseType::Subscription, 50),
            (ExpenseType::Other, 6),
        ];
        for (i, (kind, amt)) in kinds.iter().enumerate() {
            investment_expense::db_upsert(
                &pool,
                &make_expense(i as i64 + 1, d, *kind, Decimal::from(*amt)),
            )
            .await
            .unwrap();
        }

        let result = db_tax_summary(&pool).await.unwrap();
        assert_eq!(result.len(), 1);
        let s = &result[0];
        assert_eq!(s.deductions_loan_interest, Decimal::from(100));
        assert_eq!(s.deductions_management_fee, Decimal::from(200));
        assert_eq!(s.deductions_advice_fee, Decimal::from(300));
        assert_eq!(s.deductions_account_keeping_fee, Decimal::from(40));
        assert_eq!(s.deductions_subscription, Decimal::from(50));
        assert_eq!(s.deductions_other, Decimal::from(6));
        assert_eq!(s.deductions_total, Decimal::from(696));
        // No assessable income → net is the negative of the deductions.
        assert_eq!(s.gross_assessable_investment_income, Decimal::ZERO);
        assert_eq!(s.net_assessable_investment_income, Decimal::from(-696));
    }

    /// Expenses are attributed to the financial year of the date incurred (a July
    /// date belongs to the next FY), independently per year.
    #[tokio::test]
    async fn db_deductions_attributed_by_financial_year() {
        let pool = test_pool().await;
        // June 2024 → FY2024; July 2024 → FY2025.
        investment_expense::db_upsert(
            &pool,
            &make_expense(
                1,
                NaiveDate::from_ymd_opt(2024, 6, 30).unwrap(),
                ExpenseType::LoanInterest,
                Decimal::from(100),
            ),
        )
        .await
        .unwrap();
        investment_expense::db_upsert(
            &pool,
            &make_expense(
                2,
                NaiveDate::from_ymd_opt(2024, 7, 1).unwrap(),
                ExpenseType::LoanInterest,
                Decimal::from(200),
            ),
        )
        .await
        .unwrap();

        let result = db_tax_summary(&pool).await.unwrap();
        assert_eq!(result.len(), 2);
        assert_eq!(result[0].tax_year, 2024);
        assert_eq!(result[0].deductions_total, Decimal::from(100));
        assert_eq!(result[1].tax_year, 2025);
        assert_eq!(result[1].deductions_total, Decimal::from(200));
    }

    /// The documented workaround for an expense the ATO spreads across years
    /// (SCENARIOS H-08): a $2,000 loan establishment fee is apportioned over 5
    /// years (s 25-25, `docs/ato/expense-time-apportionment.md`), and this
    /// model has one date per row — so it is entered as five rows of $400, one
    /// per financial year, and each year deducts its own share and no more.
    /// The single-row entry the convention exists to steer away from is pinned
    /// alongside it: nothing refuses it, and it claims all five years at once.
    #[tokio::test]
    async fn db_a_multi_year_expense_deducts_per_year_when_entered_per_year() {
        let pool = test_pool().await;
        // The fee is incurred on 1 August 2024 (FY2025) and spread over
        // FY2025–FY2029: one row per year, each carrying that year's $400.
        for (i, year) in (2024..=2028).enumerate() {
            investment_expense::db_upsert(
                &pool,
                &make_expense(
                    i as i64 + 1,
                    NaiveDate::from_ymd_opt(year, 8, 1).unwrap(),
                    ExpenseType::Other,
                    Decimal::from(400),
                ),
            )
            .await
            .unwrap();
        }

        let result = db_tax_summary(&pool).await.unwrap();
        assert_eq!(result.len(), 5);
        for (i, s) in result.iter().enumerate() {
            assert_eq!(s.tax_year, 2025 + i as i32);
            assert_eq!(s.deductions_other, Decimal::from(400));
            assert_eq!(s.deductions_total, Decimal::from(400));
        }

        // Keyed as one row instead, the whole fee lands in the first year —
        // five years' deduction at once, accepted without complaint. That is
        // the limitation `docs/API.md` documents, not a computed apportionment.
        let pool = test_pool().await;
        investment_expense::db_upsert(
            &pool,
            &make_expense(
                1,
                NaiveDate::from_ymd_opt(2024, 8, 1).unwrap(),
                ExpenseType::Other,
                Decimal::from(2000),
            ),
        )
        .await
        .unwrap();
        let result = db_tax_summary(&pool).await.unwrap();
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].tax_year, 2025);
        assert_eq!(result[0].deductions_total, Decimal::from(2000));
    }

    /// A non-AUD expense converts to AUD via the ATO rate for the month incurred.
    #[tokio::test]
    async fn db_non_aud_expense_converted_to_aud() {
        let pool = test_pool().await;
        // A$1 = 0.50 USD for Mar 2024 → AUD = USD / 0.50.
        rba_fx_rate::db_import_rate(&pool, "USD", "2024-03", "0.50".parse().unwrap())
            .await
            .unwrap();
        let mut e = make_expense(
            1,
            NaiveDate::from_ymd_opt(2024, 3, 15).unwrap(),
            ExpenseType::AdviceFee,
            Decimal::from(100),
        );
        e.currency = "USD".to_string();
        investment_expense::db_upsert(&pool, &e).await.unwrap();

        let result = db_tax_summary(&pool).await.unwrap();
        // 100 / 0.50 = 200 AUD.
        assert_eq!(result[0].deductions_advice_fee, Decimal::from(200));
        assert_eq!(result[0].deductions_total, Decimal::from(200));
    }

    /// SCENARIOS H-06/H-09: a deduction alone can no longer lift the net
    /// assessable line above the gross. The scenario keyed a `-500` `Other`
    /// expense beside a legitimate `+5` loan-interest row, which reported
    /// `deductions_total` `-495` and `net_assessable_investment_income` `495`
    /// on a year whose gross was `0` — a negative deduction is arithmetically
    /// income. The write is now refused at the entity, so the year that
    /// reaches the report carries only the real expense and its net line sits
    /// below its gross.
    #[tokio::test]
    async fn db_a_deduction_alone_cannot_lift_the_net_line_above_the_gross() {
        let pool = test_pool().await;
        let d = ymd(2026, 3, 15);
        investment_expense::db_upsert(
            &pool,
            &make_expense(1, d, ExpenseType::LoanInterest, Decimal::from(5)),
        )
        .await
        .unwrap();
        assert!(
            investment_expense::db_upsert(
                &pool,
                &make_expense(2, d, ExpenseType::Other, Decimal::from(-500)),
            )
            .await
            .is_err(),
            "a negative expense must never reach the report"
        );

        let s = &db_tax_summary(&pool).await.unwrap()[0];
        assert_eq!(s.gross_assessable_investment_income, Decimal::ZERO);
        assert_eq!(s.deductions_other, Decimal::ZERO);
        assert_eq!(s.deductions_total, Decimal::from(5));
        assert_eq!(s.net_assessable_investment_income, Decimal::from(-5));
        assert!(
            s.net_assessable_investment_income <= s.gross_assessable_investment_income,
            "deductions can only reduce the net line, never lift it above the gross"
        );
    }

    /// A non-AUD expense with no ATO rate fails loudly (no silent zero).
    #[tokio::test]
    async fn db_non_aud_expense_without_rate_fails_loudly() {
        let pool = test_pool().await;
        let mut e = make_expense(
            1,
            NaiveDate::from_ymd_opt(2024, 3, 15).unwrap(),
            ExpenseType::LoanInterest,
            Decimal::from(100),
        );
        e.currency = "USD".to_string();
        investment_expense::db_upsert(&pool, &e).await.unwrap();
        assert!(db_tax_summary(&pool).await.is_err());
    }

    // Interest income (docs/ato/tax-return-labels-2026.md question 10): the
    // gross interest (10L) is its own per-FY line inside gross assessable
    // investment income; the TFN amount withheld (10M) joins the combined
    // withholding line.

    fn make_interest(id: i64, date: NaiveDate, amount: Decimal) -> interest_income::InterestIncome {
        interest_income::InterestIncome {
            id,
            date_paid: date,
            amount,
            tfn_withholding_tax: Decimal::ZERO,
            foreign_source: false,
            foreign_tax_paid: Decimal::ZERO,
            currency: "AUD".to_string(),
            source: None,
            holding_account_id: None,
        }
    }

    /// Interest is attributed to the financial year of the payment date (a
    /// July date belongs to the next FY), independently per year.
    #[tokio::test]
    async fn db_interest_aggregated_by_financial_year() {
        let pool = test_pool().await;
        // June 2024 → FY2024; July 2024 → FY2025.
        interest_income::db_upsert(
            &pool,
            &make_interest(
                1,
                NaiveDate::from_ymd_opt(2024, 6, 30).unwrap(),
                Decimal::from(100),
            ),
        )
        .await
        .unwrap();
        interest_income::db_upsert(
            &pool,
            &make_interest(
                2,
                NaiveDate::from_ymd_opt(2024, 7, 1).unwrap(),
                Decimal::from(200),
            ),
        )
        .await
        .unwrap();

        let result = db_tax_summary(&pool).await.unwrap();
        assert_eq!(result.len(), 2);
        assert_eq!(result[0].tax_year, 2024);
        assert_eq!(result[0].interest_income, Decimal::from(100));
        assert_eq!(result[1].tax_year, 2025);
        assert_eq!(result[1].interest_income, Decimal::from(200));
        // Interest is not lumped into the dividend line.
        assert_eq!(result[0].dividends_assessable, Decimal::ZERO);
    }

    /// The gross/net identity holds with interest in it: gross assessable
    /// investment income includes the interest line, and net subtracts the
    /// deductions from that gross.
    #[tokio::test]
    async fn db_interest_included_in_gross_and_net_assessable() {
        let pool = test_pool().await;
        insert_listing(&pool, 1).await;
        let mut div = make_income(1, 1, NaiveDate::from_ymd_opt(2024, 3, 15).unwrap());
        div.unfranked_amount = Decimal::from(100);
        income::db_upsert(&pool, &div).await.unwrap();
        interest_income::db_upsert(
            &pool,
            &make_interest(
                1,
                NaiveDate::from_ymd_opt(2024, 2, 1).unwrap(),
                Decimal::from(80),
            ),
        )
        .await
        .unwrap();
        investment_expense::db_upsert(
            &pool,
            &make_expense(
                1,
                NaiveDate::from_ymd_opt(2024, 5, 1).unwrap(),
                ExpenseType::LoanInterest,
                Decimal::from(30),
            ),
        )
        .await
        .unwrap();

        let result = db_tax_summary(&pool).await.unwrap();
        assert_eq!(result.len(), 1);
        let s = &result[0];
        assert_eq!(s.interest_income, Decimal::from(80));
        // Gross = 100 dividends + 80 interest; net = gross − 30 deductions.
        assert_eq!(s.gross_assessable_investment_income, Decimal::from(180));
        assert_eq!(s.net_assessable_investment_income, Decimal::from(150));
    }

    /// The TFN amount withheld from interest joins the combined withholding
    /// line, while the gross interest line keeps the full (gross) figure.
    #[tokio::test]
    async fn db_interest_tfn_withholding_joins_the_withholding_line() {
        let pool = test_pool().await;
        insert_listing(&pool, 1).await;
        let mut div = make_income(1, 1, NaiveDate::from_ymd_opt(2024, 3, 15).unwrap());
        div.tfn_withholding_tax = Decimal::from(5);
        income::db_upsert(&pool, &div).await.unwrap();
        let mut int = make_interest(
            1,
            NaiveDate::from_ymd_opt(2024, 2, 1).unwrap(),
            Decimal::from(100),
        );
        int.tfn_withholding_tax = Decimal::from(47);
        interest_income::db_upsert(&pool, &int).await.unwrap();

        let result = db_tax_summary(&pool).await.unwrap();
        assert_eq!(result[0].tfn_withholding_tax, Decimal::from(52)); // 5 + 47
        assert_eq!(result[0].interest_income, Decimal::from(100)); // stays gross
    }

    /// A foreign-source interest row (docs/ato/tax-return-labels-2026.md,
    /// question 20 — e.g. a US broker's money-market sweep fund) reports as
    /// assessable foreign source income (`foreign_interest_income`, 20E), not
    /// on the Australian 10L line; its foreign tax withheld joins the FITO
    /// line; and it still counts in gross assessable investment income.
    #[tokio::test]
    async fn db_foreign_source_interest_reports_at_20e_with_fito() {
        let pool = test_pool().await;
        // Australian savings interest stays at 10L.
        interest_income::db_upsert(
            &pool,
            &make_interest(
                1,
                NaiveDate::from_ymd_opt(2024, 2, 1).unwrap(),
                Decimal::from(100),
            ),
        )
        .await
        .unwrap();
        // Foreign broker-cash interest with US withholding.
        let mut foreign = make_interest(
            2,
            NaiveDate::from_ymd_opt(2024, 3, 15).unwrap(),
            Decimal::from(80),
        );
        foreign.foreign_source = true;
        foreign.foreign_tax_paid = Decimal::from(12);
        interest_income::db_upsert(&pool, &foreign).await.unwrap();

        let result = db_tax_summary(&pool).await.unwrap();
        assert_eq!(result.len(), 1);
        let s = &result[0];
        assert_eq!(s.interest_income, Decimal::from(100)); // 10L: Australian only
        assert_eq!(s.foreign_interest_income, Decimal::from(80)); // 20E
        assert_eq!(s.foreign_tax_offsets, Decimal::from(12)); // 20O
        assert_eq!(
            s.gross_assessable_investment_income,
            Decimal::from(180) // both classifications are assessable income
        );
    }

    /// Foreign tax withheld from interest counts toward the A$1,000 FITO
    /// de-minimis like any other foreign tax: the claimable offset caps and
    /// the excess is surfaced (docs/ato/fito-limit.md).
    #[tokio::test]
    async fn db_foreign_interest_tax_subject_to_fito_cap() {
        let pool = test_pool().await;
        let mut foreign = make_interest(
            1,
            NaiveDate::from_ymd_opt(2024, 3, 15).unwrap(),
            Decimal::from(5000),
        );
        foreign.foreign_source = true;
        foreign.foreign_tax_paid = Decimal::from(1300);
        interest_income::db_upsert(&pool, &foreign).await.unwrap();

        let result = db_tax_summary(&pool).await.unwrap();
        assert_eq!(result[0].foreign_tax_offsets, Decimal::from(1000));
        assert_eq!(result[0].foreign_tax_offset_excess, Decimal::from(300));
    }

    /// A non-AUD foreign-source amount converts to AUD via the ATO rate for
    /// the month paid — the gross and the foreign tax withheld both.
    #[tokio::test]
    async fn db_non_aud_foreign_interest_converted_to_aud() {
        let pool = test_pool().await;
        // A$1 = 0.50 USD for Mar 2024 → AUD = USD / 0.50.
        rba_fx_rate::db_import_rate(&pool, "USD", "2024-03", "0.50".parse().unwrap())
            .await
            .unwrap();
        let mut foreign = make_interest(
            1,
            NaiveDate::from_ymd_opt(2024, 3, 15).unwrap(),
            Decimal::from(100),
        );
        foreign.foreign_source = true;
        foreign.foreign_tax_paid = Decimal::from(15);
        foreign.currency = "USD".to_string();
        interest_income::db_upsert(&pool, &foreign).await.unwrap();

        let result = db_tax_summary(&pool).await.unwrap();
        assert_eq!(result[0].foreign_interest_income, Decimal::from(200)); // 100 / 0.50
        assert_eq!(result[0].foreign_tax_offsets, Decimal::from(30)); // 15 / 0.50
    }

    /// A non-AUD interest amount converts to AUD via the ATO rate for the
    /// month paid — both the gross and the withheld amount.
    #[tokio::test]
    async fn db_non_aud_interest_converted_to_aud() {
        let pool = test_pool().await;
        // A$1 = 0.50 USD for Mar 2024 → AUD = USD / 0.50.
        rba_fx_rate::db_import_rate(&pool, "USD", "2024-03", "0.50".parse().unwrap())
            .await
            .unwrap();
        let mut int = make_interest(
            1,
            NaiveDate::from_ymd_opt(2024, 3, 15).unwrap(),
            Decimal::from(100),
        );
        int.currency = "USD".to_string();
        int.tfn_withholding_tax = Decimal::from(10);
        interest_income::db_upsert(&pool, &int).await.unwrap();

        let result = db_tax_summary(&pool).await.unwrap();
        assert_eq!(result[0].interest_income, Decimal::from(200)); // 100 / 0.50
        assert_eq!(result[0].tfn_withholding_tax, Decimal::from(20)); // 10 / 0.50
    }

    /// A non-AUD interest amount with no ATO rate fails loudly (no silent zero).
    #[tokio::test]
    async fn db_non_aud_interest_without_rate_fails_loudly() {
        let pool = test_pool().await;
        let mut int = make_interest(
            1,
            NaiveDate::from_ymd_opt(2024, 3, 15).unwrap(),
            Decimal::from(100),
        );
        int.currency = "USD".to_string();
        interest_income::db_upsert(&pool, &int).await.unwrap();
        assert!(db_tax_summary(&pool).await.is_err());
    }

    /// SCENARIOS H-05: interest belongs to the year it is **credited**, not
    /// the year the money becomes reachable — "You must declare interest
    /// income in the year it is credited, received or applied or dealt with in
    /// any way on your behalf or as you direct" (ATO, *Investment income*,
    /// QC 72101, retrieved 2026-08-17). A term deposit crediting $500 on
    /// 30 June 2026 whose funds are only available on 2 July is FY2026 income,
    /// and `date_paid` is the single date the row records: keying the
    /// availability date instead moves the whole amount into FY2027 (the
    /// second row here), which is what the entry convention has to prevent.
    #[tokio::test]
    async fn db_interest_is_assessed_in_the_year_it_is_credited() {
        let pool = test_pool().await;
        // Credited 30 June 2026 — FY2026, even though the funds are only
        // reachable on 2 July.
        interest_income::db_upsert(
            &pool,
            &make_interest(
                1,
                NaiveDate::from_ymd_opt(2026, 6, 30).unwrap(),
                Decimal::from(500),
            ),
        )
        .await
        .unwrap();
        // The same interest keyed at the availability date lands a year later.
        interest_income::db_upsert(
            &pool,
            &make_interest(
                2,
                NaiveDate::from_ymd_opt(2026, 7, 2).unwrap(),
                Decimal::from(500),
            ),
        )
        .await
        .unwrap();

        let result = db_tax_summary(&pool).await.unwrap();
        assert_eq!(result.len(), 2);
        assert_eq!(result[0].tax_year, 2026);
        assert_eq!(result[0].interest_income, Decimal::from(500));
        assert_eq!(result[1].tax_year, 2027);
        assert_eq!(result[1].interest_income, Decimal::from(500));
    }

    /// The interest lines (Australian and foreign-source) ship in the CSV export.
    #[tokio::test]
    async fn db_csv_header_carries_interest_column() {
        assert!(CSV_HEADER.contains(&"interest_income"));
        assert!(CSV_HEADER.contains(&"foreign_interest_income"));
    }

    /// The new deduction columns ship in the CSV export.
    #[tokio::test]
    async fn db_csv_header_carries_deduction_columns() {
        assert!(CSV_HEADER.contains(&"gross_assessable_investment_income"));
        assert!(CSV_HEADER.contains(&"deductions_loan_interest"));
        assert!(CSV_HEADER.contains(&"deductions_total"));
        assert!(CSV_HEADER.contains(&"net_assessable_investment_income"));
    }

    #[tokio::test]
    async fn api_export_of_empty_report_still_returns_header() {
        let pool = test_pool().await;
        let resp = client(&pool).get("/portfolio/tax-summary/export").await;
        assert_eq!(resp.status, StatusCode::OK);
        let csv = resp.text().to_string();
        assert_eq!(
            csv,
            CSV_HEADER.join(",") + "\n" + &CSV_ATO_LABELS.join(",") + "\n"
        );
    }
}
