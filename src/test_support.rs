//! Shared test fixtures (`#[cfg(test)]`-only, see the declaration in `main.rs`).
//!
//! Test modules build their data through these builders instead of spelling
//! out every `Listing`/`Trade` field — a new column is then added here once,
//! not in every test module. Each builder starts from a plausible default row
//! and exposes setters only for what tests actually vary; `insert` upserts
//! through the entity's own `db_upsert` so write-time invariants still apply.

use crate::entities::{
    amit_adjustment, amma, closing_price, ess_statement, income, listing, parcel_allocation, trade,
};
use crate::infra::db;
use chrono::NaiveDate;
use rust_decimal::Decimal;
use sqlx::SqlitePool;

/// Fresh in-memory database with migrations and seed data applied.
pub async fn test_pool() -> SqlitePool {
    db::init(":memory:").await.unwrap()
}

/// `NaiveDate` literal without the `from_ymd_opt(..).unwrap()` ceremony.
pub fn ymd(y: i32, m: u32, d: u32) -> NaiveDate {
    NaiveDate::from_ymd_opt(y, m, d).unwrap()
}

/// `Decimal` literal from a string.
pub fn dec(s: &str) -> Decimal {
    s.parse().unwrap()
}

/// Listing fixture: ASX-listed AUD ETF `T{id}` named `Test {id}`.
pub fn listing(id: i64) -> ListingBuilder {
    ListingBuilder {
        l: listing::Listing {
            id,
            exchange_mic: Some("XASX".to_string()),
            ticker: format!("T{id}"),
            name: format!("Test {id}"),
            isin: None,
            security_type: listing::SecurityType::ETF,
            currency: "AUD".to_string(),
            amit: false,
            preference: false,
            price_symbol: None,
        },
    }
}

pub struct ListingBuilder {
    l: listing::Listing,
}

impl ListingBuilder {
    pub fn ticker(mut self, ticker: &str) -> Self {
        self.l.ticker = ticker.to_string();
        self
    }

    pub fn name(mut self, name: &str) -> Self {
        self.l.name = name.to_string();
        self
    }

    pub fn mic(mut self, mic: &str) -> Self {
        self.l.exchange_mic = Some(mic.to_string());
        self
    }

    pub fn currency(mut self, currency: &str) -> Self {
        self.l.currency = currency.to_string();
        self
    }

    pub fn security_type(mut self, st: listing::SecurityType) -> Self {
        self.l.security_type = st;
        self
    }

    /// Crypto listing: no exchange MIC (CHECK-enforced pairing).
    pub fn crypto(mut self) -> Self {
        self.l.security_type = listing::SecurityType::Crypto;
        self.l.exchange_mic = None;
        self
    }

    pub fn amit(mut self, amit: bool) -> Self {
        self.l.amit = amit;
        self
    }

    pub fn preference(mut self, preference: bool) -> Self {
        self.l.preference = preference;
        self
    }

    pub fn price_symbol(mut self, symbol: &str) -> Self {
        self.l.price_symbol = Some(symbol.to_string());
        self
    }

    /// Escape hatch for fields without a dedicated setter.
    pub fn with(mut self, f: impl FnOnce(&mut listing::Listing)) -> Self {
        f(&mut self.l);
        self
    }

    pub fn build(self) -> listing::Listing {
        self.l
    }

    pub async fn insert(self, pool: &SqlitePool) {
        listing::db_upsert(pool, &self.l).await.unwrap();
    }
}

/// Closing-price fixture: a provider-fetched ok price of 10 in the listing's
/// quote currency. `.errored(msg)` turns it into a recorded fetch failure and
/// `.manual(sourced_from, reason)` into a hand-entered price — each keeps the
/// row's CHECK pairings consistent, so a test cannot build an impossible one.
pub fn closing_price(listing_id: i64, price_date: NaiveDate) -> ClosingPriceBuilder {
    ClosingPriceBuilder {
        p: closing_price::ClosingPrice {
            listing_id,
            price_date,
            price: Some(Decimal::from(10)),
            source: "test".to_string(),
            fetched_at: "2026-06-05T08:00:00Z".to_string(),
            status: closing_price::PriceStatus::Ok,
            error: None,
            origin: closing_price::PriceOrigin::Fetched,
            sourced_from: None,
            reason: None,
        },
    }
}

