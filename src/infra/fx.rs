//! AUD conversion using the ATO reference rate.
//!
//! Australian tax reporting is in AUD, so every non-AUD amount must be converted
//! to AUD before it is aggregated or compared (see CLAUDE.md "Financial
//! correctness"). The ATO directs taxpayers to the RBA's monthly F11 rates, which
//! are imported into `rba_fx_rates` as foreign currency units per 1 AUD
//! (foreign-per-AUD), so `AUD = foreign / rate`.
//!
//! [`to_aud`] converts an amount for a given currency and date: AUD passes through
//! unchanged, and the caller's per-record rate ([`FxOverride`]) interacts with the
//! ATO monthly rate per the one precedence rule in [`pick_rate`]: a deliberate
//! spot-rate override wins outright (QC 18020 — an average rate is not a
//! reasonable approximation for a one-off purchase or sale of a large capital
//! asset), otherwise the ATO rate for the amount's month is used when available,
//! with the trade's `fx_rate` as the fallback when no ATO rate exists. When none
//! is available it fails loudly rather than substituting a default — a silently
//! unconverted or zeroed amount would corrupt a financial total without failing
//! the request.
//!
//! **Valuation-only fallback**: the RBA publishes a month's average rate only
//! after the month ends, so a *current-month* valuation (a report snapshot, a
//! live quote) would otherwise be blocked all month. [`resolve_valuation_rate`]
//! / [`FxRates::resolve_valuation_rate`] substitute the most recent earlier
//! month's rate — bounded by [`VALUATION_FALLBACK_MONTHS`] — and say so in the
//! result ([`ValuationRate::provisional`]) so the caller flags the output as
//! provisional, never silently. Only valuation paths (snapshot generation in
//! `reports::snapshot`, live-quote conversion in `entities::closing_price`,
//! and the period-performance report's FX attribution in
//! `reports::period_performance`, both over `reports::valuation`) may call
//! these; tax calculations and FY reports keep the strict [`resolve_rate`]
//! rule so no tax figure can ever be computed from a fallback-month rate.

use chrono::{Datelike, NaiveDate};
use rust_decimal::Decimal;
use sqlx::{Row, SqlitePool};
use std::collections::HashMap;

use crate::infra::decimal::parse_dec;

/// The per-record manual rate a conversion carries (foreign-per-AUD, same
/// convention as the ATO rate), distinguishing how it interacts with the
/// imported monthly rate — the distinction `Option<Decimal>` couldn't make.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum FxOverride {
    /// No per-record rate (income, AMMA statements, live quotes, …): the ATO
    /// monthly rate is the only source, missing means a loud failure.
    #[default]
    None,
    /// A trade's `fx_rate`: used only when no ATO rate exists for the
    /// (currency, month) — the ATO rate takes precedence once imported.
    Fallback(Decimal),
    /// A trade's `spot_fx_rate`: a deliberate transaction-date spot rate that
    /// wins over the ATO monthly rate. QC 18020 (Examples 5/7,
    /// `docs/ato/forex-average-rates.md`) permits average rates only where
    /// they reasonably approximate the spot rates at the translation times,
    /// and says they are not appropriate for a one-off purchase or sale of a
    /// large capital asset.
    Spot(Decimal),
}

impl FxOverride {
    /// The override a trade row carries: its deliberate `spot_fx_rate` when
    /// set, else its `fx_rate` fallback. Every trade-amount conversion goes
    /// through this so no caller can re-derive the precedence differently.
    pub fn from_trade(fx_rate: Decimal, spot_fx_rate: Option<Decimal>) -> FxOverride {
        match spot_fx_rate {
            Some(spot) => FxOverride::Spot(spot),
            None => FxOverride::Fallback(fx_rate),
        }
    }
}

