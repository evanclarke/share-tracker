use axum::{
    Json, Router,
    extract::{Path, State},
    http::StatusCode,
    routing::get,
};
use chrono::NaiveDate;
use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct Income {
    pub id: i64,
    pub listing_id: i64,
    pub date_paid: NaiveDate,
    pub ex_date: Option<NaiveDate>,
    pub franked_amount: f64,
    pub unfranked_amount: f64,
    pub foreign_source_income: f64,
    pub foreign_tax_paid: f64,
    pub tfn_withholding_tax: f64,
    pub franking_credits: f64,
    pub lic_capital_gain_deduction: f64,
    pub conduit_foreign_income: f64,
    pub trust_income: bool,
    pub reinvestment_trade_id: Option<i64>,
}

#[derive(Debug, Deserialize)]
pub struct IncomeBody {
    pub listing_id: i64,
    pub date_paid: NaiveDate,
    #[serde(default)]
    pub ex_date: Option<NaiveDate>,
    #[serde(default)]
    pub franked_amount: f64,
    #[serde(default)]
    pub unfranked_amount: f64,
    #[serde(default)]
    pub foreign_source_income: f64,
    #[serde(default)]
    pub foreign_tax_paid: f64,
    #[serde(default)]
    pub tfn_withholding_tax: f64,
    #[serde(default)]
    pub franking_credits: f64,
    #[serde(default)]
    pub lic_capital_gain_deduction: f64,
    #[serde(default)]
    pub conduit_foreign_income: f64,
    #[serde(default)]
    pub trust_income: bool,
    #[serde(default)]
    pub reinvestment_trade_id: Option<i64>,
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
         lic_capital_gain_deduction, conduit_foreign_income, trust_income, reinvestment_trade_id \
         FROM income ORDER BY date_paid, id",
    )
    .fetch_all(pool)
    .await
}

pub async fn db_get(pool: &SqlitePool, id: i64) -> Result<Option<Income>, sqlx::Error> {
    sqlx::query_as(
        "SELECT id, listing_id, date_paid, ex_date, franked_amount, unfranked_amount, \
         foreign_source_income, foreign_tax_paid, tfn_withholding_tax, franking_credits, \
         lic_capital_gain_deduction, conduit_foreign_income, trust_income, reinvestment_trade_id \
         FROM income WHERE id = ?",
    )
    .bind(id)
    .fetch_optional(pool)
    .await
}

pub async fn db_upsert(pool: &SqlitePool, income: &Income) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO income \
         (id, listing_id, date_paid, ex_date, franked_amount, unfranked_amount, \
          foreign_source_income, foreign_tax_paid, tfn_withholding_tax, franking_credits, \
          lic_capital_gain_deduction, conduit_foreign_income, trust_income, reinvestment_trade_id) \
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?) \
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
             reinvestment_trade_id      = excluded.reinvestment_trade_id",
    )
    .bind(income.id)
    .bind(income.listing_id)
    .bind(income.date_paid)
    .bind(income.ex_date)
    .bind(income.franked_amount)
    .bind(income.unfranked_amount)
    .bind(income.foreign_source_income)
    .bind(income.foreign_tax_paid)
    .bind(income.tfn_withholding_tax)
    .bind(income.franking_credits)
    .bind(income.lic_capital_gain_deduction)
    .bind(income.conduit_foreign_income)
    .bind(income.trust_income)
    .bind(income.reinvestment_trade_id)
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn db_delete(pool: &SqlitePool, id: i64) -> Result<bool, sqlx::Error> {
    let result = sqlx::query("DELETE FROM income WHERE id = ?")
        .bind(id)
        .execute(pool)
        .await?;
    Ok(result.rows_affected() > 0)
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
) -> Result<StatusCode, StatusCode> {
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
    };
    db_upsert(&pool, &income)
        .await
        .map(|_| StatusCode::NO_CONTENT)
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)
}

