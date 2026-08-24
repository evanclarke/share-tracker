# TODO

Items are only marked done when a passing test exists for them.

Completed and decided (out-of-scope / not-reproducible) sections are archived in the topical `DONE/*.md` files, indexed by [DONE.md](DONE.md), to keep this list focused on active work. When a section here is fully done, move it into the matching `DONE/*.md` file rather than leaving it — see CLAUDE.md.

Sections here come from the **2026-07-13 improvement review** (a whole-project pass for
operational and test-strategy gaps, as distinct from the 2026-07-12 programming/domain review whose
findings are all closed in DONE.md), except where a section's heading names another source
(e.g. REQUIREMENTS, SCENARIOS). Each section records one finding; sections land in DONE.md as they
are fixed or decided.

**Open: the findings of the SCENARIOS AA pass, at the end of this file.**

**SCENARIOS.md sections A–Z are driven and every finding they raised is closed** in the `DONE/*.md`
archive. Only section **AA. Boundary and out-of-scope scenarios** (20 scenarios) remains. Section **S. Settlement, holidays, and dates** was driven 2026-08-22 (`d501408`) and its
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

After Z, the next SCENARIOS pass is section **AA. Boundary and out-of-scope scenarios** (20
scenarios) — each a documented limitation, where the verification is that the system **fails safe**
(refuses, or flags, or documents) rather than silently producing a wrong number, and that the
documented workaround actually works. The lessons worth carrying forward are in the handover memory.

---

# SCENARIOS AA. Boundary and out-of-scope scenarios

Section **AA** (20 scenarios) was driven on **2026-08-24** against a throwaway database through the
HTTP API. AA is a different shape from every earlier pass: each entry is a *documented limitation*,
so the verification is that the system **fails safe** — refuses, or flags, or documents — rather than
silently producing a wrong number, and that the documented workaround actually works.

**Fifteen came back correct.** A pre-CGT parcel cannot be entered by any path and the boundary is
exact — 1985-09-19 refused, 1985-09-20 accepted — with the same guard on trades, Sells, ESS taxing
points and inheritances, and the one legitimate pre-CGT interaction (`MarketValueAtDeath`) left open
while `DeceasedCostBase` with a pre-CGT acquisition is refused (AA-01); rights over pre-CGT originals
are unreachable by construction, since every anchoring parcel must be a post-CGT Buy/DRP (AA-14).
The **individual-resident basis is stated on every report that applies the rate** — realised gains,
net capital gain, tax summary, the parcel optimiser, the pre-sale what-if's year rows, and the
archived Annual Tax Report's `meta` (AA-05). **Partial DRP participation is refused and its
documented workaround works exactly as written**: the partial allotment as reinvest `units` is
`422` naming both figures, the split-into-two-rows workaround costs the parcel at the cash actually
applied, and both stated caveats hold — the per-share cross-check left on a half is itself a `422`,
and an exactly half-and-half split does trip `duplicate_income` (AA-10). **K10/K11 is reported on
the data, not only in a paragraph**: a USD trade contracted 2025-01-30 and settling 2025-02-03 raises
FX coverage's `settlement_crosses_rate_month` with a detail sentence naming the omission (AA-11). A
collectable, a personal-use asset, a main residence or any non-listed asset has **no `security_type`
to arrive under** — the enum is five listed kinds plus `Crypto`, and a non-`Crypto` listing must
carry an `exchange_mic` (AA-04, AA-18). The estate/LPR side (AA-15) and unvested ESS grants with
their dividend equivalents (AA-16) are documented in unusual detail, and the recordable half works:
a dividend equivalent entered as `EmploymentIncome` reports on its own `employment_income` line and
in no dividend or investment-income total. Crypto chain splits and wrapping enter exactly as
documented — a nil-price Buy dated the split, and a market-value Sell plus Buy (AA-17). The reduced
cost base is identical to the cost base by construction, since element 3 is the only excluded element
and it is not recordable (AA-09). The related-payments rule and the 30%-at-risk test are documented
with the honest statement that what the walk denies is a floor, not the whole denial; an
entity-level franking deficit has no shareholder-side consequence for the modelled taxpayer (AA-20).

It raised **five findings**, logged below.

## SCENARIOS AA-a — an indexation-eligible parcel is silently costed on the discount, and the reason given for not modelling it is false for a wide, enterable range

- [x] Decide and implement (options below).

Scenario AA-02. `docs/API.md`'s [Known limitations](docs/API.md#known-limitations) justifies the
scope cut this way:

> **Indexation method** (2026-06-10) — for an asset acquired before **21 September 1999** an
> individual may index the cost base for inflation (frozen at the 30 September 1999 CPI) *instead of*
> applying the 50% discount … The discount **almost always** gives an individual the better result,
> so indexation is not modelled — the 50% discount is used throughout.

