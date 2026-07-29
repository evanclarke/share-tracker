//! Atomic demerger: apportion every open parcel of a `Demerger` action's
//! head-entity listing between the head entity and the demerged entity,
//! applying the rollover (Div 125; see `docs/ato/demergers.md`).
//!
//! Under an eligible demerger with rollover chosen, any capital gain or loss
//! is disregarded, the cost base of each original parcel is spread over the
//! remaining head interests and the new demerged-entity interests by the
//! head-entity-advised percentages, the head interests' acquisition dates are
//! unchanged, and the new interests' 12-month discount clock runs from the
//! date the corresponding original interests were acquired (the ATO's
//! Example 32). The demerge therefore creates, in one transaction:
//!
//! - a **closing Sell** on the head listing dated the demerger date — price
//!   0, with parcel allocations consuming every open parcel, written through
//!   the shared `/sells` core so all its invariants hold. It carries
//!   `demerger_action_id`, which excludes it from the realised-gains and
//!   net-capital-gain reports (with rollover no gain or loss is recognised;
//!   the zero proceeds must never surface as a loss) — and per consumed
//!   parcel:
//! - a **head replacement Buy** on the same listing for the parcel's
//!   remaining units, carrying `(100 − demerger_cost_base_pct)%` of the
//!   parcel's remaining reduced cost base, and
//! - a **demerged-entity Buy** on `demerger_listing_id` for those units ×
//!   the entitlement ratio, carrying the other `demerger_cost_base_pct`%.
//!
//! Both Buys are dated the demerger date (so later splits and returns of
//! capital on either listing apply only from then), carry the cost base on
//! the `brokerage` column with a zero price — numerically part of the single
//! cost base everywhere, with no division — and carry the parcel's
//! acquisition date (chaining through any earlier rollover) as
//! `deemed_acquisition_date`, which drives the discount clock and the AUD
//! translation month of the cost base in the reports. The trade's `currency`
//! and manual `fx_rate` fallback also carry over, so a non-AUD parcel's AUD
//! cost base is unchanged by the demerger. The two cost-base legs are
//! computed as `demerged = cost × pct / 100` and `head = cost − demerged`,
//! so they always sum exactly to the original.
//!
//! The created trades form the demerger group (`trades.demerger_action_id`):
//! each is immutable via `PUT /sells` / `PUT /trades` and protected from
//! individual deletion via `DELETE /trades`; `DELETE /sells` on the closing
//! Sell removes the whole group (refused while a replacement Buy is consumed
//! by later allocations or AMIT adjustments); and the action is frozen
//! against edits and deletes while the group exists.
//!
//! The head shares are never actually disposed of in a demerger, so the
//! closing Sell and head replacement Buys are excluded from the
//! franking-credit 45-day walk (`reports::franking`) — the original parcels
//! keep their at-risk days running there.
//!
//! Out of scope (documented in `docs/ato/demergers.md`): demergers without
//! rollover, pre-CGT original interests, assessable demerger dividends or
//! separate capital returns, and registry cash-in-lieu of fractional
//! entitlements (the demerge keeps exact fractional unit counts).

use crate::domain::rollover;
use crate::entities::corporate_action::{self, ActionKind};
use crate::entities::sell::{self, AllocationInput};
use crate::entities::trade::{self, Trade};
use crate::infra::http::ApiError;
use axum::{
    Json, Router,
    extract::{Path, State},
    http::StatusCode,
    routing::post,
};
use chrono::NaiveDate;
use rust_decimal::Decimal;
use serde::Serialize;
use sqlx::SqlitePool;

/// The sides of a demerge: the closing Sell on the head listing, the head
/// replacement Buys, and the demerged-entity Buys (the latter two one per
/// consumed parcel, in the original parcels' date order, pairwise matching).
#[derive(Debug, Serialize)]
pub struct Demerge {
    pub sell: Trade,
    pub head_replacements: Vec<Trade>,
    pub demerged_replacements: Vec<Trade>,
}

