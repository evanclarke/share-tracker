# TODO

Items are only marked done when a passing test exists for them.

Completed and decided (out-of-scope / not-reproducible) sections are archived in the topical `DONE/*.md` files, indexed by [DONE.md](DONE.md), to keep this list focused on active work. When a section here is fully done, move it into the matching `DONE/*.md` file rather than leaving it — see CLAUDE.md.

Sections here come from the **2026-07-13 improvement review** (a whole-project pass for
operational and test-strategy gaps, as distinct from the 2026-07-12 programming/domain review whose
findings are all closed in DONE.md), except where a section's heading names another source
(e.g. REQUIREMENTS, SCENARIOS). Each section records one finding; sections land in DONE.md as they
are fixed or decided.

**Open: 9 sections — 7 from the 2026-08-25 code review, plus 2 follow-ups.** Sixteen sections were
recorded from that review (a whole-codebase pass over `src` — business-logic errors, quality,
duplication — every finding adversarially verified against `f171999`). Nine are closed and archived
in [`DONE/reviews.md`](DONE/reviews.md): the AMIT×rollover pair (`44b8a6b`), the four-endpoint
fx-parity family (`1341068`), and the three web findings (`c190871`). The remaining seven are
below, with two follow-up sections recorded from observations made while fixing.

## The tax report's per-parcel `capped` flags apply events in kind order, the CGT walk in date order (code review 2026-08-25)
(`adjustment_detail` (src/domain/cost_base.rs:755) computes its per-row `capped` flags by applying
all AMIT rows before any ROC rows — kind order, dates ignored, then merely sorts for display —
while `net_capital_gain`'s E10/G1 walk (net_capital_gain.rs:529) applies the same events in date
order. The two halves of one archived Annual Tax Report can disagree about which event exhausted
the cost base and by how much.)
- [ ] Reproduce: base $100, ROC $60 (2023-09), AMIT $60 (year end 2024-06-30) → the printed
  per-parcel worksheet flags the 2023 ROC row as capped (AMIT fully absorbed) while the same
  report's CGT summary shows `cgt_event_e10_gain` = $20 attributed to the AMIT event —
  contradictory attribution inside one archived PDF
- [ ] Fix: `adjustment_detail` applies events in the same date order as the E10/G1 walk — one
  ordering rule, not two
- [ ] Tests: the reproduction's worksheet and CGT summary attribute the excess to the same event

## The period-performance report reads across five-plus separate snapshots, invisibly to the scan test (code review 2026-08-25)
(`compute` (src/reports/period_performance.rs:278) takes `&SqlitePool` and reads through separate
implicit transactions — `valuations_or_empty` twice, `db_performance` twice (each opening its own
internal `pool.begin()`), plus `FxRates::load(pool)` — instead of one `pool.begin()` snapshot,
violating the one-consistent-snapshot rule. The `DEFERRED_BEGIN_ALLOWED` scan never sees it because
the file never types `.begin()`, and `reports/period_performance.rs` is absent from the allowlist.)
- [ ] Reproduce/fix: a backdated Buy committing between the `perf_from` and `perf_to` reads appears
  in `invested(to)` but not `invested(from)`, so `purchases` misbooks it as an in-window purchase
  and the Capital + FX + Income identity no longer sums; refactor `compute` onto one read
  transaction threaded through its helpers, and add the file to `DEFERRED_BEGIN_ALLOWED`
- [ ] Strengthen the scan: a report file that reaches the pool without ever typing `.begin()`
  currently passes unclassified — extend the scan (or add a companion check) so a report absent
  from the allowlist that queries via `&SqlitePool` is also an offender
- [ ] Fold in the redundant-loads section below if the same refactor removes them
- [ ] Tests: the one-transaction shape pinned the way other reports are

## A split dated on the parcel's own trade date prints a no-op rebase row (code review 2026-08-25)
(`adjustment_detail`'s informational split-rebase rows skip only strictly-earlier splits
(`s.date < parcel.trade_date`, src/domain/cost_base.rs:788) while `split_ratio`'s window skips
`s.date <= from` — a split dated exactly on the trade date emits a rebase row that rebased nothing,
so the archived worksheet no longer explains its own quantity. Cosmetic: informational-only row.)
- [ ] Fix: align the boundary (`<=`) so the same-day split emits no row; test with a 2-for-1 split
  effective on the Buy's own date — unit count unchanged and no rebase row printed

