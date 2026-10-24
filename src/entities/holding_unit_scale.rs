//! Unit precision: how many decimal places a quantity held in one place may
//! carry, and the per-(account, listing) override table behind it
//! (`holding_unit_scales`, migration 0053).
//!
//! The unit grid belongs to the custody, not the exchange: the same NYSE
//! listing is whole units at a CHESS broker and 3 dp in a share plan's DRP, a
//! custodial broker sets its own cap, and a crypto wallet has no grid at all.
//! So the scale is `holding_accounts.unit_scale` (NULL = unconstrained), and a
//! row here overrides it for one listing in one account — its own `unit_scale`
//! may be NULL too, making that one holding unconstrained.
//!
//! [`db_too_fine`] is the write-time check: a quantity with more decimal places
//! than its holding's scale is refused `422` naming the scale. It runs on every
//! write whose quantity the **taxpayer states** — a parcel entered through
//! `PUT`/`POST /trades`, a Sell and each of its allocations (also when a
//! transfer or a buy-back participation writes them, and a transfer's
//! network-fee disposal), a transfer-in parcel (against the *destination*
//! account), an ESS vest, an inheritance, a rights exercise and a DRP
//! reinvestment.
//!
//! It deliberately does **not** run on a quantity the system *computes* from a
//! ratio: a scrip-for-scrip exchange's or demerger's replacement parcels, and
//! the closing Sell (and its allocations) of a scrip exchange, demerger or
//! worthless-shares recognise, which consume whatever each parcel still holds.
//! Those quantities are the corporate action's, not a typing slip; when a ratio
//! does not divide a holding they land off any register's grid (200 units
//! through a 1-for-3 consolidation), and refusing them would make the action
//! unrecordable. Rounding them onto the grid is the job of the section's later
//! items (a fraction rule per action, and per-holding rounding stored per
//! parcel). Until then such a holding is reported as it is computed, and
//! selling the off-grid remainder needs the holding's scale raised — an
//! override with a NULL scale is the narrow way to do it.
//!
//! Changing a scale is checked the other way round: an account's or
//! override's scale may not be set below the finest quantity that holding
//! already carries ([`db_finest_held`]), so lowering it can never leave a stored
//! row its own re-save would refuse. (An account's check skips the listings an
//! override governs.)
//!
//! Neither table is audited and neither stales a snapshot: a scale changes no
//! stored figure and no report reads it — it only decides which writes are
//! accepted (see the migration's header).

use crate::infra::db::{self, write_tx};
use crate::infra::decimal::parse_dec;
use crate::infra::extract::Json;
use crate::infra::http::{self, ApiError, CrudEntity, NoFilter, UpsertResponse, Upserted};
use crate::infra::id::{HoldingAccountId, ListingId};
use crate::infra::respond::Created;
use axum::extract::{Path, State};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use sqlx::{Row, SqlitePool};
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

/// The largest `unit_scale` either table accepts: `rust_decimal` carries at
/// most 28 decimal places, so a finer grid could hold no quantity anyway.
pub const MAX_UNIT_SCALE: u32 = 28;

/// The decimal places `quantity` actually carries, trailing zeros dropped
/// (`100.000` → 0, `1.50` → 1) — the figure compared against a unit scale.
pub fn places(quantity: Decimal) -> u32 {
    quantity.normalize().scale()
}

/// A per-(holding account, listing) unit-scale override.
#[derive(utoipa::ToSchema, Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct HoldingUnitScale {
    pub id: i64,
    pub holding_account_id: HoldingAccountId,
    pub listing_id: ListingId,
    /// Decimal places a quantity of this listing held in this account may
    /// carry; `null` = unconstrained (for this holding only).
    pub unit_scale: Option<u32>,
}

#[derive(utoipa::ToSchema, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HoldingUnitScaleBody {
    /// The account whose scale this overrides.
    pub holding_account_id: HoldingAccountId,
    /// The one listing the override applies to within that account.
    pub listing_id: ListingId,
    /// Decimal places (0–28) a quantity of this holding may carry, e.g. `3`
    /// for a share plan's DRP units in an otherwise whole-unit account;
    /// omitted or `null` leaves this holding unconstrained.
    #[serde(default)]
    pub unit_scale: Option<u32>,
}

