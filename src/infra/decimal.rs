//! Shared helpers for reading and writing TEXT-stored decimal columns.
//!
//! Financial values are persisted as TEXT to preserve arbitrary precision (see the
//! schema migration). When reading them back in report queries, a malformed value
//! must surface as an error rather than being silently coerced to zero — a wrong
//! number in a financial report is worse than a failed request.
//!
//! [`Money`] and [`OptMoney`] are the sqlx-level expression of that rule: they are the
//! TEXT⇄`Decimal` codec, so a row struct declares `#[sqlx(try_from = "Money")]` on a plain
//! `Decimal` field and derives `FromRow`, and a write binds `.bind(Money(x))` instead of
//! `.bind(x.to_string())`. Both directions then go through one type the compiler checks,
//! rather than through per-column hand-written code that review has to police.

use rust_decimal::Decimal;
use sqlx::encode::IsNull;
use sqlx::error::BoxDynError;
use sqlx::sqlite::{SqliteArgumentsBuffer, SqliteTypeInfo, SqliteValueRef};
use sqlx::{Row, Sqlite, ValueRef, sqlite::SqliteRow};

/// A `Decimal` that encodes to, and decodes from, a TEXT column.
///
/// Decoding parses the full stored string, so precision is preserved exactly and a
/// malformed value is a decode error naming the column (sqlx supplies the column name,
/// so it cannot drift from the actual query the way a hand-passed literal can).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Money(pub Decimal);

/// The nullable twin of [`Money`]: SQL `NULL` decodes to `None` and encodes back to `NULL`,
/// while a present value goes through [`Money`] (so a malformed value is still an error,
/// never a silent `None`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct OptMoney(pub Option<Decimal>);

impl From<Money> for Decimal {
    fn from(m: Money) -> Self {
        m.0
    }
}

impl From<Decimal> for Money {
    fn from(d: Decimal) -> Self {
        Money(d)
    }
}

// Orphan-legal: the local type sits in argument position.
impl From<OptMoney> for Option<Decimal> {
    fn from(m: OptMoney) -> Self {
        m.0
    }
}

impl From<Option<Decimal>> for OptMoney {
    fn from(d: Option<Decimal>) -> Self {
        OptMoney(d)
    }
}

impl sqlx::Type<Sqlite> for Money {
    fn type_info() -> SqliteTypeInfo {
        <String as sqlx::Type<Sqlite>>::type_info()
    }

    fn compatible(ty: &SqliteTypeInfo) -> bool {
        <String as sqlx::Type<Sqlite>>::compatible(ty)
    }
}

impl sqlx::Type<Sqlite> for OptMoney {
    fn type_info() -> SqliteTypeInfo {
        <Money as sqlx::Type<Sqlite>>::type_info()
    }

    fn compatible(ty: &SqliteTypeInfo) -> bool {
        <Money as sqlx::Type<Sqlite>>::compatible(ty)
    }
}

impl<'r> sqlx::Decode<'r, Sqlite> for Money {
    fn decode(value: SqliteValueRef<'r>) -> Result<Self, BoxDynError> {
        let text = <&str as sqlx::Decode<Sqlite>>::decode(value)?;
        text.parse()
            .map(Money)
            .map_err(|e: rust_decimal::Error| format!("invalid decimal {text:?} ({e})").into())
    }
}

impl<'r> sqlx::Decode<'r, Sqlite> for OptMoney {
    fn decode(value: SqliteValueRef<'r>) -> Result<Self, BoxDynError> {
        // `Row::try_get` skips the type-compatibility check for a NULL value but still calls
        // `decode`, so the NULL case has to be handled here rather than by an `Option` wrapper.
        if value.is_null() {
            return Ok(OptMoney(None));
        }
        <Money as sqlx::Decode<Sqlite>>::decode(value).map(|m| OptMoney(Some(m.0)))
    }
}

impl sqlx::Encode<'_, Sqlite> for Money {
    fn encode_by_ref(&self, buf: &mut SqliteArgumentsBuffer) -> Result<IsNull, BoxDynError> {
        <String as sqlx::Encode<Sqlite>>::encode(self.0.to_string(), buf)
    }
}

impl sqlx::Encode<'_, Sqlite> for OptMoney {
    fn encode_by_ref(&self, buf: &mut SqliteArgumentsBuffer) -> Result<IsNull, BoxDynError> {
        <Option<String> as sqlx::Encode<Sqlite>>::encode(self.0.map(|d| d.to_string()), buf)
    }
}

/// Parse a TEXT decimal column value, mapping a malformed value to a decode error
/// that names the offending column (so the failure is diagnosable in logs).
///
/// For row structs prefer [`Money`]/[`OptMoney`] with a derived `FromRow`; this stays for
/// the callers that pull a scalar out of an ad-hoc query and have no row struct to derive on.
pub fn parse_dec(column: &str, value: String) -> Result<Decimal, sqlx::Error> {
    value.parse().map_err(|e: rust_decimal::Error| {
        sqlx::Error::Decode(format!("invalid decimal in column '{column}': {value:?} ({e})").into())
    })
}

/// Read a required TEXT decimal column straight off a row via [`Money`], propagating
/// both a missing/typed-wrong column and a malformed value as errors (sqlx names the
/// column in either). For the hand-written `FromRow` impls that a derive cannot
/// express — a tagged enum whose payload columns vary by variant — and for ad-hoc
/// row reads outside a row struct.
pub fn row_dec(row: &SqliteRow, column: &str) -> Result<Decimal, sqlx::Error> {
    Ok(row.try_get::<Money, _>(column)?.0)
}

