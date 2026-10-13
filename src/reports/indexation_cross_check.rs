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
//! # The loss-aware optimum
//!
//! Each `years` row also answers the question the per-parcel rows cannot:
//! with the year's losses actually applied, which **units** should be
//! indexed? Every share is a separate CGT asset, so a parcel can be split —
//! the Personal Investors Guide's Example 18 (Clare) indexes just enough
//! shares of a 1995 parcel to absorb her losses and discounts the rest, which
//! assesses less than either method over the whole parcel
//! (`docs/ato/personal-investors-guide-shares-and-units.md`).
//!
//! The optimum is **per year, across every eligible allocation in it**:
//! losses are a year-level pool (this year's plus the balance brought
//! forward), so the parcels compete for them and no parcel's best split can
//! be stated on its own. The losses are netted in the guide's Step 8 order —
//! against 'other' (non-discountable) gains first, then indexed gains, then
//! discount gains — and the 50% discount applies to what is left of the last.
//! Writing `R` for the losses left after the 'other' gains, a unit with gross
//! gain `g` and indexed gain `h` (both per unit) does one of two things when
//! indexed:
//!
//! - while the indexed gains taken so far are within `R`, the loss absorbs
//!   its `h` and its `g` leaves the discount pool, so the year's assessable
//!   figure falls by `(g − h) / 2` for `h` of loss consumed — the units that
//!   save most per dollar of loss (`(g − h) / h`) go first;
//! - past `R` it is assessed in full, so it is indexed only where `h < g / 2`
//!   — the parcel's own before-losses comparison.
//!
//! The unit at which the losses run out is the one judgement call: indexing
//! it leaves its unabsorbed indexed gain assessed in full, not indexing it
//! leaves the remaining loss to come off discount gains at half value. The
//! whole-unit count either side is evaluated and the lower kept — for Clare
//! that is **rounding down** (395 shares, \$521.89, against the ATO's 396 and
//! \$522.62), which is the usual answer but not the only one. Fractional
//! units are not split: the count indexed is whole, except that a whole
//! allocation is indexed with its fraction. The chosen split is checked
//! against indexing nothing and indexing everything, and never reported worse
//! than either.
//!
//! The brought-forward balance is the one the [net capital
//! gain](crate::reports::net_capital_gain) chain gives the year — which
//! assumes the discount was applied in every earlier year. Advisory, like the
//! rest: the year's reported net capital gain is unchanged, and the optimum's
//! figures are exact (not rounded to the cent as that report's are).
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
use crate::infra::decimal::{mul_div, to_cents};
use crate::infra::extract::Json;
use crate::infra::http::ApiError;
use crate::infra::id::ListingId;
use crate::infra::id::TradeId;
use axum::extract::State;
use chrono::NaiveDate;
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use sqlx::{Row, SqlitePool};
use std::collections::HashMap;
use utoipa_axum::{router::OpenApiRouter, routes};

/// One parcel allocation of one disposal, with the two methods' assessable
/// gains set against each other. Every figure is AUD.
#[derive(utoipa::ToSchema, Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IndexationComparison {
    /// The financial year the disposal falls in (calendar year of its 30 June
    /// end) — the key of the matching `years` row.
    pub tax_year: i32,
    /// The Sell trade this allocation belongs to.
    pub sale_trade_id: TradeId,
    pub listing_id: ListingId,
    pub ticker: String,
    pub sale_date: NaiveDate,
    /// The Buy/DRP parcel the units came from.
    pub purchase_trade_id: TradeId,
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
    /// How many of `units` the year's **loss-aware optimum** indexes (the
    /// rest take the discount) — see the module doc. Whole units; `0` where
    /// the discount is better for all of them even with the year's losses
    /// applied, `units` where indexation is better for all of them.
    pub loss_aware_units_indexed: Decimal,
}

