//! Which of the two methods — the 50% CGT discount or **indexation** — would
//! have given the better result on each disposal the indexation method is
//! still available for (SCENARIOS AA-a).
//!
//! For a CGT asset whose costs were incurred by **21 September 1999** an
//! individual may index those costs for inflation instead of applying the 50%
//! discount — never both, and never on a capital loss
//! (`docs/ato/indexing-the-cost-base.md`, QC 66024). This system applies the
//! **discount** throughout and does not model the election; that scope cut
//! used to be justified by "the discount almost always gives an individual
//! the better result", which is not true of the parcels this system can hold.
//! The earliest acquisition it accepts is 20 September 1985 (the pre-CGT
//! floor), whose indexation factor is 68.7 ÷ 39.7 = **1.730**, and indexation
//! wins on such a parcel whenever the proceeds are below **2.460 × cost** —
//! ordinary over a forty-year hold, not an edge case. This report is what
//! names the alternative on the data instead of leaving it in a paragraph.
//!
//! **Advisory only. No reported tax figure is affected by anything here** —
//! the net capital gain, the tax summary, the Annual Tax Report and every CSV
//! export apply the 50% discount, unchanged. The arithmetic of *taking* the
//! other method stays the taxpayer's own adjustment, exactly as it does for
//! CGT events K10/K11.
//!
//! # What the comparison is, exactly
//!
//! Each row compares **one parcel allocation of one disposal, in the absence
//! of capital losses applied against its gain.** That qualifier is the whole
//! honesty of the report, and it is stated on every year's row as well as
//! here, because the two methods do not meet losses at the same point:
//!
//! - Under the **discount** method, capital losses are subtracted from the
//!   gross gain and the 50% discount applies to what is left
//!   (`docs/ato/cgt-using-capital-losses.md`).
//! - Under **indexation** the gain is already net of the indexed cost base
//!   and no discount follows, so a loss comes straight off it.
//!
//! Writing the gross gain `g = proceeds − cost base`, the indexation uplift
//! `r = indexed cost base − cost base`, and `L` for the losses applied to
//! this gain, the discount method assesses `(g − L) / 2` and indexation
//! assesses `g − r − L`. Indexation's advantage is therefore
//! `r − (g − L) / 2`, which **rises** with `L` until `L` reaches `g − r` and
//! both methods reach nil together. So applying losses never moves the answer
//! toward the discount: a row reported here as "Discount" may flip to
//! "Indexation" once losses enter, and a row reported as "Indexation" can
//! only become more so, up to the point where enough losses make the choice
//! moot. **The rows are a floor on indexation's case, not the whole answer** —
//! which is the ATO's own caveat ("Indexation may give you a better result in
//! some situations, such as if you also have capital losses").
//!
//! The comparison is stated **per parcel allocation** rather than per
//! disposal or per year because that is the only level at which it is a fact
//! rather than an assumption: the election is made per CGT asset, a parcel is
//! a separate CGT asset (`docs/ato/cgt-keeping-records-shares.md`), and a
//! single Sell can draw on a 1998 parcel and a 2015 one whose methods differ.
//! Summing the per-parcel results into a year total is offered on the
//! `years` table, but it is a sum of independent per-parcel choices, not a
//! year-level election, and it inherits the same loss caveat — each year row
//! therefore carries the capital losses actually realised in that year, so a
//! reader can see at once whether the qualifier bites.
//!
//! # Which allocations appear
//!
//! - The parcel's cost was incurred **on or before 21 September 1999** —
//!   tested on the parcel's own **trade date**, when the cost was actually
//!   incurred, not on the CGT acquisition date the discount clock runs from
//!   (which may be a *deemed* date carried from a rollover or an
//!   inheritance). A deemed-date parcel has its own indexation rules — an
//!   inherited asset indexes only where the death was before 21 September
//!   1999, with the deceased's own indexed cost base carried over — and none
//!   of them are modelled, so those parcels are left out rather than
//!   compared on the wrong date.
//! - The allocation produced a **capital gain**. Indexation cannot be used on
//!   a capital loss at all, so a loss allocation is not "the discount wins" —
//!   there is no comparison to make, and none is shown. Loss allocations
//!   still reach the report as part of their year's `capital_losses_realised`.
//! - The disposal is a **Sell**. A rights sale is excluded: what would be
//!   indexed is the rights' own cost base (nil for the free rights modelled
//!   here, and indexing nil gives nil), and the anchoring parcel's date is
//!   the discount clock's, not the date a cost of the rights was incurred.
//!
//! # Costs incurred after 21 September 1999
//!
//! Only a parcel's **acquisition** cost is indexable, and in this data model
//! that is the whole of the indexable cost base — price × quantity plus
//! brokerage and GST, all incurred at the trade date. The two later movements the
//! cost-base pipeline knows about — an AMIT cost-base reduction (CGT event
//! E10) and a return of capital (G1) — are **reductions**, not costs, so
//! there is no question of indexing them; they come off the indexed figure at
//! face value (`domain::indexation::indexed_cost_base`), which is the
//! conservative direction. A disposal's own brokerage is netted from the
//! proceeds here rather than added to the cost base, so it never enters an
//! indexed figure either — and it would not be indexable if it did, being
//! incurred at the sale.

