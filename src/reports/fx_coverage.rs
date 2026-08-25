//! FX coverage cross-check: does every recorded amount have the ATO rate it
//! needs, and which amounts sit on a documented FX simplification?
//!
//! **Non-blocking**, like [`settlement_coverage`](super::settlement_coverage),
//! whose shape this follows: writes are never rejected because of what is in
//! here, and an empty report is the statement that every non-AUD amount
//! converts at a published rate with no simplification in play.
//!
//! Three things go silent without it (SCENARIOS M-04, M-09, M-10, M-14), all
//! of them identifiable from stored facts:
//!
//! 1. **A missing monthly rate.** [`reports::health`](super::health) reports
//!    only the *newest* imported month across all currencies, which answers
//!    "has the import run lately", not "is every amount I have recorded
//!    convertible". A hole in the middle of a series — the RBA F11 CSV leaves
//!    a currency's cell empty for a month, and the import skips it — is
//!    invisible until a report needs it. Then one of two things happens: an
//!    amount with a per-record fallback is **silently costed at that manual
//!    rate** (a trade left at its default parity would cost a US$15,000 parcel
//!    at A$15,000), and one without a fallback — income, an AMMA statement, a
//!    return of capital — fails the whole report.
//! 2. **A settlement window crossing a rate month** — CGT event K10/K11, a
//!    documented Known limitation. Both legs of a non-AUD trade translate at
//!    the *contract* month, so the currency movement to settlement is dropped;
//!    a trade at risk is exactly one whose `date` and `settlement_date` fall in
//!    different months.
//! 3. **A cost-base reduction converted at the acquisition month** — the other
//!    documented FX simplification. A parcel's AMIT (CGT event E10) and
//!    return-of-capital (G1) reductions convert at the parcel's own acquisition
//!    month rather than the month they arose in, which keeps the breakdown
//!    internally consistent in AUD and is only visible on a non-AUD parcel.
//!
//! The third member of that family — LPR expenditure on a foreign inherited
//! parcel — is *refused* at write time rather than surfaced, because there the
//! amount has nowhere correct to go at all. These two produce a defensible
//! figure, so they are reported rather than blocked.

use crate::infra::fx::FxRates;
use crate::infra::http::ApiError;
use axum::{Json, Router, extract::State, routing::get};
use chrono::{Datelike, NaiveDate};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use sqlx::{Row, SqlitePool};

/// One recorded amount worth a second look before a tax figure rests on it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FxCoverageAlert {
    /// `missing_rate`, `settlement_crosses_rate_month`, or
    /// `reduction_converted_at_acquisition_month`.
    pub kind: String,
    /// The table the record lives in, as the screens name it (`trade`,
    /// `income`, `AMMA statement`, …).
    pub record: String,
    pub record_id: i64,
    pub listing_id: Option<i64>,
    pub ticker: Option<String>,
    /// The record's own date — the one whose month anchors its conversion.
    pub date: NaiveDate,
    pub currency: String,
    /// The conversion month (`YYYY-MM`) the alert is about: the missing one,
    /// the settlement month, or the reduction's own month.
    pub month: String,
    /// `missing_rate` only: what the conversion currently rests on —
    /// `spot_override` (a deliberate transaction-date rate, so nothing is
    /// wrong), `record_fx_rate` (the record's manual fallback, silently), or
    /// `nothing` (the report will fail until the month is imported).
    pub resting_on: Option<String>,
    /// A sentence naming what is at stake, so the row is actionable without
    /// cross-referencing the documentation.
    pub detail: String,
}

pub fn router() -> Router<SqlitePool> {
    Router::new().route("/reports/fx_coverage", get(report))
}

/// One record that needs an AUD conversion, as the union query below reads it.
struct Convertible {
    record: &'static str,
    id: i64,
    listing_id: Option<i64>,
    ticker: Option<String>,
    date: NaiveDate,
    currency: String,
    /// The per-record fallback, when the record has one and it is set.
    fallback: Option<Decimal>,
    /// Whether that fallback is a deliberate spot override rather than a
    /// silent stand-in.
    spot: bool,
}

