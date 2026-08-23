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
use serde::Deserialize as _;
use serde::de::{self, Deserializer, Visitor};
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

// ---------------------------------------------------------------------------
// The serde half: reading a decimal *out of an inbound HTTP request*.
//
// Distinct from `Money`/`OptMoney` above, which are the **sqlx** codec (TEXT
// column ⇄ `Decimal`) and have nothing to do with JSON. These two functions are
// the **request** codec (JSON/query-string value ⇄ `Decimal`), spelled as
// `#[serde(deserialize_with = "…")]` field attributes rather than as a newtype
// so the body struct's field stays a plain `Decimal` and every reader of it is
// unchanged.
// ---------------------------------------------------------------------------

/// What a money/quantity field is refused with when it arrives as a JSON
/// number. `serde_json` hands a JSON number over as an `f64`, which keeps only
/// ~15 significant digits, so `{"quantity": 100000000.00000001}` used to be
/// accepted `204` and stored as `100000000` — a silent loss, and silent
/// precisely because every ordinary figure survives the round trip and only the
/// long ones don't (SCENARIOS W-a).
///
/// Refusing every JSON number, integers included, is deliberate: a rule that
/// held only past some digit count would reintroduce the same silent boundary
/// in the error path that the `f64` conversion has in the value path.
///
/// The field name is not in this message because it does not need to be: axum's
/// `Json`/`Query`/`Form` rejections prefix the failing field's path (`quantity:
/// …`), exactly as they do for `deny_unknown_fields`.
pub const JSON_NUMBER_REFUSED: &str = "send this money/quantity value as a decimal string (\"12.34\", not 12.34) — a JSON number \
     is read as a 64-bit float and silently loses digits past about the 15th significant one";

/// `Decimal` field of a request body: accepts a decimal **string**, refuses a
/// JSON number with [`JSON_NUMBER_REFUSED`].
///
/// Spell it as `#[serde(deserialize_with = "crate::infra::decimal::strict_decimal")]`.
/// `infra::http::tests::every_money_request_field_refuses_a_json_number` walks
/// every handler-reachable request body and fails on a `Decimal` field that
/// lacks it, so a new body is covered without its author having to remember.
pub fn strict_decimal<'de, D>(deserializer: D) -> Result<Decimal, D::Error>
where
    D: Deserializer<'de>,
{
    // `deserialize_any`, not `deserialize_str`: with a scalar hint `serde_json`
    // answers a wrong type itself, from the hint, and the visitor never sees
    // the number — so the refusal would read "invalid type: floating point"
    // with no remedy in it. Asking what is actually there routes the number to
    // [`StrictDecimal::visit_f64`] and its message. Self-describing formats
    // only, which JSON and the query string both are.
    deserializer.deserialize_any(StrictDecimal)
}

/// The `Option<Decimal>` twin of [`strict_decimal`]: `null` and an absent field
/// are `None`, a string is parsed, a JSON number is refused.
///
/// Spell it as
/// `#[serde(default, deserialize_with = "crate::infra::decimal::strict_optional_decimal")]`.
pub fn strict_optional_decimal<'de, D>(deserializer: D) -> Result<Option<Decimal>, D::Error>
where
    D: Deserializer<'de>,
{
    deserializer.deserialize_option(StrictOptionalDecimal)
}

/// The `HashMap<_, Decimal>` form: the price-override maps the three portfolio
/// reports take (`{"prices": {"7": "58.12"}}`). Same rule for every value.
///
/// Spell it as
/// `#[serde(default, deserialize_with = "crate::infra::decimal::strict_decimal_map")]`.
pub fn strict_decimal_map<'de, D, K>(
    deserializer: D,
) -> Result<std::collections::HashMap<K, Decimal>, D::Error>
where
    D: Deserializer<'de>,
    K: serde::Deserialize<'de> + Eq + std::hash::Hash,
{
    let map = std::collections::HashMap::<K, StrictDecimalValue>::deserialize(deserializer)?;
    Ok(map.into_iter().map(|(k, v)| (k, v.0)).collect())
}

/// [`strict_decimal`] as a `Deserialize` impl, so it composes into the
/// container deserialisers serde already generates.
struct StrictDecimalValue(Decimal);

impl<'de> serde::Deserialize<'de> for StrictDecimalValue {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        strict_decimal(deserializer).map(StrictDecimalValue)
    }
}

struct StrictDecimal;

