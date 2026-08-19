use crate::infra::decimal::Money;
use crate::infra::http::{self, ApiError, CrudEntity};
use axum::{
    Json, Router,
    extract::{Path, State},
    http::StatusCode,
    routing::get,
};
use chrono::{Datelike, NaiveDate};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct AmmaStatement {
    pub id: i64,
    pub listing_id: i64,
    /// End of the Australian financial year the statement attributes — always a
    /// 30 June date, enforced at write time. Every AMMA-keyed report buckets the
    /// statement into the FY identified by this date's calendar year (the
    /// `domain::tax_year` convention), so any other date would land the
    /// statement in the wrong year silently.
    pub tax_year_end_date: NaiveDate,
    #[sqlx(try_from = "Money")]
    pub units_held: Decimal,
    pub date_received: NaiveDate,
    #[sqlx(try_from = "Money")]
    pub australian_interest: Decimal,
    #[sqlx(try_from = "Money")]
    pub australian_dividends_unfranked: Decimal,
    #[sqlx(try_from = "Money")]
    pub franked_dividends: Decimal,
    #[sqlx(try_from = "Money")]
    pub franking_credits: Decimal,
    #[sqlx(try_from = "Money")]
    pub net_rent: Decimal,
    #[sqlx(try_from = "Money")]
    pub foreign_income: Decimal,
    #[sqlx(try_from = "Money")]
    pub foreign_tax_credits: Decimal,
    #[sqlx(try_from = "Money")]
    pub other_income: Decimal,
    #[sqlx(try_from = "Money")]
    pub cgt_discount_gains: Decimal,
    #[sqlx(try_from = "Money")]
    pub cgt_indexation_gains: Decimal,
    #[sqlx(try_from = "Money")]
    pub cgt_other_gains: Decimal,
    #[sqlx(try_from = "Money")]
    pub capital_losses_applied: Decimal,
    /// Informational only. Tax-deferred amounts are a reported AMMA statement line, but
    /// they do NOT directly drive the member's cost base adjustment — the ATO's annual
    /// AMIT cost base net amount (`cost_base_adjustment` below) already reflects them.
    /// See `docs/ato/amit-cost-base-adjustments.md`. Not consumed by any calculation.
    #[sqlx(try_from = "Money")]
    pub tax_deferred_amount: Decimal,
    /// Informational only. As with `tax_deferred_amount`, tax-free amounts are reported
    /// on the statement but are not a direct cost-base driver; they are broadly reflected
    /// in `cost_base_adjustment`. See `docs/ato/amit-cost-base-adjustments.md`.
    #[sqlx(try_from = "Money")]
    pub tax_free_amount: Decimal,
    /// The AMIT cost base net amount **per unit** for the year — the sole driver of the
    /// cost base adjustment applied to affected parcels (see
    /// `amit_adjustment::db_cost_base_reductions`). A positive value reduces the cost base;
    /// a negative value increases it (upward adjustments are permitted under the AMIT
    /// regime). See `docs/ato/amit-cost-base-adjustments.md`.
    #[sqlx(try_from = "Money")]
    pub cost_base_adjustment: Decimal,
    #[sqlx(try_from = "Money")]
    pub tfn_withholding_tax: Decimal,
    /// ISO 4217 currency the attributed amounts are denominated in. The tax summary
    /// converts non-AUD amounts to AUD via the ATO rate for this currency and the
    /// month of `tax_year_end_date` (see `infra::fx::to_aud`). Defaults to AUD.
    pub currency: String,
    /// The holding account the statement covers (a registry issues one AMMA
    /// statement per holder account; see `entities::holding_account`).
    /// Defaults to the seeded default account when omitted from a request.
    pub holding_account_id: i64,
}

