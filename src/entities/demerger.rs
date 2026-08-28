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
use crate::infra::db::write_tx;
use crate::infra::decimal::mul_div;
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
    /// The **demerged** listing already carries a whole-holding operation of
    /// its own dated on or after this demerger's date — a scrip-for-scrip
    /// exchange, a demerger, or a worthless-shares recognise. The demerged
    /// parcels are dated the demerger date, so they would land behind it and
    /// could never be consumed by it (SCENARIOS V-d). The head listing needs no
    /// such check: [`TradedOnOrAfterDemergerDate`](DemergeError::TradedOnOrAfterDemergerDate)
    /// already refuses *any* trade of it dated on or after the demerger, an
    /// operation's closing Sell included. Wording and recovery in
    /// `domain::whole_holding`. Mapped to 422.
    #[error("the demerged parcels are dated behind a whole-holding operation: {0}")]
    BackDatedOverWholeHolding(#[source] crate::domain::whole_holding::BackDatedParcel),
    /// The entitlement ratio applied to the holding produces a demerged
    /// quantity past `Decimal`'s range — a 1000-for-1 demerger of 1e27 units
    /// asks for 1e30 demerged units. There is no lesser number of units to
    /// write, and nothing downstream could recover one, so the demerge is
    /// refused before it writes anything
    /// (`domain::cost_base::checked_rebased_quantity`; the arithmetic used to
    /// panic, which the panic layer answered as a logged `500` with an empty
    /// body). Mapped to 422.
    #[error("the demerged quantity is beyond the representable range: {0}")]
    UnrepresentableDemergedQuantity(#[source] crate::domain::cost_base::UnrepresentableQuantity),
    /// The **demerged listing's** own recorded splits and bonus issues re-base
    /// a demerged-entity parcel past what a `Decimal` can hold — and the
    /// entitlement ratio can be entirely ordinary while they do. A 1-for-1
    /// demerger of 1e26 units onto a listing carrying a 1000-for-1 split
    /// answered `201` and then killed every open-holdings read of the whole
    /// portfolio: a `ShareSplit` materialises nothing, so
    /// [`UnrepresentableDemergedQuantity`](DemergeError::UnrepresentableDemergedQuantity)
    /// above — which asks about the *entitlement* ratio — is satisfied, and the
    /// destination's ratio is applied at read time afterwards. So the walk runs
    /// on the listing the demerged parcels land on, which is not the listing
    /// the operation is about (SCENARIOS V-d's lesson, a second time). The head
    /// replacements need no walk of their own: each carries its parcel's own
    /// units at the demerger date, so every later ratio re-bases the parcel it
    /// came from at least as far, and that parcel is already bounded. Mapped
    /// to 422.
    #[error("the demerged listing re-bases a quantity beyond the representable range: {0}")]
    UnrepresentableRebasedQuantity(#[source] crate::domain::cost_base::UnrepresentableQuantity),
    /// The Sell-side invariants failed — defensive as to the allocations,
    /// which the demerge constructs to satisfy them; the reachable case is a
    /// demerger dated after today (SCENARIOS S-10), which the 422 names.
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
    let mut tx = write_tx(pool).await?;

    let action = match corporate_action::db_get_tx(&mut *tx, action_id).await? {
        Some(a) => a,
        None => return Err(DemergeError::ActionNotFound),
    };
    let ActionKind::Demerger {
        demerger_listing_id,
        demerger_new_units: new_units,
        demerger_held_units: held_units,
        demerger_cost_base_pct: cost_base_pct,
        // The stated pre-demerger close is a price-series fact, read only by
        // the closing-price re-base; the demerge operation ignores it.
        ..
    } = action.kind
    else {
        return Err(DemergeError::NotADemerger);
    };

    check_demergeable(&mut tx, action_id, action.listing_id, action.date).await?;

    // The demerged parcels are dated the demerger date on the *demerged*
    // listing, so if that listing has itself been taken over, demerged or
    // written off since — dated on or after this demerger — they would land
    // behind an operation that consumed the whole holding and could never be
    // consumed by it (SCENARIOS V-d). Checked before anything is written, so
    // this demerger's own rows cannot be mistaken for the offender.
    if let Some(back_dated) = crate::domain::whole_holding::db_back_dated_parcel(
        &mut tx,
        demerger_listing_id,
        action.date,
        None,
    )
    .await?
    {
        return Err(DemergeError::BackDatedOverWholeHolding(back_dated));
    }

    // The head listing's open parcels, costed by the shared rollover
    // machinery (as-acquired units internally; allocations re-based across
    // splits). The ATO's step 1 takes the cost base immediately before the
    // demerger, so it is bounded at the demerger date.
    let inputs = rollover::CostBaseInputs::load(&mut tx, action.listing_id, action.date).await?;
    let parcels = inputs.open_parcels(&mut tx, action.listing_id).await?;

    let mut replacements: Vec<Replacement> = Vec::with_capacity(parcels.len());
    for p in &parcels {
        let carried_cost_base = inputs.carried_cost_base(&p.parcel, p.remaining)?;

        // Step 2: apportion by the advised percentage. demerged + head sum
        // exactly to the carried cost base by construction. Both pro-rates go
        // through `mul_div`, which keeps the multiply-then-divide order for
        // every figure that fits and still answers where the intermediate
        // product would overflow `rust_decimal` — a parcel costed at 1e27 at
        // 99% is `9.9e28` in the working and `9.9e26` in the answer, and this
        // is a write path, so the panic aborted the whole demerge.
        let demerged_cost_base = mul_div(&[carried_cost_base, cost_base_pct], Decimal::ONE_HUNDRED);

        replacements.push(Replacement {
            parcel_id: p.parcel.id,
            at_date_units: p.at_date_units,
            // The entitlement ratio applies to units as held at the demerger
            // date. `held_units` is the action's own ratio denominator,
            // validated positive when the corporate action is written
            // (`CorporateActionBody::kind`'s `positive`), so it is never nil.
            // Checked rather than computed: an entitlement ratio greater than
            // one over a very large holding produces a demerged quantity past
            // `Decimal`'s ceiling, and there is no lesser count to write.
            demerged_quantity: crate::domain::cost_base::checked_rebased_quantity(
                ("units held", p.at_date_units),
                ("demerger_new_units", new_units),
                ("demerger_held_units", held_units),
            )
            .map_err(DemergeError::UnrepresentableDemergedQuantity)?,
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
    // No id of our own: the database assigns one its AUTOINCREMENT sequence
    // has never issued, so this Sell can never land on a deleted trade's id
    // (SCENARIOS U-a — that is exactly how live trade 9072's audit trail
    // became this operation's closing Sell's).
    let sell_id = sell::upsert_sell_in_tx(
        &mut tx,
        sell::SellWrite {
            id: None,
            body: &sell_body,
            settlement: trade::Settlement::stated(action.date),
            provenance: Some(sell::SellProvenance::DemergerAction(action_id)),
        },
    )
    .await?;

    // The replacement Buys: per consumed parcel, the head replacement (same
    // listing, same units, head share of the cost base) and the
    // demerged-entity Buy (entitlement units, demerged share), both dated the
    // demerger date and carrying the parcel's acquisition date.
    let mut head_ids = Vec::with_capacity(replacements.len());
    let mut demerged_ids = Vec::with_capacity(replacements.len());
    for r in &replacements {
        // Each Buy takes the id its own INSERT was given — never the previous
        // one plus one, which would be the same guess `MAX(id) + 1` was.
        for (into, listing_id, quantity, cost_base) in [
            (
                &mut head_ids,
                action.listing_id,
                r.at_date_units,
                r.head_cost_base,
            ),
            (
                &mut demerged_ids,
                demerger_listing_id,
                r.demerged_quantity,
                r.demerged_cost_base,
            ),
        ] {
            let buy_id = rollover::insert_replacement_buy(
                &mut tx,
                &rollover::ReplacementBuy {
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
            into.push(buy_id);
        }
    }

    // The demerged listing's own `ShareSplit`/`BonusIssue` ratios are re-applied
    // at *read* time, so one of them can push a demerged-entity parcel past
    // `Decimal`'s range while the entitlement ratio checked above is perfectly
    // ordinary — a 1-for-1 demerger onto a listing carrying a 1000-for-1 split
    // did exactly that. Asked of the **demerged** listing, over the parcels
    // this operation has just written, so what is judged is the state the
    // commit would leave behind
    // (`corporate_action::rebased_quantity_beyond_range`). The head listing is
    // not asked: a head replacement carries its own parcel's units at the
    // demerger date, so every later ratio re-bases that parcel at least as far,
    // and the parcel is bounded by the write that created it.
    if let Some(beyond) =
        corporate_action::rebased_quantity_beyond_range(&mut tx, demerger_listing_id).await?
    {
        return Err(DemergeError::UnrepresentableRebasedQuantity(beyond));
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
            // The same body every parcel-creating path answers for this fact —
            // here the parcels are the demerger's own demerged Buys.
            DemergeError::BackDatedOverWholeHolding(e) => ApiError::Unprocessable(e.message()),
            // The ratio times the holding is past what a decimal can hold →
            // 422 quoting the arithmetic, the same wording every
            // beyond-the-range refusal answers with.
            DemergeError::UnrepresentableDemergedQuantity(e) => {
                ApiError::Unprocessable(e.message())
            }
            // The demerged listing's own ratios re-base one of the parcels
            // this would write past what a decimal can hold → the same body.
            DemergeError::UnrepresentableRebasedQuantity(e) => ApiError::Unprocessable(e.message()),
            DemergeError::Sell(err) => {
                tracing::warn!(error = ?err, "demerge rejected by a sell invariant");
                // A future-dated demerger is the one Sell rejection a user can
                // actually cause (SCENARIOS S-10): the demerger may be recorded
                // ahead of its date, but the parcels it creates would be dated
                // then too, and would not be held today.
                if matches!(
                    err,
                    sell::SellError::Amounts(trade::AmountsError::FutureDate)
                ) {
                    ApiError::unprocessable(
                        "the demerger is dated after today — record the action now and demerge \
                         on its effective date",
                    )
                } else {
                    ApiError::unprocessable("the demerger's parcel allocations are invalid")
                }
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
                    demerger_close_date: None,
                    demerger_close_price: None,
                    demerger_close_sourced_from: None,
                    demerger_close_reason: None,
                },
            },
        )
        .await
        .unwrap();
    }

    /// [`sell_units`] against an arbitrary listing at an arbitrary price —
    /// for the post-demerger sales, which dispose of the *demerged* listing.
    #[allow(clippy::too_many_arguments)]
    async fn sell_listing(
        pool: &SqlitePool,
        sell_id: i64,
        listing_id: i64,
        parcel_id: i64,
        date: NaiveDate,
        qty: &str,
        price: &str,
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
                listing_id,
                average_price: dec(price),
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

    /// An AMMA statement whose year end postdates the demerger must not fold
    /// its reduction into the carried cost base the two legs apportion: the
    /// adjustment arises at the statement's year end, which has not happened
    /// yet as at the demerger date (the bound the return-of-capital events
    /// already observe). The reduction reaches the replacement parcels later,
    /// through their own adjustment rows — never through the carry as well.
    #[tokio::test]
    async fn a_statement_year_ending_after_the_demerger_does_not_reach_the_carried_cost_base() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "HEAD").await;
        insert_listing(&pool, 2, "DEM").await;
        // 1,000 units at $10: a $10,000 initial cost base.
        insert_buy(&pool, 1, 1, d(2024, 1, 10), "1000", "10").await;
        // FY2025 statement ($1/unit, year end 2025-06-30) entered with its
        // adjustment row before the demerger, which is dated earlier in the
        // same year.
        test_support::amma(6, 1)
            .units(dec("1000"))
            .cost_base_adjustment(dec("1"))
            .with(|a| a.tax_year_end_date = d(2025, 6, 30))
            .insert(&pool)
            .await;
        test_support::amit_adjustment(&pool, 1, 6, 1, dec("1000")).await;
        insert_demerger(&pool, 10, d(2025, 5, 1)).await;

        let dm = db_demerge(&pool, 10).await.unwrap();

        // 80/20 of the unreduced $10,000 — not of $9,000.
        assert_eq!(dm.head_replacements[0].brokerage, dec("8000"));
        assert_eq!(dm.demerged_replacements[0].brokerage, dec("2000"));
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

    /// SCENARIOS E-42: the percentage at either extreme of what the write
    /// path allows. Neither end may strand a cent — the side that keeps
    /// almost nothing still carries the remainder exactly, because the head
    /// leg is computed as `cost − demerged` rather than as its own percentage.
    #[tokio::test]
    async fn extreme_percentages_still_apportion_the_whole_cost_base() {
        for (pct, demerged_cost) in [("0.01", "0.50"), ("99.99", "4999.50")] {
            let pool = test_pool().await;
            insert_listing(&pool, 1, "HEAD").await;
            insert_listing(&pool, 2, "DEM").await;
            insert_buy(&pool, 1, 1, d(2022, 1, 10), "500", "10").await;
            insert_demerger_terms(&pool, 10, 1, 2, d(2024, 3, 1), "1", "5", pct).await;

            let dm = db_demerge(&pool, 10).await.unwrap();
            assert_eq!(dm.demerged_replacements[0].brokerage, dec(demerged_cost));
            assert_eq!(
                dm.head_replacements[0].brokerage + dm.demerged_replacements[0].brokerage,
                dec("5000")
            );
        }
    }

    /// SCENARIOS E-43: the head parcel was inherited — a pre-CGT asset in the
    /// deceased's hands, so its cost base is the market value at death and
    /// its discount clock runs from the death (s 115-30). Both are carried
    /// through the demerger: the two replacement parcels report the death
    /// date, not the demerger date, and split the inherited cost base.
    #[tokio::test]
    async fn an_inherited_head_parcel_carries_its_deemed_date_and_cost_base() {
        use crate::entities::inheritance::{self, CostBaseRule, Inheritance};
        let pool = test_pool().await;
        insert_listing(&pool, 1, "HEAD").await;
        insert_listing(&pool, 2, "DEM").await;
        inheritance::db_upsert(
            &pool,
            &Inheritance {
                id: 1,
                listing_id: 1,
                holding_account_id: 1,
                quantity: dec("500"),
                date_of_death: d(2023, 5, 10),
                cost_base_rule: CostBaseRule::MarketValueAtDeath,
                cost_base: dec("6000"),
                lpr_expenditure: Decimal::ZERO,
                lpr_expenditure_date: None,
                deceased_acquisition_date: None,
                currency: "AUD".to_string(),
                fx_rate: Decimal::ONE,
            },
        )
        .await
        .unwrap();
        insert_demerger_terms(&pool, 10, 1, 2, d(2024, 3, 1), "1", "5", "10").await;

        let dm = db_demerge(&pool, 10).await.unwrap();
        assert_eq!(
            dm.head_replacements[0].deemed_acquisition_date,
            Some(d(2023, 5, 10))
        );
        assert_eq!(
            dm.demerged_replacements[0].deemed_acquisition_date,
            Some(d(2023, 5, 10))
        );
        assert_eq!(dm.head_replacements[0].brokerage, dec("5400"));
        assert_eq!(dm.demerged_replacements[0].brokerage, dec("600"));

        let parcels = crate::reports::open_parcels::db_open_parcels(&pool)
            .await
            .unwrap();
        assert!(
            parcels.iter().all(|p| p.acquisition_date == d(2023, 5, 10)),
            "{parcels:?}"
        );
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
        sell_units(&pool, 2, 1, d(2022, 5, 2), "400").await;
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
                    record_date: None,
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

    /// **Reproduction (TODO, `net_capital_gain` C2 follow-up).** A return of
    /// capital whose record date falls *before* a demerger and whose payment
    /// falls after it still reaches the **head** replacement parcel: the head
    /// listing continued, and the taxpayer was on its register when
    /// entitlement was fixed. The demerged entity's own payment, fixed at the
    /// same record date, does not reach the parcel it spun off into: those
    /// units are of a listing the taxpayer was not yet on the register of.
    ///
    /// Both parcels carry the same `demerger_action_id`, so the two halves of
    /// this are one question — which is why they are asserted together.
    #[tokio::test]
    async fn a_head_parcel_keeps_its_entitlement_to_a_return_of_capital_across_the_demerger() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "HEAD").await;
        insert_listing(&pool, 2, "DEM").await;
        insert_buy(&pool, 1, 1, d(2020, 10, 1), "1000", "1.50").await;

        // Record date 25 Jun, payment 1 Aug, demerger 1 Jul — inside the
        // window. Both listings pay 5c a unit off the same record date.
        for (id, listing_id) in [(5, 1), (6, 2)] {
            corporate_action::db_upsert(
                &pool,
                &CorporateAction {
                    id,
                    listing_id,
                    date: d(2024, 8, 1),
                    kind: ActionKind::ReturnOfCapital {
                        amount_per_unit: dec("0.05"),
                        currency: "AUD".to_string(),
                        record_date: Some(d(2024, 6, 25)),
                    },
                },
            )
            .await
            .unwrap();
        }
        insert_demerger(&pool, 10, d(2024, 7, 1)).await;
        let dm = db_demerge(&pool, 10).await.unwrap();

        // The payment is dated after the demerger, so it is not in the cost
        // base apportioned at it: head $1,200 + demerged $300 of the $1,500.
        assert_eq!(dm.head_replacements[0].brokerage, dec("1200"));
        assert_eq!(dm.demerged_replacements[0].brokerage, dec("300"));

        let open = crate::reports::open_parcels::db_open_parcels(&pool)
            .await
            .unwrap();
        let head = open
            .iter()
            .find(|p| p.listing_id == 1)
            .expect("the head replacement parcel is open");
        let demerged = open
            .iter()
            .find(|p| p.listing_id == 2)
            .expect("the demerged parcel is open");
        // 1,000 units × 5c against the head parcel. Nothing against the
        // demerged one: its 200 units only joined that register at the
        // demerger, a week after the record date fixed the entitlement.
        assert_eq!(head.return_of_capital_reduction, dec("50"));
        assert_eq!(head.remaining_cost_base, dec("1150"));
        assert_eq!(demerged.return_of_capital_reduction, Decimal::ZERO);
        assert_eq!(demerged.remaining_cost_base, dec("300"));
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

    /// Sell a demerged-entity parcel 6 months after the demerger, where the
    /// head parcel had been held 5 years: the new interests' 12-month clock
    /// runs from the *original* acquisition (the ATO's Example 32), so the
    /// gain is discount-eligible even though the parcel itself is 6 months
    /// old. A report anchoring on the replacement Buy's own trade date would
    /// call the whole gain non-discountable (SCENARIOS C-08).
    #[tokio::test]
    async fn demerged_parcel_sold_six_months_later_discounts_from_the_original_buy() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "HEAD").await;
        insert_listing(&pool, 2, "DEM").await;
        // 1,000 @ $1.50 = $1,500, held from 2019.
        insert_buy(&pool, 1, 1, d(2019, 6, 3), "1000", "1.50").await;
        insert_demerger(&pool, 10, d(2024, 7, 1)).await;
        let dm = db_demerge(&pool, 10).await.unwrap();

        // 1-for-5 → 200 demerged units carrying 20% of $1,500 = $300.
        let demerged = &dm.demerged_replacements[0];
        assert_eq!(demerged.quantity, dec("200"));
        assert_eq!(demerged.brokerage, dec("300"));
        assert_eq!(demerged.date, d(2024, 7, 1));
        assert_eq!(demerged.deemed_acquisition_date, Some(d(2019, 6, 3)));

        sell_listing(&pool, 20, 2, demerged.id, d(2025, 1, 6), "200", "5").await;

        let realised = crate::reports::realised_gains::db_realised_gains(&pool)
            .await
            .unwrap();
        // Only the sale — the demerge's own closing Sell is excluded.
        assert_eq!(realised.len(), 1);
        let g = &realised[0];
        assert_eq!(g.cost_base, dec("300"));
        assert_eq!(g.proceeds, dec("1000"));
        assert_eq!(g.capital_gain_loss, dec("700"));
        assert_eq!(g.discount_eligible_gain, dec("700"));
        assert_eq!(g.non_discountable_gain, Decimal::ZERO);
        assert_eq!(g.parcels[0].acquisition_date, d(2019, 6, 3));
    }

    /// A deemed acquisition date and a split have to survive *each other*:
    /// the demerged parcel carries a 2019 deemed date on a 2024 trade date,
    /// and a split on the demerged listing after the demerger re-bases its
    /// units. The quantity re-base keys off the replacement's own trade date
    /// (a split before the demerger is already reflected in the units it was
    /// created with and must not apply twice), while the discount clock keys
    /// off the deemed date (SCENARIOS C-15).
    #[tokio::test]
    async fn deemed_date_and_a_later_split_both_survive_on_a_replacement_parcel() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "HEAD").await;
        insert_listing(&pool, 2, "DEM").await;
        insert_buy(&pool, 1, 1, d(2019, 6, 3), "1000", "1.50").await;
        // A 2-for-1 split on the *head* listing before the demerger: the
        // parcel is 2,000 units at the demerger, so 1-for-5 gives 400
        // demerged units.
        corporate_action::db_upsert(
            &pool,
            &CorporateAction {
                id: 4,
                listing_id: 1,
                date: d(2021, 1, 1),
                kind: ActionKind::ShareSplit {
                    split_new_units: dec("2"),
                    split_old_units: dec("1"),
                },
            },
        )
        .await
        .unwrap();
        insert_demerger(&pool, 10, d(2024, 7, 1)).await;
        let dm = db_demerge(&pool, 10).await.unwrap();
        let demerged = &dm.demerged_replacements[0];
        assert_eq!(demerged.quantity, dec("400"));
        assert_eq!(demerged.brokerage, dec("300"));
        assert_eq!(demerged.deemed_acquisition_date, Some(d(2019, 6, 3)));

        // A 3-for-1 split on the *demerged* listing after the demerger:
        // 400 → 1,200 units.
        corporate_action::db_upsert(
            &pool,
            &CorporateAction {
                id: 11,
                listing_id: 2,
                date: d(2024, 10, 1),
                kind: ActionKind::ShareSplit {
                    split_new_units: dec("3"),
                    split_old_units: dec("1"),
                },
            },
        )
        .await
        .unwrap();
        sell_listing(&pool, 20, 2, demerged.id, d(2025, 1, 6), "1200", "1").await;

        let realised = crate::reports::realised_gains::db_realised_gains(&pool)
            .await
            .unwrap();
        assert_eq!(realised.len(), 1);
        let g = &realised[0];
        // The whole $300 carried cost base, spread over the split units —
        // the pre-demerger split is not re-applied on top.
        assert_eq!(g.cost_base, dec("300"));
        assert_eq!(g.proceeds, dec("1200"));
        assert_eq!(g.capital_gain_loss, dec("900"));
        assert_eq!(g.discount_eligible_gain, dec("900"));
        assert_eq!(g.parcels[0].acquisition_date, d(2019, 6, 3));
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

    /// …with exactly one exception: the stated pre-demerger close. It is a
    /// *price* fact — the demerge trades were created and validated against
    /// the entitlement ratio and the cost-base percentage, never against this
    /// — and without the exception the fix for the provider's spin-off price
    /// adjustment would be unreachable on every demerger that has actually
    /// been run, which is the only kind with prices to correct (Evan's live
    /// LAC demerger is one).
    #[tokio::test]
    async fn a_referenced_demerger_still_takes_its_stated_pre_demerger_close() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "LAC").await;
        insert_listing(&pool, 2, "LAR").await;
        insert_buy(&pool, 1, 1, d(2020, 10, 1), "1000", "1.50").await;
        insert_demerger(&pool, 10, d(2024, 7, 1)).await;
        db_demerge(&pool, 10).await.unwrap();

        // A pre-demerger close the provider served afterwards, so it carries
        // the spin-off adjustment.
        crate::test_support::closing_price(1, d(2024, 6, 28))
            .price("10.13")
            .fetched_at("2026-07-26T07:44:56Z")
            .insert(&pool)
            .await;

        let mut action = corporate_action::db_get(&pool, 10).await.unwrap().unwrap();
        let ActionKind::Demerger {
            demerger_close_date,
            demerger_close_price,
            demerger_close_sourced_from,
            demerger_close_reason,
            ..
        } = &mut action.kind
        else {
            unreachable!("the fixture records a Demerger")
        };
        *demerger_close_date = Some(d(2024, 6, 28));
        *demerger_close_price = Some("24.90".parse().unwrap());
        *demerger_close_sourced_from = Some("nyse.com daily close".to_string());
        *demerger_close_reason = Some("the provider adjusts the series".to_string());
        corporate_action::db_upsert(&pool, &action).await.unwrap();

        let stored: String = sqlx::query_scalar(
            "SELECT price FROM closing_prices WHERE listing_id = 1 AND price_date = '2024-06-28'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(
            stored, "24.9",
            "stating the close re-bases the listing's pre-demerger prices even though the \
             demerger has been run"
        );

        // Everything else about the action is still frozen: a re-PUT that
        // changes nothing, and one that changes a term, both stay 422.
        assert!(matches!(
            corporate_action::db_upsert(&pool, &action).await,
            Err(corporate_action::WriteError::ReferencedByTrade)
        ));
        let mut retermed = action.clone();
        if let ActionKind::Demerger {
            demerger_cost_base_pct,
            ..
        } = &mut retermed.kind
        {
            *demerger_cost_base_pct = "30".parse().unwrap();
        }
        assert!(matches!(
            corporate_action::db_upsert(&pool, &retermed).await,
            Err(corporate_action::WriteError::ReferencedByTrade)
        ));
        let mut moved = action.clone();
        moved.date = d(2024, 7, 2);
        assert!(matches!(
            corporate_action::db_upsert(&pool, &moved).await,
            Err(corporate_action::WriteError::ReferencedByTrade)
        ));
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

        let resp = client(&pool)
            .post_empty("/corporate_actions/10/demerge")
            .await;
        assert_eq!(resp.status, StatusCode::CREATED);
        let v: serde_json::Value = resp.json();
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

    /// "A replacement quantity no `Decimal` can hold". A 1000-for-1 demerger
    /// of 1e27 units asks for 1e30 demerged units — past `Decimal`'s ceiling
    /// however the arithmetic is ordered, so `mul_div`'s divide-early headroom
    /// cannot reach it and the write panicked, answering a logged `500` with
    /// an empty body. Refused `422` before anything is written, quoting the
    /// ratio and the holding that produced it.
    #[tokio::test]
    async fn api_an_unrepresentable_demerged_quantity_is_refused_naming_the_ratio() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "HEAD").await;
        insert_listing(&pool, 2, "DEM").await;
        // Nil-priced: the *cost base* is representable (W-e's bound accepts
        // this parcel), so only the demerged quantity is at the ceiling.
        insert_buy(&pool, 1, 1, d(2020, 10, 1), "1e27", "0").await;
        insert_demerger_terms(&pool, 10, 1, 2, d(2024, 7, 1), "1000", "1", "20").await;

        let err = db_demerge(&pool, 10).await.unwrap_err();
        assert!(
            matches!(err, DemergeError::UnrepresentableDemergedQuantity(_)),
            "expected the unrepresentable-quantity refusal, got: {err:?}"
        );
        let resp = client(&pool)
            .post_empty("/corporate_actions/10/demerge")
            .await;
        let (status, detail) = resp.status_and_body();
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
        assert!(
            detail.contains(
                "units held 1000000000000000000000000000 × demerger_new_units 1000 \
                 / demerger_held_units 1"
            ),
            "the ratio and the holding are not named: {detail}"
        );
        assert!(
            detail.contains(&Decimal::MAX.to_string()),
            "the limit is not named: {detail}"
        );
        // Nothing was written: the demerge is refused before its own rows.
        let created: bool =
            sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM trades WHERE demerger_action_id = 10)")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert!(!created);
    }

    /// SCENARIOS W-b's residual in the demerger, closed in the same
    /// treatment — and this one is a **write** path, so the panic aborted the
    /// whole operation rather than one report's read. Both of the operation's
    /// pro-rates multiplied before they divided, and each could pass
    /// `rust_decimal`'s ~7.9228e28 ceiling with a perfectly representable
    /// answer on the other side:
    ///
    /// * parcel 1 is costed at 1e27 (`average_price 1e27 × quantity 1`, which
    ///   the write path accepts — `checked_cost_base` bounds the cost base,
    ///   and 1e27 is well inside it): apportioning it at 99% overflows at
    ///   `1e27 × 99 = 9.9e28`, though the answer is 9.9e26;
    /// * parcel 2 holds 1e27 units: the entitlement ratio, written unreduced
    ///   as 100-for-1000, overflows at `1e27 × 100 = 1e29`, though the answer
    ///   is 1e26 units.
    ///
    /// Before the fix `POST /corporate_actions/{id}/demerge` answered a
    /// logged `500` with an empty body and wrote nothing.
    #[tokio::test]
    async fn api_demerge_past_the_old_multiply_first_ceiling_completes() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "HEAD").await;
        insert_listing(&pool, 2, "DEM").await;
        // Cost base 1e27 on one unit.
        insert_buy(&pool, 1, 1, d(2020, 10, 1), "1", "1e27").await;
        // 1e27 units at a cost base of 1e21 — the quantity, not the money, is
        // what overflows here.
        insert_buy(&pool, 2, 1, d(2021, 10, 1), "1e27", "0.000001").await;
        insert_demerger_terms(&pool, 10, 1, 2, d(2024, 7, 1), "100", "1000", "99").await;

        let resp = client(&pool)
            .post_empty("/corporate_actions/10/demerge")
            .await;
        assert_eq!(resp.status, StatusCode::CREATED);
        let v: serde_json::Value = resp.json();
        // The cost base rides on the replacement Buys' brokerage (price 0),
        // and the two legs still sum exactly to the parcel's own.
        assert_eq!(
            v["demerged_replacements"][0]["brokerage"],
            "990000000000000000000000000"
        );
        assert_eq!(
            v["head_replacements"][0]["brokerage"],
            "10000000000000000000000000"
        );
        assert_eq!(v["demerged_replacements"][0]["quantity"], "0.10");
        // Parcel 2: the entitlement ratio applied to 1e27 units.
        assert_eq!(
            v["demerged_replacements"][1]["quantity"],
            "100000000000000000000000000"
        );
        assert_eq!(
            v["demerged_replacements"][1]["brokerage"],
            "990000000000000000000.00000"
        );
        assert_eq!(
            v["head_replacements"][1]["brokerage"],
            "10000000000000000000.000000"
        );
    }

    #[tokio::test]
    async fn api_demerge_maps_errors_to_statuses() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "HEAD").await;
        insert_listing(&pool, 2, "DEM").await;

        // Missing action → 404.
        let resp = client(&pool)
            .post_empty("/corporate_actions/99/demerge")
            .await;
        assert_eq!(resp.status, StatusCode::NOT_FOUND);

        // Nothing held → 422.
        insert_demerger(&pool, 10, d(2024, 7, 1)).await;
        let resp = client(&pool)
            .post_empty("/corporate_actions/10/demerge")
            .await;
        assert_eq!(resp.status, StatusCode::UNPROCESSABLE_ENTITY);
    }

    /// A consumed parcel's deliberate spot-rate override carries onto both
    /// the head and demerged replacement Buys (like `fx_rate` and the deemed
    /// acquisition date), so the apportioned AUD cost bases are unchanged by
    /// the demerger.
    #[tokio::test]
    async fn demerge_carries_spot_fx_rate_onto_replacements() {
        let pool = test_pool().await;
        insert_listing_in(&pool, 1, "HEAD", "USD").await;
        insert_listing_in(&pool, 2, "DEM", "USD").await;
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

    /// SCENARIOS V-d, the demerger's own output: the demerged parcels are dated
    /// the demerger date on the **demerged** listing, so if that listing has
    /// since been written off — or taken over, or demerged — dated on or after
    /// this demerger, they would land behind an operation that consumed the
    /// whole holding and could never be consumed by it. Refused before anything
    /// is written; the head listing needs no such check, since
    /// `TradedOnOrAfterDemergerDate` already refuses any trade of it dated on
    /// or after the demerger.
    #[tokio::test]
    async fn demerge_into_a_listing_already_written_off_is_refused() {
        let pool = test_pool().await;
        test_support::recognised_worthless_listing(
            &pool,
            2,
            "SPIN",
            d(2024, 1, 2),
            90,
            d(2024, 9, 2),
        )
        .await;
        insert_listing(&pool, 1, "HEAD").await;
        test_support::buy(1, 1)
            .date(d(2024, 1, 2))
            .insert(&pool)
            .await;
        corporate_action::db_upsert(
            &pool,
            &CorporateAction {
                id: 10,
                listing_id: 1,
                date: d(2024, 6, 11),
                kind: ActionKind::Demerger {
                    demerger_listing_id: 2,
                    demerger_new_units: Decimal::ONE,
                    demerger_held_units: Decimal::from(5),
                    demerger_cost_base_pct: Decimal::from(10),
                    demerger_close_date: None,
                    demerger_close_price: None,
                    demerger_close_sourced_from: None,
                    demerger_close_reason: None,
                },
            },
        )
        .await
        .unwrap();

        let err = db_demerge(&pool, 10).await.unwrap_err();
        assert!(
            matches!(err, DemergeError::BackDatedOverWholeHolding(_)),
            "expected the whole-holding refusal, got: {err:?}"
        );
        let created: bool =
            sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM trades WHERE demerger_action_id = 10)")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert!(!created);
    }

    /// A `ShareSplit` on the **demerged** listing, dated after the demerger, so
    /// the demerged-entity parcels are re-based at read time by 1000-for-1.
    async fn split_the_demerged_listing(pool: &SqlitePool) {
        corporate_action::db_upsert(
            pool,
            &CorporateAction {
                id: 11,
                listing_id: 2,
                date: d(2024, 6, 1),
                kind: ActionKind::ShareSplit {
                    split_new_units: Decimal::from(1000),
                    split_old_units: Decimal::ONE,
                },
            },
        )
        .await
        .unwrap();
    }

    /// The demerger's version of the case the mirror rule was raised for: the
    /// **entitlement ratio is 1-for-1**, so the `UnrepresentableDemergedQuantity`
    /// check that asks about it is satisfied — and the demerged listing's own
    /// recorded 1000-for-1 split re-bases the demerged parcel past the range at
    /// read time. The demerge answered `201`, and `GET /portfolio/open-parcels`
    /// and `POST /portfolio/overview` were both a logged `500` afterwards. So
    /// the walk asks about the **destination** listing, not the head listing
    /// the operation is about.
    #[tokio::test]
    async fn api_a_demerged_parcel_the_demerged_listings_own_ratio_rebases_out_of_range() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "HEAD").await;
        insert_listing(&pool, 2, "SPIN").await;
        insert_buy(
            &pool,
            1,
            1,
            d(2024, 1, 15),
            "100000000000000000000000000",
            "0",
        )
        .await;
        insert_demerger_terms(&pool, 10, 1, 2, d(2024, 3, 15), "1", "1", "10").await;
        split_the_demerged_listing(&pool).await;

        let response = client(&pool)
            .post_empty("/corporate_actions/10/demerge")
            .await;
        let (status, detail) = response.status_and_body();
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{detail}");
        assert!(
            detail.contains("quantity 100000000000000000000000000 × new units 1000 / old units 1"),
            "the quantity and the ratio are not named: {detail}"
        );
        // Nothing was written — the whole operation rolled back.
        let n: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM trades")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(n, 1);
        let rows: Vec<serde_json::Value> = ApiClient::full(&pool)
            .get_json("/portfolio/open-parcels")
            .await;
        assert_eq!(rows.len(), 1, "{rows:?}");
    }

    /// The control, pinned at the figures this build answered before the
    /// refusal existed: the same 1-for-1 demerger of 7.9e25 units onto the same
    /// split-carrying listing re-bases to 7.9e28, inside the range, so it lands
    /// — and the head replacement, which no ratio of its own touches, keeps its
    /// 7.9e25 units and the other 90% of the cost base.
    #[tokio::test]
    async fn api_a_demerged_parcel_the_demerged_ratio_still_fits_lands_and_reports() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "HEAD").await;
        insert_listing(&pool, 2, "SPIN").await;
        test_support::buy(1, 1)
            .date(d(2024, 1, 15))
            .settlement(d(2024, 1, 15))
            .qty(dec("79000000000000000000000000"))
            .price(Decimal::ZERO)
            .brokerage(dec("1000"))
            .insert(&pool)
            .await;
        insert_demerger_terms(&pool, 10, 1, 2, d(2024, 3, 15), "1", "1", "10").await;
        split_the_demerged_listing(&pool).await;

        client(&pool)
            .post_empty("/corporate_actions/10/demerge")
            .await
            .expect_status(StatusCode::CREATED);

        let rows: Vec<serde_json::Value> = ApiClient::full(&pool)
            .get_json("/portfolio/open-parcels")
            .await;
        assert_eq!(rows.len(), 2, "{rows:?}");
        let head = rows.iter().find(|r| r["ticker"] == "HEAD").unwrap();
        assert_eq!(head["remaining_quantity"], "79000000000000000000000000");
        assert_eq!(head["original_cost_base"], "900");
        let spin = rows.iter().find(|r| r["ticker"] == "SPIN").unwrap();
        assert_eq!(spin["original_quantity"], "79000000000000000000000000");
        assert_eq!(spin["remaining_quantity"], "79000000000000000000000000000");
        assert_eq!(spin["original_cost_base"], "100");
    }
}
