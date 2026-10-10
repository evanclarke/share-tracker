//! The write side of the three parcel-substituting operations — the
//! scrip-for-scrip exchange (`entities::scrip_exchange`), the demerger
//! (`entities::demerger`) and the inter-account transfer
//! (`entities::transfer`): the closing Sell body that consumes the
//! substituted parcels, the replacement Buy that carries each one's cost base
//! forward, and the read-back of what was written.
//!
//! The read side they share — costing each open parcel through the
//! cost-base pipeline, and the rollover-chain walks — is
//! [`domain::rollover`](crate::domain::rollover), re-exported here so an
//! operation reaches everything it shares through this one module. The split
//! follows the layering: `domain` reads and calculates and depends on nothing
//! in `entities`, while writing a Sell or a Buy is an entity's business.

use crate::infra::currency::CurrencyCode;
use crate::infra::id::HoldingAccountId;
use crate::infra::id::ListingId;
use chrono::NaiveDate;
use rust_decimal::Decimal;

pub use crate::domain::rollover::{CostBaseInputs, RolledParcel};
use crate::entities::sell::{AllocationInput, SellBody};
use crate::entities::trade::{self, Trade};

/// The closing Sell every rollover writes: it consumes the substituted
/// parcels, carries no brokerage, and settles on the operation's own date
/// (nothing market-settles). Proceeds are zero unless the operation pays cash
/// — an all-scrip exchange, a demerger and a transfer are not disposals, and
/// the provenance column the caller passes to
/// [`sell::upsert_sell_in_tx`](crate::entities::sell::upsert_sell_in_tx) is
/// what keeps the Sell out of the gains reports.
pub fn closing_sell_body(
    date: NaiveDate,
    listing_id: ListingId,
    holding_account_id: HoldingAccountId,
    average_price: Decimal,
    currency: CurrencyCode,
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
    fn origin(self) -> trade::ParcelOrigin {
        match self {
            Provenance::ScripAction(id) => trade::ParcelOrigin::ScripExchange(id),
            Provenance::DemergerAction(id) => trade::ParcelOrigin::Demerger(id),
            Provenance::Transfer(id) => trade::ParcelOrigin::Transfer(id),
        }
    }
}

/// A replacement Buy to write: the substituted parcel's units and the cost
/// base they carry forward. It carries no id — the database assigns one (see
/// [`insert_replacement_buy`]).
pub struct ReplacementBuy<'a> {
    pub date: NaiveDate,
    pub listing_id: ListingId,
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
    pub holding_account_id: HoldingAccountId,
}

/// Writes one replacement Buy and answers the id the database gave it. The
/// carried cost base goes on the `brokerage` column with a zero price —
/// numerically part of the one cost base everywhere, with no division — and
/// the row carries `provenance` so it can be recognised as part of the
/// operation's group. The row itself is `trade::NewParcel`'s, whose
/// database-assigned id is why a caller writing several rows takes each id
/// from this call, never by adding to the previous one (SCENARIOS U-a).
pub async fn insert_replacement_buy(
    conn: &mut sqlx::SqliteConnection,
    buy: &ReplacementBuy<'_>,
    provenance: Provenance,
) -> Result<i64, sqlx::Error> {
    trade::NewParcel::new(
        provenance.origin(),
        buy.date,
        buy.listing_id,
        buy.holding_account_id,
        buy.quantity,
        buy.currency,
    )
    .brokerage(buy.cost_base)
    .fx_rate(buy.fx_rate)
    .spot_fx_rate(buy.spot_fx_rate)
    .deemed_acquisition_date(Some(buy.deemed_acquisition_date))
    .insert(conn)
    .await
}

/// Reads freshly created trades back, in the order written, so an operation's
/// response is exactly what was stored. A missing row is a bug, not a
/// not-found: the ids were just inserted in the committed transaction.
pub async fn created_trades(
    conn: &mut sqlx::SqliteConnection,
    ids: impl IntoIterator<Item = i64>,
) -> Result<Vec<Trade>, sqlx::Error> {
    let mut out = Vec::new();
    for id in ids {
        out.push(
            trade::db_get(&mut *conn, id)
                .await?
                .ok_or(sqlx::Error::RowNotFound)?,
        );
    }
    Ok(out)
}
