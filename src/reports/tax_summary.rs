use crate::infra::decimal::parse_dec;
use axum::{Json, Router, extract::State, http::StatusCode, routing::get};
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
    /// Total franking credits (income + AMMA).
    pub franking_credits: Decimal,
    /// Total foreign tax offsets (income foreign_tax_paid + AMMA foreign_tax_credits).
    pub foreign_tax_offsets: Decimal,
    /// Total TFN withholding tax (income + AMMA).
    pub tfn_withholding_tax: Decimal,
}

pub fn router() -> Router<SqlitePool> {
    Router::new().route("/portfolio/tax-summary", get(tax_summary_handler))
}

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
        foreign_tax_offsets: Decimal::ZERO,
        tfn_withholding_tax: Decimal::ZERO,
    }
}

pub async fn db_tax_summary(pool: &SqlitePool) -> Result<Vec<TaxYearSummary>, sqlx::Error> {
    let income_rows = sqlx::query(
        "SELECT date_paid, franked_amount, unfranked_amount, foreign_source_income, \
         foreign_tax_paid, tfn_withholding_tax, franking_credits, lic_capital_gain_deduction \
         FROM income",
    )
    .fetch_all(pool)
    .await?;

    let amma_rows = sqlx::query(
        "SELECT tax_year_end_date, australian_interest, australian_dividends_unfranked, \
         franked_dividends, franking_credits, net_rent, foreign_income, foreign_tax_credits, \
         other_income, cgt_discount_gains, cgt_indexation_gains, cgt_other_gains, \
         capital_losses_applied, tfn_withholding_tax \
         FROM amma_statements",
    )
    .fetch_all(pool)
    .await?;

    let mut map: HashMap<i32, TaxYearSummary> = HashMap::new();

    for row in &income_rows {
        let date_paid: NaiveDate = row.try_get("date_paid")?;
        let tax_year = if date_paid.month() >= 7 {
            date_paid.year() + 1
        } else {
            date_paid.year()
        };

        let franked = parse_dec("franked_amount", row.try_get("franked_amount")?)?;
        let unfranked = parse_dec("unfranked_amount", row.try_get("unfranked_amount")?)?;
        let foreign_income = parse_dec("foreign_source_income", row.try_get("foreign_source_income")?)?;
        let foreign_tax = parse_dec("foreign_tax_paid", row.try_get("foreign_tax_paid")?)?;
        let tfn_wht = parse_dec("tfn_withholding_tax", row.try_get("tfn_withholding_tax")?)?;
        let fc = parse_dec("franking_credits", row.try_get("franking_credits")?)?;
        let lic = parse_dec("lic_capital_gain_deduction", row.try_get("lic_capital_gain_deduction")?)?;

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

        let interest = parse_dec("australian_interest", row.try_get("australian_interest")?)?;
        let div_unfranked =
            parse_dec("australian_dividends_unfranked", row.try_get("australian_dividends_unfranked")?)?;
        let franked_div = parse_dec("franked_dividends", row.try_get("franked_dividends")?)?;
        let fc = parse_dec("franking_credits", row.try_get("franking_credits")?)?;
        let rent = parse_dec("net_rent", row.try_get("net_rent")?)?;
        let foreign_inc = parse_dec("foreign_income", row.try_get("foreign_income")?)?;
        let foreign_tax = parse_dec("foreign_tax_credits", row.try_get("foreign_tax_credits")?)?;
        let other = parse_dec("other_income", row.try_get("other_income")?)?;
        let cgt_disc = parse_dec("cgt_discount_gains", row.try_get("cgt_discount_gains")?)?;
        let cgt_idx = parse_dec("cgt_indexation_gains", row.try_get("cgt_indexation_gains")?)?;
        let cgt_other = parse_dec("cgt_other_gains", row.try_get("cgt_other_gains")?)?;
        let cap_losses = parse_dec("capital_losses_applied", row.try_get("capital_losses_applied")?)?;
        let tfn_wht = parse_dec("tfn_withholding_tax", row.try_get("tfn_withholding_tax")?)?;

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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{infra::db, entities::{amma, income, listing}};
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
            },
        )
        .await
        .unwrap();
    }

    fn make_income(id: i64, listing_id: i64, date: NaiveDate) -> income::Income {
        income::Income {
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
        }
    }

    fn make_amma(id: i64, listing_id: i64, year_end: NaiveDate) -> amma::AmmaStatement {
        amma::AmmaStatement {
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
}
