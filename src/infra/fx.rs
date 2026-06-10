//! AUD conversion using the ATO reference rate.
//!
//! Australian tax reporting is in AUD, so every non-AUD amount must be converted
//! to AUD before it is aggregated or compared (see CLAUDE.md "Financial
//! correctness"). The ATO directs taxpayers to the RBA's monthly F11 rates, which
//! are imported into `rba_fx_rates` as foreign currency units per 1 AUD
//! (foreign-per-AUD), so `AUD = foreign / rate`.
//!
//! [`to_aud`] converts an amount for a given currency and date: AUD passes through
//! unchanged, the ATO rate for the amount's month is used when available, and the
//! caller's manual override (e.g. a trade's `fx_rate`) is the fallback when no ATO
//! rate exists. When neither is available it fails loudly rather than substituting
//! a default — a silently unconverted or zeroed amount would corrupt a financial
//! total without failing the request.

use chrono::NaiveDate;
use rust_decimal::Decimal;
use sqlx::SqlitePool;

use crate::infra::decimal::parse_dec;

/// Why a required AUD conversion could not be performed.
#[derive(Debug)]
pub enum FxError {
    /// No ATO rate exists for this (currency, month) and no manual override was
    /// supplied. The conversion fails rather than guessing a rate.
    MissingRate { currency: String, month: String },
    /// Reading the rate from the database failed.
    Db(sqlx::Error),
}

impl std::fmt::Display for FxError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FxError::MissingRate { currency, month } => write!(
                f,
                "no ATO FX rate for {currency} in {month} and no manual override supplied"
            ),
            FxError::Db(e) => write!(f, "FX rate lookup failed: {e}"),
        }
    }
}

impl std::error::Error for FxError {}

impl From<sqlx::Error> for FxError {
    fn from(e: sqlx::Error) -> Self {
        FxError::Db(e)
    }
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
/// `date`. AUD always resolves to 1. Otherwise the ATO rate for the month of
/// `date` takes precedence; `manual_override` is used only when no ATO rate exists
/// for that (currency, month). Fails loudly when neither is available.
///
/// Public so a caller converting several amounts at the same (currency, month)
/// — e.g. `domain::cost_base` converting a cost-base breakdown — can resolve
/// the rate once instead of one lookup per amount. Apply it as `AUD = foreign
/// / rate`, passing amounts through unchanged at rate 1 (AUD and parity rates)
/// so an exact value is never reshaped by a divide, exactly as [`to_aud`] does.
pub async fn resolve_rate(
    pool: &SqlitePool,
    currency: &str,
    date: NaiveDate,
    manual_override: Option<Decimal>,
) -> Result<Decimal, FxError> {
    if currency.eq_ignore_ascii_case("AUD") {
        return Ok(Decimal::ONE);
    }
    let month = date.format("%Y-%m").to_string();
    if let Some(rate) = lookup_ato_rate(pool, currency, &month).await? {
        return Ok(rate);
    }
    manual_override.ok_or(FxError::MissingRate {
        currency: currency.to_string(),
        month,
    })
}

/// Convert `amount` (denominated in `currency`) to AUD for `date`.
///
/// `AUD = foreign / rate`, where the rate is foreign currency units per 1 AUD.
/// AUD amounts pass through unchanged (rate = 1). The ATO rate for the month of
/// `date` is preferred; `manual_override` (e.g. a trade's `fx_rate`, the same
/// foreign-per-AUD convention) is the fallback when no ATO rate exists. Fails
/// loudly via [`FxError`] when neither a rate nor an override is available.
pub async fn to_aud(
    pool: &SqlitePool,
    amount: Decimal,
    currency: &str,
    date: NaiveDate,
    manual_override: Option<Decimal>,
) -> Result<Decimal, FxError> {
    let rate = resolve_rate(pool, currency, date, manual_override).await?;
    // Pass amounts through unchanged at rate 1 (AUD and parity rates) so an exact
    // value is never reshaped by a divide.
    if rate == Decimal::ONE {
        return Ok(amount);
    }
    Ok(amount / rate)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::entities::rba_fx_rate;
    use crate::infra::db;

    async fn test_pool() -> SqlitePool {
        db::init(":memory:").await.unwrap()
    }

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
            None,
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
        let aud = to_aud(&pool, Decimal::from(1000), "USD", date(2024, 1, 15), None)
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
            Some("0.80".parse().unwrap()),
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
            Some("0.80".parse().unwrap()),
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
            Some("0.40".parse().unwrap()),
        )
        .await
        .unwrap();
        // Jan falls back to the 0.40 override → 1000 / 0.40 = 2500.
        assert_eq!(aud, Decimal::from(2500));
    }

    #[tokio::test]
    async fn fails_loudly_when_neither_rate_nor_override() {
        let pool = test_pool().await;
        let err = to_aud(&pool, Decimal::from(1000), "USD", date(2024, 1, 15), None)
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

    #[tokio::test]
    async fn malformed_stored_rate_is_an_error_not_zero() {
        let pool = test_pool().await;
        sqlx::query(
            "INSERT INTO rba_fx_rates (currency, month, rate) VALUES ('USD', '2024-01', 'oops')",
        )
        .execute(&pool)
        .await
        .unwrap();
        let err = to_aud(&pool, Decimal::from(1000), "USD", date(2024, 1, 15), None)
            .await
            .unwrap_err();
        assert!(matches!(err, FxError::Db(_)));
    }
}
