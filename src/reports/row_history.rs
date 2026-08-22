//! Row-history inspection: the read side of the append-only audit trail
//! (migration `0013_row_history.sql`; aligns with the ATO record-keeping
//! guidance mirrored in `docs/ato/cgt-keeping-records-shares.md`).
//!
//! Database triggers record the prior row on every UPDATE and DELETE of an
//! audited table into `row_history`; this report reads it back two ways:
//!
//! - **one row's trail** (`{table, row_id}`) — every prior version of one
//!   record, so an accidental edit to a historical fact can be noticed and
//!   reconstructed;
//! - **recent changes** (no `row_id`) — the newest entries across every
//!   audited table, cursor-paged. A multi-row operation writes entries for
//!   rows the user never named — a demerger's replacement Buys, a cascade's
//!   attachments, the price rows a bulk clear removes — and those ids appear
//!   in no list endpoint afterwards, so the single-row form alone can only be
//!   asked about a row you already know the id of (SCENARIOS U-b). Browsing
//!   by *when it happened* is the way in: find the operation, then drill into
//!   the row's own trail with the `table_name`/`row_id` the entry carries.
//!
//! A trail is keyed on `(table_name, row_id)`, and nothing binds an id to one
//! row for the life of the database — so the single-row form additionally
//! segments a trail into the successive **occupants** of the id, and says
//! which entries are the asked-for record's own (see [`Occupants`]).
//!
//! Read-only — the trail itself is written by the triggers alone and is
//! append-only (enforced in the schema), so there is nothing here to write.

use crate::infra::http::ApiError;
use axum::{Json, Router, extract::State, routing::post};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use sqlx::{Row, SqlitePool};

/// The audited tables, exactly as migration 0013 enumerates them in the
/// `row_history.table_name` CHECK and its per-table trigger pairs — a test
/// pins the three lists to each other, and the web UI's table picker is
/// asserted against this list too. Five joined later: `listing_renames`
/// (0018), `closing_prices` (0021, once 0020 made a price hand-enterable),
/// `tax_year_settings` (0027), `rba_fx_rates` (0031, once a stored rate
/// became correctable) and `exchange_holidays` (0039, once the calendar was
/// shown to be read *live* by valuation rather than only consumed at trade
/// write time) — each migration rebuilding `row_history` to extend the CHECK.
pub const AUDITED_TABLES: [&str; 22] = [
    "trades",
    "parcel_allocations",
    "income",
    "interest_income",
    "amma_statements",
    "amit_adjustments",
    "ess_statements",
    "transfers",
    "corporate_actions",
    "inheritances",
    "rights_sales",
    "rights_sale_allocations",
    "investment_expenses",
    "drp_enrolments",
    "cgt_settings",
    "attachments",
    "listings",
    "listing_renames",
    "closing_prices",
    "tax_year_settings",
    "rba_fx_rates",
    "exchange_holidays",
];

/// One audited table, resolved from a requested name: how `row_history`
/// keys its rows, and whether that key can ever be handed to a *different*
/// record.
///
/// Every audited table keys the trail on its surrogate `id` — a value SQLite
/// or the server picks, freed for re-use the moment the row is deleted — with
/// one exception. `tax_year_settings`'s `row_id` is the financial year
/// itself, and migration 0027 says why that is not the same thing:
/// *"`tax_year` is already a meaningful integer, and it is never reused for a
/// different fact. Deleting FY2026's settings and entering them again is the
/// **same** taxpayer-year fact, so inheriting that year's own history is right
/// rather than a leak."* A natural key names one fact forever, so its trail is
/// always a single occupant however often the row is re-entered.
#[derive(Clone, Copy, Debug)]
struct AuditedTable {
    /// The table's own name — always one of the [`AUDITED_TABLES`] constants,
    /// never request text, because it is interpolated into the occupancy
    /// check's SQL (an identifier cannot be bound as a parameter).
    name: &'static str,
    /// The column the trail's `row_id` holds for this table.
    key_column: &'static str,
    /// Whether a *different* record can later hold the same key.
    key_is_reusable: bool,
}

impl AuditedTable {
    /// Resolve a requested table name against [`AUDITED_TABLES`]. `None` for
    /// anything else — which [`report`] rejects `422` before it ever reaches
    /// here, and which `row_history.table_name`'s CHECK constraint guarantees
    /// has no entries in the trail either way.
    fn parse(name: &str) -> Option<Self> {
        let name = *AUDITED_TABLES.iter().find(|t| **t == name)?;
        Some(match name {
            "tax_year_settings" => Self {
                name,
                key_column: "tax_year",
                key_is_reusable: false,
            },
            _ => Self {
                name,
                key_column: "id",
                key_is_reusable: true,
            },
        })
    }
}

/// Walks one row's trail **newest first**, handing each entry the occupant of
/// the id it belongs to: `1` is the id's most recent occupant, `2` the record
/// that held it before that, and so on.
///
/// Why a trail needs segmenting at all: nothing binds an id to one row for the
/// life of the database. Delete a row and the id can be handed out again — by
/// hand (`PUT /trades/9072` after deleting trade 9072), or, as it happened
/// live, by the server (`POST /corporate_actions/1/demerge` computed its
/// closing Sell's id as `MAX(id) + 1` and took the just-deleted trade 9072's
/// id back, SCENARIOS U-a). The new occupant then inherits every entry the
/// previous one left, and the trail presents a 2025 share sale as the past of
/// a 2023 demerger row.
///
/// The server half is now closed at the source: every audited table's id is
/// `AUTOINCREMENT` (0021/0039/0045) and no write path computes an id of its
/// own, so a server-created row can never take a freed one. The hand-entered
/// half stays open on purpose — re-entering a mis-keyed row under its old id
/// is a legitimate workflow — which is what this marking is for, along with
/// every trail already carrying a reuse from before the fix.
///
/// The trail already holds the evidence, because INSERTs are not recorded: a
/// `DELETE` entry can only mean the record it describes ended there, so a
/// `DELETE` on an id that **still holds a row** can only mean the id was
/// handed out again afterwards. Every `DELETE` therefore closes an occupancy,
/// and the entries at or before it belong to an earlier occupant. The one
/// exception is the newest entry of a trail whose id holds no row now: that
/// `DELETE` is the current occupant's own death — an ordinary deleted row, one
/// occupant, not a re-use.
///
/// What this cannot know is *when* the id was taken again, because the
/// re-insert recorded nothing; only that it was. Nor can it know whether the
/// new occupant is a re-entry of the same record (deleting a mis-keyed row and
/// re-entering it under the same id reads as two occupants, which is all the
/// trail can honestly say).
struct Occupants {
    /// The occupant the walk is currently inside.
    occupant: i64,
    /// Whether the id holds a row *now* — read from the same snapshot as the
    /// trail, and the whole basis for telling a re-used id from a deleted one.
    occupied: bool,
    /// [`AuditedTable::key_is_reusable`]: false pins every entry to occupant 1.
    reusable: bool,
    /// Whether any entry has been walked yet (the newest entry is the only one
    /// that can be the current occupant's own `DELETE`).
    seen_any: bool,
}

impl Occupants {
    fn new(occupied: bool, reusable: bool) -> Self {
        Self {
            occupant: 1,
            occupied,
            reusable,
            seen_any: false,
        }
    }

    /// The occupant the next (older) entry belongs to, and whether that
    /// occupant is the record holding the id now.
    fn next(&mut self, operation: &str) -> (i64, bool) {
        if self.reusable && operation == "DELETE" && (self.seen_any || self.occupied) {
            self.occupant += 1;
        }
        self.seen_any = true;
        (self.occupant, self.occupant == 1 && self.occupied)
    }
}

/// Browse page size when the request names none.
pub const DEFAULT_BROWSE_LIMIT: i64 = 100;
/// The largest page the browse form will answer: past this the response is a
/// refusal naming the cap, never a silently truncated page.
pub const MAX_BROWSE_LIMIT: i64 = 1000;

#[derive(Debug, Deserialize)]
pub struct RowHistoryRequest {
    /// One of [`AUDITED_TABLES`]; anything else is rejected 422. Required
    /// alongside `row_id` (a row id means nothing without the table it is an
    /// id in); optional on its own, where it filters the browse page to one
    /// table. Omitted entirely, the browse page spans every audited table.
    #[serde(default)]
    pub table: Option<String>,
    /// The audited row's `id` — for `tax_year_settings`, whose identity *is*
    /// the financial year, that year. A row with no recorded history (never
    /// updated or deleted since the trail began) returns an empty array.
    /// Omitted, the request is the browse form.
    #[serde(default)]
    pub row_id: Option<i64>,
    /// Browse cursor: return the entries **older** than this trail id (the
    /// `next_before_id` of the page before). A cursor rather than an offset
    /// because the trail is append-only — new entries land at the top, which
    /// would shift an offset page under a concurrent write.
    #[serde(default)]
    pub before_id: Option<i64>,
    /// Browse page size: 1..=[`MAX_BROWSE_LIMIT`], default
    /// [`DEFAULT_BROWSE_LIMIT`].
    #[serde(default)]
    pub limit: Option<i64>,
}

pub fn router() -> Router<SqlitePool> {
    Router::new().route("/reports/row_history", post(report))
}

/// One browse entry: the trail's own uniform columns, and nothing else.
///
/// Deliberately *not* the single-row form's flattened old row: entries from
/// different tables have different columns, and every data table in the web
/// UI is one `filterableTable` with one column set. `old_row` is not
/// summarised either — a summary would have to choose what to show and could
/// misrepresent what changed — so the prior values stay one drill-down away,
/// through the single-row form this entry names in full (`table_name` +
/// `row_id`).
#[derive(Debug, Serialize, sqlx::FromRow)]
pub struct BrowseEntry {
    #[sqlx(rename = "id")]
    pub history_id: i64,
    pub table_name: String,
    pub row_id: i64,
    pub operation: String,
    pub changed_at: String,
}

/// A browse page: the entries plus what it took to get them and how to
/// continue. `next_before_id` is `None` exactly when the page reached the end
/// of the trail, so "there is more" is a stated fact rather than something
/// the caller has to infer from a full-looking page.
#[derive(Debug, Serialize)]
pub struct RowHistoryPage {
    pub entries: Vec<BrowseEntry>,
    pub page_size: i64,
    pub next_before_id: Option<i64>,
}

/// The two response shapes of the one endpoint, untagged so each serialises
/// as itself: the single-row trail is the flat array it has always been, and
/// the browse page is an object (it carries the cursor as well as the rows).
#[derive(Debug, Serialize)]
#[serde(untagged)]
pub enum RowHistoryResponse {
    Row(Vec<Map<String, Value>>),
    Page(RowHistoryPage),
}

