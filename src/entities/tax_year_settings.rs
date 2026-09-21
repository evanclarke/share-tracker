//! Per-financial-year taxpayer settings: facts about the taxpayer, not about a
//! holding, that are answered year by year.
//!
//! One row per Australian financial year (identified by the calendar year of
//! its 30 June end — `domain::tax_year`), holding two recorded facts:
//!
//! - `ess_taxed_upfront_reduction_eligible`: whether the taxpayer's *adjusted
//!   taxable income* for that year was within the A$180,000 limit for the $1,000
//!   taxed-upfront ESS reduction (`docs/ato/employee-share-schemes.md`). That
//!   test is over income this system does not hold, so it is recorded rather than
//!   computed, and the tax summary reads it (`reports::tax_summary`).
//! - `foreign_or_temporary_resident_at_some_time`: whether the taxpayer was a
//!   foreign or temporary resident at some time during that year — the s 114-25
//!   testing-period answer the CGT reform's indexation needs
//!   (`docs/ato/cgt-reform-cgt-adjustments.md`, EM 1.59/1.62;
//!   `domain::cgt_indexation::residency_for_testing_period`), read by
//!   `reports::realised_gains`. Added by migration `0053`.
//!
//! **Absent row = the standing assumption.** An empty table behaves exactly as
//! the system did before these settings existed: the ESS reduction applies, and
//! the taxpayer is an Australian resident throughout. Only an explicitly
//! recorded exception changes a figure — both fields are stored as the
//! *exception*, so a `PUT` that omits one leaves it at that assumption rather
//! than silently flipping it. Per year rather than on the `cgt_settings`
//! singleton because both facts are answered year by year and the tax summary
//! reports every recorded year at once — one global flag would change years
//! that never crossed a threshold or never left the country.

use crate::infra::http::{self, ApiError, CrudEntity};
use axum::{
    Json, Router,
    extract::{Path, State},
    http::StatusCode,
    routing::get,
};
use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;
use std::collections::{BTreeSet, HashSet};

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
    /// Whether the taxpayer was a **foreign or temporary resident at some time
    /// during the year** — the s 114-25 testing-period exception. Defaults to
    /// false (an Australian resident throughout), which is also what an absent
    /// row means, so only an explicitly recorded year denies indexation.
    pub foreign_or_temporary_resident_at_some_time: bool,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TaxYearSettingsBody {
    /// Defaults to true so a PUT can state only the ineligible case, and an
    /// omitted field can never silently *remove* a reduction.
    #[serde(default = "default_true")]
    pub ess_taxed_upfront_reduction_eligible: bool,
    /// Defaults to false so a PUT can state only the exception, and an omitted
    /// field can never silently *add* one — reading as "resident throughout",
    /// which is the standing assumption.
    #[serde(default)]
    pub foreign_or_temporary_resident_at_some_time: bool,
}

fn default_true() -> bool {
    true
}

impl CrudEntity for TaxYearSettings {
    type Key = i64;
    const TABLE: &'static str = "tax_year_settings";
    const COLUMNS: &'static str = "tax_year, ess_taxed_upfront_reduction_eligible, \
         foreign_or_temporary_resident_at_some_time";
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
        "INSERT INTO tax_year_settings \
             (tax_year, ess_taxed_upfront_reduction_eligible, \
              foreign_or_temporary_resident_at_some_time) \
         VALUES (?, ?, ?) \
         ON CONFLICT(tax_year) DO UPDATE SET \
             ess_taxed_upfront_reduction_eligible = excluded.ess_taxed_upfront_reduction_eligible, \
             foreign_or_temporary_resident_at_some_time = \
                 excluded.foreign_or_temporary_resident_at_some_time",
    )
    .bind(settings.tax_year)
    .bind(settings.ess_taxed_upfront_reduction_eligible)
    .bind(settings.foreign_or_temporary_resident_at_some_time)
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

