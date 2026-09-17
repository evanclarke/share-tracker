# TODO

Items are only marked done when a passing test exists for them.

This file holds only open / in-flight work. Completed and decided (out-of-scope / not-reproducible)
sections are archived in the topical `DONE/*.md` files, indexed by [DONE.md](DONE.md). When a
section here is fully done, move it into the matching `DONE/*.md` file rather than leaving it — see
CLAUDE.md.

A section records one finding, and its heading names where it came from — a REQUIREMENTS entry, a
[SCENARIOS.md](SCENARIOS.md) section, or a dated review pass.

**Open: the 2026-09-17 code review pass.** Its findings are the sections below, most-urgent first.
Both of its financial-correctness defects in the CGT arithmetic — the G1 excess's FX date and the
cost-base pipeline's single end-floor — were fixed on 2026-09-17 and moved to
[`DONE/reviews.md`](DONE/reviews.md); next are an availability panic and two write-time validation
holes, a packaging permission, tax-document/label inconsistencies, two frontend state bugs, and a
tail of low-severity concurrency, hygiene, documentation and test-gap items. The pass verified the
three gates green (`cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`, `cargo test`
2389 passed / 6.09 s, `node --test 'src/web/*.test.js'` 149 passed), verified the documented
`GET` → edit → `PUT` round trip byte-exact end to end, and probed the running server against a
throwaway database (~120 requests) — the write-time invariant layer, the error bodies, the body
limit, and 60-way concurrent read/write (30/30 `200`, 30/30 `204`, zero `SQLITE_BUSY`) all held. Its
findings are therefore all in corners the existing suite cannot reach, not in the surfaces it
already pins.

Before this pass, every section recorded here was closed and archived — the last was the annual tax
report's version stamp, and before it its foreign income totals (both REQUIREMENTS entries, closed
2026-08-28 and moved to [`DONE/reporting.md`](DONE/reporting.md)), and before it the 2026-08-28
cyclomatic complexity audit, whose six items (the `tax_summary` split, the two nesting outliers, the
`rights_sale` anchoring walk, the `corporate_action` presence flags, the `upsert_sell_in_tx`
parameters struct, and the decision not to gate complexity in CI) closed on 2026-08-28 and moved to
[`DONE/reviews.md`](DONE/reviews.md). The closing narrative that used to stand here — the
pass-by-pass record of driving SCENARIOS.md sections S through AA, and the last two sections to
close before that audit (the distribution calendar and the 2026-08-25 code review) — was moved to
[`DONE/verification-passes.md`](DONE/verification-passes.md) on 2026-08-28. The maintained record of
what has been verified is SCENARIOS.md's
[Verification status](SCENARIOS.md#verification-status) table and its per-section findings blocks;
the maintained record of what was built and decided is the `DONE/*.md` archive.

## The outbound reference-data fetches have no timeout or response-size cap (2026-09-17 review, security)

(2026-09-17 review's security pass. Three sibling feed fetches build a client with no timeout and
read the body unbounded; a stalled or very large response parks the request task and buffers the
whole body.)

- [ ] Reproduced by reading `src/entities/currencies.rs:509-523` (`reqwest::Client::new()` then
  `resp.text()`), `src/entities/mic_registry.rs:211-218` and `src/entities/rba_fx_rate.rs:372-379`
  (both `reqwest::get` then `resp.text()`); a grep for `.timeout(` finds no request timeout anywhere
  in the tree
- [ ] Reachable pre-auth when `[auth]` is unset, through the documented manual trigger
  (`POST /{currencies,mic_registry,rba_fx_rates}/import` with an empty body)
- [ ] Mitigating facts, verified: the URLs are `const` (no user-supplied URL, so no SSRF target
  control), TLS is reqwest's default rustls with `rustls-platform-verifier` (no
  `danger_accept_invalid_certs`), and this tree's reqwest has no compression features enabled, so
  there is no decompression-bomb path
- [ ] Fix: one shared `reqwest::Client::builder().timeout(..).build()` per fetch, with a
  size-bounded body read
- [ ] Tests: a fetch against a stalled/bounded stub asserting the timeout path surfaces as the
  documented `502`
- [ ] Docs sync: none

## A missing FX rate answers an empty 500 on three paths where the same data answers 422 elsewhere, and one tax-report path converts at parity (2026-09-17 review, financial correctness)

(2026-09-17 review. The `ApiError::from(sqlx::Error)` arm recovers the boxed `FxError` from a decode
error to answer a documented `422` naming the currency and month; three callers stringify the error
first and lose it, and one converts rather than failing.)

