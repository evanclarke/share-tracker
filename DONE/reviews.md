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

## 37 error enums with hand-written `From<sqlx::Error>` (2026-07-29 Rust review)

There are 37 error enums and 32 hand-written `impl From<sqlx::Error>` blocks that all read
`fn from(e: sqlx::Error) -> Self { X::Db(e) }`. None of them implement `Display` or
`std::error::Error`, so when one is wrapped into `ApiError::Internal` the log message quality
depends entirely on whatever the `From<EntityError> for ApiError` arm writes, and the underlying
error's `source()` chain is lost.

`thiserror` gives `#[from]`, `Display`, and `source()` chaining for free. It is a proc-macro-only
dependency (no runtime surface) with a clean advisory history, so it passes the `cargo deny check
advisories` gate.

- [x] Add `thiserror` and convert the 37 enums: `#[derive(thiserror::Error, Debug)]` with `#[error("…")]` per variant and `#[from]` on the `Db(sqlx::Error)` variant, deleting the 32 `From<sqlx::Error>` impls
  - `thiserror = "2"` added; all 37 enums now derive it, and every variant carries an `#[error("…")]`. 28 of the 32 `From<sqlx::Error>` impls are gone (replaced by `#[from]`), along with `From<std::io::Error> for BackupError` and the six hand-written `Display`/`Error` impls that predated this (`BackupError`, `ScheduleError`, `FxError`, `PeriodError`, `GenerateError`, `ValuationError`) — their message text was carried across verbatim, so `FxError`'s Display, which `From<FxError> for sqlx::Error` stringifies into a `Decode` error, is unchanged. Four `From<sqlx::Error>` impls stay hand-written, each for a reason now recorded in a doc comment beside it: `ApiError`'s *classifies* constraint violations into 422 rather than wrapping, and `reports::valuation`/`snapshot`/`period_performance` hold `Db(String)`, not `Db(sqlx::Error)` — the same variant also carries failures with no `sqlx::Error` behind them (a "listing disappeared" read, a `ValuationError::Db` re-wrap), so `#[from]` cannot express it; converting those to a real source chain would be its own change of shape, not this refactor. A variant wrapping another enum takes `#[source]` (`SellError::Amounts`, `TransferError::Sell`, `income::UpsertError::PerShare`, …) so the chain reaches the innermost cause. One incidental fix fell out: `parcel_allocation`'s test-only `UpsertError::Db` was a payload-less variant that *discarded* the `sqlx::Error` (`.map_err(|_| UpsertError::Db)`), and its three uses were bare `.parse()` calls on decimal columns — they now go through `infra::decimal::parse_dec`, which is the convention CLAUDE.md already required, so a malformed stored decimal names its column instead of vanishing
- [x] Keep every `impl From<EntityError> for ApiError` exactly as it is — those carry the user-facing 422 wording that `docs/API.md` documents and must not become derived `Display` output
  - Untouched, all of them. The `#[error("…")]` strings are deliberately *log* wording — shorter, and free of the "— do X instead" remediation clauses the 422 bodies carry — precisely so the two can't be confused for each other later. `docs/API.md` needed no change: no endpoint, status code, or response shape moved
- [x] Tests: the existing per-entity rejection tests already assert the 422 bodies, so a green suite is the gate that the user-facing messages are unchanged. Add one test asserting a wrapped `sqlx::Error` still reaches the log through `ApiError::Internal` with its own message intact (extending `infra::http::tests::internal_logs_the_wrapped_error_at_error_level`)
  - `infra::http::tests::an_entity_enum_keeps_the_wrapped_sqlx_error_in_its_message_and_source`: builds a `listing::UpsertError` from a `sqlx::Error::Decode` via the derived `#[from]`, asserts the decode message survives into the enum's `Display`, asserts `source()` downcasts back to `sqlx::Error` (the half that did not exist before this section), then puts it through `ApiError::internal` and asserts the message reaches the captured error-level log. The subscriber-capture setup the original test open-coded is now a `logs_of(ApiError) -> String` helper both tests share. Full suite green — 1277 tests, `cargo build`/`cargo test` warning-free, `cargo fmt --check` and `cargo clippy --all-targets -- -D warnings` clean, `cargo deny check advisories` ok with the new dependency

## Over-long functions (2026-07-29 Rust review)

23 functions exceed 100 lines in the non-test build (`cargo clippy -- -W clippy::too_many_lines`).
The tail is what matters:

| lines | function |
| --- | --- |
| 362 | `reports/activity.rs:128` |
| 327 | `reports/tax_report.rs:693` |
| 249 | `entities/transfer.rs:208` |
| 249 | `entities/demerger.rs:130` |
| 234 | `entities/scrip_exchange.rs:126` |
| 212 | `reports/tax_summary.rs:342` |
| 206 | `reports/realised_gains.rs:358` |

The three entity ones are all the same shape — validate → walk parcels → build replacement rows →
write, in one transaction — and split naturally along those seams. The open-parcel and `Money`
sections will already shrink several of these, so this is deliberately sequenced last. (The
open-parcel extraction has since landed — see DONE/reviews.md — taking the count from 23 to 22;
none of the tail entries below moved, so the table still stands.)

- [x] Split `reports/activity.rs:128` (362 lines) — the largest single function in the codebase; treat as its own task
  - Split along the seam the function already had: read everything → build a proto row per source table → sort and walk. The reads became `Sources::load` (one struct holding the ten source tables, the FX table, the account/ticker name maps and the holding summary, all read on the caller's transaction, so the single-snapshot rule is unchanged and `None` for an unknown listing still comes from one place), the row building became nine small `Sources::*_rows` methods behind `proto_rows`, and the sort + running-balance walk became the free function `ledger`. `db_activity` is now 12 lines. The AMMA and ESS rows share one `statement_rows` (both are unit-less, amount-less statement rows) and the DRP enrolment/unenrolment pair is one `flat_map`, so the method count stayed below the row-kind count
- [x] Split `reports/tax_report.rs:693` (327 lines)
  - `income_section` is now 12 lines over five `push_*_rows` functions (income, AMMA, interest, ESS, deductions), each taking the section accumulator so the rows that also feed the foreign-income table still push into it from one place. What every one of them needs — the tax year, the FX table, and the ticker-as-at-date naming history — moved into an `IncomeContext` loaded once, which also turned the `ticker_as_at` closure into a method (the closure was why the whole thing had to be one function). Two incidental shrinks fell out of it: `amma_statement_row` builds the 16-component AMMA row through a local `aud(column)` closure instead of 16 five-line `tax_summary::aud_field(&fx, row, …)` calls, and the ESS row does the same through `label`; the sorts became `IncomeSections::sort`
- [x] Split the three rollover/transfer operations (`transfer.rs:208`, `demerger.rs:130`, `scrip_exchange.rs:126`) along the shared validate → walk → build → write seam, ideally sharing the extracted pieces rather than each growing its own
  - Shared, not tripled: the new `domain/rollover.rs` holds `CostBaseInputs` (the splits/ROC/AMIT reference data for one listing, with `carried_cost_base` over `domain::cost_base` and `open_parcels` for the per-listing remaining-units walk), `closing_sell_body`, `insert_replacement_buy` + `Provenance` (the one INSERT, with the provenance column named by the enum), and `created_trades` (the read-back loop). All three operations now call those instead of each carrying its own copy — the ROC query, the allocations query, the parcel walk and the 16-column INSERT existed three times each and now exist once. What stays per-operation is only what actually differs: `scrip_exchange::Terms` (ratio + cash apportionment), the demerger's percentage split into two Buys, and `transfer::transfer_ins` (per-allocation pro-rating) plus `write_fee_sale` (the network-fee disposal, which is a real disposal and stays outside the transfer group). Each operation's two write-time checks moved into a named `check_*` function. `db_exchange` 234→~110, `db_demerge` 249→124, `db_transfer` 249→112 lines
- [x] Re-measure after the open-parcel and `Money` refactors land and record here which of the remaining sub-250-line entries are actually worth splitting — a long function that is one flat, well-commented sequence is not automatically a defect
  - Re-measured: 20 functions over 100 lines (from 22 before this section), and the tail now tops out at 212 rather than 362. The two remaining 200+ entries are judged **not worth splitting**, both for the reason the item anticipated:
    - `reports/tax_summary.rs:342` `db_tax_summary_on` (212) — one flat sequence of "read this source table, convert each row at its own assessment month, add it to the year's bucket", five times over. Splitting it per source would need the year-keyed accumulator threaded through five functions, which is the `tax_report.rs` shape *without* the shared context that made that split pay
    - `reports/realised_gains.rs:351` `compute_realised_gains` (206) — a single walk over the allocations carrying six pieces of running state (per-sale proceeds, cost base, discount/non-discount gain, loss, the parcel breakdown, and the cumulative sale-costs pro-rating). It is already the pure half of the report (`load_report_data` is separate and unit tests call it without a DB); cutting the loop body out would mean passing all six maps to a helper, trading one long function for a longer parameter list
  - The rest of the list is 104–192 lines, none of it a copy of anything else. Left alone deliberately: `entities/sell.rs:455` `upsert_sell_in_tx` (177) and `entities/corporate_action/db.rs:81` (192) are write-time-invariant sequences where each check is a few lines and the ordering is the point, and `corporate_action/model.rs:339` (175) is the `ActionKind` FromRow match, which is one arm per variant by construction
- [x] Tests: pure refactor, so the gate is the existing suite plus `ato_examples.rs` passing unchanged; no behaviour change means no new test, which is the one case where an item here closes without one
  - Full suite green unchanged at 1277 tests (no test file touched except three test-module `use` lines that had been relying on a now-narrowed parent import), including the `ato_examples` acceptance tests that reproduce the ATO's own worked examples for both rollovers — `takeovers_example_27_gunther_partial_scrip_for_scrip_rollover` and `demergers_examples_30_32_anita_bhp_billiton_demerger`, which is the assurance that the shared `domain::rollover` pipeline still reaches the ATO's stated figures. `cargo build`/`cargo test` warning-free, `cargo fmt --check` and `cargo clippy --all-targets -- -D warnings` clean

## HTTP test boilerplate (2026-07-29 Rust review)

`test_support.rs` solved the *data* half of test setup (builders for the wide structs) but not the
HTTP half: 274 `Request::builder()` calls across 55 files and 130 copies of
`.collect().await.unwrap().to_bytes()`. Only `entities/closing_price.rs` has local `post_json`
/`put_json` helpers (`:2314`, `:2424`); every other module open-codes the request and the body
decode. Since tests are ~60% of the tree (41.6k of 69.8k lines), this is where line-count reduction
is largest — but it is lower value than the sections above, so it should not jump the queue.

- [x] Add the HTTP half to `test_support.rs`: an `ApiClient` wrapping `app::router(pool, registry, fetcher)` (or the narrower `router().with_state(pool)` where a test doesn't need the registry/fetcher) with `get_json::<T>(path)`, `put_json(path, &body) -> StatusCode`, `post_json::<T>(path, &body)`, and a `status_and_body(path)` for the rejection tests that assert on the 422 text
- [x] Migrate test modules onto it opportunistically — when a module is already being touched for one of the sections above, rather than as one large mechanical commit
- [x] Tests: the migrated tests are the test; the gate is the full suite passing unchanged after each module's migration

**Closed 2026-07-29.** `ApiClient` + `ApiResponse` live in `src/test_support.rs` beside the row
builders. `ApiClient::over(router)` wraps any assembled router, `ApiClient::full(&pool)` the whole
application as `main` serves it (offline `QuoteStub` in place of `YahooFetcher`, so no test path can
reach the network) and `full_with` the same with a caller-supplied fetcher. Verbs: `get`, `put`,
`post`, `delete`, `put_raw`/`post_raw` (a body already written out as a string), `post_bytes`
(non-JSON payloads — the multipart uploads, the CSV/XML import feeds, content type optional because
the import endpoints take a bare `String` body and are driven with none) and `post_empty`
(`POST /jobs/{name}`). Each returns an `ApiResponse { status, headers, body }` with `json()`,
`text()`, `status_and_body()` and `expect_status()`; the convenience wrappers `get_json`, `put_json`,
`put_ok` and `post_json` fold the status assertion and the decode into one call. `status_and_body`
landed on the *response* rather than as `status_and_body(path)`, because the rejection tests it is
for assert on a PUT/POST body, not a GET.

Migration was **not** opportunistic in the end: it went in as one pass over the whole tree, because
the mechanical shape was uniform enough that a script plus the compiler and the full suite made a
single sweep safer than 55 partial ones spread over months. All 274 `Request::builder()` blocks and
every `.collect().await.unwrap().to_bytes()` are gone (`test_support.rs` itself is the only file
that builds a `Request`), and the three modules that had grown local duplicates —
`entities/closing_price.rs`'s `post_json`/`put_json`/`delete_req`, `ato_examples.rs`'s
`api_put`/`api_post`/`api_get`, `web.rs`'s `get`/`body_string` — now delegate instead of
open-coding. Modules that hit the same `router().with_state(pool)` three or more times gained a
one-line local `fn client(pool: &SqlitePool) -> ApiClient`. Net **−2,297 lines** across 57 files.

`test_support::tests` is the new self-test of the harness: it drives every verb against the real
router (CRUD round-trip, report POST decode, a 422 reason read as text, `post_bytes`/`post_empty`,
and `over` seeing a narrower route table than `full`), so a change that broke the request shape
fails there rather than in the ~50 modules that depend on it. Full suite 1282 passed / 0 failed,
`cargo fmt --check`, `cargo clippy --all-targets -- -D warnings` clean.

## A Buy's date and holding account escape the Sell-allocation invariants (SCENARIOS A-09, A-13)
(SCENARIOS.md section A verification pass, 2026-08-14. `PUT /trades/:id` re-checks the dependants
of the two fields it already guards — `listing_id` (`UpsertError::ListingChangeReferenced`) and a
`quantity` shrink (`QuantityBelowAllocated` / `QuantityBelowAmitAdjustment`) — but not `date` or
`holding_account_id`, which are equally load-bearing for a Sell's allocations. Both edits are
accepted `204` and leave a state `PUT /sells/:id` itself refuses, so the invariant holds only on the
Sell side of the pair. This is the failure mode CLAUDE.md's data-integrity rule names: a write path
that can reintroduce a state another path forbids.)
- [x] A-09 — moving a Buy's `date` after a Sell that allocates from it is accepted. Reproduced:
  Buy 2022-08-01 ×100, Sell 2023-03-01 ×100 allocating all 100, then `PUT /trades/1` with
  `date: 2023-05-01` → `204`. The open-parcels and realised-gains reports then show the sale
  costed against a parcel acquired two months *after* it, `discount_eligible: false` (the discount
  clock runs backwards); the annual tax report prints the same impossible acquisition/sale pair and
  still reports `completeness.complete: true`; no cross-check flags it. Re-`PUT`ting the identical
  Sell body is refused `422 "an allocated parcel is dated after the sale date"`
  (`sell::SellError::PurchaseAfterSale`, `src/entities/sell.rs:623`) — so the guard exists, just not
  on this side. Re-check it in `trade::db_upsert` when `date` moves later, against
  `parcel_allocations JOIN trades s ON s.id = sale_trade_id` (min sale date)
- [x] A-13 — moving a Buy's `holding_account_id` out of the account a Sell allocating from it sits
  in is accepted. Reproduced: Buy ×100 in account 1, Sell ×60 in account 1, then `PUT /trades/1`
  with `holding_account_id: 2` → `204`; the parcel reports as held in account 2 while the realised
  gain stays costed against it in account 1. Re-`PUT`ting the identical Sell body is refused
  `422 "an allocated parcel is held in a different holding account from the Sell"`. Same fix shape:
  re-check in `trade::db_upsert` when the account changes and any allocation references the parcel
- [x] Tests: `entities::trade::tests` — a date move past an allocating Sell and an account move away
  from one are each refused `422` naming the rule, and the pre-edit state is unchanged (nothing
  persisted); the same edits stay allowed while no allocation references the parcel
- [x] Docs sync: `docs/API.md` Trades section + the Response codes 422 list (the two new refusals,
  alongside the existing listing-change and quantity-shrink wording)

**Closed 2026-08-14.** Both re-checks live in `trade::db_upsert` (`src/entities/trade/db.rs`),
inside the same transaction as the existing listing/quantity guards, so a rejected edit persists
nothing. The stored row they compare against is now read once into a `FromRow` struct
(`ExistingTrade` — `listing_id`, `date`, `holding_account_id` + the provenance links), replacing the
8-tuple the provenance check used.

- `UpsertError::DateAfterAllocatedSale` — only a *later* date can break the pair, so the check runs
  when `trade.date` moves past the stored date, against `MIN(s.date)` over
  `parcel_allocations JOIN trades s ON s.id = pa.sale_trade_id`. A move to the sale date itself is
  allowed, matching the Sell side (a same-day parcel is a valid allocation).
- `UpsertError::AccountChangeReferenced` — refused while *either* a Sell allocation or an AMIT
  adjustment references the parcel, mirroring `ListingChangeReferenced`'s blanket shape. The AMIT
  half is the same hole one entity over: `amit_adjustment::db_upsert_on` refuses a trade in a
  different account from its AMMA statement, so moving the parcel afterwards left a state that write
  path forbids too. The 422 points at `Transfer` as the supported way to move a parcel.

Both errors carry the rule in the 422 body (`the date cannot move after a Sell that allocates from
this parcel …`, `the holding account cannot be changed while Sell allocations or AMIT adjustments
reference this parcel …`). Five tests in `entities::trade::tests`: the two DB-level refusals (state
unchanged after each), the AMIT variant refused-until-unlinked, the same edits free while nothing
references the parcel, and one API-level test asserting both 422s and the untouched row.
`docs/API.md` gains a bullet per refusal in the Trades section and both in the Response codes 422
row. Full suite 1370 passed / 0 failed; `cargo fmt --check` and
`cargo clippy --all-targets -- -D warnings` clean.

## Deleting a split/bonus/return-of-capital silently restates reported gains (SCENARIOS A-06, A-20)
(SCENARIOS.md section A verification pass, 2026-08-14. `RightsIssue`, `BuyBack`, `ScripForScrip`,
`Demerger`, and `WorthlessShares` are all frozen while the trades they produced exist. The three
action types that re-base parcels instead of creating trades — `ShareSplit`, `BonusIssue`,
`ReturnOfCapital` — carry no delete guard at all, even though every open-parcel quantity, cost base,
and realised gain is computed from them at read time.)
- [x] A-20 — deleting a `ShareSplit` after trades were entered on the post-split basis can leave
  allocations the write path rejects. Reproduced: Buy 2023-01-10 ×100, 2-for-1 split 2023-03-01,
  Sell 2023-06-01 ×200 (post-split, the whole holding) → realised **gain $200**. `DELETE
  /corporate_actions/1` → `204`, and the same Sell now reports a **loss of $800** against a parcel
  it over-consumes: re-`PUT`ting the identical Sell body is refused `422 "the allocations exceed a
  purchase parcel's available quantity"`. A `BonusIssue` delete has the same shape
- [x] A-06 — the same delete leaves a generated AMIT adjustment covering more units than the parcel
  has (quantity materialised in as-acquired units at generation time). The AMIT adjustment
  cross-check does flag the resulting statement, so this half fails visibly rather than silently
- [x] A-21 — deleting a `ReturnOfCapital` cannot break a quantity invariant (it changes no unit
  count), but it does silently drop an already-reported CGT event G1 gain from a prior year's net
  capital gain (reproduced: G1 gain $50.00 in FY2023 → the year disappears from the report entirely)
- [x] Decide the guard: refuse the delete while any trade of the listing is dated on/after the
  action's `date` (the rule the `demerge`/`exchange`/`recognise` operations already apply at the
  other end), or require an explicit override. Whichever way, a delete must not be able to leave
  allocations exceeding a parcel — the invariant `PUT /sells/:id` enforces
- [x] Tests: `entities::corporate_action::tests` — deleting each of the three re-basing types is
  refused while dependent trades exist, and still allowed when none do
- [x] Docs sync: `docs/API.md` Corporate actions (the new refusal, per type) + Response codes 422

**Closed 2026-08-14.** The guard is per type, in `corporate_action::db_delete`'s own transaction
(`src/entities/corporate_action/db.rs`), which replaces the generic `delete_handler` on the route —
each type is checked in the direction its effect actually runs, rather than one blanket rule:

- `DeleteError::RebasedTrades` — a `ShareSplit`/`BonusIssue` whose listing has **any trade dated on
  or after** the action's `date`. That is the demerge/exchange/recognise rule the TODO proposed,
  chosen for consistency and because it is the honest statement of the dependency: those quantities
  are recorded in the post-action unit basis. It over-refuses in one harmless case (an action
  before every parcel re-bases nothing), which is the safe direction.
- `DeleteError::ReducedParcels` — a `ReturnOfCapital` whose listing has **any acquisition dated on
  or before** the payment date. The "on/after" rule would have guarded nothing here: a return of
  capital changes no unit count and its dependants are the parcels it *already* reduced, in the
  other direction. Conservatively "acquired on or before" rather than "still held at the date" — a
  delete is not worth an open-parcels load.

The other five types keep their `trades.*_action_id` foreign key as the guard (they create trades;
the three guarded here create none, which is exactly why nothing protected them). A-06 closes with
A-20: the stranded AMIT adjustment was the same over-consumption seen from the adjustment side.

`PUT` is deliberately **not** guarded the same way — freezing an action the moment anything depends
on it would mean deleting every later trade of the listing to fix a typo'd ratio. That leaves the
restatement reachable by editing, now documented as a Known limitation in `docs/API.md` and pinned
by `doc_checks::corporate_action_delete_guard_documented`. That open work is the next section here,
*Editing a split/bonus/return-of-capital in place restates the same figures a delete now can't*,
closed 2026-08-15. Six tests in `entities::corporate_action::tests` (the A-20 repro end to end, including that
the identical Sell is refused once the split is gone; the bonus-issue shape plus the
predates-everything case that still deletes; the return-of-capital pair; an unapplied `RightsIssue`
unaffected; and the two 422 bodies over the router), verified end to end against a running server.
Full suite 1376 passed / 0 failed; `cargo fmt --check` and `cargo clippy --all-targets -- -D
warnings` clean.

## Editing a split/bonus/return-of-capital in place restates the same figures a delete now can't
(Found closing *Deleting a split/bonus/return-of-capital silently restates reported gains* — now in
[DONE/reviews.md](DONE/reviews.md) — 2026-08-14. `PUT /corporate_actions/:id` re-checks only the
`trades.*_action_id` references (`WriteError::ReferencedByTrade`), so for the three read-time
action types an edit is unguarded: changing a `ShareSplit`'s ratio from 2:1 to 1:1, or moving its
`date` past a Sell, restates every quantity, cost base, and realised gain computed from it — the
same A-20 state the new delete guard refuses, reached one verb over. Documented as a Known
limitation rather than left silent, because the correction path is worth keeping: the blanket freeze
would mean deleting years of trades to fix a typo.)
- [x] Decide the shape. A blanket freeze is wrong (it closes the only way to fix a mis-keyed
  ratio). Candidates: refuse only the *breaking* edits — a ratio change, or a `date` move — while
  dependent trades exist, leaving a same-terms correction free; or accept the edit but validate the
  resulting state (re-run the affected Sells' allocation checks inside the write transaction and
  refuse `422` if any would now over-consume its parcel), which is stricter and needs no rule about
  which fields matter
- [x] Whichever way: an edit must not be able to leave allocations exceeding a parcel, the same
  invariant the delete guard now upholds
- [x] Tests: `entities::corporate_action::tests` — the A-20 shape reached by `PUT` is refused, and
  a correction that breaks nothing still lands
- [x] Docs sync: `docs/API.md` Corporate actions + Response codes 422, and retire the Known
  limitations entry (`Editing a split, bonus issue, or return of capital in place restates prior
  figures`) plus its `doc_checks` assertions if the edit stops being possible

**Closed 2026-08-15.** Shape chosen: the **second** candidate — accept the edit, validate the
resulting state. The first (freeze the *breaking* fields while dependants exist) turns out to be
nearly a blanket freeze in disguise: a `ShareSplit` row is only `listing_id` + `date` + the ratio,
so every field of it is breaking, and "leaving a same-terms correction free" would have freed
almost nothing while closing the correction path the previous section deliberately kept open.

`corporate_action::db_upsert` now re-checks, in the write's own transaction and after the row is
written, that every parcel of each affected listing still covers the sale allocations drawn on it
(`allocations_fit_parcels` in `src/entities/corporate_action/db.rs` → `422
WriteError::AllocationsExceedParcel`). It is the listing-wide form of the per-parcel invariant the
Sell and trade write paths already uphold from their own sides
(`sell::SellError::PurchaseQuantityExceeded`, `trade::UpsertError::QuantityBelowAllocated`), and it
re-bases each allocation through the same shared `as_acquired_quantity` (TD 2000/10) they use — a
corporate-action write is simply the third way that comparison can move, changing the split stream
rather than either side of the sum. Two properties fall out of checking the *written state* rather
than the changed fields:

- it needs no rule about which fields matter, so a re-type, a `date` move, and a `listing_id` move
  are all covered (a move re-checks **both** listings — the one it lands on and the one whose split
  stream it leaves);
- it equally catches a *newly recorded* consolidation over sales already allocated in the
  pre-consolidation basis, which was the same hole reached by `PUT` of a new id rather than an edit.

A correction that breaks nothing still lands: a wider ratio, a date move that stays before the
sales, and any `ReturnOfCapital` amount edit (it moves cost base, not quantities, so no quantity
invariant is at risk). The Known limitations entry is therefore **narrowed, not retired** — the edit
is still possible and still restates prior figures; what it can no longer do is leave an invalid
state — and `doc_checks::corporate_action_delete_guard_documented` now pins that narrowing
alongside the new `doc_checks::corporate_action_write_state_check_documented`.

Five tests in `entities::corporate_action::tests` (the A-20 shape reached by `PUT` — ratio shrunk and
date moved past the Sell, with the stored terms asserted untouched; the correction that still lands,
including the `ReturnOfCapital` amount edit; the new-consolidation insert; the cross-listing move;
and the 422 body over the router). Verified end to end against a running server: the A-20 repro
(realised gain $200) refuses all three breaking edits with the 422 body, leaves
`GET /corporate_actions/10` on its original terms, and accepts 2:1 → 4:1 plus a compensating 1-for-2
consolidation, after which the reports still compute (`cost_base` 850.00 = $1,000 less a $0.75/unit
return of capital re-based across the splits). Full suite 1382 passed / 0 failed; `cargo fmt
--check` and `cargo clippy --all-targets -- -D warnings` clean.

## A DELETE blocked by an inbound foreign key says the row does not exist (SCENARIOS A-18, A-23, A-38, A-41)
(SCENARIOS.md section A verification pass, 2026-08-14. `ApiError`'s `From<sqlx::Error>` maps
`ErrorKind::ForeignKeyViolation` to `"the request refers to a record that does not exist"`
(`src/infra/http.rs:295`) — correct for an *outgoing* FK (a write naming an unknown listing or
currency), but the same SQLite error kind covers the *incoming* case, where a DELETE is blocked
because something still references the row. `delete_handler`'s own doc comment (`src/infra/http.rs:266`)
records that this is the path such deletes take. For a delete the message states the opposite of
the truth: the row exists, and what is missing is the name of whatever depends on it. It also
breaks the error-bodies contract in `docs/API.md` ("saying *why* it failed — the failed invariant").)
- [x] Reproduced on every entity whose delete has no hand-written guard: `DELETE
  /amma_statements/:id` with generated AMIT adjustments (A-18/A-19 — and the statement is
  undeletable until they are removed one by one, which the message never says), `DELETE
  /listings/:id` with stored closing prices (A-23), `DELETE /exchanges/:mic` referenced by a listing
  or its own holidays (A-41), and `DELETE /corporate_actions/:id` frozen by its trade group (A-38 —
  `docs/API.md` promises a `422` here, and the status is right, only the reason is wrong)
- [x] Fix shape: keep the outgoing wording for writes, and give deletes a message that names the
  dependant — either by parsing the constraint's table out of the SQLite detail (it names the child
  table) or by adding hand-written guards like `trade`'s and `holding_account`'s. Entities with an
  explicit guard already answer well ("this account still has trades, income, AMMA statements …")
