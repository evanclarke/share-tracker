use crate::domain::tax_year::tax_year_for;
use crate::infra::decimal::parse_dec;
use crate::infra::fx::{FxOverride, FxRates};
use crate::infra::http::ApiError;
use axum::{Json, Router, extract::State, routing::get};
use chrono::{Datelike, NaiveDate};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use sqlx::{Row, SqlitePool};
use std::collections::{BTreeMap, HashSet};

/// A financial year in which an AMIT listing received cash distributions but
/// has no AMMA statement covering that year. AMIT cash rows are excluded from
/// the [tax summary](crate::reports::tax_summary) — the AMMA attribution is
/// the assessable record — so a missing AMMA would silently drop the year's
/// income from the return. Non-blocking: income writes are never rejected —
/// entering the fund's AMMA statement clears the flag. The converse (an AMMA
/// year with no cash rows) is not flagged: a fund can be held without
/// receiving or recording cash that year.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AmitCashAlert {
    pub listing_id: i64,
    pub ticker: String,
    /// The financial year the AMMA statement is expected for, identified by
    /// the calendar year of its 30 June end — the cash rows' assessment year
    /// (the governing `entitlement_date` when set, else `date_paid`, matching
    /// the tax summary's attribution).
    pub tax_year: i32,
    /// How many cash income rows the year has on this listing.
    pub cash_rows: i64,
    /// The year's gross cash (franked + unfranked + foreign components) in
    /// AUD — the income that would go unreported while the AMMA is missing.
    pub cash_total_aud: Decimal,
}

pub fn router() -> Router<SqlitePool> {
    Router::new().route("/reports/amit_cash_cross_check", get(report))
}

/// Flag every (AMIT listing, financial year) with cash income rows but no
/// AMMA statement whose `tax_year_end_date` falls in that year. An empty
/// report means every AMIT cash year has its attribution entered.
pub async fn db_amit_cash_alerts(pool: &SqlitePool) -> Result<Vec<AmitCashAlert>, sqlx::Error> {
    // Both inputs (and the FX rates converting the cash) on one read
    // transaction: a single consistent snapshot, so an interleaved write
    // can't pair a cash row with a half-entered statement set.
    let mut tx = pool.begin().await?;
    let cash_rows = sqlx::query(
        "SELECT i.listing_id, l.ticker, i.date_paid, i.entitlement_date, i.trust_income, \
                i.franked_amount, i.unfranked_amount, i.foreign_source_income, i.currency \
         FROM income i \
         JOIN listings l ON l.id = i.listing_id \
         WHERE l.amit",
    )
    .fetch_all(&mut *tx)
    .await?;
    let amma_rows = sqlx::query("SELECT listing_id, tax_year_end_date FROM amma_statements")
        .fetch_all(&mut *tx)
        .await?;
    let fx = FxRates::load(&mut *tx).await?;
    tx.commit().await?;

    // (listing, FY) pairs covered by an AMMA statement — the statement's year
    // is the calendar year of its tax_year_end_date, as the tax summary reads it.
    let mut covered = HashSet::new();
    for row in &amma_rows {
        let listing_id: i64 = row.try_get("listing_id")?;
        let year_end: NaiveDate = row.try_get("tax_year_end_date")?;
        covered.insert((listing_id, year_end.year()));
    }

    // Aggregate the cash rows per (ticker, listing, FY); BTreeMap keeps the
    // report ordered by ticker then year.
    let mut by_year: BTreeMap<(String, i64, i32), (i64, Decimal)> = BTreeMap::new();
    for row in &cash_rows {
        let listing_id: i64 = row.try_get("listing_id")?;
        let date_paid: NaiveDate = row.try_get("date_paid")?;
        // Trust rows are assessed by present entitlement when recorded —
        // the same attribution the tax summary applies.
        let trust_income: bool = row.try_get("trust_income")?;
        let entitlement_date: Option<NaiveDate> = row.try_get("entitlement_date")?;
        let assessed = match entitlement_date {
            Some(d) if trust_income => d,
            _ => date_paid,
        };
        let tax_year = tax_year_for(assessed);
        if covered.contains(&(listing_id, tax_year)) {
            continue;
        }
        let currency: String = row.try_get("currency")?;
        let dec = |col: &str| -> Result<Decimal, sqlx::Error> { parse_dec(col, row.try_get(col)?) };
        let gross =
            dec("franked_amount")? + dec("unfranked_amount")? + dec("foreign_source_income")?;
        let gross_aud = fx.to_aud(gross, &currency, assessed, FxOverride::None)?;
        let ticker: String = row.try_get("ticker")?;
        let entry = by_year
            .entry((ticker, listing_id, tax_year))
            .or_insert((0, Decimal::ZERO));
        entry.0 += 1;
        entry.1 += gross_aud;
    }

    Ok(by_year
        .into_iter()
        .map(
            |((ticker, listing_id, tax_year), (cash_rows, cash_total_aud))| AmitCashAlert {
                listing_id,
                ticker,
                tax_year,
                cash_rows,
                cash_total_aud,
            },
        )
        .collect())
}

