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
            "SELECT id, listing_id, date, quantity FROM trades \
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
                acquired,
                qty,
                sales,
            });
        }
        Ok(HeldTimeline { parcels })
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
