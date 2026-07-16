# TODO

Items are only marked done when a passing test exists for them.

Completed and decided (out-of-scope / not-reproducible) sections are archived in [DONE.md](DONE.md) to keep this list focused on active work. When a section here is fully done, move it to DONE.md rather than leaving it — see CLAUDE.md.

The sections below come from the **2026-07-13 improvement review** (a whole-project pass for
operational and test-strategy gaps, as distinct from the 2026-07-12 programming/domain review whose
findings are all closed in DONE.md), except where a section's heading names another source
(e.g. REQUIREMENTS). Each section records one finding; sections land in DONE.md as they are fixed
or decided.

## Authentication if ever exposed beyond localhost (honourable mention)
Currently handled correctly: the server binds `127.0.0.1` by default and exposing it is an explicit
`--host` opt-in documented as unauthenticated. Only matters if the server is ever exposed.
- [ ] If/when exposure is wanted: add an auth layer (e.g. a bearer token/basic-auth middleware over the whole router) before recommending `--host 0.0.0.0` for anything but a trusted LAN; until then this section records the decision that localhost-only is the accepted posture

## Report snapshots: provisional FX, catch-up generation, self-healing prices (REQUIREMENTS 2026-07-16)
The daily `report-snapshot` job fails all month for non-AUD holdings (no ATO monthly rate until
after month end) and both the price-import and snapshot jobs only target "the latest" date, so a
missed day is a permanent series hole. Strategy: flag-and-true-up provisional FX instead of
failing, and bounded catch-up windows in both jobs so late inputs delay a snapshot instead of
losing it. See REQUIREMENTS.md 2026-07-16 for full context.
- [ ] Valuation-only FX fallback: a new explicit resolution mode in `infra/fx.rs` — when the valuation month has no imported rate, use the most recent earlier month's rate for that currency, at most 2 months back, else fail loudly as today. The result distinguishes a real-month rate from a fallback one (caller must know). Only valuation paths (snapshot generation, live-quote conversion) can invoke it — no tax calculation or FY report can reach a fallback rate
- [ ] Live-quote conversion (`fetch_live_aud_prices`) uses the fallback: an early-month USD holding is valued (annotated as provisional) instead of erroring with a missing-rate reason
- [ ] `report_snapshots.provisional` flag (additive migration, no data loss, staleness triggers unaffected): set iff any conversion in the generation run used a fallback-month rate; regeneration with all real rates clears it; semantics distinct from `stale` (facts changed)
- [ ] `provisional` surfaced in the list/get/series API responses and the web UI (snapshot list + series/graph mark provisional points, as they do stale)
- [ ] Snapshot job catch-up: each run generates every missing snapshot date in a bounded lookback window (from the last stored snapshot date, capped ~14 calendar days) up to the latest fully-valuable date, and regenerates stale or improvable-provisional snapshots in the window; a still-blocked date is skipped with its blocker surfaced (log + job failure detail) and retried on later runs
- [ ] Price-import lookback: `run_collection` re-attempts, per held listing, every trading day in the last ~7 trading days whose stored row is missing or errored — not just the latest complete trading day; ok rows are never re-fetched (idempotent), no schedule changes
- [ ] RBA-import true-up: after a successful FX import that added new (currency, month) rows — the weekly `rba-fx-import` job and the manual `POST /rba_fx_rates/import` both — provisional snapshots whose valuation now resolves with a real rate are regenerated in that same run
- [ ] "Regenerate all" (API endpoint + web UI button on the snapshots screen): regenerates every stored snapshot date across the series; per-date blockers reported, unblocked dates still regenerate; single-date generation semantics reused
- [ ] "Regenerate provisional" (API endpoint + web UI button): same shape, provisional snapshots only — the manual counterpart of the post-import true-up
- [ ] Docs: `docs/SCHEMA.md` (`provisional` column), `docs/API.md` (flag in responses, the two regeneration endpoints, Response codes), README features (provisional-then-finalised snapshots)

