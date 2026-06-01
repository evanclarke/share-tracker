//! Net capital gain / overall CGT position per Australian tax year.
//!
//! Combines the realised parcel gains (per the realised-gains report) with the
//! CGT components attributed on AMMA statements, then computes the assessable net
//! capital gain the ATO way:
//!
//!  1. Total the year's gross capital gains, split into:
//!     - discount-eligible gains (realised parcels held > 12 months, plus AMMA
//!       discount-method gains grossed up ×2 — the AMMA value is the already-halved
//!       "discounted capital gain" line, so doubling it restores the gross gain);
//!     - non-discountable gains (realised parcels held ≤ 12 months, plus AMMA
//!       indexation-method and other-method gains, neither of which gets the 50%
//!       discount).
//!  2. Total the year's capital losses (realised losses + AMMA capital losses
//!     applied).
//!  3. Apply losses against gains in the taxpayer-favourable order — non-discountable
//!     gains first, then discount-eligible gains — so the 50% discount falls on the
//!     largest possible remaining gain.
//!  4. Net capital gain = remaining non-discountable gain + 50% of the remaining
//!     discount-eligible gain. Any unused loss is carried forward.
//!
//! Prior-year carried-forward capital losses are not modelled (there is no store for
//! them), so `capital_loss_carried_forward` reflects only the current year's excess.

use crate::infra::decimal::parse_dec;
use crate::infra::fx::to_aud;
use axum::{Json, Router, extract::State, http::StatusCode, routing::get};
use chrono::{Datelike, Months, NaiveDate};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use sqlx::{Row, SqlitePool};
use std::collections::HashMap;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NetCapitalGainYear {
    /// Australian tax year: the calendar year in which 30 June falls (e.g. 2024 = FY2023/24).
    pub tax_year: i32,
    /// Gross discount-eligible capital gains before the discount (realised parcels
    /// held > 12 months + AMMA discount-method gains grossed up ×2).
    pub discount_eligible_gains: Decimal,
    /// Gross non-discountable capital gains (realised parcels held ≤ 12 months +
    /// AMMA indexation-method + AMMA other-method gains).
    pub other_gains: Decimal,
    /// Total capital losses available this year (realised losses + AMMA capital
    /// losses applied), as a positive amount.
    pub capital_losses: Decimal,
    /// Discount-eligible gain remaining after capital losses are applied (gross,
    /// before the 50% discount).
    pub net_discount_eligible_gain: Decimal,
    /// Non-discountable gain remaining after capital losses are applied.
    pub net_other_gain: Decimal,
    /// The 50% CGT discount amount removed from the remaining discount-eligible gain
    /// (= net_discount_eligible_gain / 2).
    pub cgt_discount: Decimal,
    /// Assessable net capital gain = net_other_gain + net_discount_eligible_gain / 2.
    pub net_capital_gain: Decimal,
    /// Capital losses left unused after offsetting all gains, carried forward.
    pub capital_loss_carried_forward: Decimal,
    /// Informational: gross CGT event E10 gains included in this year (the excess of
    /// AMIT cost base reductions over a parcel's cost base). Already counted within
    /// `discount_eligible_gains` / `other_gains` above per the holding period at the
    /// statement's year end; surfaced separately for transparency.
    pub cgt_event_e10_gain: Decimal,
}

pub fn router() -> Router<SqlitePool> {
    Router::new().route("/portfolio/net-capital-gain", get(net_capital_gain_handler))
}

/// Gross gains and losses accumulated for one tax year before netting.
#[derive(Default)]
struct GrossBuckets {
    discount_eligible: Decimal,
    other: Decimal,
    losses: Decimal,
    /// Gross CGT event E10 gains folded into the buckets above (informational).
    e10: Decimal,
}

/// Australian tax year for a dividend/sale date: July–December fall in the next FY.
fn tax_year_for(date: NaiveDate) -> i32 {
    if date.month() >= 7 {
        date.year() + 1
    } else {
        date.year()
    }
}

/// Read a TEXT decimal column and convert it to AUD via the ATO rate for `currency`
/// and the month of `date`. AMMA records carry no manual fx override, so a non-AUD
/// amount with no ATO rate fails loudly (the `FxError` surfaces as a decode error).
async fn aud_field(
    pool: &SqlitePool,
    row: &sqlx::sqlite::SqliteRow,
    field: &str,
    currency: &str,
    date: NaiveDate,
) -> Result<Decimal, sqlx::Error> {
    let value = parse_dec(field, row.try_get(field)?)?;
    Ok(to_aud(pool, value, currency, date, None).await?)
}

