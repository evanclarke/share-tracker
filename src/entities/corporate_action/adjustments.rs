//! Cost-base/quantity adjustment events derived from corporate actions:
//! return-of-capital payments ([`RocEvent`]), quantity re-basing events
//! ([`SplitEvent`], covering both ShareSplit and BonusIssue) and — a
//! deliberately *different* set — price re-basing events
//! ([`PriceBasisEvent`]). Loaded by the reports and write-time checks, which
//! never re-derive this arithmetic inline.
//!
//! # The two event sets are not the same set
//!
//! **The price re-basing event set is a strict superset of the quantity
//! re-basing one.** Every quantity re-basing event also restates the price
//! series — the provider divides the per-unit price by the same ratio it
//! multiplies the unit count by — but the converse fails: a **Demerger**
//! restates the provider's whole pre-demerger series by a spin-off adjustment
//! factor while changing no unit count on the original listing (it issues
//! units of a *different* listing).
//!
//! So they are separate types, and nothing converts a [`PriceBasisEvent`]
//! back into a [`SplitEvent`]. [`split_ratio`] and its callers
//! ([`split_adjusted_quantity`], [`as_acquired_quantity`],
//! [`RocEvent::per_unit_for`]) mean *unit basis conversion*, and are read by
//! `domain::cost_base`, `domain::open_parcels`, the AMIT re-basing and the
//! write-time allocation-capacity checks. A demerger factor entering there
//! would silently restate quantities, cost bases and allocation capacity
//! across the whole application — which is why the price side has its own
//! type, its own ratio walk ([`price_basis_ratio`]) and its own loader
//! (`entities::closing_price::db_price_basis_events`).

use crate::infra::decimal::{mul_div, parse_dec};
use chrono::NaiveDate;
use rust_decimal::Decimal;
use sqlx::Row;
use std::collections::HashMap;

/// A return-of-capital payment, as consumed by the cost-base reports.
#[derive(Debug, Clone)]
pub struct RocEvent {
    /// The payment date.
    pub date: NaiveDate,
    pub amount_per_unit: Decimal,
    pub currency: String,
    /// The date entitlement to the payment was fixed, when recorded (always on
    /// or before [`Self::date`]). `None` = not recorded, and entitlement falls
    /// back to the payment date — see [`Self::per_unit_for`].
    pub record_date: Option<NaiveDate>,
}

/// What a rollover replacement parcel tells a return-of-capital payment about
/// the operation that created it — [`RocEvent::per_unit_for`] is the only
/// reader, and `domain::cost_base::ParcelRow::rollover` the only producer.
#[derive(Debug, Clone, Copy)]
pub struct RolloverOrigin {
    /// The operation date, which is also the replacement parcel's own trade
    /// date. `domain::rollover` folded every payment up to **and including**
    /// it into the cost base this parcel carries, so those must not reduce it
    /// a second time (SCENARIOS N-06).
    pub on: NaiveDate,
    /// The date the units joined **this listing's** register — what decides
    /// entitlement to a payment whose record date precedes the operation.
    ///
    /// For a **transfer** it is the units' own acquisition date: they never
    /// left the listing's register or the taxpayer's ownership, they only
    /// moved between the taxpayer's own accounts. A payment they were entitled
    /// to at its record date still reduces their cost base; it is simply the
    /// replacement parcel that holds them by the time it is paid.
    ///
    /// For a **scrip-for-scrip exchange**, and for a demerger, it is the
    /// operation date. The exchange's replacement units are of a listing the
    /// taxpayer was not on the register of beforehand, so a record date
    /// preceding the operation found them not there.
    pub registered_from: NaiveDate,
}