pub struct ClosingPriceBuilder {
    p: closing_price::ClosingPrice,
}

impl ClosingPriceBuilder {
    pub fn price(mut self, price: &str) -> Self {
        self.p.price = Some(dec(price));
        self
    }

    pub fn source(mut self, source: &str) -> Self {
        self.p.source = source.to_string();
        self
    }

    pub fn fetched_at(mut self, fetched_at: &str) -> Self {
        self.p.fetched_at = fetched_at.to_string();
        self
    }

    /// A recorded fetch failure: no price, the message stored (CHECK-paired).
    pub fn errored(mut self, error: &str) -> Self {
        self.p.price = None;
        self.p.status = closing_price::PriceStatus::Error;
        self.p.error = Some(error.to_string());
        self
    }

    /// A price entered by hand, with the provenance the schema requires: the
    /// `source` moves to `manual` in step with the origin (CHECK-paired).
    pub fn manual(mut self, sourced_from: &str, reason: &str) -> Self {
        self.p.source = closing_price::MANUAL_SOURCE.to_string();
        self.p.origin = closing_price::PriceOrigin::Manual;
        self.p.sourced_from = Some(sourced_from.to_string());
        self.p.reason = Some(reason.to_string());
        self
    }

    pub async fn insert(self, pool: &SqlitePool) {
        closing_price::db_store(pool, &self.p).await.unwrap();
    }
}

/// Buy fixture: 100 units @ 10 AUD on 2024-01-01 (T+2), no brokerage,
/// default holding account.
pub fn buy(id: i64, listing_id: i64) -> TradeBuilder {
    TradeBuilder::new(id, listing_id, trade::TradeType::Buy)
}

/// Sell fixture: same defaults as [`buy`].
pub fn sell(id: i64, listing_id: i64) -> TradeBuilder {
    TradeBuilder::new(id, listing_id, trade::TradeType::Sell)
}

/// DRP fixture: same defaults as [`buy`].
pub fn drp(id: i64, listing_id: i64) -> TradeBuilder {
    TradeBuilder::new(id, listing_id, trade::TradeType::DRP)
}

/// Trade fixture with an explicit type, for helpers parameterised over
/// Buy/Sell. Same defaults as [`buy`].
pub fn trade(id: i64, listing_id: i64, trade_type: trade::TradeType) -> TradeBuilder {
    TradeBuilder::new(id, listing_id, trade_type)
}

pub struct TradeBuilder {
    t: trade::Trade,
    settlement_overridden: bool,
}

impl TradeBuilder {
    fn new(id: i64, listing_id: i64, trade_type: trade::TradeType) -> Self {
        let date = ymd(2024, 1, 1);
        TradeBuilder {
            t: trade::Trade {
                id,
                trade_type,
                date,
                settlement_date: date + chrono::Duration::days(2),
                listing_id,
                average_price: Decimal::from(10),
                quantity: Decimal::from(100),
                currency: "AUD".to_string(),
                brokerage: Decimal::ZERO,
                gst_on_brokerage: Decimal::ZERO,
                brokerage_includes_gst: false,
                brokerage_currency: "AUD".to_string(),
                fx_rate: Decimal::ONE,
                spot_fx_rate: None,
                contract_note_ref: None,
                statement_total: None,
                residual_brought_forward: Decimal::ZERO,
                residual_carried_forward: Decimal::ZERO,
                residual_paid_out: Decimal::ZERO,
                rights_action_id: None,
                buyback_action_id: None,
                scrip_action_id: None,
                demerger_action_id: None,
                worthless_action_id: None,
                deemed_acquisition_date: None,
                holding_account_id: 1,
                transfer_id: None,
                ess_statement_id: None,
                inheritance_id: None,
            },
            settlement_overridden: false,
        }
    }