/// One (table, row)'s audit entries, newest first. Each entry flattens the
/// stored old-row JSON behind its own five fields (`history_id`,
/// `operation`, `changed_at`, `occupant`, `current_occupant`), so a set of
/// entries renders as one table whose remaining columns are the audited
/// table's own — including `id`, which is the audited row's id (= the
/// request's `row_id`).
///
/// `occupant` and `current_occupant` say which record's history an entry
/// actually is: an id can be handed to a second record after the first is
/// deleted, and the trail is then two records' pasts under one key. See
/// [`Occupants`] for how the boundary is found and what it cannot know.
pub async fn db_row_history(
    pool: &SqlitePool,
    table: &str,
    row_id: i64,
) -> Result<Vec<Map<String, Value>>, sqlx::Error> {
    let audited = AuditedTable::parse(table);
    // Both reads on one transaction: whether the id still holds a row is what
    // separates a re-used id from a plainly deleted one, so it has to come
    // from the same snapshot as the trail — a delete landing between the two
    // would label the boundary against a row that had just gone.
    let mut tx = pool.begin().await?;
    let occupied = match audited {
        Some(t) => {
            sqlx::query_scalar::<_, bool>(sqlx::AssertSqlSafe(format!(
                "SELECT EXISTS (SELECT 1 FROM {} WHERE {} = ?)",
                t.name, t.key_column
            )))
            .bind(row_id)
            .fetch_one(&mut *tx)
            .await?
        }
        // Not an audited table: the CHECK on `row_history.table_name` means
        // the trail below is empty, so there is nothing to segment.
        None => false,
    };
    let rows = sqlx::query(
        "SELECT id, operation, changed_at, old_row FROM row_history \
         WHERE table_name = ? AND row_id = ? ORDER BY id DESC",
    )
    .bind(table)
    .bind(row_id)
    .fetch_all(&mut *tx)
    .await?;
    tx.commit().await?;

    let mut occupants = Occupants::new(occupied, audited.is_some_and(|t| t.key_is_reusable));
    rows.iter()
        .map(|row| {
            let mut entry = Map::new();
            entry.insert("history_id".into(), row.try_get::<i64, _>("id")?.into());
            let operation: String = row.try_get("operation")?;
            let (occupant, current_occupant) = occupants.next(&operation);
            entry.insert("operation".into(), operation.into());
            entry.insert(
                "changed_at".into(),
                row.try_get::<String, _>("changed_at")?.into(),
            );
            entry.insert("occupant".into(), occupant.into());
            entry.insert("current_occupant".into(), current_occupant.into());
            let old_row: String = row.try_get("old_row")?;
            // Propagate a malformed stored JSON loudly (the same contract as
            // the TEXT decimal columns) — a silently dropped audit entry
            // would defeat the trail's purpose.
            let old_row: Map<String, Value> =
                serde_json::from_str(&old_row).map_err(|e| sqlx::Error::Decode(Box::new(e)))?;
            entry.extend(old_row);
            Ok(entry)
        })
        .collect()
}

/// One page of the trail, newest first, across every audited table (or one,
/// when `table` is given). Ordered by the trail's own `id`, never
/// `changed_at`: every row a single statement changes carries the same
/// timestamp to the millisecond (SQLite's `'now'` is fixed for the duration
/// of one statement), so a multi-row operation's entries tie — and paging a
/// non-total order silently skips or repeats rows. Nor is the converse safe:
/// `'now'` is *not* fixed across a transaction, so two statements of one
/// operation can land on different milliseconds (measured 2026-08-22:
/// 227 ms apart inside one transaction). The id is the only total,
/// write-order key the trail has. One query, so no read transaction is
/// needed to see a consistent snapshot.
pub async fn db_browse_row_history(
    pool: &SqlitePool,
    table: Option<&str>,
    before_id: Option<i64>,
    limit: i64,
) -> Result<RowHistoryPage, sqlx::Error> {
    // One row beyond the page: its presence is what says more entries exist,
    // and it is dropped before answering.
    let mut entries = sqlx::query_as::<_, BrowseEntry>(
        "SELECT id, table_name, row_id, operation, changed_at FROM row_history \
         WHERE (? IS NULL OR table_name = ?) AND (? IS NULL OR id < ?) \
         ORDER BY id DESC LIMIT ?",
    )
    .bind(table)
    .bind(table)
    .bind(before_id)
    .bind(before_id)
    .bind(limit + 1)
    .fetch_all(pool)
    .await?;

    let next_before_id = if entries.len() as i64 > limit {
        entries.truncate(limit as usize);
        entries.last().map(|e| e.history_id)
    } else {
        None
    };
    Ok(RowHistoryPage {
        entries,
        page_size: limit,
        next_before_id,
    })
}