/// Why a required AUD conversion could not be performed.
#[derive(thiserror::Error, Debug)]
pub enum FxError {
    /// No ATO rate exists for this (currency, month) and no manual override was
    /// supplied. The conversion fails rather than guessing a rate.
    #[error("no ATO FX rate for {currency} in {month} and no manual override supplied")]
    MissingRate { currency: String, month: String },
    /// Reading the rate from the database failed.
    #[error("FX rate lookup failed: {0}")]
    Db(#[from] sqlx::Error),
}

/// Surface an FX failure through report code that returns `sqlx::Error`: a DB
/// error passes through unchanged; a missing rate becomes a decode error so it
/// fails loudly (HTTP 500) rather than being silently swallowed or zeroed.
impl From<FxError> for sqlx::Error {
    fn from(e: FxError) -> Self {
        match e {
            FxError::Db(inner) => inner,
            missing => sqlx::Error::Decode(missing.to_string().into()),
        }
    }
}

/// The ATO reference rate for `currency` in `month` ('YYYY-MM'), if one has been
/// imported. Propagates a malformed stored rate as a decode error.
async fn lookup_ato_rate(
    pool: &SqlitePool,
    currency: &str,
    month: &str,
) -> Result<Option<Decimal>, sqlx::Error> {
    let raw: Option<String> =
        sqlx::query_scalar("SELECT rate FROM rba_fx_rates WHERE currency = ? AND month = ?")
            .bind(currency)
            .bind(month)
            .fetch_optional(pool)
            .await?;
    raw.map(|r| parse_dec("rate", r)).transpose()
}

/// Resolve the foreign-per-AUD rate to apply when converting `currency` for
/// `date`. AUD always resolves to 1. Otherwise [`pick_rate`] arbitrates
/// between the record's [`FxOverride`] and the ATO rate for the month of
/// `date`. Fails loudly when no rate is available.
///
/// Every production caller now resolves through the pre-loaded [`FxRates`]
/// (reports) or [`resolve_valuation_rate`] (valuation paths), so this
/// DB-lookup twin is test-only: the tests pin that both paths resolve
/// identically.
#[cfg(test)]
pub async fn resolve_rate(
    pool: &SqlitePool,
    currency: &str,
    date: NaiveDate,
    manual: FxOverride,
) -> Result<Decimal, FxError> {
    if currency.eq_ignore_ascii_case("AUD") {
        return Ok(Decimal::ONE);
    }
    let month = date.format("%Y-%m").to_string();
    let ato_rate = lookup_ato_rate(pool, currency, &month).await?;
    pick_rate(ato_rate, currency, month, manual)
}

/// The shared precedence rule (used by both the DB-lookup path and the
/// pre-loaded [`FxRates`] map): a deliberate spot override wins outright, the
/// ATO rate for the month is next, a fallback override is used only when no
/// ATO rate exists, and none means a loud [`FxError::MissingRate`].
fn pick_rate(
    ato_rate: Option<Decimal>,
    currency: &str,
    month: String,
    manual: FxOverride,
) -> Result<Decimal, FxError> {
    if let FxOverride::Spot(rate) = manual {
        return Ok(rate);
    }
    if let Some(rate) = ato_rate {
        return Ok(rate);
    }
    if let FxOverride::Fallback(rate) = manual {
        return Ok(rate);
    }
    Err(FxError::MissingRate {
        currency: currency.to_string(),
        month,
    })
}

/// Apply a resolved foreign-per-AUD rate: `AUD = foreign / rate`, passing
/// amounts through unchanged at rate 1 (AUD and parity rates) so an exact
/// value is never reshaped by a divide.
pub fn apply_rate(amount: Decimal, rate: Decimal) -> Decimal {
    if rate == Decimal::ONE {
        amount
    } else {
        amount / rate
    }
}

/// How many months before the valuation month [`resolve_valuation_rate`] may
/// reach back for a rate. Beyond this the conversion fails loudly, exactly as
/// the strict path does: the RBA import runs weekly, so a gap this old means
/// the import has been broken for months and a "provisional" value would be
/// meaningless.
pub const VALUATION_FALLBACK_MONTHS: u32 = 2;

/// A foreign-per-AUD rate resolved for *valuation* (never tax): the rate to
/// apply via [`apply_rate`], plus whether it is an earlier month's substitute.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ValuationRate {
    pub rate: Decimal,
    /// The valuation month has no imported rate yet and an earlier month's
    /// (at most [`VALUATION_FALLBACK_MONTHS`] back) was substituted. The
    /// caller must surface this — the snapshot `provisional` flag, the live
    /// row's provisional annotation — never treat the value as final.
    pub provisional: bool,
}