/// CGT event E10 gains: when the cumulative AMIT cost base reductions applied to a
/// parcel exceed its cost base, the cost base is floored at nil and the excess is a
/// capital gain in the income year the reducing AMMA statement applies to (see
/// `docs/amit-cost-base-adjustments.md`).
///
/// Returns `(tax_year, gross_gain_aud, discount_eligible)` for each AMMA statement
/// that pushes a parcel's cost base below nil. Adjustments are walked per parcel in
/// tax-year order, so the gain falls in the year the cost base is first exhausted —
/// and in every later year, since the cost base stays at nil (a later negative
/// adjustment, i.e. a cost base increase, restores it first). The excess is computed
/// in the parcel's native currency and converted to AUD at the parcel's buy-month ATO
/// rate (matching how the cost base itself is converted in the realised report), then
/// classified as discount-eligible when the units were held more than 12 months as at
/// the statement's `tax_year_end_date`.
async fn e10_gains(pool: &SqlitePool) -> Result<Vec<(i32, Decimal, bool)>, sqlx::Error> {
    let rows = sqlx::query(
        "SELECT aa.trade_id, aa.quantity AS adj_qty, \
                t.date AS trade_date, t.quantity AS trade_qty, t.average_price, \
                t.brokerage, t.gst_on_brokerage, t.currency AS trade_currency, t.fx_rate, \
                a.cost_base_adjustment, a.tax_year_end_date \
         FROM amit_adjustments aa \
         JOIN trades t ON t.id = aa.trade_id \
         JOIN amma_statements a ON a.id = aa.amma_statement_id \
         ORDER BY aa.trade_id, a.tax_year_end_date, a.id",
    )
    .fetch_all(pool)
    .await?;

    let mut out = Vec::new();
    let mut i = 0;
    while i < rows.len() {
        let trade_id: i64 = rows[i].try_get("trade_id")?;
        // Parcel cost base in native currency (the trade columns repeat per row).
        let trade_qty = parse_dec("trade_qty", rows[i].try_get("trade_qty")?)?;
        let price = parse_dec("average_price", rows[i].try_get("average_price")?)?;
        let brok = parse_dec("brokerage", rows[i].try_get("brokerage")?)?;
        let gst = parse_dec("gst_on_brokerage", rows[i].try_get("gst_on_brokerage")?)?;
        let trade_date: NaiveDate = rows[i].try_get("trade_date")?;
        let currency: String = rows[i].try_get("trade_currency")?;
        let fx_rate = parse_dec("fx_rate", rows[i].try_get("fx_rate")?)?;
        let mut remaining = price * trade_qty + brok + gst;

        while i < rows.len() && rows[i].try_get::<i64, _>("trade_id")? == trade_id {
            let adj_qty = parse_dec("adj_qty", rows[i].try_get("adj_qty")?)?;
            let cba = parse_dec("cost_base_adjustment", rows[i].try_get("cost_base_adjustment")?)?;
            let year_end: NaiveDate = rows[i].try_get("tax_year_end_date")?;
            let reduction = adj_qty * cba;
            if reduction > remaining {
                let excess = reduction - remaining;
                let excess_aud =
                    to_aud(pool, excess, &currency, trade_date, Some(fx_rate)).await?;
                let discount_eligible = year_end > trade_date + Months::new(12);
                out.push((year_end.year(), excess_aud, discount_eligible));
                remaining = Decimal::ZERO;
            } else {
                remaining -= reduction;
            }
            i += 1;
        }
    }
    Ok(out)
}

