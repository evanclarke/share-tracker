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

- [x] Refuse a JSON number in every money/quantity request field with a `422` naming it, with a test
      that walks every handler-reachable request body's `Decimal`/`Option<Decimal>` fields so a new
      body is covered without anyone remembering; document the rule in `docs/API.md` beside
      [Unrecognised body fields](docs/API.md#unrecognised-body-fields)
      — `infra::decimal` gained a **serde** half beside its sqlx one (`strict_decimal`,
      `strict_optional_decimal`, `strict_decimal_map`), spelled as
      `#[serde(deserialize_with = …)]` on **120 fields** across 25 files, so a body field stays a
      plain `Decimal` and every reader of it is unchanged. The two halves are named apart
      deliberately: `Money`/`OptMoney` remain the TEXT⇄`Decimal` **sqlx** codec, these three are the
      JSON⇄`Decimal` **request** codec. Integers are refused too (`10` as much as `10.5`) — a rule
      that only bit past some digit count would put the same silent boundary back one step along.
      The message says the remedy, and axum's extractor prefixes the field path, so a nested one
      names itself (`prices.1: send this money/quantity value as a decimal string …`); reaching the
      visitor needed `deserialize_any` rather than `deserialize_str`, since with a scalar hint
      `serde_json` answers the wrong type itself and the remedy never gets a chance to be said.
      `infra::http::tests::every_money_request_field_refuses_a_json_number` is the guard, walking
      the same handler-reachable set as `every_request_body_denies_unknown_fields` — the two now
      share one `request_body_types` walk, so neither can drift — with an empty `JSON_NUMBER_ALLOWED`
      alongside `UNKNOWN_FIELDS_ALLOWED`. Behaviour is pinned at the HTTP surface by
      `entities::trade::tests::api_a_quantity_sent_as_a_json_number_is_refused_naming_the_field` and
      its control `…::api_the_same_quantity_sent_as_a_string_is_stored_exactly`, and the codec by
      four unit tests in `infra::decimal`; `docs/API.md` gained **Money as a JSON number** beside
      Unrecognised body fields, a line in the `422` row, and `doc_checks::money_as_a_json_number_documented`

Collateral damage was one decision and it went the harmless way: 15 existing tests in
`entities/trade.rs` and `entities/income.rs` sent money as bare JSON numbers in their request
bodies, and were fixed by quoting them (67 literals) rather than by widening the rule — none was
asserting anything about the number form. `scripts/fixtures/demo.json` was already clean (its only
bare numbers are ids), and the web UI reads every decimal field out of a text input as a string,
the price-override maps included, so no UI path had to change.

Verified independently at the HTTP surface, against a throwaway database: all four measured values
— `quantity` `99999999.87654321`, `100000000.00000001` and `0.123456789012345678`, and
`average_price` `1234567890123456789.12` — now answer `422` naming their field, where they
previously answered `204` and stored `99999999.8765432`, `100000000`, `0.12345678901234568` and
`1234567890123456800`; the same two quantities sent as strings still answer `204` and read back to
the digit, the control.

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

Verified independently at the HTTP surface after the fix, against a throwaway database: the three
pro-rate cases (`price 1 / qty 1e15`, `price 0.5 / qty 4e14`, `price 1 / qty 1e14`) now answer `200`
on `/portfolio/open-parcels`, `POST /portfolio/overview` and `POST /portfolio/unrealised-gains`; the
headline `1e15 × 1e15` trade answers **`500` with an empty body** and logs
`request handler panicked panic=Multiplication overflowed`, where it previously reset the connection;
`/reports/health` stays `200` throughout, the control.

One further residual, out of this section's scope and noted while fixing it:
`AmitReductionEvent::reduction_for_units` carries the same multiply-before-divide shape
(`per_unit * covered.min(held) * units / held`). Probed at 1e15 units with a `0.05` per-unit
adjustment it survives (`1.5e28`), but a larger per-unit figure at that scale would overflow — now a
logged `500` rather than a dropped connection, so it fails safe. Worth closing properly later.

## SCENARIOS W-e — A trade whose price × quantity is unrepresentable is still accepted with 204

Split out of W-b once fixing it established that the headline trade overflows at
`Parcel::initial_cost`, not at the pro-rate. `PUT /trades/:id` with an `average_price` and `quantity`
whose product exceeds `Decimal`'s range (≈ `7.9228e28`) is still accepted `204`. It now fails
*safely* — the three portfolio reads answer a logged `500` with an empty body instead of resetting
the connection — but three screens are still dead, the reply still names nothing, and the offending
row is still invisible in exactly the reports that would have found it, because those are the reports
that fail.

The write path checks `quantity > 0` and `average_price >= 0` and imposes no upper bound. W-b's
option (b) — refuse the write, naming the figure — is what closes this, and the objection recorded
against it there (*"a magnitude ceiling would be an arbitrary number to defend"*) turns out **not** to
apply to this half: the ceiling here is not a policy number but a representability limit the type
itself states, so the refusal has a principled bound to quote. That is a new fact, so the decision is
worth re-taking rather than inheriting.

- [ ] Decide whether to bound `average_price × quantity` at write time, and if so refuse it `422`
      naming the product and the limit, across every parcel-creating write

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

- [x] Round every money column of `net-capital-gain.csv` and `tax-summary.csv` to the cent (half away
      from zero, the `roundDecimalStr` rule) in `reports::export`'s `csv_response` path, leaving rate
      and quantity columns verbatim and the JSON responses untouched; update `docs/API.md`'s two
      export paragraphs to say so
      — the rounding is a **type**, not a column-name list: `reports::export::Cents(Decimal)`
      serializes to 2 dp, half away from zero, always both places (`0.00`, never `-0.00`, no
      thousands grouping — the separator is the delimiter). A CSV row is a *projection* of the
      report record whose money fields are `Cents`, so which columns round is decided by the
      field's type and nothing duplicates `util.js`'s `COLUMN_KINDS` in Rust — a name list in
      `csv_response` was the alternative and was rejected for exactly that reason (serde hands the
      writer a `Decimal` as a string, indistinguishable from `taxpayer_basis`, so a writer-level
      pass has no way to tell money from text without such a list).
      `NetCapitalGainYearCsv` already existed and gained `Cents` fields; `tax_summary` grew the
      matching `TaxYearSummaryCsv` (39 money columns) rather than exporting the JSON struct
      directly. `tax_year` and `taxpayer_basis` are not money and pass through untouched; the JSON
      responses are unchanged, pinned by a control test on each report over the same facts.
      Verified against the read-only copy of the 2026-08-22 backup: 18A `39592.120176274130543388699381`
      → `39592.12`, 18V `0.000000000000000000000000` → `0.00`, FY2022 18A → `3151.90`, tax-summary
      FY2026 assessable income `20243.630345624323612748757063` → `20243.63`, with both JSON reports
      still answering the full-precision figure. Tests: `reports::export::tests::{a_money_column_rounds_to_the_cent_and_a_plain_decimal_does_not,
      a_half_cent_rounds_away_from_zero_in_both_directions, a_nil_money_figure_is_two_zero_decimals,
      a_whole_or_short_money_figure_is_padded_to_the_cent}`, and on each export
      `api_export_rounds_money_columns_to_the_cent` + its control
      `api_the_json_report_keeps_the_precision_the_export_rounds`
      (`reports::net_capital_gain`, `reports::tax_summary`) plus
      `reports::tax_summary::tests::api_export_rounds_a_half_cent_away_from_zero`;
      `doc_checks::cent_rounded_csv_exports_documented` pins the two export paragraphs and the
      display-rules sentence that had promised full-precision CSV.
      One consequence, deliberate and matching the screens: each column rounds independently, so
      rounded components need not add to a rounded total (here 39344.55 + 247.57 = 39592.12 does,
      but that is arithmetic, not a guarantee) — the same behaviour every table on screen has.

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

- [x] Round each disposal-schedule parcel figure to the cent in `reports::tax_report` and sum the
      rounded values into every subtotal and grand total, with a test asserting each column's rows
      sum exactly to the total it sits under; note the convention in `docs/API.md`'s Annual tax
      report section
      — the rounding happens **once, at the row**: `DisposalParcelRow::round_money_to_cents` runs as
      each row is built, so `DisposalTotals::add` can only ever sum figures that are already
      rounded and a subtotal is the sum of the rows printed above it *by construction*, not by a
      second pass someone could forget. The rule itself is now `infra::decimal::to_cents` (to the
      cent, half away from zero, a figure that rounds to nil normalised to a positive zero) and
      W-c's `reports::export::Cents` delegates to it: that type is a *serialisation* wrapper whose
      `Display` renders `{:.2}`, and here the rounded value has to be **summed**, so the two now
      share the rounding and differ only in the rendering — one rule provably, rather than two
      that agree today. Money, and so rounded: `initial_cost_base_aud`, `adjusted_cost_base_aud`,
      `proceeds_aud`, `gain_loss_aud`, `cgt_discount_amount_aud`, `gain_after_discount_aud`, and
      each itemised adjustment's `amount` (the document prints those under the parcel too). Left
      verbatim: the two `*_per_unit_aud` figures and an adjustment's `per_unit` — a derived
      per-unit figure shows at 4+ dp by the documented display rule, never cent-rounded —
      `buy_price`/`sale_price`, `units`, `days_held`, the two FX rates, and
      `buy_brokerage`/`buy_gst_on_brokerage`, the contract note's own native-currency figures,
      transcribed for hand-checking against it (99.5c of GST on $9.95 of brokerage is genuinely
      sub-cent) and totalled nowhere. Nothing downstream reads these rows — the report computes
      nothing new — so no tax figure moved: `realised_gains` and `net_capital_gain` still answer
      the exact decimal, and the tax-report/realised-gains reconciliation test still passes.
      Measured on the three-parcel BHP disposal: the discount and discounted-gain columns now
      subtotal **1652.18** over rows of 63.55 + 527.90 + 1060.73 (printed 1652.17), and the
      cost-base column **27453.44** over 4991.52 + 9227.54 + 13234.38 (printed 27453.43); proceeds
      and gain/loss happened to reconcile already and are unchanged (30757.77, 3304.34).
      Tests: `reports::tax_report::tests::{api_a_disposal_columns_rows_add_up_to_its_printed_subtotal,
      api_every_disposal_money_column_totals_the_rounded_rows_beneath_it,
      api_the_per_unit_and_as_entered_disposal_columns_are_not_cent_rounded}` — the middle one
      finds the money columns *by name* (`*_aud` less the per-unit pair) across a three-group
      document (an AUD disposal, an AMIT parcel carrying itemised adjustments, a USD parcel whose
      every figure is an FX conversion), so a newly added money column is covered without being
      listed, and asserts its total↔parcel column pairing covers the whole of `DisposalTotals`, so
      a newly added *total* fails until it is reconciled too. Plus
      `infra::decimal::tests::to_cents_rounds_half_away_from_zero_and_keeps_the_cent_scale` and
      `doc_checks::cent_rounded_tax_report_disposals_documented`; all three W-d tests were
      confirmed to fail with the rounding call removed. `taxreport.js` needed no change: it prints
      the server's subtotals and re-derives nothing client-side, and `numericDisplay`'s money
      rounding is idempotent on a figure already at the cent (its hover tooltip simply stops
      appearing, having nothing left to show).

Two corrections to this section's own write-up, found by re-deriving it. The cost-base rows are
4991.52 + 9227.54 + **13234.38** (the third parcel is 333 units at 39.71, not 11726.38) — the total
27453.44 was right. And the control is narrower than stated: at four decimal places the *cost base*
column reconciles, but the discount column does not (63.5468 + 527.8979 + 1060.7254 = 1652.1701
against a subtotal of 1652.1700), because halving three exact-arithmetic gains lands on figures no
display precision reconciles. The true control is that the underlying arithmetic is exact and it is
any *display* rounding of the rows that disagrees with the rounded exact total — which is why the
fix has to be to total the rounded rows rather than to print more places.

Three residues deliberately left, each a decision rather than an oversight, and each Evan's to take
as its own section:

1. **A row's own arithmetic can still be a cent out.** The schedule prints proceeds, cost base and
   gain/loss on one line; rounded independently, the second BHP parcel prints 10283.33 − 9227.54
   beside a gain of 1055.80. Deriving the gain from the rounded components would fix the row *and*
   keep the columns adding up (Σ of derived gains = Σ proceeds − Σ cost base), at the price of a
   printed gain that is not the rounded gain — a figure the chosen option (a) does not authorise.
   This is unchanged by W-d: the printed page has shown those same three rounded numbers all along,
   since the UI rounds every money cell.
2. **`income` vs the overall tax summary is the same shape, one level up.** The income tables print
   per-record AUD figures whose totals appear in the tax-summary section, and `docs/API.md`
   currently promises "Every AUD figure here sums to exactly the matching tax summary line" —
   which is true only at full precision. Rounding the income rows would break that documented
   guarantee unless the summary line were rounded too, and that line is `tax_summary`'s, shared
   with its own screen and CSV. Left alone deliberately.
3. **`cgt_summary` likewise**: it is `net_capital_gain::CgtSummaryYear`, printed as a worksheet
   whose lines subtract from one another, and rounding it here would fork the figure from the
   report that owns it. See the note under W-c for the same fault in that report's CSV.
