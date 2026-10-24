//! Cost-base/quantity adjustment events derived from corporate actions:
//! return-of-capital payments ([`RocEvent`]), quantity re-basing events
//! ([`SplitEvent`], covering both `ShareSplit` and `BonusIssue`) and — a
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

use crate::domain::cost_base;
use crate::domain::cost_base::UnrepresentableQuantity;
use crate::infra::currency::CurrencyCode;
use crate::infra::decimal::{Money, mul_div, parse_dec};
use crate::infra::id::{ListingId, TradeId};
use chrono::NaiveDate;
use rust_decimal::Decimal;
use sqlx::Row;
use sqlx::sqlite::SqliteRow;
use std::borrow::Cow;
use std::cmp;
use std::collections::HashMap;
use std::iter::Sum;
#[cfg(test)]
use std::ops::Deref;
use std::ops::{Add, AddAssign, Sub, SubAssign};
use std::sync::Arc;

/// A return-of-capital payment, as consumed by the cost-base reports.
#[derive(Debug, Clone)]
pub struct RocEvent {
    /// The payment date.
    pub date: NaiveDate,
    pub amount_per_unit: Decimal,
    pub currency: CurrencyCode,
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
    /// For a **scrip-for-scrip exchange**, and for a **demerger's spun-off**
    /// parcel, it is the operation date. Those replacement units are of a
    /// listing the taxpayer was not on the register of beforehand, so a record
    /// date preceding the operation found them not there.
    ///
    /// A **demerger's head** parcel goes the other way, with the transfer: the
    /// units stayed on the head company's own register throughout — the
    /// demerger spun a subsidiary out from under them, it did not move them —
    /// so it too carries its source parcel's date. `domain::cost_base`'s
    /// `ParcelRow::registered_across_the_rollover` is where the three cases are
    /// decided, and treating the head parcel as the exchange's twin is exactly
    /// the entitlement it used to lose.
    pub registered_from: NaiveDate,
}

impl RocEvent {
    /// What this payment reduces the cost base by, per *as-acquired* unit of
    /// a parcel acquired on `acquired`, settled (so registered) on `settled`,
    /// and still held at `up_to` (`None` =
    /// held today) — or `None` when the payment does not reach those units at
    /// all.
    ///
    /// Two independent tests, one at each end of the window:
    ///
    /// - **Entitlement** — only units on the register when the payment was
    ///   fixed receive it. When the action records a
    ///   [record date](Self::record_date) that is what fixes it: a parcel
    ///   whose units joined the register — its `settled` date, the day a
    ///   market purchase is registered — on or before the record date earns
    ///   the payment, and one settling after it is ex-entitlement. That is the
    ///   exchange's ex-date convention without needing the ex-date: under T+2
    ///   a buy on the ex-date (one business day before the record date)
    ///   settles the day after it, and a same-day-settled parcel (a DRP, an
    ///   ESS vest) is registered on its own date. With no record date recorded
    ///   the payment date stands in, so a parcel settled on or before it is
    ///   treated as entitled — an over-reduction for a parcel settled inside
    ///   the record-to-payment window (`docs/API.md`, Corporate actions).
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
    /// `domain::cost_base`'s reduction walk applies it (summing the per-event
    /// figures itself, so a payment's amount can floor the cost base at nil
    /// exactly where it lands), the annual tax report itemises it, and the
    /// net-capital-gain report's G1 walk scales it to the whole parcel — none
    /// of them restates the window, the guard, or the re-basing.
    pub fn per_unit_for(
        &self,
        splits: &[SplitEvent],
        parcel_currency: &str,
        acquired: NaiveDate,
        settled: NaiveDate,
        rollover: Option<RolloverOrigin>,
        up_to: Option<NaiveDate>,
    ) -> Result<Option<Decimal>, sqlx::Error> {
        // Entitlement asks when the units joined *this listing's* register.
        // For an ordinary parcel that is its settlement date, not its trade
        // date: a market purchase is registered when it settles, which is what
        // the exchange's ex-date convention encodes (under T+2 a buy on the
        // ex-date, one business day before the record date, settles after it
        // and is ex-entitlement). For a rollover replacement parcel it is
        // neither: a transfer between the taxpayer's accounts leaves the
        // units registered throughout (see `RolloverOrigin::registered_from`).
        // The split re-basing below still runs from `acquired`, the parcel's
        // own trade date, because that is the basis its quantity is stated in.
        let registered_from = rollover.map_or(settled, |r| r.registered_from);
        let entitled = registered_from <= self.record_date.unwrap_or(self.date);
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
        if *self.currency != *parcel_currency {
            return Err(sqlx::Error::Decode(
                format!(
                    "return-of-capital currency {} differs from the parcel's currency {}",
                    self.currency, parcel_currency
                )
                .into(),
            ));
        }
        let SplitRatio { new, old } = split_ratio(splits, acquired, Some(self.date));
        Ok(Some(if new == old {
            self.amount_per_unit
        } else {
            mul_div(&[self.amount_per_unit, new], old)
        }))
    }
}

