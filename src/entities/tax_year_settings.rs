//! Per-financial-year taxpayer settings: facts about the taxpayer, not about a
//! holding, that are answered year by year.
//!
//! One row per Australian financial year (identified by the calendar year of
//! its 30 June end — `domain::tax_year`), holding
//! `ess_taxed_upfront_reduction_eligible`: whether the taxpayer's *adjusted
//! taxable income* for that year was within the A$180,000 limit for the $1,000
//! taxed-upfront ESS reduction (`docs/ato/employee-share-schemes.md`). That
//! test is over income this system does not hold, so it is recorded rather than
//! computed, and the tax summary reads it (`reports::tax_summary`).
//!
//! **Absent row = eligible.** An empty table behaves exactly as the system did
//! before the setting existed; only an explicitly ineligible year changes a
//! figure. Per year rather than on the `cgt_settings` singleton because the
//! income test is answered year by year and the tax summary reports every
//! recorded year at once — one global flag would strip the reduction from years
//! that never crossed the threshold.

use crate::infra::http::{self, ApiError, CrudEntity};
use axum::{
    Json, Router,
    extract::{Path, State},
    http::StatusCode,
    routing::get,
};
use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;
use std::collections::HashSet;

/// The first financial year that can carry settings: CGT starts 20 September
/// 1985, inside FY1986. Pinned by the table's CHECK too.
pub const FIRST_TAX_YEAR: i64 = 1986;

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct TaxYearSettings {
    /// The financial year, by the calendar year of its 30 June end.
    pub tax_year: i64,
    /// Whether the year's adjusted taxable income was within A$180,000, so the
    /// $1,000 taxed-upfront ESS reduction applies. Defaults to true, which is
    /// also what an absent row means.
    pub ess_taxed_upfront_reduction_eligible: bool,
}

#[derive(Debug, Deserialize)]
pub struct TaxYearSettingsBody {
    /// Defaults to true so a PUT can state only the ineligible case, and an
    /// omitted field can never silently *remove* a reduction.
    #[serde(default = "default_true")]
    pub ess_taxed_upfront_reduction_eligible: bool,
}

fn default_true() -> bool {
    true
}

impl CrudEntity for TaxYearSettings {
    type Key = i64;
    const TABLE: &'static str = "tax_year_settings";
    const COLUMNS: &'static str = "tax_year, ess_taxed_upfront_reduction_eligible";
    const KEY_COLUMN: &'static str = "tax_year";
    const ORDER_BY: &'static str = "tax_year";
    const NOUN: &'static str = "tax year settings row";
}

pub fn router() -> Router<SqlitePool> {
    Router::new()
        .route(
            "/tax_year_settings",
            get(http::list_handler::<TaxYearSettings>),
        )
        .route(
            "/tax_year_settings/{tax_year}",
            get(http::get_handler::<TaxYearSettings>)
                .put(upsert)
                .delete(http::delete_handler::<TaxYearSettings>),
        )
}

#[cfg(test)]
pub async fn db_get(
    pool: &SqlitePool,
    tax_year: i64,
) -> Result<Option<TaxYearSettings>, sqlx::Error> {
    http::crud_get(pool, tax_year).await
}

pub async fn db_upsert(pool: &SqlitePool, settings: &TaxYearSettings) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO tax_year_settings (tax_year, ess_taxed_upfront_reduction_eligible) \
         VALUES (?, ?) \
         ON CONFLICT(tax_year) DO UPDATE SET \
             ess_taxed_upfront_reduction_eligible = excluded.ess_taxed_upfront_reduction_eligible",
    )
    .bind(settings.tax_year)
    .bind(settings.ess_taxed_upfront_reduction_eligible)
    .execute(pool)
    .await?;
    Ok(())
}

/// The financial years recorded as **not** eligible for the $1,000 taxed-upfront
/// ESS reduction. Every other year (recorded eligible, or with no row at all)
/// keeps the reduction — the set is the exception list, so an empty table means
/// "apply it everywhere", which is what the system did before the setting
/// existed.
///
/// Executor-generic like `cgt_settings::db_opening_capital_loss`, so the tax
/// summary reads it inside its own single-snapshot read transaction.
pub async fn db_ineligible_tax_years<'e, E>(executor: E) -> Result<HashSet<i32>, sqlx::Error>
where
    E: sqlx::Executor<'e, Database = sqlx::Sqlite>,
{
    let years: Vec<i64> = sqlx::query_scalar(
        "SELECT tax_year FROM tax_year_settings \
         WHERE ess_taxed_upfront_reduction_eligible = 0",
    )
    .fetch_all(executor)
    .await?;
    Ok(years.into_iter().map(|y| y as i32).collect())
}

