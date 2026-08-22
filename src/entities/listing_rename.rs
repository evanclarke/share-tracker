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
//! `effective_date` is bounded at both ends: after the listing's most recent
//! rename (`RenameError::OutOfOrder`) and no later than today
//! (`RenameError::FutureDated`). The second bound exists because the rename
//! is applied to `listings` as it is recorded — there is no pending state —
//! so a rename entered ahead of its announced date would rename the security
//! now while its own chain said the change had not happened yet
//! (SCENARIOS R-02). Record an announced change on the day it takes effect.
//!
//! The resulting identity must be free: a ticker another listing already
//! holds is refused by name (`RenameError::TickerCollision`) rather than by
//! the `UNIQUE(exchange_mic, ticker)` constraint's own text, which says which
//! columns clashed but not which listing is in the way (SCENARIOS R-03). An
//! exchange-less (Crypto) listing is checked against the bare ticker, the
//! `listings_crypto_ticker` partial index's basis, since NULL exchanges
//! compare distinct under the composite constraint. Both constraints remain
//! the invariant — the check is a better message ahead of them, not a
//! replacement. A plain `PUT /listings/:id` keeps answering an ordinary
//! duplicate with the shared classifier's constraint text: its write is one
//! row the request itself describes, so the constraint's wording is already
//! about the thing the caller sent (`docs/API.md`'s "A ticker an exchange
//! reissues to a different company cannot be recorded" quotes it verbatim).
//!
//! A rename may move the listing to another exchange, but not to one quoting
//! a **different currency** (`RenameError::ExchangeCurrencyMismatch`,
//! SCENARIOS R-01): `listings.currency` denominates every stored closing
//! price and is frozen once the listing has any history, so such a move would
//! leave a listing priced from a market quoting other money with no way to
//! correct it in place. That is the same rule `listing::db_upsert`'s currency
//! freeze states, applied at the one door that would otherwise bypass it —
//! and its remedy is the same one: a new listing in the other currency plus a
//! transfer of the parcels to it. A rename that leaves the exchange alone is
//! never refused for a mismatch it did not introduce; those are reported by
//! `reports::health`'s `exchange_currency_mismatches` instead.
//!
//! `DELETE /listings/:id/renames/:rename_id` undoes a rename: allowed only
//! for the *newest* rename of that listing (chain integrity — an
//! intermediate entry can't be removed out of order), restoring all four
//! fields the rename could change — `ticker`/`exchange_mic`/`name`/
//! `price_symbol` — from the row's `old_*` columns (migration 0040; see
//! [`db_undo`] for how a row recorded before those columns existed is
//! undone).

