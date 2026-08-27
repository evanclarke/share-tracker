//! Held timeline: what was held, and when.
//!
//! Price collection is only interested in a listing on the days it was
//! actually held, so this walks the Buy/DRP parcels and their sale
//! allocations into a per-listing holding timeline the collection window and
//! the health report both ask questions of.

use crate::infra::decimal::parse_dec;
use chrono::{Duration, NaiveDate};
use rust_decimal::Decimal;
use sqlx::{Row, SqlitePool};
use std::collections::HashMap;

/// One purchase parcel's contribution to a listing's holding over time: `qty`
/// units from `acquired`, less the units each sale allocated out of it from
/// that sale's date.
struct ParcelHolding {
    /// Which account the parcel sits in. Only
    /// [`HeldTimeline::units_by_account_on`] reads it — every other question
    /// here is about the listing as a whole — but a holding *is* per account
    /// (an entitlement is paid to a registered holder, not to a security), so
    /// the split has to survive the load.
    holding_account_id: i64,
    acquired: NaiveDate,
    qty: Decimal,
    /// `(sale date, units sold)`, each already re-based to this parcel's
    /// as-acquired unit basis.
    sales: Vec<(NaiveDate, Decimal)>,
}

impl ParcelHolding {
    /// Units of this parcel still held on `date`, floored at nil: an
    /// over-allocated parcel must not net off another parcel's remaining
    /// units.
    fn remaining_on(&self, date: NaiveDate) -> Decimal {
        if self.acquired > date {
            return Decimal::ZERO;
        }
        let sold: Decimal = self
            .sales
            .iter()
            .filter(|(sale_date, _)| *sale_date <= date)
            .map(|(_, qty)| *qty)
            .sum();
        (self.qty - sold).max(Decimal::ZERO)
    }
}

/// Every purchase parcel and the sales out of it, loaded once: *the* in-memory
/// model of what was held and when. Decimal arithmetic in Rust, never float
/// SUM in SQL.
///
/// Each sale's `quantity_allocated` is expressed in the unit basis of its own
/// sale date, so it is re-based back to the parcel's as-acquired units
/// (`corporate_action::as_acquired_quantity`) as it is loaded — exactly as
/// `reports::portfolio::db_holdings_on` does. Without that, a split between a
/// Buy and a Sell makes this and the holdings reports disagree about whether
/// the listing is held at all, and snapshot generation then stores a silently
/// unvalued row (a holding the price map has no entry for) or blocks a date on
/// a security already fully sold.
///
/// Three queries answer any number of dates, so a caller walking years of
/// history ([`HeldTimeline::held_spans`], the health report's unpriced-day
/// check) never makes a per-day round trip.
pub struct HeldTimeline {
    /// Purchase parcels per listing; a listing appears exactly if it was ever
    /// bought, whether or not it is still held.
    parcels: HashMap<i64, Vec<ParcelHolding>>,
    /// The recorded share splits per listing, kept from the load so a holding
    /// can be expressed in the unit basis of a **past** date rather than only
    /// in each parcel's as-acquired one
    /// ([`HeldTimeline::units_by_account_on`]).
    splits: HashMap<i64, Vec<crate::entities::corporate_action::SplitEvent>>,
}

impl HeldTimeline {
    pub async fn load(pool: &SqlitePool) -> Result<Self, sqlx::Error> {
        let mut conn = pool.acquire().await?;
        HeldTimeline::load_on(&mut conn).await
    }