/// `YYYY-MM` — the conversion month key `rba_fx_rates` is stored under.
fn month_of(date: NaiveDate) -> String {
    format!("{:04}-{:02}", date.year(), date.month())
}

/// Every non-AUD amount that converts, with the date whose month anchors it
/// and whatever fallback its own row carries.
///
/// Deliberately omits two record kinds whose conversion is already covered by
/// a row that *is* here: an [inheritance](crate::entities::inheritance) and an
/// [ESS vest](crate::entities::ess_vest) each resolve their rate once and
/// carry it onto the parcel Buy they create, so the trade row is the one that
/// matters. An ESS statement's *income* side converts from the statement
/// itself, so that row is included.
async fn load_convertibles(
    conn: &mut sqlx::SqliteConnection,
) -> Result<Vec<Convertible>, sqlx::Error> {
    let mut out = Vec::new();
    // (label, SQL) — each yields id, listing_id, ticker, date, currency, and
    // the record's fallback columns (NULL where the record has none).
    let sources: [(&'static str, &'static str); 8] = [
        (
            "trade",
            "SELECT t.id, t.listing_id, l.ticker, t.date, t.currency, t.fx_rate AS fallback, \
                    t.spot_fx_rate AS spot \
             FROM trades t JOIN listings l ON l.id = t.listing_id WHERE t.currency <> 'AUD'",
        ),
        (
            "income",
            "SELECT i.id, i.listing_id, l.ticker, i.date_paid AS date, i.currency, \
                    NULL AS fallback, NULL AS spot \
             FROM income i JOIN listings l ON l.id = i.listing_id WHERE i.currency <> 'AUD'",
        ),
        (
            "AMMA statement",
            "SELECT a.id, a.listing_id, l.ticker, a.tax_year_end_date AS date, a.currency, \
                    NULL AS fallback, NULL AS spot \
             FROM amma_statements a JOIN listings l ON l.id = a.listing_id \
             WHERE a.currency <> 'AUD'",
        ),
        (
            "ESS statement",
            "SELECT e.id, e.listing_id, l.ticker, e.taxing_point_date AS date, e.currency, \
                    e.fx_rate AS fallback, NULL AS spot \
             FROM ess_statements e JOIN listings l ON l.id = e.listing_id \
             WHERE e.currency <> 'AUD'",
        ),
        (
            "interest income",
            "SELECT id, NULL AS listing_id, NULL AS ticker, date_paid AS date, currency, \
                    NULL AS fallback, NULL AS spot \
             FROM interest_income WHERE currency <> 'AUD'",
        ),
        (
            "investment expense",
            "SELECT e.id, e.listing_id, l.ticker, e.date_incurred AS date, e.currency, \
                    NULL AS fallback, NULL AS spot \
             FROM investment_expenses e LEFT JOIN listings l ON l.id = e.listing_id \
             WHERE e.currency <> 'AUD'",
        ),
        (
            "return of capital",
            "SELECT c.id, c.listing_id, l.ticker, c.date, c.currency, \
                    NULL AS fallback, NULL AS spot \
             FROM corporate_actions c JOIN listings l ON l.id = c.listing_id \
             WHERE c.action_type = 'ReturnOfCapital' AND c.currency IS NOT NULL \
               AND c.currency <> 'AUD'",
        ),
        // Both legs of a rights sale — proceeds and rights cost — convert at
        // the *sale* month in the issue's currency, with the row's own
        // `fx_rate` as the manual fallback (`reports::realised_gains`), so a
        // missing month rests on that stored rate like a trade's does.
        (
            "rights sale",
            "SELECT rs.id, ca.listing_id, l.ticker, rs.date, ca.currency, \
                    rs.fx_rate AS fallback, NULL AS spot \
             FROM rights_sales rs \
             JOIN corporate_actions ca ON ca.id = rs.rights_action_id \
             JOIN listings l ON l.id = ca.listing_id \
             WHERE ca.currency <> 'AUD'",
        ),
    ];
    for (record, sql) in sources {
        let rows = sqlx::query(sql).fetch_all(&mut *conn).await?;
        for row in &rows {
            let fallback: Option<String> = row.try_get("fallback")?;
            let spot: Option<String> = row.try_get("spot")?;
            let parse = |raw: Option<String>| -> Result<Option<Decimal>, sqlx::Error> {
                raw.map(|r| crate::infra::decimal::parse_dec("fx_rate", r))
                    .transpose()
            };
            let (spot_rate, fallback_rate) = (parse(spot)?, parse(fallback)?);
            out.push(Convertible {
                record,
                id: row.try_get("id")?,
                listing_id: row.try_get("listing_id")?,
                ticker: row.try_get("ticker")?,
                date: row.try_get("date")?,
                currency: row.try_get("currency")?,
                fallback: spot_rate.or(fallback_rate),
                spot: spot_rate.is_some(),
            });
        }
    }
    Ok(out)
}