async fn delete(
    State(pool): State<SqlitePool>,
    Path(id): Path<i64>,
) -> Result<StatusCode, StatusCode> {
    db_delete(&pool, id)
        .await
        .map(|found| if found { StatusCode::NO_CONTENT } else { StatusCode::NOT_FOUND })
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{db, listing, trade};
    use axum::{body::Body, http::Request};
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    async fn test_pool() -> SqlitePool {
        db::init(":memory:").await.unwrap()
    }

    async fn insert_test_listing(pool: &SqlitePool) {
        listing::db_upsert(
            pool,
            &listing::Listing {
                id: 1,
                exchange_mic: "XASX".to_string(),
                ticker: "VAS".to_string(),
                name: "Vanguard Australian Shares ETF".to_string(),
                isin: None,
                security_type: "ETF".to_string(),
                currency: "AUD".to_string(),
                amit: false,
            },
        )
        .await
        .unwrap();
    }

    async fn insert_test_trade(pool: &SqlitePool) -> i64 {
        let t = trade::Trade {
            id: 1,
            trade_type: "DRP".to_string(),
            date: NaiveDate::from_ymd_opt(2024, 3, 15).unwrap(),
            settlement_date: NaiveDate::from_ymd_opt(2024, 3, 15).unwrap(),
            listing_id: 1,
            average_price: 95.0,
            quantity: 2.0,
            currency: "AUD".to_string(),
            brokerage: 0.0,
            gst_on_brokerage: 0.0,
            brokerage_currency: "AUD".to_string(),
            fx_rate: 1.0,
            contract_note_ref: None,
        };
        trade::db_upsert(pool, &t).await.unwrap();
        t.id
    }

    fn dividend_income() -> Income {
        Income {
            id: 1,
            listing_id: 1,
            date_paid: NaiveDate::from_ymd_opt(2024, 3, 15).unwrap(),
            ex_date: Some(NaiveDate::from_ymd_opt(2024, 3, 1).unwrap()),
            franked_amount: 70.0,
            unfranked_amount: 30.0,
            foreign_source_income: 0.0,
            foreign_tax_paid: 0.0,
            tfn_withholding_tax: 0.0,
            franking_credits: 30.0,
            lic_capital_gain_deduction: 0.0,
            conduit_foreign_income: 0.0,
            trust_income: false,
            reinvestment_trade_id: None,
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
        assert_eq!(got.franked_amount, 70.0);
        assert_eq!(got.unfranked_amount, 30.0);
        assert_eq!(got.franking_credits, 30.0);
        assert_eq!(got.ex_date, Some(NaiveDate::from_ymd_opt(2024, 3, 1).unwrap()));
        assert!(!got.trust_income);
        assert!(got.reinvestment_trade_id.is_none());
    }

    #[tokio::test]
    async fn db_trust_distribution_insert_and_retrieve() {
        let pool = test_pool().await;
        insert_test_listing(&pool).await;
        let dist = Income {
            id: 2,
            listing_id: 1,
            date_paid: NaiveDate::from_ymd_opt(2024, 6, 30).unwrap(),
            ex_date: None,
            franked_amount: 0.0,
            unfranked_amount: 50.0,
            foreign_source_income: 10.0,
            foreign_tax_paid: 1.5,
            tfn_withholding_tax: 0.0,
            franking_credits: 0.0,
            lic_capital_gain_deduction: 5.0,
            conduit_foreign_income: 3.0,
            trust_income: true,
            reinvestment_trade_id: None,
        };
        db_upsert(&pool, &dist).await.unwrap();
        let got = db_get(&pool, 2).await.unwrap().unwrap();
        assert!(got.trust_income);
        assert_eq!(got.foreign_source_income, 10.0);
        assert_eq!(got.conduit_foreign_income, 3.0);
        assert_eq!(got.lic_capital_gain_deduction, 5.0);
    }

    #[tokio::test]
    async fn db_drp_reinvestment_linkage() {
        let pool = test_pool().await;
        insert_test_listing(&pool).await;
        let trade_id = insert_test_trade(&pool).await;
        let inc = Income {
            id: 3,
            listing_id: 1,
            date_paid: NaiveDate::from_ymd_opt(2024, 3, 15).unwrap(),
            ex_date: None,
            franked_amount: 140.0,
            unfranked_amount: 60.0,
            foreign_source_income: 0.0,
            foreign_tax_paid: 0.0,
            tfn_withholding_tax: 0.0,
            franking_credits: 60.0,
            lic_capital_gain_deduction: 0.0,
            conduit_foreign_income: 0.0,
            trust_income: false,
            reinvestment_trade_id: Some(trade_id),
        };
        db_upsert(&pool, &inc).await.unwrap();
        let got = db_get(&pool, 3).await.unwrap().unwrap();
        assert_eq!(got.reinvestment_trade_id, Some(trade_id));
        assert_eq!(got.franked_amount, 140.0);
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
        assert_eq!(got.franked_amount, 70.0);
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
}
