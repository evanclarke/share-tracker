# TODO

Items are only marked done when a passing test exists for them.

Completed and decided (out-of-scope / not-reproducible) sections are archived in [DONE.md](DONE.md) to keep this list focused on active work. When a section here is fully done, move it to DONE.md rather than leaving it — see CLAUDE.md.

The sections below come from the **2026-07-13 improvement review** (a whole-project pass for
operational and test-strategy gaps, as distinct from the 2026-07-12 programming/domain review whose
findings are all closed in DONE.md), except where a section's heading names another source
(e.g. REQUIREMENTS). Each section records one finding; sections land in DONE.md as they are fixed
or decided.

## Proactive job-failure and data-staleness surfacing in the UI
A failing price import or RBA FX import is only visible if the Jobs page is opened; meanwhile
valuations silently go stale (yfinance is an unofficial API and will break eventually). `job_runs`
keeps only the last run per job (`scheduler::db_record_run` overwrites), so an intermittent failure
that later succeeds leaves no trace.
- [ ] Health/freshness endpoint: latest closing-price date, latest RBA FX rate month, and any job whose last run errored, in one read (report-style, single read transaction)
- [ ] Web UI banner/strip on the main views driven by that endpoint: show stale price/FX data (threshold-based, e.g. prices older than N business days) and any failed job, linking to the Jobs page
- [ ] Bounded per-job run history (e.g. last 20 runs per job, pruned in the same write) so flapping jobs are diagnosable; `GET /jobs` exposes it; migration extends/replaces the single-row `job_runs` shape without dropping data
- [ ] Tests: staleness thresholds (fresh vs stale), failed-job surfacing, history bound enforced, UI binding asserted in the served bundle
- [ ] Docs: SCHEMA.md for the run-history shape; API.md for the new endpoint(s)

## Frontend: executed tests for the pure JS helpers + CI smoke check
~3,100 lines of JS include hand-rolled BigInt decimal arithmetic (`roundDecimalStr`,
half-away-from-zero rounding, `decStrEq` in `web/util.js`) and the allocation editor — money-adjacent
logic — yet the UI test strategy only asserts strings appear in the served bundle;
`scripts/ui-check.sh` is manual-only.
- [ ] Unit tests for the pure helpers in `util.js` (rounding, thousands grouping, min-dp padding, decimal equality, numericDisplay kinds) runnable with `node --test` and no build step; include edge cases (negative values, dp increase/decrease, carry on round-up, zero)
- [ ] Run the JS unit tests in CI (one extra ci.yml step; document the required Node version)
- [ ] CI smoke test via `scripts/ui-check.sh` (or an equivalent headless check): server starts on a temp DB, key hash routes render without JS errors — catches a broken module route or load-time exception that string-presence tests can't
- [ ] Decide and record how the JS test files are excluded from the served-bundle route table (they must not become servable modules)

## Append-only audit trail for financial writes
Every entity is PUT-upsert-in-place and hard DELETE: an accidental edit to a historical Buy silently
changes prior-year cost bases and tax figures, with the weekly backup as the only recourse and no way
to notice it happened. Aligns with the ATO record-keeping guidance already mirrored
(`docs/ato/cgt-keeping-records-shares.md`).
- [ ] Trigger-maintained history tables recording the old row + timestamp + operation on UPDATE and DELETE for the financial fact tables (trades, sells, parcel allocations, income, AMMA statements, transfers, corporate actions, …) — enforced in the database per the data-integrity convention, so no write path can bypass it
- [ ] Read-only endpoint (and UI view) to inspect an entity row's history
- [ ] Decide retention (likely: keep forever; it's the audit trail) and whether reference-data tables (exchanges, listings, FX rates) are in scope — record the decision here
- [ ] Tests: an UPDATE and a DELETE each leave a history row with the prior values; history survives the entity's own 422-rejected writes unchanged; migration preserves existing data
- [ ] Docs: SCHEMA.md (history tables + Relationships), API.md (history endpoint)

## CI supply-chain checks
CI runs fmt/clippy/test but nothing watches dependencies, and the binary talks to the internet
(`reqwest`, `yfinance-rs`, `quick-xml`).
- [ ] `cargo audit` (or `cargo deny check advisories`) as a CI step failing on known RustSec advisories; document the local equivalent
- [ ] Dependabot (or Renovate) config for Cargo so security patches in the HTTP/TLS stack arrive without manual attention
- [ ] Decide how advisory failures with no upstream fix are handled (temporary ignore list with expiry + reason) — record the policy

## Split `trade.rs` non-test code (honourable mention)
`entities/trade.rs` carries ~1,180 lines of non-test code mixing the model, write-time invariants,
and handlers. Not a defect — a maintainability nice-to-have.
- [ ] Split into focused units (e.g. model + `db_*`, invariant validation, handlers/router) without changing behaviour or the module's public surface; existing tests keep passing unchanged (they are the behaviour lock)

## Authentication if ever exposed beyond localhost (honourable mention)
Currently handled correctly: the server binds `127.0.0.1` by default and exposing it is an explicit
`--host` opt-in documented as unauthenticated. Only matters if the server is ever exposed.
- [ ] If/when exposure is wanted: add an auth layer (e.g. a bearer token/basic-auth middleware over the whole router) before recommending `--host 0.0.0.0` for anything but a trusted LAN; until then this section records the decision that localhost-only is the accepted posture

## Lossless trade round-trip for GST-inclusive brokerage (REQUIREMENTS 2026-07-13)
Found scripting against the API during the 2026-07-13 crypto reconciliation: on a trade stored with
`brokerage_includes_gst` set, `GET /trades/:id` returns the stored ex-GST split (`brokerage` +
`gst_on_brokerage`) alongside the flag, but `PUT /trades/:id` with the flag set interprets
`brokerage` as the one GST-inclusive amount and re-splits it — so a faithful GET→edit→PUT
round-trip silently shrinks the brokerage by the GST each pass (0.99 stored → read back 0.90 +
0.09 → re-split 0.82 + 0.08), with no 422. The web form escapes only because `wireGstBrokerage`
recombines the pair before saving; every other API client hits silent data corruption.
- [ ] Decide and implement the lossless shape (design-open per REQUIREMENTS): either reads present `brokerage` as the same GST-inclusive amount the write path expects when the flag is set (updating the web form in the same step so it doesn't double-recombine), or the write path accepts the stored split pair as-is when supplied intact — either way the read/write asymmetry goes
- [ ] Cover both write paths that share `resolve_brokerage` — `PUT /trades/:id` and `PUT /sells/:id` — so a flagged Sell round-trips losslessly too
- [ ] Regression test: PUT a GST-inclusive trade, GET it, PUT the response body back verbatim, assert the stored `brokerage`/`gst_on_brokerage` are unchanged (and the same for a flagged Sell)
- [ ] Docs: `docs/API.md`'s GST-inclusive brokerage section states the round-trip semantics explicitly
