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
    /// The operation's own date — the one bound every read here observes: an
    /// AMMA statement whose year end postdates it is not loaded at all (its
    /// adjustment arises at that year end, which has not happened yet as at
    /// the operation; the reduction reaches the *replacement* parcel later,
    /// through its own adjustment row), a return-of-capital payment after it
    /// reduces nothing, and [`Self::open_parcels`] reports units in this
    /// date's basis. Held once so the three cannot be given different dates.
    up_to: NaiveDate,
}

impl CostBaseInputs {
    /// Reads all three on the caller's transaction, so the operation's checks
    /// and writes see one consistent snapshot. `up_to` is the operation's
    /// date; every adjustment folded into the carried cost base is bounded by
    /// it (see the field doc).
    pub async fn load(
        conn: &mut sqlx::SqliteConnection,
        listing_id: i64,
        up_to: NaiveDate,
    ) -> Result<Self, sqlx::Error> {
        let splits = corporate_action::db_splits_for_listing(&mut *conn, listing_id).await?;
        let roc_events = corporate_action::db_return_of_capital_events(&mut *conn)
            .await?
            .remove(&listing_id)
            .unwrap_or_default();
        // Bounded to statements for years ending on or before the operation,
        // exactly as `domain::open_parcels::load` bounds its view of the same
        // events — so the carried cost base and the open-parcels reports can
        // never disagree about which statements have taken effect by a date.
        let amit_events = crate::entities::amit_adjustment::db_cost_base_reduction_events(
            &mut *conn,
            Some(up_to),
        )
        .await?;
        Ok(Self {
            splits,
            roc_events,
            amit_events,
            up_to,
        })
    }

