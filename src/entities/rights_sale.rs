//! Atomic rights sale/lapse: dispose of a `RightsIssue`'s rights themselves
//! (see `docs/ato/rights-issues.md` Example 39 and
//! `docs/ato/retail-premiums.md` TR 2017/4).
//!
//! Selling renounceable rights (or letting them lapse, or receiving a retail
//! premium for not taking them up) is a CGT event on the **rights**, not on
//! the original shares — the holding is unchanged, so the disposal is its own
//! row (`rights_sales`), never a Sell trade: a Sell would consume share
//! parcels. The realised-gains report reads these rows alongside ordinary
//! Sells (`source = RightsSale`) and the net-capital-gain report nets them
//! with everything else.
//!
//! Free rights are taken to have been **acquired when the original shares
//! were acquired**, so each sale carries allocations anchoring the sold
//! rights to original parcels (Buy/DRP trades of the listing dated before the
//! record date): the allocation's 12-month discount clock runs from its
//! parcel's (possibly deemed) acquisition date. Unlike a Sell's
//! `parcel_allocations`, these allocations consume nothing.
//!
//! Only a **renounceable** offer's rights can be sold at all: under a
//! non-renounceable one the entitlements cannot be traded, transferred or
//! assigned (TR 2012/1 para 2), and the payment that does arise for not taking
//! one up — a **retail premium** — is an unfranked dividend, entered as
//! `income`, not a capital gain (`docs/ato/retail-premiums.md`). So proceeds
//! (and any cost paid to acquire the rights) are refused on a non-renounceable
//! issue, while a nil-proceeds **lapse** stays recordable: a non-renounceable
//! right lapses like any other, and nil against nil is a non-event on either
//! treatment that still consumes the entitlement here.
//!
//! Cost base: nil for rights issued free; `rights_cost` carries the total
//! paid to acquire the disposed rights (the purchased-rights case), which the
//! report apportions over the allocations — so nil proceeds on a paid right
//! (a lapse) realises a capital loss, while a lapsed free right is a nil/nil
//! non-event that still consumes entitlement.
//!
//! Caps, validated at write time in one transaction:
//! - **total**: rights used against the action — exercises plus sales, via
//!   the shared `rights_exercise::db_rights_used` — may not exceed the
//!   holding's record-date entitlement;
//! - **per parcel**: rights anchored to a parcel (across all sales of the
//!   action) may not exceed the entitlement that parcel's record-date units
//!   earned, so a sale can't borrow an older parcel's acquisition date for
//!   the discount.
//!
//! To keep the validated figures honest a rights sale is immutable (no PUT —
//! delete it and re-enter), the action is frozen while sales reference it
//! (`entities::corporate_action`), and an anchoring parcel Buy is frozen
//! against `PUT`/`DELETE /trades` while referenced (`entities::trade`).

use crate::entities::corporate_action::{
    self, ActionKind, NOTHING_PAID_FOR_NON_RENOUNCEABLE_RIGHTS, SplitEvent, sold_in_acquired_units,
    split_adjusted_quantity,
};
use crate::entities::rights_exercise::{db_held_at_record_date, db_rights_used, entitled_units};
use crate::entities::trade::TradeType;
use crate::infra::db::write_tx;
use crate::infra::decimal::{Money, parse_dec, row_dec};
use crate::infra::http::{self, ApiError};
use axum::{
    Json, Router,
    extract::{Path, State},
    http::StatusCode,
    routing::{get, post},
};
use chrono::NaiveDate;
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use sqlx::{Row, SqlitePool, sqlite::SqliteRow};
use std::collections::HashMap;

#[derive(Debug, Clone, Serialize)]
pub struct RightsSale {
    pub id: i64,
    pub rights_action_id: i64,
    /// Sale (or lapse/expiry) date.
    pub date: NaiveDate,
    /// Rights disposed of, in record-date (as-issued) rights units.
    pub units: Decimal,
    /// Per-right capital proceeds in the issue's currency; 0 = lapse.
    pub proceeds_per_right: Decimal,
    /// Total paid to acquire the disposed rights (0 for rights issued free).
    pub rights_cost: Decimal,
    pub fx_rate: Decimal,
    pub holding_account_id: i64,
    pub allocations: Vec<RightsSaleAllocation>,
}

#[derive(Debug, Clone, Serialize)]
pub struct RightsSaleAllocation {
    pub purchase_trade_id: i64,
    pub units: Decimal,
}

fn sale_from_row(row: &SqliteRow) -> Result<RightsSale, sqlx::Error> {
    Ok(RightsSale {
        id: row.try_get("id")?,
        rights_action_id: row.try_get("rights_action_id")?,
        date: row.try_get("date")?,
        units: row_dec(row, "units")?,
        proceeds_per_right: row_dec(row, "proceeds_per_right")?,
        rights_cost: row_dec(row, "rights_cost")?,
        fx_rate: row_dec(row, "fx_rate")?,
        holding_account_id: row.try_get("holding_account_id")?,
        allocations: Vec::new(),
    })
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AllocationInput {
    pub purchase_trade_id: i64,
    #[serde(deserialize_with = "crate::infra::decimal::strict_decimal")]
    pub units: Decimal,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SellRightsBody {
    /// Sale (or lapse) date. Must not precede the issue's record date.
    pub date: NaiveDate,
    /// Rights disposed of (strictly positive, record-date rights units).
    #[serde(deserialize_with = "crate::infra::decimal::strict_decimal")]
    pub units: Decimal,
    /// Per-right proceeds in the action's currency (defaults to 0 — a lapse).
    #[serde(
        default,
        deserialize_with = "crate::infra::decimal::strict_optional_decimal"
    )]
    pub proceeds_per_right: Option<Decimal>,
    /// Total amount paid to acquire the disposed rights, in the action's
    /// currency (defaults to 0 — rights issued free have a nil cost base).
    #[serde(
        default,
        deserialize_with = "crate::infra::decimal::strict_optional_decimal"
    )]
    pub rights_cost: Option<Decimal>,
    /// Optional foreign-per-AUD override (reports prefer the ATO rate and
    /// fall back to this — see `infra::fx`). When omitted, the sale month's
    /// ATO rate is resolved and bound (AUD resolves to 1); a non-AUD,
    /// non-nil sale in a month with no imported rate is refused rather than
    /// defaulted to parity ([`SellRightsError::MissingFxRate`]).
    #[serde(
        default,
        deserialize_with = "crate::infra::decimal::strict_optional_decimal"
    )]
    pub fx_rate: Option<Decimal>,
    /// The holding account the disposal is reported under. Defaults to the
    /// seeded default account when omitted.
    #[serde(default = "crate::entities::holding_account::default_holding_account_id")]
    pub holding_account_id: i64,
    /// Which original parcels the sold rights are anchored to; must sum
    /// exactly to `units`.
    pub allocations: Vec<AllocationInput>,
}

