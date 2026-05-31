# TODO

Items are only marked done when a passing test exists for them.

## Infrastructure
- [x] Add dependencies: sqlx (SQLite, tokio, chrono), tokio, chrono, chrono-tz, clap, serde, serde_json, axum (web server)
- [x] CLI arg parsing (`--db <path>`, default: `share-tracker.db`)
- [x] Database initialisation and connection pool
- [x] Daily backup on startup (copy DB to `<file>-YYYY-MM-DD.db`)
- [x] GitHub Actions CI: run tests on push
- [x] CI: verify no migration contains DROP TABLE or DROP COLUMN statements
- [x] Logging setup: tracing subscriber with INFO as default level, configurable via RUST_LOG
- [x] Tests: log output at INFO level; RUST_LOG override works
- [x] Database migration system (sqlx migrate): migrations run on startup, applied once
- [x] Tests: migrations apply cleanly on a fresh in-memory DB
- [x] Add rust_decimal dependency (with sqlx feature) for arbitrary-precision decimal arithmetic

## Reference Data — Exchange
- [x] Exchange model (MIC, name, country, currency, timezone, settlement period)
- [x] DB schema: `exchanges` table
- [x] Seed data for known exchanges (XASX, XNYS at minimum)
- [x] CRUD API endpoints for exchanges
- [x] Tests: insert, retrieve, upsert exchange

## Reference Data — Listing
- [x] Listing model (exchange FK, ticker, name, ISIN, security type, currency, AMIT flag)
- [x] DB schema: `listings` table
- [x] CRUD API endpoints for listings
- [x] Tests: insert, retrieve listing; FK constraint to exchange

## Reference Data — RBA FX Rate (the monthly rate used for ATO conversion)
- [x] FX Rate model (currency ISO 4217 code, month, rate as foreign-currency-per-AUD) — `src/rba_fx_rate.rs`, struct `RbaFxRate`
- [x] DB schema: `rba_fx_rates` table; rate stored as TEXT Decimal; UNIQUE on (currency, month) — migration `0010_rba_fx_rates.sql`
- [x] List/get API endpoints for FX rates (`GET /rba_fx_rates`, read-only over HTTP; writes come from the import via `db_import_rate`)
- [x] Tests: insert, retrieve; (currency, month) uniqueness enforced; rate decimal precision preserved in round-trip (`db_insert_and_retrieve`, `db_currency_month_uniqueness_enforced`, `db_decimal_precision_preserved_in_round_trip`, plus API tests)