impl RocEvent {
    /// What this payment reduces the cost base by, per *as-acquired* unit of
    /// a parcel acquired on `acquired` and still held at `up_to` (`None` =
    /// held today) — or `None` when the payment does not reach those units at
    /// all.
    ///
    /// Two independent tests, one at each end of the window:
    ///
    /// - **Entitlement** — a parcel acquired after the payment was fixed never
    ///   received it. When the action records a [record date](Self::record_date)
    ///   that is what fixes it: units held *before* it earn the payment, so a
    ///   parcel acquired on or after it is ex-entitlement (the convention a
    ///   `RightsIssue`'s own date already uses). With none recorded the payment
    ///   date stands in, so a parcel acquired on or before it is treated as
    ///   entitled — the behaviour every action carried before record dates were
    ///   modelled, and an over-reduction for a parcel bought inside the
    ///   record-to-payment window (`docs/API.md`, Corporate actions).
    /// - **Still held** — a unit sold before the payment was not held for it
    ///   (CGT event G1 adjusts the cost base of shares owned *at the time of
    ///   the payment*, `docs/ato/cgt-non-assessable-payments.md`), so a payment
    ///   dated after `up_to` never applies.
    ///
    /// Each payment is per unit *at the payment date*: a split between
    /// acquisition and the payment multiplies the units receiving it, so the
    /// amount is scaled by the split ratio over `(acquired, payment date]` to
    /// express it per as-acquired unit (TD 2000/10).
    ///
    /// Fails loudly when the payment's currency differs from the parcel's —
    /// amounts in different currencies must never be netted against each
    /// other. This is the whole of a payment's relationship to a parcel:
    /// [`per_unit_reduction`] sums it, `domain::cost_base::adjustment_detail`
    /// itemises it, and the net-capital-gain report's G1 walk scales it to the
    /// whole parcel — none of them restates the window, the guard, or the
    /// re-basing.
    pub fn per_unit_for(
        &self,
        splits: &[SplitEvent],
        parcel_currency: &str,
        acquired: NaiveDate,
        rollover: Option<RolloverOrigin>,
        up_to: Option<NaiveDate>,
    ) -> Result<Option<Decimal>, sqlx::Error> {
        // Entitlement asks when the units joined *this listing's* register,
        // which for a rollover replacement parcel is not its own trade date:
        // a transfer between the taxpayer's accounts leaves them registered
        // throughout (see `RolloverOrigin::registered_from`). The split
        // re-basing below still runs from `acquired`, the parcel's own trade
        // date, because that is the basis its quantity is stated in.
        let registered_from = rollover.map_or(acquired, |r| r.registered_from);
        let entitled = match self.record_date {
            Some(record_date) => registered_from < record_date,
            None => registered_from <= self.date,
        };
        if !entitled || up_to.is_some_and(|d| self.date > d) {
            return Ok(None);
        }
        // A rollover replacement parcel's cost base was computed when the
        // operation ran, and `domain::rollover` folded into it every payment the
        // consumed parcels had received up to **and including** the operation
        // date. The entitlement test above admits a payment dated exactly on
        // that date — the replacement's own trade date — so without this the
        // same payment would come off the units twice: the carried cost base and
        // again here (SCENARIOS N-06, where a $1/unit return of capital paid on
        // the transfer date reported a $300 cost base for a $500 parcel that had
        // received $100). The operation date belongs to the operation, which is
        // also the only side that can account for it when the replacement is of
        // a *different* listing (a scrip-for-scrip exchange) or splits one
        // parcel's cost base across two (a demerger).
        if rollover.is_some_and(|r| self.date <= r.on) {
            return Ok(None);
        }
        if self.currency != parcel_currency {
            return Err(sqlx::Error::Decode(
                format!(
                    "return-of-capital currency {} differs from the parcel's currency {}",
                    self.currency, parcel_currency
                )
                .into(),
            ));
        }
        let (new, old) = split_ratio(splits, acquired, Some(self.date));
        Ok(Some(if new == old {
            self.amount_per_unit
        } else {
            mul_div(&[self.amount_per_unit, new], old)
        }))
    }
}