pub async fn db_net_capital_gain(pool: &SqlitePool) -> Result<Vec<NetCapitalGainYear>, sqlx::Error> {
    let mut buckets: HashMap<i32, GrossBuckets> = HashMap::new();

    // Realised parcel gains (already AUD), bucketed by the sale's tax year.
    let realised = super::realised_gains::db_realised_gains(pool).await?;
    for r in &realised {
        let b = buckets.entry(tax_year_for(r.sale_date)).or_default();
        b.discount_eligible += r.discount_eligible_gain;
        b.other += r.non_discountable_gain;
        b.losses += r.capital_loss;
    }

    // AMMA-attributed CGT components, converted to AUD via the ATO rate for the
    // month of tax_year_end_date (the statement's only period anchor).
    let amma_rows = sqlx::query(
        "SELECT tax_year_end_date, cgt_discount_gains, cgt_indexation_gains, \
         cgt_other_gains, capital_losses_applied, currency \
         FROM amma_statements",
    )
    .fetch_all(pool)
    .await?;

    for row in &amma_rows {
        let year_end: NaiveDate = row.try_get("tax_year_end_date")?;
        let currency: String = row.try_get("currency")?;
        let d = year_end;
        // AMMA discount-method gains are the already-halved "discounted capital gain"
        // line; gross up ×2 to the pre-discount gain before netting losses.
        let discount_net = aud_field(pool, row, "cgt_discount_gains", &currency, d).await?;
        let indexation = aud_field(pool, row, "cgt_indexation_gains", &currency, d).await?;
        let other = aud_field(pool, row, "cgt_other_gains", &currency, d).await?;
        let losses = aud_field(pool, row, "capital_losses_applied", &currency, d).await?;

        let b = buckets.entry(year_end.year()).or_default();
        b.discount_eligible += discount_net * Decimal::from(2);
        b.other += indexation + other;
        b.losses += losses;
    }

    // CGT event E10 gains — excess AMIT cost base reductions over a parcel's cost
    // base — are ordinary capital gains: they enter the buckets (discount-eligible or
    // not, per the holding period at year end), so losses can offset them and the
    // discount applies to the eligible portion.
    for (tax_year, amount, discount_eligible) in e10_gains(pool).await? {
        let b = buckets.entry(tax_year).or_default();
        if discount_eligible {
            b.discount_eligible += amount;
        } else {
            b.other += amount;
        }
        b.e10 += amount;
    }

    let two = Decimal::from(2);
    let mut result: Vec<NetCapitalGainYear> = buckets
        .into_iter()
        .map(|(tax_year, b)| {
            // Apply losses to non-discountable gains first, then to discount-eligible
            // gains (taxpayer-favourable: the discount falls on the largest remainder).
            let loss_to_other = b.other.min(b.losses);
            let net_other = b.other - loss_to_other;
            let remaining_loss = b.losses - loss_to_other;

            let loss_to_discount = b.discount_eligible.min(remaining_loss);
            let net_discount = b.discount_eligible - loss_to_discount;
            let carried_forward = remaining_loss - loss_to_discount;

            let cgt_discount = net_discount / two;
            NetCapitalGainYear {
                tax_year,
                discount_eligible_gains: b.discount_eligible,
                other_gains: b.other,
                capital_losses: b.losses,
                net_discount_eligible_gain: net_discount,
                net_other_gain: net_other,
                cgt_discount,
                net_capital_gain: net_other + cgt_discount,
                capital_loss_carried_forward: carried_forward,
                cgt_event_e10_gain: b.e10,
            }
        })
        .collect();
    result.sort_by_key(|s| s.tax_year);
    Ok(result)
}

