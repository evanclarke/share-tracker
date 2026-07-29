use crate::domain::tax_year::tax_year_for;
use crate::entities::income::Income;
use crate::infra::http::ApiError;
use axum::{Json, Router, extract::State, routing::get};
use chrono::NaiveDate;
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use sqlx::{FromRow, Row, SqlitePool};

/// A trust income row whose statement reported a tax-deferred amount with no
/// `ReturnOfCapital` corporate action on the listing in the same financial
/// year. For a non-AMIT unit trust, a tax-deferred amount is a CGT event E4
/// cost-base reduction (`docs/ato/cgt-non-assessable-payments.md`) that this
/// system models as a `ReturnOfCapital` action — a recorded amount with no
/// matching action means the cost base is silently overstated. Non-blocking:
/// income writes are never rejected — this report only surfaces rows whose
/// reduction still needs to be entered; entering the action clears the flag.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct E4CrossCheckAlert {
    pub income_id: i64,
    pub listing_id: i64,
    pub ticker: String,
    pub date_paid: NaiveDate,
    /// The statement's tax-deferred amount, verbatim (record currency).
    pub tax_deferred_amount: Decimal,
    /// The financial year the matching `ReturnOfCapital` action is expected
    /// in — the row's assessment year (calendar year of its 30 June end),
    /// per the governing `entitlement_date` when set, else `date_paid`.
    pub tax_year: i32,
}

pub fn router() -> Router<SqlitePool> {
    Router::new().route("/reports/e4_cross_check", get(report))
}

/// Flag every trust income row carrying a non-zero `tax_deferred_amount`
/// whose listing has no `ReturnOfCapital` corporate action dated in the
/// row's financial year (the assessment year — `entitlement_date` when set,
/// else `date_paid`, matching the tax summary's attribution). Rows whose
/// listing has a same-FY action are omitted — an empty report means every
/// recorded tax-deferred amount has its E4 cost-base reduction entered.
pub async fn db_e4_alerts(pool: &SqlitePool) -> Result<Vec<E4CrossCheckAlert>, sqlx::Error> {
    // Both inputs on one read transaction: a single consistent snapshot, so
    // an interleaved write can't pair an income row with a half-entered
    // action set.
    let mut tx = pool.begin().await?;
    let income_rows = sqlx::query(
        "SELECT i.*, l.ticker \
         FROM income i \
         JOIN listings l ON l.id = i.listing_id \
         WHERE i.trust_income = 1 AND i.tax_deferred_amount IS NOT NULL \
         ORDER BY l.ticker, i.date_paid, i.id",
    )
    .fetch_all(&mut *tx)
    .await?;
    let roc_rows = sqlx::query(
        "SELECT listing_id, date FROM corporate_actions \
         WHERE action_type = 'ReturnOfCapital'",
    )
    .fetch_all(&mut *tx)
    .await?;
    tx.commit().await?;

    // (listing, FY) pairs that have a return-of-capital action.
    let mut covered = std::collections::HashSet::new();
    for row in &roc_rows {
        let listing_id: i64 = row.try_get("listing_id")?;
        let date: NaiveDate = row.try_get("date")?;
        covered.insert((listing_id, tax_year_for(date)));
    }

    let mut alerts = Vec::new();
    for row in &income_rows {
        // The joined ticker aside, every field comes off the income model, so
        // the assessment-date rule is the entity's own
        // (`Income::assessment_date` — present entitlement for a trust row,
        // payment otherwise), shared with the tax summary rather than
        // restated here.
        let income = Income::from_row(row)?;
        // The query filters to non-NULL; parse failures still propagate.
        let Some(tax_deferred_amount) = income.tax_deferred_amount else {
            continue;
        };
        if tax_deferred_amount.is_zero() {
            continue;
        }
        let tax_year = tax_year_for(income.assessment_date());
        if covered.contains(&(income.listing_id, tax_year)) {
            continue;
        }
        alerts.push(E4CrossCheckAlert {
            income_id: income.id,
            listing_id: income.listing_id,
            ticker: row.try_get("ticker")?,
            date_paid: income.date_paid,
            tax_deferred_amount,
            tax_year,
        });
    }
    Ok(alerts)
}

