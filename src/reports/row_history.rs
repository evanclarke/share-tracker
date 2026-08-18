//! Row-history inspection: the read side of the append-only audit trail
//! (migration `0013_row_history.sql`; aligns with the ATO record-keeping
//! guidance mirrored in `docs/ato/cgt-keeping-records-shares.md`).
//!
//! Database triggers record the prior row on every UPDATE and DELETE of an
//! audited table into `row_history`; this report returns one row's entries so
//! an accidental edit to a historical fact can be noticed and reconstructed.
//! Read-only — the trail itself is written by the triggers alone and is
//! append-only (enforced in the schema), so there is nothing here to write.

use crate::infra::http::ApiError;
use axum::{Json, Router, extract::State, routing::post};
use serde::Deserialize;
use serde_json::{Map, Value};
use sqlx::{Row, SqlitePool};

/// The audited tables, exactly as migration 0013 enumerates them in the
/// `row_history.table_name` CHECK and its per-table trigger pairs — a test
/// pins the three lists to each other, and the web UI's table picker is
/// asserted against this list too. Three joined later: `listing_renames`
/// (0018), `closing_prices` (0021, once 0020 made a price hand-enterable) and
/// `tax_year_settings` (0027), each migration rebuilding `row_history` to
/// extend the CHECK.
pub const AUDITED_TABLES: [&str; 20] = [
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
];

#[derive(Debug, Deserialize)]
pub struct RowHistoryRequest {
    /// One of [`AUDITED_TABLES`]; anything else is rejected 422.
    pub table: String,
    /// The audited row's `id` — for `tax_year_settings`, whose identity *is*
    /// the financial year, that year. A row with no recorded history (never
    /// updated or deleted since the trail began) returns an empty array.
    pub row_id: i64,
}

pub fn router() -> Router<SqlitePool> {
    Router::new().route("/reports/row_history", post(report))
}

/// One (table, row)'s audit entries, newest first. Each entry flattens the
/// stored old-row JSON behind its own three fields (`history_id`,
/// `operation`, `changed_at`), so a set of entries renders as one table
/// whose remaining columns are the audited table's own — including `id`,
/// which is the audited row's id (= the request's `row_id`).
pub async fn db_row_history(
    pool: &SqlitePool,
    table: &str,
    row_id: i64,
) -> Result<Vec<Map<String, Value>>, sqlx::Error> {
    let rows = sqlx::query(
        "SELECT id, operation, changed_at, old_row FROM row_history \
         WHERE table_name = ? AND row_id = ? ORDER BY id DESC",
    )
    .bind(table)
    .bind(row_id)
    .fetch_all(pool)
    .await?;

    rows.iter()
        .map(|row| {
            let mut entry = Map::new();
            entry.insert("history_id".into(), row.try_get::<i64, _>("id")?.into());
            entry.insert(
                "operation".into(),
                row.try_get::<String, _>("operation")?.into(),
            );
            entry.insert(
                "changed_at".into(),
                row.try_get::<String, _>("changed_at")?.into(),
            );
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

async fn report(
    State(pool): State<SqlitePool>,
    Json(req): Json<RowHistoryRequest>,
) -> Result<Json<Vec<Map<String, Value>>>, ApiError> {
    if !AUDITED_TABLES.contains(&req.table.as_str()) {
        return Err(ApiError::Unprocessable(format!(
            "'{}' is not an audited table (one of: {})",
            req.table,
            AUDITED_TABLES.join(", ")
        )));
    }
    db_row_history(&pool, &req.table, req.row_id)
        .await
        .map(Json)
        .map_err(ApiError::from)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::entities::{sell, trade};
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
                    "INSERT INTO closing_prices (id, listing_id, price_date, price, source, fetched_at, status, origin) VALUES (19, 1, '2024-01-02', '10', 'yahoo', '2024-01-02T08:00:00Z', 'ok', 'fetched')",
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
        // listing_renames (0018) and closing_prices (0021) postdate 0013, and
        // are checked below — every other table was audited from the start.
        let tables_as_of_0013 = AUDITED_TABLES.into_iter().filter(|&t| {
            t != "listing_renames" && t != "closing_prices" && t != "tax_year_settings"
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