/// All `ReturnOfCapital` actions keyed by listing, each list sorted by payment
/// date (then id). Shared by the portfolio/unrealised/realised/open-parcels
/// reports to reduce affected parcels' cost bases, and by the net-capital-gain
/// report's G1 walk. Generic over the executor so reports can run it on their
/// read transaction.
pub async fn db_return_of_capital_events<'e, E>(
    executor: E,
) -> Result<HashMap<ListingId, Vec<RocEvent>>, sqlx::Error>
where
    E: sqlx::Executor<'e, Database = sqlx::Sqlite>,
{
    let rows = sqlx::query(
        "SELECT listing_id, date, amount_per_unit, currency, record_date FROM corporate_actions \
         WHERE action_type = 'ReturnOfCapital' ORDER BY listing_id, date, id",
    )
    .fetch_all(executor)
    .await?;

    let mut map: HashMap<ListingId, Vec<RocEvent>> = HashMap::new();
    for row in &rows {
        let listing_id: ListingId = row.try_get("listing_id")?;
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
    pub payment_currency: CurrencyCode,
    pub parcel_currency: CurrencyCode,
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
/// test, expressed in SQL: units settled (registered) on or before a recorded
/// record date, or — with none recorded — the payment date (the same test
/// the delete guard in `db.rs` applies). A parcel acquired ex-entitlement was
/// never reduced by the payment, so its currency is free to differ. The
/// "still held" half of the window is deliberately *not* applied: a parcel
/// sold before the payment can't be reduced either, but a future sale can't be
/// known at write time, so the check is the conservative one.
pub async fn db_payment_currency_conflict<'e, E>(
    executor: E,
    listing_id: ListingId,
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
           AND t.settlement_date <= COALESCE(ca.record_date, ca.date) \
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
/// A `ShareSplit` (TD 2000/10) is stored as its ratio directly; a
/// non-assessable `BonusIssue` (`docs/ato/bonus-shares.md`) is its equivalent
/// split — every `bonus_held_units` units become `bonus_held_units +
/// bonus_units` units — because both preserve the parcel's total cost base
/// and acquisition date while scaling the unit count.
///
/// `terms` carries what the action was and how it was announced, which that
/// normalisation drops; it is for labelling only ([`RebaseTerms`]) and no
/// arithmetic reads it.
///
/// A non-`Exact` action re-bases each parcel by the count **stored** for it
/// (`corporate_action_parcel_counts`, see [`StoredCount`]) instead of by the
/// ratio: `stored` carries those counts, and [`ListingSplits::for_parcel`]
/// resolves them into the parcel's own `new_units`/`old_units`. An event
/// still carrying them must not reach [`split_ratio`] — it asks *which*
/// parcel, and only the resolution can answer.
#[derive(Debug, Clone)]
pub struct SplitEvent {
    pub date: NaiveDate,
    pub new_units: Decimal,
    pub old_units: Decimal,
    pub terms: RebaseTerms,
    /// The per-parcel stored counts of a non-`Exact` action that rounded at
    /// least one holding (`None` for an `Exact` action, and once resolved for
    /// one parcel).
    pub stored: Option<Arc<HashMap<TradeId, StoredCount>>>,
}

/// One parcel's stored count across a non-`Exact` share split or bonus
/// issue: it held `before` units immediately before the action (in the basis
/// then in force) and holds `after` — on its holding's unit grid, as the
/// registry's rounding distributed it (`domain::entitlement_rounding`) —
/// immediately after. The parcel re-bases across the action by exactly
/// `after / before`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StoredCount {
    /// Units held immediately before the action.
    pub before: Decimal,
    /// Units held immediately after it.
    pub after: Decimal,
}

/// One listing's quantity re-basing events, sorted by date (then id), as the
/// loaders return them. Deliberately not a slice: a non-`Exact` action
/// re-bases each parcel by its own stored count, so the events a conversion
/// runs over depend on the parcel — [`Self::for_parcel`] is the way to them.
/// [`Self::announced`] is the escape for the readers that want the action as
/// announced rather than any one parcel's count (a price's re-basing, a
/// basis-invariant ratio, a representability bound).
#[derive(Debug, Clone, Default)]
pub struct ListingSplits(Vec<SplitEvent>);

impl ListingSplits {
    /// The events as they re-base parcel `parcel`: a non-`Exact` action's
    /// stored count for it in place of the ratio (its exact ratio where none
    /// is stored for it — a holding with no unit grid, or a parcel the action
    /// did not convert).
    pub fn for_parcel(&self, parcel: TradeId) -> Cow<'_, [SplitEvent]> {
        if self.0.iter().all(|e| e.stored.is_none()) {
            return Cow::Borrowed(&self.0);
        }
        Cow::Owned(
            self.0
                .iter()
                .map(|e| {
                    let mut resolved = SplitEvent {
                        stored: None,
                        ..e.clone()
                    };
                    if let Some(count) = e.stored.as_ref().and_then(|s| s.get(&parcel)) {
                        resolved.new_units = count.after;
                        resolved.old_units = count.before;
                    }
                    resolved
                })
                .collect(),
        )
    }

    /// The events at their announced ratios, ignoring any stored counts — for
    /// a reader that is not converting one parcel's units: a price series'
    /// re-basing (the provider divides by the announced ratio), a ratio that is
    /// basis-invariant, or a representability bound.
    pub fn announced(&self) -> Cow<'_, [SplitEvent]> {
        if self.0.iter().all(|e| e.stored.is_none()) {
            return Cow::Borrowed(&self.0);
        }
        Cow::Owned(
            self.0
                .iter()
                .map(|e| SplitEvent {
                    stored: None,
                    ..e.clone()
                })
                .collect(),
        )
    }
}

