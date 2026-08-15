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