/// All ReturnOfCapital actions keyed by listing, each list sorted by payment
/// date (then id). Shared by the portfolio/unrealised/realised/open-parcels
/// reports to reduce affected parcels' cost bases, and by the net-capital-gain
/// report's G1 walk. Generic over the executor so reports can run it on their
/// read transaction.
pub async fn db_return_of_capital_events<'e, E>(
    executor: E,
) -> Result<HashMap<i64, Vec<RocEvent>>, sqlx::Error>
where
    E: sqlx::Executor<'e, Database = sqlx::Sqlite>,
{
    let rows = sqlx::query(
        "SELECT listing_id, date, amount_per_unit, currency, record_date FROM corporate_actions \
         WHERE action_type = 'ReturnOfCapital' ORDER BY listing_id, date, id",
    )
    .fetch_all(executor)
    .await?;

    let mut map: HashMap<i64, Vec<RocEvent>> = HashMap::new();
    for row in &rows {
        let listing_id: i64 = row.try_get("listing_id")?;
        map.entry(listing_id).or_default().push(RocEvent {
            date: row.try_get("date")?,
            amount_per_unit: parse_dec("amount_per_unit", row.try_get("amount_per_unit")?)?,
            currency: row.try_get("currency")?,
            record_date: row.try_get("record_date")?,
        });
    }
    Ok(map)
}

/// A return-of-capital payment recorded against a listing in one currency
/// while a parcel it reduces is held in another — the state
/// [`RocEvent::per_unit_for`] refuses to compute over, carried out of
/// [`db_payment_currency_conflict`] so a write path can name both sides in its
/// rejection.
#[derive(Debug, Clone)]
pub struct CurrencyConflict {
    pub payment_date: NaiveDate,
    pub payment_currency: String,
    pub parcel_currency: String,
}

/// The first (if any) return-of-capital payment on `listing_id` whose currency
/// differs from that of a Buy/DRP parcel it reaches — read on the caller's own
/// connection so a write can check the state it is about to commit.
///
/// This is the *write-time* form of [`RocEvent::per_unit_for`]'s currency
/// guard: the payment reduces each parcel's cost base in the parcel's own
/// currency, and amounts in different currencies are never netted, so the read
/// side fails loudly (`sqlx::Error::Decode` → `500`) on a pair it can't
/// compute. Refusing the pair at write time — from either side, the payment's
/// or the parcel's — keeps that state unrepresentable, the same shape as the
/// brokerage-currency invariant (`trade::checks::check_amounts`).
///
/// Which parcels a payment reaches is [`RocEvent::per_unit_for`]'s entitlement
/// test, expressed in SQL: units held *before* a recorded record date, or —
/// with none recorded — acquired on or before the payment date (the same test
/// the delete guard in `db.rs` applies). A parcel acquired ex-entitlement was
/// never reduced by the payment, so its currency is free to differ. The
/// "still held" half of the window is deliberately *not* applied: a parcel
/// sold before the payment can't be reduced either, but a future sale can't be
/// known at write time, so the check is the conservative one.
pub async fn db_payment_currency_conflict<'e, E>(
    executor: E,
    listing_id: i64,
) -> Result<Option<CurrencyConflict>, sqlx::Error>
where
    E: sqlx::Executor<'e, Database = sqlx::Sqlite>,
{
    let row = sqlx::query(
        "SELECT ca.date AS payment_date, ca.currency AS payment_currency, \
                t.currency AS parcel_currency \
         FROM corporate_actions ca JOIN trades t ON t.listing_id = ca.listing_id \
         WHERE ca.action_type = 'ReturnOfCapital' AND ca.listing_id = ? \
           AND t.trade_type IN ('Buy', 'DRP') \
           AND (CASE WHEN ca.record_date IS NOT NULL \
                     THEN t.date < ca.record_date ELSE t.date <= ca.date END) \
           AND upper(t.currency) <> upper(ca.currency) \
         ORDER BY ca.date, ca.id, t.date, t.id LIMIT 1",
    )
    .bind(listing_id)
    .fetch_optional(executor)
    .await?;
    row.map(|row| {
        Ok(CurrencyConflict {
            payment_date: row.try_get("payment_date")?,
            payment_currency: row.try_get("payment_currency")?,
            parcel_currency: row.try_get("parcel_currency")?,
        })
    })
    .transpose()
}

