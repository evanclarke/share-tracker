use axum::{
    Json, Router,
    extract::{Path, State},
    http::StatusCode,
    routing::get,
};
use chrono::NaiveDate;
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use sqlx::{Row, SqlitePool};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Income {
    pub id: i64,
    pub listing_id: i64,
    pub date_paid: NaiveDate,
    pub ex_date: Option<NaiveDate>,
    pub franked_amount: Decimal,
    pub unfranked_amount: Decimal,
    pub foreign_source_income: Decimal,
    pub foreign_tax_paid: Decimal,
    pub tfn_withholding_tax: Decimal,
    pub franking_credits: Decimal,
    pub lic_capital_gain_deduction: Decimal,
    pub conduit_foreign_income: Decimal,
    pub trust_income: bool,
    pub reinvestment_trade_id: Option<i64>,
    /// ISO 4217 currency the amounts are denominated in. The tax summary converts
    /// non-AUD amounts to AUD via the ATO rate for this currency and the month of
    /// `date_paid` (see `infra::fx::to_aud`). Defaults to AUD.
    pub currency: String,
    /// Provenance link from a buy-back dividend-component row to the buy-back
    /// Sell trade it was created with (`None` for every other row). Set only
    /// by `POST /corporate_actions/:id/participate`
    /// (`entities::buyback_participation`). A row carrying it is managed by
    /// the participation: `PUT`/`DELETE /income` reject it (`422`), and it is
    /// removed together with the Sell by `DELETE /sells/:id`.
    pub buyback_trade_id: Option<i64>,
    /// The holding account the distribution was paid to (see
    /// `entities::holding_account`): decides whose DRP enrolment applies and
    /// which account a reinvestment trade lands in. Defaults to the seeded
    /// default account when omitted from a request.
    pub holding_account_id: i64,
    /// Optional per-share figure from the registry statement, supplied only
    /// together with `securities_held`: their product, cent-rounded, must
    /// equal the gross cash components (see `check_per_share`). Informational
    /// / validation-only — no report uses it (mirrors
    /// `trades.statement_total`).
    pub amount_per_security: Option<Decimal>,
    /// See `amount_per_security` — the statement's securities-held count.
    pub securities_held: Option<Decimal>,
}

impl<'r> sqlx::FromRow<'r, sqlx::sqlite::SqliteRow> for Income {
    fn from_row(row: &'r sqlx::sqlite::SqliteRow) -> Result<Self, sqlx::Error> {
        fn dec(s: String) -> Result<Decimal, sqlx::Error> {
            s.parse().map_err(|e: rust_decimal::Error| sqlx::Error::Decode(Box::new(e)))
        }
        fn opt_dec(s: Option<String>) -> Result<Option<Decimal>, sqlx::Error> {
            s.map(dec).transpose()
        }
        Ok(Income {
            id: row.try_get("id")?,
            listing_id: row.try_get("listing_id")?,
            date_paid: row.try_get("date_paid")?,
            ex_date: row.try_get("ex_date")?,
            franked_amount: dec(row.try_get("franked_amount")?)?,
            unfranked_amount: dec(row.try_get("unfranked_amount")?)?,
            foreign_source_income: dec(row.try_get("foreign_source_income")?)?,
            foreign_tax_paid: dec(row.try_get("foreign_tax_paid")?)?,
            tfn_withholding_tax: dec(row.try_get("tfn_withholding_tax")?)?,
            franking_credits: dec(row.try_get("franking_credits")?)?,
            lic_capital_gain_deduction: dec(row.try_get("lic_capital_gain_deduction")?)?,
            conduit_foreign_income: dec(row.try_get("conduit_foreign_income")?)?,
            trust_income: row.try_get("trust_income")?,
            reinvestment_trade_id: row.try_get("reinvestment_trade_id")?,
            currency: row.try_get("currency")?,
            buyback_trade_id: row.try_get("buyback_trade_id")?,
            holding_account_id: row.try_get("holding_account_id")?,
            amount_per_security: opt_dec(row.try_get("amount_per_security")?)?,
            securities_held: opt_dec(row.try_get("securities_held")?)?,
        })
    }
}