/// The valuation month of `date` and its fallback predecessors, newest first:
/// `['YYYY-MM'; 1 + VALUATION_FALLBACK_MONTHS]`.
fn valuation_months(date: NaiveDate) -> Vec<String> {
    let (mut y, mut m) = (date.year(), date.month());
    let mut months = vec![format!("{y:04}-{m:02}")];
    for _ in 0..VALUATION_FALLBACK_MONTHS {
        (y, m) = if m == 1 { (y - 1, 12) } else { (y, m - 1) };
        months.push(format!("{y:04}-{m:02}"));
    }
    months
}

/// Resolve the rate to *value* an amount of `currency` at `date`: the ATO rate
/// for the valuation month when imported, else the most recent earlier month's
/// rate (at most [`VALUATION_FALLBACK_MONTHS`] back, flagged provisional),
/// else a loud [`FxError::MissingRate`] for the valuation month.
///
/// Valuation-only: exactly the snapshot-generation and live-quote paths may
/// call this. Tax calculations and FY reports must use the strict
/// [`resolve_rate`] / [`FxRates::resolve_rate`] so no tax figure is ever
/// computed from a fallback-month rate.
pub async fn resolve_valuation_rate(
    pool: &SqlitePool,
    currency: &str,
    date: NaiveDate,
) -> Result<ValuationRate, FxError> {
    if currency.eq_ignore_ascii_case("AUD") {
        return Ok(ValuationRate {
            rate: Decimal::ONE,
            provisional: false,
        });
    }
    let months = valuation_months(date);
    for (i, month) in months.iter().enumerate() {
        if let Some(rate) = lookup_ato_rate(pool, currency, month).await? {
            return Ok(ValuationRate {
                rate,
                provisional: i > 0,
            });
        }
    }
    Err(FxError::MissingRate {
        currency: currency.to_string(),
        month: months.into_iter().next().expect("valuation month present"),
    })
}

/// Every imported ATO/RBA reference rate, pre-loaded into memory.
///
/// Report loops convert one amount per row; resolving each conversion with a
/// DB round-trip (the [`to_aud`] path) is an N+1. Loading the whole
/// `rba_fx_rates` table once — it is small (one row per currency-month) — lets
/// the rest of the report run as pure computation over in-memory data, and a
/// load inside the report's read transaction sees the same snapshot as the
/// report's other queries.
#[derive(Debug, Clone, Default)]
pub struct FxRates {
    /// (currency, 'YYYY-MM') → foreign currency units per 1 AUD.
    rates: HashMap<(String, String), Decimal>,
}

impl FxRates {
    /// Load every `rba_fx_rates` row. Executor-generic so it can run on a
    /// report's read transaction as well as the pool.
    pub async fn load<'e, E>(executor: E) -> Result<FxRates, sqlx::Error>
    where
        E: sqlx::Executor<'e, Database = sqlx::Sqlite>,
    {
        let rows = sqlx::query("SELECT currency, month, rate FROM rba_fx_rates")
            .fetch_all(executor)
            .await?;
        let mut rates = HashMap::new();
        for row in &rows {
            let currency: String = row.try_get("currency")?;
            let month: String = row.try_get("month")?;
            let rate = parse_dec("rate", row.try_get("rate")?)?;
            rates.insert((currency, month), rate);
        }
        Ok(FxRates { rates })
    }