- [x] A-23 follow-on to document either way: a listing that has ever had a **manual** closing price
  entered can never be deleted — the manual price is `status: ok`, so `DELETE /closing_prices/…`
  refuses it (the documented one-way rule), and the listing's FK refuses while it stands. That
  dead-end is a consequence of two documented rules but is not itself stated anywhere
- [x] Tests: `infra::http::tests` (or per-entity) — a delete blocked by a dependant answers `422`
  naming the dependant, and a write naming an unknown row keeps the existing wording
- [x] Docs sync: `docs/API.md` Response codes `422` row + the AMMA statements and Listings sections
  (what blocks a delete and how to clear it)

**Closed 2026-08-15.** Neither of the two shapes the finding offered was taken, because the first
does not exist: SQLite's foreign-key violation carries **no detail at all** — the message is a bare
`FOREIGN KEY constraint failed`, naming neither the child table nor the constraint — so there is
nothing to parse. Hand-written guards per entity were rejected as the general answer for the reason
the finding itself hints at: they are what the nine `delete_handler` entities were missing, and
adding nine more would leave the tenth entity to rediscover the bug.

Instead the dependants are **discovered from the schema at the moment of the refusal**
(`infra::http::fk_dependants`): walk every table's `PRAGMA foreign_key_list`, keep the foreign keys
that point at the deleted row's table *and* would actually block it (`NO ACTION` / `RESTRICT` — a
`CASCADE` or `SET NULL` child goes with the row, so an attachment is never named as a blocker), and
count the matching rows on each. A new table with a new foreign key is therefore named without
touching this code, which is the property a per-entity guard cannot have. The counting query ORs a
child's foreign keys into one `COUNT(*)` rather than summing per key: a `listing_renames` row names
the same exchange as both its old and its new one, and per-key summing would report that single row
twice (pinned by the ticker-only-rename case in the test).

The wording is one sentence naming each blocking table and its row count —
`this listing is still referenced by closing prices (2) — remove those records first` — with table
names humanised (`closing_prices` → `closing prices`) through a small override map for the acronyms
the schema spells lower-case (`amit_adjustments` → `AMIT adjustments`). If the scan somehow matches
nothing it still says the row is referenced ("…by another record"), never that it does not exist:
the one thing the message must not do is state the reverse of the truth.

The write direction is untouched. `ApiError`'s `From<sqlx::Error>` keeps the outgoing wording, and
deletes never reach that arm — `delete_handler` classifies the violation itself via
`fk_dependants_message`, which returns `None` for every non-FK failure so a `CHECK` violation is
still a 422 quoting the constraint and a decode failure still a 500. A-38's hand-written path
(`corporate_action::db_delete`) takes the same helper and carries the ready-made body in a new
`DeleteError::StillReferenced`, so the message lives in the `From<DeleteError> for ApiError` arm
where every other per-entity 422 body lives and the handler still never matches variants.

A-23's dead end is documented rather than fixed, because both halves of it are deliberate: a manual
closing price is an audited correction the provider does not take back, and a listing anyone has
priced by hand has real history. `docs/API.md`'s Listings section now states it plainly, and a new
`## Deletes blocked by a dependant` section carries the shared explainer — the message shape, the
two opposite meanings the same `422` can carry, and the no-cascade rule — with the Exchanges and
AMMA statements sections and the Response-codes `422` row pointing at it.

Tests: `entities::tests::a_delete_blocked_by_a_dependant_names_it_rather_than_denying_the_row_exists`
(all four reproductions end-to-end over the router, asserting the exact bodies) and its companion
`a_write_naming_an_unknown_row_still_says_the_record_does_not_exist`; three unit tests in
`infra::http::tests` for the labelling, the empty-scan fallback, and the non-FK passthrough; and
`doc_checks::deletes_blocked_by_a_dependant_documented` for the documentation-only half. Full suite
1388 passed / 0 failed; `cargo build`, `cargo fmt --check`, and `cargo clippy --all-targets -D
warnings` all clean.

## Deleting a DRP enrolment period strands its trailing residual (SCENARIOS A-43)
(SCENARIOS.md section A verification pass, 2026-08-14. Closing a period by setting
`unenrolment_date` settles the trailing residual — the leftover the period's last reinvestment
carried forward moves to `residual_paid_out` on that DRP trade, in the same transaction, because the
registry refunds it at termination (`db_unenrolment_pays_out_trailing_carried_residual` pins this).
`DELETE /drp_enrolments/:id` ends the period just as finally and does none of it.)
- [x] Reproduced: enrol open-ended, reinvest $100 at $10.50 → DRP trade with
  `residual_carried_forward: 5.5`, `residual_paid_out: 0`. Unenrolling → `carried 0 / paid_out 5.5`.
  Deleting the period instead → `carried 5.5 / paid_out 0` — cash recorded as carrying forward into
  a period that no longer exists, and nothing can pick it up (a later reinvestment is refused
  outright, `"account 'Default' is not enrolled …"`)
- [x] Decide: settle the trailing residual on delete the same way unenrolment does, or refuse the
  delete while the period covers a reinvestment (pointing at unenrolment instead). The second is
  probably right — deleting a period that already produced DRP trades erases the record of why they
  exist, and the reinvestment cannot be re-created afterwards
- [x] Tests: `entities::drp_enrolment::tests`, mirroring
  `db_unenrolment_pays_out_trailing_carried_residual` for the delete path
- [x] Docs sync: `docs/API.md` DRP enrolments (what deleting a period does to a trailing residual)

**Resolution (2026-08-15): the second option — the delete is refused while the period covers a
reinvestment.** Settling the residual on delete would have made the two paths agree numerically
while leaving the deeper problem in place: the DRP trade would survive with no period explaining why
it exists, and it could never be re-created, because `drp_reinvestment` matches a distribution to a
period by date at read time — with the period gone, the ex date falls in no period and the
reinvestment is refused outright. Deleting a period is "it never existed"; ending one is what
`unenrolment_date` is for.

`drp_enrolment` therefore drops `http::delete_handler` for a hand-written `db_delete` in the shape
`corporate_action::db_delete` established: load the period inside the delete's own transaction, test
for a covering DRP trade over the *same* half-open `[enrolment_date, unenrolment_date)` window and
the same `(listing_id, holding_account_id)` pair the reinvestment path and the unenrolment
settlement walk use, and return `DeleteError::CoversReinvestment` if one exists. Nothing references
`drp_enrolments` by foreign key, so there was no FK for the generic path to trip on — the check has
to be explicit. The 422 body lives in the `From<DeleteError> for ApiError` arm with every other
per-entity rejection wording, and points at the way out (set an unenrolment date, which pays the
residual out, or delete the reinvestment first). A period covering nothing still deletes normally,
and a missing id still answers the pinned `404` / `no DRP enrolment with that id` via
`http::deleted`.

The guard is deliberately scoped to the period's own window and account rather than "any DRP trade
on this listing": a trade before the period, after it, or in another holding account was never
produced by this enrolment, so it has no claim on it. A *closed* period covering a reinvestment is
refused too — the record of why the trade exists matters after the residual is settled just as much
as before.

Tests: `db_delete_refused_while_the_period_covers_a_reinvestment` (the delete counterpart of
`db_unenrolment_pays_out_trailing_carried_residual` — same fixture, asserting the period and the
residual chain are both untouched, then that unenrolling settles it, then that the now-closed period
is still refused), `db_delete_allowed_when_no_reinvestment_falls_in_the_period` (all three
out-of-scope cases at once), and `api_delete_covering_period_returns_422_pointing_at_unenrolment`.
`docs/API.md`'s DRP enrolments section gained a "Deleting a period is not how you end one" paragraph
and a route-table note. Full suite 1391 passed / 0 failed; `cargo build`, `cargo fmt --check`, and
`cargo clippy --all-targets -D warnings` all clean.

## A closed financial year can be restated with nothing marking it (SCENARIOS A-15, A-21, A-25, A-35)
(SCENARIOS.md section A verification pass, 2026-08-14. Every tax report is computed live from the
current facts, so editing a prior year's inputs silently changes figures that may already have been
lodged. Report snapshots do not cover this — they snapshot the three price-dependent reports only,
never the tax summary, net capital gain, or annual tax report. `row_history` records the change, so
the restatement is *auditable* after the fact, but nothing *surfaces* it.)
- [x] Reproduced four ways, all `204`/`200` with no flag: changing a lodged year's Buy price
  (FY2023 net capital gain $500 → $1,100, A-15); deleting a `ReturnOfCapital` after its G1 gain was
  reported (A-21 — that *delete* is now refused `422`, see DONE/reviews.md; editing the payment
  amount in place restates the same year, so the finding stands); deleting the `cgt_settings`
  opening carried-forward loss after later years
  consumed it (FY2024 net gain $500 → $1,000, A-25); deleting the only disposal of a loss year that
  a later year's carry-forward drew on (FY2024 net gain $750 → $1,500, A-35). The annual tax report
  keeps reporting `completeness.complete: true` throughout
- [x] Decide the scope: this may be honest "not modelled" — there is no lodged/closed-year concept
  in the data model, and adding one is a real feature (a lodgement marker per FY, plus a
  "changed since lodgement" flag driven off `row_history` timestamps). If it stays unmodelled it
  needs a **Known limitations** entry saying so plainly, since a user reasonably assumes a prior
  year's numbers are settled. Either way this is a documentation-or-feature decision, not a bug
- [x] Related, low severity (A-40): `DELETE /exchange_holidays/:mic/:date` has no guard and no flag,
  and a trade re-saved afterwards without an explicit `settlement_date` silently recomputes against
  the changed calendar (reproduced: an ASX trade settling 2024-04-02 recomputed to 2024-03-29 — Good
  Friday itself — once that holiday was deleted). Stored `settlement_date` values are untouched, and
  no CGT figure reads the column (only the settlement-coverage report and the annual tax report's
  display do), so the exposure is a record field, not a tax figure. Worth one line wherever the
  restatement decision above lands

**Resolution (2026-08-15): documented as a Known limitation — the "not modelled" branch.** Closing a
year properly is a feature, not a fix: a per-FY lodgement marker is a new fact table (with the
row-history and staleness-trigger decisions any new fact table brings), and the flag that would make
it useful — "this year changed since you lodged it" — has to be derived by comparing `row_history`
timestamps against that marker for every table feeding the year. That is a real build, and it is not
what the system is for; the decision is to leave every year live and say so plainly, since the one
thing that would be dishonest is letting a user assume a prior year's numbers are settled when
nothing makes them so.

`docs/API.md`'s Known limitations gained **A lodged financial year can be restated with nothing
marking it**, placed directly after the corporate-action-edit entry it generalises (that one is one
instance of this). It states the mechanism (no lodgement marker; every tax figure computed live from
the current facts, never stored), all four reproductions with their figures, and the point the
reproductions make between them that no single one does — A-25 and A-35 move a *later* year's
figures because an *earlier* year's inputs changed, so the restatement need not be in the year you
edited. It then bounds the exposure with the two facts that make it survivable: `row_history` makes
every restatement auditable after the fact (but nothing surfaces it — you have to go looking), and
report snapshots deliberately do not help, covering the three price-dependent reports only. The
mitigation left to the user is their own record-keeping: save the annual tax report as a PDF at
lodgement — it is a print document built to be archived for exactly this reason — and compare
against it before relying on a re-run of a prior year.

A-40 lands as the entry's closing sentence rather than its own limitation, which is the right weight
for it: same shape (no guard, no flag), but stored `settlement_date` values are untouched and no CGT
figure reads the column, so it is a record field, not a tax figure.

The README's deliberate-scope-cuts summary gained the matching clause, so the limitation is visible
without opening `docs/API.md`. Tests: `doc_checks::closed_year_restatement_documented` (the
documentation-only requirement — the entry, the two bounding properties, the mitigation, the A-40
footnote, and the README clause). Full suite 1392 passed / 0 failed; `cargo build`, `cargo fmt
--check`, and `cargo clippy --all-targets -D warnings` all clean.

## A return of capital has no record date, so it reduces parcels bought after the entitlement was fixed (SCENARIOS B-09)
(SCENARIOS.md section B verification pass, 2026-08-15. `corporate_actions.date` for a
`ReturnOfCapital` is the **payment** date, and both the cost-base pipeline and `g1_gains` test
entitlement by it: every parcel with `t.date <= ca.date` is reduced. Entitlement to a return of
capital is fixed at the **record date**, weeks earlier — shares bought after the ex date carry no
entitlement.)
- [x] Reproduced: parcel bought 2025-02-15, `ReturnOfCapital` of $0.50/unit paid 2025-03-01 — the
  parcel's cost base is reduced by $50 although it was bought ex-entitlement and received nothing.
  Its cost base is understated, so every later gain on it is overstated
- [x] The converse is right and stays right: a parcel **sold** between the record date and the
  payment is unaffected (checked), matching G1's own "own the shares at the time of the payment"
  test in `docs/ato/cgt-non-assessable-payments.md`
- [x] `docs/API.md` states the payment-date test as though it were the rule ("reduces the cost base
  of every parcel of the listing held on the payment date"), so nothing warns the user
- [x] Decide the fix: add an optional record/ex date to the `ReturnOfCapital` payload and test
  entitlement by it (falling back to the payment date when absent, so existing rows are unchanged),
  or document the approximation and the manual correction. Note `income.ex_date` already models
  exactly this distinction for distributions, and the `RightsIssue` action's own `date` **is** its
  record date — the concept is present in the model everywhere but here
- [x] Tests: `reports::open_parcels` / `reports::net_capital_gain` (a parcel inside the window),
  or `doc_checks` for the documentation-only route
- [x] Docs sync: `docs/API.md` Corporate actions (`ReturnOfCapital`), `docs/SCHEMA.md`'s
  `corporate_actions.date` comment, and Known limitations if it is documented rather than modelled

**Resolution (2026-08-15): modelled — an optional `record_date` on the `ReturnOfCapital` payload,
with the payment date as the fallback.** The alternative (document the approximation) was rejected:
the same distinction is already modelled everywhere else it arises — `income.ex_date` for a
distribution, and the `RightsIssue` action's own `date`, which *is* its record date — so a return of
capital was the one entitlement in the system decided by the wrong date. Migration
`0023_return_of_capital_record_date.sql` adds a nullable `record_date` column (CHECK: only on
`ReturnOfCapital` rows, never after the payment `date` — entitlement cannot be fixed after the money
is paid) and re-creates the table's two `row_history` triggers with it, per the audited-table rule.

The rule itself lives in exactly one place, `RocEvent::per_unit_for`: a payment applies to a parcel
acquired *before* the record date (a parcel acquired on it is ex-entitlement — the convention
`RightsIssue` already uses), or, when no record date is recorded, to one acquired on or before the
payment date, which is byte-for-byte the previous behaviour so no existing row moves. Every
cost-base consumer inherits it through `domain::cost_base` unchanged; the net-capital-gain report's
G1 walk, which read the join's coarse payment-date bound directly, now skips a payment
`per_unit_for` declines, so the reported gain can never disagree with the cost base it is walking
down. `db_delete`'s return-of-capital guard bounds the acquisitions it looks for by the same date,
so an action whose only parcels were bought ex-entitlement now deletes freely instead of being held
by a reduction that never happened.

The second half of the finding needed no change and is now pinned: a parcel entitled at the record
date but *sold* before the payment stays unaffected — the two ends of the window are independent
tests, entitlement at the record date and ownership at the payment (G1 adjusts the shares owned at
the time of the payment, `docs/ato/cgt-non-assessable-payments.md`).

Tests: `corporate_action::tests::per_unit_for_tests_entitlement_at_the_record_date` (both ends of
the window, plus the fallback), `db_return_of_capital_record_date_round_trips` (round-trips through
the event stream the reports read, and clears again),
`db_check_rejects_an_impossible_record_date` (the CHECK, both arms),
`api_return_of_capital_record_date_round_trip`, `api_invalid_record_dates_return_422` (after the
payment date; on a `ShareSplit`/`RightsIssue`; and the same-day fixing that is legal),
`db_deleting_a_return_of_capital_is_refused_while_it_reduced_a_parcel` (extended: the ex-entitlement
parcel deletes, one day earlier it doesn't);
`open_parcels::tests::db_return_of_capital_skips_parcels_bought_after_the_record_date` (the
reproduction — the ex-entitlement parcel keeps its $1,010.945 cost base — and the payment-date
fallback reducing both parcels);
`net_capital_gain::tests::db_g1_skips_a_parcel_bought_after_the_record_date` (one parcel's excess,
not both; $50 → $100 without the record date);
`realised_gains::tests::db_return_of_capital_needs_both_entitlement_and_holding_at_payment` (both
ways of missing a payment, in one report);
`doc_checks::return_of_capital_record_date_documented`; `web::tests::corporate_actions_ui_present`
(the form field and its ex-entitlement hint). Docs: `docs/API.md`'s `ReturnOfCapital` bullet now
states both conditions and what leaving `record_date` out falls back to, the delete-guard bullet and
the `422` catalogue follow the record date, `docs/SCHEMA.md` documents the column and the `date`
comment, and the web UI gained the field, its hint, and the corrected type description. Full suite
1411 passed / 0 failed; `cargo build`, `cargo fmt --check`, `cargo clippy --all-targets -D
warnings`, and `node --test 'src/web/*.test.js'` all clean.

## Two documentation gaps found alongside the section B pass (SCENARIOS B-17, B-20)
(SCENARIOS.md section B verification pass, 2026-08-15. Neither produces a wrong figure; both leave
a reader unable to tell what the system did.)
- [x] B-17 — a Sell's brokerage and GST are **netted off `proceeds`** rather than added to the
  cost base: a 100-unit sale at $12 with $10.945 of costs reports `proceeds: 1189.055` /
  `cost_base: 1010.945`, where the ATO's own presentation is capital proceeds $1,200 against a cost
  base including the disposal's incidental costs (`docs/ato/cgt-cost-base.md`, second element:
  costs "that relate to the CGT event"). The capital gain is identical either way — only the two
  reported components differ — but `docs/API.md`'s realised-gains section defines neither, so a
  user reconciling against an ATO worksheet finds two figures that don't match and a gain that
  does. Document which convention the report uses, and why
- [x] B-20 — rights **bought on-market** can be exercised only up to the holding's own record-date
  entitlement: `POST /corporate_actions/:id/exercise` caps cumulative units at the entitlement and
  answers `the units exercised exceed the entitlement earned by the holding at the record date`.
  That is a safe refusal, and the cost-base side works (`rights_cost` lands in the parcel's cost
  base and the discount clock runs from exercise, both checked) — but `rights_cost`'s documentation
  ("the total paid to acquire the exercised rights, 0 … for rights issued free") implies purchased
  rights are supported, while Known limitations names only pre-CGT originals and non-renounceable
  retail premiums. Say that rights acquired beyond the holding's own entitlement are not recordable

**Resolution (2026-08-15): both documented — neither is a wrong figure, and neither needed code.**

B-17: the realised-gains section gained *Where a Sell's brokerage and GST land*, which states the
convention (netted off `proceeds`, pro-rated across the sale's allocations, never added to
`cost_base`), contrasts it with the ATO's own presentation (`docs/ato/cgt-cost-base.md`, second
element — incidental costs "that relate to the CGT event"), and works the TODO's figures through
both: `proceeds: 1189.055` / `cost_base: 1010.945` here against $1,200.00 / $1,021.89 on a
worksheet, the same $178.11 gain. It also says *why* the convention is what it is, which is the part
a user can't infer: netting keeps `proceeds` the cash actually received and keeps `cost_base` the
same figure the open-parcels and unrealised reports show for that parcel while it is still held, so
a parcel's cost base doesn't move the moment it is sold. The reader reconciling against a worksheet
is told what to expect — the gain agrees, the two components differ by the disposal costs.

