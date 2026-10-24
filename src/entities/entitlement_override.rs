//! The "units received" override of a rounded corporate action
//! (`entitlement_overrides`, migration 0055): the count one holding account's
//! statement says it received from a ratio-driven action whose
//! `fraction_rule` is not `Exact`, where that disagrees with the rule's own
//! arithmetic. It replaces the holding's rounded total, which is then
//! distributed across its parcels by largest remainder exactly as a computed
//! total is (`domain::entitlement_rounding`).
//!
//! For a `ShareSplit`/`BonusIssue` the override takes effect at once: the
//! write recomputes the listing's stored per-parcel counts inside its own
//! transaction (and checks the sales after the action still fit them). For a
//! `ScripForScrip`/`Demerger` it is read when the exchange or demerger runs —
//! the units are of the replacement (demerged) listing — so it is frozen,
//! like the action's own terms, once that operation has run.
//!
//! Audited: a user-entered figure that decides stored quantities.

use crate::entities::corporate_action::{self, ActionKind, FractionRule};
use crate::infra::db::{self, write_tx};
use crate::infra::decimal::Money;
use crate::infra::extract::Json;
use crate::infra::http::{
    self, ApiError, CrudEntity, CrudListFilter, FilterClauses, UpsertResponse, Upserted,
};
use crate::infra::id::{HoldingAccountId, ListingId};
use crate::infra::respond::{Created, NoContent};
use axum::extract::{Path, State};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;
use std::iter;
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

/// A holding account's stated count from one rounded corporate action.
#[derive(utoipa::ToSchema, Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct EntitlementOverride {
    pub id: i64,
    /// The ratio-driven action (`ShareSplit`, `BonusIssue`, `ScripForScrip`,
    /// `Demerger`) whose rounding this overrides.
    pub corporate_action_id: i64,
    /// The holding account whose count it states.
    pub holding_account_id: HoldingAccountId,
    /// Units the holding received: its whole post-action count of the
    /// action's listing (of the replacement or demerged listing for an
    /// exchange or demerger), on the holding's unit grid.
    #[sqlx(try_from = "Money")]
    pub units_received: Decimal,
}

/// The `PUT`/`POST /entitlement_overrides` body.
#[derive(utoipa::ToSchema, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EntitlementOverrideBody {
    /// The ratio-driven action whose rounding the statement disagrees with;
    /// its `fraction_rule` must not be `Exact`.
    pub corporate_action_id: i64,
    /// The holding account the statement is for.
    pub holding_account_id: HoldingAccountId,
    /// The units the statement says the holding received from the action
    /// (positive, on the holding's unit grid).
    #[serde(deserialize_with = "crate::infra::decimal::strict_decimal")]
    pub units_received: Decimal,
}

/// The `/entitlement_overrides` list filters.
#[derive(Debug, Default, Deserialize, utoipa::IntoParams)]
#[serde(deny_unknown_fields)]
pub struct EntitlementOverrideListQuery {
    pub corporate_action_id: Option<i64>,
    pub holding_account_id: Option<HoldingAccountId>,
}

impl CrudListFilter for EntitlementOverrideListQuery {
    fn apply_filter(&self, clauses: &mut FilterClauses<'_>) {
        clauses.eq("corporate_action_id", self.corporate_action_id);
        clauses.eq("holding_account_id", self.holding_account_id);
    }
}

impl CrudEntity for EntitlementOverride {
    type Key = i64;
    type Filter = EntitlementOverrideListQuery;
    const TABLE: &'static str = "entitlement_overrides";
    const COLUMNS: &'static str = "id, corporate_action_id, holding_account_id, units_received";
    const NOUN: &'static str = "units-received override";
}

http::crud_list_route!(
    list,
    EntitlementOverride,
    EntitlementOverrideListQuery,
    "/entitlement_overrides",
    "List the units-received overrides of rounded corporate actions; filter with ?corporate_action_id= and ?holding_account_id=."
);
http::crud_get_route!(
    get_one,
    EntitlementOverride,
    i64,
    "/entitlement_overrides/{id}",
    "Fetch one units-received override, or 404."
);

pub fn router() -> OpenApiRouter<SqlitePool> {
    OpenApiRouter::new()
        .routes(routes!(list, create))
        .routes(routes!(get_one, upsert, delete_one))
}