    /// Build a rate map directly — for pool-free unit tests of report
    /// computations.
    #[cfg(test)]
    pub fn from_rates<'a>(rates: impl IntoIterator<Item = (&'a str, &'a str, Decimal)>) -> FxRates {
        FxRates {
            rates: rates
                .into_iter()
                .map(|(currency, month, rate)| ((currency.to_string(), month.to_string()), rate))
                .collect(),
        }
    }

    /// [`resolve_rate`], over the pre-loaded map: AUD resolves to 1, otherwise
    /// [`pick_rate`] arbitrates between `manual` and the ATO rate for the
    /// month of `date`, failing loudly when no rate is available.
    pub fn resolve_rate(
        &self,
        currency: &str,
        date: NaiveDate,
        manual: FxOverride,
    ) -> Result<Decimal, FxError> {
        if currency.eq_ignore_ascii_case("AUD") {
            return Ok(Decimal::ONE);
        }
        let month = date.format("%Y-%m").to_string();
        let ato_rate = self
            .rates
            .get(&(currency.to_string(), month.clone()))
            .copied();
        pick_rate(ato_rate, currency, month, manual)
    }

    /// [`resolve_valuation_rate`], over the pre-loaded map — same valuation-only
    /// restriction: snapshot generation and live-quote conversion only.
    pub fn resolve_valuation_rate(
        &self,
        currency: &str,
        date: NaiveDate,
    ) -> Result<ValuationRate, FxError> {
        if currency.eq_ignore_ascii_case("AUD") {
            return Ok(ValuationRate {
                rate: Decimal::ONE,
                provisional: false,
            });
        }
        let months = valuation_months(date);
        for (i, month) in months.iter().enumerate() {
            if let Some(&rate) = self.rates.get(&(currency.to_string(), month.clone())) {
                return Ok(ValuationRate {
                    rate,
                    provisional: i > 0,
                });
            }
        }
        Err(FxError::MissingRate {
            currency: currency.to_string(),
            month: months.into_iter().next().expect("valuation month present"),
        })
    }

    /// [`to_aud`], over the pre-loaded map.
    pub fn to_aud(
        &self,
        amount: Decimal,
        currency: &str,
        date: NaiveDate,
        manual: FxOverride,
    ) -> Result<Decimal, FxError> {
        Ok(apply_rate(
            amount,
            self.resolve_rate(currency, date, manual)?,
        ))
    }
}

