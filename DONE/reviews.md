# Done — Code & Design Review Findings

## Review Findings — Requirements Gaps
(FX source changed: conversion now uses the ATO FX Rate lookup, not a per-trade `fx_rate` — see the FX Conversion section.)
- [x] Convert cost base and proceeds to AUD for non-AUD trades via the ATO FX Rate lookup — portfolio + unrealised convert each parcel's cost base; realised converts proceeds (sale month rate) and cost base (purchase month rate) per allocation, all via `infra::fx::to_aud`. Supplied market prices are documented as AUD so market value/gain stay AUD
- [x] Convert income/AMMA foreign amounts to AUD via the ATO FX Rate lookup so tax summary totals are in AUD — migration `0014_income_amma_currency.sql` adds a `currency TEXT NOT NULL DEFAULT 'AUD'` column to `income` and `amma_statements` (defaults preserve existing AUD pass-through). `tax_summary::aud_field` converts every aggregated income/AMMA amount via `infra::fx::to_aud` using the record's `currency` and the month of `date_paid` (income) / `tax_year_end_date` (AMMA). These records carry no manual `fx_rate`, so `manual_override = None`: a non-AUD amount with no ATO rate fails loudly (decode error → 500), never passed through or zeroed. Tests: `tax_summary::tests::db_usd_income_converted_to_aud_via_ato_rate`, `db_usd_amma_converted_to_aud_via_ato_rate`, `db_non_aud_without_ato_rate_fails_loudly`
- [x] Tests: USD (XNYS) buy/sell produces AUD cost base and gain using the ATO FX Rate (`realised_gains::tests::db_usd_buy_sell_produces_aud_cost_base_and_gain_via_ato_rate`, plus `db_usd_falls_back_to_manual_fx_rate_when_no_ato_rate`)

## Review Findings — Needs Clarification (resolve intended behaviour before implementing)
- [x] Confirm how the 50% CGT discount should be applied: reports currently expose only the gross eligible gain and never halve it, nor net losses against gains. Decide whether the tool should compute the discounted/assessable net capital gain or continue exposing components only — RESOLVED: the tool computes the assessable net capital gain (in the new `/portfolio/net-capital-gain` report), while the per-sale realised-gains report keeps exposing gross buckets (`discount_eligible_gain`, `non_discountable_gain`, `capital_loss`). The discount/netting is a portfolio-year concern, not per-sale, because losses net across all gains before the discount is applied
- [x] Apply the 50% CGT discount to eligible gains (currently only the gross eligible gain is exposed, never halved) — done in `/portfolio/net-capital-gain`: `cgt_discount = net_discount_eligible_gain / 2` and `net_capital_gain` includes only the halved eligible gain. AMMA `cgt_discount_gains` is grossed up ×2 first (it is the already-halved statement line). Tests: `net_capital_gain::tests::db_discount_eligible_gain_is_halved`, `db_amma_discount_gains_grossed_up_then_halved`
- [x] Net capital losses against gains and apply the discount to produce an assessable net capital gain — done in `/portfolio/net-capital-gain`: realised losses + AMMA `capital_losses_applied` are applied against non-discountable gains first, then discount-eligible gains (ATO-optimal, per the design decision), then the remaining eligible gain is halved; unused losses become `capital_loss_carried_forward`
- [x] Tests: discounted net capital gain after offsetting losses — `net_capital_gain::tests::db_losses_applied_to_non_discount_gains_first` (loss hits non-discountable gain first; eligible gain still halved), `db_losses_spill_into_discount_gains_then_carry_forward` (loss exhausts gains → NCG 0, excess carried forward), `db_amma_indexation_other_gains_and_losses`
- [x] Confirm AMMA cost-base driver: the model annotates `tax_deferred_amount` as "reduces cost base", but calculations use the per-unit `cost_base_adjustment`; `tax_deferred_amount` and `tax_free_amount` are currently stored but unused. Decide which field(s) drive the adjustment — RESOLVED per ATO guidance (`docs/ato/amit-cost-base-adjustments.md`): for an AMIT the cost base is adjusted by the single **AMIT cost base net amount** stated on the AMMA statement (the per-unit `cost_base_adjustment` field). Tax-deferred and tax-free amounts are NOT direct cost-base drivers — the ATO says they are only "broadly reflected" in that net amount. So `cost_base_adjustment` is the driver; `tax_deferred_amount`/`tax_free_amount` are informational-only.
- [x] Resolve AMMA cost-base driver per the decision above and remove or wire up the now-redundant fields — kept `cost_base_adjustment` as the sole driver (already consumed by `amit_adjustment::db_cost_base_reductions`, used by the portfolio/realised/unrealised reports; a positive value reduces cost base, a negative value increases it per the AMIT regime). `tax_deferred_amount` and `tax_free_amount` retained as legitimate AMMA statement line items but documented informational-only (doc comments in `amma.rs`, README schema annotations) so they aren't flagged as silently-unused fields. Not removed (they are real statement components and CI forbids DROP COLUMN). CGT event E10 (a net cost-base reduction exceeding the remaining cost base → immediate capital gain) is now modelled — see the Cost Base Adjustments section.
- [x] Tests: cost base reflects the agreed AMMA field(s) — `amit_adjustment::tests::db_cost_base_reduction_ignores_tax_deferred_and_tax_free` locks in that the reduction is driven solely by `cost_base_adjustment` (5.00 = 100 units × 0.05) and is unaffected by large `tax_deferred_amount`/`tax_free_amount` lines; existing `db_cost_base_reduction_calculation` covers multi-statement summation

