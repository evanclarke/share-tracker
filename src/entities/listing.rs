use crate::domain::tax_year::tax_year_for;
use crate::infra::http::{self, ApiError, CrudEntity};
use axum::{
    Json, Router,
    extract::{Path, State},
    http::StatusCode,
    routing::get,
};
use chrono::{Datelike, NaiveDate};
use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;

// Variants are serialized verbatim to JSON and persisted to the TEXT
// `security_type` column (matched by a CHECK constraint), so the acronym
// spellings are the wire/storage format and must not be camel-cased.
#[allow(clippy::upper_case_acronyms)]
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, sqlx::Type)]
pub enum SecurityType {
    Share,
    ETF,
    LIC,
    Trust,
    /// A crypto asset held as an investment: a CGT asset like the others
    /// (docs/ato/crypto-cgt.md), but listed on no exchange — `exchange_mic`
    /// is NULL, settlement is same-day, and the ticker must be a recognised
    /// digital-token code in `currencies` (kind DigitalToken).
    Crypto,
}

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct Listing {
    pub id: i64,
    /// NULL exactly for Crypto listings (CHECK-enforced): a crypto asset
    /// trades on no MIC-coded venue. Exchange-less listings are unique by
    /// ticker (partial unique index); the rest by (exchange_mic, ticker).
    pub exchange_mic: Option<String>,
    pub ticker: String,
    pub name: String,
    pub isin: Option<String>,
    pub security_type: SecurityType,
    pub currency: String,
    pub amit: bool,
    /// The date the fund *became* an AMIT, when it was not always one: every
    /// reader of `amit` compares its record's own date against this, so a
    /// fund that converted part-way through a holding is an ordinary trust
    /// for the earlier years and an AMIT from here on (SCENARIOS F-23).
    /// `None` — the ordinary case — means the flag applies to the whole
    /// history. Only meaningful on a listing with `amit` set, which
    /// [`db_upsert`] enforces.
    pub amit_from: Option<NaiveDate>,
    /// Preference share: the franking-credit holding-period rule requires 90
    /// at-risk days instead of 45 (see `reports::franking`).
    pub preference: bool,
    /// Provider-symbol override: `closing_price::yahoo_symbol` uses this
    /// verbatim, ahead of its derived ticker/exchange mapping, for a symbol
    /// the provider spells differently or an exchange with no mapping.
    /// Independent of `listing_rename`'s rename chain — set directly via
    /// `PUT`, since it rarely needs to change in lockstep with a rename (an
    /// override that matched the old ticker rarely matches the new one).
    pub price_symbol: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct ListingBody {
    #[serde(default)]
    pub exchange_mic: Option<String>,
    pub ticker: String,
    pub name: String,
    pub isin: Option<String>,
    pub security_type: SecurityType,
    pub currency: String,
    pub amit: bool,
    #[serde(default)]
    pub amit_from: Option<NaiveDate>,
    #[serde(default)]
    pub preference: bool,
    #[serde(default)]
    pub price_symbol: Option<String>,
}

/// The `422` body for a Crypto listing carrying an exchange. Shared with
/// `entities::listing_rename`, which can move a listing between exchanges and
/// so meets the same pairing.
pub(crate) const CRYPTO_WITH_EXCHANGE: &str = "a Crypto listing has no exchange — a crypto asset trades on no MIC-coded venue, \
     so leave exchange_mic blank (its ticker is the digital-token code instead)";

/// The `422` body for the other direction: an exchange-listed security with no
/// exchange.
pub(crate) const EXCHANGE_REQUIRED: &str = "this listing needs an exchange — only a Crypto listing is exchange-less, so set \
     exchange_mic to the venue's MIC (or make it a Crypto listing)";

/// Why a listing upsert was refused.
#[derive(thiserror::Error, Debug)]
pub enum UpsertError {
    /// A Crypto listing's ticker is not a recognised digital-token code in
    /// `currencies` (kind DigitalToken — the ISO 24165 / DTIF list, matched on
    /// the DTI `code` or the human `short_name` ticker the import carries).
    #[error("a Crypto listing's ticker must be a recognised digital-token code")]
    UnrecognisedDigitalToken,
    /// A plain `PUT` tried to change `ticker` or `exchange_mic` on a listing
    /// that already has dependent trades, income, or closing prices. Once a
    /// listing has history, an identity change must go through
    /// `POST /listings/:id/rename` (`entities::listing_rename`) so the change
    /// is recorded as a dated event, not silently lost — a brand-new listing
    /// with no dependents yet stays freely editable.
    #[error("a ticker or exchange change on a listing with history needs POST /rename")]
    IdentityChangeRequiresRename,
    /// `amit_from` was supplied on a listing that is not an AMIT at all. The
    /// date says *when the fund became* an AMIT; without the flag there is no
    /// status for it to date, and a reader comparing against it would treat
    /// the listing as an ordinary trust forever — a stored value that means
    /// nothing (SCENARIOS F-23). Mapped to `422`.
    #[error("amit_from is set on a listing that is not an AMIT")]
    AmitFromWithoutAmit,
    /// `amit_from` is not a 1 July date (carries the rejected date). AMIT
    /// status is *elected for an income year*
    /// (`docs/ato/amit-reporting-requirements.md`), so it turns on at a year
    /// boundary: a mid-year date would split one financial year's treatment
    /// in two, leaving the same year's income partly attributed and partly
    /// assessed as ordinary trust income. Mapped to `422`.
    #[error("amit_from {0} is not a 1 July date")]
    AmitFromNotFinancialYearStart(NaiveDate),
    /// A `Crypto` listing was given an `exchange_mic`. The table's CHECK
    /// catches it too, but only as a raw constraint expression the web UI
    /// shows verbatim — this variant is what says which side is wrong
    /// (SCENARIOS L-09).
    #[error("a Crypto listing was given an exchange")]
    CryptoWithExchange,
    /// A non-`Crypto` listing was written with no `exchange_mic`. The other
    /// half of the same CHECK, and the other half of SCENARIOS L-09.
    #[error("a non-Crypto listing was written without an exchange")]
    ExchangeRequired,
    /// Constraint violations (duplicate ticker, unknown exchange or currency)
    /// surface here via the table's CHECKs and FKs. The exchange/security-type
    /// pairing is checked above, before its CHECK can fire.
    #[error("listing write failed: {0}")]
    Db(#[from] sqlx::Error),
}

impl From<UpsertError> for ApiError {
    fn from(e: UpsertError) -> Self {
        match e {
            UpsertError::UnrecognisedDigitalToken => ApiError::unprocessable(
                "a Crypto listing's ticker must be a recognised digital-token code",
            ),
            UpsertError::CryptoWithExchange => ApiError::unprocessable(CRYPTO_WITH_EXCHANGE),
            UpsertError::ExchangeRequired => ApiError::unprocessable(EXCHANGE_REQUIRED),
            UpsertError::IdentityChangeRequiresRename => ApiError::unprocessable(
                "use POST /listings/:id/rename to record a ticker or exchange change \
                 on a listing with recorded trades, income, or prices",
            ),
            UpsertError::AmitFromWithoutAmit => ApiError::unprocessable(
                "amit_from dates when the fund became an AMIT, so it needs amit set — \
                 leave it out for a listing that is not an AMIT",
            ),
            UpsertError::AmitFromNotFinancialYearStart(date) => ApiError::unprocessable(format!(
                "amit_from {date} is not a 1 July date — AMIT status is elected for a whole \
                 income year, so it starts at one: use the 1 July the fund's first AMIT \
                 financial year began"
            )),
            UpsertError::Db(err) => err.into(),
        }
    }
}

impl CrudEntity for Listing {
    type Key = i64;
    const TABLE: &'static str = "listings";
    const COLUMNS: &'static str = "id, exchange_mic, ticker, name, isin, security_type, currency, amit, \
         amit_from, preference, price_symbol";
    const ORDER_BY: &'static str = "exchange_mic, ticker";
    const NOUN: &'static str = "listing";
}

/// Was this listing an AMIT in the Australian financial year `tax_year`
/// (identified by the calendar year of its 30 June end)?
///
/// *The* rule behind every reader of the flag — the income entity's
/// write-time checks, the [tax summary](crate::reports::tax_summary)'s
/// whole-row exclusion, the [AMIT cash
/// cross-check](crate::reports::amit_cash_cross_check), the annual tax
/// report's completeness section, and the `ReturnOfCapital` refusal — so a
/// fund that converted part-way through a holding cannot be an AMIT to one
/// reader and an ordinary trust to another (SCENARIOS F-23).
///
/// `amit_from` is a 1 July date (write-time enforced), so this is a
/// whole-year comparison: AMIT status is elected for an income year
/// (`docs/ato/amit-reporting-requirements.md`), never for part of one. `None`
/// means the flag applies to the whole recorded history — the ordinary case,
/// a fund that has always been an AMIT.
pub fn amit_in_tax_year(amit: bool, amit_from: Option<NaiveDate>, tax_year: i32) -> bool {
    amit && amit_from.is_none_or(|from| tax_year_for(from) <= tax_year)
}

pub fn router() -> Router<SqlitePool> {
    Router::new()
        .route("/listings", get(http::list_handler::<Listing>))
        .route(
            "/listings/{id}",
            get(http::get_handler::<Listing>)
                .put(upsert)
                // Deleting a listing still referenced by trades/income violates an FK → 422.
                .delete(http::delete_handler::<Listing>),
        )
}

pub async fn db_get(pool: &SqlitePool, id: i64) -> Result<Option<Listing>, sqlx::Error> {
    http::crud_get(pool, id).await
}

pub async fn db_upsert(pool: &SqlitePool, listing: &Listing) -> Result<(), UpsertError> {
    let mut tx = pool.begin().await?;

    // An identity change (ticker or exchange) on a listing that already has
    // history must go through the rename action instead of a bare field
    // edit, so the change is recorded rather than silently lost — a
    // brand-new listing with no dependents yet stays freely editable here.
    let current: Option<(String, Option<String>)> =
        sqlx::query_as("SELECT ticker, exchange_mic FROM listings WHERE id = ?")
            .bind(listing.id)
            .fetch_optional(&mut *tx)
            .await?;
    if let Some((old_ticker, old_exchange_mic)) = current
        && (old_ticker != listing.ticker || old_exchange_mic != listing.exchange_mic)
    {
        let has_dependents: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM trades WHERE listing_id = ?1) \
                 OR EXISTS(SELECT 1 FROM income WHERE listing_id = ?1) \
                 OR EXISTS(SELECT 1 FROM closing_prices WHERE listing_id = ?1)",
        )
        .bind(listing.id)
        .fetch_one(&mut *tx)
        .await?;
        if has_dependents {
            return Err(UpsertError::IdentityChangeRequiresRename);
        }
    }

    // The date only means something on a listing that is an AMIT: it says
    // when the fund *became* one (SCENARIOS F-23). Checked here because
    // SQLite cannot add a table-level CHECK to an existing table, and a
    // column CHECK cannot reference another column (migration 0024).
    if let Some(from) = listing.amit_from {
        if !listing.amit {
            return Err(UpsertError::AmitFromWithoutAmit);
        }
        // A whole-year boundary: see `amit_in_tax_year`.
        if (from.month(), from.day()) != (7, 1) {
            return Err(UpsertError::AmitFromNotFinancialYearStart(from));
        }
    }

    // The exchange/security-type pairing: a crypto asset trades on no
    // MIC-coded venue, every other security does. The table CHECKs this too,
    // but a CHECK can only answer with its own expression, and this is the
    // pair of mistakes a user makes while adding a crypto listing — so each
    // direction is named here first (SCENARIOS L-09).
    match (listing.security_type, &listing.exchange_mic) {
        (SecurityType::Crypto, Some(_)) => return Err(UpsertError::CryptoWithExchange),
        (other, None) if other != SecurityType::Crypto => {
            return Err(UpsertError::ExchangeRequired);
        }
        _ => {}
    }

    // A Crypto listing's ticker must be a recognised digital-token code
    // (validated in the write transaction; ticker uniqueness stays
    // index-enforced by the table itself).
    if listing.security_type == SecurityType::Crypto {
        let recognised: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM currencies \
             WHERE kind = 'DigitalToken' AND (code = ?1 OR short_name = ?1))",
        )
        .bind(&listing.ticker)
        .fetch_one(&mut *tx)
        .await?;
        if !recognised {
            return Err(UpsertError::UnrecognisedDigitalToken);
        }
    }
    sqlx::query(
        "INSERT INTO listings \
         (id, exchange_mic, ticker, name, isin, security_type, currency, amit, amit_from, \
          preference, price_symbol) \
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?) \
         ON CONFLICT(id) DO UPDATE SET \
             exchange_mic  = excluded.exchange_mic, \
             ticker        = excluded.ticker, \
             name          = excluded.name, \
             isin          = excluded.isin, \
             security_type = excluded.security_type, \
             currency      = excluded.currency, \
             amit          = excluded.amit, \
             amit_from     = excluded.amit_from, \
             preference    = excluded.preference, \
             price_symbol  = excluded.price_symbol",
    )
    .bind(listing.id)
    .bind(&listing.exchange_mic)
    .bind(&listing.ticker)
    .bind(&listing.name)
    .bind(&listing.isin)
    .bind(listing.security_type)
    .bind(listing.currency.as_str())
    .bind(listing.amit)
    .bind(listing.amit_from)
    .bind(listing.preference)
    .bind(&listing.price_symbol)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(())
}