/// What a quantity re-basing event actually *was*, in the terms it was
/// announced in — carried beside [`SplitEvent`]'s derived rebase factor for
/// labelling only, never for arithmetic.
///
/// The factor normalises a bonus issue into its equivalent split (below),
/// which is right for the re-basing but discards the announced terms: a
/// 1-for-10 bonus issue re-bases by 11/10, a ratio that appears in no
/// company announcement. A `ShareSplit`'s own ratio survives the
/// normalisation intact, but its *name* does not — new < old is a
/// consolidation, and calling it a split says the opposite of what happened
/// to the unit count. So the announced terms travel with the event, and
/// [`RebaseTerms::label`] is what any human-readable surface names it by.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RebaseTerms {
    /// A `ShareSplit` whose terms increase the unit count (`new_units >=
    /// old_units`): every `old_units` units become `new_units`.
    ///
    /// The degenerate `new_units == old_units` (a 1-for-1 `ShareSplit`) lands
    /// here deliberately. It is representable — the write path only requires
    /// both terms positive — and re-bases nothing. "Consolidation" would be
    /// actively wrong for it (no unit count fell), so it keeps the action's
    /// own name: "1-for-1 split" reads as the no-op restatement it is.
    Split {
        new_units: Decimal,
        old_units: Decimal,
    },
    /// A `ShareSplit` whose terms *reduce* the unit count (`new_units <
    /// old_units`) — a consolidation (reverse split). Announced in exactly
    /// the same new-for-old terms as a split; only the name differs.
    Consolidation {
        new_units: Decimal,
        old_units: Decimal,
    },
    /// A `BonusIssue`: `bonus_units` new units for every `held_units` held —
    /// the terms as announced, *not* the equivalent split factor the event's
    /// `new_units`/`old_units` carry.
    BonusIssue {
        bonus_units: Decimal,
        held_units: Decimal,
    },
}

impl RebaseTerms {
    /// The event named by its own kind at its own announced ratio —
    /// `2-for-1 split`, `1-for-2 consolidation`, `1-for-10 bonus issue`.
    /// Reaches the archived CGT worksheet (the annual tax report's per-parcel
    /// `adjustments` rows, via `domain::cost_base::adjustment_detail`), where
    /// a reader reconciles it against the company's announcement — so both
    /// halves have to match what was announced.
    pub fn label(&self) -> String {
        match self {
            RebaseTerms::Split {
                new_units,
                old_units,
            } => format!("{} split", ratio(*new_units, *old_units)),
            RebaseTerms::Consolidation {
                new_units,
                old_units,
            } => format!("{} consolidation", ratio(*new_units, *old_units)),
            RebaseTerms::BonusIssue {
                bonus_units,
                held_units,
            } => format!("{} bonus issue", ratio(*bonus_units, *held_units)),
        }
    }
}

/// `new-for-old`, with trailing zeros stripped: terms entered as `2.00`/`1.00`
/// were announced as "2-for-1", and the stored scale is an artefact of how
/// they were typed.
fn ratio(new: Decimal, old: Decimal) -> String {
    format!("{}-for-{}", new.normalize(), old.normalize())
}

/// A quantity re-basing event, as consumed by the reports and write-time
/// checks: on `date`, every `old_units` existing units become `new_units`.
/// A ShareSplit (TD 2000/10) is stored as its ratio directly; a
/// non-assessable BonusIssue (`docs/ato/bonus-shares.md`) is its equivalent
/// split — every `bonus_held_units` units become `bonus_held_units +
/// bonus_units` units — because both preserve the parcel's total cost base
/// and acquisition date while scaling the unit count.
///
/// `terms` carries what the action was and how it was announced, which that
/// normalisation drops; it is for labelling only ([`RebaseTerms`]) and no
/// arithmetic reads it.
#[derive(Debug, Clone)]
pub struct SplitEvent {
    pub date: NaiveDate,
    pub new_units: Decimal,
    pub old_units: Decimal,
    pub terms: RebaseTerms,
}