/// One financial year's roll-up of the comparisons above, with the fact that
/// decides whether they can be read at face value: the capital losses the
/// year actually realised.
#[derive(utoipa::ToSchema, Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
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
    /// The net capital loss brought forward into the year (the [net capital
    /// gain](crate::reports::net_capital_gain) report's chain, which assumes
    /// the discount throughout every earlier year). With
    /// `capital_losses_realised`, the loss pool the three figures below net.
    pub capital_loss_brought_forward: Decimal,
    /// The year's net capital gain with the discount applied to every
    /// eligible allocation — the [net capital
    /// gain](crate::reports::net_capital_gain) report's figure, unrounded.
    pub discount_throughout_net_capital_gain: Decimal,
    /// The year's net capital gain with **every** eligible allocation
    /// indexed instead, losses applied.
    pub indexation_throughout_net_capital_gain: Decimal,
    /// The year's net capital gain with each allocation's
    /// `loss_aware_units_indexed` indexed and the rest discounted, losses
    /// applied — the lowest of the three.
    pub loss_aware_net_capital_gain: Decimal,
    /// `discount_throughout_net_capital_gain − loss_aware_net_capital_gain`:
    /// what the loss-aware split would take off the year's reported figure.
    pub loss_aware_saving: Decimal,
    /// The comparison this year's rows are actually making, in words — the
    /// qualifier that makes them honest, restated per year so it travels with
    /// the figures into a CSV or a printout.
    pub comparison: String,
}

/// The report: the per-allocation comparisons and their per-year roll-up.
#[derive(utoipa::ToSchema, Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IndexationCrossCheck {
    pub comparisons: Vec<IndexationComparison>,
    pub years: Vec<IndexationYear>,
}

pub fn router() -> OpenApiRouter<SqlitePool> {
    OpenApiRouter::new().routes(routes!(report))
}

