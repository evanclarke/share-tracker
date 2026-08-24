//! Atomic DRP reinvestment: turn a distribution into a linked DRP trade.
//!
//! Given a distribution (an `income` row) on a DRP-enrolled holding and the
//! reinvestment price, this creates the reinvestment Trade (type `DRP`) and
//! links it back to the distribution (`income.reinvestment_trade_id`) in one
//! transaction. By default the reinvestable cash plus any residual brought
//! forward from the holding's previous reinvestment is spent on whole shares;
//! the leftover is carried forward or paid out per the enrolment period's
//! residual handling.
//!
//! A plan may instead **state the units it allotted** on its statement (a US
//! broker DRP allotting fractional shares always does). For those, the body's
//! optional `units` is the statement's figure and is authoritative: the trade
//! takes exactly that quantity, cross-checked against the available cash —
//! `units × price` must agree with it to within one unit-step at the units'
//! stated precision (a figure stated to 3 decimals must be within
//! 0.001 × price), which is the property any plan-computed allotment has
//! regardless of its rounding direction.
//!
//! What that allotment did not spend depends on how it was stated. **Whole
//! units** (no decimals) are an exact count: the plan bought whole units and
//! left the rest over, so — less than one unit's price, by the check above —
//! the difference is cash, carried or paid out per the period's handling
//! exactly as on the computed path (SCENARIOS I-06: it used to be discarded,
//! up to a share's worth). **Units stated to decimals** are a rounded
//! allotment: the plan applied all the cash and printed the units to its own
//! precision, so the difference is the printing rather than money and the
//! residual columns record zero — a real broker statement misses by several
//! cents that way, and carrying them would double-count cash the stated units
//! already include.
//!
//! The distribution must be recorded in the listing's own currency: the cash
//! and the per-unit price are two figures in one division, so a mismatch is
//! rejected rather than silently mixing currencies (SCENARIOS I-08).
//!
//! Enrolment is checked as at the distribution's ex date (registry practice:
//! DRP participation is fixed at the record date), falling back to the pay
//! date when no ex date is recorded. That date must fall inside one of the
//! enrolment periods *for the distribution's holding account*
//! (`entities::drp_enrolment` — enrolment is per (listing, holding account),
//! so an employer-plan account's distribution never reinvests off a personal
//! account's enrolment) — a distribution dated before enrolment, or in a gap
//! between unenrolment and re-enrolment, is rejected — and the matching
//! period's residual handling applies. The created DRP trade lands in the
//! distribution's holding account.
//!
//! The carried-forward residual is *not* stored as a separate running balance:
//! it lives on each DRP trade (`residual_carried_forward` +
//! `residual_paid_out`, which together are simply what that reinvestment left
//! over), and "brought forward" for the next reinvestment is the most recent
//! prior DRP trade's leftover *within the same enrolment period*. That single
//! source of truth can't drift, and the chain never crosses periods — a
//! period's trailing residual is paid out at unenrolment (see
//! `drp_enrolment::db_upsert`), not picked up after re-enrolment.
//!
//! Which of the two columns holds that leftover is never read as the answer,
//! because it depends on which trade is currently the chain's tail — and the
//! reinvestment being written is what takes the tail from it. The split is put
//! back to the period instead (`DrpEnrolment::residual_split`, the same rule
//! `drp_enrolment::recompute_residuals` walks the chain with), at the position
//! each trade occupies *after* this write: `Followed` for the prior trade,
//! `Tail` for the new one. Reading `residual_carried_forward` alone missed a
//! closed period's trailing trade, whose leftover the closure had settled to
//! `residual_paid_out` — a reinvestment entered into an already-closed period
//! brought forward nothing and came out a unit short, with the cash carried to
//! nobody (SCENARIOS V-e).
//!
//! Because each reinvestment reads that chain backwards, **reinvestments are
//! entered in payment order**: a reinvestment for which the period already
//! holds a DRP trade dated *after* it is refused
//! (`ReinvestError::LaterReinvestmentExists`). Entered mid-chain it would take
//! its brought-forward cash from a trade that has already handed the same cash
//! to a later one — spending it twice and leaving both parcels the wrong size
//! (SCENARIOS V-b). That is the create-side half of the rule undo enforces
//! from the other end (`ReinvestmentNotChainTail`, below): a reinvestment can
//! only join the chain at its tail and can only leave it from its tail. The
//! order is the **DRP trade's own date** (`trades.date` — the body's optional
//! `date`, else the distribution's `date_paid`), tie-broken by trade id, which
//! is the order this lookup and `drp_enrolment::recompute_residuals` both walk;
//! a *same-dated* reinvestment is allowed, since a registry can pay two
//! distributions on one day, and it joins the chain behind the trades already
//! dated that day (its id is the higher).
//!
//! A distribution may be reinvested at most once — re-posting is rejected
//! rather than creating a second trade.
//!
//! The inverse operation, `DELETE /income/:id/reinvest`, undoes a
//! reinvestment: it deletes the DRP trade and clears the distribution's link
//! in one transaction (the only path that clears it — `PUT /income` never
//! touches the link, and `DELETE /income` is refused while it is set, so an
//! orphaned DRP trade can't exist). Refused while the trade is drawn on (a
//! Sell allocation or AMIT adjustment references it) or while a later DRP
//! trade exists for the same listing and holding account — the chain reads
//! residuals back from the most recent prior trade, so undo runs
//! last-in-first-out.

use crate::entities::{
    drp_enrolment::{self, ResidualHandling},
    income::{Income, IncomeType},
    trade::{self, Trade},
};
use crate::infra::db::write_tx;
use crate::infra::decimal::{Money, parse_dec};
use crate::infra::http::ApiError;
use axum::{
    Json, Router,
    extract::{Path, State},
    http::StatusCode,
    routing::post,
};
use chrono::NaiveDate;
use rust_decimal::Decimal;
use serde::Deserialize;
use sqlx::SqlitePool;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReinvestBody {
    /// Per-share price the distribution is reinvested at.
    #[serde(deserialize_with = "crate::infra::decimal::strict_decimal")]
    pub reinvestment_price: Decimal,
    /// Optional broker-stated fractional allotment. When present it is
    /// authoritative — the trade takes exactly this quantity — and
    /// `units × reinvestment_price` is cross-checked against the available
    /// cash to within one unit-step at the stated precision. Omitted: whole
    /// shares with residual carry (the registry default).
    #[serde(
        default,
        deserialize_with = "crate::infra::decimal::strict_optional_decimal"
    )]
    pub units: Option<Decimal>,
    /// Optional foreign-per-AUD override for the created DRP trade (defaults to
    /// 1; reports prefer the ATO rate and fall back to this — see `infra::fx`).
    #[serde(
        default,
        deserialize_with = "crate::infra::decimal::strict_optional_decimal"
    )]
    pub fx_rate: Option<Decimal>,
    /// Optional trade date; defaults to the distribution's `date_paid`.
    #[serde(default)]
    pub date: Option<NaiveDate>,
}