/// Why an override write or delete failed.
#[derive(thiserror::Error, Debug)]
pub enum OverrideError {
    #[error("units-received override write failed: {0}")]
    Db(#[from] sqlx::Error),
    /// No corporate action with that id. Mapped to `422` (a body naming a
    /// missing parent).
    #[error("no corporate action with that id")]
    ActionNotFound,
    /// The action is not ratio-driven, or its rule is `Exact` (nothing is
    /// rounded, so there is no rounded count to override). Mapped to `422`.
    #[error("the action does not round a holding")]
    NotRounded,
    /// The action's exchange or demerger has run (or its cash in lieu has been
    /// paid): the override was read then, and its replacement parcels stand.
    /// Mapped to `422`.
    #[error("the action's operation has already run")]
    OperationRan,
    /// The listing's rounded counts cannot be stored with the override.
    #[error("the listing's rounded entitlements cannot be stored: {0}")]
    Rounding(#[source] corporate_action::RoundError),
    /// The recomputed counts leave a later sale allocating more units than its
    /// parcel holds. Mapped to `422`.
    #[error("the override leaves a sale allocating more units than its parcel holds")]
    AllocationsExceedParcel,
    /// A create committed but the row could not be read back. Mapped to `500`.
    #[error("the units-received override was created but could not be read back")]
    VanishedAfterCreate,
}

impl From<corporate_action::RoundError> for OverrideError {
    fn from(e: corporate_action::RoundError) -> Self {
        match e {
            corporate_action::RoundError::Db(err) => OverrideError::Db(err),
            other @ corporate_action::RoundError::Holding { .. } => OverrideError::Rounding(other),
        }
    }
}

impl From<OverrideError> for ApiError {
    fn from(e: OverrideError) -> Self {
        match e {
            OverrideError::ActionNotFound => {
                ApiError::unprocessable("no corporate action with that corporate_action_id")
            }
            OverrideError::NotRounded => ApiError::unprocessable(
                "a units-received override applies only to a ShareSplit, BonusIssue, \
                 ScripForScrip or Demerger whose fraction_rule is not Exact — an Exact action \
                 rounds nothing",
            ),
            OverrideError::OperationRan => ApiError::unprocessable(
                "this action's exchange, demerger or cash-in-lieu payment has already been \
                 recorded, and its parcels were written from the units received then — undo \
                 that operation first",
            ),
            OverrideError::Rounding(e) => match e.detail() {
                Some(detail) => ApiError::Unprocessable(detail),
                None => ApiError::internal(e),
            },
            OverrideError::AllocationsExceedParcel => ApiError::unprocessable(
                "with these units received, a sale after the action allocates more units than \
                 the parcel it draws on holds — correct or remove those allocations first",
            ),
            OverrideError::Db(err) => err.into(),
            err @ OverrideError::VanishedAfterCreate => ApiError::internal(err),
        }
    }
}

/// The action's listing and whether its rounding is applied at read time
/// (a split/bonus, recomputed by this write) — refusing an action that rounds
/// nothing, or whose operation has run.
async fn check_action(
    conn: &mut sqlx::SqliteConnection,
    action_id: i64,
) -> Result<(ListingId, bool), OverrideError> {
    let action = corporate_action::db_get_tx(&mut *conn, action_id)
        .await?
        .ok_or(OverrideError::ActionNotFound)?;
    let (rule, read_time) = match action.kind {
        ActionKind::ShareSplit { fraction_rule, .. }
        | ActionKind::BonusIssue { fraction_rule, .. } => (fraction_rule, true),
        ActionKind::ScripForScrip { fraction_rule, .. }
        | ActionKind::Demerger { fraction_rule, .. } => (fraction_rule, false),
        ActionKind::ReturnOfCapital { .. }
        | ActionKind::RightsIssue { .. }
        | ActionKind::BuyBack { .. }
        | ActionKind::WorthlessShares { .. } => return Err(OverrideError::NotRounded),
    };
    if rule == FractionRule::Exact {
        return Err(OverrideError::NotRounded);
    }
    if corporate_action::db_operation_ran(&mut *conn, action_id).await? {
        return Err(OverrideError::OperationRan);
    }
    Ok((action.listing_id, read_time))
}

/// Recompute the stored counts of the listing a split/bonus override belongs
/// to, and check the sales after it still fit.
async fn recompute(
    conn: &mut sqlx::SqliteConnection,
    listing_id: ListingId,
) -> Result<(), OverrideError> {
    corporate_action::db_recompute_listing(&mut *conn, listing_id).await?;
    if !corporate_action::allocations_fit_parcels(&mut *conn, listing_id).await? {
        return Err(OverrideError::AllocationsExceedParcel);
    }
    Ok(())
}

/// Write an override, allocating its id when `id` is `None`.
async fn write(
    pool: &SqlitePool,
    id: Option<i64>,
    body: &EntitlementOverrideBody,
) -> Result<Upserted<EntitlementOverride>, OverrideError> {
    let mut tx = write_tx(pool).await?;
    let outcome =
        http::Upsert::of(http::crud_exists_opt::<EntitlementOverride, _>(&mut *tx, id).await?);
    // An edit can move the override off another action; that one is checked
    // (and recomputed) too.
    let previous: Option<i64> = match id {
        Some(id) => {
            sqlx::query_scalar("SELECT corporate_action_id FROM entitlement_overrides WHERE id = ?")
                .bind(id)
                .fetch_optional(&mut *tx)
                .await?
        }
        None => None,
    };
    let mut listings = Vec::new();
    for action_id in iter::once(body.corporate_action_id).chain(previous) {
        let (listing, read_time) = check_action(&mut tx, action_id).await?;
        if read_time {
            listings.push(listing);
        }
    }
    let result = db::insert_with_optional_id(
        "INSERT INTO entitlement_overrides \
         (id, corporate_action_id, holding_account_id, units_received) VALUES (?, ?, ?, ?) \
         ON CONFLICT(id) DO UPDATE SET \
             corporate_action_id = excluded.corporate_action_id, \
             holding_account_id  = excluded.holding_account_id, \
             units_received      = excluded.units_received",
        id,
    )
    .bind(body.corporate_action_id)
    .bind(body.holding_account_id)
    .bind(Money(body.units_received))
    .execute(&mut *tx)
    .await?;
    let assigned = id.unwrap_or_else(|| result.last_insert_rowid());
    listings.dedup();
    for listing in listings {
        recompute(&mut tx, listing).await?;
    }
    let upserted =
        http::crud_upserted::<EntitlementOverride, _>(&mut *tx, outcome, assigned).await?;
    tx.commit().await?;
    Ok(upserted)
}

/// Delete the override at `id`, `Ok(false)` when there is none; the holding
/// falls back to its rule's own rounding.
pub async fn db_delete(pool: &SqlitePool, id: i64) -> Result<bool, OverrideError> {
    let mut tx = write_tx(pool).await?;
    let action_id: Option<i64> =
        sqlx::query_scalar("SELECT corporate_action_id FROM entitlement_overrides WHERE id = ?")
            .bind(id)
            .fetch_optional(&mut *tx)
            .await?;
    let Some(action_id) = action_id else {
        return Ok(false);
    };
    // An override on an action that no longer rounds (its rule set back to
    // Exact) is simply removed; one whose operation ran is frozen with it.
    let (listing, read_time) = match check_action(&mut tx, action_id).await {
        Ok(found) => (Some(found.0), found.1),
        Err(OverrideError::NotRounded) => (None, false),
        Err(e) => return Err(e),
    };
    sqlx::query("DELETE FROM entitlement_overrides WHERE id = ?")
        .bind(id)
        .execute(&mut *tx)
        .await?;
    if let Some(listing) = listing.filter(|_| read_time) {
        recompute(&mut tx, listing).await?;
    }
    tx.commit().await?;
    Ok(true)
}

#[utoipa::path(
    put,
    path = "/entitlement_overrides/{id}",
    summary = "Create or replace the units-received override at this id."
)]
async fn upsert(
    State(pool): State<SqlitePool>,
    Path(id): Path<i64>,
    Json(body): Json<EntitlementOverrideBody>,
) -> Result<UpsertResponse<EntitlementOverride>, ApiError> {
    let Upserted { outcome, created } = write(&pool, Some(id), &body).await?;
    http::upsert_response::<EntitlementOverride>(outcome, created)
}

#[utoipa::path(
    post,
    path = "/entitlement_overrides",
    summary = "Create a units-received override; the created row is returned."
)]
async fn create(
    State(pool): State<SqlitePool>,
    Json(body): Json<EntitlementOverrideBody>,
) -> Result<Created<EntitlementOverride>, ApiError> {
    let created = write(&pool, None, &body)
        .await?
        .created
        .ok_or(OverrideError::VanishedAfterCreate)?;
    Ok(Created(created))
}

#[utoipa::path(
    delete,
    path = "/entitlement_overrides/{id}",
    summary = "Delete the units-received override; the holding falls back to its rule's own rounding."
)]
async fn delete_one(
    State(pool): State<SqlitePool>,
    Path(id): Path<i64>,
) -> Result<NoContent, ApiError> {
    let found = db_delete(&pool, id).await?;
    http::deleted(found, EntitlementOverride::NOUN)
}