impl SplitEvent {
    /// A `ShareSplit`'s event: its stated ratio is the rebase factor as-is,
    /// and the terms name it a split or a consolidation by which way the
    /// unit count moves.
    pub fn share_split(date: NaiveDate, new_units: Decimal, old_units: Decimal) -> Self {
        let terms = if new_units < old_units {
            RebaseTerms::Consolidation {
                new_units,
                old_units,
            }
        } else {
            RebaseTerms::Split {
                new_units,
                old_units,
            }
        };
        SplitEvent {
            date,
            new_units,
            old_units,
            terms,
        }
    }

    /// A `BonusIssue`'s event, normalised into its equivalent split for the
    /// arithmetic (`held + bonus` for every `held`) while `terms` keeps the
    /// announced bonus-for-held ratio.
    pub fn bonus_issue(date: NaiveDate, bonus_units: Decimal, held_units: Decimal) -> Self {
        SplitEvent {
            date,
            new_units: held_units + bonus_units,
            old_units: held_units,
            terms: RebaseTerms::BonusIssue {
                bonus_units,
                held_units,
            },
        }
    }
}

fn split_event_from_row(row: &sqlx::sqlite::SqliteRow) -> Result<SplitEvent, sqlx::Error> {
    let date = row.try_get("date")?;
    match row.try_get::<String, _>("action_type")?.as_str() {
        "BonusIssue" => Ok(SplitEvent::bonus_issue(
            date,
            parse_dec("bonus_units", row.try_get("bonus_units")?)?,
            parse_dec("bonus_held_units", row.try_get("bonus_held_units")?)?,
        )),
        _ => Ok(SplitEvent::share_split(
            date,
            parse_dec("split_new_units", row.try_get("split_new_units")?)?,
            parse_dec("split_old_units", row.try_get("split_old_units")?)?,
        )),
    }
}

/// All quantity re-basing actions (ShareSplit + BonusIssue, each expressed as
/// its equivalent split) keyed by listing, each list sorted by event date
/// (then id). The reports use these to re-base parcel quantities between unit
/// bases. Generic over the executor so reports can run it on their read
/// transaction.
pub async fn db_share_split_events<'e, E>(
    executor: E,
) -> Result<HashMap<i64, Vec<SplitEvent>>, sqlx::Error>
where
    E: sqlx::Executor<'e, Database = sqlx::Sqlite>,
{
    let rows = sqlx::query(
        "SELECT listing_id, action_type, date, split_new_units, split_old_units, \
                bonus_units, bonus_held_units FROM corporate_actions \
         WHERE action_type IN ('ShareSplit', 'BonusIssue') ORDER BY listing_id, date, id",
    )
    .fetch_all(executor)
    .await?;

    let mut map: HashMap<i64, Vec<SplitEvent>> = HashMap::new();
    for row in &rows {
        let listing_id: i64 = row.try_get("listing_id")?;
        map.entry(listing_id)
            .or_default()
            .push(split_event_from_row(row)?);
    }
    Ok(map)
}

/// The quantity re-basing actions (ShareSplit + BonusIssue) for one listing,
/// sorted by event date (then id). Generic over the executor so write-time
/// validators (sells/trades) can run it inside their transaction.
pub async fn db_splits_for_listing<'e, E>(
    executor: E,
    listing_id: i64,
) -> Result<Vec<SplitEvent>, sqlx::Error>
where
    E: sqlx::Executor<'e, Database = sqlx::Sqlite>,
{
    let rows = sqlx::query(
        "SELECT action_type, date, split_new_units, split_old_units, \
                bonus_units, bonus_held_units FROM corporate_actions \
         WHERE action_type IN ('ShareSplit', 'BonusIssue') AND listing_id = ? \
         ORDER BY date, id",
    )
    .bind(listing_id)
    .fetch_all(executor)
    .await?;
    rows.iter().map(split_event_from_row).collect()
}