/// Test fixtures read a listing's events as the slice they are (the
/// non-test build has no such view, so production code cannot bypass
/// [`ListingSplits::for_parcel`]).
#[cfg(test)]
impl Deref for ListingSplits {
    type Target = [SplitEvent];
    fn deref(&self) -> &[SplitEvent] {
        &self.0
    }
}

/// The events of `listing` in `splits` as they re-base parcel `parcel` (no
/// events where the listing has none) — [`ListingSplits::for_parcel`] over a
/// whole-portfolio map.
pub fn parcel_splits(
    splits: &HashMap<ListingId, ListingSplits>,
    listing: ListingId,
    parcel: TradeId,
) -> Cow<'_, [SplitEvent]> {
    splits
        .get(&listing)
        .map_or(Cow::Borrowed(&[][..]), |s| s.for_parcel(parcel))
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
            stored: None,
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
            stored: None,
        }
    }
}

fn split_event_from_row(row: &SqliteRow) -> Result<SplitEvent, sqlx::Error> {
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

/// The stored per-parcel counts of every share split / bonus issue (of one
/// listing, or all), keyed by action id.
async fn db_stored_counts(
    conn: &mut sqlx::SqliteConnection,
    listing_id: Option<ListingId>,
) -> Result<HashMap<i64, HashMap<TradeId, StoredCount>>, sqlx::Error> {
    let rows = sqlx::query(
        "SELECT c.corporate_action_id, c.trade_id, c.units_before, c.units_after \
         FROM corporate_action_parcel_counts c \
         JOIN corporate_actions a ON a.id = c.corporate_action_id \
         WHERE (a.action_type = 'ShareSplit' OR a.action_type = 'BonusIssue') \
           AND (?1 IS NULL OR a.listing_id = ?1)",
    )
    .bind(listing_id)
    .fetch_all(&mut *conn)
    .await?;
    let mut map: HashMap<i64, HashMap<TradeId, StoredCount>> = HashMap::new();
    for row in &rows {
        map.entry(row.try_get("corporate_action_id")?)
            .or_default()
            .insert(
                row.try_get("trade_id")?,
                StoredCount {
                    before: parse_dec("units_before", row.try_get("units_before")?)?,
                    after: parse_dec("units_after", row.try_get("units_after")?)?,
                },
            );
    }
    Ok(map)
}

/// The re-basing events of the rows `rows` (each carrying `id`), with their
/// stored counts attached.
fn events_with_counts(
    rows: &[SqliteRow],
    mut counts: HashMap<i64, HashMap<TradeId, StoredCount>>,
) -> Result<Vec<(ListingId, SplitEvent)>, sqlx::Error> {
    rows.iter()
        .map(|row| {
            let mut event = split_event_from_row(row)?;
            let id: i64 = row.try_get("id")?;
            event.stored = counts.remove(&id).map(Arc::new);
            Ok((row.try_get("listing_id")?, event))
        })
        .collect()
}

/// All quantity re-basing actions (`ShareSplit` + `BonusIssue`, each expressed as
/// its equivalent split) keyed by listing, each list sorted by event date
/// (then id), with each non-`Exact` action's stored per-parcel counts. The
/// reports use these to re-base parcel quantities between unit bases. Read on
/// the caller's connection so reports can run it on their read transaction.
pub async fn db_share_split_events(
    conn: &mut sqlx::SqliteConnection,
) -> Result<HashMap<ListingId, ListingSplits>, sqlx::Error> {
    let rows = sqlx::query(
        "SELECT id, listing_id, action_type, date, split_new_units, split_old_units, \
                bonus_units, bonus_held_units FROM corporate_actions \
         WHERE action_type IN ('ShareSplit', 'BonusIssue') ORDER BY listing_id, date, id",
    )
    .fetch_all(&mut *conn)
    .await?;
    let counts = db_stored_counts(&mut *conn, None).await?;

    let mut map: HashMap<ListingId, Vec<SplitEvent>> = HashMap::new();
    for (listing_id, event) in events_with_counts(&rows, counts)? {
        map.entry(listing_id).or_default().push(event);
    }
    Ok(map
        .into_iter()
        .map(|(listing, events)| (listing, ListingSplits(events)))
        .collect())
}

/// The quantity re-basing actions (`ShareSplit` + `BonusIssue`) for one listing,
/// sorted by event date (then id), with their stored per-parcel counts. Read
/// on the caller's connection so write-time validators (sells/trades) can run
/// it inside their transaction.
pub async fn db_splits_for_listing(
    conn: &mut sqlx::SqliteConnection,
    listing_id: ListingId,
) -> Result<ListingSplits, sqlx::Error> {
    let rows = sqlx::query(
        "SELECT id, listing_id, action_type, date, split_new_units, split_old_units, \
                bonus_units, bonus_held_units FROM corporate_actions \
         WHERE action_type IN ('ShareSplit', 'BonusIssue') AND listing_id = ? \
         ORDER BY date, id",
    )
    .bind(listing_id)
    .fetch_all(&mut *conn)
    .await?;
    let counts = db_stored_counts(&mut *conn, Some(listing_id)).await?;
    Ok(ListingSplits(
        events_with_counts(&rows, counts)?
            .into_iter()
            .map(|(_, event)| event)
            .collect(),
    ))
}

/// A conversion ratio between unit bases: `new` units for every `old`. Named
/// because both terms are `Decimal`s, and a swapped pair still compiles — and
/// re-bases every quantity the wrong way.
#[must_use = "a dropped money or quantity figure is a silent money error"]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SplitRatio {
    /// The units held in the later basis for every `old`.
    pub new: Decimal,
    /// The units held in the earlier basis.
    pub old: Decimal,
}

