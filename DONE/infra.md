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

---

## SCENARIOS T-11: a run interrupted by a restart leaves no record, and an unverified file that looks like a good backup

`run_job` records the run **after** the work returns, and `main`'s graceful shutdown waits only for
in-flight *HTTP requests* — a scheduled job runs in a spawned task the process does not wait for.
Reproduced on 2026-08-22 against a 265 MB throwaway database, with a schedule entry firing at a known
second and a `SIGTERM` 0.6 s into it:

```
16:51:00.301  INFO job started job=backup
16:51:00.302  INFO starting backup path=".../big/t-2026-08-22-165100.db"
16:51:00.649  INFO shutting down
```

and on disk afterwards, `t-2026-08-22-165100.db` — 265,175,040 bytes against the live database's
265,199,616. What that leaves behind:

- **No `job_runs` row at all.** `GET /jobs` still shows the *previous* run's result; there is no
  "started but never finished" record, so nothing distinguishes an interrupted run from one that
  never began.
- **A file that was never verified.** `verify_or_quarantine` runs after `VACUUM INTO` returns, in the
  process that just died. The file matches the backup naming pattern exactly, so it is counted by
  retention pruning, can become a first-of-month keeper, and is a restore candidate indistinguishable
  from a verified one. (This particular truncation happened to open and pass `integrity_check` —
  which is the point: whether an interrupted copy is restorable is luck, and nothing checks.)
- Nothing ever re-verifies an existing backup, so the only verification a file gets is the one it
  missed.

The plain operational trigger is `service share-tracker restart` (or a host reboot, or a `pkg`
upgrade) landing on Sunday 00:00 — the weekly backup's own slot.

