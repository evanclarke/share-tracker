//! The one INSERT behind every acquisition parcel an **operation** writes —
//! as opposed to one entered through `PUT /trades/:id`, which is
//! `db::db_upsert`'s.
//!
//! Six paths construct a Buy (or a DRP) on the taxpayer's behalf: the
//! scrip-for-scrip exchange, demerger and holding-account transfer replacement
//! Buys (`entities::rollover::insert_replacement_buy`), the ESS vest, the rights
//! exercise, the inherited parcel and the DRP reinvestment. They differ in
//! which figures they carry and which record they point back to, and agree on
//! everything else — the row is dated and settled on one stated date, carries
//! no GST or contract note, and states its brokerage in its own currency. Each
//! used to spell that agreement out as its own `INSERT INTO trades`, so a new
//! `trades` column meant finding all of them. [`NewParcel`] states it once,
//! and [`ParcelOrigin`] makes "which provenance column does this row carry" a
//! type rather than a column name typed at each call site.

use crate::infra::db;
use crate::infra::decimal::{Money, OptMoney};
use crate::infra::id::HoldingAccountId;
use crate::infra::id::ListingId;
use crate::infra::id::TradeId;
use chrono::NaiveDate;
use rust_decimal::Decimal;

/// Which operation wrote a parcel, and so which provenance link its row
/// carries (see `provenance::TRADE_PROVENANCE`). The parcel-writing half of
/// that list — the closing-Sell half is `sell::SellProvenance`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ParcelOrigin {
    /// The cost-base-reset Buy of a vested ESS statement (`ess_statement_id`).
    EssVest(i64),
    /// The inherited parcel of an inheritance (`inheritance_id`).
    Inheritance(i64),
    /// The Buy a rights exercise creates (`rights_action_id`).
    RightsExercise(i64),
    /// A scrip-for-scrip exchange's replacement Buy (`scrip_action_id`).
    ScripExchange(i64),
    /// A demerger's replacement Buy (`demerger_action_id`).
    Demerger(i64),
    /// A holding-account transfer's replacement Buy (`transfer_id`).
    Transfer(i64),
    /// A DRP reinvestment. It carries **no** column of its own: the link runs
    /// the other way, from the funding distribution's
    /// `income.reinvestment_trade_id`, and the row is a `DRP` rather than a
    /// `Buy` — which is what says which kind of parcel it is.
    DrpReinvestment,
}

impl ParcelOrigin {
    /// The `trades` provenance column this origin sets, and the id it holds.
    fn link(self) -> Option<(&'static str, i64)> {
        match self {
            ParcelOrigin::EssVest(id) => Some(("ess_statement_id", id)),
            ParcelOrigin::Inheritance(id) => Some(("inheritance_id", id)),
            ParcelOrigin::RightsExercise(id) => Some(("rights_action_id", id)),
            ParcelOrigin::ScripExchange(id) => Some(("scrip_action_id", id)),
            ParcelOrigin::Demerger(id) => Some(("demerger_action_id", id)),
            ParcelOrigin::Transfer(id) => Some(("transfer_id", id)),
            ParcelOrigin::DrpReinvestment => None,
        }
    }

    fn trade_type(self) -> super::TradeType {
        match self {
            ParcelOrigin::DrpReinvestment => super::TradeType::DRP,
            ParcelOrigin::EssVest(_)
            | ParcelOrigin::Inheritance(_)
            | ParcelOrigin::RightsExercise(_)
            | ParcelOrigin::ScripExchange(_)
            | ParcelOrigin::Demerger(_)
            | ParcelOrigin::Transfer(_) => super::TradeType::Buy,
        }
    }
}

/// The provenance columns a [`NewParcel`] binds, in the INSERT's order: its
/// origin's id in its own column, NULL in the rest. A parcel never carries
/// a Sell-only link (`buyback_action_id`, `worthless_action_id`).
const PARCEL_LINK_COLUMNS: [&str; 6] = [
    "ess_statement_id",
    "inheritance_id",
    "rights_action_id",
    "scrip_action_id",
    "demerger_action_id",
    "transfer_id",
];

/// A DRP's cash residual columns (see `entities::drp_enrolment`); zero for
/// every other parcel.
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct Residuals {
    pub brought_forward: Decimal,
    pub carried_forward: Decimal,
    pub paid_out: Decimal,
}

/// An operation-written acquisition parcel. [`NewParcel::new`] takes what
/// every one states; the rest default to the plain parcel — price, brokerage
/// and residuals zero, `fx_rate` parity, no spot rate, no deemed acquisition
/// date — and are set by the builder methods each operation needs.
#[derive(Debug, Clone)]
pub(crate) struct NewParcel<'a> {
    origin: ParcelOrigin,
    date: NaiveDate,
    listing_id: ListingId,
    holding_account_id: HoldingAccountId,
    quantity: Decimal,
    currency: &'a str,
    average_price: Decimal,
    brokerage: Decimal,
    fx_rate: Decimal,
    spot_fx_rate: Option<Decimal>,
    deemed_acquisition_date: Option<NaiveDate>,
    residuals: Residuals,
}