impl SplitRatio {
    /// The ratio that converts no units: 1 for 1.
    pub const IDENTITY: Self = Self {
        new: Decimal::ONE,
        old: Decimal::ONE,
    };

    /// The same conversion run backwards, from the later basis to the
    /// earlier.
    fn inverse(self) -> Self {
        Self {
            new: self.old,
            old: self.new,
        }
    }
}

/// Cumulative conversion ratio between unit bases: the product of
/// `new_units`/`old_units` over the splits dated in `(from, up_to]` (`None` =
/// every split after `from`). A holding of `q` units in the basis of `from` is
/// `q × new / old` units in the basis of `up_to`. The interval is half-open
/// because a trade dated on a conversion date is already in post-split units,
/// while a sale or payment dated on it is post-split too.
pub fn split_ratio(splits: &[SplitEvent], from: NaiveDate, up_to: Option<NaiveDate>) -> SplitRatio {
    let mut new = Decimal::ONE;
    let mut old = Decimal::ONE;
    for s in splits {
        if s.date <= from || up_to.is_some_and(|d| s.date > d) {
            continue;
        }
        debug_assert!(
            s.stored.is_none(),
            "a stored-count event reached split_ratio unresolved (ListingSplits::for_parcel)"
        );
        new *= s.new_units;
        old *= s.old_units;
    }
    SplitRatio { new, old }
}

/// A quantity in one parcel's own **as-acquired** unit basis — the basis
/// `trades.quantity` is recorded and capped in, and the one the cost-base
/// pipeline (`domain::cost_base::adjusted_cost_base`) costs units in. A split
/// scales the unit count but never the cost base (TD 2000/10), which is why
/// the cost-base arithmetic stays on this basis however many splits follow.
///
/// It carries no date: the parcel it belongs to knows its acquisition date.
/// A quantity in any *other* basis is a [`UnitsAt`], and the only ways
/// between the two are [`split_adjusted_quantity`], [`as_acquired_quantity`]
/// (with its checked twin) and [`sold_in_acquired_units`] — so an allocation
/// recorded in sale-date units cannot be subtracted from a parcel's quantity,
/// or costed, without being re-based first.
///
/// The count is held as an **exact fraction** `num / den`. Re-basing a
/// sale-date quantity back across a split divides by the split's ratio, and
/// 200 post-split units of a 3-for-1 split are 66.666… as-acquired units,
/// which no `Decimal` holds: rounded there, a remainder re-based forward
/// came back `399.99…9` instead of `400`, and three such sales summed past
/// (or short of) the parcel they emptied. So the conversion keeps the ratio's
/// denominator instead of dividing by it, sums and differences cross-multiply,
/// and the one division happens where the figure leaves this type —
/// re-based forward ([`split_adjusted_quantity`], which folds the ratio's own
/// divisor into the same division), pro-rated into a cost base
/// (`cost_base::prorated_initial_cost`), or read out by [`Self::get`]. Only
/// where a cross-multiplication would itself overflow does an operation fall
/// back to the rounded figures, which is no worse than dividing up front.
#[must_use = "a dropped money or quantity figure is a silent money error"]
#[derive(Debug, Clone, Copy)]
pub struct AcquiredUnits {
    num: Decimal,
    /// Always positive, and 1 whenever `num` is a whole multiple of it.
    den: Decimal,
}

impl AcquiredUnits {
    /// No units (the exact fraction `0 / 1`): what an emptied parcel's
    /// remainder, or an empty covered count, is compared against.
    pub const ZERO: Self = Self {
        num: Decimal::ZERO,
        den: Decimal::ONE,
    };

    /// A quantity the caller knows is in its parcel's as-acquired basis: a
    /// parcel's own recorded quantity, or a figure stated against it (an
    /// AMIT adjustment row's covered units).
    pub fn new(quantity: Decimal) -> Self {
        Self {
            num: quantity,
            den: Decimal::ONE,
        }
    }

    /// `num / den` units, kept exact. `den` is positive (a split ratio's
    /// term, or a product of them).
    fn fraction(num: Decimal, den: Decimal) -> Self {
        if den == Decimal::ONE {
            return Self::new(num);
        }
        if (num % den).is_zero() {
            // A whole multiple of the denominator divides exactly.
            return Self::new(num / den);
        }
        Self { num, den }
    }

    /// The unit count, for output and for arithmetic that has to leave this
    /// type — rounded to `Decimal`'s precision where the exact figure is not
    /// a `Decimal` (the only rounding the count ever takes).
    pub fn get(self) -> Decimal {
        if self.den == Decimal::ONE {
            self.num
        } else {
            self.num / self.den
        }
    }

    /// The count as an exact fraction, for a caller that folds the
    /// denominator into a division of its own rather than rounding twice.
    pub(crate) fn parts(self) -> Fraction {
        Fraction {
            num: self.num,
            den: self.den,
        }
    }

