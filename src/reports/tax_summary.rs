use crate::infra::decimal::parse_dec;
use crate::infra::fx::to_aud;
use crate::reports::{export, franking};
use axum::{
    Json, Router, extract::State, http::StatusCode, response::Response, routing::get,
};
use chrono::{Datelike, NaiveDate};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use sqlx::{Row, SqlitePool};
use std::collections::HashMap;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaxYearSummary {
    /// Australian tax year: the calendar year in which June 30 falls (e.g. 2024 = FY2023/24).
    pub tax_year: i32,
    /// Assessable dividend income: franked_amount + unfranked_amount from income records.
    pub dividends_assessable: Decimal,
    /// Assessable foreign source income (conduit foreign income excluded).
    pub foreign_source_income: Decimal,
    /// LIC capital gain deduction from income records.
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
    /// AMMA capital losses applied.
    pub amma_capital_losses_applied: Decimal,
    /// Claimable franking credits (income + AMMA). Credits attached to a
    /// dividend whose shares fail the 45-day at-risk holding-period rule (90
    /// days for preference shares) are excluded, unless the small-shareholder
    /// exemption applies — total attached credits in the year below A$5,000
    /// (see `reports::franking`).
    pub franking_credits: Decimal,
    /// Franking credits attached but denied by the holding-period rule (the
    /// amount excluded from `franking_credits`).
    pub franking_credits_denied: Decimal,
    /// Claimable foreign income tax offset (income foreign_tax_paid + AMMA
    /// foreign_tax_credits), capped at the A$1,000 FITO de-minimis: above that
    /// the ATO requires the offset-limit calculation, which needs the
    /// taxpayer's full income-tax position and is outside this system's data
    /// (see `docs/fito-limit.md`).
    pub foreign_tax_offsets: Decimal,
    /// Foreign tax paid above the A$1,000 de-minimis (the amount excluded from
    /// `foreign_tax_offsets`). Claimable only to the extent the taxpayer's own
    /// offset-limit calculation supports it.
    pub foreign_tax_offset_excess: Decimal,
    /// Total TFN withholding tax (income + AMMA).
    pub tfn_withholding_tax: Decimal,
    /// Informational: the taxpayer assumption behind the hard-wired rates
    /// (always [`crate::reports::TAXPAYER_BASIS`]) — the LIC capital gain
    /// deduction passed through here is the Australian-resident-individual 50%
    /// figure from the income record; other entity types are not modelled.
    pub taxpayer_basis: String,
}

pub fn router() -> Router<SqlitePool> {
    Router::new()
        .route("/portfolio/tax-summary", get(tax_summary_handler))
        .route("/portfolio/tax-summary/export", get(tax_summary_export_handler))
}

/// CSV export columns — `TaxYearSummary`'s fields in declaration order. The csv
/// writer rejects a record whose length differs from this header (see
/// `reports::export`), so a drift between the two fails loudly.
const CSV_HEADER: &[&str] = &[
    "tax_year",
    "dividends_assessable",
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
    "taxpayer_basis",
];

fn zero_summary(tax_year: i32) -> TaxYearSummary {
    TaxYearSummary {
        tax_year,
        dividends_assessable: Decimal::ZERO,
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
        taxpayer_basis: crate::reports::TAXPAYER_BASIS.to_string(),
    }
}

/// FITO de-minimis (docs/fito-limit.md): up to A$1,000 of foreign income tax
/// paid in a year is claimable without working out the offset limit.
fn fito_de_minimis_aud() -> Decimal {
    Decimal::from(1000)
}

/// Read a TEXT decimal column from `row` and convert it to AUD via the ATO rate
/// for `currency` and the month of `date`. Income and AMMA records carry no manual
/// fx override, so a non-AUD amount with no ATO rate fails loudly (the `FxError`
/// surfaces as a decode error) rather than being passed through or zeroed.
async fn aud_field(
    pool: &SqlitePool,
    row: &sqlx::sqlite::SqliteRow,
    field: &str,
    currency: &str,
    date: NaiveDate,
) -> Result<Decimal, sqlx::Error> {
    let value = parse_dec(field, row.try_get(field)?)?;
    Ok(to_aud(pool, value, currency, date, None).await?)
}

