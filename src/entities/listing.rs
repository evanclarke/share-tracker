use crate::infra::http::{self, ApiError, CrudEntity};
use axum::{
    Json, Router,
    extract::{Path, State},
    http::StatusCode,
    routing::get,
};
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
    pub preference: bool,
    #[serde(default)]
    pub price_symbol: Option<String>,
}

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
    /// Constraint violations (Crypto with an exchange / non-Crypto without
    /// one, duplicate ticker, unknown exchange or currency) surface here via
    /// the table's CHECKs and FKs.
    #[error("listing write failed: {0}")]
    Db(#[from] sqlx::Error),
}

impl From<UpsertError> for ApiError {
    fn from(e: UpsertError) -> Self {
        match e {
            UpsertError::UnrecognisedDigitalToken => ApiError::unprocessable(
                "a Crypto listing's ticker must be a recognised digital-token code",
            ),
            UpsertError::IdentityChangeRequiresRename => ApiError::unprocessable(
                "use POST /listings/:id/rename to record a ticker or exchange change \
                 on a listing with recorded trades, income, or prices",
            ),
            UpsertError::Db(err) => err.into(),
        }
    }
}

impl CrudEntity for Listing {
    type Key = i64;
    const TABLE: &'static str = "listings";
    const COLUMNS: &'static str = "id, exchange_mic, ticker, name, isin, security_type, currency, amit, preference, \
         price_symbol";
    const ORDER_BY: &'static str = "exchange_mic, ticker";
    const NOUN: &'static str = "listing";
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

    // A Crypto listing's ticker must be a recognised digital-token code
    // (validated in the write transaction; the mic/Crypto pairing and ticker
    // uniqueness are CHECK/index-enforced by the table itself).
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
        "INSERT INTO listings (id, exchange_mic, ticker, name, isin, security_type, currency, amit, preference, price_symbol) \
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?) \
         ON CONFLICT(id) DO UPDATE SET \
             exchange_mic  = excluded.exchange_mic, \
             ticker        = excluded.ticker, \
             name          = excluded.name, \
             isin          = excluded.isin, \
             security_type = excluded.security_type, \
             currency      = excluded.currency, \
             amit          = excluded.amit, \
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
        preference: body.preference,
        price_symbol: body.price_symbol,
    };
    db_upsert(&pool, &listing).await?;
    Ok(StatusCode::NO_CONTENT)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::test_pool;
    use axum::{body::Body, http::Request};
    use http_body_util::BodyExt;
    use tower::ServiceExt;

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

    #[tokio::test]
    async fn db_check_pairs_exchange_with_security_type() {
        let pool = test_pool().await;
        // A Crypto listing with an exchange is rejected by the CHECK...
        let mut bad = crypto();
        bad.exchange_mic = Some("XASX".to_string());
        let UpsertError::Db(err) = db_upsert(&pool, &bad).await.unwrap_err() else {
            panic!("expected a DB error");
        };
        assert!(
            err.to_string().contains("CHECK"),
            "expected CHECK error, got: {err}"
        );
        // ...and so is a non-Crypto listing without one.
        let mut bare = xtest();
        bare.exchange_mic = None;
        let UpsertError::Db(err) = db_upsert(&pool, &bare).await.unwrap_err() else {
            panic!("expected a DB error");
        };
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
        let resp = router()
            .with_state(pool)
            .oneshot(
                Request::builder()
                    .uri("/listings")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let listings: Vec<Listing> = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(listings.len(), 1);
        assert_eq!(listings[0].ticker, "VAS");
    }

    #[tokio::test]
    async fn api_get_existing_returns_listing() {
        let pool = test_pool().await;
        db_upsert(&pool, &xtest()).await.unwrap();
        let resp = router()
            .with_state(pool)
            .oneshot(
                Request::builder()
                    .uri("/listings/1")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let l: Listing = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(l.ticker, "VAS");
    }

    #[tokio::test]
    async fn api_get_missing_returns_404() {
        let pool = test_pool().await;
        let resp = router()
            .with_state(pool)
            .oneshot(
                Request::builder()
                    .uri("/listings/999")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
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
        let resp = router()
            .with_state(pool.clone())
            .oneshot(
                Request::builder()
                    .method("PUT")
                    .uri("/listings/1")
                    .header("content-type", "application/json")
                    .body(Body::from(body.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NO_CONTENT);
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
        let resp = router()
            .with_state(pool)
            .oneshot(
                Request::builder()
                    .method("PUT")
                    .uri("/listings/1")
                    .header("content-type", "application/json")
                    .body(Body::from(body.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY);
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
        let resp = router()
            .with_state(pool.clone())
            .oneshot(
                Request::builder()
                    .method("PUT")
                    .uri("/listings/2")
                    .header("content-type", "application/json")
                    .body(Body::from(body.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NO_CONTENT);
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
            let resp = router()
                .with_state(pool.clone())
                .oneshot(
                    Request::builder()
                        .method("PUT")
                        .uri("/listings/2")
                        .header("content-type", "application/json")
                        .body(Body::from(body.to_string()))
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(
                resp.status(),
                StatusCode::UNPROCESSABLE_ENTITY,
                "body: {body}"
            );
        }

        // The unrecognised-digital-token rejection says why, not a bare "HTTP 422".
        let resp = router()
            .with_state(pool.clone())
            .oneshot(
                Request::builder()
                    .method("PUT")
                    .uri("/listings/2")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        serde_json::json!({
                            "ticker": "DOGE", "name": "Dogecoin", "isin": null,
                            "security_type": "Crypto", "currency": "AUD", "amit": false
                        })
                        .to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        let bytes = http_body_util::BodyExt::collect(resp.into_body())
            .await
            .unwrap()
            .to_bytes();
        let detail = String::from_utf8(bytes.to_vec()).unwrap();
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
        let resp = router()
            .with_state(pool.clone())
            .oneshot(
                Request::builder()
                    .method("PUT")
                    .uri("/listings/1")
                    .header("content-type", "application/json")
                    .body(Body::from(body.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NO_CONTENT);
        let got = db_get(&pool, 1).await.unwrap().unwrap();
        assert_eq!(got.name, "Renamed ETF");
        assert!(!got.amit);
    }

    #[tokio::test]
    async fn api_delete_existing_returns_no_content() {
        let pool = test_pool().await;
        db_upsert(&pool, &xtest()).await.unwrap();
        let resp = router()
            .with_state(pool)
            .oneshot(
                Request::builder()
                    .method("DELETE")
                    .uri("/listings/1")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NO_CONTENT);
    }

    #[tokio::test]
    async fn api_delete_missing_returns_404() {
        let pool = test_pool().await;
        let resp = router()
            .with_state(pool)
            .oneshot(
                Request::builder()
                    .method("DELETE")
                    .uri("/listings/999")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }
}
