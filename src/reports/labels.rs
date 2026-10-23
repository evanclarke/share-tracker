//! How a report's rejection or detail message names a listing or a holding
//! account: by ticker or name, never by raw foreign-key id (the error-bodies
//! contract in API.md).

use crate::infra::id::{HoldingAccountId, ListingId};
use sqlx::SqlitePool;

/// A listing's ticker for a rejection or detail message — the error-bodies
/// contract (API.md) names entities by ticker/name, never by raw foreign-key
/// id. Falls back to `listing <id>` when no such row exists (e.g. an unknown
/// id straight from the request).
pub(crate) async fn listing_label(
    pool: &SqlitePool,
    listing_id: ListingId,
) -> Result<String, sqlx::Error> {
    let ticker: Option<String> = sqlx::query_scalar("SELECT ticker FROM listings WHERE id = ?")
        .bind(listing_id)
        .fetch_optional(pool)
        .await?;
    Ok(ticker.unwrap_or_else(|| format!("listing {listing_id}")))
}

/// A holding account's quoted name for a message (`account 'Default'`),
/// falling back to `account <id>` when no such row exists.
pub(crate) async fn account_label(
    pool: &SqlitePool,
    account_id: HoldingAccountId,
) -> Result<String, sqlx::Error> {
    let name: Option<String> = sqlx::query_scalar("SELECT name FROM holding_accounts WHERE id = ?")
        .bind(account_id)
        .fetch_optional(pool)
        .await?;
    Ok(name.map_or_else(
        || format!("account {account_id}"),
        |n| format!("account '{n}'"),
    ))
}