B-20: the exercise section now says `rights_cost` covers rights **bought on-market** only within the
entitlement the holding itself earned, and quotes the refusal beyond it; the Known-limitations
*Rights issues* entry gained rights acquired beyond the holding's own entitlement as a third
unmodelled case, with the entry route it leaves — the extra shares go in as an ordinary Buy at their
full acquisition cost. The cap itself was already right and already tested; what was missing was
that `rights_cost`'s wording implied more support than the endpoints give.

Tests: `doc_checks::sale_side_incidental_costs_convention_documented` and
`doc_checks::rights_beyond_the_entitlement_documented` (documentation-only requirements — the
convention, its worked figures, its reason, and the cited ATO mirror's own second-element heading;
the scope cut and its entry route). Full suite 1411 passed / 0 failed.

## Scrip-for-scrip and demerger rollovers are assumed, not stated as a scope cut (SCENARIOS C-09)
(SCENARIOS.md section C verification pass, 2026-08-15. Not a wrong figure — a scope cut that lived
everywhere except the list a reader scans for scope cuts.)
- [x] C-09 — a demerged parcel where the **rollover was not chosen** is not modelled: the `Demerger`
  corporate action and its demerge operation implement Div 125 with rollover, and recording one is
  in effect the taxpayer's assertion that the rollover applies — nothing checks eligibility, and
  there is no no-rollover variant of the operation. The behaviour is right for what it models and
  fails safe (there is no operation to invoke; the user enters the trades by hand), and the cut was
  stated in `docs/API.md`'s `Demerger` bullet, the README's demerger line, and `docs/ato/demergers.md`'s
  own *Out of scope* section — but **not** in Known limitations, where the neighbouring `Rights
  issues` scope cut is. That matters more here than for most omissions because the two cases differ
  in the *opposite* direction on the discount clock: with rollover the new interests carry the
  consumed parcel's acquisition date (`deemed_acquisition_date`), so a parcel sold six months after
  the demerger still discounts off the original acquisition (the ATO's Example 32); without it they
  are acquired at the demerger date and run their own 12-month clock from it (Example 33). A reader
  who checks Known limitations, finds nothing, and assumes the general case is handled gets the
  wrong clock

**Resolution (2026-08-15): documented — the behaviour is correct for the modelled case and needed
no code.**

Known limitations gained *Rollovers assume the rollover was chosen*, covering both parcel-substituting
actions (`ScripForScrip` under Subdiv 124-M, `Demerger` under Div 125) rather than the demerger alone,
since the same assumption and the same silence applied to takeovers. The entry states that recording
either action *is* the rollover assertion and that nothing verifies eligibility, gives both directions
of the discount clock explicitly, and names the manual entry route each no-rollover case leaves — a
Sell plus Buy for an exchange, a Buy dated the demerger date for demerged interests (with the capital
return as a `ReturnOfCapital` and any assessable demerger dividend as income). Pre-CGT original
interests, which the ATO's Examples 31 and 33 turn on, are cross-referenced to the existing *Pre-CGT
holdings* entry — they are unenterable anywhere, so that half cannot arise.

Tests: `doc_checks::rollover_assumed_scope_cut_documented` (the entry, both clock directions, both
entry routes, and the cited ATO mirror still carrying the no-rollover rule it rests on). The
rollover side's own behaviour is pinned by
`entities::demerger::tests::demerged_parcel_sold_six_months_later_discounts_from_the_original_buy`
and `entities::demerger::tests::deemed_date_and_a_later_split_both_survive_on_a_replacement_parcel`,
added in the same pass. Full suite 1424 passed / 0 failed.

## A negative investment expense is accepted and *adds* to assessable income (SCENARIOS H-06, H-09)
(SCENARIOS.md section H verification pass, 2026-08-17. `entities::investment_expense::db_upsert` is
the only `db_upsert` in the tree with no write-time check at all — it has no error enum, returning
`sqlx::Error` — so every figure on the row is whatever was keyed.)
- [x] H-06 — `PUT /investment_expenses/1` with `expense_type` `Other` and `amount` `-500` answers
  `204`. The tax summary then reports `deductions_other` `-500`, `deductions_total` `-495` (against
  a legitimate `+5` loan-interest row) and `net_assessable_investment_income` **`495`** on a year
  whose `gross_assessable_investment_income` is `0`: a negative deduction is arithmetically income,
  and it inflates the net line above the gross
- [x] The sibling entity already refuses exactly this. `interest_income::UpsertError::NegativeAmount`
  rejects a negative `amount`/`tfn_withholding_tax`/`foreign_tax_paid` with `422` naming the field,
  "interest figures are the statement's own positive (or zero) amounts" (2026-07-12 review, where
  negatives "silently reduced the year's gross-interest line"). The expense entity is the one that
  was missed — the same class of defect, one line further down the same report
- [x] `gross_amount` `-100` is accepted too, and `deductible_percentage` takes `150` and `-10` — a
  percentage outside 0–100 is not a percentage
- [x] The fix is the sibling's, verbatim in shape: an `UpsertError` with `NegativeAmount(&'static str)`
  (plus a percentage-range variant), the `From<UpsertError> for ApiError` arm carrying the
  user-facing wording, and the new `422` causes in `docs/API.md`'s catalogue
- [x] Tests: a negative `amount`/`gross_amount` and an out-of-range `deductible_percentage` are each
  refused `422` naming the field with nothing persisted, zero stays acceptable, and the tax summary's
  net line can no longer exceed its gross line from a deduction alone

**Resolution (2026-08-17): fixed as the sibling's shape, together with the apportionment
cross-check below (one `UpsertError`, one pass over the entity).**

`entities::investment_expense` gained the `UpsertError` enum it was missing —
`Db(#[from] sqlx::Error)`, `NegativeAmount(&'static str)`, `PercentageOutOfRange(Decimal)` (carrying
the rejected value) — with the user-facing wording in the `From<UpsertError> for ApiError` arms, per
the project's split between `#[error]` log wording and 422 bodies. `db_upsert` now returns
`Result<(), UpsertError>` and rejects a negative `amount`/`gross_amount` naming the field, and a
`deductible_percentage` outside 0–100 inclusive naming the value. Zero stays acceptable on all three
(a nil-cost expense, and the 0%/100% boundaries).

`docs/API.md` gained a *No negative amounts, and a percentage is a percentage* paragraph under
Investment expenses spelling out **why** a negative deduction is income (the tax summary subtracts
the deduction total from gross, so `-500` lifts the net line above the gross), plus the new causes in
the Response-codes `422` catalogue; `docs/SCHEMA.md`'s three column lines now state the constraints.

Tests: `entities::investment_expense::tests::api_negative_amounts_rejected_422` (both fields named,
nothing persisted, zero accepted), `::api_percentage_outside_0_100_rejected_422` (both ends refused
naming the value, 0 and 100 accepted), and
`reports::tax_summary::tests::db_a_deduction_alone_cannot_lift_the_net_line_above_the_gross` — the
scenario's own `-500`/`+5` pair, asserting the negative write is refused and the year that reaches
the report has `net ≤ gross`. Full suite 1554 passed / 0 failed.

## An investment expense's apportionment provenance is never checked against what is claimed (SCENARIOS H-06)
(SCENARIOS.md section H verification pass, 2026-08-17.)
- [x] H-06 — the scenario is the ordinary one: a fee that is part income-producing and part private,
  where the user works out the deductible share. The row records all three figures — `gross_amount`,
  `deductible_percentage`, `amount` — and nothing relates them. `gross_amount` `100` with
  `deductible_percentage` `50` and `amount` `900` answers `204`; so does an `amount` nine times a
  `gross_amount` with no percentage at all
- [x] Both fields are documented "optional provenance (informational only)", so this is a deliberate
  starting point, not an oversight — but the system has the opposite precedent for a supplied pair:
  `income.amount_per_security × securities_held` must equal the components to the cent or the write
  is refused `422` naming the computed product (G-23), and `trades.statement_total` reconciles the
  same way. A user who keys 50% and then the *gross* figure as the amount over-claims, and the two
  fields that record the mistake sit inertly beside it
- [x] **Decided 2026-08-17: cross-check it, the `amount_per_security` way.** When both provenance
  fields are supplied, `gross × pct` cent-rounded must equal `amount` or the write is refused `422`
  naming the computed figure. (The alternatives put aside: a health-report warning like the
  `duplicate_*` lists, or documenting the pair as a note to self that nothing verifies.)
- [x] Tests: whichever way it lands, an inconsistent triple is refused/flagged and a consistent one
  (including the exactly-100% and no-percentage cases) is accepted

**Resolution (2026-08-17): cross-checked as decided.**

`check_apportionment` runs in `db_upsert` after the negative/range checks (so a degenerate figure
gets the clearer message first) and mirrors `income::check_per_share`: with **both** provenance
fields supplied, `gross_amount × deductible_percentage / 100` must equal `amount`, and the refusal
— `ApportionmentMismatch { product }` → `422` — carries the computed figure. Supplying one of the
pair, or neither, skips the check: either alone records less than a determination, and the decision
was explicit that the no-percentage case stays acceptable.

One departure from the `amount_per_security` precedent, deliberate: **both** sides are cent-rounded
(half away from zero) rather than only the product. Either figure here can legitimately carry
sub-cent precision — a fee stated to more decimals, a percentage that doesn't divide evenly (the
existing `db_round_trips_with_decimal_precision_and_provenance` row is 816.4609052 at 75% =
612.3456789) — while the money that reaches the return is cents, so comparing a cent-rounded product
against an exact amount would reject faithfully-keyed statements. The `entities::mod` round-trip
fixture was re-based onto a reconciling triple for the same reason.

`docs/API.md` gained an *Apportionment cross-check* paragraph (the rule, why both sides round, what
skips the check, and the over-claim it catches) plus the catalogue entry; `docs/SCHEMA.md` states it
on the column; the two `config.js` field hints now say the pair is cross-checked rather than
"informational only".

Tests: `entities::investment_expense::tests::api_apportionment_provenance_must_reconcile` — the
scenario's own 100-at-50%-claimed-as-900 refused with the computed figure in the body and nothing
persisted, and the consistent, exactly-100%, no-percentage, no-gross-amount, neither, and
reconciles-to-the-cent (1000 × 33.3333%) cases all accepted. Full suite 1554 passed / 0 failed.

## An expense covering more than one financial year has nowhere to be apportioned (SCENARIOS H-08)
(SCENARIOS.md section H verification pass, 2026-08-17.)
- [x] H-08 — one `investment_expenses` row is one `date_incurred`, one financial year, deducted in
  full in that year (`tax_summary`'s deduction loop buckets by `tax_year_for(date_incurred)`). Two
  ordinary share-investor expenses do not work that way:
  - **Borrowing expenses** — loan establishment fees, legal expenses, stamp duty on the loan: "If
    your expenses total more than $100, apportion them over 5 years or the loan term, whichever is
    shorter. If your expenses are $100 or less, you can claim a deduction for the full amount in the
    year you incur them" (ATO, *Dividend income deductions*, QC 104069, retrieved 2026-08-17; s 25-25)
  - **Prepaid interest** — a prepayment whose eligible service period runs over 12 months, or ends
    after the last day of the next income year, is apportioned by days across the years it covers
    (ATO, *Deductions for prepaid expenses*, the Martin example: $1,250 over 397 days → $573 in the
    first year, $677 in the second). Inside the 12-month rule it *is* immediately deductible, which
    is the case the current model gets right by construction
- [x] So a $2,000 loan establishment fee entered as one row claims 5× the first year's deduction, and
  nothing refuses it, flags it, or documents the alternative. `gross_amount`/`deductible_percentage`
  are no help: they describe the private-vs-income-producing split, not a split across time, so there
  is not even a provenance field saying "this row is one year of five"
- [x] Nothing in `docs/API.md`'s Known limitations, the entity's UI description, or
  `docs/ato/investment-income-deductions.md` (which lists borrowing costs as claimable without saying
  over what period) mentions time apportionment
- [x] **Decided 2026-08-17: document the workaround** — one row per financial year carrying that
  year's apportioned share, stated as a Known limitation naming both ATO rules (QC 104069 for the
  5-years-or-loan-term borrowing expenses, the prepaid-expenses guide for the 12-month rule and the
  day-count formula), a UI hint on the entity, and a mirrored `docs/ato/` doc indexed in OVERVIEW.
  Not modelled: a `service_period_start`/`service_period_end` pair the tax summary apportions by
  days is the honest version but a real feature — new columns, the day-count split, the annual tax
  report's rows, and the "which year is this row in" question every report answers with one date
- [x] Tests: whichever way it lands, a multi-year expense reaches the right per-year deduction (or is
  refused/documented), and `doc_checks` pins the stated rule

**Implemented 2026-08-17 as decided: the workaround is documented, the apportionment is not
modelled.**

One `investment_expenses` row stays one financial year, deducted in full there — which is right for
a management fee, an account-keeping fee, a month's loan interest, and any prepayment *inside* the
12-month rule (immediately deductible, the case the one-date model gets right by construction). For
the two expenses the ATO spreads across years, the stated entry convention is **one row per
financial year** carrying that year's apportioned share, with the `description` naming the whole
expense and its place in the sequence ("loan establishment fee, year 2 of 5"). The apportionment is
the taxpayer's working, exactly as the private-use percentage beside it already is.

- **The ATO mirror.** `docs/ato/expense-time-apportionment.md` carries both rules verbatim from
  their own sources: the borrowing-expenses paragraph of *Dividend income deductions* (QC 104069,
  retrieved 2026-08-17 — over $100 apportioned over 5 years or the loan term, whichever is shorter;
  $100 or less deductible in full when incurred; ordinary loan *interest* deductible when incurred,
  which is the distinction that keeps a monthly interest charge a single row), and the non-business
  prepayment rules of *Deductions for prepaid expenses 2026* (QC 106556 — the 12-month rule, the
  `A × (B ÷ C)` day-count formula, and both worked examples). The Martin example is mirrored at the
  ATO's current 2026 figures ($1,250 over 396 days → $572 + $678) with a note that the ATO rolls the
  example's dates forward each year — the TODO's $573/$677 was the 2025 edition's 397 days. Indexed
  in `docs/ato/OVERVIEW.md`.
- **Known limitation.** `docs/API.md` states it as a scope decision naming both rules, both QC
  numbers, the day-count formula, the workaround, and why it isn't modelled — a
  `service_period_start`/`service_period_end` pair the tax summary splits by days means new columns,
  a day-count apportionment, extra annual-tax-report rows, and a different answer to "which year is
  this row in" than every other dated record gives.
- **Where the row is written.** A paragraph in the API doc's Investment expenses section, the
  `date_incurred` column note in `docs/SCHEMA.md`, a README line on the feature bullet, and — the
  surface that actually catches the mistake — the UI: the entity description and the `Date incurred`
  field hint both say one row is one year and name the two spread-across-years cases.

Tests: `reports::tax_summary::tests::db_a_multi_year_expense_deducts_per_year_when_entered_per_year`
(a $2,000 fee entered as five $400 rows deducts $400 in each of FY2025–FY2029 and no more — and the
same fee keyed as one row lands $2,000 in FY2025, accepted without complaint, which is the
limitation being documented), `doc_checks::multi_year_expense_apportionment_documented` (both rules
quoted in the mirror with their QC headers, the formula and worked figures, the index entry, and the
limitation naming both sources and the workaround), and
`web::tests::investment_expense_per_year_entry_hint_present`.

## A reinvestment paid after its period's unenrolment escapes that period (SCENARIOS I-01, I-02, I-04)
(SCENARIOS.md section I verification pass, 2026-08-17. Eligibility is decided on the **ex date** —
registry practice, and right: participation is fixed at the record date. But every *other* question
about which period a reinvestment belongs to is decided on the **trade date**, which is the payment
date. Those two dates straddle a period boundary whenever a plan is ended between a distribution
going ex and its payment — the ordinary way a DRP is stopped — and then the three trade-date reads
disagree with the ex-date one: the trailing-residual settlement in `drp_enrolment::db_upsert`, the
`residual_brought_forward` chain lookup in `drp_reinvestment::db_reinvest`, and the
`CoversReinvestment` delete guard in `drp_enrolment::db_delete`.)
- [x] I-01 — reproduced: period `[2020-01-01, 2024-07-01)` CarryForward; a $100 distribution ex
  2024-06-20, paid 2024-07-15, reinvested at $7 → `201`, trade dated **2024-07-15**, 14 units,
  `residual_carried_forward: 2`. That $2 is **stranded**: the period's settlement walk
  (`date >= enrolment_date AND date < unenrolment_date`) never sees the trade, so it is neither
  paid out nor available to any later reinvestment — re-saving the closed period does not reach it
  either. The registry refunds that leftover at termination; the record says it is still carried
- [x] I-01 — same fixture, re-enrolling on the same day (`[2024-07-01, …)` PayOut): the trade dated
  2024-07-15 now falls inside the **new** period, and the next reinvestment (Sep 2024) brings its
  $2 forward — the carry crosses a period boundary the module doc guarantees it never crosses, and
  the *new* period's residual handling settles money the *old* period's plan left over
- [x] I-02 — the A-43 guard is defeated by the same mismatch: `DELETE /drp_enrolments/1` on the
  first fixture answers **`204`**, deleting a period that demonstrably produced a reinvestment.
  `db_delete`'s `EXISTS(… trades … date >= ? AND date < ?)` asks the trade-date question, so the
  refusal that exists precisely to keep "the record of why that trade exists" (DONE/reviews.md,
  A-43) never fires for a distribution paid after the unenrolment
- [x] **Decided 2026-08-17 (Evan): (a) match by the distribution's entitlement date.** The three
  reads all want *the period that authorised this reinvestment*, which is knowable exactly:
  `income.reinvestment_trade_id` links the trade back to its distribution, whose `ex_or_pay_date`
  is the date eligibility was decided on. Join `trades → income` in all three places, so period
  membership is the same question everywhere and the trade date stops deciding anything. (Rejected:
  refusing a trade date outside the period — it refuses a genuine registry pattern and mis-dates
  the parcel if the user complies; a `drp_enrolment_id` provenance column — a fourth thing to keep
  in step.)
- [x] Tests: the ex-in/paid-after fixture settles its residual at unenrolment; the same fixture with
  an immediate re-enrolment does **not** carry into the new period; `DELETE` of the period is
  refused `422` pointing at unenrolment
- [x] Docs sync: `docs/API.md` DRP enrolments (which period a reinvestment belongs to, and that it
  is not the trade date), plus the module docs in `entities::drp_enrolment`/`drp_reinvestment` that
  currently state the trade-date rule


**Resolution (2026-08-17): one query answers "which trades does this period cover", and it asks the
entitlement-date question.**