/// Build the report. Every input is read on **one** read transaction — the
/// net-capital-gain year rows (and the realised disposals they carry), the
/// parcels' trade dates, the tickers and the CPI series — so the comparison
/// can never pair a disposal with a parcel that a concurrent write has moved,
/// nor a year's loss pool with disposals from another snapshot.
pub async fn db_indexation_cross_check(
    pool: &SqlitePool,
) -> Result<IndexationCrossCheck, sqlx::Error> {
    let mut tx = crate::infra::db::read_tx(pool).await?;
    let ncg_years = super::net_capital_gain::db_net_capital_gain_on(&mut tx).await?;
    // The parcels' own trade dates: `ParcelDetail` carries the CGT
    // acquisition date (possibly deemed), and indexation turns on the date
    // the cost was incurred instead.
    let parcel_rows = sqlx::query("SELECT id, date FROM trades WHERE trade_type IN ('Buy', 'DRP')")
        .fetch_all(&mut *tx)
        .await?;
    let tickers = crate::entities::listing::db_tickers(&mut *tx).await?;
    let cpi = CpiQuarters::load(&mut tx).await?;
    tx.commit().await?;

    let mut incurred: HashMap<TradeId, NaiveDate> = HashMap::with_capacity(parcel_rows.len());
    for row in &parcel_rows {
        incurred.insert(row.try_get("id")?, row.try_get("date")?);
    }

    // Each comparison beside the two per-allocation figures the loss-aware
    // optimum works from (gross gain, discount eligibility), which the row
    // does not publish.
    let mut rows: Vec<(IndexationComparison, (Decimal, bool))> = Vec::new();
    // Capital losses realised per year, over every disposal — the qualifier
    // each year's rows are read under.
    let mut losses: HashMap<i32, Decimal> = HashMap::new();
    for disposal in ncg_years.iter().flat_map(|y| &y.disposals) {
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
            // Only a Sell is ever indexation-eligible (a rights sale's
            // `indexed_cost_base` is always `None`), so this skips nothing
            // the line above has not — it names the trade the row cites.
            let (Some(sale_trade_id), Some(cost_incurred_date), Some(ticker)) = (
                disposal.disposal.sell(),
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
            let comparison = IndexationComparison {
                tax_year,
                sale_trade_id,
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
                // Set per year below, once every allocation competing for
                // the year's losses is known.
                loss_aware_units_indexed: Decimal::ZERO,
            };
            rows.push((
                comparison,
                (parcel.capital_gain_loss, parcel.discount_eligible),
            ));
        }
    }
    rows.sort_by(|(a, _), (b, _)| {
        a.sale_date
            .cmp(&b.sale_date)
            .then(a.sale_trade_id.cmp(&b.sale_trade_id))
            .then(a.purchase_trade_id.cmp(&b.purchase_trade_id))
    });
    let (mut comparisons, gross): (Vec<IndexationComparison>, Vec<(Decimal, bool)>) =
        rows.into_iter().unzip();

    let ncg_by_year: HashMap<i32, &super::net_capital_gain::NetCapitalGainYear> =
        ncg_years.iter().map(|y| (y.tax_year, y)).collect();
    let mut years: Vec<IndexationYear> = Vec::new();
    let mut tax_years: Vec<i32> = comparisons.iter().map(|c| c.tax_year).collect();
    tax_years.dedup();
    for tax_year in tax_years {
        let members: Vec<usize> = (0..comparisons.len())
            .filter(|&i| comparisons[i].tax_year == tax_year)
            .collect();
        let ncg = ncg_by_year.get(&tax_year);
        let brought_forward = ncg.map_or(Decimal::ZERO, |y| y.capital_loss_brought_forward);
        // The optimum's candidates are the discount-eligible allocations —
        // the choice it makes is "discount or index" for each unit. An
        // eligible allocation that is *not* discount-eligible would need a
        // deemed acquisition date inside 12 months of the sale, which the
        // realised-gains eligibility test already excludes; were one to
        // appear, its gain stays where the net-capital-gain report put it,
        // among the 'other' gains, and none of its units is indexed.
        let candidates: Vec<usize> = members.iter().copied().filter(|&i| gross[i].1).collect();
        let cands: Vec<Candidate> = candidates
            .iter()
            .map(|&i| Candidate {
                units: comparisons[i].units,
                gain: gross[i].0,
                indexed_gain: comparisons[i].indexation_method_gain,
            })
            .collect();
        let candidate_gains: Decimal = cands.iter().map(|c| c.gain).sum();
        let pool = LossPool {
            other_gains: ncg.map_or(Decimal::ZERO, |y| y.other_gains),
            // The year's discount-eligible gains other than the candidates'
            // (later parcels, AMMA discount gains, E10/G1/C2): the report's
            // figure is at the cent and the candidates' exact, so a sub-cent
            // difference is floored rather than going negative.
            other_discount_gains: ncg
                .map_or(Decimal::ZERO, |y| {
                    y.discount_eligible_gains - candidate_gains
                })
                .max(Decimal::ZERO),
            losses: losses.get(&tax_year).copied().unwrap_or(Decimal::ZERO) + brought_forward,
        };
        let none = vec![Decimal::ZERO; cands.len()];
        let all: Vec<Decimal> = cands.iter().map(|c| c.units).collect();
        let indexed = loss_aware_optimum(&pool, &cands);
        for (&i, &x) in candidates.iter().zip(&indexed) {
            comparisons[i].loss_aware_units_indexed = x;
        }
        let discount_throughout = pool.assessed(&cands, &none);
        let loss_aware = pool.assessed(&cands, &indexed);

        let mut year = IndexationYear {
            tax_year,
            eligible_allocations: 0,
            discount_method_total: Decimal::ZERO,
            indexation_method_total: Decimal::ZERO,
            indexation_advantage_total: Decimal::ZERO,
            capital_losses_realised: losses.get(&tax_year).copied().unwrap_or(Decimal::ZERO),
            capital_loss_brought_forward: brought_forward,
            discount_throughout_net_capital_gain: discount_throughout,
            indexation_throughout_net_capital_gain: pool.assessed(&cands, &all),
            loss_aware_net_capital_gain: loss_aware,
            loss_aware_saving: discount_throughout - loss_aware,
            comparison: String::new(),
        };
        for &i in &members {
            let c = &comparisons[i];
            year.eligible_allocations += 1;
            year.discount_method_total += c.discount_method_gain;
            year.indexation_method_total += c.indexation_method_gain;
            year.indexation_advantage_total += c.indexation_advantage;
        }
        year.comparison = comparison_sentence(year.capital_losses_realised, brought_forward);
        years.push(year);
    }
    years.sort_by_key(|y| y.tax_year);

    Ok(IndexationCrossCheck { comparisons, years })
}

/// One discount-eligible allocation the loss-aware optimum may index, in
/// whole-allocation figures: `gain` is the gross gain the discount method
/// halves, `indexed_gain` the indexation method's gain (both AUD).
#[derive(Debug, Clone)]
struct Candidate {
    units: Decimal,
    gain: Decimal,
    indexed_gain: Decimal,
}

impl Candidate {
    /// Whether indexing beats the discount on this allocation with no loss
    /// to absorb it — the per-parcel comparison: `indexed_gain < gain / 2`.
    fn wins_alone(&self) -> bool {
        self.indexed_gain * Decimal::TWO < self.gain
    }

