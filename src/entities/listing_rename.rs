//! Ticker/exchange-code renames as an explicit, dated, audited event
//! (`listing_renames`, migration 0018) — see the module doc on
//! `entities::listing` and `docs/API.md`'s "Ticker or name changes" section
//! for the full rationale (LAAC -> LAR being the prompting case).
//!
//! `POST /listings/:id/rename` is the only path that can change `ticker` or
//! `exchange_mic` once a listing has any recorded trades, income, or closing
//! prices (`listing::db_upsert` refuses a bare `PUT` doing that — see
//! `UpsertError::IdentityChangeRequiresRename`). It records one
//! `listing_renames` row — with `old_ticker`/`old_exchange_mic` always taken
//! from the listing's current row, never trusted from the request body, so
//! the chain can't be falsified — and updates the listing, atomically.
//! `exchange_mic` and `name` are optional in the request: omitted, they keep
//! their current value (a rename never needs to *clear* a non-Crypto
//! listing's exchange — that would violate the exchange/security_type CHECK
//! pairing, which a rename does not change). `price_symbol` is likewise
//! optional and, when omitted, is left exactly as it was — it is not part of
//! the tracked identity chain (an override that matched the old ticker
//! rarely matches the new one, so it is not carried over automatically
//! either; set it explicitly via `PUT /listings/:id` or the rename body).
//!
//! `DELETE /listings/:id/renames/:rename_id` undoes a rename: allowed only
//! for the *newest* rename of that listing (chain integrity — an
//! intermediate entry can't be removed out of order), restoring
//! `ticker`/`exchange_mic` from the row's `old_*` columns.