/// The financial years recorded as containing a **foreign or temporary
/// residency period**, keyed by the calendar year of the year's 30 June end.
///
/// The exception list [`db_ineligible_tax_years`] is for the ESS income test,
/// and the same shape serves the s 114-25 residency testing period: every other
/// year (recorded resident, or with no row at all) is an Australian-resident
/// year, so an empty table means "resident throughout", which is what the
/// system assumed before the setting existed.
///
/// Executor-generic like its sibling, so `reports::realised_gains` reads it
/// inside its own single-snapshot read transaction.
pub async fn db_foreign_resident_years<'e, E>(executor: E) -> Result<BTreeSet<i32>, sqlx::Error>
where
    E: sqlx::Executor<'e, Database = sqlx::Sqlite>,
{
    let years: Vec<i64> = sqlx::query_scalar(
        "SELECT tax_year FROM tax_year_settings \
         WHERE foreign_or_temporary_resident_at_some_time = 1",
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
        foreign_or_temporary_resident_at_some_time: body.foreign_or_temporary_resident_at_some_time,
    };
    db_upsert(&pool, &settings)
        .await
        .map(|_| StatusCode::NO_CONTENT)
        .map_err(ApiError::from)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{ApiClient, test_pool, ymd};
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
                foreign_or_temporary_resident_at_some_time: true,
            },
        )
        .await
        .unwrap();
        let got = db_get(&pool, 2026).await.unwrap().unwrap();
        assert_eq!(got.tax_year, 2026);
        assert!(!got.ess_taxed_upfront_reduction_eligible);
        assert!(got.foreign_or_temporary_resident_at_some_time);

        // The same year again replaces rather than duplicating.
        db_upsert(
            &pool,
            &TaxYearSettings {
                tax_year: 2026,
                ess_taxed_upfront_reduction_eligible: true,
                foreign_or_temporary_resident_at_some_time: false,
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
        assert!(
            !db_get(&pool, 2026)
                .await
                .unwrap()
                .unwrap()
                .foreign_or_temporary_resident_at_some_time
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
                    foreign_or_temporary_resident_at_some_time: false,
                },
            )
            .await
            .unwrap();
        }
        let ineligible = db_ineligible_tax_years(&pool).await.unwrap();
        assert_eq!(ineligible, HashSet::from([2025, 2026]));
    }

    /// The residency exception list is the one the CGT reform's s 114-25
    /// testing period reads: only years recorded as containing a foreign or
    /// temporary period appear, so an empty table is the standing
    /// Australian-resident assumption.
    #[tokio::test]
    async fn only_years_recorded_foreign_are_listed() {
        let pool = test_pool().await;
        assert!(db_foreign_resident_years(&pool).await.unwrap().is_empty());
        for (year, foreign) in [(2027, false), (2028, true), (2029, true)] {
            db_upsert(
                &pool,
                &TaxYearSettings {
                    tax_year: year,
                    ess_taxed_upfront_reduction_eligible: true,
                    foreign_or_temporary_resident_at_some_time: foreign,
                },
            )
            .await
            .unwrap();
        }
        let foreign = db_foreign_resident_years(&pool).await.unwrap();
        assert_eq!(foreign, BTreeSet::from([2028, 2029]));
    }

    /// The s 114-25 testing period runs from 1 July 2027 (or acquisition, if
    /// later) to the event, so a flagged year inside that run denies
    /// indexation and one outside it never does.
    #[test]
    fn the_testing_period_covers_every_year_it_runs_through() {
        use crate::domain::cgt_indexation::{Residency, residency_for_testing_period};
        let years = BTreeSet::from([2029]);
        // 1 July 2027 – 30 June 2028 is FY2028, so a 2028 event's period is
        // just that year — the 2029 flag cannot reach it.
        assert_eq!(
            residency_for_testing_period(&years, ymd(2020, 1, 1), ymd(2028, 3, 1)),
            Residency::AustralianResidentThroughout
        );
        // An event in FY2029's own year is denied.
        assert_eq!(
            residency_for_testing_period(&years, ymd(2020, 1, 1), ymd(2029, 3, 1)),
            Residency::ForeignOrTemporaryAtSomeTime
        );
        // A parcel acquired after the commencement starts its period at
        // acquisition, so an earlier flagged year falls outside it: bought
        // August 2029 (FY2030), and the 2029 flag is FY2029.
        assert_eq!(
            residency_for_testing_period(&years, ymd(2029, 8, 1), ymd(2029, 10, 1)),
            Residency::AustralianResidentThroughout
        );
        // Flag the year the period actually opens in and it denies.
        assert_eq!(
            residency_for_testing_period(
                &BTreeSet::from([2030]),
                ymd(2029, 8, 1),
                ymd(2029, 10, 1)
            ),
            Residency::ForeignOrTemporaryAtSomeTime
        );
        // Nothing recorded: the standing assumption.
        assert_eq!(
            residency_for_testing_period(&BTreeSet::new(), ymd(2020, 1, 1), ymd(2029, 3, 1)),
            Residency::AustralianResidentThroughout
        );
    }

    #[tokio::test]
    async fn api_crud_round_trip() {
        let pool = test_pool().await;
        let c = client(&pool);
        c.put_ok(
            "/tax_year_settings/2026",
            &serde_json::json!({
                "ess_taxed_upfront_reduction_eligible": false,
                "foreign_or_temporary_resident_at_some_time": true,
            }),
        )
        .await;
        let row: serde_json::Value = c.get_json("/tax_year_settings/2026").await;
        assert_eq!(row["tax_year"], serde_json::json!(2026));
        assert_eq!(
            row["ess_taxed_upfront_reduction_eligible"],
            serde_json::json!(false)
        );
        assert_eq!(
            row["foreign_or_temporary_resident_at_some_time"],
            serde_json::json!(true)
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
        let row = db_get(&pool, 2026).await.unwrap().unwrap();
        assert!(row.ess_taxed_upfront_reduction_eligible);
        // And the residency exception is the other direction: a PUT that
        // forgets it can never silently *add* one.
        assert!(!row.foreign_or_temporary_resident_at_some_time);
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