/// Cumulative conversion ratio `(new, old)` between unit bases: the product of
/// `new_units`/`old_units` over the splits dated in `(from, up_to]` (`None` =
/// every split after `from`). A holding of `q` units in the basis of `from` is
/// `q × new / old` units in the basis of `up_to`. The interval is half-open
/// because a trade dated on a conversion date is already in post-split units,
/// while a sale or payment dated on it is post-split too.
pub fn split_ratio(
    splits: &[SplitEvent],
    from: NaiveDate,
    up_to: Option<NaiveDate>,
) -> (Decimal, Decimal) {
    let mut new = Decimal::ONE;
    let mut old = Decimal::ONE;
    for s in splits {
        if s.date <= from || up_to.is_some_and(|d| s.date > d) {
            continue;
        }
        new *= s.new_units;
        old *= s.old_units;
    }
    (new, old)
}

/// A parcel quantity as transacted at `acquired`, re-based to the unit basis
/// at `up_to` (`None` = after every recorded split). TD 2000/10: only the unit
/// count scales — the parcel's total cost base and original acquisition date
/// are untouched.
pub fn split_adjusted_quantity(
    qty: Decimal,
    splits: &[SplitEvent],
    acquired: NaiveDate,
    up_to: Option<NaiveDate>,
) -> Decimal {
    let (new, old) = split_ratio(splits, acquired, up_to);
    if new == old {
        qty
    } else {
        mul_div(&[qty, new], old)
    }
}

/// The inverse of [`split_adjusted_quantity`]: a quantity expressed in the
/// unit basis at `at` (e.g. a sale's allocated units) converted back to the
/// as-acquired units of a parcel bought at `acquired`.
pub fn as_acquired_quantity(
    qty: Decimal,
    splits: &[SplitEvent],
    acquired: NaiveDate,
    at: NaiveDate,
) -> Decimal {
    let (new, old) = split_ratio(splits, acquired, Some(at));
    if new == old {
        qty
    } else {
        mul_div(&[qty, old], new)
    }
}

/// [`as_acquired_quantity`] in checked form: the same conversion, refusing
/// rather than panicking where the result is past `Decimal`'s range.
///
/// Only a **consolidation** between `acquired` and `at` can reach that — it
/// multiplies the quantity up — and only for a quantity larger than the parcel
/// could ever have held, so this is the write paths' guard rather than the
/// reports' (`entities::transfer`, whose 1:1 move has no ratio of its own but
/// still re-bases the units asked for). `label` names the caller's own field
/// so the refusal quotes the request's vocabulary.
pub fn checked_as_acquired_quantity(
    qty: (&str, Decimal),
    splits: &[SplitEvent],
    acquired: NaiveDate,
    at: NaiveDate,
) -> Result<Decimal, crate::domain::cost_base::UnrepresentableQuantity> {
    let (new, old) = split_ratio(splits, acquired, Some(at));
    if new == old {
        return Ok(qty.1);
    }
    crate::domain::cost_base::checked_rebased_quantity(qty, ("old units", old), ("new units", new))
}

/// Total units sold out of a parcel acquired at `acquired`, re-based to its
/// as-acquired units. Each `(sale_date, quantity_allocated)` is expressed in
/// the unit basis of its own sale date — a post-split sale allocates post-split
/// units against the pre-split parcel.
pub fn sold_in_acquired_units(
    sales: impl IntoIterator<Item = (NaiveDate, Decimal)>,
    splits: &[SplitEvent],
    acquired: NaiveDate,
) -> Decimal {
    sales
        .into_iter()
        .map(|(date, qty)| as_acquired_quantity(qty, splits, acquired, date))
        .sum()
}

/// Cumulative return-of-capital cost-base reduction per *as-acquired* unit for
/// a unit acquired on `acquired` and still held at `up_to` (or held today when
/// `None`): the sum of `amount_per_unit` over the listing's payments the unit
/// was both entitled to and still held for. A unit sold before a payment was
/// not held for it, so the realised report bounds `up_to` at the sale date; the
/// open-holdings reports pass `None` (an unsold unit was held for every
/// payment since acquisition).
///
/// Which payments apply (entitlement at the record date, or the payment date
/// when none is recorded), what a rollover replacement parcel's `rolled_over_on`
/// date already accounts for, how a split re-bases each one, and the loud
/// failure on a currency mismatch are all [`RocEvent::per_unit_for`]'s — this is
/// only the sum over the listing's payments.
pub fn per_unit_reduction(
    events: &[RocEvent],
    splits: &[SplitEvent],
    trade_currency: &str,
    acquired: NaiveDate,
    rollover: Option<RolloverOrigin>,
    up_to: Option<NaiveDate>,
) -> Result<Decimal, sqlx::Error> {
    let mut total = Decimal::ZERO;
    for e in events {
        if let Some(per_unit) = e.per_unit_for(splits, trade_currency, acquired, rollover, up_to)? {
            total += per_unit;
        }
    }
    Ok(total)
}