    /// The two counts' numerators over one common denominator, or `None`
    /// where that cannot be represented (the caller then falls back to the
    /// rounded figures).
    fn over_common_denominator(self, rhs: Self) -> Option<(Decimal, Decimal, Decimal)> {
        if self.den == rhs.den {
            return Some((self.num, rhs.num, self.den));
        }
        let common = lcm(self.den, rhs.den)?;
        Some((
            self.num.checked_mul(common / self.den)?,
            rhs.num.checked_mul(common / rhs.den)?,
            common,
        ))
    }

    fn combine(self, rhs: Self, op: fn(Decimal, Decimal) -> Option<Decimal>) -> Self {
        self.over_common_denominator(rhs)
            .and_then(|(a, b, den)| Some(Self::fraction(op(a, b)?, den)))
            .unwrap_or_else(|| Self::new(op(self.get(), rhs.get()).expect("unit count overflow")))
    }
}

/// An [`AcquiredUnits`] count as the exact fraction `num / den` it holds —
/// see [`AcquiredUnits::parts`].
#[must_use = "a dropped money or quantity figure is a silent money error"]
#[derive(Debug, Clone, Copy)]
pub(crate) struct Fraction {
    pub num: Decimal,
    /// Always positive.
    pub den: Decimal,
}

/// The least common multiple of two positive ratio denominators: their
/// product over their greatest common divisor where both are whole numbers
/// (every split ratio recorded so far), else their plain product.
fn lcm(a: Decimal, b: Decimal) -> Option<Decimal> {
    if a.fract().is_zero() && b.fract().is_zero() {
        let (mut x, mut y) = (a, b);
        while !y.is_zero() {
            (x, y) = (y, x % y);
        }
        (a / x).checked_mul(b)
    } else {
        a.checked_mul(b)
    }
}

impl Default for AcquiredUnits {
    fn default() -> Self {
        Self::ZERO
    }
}

impl PartialEq for AcquiredUnits {
    fn eq(&self, other: &Self) -> bool {
        self.cmp(other) == cmp::Ordering::Equal
    }
}

impl Eq for AcquiredUnits {}

impl PartialOrd for AcquiredUnits {
    fn partial_cmp(&self, other: &Self) -> Option<cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for AcquiredUnits {
    fn cmp(&self, other: &Self) -> cmp::Ordering {
        match self.over_common_denominator(*other) {
            Some((a, b, _)) => a.cmp(&b),
            None => self.get().cmp(&other.get()),
        }
    }
}

impl From<Money> for AcquiredUnits {
    fn from(m: Money) -> Self {
        Self::new(m.0)
    }
}

impl Add for AcquiredUnits {
    type Output = Self;
    fn add(self, rhs: Self) -> Self {
        self.combine(rhs, Decimal::checked_add)
    }
}

impl Sub for AcquiredUnits {
    type Output = Self;
    fn sub(self, rhs: Self) -> Self {
        self.combine(rhs, Decimal::checked_sub)
    }
}

impl AddAssign for AcquiredUnits {
    fn add_assign(&mut self, rhs: Self) {
        *self = *self + rhs;
    }
}

impl SubAssign for AcquiredUnits {
    fn sub_assign(&mut self, rhs: Self) {
        *self = *self - rhs;
    }
}

impl Sum for AcquiredUnits {
    fn sum<I: Iterator<Item = Self>>(iter: I) -> Self {
        iter.fold(Self::ZERO, |acc, u| acc + u)
    }
}

#[cfg(test)]
impl PartialEq<Decimal> for AcquiredUnits {
    fn eq(&self, other: &Decimal) -> bool {
        *self == Self::new(*other)
    }
}

/// A quantity in the unit basis **in force at a date** — after every split,
/// consolidation and bonus issue dated on or before it (`None` = after every
/// recorded one). A sale's allocation is in its own sale date's basis, a
/// position as at a date in that date's, an entitlement in its record date's.
///
/// Unlike [`AcquiredUnits`] it carries the date it is stated at, so a
/// conversion is handed only the date it converts *to* and cannot be given
/// the wrong *from*. The quantity counterpart of `cost_base::Held`.
#[must_use = "a dropped money or quantity figure is a silent money error"]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UnitsAt {
    quantity: Decimal,
    basis: Option<NaiveDate>,
}

impl UnitsAt {
    /// `quantity` units in the basis in force at `basis`.
    pub fn new(quantity: Decimal, basis: NaiveDate) -> Self {
        Self {
            quantity,
            basis: Some(basis),
        }
    }

    /// The unit count, read in this value's own basis — for output, and for
    /// comparing or summing with other quantities of that same basis.
    pub fn quantity(self) -> Decimal {
        self.quantity
    }

    /// The date whose unit basis [`Self::quantity`] is in (`None` = after
    /// every recorded split).
    pub fn basis(self) -> Option<NaiveDate> {
        self.basis
    }

    /// The same units re-based to the basis in force at `to`, in whichever
    /// direction the two dates sit.
    pub fn rebased(self, splits: &[SplitEvent], to: Option<NaiveDate>) -> Self {
        Self {
            quantity: rebase(self.quantity, splits, self.basis, to),
            basis: to,
        }
    }
}

#[cfg(test)]
impl PartialEq<Decimal> for UnitsAt {
    fn eq(&self, other: &Decimal) -> bool {
        self.quantity == *other
    }
}

