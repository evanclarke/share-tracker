# TODO

Items are only marked done when a passing test exists for them.

## Infrastructure
- [x] Add dependencies: sqlx (SQLite, tokio, chrono), tokio, chrono, chrono-tz, clap, serde, serde_json, axum (web server)
- [x] CLI arg parsing (`--db <path>`, default: `share-tracker.db`)
- [x] Database initialisation and connection pool
- [x] Daily backup on startup (copy DB to `<file>-YYYY-MM-DD.db`)
- [x] Switch the backup job from daily to weekly cadence (REQUIREMENTS now specifies *weekly* backups) — `schedule.cron`'s `backup` entry changed from `0 0 * * *` (daily) to `0 0 * * 0` (weekly, Sunday 00:00); on-demand `POST /jobs/backup` is unaffected
- [x] Backup filename includes time as well as date — `db::backup_path` now formats `<file>-YYYY-MM-DD-HHMMSS.db` (via `backup_path_at`), was date-only `<file>-YYYY-MM-DD.db`. The time-to-the-second component keeps each weekly run distinct; the skip-if-exists guard (`backup_to`) now only collides for two runs in the same second
- [x] Tests: backup filename carries the date-time component (`db::tests::backup_path_includes_date_and_time`); weekly `backup` schedule entry parses and fires 7 days apart (`scheduler::tests::backup_is_scheduled_weekly`)
- [x] Scheduled jobs log an INFO when started and an INFO when finished (REQUIREMENTS: "Jobs that are scheduled will log an info when started and finished") — shared `scheduler::run_job` brackets every run with `job started` / `job finished` (the finish line carries `ok`) INFO lines; both `scheduler::spawn`'s loop and the manual `POST /jobs/{name}` trigger go through it, so all jobs (backup, rba-fx-import, mic-import, currency-import) log both regardless of any per-job logging
- [x] Tests: a scheduled/triggered job emits both a started and a finished INFO log (`scheduler::tests::run_job_logs_started_and_finished` for the scheduled path, `triggered_job_logs_started_and_finished` for the HTTP trigger)
- [x] Persist each job's last run (started/finished timestamps, success, error text) across restarts — `job_runs` table (migration `0005_job_runs.sql`), one row per job keyed by name, upserted by `scheduler::db_record_run`. The shared `scheduler::run_job` records every run (scheduled loop + manual `POST /jobs/{name}` both go through it), so no job bypasses recording; a recording failure is logged but does not change the job's own result. `GET /jobs` now returns `{ name, last_started_at, last_finished_at, last_success, last_error }` per job (nulls until first run), driven by `db_last_runs`
- [x] Jobs web UI exposes the last run — `viewJobs` renders job name, description, last-finished timestamp, a status badge (`ok`/`failed`/`never`), and the error text through the shared `filterableTable`, with the run-now action reloading to show the freshly recorded run
- [x] Tests: a successful run is recorded and surfaced by `GET /jobs` (`scheduler::tests::run_job_records_successful_last_run`); a failed run persists `success=0` + error text and a later success overwrites it (`record_run_persists_failure_with_error`); a never-run job reports null last-run fields (`list_jobs_returns_registered_names`); the UI binds to the last-run fields (`web::tests::jobs_ui_present` asserts `last_finished_at`/`last_success`/`last_error` ship in the bundle)
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

## Reference Data — Currencies (ISO 4217 fiat + ISO 24165 digital tokens)
- [x] Currency model (kind enum Fiat/DigitalToken, code, numeric_code, name, short_name, minor_units, source enum Iso4217/Iso24165) — one table covering both fiat and digital tokens; `src/entities/currencies.rs`, struct `Currency`, enums `CurrencyKind`/`CurrencySource` (derive `sqlx::Type`)
- [x] DB schema: `currencies` table keyed by `code`; CHECK constraints on the kind and source enums; numeric_code nullable (fiat only); minor_units stored but commented informational-only (does not round stored amounts); migration `0015_currencies.sql`
- [x] List/get API endpoints (`GET /currencies`, `GET /currencies/:code`, read-only over HTTP; writes come from the import)
- [x] Tests: insert/retrieve; kind/source enum constraints enforced; missing returns None/404 (`db_insert_and_retrieve`, `db_upsert_updates_existing`, `db_kind_enum_constraint_enforced`, `db_source_enum_constraint_enforced`, `db_get_missing_returns_none`, plus API tests)