use crate::entities::listing::{self, Listing, SecurityType};
use crate::infra::db::write_tx;
use crate::infra::http::{ApiError, CrudEntity};
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
    /// What the rename overwrote (migration 0040), read from the listing's
    /// own row like `old_ticker`. NULL only on a rename recorded before 0040,
    /// which kept none of this — and since `listings.name` is NOT NULL, that
    /// is what makes `old_price_symbol` NULL readable as "the listing had no
    /// override" rather than "not recorded". See [`db_undo`].
    pub old_name: Option<String>,
    pub old_price_symbol: Option<String>,
    pub note: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
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
    /// `effective_date` is after today. A rename is applied to the listing
    /// the moment it is recorded — there is no pending state — so a
    /// future-dated one would rename the security while its own chain says
    /// the change hasn't happened yet (SCENARIOS R-02): every report would
    /// show the future ticker today, and the live quote, which resolves the
    /// symbol from the chain's last span, would fail until the date arrived.
    /// A rename dated *today* is fine; only a later date is refused.
    #[error("effective_date must not be after today ({today})")]
    FutureDated { today: NaiveDate },
    /// A Crypto listing's new ticker is not a recognised digital-token code
    /// (the same rule `listing::db_upsert` enforces).
    #[error("a Crypto listing's ticker must be a recognised digital-token code")]
    UnrecognisedDigitalToken,
    /// The rename would move a Crypto listing onto an exchange — the pairing
    /// `listing::db_upsert` refuses, met here because a rename may change
    /// `exchange_mic` (SCENARIOS L-09).
    #[error("a rename gave a Crypto listing an exchange")]
    CryptoWithExchange,
    /// The rename would move the listing to an exchange quoting a *different*
    /// currency from the listing's own (SCENARIOS R-01). `exchanges.currency`
    /// is the money the market quotes in; `listings.currency` denominates
    /// every stored closing price and, once the listing has any history, can
    /// never change again (`listing::UpsertError::CurrencyChangeWithHistory`).
    /// So a rename across that boundary leaves a listing whose prices are
    /// collected from a market quoting other money, with no way to correct it
    /// in place — the same rule the currency freeze states, applied at the one
    /// door that would otherwise bypass it. Only a *changed* exchange is
    /// tested: a rename that leaves the listing where it is never introduces
    /// the mismatch, so it is not refused for a pre-existing one (which
    /// `reports::health`'s `exchange_currency_mismatches` reports instead).
    #[error(
        "a rename to {mic} would leave a {listing_currency} listing on a {exchange_currency} market"
    )]
    ExchangeCurrencyMismatch {
        mic: String,
        listing_currency: String,
        exchange_currency: String,
    },
    /// The resulting identity — `(exchange_mic, ticker)`, or the bare
    /// ticker for an exchange-less (Crypto) listing — is already another
    /// listing's (SCENARIOS R-03). `UNIQUE(exchange_mic, ticker)` and the
    /// `listings_crypto_ticker` partial index remain the invariant; this is
    /// checked first only so the refusal can name the listing standing in
    /// the way, which the constraint's own message cannot. The listing's own
    /// row is excluded, so a rename that keeps the ticker and moves only the
    /// exchange never collides with itself.
    #[error("ticker {ticker} is already held by listing {holder_id}")]
    TickerCollision {
        ticker: String,
        /// The exchange the renamed listing would sit on; `None` for an
        /// exchange-less (Crypto) listing, where the bare ticker collides.
        mic: Option<String>,
        holder_id: i64,
        holder_name: String,
    },
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
            RenameError::FutureDated { today } => ApiError::unprocessable(format!(
                "effective_date must not be after today ({today}) — a rename is applied to the \
                 listing as soon as it is recorded, so record an announced change on the day it \
                 takes effect"
            )),
            RenameError::UnrecognisedDigitalToken => {
                ApiError::unprocessable(listing::UNRECOGNISED_DIGITAL_TOKEN)
            }
            RenameError::CryptoWithExchange => {
                ApiError::unprocessable(listing::CRYPTO_WITH_EXCHANGE)
            }
            RenameError::ExchangeCurrencyMismatch {
                mic,
                listing_currency,
                exchange_currency,
            } => ApiError::unprocessable(format!(
                "this rename would move a {listing_currency} listing to {mic}, which quotes in \
                 {exchange_currency} — and the listing's own currency cannot follow it: every \
                 stored closing price is denominated in it, so changing it would silently \
                 re-value the whole price history. Record a redenomination as a new listing in \
                 {exchange_currency} and transfer the parcels to it (a listing with no recorded \
                 trades, income, or prices can have its currency corrected with \
                 PUT /listings/:id first)"
            )),
            RenameError::TickerCollision {
                ticker,
                mic,
                holder_id,
                holder_name,
            } => {
                let held = match &mic {
                    Some(mic) => format!("on {mic}"),
                    None => "among the exchange-less (Crypto) listings, which are unique by \
                             ticker alone"
                        .to_string(),
                };
                ApiError::unprocessable(format!(
                    "listing {holder_id} ({holder_name}) already holds the ticker {ticker} \
                     {held} — a ticker is unique across the whole recorded history, not just a \
                     listing's current identity. If that listing is this same security recorded \
                     twice, transfer its parcels here and delete it; otherwise record this \
                     change under a ticker spelling that does not collide, and set price_symbol \
                     so price collection still asks the provider for the real code"
                ))
            }
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
                old_exchange_mic, new_exchange_mic, old_name, old_price_symbol, note \
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
    let mut tx = write_tx(pool).await?;

    // The column list is the entity's own (`CrudEntity::COLUMNS`), so a new
    // listing column can never be forgotten here — spelling it out by hand is
    // what let `amit_from` (migration 0024) break this read.
    let current: Option<Listing> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT {} FROM listings WHERE id = ?",
        Listing::COLUMNS
    )))
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

    // Bounded from above by today as well as from below by the chain: the
    // rename is applied to `listings` in this same transaction, so a
    // future-dated one takes effect immediately while its own span says it
    // has not (SCENARIOS R-02). Today itself is allowed — the change is
    // recorded on the day it happens.
    let today = crate::infra::date::today();
    if body.effective_date > today {
        return Err(RenameError::FutureDated { today });
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

    if current.security_type == SecurityType::Crypto && new_exchange_mic.is_some() {
        return Err(RenameError::CryptoWithExchange);
    }

    // An exchange quotes in one currency (`exchanges.currency`) and the
    // listing's own `currency` denominates every stored closing price — and
    // once the listing has history it can never change again, so a rename onto
    // a market quoting other money leaves a state nothing can correct in place
    // (SCENARIOS R-01). Only a *changed* exchange is tested: a rename that
    // leaves the listing where it is introduces no mismatch, and must not
    // start failing over one that was already there.
    if new_exchange_mic != current.exchange_mic
        && let Some(mic) = new_exchange_mic.as_deref()
    {
        let exchange_currency: Option<String> =
            sqlx::query_scalar("SELECT currency FROM exchanges WHERE mic = ?")
                .bind(mic)
                .fetch_optional(&mut *tx)
                .await?;
        // An unknown MIC is left to the foreign key on the UPDATE, which
        // already says so in its own words.
        if let Some(exchange_currency) = exchange_currency
            && exchange_currency != current.currency
        {
            return Err(RenameError::ExchangeCurrencyMismatch {
                mic: mic.to_string(),
                listing_currency: current.currency.clone(),
                exchange_currency,
            });
        }
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

    // The resulting identity must be free. `UNIQUE(exchange_mic, ticker)`
    // and, for an exchange-less listing, the `listings_crypto_ticker`
    // partial index (NULL exchanges compare distinct, so the composite
    // constraint cannot hold there) both still enforce this — the check
    // ahead of them exists only so the refusal names the listing holding
    // the ticker, which is the one fact needed to act on it, instead of
    // quoting the constraint (SCENARIOS R-03). The lookup differs by
    // whether the resulting listing has an exchange, matching the index
    // that would catch it. `id <> ?` is what keeps it from being a false
    // positive: a rename that keeps the listing's own ticker and moves only
    // its exchange finds its own row otherwise, and a listing never
    // collides with itself.
    let holder: Option<(i64, String)> = match new_exchange_mic.as_deref() {
        Some(mic) => {
            sqlx::query_as(
                "SELECT id, name FROM listings \
                 WHERE exchange_mic = ? AND ticker = ? AND id <> ?",
            )
            .bind(mic)
            .bind(&body.ticker)
            .bind(listing_id)
            .fetch_optional(&mut *tx)
            .await?
        }
        None => {
            sqlx::query_as(
                "SELECT id, name FROM listings \
                 WHERE exchange_mic IS NULL AND ticker = ? AND id <> ?",
            )
            .bind(&body.ticker)
            .bind(listing_id)
            .fetch_optional(&mut *tx)
            .await?
        }
    };
    if let Some((holder_id, holder_name)) = holder {
        return Err(RenameError::TickerCollision {
            ticker: body.ticker.clone(),
            mic: new_exchange_mic.clone(),
            holder_id,
            holder_name,
        });
    }

    // `old_name`/`old_price_symbol` are what the rename is about to overwrite
    // (0040), taken from the listing's current row exactly like `old_ticker`
    // — never from the request — so `db_undo` can put all four fields back.
    let result = sqlx::query(
        "INSERT INTO listing_renames \
         (listing_id, effective_date, old_ticker, new_ticker, old_exchange_mic, \
          new_exchange_mic, old_name, old_price_symbol, note) \
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(listing_id)
    .bind(body.effective_date)
    .bind(&current.ticker)
    .bind(&body.ticker)
    .bind(&current.exchange_mic)
    .bind(&new_exchange_mic)
    .bind(&current.name)
    .bind(&current.price_symbol)
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
        old_name: Some(current.name),
        old_price_symbol: current.price_symbol,
        note: body.note.clone(),
    })
}