#[derive(thiserror::Error, Debug)]
pub enum ReinvestError {
    #[error("DRP reinvestment write failed: {0}")]
    Db(#[from] sqlx::Error),
    /// No income row with that id.
    #[error("no distribution with that id")]
    IncomeNotFound,
    /// No enrolment period covers the distribution's ex date (or pay date when
    /// no ex date is recorded): never enrolled, dated before enrolment, or in a
    /// gap between unenrolment and re-enrolment. Carries the account name,
    /// ticker, and date so the rejection can name them rather than raw ids.
    #[error("account '{account}' is not enrolled in a DRP for {ticker} at {date}")]
    NotEnrolled {
        account: String,
        ticker: String,
        date: NaiveDate,
    },
    /// The distribution already has a reinvestment trade.
    #[error("this distribution already has a reinvestment trade")]
    AlreadyReinvested,
    /// The income row is not a distribution at all — an employment-income row
    /// (a dividend equivalent on unvested RSUs; TD 2017/26, SCENARIOS J-10) or
    /// an other-income row (a staking reward or established-token airdrop;
    /// SCENARIOS L-03). A DRP reinvests a payment *of* the holding into more of
    /// it; remuneration is paid for services and a staking reward is paid in
    /// the tokens themselves — no registry plan applies either to shares.
    /// Mapped to `422`.
    #[error("a non-distribution income row cannot be reinvested")]
    NotADistribution,
    /// The distribution's currency is not the listing's, so its cash and the
    /// reinvestment price are different money and cannot be divided.
    #[error("the distribution is in {distribution} but the listing trades in {listing}")]
    CurrencyMismatch {
        distribution: String,
        listing: String,
    },
    /// The reinvestment price is not strictly positive.
    #[error("the reinvestment price must be greater than zero")]
    NonPositivePrice,
    /// The stated units are not strictly positive.
    #[error("the stated units must be greater than zero")]
    NonPositiveUnits,
    /// The stated units don't spend the available cash at the given price:
    /// `units × price` differs from it by a full unit-step (at the units'
    /// stated precision) or more. Carries both figures for the rejection.
    #[error("the stated units spend {cost}, but the reinvestable cash is {available}")]
    UnitsCashMismatch { cost: Decimal, available: Decimal },
    /// `units × reinvestment_price` — the DRP parcel's cost base — cannot be
    /// represented (SCENARIOS W-e). Reachable only on the stated-units branch:
    /// the registry-default branch derives the quantity *from* the cash
    /// (`available / price`), so its product can never exceed the recorded
    /// distribution. Without the bound the multiply below panicked, answering
    /// a bare `500` with an empty body. Wording and limit in
    /// `domain::cost_base`, shared with `trade::check_amounts` — which this
    /// path does not go through, since the DRP trade is written directly.
    #[error("the reinvested parcel's cost base is not representable: {0}")]
    UnrepresentableCostBase(#[source] crate::domain::cost_base::UnrepresentableCost),
    /// Undo requested on a distribution with no reinvestment trade.
    #[error("this distribution has no reinvestment trade to undo")]
    NotReinvested,
    /// Undo refused: the DRP trade is drawn on by a Sell allocation or an
    /// AMIT adjustment — deleting it would orphan those dependants. Remove
    /// them first (e.g. delete the Sell via `DELETE /sells/:id`).
    #[error("the reinvestment trade is drawn on by a Sell allocation or AMIT adjustment")]
    ReinvestmentConsumed,
    /// Undo refused: a later DRP trade exists for the same listing and
    /// holding account. Its `residual_brought_forward` was read from this
    /// chain, so removing a mid-chain trade would falsify it — undo the later
    /// reinvestments first (LIFO).
    #[error("a later DRP reinvestment brought this trade's residual forward")]
    ReinvestmentNotChainTail,
    /// Create refused: the enrolment period already holds a DRP trade dated
    /// after the one this reinvestment would create, so it is not the chain's
    /// latest. Reinvesting mid-chain would take brought-forward cash the later
    /// trade has already spent — the sibling of `ReinvestmentNotChainTail`,
    /// which refuses the same falsification from the undo side (SCENARIOS
    /// V-b). Carries both dates so the rejection can name them.
    #[error("a DRP reinvestment dated {later} already follows this one, dated {date}")]
    LaterReinvestmentExists { date: NaiveDate, later: NaiveDate },
    /// Create refused: the DRP trade would be dated on or before an executed
    /// whole-holding operation of its listing — a scrip-for-scrip exchange, a
    /// demerger, or a worthless-shares recognise. Each consumed every parcel
    /// open at its own date, so units reinvested behind one can never be
    /// consumed and stay open forever (SCENARIOS V-d). The distribution's
    /// `date_paid` is only the *default* trade date, and the body may state an
    /// earlier one, so the check is on the date the trade will carry. The DRP
    /// side of `trade::UpsertError::BackDatedOverWholeHolding`; wording and
    /// recovery in `domain::whole_holding`. Mapped to 422.
    #[error("this parcel is dated behind a whole-holding operation: {0}")]
    BackDatedOverWholeHolding(#[source] crate::domain::whole_holding::BackDatedParcel),
    /// The reinvested quantity is one the listing's recorded splits and bonus
    /// issues re-base past what a `Decimal` can hold. The DRP side of
    /// `trade::UpsertError::UnrepresentableRebasedQuantity`, and reachable both
    /// ways the quantity is arrived at: a stated allotment is the taxpayer's
    /// own figure, and a derived one is `available / reinvestment_price`, which
    /// a nil-ish price makes as large as you like out of ordinary cash. Same
    /// walk, same wording (`corporate_action::rebased_quantity_beyond_range`).
    /// Mapped to 422.
    #[error("this parcel's quantity re-bases beyond the representable range: {0}")]
    UnrepresentableRebasedQuantity(#[source] crate::domain::cost_base::UnrepresentableQuantity),
}

impl From<ReinvestError> for ApiError {
    fn from(e: ReinvestError) -> Self {
        match e {
            ReinvestError::IncomeNotFound => ApiError::not_found("no distribution with that id"),
            ReinvestError::NotEnrolled {
                account,
                ticker,
                date,
            } => ApiError::unprocessable(format!(
                "account '{account}' is not enrolled in a DRP for {ticker} at {date} \
                     — enrol it on the DRP enrolments screen first"
            )),
            ReinvestError::AlreadyReinvested => ApiError::unprocessable(
                "this distribution already has a reinvestment trade — undo it first \
                 (DELETE /income/:id/reinvest) to redo it",
            ),
            ReinvestError::NotADistribution => ApiError::unprocessable(
                "this income row is not a distribution — a DRP reinvests a payment of the \
                 holding into more of it, and neither employment income (a dividend \
                 equivalent, paid for services) nor other income (a staking reward or \
                 airdrop, already paid in the tokens) is one",
            ),
            ReinvestError::CurrencyMismatch {
                distribution,
                listing,
            } => ApiError::unprocessable(format!(
                "this distribution is recorded in {distribution} but the listing trades in \
                 {listing}, so its cash cannot be divided by a {listing} reinvestment price — \
                 enter the distribution in {listing} (a registry reinvesting a foreign-currency \
                 payment converts it, and the statement prints the converted figure)"
            )),
            ReinvestError::NonPositivePrice => {
                ApiError::unprocessable("the reinvestment price must be greater than zero")
            }
            ReinvestError::NonPositiveUnits => {
                ApiError::unprocessable("the stated units must be greater than zero")
            }
            ReinvestError::UnrepresentableCostBase(e) => ApiError::unprocessable(e.message()),
            ReinvestError::UnitsCashMismatch { cost, available } => {
                ApiError::unprocessable(format!(
                    "the stated units at the reinvestment price spend {cost}, but the \
                     reinvestable cash (including any residual brought forward) is {available} \
                     — they must agree to within one unit-step at the stated precision"
                ))
            }
            ReinvestError::NotReinvested => {
                ApiError::unprocessable("this distribution has no reinvestment trade to undo")
            }
            ReinvestError::ReinvestmentConsumed => ApiError::unprocessable(
                "the reinvestment trade is drawn on by a Sell allocation or AMIT adjustment \
                 — remove those first (e.g. delete the Sell via DELETE /sells/:id)",
            ),
            ReinvestError::ReinvestmentNotChainTail => ApiError::unprocessable(
                "a later DRP reinvestment for this listing and holding account brought this \
                 trade's residual forward — undo the later reinvestments first",
            ),
            ReinvestError::LaterReinvestmentExists { date, later } => {
                ApiError::unprocessable(format!(
                    "this reinvestment would be dated {date}, but a DRP reinvestment dated \
                     {later} already exists in the same enrolment period and holding account \
                     — each reinvestment brings its cash forward from the one before it, so \
                     they are entered in payment order; undo the later reinvestments \
                     (DELETE /income/:id/reinvest) and re-enter them in date order"
                ))
            }
            // The same body every parcel-creating path answers for this fact.
            ReinvestError::BackDatedOverWholeHolding(e) => ApiError::Unprocessable(e.message()),
            ReinvestError::UnrepresentableRebasedQuantity(e) => {
                ApiError::Unprocessable(e.message())
            }
            ReinvestError::Db(err) => err.into(),
        }
    }
}

pub fn router() -> Router<SqlitePool> {
    Router::new().route("/income/{id}/reinvest", post(reinvest).delete(unreinvest))
}

/// Create the DRP trade for a distribution and link it, atomically.
pub async fn db_reinvest(
    pool: &SqlitePool,
    income_id: i64,
    body: &ReinvestBody,
) -> Result<Trade, ReinvestError> {
    if body.reinvestment_price <= Decimal::ZERO {
        return Err(ReinvestError::NonPositivePrice);
    }
    if let Some(units) = body.units
        && units <= Decimal::ZERO
    {
        return Err(ReinvestError::NonPositiveUnits);
    }

    let mut tx = write_tx(pool).await?;

    // Load the distribution and its cash components.
    let income: Option<Income> = sqlx::query_as("SELECT * FROM income WHERE id = ?")
        .bind(income_id)
        .fetch_optional(&mut *tx)
        .await?;
    let income = match income {
        Some(r) => r,
        None => return Err(ReinvestError::IncomeNotFound),
    };

    if income.reinvestment_trade_id.is_some() {
        return Err(ReinvestError::AlreadyReinvested);
    }
    if income.income_type != IncomeType::Dividend {
        return Err(ReinvestError::NotADistribution);
    }

    let Income {
        listing_id,
        holding_account_id,
        date_paid,
        ..
    } = income;
    let cash = income.net_cash_received();

    // Reinvestability is decided as at the ex date (DRP participation is fixed
    // at the record date), falling back to the pay date when not recorded —
    // the model's own `ex_or_pay_date`.
    let entitlement_date = income.ex_or_pay_date();

    // That date must fall inside an enrolment period *for the distribution's
    // holding account* — half-open [enrolment_date, unenrolment_date),
    // open-ended when NULL — and the matching period decides what happens to
    // the leftover. No match means that account's holding wasn't enrolled
    // when the distribution went ex (never enrolled, before enrolment, in an
    // unenrolment gap — or only ever enrolled in a different account, e.g. a
    // personal account's enrolment never reinvests an employer-plan
    // distribution).
    let matched: Option<(ResidualHandling, NaiveDate, Option<NaiveDate>)> = sqlx::query_as(
        "SELECT residual_handling, enrolment_date, unenrolment_date FROM drp_enrolments \
         WHERE listing_id = ? AND holding_account_id = ? AND enrolment_date <= ? \
           AND (unenrolment_date IS NULL OR ? < unenrolment_date)",
    )
    .bind(listing_id)
    .bind(holding_account_id)
    .bind(entitlement_date)
    .bind(entitlement_date)
    .fetch_optional(&mut *tx)
    .await?;
    let (handling, period_start, period_end) = match matched {
        Some(p) => p,
        None => {
            // Name the account and listing in the rejection so the user knows
            // exactly what to enrol — never echo the raw foreign-key ids.
            let account: String =
                sqlx::query_scalar("SELECT name FROM holding_accounts WHERE id = ?")
                    .bind(holding_account_id)
                    .fetch_one(&mut *tx)
                    .await?;
            let ticker: String = sqlx::query_scalar("SELECT ticker FROM listings WHERE id = ?")
                .bind(listing_id)
                .fetch_one(&mut *tx)
                .await?;
            return Err(ReinvestError::NotEnrolled {
                account,
                ticker,
                date: entitlement_date,
            });
        }
    };

    // The period as it now stands. It is the only thing that decides where any
    // of its leftovers sits, and it is asked three times below: for what the
    // trade before this one hands forward, for this one's own split, and
    // finally to re-derive the whole chain once this one has joined it.
    let period = drp_enrolment::DrpEnrolment {
        id: 0, // not read: every walk is keyed on the period's own shape
        listing_id,
        holding_account_id,
        enrolment_date: period_start,
        unenrolment_date: period_end,
        residual_handling: handling,
    };

    // The DRP trade is denominated in the holding's currency.
    let currency: String = sqlx::query_scalar("SELECT currency FROM listings WHERE id = ?")
        .bind(listing_id)
        .fetch_one(&mut *tx)
        .await?;

    // The reinvestment is one calculation over two figures — the
    // distribution's cash and the plan's per-unit price — so they must be the
    // same money. The price is the listing's currency (it is what the trade is
    // stamped with); the cash is the income row's own. Dividing one by the
    // other unconverted would silently cost the parcel in the wrong currency
    // (US$100 ÷ A$7 → 14 units costed A$98), which is exactly the mixing
    // CLAUDE.md forbids. A registry that pays a foreign-currency distribution
    // into a plan converts it itself and prints the converted figure, so the
    // entry to correct is the income row's (SCENARIOS I-06, I-08).
    if income.currency != currency {
        return Err(ReinvestError::CurrencyMismatch {
            distribution: income.currency.clone(),
            listing: currency,
        });
    }

    // The date the DRP trade will carry. Known here rather than at the INSERT
    // because it is the axis the residual chain is ordered on, and the two
    // checks below are both about where in that chain this reinvestment lands.
    // DRP units are issued by the registry, not market-settled, so it is the
    // settlement date too.
    let date = body.date.unwrap_or(date_paid);

    // A whole-holding operation of this listing that has already run consumed
    // every parcel open at its own date and cannot reach back for this one, so
    // a DRP dated on or before it would stay open forever (SCENARIOS V-d).
    // Checked on `date` — the date the trade will carry, which the body may
    // state ahead of the distribution's own `date_paid` default
    // (`domain::whole_holding`).
    if let Some(back_dated) =
        crate::domain::whole_holding::db_back_dated_parcel(&mut tx, listing_id, date, None).await?
    {
        return Err(ReinvestError::BackDatedOverWholeHolding(back_dated));
    }

    // Reinvestments are entered in payment order. Which trades are *in* the
    // period is the distribution's entitlement date
    // (`drp_enrolment::PERIOD_TRADES_FROM_WHERE`), but the residual chain runs
    // in the order the cash moved — the DRP trade's own date, which is what
    // the lookup below and `drp_enrolment::recompute_residuals` both walk. A
    // reinvestment slipped in *behind* one already recorded would read its
    // brought-forward cash from a trade that has already handed the same cash
    // to that later one: the cash is spent twice and both parcels take the
    // wrong quantity, with nothing to surface it afterwards (SCENARIOS V-b).
    // So refuse, exactly as undo refuses a mid-chain delete
    // (`ReinvestmentNotChainTail`) for the same reason: the chain is joined at
    // its tail and left from its tail. Strictly later only — a registry can
    // pay two distributions on one day, and a same-dated reinvestment joins
    // the chain *behind* them, since the row about to be inserted takes an id
    // above every existing one and the order is (date, id). The date named is
    // the earliest offender: the reinvestment that would have picked this
    // one's leftover up.
    let later: Option<NaiveDate> = sqlx::query_scalar(sqlx::AssertSqlSafe(format!(
        "SELECT t.date {} AND t.date > ? ORDER BY t.date, t.id LIMIT 1",
        *drp_enrolment::PERIOD_TRADES_FROM_WHERE
    )))
    .bind(listing_id)
    .bind(holding_account_id)
    .bind(period_start)
    .bind(period_end)
    .bind(period_end)
    .bind(date)
    .fetch_optional(&mut *tx)
    .await?;
    if let Some(later) = later {
        return Err(ReinvestError::LaterReinvestmentExists { date, later });
    }

    // Residual brought forward = what the most recent prior DRP trade leaves
    // over, *within the same enrolment period and holding account*: an earlier
    // period's trailing residual was paid out at its unenrolment, and another
    // account runs its own chain, so the chain never crosses a period boundary
    // or an account boundary. Membership is the period's own question
    // (`drp_enrolment::PERIOD_TRADES_FROM_WHERE`, matched on each
    // distribution's entitlement date); "most recent" is the payment order the
    // cash actually moved in, bounded to trades dated at or before this one so
    // the lookup can never read *forward* in time. "At or before" is strictly
    // before on the chain's (date, id) order — the new row's id exceeds every
    // existing one — and the refusal above is what makes the bound unreachable
    // rather than merely defensive.
    //
    // What that trade *leaves over* is both its residual columns summed: which
    // of the two currently holds it depends on whether it is the chain's tail,
    // and the row about to be inserted is what takes the tail from it. Reading
    // `residual_carried_forward` alone got a closed period's trailing trade
    // wrong — its leftover had been settled to `residual_paid_out` when the
    // period closed, so a reinvestment entered afterwards brought forward
    // nothing and came out a unit short, with the cash carried to nobody
    // (SCENARIOS V-e). So the split is not read off the row: it is put back to
    // the period, at the position the prior trade is about to occupy.
    let prior: Option<(String, String)> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT t.residual_carried_forward, t.residual_paid_out {} AND t.date <= ? \
         ORDER BY t.date DESC, t.id DESC LIMIT 1",
        *drp_enrolment::PERIOD_TRADES_FROM_WHERE
    )))
    .bind(listing_id)
    .bind(holding_account_id)
    .bind(period_start)
    .bind(period_end)
    .bind(period_end)
    .bind(date)
    .fetch_optional(&mut *tx)
    .await?;
    let residual_bf = match prior {
        Some((carried, paid)) => {
            let leftover = parse_dec("residual_carried_forward", carried)?
                + parse_dec("residual_paid_out", paid)?;
            period
                .residual_split(leftover, drp_enrolment::ChainPosition::Followed)
                .carried_forward
        }
        None => Decimal::ZERO,
    };

    let available = cash + residual_bf;
    let (quantity, leftover) = match body.units {
        // Statement-stated allotment: the figure is authoritative, checked
        // against the cash to within one unit-step at its stated precision
        // (`step × price`) — the right bound for both kinds of statement: a
        // fractional plan's product misses the cash by a sliver of a step,
        // and a whole-share plan always leaves less than one unit's price
        // over. Beyond it the two figures disagree about something that is
        // neither rounding nor a leftover.
        Some(units) => {
            // The stated allotment's cost — and the parcel's cost base. Both
            // figures are the taxpayer's, so the product is bounded by nothing
            // but the type: computed checked, so an unrepresentable pair is a
            // 422 naming it rather than a panic inside this multiply
            // (SCENARIOS W-e).
            let cost = crate::domain::cost_base::checked_cost_base(&[
                crate::domain::cost_base::Term::Product {
                    price: ("reinvestment_price", body.reinvestment_price),
                    units: ("units", units),
                },
            ])
            .map_err(ReinvestError::UnrepresentableCostBase)?;
            let step = Decimal::new(1, units.scale());
            if (available - cost).abs() >= step * body.reinvestment_price {
                return Err(ReinvestError::UnitsCashMismatch { cost, available });
            }
            // Whether the difference is *cash* or *rounding* is decided by
            // what the stated units are. A whole number (scale 0) is an exact
            // allotment: the plan bought whole units and left the rest over,
            // so — bounded by the check above at less than one unit's price —
            // the difference is that leftover, cash to be carried or refunded
            // (SCENARIOS I-06). A figure stated to decimals is a *rounded*
            // allotment: the plan applied all the cash and printed the units
            // to its own precision, so the difference is the printing, not
            // money. It stays zero, as it always has — a real statement
            // (`morgan_stanley_ice_fractional_statements_reproduce`) misses by
            // several cents that way, and carrying them would double-count
            // cash the stated units already include.
            let leftover = match units.scale() {
                0 => (available - cost).max(Decimal::ZERO),
                _ => Decimal::ZERO,
            };
            (units, leftover)
        }
        // Registry default: spend the available cash on whole shares. The
        // leftover here is exact arithmetic on the recorded cash — nothing was
        // rounded to reach it, so it is carried exactly.
        None => {
            let quantity = (available / body.reinvestment_price).floor();
            (quantity, available - quantity * body.reinvestment_price)
        }
    };
    // Whichever way the units were arrived at, the leftover is cash the plan
    // did not spend, so where it sits is the period's decision — asked for at
    // the position this reinvestment takes, which is the chain's tail (the
    // refusal above is what guarantees that). Under `PayOut`, and under
    // `CarryForward` in a period that has already ended, that means it is
    // refunded here rather than a moment later by the recompute below
    // (SCENARIOS I-06: a whole-number stated allotment used to discard it —
    // up to a share's worth of cash, neither carried nor refunded).
    let split = period.residual_split(leftover, drp_enrolment::ChainPosition::Tail);

    let fx_rate = body.fx_rate.unwrap_or(Decimal::ONE);

    let result = sqlx::query(
        "INSERT INTO trades \
         (trade_type, date, settlement_date, settlement_date_source, listing_id, \
          average_price, quantity, currency, \
          brokerage, gst_on_brokerage, brokerage_currency, fx_rate, contract_note_ref, \
          residual_brought_forward, residual_carried_forward, residual_paid_out, \
          holding_account_id) \
         VALUES ('DRP', ?, ?, 'stated', ?, ?, ?, ?, '0', '0', ?, ?, NULL, ?, ?, ?, ?)",
    )
    .bind(date)
    .bind(date)
    .bind(listing_id)
    .bind(Money(body.reinvestment_price))
    .bind(Money(quantity))
    .bind(&currency)
    .bind(&currency)
    .bind(Money(fx_rate))
    .bind(Money(residual_bf))
    .bind(Money(split.carried_forward))
    .bind(Money(split.paid_out))
    .bind(holding_account_id)
    .execute(&mut *tx)
    .await?;
    let new_id = result.last_insert_rowid();

    sqlx::query("UPDATE income SET reinvestment_trade_id = ? WHERE id = ?")
        .bind(new_id)
        .bind(income_id)
        .execute(&mut *tx)
        .await?;

    // The new reinvestment has taken over as the chain's tail, so re-derive
    // the period's whole split: this one's own columns were already written
    // from the same rule, and what this fixes is the trade *behind* it, whose
    // leftover a closed period had settled to `residual_paid_out` and which is
    // now carried forward — into this trade, which brought exactly that figure
    // forward above. The two agree because they ask the same function.
    drp_enrolment::recompute_residuals(&mut tx, &period).await?;

    // The listing's recorded splits and bonus issues are re-applied at *read*
    // time, so a reinvested quantity they push past `Decimal`'s range is
    // accepted here and then answers a logged `500` from every open-holdings
    // report of the whole portfolio. Checked over the state this write leaves
    // behind, like every other parcel-creating path
    // (`corporate_action::rebased_quantity_beyond_range`).
    if let Some(beyond) =
        crate::entities::corporate_action::rebased_quantity_beyond_range(&mut tx, listing_id)
            .await?
    {
        return Err(ReinvestError::UnrepresentableRebasedQuantity(beyond));
    }

    tx.commit().await?;

    // Read the freshly created trade back so the response is exactly what was stored.
    trade::db_get(pool, new_id)
        .await?
        .ok_or_else(|| ReinvestError::Db(sqlx::Error::RowNotFound))
}

/// Undo a reinvestment: delete the DRP trade and clear the distribution's
/// link, atomically. The inverse of [`db_reinvest`] — after it the
/// distribution can be reinvested again.
pub async fn db_unreinvest(pool: &SqlitePool, income_id: i64) -> Result<(), ReinvestError> {
    let mut tx = write_tx(pool).await?;

    let link: Option<Option<i64>> =
        sqlx::query_scalar("SELECT reinvestment_trade_id FROM income WHERE id = ?")
            .bind(income_id)
            .fetch_optional(&mut *tx)
            .await?;
    let trade_id = match link {
        None => return Err(ReinvestError::IncomeNotFound),
        Some(None) => return Err(ReinvestError::NotReinvested),
        Some(Some(id)) => id,
    };

    // The trade must not be drawn on: a Sell allocation or AMIT adjustment
    // referencing it would be orphaned by the delete (the same dependants
    // `trade::db_delete` guards).
    let consumed: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM parcel_allocations WHERE purchase_trade_id = ?1) \
             OR EXISTS(SELECT 1 FROM amit_adjustments WHERE trade_id = ?1)",
    )
    .bind(trade_id)
    .fetch_one(&mut *tx)
    .await?;
    if consumed {
        return Err(ReinvestError::ReinvestmentConsumed);
    }

    // Undo is last-in-first-out: a later DRP trade for the same listing and
    // account read its residual_brought_forward back from the chain this
    // trade is part of (see the module doc), so a mid-chain trade can't be
    // removed. Ordered by (date, id), matching db_reinvest's chain lookup.
    let (listing_id, holding_account_id, date): (i64, i64, NaiveDate) =
        sqlx::query_as("SELECT listing_id, holding_account_id, date FROM trades WHERE id = ?")
            .bind(trade_id)
            .fetch_one(&mut *tx)
            .await?;
    let has_later: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM trades \
                       WHERE listing_id = ? AND holding_account_id = ? AND trade_type = 'DRP' \
                         AND (date > ? OR (date = ? AND id > ?)))",
    )
    .bind(listing_id)
    .bind(holding_account_id)
    .bind(date)
    .bind(date)
    .bind(trade_id)
    .fetch_one(&mut *tx)
    .await?;
    if has_later {
        return Err(ReinvestError::ReinvestmentNotChainTail);
    }

    // Clear the link before deleting the trade so the FK never dangles.
    sqlx::query("UPDATE income SET reinvestment_trade_id = NULL WHERE id = ?")
        .bind(income_id)
        .execute(&mut *tx)
        .await?;
    sqlx::query("DELETE FROM trades WHERE id = ?")
        .bind(trade_id)
        .execute(&mut *tx)
        .await?;

    tx.commit().await?;
    Ok(())
}

