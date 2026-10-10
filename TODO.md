# TODO

Items are only marked done when a passing test exists for them.

This file holds only open / in-flight work. Completed and decided (out-of-scope / not-reproducible)
sections are archived in the topical `DONE/*.md` files, indexed by [DONE.md](DONE.md). When a
section here is fully done, move it into the matching `DONE/*.md` file rather than leaving it — see
CLAUDE.md.

A section records one finding, and its heading names where it came from — a REQUIREMENTS entry, a
[SCENARIOS.md](SCENARIOS.md) section, or a dated review pass.

The last full pass recorded here — the 2026-09-17 code review — is closed and archived in
[`DONE/reviews.md`](DONE/reviews.md) (26 sections), with its summary and closing narrative in
[`DONE/verification-passes.md`](DONE/verification-passes.md). Everything before it is likewise
archived; the maintained record of what has been verified is SCENARIOS.md's
[Verification status](SCENARIOS.md#verification-status) table, and of what was built and decided,
the [DONE.md](DONE.md) index. The 2026-09-24 REST API audit — its documentation fixes, consistency fixes,
machine-client surface and API improvements — is closed and archived in
[`DONE/api.md`](DONE/api.md), which is the record of what it found and what
answered each finding.

The nine items the 2026-09-26 per-commit review deliberately left — the structural
enforcement it judged out of proportion, and the three decisions it would not make on the
owner's behalf — are closed too, and archived with the review itself in
[`DONE/reviews.md`](DONE/reviews.md).

The 2026-10-04 HTTP API consistency sweep — cross-site write and DNS-rebinding guard, strict lists,
one path casing, readable writable URLs, one rejection shape, and the `200`/`201` rule — is closed
and archived in [`DONE/api.md`](DONE/api.md), and so is the 2026-10-04 agent-usability pass that
followed it (OpenAPI bodies, ids, descriptions and slices, `?dry_run=true`, the live-API skill,
trustworthy `required` lists, and reasoned GET-one `404`s). The Budget section below came from the
2026-10-08 sweep of the ATO mirrors (`docs/ato/OVERVIEW.md`, *Full sweep, 2026-10-08*); the sections
after it are the findings of the 2026-10-10 metric-scored code review (financial correctness 8,
error handling 8.5, concurrency/integrity/performance 8, security 8.5, architecture 6, tests 8,
hygiene 9 — out of 10), most severe first.

## 2026–27 Budget CGT changes — announced, not law (ATO mirror sweep 2026-10-08)
The ATO's CGT discount, cost base, how-to-calculate and indexation pages have carried a banner
since 29 June 2026: "Recent changes to capital gains tax (CGT) announced in the 2026–27 Federal
Budget don't apply to Tax Time 2026", with "Resources will be available at a later date". As
reported (not yet ATO guidance), the measure replaces the 50% discount with **cost-base indexation
plus a 30% minimum tax** on real gains for assets held 12 months or more, from **1 July 2027**,
with an asset bought before and sold after that date taxed under the existing rules for the gain
accrued to 1 July 2027 and the new rules after it. Nothing is modelled or mirrored yet: there is
no legislation or ATO guidance to implement against, and every disposal this system can hold today
falls under the existing rules.
- [ ] Watch: when the measure is legislated and the ATO publishes guidance, mirror it into
  `docs/ato/` and index it in `docs/ato/OVERVIEW.md` (re-fetch the four bannered pages then too)
- [ ] NEEDS DECISION (owner), once the rules are known: whether and how to model the 1 July 2027
  split. Known shape of the inputs: a per-parcel **market value at 1 July 2027** to divide the gain
  (the closing-price table is the natural source); a CPI series that runs past September 1999
  (`cpi_quarters` is frozen at the 1999 freeze today); and the 30% minimum tax, which turns on the
  taxpayer's marginal rate — outside the data model, like the FITO offset limit
- [ ] Whatever is decided, make sure a closing price is stored for every held listing on the last
  trading day before 1 July 2027 (the scheduled price import plus `closing_price`'s held-days
  backfill), since a missing one cannot be fetched reliably years later

## Return-of-capital and rights entitlement compare the trade date with the record date (2026-10-10 review, financial correctness — medium)
`corporate_action/adjustments.rs` (~line 132, `registered_from < record_date`; the SQL at ~248) and the
C2 test in `reports/net_capital_gain.rs` (~595) decide entitlement on the trade date. Under ASX T+2
the ex-date is one business day before the record date, so a buy *on* the ex-date is wrongly
entitled (cost base reduced) and a sale on the ex-date is wrongly unentitled (its C2 gain missed).
The rights-issue "dated on or after the record date is ex-rights" cut-off has the same off-by-one.
The behaviour is documented today, but trades already carry `settlement_date`.
- [ ] Tests at the boundary: a buy and a sell on the ex-date (record date − 1 business day) and on
  the business day before it, for a ROC, a rights issue and the C2 check
- [ ] Decide on the entitlement date per registration (settlement date vs record date, or an explicit
  ex-date on the corporate action), implement it consistently across ROC, rights and C2, and update
  the Known-limitations entry

## Smaller financial-correctness inconsistencies (2026-10-10 review, financial correctness — low)
- [ ] E10 and G1 excess gains are translated to AUD differently (`reports/net_capital_gain.rs`
  ~640–650): the E10 excess converts wholly at the acquisition-month rate, the G1 excess uses the
  payment-date rate for the payment and the acquisition rate for the cost base. Make the two
  consistent (or document why they differ) with a non-AUD AMIT-fund test covering both
- [ ] "Gross assessable investment income" (`reports/tax_summary.rs` ~1097–1111) includes the
  franking credits inside trust 13C/AMMA franked lines but excludes company-dividend (11U) credits,
  so it is neither a cash nor an assessable total. Make it one or the other (or split it into two
  labelled totals), with a test pinning the figure

## The post-backup command has no timeout and holds the backup job's lock (2026-10-10 review, robustness)
`run_backup_command` (`src/infra/db.rs`) awaits `sh -c … .output()` with no bound, inside `run_job`'s
per-job lock. A hook that hangs (`scp` to an unreachable host, `rclone` waiting on a prompt) never
returns: the scheduled loop is stuck and every later `POST /jobs/backup` queues behind it, so no
backup is taken again until restart.
- [ ] Bound it with `tokio::time::timeout` + `kill_on_drop(true)` (a fixed generous default, or a
  config key), failing the run with a `BackupError::Command` naming the timeout; test with a
  `sleep` hook against a short test timeout
- [ ] Shell-quote the substituted `{BACKUP_FILE}` path so a database/backup directory containing
  spaces or shell metacharacters reaches the hook as one argument; test with such a path

## A backup is renamed into place without reaching stable storage (2026-10-10 review, data integrity)
`VACUUM INTO` does not fsync its output, and `backup` (`src/infra/db.rs` ~424–429) verifies the
staging file (from the page cache) then `std::fs::rename`s it with no `sync_all` on the file or its
directory. A power cut shortly after can leave a "verified" backup that never reached disk — the
case the staging-file design (SCENARIOS T-11) exists to rule out — and pruning may count it as a
keeper.
- [ ] `sync_all` the staging file before the rename and fsync the destination directory after it;
  a test that the backup path calls both (or a seam proving the order), and note it in SCENARIOS T-11

## A manual job is cancelled when its HTTP client disconnects (2026-10-10 review, robustness)
`POST /jobs/{name}` (`scheduler/http.rs` ~131) awaits `run_job` inline, so closing the tab or
Ctrl-C in curl drops the future between commits and leaves the `job_runs` row `running`.
- [ ] Run the job on a `tokio::spawn`ed task and await its handle, so a disconnect detaches from the
  run rather than cancelling it; test by dropping the request future mid-job and asserting the run
  finishes and records `finished`

## A panic in a scheduled job ends that job's schedule until restart (2026-10-10 review, robustness)
`schedule::spawn` spawns `run_entry` with no `catch_unwind`; `CatchPanicLayer` covers HTTP only, so
one panic ends the timer loop for that job.
- [ ] Catch a panicking run (`FutureExt::catch_unwind` / a `JoinHandle` per run), record it as a failed
  run with its message, release the job lock and keep the loop scheduling; test with a registry
  containing a job that panics once

## Price collection re-bases prices outside a transaction (2026-10-10 review, data integrity)
`closing_price/collection.rs` (~212) calls `db_rebase_listing_prices` on a bare pooled connection, so
its read and per-row `UPDATE`s autocommit one by one; the other three callers run it inside
`write_tx`. A failure part-way leaves a listing's prices in mixed bases until the next run.
- [ ] Run it inside `infra::db::write_tx`, and extend the deferred-`BEGIN`/write scan (or add a test)
  so an autocommit multi-statement write on a pooled connection is caught

## Server-side constraint failures are reported as the client's fault (2026-10-10 review, error handling)
`From<sqlx::Error> for ApiError` (`src/infra/http.rs` ~1019–1034) maps every CHECK/UNIQUE/NOT NULL
violation to a `422` worded as a bad request — including one raised by a server-owned write
(`report_snapshots`, `job_runs`), where it is a bug that should be a logged `500`.
- [ ] Classify violations from server-owned writes as `Internal` (e.g. those paths map their own
  errors before `?`, or the constraint's table decides); test that a constraint failure in a
  snapshot write answers `500` and logs, while a client write still answers `422`

## No anti-framing header (2026-10-10 review, security — low-medium)
Nothing sets `X-Frame-Options` or a CSP. With `[auth]` off (the default) a hostile page can frame the
app and trick a click on a delete or job button; the clicks are same-origin inside the frame, so
`request_guard`'s `Sec-Fetch-Site` check passes them.
- [ ] Add a response-header layer in `app::router`: `X-Frame-Options: DENY` and
  `Content-Security-Policy: frame-ancestors 'none'` (consider a full `default-src 'self'` CSP — the
  frontend loads nothing third-party; check the inline-SVG chart and theme script still work under
  it via `scripts/ui-smoke.sh`); test the headers on `/`, a JSON route and the login page

## Login lockout: proxied source read from the first `X-Forwarded-For` line, and no global ceiling (2026-10-10 review, security — low)
- [ ] `auth.rs` (~659) reads `headers.get("x-forwarded-for")`, the **first** header line, then its
  rightmost entry. A proxy that appends a separate XFF line leaves the client's forged line first,
  so a client can rotate its lockout bucket. Read the last line (`get_all(..).iter().last()`); test
  with two XFF lines from a trusted proxy
- [ ] The lockout is per IPv4 / IPv6 /64 only; an attacker with an IPv6 /48 has ~65k buckets and is
  bounded only by Argon2 throughput (~130 guesses/s at 4 concurrent verifies). Add a process-wide
  failure ceiling (e.g. N failures per hour slows or refuses every login), with a test, and
  document it beside the per-source lockout

## Bulk snapshot regeneration and the health banner repeat whole-table reads (2026-10-10 review, performance)
- [ ] `reports/snapshot.rs` (~953–962) calls `closing_price::db_held_listing_ids(pool, Some(date))`
  once per calendar day, each rebuilding `HeldTimeline` from every trade (~1,800 full reads for five
  years). Build the timeline once and ask it per day; test that the per-day held sets are unchanged
- [ ] The health report (`reports/health.rs` ~1689–1716), refreshed on every navigation, runs
  `load_market` (listing, rename history, holidays) and a full `closing_prices` date scan per held
  listing. Batch them into set-based reads; a test that the report's output is unchanged

## Architecture: domain↔entities cycle, mega-files, and conventions enforced by source scans (2026-10-10 review, maintainability)
Scored 6/10 — the weakest dimension. The operation modules are well factored, but:
- [x] `domain` imports `entities` (`cost_base.rs` uses `corporate_action::{RocEvent, SplitEvent,
  RolloverOrigin}`; `open_parcels`/`rollover`/`contemplated_disposal`/`deduction_destination` call
  entity db functions), so it is not a lower layer. Move the event types and their loaders into
  `domain` (or a repository layer) so `domain` depends on nothing in `entities`; a scan test pins it
  — done 2026-10-10: `corporate_action/adjustments.rs` → `domain/adjustments.rs` (re-exported from
  `corporate_action`); `reduction_for`/`db_cost_base_reduction_events` → `domain::cost_base`;
  `db_parcel_sales`/`db_sales_by_parcel` → `domain::open_parcels`; `amit_in_tax_year` →
  `domain::tax_year`; `deduction_destination` takes `debt_deduction: bool`
  (`ExpenseType::is_debt_deduction`); rollover's write half → new `entities::rollover`. Pinned by
  `domain::tests::domain_depends_on_nothing_in_entities` (`strip_test_modules` moved to
  `test_support` to share it)
- [x] Split `reports/health.rs` (~3,300 prod lines, ~25 checks) into a `health/` directory by check
  family, and fold the ~10 `Duplicate*` checks (`same_x_entry` + `db_duplicate_x` pairs) into one
  generic duplicate detector — the existing health tests unchanged are the proof
  — done 2026-10-10: `reports/health/` = `mod.rs` (report, freshness, `db_health`, route) over
  `jobs`/`prices`/`duplicates`/`trades`/`calendar`, tests moved verbatim to `health/tests.rs`
  (split committed alone first); the eight `duplicate_*` checks are one `DuplicateCheck` trait
  (candidate SQL, `same_entry`, report, newest-first order) run by `find_duplicates`, the three
  formerly SQL-grouped ones (trades/actions/AMMA) included — 2714 tests pass with the health
  tests unchanged. (The price-series pair check stays separate: it is a run walk, not a group)
- [x] `trade/db.rs` `write`: replace the seven per-provenance-column `is_some()` guards with one lookup
  over `TRADE_PROVENANCE`, and drop the duplicated "is this trade referenced?" `EXISTS` query
  — done 2026-10-10: each `TRADE_PROVENANCE` row carries its `edit_refusal` and `deleted_alone`
  policy, the upsert and delete guards read the row's one column via `provenance_column_sql`
  (so `every_trades_foreign_key_is_classified` makes a new link choose both), and the listing- and
  account-change freezes share `drawn_on`; the existing refusal tests pass unchanged
- [x] DECIDED 2026-10-10 (owner: all four): replace source-scanning conventions with compile-time ones where the
  type system can carry them — `utoipa-axum` (`#[utoipa::path]`/`OpenApiRouter`) instead of the
  hand-kept `ROUTES` table and its ~3,700 lines of Rust-parsing tests; a `ReadPool`/`WritePool` split
  instead of the deferred-`BEGIN` text scan; a `Currency` type instead of `String` + `== "AUD"`;
  id newtypes (`ListingId`, `TradeId`, …) instead of bare `i64`. Each is a large mechanical change —
  decide which (if any) are worth it
  - [x] `Currency` type in place of `String` + `== "AUD"` (do first: financial-correctness value)
    — done 2026-10-10 as `infra::currency::CurrencyCode` (the `currencies` entity already owns the
    name `Currency`): all 79 currency fields, the local currency reads and the currency-carrying
    error payloads; `is_aud()` is the one AUD test, and production code cannot compile a `== "AUD"`
    (test-only `PartialEq<&str>`); pinned by `infra::currency`'s tests and the type itself
  - [x] `ReadPool`/`WritePool` split, retiring the deferred-`BEGIN` text scan
    — done 2026-10-10, at the transaction rather than the pool (two pool types would have rethreaded
    ~940 `SqlitePool` mentions for the same guarantee): reads begin with `infra::db::read_tx`, a
    `ReadTx` under `PRAGMA query_only` that SQLite refuses to write through; `sqlx::Pool::begin` is
    in `clippy.toml`'s `disallowed-methods`; the pool's release hook resets `query_only`. The
    `DEFERRED_BEGIN_ALLOWED` scan is gone; pinned by `a_read_transaction_refuses_every_write`
  - [ ] Id newtypes (`ListingId`, `TradeId`, …) in place of bare `i64` — `ListingId` done 2026-10-10
    (`infra::id`, every listing-id field, param and listing-keyed map); `HoldingAccountId` and
    `TradeId` remain, the same way (`id_type!` + the compiler-driven pass)
  - [ ] `utoipa-axum` (`#[utoipa::path]`/`OpenApiRouter`) in place of the `ROUTES` table and its
    Rust-parsing tests
- [x] DECIDED 2026-10-10 (owner): keep them — N/A. prune `doc_checks.rs` phrase pins (~150 tests asserting exact prose,
  which break on rewording without protecting behaviour) down to the structural checks (link
  resolution, advisory-ignore expiry, QC headers)
  The pins are the passing test CLAUDE.md requires before a doc-only TODO item can be checked off;
  rewording costs a test edit, which is the intended friction

## Test-suite gaps (2026-10-10 review, tests)
- [ ] Add `proptest` for the invariants the suite currently pins with hand-picked cases: cost-base
  apportionment sums exactly (demerger/scrip percentages), parcel allocations conserve quantity,
  split re-basing round-trips, `tax_year_for` boundaries, AMIT coverage totals equal the statement
  amount across any transfer/sale sequence (would have caught the partial-transfer finding above)
- [ ] Replace the 11 fixed `sleep` grace periods in concurrency tests with a deterministic signal
  (e.g. wait until the competing task is observed blocked on the busy handler)
- [ ] Remove the 10 local `fn dec(s: &str)` copies in favour of `test_support::dec`
