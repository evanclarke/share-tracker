use crate::domain::open_parcels;
use crate::infra::http::ApiError;
use axum::{Json, Router, extract::State, routing::get};
use chrono::NaiveDate;
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use sqlx::{Row, SqlitePool};
use std::collections::HashMap;

/// One open (not fully sold) purchase parcel — the per-parcel schedule a user
/// reconciles against a broker statement. All monetary figures are AUD,
/// converted at the parcel's buy-month ATO rate (manual `fx_rate` fallback).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OpenParcel {
    /// The Buy/DRP trade that created the parcel.
    pub trade_id: i64,
    pub listing_id: i64,
    /// The holding account the parcel sits in.
    pub holding_account_id: i64,
    pub ticker: String,
    /// Original acquisition date — preserved across share splits/consolidations
    /// (TD 2000/10), so the 12-month discount clock keeps running from here.
    /// A scrip-for-scrip replacement parcel reports the consumed parcel's
    /// acquisition date (its `deemed_acquisition_date` — the rollover's
    /// combined holding period), not the exchange date.
    pub acquisition_date: NaiveDate,
    /// Units as originally transacted (pre-split basis where a split followed).
    pub original_quantity: Decimal,
    /// Units not yet allocated to a Sell, in *current* units — after every
    /// recorded share split/consolidation — so it reconciles with a broker
    /// statement.
    pub remaining_quantity: Decimal,
    /// Whole-parcel cost base as acquired (price × qty + brokerage + GST), AUD.
    pub original_cost_base: Decimal,
    /// Cumulative AMIT cost-base reductions reaching the **remaining** units
    /// to date, AUD (the full reduction, even where CGT event E10 has floored
    /// the cost base) — the reduction netted off `remaining_cost_base`, not
    /// the whole parcel's, so a statement that covered only the units still
    /// held is reported at what it took off them.
    pub amit_cost_base_reduction: Decimal,
    /// Cumulative return-of-capital payments (CGT event G1) received on the
    /// remaining units since acquisition, AUD (the full amount, even where the
    /// cost base has been floored at nil).
    pub return_of_capital_reduction: Decimal,
    /// Adjusted cost base of the remaining units, AUD: max(original − AMIT, 0)
    /// pro-rated to the remaining quantity, less return-of-capital payments on
    /// those units (CGT events E10 and G1 both floor the cost base at nil).
    pub remaining_cost_base: Decimal,
}

pub fn router() -> Router<SqlitePool> {
    Router::new().route("/portfolio/open-parcels", get(open_parcels_handler))
}

/// Lists every open parcel: a Buy/DRP trade whose quantity is not fully
/// consumed by parcel allocations. Same open-quantity and AMIT/E10 cost-base
/// rules as the portfolio/unrealised reports, but per parcel instead of
/// aggregated per listing.
pub async fn db_open_parcels(pool: &SqlitePool) -> Result<Vec<OpenParcel>, sqlx::Error> {
    // One read transaction: every input below comes from the same snapshot,
    // so an interleaved write can't yield e.g. an allocation whose parcel is
    // missing from the same read.
    let mut tx = pool.begin().await?;
    // `None`: this endpoint is the live schedule, as at today.
    let parcels = db_open_parcels_on(&mut tx, None).await?;
    tx.commit().await?;
    Ok(parcels)
}

/// The same report read on the caller's own connection, for callers (the
/// parcel optimiser, and through it the what-if) that fold the open parcels
/// into a wider single-snapshot read transaction.
///
/// `as_of` is the position's date — `None` the live view (as at today), and
/// `Some(date)` the holding as it stood on that date, with quantities in that
/// date's unit basis (see `docs/API.md`'s As-at date section). The optimiser
/// and pre-sale what-if pass their own request's sale date, so the candidate
/// parcels are exactly the ones a real Sell on that date could allocate.
pub async fn db_open_parcels_on(
    conn: &mut sqlx::SqliteConnection,
    as_of: Option<NaiveDate>,
) -> Result<Vec<OpenParcel>, sqlx::Error> {
    // The open parcels and their AUD cost bases come from the shared loader
    // (`domain::open_parcels`): with `as_of: None` the live view — an unsold
    // unit was held for every recorded payment, quantities in current units —
    // and with a date, the same read bounded at it. This report adds only the
    // joined ticker and the shaping into the printable schedule.
    let open = open_parcels::load(&mut *conn, as_of).await?;

    // Ticker per listing, as a separate lookup rather than a join option on
    // the shared loader (only this report wants it). Every trade's listing_id
    // is a foreign key, so the map always has the row.
    let ticker_rows = sqlx::query("SELECT id, ticker FROM listings")
        .fetch_all(&mut *conn)
        .await?;
    let mut tickers: HashMap<i64, String> = HashMap::new();
    for row in &ticker_rows {
        tickers.insert(row.try_get("id")?, row.try_get("ticker")?);
    }

    let mut parcels = Vec::with_capacity(open.len());
    for p in &open {
        let t = &p.parcel;
        // A scrip-for-scrip replacement parcel carries the consumed parcel's
        // acquisition date (`t.acquired()`): it drives the reported
        // acquisition date and the AUD translation month (the rollover
        // carries the AUD cost base over); split/ROC applicability stays on
        // the actual trade date.
        parcels.push(OpenParcel {
            trade_id: t.id,
            listing_id: t.listing_id,
            holding_account_id: t.holding_account_id,
            ticker: tickers.get(&t.listing_id).cloned().unwrap_or_default(),
            acquisition_date: t.acquired(),
            original_quantity: t.quantity,
            // Current units — after every recorded split/consolidation — so
            // it reconciles with a broker statement; `original_quantity`
            // stays as transacted.
            remaining_quantity: p.remaining_as_of,
            original_cost_base: p.cost_base.initial_cost,
            amit_cost_base_reduction: p.cost_base.amit_reduction,
            return_of_capital_reduction: p.cost_base.roc_reduction,
            remaining_cost_base: p.cost_base.adjusted,
        });
    }

    parcels.sort_by(|a, b| {
        (
            a.listing_id,
            a.holding_account_id,
            a.acquisition_date,
            a.trade_id,
        )
            .cmp(&(
                b.listing_id,
                b.holding_account_id,
                b.acquisition_date,
                b.trade_id,
            ))
    });
    Ok(parcels)
}

