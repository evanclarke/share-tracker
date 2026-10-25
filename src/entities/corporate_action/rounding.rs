//! The stored per-parcel counts of a listing's rounded share splits and bonus
//! issues (`corporate_action_parcel_counts`, migration 0055): computed per
//! holding by `domain::entitlement_rounding` and written inside the write
//! transaction of whatever changed them — the action itself (its ratio, rule,
//! date or listing; a new action; a deletion), a "units received" override
//! ([`crate::entities::entitlement_override`]), or any trade, sale, allocation
//! or unit-scale write the counts were computed from.
//!
//! **Settling** ([`db_settle`]) is how every such write gets here. Migration
//! 0057's triggers mark the written fact's listing in `rounding_dirty`, whose
//! rows violate a deferred foreign key, so a transaction that wrote one cannot
//! commit until `db_settle` has recomputed the listing and drained the row —
//! a write path that forgets to settle fails at COMMIT instead of storing
//! counts computed from facts that have since moved. Settling also runs the
//! checks the recomputed counts must pass (representability, every sale still
//! fitting its parcel, every recorded cash-in-lieu payment still selling the
//! fraction it was paid for) and answers a failure `422` through
//! [`crate::infra::http::Refusal`].
//!
//! The whole listing is recomputed in date order, because each action's
//! "units before" are re-based through every earlier one — an earlier
//! rounded action by the counts this pass has just stored for it. Each
//! holding's distribution is **stable**: the counts already stored are passed
//! back as priors, and `round_holding` keeps every one the new total allows
//! (a registry does not redistribute a holding it has already rounded). The
//! rows are then reconciled with what is stored: a row that would not change
//! is not rewritten, so recomputing after an unrelated edit writes nothing.
//!
//! **Which units are rounded — the register at the record date.** A share
//! split's or bonus issue's `date` is its ex-date: a trade dated before it is
//! stated in the pre-action basis and is converted by the re-base, a trade on
//! or after it is already in the post-action basis (the convention every
//! quantity re-base keys on, `domain::adjustments::split_ratio`). The registry
//! rounds something else — each holding as its **register** stood at the
//! action's record date, read off settlement dates (a Buy settled on or before
//! it, a Sell settled after it; CLAUDE.md, Financial correctness). Under the
//! market timetable the two sets are the same for every normally settled trade
//! (ASX: the record date is the business day after the ex-date, and post-ex
//! trades settle on a deferred basis after the register is reconstructed;
//! NYSE: a due bill carries the new shares to a buyer between the record date
//! and the ex-date), so they differ only by a trade that **straddles** them —
//! traded before the ex-date, settled after the record date. Such a trade is a
//! settlement obligation the clearing house converts on its own, and that is
//! how it is rounded here:
//!
//! - a **straddling Buy** is not on its buyer's register at the record date: its
//!   parcel is rounded alone (`Rounding::for_obligation` — the action's rule,
//!   with a cash-in-lieu fraction left to the registered seller);
//! - a **straddling Sell** was still on its seller's register: its units are
//!   rounded with the holding, and its own converted count is delivered out of
//!   the rounded total, the holding's remaining parcels sharing what is left
//!   (`entitlement_rounding::round_registered_holding`).
//!
//! Every per-parcel count is still stored against the parcels the re-base
//! converts — the ones traded before the action — so the re-base and the
//! rounding always describe the same units. A trade traded on or after the
//! ex-date is in the post-action basis whenever it settles, and takes no part.
//! With no record date recorded the register is the timetable's: every trade
//! dated before the action, which is what every action recorded before the
//! column existed has always rounded.

use crate::domain::adjustments::{
    AcquiredUnits, Fraction, SplitEvent, StoredCount, UnitsAt, sold_in_acquired_units,
    split_adjusted_fraction, split_adjusted_quantity,
};
use crate::domain::cost_base::UnrepresentableQuantity;
use crate::domain::entitlement_rounding::{self, ParcelBefore, ParcelCount, Prior, RoundingError};
use crate::domain::rollover::StraddlingSale;
use crate::entities::corporate_action::model::FractionRule;
use crate::entities::holding_unit_scale;
use crate::infra::decimal::{Money, parse_dec};
use crate::infra::http::Refusal;
use crate::infra::id::{HoldingAccountId, ListingId, TradeId};
use chrono::NaiveDate;
use rust_decimal::Decimal;
use sqlx::Row;
use std::collections::{BTreeMap, HashMap};