#[derive(Debug, Deserialize)]
pub struct IncomeBody {
    pub listing_id: i64,
    pub date_paid: NaiveDate,
    #[serde(default)]
    pub ex_date: Option<NaiveDate>,
    #[serde(default)]
    pub franked_amount: Decimal,
    #[serde(default)]
    pub unfranked_amount: Decimal,
    #[serde(default)]
    pub foreign_source_income: Decimal,
    #[serde(default)]
    pub foreign_tax_paid: Decimal,
    #[serde(default)]
    pub tfn_withholding_tax: Decimal,
    #[serde(default)]
    pub franking_credits: Decimal,
    #[serde(default)]
    pub lic_capital_gain_deduction: Decimal,
    #[serde(default)]
    pub conduit_foreign_income: Decimal,
    #[serde(default)]
    pub trust_income: bool,
    #[serde(default)]
    pub reinvestment_trade_id: Option<i64>,
    #[serde(default = "default_currency")]
    pub currency: String,
    /// Defaults to the seeded default holding account when omitted.
    #[serde(default = "crate::entities::holding_account::default_holding_account_id")]
    pub holding_account_id: i64,
    /// Optional statement cross-check; see `Income::amount_per_security`.
    #[serde(default)]
    pub amount_per_security: Option<Decimal>,
    #[serde(default)]
    pub securities_held: Option<Decimal>,
}

fn default_currency() -> String {
    "AUD".to_string()
}

pub fn router() -> Router<SqlitePool> {
    Router::new()
        .route("/income", get(list))
        .route("/income/{id}", get(get_one).put(upsert).delete(delete))
}

pub async fn db_list(pool: &SqlitePool) -> Result<Vec<Income>, sqlx::Error> {
    sqlx::query_as(
        "SELECT id, listing_id, date_paid, ex_date, franked_amount, unfranked_amount, \
         foreign_source_income, foreign_tax_paid, tfn_withholding_tax, franking_credits, \
         lic_capital_gain_deduction, conduit_foreign_income, trust_income, reinvestment_trade_id, \
         currency, buyback_trade_id, holding_account_id, amount_per_security, securities_held \
         FROM income ORDER BY date_paid, id",
    )
    .fetch_all(pool)
    .await
}

pub async fn db_get(pool: &SqlitePool, id: i64) -> Result<Option<Income>, sqlx::Error> {
    sqlx::query_as(
        "SELECT id, listing_id, date_paid, ex_date, franked_amount, unfranked_amount, \
         foreign_source_income, foreign_tax_paid, tfn_withholding_tax, franking_credits, \
         lic_capital_gain_deduction, conduit_foreign_income, trust_income, reinvestment_trade_id, \
         currency, buyback_trade_id, holding_account_id, amount_per_security, securities_held \
         FROM income WHERE id = ?",
    )
    .bind(id)
    .fetch_optional(pool)
    .await
}

#[derive(Debug)]
pub enum UpsertError {
    Db(sqlx::Error),
    /// The existing row is a buy-back dividend component (`buyback_trade_id`
    /// set): its figures derive from the buy-back's terms, so free-form edits
    /// are rejected. Delete the buy-back Sell via `DELETE /sells/:id` (which
    /// removes this row too) and re-participate instead. Mapped to `422`.
    BuyBackIncome,
    /// The supplied per-share figures failed the cross-check. Mapped to `422`.
    PerShare(PerShareError),
}

impl From<sqlx::Error> for UpsertError {
    fn from(e: sqlx::Error) -> Self {
        UpsertError::Db(e)
    }
}

/// Why the supplied per-share figures failed to reconcile (both map to 422).
#[derive(Debug, PartialEq)]
pub enum PerShareError {
    /// Exactly one of `amount_per_security` / `securities_held` was supplied —
    /// the cross-check needs both (or neither).
    SuppliedAlone,
    /// amount_per_security × securities_held, cent-rounded, does not equal
    /// the gross cash components (carried so the rejection can say what the
    /// statement figures actually multiply to).
    ProductMismatch { product: Decimal },
}