/// `qty` units in the basis at `from` re-based to the basis at `to` (`None`
/// = after every recorded split, i.e. later than any date). Forward when
/// `to` is the later date, by the inverse ratio when it is the earlier: a
/// rollover replacement parcel can postdate the AMMA statement year end its
/// units are costed against (SCENARIOS N-06), and a one-directional window
/// would multiply by 1 there and apply a per-unit figure to post-split units.
#[must_use = "a dropped money or quantity figure is a silent money error"]
fn rebase(
    qty: Decimal,
    splits: &[SplitEvent],
    from: Option<NaiveDate>,
    to: Option<NaiveDate>,
) -> Decimal {
    let SplitRatio { new, old } = rebase_ratio(splits, from, to);
    if new == old {
        qty
    } else {
        mul_div(&[qty, new], old)
    }
}

/// The ratio [`rebase`] multiplies by, for a caller that keeps the division
/// to itself.
fn rebase_ratio(
    splits: &[SplitEvent],
    from: Option<NaiveDate>,
    to: Option<NaiveDate>,
) -> SplitRatio {
    match (from, to) {
        (None, None) => SplitRatio::IDENTITY,
        (Some(f), None) => split_ratio(splits, f, None),
        (Some(f), Some(t)) if f <= t => split_ratio(splits, f, Some(t)),
        (f, Some(t)) => split_ratio(splits, t, f).inverse(),
    }
}

/// A parcel quantity as transacted at `acquired`, re-based to the unit basis
/// at `up_to` (`None` = after every recorded split). TD 2000/10: only the unit
/// count scales — the parcel's total cost base and original acquisition date
/// are untouched.
pub fn split_adjusted_quantity(
    qty: AcquiredUnits,
    splits: &[SplitEvent],
    acquired: NaiveDate,
    up_to: Option<NaiveDate>,
) -> UnitsAt {
    let SplitRatio { new, old } = rebase_ratio(splits, Some(acquired), up_to);
    let Fraction { num, den } = qty.parts();
    // One division for the fraction's denominator and the ratio's together:
    // a remainder kept exact is not rounded on its way out either.
    let quantity = if new == old {
        qty.get()
    } else {
        match den.checked_mul(old) {
            Some(divisor) => mul_div(&[num, new], divisor),
            None => mul_div(&[qty.get(), new], old),
        }
    };
    UnitsAt {
        quantity,
        basis: up_to,
    }
}

/// The smallest mantissa of a `Decimal` carrying 28 significant digits — the
/// precision a rounded quotient comes out at.
const FULL_PRECISION: u128 = 10u128.pow(27);

/// The inverse of [`split_adjusted_quantity`]: a quantity expressed in the
/// unit basis of its own date (e.g. a sale's allocated units) converted back
/// to the as-acquired units of a parcel bought at `acquired`.
pub fn as_acquired_quantity(
    qty: UnitsAt,
    splits: &[SplitEvent],
    acquired: NaiveDate,
) -> AcquiredUnits {
    let SplitRatio { new, old } = rebase_ratio(splits, qty.basis, Some(acquired));
    if new == old {
        return AcquiredUnits::new(qty.quantity);
    }
    // Kept as the fraction `quantity × new / old` rather than divided out
    // (see `AcquiredUnits`); only an unrepresentable product rounds.
    let exact = match qty.quantity.checked_mul(new) {
        Some(num) => AcquiredUnits::fraction(num, old),
        None => return AcquiredUnits::new(mul_div(&[qty.quantity, new], old)),
    };
    // Only a figure at `Decimal`'s full precision can be a rounding; a
    // shorter one (an integer sale, a broker's 2-dp quantity) states its units
    // exactly and keeps the exact fraction they re-base to. That holds even
    // where reading it back divides by nothing: across a consolidation the
    // later figure is the rounded one (284 units, 1-for-3, are 94.666…67), and
    // multiplied back it lies past the parcel (284.000…001).
    if qty.quantity.mantissa().unsigned_abs() < FULL_PRECISION {
        return exact;
    }
    // A quantity stated in a later basis is itself a rounded figure where
    // the units it re-bases from are not a terminating decimal there: after a
    // 1-for-3 bonus issue 101 units are 134.666…, which the open-parcels
    // schedule shows (and a Sell can state) only as `…67` — and that, read
    // back exactly, is 101.000…0025 units, so the Sell emptying the parcel
    // was refused as exceeding it, and the `…66` that is accepted left dust
    // open for ever. So the figure reads back as the *shortest* as-acquired
    // count that re-bases to exactly it — the count it was rounded from — and
    // keeps the exact fraction only where no such count exists (66.67 units
    // after a 3-for-1 split is 22.2233…, which nothing shorter re-bases to).
    let approx = exact.get();
    for dp in 0..=approx.scale() {
        let candidate = approx.round_dp(dp);
        if split_adjusted_quantity(AcquiredUnits::new(candidate), splits, acquired, qty.basis)
            .quantity
            == qty.quantity
        {
            return AcquiredUnits::new(candidate);
        }
    }
    exact
}

