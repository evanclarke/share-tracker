//! Atomic ESS vesting: tie an ESS statement's income and CGT sides together.
//!
//! Given an `ess_statements` row, this creates the **cost-base-reset Buy** for
//! the vested shares and links it back (`trades.ess_statement_id`) in one
//! transaction. At the taxing point the ESS interest's first-element cost base
//! is reset to its market value and it is taken to be re-acquired on that date
//! for CGT (docs/ato/employee-share-schemes.md), so the Buy is dated the
//! taxing-point date with `average_price` = the per-share market value, zero
//! brokerage, in the statement's currency. The 12-month CGT discount clock and
//! the cost base both run from the taxing point — no `deemed_acquisition_date`.
//!
//! The income side (the assessable discount) is already on the statement and
//! reaches the tax summary directly; the vest is purely the parcel side.
//!
//! A non-AUD parcel needs a rate, and the Buy's `fx_rate` is the *fallback*
//! one — used when no ATO monthly rate exists for the month — so it is never
//! invented here: the statement's stated `fx_rate` is carried onto the Buy when
//! the taxpayer gave one, otherwise the taxing-point month's ATO rate is
//! resolved and carried, and a month with neither is refused (`422`,
//! [`VestError::MissingFxRate`]) rather than costing the parcel at parity.
//!
//! A statement may be vested at most once — re-posting is rejected rather than
//! creating a second Buy. The created Buy is immutable (`PUT /trades` → 422) and
//! never deleted individually; `DELETE /ess_statements/:id` removes it.
//!
//! # The trade write-time checks, and where each is satisfied
//!
//! The Buy is written with a raw `INSERT INTO trades`, not through
//! `trade::db_upsert`, so `trade::check_amounts` never runs over it. That is
//! deliberate, and this is the list it rests on — every one of that check's
//! rejections is already impossible here (SCENARIOS J-03):
//!
//! - `QuantityNotPositive` / `PriceNegative` — [`VestError::NothingToVest`]
//!   refuses a statement whose quantity or per-share market value is not
//!   positive, which is strictly stronger (it rejects a nil price too).
//! - `BrokerageNegative` / `GstNegative` — both are written as literal `'0'`:
//!   ESS-vested units are issued by the plan, so there is no broker.
//! - `BrokerageCurrencyMismatch` — `brokerage_currency` is bound from the same
//!   `currency` value as the trade's own, in one statement.
//! - `FxRateNotPositive` — the statement's stated rate is refused at write time
//!   unless positive (`ess_statement::UpsertError::FxRateNotPositive`), and a
//!   resolved ATO/RBA rate is positive by import.
//! - `SettlementBeforeTrade` — `settlement_date` is bound to the trade date
//!   itself (plan-issued units are not market-settled).
//! - `PreCgtDate` — the only one with nothing about the *insert* to prevent it,
//!   since the date comes from the statement. It is refused on the statement
//!   instead (`ess_statement::UpsertError::PreCgtTaxingPoint`), the earlier and
//!   better place: the taxing point is what the user typed.
//! - `FutureDate` — the same shape as `PreCgtDate` and settled the same way
//!   (SCENARIOS S-10): the Buy's date is the statement's taxing point, so the
//!   bound is applied on the statement
//!   (`ess_statement::UpsertError::FutureTaxingPoint`). An employer's statement
//!   reports a taxing point that has already happened, so a future one is a
//!   typo, and refusing it there names the field the user typed.
//!
//! A new check added to `trade::check_amounts` therefore needs a line here, and
//! either an argument that the vest satisfies it or a guard that makes it so.
//!
//! One write-time rule deliberately stops **outside** that list: the trading-day
//! check `trade::db_upsert` and `sell::db_upsert_sell` apply to the date
//! (`trade::UpsertError::NonTradingDay`, SCENARIOS S-08). It is not in
//! `check_amounts` — it needs a database read — and it does not reach the vest
//! by any other route either, on purpose: a deferred taxing point is a date the
//! *scheme* sets, and nothing obliges it to fall on a day the exchange traded.
//! A vest so dated is surfaced instead, non-blockingly, by
//! `reports::health`'s `non_trading_day_trades`.