use crate::domain::indexation::CpiQuarters;
use crate::domain::tax_year::tax_year_for;
use crate::infra::decimal::to_cents;
use crate::infra::http::ApiError;
use axum::{Json, Router, extract::State, routing::get};
use chrono::NaiveDate;
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use sqlx::{Row, SqlitePool};
use std::collections::HashMap;

/// One parcel allocation of one disposal, with the two methods' assessable
/// gains set against each other. Every figure is AUD.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IndexationComparison {
    /// The financial year the disposal falls in (calendar year of its 30 June
    /// end) — the key of the matching `years` row.
    pub tax_year: i32,
    /// The Sell trade this allocation belongs to.
    pub sale_trade_id: i64,
    pub listing_id: i64,
    pub ticker: String,
    pub sale_date: NaiveDate,
    /// The Buy/DRP parcel the units came from.
    pub purchase_trade_id: i64,
    /// The parcel's own trade date — when the indexable cost was incurred.
    pub cost_incurred_date: NaiveDate,
    /// End date of the CPI quarter that date falls in — the row of the ATO's
    /// published table (`docs/ato/consumer-price-index.md`) the factor reads.
    pub cpi_quarter_end: NaiveDate,
    /// That quarter's CPI, verbatim as published.
    pub cpi: Decimal,
    /// 68.7 (the September 1999 CPI, where indexation is frozen) ÷ `cpi`,
    /// limited to 3 decimal places.
    pub indexation_factor: Decimal,
    /// Units allocated from the parcel (sale-date unit basis).
    pub units: Decimal,
    /// This allocation's share of the disposal's net proceeds.
    pub proceeds: Decimal,
    /// The adjusted cost base actually used by every report — what the
    /// discount method assesses against.
    pub cost_base: Decimal,
    /// The same units' cost base indexed to the September 1999 quarter.
    pub indexed_cost_base: Decimal,
    /// The assessable gain under the method this system uses: the gross gain
    /// halved where the parcel is discount-eligible (all of these are, being
    /// held since 1999 or earlier). **Before** any capital losses — see the
    /// module doc.
    pub discount_method_gain: Decimal,
    /// The assessable gain under the indexation method: proceeds less the
    /// indexed cost base, floored at nil, with no discount. Before any
    /// capital losses.
    pub indexation_method_gain: Decimal,
    /// Which method assesses less on this allocation, absent capital losses:
    /// `Indexation`, `Discount`, or `Equal`.
    pub better_method: String,
    /// `discount_method_gain − indexation_method_gain` — what choosing
    /// indexation on this allocation would take off the assessable gain.
    /// Negative where the discount is the better choice.
    pub indexation_advantage: Decimal,
}

/// One financial year's roll-up of the comparisons above, with the fact that
/// decides whether they can be read at face value: the capital losses the
/// year actually realised.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IndexationYear {
    pub tax_year: i32,
    /// How many parcel allocations in the year are indexation-eligible gains.
    pub eligible_allocations: i64,
    /// Sum of the year's `discount_method_gain`, over eligible allocations
    /// only — never the year's whole assessable gain.
    pub discount_method_total: Decimal,
    /// Sum of the year's `indexation_method_gain`, over the same allocations.
    pub indexation_method_total: Decimal,
    /// `discount_method_total − indexation_method_total`.
    pub indexation_advantage_total: Decimal,
    /// Capital losses realised in the year across **all** disposals, eligible
    /// or not — the quantity that decides whether the per-parcel comparison
    /// can be read at face value. Brought-forward losses are not included
    /// (see [net capital gain](crate::reports::net_capital_gain) for those);
    /// they move the comparison the same way.
    pub capital_losses_realised: Decimal,
    /// The comparison this year's rows are actually making, in words — the
    /// qualifier that makes them honest, restated per year so it travels with
    /// the figures into a CSV or a printout.
    pub comparison: String,
}