/// [`as_acquired_quantity`] in checked form: the same conversion, refusing
/// rather than panicking where the result is past `Decimal`'s range.
///
/// Only a **consolidation** between `acquired` and the quantity's date can
/// reach that — it multiplies the quantity up — and only for a quantity larger
/// than the parcel could ever have held, so this is the write paths' guard
/// rather than the reports' (`entities::transfer`, whose 1:1 move has no ratio
/// of its own but still re-bases the units asked for). `label` names the
/// caller's own field so the refusal quotes the request's vocabulary.
pub fn checked_as_acquired_quantity(
    qty: (&str, UnitsAt),
    splits: &[SplitEvent],
    acquired: NaiveDate,
) -> Result<AcquiredUnits, UnrepresentableQuantity> {
    let SplitRatio { new, old } = split_ratio(splits, acquired, qty.1.basis);
    if new == old {
        return Ok(AcquiredUnits::new(qty.1.quantity));
    }
    cost_base::checked_rebased_quantity(
        (qty.0, qty.1.quantity),
        ("old units", old),
        ("new units", new),
    )?;
    Ok(as_acquired_quantity(qty.1, splits, acquired))
}

/// Total units sold out of `parcel`, acquired at `acquired`, re-based to its
/// as-acquired units. Each allocation is expressed in the unit basis of its
/// own sale date — a post-split sale allocates post-split units against the
/// pre-split parcel.
///
/// The sales are taken in date order, and one stating exactly what the
/// parcel's remainder re-bases to on its date consumes exactly that
/// remainder. [`as_acquired_quantity`] alone cannot know it: where the
/// parcel's own count is itself a full-precision figure (a rollover's
/// replacement parcel after a bonus issue holds 658.28571428571428571428571429
/// units), every count within its last digit re-bases to the same rounded
/// figure, and the shortest of them can lie past the parcel — so the Sell of
/// everything the open-parcels schedule shows was refused as exceeding it.
pub fn sold_in_acquired_units(
    parcel: AcquiredUnits,
    sales: impl IntoIterator<Item = UnitsAt>,
    splits: &[SplitEvent],
    acquired: NaiveDate,
) -> AcquiredUnits {
    let sales: Vec<UnitsAt> = sales.into_iter().collect();
    acquired_units_of_sales(parcel, &sales, splits, acquired)
        .into_iter()
        .sum()
}