/// Convert `amount` (denominated in `currency`) to AUD for `date`.
///
/// `AUD = foreign / rate`, where the rate is foreign currency units per 1 AUD.
/// AUD amounts pass through unchanged (rate = 1). `manual` and the ATO rate
/// for the month of `date` are arbitrated by [`pick_rate`] — a spot override
/// wins, the ATO rate is next, a fallback override is used only when no ATO
/// rate exists. Fails loudly via [`FxError`] when no rate is available.
///
/// Test-only, like [`resolve_rate`]: production conversions go through the
/// pre-loaded [`FxRates`] map (reports load it once per run) or the
/// valuation-only [`resolve_valuation_rate`]; the tests here pin that the
/// DB-lookup path and the map resolve identically.
#[cfg(test)]
pub async fn to_aud(
    pool: &SqlitePool,
    amount: Decimal,
    currency: &str,
    date: NaiveDate,
    manual: FxOverride,
) -> Result<Decimal, FxError> {
    Ok(apply_rate(
        amount,
        resolve_rate(pool, currency, date, manual).await?,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::entities::rba_fx_rate;
    use crate::test_support::test_pool;

    fn date(y: i32, m: u32, d: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, d).unwrap()
    }

    #[tokio::test]
    async fn aud_passes_through_without_a_rate() {
        let pool = test_pool().await;
        // No rba_fx_rates rows at all: AUD must still convert (rate = 1).
        let aud = to_aud(
            &pool,
            "1234.56".parse().unwrap(),
            "AUD",
            date(2024, 1, 15),
            FxOverride::None,
        )
        .await
        .unwrap();
        assert_eq!(aud, "1234.56".parse::<Decimal>().unwrap());
    }

    #[tokio::test]
    async fn ato_rate_used_when_present() {
        let pool = test_pool().await;
        // A$1 = 0.50 USD → AUD = USD / 0.50.
        rba_fx_rate::db_import_rate(&pool, "USD", "2024-01", "0.50".parse().unwrap())
            .await
            .unwrap();
        let aud = to_aud(
            &pool,
            Decimal::from(1000),
            "USD",
            date(2024, 1, 15),
            FxOverride::None,
        )
        .await
        .unwrap();
        assert_eq!(aud, Decimal::from(2000));
    }

    #[tokio::test]
    async fn ato_rate_takes_precedence_over_manual_override() {
        let pool = test_pool().await;
        rba_fx_rate::db_import_rate(&pool, "USD", "2024-01", "0.50".parse().unwrap())
            .await
            .unwrap();
        // Override would give a different answer; the ATO rate must win.
        let aud = to_aud(
            &pool,
            Decimal::from(1000),
            "USD",
            date(2024, 1, 15),
            FxOverride::Fallback("0.80".parse().unwrap()),
        )
        .await
        .unwrap();
        assert_eq!(aud, Decimal::from(2000));
    }

    #[tokio::test]
    async fn manual_override_used_when_no_ato_rate() {
        let pool = test_pool().await;
        // No USD rate imported for this month → fall back to the override (0.80).
        let aud = to_aud(
            &pool,
            Decimal::from(800),
            "USD",
            date(2024, 1, 15),
            FxOverride::Fallback("0.80".parse().unwrap()),
        )
        .await
        .unwrap();
        assert_eq!(aud, Decimal::from(1000));
    }

    #[tokio::test]
    async fn ato_rate_used_only_for_its_own_month() {
        let pool = test_pool().await;
        // Rate exists for Feb but not Jan; a Jan amount must fall back to the override.
        rba_fx_rate::db_import_rate(&pool, "USD", "2024-02", "0.50".parse().unwrap())
            .await
            .unwrap();
        let aud = to_aud(
            &pool,
            Decimal::from(1000),
            "USD",
            date(2024, 1, 15),
            FxOverride::Fallback("0.40".parse().unwrap()),
        )
        .await
        .unwrap();
        // Jan falls back to the 0.40 override → 1000 / 0.40 = 2500.
        assert_eq!(aud, Decimal::from(2500));
    }

    #[tokio::test]
    async fn fails_loudly_when_neither_rate_nor_override() {
        let pool = test_pool().await;
        let err = to_aud(
            &pool,
            Decimal::from(1000),
            "USD",
            date(2024, 1, 15),
            FxOverride::None,
        )
        .await
        .unwrap_err();
        match err {
            FxError::MissingRate { currency, month } => {
                assert_eq!(currency, "USD");
                assert_eq!(month, "2024-01");
            }
            other => panic!("expected MissingRate, got {other:?}"),
        }
    }

    // FxRates: the pre-loaded map must resolve exactly like the DB-lookup path.

    async fn loaded_rates(pool: &SqlitePool) -> FxRates {
        FxRates::load(pool).await.unwrap()
    }

    #[tokio::test]
    async fn fx_rates_aud_passes_through_without_a_rate() {
        let pool = test_pool().await;
        let rates = loaded_rates(&pool).await;
        let aud = rates
            .to_aud(
                "1234.56".parse().unwrap(),
                "AUD",
                date(2024, 1, 15),
                FxOverride::None,
            )
            .unwrap();
        assert_eq!(aud, "1234.56".parse::<Decimal>().unwrap());
    }

    #[tokio::test]
    async fn fx_rates_ato_rate_takes_precedence_and_is_month_scoped() {
        let pool = test_pool().await;
        rba_fx_rate::db_import_rate(&pool, "USD", "2024-01", "0.50".parse().unwrap())
            .await
            .unwrap();
        let rates = loaded_rates(&pool).await;
        // The ATO rate wins over the override in its own month…
        let aud = rates
            .to_aud(
                Decimal::from(1000),
                "USD",
                date(2024, 1, 15),
                FxOverride::Fallback("0.80".parse().unwrap()),
            )
            .unwrap();
        assert_eq!(aud, Decimal::from(2000));
        // …and a different month falls back to the override.
        let aud = rates
            .to_aud(
                Decimal::from(1000),
                "USD",
                date(2024, 2, 15),
                FxOverride::Fallback("0.40".parse().unwrap()),
            )
            .unwrap();
        assert_eq!(aud, Decimal::from(2500));
    }

    #[tokio::test]
    async fn fx_rates_fails_loudly_when_neither_rate_nor_override() {
        let pool = test_pool().await;
        let rates = loaded_rates(&pool).await;
        let err = rates
            .to_aud(
                Decimal::from(1000),
                "USD",
                date(2024, 1, 15),
                FxOverride::None,
            )
            .unwrap_err();
        match err {
            FxError::MissingRate { currency, month } => {
                assert_eq!(currency, "USD");
                assert_eq!(month, "2024-01");
            }
            other => panic!("expected MissingRate, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn fx_rates_load_propagates_malformed_stored_rate() {
        let pool = test_pool().await;
        sqlx::query(
            "INSERT INTO rba_fx_rates (currency, month, rate) VALUES ('USD', '2024-01', 'oops')",
        )
        .execute(&pool)
        .await
        .unwrap();
        assert!(FxRates::load(&pool).await.is_err());
    }

    #[tokio::test]
    async fn malformed_stored_rate_is_an_error_not_zero() {
        let pool = test_pool().await;
        sqlx::query(
            "INSERT INTO rba_fx_rates (currency, month, rate) VALUES ('USD', '2024-01', 'oops')",
        )
        .execute(&pool)
        .await
        .unwrap();
        let err = to_aud(
            &pool,
            Decimal::from(1000),
            "USD",
            date(2024, 1, 15),
            FxOverride::None,
        )
        .await
        .unwrap_err();
        assert!(matches!(err, FxError::Db(_)));
    }

    // Spot override (QC 18020): a deliberate transaction-date spot rate wins
    // over the imported monthly rate — and still converts when no monthly
    // rate exists (it is first in precedence, not merely a fallback).

    #[tokio::test]
    async fn spot_override_wins_over_ato_rate() {
        let pool = test_pool().await;
        rba_fx_rate::db_import_rate(&pool, "USD", "2024-01", "0.50".parse().unwrap())
            .await
            .unwrap();
        // 1000 / 0.40 = 2500, not 1000 / 0.50 = 2000.
        let aud = to_aud(
            &pool,
            Decimal::from(1000),
            "USD",
            date(2024, 1, 15),
            FxOverride::Spot("0.40".parse().unwrap()),
        )
        .await
        .unwrap();
        assert_eq!(aud, Decimal::from(2500));
    }

    #[tokio::test]
    async fn spot_override_converts_when_no_ato_rate_exists() {
        let pool = test_pool().await;
        let aud = to_aud(
            &pool,
            Decimal::from(1000),
            "USD",
            date(2024, 1, 15),
            FxOverride::Spot("0.40".parse().unwrap()),
        )
        .await
        .unwrap();
        assert_eq!(aud, Decimal::from(2500));
    }

    #[tokio::test]
    async fn fx_rates_spot_override_wins_over_ato_rate() {
        let pool = test_pool().await;
        rba_fx_rate::db_import_rate(&pool, "USD", "2024-01", "0.50".parse().unwrap())
            .await
            .unwrap();
        let rates = loaded_rates(&pool).await;
        // The pre-loaded map applies the same precedence as the DB path.
        let aud = rates
            .to_aud(
                Decimal::from(1000),
                "USD",
                date(2024, 1, 15),
                FxOverride::Spot("0.40".parse().unwrap()),
            )
            .unwrap();
        assert_eq!(aud, Decimal::from(2500));
    }

    // Valuation-only fallback: the current month's missing rate falls back to
    // the most recent earlier month (≤ VALUATION_FALLBACK_MONTHS), flagged
    // provisional; a real-month rate is not flagged; beyond the bound it fails
    // loudly like the strict path. Checked on both the DB-lookup path (live
    // quotes) and the pre-loaded map (snapshot generation).

    #[tokio::test]
    async fn valuation_rate_prefers_the_real_month_unflagged() {
        let pool = test_pool().await;
        rba_fx_rate::db_import_rate(&pool, "USD", "2026-06", "0.50".parse().unwrap())
            .await
            .unwrap();
        rba_fx_rate::db_import_rate(&pool, "USD", "2026-05", "0.40".parse().unwrap())
            .await
            .unwrap();
        let vr = resolve_valuation_rate(&pool, "USD", date(2026, 6, 15))
            .await
            .unwrap();
        assert_eq!(vr.rate, "0.50".parse::<Decimal>().unwrap());
        assert!(!vr.provisional, "the real month's rate is not provisional");

        let rates = loaded_rates(&pool).await;
        assert_eq!(
            rates
                .resolve_valuation_rate("USD", date(2026, 6, 15))
                .unwrap(),
            vr
        );
    }

    #[tokio::test]
    async fn valuation_rate_falls_back_at_most_two_months_flagged_provisional() {
        let pool = test_pool().await;
        // Only April's rate exists; June resolves to it (2 months back),
        // flagged provisional. July (3 months after April) fails loudly.
        rba_fx_rate::db_import_rate(&pool, "USD", "2026-04", "0.40".parse().unwrap())
            .await
            .unwrap();
        let vr = resolve_valuation_rate(&pool, "USD", date(2026, 6, 15))
            .await
            .unwrap();
        assert_eq!(vr.rate, "0.40".parse::<Decimal>().unwrap());
        assert!(vr.provisional, "a fallback-month rate must be flagged");

        let err = resolve_valuation_rate(&pool, "USD", date(2026, 7, 1))
            .await
            .unwrap_err();
        match err {
            FxError::MissingRate { currency, month } => {
                assert_eq!(currency, "USD");
                assert_eq!(month, "2026-07", "the error names the valuation month");
            }
            other => panic!("expected MissingRate, got {other:?}"),
        }

        let rates = loaded_rates(&pool).await;
        assert_eq!(
            rates
                .resolve_valuation_rate("USD", date(2026, 6, 15))
                .unwrap(),
            vr
        );
        assert!(matches!(
            rates.resolve_valuation_rate("USD", date(2026, 7, 1)),
            Err(FxError::MissingRate { .. })
        ));
    }

    #[tokio::test]
    async fn valuation_rate_fallback_crosses_a_year_boundary() {
        let pool = test_pool().await;
        rba_fx_rate::db_import_rate(&pool, "USD", "2025-12", "0.40".parse().unwrap())
            .await
            .unwrap();
        let vr = resolve_valuation_rate(&pool, "USD", date(2026, 1, 10))
            .await
            .unwrap();
        assert_eq!(vr.rate, "0.40".parse::<Decimal>().unwrap());
        assert!(vr.provisional);
    }

    #[tokio::test]
    async fn valuation_rate_aud_is_always_final() {
        let pool = test_pool().await;
        let vr = resolve_valuation_rate(&pool, "AUD", date(2026, 6, 15))
            .await
            .unwrap();
        assert_eq!(vr.rate, Decimal::ONE);
        assert!(!vr.provisional);
    }

    #[test]
    fn from_trade_maps_spot_over_fallback() {
        let fx_rate: Decimal = "0.80".parse().unwrap();
        let spot: Decimal = "0.40".parse().unwrap();
        assert_eq!(
            FxOverride::from_trade(fx_rate, Some(spot)),
            FxOverride::Spot(spot)
        );
        assert_eq!(
            FxOverride::from_trade(fx_rate, None),
            FxOverride::Fallback(fx_rate)
        );
    }
}
