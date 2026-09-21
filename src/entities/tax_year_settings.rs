//! Per-financial-year taxpayer settings: facts about the taxpayer, not about a
//! holding, that are answered year by year.
//!
//! One row per Australian financial year (identified by the calendar year of
//! its 30 June end — `domain::tax_year`), holding four recorded facts:
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
//! - `minimum_tax_gap_amount`: the year's **Division 119 minimum tax gap
//!   amount** (the reform's 30 per cent minimum tax on capital gains), worked
//!   out by the taxpayer under s 119-10(2) and entered here. Steps 2–4 of that
//!   method statement need a basic income tax liability on a taxable income
//!   this project has never computed (there is no marginal-rate schedule and no
//!   salary/other-income model), so the gap is **recorded rather than computed**
//!   — the same decision shape as the ESS income test and the FITO cap. Absent
//!   (NULL) means no gap, which is the "already taxed at 30 per cent or more"
//!   case. Read by `reports::net_capital_gain`, which derives the step-1
//!   benchmark and the s 12AA rate around it. Added by migration `0054`.
//! - `minimum_tax_income_support_exempt`: whether the taxpayer received a
//!   payment of a kind the Minister prescribes under s 119-15 at any time in
//!   the year (the intended list is the income-support payments, EM
//!   1.186–1.190), which exempts them from the minimum tax entirely. Recorded
//!   rather than inferred: the instrument is not yet made and eligibility is a
//!   fact about the taxpayer this system cannot see. Added by migration `0054`.
//!
//! **Absent row = the standing assumption.** An empty table behaves exactly as
//! the system did before these settings existed: the ESS reduction applies, the
//! taxpayer is an Australian resident throughout, no minimum tax gap is
//! recorded, and no income-support exemption applies. Only an explicitly
//! recorded exception changes a figure — every field is stored as the
//! *exception*, so a `PUT` that omits one leaves it at that assumption rather
//! than silently flipping it. Per year rather than on the `cgt_settings`
//! singleton because every fact is answered year by year and the tax summary
//! reports every recorded year at once — one global flag would change years
//! that never crossed a threshold or never left the country.

use crate::infra::decimal::OptMoney;
use crate::infra::http::{self, ApiError, CrudEntity};
use axum::{
    Json, Router,
    extract::{Path, State},
    http::StatusCode,
    routing::get,
};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;
use std::collections::{BTreeSet, HashMap, HashSet};

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
    /// The year's Division 119 **minimum tax gap amount**, as the taxpayer
    /// worked it out under s 119-10(2) — a non-negative amount, AUD, or `None`
    /// (and an absent row) for no gap. The report derives the step-1 benchmark
    /// and the s 12AA rate around it; the gap itself is the taxpayer's own
    /// figure because steps 2–4 need a basic income tax liability this project
    /// does not compute.
    #[sqlx(try_from = "OptMoney")]
    pub minimum_tax_gap_amount: Option<Decimal>,
    /// Whether the taxpayer received a prescribed income-support payment at
    /// some time in the year, exempting them from the minimum tax (s 119-15).
    /// Defaults to false, which is also what an absent row means.
    pub minimum_tax_income_support_exempt: bool,
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
    /// Defaults to absent (no gap recorded) so a PUT that omits it leaves the
    /// standing assumption — the "already taxed at 30 per cent or more" case.
    #[serde(
        default,
        deserialize_with = "crate::infra::decimal::strict_optional_decimal"
    )]
    pub minimum_tax_gap_amount: Option<Decimal>,
    /// Defaults to false so a PUT can state only the exempt case, and an
    /// omitted field can never silently *add* an exemption.
    #[serde(default)]
    pub minimum_tax_income_support_exempt: bool,
}

fn default_true() -> bool {
    true
}

impl CrudEntity for TaxYearSettings {
    type Key = i64;
    const TABLE: &'static str = "tax_year_settings";
    const COLUMNS: &'static str = "tax_year, ess_taxed_upfront_reduction_eligible, \
         foreign_or_temporary_resident_at_some_time, minimum_tax_gap_amount, \
         minimum_tax_income_support_exempt";
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
              foreign_or_temporary_resident_at_some_time, minimum_tax_gap_amount, \
              minimum_tax_income_support_exempt) \
         VALUES (?, ?, ?, ?, ?) \
         ON CONFLICT(tax_year) DO UPDATE SET \
             ess_taxed_upfront_reduction_eligible = excluded.ess_taxed_upfront_reduction_eligible, \
             foreign_or_temporary_resident_at_some_time = \
                 excluded.foreign_or_temporary_resident_at_some_time, \
             minimum_tax_gap_amount = excluded.minimum_tax_gap_amount, \
             minimum_tax_income_support_exempt = excluded.minimum_tax_income_support_exempt",
    )
    .bind(settings.tax_year)
    .bind(settings.ess_taxed_upfront_reduction_eligible)
    .bind(settings.foreign_or_temporary_resident_at_some_time)
    .bind(OptMoney(settings.minimum_tax_gap_amount))
    .bind(settings.minimum_tax_income_support_exempt)
    .execute(pool)
    .await?;
    Ok(())
}