    /// The gross and indexed gains of `x` of the allocation's units.
    fn share(&self, x: Decimal) -> (Decimal, Decimal) {
        if x == self.units {
            return (self.gain, self.indexed_gain);
        }
        (
            mul_div(&[self.gain, x], self.units),
            mul_div(&[self.indexed_gain, x], self.units),
        )
    }
}

/// A year's gains outside the candidates and its loss pool.
#[derive(Debug, Clone)]
struct LossPool {
    /// Non-discountable gains — netted against the losses first.
    other_gains: Decimal,
    /// Discount-eligible gains that are not candidates' — they stay on the
    /// discount method whatever is chosen.
    other_discount_gains: Decimal,
    /// This year's capital losses plus the balance brought forward.
    losses: Decimal,
}

impl LossPool {
    /// The year's net capital gain with `indexed[i]` of candidate `i`'s units
    /// indexed and the rest discounted: losses netted against the 'other'
    /// gains, then the indexed gains, then the discount gains (Part B Step 8),
    /// and the 50% discount on what is left of the last. Exact — no cent
    /// rounding.
    fn assessed(&self, cands: &[Candidate], indexed: &[Decimal]) -> Decimal {
        let (mut indexed_gains, mut discount_gains) = (Decimal::ZERO, self.other_discount_gains);
        for (c, &x) in cands.iter().zip(indexed) {
            let (g, h) = c.share(x);
            indexed_gains += h;
            discount_gains += c.gain - g;
        }
        let other_left = (self.other_gains - self.losses).max(Decimal::ZERO);
        let loss_left = (self.losses - self.other_gains).max(Decimal::ZERO);
        let indexed_left = (indexed_gains - loss_left).max(Decimal::ZERO);
        let loss_left = (loss_left - indexed_gains).max(Decimal::ZERO);
        let discount_left = (discount_gains - loss_left).max(Decimal::ZERO);
        other_left + indexed_left + discount_left / Decimal::TWO
    }
}

/// The loss-aware optimum: how many units of each candidate to index (see
/// the module doc). Candidates are taken in order of the saving per dollar of
/// loss they consume, `(gain − indexed_gain) / indexed_gain`; a candidate
/// that wins alone is indexed in full wherever it falls, one with no
/// indexation uplift never is, and the rest are indexed while the losses
/// left after the 'other' gains still absorb them. The candidate at which
/// the losses run out is indexed to the whole-unit count either side of the
/// exact point, whichever assesses less. The result is then never worse than
/// indexing nothing or everything — ties keep the fewest units indexed.
fn loss_aware_optimum(pool: &LossPool, cands: &[Candidate]) -> Vec<Decimal> {
    let mut order: Vec<usize> = (0..cands.len())
        .filter(|&i| cands[i].indexed_gain < cands[i].gain)
        .collect();
    // Descending saving per dollar of loss, compared by cross-multiplying so
    // a nil indexed gain (an infinite ratio) needs no division.
    order.sort_by(|&a, &b| {
        let (a_c, b_c) = (&cands[a], &cands[b]);
        let lhs = (b_c.gain - b_c.indexed_gain) * a_c.indexed_gain;
        let rhs = (a_c.gain - a_c.indexed_gain) * b_c.indexed_gain;
        lhs.cmp(&rhs).then(a.cmp(&b))
    });

    let mut indexed = vec![Decimal::ZERO; cands.len()];
    let mut budget = (pool.losses - pool.other_gains).max(Decimal::ZERO);
    for i in order {
        let c = &cands[i];
        if c.wins_alone() || c.indexed_gain <= budget {
            indexed[i] = c.units;
            budget = (budget - c.indexed_gain).max(Decimal::ZERO);
            continue;
        }
        if budget.is_zero() {
            continue;
        }
        // The losses run out inside this allocation: the whole units they
        // absorb in full, and the one more that they absorb only in part.
        let below = mul_div(&[budget, c.units], c.indexed_gain)
            .floor()
            .min(c.units);
        let above = (below + Decimal::ONE).min(c.units);
        indexed[i] = above;
        let assessed_above = pool.assessed(cands, &indexed);
        indexed[i] = below;
        if assessed_above < pool.assessed(cands, &indexed) {
            indexed[i] = above;
            budget = Decimal::ZERO;
        } else {
            budget -= c.share(below).1;
        }
    }

    let none = vec![Decimal::ZERO; cands.len()];
    let all: Vec<Decimal> = cands.iter().map(|c| c.units).collect();
    [none, indexed, all]
        .into_iter()
        .map(|x| (pool.assessed(cands, &x), x))
        .reduce(|best, next| if next.0 < best.0 { next } else { best })
        .map(|(_, x)| x)
        .unwrap_or_default()
}

