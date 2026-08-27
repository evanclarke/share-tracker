//! Assembling the *open* Buy/DRP parcels — the loader wrapped around
//! [`domain::cost_base`](crate::domain::cost_base)'s per-parcel pipeline, and
//! the single implementation of the "what is still held, and what did it
//! cost?" read every open-holdings view starts from.
//!
//! [`cost_base::adjusted_cost_base`] costs *some units of one parcel* given
//! reference data the caller has already loaded. Everything around that call
//! — read the Buy/DRP rows, read the sale allocations with each sale's date,
//! re-base the allocated quantity across splits, subtract, drop the fully
//! consumed parcels, load the AMIT/return-of-capital/split events and the FX
//! table, convert to AUD, and re-base the remainder into the reported unit
//! basis — is this module's [`load`]. It exists because the same block used to
//! be copy-pasted into the portfolio overview, the unrealised-gains report and
//! the open-parcels schedule, where a fix to (say) the split/ROC re-basing
//! interaction had to land three times and the copies had already drifted in
//! how they spelled the same intent.
//!
//! The one axis of variation is the as-of date: `Some(date)` is the position
//! as at that date (trades, sales and AMIT statements after it excluded,
//! return-of-capital payments bounded at it, quantities in that date's unit
//! basis), `None` the live view — which is the same thing as at *today*, not
//! "every recorded fact" (a future-dated corporate action or trade is not in
//! force yet; see [`infra::date::as_of_or_today`](crate::infra::date::as_of_or_today)).
//! Callers keep their own aggregation and shaping — this module returns
//! parcels, not report rows.
//!
//! Reports that walk *sold* parcels (realised gains, the net-capital-gain
//! report's E10/G1 walks, the annual tax report's disposal detail) are
//! deliberately not built on [`load`]: they cost allocations against sale
//! dates rather than a remainder, and each needs a different slice of the
//! reference data. What they do share is [`db_units_sold`], the allocations
//! read.

use std::collections::HashMap;

use chrono::NaiveDate;
use rust_decimal::Decimal;
use sqlx::Row;

use crate::domain::cost_base::{self, CostBase, ParcelRow};
use crate::entities::corporate_action;
use crate::infra::date::{as_of_or_open, as_of_or_today};
use crate::infra::decimal::parse_dec;
use crate::infra::fx::FxRates;

/// One Buy/DRP parcel with units still open, as [`load`] assembles it.
#[derive(Debug, Clone)]
pub struct OpenParcel {
    /// The parcel's trade row — `listing_id`, `holding_account_id`,
    /// `quantity` (as transacted), `acquired()` and the rest.
    pub parcel: ParcelRow,
    /// Units not yet allocated to a Sell, in the unit basis of `as_of`
    /// (current units when `as_of` is `None`) — the figure that lines up with
    /// a market price for that date, and the one a broker statement shows.
    /// Always > 0: fully consumed parcels are not returned.
    ///
    /// The cost-base arithmetic behind [`Self::cost_base`] runs on the
    /// parcel's *as-acquired* basis instead; a split scales the unit count
    /// but never the cost base (TD 2000/10), so the two bases differ only in
    /// this field. `parcel.quantity` is the whole parcel as transacted, on
    /// that as-acquired basis.
    pub remaining_as_of: Decimal,
    /// The remaining units' AUD cost-base breakdown, converted at the
    /// parcel's (possibly deemed) acquisition-month rate — the full
    /// [`cost_base`] pipeline, so callers needing only the bottom line take
    /// `.adjusted`.
    pub cost_base: CostBase,
}

/// Which parcel-consuming Sells [`db_units_sold`] tallies. A rollover's
/// closing Sell — a transfer's, a scrip-for-scrip exchange's, a demerger's
/// (the trades carrying a `rollover::Provenance` column) — consumes its
/// parcel without the units leaving the taxpayer's hands: they are carried
/// into the operation's replacement parcels, and an AMMA statement's
/// reduction follows them there (the reach-through entry
/// `entities::amit_adjustment_generation` and the adjustment write path both
/// accept). So "how many units has this parcel got left?" and "how many of
/// its units were disposed of?" are different questions, and a caller must
/// say which it is asking rather than every call site quietly meaning the
/// first.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Counted {
    /// Every allocation — any Sell that consumed units of the parcel,
    /// rollover closing Sells included: the read behind a parcel's remaining
    /// units ([`load`], the performance walk, the net-capital-gain report's
    /// unit cohorts, the cross-check's departed-units allowance).
    AllConsumptions,
    /// Only real disposals — allocations whose Sell carries no rollover
    /// provenance column: the read behind the AMIT held/spill split
    /// (`entities::amit_adjustment::db_cost_base_reduction_events`), where a
    /// transfer counted as a disposal spilled a statement's reduction onto an
    /// earlier real Sell and silently changed its realised gain. A transfer's
    /// separate network-fee Sell carries no provenance column and still
    /// counts — that one *is* a disposal.
    DisposalsOnly,
}