async fn report(
    State(pool): State<SqlitePool>,
    Json(req): Json<RowHistoryRequest>,
) -> Result<Json<RowHistoryResponse>, ApiError> {
    if let Some(table) = &req.table
        && !AUDITED_TABLES.contains(&table.as_str())
    {
        return Err(ApiError::Unprocessable(format!(
            "'{}' is not an audited table (one of: {})",
            table,
            AUDITED_TABLES.join(", ")
        )));
    }

    if let Some(row_id) = req.row_id {
        let Some(table) = req.table.as_deref() else {
            return Err(ApiError::Unprocessable(
                "'row_id' needs the 'table' it is an id in; omit both to browse the recent changes across every audited table".into(),
            ));
        };
        // Browse-only parameters, refused rather than ignored: one row's
        // trail is returned whole, so a cursor or a page size asked for here
        // would be a request the answer does not honour.
        if req.before_id.is_some() || req.limit.is_some() {
            return Err(ApiError::Unprocessable(
                "'before_id' and 'limit' page the browse form; one row's trail is returned in full, so omit 'row_id' to use them".into(),
            ));
        }
        return db_row_history(&pool, table, row_id)
            .await
            .map(|entries| Json(RowHistoryResponse::Row(entries)))
            .map_err(ApiError::from);
    }

    let limit = req.limit.unwrap_or(DEFAULT_BROWSE_LIMIT);
    if !(1..=MAX_BROWSE_LIMIT).contains(&limit) {
        return Err(ApiError::Unprocessable(format!(
            "'limit' must be between 1 and {MAX_BROWSE_LIMIT} (default {DEFAULT_BROWSE_LIMIT}); got {limit}"
        )));
    }
    db_browse_row_history(&pool, req.table.as_deref(), req.before_id, limit)
        .await
        .map(|page| Json(RowHistoryResponse::Page(page)))
        .map_err(ApiError::from)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::entities::{listing, sell, trade};
    use crate::test_support::{self, ApiClient, allocate, test_pool, ymd};
    use axum::http::StatusCode;
    use rust_decimal::Decimal;

    async fn history_count(pool: &SqlitePool, table: &str, row_id: i64) -> i64 {
        sqlx::query_scalar("SELECT COUNT(*) FROM row_history WHERE table_name = ? AND row_id = ?")
            .bind(table)
            .bind(row_id)
            .fetch_one(pool)
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn insert_records_no_history() {
        let pool = test_pool().await;
        test_support::listing(1).insert(&pool).await;
        test_support::buy(1, 1).insert(&pool).await;
        let total: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM row_history")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(total, 0, "an INSERT must not write history");
    }

    #[tokio::test]
    async fn update_records_the_prior_row() {
        let pool = test_pool().await;
        test_support::listing(1).insert(&pool).await;
        test_support::buy(1, 1)
            .qty(Decimal::from(100))
            .insert(&pool)
            .await;

        // Edit the quantity through the entity's own upsert (the real write
        // path: INSERT .. ON CONFLICT DO UPDATE fires the UPDATE trigger).
        let mut edited = trade::db_get(&pool, 1).await.unwrap().unwrap();
        edited.quantity = Decimal::from(150);
        trade::db_upsert(&pool, &edited).await.unwrap();

        let entries = db_row_history(&pool, "trades", 1).await.unwrap();
        assert_eq!(entries.len(), 1);
        let e = &entries[0];
        assert_eq!(e["operation"], "UPDATE");
        assert_eq!(e["quantity"], "100", "old_row holds the prior value");
        assert!(
            e["changed_at"].as_str().unwrap().ends_with('Z'),
            "changed_at is an RFC 3339 UTC timestamp: {:?}",
            e["changed_at"]
        );
    }

    #[tokio::test]
    async fn delete_records_the_prior_row() {
        let pool = test_pool().await;
        test_support::listing(1).insert(&pool).await;
        test_support::buy(1, 1)
            .qty(Decimal::from(100))
            .insert(&pool)
            .await;
        trade::db_delete(&pool, 1).await.unwrap();

        let entries = db_row_history(&pool, "trades", 1).await.unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0]["operation"], "DELETE");
        assert_eq!(entries[0]["quantity"], "100");
        assert_eq!(entries[0]["id"], 1, "old_row carries the deleted row's id");
    }

    #[tokio::test]
    async fn entries_come_newest_first() {
        let pool = test_pool().await;
        test_support::listing(1).insert(&pool).await;
        test_support::buy(1, 1)
            .qty(Decimal::from(100))
            .insert(&pool)
            .await;
        for qty in [150i64, 200] {
            let mut edited = trade::db_get(&pool, 1).await.unwrap().unwrap();
            edited.quantity = Decimal::from(qty);
            trade::db_upsert(&pool, &edited).await.unwrap();
        }

        let entries = db_row_history(&pool, "trades", 1).await.unwrap();
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0]["quantity"], "150", "newest entry first");
        assert_eq!(entries[1]["quantity"], "100");
    }

    /// A 422-rejected write rolls back atomically: the rejected transaction's
    /// history rows vanish with it, and previously recorded history survives
    /// unchanged. Driven through PUT /sells, whose allocation validation
    /// rejects *after* the trade UPDATE has executed inside the transaction.
    #[tokio::test]
    async fn rejected_write_leaves_history_unchanged() {
        let pool = test_pool().await;
        test_support::listing(1).insert(&pool).await;
        test_support::buy(1, 1)
            .qty(Decimal::from(100))
            .insert(&pool)
            .await;
        test_support::sell(2, 1)
            .qty(Decimal::from(50))
            .insert(&pool)
            .await;
        allocate(&pool, 1, 2, 1, Decimal::from(50)).await;

        // One legitimate edit so pre-existing history is at stake too.
        let mut edited = trade::db_get(&pool, 2).await.unwrap().unwrap();
        edited.average_price = Decimal::from(11);
        trade::db_upsert(&pool, &edited).await.unwrap();
        let before = db_row_history(&pool, "trades", 2).await.unwrap();
        assert_eq!(before.len(), 1);

        // Re-PUT the sell with allocations that do not sum to its quantity.
        let body = serde_json::json!({
            "date": ymd(2024, 6, 3), "listing_id": 1, "average_price": "12",
            "quantity": "50", "currency": "AUD", "brokerage": "0",
            "brokerage_currency": "AUD", "fx_rate": "1",
            "allocations": [{ "purchase_trade_id": 1, "quantity_allocated": "10" }],
        });
        let resp = ApiClient::over(sell::router().with_state(pool.clone()))
            .put("/sells/2", &body)
            .await;
        assert_eq!(resp.status, StatusCode::UNPROCESSABLE_ENTITY);

        let after = db_row_history(&pool, "trades", 2).await.unwrap();
        assert_eq!(
            after, before,
            "a rejected write must leave history exactly as it was"
        );
    }

    #[tokio::test]
    async fn history_is_append_only() {
        let pool = test_pool().await;
        test_support::listing(1).insert(&pool).await;
        test_support::buy(1, 1).insert(&pool).await;
        trade::db_delete(&pool, 1).await.unwrap();

        for sql in [
            "UPDATE row_history SET operation = 'UPDATE'",
            "DELETE FROM row_history",
        ] {
            let err = sqlx::query(sql).execute(&pool).await.unwrap_err();
            assert!(
                err.to_string().contains("append-only"),
                "{sql} must abort: {err}"
            );
        }
    }

    /// Deleting a parent cascade-deletes its children (an attachment dies
    /// with its trade), and the cascade fires the child's DELETE trigger too
    /// — the audit trail records the attachment it took along.
    #[tokio::test]
    async fn cascade_delete_records_child_history() {
        let pool = test_pool().await;
        test_support::listing(1).insert(&pool).await;
        test_support::buy(1, 1).insert(&pool).await;
        sqlx::query(
            "INSERT INTO attachments (id, trade_id, filename, content_type, byte_size, \
             checksum, uploaded_at, content) \
             VALUES (7, 1, 'note.pdf', 'application/pdf', 4, 'abcd', \
             '2024-01-01T00:00:00Z', X'25504446')",
        )
        .execute(&pool)
        .await
        .unwrap();

        trade::db_delete(&pool, 1).await.unwrap();

        let entries = db_row_history(&pool, "attachments", 7).await.unwrap();
        assert_eq!(entries.len(), 1);
        let e = &entries[0];
        assert_eq!(e["operation"], "DELETE");
        assert_eq!(e["filename"], "note.pdf");
        assert_eq!(e["checksum"], "abcd");
        assert!(
            !e.contains_key("content"),
            "the BLOB itself is excluded from the old row"
        );
    }

    #[tokio::test]
    async fn api_returns_flattened_entries_and_rejects_unknown_table() {
        let pool = test_pool().await;
        test_support::listing(1).insert(&pool).await;
        test_support::buy(1, 1)
            .qty(Decimal::from(100))
            .insert(&pool)
            .await;
        let mut edited = trade::db_get(&pool, 1).await.unwrap().unwrap();
        edited.quantity = Decimal::from(150);
        trade::db_upsert(&pool, &edited).await.unwrap();

        let post = |body: String| {
            let client = ApiClient::over(router().with_state(pool.clone()));
            async move { client.post_raw("/reports/row_history", body.as_ref()).await }
        };

        let resp = post(r#"{"table": "trades", "row_id": 1}"#.to_string()).await;
        assert_eq!(resp.status, StatusCode::OK);
        let entries: Vec<Map<String, Value>> = resp.json();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0]["operation"], "UPDATE");
        assert_eq!(entries[0]["quantity"], "100");

        // A row with no recorded history is an empty trail, not an error.
        let resp = post(r#"{"table": "trades", "row_id": 999}"#.to_string()).await;
        assert_eq!(resp.status, StatusCode::OK);
        assert_eq!(resp.text(), "[]");

        // Unknown table: rejected with the audited list, never interpolated
        // into SQL.
        let resp = post(r#"{"table": "sqlite_master", "row_id": 1}"#.to_string()).await;
        assert_eq!(resp.status, StatusCode::UNPROCESSABLE_ENTITY);
        let msg = resp.text();
        assert!(msg.contains("not an audited table"), "{msg}");
    }

    // ---- id re-use: whose history is this? (SCENARIOS U-a) --------------
    //
    // A trail is keyed on (table, row_id), and nothing binds an id to one row
    // for the life of the database — so a re-used id inherits every entry the
    // previous occupant left. Each entry says which occupant it belongs to
    // (`occupant`) and whether that occupant is the record holding the id now
    // (`current_occupant`), so the boundary is stated rather than inferred.

    /// A trail's occupant marking, newest first: `(occupant, current_occupant)`.
    fn occupants(entries: &[Map<String, Value>]) -> Vec<(i64, bool)> {
        entries
            .iter()
            .map(|e| {
                (
                    e["occupant"].as_i64().unwrap(),
                    e["current_occupant"].as_bool().unwrap(),
                )
            })
            .collect()
    }

    async fn edit_trade_qty(pool: &SqlitePool, id: i64, qty: i64) {
        let mut edited = trade::db_get(pool, id).await.unwrap().unwrap();
        edited.quantity = Decimal::from(qty);
        trade::db_upsert(pool, &edited).await.unwrap();
    }

    /// The ordinary case: a row that has only ever been edited is one
    /// occupant, and it is the record you asked about.
    #[tokio::test]
    async fn an_edited_row_that_still_exists_is_one_occupant() {
        let pool = test_pool().await;
        test_support::listing(1).insert(&pool).await;
        test_support::buy(1, 1)
            .qty(Decimal::from(100))
            .insert(&pool)
            .await;
        edit_trade_qty(&pool, 1, 150).await;
        edit_trade_qty(&pool, 1, 200).await;

        let entries = db_row_history(&pool, "trades", 1).await.unwrap();
        assert_eq!(occupants(&entries), vec![(1, true), (1, true)]);
    }

    /// The other ordinary case, and the one a naive "a DELETE means the id was
    /// re-used" rule would get wrong: a trail whose newest entry is a DELETE
    /// on an id holding **no** row is simply a deleted record — one occupant,
    /// its own whole history, no re-use.
    #[tokio::test]
    async fn a_deleted_row_is_one_occupant_not_a_reuse() {
        let pool = test_pool().await;
        test_support::listing(1).insert(&pool).await;
        test_support::buy(1, 1)
            .qty(Decimal::from(100))
            .insert(&pool)
            .await;
        edit_trade_qty(&pool, 1, 150).await;
        trade::db_delete(&pool, 1).await.unwrap();

        let entries = db_row_history(&pool, "trades", 1).await.unwrap();
        assert_eq!(entries[0]["operation"], "DELETE");
        assert_eq!(
            occupants(&entries),
            vec![(1, false), (1, false)],
            "one occupant, deleted — nothing here belongs to anyone else"
        );
    }

    /// Delete a row, put a different record under the same id, edit that: the
    /// trail holds two records' pasts, and says where one ends.
    #[tokio::test]
    async fn a_reused_id_splits_into_two_occupants() {
        let pool = test_pool().await;
        test_support::listing(1).insert(&pool).await;
        test_support::buy(1, 1)
            .date(ymd(2024, 1, 10))
            .qty(Decimal::from(100))
            .insert(&pool)
            .await;
        edit_trade_qty(&pool, 1, 150).await;
        trade::db_delete(&pool, 1).await.unwrap();

        // A different trade takes the freed id (the hand-entered flavour:
        // `PUT /trades/1` after deleting trade 1).
        test_support::buy(1, 1)
            .date(ymd(2025, 3, 4))
            .qty(Decimal::from(7))
            .insert(&pool)
            .await;
        edit_trade_qty(&pool, 1, 9).await;

        let entries = db_row_history(&pool, "trades", 1).await.unwrap();
        assert_eq!(
            occupants(&entries),
            vec![(1, true), (2, false), (2, false)],
            "the DELETE and everything older belong to the previous occupant"
        );
        assert_eq!(entries[0]["quantity"], "7", "the current record's own edit");
        assert_eq!(entries[1]["operation"], "DELETE");
        assert_eq!(
            entries[1]["date"],
            ymd(2024, 1, 10).to_string(),
            "the boundary DELETE is the previous occupant's, not this row's"
        );
    }

    /// The live shape: the record now holding the id has never been edited, so
    /// **every** entry belongs to an earlier occupant and the trail's newest
    /// entry is a DELETE on a row that exists. Nothing here is this record's
    /// own history, and no entry claims to be.
    #[tokio::test]
    async fn a_reused_ids_new_occupant_may_have_no_history_of_its_own() {
        let pool = test_pool().await;
        test_support::listing(1).insert(&pool).await;
        test_support::buy(1, 1)
            .qty(Decimal::from(100))
            .insert(&pool)
            .await;
        trade::db_delete(&pool, 1).await.unwrap();
        test_support::buy(1, 1)
            .qty(Decimal::from(7))
            .insert(&pool)
            .await;

        let entries = db_row_history(&pool, "trades", 1).await.unwrap();
        assert_eq!(occupants(&entries), vec![(2, false)]);
        assert!(
            !entries.iter().any(|e| e["occupant"] == 1),
            "the record holding the id now has left no entries at all"
        );
    }

    /// Delete, recreate, delete, recreate: the trail is segmented into
    /// occupants, not split once at "the" boundary.
    #[tokio::test]
    async fn an_id_reused_twice_segments_into_three_occupants() {
        let pool = test_pool().await;
        test_support::listing(1).insert(&pool).await;
        for qty in [100i64, 200] {
            test_support::buy(1, 1)
                .qty(Decimal::from(qty))
                .insert(&pool)
                .await;
            edit_trade_qty(&pool, 1, qty + 1).await;
            trade::db_delete(&pool, 1).await.unwrap();
        }
        test_support::buy(1, 1)
            .qty(Decimal::from(300))
            .insert(&pool)
            .await;
        edit_trade_qty(&pool, 1, 301).await;

        let entries = db_row_history(&pool, "trades", 1).await.unwrap();
        assert_eq!(
            occupants(&entries),
            vec![
                (1, true),  // UPDATE 300 → 301, the record holding the id now
                (2, false), // DELETE of the second occupant
                (2, false), // UPDATE 200 → 201
                (3, false), // DELETE of the first occupant
                (3, false), // UPDATE 100 → 101
            ]
        );
    }

    /// The case that raised this (SCENARIOS U-a), reproduced end to end and
    /// now **prevented**: no user chose the id at all. The demerge used to
    /// assign its closing Sell `MAX(id) + 1`, so deleting the highest trade
    /// handed the freed id straight to a server-created row — exactly how
    /// live trade 9072, a 2025 share sale, became the LAC demerger's 2023
    /// closing Sell, inheriting its audit trail. Every server-created row now
    /// leaves the id to the database, whose `AUTOINCREMENT` sequence never
    /// re-issues one, so the freed id stays free and the new rows carry no
    /// history at all.
    ///
    /// The boundary marking above still covers the reuse a *user* can still
    /// make (`PUT /trades/9072` after deleting trade 9072), which is
    /// deliberately allowed.
    #[tokio::test]
    async fn a_server_assigned_insert_never_takes_a_deleted_trades_id() {
        let pool = test_pool().await;
        test_support::listing(1).insert(&pool).await;
        test_support::listing(2).ticker("DMG").insert(&pool).await;
        test_support::buy(1, 1)
            .date(ymd(2024, 1, 10))
            .settlement(ymd(2024, 1, 10))
            .qty(Decimal::from(100))
            .insert(&pool)
            .await;
        // The highest trade id, deleted — so `MAX(id) + 1` points back at it.
        test_support::buy(2, 1)
            .date(ymd(2024, 2, 20))
            .qty(Decimal::from(50))
            .insert(&pool)
            .await;

        let client = ApiClient::full(&pool);
        assert_eq!(
            client.delete("/trades/2").await.status,
            StatusCode::NO_CONTENT
        );
        client
            .put_ok(
                "/corporate_actions/7",
                &serde_json::json!({
                    "listing_id": 1, "date": ymd(2024, 6, 1), "action_type": "Demerger",
                    "demerger_listing_id": 2, "demerger_new_units": "1",
                    "demerger_held_units": "5", "demerger_cost_base_pct": "20",
                }),
            )
            .await;
        let resp = client
            .post("/corporate_actions/7/demerge", &serde_json::json!({}))
            .await;
        assert_eq!(resp.status, StatusCode::CREATED);
        let demerge: Value = resp.json();
        let sell_id = demerge["sell"]["id"].as_i64().unwrap();
        // Every id the operation wrote — the closing Sell and both
        // replacement Buys — must be new. Each takes the id its own INSERT
        // was given, so none of them may be the freed 2 either.
        let mut created = vec![sell_id];
        for side in ["head_replacements", "demerged_replacements"] {
            for row in demerge[side].as_array().unwrap() {
                created.push(row["id"].as_i64().unwrap());
            }
        }
        assert_eq!(created.len(), 3, "a closing Sell and two replacement Buys");
        for id in &created {
            assert_ne!(*id, 2, "a server-created row took the deleted trade's id");
            assert!(
                db_row_history(&pool, "trades", *id)
                    .await
                    .unwrap()
                    .is_empty(),
                "trade {id} inherited a trail it never wrote"
            );
        }
        assert_eq!(
            created
                .iter()
                .collect::<std::collections::HashSet<_>>()
                .len(),
            3,
            "each row took its own freshly assigned id"
        );
        // The deleted Buy's own trail still stands under its own id, and no
        // row occupies it — one occupant, an ordinary deletion.
        let entries = db_row_history(&pool, "trades", 2).await.unwrap();
        assert_eq!(occupants(&entries), vec![(1, false)]);
        assert_eq!(entries[0]["operation"], "DELETE");
        assert_eq!(entries[0]["quantity"], "50", "the deleted Buy");
        let still_free: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM trades WHERE id = 2")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(still_free, 0, "the freed id was handed out again");
    }

    /// `tax_year_settings` is keyed on the financial year itself, which names
    /// one taxpayer-year fact forever — migration 0027 says deleting FY2026's
    /// settings and entering them again is the *same* fact, so that year's own
    /// history is inherited rightly. A natural key is never re-used by a
    /// different record, so its trail stays one occupant across a
    /// delete-and-re-enter.
    #[tokio::test]
    async fn a_natural_key_re_entered_is_still_one_occupant() {
        let pool = test_pool().await;
        let client = ApiClient::full(&pool);
        let path = "/tax_year_settings/2026";
        client
            .put_ok(
                path,
                &serde_json::json!({ "ess_taxed_upfront_reduction_eligible": false }),
            )
            .await;
        client
            .put_ok(
                path,
                &serde_json::json!({ "ess_taxed_upfront_reduction_eligible": true }),
            )
            .await;
        assert_eq!(client.delete(path).await.status, StatusCode::NO_CONTENT);
        client
            .put_ok(
                path,
                &serde_json::json!({ "ess_taxed_upfront_reduction_eligible": false }),
            )
            .await;

        let entries = db_row_history(&pool, "tax_year_settings", 2026)
            .await
            .unwrap();
        assert_eq!(entries[0]["operation"], "DELETE");
        assert_eq!(
            occupants(&entries),
            vec![(1, true), (1, true)],
            "the same taxpayer-year fact throughout"
        );
    }

    /// The API answers the marking, so the reader of the endpoint (and the
    /// Row History screen over it) never has to infer the boundary.
    #[tokio::test]
    async fn api_entries_carry_the_occupant_they_belong_to() {
        let pool = test_pool().await;
        test_support::listing(1).insert(&pool).await;
        test_support::buy(1, 1)
            .qty(Decimal::from(100))
            .insert(&pool)
            .await;
        trade::db_delete(&pool, 1).await.unwrap();
        test_support::buy(1, 1)
            .qty(Decimal::from(7))
            .insert(&pool)
            .await;
        edit_trade_qty(&pool, 1, 9).await;

        let entries: Vec<Map<String, Value>> = ApiClient::over(router().with_state(pool.clone()))
            .post_json(
                "/reports/row_history",
                &serde_json::json!({ "table": "trades", "row_id": 1 }),
            )
            .await;
        assert_eq!(occupants(&entries), vec![(1, true), (2, false)]);
    }

    /// The browse form deliberately carries no occupant marking: it lists the
    /// trail in write order, where no entry is presented as any row's own
    /// history, and its columns are the ones every audited table shares. The
    /// question "whose history is this?" is asked of one row's trail, which is
    /// exactly where the drill-through link lands.
    #[tokio::test]
    async fn browse_entries_carry_no_occupant_marking() {
        let pool = test_pool().await;
        test_support::listing(1).insert(&pool).await;
        test_support::buy(1, 1).insert(&pool).await;
        trade::db_delete(&pool, 1).await.unwrap();
        test_support::buy(1, 1).insert(&pool).await;

        let page: Value = ApiClient::over(router().with_state(pool.clone()))
            .post_json("/reports/row_history", &serde_json::json!({}))
            .await;
        let entry = &page["entries"][0];
        assert_eq!(entry["row_id"], 1);
        assert!(entry.get("occupant").is_none(), "{entry:#?}");
        assert!(entry.get("current_occupant").is_none(), "{entry:#?}");
    }

    // ---- browse form (SCENARIOS U-b) ------------------------------------
    //
    // The single-row form can only be asked about a row whose id you already
    // know. A multi-row operation writes entries for rows the user never
    // named — a demerger's replacement Buys, a cascade's attachments — and
    // those ids are in no list endpoint afterwards, so the way in is *when it
    // happened*: the newest entries across every audited table, cursor-paged.

    /// Set up a trail spanning three tables in a known order: a trade edit,
    /// a listing edit, then a Sell delete (which takes its allocation with
    /// it, so that one operation writes two entries sharing a `changed_at`).
    async fn seed_mixed_trail(pool: &SqlitePool) {
        test_support::listing(1).insert(pool).await;
        test_support::buy(1, 1)
            .qty(Decimal::from(100))
            .insert(pool)
            .await;
        test_support::sell(2, 1)
            .qty(Decimal::from(40))
            .insert(pool)
            .await;
        allocate(pool, 1, 2, 1, Decimal::from(40)).await;

        let mut edited = trade::db_get(pool, 1).await.unwrap().unwrap();
        edited.quantity = Decimal::from(150);
        trade::db_upsert(pool, &edited).await.unwrap();

        let mut listing = listing::db_get(pool, 1).await.unwrap().unwrap();
        listing.name = "Renamed".into();
        listing::db_upsert(pool, &listing).await.unwrap();

        sell::db_delete_sell(pool, 2).await.unwrap();
    }

    fn page(resp: &crate::test_support::ApiResponse) -> Value {
        assert_eq!(resp.status, StatusCode::OK);
        resp.json()
    }

    #[tokio::test]
    async fn browse_returns_entries_across_tables_newest_first() {
        let pool = test_pool().await;
        seed_mixed_trail(&pool).await;

        let client = ApiClient::over(router().with_state(pool.clone()));
        let body = page(&client.post_raw("/reports/row_history", "{}").await);
        let entries = body["entries"].as_array().unwrap();

        // Four entries over three tables: the Sell delete's pair, then the
        // listing edit, then the trade edit.
        assert_eq!(entries.len(), 4, "{entries:#?}");
        let tables: Vec<&str> = entries
            .iter()
            .map(|e| e["table_name"].as_str().unwrap())
            .collect();
        assert_eq!(tables[2], "listings");
        assert_eq!(tables[3], "trades");
        let mut newest_two = [tables[0], tables[1]];
        newest_two.sort_unstable();
        assert_eq!(
            newest_two,
            ["parcel_allocations", "trades"],
            "the Sell delete's own two entries are the newest"
        );

        // Uniform across tables: the trail's own columns and nothing else, so
        // one table renders them all. The prior values stay one drill-down
        // away, through the (table_name, row_id) each entry names.
        for e in entries {
            let keys: Vec<&String> = e.as_object().unwrap().keys().collect();
            assert_eq!(
                keys,
                [
                    "history_id",
                    "table_name",
                    "row_id",
                    "operation",
                    "changed_at"
                ],
                "browse entries carry no flattened old row: {e:#?}"
            );
        }

        // Ordered on the trail's own id — total and deterministic. Ordering
        // on `changed_at` would not be: every row one statement deletes
        // carries the same timestamp to the millisecond (see the demerger
        // test below), a tie the database could break either way.
        let ids: Vec<i64> = entries
            .iter()
            .map(|e| e["history_id"].as_i64().unwrap())
            .collect();
        let mut descending = ids.clone();
        descending.sort_unstable_by(|a, b| b.cmp(a));
        assert_eq!(ids, descending, "newest first, by trail id");

        // Nothing more to fetch: the whole trail fitted on the page.
        assert_eq!(body["next_before_id"], Value::Null);
        assert_eq!(body["page_size"], DEFAULT_BROWSE_LIMIT);
    }

    #[tokio::test]
    async fn browse_pages_by_cursor_and_says_when_more_remain() {
        let pool = test_pool().await;
        seed_mixed_trail(&pool).await;
        let client = ApiClient::over(router().with_state(pool.clone()));

        let first = page(
            &client
                .post_raw("/reports/row_history", r#"{"limit": 2}"#)
                .await,
        );
        let ids = |body: &Value| -> Vec<i64> {
            body["entries"]
                .as_array()
                .unwrap()
                .iter()
                .map(|e| e["history_id"].as_i64().unwrap())
                .collect()
        };
        let first_ids = ids(&first);
        assert_eq!(first_ids.len(), 2);
        assert_eq!(first["page_size"], 2);
        // More entries exist, and the response says so rather than just
        // stopping — the cursor to continue with is the last id on the page.
        assert_eq!(first["next_before_id"], first_ids[1]);

        let cursor = first_ids[1];
        let second = page(
            &client
                .post_raw(
                    "/reports/row_history",
                    &format!(r#"{{"limit": 2, "before_id": {cursor}}}"#),
                )
                .await,
        );
        let second_ids = ids(&second);
        assert_eq!(second_ids.len(), 2);
        assert!(
            second_ids.iter().all(|id| *id < cursor),
            "a cursor page is strictly older: {second_ids:?}"
        );
        assert_eq!(
            second["next_before_id"],
            Value::Null,
            "the trail ended on this page"
        );

        let mut all = first_ids;
        all.extend(second_ids);
        all.sort_unstable();
        all.dedup();
        assert_eq!(all.len(), 4, "the pages partition the trail, no repeats");
    }

    #[tokio::test]
    async fn browse_filters_to_one_table_and_refuses_a_bad_request() {
        let pool = test_pool().await;
        seed_mixed_trail(&pool).await;
        let client = ApiClient::over(router().with_state(pool.clone()));

        // `table` without `row_id` is a filter, not a lookup.
        let body = page(
            &client
                .post_raw("/reports/row_history", r#"{"table": "trades"}"#)
                .await,
        );
        let entries = body["entries"].as_array().unwrap();
        assert_eq!(entries.len(), 2);
        assert!(entries.iter().all(|e| e["table_name"] == "trades"));

        // The audited-table check still applies to the filter.
        let resp = client
            .post_raw("/reports/row_history", r#"{"table": "sqlite_master"}"#)
            .await;
        assert_eq!(resp.status, StatusCode::UNPROCESSABLE_ENTITY);
        assert!(resp.text().contains("not an audited table"));

        // A row id means nothing without the table it is an id in.
        let resp = client
            .post_raw("/reports/row_history", r#"{"row_id": 1}"#)
            .await;
        assert_eq!(resp.status, StatusCode::UNPROCESSABLE_ENTITY);
        assert!(resp.text().contains("needs the 'table'"), "{}", resp.text());

        // Browse-only parameters are refused on the single-row form rather
        // than silently ignored.
        let resp = client
            .post_raw(
                "/reports/row_history",
                r#"{"table": "trades", "row_id": 1, "limit": 5}"#,
            )
            .await;
        assert_eq!(resp.status, StatusCode::UNPROCESSABLE_ENTITY);
        assert!(
            resp.text().contains("page the browse form"),
            "{}",
            resp.text()
        );

        // The page size is bounded, and out-of-range is a refusal naming the
        // cap — never a silently truncated page.
        for limit in [0, MAX_BROWSE_LIMIT + 1] {
            let resp = client
                .post_raw("/reports/row_history", &format!(r#"{{"limit": {limit}}}"#))
                .await;
            assert_eq!(
                resp.status,
                StatusCode::UNPROCESSABLE_ENTITY,
                "limit {limit}"
            );
            assert!(
                resp.text().contains(&MAX_BROWSE_LIMIT.to_string()),
                "{}",
                resp.text()
            );
        }
    }

    /// The case that raised this (SCENARIOS U-b): deleting a demerger's
    /// closing Sell removes the whole group — including the replacement Buys
    /// the demerge itself created and the allocation it wrote, rows whose ids
    /// the user never saw and which are in no list endpoint afterwards. The
    /// browse form finds them from *when it happened* alone, and each entry
    /// names the (table, row_id) that drills into its own trail.
    #[tokio::test]
    async fn a_demerger_group_delete_is_findable_without_knowing_any_ids() {
        let pool = test_pool().await;
        test_support::listing(1).insert(&pool).await;
        test_support::listing(2).ticker("DMG").insert(&pool).await;
        test_support::buy(1, 1)
            .date(ymd(2024, 1, 10))
            .settlement(ymd(2024, 1, 10))
            .qty(Decimal::from(100))
            .insert(&pool)
            .await;

        let client = ApiClient::full(&pool);
        client
            .put_ok(
                "/corporate_actions/7",
                &serde_json::json!({
                    "listing_id": 1, "date": ymd(2024, 6, 1), "action_type": "Demerger",
                    "demerger_listing_id": 2, "demerger_new_units": "1",
                    "demerger_held_units": "5", "demerger_cost_base_pct": "20",
                }),
            )
            .await;
        let resp = client
            .post("/corporate_actions/7/demerge", &serde_json::json!({}))
            .await;
        assert_eq!(resp.status, StatusCode::CREATED);
        let demerge: Value = resp.json();
        // The only id the user ever names: the closing Sell, which is the one
        // row of the group any list endpoint shows.
        let sell_id = demerge["sell"]["id"].as_i64().unwrap();
        let created: Vec<i64> = demerge["head_replacements"]
            .as_array()
            .unwrap()
            .iter()
            .chain(demerge["demerged_replacements"].as_array().unwrap())
            .map(|t| t["id"].as_i64().unwrap())
            .collect();
        assert_eq!(created.len(), 2, "one head + one demerged replacement Buy");

        assert_eq!(
            client.delete(&format!("/sells/{sell_id}")).await.status,
            StatusCode::NO_CONTENT
        );
        // The created rows really are gone from every list endpoint: only
        // the original Buy is left, so nothing names the ids the demerge
        // minted.
        let listed: Vec<i64> = client
            .get_json::<Vec<Value>>("/trades")
            .await
            .iter()
            .map(|t| t["id"].as_i64().unwrap())
            .collect();
        assert_eq!(listed, vec![1], "the whole group went with the Sell");

        // Browse: no ids, no table — just what changed most recently. The
        // demerge itself only INSERTed (INSERTs are not audited), so the
        // whole trail is the one delete: four entries over two tables.
        let body: Value = client
            .post_json("/reports/row_history", &serde_json::json!({}))
            .await;
        let entries = body["entries"].as_array().unwrap();
        assert_eq!(
            entries.len(),
            4,
            "one delete, four rows recorded: {entries:#?}"
        );
        let found: Vec<i64> = entries
            .iter()
            .filter(|e| e["table_name"] == "trades")
            .map(|e| e["row_id"].as_i64().unwrap())
            .collect();
        for id in created.iter().chain(std::iter::once(&sell_id)) {
            assert!(
                found.contains(id),
                "the demerge-created trade {id} is reachable from the browse page: {found:?}"
            );
        }
        assert!(
            entries
                .iter()
                .any(|e| e["table_name"] == "parcel_allocations"),
            "the allocation the demerge wrote is on the page too: {entries:#?}"
        );
        // Why the ordering is on the trail's own id: the two replacement Buys
        // go in one DELETE statement, so their entries carry the same
        // `changed_at` to the millisecond — a total order needs the id.
        let stamps: Vec<&str> = entries
            .iter()
            .filter(|e| created.contains(&e["row_id"].as_i64().unwrap()))
            .map(|e| e["changed_at"].as_str().unwrap())
            .collect();
        assert_eq!(stamps.len(), 2);
        assert_eq!(stamps[0], stamps[1], "one statement, one timestamp");

        // Drill in with what the entry itself carries: the prior row comes
        // back from the single-row form, ids and all.
        let unknown = created[0];
        let trail: Vec<Map<String, Value>> = client
            .post_json(
                "/reports/row_history",
                &serde_json::json!({ "table": "trades", "row_id": unknown }),
            )
            .await;
        assert_eq!(trail.len(), 1);
        assert_eq!(trail[0]["operation"], "DELETE");
        assert_eq!(trail[0]["id"], unknown);
        assert_eq!(trail[0]["trade_type"], "Buy");
    }

    /// Every audited table is wired end to end: an UPDATE and a DELETE on a
    /// real row of each table leave history entries. Exercised straight in
    /// SQL (the trigger layer is what's under test) over one row per table,
    /// built respecting the FK graph.
    #[tokio::test]
    async fn every_audited_table_records_update_and_delete() {
        let pool = test_pool().await;
        test_support::listing(1).insert(&pool).await;
        test_support::listing(2).ticker("OTH").insert(&pool).await;
        test_support::buy(1, 1).insert(&pool).await;
        test_support::sell(2, 1)
            .qty(Decimal::from(5))
            .insert(&pool)
            .await;
        allocate(&pool, 1, 2, 1, Decimal::from(5)).await;
        test_support::income(3, 1, ymd(2024, 1, 5))
            .insert(&pool)
            .await;
        test_support::amma(4, 1).insert(&pool).await;
        test_support::ess_statement(5, 1, ymd(2024, 1, 5))
            .insert(&pool)
            .await;
        sqlx::query("INSERT INTO holding_accounts (id, name) VALUES (2, 'other')")
            .execute(&pool)
            .await
            .unwrap();

        // (table, insert-if-any, key column, key, editable column, new value).
        // The key column is `id` everywhere but tax_year_settings, which is
        // keyed on the financial year itself.
        type UpdateCase = (
            &'static str,
            Option<&'static str>,
            &'static str,
            i64,
            &'static str,
            &'static str,
        );
        let cases: Vec<UpdateCase> = vec![
            ("trades", None, "id", 1, "quantity", "'42'"),
            (
                "parcel_allocations",
                None,
                "id",
                1,
                "quantity_allocated",
                "'4'",
            ),
            ("income", None, "id", 3, "unfranked_amount", "'9'"),
            (
                "interest_income",
                Some(
                    "INSERT INTO interest_income (id, date_paid, amount) VALUES (6, '2024-01-01', '5')",
                ),
                "id",
                6,
                "amount",
                "'6'",
            ),
            ("amma_statements", None, "id", 4, "units_held", "'7'"),
            (
                "amit_adjustments",
                Some(
                    "INSERT INTO amit_adjustments (id, amma_statement_id, trade_id, quantity) VALUES (8, 4, 1, '1')",
                ),
                "id",
                8,
                "quantity",
                "'2'",
            ),
            ("ess_statements", None, "id", 5, "quantity", "'3'"),
            (
                "transfers",
                Some(
                    "INSERT INTO transfers (id, listing_id, date, from_account_id, to_account_id) VALUES (9, 1, '2024-01-01', 1, 2)",
                ),
                "id",
                9,
                "date",
                "'2024-01-02'",
            ),
            (
                "corporate_actions",
                Some(
                    "INSERT INTO corporate_actions (id, action_type, listing_id, date, amount_per_unit, currency) VALUES (10, 'ReturnOfCapital', 1, '2024-01-01', '1', 'AUD')",
                ),
                "id",
                10,
                "amount_per_unit",
                "'2'",
            ),
            (
                "inheritances",
                Some(
                    "INSERT INTO inheritances (id, listing_id, quantity, date_of_death, cost_base_rule, cost_base, deceased_acquisition_date) VALUES (11, 1, '10', '2024-01-01', 'DeceasedCostBase', '100', '2020-01-01')",
                ),
                "id",
                11,
                "cost_base",
                "'110'",
            ),
            (
                "rights_sales",
                Some(
                    "INSERT INTO corporate_actions (id, action_type, listing_id, date, rights_units, rights_held_units, exercise_price, currency) VALUES (12, 'RightsIssue', 1, '2024-01-01', '1', '10', '1', 'AUD'); \
                      INSERT INTO rights_sales (id, rights_action_id, date, units) VALUES (13, 12, '2024-01-02', '5')",
                ),
                "id",
                13,
                "units",
                "'4'",
            ),
            (
                "rights_sale_allocations",
                Some(
                    "INSERT INTO rights_sale_allocations (id, rights_sale_id, purchase_trade_id, units) VALUES (14, 13, 1, '5')",
                ),
                "id",
                14,
                "units",
                "'4'",
            ),
            (
                "investment_expenses",
                Some(
                    "INSERT INTO investment_expenses (id, date_incurred, expense_type, amount) VALUES (15, '2024-01-01', 'ManagementFee', '10')",
                ),
                "id",
                15,
                "amount",
                "'11'",
            ),
            (
                "drp_enrolments",
                Some(
                    "INSERT INTO drp_enrolments (id, listing_id, enrolment_date) VALUES (16, 1, '2024-01-01')",
                ),
                "id",
                16,
                "enrolment_date",
                "'2024-01-02'",
            ),
            (
                "cgt_settings",
                Some("INSERT INTO cgt_settings (id, opening_capital_loss) VALUES (1, '0')"),
                "id",
                1,
                "opening_capital_loss",
                "'5'",
            ),
            (
                "attachments",
                Some(
                    "INSERT INTO attachments (id, trade_id, filename, content_type, byte_size, checksum, uploaded_at, content) VALUES (17, 1, 'a.pdf', 'application/pdf', 1, 'x', '2024-01-01T00:00:00Z', X'00')",
                ),
                "id",
                17,
                "filename",
                "'b.pdf'",
            ),
            ("listings", None, "id", 2, "name", "'Renamed'"),
            (
                "listing_renames",
                Some(
                    "INSERT INTO listing_renames (id, listing_id, effective_date, old_ticker, new_ticker) VALUES (18, 1, '2024-01-01', 'OLD', 'NEW')",
                ),
                "id",
                18,
                "note",
                "'edited note'",
            ),
            (
                "closing_prices",
                Some(
                    "INSERT INTO closing_prices (id, listing_id, price_date, price, price_as_observed, source, fetched_at, status, origin) VALUES (19, 1, '2024-01-02', '10', '10', 'yahoo', '2024-01-02T08:00:00Z', 'ok', 'fetched')",
                ),
                "id",
                19,
                "price",
                "'11'",
            ),
            (
                "tax_year_settings",
                Some(
                    "INSERT INTO tax_year_settings (tax_year, ess_taxed_upfront_reduction_eligible) VALUES (2026, 0)",
                ),
                "tax_year",
                2026,
                "ess_taxed_upfront_reduction_eligible",
                "1",
            ),
            (
                "rba_fx_rates",
                Some(
                    "INSERT INTO rba_fx_rates (id, currency, month, rate) VALUES (20, 'USD', '2024-03', '0.65')",
                ),
                "id",
                20,
                "rate",
                "'0.6512'",
            ),
            (
                "exchange_holidays",
                Some(
                    // Explicit id above the 160 seeded holidays' ids.
                    "INSERT INTO exchange_holidays (id, mic, holiday_date, name) VALUES (901, 'XASX', '2030-04-01', 'Test Closure')",
                ),
                "id",
                901,
                "name",
                "'Renamed Closure'",
            ),
        ];
        assert_eq!(cases.len(), AUDITED_TABLES.len());

        for (table, setup, key_column, id, column, new_value) in cases {
            assert!(AUDITED_TABLES.contains(&table), "{table} not in the const");
            if let Some(setup) = setup {
                for stmt in setup.split(';') {
                    sqlx::query(stmt).execute(&pool).await.unwrap();
                }
            }
            sqlx::query(sqlx::AssertSqlSafe(format!(
                "UPDATE {table} SET {column} = {new_value} WHERE {key_column} = {id}"
            )))
            .execute(&pool)
            .await
            .unwrap();
            let updates = history_count(&pool, table, id).await;
            assert_eq!(updates, 1, "{table}: UPDATE must record one entry");
        }

        // Deletes, children before parents so FKs allow them; each leaves a
        // DELETE entry on top of the one UPDATE entry (expected 2), except
        // the two rows deleted only to clear FK paths — the Sell trade and
        // the RightsIssue action were never updated, so their trail is the
        // single DELETE entry (expected 1).
        for (table, key_column, id, expected) in [
            ("attachments", "id", 17i64, 2i64),
            ("rights_sale_allocations", "id", 14, 2),
            ("rights_sales", "id", 13, 2),
            ("corporate_actions", "id", 12, 1),
            ("amit_adjustments", "id", 8, 2),
            ("parcel_allocations", "id", 1, 2),
            ("trades", "id", 2, 1),
            ("investment_expenses", "id", 15, 2),
            ("drp_enrolments", "id", 16, 2),
            ("inheritances", "id", 11, 2),
            ("interest_income", "id", 6, 2),
            ("transfers", "id", 9, 2),
            ("income", "id", 3, 2),
            ("amma_statements", "id", 4, 2),
            ("ess_statements", "id", 5, 2),
            ("cgt_settings", "id", 1, 2),
            ("trades", "id", 1, 2),
            ("corporate_actions", "id", 10, 2),
            ("closing_prices", "id", 19, 2),
            ("listings", "id", 2, 2),
            ("listing_renames", "id", 18, 2),
            ("tax_year_settings", "tax_year", 2026, 2),
            ("exchange_holidays", "id", 901, 2),
        ] {
            sqlx::query(sqlx::AssertSqlSafe(format!(
                "DELETE FROM {table} WHERE {key_column} = {id}"
            )))
            .execute(&pool)
            .await
            .unwrap();
            assert_eq!(
                history_count(&pool, table, id).await,
                expected,
                "{table} {id}: DELETE must be recorded"
            );
        }
    }

    /// The three copies of the audited-table list — the Rust const, the
    /// migration's CHECK constraint, and the migration's trigger pairs —
    /// cannot drift apart. Pinned against 0013 for the tables audited from
    /// the trail's introduction, and against each later migration that
    /// introduced or rebuilt one (mirroring the attachments blocks below).
    #[test]
    fn audited_tables_match_migration_check_and_triggers() {
        let sql = include_str!("../../migrations/0013_row_history.sql");
        // listing_renames (0018), closing_prices (0021), tax_year_settings
        // (0027), rba_fx_rates (0031) and exchange_holidays (0039) postdate
        // 0013, and are checked below — every other table was audited from
        // the start.
        let tables_as_of_0013 = AUDITED_TABLES.into_iter().filter(|&t| {
            t != "listing_renames"
                && t != "closing_prices"
                && t != "tax_year_settings"
                && t != "rba_fx_rates"
                && t != "exchange_holidays"
        });
        let mut count = 0;
        for table in tables_as_of_0013 {
            count += 1;
            assert!(
                sql.contains(&format!("'{table}'")),
                "{table} missing from the table_name CHECK"
            );
            for op in ["update", "delete"] {
                let trigger = format!("CREATE TRIGGER {table}_row_history_{op} ");
                assert!(sql.contains(&trigger), "{table} lacks its {op} trigger");
            }
        }
        assert_eq!(
            sql.matches("CREATE TRIGGER").count(),
            count * 2 + 2,
            "one UPDATE + one DELETE trigger per table audited as of 0013, plus \
             the two append-only guards on row_history itself — nothing extra, \
             nothing missing"
        );

        // 0014 rebuilt the attachments table (new owner columns + text/plain),
        // which dropped its trigger pair with the old table — the *live*
        // attachments triggers come from 0014 and must carry the expanded
        // column list.
        let sql14 = include_str!("../../migrations/0014_attachment_owner_expansion.sql");
        for op in ["update", "delete"] {
            assert!(
                sql14.contains(&format!("CREATE TRIGGER attachments_row_history_{op}")),
                "0014 must re-create the attachments {op} trigger"
            );
        }
        for col in ["ess_statement_id", "interest_income_id"] {
            assert_eq!(
                sql14.matches(&format!("'{col}', OLD.{col}")).count(),
                2,
                "both re-created attachments triggers must record {col}"
            );
        }

        // 0017 rebuilt the attachments table again (corporate_action_id
        // owner), which again dropped its trigger pair with the old table —
        // the *live* attachments triggers come from 0017 and must carry the
        // full column list.
        let sql17 = include_str!("../../migrations/0017_attachment_corporate_action_owner.sql");
        for op in ["update", "delete"] {
            assert!(
                sql17.contains(&format!("CREATE TRIGGER attachments_row_history_{op}")),
                "0017 must re-create the attachments {op} trigger"
            );
        }
        for col in [
            "ess_statement_id",
            "interest_income_id",
            "corporate_action_id",
        ] {
            assert_eq!(
                sql17.matches(&format!("'{col}', OLD.{col}")).count(),
                2,
                "both re-created attachments triggers must record {col}"
            );
        }

        // 0018 added listing_renames and, because the table_name CHECK lives
        // on row_history itself (a table-level CHECK SQLite cannot ALTER),
        // rebuilt row_history to extend it — dropping and re-creating its own
        // append-only guard triggers with it. It also re-created the
        // listings trigger pair (new price_symbol column) and the newly
        // audited listing_renames pair.
        let sql18 = include_str!("../../migrations/0018_listing_renames.sql");
        assert!(
            sql18.contains("'listing_renames'"),
            "0018 must add listing_renames to the table_name CHECK"
        );
        for table in ["row_history", "listings", "listing_renames"] {
            for op in ["update", "delete"] {
                let trigger = if table == "row_history" {
                    format!("CREATE TRIGGER row_history_append_only_{op}")
                } else {
                    format!("CREATE TRIGGER {table}_row_history_{op}")
                };
                assert!(sql18.contains(&trigger), "0018 must re-create {trigger}");
            }
        }
        assert_eq!(
            sql18.matches("'price_symbol', OLD.price_symbol").count(),
            2,
            "both re-created listings triggers must record price_symbol"
        );

        // 0024 added listings.amit_from (SCENARIOS F-23), so the listings
        // trigger pair was re-created once more with it — the *live* pair now
        // comes from 0024, and must still carry every earlier column too.
        let sql24 = include_str!("../../migrations/0024_listing_amit_from.sql");
        for op in ["update", "delete"] {
            assert!(
                sql24.contains(&format!("CREATE TRIGGER listings_row_history_{op}")),
                "0024 must re-create the listings {op} trigger"
            );
        }
        for col in ["amit", "amit_from", "price_symbol", "preference"] {
            assert_eq!(
                sql24.matches(&format!("'{col}', OLD.{col}")).count(),
                2,
                "both re-created listings triggers must record {col}"
            );
        }

        // 0035 added listings.unpriced_from (SCENARIOS Q-02), so the listings
        // trigger pair was re-created again — the *live* pair now comes from
        // 0035, and must still carry every earlier column too.
        let sql35 = include_str!("../../migrations/0035_listing_unpriced_from.sql");
        for op in ["update", "delete"] {
            assert!(
                sql35.contains(&format!("CREATE TRIGGER listings_row_history_{op}")),
                "0035 must re-create the listings {op} trigger"
            );
        }
        for col in [
            "amit",
            "amit_from",
            "unpriced_from",
            "price_symbol",
            "preference",
        ] {
            assert_eq!(
                sql35.matches(&format!("'{col}', OLD.{col}")).count(),
                2,
                "both re-created listings triggers must record {col}"
            );
        }

        // 0037 added listings.unpriced_before (the mirror of 0035's
        // unpriced_from), so the listings trigger pair was re-created once
        // more — the *live* pair now comes from 0037, and must still carry
        // every earlier column too.
        let sql37 = include_str!("../../migrations/0037_listing_unpriced_before.sql");
        for op in ["update", "delete"] {
            assert!(
                sql37.contains(&format!("CREATE TRIGGER listings_row_history_{op}")),
                "0037 must re-create the listings {op} trigger"
            );
        }
        for col in [
            "amit",
            "amit_from",
            "unpriced_from",
            "unpriced_before",
            "price_symbol",
            "preference",
        ] {
            assert_eq!(
                sql37.matches(&format!("'{col}', OLD.{col}")).count(),
                2,
                "both re-created listings triggers must record {col}"
            );
        }

        // 0026 added ess_statements.fx_rate (SCENARIOS J-08/J-12), so that
        // table's trigger pair was re-created with it — the *live* pair now
        // comes from 0026 and must still carry every earlier column too.
        let sql26 = include_str!("../../migrations/0026_ess_statement_fx_rate.sql");
        for op in ["update", "delete"] {
            assert!(
                sql26.contains(&format!("CREATE TRIGGER ess_statements_row_history_{op} ")),
                "0026 must re-create the ess_statements {op} trigger"
            );
        }
        // The column list wraps across lines in this migration, so match on a
        // whitespace-normalised copy rather than the raw text.
        let flat26 = sql26.split_whitespace().collect::<Vec<_>>().join(" ");
        for col in [
            "fx_rate",
            "currency",
            "quantity",
            "market_value_per_share",
            "taxed_upfront_eligible",
            "aud_deferral_discount",
        ] {
            assert_eq!(
                flat26.matches(&format!("'{col}', OLD.{col}")).count(),
                2,
                "both re-created ess_statements triggers must record {col}"
            );
        }

        // 0021 added closing_prices, which needed two rebuilds: the table
        // itself (a surrogate `id` for row_history.row_id to key on, the old
        // composite primary key kept as a UNIQUE constraint) and row_history
        // again to extend the table_name CHECK — re-creating its append-only
        // guards, closing_prices' own staleness trigger, and creating the new
        // audit pair.
        let sql21 = include_str!("../../migrations/0021_audit_closing_prices.sql");
        assert!(
            sql21.contains("'closing_prices'"),
            "0021 must add closing_prices to the table_name CHECK"
        );
        assert!(
            sql21.contains("id           INTEGER PRIMARY KEY AUTOINCREMENT"),
            "the surrogate key must not reuse a deleted row's id"
        );
        assert!(
            sql21.contains("UNIQUE (listing_id, price_date)"),
            "the former primary key stays enforced as a UNIQUE constraint"
        );
        for op in ["update", "delete"] {
            assert!(
                sql21.contains(&format!("CREATE TRIGGER closing_prices_row_history_{op} ")),
                "0021 must create the closing_prices {op} trigger"
            );
            assert!(
                sql21.contains(&format!("CREATE TRIGGER row_history_append_only_{op} ")),
                "0021 must re-create the row_history {op} guard"
            );
        }
        assert!(
            sql21.contains("CREATE TRIGGER closing_prices_stale_snapshots_update "),
            "0021 must re-create closing_prices' staleness trigger"
        );
        // Every column of the rebuilt table is recorded by both triggers —
        // a column the trail drops is a version that cannot be reconstructed.
        for col in [
            "id",
            "listing_id",
            "price_date",
            "price",
            "source",
            "fetched_at",
            "status",
            "error",
            "origin",
            "sourced_from",
            "reason",
        ] {
            assert_eq!(
                sql21.matches(&format!("'{col}', OLD.{col}")).count(),
                2,
                "both closing_prices triggers must record {col}"
            );
        }

        // 0027 added tax_year_settings (SCENARIOS J-02), which — like 0021 —
        // needed row_history rebuilt to extend the table_name CHECK, and so
        // re-created its own append-only guards alongside the new audit pair.
        // No surrogate key this time: `tax_year` is already an integer
        // identity, and it is what row_id records.
        let sql27 = include_str!("../../migrations/0027_tax_year_settings.sql");
        assert!(
            sql27.contains("'tax_year_settings'"),
            "0027 must add tax_year_settings to the table_name CHECK"
        );
        for op in ["update", "delete"] {
            assert!(
                sql27.contains(&format!(
                    "CREATE TRIGGER tax_year_settings_row_history_{op} "
                )),
                "0027 must create the tax_year_settings {op} trigger"
            );
            assert!(
                sql27.contains(&format!("CREATE TRIGGER row_history_append_only_{op} ")),
                "0027 must re-create the row_history {op} guard"
            );
        }
        let flat27 = sql27.split_whitespace().collect::<Vec<_>>().join(" ");
        for col in ["tax_year", "ess_taxed_upfront_reduction_eligible"] {
            assert_eq!(
                flat27.matches(&format!("'{col}', OLD.{col}")).count(),
                2,
                "both tax_year_settings triggers must record {col}"
            );
        }
        assert_eq!(
            sql27.matches("OLD.tax_year, 'UPDATE'").count()
                + sql27.matches("OLD.tax_year, 'DELETE'").count(),
            2,
            "row_id is the financial year itself"
        );

        // 0031 added rba_fx_rates (SCENARIOS M-13): a stored rate became
        // correctable, so — the closing_prices story of 0021 — the superseded
        // figure every earlier report was computed at has to stay recoverable.
        // Again row_history was rebuilt to extend the CHECK, so the *live*
        // append-only guards now come from 0031. No surrogate key was needed:
        // rba_fx_rates.id has been AUTOINCREMENT since 0001.
        let sql31 = include_str!("../../migrations/0031_audit_rba_fx_rates.sql");
        assert!(
            sql31.contains("'rba_fx_rates'"),
            "0031 must add rba_fx_rates to the table_name CHECK"
        );
        for op in ["update", "delete"] {
            assert!(
                sql31.contains(&format!("CREATE TRIGGER rba_fx_rates_row_history_{op} ")),
                "0031 must create the rba_fx_rates {op} trigger"
            );
            assert!(
                sql31.contains(&format!("CREATE TRIGGER row_history_append_only_{op} ")),
                "0031 must re-create the row_history {op} guard"
            );
        }
        let flat31 = sql31.split_whitespace().collect::<Vec<_>>().join(" ");
        for col in ["id", "currency", "month", "rate"] {
            assert_eq!(
                flat31.matches(&format!("'{col}', OLD.{col}")).count(),
                2,
                "both rba_fx_rates triggers must record {col}"
            );
        }

        // 0039 added exchange_holidays (SCENARIOS Q-05/Q-08): the trading
        // calendar is read *live* by valuation, so a hand-edited holiday
        // changes a reported figure — the audited set's own criterion. Like
        // 0021's closing_prices it needed a surrogate `id` for
        // row_history.row_id to key on (the natural key is composite), which
        // meant rebuilding the table as well as row_history, so this pins the
        // whole shape: the CHECK, the surrogate key, the natural key kept as
        // UNIQUE, the three 0033 staleness triggers re-created with the
        // rebuilt table, the re-created append-only guards, and the new audit
        // pair recording every column.
        let sql39 = include_str!("../../migrations/0039_audit_exchange_holidays.sql");
        assert!(
            sql39.contains("'exchange_holidays'"),
            "0039 must add exchange_holidays to the table_name CHECK"
        );
        assert!(
            sql39.contains("id           INTEGER PRIMARY KEY AUTOINCREMENT"),
            "the surrogate key must not reuse a deleted row's id"
        );
        assert!(
            sql39.contains("UNIQUE (mic, holiday_date)"),
            "the former primary key stays enforced as a UNIQUE constraint"
        );
        for op in ["update", "delete"] {
            assert!(
                sql39.contains(&format!(
                    "CREATE TRIGGER exchange_holidays_row_history_{op} "
                )),
                "0039 must create the exchange_holidays {op} trigger"
            );
            assert!(
                sql39.contains(&format!("CREATE TRIGGER row_history_append_only_{op} ")),
                "0039 must re-create the row_history {op} guard"
            );
        }
        for op in ["insert", "update", "delete"] {
            assert!(
                sql39.contains(&format!(
                    "CREATE TRIGGER exchange_holidays_stale_snapshots_{op} "
                )),
                "0039 must re-create exchange_holidays' {op} staleness trigger"
            );
        }
        let flat39 = sql39.split_whitespace().collect::<Vec<_>>().join(" ");
        for col in ["id", "mic", "holiday_date", "name"] {
            assert_eq!(
                flat39.matches(&format!("'{col}', OLD.{col}")).count(),
                2,
                "both exchange_holidays triggers must record {col}"
            );
        }

        // 0040 added listing_renames.old_name/old_price_symbol (SCENARIOS
        // R-04/R-08 — what the rename overwrote, so its undo can put it
        // back), so that table's trigger pair was dropped and re-created with
        // them: the *live* pair now comes from 0040 and must still carry
        // every earlier column too.
        let sql40 = include_str!("../../migrations/0040_listing_rename_old_name_and_symbol.sql");
        for op in ["update", "delete"] {
            assert!(
                sql40.contains(&format!("CREATE TRIGGER listing_renames_row_history_{op} ")),
                "0040 must re-create the listing_renames {op} trigger"
            );
        }
        let flat40 = sql40.split_whitespace().collect::<Vec<_>>().join(" ");
        for col in [
            "id",
            "listing_id",
            "effective_date",
            "old_ticker",
            "new_ticker",
            "old_exchange_mic",
            "new_exchange_mic",
            "old_name",
            "old_price_symbol",
            "note",
        ] {
            assert_eq!(
                flat40.matches(&format!("'{col}', OLD.{col}")).count(),
                2,
                "both re-created listing_renames triggers must record {col}"
            );
        }

        // 0029 widened income_type's CHECK for `OtherIncome` (SCENARIOS
        // L-03/L-04), which meant rebuilding income — so the *live* income
        // trigger pair comes from 0029, along with the three staleness
        // triggers and five indexes that moved with the renamed table. Every
        // column must still be recorded: a column the trail drops is a version
        // that cannot be reconstructed.
        let sql29 = include_str!("../../migrations/0029_income_type_other_income.sql");
        assert!(
            sql29.contains("'Dividend', 'EmploymentIncome', 'OtherIncome'"),
            "0029 must widen the income_type CHECK"
        );
        for op in ["update", "delete"] {
            assert!(
                sql29.contains(&format!("CREATE TRIGGER income_row_history_{op} ")),
                "0029 must re-create the income {op} trigger"
            );
        }
        for op in ["insert", "update", "delete"] {
            assert!(
                sql29.contains(&format!("CREATE TRIGGER income_stale_snapshots_{op} ")),
                "0029 must re-create income's {op} staleness trigger"
            );
        }
        for index in [
            "income_date_paid",
            "income_listing_id",
            "income_reinvestment_trade_id",
            "income_buyback_trade_id",
            "income_holding_account_id",
        ] {
            assert!(
                sql29.contains(&format!("CREATE INDEX {index} ")),
                "0029 must re-create {index}"
            );
        }
        let flat29 = sql29.split_whitespace().collect::<Vec<_>>().join(" ");
        for col in [
            "id",
            "listing_id",
            "date_paid",
            "ex_date",
            "franked_amount",
            "unfranked_amount",
            "foreign_source_income",
            "foreign_tax_paid",
            "tfn_withholding_tax",
            "franking_credits",
            "lic_capital_gain_amount",
            "conduit_foreign_income",
            "trust_income",
            "reinvestment_trade_id",
            "currency",
            "buyback_trade_id",
            "holding_account_id",
            "amount_per_security",
            "securities_held",
            "entitlement_date",
            "tax_deferred_amount",
            "income_type",
        ] {
            assert_eq!(
                flat29.matches(&format!("'{col}', OLD.{col}")).count(),
                2,
                "both re-created income triggers must record {col}"
            );
        }
        // The rebuild runs with foreign keys off, outside a transaction, so
        // SQLite cannot repoint attachments' income_id at the renamed table —
        // the pragma pair and the directive are what make that true.
        assert!(
            sql29.starts_with("-- no-transaction"),
            "0029 must run outside a transaction to change PRAGMA foreign_keys"
        );
        assert!(sql29.contains("PRAGMA foreign_keys = OFF;"));
        assert!(sql29.contains("PRAGMA foreign_keys = ON;"));
        assert!(sql29.contains("BEGIN;") && sql29.contains("COMMIT;"));
    }

    /// The `json_object(...)` key list of one trigger's SQL: its keys are
    /// the quoted strings inside that call, each paired with an `OLD.<col>`
    /// value. Only the text between the call's own parentheses is scanned —
    /// the `INSERT INTO row_history ... VALUES ('<table>', OLD.id, ...)`
    /// wrapped around it carries a quoted-string/`OLD.`-value pair of its
    /// own, and that one is the table name, not a column.
    fn json_object_keys(sql: &str) -> Vec<String> {
        let open = sql
            .find("json_object(")
            .expect("a row-history trigger builds its snapshot with json_object(")
            + "json_object(".len();
        let mut depth = 1usize;
        let mut close = None;
        for (i, c) in sql[open..].char_indices() {
            match c {
                '(' => depth += 1,
                ')' => {
                    depth -= 1;
                    if depth == 0 {
                        close = Some(open + i);
                        break;
                    }
                }
                _ => {}
            }
        }
        let mut args = &sql[open..close.expect("unbalanced json_object( in trigger body")];
        let mut keys = Vec::new();
        while let Some(quote) = args.find('\'') {
            let after = &args[quote + 1..];
            let end = after
                .find('\'')
                .expect("unterminated quoted json_object key");
            keys.push(after[..end].to_string());
            args = &after[end + 1..];
        }
        keys
    }

    /// Every column of every audited table must be recorded by *both* of that
    /// table's `*_row_history_*` triggers: a column the trail drops is a
    /// version that cannot be reconstructed, and the drop is silent — the
    /// trail keeps looking healthy while the column's prior values are simply
    /// never written.
    ///
    /// Everything here is derived from the live schema (`PRAGMA table_info`
    /// against the triggers' own `json_object` keys), so this **supersedes
    /// the bespoke per-migration column assertions in
    /// `audited_tables_match_migration_check_and_triggers` for future
    /// migrations**: an `ALTER TABLE ... ADD COLUMN` on an audited table that
    /// forgets to DROP and re-CREATE that table's trigger pair fails here,
    /// and the next migration's author does not need to hand-write another
    /// assertion of their own. (The ones already written stay — they pin
    /// something this cannot: *which* migration the live pair came from.)
    #[tokio::test]
    async fn every_audited_column_is_recorded_by_both_triggers() {
        // The one documented exclusion: `attachments.content` is a BLOB, and
        // a json_object cannot hold one (migration 0013's header says so).
        const EXCLUDED: [(&str, &str); 1] = [("attachments", "content")];

        let pool = test_pool().await;
        for table in AUDITED_TABLES {
            let columns: Vec<String> = sqlx::query_scalar("SELECT name FROM pragma_table_info(?)")
                .bind(table)
                .fetch_all(&pool)
                .await
                .unwrap();
            assert!(!columns.is_empty(), "{table} is not in the live schema");

            for op in ["update", "delete"] {
                let trigger = format!("{table}_row_history_{op}");
                let sql: Option<String> = sqlx::query_scalar(
                    "SELECT sql FROM sqlite_master WHERE type = 'trigger' AND name = ?",
                )
                .bind(&trigger)
                .fetch_optional(&pool)
                .await
                .unwrap();
                // A missing trigger is a failure, not a table to skip over:
                // an unaudited audited table records nothing at all.
                let sql = sql.unwrap_or_else(|| panic!("the live schema has no {trigger} trigger"));
                let keys = json_object_keys(&sql);
                for column in &columns {
                    if EXCLUDED.contains(&(table, column.as_str())) {
                        continue;
                    }
                    assert!(
                        keys.contains(column),
                        "{trigger} does not record {table}.{column} — the migration \
                         that added the column must DROP and re-CREATE both \
                         {table}_row_history_* triggers with the new column list"
                    );
                }
            }
        }
    }

    /// The live schema with `--` comments stripped and whitespace collapsed,
    /// so a column definition can be matched as one string however the DDL
    /// lays it out (several tables carry a comment above their `id`).
    fn table_ddl(sql: &str) -> String {
        sql.lines()
            .map(|line| match line.find("--") {
                Some(at) => &line[..at],
                None => line,
            })
            .collect::<Vec<_>>()
            .join(" ")
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
    }

    /// Every audited table's surrogate key must be `AUTOINCREMENT`, so the
    /// database never re-issues the id of a deleted row: `row_history` keys a
    /// trail on `(table_name, row_id)`, and a re-issued id hands the new
    /// occupant every entry the previous one left (SCENARIOS U-a — live trade
    /// 9072, a 2025 share sale, became the LAC demerger's 2023 closing Sell).
    /// A plain `INTEGER PRIMARY KEY` is an alias for the rowid, which SQLite
    /// re-uses from the largest freed one.
    ///
    /// Derived from the live schema and [`AUDITED_TABLES`], so a **new**
    /// audited table cannot be added without it: the migration must declare
    /// `id INTEGER PRIMARY KEY AUTOINCREMENT` (0021, 0031, 0039 and 0045 did
    /// it for the 20 that came before), or the table must earn one of the two
    /// exemptions below, each of which is checked here rather than skipped.
    #[tokio::test]
    async fn every_audited_tables_id_is_autoincrement() {
        /// The two audited tables with no server-assigned surrogate id, and
        /// what makes each safe. Both are checked, not waved through: a table
        /// that stops satisfying its reason fails here.
        enum Exempt {
            /// `tax_year_settings` is keyed on the financial year itself
            /// (migration 0027) — a natural key naming one taxpayer-year fact
            /// forever, so re-entering it after a delete is the *same* fact
            /// and rightly inherits that year's trail. There is no surrogate
            /// id to make `AUTOINCREMENT`, which is also why
            /// `AuditedTable::key_is_reusable` is false for it.
            NaturalKey,
            /// `cgt_settings` is `id INTEGER PRIMARY KEY CHECK (id = 1)`: a
            /// singleton whose CHECK pins the only id there can be, so
            /// re-creating its row is re-entry of the same fact, not reuse.
            Singleton,
        }
        const EXEMPT: [(&str, Exempt); 2] = [
            ("tax_year_settings", Exempt::NaturalKey),
            ("cgt_settings", Exempt::Singleton),
        ];

        let pool = test_pool().await;
        for table in AUDITED_TABLES {
            let sql: String = sqlx::query_scalar(
                "SELECT sql FROM sqlite_master WHERE type = 'table' AND name = ?",
            )
            .bind(table)
            .fetch_one(&pool)
            .await
            .unwrap_or_else(|_| panic!("{table} is not in the live schema"));
            let ddl = table_ddl(&sql);
            match EXEMPT.iter().find(|(name, _)| *name == table) {
                Some((_, Exempt::NaturalKey)) => {
                    let columns: Vec<String> =
                        sqlx::query_scalar("SELECT name FROM pragma_table_info(?)")
                            .bind(table)
                            .fetch_all(&pool)
                            .await
                            .unwrap();
                    assert!(
                        !columns.iter().any(|c| c == "id"),
                        "{table} is exempt because it has no surrogate id, but it has one now — \
                         make it AUTOINCREMENT or drop the exemption"
                    );
                }
                Some((_, Exempt::Singleton)) => assert!(
                    ddl.contains("id INTEGER PRIMARY KEY CHECK (id = 1)"),
                    "{table} is exempt because a CHECK pins it to the single id 1, and it no \
                     longer does: {ddl}"
                ),
                None => assert!(
                    ddl.contains("id INTEGER PRIMARY KEY AUTOINCREMENT"),
                    "{table} is audited, so its id must be INTEGER PRIMARY KEY AUTOINCREMENT — a \
                     plain INTEGER PRIMARY KEY re-uses the largest freed rowid, handing the new \
                     row the deleted one\u{2019}s row_history trail: {ddl}"
                ),
            }
        }
    }

    /// The migration is purely additive: it creates the trail and its
    /// triggers but touches no existing row — line-level pin that no
    /// statement alters, drops, updates, deletes, or inserts into anything
    /// but row_history (the global no-DROP/no-REAL migration tests apply on
    /// top of this).
    #[test]
    fn migration_0013_preserves_existing_data() {
        let sql = include_str!("../../migrations/0013_row_history.sql");
        for line in sql.lines() {
            let trimmed = line.trim();
            if trimmed.starts_with("--") {
                continue;
            }
            let upper = trimmed.to_uppercase();
            for forbidden in ["ALTER ", "DROP ", "UPDATE ", "DELETE FROM"] {
                assert!(
                    !upper.starts_with(forbidden),
                    "0013 must not start a {forbidden} statement: {trimmed}"
                );
            }
            if upper.starts_with("INSERT INTO") {
                assert!(
                    upper.starts_with("INSERT INTO ROW_HISTORY"),
                    "0013 may only insert into row_history: {trimmed}"
                );
            }
        }
    }
}
