# TODO

Items are only marked done when a passing test exists for them.

Completed and decided (out-of-scope / not-reproducible) sections are archived in [DONE.md](DONE.md) to keep this list focused on active work. When a section here is fully done, move it to DONE.md rather than leaving it — see CLAUDE.md.

The sections below come from the **2026-07-13 improvement review** (a whole-project pass for
operational and test-strategy gaps, as distinct from the 2026-07-12 programming/domain review whose
findings are all closed in DONE.md), except where a section's heading names another source
(e.g. REQUIREMENTS). Each section records one finding; sections land in DONE.md as they are fixed
or decided.

## Portfolio overview performance panel — graph, date range, and period attribution (REQUIREMENTS 2026-07-25)
- [ ] Extract the stored-price valuation path into `src/reports/valuation.rs` (`stored_valuations`, `ListingValuation`, `ValuationError`); `snapshot::aud_prices_for` becomes a thin adapter over it with unchanged behaviour and passing existing snapshot tests
- [ ] Extract the shared cash-income formula into `src/domain/income_cash.rs` (`cash_income`); `reports::performance` and `entities::drp_reinvestment::reinvestable_cash` both call it, no duplicated formula
- [ ] Refactor `reports::performance`'s handler body into a callable `pub(crate)` function taking `(conn, as_of, prices)`, independent of HTTP/live-fetcher, with the existing handler unchanged in behaviour
- [ ] New report `src/reports/period_performance.rs`: `POST /portfolio/period-performance`, `(from, to]` window, capital/FX/income breakdown that sums exactly to the period return, per-holding contributions, per-currency FX, informational realised capital gain, `provisional` flag, 422 on `from >= to` or a blocked valuation
- [ ] Rust tests: additivity, cross-check against `reports::performance` and `snapshot::db_series`, AUD-only zero-FX, USD rate-change FX, holding opened/closed mid-window, split inside window, income-only period, provisional propagation, blocked-price and invalid-range 422s, null `total_return_pct` when opening value is zero; plus an API test via `oneshot`
- [ ] `src/web/chart.js`: move `svgEl`/`seriesChart` out of `app.js`; add `presetRange`/`sliceSeries`; register in `JS_MODULES` (`src/web.rs`); node tests in `chart.test.js` for every preset, clamping, and the FY boundary
- [ ] Portfolio Overview screen gains the performance panel (range presets + custom dates, summary stat grid, per-holding contributions via `filterableTable`, provisional marker); config-driven via a `performancePanel` key read generically in `viewReport`, not overview-specific code
- [ ] Snapshots screen drops the chart card and its `/report_snapshots/series` fetch; `config.js`'s snapshots `desc` no longer claims the time-series graph
- [ ] `web.rs` bundle assertions cover the panel view and the `/portfolio/period-performance` path
- [ ] Docs: `docs/API.md` (new endpoint, `(from, to]` + FX-attribution conventions, Known limitations entry, response codes), README features line, `src/infra/fx.rs` doc comment + CLAUDE.md's `resolve_valuation_rate` allowed-callers sentence updated for the new caller, CLAUDE.md web module list gains `chart.js`; a `doc_checks.rs` test pins the new Known-limitations text
- [ ] `scripts/fixtures/demo` has ≥ 2 snapshot dates so `scripts/ui-smoke.sh` exercises the chart/summary, not just the empty-series hint
- [ ] `cargo build`, `cargo test`, `cargo fmt --check`, `node --test 'src/web/*.test.js'` all clean; manual `/verify` pass on `#/r/overview` (presets, custom range, breakdown sums on screen)


## Authentication if ever exposed beyond localhost (honourable mention)
Currently handled correctly: the server binds `127.0.0.1` by default and exposing it is an explicit
`--host` opt-in documented as unauthenticated. Only matters if the server is ever exposed.
- [ ] If/when exposure is wanted: add an auth layer (e.g. a bearer token/basic-auth middleware over the whole router) before recommending `--host 0.0.0.0` for anything but a trusted LAN; until then this section records the decision that localhost-only is the accepted posture


