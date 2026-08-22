//! Atomic scrip-for-scrip exchange: substitute every open parcel of a
//! `ScripForScrip` action's original (target) listing with parcels of the
//! replacement listing, applying the rollover (Subdiv 124-M; see
//! `docs/ato/takeovers-and-scrip-for-scrip.md`).
//!
//! The rollover disregards the capital gain on the original shares and deems
//! the replacement shares acquired *for the cost base of the original
//! interest*, with the combined holding period counting toward the 12-month
//! CGT discount. With a cash component (the partial rollover, Example 27),
//! the rollover applies only to the scrip portion: each parcel's remaining
//! reduced cost base is apportioned between cash and scrip by the
//! consideration's market values — cash×old / (cash×old + mv×new) to the
//! cash side — and the cash side's gain is assessed now. The exchange
//! creates, in one transaction:
//!
//! - a **closing Sell** on the original listing dated the exchange date —
//!   priced at the per-old-unit cash component (0 when all-scrip), with
//!   parcel allocations consuming every open parcel, written through the
//!   shared `/sells` core so all its invariants hold. It carries
//!   `scrip_action_id`: when all-scrip that excludes it from the
//!   realised-gains and net-capital-gain reports (the disposal happens, but
//!   its gain is disregarded; the zero proceeds never surface as a loss);
//!   with cash those reports assess it against the cash-apportioned share of
//!   each parcel's reduced cost base, discount-classified by the parcel's
//!   original (or deemed) acquisition date, and
//! - one **replacement Buy** per consumed parcel on the replacement listing,
//!   dated the exchange date (so later splits and returns of capital on the
//!   replacement listing apply only from then), with quantity = the parcel's
//!   remaining units at the exchange date × the exchange ratio. The parcel's
//!   remaining reduced cost base (AMIT- and return-of-capital-adjusted,
//!   floored at nil; the scrip-apportioned share of it when there is cash)
//!   is carried on the `brokerage` column with a zero price —
//!   numerically part of the single cost base everywhere, with no division —
//!   and the parcel's acquisition date (chaining through any earlier
//!   exchange) is carried as `deemed_acquisition_date`, which drives the
//!   discount clock and the AUD translation month of the cost base in the
//!   reports. The trade's `currency` and manual `fx_rate` fallback also carry
//!   over, so a non-AUD parcel's AUD cost base is unchanged by the exchange.
//!
//! The created trades form the exchange group (`trades.scrip_action_id`):
//! each is immutable via `PUT /sells` / `PUT /trades` and protected from
//! individual deletion via `DELETE /trades`; `DELETE /sells` on the closing
//! Sell removes the whole group (refused while a replacement Buy is consumed
//! by later allocations or AMIT adjustments); and the action is frozen
//! against edits and deletes while the group exists.
//!
//! Out of scope (documented in `docs/ato/takeovers-and-scrip-for-scrip.md`):
//! takeovers without rollover (an ordinary market-value disposal — enter the
//! Sell and Buy manually; a pure-cash takeover is an ordinary Sell), multiple
//! replacement share classes, pre-CGT originals, and exchanges that would
//! crystallise a capital loss (the law does not allow rolling over a loss).

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

/// The two sides of an exchange: the closing Sell on the original listing
/// and the replacement Buys it was substituted with (one per consumed
/// parcel, in the original parcels' date order).
#[derive(Debug, Serialize)]
pub struct Exchange {
    pub sell: Trade,
    pub replacements: Vec<Trade>,
}