/// Every alert, newest record first within each kind.
///
/// Reads its inputs on one `pool.begin()` read transaction — a consistent
/// snapshot, so a rate imported part-way through cannot make one query think a
/// month is covered while another thinks it is not.
pub async fn db_fx_coverage(pool: &SqlitePool) -> Result<Vec<FxCoverageAlert>, sqlx::Error> {
    let mut tx = pool.begin().await?;
    let fx = FxRates::load(&mut *tx).await?;
    let mut alerts = Vec::new();

    for c in load_convertibles(&mut tx).await? {
        let month = month_of(c.date);
        if fx.has_rate(&c.currency, &month) {
            continue;
        }
        let (resting_on, detail) = match (c.fallback, c.spot) {
            (Some(rate), true) => (
                "spot_override",
                format!(
                    "no ATO rate for {} in {month}; this record converts at its own spot rate of \
                     {rate}, which is a deliberate transaction-date rate — nothing is missing, but \
                     importing the month will not change it either",
                    c.currency
                ),
            ),
            (Some(rate), false) => (
                "record_fx_rate",
                format!(
                    "no ATO rate for {} in {month}, so this record silently converts at its own \
                     stated rate of {rate}. Import that month's RBA rates and the published rate \
                     takes over",
                    c.currency
                ),
            ),
            (None, _) => (
                "nothing",
                format!(
                    "no ATO rate for {} in {month} and this record carries no rate of its own, so \
                     every report that has to convert it fails until the month is imported",
                    c.currency
                ),
            ),
        };
        alerts.push(FxCoverageAlert {
            kind: "missing_rate".to_string(),
            record: c.record.to_string(),
            record_id: c.id,
            listing_id: c.listing_id,
            ticker: c.ticker,
            date: c.date,
            currency: c.currency,
            month,
            resting_on: Some(resting_on.to_string()),
            detail,
        });
    }

    // CGT event K10/K11: both legs translate at the contract month, so a
    // settlement in a different one drops the currency movement between them.
    let settlements = sqlx::query(
        "SELECT t.id, t.listing_id, l.ticker, t.date, t.settlement_date, t.currency \
         FROM trades t JOIN listings l ON l.id = t.listing_id \
         WHERE t.currency <> 'AUD' \
           AND strftime('%Y-%m', t.date) <> strftime('%Y-%m', t.settlement_date) \
         ORDER BY t.date DESC, t.id DESC",
    )
    .fetch_all(&mut *tx)
    .await?;
    for row in &settlements {
        let date: NaiveDate = row.try_get("date")?;
        let settlement_date: NaiveDate = row.try_get("settlement_date")?;
        let currency: String = row.try_get("currency")?;
        alerts.push(FxCoverageAlert {
            kind: "settlement_crosses_rate_month".to_string(),
            record: "trade".to_string(),
            record_id: row.try_get("id")?,
            listing_id: row.try_get("listing_id")?,
            ticker: row.try_get("ticker")?,
            date,
            month: month_of(settlement_date),
            detail: format!(
                "contracted in {} and settled in {} — both legs convert at the contract month, so \
                 the {currency} movement over the settlement window is not computed (CGT event \
                 K10/K11, a documented limitation). A material movement is the taxpayer's own \
                 adjustment",
                month_of(date),
                month_of(settlement_date)
            ),
            currency,
            resting_on: None,
        });
    }

    // The cost-base FX-timing simplification: a non-AUD parcel's AMIT and
    // return-of-capital reductions convert at the parcel's acquisition month.
    let reductions = sqlx::query(
        "SELECT t.id AS id, t.listing_id AS listing_id, l.ticker AS ticker, \
                t.date AS date, t.currency AS currency, \
                s.tax_year_end_date AS reduction_date, 'AMIT' AS reduction \
         FROM amit_adjustments adj \
         JOIN trades t ON t.id = adj.trade_id \
         JOIN amma_statements s ON s.id = adj.amma_statement_id \
         JOIN listings l ON l.id = t.listing_id \
         WHERE t.currency <> 'AUD' \
           AND strftime('%Y-%m', s.tax_year_end_date) <> strftime('%Y-%m', t.date) \
         UNION ALL \
         SELECT t.id AS id, t.listing_id AS listing_id, l.ticker AS ticker, \
                t.date AS date, t.currency AS currency, \
                c.date AS reduction_date, 'return of capital' AS reduction \
         FROM corporate_actions c \
         JOIN trades t ON t.listing_id = c.listing_id \
         JOIN listings l ON l.id = t.listing_id \
         WHERE c.action_type = 'ReturnOfCapital' \
           AND t.trade_type IN ('Buy', 'DRP') AND t.currency <> 'AUD' \
           AND t.date <= COALESCE(c.record_date, c.date) \
           AND strftime('%Y-%m', c.date) <> strftime('%Y-%m', t.date) \
         ORDER BY reduction_date DESC, id DESC",
    )
    .fetch_all(&mut *tx)
    .await?;
    for row in &reductions {
        let date: NaiveDate = row.try_get("date")?;
        let reduction_date: NaiveDate = row.try_get("reduction_date")?;
        let currency: String = row.try_get("currency")?;
        let reduction: String = row.try_get("reduction")?;
        // Both rates where they resolve, so the row says what the difference
        // actually costs rather than only that one exists.
        let rates = match (
            fx.rate_for(&currency, &month_of(date)),
            fx.rate_for(&currency, &month_of(reduction_date)),
        ) {
            (Some(acquisition), Some(own)) if acquisition != own => {
                format!(" (converted at {acquisition}, its own month's rate is {own})")
            }
            _ => String::new(),
        };
        alerts.push(FxCoverageAlert {
            kind: "reduction_converted_at_acquisition_month".to_string(),
            record: "trade".to_string(),
            record_id: row.try_get("id")?,
            listing_id: row.try_get("listing_id")?,
            ticker: row.try_get("ticker")?,
            date,
            month: month_of(reduction_date),
            detail: format!(
                "this {currency} parcel takes a {reduction} reduction from {}, which converts at \
                 the parcel's own acquisition month of {} instead{rates} — the documented \
                 single-rate cost base. A material difference is adjusted by hand at the \
                 payment-period rate",
                month_of(reduction_date),
                month_of(date)
            ),
            currency,
            resting_on: None,
        });
    }

    tx.commit().await?;
    Ok(alerts)
}