    /// [`HeldTimeline::load`] on the caller's own connection, so a write path
    /// can read the timeline **inside its own transaction** — snapshot
    /// generation does, so a trade committed between its reads and its store
    /// cannot be missed (SCENARIOS X-a).
    pub async fn load_on(conn: &mut sqlx::SqliteConnection) -> Result<Self, sqlx::Error> {
        let buys = sqlx::query(
            "SELECT id, listing_id, holding_account_id, date, quantity FROM trades \
             WHERE trade_type IN ('Buy', 'DRP')",
        )
        .fetch_all(&mut *conn)
        .await?;

        // sale-date-basis units allocated out of each purchase parcel
        let allocs = sqlx::query(
            "SELECT pa.purchase_trade_id, pa.quantity_allocated, s.date AS sale_date \
             FROM parcel_allocations pa JOIN trades s ON s.id = pa.sale_trade_id",
        )
        .fetch_all(&mut *conn)
        .await?;
        let mut qty_sold: HashMap<i64, Vec<(NaiveDate, Decimal)>> = HashMap::new();
        for row in &allocs {
            let trade_id: i64 = row.try_get("purchase_trade_id")?;
            qty_sold.entry(trade_id).or_default().push((
                row.try_get("sale_date")?,
                parse_dec("quantity_allocated", row.try_get("quantity_allocated")?)?,
            ));
        }

        let split_events =
            crate::entities::corporate_action::db_share_split_events(&mut *conn).await?;

        let mut parcels: HashMap<i64, Vec<ParcelHolding>> = HashMap::new();
        for row in &buys {
            let trade_id: i64 = row.try_get("id")?;
            let listing_id: i64 = row.try_get("listing_id")?;
            let acquired: NaiveDate = row.try_get("date")?;
            let qty = parse_dec("quantity", row.try_get("quantity")?)?;
            let splits = split_events.get(&listing_id).map_or(&[][..], |v| v);
            let sales = qty_sold
                .get(&trade_id)
                .map_or(&[][..], |v| v)
                .iter()
                .map(|&(sale_date, sold)| {
                    (
                        sale_date,
                        crate::entities::corporate_action::as_acquired_quantity(
                            sold, splits, acquired, sale_date,
                        ),
                    )
                })
                .collect();
            parcels.entry(listing_id).or_default().push(ParcelHolding {
                holding_account_id: row.try_get("holding_account_id")?,
                acquired,
                qty,
                sales,
            });
        }
        Ok(HeldTimeline {
            parcels,
            splits: split_events,
        })
    }

    /// Units of `listing_id` held **in each holding account** at the close of
    /// `held_on`, expressed in the unit basis in force at `unit_basis_at`,
    /// ascending by account; an account holding nothing is omitted.
    ///
    /// Per account because an entitlement is paid to a registered holder: the
    /// same listing held in two accounts pays two distributions and is entered
    /// as two income rows, so the distribution calendar's alerts
    /// (`reports::health`) have to ask the question one holder at a time.
    ///
    /// **The two dates are separate on purpose.** The holding is a fact about
    /// one day (for a distribution, the last cum-dividend day), but the number
    /// of units that holding *is* depends on which side of a split you ask
    /// from — and the figure it will be multiplied by has a basis of its own.
    /// A provider quotes a distribution per unit in the basis in force when it
    /// answered, not in the basis of the ex-date: Yahoo reports NVDA's
    /// pre-split dividends as 0.004 against a declared $0.04, restated
    /// cumulatively through every later split. Multiplying a figure in one
    /// basis by units in another is off by the split ratio; multiplying within
    /// one basis gives the same total dollars whichever basis that is, since
    /// the two scale inversely. So the caller names the basis its per-unit
    /// figure came in, and gets units to match.
    ///
    /// Pass `held_on` for both when the question is simply "what did the
    /// register say that day".
    pub fn units_by_account_on(
        &self,
        listing_id: i64,
        held_on: NaiveDate,
        unit_basis_at: NaiveDate,
    ) -> Vec<(i64, Decimal)> {
        let Some(parcels) = self.parcels.get(&listing_id) else {
            return Vec::new();
        };
        let splits = self.splits.get(&listing_id).map_or(&[][..], |v| v);
        let mut by_account: HashMap<i64, Decimal> = HashMap::new();
        for p in parcels {
            let remaining = p.remaining_on(held_on);
            if remaining <= Decimal::ZERO {
                continue;
            }
            *by_account.entry(p.holding_account_id).or_default() +=
                crate::entities::corporate_action::split_adjusted_quantity(
                    remaining,
                    splits,
                    p.acquired,
                    Some(unit_basis_at),
                );
        }
        let mut out: Vec<(i64, Decimal)> = by_account
            .into_iter()
            .filter(|(_, qty)| *qty > Decimal::ZERO)
            .collect();
        out.sort_by_key(|(account, _)| *account);
        out
    }