#[derive(thiserror::Error, Debug)]
pub enum ExchangeError {
    #[error("scrip-for-scrip exchange write failed: {0}")]
    Db(#[from] sqlx::Error),
    /// No corporate action with that id.
    #[error("no corporate action with that id")]
    ActionNotFound,
    /// The action is not a ScripForScrip.
    #[error("that corporate action is not a scrip-for-scrip exchange")]
    NotAScripForScrip,
    /// The action has already been exchanged (trades reference it). Delete
    /// the closing Sell via `DELETE /sells` first to redo it.
    #[error("this exchange has already been applied")]
    AlreadyExchanged,
    /// Nothing of the original listing is held at the exchange date — there
    /// is nothing to substitute.
    #[error("nothing of the original listing is held at the exchange date")]
    NothingHeld,
    /// The original listing has a trade dated on or after the exchange date.
    /// The takeover delisted it, so such a trade contradicts the action —
    /// fix the data before exchanging.
    #[error("the original listing has a trade dated on or after the exchange date")]
    TradedOnOrAfterExchangeDate,
    /// The Sell-side invariants failed — defensive as to the allocations,
    /// which the exchange constructs to satisfy them; the reachable case is an
    /// exchange dated after today (SCENARIOS S-10), which the 422 names.
    #[error("the exchange's closing Sell was rejected: {0}")]
    Sell(#[source] sell::SellError),
}

impl From<sell::SellError> for ExchangeError {
    fn from(e: sell::SellError) -> Self {
        match e {
            sell::SellError::Db(err) => ExchangeError::Db(err),
            other => ExchangeError::Sell(other),
        }
    }
}

pub fn router() -> Router<SqlitePool> {
    Router::new().route("/corporate_actions/{id}/exchange", post(exchange))
}

/// Substitute every open parcel of the action's original listing with
/// replacement-listing parcels, atomically. The exchange takes no parameters:
/// the action's terms and the holdings at its date determine everything.
pub async fn db_exchange(pool: &SqlitePool, action_id: i64) -> Result<Exchange, ExchangeError> {
    let mut tx = pool.begin().await?;

    let action = match corporate_action::db_get_tx(&mut *tx, action_id).await? {
        Some(a) => a,
        None => return Err(ExchangeError::ActionNotFound),
    };
    let terms = Terms::of(&action.kind)?;
    check_exchangeable(&mut tx, action_id, action.listing_id, action.date).await?;

    // The original listing's open parcels, costed by the shared rollover
    // machinery (as-acquired units internally; allocations re-based across
    // splits; AMIT/return-of-capital reductions up to the exchange date).
    let inputs = rollover::CostBaseInputs::load(&mut tx, action.listing_id).await?;
    let parcels = inputs
        .open_parcels(&mut tx, action.listing_id, action.date)
        .await?;
    let mut replacements: Vec<Replacement> = Vec::new();
    for p in &parcels {
        replacements.push(terms.replacement_for(p, &inputs, action.date)?);
    }
    if replacements.is_empty() {
        return Err(ExchangeError::NothingHeld);
    }

    // The closing Sell, consuming every open parcel. All-scrip: zero
    // proceeds — the rollover disregards the gain and the Sell never reaches
    // the realised-gains report. With a cash component: the cash per old
    // unit is the Sell's price, and the realised-gains/net-capital-gain
    // reports assess its gain over the cash-apportioned cost base (the
    // AUD conversion prefers the ATO monthly rate for the cash currency).
    let listing_currency: String = sqlx::query_scalar("SELECT currency FROM listings WHERE id = ?")
        .bind(action.listing_id)
        .fetch_one(&mut *tx)
        .await?;
    let (sell_price, sell_currency) = match &terms.cash {
        Some((cash_per_unit, _, currency)) => (*cash_per_unit, currency.clone()),
        None => (Decimal::ZERO, listing_currency),
    };
    let sell_id: i64 = sqlx::query_scalar("SELECT COALESCE(MAX(id), 0) + 1 FROM trades")
        .fetch_one(&mut *tx)
        .await?;
    let sell_body = rollover::closing_sell_body(
        action.date,
        action.listing_id,
        1,
        sell_price,
        sell_currency,
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
        trade::Settlement::stated(action.date),
        None,
        Some(action_id),
        None,
        None,
        None,
    )
    .await?;

    // The replacement Buys: one per consumed parcel, dated the exchange date,
    // carrying the parcel's cost base (on the brokerage column, price 0) and
    // acquisition date.
    let mut replacement_ids = Vec::with_capacity(replacements.len());
    for (i, r) in replacements.iter().enumerate() {
        let buy_id = sell_id + 1 + i as i64;
        rollover::insert_replacement_buy(
            &mut tx,
            &rollover::ReplacementBuy {
                id: buy_id,
                date: action.date,
                listing_id: terms.scrip_listing_id,
                quantity: r.new_quantity,
                cost_base: r.carried_cost_base,
                currency: &r.currency,
                fx_rate: r.fx_rate,
                spot_fx_rate: r.spot_fx_rate,
                deemed_acquisition_date: r.deemed_acquisition_date,
                holding_account_id: r.holding_account_id,
            },
            rollover::Provenance::ScripAction(action_id),
        )
        .await?;
        replacement_ids.push(buy_id);
    }

    tx.commit().await?;

    // Read the freshly created rows back so the response is exactly what was
    // stored.
    let sell = trade::db_get(pool, sell_id)
        .await?
        .ok_or(sqlx::Error::RowNotFound)?;
    Ok(Exchange {
        sell,
        replacements: rollover::created_trades(pool, replacement_ids).await?,
    })
}

/// The exchange's terms, read off the action: the replacement listing, the
/// ratio, and the cash component of a partial rollover.
struct Terms {
    scrip_listing_id: i64,
    new_units: Decimal,
    old_units: Decimal,
    /// `(cash per old unit, replacement market value, cash currency)` — all
    /// three present together or all absent (body validation + table CHECKs),
    /// so a partial set never reaches here.
    cash: Option<(Decimal, Decimal, String)>,
    /// Partial rollover (Example 27): the cash side's share of each parcel's
    /// remaining reduced cost base, apportioned by the consideration's market
    /// values. Per old unit the holder receives `cash` plus `mv × new/old` of
    /// scrip, so cash's share is cash×old / (cash×old + mv×new) — kept as a
    /// (numerator, denominator) pair and multiplied before dividing, so exact
    /// fractions (e.g. Gunther's 1/3) don't round twice.
    cash_apportionment: Option<(Decimal, Decimal)>,
}

impl Terms {
    fn of(kind: &ActionKind) -> Result<Self, ExchangeError> {
        let ActionKind::ScripForScrip {
            scrip_listing_id,
            scrip_new_units,
            scrip_old_units,
            scrip_cash_per_unit,
            scrip_market_value,
            scrip_cash_currency,
        } = kind
        else {
            return Err(ExchangeError::NotAScripForScrip);
        };
        let cash = match (scrip_cash_per_unit, scrip_market_value, scrip_cash_currency) {
            (Some(cash), Some(mv), Some(currency)) => Some((*cash, *mv, currency.clone())),
            _ => None,
        };
        let cash_apportionment = cash.as_ref().map(|(cash, mv, _)| {
            (
                *cash * *scrip_old_units,
                *cash * *scrip_old_units + *mv * *scrip_new_units,
            )
        });
        Ok(Self {
            scrip_listing_id: *scrip_listing_id,
            new_units: *scrip_new_units,
            old_units: *scrip_old_units,
            cash,
            cash_apportionment,
        })
    }

    /// The replacement one open parcel is substituted with.
    fn replacement_for(
        &self,
        p: &rollover::RolledParcel,
        inputs: &rollover::CostBaseInputs,
        exchange_date: NaiveDate,
    ) -> Result<Replacement, sqlx::Error> {
        let reduced_cost_base = inputs.carried_cost_base(&p.parcel, p.remaining, exchange_date)?;

        // With a cash component, only the scrip side's market-value share of
        // the cost base rolls over into the replacement; the cash side's
        // share stays behind for the closing Sell's gain (computed the same
        // way by the realised-gains report). The two sides sum exactly to
        // the reduced cost base by construction.
        let carried_cost_base = match self.cash_apportionment {
            Some((num, den)) => reduced_cost_base - reduced_cost_base * num / den,
            None => reduced_cost_base,
        };

        Ok(Replacement {
            parcel_id: p.parcel.id,
            at_date_units: p.at_date_units,
            // The exchange ratio applies to units as held at the exchange date.
            new_quantity: p.at_date_units * self.new_units / self.old_units,
            carried_cost_base,
            currency: p.parcel.currency.clone(),
            fx_rate: p.parcel.fx_rate,
            spot_fx_rate: p.parcel.spot_fx_rate,
            // Chain through an earlier exchange: the clock always runs from
            // the first acquisition in the rollover chain — the parcel's own
            // `acquired()`, deemed date where it carries one.
            deemed_acquisition_date: p.parcel.acquired(),
            holding_account_id: p.parcel.holding_account_id,
        })
    }
}

/// A replacement Buy to create: the consumed parcel's exchange-date units
/// scaled by the ratio, its remaining reduced cost base, and its carried
/// acquisition date.
struct Replacement {
    parcel_id: i64,
    at_date_units: Decimal,
    new_quantity: Decimal,
    carried_cost_base: Decimal,
    currency: String,
    fx_rate: Decimal,
    spot_fx_rate: Option<Decimal>,
    deemed_acquisition_date: NaiveDate,
    /// A replacement parcel stays in the account of the parcel it
    /// substitutes.
    holding_account_id: i64,
}

/// The two write-time checks: the exchange has not already been applied, and
/// nothing of the original listing traded on or after the exchange date (the
/// takeover delisted it, so such a trade contradicts the action).
async fn check_exchangeable(
    conn: &mut sqlx::SqliteConnection,
    action_id: i64,
    listing_id: i64,
    date: NaiveDate,
) -> Result<(), ExchangeError> {
    let already: bool =
        sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM trades WHERE scrip_action_id = ?)")
            .bind(action_id)
            .fetch_one(&mut *conn)
            .await?;
    if already {
        return Err(ExchangeError::AlreadyExchanged);
    }

    let late_trade: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM trades WHERE listing_id = ? AND date >= ?)",
    )
    .bind(listing_id)
    .bind(date)
    .fetch_one(&mut *conn)
    .await?;
    if late_trade {
        return Err(ExchangeError::TradedOnOrAfterExchangeDate);
    }
    Ok(())
}