#[derive(thiserror::Error, Debug)]
pub enum DemergeError {
    #[error("demerge write failed: {0}")]
    Db(#[from] sqlx::Error),
    /// No corporate action with that id.
    #[error("no corporate action with that id")]
    ActionNotFound,
    /// The action is not a Demerger.
    #[error("that corporate action is not a demerger")]
    NotADemerger,
    /// The action has already been demerged (trades reference it). Delete
    /// the closing Sell via `DELETE /sells` first to redo it.
    #[error("this demerger has already been applied")]
    AlreadyDemerged,
    /// Nothing of the head listing is held at the demerger date — there is
    /// nothing to apportion.
    #[error("nothing of the head listing is held at the demerger date")]
    NothingHeld,
    /// The head listing has a trade dated on or after the demerger date.
    /// The demerge closes and recreates every open parcel as at that date, so
    /// later-dated activity must be entered after demerging, not before.
    #[error("the head listing has a trade dated on or after the demerger date")]
    TradedOnOrAfterDemergerDate,
    /// The Sell-side invariants failed (defensive: the demerge constructs
    /// its allocations to satisfy them).
    #[error("the demerger's closing Sell was rejected: {0}")]
    Sell(#[source] sell::SellError),
}

impl From<sell::SellError> for DemergeError {
    fn from(e: sell::SellError) -> Self {
        match e {
            sell::SellError::Db(err) => DemergeError::Db(err),
            other => DemergeError::Sell(other),
        }
    }
}

pub fn router() -> Router<SqlitePool> {
    Router::new().route("/corporate_actions/{id}/demerge", post(demerge))
}

/// Apportion every open parcel of the action's head listing between the head
/// entity and the demerged entity, atomically. The demerge takes no
/// parameters: the action's terms and the holdings at its date determine
/// everything.
pub async fn db_demerge(pool: &SqlitePool, action_id: i64) -> Result<Demerge, DemergeError> {
    let mut tx = pool.begin().await?;

    let action = match corporate_action::db_get_tx(&mut *tx, action_id).await? {
        Some(a) => a,
        None => return Err(DemergeError::ActionNotFound),
    };
    let ActionKind::Demerger {
        demerger_listing_id,
        demerger_new_units: new_units,
        demerger_held_units: held_units,
        demerger_cost_base_pct: cost_base_pct,
    } = action.kind
    else {
        return Err(DemergeError::NotADemerger);
    };

    check_demergeable(&mut tx, action_id, action.listing_id, action.date).await?;

    // The head listing's open parcels, costed by the shared rollover
    // machinery (as-acquired units internally; allocations re-based across
    // splits). The ATO's step 1 takes the cost base immediately before the
    // demerger, so it is bounded at the demerger date.
    let inputs = rollover::CostBaseInputs::load(&mut tx, action.listing_id).await?;
    let parcels = inputs
        .open_parcels(&mut tx, action.listing_id, action.date)
        .await?;

    let mut replacements: Vec<Replacement> = Vec::with_capacity(parcels.len());
    for p in &parcels {
        let carried_cost_base = inputs.carried_cost_base(&p.parcel, p.remaining, action.date)?;

        // Step 2: apportion by the advised percentage. demerged + head sum
        // exactly to the carried cost base by construction.
        let demerged_cost_base = carried_cost_base * cost_base_pct / Decimal::ONE_HUNDRED;

        replacements.push(Replacement {
            parcel_id: p.parcel.id,
            at_date_units: p.at_date_units,
            // The entitlement ratio applies to units as held at the demerger
            // date.
            demerged_quantity: p.at_date_units * new_units / held_units,
            head_cost_base: carried_cost_base - demerged_cost_base,
            demerged_cost_base,
            currency: p.parcel.currency.clone(),
            fx_rate: p.parcel.fx_rate,
            spot_fx_rate: p.parcel.spot_fx_rate,
            // Chain through an earlier rollover: the clock always runs from
            // the first acquisition in the chain — the parcel's own
            // `acquired()`, deemed date where it carries one.
            deemed_acquisition_date: p.parcel.acquired(),
            holding_account_id: p.parcel.holding_account_id,
        });
    }
    if replacements.is_empty() {
        return Err(DemergeError::NothingHeld);
    }

    // The closing Sell: zero proceeds (the rollover disregards any gain and
    // this Sell never reaches the realised-gains report), consuming every
    // open parcel.
    let listing_currency: String = sqlx::query_scalar("SELECT currency FROM listings WHERE id = ?")
        .bind(action.listing_id)
        .fetch_one(&mut *tx)
        .await?;
    let sell_id: i64 = sqlx::query_scalar("SELECT COALESCE(MAX(id), 0) + 1 FROM trades")
        .fetch_one(&mut *tx)
        .await?;
    let sell_body = rollover::closing_sell_body(
        action.date,
        action.listing_id,
        1,
        Decimal::ZERO,
        listing_currency,
        Decimal::ONE,
        replacements
            .iter()
            .map(|r| AllocationInput {
                purchase_trade_id: r.parcel_id,
                quantity_allocated: r.at_date_units,
            })
            .collect(),
    );
    sell::upsert_sell_in_tx(
        &mut tx,
        sell_id,
        &sell_body,
        action.date,
        None,
        None,
        Some(action_id),
        None,
        None,
    )
    .await?;

    // The replacement Buys: per consumed parcel, the head replacement (same
    // listing, same units, head share of the cost base) and the
    // demerged-entity Buy (entitlement units, demerged share), both dated the
    // demerger date and carrying the parcel's acquisition date.
    let mut head_ids = Vec::with_capacity(replacements.len());
    let mut demerged_ids = Vec::with_capacity(replacements.len());
    for (i, r) in replacements.iter().enumerate() {
        let head_id = sell_id + 1 + (2 * i) as i64;
        let demerged_id = head_id + 1;
        for (buy_id, listing_id, quantity, cost_base) in [
            (
                head_id,
                action.listing_id,
                r.at_date_units,
                r.head_cost_base,
            ),
            (
                demerged_id,
                demerger_listing_id,
                r.demerged_quantity,
                r.demerged_cost_base,
            ),
        ] {
            rollover::insert_replacement_buy(
                &mut tx,
                &rollover::ReplacementBuy {
                    id: buy_id,
                    date: action.date,
                    listing_id,
                    quantity,
                    cost_base,
                    currency: &r.currency,
                    fx_rate: r.fx_rate,
                    spot_fx_rate: r.spot_fx_rate,
                    deemed_acquisition_date: r.deemed_acquisition_date,
                    holding_account_id: r.holding_account_id,
                },
                rollover::Provenance::DemergerAction(action_id),
            )
            .await?;
        }
        head_ids.push(head_id);
        demerged_ids.push(demerged_id);
    }

    tx.commit().await?;

    // Read the freshly created rows back so the response is exactly what was
    // stored.
    let sell = trade::db_get(pool, sell_id)
        .await?
        .ok_or(sqlx::Error::RowNotFound)?;
    Ok(Demerge {
        sell,
        head_replacements: rollover::created_trades(pool, head_ids).await?,
        demerged_replacements: rollover::created_trades(pool, demerged_ids).await?,
    })
}

/// The two Buys to create for one consumed parcel: its demerger-date units
/// stay on the head listing with the head share of its remaining reduced cost
/// base; the entitlement units go to the demerged listing with the rest. Both
/// carry the parcel's acquisition date.
struct Replacement {
    parcel_id: i64,
    at_date_units: Decimal,
    demerged_quantity: Decimal,
    head_cost_base: Decimal,
    demerged_cost_base: Decimal,
    currency: String,
    fx_rate: Decimal,
    spot_fx_rate: Option<Decimal>,
    deemed_acquisition_date: NaiveDate,
    /// Replacement parcels stay in the account of the parcel that produced
    /// them.
    holding_account_id: i64,
}

/// The two write-time checks: the demerge has not already been applied, and
/// no trade of the head listing is dated on or after the demerger date. The
/// demerge closes and recreates the holding as at that date, so a later-dated
/// trade would draw on parcels the closing Sell consumes. (Unlike a takeover,
/// the head listing keeps trading — enter post-demerger activity after
/// demerging.)
async fn check_demergeable(
    conn: &mut sqlx::SqliteConnection,
    action_id: i64,
    listing_id: i64,
    date: NaiveDate,
) -> Result<(), DemergeError> {
    let already: bool =
        sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM trades WHERE demerger_action_id = ?)")
            .bind(action_id)
            .fetch_one(&mut *conn)
            .await?;
    if already {
        return Err(DemergeError::AlreadyDemerged);
    }

    let late_trade: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM trades WHERE listing_id = ? AND date >= ?)",
    )
    .bind(listing_id)
    .bind(date)
    .fetch_one(&mut *conn)
    .await?;
    if late_trade {
        return Err(DemergeError::TradedOnOrAfterDemergerDate);
    }
    Ok(())
}

