# Rules
- Only mark a TODO item done when a test exists and passes for it
- Never delete a TODO item
- Solo project: commit directly to `main`. Don't create feature branches for commits
- `cargo build` and `cargo test` must both be warning-free before a task is done (a `pub` item only reached from `#[cfg(test)]` code warns in the non-test build — gate it `#[cfg(test)]` or remove it)
- Implement a requirement fully: if the spec says "apply the 50% CGT discount", compute the discounted figure — don't stop at an eligibility flag/indicator. If only partially done, leave it unchecked in TODO with a note on what remains
- Recurring background tasks (weekly backup, weekly RBA FX import, monthly imports, future jobs) run via the cron scheduler in `infra/scheduler.rs`: register the work in `registry()` and add a line to `schedule.cron`. The scheduler logs the next scheduled run time at INFO after every run (and at startup) — its `next run scheduled` line — so the schedule is verifiable from logs without reading code
- Keep `README.md` in sync when the data model or HTTP API changes: a new/changed table or column updates the Database schema (and Relationships) section; a new/changed/removed endpoint, status code, or request/response shape updates the HTTP API and Response codes sections. README updates are part of the same task, not a follow-up

# Financial correctness
- ATO reference guidance is mirrored in `docs/` (CGT calculation, the 50% discount and loss-netting order, cost base, AMIT/AMMA cost-base adjustments incl. CGT event E10, LIC deduction, managed-fund income components). Read `docs/OVERVIEW.md` first — it indexes each file and maps it to the relevant calculation. Consult these before implementing or changing any tax rule; the live ATO site (ato.gov.au) is authoritative if a doc looks stale
- Money and quantities are always `Decimal`, never `f64`. New monetary columns are `TEXT`; migrations must preserve precision (never round-trip a value through a `REAL` column)
- When reading a `TEXT` decimal column, propagate parse failures (map to `sqlx::Error::Decode`, as the `FromRow` `dec` helpers do). Never `.parse().unwrap_or(Decimal::ZERO)` — a silent zero corrupts financial output without failing
- Reports take the Australian-tax view: cost base, proceeds, and income totals are in AUD. Convert every non-AUD amount to AUD using the record's `fx_rate` before aggregating or comparing — never mix currencies in one calculation
- Market settlement (T+n) counts business days — skip weekends and the exchange's seeded public holidays (`exchange_holidays`, looked up per the listing's exchange via `exchange_holiday::exchange_holidays_for_listing`); never just add calendar days

# Data integrity
- Enforce data-model invariants at write time inside a transaction, not only in reports. A multi-row invariant (e.g. a Sell's parcel allocations must sum to its quantity) must be validated and committed atomically so a partial/invalid state can never be persisted; reject with `422` otherwise. If standalone child-entity writes could reintroduce a bad state, restrict them
- Every field in the data model must be used by a calculation or endpoint, or carry a comment marking it informational-only. Don't leave stored fields silently unused

# Project structure
Modules are grouped into three folders; `main.rs`, `app.rs`, and the migrations live at the `src` root.
- `src/main.rs` — server startup: init pool, build registry, spawn scheduler, serve, graceful shutdown
- `src/app.rs` — `app::router(pool, registry)` assembles entity + report + scheduler routers (testable without `main`)
- `src/infra/` — cross-cutting infrastructure (`mod.rs` re-exports each):
  - `infra/db.rs` — pool init (runs migrations), weekly DB backup (`backup`/`backup_path`)
  - `infra/args.rs` — CLI args: `--db` (default `share-tracker.db`), `--port` (default 3000), `--schedule`
  - `infra/logging.rs` — tracing subscriber init; reads `RUST_LOG`, defaults to `info`
  - `infra/decimal.rs` — `parse_dec` and `FromRow` decimal helpers
  - `infra/fx.rs` — `to_aud` AUD conversion via the ATO/RBA reference rate (per-trade `fx_rate` fallback); reports use it to convert non-AUD cost base/proceeds
  - `infra/scheduler.rs` — maintenance-job registry, cron parsing/spawn, inspection routes
- `src/entities/<entity>.rs` — one file per domain entity, CRUD + write-time invariants (see pattern below). `entities::router()` (in `entities/mod.rs`) merges them all
- `src/reports/<report>.rs` — read-only AUD aggregations (portfolio, realised/unrealised gains, tax summary). `reports::router()` (in `reports/mod.rs`) merges them all
- `src/web.rs` + `src/web/{index.html,app.js,style.css}` — the web frontend: a no-build-step single-page app (plain HTML/CSS/JS) embedded with `include_str!` and served by `web::router()` (`GET /`, `/static/app.js`, `/static/style.css`), merged in `app::router`. `app.js` is config-driven — each entity is described once and generic list/form code drives the existing JSON API; do not add a parallel data path. When you add/change an entity, report, or its fields, update the matching `ENTITIES`/`REPORTS` config entry in `app.js`. UI items are tested by asserting the view + the API path it drives appears in the served bundle (no browser harness)
- `migrations/NNNN_description.sql` — sqlx migrations, applied once in order by `infra::db::init()`

# Domain entity module pattern
Each entity in `src/entities/<entity>.rs` follows this structure:
1. Model struct — derives `Serialize`, `Deserialize`, `sqlx::FromRow`
2. Input body struct — for PUT endpoints; omits the primary key (comes from URL path)
3. `pub db_*` functions — `db_list`, `db_get`, `db_upsert`, `db_delete`
4. Private axum handlers — call the `db_*` functions, map errors to `StatusCode`
5. `pub fn router() -> Router<SqlitePool>` — registers the entity's routes
6. Inline `#[cfg(test)]` module — DB-level tests and API-level tests via `tower::ServiceExt::oneshot`

New entity modules are added by dropping the file in `src/entities/` and adding one `pub mod <entity>;` line plus one `.merge(<entity>::router())` line in `src/entities/mod.rs` — `main.rs` and `app.rs` don't change. Reports follow the same pattern in `src/reports/mod.rs`.

# API conventions
- `GET  /entities`      → 200 JSON array
- `GET  /entities/:id`  → 200 JSON or 404
- `PUT  /entities/:id`  → 204 No Content (upsert)
- `DELETE /entities/:id` → 204 No Content or 404

# Test conventions
- `test_pool()` creates an in-memory DB via `db::init(":memory:")` — migrations and seed data are included
- API tests use `router().with_state(pool).oneshot(request)` — no network, no port binding
- DB tests call `db_*` functions directly against the pool
