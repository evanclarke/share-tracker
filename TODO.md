# TODO

Items are only marked done when a passing test exists for them.

Completed and decided (out-of-scope / not-reproducible) sections are archived in the topical `DONE/*.md` files, indexed by [DONE.md](DONE.md), to keep this list focused on active work. When a section here is fully done, move it into the matching `DONE/*.md` file rather than leaving it — see CLAUDE.md.

Sections here come from the **2026-07-13 improvement review** (a whole-project pass for
operational and test-strategy gaps, as distinct from the 2026-07-12 programming/domain review whose
findings are all closed in DONE.md), except where a section's heading names another source
(e.g. REQUIREMENTS, SCENARIOS). Each section records one finding; sections land in DONE.md as they
are fixed or decided.

**Open: the SCENARIOS Z pass's findings** — see the `## SCENARIOS Z-*` sections at the end of this
file. Everything else is closed.

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

## SCENARIOS Z-a — one disposal's gain prints as two different figures on two screens

- [ ] Make a sale's own proceeds and gain the exact figures, not the sum of re-rounded shares.

Found driving **Z-01** (the 10-year ETF), whose closing sale spans 10 parcels. Measured in a real
browser on the two screens a user would compare:

| screen | what it prints for the 2026-05-15 disposal |
| --- | --- |
| Realised Gains (`#/r/realised-gains`) | proceeds `69,785.05`  cost base `39,139.98`  **gain `30,645.07`** |
| Annual Tax Report (`#/r/tax-report`) | *"Subtotal: proceeds 69,785.05, cost base 39,139.98, **gain/loss 30,645.08**"* |

The exact figure is `69,785.05 − 39,139.975 = 30,645.075`, so the tax report is right and the
Realised Gains cell is a cent low. **The same row disagrees with itself**: its discount-eligible and
non-discountable columns print `30,316.38` and `328.70`, which add to `30,645.08` — the cent the gain
cell beside them does not show. That is W-d's "printed columns do not add up", on screen this time.
The [net capital gain](docs/API.md#net-capital-gain) report agrees with the tax report.

**Mechanism.** `reports::realised_gains` never computes a sale's total: `sale_proceeds` is accumulated
from the per-allocation shares. Each share is `sale.average_price × qty_alloc − alloc_costs`, and
`alloc_costs` — the pro-rated brokerage — is deliberately a *cumulative difference* so the shares
telescope to exactly `brokerage + gst`. They do. What breaks the telescoping is the **subtraction that
follows**: `price × qty` is a large exact number and `alloc_costs` a 28-significant-digit repeating
one, so each difference is re-rounded to fit `Decimal`'s mantissa and the residues no longer cancel.
The report's own test comment names the hazard ("a larger price would re-round there") but only for
the shares, not for the total.

**Reproduced with three controls agreeing** (parcels of equal size, sale brokerage 9.95):

| case | exact proceeds | reported |
| --- | ---: | --- |
| 3 parcels × 517u @ 45.00 | 69,785.05 | `69785.049999999999999999999999` |
| 3 parcels × 517u @ **4.00** | 6,194.05 | `6194.0499999999999999999999999` |
| 3 parcels × 517u, **no brokerage** | 69,795.00 | `69795.00` ✔ |
| **1 parcel** × 1551u @ 45.00 | 69,785.05 | `69785.05` ✔ |

So it is the apportionment, not the magnitude: the drift appears whenever the brokerage share is a
repeating decimal, and disappears when there is nothing to apportion or nothing to apportion it
across. Whether it changes a *displayed* cent then depends on where the exact figure sits — Z-01's
landed on a half-cent, which is what made it visible.

**Direction.** The sale-level `proceeds` is knowable exactly (`price × quantity − brokerage − gst`,
converted once); computing it there and letting the last allocation absorb the difference keeps both
properties W-d established — the total is exact, and the per-parcel rows still sum to it.

## SCENARIOS Z-e — the archived CGT worksheet calls a bonus issue and a consolidation "splits", at ratios nobody announced

- [ ] Name each unit-count event by what it was, at the ratio its terms were stated in.

Found driving **Z-08** (the rights round trip), which ends with a 1-for-10 **bonus issue** and a 1-for-2
**consolidation** over the same parcels. The [Annual Tax Report](docs/API.md#annual-tax-report) — the
print document meant to be saved to PDF and archived — prints one `adjustments` row per event on every
disposed parcel, with a `reference` naming the action it came from. Both come out wrong:

| what was recorded | what the worksheet prints |
| --- | --- |
| `BonusIssue` 1 for every 10 held | `11-for-10 split` |
| `ShareSplit` 1 new for 2 old (a consolidation) | `1-for-2 split` |

`domain::cost_base::adjustment_detail` builds every one of these as
`format!("{}-for-{} split", s.new_units, s.old_units)` over the *derived rebase factor*, so:

- a **bonus issue** is not a split and was never announced as "11-for-10" — that factor is this tool's
  own arithmetic (10 held → 11 held), and a reader reconciling the worksheet against the company's
  announcement finds no such ratio in it;
- a **consolidation** is announced as "1-for-2" and that part is right, but calling it a *split* says
  the opposite of what happened — the parcel went from 2,200 units to 1,100.

The figures are all correct (`amount` is 0 — these rows are informational, explaining a changed unit
count, never a cost-base movement); it is the provenance label that misnames them, in the one document
that exists to be handed to someone else. `docs/API.md`'s worked example of the field is `"2-for-1
split"`, so the design only ever contemplated splits.

**Direction.** The rebase events already know which action kind they came from. Carry that through and
label each one from its own terms — `1-for-10 bonus issue`, `1-for-2 consolidation`, `2-for-1 split` —
rather than formatting one derived factor three ways.
