//! Shared fixtures for the trade tests, and the topic modules over them.
//! Moved verbatim out of the entity's single inline `mod tests`; what lives
//! here is what more than one topic uses, and a fixture only one topic calls
//! lives in that topic's file.

mod back_dated;
mod create;
mod crud;
mod integrity;
mod representability;
mod round_trip;
mod settlement_dates;
mod spot_fx;
mod trade_type;
mod validation;

use super::*;
use crate::entities::corporate_action::{ActionKind, CorporateAction};
use crate::entities::{closing_price, corporate_action, demerger, holding_account, scrip_exchange};
use crate::infra;
use crate::infra::date;
use crate::test_support::{self, ApiClient, dec, test_pool, ymd};
use rust_decimal::Decimal;
use std::collections::HashSet;

/// Client over this module's own routes.
fn client(pool: &SqlitePool) -> ApiClient {
    ApiClient::over(router().with_state(pool.clone()))
}

async fn insert_test_listing(pool: &SqlitePool) {
    test_support::listing(1)
        .ticker("VAS")
        .name("Vanguard Australian Shares ETF")
        .insert(pool)
        .await;
}

/// A USD-quoted listing, for the cases about a foreign trade: a trade is
/// recorded in its listing's currency (`UpsertError::CurrencyNotListings`).
async fn insert_usd_listing(pool: &SqlitePool) {
    test_support::listing(1)
        .mic("XNYS")
        .ticker("VTS")
        .name("Vanguard US Total Market Shares Index ETF")
        .currency("USD")
        .insert(pool)
        .await;
}

fn buy_trade() -> Trade {
    test_support::buy(1, 1)
        .date(ymd(2024, 1, 15))
        .qty(Decimal::from(10))
        .price(Decimal::from(100))
        .brokerage(dec("9.95"))
        .gst_on_brokerage(dec("0.995"))
        .with(|t| t.contract_note_ref = Some("CN001".to_owned()))
        .build()
}

/// Sell `qty` units out of the Buy parcel `buy_id` (listing 1), via the
/// atomic Sell + allocation path.
async fn insert_sell_consuming(pool: &SqlitePool, sell_id: TradeId, buy_id: TradeId, qty: Decimal) {
    use crate::entities::sell;
    sell::db_upsert_sell(
        pool,
        sell_id,
        &sell::SellBody {
            brokerage_includes_gst: false,
            statement_total: None,
            holding_account_id: HoldingAccountId(1),
            date: NaiveDate::from_ymd_opt(2024, 6, 3).unwrap(),
            settlement_date: Some(NaiveDate::from_ymd_opt(2024, 6, 3).unwrap()),
            listing_id: ListingId(1),
            average_price: Decimal::from(120),
            quantity: qty,
            currency: "AUD".into(),
            brokerage: Decimal::ZERO,
            gst_on_brokerage: Decimal::ZERO,
            brokerage_currency: "AUD".into(),
            fx_rate: Decimal::ONE,
            spot_fx_rate: None,
            contract_note_ref: None,
            allocations: vec![sell::AllocationInput {
                purchase_trade_id: buy_id,
                quantity_allocated: qty,
            }],
        },
    )
    .await
    .unwrap();
}

/// A Buy body with `settlement_date` omitted, so the server computes it —
/// the only way a trade's settlement date is recorded as `computed`, and
/// so the only shape the `settlement-recompute` job will ever rewrite.
fn auto_settled_buy(date: NaiveDate) -> serde_json::Value {
    serde_json::json!({
        "trade_type": "Buy",
        "date": date,
        "listing_id": 1,
        "average_price": "100.0",
        "quantity": "10.0",
        "currency": "AUD",
        "brokerage": "0",
        "gst_on_brokerage": "0",
        "brokerage_currency": "AUD",
        "fx_rate": "1.0"
    })
}

/// PUT the JSON body to /trades/{id}, returning the status and response
/// body text (the statement-total 422 carries its detail there).
async fn put_trade_json(
    pool: &SqlitePool,
    id: i64,
    body: serde_json::Value,
) -> (StatusCode, String) {
    let resp = client(pool).put(format!("/trades/{id}"), &body).await;
    let status = resp.status;
    (status, resp.text().to_owned())
}
