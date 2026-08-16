use crate::domain::tax_year::tax_year_for;
use crate::entities::income::Income;
use crate::infra::fx::{FxOverride, FxRates};
use crate::infra::http::ApiError;
use axum::{Json, Router, extract::State, routing::get};
use chrono::{Datelike, NaiveDate};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use sqlx::{FromRow, Row, SqlitePool};
use std::collections::{BTreeMap, HashSet};

/// A financial year in which an AMIT listing received cash distributions into
/// one holding account but has no AMMA statement covering that account and
/// year. AMIT cash rows are excluded from the [tax
/// summary](crate::reports::tax_summary) — the AMMA attribution is the
/// assessable record — so a missing AMMA would silently drop the year's
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
    /// The holding account the cash was paid into, and so the account whose
    /// AMMA statement is missing. A registry issues **one statement per
    /// holder account** — which is why an AMIT adjustment may only touch its
    /// statement's own account
    /// (`entities::amit_adjustment::UpsertError::HoldingAccountMismatch`) and
    /// why generation narrows to it — so coverage is asked per account too: a
    /// fund held in two accounts needs two statements, and one of them
    /// leaves the other account's income unattributed (SCENARIOS F-03, F-08).
    pub holding_account_id: i64,
    /// How many cash income rows the year has on this listing in this account.
    pub cash_rows: i64,
    /// The year's gross cash (franked + unfranked + foreign components) in
    /// AUD — the income that would go unreported while the AMMA is missing.
    pub cash_total_aud: Decimal,
}

pub fn router() -> Router<SqlitePool> {
    Router::new().route("/reports/amit_cash_cross_check", get(report))
}