/// Read a nullable TEXT decimal column via [`OptMoney`]: `NULL` maps to `None`, a
/// present value is parsed (so a malformed value is a column-named error, never a
/// silent `None`).
pub fn row_opt_dec(row: &SqliteRow, column: &str) -> Result<Option<Decimal>, sqlx::Error> {
    Ok(row.try_get::<OptMoney, _>(column)?.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::test_pool;
    use sqlx::SqlitePool;

    /// A row struct in the shape the entities use: a required and a nullable TEXT decimal,
    /// read by the derive rather than a hand-written `FromRow`.
    #[derive(sqlx::FromRow, Debug, PartialEq)]
    struct Row {
        id: i64,
        #[sqlx(try_from = "Money")]
        amount: Decimal,
        #[sqlx(try_from = "OptMoney")]
        gross_amount: Option<Decimal>,
    }

    async fn table(pool: &SqlitePool) {
        sqlx::query("CREATE TABLE money_probe (id INTEGER PRIMARY KEY, amount TEXT NOT NULL, gross_amount TEXT)")
            .execute(pool)
            .await
            .unwrap();
    }

    async fn read(pool: &SqlitePool) -> Result<Row, sqlx::Error> {
        sqlx::query_as("SELECT id, amount, gross_amount FROM money_probe WHERE id = 1")
            .fetch_one(pool)
            .await
    }

    #[tokio::test]
    async fn round_trips_at_full_precision() {
        let pool = test_pool().await;
        table(&pool).await;
        let amount: Decimal = "123.4567890123".parse().unwrap();
        let gross: Decimal = "-0.000000000000000001".parse().unwrap();

        sqlx::query("INSERT INTO money_probe (id, amount, gross_amount) VALUES (1, ?, ?)")
            .bind(Money(amount))
            .bind(OptMoney(Some(gross)))
            .execute(&pool)
            .await
            .unwrap();

        assert_eq!(
            read(&pool).await.unwrap(),
            Row {
                id: 1,
                amount,
                gross_amount: Some(gross)
            }
        );
    }

    #[tokio::test]
    async fn opt_money_round_trips_null_as_none() {
        let pool = test_pool().await;
        table(&pool).await;

        sqlx::query("INSERT INTO money_probe (id, amount, gross_amount) VALUES (1, ?, ?)")
            .bind(Money(Decimal::ONE))
            .bind(OptMoney(None))
            .execute(&pool)
            .await
            .unwrap();

        // The NULL must survive as a NULL, not as the empty string or a zero.
        let stored: Option<String> = sqlx::query_scalar("SELECT gross_amount FROM money_probe")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(stored, None);
        assert_eq!(read(&pool).await.unwrap().gross_amount, None);
    }

    #[tokio::test]
    async fn malformed_value_is_a_decode_error_naming_the_column() {
        let pool = test_pool().await;
        table(&pool).await;
        sqlx::query("INSERT INTO money_probe (id, amount) VALUES (1, 'oops')")
            .execute(&pool)
            .await
            .unwrap();

        let err = read(&pool).await.unwrap_err().to_string();
        assert!(err.contains("amount"), "column not named: {err}");
        assert!(
            err.contains("invalid decimal \"oops\""),
            "cause not carried: {err}"
        );
    }

    #[tokio::test]
    async fn malformed_nullable_value_is_an_error_not_none() {
        let pool = test_pool().await;
        table(&pool).await;
        sqlx::query("INSERT INTO money_probe (id, amount, gross_amount) VALUES (1, '1', 'oops')")
            .execute(&pool)
            .await
            .unwrap();

        let err = read(&pool).await.unwrap_err().to_string();
        assert!(err.contains("gross_amount"), "column not named: {err}");
        assert!(
            err.contains("invalid decimal \"oops\""),
            "cause not carried: {err}"
        );
    }

    /// The read half needs no source check — `Decimal` has no `sqlx::Type<Sqlite>`
    /// impl (rust_decimal's sqlx feature is deliberately off), so a `FromRow` derive
    /// over a `Decimal` field without `#[sqlx(try_from = "Money")]` does not compile.
    /// The write half has no such backstop: `String` *is* bindable, so
    /// `.bind(x.to_string())` would silently compile and reintroduce the untyped path.
    /// This pins it out of the tree.
    #[test]
    fn no_write_binds_a_decimal_as_a_stringified_value() {
        // Assembled rather than written out so this test's own scan line, and the
        // module docs above it, are not themselves matches.
        let bind = format!(".{}(", "bind");
        let stringified = format!(".to_{}())", "string");
        let mut offenders = Vec::new();
        let mut walk = vec![std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src")];
        while let Some(dir) = walk.pop() {
            for entry in std::fs::read_dir(&dir)
                .expect("src should be readable")
                .flatten()
            {
                let path = entry.path();
                if path.is_dir() {
                    walk.push(path);
                } else if path.extension().is_some_and(|x| x == "rs") {
                    let src = std::fs::read_to_string(&path).expect("source should be readable");
                    for (n, line) in src.lines().enumerate() {
                        let code = line.trim_start();
                        if code.starts_with("//") {
                            continue;
                        }
                        if code.contains(&bind) && code.contains(&stringified) {
                            offenders.push(format!("{}:{}: {code}", path.display(), n + 1));
                        }
                    }
                }
            }
        }
        assert!(
            offenders.is_empty(),
            "bind a decimal as Money(x) / OptMoney(x), not as a string — and bind a \
             non-decimal value directly (it has its own sqlx::Type impl):\n{}",
            offenders.join("\n")
        );
    }
}
