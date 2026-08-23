# TODO

Items are only marked done when a passing test exists for them.

Completed and decided (out-of-scope / not-reproducible) sections are archived in the topical `DONE/*.md` files, indexed by [DONE.md](DONE.md), to keep this list focused on active work. When a section here is fully done, move it into the matching `DONE/*.md` file rather than leaving it — see CLAUDE.md.

Sections here come from the **2026-07-13 improvement review** (a whole-project pass for
operational and test-strategy gaps, as distinct from the 2026-07-12 programming/domain review whose
findings are all closed in DONE.md), except where a section's heading names another source
(e.g. REQUIREMENTS, SCENARIOS). Each section records one finding; sections land in DONE.md as they
are fixed or decided.

**SCENARIOS.md sections A–V are driven and every finding they raised is closed** in the `DONE/*.md`
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


After V, the next SCENARIOS pass is section **W. Precision, rounding, and scale** (8 scenarios),
driven the way S, T, U and V were: run every scenario against a throwaway database, apply the
standing probes to each, and log what each raises as a `## SCENARIOS W-nn` section here with the
option Evan chose. The lessons worth carrying forward are in the handover memory. V added three.
First, and again — **the standing probes find what the scenario list does not name**: three of V's
five findings came from the probes rather than the scenario they are filed under, and V-a was found
by *making the typo myself* while driving V-04, which is the strongest argument yet for driving a
scenario by hand rather than by script. Second, **a finding often has a sibling one room away**: the
project already held the rule V-a, V-c and V-d each needed — `deny_unknown_fields` on the two
non-HTTP bodies, a `duplicate_*` check on every fact table but one, a refusal for three of the four
things that can sit behind an executed rollover — so the useful question after finding a gap is
"where is this rule already applied, and what did it miss?". Third, **bound a finding with its
control before logging it**: V-e reads as a general DRP fault until the same facts are driven with
the period left open, which are correct throughout — that control is what identified the closure,
not the chain, as the cause, and it is what kept the fix from being aimed at the wrong mechanism.

## SCENARIOS W-a — A money or quantity sent as a JSON number is rounded through f64

`PUT /trades/:id` with `{"quantity": 99999999.87654321}` stores `99999999.8765432`, and
`{"quantity": 100000000.00000001}` stores `100000000` — a satoshi gone, under a `204`. The same two
values sent as **strings** are stored exactly, which is the control: the loss is entirely in the
request deserialiser. `rust_decimal`'s `Deserialize` accepts a JSON number, `serde_json` hands it
over as an `f64`, and the conversion keeps ~15 significant digits — so every ordinary figure
(`58.1234`, `19.995`, `0.00000001`) survives and only the long ones don't, which is what makes it
silent. 100,000,000 units of a sub-cent token held to 8 decimals is an ordinary crypto position, and
`quantity` is exactly where a bulk import puts one.

CLAUDE.md's rule — *"Money and quantities are always `Decimal`, never `f64`"* — holds everywhere else
in the tree: there is no `f64` in `src` outside a comment, the columns are `TEXT`, the sqlx codec is
`infra::decimal`'s, and `row_history` keeps the decimal strings verbatim. The HTTP boundary is the
one place the rule is not enforced. This is the same shape as V-a (a body field silently dropped,
now `deny_unknown_fields`) and T-10 (a query parameter silently ignored, now a `422` naming it): the
project's answer to *"the request said something we cannot honour"* is already to refuse and name it.

Found by the standing probes while driving **W-04** (the `REAL` round-trip scenario) — the migrations
are clean (`migrations_store_decimals_as_text` guards them), so the round-trip that loses precision
is on the way *in*, not in a column.

Options offered:

- **(a) Refuse a JSON number** in any money/quantity field — `422` naming the field, "enter
  `quantity` as a decimal string" — the way `deny_unknown_fields` refuses a misspelt one, pinned by a
  test that walks every request body's `Decimal` fields the way
  `every_request_body_denies_unknown_fields` walks the extractors.
- (b) Accept it exactly by enabling `serde_json/arbitrary_precision` +
  `rust_decimal/serde-arbitrary-precision`.
- (c) Accept the number only when its `f64` round-trip reproduces the literal exactly.
- (d) Document as a known limitation.