/// Why a listing's rounded actions could not be stored.
#[derive(thiserror::Error, Debug)]
pub enum RoundError {
    #[error("rounded-entitlement write failed: {0}")]
    Db(#[from] sqlx::Error),
    /// The rounding of one holding failed — the action, the account and the
    /// reason. Mapped to `422` by the caller.
    #[error("corporate action #{action_id} cannot round holding account {account}: {source}")]
    Holding {
        action_id: i64,
        account: HoldingAccountId,
        #[source]
        source: RoundingError,
    },
}

impl RoundError {
    /// The user-facing `422` body (`None` for a database failure).
    pub fn detail(&self) -> Option<String> {
        let RoundError::Holding {
            action_id,
            account,
            source,
        } = self
        else {
            return None;
        };
        Some(match source {
            RoundingError::ParcelEmptied(trade) => format!(
                "corporate action #{action_id}'s rounding leaves parcel (trade #{trade}) in \
                 holding account {account} with no units: the holding's rounded count is too \
                 small to give every parcel a unit on its grid (or the parcel, settled after the \
                 action's record date, is converted on its own and is too small for a unit), \
                 and a parcel with no units would strand its cost base where no sale can reach \
                 it. Record the action with fraction_rule Exact, or a units-received override for that account, instead"
            ),
            RoundingError::ReceivedNotPositive(received) => format!(
                "the units received by holding account {account} from corporate action \
                 #{action_id} must be positive, not {received}"
            ),
            RoundingError::ReceivedOffGrid { received, scale } => format!(
                "the units received by holding account {account} from corporate action \
                 #{action_id}, {received}, are finer than the holding's unit scale of {scale}"
            ),
            RoundingError::Unrepresentable => format!(
                "corporate action #{action_id}'s ratio applied to holding account {account} is \
                 beyond the representable range"
            ),
        })
    }
}

/// One share split / bonus issue of the listing, as the pass reads it.
struct Action {
    id: i64,
    date: NaiveDate,
    /// The register's record date, if one is recorded.
    record_date: Option<NaiveDate>,
    event: SplitEvent,
    rule: FractionRule,
}

/// One Buy/DRP parcel of the listing.
struct Parcel {
    id: TradeId,
    date: NaiveDate,
    /// Its settlement date — when it joined the register.
    settled: NaiveDate,
    quantity: AcquiredUnits,
    account: HoldingAccountId,
}

/// One sale's allocation from a parcel of the listing, as the pass reads it.
struct Sale {
    /// The Sell.
    sale: TradeId,
    /// The units allocated, in the basis of the sale's own (trade) date.
    units: UnitsAt,
    /// The Sell's settlement date — when its units left the register.
    settled: NaiveDate,
}

/// What one holding's register held at a rounded action's record date beyond
/// its registered parcels' units before the action: the units of each Sell
/// traded before the action that had not yet settled (by Sell, each
/// allocation in the basis immediately before the action, exact).
type Pending = BTreeMap<TradeId, Vec<Fraction>>;

/// Each straddling Sell's units as one exact count — the register's
/// deliveries (`entitlement_rounding::Register::delivered`).
fn delivered(pending: Pending) -> Vec<Fraction> {
    pending
        .into_values()
        .map(|allocations| {
            Fraction::sum(&allocations).unwrap_or_else(|| {
                // Past `Decimal`'s range on one denominator: the 28-digit
                // roundings, summed.
                Fraction::whole(allocations.iter().map(|f| f.num / f.den).sum())
            })
        })
        .collect()
}

/// Why a write's recomputed counts could not stand. Each is answered `422`
/// (through [`Refusal`]) by [`db_settle`].
#[derive(thiserror::Error, Debug)]
pub enum SettleError {
    /// A holding cannot be rounded (a parcel left with no units, or an
    /// override that does not fit its grid).
    #[error("a rounded holding cannot be stored: {0}")]
    Rounding(#[source] RoundError),
    /// The listing's split stream re-bases a quantity past `Decimal`'s range.
    #[error("a rebased quantity is unrepresentable: {0}")]
    Unrepresentable(#[source] UnrepresentableQuantity),
    /// The recomputed counts leave a sale allocating more units than its
    /// parcel now holds.
    #[error("a sale no longer fits the recomputed count of its parcel")]
    AllocationsExceedParcel,
    /// A recorded cash-in-lieu payment no longer sells the fraction its
    /// holding now carries (the fraction, or the parcel carrying it, moved).
    #[error("cash-in-lieu Sell #{sell} no longer sells corporate action #{action_id}'s fraction")]
    CashInLieuMoved {
        sell: TradeId,
        action_id: i64,
        account: HoldingAccountId,
    },
}

impl SettleError {
    /// The user-facing `422` body.
    fn body(&self) -> String {
        match self {
            SettleError::Rounding(e) => e.detail().unwrap_or_else(|| e.to_string()),
            SettleError::Unrepresentable(e) => e.message(),
            SettleError::AllocationsExceedParcel => {
                "this write changes the units a rounded share split or bonus issue of the \
                 listing converted, and the counts recomputed for it leave a sale allocating \
                 more units than the parcel it draws on now holds — correct or remove those \
                 allocations first"
                    .to_owned()
            }
            SettleError::CashInLieuMoved {
                sell,
                action_id,
                account,
            } => format!(
                "this write changes the fraction corporate action #{action_id} left holding \
                 account {account}, which the cash-in-lieu Sell #{sell} has already sold — \
                 delete that Sell (DELETE /sells/{sell}), make this change, then record the \
                 payment again"
            ),
        }
    }
}

impl From<SettleError> for sqlx::Error {
    fn from(e: SettleError) -> Self {
        match e {
            SettleError::Rounding(RoundError::Db(err)) => err,
            refused @ (SettleError::Rounding(RoundError::Holding { .. })
            | SettleError::Unrepresentable(_)
            | SettleError::AllocationsExceedParcel
            | SettleError::CashInLieuMoved { .. }) => Refusal::new(refused.body(), refused).into(),
        }
    }
}

/// Recompute every listing the open write transaction has marked dirty
/// (`rounding_dirty`, migration 0057), check the state it leaves, and clear
/// the marks — called by every write path that can touch a fact a rounded
/// share split's or bonus issue's stored counts are computed from, after its
/// own writes and before it commits (or rolls back a `?dry_run=true`
/// preview). A transaction that leaves a mark behind cannot commit.
///
/// A refusal is a [`Refusal`] carried in the `sqlx::Error`, so `?` delivers
/// it to the handler as the `422` it is.
pub async fn db_settle(conn: &mut sqlx::SqliteConnection) -> Result<(), sqlx::Error> {
    let dirty: Vec<ListingId> =
        sqlx::query_scalar("SELECT DISTINCT listing_id FROM rounding_dirty ORDER BY listing_id")
            .fetch_all(&mut *conn)
            .await?;
    for listing_id in dirty {
        settle_listing(&mut *conn, listing_id).await?;
    }
    sqlx::query("DELETE FROM rounding_dirty")
        .execute(&mut *conn)
        .await?;
    Ok(())
}

/// One dirty listing: recompute, then the checks the recomputed counts must
/// pass.
async fn settle_listing(
    conn: &mut sqlx::SqliteConnection,
    listing_id: ListingId,
) -> Result<(), SettleError> {
    db_recompute_listing(&mut *conn, listing_id)
        .await
        .map_err(SettleError::Rounding)?;
    let db = |e: sqlx::Error| SettleError::Rounding(RoundError::Db(e));
    // Representability first: `allocations_fit_parcels` re-bases each
    // allocation itself, so an unrepresentable one overflows inside that check
    // before it can answer.
    if let Some(beyond) = super::db::rebased_quantity_beyond_range(&mut *conn, listing_id)
        .await
        .map_err(db)?
    {
        return Err(SettleError::Unrepresentable(beyond));
    }
    if !super::db::allocations_fit_parcels(&mut *conn, listing_id)
        .await
        .map_err(db)?
    {
        return Err(SettleError::AllocationsExceedParcel);
    }
    if let Some((sell, action_id, account)) =
        db_moved_payment(&mut *conn, listing_id).await.map_err(db)?
    {
        return Err(SettleError::CashInLieuMoved {
            sell,
            action_id,
            account,
        });
    }
    Ok(())
}

/// The first cash-in-lieu Sell of one of the listing's split/bonus actions
/// that no longer sells exactly the fraction its holding carries — a
/// different parcel, or a different quantity once re-based to the payment
/// date — as (Sell, action, account).
async fn db_moved_payment(
    conn: &mut sqlx::SqliteConnection,
    listing_id: ListingId,
) -> Result<Option<(TradeId, i64, HoldingAccountId)>, sqlx::Error> {
    let payments = sqlx::query(
        "SELECT s.id, s.date AS paid, s.holding_account_id, a.id AS action_id, \
                a.date AS action_date, pa.purchase_trade_id, pa.quantity_allocated \
         FROM trades s \
         JOIN corporate_actions a ON a.id = s.cash_in_lieu_action_id \
         JOIN parcel_allocations pa ON pa.sale_trade_id = s.id \
         WHERE a.listing_id = ? \
           AND (a.action_type = 'ShareSplit' OR a.action_type = 'BonusIssue') \
         ORDER BY s.id",
    )
    .bind(listing_id)
    .fetch_all(&mut *conn)
    .await?;
    if payments.is_empty() {
        return Ok(None);
    }
    let splits = super::db_splits_for_listing(&mut *conn, listing_id).await?;
    for row in &payments {
        let sell: TradeId = row.try_get("id")?;
        let action_id: i64 = row.try_get("action_id")?;
        let account: HoldingAccountId = row.try_get("holding_account_id")?;
        let parcel: TradeId = row.try_get("purchase_trade_id")?;
        let sold = parse_dec("quantity_allocated", row.try_get("quantity_allocated")?)?;
        let carried: Option<String> = sqlx::query_scalar(
            "SELECT c.in_lieu_units FROM corporate_action_parcel_counts c \
             WHERE c.corporate_action_id = ? AND c.trade_id = ?",
        )
        .bind(action_id)
        .bind(parcel)
        .fetch_optional(&mut *conn)
        .await?;
        let carried = carried
            .map(|c| parse_dec("in_lieu_units", &c))
            .transpose()?
            .unwrap_or(Decimal::ZERO);
        let fits = carried > Decimal::ZERO
            && UnitsAt::new(carried, row.try_get("action_date")?)
                .rebased(&splits.for_parcel(parcel), Some(row.try_get("paid")?))
                .quantity()
                == sold;
        if !fits {
            return Ok(Some((sell, action_id, account)));
        }
    }
    Ok(None)
}

/// Recompute and store the per-parcel counts of every rounded share split and
/// bonus issue of `listing_id`, on the caller's write transaction — keeping,
/// holding by holding, every count already stored that the new total allows.
/// Reached through [`db_settle`].
async fn db_recompute_listing(
    conn: &mut sqlx::SqliteConnection,
    listing_id: ListingId,
) -> Result<(), RoundError> {
    let action_rows = sqlx::query(
        "SELECT id, action_type, date, rebase_record_date, split_new_units, split_old_units, \
                bonus_units, bonus_held_units, fraction_rule FROM corporate_actions \
         WHERE listing_id = ? AND (action_type = 'ShareSplit' OR action_type = 'BonusIssue') \
         ORDER BY date, id",
    )
    .bind(listing_id)
    .fetch_all(&mut *conn)
    .await?;
    let mut actions = Vec::with_capacity(action_rows.len());
    for row in &action_rows {
        let date: NaiveDate = row.try_get("date")?;
        let event = match row.try_get::<String, _>("action_type")?.as_str() {
            "BonusIssue" => SplitEvent::bonus_issue(
                date,
                parse_dec("bonus_units", row.try_get("bonus_units")?)?,
                parse_dec("bonus_held_units", row.try_get("bonus_held_units")?)?,
            ),
            _ => SplitEvent::share_split(
                date,
                parse_dec("split_new_units", row.try_get("split_new_units")?)?,
                parse_dec("split_old_units", row.try_get("split_old_units")?)?,
            ),
        };
        let rule = row
            .try_get::<Option<&str>, _>("fraction_rule")?
            .map_or(Ok(FractionRule::Exact), str::parse)
            .map_err(|e| sqlx::Error::Decode(Box::new(e)))?;
        actions.push(Action {
            id: row.try_get("id")?,
            date,
            record_date: row.try_get("rebase_record_date")?,
            event,
            rule,
        });
    }

    // What should be stored, by (action, parcel).
    let mut desired: BTreeMap<(i64, TradeId), ParcelCount> = BTreeMap::new();
    if actions.iter().any(|a| a.rule.rounding().is_some()) {
        let parcels = db_parcels(&mut *conn, listing_id).await?;
        let sales = db_sales(&mut *conn, listing_id).await?;
        let overrides = db_overrides(&mut *conn, listing_id).await?;
        let priors = db_priors(&mut *conn, listing_id).await?;
        let mut scales: HashMap<HoldingAccountId, Option<u32>> = HashMap::new();
        // Each rounded action's stored counts, as computed so far in this
        // pass — what re-bases a parcel across it for the actions after it.
        let mut computed: HashMap<i64, HashMap<TradeId, StoredCount>> = HashMap::new();

        for (index, action) in actions.iter().enumerate() {
            let Some(rounding) = action.rule.rounding() else {
                continue;
            };
            // Each account's registered parcels as they stood immediately
            // before the action, in the basis then in force, with what its
            // register held at the record date; and the parcels that joined
            // no register by then, each its own settlement obligation.
            let mut holdings: BTreeMap<HoldingAccountId, (Vec<ParcelBefore>, Pending)> =
                BTreeMap::new();
            let mut obligations: Vec<(HoldingAccountId, ParcelBefore)> = Vec::new();
            // Registered at the record date: settled on or before it. With
            // none recorded, the timetable's register — every trade dated
            // before the action.
            let registered = |settled: NaiveDate| action.record_date.is_none_or(|d| settled <= d);
            for parcel in parcels.iter().filter(|p| p.date < action.date) {
                let prior: Vec<SplitEvent> = actions[..index]
                    .iter()
                    .map(|a| {
                        let mut event = a.event.clone();
                        if let Some(count) = computed.get(&a.id).and_then(|c| c.get(&parcel.id)) {
                            event.new_units = count.after;
                            event.old_units = count.before;
                        }
                        event
                    })
                    .collect();
                // The sales traded before the action, which its basis
                // already deducts from the parcel.
                let sold_before: Vec<&Sale> = sales
                    .get(&parcel.id)
                    .map_or(&[][..], |v| v)
                    .iter()
                    .filter(|s| s.units.basis().is_some_and(|d| d < action.date))
                    .collect();
                let remaining = parcel.quantity
                    - sold_in_acquired_units(
                        parcel.quantity,
                        sold_before.iter().map(|s| s.units),
                        &prior,
                        parcel.date,
                    );
                let (units, exact) = if remaining > AcquiredUnits::ZERO {
                    (
                        split_adjusted_quantity(remaining, &prior, parcel.date, None).quantity(),
                        split_adjusted_fraction(remaining, &prior, parcel.date, None),
                    )
                } else {
                    (Decimal::ZERO, None)
                };
                let before = ParcelBefore {
                    trade_id: parcel.id,
                    acquired: parcel.date,
                    units,
                    exact,
                    prior: priors.get(&(action.id, parcel.id)).copied(),
                };
                if !registered(parcel.settled) {
                    if units > Decimal::ZERO {
                        obligations.push((parcel.account, before));
                    }
                    continue;
                }
                let (held, pending) = holdings.entry(parcel.account).or_default();
                // A sale traded before the action but settled after the
                // record date: its units were still on the register, and
                // leave it afterwards as an obligation of their own.
                for sale in sold_before.iter().filter(|s| !registered(s.settled)) {
                    let units = sale
                        .units
                        .rebased_fraction(&prior, None)
                        .unwrap_or_else(|| {
                            Fraction::whole(sale.units.rebased(&prior, None).quantity())
                        });
                    pending.entry(sale.sale).or_default().push(units);
                }
                if units > Decimal::ZERO {
                    held.push(before);
                }
            }

            let mut stored = HashMap::new();
            let mut store = |counts: Vec<ParcelCount>| {
                for count in counts {
                    stored.insert(
                        count.trade_id,
                        StoredCount {
                            before: count.before,
                            after: count.after,
                        },
                    );
                    desired.insert((action.id, count.trade_id), count);
                }
            };
            for (account, (before, pending)) in holdings {
                // An unconstrained holding has no grid to round onto: the
                // registry holds what the ratio produces, as for `Exact`.
                let Some(scale) = scale_of(&mut *conn, &mut scales, account, listing_id).await?
                else {
                    continue;
                };
                let received = overrides.get(&(action.id, account)).copied();
                let counts = if pending.is_empty() {
                    entitlement_rounding::round_holding(
                        &before,
                        action.event.new_units,
                        action.event.old_units,
                        scale,
                        rounding,
                        received,
                    )
                } else {
                    entitlement_rounding::round_registered_holding(
                        &before,
                        &entitlement_rounding::Register {
                            delivered: delivered(pending),
                        },
                        action.event.new_units,
                        action.event.old_units,
                        scale,
                        rounding,
                        received,
                    )
                }
                .map_err(|source| RoundError::Holding {
                    action_id: action.id,
                    account,
                    source,
                })?;
                store(counts);
            }
            // A parcel traded before the action but registered after the
            // record date: the seller's holding was rounded with its units,
            // and the clearing house converts the obligation on its own.
            for (account, before) in obligations {
                let Some(scale) = scale_of(&mut *conn, &mut scales, account, listing_id).await?
                else {
                    continue;
                };
                let counts = entitlement_rounding::round_holding(
                    &[before],
                    action.event.new_units,
                    action.event.old_units,
                    scale,
                    rounding.for_obligation(),
                    None,
                )
                .map_err(|source| RoundError::Holding {
                    action_id: action.id,
                    account,
                    source,
                })?;
                store(counts);
            }
            computed.insert(action.id, stored);
        }
    }

    reconcile(&mut *conn, &actions, desired).await
}

/// Bring the stored rows of `actions` into line with `desired`: unchanged rows
/// are left alone, changed ones updated, missing ones inserted and the rest
/// deleted.
async fn reconcile(
    conn: &mut sqlx::SqliteConnection,
    actions: &[Action],
    mut desired: BTreeMap<(i64, TradeId), ParcelCount>,
) -> Result<(), RoundError> {
    for action in actions {
        let existing = sqlx::query(
            "SELECT id, trade_id, units_before, units_after, in_lieu_units \
             FROM corporate_action_parcel_counts WHERE corporate_action_id = ?",
        )
        .bind(action.id)
        .fetch_all(&mut *conn)
        .await?;
        for row in &existing {
            let row_id: i64 = row.try_get("id")?;
            let trade_id: TradeId = row.try_get("trade_id")?;
            match desired.remove(&(action.id, trade_id)) {
                Some(count) => {
                    let same = parse_dec("units_before", row.try_get("units_before")?)?
                        == count.before
                        && parse_dec("units_after", row.try_get("units_after")?)? == count.after
                        && parse_dec("in_lieu_units", row.try_get("in_lieu_units")?)?
                            == count.in_lieu;
                    if !same {
                        sqlx::query(
                            "UPDATE corporate_action_parcel_counts \
                             SET units_before = ?, units_after = ?, in_lieu_units = ? \
                             WHERE id = ?",
                        )
                        .bind(Money(count.before))
                        .bind(Money(count.after))
                        .bind(Money(count.in_lieu))
                        .bind(row_id)
                        .execute(&mut *conn)
                        .await?;
                    }
                }
                None => {
                    sqlx::query("DELETE FROM corporate_action_parcel_counts WHERE id = ?")
                        .bind(row_id)
                        .execute(&mut *conn)
                        .await?;
                }
            }
        }
    }
    for ((action_id, _), count) in desired {
        db_insert_count(&mut *conn, action_id, &count).await?;
    }
    Ok(())
}

/// Store one parcel's count for `action_id` — also how the exchange and
/// demerger record the count they wrote onto a replacement Buy.
pub async fn db_insert_count(
    conn: &mut sqlx::SqliteConnection,
    action_id: i64,
    count: &ParcelCount,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO corporate_action_parcel_counts \
         (corporate_action_id, trade_id, units_before, units_after, in_lieu_units) \
         VALUES (?, ?, ?, ?, ?)",
    )
    .bind(action_id)
    .bind(count.trade_id)
    .bind(Money(count.before))
    .bind(Money(count.after))
    .bind(Money(count.in_lieu))
    .execute(&mut *conn)
    .await?;
    Ok(())
}

/// One parcel a scrip-for-scrip exchange or demerger converts, as it stood at
/// the operation date.
#[derive(Debug, Clone, Copy)]
pub struct Converted {
    /// The source parcel.
    pub parcel: TradeId,
    /// Its acquisition (trade) date — the distribution's tie-break.
    pub acquired: NaiveDate,
    /// Its settlement date — when it joined its holding's register, which
    /// decides whether the registry rounded it with the holding or converts
    /// it as a settlement obligation of its own.
    pub settled: NaiveDate,
    /// Its holding account: each account is one holding, rounded once.
    pub account: HoldingAccountId,
    /// Its units at the operation date.
    pub units: Decimal,
    /// The same units as an exact fraction, where `units` rounds a count
    /// that does not terminate ([`ParcelBefore::exact`]).
    pub exact: Option<Fraction>,
}

/// What a scrip-for-scrip exchange or demerger rounds: the action's rule, the
/// listing its replacement units land on, its ratio, and the register at its
/// record date.
pub struct Replacing<'a> {
    /// The corporate action (its units-received overrides are read by it).
    pub action_id: i64,
    /// The issuer's fraction rule.
    pub rule: FractionRule,
    /// The replacement or demerged listing — whose grid the units land on.
    pub destination: ListingId,
    /// `(new, old)`: every `old` units held become `new`.
    pub ratio: (Decimal, Decimal),
    /// The action's record date, if one is recorded (`None`: every parcel
    /// traded before the operation was registered).
    pub record_date: Option<NaiveDate>,
    /// The parcels the operation converts (every one open on its date).
    pub parcels: &'a [Converted],
    /// The allocations of Sells straddling the record date
    /// (`rollover::CostBaseInputs::straddling_sales`).
    pub straddling: &'a [StraddlingSale],
}

/// The replacement count each source parcel's holding is rounded to by a
/// scrip-for-scrip exchange or demerger — its units of the destination
/// listing on that listing's grid in the account, by the rule, or a
/// units-received override — keyed by source parcel ([`ParcelCount::trade_id`]
/// is the source). A parcel in a holding with no grid, or under an `Exact`
/// rule, is absent: its replacement keeps the exact ratio's count.
///
/// **The register at the record date** is what is rounded, as for a share
/// split or bonus issue (this module's doc): the operation converts — and its
/// closing Sell consumes — every parcel open on its date by **trade** date,
/// whatever the record date says, and the record date decides only how each
/// one's replacement count is rounded. A parcel registered by it (settled on
/// or before it) is rounded with its holding; a Sell straddling it (traded
/// before the operation, settled after the record date) was still on the
/// register, so its units are rounded with the holding and its own converted
/// count delivered out of the total
/// (`entitlement_rounding::round_registered_holding`); and a parcel that
/// joined no register by then is a settlement obligation, rounded alone by
/// `Rounding::for_obligation`. With no record date every parcel is
/// registered, which is what every operation before the field existed
/// rounded.
pub async fn db_round_replacements(
    conn: &mut sqlx::SqliteConnection,
    replacing: &Replacing<'_>,
) -> Result<HashMap<TradeId, ParcelCount>, RoundError> {
    let Replacing {
        action_id,
        rule,
        destination,
        ratio: (new, old),
        record_date,
        parcels,
        straddling,
    } = *replacing;
    let Some(rounding) = rule.rounding() else {
        return Ok(HashMap::new());
    };
    let registered = |settled: NaiveDate| record_date.is_none_or(|d| settled <= d);
    let mut holdings: BTreeMap<HoldingAccountId, (Vec<ParcelBefore>, Pending)> = BTreeMap::new();
    let mut obligations: Vec<(HoldingAccountId, ParcelBefore)> = Vec::new();
    for p in parcels.iter().filter(|p| p.units > Decimal::ZERO) {
        let before = ParcelBefore {
            trade_id: p.parcel,
            acquired: p.acquired,
            units: p.units,
            exact: p.exact,
            // The operation runs once, over parcels nothing has rounded yet.
            prior: None,
        };
        if registered(p.settled) {
            holdings.entry(p.account).or_default().0.push(before);
        } else {
            obligations.push((p.account, before));
        }
    }
    for sale in straddling {
        holdings
            .entry(sale.account)
            .or_default()
            .1
            .entry(sale.sale)
            .or_default()
            .push(sale.units);
    }
    let overrides: HashMap<HoldingAccountId, Decimal> = sqlx::query(
        "SELECT holding_account_id, units_received FROM entitlement_overrides \
         WHERE corporate_action_id = ?",
    )
    .bind(action_id)
    .fetch_all(&mut *conn)
    .await?
    .iter()
    .map(|row| {
        Ok((
            row.try_get("holding_account_id")?,
            parse_dec("units_received", row.try_get("units_received")?)?,
        ))
    })
    .collect::<Result<_, sqlx::Error>>()?;
    let holding_error = |account| {
        move |source| RoundError::Holding {
            action_id,
            account,
            source,
        }
    };
    let mut out = HashMap::new();
    for (account, (before, pending)) in holdings {
        let Some(scale) =
            holding_unit_scale::db_unit_scale(&mut *conn, account, destination).await?
        else {
            continue;
        };
        let received = overrides.get(&account).copied();
        let counts = if pending.is_empty() {
            entitlement_rounding::round_holding(&before, new, old, scale, rounding, received)
        } else {
            entitlement_rounding::round_registered_holding(
                &before,
                &entitlement_rounding::Register {
                    delivered: delivered(pending),
                },
                new,
                old,
                scale,
                rounding,
                received,
            )
        }
        .map_err(holding_error(account))?;
        out.extend(counts.into_iter().map(|c| (c.trade_id, c)));
    }
    // A parcel traded before the operation but registered after the record
    // date: the seller's holding was rounded with its units, and the clearing
    // house converts the obligation on its own.
    for (account, before) in obligations {
        let Some(scale) =
            holding_unit_scale::db_unit_scale(&mut *conn, account, destination).await?
        else {
            continue;
        };
        let counts = entitlement_rounding::round_holding(
            &[before],
            new,
            old,
            scale,
            rounding.for_obligation(),
            None,
        )
        .map_err(holding_error(account))?;
        out.extend(counts.into_iter().map(|c| (c.trade_id, c)));
    }
    Ok(out)
}

/// Every Buy/DRP parcel of the listing, in date (then id) order.
async fn db_parcels(
    conn: &mut sqlx::SqliteConnection,
    listing_id: ListingId,
) -> Result<Vec<Parcel>, sqlx::Error> {
    let rows = sqlx::query(
        "SELECT id, date, settlement_date, quantity, holding_account_id FROM trades \
         WHERE listing_id = ? AND trade_type IN ('Buy', 'DRP') ORDER BY date, id",
    )
    .bind(listing_id)
    .fetch_all(&mut *conn)
    .await?;
    rows.iter()
        .map(|row| {
            Ok(Parcel {
                id: row.try_get("id")?,
                date: row.try_get("date")?,
                settled: row.try_get("settlement_date")?,
                quantity: AcquiredUnits::new(parse_dec("quantity", row.try_get("quantity")?)?),
                account: row.try_get("holding_account_id")?,
            })
        })
        .collect()
}

/// Every allocation from a parcel of the listing, by parcel.
async fn db_sales(
    conn: &mut sqlx::SqliteConnection,
    listing_id: ListingId,
) -> Result<HashMap<TradeId, Vec<Sale>>, sqlx::Error> {
    let rows = sqlx::query(
        "SELECT pa.purchase_trade_id, pa.sale_trade_id, s.date, s.settlement_date, \
                pa.quantity_allocated \
         FROM parcel_allocations pa \
         JOIN trades s ON s.id = pa.sale_trade_id \
         JOIN trades p ON p.id = pa.purchase_trade_id \
         WHERE p.listing_id = ? ORDER BY pa.sale_trade_id, pa.purchase_trade_id",
    )
    .bind(listing_id)
    .fetch_all(&mut *conn)
    .await?;
    let mut sales: HashMap<TradeId, Vec<Sale>> = HashMap::new();
    for row in &rows {
        sales
            .entry(row.try_get("purchase_trade_id")?)
            .or_default()
            .push(Sale {
                sale: row.try_get("sale_trade_id")?,
                units: UnitsAt::new(
                    parse_dec("quantity_allocated", row.try_get("quantity_allocated")?)?,
                    row.try_get("date")?,
                ),
                settled: row.try_get("settlement_date")?,
            });
    }
    Ok(sales)
}

/// The unit scale of `account`'s holding of `listing_id`, read once per pass.
async fn scale_of(
    conn: &mut sqlx::SqliteConnection,
    scales: &mut HashMap<HoldingAccountId, Option<u32>>,
    account: HoldingAccountId,
    listing_id: ListingId,
) -> Result<Option<u32>, sqlx::Error> {
    if let Some(scale) = scales.get(&account) {
        return Ok(*scale);
    }
    let scale = holding_unit_scale::db_unit_scale(&mut *conn, account, listing_id).await?;
    scales.insert(account, scale);
    Ok(scale)
}

/// The "units received" overrides of the listing's actions, by (action,
/// account).
pub(crate) async fn db_overrides(
    conn: &mut sqlx::SqliteConnection,
    listing_id: ListingId,
) -> Result<HashMap<(i64, HoldingAccountId), Decimal>, sqlx::Error> {
    let rows = sqlx::query(
        "SELECT o.corporate_action_id, o.holding_account_id, o.units_received \
         FROM entitlement_overrides o \
         JOIN corporate_actions a ON a.id = o.corporate_action_id \
         WHERE a.listing_id = ?",
    )
    .bind(listing_id)
    .fetch_all(&mut *conn)
    .await?;
    rows.iter()
        .map(|row| {
            Ok((
                (
                    row.try_get("corporate_action_id")?,
                    row.try_get("holding_account_id")?,
                ),
                parse_dec("units_received", row.try_get("units_received")?)?,
            ))
        })
        .collect()
}

/// The counts stored for the listing's actions by the last recompute, by
/// (action, parcel): the priors the stable distribution keeps.
async fn db_priors(
    conn: &mut sqlx::SqliteConnection,
    listing_id: ListingId,
) -> Result<HashMap<(i64, TradeId), Prior>, sqlx::Error> {
    let rows = sqlx::query(
        "SELECT c.corporate_action_id, c.trade_id, c.units_before, c.units_after, \
                c.in_lieu_units \
         FROM corporate_action_parcel_counts c \
         JOIN corporate_actions a ON a.id = c.corporate_action_id \
         WHERE a.listing_id = ? \
           AND (a.action_type = 'ShareSplit' OR a.action_type = 'BonusIssue')",
    )
    .bind(listing_id)
    .fetch_all(&mut *conn)
    .await?;
    rows.iter()
        .map(|row| {
            let after = parse_dec("units_after", row.try_get("units_after")?)?;
            let in_lieu = parse_dec("in_lieu_units", row.try_get("in_lieu_units")?)?;
            Ok((
                (
                    row.try_get("corporate_action_id")?,
                    row.try_get("trade_id")?,
                ),
                Prior {
                    before: parse_dec("units_before", row.try_get("units_before")?)?,
                    on_grid: after - in_lieu,
                },
            ))
        })
        .collect()
}
