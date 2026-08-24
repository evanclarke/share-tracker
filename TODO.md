# TODO

Items are only marked done when a passing test exists for them.

Completed and decided (out-of-scope / not-reproducible) sections are archived in the topical `DONE/*.md` files, indexed by [DONE.md](DONE.md), to keep this list focused on active work. When a section here is fully done, move it into the matching `DONE/*.md` file rather than leaving it — see CLAUDE.md.

Sections here come from the **2026-07-13 improvement review** (a whole-project pass for
operational and test-strategy gaps, as distinct from the 2026-07-12 programming/domain review whose
findings are all closed in DONE.md), except where a section's heading names another source
(e.g. REQUIREMENTS, SCENARIOS). Each section records one finding; sections land in DONE.md as they
are fixed or decided.

**Open: the six findings of the SCENARIOS Y pass** (`## SCENARIOS Y-a` … `Y-f` below), raised
2026-08-24 and each carrying the options offered and the option chosen.

**SCENARIOS.md sections A–X are driven and every finding they raised is closed** in the `DONE/*.md`
archive. Section **S. Settlement, holidays, and dates** was driven 2026-08-22 (`d501408`) and its
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

It raised **six findings**, logged below with the options offered and the option chosen. Two came
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

After Y, the next SCENARIOS pass is section **Z. Composite lifecycle scenarios** (12 scenarios),
driven the way S through Y were. The lessons worth carrying forward are in the handover memory.
Y added two. First, **drive the UI in a browser that can click, not one that can only render**:
four of this pass's six findings are invisible to a `--dump-dom` render — the toast's lifetime, the
50-row allocation refusal, the `confirm()` text, and the no-op save — and the fifth (Y-d) needed the
rendered cell rather than the JSON behind it. Second, **an apparent finding is a measurement until
its control agrees**: this pass produced two false alarms before either was written down — every
graph preset appeared stuck (my own script left stale `id` attributes on earlier buttons, so every
click hit `1M`), and `tax_year_settings` appeared to save a blank form (the "Saved." toast was the
previous entity's, still on screen inside its 6-second window — which is Y-a seen from the other
side). Both would have been logged as findings by a pass that trusted its first reading.

## SCENARIOS Y-a — an error toast holds a refusal for six seconds and then it is gone

- [ ] Make an error toast persist until it is dismissed, and announce it.

Driven through `scripts/ui-drive.js` against a throwaway database. Deleting a listing that nine other
tables draw on answers `422 this listing is still referenced by AMMA statements (1), closing prices
(1), corporate actions (1), DRP enrolment periods (1), ESS statements (1), income (1), inheritances
(1), investment expenses (1), trades (2) — remove those records first`. The toast **does** show it in
full — measured at 1280×900, 1024×768 and 820×700, wrapping to 2–3 lines, never clipped, never
overflowing the viewport, `scrollHeight == clientHeight` — so the rendering half of Y-02 is correct.