    /// Listings with a non-zero holding as at `as_of` (live holdings when
    /// `None`) — trades and sales dated after it don't count.
    pub fn held_listing_ids(&self, as_of: Option<NaiveDate>) -> Vec<i64> {
        let cutoff = crate::infra::date::as_of_or_open(as_of);
        let mut ids: Vec<i64> = self
            .parcels
            .iter()
            .filter(|(_, parcels)| {
                parcels
                    .iter()
                    .map(|p| p.remaining_on(cutoff))
                    .sum::<Decimal>()
                    > Decimal::ZERO
            })
            .map(|(listing_id, _)| *listing_id)
            .collect();
        ids.sort();
        ids
    }

    /// Every listing ever held, ascending — including ones since fully sold,
    /// whose held span is still history that needed pricing.
    pub fn listing_ids(&self) -> Vec<i64> {
        let mut ids: Vec<i64> = self.parcels.keys().copied().collect();
        ids.sort();
        ids
    }

    /// The listing's held spans as inclusive date ranges, ascending and
    /// non-adjacent, ending no later than `until`. A listing sold down to nil
    /// and later re-bought yields one span per holding period.
    ///
    /// A holding only changes on an acquisition or a sale date, so the
    /// quantity is evaluated at those dates alone and held constant in between
    /// — walking six years of calendar dates would be thousands of sums.
    pub fn held_spans(&self, listing_id: i64, until: NaiveDate) -> Vec<(NaiveDate, NaiveDate)> {
        let Some(parcels) = self.parcels.get(&listing_id) else {
            return Vec::new();
        };
        let mut events: Vec<NaiveDate> = parcels
            .iter()
            .flat_map(|p| std::iter::once(p.acquired).chain(p.sales.iter().map(|(date, _)| *date)))
            .filter(|date| *date <= until)
            .collect();
        events.sort();
        events.dedup();

        let mut spans: Vec<(NaiveDate, NaiveDate)> = Vec::new();
        for (i, &start) in events.iter().enumerate() {
            let held: Decimal = parcels.iter().map(|p| p.remaining_on(start)).sum();
            if held <= Decimal::ZERO {
                continue;
            }
            // The holding stands until the next event changes it, or to the
            // caller's bound when nothing else happens.
            let end = events
                .get(i + 1)
                .map_or(until, |next| *next - Duration::days(1));
            match spans.last_mut() {
                Some(last) if last.1 + Duration::days(1) == start => last.1 = end,
                _ => spans.push((start, end)),
            }
        }
        spans
    }
}

/// Listings with a non-zero holding. With `as_of` the holding is taken as at
/// that date — trades and sales dated after it don't count (snapshot
/// generation for a past date values what was held then, not what is held
/// now). A thin wrapper over [`HeldTimeline`], which documents the re-basing
/// rules; a caller asking about more than one date should load the timeline
/// once instead.
pub async fn db_held_listing_ids(
    pool: &SqlitePool,
    as_of: Option<NaiveDate>,
) -> Result<Vec<i64>, sqlx::Error> {
    let mut conn = pool.acquire().await?;
    db_held_listing_ids_on(&mut conn, as_of).await
}

/// [`db_held_listing_ids`] on the caller's own connection — the read half of
/// snapshot generation runs inside the transaction that stores the result
/// (SCENARIOS X-a).
pub async fn db_held_listing_ids_on(
    conn: &mut sqlx::SqliteConnection,
    as_of: Option<NaiveDate>,
) -> Result<Vec<i64>, sqlx::Error> {
    Ok(HeldTimeline::load_on(conn).await?.held_listing_ids(as_of))
}