async fn exchange(
    State(pool): State<SqlitePool>,
    Path(action_id): Path<i64>,
) -> Result<(StatusCode, Json<Exchange>), ApiError> {
    let exchange = db_exchange(&pool, action_id).await?;
    Ok((StatusCode::CREATED, Json(exchange)))
}

impl From<ExchangeError> for ApiError {
    fn from(e: ExchangeError) -> Self {
        match e {
            ExchangeError::ActionNotFound => {
                ApiError::not_found("no corporate action with that id")
            }
            ExchangeError::NotAScripForScrip => {
                ApiError::unprocessable("that corporate action is not a scrip-for-scrip exchange")
            }
            ExchangeError::AlreadyExchanged => ApiError::unprocessable(
                "this exchange has already been applied — delete its closing Sell first to redo \
                 it",
            ),
            ExchangeError::NothingHeld => ApiError::unprocessable(
                "nothing of the original listing is held at the exchange date",
            ),
            ExchangeError::TradedOnOrAfterExchangeDate => ApiError::unprocessable(
                "the original listing has a trade dated on or after the exchange date — \
                 fix that trade before exchanging",
            ),
            ExchangeError::Sell(err) => {
                tracing::warn!(
                    error = ?err,
                    "scrip-for-scrip exchange rejected by a sell invariant"
                );
                // A future-dated exchange is the one Sell rejection a user can
                // actually cause (SCENARIOS S-10): the takeover may be recorded
                // ahead of its date, but the replacement parcels would be dated
                // then too, and would not be held today.
                if matches!(
                    err,
                    sell::SellError::Amounts(trade::AmountsError::FutureDate)
                ) {
                    ApiError::unprocessable(
                        "the exchange is dated after today — record the action now and exchange \
                         on its effective date",
                    )
                } else {
                    ApiError::unprocessable("the exchange's parcel allocations are invalid")
                }
            }
            ExchangeError::Db(err) => err.into(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::entities::sell::SellBody;
    use crate::entities::trade::TradeType;
    use crate::entities::{corporate_action::CorporateAction, listing};
    use crate::test_support::{self, ApiClient, test_pool};

    /// Client over this module's own routes.
    fn client(pool: &SqlitePool) -> ApiClient {
        ApiClient::over(router().with_state(pool.clone()))
    }

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

    /// A listing quoted in a foreign currency: a trade is recorded in its
    /// listing's own currency (`trade::UpsertError::CurrencyNotListings`).
    async fn insert_listing_in(pool: &SqlitePool, id: i64, ticker: &str, currency: &str) {
        test_support::listing(id)
            .mic("XNYS")
            .ticker(ticker)
            .name(ticker)
            .currency(currency)
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

    /// A 2-for-1 takeover of listing 1 by listing 2 on the given date: every
    /// 1 unit of listing 1 becomes 2 units of listing 2.
    async fn insert_scrip(pool: &SqlitePool, id: i64, date: NaiveDate) {
        insert_scrip_terms(pool, id, 1, 2, date, "2", "1").await;
    }

    async fn insert_scrip_terms(
        pool: &SqlitePool,
        id: i64,
        listing_id: i64,
        scrip_listing_id: i64,
        date: NaiveDate,
        new: &str,
        old: &str,
    ) {
        insert_scrip_cash_terms(pool, id, listing_id, scrip_listing_id, date, new, old, None).await;
    }

    /// A takeover with an optional cash component: `cash` is
    /// `(cash per old unit, market value per new unit)` in AUD.
    #[allow(clippy::too_many_arguments)]
    async fn insert_scrip_cash_terms(
        pool: &SqlitePool,
        id: i64,
        listing_id: i64,
        scrip_listing_id: i64,
        date: NaiveDate,
        new: &str,
        old: &str,
        cash: Option<(&str, &str)>,
    ) {
        corporate_action::db_upsert(
            pool,
            &CorporateAction {
                id,
                listing_id,
                date,
                kind: ActionKind::ScripForScrip {
                    scrip_listing_id,
                    scrip_new_units: new.parse().unwrap(),
                    scrip_old_units: old.parse().unwrap(),
                    scrip_cash_per_unit: cash.map(|(c, _)| c.parse().unwrap()),
                    scrip_market_value: cash.map(|(_, mv)| mv.parse().unwrap()),
                    scrip_cash_currency: cash.map(|_| "AUD".to_string()),
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

    /// The core substitution: each open parcel becomes a replacement parcel
    /// carrying its cost base and acquisition date; the original holding is
    /// closed by a zero-proceeds Sell consuming every parcel.
    #[tokio::test]
    async fn exchange_substitutes_parcels_carrying_cost_base_and_acquisition_date() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "OLD").await;
        insert_listing(&pool, 2, "NEW").await;
        // Two parcels: 1,000 @ $1.50 (2020) and 500 @ $2.00 (2023).
        insert_buy(&pool, 1, 1, d(2020, 10, 1), "1000", "1.50").await;
        insert_buy(&pool, 2, 1, d(2023, 3, 1), "500", "2.00").await;
        insert_scrip(&pool, 10, d(2024, 7, 1)).await;

        let ex = db_exchange(&pool, 10).await.unwrap();

        // The closing Sell: all 1,500 units at zero proceeds on the exchange
        // date, marked with the action's provenance.
        assert_eq!(ex.sell.trade_type, TradeType::Sell);
        assert_eq!(ex.sell.listing_id, 1);
        assert_eq!(ex.sell.date, d(2024, 7, 1));
        assert_eq!(ex.sell.settlement_date, d(2024, 7, 1));
        assert_eq!(ex.sell.quantity, dec("1500"));
        assert_eq!(ex.sell.average_price, Decimal::ZERO);
        assert_eq!(ex.sell.scrip_action_id, Some(10));

        // Two replacement Buys on listing 2, 2-for-1, each carrying its
        // parcel's cost base (price 0 + brokerage = exact) and acquisition
        // date, dated at the exchange.
        assert_eq!(ex.replacements.len(), 2);
        let r1 = &ex.replacements[0];
        assert_eq!(r1.trade_type, TradeType::Buy);
        assert_eq!(r1.listing_id, 2);
        assert_eq!(r1.date, d(2024, 7, 1));
        assert_eq!(r1.quantity, dec("2000"));
        assert_eq!(r1.average_price, Decimal::ZERO);
        assert_eq!(r1.brokerage, dec("1500"));
        assert_eq!(r1.deemed_acquisition_date, Some(d(2020, 10, 1)));
        assert_eq!(r1.scrip_action_id, Some(10));
        let r2 = &ex.replacements[1];
        assert_eq!(r2.quantity, dec("1000"));
        assert_eq!(r2.brokerage, dec("1000"));
        assert_eq!(r2.deemed_acquisition_date, Some(d(2023, 3, 1)));

        // The allocations consume both parcels exactly.
        let n: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM parcel_allocations WHERE sale_trade_id = ?")
                .bind(ex.sell.id)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(n, 2);
    }

    /// A partly sold parcel carries only its remaining units' share of the
    /// cost base (incl. brokerage) into the replacement.
    #[tokio::test]
    async fn partly_sold_parcel_carries_only_the_remaining_cost_base() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "OLD").await;
        insert_listing(&pool, 2, "NEW").await;
        // 1,000 @ $1.50 + $50 brokerage = $1,550; sell 400 → 600 remain at
        // 60% of the cost base = $930.
        test_support::buy(1, 1)
            .date(d(2020, 10, 1))
            .settlement(d(2020, 10, 1))
            .qty(dec("1000"))
            .price(dec("1.50"))
            .brokerage(dec("50"))
            .insert(&pool)
            .await;
        sell_units(&pool, 2, 1, d(2022, 5, 2), "400").await;
        insert_scrip(&pool, 10, d(2024, 7, 1)).await;

        let ex = db_exchange(&pool, 10).await.unwrap();

        assert_eq!(ex.sell.quantity, dec("600"));
        assert_eq!(ex.replacements.len(), 1);
        assert_eq!(ex.replacements[0].quantity, dec("1200"));
        assert_eq!(ex.replacements[0].brokerage, dec("930"));
    }

    /// AMIT cost-base reductions and return-of-capital payments received
    /// while held reduce the carried cost base — the replacement carries the
    /// *reduced* cost base of the original interest.
    #[tokio::test]
    async fn amit_and_roc_reductions_carry_into_the_replacement_cost_base() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "OLD").await;
        insert_listing(&pool, 2, "NEW").await;
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
                    record_date: None,
                },
            },
        )
        .await
        .unwrap();