    /// Sets the trade date; the settlement date follows at T+2 unless
    /// [`Self::settlement`] pinned it.
    pub fn date(mut self, date: NaiveDate) -> Self {
        self.t.date = date;
        if !self.settlement_overridden {
            self.t.settlement_date = date + chrono::Duration::days(2);
        }
        self
    }

    pub fn settlement(mut self, date: NaiveDate) -> Self {
        self.t.settlement_date = date;
        self.settlement_overridden = true;
        self
    }

    pub fn qty(mut self, qty: Decimal) -> Self {
        self.t.quantity = qty;
        self
    }

    pub fn price(mut self, price: Decimal) -> Self {
        self.t.average_price = price;
        self
    }

    /// Sets the trade currency (and the brokerage currency with it — tests
    /// never split the two).
    pub fn currency(mut self, currency: &str) -> Self {
        self.t.currency = currency.to_string();
        self.t.brokerage_currency = currency.to_string();
        self
    }

    pub fn fx_rate(mut self, fx_rate: Decimal) -> Self {
        self.t.fx_rate = fx_rate;
        self
    }

    /// Sets the deliberate transaction-date spot-rate override (wins over the
    /// ATO monthly rate; see `infra::fx::FxOverride`).
    pub fn spot_fx_rate(mut self, spot: Decimal) -> Self {
        self.t.spot_fx_rate = Some(spot);
        self
    }

    pub fn brokerage(mut self, brokerage: Decimal) -> Self {
        self.t.brokerage = brokerage;
        self
    }

    pub fn gst_on_brokerage(mut self, gst: Decimal) -> Self {
        self.t.gst_on_brokerage = gst;
        self
    }

    pub fn account(mut self, holding_account_id: i64) -> Self {
        self.t.holding_account_id = holding_account_id;
        self
    }

    /// Escape hatch for fields without a dedicated setter.
    pub fn with(mut self, f: impl FnOnce(&mut trade::Trade)) -> Self {
        f(&mut self.t);
        self
    }

    pub fn build(self) -> trade::Trade {
        self.t
    }

    pub async fn insert(self, pool: &SqlitePool) {
        trade::db_upsert(pool, &self.t).await.unwrap();
    }
}

/// AMMA statement fixture: 100 units over FY ending 2024-06-30, received
/// 2024-08-15, every amount zero, AUD, default holding account.
pub fn amma(id: i64, listing_id: i64) -> AmmaBuilder {
    AmmaBuilder {
        a: amma::AmmaStatement {
            id,
            listing_id,
            tax_year_end_date: ymd(2024, 6, 30),
            units_held: Decimal::from(100),
            date_received: ymd(2024, 8, 15),
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
            cost_base_adjustment: Decimal::ZERO,
            tfn_withholding_tax: Decimal::ZERO,
            currency: "AUD".to_string(),
            holding_account_id: 1,
        },
    }
}

pub struct AmmaBuilder {
    a: amma::AmmaStatement,
}

impl AmmaBuilder {
    pub fn units(mut self, units: Decimal) -> Self {
        self.a.units_held = units;
        self
    }

    pub fn cost_base_adjustment(mut self, per_unit: Decimal) -> Self {
        self.a.cost_base_adjustment = per_unit;
        self
    }

    /// Escape hatch for fields without a dedicated setter.
    pub fn with(mut self, f: impl FnOnce(&mut amma::AmmaStatement)) -> Self {
        f(&mut self.a);
        self
    }

    pub fn build(self) -> amma::AmmaStatement {
        self.a
    }

