# Done — Infrastructure, FX/Reference-Data Imports, Backups & Scheduler

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

## Settlement-holiday coverage alerting
(REQUIREMENTS "Planned Enhancements — Settlement-holiday coverage alerting". Holidays are seeded only 2024–2027; settlement silently degrades to weekends-only beyond that.)
- [x] Surface (warn/flag) when a trade's date or computed settlement window falls outside the seeded holiday coverage for its exchange, rather than silently using an incomplete calendar — coverage is the calendar-year span of the exchange's seeded holidays (1 Jan of the earliest's year to 31 Dec of the latest's; no holidays = no coverage), via `exchange_holiday::coverage_span`/`coverage_span_for`/`window_outside_coverage`. Two non-blocking surfaces: (1) **warn** — settlement auto-population on `PUT /trades/:id` and `PUT /sells/:id` logs a WARN (`trade::warn_if_outside_holiday_coverage`) when the computed `[date, settlement_date]` window leaves coverage (weekend-only skipping); (2) **flag** — new report `GET /reports/settlement_holiday_coverage` (`src/reports/settlement_coverage.rs`, MIC-validation pattern) lists every persisted trade whose window is outside its exchange's coverage, with `coverage_status` (`outside_holiday_coverage` / `no_holiday_coverage`) and the `coverage_start`/`coverage_end` span; trades fully inside coverage are omitted. SPA `REPORTS` entry `settlement-holiday-coverage` (status badged via `statusField`, badge styles in `style.css`)
- [x] Tests: a trade dated beyond the seeded holiday range is flagged — `settlement_coverage::tests::db_trade_beyond_seeded_holiday_range_is_flagged` (plus `db_trade_inside_coverage_is_not_flagged`, `db_trade_before_seeded_holiday_range_is_flagged`, `db_window_straddling_coverage_end_is_flagged`, `db_exchange_without_seeded_holidays_is_flagged_as_no_coverage`, `api_get_settlement_holiday_coverage`); the WARN fires/doesn't fire (`trade::tests::api_settlement_beyond_holiday_coverage_logs_warning`, `api_settlement_inside_holiday_coverage_does_not_warn`); span/window helpers (`exchange_holiday::tests::coverage_span_spans_whole_calendar_years`, `window_outside_coverage_checks_both_ends_and_no_coverage`); UI in the bundle (`web::tests::settlement_coverage_report_ui_present`)
- [x] README sync: note the coverage-alert behaviour on the Trades / Exchange holidays sections — coverage-span + WARN/report note on Exchange holidays, settlement-calculation note on Trades (incl. the Sells path), a "Settlement holiday coverage" report subsection under Portfolio reports, a Features bullet, and the report added to the web-frontend view list

## Operational hardening — restore, off-disk backups, localhost default (2026-06-10)

(REQUIREMENTS 2026-06-10.)