async fn demerge(
    State(pool): State<SqlitePool>,
    Path(action_id): Path<i64>,
) -> Result<(StatusCode, Json<Demerge>), ApiError> {
    let demerge = db_demerge(&pool, action_id).await?;
    Ok((StatusCode::CREATED, Json(demerge)))
}

impl From<DemergeError> for ApiError {
    fn from(e: DemergeError) -> Self {
        match e {
            DemergeError::ActionNotFound => ApiError::not_found("no corporate action with that id"),
            DemergeError::NotADemerger => {
                ApiError::unprocessable("that corporate action is not a demerger")
            }
            DemergeError::AlreadyDemerged => ApiError::unprocessable(
                "this demerger has already been applied — delete its closing Sell first to redo \
                 it",
            ),
            DemergeError::NothingHeld => {
                ApiError::unprocessable("nothing of the head listing is held at the demerger date")
            }
            DemergeError::TradedOnOrAfterDemergerDate => ApiError::unprocessable(
                "the head listing has a trade dated on or after the demerger date — \
                 enter later activity after demerging, not before",
            ),
            DemergeError::Sell(err) => {
                tracing::warn!(error = ?err, "demerge rejected by a sell invariant");
                ApiError::unprocessable("the demerger's parcel allocations are invalid")
            }
            DemergeError::Db(err) => err.into(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::entities::sell::SellBody;
    use crate::entities::trade::TradeType;
    use crate::entities::{corporate_action::CorporateAction, listing};
    use crate::test_support::{self, test_pool};
    use axum::{body::Body, http::Request};
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    fn d(y: i32, m: u32, day: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, day).unwrap()
    }

    fn dec(s: &str) -> Decimal {
        s.parse().unwrap()
    }

    async fn insert_listing(pool: &SqlitePool, id: i64, ticker: &str) {
        test_support::listing(id)
            .ticker(ticker)
            .name(ticker)
            .security_type(listing::SecurityType::Share)
            .insert(pool)
            .await;
    }

    async fn insert_buy(
        pool: &SqlitePool,
        id: i64,
        listing_id: i64,
        date: NaiveDate,
        qty: &str,
        price: &str,
    ) {
        test_support::buy(id, listing_id)
            .date(date)
            .settlement(date)
            .qty(qty.parse().unwrap())
            .price(price.parse().unwrap())
            .insert(pool)
            .await;
    }

    /// A 1-for-5 demerger of listing 2 out of listing 1 on the given date,
    /// apportioning 20% of the cost base to the demerged entity.
    async fn insert_demerger(pool: &SqlitePool, id: i64, date: NaiveDate) {
        insert_demerger_terms(pool, id, 1, 2, date, "1", "5", "20").await;
    }

    // Test fixture: flat positional args read fine here; bundling them into a
    // params struct would add ceremony without aiding the tests.
    #[allow(clippy::too_many_arguments)]
    async fn insert_demerger_terms(
        pool: &SqlitePool,
        id: i64,
        listing_id: i64,
        demerger_listing_id: i64,
        date: NaiveDate,
        new: &str,
        held: &str,
        pct: &str,
    ) {
        corporate_action::db_upsert(
            pool,
            &CorporateAction {
                id,
                listing_id,
                date,
                kind: ActionKind::Demerger {
                    demerger_listing_id,
                    demerger_new_units: new.parse().unwrap(),
                    demerger_held_units: held.parse().unwrap(),
                    demerger_cost_base_pct: pct.parse().unwrap(),
                },
            },
        )
        .await
        .unwrap();
    }

    async fn sell_units(
        pool: &SqlitePool,
        sell_id: i64,
        parcel_id: i64,
        date: NaiveDate,
        qty: &str,
    ) {
        sell::db_upsert_sell(
            pool,
            sell_id,
            &SellBody {
                brokerage_includes_gst: false,
                statement_total: None,
                holding_account_id: 1,
                date,
                settlement_date: Some(date),
                listing_id: 1,
                average_price: dec("10"),
                quantity: dec(qty),
                currency: "AUD".to_string(),
                brokerage: Decimal::ZERO,
                gst_on_brokerage: Decimal::ZERO,
                brokerage_currency: "AUD".to_string(),
                fx_rate: Decimal::ONE,
                spot_fx_rate: None,
                contract_note_ref: None,
                allocations: vec![AllocationInput {
                    purchase_trade_id: parcel_id,
                    quantity_allocated: dec(qty),
                }],
            },
        )
        .await
        .unwrap();
    }

    // DB-level tests

    /// The core apportionment: each open parcel is closed by a zero-proceeds
    /// Sell and recreated as a head replacement Buy plus a demerged-entity
    /// Buy, splitting its cost base by the advised percentage with both
    /// carrying the parcel's acquisition date.
    #[tokio::test]
    async fn demerge_apportions_cost_base_and_carries_acquisition_dates() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "HEAD").await;
        insert_listing(&pool, 2, "DEM").await;
        // Two parcels: 1,000 @ $1.50 (2020) and 500 @ $2.00 (2023).
        insert_buy(&pool, 1, 1, d(2020, 10, 1), "1000", "1.50").await;
        insert_buy(&pool, 2, 1, d(2023, 3, 1), "500", "2.00").await;
        insert_demerger(&pool, 10, d(2024, 7, 1)).await;

        let dm = db_demerge(&pool, 10).await.unwrap();

        // The closing Sell: all 1,500 units at zero proceeds on the demerger
        // date, marked with the action's provenance.
        assert_eq!(dm.sell.trade_type, TradeType::Sell);
        assert_eq!(dm.sell.listing_id, 1);
        assert_eq!(dm.sell.date, d(2024, 7, 1));
        assert_eq!(dm.sell.quantity, dec("1500"));
        assert_eq!(dm.sell.average_price, Decimal::ZERO);
        assert_eq!(dm.sell.demerger_action_id, Some(10));

        // Per parcel: the head replacement keeps the units with 80% of the
        // cost base; the demerged Buy takes 1-for-5 units with 20% (price 0 +
        // brokerage = exact); both carry the parcel's acquisition date and
        // are dated at the demerger.
        assert_eq!(dm.head_replacements.len(), 2);
        assert_eq!(dm.demerged_replacements.len(), 2);
        let h1 = &dm.head_replacements[0];
        assert_eq!(h1.trade_type, TradeType::Buy);
        assert_eq!(h1.listing_id, 1);
        assert_eq!(h1.date, d(2024, 7, 1));
        assert_eq!(h1.quantity, dec("1000"));
        assert_eq!(h1.average_price, Decimal::ZERO);
        assert_eq!(h1.brokerage, dec("1200"));
        assert_eq!(h1.deemed_acquisition_date, Some(d(2020, 10, 1)));
        assert_eq!(h1.demerger_action_id, Some(10));
        let d1 = &dm.demerged_replacements[0];
        assert_eq!(d1.listing_id, 2);
        assert_eq!(d1.date, d(2024, 7, 1));
        assert_eq!(d1.quantity, dec("200"));
        assert_eq!(d1.brokerage, dec("300"));
        assert_eq!(d1.deemed_acquisition_date, Some(d(2020, 10, 1)));
        assert_eq!(d1.demerger_action_id, Some(10));
        let h2 = &dm.head_replacements[1];
        assert_eq!(h2.quantity, dec("500"));
        assert_eq!(h2.brokerage, dec("800"));
        assert_eq!(h2.deemed_acquisition_date, Some(d(2023, 3, 1)));
        let d2 = &dm.demerged_replacements[1];
        assert_eq!(d2.quantity, dec("100"));
        assert_eq!(d2.brokerage, dec("200"));
        assert_eq!(d2.deemed_acquisition_date, Some(d(2023, 3, 1)));

        // The allocations consume both parcels exactly.
        let n: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM parcel_allocations WHERE sale_trade_id = ?")
                .bind(dm.sell.id)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(n, 2);
    }