- [ ] Reproduced by reading: `src/reports/period_performance.rs:161-165`,
  `src/reports/snapshot.rs:248-252` and `src/reports/valuation.rs:47-51` stringify `sqlx::Error`, so
  the `FxError` that `infra::fx` carries through `sqlx::Error::Decode` and that `ApiError::from`
  downcasts (`fx.rs:95-102`, `http.rs:601-619`) never reaches the classifier. The same missing
  (currency, month) answers `422` from `/portfolio/performance` and an empty-bodied `500` from
  `/portfolio/period-performance` — against SCENARIOS M-04's intent
- [ ] `src/reports/tax_report.rs:739-742` does `.unwrap_or(Decimal::ONE)` on a required rate, so a
  failed resolution silently converts at parity and `:769-771` then divides the itemised adjustments
  by it. The comment calls it unreachable, but the same read's other failures are swallowed too
  (`.ok()` at `:745`, `.unwrap_or(p.cost_base)` at `:765`, `.unwrap_or_default()` at `:730`), so the
  claim is not enforced
- [ ] Fix: propagate the `FxError` rather than stringifying (the report enums whose `Db` arm is a
  `String` need to keep the source recoverable), and replace the parity fallback with a propagated
  error
- [ ] Tests: one missing-rate case asserted to answer the same `422` body on all four endpoints, and
  a tax-report case with an unimported rate asserting a failure rather than a parity figure
- [ ] Docs sync: `docs/API.md`'s Response codes section if any status changes

## AMMA and E10 financial-year buckets use `.year()` rather than `tax_year_for` (2026-09-17 review, financial correctness)

(2026-09-17 review. `domain::tax_year.rs`'s `tax_year_for` is documented as *the* Australian
financial-year bucketing rule and is the only `month() >= 7` in `src` — but a handful of AMMA/E10
buckets take the calendar year of a June date directly, which is equivalent only because
`amma::db_upsert` forces a 30 June year end.)

- [ ] Reproduced by reading `src/reports/net_capital_gain.rs:626` and `:764`,
  `src/reports/tax_summary.rs:821`, `src/reports/franking.rs:396`, `src/reports/activity.rs:446`
- [ ] The equivalence rests on a write-time check, not on the type: a hand-entered or imported row
  with a different year end would file those gains one FY early, while G1/C2 and realised gains —
  which do use `tax_year_for` — file the same facts in the other year
- [ ] Fix: use `tax_year_for` at each site (it returns the calendar year of the 30 June end, so a
  30-June input is unchanged), or state the invariant in the type by storing a `TaxYear` rather than
  a date wherever a statement year is used
- [ ] Tests: an AMMA row with a non-30-June year end (written directly at the DB level, since the
  write path refuses it) asserting the bucket `tax_year_for` gives, pinning the two paths together
- [ ] Docs sync: none

## The snapshot series plots zero for an excluded holding and clears its flag (2026-09-17 review, financial correctness)

(2026-09-17 review. A holding excluded from a date's valuation — no obtainable price, or before its
`unpriced_before` — is supposed to be a *gap* in the series, and the doc comment says so; one
builder emits zero instead.)

- [ ] Reproduced by reading `src/reports/snapshot.rs:393-401`: `market_value.unwrap_or(Decimal::ZERO)`
  with `holding_excluded = false`. An excluded listing still has a stored row (`market_value` null,
  `price_unavailable` set), so the `rows.is_empty()` check does not catch it, and the Listing
  Activity graph draws a fall to zero for a value that is merely unknown. `db_holding_series`
  (`:462-464`) already does it correctly
- [ ] The full series has the same shape: the excluded holding's cost base is folded into
  `total_cost_base` while its market value is omitted, so the reported total mixes a cost in with no
  matching value
- [ ] Fix: skip the row (leaving the gap the doc promises) or emit it with `holding_excluded` set,
  matching `db_holding_series`; exclude its cost base from the total
- [ ] Tests: an excluded holding in a window asserting a gap (not a zero) and a total that omits both
  its value and its cost base
- [ ] Docs sync: `docs/API.md`'s report snapshots section if the emitted shape changes

## The annual report's worksheet ignores the scrip-cash apportionment, and the E10/G1 walk divides per unit (2026-09-17 review, financial correctness)

(2026-09-17 review; two presentation/precision defects in the tax-report path, neither reaching an
ATO label.)