/// Flag every (AMIT listing, holding account, financial year) with cash
/// income rows but no AMMA statement of that account whose `tax_year_end_date`
/// falls in that year. An empty report means every AMIT cash year has its
/// attribution entered, in the account the cash was paid into.
pub async fn db_amit_cash_alerts(pool: &SqlitePool) -> Result<Vec<AmitCashAlert>, sqlx::Error> {
    // Both inputs (and the FX rates converting the cash) on one read
    // transaction: a single consistent snapshot, so an interleaved write
    // can't pair a cash row with a half-entered statement set.
    let mut tx = pool.begin().await?;
    let cash_rows = sqlx::query(
        "SELECT i.*, l.ticker \
         FROM income i \
         JOIN listings l ON l.id = i.listing_id \
         WHERE l.amit",
    )
    .fetch_all(&mut *tx)
    .await?;
    let amma_rows = sqlx::query(
        "SELECT listing_id, holding_account_id, tax_year_end_date FROM amma_statements",
    )
    .fetch_all(&mut *tx)
    .await?;
    let fx = FxRates::load(&mut *tx).await?;
    tx.commit().await?;

    // (listing, account, FY) triples covered by an AMMA statement — the
    // statement's year is the calendar year of its tax_year_end_date, as the
    // tax summary reads it, and its account is the holder account the
    // registry issued it for. A statement in another account attributes that
    // account's units, not these, so it covers nothing here.
    let mut covered = HashSet::new();
    for row in &amma_rows {
        let listing_id: i64 = row.try_get("listing_id")?;
        let holding_account_id: i64 = row.try_get("holding_account_id")?;
        let year_end: NaiveDate = row.try_get("tax_year_end_date")?;
        covered.insert((listing_id, holding_account_id, year_end.year()));
    }

    // Aggregate the cash rows per (ticker, listing, FY, account); BTreeMap
    // keeps the report ordered by ticker, then year, then account.
    let mut by_year: BTreeMap<(String, i64, i32, i64), (i64, Decimal)> = BTreeMap::new();
    for row in &cash_rows {
        // The joined ticker aside, every field comes off the income model, so
        // the assessment-date rule and the gross-cash definition are the
        // entity's own (`Income::assessment_date` — present entitlement for a
        // trust row, payment otherwise — and `Income::gross_cash_income`),
        // shared with the tax summary rather than restated here.
        let income = Income::from_row(row)?;
        let assessed = income.assessment_date();
        let tax_year = tax_year_for(assessed);
        if covered.contains(&(income.listing_id, income.holding_account_id, tax_year)) {
            continue;
        }
        let gross_aud = fx.to_aud(
            income.gross_cash_income(),
            &income.currency,
            assessed,
            FxOverride::None,
        )?;
        let ticker: String = row.try_get("ticker")?;
        let entry = by_year
            .entry((
                ticker,
                income.listing_id,
                tax_year,
                income.holding_account_id,
            ))
            .or_insert((0, Decimal::ZERO));
        entry.0 += 1;
        entry.1 += gross_aud;
    }

    Ok(by_year
        .into_iter()
        .map(
            |((ticker, listing_id, tax_year, holding_account_id), (cash_rows, cash_total_aud))| {
                AmitCashAlert {
                    listing_id,
                    ticker,
                    tax_year,
                    holding_account_id,
                    cash_rows,
                    cash_total_aud,
                }
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
    use crate::test_support::{self, ApiClient, test_pool, ymd};
    use axum::http::StatusCode;

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
        insert_amma_in(pool, id, listing_id, year_end, 1).await;
    }

    async fn insert_amma_in(
        pool: &SqlitePool,
        id: i64,
        listing_id: i64,
        year_end: NaiveDate,
        holding_account_id: i64,
    ) {
        test_support::amma(id, listing_id)
            .with(|a| {
                a.tax_year_end_date = year_end;
                a.date_received = year_end + chrono::Duration::days(60);
                a.holding_account_id = holding_account_id;
            })
            .insert(pool)
            .await;
    }

    /// A second holding account to pay a distribution into.
    async fn second_account(pool: &SqlitePool) {
        crate::entities::holding_account::db_upsert(
            pool,
            &crate::entities::holding_account::HoldingAccount {
                id: 2,
                name: "Second".to_string(),
            },
        )
        .await
        .unwrap();
    }

    async fn insert_cash_in(
        pool: &SqlitePool,
        id: i64,
        listing_id: i64,
        date_paid: NaiveDate,
        holding_account_id: i64,
    ) {
        test_support::income(id, listing_id, date_paid)
            .with(|i| {
                i.trust_income = true;
                i.unfranked_amount = Decimal::from(100);
                i.holding_account_id = holding_account_id;
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
        assert_eq!(a.holding_account_id, 1);
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

    /// SCENARIOS F-08: the AMMA must also be for the account the cash was
    /// paid into. A registry issues one statement per holder account, so a
    /// statement covering the *other* account attributes that account's
    /// units and leaves this one's income unattributed.
    #[tokio::test]
    async fn db_amma_in_another_holding_account_does_not_clear_the_flag() {
        let pool = test_pool().await;
        insert_amit_listing(&pool, 1, "VDHG").await;
        second_account(&pool).await;
        insert_cash_in(&pool, 1, 1, ymd(2024, 10, 16), 1).await;
        insert_amma_in(&pool, 1, 1, ymd(2025, 6, 30), 2).await;

        let alerts = db_amit_cash_alerts(&pool).await.unwrap();
        assert_eq!(alerts.len(), 1);
        assert_eq!(alerts[0].holding_account_id, 1);
        assert_eq!(alerts[0].tax_year, 2025);
    }

    /// The fund held in two accounts: each account's cash is its own alert,
    /// and entering one account's statement clears only that one.
    #[tokio::test]
    async fn db_each_holding_account_is_flagged_and_cleared_on_its_own() {
        let pool = test_pool().await;
        insert_amit_listing(&pool, 1, "VDHG").await;
        second_account(&pool).await;
        insert_cash_in(&pool, 1, 1, ymd(2024, 10, 16), 1).await;
        insert_cash_in(&pool, 2, 1, ymd(2024, 10, 16), 2).await;

        let alerts = db_amit_cash_alerts(&pool).await.unwrap();
        assert_eq!(
            alerts
                .iter()
                .map(|a| (a.holding_account_id, a.cash_rows, a.cash_total_aud))
                .collect::<Vec<_>>(),
            vec![(1, 1, Decimal::from(100)), (2, 1, Decimal::from(100))]
        );

        insert_amma_in(&pool, 1, 1, ymd(2025, 6, 30), 1).await;
        let remaining = db_amit_cash_alerts(&pool).await.unwrap();
        assert_eq!(remaining.len(), 1);
        assert_eq!(remaining[0].holding_account_id, 2);

        insert_amma_in(&pool, 2, 1, ymd(2025, 6, 30), 2).await;
        assert!(db_amit_cash_alerts(&pool).await.unwrap().is_empty());
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
        let resp = ApiClient::over(router().with_state(pool))
            .get("/reports/amit_cash_cross_check")
            .await;
        assert_eq!(resp.status, StatusCode::OK);
        let alerts: Vec<AmitCashAlert> = resp.json();
        assert_eq!(alerts.len(), 1);
        assert_eq!(alerts[0].ticker, "VDHG");
        assert_eq!(alerts[0].cash_total_aud, Decimal::from(100));
    }
}