#[derive(thiserror::Error, Debug)]
pub enum SellRightsError {
    #[error("rights sale write failed: {0}")]
    Db(#[from] sqlx::Error),
    /// No corporate action with that id.
    #[error("no corporate action with that id")]
    ActionNotFound,
    /// The action is not a RightsIssue.
    #[error("that corporate action is not a rights issue")]
    NotARightsIssue,
    /// Positive proceeds against a **non-renounceable** offer. Under such an
    /// offer the entitlements "cannot be traded, transferred, assigned or
    /// otherwise dealt with" (TR 2012/1 para 2, which is what the ATO's
    /// definition of non-renounceable amounts to — `docs/ato/retail-premiums.md`),
    /// so there are no proceeds to receive on them: the only payment that
    /// arises is a **retail premium** for not taking the entitlement up, and
    /// that is an unfranked dividend assessable under s 44 ITAA 1936 (TR
    /// 2012/1), with any capital gain on it reduced to nil by s 118-20. Booked
    /// here it would be a capital gain — halved again by the discount, since
    /// free rights inherit the original shares' acquisition date — instead of
    /// fully assessable income at item 11S (SCENARIOS AA-b). Mapped to `422`
    /// naming the income path.
    #[error("this rights issue is a non-renounceable offer, whose rights cannot be sold")]
    ProceedsOnNonRenounceableOffer,
    /// A cost paid to acquire rights under a **non-renounceable** offer. The
    /// same terms make it impossible: an entitlement that cannot be traded,
    /// transferred or assigned cannot have been bought, so the cost has no
    /// source — and left unchecked it would realise a capital loss on a lapse
    /// (proceeds nil against a positive cost base) out of an amount that could
    /// never have been paid. Mapped to `422` off the shared
    /// [`NOTHING_PAID_FOR_NON_RENOUNCEABLE_RIGHTS`] clause, the same one the
    /// exercise path answers with
    /// (`entities::rights_exercise::ExerciseError::RightsCostOnNonRenounceableOffer`).
    #[error("this rights issue is a non-renounceable offer, whose rights cannot be bought")]
    RightsCostOnNonRenounceableOffer,
    /// `units` is not strictly positive.
    #[error("the number of rights sold must be greater than zero")]
    NonPositiveUnits,
    /// `proceeds_per_right` is negative.
    #[error("the proceeds per right cannot be negative")]
    NegativeProceeds,
    /// `rights_cost` is negative.
    #[error("the rights cost cannot be negative")]
    NegativeRightsCost,
    /// The sale date precedes the issue's record date.
    #[error("the sale date is before the issue's record date")]
    BeforeRecordDate,
    /// An allocation anchors a zero or negative number of rights. Split out
    /// from [`SellRightsError::AllocationsDontSum`] so that one can name its
    /// figures: a negative row can sum correctly while being nonsense, so
    /// "they sum to N" would be the wrong thing to say about it.
    #[error("each anchoring parcel allocation must be for a positive number of rights")]
    AllocationNotPositive,
    /// The allocations (none at all counting as nil) do not sum to `units`.
    /// Both figures are carried so the refusal can name them — see
    /// [`crate::entities::sell::SellError::AllocationMismatch`], the same
    /// wording on the Sell side (SCENARIOS Y-b).
    #[error("the parcel allocations sum to {allocated}, not the rights sold {units}")]
    AllocationsDontSum { allocated: Decimal, units: Decimal },
    /// An allocation's parcel is missing, not a Buy/DRP of the issue's
    /// listing, or not held before the record date (so it earned no rights).
    #[error(
        "an allocated parcel is not a Buy/DRP of the issue's listing held before the record date"
    )]
    NotAnOriginalParcel,
    /// Rights anchored to a parcel (across the action's sales) exceed the
    /// entitlement that parcel's record-date units earned.
    #[error("the rights anchored to a parcel exceed the entitlement its record-date units earned")]
    ExceedsParcelEntitlement,
    /// Total rights used — exercises plus sales — would exceed the
    /// entitlement earned by the units held at the record date.
    #[error("the rights sold and exercised exceed the entitlement earned at the record date")]
    ExceedsEntitlement,
    /// A non-AUD sale whose month has no imported ATO rate and whose body
    /// states no `fx_rate`: the proceeds and rights cost have no rate to
    /// convert at, and binding a placeholder 1 would silently book them at
    /// parity via `FxOverride::Fallback(1)` — the path the ESS vest refuses
    /// (`ess_vest::VestError::MissingFxRate`). Mapped to 422.
    #[error("no ATO FX rate for {currency} in {month} and the request states none")]
    MissingFxRate { currency: String, month: String },
}

impl From<SellRightsError> for ApiError {
    fn from(e: SellRightsError) -> Self {
        match e {
            SellRightsError::ActionNotFound => {
                ApiError::not_found("no corporate action with that id")
            }
            SellRightsError::NotARightsIssue => {
                ApiError::unprocessable("that corporate action is not a rights issue")
            }
            // A non-renounceable offer's rights cannot be sold at all, and the
            // one payment that does arise under it is income, not capital →
            // 422 naming the ruling and the path the premium belongs on.
            SellRightsError::ProceedsOnNonRenounceableOffer => ApiError::unprocessable(
                "this rights issue is a non-renounceable offer, whose entitlements cannot be \
                 traded, transferred or assigned — there are no capital proceeds to record on \
                 them. A retail premium paid for not taking the entitlement up is an unfranked \
                 dividend (TR 2012/1), not a capital gain: enter it as unfranked dividend income \
                 against the listing instead. Only a lapse — nil proceeds — is recorded here",
            ),
            // The same impossible amount the exercise path refuses, off the
            // one shared clause, each naming what it would have done to its
            // own figures.
            SellRightsError::RightsCostOnNonRenounceableOffer => ApiError::unprocessable(format!(
                "{NOTHING_PAID_FOR_NON_RENOUNCEABLE_RIGHTS}, and a cost recorded here would \
                 realise a capital loss on a lapse out of an amount that was never paid: leave \
                 the rights cost at 0"
            )),
            SellRightsError::NonPositiveUnits => {
                ApiError::unprocessable("the number of rights sold must be greater than zero")
            }
            SellRightsError::NegativeProceeds => {
                ApiError::unprocessable("the proceeds per right cannot be negative")
            }
            SellRightsError::NegativeRightsCost => {
                ApiError::unprocessable("the rights cost cannot be negative")
            }
            SellRightsError::BeforeRecordDate => {
                ApiError::unprocessable("the sale date is before the issue's record date")
            }
            SellRightsError::AllocationNotPositive => ApiError::unprocessable(
                "each anchoring parcel allocation must be for a positive number of rights",
            ),
            SellRightsError::AllocationsDontSum { allocated, units } => ApiError::unprocessable(
                format!("the allocations sum to {allocated}, not the {units} rights sold"),
            ),
            SellRightsError::NotAnOriginalParcel => ApiError::unprocessable(
                "an allocated parcel is not a Buy/DRP of the issue's listing held before the \
                 record date",
            ),
            SellRightsError::ExceedsParcelEntitlement => ApiError::unprocessable(
                "the rights anchored to a parcel exceed the entitlement its record-date units \
                 earned",
            ),
            SellRightsError::MissingFxRate { currency, month } => ApiError::unprocessable(format!(
                "this rights sale is in {currency} but no ATO/RBA rate has been imported \
                     for {currency} in {month} and the request states no fx_rate — supply \
                     fx_rate or import that month's RBA rates; recording without one would \
                     convert the proceeds at parity (1 AUD per {currency})"
            )),
            SellRightsError::ExceedsEntitlement => ApiError::unprocessable(
                "the rights sold and exercised exceed the entitlement earned by the holding at \
                 the record date",
            ),
            SellRightsError::Db(err) => err.into(),
        }
    }
}

pub fn router() -> Router<SqlitePool> {
    Router::new()
        .route("/corporate_actions/{id}/sell_rights", post(sell_rights))
        .route("/rights_sales", get(list))
        .route("/rights_sales/{id}", get(get_one).delete(delete_one))
}

/// The amounts a sale carries once the body-only checks have passed, each
/// defaulted from its optional field.
struct SaleAmounts {
    proceeds_per_right: Decimal,
    rights_cost: Decimal,
}

impl SaleAmounts {
    /// A nil/nil disposal — a lapse of a free right — converts nothing, so no
    /// FX rate can affect what it books (see [`resolve_fx_rate`]).
    fn converts_nothing(&self) -> bool {
        self.proceeds_per_right == Decimal::ZERO && self.rights_cost == Decimal::ZERO
    }
}

/// The checks that need nothing but the request: rights sold strictly
/// positive, proceeds and cost non-negative, every allocation positive, and
/// the allocations summing exactly to the rights sold. They run before the
/// transaction is opened, since none of them reads the database.
fn check_body(body: &SellRightsBody) -> Result<SaleAmounts, SellRightsError> {
    if body.units <= Decimal::ZERO {
        return Err(SellRightsError::NonPositiveUnits);
    }
    let proceeds_per_right = body.proceeds_per_right.unwrap_or(Decimal::ZERO);
    if proceeds_per_right < Decimal::ZERO {
        return Err(SellRightsError::NegativeProceeds);
    }
    let rights_cost = body.rights_cost.unwrap_or(Decimal::ZERO);
    if rights_cost < Decimal::ZERO {
        return Err(SellRightsError::NegativeRightsCost);
    }
    if body.allocations.iter().any(|a| a.units <= Decimal::ZERO) {
        return Err(SellRightsError::AllocationNotPositive);
    }
    // No allocations at all sums to nil, which `body.units` (already checked
    // positive above) can never equal — so the empty case falls out here,
    // named with its figures like any other shortfall.
    let allocated: Decimal = body.allocations.iter().map(|a| a.units).sum();
    if allocated != body.units {
        return Err(SellRightsError::AllocationsDontSum {
            allocated,
            units: body.units,
        });
    }
    Ok(SaleAmounts {
        proceeds_per_right,
        rights_cost,
    })
}

/// The issue's own terms, as the per-parcel anchoring cap needs them: which
/// action and listing the rights were earned under, the record date they were
/// earned on, the offer's ratio, and the listing's splits (every quantity in
/// the walk is compared in record-date units).
struct Anchoring<'a> {
    action_id: i64,
    listing_id: i64,
    record_date: NaiveDate,
    rights_units: Decimal,
    rights_held_units: Decimal,
    splits: &'a [SplitEvent],
}