impl Visitor<'_> for StrictDecimal {
    type Value = Decimal;

    fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        f.write_str("a decimal amount as a string")
    }

    fn visit_str<E: de::Error>(self, v: &str) -> Result<Decimal, E> {
        v.trim().parse().map_err(|e: rust_decimal::Error| {
            E::custom(format!("not a decimal number: {v:?} ({e})"))
        })
    }

    // Every width of JSON number lands on one of these. `serde_json` picks the
    // visitor by the literal's shape, so all of them have to refuse.
    fn visit_f64<E: de::Error>(self, _: f64) -> Result<Decimal, E> {
        Err(E::custom(JSON_NUMBER_REFUSED))
    }

    fn visit_i64<E: de::Error>(self, _: i64) -> Result<Decimal, E> {
        Err(E::custom(JSON_NUMBER_REFUSED))
    }

    fn visit_u64<E: de::Error>(self, _: u64) -> Result<Decimal, E> {
        Err(E::custom(JSON_NUMBER_REFUSED))
    }

    fn visit_i128<E: de::Error>(self, _: i128) -> Result<Decimal, E> {
        Err(E::custom(JSON_NUMBER_REFUSED))
    }

    fn visit_u128<E: de::Error>(self, _: u128) -> Result<Decimal, E> {
        Err(E::custom(JSON_NUMBER_REFUSED))
    }
}

struct StrictOptionalDecimal;

impl<'de> Visitor<'de> for StrictOptionalDecimal {
    type Value = Option<Decimal>;

    fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        f.write_str("a decimal amount as a string, or null")
    }

    fn visit_none<E: de::Error>(self) -> Result<Option<Decimal>, E> {
        Ok(None)
    }

    fn visit_unit<E: de::Error>(self) -> Result<Option<Decimal>, E> {
        Ok(None)
    }

    fn visit_some<D: Deserializer<'de>>(self, d: D) -> Result<Option<Decimal>, D::Error> {
        strict_decimal(d).map(Some)
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

    // -----------------------------------------------------------------------
    // The serde half: a request's decimal fields
    // -----------------------------------------------------------------------

    /// A stand-in for a request body — a required decimal, a nullable one and
    /// a price-override map. Deliberately *not* named `…Body`/`…Request`, so
    /// `infra::http::tests`' two request-body guards, which take those suffixes
    /// as a request body even where no handler names the type, leave this test
    /// fixture alone.
    #[derive(Debug, serde::Deserialize)]
    #[serde(deny_unknown_fields)]
    struct MoneyFields {
        #[serde(deserialize_with = "strict_decimal")]
        quantity: Decimal,
        #[serde(default, deserialize_with = "strict_optional_decimal")]
        statement_total: Option<Decimal>,
        #[serde(default, deserialize_with = "strict_decimal_map")]
        prices: std::collections::HashMap<i64, Decimal>,
    }

    fn parse(json: &str) -> Result<MoneyFields, serde_json::Error> {
        serde_json::from_str(json)
    }

    #[test]
    fn a_decimal_string_is_read_at_full_precision() {
        // Both of the figures SCENARIOS W-a was found on, plus the sub-satoshi
        // scale that motivated it.
        for value in [
            "99999999.87654321",
            "100000000.00000001",
            "0.123456789012345678",
            "1234567890123456789.12",
        ] {
            let body = parse(&format!(r#"{{"quantity": "{value}"}}"#)).expect("a string is fine");
            assert_eq!(body.quantity.to_string(), value);
        }
    }

    #[test]
    fn a_json_number_is_refused_with_the_remedy() {
        // Integers too: a rule that only bit past some digit count would put
        // the same silent boundary back, just in the error path.
        for value in ["100000000.00000001", "58.1234", "10", "0"] {
            let err = parse(&format!(r#"{{"quantity": {value}}}"#))
                .expect_err("a JSON number must be refused")
                .to_string();
            assert!(err.contains(JSON_NUMBER_REFUSED), "{value}: {err}");
        }
        // …in the nullable and the map form as well.
        let err = parse(r#"{"quantity": "1", "statement_total": 1.5}"#)
            .expect_err("a nullable field is no different")
            .to_string();
        assert!(err.contains(JSON_NUMBER_REFUSED), "{err}");
        let err = parse(r#"{"quantity": "1", "prices": {"7": 58.12}}"#)
            .expect_err("a map value is no different")
            .to_string();
        assert!(err.contains(JSON_NUMBER_REFUSED), "{err}");
    }

    #[test]
    fn null_and_absence_stay_none_and_a_map_reads_its_values() {
        let body = parse(r#"{"quantity": "1", "statement_total": null}"#).unwrap();
        assert_eq!(body.statement_total, None);
        let body = parse(r#"{"quantity": "1"}"#).unwrap();
        assert_eq!(body.statement_total, None);
        assert!(body.prices.is_empty());

        let body = parse(r#"{"quantity": "1", "prices": {"7": "58.1234"}}"#).unwrap();
        assert_eq!(body.prices[&7], "58.1234".parse::<Decimal>().unwrap());
    }

    #[test]
    fn a_string_that_is_not_a_number_is_refused_quoting_it() {
        let err = parse(r#"{"quantity": "ten"}"#).unwrap_err().to_string();
        assert!(err.contains("not a decimal number: \"ten\""), "{err}");
        // …and not with the JSON-number remedy, which would misdirect.
        assert!(!err.contains(JSON_NUMBER_REFUSED), "{err}");
    }

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