async fn report(State(pool): State<SqlitePool>) -> Result<Json<Vec<E4CrossCheckAlert>>, ApiError> {
    db_e4_alerts(&pool).await.map(Json).map_err(ApiError::from)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::entities::corporate_action;
    use crate::test_support::{self, ApiClient, test_pool, ymd};
    use axum::http::StatusCode;

    async fn insert_trust_income(
        pool: &SqlitePool,
        id: i64,
        date_paid: NaiveDate,
        tax_deferred: Option<&str>,
    ) {
        test_support::income(id, 1, date_paid)
            .with(|i| {
                i.unfranked_amount = Decimal::from(100);
                i.trust_income = true;
                i.tax_deferred_amount = tax_deferred.map(|d| d.parse().unwrap());
            })
            .insert(pool)
            .await;
    }

    async fn insert_roc(pool: &SqlitePool, id: i64, date: NaiveDate) {
        corporate_action::db_upsert(
            pool,
            &corporate_action::CorporateAction {
                id,
                listing_id: 1,
                date,
                kind: corporate_action::ActionKind::ReturnOfCapital {
                    amount_per_unit: "0.10".parse().unwrap(),
                    currency: "AUD".to_string(),
                },
            },
        )
        .await
        .unwrap();
    }

    /// A trust row with a tax-deferred amount and no return-of-capital action
    /// is flagged: the statement says the cost base must come down, but no
    /// reduction has been entered.
    #[tokio::test]
    async fn db_tax_deferred_without_action_is_flagged() {
        let pool = test_pool().await;
        test_support::listing(1).insert(&pool).await;
        insert_trust_income(&pool, 1, ymd(2025, 3, 15), Some("120.50")).await;
        let alerts = db_e4_alerts(&pool).await.unwrap();
        assert_eq!(alerts.len(), 1);
        let a = &alerts[0];
        assert_eq!(a.income_id, 1);
        assert_eq!(a.listing_id, 1);
        assert_eq!(a.tax_deferred_amount, "120.50".parse().unwrap());
        // Paid March 2025 → FY ending 30 June 2025.
        assert_eq!(a.tax_year, 2025);
    }

    /// Entering the matching same-FY ReturnOfCapital action clears the flag.
    #[tokio::test]
    async fn db_same_fy_return_of_capital_clears_the_flag() {
        let pool = test_pool().await;
        test_support::listing(1).insert(&pool).await;
        insert_trust_income(&pool, 1, ymd(2025, 3, 15), Some("120.50")).await;
        insert_roc(&pool, 1, ymd(2025, 5, 1)).await;
        assert!(db_e4_alerts(&pool).await.unwrap().is_empty());
    }

    /// An action in a *different* FY does not clear the flag — the E4
    /// adjustment belongs to the income year the statement reported.
    #[tokio::test]
    async fn db_action_in_another_fy_does_not_clear_the_flag() {
        let pool = test_pool().await;
        test_support::listing(1).insert(&pool).await;
        insert_trust_income(&pool, 1, ymd(2025, 3, 15), Some("120.50")).await;
        // July 2025 is the *next* FY (2026), so the March-2025 row stays flagged.
        insert_roc(&pool, 1, ymd(2025, 7, 1)).await;
        let alerts = db_e4_alerts(&pool).await.unwrap();
        assert_eq!(alerts.len(), 1);
    }

    /// Rows with no tax-deferred amount (NULL) or an explicit zero are
    /// omitted — there is nothing the statement says to cross-check.
    #[tokio::test]
    async fn db_null_or_zero_amounts_are_omitted() {
        let pool = test_pool().await;
        test_support::listing(1).insert(&pool).await;
        insert_trust_income(&pool, 1, ymd(2025, 3, 15), None).await;
        insert_trust_income(&pool, 2, ymd(2025, 3, 16), Some("0")).await;
        assert!(db_e4_alerts(&pool).await.unwrap().is_empty());
    }

    /// A trust row carrying an entitlement date is attributed by it (the tax
    /// summary's rule): a July-paid June distribution expects the action in
    /// the FY just ended, and an action dated in that FY clears it.
    #[tokio::test]
    async fn db_entitlement_date_governs_the_expected_fy() {
        let pool = test_pool().await;
        test_support::listing(1).insert(&pool).await;
        test_support::income(1, 1, ymd(2025, 7, 15))
            .with(|i| {
                i.unfranked_amount = Decimal::from(100);
                i.trust_income = true;
                i.entitlement_date = Some(ymd(2025, 6, 30));
                i.tax_deferred_amount = Some("40".parse().unwrap());
            })
            .insert(&pool)
            .await;
        // Action in FY2025 (the entitlement year) clears the July-paid row.
        insert_roc(&pool, 1, ymd(2025, 6, 30)).await;
        assert!(db_e4_alerts(&pool).await.unwrap().is_empty());

        // Re-dated into FY2026 (the payment year) it no longer matches.
        insert_roc(&pool, 1, ymd(2025, 8, 1)).await;
        let alerts = db_e4_alerts(&pool).await.unwrap();
        assert_eq!(alerts.len(), 1);
        assert_eq!(alerts[0].tax_year, 2025);
    }

    /// The action must be on the *same listing* — another fund's return of
    /// capital clears nothing.
    #[tokio::test]
    async fn db_action_on_another_listing_does_not_clear_the_flag() {
        let pool = test_pool().await;
        test_support::listing(1).insert(&pool).await;
        test_support::listing(2).ticker("VGS").insert(&pool).await;
        insert_trust_income(&pool, 1, ymd(2025, 3, 15), Some("120.50")).await;
        corporate_action::db_upsert(
            &pool,
            &corporate_action::CorporateAction {
                id: 1,
                listing_id: 2,
                date: ymd(2025, 5, 1),
                kind: corporate_action::ActionKind::ReturnOfCapital {
                    amount_per_unit: "0.10".parse().unwrap(),
                    currency: "AUD".to_string(),
                },
            },
        )
        .await
        .unwrap();
        assert_eq!(db_e4_alerts(&pool).await.unwrap().len(), 1);
    }

    #[tokio::test]
    async fn api_get_e4_cross_check() {
        let pool = test_pool().await;
        test_support::listing(1).insert(&pool).await;
        insert_trust_income(&pool, 1, ymd(2025, 3, 15), Some("120.50")).await;
        let resp = ApiClient::over(router().with_state(pool))
            .get("/reports/e4_cross_check")
            .await;
        assert_eq!(resp.status, StatusCode::OK);
        let alerts: Vec<E4CrossCheckAlert> = resp.json();
        assert_eq!(alerts.len(), 1);
        assert_eq!(alerts[0].income_id, 1);
        assert_eq!(alerts[0].ticker, "T1");
    }
}