Related, same directory: a quarantined `<name>.db.bad` is deliberately never pruned ("kept for
diagnosis"), so a failing disk — the likely cause of a verification failure — leaves a full-size copy
per weekly run until the volume fills. Worth settling with whatever this finding takes.

**Question for Evan — which half to fix, and how?**

- **(a) Both: record the start, and never leave an unverified file looking good.** `run_job` inserts
  the `job_runs` row when the job *starts* (finished_at/success NULL) and updates it on completion,
  so an interrupted run is visible as one that started and never finished; and the backup writes to a
  temporary name (`<name>.db.partial`) that is renamed into place only after verification passes, so
  an interrupted copy can never be mistaken for a backup. Startup could additionally sweep leftover
  `.partial` files.
- **(b) The file only.** Write-then-rename as above; leave run recording as it is.
- **(c) The record only.** Start/finish rows as above; leave the file naming as it is (an
  unverified leftover keeps a backup's name).
- **(d) Wait for the job instead.** Hold shutdown until in-flight jobs finish, bounded by a timeout.
  Fixes the common case but not a `SIGKILL`, a power cut, or a timeout expiring.

**Decision (Evan, 2026-08-22): (a), both halves.** Rejected: fixing only the file, only the record,
and waiting for the job on shutdown (it cannot cover `SIGKILL`, a power cut, or the timeout expiring).
The quarantined-file question was settled alongside: **bound the `.bad` files** to the newest few
rather than leaving them unbounded or only documenting them.

- [x] `run_job` inserts the `job_runs` row when the job **starts** (`finished_at`/`success` NULL) and
      updates that row on completion — an interrupted run is then visible as one that started and
      never finished. Needs a migration relaxing `job_runs.finished_at`/`success` to nullable, and
      `JobStatus`/`JobRunRecord` to carry the in-flight state honestly
- [x] `GET /jobs` and the Jobs screen show an unfinished run as such (not as a success, not as a
      failure), and the history pruning still bounds the table
- [x] The backup writes to a staging name (`<name>.db.partial`) and renames it into place **only
      after verification passes**, so an interrupted copy can never carry a backup's name, be counted
      by pruning, or be picked for a restore
- [x] Startup sweeps leftover `.partial` files for this database (an interrupted run's debris), and
      pruning bounds quarantined `<name>.db.bad` files to the newest few
- [x] `docs/API.md` (`GET /jobs` shape and the in-flight state, the backup job's paragraph),
      `docs/SCHEMA.md` (`job_runs` columns), README "Scheduled maintenance" (staging + `.bad` bound)
- [x] Regression tests: a run recorded at start is visible before it finishes; a verification failure
      leaves no file under the backup name (the `.partial` is quarantined instead); a leftover
      `.partial` is swept at startup and never counted by pruning; `.bad` files are bounded


Tests: `scheduler::tests::a_run_is_visible_from_the_moment_it_starts` (a job parked mid-run shows on
`GET /jobs` as `running`, with the previous run untouched beneath it, and finishing updates that row
rather than appending a second), `an_interrupted_run_reads_as_started_and_never_finished` (and the
health report does not call it a failure), `starting_a_run_prunes_the_history_and_never_the_in_flight_row`,
`migration_0042_carries_every_run_forward_as_ok_or_failed` (ids, timestamps and error text intact,
plus the two CHECKs); `db::tests::a_backup_is_written_under_a_staging_name_and_renamed_only_once_verified`,
`an_unverified_copy_never_carries_a_backup_name` (does not parse as a backup, never pruned, never
takes a real first-of-month keeper's slot, swept at startup),
`the_startup_sweep_only_removes_this_database_s_staging_files`,
`quarantined_files_are_bounded_to_the_newest_few`, `verification_quarantines_corrupt_file` (extended:
the staging file is moved aside and the backup name never appears), `prune_never_touches_non_matching_files`
(extended with a `.partial` bystander); `web::tests::jobs_ui_present` and
`doc_checks::interrupted_runs_and_staged_backups_documented`.

Consequences found while implementing:

- **The three-state run needed an enum, not a nullable boolean.** The finding's own wording says
  "`finished_at`/`success` NULL", but a nullable `success` cannot say *running* without being read by
  inference — `NULL` would have meant "unfinished" in `job_runs` while `last_success: null` already
  means "never run" in `GET /jobs`, two different nothings in the same field. Migration **0042**
  therefore replaces the boolean with a CHECK-constrained
  `status TEXT ('running' | 'ok' | 'failed')` — the codebase's rule for a limited set of values — and
  relaxes `finished_at` to nullable, with a second CHECK holding the two in step (`'running'` exactly
  while there is no finish time). `GET /jobs`' `last_success` became `last_status` carrying the same
  three values, and the Jobs screen shows the run's state as it stands rather than folding it into
  ok/failed. SQLite can relax neither a NOT NULL nor a table-level CHECK in place, so it is the
  rename-and-rebuild pattern: every existing row is carried forward id-for-id with `success = 1` →
  `'ok'` and `0` → `'failed'`, and 0012's index has to be dropped **before** the rename or the new
  table cannot claim its name (index names are global, and a renamed table keeps its own).
- **`job_runs` is neither audited nor snapshot-triggered**, so the rebuild re-created no triggers —
  checked rather than assumed. It is out of scope for `row_history` (derived state, scope decision
  2026-07-14) and already classified *exempt* in `reports::snapshot`, so
  `every_table_is_classified_for_snapshot_staleness` stayed green with no edit.
- **Splitting the write in two had to keep the prune bounded and non-destructive.** The insert still
  prunes to `JOB_RUN_HISTORY_LIMIT` in its own transaction — after the insert, never before, or the
  bound would be the limit *plus* the fresh row — and the prune can never take the row the in-flight
  run is about to update, because that row is the newest of its job and the per-job lock keeps any
  other run of the same job from inserting meanwhile. `db_record_run` survives as the fallback for a
  run whose opening row never landed: the run is over by then, so the whole of it is recorded in one
  write rather than lost.
- **A test with nothing to do with this broke, and was right to.** `run_job` now awaits a database
  write *before* calling the job, and `capped_sleep_reanchors_after_wall_clock_shift` runs under
  `tokio::time::pause()`, where an idle runtime is licence to jump to the next timer — sqlx's own
  600 s pool-maintenance tick. The fake wall clock advanced ten minutes between the timer firing and
  the job body reading it. Fixed in the test (the pool is closed before the loop starts, so the write
  fails without awaiting anything), not by moving the recording: what that test pins is *when the
  timer fires*, not what it records.
- **The `.partial` suffix is invisible to the pruner for free — but only because the parser demands a
  trailing `.db`.** Reasoned through rather than assumed: `backup_timestamp` strips `.db` from the
  end, so `<stem>-YYYY-MM-DD-HHMMSS.db.partial` fails to parse and can never be a pruning candidate,
  a monthly keeper, or a restore option. The startup sweep and the `.bad` bound therefore match by
  the artefact's *own* suffix over an otherwise well-formed backup name
  (`backup_artefact_timestamp`), which keeps both as narrow as pruning is.
- **The quarantine name is the backup's, not the staging file's.** Verification now runs on
  `<name>.db.partial`, but a failure still quarantines it as `<name>.db.bad` — naming the artefact
  after the staging path would have read as a different thing to an operator and broken the
  documented name. `KEEP_BAD = 3` bounds them, which contradicts the README's old promise that
  quarantined files are "never touched"; the doc-check asserts that sentence is *gone*, rather than
  leaving it sitting beside the new bound.
- **Re-driven end to end on a 247 MB throwaway database**, the way the finding was found: a
  once-a-minute `backup` entry and a `SIGTERM` 0.15 s into the run — 0.6 s was too late, the copy
  finishing in ~0.37 s on this disk. What it leaves now is `t-2026-08-22-183600.db.partial` and a
  `job_runs` row `status = running` with no `finished_at`; nothing carries a backup's name. On
  restart the server logged `removed an unfinished backup left by an interrupted run` and swept it,
  and `GET /jobs` — and the Jobs screen, checked in headless Chrome — showed the interrupted run as
  **running** above the previous run's `ok`, rather than as that previous run's result.
- **Not in scope, and worth knowing for the section queued next:** an interrupted run and a run
  genuinely in flight now look identical (`running`, no finish time). Telling them apart needs a
  notion of *overdue*, which is what `SCENARIOS T-11/T-02/T-12` is about; the row this leaves behind
  is the fact that section can build on. The health report deliberately does not treat an unfinished
  run as a failure — nothing failed — and a test pins that.

## SCENARIOS T-11/T-02/T-12: nothing notices a job that has stopped running

`reports::health`'s `failed_jobs` fires only when a job's **most recently recorded run failed**. A
job that is not running at all records nothing, so it raises nothing — the last successful run stays
in place and the Jobs screen keeps showing `ok`, indefinitely. Driven on 2026-08-22:

- A schedule line with no future occurrence (`0 0 30 2 *   backup` — 30 February) is **accepted at
  startup**. `run_entry` logs one `ERROR cannot compute next run, stopping` and the task exits. The
  backup will never run again for the life of the process, and `GET /jobs` still answers
  `backup: last_success = true`.
- Same outcome, no ERROR line at all, for the ordinary operational cases: the server was down at
  00:00 every Sunday, or a hand-edited `--schedule` file lost its `backup` line (that one logs a
  single startup `WARN`; it was indistinguishable from the two deliberate ones until T-09/schedule
  marked the manual-only jobs in the registry — closed in [`DONE/infra.md`](DONE/infra.md) — so it
  now fires only for a line that has actually been lost).

Prices and FX each have a *database-derived* freshness signal that catches their job going quiet —
`prices_stale` (latest ok `closing_prices` date more than 3 business days old) and `fx_stale`
(latest `rba_fx_rates` month older than last month). **The backup has none**, and it is the job where
this matters most: nothing in the database changes when a backup does or does not happen, so a backup
that silently stopped a year ago is indistinguishable from one that ran on Sunday. `mic-import` and
`currency-import` have none either.

The Jobs screen compounds it: it shows each job's **last** run and its history, but never the
schedule or the next run — so the one surface an operator would check cannot answer "is this job
still scheduled, and when is it due?". The scheduler already computes that instant every iteration
and logs it (`next run scheduled`), but nothing persists it.

**Live database: no false alarm.** `job_runs` in the 2026-08-16 backup shows every job last
succeeding exactly when its schedule says it should (backup Sunday 00:01 local, rba-fx-import Monday
02:00, mic/currency-import on the 1st, price-import and report-snapshot daily), so an overdue check
would start quiet. Note the thresholds must be **per job** — weekly, monthly and daily jobs sit side
by side.

**Question for Evan — how should an overdue job be detected?**

- **(a) Persist the next scheduled run.** `run_entry` writes the instant it already computes to a
  new `job_schedule` table (job name, cron expression, timezone, `next_run_at`) on every iteration;
  health gains `overdue_jobs` (now past `next_run_at` plus a grace margin) and the Jobs screen gains
  a "next run" column. Catches all three causes — the dead task, the dropped schedule line, and the
  server that was down — because a stopped task stops moving the stored instant. Most work; also the
  only option that answers "when is this due?" in the UI.
- **(b) A per-job maximum age.** Health alerts when a job's last *successful* run is older than a
  constant declared beside the job in `registry()` (e.g. backup 10 days, rba-fx-import 10 days,
  mic/currency-import 40 days). No new table; duplicates the schedule's knowledge in a second place,
  and stays silent if the schedule is edited to something slower.
- **(c) Backup only.** A `backup_stale` flag on the health report, mirroring `prices_stale`, derived
  from `job_runs`. Smallest fix, covers the job with no other signal, leaves the rest as they are.
- **(d) Documentation only** — a Known limitation saying job liveness is the operator's business.

**Decision (Evan, 2026-08-22): (a), persist the next scheduled run.** Rejected: a per-job maximum
age, a `backup_stale` flag alone, and documentation only.

- [x] A `job_schedule` table (migration): job name, cron expression, timezone, `next_run_at`,
      `updated_at`. Written by `run_entry` every iteration, from the instant it already computes for
      its `next run scheduled` log line — so a task that has stopped stops moving its row
- [x] `reports::health` gains `overdue_jobs`: jobs whose `next_run_at` is now in the past by more
      than a grace margin. Per-job by construction (the stored instant carries the job's own cadence),
      so weekly, monthly and daily jobs need no separate thresholds
- [x] The health banner surfaces it, linking to Jobs, with wording that names the job and how long it
      is overdue
- [x] The Jobs screen gains a **next run** column from `GET /jobs`, so the one surface an operator
      checks can answer "is this still scheduled, and when is it due?"
- [x] A **manual-only** job (`GET /jobs`'s `trigger` — SCENARIOS T-09/schedule) is never reported
      overdue and gets no `job_schedule` row: it has no schedule by design, so an overdue check must
      leave it alone rather than treating "never ran" as "late"
- [x] Classify `job_schedule` for snapshot staleness (exempt, with the reason in the migration) —
      `every_table_is_classified_for_snapshot_staleness` fails otherwise
- [x] `docs/SCHEMA.md` (new table + Relationships), `docs/API.md` (`GET /jobs` shape, health report's
      new field, Response codes if any), README "Scheduled maintenance"
- [x] Regression tests: a schedule with no future occurrence leaves a stored instant that goes stale
      and health reports the job overdue; a job that ran on time is not overdue; the grace margin's
      boundary

Tests: `scheduler::tests::a_spawned_entry_stores_when_it_is_next_due` (one row per schedule entry,
the cron expression as written and the entry's IANA zone beside a future instant; `GET /jobs` carries
it, and a manual-only job — and a scheduled job with no line — carry `null`),
`a_schedule_with_no_future_occurrence_is_reported_overdue` (the finding itself, end to end: spawn
`0 0 30 2 *`, the task logs its ERROR and exits, health is silent within the margin and names the job
past it — while `failed_jobs` stays empty, which is the whole point),
`spawn_forgets_a_schedule_entry_that_has_been_removed` (a row left by a previous process for a
deleted line is cleared at startup, not reported overdue for ever);
`reports::health::tests::a_database_with_no_stored_schedule_reports_nothing_overdue` (the state every
existing database is in the moment it is upgraded), `a_job_whose_schedule_is_still_moving_is_not_overdue`,
`the_overdue_grace_margin_is_exclusive_at_its_boundary` (exactly the margin is on time, one second
later is not), `an_overdue_job_names_its_schedule_and_how_late_it_is` (and only the dead one of a
job's three entries), `a_run_open_longer_than_any_run_takes_is_reported_stalled`,
`an_abandoned_run_stops_being_reported_once_the_job_runs_again`;
`web::tests::jobs_ui_present` and `health_banner_ui_present` (extended);
`doc_checks::overdue_jobs_and_the_stored_schedule_documented`;
`reports::snapshot::tests::every_table_is_classified_for_snapshot_staleness` (extended by one exempt
table, with the reason migration 0043 gives).

Consequences found while implementing:

- **The headline case would have stored nothing at all.** A cron pattern with no future occurrence
  never reaches the loop body — `next_run` fails on the first computation, the task logs its ERROR and
  returns — so a row written only from the computed instant would have left `0 0 30 2 *` with no row,
  and the check that exists for that scenario silent on it. `run_entry` therefore claims its row
  **before** computing anything, at the instant the task starts; for a healthy entry that value lives
  for microseconds before the real instant overwrites it, and for the impossible one it is the frozen
  instant the scheduler gave up at. Driven end to end: `backup` sat at its startup instant while the
  two live entries held 2026-08-23/24.
- **One row per schedule *entry*, not per job.** `price-import` has three lines (Sydney, New York,
  UTC). Keyed by job name, all three would have written to one row, whichever wrote last winning, and
  one of the three dying would have been invisible behind the other two refreshing it. Per entry, the
  dead line is named with its own cron expression and zone — which is also what makes those two
  columns load-bearing rather than decorative, the data-model rule that every field is used by a
  calculation or endpoint. `GET /jobs` folds a job's entries to the earliest, since the column asks
  "when is it next due".
- **The table is the schedule the *running process* is executing.** Ordering forced the design: a
  removed schedule line leaves a row from the previous process, and reporting it overdue for ever is
  the permanent-alarm pattern this project has undone three times (`unpriced_from`, the duplicate
  income key, T-09/schedule in this same pass). Clearing at startup and letting the entry tasks
  rebuild answers it without a reconciliation query — and the clear has to *complete* before any task
  claims a row, which is why `scheduler::spawn` is now `async` (its six test call sites and `main`
  gained an `.await`). A surrogate id then falls out for free: the alternative, keying on
  (name, cron, timezone), needs `timezone` NOT NULL, because SQLite treats NULLs as distinct in a
  unique index and every restart would have added another row for every zone-less entry.
- **The decision's own claim about "the server that was down" does not hold, and is documented
  rather than repeated.** Persisting the next run catches a scheduler that has stopped *while the
  process is up*; a server that was down at 00:00 on Sunday and started on Monday rebuilds the row at
  the *next* Sunday, so the missed run is refreshed forward and nothing reports it. Two designs that
  would catch it were considered and rejected: preserving a missed instant makes an alarm no manual
  run can clear, and clearing it on any later run lets a manual trigger mask a dead task permanently.
  `docs/API.md` says plainly what `overdue_jobs` is and is not.
- **The long-`running` row T-11 left open is surfaced here, as its own list.** That section's write-up
  said telling an interrupted run from one in flight "needs *overdue*, which is exactly this
  section's business" — so `stalled_jobs` reports a job whose latest run has been open longer than
  any run of these jobs takes. It is deliberately not a failure (nothing failed) and deliberately not
  folded into `overdue_jobs`: a schedule can be perfectly alive and still due on time while a run of
  it lies open for ever. Its threshold is its own constant at the same six hours — the two answer
  different questions and either could move alone.
- **Six hours, reasoned rather than picked.** The margin has to absorb the longest a run takes (the
  stored instant only moves on *after* the run returns), the up-to-an-hour re-anchor the capped sleep
  performs after a DST or NTP shift mid-wait, and a slow startup. Six clears all three by an order of
  magnitude and still catches a dead weekly task on the morning of the day it should have run. The
  boundary is pinned exclusive from both sides.
- **Re-driven end to end, and the first attempt lied.** A server on a throwaway database with
  `0 0 30 2 * backup` on its schedule showed the frozen row and `GET /jobs`' `next_run_at`; back-dating
  that row seven hours produced the banner and the overdue entry. The first headless-Chrome check
  showed no **Next run** column and no banner — the binary was the one built *before* the `app.js`
  edits, since `include_str!` bakes the bundle in and `cargo test` had only rebuilt the test harness.
  Rebuilt, the Jobs screen renders the column (localised from the stored UTC instant) and the banner
  names both the overdue job and the stalled run.
- **The deployed database starts quiet, and a test pins it.** Rows exist only once an upgraded server
  has run the scheduler, so `overdue_jobs` on a freshly migrated database is empty — not loudly
  overdue for every job at once, which is the one way this change could have gone wrong on the live
  machine.

## SCENARIOS T-09: a currency-import that skipped the whole ISO 24165 half reports unqualified success

Without `DTI_REGISTRY_USER_ID` / `DTI_REGISTRY_PASSWORD`, `currencies::run_import` fetches the ISO 4217
fiat list, logs `WARN ... skipping ISO 24165 digital token import`, and returns
`ImportSummary { imported: 178 }`. Driven on 2026-08-22: the job records **success**, `GET /jobs` shows
a clean run, and the Jobs screen shows `ok` with no error — nothing in the operational surface says
that half the reference data the job exists to import was never fetched.

The consequence is already known to the project: `listing::UNRECOGNISED_DIGITAL_TOKEN` (closed as
SCENARIOS L-10) exists precisely because "the seeded list is just BTC and ETH" is otherwise a dead end,
and it names the credentials as the remedy. So the *point of use* is well handled; what is missing is
that the job the user would check first reports the gap as a clean success. A green Jobs screen is
evidence the reference data is complete, and here it is not.

**Question for Evan — what should a half-import report?**

- **(a) Say what ran.** `ImportSummary` gains a per-feed breakdown (`fiat`, `tokens: Option<usize>`)
  and the job's INFO line and the `/currencies/import` response carry it; a skipped feed is named in
  the summary rather than only in a WARN. Health/Jobs then have something to show.
- **(b) Fail the job.** A run that could not do half its work is not a success — the operator either
  configures the credentials or accepts a permanently red job.
- **(c) A health alert** — a `reference_data_incomplete` entry while the token feed has never been
  imported, cleared by a successful token import.
- **(d) Documentation only** — the credentials are optional by design and L-10 already names them at
  the point of use.

**Decision (Evan, 2026-08-22): (a), say what ran.** Rejected: failing the job, a health alert, and
documentation only.

- [x] `currencies::ImportSummary` gains a per-feed breakdown (the fiat count, and the token count as
      an `Option` — `None` meaning the feed was not attempted) rather than one total
- [x] The job's INFO line and the `POST /currencies/import` response carry it, so a skipped feed is
      named in the summary rather than only in a `WARN` nobody reads
- [x] The Jobs surface shows a half-import as what it is (the run is still a success — the
      credentials are optional by design; it just no longer *reads* as complete)
- [x] `docs/API.md` (the currencies import response shape), README where the credentials are described
- [x] Regression tests: an import with no credentials reports the fiat count and `None` tokens; one
      with both feeds reports both

**Consequences found while implementing (2026-08-22).**

- **A per-feed count alone still could not say what happened.** `fiat`/`tokens` as two `Option`s
  answers the scheduled path, but the *manual* path is a different shape: `POST /currencies/import`
  with a body imports whichever single feed was pasted, so reporting `{fiat: 0, tokens: 2}` for a
  pasted DTIF snapshot would have invented a fiat feed that returned nothing. `None` therefore means
  **not part of this run**, and a third field, `skipped`, is what separates the two ways to reach it:
  the credential-less live run carries it (a feed it would have fetched and did not), a pasted feed
  never does (it intended one feed and imported one feed). The four paths are tabulated in
  `docs/API.md`, and `import_from_content` can only ever set the count belonging to the format it
  actually parsed, so one feed's rows cannot be attributed to the other.
- **"The Jobs surface shows it" needed a place to put it, and there was none.** `job_runs` could
  record why a run went *wrong* and nothing else; a successful run had no field an operator ever
  sees. Migration 0044 adds `note` — a qualification on a **success**, written by `run_job` from what
  the job body returns, deliberately not folded into `error` (the run did not fail, and overloading
  `error` would make the screen's own red/green reading a lie) and deliberately not a health alert
  (Evan rejected one: it would nag permanently about a configuration he has chosen). A plain
  `ADD COLUMN`, no rebuild: `job_runs` carries no triggers, is out of scope for `row_history` as
  derived state, and is snapshot-staleness exempt.
- **That made the job body's return type the carrier, not the currency import's own summary.** A job
  now returns `JobOutcome = Result<Option<String>, String>` — `Ok(None)` for a complete run,
  `Ok(Some(note))` for one that did less and says so. Eight registered bodies and three synthetic
  test jobs moved to it; `run_job`'s own `Ok`/`Err` result is unchanged, so `POST /jobs/{name}` still
  answers `204` and no other caller cares. The alternative — a currency-specific field somewhere —
  would have put the reporting rule in the one job instead of in the scheduler that displays it.
- **Driven end to end against a throwaway database, not reasoned about.** A server on a temp DB with
  a `note`-carrying `job_runs` row renders a **Note** column on the Jobs screen with the badge still
  reading `ok` and the Error cell empty; the run-history expansion carries it too. The pasted-body
  responses were checked live over HTTP: `{"fiat":1,"tokens":null}` for an XML body and
  `{"fiat":null,"tokens":1}` for a JSON one — neither reporting a skip, which is the distinction the
  shape exists to make.
- **The credentials were nowhere in the README at all.** The checklist said "README where the
  credentials are described"; they were described only in a source comment and in
  `listing::UNRECOGNISED_DIGITAL_TOKEN`'s refusal text. `README.md` now has a *Digital token
  reference data (ISO 24165)* subsection under Scheduled maintenance saying what the two feeds are,
  that the skip is a supported configuration rather than a failure, the three places the run says so,
  and that a DTIF snapshot can be loaded by pasting it without giving the server credentials at all.
- **The regression tests needed an offline seam.** `run_import` fetches before it decides anything,
  so the reporting was untestable without the network. `import_feeds(pool, fiat, tokens: Option<&str>)`
  is that split — `run_import` fetches and delegates, passing `None` for the tokens in exactly the
  case the credentials are absent — and the two tests drive it directly.

## SCENARIOS U-a: a reused id inherits the deleted row's audit trail

Driven on 2026-08-22 against a throwaway database, then confirmed **live** in
`share-tracker-2026-08-16-000000.db` (read-only).

`reports::row_history` keys a trail on `(table_name, row_id)`, and `row_id` is the audited row's
`id`. Nothing binds an `id` to one row for the life of the database, so when an id is handed out
again the new occupant inherits every entry the previous one left.

**It has already happened twice in the live database, and not by mistyping an id.** Trade **9072**
was a real Sell — 2025-01-13, 1049 units at USD 3.16, brokerage 8.64, contract note `2501852833` —
deleted on 2026-07-26. Id 9072 is now the **LAC demerger's closing Sell**: 2023-10-03, price 0,
`demerger_action_id = 1`. `POST /reports/row_history {"table":"trades","row_id":9072}` answers with
the 2025 sale, presented as this trade's own past. `parcel_allocations` **#61** was reused in the
same session (there the re-created row is byte-identical, so the trail happens to read correctly).

The reuse is *server*-assigned. `trades.id` is a plain `INTEGER PRIMARY KEY`, which is an alias for
the rowid, and SQLite reuses the largest freed rowid on the next insert — so `POST
/corporate_actions/1/demerge`, inserting its closing Sell with no id of its own, was handed the
deleted trade's. No user chose it and nothing reported it.

This is the exact hazard migration **0021** identified for `closing_prices` and **0039** restated for
`exchange_holidays`, both fixed with `AUTOINCREMENT`, in 0039's words: *"a plain INTEGER PRIMARY KEY
reuses the highest rowid after a delete, so deleting a holiday and later adding another would hand
the new row the deleted one's id — and with it the deleted row's audit history. AUTOINCREMENT never
reuses an id, so a trail always belongs to exactly one holiday."* The reasoning was never applied to
the rest: of the 22 audited tables, **only those two are `AUTOINCREMENT`; the other 20 reuse ids.**

`AUTOINCREMENT` alone does not close it. It governs only what SQLite picks; every entity is
PUT-upsert on a client-supplied id, so `PUT /trades/9072` after deleting trade 9072 reuses the id
whatever the column says. The trail does hold the evidence in both cases — a trail whose newest entry
is a `DELETE` on a row that currently exists can only mean the id was recycled — but nothing says so,
and INSERTs record nothing to mark the boundary.

**Question for Evan — how far to take it?**

- **(a) Mark the boundary in the report.** No migration; covers server-assigned and hand-entered
  reuse alike; fixes what the trail *claims* without touching 20 tables.
- **(b) Mark the boundary, and make the remaining audited tables `AUTOINCREMENT`.** Ends accidental
  server-side reuse permanently — an id then means one row forever, which matters beyond the trail
  (an export or a note citing "trade 9072"). Large: rename-pattern rebuilds, every FK and trigger set
  re-created.
- **(c) Refuse to reuse an id** — reject a PUT on an id carrying a `DELETE` entry. Blocks the
  legitimate "delete a mis-entered row, re-enter it under the same id" workflow, and cannot cover
  server-assigned inserts without (b) anyway.

**Decision (Evan, 2026-08-22): (b), both halves.** Rejected: marking the boundary alone, and
refusing the reuse.

**Revision (2026-08-22), after the fix's premise was re-derived and found false.** `AUTOINCREMENT`
governs only the ids **SQLite** picks, when an INSERT omits the id column. It would **not** have
prevented the headline case. Nine call sites in `src/entities/` assign ids themselves —
`SELECT COALESCE(MAX(id), 0) + 1 FROM <table>`, seven for `trades` (`demerger`, `transfer`,
`scrip_exchange`, `worthless`, `ess_vest`, `inheritance`, `buyback_participation`), one for `income`
and one for `amit_adjustments` — and bind the result explicitly, so the column definition never gets
a say. The live sequence confirms it: 9072 was the highest trade id, deleting it made `MAX(id)` 9071,
`POST /corporate_actions/1/demerge` computed 9071 + 1 and took 9072 back, and the re-entered 2025 sale
became 9075.

The migration is still worth doing — the *other* live reuse, `parcel_allocations` #61, came from an
id-less INSERT where SQLite reused the freed rowid, which is exactly what `AUTOINCREMENT` prevents,
and both flavours of INSERT are common across the audited tables. But it is only half the prevention.
Dropping the nine `MAX(id) + 1` queries is worth doing on its own account too: computing an id by
reading the table's maximum is a race between concurrent operations, independent of the audit trail.

**Revised decision (Evan, 2026-08-22): the full fix — boundary marking, `AUTOINCREMENT`, *and*
the nine call sites reworked to let the database assign the id.** Rejected: allocating above the
trail's high-water mark instead of migrating (leaves SQLite's own rowid reuse unfixed), and boundary
marking alone.

- [x] The row-history report marks a trail that crosses a `DELETE` on a row that still exists: the
      entries at or before that `DELETE` belonged to a previous occupant of the id, and are labelled
      as such rather than presented as this row's own history
- [x] The Row History screen surfaces that boundary (not a bare extra column — the reader must not
      have to infer it), and `docs/API.md`'s Row history section states the rule
- [x] The nine `SELECT COALESCE(MAX(id), 0) + 1` call sites stop assigning ids: the INSERT omits the
      id and the server reads `last_insert_rowid()`, so a never-reused id comes from the database
      (this, not the migration, is what fixes the trade 9072 case)
- [x] The audited tables that reuse ids are rebuilt with `AUTOINCREMENT` ids by the rename pattern
      0021/0039 established — every FK re-pointed, every `*_row_history_*` and `*_stale_snapshots_*`
      trigger set dropped and re-created, no row dropped or re-scaled (see the 0029 FK-rewrite
      gotcha and `infra::db`'s `migrations_store_decimals_as_text` guard)
- [x] `docs/SCHEMA.md` records the `AUTOINCREMENT` requirement for an audited table and why
- [x] Regression tests: a delete-then-recreate on the same id reads back as two occupants, not one
      history; a server-assigned insert after a delete never reuses the freed id; a test pinning
      that every audited table's id column is `AUTOINCREMENT` (so a new audited table cannot be
      added without it)

**Boundary marking done 2026-08-22** (the first two items; the id-assignment rework, the
`AUTOINCREMENT` migration, the SCHEMA.md note and the AUTOINCREMENT pin test are still open).

**`AUTOINCREMENT` migration done 2026-08-22** (`0045_autoincrement_audited_ids.sql`). Still open:
the nine `SELECT COALESCE(MAX(id), 0) + 1` call sites, `docs/SCHEMA.md`'s statement of the
*requirement* and why, and the regression tests — including the pin that every audited table's id
column is `AUTOINCREMENT`, so a new audited table cannot be added without it.

**Id assignment reworked, and the section closed — done 2026-08-22.** All nine
`SELECT COALESCE(MAX(id), 0) + 1` queries are gone: every server-created row now omits the id
column (or binds NULL, which for an `INTEGER PRIMARY KEY AUTOINCREMENT` column is the same thing)
and reads back what the database assigned. One shared write core covers five of the nine sites:
`sell::upsert_sell_in_tx`, whose `id` is now `Option<i64>` — `Some` only on the client-supplied-id
path `PUT /sells/{id}`, which stays an upsert — and which answers the id written, for the
scrip-exchange, demerger, transfer (and its network-fee), worthless and buy-back closing Sells. The
other four are the ESS vest parcel, the inherited parcel (NULL on first entry; an edit still keeps
the linked Buy's id through the `ON CONFLICT` arm), the buy-back's dividend `income` row, and the
generated AMIT adjustments — `amit_adjustment` gained `db_insert_on` over the same validation core
as `db_upsert_on`, and its duplicate-parcel check now reads `id IS NOT ?` so a NULL id excludes
nothing. Alongside the nine, `rollover::insert_replacement_buy` (no `id` field on `ReplacementBuy`
any more, answers `last_insert_rowid()`) was the same bug in another shape: the demerger, transfer
and scrip exchange numbered their replacement Buys `sell_id + 1 + i`. Each Buy now takes the id its
own INSERT was given. A *preview* generation writes nothing, so its rows
carry no id at all (`UNASSIGNED_ID` = 0) rather than a prediction; `docs/API.md` says so.

Proved behaviourally: `reports::row_history`'s
`a_server_assigned_insert_never_takes_a_deleted_trades_id` (the old
`a_server_assigned_id_taking_a_deleted_trades_place_is_marked`, inverted) deletes the highest-id
trade and drives a real `POST /corporate_actions/{id}/demerge` — the closing Sell and both
replacement Buys take three fresh ids, none of them the freed one, each with an empty trail, while
the deleted Buy's own trail still reads as one occupant and its id holds no row.
`amit_adjustment_generation`'s `db_generation_never_reuses_a_deleted_adjustments_id` does the same
for the adjustments table and checks each reported id is the row really stored. The
`AUTOINCREMENT` pin is `every_audited_tables_id_is_autoincrement`: derived from the live schema and
`AUDITED_TABLES`, so a new audited table cannot be added without it (verified non-vacuous — adding
a plain-PK table to the loop fails it). Its two exemptions are *checked*, not skipped —
`tax_year_settings` must still have no `id` column, `cgt_settings` must still carry
`CHECK (id = 1)`. On a copy of the live database, deleting the highest trade (9076) and inserting
an id-less trade takes **9077**, not the freed id.

Exactly **17** of the 22 audited tables reused ids and are rebuilt: `trades`, `parcel_allocations`,
`income`, `interest_income`, `amma_statements`, `amit_adjustments`, `ess_statements`, `transfers`,
`corporate_actions`, `inheritances`, `rights_sales`, `rights_sale_allocations`,
`investment_expenses`, `drp_enrolments`, `attachments`, `listings`, `listing_renames`. Five are
deliberately left alone, and the migration header says why: `closing_prices` (0021),
`rba_fx_rates` (0031) and `exchange_holidays` (0039) are already `AUTOINCREMENT`;
`tax_year_settings` is keyed on the financial year itself, with no surrogate id to make one (0027,
and the boundary marking exempts it for the same reason); and `cgt_settings` is
`id INTEGER PRIMARY KEY CHECK (id = 1)`, a singleton whose CHECK pins the id, so re-creating its one
row is re-entry of the same fact, not reuse.

The shape is 0029's, because most of these tables are referenced by another — `attachments` alone
has six `ON DELETE CASCADE` parents among them, and a rename that repointed it at `<parent>_old`
would have cascaded every attachment away when that table was dropped. `-- no-transaction` with the
migration's own `BEGIN`/`COMMIT` around `PRAGMA foreign_keys = OFF` (a no-op inside a transaction),
plus `legacy_alter_table` per rename so no trigger body is rewritten either. Per table: both trigger
sets dropped, rename, re-create with `id INTEGER PRIMARY KEY AUTOINCREMENT` and every other column,
constraint and index unchanged, copy `ORDER BY id`, drop the old table, re-create the indexes and
then the triggers — the staleness triggers last, so the migration's own copy does not stale every
stored snapshot. Each table's definition and both triggers are reproduced from the **live** schema
rather than from the migration that first created them, since several had been re-created since
(`trades`' pair comes from 0041, not 0013).

**Seeding `sqlite_sequence` is load-bearing, not defensive.** `AUTOINCREMENT` never issues an id at
or below the table's stored sequence, and a plain copy sets that to the largest *live* id — leaving
an id freed before the migration still issuable. In the live database `parcel_allocations` holds 33
rows with a maximum id of **63**, while its trail's highest `row_id` is **65**: a plain copy would
have handed the next two allocations 64 and 65, and 65 already has an audit trail — the bug would
have reproduced on the first write after the migration. So each table's sequence is seeded to
`MAX(largest live id, largest row_id that table has ever recorded in row_history)`, the trail being
the only surviving record of an id that no longer holds a row (append-only and keep-forever, 0013,
so the mark cannot recede). `attachments` is the mirror case (live 140, trail 136) and takes 140; an
empty table with no trail seeds to 0, which is what an untouched `AUTOINCREMENT` table means anyway.

Acceptance-tested against a copy of the live database (`share-tracker-2026-08-22-205530.db`, 45 MB,
1,329 trail entries, migrated to head): for all 30 tables the row count, the full id set and an
all-columns checksum are **byte-identical** before and after — `PRAGMA integrity_check` `ok`,
`PRAGMA foreign_key_check` empty. All 155 schema objects compare equal after comment/whitespace
normalisation *modulo* the 17 `AUTOINCREMENT` keywords, and a DB built from the migrations from
scratch produces the same schema. Post-migration sequences: `trades` 9076, `parcel_allocations`
**65** (not 63), `attachments` 140, `income` 47, `amit_adjustments` 149, `interest_income` 25,
`transfers` 10, `amma_statements`/`listings` 8, `ess_statements` 5, `drp_enrolments` 3,
`corporate_actions`/`listing_renames` 1, the four empty tables 0. Behaviourally, two inserts into
the migrated copy take ids **66 and 67** — not the freed 64 and 65 — and a new trade takes 9077.

The reuse this fixes is measurable in that database: the trail carries ten `DELETE` entries on ids
that hold a row again, across eight distinct ids (`trades` 9072-9076, `parcel_allocations` 61-63,
two of them reused twice).

The single-row form now segments a trail into the successive **occupants** of the id and says which
is which, because the trail already holds the evidence: INSERTs are not recorded, so a `DELETE` on an
id that **still holds a row** can only mean the id was handed out again. Every `DELETE` therefore
closes an occupancy — the `DELETE` and everything older belong to an earlier occupant — with one
exception: the newest entry of a trail whose id holds no row now is that occupant's own death, an
ordinary deleted row. Segmenting rather than splitting once was deliberate: delete/recreate twice
reads as three occupants. Each entry carries `occupant` (`1` = the id's most recent occupant) and
`current_occupant` (`true` when it belongs to the record holding the id now); both are additive, so
no existing field changed meaning. "Does the id hold a row?" is read on the **same transaction** as
the trail, or a concurrent delete would label the boundary against a row that had just gone.

It is honest about the two things it cannot know, both stated in `docs/API.md` and on screen: *when*
the id was taken again (the re-insert recorded nothing), and whether the new occupant is a re-entry
of the same record. `tax_year_settings` is exempt — its `row_id` is the financial year itself, and
0027 already decided that re-entering a year's settings is the *same* taxpayer-year fact, so it stays
one occupant.

The screen (`sections`, a new generic `viewReport` hook for an array response whose rows are not all
one thing) renders a headed section per occupant — a previous occupant's named with the timestamp of
the `DELETE` that ended it — under a boxed `.section-notice` warning; the record holding the id with
no entries of its own gets an explicit empty section rather than silently missing one. A trail with
one occupant renders exactly as before: one plain table, no heading, no notice, and neither marking
field as a column. The browse form deliberately carries no marking (it lists the trail in write
order, where no entry is presented as any row's own history; the drill-through link lands on the
single-row form, which does).

Tests: `reports::row_history::tests::a_reused_id_splits_into_two_occupants`,
`a_reused_ids_new_occupant_may_have_no_history_of_its_own`,
`an_id_reused_twice_segments_into_three_occupants`,
`a_server_assigned_id_taking_a_deleted_trades_place_is_marked` (the live shape reproduced end to end
— deleting the highest trade id then demerging, whose `MAX(id) + 1` hands the freed id straight to a
server-created Sell, exactly as trade 9072 became the LAC demerger's closing Sell), the two
non-reuse cases (`an_edited_row_that_still_exists_is_one_occupant`,
`a_deleted_row_is_one_occupant_not_a_reuse`), `a_natural_key_re_entered_is_still_one_occupant`,
`api_entries_carry_the_occupant_they_belong_to`, `browse_entries_carry_no_occupant_marking`,
`web::tests::row_history_ui_present` and `doc_checks::row_history_audit_trail_documented`. Rendering
verified with `scripts/ui-check.sh` over all three shapes (re-use, re-use with no own history, plain
trail).

---

## SCENARIOS U-b: a multi-row operation's trail is only readable one row at a time, by ids you never saw

Driven on 2026-08-22.

`POST /reports/row_history` takes `{table, row_id}` and nothing else, so reading the trail requires
already knowing the numeric id of the row you lost. That is exactly what a user does not have for the
rows the system created or destroyed on their behalf.

Driven concretely: `DELETE /sells/3` on a demerger's closing Sell removed the whole group and wrote
**four entries across two tables** — `trades` 3, 4 and 5 and `parcel_allocations` 1. Only trade 3's
id was ever named by the user: trades 4 and 5 and the allocation were created
by `POST /corporate_actions/7/demerge` and are now gone from every list endpoint. The same shape is
in the live database at `2026-07-26T07:39:44.222Z`, spanning `trades`, `attachments` and
`parcel_allocations`. The cascade case (U-04) is identical — deleting a trade takes its attachments
with it, and their ids appear nowhere afterwards.

The UI states the gap without meaning to: the Row ID field's hint reads *"The record's id as shown in
its entity list"*, and a deleted row is by definition not in its entity list. The trail is complete
and correct; it is simply not reachable.

Also confirmed while driving this: `POST /clear_unpriced_before` deletes hundreds of price rows in
one transaction (the documented case is 635), every one recorded and every one keyed on an id the
user never saw.

**Question for Evan — how should the trail become discoverable?**

- **(a) A recent-changes browse mode** — list entries newest-first across all tables, paged, with no
  `row_id`, so an operation is found by when it happened and drilled into. Covers every unknown-id
  case at once and adds no new concept: the trail is already ordered and indexed.
- **(b) Group by transaction timestamp** — return every entry sharing one `changed_at`. Answers the
  multi-row question exactly, but still needs one known id for a foothold.
- **(c) Both.**
- **(d) Document it as a known limitation.**

**Decision (Evan, 2026-08-22): (a), the browse mode.** Rejected: timestamp grouping (it needs a
foothold the user does not have), both, and documenting it.

- [x] `POST /reports/row_history` returns a newest-first page of entries across every audited table
      when no `row_id` is given, keeping the existing single-row behaviour unchanged when one is
      — paged, so a large trail stays bounded on this path
- [x] The Row History screen reaches it without a row id, and the `row_id` field's hint stops
      implying the entity list is the only way in
- [x] `docs/API.md`'s Row history section documents the browse form and its paging
- [x] Regression tests: the browse form returns entries across tables newest-first and pages; a
      multi-row operation (a demerger group delete) is findable through it without knowing any of
      the created rows' ids

Done 2026-08-22. `POST /reports/row_history` now answers two shapes, chosen by whether the body
names a `row_id`:

- **One row's trail** — unchanged, byte for byte: the same flat array of prior versions, each
  flattening the audited table's own columns behind `history_id`/`operation`/`changed_at`. `table`
  is still required alongside a `row_id` (a row id means nothing without the table it is an id in),
  and the existing tests were not touched.
- **Recent changes** (`{}`) — an object, `{entries, page_size, next_before_id}`, whose entries are
  **uniform across tables**: `history_id`, `table_name`, `row_id`, `operation`, `changed_at`, and
  nothing else. The flat shape could not be reused — rows of `trades` and `parcel_allocations` have
  different columns and the UI renders every data table through one `filterableTable` with one
  column set — and `old_row` is deliberately neither flattened nor summarised: a summary would have
  to choose what to show, and could misrepresent what changed. The prior values stay one drill-down
  away, through the `(table_name, row_id)` each entry names in full.

Paging is a **cursor**, not an offset: `before_id` returns the entries older than that trail id,
`limit` defaults to 100 and is bounded at 1000 (outside that is a 422 naming the cap, never a
silently truncated page), and `next_before_id` is null *exactly* when the page reached the end of
the trail — so "more remains" is stated, not inferred from a full-looking page. The trail is
append-only, so new entries land at the top and an offset page would shift under a concurrent
write. `table` without a `row_id` filters the page to one audited table (still 422 if it is not
one); `before_id`/`limit` alongside a `row_id` are refused rather than ignored (one row's trail is
returned whole).

UI: the same screen, still one config-driven `REPORTS` entry. Both params became optional, a
`before_id` param joined them, and the screen `autoRun`s — every field is optional, so it opens on
the browse page and the form narrows it. The browse object renders through the existing `tables`
mechanism (`viewReport` now applies `tables` to object responses only, so the single-row array
falls through to the plain table as before), each browse row carries a **Trail** link to
`#/r/row-history/<table>/<row_id>` (report hash routes now take extra path segments that prefill a
params form positionally and run it), and a paged response renders a "Load older →" button that
fills `before_id` and re-runs — field-driven on `next_before_id`, like the existing taxpayer-basis
note. `dataTable` grew one guard: the Actions column appears only where some row has an action, so
the single-row trail (whose rows have no `table_name`) does not carry an empty column. The Row ID
hint no longer says "as shown in its entity list" — the wording the finding called out, since a
deleted row is not in one; it now names the browse form as the way in for a row no list shows.

Tests: `reports::row_history::tests::browse_returns_entries_across_tables_newest_first`,
`browse_pages_by_cursor_and_says_when_more_remain`,
`browse_filters_to_one_table_and_refuses_a_bad_request`, and
`a_demerger_group_delete_is_findable_without_knowing_any_ids` — which drives the finding's own
case: demerge, delete the group's closing Sell, then find the two demerge-created Buys and the
allocation from the browse page alone and drill into one of them by the `row_id` its entry carries.
Plus `web::tests::row_history_ui_present` (the cursor param, the drill-in link, and the absence of
the old hint) and `doc_checks::row_history_audit_trail_documented` (the browse section, the response
shape, the cursor, the bound, the ordering rule). Rendered end to end with
`scripts/ui-check.sh --seed … '#/r/row-history'`: the browse table, the Trail links, and — over 100
entries — the "Load older" affordance.

**A claim in the finding is wrong, and it mattered.** "`'now'` is constant across a transaction in
SQLite" is not so: it is fixed for one *statement*. Measured (2026-08-22): two `strftime('now')`
reads in one transaction, a long query between them, came back 227 ms apart — and a first draft of
the demerger test, which grouped the operation's entries by shared `changed_at`, failed
intermittently because the delete's four rows span three statements. So the timestamps of one
operation *tie* where a single statement wrote them and *differ* where it did not: `changed_at` is
neither unique nor a total order, and ordering/paging on it would skip or repeat rows. Ordering is
on the trail's own `id` throughout, which is what the decision asked for. It also retires option (b)
(group by `changed_at`) as more than merely needing a foothold: it would have been unreliable.

---

## SCENARIOS U-c: nothing pins an audited table's trigger column list against the live schema

Driven on 2026-08-22 — and the machinery came back **correct**: diffing every audited table's
`PRAGMA table_info` against the `json_object` keys of both its `*_row_history_*` triggers found all
22 tables complete, with `attachments.content` the single documented exclusion (a BLOB `json_object`
cannot hold).

What is missing is the guard. "A migration that adds a column to an audited table must DROP and
re-CREATE that table's two `*_row_history_*` triggers with the new column list" is stated in
CLAUDE.md, in 0013's header and in `docs/SCHEMA.md`, and is enforced only by **hand-written
per-migration assertions** — `audited_tables_match_migration_check_and_triggers` pins the *lists* to
each other and then checks specific migrations by name (0026 re-creating the `ess_statements` pair
with `fx_rate`, and so on). A future `ALTER TABLE ... ADD COLUMN` that forgets the rebuild adds no
failing test: the column would simply stop being recorded, silently, and the trail would keep looking
healthy. The check that catches it is about twenty lines and derives everything from the live schema.

**Question for Evan — add the generic guard?**

- **(a) Add the generic test.**
- **(b) Leave the per-migration assertions.**

**Decision (Evan, 2026-08-22): (a).**

- [x] A test walks every table in `AUDITED_TABLES`, reads its columns from the live schema and both
      its triggers' `json_object` keys, and fails on any column the trail would drop — with
      `attachments.content` allowlisted as the documented BLOB exclusion, and a missing trigger
      reported as a failure rather than a skip
- [x] The comment says it supersedes the bespoke per-migration column assertions for future
      migrations, so the next one is not written by hand

Done 2026-08-22. `reports::row_history::tests::every_audited_column_is_recorded_by_both_triggers`
walks all 22 `AUDITED_TABLES` over a `test_pool()` (every migration applied), reads each table's
columns from `pragma_table_info` and each of its two `*_row_history_*` triggers' recorded keys from
`sqlite_master`, and asserts no column is missing from either. A trigger that is absent panics
naming it rather than being skipped, so an audited table with no pair fails just as loudly as one
with a dropped column. `attachments.content` is the single allowlisted exclusion, carrying 0013's
reason (a BLOB is not something a `json_object` can hold).

The key list is parsed by a `json_object_keys` helper that scans only the text between the
`json_object(` call's own parentheses — the enclosing `INSERT INTO row_history ... VALUES
('<table>', OLD.id, ...)` has a quoted-string/`OLD.`-value pair of its own that a whole-body regex
reads as a column — and takes every quoted string in it, so identifiers with digits
(`pre_2009_cessation_discount`) are matched whole. No new dependency: plain string scanning, since
`regex` is not in the tree. All 22 tables pass on `main` as it stands; deleting the allowlist makes
it fail on `attachments.content`, which is how the check was confirmed to have teeth.

The doc comment says it supersedes the per-migration column assertions in
`audited_tables_match_migration_check_and_triggers` for *future* migrations — the existing ones stay,
because they pin something derived checking cannot: which migration the live trigger pair came from.

## `next_run_log_shows_timezone` flakes under CPU contention

Surfaced 2026-08-22 while closing SCENARIOS U-a, in a run unrelated to the change under test (the
diff touched no scheduler code). `infra::scheduler::tests::next_run_log_shows_timezone` failed once
in roughly ten full-suite runs, only when the machine was loaded; it passed 15/15 in isolation under
the same load.

The test waits for a spawned task to emit its `next run scheduled` log line by calling `yield_now`
about fifty times. That is a bounded spin on the scheduler's goodwill, not a synchronisation
primitive: under contention the spawned task may not have been polled to the point of logging within
those yields, and the assertion then reads an empty buffer. Nothing about the *behaviour* is in
doubt — the same line is asserted by neighbouring tests that do not race for it.

A flaky test in a suite that gates every commit is worse than a missing one: it trains the reader to
re-run rather than read. Worth fixing properly (wait on a signal the logging path actually sets,
rather than counting yields), not by raising the yield count.

- [x] `next_run_log_shows_timezone` waits on something deterministic instead of a bounded `yield_now`
      spin, and no longer fails under CPU contention
- [x] A note in the test says why it cannot go back to counting yields

Done 2026-08-22. It went red in CI first — on the `v0.13.0` release push, at 1953 passed / 1 failed,
in a run whose suite took 140s against ~4s locally, which is exactly the contention the yield count
could not survive. The fixed spin is replaced by `wait_until(cond, what)`, a bounded poll on the
condition itself: it returns on the first poll that sees the line (so it is *faster* in the common
case, not slower), and panics naming what it waited for rather than hanging, so a real regression is
still a failure with a name. Verified by running the test **25 times under full CPU saturation** —
every core pinned by spinners, the condition that produced the CI failure — with 0 failures.
The comment records why counting yields cannot work: a `yield_now()` count bounds how many times
*this* task defers, not whether the spawned task has been polled far enough to log.

The other `yield_now` loop in this file (the in-flight-run wait in the `POST /jobs` tests) is left
alone deliberately: it already spins on the real condition rather than a fixed count. It has no
deadline, which is a smaller wart — a broken condition hangs rather than failing by name — and is
worth tidying only if it ever bites.

---

## A request arriving during startup answers 500 "database is locked"

Found 2026-08-22 by CI going red on the `v0.13.0` push — not on the test suite this time but on
`scripts/ui-smoke.sh`, whose fixture seeding got `PUT /listings/2 -> 500` with an empty body.

Reproduced locally by starting a fresh server and issuing one `PUT` as soon as it answers: **2 in 40
runs** fail with `error returned from database: (code: 517) database is locked`. The server log puts
the error in the middle of the scheduler's startup `next run scheduled` lines, and an **empty
`schedule.cron` makes it vanish (0 in 40)** — so the collision is with the per-entry `job_schedule`
writes that `spawn` performs at startup, concurrently with the server already serving requests.

**It is not migration 0045.** Checked rather than assumed, because the migration had just been
deployed: a build of `7b915cf` (the commit before 0045) fails the same way **8 times in 60**, a
higher rate than the 2 in 40 measured after it. The race arrived with the `job_schedule` table
(migration 0043, SCENARIOS T-11/T-02/T-12) — the first thing in this system to write to the database
from a background task while requests are being served.

**Why `busy_timeout` does not already cover it.** sqlx sets a 5-second `busy_timeout` by default, so
plain `SQLITE_BUSY` waits. But code 517 is `SQLITE_BUSY_SNAPSHOT`, which `sqlite3_busy_timeout()`
deliberately does **not** retry: it is returned when a transaction that began deferred — as a reader
— tries to upgrade to a writer after another connection has committed since its read snapshot was
taken. SQLite returns it immediately and expects the application to roll back and retry, or to have
taken the write lock up front. `pool.begin()` issues a deferred `BEGIN`, so every write transaction
in the tree is exposed; there are 26 files beginning transactions on the write side and 21 on the
read-only report side.

The impact is small but real and user-visible: a 500 with an empty body, which the web UI can only
show as `HTTP 500` — the same complaint SCENARIOS T-10 raised about the jobs endpoint. It is not a
correctness risk (the transaction fails atomically; nothing partial is written), and it needs a
concurrent writer, which for a single-user tool means startup or a job running as you click.

**Question for Evan — how to fix it?**

- **(a) Write transactions take the lock up front** — `pool.begin_with("BEGIN IMMEDIATE")` on the
  write paths, leaving the read-only report snapshots deferred. This is what SQLite documents for
  exactly this error, and it makes the existing 5-second `busy_timeout` effective: a concurrent
  writer waits instead of failing. Touches the write-side `begin()` sites and needs a shared helper
  so a new write path cannot quietly go back to a deferred `BEGIN`.
- **(b) Retry the transaction on 517** in a wrapper, leaving the `BEGIN`s deferred. Keeps the
  transaction shapes but puts retry logic on every write path, and a retried financial write must
  re-run the whole transaction, not resume it.
- **(c) Keep the scheduler off the database while the server is starting** — write `job_schedule`
  before binding, or serialise it. Narrows this trigger but leaves the general race live (a manual
  `POST /jobs/{name}` while you are entering a trade collides just the same).
- **(d) Accept it** — a rare startup-only 500, already atomic, and document it.

**Decision (Evan, 2026-08-22): (a)**, write transactions take the lock up front. Rejected: retrying
on 517 (b), narrowing the scheduler's startup writes (c), and accepting it (d).

- [x] Decision recorded and implemented

      Done 2026-08-23. `infra::db::write_tx(pool)` is the one way a write transaction is begun —
      `pool.begin_with("BEGIN IMMEDIATE")`, with the reasoning in its doc comment — and all 39
      write-side `pool.begin()` sites across 27 files now go through it: every file under
      `src/entities/`, `src/infra/scheduler/db.rs`, and **`src/reports/snapshot.rs`**, the one
      report that writes (it persists the price-dependent reports to `report_snapshots`; a
      reports-are-readers split would have missed it). The other 20 report files stay deferred:
      they never upgrade, so they cannot hit this, and taking the write lock up front would
      serialise every report against every other for nothing. `src/domain/` turned out to begin no
      transactions at all — it composes onto the caller's connection — so it needed no change.

      `StoredSchedule::record` (`infra/scheduler/run.rs`), the `job_schedule` write that triggered
      the bug, needed no change either: it writes through `db_insert_schedule`/`db_update_schedule`,
      single statements executed straight on the pool. A lone statement is its own implicit
      transaction, which takes the write lock immediately and gets plain `SQLITE_BUSY` — the one
      the 5-second `busy_timeout` *does* retry. It was only ever the other side of the race.

      Pinned by `infra::db::tests::write_side_modules_never_begin_a_deferred_transaction`, a source
      scan in the spirit of the `.bind(x.to_string())` one: a deferred `BEGIN` anywhere under `src`
      fails the test unless the file is named in `DEFERRED_BEGIN_ALLOWED`, which lists the 20
      read-only report files one at a time (not `src/reports/`, so a *new* report is an offender
      until someone decides which side it is on) and rejects an entry that has gone stale.

      Measured on the reproduction from the diagnosis above — fresh server on a temp DB with the
      real `schedule.cron`, poll until it answers, then immediately `PUT /listings/2`: **2 failures
      in 160 runs before** (one `(code: 517)`, one `(code: 5)` — the same failed upgrade surfaces
      as either), **0 in 200 after**.
- [x] `scripts/ui-smoke.sh` dumps the server log when a **seed** request fails, not only when the
      server fails to start — the cause was logged and CI threw it away, which is what made this a
      half-hour diagnosis instead of a one-line one

      Done 2026-08-23, in `scripts/ui-check.sh`, which is where ui-smoke's seeding happens: the
      seed step's exit status is captured and a non-zero one prints the server log before exiting.
      A failed seed reached a *running* server, so a 500's cause is in the log and nowhere else —
      `ApiError::Internal` answers with an empty body by design. Verified by seeding a deliberately
      invalid fixture: the run exits 1 and the log is printed.
- [x] A regression test: a write arriving concurrently with the scheduler's startup writes succeeds
      rather than answering 500

      Done 2026-08-23, as a pair. `infra::scheduler::tests::
      a_write_arriving_during_scheduler_startup_is_served_not_locked_out` is the end-to-end one:
      `spawn` over the real `schedule.cron` (repeated, so ~64 entry tasks claim `job_schedule` rows
      at once), 15 concurrent `PUT /listings/…` fired immediately behind it, five startups, every
      request required to answer 204. It is a race, not a scripted interleaving, so its power is
      measured rather than assumed: against a build with `write_tx` reverted to a deferred `BEGIN`
      it caught the regression **29 times in 30** (`PUT … -> 500`), and passed **30 in 30** with
      the fix. The deterministic half is in `infra::db`:
      `a_deferred_transaction_cannot_upgrade_after_a_concurrent_write` pins the failure itself
      (immediate, not after the busy timeout), and
      `write_tx_holds_off_a_concurrent_writer_instead_of_failing_to_upgrade` pins the fix — the
      concurrent writer must still be blocked while the transaction holds the lock, which is
      exactly what a deferred `BEGIN` cannot do, so it fails 100% of the time on a regressed build.

## SCENARIOS V-a — a misspelt field name in a request body is silently ignored

Raised driving **V-01 / V-09** (a year of history entered in one session). Every HTTP request
body in the tree deserialises with serde's default behaviour, so a key the struct does not
recognise is **dropped**, and the field it was meant to set takes its `#[serde(default)]` value.
A **required** field is already safe — omitting it is a `422` naming it — but almost every
*money* field on the tax-bearing entities is optional-with-default, so a one-character typo
writes a legitimate-looking row with a zero in it and answers `204`.

Measured against a throwaway database:

| Request | Sent | Stored | Response |
| --- | --- | --- | --- |
| `PUT /amma_statements/9` | `franked_dividend: "5000"`, `frankingcredits: "2142"` | `franked_dividends: 0`, `franking_credits: 0` | `204` |
| `PUT /trades/7` | `settlment_date: "2025-04-09"` | `settlement_date: 2025-04-03` (`computed`) | `204` |
| `PUT /trades/7` | `contract_note: "CN123"` | `contract_note_ref: null` | `204` |
| `POST /reports/row_history` | `table_name: "parcel_allocations"` | filter ignored — whole trail returned | `200` |

The AMMA row is the one that matters: A$7,142 of a lodgeable tax figure vanished with nothing
anywhere saying so. `income` (every component `#[serde(default)]`), `interest_income` (`amount`
itself defaults, as does `foreign_source`, which routes the row between 10L and 20E) and
`investment_expense` have the same shape.

**The project already holds the opposite convention, for the two bodies that are not HTTP.**
`infra/config.rs` and `scheduler::JobParams` both carry `#[serde(deny_unknown_fields)]` with the
reasoning written out beside it — *"`deny_unknown_fields` makes a misspelt parameter a rejection
rather than a silently-ignored default"* — and **T-10** made an unrecognised *query* parameter on
`POST /jobs/:name` a `422` naming it for exactly this reason (`` cannot read the query string:
sufix: unknown field `sufix` ``). The HTTP request bodies are the gap. 233 `Deserialize` derives
in `src`, none of them denying.

Options offered:

1. `#[serde(deny_unknown_fields)]` on every HTTP request-body struct, with a test that
   enumerates the bodies reachable from a handler so a new one cannot be added without it.
2. The same, but on the **write** bodies only (entity `PUT`/`POST`), leaving report request
   bodies permissive.
3. Leave it and document it as a known limitation.

**Evan chose option 1** — `deny_unknown_fields` on *every* HTTP request-body struct, report
bodies included, with a test enumerating the bodies reachable from a handler so a new one cannot
be added without it.

- [x] Add `#[serde(deny_unknown_fields)]` to every HTTP request-body struct, with the enumerating
      test and a `docs/API.md` note that an unrecognised body field is refused.
