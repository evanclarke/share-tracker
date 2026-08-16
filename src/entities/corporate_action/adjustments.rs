//! Cost-base/quantity adjustment events derived from corporate actions:
//! return-of-capital payments ([`RocEvent`]) and quantity re-basing events
//! ([`SplitEvent`], covering both ShareSplit and BonusIssue). Loaded by the
//! reports and write-time checks, which never re-derive this arithmetic
//! inline.

use crate::infra::decimal::parse_dec;
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
        up_to: Option<NaiveDate>,
    ) -> Result<Option<Decimal>, sqlx::Error> {
        let entitled = match self.record_date {
            Some(record_date) => acquired < record_date,
            None => acquired <= self.date,
        };
        if !entitled || up_to.is_some_and(|d| self.date > d) {
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
            self.amount_per_unit * new / old
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

/// A quantity re-basing event, as consumed by the reports and write-time
/// checks: on `date`, every `old_units` existing units become `new_units`.
/// A ShareSplit (TD 2000/10) is stored as its ratio directly; a
/// non-assessable BonusIssue (`docs/ato/bonus-shares.md`) is its equivalent
/// split — every `bonus_held_units` units become `bonus_held_units +
/// bonus_units` units — because both preserve the parcel's total cost base
/// and acquisition date while scaling the unit count.
#[derive(Debug, Clone)]
pub struct SplitEvent {
    pub date: NaiveDate,
    pub new_units: Decimal,
    pub old_units: Decimal,
}

fn split_event_from_row(row: &sqlx::sqlite::SqliteRow) -> Result<SplitEvent, sqlx::Error> {
    let date = row.try_get("date")?;
    match row.try_get::<String, _>("action_type")?.as_str() {
        "BonusIssue" => {
            let bonus = parse_dec("bonus_units", row.try_get("bonus_units")?)?;
            let held = parse_dec("bonus_held_units", row.try_get("bonus_held_units")?)?;
            Ok(SplitEvent {
                date,
                new_units: held + bonus,
                old_units: held,
            })
        }
        _ => Ok(SplitEvent {
            date,
            new_units: parse_dec("split_new_units", row.try_get("split_new_units")?)?,
            old_units: parse_dec("split_old_units", row.try_get("split_old_units")?)?,
        }),
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
    if new == old { qty } else { qty * new / old }
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
    if new == old { qty } else { qty * old / new }
}

/// Total units sold out of a parcel acquired at `acquired`, re-based to its
/// as-acquired units. Each `(sale_date, quantity_allocated)` is expressed in
/// the unit basis of its own sale date — a post-split sale allocates post-split
/// units against the pre-split parcel.
pub fn sold_in_acquired_units(
    sales: &[(NaiveDate, Decimal)],
    splits: &[SplitEvent],
    acquired: NaiveDate,
) -> Decimal {
    sales
        .iter()
        .map(|&(date, qty)| as_acquired_quantity(qty, splits, acquired, date))
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
/// when none is recorded), how a split re-bases each one, and the loud failure
/// on a currency mismatch are all [`RocEvent::per_unit_for`]'s — this is only
/// the sum over the listing's payments.
pub fn per_unit_reduction(
    events: &[RocEvent],
    splits: &[SplitEvent],
    trade_currency: &str,
    acquired: NaiveDate,
    up_to: Option<NaiveDate>,
) -> Result<Decimal, sqlx::Error> {
    let mut total = Decimal::ZERO;
    for e in events {
        if let Some(per_unit) = e.per_unit_for(splits, trade_currency, acquired, up_to)? {
            total += per_unit;
        }
    }
    Ok(total)
}