/// One Sell's consumption of a purchase parcel's units: how many, when, and
/// whether that Sell was a **real disposal**.
///
/// Both kinds of consumption end the *parcel's* hold on the units, so a "were
/// these units still in this parcel on that date?" test — which is what every
/// cost-base reduction asks — treats them alike. Only a question about a CGT
/// event, where the answer turns on whether the taxpayer disposed of anything,
/// needs [`Self::disposal`]: a rollover's closing Sell empties the parcel
/// without the units leaving the taxpayer's hands.
#[derive(Debug, Clone, Copy)]
pub struct UnitsConsumed {
    /// The Sell's date.
    pub date: NaiveDate,
    /// Units allocated, in *sale-date* units (see [`db_units_sold`]).
    pub quantity: Decimal,
    /// False for a rollover's closing Sell — a transfer's, a scrip-for-scrip
    /// exchange's or a demerger's, the trades carrying a
    /// `rollover::Provenance` column. Always true under
    /// [`Counted::DisposalsOnly`], which filters the rest out at the query.
    pub disposal: bool,
}

impl UnitsConsumed {
    /// The `(date, quantity)` pair the split re-basing helpers take — the two
    /// fields that matter to a quantity question, without the provenance that
    /// does not.
    pub fn dated(&self) -> (NaiveDate, Decimal) {
        (self.date, self.quantity)
    }
}

/// Units allocated out of each purchase parcel by a Sell on or before
/// `as_of`, keyed by `purchase_trade_id` and carrying each sale's date: the
/// allocated quantity is in *sale-date* units, so a caller must re-base it to
/// the parcel's as-acquired basis (`corporate_action::sold_in_acquired_units`,
/// or `as_acquired_quantity` per sale where the sale date matters on its own)
/// rather than subtracting it directly. `None` bounds nothing. `counted` says
/// which Sells tally — see [`Counted`] — and each row says which kind it is,
/// so a caller that must count every consumption can still tell a disposal
/// from a rollover without a second read.
pub async fn db_units_sold(
    conn: &mut sqlx::SqliteConnection,
    as_of: Option<NaiveDate>,
    counted: Counted,
) -> Result<HashMap<i64, Vec<UnitsConsumed>>, sqlx::Error> {
    let no_provenance = crate::domain::rollover::no_provenance_sql("s");
    let provenance_filter = match counted {
        Counted::AllConsumptions => String::new(),
        Counted::DisposalsOnly => format!(" AND {no_provenance}"),
    };
    let rows = sqlx::query(sqlx::AssertSqlSafe(format!(
        "SELECT pa.purchase_trade_id, pa.quantity_allocated, s.date AS sale_date, \
                ({no_provenance}) AS is_disposal \
         FROM parcel_allocations pa JOIN trades s ON s.id = pa.sale_trade_id \
         WHERE s.date <= ?{provenance_filter}",
    )))
    .bind(as_of_or_open(as_of))
    .fetch_all(&mut *conn)
    .await?;

    let mut sold: HashMap<i64, Vec<UnitsConsumed>> = HashMap::new();
    for row in &rows {
        let purchase_trade_id: i64 = row.try_get("purchase_trade_id")?;
        sold.entry(purchase_trade_id)
            .or_default()
            .push(UnitsConsumed {
                date: row.try_get("sale_date")?,
                quantity: parse_dec("quantity_allocated", row.try_get("quantity_allocated")?)?,
                disposal: row.try_get::<i64, _>("is_disposal")? != 0,
            });
    }
    Ok(sold)
}