    pub async fn insert(self, pool: &SqlitePool) {
        amma::db_upsert(pool, &self.a).await.unwrap();
    }
}

/// Income fixture: every amount zero, no dates beyond `date_paid`, AUD,
/// default holding account. Vary fields via [`IncomeBuilder::with`].
pub fn income(id: i64, listing_id: i64, date_paid: NaiveDate) -> IncomeBuilder {
    IncomeBuilder {
        i: income::Income {
            id,
            listing_id,
            date_paid,
            ex_date: None,
            franked_amount: Decimal::ZERO,
            unfranked_amount: Decimal::ZERO,
            foreign_source_income: Decimal::ZERO,
            foreign_tax_paid: Decimal::ZERO,
            tfn_withholding_tax: Decimal::ZERO,
            franking_credits: Decimal::ZERO,
            lic_capital_gain_deduction: Decimal::ZERO,
            conduit_foreign_income: Decimal::ZERO,
            trust_income: false,
            entitlement_date: None,
            reinvestment_trade_id: None,
            currency: "AUD".to_string(),
            buyback_trade_id: None,
            holding_account_id: 1,
            amount_per_security: None,
            securities_held: None,
            tax_deferred_amount: None,
        },
    }
}

pub struct IncomeBuilder {
    i: income::Income,
}

impl IncomeBuilder {
    /// Escape hatch for fields without a dedicated setter.
    pub fn with(mut self, f: impl FnOnce(&mut income::Income)) -> Self {
        f(&mut self.i);
        self
    }

    pub fn build(self) -> income::Income {
        self.i
    }

    pub async fn insert(self, pool: &SqlitePool) {
        income::db_upsert(pool, &self.i).await.unwrap();
    }
}

/// ESS statement fixture: every amount zero, AUD, default holding account.
/// Vary fields via [`EssStatementBuilder::with`].
pub fn ess_statement(id: i64, listing_id: i64, taxing_point: NaiveDate) -> EssStatementBuilder {
    EssStatementBuilder {
        s: ess_statement::EssStatement {
            id,
            listing_id,
            holding_account_id: 1,
            taxing_point_date: taxing_point,
            quantity: Decimal::ZERO,
            market_value_per_share: Decimal::ZERO,
            taxed_upfront_eligible: Decimal::ZERO,
            taxed_upfront_not_eligible: Decimal::ZERO,
            deferral_discount: Decimal::ZERO,
            pre_2009_cessation_discount: Decimal::ZERO,
            foreign_source_discount: Decimal::ZERO,
            tfn_withholding: Decimal::ZERO,
            currency: "AUD".to_string(),
            aud_taxed_upfront_eligible: None,
            aud_taxed_upfront_not_eligible: None,
            aud_deferral_discount: None,
            aud_pre_2009_cessation_discount: None,
            aud_foreign_source_discount: None,
            vest_trade_id: None,
        },
    }
}

pub struct EssStatementBuilder {
    s: ess_statement::EssStatement,
}

impl EssStatementBuilder {
    /// Escape hatch for fields without a dedicated setter.
    pub fn with(mut self, f: impl FnOnce(&mut ess_statement::EssStatement)) -> Self {
        f(&mut self.s);
        self
    }

    pub fn build(self) -> ess_statement::EssStatement {
        self.s
    }

    pub async fn insert(self, pool: &SqlitePool) {
        ess_statement::db_upsert(pool, &self.s).await.unwrap();
    }
}

/// AMIT adjustment linking an AMMA statement's per-unit cost-base adjustment
/// to `qty` units of a parcel.
pub async fn amit_adjustment(
    pool: &SqlitePool,
    id: i64,
    amma_id: i64,
    trade_id: i64,
    qty: Decimal,
) {
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

/// Parcel allocation linking a Sell to the Buy/DRP parcel it consumes.
pub async fn allocate(pool: &SqlitePool, id: i64, sale_id: i64, buy_id: i64, qty: Decimal) {
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