impl CrudEntity for HoldingUnitScale {
    type Key = i64;
    type Filter = NoFilter;
    const TABLE: &'static str = "holding_unit_scales";
    const COLUMNS: &'static str = "id, holding_account_id, listing_id, unit_scale";
    const NOUN: &'static str = "unit-scale override";
}

http::crud_list_route!(
    list,
    HoldingUnitScale,
    NoFilter,
    "/holding_unit_scales",
    "List every per-listing unit-scale override."
);
http::crud_get_route!(
    get_one,
    HoldingUnitScale,
    i64,
    "/holding_unit_scales/{id}",
    "Fetch one unit-scale override, or 404."
);
http::crud_delete_route!(
    delete_one,
    HoldingUnitScale,
    i64,
    "/holding_unit_scales/{id}",
    "Delete the unit-scale override; the holding falls back to its account's scale."
);

pub fn router() -> OpenApiRouter<SqlitePool> {
    OpenApiRouter::new()
        .routes(routes!(list, create))
        .routes(routes!(get_one, upsert, delete_one))
}

/// Where a holding's unit scale was read from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScaleSource {
    /// `holding_accounts.unit_scale`.
    Account,
    /// A `holding_unit_scales` row for the pair.
    Override,
}

/// A quantity with more decimal places than its holding's unit scale allows.
#[derive(thiserror::Error, Debug, Clone, PartialEq)]
#[error("quantity {quantity} is finer than its holding's unit scale of {scale}")]
pub struct TooFine {
    pub quantity: Decimal,
    pub scale: u32,
    pub from: ScaleSource,
    /// The holding account's name, for the message.
    pub account: String,
    /// The listing's ticker, for the message.
    pub ticker: String,
}

/// How a scale reads in prose: `0` is "whole units".
fn grid(scale: u32) -> String {
    match scale {
        0 => "whole units".to_owned(),
        1 => "1 decimal place".to_owned(),
        n => format!("{n} decimal places"),
    }
}

impl TooFine {
    /// The user-facing `422` body: the figure, the scale it broke and where
    /// that scale is set, so a typo is told apart from a holding whose scale
    /// is simply wrong.
    pub fn detail(&self) -> String {
        let TooFine {
            quantity,
            scale,
            from,
            account,
            ticker,
        } = self;
        let grid = grid(*scale);
        match from {
            ScaleSource::Account => format!(
                "the quantity {quantity} is finer than holding account {account} holds units \
                 to: its unit_scale is {scale} ({grid}). Check the figure; if the custodian \
                 really holds {ticker} to more places, raise the account's unit_scale or add a \
                 unit-scale override for {ticker} in it"
            ),
            ScaleSource::Override => format!(
                "the quantity {quantity} is finer than holding account {account} holds {ticker} \
                 to: its unit-scale override is {scale} ({grid}). Check the figure; if the \
                 custodian really holds it to more places, raise the override"
            ),
        }
    }
}

/// `Some` when `quantity` carries more decimal places than the unit scale of
/// `listing_id` held in `account_id` — the override's when one exists for the
/// pair, else the account's. `None` when it fits, when the holding is
/// unconstrained (a NULL scale), or when the account or listing does not exist
/// (the write's own foreign key answers that).
pub(crate) async fn db_too_fine(
    conn: &mut sqlx::SqliteConnection,
    account_id: HoldingAccountId,
    listing_id: ListingId,
    quantity: Decimal,
) -> Result<Option<TooFine>, sqlx::Error> {
    let row = sqlx::query(
        "SELECT ha.name AS account, l.ticker AS ticker, o.id IS NOT NULL AS overridden, \
                CASE WHEN o.id IS NULL THEN ha.unit_scale ELSE o.unit_scale END AS scale \
         FROM holding_accounts ha \
         CROSS JOIN listings l \
         LEFT JOIN holding_unit_scales o \
                ON o.holding_account_id = ha.id AND o.listing_id = l.id \
         WHERE ha.id = ? AND l.id = ?",
    )
    .bind(account_id)
    .bind(listing_id)
    .fetch_optional(&mut *conn)
    .await?;
    let Some(row) = row else {
        return Ok(None);
    };
    let Some(scale) = row.try_get::<Option<u32>, _>("scale")? else {
        return Ok(None);
    };
    if places(quantity) <= scale {
        return Ok(None);
    }
    Ok(Some(TooFine {
        quantity,
        scale,
        from: if row.try_get("overridden")? {
            ScaleSource::Override
        } else {
            ScaleSource::Account
        },
        account: row.try_get("account")?,
        ticker: row.try_get("ticker")?,
    }))
}