        insert_scrip(&pool, 10, d(2024, 7, 1)).await;
        let ex = db_exchange(&pool, 10).await.unwrap();

        // $1,500 − $100 (AMIT) − $50 (ROC) = $1,350 carried.
        assert_eq!(ex.replacements[0].brokerage, dec("1350"));
    }

    /// A cash component (partial rollover — Example 27's arithmetic at the
    /// exchange level): the consumed parcel's reduced cost base is
    /// apportioned by the consideration's market values — cash×old /
    /// (cash×old + mv×new) to the cash side — and only the scrip side's
    /// share is carried into the replacement; the closing Sell is priced at
    /// the cash per old unit in the cash currency.
    #[tokio::test]
    async fn cash_component_apportions_the_cost_base_and_prices_the_closing_sell() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "WDR").await;
        insert_listing(&pool, 2, "RGL").await;
        // Gunther: 100 shares with a $9 cost base each = $900.
        insert_buy(&pool, 1, 1, d(2020, 10, 1), "100", "9").await;
        // 1-for-1, $10 cash per old share, $20 market value per new share.
        insert_scrip_cash_terms(&pool, 10, 1, 2, d(2024, 7, 1), "1", "1", Some(("10", "20"))).await;

        let ex = db_exchange(&pool, 10).await.unwrap();

        // The closing Sell carries the cash proceeds: 100 × $10.
        assert_eq!(ex.sell.average_price, dec("10"));
        assert_eq!(ex.sell.quantity, dec("100"));
        assert_eq!(ex.sell.currency, "AUD");
        assert_eq!(ex.sell.scrip_action_id, Some(10));
        // Cash's share of the $900: 10/(10 + 20) = $300 — realised by the
        // reports against the Sell; the replacement carries the $600 rest.
        assert_eq!(ex.replacements.len(), 1);
        assert_eq!(ex.replacements[0].quantity, dec("100"));
        assert_eq!(ex.replacements[0].brokerage, dec("600"));
        assert_eq!(
            ex.replacements[0].deemed_acquisition_date,
            Some(d(2020, 10, 1))
        );
    }

    /// The apportionment respects a non-1:1 exchange ratio — per old unit
    /// the scrip side is worth mv × new/old — and an exact fraction (3/11
    /// here) divides once, after the multiplication, so the carried and
    /// realised shares sum exactly to the reduced cost base.
    #[tokio::test]
    async fn cash_apportionment_scales_the_scrip_side_by_the_exchange_ratio() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "OLD").await;
        insert_listing(&pool, 2, "NEW").await;
        // Cost base $1,100; 2-for-1 exchange, $3 cash per old unit, $4 market
        // value per new unit → per old unit 3 cash + 8 scrip; the cash share
        // is 3/11 of $1,100 = $300.
        insert_buy(&pool, 1, 1, d(2020, 10, 1), "100", "11").await;
        insert_scrip_cash_terms(&pool, 10, 1, 2, d(2024, 7, 1), "2", "1", Some(("3", "4"))).await;

        let ex = db_exchange(&pool, 10).await.unwrap();

        assert_eq!(ex.sell.average_price, dec("3"));
        assert_eq!(ex.replacements[0].quantity, dec("200"));
        // Carried = 1,100 − 1,100 × 3/11 = $800, exact.
        assert_eq!(ex.replacements[0].brokerage, dec("800"));
    }

    /// A split on the original listing before the exchange re-bases the
    /// units the ratio applies to; the cost base is unchanged by the split.
    #[tokio::test]
    async fn split_before_the_exchange_rebases_the_exchanged_units() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "OLD").await;
        insert_listing(&pool, 2, "NEW").await;
        insert_buy(&pool, 1, 1, d(2020, 10, 1), "1000", "1.50").await;
        // 3-for-1 split: the 1,000 as-acquired units are 3,000 at the exchange.
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
        insert_scrip(&pool, 10, d(2024, 7, 1)).await;

        let ex = db_exchange(&pool, 10).await.unwrap();

        // 3,000 exchange-date units close; 2-for-1 → 6,000 replacement units
        // carrying the unchanged $1,500 cost base and the 2020 acquisition.
        assert_eq!(ex.sell.quantity, dec("3000"));
        assert_eq!(ex.replacements[0].quantity, dec("6000"));
        assert_eq!(ex.replacements[0].brokerage, dec("1500"));
        assert_eq!(
            ex.replacements[0].deemed_acquisition_date,
            Some(d(2020, 10, 1))
        );
    }

    /// SCENARIOS E-36: an exchange ratio that does not divide the holding.
    /// The replacement keeps the **exact** fractional unit count (registry
    /// rounding and cash-in-lieu of the fraction are not modelled), and the
    /// whole cost base rides on it — nothing is dropped in the division.
    #[tokio::test]
    async fn a_ratio_that_does_not_divide_keeps_the_exact_fraction() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "OLD").await;
        insert_listing(&pool, 2, "NEW").await;
        insert_buy(&pool, 1, 1, d(2022, 1, 10), "101", "10").await;
        insert_scrip_terms(&pool, 10, 1, 2, d(2024, 3, 1), "1", "3").await;

        let ex = db_exchange(&pool, 10).await.unwrap();
        assert_eq!(ex.replacements.len(), 1);
        assert_eq!(
            ex.replacements[0].quantity,
            dec("33.666666666666666666666666667")
        );
        assert_eq!(ex.replacements[0].brokerage, dec("1010"));

        let parcels = crate::reports::open_parcels::db_open_parcels(&pool)
            .await
            .unwrap();
        assert_eq!(parcels.len(), 1);
        assert_eq!(
            parcels[0].remaining_quantity,
            dec("33.666666666666666666666666667")
        );
        assert_eq!(parcels[0].remaining_cost_base, dec("1010"));
    }

    /// A second exchange chains the deemed acquisition date from the first —
    /// the discount clock always runs from the original acquisition.
    #[tokio::test]
    async fn chained_exchange_carries_the_original_acquisition_date() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "OLD").await;
        insert_listing(&pool, 2, "MID").await;
        insert_listing(&pool, 3, "NEW").await;
        insert_buy(&pool, 1, 1, d(2020, 10, 1), "1000", "1.50").await;
        insert_scrip(&pool, 10, d(2024, 7, 1)).await;
        db_exchange(&pool, 10).await.unwrap();

        // Listing 2 is itself taken over by listing 3, 1-for-2.
        insert_scrip_terms(&pool, 11, 2, 3, d(2025, 2, 1), "1", "2").await;
        let ex = db_exchange(&pool, 11).await.unwrap();

        assert_eq!(ex.replacements.len(), 1);
        assert_eq!(ex.replacements[0].listing_id, 3);
        assert_eq!(ex.replacements[0].quantity, dec("1000"));
        assert_eq!(ex.replacements[0].brokerage, dec("1500"));
        assert_eq!(
            ex.replacements[0].deemed_acquisition_date,
            Some(d(2020, 10, 1))
        );
    }

    #[tokio::test]
    async fn invalid_exchanges_are_rejected_and_nothing_persisted() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "OLD").await;
        insert_listing(&pool, 2, "NEW").await;

        // Missing action.
        assert!(matches!(
            db_exchange(&pool, 99).await,
            Err(ExchangeError::ActionNotFound)
        ));

        // Not a ScripForScrip.
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
            db_exchange(&pool, 1).await,
            Err(ExchangeError::NotAScripForScrip)
        ));

        // Nothing held at the exchange date.
        insert_scrip(&pool, 10, d(2024, 7, 1)).await;
        assert!(matches!(
            db_exchange(&pool, 10).await,
            Err(ExchangeError::NothingHeld)
        ));

        // A target trade dated on/after the exchange date contradicts the
        // takeover.
        insert_buy(&pool, 1, 1, d(2024, 7, 1), "100", "1.50").await;
        assert!(matches!(
            db_exchange(&pool, 10).await,
            Err(ExchangeError::TradedOnOrAfterExchangeDate)
        ));

        // Nothing was persisted by any of the rejections.
        let trades: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM trades WHERE scrip_action_id IS NOT NULL")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(trades, 0);
    }

    #[tokio::test]
    async fn a_second_exchange_of_the_same_action_is_rejected() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "OLD").await;
        insert_listing(&pool, 2, "NEW").await;
        insert_buy(&pool, 1, 1, d(2020, 10, 1), "1000", "1.50").await;
        insert_scrip(&pool, 10, d(2024, 7, 1)).await;
        db_exchange(&pool, 10).await.unwrap();

        assert!(matches!(
            db_exchange(&pool, 10).await,
            Err(ExchangeError::AlreadyExchanged)
        ));
    }

    /// The group is immutable trade by trade: the closing Sell rejects
    /// `PUT /sells`, the replacement Buys reject `PUT /trades`, and neither
    /// can be deleted individually via `DELETE /trades`.
    #[tokio::test]
    async fn exchange_trades_are_immutable_individually() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "OLD").await;
        insert_listing(&pool, 2, "NEW").await;
        insert_buy(&pool, 1, 1, d(2020, 10, 1), "1000", "1.50").await;
        insert_scrip(&pool, 10, d(2024, 7, 1)).await;
        let ex = db_exchange(&pool, 10).await.unwrap();

        // PUT /sells on the closing Sell → rejected.
        let err = sell::db_upsert_sell(
            &pool,
            ex.sell.id,
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
        assert!(matches!(err, Err(sell::SellError::ScripExchangeSell)));

        // PUT /trades on a replacement Buy → rejected.
        let mut edited = ex.replacements[0].clone();
        edited.quantity = dec("9999");
        assert!(matches!(
            trade::db_upsert(&pool, &edited).await,
            Err(trade::UpsertError::ScripExchangeTrade)
        ));

        // DELETE /trades on a replacement Buy (or the Sell) → refused.
        assert_eq!(
            trade::db_delete(&pool, ex.replacements[0].id)
                .await
                .unwrap(),
            trade::DeleteOutcome::Referenced
        );
        assert_eq!(
            trade::db_delete(&pool, ex.sell.id).await.unwrap(),
            trade::DeleteOutcome::Referenced
        );
    }

    /// `DELETE /sells` on the closing Sell removes the whole group and
    /// restores the pre-exchange holding; it is refused while a replacement
    /// Buy is consumed by a later sale.
    #[tokio::test]
    async fn deleting_the_closing_sell_removes_the_whole_group() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "OLD").await;
        insert_listing(&pool, 2, "NEW").await;
        insert_buy(&pool, 1, 1, d(2020, 10, 1), "1000", "1.50").await;
        insert_scrip(&pool, 10, d(2024, 7, 1)).await;
        let ex = db_exchange(&pool, 10).await.unwrap();

        // A later sale out of the replacement parcel blocks the group delete.
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
                quantity: dec("500"),
                currency: "AUD".to_string(),
                brokerage: Decimal::ZERO,
                gst_on_brokerage: Decimal::ZERO,
                brokerage_currency: "AUD".to_string(),
                fx_rate: Decimal::ONE,
                spot_fx_rate: None,
                contract_note_ref: None,
                allocations: vec![AllocationInput {
                    purchase_trade_id: ex.replacements[0].id,
                    quantity_allocated: dec("500"),
                }],
            },
        )
        .await
        .unwrap();
        assert_eq!(
            sell::db_delete_sell(&pool, ex.sell.id).await.unwrap(),
            sell::DeleteOutcome::ReplacementReferenced
        );

        // Remove the later sale; the group then deletes as a whole and the
        // action thaws (it can be deleted again).
        assert_eq!(
            sell::db_delete_sell(&pool, 50).await.unwrap(),
            sell::DeleteOutcome::Deleted
        );
        assert_eq!(
            sell::db_delete_sell(&pool, ex.sell.id).await.unwrap(),
            sell::DeleteOutcome::Deleted
        );
        let remaining: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM trades WHERE scrip_action_id IS NOT NULL")
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

    /// The action is frozen while its exchange group exists.
    #[tokio::test]
    async fn referenced_action_cannot_be_edited_or_deleted() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "OLD").await;
        insert_listing(&pool, 2, "NEW").await;
        insert_buy(&pool, 1, 1, d(2020, 10, 1), "1000", "1.50").await;
        insert_scrip(&pool, 10, d(2024, 7, 1)).await;
        db_exchange(&pool, 10).await.unwrap();

        let action = corporate_action::db_get(&pool, 10).await.unwrap().unwrap();
        assert!(matches!(
            corporate_action::db_upsert(&pool, &action).await,
            Err(corporate_action::WriteError::ReferencedByTrade)
        ));
        assert!(corporate_action::db_delete(&pool, 10).await.is_err());
    }

    /// The exchange closes the whole holding across every holding account;
    /// each replacement parcel stays in the account of the parcel it
    /// substitutes.
    #[tokio::test]
    async fn replacements_stay_in_each_parcels_holding_account() {
        use crate::entities::holding_account::{self, HoldingAccount};
        let pool = test_pool().await;
        insert_listing(&pool, 1, "OLD").await;
        insert_listing(&pool, 2, "NEW").await;
        holding_account::db_upsert(
            &pool,
            &HoldingAccount {
                id: 2,
                name: "ICE Employee Plan".to_string(),
            },
        )
        .await
        .unwrap();
        // One parcel per account.
        insert_buy(&pool, 1, 1, d(2020, 10, 1), "1000", "1.50").await;
        insert_buy(&pool, 2, 1, d(2023, 3, 1), "500", "2.00").await;
        sqlx::query("UPDATE trades SET holding_account_id = 2 WHERE id = 2")
            .execute(&pool)
            .await
            .unwrap();
        insert_scrip(&pool, 10, d(2024, 7, 1)).await;

        let ex = db_exchange(&pool, 10).await.unwrap();

        assert_eq!(ex.replacements.len(), 2);
        assert_eq!(ex.replacements[0].holding_account_id, 1);
        assert_eq!(ex.replacements[1].holding_account_id, 2);
    }

    // API-level tests

    #[tokio::test]
    async fn api_exchange_creates_the_group() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "OLD").await;
        insert_listing(&pool, 2, "NEW").await;
        insert_buy(&pool, 1, 1, d(2020, 10, 1), "1000", "1.50").await;
        insert_scrip(&pool, 10, d(2024, 7, 1)).await;

        let resp = client(&pool)
            .post_empty("/corporate_actions/10/exchange")
            .await;
        assert_eq!(resp.status, StatusCode::CREATED);
        let v: serde_json::Value = resp.json();
        assert_eq!(v["sell"]["quantity"], "1000");
        assert_eq!(v["replacements"][0]["quantity"], "2000");
        assert_eq!(
            v["replacements"][0]["deemed_acquisition_date"],
            "2020-10-01"
        );
    }

    #[tokio::test]
    async fn api_exchange_maps_errors_to_statuses() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "OLD").await;
        insert_listing(&pool, 2, "NEW").await;

        // Missing action → 404.
        let resp = client(&pool)
            .post_empty("/corporate_actions/99/exchange")
            .await;
        assert_eq!(resp.status, StatusCode::NOT_FOUND);

        // Nothing held → 422.
        insert_scrip(&pool, 10, d(2024, 7, 1)).await;
        let resp = client(&pool)
            .post_empty("/corporate_actions/10/exchange")
            .await;
        assert_eq!(resp.status, StatusCode::UNPROCESSABLE_ENTITY);
    }

    /// A consumed parcel's deliberate spot-rate override carries onto its
    /// replacement Buy (like `fx_rate` and the deemed acquisition date), so
    /// the AUD cost base is unchanged by the exchange — the spot rate keeps
    /// winning at the deemed acquisition month.
    #[tokio::test]
    async fn exchange_carries_spot_fx_rate_onto_replacement() {
        let pool = test_pool().await;
        insert_listing_in(&pool, 1, "OLD", "USD").await;
        insert_listing_in(&pool, 2, "NEW", "USD").await;
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
        insert_scrip(&pool, 10, d(2024, 7, 1)).await;

        let ex = db_exchange(&pool, 10).await.unwrap();
        assert_eq!(ex.replacements.len(), 1);
        assert_eq!(ex.replacements[0].fx_rate, dec("0.70"));
        assert_eq!(ex.replacements[0].spot_fx_rate, Some(dec("0.6543")));
    }

    /// SCENARIOS D-19. The exchange's closing Sell consumed the original
    /// parcel outright, so a Sell entered the next day against that parcel has
    /// nothing to draw on: it is refused on capacity (the units live in the
    /// replacement parcel now), and nothing is persisted.
    #[tokio::test]
    async fn a_parcel_the_exchange_consumed_cannot_be_allocated_to_a_later_sell() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "OLD").await;
        insert_listing(&pool, 2, "NEW").await;
        insert_buy(&pool, 1, 1, d(2022, 1, 10), "100", "10").await;
        insert_scrip(&pool, 10, d(2024, 6, 2)).await;
        db_exchange(&pool, 10).await.unwrap();

        let err = sell::db_upsert_sell(
            &pool,
            60,
            &SellBody {
                brokerage_includes_gst: false,
                statement_total: None,
                holding_account_id: 1,
                date: d(2024, 6, 3),
                settlement_date: Some(d(2024, 6, 3)),
                listing_id: 1,
                average_price: dec("15"),
                quantity: dec("100"),
                currency: "AUD".to_string(),
                brokerage: Decimal::ZERO,
                gst_on_brokerage: Decimal::ZERO,
                brokerage_currency: "AUD".to_string(),
                fx_rate: Decimal::ONE,
                spot_fx_rate: None,
                contract_note_ref: None,
                allocations: vec![AllocationInput {
                    purchase_trade_id: 1,
                    quantity_allocated: dec("100"),
                }],
            },
        )
        .await
        .unwrap_err();
        assert!(
            matches!(err, sell::SellError::PurchaseQuantityExceeded),
            "{err}"
        );
        let exists: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM trades WHERE id = 60")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(exists, 0);
    }
}
