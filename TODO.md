# TODO

Items are only marked done when a passing test exists for them.

Completed and decided (out-of-scope / not-reproducible) sections are archived in [DONE.md](DONE.md) to keep this list focused on active work. When a section here is fully done, move it to DONE.md rather than leaving it — see CLAUDE.md.

## Code quality (from 2026-06-10 code review)

High value:

- [ ] Extract a shared adjusted-cost-base module: the pipeline (initial cost → AMIT reduction floored at nil / E10 → return-of-capital per-unit reduction → split re-basing → AUD conversion at acquisition month) is independently re-implemented in `reports/realised_gains.rs` (~212–250), `reports/open_parcels.rs` (~128–140), `reports/portfolio.rs` (~150–165), and `reports/unrealised_gains.rs` (~141–155). One `domain`/`cost_base` function with the ATO citations on it, called by every report — divergence between copies is the biggest correctness risk in the codebase. The `ato_examples.rs` suite is the safety net for this refactor
- [ ] Stop swallowing errors behind bare 500s: `map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)` appears ~62 times across 30 files (e.g. `reports/realised_gains.rs:314`) — a decode failure that `parse_dec` carefully named never reaches the logs. Introduce a shared `ApiError` in `infra/http.rs` implementing `IntoResponse` that logs internal variants via `tracing::error!` and carries 422 detail messages; per-entity error enums stay (they're good docs) and gain `impl From<EntityError> for ApiError`, replacing the hand-written 50-line match in each handler
- [ ] Add `cargo fmt --check` to CI (ci.yml) and the pre-push hook, and fix the existing fmt drift (`cargo fmt --check` currently fails, e.g. `src/ato_examples.rs:164`)

Medium value:

- [ ] Fix the FX N+1 in report loops: `db_realised_gains` calls `infra::fx::to_aud(pool, …)` twice per allocation (one DB round-trip each). Pre-load `rba_fx_rates` into a `HashMap<(currency, month), Decimal>` before the loop — the real win is the gain/loss computation becoming a pure function over in-memory data (unit-testable without a pool, and a natural seam for the cost-base extraction above)
- [ ] Run multi-query reports on one read transaction/connection: `db_realised_gains` reads sells, buys, allocations, AMIT reductions, and corporate actions in separate pool queries, so an interleaved write yields an inconsistent snapshot (an allocation whose sell is missing from `sell_map` is silently skipped at ~line 176). Wrap report reads in a single `pool.begin()`
- [ ] Cut manual row mapping in reports: local structs like `SellInfo`/`BuyInfo` in `realised_gains.rs` (~119–157) are built field-by-field with `try_get` + `parse_dec`; derive `FromRow` using the `infra/decimal.rs` helpers instead. Optionally evaluate sqlx compile-time `query!` macros with offline mode (`.sqlx` prepare step) for SQL validated at build time
- [ ] Shared test fixtures: nearly every test module re-defines `test_pool`/`insert_listing`/`insert_buy` (compare `entities/sell.rs:625–699` with `reports/realised_gains.rs:325+`); the `insert_buy` in sell.rs initialises 25 `Trade` fields, so every new column touches ~25 test modules. Add a crate-level `#[cfg(test)] mod test_support` with builder-style fixtures

Smaller:

- [ ] Pro-rating remainders: per-allocation brokerage shares (`sale_costs * qty_alloc / sale.quantity`, `realised_gains.rs:186`) may not sum exactly to the total. Sub-cent today, but if rows are ever rounded to cents for display/export, assign the remainder to the last allocation rather than rounding each independently
- [ ] Split `web/app.js` (2,445 lines, one file) into native ES modules (`<script type="module">`) — config (`ENTITIES`/`REPORTS`/`ACTIONS`) separate from the generic rendering engine; no build step needed
- [ ] Trim `tokio` features in Cargo.toml from `full` to what the server uses (`rt-multi-thread`, `macros`, `signal`, `net`, `time`, `fs`) for slightly faster builds