## Currency Reference Import
- [x] ISO 4217 import logic: fetch the SIX Group "List One" XML (`ISO_4217_URL` = https://www.six-group.com/dam/download/financial-information/data-center/iso-currrency/lists/list-one.xml) via reqwest; `parse_iso4217` walks the `<CcyNtry>` elements with quick-xml (code, numeric code, currency name, minor units), skips entries with no `<Ccy>` (e.g. ANTARCTICA), maps `N.A.` minor units to None, deduplicates a code shared across countries (EUR), and fails loudly on a malformed minor-unit value; upserts as kind Fiat / source Iso4217 idempotently (`ON CONFLICT(code)` — no duplicates, unchanged rows untouched)
- [x] ISO 24165 import logic: `parse_iso24165` parses the DTIF registry JSON (`{ "records": [ { "Header": {DTI…}, "Informative": {LongName, ShortNames} } ] }`) with serde_json — DTI → code, long name → name, first short name → short_name; skips records with no `Header.DTI` and fails loudly on a missing `records` array; upserts as kind DigitalToken / source Iso24165 idempotently. Live fetch is credential-gated (`ISO_24165_URL` = https://download.dtif.org/data.json requires DTIF Basic auth via `DTI_REGISTRY_USER_ID`/`DTI_REGISTRY_PASSWORD`); `run_import` skips the token fetch with a warning when unset (fiat still imports), so the live authed fetch path is not yet exercised by a test
- [x] Monthly scheduled task runs both imports (alongside the MIC monthly job) — `currency-import` job in `infra::scheduler::registry`, scheduled `0 4 1 * *` in `schedule.cron`; logs `imported` count, and the scheduler logs the next run time at INFO
- [x] HTTP endpoint to trigger the import manually (empty body → `run_import` fetches the live sources; non-empty body → `import_from_content` detects ISO 4217 XML vs ISO 24165 JSON from the leading char and imports the supplied content for retries/offline), sharing the same idempotent import logic — `POST /currencies/import`
- [x] Tests: both imports idempotent (re-run stores no duplicates, leaves existing rows unchanged); parse fiat XML and DTI JSON; malformed feed rejected; manual-trigger endpoint invokes the import (`import_iso4217_is_idempotent`, `import_iso24165_inserts_tokens`, `import_both_feeds_coexist_in_one_table`, `import_rejects_unrecognised_feed`, `parse_iso4217_handles_minor_units_dedup_and_missing_code`, `parse_iso4217_errors_on_malformed_minor_units`, `parse_iso24165_extracts_dti_names_and_skips_non_token_records`, `parse_iso24165_errors_when_records_missing`, `api_import_endpoint_invokes_import`, `api_import_endpoint_rejects_malformed_feed`)
- [x] Currency-code validation: enforced via DB foreign keys (blocking write-time). Every currency column references `currencies(code)` — `exchanges.currency`, `listings.currency`, `trades.currency`, `trades.brokerage_currency`, `income.currency`, `amma_statements.currency` — so an unrecognised code is rejected when the row is written, surfaced as 422 by the entity PUT handlers (see the 422-mapping item below). Added by migration `0017_currency_foreign_keys.sql`, which rebuilds the whole FK-connected cluster via the rename pattern (no data dropped; verified data preserved + `foreign_key_check` clean). Migration `0016_seed_currencies.sql` seeds a baseline (AUD/USD/major fiat + BTC/ETH) so the FKs hold without an import, and 0017 backfills any code already present in existing data. Tests: `listing::tests::db_fk_constraint_rejects_unknown_currency`, `trade::tests::db_unknown_currency_rejected_on_both_currency_columns`
- [x] Map currency/listing/exchange FK (and other constraint) violations on the entity PUT handlers to 422 instead of 500, per the data-integrity convention — shared `infra::http::write_error_status` maps foreign-key / check / unique / not-null violations to 422 and any other DB error to 500; wired into the exchange/listing/trade/income/amma upsert handlers. Test: `listing::tests::api_upsert_unknown_currency_returns_422`

## FX Conversion (ATO reference rate)
- [x] Conversion helper: AUD = foreign / Rate, using the ATO FX Rate for the amount's currency and the month of the relevant date (e.g. trade date); AUD amounts pass through (rate = 1) — `infra::fx::to_aud` (looks up `rba_fx_rates` by (currency, month))
- [x] Fall back to the trade's manual FX Rate override (same foreign-per-AUD convention) only when no ATO FX Rate exists for that (currency, month); the ATO rate takes precedence once available — `to_aud`'s `manual_override` param; ATO rate wins when present
- [x] Keep the trade FX Rate field as the optional manual override (no longer the primary source) — remains Decimal; document/comment it as a fallback so it isn't flagged as an unused field (`trade.rs` `fx_rate` doc comment; consumed as the fallback by the reports via `to_aud`)
- [x] Fail loudly when neither an ATO FX Rate nor a manual override is available for a required conversion — never substitute a zero/default or leave the amount unconverted (`FxError::MissingRate`; surfaces as a decode error → HTTP 500)
- [x] Tests: ATO rate used when present (takes precedence over the manual field); manual override used when ATO rate absent; neither present fails loudly (`infra::fx::tests`: `ato_rate_used_when_present`, `ato_rate_takes_precedence_over_manual_override`, `manual_override_used_when_no_ato_rate`, `fails_loudly_when_neither_rate_nor_override`, plus `aud_passes_through_without_a_rate`, `malformed_stored_rate_is_an_error_not_zero`)

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

## DRP (Dividend Reinvestment Plan)
- [x] DRP Enrolment model (listing FK, residual handling enum: CarryForward/PayOut) — `src/entities/drp_enrolment.rs`, struct `DrpEnrolment` keyed by `listing_id`, enum `ResidualHandling` (defaults CarryForward)
- [x] DB schema: `drp_enrolments` table; `listing_id` PRIMARY KEY + FK→listings (at most one enrolment per holding); CHECK constraint on residual_handling enum; default CarryForward — migration `0013_drp_enrolments.sql`
- [x] CRUD API endpoints for DRP enrolments — `/drp_enrolments` and `/drp_enrolments/:listing_id` (GET/PUT/DELETE); a bad listing FK on PUT → 422
- [x] Tests: insert/retrieve enrolment; one-per-listing uniqueness enforced; residual_handling enum constraint enforced; FK to listing (`db_insert_and_retrieve`, `db_one_enrolment_per_listing_upsert_updates`, `db_residual_handling_enum_constraint_enforced`, `db_listing_fk_enforced`, plus API tests)
- [x] Trade Activity: add residual_brought_forward, residual_carried_forward, residual_paid_out columns (DRP trades only) — Decimal stored as TEXT; migration `0012_trade_drp_residuals.sql` (ALTER TABLE ADD COLUMN, defaults '0', no data dropped)
- [x] Tests: residual fields round-trip with decimal precision preserved (`trade::tests::db_drp_residual_fields_round_trip_with_precision`, `db_non_drp_trade_defaults_residuals_to_zero`)
- [x] DRP reinvestment operation: create a DRP trade from a distribution (Income Activity) + reinvestment price — `src/entities/drp_reinvestment.rs` `db_reinvest`: reinvestable cash (`franked+unfranked+foreign_source−foreign_tax−tfn`, excludes franking credits) + residual brought forward (latest prior DRP trade's carried-forward for the listing, else 0) = available; quantity = floor(available / price); cost = quantity × price; leftover → carried-forward or paid-out per the enrolment's residual handling
- [x] DRP reinvestment is atomic: `POST /income/:id/reinvest` creates the Trade (Type DRP, listing + currency + pay date from the distribution, quantity, average price = reinvestment price, residual fields) and sets the distribution's reinvestment_trade FK in one transaction; returns 201 with the trade
- [x] Validation: reject (422) reinvestment for a non-enrolled holding, or a distribution that already has a reinvestment trade (at most one per distribution); also 422 on non-positive price, 404 on missing income
- [x] Tests: carry-forward residual is picked up by the next reinvestment for the holding; pay-out records leftover as paid out (not carried); whole-share floor; reinvestable cash excludes franking credits; atomic trade-creation + distribution linkage; rolled back on failure; rejected when not enrolled / already reinvested (`drp_reinvestment::tests`: `carry_forward_buys_whole_shares_and_carries_leftover`, `carried_residual_is_picked_up_by_the_next_reinvestment`, `pay_out_records_leftover_as_paid_not_carried`, `franking_credits_are_excluded_from_reinvestable_cash`, `not_enrolled_is_rejected_and_nothing_persisted`, `already_reinvested_is_rejected`, `missing_income_is_not_found`, `non_positive_price_is_rejected`, plus API tests)

## Document Attachments
Attach supporting documents (trade confirmation PDF, dividend statement, AMMA scan) to a Trade, Income, or AMMA Statement. Contents are stored in the DB as a BLOB so the existing weekly backup captures them. Binary payload, so the API is not the JSON-CRUD convention (multipart upload, raw-bytes download, metadata-only list/get).
- [x] Add `sha2` dependency for the SHA-256 content checksum
- [x] Attachment model — `src/entities/attachment.rs`, struct `Attachment`: id, exactly-one owner FK (`trade_id` / `income_id` / `amma_statement_id`, others null), `filename`, `content_type` (enum `ContentType` over the MIME allowlist; `sqlx::Type` + serde `rename` so it round-trips as the MIME string), `byte_size`, `checksum` (SHA-256 hex via `checksum_hex`), `uploaded_at`. The `content` BLOB is absent from this metadata struct — loaded only by `db_get_content` for the download path
- [x] DB schema: `attachments` table — three nullable owner FK columns, each `REFERENCES <activity>(id) ON DELETE CASCADE`; `CHECK` that exactly one owner is non-null (`(trade_id IS NOT NULL) + (income_id IS NOT NULL) + (amma_statement_id IS NOT NULL) = 1`); `content_type` CHECK enum (`application/pdf`, `image/png`, `image/jpeg`); `content` BLOB NOT NULL; `byte_size` INTEGER, `checksum`/`filename`/`uploaded_at` TEXT — migration `0004_attachments.sql`. The pool already sets `foreign_keys(true)` (`infra::db::init`), so the cascade fires
- [x] Upload endpoint: `POST /attachments` multipart/form-data (file + target owner) → 201 with metadata; computes `byte_size` + `checksum` server-side; unsupported/absent content type → 422, missing/>1 owner → 422, unknown owner activity (FK violation via `write_error_status`) → 422, oversized (> 25 MB `MAX_UPLOAD_BYTES`) → 413. The route's `DefaultBodyLimit` is raised above the per-file cap so the explicit size check (not axum's default 2 MB limit) is what returns 413
- [x] Metadata list/get: `GET /attachments` (filterable by owner via `?trade_id=` / `?income_id=` / `?amma_statement_id=`, built with `QueryBuilder`) and `GET /attachments/:id` select only the metadata columns — the blob is never returned
- [x] Content download: `GET /attachments/:id/content` streams the raw bytes with the stored `Content-Type` and a `Content-Disposition` filename (quotes/backslashes stripped); 404 if unknown
- [x] Delete endpoint: `DELETE /attachments/:id` → 204, or 404 if unknown
- [x] Cascade: deleting a Trade / Income / AMMA Statement removes its attachments automatically (`ON DELETE CASCADE`), so no orphaned blobs remain
- [x] README sync: `attachments` table in the Database schema + Relationships sections; an Attachments section in the HTTP API; 413 added to Response codes (and the 201/422 rows extended); web-frontend paragraph mentions the Attachments action
- [x] Tests (`attachment::tests`): upload returns 201 + metadata and stores content; checksum + byte_size computed correctly; round-trip download returns the exact bytes with the stored content-type + filename; content type outside the allowlist → 422; missing-owner and two-owners → 422; unknown owner activity → 422; oversized upload → 413; metadata list excludes the blob + filters by owner; deleting the owning activity cascades; delete unknown → 404; plus DB-level exactly-one-owner and content_type CHECK enforcement

## Cost Base Adjustments
- [x] AMIT cost base adjustment: apply AMMA `tax deferred` amounts to reduce cost base of affected parcels
- [x] Tests: AMIT adjustment
- [x] CGT event E10: when cumulative AMIT cost base reductions on a parcel exceed its cost base, floor the cost base at nil (never negative) and report the excess as a capital gain in the AMMA statement's income year — cost base floored in the portfolio/unrealised/realised reports (`(initial_cost - amit).max(0)`); `net_capital_gain::e10_gains` walks each parcel's adjustments in tax-year order, emits the per-year excess (converted to AUD at the parcel's buy-month rate), classifies it discount-eligible by the holding period as at `tax_year_end_date`, and folds it into the year's gain buckets; new informational `cgt_event_e10_gain` response field. See `docs/amit-cost-base-adjustments.md`
- [x] Tests: E10 excess becomes a capital gain (non-discount + discount-eligible), accumulates across years and fires only once the cost base is exhausted, and cost base floors at nil (`net_capital_gain::tests::db_e10_excess_reduction_becomes_capital_gain`, `db_e10_gain_discount_eligible_when_held_over_12_months`, `db_e10_accumulates_across_years_fires_when_cost_base_exhausted`, `portfolio::tests::db_amit_reduction_capped_at_nil_cost_base`)

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
A no-build-step single-page app (plain HTML/CSS/JS) embedded in the binary with `include_str!` and served by axum (`src/web.rs` + `src/web/{index.html,app.js,style.css}`, merged in `app::router`). The SPA is config-driven: each domain entity is described once (API path, key, fields) and generic list/form code renders its CRUD view; reports render as tables. It drives the existing JSON API on the same origin, so there is no second source of truth. Tests live in `src/web::tests`: the served shell/assets return the right status + content-type, and — since there is no browser harness — each UI item is covered by asserting its view (and the API endpoint it drives) is present in the shipped `app.js` bundle.
- [x] Serve frontend from the Rust server (axum) — `web::router` serves `GET /`, `/static/app.js`, `/static/style.css` with correct content-types (`index_is_served_as_html`, `app_js_is_served_as_javascript`, `style_css_is_served_as_css`)
- [x] Exchange management UI — generic CRUD view over `/exchanges` (`exchange_management_ui_present`)
- [x] Listing management UI — generic CRUD view over `/listings`, with exchange/currency dropdowns (`listing_management_ui_present`)
- [x] Trade entry and listing UI — generic CRUD view over `/trades` for Buy/DRP (Sells excluded — entered via the Sells view); optional settlement date auto-calculates (`trade_ui_present`)
- [x] Income entry and listing UI — generic CRUD view over `/income`, full tax-component fields (`income_ui_present`)
- [x] AMMA statement entry and listing UI — generic CRUD view over `/amma_statements` (`amma_statement_ui_present`)
- [x] Share parcel allocation UI — bespoke Sells view: a Sell trade form with a dynamic allocations list, submitted atomically via `PUT /sells/:id`; `parcel_allocations` shown read-only (`parcel_allocation_ui_present`)
- [x] DRP enrolment management + reinvest-distribution UI — CRUD over `/drp_enrolments` (keyed by listing); income rows expose a Reinvest action driving `POST /income/:id/reinvest` (`drp_enrolment_ui_present`, `income_ui_present`)
- [x] Portfolio overview UI — `/portfolio/overview` report view with a per-listing price form (`portfolio_overview_ui_present`)
- [x] Gains/losses report UI — `/portfolio/unrealised-gains` (price + as-of-date form), `/portfolio/realised-gains`, and `/portfolio/net-capital-gain` report views (`gains_report_ui_present`)
- [x] Tax summary UI — `/portfolio/tax-summary` report view (`tax_summary_ui_present`)
- [x] Attachments UI on the Trade / Income / AMMA views — each of those entities carries an `attachOwner` field name; the generic list adds an "Attachments" row action linking to `#/attachments/<owner>/<id>`. `viewAttachments` lists an activity's attachments through the shared `filterableTable`, uploads a new file via `FormData` → `POST /attachments` (browser sets the multipart boundary + part content-type), links each row to its download (`/attachments/:id/content`), and deletes. Test `web::tests::attachments_ui_present` asserts `viewAttachments` + `/attachments` + `attachOwner` ship in the bundle (no browser harness)
- Also wired into the SPA (no separate TODO item): read-only views for currencies / MIC registry / RBA FX rates / parcel allocations, the AMIT adjustments CRUD view, exchange holidays CRUD, the exchange MIC validation report, and a Maintenance → Jobs view that lists and triggers the scheduled jobs on demand via `GET /jobs` + `POST /jobs/:name` (`jobs_ui_present`).
- [x] Web UI tables are filterable and sortable (REQUIREMENTS: "Tables in the Web UI should be filterable and sortable") — shared `filterableTable(rows, cols, opts)` in `app.js` renders every data table (entity lists, the Sells list, and report tables via `dataTable`) with a per-column filter row (one input per column, substring match, AND-combined so e.g. currency="USD" + date="2024" filter together) and click-to-sort headers (toggle asc/desc, numeric columns sort numerically). `opts.actions` keeps the trailing Edit/Delete column non-sortable/non-filtered; no per-entity code, no parallel data path
- [x] Tests: the filter input and sortable-column controls are present in the served `app.js` bundle (`web::tests::tables_are_filterable_and_sortable` asserts `filterableTable`/`table-filter`/`sortable` ship in the bundle, per the no-browser-harness convention)

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
- [x] Confirm AMMA cost-base driver: the model annotates `tax_deferred_amount` as "reduces cost base", but calculations use the per-unit `cost_base_adjustment`; `tax_deferred_amount` and `tax_free_amount` are currently stored but unused. Decide which field(s) drive the adjustment — RESOLVED per ATO guidance (`docs/amit-cost-base-adjustments.md`): for an AMIT the cost base is adjusted by the single **AMIT cost base net amount** stated on the AMMA statement (the per-unit `cost_base_adjustment` field). Tax-deferred and tax-free amounts are NOT direct cost-base drivers — the ATO says they are only "broadly reflected" in that net amount. So `cost_base_adjustment` is the driver; `tax_deferred_amount`/`tax_free_amount` are informational-only.
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

## ATO worked-example acceptance tests
(API-level tests reproducing the worked examples from the ATO guidance mirrored in `docs/` — each test cites its document + example, enters the facts purely via the HTTP API, and asserts the figures the ATO states. `src/ato_examples.rs`, a `#[cfg(test)]`-only module.)
- [x] `docs/cgt-how-to-calculate.md` "Example: CGT with discount" (Justin: $10,000 gain held 18 months → declares $5,000) — `ato_examples::cgt_how_to_calculate_example_cgt_with_discount`
- [x] `docs/cgt-how-to-calculate.md` "Example: working out CGT for a single asset" (Rhi's property: $530,000 all-in costs vs $600,000 → $70,000 gain, $35,000 net) — `ato_examples::cgt_how_to_calculate_example_single_asset`
- [x] `docs/cgt-how-to-calculate.md` "Example: working out CGT for multiple assets" (adds the $4,500 share loss: losses before the discount → $65,500 → $32,750 net) — `ato_examples::cgt_how_to_calculate_example_multiple_assets`
- [x] `docs/lic-capital-gain-deduction.md` "Example: Resident individual" (Ben: $70 franked, $30 credit, $25 LIC deduction in the FY2025 tax summary) — `ato_examples::lic_capital_gain_deduction_example_resident_individual`
- [x] `docs/cgt-dividend-reinvestment-plans.md` "Example: dividend reinvestment plans" (Natalie: $360 dividend reinvested at $8 → 45 new shares acquired for $360 on 20 Dec 2024; the $360 stays assessable in FY2025) — `ato_examples::drp_example_natalie_reinvested_dividend`, driving DRP enrolment + `POST /income/:id/reinvest` + overview + tax summary
- [x] `docs/cgt-keeping-records-shares.md` "Example: identifying when shares or units were acquired" (Boris nominates the 2024 $10 parcel for his 1,500-share sale at $8 → $3,000 capital loss in FY2025, keeping 1,000 @ $5 + 1,500 @ $10 = $20,000 cost base) — `ato_examples::keeping_records_example_boris_identifying_shares_sold`, driving specific parcel allocation via `PUT /sells`
- [x] `docs/you-and-your-shares-dividends.md` Examples 1–2 (John: $700 franked + $200 unfranked + $300 credit → $1,200 total assessable dividend income in FY2025) — `ato_examples::you_and_your_shares_examples_1_2_john_assessable_dividend_income`
- [ ] `docs/you-and-your-shares-dividends.md` "Example 6" (Matthew: held < 45 days, credits > $5,000 → the $5,600 franking credits are denied) — test written and `#[ignore]`d (`ato_examples::you_and_your_shares_example_6_matthew_holding_period_rule`); blocked on the "Franking-credit entitlement rules" section below — un-ignore when implemented
- [ ] `docs/cgt-non-assessable-payments.md` "Example 45" (Rob: 50c/share return of capital reduces the cost base to $4.50/share, no capital gain) — test written and `#[ignore]`d with a speculative entry API (`ato_examples::cgt_non_assessable_payments_example_45_rob_return_of_capital`); blocked on the "Corporate actions" section below (CGT event G1) — adjust the entry API and un-ignore when implemented
- [ ] `docs/cgt-cost-base.md` worked examples (capital works deduction on reduced cost base; recouped expenditure) — blocked on the "Reduced cost base and the five cost-base elements" section below (NEEDS CLARIFICATION; the asserted outcome depends on the clarification, so no ignored test is written yet)
- [ ] `docs/lic-capital-gain-deduction.md` "Example: Beneficiary of a trust or partner in partnership" — blocked on the "Taxpayer entity type" section below (partnerships/trusts not modelled, NEEDS CLARIFICATION)
- [ ] `docs/you-and-your-shares-dividends.md` "Example 7" (Jessica: last-in-first-out identification for the 45-day rule) — blocked on the "Franking-credit entitlement rules" section below; add alongside Matthew's test when the LIFO identification is modelled
- [ ] "Guide to foreign income tax offset rules 2025" Example 16 (Anna: $3,400 foreign tax limited to a $2,321 offset) — not reproducible in this system: the offset-limit calculation needs the taxpayer's full income-tax position (employment income, deductions, Medicare levy), which is outside the data model. The FITO section below covers only the $1,000 de-minimis cap computable from this system's data

## Capital-loss carry-forward across years
(REQUIREMENTS "Planned Enhancements — Capital-loss carry-forward across years". Net capital losses carry forward indefinitely and apply before the discount, per `docs/cgt-using-capital-losses.md`. Today `net-capital-gain` computes the current year's `capital_loss_carried_forward` but never consumes a prior year's carried-forward loss in a later year, so post-loss years are overstated.)
- [x] Chain carried-forward losses across the year series in `/portfolio/net-capital-gain`: an unused net capital loss from one year is applied in the next year that has gains (non-discountable gains first, then discount-eligible, then halve the remainder) — `db_net_capital_gain` now walks the years ascending with a running brought-forward balance: each year nets gains against `capital_losses + capital_loss_brought_forward` (non-discountable first, losses always before the discount), and the unused excess (`capital_loss_carried_forward`) becomes the next year's brought-forward. New response field `capital_loss_brought_forward`; `capital_losses` remains only the losses arising that year
- [x] Add an enterable opening carried-forward capital loss (losses from before the first year in the system), stored as a recognised data-model value (not derived) and used as the starting balance — DB schema + migration (no data dropped) + write path — singleton `cgt_settings` table (migration `0006_cgt_settings.sql`, `CHECK (id = 1)` so at most one row), entity `src/entities/cgt_settings.rs` with GET/PUT/DELETE at `/cgt_settings(/:id)`; PUT rejects a negative amount or id ≠ 1 with 422; absent row reads as zero (`db_opening_capital_loss`), which seeds the report's loss chain. A CGT Settings CRUD view is added to the SPA `ENTITIES` config
- [x] Tests: an earlier-year loss reduces a later year's net capital gain; a loss fully absorbing later gains leaves zero assessable and carries the remainder forward; an entered opening loss balance is applied (`net_capital_gain::tests::db_earlier_year_loss_reduces_later_year_gain`, `db_loss_absorbing_later_gains_leaves_zero_and_carries_remainder`, `db_opening_capital_loss_is_applied_as_starting_balance`, `db_opening_loss_chains_through_a_loss_year_in_order`; plus `cgt_settings::tests` — CRUD round-trip with decimal precision, singleton CHECK, negative/non-singleton-id 422s, zero default — and `web::tests::cgt_settings_ui_present`)
- [x] README sync: net-capital-gain report description (cross-year carry + opening balance), schema/endpoint for the opening loss balance — Features bullet, web-frontend paragraph, `cgt_settings` in Database schema + the standalone-tables Relationships note, a CGT settings HTTP API section, the net-capital-gain computation/response-fields description, and the 422 response-code row

## Reduced cost base and the five cost-base elements
(REQUIREMENTS "Planned Enhancements — Reduced cost base and the five cost-base elements", `docs/cgt-cost-base.md`.)
- [ ] NEEDS CLARIFICATION: decide whether to model the ATO reduced cost base (for losses — excludes element 3, no indexation) as distinct from the cost base, or document the single-cost-base behaviour as a known limitation
- [ ] NEEDS CLARIFICATION: decide whether to capture cost-base elements beyond acquisition (1) and incidental/brokerage (2) — element 3 (ownership costs), 4 (capital improvements), 5 (title/defence costs)
- [ ] If elements 3–5 in scope: model per-parcel additional cost-base costs (DB schema + migration) and include them in cost base (excluding element 3 from the reduced cost base) in the portfolio/unrealised/realised/net-capital-gain reports
- [ ] Tests: additional cost-base costs flow into the cost base; element 3 is excluded from the reduced cost base used for losses
- [ ] README sync: cost-base composition in the report descriptions + any new schema

## Taxpayer entity type and CGT discount rate
(REQUIREMENTS "Planned Enhancements — Taxpayer entity type and CGT discount rate". Discount is currently hard-wired to the individual 50% rate.)
- [ ] NEEDS CLARIFICATION: decide whether to introduce a taxpayer-entity concept (Individual, SMSF/complying super, Company, Trust/Partnership) driving the CGT discount rate (50% / 33⅓% / 0% / 50%) and the LIC deduction rate (`docs/lic-capital-gain-deduction.md`)
- [ ] If entity type in scope: model it (DB schema + migration), drive the discount and LIC-deduction rates from it in `/portfolio/net-capital-gain` and the tax summary
- [ ] If not yet modelled: state the individual-resident 50% assumption explicitly in the report output and README
- [ ] Tests: discount/LIC rates vary correctly by entity type (or: the individual-resident assumption is surfaced)

## Franking-credit entitlement rules
(REQUIREMENTS "Planned Enhancements — Franking-credit entitlement rules". `ex_date` is already captured and is the input the at-risk holding-period test needs. ATO worked example mirrored in `docs/you-and-your-shares-dividends.md`; an acceptance test for it is already written and `#[ignore]`d — `ato_examples::you_and_your_shares_example_6_matthew_holding_period_rule` — un-ignore it as part of implementing this section. Jessica's Example 7 (LIFO identification) should get a test alongside it.)
- [ ] Apply the 45-day holding-period rule (90 days for preference shares) to decide whether a dividend's franking credits are claimable
- [ ] Apply the $5,000 small-shareholder exemption (franking offsets up to $5,000/year claimable without the holding-period rule)
- [ ] Tax summary reflects only claimable franking credits (or clearly flags credits at risk of disallowance), not all attached credits
- [ ] Tests: a dividend held under 45 days has its franking credits excluded; the small-shareholder exemption restores credits below the $5,000 threshold
- [ ] README sync: tax summary franking-credit treatment

## Foreign income tax offset (FITO) cap
(REQUIREMENTS "Planned Enhancements — Foreign income tax offset (FITO) cap", `docs/mytax-managed-funds.md`. Tax summary currently sums foreign tax with no cap.)
- [ ] Apply the FITO limit: offsets above $1,000/year capped unless the full offset-limit calculation supports more
- [ ] Tests: foreign tax under $1,000 passes through; above $1,000 is limited to the computed cap
- [ ] README sync: tax summary FITO treatment

## Corporate actions / additional CGT events
(REQUIREMENTS "Planned Enhancements — Corporate actions / additional CGT events". Only A1 and E10 are modelled today.)
- [ ] NEEDS CLARIFICATION: decide scope and data model for recording corporate actions per holding/parcel
- [ ] Share split / consolidation: adjust quantity and per-unit cost base, preserving total cost base and the original acquisition date for the discount
- [ ] Bonus shares: new parcels with apportioned cost base
- [ ] Rights issues: new parcels with their cost-base treatment
- [ ] Return of capital (non-AMIT, CGT event G1): reduce cost base, distinct from the AMIT tax-deferred amount — ATO worked example mirrored in `docs/cgt-non-assessable-payments.md` (Rob, Example 45); an acceptance test is already written and `#[ignore]`d with a speculative entry API (`ato_examples::cgt_non_assessable_payments_example_45_rob_return_of_capital`) — adjust its entry call to the real endpoint and un-ignore as part of implementing this
- [ ] Off-market share buy-back: split into capital and dividend components
- [ ] Merger / takeover / demerger incl. scrip-for-scrip rollover: parcel substitution carrying the original cost base and acquisition date
- [ ] Security identity continuity across a ticker/name change, so a renamed listing's parcels are not orphaned
- [ ] Tests: each modelled action produces the correct adjusted parcels, cost base, and preserved acquisition date
- [ ] README sync: new entities/endpoints and their schema + relationships

## Accounts / ownership dimension
(REQUIREMENTS "Planned Enhancements — Accounts / ownership dimension". Everything is one flat portfolio today.)
- [ ] NEEDS CLARIFICATION: decide whether to introduce an account/owner entity (Individual, Joint, SMSF, Family Trust) partitioning all holdings and reports per taxpayer
- [ ] If in scope: model the account entity (DB schema + migration); add an account FK to trades, income, AMMA statements, DRP enrolments; allow every report to be produced per account (FX/AUD rules unchanged within each)
- [ ] Tests: gains and tax summaries are partitioned correctly across two accounts
- [ ] README sync: account entity + per-account report parameters

## Buy-trade edit/delete integrity (symmetric with Sells)
(REQUIREMENTS "Planned Enhancements — Buy-trade edit/delete integrity". The Sell path enforces a write-time invariant; the Buy path does not.)
- [ ] Reject deleting a Buy/DRP trade referenced by a parcel allocation or AMIT adjustment with a clear `422` (or `409`) instead of surfacing the SQLite FK error as `500`
- [ ] Reject `PUT /trades/:id` editing a Buy/DRP when the new quantity falls below the quantity already allocated out of it or covered by AMIT adjustments (`422`)
- [ ] Tests: delete of a consumed Buy rejected; edit shrinking a partly-sold Buy rejected; an unconsumed Buy still edits/deletes freely
- [ ] README sync: response-code/behaviour notes on the Trades endpoints

## Open-parcel cost-base inventory report
(REQUIREMENTS "Planned Enhancements — Open-parcel cost-base inventory report". Portfolio overview only aggregates per listing.)
- [ ] New report listing every open (unsold) parcel: listing, acquisition date, original cost base, cumulative AMIT reductions to date, remaining quantity, remaining adjusted cost base (AUD)
- [ ] Web UI view for the open-parcel inventory report (routed through the shared filterable table)
- [ ] Tests: open parcels listed with correct remaining quantity and adjusted cost base after partial sells and AMIT adjustments
- [ ] README sync: new report endpoint + web-frontend mention

## Tax-return export
(REQUIREMENTS "Planned Enhancements — Tax-return export". Reports are JSON/HTML only.)
- [ ] Export the tax summary and net-capital-gain reports to a downloadable, tax-return-ready format (CSV at minimum)
- [ ] Web UI export action on those report views
- [ ] Tests: the export endpoint returns the report rows in the chosen format with the expected columns
- [ ] README sync: export endpoints + response content types

## Performance / return metrics
(REQUIREMENTS "Planned Enhancements — Performance / return metrics".)
- [ ] NEEDS CLARIFICATION: decide whether to report investment performance (total return, money-weighted return/IRR, income/dividend yield per holding and overall)
- [ ] If in scope: implement the chosen performance report(s) + Web UI view
- [ ] Tests: performance metrics computed correctly over a known trade/income history

## Settlement-holiday coverage alerting
(REQUIREMENTS "Planned Enhancements — Settlement-holiday coverage alerting". Holidays are seeded only 2024–2027; settlement silently degrades to weekends-only beyond that.)
- [ ] Surface (warn/flag) when a trade's date or computed settlement window falls outside the seeded holiday coverage for its exchange, rather than silently using an incomplete calendar
- [ ] Tests: a trade dated beyond the seeded holiday range is flagged
- [ ] README sync: note the coverage-alert behaviour on the Trades / Exchange holidays sections
