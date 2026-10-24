//! The stored per-parcel counts of a listing's rounded share splits and bonus
//! issues (`corporate_action_parcel_counts`, migration 0055): computed per
//! holding by `domain::entitlement_rounding` and written inside the write
//! transaction of whatever changed them — the action itself (its ratio, rule,
//! date or listing; a new action; a deletion) or a "units received" override
//! ([`crate::entities::entitlement_override`]).
//!
//! The whole listing is recomputed in date order, because each action's
//! "units before" are re-based through every earlier one — an earlier
//! rounded action by the counts this pass has just stored for it. The rows
//! are then reconciled with what is stored: a row that would not change is
//! not rewritten, so recomputing after an unrelated edit writes nothing.
//!
//! The units an action converts are those its own re-base converts: the
//! parcels acquired (traded) before its date, less the sales dated before it
//! — a trade dated on or after it is stated in the post-action basis, which is
//! the convention every quantity re-base in the application keys on
//! (`domain::adjustments::split_ratio`). A share split or bonus issue carries
//! no record date of its own here, and its `date` is what decides which basis
//! a trade's quantity is in, so that is also what decides which units the
//! registry rounded; the trades it counted are frozen while it stands (0055's
//! guard triggers).

use crate::domain::adjustments::{
    AcquiredUnits, SplitEvent, StoredCount, UnitsAt, sold_in_acquired_units,
    split_adjusted_quantity,
};
use crate::domain::entitlement_rounding::{self, ParcelBefore, ParcelCount, RoundingError};
use crate::domain::open_parcels;
use crate::entities::corporate_action::model::FractionRule;
use crate::entities::holding_unit_scale;
use crate::infra::decimal::{Money, parse_dec};
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
                 small to give every parcel a unit on its grid, and a parcel with no units would \
                 strand its cost base where no sale can reach it. Record the action with \
                 fraction_rule Exact, or a units-received override for that account, instead"
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
    event: SplitEvent,
    rule: FractionRule,
}

/// One Buy/DRP parcel of the listing.
struct Parcel {
    id: TradeId,
    date: NaiveDate,
    quantity: AcquiredUnits,
    account: HoldingAccountId,
}

/// Recompute and store the per-parcel counts of every rounded share split and
/// bonus issue of `listing_id`, on the caller's write transaction.
pub async fn db_recompute_listing(
    conn: &mut sqlx::SqliteConnection,
    listing_id: ListingId,
) -> Result<(), RoundError> {
    let action_rows = sqlx::query(
        "SELECT id, action_type, date, split_new_units, split_old_units, bonus_units, \
                bonus_held_units, fraction_rule FROM corporate_actions \
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
            event,
            rule,
        });
    }

    // What should be stored, by (action, parcel).
    let mut desired: BTreeMap<(i64, TradeId), ParcelCount> = BTreeMap::new();
    if actions.iter().any(|a| a.rule.rounding().is_some()) {
        let parcels = db_parcels(&mut *conn, listing_id).await?;
        let sales = open_parcels::db_sales_by_parcel(&mut *conn, Some(listing_id)).await?;
        let overrides = db_overrides(&mut *conn, listing_id).await?;
        let mut scales: HashMap<HoldingAccountId, Option<u32>> = HashMap::new();
        // Each rounded action's stored counts, as computed so far in this
        // pass — what re-bases a parcel across it for the actions after it.
        let mut computed: HashMap<i64, HashMap<TradeId, StoredCount>> = HashMap::new();

        for (index, action) in actions.iter().enumerate() {
            let Some(rounding) = action.rule.rounding() else {
                continue;
            };
            // Each account's parcels as they stood immediately before the
            // action, in the basis then in force.
            let mut holdings: BTreeMap<HoldingAccountId, Vec<ParcelBefore>> = BTreeMap::new();
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
                let sold_before: Vec<UnitsAt> = sales
                    .get(&parcel.id)
                    .map_or(&[][..], |v| v)
                    .iter()
                    .copied()
                    .filter(|s| s.basis().is_some_and(|d| d < action.date))
                    .collect();
                let remaining = parcel.quantity
                    - sold_in_acquired_units(parcel.quantity, sold_before, &prior, parcel.date);
                if remaining <= AcquiredUnits::ZERO {
                    continue;
                }
                let units =
                    split_adjusted_quantity(remaining, &prior, parcel.date, None).quantity();
                holdings
                    .entry(parcel.account)
                    .or_default()
                    .push(ParcelBefore {
                        trade_id: parcel.id,
                        acquired: parcel.date,
                        units,
                    });
            }

            let mut stored = HashMap::new();
            for (account, before) in holdings {
                let scale = match scales.get(&account) {
                    Some(scale) => *scale,
                    None => {
                        let scale =
                            holding_unit_scale::db_unit_scale(&mut *conn, account, listing_id)
                                .await?;
                        scales.insert(account, scale);
                        scale
                    }
                };
                // An unconstrained holding has no grid to round onto: the
                // registry holds what the ratio produces, as for `Exact`.
                let Some(scale) = scale else {
                    continue;
                };
                let counts = entitlement_rounding::round_holding(
                    &before,
                    action.event.new_units,
                    action.event.old_units,
                    scale,
                    rounding,
                    overrides.get(&(action.id, account)).copied(),
                )
                .map_err(|source| RoundError::Holding {
                    action_id: action.id,
                    account,
                    source,
                })?;
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
    /// Its holding account: each account is one holding, rounded once.
    pub account: HoldingAccountId,
    /// Its units at the operation date.
    pub units: Decimal,
}

/// The replacement count each source parcel's holding is rounded to by a
/// scrip-for-scrip exchange or demerger of `action_id` — its units of
/// `destination` (the replacement or demerged listing) on that listing's grid
/// in the account, by `rule`, or a units-received override — keyed by source
/// parcel ([`ParcelCount::trade_id`] is the source). A parcel in a holding
/// with no grid, or under an `Exact` rule, is absent: its replacement keeps
/// the exact ratio's count.
pub async fn db_round_replacements(
    conn: &mut sqlx::SqliteConnection,
    action_id: i64,
    rule: FractionRule,
    destination: ListingId,
    (new, old): (Decimal, Decimal),
    parcels: &[Converted],
) -> Result<HashMap<TradeId, ParcelCount>, RoundError> {
    let Some(rounding) = rule.rounding() else {
        return Ok(HashMap::new());
    };
    let mut holdings: BTreeMap<HoldingAccountId, Vec<ParcelBefore>> = BTreeMap::new();
    for p in parcels.iter().filter(|p| p.units > Decimal::ZERO) {
        holdings.entry(p.account).or_default().push(ParcelBefore {
            trade_id: p.parcel,
            acquired: p.acquired,
            units: p.units,
        });
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
    let mut out = HashMap::new();
    for (account, before) in holdings {
        let Some(scale) =
            holding_unit_scale::db_unit_scale(&mut *conn, account, destination).await?
        else {
            continue;
        };
        let counts = entitlement_rounding::round_holding(
            &before,
            new,
            old,
            scale,
            rounding,
            overrides.get(&account).copied(),
        )
        .map_err(|source| RoundError::Holding {
            action_id,
            account,
            source,
        })?;
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
        "SELECT id, date, quantity, holding_account_id FROM trades \
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
                quantity: AcquiredUnits::new(parse_dec("quantity", row.try_get("quantity")?)?),
                account: row.try_get("holding_account_id")?,
            })
        })
        .collect()
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