## Review Findings — Feature Gaps
- [x] Net capital gain / overall tax-position report combining realised parcel gains, AMMA-attributed CGT gains, and AMMA capital losses applied — `GET /portfolio/net-capital-gain` (`src/reports/net_capital_gain.rs`), one record per AU financial year. Buckets the year's gross gains into discount-eligible (realised parcels held >12mo + AMMA `cgt_discount_gains` grossed up ×2, since that field is the already-halved "discounted capital gain" line) and non-discountable (realised parcels held ≤12mo + AMMA `cgt_indexation_gains` + `cgt_other_gains`), totals capital losses (realised losses + AMMA `capital_losses_applied`), applies losses ATO-optimally (non-discountable gains first, then discount-eligible), then halves the remaining eligible gain → assessable `net_capital_gain` (+ `capital_loss_carried_forward` for the current-year excess; prior-year carried-forward losses not modelled). Realised-gains report extended with `non_discountable_gain` + `capital_loss` buckets (identity `capital_gain_loss == discount_eligible_gain + non_discountable_gain − capital_loss`) so the parcel-level classification lives in one place. AMMA amounts converted to AUD via the ATO rate (non-AUD with no rate fails loudly → 500). Tests: `net_capital_gain::tests` (discount halved, short-term not discounted, losses applied non-discount-first / spill-to-discount-then-carry-forward, AMMA grossed-up/indexation/other/losses, realised+AMMA combined in one year, FX conversion + fail-loudly, sorted by tax year, API), plus realised-gains bucket assertions
- [x] Prevent an under-allocated Sell from being persisted: new atomic `PUT /sells/{id}` (src/sell.rs) inserts the Sell trade + all its parcel allocations in one transaction and rejects (422) unless allocations sum exactly to the sell quantity and every parcel is a valid, not-over-allocated Buy/DRP. To keep the invariant: `PUT /trades/{id}` now rejects `Sell` (422), and `parcel_allocations` is read-only over HTTP (PUT/DELETE removed; allocations are managed via /sells).
- [x] Tests: Sell+allocations rejected when allocations don't sum to sell quantity (under and over), rejected on parcel over-allocation / non-Buy parcel, accepted and rolled-back-on-failure when valid; PUT /trades Sell → 422; parcel_allocations PUT/DELETE → 405 (src/sell.rs, src/trade.rs, src/parcel_allocation.rs)

## Review Findings — Implementation Issues
- [x] Settlement date should advance by business days, not calendar days (trade.rs `add_business_days` now skips weekends; public holidays still not modelled)
- [x] Tests: settlement date skips weekends (`add_business_days_skips_weekend`, `api_settlement_date_auto_populated_skips_weekend`)
- [x] Model exchange public holidays so settlement dates skip them too — new `exchange_holidays` table (`(mic, holiday_date)` PK, FK→exchanges; migration `0003_exchange_holidays.sql`) seeded with the published NYSE + ASX full-closure calendars for 2024–2027. `trade::add_business_days` now also skips holidays; `exchange_holiday::exchange_holidays_for_listing` loads the set for a listing's exchange, used by both `/trades` and `/sells` settlement auto-population. Full CRUD at `/exchange_holidays` (`src/entities/exchange_holiday.rs`). Tests: `trade::tests::add_business_days_skips_public_holidays`, `api_settlement_date_skips_public_holiday`, plus `exchange_holiday::tests` (seed/CRUD/FK/holidays-for-listing)
- [x] Avoid CAST(REAL AS TEXT) float imprecision for any rows written before migration 0006 (trades/income were created as REAL in 0004/0005) — resolved structurally by consolidating the incremental migrations into a single baseline schema (`0001_schema.sql`) + seed (`0002_seed.sql`). The schema has no REAL columns and no REAL→TEXT conversion, so the float-imprecision path no longer exists; every monetary/quantity column is TEXT (arbitrary-precision Decimal) from creation. Existing data is migrated once manually by the operator (the consolidated `0001` has a different checksum than the old chain, so an in-place upgrade is not supported — start from a fresh DB and import). The earlier runtime canonicalization pass (`canonicalize_pre0006_decimals`/`canonicalize_decimal`) was removed as no longer needed. Guarded by `db::tests::migrations_store_decimals_as_text_never_real`, which fails if any migration reintroduces a REAL column or a `CAST(... AS TEXT)`.
- [x] Surface malformed decimal values instead of silently coercing to zero via `.parse().unwrap_or(Decimal::ZERO)` in the report modules (shared `decimal::parse_dec` propagates a decode error; test `db_malformed_decimal_is_an_error_not_zero`)
- [x] Remove dead-code warnings from unused `UpsertError::Db(sqlx::Error)` field in parcel_allocation.rs and amit_adjustment.rs (now logged via `tracing::error!`)
- [x] Remove remaining dead-code warning: replaced the unused per-trade `amit_adjustment::db_cost_base_reduction` with a shared bulk `db_cost_base_reductions` (returns `HashMap<trade_id, Decimal>`); portfolio/realised/unrealised reports now call it instead of each re-implementing the AMIT-reduction query inline. Also fixes the silent `.parse().unwrap_or(Decimal::ZERO)` (now propagates via `parse_dec`). Test `db_cost_base_reduction_calculation` exercises the bulk helper.

## Code quality (from 2026-06-10 code review)

High value:

- [x] Extract a shared adjusted-cost-base module: the pipeline (initial cost → AMIT reduction floored at nil / E10 → return-of-capital per-unit reduction → split re-basing → AUD conversion at acquisition month) is independently re-implemented in `reports/realised_gains.rs` (~212–250), `reports/open_parcels.rs` (~128–140), `reports/portfolio.rs` (~150–165), and `reports/unrealised_gains.rs` (~141–155). One `domain`/`cost_base` function with the ATO citations on it, called by every report — divergence between copies is the biggest correctness risk in the codebase. The `ato_examples.rs` suite is the safety net for this refactor
  - Done as `src/domain/cost_base.rs` (`adjusted_cost_base` + `CostBase::into_aud`), with unit tests on every pipeline step. Beyond the four reports, the same inline copies in `entities/scrip_exchange.rs`, `entities/demerger.rs`, and `entities/transfer.rs` (native-currency carried cost base) were rewired too — seven call sites, one implementation