/// The allocated parcel's units still held when the record date arrived, in
/// record-date units — its as-acquired quantity minus the units consumed by
/// sales dated *before* the record date, re-based across any splits. That is
/// what decides the entitlement the parcel earned.
///
/// Errors with [`SellRightsError::NotAnOriginalParcel`] unless the trade is a
/// Buy/DRP of the issue's own listing dated before the record date: a parcel
/// that is none of those earned no rights, so no sold right can borrow its
/// acquisition date for the 12-month discount clock.
async fn parcel_units_at_record_date(
    conn: &mut sqlx::SqliteConnection,
    anchoring: &Anchoring<'_>,
    purchase_trade_id: i64,
) -> Result<Decimal, SellRightsError> {
    let parcel =
        sqlx::query("SELECT trade_type, listing_id, date, quantity FROM trades WHERE id = ?")
            .bind(purchase_trade_id)
            .fetch_optional(&mut *conn)
            .await?;
    let Some(parcel) = parcel else {
        return Err(SellRightsError::NotAnOriginalParcel);
    };
    let parcel_listing: i64 = parcel.try_get("listing_id")?;
    let parcel_date: NaiveDate = parcel.try_get("date")?;
    let parcel_type: TradeType = parcel.try_get("trade_type")?;
    if !parcel_type.is_acquisition()
        || parcel_listing != anchoring.listing_id
        || parcel_date >= anchoring.record_date
    {
        return Err(SellRightsError::NotAnOriginalParcel);
    }
    let parcel_qty = parse_dec("quantity", parcel.try_get("quantity")?)?;

    let sold_rows = sqlx::query(
        "SELECT s.date AS sale_date, pa.quantity_allocated \
         FROM parcel_allocations pa JOIN trades s ON s.id = pa.sale_trade_id \
         WHERE pa.purchase_trade_id = ? AND s.date < ?",
    )
    .bind(purchase_trade_id)
    .bind(anchoring.record_date)
    .fetch_all(&mut *conn)
    .await?;
    let mut sold: Vec<(NaiveDate, Decimal)> = Vec::with_capacity(sold_rows.len());
    for row in &sold_rows {
        sold.push((
            row.try_get("sale_date")?,
            parse_dec("quantity_allocated", row.try_get("quantity_allocated")?)?,
        ));
    }
    let remaining =
        parcel_qty - sold_in_acquired_units(sold.iter().copied(), anchoring.splits, parcel_date);
    Ok(split_adjusted_quantity(
        remaining,
        anchoring.splits,
        parcel_date,
        Some(anchoring.record_date),
    ))
}

/// Rights already anchored to a parcel by the action's *stored* sales, in
/// record-date units. Read once per distinct parcel — the in-request running
/// total carries it forward across a request that splits one parcel over
/// several allocations, so it is never counted twice.
async fn prior_anchored_units(
    conn: &mut sqlx::SqliteConnection,
    action_id: i64,
    purchase_trade_id: i64,
) -> Result<Decimal, SellRightsError> {
    let prior: Vec<String> = sqlx::query_scalar(
        "SELECT rsa.units FROM rights_sale_allocations rsa \
         JOIN rights_sales rs ON rs.id = rsa.rights_sale_id \
         WHERE rs.rights_action_id = ? AND rsa.purchase_trade_id = ?",
    )
    .bind(action_id)
    .bind(purchase_trade_id)
    .fetch_all(&mut *conn)
    .await?;
    let mut anchored = Decimal::ZERO;
    for units in prior {
        anchored += parse_dec("units", units)?;
    }
    Ok(anchored)
}

/// Per-parcel cap: each anchoring parcel must have earned the rights anchored
/// to it (cumulatively, across the action's sales), per its units held when
/// the record date arrived. In-request totals accumulate so a request can't
/// split one parcel over two allocations to dodge the cap.
///
/// A validation pass and nothing else: it computes no figure the write goes on
/// to store, so it reads the caller's transaction and answers only whether the
/// anchoring stands.
async fn check_parcel_anchoring(
    conn: &mut sqlx::SqliteConnection,
    anchoring: &Anchoring<'_>,
    allocations: &[AllocationInput],
) -> Result<(), SellRightsError> {
    let mut in_request: HashMap<i64, Decimal> = HashMap::new();
    for alloc in allocations {
        let at_record =
            parcel_units_at_record_date(conn, anchoring, alloc.purchase_trade_id).await?;
        let parcel_entitled = entitled_units(
            at_record,
            anchoring.rights_units,
            anchoring.rights_held_units,
        );

        let already = match in_request.get(&alloc.purchase_trade_id) {
            Some(&running) => running,
            None => {
                prior_anchored_units(conn, anchoring.action_id, alloc.purchase_trade_id).await?
            }
        };
        let anchored = already + alloc.units;
        if parcel_entitled.is_some_and(|parcel_entitled| anchored > parcel_entitled) {
            return Err(SellRightsError::ExceedsParcelEntitlement);
        }
        in_request.insert(alloc.purchase_trade_id, anchored);
    }
    Ok(())
}

/// The rate the row carries. The realised-gains report converts both legs
/// via `FxOverride::Fallback(fx_rate)`, so a placeholder 1 would become
/// the real answer exactly when the sale month's ATO rate is missing and
/// book USD proceeds 1:1 as AUD — the silent-parity path the ESS vest
/// refuses (`ess_vest::VestError::MissingFxRate`). The body's stated rate
/// is bound when the caller gave one; otherwise the sale month's ATO rate
/// is resolved and bound (AUD resolves to 1), and a month with neither is
/// refused rather than invented. A nil/nil lapse converts nothing
/// (`converts_nothing`), so a missing month cannot corrupt it and parity is
/// bound harmlessly.
async fn resolve_fx_rate(
    conn: &mut sqlx::SqliteConnection,
    stated: Option<Decimal>,
    currency: &str,
    date: NaiveDate,
    converts_nothing: bool,
) -> Result<Decimal, SellRightsError> {
    if let Some(rate) = stated {
        return Ok(rate);
    }
    let fx = crate::infra::fx::FxRates::load(&mut *conn).await?;
    match fx.resolve_rate(currency, date, crate::infra::fx::FxOverride::None) {
        Ok(rate) => Ok(rate),
        Err(_) if converts_nothing => Ok(Decimal::ONE),
        Err(_) => Err(SellRightsError::MissingFxRate {
            currency: currency.to_string(),
            month: date.format("%Y-%m").to_string(),
        }),
    }
}