pub async fn db_tax_summary(pool: &SqlitePool) -> Result<Vec<TaxYearSummary>, sqlx::Error> {
    let income_rows = sqlx::query(
        "SELECT listing_id, date_paid, ex_date, franked_amount, unfranked_amount, \
         foreign_source_income, foreign_tax_paid, tfn_withholding_tax, franking_credits, \
         lic_capital_gain_deduction, currency \
         FROM income",
    )
    .fetch_all(pool)
    .await?;

    let amma_rows = sqlx::query(
        "SELECT tax_year_end_date, australian_interest, australian_dividends_unfranked, \
         franked_dividends, franking_credits, net_rent, foreign_income, foreign_tax_credits, \
         other_income, cgt_discount_gains, cgt_indexation_gains, cgt_other_gains, \
         capital_losses_applied, tfn_withholding_tax, currency \
         FROM amma_statements",
    )
    .fetch_all(pool)
    .await?;

    let mut map: HashMap<i32, TaxYearSummary> = HashMap::new();

    // Per-dividend candidates for the franking holding-period rule, and the
    // year's total attached credits (income + AMMA, AUD) for the
    // small-shareholder exemption test.
    struct FrankedDividend {
        tax_year: i32,
        listing_id: i64,
        /// Ex-dividend date; falls back to the payment date when not recorded
        /// (a dividend is never paid before its shares go ex-dividend).
        ex_date: NaiveDate,
        credits_aud: Decimal,
    }
    let mut franked_dividends: Vec<FrankedDividend> = Vec::new();
    let mut attached_credits_by_year: HashMap<i32, Decimal> = HashMap::new();

    for row in &income_rows {
        let date_paid: NaiveDate = row.try_get("date_paid")?;
        let tax_year = if date_paid.month() >= 7 {
            date_paid.year() + 1
        } else {
            date_paid.year()
        };

        // Amounts are denominated in the record's currency; convert to AUD via the
        // ATO rate for the month of date_paid before aggregating.
        let currency: String = row.try_get("currency")?;
        let franked = aud_field(pool, row, "franked_amount", &currency, date_paid).await?;
        let unfranked = aud_field(pool, row, "unfranked_amount", &currency, date_paid).await?;
        let foreign_income =
            aud_field(pool, row, "foreign_source_income", &currency, date_paid).await?;
        let foreign_tax = aud_field(pool, row, "foreign_tax_paid", &currency, date_paid).await?;
        let tfn_wht = aud_field(pool, row, "tfn_withholding_tax", &currency, date_paid).await?;
        let fc = aud_field(pool, row, "franking_credits", &currency, date_paid).await?;
        let lic =
            aud_field(pool, row, "lic_capital_gain_deduction", &currency, date_paid).await?;

        if fc > Decimal::ZERO {
            let ex_date: Option<NaiveDate> = row.try_get("ex_date")?;
            franked_dividends.push(FrankedDividend {
                tax_year,
                listing_id: row.try_get("listing_id")?,
                ex_date: ex_date.unwrap_or(date_paid),
                credits_aud: fc,
            });
            *attached_credits_by_year.entry(tax_year).or_default() += fc;
        }

        let s = map.entry(tax_year).or_insert_with(|| zero_summary(tax_year));
        s.dividends_assessable += franked + unfranked;
        s.foreign_source_income += foreign_income;
        s.lic_capital_gain_deduction += lic;
        s.franking_credits += fc;
        s.foreign_tax_offsets += foreign_tax;
        s.tfn_withholding_tax += tfn_wht;
    }

    for row in &amma_rows {
        let tax_year_end_date: NaiveDate = row.try_get("tax_year_end_date")?;
        let tax_year = tax_year_end_date.year();

        // Convert to AUD via the ATO rate for the month of tax_year_end_date (the
        // statement's only period anchor) before aggregating.
        let currency: String = row.try_get("currency")?;
        let d = tax_year_end_date;
        let interest = aud_field(pool, row, "australian_interest", &currency, d).await?;
        let div_unfranked =
            aud_field(pool, row, "australian_dividends_unfranked", &currency, d).await?;
        let franked_div = aud_field(pool, row, "franked_dividends", &currency, d).await?;
        let fc = aud_field(pool, row, "franking_credits", &currency, d).await?;
        let rent = aud_field(pool, row, "net_rent", &currency, d).await?;
        let foreign_inc = aud_field(pool, row, "foreign_income", &currency, d).await?;
        let foreign_tax = aud_field(pool, row, "foreign_tax_credits", &currency, d).await?;
        let other = aud_field(pool, row, "other_income", &currency, d).await?;
        let cgt_disc = aud_field(pool, row, "cgt_discount_gains", &currency, d).await?;
        let cgt_idx = aud_field(pool, row, "cgt_indexation_gains", &currency, d).await?;
        let cgt_other = aud_field(pool, row, "cgt_other_gains", &currency, d).await?;
        let cap_losses = aud_field(pool, row, "capital_losses_applied", &currency, d).await?;
        let tfn_wht = aud_field(pool, row, "tfn_withholding_tax", &currency, d).await?;

        let s = map.entry(tax_year).or_insert_with(|| zero_summary(tax_year));
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
        // AMMA credits count toward the small-shareholder threshold but are
        // never themselves denied: the holding-period rule needs a
        // per-distribution ex-date, which an annual AMMA statement doesn't
        // carry.
        *attached_credits_by_year.entry(tax_year).or_default() += fc;
    }

    // Franking-credit entitlement (docs/you-and-your-shares-dividends.md): in a
    // year with A$5,000 or more of attached credits the small-shareholder
    // exemption doesn't apply, so each dividend's shares must pass the at-risk
    // holding-period test; the credits on units that fail it are denied.
    for div in &franked_dividends {
        let attached = attached_credits_by_year[&div.tax_year];
        if attached < franking::small_shareholder_threshold_aud() {
            continue;
        }
        let test = franking::holding_period_test(pool, div.listing_id, div.ex_date).await?;
        let denied = test.denied(div.credits_aud);
        if denied > Decimal::ZERO {
            let s = map.get_mut(&div.tax_year).expect("year inserted with the income row");
            s.franking_credits -= denied;
            s.franking_credits_denied += denied;
        }
    }

    // FITO de-minimis (docs/fito-limit.md): a year's foreign tax offset over
    // A$1,000 needs the offset-limit calculation, which is outside this
    // system's data — cap the claimable offset and surface the excess.
    for s in map.values_mut() {
        let limit = fito_de_minimis_aud();
        if s.foreign_tax_offsets > limit {
            s.foreign_tax_offset_excess = s.foreign_tax_offsets - limit;
            s.foreign_tax_offsets = limit;
        }
    }

    let mut result: Vec<TaxYearSummary> = map.into_values().collect();
    result.sort_by_key(|s| s.tax_year);
    Ok(result)
}

