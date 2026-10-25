//! Shared fixtures for the corporate-action tests, and the topic modules over
//! them. Moved verbatim out of the entity's single inline `mod tests`; what
//! lives here is what more than one topic uses, and a fixture only one topic
//! calls lives in that topic's file.

mod amit;
mod create;
mod currency;
mod delete_guard;
mod edit_guard;
mod events;
mod payloads;
mod representability;
mod rounding;
mod storage;

use super::*;
use crate::domain::adjustments::{RolloverOrigin, SplitRatio};
use crate::domain::open_parcels;
use crate::entities::corporate_action::FractionRule;
use crate::entities::sell::{AllocationInput, SellBody, SellError};
use crate::entities::transfer::TransferBody;
use crate::entities::{holding_account, listing, sell, transfer};
use crate::reports;
use crate::reports::realised_gains;
use crate::test_support::{self, ApiClient, test_pool};

/// Client over this module's own routes.
fn client(pool: &SqlitePool) -> ApiClient {
    ApiClient::over(router().with_state(pool.clone()))
}

async fn insert_listing(pool: &SqlitePool, id: i64, ticker: &str) {
    test_support::listing(id)
        .ticker(ticker)
        .name(ticker)
        .security_type(listing::SecurityType::Share)
        .insert(pool)
        .await;
}

fn roc(id: i64, listing_id: ListingId, date: NaiveDate, amount: &str) -> CorporateAction {
    CorporateAction {
        id,
        listing_id,
        date,
        kind: ActionKind::ReturnOfCapital {
            amount_per_unit: amount.parse().unwrap(),
            currency: "AUD".into(),
            record_date: None,
        },
    }
}

/// [`roc`] carrying the record date that fixed entitlement to it.
fn roc_with_record(
    id: i64,
    listing_id: ListingId,
    date: NaiveDate,
    amount: &str,
    record: NaiveDate,
) -> CorporateAction {
    CorporateAction {
        kind: ActionKind::ReturnOfCapital {
            amount_per_unit: amount.parse().unwrap(),
            currency: "AUD".into(),
            record_date: Some(record),
        },
        ..roc(id, listing_id, date, amount)
    }
}

fn split(id: i64, listing_id: ListingId, date: NaiveDate, new: &str, old: &str) -> CorporateAction {
    CorporateAction {
        id,
        listing_id,
        date,
        kind: ActionKind::ShareSplit {
            fraction_rule: FractionRule::Exact,
            split_new_units: new.parse().unwrap(),
            split_old_units: old.parse().unwrap(),
            record_date: None,
        },
    }
}

fn bonus(
    id: i64,
    listing_id: ListingId,
    date: NaiveDate,
    units: &str,
    held: &str,
) -> CorporateAction {
    CorporateAction {
        id,
        listing_id,
        date,
        kind: ActionKind::BonusIssue {
            fraction_rule: FractionRule::Exact,
            bonus_units: units.parse().unwrap(),
            bonus_held_units: held.parse().unwrap(),
            record_date: None,
        },
    }
}

fn rights(
    id: i64,
    listing_id: ListingId,
    date: NaiveDate,
    units: &str,
    held: &str,
    price: &str,
) -> CorporateAction {
    CorporateAction {
        id,
        listing_id,
        date,
        kind: ActionKind::RightsIssue {
            rights_units: units.parse().unwrap(),
            rights_held_units: held.parse().unwrap(),
            exercise_price: price.parse().unwrap(),
            currency: "AUD".into(),
            renounceable: true,
        },
    }
}

fn scrip(
    id: i64,
    listing_id: ListingId,
    scrip_listing_id: ListingId,
    date: NaiveDate,
    new: &str,
    old: &str,
) -> CorporateAction {
    CorporateAction {
        id,
        listing_id,
        date,
        kind: ActionKind::ScripForScrip {
            fraction_rule: FractionRule::Exact,
            scrip_listing_id,
            scrip_new_units: new.parse().unwrap(),
            scrip_old_units: old.parse().unwrap(),
            scrip_cash_per_unit: None,
            scrip_market_value: None,
            scrip_cash_currency: None,
            record_date: None,
        },
    }
}

fn demerger(
    id: i64,
    listing_id: ListingId,
    demerger_listing_id: ListingId,
    date: NaiveDate,
    new: &str,
    held: &str,
    pct: &str,
) -> CorporateAction {
    CorporateAction {
        id,
        listing_id,
        date,
        kind: ActionKind::Demerger {
            fraction_rule: FractionRule::Exact,
            demerger_listing_id,
            demerger_new_units: new.parse().unwrap(),
            demerger_held_units: held.parse().unwrap(),
            demerger_cost_base_pct: pct.parse().unwrap(),
            demerger_close_date: None,
            demerger_close_price: None,
            demerger_close_sourced_from: None,
            demerger_close_reason: None,
            record_date: None,
        },
    }
}

fn split_event(date: NaiveDate, new: &str, old: &str) -> SplitEvent {
    SplitEvent::share_split(date, new.parse().unwrap(), old.parse().unwrap())
}

fn d(y: i32, m: u32, day: u32) -> NaiveDate {
    NaiveDate::from_ymd_opt(y, m, day).unwrap()
}

async fn api_put_expecting(pool: &SqlitePool, body: serde_json::Value, expected: StatusCode) {
    let resp = client(pool).put("/corporate_actions/1", &body).await;
    assert_eq!(resp.status, expected);
}

/// The parcel the guarded actions below act on: 100 units @ $10 on
/// 2023-01-10.
async fn insert_parcel(pool: &SqlitePool) {
    test_support::buy(1, 1)
        .date(d(2023, 1, 10))
        .insert(pool)
        .await;
}

/// A Sell of `qty` units, entered on the unit basis in force at `date`,
/// allocated wholly against parcel 1 — the write path validates the
/// allocation against the parcel's re-based capacity.
async fn insert_sell(pool: &SqlitePool, date: NaiveDate, qty: &str) -> Result<Upsert, SellError> {
    sell::db_upsert_sell(
        pool,
        TradeId(2),
        &SellBody {
            brokerage_includes_gst: false,
            statement_total: None,
            holding_account_id: HoldingAccountId(1),
            date,
            settlement_date: Some(date),
            listing_id: ListingId(1),
            average_price: "6".parse().unwrap(),
            quantity: qty.parse().unwrap(),
            currency: "AUD".into(),
            brokerage: Decimal::ZERO,
            gst_on_brokerage: Decimal::ZERO,
            brokerage_currency: "AUD".into(),
            fx_rate: Decimal::ONE,
            spot_fx_rate: None,
            contract_note_ref: None,
            allocations: vec![AllocationInput {
                purchase_trade_id: TradeId(1),
                quantity_allocated: qty.parse().unwrap(),
            }],
        },
    )
    .await
    .map(|upserted| upserted.outcome)
}