/// The report: the per-allocation comparisons and their per-year roll-up.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IndexationCrossCheck {
    pub comparisons: Vec<IndexationComparison>,
    pub years: Vec<IndexationYear>,
}

pub fn router() -> Router<SqlitePool> {
    Router::new().route("/reports/indexation_cross_check", get(report))
}

/// Build the report. Every input is read on **one** read transaction — the
/// realised-gains rows, the parcels' trade dates, the tickers and the CPI
/// series — so the comparison can never pair a disposal with a parcel that a
/// concurrent write has moved.
pub async fn db_indexation_cross_check(
    pool: &SqlitePool,
) -> Result<IndexationCrossCheck, sqlx::Error> {
    let mut tx = pool.begin().await?;
    let disposals = super::realised_gains::db_realised_gains_on(&mut tx).await?;
    // The parcels' own trade dates: `ParcelDetail` carries the CGT
    // acquisition date (possibly deemed), and indexation turns on the date
    // the cost was incurred instead.
    let parcel_rows = sqlx::query("SELECT id, date FROM trades WHERE trade_type IN ('Buy', 'DRP')")
        .fetch_all(&mut *tx)
        .await?;
    let ticker_rows = sqlx::query("SELECT id, ticker FROM listings")
        .fetch_all(&mut *tx)
        .await?;
    let cpi = CpiQuarters::load(&mut tx).await?;
    tx.commit().await?;

    let mut incurred: HashMap<i64, NaiveDate> = HashMap::with_capacity(parcel_rows.len());
    for row in &parcel_rows {
        incurred.insert(row.try_get("id")?, row.try_get("date")?);
    }
    let mut tickers: HashMap<i64, String> = HashMap::with_capacity(ticker_rows.len());
    for row in &ticker_rows {
        tickers.insert(row.try_get("id")?, row.try_get("ticker")?);
    }

    let mut comparisons = Vec::new();
    // Capital losses realised per year, over every disposal — the qualifier
    // each year's rows are read under.
    let mut losses: HashMap<i32, Decimal> = HashMap::new();
    for disposal in &disposals {
        let tax_year = tax_year_for(disposal.sale_date);
        *losses.entry(tax_year).or_insert(Decimal::ZERO) += disposal.capital_loss;
        for parcel in &disposal.parcels {
            // `indexed_cost_base` is `Some` exactly when the allocation is an
            // indexation-eligible gain — the realised-gains report has
            // already applied both tests, so they are not restated here and
            // cannot drift from it.
            let Some(indexed_cost_base) = parcel.indexed_cost_base else {
                continue;
            };
            let (Some(cost_incurred_date), Some(ticker)) = (
                incurred.get(&parcel.purchase_trade_id).copied(),
                tickers.get(&disposal.listing_id).cloned(),
            ) else {
                continue;
            };
            // The same lookup that produced `indexed_cost_base`, for the
            // working the row shows; an absent one would mean the parcel is
            // not eligible after all, so the row goes rather than guessing.
            let Some(ix) = cpi.indexation_for(cost_incurred_date) else {
                continue;
            };
            let discount_method_gain = if parcel.discount_eligible {
                parcel.capital_gain_loss / Decimal::TWO
            } else {
                parcel.capital_gain_loss
            };
            let indexation_method_gain = (parcel.proceeds - indexed_cost_base).max(Decimal::ZERO);
            let indexation_advantage = discount_method_gain - indexation_method_gain;
            // Compared at the cent the figures are reported to: a difference
            // below half a cent is not a reason to tell a taxpayer one lawful
            // method beats another.
            let better_method = match to_cents(indexation_advantage) {
                d if d > Decimal::ZERO => "Indexation",
                d if d < Decimal::ZERO => "Discount",
                _ => "Equal",
            };
            comparisons.push(IndexationComparison {
                tax_year,
                sale_trade_id: disposal.sale_trade_id,
                listing_id: disposal.listing_id,
                ticker,
                sale_date: disposal.sale_date,
                purchase_trade_id: parcel.purchase_trade_id,
                cost_incurred_date,
                cpi_quarter_end: ix.quarter_end,
                cpi: ix.cpi,
                indexation_factor: ix.factor,
                units: parcel.units,
                proceeds: parcel.proceeds,
                cost_base: parcel.cost_base,
                indexed_cost_base,
                discount_method_gain,
                indexation_method_gain,
                better_method: better_method.to_string(),
                indexation_advantage,
            });
        }
    }
    comparisons.sort_by(|a, b| {
        a.sale_date
            .cmp(&b.sale_date)
            .then(a.sale_trade_id.cmp(&b.sale_trade_id))
            .then(a.purchase_trade_id.cmp(&b.purchase_trade_id))
    });

    let mut years: HashMap<i32, IndexationYear> = HashMap::new();
    for c in &comparisons {
        let year = years.entry(c.tax_year).or_insert_with(|| IndexationYear {
            tax_year: c.tax_year,
            eligible_allocations: 0,
            discount_method_total: Decimal::ZERO,
            indexation_method_total: Decimal::ZERO,
            indexation_advantage_total: Decimal::ZERO,
            capital_losses_realised: losses.get(&c.tax_year).copied().unwrap_or(Decimal::ZERO),
            comparison: String::new(),
        });
        year.eligible_allocations += 1;
        year.discount_method_total += c.discount_method_gain;
        year.indexation_method_total += c.indexation_method_gain;
        year.indexation_advantage_total += c.indexation_advantage;
    }
    let mut years: Vec<IndexationYear> = years.into_values().collect();
    for year in &mut years {
        year.comparison = comparison_sentence(year.capital_losses_realised);
    }
    years.sort_by_key(|y| y.tax_year);

    Ok(IndexationCrossCheck { comparisons, years })
}

