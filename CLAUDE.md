# Rules
- Only mark a TODO item done when a test exists and passes for it
- Never delete a TODO item

# Project structure
- `src/main.rs` — server startup: init pool, spawn backup, mount routers, graceful shutdown
- `src/db.rs` — pool init (runs migrations), daily backup
- `src/args.rs` — CLI args: `--db` (default `share-tracker.db`), `--port` (default 3000)
- `src/logging.rs` — tracing subscriber init; reads `RUST_LOG`, defaults to `info`
- `src/<entity>.rs` — one file per domain entity (see pattern below)
- `migrations/NNNN_description.sql` — sqlx migrations, applied once in order by `db::init()`

# Domain entity module pattern
Each entity in `src/<entity>.rs` follows this structure:
1. Model struct — derives `Serialize`, `Deserialize`, `sqlx::FromRow`
2. Input body struct — for PUT endpoints; omits the primary key (comes from URL path)
3. `pub db_*` functions — `db_list`, `db_get`, `db_upsert`, `db_delete`
4. Private axum handlers — call the `db_*` functions, map errors to `StatusCode`
5. `pub fn router() -> Router<SqlitePool>` — registers the entity's routes
6. Inline `#[cfg(test)]` module — DB-level tests and API-level tests via `tower::ServiceExt::oneshot`

New entity modules must be declared in `main.rs` (`mod <entity>;`) and the router mounted with `.merge(entity::router().with_state(pool.clone()))`.

# API conventions
- `GET  /entities`      → 200 JSON array
- `GET  /entities/:id`  → 200 JSON or 404
- `PUT  /entities/:id`  → 204 No Content (upsert)
- `DELETE /entities/:id` → 204 No Content or 404

# Test conventions
- `test_pool()` creates an in-memory DB via `db::init(":memory:")` — migrations and seed data are included
- API tests use `router().with_state(pool).oneshot(request)` — no network, no port binding
- DB tests call `db_*` functions directly against the pool