/// Each of `sales` out of `parcel` (acquired at `acquired`) in the parcel's
/// as-acquired units, in the order given — the per-sale form of
/// [`sold_in_acquired_units`], which is their sum. Every consumer that costs
/// or counts one allocation reads it from here, so the sale that empties a
/// parcel is costed as exactly the remainder the Sell check let it take: read
/// on its own through [`as_acquired_quantity`], the last sale of 67 units
/// after a 2-for-7 bonus issue came back a digit off the remainder, and the
/// cost base it realised a digit off the one the open-parcels schedule
/// carried for it.
///
/// The walk takes the sales in date order (a stable sort, so same-day sales
/// keep the order given).
pub fn acquired_units_of_sales(
    parcel: AcquiredUnits,
    sales: &[UnitsAt],
    splits: &[SplitEvent],
    acquired: NaiveDate,
) -> Vec<AcquiredUnits> {
    let mut order: Vec<usize> = (0..sales.len()).collect();
    order.sort_by_key(|&i| sales[i].basis);
    let mut out = vec![AcquiredUnits::ZERO; sales.len()];
    let mut sold = AcquiredUnits::ZERO;
    for i in order {
        let sale = sales[i];
        let left = parcel - sold;
        let units = if left > AcquiredUnits::ZERO
            && split_adjusted_quantity(left, splits, acquired, sale.basis).quantity == sale.quantity
        {
            left
        } else {
            as_acquired_quantity(sale, splits, acquired)
        };
        sold += units;
        out[i] = units;
    }
    out
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
    listing_id: ListingId,
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

/// Cumulative price-recovery ratio between price bases (a [`SplitRatio`]): the
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
) -> SplitRatio {
    let mut new = Decimal::ONE;
    let mut old = Decimal::ONE;
    for e in events {
        if e.date <= from || e.date > up_to {
            continue;
        }
        new *= e.recover_new;
        old *= e.recover_old;
    }
    SplitRatio { new, old }
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
#[must_use = "a dropped money or quantity figure is a silent money error"]
pub fn contemporaneous_price(
    price: Decimal,
    events: &[PriceBasisEvent],
    price_date: NaiveDate,
    observed: NaiveDate,
) -> Decimal {
    let SplitRatio { new, old } = price_basis_ratio(events, price_date, observed);
    if new == old {
        price
    } else {
        mul_div(&[price, new], old)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    /// Up to four re-basing events in the 2015–2020 window, each a share split
    /// (or consolidation) `a`-for-`b` or a bonus issue of `a` for every `b`
    /// held.
    fn rebasings() -> impl Strategy<Value = Vec<SplitEvent>> {
        prop::collection::vec((1i64..2000, 1i64..=20, 1i64..=20, any::<bool>()), 0..=4).prop_map(
            |events| {
                events
                    .into_iter()
                    .map(|(day, a, b, bonus)| {
                        let date = acquired() + chrono::Duration::days(day);
                        let (a, b) = (Decimal::from(a), Decimal::from(b));
                        if bonus {
                            SplitEvent::bonus_issue(date, a, b)
                        } else {
                            SplitEvent::share_split(date, a, b)
                        }
                    })
                    .collect()
            },
        )
    }

    fn acquired() -> NaiveDate {
        NaiveDate::from_ymd_opt(2015, 1, 1).unwrap()
    }

    /// After every event [`rebasings`] can date.
    fn after_all() -> NaiveDate {
        acquired() + chrono::Duration::days(2000)
    }

    proptest! {
        /// A parcel whose units divide evenly through every event re-bases
        /// exactly — `k × Π old` units become `k × Π new` — and the units
        /// sold out of it in the post-event basis convert back to exactly the
        /// units bought, so a fully sold parcel is never left with a residue.
        #[test]
        fn an_evenly_divisible_parcel_re_bases_and_returns_exactly(
            splits in rebasings(),
            k in 1i64..1_000_000,
        ) {
            let old: Decimal = splits.iter().map(|s| s.old_units).product();
            let new: Decimal = splits.iter().map(|s| s.new_units).product();
            let bought = Decimal::from(k) * old;

            let held = split_adjusted_quantity(
                AcquiredUnits::new(bought),
                &splits,
                acquired(),
                Some(after_all()),
            );
            prop_assert_eq!(held, Decimal::from(k) * new);
            prop_assert_eq!(held.basis(), Some(after_all()));
            prop_assert_eq!(
                held.quantity(),
                split_adjusted_quantity(AcquiredUnits::new(bought), &splits, acquired(), None)
                    .quantity()
            );
            prop_assert_eq!(as_acquired_quantity(held, &splits, acquired()), bought);
            prop_assert_eq!(
                sold_in_acquired_units(AcquiredUnits::new(bought), [held], &splits, acquired()),
                bought
            );
            // Re-basing runs both ways: back to the acquisition date's basis
            // is the as-acquired count again.
            let back = held.rebased(&splits, Some(acquired()));
            prop_assert_eq!(back, bought);
            prop_assert_eq!(back.basis(), Some(acquired()));
        }

        /// Sales in the post-event basis that do **not** divide through the
        /// ratios — each one a non-terminating number of as-acquired units —
        /// still leave an exact remainder: re-based forward it is the units
        /// held less the units sold, to the last digit, and a parcel sold in
        /// full is left with exactly nothing (no residue to keep it open, no
        /// excess to refuse the last sale).
        #[test]
        fn sales_that_do_not_divide_the_ratio_leave_an_exact_remainder(
            splits in rebasings(),
            k in 1i64..1_000_000,
            cuts in prop::collection::vec(0u32..=1000, 1..=6),
            sell_all in any::<bool>(),
        ) {
            let old: Decimal = splits.iter().map(|s| s.old_units).product();
            let new: Decimal = splits.iter().map(|s| s.new_units).product();
            let bought = AcquiredUnits::new(Decimal::from(k) * old);
            let held = Decimal::from(k) * new;

            // Integer sales of the post-event units, each a per-mille share
            // of what was held (the last one taking the rest when selling all).
            let mut sales: Vec<Decimal> = cuts
                .iter()
                .map(|c| (held * Decimal::from(*c) / Decimal::from(1000u32 * 6)).floor())
                .collect();
            if sell_all {
                let so_far: Decimal = sales.iter().sum();
                sales.push(held - so_far);
            }
            let sold_post: Decimal = sales.iter().sum();
            let sold = sold_in_acquired_units(
                bought,
                sales.iter().map(|q| UnitsAt::new(*q, after_all())),
                &splits,
                acquired(),
            );
            let remaining = bought - sold;

            prop_assert!(sold <= bought);
            prop_assert_eq!(
                split_adjusted_quantity(remaining, &splits, acquired(), Some(after_all())),
                held - sold_post
            );
            if sell_all {
                prop_assert_eq!(remaining, AcquiredUnits::ZERO);
            }
        }

        /// Any quantity — fractional, and through ratios that do not
        /// terminate (a 1-for-3 consolidation) — survives the round trip to
        /// the post-event basis and back to within `Decimal`'s last digits:
        /// the conversion multiplies before it divides, so it rounds once
        /// each way rather than compounding per event.
        #[test]
        fn any_quantity_round_trips_to_within_decimal_precision(
            splits in rebasings(),
            mantissa in 1i64..1_000_000_000_000,
            scale in 0u32..=6,
        ) {
            let bought = Decimal::new(mantissa, scale);
            let held = split_adjusted_quantity(
                AcquiredUnits::new(bought),
                &splits,
                acquired(),
                Some(after_all()),
            );
            let back = as_acquired_quantity(held, &splits, acquired());
            let tolerance = bought * Decimal::new(1, 20);
            prop_assert!(
                (back.get() - bought).abs() <= tolerance,
                "{bought} → {held:?} → {back:?}"
            );
        }

        /// Re-basing across a window is re-basing across its two halves: the
        /// cumulative ratio over `(acquired, at]` is the product, numerator
        /// and denominator separately, of the ratios over `(acquired, mid]`
        /// and `(mid, at]` — so an event on the boundary is counted in
        /// exactly one half.
        #[test]
        fn the_ratio_over_a_window_is_the_product_of_its_halves(
            splits in rebasings(),
            mid in 0i64..=2000,
        ) {
            let mid = acquired() + chrono::Duration::days(mid);
            let whole = split_ratio(&splits, acquired(), Some(after_all()));
            let first = split_ratio(&splits, acquired(), Some(mid));
            let second = split_ratio(&splits, mid, Some(after_all()));
            prop_assert_eq!(whole.new, first.new * second.new);
            prop_assert_eq!(whole.old, first.old * second.old);
        }
    }
}