What is not correct is how long it is there for. Measured: **6,045 ms** from click to `hidden`, fixed
(`util.js`'s `toast()` uses `isError ? 6000 : 3000`). It has no close button, no click handler
(clicking it does nothing), no `role` and no `aria-live`, and once it hides the text exists **nowhere
else in the document** — checked. So the user has six seconds to read and memorise nine table names
before the only statement of why their delete failed is destroyed, with no way to bring it back. The
longest 422 the API can produce is a serde unknown-field rejection at **638 characters**; it renders
in three lines and is equally unrecoverable.

The same six-second window produced a false finding inside this very pass: a "Saved." toast still on
screen from the *previous* entity was read as `tax_year_settings` having saved a blank form. A toast
that outlives the action it belongs to is ambiguous in both directions.

**Options offered:**
1. **Error toasts persist until dismissed** — an error toast stays up with a close button (and
   click-to-dismiss); success toasts keep the 3 s auto-hide. Add `role="alert"`.
2. Scale the timeout to the message length (e.g. 6 s + 60 ms/char, capped), plus `role="alert"`.
3. Keep the toast and additionally render the refusal into a persistent inline block in the view.
4. Leave it and record a known limitation.

**Chosen: option 1.** No arithmetic, matches the existing two-tier success/error split, and the
message stays until the user has actually dealt with it.

## SCENARIOS Y-b — a 50-parcel allocation is refused without saying what it adds up to

- [ ] Name the sum in the refusal, and show a running total in the allocation editor.

Y-03 driven with 50 open parcels on one listing. The editor itself holds up: "+ Add allocation" 49
times took 413 ms, all 50 rows render, each parcel select carries all 50 options labelled
`101: FIFTY — 100 remaining (acquired 2022-01-10)`, the submit button stays reachable, and a correct
50-row allocation saves (`Sell saved.`). A wrong one is refused safely.

But with one row typed as `10` instead of `100`, the refusal is
`HTTP 422: the parcel allocations do not sum to the sell quantity` — it never says what they *do* sum
to, no row is marked, and the editor shows no running total (`allocationEditor` renders heading,
hint, rows and an add button, and nothing else). So the user is told 50 numbers are wrong by an
unstated amount and must add them up by hand to find out which — inside the six-second window of
Y-a.

The better wording already exists one room away: `reports/net_capital_gain.rs:1244` refuses the same
condition on the what-if path as `the allocations sum to {total}, not the {n} units sold`. The write
path that matters has the worse message. Scope is `entities/sell.rs`'s `SellError::AllocationMismatch`
(shared with buy-back participation) and `entities/rights_sale.rs`'s equivalent; `transfer` moves
whole parcels and has no sum invariant.

**Options offered:**
1. **Both** — give the 422 the figures, and add a live allocated-vs-required total to the shared
   `allocationEditor`.
2. Server message only.
3. Running total in the editor only.
4. Leave it.

**Chosen: option 1.** The server half is the correctness fix and is testable; the editor half is what
actually helps at 50 rows.

## SCENARIOS Y-c — the AMIT confirm dialog's own numbers do not add up

- [ ] Return each generated adjustment's re-based quantity too, and show both in the confirm gate.

Y-05 driven on an AMMA statement over a listing carrying a 1-for-2 `ShareSplit` between acquisition
and the statement's year end. The gate works in every other respect: it previews without writing,
Cancel writes nothing (0 adjustments after), Accept writes 2 and reports the mismatch in the toast,
and the mismatch warning is prominent. What it showed was:

```
  • 10: Buy 1000 (XASX:MEGA, 2023-08-10) — 1000
  • 11: Buy 5 (XASX:MEGA, 2024-05-01) — 5

Adjusted units 2005 vs the statement’s units held 1000

⚠ MISMATCH of 1005 units.
```

The listed quantities sum to 1,005; the stated total is 2,005; and the mismatch figure is *also*
1,005, which actively invites the reader to think one of the two is a typo for the other. Both server
figures are right and deliberately so — `GeneratedAdjustments` documents that `created[].quantity` is
each parcel's **as-acquired** units while `units_adjusted` is "re-based into the statement year's unit
basis, so it is comparable with `units_held`". The dialog puts the two bases side by side unlabelled.

**Control:** the same generation over a listing with no corporate action lists 50 parcels summing to
exactly `units_adjusted` (5,000 = 5,000). The split re-basing is the whole cause — nothing else.

**Options offered:**
1. **Return both bases per row** — `created` gains the re-based quantity, so the dialog can read
   `1000 units (2000 in the statement year's basis)` and the list visibly reaches the total.
2. Label the difference in the dialog text only (no API change).
3. List the re-based quantities instead of the stored ones.
4. Leave it.

**Chosen: option 1.** Option 3 was rejected because the dialog would then show figures that do not
match the AMIT Adjustments screen afterwards; the stored quantity must stay visible.

## SCENARIOS Y-d — two Tax Summary money columns are rendered at four decimal places, ungrouped

- [ ] Classify both in `COLUMN_KINDS`, and pin the list with a test so the next one cannot drift.

Y-10 driven by sweeping every numeric column actually rendered on every entity list and every report
against `util.js`'s `columnKinds()`. The entity lists came back clean — every unclassified numeric
column there is an id, a foreign key, or a code that verbatim rendering is *required* for
(`currencies.numeric_code` is `036`, which money formatting would destroy). The performance report's
`total_return_pct` / `money_weighted_return_pct` are unclassified but the server already
`round_dp(4)`s both, so verbatim is stable and correct.

Two real gaps, both in the Tax Summary and both `Decimal` money on the server
(`reports/tax_summary.rs:103,135`): **`employment_income`** and
**`foreign_tax_offsets_cgt_discount_reduction`** are absent from `COLUMN_KINDS`' money list. With an
employment-income row of 1,234,567.8912 entered, the same figure renders three different ways:

| where | rendered |
| --- | --- |
| Tax Summary screen, `Employment income` | `1234567.8912` |
| Tax Summary screen, neighbouring `Gross assessable investment income` | `2,757.30` |
| the screen's own **Export CSV** | `1234567.89` |

So the screen is the odd one out, and it is odd against its own export — the CSV was already brought
to cents by W-c/W-f. This is that same rule with a hole in it, in the one place a hand-maintained
name list can always grow one.

**Options offered:**
1. **Add both, plus a test pinning the list** — walk every report/entity payload and require each
   `Decimal` column to be in `COLUMN_KINDS` or an explicit verbatim allowlist.
2. Just add the two columns.
3. Leave it.

**Chosen: option 1** — W's "when a rule needs a list, move the list somewhere the compiler sees it",
applied to the UI's copy of it.

## SCENARIOS Y-e — a no-op edit writes an entitlement date that then stops tracking the pay date

- [x] Offer the pay date as a placeholder, never as a written value.

Found by the standing probe "what else moved that shouldn't have", applied to an
**edit-and-save-unchanged** of the first row of every entity. Fifteen of sixteen entities round-trip
untouched. Income does not: opening a simple-mode trust distribution and clicking Save without
touching anything writes `entitlement_date = date_paid`.

`forms.js`'s `applyEntitlement()` prefills the field when the franking selector reads `Trust`, which
is right for a new entry and wrong for an existing row, because **NULL is a deliberate state** — the
field's own hint says *"Leave empty to assess by the pay date."*

Writing it is not immediately visible: `entitlement_date = date_paid` gives the same financial year
and the same FX month as NULL. The harm is that the written date then **stops tracking**. Measured
end to end on a trust row of A$9,000 paid 2025-07-05:

| step | `entitlement_date` | FY2025 | FY2026 |
| --- | --- | ---: | ---: |
| as entered | `null` | 2,757.30 | 23,000 |
| after a **no-op** open-and-Save | `2025-07-05` | 2,757.30 | 23,000 |
| then pay date corrected to 2025-06-25 | `2025-07-05` | 2,757.30 | 23,000 |
| **control** — same correction, `entitlement_date` never written | `null` | **11,757.30** | **14,000** |

The control is what proves it: correcting a June distribution's pay date moves A$9,000 into FY2025
unless an earlier no-op save silently pinned it to July. Nothing tells the user the row stopped
following the pay date, because they never entered the date that pinned it.

**Re-derived, per U's lesson.** The first mechanism I attributed this to was the franking at-risk
walk, which anchors on `ex_date` *else* a trust row's `entitlement_date` and flags a row
`untested_no_ex_date` when neither is recorded — so writing one would have turned an honest warning
into a confident answer on a window starting weeks late. **That is unreachable**: a row only opens in
simple mode when it fits a simple shape, and the trust shape carries no franking credits, so the rows
this prefill touches have no credits to be at risk. Driven and confirmed — the advanced-mode row was
left untouched (`applyEntitlement` returns early when the advanced flag is set). A correct
reproduction was not evidence for the mechanism first attributed to it.

**Options offered:**
1. **Show the pay date as a placeholder, never a value** — nothing is stored unless typed.
2. Prefill on new entry only (`if (!existing)`).
3. Keep writing it, but re-track it when the pay date is edited.
4. Leave it.

**Chosen: option 1.** Option 2 was rejected because a newly entered trust row would still get a
written date the user never chose, carrying exactly the same staleness exposure; the placeholder
communicates the default without fixing it, and fixes both cases at once.

**Done.** `applyEntitlement()` no longer assigns `entitlementInput.value`; the pay date is offered as
the field's default in a live hint appended below it (`entitlementDefaultHint(datePaid)`, a pure
export of `forms.js`), because `<input type="date">` renders no `placeholder` in any browser. The
hint names the pay date currently entered and re-renders on every `input`/`change` of the pay-date
field, in advanced mode too; `config.js`'s static hint drops its now-duplicated closing sentence.
Reveal-on-Trust and the `mode !== 'Trust'` clearing at submit are unchanged (both re-driven).
Tests: `src/web/forms.test.js` (new) unit-tests the hint wording — the part that can be executed —
and `web::tests::income_entitlement_date_ui_present` pins the invariant as a served-bundle
assertion, asserting the bundle contains no assignment to the entitlement input at all. `docs/API.md`'s
UI paragraph now states the field opens empty with the pay date named as its default.
Re-driven end to end (headless, `scripts/ui-drive.js`): the no-op open-and-Save leaves
`entitlement_date` `null`, and the subsequent pay-date correction to 2025-06-25 now moves the
A$9,000 to FY2025 — 11,757.30 / 14,000, identical to the API-only control. One correction to the
write-up above: the "control" row was **not** reachable through the UI before this fix — merely
opening the edit form pinned the date, so a UI-driven control reproduced the bug rather than the
control. It was necessarily measured through the API, and now the UI matches it.