/// One year's Division 119 minimum-tax inputs as the net-capital-gain report
/// reads them: the recorded gap and whether the year is exempt.
///
/// `Default` is the standing assumption — no gap recorded (`0`) and not exempt
/// — so a year with no settings row needs no special case at the call site.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct MinimumTaxSettings {
    /// The recorded minimum tax gap amount for the year, `0` when none is
    /// recorded (the "already taxed at 30 per cent or more" case).
    pub gap_amount: Decimal,
    /// Whether the year carries the s 119-15 income-support exemption.
    pub income_support_exempt: bool,
}

/// Every year's recorded Division 119 inputs, keyed by the calendar year of
/// its 30 June end. Only years with a settings row carry an entry; a year
/// absent from the map takes [`MinimumTaxSettings::default`].
///
/// Executor-generic like [`db_foreign_resident_years`], so
/// `reports::net_capital_gain` reads it inside its own single-snapshot read
/// transaction — the report walks every year at once, so it loads the whole
/// (small) table rather than querying per year.
pub async fn db_minimum_tax_settings<'e, E>(
    executor: E,
) -> Result<HashMap<i32, MinimumTaxSettings>, sqlx::Error>
where
    E: sqlx::Executor<'e, Database = sqlx::Sqlite>,
{
    let rows: Vec<(i64, OptMoney, bool)> = sqlx::query_as(
        "SELECT tax_year, minimum_tax_gap_amount, minimum_tax_income_support_exempt \
         FROM tax_year_settings",
    )
    .fetch_all(executor)
    .await?;
    Ok(rows
        .into_iter()
        .map(|(tax_year, gap_amount, income_support_exempt)| {
            (
                tax_year as i32,
                MinimumTaxSettings {
                    gap_amount: gap_amount.0.unwrap_or(Decimal::ZERO),
                    income_support_exempt,
                },
            )
        })
        .collect())
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
    // A negative minimum tax gap amount cannot arise under s 119-10(2): step 4
    // subtracts a larger liability from a smaller one, so it is never negative,
    // and step 7 makes a nil-or-negative step 5 result no gap at all. A negative
    // entry is therefore a typo and would make the s 12AA rate negative, so it
    // is refused here rather than carried into a report.
    if body
        .minimum_tax_gap_amount
        .is_some_and(|gap| gap < Decimal::ZERO)
    {
        return Err(ApiError::unprocessable(
            "the minimum tax gap amount must not be negative — step 7 of the s 119-10(2) method \
             statement makes a nil or negative result no gap at all"
                .to_string(),
        ));
    }
    let settings = TaxYearSettings {
        tax_year,
        ess_taxed_upfront_reduction_eligible: body.ess_taxed_upfront_reduction_eligible,
        foreign_or_temporary_resident_at_some_time: body.foreign_or_temporary_resident_at_some_time,
        minimum_tax_gap_amount: body.minimum_tax_gap_amount,
        minimum_tax_income_support_exempt: body.minimum_tax_income_support_exempt,
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
                minimum_tax_gap_amount: Some("500".parse().unwrap()),
                minimum_tax_income_support_exempt: true,
            },
        )
        .await
        .unwrap();
        let got = db_get(&pool, 2026).await.unwrap().unwrap();
        assert_eq!(got.tax_year, 2026);
        assert!(!got.ess_taxed_upfront_reduction_eligible);
        assert!(got.foreign_or_temporary_resident_at_some_time);
        assert_eq!(got.minimum_tax_gap_amount, Some("500".parse().unwrap()));
        assert!(got.minimum_tax_income_support_exempt);

        // The same year again replaces rather than duplicating.
        db_upsert(
            &pool,
            &TaxYearSettings {
                tax_year: 2026,
                ess_taxed_upfront_reduction_eligible: true,
                foreign_or_temporary_resident_at_some_time: false,
                minimum_tax_gap_amount: None,
                minimum_tax_income_support_exempt: false,
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
        // The minimum-tax columns replace in place too — `None` clears the gap.
        let got = db_get(&pool, 2026).await.unwrap().unwrap();
        assert_eq!(got.minimum_tax_gap_amount, None);
        assert!(!got.minimum_tax_income_support_exempt);
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
                    minimum_tax_gap_amount: None,
                    minimum_tax_income_support_exempt: false,
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
                    minimum_tax_gap_amount: None,
                    minimum_tax_income_support_exempt: false,
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
                "minimum_tax_gap_amount": "500",
                "minimum_tax_income_support_exempt": true,
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
        // Money crosses the wire as an exact decimal string (the `Money` codec
        // rule); the two new columns survive the round trip.
        assert_eq!(row["minimum_tax_gap_amount"], serde_json::json!("500"));
        assert_eq!(
            row["minimum_tax_income_support_exempt"],
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
        // The Division 119 inputs are the same direction as the residency
        // exception: omitting them can never silently add a gap or an
        // exemption — the standing assumption is no gap and not exempt.
        assert_eq!(row.minimum_tax_gap_amount, None);
        assert!(!row.minimum_tax_income_support_exempt);
    }

    /// The Division 119 reader the net-capital-gain report consumes: a year
    /// with no settings row takes the standing assumption (no gap, not
    /// exempt), and a recorded year carries its own figures. `NULL` and a
    /// recorded `0` are both "no gap" at the reader.
    #[tokio::test]
    async fn db_minimum_tax_settings_reads_the_recorded_years() {
        let pool = test_pool().await;
        assert!(db_minimum_tax_settings(&pool).await.unwrap().is_empty());

        for (year, gap, exempt) in [
            (2028, None, false),
            (2029, Some("500"), false),
            (2030, Some("0"), true),
        ] {
            db_upsert(
                &pool,
                &TaxYearSettings {
                    tax_year: year,
                    ess_taxed_upfront_reduction_eligible: true,
                    foreign_or_temporary_resident_at_some_time: false,
                    minimum_tax_gap_amount: gap.map(|g| g.parse().unwrap()),
                    minimum_tax_income_support_exempt: exempt,
                },
            )
            .await
            .unwrap();
        }

        let recorded = db_minimum_tax_settings(&pool).await.unwrap();
        assert_eq!(
            recorded.get(&2028),
            Some(&MinimumTaxSettings {
                gap_amount: Decimal::ZERO,
                income_support_exempt: false,
            })
        );
        assert_eq!(
            recorded.get(&2029),
            Some(&MinimumTaxSettings {
                gap_amount: "500".parse().unwrap(),
                income_support_exempt: false,
            })
        );
        assert_eq!(
            recorded.get(&2030),
            Some(&MinimumTaxSettings {
                gap_amount: Decimal::ZERO,
                income_support_exempt: true,
            })
        );
        // A year with no row is simply absent, and its default is the
        // standing assumption.
        assert_eq!(recorded.get(&2031), None);
        assert_eq!(MinimumTaxSettings::default().gap_amount, Decimal::ZERO);
        assert!(!MinimumTaxSettings::default().income_support_exempt);
    }

    /// A negative gap cannot arise under s 119-10(2) (step 4 is never negative
    /// and step 7 floors a nil-or-negative result at no gap), so it is refused
    /// at write time rather than carried into the s 12AA rate as a negative.
    #[tokio::test]
    async fn api_a_negative_minimum_tax_gap_is_rejected() {
        let pool = test_pool().await;
        let resp = client(&pool)
            .put(
                "/tax_year_settings/2029",
                &serde_json::json!({"minimum_tax_gap_amount": "-1"}),
            )
            .await;
        let (status, body) = resp.status_and_body();
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
        assert!(body.contains("must not be negative"), "{body}");
        assert!(db_get(&pool, 2029).await.unwrap().is_none());
    }

    /// The two Division 119 columns land in the append-only audit trail, whose
    /// trigger pair migration 0054 re-created (the live-schema coverage test
    /// `reports::row_history::every_audited_column_is_recorded_by_both_triggers`
    /// is the structural half; this pins the values a real update records).
    #[tokio::test]
    async fn the_minimum_tax_columns_are_audited() {
        let pool = test_pool().await;
        // The first write is an INSERT (which no audit trigger covers); the
        // second is the UPDATE the trail records.
        for exempt in [false, true] {
            db_upsert(
                &pool,
                &TaxYearSettings {
                    tax_year: 2029,
                    ess_taxed_upfront_reduction_eligible: true,
                    foreign_or_temporary_resident_at_some_time: false,
                    minimum_tax_gap_amount: Some("500".parse().unwrap()),
                    minimum_tax_income_support_exempt: exempt,
                },
            )
            .await
            .unwrap();
        }
        let recorded: String = sqlx::query_scalar(
            "SELECT old_row FROM row_history \
             WHERE table_name = 'tax_year_settings' AND row_id = 2029 AND operation = 'UPDATE'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        let recorded: serde_json::Value = serde_json::from_str(&recorded).unwrap();
        assert_eq!(recorded["minimum_tax_gap_amount"], serde_json::json!("500"));
        assert_eq!(
            recorded["minimum_tax_income_support_exempt"],
            serde_json::json!(0)
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