async fn net_capital_gain_handler(
    State(pool): State<SqlitePool>,
) -> Result<Json<Vec<NetCapitalGainYear>>, StatusCode> {
    db_net_capital_gain(&pool)
        .await
        .map(Json)
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        entities::{amit_adjustment, amma, listing, parcel_allocation, rba_fx_rate, trade},
        infra::db,
    };
    use axum::{body::Body, http::Request};
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    async fn test_pool() -> SqlitePool {
        db::init(":memory:").await.unwrap()
    }

    async fn insert_listing(pool: &SqlitePool, id: i64, ticker: &str) {
        listing::db_upsert(
            pool,
            &listing::Listing {
                id,
                exchange_mic: "XASX".to_string(),
                ticker: ticker.to_string(),
                name: ticker.to_string(),
                isin: None,
                security_type: listing::SecurityType::ETF,
                currency: "AUD".to_string(),
                amit: false,
            },
        )
        .await
        .unwrap();
    }

    async fn insert_trade(
        pool: &SqlitePool,
        id: i64,
        trade_type: trade::TradeType,
        listing_id: i64,
        date: NaiveDate,
        qty: Decimal,
        price: Decimal,
    ) {
        trade::db_upsert(
            pool,
            &trade::Trade {
                id,
                trade_type,
                date,
                settlement_date: date + chrono::Duration::days(2),
                listing_id,
                average_price: price,
                quantity: qty,
                currency: "AUD".to_string(),
                brokerage: Decimal::ZERO,
                gst_on_brokerage: Decimal::ZERO,
                brokerage_currency: "AUD".to_string(),
                fx_rate: Decimal::ONE,
                contract_note_ref: None,
                residual_brought_forward: Decimal::ZERO,
                residual_carried_forward: Decimal::ZERO,
                residual_paid_out: Decimal::ZERO,
            },
        )
        .await
        .unwrap();
    }

    async fn allocate(pool: &SqlitePool, id: i64, sale_id: i64, buy_id: i64, qty: Decimal) {
        parcel_allocation::db_upsert(
            pool,
            &parcel_allocation::ParcelAllocation {
                id,
                sale_trade_id: sale_id,
                purchase_trade_id: buy_id,
                quantity_allocated: qty,
            },
        )
        .await
        .unwrap();
    }

    async fn link_adjustment(pool: &SqlitePool, id: i64, amma_id: i64, trade_id: i64, qty: Decimal) {
        amit_adjustment::db_upsert(
            pool,
            &amit_adjustment::AmitAdjustment {
                id,
                amma_statement_id: amma_id,
                trade_id,
                quantity: qty,
            },
        )
        .await
        .unwrap();
    }

    fn make_amma(id: i64, listing_id: i64, year_end: NaiveDate) -> amma::AmmaStatement {
        amma::AmmaStatement {
            id,
            listing_id,
            tax_year_end_date: year_end,
            units_held: Decimal::from(100),
            date_received: year_end + chrono::Duration::days(60),
            cost_base_adjustment: Decimal::ZERO,
            australian_interest: Decimal::ZERO,
            australian_dividends_unfranked: Decimal::ZERO,
            franked_dividends: Decimal::ZERO,
            franking_credits: Decimal::ZERO,
            net_rent: Decimal::ZERO,
            foreign_income: Decimal::ZERO,
            foreign_tax_credits: Decimal::ZERO,
            other_income: Decimal::ZERO,
            cgt_discount_gains: Decimal::ZERO,
            cgt_indexation_gains: Decimal::ZERO,
            cgt_other_gains: Decimal::ZERO,
            capital_losses_applied: Decimal::ZERO,
            tax_deferred_amount: Decimal::ZERO,
            tax_free_amount: Decimal::ZERO,
            tfn_withholding_tax: Decimal::ZERO,
            currency: "AUD".to_string(),
        }
    }

    #[tokio::test]
    async fn db_empty_returns_empty() {
        let pool = test_pool().await;
        assert!(db_net_capital_gain(&pool).await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn db_discount_eligible_gain_is_halved() {
        let pool = test_pool().await;
        // Buy 100 @ $10 (Jan 2024), sell 100 @ $15 (Jun 2025) → held > 12 months.
        insert_listing(&pool, 1, "VAS").await;
        insert_trade(&pool, 1, trade::TradeType::Buy, 1,
            NaiveDate::from_ymd_opt(2024, 1, 1).unwrap(), Decimal::from(100), Decimal::from(10)).await;
        insert_trade(&pool, 2, trade::TradeType::Sell, 1,
            NaiveDate::from_ymd_opt(2025, 6, 1).unwrap(), Decimal::from(100), Decimal::from(15)).await;
        allocate(&pool, 1, 2, 1, Decimal::from(100)).await;

        let r = db_net_capital_gain(&pool).await.unwrap();
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].tax_year, 2025); // sale Jun 2025 → FY2025
        assert_eq!(r[0].discount_eligible_gains, Decimal::from(500));
        assert_eq!(r[0].other_gains, Decimal::ZERO);
        assert_eq!(r[0].capital_losses, Decimal::ZERO);
        assert_eq!(r[0].cgt_discount, Decimal::from(250));
        // Net capital gain = 500 × 50% = 250.
        assert_eq!(r[0].net_capital_gain, Decimal::from(250));
        assert_eq!(r[0].capital_loss_carried_forward, Decimal::ZERO);
    }

    #[tokio::test]
    async fn db_short_term_gain_not_discounted() {
        let pool = test_pool().await;
        // Held ≤ 12 months → non-discountable; full gain assessable.
        insert_listing(&pool, 1, "VAS").await;
        insert_trade(&pool, 1, trade::TradeType::Buy, 1,
            NaiveDate::from_ymd_opt(2024, 1, 1).unwrap(), Decimal::from(100), Decimal::from(10)).await;
        insert_trade(&pool, 2, trade::TradeType::Sell, 1,
            NaiveDate::from_ymd_opt(2024, 6, 1).unwrap(), Decimal::from(100), Decimal::from(15)).await;
        allocate(&pool, 1, 2, 1, Decimal::from(100)).await;

        let r = db_net_capital_gain(&pool).await.unwrap();
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].other_gains, Decimal::from(500));
        assert_eq!(r[0].discount_eligible_gains, Decimal::ZERO);
        assert_eq!(r[0].cgt_discount, Decimal::ZERO);
        assert_eq!(r[0].net_capital_gain, Decimal::from(500));
    }

    #[tokio::test]
    async fn db_losses_applied_to_non_discount_gains_first() {
        let pool = test_pool().await;
        // FY2025: a discount-eligible gain of 500 and a non-discountable gain of 200,
        // plus a 100 capital loss. ATO-optimal: loss hits the non-discountable gain
        // first → net_other = 100, net_discount = 500, NCG = 100 + 250 = 350.
        insert_listing(&pool, 1, "VAS").await;
        // Discount-eligible: buy Jan 2024, sell Jun 2025 (>12mo), gain 500.
        insert_trade(&pool, 1, trade::TradeType::Buy, 1,
            NaiveDate::from_ymd_opt(2024, 1, 1).unwrap(), Decimal::from(100), Decimal::from(10)).await;
        insert_trade(&pool, 2, trade::TradeType::Sell, 1,
            NaiveDate::from_ymd_opt(2025, 6, 1).unwrap(), Decimal::from(100), Decimal::from(15)).await;
        allocate(&pool, 1, 2, 1, Decimal::from(100)).await;
        // Non-discountable: buy Mar 2025, sell Jun 2025 (≤12mo), gain 200.
        insert_trade(&pool, 3, trade::TradeType::Buy, 1,
            NaiveDate::from_ymd_opt(2025, 3, 1).unwrap(), Decimal::from(100), Decimal::from(10)).await;
        insert_trade(&pool, 4, trade::TradeType::Sell, 1,
            NaiveDate::from_ymd_opt(2025, 6, 2).unwrap(), Decimal::from(100), Decimal::from(12)).await;
        allocate(&pool, 2, 4, 3, Decimal::from(100)).await;
        // Capital loss of 100: buy Mar 2025 @ $10, sell Jun 2025 @ $9.
        insert_trade(&pool, 5, trade::TradeType::Buy, 1,
            NaiveDate::from_ymd_opt(2025, 3, 1).unwrap(), Decimal::from(100), Decimal::from(10)).await;
        insert_trade(&pool, 6, trade::TradeType::Sell, 1,
            NaiveDate::from_ymd_opt(2025, 6, 3).unwrap(), Decimal::from(100), Decimal::from(9)).await;
        allocate(&pool, 3, 6, 5, Decimal::from(100)).await;

        let r = db_net_capital_gain(&pool).await.unwrap();
        assert_eq!(r.len(), 1);
        let y = &r[0];
        assert_eq!(y.tax_year, 2025);
        assert_eq!(y.discount_eligible_gains, Decimal::from(500));
        assert_eq!(y.other_gains, Decimal::from(200));
        assert_eq!(y.capital_losses, Decimal::from(100));
        assert_eq!(y.net_other_gain, Decimal::from(100)); // 200 − 100
        assert_eq!(y.net_discount_eligible_gain, Decimal::from(500)); // untouched
        assert_eq!(y.net_capital_gain, Decimal::from(350)); // 100 + 500/2
        assert_eq!(y.capital_loss_carried_forward, Decimal::ZERO);
    }

    #[tokio::test]
    async fn db_losses_spill_into_discount_gains_then_carry_forward() {
        let pool = test_pool().await;
        // Discount-eligible gain 500, no other gains, capital loss 700.
        // Loss exhausts other (0), then reduces discount gain to 0 (uses 500),
        // leaving 200 carried forward. NCG = 0.
        insert_listing(&pool, 1, "VAS").await;
        insert_trade(&pool, 1, trade::TradeType::Buy, 1,
            NaiveDate::from_ymd_opt(2024, 1, 1).unwrap(), Decimal::from(100), Decimal::from(10)).await;
        insert_trade(&pool, 2, trade::TradeType::Sell, 1,
            NaiveDate::from_ymd_opt(2025, 6, 1).unwrap(), Decimal::from(100), Decimal::from(15)).await;
        allocate(&pool, 1, 2, 1, Decimal::from(100)).await;
        // Loss of 700: buy 100 @ $17, sell 100 @ $10 (Jun 2025).
        insert_trade(&pool, 3, trade::TradeType::Buy, 1,
            NaiveDate::from_ymd_opt(2025, 1, 1).unwrap(), Decimal::from(100), Decimal::from(17)).await;
        insert_trade(&pool, 4, trade::TradeType::Sell, 1,
            NaiveDate::from_ymd_opt(2025, 6, 2).unwrap(), Decimal::from(100), Decimal::from(10)).await;
        allocate(&pool, 2, 4, 3, Decimal::from(100)).await;

        let r = db_net_capital_gain(&pool).await.unwrap();
        assert_eq!(r.len(), 1);
        let y = &r[0];
        assert_eq!(y.capital_losses, Decimal::from(700));
        assert_eq!(y.net_discount_eligible_gain, Decimal::ZERO);
        assert_eq!(y.net_capital_gain, Decimal::ZERO);
        assert_eq!(y.capital_loss_carried_forward, Decimal::from(200));
    }

    #[tokio::test]
    async fn db_amma_discount_gains_grossed_up_then_halved() {
        let pool = test_pool().await;
        // AMMA discount-method gain stored as the net (already-halved) $100.
        // Grossed up ×2 = 200 discount-eligible; net capital gain = 200/2 = 100.
        insert_listing(&pool, 1, "VAF").await;
        let mut a = make_amma(1, 1, NaiveDate::from_ymd_opt(2024, 6, 30).unwrap());
        a.cgt_discount_gains = Decimal::from(100);
        amma::db_upsert(&pool, &a).await.unwrap();

        let r = db_net_capital_gain(&pool).await.unwrap();
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].tax_year, 2024);
        assert_eq!(r[0].discount_eligible_gains, Decimal::from(200));
        assert_eq!(r[0].cgt_discount, Decimal::from(100));
        assert_eq!(r[0].net_capital_gain, Decimal::from(100));
    }

    #[tokio::test]
    async fn db_amma_indexation_other_gains_and_losses() {
        let pool = test_pool().await;
        // Indexation 30 + other 20 = 50 non-discountable; capital losses applied 10.
        // Loss hits non-discountable first → net_other = 40, NCG = 40.
        insert_listing(&pool, 1, "VAF").await;
        let mut a = make_amma(1, 1, NaiveDate::from_ymd_opt(2024, 6, 30).unwrap());
        a.cgt_indexation_gains = Decimal::from(30);
        a.cgt_other_gains = Decimal::from(20);
        a.capital_losses_applied = Decimal::from(10);
        amma::db_upsert(&pool, &a).await.unwrap();

        let r = db_net_capital_gain(&pool).await.unwrap();
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].other_gains, Decimal::from(50));
        assert_eq!(r[0].capital_losses, Decimal::from(10));
        assert_eq!(r[0].net_other_gain, Decimal::from(40));
        assert_eq!(r[0].net_capital_gain, Decimal::from(40));
    }

    #[tokio::test]
    async fn db_realised_and_amma_combined_in_one_year() {
        let pool = test_pool().await;
        // FY2024: realised discount-eligible gain 500 (sale May 2024) + AMMA discount
        // gain net 100 (gross 200) → discount-eligible 700; NCG = 700/2 = 350.
        insert_listing(&pool, 1, "VAF").await;
        insert_trade(&pool, 1, trade::TradeType::Buy, 1,
            NaiveDate::from_ymd_opt(2023, 1, 1).unwrap(), Decimal::from(100), Decimal::from(10)).await;
        insert_trade(&pool, 2, trade::TradeType::Sell, 1,
            NaiveDate::from_ymd_opt(2024, 5, 1).unwrap(), Decimal::from(100), Decimal::from(15)).await;
        allocate(&pool, 1, 2, 1, Decimal::from(100)).await;
        let mut a = make_amma(1, 1, NaiveDate::from_ymd_opt(2024, 6, 30).unwrap());
        a.cgt_discount_gains = Decimal::from(100);
        amma::db_upsert(&pool, &a).await.unwrap();

        let r = db_net_capital_gain(&pool).await.unwrap();
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].tax_year, 2024);
        assert_eq!(r[0].discount_eligible_gains, Decimal::from(700));
        assert_eq!(r[0].net_capital_gain, Decimal::from(350));
    }

    #[tokio::test]
    async fn db_amma_non_aud_converted_via_ato_rate() {
        let pool = test_pool().await;
        // USD AMMA discount gain net US$50 with A$1 = 0.50 USD (Jun 2024).
        // AUD net = 100, gross ×2 = 200, NCG = 100.
        insert_listing(&pool, 1, "VAF").await;
        rba_fx_rate::db_import_rate(&pool, "USD", "2024-06", "0.50".parse().unwrap())
            .await
            .unwrap();
        let mut a = make_amma(1, 1, NaiveDate::from_ymd_opt(2024, 6, 30).unwrap());
        a.currency = "USD".to_string();
        a.cgt_discount_gains = Decimal::from(50);
        amma::db_upsert(&pool, &a).await.unwrap();

        let r = db_net_capital_gain(&pool).await.unwrap();
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].discount_eligible_gains, Decimal::from(200));
        assert_eq!(r[0].net_capital_gain, Decimal::from(100));
    }

    #[tokio::test]
    async fn db_amma_non_aud_without_rate_fails_loudly() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "VAF").await;
        let mut a = make_amma(1, 1, NaiveDate::from_ymd_opt(2024, 6, 30).unwrap());
        a.currency = "USD".to_string();
        a.cgt_discount_gains = Decimal::from(50);
        amma::db_upsert(&pool, &a).await.unwrap();

        assert!(db_net_capital_gain(&pool).await.is_err());
    }

    #[tokio::test]
    async fn db_sorted_by_tax_year() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "VAF").await;
        let mut a1 = make_amma(1, 1, NaiveDate::from_ymd_opt(2025, 6, 30).unwrap());
        a1.cgt_other_gains = Decimal::from(10);
        amma::db_upsert(&pool, &a1).await.unwrap();
        let mut a2 = make_amma(2, 1, NaiveDate::from_ymd_opt(2024, 6, 30).unwrap());
        a2.cgt_other_gains = Decimal::from(20);
        amma::db_upsert(&pool, &a2).await.unwrap();

        let r = db_net_capital_gain(&pool).await.unwrap();
        assert_eq!(r.len(), 2);
        assert_eq!(r[0].tax_year, 2024);
        assert_eq!(r[1].tax_year, 2025);
    }

    #[tokio::test]
    async fn db_e10_excess_reduction_becomes_capital_gain() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "VAF").await;
        // Buy 100 @ $1 → cost base $100; held ~6 months at the 30 Jun 2024 year end.
        insert_trade(&pool, 1, trade::TradeType::Buy, 1,
            NaiveDate::from_ymd_opt(2024, 1, 1).unwrap(), Decimal::from(100), Decimal::from(1)).await;
        // AMMA reduces cost base by $1.50/unit × 100 = $150 → $50 excess over the $100 base.
        let mut a = make_amma(1, 1, NaiveDate::from_ymd_opt(2024, 6, 30).unwrap());
        a.cost_base_adjustment = "1.50".parse().unwrap();
        amma::db_upsert(&pool, &a).await.unwrap();
        link_adjustment(&pool, 1, 1, 1, Decimal::from(100)).await;

        let r = db_net_capital_gain(&pool).await.unwrap();
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].tax_year, 2024);
        assert_eq!(r[0].cgt_event_e10_gain, Decimal::from(50));
        // Held ≤ 12 months as at the year end → non-discountable; fully assessable.
        assert_eq!(r[0].other_gains, Decimal::from(50));
        assert_eq!(r[0].discount_eligible_gains, Decimal::ZERO);
        assert_eq!(r[0].net_capital_gain, Decimal::from(50));
    }

    #[tokio::test]
    async fn db_e10_gain_discount_eligible_when_held_over_12_months() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "VAF").await;
        // Bought Jan 2023 → held > 12 months at the 30 Jun 2024 year end.
        insert_trade(&pool, 1, trade::TradeType::Buy, 1,
            NaiveDate::from_ymd_opt(2023, 1, 1).unwrap(), Decimal::from(100), Decimal::from(1)).await;
        let mut a = make_amma(1, 1, NaiveDate::from_ymd_opt(2024, 6, 30).unwrap());
        a.cost_base_adjustment = "1.50".parse().unwrap();
        amma::db_upsert(&pool, &a).await.unwrap();
        link_adjustment(&pool, 1, 1, 1, Decimal::from(100)).await;

        let r = db_net_capital_gain(&pool).await.unwrap();
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].cgt_event_e10_gain, Decimal::from(50));
        // Discount-eligible → halved.
        assert_eq!(r[0].discount_eligible_gains, Decimal::from(50));
        assert_eq!(r[0].other_gains, Decimal::ZERO);
        assert_eq!(r[0].cgt_discount, Decimal::from(25));
        assert_eq!(r[0].net_capital_gain, Decimal::from(25));
    }

    #[tokio::test]
    async fn db_e10_accumulates_across_years_fires_when_cost_base_exhausted() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "VAF").await;
        // Buy 100 @ $1 → cost base $100, bought Jan 2024.
        insert_trade(&pool, 1, trade::TradeType::Buy, 1,
            NaiveDate::from_ymd_opt(2024, 1, 1).unwrap(), Decimal::from(100), Decimal::from(1)).await;
        // FY2024: reduce $0.60/unit × 100 = $60 → cost base $40 remaining, no excess.
        let mut a1 = make_amma(1, 1, NaiveDate::from_ymd_opt(2024, 6, 30).unwrap());
        a1.cost_base_adjustment = "0.60".parse().unwrap();
        amma::db_upsert(&pool, &a1).await.unwrap();
        link_adjustment(&pool, 1, 1, 1, Decimal::from(100)).await;
        // FY2025: reduce $0.70/unit × 100 = $70 > $40 remaining → $30 excess (E10) in FY2025.
        let mut a2 = make_amma(2, 1, NaiveDate::from_ymd_opt(2025, 6, 30).unwrap());
        a2.cost_base_adjustment = "0.70".parse().unwrap();
        amma::db_upsert(&pool, &a2).await.unwrap();
        link_adjustment(&pool, 2, 2, 1, Decimal::from(100)).await;

        let r = db_net_capital_gain(&pool).await.unwrap();
        // Both AMMA statements create a year bucket; only FY2025 carries the E10 gain.
        assert_eq!(r.len(), 2);
        assert_eq!(r[0].tax_year, 2024);
        assert_eq!(r[0].cgt_event_e10_gain, Decimal::ZERO);
        assert_eq!(r[0].net_capital_gain, Decimal::ZERO);
        assert_eq!(r[1].tax_year, 2025);
        assert_eq!(r[1].cgt_event_e10_gain, Decimal::from(30));
        // Held > 12 months at the FY2025 year end → discount-eligible → $30/2 = $15.
        assert_eq!(r[1].discount_eligible_gains, Decimal::from(30));
        assert_eq!(r[1].net_capital_gain, Decimal::from(15));
    }

    #[tokio::test]
    async fn api_net_capital_gain_returns_json() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "VAS").await;
        insert_trade(&pool, 1, trade::TradeType::Buy, 1,
            NaiveDate::from_ymd_opt(2024, 1, 1).unwrap(), Decimal::from(100), Decimal::from(10)).await;
        insert_trade(&pool, 2, trade::TradeType::Sell, 1,
            NaiveDate::from_ymd_opt(2025, 6, 1).unwrap(), Decimal::from(100), Decimal::from(15)).await;
        allocate(&pool, 1, 2, 1, Decimal::from(100)).await;

        let resp = router()
            .with_state(pool)
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/portfolio/net-capital-gain")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let result: Vec<NetCapitalGainYear> = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].net_capital_gain, Decimal::from(250));
    }
}