use crate::entities::trade::{self, Trade};
use crate::infra::db::write_tx;
use crate::infra::decimal::Money;
use crate::infra::http::ApiError;
use axum::{
    Json, Router,
    extract::{Path, State},
    http::StatusCode,
    routing::post,
};
use rust_decimal::Decimal;
use sqlx::{Row, SqlitePool};

#[derive(thiserror::Error, Debug)]
pub enum VestError {
    #[error("ESS vest write failed: {0}")]
    Db(#[from] sqlx::Error),
    /// No ess_statements row with that id.
    #[error("no ESS statement with that id")]
    StatementNotFound,
    /// The statement already has a vest Buy. Delete the statement to redo it.
    #[error("this ESS statement has already been vested")]
    AlreadyVested,
    /// The statement's quantity or per-share market value is not positive —
    /// there is no parcel to create.
    #[error("the ESS statement's quantity and per-share market value must both be positive")]
    NothingToVest,
    /// A non-AUD statement whose taxing-point month has no imported ATO rate
    /// and that states no `fx_rate` of its own: the parcel's cost base has no
    /// rate to convert at, and binding a placeholder 1 would silently cost it
    /// at parity. Mapped to 422.
    #[error("no ATO FX rate for {currency} in {month} and the ESS statement states none")]
    MissingFxRate { currency: String, month: String },
}

pub fn router() -> Router<SqlitePool> {
    Router::new().route("/ess_statements/{id}/vest", post(vest))
}

/// Create the statement's cost-base-reset Buy and link it, atomically.
pub async fn db_vest(pool: &SqlitePool, statement_id: i64) -> Result<Trade, VestError> {
    let mut tx = write_tx(pool).await?;

    let row = sqlx::query(
        "SELECT listing_id, holding_account_id, taxing_point_date, quantity, \
                market_value_per_share, currency, fx_rate \
         FROM ess_statements WHERE id = ?",
    )
    .bind(statement_id)
    .fetch_optional(&mut *tx)
    .await?;
    let row = match row {
        Some(r) => r,
        None => return Err(VestError::StatementNotFound),
    };

    let already: bool =
        sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM trades WHERE ess_statement_id = ?)")
            .bind(statement_id)
            .fetch_one(&mut *tx)
            .await?;
    if already {
        return Err(VestError::AlreadyVested);
    }

    let listing_id: i64 = row.try_get("listing_id")?;
    let holding_account_id: i64 = row.try_get("holding_account_id")?;
    let taxing_point_date: chrono::NaiveDate = row.try_get("taxing_point_date")?;
    let quantity = crate::infra::decimal::parse_dec("quantity", row.try_get("quantity")?)?;
    let price = crate::infra::decimal::parse_dec(
        "market_value_per_share",
        row.try_get("market_value_per_share")?,
    )?;
    let currency: String = row.try_get("currency")?;
    let stated_fx_rate = crate::infra::decimal::row_opt_dec(&row, "fx_rate")?;

    if quantity <= Decimal::ZERO || price <= Decimal::ZERO {
        return Err(VestError::NothingToVest);
    }

    // The rate the Buy carries. `trades.fx_rate` is not a constant — it is the
    // *fallback* applied when no ATO monthly rate exists for the month
    // (`infra::fx::pick_rate`), so a placeholder 1 would become a real answer
    // exactly when the month's rate is missing, and cost a US$15,000 parcel at
    // A$15,000 (SCENARIOS J-08/J-12). The statement's stated rate is bound when
    // the taxpayer gave one; otherwise the taxing-point month's ATO rate is
    // resolved and bound (AUD resolves to 1), and a month with neither is
    // refused rather than invented.
    let fx = crate::infra::fx::FxRates::load(&mut *tx).await?;
    let fx_rate = match stated_fx_rate {
        Some(rate) => rate,
        None => fx
            .resolve_rate(
                &currency,
                taxing_point_date,
                crate::infra::fx::FxOverride::None,
            )
            .map_err(|_| VestError::MissingFxRate {
                currency: currency.clone(),
                month: taxing_point_date.format("%Y-%m").to_string(),
            })?,
    };