/// The unit scale `listing_id` is held to in `account_id` — the override's
/// when one exists for the pair, else the account's — or `None` where the
/// holding is unconstrained (or the account does not exist). What a
/// ratio-driven action's per-holding rounding rounds onto
/// (`corporate_action`'s stored counts, the scrip exchange and demerger).
pub(crate) async fn db_unit_scale(
    conn: &mut sqlx::SqliteConnection,
    account_id: HoldingAccountId,
    listing_id: ListingId,
) -> Result<Option<u32>, sqlx::Error> {
    Ok(sqlx::query_scalar::<_, Option<u32>>(
        "SELECT CASE WHEN o.id IS NULL THEN ha.unit_scale ELSE o.unit_scale END \
         FROM holding_accounts ha \
         LEFT JOIN holding_unit_scales o \
                ON o.holding_account_id = ha.id AND o.listing_id = ? \
         WHERE ha.id = ?",
    )
    .bind(listing_id)
    .bind(account_id)
    .fetch_optional(&mut *conn)
    .await?
    .flatten())
}

/// Which stored quantities a scale change must still fit.
#[derive(Debug, Clone, Copy)]
pub(crate) enum Holding {
    /// Every listing in the account that no override governs.
    Account(HoldingAccountId),
    /// One listing in one account.
    Pair(HoldingAccountId, ListingId),
}

/// The stored quantity in `holding` with the most decimal places — every trade
/// quantity there and every allocation drawn on one of its parcels — or `None`
/// when it holds nothing. A scale below its [`places`] would leave that row one
/// its own re-save refuses.
pub(crate) async fn db_finest_held(
    conn: &mut sqlx::SqliteConnection,
    holding: Holding,
) -> Result<Option<Decimal>, sqlx::Error> {
    let (account_id, listing_id) = match holding {
        Holding::Account(a) => (a, None),
        Holding::Pair(a, l) => (a, Some(l)),
    };
    // `?2 IS NULL` selects the account-wide reading, which skips the listings
    // an override governs; a listing id selects that one pair.
    let quantities: Vec<String> = sqlx::query_scalar(
        "SELECT t.quantity FROM trades t \
         WHERE t.holding_account_id = ?1 \
           AND (t.listing_id = ?2 OR (?2 IS NULL AND NOT EXISTS ( \
                SELECT 1 FROM holding_unit_scales o \
                WHERE o.holding_account_id = ?1 AND o.listing_id = t.listing_id))) \
         UNION ALL \
         SELECT pa.quantity_allocated FROM parcel_allocations pa \
         JOIN trades t ON t.id = pa.purchase_trade_id \
         WHERE t.holding_account_id = ?1 \
           AND (t.listing_id = ?2 OR (?2 IS NULL AND NOT EXISTS ( \
                SELECT 1 FROM holding_unit_scales o \
                WHERE o.holding_account_id = ?1 AND o.listing_id = t.listing_id)))",
    )
    .bind(account_id)
    .bind(listing_id)
    .fetch_all(&mut *conn)
    .await?;
    let mut finest: Option<Decimal> = None;
    for q in quantities {
        let q = parse_dec("quantity", &q)?;
        if finest.is_none_or(|f| places(q) > places(f)) {
            finest = Some(q);
        }
    }
    Ok(finest)
}