impl<'a> NewParcel<'a> {
    /// `quantity` units of `listing_id` acquired on `date` into
    /// `holding_account_id`, priced in `currency`. The date is the settlement
    /// date too (`stated`): none of these parcels is market-settled T+n.
    pub fn new(
        origin: ParcelOrigin,
        date: NaiveDate,
        listing_id: ListingId,
        holding_account_id: HoldingAccountId,
        quantity: Decimal,
        currency: &'a str,
    ) -> Self {
        NewParcel {
            origin,
            date,
            listing_id,
            holding_account_id,
            quantity,
            currency,
            average_price: Decimal::ZERO,
            brokerage: Decimal::ZERO,
            fx_rate: Decimal::ONE,
            spot_fx_rate: None,
            deemed_acquisition_date: None,
            residuals: Residuals::default(),
        }
    }

    /// The price per unit.
    pub fn price(mut self, average_price: Decimal) -> Self {
        self.average_price = average_price;
        self
    }

    /// The amount on the `brokerage` column — an incidental cost, or a whole
    /// carried cost base on a zero-price parcel (a rollover replacement, an
    /// inheritance), which is numerically part of the one cost base everywhere
    /// with no division.
    pub fn brokerage(mut self, brokerage: Decimal) -> Self {
        self.brokerage = brokerage;
        self
    }

    /// The fallback rate `infra::fx::pick_rate` applies when the month has no
    /// ATO rate.
    pub fn fx_rate(mut self, fx_rate: Decimal) -> Self {
        self.fx_rate = fx_rate;
        self
    }

    /// A deliberate spot-rate override carried over from the parcel replaced.
    pub fn spot_fx_rate(mut self, spot_fx_rate: Option<Decimal>) -> Self {
        self.spot_fx_rate = spot_fx_rate;
        self
    }

    /// The date the CGT discount clock and the cost base's AUD month run from,
    /// where it is not `date` itself (`None` says it is).
    pub fn deemed_acquisition_date(mut self, date: Option<NaiveDate>) -> Self {
        self.deemed_acquisition_date = date;
        self
    }

    pub fn residuals(mut self, residuals: Residuals) -> Self {
        self.residuals = residuals;
        self
    }

    /// Write the parcel as a new row and answer the id the database gave it —
    /// one its `AUTOINCREMENT` sequence has never issued, so a caller writing
    /// several takes each id from its own call, never by adding to the last.
    pub async fn insert(&self, conn: &mut sqlx::SqliteConnection) -> Result<TradeId, sqlx::Error> {
        self.write(conn, None).await
    }

    /// Rewrite the parcel stored at `id` in place — the inheritance's edit,
    /// which keeps its Buy's id — or, with `None`, insert it as [`insert`]
    /// does. Every column the parcel states is rewritten.
    ///
    /// [`insert`]: NewParcel::insert
    pub async fn write(
        &self,
        conn: &mut sqlx::SqliteConnection,
        id: Option<TradeId>,
    ) -> Result<TradeId, sqlx::Error> {
        let link = self.origin.link();
        let mut query = db::insert_with_optional_id(INSERT_SQL, id.map(|id| id.0))
            .bind(self.origin.trade_type())
            .bind(self.date)
            .bind(self.date)
            .bind(self.listing_id)
            .bind(Money(self.average_price))
            .bind(Money(self.quantity))
            .bind(self.currency)
            .bind(Money(self.brokerage))
            .bind(self.currency)
            .bind(Money(self.fx_rate))
            .bind(OptMoney(self.spot_fx_rate))
            .bind(self.deemed_acquisition_date)
            .bind(Money(self.residuals.brought_forward))
            .bind(Money(self.residuals.carried_forward))
            .bind(Money(self.residuals.paid_out))
            .bind(self.holding_account_id);
        for column in PARCEL_LINK_COLUMNS {
            query = query.bind(link.filter(|(c, _)| *c == column).map(|(_, id)| id));
        }
        let result = query.execute(&mut *conn).await?;
        Ok(id.unwrap_or_else(|| TradeId(result.last_insert_rowid())))
    }
}

