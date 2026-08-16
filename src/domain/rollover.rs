//! The machinery the three parcel-substituting operations share: the
//! scrip-for-scrip exchange (`entities::scrip_exchange`), the demerger
//! (`entities::demerger`) and the inter-account transfer
//! (`entities::transfer`).
//!
//! All three do the same four things in one transaction — validate, walk the
//! source listing's open parcels costing each one, write a closing Sell that
//! consumes them, and write the replacement Buys that carry their cost base
//! and acquisition date forward. Only what happens *between* the walk and the
//! writes differs (the rollover apportionment, the demerger percentage, the
//! moved quantity), so everything else lives here rather than in three copies
//! that a fix to (say) the split/ROC re-basing would have to land in
//! separately.
//!
//! This is deliberately *not* built on [`domain::open_parcels`](super::open_parcels),
//! which every open-holdings *report* is: that loader converts each parcel's
//! cost base to AUD for reporting, whereas a replacement parcel must carry
//! its cost base forward in the original parcel's own currency (the AUD
//! translation then happens later, at the *deemed* acquisition month, when a
//! report reads the replacement). What the two do share is the pipeline
//! underneath — [`cost_base::adjusted_cost_base`] — which neither
//! re-implements.

use std::collections::HashMap;

use chrono::NaiveDate;
use rust_decimal::Decimal;
use sqlx::Row;

use crate::domain::cost_base::{self, ParcelRow};
use crate::entities::corporate_action::{
    self, RocEvent, SplitEvent, sold_in_acquired_units, split_adjusted_quantity,
};
use crate::entities::sell::{AllocationInput, SellBody};
use crate::entities::trade::{self, Trade};
use crate::infra::decimal::{Money, OptMoney, parse_dec};

/// The reference data one listing's parcels are costed against: splits re-base
/// units, AMIT adjustments (CGT event E10) and return-of-capital payments (G1)
/// reduce the carried cost base.
pub struct CostBaseInputs {
    pub splits: Vec<SplitEvent>,
    roc_events: Vec<RocEvent>,
    amit_events: HashMap<i64, Vec<cost_base::AmitReductionEvent>>,
}

impl CostBaseInputs {
    /// Reads all three on the caller's transaction, so the operation's checks
    /// and writes see one consistent snapshot.
    pub async fn load(
        conn: &mut sqlx::SqliteConnection,
        listing_id: i64,
    ) -> Result<Self, sqlx::Error> {
        let splits = corporate_action::db_splits_for_listing(&mut *conn, listing_id).await?;
        let roc_events = corporate_action::db_return_of_capital_events(&mut *conn)
            .await?
            .remove(&listing_id)
            .unwrap_or_default();
        let amit_events =
            crate::entities::amit_adjustment::db_cost_base_reduction_events(&mut *conn, None)
                .await?;
        Ok(Self {
            splits,
            roc_events,
            amit_events,
        })
    }

    /// The remaining reduced cost base of `units` (in the parcel's
    /// *as-acquired* basis) of `parcel`, in the parcel's own currency, as at
    /// `up_to` — the shared pipeline, bounded so an adjustment dated after the
    /// operation does not reach it.
    pub fn carried_cost_base(
        &self,
        parcel: &ParcelRow,
        units: Decimal,
        up_to: NaiveDate,
    ) -> Result<Decimal, sqlx::Error> {
        Ok(cost_base::adjusted_cost_base(
            &parcel.parcel(),
            units,
            self.amit_events.get(&parcel.id).map_or(&[][..], |v| v),
            &self.roc_events,
            &self.splits,
            // These are the units still *open* at the operation date — the
            // ones a statement for a year ending on or before it covered.
            cost_base::Held::AsAt(Some(up_to)),
        )?
        .adjusted)
    }

    /// Every parcel of `listing_id` with units still open, in date order.
    /// Fully consumed parcels are dropped, so an empty result means nothing is
    /// held (the operations' `NothingHeld` rejection).
    pub async fn open_parcels(
        &self,
        conn: &mut sqlx::SqliteConnection,
        listing_id: i64,
        as_of: NaiveDate,
    ) -> Result<Vec<RolledParcel>, sqlx::Error> {
        let parcel_rows: Vec<ParcelRow> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
            "SELECT {} FROM trades \
             WHERE listing_id = ? AND trade_type IN ('Buy', 'DRP') ORDER BY date, id",
            ParcelRow::COLUMNS
        )))
        .bind(listing_id)
        .fetch_all(&mut *conn)
        .await?;

        let alloc_rows = sqlx::query(
            "SELECT pa.purchase_trade_id, pa.quantity_allocated, s.date AS sale_date \
             FROM parcel_allocations pa \
             JOIN trades s ON s.id = pa.sale_trade_id \
             JOIN trades p ON p.id = pa.purchase_trade_id \
             WHERE p.listing_id = ?",
        )
        .bind(listing_id)
        .fetch_all(&mut *conn)
        .await?;
        let mut qty_sold: HashMap<i64, Vec<(NaiveDate, Decimal)>> = HashMap::new();
        for row in &alloc_rows {
            let tid: i64 = row.try_get("purchase_trade_id")?;
            qty_sold.entry(tid).or_default().push((
                row.try_get("sale_date")?,
                parse_dec("quantity_allocated", row.try_get("quantity_allocated")?)?,
            ));
        }

        let mut open = Vec::with_capacity(parcel_rows.len());
        for parcel in parcel_rows {
            // Cost-base arithmetic stays in as-acquired units; each sale's
            // allocated quantity is re-based back across any splits between
            // acquisition and that sale.
            let sold = sold_in_acquired_units(
                qty_sold.get(&parcel.id).map_or(&[][..], |v| v),
                &self.splits,
                parcel.date,
            );
            let remaining = parcel.quantity - sold;
            if remaining <= Decimal::ZERO {
                continue;
            }
            // A ratio (exchange, entitlement) applies to units as held on the
            // operation's date, so both bases are carried.
            let at_date_units =
                split_adjusted_quantity(remaining, &self.splits, parcel.date, Some(as_of));
            open.push(RolledParcel {
                parcel,
                remaining,
                at_date_units,
            });
        }
        Ok(open)
    }
}