/// The qualifier every row is read under, stated in the year's own terms.
/// The per-allocation rows are made before any capital losses; the year's
/// loss-aware figures are what applies them. The wording changes on whether
/// the year actually has losses, so a reader is never left to work out
/// whether the caveat applies to them.
fn comparison_sentence(capital_losses_realised: Decimal, brought_forward: Decimal) -> String {
    let rows = "Each row compares one parcel's two methods before any capital losses.";
    if capital_losses_realised.is_zero() && brought_forward.is_zero() {
        return format!(
            "{rows} This year realised no capital losses and brought none forward, so the \
             comparison stands as shown, and the loss-aware figures index exactly the parcels \
             it says indexation wins on."
        );
    }
    let realised = if capital_losses_realised > Decimal::ZERO {
        format!(
            "This year realised A${:.2} of capital losses",
            to_cents(capital_losses_realised)
        )
    } else {
        "This year realised no capital losses".to_string()
    };
    format!(
        "{rows} {realised} and brought forward A${:.2}: losses are applied before the 50% \
         discount but come straight off an indexed gain, so applying them can only move the \
         comparison further toward indexation (until enough losses reduce both methods to nil). \
         The loss-aware figures apply them, indexing just enough units to absorb the losses and \
         discounting the rest.",
        to_cents(brought_forward)
    )
}