async fn report(State(pool): State<SqlitePool>) -> Result<Json<Vec<FxCoverageAlert>>, ApiError> {
    db_fx_coverage(&pool)
        .await
        .map(Json)
        .map_err(ApiError::from)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::entities::{corporate_action, rba_fx_rate};
    use crate::test_support::{self, ApiClient, dec, test_pool, ymd};
    use axum::http::StatusCode;

    async fn usd_listing(pool: &SqlitePool, id: i64, ticker: &str) {
        test_support::listing(id)
            .mic("XNYS")
            .ticker(ticker)
            .name(ticker)
            .currency("USD")
            .insert(pool)
            .await;
    }

    async fn rate(pool: &SqlitePool, month: &str, rate: &str) {
        rba_fx_rate::db_import_rate(pool, "USD", month, rate.parse().unwrap())
            .await
            .unwrap();
    }

    fn of_kind<'a>(alerts: &'a [FxCoverageAlert], kind: &str) -> Vec<&'a FxCoverageAlert> {
        alerts.iter().filter(|a| a.kind == kind).collect()
    }

    /// An AUD-only portfolio needs no rate and takes no simplification, so the
    /// report is empty — which is the statement it exists to make.
    #[tokio::test]
    async fn db_an_aud_portfolio_reports_nothing() {
        let pool = test_pool().await;
        test_support::listing(1).insert(&pool).await;
        test_support::buy(1, 1)
            .date(ymd(2024, 1, 30))
            .settlement(ymd(2024, 2, 1))
            .insert(&pool)
            .await;
        test_support::income(1, 1, ymd(2024, 3, 10))
            .insert(&pool)
            .await;
        assert!(db_fx_coverage(&pool).await.unwrap().is_empty());
    }

    /// The gap `reports::health` cannot see: a hole in the middle of an
    /// imported series. Health reports the newest month across all currencies,
    /// so a February that never landed is invisible there while the amounts
    /// dated inside it quietly rest on something else — or fail
    /// (SCENARIOS M-04, M-14).
    #[tokio::test]
    async fn db_a_hole_in_the_series_names_every_amount_that_falls_in_it() {
        let pool = test_pool().await;
        usd_listing(&pool, 1, "AAPL").await;
        // January and March imported; February is the hole.
        rate(&pool, "2024-01", "0.65").await;
        rate(&pool, "2024-03", "0.66").await;

        // A trade in the hole rests on its own fx_rate, silently.
        test_support::buy(1, 1)
            .date(ymd(2024, 2, 12))
            .fx_rate(dec("0.99"))
            .insert(&pool)
            .await;
        // …unless it carries a deliberate spot rate, which is not a gap at all.
        test_support::buy(2, 1)
            .date(ymd(2024, 2, 12))
            .fx_rate(dec("0.99"))
            .spot_fx_rate(dec("0.6712"))
            .insert(&pool)
            .await;
        // Income has no fallback: the report fails until the month lands.
        test_support::income(1, 1, ymd(2024, 2, 20))
            .with(|i| i.currency = "USD".to_string())
            .insert(&pool)
            .await;
        // A trade in an imported month is not mentioned.
        test_support::buy(3, 1)
            .date(ymd(2024, 1, 16))
            .insert(&pool)
            .await;

        let alerts = db_fx_coverage(&pool).await.unwrap();
        let missing = of_kind(&alerts, "missing_rate");
        assert_eq!(missing.len(), 3, "{alerts:#?}");
        let resting = |id: i64, record: &str| {
            missing
                .iter()
                .find(|a| a.record_id == id && a.record == record)
                .unwrap_or_else(|| panic!("no alert for {record} {id}"))
        };
        assert_eq!(
            resting(1, "trade").resting_on.as_deref(),
            Some("record_fx_rate")
        );
        assert_eq!(
            resting(2, "trade").resting_on.as_deref(),
            Some("spot_override")
        );
        assert_eq!(resting(1, "income").resting_on.as_deref(), Some("nothing"));
        for a in &missing {
            assert_eq!(a.month, "2024-02");
            assert_eq!(a.currency, "USD");
        }
        // Importing the missing month clears every one of them.
        rate(&pool, "2024-02", "0.655").await;
        assert!(of_kind(&db_fx_coverage(&pool).await.unwrap(), "missing_rate").is_empty());
    }

    /// A settlement window crossing a rate month is where the K10/K11
    /// omission becomes visible, and it is identifiable from the two dates
    /// alone (SCENARIOS M-09).
    #[tokio::test]
    async fn db_a_settlement_crossing_a_rate_month_is_flagged() {
        let pool = test_pool().await;
        usd_listing(&pool, 1, "AAPL").await;
        rate(&pool, "2024-03", "0.66").await;
        rate(&pool, "2024-04", "0.60").await;
        // Contracted 27 March, settled 2 April.
        test_support::buy(1, 1)
            .date(ymd(2024, 3, 27))
            .settlement(ymd(2024, 4, 2))
            .insert(&pool)
            .await;
        // Same month either side: nil by construction, nothing to say.
        test_support::buy(2, 1)
            .date(ymd(2024, 3, 5))
            .settlement(ymd(2024, 3, 7))
            .insert(&pool)
            .await;
        // An AUD trade never converts at all.
        test_support::listing(2)
            .ticker("BHP")
            .name("BHP")
            .insert(&pool)
            .await;
        test_support::buy(3, 2)
            .date(ymd(2024, 3, 27))
            .settlement(ymd(2024, 4, 2))
            .insert(&pool)
            .await;

        let alerts = db_fx_coverage(&pool).await.unwrap();
        let crossing = of_kind(&alerts, "settlement_crosses_rate_month");
        assert_eq!(crossing.len(), 1, "{alerts:#?}");
        assert_eq!(crossing[0].record_id, 1);
        assert_eq!(crossing[0].month, "2024-04");
        assert!(
            crossing[0].detail.contains("K10/K11"),
            "{}",
            crossing[0].detail
        );
    }

    /// The other documented simplification: a non-AUD parcel's AMIT and
    /// return-of-capital reductions convert at the parcel's own acquisition
    /// month. Where both months' rates are imported the row says what the
    /// difference costs (SCENARIOS M-10).
    #[tokio::test]
    async fn db_a_reduction_from_another_month_is_flagged_with_both_rates() {
        let pool = test_pool().await;
        test_support::listing(1)
            .mic("XNYS")
            .ticker("VTS")
            .name("VTS")
            .currency("USD")
            .amit(true)
            .insert(&pool)
            .await;
        rate(&pool, "2022-08", "0.70").await;
        rate(&pool, "2024-06", "0.60").await;
        test_support::buy(1, 1)
            .date(ymd(2022, 8, 10))
            .qty(dec("1000"))
            .price(dec("100"))
            .insert(&pool)
            .await;
        test_support::amma(1, 1)
            .units(dec("1000"))
            .cost_base_adjustment(dec("2"))
            .with(|a| {
                a.tax_year_end_date = ymd(2024, 6, 30);
                a.date_received = ymd(2024, 8, 15);
            })
            .insert(&pool)
            .await;
        test_support::amit_adjustment(&pool, 1, 1, 1, dec("1000")).await;

        let alerts = db_fx_coverage(&pool).await.unwrap();
        let reductions = of_kind(&alerts, "reduction_converted_at_acquisition_month");
        assert_eq!(reductions.len(), 1, "{alerts:#?}");
        assert_eq!(reductions[0].record_id, 1);
        assert_eq!(reductions[0].month, "2024-06");
        let detail = &reductions[0].detail;
        assert!(detail.contains("AMIT"), "{detail}");
        assert!(
            detail.contains("0.70") && detail.contains("0.60"),
            "the row names both rates: {detail}"
        );
    }

    /// An AUD parcel taking the same reduction is not flagged: with one
    /// currency there is no conversion for the month to matter to. This is the
    /// case the Known limitation calls the one that actually arises.
    #[tokio::test]
    async fn db_an_aud_parcels_reduction_is_not_flagged() {
        let pool = test_pool().await;
        test_support::listing(1)
            .ticker("VDHG")
            .name("VDHG")
            .amit(true)
            .insert(&pool)
            .await;
        test_support::buy(1, 1)
            .date(ymd(2022, 8, 10))
            .qty(dec("1000"))
            .insert(&pool)
            .await;
        test_support::amma(1, 1)
            .units(dec("1000"))
            .cost_base_adjustment(dec("2"))
            .with(|a| {
                a.tax_year_end_date = ymd(2024, 6, 30);
                a.date_received = ymd(2024, 8, 15);
            })
            .insert(&pool)
            .await;
        test_support::amit_adjustment(&pool, 1, 1, 1, dec("1000")).await;
        assert!(db_fx_coverage(&pool).await.unwrap().is_empty());
    }

    /// A return of capital reaching a non-AUD parcel from a later month is the
    /// G1 half of the same simplification.
    #[tokio::test]
    async fn db_a_return_of_capital_from_another_month_is_flagged() {
        let pool = test_pool().await;
        usd_listing(&pool, 1, "USL").await;
        rate(&pool, "2023-01", "0.70").await;
        rate(&pool, "2024-05", "0.60").await;
        test_support::buy(1, 1)
            .date(ymd(2023, 1, 10))
            .insert(&pool)
            .await;
        // A parcel acquired after the payment is never reached by it.
        test_support::buy(2, 1)
            .date(ymd(2024, 6, 11))
            .insert(&pool)
            .await;
        corporate_action::db_upsert(
            &pool,
            &corporate_action::CorporateAction {
                id: 1,
                listing_id: 1,
                date: ymd(2024, 5, 1),
                kind: corporate_action::ActionKind::ReturnOfCapital {
                    amount_per_unit: dec("0.50"),
                    currency: "USD".to_string(),
                    record_date: None,
                },
            },
        )
        .await
        .unwrap();

        let alerts = db_fx_coverage(&pool).await.unwrap();
        let reductions = of_kind(&alerts, "reduction_converted_at_acquisition_month");
        assert_eq!(reductions.len(), 1, "{alerts:#?}");
        assert_eq!(reductions[0].record_id, 1);
        assert!(
            reductions[0].detail.contains("return of capital"),
            "{}",
            reductions[0].detail
        );
    }

    /// A foreign-currency rights sale converts its proceeds and rights cost
    /// at the sale month, so a missing month must surface here like every
    /// other source — before `rights_sales` joined the scan this was the one
    /// default-1 site flagged nowhere (code review 2026-08-25). The row rests
    /// on its stored `fx_rate` (the write path requires one, stated or
    /// resolved, for a non-nil foreign sale).
    #[tokio::test]
    async fn db_a_rights_sale_in_a_missing_month_is_flagged() {
        let pool = test_pool().await;
        usd_listing(&pool, 1, "RTU").await;
        test_support::buy(1, 1)
            .date(ymd(2024, 1, 16))
            .currency("USD")
            .fx_rate(dec("0.60"))
            .insert(&pool)
            .await;
        corporate_action::db_upsert(
            &pool,
            &crate::entities::corporate_action::CorporateAction {
                id: 10,
                listing_id: 1,
                date: ymd(2024, 7, 1),
                kind: crate::entities::corporate_action::ActionKind::RightsIssue {
                    rights_units: Decimal::ONE,
                    rights_held_units: Decimal::ONE,
                    exercise_price: dec("1.80"),
                    currency: "USD".to_string(),
                    renounceable: true,
                },
            },
        )
        .await
        .unwrap();
        crate::entities::rights_sale::db_sell_rights(
            &pool,
            10,
            &crate::entities::rights_sale::SellRightsBody {
                date: ymd(2024, 7, 20),
                units: dec("50"),
                proceeds_per_right: Some(dec("0.20")),
                rights_cost: None,
                fx_rate: Some(dec("0.99")),
                holding_account_id: 1,
                allocations: vec![crate::entities::rights_sale::AllocationInput {
                    purchase_trade_id: 1,
                    units: dec("50"),
                }],
            },
        )
        .await
        .unwrap();

        let alerts = db_fx_coverage(&pool).await.unwrap();
        let sale_rows: Vec<_> = of_kind(&alerts, "missing_rate")
            .into_iter()
            .filter(|a| a.record == "rights sale")
            .collect();
        assert_eq!(sale_rows.len(), 1, "{alerts:#?}");
        let row = sale_rows[0];
        assert_eq!(row.month, "2024-07");
        assert_eq!(row.currency, "USD");
        assert_eq!(row.ticker.as_deref(), Some("RTU"));
        assert_eq!(row.resting_on.as_deref(), Some("record_fx_rate"));
        assert!(row.detail.contains("0.99"), "{}", row.detail);
    }

    #[tokio::test]
    async fn api_report_is_served() {
        let pool = test_pool().await;
        usd_listing(&pool, 1, "AAPL").await;
        test_support::buy(1, 1)
            .date(ymd(2024, 2, 12))
            .insert(&pool)
            .await;
        let alerts: Vec<FxCoverageAlert> = ApiClient::over(router().with_state(pool.clone()))
            .get("/reports/fx_coverage")
            .await
            .expect_status(StatusCode::OK)
            .json();
        assert_eq!(alerts.len(), 1);
        assert_eq!(alerts[0].kind, "missing_rate");
        assert_eq!(alerts[0].ticker.as_deref(), Some("AAPL"));
    }
}
