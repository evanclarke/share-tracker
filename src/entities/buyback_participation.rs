//! Atomic buy-back participation: sell units into a `BuyBack` corporate
//! action, splitting the buy-back price into its capital and dividend
//! components (see `docs/share-buy-backs.md`, QC 66049).
//!
//! Selling shares back to the company is an ordinary CGT disposal, so the
//! participation creates a regular Sell trade with the caller's parcel
//! allocations (specific identification applies as for any sale) — but its
//! per-unit price is the **capital proceeds per unit**, not the cheque
//! amount: `max(buy-back price, market value had the buy-back not been
//! proposed) − dividend component`. The ATO's market-value rule says capital
//! proceeds can't be less than that market value, and any dividend paid as
//! part of the buy-back price is excluded from them. The dividend component
//! (`dividend × units`, with its franking credits) is created as an income
//! row in the same transaction, so it is assessable in the tax summary and
//! subject to the ordinary franking-credit entitlement rules. A buy-back with
//! no dividend component (a listed-company buy-back announced after 7:30 pm
//! AEDT 25 Oct 2022) creates no income row — the whole price is capital
//! proceeds.
//!
//! The proceeds are paid by the company, not market-settled, so the Sell's
//! settlement date is the participation date. The created rows carry
//! provenance links (`trades.buyback_action_id`, `income.buyback_trade_id`)
//! that keep the split consistent: the Sell is immutable via `PUT /sells`
//! and removed — together with its income row — by `DELETE /sells/:id`; the
//! income row is rejected by `PUT`/`DELETE /income`; and the action itself
//! is frozen against edits and deletes while participations reference it.
//!
//! Out of scope (documented in `docs/share-buy-backs.md`): the further
//! adjustments where the participating shareholder is itself a company, and
//! shares held on revenue account.

use crate::entities::corporate_action::{self, ActionKind};
use crate::entities::income::{self, Income};
use crate::entities::sell::{self, AllocationInput, SellBody};
use crate::entities::trade::{self, Trade};
use axum::{
    Json, Router,
    extract::{Path, State},
    http::StatusCode,
    routing::post,
};
use chrono::NaiveDate;
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;

#[derive(Debug, Deserialize)]
pub struct ParticipationBody {
    /// The date of the CGT event — when the company accepts the application
    /// (or the holder accepts a conditional offer). Must not precede the
    /// action's buy-back date. Also the dividend component's payment date.
    pub date: NaiveDate,
    /// Units sold into the buy-back (strictly positive; must equal the sum
    /// of the allocations).
    pub units: Decimal,
    /// The parcels the sold units come from — specific identification, the
    /// same shape and write-time invariants as `PUT /sells`.
    #[serde(default)]
    pub allocations: Vec<AllocationInput>,
    /// Optional foreign-per-AUD override for the created trade (defaults to
    /// 1; reports prefer the ATO rate and fall back to this — see `infra::fx`).
    #[serde(default)]
    pub fx_rate: Option<Decimal>,
}

/// The two sides of a participation: the Sell carrying the capital proceeds,
/// and the dividend-component income row (`None` when the buy-back price has
/// no dividend component).
#[derive(Debug, Serialize)]
pub struct Participation {
    pub trade: Trade,
    pub income: Option<Income>,
}

#[derive(Debug)]
pub enum ParticipationError {
    Db(sqlx::Error),
    /// No corporate action with that id.
    ActionNotFound,
    /// The action is not a BuyBack.
    NotABuyBack,
    /// `units` is not strictly positive.
    NonPositiveUnits,
    /// The participation date precedes the action's buy-back date.
    BeforeBuyBackDate,
    /// The Sell-side invariants failed (allocation mismatch, bad parcel,
    /// over-allocation, ...).
    Sell(sell::SellError),
}

impl From<sqlx::Error> for ParticipationError {
    fn from(e: sqlx::Error) -> Self {
        ParticipationError::Db(e)
    }
}

impl From<sell::SellError> for ParticipationError {
    fn from(e: sell::SellError) -> Self {
        match e {
            sell::SellError::Db(err) => ParticipationError::Db(err),
            other => ParticipationError::Sell(other),
        }
    }
}