- [ ] Reproduced by reading `src/reports/tax_report.rs:743-776` against `:851`: the worksheet's
  `initial_cost_base_aud` and `adjustments` come from the raw pipeline while
  `adjusted_cost_base_aud` is the realised-gains figure with the partial-rollover scrip cash
  apportioned (`reports/realised_gains.rs:523-527`). A scrip exchange with a cash component therefore
  prints `Initial 10,000 − adjustments 0 = Adjusted 2,000`, with 8,000 unexplained — contradicting
  the identity the same module documents at `:468-476`. The gain and 18A are unaffected
- [ ] Reproduced by reading `src/reports/net_capital_gain.rs:564`: the E10/G1 walk re-implements step
  1 as `initial_cost() / trade_qty` × units where the shared pipeline multiplies first
  (`domain/cost_base.rs:754-758`), which differs in the last place; the `amount <= remaining` floor
  decision is then taken on the slightly different figure. It is also a second implementation of the
  pipeline the project's rules say must not be re-implemented
- [ ] Fix: derive the worksheet's initial and adjustment figures from the same apportioned source the
  adjusted figure uses, so the printed identity holds; and have the E10/G1 walk call the pipeline's
  own pro-rating helper rather than dividing per unit
- [ ] Tests: a partial-rollover scrip exchange with cash asserting
  `initial − Σ adjustments = adjusted` on the printed rows; a case where divide-first and
  multiply-first differ in the last place asserting the pipeline's answer
- [ ] Docs sync: `docs/API.md`'s annual tax report section if any printed column changes

## Numeric table sorting and two money tests coerce decimal strings through `Number()` (2026-09-17 review, web frontend)

(2026-09-17 review. The frontend's exact decimal arithmetic is BigInt-on-string throughout, and this
is the one place a money/quantity value leaves it.)

- [ ] Reproduced by reading `src/web/app.js:226`: `cmp = Number(av) - Number(bv)` for a numeric
  column. Past ~15 significant digits two long 8-dp quantities compare equal and the sort falls back
  to server order — a wrong order the user cannot correct by re-sorting
- [ ] `src/web/taxreport.js:307` (`Number(r.conduit_foreign_income_aud) !== 0`) and `:345`
  (`Number(line.value) === 0`) test money for zero through a float. Not currently wrong (a float
  zero-test only misfires below ~1e-308), but it is the same escape the rule forbids
- [ ] Fix: compare through the existing exact helpers (`decParts`/BigInt-scaled strings) and test
  zero with `decStrEq(v, '0')`; add a sort comparator unit test in `src/web/*.test.js`
- [ ] Tests: a `src/web/app.js`-adjacent unit test (or an extracted pure comparator) asserting two
  long 8-dp values that differ in the last digits order correctly
- [ ] Docs sync: none

## Bespoke form labels are not associated with their controls (2026-09-17 review, web frontend)

(2026-09-17 review. `buildFieldInput` associates its label and control correctly; the hand-built
forms do not, so those controls have no accessible name and clicking the label does nothing.)

- [ ] Reproduced by reading `src/web/forms.js:419-420` (the allocation parcel/quantity rows) and
  `src/web/app.js:1774-1776` (Backfill listing/from/to), `:1816-1820` (Manual price), `:1857` (Clear
  superseded), `:1988` (snapshot date), `:2878-2879` (as-of date), `:2913-2916` (price-override
  inputs)
- [ ] Fix: give each control an `id` and its label a matching `for` (or nest the control inside the
  label), as `buildFieldInput` already does
- [ ] Tests: a served-bundle assertion that each hand-built form's labels carry a `for` naming an
  input the same view creates — or extract the label/control pairing into a shared helper and unit
  test that
- [ ] Docs sync: none

## `filterableTable` re-filters and re-sorts the whole unpaginated set on every interaction (2026-09-17 review, web frontend)

(2026-09-17 review, measured. Every data table renders through `filterableTable`, which sorts and
filters the entire result set on the main thread.)

- [ ] Reproduced by reading `src/web/app.js:214-233` (called from `renderBody` at `:245-247`), with
  `numeric` recomputed by a `rows.some` per column at `:100-103`, and measured on this machine: a
  40k-row string-column sort is ~51 ms and a numeric one ~25 ms, per filter keystroke and per pager
  click. The Closing Prices list returns every stored price row (20 listings × 5 years ≈ 25k rows at
  the time of measurement), so typing in its filter visibly stutters
- [ ] Fix: debounce the filter input and/or cache the sorted view keyed by (column, direction,
  filter); the pager needs no re-sort at all