async fn open_parcels_handler(
    State(pool): State<SqlitePool>,
) -> Result<Json<Vec<OpenParcel>>, ApiError> {
    db_open_parcels(&pool)
        .await
        .map(Json)
        .map_err(ApiError::from)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::entities::corporate_action;
    use crate::test_support::{self, ApiClient, allocate, dec, test_pool, ymd};
    use axum::http::StatusCode;

    async fn insert_listing(pool: &SqlitePool, id: i64, ticker: &str) {
        test_support::listing(id)
            .ticker(ticker)
            .name(ticker)
            .insert(pool)
            .await;
    }

    /// A listing quoted in a foreign currency — its trades must be recorded in
    /// the same one (`trade::UpsertError::CurrencyNotListings`).
    async fn insert_listing_in(pool: &SqlitePool, id: i64, ticker: &str, currency: &str) {
        test_support::listing(id)
            .mic("XNYS")
            .ticker(ticker)
            .name(ticker)
            .currency(currency)
            .insert(pool)
            .await;
    }

    async fn insert_buy(
        pool: &SqlitePool,
        id: i64,
        listing_id: i64,
        date: NaiveDate,
        qty: Decimal,
        price: Decimal,
    ) {
        insert_buy_ccy(pool, id, listing_id, date, qty, price, "AUD").await;
    }

    async fn insert_buy_ccy(
        pool: &SqlitePool,
        id: i64,
        listing_id: i64,
        date: NaiveDate,
        qty: Decimal,
        price: Decimal,
        currency: &str,
    ) {
        test_support::buy(id, listing_id)
            .date(date)
            .qty(qty)
            .price(price)
            .currency(currency)
            .brokerage(dec("9.95"))
            .gst_on_brokerage(dec("0.995"))
            .insert(pool)
            .await;
    }

    async fn insert_sell(pool: &SqlitePool, id: i64, listing_id: i64, qty: Decimal) {
        test_support::sell(id, listing_id)
            .date(ymd(2025, 6, 1))
            .qty(qty)
            .price(Decimal::from(120))
            .brokerage(dec("9.95"))
            .gst_on_brokerage(dec("0.995"))
            .insert(pool)
            .await;
    }

    /// AMMA statement carrying only a per-unit cost base adjustment, linked to
    /// `trade_id` over `qty` units.
    async fn apply_amit(
        pool: &SqlitePool,
        amma_id: i64,
        listing_id: i64,
        trade_id: i64,
        qty: Decimal,
        per_unit: Decimal,
    ) {
        test_support::amma(amma_id, listing_id)
            .units(qty)
            .cost_base_adjustment(per_unit)
            .insert(pool)
            .await;
        test_support::amit_adjustment(pool, amma_id, amma_id, trade_id, qty).await;
    }

    // DB-level tests

    #[tokio::test]
    async fn db_no_trades_returns_empty() {
        let pool = test_pool().await;
        let parcels = db_open_parcels(&pool).await.unwrap();
        assert!(parcels.is_empty());
    }

    #[tokio::test]
    async fn db_open_parcel_listed_with_original_figures() {
        let pool = test_pool().await;
        let buy_date = NaiveDate::from_ymd_opt(2024, 1, 1).unwrap();
        insert_listing(&pool, 1, "VAS").await;
        insert_buy(&pool, 1, 1, buy_date, Decimal::from(100), Decimal::from(10)).await;

        let parcels = db_open_parcels(&pool).await.unwrap();
        assert_eq!(parcels.len(), 1);
        let p = &parcels[0];
        assert_eq!(p.trade_id, 1);
        assert_eq!(p.listing_id, 1);
        assert_eq!(p.ticker, "VAS");
        assert_eq!(p.acquisition_date, buy_date);
        assert_eq!(p.original_quantity, Decimal::from(100));
        assert_eq!(p.remaining_quantity, Decimal::from(100));
        // cost = 10 * 100 + 9.95 + 0.995 = 1010.945
        assert_eq!(p.original_cost_base, "1010.945".parse::<Decimal>().unwrap());
        assert_eq!(p.amit_cost_base_reduction, Decimal::ZERO);
        assert_eq!(
            p.remaining_cost_base,
            "1010.945".parse::<Decimal>().unwrap()
        );
    }

    /// Security identity continuity across a ticker/name change: a rename is an
    /// in-place edit to the listing (`PUT /listings/:id` with the same id), so
    /// every parcel stays attached — same listing id, new ticker, unchanged
    /// acquisition date and cost base. Nothing is keyed by ticker.
    #[tokio::test]
    async fn db_ticker_rename_keeps_parcels_attached_to_the_listing() {
        let pool = test_pool().await;
        let buy_date = NaiveDate::from_ymd_opt(2024, 1, 1).unwrap();
        insert_listing(&pool, 1, "OLD").await;
        insert_buy(&pool, 1, 1, buy_date, Decimal::from(100), Decimal::from(10)).await;

        // The security is renamed: same listing id, new ticker — recorded via
        // the rename action (a bare PUT is refused once the listing has a
        // trade; see entities::listing::UpsertError::IdentityChangeRequiresRename).
        crate::entities::listing_rename::db_rename(
            &pool,
            1,
            &crate::entities::listing_rename::RenameBody {
                effective_date: NaiveDate::from_ymd_opt(2024, 6, 1).unwrap(),
                ticker: "NEW".to_string(),
                exchange_mic: None,
                name: Some("NEW".to_string()),
                price_symbol: None,
                note: None,
            },
        )
        .await
        .unwrap();

        let parcels = db_open_parcels(&pool).await.unwrap();
        assert_eq!(parcels.len(), 1);
        let p = &parcels[0];
        assert_eq!(p.listing_id, 1);
        assert_eq!(p.ticker, "NEW");
        assert_eq!(p.acquisition_date, buy_date);
        assert_eq!(p.remaining_quantity, Decimal::from(100));
        assert_eq!(
            p.remaining_cost_base,
            "1010.945".parse::<Decimal>().unwrap()
        );
    }

    #[tokio::test]
    async fn db_partial_sell_pro_rates_remaining_cost_base() {
        let pool = test_pool().await;
        let buy_date = NaiveDate::from_ymd_opt(2024, 1, 1).unwrap();
        insert_listing(&pool, 1, "VAS").await;
        insert_buy(&pool, 1, 1, buy_date, Decimal::from(100), Decimal::from(10)).await;
        insert_sell(&pool, 2, 1, Decimal::from(40)).await;
        allocate(&pool, 1, 2, 1, Decimal::from(40)).await;

        let parcels = db_open_parcels(&pool).await.unwrap();
        assert_eq!(parcels.len(), 1);
        let p = &parcels[0];
        assert_eq!(p.original_quantity, Decimal::from(100));
        assert_eq!(p.remaining_quantity, Decimal::from(60));
        // original cost base reported for the whole parcel
        assert_eq!(p.original_cost_base, "1010.945".parse::<Decimal>().unwrap());
        // remaining = 1010.945 * 60 / 100 = 606.567
        assert_eq!(p.remaining_cost_base, "606.567".parse::<Decimal>().unwrap());
    }

    #[tokio::test]
    async fn db_fully_sold_parcel_excluded() {
        let pool = test_pool().await;
        let buy_date = NaiveDate::from_ymd_opt(2024, 1, 1).unwrap();
        insert_listing(&pool, 1, "VAS").await;
        insert_buy(&pool, 1, 1, buy_date, Decimal::from(100), Decimal::from(10)).await;
        insert_buy(&pool, 2, 1, buy_date, Decimal::from(50), Decimal::from(11)).await;
        insert_sell(&pool, 3, 1, Decimal::from(100)).await;
        allocate(&pool, 1, 3, 1, Decimal::from(100)).await;

        let parcels = db_open_parcels(&pool).await.unwrap();
        assert_eq!(parcels.len(), 1);
        assert_eq!(parcels[0].trade_id, 2);
        assert_eq!(parcels[0].remaining_quantity, Decimal::from(50));
    }

    #[tokio::test]
    async fn db_amit_reduction_reported_and_netted_off() {
        let pool = test_pool().await;
        let buy_date = NaiveDate::from_ymd_opt(2024, 1, 1).unwrap();
        insert_listing(&pool, 1, "VAF").await;
        insert_buy(&pool, 1, 1, buy_date, Decimal::from(100), Decimal::from(10)).await;
        // The statement covers the whole parcel at 5c/unit — 5.00 across 100
        // units, so every unit is reduced by 5c whether or not it is later
        // sold.
        apply_amit(&pool, 1, 1, 1, Decimal::from(100), "0.05".parse().unwrap()).await;
        // Partial sell of 40 after the adjustment
        insert_sell(&pool, 2, 1, Decimal::from(40)).await;
        allocate(&pool, 1, 2, 1, Decimal::from(40)).await;

        let parcels = db_open_parcels(&pool).await.unwrap();
        assert_eq!(parcels.len(), 1);
        let p = &parcels[0];
        assert_eq!(p.remaining_quantity, Decimal::from(60));
        assert_eq!(p.original_cost_base, "1010.945".parse::<Decimal>().unwrap());
        // The 60 units still held carry 60 × 5c of the reduction; the other
        // $2.00 went with the 40 units sold, and is netted off *their* cost
        // base in the realised report.
        assert_eq!(p.amit_cost_base_reduction, Decimal::from(3));
        // remaining = 1010.945 * 60 / 100 - 3.00 = 603.567
        assert_eq!(p.remaining_cost_base, "603.567".parse::<Decimal>().unwrap());
    }

    #[tokio::test]
    async fn db_e10_floors_remaining_cost_base_at_nil() {
        let pool = test_pool().await;
        let buy_date = NaiveDate::from_ymd_opt(2024, 1, 1).unwrap();
        insert_listing(&pool, 1, "VAF").await;
        insert_buy(&pool, 1, 1, buy_date, Decimal::from(100), Decimal::from(10)).await;
        // Reduction 100 * 11 = 1100 exceeds the 1010.945 cost base → E10 floor.
        apply_amit(&pool, 1, 1, 1, Decimal::from(100), Decimal::from(11)).await;

        let parcels = db_open_parcels(&pool).await.unwrap();
        assert_eq!(parcels.len(), 1);
        let p = &parcels[0];
        // The full cumulative reduction is still reported…
        assert_eq!(p.amit_cost_base_reduction, Decimal::from(1100));
        // …but the remaining cost base is floored at nil, never negative.
        assert_eq!(p.remaining_cost_base, Decimal::ZERO);
    }

    /// SCENARIOS F-13: the year's attribution exceeded the cash actually
    /// paid, so the AMMA states a net cost base **increase** — a negative
    /// per-unit figure, which the AMIT regime permits (upward adjustments
    /// were not allowed before it; `docs/ato/amit-cost-base-adjustments.md`).
    /// It raises the parcel's cost base by exactly that much, and no floor
    /// applies in this direction.
    #[tokio::test]
    async fn db_a_negative_per_unit_figure_increases_the_cost_base() {
        let pool = test_pool().await;
        let buy_date = NaiveDate::from_ymd_opt(2024, 1, 1).unwrap();
        insert_listing(&pool, 1, "VDHG").await;
        insert_buy(&pool, 1, 1, buy_date, Decimal::from(100), Decimal::from(10)).await;
        apply_amit(&pool, 1, 1, 1, Decimal::from(100), "-0.30".parse().unwrap()).await;

        let parcels = db_open_parcels(&pool).await.unwrap();
        let p = &parcels[0];
        // Reported as a negative reduction — the statement's own sign.
        assert_eq!(p.amit_cost_base_reduction, Decimal::from(-30));
        // 1010.945 + 30
        assert_eq!(
            p.remaining_cost_base,
            "1040.945".parse::<Decimal>().unwrap()
        );
    }

    #[tokio::test]
    async fn db_non_aud_parcel_converted_to_aud() {
        let pool = test_pool().await;
        let buy_date = NaiveDate::from_ymd_opt(2024, 1, 10).unwrap();
        insert_listing_in(&pool, 1, "VTS", "USD").await;
        // A$1 = 0.50 USD for the buy month → AUD = USD / 0.50.
        crate::entities::rba_fx_rate::db_import_rate(
            &pool,
            "USD",
            "2024-01",
            "0.50".parse().unwrap(),
        )
        .await
        .unwrap();
        insert_buy_ccy(
            &pool,
            1,
            1,
            buy_date,
            Decimal::from(100),
            Decimal::from(10),
            "USD",
        )
        .await;

        let parcels = db_open_parcels(&pool).await.unwrap();
        assert_eq!(parcels.len(), 1);
        // USD cost = 1010.945 → AUD 2021.89 at 0.50
        assert_eq!(
            parcels[0].original_cost_base,
            "2021.890".parse::<Decimal>().unwrap()
        );
        assert_eq!(
            parcels[0].remaining_cost_base,
            "2021.890".parse::<Decimal>().unwrap()
        );
    }

    /// SCENARIOS B-02: an Australian broker's AUD fee on a US trade. The
    /// brokerage leg is part of a single-currency cost base, so a differing
    /// `brokerage_currency` is refused at write time
    /// (`trade::AmountsError::BrokerageCurrencyMismatch`); the fee is recorded
    /// converted into the trade's own currency instead, and the whole cost
    /// base then converts once at the acquisition month's rate. A$33 of fees
    /// on USD 1,000 of consideration: USD 16.50 at 0.50 → A$2,033, not the
    /// A$2,066 an unconverted A$33 would have produced.
    #[tokio::test]
    async fn db_foreign_fee_recorded_in_the_trade_currency_costs_at_its_own_scale() {
        let pool = test_pool().await;
        let buy_date = NaiveDate::from_ymd_opt(2024, 1, 10).unwrap();
        insert_listing_in(&pool, 1, "VTS", "USD").await;
        crate::entities::rba_fx_rate::db_import_rate(
            &pool,
            "USD",
            "2024-01",
            "0.50".parse().unwrap(),
        )
        .await
        .unwrap();
        // A$30 brokerage + A$3 GST = A$33, entered as USD 15 + USD 1.50.
        test_support::buy(1, 1)
            .date(buy_date)
            .qty(Decimal::from(10))
            .price(Decimal::from(100))
            .currency("USD")
            .brokerage(Decimal::from(15))
            .gst_on_brokerage("1.5".parse().unwrap())
            .insert(&pool)
            .await;

        let parcels = db_open_parcels(&pool).await.unwrap();
        assert_eq!(parcels.len(), 1);
        assert_eq!(
            parcels[0].original_cost_base,
            Decimal::from(2033),
            "USD 1016.50 / 0.50 — the fee converts at the same rate as the consideration"
        );
    }

    #[tokio::test]
    async fn db_spot_fx_rate_wins_over_monthly_rate() {
        let pool = test_pool().await;
        let buy_date = NaiveDate::from_ymd_opt(2024, 1, 10).unwrap();
        insert_listing_in(&pool, 1, "VTS", "USD").await;
        // A monthly rate exists (0.50), but the parcel carries a deliberate
        // transaction-date spot rate (0.40) that must win (QC 18020):
        // USD 1000 / 0.40 = AUD 2500, not / 0.50 = 2000.
        crate::entities::rba_fx_rate::db_import_rate(
            &pool,
            "USD",
            "2024-01",
            "0.50".parse().unwrap(),
        )
        .await
        .unwrap();
        test_support::buy(1, 1)
            .date(buy_date)
            .qty(Decimal::from(100))
            .price(Decimal::from(10))
            .currency("USD")
            .spot_fx_rate("0.40".parse().unwrap())
            .insert(&pool)
            .await;

        let parcels = db_open_parcels(&pool).await.unwrap();
        assert_eq!(parcels.len(), 1);
        assert_eq!(
            parcels[0].remaining_cost_base,
            Decimal::from(2500),
            "the deliberate spot rate converts, not the monthly rate"
        );
    }

    #[tokio::test]
    async fn db_sorted_by_listing_then_acquisition_date() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "VAS").await;
        insert_listing(&pool, 2, "VGS").await;
        let d1 = NaiveDate::from_ymd_opt(2023, 5, 1).unwrap();
        let d2 = NaiveDate::from_ymd_opt(2024, 1, 1).unwrap();
        insert_buy(&pool, 1, 2, d1, Decimal::from(10), Decimal::from(10)).await;
        insert_buy(&pool, 2, 1, d2, Decimal::from(10), Decimal::from(10)).await;
        insert_buy(&pool, 3, 1, d1, Decimal::from(10), Decimal::from(10)).await;

        let parcels = db_open_parcels(&pool).await.unwrap();
        let order: Vec<(i64, NaiveDate)> = parcels
            .iter()
            .map(|p| (p.listing_id, p.acquisition_date))
            .collect();
        assert_eq!(order, vec![(1, d1), (1, d2), (2, d1)]);
    }

    async fn apply_roc(pool: &SqlitePool, id: i64, listing_id: i64, date: NaiveDate, amount: &str) {
        apply_roc_with_record(pool, id, listing_id, date, amount, None).await;
    }

    /// [`apply_roc`] with the record date that fixed entitlement to the
    /// payment (`None` = not recorded, so the payment date decides).
    async fn apply_roc_with_record(
        pool: &SqlitePool,
        id: i64,
        listing_id: i64,
        date: NaiveDate,
        amount: &str,
        record_date: Option<NaiveDate>,
    ) {
        corporate_action::db_upsert(
            pool,
            &corporate_action::CorporateAction {
                id,
                listing_id,
                date,
                kind: corporate_action::ActionKind::ReturnOfCapital {
                    amount_per_unit: amount.parse().unwrap(),
                    currency: "AUD".to_string(),
                    record_date,
                },
            },
        )
        .await
        .unwrap();
    }

    async fn apply_split(
        pool: &SqlitePool,
        id: i64,
        listing_id: i64,
        date: NaiveDate,
        new: &str,
        old: &str,
    ) {
        corporate_action::db_upsert(
            pool,
            &corporate_action::CorporateAction {
                id,
                listing_id,
                date,
                kind: corporate_action::ActionKind::ShareSplit {
                    split_new_units: new.parse().unwrap(),
                    split_old_units: old.parse().unwrap(),
                },
            },
        )
        .await
        .unwrap();
    }

    async fn apply_bonus(
        pool: &SqlitePool,
        id: i64,
        listing_id: i64,
        date: NaiveDate,
        bonus: &str,
        held: &str,
    ) {
        corporate_action::db_upsert(
            pool,
            &corporate_action::CorporateAction {
                id,
                listing_id,
                date,
                kind: corporate_action::ActionKind::BonusIssue {
                    bonus_units: bonus.parse().unwrap(),
                    bonus_held_units: held.parse().unwrap(),
                },
            },
        )
        .await
        .unwrap();
    }

    /// SCENARIOS E-11: a bonus issue whose ratio doesn't divide the holding
    /// evenly keeps the exact fractional entitlement, the same convention a
    /// consolidation follows — the registry issues 10 whole units and pays
    /// cash for the half, and neither the rounding nor the cash in lieu is
    /// modelled (`docs/API.md` "Fractional entitlements"; the cash received is
    /// entered as a Sell of the fractional units). The cost base is unchanged:
    /// a bonus issue is no CGT event, only a re-base.
    #[tokio::test]
    async fn db_bonus_issue_keeps_the_exact_fractional_entitlement() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "BON").await;
        insert_buy(
            &pool,
            1,
            1,
            ymd(2024, 1, 1),
            Decimal::from(105),
            Decimal::from(10),
        )
        .await;
        // 1-for-10 on 105 units: 10.5 bonus units, not 10.
        apply_bonus(&pool, 1, 1, ymd(2024, 3, 1), "1", "10").await;

        let parcels = db_open_parcels(&pool).await.unwrap();
        assert_eq!(parcels.len(), 1);
        assert_eq!(parcels[0].remaining_quantity, dec("115.5"));
        assert_eq!(parcels[0].original_quantity, Decimal::from(105));
        // Same total cost base, spread over the larger unit count.
        assert_eq!(
            parcels[0].remaining_cost_base,
            parcels[0].original_cost_base
        );
    }

    /// TD 2000/10 (`docs/ato/share-splits-and-consolidations.md`): the split parcel
    /// keeps its acquisition date and total cost base; the remaining quantity
    /// is reported in post-split units while original_quantity stays as
    /// transacted.
    #[tokio::test]
    async fn db_share_split_rebases_remaining_quantity_and_preserves_cost_base() {
        let pool = test_pool().await;
        let buy_date = NaiveDate::from_ymd_opt(2024, 1, 1).unwrap();
        insert_listing(&pool, 1, "SPL").await;
        insert_buy(&pool, 1, 1, buy_date, Decimal::from(100), Decimal::from(10)).await;
        apply_split(
            &pool,
            1,
            1,
            NaiveDate::from_ymd_opt(2024, 3, 1).unwrap(),
            "2",
            "1",
        )
        .await;
        // Sell 80 post-split units (= 40 as-acquired) after the split.
        insert_sell(&pool, 2, 1, Decimal::from(80)).await; // sale dated 2025-06-01
        allocate(&pool, 1, 2, 1, Decimal::from(80)).await;

        let parcels = db_open_parcels(&pool).await.unwrap();
        assert_eq!(parcels.len(), 1);
        let p = &parcels[0];
        assert_eq!(p.acquisition_date, buy_date);
        assert_eq!(p.original_quantity, Decimal::from(100));
        // 200 post-split − 80 sold = 120 in current units.
        assert_eq!(p.remaining_quantity, Decimal::from(120));
        assert_eq!(p.original_cost_base, "1010.945".parse::<Decimal>().unwrap());
        // 60 of the 100 as-acquired units remain: 1010.945 × 60/100.
        assert_eq!(p.remaining_cost_base, "606.567".parse::<Decimal>().unwrap());
    }

    /// A consolidation that doesn't divide the holding evenly keeps the exact
    /// fractional quantity (rounding/cash-in-lieu arrangements aren't modelled).
    #[tokio::test]
    async fn db_consolidation_rebases_remaining_quantity() {
        let pool = test_pool().await;
        let buy_date = NaiveDate::from_ymd_opt(2024, 1, 1).unwrap();
        insert_listing(&pool, 1, "CON").await;
        insert_buy(&pool, 1, 1, buy_date, Decimal::from(150), Decimal::from(10)).await;
        // 1-for-10 consolidation → 15 units.
        apply_split(
            &pool,
            1,
            1,
            NaiveDate::from_ymd_opt(2024, 3, 1).unwrap(),
            "1",
            "10",
        )
        .await;

        let parcels = db_open_parcels(&pool).await.unwrap();
        assert_eq!(parcels[0].remaining_quantity, Decimal::from(15));
        // Total cost base untouched: 10 × 150 + 9.95 + 0.995.
        assert_eq!(
            parcels[0].remaining_cost_base,
            "1510.945".parse::<Decimal>().unwrap()
        );
    }

    /// SCENARIOS B-12: several re-basing actions in sequence on one parcel —
    /// a split, a bonus issue, then a consolidation. Each re-bases the unit
    /// count off the *previous* basis (they compose, not merely the last one
    /// winning), while the acquisition date and the total cost base never move
    /// (TD 2000/10, `docs/ato/bonus-shares.md`). The single-action cases above
    /// can't catch a chain that composes wrongly.
    #[tokio::test]
    async fn db_a_chain_of_rebasing_actions_composes_and_preserves_the_cost_base() {
        let pool = test_pool().await;
        let buy_date = ymd(2024, 1, 1);
        insert_listing(&pool, 1, "CHN").await;
        insert_buy(&pool, 1, 1, buy_date, Decimal::from(100), Decimal::from(10)).await;
        // 2-for-1 split → 200 units.
        apply_split(&pool, 1, 1, ymd(2024, 3, 1), "2", "1").await;
        // 1-for-10 bonus issue on the 200 → 220 units.
        corporate_action::db_upsert(
            &pool,
            &corporate_action::CorporateAction {
                id: 2,
                listing_id: 1,
                date: ymd(2024, 4, 1),
                kind: corporate_action::ActionKind::BonusIssue {
                    bonus_units: Decimal::ONE,
                    bonus_held_units: Decimal::from(10),
                },
            },
        )
        .await
        .unwrap();
        // 1-for-10 consolidation of the 220 → 22 units.
        apply_split(&pool, 3, 1, ymd(2024, 5, 1), "1", "10").await;

        let parcels = db_open_parcels(&pool).await.unwrap();
        assert_eq!(parcels.len(), 1);
        let p = &parcels[0];
        // 100 → 200 → 220 → 22, and the transacted quantity is untouched.
        assert_eq!(p.remaining_quantity, Decimal::from(22));
        assert_eq!(p.original_quantity, Decimal::from(100));
        assert_eq!(p.acquisition_date, buy_date);
        // No CGT event happened: 10 × 100 + 9.95 + 0.995, three times over.
        assert_eq!(p.original_cost_base, dec("1010.945"));
        assert_eq!(p.remaining_cost_base, dec("1010.945"));
    }

    /// SCENARIOS B-13: a consolidation leaving a fractional quantity (7-for-10
    /// on 33 units → 23.1) survives to a sale that consumes it exactly — the
    /// fraction is neither rounded away at re-basing nor rejected by the
    /// allocation capacity check, and the parcel closes with no dust left.
    #[tokio::test]
    async fn db_a_fractional_consolidated_quantity_is_sellable_in_full() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "FRC").await;
        insert_buy(
            &pool,
            1,
            1,
            ymd(2024, 1, 1),
            Decimal::from(33),
            Decimal::from(10),
        )
        .await;
        apply_split(&pool, 1, 1, ymd(2024, 3, 1), "7", "10").await;

        let parcels = db_open_parcels(&pool).await.unwrap();
        assert_eq!(parcels[0].remaining_quantity, dec("23.10"));

        // Selling exactly that fractional quantity closes the parcel.
        insert_sell(&pool, 2, 1, dec("23.1")).await;
        allocate(&pool, 1, 2, 1, dec("23.1")).await;
        assert!(db_open_parcels(&pool).await.unwrap().is_empty());
    }

    /// A return of capital (CGT event G1) is reported per parcel and netted off
    /// the remaining cost base for the units still held
    /// (`docs/ato/cgt-non-assessable-payments.md`).
    #[tokio::test]
    async fn db_return_of_capital_reduction_reported_and_netted_off() {
        let pool = test_pool().await;
        let buy_date = NaiveDate::from_ymd_opt(2024, 1, 1).unwrap();
        insert_listing(&pool, 1, "RAP").await;
        insert_buy(&pool, 1, 1, buy_date, Decimal::from(100), Decimal::from(10)).await;
        // Sell 40 first, then a 50c/unit payment on the 60 still held.
        insert_sell(&pool, 2, 1, Decimal::from(40)).await; // sale dated 2025-06-01
        allocate(&pool, 1, 2, 1, Decimal::from(40)).await;
        apply_roc(
            &pool,
            1,
            1,
            NaiveDate::from_ymd_opt(2025, 7, 1).unwrap(),
            "0.50",
        )
        .await;

        let parcels = db_open_parcels(&pool).await.unwrap();
        assert_eq!(parcels.len(), 1);
        let p = &parcels[0];
        assert_eq!(p.remaining_quantity, Decimal::from(60));
        // The payment lands only on the 60 remaining units: 60 × 0.50 = 30.
        assert_eq!(p.return_of_capital_reduction, Decimal::from(30));
        // remaining = 1010.945 × 60/100 − 30 = 606.567 − 30 = 576.567
        assert_eq!(p.remaining_cost_base, "576.567".parse::<Decimal>().unwrap());
    }

    /// Entitlement to a return of capital is fixed at its record date, weeks
    /// before the money arrives (SCENARIOS B-09): a parcel bought after the
    /// shares went ex-entitlement received nothing, so its cost base is
    /// untouched — where the parcel bought before the record date is reduced as
    /// always. With no record date recorded the payment date decides, reducing
    /// both (the documented fallback every pre-existing action keeps).
    #[tokio::test]
    async fn db_return_of_capital_skips_parcels_bought_after_the_record_date() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "RAP").await;
        let cum = NaiveDate::from_ymd_opt(2025, 2, 1).unwrap();
        let ex = NaiveDate::from_ymd_opt(2025, 2, 15).unwrap();
        let record = NaiveDate::from_ymd_opt(2025, 2, 10).unwrap();
        let paid = NaiveDate::from_ymd_opt(2025, 3, 1).unwrap();
        insert_buy(&pool, 1, 1, cum, Decimal::from(100), Decimal::from(10)).await;
        insert_buy(&pool, 2, 1, ex, Decimal::from(100), Decimal::from(10)).await;
        apply_roc_with_record(&pool, 1, 1, paid, "0.50", Some(record)).await;

        // Sorted by acquisition date: the entitled parcel first.
        let parcels = db_open_parcels(&pool).await.unwrap();
        assert_eq!(parcels.len(), 2);
        assert_eq!(parcels[0].return_of_capital_reduction, Decimal::from(50));
        assert_eq!(
            parcels[1].return_of_capital_reduction,
            Decimal::ZERO,
            "bought ex-entitlement — the payment never reached it"
        );
        assert_eq!(
            parcels[1].remaining_cost_base,
            "1010.945".parse::<Decimal>().unwrap(),
            "its cost base is the one it was acquired at"
        );

        // The same action without a record date falls back to the payment date.
        apply_roc_with_record(&pool, 1, 1, paid, "0.50", None).await;
        let parcels = db_open_parcels(&pool).await.unwrap();
        assert_eq!(parcels[1].return_of_capital_reduction, Decimal::from(50));
    }

    /// G1 floors the remaining cost base at nil — the excess is a capital gain
    /// in the net-capital-gain report, never a negative cost base here.
    #[tokio::test]
    async fn db_return_of_capital_floors_remaining_cost_base_at_nil() {
        let pool = test_pool().await;
        let buy_date = NaiveDate::from_ymd_opt(2024, 1, 1).unwrap();
        insert_listing(&pool, 1, "RAP").await;
        insert_buy(&pool, 1, 1, buy_date, Decimal::from(100), Decimal::from(10)).await;
        // $11/unit × 100 = 1100 exceeds the 1010.945 cost base.
        apply_roc(
            &pool,
            1,
            1,
            NaiveDate::from_ymd_opt(2024, 3, 1).unwrap(),
            "11",
        )
        .await;

        let parcels = db_open_parcels(&pool).await.unwrap();
        assert_eq!(parcels.len(), 1);
        // The full payment is still reported…
        assert_eq!(parcels[0].return_of_capital_reduction, Decimal::from(1100));
        // …but the remaining cost base is floored at nil.
        assert_eq!(parcels[0].remaining_cost_base, Decimal::ZERO);
    }

    // API-level tests

    #[tokio::test]
    async fn api_get_open_parcels() {
        let pool = test_pool().await;
        let buy_date = NaiveDate::from_ymd_opt(2024, 1, 1).unwrap();
        insert_listing(&pool, 1, "VAS").await;
        insert_buy(&pool, 1, 1, buy_date, Decimal::from(100), Decimal::from(10)).await;
        insert_sell(&pool, 2, 1, Decimal::from(40)).await;
        allocate(&pool, 1, 2, 1, Decimal::from(40)).await;

        let resp = ApiClient::over(router().with_state(pool))
            .get("/portfolio/open-parcels")
            .await;
        assert_eq!(resp.status, StatusCode::OK);
        let parcels: Vec<OpenParcel> = resp.json();
        assert_eq!(parcels.len(), 1);
        assert_eq!(parcels[0].ticker, "VAS");
        assert_eq!(parcels[0].remaining_quantity, Decimal::from(60));
        assert_eq!(
            parcels[0].remaining_cost_base,
            "606.567".parse::<Decimal>().unwrap()
        );
    }

    /// A scrip-for-scrip replacement parcel is an ordinary open parcel, but
    /// reports the consumed parcel's acquisition date (the rollover's
    /// combined holding period) and its carried cost base — and a non-AUD
    /// carried cost base converts at the *original* buy month's ATO rate, so
    /// the AUD figure is unchanged by the exchange.
    #[tokio::test]
    async fn db_scrip_replacement_parcel_reports_carried_date_and_cost_base() {
        let pool = test_pool().await;
        let buy_date = NaiveDate::from_ymd_opt(2024, 1, 10).unwrap();
        insert_listing_in(&pool, 1, "OLD", "USD").await;
        insert_listing_in(&pool, 2, "NEW", "USD").await;
        // A$1 = 0.50 USD in the buy month, 0.80 in the exchange month.
        for (month, rate) in [("2024-01", "0.50"), ("2024-07", "0.80")] {
            crate::entities::rba_fx_rate::db_import_rate(
                &pool,
                "USD",
                month,
                rate.parse().unwrap(),
            )
            .await
            .unwrap();
        }
        // US$10 × 100 + US$9.95 + US$0.995 = US$1,010.945 = A$2,021.89.
        insert_buy_ccy(
            &pool,
            1,
            1,
            buy_date,
            Decimal::from(100),
            Decimal::from(10),
            "USD",
        )
        .await;
        corporate_action::db_upsert(
            &pool,
            &corporate_action::CorporateAction {
                id: 10,
                listing_id: 1,
                date: NaiveDate::from_ymd_opt(2024, 7, 1).unwrap(),
                kind: corporate_action::ActionKind::ScripForScrip {
                    scrip_listing_id: 2,
                    scrip_new_units: Decimal::from(2),
                    scrip_old_units: Decimal::ONE,
                    scrip_cash_per_unit: None,
                    scrip_market_value: None,
                    scrip_cash_currency: None,
                },
            },
        )
        .await
        .unwrap();
        crate::entities::scrip_exchange::db_exchange(&pool, 10)
            .await
            .unwrap();

        let parcels = db_open_parcels(&pool).await.unwrap();
        // The original parcel is fully consumed; only the replacement is open.
        assert_eq!(parcels.len(), 1);
        let p = &parcels[0];
        assert_eq!(p.listing_id, 2);
        assert_eq!(p.acquisition_date, buy_date);
        assert_eq!(p.remaining_quantity, Decimal::from(200));
        // A$2,021.89 at the Jan-2024 rate — not US$1,010.945 / 0.80.
        assert_eq!(p.original_cost_base, "2021.890".parse::<Decimal>().unwrap());
        assert_eq!(
            p.remaining_cost_base,
            "2021.890".parse::<Decimal>().unwrap()
        );
    }

    /// Demerger head and demerged parcels are ordinary open parcels, each
    /// reporting the consumed parcel's acquisition date (the head dates are
    /// unchanged by law; the new interests' discount clock runs from the
    /// original acquisition) and its share of the apportioned cost base.
    #[tokio::test]
    async fn db_demerger_parcels_report_carried_date_and_apportioned_cost_base() {
        let pool = test_pool().await;
        let buy_date = NaiveDate::from_ymd_opt(2024, 1, 10).unwrap();
        insert_listing(&pool, 1, "HEAD").await;
        insert_listing(&pool, 2, "DEM").await;
        // 100 @ $10 + $9.95 + $0.995 = $1,010.945 cost base.
        insert_buy(&pool, 1, 1, buy_date, Decimal::from(100), Decimal::from(10)).await;
        corporate_action::db_upsert(
            &pool,
            &corporate_action::CorporateAction {
                id: 10,
                listing_id: 1,
                date: NaiveDate::from_ymd_opt(2024, 7, 1).unwrap(),
                kind: corporate_action::ActionKind::Demerger {
                    demerger_listing_id: 2,
                    demerger_new_units: Decimal::ONE,
                    demerger_held_units: Decimal::from(5),
                    demerger_cost_base_pct: Decimal::from(20),
                },
            },
        )
        .await
        .unwrap();
        crate::entities::demerger::db_demerge(&pool, 10)
            .await
            .unwrap();

        let mut parcels = db_open_parcels(&pool).await.unwrap();
        // The original parcel is fully consumed; both replacement legs are
        // open and keep the original acquisition date.
        parcels.sort_by_key(|p| p.listing_id);
        assert_eq!(parcels.len(), 2);
        let head = &parcels[0];
        assert_eq!(head.listing_id, 1);
        assert_eq!(head.acquisition_date, buy_date);
        assert_eq!(head.remaining_quantity, Decimal::from(100));
        // 80% of $1,010.945 = $808.756.
        assert_eq!(
            head.remaining_cost_base,
            "808.756".parse::<Decimal>().unwrap()
        );
        let demerged = &parcels[1];
        assert_eq!(demerged.listing_id, 2);
        assert_eq!(demerged.acquisition_date, buy_date);
        assert_eq!(demerged.remaining_quantity, Decimal::from(20));
        // 20% of $1,010.945 = $202.189.
        assert_eq!(
            demerged.remaining_cost_base,
            "202.189".parse::<Decimal>().unwrap()
        );
    }
}