pub fn router() -> Router<SqlitePool> {
    Router::new().route("/corporate_actions/{id}/participate", post(participate))
}

/// Create the Sell trade (with its allocations) and the dividend-component
/// income row for a buy-back participation, atomically.
pub async fn db_participate(
    pool: &SqlitePool,
    action_id: i64,
    body: &ParticipationBody,
) -> Result<Participation, ParticipationError> {
    if body.units <= Decimal::ZERO {
        return Err(ParticipationError::NonPositiveUnits);
    }

    let mut tx = pool.begin().await?;

    let action = match corporate_action::db_get_tx(&mut *tx, action_id).await? {
        Some(a) => a,
        None => return Err(ParticipationError::ActionNotFound),
    };
    let (price, dividend, credit, market_value, currency) = match &action.kind {
        ActionKind::BuyBack {
            buyback_price,
            buyback_dividend,
            buyback_franking_credit,
            buyback_market_value,
            currency,
        } => (
            *buyback_price,
            *buyback_dividend,
            *buyback_franking_credit,
            *buyback_market_value,
            currency.clone(),
        ),
        _ => return Err(ParticipationError::NotABuyBack),
    };
    if body.date < action.date {
        return Err(ParticipationError::BeforeBuyBackDate);
    }

    // Capital proceeds per unit (QC 66049): can't be less than the market
    // value had the buy-back not been proposed, and the dividend component
    // is excluded. A market value below the price (or omitted) leaves the
    // price as the proceeds base.
    let proceeds_per_unit = price.max(market_value.unwrap_or(price)) - dividend;

    // The Sell goes through the shared transactional core, so every /sells
    // invariant (allocations sum to the quantity; parcels are valid,
    // not-over-allocated Buy/DRPs) holds here too. Proceeds are paid by the
    // company, not market-settled: settlement is the participation date.
    let sell_id: i64 = sqlx::query_scalar("SELECT COALESCE(MAX(id), 0) + 1 FROM trades")
        .fetch_one(&mut *tx)
        .await?;
    let sell_body = SellBody {
        date: body.date,
        settlement_date: Some(body.date),
        listing_id: action.listing_id,
        average_price: proceeds_per_unit,
        quantity: body.units,
        currency: currency.clone(),
        brokerage: Decimal::ZERO,
        gst_on_brokerage: Decimal::ZERO,
        brokerage_currency: currency.clone(),
        fx_rate: body.fx_rate.unwrap_or(Decimal::ONE),
        contract_note_ref: None,
        allocations: body
            .allocations
            .iter()
            .map(|a| AllocationInput {
                purchase_trade_id: a.purchase_trade_id,
                quantity_allocated: a.quantity_allocated,
            })
            .collect(),
    };
    sell::upsert_sell_in_tx(&mut tx, sell_id, &sell_body, body.date, Some(action_id), None, None)
        .await?;

    // The dividend component: assessable franked income on the participation
    // date, linked to the Sell so the two sides stay consistent.
    let income_id = if dividend > Decimal::ZERO {
        let id: i64 = sqlx::query_scalar("SELECT COALESCE(MAX(id), 0) + 1 FROM income")
            .fetch_one(&mut *tx)
            .await?;
        sqlx::query(
            "INSERT INTO income \
             (id, listing_id, date_paid, franked_amount, franking_credits, currency, \
              buyback_trade_id) \
             VALUES (?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(id)
        .bind(action.listing_id)
        .bind(body.date)
        .bind((dividend * body.units).to_string())
        .bind((credit * body.units).to_string())
        .bind(&currency)
        .bind(sell_id)
        .execute(&mut *tx)
        .await?;
        Some(id)
    } else {
        None
    };

    tx.commit().await?;

    // Read the freshly created rows back so the response is exactly what was
    // stored.
    let trade = trade::db_get(pool, sell_id)
        .await?
        .ok_or_else(|| ParticipationError::Db(sqlx::Error::RowNotFound))?;
    let income = match income_id {
        Some(id) => Some(
            income::db_get(pool, id)
                .await?
                .ok_or_else(|| ParticipationError::Db(sqlx::Error::RowNotFound))?,
        ),
        None => None,
    };
    Ok(Participation { trade, income })
}

async fn participate(
    State(pool): State<SqlitePool>,
    Path(action_id): Path<i64>,
    Json(body): Json<ParticipationBody>,
) -> Result<(StatusCode, Json<Participation>), StatusCode> {
    match db_participate(&pool, action_id, &body).await {
        Ok(participation) => Ok((StatusCode::CREATED, Json(participation))),
        Err(ParticipationError::ActionNotFound) => Err(StatusCode::NOT_FOUND),
        Err(
            ParticipationError::NotABuyBack
            | ParticipationError::NonPositiveUnits
            | ParticipationError::BeforeBuyBackDate,
        ) => Err(StatusCode::UNPROCESSABLE_ENTITY),
        Err(ParticipationError::Sell(e)) => {
            tracing::warn!(error = ?e, "buy-back participation rejected by a sell invariant");
            Err(StatusCode::UNPROCESSABLE_ENTITY)
        }
        Err(ParticipationError::Db(e)) => {
            tracing::error!(error = %e, "buy-back participation failed");
            Err(StatusCode::INTERNAL_SERVER_ERROR)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::entities::trade::TradeType;
    use crate::entities::{corporate_action::CorporateAction, listing};
    use crate::infra::db;
    use axum::{body::Body, http::Request};
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    async fn test_pool() -> SqlitePool {
        db::init(":memory:").await.unwrap()
    }

    fn d(y: i32, m: u32, day: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, day).unwrap()
    }

    fn dec(s: &str) -> Decimal {
        s.parse().unwrap()
    }

    async fn insert_listing(pool: &SqlitePool, id: i64) {
        listing::db_upsert(
            pool,
            &listing::Listing {
                id,
                exchange_mic: "XASX".to_string(),
                ticker: "BBK".to_string(),
                name: "Buy-Back Test Co".to_string(),
                isin: None,
                security_type: listing::SecurityType::Share,
                currency: "AUD".to_string(),
                amit: false,
                preference: false,
            },
        )
        .await
        .unwrap();
    }

    async fn insert_buy(pool: &SqlitePool, id: i64, date: NaiveDate, qty: &str, price: &str) {
        trade::db_upsert(
            pool,
            &Trade {
                id,
                trade_type: TradeType::Buy,
                date,
                settlement_date: date,
                listing_id: 1,
                average_price: price.parse().unwrap(),
                quantity: qty.parse().unwrap(),
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
            },
        )
        .await
        .unwrap();
    }

    /// Ranjini-shaped terms (docs/share-buy-backs.md): $9.60 price including
    /// a $1.40 franked dividend with a $0.60 credit; market value had the
    /// buy-back not been proposed $10.20.
    async fn insert_buyback(pool: &SqlitePool, id: i64, date: NaiveDate) {
        insert_buyback_terms(pool, id, date, "9.60", "1.40", "0.60", Some("10.20")).await;
    }

    async fn insert_buyback_terms(
        pool: &SqlitePool,
        id: i64,
        date: NaiveDate,
        price: &str,
        dividend: &str,
        credit: &str,
        market_value: Option<&str>,
    ) {
        corporate_action::db_upsert(
            pool,
            &CorporateAction {
                id,
                listing_id: 1,
                date,
                kind: ActionKind::BuyBack {
                    buyback_price: price.parse().unwrap(),
                    buyback_dividend: dividend.parse().unwrap(),
                    buyback_franking_credit: credit.parse().unwrap(),
                    buyback_market_value: market_value.map(|v| v.parse().unwrap()),
                    currency: "AUD".to_string(),
                },
            },
        )
        .await
        .unwrap();
    }

    fn body(date: NaiveDate, units: &str, parcel: i64) -> ParticipationBody {
        ParticipationBody {
            date,
            units: units.parse().unwrap(),
            allocations: vec![AllocationInput {
                purchase_trade_id: parcel,
                quantity_allocated: units.parse().unwrap(),
            }],
            fx_rate: None,
        }
    }

    // DB-level tests

    /// The market-value rule: the buy-back price ($9.60) is below the market
    /// value ($10.20), so capital proceeds use the market value, less the
    /// dividend → $8.80/unit. The dividend side lands as franked income.
    #[tokio::test]
    async fn participation_splits_capital_proceeds_from_the_dividend() {
        let pool = test_pool().await;
        insert_listing(&pool, 1).await;
        insert_buy(&pool, 1, d(2020, 1, 15), "10000", "6.00").await;
        insert_buyback(&pool, 10, d(2024, 7, 1)).await;

        let p = db_participate(&pool, 10, &body(d(2024, 7, 10), "1000", 1)).await.unwrap();

        // The Sell: 1,000 units at $8.80 capital proceeds per unit
        // (max(9.60, 10.20) − 1.40), settled on the participation date.
        assert_eq!(p.trade.trade_type, TradeType::Sell);
        assert_eq!(p.trade.date, d(2024, 7, 10));
        assert_eq!(p.trade.settlement_date, d(2024, 7, 10));
        assert_eq!(p.trade.quantity, dec("1000"));
        assert_eq!(p.trade.average_price, dec("8.80"));
        assert_eq!(p.trade.currency, "AUD");
        assert_eq!(p.trade.buyback_action_id, Some(10));

        // The dividend component: $1,400 franked + $600 credits, linked back
        // to the Sell.
        let income = p.income.expect("a dividend component income row");
        assert_eq!(income.listing_id, 1);
        assert_eq!(income.date_paid, d(2024, 7, 10));
        assert_eq!(income.franked_amount, dec("1400"));
        assert_eq!(income.franking_credits, dec("600"));
        assert_eq!(income.currency, "AUD");
        assert_eq!(income.buyback_trade_id, Some(p.trade.id));

        // The allocation persisted with the Sell.
        let n: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM parcel_allocations WHERE sale_trade_id = ?")
                .bind(p.trade.id)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(n, 1);
    }

    /// A market value below the price never reduces proceeds (the rule only
    /// lifts them), and an omitted market value uses the price.
    #[tokio::test]
    async fn market_value_only_ever_lifts_capital_proceeds() {
        let pool = test_pool().await;
        insert_listing(&pool, 1).await;
        insert_buy(&pool, 1, d(2020, 1, 15), "10000", "6.00").await;

        // MV $9.00 < price $9.60 → proceeds = 9.60 − 1.40 = 8.20.
        insert_buyback_terms(&pool, 10, d(2024, 7, 1), "9.60", "1.40", "0.60", Some("9.00")).await;
        let p = db_participate(&pool, 10, &body(d(2024, 7, 10), "100", 1)).await.unwrap();
        assert_eq!(p.trade.average_price, dec("8.20"));

        // No MV recorded → proceeds = 9.60 − 1.40 = 8.20.
        insert_buyback_terms(&pool, 11, d(2024, 8, 1), "9.60", "1.40", "0.60", None).await;
        let p = db_participate(&pool, 11, &body(d(2024, 8, 10), "100", 1)).await.unwrap();
        assert_eq!(p.trade.average_price, dec("8.20"));
    }

    /// A buy-back with no dividend component (a listed-company buy-back
    /// announced after 25 Oct 2022): the whole price is capital proceeds and
    /// no income row is created.
    #[tokio::test]
    async fn no_dividend_component_creates_no_income_row() {
        let pool = test_pool().await;
        insert_listing(&pool, 1).await;
        insert_buy(&pool, 1, d(2020, 1, 15), "10000", "6.00").await;
        insert_buyback_terms(&pool, 10, d(2024, 7, 1), "9.60", "0", "0", None).await;

        let p = db_participate(&pool, 10, &body(d(2024, 7, 10), "1000", 1)).await.unwrap();
        assert_eq!(p.trade.average_price, dec("9.60"));
        assert!(p.income.is_none());

        let n: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM income")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(n, 0);
    }

    /// The created Sell flows through the realised-gains report like any
    /// disposal: Ranjini's gain is $8,800 − $6,000 = $2,800, discount-eligible
    /// because the parcel was held for years.
    #[tokio::test]
    async fn realised_gain_uses_the_capital_proceeds() {
        let pool = test_pool().await;
        insert_listing(&pool, 1).await;
        insert_buy(&pool, 1, d(2020, 1, 15), "10000", "6.00").await;
        insert_buyback(&pool, 10, d(2024, 7, 1)).await;
        let p = db_participate(&pool, 10, &body(d(2024, 7, 10), "1000", 1)).await.unwrap();

        let gains = crate::reports::realised_gains::db_realised_gains(&pool).await.unwrap();
        let sale = gains.iter().find(|g| g.sale_trade_id == p.trade.id).unwrap();
        assert_eq!(sale.proceeds, dec("8800.00"));
        assert_eq!(sale.cost_base, dec("6000.00"));
        assert_eq!(sale.capital_gain_loss, dec("2800.00"));
        assert_eq!(sale.discount_eligible_gain, dec("2800.00"));
    }

    #[tokio::test]
    async fn invalid_participations_are_rejected_and_nothing_persisted() {
        let pool = test_pool().await;
        insert_listing(&pool, 1).await;
        insert_buy(&pool, 1, d(2020, 1, 15), "10000", "6.00").await;
        insert_buyback(&pool, 10, d(2024, 7, 1)).await;
        // A non-buy-back action on the same listing (a ReturnOfCapital — it
        // adjusts cost bases, never the parcel's unit capacity).
        corporate_action::db_upsert(
            &pool,
            &CorporateAction {
                id: 11,
                listing_id: 1,
                date: d(2024, 7, 1),
                kind: ActionKind::ReturnOfCapital {
                    amount_per_unit: dec("0.10"),
                    currency: "AUD".to_string(),
                },
            },
        )
        .await
        .unwrap();

        let err = db_participate(&pool, 999, &body(d(2024, 7, 10), "100", 1)).await;
        assert!(matches!(err, Err(ParticipationError::ActionNotFound)));
        let err = db_participate(&pool, 11, &body(d(2024, 7, 10), "100", 1)).await;
        assert!(matches!(err, Err(ParticipationError::NotABuyBack)));
        let err = db_participate(&pool, 10, &body(d(2024, 6, 30), "100", 1)).await;
        assert!(matches!(err, Err(ParticipationError::BeforeBuyBackDate)));
        let err = db_participate(&pool, 10, &body(d(2024, 7, 10), "0", 1)).await;
        assert!(matches!(err, Err(ParticipationError::NonPositiveUnits)));
        let err = db_participate(&pool, 10, &body(d(2024, 7, 10), "-5", 1)).await;
        assert!(matches!(err, Err(ParticipationError::NonPositiveUnits)));

        // Sell-side invariants: allocations must sum to the units…
        let mut mismatch = body(d(2024, 7, 10), "100", 1);
        mismatch.allocations[0].quantity_allocated = dec("60");
        let err = db_participate(&pool, 10, &mismatch).await;
        assert!(matches!(
            err,
            Err(ParticipationError::Sell(sell::SellError::AllocationMismatch))
        ));
        // …and may not over-allocate the parcel.
        let err = db_participate(&pool, 10, &body(d(2024, 7, 10), "10001", 1)).await;
        assert!(matches!(
            err,
            Err(ParticipationError::Sell(sell::SellError::PurchaseQuantityExceeded))
        ));

        // Nothing persisted by any rejected attempt.
        let trades: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM trades WHERE trade_type = 'Sell'")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(trades, 0, "no rejected participation may persist a trade");
        let incomes: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM income").fetch_one(&pool).await.unwrap();
        assert_eq!(incomes, 0, "no rejected participation may persist income");
    }

    /// The participation Sell is immutable via PUT /sells; deleting it via
    /// DELETE /sells removes the linked dividend income row with it.
    #[tokio::test]
    async fn participation_sell_is_immutable_and_deletes_with_its_income() {
        let pool = test_pool().await;
        insert_listing(&pool, 1).await;
        insert_buy(&pool, 1, d(2020, 1, 15), "10000", "6.00").await;
        insert_buyback(&pool, 10, d(2024, 7, 1)).await;
        let p = db_participate(&pool, 10, &body(d(2024, 7, 10), "1000", 1)).await.unwrap();
        let income_id = p.income.as_ref().unwrap().id;

        // Any edit through the sells endpoint is rejected.
        let edit = sell::SellBody {
            date: p.trade.date,
            settlement_date: Some(p.trade.settlement_date),
            listing_id: 1,
            average_price: dec("99"),
            quantity: dec("1000"),
            currency: "AUD".to_string(),
            brokerage: Decimal::ZERO,
            gst_on_brokerage: Decimal::ZERO,
            brokerage_currency: "AUD".to_string(),
            fx_rate: Decimal::ONE,
            contract_note_ref: None,
            allocations: vec![AllocationInput {
                purchase_trade_id: 1,
                quantity_allocated: dec("1000"),
            }],
        };
        let err = sell::db_upsert_sell(&pool, p.trade.id, &edit).await;
        assert!(matches!(err, Err(sell::SellError::BuyBackSell)));

        // The income row is managed by the participation: PUT/DELETE /income
        // reject it.
        let mut edited_income = p.income.clone().unwrap();
        edited_income.franked_amount = dec("9999");
        let err = income::db_upsert(&pool, &edited_income).await;
        assert!(matches!(err, Err(income::UpsertError::BuyBackIncome)));
        let outcome = income::db_delete(&pool, income_id).await.unwrap();
        assert_eq!(outcome, income::DeleteOutcome::BuyBackIncome);

        // DELETE /trades refuses too (the Sell is referenced by its
        // allocation and income row).
        let outcome = trade::db_delete(&pool, p.trade.id).await.unwrap();
        assert_eq!(outcome, trade::DeleteOutcome::Referenced);

        // DELETE /sells removes the Sell, its allocations, and the income row.
        let outcome = sell::db_delete_sell(&pool, p.trade.id).await.unwrap();
        assert_eq!(outcome, sell::DeleteOutcome::Deleted);
        assert!(trade::db_get(&pool, p.trade.id).await.unwrap().is_none());
        assert!(income::db_get(&pool, income_id).await.unwrap().is_none());
        let n: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM parcel_allocations")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(n, 0);

        // The holding is back to its pre-participation state (a withdrawn or
        // failed buy-back leaves no trace): the full 10,000 units at the
        // original cost base, and the parcel's capacity is freed so a fresh
        // participation for the same units succeeds.
        let holdings = crate::reports::portfolio::db_holdings(&pool).await.unwrap();
        assert_eq!(holdings.len(), 1);
        assert_eq!(holdings[0].quantity, dec("10000"));
        assert_eq!(holdings[0].total_cost_base, dec("60000.00"));
        db_participate(&pool, 10, &body(d(2024, 7, 11), "1000", 1)).await.unwrap();
    }

    /// The action the participations were created against is frozen while
    /// they reference it: editing or deleting it is rejected until they are
    /// gone.
    #[tokio::test]
    async fn referenced_action_cannot_be_edited_or_deleted() {
        let pool = test_pool().await;
        insert_listing(&pool, 1).await;
        insert_buy(&pool, 1, d(2020, 1, 15), "10000", "6.00").await;
        insert_buyback(&pool, 10, d(2024, 7, 1)).await;
        let p = db_participate(&pool, 10, &body(d(2024, 7, 10), "1000", 1)).await.unwrap();

        let action = corporate_action::db_get(&pool, 10).await.unwrap().unwrap();
        let err = corporate_action::db_upsert(&pool, &action).await;
        assert!(matches!(err, Err(corporate_action::WriteError::ReferencedByTrade)));
        let err = corporate_action::db_delete(&pool, 10).await;
        assert!(err.is_err(), "the trades.buyback_action_id FK must block the delete");

        // Removing the participation Sell unfreezes the action.
        sell::db_delete_sell(&pool, p.trade.id).await.unwrap();
        corporate_action::db_upsert(&pool, &action).await.unwrap();
        assert!(corporate_action::db_delete(&pool, 10).await.unwrap());
    }

    // API-level tests

    #[tokio::test]
    async fn api_participate_returns_201_with_trade_and_income() {
        let pool = test_pool().await;
        insert_listing(&pool, 1).await;
        insert_buy(&pool, 1, d(2020, 1, 15), "10000", "6.00").await;
        insert_buyback(&pool, 10, d(2024, 7, 1)).await;

        let resp = router()
            .with_state(pool.clone())
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/corporate_actions/10/participate")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        serde_json::json!({
                            "date": "2024-07-10",
                            "units": "1000",
                            "allocations": [
                                { "purchase_trade_id": 1, "quantity_allocated": "1000" }
                            ],
                        })
                        .to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::CREATED);
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let v: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(v["trade"]["average_price"], "8.80");
        assert_eq!(v["trade"]["quantity"], "1000");
        assert_eq!(v["trade"]["buyback_action_id"], 10);
        assert_eq!(v["income"]["franked_amount"], "1400.00");
        assert_eq!(v["income"]["franking_credits"], "600.00");
        assert_eq!(v["income"]["buyback_trade_id"], v["trade"]["id"]);
    }

    async fn api_participate_expecting(
        pool: &SqlitePool,
        action_id: i64,
        body: serde_json::Value,
        expected: StatusCode,
    ) {
        let resp = router()
            .with_state(pool.clone())
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!("/corporate_actions/{action_id}/participate"))
                    .header("content-type", "application/json")
                    .body(Body::from(body.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), expected);
    }

    #[tokio::test]
    async fn api_invalid_participations_return_404_or_422() {
        let pool = test_pool().await;
        insert_listing(&pool, 1).await;
        insert_buy(&pool, 1, d(2020, 1, 15), "10000", "6.00").await;
        insert_buyback(&pool, 10, d(2024, 7, 1)).await;

        let ok = serde_json::json!({
            "date": "2024-07-10",
            "units": "1000",
            "allocations": [ { "purchase_trade_id": 1, "quantity_allocated": "1000" } ],
        });
        api_participate_expecting(&pool, 999, ok.clone(), StatusCode::NOT_FOUND).await;
        for invalid in [
            // Units that don't match the allocations.
            serde_json::json!({
                "date": "2024-07-10",
                "units": "1000",
                "allocations": [ { "purchase_trade_id": 1, "quantity_allocated": "600" } ],
            }),
            // Dated before the buy-back.
            serde_json::json!({
                "date": "2024-06-30",
                "units": "1000",
                "allocations": [ { "purchase_trade_id": 1, "quantity_allocated": "1000" } ],
            }),
            // Non-positive units.
            serde_json::json!({ "date": "2024-07-10", "units": "0", "allocations": [] }),
        ] {
            api_participate_expecting(&pool, 10, invalid, StatusCode::UNPROCESSABLE_ENTITY).await;
        }

        // After a valid participation: PUT /sells on the trade → 422; PUT and
        // DELETE /income on the dividend row → 422.
        api_participate_expecting(&pool, 10, ok, StatusCode::CREATED).await;
        let trade_id: i64 =
            sqlx::query_scalar("SELECT id FROM trades WHERE buyback_action_id = 10")
                .fetch_one(&pool)
                .await
                .unwrap();
        let income_id: i64 =
            sqlx::query_scalar("SELECT id FROM income WHERE buyback_trade_id = ?")
                .bind(trade_id)
                .fetch_one(&pool)
                .await
                .unwrap();
        let app = crate::entities::router().with_state(pool.clone());
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("PUT")
                    .uri(format!("/sells/{trade_id}"))
                    .header("content-type", "application/json")
                    .body(Body::from(
                        serde_json::json!({
                            "date": "2024-07-10", "listing_id": 1,
                            "average_price": "99", "quantity": "1000",
                            "currency": "AUD", "brokerage": "0",
                            "gst_on_brokerage": "0", "brokerage_currency": "AUD",
                            "fx_rate": "1",
                            "allocations": [
                                { "purchase_trade_id": 1, "quantity_allocated": "1000" }
                            ],
                        })
                        .to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY);
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("PUT")
                    .uri(format!("/income/{income_id}"))
                    .header("content-type", "application/json")
                    .body(Body::from(
                        serde_json::json!({
                            "listing_id": 1, "date_paid": "2024-07-10",
                            "franked_amount": "9999",
                        })
                        .to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY);
        let resp = app
            .oneshot(
                Request::builder()
                    .method("DELETE")
                    .uri(format!("/income/{income_id}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY);
    }
}