/// Cross-check the optional per-share statement figures against the entered
/// amounts: amount_per_security × securities_held, rounded to the cent (half
/// away from zero, matching statements), must equal the gross cash components
/// `franked + unfranked + foreign_source_income` — franking credits are
/// notional and TFN withholding is deducted from (not part of) the gross.
/// Comparison is numeric (`Decimal` equality ignores trailing zeros). Neither
/// supplied means the figures weren't recorded — nothing to check.
fn check_per_share(income: &Income) -> Result<(), PerShareError> {
    let (aps, held) = match (income.amount_per_security, income.securities_held) {
        (None, None) => return Ok(()),
        (Some(aps), Some(held)) => (aps, held),
        _ => return Err(PerShareError::SuppliedAlone),
    };
    let product = (aps * held)
        .round_dp_with_strategy(2, rust_decimal::RoundingStrategy::MidpointAwayFromZero);
    let gross = income.franked_amount + income.unfranked_amount + income.foreign_source_income;
    if product != gross {
        return Err(PerShareError::ProductMismatch { product });
    }
    Ok(())
}

/// Human-readable body for a per-share 422 (shown by the web UI).
pub(crate) fn per_share_detail(e: &PerShareError) -> String {
    match e {
        PerShareError::SuppliedAlone => {
            "amount_per_security and securities_held must be supplied together \
             — provide both or neither"
                .to_string()
        }
        PerShareError::ProductMismatch { product } => {
            format!(
                "per-share figures do not reconcile: amount_per_security × \
                 securities_held computes to {product}, which must equal \
                 franked + unfranked + foreign source income"
            )
        }
    }
}

pub async fn db_upsert(pool: &SqlitePool, income: &Income) -> Result<(), UpsertError> {
    check_per_share(income).map_err(UpsertError::PerShare)?;

    let mut tx = pool.begin().await?;

    // A buy-back dividend-component row is immutable here: it was created
    // from its action's terms by the participation operation. (The INSERT
    // below never sets buyback_trade_id, so a normal row can't become one
    // either.)
    let existing_buyback: Option<Option<i64>> =
        sqlx::query_scalar("SELECT buyback_trade_id FROM income WHERE id = ?")
            .bind(income.id)
            .fetch_optional(&mut *tx)
            .await?;
    if existing_buyback.flatten().is_some() {
        return Err(UpsertError::BuyBackIncome);
    }

    sqlx::query(
        "INSERT INTO income \
         (id, listing_id, date_paid, ex_date, franked_amount, unfranked_amount, \
          foreign_source_income, foreign_tax_paid, tfn_withholding_tax, franking_credits, \
          lic_capital_gain_deduction, conduit_foreign_income, trust_income, reinvestment_trade_id, \
          currency, holding_account_id, amount_per_security, securities_held) \
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?) \
         ON CONFLICT(id) DO UPDATE SET \
             listing_id                 = excluded.listing_id, \
             date_paid                  = excluded.date_paid, \
             ex_date                    = excluded.ex_date, \
             franked_amount             = excluded.franked_amount, \
             unfranked_amount           = excluded.unfranked_amount, \
             foreign_source_income      = excluded.foreign_source_income, \
             foreign_tax_paid           = excluded.foreign_tax_paid, \
             tfn_withholding_tax        = excluded.tfn_withholding_tax, \
             franking_credits           = excluded.franking_credits, \
             lic_capital_gain_deduction = excluded.lic_capital_gain_deduction, \
             conduit_foreign_income     = excluded.conduit_foreign_income, \
             trust_income               = excluded.trust_income, \
             reinvestment_trade_id      = excluded.reinvestment_trade_id, \
             currency                   = excluded.currency, \
             holding_account_id         = excluded.holding_account_id, \
             amount_per_security        = excluded.amount_per_security, \
             securities_held            = excluded.securities_held",
    )
    .bind(income.id)
    .bind(income.listing_id)
    .bind(income.date_paid)
    .bind(income.ex_date)
    .bind(income.franked_amount.to_string())
    .bind(income.unfranked_amount.to_string())
    .bind(income.foreign_source_income.to_string())
    .bind(income.foreign_tax_paid.to_string())
    .bind(income.tfn_withholding_tax.to_string())
    .bind(income.franking_credits.to_string())
    .bind(income.lic_capital_gain_deduction.to_string())
    .bind(income.conduit_foreign_income.to_string())
    .bind(income.trust_income)
    .bind(income.reinvestment_trade_id)
    .bind(&income.currency)
    .bind(income.holding_account_id)
    .bind(income.amount_per_security.map(|d| d.to_string()))
    .bind(income.securities_held.map(|d| d.to_string()))
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(())
}