`drp_enrolment::PERIOD_TRADES_FROM_WHERE` is that query's `FROM`/`WHERE` — `trades t JOIN income i
ON i.reinvestment_trade_id = t.id`, filtered on the listing, the account and the distribution's
entitlement date — and all three readers now build on it: the settlement recompute, the delete
guard, and `db_reinvest`'s residual-brought-forward lookup (which still *orders* by trade date, the
order the cash moved in; only membership changed). The entitlement date in SQL is
`Income::EX_OR_PAY_DATE_SQL`, the row-level twin of `Income::ex_or_pay_date`, pinned against it by
`ex_or_pay_date_sql_matches_the_model` so the two cannot drift.

The join is total in practice — a DRP trade exists only because the reinvest operation created it
and linked it in the same transaction, and `PUT /trades` refuses the type — which the enrolment
tests' `insert_drp_trade` fixture now reflects: it inserts the funding distribution too.

Tests: `a_reinvestment_paid_after_the_unenrolment_still_belongs_to_its_period` (settles at the
unenrolment, the period cannot be deleted, and an immediately re-opened period does not adopt it)
and `a_reinvestment_paid_under_the_next_period_settles_under_its_own` (the same fixture from the
reinvest side: refunded under period 1's terms as it is entered, and period 2's chain starts from
nothing).
## Re-opening or extending an unenrolment does not restore the residual it paid out (SCENARIOS I-01, I-03)
(SCENARIOS.md section I verification pass, 2026-08-17. Closing a period moves the trailing
`residual_carried_forward` to `residual_paid_out` — correct, the registry refunds it. The write is
one-way: nothing restores it if the closure is undone or moved, and `db_upsert`'s own comment calls
the settlement "idempotent — once moved, carried is zero", which is exactly why the reverse edit
cannot recover.)
- [x] I-03 — reproduced: open period, $100 reinvested at $7 → 14 units, `carried 2`. Unenrol
  (`carried 0 / paid_out 2` — correct). Then correct the mistake by clearing the unenrolment date:
  the trade still reads `carried 0 / paid_out 2`, and the next reinvestment in the re-opened period
  brings forward **0**, buying 14 units off $100 instead of 14 off $102 (`carried 2` instead of
  `carried 4`). The chain has silently lost $2 — and with a smaller price step it loses a *unit*
- [x] I-01 — the realistic version is a mistyped end date, not a change of mind: closing at
  `2021-01-01` and correcting to `2025-01-01` settles the residual under the first window and never
  un-settles it, leaving a mid-chain trade carrying `paid_out` and every later reinvestment in the
  period funded short
- [x] **Decided 2026-08-17 (Evan): (a) make the settlement a function of the period, not an
  event.** On every upsert, recompute both residual columns for the period's trades from the period
  as it now stands — the trailing trade settles iff the period is closed, every other trade carries
  — which makes the edit reversible by construction. (Rejected: restoring on re-open only, which
  leaves the mistyped-then-extended case wrong; documenting it as one-way.)
- [x] Tests: unenrol → re-open restores `carried` and the next reinvestment brings it forward;
  unenrol → extend moves the settlement to the new trailing trade and leaves no `paid_out` behind;
  the existing `db_unenrolment_pays_out_trailing_carried_residual` still holds
- [x] Docs sync: `docs/API.md` DRP enrolments (what editing an unenrolment date does to a residual)


**Resolution (2026-08-17): `recompute_residuals` derives the split from the period on every write.**

Each reinvestment's leftover is `residual_carried_forward + residual_paid_out` — an invariant total,
since no edit changes what the plan did not spend. Where it sits is now a function of the period:
`PayOut` refunds every leftover, `CarryForward` carries all but the last, and the last is refunded
iff the period is closed. `drp_enrolment::db_upsert` runs it in its own transaction, and
`drp_reinvestment::db_reinvest` runs it too after linking the new trade, so a reinvestment entered
against an already-closed period settles at once instead of waiting for the period to be saved
again. The trades' `residual_brought_forward` and quantities are history and are never rewritten.

Tests: `re_opening_or_moving_an_unenrolment_re_derives_the_settlement` — unenrol → re-open restores
the carry, and closing the period between two reinvestments moves the settlement to the first while
leaving the now-uncovered second alone. `db_unenrolment_pays_out_trailing_carried_residual` and
`db_unenrolment_only_settles_trades_inside_the_period` still hold unchanged.
## A whole-number stated allotment can swallow a share's worth of cash (SCENARIOS I-06)
(SCENARIOS.md section I verification pass, 2026-08-17. The optional `units` path exists for broker
plans that allot **fractional** shares: the statement's figure is authoritative, cross-checked
against the available cash to within `1 unit-step at the stated precision × price`, and the residual
columns record zero because a fractional allotment leaves no cash behind. The tolerance scales with
the units' own scale, so at scale 0 it is a *whole unit's* worth of cash — and the discarded
difference is real money, not statement rounding.)
- [x] I-06 — reproduced: $100 available, price $7, `units: "14"` → **`201`**, quantity 14,
  `residual_brought_forward/carried_forward/paid_out` all `0`. The $2 that bought no whole unit is
  neither carried nor paid out; the next reinvestment brings forward nothing. At `units: "14.286"`
  (3 dp, the fractional case the path is for) the tolerance is $0.007 and the same $100 is fully
  spent — the behaviour is right there. A full step off (`14.290`) is correctly refused `422`
  carrying both figures
- [x] The entry path makes this reachable: the reinvest form's units field is offered on every
  distribution, and an ASX registry statement *does* state whole units allotted — keying them in
  is the natural thing to do, and it silently costs the parcel $2 less than the cash applied while
  losing the carry
- [x] **Decided 2026-08-17 (Evan): (a) treat the difference as a residual.** Compute
  `available − units × price` as the leftover and apply the period's residual handling to it,
  cent-rounded so a fractional plan's sub-cent statement rounding still records zero; the tolerance
  check stays as the sanity bound. Records what actually happened rather than discarding it.
  (Rejected: refusing whole-number `units`; a fixed cent tolerance, which would reject the
  fractional case the field exists for.)
- [x] Tests: whole units with a genuine leftover carry it (or are refused, per the decision) and
  the next reinvestment brings it forward; the fractional cases
  (`explicit_units_take_the_statements_fractional_allotment`,
  `explicit_units_tolerate_sub_step_statement_rounding`, `morgan_stanley_ice_fractional_statements_reproduce`)
  are unchanged
- [x] Docs sync: `docs/API.md` reinvest `units` semantics + the Response-codes `422` catalogue if a
  refusal is added; the units hint in `config.js`


**Resolution (2026-08-17): the leftover is the period's residual on both paths, and which kind of
difference it is follows from how the units were stated.**

The stated-units branch of `db_reinvest` no longer returns `(units, ZERO, ZERO)`: it computes
`available − units × price` like the whole-share branch, and the two share one `match handling`
that carries or pays it out. Cent-rounding the difference (the first attempt) turned out to be the
wrong discriminator — the real Morgan Stanley statements in
`morgan_stanley_ice_fractional_statements_reproduce` miss the cash by up to **5 cents**, because
0.500 units printed to 3 dp is a *rounded* allotment whose true fraction already spent that cash;
carrying it would double-count it. The discriminator is the units' own **scale**: a whole number is
an exact count (the plan bought whole units and left the rest over — cash), a figure stated to
decimals is a rounded one (the plan applied everything — printing, not money, so zero as before).
The one-unit-step tolerance is unchanged and is what bounds the whole-unit leftover below one
unit's price.

Not fixed, and deliberately: the *overspend* direction is still bounded only by that tolerance, so
stated units costing up to a unit's price **more** than the available cash are accepted with no
residual (15 units at $7 against $100). Tightening it needs a bound that does not reject a genuine
fractional statement — a separate question, noted here rather than guessed at.

Tests: `stated_whole_units_carry_the_cash_they_left_over` (14 units at $7 against $100 carries $2,
and the next reinvestment brings it forward), `stated_whole_units_pay_out_the_leftover_where_the_period_says_so`,
and the three fractional tests unchanged. `docs/API.md`'s "Stated allotments (`units`)" paragraph
and the reinvest form's units hint now state both halves.
## A reinvested distribution can be edited afterwards with nothing re-checked (SCENARIOS I-01, I-04, I-07)
(SCENARIOS.md section I verification pass, 2026-08-17. `income::db_upsert` deliberately never writes
`reinvestment_trade_id` — a client can't forge or drop the link — but it also never *looks* at it,
so every field the reinvest operation validated against can be changed underneath the DRP trade. This
is A-09/A-13's failure mode on the income side: a write path that reintroduces a state the operation
itself refuses.)
- [x] I-07 — reproduced, all four accepted `204` with the DRP trade untouched:
  **listing** moved to another listing (the link now crosses listings — the trade is a parcel of the
  old one, and `POST …/reinvest` would have refused the new one for want of an enrolment);
  **holding account** moved to an account with no enrolment (the trade stays in the old account's
  chain, and enrolment is per (listing, account));
  **ex date** moved outside every enrolment period (the reinvestment now rests on an enrolment that
  does not cover it — the very check that gated its creation);
  **cash amounts** changed from $100 to $200 (the trade still says 14 units and `carried 2`, figures
  computed from a distribution that no longer exists)
- [x] I-01/I-04 — the cash edit is the one that reaches a report: the parcel's cost base stays at
  the old cash while the assessable dividend becomes the new figure, so the ATO identity the whole
  operation rests on — "the acquisition cost is the amount of the dividends used to acquire them"
  (`docs/ato/cgt-dividend-reinvestment-plans.md`) — quietly stops holding, with no cross-check
  flagging it
- [x] Fix shape is A-09's, verbatim: re-check in `income::db_upsert`, in its own transaction, when
  the stored row has a `reinvestment_trade_id` — refuse a change to `listing_id`,
  `holding_account_id`, the entitlement inputs (`ex_date`/`entitlement_date`/`trust_income`) or any
  cash component, naming the field and pointing at `DELETE /income/:id/reinvest` (the operation's
  own undo, which already exists and is the documented way to redo a reinvestment). Non-load-bearing
  fields (`amount_per_security`, memo columns) stay editable
- [x] Tests: each of the four edits is refused `422` naming the rule with nothing persisted; the
  same edits stay allowed on a distribution with no reinvestment; undo → edit → re-reinvest works
- [x] Docs sync: `docs/API.md` Income (what is frozen while a distribution is reinvested) + the
  Response-codes `422` catalogue


**Resolution (2026-08-17): frozen at write time, the way a Buy is frozen while a Sell allocates
from it.**

`income::db_upsert` already read the stored row to reject a buy-back component; it now reads the
whole row (one `FromRow` select of `COLUMNS`, replacing the `buyback_trade_id` scalar) and, when
`reinvestment_trade_id` is set, refuses a change to any of the twelve figures the reinvestment used:
`listing_id`, `holding_account_id`, `currency`, `date_paid`, `ex_date`, `entitlement_date`,
`trust_income` and the five cash components. `UpsertError::ReinvestedIncome(&'static str)` carries
the field name and the 422 names it plus the undo that frees it. The notional and memo figures the
operation never read — franking credits, LIC, CFI, the per-share pair, `tax_deferred_amount` — stay
editable, which is what the two existing link-preservation tests now edit.

Tests: `a_reinvested_distribution_freezes_what_the_reinvestment_used` — the seven representative
edits each refused naming the field, nothing persisted, the 422 body naming the field and
`/reinvest`, and the same edit going through once the reinvestment is undone. `docs/API.md` (Income
+ the 422 catalogue) and `docs/SCHEMA.md`'s `reinvestment_trade_id` line state the freeze.
## A distribution in a currency other than its listing's is reinvested without conversion (SCENARIOS I-06, I-08)
(SCENARIOS.md section I verification pass, 2026-08-17. `db_reinvest` takes the cash from the income
row — in the income row's currency — and divides it by a price it stamps with the **listing's**
currency. Nothing checks the two agree. CLAUDE.md's rule is explicit: "Convert every non-AUD amount
to AUD using the record's `fx_rate` before aggregating or comparing — never mix currencies in one
calculation".)
- [x] I-08 — reproduced: AUD listing, income row `currency: "USD"` with `foreign_source_income: 100`
  → reinvest at `7` answers `201` with quantity **14** and `residual_carried_forward: 2` on an
  **AUD** trade. US$100 was divided by A$7; the parcel is costed A$98 for cash that was US$100
- [x] The mismatch is reachable because an income row's currency is free-form (the currencies FK
  aside) and is not tied to its listing's. Whether *that* should be constrained in general is a
  wider question than this section — but the reinvest operation is a single calculation over the
  two, and is where the mixing actually happens
- [x] **Decided 2026-08-17 (Evan): (a) refuse the reinvestment** when the distribution's currency
  differs from the listing's, naming both. Fails safe, one check, no FX policy invented: a registry
  paying a foreign-currency distribution into a plan converts it itself, and the converted figure is
  what the statement shows, so the user has it. (Rejected: converting at the ATO rate, which invents
  an FX policy the statement already settled; constraining `income.currency` to its listing's
  everywhere — the widest fix, noted as a question for a later pass rather than this section.)
- [x] Tests: a distribution whose currency differs from its listing's is refused `422` naming both
  currencies with nothing persisted; the matching-currency USD case
  (`morgan_stanley_ice_fractional_statements_reproduce`) is unchanged
- [x] Docs sync: `docs/API.md` reinvest + the Response-codes `422` catalogue


**Resolution (2026-08-17): refused, naming both currencies.**

`ReinvestError::CurrencyMismatch { distribution, listing }` is raised in `db_reinvest` beside the
listing-currency read it already does, before any arithmetic; the 422 body names both currencies
and says where to correct the entry (a registry reinvesting a foreign-currency payment converts it
and prints the converted figure). The check also caught the module's own fixtures: three tests set a
USD listing and left the distribution at the builder's default AUD, so `insert_distribution_dated`
now stamps the listing's currency and the ICE statements' rows are USD, as the statements are.

Tests: `a_distribution_in_another_currency_than_its_listing_is_refused` — the error variant, the 422
naming both currencies, and nothing persisted (no trade, no link).
## The partial-participation limitation names no workaround (SCENARIOS I-09)
(SCENARIOS.md section I verification pass, 2026-08-17. The Known limitation is honest — "enrolment
is all-or-nothing per (listing, holding account): a registry plan that reinvests only a portion of a
holding's units is not modelled" — and the system fails safe: stating the partial units is refused
`422` with both figures. What it doesn't say is what to do instead, which the scenario asks to
verify.)
- [x] I-09 — reproduced: a $100 distribution half reinvested, entered as `units: "7"` at $7 → `422`
  "the stated units at the reinvestment price spend 49, but the reinvestable cash … is 100". Good
  refusal, no guidance
- [x] I-09 — the workaround does produce a defensible cost base, verified end to end: split the
  distribution into two income rows — the reinvested $50 and the cash $50 — and reinvest the first.
  The parcel costs $49 for 7 units with $1 carried (the dividends actually applied, per
  `docs/ato/cgt-dividend-reinvestment-plans.md`), and the tax summary still declares the full $100
  as assessable dividend income. The per-share cross-check (`amount_per_security`/`securities_held`)
  has to be left off the split rows, since neither half reconciles against the whole holding
- [x] Caveat worth stating with it: an exactly half-and-half split trips the `duplicate_income`
  health warning (same listing, account, `date_paid` and *identical* amounts — G-24's key), so the
  banner reports a duplicate that is deliberate. Uneven splits don't
- [x] Fix: documentation only — extend the Known-limitations entry with the two-row workaround, the
  per-share-cross-check caveat and the duplicate-income note, and pin it in `doc_checks.rs` the way
  the other doc-only requirements are

**Resolution (2026-08-17): the limitation now names the workaround, and the workaround has a test.**

`docs/API.md`'s Known-limitations entry states the refusal, the two-row split (a reinvested row and
a cash row), why the result is defensible — the parcel is costed at the dividends actually applied
to it, `docs/ato/cgt-dividend-reinvestment-plans.md` — and both caveats: leave the per-share
cross-check off the halves, and an exactly even split reads as a duplicate to the health report.
The README's DRP feature line carries the short version.

Tests: `known_limitations_document_the_partial_drp_workaround` (doc_checks, the entry and its ATO
citation) and
`the_partial_participation_workaround_costs_the_parcel_at_the_cash_reinvested` — the refusal, then
the two-row entry producing a $49 cost base for 7 units with the full $100 still declared.

## The ESS vest Buy's FX rate is a hard-coded 1, so a foreign-currency vest can cost at parity (SCENARIOS J-08, J-12)
(SCENARIOS.md section J verification pass, 2026-08-18. `entities::ess_vest::db_vest` INSERTs the
cost-base-reset Buy with `fx_rate` literal `'1'`. On the trade that column is **not** a constant —
`infra::fx::pick_rate` treats it as `FxOverride::Fallback`, the rate used *when no ATO rate exists
for the month*. So the placeholder becomes a real answer exactly when the RBA rate is missing, and
the answer is 1 AUD per USD.)
- [x] J-12 — reproduced: a USD listing, statement `taxing_point_date 2024-09-01`, 100 shares at
  US$150, no `rba_fx_rates` row for `USD 2024-09`. `POST /ess_statements/1/vest` → `201` with
  `currency USD, fx_rate 1`, and `POST /portfolio/overview` answers `total_cost_base 15000` — a
  **US$15,000 parcel costed at A$15,000**. Importing the month's rate (0.65) moves it to
  A$23,076.92, so the figure was ~35% understated with nothing marked provisional
- [x] J-12 — the two sides disagree about the same missing month: `GET /portfolio/tax-summary`
  **500s** (`FxError::MissingRate`, documented in `docs/API.md` as "no rate ⇒ fails loudly with
  `500`") while the price-free CGT reports keep answering off the parity cost base. A user in this
  state sees the income report break and the capital-gains reports look fine
- [x] J-08 — an ICE-style US RSU release has nowhere to put the release-date spot rate on the CGT
  side at all. The statement-AUD overrides (`aud_deferral_discount` &c.) cover only the **income**
  labels; every other parcel-creating operation takes a rate — `inheritance.fx_rate` (its own
  column), `rights_exercise`'s `fx_rate` body field, `drp_reinvestment`'s `fx_rate` body field, and
  `domain::rollover` carries the consumed parcel's forward. The ESS vest is the only one that
  invents one
- [x] **Decide the model** (an `AskUserQuestion` for Evan, not a silent call). (a) **Give the
  statement an `fx_rate` column** the vest binds (default 1, refused ≤ 0, and — like
  `trades.spot_fx_rate` — only accepted on a non-AUD statement), so the taxpayer states the rate
  they used, matching `inheritance`. (b) **Bind `NULL`/no fallback** so a missing month fails loudly
  on the CGT side too, the way the income side already does — smallest change, but it leaves a
  correct-rate month with no way to record the spot rate the employer used. (c) **Bind
  `spot_fx_rate`** (the existing column, which *outranks* the ATO monthly rate) from a new statement
  field — the honest mapping for a release-date rate, but it changes the reported cost base for
  every month, not only missing ones. (a)+(b) together look right: a stated rate when the user has
  one, a loud failure when neither exists
- [x] Tests: a USD vest with the month's rate missing does not answer a parity cost base; with a
  stated rate it converts at it; an AUD statement rejects the rate field
- [x] Docs sync: `docs/SCHEMA.md` (`ess_statements`, `trades.fx_rate` on a vest Buy), `docs/API.md`
  (ESS statements + the 422 catalogue), README's ESS feature line

**Resolution (2026-08-18): Evan chose (a)+(b) — a stated rate when there is one, a loud failure when
there is neither.**

`ess_statements.fx_rate` (migration 0026, nullable TEXT decimal; the table is audited, so its two
`row_history` triggers are dropped and re-created with the column) is the foreign-per-AUD rate the
taxpayer states for the statement — refused ≤ 0, refused on an AUD statement, and frozen with the
rest of the vest-side fields once vested.

(b) took one adaptation worth recording: `trades.fx_rate` is `NOT NULL`, and making it nullable
would have rippled through ~330 references for one entity's case. Instead the *vest* resolves the
rate through the one precedence rule (`FxRates::resolve_rate`, loaded on its own transaction) and
binds a real one: the statement's stated rate when there is one, otherwise the taxing-point month's
ATO rate — and a non-AUD statement with neither is refused `422` (`VestError::MissingFxRate`,
naming the currency and month) instead of creating a parity parcel. The loud failure therefore
lands at write time, earlier and more actionable than a report-time 500, and no parcel can enter
the system costed at parity.

The stated rate also converts the **income** side (`reports::tax_summary`'s ESS loop and the annual
tax report's, via `aud_label`/`aud_field_with` taking an `FxOverride` and the shared
`tax_summary::ess_fx_override`), which closes the second finding: one statement's income and CGT
sides now convert at the same rate, and a statement with no rate anywhere fails loudly on both.

Tests: `a_foreign_vest_with_no_rate_anywhere_is_refused_rather_than_costed_at_parity` (the refusal,
nothing written, then A$23,076.92… once the month is imported),
`a_statements_stated_rate_costs_the_parcel`, `an_aud_vest_carries_the_parity_rate_because_aud_never_converts`,
`api_vest_without_a_rate_returns_422_naming_the_month`,
`db_fx_rate_must_be_positive_and_only_on_a_non_aud_statement`, `api_fx_rate_on_an_aud_statement_rejected_422`,
`a_vested_statements_fx_rate_is_frozen`, and
`db_ess_stated_fx_rate_converts_a_month_with_no_ato_rate` (the income side, and that an imported ATO
rate still wins). Docs: `docs/SCHEMA.md` (`ess_statements.fx_rate`, `trades.fx_rate` on a vest Buy),
`docs/API.md` (ESS statements, Vesting, the 422 catalogue), README's ESS feature line, and the field
hint in `src/web/config.js`.

## An ESS statement in a currency other than its listing's is vested without conversion (SCENARIOS J-08, J-12)
(SCENARIOS.md section J verification pass, 2026-08-18. The I-06/I-08 pattern on the ESS side:
`ess_statement::db_upsert` never compares `currency` with the listing's. `market_value_per_share` is
the market value of *that listed share*, so a statement whose currency is not the listing's is
either a data-entry slip or two currencies in one row — and the vest copies the statement's currency
onto the parcel regardless.)
- [x] J-08 — reproduced: an **AUD** ASX listing, statement `currency USD`, 100 shares at 150 →
  `204`, vest `201` with a **USD** parcel on an AUD-priced security. With `USD 2024-09` imported at
  0.65 the overview reports `total_cost_base 23076.92` for what the listing says is a A$15,000
  holding, and a later closing price (AUD, from the exchange) values a USD-costed parcel
- [x] Precedent: the DRP side already refuses this (`450b887`, "reinvesting … a distribution
  recorded in a currency other than its listing's (the cash and the per-unit price are one
  division, so they must be the same money)"). The same argument holds here: the per-share market
  value and the listed price are the same money
- [x] Fix: refuse at write time in `db_upsert` (`422` naming both currencies), the way the income
  reinvest path does — no model decision needed unless Evan wants the check on the *vest* instead
  (a statement can be entered before the listing exists in the right currency; the vest is the
  first point the currency reaches a parcel)
- [x] Tests: a statement whose currency differs from its listing's is refused `422`; the matching
  case is unaffected; an AUD listing with an AUD statement still vests
- [x] Docs sync: `docs/API.md` ESS statements + the 422 catalogue

**Resolution (2026-08-18): refused at write time, as the TODO's default said.**

`ess_statement::db_upsert` reads the listing's currency on its own transaction and answers `422`
(`UpsertError::CurrencyNotListings`) naming both currencies and saying what to do — convert the
employer's statement before entry, or choose the listing quoted in its currency. The check sits on
the statement rather than the vest deliberately: the statement is what the user typed, and an
income-only statement (never vested) reaches the tax summary too, so the slip must be caught there
as well.

Tests: `a_statement_in_another_currency_than_its_listing_is_refused` (the refusal, plus the same
statement accepted on a USD listing) and `api_currency_not_the_listings_rejected_422_naming_both`.
The existing foreign-currency ESS fixtures across the suite now hang off USD listings, which is the
shape a real statement has. Docs: `docs/API.md` (ESS statements + the 422 catalogue),
`docs/SCHEMA.md`'s `ess_statements.currency` line, README's ESS feature line, and the currency field
hint in `src/web/config.js`.

## The ESS vest bypasses the trade write-time checks, and creates a Buy `PUT /trades` refuses (SCENARIOS J-03, J-13)
(SCENARIOS.md section J verification pass, 2026-08-18. `db_vest` writes its Buy with a raw
`INSERT INTO trades`, not through `trade::db_upsert`, so `checks::check_amounts` never runs. Most of
that check is satisfied by construction — the vest enforces positive quantity and price itself, sets
`brokerage_currency = currency`, `fx_rate = 1`, and `settlement_date = date` — with exactly one
exception: `AmountsError::PreCgtDate`.)
- [x] J-13 — reproduced: a statement with `taxing_point_date 1985-01-01` is accepted (`204`) and
  vests (`201`) a Buy dated 1985-01-01. `PUT /trades/1` with that date answers `422`: "the trade is
  dated before 20 September 1985 — a pre-CGT holding is outside CGT and not modelled, so recording
  it would wrongly compute a capital gain or loss". The vest creates precisely the row the trade
  entity refuses, and the tax summary grows a `tax_year 1985` row
- [x] Nothing about ESS can genuinely predate 20 September 1985 (Division 83A dates from 2009, its
  predecessor from 1995), so this is a typo guard rather than a live case — but it is the one place
  a parcel can enter the system below the CGT floor, and A-series work has consistently closed those
- [x] Fix: reject a pre-CGT `taxing_point_date` in `ess_statement::db_upsert` (the earlier, better
  place: the statement is what the user typed), and state in `ess_vest`'s module doc which trade
  checks the vest satisfies by construction so the next reader can see the list is deliberate
- [x] Tests: a pre-CGT taxing point is refused `422`; 1985-09-20 itself is accepted
- [x] Docs sync: `docs/API.md` ESS statements + the 422 catalogue

**Resolution (2026-08-18): refused on the statement, with the vest's reliance written down.**

`ess_statement::validate` (new — see the section below, which landed in the same pass) refuses a
`taxing_point_date` before `trade::CGT_START` with `UpsertError::PreCgtTaxingPoint` → `422`. The
check sits on the statement, not the vest: the taxing point is what the user typed, and an
income-only statement never reaches the vest at all.

`ess_vest`'s module doc gained a **"The trade write-time checks, and where each is satisfied"**
section enumerating every `AmountsError` variant against what makes it impossible here —
`NothingToVest` (stronger than `QuantityNotPositive`/`PriceNegative`, since it rejects a nil price
too), the literal `'0'` brokerage and GST, `brokerage_currency` bound from the same `currency` value
in one statement, the statement-level `FxRateNotPositive` check plus positive-by-import ATO rates,
`settlement_date` bound to the trade date, and `PreCgtDate` now closed on the statement. It closes
with the standing instruction: a new check in `trade::check_amounts` needs a line there and either
an argument or a guard.

Tests: `db_a_pre_cgt_taxing_point_is_refused_and_the_cutoff_day_accepted` (1985-09-19 refused with
nothing persisted, 1985-09-20 accepted and round-tripped — the cutoff day is on the CGT side of the
line) and `api_pre_cgt_taxing_point_rejected_422` (the 422 body names the date, and no statement
exists to vest). Docs: `docs/API.md` (ESS statements' new write-time-rules list + the 422
catalogue), `docs/SCHEMA.md`'s `taxing_point_date` line, and the taxing-point field hint in
`src/web/config.js`.

## An ESS statement has no write-time checks on what it may say (SCENARIOS J-01, J-09, J-11)
(SCENARIOS.md section J verification pass, 2026-08-18. Section H's `investment_expenses` finding,
again: apart from the statement-AUD-override rule, `ess_statement::db_upsert` validates **nothing**
about its amounts. Every discount label, the foreign-source memo, the TFN withheld, the quantity and
the market value are taken as typed and reach the tax summary and the printed annual document
unchallenged.)
- [x] J-09 — reproduced: `deferral_discount -1000` with `tfn_withholding -50` → `204`. The tax
  summary reports `tfn_withholding_tax: "-50"` — negative withholding is a refund from nowhere,
  and the negative discount silently nets against the other statements' discounts in the same year
  (four statements totalling A$17,000 of positive labels reported `ess_discount_assessable 16000`)
- [x] J-01 — reproduced: `quantity -100`, `market_value_per_share -10` → `204` (the vest then
  refuses, `NothingToVest`, so the nonsense row simply sits there claiming income)
- [x] J-01 — reproduced: 100 shares at $10 (A$1,000 of market value) with `deferral_discount 15000`
  → `204`. The discount is *by definition* market value less what the employee paid
  (`docs/ato/employee-share-schemes.md`), so a discount above the vested shares' market value
  implies a negative payment. The obvious cause is a transposed column or a foreign-currency figure
  against an AUD market value — the check must only apply when both `quantity` and
  `market_value_per_share` are positive, since an income-only statement (no vest recorded) leaves
  them zero and is legitimate
- [x] J-11 — reproduced: `foreign_source_discount 5000` against `deferral_discount 1000` → `204`,
  and the tax summary reports `ess_foreign_source_discount 5000` inside a `ess_discount_assessable`
  of 1000. Label A is a **memo subset** of D+E+F+G (`docs/API.md`: "a memo already within
  `ess_discount_assessable`"), so a memo larger than what it is a memo of is a contradiction — the
  same shape as the CFI-within-unfranked check the income entity already enforces (`0a4e198`)
- [x] Fix (the H-section pattern, `db81aab`): refuse at write time, `422` per cause — negative
  discount label / TFN / quantity / market value; label A above D+E+F+G; and total discount above
  `quantity × market_value_per_share` when both are positive
- [x] Tests: one rejection test per cause, plus the income-only (zero quantity) statement still
  accepted and the exact-equality boundary (discount == market value, an RSU with nil consideration)
  accepted
- [x] Docs sync: `docs/API.md` ESS statements + the 422 catalogue, and the field hints in
  `src/web/config.js`

**Resolution (2026-08-18): refused at write time, the H-section pattern, in one `validate` pass.**

`ess_statement::validate(&EssStatement)` — everything decidable from the row alone, called first in
`db_upsert`, leaving the currency and vest-freeze rules that need the database where they were.
Four causes, four `UpsertError` variants, with the user-facing wording in the
`From<UpsertError> for ApiError` arms per the project's split between `#[error]` log text and 422
bodies:

- `NegativeAmount(&'static str)` — the `interest_income`/`investment_expense` shape, over **all
  thirteen** amounts on the row: quantity, market value, the four discount labels, the label-A memo,
  the TFN withheld, and the five statement-AUD overrides (which the tax summary reports *verbatim*,
  so a negative one is as load-bearing as a native label). Checked first, so a negative figure gets
  the message naming its field rather than tripping a cross-check confusingly.
- `PreCgtTaxingPoint` — the section above.
- `ForeignSourceExceedsDiscounts { label, foreign, discounts }` — the income-entity
  CFI-within-unfranked rule: label A cannot exceed the D+E+F+G it is a memo of. Applied twice, hence
  the `label` field: once to the statement's own amounts, and once to `aud_foreign_source_discount`
  against the other four overrides. The AUD side is checked **only where that total is knowable** —
  `aud_discount_labels` returns `None` when a label carries an amount with no override, since it
  converts at the RBA rate, which no write-time check can resolve. Equality is ordinary (a wholly
  foreign-sourced discount) and stays accepted.
- `DiscountExceedsMarketValue { discount, market_value }` — D+E+F+G cannot exceed
  `quantity × market_value_per_share`, both cent-rounded (a per-share market value can carry
  sub-cent precision while the statement's discount is cents), and only when both figures are
  positive. Exact equality is the RSU case (nil consideration) and is the *normal* entry, not an
  edge: the ATO's own Example 11 (400 × $3.795 = $1,518 of discount) is pinned in `ato_examples`.

One existing test moved with the rule rather than around it:
`ess_vest::tests::deleting_the_statement_removes_the_vest_buy` revised its statement's discount
*down* (600 → 500) instead of up past the vested shares' $600 market value — still the point being
made (the income side stays editable after the vest), now within the invariant.

Tests: `db_negative_amounts_are_refused_naming_the_field` (a sweep over all thirteen fields, each
refused naming itself with nothing persisted), `api_negative_tfn_withholding_rejected_422`,
`db_the_foreign_source_memo_cannot_exceed_the_discounts_it_memos` (refused with both figures
carried; equality accepted), `db_the_aud_foreign_source_memo_is_checked_only_when_the_total_is_known`
(refused on the override total; within it accepted; unknowable-total statement passes unchecked),
`db_the_discount_cannot_exceed_the_market_value_that_vests` (the J-01 15,000-against-1,000 case, the
nil-consideration equality, and the sub-cent 400 × 3.795 = 1518 case),
`db_an_income_only_statement_keeps_its_discount`, and
`api_discount_above_the_market_value_rejected_422`. Full suite 1603 passed / 0 failed.

Docs: `docs/API.md` gained a **What a statement may say** list under ESS statements (one bullet per
rule, each with the *why*) plus the new causes in the Response-codes `422` catalogue;
`docs/SCHEMA.md`'s `taxing_point_date` / `quantity` / `market_value_per_share` /
`foreign_source_discount` / `tfn_withholding` / `aud_foreign_source_discount` column lines now state
their constraints; and `src/web/config.js`'s ESS field hints say the ceiling, the memo-subset rule,
the pre-CGT floor, and that zero quantity means an income-only statement.

## A duplicated ESS statement is caught by nothing (SCENARIOS J-11)
(SCENARIOS.md section J verification pass, 2026-08-18. `reports::health` warns on duplicate
corporate actions (E-03), AMMA statements (F), income (G-24), interest and expenses (H) —
`ess_statements` is the one income-bearing fact table with no such check.)
- [x] J-11 — reproduced: the same statement entered twice (same listing, account, taxing point,
  quantity, market value and discount) is accepted, vests **two** parcels, and doubles both the
  Item 12 discount (`ess_discount_assessable 2000` for a $1,000 grant) and the holding
  (`quantity 200`). The health report answers with every list empty
- [x] The 30-day rule makes this the *expected* accident rather than a hypothetical: the employer
  issues an **amended** statement for the same vest (`docs/ato/ess-30-day-rule.md` — an amended 2019
  statement and a new 2020 one for one grant), and a user who enters both has exactly this shape
- [x] J-11 — the legitimate case must stay silent: two vests on the same date from different grants
  are ordinary. The G-24 key (identical amounts as part of the key, grouped in Rust because the
  amounts are TEXT decimals SQL would compare as strings) already handles that — differing
  quantities or discounts are not duplicates
- [x] Fix: `duplicate_ess_statements` in `reports::health` + the UI banner, keyed on listing,
  holding account, `taxing_point_date` *and* identical quantity / market value / discount labels
- [x] Tests: the doubled statement is reported with both ids; two same-date statements from
  different grants (different quantity or discount) are not
- [x] Docs sync: `docs/API.md` health report, README

**Closed 2026-08-18.** `reports::health` gained a sixth duplicate list, `duplicate_ess_statements`,
built on the `db_duplicate_income` shape rather than the SQL-grouped `db_duplicate_actions` one: the
figures are part of the key and they are TEXT decimals SQL would compare as strings, so the SELECT
narrows to rows already sharing a (listing, holding account, `taxing_point_date`) with another row
and the fingerprint match happens in Rust over `Decimal`s (`same_ess_entry`) — `1000.0` and
`1000.00` are the one grant however two clients wrote them. It selects `ess_statement::COLUMNS`
rather than `*`, since `vest_trade_id` is a derived back-link the row mapping requires.

Three decisions worth recording:
- **Every stored money field is in the key**, including `fx_rate` and the five statement-AUD
  overrides — the `same_income_entry` precedent (two rows agreeing on the assessable figures but
  not on an informational one came off different statements). Two same-date statements differing in
  quantity, market value or any discount label are two grants vesting the same day, which is
  ordinary and stays silent.
- **`vest_trade_id` is deliberately *not* in the key.** It is derived, not stored, and whether the
  surplus statement has been vested yet says nothing about whether it is a duplicate — the pair must
  still be reported after vesting, which is exactly when the doubled *holding* exists.
- **`discount_total` reuses `ess_statement::discount_labels`** (D+E+F+G), promoted from private to
  `pub(crate)` rather than re-summed here, so the warning and the tax summary can never disagree on
  what "the discount" is.

Tests (`reports::health`): `duplicated_ess_statements_are_reported_with_their_ids` (two grants on
two listings, newest taxing point first, both ids ascending, `discount_total` naming the grant),
`ess_statements_differing_in_any_key_field_are_not_duplicates` (a sweep over listing, holding
account, taxing point, quantity and discount — six statements, nothing reported),
`ess_figures_equal_in_value_but_not_in_text_are_still_duplicates`, and
`a_duplicated_ess_pair_is_reported_beside_a_second_grant_and_after_vesting` (the pair survives both
an unrelated same-day tranche and `ess_vest::db_vest`). The empty-database assertion gained the new
list, and `web.rs`'s health-banner bundle test pins the wording and the `#/e/ess_statements` link.
Full suite 1607 passed / 0 failed; `node --test` 69 passed; ui-smoke green. Verified end to end
against a running server: two identical statements entered over HTTP produce one
`duplicate_ess_statements` row naming ids [1, 2].

Docs: `docs/API.md`'s Health section gained the `duplicate_ess_statements` bullet (and the field in
the response shape), README's job/data-freshness feature bullet gained the duplicated-ESS-statement
clause.

## Nothing on the product side mentions the ESS 30-day rule (SCENARIOS J-04)
(SCENARIOS.md section J verification pass, 2026-08-18. A disposal within 30 days after the deferred
taxing point **moves the taxing point to the disposal date**: the discount is re-measured at the
proceeds and the cost base resets to the same figure, so there is no separate capital gain, and the
discount can move into the next financial year — `docs/ato/ess-30-day-rule.md`, QC 23058 Example 11.
The mirror is indexed in `docs/ato/OVERVIEW.md`, but the words "30-day rule" appear nowhere in
`README.md`, `docs/API.md`, or the ESS screen, and no report flags the pattern.)
- [x] The corrected entry works and is now pinned: `ato_examples::ess_30_day_rule_example_11_wyatt_amended_statement`
  enters the *amended* statement (taxing point = the 20 July 2019 disposal, market value = the
  $3.795 per-share sale price), vests it, and sells the same day — FY2020 discount $1,518, capital
  gain $0, exactly the ATO's answer. `docs/ato/OVERVIEW.md` already claimed this test existed; it
  does now
- [x] J-04 — the *natural* entry is wrong in two ways at once and nothing says so. Entering the
  employer's original statement (taxing point 23 June 2019, discount $1,400) and then the 20 July
  sale gives `ess_discount_assessable 1400` in **FY2019** and a **$118 capital gain** in FY2020 —
  where the ATO's answer is $1,518 of discount in FY2020 and no capital gain. Both figures are
  wrong, in different years, from an entry the system accepts without comment
- [x] The trigger is mechanically detectable from data already held: a Sell allocating a parcel
  whose Buy carries `ess_statement_id`, dated within 30 days after that statement's
  `taxing_point_date`. `reports::wash_sales` is the precedent for an advisory, non-blocking
  date-pattern report, and `reports::health` for a banner
- [x] **Decide the model.** (a) **Documentation only** — a Known-limitations entry plus a hint on
  the ESS screen's taxing-point field saying an amended statement supersedes the original (cheapest,
  and the G-14 precedent for a scope cut honestly stated). (b) **Plus an advisory alert** — a
  `ess_30_day_rule` list in `reports::health` (or its own cross-check report) naming each sale
  within the window and the statement it draws on, so the case is caught rather than remembered.
  (c) **Re-measure automatically** — rejected in advance: the system cannot know whether the
  employer issued an amended statement, and rewriting a user's stated discount would be a
  calculation the ATO puts on the employer
- [x] Tests: whichever of (a)/(b) is chosen — a `doc_checks` assertion for the wording, and/or an
  alert test with a sale on day 30 and day 31 either side of the boundary
- [x] Docs sync: `docs/API.md` Known limitations (+ the report, if (b)), README Features

**Closed 2026-08-18 — Evan chose (b), documentation *plus* an advisory alert.** (c) stayed rejected
for the reason the finding gave: the re-measurement is a calculation the ATO puts on the employer,
and no stored fact says whether an amended statement was issued.

The alert is `ess_30_day_rule` in `reports::health` — the sixth list on that report and the first
that is a **date pattern** rather than a double entry, which is why it went there rather than into
its own report: it takes no parameters and belongs on the same cross-view banner, since the point is
to catch the case at entry time rather than at return time. `reports::wash_sales` remains the
precedent for the advisory posture (nothing rejected, nothing rewritten).

Four decisions worth recording:
- **The window is 1..=30 days, not 0..=30.** A sale *on* the taxing point is never flagged: the
  rule's effect is a no-op there (the taxing point already is the disposal date), and that is
  precisely the shape of the **corrected** entry — the amended statement vested and sold the same
  day, which `ato_examples::ess_30_day_rule_example_11_wyatt_amended_statement` enters. Flagging day
  0 would have nagged on the only entry that is right. The upper bound is statutory (ITAA 1997
  s 83A-115(3)), so `ESS_THIRTY_DAY_WINDOW` is a constant interpolated into the SQL rather than a
  request parameter like the wash-sale window, which is only a review convention.
- **Both financial years are surfaced** (`statement_tax_year`, `disposal_tax_year`). The rule's
  costliest consequence is that the discount can move *years*, so the alert names where it is
  assessed today and where it belongs; the banner only mentions the move when the two differ, since
  a window inside one financial year is the common case.
- **One row per allocation, not per sale.** A Sell drawing on two vest parcels inside their windows
  is two alerts — each statement is amended separately — and `units_sold` is what that allocation
  consumed, not the whole vest.
- **Two reads rather than one join**, so `statement_discount` is summed by
  `ess_statement::discount_labels` (the tax summary's own definition of the discount) instead of
  being re-added over TEXT columns in the report.

Tests: `a_sale_inside_the_thirty_day_window_is_flagged_with_both_years` (Example 11's own dates and
figures — 27 days, FY2019 → FY2020), `the_window_includes_day_thirty_and_excludes_day_thirty_one`,
`a_same_day_sale_and_a_non_ess_parcel_are_not_flagged` (the corrected entry stays silent, and an
ordinary parcel is none of this check's business), `each_vest_parcel_a_sale_draws_on_is_named_separately`,
the empty-database assertion, `doc_checks::known_limitations_document_the_ess_30_day_rule`, and the
`web.rs` bundle assertions for the banner text and the form hint. Full suite 1612 passed / 0 failed;
`node --test` 69 passed; ui-smoke green. Verified end to end against a running server seeded with
Example 11: the banner reads "Sale of 400 PEPP ESS shares on 2019-07-20 is 27 day(s) after the taxing
point of statement 1 … moves from FY2019 to FY2020 … no separate capital gain."

One test-fixture gotcha found the hard way: a vest Buy's id is assigned **max+1**, so every vest a
test needs must be created *before* its sells — otherwise the next vest lands on top of a sell just
inserted, and `trade::db_upsert` refuses with `EssVestTrade`.

Docs: `docs/API.md` gained the `ess_30_day_rule` field on the Health report and a **The ESS 30-day
rule is flagged, never applied** Known-limitations entry (what the rule does, why the tool won't do
it, and what to enter instead); README's health-monitoring feature line gained the clause; and
`config.js`'s taxing-point hint now states the rule where the date is typed.

## The $1,000 taxed-upfront reduction is always applied, with no way to record failing the income test (SCENARIOS J-02)
(SCENARIOS.md section J verification pass, 2026-08-18. The reduction is available only if *adjusted
taxable income* is ≤ A$180,000 — a taxpayer-level test outside this system's data
(`docs/ato/employee-share-schemes.md`). The tool applies `min(A$1,000, D)` unconditionally and
documents the test as the user's responsibility in `README.md`, `docs/API.md` (both the tax-summary
section and Known limitations) and the ESS screen description — thorough, and the applied amount is
surfaced as its own `ess_taxed_upfront_reduction` line so it can be added back by hand.)
- [x] J-02 — the gap is that "add it back by hand" has no home in the system: there is no
  per-taxpayer or per-year flag, and the only way to make the summary report the right figure is to
  enter the discount at label **E** (taxed-upfront *not eligible*), which misstates 12D/12E to get
  12B right. An ineligible taxpayer's every stored figure and export stays $1,000 light
- [x] J-02 — the printed archival document (`/reports/tax-report`, the PDF the accountant gets)
  prints `ess_taxed_upfront_reduction 1,000` as a bare line with an empty ATO label and no statement
  of the condition it assumes. `taxreport.js` already carries the precedent for exactly this: the
  CFI footnote (`cfiFootnote`) explains a figure the reader would otherwise misread
- [x] **Decide the model.** (a) **A footnote only** — print the ≤A$180,000 condition under the ESS
  table whenever a reduction was applied (cheap, honest, matches the CFI precedent). (b) **Plus a
  `cgt_settings` flag** — the singleton settings entity already carries a taxpayer-level fact (the
  opening capital loss); an `ess_taxed_upfront_reduction_eligible` boolean (default true) would let
  the summary report the ineligible position and keep the exports right. (c) **Per-year** rather
  than singleton, since the income test is answered year by year — more faithful, and the only one
  that survives a year where the taxpayer crosses $180,000; costs a new dated settings table
- [x] Tests: whichever is chosen — a `doc_checks`/bundle assertion for the footnote wording, and a
  summary test that an ineligible year reports the unreduced discount
- [x] Docs sync: `docs/API.md` tax summary + Known limitations, README

**Closed 2026-08-18 — Evan chose (c), the per-year flag, plus (a)'s footnote.** The singleton (b) was
rejected for a concrete reason rather than a stylistic one: the tax summary reports **every** recorded
year in one response, so a global flag would strip the reduction from years that never crossed
A$180,000 — wrong for any taxpayer whose income crosses the threshold partway through their recorded
history, which is the ordinary case over a working life.

New table `tax_year_settings` (migration 0027), keyed on the financial year itself, with a matching
entity (`entities::tax_year_settings`), CRUD routes, and a UI screen. Design decisions worth
recording:
- **Absent row = eligible.** The flag defaults true, the reader is an *exception list*
  (`db_ineligible_tax_years`), and an omitted field on a PUT means eligible — so an empty table
  behaves exactly as the system did before, no existing database's figures move, and no request that
  forgets the field can silently remove a reduction.
- **No surrogate key**, unlike closing_prices (0021), which needed one to join the audit trail:
  `tax_year` is already an integer identity and is what `row_id` records. It is never reused for a
  different fact either — deleting FY2026's settings and entering them again is the *same*
  taxpayer-year fact, so inheriting that year's own trail is right rather than a leak. The table is
  audited for the same reason `cgt_settings` is (a taxpayer-level fact that changes an assessable
  total), which meant rebuilding `row_history` once more to extend its `table_name` CHECK — the live
  append-only guards now come from 0027.
- **Named for the shape, not the field.** It is `tax_year_settings`, not
  `ess_reduction_eligibility`: it is the per-year counterpart of the `cgt_settings` singleton, and the
  next taxpayer fact answered year by year is a column here rather than a fourth settings table.
- The **footnote is printed regardless of the flag**, whenever a reduction was actually applied
  (`taxreport.js`'s `essReductionFootnote`, on the `cfiFootnote` precedent): the archived document
  states the condition it rests on and names where to record the other answer. An ineligible year
  applies no reduction, so no footnote prints — there is nothing conditional left to disclose.

One test-harness change came with it: `row_history`'s end-to-end audit sweep keyed every case on
`id`, which this table does not have, so its case tuples now carry the key column.

Tests: `tax_year_settings`'s own module (round-trip and in-place replace, the ineligible-only
exception list, the CRUD API round trip, the omitted-flag default, and the pre-1986 rejection naming
the year), `tax_summary::db_ess_reduction_is_withheld_from_a_year_recorded_ineligible` (two years,
only the flagged one unreduced — the whole reason the flag is per year) and
`db_a_year_recorded_eligible_keeps_its_reduction`, the row-history sweep's new case and its 0027
migration assertions, `web.rs::tax_year_settings_ui_present`, and
`doc_checks::per_year_ess_reduction_eligibility_documented`. Full suite 1621 passed / 0 failed;
`node --test` 69 passed; ui-smoke green. Verified end to end against a running server: the same
statement reports `ess_discount_assessable 1400 / reduction 1000` by default, `2400 / 0` once FY2026
is marked ineligible, back to `1400 / 1000` when re-marked eligible, `422` naming the year for
FY1985, and the flip recorded in `row_history`. The tax-report line the footnote keys on reads
`1000` then `0` across the same flip.

Docs: `docs/SCHEMA.md` gained the table (and corrected the `row_history` CHECK enum, which was still
missing `closing_prices`), `docs/API.md` a **Tax year settings** entity section plus the rewritten
tax-summary and Known-limitations wording and the new 422, README the recorded-per-year clause.

## The documented dividend-equivalent workaround reports remuneration as a dividend (SCENARIOS J-10)
(SCENARIOS.md section J verification pass, 2026-08-18. A dividend equivalent paid on unvested RSUs
is **ordinary income as remuneration** under s 6-5 — "not a dividend in the employee's hands", not
part of the ESS discount, and carrying no franking (TD 2017/26,
`docs/ato/ess-dividend-equivalents.md`). `docs/API.md` Known limitations tells the user it is
"enterable manually as an [income](#income) row if the user wants it aggregated here".)
- [x] J-10 — reproduced: that row (`unfranked_amount 250` against the employer's listing) reports as
  `dividends_assessable 250` — **item 11S, unfranked dividends** — counts in
  `gross_assessable_investment_income`, and prints in the annual document's **Dividend income**
  table with `franking_status "entitled"`. The one place the amount belongs (salary and wages,
  item 1/2) is not somewhere this system reports at all
- [x] The workaround is not wrong so much as unlabelled: aggregating the cash here is fine, but
  nothing tells the reader the row will be **called a dividend** by every surface it reaches, and
  the printed document is the one that goes to an accountant
- [x] **Decide the model.** (a) **Sharpen the documentation** — say plainly that an income row
  reports at 11S and that the amount must be moved to salary/wages in the return, or say don't enter
  it here at all (cheapest; keeps the data model unchanged). (b) **Give income rows a kind** — an
  `income_type` enum (dividend / other) whose non-dividend value reports on its own tax-summary line
  and prints in its own table; correct, but it touches an audited table, the tax summary, the export
  header and the printed document. (a) looks proportionate for a payment the system deliberately
  does not model
- [x] Tests: a `doc_checks` assertion for the wording (the H/G precedent for documentation-only
  requirements)
- [x] Docs sync: `docs/API.md` Known limitations (the RSU dividend-equivalents entry), README

**Closed 2026-08-18 — Evan chose (b), the `income_type` enum**, over the documentation-only cut the
finding itself leaned towards. So the row now says what it is instead of the docs warning what it
will be called.

`income_type` (migration 0028) is `Dividend` (the default, and what every existing row is — no
stored figure moves) or `EmploymentIncome`. Decisions worth recording:
- **Named for the case, not `Other`.** The option said "dividend / other", but the point of the kind
  is to say *where the amount belongs on the return*, and only a named kind can carry that: this one
  is item 1/2, salary and wages. The enum is the extension point, exactly as
  `corporate_actions.action_type` is — a further non-dividend kind is a new value, not a second flag.
- **Orthogonal to `trust_income`, not folded into it.** A single three-valued kind
  (Dividend/Trust/Employment) would be cleaner on paper, but `trust_income` drives assessability
  timing, the AMIT rules and the franking exemption across the whole codebase; rewriting it would
  touch every one of those for no gain on this finding. A write-time rule keeps the two consistent
  instead: an EmploymentIncome row can never be trust income.
- **The cash goes in `unfranked_amount`, and nothing else is allowed.** Every dividend-shaped field —
  franking, foreign-source, LIC, CFI, tax-deferred, ex/entitlement dates, the per-share pair — is
  refused `422` naming itself, and the check runs *before* the per-share cross-check so the message
  names the kind rather than the confusing "supply both or neither". It is also not reinvestable: a
  DRP reinvests a payment *of* the holding, and remuneration is paid for services.
- **The tax-summary line is informational.** `employment_income` carries an empty ATO label and joins
  no assessable total. The amount belongs at item 1/2, which the ATO normally prefills from the
  employer's STP reporting — reporting it as assessable here would invite entering it twice, so the
  line exists to reconcile the cash.
- **Left out of performance's income yield** as well: remuneration is not a return *on* the holding,
  and counting it would inflate the yield of whatever listing it was recorded against. Kind also
  joins the health report's duplicate-income key — a dividend and a dividend equivalent of the same
  amount on one day are two different payments.

Tests: `income`'s own module (the kind defaults to Dividend; an employment-income row round-trips
with the cash alone; a sweep over all thirteen distribution fields, each refused naming itself with
nothing persisted), `tax_summary::db_employment_income_is_not_a_dividend_and_not_investment_income`
(the dividend beside it unaffected, gross assessable investment income unmoved),
`tax_report::employment_income_prints_in_its_own_table_not_among_the_dividends`,
`web.rs::income_type_ui_present`, and the updated
`doc_checks::known_limitations_document_rsu_dividend_equivalents`. Full suite 1627 passed / 0 failed;
`node --test` 69 passed; ui-smoke green. Verified end to end against a running server: a $100
dividend and a $250 dividend equivalent on one day report `dividends_assessable 100 /
employment_income 250 / gross_assessable_investment_income 100`; the printed document lists the
dividend and the equivalent in separate tables; a franking credit on the equivalent is `422`;
reinvesting it is `422`; the activity ledger reads "Employment income (dividend equivalent)".

One thing this does **not** fix, and the Known-limitations entry now says so plainly: salary and
wages (item 1/2) is still not somewhere this system reports. The kind stops every surface calling
the payment a dividend; it does not put the amount where it belongs on the return.

Docs: `docs/SCHEMA.md` gained the column, `docs/API.md` an `income_type` section under Income plus
the rewritten Known-limitations entry, the tax-summary unlabelled-columns list and the 422 catalogue,
README the recordable-payment clause. `docs/ato/ess-dividend-equivalents.md`'s **How this project
uses it** section (the project's own note, not mirrored ATO text) was updated to match.

## The inheritance's parcel Buy bypasses the trade write-time checks (SCENARIOS K-01, K-02, K-04)
(SCENARIOS.md section K verification pass, 2026-08-18. J-03/J-13's finding on the inheritance side:
`inheritance::db_upsert` writes its parcel Buy with a raw `INSERT INTO trades`, not through
`trade::db_upsert`, so neither `checks::check_amounts` nor the return-of-capital currency
cross-check runs. `validate()` covers the quantity, the two amounts, the dates and the rule pairing
— it says nothing at all about `fx_rate` or the currency, and both gaps land as a **500**, not a
wrong figure.)
- [x] Reproduced — `fx_rate: "0"`: `PUT /inheritances/1` with `currency USD, fx_rate 0` → `204`, and
  the Buy is stored with `fx_rate 0`. `GET /portfolio/open-parcels` then **panics** —
  `rust_decimal … Division by zero` inside `infra::fx::apply_rate` (`AUD = foreign / rate`) — so the
  report answers `500` and every price-free CGT report on that listing is unusable until the row is
  found and fixed. A negative rate (`-0.65`) is accepted the same way. `PUT /trades` refuses both:
  "fx_rate must be a positive foreign-per-AUD rate (1 for an AUD trade)"
- [x] Reproduced — the return-of-capital currency cross-check: a USD listing carrying an **AUD**
  `ReturnOfCapital`. `PUT /trades` refuses a USD Buy of it with the full
  `PaymentCurrencyMismatch` explanation ("a payment reduces each parcel's cost base in the parcel's
  own currency, and amounts are never netted across currencies, so the two must agree"). The same
  parcel entered as an inheritance is accepted `204`, and `GET /portfolio/open-parcels` answers
  `500` — the pipeline's own loud failure, fired at *read* time on every request instead of once at
  write time with a message naming the fix
- [x] The pre-CGT floor — the third thing `check_amounts` enforces — is the one the inheritance path
  already covers itself (`DeathPreCgt`, and the Buy is dated the death), so it is not at issue here
- [x] Fix: route the Buy through `trade::db_upsert` (the shape `4b77972` gave the ESS vest), or at
  minimum add both checks to `validate()`; either way state in the module doc which trade checks the
  inheritance satisfies by construction, so the list is visibly deliberate
- [x] Tests: a non-positive `fx_rate` is refused `422` (not a 500 from a later report); an
  inheritance whose currency conflicts with a return of capital on its listing is refused at write
  time with the same wording `PUT /trades` gives
- [x] Docs sync: `docs/API.md` Inheritances + the 422 catalogue

**Resolution (2026-08-18): both checks land on the inheritance, with the vest's reliance written down.**

`validate` now refuses a non-positive `fx_rate` (`UpsertError::FxRateNotPositive` → `422`, the same
wording `PUT /trades` gives: "fx_rate must be a positive foreign-per-AUD rate"), and `db_upsert`
runs `corporate_action::db_payment_currency_conflict` over the written state inside its own
transaction — the same call, in the same position, `trade::db_upsert` makes — answering
`UpsertError::PaymentCurrencyMismatch` with the trade path's verbatim body, which names the payment
date and both currencies. The Buy stays a direct `INSERT`: routing it through `trade::db_upsert`
is not available, because the trade write paths refuse an inheritance-linked row outright.

`inheritance`'s module doc gained the **"The trade write-time checks, and where each is satisfied"**
section `ess_vest` carries, enumerating every `AmountsError` variant against what makes it
impossible here — `QuantityNotPositive` (the same rule on the inherited unit count), the literal
`'0'` price and GST, the brokerage bound from the cost base `NegativeAmount` already guards,
`brokerage_currency` bound from the same `currency` value in one statement, the new
`FxRateNotPositive`, `settlement_date` bound to the trade date, and `PreCgtDate` closed by
`DeathPreCgt` on the date the Buy is dated. It closes with the same standing instruction: a new
check in `trade::check_amounts` needs a line there and either an argument or a guard.

Tests: `the_parcel_buys_trade_checks_are_enforced_here` — `fx_rate` `0` and `-0.65` both refused
with nothing persisted (the zero was a *panic*, `rust_decimal` "Division by zero" inside
`infra::fx::apply_rate`, so every cost-base report of the listing answered `500`), a USD inheritance
of a listing carrying an AUD return of capital refused with both currencies named, and
`GET /portfolio/open-parcels` still `200` afterwards. Docs: `docs/API.md` (Inheritances' `422` list,
the parcel-Buy paragraph, and the 422 catalogue).

## A non-AUD inheritance with no rate is costed at parity (SCENARIOS K-01, K-04)
(SCENARIOS.md section K verification pass, 2026-08-18. J-08/J-12 exactly, one entity along.
`inheritances.fx_rate` defaults to `1` — `InheritanceBody.fx_rate` is `Option<Decimal>` with
`unwrap_or(Decimal::ONE)`, where `TradeBody.fx_rate` is a **required** field. On the parcel that
column is not a constant: `infra::fx::pick_rate` treats it as `FxOverride::Fallback`, the rate used
*when no ATO rate exists for the month*. So the default becomes a real answer exactly when the rate
is missing, and the answer is 1 AUD per USD.)
- [x] Reproduced: a USD listing, `cost_base 3000`, `currency USD`, no `fx_rate` given and no
  `rba_fx_rates` row for the acquisition month → `204`, and `GET /portfolio/open-parcels` reports
  `original_cost_base 3000` — a **US$3,000 parcel costed at A$3,000**, with nothing marked
  provisional
- [x] The exposure is larger here than it was for ESS, because the translation month is the
  *parcel's* (`ParcelRow::acquired()`): under `DeceasedCostBase` that is the **deceased's**
  acquisition month, which for an inherited holding is routinely decades before anything the RBA
  import covers. The missing-rate case is the normal case, not the edge case
- [x] Precedent to copy: `ef479dd` gave `ess_statements` an `fx_rate` column and made the vest bind
  the statement's stated rate, else the taxing-point month's ATO rate, else refuse
  ("vesting an ESS statement whose currency has no imported ATO rate for the taxing point's month
  and that states no `fx_rate` of its own (the parcel would be costed at parity)"). The column
  already exists here; what is missing is the refusal
- [x] Fix: at write time, when `currency` is not AUD and no `fx_rate` was **stated**, resolve the
  acquisition month's ATO rate and refuse `422` when there is none. Note the wrinkle the ESS fix did
  not have: `fx_rate` defaults to 1 rather than being absent, so "stated" has to be distinguishable
  from "defaulted" — either make the body field required for a non-AUD inheritance, or refuse
  `fx_rate = 1` on a non-AUD row (the honest reading: parity is not a rate anyone states)
- [x] Tests: a non-AUD inheritance with neither a stated rate nor an imported month is refused; with
  a stated rate it converts at it; with the month imported it converts at the ATO rate; an AUD
  inheritance is unaffected
- [x] Docs sync: `docs/API.md` Inheritances + the 422 catalogue, `docs/SCHEMA.md` (`inheritances.fx_rate`)

**Resolution (2026-08-18): refused, on the deceased's acquisition month.**

`db_upsert` runs `check_convertible` first, inside its own transaction: a non-AUD inheritance
whose `fx_rate` is still the default 1 and whose **conversion month** has no `rba_fx_rates` row is
refused with `UpsertError::MissingFxRate` → `422`, naming the currency and the month and saying
what would otherwise happen ("would cost the parcel at parity (1 AUD per USD)"). Everything else
passes straight through — an AUD inheritance, a stated rate, or an imported month.

The month is the one the cost base actually converts at, spelled out as `conversion_month`:
`ParcelRow::acquired()`'s rule on the inheritance's own fields — the deceased's acquisition under
`DeceasedCostBase`, the death under `MarketValueAtDeath`. That is the substantive difference from
the ESS fix this copies: the ESS taxing point is recent, while an inherited parcel converts at a
month decades old, so the rate is usually the taxpayer's to state rather than the import's to
supply.

The "stated" / "defaulted" ambiguity the section raised is answered by testing the condition rather
than the provenance: the check fires only where the fallback would actually be *used*, so a
`fx_rate` of 1 on a non-AUD row is refused exactly when it would become the answer, and is
harmless (and accepted) when the month's ATO rate exists to outrank it. No migration, and no new
required body field.

Tests: `a_non_aud_inheritance_with_no_rate_is_refused_not_costed_at_parity` — a USD inheritance with
only the *death* month imported is refused naming `2020-02` (the deceased's acquisition month) with
nothing persisted; a stated 0.75 converts US$3,200 to A$4,266.67; and importing `2020-02` at 0.80
lets the same row through at A$4,000, the ATO rate outranking the fallback. Docs: `docs/API.md`
(Inheritances' `fx_rate` paragraph, the `422` list, the 422 catalogue) and the field hint in
`src/web/config.js`.

## An inheritance recorded in a currency other than its listing's rides through to the parcel (SCENARIOS K-01)
(SCENARIOS.md section K verification pass, 2026-08-18. `inheritance::db_upsert` never compares
`currency` with the listing's, and the linked Buy takes the inheritance's currency verbatim. The
same finding closed for ESS statements in `ef479dd` and for DRP distributions in `450b887`.)
- [x] Reproduced: an **AUD** listing with `currency: "USD"` on the inheritance → `204`, and the
  parcel is a USD-costed holding of an AUD-priced security. Any closing price for it comes from the
  exchange in AUD, so the unrealised-gains and portfolio screens compare a USD cost base against an
  AUD market value
- [x] The argument is the one already accepted twice: a parcel's cost base and its market price are
  the same money. For an inheritance it is sharper still — under `MarketValueAtDeath` the figure
  entered *is* a market value of that listed security
- [x] Fix: refuse at write time in `db_upsert`, `422` naming both currencies, matching
  `ess_statement`'s wording ("the per-share market value and the listed price are the same money")
- [x] Tests: an inheritance whose currency differs from its listing's is refused; the matching case
  is unaffected
- [x] Docs sync: `docs/API.md` Inheritances + the 422 catalogue

**Resolution (2026-08-18): refused at write time, the ESS statement's wording.**

`db_upsert` runs `check_listing_currency` first: the listing's `currency` is read on the write's own
transaction and an inheritance recorded in another is refused with
`UpsertError::CurrencyNotListings` → `422`, naming both and saying which one to use ("the parcel's
cost base and the exchange's price for the same security are one money"). An unknown `listing_id`
falls through to the foreign-key rejection, as it does on the ESS side.

The argument is the one already accepted twice, sharpened by the entity: under
`MarketValueAtDeath` the figure entered *is* a market value of that listed security, so a currency
other than the one the exchange quotes it in cannot be right.

Tests: `an_inheritance_in_another_currency_than_its_listings_is_refused` — a USD inheritance of an
AUD listing refused with both currencies in the body and nothing persisted, and the matching pair
accepted either way round (AUD/AUD and USD/USD). Docs: `docs/API.md` (Inheritances' `422` list and
the 422 catalogue).

## A duplicated inheritance is caught by nothing (SCENARIOS K-09)
(SCENARIOS.md section K verification pass, 2026-08-18. The G-24 / H / J-11 pattern, one table
further on: every other statement-shaped fact table now has a duplicate health check —
`duplicate_income`, `duplicate_interest`, `duplicate_expenses`, `duplicate_amma_statements`,
`duplicate_ess_statements`, `duplicate_actions` — and `inheritances` has none, though it is the same
shape: a document-derived row, re-entered by hand, that creates a **parcel**.)
- [x] Reproduced: two identical inheritances (same listing, account, date of death, quantity, cost
  base and rule) → two parcels of 100 units each, `GET /reports/health` completely silent. The
  holding is doubled and so is every cost base and gain computed off it
- [x] The duplicate is indistinguishable from the legitimate case only by its *figures*, which is
  exactly what the existing checks key on: K-09 (two beneficiaries, or two deaths, or the same death
  across two accounts) all differ in quantity, cost base or account, so a check keyed on identical
  figures stays silent for them. Follow `duplicate_ess_statements` exactly — grouped in Rust because
  the amounts are TEXT decimals SQL would compare as strings
- [x] Fix: `duplicate_inheritances` in `reports::health` + the web UI banner, keyed on (listing,
  holding account, date of death) *plus* identical quantity, cost base, rule and LPR figures
- [x] Tests: two identical inheritances are reported; two differing in quantity or account are not
- [x] Docs sync: `docs/API.md` Health, README's health-check feature line

**Resolution (2026-08-18): `duplicate_inheritances`, the `duplicate_ess_statements` shape exactly.**

`reports::health` grew a sixth duplicate list: every (listing, holding account, date of death)
carrying more than one inheritance whose *figures* also match, newest death first, naming the ticker,
the units, the whole cost base each row carries onto its parcel (first element + LPR expenditure)
and the ids to open. Read on the health report's own transaction, grouped in Rust because the
amounts are TEXT decimals SQL would compare as strings, and pre-filtered in SQL to rows already
sharing the (listing, account, death) key, so an unrelated portfolio never reaches memory.

The key includes the **cost-base rule**, and deliberately does not collapse across it: the same units
and the same figure recorded once as the deceased's cost base and once as market value at death are
two different claims about one holding — a contradiction worth showing, not a duplicate to hide.
Everything else that identifies the row is compared too, `fx_rate` included.

Warning, not constraint, as with every other duplicate list: two inheritances of one listing from
one death are ordinary (two holding accounts, two estates, a part interest recorded in stages), which
is exactly why the figures are in the key.

The read side needed one small tidy: `inheritance::COLUMNS` now holds the SELECT list the entity's
own `db_list`/`db_get` and the health check all use, instead of the column names being spelled out
per query.

Tests: `one_inherited_parcel_entered_twice_is_reported` (the pair reported with its ticker, units,
`3000` cost base and both ids) and `inheritances_from_one_death_that_differ_are_not_reported` (a
part interest in a different quantity, the same figures in another holding account, and the same
figures on another listing — all silent), plus the served-bundle assertion in `web.rs` for the
banner text and its Inheritances link. Docs: `docs/API.md`'s Health list and response shape, and
the README health feature line.

## Nothing states what the deceased's cost-base figure must be net of (SCENARIOS K-02, K-09)
(SCENARIOS.md section K verification pass, 2026-08-18. The G-03/G-04 shape: the figure is entered by
hand, one number, and two ATO rules about what that number must already have had done to it are
recorded nowhere the user will see. `cost_base`'s form field is labelled "Cost base" and carries no
hint, where `lpr_expenditure`, `lpr_expenditure_date` and `deceased_acquisition_date` — the fields
with rules attached — each carry one, and the per-rule `typeDescs` prose describes *which* figure
to enter without saying what it must be net of.)
- [x] **Indexation recalculated out.** QC 66053 (`docs/ato/inherited-assets-cost-base.md`): where
  the deceased died **on or after 21 September 1999**, indexation is unavailable to the beneficiary
  "and any indexation inside the deceased's cost base must be recalculated out". A deceased who
  acquired before 21 September 1999 may well have been carrying an indexed cost base, so the figure
  copied off the estate's records is the wrong one — silently overstating the cost base and
  understating every later gain. Nothing in the UI, `docs/API.md` or the README says this; the
  mirror says it and is not a user surface. The existing Known-limitations entry covers only the
  indexation *alternative* not being modelled, which is the opposite direction
- [x] **Apportionment between beneficiaries.** K-09 verified as the documented boundary — one
  taxpayer, so the beneficiary records their own share and there is nowhere to represent the other
  beneficiaries — and entering a part share works cleanly (500 of the estate's 1,000 units at
  $10,000 of its $20,000 cost base gives the expected parcel, fractional quantities included). What
  is missing is that the *cost base* must be apportioned with the units: a user who takes half a
  holding and types the deceased's whole cost base doubles their cost base, and no check can see it
  (a 500-unit inheritance at a $20,000 cost base is a perfectly ordinary row). The Known-limitations
  entry says only that the estate/LPR side is out of scope
- [x] Fix (documentation, unless Evan wants more): a hint on the `cost_base` field for each rule
  (`typeDescs` already carries per-rule prose to extend), a sentence in `docs/API.md`'s Inheritances
  section, and an extension of the inherited-parcels Known limitation. A `doc_checks.rs` test pins
  the text, per the "a doc-only requirement is done when a test asserts it" rule
- [x] Tests: `doc_checks.rs` assertions on the API.md/README text, and the served-bundle assertion
  for the field hints
- [x] Docs sync: `docs/API.md` (Inheritances + Known limitations), README's inherited-parcels
  feature line, `src/web/config.js`

**Resolution (2026-08-18): documented on every surface, and the mirror it rests on refreshed.**

Re-fetching QC 66053 to check the indexation wording found the page had moved on since the
2026-06-10 capture (now "last updated 22 June 2026"): the Maria example's dates had rolled forward,
several rules the mirror had *summarised* are now quoted, and a whole **"Legal costs incurred by a
legal personal representative"** section with two worked examples (Annie — probate and
will-validity costs are in the cost base; Cassie — the same solicitor's pre-death charges are not)
had been added. `docs/ato/inherited-assets-cost-base.md` is rebuilt from that fetch, and the
companion QC 69713 mirror was re-verified word for word (only its "last updated" moved).

The indexation rule is verbatim: "If the deceased died on or after 21 September 1999, you can't use
indexation. If the deceased's cost base includes indexation, you must recalculate the first element
of your cost base to exclude it."

Both conventions are now stated where the figure is typed — a new **"What the `cost_base` figure
must already be net of"** block in `docs/API.md`'s Inheritances section, the inherited-parcels
Known limitation, the README feature line, and a hint on the `cost_base` field itself ("Your share
of it: half the units carry half the deceased's cost base…"). The LPR field's hint gained the
Annie/Cassie test ("what the LPR incurred administering the estate… Not anything billed before the
death"), and the section now says that several LPR expenses are entered as their total, since one
row carries one figure and one date and nothing reads the date.

Tests: `doc_checks::inherited_cost_base_entry_conventions_documented` pins the mirror's three
quotes, its refreshed provenance header, its presence in the ATO index, both conventions in the
Inheritances section, both in the Known limitation, and the README line; the served-bundle
assertions in `web.rs`'s `inheritance_ui_present` pin the two field hints.

## LPR expenditure converts at the parcel's acquisition month, not the month it was incurred (SCENARIOS K-04)
(SCENARIOS.md section K verification pass, 2026-08-18. `db_upsert` folds the LPR expenditure into
the Buy's single `brokerage` figure, so `domain::cost_base` translates the whole parcel — first
element *and* LPR expenditure together — at one rate: the parcel's (possibly deemed) acquisition
month. Under `DeceasedCostBase` that month is the **deceased's acquisition**, while the LPR incurred
the expense after the death, by definition a later month and often a much later one.)
- [x] Reproduced: a USD listing; deceased acquired 2015-05-05, died 2024-03-01; `cost_base` US$2,000
  and `lpr_expenditure` US$1,000 incurred 2024-06-01. Rates imported: `USD 2015-05 = 2`,
  `USD 2024-06 = 0.5`. `GET /portfolio/open-parcels` reports `original_cost_base 1500` (US$3,000 ÷ 2).
  Translating each element at its own month gives A$1,000 + A$2,000 = **A$3,000** — the LPR element
  is understated 4×, and it moves the reported cost base by 50%
- [x] The existing Known limitation does not cover it. "Cost-base FX timing" (2026-07-13) is about
  the AMIT/return-of-capital **reductions** and argues the single rate "keeps each parcel's
  cost-base breakdown internally consistent"; it also says the simplification "only bites on a
  non-AUD holding receiving non-AUD AMIT/return-of-capital reductions, which in practice does not
  arise". An LPR expense on an inherited foreign parcel is an **addition**, is dated by the user on
  the row itself, and does arise. `inheritance.rs`'s module doc mentions the single-rate treatment
  ("LPR expenditure translates with the parcel; its own incurral date is provenance only") but ties
  it to indexation, and no user-facing surface says it at all
- [x] The ATO position: s 960-50(6) translates each amount at its own transaction time
  (`docs/ato/forex-common-transactions.md`, QC 18322 — Lisa's cost base and proceeds each translate
  at their own date), and QC 66053 has the LPR expense "included on the date the LPR incurred it"
- [x] **Decide the model** (an `AskUserQuestion` for Evan, not a silent call). (a) **Translate the
  LPR element at its own month** — correct per s 960-50(6), but it means the parcel's cost base is
  no longer one currency amount at one rate: either the Buy carries the LPR expenditure in a second
  column the pipeline converts separately, or the inheritance stores the LPR expenditure already in
  AUD. Breaks the "initial − reductions = adjusted holds in the native currency" property the
  pipeline currently guarantees. (b) **Give `lpr_expenditure` its own currency column** and require
  it in AUD (the realistic case: an Australian LPR bills an Australian estate in AUD, whatever the
  shares are denominated in), converting nothing — narrower than (a) and matches how the expense is
  actually incurred. (c) **Document it** as a Known limitation and say so in the field hint —
  cheapest, and consistent with the 2026-07-13 FX-timing cut, but it leaves a wrong figure the user
  has to fix by hand. (b) looks strongest: the mismatch is not really an FX-timing subtlety, it is
  that an AUD fee has nowhere to be recorded as AUD
- [x] Tests: whichever model is chosen, a foreign inherited parcel with LPR expenditure reports the
  element at its own rate/currency, and the AUD case is unchanged
- [x] Docs sync: `docs/SCHEMA.md` (`inheritances`), `docs/API.md` (Inheritances, Known limitations,
  the FX-conversion section), README's inherited-parcels feature line

**Resolution (2026-08-18): refused on a foreign parcel, and documented — Evan chose option (c'),
after (b) turned out not to be available as described.**

Implementing (b) — an AUD `lpr_expenditure` that "converts nothing" — proved to need somewhere for
an **AUD** amount to land in the cost base *after* conversion, which the single-rate pipeline has
no place for: a new `trades` column (migration + `row_history` trigger rebuild) threaded through
`domain::cost_base`, its three production `into_aud_with` call sites, and carried + pro-rated
through all three rollover operations, whose `carried_cost_base` works in the parcel's *own*
currency. Put back to Evan with that cost measured, the answer was to refuse the pair instead.

So `validate` now rejects a non-zero `lpr_expenditure` on a non-AUD inheritance
(`UpsertError::LprExpenditureOnForeignParcel` → `422`), and the limitation says why: the fee has no
home a foreign parcel can hold correctly, and folding it into `cost_base` by hand translates it at
the same wrong month, so nothing is gained by accepting it. The ordinary case — an Australian LPR
fee on an Australian holding, where the conversion is the identity — is untouched.

Tests: `lpr_expenditure_is_refused_on_a_foreign_parcel` (a US$1,000 fee on a USD parcel refused with
the previously accepted row untouched, and the AUD parcel still taking its $200 fee onto the Buy),
and `doc_checks::lpr_expenditure_on_a_foreign_parcel_documented` pinning the Known limitation
(including the size of the error it would otherwise report), the Inheritances section, the 422
catalogue entry, the README line and the SCHEMA column. The module doc's old claim that "LPR
expenditure translates with the parcel" — true, and the bug — is replaced by the rule and its
reason. Docs: `docs/API.md`, `docs/SCHEMA.md`, README, and the field hint in `src/web/config.js`.

## The Crypto/exchange pairing refusals answer with a raw CHECK expression (SCENARIOS L-09)
(SCENARIOS.md section L verification pass, 2026-08-18. `listing::db_upsert` validates the
digital-token ticker itself and returns a sentence for it, but leaves the `exchange_mic`/
`security_type` pairing to the database's CHECK — so the two mistakes a user is most likely to make
while adding a crypto listing are answered in SQL.)
- [x] Reproduced, both directions, identical body: a `Crypto` listing with `exchange_mic: "XASX"`,
  and a `Share` listing with no exchange, each return `422` `a value falls outside its allowed set
  (CHECK constraint failed: (exchange_mic IS NULL) = (security_type = 'Crypto'))`. The web UI shows
  that string in its toast. It does not say which side is wrong, and the two errors are
  indistinguishable
- [x] Contrast the sibling refusal on the same write: "a Crypto listing's ticker must be a
  recognised digital-token code". `docs/API.md` documents the pairing as two distinct refusals; the
  server does not distinguish them
- [x] Precedent: A-18 (`2af8d4f`) — a DELETE blocked by an inbound foreign key used to say the row
  did not exist; the fix classified the violation and named it
- [x] Fix: two `UpsertError` variants checked in `db_upsert` (the `UnrecognisedDigitalToken` shape),
  each with its own message; the CHECK stays as the backstop it is
- [x] Tests: each direction returns its own sentence, and the CHECK still holds against a direct DB
  write
- [x] Docs sync: `docs/API.md` Listings + the 422 catalogue wording

**Resolution (2026-08-18): both directions named in `listing::db_upsert`, and in the rename that can
also meet them.**

`UpsertError::CryptoWithExchange` / `ExchangeRequired` are checked before the INSERT, so the table's
CHECK never has to answer for them; the two bodies live as `listing::CRYPTO_WITH_EXCHANGE` /
`EXCHANGE_REQUIRED` because `listing_rename` needs the first of them too — a rename may change
`exchange_mic`, so it is the second way a Crypto listing can be handed an exchange
(`RenameError::CryptoWithExchange`). The CHECK stays as the backstop for any write that does not go
through `db_upsert`, and a test pins it there by inserting the violating row directly.

Tests: `db_pairing_of_exchange_and_security_type_is_refused_by_name` (each direction by variant,
nothing persisted, and the raw INSERT still refused by the CHECK),
`api_invalid_crypto_listings_return_422` extended to assert each direction's own sentence and the
absence of "CHECK" from either body, and `db_rename_cannot_give_a_crypto_listing_an_exchange`.
Docs: `docs/API.md` Listings, Renames, and the 422 catalogue.

## A Crypto listing can be marked `amit` (SCENARIOS L-09)
(SCENARIOS.md section L verification pass, 2026-08-18. An AMIT is an attribution managed investment
**trust**; a crypto asset is not a trust interest — TD 2014/25 says it is not even currency. Nothing
refuses the flag, and the state it creates is unreachable rather than merely odd.)
- [x] Reproduced: `PUT /listings/1` with `security_type: "Crypto", amit: true` → `204`. The annual
  tax report then answers `completeness.complete: false` with
  `amma_missing: [{listing_id: 1, ticker: "BTC"}]` — an AMMA statement no coin will ever issue — and
  every income row on the listing is refused `422` ("this listing is an AMIT — its distributions are
  trust income") unless it claims to be trust income
- [x] `amit_from` (migration 0024) is the same flag dated, so it needs the same refusal
- [x] Fix: refuse `amit` / `amit_from` on a `Crypto` listing at write time, in `listing::db_upsert`
  beside the digital-token check — a write-time invariant, per CLAUDE.md, not a report-time
  complaint
- [x] Tests: the write is refused with a sentence saying why; a non-Crypto listing is unaffected
- [x] Docs sync: `docs/API.md` Listings (the crypto paragraph) + the 422 catalogue

**Resolution (2026-08-18): refused at write time, alongside the pairing checks.**

`listing::db_upsert` rejects `amit` — and `amit_from`, the dated form of the same flag — on a
`Crypto` listing (`UpsertError::CryptoCannotBeAmit` → `422`), so the unreachable state cannot be
created rather than being complained about by a report afterwards. An ordinary `Trust` listing is
untouched.

Tests: `db_crypto_listing_cannot_be_an_amit` (both the flag and the date refused, nothing persisted,
a Trust listing still accepted). Docs: `docs/API.md` Listings + the 422 catalogue, and the AMIT
checkbox's hint in `src/web/config.js`.

## The crypto limitations say "not modelled" where the ordinary entry path already gives the ATO's figures (SCENARIOS L-04, L-05, L-06)
(SCENARIOS.md section L verification pass, 2026-08-18. `docs/API.md` Known limitations says "Chain
splits/forks, wrapping, and the personal-use-asset exemption are not modelled", and gives one recipe
for staking rewards *and* airdrops together. Driving the scenarios against the running system, three
of those four are not gaps at all — the machinery already produces the ATO's own worked answers, and
what is missing is the sentence naming the entry path, the way the gift and swap entries do.)
- [x] **Initial-allocation airdrop** (L-04): the ATO's rule is the opposite of the documented recipe
  — you derive **no** ordinary income and make no capital gain on receipt, and the tokens have a
  **cost base of zero** (or what you paid). Reproduced: a Buy of 800 units at price `0` is accepted,
  opens a nil-cost-base parcel with its clock from receipt, and the later sale reports the ATO's
  Josh example exactly — $4,000 proceeds, $4,000 discount-eligible gain, $2,000 after the discount.
  The documented "an income row plus a Buy at receipt-date market value" is wrong for this half of
  L-04 and would overstate assessable income by the full market value
- [x] **Chain split** (L-05): the new asset is neither ordinary income nor a capital gain on
  receipt, has a nil cost base, is acquired at the split, and is discountable after 12 months
  (QC 69953, Alex's example: 2 Bitcoin Cash for $1,260 → a $630 discount gain). That is the same
  nil-cost-base Buy dated the split. The page's *other* case — no post-split asset continues the
  original, so a **CGT event C2** happens to it — is representable too: reproduced with a
  `WorthlessShares` corporate action carrying `worthless_event: "C2Cancellation"`, whose recognise
  closed the parcel at nil proceeds for a capital loss of $8,300, Ming's stated figure to the dollar
- [x] **Wrapping** (L-06): the ATO says wrapping or unwrapping a token *is* a CGT event — you
  exchange one crypto asset for another, with capital proceeds equal to the market value of the
  wrapped token received. That is the documented **swap** recipe, already implemented and already
  covered by `ato_examples.rs`'s Katrina test. Saying "not modelled" beside it invites the opposite
  reading — that no CGT event arises — which is the expensive mistake
- [x] **Stablecoins** (L-14) belong in the same rewrite: TD 2014/25 rules that bitcoin is not
  "foreign currency" for Division 775, and Schedule 2 to the Treasury Laws Amendment (2022 Measures
  No. 4) Act 2023 excluded digital currency from the definition for income years starting on or
  after 1 July 2021. So the Div 775 deferral in the same limitation entry **never reaches a
  stablecoin**: it is a CGT asset like any other crypto, which is exactly what the system does
  (verified: a stablecoin holding bought at A$1.55 and spent at A$1.60 reports a $500 capital gain).
  The entry should say so rather than leaving a reader to wonder which half applies
- [x] Nothing in `docs/ato/` mirrors any of this: `crypto-cgt.md` covers only the CGT basics and the
  swap, and `OVERVIEW.md` indexes nothing on staking, airdrops, chain splits, or wrapping
- [x] Fix: mirror the two ATO pages (`staking-rewards-and-airdrops`, `crypto-chain-splits`, plus the
  wrapped-tokens section of `decentralised-finance-and-wrapping-crypto`) into `docs/ato/` with their
  source URLs and retrieval date, index them in `docs/ato/OVERVIEW.md`, and rewrite the crypto
  Known-limitations entry so each case names its entry path — as the gift entry already does
- [x] Tests: `ato_examples.rs` for Josh (initial-allocation airdrop), Alex (chain split) and Ming
  (the abandoned original, C2); `doc_checks.rs` for the rewritten limitation text
- [x] Docs sync: `docs/API.md` Known limitations, README Features (the crypto bullet's parenthetical)

**Resolution (2026-08-18): four ATO mirrors added, the limitation rewritten to name an entry path
per case, and three of the worked examples reproduced.**

New mirrors, each indexed in `docs/ato/OVERVIEW.md`: `crypto-staking-airdrops.md` (QC 69950),
`crypto-chain-splits.md` (QC 69953), `crypto-wrapping.md` (QC 73649 — the wrapped-tokens and
DeFi-rewards sections, the two the entry paths rest on) and `crypto-not-foreign-currency.md`
(TD 2014/25's Ruling and date of effect, for the Div 775 half).

The Known-limitations entry now opens with the fact that made "not modelled" misleading — there is
no crypto-specific *operation*, because each of these is a CGT event the ordinary trade entry
already records — and then gives the entry for each: the swap (wrapping included), the chain split's
nil-cost-base Buy and the `WorthlessShares` / `C2Cancellation` close of an abandoned original, the
initial-allocation airdrop as that same nil-cost-base Buy, and the established-token airdrop /
staking reward as income plus a Buy, with the income half named as the open limitation it is
(item 24 has no label here — the L-03/L-04 finding). The personal-use-asset exemption stays listed
as genuinely not modelled, and the Div 775 deferral now says it never reaches a crypto holding.

Tests: `ato_examples.rs` reproduces Alex (chain split: a $1,260 discount gain, $630 after the
discount), Ming (the abandoned original: a capital loss of $8,300 via C2) and Josh (initial-
allocation airdrop: nil cost base, $4,000 gain, $2,000 after the discount) — the module doc records
why Kal, Anastasia, Merindah, Calista, Craig and Bree are not reproduced. `doc_checks::
known_limitations_document_the_crypto_entry_paths` pins the rewritten entry, each mirror's source
header, and that OVERVIEW.md indexes all four.

## A trading fee paid in crypto has no stated treatment (SCENARIOS L-08)
(SCENARIOS.md section L verification pass, 2026-08-18. Exchanges commonly bill the trading fee in a
crypto asset — the one being traded, or a third token. `PUT /trades` refuses a `brokerage_currency`
other than the trade's, with the documented "enter it converted into the trade's currency". That is
right for the *incidental-cost* leg, and silent about the other one: crypto spent on a fee is itself
a **disposal**, the very rule the holding-account transfer's `fee_allocations` already implements
for an on-chain network fee.)
- [x] Reproduced: a 1 BTC buy with `brokerage: "0.001", brokerage_currency: "BTC"` → `422`
  "brokerage_currency must equal the trade's currency…"; the same fee entered as `"50"` AUD is
  accepted and lands in the cost base. Nothing anywhere says whether the 0.001 BTC also had to be
  disposed of — and the answer differs by case
- [x] The three cases a user actually meets: a fee **netted out of the crypto received** (you simply
  acquired fewer units — enter the net quantity, no disposal); a fee **taken from the crypto sold**
  (its AUD value is an incidental cost of the sale — brokerage in the trade's currency, no second
  disposal); a fee **paid in a third asset you hold** (a disposal of those units at market value,
  entered as a Sell, *and* the same AUD value as the trade's brokerage). Only the middle one is
  what the current sentence describes
- [x] This is live data, not a hypothetical: the 2026-07-13 crypto reconciliation traced a $4.14
  gap to a Binance trade fee charged in ETH
- [x] Fix (decision): documentation naming the three cases beside the existing brokerage-currency
  limitation, or an entry path — `fee_allocations` on a Buy/Sell, the shape `transfers` already has,
  which would make the disposal atomic with the trade instead of a second row the user must
  remember
- [x] Tests: whichever way it goes, a fee-in-crypto entry reports the incidental cost in the cost
  base and the disposal (where there is one) in the gains reports
- [x] Docs sync: `docs/API.md` Known limitations (the brokerage-currency entry) + Trades, README


**Resolution (2026-08-18): documented, with the three cases separated — Evan chose the paragraph
over an entry path.**

The brokerage-currency Known limitation now carries the crypto shape of the same rule and says which
of its three cases is a second CGT event: a fee netted out of the units received is not a fee at all
(enter the net quantity), a fee taken from the units sold is an incidental cost already inside the
Sell's quantity (brokerage, no second disposal), and a fee paid in a third asset you hold *is* a
disposal of those units at market value (a Sell of that listing beside the trade, and the same value
as the trade's brokerage). The note also says why the disposal leg cannot be inferred: a transfer
states which parcels were burned to pay its network fee, and a trade does not.

Tests: `doc_checks::known_limitations_document_the_brokerage_currency_invariant` extended to pin the
three cases and the reason. Docs: `docs/API.md` Known limitations.

## The recognised digital-token list is BTC and ETH until a credentialed import runs (SCENARIOS L-10)
(SCENARIOS.md section L verification pass, 2026-08-18. A `Crypto` listing's ticker must be a
`DigitalToken` row in `currencies`. `0001_schema.sql` seeds exactly two — BTC and ETH — and the rest
come from the ISO 24165 (DTIF) import, which is **skipped with a log warning** unless
`DTI_REGISTRY_USER_ID` / `DTI_REGISTRY_PASSWORD` are set. Out of the box, therefore, no other crypto
asset can be recorded at all, and the refusal does not say why or what to do.)
- [x] Reproduced: `DOGE`, `USDT` and `WETH` are each refused with the same sentence — "a Crypto
  listing's ticker must be a recognised digital-token code" — with no hint that the list is two rows
  long, that an import fills it, or that the import needs credentials. The live database confirms
  the shape: 178 fiat rows, 2 digital-token rows
- [x] It is what blocks two of this section's own scenarios from being entered under their real
  tickers (L-06's WETH, L-14's USDT), and would block any real portfolio holding SOL, USDC or ADA
- [x] The credential requirement *is* documented, in `docs/API.md`'s Currencies import paragraph —
  which is not where a user meets the problem
- [x] Fix: name the remedy in the refusal itself (import the ISO 24165 registry; the credentials it
  needs), and consider a `reports::health` line when the token list is still only the seeds, the way
  `prices_stale` / `fx_stale` surface a feed that has not run
- [x] Tests: the refusal names the import; the health line appears only while the list is unimported
- [x] Docs sync: `docs/API.md` Listings + Currencies, README (setup — what the crypto feature needs
  before it works)

**Resolution (2026-08-18): the refusal names the remedy — Evan chose the message without a health
line.**

`listing::UNRECOGNISED_DIGITAL_TOKEN` is now the shared `422` body for both the listing write and
the rename that can meet the same rule: it says the seeded list is just BTC and ETH and names the
ISO 24165 (DTIF) import — endpoint and credential environment variables — as the way to widen it.
The listings screen's description says the same thing where the listing is created, and the API doc
says it in the crypto-listings paragraph rather than only in the Currencies import section, which is
not where a user meets the problem.

Tests: `api_invalid_crypto_listings_return_422` extended to pin the remedy in the body. Docs:
`docs/API.md` Listings, `src/web/config.js`.

## Staking rewards and airdropped tokens are reported as dividends (SCENARIOS L-03, L-04)
(SCENARIOS.md section L verification pass, 2026-08-18. The ATO is explicit: the money value of
staking rewards, and of an **established** token received by airdrop, is **ordinary income at the
time of receipt**, declared "as **other income**" — item 24 of the individual return, not item 11
(QC 69950, "Staking rewards and airdrops"; the same page's Anastasia and Merindah examples). The
documented workaround — README + `docs/API.md` Known limitations, "an income row plus a Buy at
receipt-date market value" — has nowhere to put that income: `income.income_type` is
`Dividend | EmploymentIncome`, so the row is a dividend unless it is remuneration.)
- [x] Reproduced: 0.5 ETH of staking rewards worth A$2,000 entered as an income row on the ETH
  listing → `GET /portfolio/tax-summary` reports `dividends_assessable: "2000"` against ATO label
  **`11S + 11T`**, and the annual tax report prints it in the **Dividends** table with
  `franking_status: "entitled"` — a franking entitlement on a payment no company made. The total
  assessable income is right; every label on it is wrong
- [x] `income_type: "EmploymentIncome"` is no better: it reports on the tax summary's
  `employment_income` line, which `docs/API.md` describes as item 1/2 salary and wages. Staking
  rewards are neither a distribution of a holding nor remuneration for services
- [x] The cost-base half is already right: the reward tokens entered as a Buy at receipt-date market
  value open a parcel at that value with its own 12-month clock, exactly as the ATO states, and the
  later sale reports correctly (verified). This finding is only about where the *income* lands
- [x] Precedent: J-10 (`1d76d3f`) is this finding one income type earlier — the dividend-equivalent
  workaround reported remuneration at 11S, and Evan chose the `income_type` enum over sharpening the
  wording. The same choice is open here (a third variant reported on its own line and in its own
  annual-tax-report table, against item 24), against the cheaper alternative of documenting that
  crypto income must be carried to item 24 by hand
- [x] The ATO page is now mirrored: `docs/ato/crypto-staking-airdrops.md`, indexed in `docs/ato/OVERVIEW.md` (closed 2026-08-18 with the crypto entry-path documentation finding)
- [x] **Decided 2026-08-18: Evan chose the third `income_type` variant** — reported on its own tax-summary line against item 24 and in its own annual-tax-report table, out of every dividend total
- [x] Tests: a staking-reward row reports at its own label, is in no dividend total, and carries no
  franking status; the annual tax report prints it in its own table
- [x] Docs sync: `docs/API.md` Income (`income_type`) + the tax-summary/annual-tax-report field
  tables, README Features / Known limitations

**Resolution (2026-08-18): `IncomeType::OtherIncome` — Evan chose the third variant, as J-10's own
migration note predicted ("the enum is the extension point").**

Migration 0029 widens the `income_type` CHECK. It is the first rebuild of a table another table
*references*: `attachments.income_id` points at income, and SQLite rewrites such a reference onto the
renamed table whenever `foreign_keys` is on — which would have left attachments pointing at
`income_old`, and its drop would then have cascaded every income attachment away. Neither pragma that
prevents that can be set inside a transaction, so the migration is `-- no-transaction` and brackets
its own work in BEGIN/COMMIT: SQLite's documented procedure. Rehearsed against a copy of the live
database — 47 income rows and 134 attachments (58 of them income-linked) intact, `foreign_key_check`
and `integrity_check` clean, the rebuilt triggers firing.

An `OtherIncome` row carries the cash and nothing else, exactly as the employment kind does
(`check_non_distribution_row` now covers both, and names the kind in its rejection), and is equally
un-reinvestable. Where it differs is the label: item 24 is prefilled by nothing, so the row reports
on the tax summary's own `other_income` line **and** counts in gross assessable investment income,
prints in its own *Other income (item 24)* table in the annual tax report, is labelled *Other income
(staking reward / airdrop)* in the activity ledger, and — unlike a dividend equivalent — **is**
counted in the performance report's income yield, because a staking reward is a return the holding
itself produced.

Tests: `api_other_income_round_trips_and_refuses_distribution_fields`,
`db_other_income_reports_at_item_24_and_is_assessable`,
`other_income_prints_in_its_own_item_24_table`, the 0029 block of
`row_history::audited_tables_match_migration_check_and_triggers` (trigger pair, staleness triggers,
five indexes, every column, and the pragma pair), the web bundle assertions, and
`ato_examples::crypto_defi_reward_example_craig_stablecoin_tokens` — Craig's DeFi reward (QC 73649),
whose $10 of ordinary income and $10 cost base is the same pair a staking reward is entered as.
Docs: `docs/API.md` (Income, the tax-summary label table, Known limitations), `docs/SCHEMA.md`,
README, and the income form's kind hint in `src/web/config.js`.

## The annual tax report's printed sell-side FX rate is not the rate the proceeds used (SCENARIOS M-01, M-02)
(SCENARIOS.md section M verification pass, 2026-08-19. The annual tax report is the print-to-PDF
document the year is archived as, and each non-AUD disposal row prints `currency`,
`buy_month_fx_rate` and `sell_month_fx_rate` beside its AUD figures so the arithmetic can be
checked. The buy side prints the rate actually applied; the sell side prints the ATO monthly rate
whatever the proceeds used.)
- [x] Reproduced (a): a Sell of US$20,000 carrying `spot_fx_rate: 0.5000` in a month whose ATO rate
  is 0.6800 prints `proceeds_aud: 40000` (= 20000 / 0.50, correct) beside
  `sell_month_fx_rate: 0.6800`, which computes A$29,411.76 — a A$10,588 gap between the printed
  figure and the printed rate, in the document a reader checks the return against
- [x] Reproduced (b): a Sell in a month with no imported ATO rate, resting on its own `fx_rate` of
  0.55, prints `proceeds_aud: 36363.64` beside `sell_month_fx_rate: null` — the fallback rate the
  buy side *does* print (via `fx_override()`) is hidden on the sell side
- [x] Cause: `reports::tax_report`'s `sell_rate` resolves with `FxOverride::None` where `buy_rate`
  resolves with `bt.fx_override()`. It is also keyed on the *buy* trade existing
  (`buy_trade.and_then(|_| …)`), which is unrelated to the sale's own conversion
- [x] Fix: resolve the sell-side rate from the **sale trade's** override, mirroring the buy side, so
  the printed rate is always the rate the printed proceeds were computed at
- [x] Tests: a spot-override Sell and a fallback Sell each print the rate their proceeds used;
  an AUD disposal still prints neither
- [x] Docs sync: `docs/API.md`'s Annual tax report section, where the two rate columns are described

**Resolution (2026-08-19): each side prints the rate its own figure was converted at.**

`reports::tax_report`'s `sell_rate` now resolves from the **disposal's own** override, mirroring
`buy_rate`: a Sell's `fx_override()` (its `spot_fx_rate` when set, else its `fx_rate` fallback), and
for a rights sale — which is not a trade, so it is not in `DisposalInputs::trades` — that row's
`fx_rate` against the issue's currency, loaded into a new `rights_sales` map. The rate is no longer
keyed on the *buy* trade existing.

Tests: `a_disposals_printed_fx_rates_reproduce_its_printed_aud_figures` drives both cases and
asserts each printed AUD figure equals its native amount divided by the rate printed beside it —
the property the document is printed for; `an_aud_disposal_prints_no_fx_rates` keeps the AUD case
printing neither. Docs: `docs/API.md`'s Annual tax report disposal-row description, and the web
document's own note now reads "buy-side rate / sale-side rate" rather than "buy-month / sell-month",
which was only true when no override was in play.

## A missing ATO rate answers a tax report with a bare `500` and an empty body (SCENARIOS M-04, M-07)
(SCENARIOS.md section M verification pass, 2026-08-19. A non-AUD income or AMMA record has no
per-record fallback by design, so a month with no imported rate is a loud failure — the right
behaviour. What reaches the user is `500 Internal Server Error` with an empty body, which the web UI
can only show as "HTTP 500".)
- [x] Reproduced: one USD income row in a month with no `rba_fx_rates` row → `GET
  /portfolio/tax-summary` answers `500`, body empty. The cause is named precisely in the server log
  (`no ATO FX rate for USD in 2023-05 and no manual override supplied`) and nowhere else
- [x] The same gap in the *valuation* path already answers well: `POST /report_snapshots/generate`
  returns `422 AAPL: no ATO FX rate for USD in 2024-05 and no manual override supplied`. One class
  of problem, two answers
- [x] This is not an internal detail: it is a data gap the user fixes by running the RBA import (or
  entering the rate the record converted at), and they cannot act on a blank 500
- [x] Cause: `impl From<FxError> for sqlx::Error` turns a `MissingRate` into `sqlx::Error::Decode`
  so it cannot be swallowed, and `ApiError`'s `From<sqlx::Error>` then classifies every Decode as
  `Internal`. The classification is right for a malformed stored decimal and wrong for this
- [x] Fix: carry the missing-rate case through the report error path so it lands as `422` naming the
  currency and month, like the snapshot path
- [x] Tests: the affected reports answer `422` naming the currency and month; a genuine decode
  failure still answers `500`
- [x] Docs sync: `docs/API.md` Response codes (the 422 catalogue) and the FX conversion section,
  which stated the failure as `500`

**Resolution (2026-08-19): the `FxError` is carried, not stringified, and a missing rate answers
`422` naming the currency and month.**

`impl From<FxError> for sqlx::Error` now boxes the `FxError` itself into `sqlx::Error::Decode`
instead of its `to_string()`, so the far end can get it back. `impl From<sqlx::Error> for ApiError`
downcasts a decode error to `FxError` and routes a `MissingRate` through the new
`missing_rate_unprocessable` — a `422` whose body is the error's own sentence plus the remedy
(`import that month's rates with POST /rba_fx_rates/import`), logged at warn with the currency and
month. Every other decode failure — a malformed stored decimal, the case the classification was
written for — stays the `500` it should be, and `FxError::Db` (a failed lookup, a genuine fault)
does too. `impl From<FxError> for ApiError` classifies the same way, so a rate raised directly and
one carried through a report answer identically.

Tests: `infra::http`'s `a_missing_fx_rate_is_a_422_naming_the_currency_and_month` (both routes into
the classification, plus a non-FX decode error still 500 with an empty body) and
`tax_summary`'s `api_a_month_with_no_ato_rate_is_a_422_naming_it_not_a_bare_500` (the tax summary,
its CSV export and the annual tax report end to end, and that importing the month unblocks them).
Docs: the FX conversion section's rule 4 and every "fails loudly with `500`" in `docs/API.md`, plus
the 422 catalogue.

## Nothing lists which (currency, month) rates the recorded data needs (SCENARIOS M-04, M-14)
(SCENARIOS.md section M verification pass, 2026-08-19. `GET /reports/health` reports
`latest_fx_month` and `fx_stale` — the newest imported month across all currencies, and whether it
is old. That answers "has the import run lately", not "is every amount I have recorded convertible".)
- [x] Reproduced (M-14): an F11 CSV with an empty February cell imports January and March and skips
  February silently (`{"inserted": 2}`); health then reports `latest_fx_month: "2024-03"` — healthy
  by its own measure — while a February USD trade is costed from its own `fx_rate` of 0.99 at
  A$15,151 where the real rate would give A$22,727, and a February income row would `500`
- [x] The gap is invisible in both directions: a *silent* one (an amount resting on a per-trade
  `fx_rate` fallback because its month is missing) and a *fatal* one (an income/AMMA amount with no
  fallback at all, which fails the whole report)
- [x] The analogous reference-data gap already has its own report: `reports::settlement_coverage`
  lists every trade whose settlement window falls outside the seeded exchange-holiday years,
  non-blocking, "an empty report means every settlement window was computed against a complete
  calendar". FX has no twin
- [x] Fix: a coverage cross-check in `reports/` on that model
- [x] Tests: a complete series reports empty; a hole in the middle names the currency and month and
  what each affected amount rests on; an AUD-only portfolio reports empty
- [x] Docs sync: `docs/API.md` (a new report section + the reports list), README Features

**Resolution (2026-08-19): `GET /reports/fx_coverage`, on the settlement-coverage model.**

`reports::fx_coverage` lists every recorded amount whose conversion month has no imported ATO rate,
with `resting_on` saying what it converts at meanwhile — `spot_override` (a deliberate
transaction-date rate, so nothing is missing), `record_fx_rate` (the record's own fallback, applied
*silently*), or `nothing` (income, an AMMA statement, a return of capital — the report fails until
the month lands). It covers trades, income, AMMA and ESS statements, interest income, investment
expenses and returns of capital; an inheritance and an ESS vest are deliberately absent, since each
resolves its rate once and carries it onto the parcel Buy that *is* listed. Non-blocking like its
sibling: an empty report is the statement that every non-AUD amount converts at a published rate.
The other two alert kinds are the sibling finding below.

## A listing's `currency` is freely editable, silently re-denominating every stored price (SCENARIOS M-08)
(SCENARIOS.md section M verification pass, 2026-08-19. `listing::db_upsert` freezes `ticker` and
`exchange_mic` once a listing has trades, income or closing prices — an identity change must go
through `POST /listings/:id/rename` so it is recorded. `currency` is not in that list, though it is
just as much part of the listing's identity: every stored closing price is denominated in it.)
- [x] Reproduced: a USD listing with a Buy, a stored price of 200 and a generated snapshot. `PUT
  /listings/1` changing `currency` to EUR → `204`. The stored snapshot still reads `current_price:
  298.51` (200 / 0.67) and — because `listings` has no snapshot-staleness trigger — is still
  `stale: false`. Regenerating the same date silently answers `333.33` (200 / 0.60). The same stored
  fact, two AUD valuations, nothing marked
- [x] Trades keep their own `currency`, so cost bases are unaffected; the damage is to every
  price-derived figure (the overview, unrealised gains, performance, every snapshot in the series)
**Decision (2026-08-19, Evan): refuse it once there is history — option (a) — and add the triggers.**

- [x] Two questions for the model, worth asking together:
  - **(a)** What should a currency change *be*? A redenomination is a real event (a listing moving
    quote currency, a currency replaced) — so either it joins the rename path as a recorded event
    with an effective date (prices before it are in the old currency, after it in the new), or it is
    refused outright once there is history and the answer is a new listing plus a transfer
  - **(b)** Regardless of (a), `listings` needs snapshot-staleness triggers: a change to the row
    that a snapshot's figures depend on must stale the snapshots, which is the schema's rule for
    every other dated fact
- [x] Tests: whichever of (a) — the refusal, or the recorded event with prices resolved per span;
  and for (b) that a listing edit marks snapshots stale
- [x] Docs sync: `docs/SCHEMA.md` (the triggers), `docs/API.md` Listings + the 422 catalogue

**Resolution (2026-08-19): refused once there is history, and `listings` given its staleness
trigger.**

`listing::db_upsert`'s identity check now reads `currency` alongside `ticker`/`exchange_mic` and,
where the listing has recorded trades, income or closing prices, answers
`UpsertError::CurrencyChangeWithHistory { from, to }` → `422` naming both currencies and the way to
record a real redenomination: a new listing in the new currency plus a transfer of the parcels,
which keeps every stored price with the currency it was quoted in. A listing with no history yet
stays freely editable, as it is for the ticker.

Migration `0030` adds the `listings` staleness trigger the schema's own rule asks for, narrowed by
a `WHEN` clause to the two columns that change what a *stored* snapshot's figures mean — `currency`
(which denominates every stored price) and `security_type` (which decides the days a listing can be
valued on). A listing edit has no date of its own, so it stales the whole series rather than a
suffix; a `name`/`isin`/`price_symbol` edit fires nothing, since a series left permanently stale by
a name change — beyond the daily job's 14-day catch-up window, so never cleared — would teach a
reader to ignore the flag. No INSERT/DELETE counterpart is needed and the migration says why.

Tests: `db_currency_change_refused_once_dependents_exist` (free before history, refused after,
naming both currencies and the remedy, nothing written) and
`a_valuation_relevant_listing_edit_stales_every_snapshot` (currency and security type stale the
series, a name change does not). Docs: `docs/API.md` Listings + the 422 catalogue, `docs/SCHEMA.md`
Relationships + the trigger description.

## A trade may be recorded in a currency other than its listing's (SCENARIOS M-08)
(SCENARIOS.md section M verification pass, 2026-08-19. Four entities now refuse a currency that is
not the listing's, each for the same reason: ESS statements — "the per-share market value and the
listed price are the same money" — inheritances, `ReturnOfCapital` corporate actions, and a DRP
reinvestment's distribution. The trade, whose `average_price` *is* the listed price and whose
currency drives the cost base, has no such check.)
- [x] Reproduced: `PUT /trades/1` with `currency: "USD"` on an AUD-quoted ASX listing → `204`. The
  parcel is then costed by dividing an AUD price by a USD rate. `PUT /income/1`, `PUT
  /amma_statements/1` and `PUT /investment_expenses/1` accept the same mismatch
**Decision (2026-08-19, Evan): trades and AMMA statements take the rule; income and investment
expenses stay free.**

- [x] The four cases are not equally strong, which is the decision to take:
  - **Trades** are the strong case, and the same argument the ESS refusal already makes:
    `average_price × quantity` is the security's own price, so it is the listed currency by
    construction. A Sell shares the check via the Sell path
  - **AMMA statements** attribute a distribution of the listed trust — the same money as its price
  - **Income** is weaker: a distribution is normally paid in the listing's currency (which is why
    the DRP reinvest refuses otherwise), but a custodian paying an AUD dividend on a US holding is
    conceivable
  - **Investment expenses** are the weakest and probably should stay free: an Australian adviser's
    AUD fee attributed to a US holding is the ordinary case
- [x] Note this has a data question behind it: the rule must be checked against the live database
  before it lands, since an existing row it would now refuse cannot be edited afterwards — checked
  read-only against `share-tracker.db`: **zero** trades, AMMA statements or income rows disagree
  with their listing, so nothing existing is refused
- [x] Fix: the refusal on whichever set (a) chooses, in each entity's `db_upsert`, naming both
  currencies as the ESS and inheritance refusals do
- [x] Tests: per entity, the mismatch refused naming both currencies; a matching currency accepted;
  an AUD listing with an AUD row unaffected
- [x] Docs sync: `docs/API.md` per entity + the 422 catalogue

**Resolution (2026-08-19): trades, Sells and AMMA statements must be in their listing's currency;
income and investment expenses stay free.**

`trade::db::listing_currency_mismatch` is the shared half — the listing's currency when it differs,
else `None` — called by `trade::db_upsert`, `sell::db_upsert` and `amma::db_upsert`, each mapping it
to its own `CurrencyNotListings` variant and a `422` naming both currencies and what to do (enter
the contract note / statement converted, or pick the right listing). Checked **after** the write
inside the same transaction, like the return-of-capital rule beside it, so an unrecognised currency
code still meets its own foreign-key rejection first — "no such currency" is a better answer to
`ZZZ` than "not the listing's".

The one state that legitimately disagrees is not enterable: a rollover replacement parcel
(scrip-for-scrip, demerger, transfer) carries its consumed parcel's currency onto the new listing so
the carried AUD cost base survives the substitution, and `domain::rollover` writes those rows itself
rather than through `db_upsert`. That is exactly the state the return-of-capital currency check
exists for, so `corporate_action`'s two tests now construct it the way a rollover does.

`TradeBuilder::insert` and `AmmaBuilder::insert` default an unnamed currency to the **listing's**,
so a fixture built on a foreign listing is correct by construction rather than by repeating at each
call site what the listing already says; twelve fixtures that had put a foreign trade on an AUD
listing were given a listing quoted to match.

Tests: `api_trade_currency_must_be_the_listings` (the trade and the Sell paths, refusal naming both
currencies, nothing written, the matching currency accepted) and
`api_amma_currency_must_be_the_listings`. Docs: `docs/API.md` Trades (with the rollover exception
stated), AMMA statements, and the 422 catalogue.

## A stored RBA rate can never be corrected, and a differing feed value is silently discarded (SCENARIOS M-13)
(SCENARIOS.md section M verification pass, 2026-08-19. `rba_fx_rates` is written only by the import,
which is `INSERT … ON CONFLICT DO NOTHING`, and the resource is read-only over HTTP — no `PUT`, no
`DELETE`. First value wins, permanently.)
- [x] Reproduced: importing `29-Mar-2024,0.6500` then `29-Mar-2024,0.6512` answers
  `{"inserted": 0, "skipped": 1}` and stores 0.6500. The response cannot distinguish "the feed
  repeated what we had" from "the feed disagreed with what we had"
- [x] Consequence: a rate that lands wrong — a hand-supplied retry body with a typo (the endpoint
  accepts a pasted CSV precisely for retries), a truncated download, or an upstream revision — can
  be fixed only by editing the database by hand, and every tax figure in that currency-month rests
  on it. `rba_fx_rates` is also **not** in `row_history::AUDITED_TABLES`, so a hand-edit leaves no
  trace either
- [x] The idempotency the `DO NOTHING` buys is worth keeping: re-running the import must not
  rewrite history unasked, and a silently-changing rate would be worse than a stuck one
**Decision (2026-08-19, Evan): option (b) — report the disagreement *and* add the correction path,
with `rba_fx_rates` audited.**

- [x] A model decision, three options:
  - **(a)** Keep the import idempotent but *report* the disagreement: count `conflicted` separately
    from `skipped`, listing each (currency, month, stored, feed), and surface it in the job's
    failure detail and the health report. Nothing changes without the user asking
  - **(b)** (a), plus an explicit correction path — a `PUT /rba_fx_rates/:id` (or an import flag)
    that overwrites, with `rba_fx_rates` added to the audited tables so the old value is recorded
    and the snapshots it fed marked stale
  - **(c)** Documentation only: state in `docs/API.md` that the first imported value for a
    (currency, month) is final and a correction needs direct database access
- [x] Tests: per the option — a differing re-import is counted and named; an identical one is not;
  a correction (if (b)) restages the affected snapshots and writes a history row
- [x] Docs sync: `docs/API.md` RBA FX rates + Response codes; `docs/SCHEMA.md` and the three audited
  -table lists if (b)

**Resolution (2026-08-19): the disagreement is reported, and a stored rate is correctable — audited,
and staling the snapshots it fed.**

`db_import_rate` now answers an `ImportOutcomeRow` (`Inserted` | `Skipped` | `Conflicted`) instead
of a bool: where a `(currency, month)` already exists, its stored rate is compared with the feed's,
so a *disagreement* is separated from a repeat. `ImportSummary` carries `conflicted:
[{id, currency, month, stored, feed}]` (omitted when empty) and each one is logged at warn — a
scheduled run's response goes nowhere, so the log line is where the operator sees it. The import
itself is unchanged: `ON CONFLICT DO NOTHING`, because a scheduled run must never rewrite a figure a
lodged return was computed at.

`PUT /rba_fx_rates/:id` is the correction — one row by id, carrying only the new rate, since
`(currency, month)` is the row's identity and re-pointing it would silently move every conversion
that used it. `422` for a non-positive rate (every conversion divides by it; `apply_rate` panics on
zero), `404` naming what was missing. Migration `0031` puts `rba_fx_rates` in the audit trail — the
`closing_prices` story of 0021 exactly, and the same `row_history` rebuild to extend its
`table_name` CHECK — so the superseded figure stays recoverable, and adds the staleness trigger that
marks every snapshot from that month on stale when the rate itself changes.

Tests: `db_currency_month_uniqueness_enforced` and `import_is_idempotent` now separate the identical
re-import from the disagreeing one (naming the row and both rates);
`api_a_stored_rate_is_correctable_audited_and_stales_snapshots` drives the refusals, the correction,
the recovered old figure in `row_history`, and that a snapshot before the rate's month is untouched
while one inside it is stale. `row_history`'s three-list pin and its per-migration block cover 0031.
Docs: `docs/API.md` RBA FX rates + Row history + the 422 catalogue, `docs/SCHEMA.md`'s
`rba_fx_rates` block, audited-set paragraph and Relationships, and the UI's row-history table
picker.

## Foreign tax on a discountable foreign capital gain is claimed in full, not apportioned (SCENARIOS M-12)
(SCENARIOS.md section M verification pass, 2026-08-19. The FITO guide's "Foreign income tax paid on
part of an amount included in your income" (QC 104349, *When a FITO applies*) states: "If only part
of a foreign capital gain is assessable in Australia (for example, the gain is subject to the
discount capital gains concessions in Division 115 of the ITAA 1997) the foreign tax paid on the
gain must be apportioned accordingly. This includes, where a foreign capital gain is distributed to
a unitholder of a … (AMIT). In such circumstances, when calculating your FITO, the 'Foreign tax
offset applicable to discountable capital gains' shown at Part C … must be reduced for discounted
capital gains." The AMMA guidance notes confirm the trustee reports the **gross** foreign tax and
the reduction is the investor's job — this system's job.)
- [x] Reproduced: an AMMA statement with `cgt_discount_gains: 5000` and `foreign_tax_credits: 1500`
  reports `foreign_tax_offsets: 1000` and `foreign_tax_offset_excess: 500`. Apportioned to the
  assessable half, the claimable figure is A$750 — so the report's A$1,000 over-claims by A$250,
  and a smaller de-minimis-covered case over-claims by the full apportionment
- [x] The de-minimis cap bounds the damage at A$1,000 but does not remove it, and the excess figure
  the user is told they *may* claim with their own limit calculation is overstated by the whole
  un-apportioned amount
- [x] The blocker is the data model: `amma_statements.foreign_tax_credits` is one field for both
  "foreign tax on foreign income" and "foreign tax on foreign capital gains", which the AMMA's own
  Part C reports as separate lines. The apportionment applies only to the second, so it cannot be
  computed from what is stored
- [ ] The same is true of a *direct* foreign-taxed disposal: foreign tax paid on a capital gain the
  taxpayer realises themselves has nowhere to be recorded at all — **left open**, and narrower than
  it looks: a foreign country rarely taxes a non-resident's gain on listed shares, and the AMIT
  distribution path above is where a listed-share investor actually meets a foreign-taxed gain
**Decision (2026-08-19, Evan): option (a) — split the field and compute the apportionment.**

- [x] A model decision, two options:
  - **(a)** Split the field — a new `foreign_tax_credits_capital_gains` column (migration; the AMMA
    is audited, so its two `*_row_history_*` triggers must be dropped and re-created) — and apply
    the Division 115 reduction to that half in the tax summary, with the AMMA screen's field hint
    naming which Part C line each takes. The system then computes the ATO's figure
  - **(b)** Documentation only: a Known-limitations entry stating that a `foreign_tax_credits`
    figure attributable to a discountable foreign capital gain must be entered already reduced, with
    the ATO citation, and the AMMA field hint saying so
- [x] Mirror the ATO page into `docs/ato/` with its source URL and retrieval date and index it in
  `docs/ato/OVERVIEW.md` either way — nothing there covers the FITO apportionment rule today
  (`fito-limit.md` mirrors only the offset-limit page)
- [x] Tests: per the option — the apportioned offset computed, or `doc_checks.rs` for the entry
- [x] Docs sync: `docs/SCHEMA.md` + `config.js` if (a); `docs/API.md` Known limitations either way

**Resolution (2026-08-19): the AMMA's second foreign-tax line, and the apportionment computed.**

New mirror `docs/ato/fito-capital-gains-apportionment.md` (QC 104349 *When a FITO applies*, the
section this rests on, plus Examples 11 and 12), indexed in `docs/ato/OVERVIEW.md`.

Migration `0032` adds `amma_statements.foreign_tax_credits_capital_gains` — Part C's *other*
foreign-tax line — and re-creates the statement's two `*_row_history_*` triggers with it, per the
audited-table maintenance rule. It is **additional** to `foreign_tax_credits` and defaults to `0`,
so every existing row reports exactly what it does today: no migration can infer the split out of a
combined figure, and guessing at one would silently move a live tax figure. Moving a statement's
capital-gains portion across is a deliberate edit against its own Part C detail.

`reports::tax_summary::apportion_capital_gains_foreign_tax` does the member's step the trustee
deliberately leaves undone: `claimable = tax × (discount + indexation + other) ÷ (2 × discount +
indexation + other)` — the discount component is reported net, so it is grossed up to the amount the
tax was actually paid on. With only discount gains this is the halving the ATO describes; with a mix
it splits across the three methods. `foreign_tax_offsets_cgt_discount_reduction` surfaces what was
apportioned away (a new tax-summary line and CSV column), so the statement's Part C figure and the
report's line reconcile instead of silently disagreeing. `amma::db_upsert` refuses a capital-gains
tax figure with **no** capital gains behind it: there would be no proportion to apportion by, so the
whole amount would be claimed in full.

Tests: `db_amma_capital_gains_foreign_tax_is_apportioned_to_the_assessable_part` (discount-only, a
three-method mix, and foreign *income* tax left untouched) and
`api_capital_gains_foreign_tax_needs_capital_gains`. Docs: the new mirror + OVERVIEW,
`docs/API.md` AMMA statements (the two lines) + the FITO cap paragraph + the 422 catalogue,
`docs/SCHEMA.md`, the AMMA form field with its hint in `config.js`, the annual tax report's AMMA
breakdown, and `util.js`'s money-column list.

## The two documented FX simplifications are silent where their sibling is refused (SCENARIOS M-09, M-10)
(SCENARIOS.md section M verification pass, 2026-08-19. Both simplifications are honestly documented
and both behave exactly as documented — the verification confirmed each. What neither has is a
surface telling a user that *their* data has hit it, though in both cases the affected rows are
identifiable from stored facts. The third member of the family, LPR expenditure on a foreign
inherited parcel, was refused outright at write time in the section K pass for the same reason.)
- [x] **K10/K11 (M-09)**: reproduced with a US$1.5m disposal contracted 27 March (rate 0.66) and
  settled 2 April (rate 0.60). Proceeds convert at the contract month, correctly; the A$227,272 of
  settlement-window movement is a CGT event K10 gain or K11 loss the system does not compute, per
  the Known-limitations entry. A trade at risk is exactly identifiable: non-AUD, and
  `date`'s month ≠ `settlement_date`'s month
- [x] **Cost-base FX timing (M-10)**: reproduced with a USD parcel acquired at 0.70 taking a USD
  AMIT reduction whose own month is 0.60 — the reduction converts at 0.70 (A$2,857 where its own
  month gives A$3,333), keeping `initial − reductions = adjusted` exact in AUD. Affected rows are
  likewise identifiable: a non-AUD parcel with a non-AUD AMIT or return-of-capital reduction. The
  limitation says this "in practice does not arise"; nothing checks whether it has
- [x] Fix: surface both, non-blocking, on the `reports::settlement_coverage` model
- [x] Tests: a same-month settlement and an AUD trade produce no alert; a cross-month non-AUD
  settlement does; a non-AUD parcel with a non-AUD reduction does; an AUD fund with an AMIT
  reduction does not
- [x] Docs sync: `docs/API.md` — both Known-limitations entries gain the sentence naming where the
  affected rows are listed

**Resolution (2026-08-19): both are alert kinds on the new FX coverage report.**

`settlement_crosses_rate_month` lists every non-AUD trade whose contract and settlement months
differ — inside one month the K10/K11 component is nil by construction, so nothing is reported.
`reduction_converted_at_acquisition_month` lists every non-AUD parcel taking an AMIT or
return-of-capital reduction from another month, and where both months' rates are imported the
detail **names them both**, so the row says what the difference costs rather than only that one
exists. An AUD parcel is never listed: with one currency there is no conversion for the month to
matter to. Both Known-limitations entries now name the report, so "in practice does not arise" is
checkable against the data instead of assumed.

Tests: `db_a_settlement_crossing_a_rate_month_is_flagged` (a same-month settlement and an AUD trade
stay silent), `db_a_reduction_from_another_month_is_flagged_with_both_rates`,
`db_an_aud_parcels_reduction_is_not_flagged` and
`db_a_return_of_capital_from_another_month_is_flagged` (a parcel acquired after the payment is never
reached by it).