/// Record a rights sale/lapse with its parcel anchoring, atomically.
///
/// The order is the order the refusals are owed in: the request's own figures
/// ([`check_body`]) before anything is opened, then what the offer's terms
/// allow, then the two caps — the total one shared with exercises, and the
/// per-parcel anchoring walk ([`check_parcel_anchoring`]) — and only then the
/// rate and the insert. Every check above the insert is a pure validation
/// pass: none of them produces a figure the write stores, so the whole walk
/// separates from it.
pub async fn db_sell_rights(
    pool: &SqlitePool,
    action_id: i64,
    body: &SellRightsBody,
) -> Result<RightsSale, SellRightsError> {
    let amounts = check_body(body)?;

    let mut tx = write_tx(pool).await?;

    let action = match corporate_action::db_get_tx(&mut *tx, action_id).await? {
        Some(a) => a,
        None => return Err(SellRightsError::ActionNotFound),
    };
    let (rights_units, rights_held_units, currency, renounceable) = match &action.kind {
        ActionKind::RightsIssue {
            rights_units,
            rights_held_units,
            currency,
            renounceable,
            ..
        } => (
            *rights_units,
            *rights_held_units,
            currency.clone(),
            *renounceable,
        ),
        _ => return Err(SellRightsError::NotARightsIssue),
    };
    // What the offer's own terms allow. A non-renounceable entitlement cannot
    // be traded, transferred or assigned (TR 2012/1 para 2), so it can neither
    // be sold for proceeds nor have been bought for a cost — the payment that
    // does arise, a retail premium for not taking it up, is an unfranked
    // dividend and belongs on the income path. What remains recordable is the
    // **lapse**: nil proceeds against a nil cost, which a non-renounceable
    // right can certainly do and which is a non-event on either treatment,
    // while still consuming the entitlement here (SCENARIOS AA-b).
    if !renounceable {
        if amounts.proceeds_per_right > Decimal::ZERO {
            return Err(SellRightsError::ProceedsOnNonRenounceableOffer);
        }
        if amounts.rights_cost > Decimal::ZERO {
            return Err(SellRightsError::RightsCostOnNonRenounceableOffer);
        }
    }
    let record_date = action.date;
    if body.date < record_date {
        return Err(SellRightsError::BeforeRecordDate);
    }

    let splits = corporate_action::db_splits_for_listing(&mut *tx, action.listing_id).await?;

    // Total cap, shared with the exercise operation: every right used against
    // the action — exercised or sold — comes out of one entitlement.
    let held = db_held_at_record_date(&mut tx, action.listing_id, record_date, &splits).await?;
    let entitled = entitled_units(held, rights_units, rights_held_units);
    let used = db_rights_used(&mut tx, action_id, record_date, &splits).await? + body.units;
    // `None` means the entitlement is past `Decimal`'s range, so nothing the
    // request can name reaches it (`entitled_units`).
    if entitled.is_some_and(|entitled| used > entitled) {
        return Err(SellRightsError::ExceedsEntitlement);
    }

    check_parcel_anchoring(
        &mut tx,
        &Anchoring {
            action_id,
            listing_id: action.listing_id,
            record_date,
            rights_units,
            rights_held_units,
            splits: &splits,
        },
        &body.allocations,
    )
    .await?;

    let fx_rate = resolve_fx_rate(
        &mut tx,
        body.fx_rate,
        &currency,
        body.date,
        amounts.converts_nothing(),
    )
    .await?;
    let result = sqlx::query(
        "INSERT INTO rights_sales \
         (rights_action_id, date, units, proceeds_per_right, rights_cost, fx_rate, \
          holding_account_id) \
         VALUES (?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(action_id)
    .bind(body.date)
    .bind(Money(body.units))
    .bind(Money(amounts.proceeds_per_right))
    .bind(Money(amounts.rights_cost))
    .bind(Money(fx_rate))
    .bind(body.holding_account_id)
    .execute(&mut *tx)
    .await?;
    let new_id = result.last_insert_rowid();
    for alloc in &body.allocations {
        sqlx::query(
            "INSERT INTO rights_sale_allocations (rights_sale_id, purchase_trade_id, units) \
             VALUES (?, ?, ?)",
        )
        .bind(new_id)
        .bind(alloc.purchase_trade_id)
        .bind(Money(alloc.units))
        .execute(&mut *tx)
        .await?;
    }

    tx.commit().await?;

    // Read the freshly created sale back so the response is exactly what was
    // stored.
    db_get(pool, new_id)
        .await?
        .ok_or(SellRightsError::Db(sqlx::Error::RowNotFound))
}

async fn attach_allocations(
    pool: &SqlitePool,
    sales: &mut [RightsSale],
) -> Result<(), sqlx::Error> {
    let rows = sqlx::query(
        "SELECT rights_sale_id, purchase_trade_id, units FROM rights_sale_allocations ORDER BY id",
    )
    .fetch_all(pool)
    .await?;
    let mut by_sale: HashMap<i64, Vec<RightsSaleAllocation>> = HashMap::new();
    for row in &rows {
        by_sale
            .entry(row.try_get("rights_sale_id")?)
            .or_default()
            .push(RightsSaleAllocation {
                purchase_trade_id: row.try_get("purchase_trade_id")?,
                units: row_dec(row, "units")?,
            });
    }
    for sale in sales {
        sale.allocations = by_sale.remove(&sale.id).unwrap_or_default();
    }
    Ok(())
}

pub async fn db_list(pool: &SqlitePool) -> Result<Vec<RightsSale>, sqlx::Error> {
    let rows = sqlx::query("SELECT * FROM rights_sales ORDER BY date, id")
        .fetch_all(pool)
        .await?;
    let mut sales = rows
        .iter()
        .map(sale_from_row)
        .collect::<Result<Vec<_>, _>>()?;
    attach_allocations(pool, &mut sales).await?;
    Ok(sales)
}

pub async fn db_get(pool: &SqlitePool, id: i64) -> Result<Option<RightsSale>, sqlx::Error> {
    let row = sqlx::query("SELECT * FROM rights_sales WHERE id = ?")
        .bind(id)
        .fetch_optional(pool)
        .await?;
    let Some(row) = row else { return Ok(None) };
    let mut sales = vec![sale_from_row(&row)?];
    attach_allocations(pool, &mut sales).await?;
    Ok(sales.pop())
}

/// Delete a rights sale (its allocations cascade), freeing the entitlement it
/// consumed.
pub async fn db_delete(pool: &SqlitePool, id: i64) -> Result<bool, sqlx::Error> {
    let result = sqlx::query("DELETE FROM rights_sales WHERE id = ?")
        .bind(id)
        .execute(pool)
        .await?;
    Ok(result.rows_affected() > 0)
}

async fn sell_rights(
    State(pool): State<SqlitePool>,
    Path(action_id): Path<i64>,
    Json(body): Json<SellRightsBody>,
) -> Result<(StatusCode, Json<RightsSale>), ApiError> {
    let sale = db_sell_rights(&pool, action_id, &body).await?;
    Ok((StatusCode::CREATED, Json(sale)))
}

async fn list(State(pool): State<SqlitePool>) -> Result<Json<Vec<RightsSale>>, ApiError> {
    Ok(Json(db_list(&pool).await?))
}

async fn get_one(
    State(pool): State<SqlitePool>,
    Path(id): Path<i64>,
) -> Result<Json<RightsSale>, ApiError> {
    db_get(&pool, id)
        .await?
        .map(Json)
        .ok_or_else(|| ApiError::not_found("no rights sale with that id"))
}

async fn delete_one(
    State(pool): State<SqlitePool>,
    Path(id): Path<i64>,
) -> Result<StatusCode, ApiError> {
    http::deleted(db_delete(&pool, id).await?, "rights sale")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::entities::corporate_action::CorporateAction;
    use crate::entities::{listing, rights_exercise, trade};
    use crate::test_support::{self, ApiClient, dec, test_pool};

    fn d(y: i32, m: u32, day: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, day).unwrap()
    }

    async fn insert_listing(pool: &SqlitePool, id: i64) {
        test_support::listing(id)
            .ticker("RTS")
            .name("Rights Test Co")
            .security_type(listing::SecurityType::Share)
            .insert(pool)
            .await;
    }

    async fn insert_buy(pool: &SqlitePool, id: i64, date: NaiveDate, qty: &str) {
        test_support::buy(id, 1)
            .date(date)
            .settlement(date)
            .qty(qty.parse().unwrap())
            .price("2.00".parse().unwrap())
            .insert(pool)
            .await;
    }

    /// A 1-for-4 **non-renounceable** rights issue at $1.80, record date
    /// `date`: the entitlements cannot be traded, transferred or assigned, so
    /// the only payment that can arise on them is a retail premium — an
    /// unfranked dividend, not capital proceeds (TR 2012/1).
    async fn insert_non_renounceable_issue(pool: &SqlitePool, id: i64, date: NaiveDate) {
        corporate_action::db_upsert(
            pool,
            &CorporateAction {
                id,
                listing_id: 1,
                date,
                kind: ActionKind::RightsIssue {
                    rights_units: Decimal::ONE,
                    rights_held_units: Decimal::from(4),
                    exercise_price: "1.80".parse().unwrap(),
                    currency: "AUD".to_string(),
                    renounceable: false,
                },
            },
        )
        .await
        .unwrap();
    }

    /// A 1-for-4 rights issue at $1.80, record date `date`.
    async fn insert_rights_issue(pool: &SqlitePool, id: i64, date: NaiveDate) {
        corporate_action::db_upsert(
            pool,
            &CorporateAction {
                id,
                listing_id: 1,
                date,
                kind: ActionKind::RightsIssue {
                    rights_units: Decimal::ONE,
                    rights_held_units: Decimal::from(4),
                    exercise_price: "1.80".parse().unwrap(),
                    currency: "AUD".to_string(),
                    renounceable: true,
                },
            },
        )
        .await
        .unwrap();
    }

    fn body(date: NaiveDate, units: &str, parcel: i64) -> SellRightsBody {
        SellRightsBody {
            date,
            units: units.parse().unwrap(),
            proceeds_per_right: Some("0.20".parse().unwrap()),
            rights_cost: None,
            fx_rate: None,
            holding_account_id: 1,
            allocations: vec![AllocationInput {
                purchase_trade_id: parcel,
                units: units.parse().unwrap(),
            }],
        }
    }

    // DB-level tests

    #[tokio::test]
    async fn sell_rights_persists_the_sale_with_its_anchoring() {
        let pool = test_pool().await;
        insert_listing(&pool, 1).await;
        insert_buy(&pool, 1, d(2024, 1, 16), "1000").await;
        insert_rights_issue(&pool, 10, d(2024, 7, 1)).await;

        let sale = db_sell_rights(&pool, 10, &body(d(2024, 7, 20), "250", 1))
            .await
            .unwrap();
        assert_eq!(sale.rights_action_id, 10);
        assert_eq!(sale.date, d(2024, 7, 20));
        assert_eq!(sale.units, Decimal::from(250));
        assert_eq!(sale.proceeds_per_right, "0.20".parse::<Decimal>().unwrap());
        assert_eq!(sale.rights_cost, Decimal::ZERO);
        assert_eq!(sale.allocations.len(), 1);
        assert_eq!(sale.allocations[0].purchase_trade_id, 1);
        assert_eq!(sale.allocations[0].units, Decimal::from(250));

        // The holding is untouched: selling rights consumes no share parcel.
        let parcels = crate::reports::open_parcels::db_open_parcels(&pool)
            .await
            .unwrap();
        assert_eq!(parcels.len(), 1);
        assert_eq!(parcels[0].remaining_quantity, Decimal::from(1000));
    }

    /// SCENARIOS E-18/E-19: what a disposal of *free* rights realises.
    /// Letting them lapse is a nil-proceeds/nil-cost non-event that still
    /// consumes the entitlement (`docs/ato/rights-issues.md`), while a
    /// renounceable offer's retail premium is capital proceeds on the rights
    /// — a capital gain, never dividend income (TR 2017/4,
    /// `docs/ato/retail-premiums.md`) — discountable on the *original*
    /// parcel's holding period, since free rights are acquired when the
    /// shares were.
    #[tokio::test]
    async fn a_lapse_realises_nothing_while_a_retail_premium_is_a_capital_gain() {
        let pool = test_pool().await;
        insert_listing(&pool, 1).await;
        insert_buy(&pool, 1, d(2023, 1, 17), "1000").await;
        insert_rights_issue(&pool, 10, d(2024, 7, 1)).await; // entitled to 250

        // 100 rights lapse (nil proceeds), 150 attract a $0.55 premium.
        let mut lapse = body(d(2024, 7, 25), "100", 1);
        lapse.proceeds_per_right = Some(Decimal::ZERO);
        db_sell_rights(&pool, 10, &lapse).await.unwrap();
        let mut premium = body(d(2024, 8, 5), "150", 1);
        premium.proceeds_per_right = Some("0.55".parse().unwrap());
        db_sell_rights(&pool, 10, &premium).await.unwrap();

        let gains = crate::reports::realised_gains::db_realised_gains(&pool)
            .await
            .unwrap();
        assert_eq!(gains.len(), 2);
        // The lapse: nothing realised either way.
        assert_eq!(gains[0].proceeds, Decimal::ZERO);
        assert_eq!(gains[0].cost_base, Decimal::ZERO);
        assert_eq!(gains[0].capital_gain_loss, Decimal::ZERO);
        assert_eq!(gains[0].capital_loss, Decimal::ZERO);
        // The premium: the whole 150 × $0.55, discounted on the shares' clock.
        assert_eq!(gains[1].proceeds, "82.50".parse::<Decimal>().unwrap());
        assert_eq!(
            gains[1].discount_eligible_gain,
            "82.50".parse::<Decimal>().unwrap()
        );
        // ... and not a cent of it is income.
        let summary = crate::reports::tax_summary::db_tax_summary(&pool)
            .await
            .unwrap();
        assert!(summary.is_empty(), "{summary:?}");

        // Both disposals consumed the entitlement: nothing is left to exercise.
        let err = rights_exercise::db_exercise(
            &pool,
            10,
            &rights_exercise::ExerciseBody {
                date: d(2024, 8, 20),
                units: Decimal::ONE,
                rights_cost: None,
                fx_rate: None,
                holding_account_id: 1,
            },
        )
        .await;
        assert!(matches!(
            err,
            Err(rights_exercise::ExerciseError::ExceedsEntitlement)
        ));
    }

    /// The entitlement is one pool shared with exercises, in both directions:
    /// rights already sold block an exercise and vice versa.
    #[tokio::test]
    async fn entitlement_cap_is_shared_with_exercises() {
        let pool = test_pool().await;
        insert_listing(&pool, 1).await;
        insert_buy(&pool, 1, d(2024, 1, 16), "1000").await;
        insert_rights_issue(&pool, 10, d(2024, 7, 1)).await; // entitled to 250

        // Exercise 100, sell 150 — the entitlement is exactly consumed.
        rights_exercise::db_exercise(
            &pool,
            10,
            &rights_exercise::ExerciseBody {
                date: d(2024, 7, 10),
                units: "100".parse().unwrap(),
                rights_cost: None,
                fx_rate: None,
                holding_account_id: 1,
            },
        )
        .await
        .unwrap();
        db_sell_rights(&pool, 10, &body(d(2024, 7, 20), "150", 1))
            .await
            .unwrap();

        // No room left on either path.
        let err = db_sell_rights(&pool, 10, &body(d(2024, 7, 21), "1", 1)).await;
        assert!(matches!(err, Err(SellRightsError::ExceedsEntitlement)));
        let err = rights_exercise::db_exercise(
            &pool,
            10,
            &rights_exercise::ExerciseBody {
                date: d(2024, 7, 21),
                units: Decimal::ONE,
                rights_cost: None,
                fx_rate: None,
                holding_account_id: 1,
            },
        )
        .await;
        assert!(matches!(
            err,
            Err(rights_exercise::ExerciseError::ExceedsEntitlement)
        ));

        // Deleting the sale frees its share of the entitlement again.
        let sale_id: i64 = sqlx::query_scalar("SELECT id FROM rights_sales")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert!(db_delete(&pool, sale_id).await.unwrap());
        db_sell_rights(&pool, 10, &body(d(2024, 7, 22), "150", 1))
            .await
            .unwrap();
    }

    /// A parcel only anchors the rights its own record-date units earned —
    /// the cap can't be dodged by splitting one parcel across allocations or
    /// across successive sales.
    #[tokio::test]
    async fn per_parcel_anchoring_is_capped_at_the_parcels_entitlement() {
        let pool = test_pool().await;
        insert_listing(&pool, 1).await;
        // Two parcels: 800 units (entitled 200) and 200 units (entitled 50).
        insert_buy(&pool, 1, d(2023, 1, 17), "800").await;
        insert_buy(&pool, 2, d(2024, 6, 3), "200").await;
        insert_rights_issue(&pool, 10, d(2024, 7, 1)).await;

        // Anchoring 201 rights to the old parcel (to borrow its >12-month
        // acquisition date) is rejected — in one allocation or split in two.
        let err = db_sell_rights(&pool, 10, &body(d(2024, 7, 20), "201", 1)).await;
        assert!(matches!(
            err,
            Err(SellRightsError::ExceedsParcelEntitlement)
        ));
        let mut split = body(d(2024, 7, 20), "201", 1);
        split.allocations = vec![
            AllocationInput {
                purchase_trade_id: 1,
                units: "200".parse().unwrap(),
            },
            AllocationInput {
                purchase_trade_id: 1,
                units: Decimal::ONE,
            },
        ];
        let err = db_sell_rights(&pool, 10, &split).await;
        assert!(matches!(
            err,
            Err(SellRightsError::ExceedsParcelEntitlement)
        ));

        // 200 + 50 across the two parcels is the whole entitlement.
        let mut ok = body(d(2024, 7, 20), "250", 1);
        ok.allocations = vec![
            AllocationInput {
                purchase_trade_id: 1,
                units: "200".parse().unwrap(),
            },
            AllocationInput {
                purchase_trade_id: 2,
                units: "50".parse().unwrap(),
            },
        ];
        db_sell_rights(&pool, 10, &ok).await.unwrap();

        // A later sale finds the entitlement exhausted (the total cap fires
        // before the per-parcel one).
        let err = db_sell_rights(&pool, 10, &body(d(2024, 7, 21), "1", 2)).await;
        assert!(matches!(err, Err(SellRightsError::ExceedsEntitlement)));
    }

    /// A parcel's *stored* anchoring is counted once, however many
    /// allocations of the same request name that parcel. The in-request
    /// running total carries the prior figure forward
    /// (`check_parcel_anchoring`); adding it again per allocation would
    /// refuse a request that is inside the cap purely for having been split.
    #[tokio::test]
    async fn splitting_one_parcel_over_allocations_counts_prior_sales_once() {
        let pool = test_pool().await;
        insert_listing(&pool, 1).await;
        // 800 units (entitled 200) plus a second parcel keeping the *total*
        // entitlement (300) roomy, so only the per-parcel cap is in play.
        insert_buy(&pool, 1, d(2023, 1, 17), "800").await;
        insert_buy(&pool, 2, d(2024, 6, 3), "400").await;
        insert_rights_issue(&pool, 10, d(2024, 7, 1)).await;

        // 100 rights anchored to parcel 1, stored.
        db_sell_rights(&pool, 10, &body(d(2024, 7, 20), "100", 1))
            .await
            .unwrap();

        // 100 more, split 50 + 50 over the same parcel: 200 anchored in all,
        // exactly its entitlement.
        let mut split = body(d(2024, 7, 21), "100", 1);
        split.allocations = vec![
            AllocationInput {
                purchase_trade_id: 1,
                units: "50".parse().unwrap(),
            },
            AllocationInput {
                purchase_trade_id: 1,
                units: "50".parse().unwrap(),
            },
        ];
        db_sell_rights(&pool, 10, &split).await.unwrap();

        // The cap is still the cap: parcel 1 is exhausted, while the holding
        // has 100 of its 300 rights left (so this is the per-parcel refusal).
        let err = db_sell_rights(&pool, 10, &body(d(2024, 7, 22), "1", 1)).await;
        assert!(matches!(
            err,
            Err(SellRightsError::ExceedsParcelEntitlement)
        ));
    }

    /// Units sold out of a parcel before the record date earned no rights:
    /// the parcel's anchoring cap reflects its record-date remainder. A
    /// second untouched parcel keeps the *total* entitlement roomy, so the
    /// rejection is specifically the per-parcel cap.
    #[tokio::test]
    async fn parcel_cap_reflects_units_sold_before_the_record_date() {
        let pool = test_pool().await;
        insert_listing(&pool, 1).await;
        insert_buy(&pool, 1, d(2024, 1, 16), "1000").await;
        insert_buy(&pool, 3, d(2024, 2, 1), "1000").await;
        test_support::sell(2, 1)
            .date(d(2024, 5, 1))
            .qty("600".parse().unwrap())
            .price("2.00".parse().unwrap())
            .insert(&pool)
            .await;
        test_support::allocate(&pool, 1, 2, 1, "600".parse().unwrap()).await;
        insert_rights_issue(&pool, 10, d(2024, 7, 1)).await;

        // Parcel 1 has 400 of its 1000 left at the record date → it anchors
        // at most 100, even though the holding's total entitlement is 350.
        let err = db_sell_rights(&pool, 10, &body(d(2024, 7, 20), "101", 1)).await;
        assert!(matches!(
            err,
            Err(SellRightsError::ExceedsParcelEntitlement)
        ));
        db_sell_rights(&pool, 10, &body(d(2024, 7, 20), "100", 1))
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn invalid_sales_are_rejected_and_nothing_persisted() {
        let pool = test_pool().await;
        insert_listing(&pool, 1).await;
        insert_buy(&pool, 1, d(2024, 1, 16), "1000").await;
        insert_buy(&pool, 2, d(2024, 7, 1), "400").await; // ex-rights — earned nothing
        insert_rights_issue(&pool, 10, d(2024, 7, 1)).await;
        corporate_action::db_upsert(
            &pool,
            &CorporateAction {
                id: 11,
                listing_id: 1,
                date: d(2024, 7, 1),
                kind: ActionKind::ShareSplit {
                    split_new_units: Decimal::from(2),
                    split_old_units: Decimal::ONE,
                },
            },
        )
        .await
        .unwrap();

        let err = db_sell_rights(&pool, 999, &body(d(2024, 7, 20), "10", 1)).await;
        assert!(matches!(err, Err(SellRightsError::ActionNotFound)));
        let err = db_sell_rights(&pool, 11, &body(d(2024, 7, 20), "10", 1)).await;
        assert!(matches!(err, Err(SellRightsError::NotARightsIssue)));
        let err = db_sell_rights(&pool, 10, &body(d(2024, 6, 30), "10", 1)).await;
        assert!(matches!(err, Err(SellRightsError::BeforeRecordDate)));
        let err = db_sell_rights(&pool, 10, &body(d(2024, 7, 20), "0", 1)).await;
        assert!(matches!(err, Err(SellRightsError::NonPositiveUnits)));
        let err = db_sell_rights(&pool, 10, &body(d(2024, 7, 20), "-5", 1)).await;
        assert!(matches!(err, Err(SellRightsError::NonPositiveUnits)));

        let mut bad = body(d(2024, 7, 20), "10", 1);
        bad.proceeds_per_right = Some("-0.20".parse().unwrap());
        let err = db_sell_rights(&pool, 10, &bad).await;
        assert!(matches!(err, Err(SellRightsError::NegativeProceeds)));

        let mut bad = body(d(2024, 7, 20), "10", 1);
        bad.rights_cost = Some("-1".parse().unwrap());
        let err = db_sell_rights(&pool, 10, &bad).await;
        assert!(matches!(err, Err(SellRightsError::NegativeRightsCost)));

        // Allocations must be present, positive, and sum to the units.
        let mut bad = body(d(2024, 7, 20), "10", 1);
        bad.allocations.clear();
        let err = db_sell_rights(&pool, 10, &bad).await;
        assert!(matches!(
            err,
            Err(SellRightsError::AllocationsDontSum { .. })
        ));
        let mut bad = body(d(2024, 7, 20), "10", 1);
        bad.allocations[0].units = "9".parse().unwrap();
        let err = db_sell_rights(&pool, 10, &bad).await;
        assert!(matches!(
            err,
            Err(SellRightsError::AllocationsDontSum { .. })
        ));
        // A non-positive anchoring row is its own rejection: it could sum
        // correctly while being nonsense (SCENARIOS Y-b).
        let mut bad = body(d(2024, 7, 20), "10", 1);
        bad.allocations[0].units = "-10".parse().unwrap();
        let err = db_sell_rights(&pool, 10, &bad).await;
        assert!(matches!(err, Err(SellRightsError::AllocationNotPositive)));

        // Anchoring to a missing parcel, or to one dated on the record date
        // (ex-rights), is rejected.
        let err = db_sell_rights(&pool, 10, &body(d(2024, 7, 20), "10", 999)).await;
        assert!(matches!(err, Err(SellRightsError::NotAnOriginalParcel)));
        let err = db_sell_rights(&pool, 10, &body(d(2024, 7, 20), "10", 2)).await;
        assert!(matches!(err, Err(SellRightsError::NotAnOriginalParcel)));

        let n: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM rights_sales")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(n, 0, "no rejected sale may persist a row");
    }

    /// The action the sales were validated against is frozen while they
    /// reference it, exactly like exercise trades.
    #[tokio::test]
    async fn referenced_action_cannot_be_edited_or_deleted() {
        let pool = test_pool().await;
        insert_listing(&pool, 1).await;
        insert_buy(&pool, 1, d(2024, 1, 16), "1000").await;
        insert_rights_issue(&pool, 10, d(2024, 7, 1)).await;
        let sale = db_sell_rights(&pool, 10, &body(d(2024, 7, 20), "250", 1))
            .await
            .unwrap();

        let action = corporate_action::db_get(&pool, 10).await.unwrap().unwrap();
        let err = corporate_action::db_upsert(&pool, &action).await;
        assert!(matches!(
            err,
            Err(corporate_action::WriteError::ReferencedByTrade)
        ));
        let err = corporate_action::db_delete(&pool, 10).await;
        assert!(err.is_err(), "the rights_sales FK must block the delete");

        // Removing the sale unfreezes the action.
        assert!(db_delete(&pool, sale.id).await.unwrap());
        corporate_action::db_upsert(&pool, &action).await.unwrap();
        assert!(corporate_action::db_delete(&pool, 10).await.unwrap());
    }

    /// An anchoring parcel Buy is frozen against free-form trade edits and
    /// individual deletion while the sale references it — its date and
    /// quantity were what the anchoring caps validated.
    #[tokio::test]
    async fn anchoring_parcel_is_frozen_while_referenced() {
        let pool = test_pool().await;
        insert_listing(&pool, 1).await;
        insert_buy(&pool, 1, d(2024, 1, 16), "1000").await;
        insert_rights_issue(&pool, 10, d(2024, 7, 1)).await;
        let sale = db_sell_rights(&pool, 10, &body(d(2024, 7, 20), "250", 1))
            .await
            .unwrap();

        let parcel = trade::db_get(&pool, 1).await.unwrap().unwrap();
        let err = trade::db_upsert(&pool, &parcel).await;
        assert!(matches!(err, Err(trade::UpsertError::RightsAnchorParcel)));
        assert_eq!(
            trade::db_delete(&pool, 1).await.unwrap(),
            trade::DeleteOutcome::Referenced
        );

        // Deleting the sale unfreezes the parcel.
        assert!(db_delete(&pool, sale.id).await.unwrap());
        trade::db_upsert(&pool, &parcel).await.unwrap();
    }

    /// SCENARIOS AA-b. A retail premium under a **non-renounceable** offer is
    /// an unfranked dividend (TR 2012/1), not capital proceeds — and the
    /// entitlements it was paid on could not have been sold in the first
    /// place. Recorded here it would be a capital gain, discounted again off
    /// the original parcel's acquisition date. Refused instead, with the
    /// income path named.
    #[tokio::test]
    async fn proceeds_against_a_non_renounceable_offer_are_refused() {
        let pool = test_pool().await;
        insert_listing(&pool, 1).await;
        insert_buy(&pool, 1, d(2024, 1, 16), "1000").await;
        insert_non_renounceable_issue(&pool, 10, d(2024, 7, 1)).await;

        let err = db_sell_rights(&pool, 10, &body(d(2024, 7, 20), "250", 1)).await;
        assert!(matches!(
            err,
            Err(SellRightsError::ProceedsOnNonRenounceableOffer)
        ));
        let n: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM rights_sales")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(n, 0, "nothing is written");
    }

    /// The lapse stays recordable: a non-renounceable right lapses like any
    /// other, and nil proceeds against the nil cost base of a free right is a
    /// non-event on either treatment — while still consuming the entitlement,
    /// which is the bookkeeping the row is there for.
    #[tokio::test]
    async fn a_lapse_at_nil_against_a_non_renounceable_offer_is_still_recorded() {
        let pool = test_pool().await;
        insert_listing(&pool, 1).await;
        insert_buy(&pool, 1, d(2024, 1, 16), "1000").await;
        insert_non_renounceable_issue(&pool, 10, d(2024, 7, 1)).await;

        let mut lapse = body(d(2024, 7, 20), "250", 1);
        lapse.proceeds_per_right = Some(Decimal::ZERO);
        let sale = db_sell_rights(&pool, 10, &lapse).await.unwrap();
        assert_eq!(sale.units, Decimal::from(250));
        assert_eq!(sale.proceeds_per_right, Decimal::ZERO);
        assert_eq!(sale.rights_cost, Decimal::ZERO);

        // And it consumed the entitlement, which is what the row is for: the
        // 1000 shares earned 250 rights and all 250 have now lapsed, so one
        // more meets the shared cap.
        let mut more = body(d(2024, 7, 21), "1", 1);
        more.proceeds_per_right = Some(Decimal::ZERO);
        assert!(matches!(
            db_sell_rights(&pool, 10, &more).await,
            Err(SellRightsError::ExceedsEntitlement)
        ));
    }

    /// A cost paid to acquire the rights is impossible under the same terms —
    /// an entitlement that cannot be traded cannot have been bought — and left
    /// alone it would realise a capital loss on the lapse out of an amount
    /// that was never paid.
    #[tokio::test]
    async fn a_purchase_cost_against_a_non_renounceable_offer_is_refused() {
        let pool = test_pool().await;
        insert_listing(&pool, 1).await;
        insert_buy(&pool, 1, d(2024, 1, 16), "1000").await;
        insert_non_renounceable_issue(&pool, 10, d(2024, 7, 1)).await;

        let mut lapse = body(d(2024, 7, 20), "250", 1);
        lapse.proceeds_per_right = Some(Decimal::ZERO);
        lapse.rights_cost = Some("40".parse().unwrap());
        assert!(matches!(
            db_sell_rights(&pool, 10, &lapse).await,
            Err(SellRightsError::RightsCostOnNonRenounceableOffer)
        ));
    }

    /// The same premium under a **renounceable** offer is untouched: TR 2017/4
    /// capital proceeds, which is what this operation has always recorded.
    #[tokio::test]
    async fn a_renounceable_offer_still_takes_its_premium_as_proceeds() {
        let pool = test_pool().await;
        insert_listing(&pool, 1).await;
        insert_buy(&pool, 1, d(2024, 1, 16), "1000").await;
        insert_rights_issue(&pool, 10, d(2024, 7, 1)).await;

        let sale = db_sell_rights(&pool, 10, &body(d(2024, 7, 20), "250", 1))
            .await
            .unwrap();
        assert_eq!(sale.proceeds_per_right, "0.20".parse::<Decimal>().unwrap());
    }

    // API-level tests

    #[tokio::test]
    async fn api_sell_rights_returns_201_then_lists_gets_and_deletes() {
        let pool = test_pool().await;
        insert_listing(&pool, 1).await;
        insert_buy(&pool, 1, d(2024, 1, 16), "1000").await;
        insert_rights_issue(&pool, 10, d(2024, 7, 1)).await;
        let app = ApiClient::over(router().with_state(pool.clone()));

        let resp = app
            .post(
                "/corporate_actions/10/sell_rights",
                &serde_json::json!({
                    "date": "2024-07-20",
                    "units": "250",
                    "proceeds_per_right": "0.20",
                    "allocations": [
                        { "purchase_trade_id": 1, "units": "250" },
                    ],
                }),
            )
            .await;
        assert_eq!(resp.status, StatusCode::CREATED);
        let sale: serde_json::Value = resp.json();
        assert_eq!(sale["units"], "250");
        assert_eq!(sale["proceeds_per_right"], "0.20");
        assert_eq!(sale["allocations"][0]["purchase_trade_id"], 1);
        let id = sale["id"].as_i64().unwrap();

        let resp = app.get("/rights_sales").await;
        assert_eq!(resp.status, StatusCode::OK);
        let sales: serde_json::Value = resp.json();
        assert_eq!(sales.as_array().unwrap().len(), 1);

        let resp = app.get(format!("/rights_sales/{id}")).await;
        assert_eq!(resp.status, StatusCode::OK);

        let resp = app.delete(format!("/rights_sales/{id}")).await;
        assert_eq!(resp.status, StatusCode::NO_CONTENT);
        let resp = app.get(format!("/rights_sales/{id}")).await;
        assert_eq!(resp.status, StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn api_invalid_sales_return_404_or_422_with_a_reason() {
        let pool = test_pool().await;
        insert_listing(&pool, 1).await;
        insert_buy(&pool, 1, d(2024, 1, 16), "1000").await;
        insert_rights_issue(&pool, 10, d(2024, 7, 1)).await;
        let app = ApiClient::over(router().with_state(pool.clone()));

        let cases: Vec<(i64, serde_json::Value, StatusCode)> = vec![
            (
                999,
                serde_json::json!({
                    "date": "2024-07-20", "units": "10",
                    "allocations": [{ "purchase_trade_id": 1, "units": "10" }],
                }),
                StatusCode::NOT_FOUND,
            ),
            (
                10,
                serde_json::json!({
                    "date": "2024-07-20", "units": "251",
                    "allocations": [{ "purchase_trade_id": 1, "units": "251" }],
                }),
                StatusCode::UNPROCESSABLE_ENTITY,
            ),
            (
                10,
                serde_json::json!({
                    "date": "2024-07-20", "units": "10",
                    "allocations": [{ "purchase_trade_id": 1, "units": "9" }],
                }),
                StatusCode::UNPROCESSABLE_ENTITY,
            ),
            (
                10,
                serde_json::json!({
                    "date": "2024-06-30", "units": "10",
                    "allocations": [{ "purchase_trade_id": 1, "units": "10" }],
                }),
                StatusCode::UNPROCESSABLE_ENTITY,
            ),
        ];
        for (action_id, json, expected) in cases {
            let resp = app
                .post(format!("/corporate_actions/{action_id}/sell_rights"), &json)
                .await;
            assert_eq!(resp.status, expected);
            assert!(
                !resp.body.is_empty(),
                "a {expected} rejection must carry a reason body"
            );
        }

        let n: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM rights_sales")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(n, 0);
    }

    /// SCENARIOS AA-b at the HTTP surface: the refusal has to say *why* and
    /// where the premium belongs, since the whole failure it replaces was a
    /// silent acceptance. The lapse still answers `201`.
    #[tokio::test]
    async fn api_a_premium_on_a_non_renounceable_offer_is_refused_naming_the_income_path() {
        let pool = test_pool().await;
        insert_listing(&pool, 1).await;
        insert_buy(&pool, 1, d(2024, 1, 16), "1000").await;
        insert_non_renounceable_issue(&pool, 10, d(2024, 7, 1)).await;
        let app = ApiClient::over(router().with_state(pool.clone()));

        let resp = app
            .post(
                "/corporate_actions/10/sell_rights",
                &serde_json::json!({
                    "date": "2024-07-20",
                    "units": "250",
                    "proceeds_per_right": "0.20",
                    "allocations": [{ "purchase_trade_id": 1, "units": "250" }],
                }),
            )
            .await;
        assert_eq!(resp.status, StatusCode::UNPROCESSABLE_ENTITY);
        let body = resp.text();
        assert!(body.contains("non-renounceable"), "{body}");
        assert!(body.contains("TR 2012/1"), "{body}");
        assert!(body.contains("unfranked dividend income"), "{body}");

        // A paid right under the same offer is refused for the same reason.
        let resp = app
            .post(
                "/corporate_actions/10/sell_rights",
                &serde_json::json!({
                    "date": "2024-07-20",
                    "units": "250",
                    "proceeds_per_right": "0",
                    "rights_cost": "40",
                    "allocations": [{ "purchase_trade_id": 1, "units": "250" }],
                }),
            )
            .await;
        assert_eq!(resp.status, StatusCode::UNPROCESSABLE_ENTITY);
        assert!(resp.text().contains("nothing can have been paid"));

        // The lapse itself still lands.
        let resp = app
            .post(
                "/corporate_actions/10/sell_rights",
                &serde_json::json!({
                    "date": "2024-07-20",
                    "units": "250",
                    "proceeds_per_right": "0",
                    "allocations": [{ "purchase_trade_id": 1, "units": "250" }],
                }),
            )
            .await;
        assert_eq!(resp.status, StatusCode::CREATED);

        let n: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM rights_sales")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(n, 1, "only the lapse was written");
    }

    /// The anchoring-allocation refusals name their figures rather than
    /// restating the rule, so a wrong row is found by reading the message and
    /// not by adding the rows up (SCENARIOS Y-b). Same shape as the Sell
    /// side's `the allocations sum to …, not the … units sold`.
    #[tokio::test]
    async fn api_allocation_refusals_name_the_sum_and_what_it_should_be() {
        let pool = test_pool().await;
        insert_listing(&pool, 1).await;
        insert_buy(&pool, 1, d(2024, 1, 16), "1000").await;
        insert_rights_issue(&pool, 10, d(2024, 7, 1)).await;
        let app = ApiClient::over(router().with_state(pool.clone()));

        let short = serde_json::json!({
            "date": "2024-07-20", "units": "10",
            "allocations": [{ "purchase_trade_id": 1, "units": "9" }],
        });
        let resp = app.post("/corporate_actions/10/sell_rights", &short).await;
        assert_eq!(resp.status, StatusCode::UNPROCESSABLE_ENTITY);
        assert_eq!(
            resp.text(),
            "the allocations sum to 9, not the 10 rights sold"
        );

        // No allocations at all is the same refusal, with a nil sum.
        let none = serde_json::json!({
            "date": "2024-07-20", "units": "10", "allocations": [],
        });
        let resp = app.post("/corporate_actions/10/sell_rights", &none).await;
        assert_eq!(resp.status, StatusCode::UNPROCESSABLE_ENTITY);
        assert_eq!(
            resp.text(),
            "the allocations sum to 0, not the 10 rights sold"
        );

        // A non-positive row is its own message: it can sum correctly while
        // being nonsense, so naming the sum would say the wrong thing.
        let negative = serde_json::json!({
            "date": "2024-07-20", "units": "10",
            "allocations": [
                { "purchase_trade_id": 1, "units": "-5" },
                { "purchase_trade_id": 1, "units": "15" },
            ],
        });
        let resp = app
            .post("/corporate_actions/10/sell_rights", &negative)
            .await;
        assert_eq!(resp.status, StatusCode::UNPROCESSABLE_ENTITY);
        assert_eq!(
            resp.text(),
            "each anchoring parcel allocation must be for a positive number of rights"
        );
    }

    // --- fx-default family (code review 2026-08-25) ---

    /// A USD listing whose trades must be recorded in USD.
    async fn insert_usd_listing(pool: &SqlitePool, id: i64) {
        test_support::listing(id)
            .mic("XNYS")
            .ticker("RTU")
            .name("Rights Test Co US")
            .currency("USD")
            .insert(pool)
            .await;
    }

    /// A USD parcel carrying its own stated rate, so only the *sale* under
    /// test is missing one.
    async fn insert_usd_buy(pool: &SqlitePool, id: i64, date: NaiveDate, qty: &str) {
        test_support::buy(id, 1)
            .date(date)
            .settlement(date)
            .qty(qty.parse().unwrap())
            .price("2.00".parse().unwrap())
            .currency("USD")
            .fx_rate(dec("0.60"))
            .insert(pool)
            .await;
    }

    /// A 1-for-4 renounceable rights issue in USD, record date `date`.
    async fn insert_usd_rights_issue(pool: &SqlitePool, id: i64, date: NaiveDate) {
        corporate_action::db_upsert(
            pool,
            &CorporateAction {
                id,
                listing_id: 1,
                date,
                kind: ActionKind::RightsIssue {
                    rights_units: Decimal::ONE,
                    rights_held_units: Decimal::from(4),
                    exercise_price: "1.80".parse().unwrap(),
                    currency: "USD".to_string(),
                    renounceable: true,
                },
            },
        )
        .await
        .unwrap();
    }

    /// Fx-default family (code review 2026-08-25): a USD retail premium
    /// recorded with `fx_rate` omitted, dated in a month with no imported RBA
    /// rate, used to store parity silently — realised gains booked USD
    /// proceeds 1:1 as AUD, and (before it joined the fx_coverage scan)
    /// nothing anywhere flagged it. Now refused like the ESS vest, naming the
    /// currency, the month, and the remedies, with nothing persisted.
    #[tokio::test]
    async fn api_a_foreign_sale_with_fx_omitted_in_a_missing_month_is_refused_naming_the_month() {
        let pool = test_pool().await;
        insert_usd_listing(&pool, 1).await;
        insert_usd_buy(&pool, 1, d(2024, 1, 16), "1000").await;
        insert_usd_rights_issue(&pool, 10, d(2024, 7, 1)).await;

        let resp = ApiClient::over(router().with_state(pool.clone()))
            .post(
                "/corporate_actions/10/sell_rights",
                &serde_json::json!({
                    "date": "2024-07-20",
                    "units": "250",
                    "proceeds_per_right": "0.20",
                    "allocations": [ { "purchase_trade_id": 1, "units": "250" } ],
                }),
            )
            .await;
        let (status, body) = resp.status_and_body();
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
        assert!(body.contains("USD") && body.contains("2024-07"), "{body}");
        let n: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM rights_sales")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(n, 0, "nothing persisted");
    }

    /// With the month's rate imported an omitted `fx_rate` resolves to it —
    /// the stored row carries the real rate, never parity.
    #[tokio::test]
    async fn a_foreign_sale_resolves_the_imported_month_rate_when_fx_omitted() {
        let pool = test_pool().await;
        insert_usd_listing(&pool, 1).await;
        insert_usd_buy(&pool, 1, d(2024, 1, 16), "1000").await;
        insert_usd_rights_issue(&pool, 10, d(2024, 7, 1)).await;
        crate::entities::rba_fx_rate::db_import_rate(&pool, "USD", "2024-07", dec("0.65"))
            .await
            .unwrap();

        let sale = db_sell_rights(
            &pool,
            10,
            &SellRightsBody {
                date: d(2024, 7, 20),
                units: dec("250"),
                proceeds_per_right: Some(dec("0.20")),
                rights_cost: None,
                fx_rate: None,
                holding_account_id: 1,
                allocations: vec![AllocationInput {
                    purchase_trade_id: 1,
                    units: dec("250"),
                }],
            },
        )
        .await
        .unwrap();
        assert_eq!(sale.fx_rate, dec("0.65"));
    }

    /// A stated `fx_rate` is bound as given even in a missing-rate month —
    /// the caller said what the money converted at.
    #[tokio::test]
    async fn a_stated_fx_rate_is_stored_even_when_the_month_is_missing() {
        let pool = test_pool().await;
        insert_usd_listing(&pool, 1).await;
        insert_usd_buy(&pool, 1, d(2024, 1, 16), "1000").await;
        insert_usd_rights_issue(&pool, 10, d(2024, 7, 1)).await;

        let sale = db_sell_rights(
            &pool,
            10,
            &SellRightsBody {
                date: d(2024, 7, 20),
                units: dec("250"),
                proceeds_per_right: Some(dec("0.20")),
                rights_cost: None,
                fx_rate: Some(dec("0.62")),
                holding_account_id: 1,
                allocations: vec![AllocationInput {
                    purchase_trade_id: 1,
                    units: dec("250"),
                }],
            },
        )
        .await
        .unwrap();
        assert_eq!(sale.fx_rate, dec("0.62"));
    }

    /// A nil/nil lapse of a foreign issue converts nothing, so a missing
    /// month does not block it — a free right can always lapse.
    #[tokio::test]
    async fn a_nil_lapse_of_a_foreign_issue_needs_no_rate() {
        let pool = test_pool().await;
        insert_usd_listing(&pool, 1).await;
        insert_usd_buy(&pool, 1, d(2024, 1, 16), "1000").await;
        insert_usd_rights_issue(&pool, 10, d(2024, 7, 1)).await;

        let sale = db_sell_rights(
            &pool,
            10,
            &SellRightsBody {
                date: d(2024, 7, 20),
                units: dec("250"),
                proceeds_per_right: None,
                rights_cost: None,
                fx_rate: None,
                holding_account_id: 1,
                allocations: vec![AllocationInput {
                    purchase_trade_id: 1,
                    units: dec("250"),
                }],
            },
        )
        .await
        .unwrap();
        assert_eq!(sale.fx_rate, Decimal::ONE);
    }
}
