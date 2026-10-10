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

## Test-suite gaps (2026-10-10 review, tests)
- [ ] Add `proptest` for the invariants the suite currently pins with hand-picked cases: cost-base
  apportionment sums exactly (demerger/scrip percentages), parcel allocations conserve quantity,
  split re-basing round-trips, `tax_year_for` boundaries, AMIT coverage totals equal the statement
  amount across any transfer/sale sequence (would have caught the partial-transfer finding above)
- [ ] Replace the 11 fixed `sleep` grace periods in concurrency tests with a deterministic signal
  (e.g. wait until the competing task is observed blocked on the busy handler)
- [ ] Remove the 10 local `fn dec(s: &str)` copies in favour of `test_support::dec`