- [ ] Tests: a `src/web/*.test.js` unit test on whichever caching/derivation is extracted (the
  project's existing pattern for pure frontend helpers)
- [ ] Docs sync: none

## Frontend robustness nits: no loading state, an unguarded decode, an undisconnected observer, an unencoded query value, and a duplicated helper (2026-09-17 review, web frontend)

(2026-09-17 review; small, independent fixes in `src/web/`, grouped because each is a few lines.
The pass verified no XSS path exists — every `innerHTML` assignment is a clear, `el()`'s `children`
goes through `append` (which never parses markup), and the one unescaped sink is unused.)

- [ ] `src/web/app.js:2771-2773` awaits a report `GET` before painting anything, so `#app` is blank
  (topbar only) until it lands, with no spinner and no error state. The overview deliberately paints
  its shell first; nothing else does. Fix: a shared pending/error state in the report view
- [ ] `src/web/app.js:2827` calls `decodeURIComponent(args[i])` unguarded, so a hand-edited `%` in
  the hash turns the whole screen into a `URI malformed` error via `render()`'s catch. Fix: a safe
  decode helper
- [ ] `src/web/app.js:2391-2395` creates a `ResizeObserver` per panel render and never disconnects
  it. Not a true leak (the observed holder is discarded, so the cycle is collectable), but it is the
  only listener in the app with no teardown. Fix: hold it and `disconnect()` when the view is
  replaced
- [ ] `src/web/app.js:1090` interpolates `ownerField` from the hash into the attachments query
  string unencoded, so a hand-edited hash can add query parameters (the server refuses `422`; no
  security impact). Fix: `encodeURIComponent(ownerField)`
- [ ] `moneyEl` is duplicated verbatim (`src/web/app.js:2177-2180` and
  `src/web/taxreport.js:28-31`) — identical today, and unshared, unlike `moneyText`. Fix: one
  `moneyEl` in `util.js` beside `moneyText`
- [ ] `src/web/chart.js:293-297` uses `setUTCMonth`, so a "1M" preset from the 31st overshoots
  (31 Mar − 1 month → 2 Mar, not 29 Feb); a preset can therefore start a few days late on month-end
  dates. Fix: clamp to the target month's last day
- [ ] `src/web/app.js:437` dereferences `entity.keyFields`/`entity.fields` in the empty-rows
  fallback, absent on a columns-less entity, so a new readonly entity without `columns` and with an
  empty table would throw a `TypeError` instead of "No records yet." (Unreachable today — the only
  four columns-less entities are `custom` and redirected at `:2963`.) Fix: guard the fallback
- [ ] `src/web/util.js:16`'s `html:` attribute is the one unescaped HTML entry point in the helper
  and has **no call sites** anywhere in the bundle. Fix: delete it, or comment that it is unused —
  removing the only latent XSS footgun
- [ ] Tests: a `src/web/*.test.js` unit test for whichever of these is extracted as a pure helper
  (the month clamp and the safe decode are the natural ones); the rest are served-bundle assertions
  in the `web.rs` style
- [ ] Docs sync: none

## Documentation drift found by the 2026-09-17 pass (2026-09-17 review, docs)

(2026-09-17 review, verified by reading each file. The project's rule is that a user-visible or
structural change updates its documentation in the same task; these four were missed.)

- [ ] `CLAUDE.md`'s `src/infra/` module map lists `args`, `db`, `logging`, `decimal`, `fx`, `http` and
  `scheduler`, but omits `auth.rs` (+ `auth/`), `config.rs`, `date.rs`, `email.rs` and `fetch.rs` —
  including the whole authentication subsystem and the outbound-email subsystem added in v0.23.0. A
  reader of the module map would not know they exist
- [ ] `CLAUDE.md`'s `cargo test` bullet says "~1955 tests, ~4s as of 2026-08-22"; the suite is now
  2389 tests in 6.09 s. (The three build settings it names are all still in effect, re-verified: the
  `.cargo/config.toml` SQLite flag, the dev-profile dependency opt-level, and the cached test schema)
- [ ] `src/reports/row_history.rs:37-42` says "Five joined later" and lists five tables; the live
  audited set is 23 and includes `distribution_events` (migration 0048), the sixth joiner. The
  `AUDITED_TABLES` const, the migration CHECK, the triggers and the UI picker are all correct — only
  the comment is stale
- [ ] `migrations/0045_autoincrement_audited_ids.sql:25-30` still claims nine call sites compute
  `SELECT COALESCE(MAX(id), 0) + 1` and that reworking them "is still open (TODO, SCENARIOS U-a)".
  That is no longer true: the only occurrence of `COALESCE(MAX(id` left in `src/` is the SCHEMA.md
  quotation in `doc_checks.rs:424`, server-created rows omit the id and read
  `last_insert_rowid()`, and `a_server_assigned_insert_never_takes_a_deleted_trades_id` plus
  `every_audited_tables_id_is_autoincrement` pin it. Correct the comment to state the work is done
- [ ] Tests: none needed for a pure comment/doc correction, except that the `CLAUDE.md` edits are
  verified by eye (nothing in `doc_checks` reads it) — if any of these becomes a pinned requirement,
  add the assertion then
- [ ] Docs sync: the files above

## Two integrity assertions are missing: no data-preservation test for the table-rebuild migrations, and no read-back of `PRAGMA foreign_keys` (2026-09-17 review, test gaps)

(2026-09-17 review, from the schema replay. Both are safety nets for invariants the project relies
on but does not currently assert; the review verified the *current* state is correct in both cases,
so these are regressions waiting to happen rather than existing defects.)

- [ ] `pool_migrated_below(N)` data-preservation tests exist for migrations 20/21/25/34/38/39/40/47
  (`src/infra/db.rs`), but none for the two full table rebuilds, 0029 and 0045.
  `migrations_do_not_drop_tables_or_columns` only forbids `DROP COLUMN` and a non-`_old` `DROP
  TABLE`, so it cannot see a column silently omitted from a rename pattern's new `CREATE TABLE` +
  `INSERT … SELECT`. The review replayed 0001–0044 and diffed against the final schema: no column and
  no table was lost, and only `corporate_actions.renounceable` (0047) was added. Fix: add
  `migration_0045_…`/`migration_0029_…` row-and-column-count tests in the style of
  `migration_0039_keeps_every_holiday_and_audits_the_calendar`
- [ ] `src/infra/db.rs`'s `.foreign_keys(true)` is the only thing making every FK constraint real —
  the review verified it holds on the replayed schema and both real databases (`foreign_key_check`
  clean) — but nothing reads `PRAGMA foreign_keys` back on a pooled connection, unlike the sibling
  `the_chosen_busy_timeout_is_in_force_on_every_connection`. A one-line assertion matching that
  precedent is cheap
- [ ] Note (not a defect): 0029 and 0045 correctly carry `-- no-transaction` and
  `PRAGMA foreign_keys = OFF`, which is load-bearing — with FKs on, `RENAME TO x_old` rewrites other
  tables' FK clauses to point at `x_old`, and `PRAGMA foreign_keys` is a no-op inside a transaction.
  `src/reports/row_history.rs:2150-2156` pins it for 0029
- [ ] Tests: the two preservation tests and the pragma read-back above
- [ ] Docs sync: none

## `base_path` accepts `.` and `..` segments, producing a prefix no browser can reach (2026-09-17 review, config)

(2026-09-17 review, reproduced against a running server. `normalise_base_path` validates each
segment's characters but does not exclude the two relative segments those characters permit.)

- [ ] Reproduced: `--base-path '/..'` starts the server cleanly (no error, no warning) and serves the
  application at `/..` — reachable with a raw request (`curl --path-as-is`) but not from a browser or
  a proxy, both of which normalise `/..` to `/`, so `GET /` answers `404`. An operator who types it
  gets a server that reports healthy and serves nothing. `"."` behaves the same way
- [ ] This is the case the function's own neighbours argue against: `Auth::new` and
  `normalise_base_path`'s siblings abort startup rather than "serving a login page that can never
  succeed"
- [ ] Fix: reject a segment of `.` or `..` in `normalise_base_path` with the existing error shape
- [ ] Tests: `normalise_base_path` unit cases for `.`, `..` and a `..` inside an otherwise valid
  prefix, each an `Err`
- [ ] Docs sync: none

## A char-boundary slice in `hex_decode` could panic on non-ASCII input (unreachable from HTTP) (2026-09-17 review, nit)

(2026-09-17 review. Defensive only: the review verified the function is not reachable with non-ASCII
input, so this is hardening rather than a live bug.)

- [ ] `src/infra/auth.rs:238-246` slices `&s[i..i + 2]` after checking only that the *byte* length is
  even, so a non-ASCII string of even byte length (an emoji is four bytes) would panic on a char
  boundary
- [ ] Verified unreachable from HTTP: `session_cookie` (`:306-312`) goes through
  `HeaderValue::to_str()`, which rejects every non-visible-ASCII byte, and the only other caller is a
  test. So there is no reproducible defect to fix first
- [ ] Fix: an `s.is_ascii()` guard (or decode bytewise) so the invariant is local rather than
  dependent on a caller two modules away
- [ ] Tests: a unit case passing a non-ASCII even-byte string asserting an error rather than a panic
- [ ] Docs sync: none