**"Almost always" is not true of the parcels this system can actually hold.** The earliest
enterable acquisition is **1985-09-20** (AA-01's own floor), and the indexation factor for a
September 1985 quarter cost is 68.7 ÷ 39.7 ≈ **1.731** — so indexation wins whenever the proceeds
are below about **2.46 × cost**, which over a forty-year hold is an ordinary outcome, not an edge
case. (The ATO's own page says only "in most cases", and adds the loss caveat that widens the range
further: `docs/ato/indexing-the-cost-base.md`, "Indexation may give you a better result in some
situations, such as if you also have capital losses.")

Driven: a parcel bought 1985-09-20 for A$10,000, sold 2025-06-02 for A$20,000.

| method | assessable gain |
| --- | ---: |
| 50% discount (what the system reports) | **A$5,000.00** |
| indexation (10,000 × 1.731 = 17,310 indexed cost) | **A$2,690** |

`GET /portfolio/net-capital-gain` reported the year's `cgt_discount` of `5450.00` and
`net_capital_gain` of `5450.00` with this parcel's A$5,000 inside it. **Nothing anywhere names the
alternative** — no report field, no cross-check row, no health entry mentions indexation for a
directly held parcel. (The word is not absent from the tree: an AMMA statement already carries
`cgt_indexation_gains`, so the *trust* side of the indexation method is modelled and reported while
the taxpayer's own election is invisible.)

**This does not compute a wrong number** — the discount method is a lawful choice, so the reported
figure is defensible — but the taxpayer is never told a cheaper lawful choice existed, and the
documented reason for withholding it is wrong for exactly the parcels most likely to be affected.

> **Note for whoever fixes this:** re-derive the September 1985 CPI (39.7) and the factor from the
> ATO's own published table rather than from this write-up — per the standing lesson, a finding's
> arithmetic is not evidence. The rounding rule is "limited to 3 decimal places, round the fourth
> decimal up from 5".

**Options.**

1. **Flag, don't compute.** Mark every disposal parcel acquired before 21 September 1999 as
   indexation-eligible on the realised-gains / net-capital-gain / annual-tax-report parcel rows, and
   add a cross-check (or health) row naming each affected disposal with the two figures side by side
   so the taxpayer can see which method wins. Correct the Known-limitations wording to state the
   actual boundary rather than "almost always". Cheapest honest fix; the arithmetic stays the
   taxpayer's own adjustment, exactly as K10/K11 does — and K10/K11's `settlement_crosses_rate_month`
   is the existing precedent for reporting an omission on the data.
2. **Model it.** A frozen ATO quarterly CPI table (seeded, ~56 rows to September 1999), an indexed
   cost base through `domain::cost_base`, and a per-parcel election reported both ways with the
   better taken. Substantial: indexation is forbidden on a capital loss and cannot be combined with
   the discount, so the net-capital-gain loss-netting walk has to choose per parcel, and the choice
   interacts with the brought-forward loss chain.
3. **Documentation only.** Correct the "almost always" claim and state the crossover, add nothing to
   any report.

**Chosen: option 1 — flag, don't compute.** Clarified 2026-08-25: *both figures side by side*. The
frozen ATO quarterly CPI table is seeded and an indexed cost base computed, so the advisory row can
show the two methods against each other — but **no reported tax figure changes**: the net capital
gain, the annual tax report and every CSV export stay on the 50% discount throughout. The indexed
figure exists only to answer "which method wins here", which is the question the finding is about.

**Fixed.** The frozen ATO quarterly CPI series is seeded as `cpi_quarters` (migration
`0046_cpi_quarters.sql`, 57 rows — the September 1985 quarter through the September 1999 freeze and
deliberately nothing after it), mirrored from Appendix 2 of the *Guide to capital gains tax 2025*
(QC 104764) in [`docs/ato/consumer-price-index.md`](docs/ato/consumer-price-index.md) and indexed in
`docs/ato/OVERVIEW.md`. `domain::indexation` holds the method's arithmetic — the eligibility
boundary, the quarter mapping, the factor (68.7 ÷ the quarter's CPI, limited to 3 decimal places
with the fourth decimal rounded up from 5), and the indexed cost base — and
`domain::cost_base::CostBase` gained `costed_initial_cost`, the costed units' share of the initial
cost base, which is the only *indexable* component and cannot be reconstructed from the netted
total once CGT event E10/G1 has floored it at nil. `reports::realised_gains` computes the indexed
figure per allocation and carries it on the parcel rows as `indexation_eligible` +
`indexed_cost_base`, the annual tax report notes it under an eligible parcel on the archived
document, and `GET /reports/indexation_cross_check` sets both methods' assessable gains against
each other per parcel and per year. **No reported tax figure moved**: the same disposal driven
through both builds — pre-change and post-change, same facts, same throwaway database — answers
byte-identical net capital gain, realised gains, tax summary and annual tax report once the two new
advisory fields are stripped.

**The finding's arithmetic was wrong in one place, and it is the place it warned about.** The
September 1985 factor is **1.730**, not 1.731: 68.7 ÷ 39.7 = 1.730478…, whose fourth decimal is a 4,
so the ATO's "round the fourth decimal up from 5" rounds it *down*. 1.731 is what the **superseded
1989-90-base** series gives (123.4 ÷ 71.3 = 1.73070…), and the ATO marks that table "no longer
[usable] for tax and super purposes". So the finding's A$2,690 indexed gain is really **A$2,700**
(A$10,000 → A$17,300 indexed). The **2.46× crossover was right** — with a 1.730 factor indexation
wins below exactly 2.460 × cost, driven at A$24.59 / A$24.60 / A$24.61 per unit against a A$10 cost
and answering Indexation / Equal / Discount. Both of the ATO's own worked-example factors (Val:
1.164 and 1.159) reproduce against the seeded table, which is what says the table *and* the rounding
rule are right rather than merely self-consistent.

**How the comparison is stated, and why.** Per **parcel allocation**, and explicitly *before any
capital losses applied against the gain* — stated in the module doc, in `docs/API.md`, and on every
year's own `comparison` row so the qualifier travels with the figures into a printout. Per parcel
because that is the only level at which it is a fact rather than an assumption: a parcel is a
separate CGT asset, and one Sell can draw on a 1998 parcel and a 2015 one whose methods differ.
Before losses because the two methods do not meet losses at the same point — losses come off the
gross gain and the discount applies to what is left, while an indexed gain has no discount to follow
— so writing `g` for the gross gain, `r` for the indexation uplift and `L` for the losses applied,
indexation's advantage is `r − (g − L) / 2`, which **rises** with `L` until `L` reaches `g − r` and
both methods reach nil together. Applying losses therefore never moves the answer toward the
discount, which makes every row a **floor** on indexation's case rather than the whole answer; each
year row carries the capital losses the year actually realised so a reader can see whether the
qualifier bites. Two exclusions decided rather than assumed: a parcel disposed of at a **loss** is
left out entirely rather than shown as "discount wins" (indexation cannot be used on a capital loss
at all — its loss still reaches its year's `capital_losses_realised`), and a **rights sale** is left
out because what would be indexed is the rights' own cost base, nil for the free rights modelled
here. Eligibility is tested on the parcel's own **trade date** — when the cost was incurred — never
on the deemed acquisition date the discount clock runs from, since an inherited or
rollover-replacement parcel has its own indexation rules and none of them are modelled. Costs
incurred after the cut-off cannot arise on the indexable side: the AMIT (E10) and return-of-capital
(G1) movements are *reductions*, not costs, so they come off the indexed figure at face value (the
conservative direction), and a disposal's own brokerage is netted from proceeds rather than added to
the cost base.

`cpi_quarters` is classified **exempt** for snapshot staleness (nothing writes it after its
migration, and no snapshotted report reads it) and deliberately **not audited** (`row_history` exists
to recover a user's edit of a financial fact; this table has no write path at all — contrast
`exchange_holidays`, audited precisely because it has a DELETE route). The *Indexation method*
Known limitation is rewritten: the "almost always" claim is explicitly withdrawn and replaced by the
1.730 factor and the 2.460 × cost crossover, with the scope cut narrowed to the **election** —
choosing per parcel, and that choice's interaction with the loss-netting walk — which stays out of
scope. `docs/API.md` gains the report's own section, `docs/SCHEMA.md` the table and its
relationships entry, `README.md` a feature line and a corrected scope clause, and `src/doc_checks.rs`
two pins (one of which asserts "almost always" survives in exactly one place: the sentence
withdrawing it).

## SCENARIOS AA-b — a non-renounceable rights issue is indistinguishable from a renounceable one, and its retail premium is recorded as a capital gain

- [x] Decide and implement (options below).

Scenario AA-13. The two treatments of a retail premium turn entirely on whether the offer was
renounceable, and `docs/ato/retail-premiums.md` states the split plainly: under a **renounceable**
offer the premium is capital proceeds on the rights (TR 2017/4), and under a **non-renounceable**
offer it is an **unfranked dividend** (TR 2012/1) — "enter it as unfranked dividend `income` against
the listing, not as a corporate action or rights sale."

**The `RightsIssue` corporate action records no such fact.** Its fields are `rights_units`,
`rights_held_units`, `exercise_price` and `currency` — there is no `renounceable` column anywhere in
the tree (`grep -ri renounce src` finds only prose). A non-renounceable entitlement offer is a
perfectly legitimate thing to record, because *exercising* one is identical either way and the
exercise path is the reason to enter the action at all. Having entered it, `sell_rights` is offered,
and it accepts:

```
PUT  /corporate_actions/1  {"action_type":"RightsIssue", ... }          → 204
POST /corporate_actions/1/sell_rights
     {"units":"250","proceeds_per_right":"0.20", ... }                  → 201
```

A$50 of retail premium is now a **capital gain** — halved again if the anchoring parcel is past
twelve months, since free rights inherit the original shares' acquisition date — where TR 2012/1
makes it fully assessable unfranked dividend income at item 11S. Wrong amount and wrong return label,
with nothing asked and nothing said. The endpoint's own documentation says "under this
(**renounceable**) offer" and the UI's action description says "under this renounceable offer" — both
*assume* the fact neither collects.

**Options.**

1. **Record it and refuse the wrong path.** Add `renounceable: bool` to the `RightsIssue` action
   kind (defaulting existing rows to renounceable, which is what every stored row means today), and
   have `sell_rights` refuse `422` on a non-renounceable offer when `proceeds_per_right` is positive
   — naming TR 2012/1 and pointing at the income path. A **nil**-proceeds lapse stays accepted: a
   non-renounceable right can lapse, and at nil/nil it is a non-event either way.
2. **Record it and flag it.** Add the column and surface a cross-check row for every rights sale
   with positive proceeds against a non-renounceable offer, refusing nothing.
3. **Documentation only.** Add a Known-limitations line saying the action assumes a renounceable
   offer, and that a non-renounceable premium is entered as unfranked income instead.

**Chosen: option 1 — record `renounceable` and refuse the wrong path.**

**Fixed.** `ActionKind::RightsIssue` carries `renounceable` (migration 0047: an INTEGER 0/1 column,
CHECK-confined to `RightsIssue` rows, every stored row backfilled to renounceable — what they all
already meant — with `corporate_actions`' two row-history triggers dropped and re-created around it).
It is **required** on the PUT body and forbidden on every other action type, not defaulted: the
whole finding is that the fact was never asked for, and a quiet default would have left the same
assumption in place for every new entry. (The complementary CHECK — a rights issue *must* carry the
flag — is the one part SQLite cannot express by `ALTER TABLE ADD COLUMN`, since it evaluates a new
CHECK against the rows already there and would reject the very rows 0047 backfills; it lives in
`CorporateActionBody::kind`, beside this table's other write-time rules a CHECK cannot express, and
the migration header says so.)

`sell_rights` now refuses **two** things against a non-renounceable offer, both `422`: a positive
`proceeds_per_right` (naming TR 2012/1 and pointing at unfranked dividend income) and a positive
`rights_cost`. The second was the open question in the write-up, and the ruling answers it: TR
2012/1's scheme is defined by entitlements that "**cannot be traded, transferred, assigned or
otherwise dealt with**" (para 2), so nothing can have been *bought* either — an unchecked cost would
realise a capital loss on the lapse out of an amount that was never paid. A **nil/nil lapse stays
accepted** and still consumes the entitlement. The premise held up too: exercising is identical
under both offers (`docs/ato/rights-issues.md`'s rules turn on how the rights were acquired and on
the original shares' pre/post-CGT status, never on renounceability), so recording a non-renounceable
issue in order to exercise it is the normal case, and `rights_exercise` was deliberately left alone
with a comment saying why.

Re-derived rather than taken from the write-up, and it moved two things: QC 21832 had been
restructured by a 22 June 2026 update, so `docs/ato/retail-premiums.md` was re-fetched in full
(2026-08-25) and the drift recorded in `docs/ato/OVERVIEW.md`; and TR 2012/1 itself was fetched and
quoted into that mirror, which is where the non-tradeable definition and paras 9–11 come from — the
premium is **not** partly capital, since CGT event C2 does happen on the right to it but s 118-20
reduces the gain by whatever is assessed as income. One documented caveat the finding did not have:
TR 2012/1 expressly does not consider entitlement offers over **trust or stapled-group** equity, so
for those the refusal still holds (the entitlements are non-tradeable either way) but the payment's
character is whatever the distribution statement says.

`docs/SCHEMA.md`, `docs/API.md` (the `RightsIssue` description, *Selling or lapsing rights* with its
new `422`, and the Known-limitations bullet, which no longer says non-renounceable premiums "are not
modelled" — the wrong path is refused and the right one named) and `README.md` were updated with it,
and `src/web/config.js` asks for the flag (a checkbox defaulting to renounceable) and stops
asserting "under this renounceable offer" — the sell-rights screen of a non-renounceable issue now
opens by saying only a lapse can be recorded there. Tests: the two refusals and the accepted lapse
(DB and API level, the API one pinning the wording), a renounceable offer unaffected, both
round-trips through `PUT`, the flag required/forbidden, and `migration_0047_…` proving an existing
row reads back as renounceable through the model.

### AA-b, second item — the exercise path still accepts a cost on a non-renounceable offer

- [x] Refuse `rights_cost` on `POST /corporate_actions/:id/exercise` for a non-renounceable offer.

Flagged by the agent that fixed AA-b and deliberately left unenforced there; **decided 2026-08-25 to
refuse it now.** `sell_rights` refuses a positive `rights_cost` on a non-renounceable offer because
TR 2012/1 para 2 defines the scheme by entitlements that "cannot be traded, transferred, assigned or
otherwise dealt with" — so nothing can have been paid to acquire them. The same fact makes the amount
impossible on the **exercise** path, which still accepts it. The consequence is smaller and more
visible than `sell_rights`' was (a stray cost inflates the new parcel's cost base rather than
fabricating a capital loss out of money never paid), but it is the same impossible amount, and the
guard should not hold on one path and not the other.

**Fixed.** `db_exercise` now reads the offer's `renounceable` flag and returns `422` on a positive
`rights_cost` against a non-renounceable one, before anything is written. The premise was re-derived
first and held: `docs/ato/rights-issues.md`'s exercise rules give `rights_cost` the *same* meaning on
both paths — the cost base of the rights at exercise, "including any amount you paid for them" —
and `docs/API.md` says it covers rights **bought on-market**, which TR 2012/1 para 2's entitlements
(not tradeable, transferable or assignable) cannot be. The para 3 caveat does not open a legitimate
case: the Ruling declines to characterise the *payment* on trust/stapled-group offers, but a
non-renounceable entitlement is non-tradeable there too, so the cost stays impossible; and nothing
legitimate is blocked, since the exercise itself is unaffected — at the nil cost a free entitlement
carries it lands exactly as before.

The predicate is shared as far as it should be: the *fact* both refusals rest on now lives once, as
`corporate_action::NOTHING_PAID_FOR_NON_RENOUNCEABLE_RIGHTS`, and each `From<EntityError> for
ApiError` arm follows that clause with what the amount would have done to *its* figures (a capital
loss on a lapse for `sell_rights`; an inflated parcel cost base here) — so the two read as one rule
in two places without either call site being contorted into the other's shape. The check itself is
two tokens against a flag each path already has in hand and was not worth a helper.

`docs/API.md` (the exercise section's new `422` and its `rights_cost` description, the `RightsIssue`
description, the *Selling or lapsing rights* cross-reference that used to say the exercise path was
deliberately untouched, the Known-limitations rights bullet, and the response-codes catalogue),
`README.md`'s rights-issue line, and `src/web/config.js` (the exercise screen's description and
rights-cost hint, mirroring the sell-rights screen) carry it. No migration: the fact was already
recorded by 0047. Tests: the refusal and a nil-cost exercise of the same offer (DB and API level, the
API one pinning the wording and that only the nil-cost exercise was written), plus a renounceable
offer with a positive cost still costing the parcel `500.05`.

## SCENARIOS AA-c — the investor-not-share-trader assumption is stated nowhere

- [x] Decide and implement (options below).

Scenario AA-07. Every figure this system produces assumes the holdings are **CGT assets held on
capital account**. For a **share trader** carrying on a business, shares are **trading stock**: gains
and losses are ordinary income and deductions, there is no CGT event, no 12-month discount, no
capital-loss pool and no 18V carry-forward, and closing stock is valued at year end instead.

`docs/API.md`'s Known limitations has **32 bullets and none of them says this.** The closest,
*Taxpayer entity type*, is about a different axis entirely — individual vs SMSF vs company vs trust,
and the *rate* of the discount — and a share trader is very often exactly the individual resident
that bullet describes. `grep -in "trading stock\|share trader" docs/API.md README.md` returns
nothing; the phrase appears only inside a mirrored ATO page (`docs/ato/worthless-shares.md`'s G3
eligibility list).

This is the one boundary in section AA with **no documented limitation behind it at all**, and the
consequence is not a rounding: a trader who used this tool would lodge capital gains — half of them
discounted away — where ordinary income belongs, and carry forward capital losses that should have
been deductions.

Nothing can detect it, so a refusal or a flag is not available; the fix is that the assumption is
written down.

**Options.**

1. **Its own Known-limitations bullet** ("Investor, not share trader"), a README scope line beside
   the other named scope cuts, and a `doc_checks` test pinning both.
2. **Fold it into the existing *Taxpayer entity type* bullet** as a second paragraph, pinned the
   same way.

**Chosen: option 1 — its own Known-limitations bullet, README line, and `doc_checks` test.**

**Fixed.** `docs/API.md`'s Known limitations gained its own **Investor, not share trader**
(2026-08-24) bullet, placed directly after *Taxpayer entity type* with the two cross-referenced so
neither reads as covering the other (that one is *which taxpayer* and the rate; this one is whether
the CGT machinery applies at all). It states the trading-stock treatment concretely — profit on sale
assessable as ordinary income, purchase price and transaction costs deductible in the year incurred
so there is no per-parcel gain at all, losses deductible against income from any source, and
Division 70's year-end stock adjustment (s 70-35 / s 70-45) not modelled anywhere — what that costs
a trader who ran these reports (half each year's profit exempted as a discount at 18A, losses parked
at 18V instead of claimed, brokerage capitalised that was already deductible), and that **nothing can
detect it**: the test is how the activities are carried on, not anything on a trade row, so there is
no refusal, no health flag and no `taxpayer_basis`-style marker — the assumption is written down
instead of enforced. Two things the drafting of this finding did not have: the income side is
**unaffected** (dividends and similar receipts are assessable either way, so the tax summary's
dividend, franking and expense lines hold for a trader too), and 18V is not simply unavailable to a
trader — an investor→trader change keeps unused prior-year capital losses as **capital** losses,
which can never become revenue losses. The change-of-status rules are named as out of scope too
(**CGT event K4** where the change elects market value; the trader→investor deemed sale at cost),
with the manual Sell + Buy that is all an entry path could be.

The ATO source was re-derived rather than taken from the finding: QC 66047 *Share investing versus
share trading*, mirrored as `docs/ato/share-investing-versus-share-trading.md` (retrieved
2026-08-24) with its investor/trader table, carrying-on-a-business factors, the George example and
both change-of-status paths, and indexed at the head of `docs/ato/OVERVIEW.md`'s CGT table as the
threshold question the rest of it sits on. `README.md`'s scope-cut paragraph carries the matching
line, and `doc_checks::known_limitations_document_the_investor_not_share_trader_assumption` pins
both halves plus the mirror's QC number and its two load-bearing sentences.

## SCENARIOS AA-d — a disposal recorded at nil proceeds raises a capital loss that nothing questions

- [x] Decide and implement (options below).

Scenario AA-03. A gift of shares is a CGT disposal at **market value** under the market-value
substitution rule, and `docs/API.md` documents the entry convention: "enter a gift out as a manual
Sell at market-value proceeds". The failure mode the convention exists to prevent is entering what
was actually *received* — nothing — and that entry is accepted in full:

```
PUT /sells/71  {"average_price":"0","quantity":"1000", ...}   → 204
```

`GET /portfolio/realised-gains` then reports `proceeds: 0`, `cost_base: 20000.00`,
`capital_loss: 20000.00`. **A$20,000 of capital loss that does not exist**, feeding the net-capital-
gain netting and the 18V carry-forward. The health report is silent — its only non-empty lists after
the write were `unpriced_days` and an unrelated `duplicate_income`.

The system cannot know a nil-proceeds Sell is a gift, so this is a flag rather than a refusal — and
a nil-proceeds disposal is a genuinely unusual shape worth naming, in the way `duplicate_trades` and
`non_trading_day_trades` already are. (The one legitimate nil-proceeds disposal — worthless shares —
has its own operation and writes a Sell carrying `worthless_action_id`, so it is distinguishable; a
crypto burn is the residual honest case.)

**Options.**

1. **A health check** — `nil_proceeds_disposals`, listing every ordinary Sell and rights sale
   recorded at nil proceeds (excluding the operation-written closing Sells), with the
   market-value-substitution rule as its reason. Advisory, blocks nothing.
2. **Documentation only** — extend the *Gifts / off-market related-party transfers* bullet to warn
   that entering the nil consideration actually received fabricates a capital loss, and say so on
   the Sells screen.
3. **Out of scope** — a nil-proceeds disposal is legitimate often enough (a crypto burn, an
   abandonment) that naming it would be noise.

**Chosen: option 1 — a `nil_proceeds_disposals` health check.**

**Fixed.** `reports::health` now carries `nil_proceeds_disposals`: every ordinary Sell at a zero
`average_price`, plus every rights disposal at a zero `proceeds_per_right` whose rights were **paid
for** (`rights_cost > 0`) — a *free* right lapsing is nil against nil, the non-event `docs/API.md`
describes, and flagging it would fire on every ordinary lapse. The test is the *price*, not the
netted proceeds: a real price a brokerage happens to cancel is arithmetic, not a nil-consideration
disposal, and the market-value substitution rule has nothing to say about it. Advisory, blocking
nothing, with the rule (`docs/ato/capital-proceeds-market-value-substitution.md`, QC 66021) as its
reason, a cross-view banner sentence linking to Sells / Rights Sales, and the Gifts limitation,
the Health field list and the README feature line all updated (pinned by
`doc_checks::nil_proceeds_disposals_are_documented_with_the_market_value_rule`).

The exclusion of the operation-written closing Sells is the part that needed the rule rather than a
list. There were already **three** transcriptions of the provenance columns (the two guards in
`entities::sell`, and the write-path `CASE` in `non_trading_day_trades`), so the exclusion became
`entities::trade::provenance` — one list of (column, plain-English write path) with two SQL
builders over it, `operation_written_sql` and `source_case_sql`, and a test that reads the live
schema's foreign keys on `trades` and fails on one that is neither classified as a provenance link
nor named as ordinary trade data with the reason. A future operation's column is picked up by both
callers with no edit. `non_trading_day_trades` now builds its label from the same list, which also
fixed a mislabel it carried: a crypto transfer's network-fee Sell (linked from
`transfers.fee_sale_trade_id`, not `trades.transfer_id`) read as `entered directly`.

## SCENARIOS AA-e — four limitations are documented without the workaround that exists and works

- [x] Decide and implement (options below).

Scenarios AA-06, AA-08, AA-12, AA-19. Each of these bullets states what is *not* modelled and stops
there, while a correct entry convention exists — and in three of the four it was driven and works.
The pattern the file already uses elsewhere is the opposite: the *DRP partial participation*,
*Gifts*, *Rollovers assume the rollover was chosen* and *multi-year expense* bullets each name their
workaround, and the *Inherited parcels* bullet even prescribes the "enter your own share" convention
that AA-06 needs.

- **AA-06, joint ownership** — *One taxpayer* says the ownership dimension is not modelled and gives
  no remedy. Driven: a 50% joint interest entered as **your own half** — 500 units at $10 — costs
  A$5,000 and reports correctly throughout, with `amount_per_security` / `securities_held` keyed to
  your half rather than the registry statement's whole. This is the same convention *Inherited
  parcels* already prescribes for a parcel split between beneficiaries.
- **AA-08, cost-base elements** — the bullet reads "elements 1 (acquisition) and 2 (incidental costs:
  brokerage + GST) **are captured**", which over-states element 2: the ATO's element 2 also covers
  stamp duty, transfer costs, and remuneration for professional advice on the acquisition, none of
  which has a field. Driven: A$500 of off-market transfer stamp duty entered as `brokerage` with the
  reason in `contract_note_ref` lands in the cost base exactly (100 units at $10 plus $500 →
  A$1,500). It is the right answer arithmetically and is documented nowhere.
- **AA-12, Div 775 forex on a foreign-currency cash balance** — documented only as a **clause inside
  the *Crypto assets* bullet** ("Foreign-currency cash balances (Div 775 forex gains — ordinary
  income, not CGT) are deferred to a separate specification"), where a reader looking for
  foreign-currency scope will not find it. And unlike the others there is **no** workaround: an
  [income](docs/API.md#income) row requires a `listing_id`, and a cash balance has no listing, so a
  Div 775 gain has nowhere to be entered at all. The doc does not say so.
- **AA-19, a second taxpayer** — *One taxpayer* again, with no remedy stated. The remedy the tool
  already supports is **one database and one instance per taxpayer** (`--db`, `--port`), which is
  worth naming precisely because the wrong answer is so easy: a spouse's holdings entered as a second
  holding account aggregate into one net capital gain, one loss pool, one A$5,000 franking threshold
  and one A$1,000 FITO de-minimis, silently wrong for both people.

**Options.**

1. **Add the workaround to all four bullets**, each pinned by a `doc_checks` test — including
   AA-12's honest "there is no entry path", moved out of the Crypto bullet into one of its own.
2. **A subset** — say which.

**Chosen: option 1 — all four, with AA-12 promoted out of the Crypto bullet into one of its own.**

**Fixed.** All four bullets in `docs/API.md`'s Known limitations now carry their convention, each
pinned by its own `doc_checks` test, and every claim was re-driven against a throwaway database
before it was written down.

*One taxpayer* was rewritten once and carries both remedies (AA-06 and AA-19 live on the same
bullet, and splitting them would have left two halves each needing the other's context). **A second
taxpayer is a second database and a second instance** — one server per taxpayer, `--db` and `--port`
apart — with the wrong answer named: a spouse entered as another holding account aggregates into one
net capital gain, one loss pool, one A$5,000 small-shareholder franking threshold and one A$1,000
FITO de-minimis, wrong for both people and **not in one predictable direction** (pooled losses
understate one person's gains, while a combined franking or foreign-tax total tips both out of
thresholds neither reached alone — the finding's drafting had only the understating half). Nothing
can detect it, because aggregating what is in the database is what the reports are for. **A jointly
held parcel is entered as your own share** — 50% of a 1,000-unit holding is a 500-unit Buy, costing
A$5,000, verified through `/portfolio/open-parcels`. One correction to the finding's write-up: it is
**not** both per-share figures that are keyed to your half. `amount_per_security` stays the
statement's per-unit rate and only `securities_held` is your own unit count — the cross-check is
`amount_per_security × securities_held` against the entered cash, so $0.20 × 1000 against your half's
$100 is the `422` and $0.20 × 500 is accepted. Cross-referenced to *Inherited parcels*, with the cost
stated: your unit counts deliberately will not tie back to the registry's holding statement.

*Cost base elements* no longer claims element 2 is captured: element 1 has a field and element 2 has
**one** field. The other element-2 costs were re-derived from `docs/ato/cgt-cost-base.md` rather than
from the finding — costs of transfer, stamp duty or other similar duty, remuneration for a broker,
agent, accountant, consultant or legal adviser (tax advice only from a recognised tax adviser,
incurred after 30 June 1989), a valuation or apportionment made to work out the gain, and expenses
incurred as a direct result of ownership ending — with the ones a *listed-share* investor actually
meets named. The convention (fold it into `brokerage`, say what it was in `contract_note_ref`) is
exact: 100 units at $10 plus A$500 of transfer duty reports a A$1,500 cost base, and nothing bounds
the fee above. Driving it turned up **two traps the finding did not have**, both now documented and
pinned: `brokerage_includes_gst` would ÷11-split a A$500 duty into A$45.45 of GST that never existed
(the cost base is identical, the GST column is not), and a supplied `statement_total` is reconciled
against `quantity × price ± (brokerage + GST)` at write time, so a broker-note total that omits
separately paid duty is a `422` naming the computed figure. The disposal-side asymmetry points at
*Where a Sell's brokerage and GST land*, and the one thing with no home at all — an element-2 cost
belonging to no single trade — is stated.

*Div 775 forex on a foreign-currency cash balance* is now its own bullet, sited immediately after
*Settlement-window forex — CGT events K10/K11*, and says plainly that **there is no entry path at
all**: an [income](docs/API.md#income) row's `listing_id` is a required `i64` (verified —
`IncomeBody`, and both a missing and a `null` one answer `422`) and a currency balance has no
listing, so the gain has nowhere to go; the loss side is no better, since an investment expense would
report a forex loss on a line it does not belong to. Cited to `docs/ato/forex-common-transactions.md`
(QC 18322, s 775-15). The *Crypto assets* bullet keeps its load-bearing half — the deferral never
reaches a crypto holding, TD 2014/25 and the 2023 statutory exclusion — and the two bullets now
cross-reference each other instead of one carrying the other's scope.

`README.md`'s scope-cut paragraph gained two clauses (one taxpayer per database, and the
foreign-currency cash balance with no entry path); the element-2 convention stayed out of it as
entry-convention detail rather than a scope cut. Four new tests in `src/doc_checks.rs`:
`known_limitations_document_the_joint_ownership_entry_convention`,
`..._the_second_taxpayer_remedy`, `..._the_element_two_incidental_cost_convention`, and
`..._the_division_775_forex_omission`.

## SCENARIOS AA-f — the archived CGT worksheet prints a whole parcel's initial cost against a part of it

- [x] Decide and implement (options below).

Reported in passing by the agent fixing [AA-a](#scenarios-aa-a), and **re-driven from scratch against a
throwaway database before being logged** (per the standing lesson: a fixing agent's incidental report is
re-derived, not taken on trust). The reproduction is real and the mechanism is as reported.

`reports::tax_report` takes `CostBase::initial_cost` — the **whole parcel's** figure — for a disposal
row's `initial_cost_base_aud`. Sell 500 units of a 1,000-unit A$10 parcel and the Annual Tax Report's
disposal schedule prints:

| Units | Buy price | Initial cost base (AUD) | *(adjustment rows)* | Adjusted cost base (AUD) |
| ---: | ---: | ---: | --- | ---: |
| 500 | 10.00 | **10,000.00** | *(none)* | **5,000.00** |

with `cost_base_per_unit_aud` of `10.00` beside it. A hand-checker multiplies 500 × $10, gets $5,000,
and finds an "Initial cost base" of $10,000 with **nothing between the two columns explaining the
difference** — which is precisely the contract `docs/API.md` states for this section:

> the initial cost base and, **itemised underneath it, one row per cost-base adjustment** … with its
> own date, reference, and per-unit figure

**Bounded with its control**: a disposal of the *whole* parcel prints correctly (1,000 units → initial
`10,000.00`, adjusted `10,000.00`). The fault appears only on a **partial** disposal, which is the
ordinary case for any holding sold down in tranches.

**No tax figure is wrong.** `initial_cost_base_aud` is a display column, not one of the five the
section totals — the subtotal, the gain and the discount all take the adjusted figure. But this is the
print document meant to be saved to PDF and archived, and a column that does not reconcile against the
units beside it is exactly the class of fault [W-c](DONE/reporting.md) and [W-d](DONE/reporting.md)
were about: *a column has to add up on the page*.

The AA-a commit (`369e040`) added `CostBase::costed_initial_cost`, which is precisely the figure this
row wants, so option 1 is a small change — but it changes a **printed number**, which is why it was not
folded into that commit.

**Options.**

1. **Print the costed units' initial cost.** `initial_cost_base_aud` becomes `costed_initial_cost`, so
   the row reads 500 units / initial `5,000.00` / adjusted `5,000.00`, and where adjustments exist they
   account for the whole of the gap — restoring the documented contract. A previously archived PDF will
   disagree with a freshly generated one for the same year in this one column.
2. **Keep the figure, fix the label.** Rename the column to say it is the parcel's, not the disposal's
   (and say so in `docs/API.md`), leaving archived documents reconcilable against new ones.
3. **Out of scope** — a display column that no total depends on.

**Chosen: option 1 — print the costed units' initial cost.**

**Fixed.** `reports::tax_report`'s disposal row now takes `CostBase::costed_initial_cost` — the costed
units' pro-rated share of the parcel's initial cost base, the same pool `cost_base::adjustment_detail`
starts its itemised walk from — instead of the whole parcel's `initial_cost`. The reproduction row now
prints 500 units / initial `5,000.00` / adjusted `5,000.00`, and the whole-parcel control is unmoved at
1,000 / `10,000.00` / `10,000.00`. With real adjustments present the documented contract holds as an
arithmetic identity: a 400-of-1,000-unit disposal of an A$10 parcel carrying a 50c/unit AMIT reduction
prints initial `4,000.00` − `200.00` = adjusted `3,800.00`, and the same parcel carrying a 25c/unit
return of capital prints `4,000.00` − `100.00` = `3,900.00` (the identity holds except where a row is
flagged `capped` and CGT event E10/G1 has floored the balance at nil — the excess is a capital gain in
the net-capital-gain report, not a cost-base movement). The rest of the row was swept for the same
fault and is correct: `adjusted_cost_base_aud`, `proceeds_aud`, `gain_loss_aud` and the two per-unit
figures all come from the allocation (`realised_gains::ParcelDetail`), the itemised adjustment amounts
and per-unit figures are already stated for the costed units, and `indexed_cost_base_aud` was built on
`costed_initial_cost` from the start (`domain::indexation::indexed_cost_base`). The one other
whole-parcel figure is `buy_brokerage`/`buy_gst_on_brokerage` — deliberately the buy contract note's
own figures for the whole trade, transcribed for checking against the note, carried in the JSON and
printed in no column; that is now said in the field's doc comment and in `docs/API.md` rather than left
to be inferred. Rounding is untouched (the column was already in `round_money_to_cents`'s list), and
nothing totalled moved: driven end to end at the HTTP surface against a throwaway database, the whole
document — every subtotal and grand total included — is byte-identical to the pre-fix binary's apart
from this one column. Because it changes a printed number, `docs/API.md` says so where the column is
described, so a reader comparing an archived PDF against a freshly generated one is not left guessing.
Regression tests:
`reports::tax_report::tests::api_a_partial_disposal_prints_the_disposed_units_initial_cost_base` (the
partial disposal and the whole-parcel control in one document) and
`api_the_itemised_adjustments_span_the_whole_gap_on_a_partial_disposal` (the identity, over both
reduction kinds).