**Evan chose (a).** Refuse loudly, as the two siblings already do.

- [ ] Refuse a JSON number in every money/quantity request field with a `422` naming it, with a test
      that walks every handler-reachable request body's `Decimal`/`Option<Decimal>` fields so a new
      body is covered without anyone remembering; document the rule in `docs/API.md` beside
      [Unrecognised body fields](docs/API.md#unrecognised-body-fields)

## SCENARIOS W-b — A trade the write path accepts panics every portfolio read and drops the connection

`PUT /trades/9600` with `{"average_price":"1000000000000000","quantity":"1000000000000000"}` is
accepted `204`. Afterwards `GET /portfolio/open-parcels`, `POST /portfolio/overview` and
`POST /portfolio/unrealised-gains` return **no HTTP response at all** — the connection is reset
(curl reports `000`), the server logs `thread 'tokio-rt-worker' panicked … Multiplication overflowed`,
and the web UI's home screen shows a bare network error naming nothing. The row is invisible in every
report that could have found it, because those are the reports that die.

Two separate causes, and the finding needs both:

1. **The arithmetic multiplies before it divides.** `domain::cost_base` pro-rates with
   `initial_cost * units / parcel.quantity` (`src/domain/cost_base.rs:400`, and again at `:480` in
   `adjustment_detail`). The product overflows `rust_decimal` whenever
   `(price × qty + brokerage) × qty > 7.9228e28`, which the probe confirmed exactly at the boundary:
   `price 1 / qty 1e14` (product `1e28`) reads fine, `price 1 / qty 1e15` (`1e30`) and
   `price 0.5 / qty 4e14` (`8e28`) both kill the connection. In the overwhelmingly common case
   `units == parcel.quantity` and the multiply-then-divide is pure waste.
2. **Nothing catches the panic.** `app::router` layers no `CatchPanicLayer`, so a panicking handler
   drops the connection rather than answering the `500` with a logged cause that
   `infra::http::ApiError::Internal` exists to produce.

Bounded by its controls. The write path checks `quantity > 0` and `average_price >= 0` and imposes
**no** upper bound, so this is reachable by a fat-fingered run of zeros in either field. It is *not*
reachable from the paths that look most dangerous: a 1,000,000,000,000-for-1 `ShareSplit` on a live
parcel reads fine (the re-base multiplies the reported quantity, not the cost-base term, which stays
in as-acquired units), and a trillion units of a sub-cent token with `$1.95` brokerage reads fine
(`price × qty²` is what matters, not the holding's value). So the trigger is a mistyped trade,
not a large portfolio — which is precisely why it should answer with a message.

Found by the standing probes while driving **W-05** ($10M beside $0.01) and **W-08** (scale) — neither
scenario names an overflow.

Options offered:

- **(a) Fix the arithmetic and catch panics** — short-circuit `units == parcel.quantity` and divide
  before multiplying in `domain::cost_base`, plus a `CatchPanicLayer` so any remaining panic is a
  logged `500` with a body rather than a dropped connection.
- (b) Bound `price × quantity` at write time with a `422` naming it.
- (c) All three.
- (d) The panic layer only.

**Evan chose (a).** Fix the cause and make the class of failure legible; a magnitude ceiling would be
an arbitrary number to defend, and the arithmetic fix removes the whole-parcel case outright and
raises the partial-parcel ceiling by roughly fourteen orders of magnitude.

- [x] Divide before multiplying (and short-circuit `units == parcel.quantity`) in
      `domain::cost_base`'s two pro-rating sites, with a test at the old boundary
      — both sites now go through one `prorated_initial_cost` helper: identity where
      `units == quantity`, otherwise `checked_mul` first (so no figure that fits today moves by a
      digit — `39.95 × 2 / 3` and `39.95 / 3 × 2` differ in the last place) and divide-first only
      on the overflow the product used to panic on
- [x] Layer `CatchPanicLayer` in `app::router` so a panicking handler answers a logged `500` with a
      body instead of resetting the connection, with a test driving a deliberately panicking route
      — the body is *empty*, matching `ApiError::Internal`'s convention rather than inventing a new
      one (a panic payload can carry anything); the message goes to `tracing::error!`

Re-derived while fixing: the finding's own headline trade (`average_price` **and** `quantity` both
1e15) does not overflow at the pro-rate at all — `price × quantity` = 1e30 overflows inside
`Parcel::initial_cost` first, before any pro-rating. The boundary table is right (`price 1 /
quantity 1e15` *is* the pro-rate site), but the two are different overflows, and no reordering can
fix the first: that product is the cost base, so an unrepresentable one has no lesser answer. Which
is exactly why (a) needed both halves — the panic layer is what turns the headline trade's read from
a dropped connection into a `500`.

## SCENARIOS W-c — The tax-return-ready CSV exports carry 28-digit figures under ATO labels

`docs/API.md` calls the two `/export` endpoints "tax-return-ready" and gives each a second header row
mapping its columns to ATO tax-return labels. On **Evan's real database** (a read-only copy of the
2026-08-22 backup) they read:

```
net-capital-gain.csv  FY2026  18A  39592.120176274130543388699381
net-capital-gain.csv  FY2026  18V  0.000000000000000000000000
tax-summary.csv       FY2026       20243.630345624323612748757063
```

18V — the capital loss carried forward, a figure transcribed onto the return — prints as
twenty-four zeros after the point. Every year with a brokerage-bearing disposal in it is affected;
FY2021, FY2023 and FY2024 happen to come out clean, which is why this has gone unnoticed.

The control is the web UI, which is correct: `util.js`'s `COLUMN_KINDS` classifies every one of these
columns as money and `filterableTable` renders them at two decimal places, half away from zero, with
the full value on hover. The rule exists, it is documented at `docs/API.md`'s **Amounts round, rates
don't**, and the CSV export is the one money surface that does not inherit it.

Found by the standing probes while driving **W-07** (sum-of-parts vs total).

Options offered:

- **(a) Round every money column in the two exports to the cent**, half away from zero — the same
  rule and direction the screens use — leaving the JSON API full-precision as documented.
- (b) Whole dollars for the ATO-labelled columns, cents for the rest.
- (c) Round in the report itself so JSON, CSV and screens carry one figure.
- (d) Document as a known limitation.

**Evan chose (a).** The CSV mirrors a screen, so it should read like it; the JSON stays the exact
figure the docs promise.

- [ ] Round every money column of `net-capital-gain.csv` and `tax-summary.csv` to the cent (half away
      from zero, the `roundDecimalStr` rule) in `reports::export`'s `csv_response` path, leaving rate
      and quantity columns verbatim and the JSON responses untouched; update `docs/API.md`'s two
      export paragraphs to say so

## SCENARIOS W-d — The Annual Tax Report's printed columns do not add up

The Annual Tax Report is the one surface built to be printed and archived (`custom: 'tax-report'`,
its own `@media print` stylesheet, A4 landscape). Its parcel rows and its subtotals are each rounded
to the cent independently, so the column does not add up on the page. An entirely ordinary
three-parcel BHP disposal — `$9.95` brokerage plus `99.5c` GST on each buy, so each parcel's cost base
lands on a half-cent — prints:

```
parcel discount amounts   63.55 + 527.90 + 1060.73 = 1652.18
printed group subtotal                               1652.17
```

The subtotal is the exact sum rounded; the rows are each rounded, three of them upward. The same
disposal's `cost_base` column is a cent out for the same reason. At four decimal places every column
reconciles, which is the control — the arithmetic is right and the presentation is what disagrees.

Found by driving **W-07** directly, and confirmed against the printed document rather than only the
JSON.

Options offered:

- **(a) Total the rounded rows, in the report** — round each parcel figure to the cent in
  `reports::tax_report` and make each subtotal and grand total the sum of those rounded figures, so
  the API and the printed page agree and a reader can add the column up.
- (b) Do the same in `taxreport.js` only, leaving the API exact (so the two then differ).
- (c) Print rows at four decimal places.
- (d) Document as a known limitation.

**Evan chose (a).** The document's job is to be checked by hand; a column that does not add up fails
at exactly that.

- [ ] Round each disposal-schedule parcel figure to the cent in `reports::tax_report` and sum the
      rounded values into every subtotal and grand total, with a test asserting each column's rows
      sum exactly to the total it sits under; note the convention in `docs/API.md`'s Annual tax
      report section