// ---------------------------------------------------------------------------
// Price re-basing: the *superset* event set (see the module docs)
// ---------------------------------------------------------------------------

/// One event that restated the **price** series the provider serves for a
/// listing: the price-side counterpart of [`SplitEvent`], and deliberately a
/// distinct type from it (module docs — the price set is a strict superset of
/// the quantity set, and confusing the two would restate quantities).
///
/// Which corporate-action kinds belong here, and why:
///
/// - **`ShareSplit`** and **`BonusIssue`** — the unit count changes, so the
///   provider divides its whole earlier series by the same ratio. Converted
///   from [`SplitEvent`] by the `From` impl below, which is the only bridge
///   between the two sets and runs one way only.
/// - **`Demerger`** — the provider applies a spin-off price-adjustment factor
///   to the whole pre-demerger series exactly as it does for a split, but no
///   unit count on the original listing moves. The factor is set by the two
///   entities' market values at the spin-off, which the action's ATO
///   cost-base apportionment (`demerger_cost_base_pct`) does not give, so it
///   is derived from the close the operator *states* the security actually
///   traded at on the last pre-demerger trading day
///   ([`DemergerPriceStatement`]) against the provider's own adjusted figure
///   for that same day.
/// - **`ScripForScrip`** and **`WorthlessShares`** — *not* here. Both end the
///   original ticker: the provider stops serving it altogether (the
///   `listings.unpriced_from` case), so there is no continuing series to
///   restate.
/// - **`ReturnOfCapital`**, **`RightsIssue`** and **`BuyBack`** — *not* here.
///   A distribution is served through the provider's dividend-adjustment
///   channel, which `auto_adjust(false)` turns off (`entities::closing_price`
///   module docs), and neither a rights issue nor a buy-back is in the
///   provider's adjustment set at all.
#[derive(Debug, Clone)]
pub struct PriceBasisEvent {
    /// The date the provider restated its earlier series on — a split's
    /// conversion date, a bonus issue's issue date, or the demerger date.
    pub date: NaiveDate,
    /// Numerator and denominator of the factor that recovers a pre-event
    /// day's *own* price from a figure observed after the event:
    /// `price = observed × recover_new / recover_old`. Kept as an exact pair
    /// rather than a quotient so a walk over several events multiplies first
    /// and divides once, at the end ([`price_basis_ratio`]).
    pub recover_new: Decimal,
    pub recover_old: Decimal,
}

/// The one-way bridge from the quantity set into the price set: a re-basing
/// event that multiplies the unit count by `new/old` divides the per-unit
/// price by the same ratio (the parcel's total value is untouched — TD
/// 2000/10), so recovering the earlier day's price multiplies by `new/old`.
impl From<&SplitEvent> for PriceBasisEvent {
    fn from(split: &SplitEvent) -> Self {
        PriceBasisEvent {
            date: split.date,
            recover_new: split.new_units,
            recover_old: split.old_units,
        }
    }
}