use crate::entities::listing::{Listing, SecurityType};
use crate::infra::http::ApiError;
use axum::{
    Json, Router,
    extract::{Path, State},
    http::StatusCode,
    routing::{get, post},
};
use chrono::NaiveDate;
use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct ListingRename {
    pub id: i64,
    pub listing_id: i64,
    pub effective_date: NaiveDate,
    pub old_ticker: String,
    pub new_ticker: String,
    pub old_exchange_mic: Option<String>,
    pub new_exchange_mic: Option<String>,
    pub note: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct RenameBody {
    pub effective_date: NaiveDate,
    pub ticker: String,
    /// Omitted keeps the listing's current exchange (see the module doc).
    #[serde(default)]
    pub exchange_mic: Option<String>,
    /// Omitted keeps the listing's current name.
    #[serde(default)]
    pub name: Option<String>,
    /// Omitted leaves `price_symbol` exactly as it was.
    #[serde(default)]
    pub price_symbol: Option<String>,
    #[serde(default)]
    pub note: Option<String>,
}

#[derive(thiserror::Error, Debug)]
pub enum RenameError {
    #[error("no listing with that id")]
    ListingNotFound,
    /// The request changes neither `ticker` nor `exchange_mic` (also
    /// CHECK-enforced at the table level; caught here first for a clearer
    /// message).
    #[error("the rename changes neither the ticker nor the exchange")]
    NoOp,
    /// `effective_date` is not after this listing's most recent rename.
    #[error("effective_date must be after this listing's most recent rename ({latest})")]
    OutOfOrder { latest: NaiveDate },
    /// A Crypto listing's new ticker is not a recognised digital-token code
    /// (the same rule `listing::db_upsert` enforces).
    #[error("a Crypto listing's ticker must be a recognised digital-token code")]
    UnrecognisedDigitalToken,
    #[error("listing rename write failed: {0}")]
    Db(#[from] sqlx::Error),
}

#[derive(thiserror::Error, Debug)]
pub enum UndoError {
    #[error("no rename with that id")]
    RenameNotFound,
    /// The targeted rename is not the newest one for its listing — undo must
    /// unwind the chain last-in-first-out.
    #[error("only the newest rename for a listing can be undone")]
    NotNewest,
    #[error("rename undo failed: {0}")]
    Db(#[from] sqlx::Error),
}

impl From<RenameError> for ApiError {
    fn from(e: RenameError) -> Self {
        match e {
            RenameError::ListingNotFound => ApiError::not_found("no listing with that id"),
            RenameError::NoOp => {
                ApiError::unprocessable("the rename changes neither the ticker nor the exchange")
            }
            RenameError::OutOfOrder { latest } => ApiError::unprocessable(format!(
                "effective_date must be after this listing's most recent rename ({latest})"
            )),
            RenameError::UnrecognisedDigitalToken => ApiError::unprocessable(
                "a Crypto listing's ticker must be a recognised digital-token code",
            ),
            RenameError::Db(err) => err.into(),
        }
    }
}

impl From<UndoError> for ApiError {
    fn from(e: UndoError) -> Self {
        match e {
            UndoError::RenameNotFound => ApiError::not_found("no rename with that id"),
            UndoError::NotNewest => ApiError::unprocessable(
                "only the newest rename for a listing can be undone — undo later renames first",
            ),
            UndoError::Db(err) => err.into(),
        }
    }
}

pub fn router() -> Router<SqlitePool> {
    Router::new()
        .route("/listings/{id}/rename", post(rename))
        .route("/listings/{id}/renames", get(list_for_listing))
        .route(
            "/listings/{id}/renames/{rename_id}",
            axum::routing::delete(undo),
        )
}

pub async fn db_list_for_listing(
    pool: &SqlitePool,
    listing_id: i64,
) -> Result<Vec<ListingRename>, sqlx::Error> {
    sqlx::query_as(
        "SELECT id, listing_id, effective_date, old_ticker, new_ticker, \
                old_exchange_mic, new_exchange_mic, note \
         FROM listing_renames WHERE listing_id = ? ORDER BY effective_date DESC, id DESC",
    )
    .bind(listing_id)
    .fetch_all(pool)
    .await
}

/// Record a rename and update the listing, atomically.
pub async fn db_rename(
    pool: &SqlitePool,
    listing_id: i64,
    body: &RenameBody,
) -> Result<ListingRename, RenameError> {
    let mut tx = pool.begin().await?;

    let current: Option<Listing> = sqlx::query_as(
        "SELECT id, exchange_mic, ticker, name, isin, security_type, currency, amit, \
                preference, price_symbol \
         FROM listings WHERE id = ?",
    )
    .bind(listing_id)
    .fetch_optional(&mut *tx)
    .await?;
    let current = current.ok_or(RenameError::ListingNotFound)?;

    let new_exchange_mic = body.exchange_mic.clone().or(current.exchange_mic.clone());
    let new_name = body.name.clone().unwrap_or(current.name.clone());
    let new_price_symbol = body.price_symbol.clone().or(current.price_symbol.clone());

    if body.ticker == current.ticker && new_exchange_mic == current.exchange_mic {
        return Err(RenameError::NoOp);
    }

    let latest: Option<NaiveDate> =
        sqlx::query_scalar("SELECT MAX(effective_date) FROM listing_renames WHERE listing_id = ?")
            .bind(listing_id)
            .fetch_one(&mut *tx)
            .await?;
    if let Some(latest) = latest
        && body.effective_date <= latest
    {
        return Err(RenameError::OutOfOrder { latest });
    }

    if current.security_type == SecurityType::Crypto {
        let recognised: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM currencies \
             WHERE kind = 'DigitalToken' AND (code = ?1 OR short_name = ?1))",
        )
        .bind(&body.ticker)
        .fetch_one(&mut *tx)
        .await?;
        if !recognised {
            return Err(RenameError::UnrecognisedDigitalToken);
        }
    }

    let result = sqlx::query(
        "INSERT INTO listing_renames \
         (listing_id, effective_date, old_ticker, new_ticker, old_exchange_mic, \
          new_exchange_mic, note) \
         VALUES (?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(listing_id)
    .bind(body.effective_date)
    .bind(&current.ticker)
    .bind(&body.ticker)
    .bind(&current.exchange_mic)
    .bind(&new_exchange_mic)
    .bind(&body.note)
    .execute(&mut *tx)
    .await?;
    let rename_id = result.last_insert_rowid();

    sqlx::query(
        "UPDATE listings SET ticker = ?, exchange_mic = ?, name = ?, price_symbol = ? \
         WHERE id = ?",
    )
    .bind(&body.ticker)
    .bind(&new_exchange_mic)
    .bind(&new_name)
    .bind(&new_price_symbol)
    .bind(listing_id)
    .execute(&mut *tx)
    .await?;

    tx.commit().await?;

    Ok(ListingRename {
        id: rename_id,
        listing_id,
        effective_date: body.effective_date,
        old_ticker: current.ticker,
        new_ticker: body.ticker.clone(),
        old_exchange_mic: current.exchange_mic,
        new_exchange_mic,
        note: body.note.clone(),
    })
}

/// Undo the newest rename for a listing: restore `ticker`/`exchange_mic`
/// from the rename's `old_*` columns and delete the record.
pub async fn db_undo(pool: &SqlitePool, listing_id: i64, rename_id: i64) -> Result<(), UndoError> {
    let mut tx = pool.begin().await?;

    let target: Option<(NaiveDate, String, Option<String>)> = sqlx::query_as(
        "SELECT effective_date, old_ticker, old_exchange_mic FROM listing_renames \
         WHERE id = ? AND listing_id = ?",
    )
    .bind(rename_id)
    .bind(listing_id)
    .fetch_optional(&mut *tx)
    .await?;
    let (effective_date, old_ticker, old_exchange_mic) = target.ok_or(UndoError::RenameNotFound)?;

    let newest: Option<NaiveDate> =
        sqlx::query_scalar("SELECT MAX(effective_date) FROM listing_renames WHERE listing_id = ?")
            .bind(listing_id)
            .fetch_one(&mut *tx)
            .await?;
    if newest != Some(effective_date) {
        return Err(UndoError::NotNewest);
    }

    sqlx::query("UPDATE listings SET ticker = ?, exchange_mic = ? WHERE id = ?")
        .bind(&old_ticker)
        .bind(&old_exchange_mic)
        .bind(listing_id)
        .execute(&mut *tx)
        .await?;
    sqlx::query("DELETE FROM listing_renames WHERE id = ?")
        .bind(rename_id)
        .execute(&mut *tx)
        .await?;

    tx.commit().await?;
    Ok(())
}

async fn rename(
    State(pool): State<SqlitePool>,
    Path(listing_id): Path<i64>,
    Json(body): Json<RenameBody>,
) -> Result<(StatusCode, Json<ListingRename>), ApiError> {
    let created = db_rename(&pool, listing_id, &body).await?;
    Ok((StatusCode::CREATED, Json(created)))
}

async fn list_for_listing(
    State(pool): State<SqlitePool>,
    Path(listing_id): Path<i64>,
) -> Result<Json<Vec<ListingRename>>, ApiError> {
    Ok(Json(db_list_for_listing(&pool, listing_id).await?))
}

async fn undo(
    State(pool): State<SqlitePool>,
    Path((listing_id, rename_id)): Path<(i64, i64)>,
) -> Result<StatusCode, ApiError> {
    db_undo(&pool, listing_id, rename_id).await?;
    Ok(StatusCode::NO_CONTENT)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::entities::listing;
    use crate::test_support::{self, test_pool};
    use axum::{body::Body, http::Request};
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    fn body(effective_date: &str, ticker: &str) -> RenameBody {
        RenameBody {
            effective_date: effective_date.parse().unwrap(),
            ticker: ticker.to_string(),
            exchange_mic: None,
            name: None,
            price_symbol: None,
            note: None,
        }
    }

    #[tokio::test]
    async fn db_rename_updates_listing_and_records_the_chain() {
        let pool = test_pool().await;
        test_support::listing(1)
            .ticker("LAAC")
            .mic("XNYS")
            .insert(&pool)
            .await;
        test_support::buy(1, 1).insert(&pool).await;

        let created = db_rename(&pool, 1, &body("2024-06-01", "LAR"))
            .await
            .unwrap();
        assert_eq!(created.old_ticker, "LAAC");
        assert_eq!(created.new_ticker, "LAR");
        assert_eq!(created.old_exchange_mic.as_deref(), Some("XNYS"));
        assert_eq!(created.new_exchange_mic.as_deref(), Some("XNYS"));

        let got = listing::db_get(&pool, 1).await.unwrap().unwrap();
        assert_eq!(got.ticker, "LAR");
        assert_eq!(got.exchange_mic.as_deref(), Some("XNYS"));

        let chain = db_list_for_listing(&pool, 1).await.unwrap();
        assert_eq!(chain.len(), 1);
        assert_eq!(chain[0].new_ticker, "LAR");
    }

    /// A rename can move exchanges too, and its own `old_exchange_mic` is
    /// always read from the listing's row, never trusted from the request.
    #[tokio::test]
    async fn db_rename_can_move_exchange_and_records_it_from_the_current_row() {
        let pool = test_pool().await;
        test_support::listing(1).mic("XASX").insert(&pool).await;
        let mut moved = body("2024-06-01", "SAME");
        moved.ticker = "T1".to_string(); // ticker unchanged, exchange moves
        moved.exchange_mic = Some("XNYS".to_string());

        let created = db_rename(&pool, 1, &moved).await.unwrap();
        assert_eq!(created.old_exchange_mic.as_deref(), Some("XASX"));
        assert_eq!(created.new_exchange_mic.as_deref(), Some("XNYS"));
        assert_eq!(
            listing::db_get(&pool, 1)
                .await
                .unwrap()
                .unwrap()
                .exchange_mic
                .as_deref(),
            Some("XNYS")
        );
    }

    #[tokio::test]
    async fn db_rename_omitted_exchange_and_name_keep_current_values() {
        let pool = test_pool().await;
        test_support::listing(1)
            .ticker("LAAC")
            .name("Lithium Americas (Argentina) Corp.")
            .mic("XNYS")
            .insert(&pool)
            .await;
        db_rename(&pool, 1, &body("2024-06-01", "LAR"))
            .await
            .unwrap();
        let got = listing::db_get(&pool, 1).await.unwrap().unwrap();
        assert_eq!(got.name, "Lithium Americas (Argentina) Corp.");
        assert_eq!(got.exchange_mic.as_deref(), Some("XNYS"));
    }

    /// `price_symbol` is untouched by a rename that doesn't mention it, and
    /// is not carried over "for free" — it's independent of the chain.
    #[tokio::test]
    async fn db_rename_leaves_price_symbol_untouched_when_omitted() {
        let pool = test_pool().await;
        test_support::listing(1)
            .ticker("LAAC")
            .price_symbol("LAAC.OLD")
            .insert(&pool)
            .await;
        db_rename(&pool, 1, &body("2024-06-01", "LAR"))
            .await
            .unwrap();
        assert_eq!(
            listing::db_get(&pool, 1)
                .await
                .unwrap()
                .unwrap()
                .price_symbol,
            Some("LAAC.OLD".to_string())
        );

        // Setting it explicitly in the rename body does update it.
        let mut with_symbol = body("2024-07-01", "LAR2");
        with_symbol.price_symbol = Some("LAR.NEW".to_string());
        db_rename(&pool, 1, &with_symbol).await.unwrap();
        assert_eq!(
            listing::db_get(&pool, 1)
                .await
                .unwrap()
                .unwrap()
                .price_symbol,
            Some("LAR.NEW".to_string())
        );
    }

    #[tokio::test]
    async fn db_rename_missing_listing_is_not_found() {
        let pool = test_pool().await;
        assert!(matches!(
            db_rename(&pool, 99, &body("2024-06-01", "LAR"))
                .await
                .unwrap_err(),
            RenameError::ListingNotFound
        ));
    }

    #[tokio::test]
    async fn db_rename_no_op_is_rejected() {
        let pool = test_pool().await;
        test_support::listing(1).ticker("LAR").insert(&pool).await;
        assert!(matches!(
            db_rename(&pool, 1, &body("2024-06-01", "LAR"))
                .await
                .unwrap_err(),
            RenameError::NoOp
        ));
    }

    #[tokio::test]
    async fn db_rename_out_of_order_effective_date_is_rejected() {
        let pool = test_pool().await;
        test_support::listing(1).ticker("LAAC").insert(&pool).await;
        db_rename(&pool, 1, &body("2024-06-01", "LAR"))
            .await
            .unwrap();

        // Same date or earlier than the latest rename: rejected.
        for date in ["2024-06-01", "2024-01-01"] {
            let err = db_rename(&pool, 1, &body(date, "LARX")).await.unwrap_err();
            assert!(
                matches!(err, RenameError::OutOfOrder { latest } if latest == "2024-06-01".parse().unwrap()),
                "date {date}: {err:?}"
            );
        }
        // After it succeeds.
        db_rename(&pool, 1, &body("2024-07-01", "LARX"))
            .await
            .unwrap();
        assert_eq!(
            listing::db_get(&pool, 1).await.unwrap().unwrap().ticker,
            "LARX"
        );
    }

    #[tokio::test]
    async fn db_rename_crypto_requires_recognised_digital_token() {
        let pool = test_pool().await;
        test_support::listing(1)
            .crypto()
            .ticker("BTC")
            .insert(&pool)
            .await;
        assert!(matches!(
            db_rename(&pool, 1, &body("2024-06-01", "NOTATOKEN"))
                .await
                .unwrap_err(),
            RenameError::UnrecognisedDigitalToken
        ));
        // A recognised token (ETH is seeded) succeeds.
        db_rename(&pool, 1, &body("2024-06-01", "ETH"))
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn db_rename_ticker_collision_surfaces_as_422_via_shared_db_error_mapping() {
        let pool = test_pool().await;
        test_support::listing(1)
            .ticker("LAAC")
            .mic("XNYS")
            .insert(&pool)
            .await;
        test_support::listing(2)
            .ticker("LAR")
            .mic("XNYS")
            .insert(&pool)
            .await;
        let err = db_rename(&pool, 1, &body("2024-06-01", "LAR"))
            .await
            .unwrap_err();
        assert!(matches!(err, RenameError::Db(_)));
    }

    #[tokio::test]
    async fn db_undo_restores_ticker_and_exchange_and_deletes_the_record() {
        let pool = test_pool().await;
        test_support::listing(1)
            .ticker("LAAC")
            .mic("XNYS")
            .insert(&pool)
            .await;
        let created = db_rename(&pool, 1, &body("2024-06-01", "LAR"))
            .await
            .unwrap();

        db_undo(&pool, 1, created.id).await.unwrap();

        assert_eq!(
            listing::db_get(&pool, 1).await.unwrap().unwrap().ticker,
            "LAAC"
        );
        assert_eq!(db_list_for_listing(&pool, 1).await.unwrap().len(), 0);

        // A redo (the same rename again) now works, since nothing blocks it.
        db_rename(&pool, 1, &body("2024-06-01", "LAR"))
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn db_undo_refuses_a_non_newest_rename() {
        let pool = test_pool().await;
        test_support::listing(1).ticker("A").insert(&pool).await;
        let first = db_rename(&pool, 1, &body("2024-01-01", "B")).await.unwrap();
        db_rename(&pool, 1, &body("2024-06-01", "C")).await.unwrap();

        let err = db_undo(&pool, 1, first.id).await.unwrap_err();
        assert!(matches!(err, UndoError::NotNewest));
        // The listing and the chain are unchanged.
        assert_eq!(
            listing::db_get(&pool, 1).await.unwrap().unwrap().ticker,
            "C"
        );
        assert_eq!(db_list_for_listing(&pool, 1).await.unwrap().len(), 2);
    }

    #[tokio::test]
    async fn db_undo_missing_rename_is_not_found() {
        let pool = test_pool().await;
        test_support::listing(1).insert(&pool).await;
        assert!(matches!(
            db_undo(&pool, 1, 99).await.unwrap_err(),
            UndoError::RenameNotFound
        ));
    }

    // ---- API-level ----

    #[tokio::test]
    async fn api_rename_returns_201_and_updates_the_listing() {
        let pool = test_pool().await;
        test_support::listing(1).ticker("LAAC").insert(&pool).await;

        let resp = router()
            .with_state(pool.clone())
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/listings/1/rename")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        r#"{"effective_date":"2024-06-01","ticker":"LAR"}"#,
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::CREATED);
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let created: ListingRename = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(created.new_ticker, "LAR");
        assert_eq!(
            listing::db_get(&pool, 1).await.unwrap().unwrap().ticker,
            "LAR"
        );
    }

    #[tokio::test]
    async fn api_rename_missing_listing_returns_404() {
        let pool = test_pool().await;
        let resp = router()
            .with_state(pool)
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/listings/99/rename")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        r#"{"effective_date":"2024-06-01","ticker":"LAR"}"#,
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn api_list_renames_returns_newest_first() {
        let pool = test_pool().await;
        test_support::listing(1).ticker("A").insert(&pool).await;
        db_rename(&pool, 1, &body("2024-01-01", "B")).await.unwrap();
        db_rename(&pool, 1, &body("2024-06-01", "C")).await.unwrap();

        let resp = router()
            .with_state(pool)
            .oneshot(
                Request::builder()
                    .uri("/listings/1/renames")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let chain: Vec<ListingRename> = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(chain.len(), 2);
        assert_eq!(chain[0].new_ticker, "C");
        assert_eq!(chain[1].new_ticker, "B");
    }

    #[tokio::test]
    async fn api_undo_round_trip_and_rejections() {
        let pool = test_pool().await;
        test_support::listing(1).ticker("LAAC").insert(&pool).await;
        let created = db_rename(&pool, 1, &body("2024-06-01", "LAR"))
            .await
            .unwrap();

        let del = |uri: String| {
            Request::builder()
                .method("DELETE")
                .uri(uri)
                .body(Body::empty())
                .unwrap()
        };
        let app = router().with_state(pool.clone());
        let resp = app
            .clone()
            .oneshot(del(format!("/listings/1/renames/{}", created.id)))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NO_CONTENT);
        assert_eq!(
            listing::db_get(&pool, 1).await.unwrap().unwrap().ticker,
            "LAAC"
        );

        // Undoing again (already gone) is a 404.
        let resp = app
            .oneshot(del(format!("/listings/1/renames/{}", created.id)))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    /// End to end: a rename leaves parcels, cost base, and the discount
    /// clock untouched — the whole point of routing renames through this
    /// action instead of orphaning history. Mirrors the identity-continuity
    /// tests in `reports::open_parcels` / `reports::realised_gains`, but
    /// exercised at the rename action itself rather than a bare `PUT`.
    #[tokio::test]
    async fn rename_action_preserves_the_trades_row_and_its_listing_id() {
        let pool = test_pool().await;
        test_support::listing(1)
            .ticker("LAAC")
            .mic("XNYS")
            .insert(&pool)
            .await;
        test_support::buy(1, 1)
            .qty(rust_decimal::Decimal::from(100))
            .insert(&pool)
            .await;

        db_rename(&pool, 1, &body("2024-06-01", "LAR"))
            .await
            .unwrap();

        let trade = crate::entities::trade::db_get(&pool, 1)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(trade.listing_id, 1);
        assert_eq!(trade.quantity, rust_decimal::Decimal::from(100));
    }
}