#[derive(Debug, Deserialize)]
pub struct AmmaStatementBody {
    pub listing_id: i64,
    pub tax_year_end_date: NaiveDate,
    #[serde(default)]
    pub units_held: Decimal,
    pub date_received: NaiveDate,
    #[serde(default)]
    pub australian_interest: Decimal,
    #[serde(default)]
    pub australian_dividends_unfranked: Decimal,
    #[serde(default)]
    pub franked_dividends: Decimal,
    #[serde(default)]
    pub franking_credits: Decimal,
    #[serde(default)]
    pub net_rent: Decimal,
    #[serde(default)]
    pub foreign_income: Decimal,
    #[serde(default)]
    pub foreign_tax_credits: Decimal,
    #[serde(default)]
    pub other_income: Decimal,
    #[serde(default)]
    pub cgt_discount_gains: Decimal,
    #[serde(default)]
    pub cgt_indexation_gains: Decimal,
    #[serde(default)]
    pub cgt_other_gains: Decimal,
    #[serde(default)]
    pub capital_losses_applied: Decimal,
    #[serde(default)]
    pub tax_deferred_amount: Decimal,
    #[serde(default)]
    pub tax_free_amount: Decimal,
    #[serde(default)]
    pub cost_base_adjustment: Decimal,
    #[serde(default)]
    pub tfn_withholding_tax: Decimal,
    #[serde(default = "default_currency")]
    pub currency: String,
    /// Defaults to the seeded default holding account when omitted.
    #[serde(default = "crate::entities::holding_account::default_holding_account_id")]
    pub holding_account_id: i64,
}

fn default_currency() -> String {
    "AUD".to_string()
}