/// The stated fact a `Demerger` may carry: what the security **actually
/// closed at** on the last pre-demerger trading day, which the demerger's
/// price factor is derived from.
///
/// Both sides of the factor are kept as facts — this date and close, and (at
/// re-base time) the provider's own stored figure for the same day — rather
/// than the quotient, so the close can be stated before any pre-demerger
/// history is backfilled and the factor re-derives itself if that history is
/// re-fetched. The provenance the entry carries
/// (`demerger_close_sourced_from` / `demerger_close_reason`) is not read by
/// the arithmetic; it is the audit record of where the operator got the
/// figure, as a hand-entered closing price carries.
#[derive(Debug, Clone)]
pub struct DemergerPriceStatement {
    /// The demerger date — the date the provider restated the series on.
    pub date: NaiveDate,
    /// The last pre-demerger trading day, always strictly before [`Self::date`]
    /// (CHECK-enforced, 0036).
    pub close_date: NaiveDate,
    /// What the security actually closed at on [`Self::close_date`], in the
    /// listing's quote currency.
    pub close_price: Decimal,
}

/// Every `Demerger` on one listing that carries a stated close, ascending by
/// demerger date (then id) — the demerger half of the price re-basing event
/// set. A demerger with no stated close yields nothing: its factor is
/// unknowable, so its pre-demerger prices stay as the provider served them
/// (surfaced by `GET /reports/health`'s `demergers_missing_close`).
///
/// Generic over the executor so the price re-base can run it inside the
/// corporate-action write's own transaction.
pub async fn db_demerger_price_statements<'e, E>(
    executor: E,
    listing_id: i64,
) -> Result<Vec<DemergerPriceStatement>, sqlx::Error>
where
    E: sqlx::Executor<'e, Database = sqlx::Sqlite>,
{
    let rows = sqlx::query(
        "SELECT date, demerger_close_date, demerger_close_price FROM corporate_actions \
         WHERE action_type = 'Demerger' AND listing_id = ? AND demerger_close_date IS NOT NULL \
         ORDER BY date, id",
    )
    .bind(listing_id)
    .fetch_all(executor)
    .await?;
    rows.iter()
        .map(|row| {
            Ok(DemergerPriceStatement {
                date: row.try_get("date")?,
                close_date: row.try_get("demerger_close_date")?,
                close_price: parse_dec(
                    "demerger_close_price",
                    row.try_get("demerger_close_price")?,
                )?,
            })
        })
        .collect()
}

/// Cumulative price-recovery ratio `(new, old)` between price bases: the
/// product of `recover_new`/`recover_old` over the events dated in
/// `(from, up_to]`. A figure observed at `up_to` for the trading day `from` is
/// `figure × new / old` in that day's own basis.
///
/// [`split_ratio`]'s half-open convention, for the same reason: an event dated
/// on the price date has already restated that day's close, and one dated on
/// the observation date has already restated what was observed. So a figure
/// observed *before* an event scales by nothing, which is what leaves a
/// contemporaneously collected series untouched when a later split or demerger
/// is recorded.
pub fn price_basis_ratio(
    events: &[PriceBasisEvent],
    from: NaiveDate,
    up_to: NaiveDate,
) -> (Decimal, Decimal) {
    let mut new = Decimal::ONE;
    let mut old = Decimal::ONE;
    for e in events {
        if e.date <= from || e.date > up_to {
            continue;
        }
        new *= e.recover_new;
        old *= e.recover_old;
    }
    (new, old)
}

/// A per-unit *price* observed in the basis in force at `observed`, restated
/// into the basis in force on `price_date` — the price dual of
/// [`split_adjusted_quantity`], over the [`PriceBasisEvent`] set rather than
/// the [`SplitEvent`] one (module docs).
///
/// A figure quoted after a 10-for-1 split is a tenth of the figure the same
/// security traded at before it, and multiplying by 10/1 recovers the earlier
/// day's own price; a figure quoted after a demerger is the pre-demerger close
/// times the provider's spin-off factor, and multiplying by that factor's
/// reciprocal recovers it. Every event's factor is accumulated as an exact
/// numerator/denominator pair, so however many events the window holds the
/// result is one multiplication and one division — no intermediate rounding.
pub fn contemporaneous_price(
    price: Decimal,
    events: &[PriceBasisEvent],
    price_date: NaiveDate,
    observed: NaiveDate,
) -> Decimal {
    let (new, old) = price_basis_ratio(events, price_date, observed);
    if new == old {
        price
    } else {
        mul_div(&[price, new], old)
    }
}