#[cfg(test)]
pub async fn db_delete(pool: &SqlitePool, id: i64) -> Result<bool, sqlx::Error> {
    http::crud_delete::<Listing>(pool, id).await
}

async fn upsert(
    State(pool): State<SqlitePool>,
    Path(id): Path<i64>,
    Json(body): Json<ListingBody>,
) -> Result<StatusCode, ApiError> {
    let listing = Listing {
        id,
        exchange_mic: body.exchange_mic,
        ticker: body.ticker,
        name: body.name,
        isin: body.isin,
        security_type: body.security_type,
        currency: body.currency,
        amit: body.amit,
        amit_from: body.amit_from,
        preference: body.preference,
        price_symbol: body.price_symbol,
    };
    db_upsert(&pool, &listing).await?;
    Ok(StatusCode::NO_CONTENT)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{ApiClient, test_pool, ymd};

    /// Client over this module's own routes.
    fn client(pool: &SqlitePool) -> ApiClient {
        ApiClient::over(router().with_state(pool.clone()))
    }

    fn xtest() -> Listing {
        crate::test_support::listing(1)
            .ticker("VAS")
            .name("Vanguard Australian Shares ETF")
            .amit(true)
            .with(|l| l.isin = Some("AU0000VASAU4".to_string()))
            .build()
    }

    /// An exchange-less Crypto listing: BTC is a seeded digital-token code.
    fn crypto() -> Listing {
        crate::test_support::listing(2)
            .crypto()
            .ticker("BTC")
            .name("Bitcoin")
            .build()
    }

    // DB-level tests

    #[tokio::test]
    async fn db_insert_and_retrieve() {
        let pool = test_pool().await;
        db_upsert(&pool, &xtest()).await.unwrap();
        let got = db_get(&pool, 1).await.unwrap().unwrap();
        assert_eq!(got.ticker, "VAS");
        assert_eq!(got.exchange_mic.as_deref(), Some("XASX"));
        assert_eq!(got.isin, Some("AU0000VASAU4".to_string()));
        assert!(got.amit);
    }

    #[tokio::test]
    async fn db_get_missing_returns_none() {
        let pool = test_pool().await;
        assert!(db_get(&pool, 999).await.unwrap().is_none());
    }

    /// SCENARIOS F-23: the AMIT status is dated, and the date is a whole-year
    /// boundary — AMIT status is elected for an income year, so a mid-year
    /// date would split one year's treatment in two. It also needs the flag
    /// it dates.
    #[tokio::test]
    async fn db_amit_from_must_be_a_1_july_date_on_an_amit_listing() {
        let pool = test_pool().await;
        let with_from = |from: Option<NaiveDate>, amit: bool| {
            crate::test_support::listing(1)
                .ticker("VAS")
                .amit(amit)
                .with(|l| l.amit_from = from)
                .build()
        };

        db_upsert(&pool, &with_from(Some(ymd(2023, 7, 1)), true))
            .await
            .unwrap();
        assert_eq!(
            db_get(&pool, 1).await.unwrap().unwrap().amit_from,
            Some(ymd(2023, 7, 1))
        );

        for bad in [ymd(2023, 6, 30), ymd(2023, 7, 2), ymd(2024, 1, 1)] {
            let err = db_upsert(&pool, &with_from(Some(bad), true))
                .await
                .unwrap_err();
            assert!(
                matches!(err, UpsertError::AmitFromNotFinancialYearStart(d) if d == bad),
                "{err:?}"
            );
        }

        let err = db_upsert(&pool, &with_from(Some(ymd(2023, 7, 1)), false))
            .await
            .unwrap_err();
        assert!(matches!(err, UpsertError::AmitFromWithoutAmit), "{err:?}");
    }

    /// The shared rule every reader of the flag asks: an undated flag covers
    /// the whole history, and a dated one turns on with its own financial
    /// year — 1 July 2023 makes FY2024 the first AMIT year.
    #[test]
    fn amit_in_tax_year_turns_on_with_the_dated_financial_year() {
        assert!(amit_in_tax_year(true, None, 2019));
        assert!(!amit_in_tax_year(false, None, 2019));

        let from = Some(ymd(2023, 7, 1));
        assert!(!amit_in_tax_year(true, from, 2022));
        assert!(!amit_in_tax_year(true, from, 2023));
        assert!(amit_in_tax_year(true, from, 2024));
        assert!(amit_in_tax_year(true, from, 2025));
    }

    #[tokio::test]
    async fn db_upsert_updates_existing() {
        let pool = test_pool().await;
        db_upsert(&pool, &xtest()).await.unwrap();
        let mut updated = xtest();
        updated.name = "Updated ETF".to_string();
        updated.amit = false;
        db_upsert(&pool, &updated).await.unwrap();
        let got = db_get(&pool, 1).await.unwrap().unwrap();
        assert_eq!(got.name, "Updated ETF");
        assert!(!got.amit);
    }

    /// A ticker or exchange change on a listing with no recorded trades,
    /// income, or prices is still a plain field edit — no history exists yet
    /// for a rename event to protect.
    #[tokio::test]
    async fn db_ticker_change_allowed_with_no_dependents() {
        let pool = test_pool().await;
        db_upsert(&pool, &xtest()).await.unwrap();
        let mut renamed = xtest();
        renamed.ticker = "VASX".to_string();
        db_upsert(&pool, &renamed).await.unwrap();
        assert_eq!(db_get(&pool, 1).await.unwrap().unwrap().ticker, "VASX");
    }

    /// Once a listing has a trade, a bare `PUT` changing its ticker or
    /// exchange is refused — `entities::listing_rename` is the only path
    /// once there is history to protect. A same-identity edit (e.g. just the
    /// name) still goes through.
    #[tokio::test]
    async fn db_ticker_or_exchange_change_refused_once_dependents_exist() {
        let pool = test_pool().await;
        db_upsert(&pool, &xtest()).await.unwrap();
        crate::test_support::buy(1, 1).insert(&pool).await;

        let mut ticker_changed = xtest();
        ticker_changed.ticker = "VASX".to_string();
        assert!(matches!(
            db_upsert(&pool, &ticker_changed).await.unwrap_err(),
            UpsertError::IdentityChangeRequiresRename
        ));

        let mut exchange_changed = xtest();
        exchange_changed.exchange_mic = Some("XNYS".to_string());
        assert!(matches!(
            db_upsert(&pool, &exchange_changed).await.unwrap_err(),
            UpsertError::IdentityChangeRequiresRename
        ));

        // A same-identity edit (name only) is unaffected.
        let mut renamed_only = xtest();
        renamed_only.name = "Renamed ETF".to_string();
        db_upsert(&pool, &renamed_only).await.unwrap();
        assert_eq!(db_get(&pool, 1).await.unwrap().unwrap().name, "Renamed ETF");
        // The ticker is still untouched by the refused attempts.
        assert_eq!(db_get(&pool, 1).await.unwrap().unwrap().ticker, "VAS");
    }

    #[tokio::test]
    async fn db_preference_flag_round_trips_and_defaults_false() {
        let pool = test_pool().await;
        // Default: an ordinary listing is not a preference share.
        db_upsert(&pool, &xtest()).await.unwrap();
        assert!(!db_get(&pool, 1).await.unwrap().unwrap().preference);
        // The flag persists when set.
        let mut pref = xtest();
        pref.preference = true;
        db_upsert(&pool, &pref).await.unwrap();
        assert!(db_get(&pool, 1).await.unwrap().unwrap().preference);
    }

    #[tokio::test]
    async fn db_price_symbol_round_trips_and_defaults_none() {
        let pool = test_pool().await;
        db_upsert(&pool, &xtest()).await.unwrap();
        assert_eq!(db_get(&pool, 1).await.unwrap().unwrap().price_symbol, None);
        let mut with_symbol = xtest();
        with_symbol.price_symbol = Some("VAS.AX".to_string());
        db_upsert(&pool, &with_symbol).await.unwrap();
        assert_eq!(
            db_get(&pool, 1).await.unwrap().unwrap().price_symbol,
            Some("VAS.AX".to_string())
        );
    }

    #[tokio::test]
    async fn db_delete_removes_listing() {
        let pool = test_pool().await;
        db_upsert(&pool, &xtest()).await.unwrap();
        assert!(db_delete(&pool, 1).await.unwrap());
        assert!(db_get(&pool, 1).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn db_delete_missing_returns_false() {
        let pool = test_pool().await;
        assert!(!db_delete(&pool, 999).await.unwrap());
    }

    #[tokio::test]
    async fn db_fk_constraint_rejects_unknown_exchange() {
        let pool = test_pool().await;
        let mut bad = xtest();
        bad.exchange_mic = Some("XXXX".to_string());
        let UpsertError::Db(err) = db_upsert(&pool, &bad).await.unwrap_err() else {
            panic!("expected a DB error");
        };
        assert!(
            err.to_string().contains("FOREIGN KEY"),
            "expected FK error, got: {err}"
        );
    }

    #[tokio::test]
    async fn db_fk_constraint_rejects_unknown_currency() {
        let pool = test_pool().await;
        // 'ZZZ' is not a recognised currency (no row in `currencies`).
        let mut bad = xtest();
        bad.currency = "ZZZ".to_string();
        let UpsertError::Db(err) = db_upsert(&pool, &bad).await.unwrap_err() else {
            panic!("expected a DB error");
        };
        assert!(
            err.to_string().contains("FOREIGN KEY"),
            "expected currency FK error, got: {err}"
        );
        // A seeded currency is accepted.
        let mut ok = xtest();
        ok.currency = "AUD".to_string();
        db_upsert(&pool, &ok).await.unwrap();
    }

    #[tokio::test]
    async fn db_crypto_listing_round_trips_without_exchange() {
        let pool = test_pool().await;
        db_upsert(&pool, &crypto()).await.unwrap();
        let got = db_get(&pool, 2).await.unwrap().unwrap();
        assert_eq!(got.exchange_mic, None);
        assert_eq!(got.ticker, "BTC");
        assert_eq!(got.security_type, SecurityType::Crypto);
    }

    #[tokio::test]
    async fn db_crypto_ticker_must_be_a_recognised_digital_token() {
        let pool = test_pool().await;
        // 'DOGE' has no DigitalToken row in `currencies`: rejected, nothing
        // persisted (a Crypto listing's ticker is its token code).
        let mut bad = crypto();
        bad.ticker = "DOGE".to_string();
        assert!(matches!(
            db_upsert(&pool, &bad).await.unwrap_err(),
            UpsertError::UnrecognisedDigitalToken
        ));
        assert!(db_get(&pool, 2).await.unwrap().is_none());
        // A fiat code is no better: BTC must be a *DigitalToken* row.
        bad.ticker = "USD".to_string();
        assert!(matches!(
            db_upsert(&pool, &bad).await.unwrap_err(),
            UpsertError::UnrecognisedDigitalToken
        ));
    }

    /// SCENARIOS L-09. The exchange/security-type pairing is answered by name
    /// in each direction, not by the table's CHECK expression — the two
    /// mistakes a user makes while adding a crypto listing are the two the web
    /// UI has to be able to explain. The CHECK stays behind them as the
    /// backstop for a write that never went through `db_upsert`.
    #[tokio::test]
    async fn db_pairing_of_exchange_and_security_type_is_refused_by_name() {
        let pool = test_pool().await;
        // A Crypto listing with an exchange...
        let mut bad = crypto();
        bad.exchange_mic = Some("XASX".to_string());
        assert!(matches!(
            db_upsert(&pool, &bad).await.unwrap_err(),
            UpsertError::CryptoWithExchange
        ));
        // ...and a non-Crypto listing without one.
        let mut bare = xtest();
        bare.exchange_mic = None;
        assert!(matches!(
            db_upsert(&pool, &bare).await.unwrap_err(),
            UpsertError::ExchangeRequired
        ));
        assert!(db_get(&pool, 2).await.unwrap().is_none());

        // The CHECK still holds against a write that bypasses `db_upsert`.
        let err = sqlx::query(
            "INSERT INTO listings (id, exchange_mic, ticker, name, security_type, currency, amit) \
             VALUES (9, 'XASX', 'BTC', 'Bitcoin', 'Crypto', 'AUD', 0)",
        )
        .execute(&pool)
        .await
        .unwrap_err();
        assert!(
            err.to_string().contains("CHECK"),
            "expected CHECK error, got: {err}"
        );
    }

    #[tokio::test]
    async fn db_duplicate_exchange_less_ticker_rejected() {
        let pool = test_pool().await;
        // UNIQUE(exchange_mic, ticker) treats NULLs as distinct, so the
        // partial unique index must catch a second exchange-less BTC listing.
        db_upsert(&pool, &crypto()).await.unwrap();
        let mut dup = crypto();
        dup.id = 3;
        let UpsertError::Db(err) = db_upsert(&pool, &dup).await.unwrap_err() else {
            panic!("expected a DB error");
        };
        assert!(
            err.to_string().contains("UNIQUE"),
            "expected UNIQUE error, got: {err}"
        );
    }

    // API-level tests

    #[tokio::test]
    async fn api_list_returns_ok() {
        let pool = test_pool().await;
        db_upsert(&pool, &xtest()).await.unwrap();
        let resp = client(&pool).get("/listings").await;
        assert_eq!(resp.status, StatusCode::OK);
        let listings: Vec<Listing> = resp.json();
        assert_eq!(listings.len(), 1);
        assert_eq!(listings[0].ticker, "VAS");
    }

    #[tokio::test]
    async fn api_get_existing_returns_listing() {
        let pool = test_pool().await;
        db_upsert(&pool, &xtest()).await.unwrap();
        let resp = client(&pool).get("/listings/1").await;
        assert_eq!(resp.status, StatusCode::OK);
        let l: Listing = resp.json();
        assert_eq!(l.ticker, "VAS");
    }

    #[tokio::test]
    async fn api_get_missing_returns_404() {
        let pool = test_pool().await;
        let resp = client(&pool).get("/listings/999").await;
        assert_eq!(resp.status, StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn api_upsert_creates_listing() {
        let pool = test_pool().await;
        let body = serde_json::json!({
            "exchange_mic": "XASX",
            "ticker": "VAS",
            "name": "Vanguard Australian Shares ETF",
            "isin": null,
            "security_type": "ETF",
            "currency": "AUD",
            "amit": true
        });
        let resp = client(&pool).put("/listings/1", &body).await;
        assert_eq!(resp.status, StatusCode::NO_CONTENT);
        assert!(db_get(&pool, 1).await.unwrap().is_some());
    }

    #[tokio::test]
    async fn api_upsert_unknown_currency_returns_422() {
        let pool = test_pool().await;
        // 'ZZZ' has no row in `currencies`: the currency FK rejects the write, and
        // the handler surfaces a constraint violation as 422, not 500.
        let body = serde_json::json!({
            "exchange_mic": "XASX",
            "ticker": "VAS",
            "name": "Vanguard Australian Shares ETF",
            "isin": null,
            "security_type": "ETF",
            "currency": "ZZZ",
            "amit": true
        });
        let resp = client(&pool).put("/listings/1", &body).await;
        assert_eq!(resp.status, StatusCode::UNPROCESSABLE_ENTITY);
    }

    #[tokio::test]
    async fn api_crypto_listing_round_trips_without_exchange() {
        let pool = test_pool().await;
        // No exchange_mic in the body at all: the field defaults to null,
        // which is exactly what a Crypto listing requires.
        let body = serde_json::json!({
            "ticker": "BTC",
            "name": "Bitcoin",
            "isin": null,
            "security_type": "Crypto",
            "currency": "AUD",
            "amit": false
        });
        let resp = client(&pool).put("/listings/2", &body).await;
        assert_eq!(resp.status, StatusCode::NO_CONTENT);
        let got = db_get(&pool, 2).await.unwrap().unwrap();
        assert_eq!(got.exchange_mic, None);
        assert_eq!(got.security_type, SecurityType::Crypto);
    }

    #[tokio::test]
    async fn api_invalid_crypto_listings_return_422() {
        let pool = test_pool().await;
        for body in [
            // Unrecognised digital-token ticker.
            serde_json::json!({
                "ticker": "DOGE", "name": "Dogecoin", "isin": null,
                "security_type": "Crypto", "currency": "AUD", "amit": false
            }),
            // A Crypto listing with an exchange.
            serde_json::json!({
                "exchange_mic": "XASX", "ticker": "BTC", "name": "Bitcoin", "isin": null,
                "security_type": "Crypto", "currency": "AUD", "amit": false
            }),
            // A non-Crypto listing without one.
            serde_json::json!({
                "ticker": "VAS", "name": "Vanguard", "isin": null,
                "security_type": "ETF", "currency": "AUD", "amit": false
            }),
        ] {
            let resp = client(&pool).put("/listings/2", &body).await;
            assert_eq!(
                resp.status,
                StatusCode::UNPROCESSABLE_ENTITY,
                "body: {body}"
            );
        }

        // Each pairing direction says which side is wrong, in its own words —
        // never the CHECK expression (SCENARIOS L-09).
        let resp = client(&pool)
            .put(
                "/listings/2",
                &serde_json::json!({
                    "exchange_mic": "XASX", "ticker": "BTC", "name": "Bitcoin", "isin": null,
                    "security_type": "Crypto", "currency": "AUD", "amit": false
                }),
            )
            .await;
        let detail = resp.text().to_string();
        assert!(detail.contains("has no exchange"), "detail: {detail}");
        assert!(!detail.contains("CHECK"), "detail: {detail}");

        let resp = client(&pool)
            .put(
                "/listings/2",
                &serde_json::json!({
                    "ticker": "VAS", "name": "Vanguard", "isin": null,
                    "security_type": "ETF", "currency": "AUD", "amit": false
                }),
            )
            .await;
        let detail = resp.text().to_string();
        assert!(detail.contains("needs an exchange"), "detail: {detail}");
        assert!(!detail.contains("CHECK"), "detail: {detail}");

        // The unrecognised-digital-token rejection says why, not a bare "HTTP 422".
        let resp = client(&pool)
            .put(
                "/listings/2",
                &serde_json::json!({
                    "ticker": "DOGE", "name": "Dogecoin", "isin": null,
                    "security_type": "Crypto", "currency": "AUD", "amit": false
                }),
            )
            .await;
        let detail = resp.text().to_string();
        assert!(detail.contains("digital-token"), "detail: {detail}");
    }

    #[tokio::test]
    async fn api_upsert_updates_listing() {
        let pool = test_pool().await;
        db_upsert(&pool, &xtest()).await.unwrap();
        let body = serde_json::json!({
            "exchange_mic": "XASX",
            "ticker": "VAS",
            "name": "Renamed ETF",
            "isin": null,
            "security_type": "ETF",
            "currency": "AUD",
            "amit": false
        });
        let resp = client(&pool).put("/listings/1", &body).await;
        assert_eq!(resp.status, StatusCode::NO_CONTENT);
        let got = db_get(&pool, 1).await.unwrap().unwrap();
        assert_eq!(got.name, "Renamed ETF");
        assert!(!got.amit);
    }

    #[tokio::test]
    async fn api_delete_existing_returns_no_content() {
        let pool = test_pool().await;
        db_upsert(&pool, &xtest()).await.unwrap();
        let resp = client(&pool).delete("/listings/1").await;
        assert_eq!(resp.status, StatusCode::NO_CONTENT);
    }

    #[tokio::test]
    async fn api_delete_missing_returns_404() {
        let pool = test_pool().await;
        let resp = client(&pool).delete("/listings/999").await;
        assert_eq!(resp.status, StatusCode::NOT_FOUND);
    }
}