    // The cost-base-reset Buy: market value at the taxing point, dated and
    // settled on that date, in the statement's currency. ESS-vested units are
    // issued by the plan, not market-settled, so settlement is the trade date.
    // The id column is omitted, so the database assigns one its AUTOINCREMENT
    // sequence has never issued: a computed `MAX(id) + 1` would hand this
    // parcel a just-deleted trade's id, and with it that trade's row_history
    // trail (SCENARIOS U-a).
    let result = sqlx::query(
        "INSERT INTO trades \
         (trade_type, date, settlement_date, settlement_date_source, listing_id, \
          average_price, quantity, \
          currency, brokerage, gst_on_brokerage, brokerage_currency, fx_rate, \
          holding_account_id, ess_statement_id) \
         VALUES ('Buy', ?, ?, 'stated', ?, ?, ?, ?, '0', '0', ?, ?, ?, ?)",
    )
    .bind(taxing_point_date)
    .bind(taxing_point_date)
    .bind(listing_id)
    .bind(Money(price))
    .bind(Money(quantity))
    .bind(&currency)
    .bind(&currency)
    .bind(Money(fx_rate))
    .bind(holding_account_id)
    .bind(statement_id)
    .execute(&mut *tx)
    .await?;
    let new_id = result.last_insert_rowid();

    tx.commit().await?;

    trade::db_get(pool, new_id)
        .await?
        .ok_or_else(|| VestError::Db(sqlx::Error::RowNotFound))
}

async fn vest(
    State(pool): State<SqlitePool>,
    Path(statement_id): Path<i64>,
) -> Result<(StatusCode, Json<Trade>), ApiError> {
    let trade = db_vest(&pool, statement_id).await?;
    Ok((StatusCode::CREATED, Json(trade)))
}

impl From<VestError> for ApiError {
    fn from(e: VestError) -> Self {
        match e {
            VestError::StatementNotFound => ApiError::not_found("no ESS statement with that id"),
            VestError::AlreadyVested => ApiError::unprocessable(
                "this ESS statement has already been vested — delete it first to redo it",
            ),
            VestError::NothingToVest => ApiError::unprocessable(
                "the ESS statement's quantity and per-share market value must both be greater \
                 than zero to create the vest parcel",
            ),
            VestError::MissingFxRate { currency, month } => ApiError::unprocessable(format!(
                "this ESS statement is in {currency} but no ATO/RBA rate has been imported \
                 for {currency} in {month} and the statement states no fx_rate — import \
                 that month's rates or record the rate the employer used on the statement; \
                 vesting without one would cost the parcel at parity (1 AUD per {currency})"
            )),
            VestError::Db(err) => err.into(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::entities::trade::TradeType;
    use crate::entities::{ess_statement, listing};
    use crate::test_support::{self, ApiClient, test_pool, ymd};
    use chrono::NaiveDate;

    async fn insert_listing(pool: &SqlitePool, id: i64, currency: &str) {
        test_support::listing(id)
            .ticker(&format!("ESS{id}"))
            .name(&format!("ESS {id}"))
            .security_type(listing::SecurityType::Share)
            .currency(currency)
            .insert(pool)
            .await;
    }

    async fn insert_statement(pool: &SqlitePool, id: i64, qty: &str, price: &str, currency: &str) {
        test_support::ess_statement(id, 1, ymd(2024, 9, 1))
            .with(|s| {
                s.quantity = qty.parse().unwrap();
                s.market_value_per_share = price.parse().unwrap();
                s.deferral_discount = Decimal::from(600);
                s.currency = currency.to_string();
            })
            .insert(pool)
            .await;
    }

    #[tokio::test]
    async fn vest_creates_the_cost_base_reset_buy() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "AUD").await;
        insert_statement(&pool, 1, "100", "6", "AUD").await;

        let trade = db_vest(&pool, 1).await.unwrap();
        assert_eq!(trade.trade_type, TradeType::Buy);
        assert_eq!(trade.listing_id, 1);
        assert_eq!(trade.quantity, Decimal::from(100));
        assert_eq!(trade.average_price, Decimal::from(6));
        assert_eq!(trade.date, NaiveDate::from_ymd_opt(2024, 9, 1).unwrap());
        assert_eq!(
            trade.settlement_date,
            NaiveDate::from_ymd_opt(2024, 9, 1).unwrap()
        );
        assert_eq!(trade.brokerage, Decimal::ZERO);
        assert_eq!(trade.deemed_acquisition_date, None);
        assert_eq!(trade.ess_statement_id, Some(1));
        assert_eq!(trade.holding_account_id, 1);
    }

    /// The CGT clock on ESS-vested shares runs from the **deferred taxing
    /// point**, where the cost base is reset to market value and the shares
    /// are taken to be re-acquired (`docs/ato/employee-share-schemes.md`) —
    /// never from the grant, which is why no grant date is recorded at all.
    /// Sold 6 months after vesting, the gain is non-discountable no matter
    /// how long ago the shares were granted (SCENARIOS C-13).
    #[tokio::test]
    async fn discount_clock_runs_from_the_taxing_point_not_the_grant() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "AUD").await;
        insert_statement(&pool, 1, "100", "6", "AUD").await;
        let parcel = db_vest(&pool, 1).await.unwrap();
        assert_eq!(parcel.date, ymd(2024, 9, 1));
        assert_eq!(parcel.deemed_acquisition_date, None);