    /// A sub-unit percentage (BHP Steel's 5.063%) splits the cost base with
    /// no rounding: the two legs always sum exactly to the original.
    #[tokio::test]
    async fn apportionment_keeps_the_total_cost_base_exact() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "HEAD").await;
        insert_listing(&pool, 2, "DEM").await;
        insert_buy(
            &pool,
            1,
            1,
            d(2020, 10, 1),
            "280",
            "8.9285714285714285714285714286",
        )
        .await;
        insert_demerger_terms(&pool, 10, 1, 2, d(2024, 7, 1), "1", "5", "5.063").await;

        let dm = db_demerge(&pool, 10).await.unwrap();

        let head = dm.head_replacements[0].brokerage;
        let demerged = dm.demerged_replacements[0].brokerage;
        let original = dec("8.9285714285714285714285714286") * dec("280");
        assert_eq!(demerged, original * dec("5.063") / Decimal::ONE_HUNDRED);
        assert_eq!(head + demerged, original);
        assert_eq!(dm.demerged_replacements[0].quantity, dec("56"));
    }

    /// A partly sold parcel carries only its remaining units' share of the
    /// cost base (incl. brokerage) into the apportionment.
    #[tokio::test]
    async fn partly_sold_parcel_carries_only_the_remaining_cost_base() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "HEAD").await;
        insert_listing(&pool, 2, "DEM").await;
        // 1,000 @ $1.50 + $50 brokerage = $1,550; sell 400 → 600 remain at
        // 60% of the cost base = $930 → head $744 + demerged $186.
        test_support::buy(1, 1)
            .date(d(2020, 10, 1))
            .settlement(d(2020, 10, 1))
            .qty(dec("1000"))
            .price(dec("1.50"))
            .brokerage(dec("50"))
            .insert(&pool)
            .await;
        sell_units(&pool, 2, 1, d(2022, 5, 1), "400").await;
        insert_demerger(&pool, 10, d(2024, 7, 1)).await;

        let dm = db_demerge(&pool, 10).await.unwrap();

        assert_eq!(dm.sell.quantity, dec("600"));
        assert_eq!(dm.head_replacements.len(), 1);
        assert_eq!(dm.head_replacements[0].quantity, dec("600"));
        assert_eq!(dm.head_replacements[0].brokerage, dec("744"));
        assert_eq!(dm.demerged_replacements[0].quantity, dec("120"));
        assert_eq!(dm.demerged_replacements[0].brokerage, dec("186"));
    }

    /// AMIT cost-base reductions and return-of-capital payments received
    /// while held reduce the cost base being apportioned — the ATO's step 1
    /// takes the cost base immediately before the demerger.
    #[tokio::test]
    async fn amit_and_roc_reductions_reduce_the_apportioned_cost_base() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "HEAD").await;
        insert_listing(&pool, 2, "DEM").await;
        insert_buy(&pool, 1, 1, d(2020, 10, 1), "1000", "1.50").await;

        // An AMMA statement with a 10c/unit cost-base decrease over the
        // parcel's 1,000 units → −$100.
        test_support::amma(1, 1)
            .units(dec("1000"))
            .cost_base_adjustment(dec("0.10"))
            .with(|a| {
                a.tax_year_end_date = d(2021, 6, 30);
                a.date_received = d(2021, 7, 15);
            })
            .insert(&pool)
            .await;
        test_support::amit_adjustment(&pool, 1, 1, 1, dec("1000")).await;

        // A 5c/unit return of capital while held → −$50.
        corporate_action::db_upsert(
            &pool,
            &CorporateAction {
                id: 5,
                listing_id: 1,
                date: d(2022, 3, 1),
                kind: ActionKind::ReturnOfCapital {
                    amount_per_unit: dec("0.05"),
                    currency: "AUD".to_string(),
                },
            },
        )
        .await
        .unwrap();

        insert_demerger(&pool, 10, d(2024, 7, 1)).await;
        let dm = db_demerge(&pool, 10).await.unwrap();

        // $1,500 − $100 (AMIT) − $50 (ROC) = $1,350 → head $1,080 + demerged $270.
        assert_eq!(dm.head_replacements[0].brokerage, dec("1080"));
        assert_eq!(dm.demerged_replacements[0].brokerage, dec("270"));
    }

    /// A split on the head listing before the demerger re-bases the units
    /// the entitlement ratio applies to; the cost base is unchanged.
    #[tokio::test]
    async fn split_before_the_demerger_rebases_the_units() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "HEAD").await;
        insert_listing(&pool, 2, "DEM").await;
        insert_buy(&pool, 1, 1, d(2020, 10, 1), "1000", "1.50").await;
        // 3-for-1 split: the 1,000 as-acquired units are 3,000 at the demerger.
        corporate_action::db_upsert(
            &pool,
            &CorporateAction {
                id: 5,
                listing_id: 1,
                date: d(2022, 1, 1),
                kind: ActionKind::ShareSplit {
                    split_new_units: dec("3"),
                    split_old_units: dec("1"),
                },
            },
        )
        .await
        .unwrap();
        insert_demerger(&pool, 10, d(2024, 7, 1)).await;

        let dm = db_demerge(&pool, 10).await.unwrap();

        // 3,000 demerger-date units close; the head replacement keeps them
        // (with 80% of the unchanged $1,500 cost base) and 1-for-5 → 600
        // demerged units (with 20%), both keeping the 2020 acquisition.
        assert_eq!(dm.sell.quantity, dec("3000"));
        assert_eq!(dm.head_replacements[0].quantity, dec("3000"));
        assert_eq!(dm.head_replacements[0].brokerage, dec("1200"));
        assert_eq!(
            dm.head_replacements[0].deemed_acquisition_date,
            Some(d(2020, 10, 1))
        );
        assert_eq!(dm.demerged_replacements[0].quantity, dec("600"));
        assert_eq!(dm.demerged_replacements[0].brokerage, dec("300"));
        assert_eq!(
            dm.demerged_replacements[0].deemed_acquisition_date,
            Some(d(2020, 10, 1))
        );
    }

    #[tokio::test]
    async fn invalid_demerges_are_rejected_and_nothing_persisted() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "HEAD").await;
        insert_listing(&pool, 2, "DEM").await;

        // Missing action.
        assert!(matches!(
            db_demerge(&pool, 99).await,
            Err(DemergeError::ActionNotFound)
        ));

        // Not a Demerger.
        corporate_action::db_upsert(
            &pool,
            &CorporateAction {
                id: 1,
                listing_id: 1,
                date: d(2024, 7, 1),
                kind: ActionKind::ShareSplit {
                    split_new_units: dec("2"),
                    split_old_units: dec("1"),
                },
            },
        )
        .await
        .unwrap();
        assert!(matches!(
            db_demerge(&pool, 1).await,
            Err(DemergeError::NotADemerger)
        ));

        // Nothing held at the demerger date.
        insert_demerger(&pool, 10, d(2024, 7, 1)).await;
        assert!(matches!(
            db_demerge(&pool, 10).await,
            Err(DemergeError::NothingHeld)
        ));

        // A head-listing trade dated on/after the demerger date would draw on
        // parcels the closing Sell consumes — enter it after demerging.
        insert_buy(&pool, 1, 1, d(2024, 7, 1), "100", "1.50").await;
        assert!(matches!(
            db_demerge(&pool, 10).await,
            Err(DemergeError::TradedOnOrAfterDemergerDate)
        ));

        // Nothing was persisted by any of the rejections.
        let trades: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM trades WHERE demerger_action_id IS NOT NULL")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(trades, 0);
    }

    #[tokio::test]
    async fn a_second_demerge_of_the_same_action_is_rejected() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "HEAD").await;
        insert_listing(&pool, 2, "DEM").await;
        insert_buy(&pool, 1, 1, d(2020, 10, 1), "1000", "1.50").await;
        insert_demerger(&pool, 10, d(2024, 7, 1)).await;
        db_demerge(&pool, 10).await.unwrap();

        assert!(matches!(
            db_demerge(&pool, 10).await,
            Err(DemergeError::AlreadyDemerged)
        ));
    }

    /// The group is immutable trade by trade: the closing Sell rejects
    /// `PUT /sells`, the head and demerged Buys reject `PUT /trades`, and
    /// none can be deleted individually via `DELETE /trades`.
    #[tokio::test]
    async fn demerge_trades_are_immutable_individually() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "HEAD").await;
        insert_listing(&pool, 2, "DEM").await;
        insert_buy(&pool, 1, 1, d(2020, 10, 1), "1000", "1.50").await;
        insert_demerger(&pool, 10, d(2024, 7, 1)).await;
        let dm = db_demerge(&pool, 10).await.unwrap();

        // PUT /sells on the closing Sell → rejected.
        let err = sell::db_upsert_sell(
            &pool,
            dm.sell.id,
            &SellBody {
                brokerage_includes_gst: false,
                statement_total: None,
                holding_account_id: 1,
                date: d(2024, 7, 1),
                settlement_date: Some(d(2024, 7, 1)),
                listing_id: 1,
                average_price: dec("9.99"),
                quantity: dec("1000"),
                currency: "AUD".to_string(),
                brokerage: Decimal::ZERO,
                gst_on_brokerage: Decimal::ZERO,
                brokerage_currency: "AUD".to_string(),
                fx_rate: Decimal::ONE,
                spot_fx_rate: None,
                contract_note_ref: None,
                allocations: vec![AllocationInput {
                    purchase_trade_id: 1,
                    quantity_allocated: dec("1000"),
                }],
            },
        )
        .await;
        assert!(matches!(err, Err(sell::SellError::DemergerSell)));

        // PUT /trades on either Buy → rejected.
        for replacement in [&dm.head_replacements[0], &dm.demerged_replacements[0]] {
            let mut edited = replacement.clone();
            edited.quantity = dec("9999");
            assert!(matches!(
                trade::db_upsert(&pool, &edited).await,
                Err(trade::UpsertError::DemergerTrade)
            ));
        }

        // DELETE /trades on any group trade → refused.
        for id in [
            dm.head_replacements[0].id,
            dm.demerged_replacements[0].id,
            dm.sell.id,
        ] {
            assert_eq!(
                trade::db_delete(&pool, id).await.unwrap(),
                trade::DeleteOutcome::Referenced
            );
        }
    }

    /// `DELETE /sells` on the closing Sell removes the whole group and
    /// restores the pre-demerger holding; it is refused while a replacement
    /// Buy is consumed by a later sale.
    #[tokio::test]
    async fn deleting_the_closing_sell_removes_the_whole_group() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "HEAD").await;
        insert_listing(&pool, 2, "DEM").await;
        insert_buy(&pool, 1, 1, d(2020, 10, 1), "1000", "1.50").await;
        insert_demerger(&pool, 10, d(2024, 7, 1)).await;
        let dm = db_demerge(&pool, 10).await.unwrap();

        // A later sale out of the demerged parcel blocks the group delete.
        sell::db_upsert_sell(
            &pool,
            50,
            &SellBody {
                brokerage_includes_gst: false,
                statement_total: None,
                holding_account_id: 1,
                date: d(2025, 1, 10),
                settlement_date: Some(d(2025, 1, 12)),
                listing_id: 2,
                average_price: dec("3.00"),
                quantity: dec("100"),
                currency: "AUD".to_string(),
                brokerage: Decimal::ZERO,
                gst_on_brokerage: Decimal::ZERO,
                brokerage_currency: "AUD".to_string(),
                fx_rate: Decimal::ONE,
                spot_fx_rate: None,
                contract_note_ref: None,
                allocations: vec![AllocationInput {
                    purchase_trade_id: dm.demerged_replacements[0].id,
                    quantity_allocated: dec("100"),
                }],
            },
        )
        .await
        .unwrap();
        assert_eq!(
            sell::db_delete_sell(&pool, dm.sell.id).await.unwrap(),
            sell::DeleteOutcome::ReplacementReferenced
        );

        // Remove the later sale; the group then deletes as a whole and the
        // action thaws (it can be deleted again).
        assert_eq!(
            sell::db_delete_sell(&pool, 50).await.unwrap(),
            sell::DeleteOutcome::Deleted
        );
        assert_eq!(
            sell::db_delete_sell(&pool, dm.sell.id).await.unwrap(),
            sell::DeleteOutcome::Deleted
        );
        let remaining: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM trades WHERE demerger_action_id IS NOT NULL")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(remaining, 0);
        // The original parcel is open again.
        let allocs: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM parcel_allocations")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(allocs, 0);
        assert!(corporate_action::db_delete(&pool, 10).await.unwrap());
    }

    /// The action is frozen while its demerger group exists.
    #[tokio::test]
    async fn referenced_action_cannot_be_edited_or_deleted() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "HEAD").await;
        insert_listing(&pool, 2, "DEM").await;
        insert_buy(&pool, 1, 1, d(2020, 10, 1), "1000", "1.50").await;
        insert_demerger(&pool, 10, d(2024, 7, 1)).await;
        db_demerge(&pool, 10).await.unwrap();

        let action = corporate_action::db_get(&pool, 10).await.unwrap().unwrap();
        assert!(matches!(
            corporate_action::db_upsert(&pool, &action).await,
            Err(corporate_action::WriteError::ReferencedByTrade)
        ));
        assert!(corporate_action::db_delete(&pool, 10).await.is_err());
    }

    /// The demerge spans every holding account; the head replacement and
    /// demerged-entity Buys stay in the account of the parcel that produced
    /// them.
    #[tokio::test]
    async fn replacements_stay_in_each_parcels_holding_account() {
        use crate::entities::holding_account::{self, HoldingAccount};
        let pool = test_pool().await;
        insert_listing(&pool, 1, "HEAD").await;
        insert_listing(&pool, 2, "DEMERGED").await;
        holding_account::db_upsert(
            &pool,
            &HoldingAccount {
                id: 2,
                name: "ICE Employee Plan".to_string(),
            },
        )
        .await
        .unwrap();
        insert_buy(&pool, 1, 1, d(2020, 10, 1), "1000", "1.50").await;
        insert_buy(&pool, 2, 1, d(2023, 3, 1), "500", "2.00").await;
        sqlx::query("UPDATE trades SET holding_account_id = 2 WHERE id = 2")
            .execute(&pool)
            .await
            .unwrap();
        insert_demerger(&pool, 10, d(2024, 7, 1)).await;

        let dm = db_demerge(&pool, 10).await.unwrap();

        assert_eq!(dm.head_replacements[0].holding_account_id, 1);
        assert_eq!(dm.demerged_replacements[0].holding_account_id, 1);
        assert_eq!(dm.head_replacements[1].holding_account_id, 2);
        assert_eq!(dm.demerged_replacements[1].holding_account_id, 2);
    }

    // API-level tests

    #[tokio::test]
    async fn api_demerge_creates_the_group() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "HEAD").await;
        insert_listing(&pool, 2, "DEM").await;
        insert_buy(&pool, 1, 1, d(2020, 10, 1), "1000", "1.50").await;
        insert_demerger(&pool, 10, d(2024, 7, 1)).await;

        let resp = router()
            .with_state(pool.clone())
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/corporate_actions/10/demerge")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::CREATED);
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let v: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(v["sell"]["quantity"], "1000");
        assert_eq!(v["head_replacements"][0]["quantity"], "1000");
        // 1500 × 20 / 100 = 300.00 (the division's scale), head = the rest.
        assert_eq!(v["head_replacements"][0]["brokerage"], "1200.00");
        assert_eq!(v["demerged_replacements"][0]["quantity"], "200");
        assert_eq!(v["demerged_replacements"][0]["brokerage"], "300.00");
        assert_eq!(
            v["demerged_replacements"][0]["deemed_acquisition_date"],
            "2020-10-01"
        );
    }

    #[tokio::test]
    async fn api_demerge_maps_errors_to_statuses() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "HEAD").await;
        insert_listing(&pool, 2, "DEM").await;

        // Missing action → 404.
        let resp = router()
            .with_state(pool.clone())
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/corporate_actions/99/demerge")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);

        // Nothing held → 422.
        insert_demerger(&pool, 10, d(2024, 7, 1)).await;
        let resp = router()
            .with_state(pool.clone())
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/corporate_actions/10/demerge")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY);
    }

    /// A consumed parcel's deliberate spot-rate override carries onto both
    /// the head and demerged replacement Buys (like `fx_rate` and the deemed
    /// acquisition date), so the apportioned AUD cost bases are unchanged by
    /// the demerger.
    #[tokio::test]
    async fn demerge_carries_spot_fx_rate_onto_replacements() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "HEAD").await;
        insert_listing(&pool, 2, "DEM").await;
        test_support::buy(1, 1)
            .date(d(2020, 10, 1))
            .settlement(d(2020, 10, 1))
            .qty(dec("1000"))
            .price(dec("1.50"))
            .currency("USD")
            .fx_rate(dec("0.70"))
            .spot_fx_rate(dec("0.6543"))
            .insert(&pool)
            .await;
        insert_demerger(&pool, 10, d(2024, 7, 1)).await;

        let dm = db_demerge(&pool, 10).await.unwrap();
        for t in dm.head_replacements.iter().chain(&dm.demerged_replacements) {
            assert_eq!(t.fx_rate, dec("0.70"));
            assert_eq!(t.spot_fx_rate, Some(dec("0.6543")));
        }
    }
}