async fn reinvest(
    State(pool): State<SqlitePool>,
    Path(income_id): Path<i64>,
    Json(body): Json<ReinvestBody>,
) -> Result<(StatusCode, Json<Trade>), ApiError> {
    let trade = db_reinvest(&pool, income_id, &body).await?;
    Ok((StatusCode::CREATED, Json(trade)))
}

async fn unreinvest(
    State(pool): State<SqlitePool>,
    Path(income_id): Path<i64>,
) -> Result<StatusCode, ApiError> {
    db_unreinvest(&pool, income_id).await?;
    Ok(StatusCode::NO_CONTENT)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::entities::{drp_enrolment, income, listing, trade::TradeType};
    use crate::test_support::{self, ApiClient, test_pool};

    /// Client over this module's own routes.
    fn client(pool: &SqlitePool) -> ApiClient {
        ApiClient::over(router().with_state(pool.clone()))
    }

    async fn insert_listing(pool: &SqlitePool, id: i64, currency: &str) {
        test_support::listing(id)
            .security_type(listing::SecurityType::Trust)
            .currency(currency)
            .insert(pool)
            .await;
    }

    /// Create an enrolment period `[from, to)`; `to = None` = open-ended.
    async fn enrol_period(
        pool: &SqlitePool,
        id: i64,
        listing_id: i64,
        from: &str,
        to: Option<&str>,
        handling: ResidualHandling,
    ) {
        drp_enrolment::db_upsert(
            pool,
            &drp_enrolment::DrpEnrolment {
                holding_account_id: 1,
                id,
                listing_id,
                enrolment_date: from.parse().unwrap(),
                unenrolment_date: to.map(|d| d.parse().unwrap()),
                residual_handling: handling,
            },
        )
        .await
        .unwrap();
    }

    /// Enrol open-ended from 2024-01-01, covering the default distribution date.
    async fn enrol(pool: &SqlitePool, listing_id: i64, handling: ResidualHandling) {
        enrol_period(pool, listing_id, listing_id, "2024-01-01", None, handling).await;
    }

    /// Insert a distribution paying `cash` as unfranked cash (the simplest cash
    /// component), with `franking` notional franking credits that must be ignored.
    async fn insert_distribution(
        pool: &SqlitePool,
        id: i64,
        listing_id: i64,
        cash: Decimal,
        franking: Decimal,
    ) {
        insert_distribution_dated(pool, id, listing_id, "2024-03-31", None, cash, franking).await;
    }

    async fn insert_distribution_dated(
        pool: &SqlitePool,
        id: i64,
        listing_id: i64,
        date_paid: &str,
        ex_date: Option<&str>,
        cash: Decimal,
        franking: Decimal,
    ) {
        // In the listing's own currency: a distribution is paid in the money
        // the holding trades in, and reinvesting one that is not is refused
        // (`ReinvestError::CurrencyMismatch`).
        let currency: String = sqlx::query_scalar("SELECT currency FROM listings WHERE id = ?")
            .bind(listing_id)
            .fetch_one(pool)
            .await
            .unwrap();
        test_support::income(id, listing_id, date_paid.parse().unwrap())
            .with(|i| {
                i.ex_date = ex_date.map(|d| d.parse().unwrap());
                i.unfranked_amount = cash;
                i.franking_credits = franking;
                i.trust_income = true;
                i.currency = currency;
            })
            .insert(pool)
            .await;
    }

    fn body(price: &str) -> ReinvestBody {
        ReinvestBody {
            reinvestment_price: price.parse().unwrap(),
            units: None,
            fx_rate: None,
            date: None,
        }
    }

    /// Body with the broker's stated fractional allotment.
    fn body_units(price: &str, units: &str) -> ReinvestBody {
        ReinvestBody {
            units: Some(units.parse().unwrap()),
            ..body(price)
        }
    }

    /// A second holding account (e.g. an employer share plan) for the
    /// per-account tests.
    async fn insert_account(pool: &SqlitePool, id: i64, name: &str) {
        crate::entities::holding_account::db_upsert(
            pool,
            &crate::entities::holding_account::HoldingAccount {
                id,
                name: name.to_string(),
            },
        )
        .await
        .unwrap();
    }

    /// Open-ended enrolment from 2024-01-01 in a specific holding account.
    async fn enrol_in_account(
        pool: &SqlitePool,
        id: i64,
        listing_id: i64,
        account_id: i64,
        handling: ResidualHandling,
    ) {
        drp_enrolment::db_upsert(
            pool,
            &drp_enrolment::DrpEnrolment {
                id,
                listing_id,
                holding_account_id: account_id,
                enrolment_date: "2024-01-01".parse().unwrap(),
                unenrolment_date: None,
                residual_handling: handling,
            },
        )
        .await
        .unwrap();
    }

    /// Distribution of `cash` paid to a specific holding account.
    async fn insert_distribution_in_account(
        pool: &SqlitePool,
        id: i64,
        listing_id: i64,
        account_id: i64,
        cash: Decimal,
    ) {
        test_support::income(id, listing_id, "2024-03-31".parse().unwrap())
            .with(|i| {
                i.holding_account_id = account_id;
                i.unfranked_amount = cash;
                i.trust_income = true;
            })
            .insert(pool)
            .await;
    }

    #[tokio::test]
    async fn carry_forward_buys_whole_shares_and_carries_leftover() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "AUD").await;
        enrol(&pool, 1, ResidualHandling::CarryForward).await;
        // $100 cash + $30 notional franking credits (must be ignored), price $9.
        insert_distribution(&pool, 1, 1, Decimal::from(100), Decimal::from(30)).await;

        let trade = db_reinvest(&pool, 1, &body("9")).await.unwrap();

        // floor(100 / 9) = 11 shares, cost 99, leftover 1 carried forward.
        assert_eq!(trade.trade_type, TradeType::DRP);
        assert_eq!(trade.quantity, Decimal::from(11));
        assert_eq!(trade.average_price, Decimal::from(9));
        assert_eq!(trade.residual_brought_forward, Decimal::ZERO);
        assert_eq!(trade.residual_carried_forward, Decimal::ONE);
        assert_eq!(trade.residual_paid_out, Decimal::ZERO);

        // The distribution is now linked to the new trade.
        let inc = income::db_get(&pool, 1).await.unwrap().unwrap();
        assert_eq!(inc.reinvestment_trade_id, Some(trade.id));
    }

    /// Each DRP allotment is a separate acquisition running its own 12-month
    /// clock from its allotment date (`docs/ato/cgt-dividend-reinvestment-plans.md`)
    /// — the clock is not inherited from the holding the distribution was paid
    /// on. Two quarterly reinvestments sold together on 2025-06-25 straddle
    /// the line: the March parcel is over 12 months, the June parcel is
    /// 11 months and 25 days old and is not (SCENARIOS C-14).
    #[tokio::test]
    async fn each_reinvestment_parcel_runs_its_own_discount_clock() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "AUD").await;
        enrol(&pool, 1, ResidualHandling::CarryForward).await;
        // $1,000 reinvested at $10 in each of two quarters → 100 units each.
        insert_distribution_dated(
            &pool,
            1,
            1,
            "2024-03-31",
            None,
            Decimal::from(1000),
            Decimal::ZERO,
        )
        .await;
        insert_distribution_dated(
            &pool,
            2,
            1,
            "2024-06-30",
            None,
            Decimal::from(1000),
            Decimal::ZERO,
        )
        .await;
        let march = db_reinvest(&pool, 1, &body("10")).await.unwrap();
        let june = db_reinvest(&pool, 2, &body("10")).await.unwrap();
        assert_eq!(
            march.date,
            "2024-03-31".parse::<chrono::NaiveDate>().unwrap()
        );
        assert_eq!(
            june.date,
            "2024-06-30".parse::<chrono::NaiveDate>().unwrap()
        );
        assert_eq!(march.quantity, Decimal::from(100));
        assert_eq!(june.quantity, Decimal::from(100));

        // Both parcels sold on 2025-06-25 at $15.
        crate::entities::sell::db_upsert_sell(
            &pool,
            50,
            &crate::entities::sell::SellBody {
                brokerage_includes_gst: false,
                statement_total: None,
                holding_account_id: 1,
                date: "2025-06-25".parse().unwrap(),
                settlement_date: Some("2025-06-27".parse().unwrap()),
                listing_id: 1,
                average_price: Decimal::from(15),
                quantity: Decimal::from(200),
                currency: "AUD".to_string(),
                brokerage: Decimal::ZERO,
                gst_on_brokerage: Decimal::ZERO,
                brokerage_currency: "AUD".to_string(),
                fx_rate: Decimal::ONE,
                spot_fx_rate: None,
                contract_note_ref: None,
                allocations: vec![
                    crate::entities::sell::AllocationInput {
                        purchase_trade_id: march.id,
                        quantity_allocated: Decimal::from(100),
                    },
                    crate::entities::sell::AllocationInput {
                        purchase_trade_id: june.id,
                        quantity_allocated: Decimal::from(100),
                    },
                ],
            },
        )
        .await
        .unwrap();

        let realised = crate::reports::realised_gains::db_realised_gains(&pool)
            .await
            .unwrap();
        assert_eq!(realised.len(), 1);
        let g = &realised[0];
        assert_eq!(g.cost_base, Decimal::from(2000));
        assert_eq!(g.capital_gain_loss, Decimal::from(1000));
        // The March parcel's $500 discounts; the June parcel's $500 — five
        // days short of its own anniversary — does not.
        assert_eq!(g.discount_eligible_gain, Decimal::from(500));
        assert_eq!(g.non_discountable_gain, Decimal::from(500));
        let eligible: Vec<_> = g
            .parcels
            .iter()
            .map(|p| (p.purchase_trade_id, p.discount_eligible))
            .collect();
        assert_eq!(eligible, vec![(march.id, true), (june.id, false)]);
    }

    /// Operation-created trades take no part in GST-inclusive entry or the
    /// statement cross-check: a reinvestment trade reads back with the flag
    /// off and no statement total (the columns' defaults).
    #[tokio::test]
    async fn reinvestment_trade_is_not_gst_flagged_and_has_no_statement_total() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "AUD").await;
        enrol(&pool, 1, ResidualHandling::CarryForward).await;
        insert_distribution(&pool, 1, 1, Decimal::from(100), Decimal::ZERO).await;

        let trade = db_reinvest(&pool, 1, &body("9")).await.unwrap();
        let stored = crate::entities::trade::db_get(&pool, trade.id)
            .await
            .unwrap()
            .unwrap();
        assert!(!stored.brokerage_includes_gst);
        assert_eq!(stored.statement_total, None);
    }

    #[tokio::test]
    async fn carried_residual_is_picked_up_by_the_next_reinvestment() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "AUD").await;
        enrol(&pool, 1, ResidualHandling::CarryForward).await;

        // First: $100 at $9 → 11 shares, $1 carried.
        insert_distribution(&pool, 1, 1, Decimal::from(100), Decimal::ZERO).await;
        let first = db_reinvest(&pool, 1, &body("9")).await.unwrap();
        assert_eq!(first.residual_carried_forward, Decimal::ONE);

        // Second: $8 cash + $1 brought forward = $9 available at $9 → exactly 1 share, $0 leftover.
        insert_distribution(&pool, 2, 1, Decimal::from(8), Decimal::ZERO).await;
        let second = db_reinvest(&pool, 2, &body("9")).await.unwrap();
        assert_eq!(second.residual_brought_forward, Decimal::ONE);
        assert_eq!(second.quantity, Decimal::from(1));
        assert_eq!(second.residual_carried_forward, Decimal::ZERO);
    }

    #[tokio::test]
    async fn pay_out_records_leftover_as_paid_not_carried() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "AUD").await;
        enrol(&pool, 1, ResidualHandling::PayOut).await;
        insert_distribution(&pool, 1, 1, Decimal::from(100), Decimal::ZERO).await;

        let trade = db_reinvest(&pool, 1, &body("9")).await.unwrap();
        // 11 shares, $1 leftover paid out (not carried).
        assert_eq!(trade.quantity, Decimal::from(11));
        assert_eq!(trade.residual_paid_out, Decimal::ONE);
        assert_eq!(trade.residual_carried_forward, Decimal::ZERO);

        // A pay-out leaves no carried balance for the next reinvestment.
        insert_distribution(&pool, 2, 1, Decimal::from(8), Decimal::ZERO).await;
        let next = db_reinvest(&pool, 2, &body("9")).await.unwrap();
        assert_eq!(next.residual_brought_forward, Decimal::ZERO);
        assert_eq!(next.quantity, Decimal::ZERO); // 8 < 9, no whole share
    }

    /// SCENARIOS I-10. A plan that allots at a discount to the market price
    /// costs the new parcel at **the dividend applied**, not at market value:
    /// "the acquisition cost of the additional shares is the amount of the
    /// dividends used to acquire them"
    /// (`docs/ato/cgt-dividend-reinvestment-plans.md`). The discount is not a
    /// separate amount anywhere — it is simply why the same cash bought more
    /// units — and the whole distribution stays assessable.
    #[tokio::test]
    async fn a_discounted_allotment_costs_the_parcel_at_the_dividend_applied() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "AUD").await;
        enrol(&pool, 1, ResidualHandling::CarryForward).await;
        // $100 distribution; market $10.00, allotted at a 5% discount.
        insert_distribution(&pool, 1, 1, Decimal::from(100), Decimal::ZERO).await;

        let trade = db_reinvest(&pool, 1, &body("9.50")).await.unwrap();
        assert_eq!(trade.quantity, Decimal::from(10));
        assert_eq!(trade.average_price, "9.50".parse::<Decimal>().unwrap());
        // $95 of the dividend was applied; the $5 that bought no whole unit is
        // carried, not capitalised into the parcel.
        assert_eq!(trade.residual_carried_forward, Decimal::from(5));

        let parcels = crate::reports::open_parcels::db_open_parcels(&pool)
            .await
            .unwrap();
        assert_eq!(parcels.len(), 1);
        assert_eq!(parcels[0].remaining_cost_base, "95.00".parse().unwrap());
    }

    /// SCENARIOS I-08. A per-unit DRP price stated to four decimals is held
    /// exactly: whole units are floored against it and the leftover keeps the
    /// price's own scale (money is `Decimal` end to end — a `f64` division
    /// here would round the residual the next reinvestment brings forward).
    #[tokio::test]
    async fn a_four_decimal_price_floors_whole_units_and_carries_the_exact_residual() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "AUD").await;
        enrol(&pool, 1, ResidualHandling::CarryForward).await;
        insert_distribution(&pool, 1, 1, Decimal::from(1000), Decimal::ZERO).await;

        // $1,000 ÷ $12.3456 = 81.00 units and a $0.0064 remainder.
        let trade = db_reinvest(&pool, 1, &body("12.3456")).await.unwrap();
        assert_eq!(trade.quantity, Decimal::from(81));
        assert_eq!(
            trade.residual_carried_forward,
            "0.0064".parse::<Decimal>().unwrap()
        );
        let parcels = crate::reports::open_parcels::db_open_parcels(&pool)
            .await
            .unwrap();
        assert_eq!(parcels[0].remaining_cost_base, "999.9936".parse().unwrap());
    }

    /// SCENARIOS I-01, I-04. A distribution that went ex under one period but
    /// is paid under the next — a plan ended between the two dates, which is
    /// how a DRP is ordinarily stopped — reinvests under the period that
    /// authorised it, and its leftover stays there. The trade is dated in the
    /// *next* period's window, so a trade-date rule would have settled it
    /// under that period's handling and carried it into that period's chain.
    #[tokio::test]
    async fn a_reinvestment_paid_under_the_next_period_settles_under_its_own() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "AUD").await;
        enrol_period(
            &pool,
            1,
            1,
            "2024-01-01",
            Some("2024-07-01"),
            ResidualHandling::CarryForward,
        )
        .await;
        enrol_period(&pool, 2, 1, "2024-07-01", None, ResidualHandling::PayOut).await;

        // Ex 20 June (period 1), paid 15 July (inside period 2's window).
        insert_distribution_dated(
            &pool,
            1,
            1,
            "2024-07-15",
            Some("2024-06-20"),
            Decimal::from(100),
            Decimal::ZERO,
        )
        .await;
        let first = db_reinvest(&pool, 1, &body("7")).await.unwrap();
        assert_eq!(first.quantity, Decimal::from(14));
        // Period 1 is closed, so its trailing leftover is refunded — settled
        // as the reinvestment is entered, not left waiting for the period to
        // be saved again.
        assert_eq!(first.residual_carried_forward, Decimal::ZERO);
        assert_eq!(first.residual_paid_out, Decimal::TWO);

        // A distribution squarely inside period 2 starts that period's chain
        // from nothing: the earlier period's leftover was refunded, and a
        // carry never crosses the boundary.
        insert_distribution_dated(
            &pool,
            2,
            1,
            "2024-09-15",
            Some("2024-09-01"),
            Decimal::from(100),
            Decimal::ZERO,
        )
        .await;
        let second = db_reinvest(&pool, 2, &body("7")).await.unwrap();
        assert_eq!(second.residual_brought_forward, Decimal::ZERO);
        assert_eq!(second.quantity, Decimal::from(14));
        assert_eq!(second.residual_paid_out, Decimal::TWO); // period 2 pays out
    }

    /// SCENARIOS I-06. A statement that states **whole** units still leaves
    /// cash over — its unit-step is a whole unit, so the tolerance that makes
    /// a fractional allotment exact is a whole share's worth here — and that
    /// cash is the period's residual, not something to discard: it is carried
    /// (or paid out) and the next reinvestment spends it, exactly as on the
    /// computed path.
    #[tokio::test]
    async fn stated_whole_units_carry_the_cash_they_left_over() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "AUD").await;
        enrol(&pool, 1, ResidualHandling::CarryForward).await;
        // $100 at $7 buys 14 whole units for $98; the statement says "14".
        insert_distribution(&pool, 1, 1, Decimal::from(100), Decimal::ZERO).await;
        let trade = db_reinvest(&pool, 1, &body_units("7", "14")).await.unwrap();
        assert_eq!(trade.quantity, Decimal::from(14));
        assert_eq!(trade.residual_carried_forward, Decimal::TWO);
        assert_eq!(trade.residual_paid_out, Decimal::ZERO);

        // …and the next reinvestment brings it forward: $12 + $2 = 2 units.
        insert_distribution(&pool, 2, 1, Decimal::from(12), Decimal::ZERO).await;
        let next = db_reinvest(&pool, 2, &body("7")).await.unwrap();
        assert_eq!(next.residual_brought_forward, Decimal::TWO);
        assert_eq!(next.quantity, Decimal::TWO);
    }

    /// The same leftover under `PayOut`: the registry refunds it rather than
    /// holding it, so it lands on the paid-out column and nothing carries.
    #[tokio::test]
    async fn stated_whole_units_pay_out_the_leftover_where_the_period_says_so() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "AUD").await;
        enrol(&pool, 1, ResidualHandling::PayOut).await;
        insert_distribution(&pool, 1, 1, Decimal::from(100), Decimal::ZERO).await;

        let trade = db_reinvest(&pool, 1, &body_units("7", "14")).await.unwrap();
        assert_eq!(trade.residual_carried_forward, Decimal::ZERO);
        assert_eq!(trade.residual_paid_out, Decimal::TWO);
    }

    /// SCENARIOS I-08. The distribution's cash and the reinvestment price are
    /// two figures in one division, so they must be the same money: a
    /// distribution recorded in another currency is refused rather than
    /// divided (US$100 ÷ A$7 would cost the parcel A$98 for cash that was
    /// US$100). Nothing is persisted.
    #[tokio::test]
    async fn a_distribution_in_another_currency_than_its_listing_is_refused() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "AUD").await;
        enrol(&pool, 1, ResidualHandling::CarryForward).await;
        test_support::income(1, 1, "2024-03-31".parse().unwrap())
            .with(|i| {
                i.foreign_source_income = Decimal::from(100);
                i.currency = "USD".to_string();
            })
            .insert(&pool)
            .await;

        let err = db_reinvest(&pool, 1, &body("7")).await.unwrap_err();
        assert!(
            matches!(&err, ReinvestError::CurrencyMismatch { distribution, listing }
                if distribution == "USD" && listing == "AUD"),
            "{err:?}"
        );
        let response = client(&pool)
            .post(
                "/income/1/reinvest",
                &serde_json::json!({"reinvestment_price": "7"}),
            )
            .await;
        let (status, body) = response.status_and_body();
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
        assert!(body.contains("USD") && body.contains("AUD"), "{body}");
        assert!(
            crate::entities::trade::db_list(&pool)
                .await
                .unwrap()
                .is_empty(),
            "nothing persisted"
        );
        assert!(
            income::db_get(&pool, 1)
                .await
                .unwrap()
                .unwrap()
                .reinvestment_trade_id
                .is_none()
        );
    }

    /// SCENARIOS I-09. Partial participation is out of scope — stating the
    /// part-allotment is refused, since it does not spend the cash — and the
    /// documented workaround is two income rows, one reinvested and one paid.
    /// This is what makes that workaround defensible: the parcel is costed at
    /// the dividends actually applied to it, and the year still declares the
    /// whole distribution.
    #[tokio::test]
    async fn the_partial_participation_workaround_costs_the_parcel_at_the_cash_reinvested() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "AUD").await;
        enrol(&pool, 1, ResidualHandling::CarryForward).await;
        // A $100 distribution, half of it reinvested at $7: two rows.
        insert_distribution(&pool, 1, 1, Decimal::from(50), Decimal::ZERO).await;
        insert_distribution(&pool, 2, 1, Decimal::from(50), Decimal::ZERO).await;

        // Stating the partial allotment against the whole distribution is
        // refused — the rejection the limitation documents.
        insert_distribution(&pool, 3, 1, Decimal::from(100), Decimal::ZERO).await;
        let err = db_reinvest(&pool, 3, &body_units("7", "7"))
            .await
            .unwrap_err();
        assert!(
            matches!(err, ReinvestError::UnitsCashMismatch { .. }),
            "{err:?}"
        );
        crate::entities::income::db_delete(&pool, 3).await.unwrap();

        let trade = db_reinvest(&pool, 1, &body("7")).await.unwrap();
        assert_eq!(trade.quantity, Decimal::from(7));
        let parcels = crate::reports::open_parcels::db_open_parcels(&pool)
            .await
            .unwrap();
        assert_eq!(
            parcels[0].remaining_cost_base,
            Decimal::from(49),
            "the dividends applied to the units, not the whole distribution"
        );
        let years = crate::reports::tax_summary::db_tax_summary(&pool)
            .await
            .unwrap();
        assert_eq!(
            years[0].dividends_assessable,
            Decimal::from(100),
            "both halves are still declared"
        );
    }

    /// SCENARIOS I-11. On an AMIT the two sides of a reinvested distribution
    /// are recorded separately and neither leaks into the other: the cash row
    /// funds the reinvestment (it is *cash only* — the AMMA attribution is the
    /// assessable record, so the row itself declares nothing), while the DRP
    /// parcel it created is an ordinary open parcel of the fund and takes its
    /// share of the statement's per-unit cost-base movement (CGT event E10).
    #[tokio::test]
    async fn an_amit_distributions_cash_funds_the_drp_while_the_amma_attributes_it() {
        let pool = test_pool().await;
        test_support::listing(1)
            .security_type(listing::SecurityType::Trust)
            .amit(true)
            .insert(&pool)
            .await;
        enrol(&pool, 1, ResidualHandling::CarryForward).await;
        // $500 of cash distributed in the 2023-24 year, all reinvested at $50.
        insert_distribution(&pool, 1, 1, Decimal::from(500), Decimal::ZERO).await;
        let trade = db_reinvest(&pool, 1, &body("50")).await.unwrap();
        assert_eq!(trade.quantity, Decimal::from(10));

        // The fund's AMMA for the year: $500 attributed, and a $1.50/unit
        // excess (a cost-base increase).
        test_support::amma(1, 1)
            .units(Decimal::from(10))
            .cost_base_adjustment("-1.5".parse().unwrap())
            .with(|a| a.australian_dividends_unfranked = Decimal::from(500))
            .insert(&pool)
            .await;
        let generated =
            crate::entities::amit_adjustment_generation::db_generate(&pool, 1, &Default::default())
                .await
                .unwrap();
        // The DRP parcel is the parcel the statement's units are adjusted on.
        assert_eq!(generated.created.len(), 1);
        assert_eq!(generated.created[0].adjustment.trade_id, trade.id);
        assert_eq!(generated.difference, Decimal::ZERO);

        // Assessable: the AMMA's attribution, not the cash row — which is why
        // an AMIT year with cash but no statement is flagged rather than
        // reported (`reports::amit_cash_cross_check`, empty once entered).
        let years = crate::reports::tax_summary::db_tax_summary(&pool)
            .await
            .unwrap();
        assert_eq!(years.len(), 1);
        assert_eq!(years[0].dividends_assessable, Decimal::ZERO);
        assert_eq!(years[0].amma_dividends_unfranked, Decimal::from(500));
        assert!(
            crate::reports::amit_cash_cross_check::db_amit_cash_alerts(&pool)
                .await
                .unwrap()
                .is_empty()
        );

        // …and the reinvested parcel carries the E10 uplift: $500 + 10 × $1.50.
        let parcels = crate::reports::open_parcels::db_open_parcels(&pool)
            .await
            .unwrap();
        assert_eq!(parcels.len(), 1);
        assert_eq!(parcels[0].trade_id, trade.id);
        assert_eq!(parcels[0].remaining_cost_base, "515.0".parse().unwrap());
    }

    /// SCENARIOS I-13. A share split after a reinvestment re-bases the DRP
    /// parcel like any other (the split is applied at read time, so the trade
    /// keeps its as-acquired units), and undoing the reinvestment afterwards
    /// still removes exactly that parcel — the split is not a dependant.
    #[tokio::test]
    async fn a_split_after_a_reinvestment_re_bases_it_and_undo_still_works() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "AUD").await;
        enrol(&pool, 1, ResidualHandling::CarryForward).await;
        insert_distribution(&pool, 1, 1, Decimal::from(100), Decimal::ZERO).await;
        let trade = db_reinvest(&pool, 1, &body("10")).await.unwrap();
        assert_eq!(trade.quantity, Decimal::from(10));

        // 2-for-1 split after the reinvestment.
        crate::entities::corporate_action::db_upsert(
            &pool,
            &crate::entities::corporate_action::CorporateAction {
                id: 1,
                listing_id: 1,
                date: "2024-05-01".parse().unwrap(),
                kind: crate::entities::corporate_action::ActionKind::ShareSplit {
                    split_new_units: Decimal::TWO,
                    split_old_units: Decimal::ONE,
                },
            },
        )
        .await
        .unwrap();
        let parcels = crate::reports::open_parcels::db_open_parcels(&pool)
            .await
            .unwrap();
        assert_eq!(parcels[0].original_quantity, Decimal::from(10));
        assert_eq!(parcels[0].remaining_quantity, Decimal::from(20));
        assert_eq!(parcels[0].remaining_cost_base, Decimal::from(100));

        db_unreinvest(&pool, 1).await.unwrap();
        assert!(
            crate::reports::open_parcels::db_open_parcels(&pool)
                .await
                .unwrap()
                .is_empty()
        );
        assert!(
            crate::entities::trade::db_get(&pool, trade.id)
                .await
                .unwrap()
                .is_none()
        );
    }

    #[tokio::test]
    async fn franking_credits_are_excluded_from_reinvestable_cash() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "AUD").await;
        enrol(&pool, 1, ResidualHandling::CarryForward).await;
        // $9 cash but $90 franking credits — only the $9 cash reinvests.
        insert_distribution(&pool, 1, 1, Decimal::from(9), Decimal::from(90)).await;

        let trade = db_reinvest(&pool, 1, &body("9")).await.unwrap();
        assert_eq!(trade.quantity, Decimal::from(1));
        assert_eq!(trade.residual_carried_forward, Decimal::ZERO);
    }

    /// Broker-stated fractional allotment: the statement's units are taken
    /// exactly — including trailing zeros, so the stored quantity reads back
    /// as stated — and the residual columns record zero.
    #[tokio::test]
    async fn explicit_units_take_the_statements_fractional_allotment() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "USD").await;
        enrol(&pool, 1, ResidualHandling::CarryForward).await;
        // $68.47 cash at $136.94 → the statement says 0.500 shares.
        insert_distribution(&pool, 1, 1, "68.47".parse().unwrap(), Decimal::ZERO).await;

        let trade = db_reinvest(&pool, 1, &body_units("136.94", "0.500"))
            .await
            .unwrap();
        assert_eq!(trade.trade_type, TradeType::DRP);
        assert_eq!(trade.quantity, "0.500".parse::<Decimal>().unwrap());
        assert_eq!(trade.average_price, "136.94".parse::<Decimal>().unwrap());
        assert_eq!(trade.residual_brought_forward, Decimal::ZERO);
        assert_eq!(trade.residual_carried_forward, Decimal::ZERO);
        assert_eq!(trade.residual_paid_out, Decimal::ZERO);

        // The stated figure is stored exactly as stated (scale preserved).
        let stored: String = sqlx::query_scalar("SELECT quantity FROM trades WHERE id = ?")
            .bind(trade.id)
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(stored, "0.500");

        let inc = income::db_get(&pool, 1).await.unwrap().unwrap();
        assert_eq!(inc.reinvestment_trade_id, Some(trade.id));
    }

    /// The cross-check tolerates the statement's own rounding: a real broker
    /// price (not the derived cash ÷ units) leaves `units × price` within one
    /// unit-step of the cash, and that passes.
    #[tokio::test]
    async fn explicit_units_tolerate_sub_step_statement_rounding() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "USD").await;
        enrol(&pool, 1, ResidualHandling::CarryForward).await;
        // 0.501 × 137.05 = 68.66205 vs $68.66 cash — off by $0.00205,
        // well inside one 0.001 unit-step (0.001 × 137.05 = $0.13705).
        insert_distribution(&pool, 1, 1, "68.66".parse().unwrap(), Decimal::ZERO).await;

        let trade = db_reinvest(&pool, 1, &body_units("137.05", "0.501"))
            .await
            .unwrap();
        assert_eq!(trade.quantity, "0.501".parse::<Decimal>().unwrap());
        assert_eq!(trade.residual_carried_forward, Decimal::ZERO);
        assert_eq!(trade.residual_paid_out, Decimal::ZERO);
    }

    /// Units that don't spend the cash are rejected and nothing persists: a
    /// full unit-step (at the stated precision) or more off is a data error,
    /// not rounding.
    #[tokio::test]
    async fn explicit_units_cash_mismatch_is_rejected() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "USD").await;
        enrol(&pool, 1, ResidualHandling::CarryForward).await;
        insert_distribution(&pool, 1, 1, "68.66".parse().unwrap(), Decimal::ZERO).await;

        // 0.600 × 137.05 = 82.23 — $13.57 off the $68.66 cash.
        let err = db_reinvest(&pool, 1, &body_units("137.05", "0.600"))
            .await
            .unwrap_err();
        assert!(matches!(err, ReinvestError::UnitsCashMismatch { .. }));
        let n: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM trades")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(n, 0);

        // The boundary is exclusive: exactly one unit-step off still rejects
        // (a broker-computed figure is always strictly inside the step)...
        insert_distribution(&pool, 2, 1, Decimal::from(60), Decimal::ZERO).await;
        let err = db_reinvest(&pool, 2, &body_units("100", "0.5")) // step 0.1 → tolerance $10
            .await
            .unwrap_err();
        assert!(matches!(err, ReinvestError::UnitsCashMismatch { .. }));
        // ...while just inside it passes (coarser stated precision, looser check).
        insert_distribution(&pool, 3, 1, "59.99".parse().unwrap(), Decimal::ZERO).await;
        let trade = db_reinvest(&pool, 3, &body_units("100", "0.5"))
            .await
            .unwrap();
        assert_eq!(trade.quantity, "0.5".parse::<Decimal>().unwrap());
    }

    #[tokio::test]
    async fn non_positive_units_are_rejected() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "USD").await;
        enrol(&pool, 1, ResidualHandling::CarryForward).await;
        insert_distribution(&pool, 1, 1, Decimal::from(100), Decimal::ZERO).await;
        for units in ["0", "-0.5"] {
            let err = db_reinvest(&pool, 1, &body_units("9", units))
                .await
                .unwrap_err();
            assert!(
                matches!(err, ReinvestError::NonPositiveUnits),
                "units {units}"
            );
        }
    }

    /// Explicit units go through the same enrolment gate as the default path.
    #[tokio::test]
    async fn explicit_units_still_require_enrolment() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "USD").await;
        insert_distribution(&pool, 1, 1, "68.47".parse().unwrap(), Decimal::ZERO).await;
        let err = db_reinvest(&pool, 1, &body_units("136.94", "0.500"))
            .await
            .unwrap_err();
        assert!(matches!(err, ReinvestError::NotEnrolled { .. }));
    }

    /// A residual carried forward by an earlier whole-share reinvestment in
    /// the period is part of the available cash an explicit-units allotment
    /// spends: it's recorded as brought forward, and nothing is carried on —
    /// the broker spent the lot.
    #[tokio::test]
    async fn explicit_units_spend_the_brought_forward_residual() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "AUD").await;
        enrol(&pool, 1, ResidualHandling::CarryForward).await;

        // Whole-share first: $100 at $9 → 11 shares, $1 carried.
        insert_distribution(&pool, 1, 1, Decimal::from(100), Decimal::ZERO).await;
        let first = db_reinvest(&pool, 1, &body("9")).await.unwrap();
        assert_eq!(first.residual_carried_forward, Decimal::ONE);

        // Fractional next: $8 cash + $1 brought forward = $9 = 0.5 × $18.
        insert_distribution(&pool, 2, 1, Decimal::from(8), Decimal::ZERO).await;
        let second = db_reinvest(&pool, 2, &body_units("18", "0.5"))
            .await
            .unwrap();
        assert_eq!(second.quantity, "0.5".parse::<Decimal>().unwrap());
        assert_eq!(second.residual_brought_forward, Decimal::ONE);
        assert_eq!(second.residual_carried_forward, Decimal::ZERO);
        assert_eq!(second.residual_paid_out, Decimal::ZERO);
    }

    /// Live-data check (REQUIREMENTS 2026-06-12): the nine Morgan Stanley ICE
    /// dividend reinvestments from the statement archive — entered as plain
    /// Buys priced net-cash ÷ units while reinvest was whole-share-only — go
    /// through the reinvest operation with the statements' exact fractional
    /// units. Figures are the live rows: foreign source income, US
    /// withholding, the stated units, and the derived per-share price.
    #[tokio::test]
    async fn morgan_stanley_ice_fractional_statements_reproduce() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "USD").await;
        enrol(&pool, 1, ResidualHandling::CarryForward).await;

        // (pay date, gross, US tax withheld, stated units, price)
        let statements = [
            ("2024-04-01", "80.55", "12.08", "0.500", "136.94000000"),
            ("2024-06-28", "80.78", "12.12", "0.501", "137.04590818"),
            ("2024-09-30", "81.00", "12.15", "0.434", "158.64055300"),
            ("2024-12-31", "81.20", "12.18", "0.465", "148.43010753"),
            ("2025-03-31", "111.31", "16.70", "0.539", "175.52875696"),
            ("2025-06-30", "111.57", "16.74", "0.522", "181.66666667"),
            ("2025-09-30", "111.82", "16.77", "0.565", "168.23008850"),
            ("2025-12-31", "112.09", "16.81", "0.582", "163.62542955"),
            ("2026-03-31", "148.78", "22.32", "0.811", "155.89395808"),
        ];
        for (i, (date, gross, withheld, units, price)) in statements.iter().enumerate() {
            let id = i as i64 + 1;
            test_support::income(id, 1, date.parse().unwrap())
                .with(|inc| {
                    inc.foreign_source_income = gross.parse().unwrap();
                    inc.foreign_tax_paid = withheld.parse().unwrap();
                    inc.currency = "USD".to_string();
                })
                .insert(&pool)
                .await;
            let trade = db_reinvest(&pool, id, &body_units(price, units))
                .await
                .unwrap_or_else(|e| panic!("statement {date}: {e:?}"));
            assert_eq!(trade.trade_type, TradeType::DRP, "statement {date}");
            let stored: String = sqlx::query_scalar("SELECT quantity FROM trades WHERE id = ?")
                .bind(trade.id)
                .fetch_one(&pool)
                .await
                .unwrap();
            assert_eq!(&stored, units, "statement {date}: exact stated units");
            assert_eq!(trade.residual_carried_forward, Decimal::ZERO);
            assert_eq!(trade.residual_paid_out, Decimal::ZERO);
        }
    }

    #[tokio::test]
    async fn not_enrolled_is_rejected_and_nothing_persisted() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "AUD").await;
        insert_distribution(&pool, 1, 1, Decimal::from(100), Decimal::ZERO).await;

        let err = db_reinvest(&pool, 1, &body("9")).await.unwrap_err();
        assert!(matches!(err, ReinvestError::NotEnrolled { .. }));
        // No trade created, distribution unlinked.
        let n: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM trades")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(n, 0);
        assert!(
            income::db_get(&pool, 1)
                .await
                .unwrap()
                .unwrap()
                .reinvestment_trade_id
                .is_none()
        );
    }

    #[tokio::test]
    async fn already_reinvested_is_rejected() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "AUD").await;
        enrol(&pool, 1, ResidualHandling::CarryForward).await;
        insert_distribution(&pool, 1, 1, Decimal::from(100), Decimal::ZERO).await;

        db_reinvest(&pool, 1, &body("9")).await.unwrap();
        let err = db_reinvest(&pool, 1, &body("9")).await.unwrap_err();
        assert!(matches!(err, ReinvestError::AlreadyReinvested));
        // Still exactly one DRP trade.
        let n: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM trades")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(n, 1);
    }

    #[tokio::test]
    async fn missing_income_is_not_found() {
        let pool = test_pool().await;
        let err = db_reinvest(&pool, 99, &body("9")).await.unwrap_err();
        assert!(matches!(err, ReinvestError::IncomeNotFound));
    }

    #[tokio::test]
    async fn non_positive_price_is_rejected() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "AUD").await;
        enrol(&pool, 1, ResidualHandling::CarryForward).await;
        insert_distribution(&pool, 1, 1, Decimal::from(100), Decimal::ZERO).await;
        let err = db_reinvest(&pool, 1, &body("0")).await.unwrap_err();
        assert!(matches!(err, ReinvestError::NonPositivePrice));
    }

    #[tokio::test]
    async fn distribution_before_enrolment_is_rejected() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "AUD").await;
        // Enrolled only from June 2024; the distribution went ex in March.
        enrol_period(
            &pool,
            1,
            1,
            "2024-06-01",
            None,
            ResidualHandling::CarryForward,
        )
        .await;
        insert_distribution(&pool, 1, 1, Decimal::from(100), Decimal::ZERO).await; // 2024-03-31
        let err = db_reinvest(&pool, 1, &body("9")).await.unwrap_err();
        assert!(matches!(err, ReinvestError::NotEnrolled { .. }));
    }

    #[tokio::test]
    async fn distribution_in_unenrolment_gap_is_rejected() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "AUD").await;
        // Enrolled through 2023, re-enrolled from 2025 — 2024 is a gap.
        enrol_period(
            &pool,
            1,
            1,
            "2023-01-01",
            Some("2024-01-01"),
            ResidualHandling::CarryForward,
        )
        .await;
        enrol_period(
            &pool,
            2,
            1,
            "2025-01-01",
            None,
            ResidualHandling::CarryForward,
        )
        .await;
        insert_distribution(&pool, 1, 1, Decimal::from(100), Decimal::ZERO).await; // 2024-03-31
        let err = db_reinvest(&pool, 1, &body("9")).await.unwrap_err();
        assert!(matches!(err, ReinvestError::NotEnrolled { .. }));
    }

    #[tokio::test]
    async fn re_enrolment_after_unenrolment_uses_the_new_periods_handling() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "AUD").await;
        enrol_period(
            &pool,
            1,
            1,
            "2023-01-01",
            Some("2024-01-01"),
            ResidualHandling::CarryForward,
        )
        .await;
        enrol_period(&pool, 2, 1, "2025-01-01", None, ResidualHandling::PayOut).await;
        // A distribution inside the re-enrolment period reinvests, and its
        // leftover follows the *new* period's PayOut, not the old CarryForward.
        insert_distribution_dated(
            &pool,
            1,
            1,
            "2025-03-31",
            None,
            Decimal::from(100),
            Decimal::ZERO,
        )
        .await;
        let trade = db_reinvest(&pool, 1, &body("9")).await.unwrap();
        assert_eq!(trade.quantity, Decimal::from(11));
        assert_eq!(trade.residual_paid_out, Decimal::ONE);
        assert_eq!(trade.residual_carried_forward, Decimal::ZERO);
    }

    #[tokio::test]
    async fn reinvestability_is_decided_by_ex_date_not_pay_date() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "AUD").await;
        enrol_period(
            &pool,
            1,
            1,
            "2024-01-01",
            Some("2024-04-01"),
            ResidualHandling::CarryForward,
        )
        .await;

        // Ex inside the period, paid after the unenrolment took effect → the
        // participation was fixed at the ex date, so it still reinvests.
        insert_distribution_dated(
            &pool,
            1,
            1,
            "2024-04-10",
            Some("2024-03-15"),
            Decimal::from(100),
            Decimal::ZERO,
        )
        .await;
        let trade = db_reinvest(&pool, 1, &body("9")).await.unwrap();
        assert_eq!(trade.quantity, Decimal::from(11));

        // Ex before the period, paid inside it → was not enrolled at ex → rejected.
        insert_distribution_dated(
            &pool,
            2,
            1,
            "2024-02-01",
            Some("2023-12-15"),
            Decimal::from(100),
            Decimal::ZERO,
        )
        .await;
        let err = db_reinvest(&pool, 2, &body("9")).await.unwrap_err();
        assert!(matches!(err, ReinvestError::NotEnrolled { .. }));
    }

    /// SCENARIOS G-20. A trust statement rarely prints an ex date, but its
    /// entitlement date is the distribution period's end — when the units went
    /// ex — so it decides participation ahead of the payment date, which can
    /// be weeks later and on the far side of an unenrolment.
    #[tokio::test]
    async fn a_trust_rows_entitlement_date_decides_participation_before_the_pay_date() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "AUD").await;
        enrol_period(
            &pool,
            1,
            1,
            "2024-01-01",
            Some("2024-07-05"),
            ResidualHandling::CarryForward,
        )
        .await;

        // Entitled 30 June (inside the period), paid 20 July (after the
        // unenrolment): the units were enrolled when they went ex.
        test_support::income(1, 1, "2024-07-20".parse().unwrap())
            .with(|i| {
                i.trust_income = true;
                i.entitlement_date = Some("2024-06-30".parse().unwrap());
                i.unfranked_amount = Decimal::from(100);
            })
            .insert(&pool)
            .await;
        let trade = db_reinvest(&pool, 1, &body("9")).await.unwrap();
        assert_eq!(trade.quantity, Decimal::from(11));

        // The same row without the entitlement date falls back to the pay
        // date, which the enrolment no longer covers.
        test_support::income(2, 1, "2024-07-20".parse().unwrap())
            .with(|i| {
                i.trust_income = true;
                i.unfranked_amount = Decimal::from(100);
            })
            .insert(&pool)
            .await;
        let err = db_reinvest(&pool, 2, &body("9")).await.unwrap_err();
        assert!(matches!(err, ReinvestError::NotEnrolled { .. }));
    }

    #[tokio::test]
    async fn carried_residual_does_not_cross_an_unenrolment() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "AUD").await;
        enrol(&pool, 1, ResidualHandling::CarryForward).await; // open from 2024-01-01

        // First reinvestment carries $1 forward.
        insert_distribution(&pool, 1, 1, Decimal::from(100), Decimal::ZERO).await;
        let first = db_reinvest(&pool, 1, &body("9")).await.unwrap();
        assert_eq!(first.residual_carried_forward, Decimal::ONE);

        // Unenrol (close the period): the trailing $1 is paid out...
        enrol_period(
            &pool,
            1,
            1,
            "2024-01-01",
            Some("2024-06-01"),
            ResidualHandling::CarryForward,
        )
        .await;
        let settled = trade::db_get(&pool, first.id).await.unwrap().unwrap();
        assert_eq!(settled.residual_carried_forward, Decimal::ZERO);
        assert_eq!(settled.residual_paid_out, Decimal::ONE);

        // ...so a reinvestment in the re-enrolment period brings nothing forward.
        enrol_period(
            &pool,
            2,
            1,
            "2025-01-01",
            None,
            ResidualHandling::CarryForward,
        )
        .await;
        insert_distribution_dated(
            &pool,
            2,
            1,
            "2025-03-31",
            None,
            Decimal::from(8),
            Decimal::ZERO,
        )
        .await;
        let next = db_reinvest(&pool, 2, &body("9")).await.unwrap();
        assert_eq!(next.residual_brought_forward, Decimal::ZERO);
        assert_eq!(next.quantity, Decimal::ZERO); // 8 < 9, no whole share
    }

    /// The RSU scenario (REQUIREMENTS "Holding accounts"): the same listing
    /// held in two accounts at once, with the personal account DRP-enrolled
    /// and the employer-plan account not. A distribution paid to the enrolled
    /// account reinvests — and the DRP trade lands in that account — while
    /// the plan account's identical distribution is rejected: enrolment is
    /// per (listing, holding account), not per listing.
    #[tokio::test]
    async fn enrolment_is_per_holding_account() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "AUD").await;
        insert_account(&pool, 2, "ICE Employee Plan").await;
        // Only the default (personal) account is enrolled.
        enrol_in_account(&pool, 1, 1, 1, ResidualHandling::CarryForward).await;

        // Personal-account distribution reinvests, into the personal account.
        insert_distribution_in_account(&pool, 1, 1, 1, Decimal::from(100)).await;
        let trade = db_reinvest(&pool, 1, &body("9")).await.unwrap();
        assert_eq!(trade.quantity, Decimal::from(11));
        assert_eq!(trade.holding_account_id, 1);

        // The plan account's distribution on the same listing is rejected.
        insert_distribution_in_account(&pool, 2, 1, 2, Decimal::from(100)).await;
        let err = db_reinvest(&pool, 2, &body("9")).await.unwrap_err();
        assert!(matches!(err, ReinvestError::NotEnrolled { .. }));
    }

    /// The DRP trade is created in the distribution's holding account, not
    /// the default one.
    #[tokio::test]
    async fn drp_trade_lands_in_the_distributions_account() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "AUD").await;
        insert_account(&pool, 2, "Personal CHESS").await;
        enrol_in_account(&pool, 1, 1, 2, ResidualHandling::CarryForward).await;
        insert_distribution_in_account(&pool, 1, 1, 2, Decimal::from(100)).await;

        let trade = db_reinvest(&pool, 1, &body("9")).await.unwrap();
        assert_eq!(trade.holding_account_id, 2);
    }

    /// Each (listing, holding account) runs its own residual chain: a
    /// carried-forward leftover in one account is never brought forward by a
    /// reinvestment in another.
    #[tokio::test]
    async fn carried_residual_does_not_cross_accounts() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "AUD").await;
        insert_account(&pool, 2, "Personal CHESS").await;
        enrol_in_account(&pool, 1, 1, 1, ResidualHandling::CarryForward).await;
        enrol_in_account(&pool, 2, 1, 2, ResidualHandling::CarryForward).await;

        // Account 1: $100 at $9 → 11 shares, $1 carried forward.
        insert_distribution_in_account(&pool, 1, 1, 1, Decimal::from(100)).await;
        let first = db_reinvest(&pool, 1, &body("9")).await.unwrap();
        assert_eq!(first.residual_carried_forward, Decimal::ONE);

        // Account 2's next reinvestment brings nothing forward from account 1.
        insert_distribution_in_account(&pool, 2, 1, 2, Decimal::from(8)).await;
        let other = db_reinvest(&pool, 2, &body("9")).await.unwrap();
        assert_eq!(other.residual_brought_forward, Decimal::ZERO);
        assert_eq!(other.quantity, Decimal::ZERO); // 8 < 9, no whole share

        // Account 1's own chain still picks its $1 up.
        insert_distribution_in_account(&pool, 3, 1, 1, Decimal::from(8)).await;
        let next = db_reinvest(&pool, 3, &body("9")).await.unwrap();
        assert_eq!(next.residual_brought_forward, Decimal::ONE);
        assert_eq!(next.quantity, Decimal::from(1));
    }

    #[tokio::test]
    async fn api_reinvest_returns_201_with_trade() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "AUD").await;
        enrol(&pool, 1, ResidualHandling::CarryForward).await;
        insert_distribution(&pool, 1, 1, Decimal::from(100), Decimal::ZERO).await;

        let resp = client(&pool)
            .post_raw("/income/1/reinvest", r#"{"reinvestment_price":"9"}"#)
            .await;
        assert_eq!(resp.status, StatusCode::CREATED);
        let trade: Trade = resp.json();
        assert_eq!(trade.trade_type, TradeType::DRP);
        assert_eq!(trade.quantity, Decimal::from(11));
    }

    #[tokio::test]
    async fn api_reinvest_with_units_returns_201_with_fractional_trade() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "USD").await;
        enrol(&pool, 1, ResidualHandling::CarryForward).await;
        insert_distribution(&pool, 1, 1, "68.47".parse().unwrap(), Decimal::ZERO).await;

        let resp = client(&pool)
            .post_raw(
                "/income/1/reinvest",
                r#"{"reinvestment_price":"136.94","units":"0.500"}"#,
            )
            .await;
        assert_eq!(resp.status, StatusCode::CREATED);
        let trade: Trade = resp.json();
        assert_eq!(trade.trade_type, TradeType::DRP);
        assert_eq!(trade.quantity, "0.500".parse::<Decimal>().unwrap());
    }

    /// The units/cash mismatch rejection carries both figures so the user can
    /// see what the entry computes to.
    #[tokio::test]
    async fn api_reinvest_units_mismatch_returns_422_with_figures() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "USD").await;
        enrol(&pool, 1, ResidualHandling::CarryForward).await;
        insert_distribution(&pool, 1, 1, "68.66".parse().unwrap(), Decimal::ZERO).await;
        let resp = client(&pool)
            .post_raw(
                "/income/1/reinvest",
                r#"{"reinvestment_price":"137.05","units":"0.600"}"#,
            )
            .await;
        assert_eq!(resp.status, StatusCode::UNPROCESSABLE_ENTITY);
        let text = resp.text().to_string();
        assert!(text.contains("82.23000"), "body: {text}"); // 0.600 × 137.05
        assert!(text.contains("68.66"), "body: {text}");
    }

    #[tokio::test]
    async fn api_reinvest_not_enrolled_returns_422() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "AUD").await;
        insert_distribution(&pool, 1, 1, Decimal::from(100), Decimal::ZERO).await;
        let resp = client(&pool)
            .post_raw("/income/1/reinvest", r#"{"reinvestment_price":"9"}"#)
            .await;
        assert_eq!(resp.status, StatusCode::UNPROCESSABLE_ENTITY);
        let text = resp.text().to_string();
        // The rejection names the account and ticker, not raw ids.
        assert!(text.contains("Default"), "body: {text}");
        assert!(text.contains("T1"), "body: {text}");
        assert!(text.contains("not enrolled"), "body: {text}");
    }

    #[tokio::test]
    async fn api_reinvest_missing_income_returns_404() {
        let pool = test_pool().await;
        let resp = client(&pool)
            .post_raw("/income/99/reinvest", r#"{"reinvestment_price":"9"}"#)
            .await;
        assert_eq!(resp.status, StatusCode::NOT_FOUND);
    }

    // ---- unreinvest (DELETE /income/:id/reinvest) ----

    #[tokio::test]
    async fn unreinvest_deletes_the_trade_clears_the_link_and_allows_redo() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "AUD").await;
        enrol(&pool, 1, ResidualHandling::CarryForward).await;
        insert_distribution(&pool, 1, 1, Decimal::from(100), Decimal::ZERO).await;
        let trade = db_reinvest(&pool, 1, &body("9")).await.unwrap();

        db_unreinvest(&pool, 1).await.unwrap();

        assert!(
            crate::entities::trade::db_get(&pool, trade.id)
                .await
                .unwrap()
                .is_none(),
            "DRP trade must be deleted"
        );
        let inc = income::db_get(&pool, 1).await.unwrap().unwrap();
        assert_eq!(inc.reinvestment_trade_id, None, "link must be cleared");

        // The undo is a true inverse: the distribution reinvests again, at a
        // corrected price this time.
        let redo = db_reinvest(&pool, 1, &body("10")).await.unwrap();
        assert_eq!(redo.quantity, Decimal::from(10));
        assert_eq!(
            income::db_get(&pool, 1)
                .await
                .unwrap()
                .unwrap()
                .reinvestment_trade_id,
            Some(redo.id)
        );
    }

    #[tokio::test]
    async fn unreinvest_without_a_reinvestment_is_rejected() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "AUD").await;
        insert_distribution(&pool, 1, 1, Decimal::from(100), Decimal::ZERO).await;
        let err = db_unreinvest(&pool, 1).await.unwrap_err();
        assert!(
            matches!(err, ReinvestError::NotReinvested),
            "expected NotReinvested, got: {err:?}"
        );
    }

    #[tokio::test]
    async fn unreinvest_missing_income_is_not_found() {
        let pool = test_pool().await;
        let err = db_unreinvest(&pool, 99).await.unwrap_err();
        assert!(
            matches!(err, ReinvestError::IncomeNotFound),
            "expected IncomeNotFound, got: {err:?}"
        );
    }

    #[tokio::test]
    async fn unreinvest_is_refused_while_the_trade_is_drawn_on() {
        // A Sell allocation consuming the DRP parcel would be orphaned by the
        // undo — refused, and nothing changes.
        let pool = test_pool().await;
        insert_listing(&pool, 1, "AUD").await;
        enrol(&pool, 1, ResidualHandling::CarryForward).await;
        insert_distribution(&pool, 1, 1, Decimal::from(100), Decimal::ZERO).await;
        let trade = db_reinvest(&pool, 1, &body("9")).await.unwrap();
        crate::entities::sell::db_upsert_sell(
            &pool,
            50,
            &crate::entities::sell::SellBody {
                date: "2024-06-03".parse().unwrap(),
                settlement_date: Some("2024-06-05".parse().unwrap()),
                listing_id: 1,
                average_price: Decimal::from(12),
                quantity: Decimal::from(5),
                currency: "AUD".to_string(),
                brokerage: Decimal::ZERO,
                gst_on_brokerage: Decimal::ZERO,
                brokerage_includes_gst: false,
                brokerage_currency: "AUD".to_string(),
                fx_rate: Decimal::ONE,
                spot_fx_rate: None,
                contract_note_ref: None,
                statement_total: None,
                holding_account_id: 1,
                allocations: vec![crate::entities::sell::AllocationInput {
                    purchase_trade_id: trade.id,
                    quantity_allocated: Decimal::from(5),
                }],
            },
        )
        .await
        .unwrap();

        let err = db_unreinvest(&pool, 1).await.unwrap_err();
        assert!(
            matches!(err, ReinvestError::ReinvestmentConsumed),
            "expected ReinvestmentConsumed, got: {err:?}"
        );
        assert!(
            crate::entities::trade::db_get(&pool, trade.id)
                .await
                .unwrap()
                .is_some(),
            "trade must remain"
        );
        assert_eq!(
            income::db_get(&pool, 1)
                .await
                .unwrap()
                .unwrap()
                .reinvestment_trade_id,
            Some(trade.id),
            "link must remain"
        );
    }

    #[tokio::test]
    async fn unreinvest_is_lifo_a_mid_chain_trade_is_refused() {
        // Reinvest twice: the second trade brought the first's carried
        // residual forward, so the first can only be undone after the second.
        let pool = test_pool().await;
        insert_listing(&pool, 1, "AUD").await;
        enrol(&pool, 1, ResidualHandling::CarryForward).await;
        insert_distribution_dated(
            &pool,
            1,
            1,
            "2024-03-31",
            None,
            Decimal::from(100),
            Decimal::ZERO,
        )
        .await;
        insert_distribution_dated(
            &pool,
            2,
            1,
            "2024-06-30",
            None,
            Decimal::from(100),
            Decimal::ZERO,
        )
        .await;
        db_reinvest(&pool, 1, &body("9")).await.unwrap();
        let second = db_reinvest(&pool, 2, &body("9")).await.unwrap();
        // The chain is real: the second reinvestment picked up the first's $1.
        assert_eq!(second.residual_brought_forward, Decimal::ONE);

        let err = db_unreinvest(&pool, 1).await.unwrap_err();
        assert!(
            matches!(err, ReinvestError::ReinvestmentNotChainTail),
            "expected ReinvestmentNotChainTail, got: {err:?}"
        );

        // Undoing in LIFO order works: the tail first, then the first one.
        db_unreinvest(&pool, 2).await.unwrap();
        db_unreinvest(&pool, 1).await.unwrap();
        let n: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM trades")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(n, 0, "both DRP trades undone");
    }

    /// SCENARIOS V-b. The residual chain reads backwards, so it can only be
    /// built forwards: reinvesting a distribution the period already has a
    /// *later* DRP trade for is refused. Entered that way round it used to
    /// bring cash forward from a reinvestment six months later — the March
    /// parcel took the September one's $7 (which September had not left it),
    /// September started from nothing, and both parcels came out the wrong
    /// size with the same $7 spent twice.
    #[tokio::test]
    async fn reinvesting_behind_an_existing_reinvestment_is_refused() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "AUD").await;
        enrol(&pool, 1, ResidualHandling::CarryForward).await;
        // March pays $105, September $107 — the measured case.
        insert_distribution_dated(
            &pool,
            1,
            1,
            "2024-03-28",
            None,
            Decimal::from(105),
            Decimal::ZERO,
        )
        .await;
        insert_distribution_dated(
            &pool,
            2,
            1,
            "2024-09-30",
            None,
            Decimal::from(107),
            Decimal::ZERO,
        )
        .await;

        // September first, at $10 — legitimate on its own.
        db_reinvest(&pool, 2, &body("10")).await.unwrap();

        // March second is out of payment order and refused, naming both dates.
        let err = db_reinvest(&pool, 1, &body("9")).await.unwrap_err();
        assert!(
            matches!(
                err,
                ReinvestError::LaterReinvestmentExists { date, later }
                    if date == "2024-03-28".parse().unwrap()
                        && later == "2024-09-30".parse().unwrap()
            ),
            "expected LaterReinvestmentExists(2024-03-28, 2024-09-30), got: {err:?}"
        );

        // Nothing was written: no second DRP trade, and March stays unlinked.
        let drps: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM trades WHERE trade_type = 'DRP'")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(drps, 1);
        let march = income::db_get(&pool, 1).await.unwrap().unwrap();
        assert_eq!(march.reinvestment_trade_id, None);
    }

    /// The same two distributions entered in payment order — the remedy the
    /// refusal points at — chain correctly: March $105 at $9 buys 11 units and
    /// carries $6, which September brings forward ($107 + $6 = $113) to buy 11
    /// units at $10 and carry $3.
    #[tokio::test]
    async fn in_payment_order_the_two_distributions_chain_correctly() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "AUD").await;
        enrol(&pool, 1, ResidualHandling::CarryForward).await;
        insert_distribution_dated(
            &pool,
            1,
            1,
            "2024-03-28",
            None,
            Decimal::from(105),
            Decimal::ZERO,
        )
        .await;
        insert_distribution_dated(
            &pool,
            2,
            1,
            "2024-09-30",
            None,
            Decimal::from(107),
            Decimal::ZERO,
        )
        .await;

        let march = db_reinvest(&pool, 1, &body("9")).await.unwrap();
        assert_eq!(march.quantity, Decimal::from(11));
        assert_eq!(march.residual_brought_forward, Decimal::ZERO);
        assert_eq!(march.residual_carried_forward, Decimal::from(6));

        let september = db_reinvest(&pool, 2, &body("10")).await.unwrap();
        assert_eq!(september.residual_brought_forward, Decimal::from(6));
        assert_eq!(september.quantity, Decimal::from(11));
        assert_eq!(september.residual_carried_forward, Decimal::from(3));
    }

    /// The measured facts of the closed-period pair below, entered while the
    /// period is still **open** and closed afterwards — the control that
    /// bounds SCENARIOS V-e to reinvesting into an already-closed period.
    /// March buys 11 units carrying $6, June brings that forward ($107 + $6 =
    /// $113) for 11 units carrying $3, and closing the period settles only the
    /// trailing $3.
    #[tokio::test]
    async fn reinvesting_twice_into_an_open_period_then_closing_it_settles_only_the_tail() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "AUD").await;
        enrol_period(
            &pool,
            1,
            1,
            "2024-01-01",
            None,
            ResidualHandling::CarryForward,
        )
        .await;
        insert_march_and_june_distributions(&pool).await;

        let march = db_reinvest(&pool, 1, &body("9")).await.unwrap();
        assert_eq!(march.quantity, Decimal::from(11));
        assert_eq!(march.residual_carried_forward, Decimal::from(6));

        let june = db_reinvest(&pool, 2, &body("10")).await.unwrap();
        assert_eq!(june.residual_brought_forward, Decimal::from(6));
        assert_eq!(june.quantity, Decimal::from(11));
        assert_eq!(june.residual_carried_forward, Decimal::from(3));

        // Close the period: only the trailing $3 is refunded.
        enrol_period(
            &pool,
            1,
            1,
            "2024-01-01",
            Some("2024-12-01"),
            ResidualHandling::CarryForward,
        )
        .await;
        assert_eq!(
            residuals(&pool, march.id).await,
            (Decimal::from(6), Decimal::ZERO)
        );
        assert_eq!(
            residuals(&pool, june.id).await,
            (Decimal::ZERO, Decimal::from(3))
        );
    }

    /// SCENARIOS V-e. The same facts reinvested into a period that is
    /// **already closed** — the ordinary shape of a statement arriving after
    /// the plan was left — must chain identically. `db_reinvest` used to read
    /// the prior trade's `residual_carried_forward`, which for a closed
    /// period's trailing trade is `0`: its leftover had been settled to
    /// `residual_paid_out` when the period closed, and `recompute_residuals`
    /// only moved it back *after* the new trade had already been written with
    /// nothing brought forward. June came out at 10 units instead of 11 and
    /// March's $6 was carried to nobody. Both halves of the leftover are now
    /// put back to the period, which is what decides the split.
    #[tokio::test]
    async fn reinvesting_twice_into_an_already_closed_period_brings_the_leftover_forward() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "AUD").await;
        enrol_period(
            &pool,
            1,
            1,
            "2024-01-01",
            Some("2024-12-01"),
            ResidualHandling::CarryForward,
        )
        .await;
        insert_march_and_june_distributions(&pool).await;

        // March is the closed period's tail as it is entered, so its $6 is
        // refunded straight away — that is the row June must still read.
        let march = db_reinvest(&pool, 1, &body("9")).await.unwrap();
        assert_eq!(march.quantity, Decimal::from(11));
        assert_eq!(march.residual_brought_forward, Decimal::ZERO);
        assert_eq!(march.residual_carried_forward, Decimal::ZERO);
        assert_eq!(march.residual_paid_out, Decimal::from(6));

        // June takes the tail from it: $107 + $6 = $113 buys 11 units at $10.
        let june = db_reinvest(&pool, 2, &body("10")).await.unwrap();
        assert_eq!(june.residual_brought_forward, Decimal::from(6));
        assert_eq!(june.quantity, Decimal::from(11));
        assert_eq!(june.residual_carried_forward, Decimal::ZERO);
        assert_eq!(june.residual_paid_out, Decimal::from(3));

        // …and March is no longer the tail, so its $6 is carried again — the
        // figure June brought forward, from the one rule that decides both.
        assert_eq!(
            residuals(&pool, march.id).await,
            (Decimal::from(6), Decimal::ZERO)
        );
    }

    /// A `PayOut` period is unaffected either way: the registry refunds every
    /// leftover as it arises, so nothing is ever brought forward and closing
    /// the period settles nothing further. Pinning the asymmetry keeps the
    /// V-e fix from turning a refund into a carry.
    #[tokio::test]
    async fn a_closed_pay_out_period_still_brings_nothing_forward() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "AUD").await;
        enrol_period(
            &pool,
            1,
            1,
            "2024-01-01",
            Some("2024-12-01"),
            ResidualHandling::PayOut,
        )
        .await;
        insert_march_and_june_distributions(&pool).await;

        let march = db_reinvest(&pool, 1, &body("9")).await.unwrap();
        assert_eq!(march.quantity, Decimal::from(11));
        assert_eq!(march.residual_paid_out, Decimal::from(6));

        // $107 alone — March's $6 was refunded, not held for June.
        let june = db_reinvest(&pool, 2, &body("10")).await.unwrap();
        assert_eq!(june.residual_brought_forward, Decimal::ZERO);
        assert_eq!(june.quantity, Decimal::from(10));
        assert_eq!(june.residual_paid_out, Decimal::from(7));

        // March keeps its refund: it is not the tail any more, but under
        // `PayOut` the tail was never what decided this.
        assert_eq!(
            residuals(&pool, march.id).await,
            (Decimal::ZERO, Decimal::from(6))
        );
    }

    /// SCENARIOS V-e over the HTTP surface: two reinvestments posted into an
    /// already-closed period, the second returning the 11 units its $6
    /// brought-forward buys.
    #[tokio::test]
    async fn api_reinvesting_into_a_closed_period_brings_the_leftover_forward() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "AUD").await;
        enrol_period(
            &pool,
            1,
            1,
            "2024-01-01",
            Some("2024-12-01"),
            ResidualHandling::CarryForward,
        )
        .await;
        insert_march_and_june_distributions(&pool).await;

        let app = client(&pool);
        app.post_raw("/income/1/reinvest", r#"{"reinvestment_price":"9"}"#)
            .await
            .expect_status(StatusCode::CREATED);
        let june: Trade = app
            .post_raw("/income/2/reinvest", r#"{"reinvestment_price":"10"}"#)
            .await
            .expect_status(StatusCode::CREATED)
            .json();
        assert_eq!(june.residual_brought_forward, Decimal::from(6));
        assert_eq!(june.quantity, Decimal::from(11));
        assert_eq!(june.residual_paid_out, Decimal::from(3));
    }

    /// The measured V-e facts: $105 paid 28 March (ex 1 March) and $107 paid
    /// 28 June (ex 1 June), both entitled inside a 2024-01-01 → 2024-12-01
    /// period, reinvested in payment order so V-b's refusal never fires.
    async fn insert_march_and_june_distributions(pool: &SqlitePool) {
        insert_distribution_dated(
            pool,
            1,
            1,
            "2024-03-28",
            Some("2024-03-01"),
            Decimal::from(105),
            Decimal::ZERO,
        )
        .await;
        insert_distribution_dated(
            pool,
            2,
            1,
            "2024-06-28",
            Some("2024-06-01"),
            Decimal::from(107),
            Decimal::ZERO,
        )
        .await;
    }

    /// One trade's `(residual_carried_forward, residual_paid_out)` as stored,
    /// for reading a trade back after a later write re-derived the chain.
    async fn residuals(pool: &SqlitePool, trade_id: i64) -> (Decimal, Decimal) {
        let t = crate::entities::trade::db_get(pool, trade_id)
            .await
            .unwrap()
            .unwrap();
        (t.residual_carried_forward, t.residual_paid_out)
    }

    /// The order the chain is judged on is the **trade's own date**, which the
    /// body may state — so a reinvestment back-dated by `date` behind an
    /// existing one is refused even though its distribution was paid later,
    /// and one dated forward of it is accepted even though its distribution
    /// was paid earlier. (Period membership is still the entitlement date;
    /// both distributions sit in the same open period either way.)
    #[tokio::test]
    async fn the_chain_order_is_the_trades_date_not_the_distributions() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "AUD").await;
        enrol(&pool, 1, ResidualHandling::CarryForward).await;
        insert_distribution_dated(
            &pool,
            1,
            1,
            "2024-03-28",
            None,
            Decimal::from(100),
            Decimal::ZERO,
        )
        .await;
        insert_distribution_dated(
            &pool,
            2,
            1,
            "2024-09-30",
            None,
            Decimal::from(100),
            Decimal::ZERO,
        )
        .await;

        // The March distribution's trade, dated forward of September's.
        let dated_forward = ReinvestBody {
            date: Some("2024-10-15".parse().unwrap()),
            ..body("9")
        };
        db_reinvest(&pool, 1, &dated_forward).await.unwrap();

        // September's own trade now lands behind it and is refused, though
        // its distribution is the later of the two.
        let err = db_reinvest(&pool, 2, &body("9")).await.unwrap_err();
        assert!(
            matches!(
                err,
                ReinvestError::LaterReinvestmentExists { date, later }
                    if date == "2024-09-30".parse().unwrap()
                        && later == "2024-10-15".parse().unwrap()
            ),
            "expected the trade dates, not the distributions', got: {err:?}"
        );
    }

    /// A registry can pay two distributions on one day, so a *same-dated*
    /// reinvestment is allowed rather than refused: it joins the chain behind
    /// the trades already dated that day (its id is the higher, and the chain
    /// is ordered by (date, id)), bringing their residual forward.
    #[tokio::test]
    async fn a_same_dated_reinvestment_joins_the_chain_behind_it() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "AUD").await;
        enrol(&pool, 1, ResidualHandling::CarryForward).await;
        insert_distribution_dated(
            &pool,
            1,
            1,
            "2024-03-28",
            None,
            Decimal::from(100),
            Decimal::ZERO,
        )
        .await;
        insert_distribution_dated(
            &pool,
            2,
            1,
            "2024-03-28",
            None,
            Decimal::from(8),
            Decimal::ZERO,
        )
        .await;

        let first = db_reinvest(&pool, 1, &body("9")).await.unwrap();
        assert_eq!(first.residual_carried_forward, Decimal::ONE);
        let second = db_reinvest(&pool, 2, &body("9")).await.unwrap();
        assert_eq!(second.date, first.date);
        assert_eq!(second.residual_brought_forward, Decimal::ONE);
        assert_eq!(second.quantity, Decimal::ONE);
        assert_eq!(second.residual_carried_forward, Decimal::ZERO);
    }

    /// The refusal is the period's question: a chain is per (period, listing,
    /// holding account), so a reinvestment in an *earlier, closed* period is
    /// still accepted while a later period holds trades — nothing in the later
    /// period's chain reads across the boundary.
    #[tokio::test]
    async fn a_later_periods_reinvestment_does_not_block_an_earlier_periods() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "AUD").await;
        enrol_period(
            &pool,
            1,
            1,
            "2024-01-01",
            Some("2024-07-01"),
            ResidualHandling::CarryForward,
        )
        .await;
        enrol_period(
            &pool,
            2,
            1,
            "2024-07-01",
            None,
            ResidualHandling::CarryForward,
        )
        .await;

        insert_distribution_dated(
            &pool,
            1,
            1,
            "2024-03-31",
            None,
            Decimal::from(100),
            Decimal::ZERO,
        )
        .await;
        insert_distribution_dated(
            &pool,
            2,
            1,
            "2024-09-30",
            None,
            Decimal::from(100),
            Decimal::ZERO,
        )
        .await;

        // Period 2's reinvestment first...
        db_reinvest(&pool, 2, &body("9")).await.unwrap();
        // ...does not block period 1's, which runs its own chain.
        let first = db_reinvest(&pool, 1, &body("9")).await.unwrap();
        assert_eq!(first.quantity, Decimal::from(11));
        assert_eq!(first.residual_brought_forward, Decimal::ZERO);
    }

    /// The rejection reaches the API as a `422` carrying the reason the web
    /// UI shows — the same shape as undo's mid-chain refusal.
    #[tokio::test]
    async fn api_out_of_order_reinvestment_is_422_naming_the_remedy() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "AUD").await;
        enrol(&pool, 1, ResidualHandling::CarryForward).await;
        insert_distribution_dated(
            &pool,
            1,
            1,
            "2024-03-28",
            None,
            Decimal::from(105),
            Decimal::ZERO,
        )
        .await;
        insert_distribution_dated(
            &pool,
            2,
            1,
            "2024-09-30",
            None,
            Decimal::from(107),
            Decimal::ZERO,
        )
        .await;

        let app = client(&pool);
        app.post_raw("/income/2/reinvest", r#"{"reinvestment_price":"10"}"#)
            .await
            .expect_status(StatusCode::CREATED);
        let resp = app
            .post_raw("/income/1/reinvest", r#"{"reinvestment_price":"9"}"#)
            .await;
        let (status, body) = resp.status_and_body();
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
        assert!(
            body.contains("2024-09-30") && body.contains("payment order"),
            "unexpected body: {body}"
        );
    }

    #[tokio::test]
    async fn api_unreinvest_round_trip_and_rejections() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "AUD").await;
        enrol(&pool, 1, ResidualHandling::CarryForward).await;
        insert_distribution(&pool, 1, 1, Decimal::from(100), Decimal::ZERO).await;
        db_reinvest(&pool, 1, &body("9")).await.unwrap();

        let app = client(&pool);
        let resp = app.delete("/income/1/reinvest").await;
        assert_eq!(resp.status, StatusCode::NO_CONTENT);

        // Nothing left to undo → 422; unknown income → 404.
        let resp = app.delete("/income/1/reinvest").await;
        assert_eq!(resp.status, StatusCode::UNPROCESSABLE_ENTITY);
        let resp = app.delete("/income/99/reinvest").await;
        assert_eq!(resp.status, StatusCode::NOT_FOUND);
    }

    /// SCENARIOS V-d: a reinvestment's DRP trade is dated the distribution's
    /// `date_paid` by default and by the body when one is supplied, so it can
    /// land behind a whole-holding operation of the listing that has already
    /// run — units that operation could never consume. Refused both ways with
    /// the same `422` every parcel-creating path answers, and nothing written.
    #[tokio::test]
    async fn reinvest_dated_before_an_executed_recognise_is_refused() {
        let pool = test_pool().await;
        test_support::recognised_worthless_listing(
            &pool,
            1,
            "DEAD",
            "2024-01-02".parse().unwrap(),
            90,
            "2024-12-02".parse().unwrap(),
        )
        .await;
        enrol(&pool, 1, ResidualHandling::CarryForward).await;
        insert_distribution(&pool, 1, 1, Decimal::from(100), Decimal::ZERO).await;

        // The default trade date — the distribution's own `date_paid`.
        let err = db_reinvest(&pool, 1, &body("2")).await.unwrap_err();
        assert!(
            matches!(err, ReinvestError::BackDatedOverWholeHolding(_)),
            "expected the whole-holding refusal, got: {err:?}"
        );

        // …and a date stated on the body, which is the other way in.
        let stated = ReinvestBody {
            date: Some("2024-02-06".parse().unwrap()),
            ..body("2")
        };
        let err = db_reinvest(&pool, 1, &stated).await.unwrap_err();
        assert!(
            matches!(err, ReinvestError::BackDatedOverWholeHolding(_)),
            "expected the whole-holding refusal, got: {err:?}"
        );

        let response = client(&pool)
            .post(
                "/income/1/reinvest",
                &serde_json::json!({"reinvestment_price": "2"}),
            )
            .await;
        let (status, detail) = response.status_and_body();
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
        assert!(detail.contains("worthless-shares recognise"), "{detail}");
        let reinvested: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM income WHERE id = 1 AND reinvestment_trade_id IS NOT NULL)",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert!(!reinvested);
    }

    /// A stated allotment's `units × reinvestment_price` is the DRP parcel's
    /// cost base, and both figures are the taxpayer's — so the product is
    /// bounded by nothing but the type. It used to panic inside that multiply,
    /// answering a bare `500` with an empty body; now it is a `422` naming the
    /// product and the limit (SCENARIOS W-e).
    ///
    /// The registry-default branch is deliberately not guarded: it derives the
    /// quantity *from* the cash (`available / price`), so its product can
    /// never exceed the recorded distribution — the control below.
    #[tokio::test]
    async fn an_unrepresentable_reinvested_cost_base_is_refused_naming_it() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "USD").await;
        enrol(&pool, 1, ResidualHandling::CarryForward).await;
        insert_distribution(&pool, 1, 1, "68.66".parse().unwrap(), Decimal::ZERO).await;

        let response = client(&pool)
            .post(
                "/income/1/reinvest",
                &serde_json::json!({
                    "reinvestment_price": "1000000000000000",
                    "units": "1000000000000000"
                }),
            )
            .await;
        let (status, detail) = response.status_and_body();
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{detail}");
        assert!(
            detail.contains(concat!(
                "reinvestment_price 1000000000000000",
                " × units 1000000000000000"
            )),
            "the product is not named: {detail}"
        );
        assert!(
            detail.contains(&Decimal::MAX.to_string()),
            "the limit is not named: {detail}"
        );
        let n: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM trades")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(n, 0);

        // The control: the ordinary reinvestment of the same distribution
        // still lands, so the bound touches only what the type cannot hold.
        let trade = db_reinvest(&pool, 1, &body("2")).await.unwrap();
        assert_eq!(trade.quantity, Decimal::from(34));
    }

    /// A listing enrolled in a DRP, carrying a 1000-for-1 split dated after the
    /// distribution, so a reinvested parcel is re-based at read time by it.
    /// The distribution pays exactly `0.0079`, which at a near-nil
    /// reinvestment price buys 7.9e25 units.
    async fn enrolled_listing_behind_a_split(pool: &SqlitePool, cash: &str) {
        insert_listing(pool, 1, "AUD").await;
        enrol(pool, 1, ResidualHandling::CarryForward).await;
        insert_distribution(pool, 1, 1, cash.parse().unwrap(), Decimal::ZERO).await;
        crate::entities::corporate_action::db_upsert(
            pool,
            &crate::entities::corporate_action::CorporateAction {
                id: 10,
                listing_id: 1,
                date: "2024-06-01".parse().unwrap(),
                kind: crate::entities::corporate_action::ActionKind::ShareSplit {
                    split_new_units: Decimal::from(1000),
                    split_old_units: Decimal::ONE,
                },
            },
        )
        .await
        .unwrap();
    }

    /// W-e's bound on this path is `units × reinvestment_price`, which a
    /// near-nil price satisfies at any unit count at all — so nothing asked
    /// what the listing's recorded 1000-for-1 split does to 1e27 reinvested
    /// units. The reinvestment answered `201` and then killed every
    /// open-holdings read of the whole portfolio. Refused now, naming the
    /// quantity and the ratio, with nothing written.
    #[tokio::test]
    async fn api_a_reinvested_quantity_a_recorded_ratio_rebases_out_of_range_is_refused() {
        let pool = test_pool().await;
        enrolled_listing_behind_a_split(&pool, "0.1").await;

        let response = client(&pool)
            .post(
                "/income/1/reinvest",
                &serde_json::json!({
                    "reinvestment_price": "0.0000000000000000000000000001",
                    "units": "1000000000000000000000000000"
                }),
            )
            .await;
        let (status, detail) = response.status_and_body();
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{detail}");
        assert!(
            detail.contains("quantity 1000000000000000000000000000 × new units 1000 / old units 1"),
            "the quantity and the ratio are not named: {detail}"
        );
        let n: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM trades")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(n, 0);
    }

    /// The same fact with the units **derived** rather than stated: the
    /// registry default divides the cash by the price, so a near-nil price
    /// makes an ordinary distribution buy 1e27 units. That is the path W-e
    /// deliberately left unbounded — its product can never exceed the recorded
    /// distribution — which is exactly why the unit count needs its own check.
    #[tokio::test]
    async fn api_a_derived_reinvested_quantity_beyond_the_range_is_refused_too() {
        let pool = test_pool().await;
        enrolled_listing_behind_a_split(&pool, "0.1").await;

        let err = db_reinvest(&pool, 1, &body("0.0000000000000000000000000001"))
            .await
            .unwrap_err();
        assert!(
            matches!(err, ReinvestError::UnrepresentableRebasedQuantity(_)),
            "expected the re-based-quantity refusal, got: {err:?}"
        );
        let n: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM trades")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(n, 0);
    }

    /// The control, pinned at the figures this build answered before the
    /// refusal existed: 7.9e25 reinvested units behind the same real
    /// 1000-for-1 split re-base to 7.9e28, inside the range, so the
    /// reinvestment lands and the parcel reports.
    #[tokio::test]
    async fn api_a_large_reinvested_quantity_a_recorded_ratio_still_fits_lands_and_reports() {
        let pool = test_pool().await;
        enrolled_listing_behind_a_split(&pool, "0.0079").await;

        let trade = db_reinvest(
            &pool,
            1,
            &body_units(
                "0.0000000000000000000000000001",
                "79000000000000000000000000",
            ),
        )
        .await
        .unwrap();
        assert_eq!(trade.trade_type, TradeType::DRP);

        let rows: Vec<serde_json::Value> = ApiClient::full(&pool)
            .get_json("/portfolio/open-parcels")
            .await;
        assert_eq!(rows.len(), 1, "{rows:?}");
        assert_eq!(rows[0]["original_quantity"], "79000000000000000000000000");
        assert_eq!(
            rows[0]["remaining_quantity"],
            "79000000000000000000000000000"
        );
        assert_eq!(
            rows[0]["original_cost_base"],
            "0.0079000000000000000000000000"
        );
    }
}