/// Outcome of a delete request, so the handler can map to the right status.
#[derive(Debug, PartialEq)]
pub enum DeleteOutcome {
    Deleted,
    NotFound,
    /// The row is a buy-back dividend component (`buyback_trade_id` set) —
    /// deleting it alone would leave the buy-back Sell without its dividend
    /// side. Delete the Sell via `DELETE /sells/:id` instead (which removes
    /// this row too). Mapped to `422`.
    BuyBackIncome,
}

pub async fn db_delete(pool: &SqlitePool, id: i64) -> Result<DeleteOutcome, sqlx::Error> {
    let mut tx = pool.begin().await?;

    let buyback: Option<Option<i64>> =
        sqlx::query_scalar("SELECT buyback_trade_id FROM income WHERE id = ?")
            .bind(id)
            .fetch_optional(&mut *tx)
            .await?;
    match buyback {
        None => return Ok(DeleteOutcome::NotFound),
        Some(Some(_)) => return Ok(DeleteOutcome::BuyBackIncome),
        Some(None) => {}
    }

    sqlx::query("DELETE FROM income WHERE id = ?")
        .bind(id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(DeleteOutcome::Deleted)
}

async fn list(State(pool): State<SqlitePool>) -> Result<Json<Vec<Income>>, StatusCode> {
    db_list(&pool)
        .await
        .map(Json)
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)
}

async fn get_one(
    State(pool): State<SqlitePool>,
    Path(id): Path<i64>,
) -> Result<Json<Income>, StatusCode> {
    db_get(&pool, id)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
        .map(Json)
        .ok_or(StatusCode::NOT_FOUND)
}

async fn upsert(
    State(pool): State<SqlitePool>,
    Path(id): Path<i64>,
    Json(body): Json<IncomeBody>,
) -> Result<StatusCode, (StatusCode, String)> {
    let income = Income {
        id,
        listing_id: body.listing_id,
        date_paid: body.date_paid,
        ex_date: body.ex_date,
        franked_amount: body.franked_amount,
        unfranked_amount: body.unfranked_amount,
        foreign_source_income: body.foreign_source_income,
        foreign_tax_paid: body.foreign_tax_paid,
        tfn_withholding_tax: body.tfn_withholding_tax,
        franking_credits: body.franking_credits,
        lic_capital_gain_deduction: body.lic_capital_gain_deduction,
        conduit_foreign_income: body.conduit_foreign_income,
        trust_income: body.trust_income,
        reinvestment_trade_id: body.reinvestment_trade_id,
        currency: body.currency,
        buyback_trade_id: None,
        holding_account_id: body.holding_account_id,
        amount_per_security: body.amount_per_security,
        securities_held: body.securities_held,
    };
    db_upsert(&pool, &income)
        .await
        .map(|_| StatusCode::NO_CONTENT)
        .map_err(|e| match e {
            UpsertError::Db(err) => {
                (crate::infra::http::write_error_status(&err), String::new())
            }
            // Managed by the buy-back participation → 422.
            UpsertError::BuyBackIncome => (StatusCode::UNPROCESSABLE_ENTITY, String::new()),
            // The cross-check rejection says what the statement figures
            // multiply to, so a typo is findable without a calculator.
            UpsertError::PerShare(detail) => {
                (StatusCode::UNPROCESSABLE_ENTITY, per_share_detail(&detail))
            }
        })
}