## SCENARIOS Y-f — `#/e/<custom-slug>` renders a raw JavaScript TypeError as the whole page

- [ ] Send `#/e/jobs`, `#/e/closing_prices`, `#/e/sells` and `#/e/transfers` to their real routes.

Four entities are rendered by custom views reached at their own routes (`#/jobs`, `#/prices`,
`#/sells`, `#/transfers`). `app.js`'s router still resolves `#/e/<slug>` for them through the generic
`viewEntityList`, which then fails: `#/e/jobs` → `Cannot read properties of undefined (reading
'map')`, `#/e/closing_prices` and `#/e/transfers` → `... (reading 'concat')`, `#/e/sells` →
`HTTP 404`, each rendered as the entire page body.

Reachability is the mitigating half and was checked rather than assumed: `nav.js` correctly links
`'#/' + e.custom` for a custom entity, no other module builds an `#/e/<custom>` href, and the 98-route
sweep of Y-09 found no other broken route — every route the app itself generates renders a view. So
this is reachable only by a hand-typed or stale-bookmarked URL, which is why it is logged last.

**Options offered:**
1. **Redirect to the entity's real route.**
2. Reject a custom entity in the generic branch so it renders the existing "Unknown view".
3. Dismiss as unreachable.

**Chosen: option 1.** One line in the router, and a stale bookmark lands somewhere useful rather than
on what reads as a crash.