async fn upsert(
    State(pool): State<SqlitePool>,
    Path(tax_year): Path<i64>,
    Json(body): Json<TaxYearSettingsBody>,
) -> Result<StatusCode, ApiError> {
    // A year before CGT can hold no assessable ESS discount either (the ESS
    // provisions date from 1995 at the earliest), so a settings row for one is
    // a typo, not a position. The table CHECKs it as well; this answers with
    // the year rather than the constraint's wording.
    if tax_year < FIRST_TAX_YEAR {
        return Err(ApiError::unprocessable(format!(
            "tax year {tax_year} is before the first financial year CGT applies to ({FIRST_TAX_YEAR})"
        )));
    }
    let settings = TaxYearSettings {
        tax_year,
        ess_taxed_upfront_reduction_eligible: body.ess_taxed_upfront_reduction_eligible,
    };
    db_upsert(&pool, &settings)
        .await
        .map(|_| StatusCode::NO_CONTENT)
        .map_err(ApiError::from)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{ApiClient, test_pool};
    use axum::http::StatusCode;

    /// Client over this module's own routes.
    fn client(pool: &SqlitePool) -> ApiClient {
        ApiClient::over(router().with_state(pool.clone()))
    }

    #[tokio::test]
    async fn db_round_trips_a_year_and_updates_it_in_place() {
        let pool = test_pool().await;
        db_upsert(
            &pool,
            &TaxYearSettings {
                tax_year: 2026,
                ess_taxed_upfront_reduction_eligible: false,
            },
        )
        .await
        .unwrap();
        let got = db_get(&pool, 2026).await.unwrap().unwrap();
        assert_eq!(got.tax_year, 2026);
        assert!(!got.ess_taxed_upfront_reduction_eligible);

        // The same year again replaces rather than duplicating.
        db_upsert(
            &pool,
            &TaxYearSettings {
                tax_year: 2026,
                ess_taxed_upfront_reduction_eligible: true,
            },
        )
        .await
        .unwrap();
        assert!(
            db_get(&pool, 2026)
                .await
                .unwrap()
                .unwrap()
                .ess_taxed_upfront_reduction_eligible
        );
    }

    /// The exception list is what the tax summary consumes: only years recorded
    /// ineligible appear, and a year with no row at all never does.
    #[tokio::test]
    async fn only_years_recorded_ineligible_are_listed() {
        let pool = test_pool().await;
        assert!(db_ineligible_tax_years(&pool).await.unwrap().is_empty());
        for (year, eligible) in [(2024, true), (2025, false), (2026, false)] {
            db_upsert(
                &pool,
                &TaxYearSettings {
                    tax_year: year,
                    ess_taxed_upfront_reduction_eligible: eligible,
                },
            )
            .await
            .unwrap();
        }
        let ineligible = db_ineligible_tax_years(&pool).await.unwrap();
        assert_eq!(ineligible, HashSet::from([2025, 2026]));
    }

    #[tokio::test]
    async fn api_crud_round_trip() {
        let pool = test_pool().await;
        let c = client(&pool);
        c.put_ok(
            "/tax_year_settings/2026",
            &serde_json::json!({"ess_taxed_upfront_reduction_eligible": false}),
        )
        .await;
        let row: serde_json::Value = c.get_json("/tax_year_settings/2026").await;
        assert_eq!(row["tax_year"], serde_json::json!(2026));
        assert_eq!(
            row["ess_taxed_upfront_reduction_eligible"],
            serde_json::json!(false)
        );
        let listed: Vec<serde_json::Value> = c.get_json("/tax_year_settings").await;
        assert_eq!(listed.len(), 1);

        c.delete("/tax_year_settings/2026")
            .await
            .expect_status(StatusCode::NO_CONTENT);
        assert_eq!(
            c.get("/tax_year_settings/2026").await.status,
            StatusCode::NOT_FOUND
        );
    }

    /// An omitted flag means eligible: a PUT that forgets the field can never
    /// silently remove a reduction.
    #[tokio::test]
    async fn an_omitted_flag_defaults_to_eligible() {
        let pool = test_pool().await;
        client(&pool)
            .put_ok("/tax_year_settings/2026", &serde_json::json!({}))
            .await;
        assert!(
            db_get(&pool, 2026)
                .await
                .unwrap()
                .unwrap()
                .ess_taxed_upfront_reduction_eligible
        );
    }

    #[tokio::test]
    async fn api_a_pre_cgt_tax_year_is_rejected_naming_the_year() {
        let pool = test_pool().await;
        let resp = client(&pool)
            .put(
                "/tax_year_settings/1985",
                &serde_json::json!({"ess_taxed_upfront_reduction_eligible": false}),
            )
            .await;
        let (status, body) = resp.status_and_body();
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
        assert!(body.contains("1985"), "{body}");
        assert!(body.contains("1986"), "{body}");
        assert!(db_get(&pool, 1985).await.unwrap().is_none());
    }
}