async fn report(State(pool): State<SqlitePool>) -> Result<Json<Vec<AmitCashAlert>>, ApiError> {
    db_amit_cash_alerts(&pool)
        .await
        .map(Json)
        .map_err(ApiError::from)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{self, test_pool, ymd};
    use axum::http::StatusCode;
    use axum::{body::Body, http::Request};
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    async fn insert_amit_listing(pool: &SqlitePool, id: i64, ticker: &str) {
        test_support::listing(id)
            .ticker(ticker)
            .amit(true)
            .insert(pool)
            .await;
    }

    async fn insert_cash(pool: &SqlitePool, id: i64, listing_id: i64, date_paid: NaiveDate) {
        test_support::income(id, listing_id, date_paid)
            .with(|i| {
                i.trust_income = true;
                i.unfranked_amount = Decimal::from(100);
            })
            .insert(pool)
            .await;
    }

    async fn insert_amma(pool: &SqlitePool, id: i64, listing_id: i64, year_end: NaiveDate) {
        test_support::amma(id, listing_id)
            .with(|a| {
                a.tax_year_end_date = year_end;
                a.date_received = year_end + chrono::Duration::days(60);
            })
            .insert(pool)
            .await;
    }

    /// AMIT cash with no AMMA for the year is flagged, aggregated per
    /// (listing, FY) with the year's gross cash.
    #[tokio::test]
    async fn db_cash_year_without_amma_is_flagged() {
        let pool = test_pool().await;
        insert_amit_listing(&pool, 1, "VDHG").await;
        insert_cash(&pool, 1, 1, ymd(2024, 10, 16)).await;
        insert_cash(&pool, 2, 1, ymd(2025, 1, 16)).await;
        let alerts = db_amit_cash_alerts(&pool).await.unwrap();
        assert_eq!(alerts.len(), 1);
        let a = &alerts[0];
        assert_eq!(a.listing_id, 1);
        assert_eq!(a.ticker, "VDHG");
        // Oct 2024 and Jan 2025 → FY ending 30 June 2025.
        assert_eq!(a.tax_year, 2025);
        assert_eq!(a.cash_rows, 2);
        assert_eq!(a.cash_total_aud, Decimal::from(200));
    }

    /// Entering the fund's AMMA statement for that year clears the flag.
    #[tokio::test]
    async fn db_amma_covering_the_year_clears_the_flag() {
        let pool = test_pool().await;
        insert_amit_listing(&pool, 1, "VDHG").await;
        insert_cash(&pool, 1, 1, ymd(2024, 10, 16)).await;
        insert_amma(&pool, 1, 1, ymd(2025, 6, 30)).await;
        assert!(db_amit_cash_alerts(&pool).await.unwrap().is_empty());
    }

    /// An AMMA for a *different* year doesn't cover the cash year.
    #[tokio::test]
    async fn db_amma_for_another_year_does_not_clear_the_flag() {
        let pool = test_pool().await;
        insert_amit_listing(&pool, 1, "VDHG").await;
        insert_cash(&pool, 1, 1, ymd(2024, 10, 16)).await;
        insert_amma(&pool, 1, 1, ymd(2024, 6, 30)).await;
        let alerts = db_amit_cash_alerts(&pool).await.unwrap();
        assert_eq!(alerts.len(), 1);
        assert_eq!(alerts[0].tax_year, 2025);
    }

    /// The AMMA must be on the same listing — another fund's statement
    /// clears nothing.
    #[tokio::test]
    async fn db_amma_on_another_listing_does_not_clear_the_flag() {
        let pool = test_pool().await;
        insert_amit_listing(&pool, 1, "VDHG").await;
        insert_amit_listing(&pool, 2, "HNDQ").await;
        insert_cash(&pool, 1, 1, ymd(2024, 10, 16)).await;
        insert_amma(&pool, 1, 2, ymd(2025, 6, 30)).await;
        assert_eq!(db_amit_cash_alerts(&pool).await.unwrap().len(), 1);
    }

    /// An AMMA year with no cash rows is fine — the fund can be held without
    /// recording cash that year — and non-AMIT listings are never flagged.
    #[tokio::test]
    async fn db_amma_without_cash_and_non_amit_listings_not_flagged() {
        let pool = test_pool().await;
        insert_amit_listing(&pool, 1, "VDHG").await;
        insert_amma(&pool, 1, 1, ymd(2025, 6, 30)).await;
        // Non-AMIT trust with cash and no AMMA: not this report's business.
        test_support::listing(2).ticker("TRST").insert(&pool).await;
        insert_cash(&pool, 1, 2, ymd(2024, 10, 16)).await;
        assert!(db_amit_cash_alerts(&pool).await.unwrap().is_empty());
    }

    /// A July-paid June distribution carrying its entitlement date is
    /// attributed to the FY just ended — the AMMA expected is that year's.
    #[tokio::test]
    async fn db_entitlement_date_governs_the_expected_fy() {
        let pool = test_pool().await;
        insert_amit_listing(&pool, 1, "VDHG").await;
        test_support::income(1, 1, ymd(2025, 7, 16))
            .with(|i| {
                i.trust_income = true;
                i.entitlement_date = Some(ymd(2025, 6, 30));
                i.unfranked_amount = Decimal::from(100);
            })
            .insert(&pool)
            .await;
        // FY2025 AMMA covers it; without the entitlement date the row would
        // look like FY2026 and stay flagged.
        insert_amma(&pool, 1, 1, ymd(2025, 6, 30)).await;
        assert!(db_amit_cash_alerts(&pool).await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn api_get_amit_cash_cross_check() {
        let pool = test_pool().await;
        insert_amit_listing(&pool, 1, "VDHG").await;
        insert_cash(&pool, 1, 1, ymd(2024, 10, 16)).await;
        let resp = router()
            .with_state(pool)
            .oneshot(
                Request::builder()
                    .uri("/reports/amit_cash_cross_check")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let alerts: Vec<AmitCashAlert> = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(alerts.len(), 1);
        assert_eq!(alerts[0].ticker, "VDHG");
        assert_eq!(alerts[0].cash_total_aud, Decimal::from(100));
    }
}