- [x] `--backup-dir` option (default: beside the DB, as today) so backups can land on another volume; scheduler/backup job honours it — `infra::args::Args::backup_dir` (clap `--backup-dir`, `Option<String>`, `None` = beside the DB as before); `db::backup`/`backup_path` take the optional dir (only the filename moves there — the db's own directory component is dropped — and a missing dir is `create_dir_all`'d rather than failing the weekly job); `scheduler::registry` gains the `backup_dir` parameter and the `backup` job closure passes it through; `main` wires `args.backup_dir` in
- [x] Document the restore procedure in the README and prove it with a test (backup → mutate → restore → assert pre-mutation state) — README "Restoring from a backup" section (stop the server, replace the db file with the backup, delete the stale `-wal`/`-shm` sidecars, restart; each backup is a standalone `VACUUM INTO` database, post-backup entries are gone). Proven by `db::tests::restore_round_trip_recovers_pre_mutation_state`: insert → backup → mutate → close → open the backup copy via `init` (migrations run like a normal startup) → pre-backup row present, post-backup row gone. The test restores at a fresh path rather than copying over the live file: sqlx's sqlite workers tear down asynchronously after `Pool::close()` and their close-time WAL checkpoint races a same-path copy in-process (flaky both directions); the real procedure runs with the server process exited
- [x] Default `--host` changes to `127.0.0.1`; `0.0.0.0` remains opt-in and the README security note inverts accordingly — clap default flipped; the note now states the no-auth server is localhost-only by default and `--host 0.0.0.0` is the explicit trusted-network opt-in
- [x] Tests: backup lands in the configured dir; restore round-trip; default-bind assertion — `db::tests::{backup_path_honours_backup_dir, backup_lands_in_configured_dir}` (filename-only relocation; file written in a created-on-demand subdir and *not* beside the db), `scheduler::tests::backup_job_honours_configured_backup_dir` (the registry's job writes to the dir, not beside the db), `db::tests::restore_round_trip_recovers_pre_mutation_state`, `args::tests::{default_host_is_localhost_only, default_backup_dir_is_none, custom_backup_dir}`
- [x] Docs: README flags table + Scheduled maintenance section — flags table gains `--backup-dir` and the inverted `--host` row/usage line; Scheduled maintenance names the `--backup-dir` destination for the weekly backup and links the new restore section

## Scheduler timezone support (2026-06-10)

(Per-entry timezones in `schedule.cron` so market-close-driven jobs are expressed in the market's own timezone instead of Sydney-local approximations — the Sydney↔New York offset swings 14–16h across DST transitions, shifting the 11:30 price-import's margin over the NYSE close by two hours over the year. `chrono-tz` is already a dependency; `croner`'s `find_next_occurrence` is generic over `chrono::TimeZone`.)

- [x] Schedule format: optional IANA timezone field between the cron expression and the job name (e.g. `30 16 * * 1-5 America/New_York price-import`); absent → local time as today; unknown zone names rejected at startup via `ScheduleError::Parse` with the line number — `parse` returns `ScheduleEntry { cron, tz: Option<chrono_tz::Tz>, name }`; 6 fields → no tz, 7 fields → field 6 must parse as an IANA `Tz` (error names the line and suggests the `Australia/Sydney` form), other counts rejected
- [x] `next_run` computes the occurrence in the entry's timezone; the `next run scheduled` INFO line shows the zone (`%Z`) — `next_run` is generic over `chrono::TimeZone`; per-entry tasks run the shared generic `run_entry` loop with a clock yielding `DateTime<Tz>` (or `Local` when no zone), and the existing `%Z` format renders the zone abbreviation (e.g. `EDT`, `AEDT`)
- [x] DST gap/fold behaviour (e.g. a 02:30 job on the spring-forward day) covered by an explicit test — gap: fires at the first valid instant after the skipped hour (03:00 AEDT on Sydney's 2026-10-04 spring-forward); fold: fires once at the first (AEDT) 02:30 on 2026-04-05 and not again in the repeated hour (croner's documented DST semantics, pinned by our tests)
- [x] Cap each sleep (e.g. 1h) and recompute, so a DST transition mid-sleep re-anchors the wall-clock target (pre-existing issue with `Local` too) — `MAX_SLEEP` = 1h; `run_entry` sleeps in capped chunks and recomputes the remaining delay after each, so any wall-clock shift (DST, NTP, suspend) re-anchors within an hour
- [x] `schedule.cron`: move the price-import entries to their market timezones and rewrite the Sydney-clock comment block — three entries now: `30 17 * * 1-5 Australia/Sydney` (ASX), `30 17 * * 1-5 America/New_York` (NYSE), `30 0 * * * UTC` (crypto daily cut-off); each keeps a fixed 90/30-min margin over its market's close year-round, and all land before the 12:00-local report-snapshot
- [x] Tests: parse with/without the tz field; bad zone name rejected; next-occurrence in a non-local zone; DST cases; committed `schedule.cron` still validates — `scheduler::tests::{parse_accepts_timezone_field, parse_ignores_comments_and_blank_lines, parse_rejects_unknown_timezone, parse_rejects_extra_fields, next_occurrence_computed_in_entry_timezone, dst_gap_fires_at_first_valid_instant_after_gap, dst_fold_fires_once_at_first_occurrence, capped_sleep_reanchors_after_wall_clock_shift, next_run_log_shows_timezone, price_imports_are_scheduled_in_market_timezones, embedded_schedule_is_valid}`; the capped-sleep test drives `run_entry` under tokio's paused clock with an injected wall clock that jumps +1h mid-sleep and asserts the job fires on the wall-clock target
- [x] Docs: README Scheduled maintenance section (schedule file format) — documents the optional timezone field, startup rejection of unknown zones, the zone-aware next-run log, DST gap/fold semantics, and the capped-sleep re-anchoring

## Scheduler nits: wrong line number in UnknownJob; no overlap guard (2026-07-12 review, programming)

- `spawn` reports `ScheduleError::UnknownJob { line: idx + 1 }` where `idx` indexes the *parsed
  entries*, not the schedule file — comments and blank lines shift the reported line
  (`src/infra/scheduler.rs:331-338`; `parse` carries the real line number but drops it)
- [x] Carry the source line through `ScheduleEntry` so the error points at the real line; test
      with a schedule containing comments
      (`scheduler::tests::unknown_job_error_reports_file_line_not_entry_index`)
- Nothing prevents the same job running concurrently: `POST /jobs/{name}` executes inline in the
  handler and can overlap the scheduled run (or a second manual trigger) — e.g. two simultaneous
  `backup`s race the same destination second, two `price-import`s double-fetch
- [x] Serialise per-job execution (a per-job async mutex around `run_job`, or reject a trigger
      while the job is running with 409) and test it — done with a per-job `tokio::sync::Mutex`
      inside `RegisteredJob`, held by `run_job` for the whole run, so both the scheduled loop and
      the manual trigger serialise (an overlapping trigger waits, then runs; no API shape change)
      (`scheduler::tests::concurrent_runs_of_same_job_serialise`; API.md Jobs section + the
      CLAUDE.md scheduler rule document the behaviour)

## Backup pipeline hardening: verify, prune, offsite
The weekly backup is a bare `VACUUM INTO` (`infra/db.rs::backup_to`) with no verification that the
produced file is a restorable database, no retention policy (dated files accumulate forever), and —
unless `--backup-dir` is set — the backup lands beside the live DB on the same disk. For
irreplaceable multi-year tax records this is the weakest operational link.
- [x] Verify each fresh backup: open the produced file and run `PRAGMA integrity_check` (and check the migrations table is present/complete); a failed check fails the backup job loudly (job error recorded + ERROR log), and the bad file is not left looking like a good backup — `db::verify_backup` (read-only open; `PRAGMA integrity_check` must return exactly `ok`; the backup's successful `_sqlx_migrations` must equal the live database's) runs inside `backup_to` on every freshly written file; a failure quarantines the file to `<name>.db.bad` (`verify_or_quarantine`), logs at ERROR, and fails the run as `BackupError::Verification`, which `run_job` records for `GET /jobs` (`db::tests::{fresh_backup_is_verified_in_place, verification_quarantines_corrupt_file, verification_rejects_backup_missing_migrations}`)
- [x] Retention/pruning: keep a bounded set (e.g. last N weekly backups plus longer-lived monthly keepers), pruning only files matching the backup filename pattern in the backup destination — never anything else — policy: the newest 8 backups plus the first backup of each calendar month for the 12 most recent months (`db::{KEEP_RECENT, KEEP_MONTHLY}`; `prune_backups` runs after each verified backup, never after a failed one); candidates are regular files exactly matching `<stem>-YYYY-MM-DD-HHMMSS.db` for this database (`backup_timestamp`) — the live db, WAL sidecars, `.bad` quarantines, other stems, malformed timestamps, and directories are never touched (`db::tests::{prune_keeps_recent_and_first_of_month_keepers, prune_drops_monthly_keepers_beyond_the_cap, prune_never_touches_non_matching_files, prune_beside_db_spares_the_live_database, backup_job_prunes_old_backups}`)
- [x] Off-machine copy: an rsync/rclone hook or documented step so a disk failure can't take the DB and all backups together (decide: in-scope job step vs documented external setup; record the decision here either way) — DECIDED 2026-07-13: **documented external setup, not an in-scope job step** — an in-process uploader would embed remote credentials and provider-specific configuration in a local tax tool, and existing sync tools already do the job well; the README's new "Off-machine copies" section documents an rclone/rsync crontab example scheduled after the Sunday backup, the retention interplay (`sync` mirrors the local policy, `copy` retains everything uploaded), and an occasional offsite restore check — pinned by `doc_checks::backup_pipeline_documented`
- [x] Restore drill: a test that restores from a freshly produced backup file and runs a real query (e.g. row counts match the source) — proving the backup artefact actually restores, not just that the file exists — `db::tests::restore_drill_backup_restores_with_matching_row_counts` runs the full `backup()` job path (write + verify + prune), restores the produced file per the README procedure (copy, then a normal startup `init`), and asserts every table's row count matches the source (and the drill's own rows made the round trip); complements the pre-existing `restore_round_trip_recovers_pre_mutation_state`
- [x] Docs: README/API.md updated for any new job behaviour, retention policy, and `POST /jobs/backup` semantics changes — README "Scheduled maintenance" documents verification, quarantine, and the retention policy; the new "Off-machine copies" section records the offsite decision; the restore section names the drill test; API.md's Jobs section documents the backup job's verify/prune behaviour and its 500-with-recorded-reason failure semantics (no new status codes) (`doc_checks::backup_pipeline_documented`)

## FreeBSD packaging, versioned releases, and a configuration file (REQUIREMENTS 2026-07-13)
Push-to-GitHub must produce a versioned FreeBSD 15.1 (amd64) pkg release, and server settings move
to an optional TOML config file so the rc.d service needs no flags.
- [x] Config file: `infra/config.rs` — TOML `ConfigFile` (db, backup_dir, host, port, schedule; unknown keys and bad TOML rejected loudly), auto-loaded from `/usr/local/etc/share-tracker.toml` when present, `--config PATH` override (must exist when given), `Settings::resolve` precedence CLI > file > built-in defaults; `main.rs` runs off `Settings` — `config::tests` (defaults / file-only / CLI-over-file / partial-file precedence, unknown-key and bad-TOML rejection naming the culprit, explicit-path-must-exist, tempfile load round-trip); the CLI flags lost their clap defaults so "not given" is distinguishable from "explicitly the default" (`args::tests::no_flags_parse_to_none`)
- [x] `--version` reports the `Cargo.toml` version (the single source of truth for release numbering) — `args::tests::version_flag_reports_cargo_package_version`
- [x] Package skeleton in `pkg/freebsd/`: UCL manifest (version substituted at build), plist (`@sample` config, rc.d script), `share_tracker` rc script (daemon(8), dedicated non-login user, `--config /usr/local/etc/share-tracker.toml`), sample config that must always parse via `ConfigFile`, and `build-pkg.sh` (build → stage → `pkg create` → install → smoke-test) — smoke test split into its own `smoke-test.sh` (version agreement, rc-script load, server answers `/reports/health` against the installed config); the default `schedule.cron` also ships as an `@sample` file the sample config's `schedule` points at, so the installed service starts out of the box; `config::tests::shipped_sample_config_parses_and_exercises_every_setting`, `doc_checks::freebsd_packaging::{plist_matches_staged_files, rc_script_and_smoke_test_drive_the_installed_config, version_flows_from_cargo_toml_alone}`
- [x] Release workflow `.github/workflows/release.yml`: on push to `main`, when no `v<version>` release exists — build in a FreeBSD 15.1 VM (vmactions), attach the `.pkg`, publish the release tagging the commit that produced it (`gh release create --target "$GITHUB_SHA"`) — `doc_checks::freebsd_packaging::release_workflow_shape`
- [x] Tests: config precedence / unknown-key / missing-file unit tests; the shipped sample config parses; `--version` matches `CARGO_PKG_VERSION`; doc_checks pins for the workflow, plist/rc-script consistency, and README docs — all listed above
- [x] Docs: README — configuration file, FreeBSD installation (pkg install, sysrc enable), releases & versioning — "Configuration file", "Installing on FreeBSD", and "Releases and versioning" sections; `doc_checks::freebsd_packaging::readme_documents_packaging_and_versioning`

## Release notes from the commits between tags (REQUIREMENTS 2026-07-13)
Release notes generated from commit subjects between the previous release tag and the built
commit; GitHub's PR-based `--generate-notes` yields nothing on a direct-to-main repo.
- [x] `scripts/release-notes.sh <version>`: markdown notes — commit subjects `prev-tag..HEAD` newest first with abbreviated SHAs, compare link; no previous tag → "Initial release." + all commits; excludes this release's own tag so a re-run can't diff against itself — `doc_checks::freebsd_packaging::release_notes_script_lists_commits_between_tags` (executable: drives the script in a scratch git repo through all four behaviours)
- [x] Release workflow generates notes at release time (full-history checkout) and passes `--notes-file` instead of `--generate-notes` — release job now checks out with `fetch-depth: 0` and pipes the script into `--notes-file notes.md`; pinned (incl. that `--generate-notes` is gone) in `doc_checks::freebsd_packaging::release_workflow_shape`
- [x] Tests: executable test drives the script in a scratch git repo (between-tags subjects included, pre-tag subjects excluded, compare link; first-release mode); doc_checks pins the workflow wiring — both listed above
- [x] Docs: README "Releases and versioning" describes where the notes come from — pinned in `doc_checks::freebsd_packaging::readme_documents_packaging_and_versioning` module's README section coverage; sentence added to "Releases and versioning"

## Append-only audit trail for financial writes
Every entity is PUT-upsert-in-place and hard DELETE: an accidental edit to a historical Buy silently
changes prior-year cost bases and tax figures, with the weekly backup as the only recourse and no way
to notice it happened. Aligns with the ATO record-keeping guidance already mirrored
(`docs/ato/cgt-keeping-records-shares.md`).
- [x] Trigger-maintained history tables recording the old row + timestamp + operation on UPDATE and DELETE for the financial fact tables (trades, sells, parcel allocations, income, AMMA statements, transfers, corporate actions, …) — enforced in the database per the data-integrity convention, so no write path can bypass it — implemented as one generic append-only `row_history` table (migration `0013_row_history.sql`): the prior row as JSON plus operation and RFC 3339 UTC timestamp, written by an `AFTER UPDATE`/`AFTER DELETE` trigger pair per audited table (17 tables), rather than per-table history tables. The trail itself is append-only (`BEFORE UPDATE`/`BEFORE DELETE` `RAISE(ABORT)` triggers); a rejected transaction rolls its history rows back with it; cascade deletes (a trade's attachments, a rights sale's allocations) are recorded too; `attachments.content` (BLOB — JSON can't hold it) is the one excluded column, identified by filename/byte_size/checksum instead
- [x] Read-only endpoint (and UI view) to inspect an entity row's history — `POST /reports/row_history` (`reports/row_history.rs`; body `{table, row_id}`, entries newest first with the old row's columns flattened behind `history_id`/`operation`/`changed_at`; a non-audited table ⇒ 422 naming the valid list) + the generic params-report "Row History" screen (one `REPORTS` entry in `config.js` — table picker + row id, no bespoke view). `serde_json` gained the `preserve_order` feature so flattened entries keep table column order
- [x] Decide retention (likely: keep forever; it's the audit trail) and whether reference-data tables (exchanges, listings, FX rates) are in scope — record the decision here — **decided 2026-07-14**: retention = keep forever (append-only, database-enforced, deliberately no pruning job). Scope = every user-entered table whose values feed a calculation: the financial fact tables (trades, parcel_allocations, income, interest_income, amma_statements, amit_adjustments, ess_statements, transfers, corporate_actions, inheritances, rights_sales, rights_sale_allocations, investment_expenses, drp_enrolments, attachments) plus `cgt_settings` (the opening capital loss retroactively changes every year's net capital gain) and, of the reference data, **`listings` alone** (its amit/security_type/preference flags retroactively change tax calculations). Out of scope: import-managed re-importable reference data (currencies, mic_registry, rba_fx_rates, closing_prices), tables that only influence values persisted onto trades at write time (exchanges, exchange_holidays), identity-only holding_accounts, and derived state (report_snapshots, job_runs). Recorded in the 0013 migration header and SCHEMA.md
- [x] Tests: an UPDATE and a DELETE each leave a history row with the prior values; history survives the entity's own 422-rejected writes unchanged; migration preserves existing data — `reports::row_history::tests`: prior-value UPDATE/DELETE entries, newest-first ordering, INSERTs record nothing, 422-rollback via PUT /sells (pre-existing history survives byte-identical), append-only enforcement, cascade-delete recording, one UPDATE + DELETE exercised on a real row of **each** of the 17 audited tables, the const ↔ migration CHECK ↔ trigger-pair consistency pin, and a purely-additive line-level pin on 0013 (no ALTER/DROP/UPDATE/DELETE, inserts only into row_history — on top of the global no-DROP/no-REAL migration tests); `web::tests::row_history_ui_present` pins the UI view and its table picker against the Rust const
- [x] Docs: SCHEMA.md (history tables + Relationships), API.md (history endpoint) — plus the Response-codes 422 entry and a README Features line citing the ATO guidance; pinned by `doc_checks::row_history_audit_trail_documented`

## CI supply-chain checks
CI runs fmt/clippy/test but nothing watches dependencies, and the binary talks to the internet
(`reqwest`, `yfinance-rs`, `quick-xml`).
- [x] `cargo audit` (or `cargo deny check advisories`) as a CI step failing on known RustSec advisories; document the local equivalent — `cargo deny check advisories` via `EmbarkStudios/cargo-deny-action@v2` in ci.yml, configured by the committed `deny.toml`; the README "Supply-chain checks" section documents the local equivalent (`cargo install cargo-deny --locked` or `brew install cargo-deny`, then `cargo deny check advisories`). Running the new gate immediately caught real findings: quick-xml 0.37 carried two advisories (RUSTSEC-2026-0194 quadratic duplicate-attribute check + the NsReader unbounded namespace-allocation issue), fixed by upgrading to quick-xml 0.41 — `parse_iso4217` migrated to the ≥0.38 event model where an entity reference is its own `GeneralRef` event splitting the surrounding Text, with a new test (`parse_iso4217_reassembles_names_split_by_entity_references`) covering reassembly of `&amp;` names and the loud failure on an unresolvable reference — and `spin 0.9.8` was yanked (bumped via `cargo update -p spin`), so the check now passes with an **empty** ignore list. CI wiring pinned by `doc_checks::supply_chain_checks_run_in_ci`
- [x] Dependabot (or Renovate) config for Cargo so security patches in the HTTP/TLS stack arrive without manual attention — `.github/dependabot.yml`: weekly version-update PRs for the `cargo` and `github-actions` ecosystems, grouped into one PR per ecosystem to fit a solo project; alert-driven Dependabot security PRs are separate and fire as soon as an advisory lands regardless of the schedule. Pinned by `doc_checks::supply_chain_checks_run_in_ci`
- [x] Decide how advisory failures with no upstream fix are handled (temporary ignore list with expiry + reason) — record the policy — **decided 2026-07-14**: the advisory goes on `deny.toml`'s `[advisories] ignore` list, one entry per line carrying a `reason` and a same-line `# expires: YYYY-MM-DD` comment; the executable test `doc_checks::advisory_ignores_expire` fails the suite once the date passes (verified by trialling an expired entry), so every ignore is re-justified with a new date or removed on a deadline — temporary by construction, never permanent. An unmaintained dependency we decide to keep indefinitely is a replacement task in TODO.md, not an open-ended ignore. Recorded in deny.toml's header and the README "Supply-chain checks" section, both pinned by `doc_checks::supply_chain_checks_run_in_ci`

## Split `trade.rs` non-test code (honourable mention)
`entities/trade.rs` carries ~1,180 lines of non-test code mixing the model, write-time invariants,
and handlers. Not a defect — a maintainability nice-to-have.
- [x] Split into focused units (e.g. model + `db_*`, invariant validation, handlers/router) without changing behaviour or the module's public surface; existing tests keep passing unchanged (they are the behaviour lock) — `trade.rs` is now the parent module (re-exports + the unchanged inline test module, verified byte-identical to the pre-split tests) over five focused submodules in `src/entities/trade/`: `model.rs` (Trade/TradeType/TradeBody, `FromRow`, the GST-inclusive wire presentation), `checks.rs` (the write-time checks shared with the Sell path: GST split, spot-rate, core amounts incl. the pre-CGT cutoff, statement-total cross-check, and their 422 detail texts), `settlement.rs` (T+n business-day settlement derivation), `db.rs` (`db_*` + in-transaction invariants and provenance guards, `UpsertError`/`DeleteOutcome` beside their `From` conversions), `http.rs` (router + handlers). Every `crate::entities::trade::X` path is preserved by re-export, so no caller changed; the full suite (1,076 tests) passes unchanged

## Suffixed one-off backups + update.sh pre-upgrade backup (2026-07-27)
`pkg/freebsd/update.sh` restarted the service (applying any pending migration) with no backup taken
first — the only rollback point was the weekly scheduled backup, up to six days stale. Fixed by
letting a manual backup trigger label itself, and having update.sh use that right before installing.
- [x] `POST /jobs/backup` gains an optional `?suffix=` query param appended to the backup filename as `-<suffix>.db`; validated (`db::validate_backup_suffix`: non-empty, ≤40 chars, ASCII alphanumeric/`.`/`_`/`-` only, not leading with `-`/`.`) before the registry lookup or `run_job`, so an invalid suffix returns `422` and records no job run — `db::tests::invalid_suffix_is_rejected`, `scheduler::tests::trigger_with_invalid_suffix_returns_422_and_records_no_run`
- [x] The `Job` closure type threads a `JobParams { suffix }` through `registry()`/`run_job`/`trigger`; every job but `backup` ignores it; the scheduled loop always passes `JobParams::default()` (a suffix only makes sense for a deliberate manual run) — `scheduler::tests::{trigger_backup_with_suffix_writes_suffixed_file, scheduled_backup_takes_no_suffix}`
- [x] A suffixed backup is a pruning candidate under the same retention policy as any other, never exempt — `backup_timestamp` accepts an optional `-<suffix>` after the fixed-width timestamp (`db::tests::suffixed_backups_are_pruning_candidates`)
- [x] `pkg/freebsd/update.sh`: if the service is running, POSTs `/jobs/backup?suffix=pre-<version>` (reading `host`/`port` from the active config via a small `toml_value` helper, defaulting like `config.rs`'s `DEFAULT_HOST`/`DEFAULT_PORT`) and aborts before `pkg add` on failure, so the database is untouched; if the service isn't running (fresh install) the step is skipped with a warning; `-n`/`--no-backup` skips it deliberately. Needs `curl` (`fetch(1)` can't POST), now a `manifest.ucl` package dependency. `pkg/freebsd/smoke-test.sh` exercises the suffixed trigger against a real installed package
- [x] Tests: `db.rs` (path/validation/pruning) and `scheduler.rs` (HTTP trigger, 422-records-no-run, scheduled-vs-manual param) unit tests above, plus a live end-to-end run against `cargo run` confirming 204/422/204 and the exact filenames produced
- [x] Docs: `docs/API.md` Jobs section documents `?suffix=`, its validation rule, and the 422 catalogue entry; README "Scheduled maintenance" and "Installing on FreeBSD" document the naming pattern change and update.sh's abort-before-install behaviour — `doc_checks::backup_suffix_param_documented`

## Authentication (2026-08-02)
Originally recorded (2026-07-13 improvement review) as an honourable mention: the server has no
authentication of its own, and localhost-only binding was the accepted posture until exposure was
wanted (see the "Operational hardening" section above, which flipped the default to `127.0.0.1`).
Implemented once the reverse-proxy path prefix (`base_path`, 2026-08-02) made exposing the app past
localhost practical.
- [x] Optional `[auth]` config table — single shared username + Argon2id password hash, config-file only (no `--auth-*` CLI flag: a secret on argv is visible to anyone on the host via `ps`, and the rc.d service only ever passes `--config`) — gates the whole application when set, and leaves it exactly as before (open, no `/login` route at all) when absent, so none of the ~1200 pre-existing tests or deployment scripts needed a credential — `infra::auth::Auth`, `infra::config::AuthConfig`, threaded through `Settings::resolve` and `app::router` as `Option<Auth>`; `infra::args::Command::{HashPassword, GenToken}` (a `share-tracker` subcommand, not a flag) print a fresh Argon2id PHC hash (password read from stdin, never argv) and a random 64-hex-char bearer token respectively
- [x] Access control only, not per-user data (the app stays single-taxpayer): a self-contained signed session cookie — no session table, no migration — minted by `POST /login`, checked by `infra::auth::require_auth` (an `axum::middleware::from_fn_with_state` layer applied to the merged router before the reverse-proxy `nest`, so it sees the request with `base_path` already stripped, matching its `"/login"`/`"/static/style.css"` allowlist) gating every other route; also accepts `Authorization: Bearer <api_token>` for the deployment scripts (`update.sh`, `smoke-test.sh`) that call the API without a browser. The signing key derives from the password hash (`HMAC-SHA256(key = password_hash, msg = "share-tracker session v1")`), so changing the password invalidates every existing session at once; a JSON/XHR request with no valid credential gets `401`, a browser navigation (`Accept: text/html`) gets `303` to `/login`. `POST /logout` clears the browser's cookie but — no server-side session store — cannot revoke a copied-out cookie value before its own 30-day expiry; recorded as an accepted limitation in `docs/API.md`, not silently glossed over, alongside the accepted absence of a login-CSRF token (`SameSite=Lax` already covers every state-changing route)
- [x] Frontend: a server-rendered `/login` page (`infra/auth/login.html`, deliberately outside the SPA so it renders before any JS module or credential is presented) posting to `POST /login`/`POST /logout`; the shell's `<meta name="auth">` (substituted the same way as the existing `<meta name="base-path">`) tells `nav.js` whether to render "Log out" (a real form POST, not a fetch or hash route, so it needs no JS wiring beyond building the element) and `util.js`'s `api()` whether a `401` means "redirect to `/login`" rather than an ordinary rejection
- [x] Deployment: `pkg/freebsd/share-tracker.toml.sample` documents `[auth]` commented out (parses via `ConfigFile`, exercised by the existing sample-config test) and the `host` comment now says "unless `[auth]` is configured"; `pkg/freebsd/update.sh` reads `[auth].api_token` from the active config (its existing `toml_value` line-matcher, since the key is unique to `[auth]`) and sends it as an `Authorization: Bearer` header on its pre-upgrade backup POST when present, with a pointed error if `[auth]` is configured but no token is
- [x] Tests: `infra::auth` unit tests (password accept/reject, cookie mint→verify round-trip, tampered/forged/expired/garbage cookie rejection, password-change invalidates sessions, bearer accept/reject, logout-doesn't-revoke) and API-level tests (401 vs 303 branching by `Accept`, `/login`+`/static/style.css` reachable unauthenticated, wrong password sets no cookie, a valid cookie unlocks GET/PUT/DELETE, logout clears the cookie client-side, bearer token unlocks a route and a wrong one doesn't); `app.rs` tests (`auth: None` leaves every route open with no `/login` at all — pinned both directly and via the pre-existing base-path test reusing `ApiClient::full`, the layer gates one route from each merged router, base-path + auth combine correctly with the redirect `Location` and cookie `Path` both scoped to the prefix); `config.rs`/`args.rs` parsing tests (the `[auth]` table resolves, an unparseable hash fails naming the field, the subcommands parse); `web.rs` Rust tests plus `util.test.js`/Node tests for the auth meta tag and the logout UI; `test_support::ApiClient` gained `with_header` (a persistent per-client header, the single choke point being `send`) for the cookie/bearer-token test traffic
- [x] Docs: README Features bullet + a new "Authentication" section (the `[auth]` config example, the two CLI helper subcommands, the revocation/CSRF caveats) + the two rewritten "no authentication" notes (`--host` table note, "Behind a reverse proxy" opener); `docs/API.md` gained an "Authentication" section (endpoints, cookie/token mechanics), a preamble line beside "Base path.", `401`/`303` rows in Response codes, and a Known limitations entry for the two accepted gaps — `doc_checks::authentication_documented`


## SCENARIOS T-06: three jobs record their failure as a Rust `Debug` string, losing both the message and the cause

`registry()` wraps `rba-fx-import`, `mic-import` and `currency-import` with `format!("{e:?}")`. Driven
on 2026-08-22 with the three feeds made unreachable (a dead HTTP proxy), this is verbatim what lands
in `job_runs.error`, the Jobs table's Error column and the health banner:

```
Fetch("error sending request for url (https://www.rba.gov.au/statistics/tables/csv/f11-data.csv)")
```

Two things are lost. The variant's own `#[error("could not fetch the RBA FX rate feed: {0}")]` — which
CLAUDE.md makes *the* log wording for every error enum in the tree — is discarded in favour of the
derived `Debug`; and the underlying cause is discarded too, because `fetch_rates` maps the
`reqwest::Error` with `e.to_string()`, whose top-level `Display` says only "error sending request for
url" and never the `tcp connect error: Connection refused` in its `source()` chain. So the operator
is told neither what failed nor why, in Rust syntax. The `backup` job, which uses `e.to_string()`, gets
this right.

Both halves are small and independent; the fix is `{e}` in the three registry entries plus walking the
`source()` chain where a `reqwest::Error` is stringified (all four import paths do the same thing).

**Decision (Evan, 2026-08-22): fix both halves.** Rejected: correcting the `Debug` string but
leaving the cause chain, which would name the feed and still not why it failed.

- [x] `{e}` instead of `{e:?}` in the `rba-fx-import`, `mic-import` and `currency-import` registry
      entries, so the variant's `#[error]` wording is what is recorded (the `backup` job's
      `e.to_string()` is the model) — all three now `.map_err(|e| e.to_string())`, and
      `scheduler::tests::no_registered_job_records_its_failure_as_a_debug_string` scans
      `registry.rs` to keep the `Debug` form out (the way `infra::decimal` pins stringified
      decimal binds out of the writes)
- [x] Walk the `source()` chain where a `reqwest::Error` is stringified — all four import fetch paths
      do the same thing — so `tcp connect error: Connection refused` reaches the recorded error —
      `infra::fetch::cause_chain`, a new module whose doc comment carries the reason (a
      `reqwest::Error`'s own `Display` is only its outermost layer); called from all nine
      `ImportError::Fetch` sites across the three feeds (RBA F11, ISO MIC, ISO 4217/24165).
      There are three such paths, not four — see the consequences note
- [x] Regression tests: an unreachable feed's recorded `job_runs.error` carries the enum's own
      message and the underlying cause, and no longer matches Rust `Debug` syntax —
      `scheduler::tests::a_failed_job_records_its_message_and_its_cause` (end to end through
      `run_job` → `job_runs` → `GET /jobs`), the three
      `an_unreachable_feed_reports_the_feed_and_the_reason` tests
      (`rba_fx_rate`/`mic_registry`/`currencies`), and `infra::fetch`'s own unit tests
      (chain rendering, no-source, wrapper de-duplication, depth bound, and the real
      `reqwest` error whose `Display` hides the cause). Every one drives a refused **loopback**
      connection via `test_support::unreachable_url` — a free port bound and dropped — so no test
      reaches the network
- [x] Docs: `docs/API.md`'s `GET /jobs` paragraph now states what the recorded error text is, with
      the real fetch-failure string as the example

Consequences found while implementing, all settled in the same commit:

- The finding says "all four import fetch paths"; there are **three**. Grepping `reqwest` across
  `src/` finds exactly `rba_fx_rate::fetch_rates`, `mic_registry::fetch_registry` and
  `currencies::fetch`. The fourth candidate, the price import, never sees a `reqwest::Error`: it
  stringifies a `yfinance_rs::YfError`, and that crate wraps the transport failure in its own
  `RedactedHttpError`, which stores only the *rendered* message (auth query params redacted) and
  implements no `source()`. The cause is discarded inside the dependency before this code can see
  it, so walking the chain there would recover nothing — `closing_price.rs` is deliberately left
  alone. (`e.to_string()` on a `YfError` is already its `Display`, not `Debug`, so it never had the
  first half of the bug either.)
- `cause_chain` skips a layer whose own message merely re-renders what it wraps: several wrapper
  error types (yfinance-rs's among them) delegate `Display` straight to their source, and appending
  it would double the text. Pinned by `a_wrapper_that_only_re_renders_its_source_is_not_repeated`.
- The 502 bodies are unchanged in meaning: `ImportError::Fetch`'s message is the `source` of
  `ApiError::BadGateway`, which is logged rather than returned — so the richer text improves the
  server log, and the short user-facing body ("could not fetch the RBA FX rate feed from its
  source") stays as it was.

---

## SCENARIOS T-09/schedule: the startup "no schedule entry" warning cries wolf on the two deliberately-manual jobs

`schedule::spawn` warns for every registered job with no schedule line, on the stated grounds that
this is "usually an oversight". Two of the eight registered jobs — `price-rebase` and
`settlement-recompute` — are *deliberately* unscheduled one-off repairs, documented as such in the
README and `docs/API.md`, so every single startup logs two WARN lines that can never be cleared:

```
WARN registered job has no schedule entry; it will only run via POST /jobs/price-rebase
WARN registered job has no schedule entry; it will only run via POST /jobs/settlement-recompute
```

That is the permanent-alarm-nobody-can-clear pattern the project has fixed elsewhere (`unpriced_from`
so health stops reporting expected price holes, the duplicate-income key so the legitimate
ordinary+special pair stays silent). Its cost here is precise: a genuinely dropped schedule line —
the second cause in the overdue-job finding above — logs exactly the same line as the two expected
ones, so the signal is already buried at the moment it matters.

**Question for Evan — how to separate the two?**

- **(a) Mark manual-only jobs in the registry.** `register_manual(&mut jobs, "price-rebase", …)`
  beside `register`, and `spawn` warns only for a job that expects a schedule. The registry then
  states the intent in the one place that knows it, and `GET /jobs` could carry the flag too.
- **(b) Leave it** — the README names both, and two known lines per boot is cheap.

**Decision (Evan, 2026-08-22): (a), mark manual-only jobs in the registry.** Rejected: leaving the
two expected WARN lines in place.

- [x] A `register_manual` beside `register` in `registry.rs` marking a job as deliberately
      schedule-less; `schedule::spawn` warns only for a job that expects a schedule, so the warning
      fires exactly when a schedule line has actually been lost
- [x] `GET /jobs` carries the flag, so the Jobs screen can say "manual only" rather than leaving a
      never-scheduled job looking overdue (this pairs with the overdue-jobs finding above — a
      manual-only job must never be reported overdue)
- [x] README "Scheduled maintenance" and `docs/API.md` Jobs section reflect where the intent is now
      recorded
- [x] Regression tests: a manual-only job produces no startup warning and is never overdue; a
      scheduled job whose line is missing still warns

Done 2026-08-22 (SCENARIOS T-09/schedule). `RegisteredJob` now carries a `JobTrigger`
(`Scheduled` | `ManualOnly`) — an enum, not a bool, since it is a fixed set of values and it is what
`GET /jobs` serialises (`"trigger": "scheduled" | "manual_only"`). `register_manual` sits beside
`register` in `infra/scheduler/registry.rs`, both delegating to the same
`RegisteredJob::from_fn(trigger, work)`, so a job is still added with one call and the `Arc`/`Box::pin`
wrapping is still in one place; `price-rebase` and `settlement-recompute` are the two that use it.
`schedule::spawn` skips `ManualOnly` jobs when checking for missing schedule lines, so the shipped
schedule now starts with **zero** "no schedule entry" WARN lines and that WARN means one thing only:
a line has been lost. Verified against the real binary — `--schedule` with the `backup` line removed
logs exactly `WARN … registered job has no schedule entry; it will only run via POST /jobs/backup
job=backup`, and the committed schedule logs no WARN at all. The Jobs screen gained a **Trigger**
column (`scheduled` / `manual only`) from the new field, and its view description says a manual-only
job showing `never` is expected rather than a missed run.

Tests: `scheduler::tests::committed_schedule_starts_without_a_single_missing_entry_warning` (the
shipped schedule warns about nothing), `spawn_warns_about_job_with_no_schedule_entry` (extended: the
lost line still warns, and neither manual-only job is named in any warning),
`every_registered_job_is_scheduled_or_deliberately_manual` (both directions against the committed
`schedule.cron` — a `Scheduled` job has a line, a `ManualOnly` job has none, and the manual-only set
is exactly the two), `list_jobs_reports_how_each_job_is_triggered`,
`manual_only_flag_is_serialised_as_snake_case` (the wire value the UI matches on),
`web::tests::jobs_ui_present` (the column and its label), and
`doc_checks::manual_only_jobs_documented`.

Consequences found while implementing:

- **"Never overdue" cannot be tested yet, and deliberately is not.** There is no overdue concept in
  the tree — it arrives with the still-open `## SCENARIOS T-11/T-02/T-12` section (the `job_schedule`
  table, health's `overdue_jobs`, the Jobs "next run" column). What this section ships is the field
  that work must read: a `manual_only` job has no schedule by construction, so it must get no
  `job_schedule` row and must never appear in `overdue_jobs`. That constraint is now written into
  T-11's checklist rather than asserted here against machinery that does not exist.
- The invariant test is the real regression guard, not the log assertion. A future job added with
  `register` but no schedule line — or with `register_manual` and a line — fails
  `every_registered_job_is_scheduled_or_deliberately_manual`, so the flag cannot drift from the
  schedule file the way a README sentence could. That also made a "manual-only job has a schedule
  line" startup warning unnecessary: the contradiction is caught in CI, not at boot.
- `RegisteredJob::from_fn` gained the trigger as a required argument rather than defaulting to
  `Scheduled`. A default would have let a new job be silently scheduled-expecting; three test call
  sites name it explicitly instead.
- `doc_checks::contemporaneous_price_basis_documented` pinned the old README sentence
  ("`price-rebase` is deliberately one of those"), which this rewrote — the assertion moved with the
  wording, with a comment saying why.

---

## SCENARIOS T-10: `POST /jobs/:name` answers bare status codes with no body

Driven on 2026-08-22:

- `POST /jobs/nope` → **404 with an empty body**. The Jobs screen's `api()` helper turns that into the
  toast `HTTP 404`. CLAUDE.md's own rule for the delete routes — "never a bare `StatusCode::NOT_FOUND`,
  which the web UI can only show as 'HTTP 404'" — is the same rule, and `deleted(found, noun)` exists
  to satisfy it.
- A job that fails → **500 with an empty body**, toasted as `HTTP 500`, even though `run_job` has just
  returned the reason as a `String`. It is recoverable — `viewJobs()` reloads and the row's Error
  column then shows it — but the toast the user reads first says nothing.

The suffix validation, by contrast, is exemplary: `?suffix=../../etc/x`, a leading `-`, an empty value
and a 41-character value each answer **422 with a plain-text reason**, and are rejected *before* the
registry lookup so a malformed request never records a run.

One inconsistency found alongside: an unknown query parameter is silently ignored (`?sufix=pre-0.5.1`
answers 204 and takes an **unlabelled** backup), because `JobParams` derives `Deserialize` without
`deny_unknown_fields`.

**Question for Evan — how far to take it?**

- **(a) Both bodies, and reject unknown query params.** 404 names the job and lists the registered
  names; 500 carries `run_job`'s error text; `JobParams` gets `#[serde(deny_unknown_fields)]` so a
  typo'd `suffix` is a 422 rather than a silently unlabelled backup.
- **(b) Both bodies only** — leave the typo'd parameter silently ignored.
- **(c) The 404 only** — a failed run's reason is already one reload away in the table.

**Decision (Evan, 2026-08-22): (a), all three.** Rejected: the two bodies alone, and the 404 alone.

- [x] `POST /jobs/:name` for an unknown name answers 404 with a plain-text body naming the job and
      the registered names (the `deleted(found, noun)` convention, one endpoint over)
- [x] A failed run answers 500 carrying `run_job`'s error text, so the toast the user reads first
      says what went wrong
- [x] `JobParams` gets `#[serde(deny_unknown_fields)]`, so `?sufix=pre-0.5.1` is a 422 rather than a
      204 taking a silently unlabelled backup
- [x] `docs/API.md` Jobs section + the Response codes table
- [x] Regression tests: the 404 body names the job; a failing job's 500 carries its reason; a
      misspelt query parameter is refused rather than ignored

Done 2026-08-22. `trigger` (`src/infra/scheduler/http.rs`) now answers every failure with a
plain-text body:

- `POST /jobs/nope` → `404` `no job named 'nope'; registered jobs are backup, currency-import,
  mic-import, price-import, price-rebase, rba-fx-import, report-snapshot, settlement-recompute`
  (the registry's own keys, sorted — `GET /jobs` is the discovery surface, but a toast that lists
  them costs nothing).
- A failed run → `500` carrying `run_job`'s error text verbatim — the same string `job_runs.error`
  records and the Jobs table's Error column shows. Driven live against a server with
  `--backup-dir /nope/nowhere`: `backup failed: Read-only file system (os error 30)`.
- `POST /jobs/backup?sufix=pre-0.5.1` → `422` ``cannot read the query string: sufix: unknown field
  `sufix`, expected `suffix` ``, rejected before the registry lookup, so no run is recorded and no
  unlabelled backup is taken.

Tests: `scheduler::tests::trigger_unknown_job_404_names_the_job_and_the_registered_names`,
`a_failing_job_answers_500_carrying_its_reason` (body, `job_runs.error` agreement, and the WARN
still logged), `trigger_with_a_misspelt_query_parameter_is_refused_not_ignored` (no backup file, no
recorded run), and `doc_checks::job_trigger_failure_bodies_documented`.

Consequences found while implementing:

- **The 500-with-a-body is a new `ApiError` variant, not a hand-rolled response.** `ApiError::Internal`
  responds 500 with an *empty* body by contract (internal detail must not leak), so carrying the
  reason needed its own variant rather than a departure inside one handler:
  `ApiError::JobFailed { job, reason }` logs the old `manual job trigger failed` WARN with its job
  field when the response is built and puts `reason` in the body. Handlers still return
  `Result<_, ApiError>`; the endpoint is documented as the one `500` in the API with a body, and why
  (a job's failure is the operator's own diagnostic, already on display one screen away).
- **`deny_unknown_fields` alone would have answered `400` in axum's wording**, not the `422` with a
  reason every other rejected write uses — the `Query` extractor answers its own rejection. The
  handler takes `Result<Query<JobParams>, QueryRejection>` and converts it, using the rejection's
  *source* (serde's ``unknown field `sufix`, expected `suffix` ``) rather than `body_text()`, whose
  "Failed to deserialize query string:" prefix is framework jargon in a one-line toast.
- No UI change was needed: `util.js`'s `api()` already appends a non-empty error body to the thrown
  message, so `viewJobs()`'s `toast(e.message, true)` now reads `HTTP 404: no job named 'nope'; …`.
  Confirmed against a live server (all three responses driven with curl), not only through
  `ApiClient`.
- The only caller passing a query parameter is `pkg/freebsd/update.sh`
  (`POST /jobs/backup?suffix=pre-<version>`, mirrored by `smoke-test.sh`); both spell `suffix`
  correctly, so `deny_unknown_fields` breaks nothing.