#[utoipa::path(
    get,
    path = "/reports/indexation_cross_check",
    summary = "Cross-check the indexation factors against the ATO's published table."
)]
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
        // With no losses the loss-aware split is the per-parcel call.
        assert_eq!(report.comparisons[0].loss_aware_units_indexed, dec("1000"));
        assert_eq!(year.loss_aware_net_capital_gain, dec("2700.000"));
        assert_eq!(year.discount_throughout_net_capital_gain, dec("5000"));
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

    fn cand(units: &str, gain: &str, indexed_gain: &str) -> Candidate {
        Candidate {
            units: dec(units),
            gain: dec(gain),
            indexed_gain: dec(indexed_gain),
        }
    }

    fn pool_with(other_gains: &str, other_discount_gains: &str, losses: &str) -> LossPool {
        LossPool {
            other_gains: dec(other_gains),
            other_discount_gains: dec(other_discount_gains),
            losses: dec(losses),
        }
    }

    /// Clare's parcel (Example 18) as the pure function sees it: 500 units,
    /// a $5,000 gross gain, a $4,422.50 indexed one, $3,500 of losses. The
    /// losses run out inside the 396th unit, and indexing it would assess its
    /// unabsorbed $2.62 in full while saving only half of the $6.225 left —
    /// so the count is rounded **down**.
    #[test]
    fn clares_split_rounds_the_indexed_count_down() {
        let pool = pool_with("0", "0", "3500");
        let cands = [cand("500", "5000", "4422.5")];
        let indexed = loss_aware_optimum(&pool, &cands);
        assert_eq!(indexed, vec![dec("395")]);
        assert_eq!(pool.assessed(&cands, &indexed), dec("521.8875"));
        assert_eq!(pool.assessed(&cands, &[dec("396")]), dec("522.62"));
        assert_eq!(pool.assessed(&cands, &[Decimal::ZERO]), dec("750"));
        assert_eq!(pool.assessed(&cands, &[dec("500")]), dec("922.5"));
    }

    /// The same parcel with more losses, where the other side wins. With
    /// $3,537, 399 units absorb $3,529.155 and leave $7.845 beside the 400th
    /// unit's $8.845 indexed gain: indexing it assesses the unabsorbed $1.00
    /// in full, while leaving it discounted assesses ($10 − $7.845) ÷ 2 =
    /// $1.0775 — so the count is rounded **up**. Rounding down is the usual
    /// answer, not the rule: up wins whenever the loss left exceeds
    /// `2h − g` ($7.69 here).
    #[test]
    fn the_last_unit_is_indexed_when_the_loss_nearly_covers_it() {
        let pool = pool_with("0", "0", "3530");
        let cands = [cand("500", "5000", "4422.5")];
        // $3,530 leaves only $0.845 beside the 400th unit — below $7.69, so
        // down it is.
        assert_eq!(loss_aware_optimum(&pool, &cands), vec![dec("399")]);

        let pool = pool_with("0", "0", "3537");
        let indexed = loss_aware_optimum(&pool, &cands);
        assert_eq!(indexed, vec![dec("400")]);
        assert!(
            pool.assessed(&cands, &indexed) < pool.assessed(&cands, &[dec("399")]),
            "rounding up wins here"
        );
    }

    /// Parcels compete for the year's losses: the one that saves more per
    /// dollar of loss consumed is indexed first. Parcel A (gain $10, indexed
    /// $9 a unit) saves $0.50 for $9 of loss; B (gain $10, indexed $6) saves
    /// $2 for $6 — so with $60 of losses B takes them all.
    #[test]
    fn the_losses_go_to_the_best_saving_per_dollar_first() {
        let pool = pool_with("0", "0", "60");
        let cands = [cand("10", "100", "90"), cand("10", "100", "60")];
        let indexed = loss_aware_optimum(&pool, &cands);
        assert_eq!(indexed, vec![Decimal::ZERO, dec("10")]);
        // B's $60 indexed gain is absorbed and A's $100 is halved: $50.
        // Indexing A instead leaves $30 of its indexed gain assessed in full
        // beside B's $100 halved: $80.
        assert_eq!(pool.assessed(&cands, &indexed), dec("50"));
        assert_eq!(
            pool.assessed(&cands, &[dec("10"), Decimal::ZERO]),
            dec("80")
        );
    }

    /// 'Other' gains take the losses first (Step 8), so they leave nothing
    /// for an indexed gain to absorb — and a parcel indexation wins on alone
    /// is indexed whatever the losses do.
    #[test]
    fn other_gains_absorb_losses_first_and_a_lone_winner_is_always_indexed() {
        let pool = pool_with("100", "0", "100");
        let cands = [cand("10", "100", "90"), cand("10", "100", "40")];
        assert_eq!(
            loss_aware_optimum(&pool, &cands),
            vec![Decimal::ZERO, dec("10")]
        );
        // With no losses at all the optimum is exactly the per-parcel call.
        let pool = pool_with("0", "0", "0");
        assert_eq!(
            loss_aware_optimum(&pool, &cands),
            vec![Decimal::ZERO, dec("10")]
        );
    }

    /// Losses that swallow every gain make the choice moot, and a moot choice
    /// indexes nothing rather than a split that changes no figure.
    #[test]
    fn losses_beyond_every_gain_index_nothing() {
        let pool = pool_with("0", "0", "10000");
        let cands = [cand("500", "5000", "4422.5")];
        let indexed = loss_aware_optimum(&pool, &cands);
        assert_eq!(pool.assessed(&cands, &indexed), Decimal::ZERO);
        assert_eq!(indexed, vec![Decimal::ZERO]);
    }

    mod properties {
        use super::*;
        use proptest::prelude::*;

        /// Every whole-unit split of the candidates, for a brute-force check.
        fn every_split(cands: &[Candidate]) -> Vec<Vec<Decimal>> {
            cands.iter().fold(vec![Vec::new()], |splits, c| {
                let n = c.units.to_string().parse::<u32>().unwrap();
                splits
                    .iter()
                    .flat_map(|s| {
                        (0..=n).map(move |x| {
                            let mut s = s.clone();
                            s.push(Decimal::from(x));
                            s
                        })
                    })
                    .collect()
            })
        }

        fn candidate() -> impl Strategy<Value = Candidate> {
            // Whole cents of gross gain per unit, and an indexed gain at or
            // below it (indexation only ever raises the cost base).
            (1u32..=12, 1u32..=5000, 0u32..=100).prop_map(|(units, cents, pct)| {
                let units = Decimal::from(units);
                let gain = Decimal::new(i64::from(cents), 2) * units;
                Candidate {
                    units,
                    gain,
                    indexed_gain: gain * Decimal::from(pct) / Decimal::ONE_HUNDRED,
                }
            })
        }

        proptest! {
            /// With one parcel the optimum is the true minimum over every
            /// whole-unit split — the year's figure is convex in the count
            /// indexed, so the two counts either side of the point where the
            /// losses run out are the only candidates.
            #[test]
            fn one_parcels_optimum_is_the_brute_force_minimum(
                c in candidate(),
                other in 0u32..=2000,
                other_discount in 0u32..=2000,
                losses in 0u32..=8000,
            ) {
                let pool = LossPool {
                    other_gains: Decimal::from(other),
                    other_discount_gains: Decimal::from(other_discount),
                    losses: Decimal::from(losses),
                };
                let cands = [c];
                let best = every_split(&cands)
                    .iter()
                    .map(|x| pool.assessed(&cands, x))
                    .min()
                    .unwrap();
                let chosen = pool.assessed(&cands, &loss_aware_optimum(&pool, &cands));
                prop_assert_eq!(chosen, best);
            }

            /// With several parcels competing for the losses the greedy order
            /// is the exact optimum of the continuous problem; in whole units
            /// it is never worse than the brute-force minimum by more than
            /// one unit's indexed gain, and never worse than indexing nothing
            /// or everything.
            #[test]
            fn several_parcels_stay_within_a_unit_of_the_minimum(
                cands in proptest::collection::vec(candidate(), 2..=3),
                losses in 0u32..=8000,
            ) {
                let pool = LossPool {
                    other_gains: Decimal::ZERO,
                    other_discount_gains: Decimal::ZERO,
                    losses: Decimal::from(losses),
                };
                let best = every_split(&cands)
                    .iter()
                    .map(|x| pool.assessed(&cands, x))
                    .min()
                    .unwrap();
                let chosen = pool.assessed(&cands, &loss_aware_optimum(&pool, &cands));
                let one_unit = cands
                    .iter()
                    .map(|c| c.indexed_gain / c.units)
                    .max()
                    .unwrap();
                prop_assert!(chosen >= best);
                prop_assert!(chosen - best <= one_unit, "{chosen} against {best}");
                let none = vec![Decimal::ZERO; cands.len()];
                let all: Vec<Decimal> = cands.iter().map(|c| c.units).collect();
                prop_assert!(chosen <= pool.assessed(&cands, &none));
                prop_assert!(chosen <= pool.assessed(&cands, &all));
            }
        }
    }

    /// The year row carries the three figures and the allocation the split
    /// indexes: Clare's parcel, with her $3,500 brought-forward loss entered
    /// as the opening balance.
    #[tokio::test]
    async fn the_year_row_reports_the_loss_aware_split() {
        let pool = test_pool().await;
        crate::entities::cgt_settings::db_upsert(
            &pool,
            &crate::entities::cgt_settings::CgtSettings {
                id: 1,
                opening_capital_loss: dec("3500"),
            },
        )
        .await
        .unwrap();
        test_support::listing(1).ticker("CLRE").insert(&pool).await;
        test_support::buy(1, 1)
            .date(ymd(1995, 3, 15))
            .qty(dec("500"))
            .price(dec("15"))
            .insert(&pool)
            .await;
        test_support::sell(2, 1)
            .date(ymd(2026, 3, 16))
            .qty(dec("500"))
            .price(dec("25"))
            .insert(&pool)
            .await;
        test_support::allocate(&pool, 1, 2, 1, dec("500")).await;

        let report = db_indexation_cross_check(&pool).await.unwrap();
        assert_eq!(report.comparisons[0].loss_aware_units_indexed, dec("395"));
        let year = &report.years[0];
        assert_eq!(year.capital_losses_realised, Decimal::ZERO);
        assert_eq!(year.capital_loss_brought_forward, dec("3500"));
        assert_eq!(year.discount_throughout_net_capital_gain, dec("750"));
        assert_eq!(year.indexation_throughout_net_capital_gain, dec("922.5"));
        assert_eq!(year.loss_aware_net_capital_gain, dec("521.8875"));
        assert_eq!(year.loss_aware_saving, dec("228.1125"));
        assert!(
            year.comparison.contains("brought forward A$3500.00"),
            "{}",
            year.comparison
        );
        assert!(year.comparison.contains("indexing just enough units"));
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

        let realised: serde_json::Value = api.get_json("/portfolio/realised_gains").await;
        assert_eq!(realised[0]["capital_gain_loss"], "10000");
        assert_eq!(realised[0]["discount_eligible_gain"], "10000");
        // The advisory pair rides along on the parcel row, summed into
        // nothing.
        let parcel = &realised[0]["parcels"][0];
        assert_eq!(parcel["cost_base"], "10000");
        assert_eq!(parcel["indexation_eligible"], serde_json::json!(true));
        assert_eq!(parcel["indexed_cost_base"], "17300.000");

        let ncg: serde_json::Value = api.get_json("/portfolio/net_capital_gain").await;
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