/// Undo the newest rename for a listing: restore every field the rename
/// overwrote from the record's `old_*` columns and delete the record.
///
/// A rename changes four fields on the listing, and all four are put back —
/// including `price_symbol`, which is not cosmetic: `closing_price` uses it
/// verbatim for every date in the listing's *current* identity, so an undo
/// that left it behind went on collecting prices under a symbol that existed
/// only because of the undone rename (SCENARIOS R-04/R-08).
///
/// `old_name` doubles as the "this row recorded what it overwrote" marker:
/// `listings.name` is NOT NULL, so a rename recorded from migration 0040 on
/// always has one, and its absence means the row predates the columns. Such a
/// row is undone the way it always was — ticker and exchange only, `name` and
/// `price_symbol` left exactly as they stand — rather than being "restored"
/// to values it never recorded. When it *is* present, a NULL
/// `old_price_symbol` is the real prior value: the listing had no override,
/// and the undo clears the one the rename set.
pub async fn db_undo(pool: &SqlitePool, listing_id: i64, rename_id: i64) -> Result<(), UndoError> {
    let mut tx = write_tx(pool).await?;

    #[allow(clippy::type_complexity)]
    let target: Option<(
        NaiveDate,
        String,
        Option<String>,
        Option<String>,
        Option<String>,
    )> = sqlx::query_as(
        "SELECT effective_date, old_ticker, old_exchange_mic, old_name, old_price_symbol \
         FROM listing_renames WHERE id = ? AND listing_id = ?",
    )
    .bind(rename_id)
    .bind(listing_id)
    .fetch_optional(&mut *tx)
    .await?;
    let (effective_date, old_ticker, old_exchange_mic, old_name, old_price_symbol) =
        target.ok_or(UndoError::RenameNotFound)?;

    let newest: Option<NaiveDate> =
        sqlx::query_scalar("SELECT MAX(effective_date) FROM listing_renames WHERE listing_id = ?")
            .bind(listing_id)
            .fetch_one(&mut *tx)
            .await?;
    if newest != Some(effective_date) {
        return Err(UndoError::NotNewest);
    }

    match old_name {
        Some(old_name) => {
            sqlx::query(
                "UPDATE listings SET ticker = ?, exchange_mic = ?, name = ?, price_symbol = ? \
                 WHERE id = ?",
            )
            .bind(&old_ticker)
            .bind(&old_exchange_mic)
            .bind(&old_name)
            .bind(&old_price_symbol)
            .bind(listing_id)
            .execute(&mut *tx)
            .await?;
        }
        // Pre-0040: the row recorded neither, so neither is restored.
        None => {
            sqlx::query("UPDATE listings SET ticker = ?, exchange_mic = ? WHERE id = ?")
                .bind(&old_ticker)
                .bind(&old_exchange_mic)
                .bind(listing_id)
                .execute(&mut *tx)
                .await?;
        }
    }
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
    use crate::test_support::{self, ApiClient, test_pool};

    /// Client over this module's own routes.
    fn client(pool: &SqlitePool) -> ApiClient {
        ApiClient::over(router().with_state(pool.clone()))
    }

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

    /// A second **AUD**-quoting exchange, so a plain cross-exchange move is
    /// testable without crossing a currency boundary — the two seeded
    /// exchanges quote different money (XASX in AUD, XNYS in USD), which is
    /// exactly what `RenameError::ExchangeCurrencyMismatch` refuses.
    async fn insert_aud_exchange(pool: &SqlitePool, mic: &str) {
        crate::entities::exchange::db_upsert(
            pool,
            &crate::entities::exchange::Exchange {
                mic: mic.to_string(),
                name: format!("{mic} test market"),
                country: "Australia".to_string(),
                currency: "AUD".to_string(),
                timezone: "Australia/Sydney".to_string(),
                settlement_days: 2,
                close_time: "16:00".to_string(),
            },
        )
        .await
        .unwrap();
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
        // Both markets quote AUD: the move itself is what's under test, not
        // the currency rule the cross-currency tests below cover.
        insert_aud_exchange(&pool, "CXAX").await;
        test_support::listing(1).mic("XASX").insert(&pool).await;
        let mut moved = body("2024-06-01", "SAME");
        moved.ticker = "T1".to_string(); // ticker unchanged, exchange moves
        moved.exchange_mic = Some("CXAX".to_string());

        let created = db_rename(&pool, 1, &moved).await.unwrap();
        assert_eq!(created.old_exchange_mic.as_deref(), Some("XASX"));
        assert_eq!(created.new_exchange_mic.as_deref(), Some("CXAX"));
        assert_eq!(
            listing::db_get(&pool, 1)
                .await
                .unwrap()
                .unwrap()
                .exchange_mic
                .as_deref(),
            Some("CXAX")
        );
    }

    /// SCENARIOS R-01: a rename accepted any known exchange, so an AUD listing
    /// could be moved onto a USD market and keep `currency: AUD` — precisely
    /// the state the currency freeze on `PUT /listings/:id` exists to make
    /// unreachable, and which that same freeze then makes permanent. From the
    /// move on the listing is unpriceable: every fetch resolves the new
    /// exchange's symbol, and either the provider serves nothing (an errored
    /// row) or the candle's currency isn't the listing's, which the
    /// cross-check refuses (also an errored row).
    #[tokio::test]
    async fn db_rename_onto_an_exchange_quoting_another_currency_is_refused() {
        let pool = test_pool().await;
        test_support::listing(1)
            .ticker("CBA")
            .mic("XASX")
            .currency("AUD")
            .insert(&pool)
            .await;
        test_support::buy(1, 1).insert(&pool).await;

        let mut moved = body("2024-06-01", "CBA");
        moved.exchange_mic = Some("XNYS".to_string());
        let err = db_rename(&pool, 1, &moved).await.unwrap_err();
        assert!(
            matches!(
                &err,
                RenameError::ExchangeCurrencyMismatch { mic, listing_currency, exchange_currency }
                    if mic == "XNYS" && listing_currency == "AUD" && exchange_currency == "USD"
            ),
            "expected an exchange/currency mismatch, got: {err}"
        );
        // Refused before any write: the listing and the chain are untouched.
        let got = listing::db_get(&pool, 1).await.unwrap().unwrap();
        assert_eq!(got.exchange_mic.as_deref(), Some("XASX"));
        assert_eq!(got.currency, "AUD");
        assert!(db_list_for_listing(&pool, 1).await.unwrap().is_empty());
    }

    /// The 422 names both currencies and points at the same remedy the
    /// currency freeze does — the two refusals are one rule, so they read as
    /// one.
    #[tokio::test]
    async fn api_rename_across_a_currency_boundary_is_422_naming_both_currencies() {
        let pool = test_pool().await;
        test_support::listing(1)
            .ticker("CBA")
            .mic("XASX")
            .currency("AUD")
            .insert(&pool)
            .await;
        let resp = client(&pool)
            .post(
                "/listings/1/rename",
                &serde_json::json!({
                    "effective_date": "2024-06-01",
                    "ticker": "CBA",
                    "exchange_mic": "XNYS"
                }),
            )
            .await;
        let (status, body) = resp.status_and_body();
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
        assert!(
            body.contains("would move a AUD listing to XNYS, which quotes in USD"),
            "body was: {body}"
        );
        assert!(
            body.contains(
                "Record a redenomination as a new listing in USD and transfer the \
                           parcels to it"
            ),
            "body was: {body}"
        );
    }

    /// The boundary: the rule is about the exchange the rename *moves the
    /// listing to*, so a rename that leaves the exchange alone — omitting it,
    /// or naming the current one — is never refused, even on a listing that
    /// already mismatches its exchange (a state a plain `PUT` can create, and
    /// which `reports::health` reports instead). Otherwise fixing such a
    /// listing's ticker would be impossible.
    #[tokio::test]
    async fn db_rename_on_an_already_mismatched_listing_is_allowed_while_it_stays_put() {
        let pool = test_pool().await;
        // A USD listing sitting on the AUD exchange: entered this way by a
        // plain PUT, which checks no such thing.
        test_support::listing(1)
            .ticker("OLD")
            .mic("XASX")
            .currency("USD")
            .insert(&pool)
            .await;
        test_support::buy(1, 1).insert(&pool).await;

        // Exchange omitted.
        db_rename(&pool, 1, &body("2024-06-01", "NEW"))
            .await
            .unwrap();
        assert_eq!(
            listing::db_get(&pool, 1).await.unwrap().unwrap().ticker,
            "NEW"
        );

        // Exchange named, unchanged.
        let mut same = body("2024-06-02", "NEWER");
        same.exchange_mic = Some("XASX".to_string());
        db_rename(&pool, 1, &same).await.unwrap();
        let got = listing::db_get(&pool, 1).await.unwrap().unwrap();
        assert_eq!(got.ticker, "NEWER");
        assert_eq!(got.exchange_mic.as_deref(), Some("XASX"));
    }

    /// And the move that *resolves* a mismatch is allowed: the rule compares
    /// the listing's currency with the exchange it is moving to, not with the
    /// one it is leaving.
    #[tokio::test]
    async fn db_rename_onto_an_exchange_quoting_the_listings_currency_is_allowed() {
        let pool = test_pool().await;
        test_support::listing(1)
            .ticker("LAAC")
            .mic("XASX")
            .currency("USD")
            .insert(&pool)
            .await;
        let mut moved = body("2024-06-01", "LAR");
        moved.exchange_mic = Some("XNYS".to_string());

        db_rename(&pool, 1, &moved).await.unwrap();
        let got = listing::db_get(&pool, 1).await.unwrap().unwrap();
        assert_eq!(got.exchange_mic.as_deref(), Some("XNYS"));
        assert_eq!(got.currency, "USD");
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

    /// SCENARIOS R-02. A rename is applied to the listing as it is recorded,
    /// so one dated ahead of its announcement would rename the security now
    /// while its own chain said the change had not happened. Refused, and
    /// nothing is written — neither the chain row nor the listing.
    #[tokio::test]
    async fn db_rename_future_effective_date_is_rejected_and_writes_nothing() {
        let pool = test_pool().await;
        test_support::listing(1)
            .ticker("LAAC")
            .mic("XNYS")
            .insert(&pool)
            .await;

        let today = crate::infra::date::today();
        for ahead in [1, 30, 365] {
            let date = (today + chrono::Duration::days(ahead)).to_string();
            let err = db_rename(&pool, 1, &body(&date, "FUTURETICK"))
                .await
                .unwrap_err();
            assert!(
                matches!(err, RenameError::FutureDated { today: t } if t == today),
                "{ahead} days ahead: {err:?}"
            );
        }

        // Nothing was written: no chain row, and the listing keeps its
        // identity.
        assert_eq!(db_list_for_listing(&pool, 1).await.unwrap().len(), 0);
        let got = listing::db_get(&pool, 1).await.unwrap().unwrap();
        assert_eq!(got.ticker, "LAAC");
        assert_eq!(got.exchange_mic.as_deref(), Some("XNYS"));
    }

    /// The boundary: today itself is not the future — a change is recorded on
    /// the day it takes effect.
    #[tokio::test]
    async fn db_rename_dated_today_is_accepted() {
        let pool = test_pool().await;
        test_support::listing(1).ticker("LAAC").insert(&pool).await;

        let today = crate::infra::date::today();
        let created = db_rename(&pool, 1, &body(&today.to_string(), "LAR"))
            .await
            .unwrap();
        assert_eq!(created.effective_date, today);
        assert_eq!(
            listing::db_get(&pool, 1).await.unwrap().unwrap().ticker,
            "LAR"
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

    /// SCENARIOS L-09. A rename can move a listing between exchanges, so it
    /// meets the same exchange/security-type pairing — and answers it in the
    /// same words `listing::db_upsert` does, not with the table's CHECK.
    #[tokio::test]
    async fn db_rename_cannot_give_a_crypto_listing_an_exchange() {
        let pool = test_pool().await;
        test_support::listing(1)
            .crypto()
            .ticker("BTC")
            .insert(&pool)
            .await;
        let mut moving = body("2024-06-01", "ETH");
        moving.exchange_mic = Some("XASX".to_string());
        assert!(matches!(
            db_rename(&pool, 1, &moving).await.unwrap_err(),
            RenameError::CryptoWithExchange
        ));
        let err: ApiError = RenameError::CryptoWithExchange.into();
        assert!(!format!("{err:?}").contains("CHECK"));
        // The listing is untouched.
        let got = listing::db_get(&pool, 1).await.unwrap().unwrap();
        assert_eq!(got.ticker, "BTC");
        assert_eq!(got.exchange_mic, None);
    }

    /// SCENARIOS R-03, the regression this section exists for: a rename onto
    /// a ticker another listing on the same exchange already holds used to
    /// fall through to `UNIQUE(exchange_mic, ticker)`, and the shared
    /// classifier answered with the constraint's own text — which names the
    /// columns but not the listing standing in the way, the one fact needed
    /// to act on it.
    #[tokio::test]
    async fn db_rename_onto_a_ticker_another_listing_holds_names_that_listing() {
        let pool = test_pool().await;
        test_support::listing(1)
            .ticker("LAAC")
            .mic("XNYS")
            .insert(&pool)
            .await;
        test_support::listing(2)
            .ticker("LAR")
            .name("Lithium Argentina")
            .mic("XNYS")
            .insert(&pool)
            .await;

        let err = db_rename(&pool, 1, &body("2024-06-01", "LAR"))
            .await
            .unwrap_err();
        assert!(
            matches!(
                &err,
                RenameError::TickerCollision { ticker, mic, holder_id, holder_name }
                    if ticker == "LAR" && mic.as_deref() == Some("XNYS")
                        && *holder_id == 2 && holder_name == "Lithium Argentina"
            ),
            "expected a ticker collision, got: {err}"
        );
        // Refused before any write: neither the listing nor the chain moved.
        let got = listing::db_get(&pool, 1).await.unwrap().unwrap();
        assert_eq!(got.ticker, "LAAC");
        assert!(db_list_for_listing(&pool, 1).await.unwrap().is_empty());
        // And the listing that held the ticker is untouched too.
        assert_eq!(
            listing::db_get(&pool, 2).await.unwrap().unwrap().ticker,
            "LAR"
        );
    }

    /// The same collision on the other index: exchange-less (Crypto)
    /// listings have NULL `exchange_mic`, which `UNIQUE(exchange_mic,
    /// ticker)` treats as distinct, so uniqueness there is the
    /// `listings_crypto_ticker` partial index over the bare ticker. The
    /// check has to look the holder up the same way the index would.
    #[tokio::test]
    async fn db_rename_of_an_exchange_less_listing_onto_a_taken_ticker_names_the_holder() {
        let pool = test_pool().await;
        test_support::listing(1)
            .crypto()
            .ticker("BTC")
            .insert(&pool)
            .await;
        test_support::listing(2)
            .crypto()
            .ticker("ETH")
            .name("Ether")
            .insert(&pool)
            .await;

        let err = db_rename(&pool, 1, &body("2024-06-01", "ETH"))
            .await
            .unwrap_err();
        assert!(
            matches!(
                &err,
                RenameError::TickerCollision { ticker, mic, holder_id, holder_name }
                    if ticker == "ETH" && mic.is_none()
                        && *holder_id == 2 && holder_name == "Ether"
            ),
            "expected a ticker collision, got: {err}"
        );
        assert_eq!(
            listing::db_get(&pool, 1).await.unwrap().unwrap().ticker,
            "BTC"
        );
        assert!(db_list_for_listing(&pool, 1).await.unwrap().is_empty());
    }

    /// The boundary the check must not trip over: a listing renaming to its
    /// *own* current ticker while moving exchange. The row the lookup finds
    /// on the new exchange must be another listing's, never the renamed
    /// listing's own — and here there is no other listing at all, so an
    /// unqualified lookup would refuse a perfectly ordinary move.
    #[tokio::test]
    async fn db_rename_keeping_its_own_ticker_while_moving_exchange_does_not_self_collide() {
        let pool = test_pool().await;
        insert_aud_exchange(&pool, "CXAX").await;
        test_support::listing(1)
            .ticker("SAME")
            .mic("XASX")
            .insert(&pool)
            .await;

        let mut moved = body("2024-06-01", "SAME");
        moved.exchange_mic = Some("CXAX".to_string());
        let created = db_rename(&pool, 1, &moved).await.unwrap();
        assert_eq!(created.new_ticker, "SAME");
        assert_eq!(created.new_exchange_mic.as_deref(), Some("CXAX"));
        let got = listing::db_get(&pool, 1).await.unwrap().unwrap();
        assert_eq!(got.ticker, "SAME");
        assert_eq!(got.exchange_mic.as_deref(), Some("CXAX"));
    }

    /// The same ticker on a *different* exchange is not a collision at all:
    /// uniqueness is per `(exchange_mic, ticker)`, so a dual-listed code
    /// stays enterable and a rename onto it must not be refused.
    #[tokio::test]
    async fn db_rename_onto_a_ticker_only_another_exchange_holds_is_allowed() {
        let pool = test_pool().await;
        test_support::listing(1)
            .ticker("LAAC")
            .mic("XASX")
            .insert(&pool)
            .await;
        test_support::listing(2)
            .ticker("LAR")
            .mic("XNYS")
            .insert(&pool)
            .await;

        db_rename(&pool, 1, &body("2024-06-01", "LAR"))
            .await
            .unwrap();
        assert_eq!(
            listing::db_get(&pool, 1).await.unwrap().unwrap().ticker,
            "LAR"
        );
    }

    /// The UNIQUE constraint stays the invariant: the new check is a better
    /// message, not a replacement for it. Written straight past `db_rename`,
    /// a colliding `listings` update is still refused by the index.
    #[tokio::test]
    async fn the_unique_constraint_still_backstops_the_collision_check() {
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
        let err = sqlx::query("UPDATE listings SET ticker = 'LAR' WHERE id = 1")
            .execute(&pool)
            .await
            .unwrap_err();
        assert!(
            err.to_string().contains("UNIQUE"),
            "expected the UNIQUE constraint, got: {err}"
        );
    }

    /// At the HTTP surface the refusal is a 422 whose body names the listing
    /// holding the ticker — id and name — rather than the constraint.
    #[tokio::test]
    async fn api_rename_onto_a_taken_ticker_is_422_naming_the_listing_that_holds_it() {
        let pool = test_pool().await;
        test_support::listing(1)
            .ticker("LAAC")
            .mic("XNYS")
            .insert(&pool)
            .await;
        test_support::listing(2)
            .ticker("LAR")
            .name("Lithium Argentina")
            .mic("XNYS")
            .insert(&pool)
            .await;

        let resp = client(&pool)
            .post(
                "/listings/1/rename",
                &serde_json::json!({ "effective_date": "2024-06-01", "ticker": "LAR" }),
            )
            .await;
        let (status, body) = resp.status_and_body();
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
        assert!(
            body.contains("listing 2 (Lithium Argentina) already holds the ticker LAR on XNYS"),
            "body was: {body}"
        );
        // The constraint is no longer what answers.
        assert!(!body.contains("UNIQUE"), "body was: {body}");
    }

    /// The exchange-less refusal says why the bare ticker is what collided.
    #[tokio::test]
    async fn api_rename_of_an_exchange_less_listing_onto_a_taken_ticker_is_422() {
        let pool = test_pool().await;
        test_support::listing(1)
            .crypto()
            .ticker("BTC")
            .insert(&pool)
            .await;
        test_support::listing(2)
            .crypto()
            .ticker("ETH")
            .name("Ether")
            .insert(&pool)
            .await;

        let resp = client(&pool)
            .post(
                "/listings/1/rename",
                &serde_json::json!({ "effective_date": "2024-06-01", "ticker": "ETH" }),
            )
            .await;
        let (status, body) = resp.status_and_body();
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
        assert!(
            body.contains("listing 2 (Ether) already holds the ticker ETH")
                && body.contains("exchange-less (Crypto) listings, which are unique by ticker"),
            "body was: {body}"
        );
        assert!(!body.contains("UNIQUE"), "body was: {body}");
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

    /// SCENARIOS R-04/R-08, the regression this section exists for: the undo
    /// puts back *every* field the rename overwrote, not just the two the
    /// chain used to record. `price_symbol` is the one that mattered —
    /// `closing_price` uses it verbatim for the listing's current identity,
    /// so an undo that left it behind kept fetching prices under the undone
    /// rename's symbol.
    #[tokio::test]
    async fn db_undo_restores_the_name_and_price_symbol_the_rename_overwrote() {
        let pool = test_pool().await;
        test_support::listing(1)
            .ticker("OLD")
            .name("Old Co")
            .mic("XASX")
            .price_symbol("OLD.AX")
            .insert(&pool)
            .await;

        let mut renaming = body("2024-06-01", "NEWER");
        renaming.name = Some("Newer Co".to_string());
        renaming.price_symbol = Some("NEWER.AX".to_string());
        let created = db_rename(&pool, 1, &renaming).await.unwrap();
        assert_eq!(created.old_name.as_deref(), Some("Old Co"));
        assert_eq!(created.old_price_symbol.as_deref(), Some("OLD.AX"));

        let renamed = listing::db_get(&pool, 1).await.unwrap().unwrap();
        assert_eq!(renamed.name, "Newer Co");
        assert_eq!(renamed.price_symbol.as_deref(), Some("NEWER.AX"));

        db_undo(&pool, 1, created.id).await.unwrap();

        let restored = listing::db_get(&pool, 1).await.unwrap().unwrap();
        assert_eq!(restored.ticker, "OLD");
        assert_eq!(restored.name, "Old Co");
        assert_eq!(
            restored.price_symbol.as_deref(),
            Some("OLD.AX"),
            "the undone rename's symbol must not go on driving price collection"
        );
    }

    /// A rename that mentions neither `name` nor `price_symbol` still records
    /// what it left standing, so its undo writes the same values back — the
    /// listing is left exactly as it is either way.
    #[tokio::test]
    async fn db_undo_of_a_rename_that_set_neither_leaves_both_as_they_are() {
        let pool = test_pool().await;
        test_support::listing(1)
            .ticker("OLD")
            .name("Old Co")
            .price_symbol("OLD.AX")
            .insert(&pool)
            .await;

        let created = db_rename(&pool, 1, &body("2024-06-01", "NEWER"))
            .await
            .unwrap();
        db_undo(&pool, 1, created.id).await.unwrap();

        let restored = listing::db_get(&pool, 1).await.unwrap().unwrap();
        assert_eq!(restored.name, "Old Co");
        assert_eq!(restored.price_symbol.as_deref(), Some("OLD.AX"));
    }

    /// The distinguishability point behind migration 0040: `price_symbol` is
    /// nullable, so "the listing had none" is a real prior value the undo has
    /// to be able to restore — and does, because `old_name` (NOT NULL on
    /// `listings`) is what says the row recorded anything at all.
    #[tokio::test]
    async fn db_undo_clears_a_price_symbol_the_rename_introduced() {
        let pool = test_pool().await;
        test_support::listing(1).ticker("OLD").insert(&pool).await;
        assert_eq!(
            listing::db_get(&pool, 1)
                .await
                .unwrap()
                .unwrap()
                .price_symbol,
            None
        );

        let mut renaming = body("2024-06-01", "NEWER");
        renaming.price_symbol = Some("NEWER.AX".to_string());
        let created = db_rename(&pool, 1, &renaming).await.unwrap();
        assert_eq!(created.old_price_symbol, None);
        assert_eq!(created.old_name.as_deref(), Some("Test 1"));

        db_undo(&pool, 1, created.id).await.unwrap();

        assert_eq!(
            listing::db_get(&pool, 1)
                .await
                .unwrap()
                .unwrap()
                .price_symbol,
            None,
            "an override the rename introduced is cleared, not left behind"
        );
    }

    /// A rename recorded before migration 0040 kept neither value, and NULL
    /// there means "not recorded" — never "restore to NULL". Its undo behaves
    /// exactly as it did before the columns existed: ticker and exchange back,
    /// name and symbol left alone. (The migration must not retroactively
    /// change what undoing an existing row does.)
    #[tokio::test]
    async fn db_undo_of_a_rename_recorded_before_0040_touches_neither_field() {
        let pool = test_pool().await;
        test_support::listing(1)
            .ticker("NEWER")
            .name("Newer Co")
            .mic("XASX")
            .price_symbol("NEWER.AX")
            .insert(&pool)
            .await;
        // A pre-0040 row: old_name and old_price_symbol were never recorded.
        sqlx::query(
            "INSERT INTO listing_renames \
             (id, listing_id, effective_date, old_ticker, new_ticker, \
              old_exchange_mic, new_exchange_mic) \
             VALUES (7, 1, '2024-06-01', 'OLD', 'NEWER', 'XASX', 'XASX')",
        )
        .execute(&pool)
        .await
        .unwrap();

        db_undo(&pool, 1, 7).await.unwrap();

        let restored = listing::db_get(&pool, 1).await.unwrap().unwrap();
        assert_eq!(restored.ticker, "OLD");
        assert_eq!(restored.name, "Newer Co");
        assert_eq!(restored.price_symbol.as_deref(), Some("NEWER.AX"));
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

        let resp = client(&pool)
            .post_raw(
                "/listings/1/rename",
                r#"{"effective_date":"2024-06-01","ticker":"LAR"}"#,
            )
            .await;
        assert_eq!(resp.status, StatusCode::CREATED);
        let created: ListingRename = resp.json();
        assert_eq!(created.new_ticker, "LAR");
        assert_eq!(
            listing::db_get(&pool, 1).await.unwrap().unwrap().ticker,
            "LAR"
        );
    }

    #[tokio::test]
    async fn api_rename_missing_listing_returns_404() {
        let pool = test_pool().await;
        let resp = client(&pool)
            .post_raw(
                "/listings/99/rename",
                r#"{"effective_date":"2024-06-01","ticker":"LAR"}"#,
            )
            .await;
        assert_eq!(resp.status, StatusCode::NOT_FOUND);
    }

    /// SCENARIOS R-02, at the HTTP surface: the refusal is a 422 whose body
    /// names the rule and today's date (it is shown verbatim in the web UI).
    #[tokio::test]
    async fn api_rename_dated_in_the_future_returns_422_naming_the_rule() {
        let pool = test_pool().await;
        test_support::listing(1).ticker("MSFT").insert(&pool).await;

        let today = crate::infra::date::today();
        let future = today + chrono::Duration::days(365);
        let resp = client(&pool)
            .post(
                "/listings/1/rename",
                &serde_json::json!({ "effective_date": future, "ticker": "FUTURETICK" }),
            )
            .await;
        let (status, body) = resp.status_and_body();
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
        assert!(
            body.contains("effective_date must not be after today")
                && body.contains(&today.to_string()),
            "{body}"
        );
        assert_eq!(
            listing::db_get(&pool, 1).await.unwrap().unwrap().ticker,
            "MSFT"
        );
    }

    #[tokio::test]
    async fn api_list_renames_returns_newest_first() {
        let pool = test_pool().await;
        test_support::listing(1).ticker("A").insert(&pool).await;
        db_rename(&pool, 1, &body("2024-01-01", "B")).await.unwrap();
        db_rename(&pool, 1, &body("2024-06-01", "C")).await.unwrap();

        let resp = client(&pool).get("/listings/1/renames").await;
        assert_eq!(resp.status, StatusCode::OK);
        let chain: Vec<ListingRename> = resp.json();
        assert_eq!(chain.len(), 2);
        assert_eq!(chain[0].new_ticker, "C");
        assert_eq!(chain[1].new_ticker, "B");
    }

    /// The chain says what each rename *replaced* as well as what it set —
    /// the two 0040 columns are on the wire, and a rename over a listing with
    /// no override reports that as `null` rather than omitting it.
    #[tokio::test]
    async fn api_list_renames_carries_what_each_rename_overwrote() {
        let pool = test_pool().await;
        test_support::listing(1)
            .ticker("OLD")
            .name("Old Co")
            .price_symbol("OLD.AX")
            .insert(&pool)
            .await;
        let mut renaming = body("2024-06-01", "NEWER");
        renaming.name = Some("Newer Co".to_string());
        renaming.price_symbol = Some("NEWER.AX".to_string());
        db_rename(&pool, 1, &renaming).await.unwrap();
        // A second rename, this one over a listing whose override it clears
        // nothing of — its own old_price_symbol is the first one's new value.
        db_rename(&pool, 1, &body("2024-07-01", "NEWEST"))
            .await
            .unwrap();

        let chain: Vec<ListingRename> = client(&pool).get("/listings/1/renames").await.json();
        assert_eq!(chain[0].old_name.as_deref(), Some("Newer Co"));
        assert_eq!(chain[0].old_price_symbol.as_deref(), Some("NEWER.AX"));
        assert_eq!(chain[1].old_name.as_deref(), Some("Old Co"));
        assert_eq!(chain[1].old_price_symbol.as_deref(), Some("OLD.AX"));

        let resp = client(&pool).get("/listings/1/renames").await;
        let raw = resp.text();
        assert!(raw.contains("\"old_name\":\"Old Co\""), "{raw}");
        assert!(raw.contains("\"old_price_symbol\":\"OLD.AX\""), "{raw}");
    }

    #[tokio::test]
    async fn api_undo_round_trip_and_rejections() {
        let pool = test_pool().await;
        test_support::listing(1).ticker("LAAC").insert(&pool).await;
        let created = db_rename(&pool, 1, &body("2024-06-01", "LAR"))
            .await
            .unwrap();

        let app = client(&pool);
        let resp = app
            .delete(format!("/listings/1/renames/{}", created.id))
            .await;
        assert_eq!(resp.status, StatusCode::NO_CONTENT);
        assert_eq!(
            listing::db_get(&pool, 1).await.unwrap().unwrap().ticker,
            "LAAC"
        );

        // Undoing again (already gone) is a 404.
        let resp = app
            .delete(format!("/listings/1/renames/{}", created.id))
            .await;
        assert_eq!(resp.status, StatusCode::NOT_FOUND);
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