/// The qualifier every row is read under, stated in the year's own terms.
/// Both branches say the same thing — losses can only move the comparison
/// toward indexation — but the wording changes on whether the year actually
/// has any, so a reader is never left to work out whether the caveat applies
/// to them.
fn comparison_sentence(capital_losses_realised: Decimal) -> String {
    if capital_losses_realised > Decimal::ZERO {
        format!(
            "Each row compares one parcel's two methods before any capital losses. This year \
             realised A${:.2} of capital losses: losses are applied before the 50% discount but come \
             straight off an indexed gain, so applying them to these gains can only move the \
             comparison further toward indexation (until enough losses reduce both methods to \
             nil). Read these rows as a floor on indexation's case, not the whole answer.",
            to_cents(capital_losses_realised)
        )
    } else {
        "Each row compares one parcel's two methods before any capital losses. This year \
         realised no capital losses, so the comparison stands as shown — though a loss brought \
         forward from an earlier year, applied to these gains at return time, would move it \
         further toward indexation."
            .to_string()
    }
}

async fn report(State(pool): State<SqlitePool>) -> Result<Json<IndexationCrossCheck>, ApiError> {
    db_indexation_cross_check(&pool)
        .await
        .map(Json)
        .map_err(ApiError::from)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{self, ApiClient, dec, test_pool, ymd};
    use axum::http::StatusCode;

    fn client(pool: &SqlitePool) -> ApiClient {
        ApiClient::over(router().with_state(pool.clone()))
    }

    /// SCENARIOS AA-a's own parcel: 1,000 units bought on 20 September 1985
    /// (the earliest acquisition this system accepts) for A$10 each, all sold
    /// on 2 June 2025 for A$20 each.
    async fn the_findings_parcel(pool: &SqlitePool) {
        test_support::listing(1).ticker("OLD").insert(pool).await;
        test_support::buy(1, 1)
            .date(ymd(1985, 9, 20))
            .qty(dec("1000"))
            .price(dec("10"))
            .insert(pool)
            .await;
        test_support::sell(2, 1)
            .date(ymd(2025, 6, 2))
            .qty(dec("1000"))
            .price(dec("20"))
            .insert(pool)
            .await;
        test_support::allocate(pool, 1, 2, 1, dec("1000")).await;
    }

    /// The finding, reproduced and answered: the disposal the system reports a
    /// A$5,000 discounted gain on is named here with its A$2,700 indexation
    /// alternative beside it, and indexation is called the better method.
    ///
    /// Note the figures against the finding's own write-up, which quoted a
    /// 1.731 factor and ~A$2,690: the ATO's published table gives
    /// 68.7 ÷ 39.7 = 1.730478…, and the rounding rule limits that to **1.730**
    /// (the fourth decimal is a 4, so it rounds down).
    #[tokio::test]
    async fn the_findings_disposal_is_named_with_both_figures() {
        let pool = test_pool().await;
        the_findings_parcel(&pool).await;

        let report = db_indexation_cross_check(&pool).await.unwrap();
        assert_eq!(report.comparisons.len(), 1);
        let c = &report.comparisons[0];
        assert_eq!(c.ticker, "OLD");
        assert_eq!(c.tax_year, 2025);
        assert_eq!(c.cost_incurred_date, ymd(1985, 9, 20));
        assert_eq!(c.cpi_quarter_end, ymd(1985, 9, 30));
        assert_eq!(c.cpi, dec("39.7"));
        assert_eq!(c.indexation_factor, dec("1.730"));
        assert_eq!(c.cost_base, dec("10000"));
        assert_eq!(c.indexed_cost_base, dec("17300.000"));
        assert_eq!(c.proceeds, dec("20000"));
        assert_eq!(c.discount_method_gain, dec("5000"));
        assert_eq!(c.indexation_method_gain, dec("2700.000"));
        assert_eq!(c.indexation_advantage, dec("2300.000"));
        assert_eq!(c.better_method, "Indexation");
    }

    /// The crossover the corrected Known-limitations wording states: with a
    /// 1.730 factor indexation wins below 2.460 × cost and the discount wins
    /// above it. Driven at both sides of the boundary on the same parcel, so
    /// the report's verdict is what pins the arithmetic rather than a
    /// paragraph.
    #[tokio::test]
    async fn the_crossover_is_two_point_four_six_times_the_cost() {
        for (price, expected) in [
            ("24.59", "Indexation"),
            ("24.60", "Equal"),
            ("24.61", "Discount"),
        ] {
            let pool = test_pool().await;
            test_support::listing(1).ticker("OLD").insert(&pool).await;
            test_support::buy(1, 1)
                .date(ymd(1985, 9, 20))
                .qty(dec("1000"))
                .price(dec("10"))
                .insert(&pool)
                .await;
            test_support::sell(2, 1)
                .date(ymd(2025, 6, 2))
                .qty(dec("1000"))
                .price(dec(price))
                .insert(&pool)
                .await;
            test_support::allocate(&pool, 1, 2, 1, dec("1000")).await;

            let report = db_indexation_cross_check(&pool).await.unwrap();
            assert_eq!(
                report.comparisons[0].better_method, expected,
                "at {price} per unit against a $10 cost"
            );
        }
    }

    /// A parcel bought after 21 September 1999 is not in the report at all —
    /// the method is unavailable, so there is nothing to compare — and the
    /// boundary is exact on the parcel's own trade date.
    #[tokio::test]
    async fn the_twenty_first_of_september_1999_boundary_is_exact() {
        for (buy_date, rows) in [(ymd(1999, 9, 21), 1), (ymd(1999, 9, 22), 0)] {
            let pool = test_pool().await;
            test_support::listing(1).ticker("OLD").insert(&pool).await;
            test_support::buy(1, 1)
                .date(buy_date)
                .qty(dec("100"))
                .price(dec("10"))
                .insert(&pool)
                .await;
            test_support::sell(2, 1)
                .date(ymd(2025, 6, 2))
                .qty(dec("100"))
                .price(dec("30"))
                .insert(&pool)
                .await;
            test_support::allocate(&pool, 1, 2, 1, dec("100")).await;

            let report = db_indexation_cross_check(&pool).await.unwrap();
            assert_eq!(report.comparisons.len(), rows, "bought {buy_date}");
        }
    }

    /// A parcel disposed of at a **loss** is excluded rather than reported as
    /// "the discount wins": indexation cannot be used on a capital loss at
    /// all, so there is no comparison to make. Its loss still reaches the
    /// year row, because it is exactly what the year's qualifier is about.
    #[tokio::test]
    async fn a_loss_disposal_is_excluded_but_its_loss_is_counted() {
        let pool = test_pool().await;
        the_findings_parcel(&pool).await;
        // A second, loss-making 1985 parcel sold in the same year.
        test_support::buy(3, 1)
            .date(ymd(1985, 9, 20))
            .qty(dec("500"))
            .price(dec("40"))
            .insert(&pool)
            .await;
        test_support::sell(4, 1)
            .date(ymd(2025, 6, 3))
            .qty(dec("500"))
            .price(dec("30"))
            .insert(&pool)
            .await;
        test_support::allocate(&pool, 2, 4, 3, dec("500")).await;

        let report = db_indexation_cross_check(&pool).await.unwrap();
        assert_eq!(report.comparisons.len(), 1);
        assert_eq!(report.comparisons[0].purchase_trade_id, 1);
        assert_eq!(report.years.len(), 1);
        let year = &report.years[0];
        assert_eq!(year.eligible_allocations, 1);
        assert_eq!(year.capital_losses_realised, dec("5000"));
        assert!(
            year.comparison.contains("A$5000.00 of capital losses"),
            "{}",
            year.comparison
        );
        assert!(year.comparison.contains("further toward indexation"));
    }

    /// With no capital losses in the year the qualifier says so plainly
    /// rather than going silent — the reader is never left to work out
    /// whether the caveat applies to them.
    #[tokio::test]
    async fn a_year_with_no_losses_says_the_comparison_stands() {
        let pool = test_pool().await;
        the_findings_parcel(&pool).await;

        let report = db_indexation_cross_check(&pool).await.unwrap();
        let year = &report.years[0];
        assert_eq!(year.capital_losses_realised, Decimal::ZERO);
        assert!(year.comparison.contains("realised no capital losses"));
        assert!(year.comparison.contains("before any capital losses"));
        assert_eq!(year.discount_method_total, dec("5000"));
        assert_eq!(year.indexation_method_total, dec("2700.000"));
        assert_eq!(year.indexation_advantage_total, dec("2300.000"));
    }

    /// A modern-only portfolio has nothing to say: an empty report means no
    /// disposal this year could have used the indexation method.
    #[tokio::test]
    async fn a_post_1999_portfolio_reports_nothing() {
        let pool = test_pool().await;
        test_support::listing(1).insert(&pool).await;
        test_support::buy(1, 1)
            .date(ymd(2015, 1, 5))
            .qty(dec("100"))
            .price(dec("10"))
            .insert(&pool)
            .await;
        test_support::sell(2, 1)
            .date(ymd(2025, 6, 2))
            .qty(dec("100"))
            .price(dec("30"))
            .insert(&pool)
            .await;
        test_support::allocate(&pool, 1, 2, 1, dec("100")).await;

        let report = db_indexation_cross_check(&pool).await.unwrap();
        assert!(report.comparisons.is_empty());
        assert!(report.years.is_empty());
    }

    /// The endpoint answers the same object over HTTP.
    #[tokio::test]
    async fn api_get_reports_the_comparison() {
        let pool = test_pool().await;
        the_findings_parcel(&pool).await;

        let response = client(&pool).get("/reports/indexation_cross_check").await;
        assert_eq!(response.status, StatusCode::OK);
        let body: IndexationCrossCheck = response.json();
        assert_eq!(body.comparisons.len(), 1);
        assert_eq!(body.comparisons[0].better_method, "Indexation");
        assert_eq!(body.years.len(), 1);
    }

    /// **The promise this whole change is made under**: no reported tax
    /// figure moves. The finding's own disposal reports the same A$5,000
    /// discount-method gain and the same net capital gain it did before the
    /// indexed figure existed — the alternative is reported *beside* it, never
    /// instead of it.
    #[tokio::test]
    async fn no_reported_tax_figure_changes() {
        let pool = test_pool().await;
        the_findings_parcel(&pool).await;
        let api = ApiClient::full(&pool);

        let realised: serde_json::Value = api.get_json("/portfolio/realised-gains").await;
        assert_eq!(realised[0]["capital_gain_loss"], "10000");
        assert_eq!(realised[0]["discount_eligible_gain"], "10000");
        // The advisory pair rides along on the parcel row, summed into
        // nothing.
        let parcel = &realised[0]["parcels"][0];
        assert_eq!(parcel["cost_base"], "10000");
        assert_eq!(parcel["indexation_eligible"], serde_json::json!(true));
        assert_eq!(parcel["indexed_cost_base"], "17300.000");

        let ncg: serde_json::Value = api.get_json("/portfolio/net-capital-gain").await;
        let year = ncg
            .as_array()
            .unwrap()
            .iter()
            .find(|y| y["tax_year"] == 2025)
            .expect("the disposal's year");
        assert_eq!(year["discount_eligible_gains"], "10000");
        assert_eq!(year["cgt_discount"], "5000");
        assert_eq!(year["net_capital_gain"], "5000");
    }
}