## The Annual Tax Report runs the realised-gains pipeline twice per request (code review 2026-08-25)
(`db_tax_report` (src/reports/tax_report.rs:1512) calls `disposals_section` → `db_realised_gains_on`
(line 763), then `db_cgt_summary_year` (line 1530) re-runs `db_realised_gains_on` plus
`gross_buckets`; `FxRates` and `IncomeContext` each load twice. All inside the one transaction, so
consistent — efficiency only. `gross_buckets` already takes a `&realised` slice, so a
`db_cgt_summary_year` variant accepting the caller's precomputed slice is a small signature change.)
- [ ] Fix: compute the realised set once and pass it through; drop the duplicate `FxRates` /
  `IncomeContext` loads
- [ ] Tests: existing tax-report figures unchanged (the suite already pins them)

## The period-performance report loads every held listing's market data about four times (code review 2026-08-25)
(`valuations_or_empty` (src/reports/period_performance.rs:205) calls `valuation::held_markets`
solely for an `is_empty()` check, then `stored_valuations_on`'s first line re-runs
`held_markets_on` for the same date; `compute` runs the pair for both `from` and `to`. Each
`load_market_on` is several queries per listing (listing, full `RenameHistory`, exchange, holidays)
— identical loads executed ~4× per request on different pooled connections. Likely closed by the
one-transaction refactor above; fold there if so.)
- [ ] Fix: load each date's markets once and pass them through (or drop the pre-check); verify with
  the one-transaction refactor

## resolve_valuation_rate's valuation-only restriction has no scan test (code review 2026-08-25)
(`infra/fx.rs:220`'s fallback-rate resolver is restricted by CLAUDE.md to three callers — snapshot
generation, live-quote conversion, period-performance FX attribution — so no tax figure is ever
computed from a fallback-month rate. Unlike the deferred-`BEGIN` rule it parallels, nothing
enforces it: a future tax-path change can call it and the suite stays green.)
- [ ] Add a source-scan test naming the allowed caller files, mirroring
  `write_side_modules_never_begin_a_deferred_transaction` — a new caller is an offender until
  classified

## entities::inheritance skips the CrudEntity pattern and re-implements the FX lookup (code review 2026-08-25)
(src/entities/inheritance.rs implements no `CrudEntity` — hand-writing list/get against the
entity-module pattern, so the generic handlers' contracts (column list, 404 wording) can drift
untested — and its `check_convertible` (~line 540) re-implements the `FxRates` month-rate lookup
raw instead of calling `infra::fx`, so a change to FX resolution semantics silently misses it.)
- [ ] Refactor onto `CrudEntity` + the generic handlers for the verbs they cover (hand-written
  verbs stay only where they do more than one table's work, per the pattern)
- [ ] Replace the raw month-rate lookup with the `infra::fx` API
- [ ] Tests: existing behaviour pinned through the refactor (404 wording via the shared
  missing-row test)

## net_capital_gain's C2 branch can treat a rollover cohort as a C2-triggering disposal (code review 2026-08-25 follow-up)
(Observed while fixing the held/spill finding, `44b8a6b`. In the per-cohort E10/G1/C2 walk
(src/reports/net_capital_gain.rs), the C2 branch treats any cohort with a `disposed_on` between a
ROC event's record and payment dates as a C2-triggering disposal; a rollover cohort in that window
could produce a phantom C2 gain while the replacement parcel also takes the G1 reduction.
Distinguishing real-disposal cohorts there would need per-allocation provenance in the cohort read.
This is an unverified observation from the fixing pass — re-derive it with a failing reproduction
first, per the U lesson.)
- [ ] Reproduce: a transfer (or scrip/demerger) dated between a ROC's record and payment dates;
  assert whether a C2 gain is raised on the rollover cohort while the replacement parcel also takes
  the G1 reduction
- [ ] Fix if confirmed; if not reproducible, record why and close
- [ ] Tests + docs sync per the outcome

## config.js's sel() option lists and the health banner's field names are unpinned mirrors of Rust-side definitions (code review 2026-08-25 follow-up)
(Found by the JOB_DESC/tradeOrigin fix's sweep, `c190871`. config.js `sel(…)` option lists mirror
CHECK-constrained Rust enums with no programmatic pin: `security_type`, `cost_base_rule`,
`income_type`, `expense_type`, `residual_handling`, `action_type`, `worthless_event` — the
`action_type` set *is* pinned (`corporate_action_form_is_split_by_type`) but by a hand-written list
inside the test, a third copy. No iterable Rust const of the variant names exists (only per-variant
match arms and the migration CHECK), so a real derivation needs a new Rust const or a
schema-CHECK-parsing pin. Separately, the health banner's field names in app.js are pinned in
`health_banner_ui_present` by hand-written strings, not derived from `reports::health`'s serde
field names. chart.js's `fyStart` deliberately restates `domain::tax_year`'s July rule (commented,
unit-tested) — an inherent no-build-step mirror, acceptable as is.)
- [ ] Pin each `sel()` option list to its Rust enum / schema CHECK programmatically (a new Rust
  const per enum, or a pin that parses the live schema's CHECK), replacing the hand-written
  `action_type` list in the existing test rather than adding beside it
- [ ] Derive the health-banner pin from `reports::health`'s serde field names
- [ ] Tests are the deliverable; no docs change expected

**Every section of SCENARIOS.md — A through AA — is now driven and every finding they raised is
closed** in the `DONE/*.md` archive. Section **S. Settlement, holidays, and dates** was driven 2026-08-22 (`d501408`) and its
four findings closed by `67c3096` (a trade dated in the future), `30d0e96` (a trade dated on a day
its exchange was shut), `4a7ef1a` (a stored settlement date that is not a trading day) and
`e453f21` (the settlement dates a completed calendar changes) — all four archived in
[`DONE/trades-income.md`](DONE/trades-income.md), and summarised with the rest of the pass under
[Section S findings](SCENARIOS.md#section-s-findings). Every section's row in SCENARIOS.md's
[Verification status](SCENARIOS.md#verification-status) table names the pass that drove it and where
its findings went; that table is the record of what has been looked at.

Section **T. Jobs, backup, and operations** (12 scenarios) was driven on **2026-08-22** against
throwaway databases (a small one for the HTTP surface, a 265 MB one to catch a backup mid-write) and
raised **six findings**, and all six are now closed — the three jobs that recorded their failure as a
Rust `Debug` string (T-06), the startup "no schedule entry" warning that cried wolf on the two
deliberately-manual jobs (T-09/schedule), `POST /jobs/:name`'s bare-status-code failures, now
bodied 404/500 replies with an unknown query parameter refused rather than ignored (T-10), the
run interrupted by a restart that left no record and an unverified file wearing a backup's name
(T-11), now a run row opened at the start and a backup staged under `.partial` until it verifies,
the job that stops running and is never noticed (T-11/T-02/T-12), now a `job_schedule` table the
scheduler rewrites every iteration with a health `overdue_jobs` list and a **next run** column on the
Jobs screen over it, and the currency import that skipped the whole ISO 24165 half and reported an
unqualified success (T-09), now a per-feed import summary and a `job_runs.note` the Jobs screen shows
beside a still-`ok` status — all six archived in [`DONE/infra.md`](DONE/infra.md).
Everything else in the section came back correct: the
per-job lock serialises overlapping triggers (T-01), the run history bounds itself at 20 while
keeping a fail-then-succeed sequence readable (T-02), a corrupt backup is quarantined (T-03),
retention pruning over 20 backups spanning 18 months keeps exactly the newest 8 plus the first of
each of the 12 most recent months and touches nothing else in the directory (T-04), the DST-pinned
price-import entries fire at 17:30 local on both sides of every transition in both hemispheres and
handle the skipped and repeated hours exactly as the README states (T-05), the manual CSV retry
imports what the unreachable feed would have (T-06), an expiring MIC flips the validation report to
`expired` without blocking anything (T-08), and a stale price date, a stale FX month and a failed job
all surface at once and independently on the health report and its banner (T-12).

Section **U. Audit trail and history** (8 scenarios) was driven on **2026-08-22** against a
throwaway database, with two live backups read read-only to check each finding against real data.
**The trail's machinery came back correct**: every one of the 22 audited tables' triggers records
every column of the live schema (U-01, checked by diffing `PRAGMA table_info` against each trigger's
`json_object` keys, which is stronger than the name-list pin the tests carry); `row_history` is
append-only and no `REPLACE INTO` path exists anywhere in the tree, while migration 0025 — the one
migration that rewrites an audited table's data in place — deliberately drops the triggers first, so
no migration forges an entry (U-02); a cascade-deleted attachment is recorded like a directly deleted
row (U-04); a superseded manual closing price's `sourced_from`, `reason` and `origin` are fully
recoverable exactly as `docs/API.md` claims (U-06); a non-audited table is refused 422 naming the
audited list (U-07); and the report is unbounded by design, which is the safe direction for an audit
trail — 10,000 entries served in 0.42 s, against a live maximum of 2 entries on any single row
(U-08).

It raised **three findings, and all three are now closed** — the id handed out again that inherits
the deleted row's trail (U-a), which was live in the real database and took three commits: the trail
now says whose history it is (`7b915cf`), migration 0045 gives 17 audited tables `AUTOINCREMENT` ids
(`4a3a257`), and every server-created id now comes from the database rather than `MAX(id) + 1`
(`1a0f821`); the multi-row operation readable only by ids the user never saw, now a browse form over
the whole trail (`be64d3d`); and the trigger-column rule enforced only by hand-written per-migration
assertions, now a generic guard derived from the live schema (`57502eb`) — all three archived in
[`DONE/infra.md`](DONE/infra.md).

Section **V. Back-dated and out-of-order entry** (10 scenarios) was driven on **2026-08-23** against
throwaway databases and raised **five findings, all of them now closed**. Six scenarios came back
correct, and structurally so: every figure is keyed on a *date*, never on entry order or id order —
a year of trades entered in reverse changes nothing (V-01), a Sell allocated to the wrong parcel is
corrected once the forgotten Buy arrives (V-02), an AMMA statement entered before any trade is
recordable and its generation refusal names all three reasons no parcel was open (V-04), a rename
recorded after prices were collected leaves them untouched (V-05), a return of capital dated inside
a snapshotted period stales exactly the snapshots on and after it — in both directions when the date
is later moved (V-07), and a back-dated fact re-chains every later year's carried-forward loss
(V-10), whose unmarked restatement is the documented A-15 limitation rather than a new finding.

The five findings were: a misspelt request-body field dropped so the record took its default — an
AMMA statement losing A$7,142 under a `204` — now `deny_unknown_fields` on every HTTP body with a
test that walks the extractors to keep it true (`5e6246b`, archived in
[`DONE/infra.md`](DONE/infra.md)); a DRP reinvestment entered behind a later one reading its residual
forward in time, now refused as undo already was (`b08f891`) and a reinvestment into an
already-closed period bringing forward nothing, now asking the period for the split instead of a
stored column (`4b579c8`) — both archived in [`DONE/trades-income.md`](DONE/trades-income.md); the
trade entered twice that was the one duplication the health report did not look for, now a
`duplicate_trades` check keyed on a repeated broker contract note reference within a listing
(`2d9c3a8`, archived in [`DONE/reporting.md`](DONE/reporting.md)); and the parcel dated behind an
executed scrip exchange, demerge or worthless recognise that was never consumed, now refused across
all eight parcel-creating paths and reported by
[rollover consistency](SCENARIOS.md#rollover-consistency) for any database already in that state
(`fc1fd7b`, archived in [`DONE/tax-domain.md`](DONE/tax-domain.md)). All five are summarised under
[Section V findings](SCENARIOS.md#section-v-findings).

Section **W. Precision, rounding, and scale** (8 scenarios) was driven on **2026-08-23** against
throwaway databases, with the two CSV exports and the CGT worksheet additionally checked against a
read-only copy of the live backup. It raised **six findings, all of them now closed** — two of the
six were split out of the first four while those were being fixed.

**Four scenarios came back correct, and the correctness is in the arithmetic rather than in luck**:
a per-unit AMMA cost-base adjustment quoted to 10 decimal places is exact all the way through to the
CGT event E10 excess (W-01); an 8-decimal crypto quantity allocated across five parcels reconciles to
the digit in every column (W-02); the residual chain across 200 DRP reinvestments drifts by nothing
and its parcels' cost bases sum exactly to the cash paid less the trailing residual (W-03); and a
consolidation whose ratio produces a repeating decimal still sells out cleanly with its whole cost
base recognised, `Decimal`'s 28-digit saturation absorbing the remainder in both directions (W-06).
At 4,600 trades every report answers in 0.02–0.1 s and the UI already renders only the current page,
so the unpaginated payload never reaches the DOM — the one 3 s outlier was a live price fetch, not a
report (W-08).

**The six findings divide into two shapes.** Three are *silent precision loss at a boundary the
project's own rules already cover elsewhere*: a money or quantity sent as a JSON **number** went
through `f64` and lost a satoshi under a `204` (W-a, `331a183`) while CLAUDE.md's "never `f64`" rule
held everywhere else in the tree; a cost base too large for `Decimal` was accepted and then panicked
every portfolio read, dropping the connection with no HTTP response at all (W-b, `b77fe38`); and the
write path that accepted it had no magnitude bound, which stayed live until it was split out and
closed separately (W-e, `1badc54`). The other three are *rounding reaching the user inconsistently*:
the "tax-return-ready" CSV exports carried 28-digit figures under ATO labels — 18V printed as
twenty-four zeros on Evan's real data (W-c, `a2c9c81`); the Annual Tax Report's printed columns did
not add up to their own printed subtotals (W-d, `aece007`); and once the CSV rounded, its worksheet
columns no longer reached the figure they worked to (W-f, `d02cdc2`).

Archived in [`DONE/infra.md`](DONE/infra.md) (W-a, W-b),
[`DONE/trades-income.md`](DONE/trades-income.md) (W-e) and
[`DONE/reporting.md`](DONE/reporting.md) (W-c, W-d, W-f), and summarised under
[Section W findings](SCENARIOS.md#section-w-findings).

Section **X. Transactional integrity and concurrency** (8 scenarios) was driven on **2026-08-23**
against throwaway databases (`f01d814`), with the interrupted operations **actually interrupted** —
the server `SIGKILL`ed at four points inside a 6,000-parcel scrip exchange and four inside a
4,000-parcel AMIT generation, each set preceded by a control run proving a completed operation is
plainly visible — and every race fired as simultaneous HTTP requests off a thread barrier. **Seven
scenarios came back correct**: nothing persists from a Sell whose last allocation is invalid, nor
from a transfer whose network-fee Sell fails after every other leg succeeded, nor from either killed
operation (the AMIT `replace` path kept all 4,000 prior adjustments); 8,685 report reads against
3,923 create/delete cycles of a two-parcel Sell never saw one parcel consumed without the other;
forty rounds of four simultaneous Sells of one parcel's last units answered `204`/`422`/`422`/`422`
every time; and a backup taken mid-write waits for the writer and lands verified. **Two findings,
both closed**: a fact write landing during a snapshot generation was lost while the snapshot was
stored as fresh (X-a, `9e221f3`), and — found by re-running that fix at four times the scale it was
measured at — a write concurrent with a long write transaction died as an empty-bodied `500` once it
outlasted a busy timeout nobody had ever chosen (X-b, `d4b083a`). Archived in
[`DONE/reporting.md`](DONE/reporting.md) (X-a) and [`DONE/infra.md`](DONE/infra.md) (X-b), and
summarised under [Section X findings](SCENARIOS.md#section-x-findings).

Section **Y. Web UI** (12 scenarios) was driven on **2026-08-24** against a throwaway database, in a
real browser rather than a rendered DOM: `scripts/ui-drive.js` (new — a zero-dependency Chrome
DevTools Protocol driver over Node's global `WebSocket`, the interactive counterpart to
`ui-check.sh`'s `--dump-dom`, and like it a manual spot-check tool that CI does not run) clicked,
typed, hovered, answered native `confirm()` dialogs, emulated print media and captured every console
error. **Six scenarios came back correct.** All 8 corporate-action types render their own field
group, per-type date label and description, and a value typed into one group survives flipping the
type away and back (Y-04). The Annual Tax Report prints as a document: 7 tables and 124 rows with no
filter row, no sort indicators and no pager, chrome hidden, headers repeated across page breaks, and
no table overflowing A4 in either orientation — 9 pages of PDF (Y-06). The overview graph's presets
all resolve against the stored series and **FY** clamps correctly to 2024-07-01 across the 30 June
boundary, ranges longer than the series clamping to its start (Y-07). A 504-row Trades table pages at
50, sorts the **whole** set rather than the visible page, re-pages to the first page on a filter
change, and distinguishes "No matching records." from "No records." — 139 ms to sort, 137 ms to
filter (Y-08). Every one of **98 hash routes** the app can generate rendered a view with no console
error and no empty mount (Y-09). Row History renders for all 22 audited tables, and its multi-occupant
split, warning notice and deep-link drill-in all work; `tax_year_settings` deliberately stays one
occupant across a delete-and-re-enter, because a natural key names one fact forever (Y-11). The nav
mega-menu carries all 25 entities and 24 reports with none dropped, every panel opens on hover and
none overflows the viewport, and `nav.test.js` already fails a new report whose `menu` is missing or
typo'd (Y-12).

It raised **seven findings** — six from the pass itself and a seventh (Y-g) found while fixing
Y-d — all of them now closed and archived in [`DONE/web-frontend.md`](DONE/web-frontend.md),
except Y-e in [`DONE/trades-income.md`](DONE/trades-income.md). Two came
from the scenario list (Y-a the toast, Y-b the allocation editor at 50 parcels); three came from the
**standing probes** rather than from any scenario — Y-c from asking whether the confirm dialog's own
numbers add up, Y-d from sweeping every rendered numeric column against `COLUMN_KINDS`, and **Y-e,
the most serious, from asking what an unchanged edit-and-save moves in each entity** — and Y-f from
the route sweep. Y-e is another instance of U's lesson: the reproduction (a no-op save writes
`entitlement_date`) was real, but the mechanism I first attributed it to was not — the franking
at-risk anchor is unreachable from the rows this touches, because a row only opens in simple mode
when it carries no franking credits. The harm had to be re-derived, and the control (the same
pay-date correction with `entitlement_date` never written) is what proved it.

One thing this pass could **not** settle, recorded as an observation rather than a finding: the AMIT
confirm text grows with parcel count — 2,710 characters at 50 parcels — and Chromium elides
`confirm()` at 3,000 characters on macOS and at 32 *rows* on Linux/GTK, which 50 parcels already
exceeds. Headless Chrome does not render the dialog at all (CDP reports the string passed in, not
what is drawn — checked at 500, 2,000, 4,000 and 9,000 characters, all reported verbatim), so there
is no reproduction to reason from and no TODO is opened for it.

Section **Z. Composite lifecycle scenarios** (12 scenarios) was driven on **2026-08-24** (`c800fb2`)
against throwaway databases, API-first with the UI driven in a real browser where the scenario is
UI-shaped, and every chain's arithmetic re-derived by hand rather than read for plausibility. Six
chains came back correct: the takeover chain's cost base and discount clock across three consecutive
rollovers (Z-02), four years of USD RSU vests through a transfer, a sell-to-cover and a currency move
(Z-03), the estate's four asset kinds each taking the right discount clock (Z-04), four currencies
over three accounts with the Division 115 FITO apportionment and the A$1,000 cap (Z-06), the wash sale
flagged across both the FY and the holding-account boundary (Z-07), the two buy-backs either side of
the 25 October 2022 law change (Z-09), the suspended fund's carried-forward prices and eventual G3
(Z-10), and one corrected AMMA propagating coherently through nine reports and stopping there (Z-12).

It raised **seven findings**, all now closed: `bf3b4e8` (a multi-parcel sale's gain printing a cent
apart on two screens, Z-a), `19f5736` (`PUT /sells/:id` rewriting a Buy or DRP trade as a Sell, Z-b),
`6b0cef5` (a trade changing kind under the allocations that depend on it, Z-c), `1460c36` (a
back-dated parcel leaving an AMMA adjustment set stale, Z-d), `a264db1` (the archived CGT worksheet
misnaming bonus issues and consolidations, Z-e), `7aba321` (a trust distribution reported under the
dividends-from-companies label, Z-f) and `d075a18` (a cross-listing rollover blocking its AMIT
adjustment entirely, Z-g) — archived in [`DONE/trades-income.md`](DONE/trades-income.md),
[`DONE/reporting.md`](DONE/reporting.md) and [`DONE/tax-domain.md`](DONE/tax-domain.md), and
summarised under [Section Z findings](SCENARIOS.md#section-z-findings).

Only **three** of the seven came from the scenario list. Z-b and Z-c — the pass's most serious — came
from a **scripted `PUT` landing on an id the database had already assigned to something else**, and
being answered `204`; Z-f came from Z-11's label-by-label reconciliation against a hand-computed
return; and Z-g was found while *fixing* Z-d, which is W's lesson again (W-e came out of fixing W-b),
and was confirmed against a fresh reproduction rather than taken on the fixing agent's report. Four of
the seven fixes replaced a hand-maintained list with a rule — Z-b's five provenance columns became
"an existing row must already be a plain Sell", Z-c's became "a trade's `trade_type` is part of its
identity" (and ordering it after the provenance guards exposed one more hole the column list never
named), Z-e carried each action's announced terms through instead of formatting one derived factor
three ways, and Z-g asked one question of the existing chain-walk instead of consulting two pins in
sequence. That is Y-d and Y-g's lesson arriving on the server side.

The pass also produced one **false reading, caught before it was written down**: `?as_of=` was read
into `/portfolio/open-parcels` and returned sensible-looking holdings for five different dates before
the endpoint turned out to take no query string at all and to document itself as "as at today". The
reading that exposed it was a date on which the answer *had* to differ — which is Y's control lesson
in its cheapest form.

Section **AA. Boundary and out-of-scope scenarios** (20 scenarios) was driven on **2026-08-24/25**
against throwaway databases through the HTTP API, and closes the file: **every section of
SCENARIOS.md has now been driven.** AA was a different shape from every pass before it — each entry
is a *documented limitation*, so the verification was that the system **fails safe** (refuses, flags,
or documents) rather than silently producing a wrong number, and that the documented workaround
actually works.

**Fifteen scenarios came back correct, and several structurally rather than merely documentedly**: a
pre-CGT parcel cannot be entered by any path and the boundary is exact — 1985-09-19 refused,
1985-09-20 accepted — with the same guard on trades, Sells, ESS taxing points and inheritances, while
the one legitimate pre-CGT interaction (`MarketValueAtDeath`) stays open (AA-01); rights over pre-CGT
originals are unreachable by construction, since every anchoring parcel must be a post-CGT Buy/DRP
(AA-14); and a collectable, personal-use asset, main residence or any non-listed asset has **no
`security_type` to arrive under** (AA-04, AA-18). The individual-resident basis is stated on every
report that applies the rate, including the archived Annual Tax Report and the pre-sale what-if
(AA-05). Partial DRP participation is refused and its documented workaround works exactly as written,
**both stated caveats included** — the per-share cross-check left on a split half is itself a `422`,
and an exactly half-and-half split does trip `duplicate_income` (AA-10). K10/K11 is reported on the
data rather than only in a paragraph (AA-11). The reduced cost base really is identical to the cost
base by construction (AA-09), and the estate/LPR side (AA-15), unvested ESS grants and dividend
equivalents (AA-16), crypto chain splits and wrapping (AA-17) and the related-payments rule (AA-20)
are all documented in unusual detail with their recordable halves working.

It raised **six findings — five from the pass and a sixth (AA-f) found while fixing AA-a — and all
are closed**: `369e040` (an indexation-eligible parcel silently costed on the discount, AA-a),
`a86a074` + `73cd193` (a non-renounceable rights issue indistinguishable from a renounceable one,
AA-b), `8c84079` (the investor-not-share-trader assumption stated nowhere, AA-c), `28a0942` (a
nil-proceeds disposal raising a capital loss nothing questions, AA-d), `a9506a7` (four limitations
documented without the workaround that exists, AA-e) and `69981ba` (the archived CGT worksheet
printing a whole parcel's initial cost against a part of it, AA-f) — archived in
[`DONE/tax-domain.md`](DONE/tax-domain.md) (AA-a, AA-b, AA-c, AA-e) and
[`DONE/reporting.md`](DONE/reporting.md) (AA-d, AA-f), and summarised under
[Section AA findings](SCENARIOS.md#section-aa-findings).

Only **two** of the six came from the scenario list as written. AA-b came from asking what
`sell_rights` does when the offer it assumes is not the one recorded; AA-c from noticing that AA-07
was the only scenario in the section with **no documented limitation behind it at all**; and AA-f from
a fixing agent's incidental report, re-driven from scratch and bounded with its control before being
logged. Three of the six fixes replaced a hand-maintained list or a silent assumption with a rule —
AA-d's found **three** existing transcriptions of the trades provenance columns and one of them
already wrong (a crypto network-fee Sell reported as "entered directly"), replacing all three with one
rule plus a `PRAGMA foreign_key_list` guard; AA-b made `renounceable` a **required** field rather than
a defaulted one, because a default would have left the same silent assumption for every new entry; and
AA-b's second item pulled the shared clause out so the two refusals read as one rule in two places.