/// One open parcel a rollover is about to substitute.
pub struct RolledParcel {
    pub parcel: ParcelRow,
    /// Units still open, in the parcel's as-acquired basis — what
    /// [`CostBaseInputs::carried_cost_base`] costs.
    pub remaining: Decimal,
    /// The same units in the operation date's basis — what the closing Sell
    /// consumes and what a ratio scales.
    pub at_date_units: Decimal,
}

/// The closing Sell every rollover writes: it consumes the substituted
/// parcels, carries no brokerage, and settles on the operation's own date
/// (nothing market-settles). Proceeds are zero unless the operation pays cash
/// — an all-scrip exchange, a demerger and a transfer are not disposals, and
/// the provenance column the caller passes to
/// [`sell::upsert_sell_in_tx`](crate::entities::sell::upsert_sell_in_tx) is
/// what keeps the Sell out of the gains reports.
pub fn closing_sell_body(
    date: NaiveDate,
    listing_id: i64,
    holding_account_id: i64,
    average_price: Decimal,
    currency: String,
    fx_rate: Decimal,
    allocations: Vec<AllocationInput>,
) -> SellBody {
    SellBody {
        brokerage_includes_gst: false,
        statement_total: None,
        holding_account_id,
        date,
        settlement_date: Some(date),
        listing_id,
        average_price,
        quantity: allocations.iter().map(|a| a.quantity_allocated).sum(),
        brokerage: Decimal::ZERO,
        gst_on_brokerage: Decimal::ZERO,
        brokerage_currency: currency.clone(),
        currency,
        fx_rate,
        spot_fx_rate: None,
        contract_note_ref: None,
        allocations,
    }
}

/// Which operation created a replacement Buy: the trades column linking it
/// back to the row that did, so the group can be found, frozen and deleted as
/// one.
#[derive(Debug, Clone, Copy)]
pub enum Provenance {
    ScripAction(i64),
    DemergerAction(i64),
    Transfer(i64),
}

impl Provenance {
    fn column(self) -> &'static str {
        match self {
            Provenance::ScripAction(_) => "scrip_action_id",
            Provenance::DemergerAction(_) => "demerger_action_id",
            Provenance::Transfer(_) => "transfer_id",
        }
    }

    fn id(self) -> i64 {
        match self {
            Provenance::ScripAction(id)
            | Provenance::DemergerAction(id)
            | Provenance::Transfer(id) => id,
        }
    }
}

/// A replacement Buy to write: the substituted parcel's units and the cost
/// base they carry forward.
pub struct ReplacementBuy<'a> {
    pub id: i64,
    pub date: NaiveDate,
    pub listing_id: i64,
    pub quantity: Decimal,
    /// The carried cost base, in the original parcel's currency.
    pub cost_base: Decimal,
    pub currency: &'a str,
    pub fx_rate: Decimal,
    pub spot_fx_rate: Option<Decimal>,
    /// The original parcel's (possibly already deemed) acquisition date: the
    /// discount clock and the cost base's AUD translation month both run from
    /// it, so a rollover chain always dates back to the first acquisition.
    pub deemed_acquisition_date: NaiveDate,
    pub holding_account_id: i64,
}

/// Writes one replacement Buy. The carried cost base goes on the `brokerage`
/// column with a zero price — numerically part of the one cost base
/// everywhere, with no division — and the row carries `provenance` so it can
/// be recognised as part of the operation's group.
pub async fn insert_replacement_buy(
    conn: &mut sqlx::SqliteConnection,
    buy: &ReplacementBuy<'_>,
    provenance: Provenance,
) -> Result<(), sqlx::Error> {
    sqlx::query(sqlx::AssertSqlSafe(format!(
        "INSERT INTO trades \
         (id, trade_type, date, settlement_date, listing_id, average_price, quantity, \
          currency, brokerage, gst_on_brokerage, brokerage_currency, fx_rate, \
          spot_fx_rate, deemed_acquisition_date, holding_account_id, {}) \
         VALUES (?, 'Buy', ?, ?, ?, '0', ?, ?, ?, '0', ?, ?, ?, ?, ?, ?)",
        provenance.column()
    )))
    .bind(buy.id)
    .bind(buy.date)
    .bind(buy.date)
    .bind(buy.listing_id)
    .bind(Money(buy.quantity))
    .bind(buy.currency)
    .bind(Money(buy.cost_base))
    .bind(buy.currency)
    .bind(Money(buy.fx_rate))
    .bind(OptMoney(buy.spot_fx_rate))
    .bind(buy.deemed_acquisition_date)
    .bind(buy.holding_account_id)
    .bind(provenance.id())
    .execute(&mut *conn)
    .await?;
    Ok(())
}

/// Reads freshly created trades back, in the order written, so an operation's
/// response is exactly what was stored. A missing row is a bug, not a
/// not-found: the ids were just inserted in the committed transaction.
pub async fn created_trades(
    pool: &sqlx::SqlitePool,
    ids: impl IntoIterator<Item = i64>,
) -> Result<Vec<Trade>, sqlx::Error> {
    let mut out = Vec::new();
    for id in ids {
        out.push(
            trade::db_get(pool, id)
                .await?
                .ok_or(sqlx::Error::RowNotFound)?,
        );
    }
    Ok(out)
}