#[derive(thiserror::Error, Debug)]
pub enum UpsertError {
    /// `tax_year_end_date` is not a 30 June date (carries the rejected date).
    /// An AMMA statement attributes a full Australian financial year, and every
    /// AMMA-keyed report buckets it into the FY named by this date's calendar
    /// year — a mid-year date would silently land in the wrong FY. Mapped to `422`.
    #[error("tax_year_end_date {0} is not a 30 June date")]
    NotFinancialYearEnd(NaiveDate),
    /// The statement's currency is not its listing's. An AMMA attributes the
    /// income and capital gains of *that listed trust*, and the tax summary
    /// converts every component at this currency's rate for the month of
    /// `tax_year_end_date` — so a statement in another currency reports a
    /// fund's year in money the fund does not pay. The rule
    /// [`ess_statement`](crate::entities::ess_statement) applies to a
    /// statement's per-share market value and
    /// [`trade`](crate::entities::trade) to a parcel's price. Mapped to `422`.
    #[error("the AMMA statement is in {statement} but its listing is quoted in {listing}")]
    CurrencyNotListings { statement: String, listing: String },
    #[error("AMMA statement write failed: {0}")]
    Db(#[from] sqlx::Error),
}

impl From<UpsertError> for ApiError {
    fn from(err: UpsertError) -> Self {
        match err {
            UpsertError::NotFinancialYearEnd(date) => ApiError::unprocessable(format!(
                "tax_year_end_date {date} is not a 30 June date — an AMMA statement \
                 covers the Australian financial year ending 30 June, and reports \
                 attribute it to the year of that date"
            )),
            UpsertError::CurrencyNotListings { statement, listing } => {
                ApiError::unprocessable(format!(
                    "this AMMA statement is recorded in {statement} but its listing is quoted in \
                     {listing} — the statement attributes that fund's own year, so enter it in \
                     {listing} (a statement in another currency is converted before entry, or the \
                     wrong listing was picked)"
                ))
            }
            UpsertError::Db(err) => err.into(),
        }
    }
}

impl CrudEntity for AmmaStatement {
    type Key = i64;
    const TABLE: &'static str = "amma_statements";
    const COLUMNS: &'static str = "id, listing_id, tax_year_end_date, units_held, date_received, \
         australian_interest, australian_dividends_unfranked, franked_dividends, \
         franking_credits, net_rent, foreign_income, foreign_tax_credits, other_income, \
         cgt_discount_gains, cgt_indexation_gains, cgt_other_gains, capital_losses_applied, \
         tax_deferred_amount, tax_free_amount, cost_base_adjustment, tfn_withholding_tax, \
         currency, holding_account_id";
    const ORDER_BY: &'static str = "tax_year_end_date, id";
    const NOUN: &'static str = "AMMA statement";
}

pub fn router() -> Router<SqlitePool> {
    Router::new()
        .route("/amma_statements", get(http::list_handler::<AmmaStatement>))
        .route(
            "/amma_statements/{id}",
            get(http::get_handler::<AmmaStatement>)
                .put(upsert)
                // Deleting a statement still referenced by AMIT adjustments
                // violates an FK → 422.
                .delete(http::delete_handler::<AmmaStatement>),
        )
}

#[cfg(test)]
pub async fn db_list(pool: &SqlitePool) -> Result<Vec<AmmaStatement>, sqlx::Error> {
    http::crud_list(pool).await
}

#[cfg(test)]
pub async fn db_get(pool: &SqlitePool, id: i64) -> Result<Option<AmmaStatement>, sqlx::Error> {
    http::crud_get(pool, id).await
}

pub async fn db_upsert(pool: &SqlitePool, stmt: &AmmaStatement) -> Result<(), UpsertError> {
    // The FY-end date must actually be a financial-year end: reports bucket the
    // statement by this date's calendar year, which matches domain::tax_year's
    // rule only for January–June dates — in practice, 30 June.
    if (stmt.tax_year_end_date.month(), stmt.tax_year_end_date.day()) != (6, 30) {
        return Err(UpsertError::NotFinancialYearEnd(stmt.tax_year_end_date));
    }
    let mut tx = pool.begin().await?;
    sqlx::query(
        "INSERT INTO amma_statements \
         (id, listing_id, tax_year_end_date, units_held, date_received, \
          australian_interest, australian_dividends_unfranked, franked_dividends, \
          franking_credits, net_rent, foreign_income, foreign_tax_credits, other_income, \
          cgt_discount_gains, cgt_indexation_gains, cgt_other_gains, capital_losses_applied, \
          tax_deferred_amount, tax_free_amount, cost_base_adjustment, tfn_withholding_tax, \
          currency, holding_account_id) \
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?) \
         ON CONFLICT(id) DO UPDATE SET \
             listing_id                      = excluded.listing_id, \
             tax_year_end_date               = excluded.tax_year_end_date, \
             units_held                      = excluded.units_held, \
             date_received                   = excluded.date_received, \
             australian_interest             = excluded.australian_interest, \
             australian_dividends_unfranked  = excluded.australian_dividends_unfranked, \
             franked_dividends               = excluded.franked_dividends, \
             franking_credits                = excluded.franking_credits, \
             net_rent                        = excluded.net_rent, \
             foreign_income                  = excluded.foreign_income, \
             foreign_tax_credits             = excluded.foreign_tax_credits, \
             other_income                    = excluded.other_income, \
             cgt_discount_gains              = excluded.cgt_discount_gains, \
             cgt_indexation_gains            = excluded.cgt_indexation_gains, \
             cgt_other_gains                 = excluded.cgt_other_gains, \
             capital_losses_applied          = excluded.capital_losses_applied, \
             tax_deferred_amount             = excluded.tax_deferred_amount, \
             tax_free_amount                 = excluded.tax_free_amount, \
             cost_base_adjustment            = excluded.cost_base_adjustment, \
             tfn_withholding_tax             = excluded.tfn_withholding_tax, \
             currency                        = excluded.currency, \
             holding_account_id              = excluded.holding_account_id",
    )
    .bind(stmt.id)
    .bind(stmt.listing_id)
    .bind(stmt.tax_year_end_date)
    .bind(Money(stmt.units_held))
    .bind(stmt.date_received)
    .bind(Money(stmt.australian_interest))
    .bind(Money(stmt.australian_dividends_unfranked))
    .bind(Money(stmt.franked_dividends))
    .bind(Money(stmt.franking_credits))
    .bind(Money(stmt.net_rent))
    .bind(Money(stmt.foreign_income))
    .bind(Money(stmt.foreign_tax_credits))
    .bind(Money(stmt.other_income))
    .bind(Money(stmt.cgt_discount_gains))
    .bind(Money(stmt.cgt_indexation_gains))
    .bind(Money(stmt.cgt_other_gains))
    .bind(Money(stmt.capital_losses_applied))
    .bind(Money(stmt.tax_deferred_amount))
    .bind(Money(stmt.tax_free_amount))
    .bind(Money(stmt.cost_base_adjustment))
    .bind(Money(stmt.tfn_withholding_tax))
    .bind(&stmt.currency)
    .bind(stmt.holding_account_id)
    .execute(&mut *tx)
    .await?;

    // The statement's currency must be the listing's, as a trade's must
    // (SCENARIOS M-08). Checked after the write, like the trade path's twin,
    // so an unrecognised currency code meets its own foreign-key rejection
    // first.
    if let Some(listing) =
        crate::entities::trade::listing_currency_mismatch(&mut tx, stmt.listing_id, &stmt.currency)
            .await?
    {
        return Err(UpsertError::CurrencyNotListings {
            statement: stmt.currency.clone(),
            listing,
        });
    }

    tx.commit().await?;
    Ok(())
}

async fn upsert(
    State(pool): State<SqlitePool>,
    Path(id): Path<i64>,
    Json(body): Json<AmmaStatementBody>,
) -> Result<StatusCode, ApiError> {
    let stmt = AmmaStatement {
        id,
        listing_id: body.listing_id,
        tax_year_end_date: body.tax_year_end_date,
        units_held: body.units_held,
        date_received: body.date_received,
        australian_interest: body.australian_interest,
        australian_dividends_unfranked: body.australian_dividends_unfranked,
        franked_dividends: body.franked_dividends,
        franking_credits: body.franking_credits,
        net_rent: body.net_rent,
        foreign_income: body.foreign_income,
        foreign_tax_credits: body.foreign_tax_credits,
        other_income: body.other_income,
        cgt_discount_gains: body.cgt_discount_gains,
        cgt_indexation_gains: body.cgt_indexation_gains,
        cgt_other_gains: body.cgt_other_gains,
        capital_losses_applied: body.capital_losses_applied,
        tax_deferred_amount: body.tax_deferred_amount,
        tax_free_amount: body.tax_free_amount,
        cost_base_adjustment: body.cost_base_adjustment,
        tfn_withholding_tax: body.tfn_withholding_tax,
        currency: body.currency,
        holding_account_id: body.holding_account_id,
    };
    db_upsert(&pool, &stmt)
        .await
        .map(|_| StatusCode::NO_CONTENT)
        .map_err(ApiError::from)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{self, ApiClient, dec, test_pool};

    /// Client over this module's own routes.
    fn client(pool: &SqlitePool) -> ApiClient {
        ApiClient::over(router().with_state(pool.clone()))
    }

    async fn insert_test_listing(pool: &SqlitePool) {
        test_support::listing(1)
            .ticker("VAF")
            .name("Vanguard Australian Fixed Interest ETF")
            .amit(true)
            .insert(pool)
            .await;
    }

    fn sample_amma() -> AmmaStatement {
        test_support::amma(1, 1)
            .units(dec("1000"))
            .cost_base_adjustment(dec("0.0023"))
            .with(|a| {
                a.australian_interest = dec("12.50");
                a.australian_dividends_unfranked = dec("5.25");
                a.tax_deferred_amount = dec("2.30");
                a.tax_free_amount = dec("1.10");
            })
            .build()
    }

    // DB-level tests

    #[tokio::test]
    async fn db_insert_and_retrieve() {
        let pool = test_pool().await;
        insert_test_listing(&pool).await;
        db_upsert(&pool, &sample_amma()).await.unwrap();
        let got = db_get(&pool, 1).await.unwrap().unwrap();
        assert_eq!(got.listing_id, 1);
        assert_eq!(
            got.tax_year_end_date,
            NaiveDate::from_ymd_opt(2024, 6, 30).unwrap()
        );
        assert_eq!(got.units_held, "1000".parse::<Decimal>().unwrap());
        assert_eq!(got.australian_interest, "12.50".parse::<Decimal>().unwrap());
        assert_eq!(
            got.australian_dividends_unfranked,
            "5.25".parse::<Decimal>().unwrap()
        );
        assert_eq!(got.tax_deferred_amount, "2.30".parse::<Decimal>().unwrap());
        assert_eq!(got.tax_free_amount, "1.10".parse::<Decimal>().unwrap());
        assert_eq!(
            got.cost_base_adjustment,
            "0.0023".parse::<Decimal>().unwrap()
        );
    }

    #[tokio::test]
    async fn db_cost_base_adjustment_calculation() {
        let pool = test_pool().await;
        insert_test_listing(&pool).await;
        db_upsert(&pool, &sample_amma()).await.unwrap();
        let got = db_get(&pool, 1).await.unwrap().unwrap();
        // total cost base reduction = cost_base_adjustment per unit * units_held
        let total_adjustment = got.cost_base_adjustment * got.units_held;
        assert_eq!(total_adjustment, "2.3".parse::<Decimal>().unwrap());
    }

    #[tokio::test]
    async fn db_get_missing_returns_none() {
        let pool = test_pool().await;
        assert!(db_get(&pool, 999).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn db_upsert_updates_existing() {
        let pool = test_pool().await;
        insert_test_listing(&pool).await;
        db_upsert(&pool, &sample_amma()).await.unwrap();
        let mut updated = sample_amma();
        updated.australian_interest = "99.99".parse().unwrap();
        db_upsert(&pool, &updated).await.unwrap();
        let got = db_get(&pool, 1).await.unwrap().unwrap();
        assert_eq!(got.australian_interest, "99.99".parse::<Decimal>().unwrap());
    }

    // API-level tests

    #[tokio::test]
    async fn api_upsert_and_get() {
        let pool = test_pool().await;
        insert_test_listing(&pool).await;
        let body = serde_json::json!({
            "listing_id": 1,
            "tax_year_end_date": "2024-06-30",
            "units_held": "1000",
            "date_received": "2024-08-15",
            "australian_interest": "12.50",
            "tax_deferred_amount": "2.30",
            "cost_base_adjustment": "0.0023"
        });
        let resp = client(&pool).put("/amma_statements/1", &body).await;
        assert_eq!(resp.status, StatusCode::NO_CONTENT);
        let got = db_get(&pool, 1).await.unwrap().unwrap();
        assert_eq!(got.australian_interest, "12.50".parse::<Decimal>().unwrap());
        assert_eq!(
            got.cost_base_adjustment,
            "0.0023".parse::<Decimal>().unwrap()
        );
    }

    #[tokio::test]
    async fn api_list_returns_ok() {
        let pool = test_pool().await;
        insert_test_listing(&pool).await;
        db_upsert(&pool, &sample_amma()).await.unwrap();
        let resp = client(&pool).get("/amma_statements").await;
        assert_eq!(resp.status, StatusCode::OK);
        let items: Vec<AmmaStatement> = resp.json();
        assert_eq!(items.len(), 1);
    }

    #[tokio::test]
    async fn api_get_missing_returns_404() {
        let pool = test_pool().await;
        let resp = client(&pool).get("/amma_statements/999").await;
        assert_eq!(resp.status, StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn api_delete_existing_returns_no_content() {
        let pool = test_pool().await;
        insert_test_listing(&pool).await;
        db_upsert(&pool, &sample_amma()).await.unwrap();
        let resp = client(&pool).delete("/amma_statements/1").await;
        assert_eq!(resp.status, StatusCode::NO_CONTENT);
    }

    #[tokio::test]
    async fn api_delete_missing_returns_404() {
        let pool = test_pool().await;
        let resp = client(&pool).delete("/amma_statements/999").await;
        assert_eq!(resp.status, StatusCode::NOT_FOUND);
    }

    /// `tax_year_end_date` must be a 30 June FY end: reports bucket the statement
    /// by that date's calendar year, so e.g. 2024-12-31 would silently land in
    /// FY2024 while `domain::tax_year::tax_year_for` puts December in FY2025
    /// (2026-07-12 review: the 30 June assumption was never validated).
    #[tokio::test]
    async fn api_non_june_30_year_end_returns_422() {
        let pool = test_pool().await;
        insert_test_listing(&pool).await;
        for date in ["2024-12-31", "2024-06-29", "2024-07-01"] {
            let body = serde_json::json!({
                "listing_id": 1,
                "tax_year_end_date": date,
                "date_received": "2024-08-15"
            });
            let resp = client(&pool).put("/amma_statements/1", &body).await;
            assert_eq!(
                resp.status,
                StatusCode::UNPROCESSABLE_ENTITY,
                "{date} must be rejected"
            );
            let detail = resp.text().to_string();
            assert!(
                detail.contains(date) && detail.contains("30 June"),
                "{date}: detail must carry the date and the rule, got: {detail}"
            );
            assert!(
                db_get(&pool, 1).await.unwrap().is_none(),
                "{date}: nothing persisted"
            );
        }
    }

    /// 30 June is accepted for any year — the rule pins the day, not the year.
    #[tokio::test]
    async fn db_june_30_of_any_year_accepted() {
        let pool = test_pool().await;
        insert_test_listing(&pool).await;
        for (id, year) in [(1, 2019), (2, 2025)] {
            let stmt = test_support::amma(id, 1)
                .with(|a| a.tax_year_end_date = NaiveDate::from_ymd_opt(year, 6, 30).unwrap())
                .build();
            db_upsert(&pool, &stmt).await.unwrap();
        }
        assert_eq!(db_list(&pool).await.unwrap().len(), 2);
    }

    #[tokio::test]
    async fn api_decimal_precision_round_trip() {
        let pool = test_pool().await;
        insert_test_listing(&pool).await;
        let body = serde_json::json!({
            "listing_id": 1,
            "tax_year_end_date": "2024-06-30",
            "units_held": "1234.567890123",
            "date_received": "2024-08-15",
            "australian_interest": "9.876543210",
            "cost_base_adjustment": "0.001234567890"
        });
        let resp = client(&pool).put("/amma_statements/1", &body).await;
        assert_eq!(resp.status, StatusCode::NO_CONTENT);
        let resp = client(&pool).get("/amma_statements/1").await;
        let got: AmmaStatement = resp.json();
        assert_eq!(got.units_held, "1234.567890123".parse::<Decimal>().unwrap());
        assert_eq!(
            got.australian_interest,
            "9.876543210".parse::<Decimal>().unwrap()
        );
        assert_eq!(
            got.cost_base_adjustment,
            "0.001234567890".parse::<Decimal>().unwrap()
        );
    }

    /// An AMMA statement is recorded in its listing's own currency (SCENARIOS
    /// M-08): it attributes *that fund's* year, and the tax summary converts
    /// every component at this currency's rate for the month of
    /// `tax_year_end_date` — so a statement in another currency reports the
    /// fund's year in money the fund does not pay. The rule the trade path
    /// applies to a parcel's price, and the ESS path to a per-share value.
    #[tokio::test]
    async fn api_amma_currency_must_be_the_listings() {
        let pool = test_pool().await;
        test_support::listing(1)
            .mic("XNYS")
            .ticker("VTS")
            .name("VTS")
            .currency("USD")
            .amit(true)
            .insert(&pool)
            .await;
        let body = |currency: &str| {
            serde_json::json!({
                "listing_id": 1,
                "tax_year_end_date": "2024-06-30",
                "units_held": "100",
                "date_received": "2024-08-15",
                "currency": currency,
            })
        };
        let resp = client(&pool).put("/amma_statements/1", &body("AUD")).await;
        let (status, detail) = resp.status_and_body();
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
        assert!(
            detail.contains("recorded in AUD") && detail.contains("quoted in USD"),
            "the refusal names both currencies: {detail}"
        );
        assert!(
            db_get(&pool, 1).await.unwrap().is_none(),
            "nothing persisted"
        );

        // In the listing's own currency it goes through.
        client(&pool)
            .put("/amma_statements/1", &body("USD"))
            .await
            .expect_status(StatusCode::NO_CONTENT);
    }
}