        // Sold 2025-03-01 — six months after the taxing point.
        crate::entities::sell::db_upsert_sell(
            &pool,
            50,
            &crate::entities::sell::SellBody {
                brokerage_includes_gst: false,
                statement_total: None,
                holding_account_id: parcel.holding_account_id,
                date: ymd(2025, 3, 3),
                settlement_date: Some(ymd(2025, 3, 3)),
                listing_id: 1,
                average_price: Decimal::from(10),
                quantity: Decimal::from(100),
                currency: "AUD".to_string(),
                brokerage: Decimal::ZERO,
                gst_on_brokerage: Decimal::ZERO,
                brokerage_currency: "AUD".to_string(),
                fx_rate: Decimal::ONE,
                spot_fx_rate: None,
                contract_note_ref: None,
                allocations: vec![crate::entities::sell::AllocationInput {
                    purchase_trade_id: parcel.id,
                    quantity_allocated: Decimal::from(100),
                }],
            },
        )
        .await
        .unwrap();

        let realised = crate::reports::realised_gains::db_realised_gains(&pool)
            .await
            .unwrap();
        assert_eq!(realised.len(), 1);
        let g = &realised[0];
        // Proceeds $1,000 − the market-value cost base $600 = a $400 gain,
        // entirely on the "other" (non-discountable) method.
        assert_eq!(g.cost_base, Decimal::from(600));
        assert_eq!(g.capital_gain_loss, Decimal::from(400));
        assert_eq!(g.discount_eligible_gain, Decimal::ZERO);
        assert_eq!(g.non_discountable_gain, Decimal::from(400));
        assert_eq!(g.parcels[0].acquisition_date, ymd(2024, 9, 1));
    }

    /// SCENARIOS J-05: where that clock falls due. The discount needs the
    /// shares held for *at least 12 months*, counting neither the acquisition
    /// day nor the disposal day (`docs/ato/cgt-discount.md`), so a taxing point
    /// of 1 September 2024 first qualifies on **2 September 2025** — sold on
    /// the 1st, one day short, the whole gain is on the other method. Half the
    /// parcel each side of that boundary proves the vest parcel is tested like
    /// any other, on the taxing point it was re-acquired at.
    #[tokio::test]
    async fn the_twelve_month_boundary_falls_a_day_after_the_taxing_point_anniversary() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "AUD").await;
        insert_statement(&pool, 1, "100", "6", "AUD").await; // taxing point 2024-09-01
        let parcel = db_vest(&pool, 1).await.unwrap();

        for (id, date) in [(50, ymd(2025, 9, 1)), (51, ymd(2025, 9, 2))] {
            sell_parcel(&pool, id, &parcel, date, Decimal::from(50)).await;
        }

        let realised = crate::reports::realised_gains::db_realised_gains(&pool)
            .await
            .unwrap();
        assert_eq!(realised.len(), 2);
        // Each half: $500 proceeds − $300 cost base = a $200 gain.
        let short = realised.iter().find(|g| g.sale_trade_id == 50).unwrap();
        assert_eq!(short.capital_gain_loss, Decimal::from(200));
        assert_eq!(
            short.discount_eligible_gain,
            Decimal::ZERO,
            "the anniversary itself is a day short of 12 months"
        );
        assert_eq!(short.non_discountable_gain, Decimal::from(200));
        let long = realised.iter().find(|g| g.sale_trade_id == 51).unwrap();
        assert_eq!(long.capital_gain_loss, Decimal::from(200));
        assert_eq!(long.discount_eligible_gain, Decimal::from(200));
        assert_eq!(long.non_discountable_gain, Decimal::ZERO);
    }

    /// SCENARIOS J-06: vested shares released into an employer-plan account and
    /// then moved to the personal broker account. The transfer is not a
    /// disposal, so the replacement parcel must carry the taxing point's cost
    /// base *and* its discount clock (`deemed_acquisition_date`) across the
    /// account boundary — a sale 13 months after the taxing point but 12 months
    /// after the transfer is still discountable. While that transferred-in
    /// parcel stands, the statement can't be deleted: its vest Buy is drawn on
    /// by the transfer's closing Sell.
    #[tokio::test]
    async fn a_vest_parcel_moved_to_another_account_keeps_its_taxing_point_clock() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "AUD").await;
        insert_statement(&pool, 1, "100", "6", "AUD").await; // taxing point 2024-09-01
        let vest = db_vest(&pool, 1).await.unwrap();

        crate::entities::holding_account::db_upsert(
            &pool,
            &crate::entities::holding_account::HoldingAccount {
                id: 2,
                name: "Broker".to_string(),
            },
        )
        .await
        .unwrap();
        let moved = crate::entities::transfer::db_transfer(
            &pool,
            1,
            &crate::entities::transfer::TransferBody {
                listing_id: 1,
                date: ymd(2024, 10, 1),
                from_account_id: 1,
                to_account_id: 2,
                allocations: vec![crate::entities::sell::AllocationInput {
                    purchase_trade_id: vest.id,
                    quantity_allocated: Decimal::from(100),
                }],
                fee_allocations: vec![],
                fee_market_price: None,
                fee_fx_rate: None,
            },
        )
        .await
        .unwrap();
        let transferred_in = &moved.transfer_ins[0];
        assert_eq!(transferred_in.holding_account_id, 2);
        assert_eq!(
            transferred_in.deemed_acquisition_date,
            Some(ymd(2024, 9, 1)),
            "the taxing point, not the transfer date, still runs the clock"
        );

        // The statement's vest Buy is now drawn on by the transfer's closing
        // Sell, so deleting the statement is refused rather than orphaning it.
        assert_eq!(
            ess_statement::db_delete(&pool, 1).await.unwrap(),
            ess_statement::DeleteOutcome::VestDrawnOn
        );

        // Sold from the broker account 2025-10-06 — 13 months after the taxing
        // point, 12 months after the transfer.
        sell_parcel(
            &pool,
            50,
            transferred_in,
            ymd(2025, 10, 6),
            Decimal::from(100),
        )
        .await;
        let realised = crate::reports::realised_gains::db_realised_gains(&pool)
            .await
            .unwrap();
        assert_eq!(realised.len(), 1);
        let g = &realised[0];
        assert_eq!(g.holding_account_id, 2);
        assert_eq!(
            g.cost_base,
            Decimal::from(600),
            "the taxing-point cost base"
        );
        assert_eq!(g.capital_gain_loss, Decimal::from(400));
        assert_eq!(g.discount_eligible_gain, Decimal::from(400));
        assert_eq!(g.parcels[0].acquisition_date, ymd(2024, 9, 1));
    }

    /// Sell `qty` units of `parcel` at $10 on `date`, from the parcel's own
    /// holding account.
    async fn sell_parcel(
        pool: &SqlitePool,
        id: i64,
        parcel: &Trade,
        date: NaiveDate,
        qty: Decimal,
    ) {
        crate::entities::sell::db_upsert_sell(
            pool,
            id,
            &crate::entities::sell::SellBody {
                brokerage_includes_gst: false,
                statement_total: None,
                holding_account_id: parcel.holding_account_id,
                date,
                settlement_date: None,
                listing_id: parcel.listing_id,
                average_price: Decimal::from(10),
                quantity: qty,
                currency: "AUD".to_string(),
                brokerage: Decimal::ZERO,
                gst_on_brokerage: Decimal::ZERO,
                brokerage_currency: "AUD".to_string(),
                fx_rate: Decimal::ONE,
                spot_fx_rate: None,
                contract_note_ref: None,
                allocations: vec![crate::entities::sell::AllocationInput {
                    purchase_trade_id: parcel.id,
                    quantity_allocated: qty,
                }],
            },
        )
        .await
        .unwrap();
    }

    /// The cost base = quantity × market value (price × qty + 0 brokerage), so
    /// the parcel carries the full taxing-point market value as its cost base.
    #[tokio::test]
    async fn vest_buy_carries_market_value_as_cost_base() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "AUD").await;
        insert_statement(&pool, 1, "100", "6", "AUD").await;
        let trade = db_vest(&pool, 1).await.unwrap();
        assert_eq!(trade.average_price * trade.quantity, Decimal::from(600));
    }

    #[tokio::test]
    async fn vest_keeps_the_statements_currency() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "USD").await;
        insert_statement(&pool, 1, "100", "6", "USD").await;
        import_usd_rate(&pool, "2024-09", "0.65").await;
        let trade = db_vest(&pool, 1).await.unwrap();
        assert_eq!(trade.currency, "USD");
        assert_eq!(trade.brokerage_currency, "USD");
    }

    async fn import_usd_rate(pool: &SqlitePool, month: &str, rate: &str) {
        crate::entities::rba_fx_rate::db_import_rate(pool, "USD", month, rate.parse().unwrap())
            .await
            .unwrap();
    }

    /// SCENARIOS J-12: `trades.fx_rate` is the rate applied **when no ATO
    /// monthly rate exists**, so the old hard-coded `'1'` became a real answer
    /// exactly where it was most wrong — a US$15,000 parcel costed at
    /// A$15,000. A vest whose taxing-point month has no rate and whose
    /// statement states none is now refused instead, so no parcel can enter
    /// the system at parity.
    #[tokio::test]
    async fn a_foreign_vest_with_no_rate_anywhere_is_refused_rather_than_costed_at_parity() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "USD").await;
        insert_statement(&pool, 1, "100", "150", "USD").await; // taxing point 2024-09-01

        assert!(matches!(
            db_vest(&pool, 1).await,
            Err(VestError::MissingFxRate { ref currency, ref month })
                if currency == "USD" && month == "2024-09"
        ));
        // Nothing was written: no parcel, and so no cost base to be wrong.
        let n: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM trades")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(n, 0);

        // With the month's rate imported the vest goes through and the parcel
        // costs A$23,076.92… — not the A$15,000 parity figure.
        import_usd_rate(&pool, "2024-09", "0.65").await;
        let trade = db_vest(&pool, 1).await.unwrap();
        assert_eq!(trade.fx_rate, "0.65".parse::<Decimal>().unwrap());
        let holdings = crate::reports::portfolio::db_holdings(&pool, None)
            .await
            .unwrap();
        assert_eq!(
            holdings[0].total_cost_base,
            (Decimal::from(15000) / "0.65".parse::<Decimal>().unwrap())
        );
    }

    /// SCENARIOS J-08: an employer's release-date rate has a home. The rate
    /// stated on the statement is carried onto the vest Buy and costs the
    /// parcel, with no ATO rate for the month needed.
    #[tokio::test]
    async fn a_statements_stated_rate_costs_the_parcel() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "USD").await;
        test_support::ess_statement(1, 1, ymd(2024, 9, 1))
            .with(|s| {
                s.quantity = Decimal::from(100);
                s.market_value_per_share = Decimal::from(150);
                s.currency = "USD".to_string();
                s.fx_rate = Some("0.6".parse().unwrap());
            })
            .insert(&pool)
            .await;

        let trade = db_vest(&pool, 1).await.unwrap();
        assert_eq!(trade.fx_rate, "0.6".parse::<Decimal>().unwrap());
        let holdings = crate::reports::portfolio::db_holdings(&pool, None)
            .await
            .unwrap();
        assert_eq!(holdings[0].total_cost_base, Decimal::from(25000)); // 15000 / 0.6
    }

    /// An AUD statement needs no rate at all: the Buy carries 1, which is what
    /// an AUD conversion resolves to anyway.
    #[tokio::test]
    async fn an_aud_vest_carries_the_parity_rate_because_aud_never_converts() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "AUD").await;
        insert_statement(&pool, 1, "100", "6", "AUD").await;
        let trade = db_vest(&pool, 1).await.unwrap();
        assert_eq!(trade.fx_rate, Decimal::ONE);
    }

    /// The vest is refused through the API as a 422 whose body says which
    /// month is missing and what to do about it.
    #[tokio::test]
    async fn api_vest_without_a_rate_returns_422_naming_the_month() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "USD").await;
        insert_statement(&pool, 1, "100", "150", "USD").await;
        let resp = ApiClient::over(router().with_state(pool))
            .post_empty("/ess_statements/1/vest")
            .await;
        let (status, body) = resp.status_and_body();
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
        assert!(body.contains("USD") && body.contains("2024-09"), "{body}");
    }

    #[tokio::test]
    async fn missing_statement_is_not_found() {
        let pool = test_pool().await;
        assert!(matches!(
            db_vest(&pool, 99).await,
            Err(VestError::StatementNotFound)
        ));
    }

    #[tokio::test]
    async fn second_vest_is_rejected_and_only_one_buy_exists() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "AUD").await;
        insert_statement(&pool, 1, "100", "6", "AUD").await;
        db_vest(&pool, 1).await.unwrap();
        assert!(matches!(
            db_vest(&pool, 1).await,
            Err(VestError::AlreadyVested)
        ));
        let n: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM trades WHERE ess_statement_id = 1")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(n, 1);
    }

    #[tokio::test]
    async fn zero_quantity_is_rejected() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "AUD").await;
        insert_statement(&pool, 1, "0", "6", "AUD").await;
        assert!(matches!(
            db_vest(&pool, 1).await,
            Err(VestError::NothingToVest)
        ));
    }

    /// The vest Buy is immutable individually and only removed by deleting the
    /// statement; deleting the statement removes it (when not drawn on).
    #[tokio::test]
    async fn deleting_the_statement_removes_the_vest_buy() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "AUD").await;
        insert_statement(&pool, 1, "100", "6", "AUD").await;
        let trade = db_vest(&pool, 1).await.unwrap();

        // PUT /trades on the vest Buy → rejected.
        let mut edited = trade.clone();
        edited.quantity = Decimal::from(999);
        assert!(matches!(
            trade::db_upsert(&pool, &edited).await,
            Err(trade::UpsertError::EssVestTrade)
        ));
        // DELETE /trades on the vest Buy → refused.
        assert_eq!(
            trade::db_delete(&pool, trade.id).await.unwrap(),
            trade::DeleteOutcome::Referenced
        );
        // The vest-side fields are frozen while vested…
        let mut s = ess_statement::db_get(&pool, 1).await.unwrap().unwrap();
        s.quantity = Decimal::from(999);
        assert!(matches!(
            ess_statement::db_upsert(&pool, &s).await,
            Err(ess_statement::UpsertError::Vested)
        ));
        // …but the income side stays editable (the employer's annual ESS
        // statement arrives after the vest is recorded). The revision stays
        // within the vested shares' market value, which the discount can never
        // exceed (`UpsertError::DiscountExceedsMarketValue`).
        let mut s = ess_statement::db_get(&pool, 1).await.unwrap().unwrap();
        s.deferral_discount = Decimal::from(500);
        ess_statement::db_upsert(&pool, &s).await.unwrap();
        assert_eq!(
            ess_statement::db_get(&pool, 1)
                .await
                .unwrap()
                .unwrap()
                .deferral_discount,
            Decimal::from(500)
        );

        // Deleting the statement removes the vest Buy too.
        assert_eq!(
            ess_statement::db_delete(&pool, 1).await.unwrap(),
            ess_statement::DeleteOutcome::Deleted
        );
        assert!(trade::db_get(&pool, trade.id).await.unwrap().is_none());
    }

    /// While the vest Buy is drawn on by a Sell, deleting the statement is
    /// refused (it would orphan the allocation).
    #[tokio::test]
    async fn statement_delete_refused_while_vest_is_sold() {
        use crate::entities::sell::{self, AllocationInput, SellBody};
        let pool = test_pool().await;
        insert_listing(&pool, 1, "AUD").await;
        insert_statement(&pool, 1, "100", "6", "AUD").await;
        let vest = db_vest(&pool, 1).await.unwrap();

        sell::db_upsert_sell(
            &pool,
            50,
            &SellBody {
                brokerage_includes_gst: false,
                statement_total: None,
                holding_account_id: 1,
                date: NaiveDate::from_ymd_opt(2025, 1, 10).unwrap(),
                settlement_date: Some(NaiveDate::from_ymd_opt(2025, 1, 12).unwrap()),
                listing_id: 1,
                average_price: Decimal::from(9),
                quantity: Decimal::from(40),
                currency: "AUD".to_string(),
                brokerage: Decimal::ZERO,
                gst_on_brokerage: Decimal::ZERO,
                brokerage_currency: "AUD".to_string(),
                fx_rate: Decimal::ONE,
                spot_fx_rate: None,
                contract_note_ref: None,
                allocations: vec![AllocationInput {
                    purchase_trade_id: vest.id,
                    quantity_allocated: Decimal::from(40),
                }],
            },
        )
        .await
        .unwrap();

        assert_eq!(
            ess_statement::db_delete(&pool, 1).await.unwrap(),
            ess_statement::DeleteOutcome::VestDrawnOn
        );
        // Removing the sale frees the statement to delete.
        assert_eq!(
            sell::db_delete_sell(&pool, 50).await.unwrap(),
            sell::DeleteOutcome::Deleted
        );
        assert_eq!(
            ess_statement::db_delete(&pool, 1).await.unwrap(),
            ess_statement::DeleteOutcome::Deleted
        );
    }

    #[tokio::test]
    async fn api_vest_returns_201_with_trade() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "AUD").await;
        insert_statement(&pool, 1, "100", "6", "AUD").await;
        let resp = ApiClient::over(router().with_state(pool))
            .post_empty("/ess_statements/1/vest")
            .await;
        assert_eq!(resp.status, StatusCode::CREATED);
        let trade: Trade = resp.json();
        assert_eq!(trade.trade_type, TradeType::Buy);
        assert_eq!(trade.quantity, Decimal::from(100));
    }

    #[tokio::test]
    async fn api_vest_missing_statement_returns_404() {
        let pool = test_pool().await;
        let resp = ApiClient::over(router().with_state(pool))
            .post_empty("/ess_statements/99/vest")
            .await;
        assert_eq!(resp.status, StatusCode::NOT_FOUND);
    }
}
