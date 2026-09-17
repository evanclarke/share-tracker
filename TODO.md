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

## Reference-data writes accept a blank ticker, a malformed `close_time`, and an unknown exchange timezone (2026-09-17 review, integrity)

(2026-09-17 review, reproduced against a running server. Neither `listing::db_upsert` nor
`exchange::db_upsert` performs any write-time validation; the only constraints are the DB's own
(`ticker TEXT NOT NULL`, which the empty string satisfies). This is the same root cause as the
`settlement_days` panic above, split out because these three are silent data defects rather than a
crash. It also runs against the project's own rule that invariants are enforced at write time and
that blank-free text fields are checked — `income`, `investment_expense` and a manual closing price
all refuse a blank or negative value in the same situation.)

- [ ] Reproduced: `PUT /listings/:id` with `ticker: ""` returns `204` and stores it; so does
  `POST /listings/:id/rename` (returning `201` and recording a blank `new_ticker` in the audited
  rename chain) — two doors, not one. Blank `name` and `isin` are likewise accepted
- [ ] The impact is not cosmetic: an empty ticker resolves the provider symbol to `.AX`, and the
  stored price row then reports `the symbol may be wrong, renamed, or delisted; set price_symbol on
  the listing or backfill with an explicit symbol` — a diagnosis that sends the reader hunting for a
  rename that never happened, while the real cause is the blank ticker. Nothing rejects it, `health`
  does not flag it, and it is not a documented limitation
- [ ] Reproduced: `PUT /exchanges/XASX` with `close_time: "nonsense"` or `"99:99"` returns `204`
  (the docs call the field `HH:MM` local), and `timezone: "Mars/Olympus"` returns `204` — a field
  that drives market-close logic, where a bad value is only discovered downstream
- [ ] Note the web edit form masks the ticker case (HTML `required` plus `readFieldValue`'s trim →
  the server sees a missing field and `422`s), but the API is the documented interface, and the
  rename path is reachable from it
- [ ] Fix: a blank check on `ticker`/`name` in `listing::db_upsert` and in the rename path's own
  validation; an `HH:MM` range check on `close_time` and an IANA parse on `timezone` in
  `exchange::db_upsert`; each refusing `422` naming the field, as the sibling entities already do
- [ ] Tests: the two blank-ticker doors refused `422`; `close_time` and `timezone` refused `422`;
  a valid write of each still `204`
- [ ] Docs sync: `docs/API.md`'s Listings and Exchanges sections (the 422 catalogues) and, if the
  blank-ticker case stays permitted anywhere, the Known limitations list

## The config file holding every secret is installed world-readable (2026-09-17 review, security)

(2026-09-17 review, verified by reading the packaging scripts and the auth code. Earlier review
passes covered CI pinning and the argv/secret rules; this one covers the file the secrets actually
live in.)

- [ ] Reproduced: `pkg/freebsd/build-pkg.sh:33` installs the sample
  `-m 0644 pkg/freebsd/share-tracker.toml.sample`, and the post-install in
  `pkg/freebsd/manifest.ucl` copies it to the live `/usr/local/etc/share-tracker.toml` with `cp -p`,
  preserving `0644`. Nothing in the config reader (`src/infra/config.rs`) checks or tightens the
  mode, and neither the README's Authentication section nor `docs/API.md` mentions permissions
- [ ] What that exposes is not just the SMTP password: `[auth].api_token` is full read/write API
  access, and `[auth].password_hash` is the *input* to the session-signing key —
  `Auth::new` computes `derive_signing_key(&password_hash)`
  (`src/infra/auth.rs:110`, `:215-220`), an HMAC-SHA256 keyed by the PHC string itself. Any local
  user who reads the file can therefore mint a valid `st_session` cookie for any expiry without ever
  knowing the password, and can read the database and the backups beside it (created at the process
  umask — `0644` at the conventional `022`; `/var/db/share-tracker` is created by a bare `mkdir -p`,
  so `0755`)
- [ ] This is inconsistent with the project's own care elsewhere: the log file is installed `640`
  (`pkg/freebsd/newsyslog.conf`), and `--auth-*`/`--email-*` CLI flags are refused precisely because
  "a secret on the command line is visible to anyone on the host via `ps`" (`src/infra/args.rs`,
  README Authentication)
- [ ] Fix: `install -m 0600` for the sample, an explicit `chmod 600` on the live file in post-install
  when it exists, `chmod 700 /var/db/share-tracker` (or a documented `umask 077` in the rc script),
  and a startup `WARN` in `config::read` when `metadata.permissions().mode() & 0o077 != 0`