/// Why a unit scale may not be set to the value asked for — shared by the
/// account's and the override's write paths.
#[derive(thiserror::Error, Debug)]
pub enum ScaleError {
    /// Beyond [`MAX_UNIT_SCALE`].
    #[error("unit_scale {0} is beyond the 28 places a quantity can carry")]
    OutOfRange(u32),
    /// The holding already carries `finest`, which has more places than
    /// `scale`.
    #[error("unit_scale {scale} is below the stored quantity {finest}")]
    BelowHeld { scale: u32, finest: Decimal },
    #[error("unit scale check failed: {0}")]
    Db(#[from] sqlx::Error),
}

impl ScaleError {
    /// The user-facing `422` body for the two refusals (`None` for `Db`).
    pub fn detail(&self) -> Option<String> {
        match self {
            ScaleError::OutOfRange(scale) => Some(format!(
                "unit_scale must be between 0 and {MAX_UNIT_SCALE} (or null for unconstrained), \
                 not {scale}"
            )),
            ScaleError::BelowHeld { scale, finest } => Some(format!(
                "unit_scale {scale} ({}) is below a quantity this holding already carries: \
                 {finest} has {} decimal places. Correct that record first, or choose a scale \
                 of at least {}",
                grid(*scale),
                places(*finest),
                places(*finest)
            )),
            ScaleError::Db(_) => None,
        }
    }
}

/// Refuse a scale out of range or below what `holding` already carries
/// (`None` for a row being created, which holds nothing yet). A `None` scale
/// (unconstrained) always passes.
pub(crate) async fn check_new_scale(
    conn: &mut sqlx::SqliteConnection,
    holding: Option<Holding>,
    scale: Option<u32>,
) -> Result<(), ScaleError> {
    let Some(scale) = scale else {
        return Ok(());
    };
    if scale > MAX_UNIT_SCALE {
        return Err(ScaleError::OutOfRange(scale));
    }
    let Some(holding) = holding else {
        return Ok(());
    };
    if let Some(finest) = db_finest_held(conn, holding).await?
        && places(finest) > scale
    {
        return Err(ScaleError::BelowHeld { scale, finest });
    }
    Ok(())
}

/// Why an override write failed.
#[derive(thiserror::Error, Debug)]
pub enum UpsertError {
    #[error("unit-scale override write failed: {0}")]
    Db(#[from] sqlx::Error),
    #[error("the override's unit scale was refused: {0}")]
    Scale(#[source] ScaleError),
    /// A create committed but the row could not be read back. Not reachable
    /// in practice; mapped to `500` rather than an `unwrap`.
    #[error("the unit-scale override was created but could not be read back")]
    VanishedAfterCreate,
}

impl From<ScaleError> for UpsertError {
    fn from(e: ScaleError) -> Self {
        match e {
            ScaleError::Db(err) => UpsertError::Db(err),
            other @ (ScaleError::OutOfRange(_) | ScaleError::BelowHeld { .. }) => {
                UpsertError::Scale(other)
            }
        }
    }
}

impl From<UpsertError> for ApiError {
    fn from(e: UpsertError) -> Self {
        match e {
            // A second override for the same pair violates the UNIQUE
            // constraint, and an unknown account or listing its foreign key:
            // both 422 through the shared classifier.
            UpsertError::Db(err) => err.into(),
            UpsertError::Scale(scale) => match scale.detail() {
                Some(detail) => ApiError::unprocessable(detail),
                None => ApiError::internal(scale),
            },
            err @ UpsertError::VanishedAfterCreate => ApiError::internal(err),
        }
    }
}

/// Write an override, allocating its id when `id` is `None`. The new scale is
/// checked against what the pair already holds on the write's own transaction.
async fn write(
    pool: &SqlitePool,
    id: Option<i64>,
    body: &HoldingUnitScaleBody,
) -> Result<Upserted<HoldingUnitScale>, UpsertError> {
    let mut tx = write_tx(pool).await?;
    let outcome =
        http::Upsert::of(http::crud_exists_opt::<HoldingUnitScale, _>(&mut *tx, id).await?);
    // An override is new for its *row*, not for the pair: the pair may hold
    // quantities already, so it is always checked.
    check_new_scale(
        &mut tx,
        Some(Holding::Pair(body.holding_account_id, body.listing_id)),
        body.unit_scale,
    )
    .await?;
    let result = db::insert_with_optional_id(
        "INSERT INTO holding_unit_scales (id, holding_account_id, listing_id, unit_scale) \
         VALUES (?, ?, ?, ?) \
         ON CONFLICT(id) DO UPDATE SET \
             holding_account_id = excluded.holding_account_id, \
             listing_id         = excluded.listing_id, \
             unit_scale         = excluded.unit_scale",
        id,
    )
    .bind(body.holding_account_id)
    .bind(body.listing_id)
    .bind(body.unit_scale)
    .execute(&mut *tx)
    .await?;
    let assigned = id.unwrap_or_else(|| result.last_insert_rowid());
    let upserted = http::crud_upserted::<HoldingUnitScale, _>(&mut *tx, outcome, assigned).await?;
    tx.commit().await?;
    Ok(upserted)
}

/// `PUT /holding_unit_scales/:id` — create or replace the override at `id`.
pub async fn db_upsert(
    pool: &SqlitePool,
    id: i64,
    body: &HoldingUnitScaleBody,
) -> Result<Upserted<HoldingUnitScale>, UpsertError> {
    write(pool, Some(id), body).await
}

/// `POST /holding_unit_scales` — create an override with a database-assigned id.
pub async fn db_create(
    pool: &SqlitePool,
    body: &HoldingUnitScaleBody,
) -> Result<HoldingUnitScale, UpsertError> {
    write(pool, None, body)
        .await?
        .created
        .ok_or(UpsertError::VanishedAfterCreate)
}

#[utoipa::path(
    put,
    path = "/holding_unit_scales/{id}",
    summary = "Create or replace the unit-scale override at this id."
)]
async fn upsert(
    State(pool): State<SqlitePool>,
    Path(id): Path<i64>,
    Json(body): Json<HoldingUnitScaleBody>,
) -> Result<UpsertResponse<HoldingUnitScale>, ApiError> {
    let Upserted { outcome, created } = db_upsert(&pool, id, &body).await?;
    http::upsert_response::<HoldingUnitScale>(outcome, created)
}

/// `POST /holding_unit_scales` — create the override; the created row is
/// returned so its id can be acted on at once.
#[utoipa::path(
    post,
    path = "/holding_unit_scales",
    summary = "Create a unit-scale override; the created row is returned."
)]
async fn create(
    State(pool): State<SqlitePool>,
    Json(body): Json<HoldingUnitScaleBody>,
) -> Result<Created<HoldingUnitScale>, ApiError> {
    Ok(Created(db_create(&pool, &body).await?))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{ApiClient, buy, dec, listing, test_pool};
    use axum::http::StatusCode;
    use serde_json::json;

    /// A plain AUD Buy body for `POST /trades`.
    fn buy_json(listing_id: i64, quantity: &str) -> serde_json::Value {
        json!({
            "trade_type": "Buy", "date": "2024-03-04", "listing_id": listing_id,
            "average_price": "10", "quantity": quantity, "currency": "AUD",
            "brokerage": "0", "brokerage_currency": "AUD", "fx_rate": "1",
        })
    }

    /// A Sell of listing 1 on 2024-06-03 drawing `allocations` (parcel id,
    /// units) for `quantity` units.
    fn sell_json(quantity: &str, allocations: &[(i64, &str)]) -> serde_json::Value {
        let allocations: Vec<_> = allocations
            .iter()
            .map(|(id, q)| json!({ "purchase_trade_id": id, "quantity_allocated": q }))
            .collect();
        json!({
            "date": "2024-06-03", "listing_id": 1, "average_price": "15",
            "quantity": quantity, "currency": "AUD", "brokerage": "0",
            "gst_on_brokerage": "0", "brokerage_currency": "AUD", "fx_rate": "1",
            "allocations": allocations,
        })
    }

    async fn set_account_scale(app: &ApiClient, scale: Option<u32>) {
        app.put_ok(
            "/holding_accounts/1",
            &json!({ "name": "Default", "unit_scale": scale }),
        )
        .await;
    }

    #[test]
    fn places_ignores_trailing_zeros() {
        assert_eq!(places(dec("100.000")), 0);
        assert_eq!(places(dec("1.50")), 1);
        assert_eq!(places(dec("0.00012345")), 8);
        assert_eq!(places(dec("7")), 0);
    }

    /// A Buy finer than its account's scale is refused `422`, and the body
    /// names the figure, the scale and the account; a figure padded with
    /// zeros is still whole.
    #[tokio::test]
    async fn a_quantity_finer_than_the_accounts_scale_is_refused_naming_it() {
        let pool = test_pool().await;
        listing(1).insert(&pool).await;
        let app = ApiClient::full(&pool);
        set_account_scale(&app, Some(0)).await;

        let resp = app.post("/trades", &buy_json(1, "10.5")).await;
        let (status, body) = resp.status_and_body();
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
        assert!(body.contains("the quantity 10.5"), "{body}");
        assert!(body.contains("holding account Default"), "{body}");
        assert!(body.contains("unit_scale is 0 (whole units)"), "{body}");
        let stored: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM trades")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(stored, 0, "a refused write stores nothing");

        app.post("/trades", &buy_json(1, "10.000"))
            .await
            .expect_status(StatusCode::CREATED);
    }

    /// NULL is unconstrained: the seeded default account (and any account
    /// created without a scale) takes a quantity to any precision.
    #[tokio::test]
    async fn an_unconstrained_account_takes_any_precision() {
        let pool = test_pool().await;
        listing(1).insert(&pool).await;
        let app = ApiClient::full(&pool);
        let account: serde_json::Value = app.get_json("/holding_accounts/1").await;
        assert_eq!(account["unit_scale"], serde_json::Value::Null);

        app.post("/trades", &buy_json(1, "0.1234567891234"))
            .await
            .expect_status(StatusCode::CREATED);
    }

    /// The override for the pair takes precedence over the account's scale in
    /// either direction — finer, or unconstrained — and only for its listing.
    #[tokio::test]
    async fn an_override_takes_precedence_over_the_account() {
        let pool = test_pool().await;
        listing(1).insert(&pool).await;
        listing(2).insert(&pool).await;
        let app = ApiClient::full(&pool);
        set_account_scale(&app, Some(0)).await;
        let created: HoldingUnitScale = app
            .post(
                "/holding_unit_scales",
                &json!({ "holding_account_id": 1, "listing_id": 1, "unit_scale": 3 }),
            )
            .await
            .expect_status(StatusCode::CREATED)
            .json();
        assert_eq!(created.unit_scale, Some(3));

        app.post("/trades", &buy_json(1, "10.125"))
            .await
            .expect_status(StatusCode::CREATED);
        let resp = app.post("/trades", &buy_json(1, "10.1255")).await;
        let (status, body) = resp.status_and_body();
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
        assert!(
            body.contains("unit-scale override is 3 (3 decimal places)"),
            "{body}"
        );
        // The other listing is still held to the account's whole units.
        app.post("/trades", &buy_json(2, "1.5"))
            .await
            .expect_status(StatusCode::UNPROCESSABLE_ENTITY);

        // An override with no scale makes just that holding unconstrained.
        app.put(
            &format!("/holding_unit_scales/{}", created.id),
            &json!({ "holding_account_id": 1, "listing_id": 1, "unit_scale": null }),
        )
        .await
        .expect_status(StatusCode::NO_CONTENT);
        app.post("/trades", &buy_json(1, "10.12345678"))
            .await
            .expect_status(StatusCode::CREATED);

        // One override per pair.
        app.post(
            "/holding_unit_scales",
            &json!({ "holding_account_id": 1, "listing_id": 1, "unit_scale": 2 }),
        )
        .await
        .expect_status(StatusCode::UNPROCESSABLE_ENTITY);
    }

    /// A Sell's quantity, and each allocation's, is held to the scale.
    #[tokio::test]
    async fn a_sell_and_each_allocation_are_held_to_the_scale() {
        let pool = test_pool().await;
        listing(1).insert(&pool).await;
        buy(1, 1).qty(dec("100")).insert(&pool).await;
        buy(2, 1).qty(dec("100")).insert(&pool).await;
        let app = ApiClient::full(&pool);
        set_account_scale(&app, Some(0)).await;

        let resp = app.post("/sells", &sell_json("10.5", &[(1, "10.5")])).await;
        let (status, body) = resp.status_and_body();
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
        assert!(body.contains("the quantity 10.5"), "{body}");

        // Whole in total, but split across parcels in halves.
        let resp = app
            .post("/sells", &sell_json("10", &[(1, "9.5"), (2, "0.5")]))
            .await;
        let (status, body) = resp.status_and_body();
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
        assert!(body.contains("the quantity 9.5"), "{body}");

        app.post("/sells", &sell_json("10", &[(1, "9"), (2, "1")]))
            .await
            .expect_status(StatusCode::CREATED);
    }

    /// A scale may not be set below a quantity the holding already carries,
    /// nor beyond 28; an account's check skips the listings an override
    /// governs.
    #[tokio::test]
    async fn lowering_a_scale_below_a_quantity_held_is_refused() {
        let pool = test_pool().await;
        listing(1).insert(&pool).await;
        listing(2).insert(&pool).await;
        buy(1, 1).qty(dec("12.50")).insert(&pool).await;
        buy(2, 2).qty(dec("3")).insert(&pool).await;
        let app = ApiClient::full(&pool);

        let resp = app
            .put(
                "/holding_accounts/1",
                &json!({ "name": "Default", "unit_scale": 0 }),
            )
            .await;
        let (status, body) = resp.status_and_body();
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
        assert!(body.contains("12.5"), "names the quantity: {body}");
        assert!(body.contains("at least 1"), "{body}");
        app.put(
            "/holding_accounts/1",
            &json!({ "name": "Default", "unit_scale": 29 }),
        )
        .await
        .expect_status(StatusCode::UNPROCESSABLE_ENTITY);
        set_account_scale(&app, Some(1)).await;

        // The override is checked against its own pair.
        app.post(
            "/holding_unit_scales",
            &json!({ "holding_account_id": 1, "listing_id": 1, "unit_scale": 0 }),
        )
        .await
        .expect_status(StatusCode::UNPROCESSABLE_ENTITY);
        app.post(
            "/holding_unit_scales",
            &json!({ "holding_account_id": 1, "listing_id": 1, "unit_scale": 2 }),
        )
        .await
        .expect_status(StatusCode::CREATED);
        // Listing 1 is the override's now, so the account itself only has
        // listing 2's whole units to fit.
        set_account_scale(&app, Some(0)).await;
    }

    /// The check's own read: unknown account or listing is not its to refuse
    /// (the write's foreign key does that), and NULL never refuses.
    #[tokio::test]
    async fn db_too_fine_resolves_the_holdings_scale() {
        let pool = test_pool().await;
        listing(1).insert(&pool).await;
        let mut conn = pool.acquire().await.unwrap();
        let (account, l1) = (HoldingAccountId(1), ListingId(1));
        assert_eq!(
            db_too_fine(&mut conn, account, l1, dec("0.5"))
                .await
                .unwrap(),
            None
        );
        sqlx::query("UPDATE holding_accounts SET unit_scale = 0 WHERE id = 1")
            .execute(&mut *conn)
            .await
            .unwrap();
        let too_fine = db_too_fine(&mut conn, account, l1, dec("0.5"))
            .await
            .unwrap()
            .unwrap();
        assert_eq!((too_fine.scale, too_fine.from), (0, ScaleSource::Account));
        assert_eq!(
            db_too_fine(&mut conn, HoldingAccountId(99), l1, dec("0.5"))
                .await
                .unwrap(),
            None
        );
        assert_eq!(
            db_too_fine(&mut conn, account, ListingId(99), dec("0.5"))
                .await
                .unwrap(),
            None
        );
    }
}