- [x] Stop swallowing errors behind bare 500s: `map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)` appears ~62 times across 30 files (e.g. `reports/realised_gains.rs:314`) — a decode failure that `parse_dec` carefully named never reaches the logs. Introduce a shared `ApiError` in `infra/http.rs` implementing `IntoResponse` that logs internal variants via `tracing::error!` and carries 422 detail messages; per-entity error enums stay (they're good docs) and gain `impl From<EntityError> for ApiError`, replacing the hand-written 50-line match in each handler
  - Done: `infra/http.rs` `ApiError` (Internal logs on response build — verified by a log-capture test; Unprocessable/BadRequest/`not_found(msg)`/PayloadTooLarge/BadGateway preserve every existing status code and reason body, asserted by the existing handler tests). `From<sqlx::Error>` absorbed `write_error_body`'s constraint-violation→422 classification; all ~24 entity error enums gained `From<EntityError> for ApiError`; zero `map_err(|_| …INTERNAL_SERVER_ERROR)` remain
- [x] Add `cargo fmt --check` to CI (ci.yml) and the pre-push hook, and fix the existing fmt drift (`cargo fmt --check` currently fails, e.g. `src/ato_examples.rs:164`)
  - Done: `cargo fmt` applied across the tree (52 files); ci.yml gained a `cargo fmt --check` step (rustfmt added to the toolchain components) ahead of clippy, and `.githooks/pre-push` runs the same check first. Verified by running `cargo fmt --check` and the hook end to end, plus the full suite (766 tests) after reformatting

Medium value:

- [x] Fix the FX N+1 in report loops: `db_realised_gains` calls `infra::fx::to_aud(pool, …)` twice per allocation (one DB round-trip each). Pre-load `rba_fx_rates` into a `HashMap<(currency, month), Decimal>` before the loop — the real win is the gain/loss computation becoming a pure function over in-memory data (unit-testable without a pool, and a natural seam for the cost-base extraction above)
  - Done: `infra::fx::FxRates` pre-loads the whole rate table (same precedence/fail-loudly semantics as the async path, asserted by mirrored tests); every report loop now converts from the map — realised/portfolio/unrealised/open-parcels via the new sync `CostBase::into_aud_with`, tax-summary/net-capital-gain/performance/snapshot via `FxRates::to_aud`. `db_realised_gains` is split into `load_report_data` + the pure `compute_realised_gains(&ReportData)`, with pool-free unit tests (mixed eligibility + identity, map-based FX conversion, manual-override fallback). The async `CostBase::into_aud` lost its last production caller and was removed; the one-off `to_aud(pool, …)` survives where a single conversion is the right shape (live price fetch)
- [x] Run multi-query reports on one read transaction/connection: `db_realised_gains` reads sells, buys, allocations, AMIT reductions, and corporate actions in separate pool queries, so an interleaved write yields an inconsistent snapshot (an allocation whose sell is missing from `sell_map` is silently skipped at ~line 176). Wrap report reads in a single `pool.begin()`
  - Done: realised-gains, portfolio, unrealised-gains, open-parcels, performance, and tax-summary (income-side reads) each read all inputs — including the FX rates — on one `pool.begin()` snapshot; `corporate_action::db_return_of_capital_events`/`db_share_split_events` became executor-generic to run inside it. Composite callers (net-capital-gain over `db_realised_gains` + its own AMMA/E10/G1 reads; snapshot `generate` over three sub-reports) still compose individually-consistent reads rather than one outer snapshot — acceptable for a single-writer personal tool; revisit only if cross-report identities ever wobble
- [x] Cut manual row mapping in reports: local structs like `SellInfo`/`BuyInfo` in `realised_gains.rs` (~119–157) are built field-by-field with `try_get` + `parse_dec`; derive `FromRow` using the `infra/decimal.rs` helpers instead. Optionally evaluate sqlx compile-time `query!` macros with offline mode (`.sqlx` prepare step) for SQL validated at build time
  - Done: `BuyInfo` and its three sibling copies collapsed into the shared `domain::cost_base::ParcelRow` (`FromRow` via `row_dec`, with `ParcelRow::COLUMNS` for the SELECT list); `SellInfo`/`Allocation` (realised gains) and `TradeFlow` (performance) gained `FromRow` impls, removing every field-by-field report mapping. sqlx `query!` offline mode evaluated and declined: the `.sqlx` prepare artifact adds a CI/maintenance step disproportionate for a solo project, and the suite already executes every report query against a real schema
- [x] Shared test fixtures: nearly every test module re-defines `test_pool`/`insert_listing`/`insert_buy` (compare `entities/sell.rs:625–699` with `reports/realised_gains.rs:325+`); the `insert_buy` in sell.rs initialises 25 `Trade` fields, so every new column touches ~25 test modules. Add a crate-level `#[cfg(test)] mod test_support` with builder-style fixtures
  - Done as `src/test_support.rs` (`#[cfg(test)]`-declared in `main.rs`): one shared `test_pool` plus builders for the wide structs — `listing`/`buy`/`sell`/`drp`/`trade` (typed), `amma`, `income`, `ess_statement` — each defaulting every field and exposing setters only for what tests vary, with a `.with(|x| …)` escape hatch so one-off fields never grow the API; `allocate`/`amit_adjustment` helpers cover the small link rows. All 41 `test_pool` copies and every duplicated `Trade` (27), `Listing` (39), `AmmaStatement` (13) test literal collapsed — a new column now touches only the builder. `transfer.rs` keeps a local `test_pool` that delegates and seeds its second holding account. Verified by the full suite (772 tests) passing unchanged on the migrated fixtures; ~2,100 net lines removed

Smaller:

- [x] Pro-rating remainders: per-allocation brokerage shares (`sale_costs * qty_alloc / sale.quantity`, `realised_gains.rs:186`) may not sum exactly to the total. Sub-cent today, but if rows are ever rounded to cents for display/export, assign the remainder to the last allocation rather than rounding each independently
  - Done: `compute_realised_gains` pro-rates by cumulative difference — each allocation's share is `sale_costs × cum_qty / sale_qty` minus what earlier allocations took, so the last allocation absorbs the division remainder and the shares sum exactly to the total (pure test: $10 over three 1-unit allocations of a 3-unit sale). Caveat discovered en route: per-allocation `price × qty − share` subtraction can still re-round at Decimal's 28-significant-digit mantissa limit (≈1e-26 on $100-scale prices) — inherent to fixed-precision decimals, noted in the test comment
- [x] Split `web/app.js` (2,445 lines, one file) into native ES modules (`<script type="module">`) — config (`ENTITIES`/`REPORTS`/`ACTIONS`) separate from the generic rendering engine; no build step needed
  - Done as four modules with an acyclic import graph, served from `/static/`: `app.js` (the entry point — generic views, `filterableTable`, router) imports `config.js` (`ENTITIES`/`REPORTS`/`ACTIONS`), `forms.js` (field constructors, `buildFieldInput`/`readFieldValue`, GST/income form wiring, `allocationEditor`), and `util.js` (DOM/API/formatting/label-map helpers, exact decimal-string arithmetic); code unchanged beyond exports/imports and the IIFE dedent. `web.rs` serves the modules from one `JS_MODULES` table; new tests assert each module is served as JavaScript byte-for-byte and that every `./x.js` import specifier resolves to a served route (a module missing from the table would 404 and break the app), and the UI-presence tests now assert over the concatenated served bundle. Runtime-verified with `scripts/ui-check.sh --seed demo '#/r/open-parcels'` — the DOM renders identically (the script's "timed out" message after a successful dump is a pre-existing Chrome-exit quirk, reproduced on the pre-split code)
- [x] Trim `tokio` features in Cargo.toml from `full` to what the server uses (`rt-multi-thread`, `macros`, `signal`, `net`, `time`, `fs`) for slightly faster builds
  - Done: `rt-multi-thread`, `macros`, `net`, `signal`, `time` — `fs` dropped from the suggested list too, since nothing uses `tokio::fs` (the backup copies via `std::fs`). Verified by a warning-free `cargo build` and the full suite (774 tests, which exercise the runtime, macros, and timers; the server path covers net/signal)

## Sell allocations: listing and acquisition-date invariants missing (2026-07-12 review, domain + integrity)

`upsert_sell_in_tx` (`src/entities/sell.rs:515-598`) validates that each allocated parcel exists,
is a Buy/DRP, sits in the right holding account, and is not over-allocated — but never that:

- the parcel's `listing_id` equals the Sell's `listing_id` (it is read at
  `src/entities/sell.rs:546` and used only for the splits lookup), so a Sell of listing A can
  consume parcels of listing B and the CGT reports will happily cost them cross-listing
- the parcel's trade date is on or before the sale date, so a Sell can draw on a parcel acquired
  *after* it, producing a negative holding period (the discount test just says "not eligible" and
  the reports emit nonsense figures instead of rejecting the entry)

Similarly, `trade::db_upsert` (`src/entities/trade.rs:476+`) lets an existing Buy's `listing_id`
be edited while Sell allocations reference it, silently re-associating those allocations across
listings (its capacity re-check even fetches splits for the *new* listing).

- [x] Reject (422) an allocation whose parcel belongs to a different listing than the Sell, in the
      shared transactional core so every caller (sells, buy-back, scrip, demerger, transfer,
      worthless) inherits it — `SellError::PurchaseListingMismatch` in `upsert_sell_in_tx`
      ("an allocated parcel belongs to a different listing than the Sell"); every
      operation-constructed Sell already satisfies it by construction (each selects its parcels
      from the action's own listing)
- [x] Reject (422) an allocation whose parcel is dated after the sale date —
      `SellError::PurchaseAfterSale` ("an allocated parcel is dated after the sale date");
      boundary inclusive: a same-day parcel remains sellable
- [x] Reject (422, or validate against the allocations) editing a Buy's `listing_id` while
      allocations/AMIT adjustments reference it — `UpsertError::ListingChangeReferenced` in
      `trade::db_upsert`: the listing is frozen while `parcel_allocations.purchase_trade_id` or
      `amit_adjustments.trade_id` reference the trade, and edits freely again once nothing does
- [x] Tests for each rejection and message text —
      `sell::tests::{db_allocation_from_different_listing_is_rejected,
      db_allocation_of_parcel_dated_after_sale_is_rejected,
      api_cross_listing_allocation_returns_422_with_reason,
      api_allocation_after_sale_date_returns_422_with_reason}`;
      `trade::tests::{db_listing_change_on_allocated_parcel_is_refused,
      db_listing_change_under_amit_adjustment_is_refused_until_unlinked,
      api_listing_change_on_consumed_parcel_returns_422_with_reason}`. docs/API.md Sells + Trades
      422 causes and the Response codes table updated

## PUT /trades can silently rewrite a reinvest-created DRP (2026-07-12 review, integrity)

The `PUT /trades/:id` handler rejects a body with `trade_type = DRP`
(`src/entities/trade.rs:869-874`), but nothing stops a **Buy body targeting an existing DRP row**
created by `POST /income/:id/reinvest`: `db_upsert` checks every provenance column except the
reinvestment link, which lives on `income.reinvestment_trade_id`, not on the trade. The write
re-types the trade to Buy and (because the body's residual fields default to 0) silently zeroes
the residual carry-forward chain, while the income row keeps pointing at it. `DELETE /trades`
already guards this exact reference (`src/entities/trade.rs:727-737`); the upsert path doesn't.

Related: `PUT /income/:id` accepts an arbitrary client-supplied `reinvestment_trade_id`
(`src/entities/income.rs:130`, bound at 387) with no validation that the trade exists as a DRP of
the same listing/account — and an income edit that omits the field silently clears an existing
link.

- [x] `trade::db_upsert` rejects (422) an update to a trade referenced by
      `income.reinvestment_trade_id` (mirror the delete guard's message: delete the reinvestment
      via the income row instead) — `UpsertError::ReinvestmentTrade`, guarded by lookup beside
      the transfer-fee guard (the link lives on the income row, invisible to the
      provenance-column check); the 422 points at `DELETE /income/:id/reinvest`
- [x] Decide the `PUT /income` contract for `reinvestment_trade_id` (reject client-set values, or
      validate the target is an unclaimed DRP trade of the same listing) and enforce it —
      RESOLVED: **client-set values are rejected by ignoring the field** (the
      `buyback_trade_id` pattern: only reinvest-created DRP trades can exist — `PUT /trades`
      refuses free-form DRPs — so there is never a valid unclaimed target to validate);
      `IncomeBody` no longer carries the field and `income::db_upsert` never writes the column
      (absent from both the INSERT list and the ON CONFLICT SET), so an insert starts unlinked
      and an edit preserves an existing link instead of silently clearing it. The undo that
      contract requires is the new inverse operation `DELETE /income/:id/reinvest`
      (`drp_reinvestment::db_unreinvest`): deletes the DRP trade + clears the link in one
      transaction; 422 while the trade is drawn on (Sell allocation / AMIT adjustment) or is not
      the (listing, account) residual chain's tail (undo is LIFO — a later trade's
      residual_brought_forward came from this chain); `DELETE /income/:id` on a reinvested row is
      refused (422) so an orphaned DRP trade can never exist. Web UI: reinvested income rows get
      an **Undo reinvest** row action (generic confirm-and-DELETE `del` row-action support in
      app.js)
- [x] Tests: a Buy body over a reinvest-created DRP is rejected; the income-side rule is pinned —
      `trade::tests::{db_upsert_over_reinvestment_trade_is_refused,
      api_put_buy_body_over_reinvestment_drp_returns_422}`;
      `income::tests::{db_upsert_never_writes_the_reinvestment_link,
      api_reinvestment_link_is_not_client_writable, api_delete_reinvested_income_returns_422}`;
      `drp_reinvestment::tests::{unreinvest_deletes_the_trade_clears_the_link_and_allows_redo,
      unreinvest_without_a_reinvestment_is_rejected, unreinvest_missing_income_is_not_found,
      unreinvest_is_refused_while_the_trade_is_drawn_on,
      unreinvest_is_lifo_a_mid_chain_trade_is_refused, api_unreinvest_round_trip_and_rejections}`;
      `web::tests::income_ui_present` covers the Undo reinvest action. docs/API.md (Trades DRP
      paragraph, Income provenance note + endpoint table, DRP reinvestment undo section, Response
      codes), docs/SCHEMA.md `reinvestment_trade_id` note, and the README DRP feature bullet
      updated

## Net-capital-gain report reads without a transaction (2026-07-12 review, programming)

`db_net_capital_gain` / `gross_buckets` / `e10_gains` / `g1_gains`
(`src/reports/net_capital_gain.rs:404-511`) run many separate queries directly on the pool: the
realised-gains rows come from `db_realised_gains`'s own (correct) snapshot, then AMMA rows, AMIT
adjustments, ROC/split events, allocations, FX rates, and the opening loss are each read at later
instants. CLAUDE.md's report rule requires one `pool.begin()` read transaction per multi-query
report so an interleaved write can't produce inconsistent inputs (e.g. an AMMA row arriving
between the realised read and the E10 walk double- or under-counting a year). The what-if handler
(`what_if_handler`) has the same shape.

- [x] Restructure the report to read every input on one read transaction (likely: extend
      `realised_gains::load_report_data`-style loading, or take a `&mut SqliteConnection` through
      `gross_buckets`/`e10_gains`/`g1_gains`), keeping the computation pure
- [x] Same for the what-if path
- [x] A test proving the report still reproduces its fixtures (existing tests should carry this)

Closed 2026-07-13: `db_net_capital_gain` now opens one read transaction and threads a
`&mut SqliteConnection` through everything it reads — `realised_gains::db_realised_gains_on`
(the internal `load_report_data` now runs on the caller's connection; the standalone
`db_realised_gains` keeps its own tx), `gross_buckets`, `e10_gains`, `g1_gains`,
`FxRates::load`, and `cgt_settings::db_opening_capital_loss` (made executor-generic) — then
commits before the pure `net_years` computation. The what-if handler reads its inputs
(candidate parcels via the new `parcel_optimiser::db_candidate_parcels_on` /
`open_parcels::db_open_parcels_on`, realised rows, buckets, opening loss) on one transaction the
same way, with allocation validation and the scenario walk running purely after the commit.
No endpoint/schema change, so no doc updates. Covered by the existing net-capital-gain,
what-if, realised-gains, open-parcels, and parcel-optimiser fixtures (983 tests pass).

## Tax summary: franking holding-period test runs post-commit, per dividend (2026-07-12 review, programming)

`db_tax_summary` reads its inputs on one transaction, commits it, then calls
`franking::holding_period_test(pool, …)` **per franked dividend**
(`src/reports/tax_summary.rs:551-565`), each call issuing three more queries (listing preference,
trade walk, splits) on the raw pool. That both breaks the single-snapshot rule (a trade written
after the commit changes the denial outcome for a summary computed from older facts) and is an
N+1 on a report that already pre-loads everything else.

- [x] Run the holding-period walks inside the same read transaction as the rest of the report
      (thread a `&mut SqliteConnection` through `holding_period_test`, which
      `franking_at_risk` can share), and batch the per-listing lookups (preference, trades,
      splits) instead of re-querying per dividend
- [x] Existing denial tests keep passing; add one covering two dividends on one listing reusing
      the loaded walk

Closed 2026-07-13: the per-dividend async `holding_period_test`/`holding_period_test_with_sale`
functions are replaced by `franking::HoldingWalks` — `HoldingWalks::load(&mut SqliteConnection)`
batch-loads every listing's walk inputs (preference flag, artifact-excluded trade history, split
events) in three queries on the caller's connection, and `test`/`test_with_sale` run the LIFO
walk purely in memory. `db_tax_summary` and both `franking_at_risk` paths (report + what-if) load
the walks inside the same read transaction as their other inputs, so the denial is computed from
the one snapshot and no queries run per dividend. No endpoint/schema change, so no doc updates.
New tests: `franking::tests::db_one_load_answers_multiple_dividends_on_one_listing` and
`tax_summary::tests::db_two_dividends_on_one_listing_denied_independently`; all existing denial
tests pass unchanged (985 tests).

## No positivity/sanity validation on ordinary trade, Sell, allocation, and income amounts (2026-07-12 review, integrity)

The linked operations validate their inputs (`units <= 0` rejected in buy-back, rights exercise,
ESS vest, DRP reinvest, inheritance…), but the plain CRUD paths accept degenerate values, and the
schema has no CHECKs on them (`migrations/0001_schema.sql:369+`, `:444`):

- `PUT /trades` / `PUT /sells`: zero or negative `quantity` and `average_price`, negative
  brokerage/GST, and a `settlement_date` before the trade date are all accepted
- Sell `allocations`: a zero or **negative** `quantity_allocated` passes both the sum check and
  the per-parcel capacity check (e.g. −5 on parcel A and +105 on parcel B "sums" to a 100-unit
  Sell), quietly increasing another parcel's capacity
- `PUT /income` / `PUT /interest_income`: negative amounts accepted on every money column

A negative or zero quantity corrupts every downstream report without failing anything, which is
exactly what the write-time-invariant rule exists to prevent.

- [x] Decide the exact rule set (quantity > 0, price ≥ 0, brokerage/GST ≥ 0, allocation units > 0,
      income components ≥ 0, settlement ≥ trade date) and enforce it at write time with clear 422
      bodies
- [x] Tests per rejected shape; docs/API.md 422 causes updated

Closed 2026-07-13. Decided rule set, enforced in Rust at write time (each rejection is a 422
naming the rule; no schema rebuild — the checks live where every write path already runs):

- Trades and Sells (`trade::check_amounts`, shared by `trade::db_upsert` and
  `sell::upsert_sell_in_tx` so the two paths can't drift): `quantity > 0`; `average_price ≥ 0`
  (zero stays legal — the worthless-shares closing Sell has nil proceeds); `brokerage ≥ 0` and
  `gst_on_brokerage ≥ 0` (checked post-GST-split, so a negative inclusive entry is caught);
  `fx_rate > 0` (it divides the amount — zero would blow up AUD conversion);
  `settlement_date ≥ date`. Residual columns were left out: they are operation-managed
  (free-form DRP bodies are already rejected at the API).
- Sell allocations (`SellError::AllocationNotPositive`): every `quantity_allocated > 0`,
  killing the −5/+105 capacity-shift shape before the sum/capacity checks run.
- Income (`income::UpsertError::NegativeAmount`, field-naming 422): all eight money components
  ≥ 0, plus the optional `amount_per_security`/`securities_held` pair (checked before the
  per-share cross-check so a negative gets the clearer message). `tax_deferred_amount` already
  had its own check.
- Interest income (`interest_income::UpsertError::NegativeAmount`): `amount ≥ 0`,
  `tfn_withholding_tax ≥ 0`.

Operation-constructed trades/Sells (scrip, demerger, transfer, worthless, buy-back) satisfy the
rules by construction and keep passing. Tests per rejected shape:
`trade::tests::api_degenerate_trade_amounts_are_rejected_per_shape` (incl. boundary positives:
zero price/costs, same-day settlement, fractional quantity),
`sell::tests::db_zero_or_negative_allocation_is_rejected` (the exact −5/+105 review scenario),
`api_negative_allocation_returns_422_with_reason`,
`api_degenerate_sell_amounts_are_rejected_per_shape`,
`income::tests::api_negative_amount_on_any_money_column_returns_422`,
`interest_income::tests::api_negative_amounts_rejected_422`. docs/API.md updated: Trades and
Sells sanity-rule paragraphs, Income "No negative amounts", Interest income 422 causes, and the
Response codes 422 list (991 tests).

## AMMA tax_year_end_date is assumed to be 30 June but never validated (2026-07-12 review, integrity)

Every AMMA-keyed report buckets the statement by `tax_year_end_date.year()`
(`src/reports/tax_summary.rs:422`, `src/reports/net_capital_gain.rs:477`,
`src/reports/franking.rs:305`), which equals the Australian FY only when the date is in
January–June (in practice, 30 June). Nothing validates that at write time
(`src/entities/amma.rs`), so a statement keyed e.g. `2024-12-31` lands in FY2024 while
`domain::tax_year::tax_year_for` — the rule CLAUDE.md says every FY-keyed report must use — would
put it in FY2025.

- [x] Either validate at write time that `tax_year_end_date` is a 30 June date (422 otherwise),
      or bucket AMMA rows through `tax_year_for` everywhere; pick one and pin it with a test

Closed 2026-07-13. Picked write-time validation: an AMMA statement attributes a full Australian
financial year, so a non-30-June `tax_year_end_date` is bad data, not a bucketing edge case —
rejecting it keeps the reports' `.year()` bucketing provably equal to `tax_year_for`.
`amma::db_upsert` rejects any date that is not 30 June (of any year) with a new
`UpsertError::NotFinancialYearEnd` → 422 naming the rejected date and the rule. Pinned by
`amma::tests::api_non_june_30_year_end_returns_422` (2024-12-31 / 2024-06-29 / 2024-07-01 all
rejected, nothing persisted) and `db_june_30_of_any_year_accepted` (the rule pins the day, not
the year). docs/API.md (AMMA section paragraph + Response codes 422 list) and docs/SCHEMA.md
(column note) updated (994 tests).

## Buy-back participation collapses all Sell-side rejections into one message (2026-07-12 review, UX)

`ParticipationError::Sell` maps every sell invariant failure to the generic "the holding cannot
cover the units participated (over-allocated parcels)"
(`src/entities/buyback_participation.rs:127-132`), so e.g. an allocation in the wrong holding
account (`PurchaseInDifferentAccount`) or an allocation-sum mismatch is misreported. This
contradicts the useful-error-messages convention (every 422 says which invariant failed).

- [x] Pass the underlying `SellError`'s own 422 body through (it already has one per variant) and
      assert the distinct texts in tests — `ParticipationError::Sell(err)` now delegates to
      `From<SellError> for ApiError` (`err.into()`), and the participate section of API.md says
      Sell-side rejections carry the same per-invariant bodies as `PUT /sells/:id`
      (`buyback_participation::tests::api_sell_side_rejections_carry_their_own_422_bodies`)

## Open-parcel assembly duplicated across six reports (2026-07-29 Rust review)

`domain::cost_base` owns the per-parcel pipeline, but the *assembly* wrapped around it is
copy-pasted. The same ~70-line block — load Buy/DRP `ParcelRow`s, load `parcel_allocations` joined
to each sale's date, fold them into `qty_sold: HashMap<i64, Vec<(NaiveDate, Decimal)>>`, load AMIT
reductions + ROC events + split events + `FxRates`, then loop `sold_in_acquired_units` →
`remaining` → `adjusted_cost_base` → `into_aud_with` → `split_adjusted_quantity` — appears
essentially verbatim in `reports/portfolio.rs:97` (`db_holdings_on`),
`reports/unrealised_gains.rs:73` (`db_unrealised_gains`), `reports/open_parcels.rs:71`
(`db_open_parcels_on`) and `reports/performance.rs:387`, with partial repeats in
`reports/tax_report.rs:302`, `reports/realised_gains.rs:322`, and `reports/net_capital_gain.rs:309`.

This is the same class of finding as the 2026-06-10 "extract a shared adjusted-cost-base module"
item (DONE/reviews.md) one level up the call stack: that one unified steps 1–5, this one unifies
the loader around them. Today a fix to the split/ROC re-basing interaction has to land in six
places, and the copies have already drifted in ways that are correct but easy to get wrong when
edited (`up_to: Some(as_of)` vs `None`, `db_cost_base_reductions_up_to` vs
`db_cost_base_reductions`, quantity reported in as-of units vs current units).

The variation between the copies is small and parameterisable: an `as_of` cutoff (or `None`),
whether a joined `ticker` column is wanted, and whether the caller needs the full `CostBase`
breakdown or only `.adjusted`.

- [x] Add `src/domain/open_parcels.rs` with a `load(conn, as_of) -> Result<Vec<OpenParcel>, sqlx::Error>` taking the caller's own `&mut SqliteConnection` (so it composes into each report's existing single-snapshot read transaction, per the house rule) and returning per-parcel `ParcelRow` + `remaining_as_acquired` + `remaining_as_of` + the AUD `CostBase` breakdown. Parcels fully consumed (`remaining <= 0`) are filtered out, as every copy does today
  - Shipped as `OpenParcel { parcel, remaining_as_of, cost_base }`. `remaining_as_acquired` was dropped from the returned struct: no caller reads it (the cost base it feeds is already computed inside `load`), so a `pub` field only reachable from `#[cfg(test)]` would fail the warning-free build gate. `parcel.quantity` is the whole parcel on that as-acquired basis, and the field doc records where the two bases differ
- [x] Rewire `portfolio::db_holdings_on`, `unrealised_gains::db_unrealised_gains`, `open_parcels::db_open_parcels_on`, and `performance.rs:387` onto it; each keeps only its own aggregation/shaping. `open_parcels` needs its joined `ticker` — resolve it as a separate lookup rather than pushing a join option into the shared loader
  - The first three are fully on `load` (ticker via a separate `SELECT id, ticker FROM listings` lookup, as specified). `performance.rs` takes only `db_units_sold`, the allocations read: it walks *every* trade including the Sells and values acquisitions at `initial_cost` rather than the adjusted cost base, so `load` would have re-read the buys and computed cost bases it discards
- [x] Assess `tax_report.rs:302`, `realised_gains.rs:322`, and `net_capital_gain.rs:309` separately: these walk *sold* parcels, not open ones, and may only share the reference-data loading (ROC/split/AMIT/FX). Either extract that narrower piece or record here why they stay as they are
  - `net_capital_gain::g1_gains` was the one real overlap and now calls `db_units_sold`. The other two stay: their reference-data loads look alike but no two want the same set — `realised_gains` needs the *summed* AMIT reduction plus the sells and the allocations keyed by sale, `tax_report` needs the *itemised* `db_cost_base_reduction_detail` plus all trades and the transfer fee-sale ids, `g1_gains` needs neither AMIT form. A shared struct would have to carry every field for every caller, trading duplication for a wide record most callers ignore — worse than the four independent `let … = db_…().await?` lines each has now
- [x] Tests: the `ato_examples.rs` suite is the safety net (as it was for the cost-base extraction). Add a `domain::open_parcels` unit test per behaviour the copies encode — as-of cutoff excludes later trades/sales, split re-basing of an allocated quantity, AMIT/ROC reduction applied, fully-consumed parcel filtered out — plus an assertion that portfolio/unrealised/open-parcels agree on total cost base for the same fixture (the identity the duplication currently risks)
  - 8 tests in `domain::open_parcels::tests`, including `the_open_holdings_reports_agree_on_total_cost_base` (the three reports over one split/partial-sale/AMIT/ROC fixture) and `a_post_split_sale_is_re_based_before_and_after_the_subtraction`. Full suite green unchanged; net −240 lines across the six reports

## Decimal columns bypass the sqlx type system (2026-07-29 Rust review)

Every TEXT-stored decimal is read through a hand-written `FromRow` and written through
`.bind(x.to_string())`: 19 hand-written `impl sqlx::FromRow` blocks across 16 files, ~100
`row_dec`/`row_opt_dec` calls, and 109 `.bind(<decimal>.to_string())` sites. CLAUDE.md's
"never `.parse().unwrap_or(Decimal::ZERO)`" rule and "new monetary columns are TEXT" rule are
therefore enforced by review discipline on every new column, not by the compiler.

sqlx 0.9 can carry this itself. A local newtype implementing `Type`/`Decode`/`Encode` for Sqlite,
plus `#[sqlx(try_from = "…")]` on the field, lets row structs go back to `#[derive(sqlx::FromRow)]`
with plain `Decimal`/`Option<Decimal>` fields:

```rust
// infra/decimal.rs
pub struct Money(pub Decimal);             // TEXT-backed Type + Decode + Encode
pub struct OptMoney(pub Option<Decimal>);  // decodes SQL NULL itself
impl From<Money> for Decimal { … }
impl From<OptMoney> for Option<Decimal> { … }

#[derive(Serialize, Deserialize, sqlx::FromRow)]
pub struct InterestIncome {
    pub id: i64,
    #[sqlx(try_from = "Money")]    pub amount: Decimal,
    #[sqlx(try_from = "OptMoney")] pub gross_amount: Option<Decimal>,
}
```

Prototyped and runtime-verified against sqlx 0.9 before writing this section: `123.4567890123`
round-trips exactly, `NULL` decodes to `None`, and a malformed value fails with
`error occurred while decoding column "amount": invalid decimal "oops"` — the column name now comes
from sqlx rather than a hand-passed string literal that can drift from the actual column, so
diagnostics are strictly better than `parse_dec`'s. Two things worth knowing up front: sqlx 0.9's
`Encode::encode_by_ref` takes `&mut SqliteArgumentsBuffer` (not 0.8's
`&mut Vec<SqliteArgumentValue>`), and `impl From<OptMoney> for Option<Decimal>` is orphan-legal
because the local type sits in argument position.

- [x] Add `Money`/`OptMoney` to `infra/decimal.rs` with `Type`/`Decode`/`Encode` for Sqlite and the `From` conversions; keep `parse_dec` for the non-`FromRow` callers that read a scalar out of an ad-hoc query
  - Landed as specified. `row_dec`/`row_opt_dec` were kept but *reimplemented over* the newtypes (`row.try_get::<Money, _>(col)?.0`), so the hand-written readers that survive share one codec and get sqlx's column name in their errors too; `parse_dec` stays for the callers that already hold a `String`. `OptMoney` also derives `Default` (`None`), which `corporate_action::db`'s per-variant `Cols` scratch struct needs
- [x] Convert the 19 hand-written `FromRow` impls to derives, entity by entity (each file is independently convertible, so this can land as several small commits): `entities/{amit_adjustment,amma,cgt_settings,closing_price,ess_statement,income,inheritance,interest_income,investment_expense,parcel_allocation,rba_fx_rate}.rs`, `entities/{corporate_action/model,trade/model}.rs`, `reports/{performance,realised_gains}.rs`, `domain/cost_base.rs`
  - 17 of 19 converted; the 2 that remain are exactly the ones a derive cannot express. `entities::corporate_action::model::ActionKind` is internally tagged — which payload columns exist depends on `action_type`, so the variant has to be chosen before the columns are read; `CorporateAction` itself became a derive with `#[sqlx(flatten)] kind: ActionKind`, shrinking the hand-written part to the enum. `reports::performance::TradeFlow` computes `is_sell`/`group` from other columns; its decimals all come from the now-derived `ParcelRow` via `ParcelRow::from_row`. `reports::realised_gains::SellInfo` was restructured to convert: the four joined scrip-cash columns are now plain `Option<Decimal>` fields and the apportionment moved to `SellInfo::scrip_cash_apportionment()`, which returns `Result` so a half-present cash component is still an error rather than a silently un-apportioned (and so overstated) cost base
- [x] Replace the 109 `.bind(x.to_string())` sites with `.bind(Money(x))` so writes go through the same type as reads
  - All 109 replaced (110 counting `closing_price`'s `Option` form). `corporate_action::db::Cols` changed shape with them: its 19 decimal fields went from `Option<String>` to `OptMoney`, so stringification is gone from the write path entirely
- [x] Tests: a `Money`/`OptMoney` round-trip test in `infra::decimal` pinning full precision preserved, `NULL` → `None`, and a malformed value producing a decode error naming the column (the behaviour `db_malformed_decimal_is_an_error_not_zero` pins today, now at the type level). The existing per-entity CRUD tests cover the conversions; a green suite with the `FromRow` impls gone is the gate
  - 5 tests in `infra::decimal::tests` over a purpose-built `money_probe` table and a derived row struct: full-precision round trip (`123.4567890123` and `-0.000000000000000001`), `NULL` → `None` (asserting the stored value is still SQL `NULL`, not `''`), and a malformed value in each of the required and the nullable column producing an error naming that column. Full suite green (1274 tests) with the 17 impls gone
- [x] Once converted, consider whether `db::tests::migrations_store_decimals_as_text_never_real` can be strengthened to also assert every monetary column's Rust field goes through `Money`/`OptMoney` — or record here that the derive makes it unnecessary
  - Not needed for the read half, and it would have been the wrong test to write: `rust_decimal`'s sqlx feature is deliberately off, so `Decimal` has no `sqlx::Type<Sqlite>`/`Decode` impl of its own and a `FromRow` derive over a `Decimal` field *without* `#[sqlx(try_from = "Money")]` does not compile (verified by removing one attribute and building). The compiler is the gate. The write half has no such backstop — `String` is bindable, so `.bind(x.to_string())` compiles silently — so that half is pinned by a source scan instead, `infra::decimal::tests::no_write_binds_a_decimal_as_a_stringified_value`, which walks `src/**/*.rs` and rejects any line that binds a stringified value

## Entity CRUD scaffolding duplicated, and the DELETE 404 contract has drifted (2026-07-29 Rust review)

19 entity modules contain a byte-identical `async fn list`, and `get_one`/`delete` are identical
modulo the type and the message. The duplication is cheap on its own, but it has already let the
404 contract drift three ways across the delete handlers:

| style | count | user-visible effect |
| --- | --- | --- |
| `StatusCode::NOT_FOUND` | 8 — `amma`, `amit_adjustment`, `cgt_settings`, `exchange`, `exchange_holiday`, `drp_enrolment`, `attachment`, `corporate_action/http` | empty body; the web UI shows a bare "HTTP 404" |
| `Err(ApiError::NotFound)` | 1 — `listing` | empty body, same effect |
| `ApiError::not_found("no X with that id")` | 9 — `sell`, `trade/http`, `transfer`, `income`, `inheritance`, `holding_account`, `ess_statement`, `interest_income`, `investment_expense` | UI toast names what was missing |

That is exactly the split `ApiError::NotFoundWithReason` was introduced to remove
(DONE/reviews.md, "Stop swallowing errors behind bare 500s"): operation endpoints name the missing
prerequisite, and a DELETE is an operation endpoint. The fix is worth doing for the contract alone;
the boilerplate removal is the bonus.

- [x] Make the DELETE 404 contract uniform: every entity delete returns `ApiError::not_found("no <noun> with that id")`. Smallest form is one shared `infra::http::deleted(found: bool, noun: &str) -> Result<StatusCode, ApiError>` helper — do this first and independently, since it is the user-visible half
  - Landed as specified. `infra::http::deleted` is the helper; the nine drifted routes (the eight bare `StatusCode::NOT_FOUND` ones plus `listing`'s `Err(ApiError::NotFound)`) now answer with a named body, so all 21 delete routes are uniform. Most reach it through `delete_handler` (below), which calls `deleted` with the entity's `NOUN`; `rights_sale` calls the helper directly. One deliberate exception: `exchange_holiday` is keyed by `(mic, date)`, where "with that id" would be wrong, so it names both — `no exchange holiday on that date for that exchange`. The web UI needed no change; `util.js`'s `api()` already appends the response body to its error, so the toast now reads `HTTP 404: no AMMA statement with that id`
- [x] Then fold the mechanical scaffolding: a `CrudEntity` trait (`const TABLE`, `const COLUMNS`, `const NOUN`, plus the model type) with generic `list_handler`/`get_handler`/`delete_handler` in `infra/http.rs`. `db_upsert` stays per-entity — that is where the write-time invariants live and it must not be generated away
  - `CrudEntity` in `infra/http.rs` as specified, plus `type Key` (`i64`, or `String` for the code-keyed `exchanges`/`currencies`/`mic_registry`) and defaulted `KEY_COLUMN`/`ORDER_BY`. 20 entities implement it; the handlers sit over `crud_list`/`crud_get`/`crud_delete`, which build the SQL from the consts, so the SELECT column list is now written once per entity instead of once per query. Net −833/+630 lines across 26 files. Per-verb opt-in, since not every verb is the plain single-table shape: `trade` and `corporate_action` take the trait's SQL but keep hand-written handlers (a trade is presented through `Trade::present`; both have `DeleteOutcome` guards), `attachment` keeps its filtered list, `rights_sale` keeps the list/get that attach allocations, and `exchange_holiday` stays out entirely (composite key). `db_upsert` untouched everywhere. Fallout worth knowing: an entity's own `db_list`/`db_get`/`db_delete` became one-line delegations, and where the routes were their only caller they were dead in the non-test build — gated `#[cfg(test)]` where the DB-level tests still call them, deleted outright (14 of them) where nothing did
- [x] Update `docs/API.md`'s Response codes section to state that a DELETE of a missing row returns 404 *with* a plain-text reason, matching the other operation endpoints
  - The `404 Not Found` row now spells out both halves (GET → empty body, DELETE/operation → named reason, with two example messages), and the Error-bodies paragraph counts deletes among the `404`-with-a-cause responses. Pinned by `doc_checks::delete_404_reason_documented`
- [x] Tests: one test per converted entity asserting `DELETE /<entity>/{unknown-id}` is 404 with a non-empty body naming the noun (the 8 bare-404 entities have no such assertion today); the existing per-entity list/get tests cover the generic handlers
  - Written as one table-driven test rather than 21 copies of the same assertion, which would have re-created the duplication this section is about: `entities::tests::deleting_a_missing_row_is_404_naming_what_was_missing` walks a `DELETE_ROUTES` table of every delete route in the app with the noun its 404 must name — all 21, not just the converted ones — and asserts 404 plus a body containing the noun. It sits in `entities/mod.rs`, where a new entity's `.merge` line goes, so the table is next to the list it has to stay in step with; the body assertion also fails on a mistyped path (an unrouted URL 404s with an empty body). Verified to fail when a noun is wrong. Full suite green: 1276 tests, `cargo fmt --check`/`clippy --all-targets -D warnings` clean