async fn tax_summary_handler(
    State(pool): State<SqlitePool>,
) -> Result<Json<Vec<TaxYearSummary>>, StatusCode> {
    db_tax_summary(&pool)
        .await
        .map(Json)
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)
}

/// The same per-year rows as the JSON report, as a downloadable tax-return-ready CSV.
async fn tax_summary_export_handler(
    State(pool): State<SqlitePool>,
) -> Result<Response, StatusCode> {
    let rows = db_tax_summary(&pool)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    export::csv_response("tax-summary.csv", CSV_HEADER, &rows)
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{infra::db, entities::{amma, income, listing, rba_fx_rate, trade}};
    use axum::{body::Body, http::Request};
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    async fn test_pool() -> SqlitePool {
        db::init(":memory:").await.unwrap()
    }

    async fn insert_listing(pool: &SqlitePool, id: i64) {
        listing::db_upsert(
            pool,
            &listing::Listing {
                id,
                exchange_mic: "XASX".to_string(),
                ticker: format!("TST{id}"),
                name: format!("Test {id}"),
                isin: None,
                security_type: listing::SecurityType::ETF,
                currency: "AUD".to_string(),
                amit: false,
                preference: false,
            },
        )
        .await
        .unwrap();
    }

    async fn insert_trade(
        pool: &SqlitePool,
        id: i64,
        listing_id: i64,
        trade_type: trade::TradeType,
        date: NaiveDate,
        qty: i64,
    ) {
        trade::db_upsert(
            pool,
            &trade::Trade {
                holding_account_id: 1,
                transfer_id: None,
                id,
                trade_type,
                date,
                settlement_date: date + chrono::Duration::days(2),
                listing_id,
                average_price: Decimal::ONE,
                quantity: Decimal::from(qty),
                currency: "AUD".to_string(),
                brokerage: Decimal::ZERO,
                gst_on_brokerage: Decimal::ZERO,
                brokerage_currency: "AUD".to_string(),
                fx_rate: Decimal::ONE,
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

    fn make_income(id: i64, listing_id: i64, date: NaiveDate) -> income::Income {
        income::Income {
            holding_account_id: 1,
            id,
            listing_id,
            date_paid: date,
            ex_date: None,
            franked_amount: Decimal::ZERO,
            unfranked_amount: Decimal::ZERO,
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
        }
    }

    fn make_amma(id: i64, listing_id: i64, year_end: NaiveDate) -> amma::AmmaStatement {
        amma::AmmaStatement {
            holding_account_id: 1,
            id,
            listing_id,
            tax_year_end_date: year_end,
            units_held: Decimal::from(100),
            date_received: year_end + chrono::Duration::days(60),
            cost_base_adjustment: Decimal::ZERO,
            australian_interest: Decimal::ZERO,
            australian_dividends_unfranked: Decimal::ZERO,
            franked_dividends: Decimal::ZERO,
            franking_credits: Decimal::ZERO,
            net_rent: Decimal::ZERO,
            foreign_income: Decimal::ZERO,
            foreign_tax_credits: Decimal::ZERO,
            other_income: Decimal::ZERO,
            cgt_discount_gains: Decimal::ZERO,
            cgt_indexation_gains: Decimal::ZERO,
            cgt_other_gains: Decimal::ZERO,
            capital_losses_applied: Decimal::ZERO,
            tax_deferred_amount: Decimal::ZERO,
            tax_free_amount: Decimal::ZERO,
            tfn_withholding_tax: Decimal::ZERO,
            currency: "AUD".to_string(),
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

    #[tokio::test]
    async fn db_conduit_foreign_income_excluded_from_assessable() {
        let pool = test_pool().await;
        insert_listing(&pool, 1).await;
        let mut inc = make_income(1, 1, NaiveDate::from_ymd_opt(2024, 3, 15).unwrap());
        inc.foreign_source_income = Decimal::from(100);
        inc.conduit_foreign_income = Decimal::from(40); // must NOT appear in totals
        income::db_upsert(&pool, &inc).await.unwrap();

        let result = db_tax_summary(&pool).await.unwrap();
        assert_eq!(result.len(), 1);
        // Only foreign_source_income is included; conduit_foreign_income is excluded
        assert_eq!(result[0].foreign_source_income, Decimal::from(100));
        // dividends_assessable is zero (no franked/unfranked)
        assert_eq!(result[0].dividends_assessable, Decimal::ZERO);
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
    async fn db_lic_deduction_included() {
        let pool = test_pool().await;
        insert_listing(&pool, 1).await;
        let mut inc = make_income(1, 1, NaiveDate::from_ymd_opt(2024, 3, 15).unwrap());
        inc.lic_capital_gain_deduction = Decimal::from(15);
        income::db_upsert(&pool, &inc).await.unwrap();

        let result = db_tax_summary(&pool).await.unwrap();
        assert_eq!(result[0].lic_capital_gain_deduction, Decimal::from(15));
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

        // Trust distribution FY2024 with conduit foreign income
        let mut trust = make_income(2, 2, NaiveDate::from_ymd_opt(2024, 6, 30).unwrap());
        trust.foreign_source_income = Decimal::from(30);
        trust.foreign_tax_paid = Decimal::from(9);
        trust.conduit_foreign_income = Decimal::from(10); // excluded from assessable
        trust.lic_capital_gain_deduction = Decimal::from(5);
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
        assert_eq!(s.dividends_assessable, Decimal::from(200)); // 140 + 60
        assert_eq!(s.foreign_source_income, Decimal::from(30)); // conduit excluded
        assert_eq!(s.lic_capital_gain_deduction, Decimal::from(5));
        assert_eq!(s.franking_credits, Decimal::from(60)); // only from income (amma.franking_credits = 0)
        assert_eq!(s.foreign_tax_offsets, Decimal::from(12)); // 9 income + 3 amma
        assert_eq!(s.tfn_withholding_tax, Decimal::from(7)); // 5 income + 2 amma
        assert_eq!(s.amma_australian_interest, Decimal::from(8));
        assert_eq!(s.amma_cgt_discount_gains, Decimal::from(100));
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
    // exemption — docs/you-and-your-shares-dividends.md, reports::franking).

    /// Matthew-shaped facts: credits over $5,000 and the parcel held at risk
    /// under 45 days, so the credits are denied but the dividend stays assessable.
    #[tokio::test]
    async fn db_franking_credits_denied_when_held_under_45_days() {
        let pool = test_pool().await;
        insert_listing(&pool, 1).await;
        insert_trade(&pool, 1, 1, trade::TradeType::Buy, NaiveDate::from_ymd_opt(2025, 3, 1).unwrap(), 1000).await;
        insert_trade(&pool, 2, 1, trade::TradeType::Sell, NaiveDate::from_ymd_opt(2025, 4, 10).unwrap(), 1000).await;
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

    /// Same short holding, but total attached credits under $5,000: the
    /// small-shareholder exemption keeps them claimable.
    #[tokio::test]
    async fn db_small_shareholder_exemption_keeps_credits_below_5000() {
        let pool = test_pool().await;
        insert_listing(&pool, 1).await;
        insert_trade(&pool, 1, 1, trade::TradeType::Buy, NaiveDate::from_ymd_opt(2025, 3, 1).unwrap(), 1000).await;
        insert_trade(&pool, 2, 1, trade::TradeType::Sell, NaiveDate::from_ymd_opt(2025, 4, 10).unwrap(), 1000).await;
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
        insert_trade(&pool, 1, 1, trade::TradeType::Buy, NaiveDate::from_ymd_opt(2025, 3, 1).unwrap(), 1000).await;
        insert_trade(&pool, 2, 1, trade::TradeType::Sell, NaiveDate::from_ymd_opt(2025, 4, 10).unwrap(), 1000).await;
        let mut inc = make_income(1, 1, NaiveDate::from_ymd_opt(2025, 4, 8).unwrap());
        inc.ex_date = Some(NaiveDate::from_ymd_opt(2025, 3, 14).unwrap());
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
        insert_trade(&pool, 1, 1, trade::TradeType::Buy, NaiveDate::from_ymd_opt(2025, 3, 1).unwrap(), 1000).await;
        insert_trade(&pool, 2, 1, trade::TradeType::Sell, NaiveDate::from_ymd_opt(2025, 4, 10).unwrap(), 1000).await;
        // $3,000 income credits alone would be exempt…
        let mut inc = make_income(1, 1, NaiveDate::from_ymd_opt(2025, 4, 8).unwrap());
        inc.ex_date = Some(NaiveDate::from_ymd_opt(2025, 3, 14).unwrap());
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
        insert_trade(&pool, 1, 1, trade::TradeType::Buy, NaiveDate::from_ymd_opt(2025, 4, 1).unwrap(), 1000).await;
        insert_trade(&pool, 2, 1, trade::TradeType::Sell, NaiveDate::from_ymd_opt(2025, 4, 20).unwrap(), 1000).await;
        let mut inc = make_income(1, 1, NaiveDate::from_ymd_opt(2025, 4, 8).unwrap());
        inc.ex_date = None;
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
        insert_trade(&pool, 1, 1, trade::TradeType::Buy, NaiveDate::from_ymd_opt(2023, 1, 10).unwrap(), 1000).await;
        let mut inc = make_income(1, 1, NaiveDate::from_ymd_opt(2025, 4, 8).unwrap());
        inc.ex_date = Some(NaiveDate::from_ymd_opt(2025, 3, 14).unwrap());
        inc.franking_credits = Decimal::from(6000);
        income::db_upsert(&pool, &inc).await.unwrap();

        let result = db_tax_summary(&pool).await.unwrap();
        assert_eq!(result[0].franking_credits, Decimal::from(6000));
        assert_eq!(result[0].franking_credits_denied, Decimal::ZERO);
    }

    // FITO de-minimis cap (docs/fito-limit.md): up to A$1,000 of foreign tax
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
        // Anna-shaped total (docs/fito-limit.md Example 16 pays A$3,400 foreign
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

        let resp = router()
            .with_state(pool)
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/portfolio/tax-summary")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let result: Vec<TaxYearSummary> = serde_json::from_slice(&bytes).unwrap();
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
        inc.franked_amount = Decimal::from(70);
        inc.unfranked_amount = Decimal::from(30);
        inc.franking_credits = "30.50".parse().unwrap();
        income::db_upsert(&pool, &inc).await.unwrap();

        let resp = router()
            .with_state(pool)
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/portfolio/tax-summary/export")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        assert_eq!(
            resp.headers().get(axum::http::header::CONTENT_TYPE).unwrap(),
            "text/csv; charset=utf-8"
        );
        assert_eq!(
            resp.headers().get(axum::http::header::CONTENT_DISPOSITION).unwrap(),
            "attachment; filename=\"tax-summary.csv\""
        );
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let csv = String::from_utf8(bytes.to_vec()).unwrap();
        let mut lines = csv.lines();
        // Header names every TaxYearSummary field, in declaration order.
        assert_eq!(lines.next().unwrap(), CSV_HEADER.join(","));
        // One record per tax year, decimal figures rendered exactly.
        let row = lines.next().unwrap();
        assert!(row.starts_with("2024,100,"));
        assert!(row.contains(",30.50,")); // franking_credits keeps its precision
        assert_eq!(lines.next(), None);
    }

    #[tokio::test]
    async fn api_export_of_empty_report_still_returns_header() {
        let pool = test_pool().await;
        let resp = router()
            .with_state(pool)
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/portfolio/tax-summary/export")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let csv = String::from_utf8(bytes.to_vec()).unwrap();
        assert_eq!(csv, CSV_HEADER.join(",") + "\n");
    }
}