- [ ] Tests: a unit test on the mode-warning helper (the read path's warning is the testable half);
  the packaging half is verified by inspection, as the other packaging claims in `doc_checks` are
- [ ] Docs sync: a permissions note in the README's Configuration file / Authentication sections and
  in `docs/API.md`'s Authentication section

## The closing-price delete guard is read outside its transaction, and the table has no DELETE staleness trigger (2026-09-17 review, integrity)

(2026-09-17 review, reproduced by reading the handler and the migration set. The handler reads the
row and the listing's marker on the pool, decides, then deletes in a separate statement;
`closing_prices` carries only `closing_prices_stale_snapshots_update`, with no DELETE counterpart —
verified across all 49 migrations — though its two *audit* triggers do have both variants, so the
asymmetry is specific to staleness.)

- [ ] Reproduced by statement sequence: `delete_one`
  (`src/entities/closing_price/http.rs:312-330`) calls `db_get_one(&pool, …)` (`:312`) and
  `listing::db_get(&pool, …)` (`:315`) — a different connection from the one the delete will use —
  then decides at `:319` and calls `db_delete(&pool, …)` (`:329`), which is an unguarded
  single-statement `DELETE` (`src/entities/closing_price/db.rs:429-440`)
- [ ] Failure: the row is read as `status = 'error'` so the guard approves; a concurrent manual `PUT`
  or re-fetch stores an **ok** price for the same `(listing_id, price_date)` through `db_store`'s
  upsert; the delete then removes a price that snapshots were valued at. Nothing stales them,
  because there is no DELETE staleness trigger, so the snapshot keeps `stale = 0` and keeps a figure
  derived from a price row that no longer exists — permanently mis-flagged. The `unpriced_before`
  branch has the same window (the marker can be cleared between the read and the delete)
- [ ] The sequential behaviour *is* pinned (`src/entities/closing_price/tests/delete.rs`); the race
  is not
- [ ] Fix: run the handler in `infra::db::write_tx` and re-read the row and marker on that
  connection, or push the guard into the `DELETE` itself
  (`… AND (status <> 'ok' OR price_date < (SELECT unpriced_before FROM listings WHERE id = ?))`).
  `db_delete` has exactly one caller, so either shape is contained
- [ ] Tests: a concurrent test in the style of
  `reports::open_parcels::tests::a_read_never_sees_half_of_a_multi_parcel_sell`, driving a manual
  price write between the guard read and the delete
- [ ] Docs sync: none

## Negative AMMA components are accepted, producing a negative FITO and a fictitious carried-forward loss (2026-09-17 review, financial correctness)

(2026-09-17 review. `amma::db_upsert` (`src/entities/amma.rs:250-269`) validates the 30-June year
end and the FITO-needs-gains pairing, but no component's sign, and there is no CHECK in the schema.
The sibling income entities refuse negatives explicitly — `income.rs` ("`unfranked_amount` cannot be
negative — income figures are the statement's own positive (or zero) amounts"),
`interest_income.rs`, `investment_expense.rs` — so this is an asymmetry, not a deliberate liberty.)

- [ ] Reproduced by reading the arithmetic: with `cgt_discount_gains = 5000`,
  `cgt_other_gains = −6000` and `foreign_tax_credits_capital_gains = 100`,
  `tax_summary.rs:455-468` computes a negative `claimable` FITO (assessable −1000, grossed up 4000)
  and `:863` adds it, so the year's 20O is −25 and
  `foreign_tax_offsets_cgt_discount_reduction` exceeds the tax actually paid — the de-minimis cap at
  `:1044` can never fire on a negative
- [ ] Reproduced by reading the arithmetic: `cgt_other_gains = −100` alone leaves `net_other = 0` but
  makes `capital_loss_carried_forward` **+100** (`net_capital_gain.rs:889-896`), so the next year's
  real $100 gain is netted to zero and 18A understated
- [ ] Fix: refuse a negative AMMA component (and a negative `cost_base_adjustment`'s counterpart
  components, which the ATO mirror also treats as attribution amounts) at write time with `422`,
  naming the field, as the sibling income entities do
- [ ] Tests: each AMMA component refused `422` when negative; a positive/zero statement still `204`;
  a report-level assertion that `capital_loss_carried_forward` can never be positive
- [ ] Docs sync: `docs/API.md`'s AMMA statements section and its 422 catalogue

## The figure labelled 13C excludes the attached franking credits its label includes (2026-09-17 review, financial correctness)

(2026-09-17 review, verified against the project's own ATO mirror. The label map in
`src/reports/tax_summary.rs:329/333` labels `trust_franked_distributions` and
`amma_franked_dividends` as 13C, but the accumulators at `:765` and `:845` add only the franked
distribution — the attached credits are accumulated separately into `franking_credits` (11U/13Q).
`docs/ato/tax-return-labels-2026.md:66` defines 13C as "Franked distributions from trusts,
**including** the share of attached franking credits".)

- [ ] Reproduced by reading the mapping: a trust franked distribution of $700 with $300 of attached
  credits reports 13C = 700 (it should be 1,000) while 13Q correctly reports 300. The figure reaches
  the tax-summary CSV's ATO-label row and the annual tax report's printed tax-summary section
  (`src/reports/tax_report.rs:1809-1824`), both of which a return is transcribed from
- [ ] Fix: either add the attached credits to the two 13C accumulators (keeping 13Q as the offset
  entitlement, which the mirror notes "may differ from the grossed-up credit inside 13C where trust
  deductions were allocated to it"), or relabel the column — the two must agree
- [ ] Tests: a trust year with franked distributions and attached credits asserting the 13C figure
  the ATO label implies, plus a case where a trust deduction makes 13Q differ from the credit inside
  13C
- [ ] Docs sync: `docs/API.md`'s tax summary section for whichever resolution is chosen

## The annual report's sections do not reconcile to the tax summary (2026-09-17 review, financial correctness)

(2026-09-17 review. `reports::tax_report` documents that its rows sum to their tax-summary lines and
pins that with a test; three columns escape it. Each is a separate one-line fix, grouped here
because they share one contract.)

- [ ] Reproduced: the trust-income rows print the franking credits the summary *denies* — the
  dividend branch subtracts `credits_denied` at `src/reports/tax_report.rs:1442` while the trust
  branch passes `fc` straight through at `:1423`. The 45-day walk and its denial cover trust rows too
  (`reports/franking.rs:323-384` has no trust filter; the summary accumulates their credits at
  `:771` and subtracts the denial at `:1032`), and `TrustIncomeRow` (`:964-981`) has no
  status/denied column. A $6,000-credit trust distribution with a disqualifying sale prints 6,000
  against a 13Q of 0
- [ ] Reproduced: the foreign-income total omits the AMMA capital-gains foreign tax that 20O
  includes — `tax_report.rs:1489-1502` pushes only `foreign_tax_credits_aud`, while
  `tax_summary.rs:861-864` adds the apportioned `foreign_tax_credits_capital_gains`. With
  `foreign_tax_credits = 500` and a partly-discountable capital-gains foreign tax of 300, 20O shows
  500 + the claimable share while the report's total shows 500
- [ ] Reproduced: the disposal schedule discounts **each parcel before losses are netted**
  (`tax_report.rs:788-792`, summed by `DisposalTotals::add` at `:430-436`, printed at
  `src/web/taxreport.js:189-198`) and contradicts the 18A figure the same document prints. A $200
  eligible gain and a $150 loss print "gain after discount −50" where the ATO order (net, then
  halve) gives 18A = 25. No ATO label consumes the schedule figure, so 18A itself is right — but the
  archived document disagrees with itself, and the module doc's "computes nothing new" is untrue of
  this column
- [ ] Fix: carry the denial into `TrustIncomeRow`; add the apportioned AMMA capital-gains foreign tax
  to the foreign-income total; and either print per-disposal **gross** figures and strike the
  concession on the net gain, or label the column explicitly as a notional pre-netting figure
- [ ] Tests: the existing reconciliation test extended to cover a denied trust credit, an AMMA with
  capital-gains foreign tax, and a year with both a gain and a loss
- [ ] Docs sync: `docs/API.md`'s annual tax report section for the disposal-schedule resolution

## The hash router has no stale-render guard, so a slow view can overwrite a newer one (2026-09-17 review, web frontend)

(2026-09-17 review. `render()` (`src/web/app.js:2940`) dispatches on the hash, awaits the view's
fetches, and paints with `setMain`; nothing ties the paint to the navigation that requested it.)

- [ ] Reproduced by reading the dispatch (2949–2992) against `setMain` (487/628/2773): clicking
  "Trades" (a large `GET`) and then immediately "Snapshots" leaves whichever `await` resolves last
  in charge, so the URL and the nav highlight say Snapshots while the Trades table — or a stale error
  page — is on screen. A stale `toast(e.message, true)` can equally fire over the new screen
- [ ] Fix: a module-level `renderSeq` incremented in `render()`, checked after every `await` before
  any `setMain`/`toast`, or a `setMainIfCurrent(seq, node)` that every view paints through
- [ ] Tests: a `web.rs`-style assertion that the sequence guard exists and is checked before each
  paint (the served-bundle convention this project uses for UI behaviour, there being no browser
  harness), or a `src/web/*.test.js` unit test of the guard as a pure function
- [ ] Docs sync: none

## Fire-and-forget view reloads are unhandled promise rejections (2026-09-17 review, web frontend)

(2026-09-17 review. Views reload themselves after an action without awaiting or catching the result,
and there is no global `unhandledrejection` handler.)

- [ ] Reproduced by reading the call sites: `src/web/app.js:460`, `:495` (entity list reload after
  DELETE), `:674`, `:798`, `:1038`, `:1127`/`:1149`/`:1183` (`refresh()`), `:1622`,
  `:1733`/`:1756`/`:1788`/`:1831`/`:1871`, `:2000`/`:2039`/`:2081`. On Closing Prices, "Discard"
  succeeds and toasts, then `viewClosingPrices()` (`:1756`) rejects on its `GET` — the discarded row
  stays on screen, nothing tells the user, and the only trace is a console "Uncaught (in promise)"
- [ ] Fix: one shared `reload(fn, …args)` that catches and toasts, used by every call site, and/or a
  global `window.addEventListener('unhandledrejection', …)` as the net
- [ ] Tests: a `web.rs` assertion that no bare `view*(` call is invoked as fire-and-forget (or that
  the global handler is installed), in the served-bundle style
- [ ] Docs sync: none

## Settlement resolution reads the stored trade outside the write transaction (2026-09-17 review, integrity)

(2026-09-17 review. `settlement_date_source` is decided from a read on the pool and then written by a
transaction begun later, so a concurrent write can make the stamp describe a row state that no longer
holds.)

- [ ] Reproduced by statement sequence: `src/entities/trade/http.rs:69-70` resolves on the pool, and
  `src/entities/trade/db.rs:344` begins the write transaction afterwards; the Sell path is the same
  shape (`src/entities/sell.rs:464-466` → `:468`). The classifying read is
  `src/entities/trade/settlement.rs:137-150`
- [ ] Failure: a `PUT` replaying a `GET` body (what the web edit form sends) classifies the source
  from the stored row; a concurrent recompute or another `PUT` changes the row before the write
  lands, so the row is stamped with a source that does not describe how the date it wrote was
  arrived at. The blast radius is provenance only — no tax figure reads `settlement_date` — but
  `settlement_date_source` is exactly what governs whether the `settlement-recompute` job may
  rewrite the date, so a user-asserted date can be silently re-derived or a wrong computed one never
  repaired
- [ ] Fix: resolve on the transaction's own connection — `auto_settlement_date_on(conn, …)` already
  exists, so add a `resolve_on(conn, …)` and call it after `write_tx`
- [ ] Tests: a DB-level test resolving inside the transaction, plus a concurrent test that a
  recompute interleaved with a stated-date write cannot mis-stamp the source
- [ ] Docs sync: none

## The price-alert scan reads without a snapshot, and its send and send-log are not atomic (2026-09-17 review, integrity)

(2026-09-17 review of the `price-alert` job added in v0.23.0. Two related properties, both
low-severity: neither corrupts a stored financial figure.)

- [ ] Reproduced by reading `src/entities/price_alert.rs:192-257`: `db_held_listing_ids(pool)`, then
  per listing `db_latest_two_closes(pool)`, `db_listing_identity(pool)`, `db_price_basis_events(conn)`
  and `db_already_alerted(pool)` — each its own implicit snapshot while a price import or corporate
  action can land. This is the one multi-query read in the tree not on one `pool.begin()`, and the
  `deferred_begin` discipline test (`src/infra/db.rs`, `DEFERRED_BEGIN_ALLOWED`) does not see it
  because that scan walks `src/reports/` only. The recorded row states the exact pair compared, so
  the failure mode is a misleading or skipped alert
- [ ] Reproduced by reading `:343-351`: `mailer.send(...)` then `db_record(...)` (`:155-182`,
  `ON CONFLICT(listing_id, price_date) DO NOTHING`). A process death or a failed insert between them
  leaves no row, so the next run re-identifies the same move and re-sends — at-least-once
  notification. `migrations/0049_price_alerts.sql:73-75` documents only the send-failure direction
- [ ] Fix: hold the scan's reads on one `pool.begin()` as the reports do; and either document the
  at-least-once property or make the pair atomic (insert-then-delete-on-send-failure flips the
  failure to a silently-swallowed alert, which is probably worse — decide deliberately)
- [ ] Tests: a scan read inside one snapshot; a `db_record` failure leaving no row and the next run
  re-alerting (the documented contract, pinned so it stays a decision)
- [ ] Docs sync: `docs/API.md`'s Emailed reports / Jobs section for the at-least-once wording

## Enum-shaped columns stored as free text with no CHECK (2026-09-17 review, integrity)

(2026-09-17 review, from the schema replay. The project's rule is that a field holding a limited set
of values is a CHECK-constrained column and a typed enum where parsed; three remain free text. Every
user-facing enum is correctly constrained — these are the outliers, and the consequence is a silent
typo rather than corruption.)

- [ ] `mic_registry.status` — no CHECK, and `src/entities/mic_registry.rs:31` declares
  `pub status: String` while the doc comment at `:20` names the closed set `ACTIVE|UPDATED|EXPIRED`
- [ ] `distribution_events.source` — `migrations/0048_distribution_events.sql` has no CHECK at all,
  and `src/entities/distribution_event.rs:147` is `pub source: String`, although the provider trait
  (`fn source() -> &'static str`) makes it a one-value set today. `db_store` compares
  `source <> excluded.source` to decide whether a re-fetch is a revision, so a typo silently changes
  that decision
- [ ] `closing_prices.source` — the only constraint is
  `CHECK ((source = 'manual') = (origin = 'manual'))`, so a *fetched* row's source is free text
  ({yahoo, manual} live)
- [ ] Fix: a CHECK per column plus a typed enum in Rust, via a migration. Note both
  `distribution_events` and `closing_prices` are audited, so a table rebuild must DROP and re-CREATE
  both `*_row_history_*` triggers with the new column list (the 0029/0045 precedent), and
  `closing_prices` must keep its staleness trigger re-created too
- [ ] Tests: a direct DB write of an out-of-set value rejected by each CHECK
- [ ] Docs sync: `docs/SCHEMA.md` for each column

## Credential and log hygiene: the bearer token in `curl`'s argv, the login username in a log line, and the backup command in a job error (2026-09-17 review, security)

(2026-09-17 review's security pass; three low-severity hygiene findings, grouped because each is a
"the value is fine, the place it is written is not" issue.)

- [ ] `pkg/freebsd/update.sh:121-123` builds `AUTH_HEADER="Authorization: Bearer $CONF_TOKEN"` and
  passes it as a `curl` argument, so the token is visible in `ps` to any local user for up to the
  `-m 900` timeout — exactly the exposure the README's own rationale refuses `--auth-*` flags for.
  The quoting is correct (one argv element); it is argv itself that leaks. Fix: `curl --config` a
  `0600` tempfile (`trap`-removed), or an env var sourced from one
- [ ] `src/infra/auth.rs:417` logs `username = %form.username` on a failed login reached by the
  **pre-auth** `POST /login`, and the default `fmt` subscriber writes `%` fields verbatim, so
  `username=evil%0A…` appends attacker-chosen lines to the log. No secret is exposed and the app is
  unaffected, but a log reader can be shown a fake failure or have a real one buried. Fix:
  `?form.username`. Worth the same treatment on the feed parse-error fields
  (`currencies.rs:546`, `mic_registry.rs:241`, `rba_fx_rate.rs:409`)
- [ ] `POST /jobs/{name}` returns the job's raw error text in the response body
  (`src/infra/http.rs:182-187`, `src/infra/scheduler/http.rs:142-147`) and stores it in
  `job_runs.error`. Deliberate — it is the operator's own diagnostic — but for `backup` it includes
  the full substituted `backup_command` (`src/infra/db.rs:274-280`), so a credential embedded in a
  hook (`curl https://user:pass@…`) lands in the response and in the Jobs screen. Fix: redact or
  truncate the command in the error, or document "no credentials in `backup_command`"
- [ ] Tests: a log-capture test asserting a control character in a failed-login username does not
  split the line (the `tracing-test` harness is already a dev-dependency); a unit test on whatever
  redaction the backup error gains
- [ ] Docs sync: the README's Off-machine copies section if a redaction rule is documented instead

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