## RBA FX Rate Import
- [x] Import logic: `run_import` fetches the RBA F11 "Exchange Rates" CSV (`RBA_FX_RATES_URL` = https://www.rba.gov.au/statistics/tables/csv/f11-data.csv) via reqwest; `parse_rates` parses the real F11 layout (BOM, `Title` row of `A$1=<code>` columns + a skipped trade-weighted-index column, monthly `DD-Mon-YYYY` data rows → foreign-per-AUD rate per currency/month, fails loudly on a malformed rate); `import_from_content`/`db_import_rate` upsert new (currency, month) rows via `ON CONFLICT DO NOTHING` so existing rows are never created twice or altered. Verified end-to-end against the live file (24 currencies, 2010-01..2026-05). The ATO directs taxpayers to these RBA rates; table/module/struct named `rba_fx_rate(s)`/`RbaFxRate`
- [x] Weekly scheduled task runs the import on a recurring interval (alongside the daily backup) — `spawn_weekly_import` in main.rs, mirrors `spawn_daily_backup`
- [x] HTTP endpoint to trigger the import manually for retries / missed runs, sharing the same idempotent import logic — `POST /rba_fx_rates/import` (empty body → fetch from RBA; non-empty body → import a supplied F11 CSV, for retries/offline); both call `import_from_content`
- [x] Tests: import is idempotent (re-run stores no duplicates, leaves existing rows unchanged); manual-trigger endpoint invokes the import (`import_is_idempotent`, `import_adds_only_new_rows_on_rerun`, `api_import_endpoint_invokes_import`, plus parse + malformed-feed tests)

## Reference Data — MIC Registry (ISO 10383 validation list)
- [x] MIC entry model (mic, operating_mic, name, country_code, city, status, expiry_date) — `src/entities/mic_registry.rs`, struct `MicEntry`. Reference data only: the ISO list carries no currency/timezone/settlement, so it is not the operational `exchanges` table
- [x] DB schema: `mic_registry` table keyed by `mic`, no FKs — migration `0011_mic_registry.sql`
- [x] List/get API endpoints (`GET /mic_registry`, `GET /mic_registry/:mic`, read-only over HTTP; writes come from the import)
- [x] Tests: insert/retrieve; upsert updates status; missing returns None/404 (`db_insert_and_retrieve`, `db_upsert_updates_existing_status`, `db_get_missing_returns_none`, plus API tests)

## MIC Registry Import
- [x] Import logic: `run_import` fetches the ISO10383_MIC CSV (`MIC_REGISTRY_URL` = https://www.iso20022.org/sites/default/files/ISO10383_MIC/ISO10383_MIC.csv) via reqwest; `parse_registry` parses the fully-quoted CSV with the `csv` crate (columns located by header name, EXPIRY DATE `YYYYMMDD`→`YYYY-MM-DD`, fails loudly on a missing column or malformed expiry); `import_from_content` upserts every row in one transaction via `ON CONFLICT(mic) DO UPDATE` so the registry tracks the latest ISO publication with no duplicates. Verified end-to-end against the live file (2853 MICs: 2289 ACTIVE / 555 EXPIRED / 9 UPDATED)
- [x] Monthly scheduled task runs the import — `mic-import` job in `infra::scheduler::registry`, scheduled `0 3 1 * *` in `schedule.cron`; logs `imported` count and next run time at INFO
- [x] HTTP endpoint to trigger the import manually (empty body → fetch from ISO; non-empty body → import a supplied CSV) — `POST /mic_registry/import`, shares `import_from_content`
- [x] Non-blocking exchange-MIC validation report — `GET /reports/exchange_mic_validation` (`src/reports/mic_validation.rs`) classifies each curated exchange as `ok`/`expired`/`unknown` against the registry; never blocks writes
- [x] Tests: import idempotent + reflects status changes on re-run; quoted/empty-cell/expiry parsing; malformed-feed/missing-column rejected; report classifies ok/expired/unknown and treats an empty registry as unknown (`import_inserts_all_rows_and_is_idempotent`, `import_reflects_status_changes_on_rerun`, `parse_registry_*`, `classifies_ok_expired_and_unknown`, `unknown_when_registry_empty`, plus API tests)

## FX Conversion (ATO reference rate)
- [ ] Conversion helper: AUD = foreign / Rate, using the ATO FX Rate for the amount's currency and the month of the relevant date (e.g. trade date); AUD amounts pass through (rate = 1)
- [ ] Fall back to the trade's manual FX Rate override (same foreign-per-AUD convention) only when no ATO FX Rate exists for that (currency, month); the ATO rate takes precedence once available
- [ ] Keep the trade FX Rate field as the optional manual override (no longer the primary source) — remains Decimal; document/comment it as a fallback so it isn't flagged as an unused field
- [ ] Fail loudly when neither an ATO FX Rate nor a manual override is available for a required conversion — never substitute a zero/default or leave the amount unconverted
- [ ] Tests: ATO rate used when present (takes precedence over the manual field); manual override used when ATO rate absent; neither present fails loudly

## Trade Activity
- [x] Trade model (type, date, settlement date, listing FK, average price, quantity, currency, brokerage, GST on brokerage, brokerage currency, FX rate, contract note reference)
- [x] DB schema: `trades` table
- [x] Auto-populate settlement date from trade date + exchange settlement period (overridable)
- [x] CRUD API endpoints for trades
- [x] Tests: buy, sell, DRP trades; settlement date auto-population; override of settlement date
- [x] Refactor financial fields (average_price, quantity, brokerage, gst_on_brokerage, fx_rate) from f64 to Decimal
- [x] Tests: decimal precision preserved in API round-trip

## Income Activity
- [x] Income model (listing FK, date paid, ex date, franked amount, unfranked amount, foreign source income, foreign tax paid, TFN withholding tax, franking credits, LIC capital gain deduction, conduit foreign income, trust income flag, reinvestment trade FK)
- [x] DB schema: `income` table
- [x] CRUD API endpoints for income
- [x] Tests: dividend income, trust distribution, DRP reinvestment linkage
- [x] Refactor financial fields (franked_amount, unfranked_amount, foreign_source_income, foreign_tax_paid, tfn_withholding_tax, franking_credits, lic_capital_gain_deduction, conduit_foreign_income) from f64 to Decimal
- [x] Tests: decimal precision preserved in API round-trip

## AMMA Statements
- [x] AMMA model (listing FK, tax year end date, units held, date received, australian interest, australian dividends unfranked, franked dividends, franking credits, net rent, foreign income, foreign tax credits, other income, CGT discount gains, CGT indexation gains, CGT other gains, capital losses applied, tax deferred amount, tax free amount, cost base adjustment per unit, TFN withholding tax)
- [x] DB schema: `amma_statements` table
- [x] All financial fields use Decimal (not f64): australian_interest, australian_dividends, franked_dividends, franking_credits, net_rent, foreign_income, foreign_tax_credits, other_income, cgt_discount_gains, cgt_indexation_gains, cgt_other_gains, capital_losses_applied, tax_deferred_amount, tax_free_amount, cost_base_adjustment, tfn_withholding_tax
- [x] CRUD API endpoints for AMMA statements
- [x] Tests: insert and retrieve AMMA statement; cost base adjustment calculation
- [x] Tests: decimal precision preserved in API round-trip

## Share Parcel Allocation
- [x] Parcel allocation model (sale trade FK, purchase trade FK, quantity allocated)
- [x] DB schema: `parcel_allocations` table
- [x] quantity_allocated uses Decimal (not f64)
- [x] Validate quantity allocated does not exceed available quantity on purchase trade
- [x] Validate total allocations for a sale trade do not exceed sale quantity
- [x] CRUD API endpoints for parcel allocations
- [x] Tests: allocation creation, over-allocation rejection
- [x] Validate sale_trade_id references a trade of type Sell
- [x] Validate purchase_trade_id references a trade of type Buy or DRP
- [x] Tests: type constraint violations rejected

## Cost Base Adjustments
- [x] AMIT cost base adjustment: apply AMMA `tax deferred` amounts to reduce cost base of affected parcels
- [x] Tests: AMIT adjustment

## Reporting — Portfolio Overview
- [x] Current holdings: aggregate open parcels by listing (quantity, average cost base)
- [x] Accept current market prices as input to materialise portfolio value
- [x] API endpoint for portfolio overview
- [x] Tests: holdings aggregation after buys and sells

## Reporting — Unrealised Gains/Losses
- [x] Calculate unrealised gain/loss per holding (market value vs cost base)
- [x] Apply 50% CGT discount indicator for parcels held > 12 months
- [x] API endpoint for unrealised gains/losses
- [x] Tests: gain/loss calculation, discount eligibility

## Reporting — Realised Gains/Losses
- [x] Calculate capital gain/loss per sale using allocated parcels and adjusted cost bases
- [x] Apply CGT discount (50%) for parcels held > 12 months
- [x] API endpoint for realised gains/losses
- [x] Tests: specific parcel sale, CGT discount eligibility

## Reporting — Tax
- [x] Aggregate all assessable income components by tax year
- [x] Aggregate franking credits, foreign tax offsets, TFN withholding tax by tax year
- [x] Include AMMA attributed income components in tax year totals
- [x] Include LIC capital gain deductions
- [x] Exclude conduit foreign income from assessable totals
- [x] API endpoint for tax summary by year
- [x] Tests: full-year tax summary with mixed income types

## Web Frontend
- [ ] Serve frontend from the Rust server (axum)
- [ ] Exchange management UI
- [ ] Listing management UI
- [ ] Trade entry and listing UI
- [ ] Income entry and listing UI
- [ ] AMMA statement entry and listing UI
- [ ] Share parcel allocation UI
- [ ] Portfolio overview UI
- [ ] Gains/losses report UI
- [ ] Tax summary UI

## Review Findings — Requirements Gaps
(FX source changed: conversion now uses the ATO FX Rate lookup, not a per-trade `fx_rate` — see the FX Conversion section.)
- [ ] Convert cost base and proceeds to AUD for non-AUD trades via the ATO FX Rate lookup (portfolio, realised, unrealised reports currently compute in raw trade currency)
- [ ] Convert income/AMMA foreign amounts to AUD via the ATO FX Rate lookup so tax summary totals are in AUD
- [ ] Tests: USD (XNYS) buy/sell produces AUD cost base and gain using the ATO FX Rate

## Review Findings — Needs Clarification (resolve intended behaviour before implementing)
- [ ] Confirm how the 50% CGT discount should be applied: reports currently expose only the gross eligible gain and never halve it, nor net losses against gains. Decide whether the tool should compute the discounted/assessable net capital gain or continue exposing components only
- [ ] Apply the 50% CGT discount to eligible gains (currently only the gross eligible gain is exposed, never halved)
- [ ] Net capital losses against gains and apply the discount to produce an assessable net capital gain
- [ ] Tests: discounted net capital gain after offsetting losses
- [ ] Confirm AMMA cost-base driver: the model annotates `tax_deferred_amount` as "reduces cost base", but calculations use the per-unit `cost_base_adjustment`; `tax_deferred_amount` and `tax_free_amount` are currently stored but unused. Decide which field(s) drive the adjustment
- [ ] Resolve AMMA cost-base driver per the decision above and remove or wire up the now-redundant fields
- [ ] Tests: cost base reflects the agreed AMMA field(s)

## Review Findings — Feature Gaps
- [ ] Net capital gain / overall tax-position report combining realised parcel gains, AMMA-attributed CGT gains, and AMMA capital losses applied
- [x] Prevent an under-allocated Sell from being persisted: new atomic `PUT /sells/{id}` (src/sell.rs) inserts the Sell trade + all its parcel allocations in one transaction and rejects (422) unless allocations sum exactly to the sell quantity and every parcel is a valid, not-over-allocated Buy/DRP. To keep the invariant: `PUT /trades/{id}` now rejects `Sell` (422), and `parcel_allocations` is read-only over HTTP (PUT/DELETE removed; allocations are managed via /sells).
- [x] Tests: Sell+allocations rejected when allocations don't sum to sell quantity (under and over), rejected on parcel over-allocation / non-Buy parcel, accepted and rolled-back-on-failure when valid; PUT /trades Sell → 422; parcel_allocations PUT/DELETE → 405 (src/sell.rs, src/trade.rs, src/parcel_allocation.rs)

## Review Findings — Implementation Issues
- [x] Settlement date should advance by business days, not calendar days (trade.rs `add_business_days` now skips weekends; public holidays still not modelled)
- [x] Tests: settlement date skips weekends (`add_business_days_skips_weekend`, `api_settlement_date_auto_populated_skips_weekend`)
- [ ] Model exchange public holidays so settlement dates skip them too
- [ ] Avoid CAST(REAL AS TEXT) float imprecision for any rows written before migration 0006 (trades/income were created as REAL in 0004/0005)
- [x] Surface malformed decimal values instead of silently coercing to zero via `.parse().unwrap_or(Decimal::ZERO)` in the report modules (shared `decimal::parse_dec` propagates a decode error; test `db_malformed_decimal_is_an_error_not_zero`)
- [x] Remove dead-code warnings from unused `UpsertError::Db(sqlx::Error)` field in parcel_allocation.rs and amit_adjustment.rs (now logged via `tracing::error!`)
- [x] Remove remaining dead-code warning: replaced the unused per-trade `amit_adjustment::db_cost_base_reduction` with a shared bulk `db_cost_base_reductions` (returns `HashMap<trade_id, Decimal>`); portfolio/realised/unrealised reports now call it instead of each re-implementing the AMIT-reduction query inline. Also fixes the silent `.parse().unwrap_or(Decimal::ZERO)` (now propagates via `parse_dec`). Test `db_cost_base_reduction_calculation` exercises the bulk helper.