/// Every Buy/DRP parcel with units still open as at `as_of` (`None` = the
/// live view, i.e. as at today), each with its remaining quantity in that
/// date's unit basis and the AUD cost base of those units. Parcels fully
/// consumed by sales are omitted.
///
/// Takes the caller's own connection so it composes into a report's existing
/// single-snapshot read transaction (the house rule: one report, one
/// consistent read) — every input below is read on that one connection.
/// Ordered by trade id, so a caller that does not sort still gets a stable
/// result.
pub async fn load(
    conn: &mut sqlx::SqliteConnection,
    as_of: Option<NaiveDate>,
) -> Result<Vec<OpenParcel>, sqlx::Error> {
    // "Live" means as at *today*, not "every recorded fact": a split or
    // return of capital is recorded when its terms are announced, weeks
    // before it takes effect, and must not restate today's holdings until it
    // does (`infra::date::as_of_or_today`). Resolving the cutoff here — and
    // passing it on as `Some(cutoff)` below — keeps every bound in this
    // function on one date, so the live view is exactly the as-at-today view.
    let cutoff = as_of_or_today(as_of);
    let as_of = Some(cutoff);
    let parcels: Vec<ParcelRow> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT {} FROM trades WHERE trade_type IN ('Buy', 'DRP') AND date <= ? ORDER BY id",
        ParcelRow::COLUMNS
    )))
    .bind(cutoff)
    .fetch_all(&mut *conn)
    .await?;

    if parcels.is_empty() {
        // Nothing held, so none of the reference data below can matter.
        return Ok(vec![]);
    }

    // Every consumption: a rollover's closing Sell empties its source parcel
    // exactly as a real disposal does — the units live on in the replacement
    // parcels, which this same read sees as their own rows.
    let qty_sold = db_units_sold(&mut *conn, as_of, Counted::AllConsumptions).await?;
    // Total AMIT cost-base reduction per parcel (statements for years ending
    // after `as_of` excluded — the adjustment arises at its statement's year
    // end).
    let amit_events =
        crate::entities::amit_adjustment::db_cost_base_reduction_events(&mut *conn, as_of).await?;
    // Return-of-capital payments (CGT event G1) per listing.
    let roc_events = corporate_action::db_return_of_capital_events(&mut *conn).await?;
    // Share splits/consolidations per listing (quantity re-basing).
    let split_events = corporate_action::db_share_split_events(&mut *conn).await?;
    // Every imported ATO FX rate — the per-parcel conversion below is then a
    // map lookup, not one DB round-trip each.
    let fx = FxRates::load(&mut *conn).await?;

    let mut open = Vec::with_capacity(parcels.len());
    for parcel in parcels {
        let splits = split_events.get(&parcel.listing_id).map_or(&[][..], |v| v);
        // Cost-base arithmetic stays in the parcel's as-acquired units; each
        // sale's allocated quantity is re-based back across any splits
        // between acquisition and that sale.
        let sold = corporate_action::sold_in_acquired_units(
            qty_sold
                .get(&parcel.id)
                .map_or(&[][..], |v| v)
                .iter()
                .map(UnitsConsumed::dated),
            splits,
            parcel.date,
        );
        let remaining_as_acquired = parcel.quantity - sold;
        if remaining_as_acquired <= Decimal::ZERO {
            continue;
        }

        // The shared pipeline for the remaining units, converted to AUD at
        // the (possibly deemed) acquisition month. `up_to` is the as-of date:
        // a return-of-capital payment after it has not happened in this view;
        // `None` means an unsold unit was held for every recorded payment.
        let cb = cost_base::adjusted_cost_base(
            &parcel.parcel(),
            remaining_as_acquired,
            amit_events.get(&parcel.id).map_or(&[][..], |v| v),
            roc_events.get(&parcel.listing_id).map_or(&[][..], |v| v),
            splits,
            cost_base::Held::AsAt(as_of),
        )?
        .into_aud_with(
            &fx,
            &parcel.currency,
            parcel.acquired(),
            parcel.fx_override(),
        )?;

        // Reported in the unit basis of `as_of` (live view: current units,
        // after every recorded split) so it lines up with a price for that
        // date.
        let remaining_as_of = corporate_action::split_adjusted_quantity(
            remaining_as_acquired,
            splits,
            parcel.date,
            as_of,
        );

        open.push(OpenParcel {
            parcel,
            remaining_as_of,
            cost_base: cb,
        });
    }
    Ok(open)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::entities::corporate_action::{self, ActionKind, CorporateAction};
    use crate::test_support::{self, allocate, dec, test_pool, ymd};
    use sqlx::SqlitePool;

    async fn load_all(pool: &SqlitePool, as_of: Option<NaiveDate>) -> Vec<OpenParcel> {
        let mut conn = pool.acquire().await.unwrap();
        load(&mut conn, as_of).await.unwrap()
    }

    async fn split(pool: &SqlitePool, id: i64, listing_id: i64, date: NaiveDate, ratio: &str) {
        corporate_action::db_upsert(
            pool,
            &CorporateAction {
                id,
                listing_id,
                date,
                kind: ActionKind::ShareSplit {
                    split_new_units: dec(ratio),
                    split_old_units: Decimal::ONE,
                },
            },
        )
        .await
        .unwrap();
    }

    async fn return_of_capital(
        pool: &SqlitePool,
        id: i64,
        listing_id: i64,
        date: NaiveDate,
        per_unit: &str,
    ) {
        corporate_action::db_upsert(
            pool,
            &CorporateAction {
                id,
                listing_id,
                date,
                kind: ActionKind::ReturnOfCapital {
                    amount_per_unit: dec(per_unit),
                    currency: "AUD".to_string(),
                    record_date: None,
                },
            },
        )
        .await
        .unwrap();
    }

    /// 100 units at $10 + $9.95 brokerage: the whole parcel is open, and its
    /// cost base is the acquisition cost plus the incidental brokerage.
    #[tokio::test]
    async fn an_untouched_parcel_is_open_for_its_whole_quantity() {
        let pool = test_pool().await;
        test_support::listing(1).insert(&pool).await;
        test_support::buy(1, 1)
            .date(ymd(2024, 1, 10))
            .qty(dec("100"))
            .price(dec("10"))
            .brokerage(dec("9.95"))
            .insert(&pool)
            .await;

        let open = load_all(&pool, None).await;
        assert_eq!(open.len(), 1);
        assert_eq!(open[0].remaining_as_of, dec("100"));
        assert_eq!(open[0].cost_base.initial_cost, dec("1009.95"));
        assert_eq!(open[0].cost_base.adjusted, dec("1009.95"));
    }

    /// A parcel every unit of which has been allocated to a Sell is not an
    /// open parcel — every copy of this loader dropped it, and so does this.
    #[tokio::test]
    async fn a_fully_consumed_parcel_is_filtered_out() {
        let pool = test_pool().await;
        test_support::listing(1).insert(&pool).await;
        test_support::buy(1, 1)
            .date(ymd(2024, 1, 10))
            .qty(dec("100"))
            .price(dec("10"))
            .insert(&pool)
            .await;
        test_support::buy(2, 1)
            .date(ymd(2024, 2, 12))
            .qty(dec("40"))
            .price(dec("11"))
            .insert(&pool)
            .await;
        test_support::sell(3, 1)
            .date(ymd(2024, 3, 11))
            .qty(dec("100"))
            .price(dec("12"))
            .insert(&pool)
            .await;
        allocate(&pool, 1, 3, 1, dec("100")).await;

        let open = load_all(&pool, None).await;
        assert_eq!(open.len(), 1);
        assert_eq!(open[0].parcel.id, 2);
        assert_eq!(open[0].remaining_as_of, dec("40"));
    }

    /// `as_of` is a point-in-time view from both ends: a parcel bought after
    /// it does not exist yet, and a sale after it has not happened yet.
    #[tokio::test]
    async fn as_of_excludes_later_trades_and_later_sales() {
        let pool = test_pool().await;
        test_support::listing(1).insert(&pool).await;
        test_support::buy(1, 1)
            .date(ymd(2024, 1, 10))
            .qty(dec("100"))
            .price(dec("10"))
            .insert(&pool)
            .await;
        test_support::buy(2, 1)
            .date(ymd(2024, 8, 12))
            .qty(dec("50"))
            .price(dec("11"))
            .insert(&pool)
            .await;
        test_support::sell(3, 1)
            .date(ymd(2024, 9, 10))
            .qty(dec("60"))
            .price(dec("12"))
            .insert(&pool)
            .await;
        allocate(&pool, 1, 3, 1, dec("60")).await;

        // As at 30 June: only parcel 1 exists, and none of it is sold yet.
        let open = load_all(&pool, Some(ymd(2024, 6, 30))).await;
        assert_eq!(open.len(), 1);
        assert_eq!(open[0].parcel.id, 1);
        assert_eq!(open[0].remaining_as_of, dec("100"));
        assert_eq!(open[0].cost_base.adjusted, dec("1000"));

        // Live: the later buy is held and the sale has consumed 60 of the
        // first parcel.
        let open = load_all(&pool, None).await;
        assert_eq!(open.len(), 2);
        assert_eq!(open[0].remaining_as_of, dec("40"));
        assert_eq!(open[0].cost_base.adjusted, dec("400"));
        assert_eq!(open[1].remaining_as_of, dec("50"));
    }

    /// SCENARIOS E-14/E-14b: a corporate action is normally recorded when its
    /// terms are announced, weeks before it takes effect, so the live view
    /// must not apply it yet. Before the fix `None` meant the `9999-12-31`
    /// sentinel — "every recorded fact" — and a split dated years out already
    /// restated today's quantity while the as-of-dated reports ignored it.
    #[tokio::test]
    async fn the_live_view_ignores_a_future_dated_corporate_action() {
        let pool = test_pool().await;
        let today = crate::infra::date::today();
        let future = today + chrono::Days::new(400);
        test_support::listing(1).insert(&pool).await;
        test_support::buy(1, 1)
            .date(ymd(2023, 1, 10))
            .qty(dec("100"))
            .price(dec("10"))
            .insert(&pool)
            .await;
        split(&pool, 1, 1, future, "2").await;
        return_of_capital(&pool, 2, 1, future, "1.00").await;

        let open = load_all(&pool, None).await;
        assert_eq!(open.len(), 1);
        assert_eq!(
            open[0].remaining_as_of,
            dec("100"),
            "the split has not happened yet"
        );
        assert_eq!(open[0].cost_base.roc_reduction, Decimal::ZERO);
        assert_eq!(open[0].cost_base.adjusted, dec("1000"));

        // The whole point of the fix: the undated view is exactly the view
        // as at today, so the two reports can't disagree.
        let as_at_today = load_all(&pool, Some(today)).await;
        assert_eq!(as_at_today.len(), open.len());
        assert_eq!(as_at_today[0].remaining_as_of, open[0].remaining_as_of);
        assert_eq!(
            as_at_today[0].cost_base.adjusted,
            open[0].cost_base.adjusted
        );

        // And it does take effect on its own date.
        let later = load_all(&pool, Some(future)).await;
        assert_eq!(later[0].remaining_as_of, dec("200"));
        assert_eq!(later[0].cost_base.roc_reduction, dec("200"));
        assert_eq!(later[0].cost_base.adjusted, dec("800"));
    }

    /// Trades are bounded at today by the same rule rather than carved out —
    /// a future-dated Buy is not held yet, and a future-dated Sell has not
    /// consumed its parcel yet (both nearly always typos, and both surface on
    /// their own date).
    ///
    /// The write path now refuses a future-dated trade outright
    /// (`trade::AmountsError::FutureDate`, SCENARIOS S-10), so the two rows
    /// are dated forward with SQL afterwards: this loader's `as_of` filter is
    /// an independent invariant, and it still has to hold for a row that
    /// predates the refusal.
    #[tokio::test]
    async fn the_live_view_ignores_a_future_dated_trade() {
        let pool = test_pool().await;
        let future = crate::infra::date::today() + chrono::Days::new(30);
        test_support::listing(1).insert(&pool).await;
        test_support::buy(1, 1)
            .date(ymd(2023, 1, 10))
            .qty(dec("100"))
            .price(dec("10"))
            .insert(&pool)
            .await;
        test_support::buy(2, 1)
            .date(ymd(2023, 1, 20))
            .qty(dec("50"))
            .price(dec("11"))
            .insert(&pool)
            .await;
        test_support::sell(3, 1)
            .date(ymd(2023, 2, 1))
            .qty(dec("60"))
            .price(dec("12"))
            .insert(&pool)
            .await;
        allocate(&pool, 1, 3, 1, dec("60")).await;
        for id in [2, 3] {
            sqlx::query("UPDATE trades SET date = ?, settlement_date = ? WHERE id = ?")
                .bind(future)
                .bind(future)
                .bind(id)
                .execute(&pool)
                .await
                .unwrap();
        }

        let open = load_all(&pool, None).await;
        assert_eq!(open.len(), 1, "the future-dated buy is not held yet");
        assert_eq!(open[0].parcel.id, 1);
        assert_eq!(
            open[0].remaining_as_of,
            dec("100"),
            "the future-dated sale has not consumed anything yet"
        );

        let later = load_all(&pool, Some(future)).await;
        assert_eq!(later.len(), 2);
        assert_eq!(later[0].remaining_as_of, dec("40"));
        assert_eq!(later[1].remaining_as_of, dec("50"));
    }

    /// A sale after a 2-for-1 split allocates *post-split* units, so the
    /// remainder must be computed after re-basing them back to as-acquired
    /// units — and the reported quantity re-based forward again. The cost
    /// base is untouched by the split (TD 2000/10).
    #[tokio::test]
    async fn a_post_split_sale_is_re_based_before_and_after_the_subtraction() {
        let pool = test_pool().await;
        test_support::listing(1).insert(&pool).await;
        test_support::buy(1, 1)
            .date(ymd(2024, 1, 10))
            .qty(dec("100"))
            .price(dec("10"))
            .insert(&pool)
            .await;
        split(&pool, 1, 1, ymd(2024, 3, 1), "2").await;
        test_support::sell(2, 1)
            .date(ymd(2024, 4, 2))
            .qty(dec("60"))
            .price(dec("6"))
            .insert(&pool)
            .await;
        // 60 post-split units = 30 as-acquired units.
        allocate(&pool, 1, 2, 1, dec("60")).await;

        let open = load_all(&pool, None).await;
        assert_eq!(open.len(), 1);
        assert_eq!(open[0].remaining_as_of, dec("140"));
        // 70/100 of the $1000 cost base — the split scaled units, not cost.
        assert_eq!(open[0].cost_base.adjusted, dec("700"));

        // As at a date before the split, the same parcel reports in
        // pre-split units and nothing is sold yet.
        let open = load_all(&pool, Some(ymd(2024, 2, 1))).await;
        assert_eq!(open[0].remaining_as_of, dec("100"));
    }

    /// The AMIT cost-base reduction (CGT event E10) and return-of-capital
    /// payments (G1) both reach the remaining units' cost base, and the
    /// as-of date bounds which of them have happened.
    ///
    /// The fixture is the one way the two still meet on a parcel now that a
    /// return of capital is refused on an AMIT listing (SCENARIOS E-04): a
    /// fund that converts to an AMIT part-way through the holding, whose
    /// pre-conversion E4 payment was recorded while it was an ordinary trust.
    #[tokio::test]
    async fn amit_and_return_of_capital_reduce_the_remaining_cost_base() {
        let pool = test_pool().await;
        test_support::listing(1).insert(&pool).await;
        test_support::buy(1, 1)
            .date(ymd(2024, 1, 10))
            .qty(dec("100"))
            .price(dec("10"))
            .insert(&pool)
            .await;
        // 50c/unit return of capital in September 2024, while the trust was
        // not yet an AMIT…
        return_of_capital(&pool, 1, 1, ymd(2024, 9, 15), "0.50").await;
        // …and the conversion, after which its cost-base movement comes from
        // its AMMA statement instead.
        test_support::listing(1).amit(true).insert(&pool).await;
        // AMMA statement for the year ended 30 June 2024: a 20c/unit AMIT
        // cost base net amount (positive = a reduction).
        test_support::amma(1, 1)
            .cost_base_adjustment(dec("0.20"))
            .with(|a| a.tax_year_end_date = ymd(2024, 6, 30))
            .insert(&pool)
            .await;
        test_support::amit_adjustment(&pool, 1, 1, 1, dec("100")).await;

        // Live: both reductions apply — 1000 − 20 − 50 = 930.
        let open = load_all(&pool, None).await;
        assert_eq!(open[0].cost_base.amit_reduction, dec("20"));
        assert_eq!(open[0].cost_base.roc_reduction, dec("50"));
        assert_eq!(open[0].cost_base.adjusted, dec("930"));

        // As at 30 June 2024: the AMMA statement's year has ended, the
        // return-of-capital payment has not been made — 1000 − 20 = 980.
        let open = load_all(&pool, Some(ymd(2024, 6, 30))).await;
        assert_eq!(open[0].cost_base.amit_reduction, dec("20"));
        assert_eq!(open[0].cost_base.roc_reduction, Decimal::ZERO);
        assert_eq!(open[0].cost_base.adjusted, dec("980"));

        // As at 31 March 2024: neither has happened.
        let open = load_all(&pool, Some(ymd(2024, 3, 31))).await;
        assert_eq!(open[0].cost_base.amit_reduction, Decimal::ZERO);
        assert_eq!(open[0].cost_base.adjusted, dec("1000"));
    }

    /// A foreign-currency parcel converts at its acquisition month's ATO
    /// rate, and the whole breakdown converts at that one rate.
    #[tokio::test]
    async fn a_foreign_parcel_converts_at_its_acquisition_month_rate() {
        let pool = test_pool().await;
        test_support::listing(1)
            .currency("USD")
            .mic("XNYS")
            .insert(&pool)
            .await;
        crate::entities::rba_fx_rate::db_import_rate(&pool, "USD", "2024-01", dec("0.50"))
            .await
            .unwrap();
        test_support::buy(1, 1)
            .date(ymd(2024, 1, 10))
            .qty(dec("100"))
            .price(dec("10"))
            .currency("USD")
            .insert(&pool)
            .await;

        let open = load_all(&pool, None).await;
        // USD 1000 at 0.50 USD per AUD = AUD 2000.
        assert_eq!(open[0].cost_base.initial_cost, dec("2000"));
        assert_eq!(open[0].cost_base.adjusted, dec("2000"));
    }

    /// The identity the duplication risked: the portfolio overview, the
    /// unrealised-gains report and the open-parcels schedule all report the
    /// same total cost base for the same holdings, because they now share
    /// this loader. Uses a fixture with everything the copies varied on — a
    /// split, a partial sale, an AMIT reduction and a return of capital.
    #[tokio::test]
    async fn the_open_holdings_reports_agree_on_total_cost_base() {
        use crate::reports::{open_parcels, portfolio, unrealised_gains};

        let pool = test_pool().await;
        test_support::listing(1).amit(true).insert(&pool).await;
        test_support::listing(2).insert(&pool).await;
        test_support::buy(1, 1)
            .date(ymd(2023, 5, 10))
            .qty(dec("100"))
            .price(dec("10"))
            .brokerage(dec("9.95"))
            .insert(&pool)
            .await;
        test_support::buy(2, 2)
            .date(ymd(2023, 8, 1))
            .qty(dec("200"))
            .price(dec("3"))
            .insert(&pool)
            .await;
        split(&pool, 1, 2, ymd(2024, 1, 15), "2").await;
        test_support::sell(3, 2)
            .date(ymd(2024, 2, 1))
            .qty(dec("100"))
            .price(dec("2"))
            .insert(&pool)
            .await;
        allocate(&pool, 1, 3, 2, dec("100")).await;
        test_support::amma(1, 1)
            .cost_base_adjustment(dec("0.15"))
            .with(|a| a.tax_year_end_date = ymd(2024, 6, 30))
            .insert(&pool)
            .await;
        test_support::amit_adjustment(&pool, 1, 1, 1, dec("100")).await;
        return_of_capital(&pool, 2, 2, ymd(2024, 3, 1), "0.10").await;

        // The live view: the portfolio overview and the open-parcels
        // schedule are both `as_of: None`.
        let holdings = portfolio::db_holdings(&pool, None).await.unwrap();
        let parcels = open_parcels::db_open_parcels(&pool).await.unwrap();
        let holdings_total: Decimal = holdings.iter().map(|h| h.total_cost_base).sum();
        let parcels_total: Decimal = parcels.iter().map(|p| p.remaining_cost_base).sum();
        assert_eq!(holdings_total, parcels_total);
        assert!(holdings_total > Decimal::ZERO);

        // And the as-at view: with an as-of date past every recorded fact,
        // the unrealised report must reach the same figure as the live
        // portfolio overview taken at that same date.
        let as_of = ymd(2024, 12, 31);
        let unrealised = unrealised_gains::db_unrealised_gains(&pool, as_of)
            .await
            .unwrap();
        let unrealised_total: Decimal = unrealised.iter().map(|u| u.total_cost_base).sum();
        assert_eq!(unrealised_total, holdings_total);

        let dated = portfolio::db_holdings(&pool, Some(as_of)).await.unwrap();
        let dated_total: Decimal = dated.iter().map(|h| h.total_cost_base).sum();
        assert_eq!(dated_total, holdings_total);

        // Quantities agree too, holding for holding.
        let mut by_key: HashMap<(i64, i64), Decimal> = HashMap::new();
        for p in &parcels {
            *by_key
                .entry((p.listing_id, p.holding_account_id))
                .or_insert(Decimal::ZERO) += p.remaining_quantity;
        }
        for h in &holdings {
            assert_eq!(
                by_key.get(&(h.listing_id, h.holding_account_id)).copied(),
                Some(h.quantity)
            );
        }
    }

    /// SCENARIOS E-14/E-14b at the report level: with a split and a return of
    /// capital recorded ahead of their effective date, the undated reports
    /// (overview, open parcels, and the parcel optimiser priced off them) and
    /// the unrealised report run for today must give one answer. They didn't
    /// — the undated ones read to the `9999-12-31` sentinel, so the same
    /// database on the same day reported 200 units at a $900 cost base one
    /// way and 100 units at $1,000 the other.
    #[tokio::test]
    async fn the_live_reports_agree_with_todays_unrealised_report_on_a_future_action() {
        use crate::reports::{open_parcels, parcel_optimiser, portfolio, unrealised_gains};

        let pool = test_pool().await;
        let today = crate::infra::date::today();
        let future = today + chrono::Days::new(400);
        test_support::listing(1).insert(&pool).await;
        test_support::buy(1, 1)
            .date(ymd(2023, 1, 10))
            .qty(dec("100"))
            .price(dec("10"))
            .insert(&pool)
            .await;
        split(&pool, 1, 1, future, "2").await;
        return_of_capital(&pool, 2, 1, future, "1.00").await;

        let holdings = portfolio::db_holdings(&pool, None).await.unwrap();
        assert_eq!(holdings.len(), 1);
        assert_eq!(holdings[0].quantity, dec("100"));
        assert_eq!(holdings[0].total_cost_base, dec("1000"));

        let parcels = open_parcels::db_open_parcels(&pool).await.unwrap();
        assert_eq!(parcels.len(), 1);
        assert_eq!(parcels[0].remaining_quantity, dec("100"));
        assert_eq!(parcels[0].return_of_capital_reduction, Decimal::ZERO);
        assert_eq!(parcels[0].remaining_cost_base, dec("1000"));

        // The optimiser prices a contemplated sale off those parcels, so the
        // reduced $9.00/unit basis would have overstated every strategy's
        // gain.
        let candidates = parcel_optimiser::db_candidate_parcels(&pool, 1, None, today)
            .await
            .unwrap();
        assert_eq!(candidates.parcels().len(), 1);
        assert_eq!(candidates.parcels()[0].remaining_quantity, dec("100"));
        assert_eq!(candidates.parcels()[0].remaining_cost_base, dec("1000"));

        let unrealised = unrealised_gains::db_unrealised_gains(&pool, today)
            .await
            .unwrap();
        assert_eq!(unrealised.len(), 1);
        assert_eq!(unrealised[0].quantity, holdings[0].quantity);
        assert_eq!(unrealised[0].total_cost_base, holdings[0].total_cost_base);
    }

    /// [`db_units_sold`] keys by purchase parcel and carries the *sale* date
    /// (not the parcel's), bounded by `as_of`.
    #[tokio::test]
    async fn units_sold_is_keyed_by_parcel_and_dated_by_the_sale() {
        let pool = test_pool().await;
        test_support::listing(1).insert(&pool).await;
        test_support::buy(1, 1)
            .date(ymd(2024, 1, 10))
            .qty(dec("100"))
            .price(dec("10"))
            .insert(&pool)
            .await;
        test_support::sell(2, 1)
            .date(ymd(2024, 5, 1))
            .qty(dec("30"))
            .price(dec("12"))
            .insert(&pool)
            .await;
        test_support::sell(3, 1)
            .date(ymd(2024, 9, 3))
            .qty(dec("20"))
            .price(dec("13"))
            .insert(&pool)
            .await;
        allocate(&pool, 1, 2, 1, dec("30")).await;
        allocate(&pool, 2, 3, 1, dec("20")).await;

        let mut conn = pool.acquire().await.unwrap();
        let sold = db_units_sold(&mut conn, None, Counted::AllConsumptions)
            .await
            .unwrap();
        let dated = |sold: &HashMap<i64, Vec<UnitsConsumed>>, id: i64| {
            sold.get(&id)
                .map(|v| v.iter().map(UnitsConsumed::dated).collect::<Vec<_>>())
        };
        assert_eq!(
            dated(&sold, 1),
            Some(vec![
                (ymd(2024, 5, 1), dec("30")),
                (ymd(2024, 9, 3), dec("20"))
            ])
        );
        // Both are ordinary Sells, so both are disposals.
        assert!(sold[&1].iter().all(|c| c.disposal));

        // Bounded at a date between the two sales, only the first counts.
        let sold = db_units_sold(&mut conn, Some(ymd(2024, 6, 30)), Counted::AllConsumptions)
            .await
            .unwrap();
        assert_eq!(dated(&sold, 1), Some(vec![(ymd(2024, 5, 1), dec("30"))]));
    }
}
