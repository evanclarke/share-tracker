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