async fn delete(
    State(pool): State<SqlitePool>,
    Path(id): Path<i64>,
) -> Result<StatusCode, StatusCode> {
    match db_delete(&pool, id).await {
        Ok(DeleteOutcome::Deleted) => Ok(StatusCode::NO_CONTENT),
        Ok(DeleteOutcome::NotFound) => Err(StatusCode::NOT_FOUND),
        // Managed by the buy-back participation → 422.
        Ok(DeleteOutcome::BuyBackIncome) => Err(StatusCode::UNPROCESSABLE_ENTITY),
        Err(_) => Err(StatusCode::INTERNAL_SERVER_ERROR),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{infra::db, entities::{listing, trade}};
    use axum::{body::Body, http::Request};
    use http_body_util::BodyExt;
    use rust_decimal::Decimal;
    use tower::ServiceExt;

    async fn test_pool() -> SqlitePool {
        db::init(":memory:").await.unwrap()
    }

    async fn insert_test_listing(pool: &SqlitePool) {
        listing::db_upsert(
            pool,
            &listing::Listing {
                id: 1,
                exchange_mic: Some("XASX".to_string()),
                ticker: "VAS".to_string(),
                name: "Vanguard Australian Shares ETF".to_string(),
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

    async fn insert_test_trade(pool: &SqlitePool) -> i64 {
        let t = trade::Trade {
            brokerage_includes_gst: false,
            statement_total: None,
            holding_account_id: 1,
            transfer_id: None,
            id: 1,
            trade_type: trade::TradeType::DRP,
            date: NaiveDate::from_ymd_opt(2024, 3, 15).unwrap(),
            settlement_date: NaiveDate::from_ymd_opt(2024, 3, 15).unwrap(),
            listing_id: 1,
            average_price: Decimal::from(95),
            quantity: Decimal::from(2),
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
        };
        trade::db_upsert(pool, &t).await.unwrap();
        t.id
    }

    fn dividend_income() -> Income {
        Income {
            holding_account_id: 1,
            id: 1,
            listing_id: 1,
            date_paid: NaiveDate::from_ymd_opt(2024, 3, 15).unwrap(),
            ex_date: Some(NaiveDate::from_ymd_opt(2024, 3, 1).unwrap()),
            franked_amount: Decimal::from(70),
            unfranked_amount: Decimal::from(30),
            foreign_source_income: Decimal::ZERO,
            foreign_tax_paid: Decimal::ZERO,
            tfn_withholding_tax: Decimal::ZERO,
            franking_credits: Decimal::from(30),
            lic_capital_gain_deduction: Decimal::ZERO,
            conduit_foreign_income: Decimal::ZERO,
            trust_income: false,
            reinvestment_trade_id: None,
            currency: "AUD".to_string(),
            buyback_trade_id: None,
            amount_per_security: None,
            securities_held: None,
        }
    }

    // DB-level tests

    #[tokio::test]
    async fn db_dividend_income_insert_and_retrieve() {
        let pool = test_pool().await;
        insert_test_listing(&pool).await;
        db_upsert(&pool, &dividend_income()).await.unwrap();
        let got = db_get(&pool, 1).await.unwrap().unwrap();
        assert_eq!(got.listing_id, 1);
        assert_eq!(got.franked_amount, Decimal::from(70));
        assert_eq!(got.unfranked_amount, Decimal::from(30));
        assert_eq!(got.franking_credits, Decimal::from(30));
        assert_eq!(got.ex_date, Some(NaiveDate::from_ymd_opt(2024, 3, 1).unwrap()));
        assert!(!got.trust_income);
        assert!(got.reinvestment_trade_id.is_none());
    }

    #[tokio::test]
    async fn db_trust_distribution_insert_and_retrieve() {
        let pool = test_pool().await;
        insert_test_listing(&pool).await;
        let dist = Income {
            holding_account_id: 1,
            id: 2,
            listing_id: 1,
            date_paid: NaiveDate::from_ymd_opt(2024, 6, 30).unwrap(),
            ex_date: None,
            franked_amount: Decimal::ZERO,
            unfranked_amount: Decimal::from(50),
            foreign_source_income: Decimal::from(10),
            foreign_tax_paid: "1.5".parse().unwrap(),
            tfn_withholding_tax: Decimal::ZERO,
            franking_credits: Decimal::ZERO,
            lic_capital_gain_deduction: Decimal::from(5),
            conduit_foreign_income: Decimal::from(3),
            trust_income: true,
            reinvestment_trade_id: None,
            currency: "AUD".to_string(),
            buyback_trade_id: None,
            amount_per_security: None,
            securities_held: None,
        };
        db_upsert(&pool, &dist).await.unwrap();
        let got = db_get(&pool, 2).await.unwrap().unwrap();
        assert!(got.trust_income);
        assert_eq!(got.foreign_source_income, Decimal::from(10));
        assert_eq!(got.conduit_foreign_income, Decimal::from(3));
        assert_eq!(got.lic_capital_gain_deduction, Decimal::from(5));
    }

    #[tokio::test]
    async fn db_drp_reinvestment_linkage() {
        let pool = test_pool().await;
        insert_test_listing(&pool).await;
        let trade_id = insert_test_trade(&pool).await;
        let inc = Income {
            holding_account_id: 1,
            id: 3,
            listing_id: 1,
            date_paid: NaiveDate::from_ymd_opt(2024, 3, 15).unwrap(),
            ex_date: None,
            franked_amount: Decimal::from(140),
            unfranked_amount: Decimal::from(60),
            foreign_source_income: Decimal::ZERO,
            foreign_tax_paid: Decimal::ZERO,
            tfn_withholding_tax: Decimal::ZERO,
            franking_credits: Decimal::from(60),
            lic_capital_gain_deduction: Decimal::ZERO,
            conduit_foreign_income: Decimal::ZERO,
            trust_income: false,
            reinvestment_trade_id: Some(trade_id),
            currency: "AUD".to_string(),
            buyback_trade_id: None,
            amount_per_security: None,
            securities_held: None,
        };
        db_upsert(&pool, &inc).await.unwrap();
        let got = db_get(&pool, 3).await.unwrap().unwrap();
        assert_eq!(got.reinvestment_trade_id, Some(trade_id));
        assert_eq!(got.franked_amount, Decimal::from(140));
    }

    #[tokio::test]
    async fn db_get_missing_returns_none() {
        let pool = test_pool().await;
        assert!(db_get(&pool, 999).await.unwrap().is_none());
    }

    // API-level tests

    #[tokio::test]
    async fn api_upsert_and_get() {
        let pool = test_pool().await;
        insert_test_listing(&pool).await;
        let body = serde_json::json!({
            "listing_id": 1,
            "date_paid": "2024-03-15",
            "ex_date": "2024-03-01",
            "franked_amount": 70.0,
            "unfranked_amount": 30.0,
            "franking_credits": 30.0
        });
        let resp = router()
            .with_state(pool.clone())
            .oneshot(
                Request::builder()
                    .method("PUT")
                    .uri("/income/1")
                    .header("content-type", "application/json")
                    .body(Body::from(body.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NO_CONTENT);
        let got = db_get(&pool, 1).await.unwrap().unwrap();
        assert_eq!(got.franked_amount, Decimal::from(70));
    }

    #[tokio::test]
    async fn api_list_returns_ok() {
        let pool = test_pool().await;
        insert_test_listing(&pool).await;
        db_upsert(&pool, &dividend_income()).await.unwrap();
        let resp = router()
            .with_state(pool)
            .oneshot(Request::builder().uri("/income").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let items: Vec<Income> = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(items.len(), 1);
    }

    #[tokio::test]
    async fn api_get_missing_returns_404() {
        let pool = test_pool().await;
        let resp = router()
            .with_state(pool)
            .oneshot(Request::builder().uri("/income/999").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn api_delete_existing_returns_no_content() {
        let pool = test_pool().await;
        insert_test_listing(&pool).await;
        db_upsert(&pool, &dividend_income()).await.unwrap();
        let resp = router()
            .with_state(pool)
            .oneshot(
                Request::builder()
                    .method("DELETE")
                    .uri("/income/1")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NO_CONTENT);
    }

    #[tokio::test]
    async fn api_delete_missing_returns_404() {
        let pool = test_pool().await;
        let resp = router()
            .with_state(pool)
            .oneshot(
                Request::builder()
                    .method("DELETE")
                    .uri("/income/999")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn api_decimal_precision_round_trip() {
        let pool = test_pool().await;
        insert_test_listing(&pool).await;
        let body = serde_json::json!({
            "listing_id": 1,
            "date_paid": "2024-03-15",
            "franked_amount": "70.123456789",
            "unfranked_amount": "29.876543211",
            "franking_credits": "30.052631578"
        });
        let resp = router()
            .with_state(pool.clone())
            .oneshot(
                Request::builder()
                    .method("PUT")
                    .uri("/income/1")
                    .header("content-type", "application/json")
                    .body(Body::from(body.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NO_CONTENT);
        let resp = router()
            .with_state(pool)
            .oneshot(Request::builder().uri("/income/1").body(Body::empty()).unwrap())
            .await
            .unwrap();
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let inc: Income = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(inc.franked_amount, "70.123456789".parse::<Decimal>().unwrap());
        assert_eq!(inc.unfranked_amount, "29.876543211".parse::<Decimal>().unwrap());
        assert_eq!(inc.franking_credits, "30.052631578".parse::<Decimal>().unwrap());
    }

    // Per-share cross-check tests

    async fn put_income(pool: &SqlitePool, id: i64, body: serde_json::Value) -> (StatusCode, String) {
        let resp = router()
            .with_state(pool.clone())
            .oneshot(
                Request::builder()
                    .method("PUT")
                    .uri(format!("/income/{id}"))
                    .header("content-type", "application/json")
                    .body(Body::from(body.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        let status = resp.status();
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        (status, String::from_utf8(bytes.to_vec()).unwrap())
    }

    /// PLS 2023 final dividend payment advice: 14 cents per share × 19,695
    /// shares = $2,757.30, 100% franked, franking credit $1,181.70.
    #[tokio::test]
    async fn api_per_share_figures_reconcile_fully_franked_dividend() {
        let pool = test_pool().await;
        insert_test_listing(&pool).await;
        let (status, _) = put_income(
            &pool,
            1,
            serde_json::json!({
                "listing_id": 1,
                "date_paid": "2023-09-27",
                "franked_amount": "2757.30",
                "franking_credits": "1181.70",
                "amount_per_security": "0.14",
                "securities_held": "19695"
            }),
        )
        .await;
        assert_eq!(status, StatusCode::NO_CONTENT);
        let got = db_get(&pool, 1).await.unwrap().unwrap();
        assert_eq!(got.amount_per_security, Some("0.14".parse().unwrap()));
        assert_eq!(got.securities_held, Some(Decimal::from(19695)));
    }

    /// VDHG 2020-10 distribution advice: $0.89891492 per security × 866
    /// securities = $778.4603… — the statement's gross is the cent-rounded
    /// $778.46, so the check must round the product before comparing.
    #[tokio::test]
    async fn api_per_share_product_is_cent_rounded_before_comparison() {
        let pool = test_pool().await;
        insert_test_listing(&pool).await;
        let (status, _) = put_income(
            &pool,
            1,
            serde_json::json!({
                "listing_id": 1,
                "date_paid": "2020-10-16",
                "unfranked_amount": "778.46",
                "trust_income": true,
                "amount_per_security": "0.89891492",
                "securities_held": "866"
            }),
        )
        .await;
        assert_eq!(status, StatusCode::NO_CONTENT);
    }

    #[tokio::test]
    async fn api_per_share_mismatch_returns_422_with_detail_and_persists_nothing() {
        let pool = test_pool().await;
        insert_test_listing(&pool).await;
        let (status, detail) = put_income(
            &pool,
            1,
            serde_json::json!({
                "listing_id": 1,
                "date_paid": "2023-09-27",
                "franked_amount": "2757.30",
                // Typo'd per-share rate: 0.15 × 19,695 = 2,954.25 ≠ 2,757.30.
                "amount_per_security": "0.15",
                "securities_held": "19695"
            }),
        )
        .await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
        // The rejection carries the computed product so the typo is findable.
        assert!(detail.contains("2954.25"), "detail should cite the product: {detail}");
        assert!(db_get(&pool, 1).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn api_per_share_field_supplied_alone_returns_422() {
        let pool = test_pool().await;
        insert_test_listing(&pool).await;
        for body in [
            serde_json::json!({
                "listing_id": 1, "date_paid": "2024-03-15",
                "unfranked_amount": "100", "amount_per_security": "0.14"
            }),
            serde_json::json!({
                "listing_id": 1, "date_paid": "2024-03-15",
                "unfranked_amount": "100", "securities_held": "19695"
            }),
        ] {
            let (status, detail) = put_income(&pool, 1, body).await;
            assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
            assert!(detail.contains("together"), "detail: {detail}");
        }
        assert!(db_get(&pool, 1).await.unwrap().is_none());
    }

    /// The gross the product must match includes foreign source income but
    /// not franking credits or TFN withholding (notional / deducted-from).
    #[tokio::test]
    async fn api_per_share_gross_includes_foreign_income_not_credits_or_withholding() {
        let pool = test_pool().await;
        insert_test_listing(&pool).await;
        // 1.00 × 100 = 100 = franked 60 + unfranked 30 + foreign 10; the
        // franking credits and TFN withholding must not disturb the check.
        let (status, _) = put_income(
            &pool,
            1,
            serde_json::json!({
                "listing_id": 1,
                "date_paid": "2024-03-15",
                "franked_amount": "60",
                "unfranked_amount": "30",
                "foreign_source_income": "10",
                "franking_credits": "25.71",
                "tfn_withholding_tax": "47",
                "amount_per_security": "1.00",
                "securities_held": "100"
            }),
        )
        .await;
        assert_eq!(status, StatusCode::NO_CONTENT);
    }

    /// Both omitted = no check (existing clients unchanged), and the columns
    /// stay NULL.
    #[tokio::test]
    async fn api_omitted_per_share_pair_skips_the_check() {
        let pool = test_pool().await;
        insert_test_listing(&pool).await;
        let (status, _) = put_income(
            &pool,
            1,
            serde_json::json!({
                "listing_id": 1,
                "date_paid": "2024-03-15",
                "franked_amount": "70"
            }),
        )
        .await;
        assert_eq!(status, StatusCode::NO_CONTENT);
        let got = db_get(&pool, 1).await.unwrap().unwrap();
        assert_eq!(got.amount_per_security, None);
        assert_eq!(got.securities_held, None);
    }

    #[tokio::test]
    async fn api_per_share_decimal_precision_round_trips() {
        let pool = test_pool().await;
        insert_test_listing(&pool).await;
        let (status, _) = put_income(
            &pool,
            1,
            serde_json::json!({
                "listing_id": 1,
                "date_paid": "2020-10-16",
                "unfranked_amount": "778.46",
                "amount_per_security": "0.89891492",
                "securities_held": "866"
            }),
        )
        .await;
        assert_eq!(status, StatusCode::NO_CONTENT);
        let resp = router()
            .with_state(pool)
            .oneshot(Request::builder().uri("/income/1").body(Body::empty()).unwrap())
            .await
            .unwrap();
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let inc: Income = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(inc.amount_per_security, Some("0.89891492".parse::<Decimal>().unwrap()));
        assert_eq!(inc.securities_held, Some(Decimal::from(866)));
    }
}