    /// The remaining reduced cost base of `units` (in the parcel's
    /// *as-acquired* basis) of `parcel`, in the parcel's own currency, as at
    /// the operation date — the shared pipeline, bounded so an adjustment
    /// dated after the operation (an AMMA statement's year end, a
    /// return-of-capital payment) does not reach it.
    pub fn carried_cost_base(
        &self,
        parcel: &ParcelRow,
        units: Decimal,
    ) -> Result<Decimal, sqlx::Error> {
        Ok(cost_base::adjusted_cost_base(
            &parcel.parcel(),
            units,
            self.amit_events.get(&parcel.id).map_or(&[][..], |v| v),
            &self.roc_events,
            &self.splits,
            // These are the units still *open* at the operation date — the
            // ones a statement for a year ending on or before it covered.
            cost_base::Held::AsAt(Some(self.up_to)),
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
    ) -> Result<Vec<RolledParcel>, sqlx::Error> {
        let parcel_rows: Vec<ParcelRow> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
            "SELECT {} FROM trades \
             WHERE listing_id = ? AND trade_type IN ('Buy', 'DRP') ORDER BY date, id",
            ParcelRow::columns()
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
                qty_sold
                    .get(&parcel.id)
                    .map_or(&[][..], |v| v)
                    .iter()
                    .copied(),
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
                split_adjusted_quantity(remaining, &self.splits, parcel.date, Some(self.up_to));
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

/// How deep a chain of rollovers [`replacement_descendants`],
/// [`source_ancestors`] and `domain::cost_base::ParcelRow`'s register walk
/// will follow. A parcel transferred between accounts, caught in a takeover
/// and then demerged is three; ten is far past anything real and stops a cycle
/// (which the write paths cannot create) from looping. Shared so the three
/// walks cannot disagree about how far back a chain is read.
pub const MAX_ROLLOVER_DEPTH: usize = 10;

/// The provenance columns a replacement Buy and its closing Sell share, in the
/// order [`Provenance`] lists them. The chain walks are the only readers that
/// need all three at once — a specific operation always knows its own.
const PROVENANCE_COLUMNS: [&str; 3] = ["scrip_action_id", "demerger_action_id", "transfer_id"];

/// SQL predicate matching a trade (aliased `alias`) that carries **none** of
/// the rollover provenance columns — i.e. is not an operation's closing Sell
/// or replacement Buy. Built from [`PROVENANCE_COLUMNS`] so a fourth
/// operation kind extends every reader in one place rather than through a
/// hand-maintained list per query
/// (`domain::open_parcels::db_units_sold`'s disposals-only read is the
/// caller).
pub fn no_provenance_sql(alias: &str) -> String {
    PROVENANCE_COLUMNS
        .iter()
        .map(|c| format!("{alias}.{c} IS NULL"))
        .collect::<Vec<_>>()
        .join(" AND ")
}

/// The rollover groups a parcel's units were carried into: one entry per
/// (column, id) pair whose closing Sell consumed part of `parcel_id`.
async fn groups_consuming(
    conn: &mut sqlx::SqliteConnection,
    parcel_id: i64,
) -> Result<Vec<(&'static str, i64)>, sqlx::Error> {
    let mut out = Vec::new();
    for column in PROVENANCE_COLUMNS {
        let ids: Vec<i64> = sqlx::query_scalar(sqlx::AssertSqlSafe(format!(
            "SELECT DISTINCT s.{column} FROM parcel_allocations pa \
             JOIN trades s ON s.id = pa.sale_trade_id \
             WHERE pa.purchase_trade_id = ? AND s.{column} IS NOT NULL"
        )))
        .bind(parcel_id)
        .fetch_all(&mut *conn)
        .await?;
        out.extend(ids.into_iter().map(|id| (column, id)));
    }
    Ok(out)
}

/// The replacement parcels a rollover group created (its acquisition side).
async fn group_replacements(
    conn: &mut sqlx::SqliteConnection,
    column: &'static str,
    group_id: i64,
) -> Result<Vec<i64>, sqlx::Error> {
    // The column name comes from `PROVENANCE_COLUMNS`, never from user input.
    sqlx::query_scalar(sqlx::AssertSqlSafe(format!(
        "SELECT id FROM trades WHERE {column} = ? AND trade_type IN ('Buy', 'DRP') ORDER BY id"
    )))
    .bind(group_id)
    .fetch_all(&mut *conn)
    .await
}

/// The parcels a rollover group consumed (its disposal side).
async fn group_sources(
    conn: &mut sqlx::SqliteConnection,
    column: &'static str,
    group_id: i64,
) -> Result<Vec<i64>, sqlx::Error> {
    sqlx::query_scalar(sqlx::AssertSqlSafe(format!(
        "SELECT DISTINCT pa.purchase_trade_id FROM parcel_allocations pa \
         JOIN trades s ON s.id = pa.sale_trade_id \
         WHERE s.{column} = ? AND s.trade_type = 'Sell' ORDER BY pa.purchase_trade_id"
    )))
    .bind(group_id)
    .fetch_all(&mut *conn)
    .await
}

/// The rollover group that created `parcel_id`, if it is a replacement parcel.
/// At most one — a trade carries at most one provenance column.
async fn group_creating(
    conn: &mut sqlx::SqliteConnection,
    parcel_id: i64,
) -> Result<Option<(&'static str, i64)>, sqlx::Error> {
    let row = sqlx::query(
        "SELECT scrip_action_id, demerger_action_id, transfer_id FROM trades WHERE id = ?",
    )
    .bind(parcel_id)
    .fetch_optional(&mut *conn)
    .await?;
    let Some(row) = row else {
        return Ok(None);
    };
    for column in PROVENANCE_COLUMNS {
        if let Some(id) = row.try_get::<Option<i64>, _>(column)? {
            return Ok(Some((column, id)));
        }
    }
    Ok(None)
}

/// The mirror of [`replacement_descendants`]: every parcel whose units a
/// rollover carried into `parcel_id`, directly or through a chain of them,
/// ascending by id and excluding `parcel_id` itself. Empty when it is not a
/// replacement parcel.
///
/// Same coarseness, for the same reason — where one operation moved several
/// parcels at once, each of its replacements descends from all of that group's
/// sources. `entities::amit_adjustment` uses it to answer "could this parcel be
/// holding the units that statement's account held?", which is a reachability
/// question, not a claim about which source parcel became which replacement.
pub async fn source_ancestors(
    conn: &mut sqlx::SqliteConnection,
    parcel_id: i64,
) -> Result<Vec<i64>, sqlx::Error> {
    let mut seen = std::collections::HashSet::from([parcel_id]);
    let mut frontier = vec![parcel_id];
    let mut found = Vec::new();
    for _ in 0..MAX_ROLLOVER_DEPTH {
        let mut next = Vec::new();
        for id in frontier {
            let Some((column, group_id)) = group_creating(&mut *conn, id).await? else {
                continue;
            };
            for candidate in group_sources(&mut *conn, column, group_id).await? {
                if seen.insert(candidate) {
                    found.push(candidate);
                    next.push(candidate);
                }
            }
        }
        if next.is_empty() {
            break;
        }
        frontier = next;
    }
    found.sort_unstable();
    Ok(found)
}

/// Every parcel a rollover has carried `parcel_id`'s units into — directly, or
/// through a chain of them — ascending by id, with `parcel_id` itself excluded.
/// Empty when no rollover consumed it.
///
/// This is the forward half of the rollover chain, and it exists because a
/// replacement Buy carries no link to the individual parcel it replaced: the
/// only record is the group both sides share (`trades.transfer_id` /
/// `scrip_action_id` / `demerger_action_id`). So where one operation moved
/// several parcels at once, every replacement of that group is a descendant of
/// each of its sources — the walk cannot be finer than the data. Callers must
/// therefore read the result as "the parcels these units *could* now sit in",
/// which is what an advisory alert (`reports::health`'s `ess_30_day_rule`)
/// needs, and is why it does not claim a one-to-one substitution.
///
/// Bounded by [`MAX_ROLLOVER_DEPTH`] and by a visited set — a demerger's two
/// replacement parcels reconverging on one source would otherwise be counted
/// twice.
pub async fn replacement_descendants(
    conn: &mut sqlx::SqliteConnection,
    parcel_id: i64,
) -> Result<Vec<i64>, sqlx::Error> {
    let mut seen = std::collections::HashSet::from([parcel_id]);
    let mut frontier = vec![parcel_id];
    let mut found = Vec::new();
    for _ in 0..MAX_ROLLOVER_DEPTH {
        let mut next = Vec::new();
        for id in frontier {
            for (column, group_id) in groups_consuming(&mut *conn, id).await? {
                for candidate in group_replacements(&mut *conn, column, group_id).await? {
                    if seen.insert(candidate) {
                        found.push(candidate);
                        next.push(candidate);
                    }
                }
            }
        }
        if next.is_empty() {
            break;
        }
        frontier = next;
    }
    found.sort_unstable();
    Ok(found)
}

/// A replacement Buy to write: the substituted parcel's units and the cost
/// base they carry forward. It carries no id — the database assigns one (see
/// [`insert_replacement_buy`]).
pub struct ReplacementBuy<'a> {
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

/// Writes one replacement Buy and answers the id the database gave it. The
/// carried cost base goes on the `brokerage` column with a zero price —
/// numerically part of the one cost base everywhere, with no division — and
/// the row carries `provenance` so it can be recognised as part of the
/// operation's group.
///
/// The id column is **omitted**, so SQLite assigns the next id its
/// `AUTOINCREMENT` sequence has never issued. The operations used to compute
/// `MAX(id) + 1` themselves, which after a delete re-issues the freed id and
/// hands the new parcel the deleted trade's `row_history` trail (SCENARIOS
/// U-a) — and races any concurrent write besides. A caller writing several
/// rows must therefore take each id from this call, never by adding to the
/// previous one.
pub async fn insert_replacement_buy(
    conn: &mut sqlx::SqliteConnection,
    buy: &ReplacementBuy<'_>,
    provenance: Provenance,
) -> Result<i64, sqlx::Error> {
    let result = sqlx::query(sqlx::AssertSqlSafe(format!(
        "INSERT INTO trades \
         (trade_type, date, settlement_date, settlement_date_source, listing_id, \
          average_price, quantity, \
          currency, brokerage, gst_on_brokerage, brokerage_currency, fx_rate, \
          spot_fx_rate, deemed_acquisition_date, holding_account_id, {}) \
         VALUES ('Buy', ?, ?, 'stated', ?, '0', ?, ?, ?, '0', ?, ?, ?, ?, ?, ?)",
        provenance.column()
    )))
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
    Ok(result.last_insert_rowid())
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::open_parcels;
    use crate::entities::amit_adjustment_generation::{self, GenerateBody};
    use crate::entities::corporate_action::{ActionKind, CorporateAction};
    use crate::entities::holding_account::{self, HoldingAccount};
    use crate::entities::sell::AllocationInput;
    use crate::entities::transfer::{self, TransferBody};
    use crate::entities::{demerger, scrip_exchange};
    use crate::reports::open_parcels::{OpenParcel, db_open_parcels};
    use crate::test_support::{self, dec, test_pool, ymd};

    /// A transfer dated *before* an already-entered AMMA statement's year end
    /// must not fold that statement's reduction into the replacement parcel's
    /// carried cost base: the adjustment arises at the statement's year end,
    /// which has not happened yet as at the operation date — exactly the bound
    /// the return-of-capital events already observe. The statement's reduction
    /// instead reaches the replacement parcel when its adjustment row is
    /// re-entered against it (the reach-through entry the generation refusal
    /// points at), so it is applied exactly once.
    #[tokio::test]
    async fn a_statement_year_ending_after_the_operation_does_not_reach_the_carried_cost_base() {
        let pool = test_pool().await;
        test_support::listing(1).ticker("AAA").insert(&pool).await;
        holding_account::db_upsert(
            &pool,
            &HoldingAccount {
                id: 2,
                name: "Broker".to_string(),
            },
        )
        .await
        .unwrap();
        // 1,000 units at $10: a $10,000 initial cost base.
        test_support::buy(1, 1)
            .date(ymd(2024, 1, 10))
            .qty(dec("1000"))
            .insert(&pool)
            .await;
        // FY2025 AMMA statement ($1/unit, year end 2025-06-30) entered and its
        // adjustments generated while the parcel still sat in account 1.
        test_support::amma(6, 1)
            .units(dec("1000"))
            .cost_base_adjustment(dec("1"))
            .with(|a| a.tax_year_end_date = ymd(2025, 6, 30))
            .insert(&pool)
            .await;
        amit_adjustment_generation::db_generate(&pool, 6, &GenerateBody::default())
            .await
            .unwrap();

        // The transfer is dated before the statement's year end.
        let result = transfer::db_transfer(
            &pool,
            1,
            &TransferBody {
                listing_id: 1,
                date: ymd(2025, 5, 1),
                from_account_id: 1,
                to_account_id: 2,
                allocations: vec![AllocationInput {
                    purchase_trade_id: 1,
                    quantity_allocated: dec("1000"),
                }],
                fee_allocations: Vec::new(),
                fee_market_price: None,
                fee_fx_rate: None,
            },
        )
        .await
        .unwrap();
        let replacement = &result.transfer_ins[0];

        // The carried cost base (on the brokerage column, price 0) is the full
        // $10,000 — the statement's year end postdates the operation — and it
        // agrees with the open-parcels loader as at the operation date.
        assert_eq!(replacement.brokerage, dec("10000"));
        let mut conn = pool.acquire().await.unwrap();
        let open = open_parcels::load(&mut conn, Some(ymd(2025, 5, 1)))
            .await
            .unwrap();
        let row = open
            .iter()
            .find(|p| p.parcel.id == replacement.id)
            .expect("replacement parcel open at the operation date");
        assert_eq!(row.cost_base.adjusted, dec("10000"));

        // The repair path the generation refusal directs to: the stale rows
        // against the consumed source parcel are removed and the statement is
        // entered against the replacement parcel (accepted — the transfer
        // traces its units back to the statement's account).
        sqlx::query("DELETE FROM amit_adjustments WHERE amma_statement_id = 6")
            .execute(&pool)
            .await
            .unwrap();
        test_support::amit_adjustment(&pool, 50, 6, replacement.id, dec("1000")).await;

        // The reduction is applied exactly once: $10,000 − 1,000 × $1.
        let open = open_parcels::load(&mut conn, None).await.unwrap();
        let row = open
            .iter()
            .find(|p| p.parcel.id == replacement.id)
            .expect("replacement parcel still open");
        assert_eq!(row.cost_base.adjusted, dec("9000"));
    }

    /// A parcel moved twice — plan account → broker A → broker B — reports both
    /// replacements, not just the first: the walk follows the chain, which is
    /// what makes an ESS statement (or an AMMA one) still reachable from where
    /// the units ended up.
    #[tokio::test]
    async fn descendants_follow_a_chain_of_transfers() {
        let pool = test_pool().await;
        test_support::listing(1).ticker("AAA").insert(&pool).await;
        for (id, name) in [(2, "Broker A"), (3, "Broker B")] {
            holding_account::db_upsert(
                &pool,
                &HoldingAccount {
                    id,
                    name: name.to_string(),
                },
            )
            .await
            .unwrap();
        }
        test_support::buy(1, 1)
            .date(ymd(2023, 1, 10))
            .qty(dec("100"))
            .insert(&pool)
            .await;

        let move_to = async |transfer_id: i64, parcel: i64, from: i64, to: i64, date| {
            transfer::db_transfer(
                &pool,
                transfer_id,
                &TransferBody {
                    listing_id: 1,
                    date,
                    from_account_id: from,
                    to_account_id: to,
                    allocations: vec![AllocationInput {
                        purchase_trade_id: parcel,
                        quantity_allocated: dec("100"),
                    }],
                    fee_allocations: Vec::new(),
                    fee_market_price: None,
                    fee_fx_rate: None,
                },
            )
            .await
            .unwrap()
            .transfer_ins[0]
                .id
        };
        let first = move_to(1, 1, 1, 2, ymd(2023, 3, 1)).await;
        let second = move_to(2, first, 2, 3, ymd(2023, 5, 1)).await;

        let mut conn = pool.acquire().await.unwrap();
        assert_eq!(
            replacement_descendants(&mut conn, 1).await.unwrap(),
            vec![first, second]
        );
        assert_eq!(
            replacement_descendants(&mut conn, first).await.unwrap(),
            vec![second]
        );
        // The end of the chain, and a parcel no rollover touched, have none.
        assert!(
            replacement_descendants(&mut conn, second)
                .await
                .unwrap()
                .is_empty()
        );
        test_support::buy(50, 1)
            .date(ymd(2023, 1, 10))
            .qty(dec("10"))
            .insert(&pool)
            .await;
        assert!(
            replacement_descendants(&mut conn, 50)
                .await
                .unwrap()
                .is_empty()
        );
    }

    /// The chain the register question turns on: a parcel bought on one
    /// listing, **scrip-exchanged** onto another, and then moved by an
    /// operation that keeps it there — a transfer between the taxpayer's own
    /// accounts, or a demerger of the acquiring listing.
    ///
    /// A return of capital on the acquiring listing whose record date falls
    /// *before* the exchange found the taxpayer not on that register, so the
    /// exchange's replacement parcel is ex-entitlement to it
    /// (`entities::scrip_exchange`'s
    /// `a_replacement_parcel_is_ex_entitlement_to_the_acquirers_return_of_capital`).
    /// The later operation must not hand the entitlement back — which it would
    /// if `registered_from` were the parcel's own deemed acquisition date,
    /// since that chains all the way to the first buy, years before the record
    /// date. `ParcelRow::registered_from` walks up to the source parcel
    /// instead.
    ///
    /// Seeds listings 1 (bought) and 2 (acquiring), a 2,000-unit parcel at $1
    /// costing $2,000, a $0.05/unit payment (record date 2022-06-25, paid
    /// 2026-08-01 — $100 over the parcel) and the 2023 exchange already run,
    /// leaving one open parcel on listing 2. Answers its id.
    async fn exchanged_across_a_return_of_capital_record_date(pool: &sqlx::SqlitePool) -> i64 {
        test_support::listing(1).ticker("OLD").insert(pool).await;
        test_support::listing(2).ticker("NEW").insert(pool).await;
        test_support::buy(1, 1)
            .date(ymd(2019, 1, 10))
            .qty(dec("2000"))
            .price(dec("1"))
            .insert(pool)
            .await;
        corporate_action::db_upsert(
            pool,
            &CorporateAction {
                id: 5,
                listing_id: 2,
                date: ymd(2026, 8, 1),
                kind: ActionKind::ReturnOfCapital {
                    amount_per_unit: dec("0.05"),
                    currency: "AUD".to_string(),
                    record_date: Some(ymd(2022, 6, 25)),
                },
            },
        )
        .await
        .unwrap();
        corporate_action::db_upsert(
            pool,
            &CorporateAction {
                id: 10,
                listing_id: 1,
                date: ymd(2023, 3, 1),
                kind: ActionKind::ScripForScrip {
                    scrip_listing_id: 2,
                    scrip_new_units: dec("1"),
                    scrip_old_units: dec("1"),
                    scrip_cash_per_unit: None,
                    scrip_market_value: None,
                    scrip_cash_currency: None,
                },
            },
        )
        .await
        .unwrap();
        scrip_exchange::db_exchange(pool, 10)
            .await
            .unwrap()
            .replacements[0]
            .id
    }

    /// The one open parcel of listing 2: its return-of-capital reduction and
    /// remaining cost base, for the two chain tests below.
    async fn listing_2_parcel(pool: &sqlx::SqlitePool) -> (Decimal, Decimal) {
        let open: Vec<OpenParcel> = db_open_parcels(pool)
            .await
            .unwrap()
            .into_iter()
            .filter(|p| p.listing_id == 2)
            .collect();
        assert_eq!(open.len(), 1);
        (
            open[0].return_of_capital_reduction,
            open[0].remaining_cost_base,
        )
    }

    /// A transfer between the taxpayer's own accounts leaves the units on
    /// listing 2's register exactly as long as they already had been — since
    /// the 2023 exchange, eight months after the payment was fixed — so the
    /// parcel stays ex-entitlement across it.
    #[tokio::test]
    async fn a_transfer_does_not_reinstate_the_entitlement_an_earlier_exchange_denied() {
        let pool = test_pool().await;
        let exchanged = exchanged_across_a_return_of_capital_record_date(&pool).await;
        holding_account::db_upsert(
            &pool,
            &HoldingAccount {
                id: 2,
                name: "Broker".to_string(),
            },
        )
        .await
        .unwrap();
        assert_eq!(listing_2_parcel(&pool).await, (Decimal::ZERO, dec("2000")));

        transfer::db_transfer(
            &pool,
            1,
            &TransferBody {
                listing_id: 2,
                date: ymd(2025, 4, 1),
                from_account_id: 1,
                to_account_id: 2,
                allocations: vec![AllocationInput {
                    purchase_trade_id: exchanged,
                    quantity_allocated: dec("2000"),
                }],
                fee_allocations: Vec::new(),
                fee_market_price: None,
                fee_fx_rate: None,
            },
        )
        .await
        .unwrap();

        assert_eq!(listing_2_parcel(&pool).await, (Decimal::ZERO, dec("2000")));
    }

    /// The same chain with a **demerger of listing 2** in the transfer's
    /// place: its head parcel stays on listing 2, whose register the units
    /// only joined at the exchange, so it too stays ex-entitlement. Only the
    /// head parcel is asserted — the spun-off listing 3 parcel is of a
    /// register the payment was never against.
    #[tokio::test]
    async fn a_demergers_head_parcel_does_not_reinstate_it_either() {
        let pool = test_pool().await;
        exchanged_across_a_return_of_capital_record_date(&pool).await;
        test_support::listing(3).ticker("SPUN").insert(&pool).await;
        corporate_action::db_upsert(
            &pool,
            &CorporateAction {
                id: 20,
                listing_id: 2,
                date: ymd(2025, 4, 1),
                kind: ActionKind::Demerger {
                    demerger_listing_id: 3,
                    demerger_new_units: dec("1"),
                    demerger_held_units: dec("5"),
                    demerger_cost_base_pct: dec("20"),
                    demerger_close_date: None,
                    demerger_close_price: None,
                    demerger_close_sourced_from: None,
                    demerger_close_reason: None,
                },
            },
        )
        .await
        .unwrap();
        demerger::db_demerge(&pool, 20).await.unwrap();

        // 80% of the $2,000 stays with the head parcel, and none of the
        // payment reaches it.
        assert_eq!(listing_2_parcel(&pool).await, (Decimal::ZERO, dec("1600")));
    }
}