const INSERT_SQL: &str = "INSERT INTO trades \
     (id, trade_type, date, settlement_date, settlement_date_source, listing_id, \
      average_price, quantity, currency, brokerage, gst_on_brokerage, brokerage_currency, \
      fx_rate, spot_fx_rate, deemed_acquisition_date, \
      residual_brought_forward, residual_carried_forward, residual_paid_out, \
      holding_account_id, \
      ess_statement_id, inheritance_id, rights_action_id, scrip_action_id, \
      demerger_action_id, transfer_id) \
     VALUES (?, ?, ?, ?, 'stated', ?, ?, ?, ?, ?, '0', ?, ?, ?, ?, ?, ?, ?, ?, \
             ?, ?, ?, ?, ?, ?) \
     ON CONFLICT(id) DO UPDATE SET \
         trade_type               = excluded.trade_type, \
         date                     = excluded.date, \
         settlement_date          = excluded.settlement_date, \
         settlement_date_source   = excluded.settlement_date_source, \
         listing_id               = excluded.listing_id, \
         average_price            = excluded.average_price, \
         quantity                 = excluded.quantity, \
         currency                 = excluded.currency, \
         brokerage                = excluded.brokerage, \
         gst_on_brokerage         = excluded.gst_on_brokerage, \
         brokerage_currency       = excluded.brokerage_currency, \
         fx_rate                  = excluded.fx_rate, \
         spot_fx_rate             = excluded.spot_fx_rate, \
         deemed_acquisition_date  = excluded.deemed_acquisition_date, \
         residual_brought_forward = excluded.residual_brought_forward, \
         residual_carried_forward = excluded.residual_carried_forward, \
         residual_paid_out        = excluded.residual_paid_out, \
         holding_account_id       = excluded.holding_account_id, \
         ess_statement_id         = excluded.ess_statement_id, \
         inheritance_id           = excluded.inheritance_id, \
         rights_action_id         = excluded.rights_action_id, \
         scrip_action_id          = excluded.scrip_action_id, \
         demerger_action_id       = excluded.demerger_action_id, \
         transfer_id              = excluded.transfer_id";

#[cfg(test)]
mod tests {
    use super::*;
    use crate::entities::trade;
    use crate::entities::trade::TRADE_PROVENANCE;

    const ALL_ORIGINS: [ParcelOrigin; 7] = [
        ParcelOrigin::EssVest(1),
        ParcelOrigin::Inheritance(1),
        ParcelOrigin::RightsExercise(1),
        ParcelOrigin::ScripExchange(1),
        ParcelOrigin::Demerger(1),
        ParcelOrigin::Transfer(1),
        ParcelOrigin::DrpReinvestment,
    ];

    /// Every column an origin stamps is one the INSERT binds and one the
    /// provenance list classifies — so a parcel written here is recognised as
    /// operation-written by every reader of `TRADE_PROVENANCE` — and no two
    /// origins share a column.
    #[test]
    fn every_origin_link_is_a_bound_classified_provenance_column() {
        let mut seen = Vec::new();
        for origin in ALL_ORIGINS {
            let Some((column, _)) = origin.link() else {
                assert_eq!(origin, ParcelOrigin::DrpReinvestment);
                continue;
            };
            assert!(PARCEL_LINK_COLUMNS.contains(&column), "{column} unbound");
            assert!(
                TRADE_PROVENANCE.iter().any(|p| p.column == column),
                "{column} is not a classified provenance link"
            );
            assert!(!seen.contains(&column), "{column} stamped twice");
            seen.push(column);
        }
        assert_eq!(seen.len(), PARCEL_LINK_COLUMNS.len());
    }

    /// Each origin writes its own link and NULL in every other column, as the
    /// trade type its kind of parcel is.
    #[tokio::test]
    async fn insert_stamps_only_its_origins_column() {
        use crate::test_support::{dec, listing, test_pool, ymd};
        let pool = test_pool().await;
        listing(1).insert(&pool).await;
        // The links are foreign keys; the shape under test is the binding,
        // not the referenced rows.
        let mut conn = pool.acquire().await.unwrap();
        sqlx::query("PRAGMA foreign_keys = OFF")
            .execute(&mut *conn)
            .await
            .unwrap();
        for origin in ALL_ORIGINS {
            let id = NewParcel::new(
                origin,
                ymd(2024, 1, 2),
                ListingId(1),
                HoldingAccountId(1),
                dec("10"),
                "AUD",
            )
            .price(dec("1.5"))
            .insert(&mut conn)
            .await
            .unwrap();
            let trade = trade::db_get(&mut *conn, id).await.unwrap().unwrap();
            assert_eq!(trade.trade_type, origin.trade_type());
            assert_eq!(trade.settlement_date, ymd(2024, 1, 2));
            for column in PARCEL_LINK_COLUMNS {
                let value: Option<i64> = sqlx::query_scalar(sqlx::AssertSqlSafe(format!(
                    "SELECT {column} FROM trades WHERE id = ?"
                )))
                .bind(id)
                .fetch_one(&mut *conn)
                .await
                .unwrap();
                let expected = origin.link().filter(|(c, _)| *c == column).map(|_| 1);
                assert_eq!(value, expected, "{origin:?} {column}");
            }
        }
    }
}
