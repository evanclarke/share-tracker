# TODO

Items are only marked done when a passing test exists for them.

Completed and decided (out-of-scope / not-reproducible) sections are archived in [DONE.md](DONE.md) to keep this list focused on active work. When a section here is fully done, move it to DONE.md rather than leaving it — see CLAUDE.md.

The sections below come from the **2026-07-13 improvement review** (a whole-project pass for
operational and test-strategy gaps, as distinct from the 2026-07-12 programming/domain review whose
findings are all closed in DONE.md), except where a section's heading names another source
(e.g. REQUIREMENTS). Each section records one finding; sections land in DONE.md as they are fixed
or decided.

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
