# TODO

Items are only marked done when a passing test exists for them.

Completed and decided (out-of-scope / not-reproducible) sections are archived in the topical `DONE/*.md` files, indexed by [DONE.md](DONE.md), to keep this list focused on active work. When a section here is fully done, move it into the matching `DONE/*.md` file rather than leaving it — see CLAUDE.md.

Sections here come from the **2026-07-13 improvement review** (a whole-project pass for
operational and test-strategy gaps, as distinct from the 2026-07-12 programming/domain review whose
findings are all closed in DONE.md), except where a section's heading names another source
(e.g. REQUIREMENTS, SCENARIOS). Each section records one finding; sections land in DONE.md as they
are fixed or decided.

**One finding is open in this file** — SCENARIOS X-a, from the section X pass of 2026-08-23, now fixed and awaiting archive.

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

After W, the next SCENARIOS pass is section **X. Transactional integrity and concurrency**
(8 scenarios), driven the way S through W were: run every scenario against a throwaway database,
apply the standing probes to each, and log what each raises as a `## SCENARIOS X-nn` section here
with the option Evan chose. The lessons worth carrying forward are in the handover memory. W added
three. First, **the fix's own reproduction may not exercise the mechanism the finding blames**: W-b's
headline trade overflowed in `Parcel::initial_cost`, not at the pro-rate the finding named, and the
boundary table beside it described a genuinely different overflow — had only the arithmetic been
fixed, the finding's own reproduction would still have reset the connection. Second, **a control
that only holds for the columns you happened to check is not a control**: W-d's write-up claimed
every column reconciles at 4 decimal places, which is true of proceeds and cost base and false of the
halved discount column — the real control is that *any* rounding of the rows disagrees with the
rounded exact total, and that is what says the fix has to be to total the rounded rows. Third, and
the sharpest, **check a reconciliation in the form the document prints, not in the form the code
computes**: W-f's write-up said Evan's FY2026 reconciled "by luck" because `39344.55 + 247.57 =
39592.12` — but that addition *was the old code's own formula* and could therefore never fail. Read
as the worksheet actually prints it (18H net **less** the concession) FY2026 and FY2025 were each a
cent out all along.

## SCENARIOS X-a — a fact write that lands while a snapshot is being generated is lost, and the snapshot is stored as fresh

`reports::snapshot::generate` reads everything it stores **outside** the transaction it stores it in.
`aud_prices_for`, `portfolio::db_holdings`, `unrealised_gains::db_unrealised_gains` and
`performance::db_performance` each open (and close) their own read transaction against the pool;
only afterwards does `write_tx` open, and its `INSERT … ON CONFLICT … DO UPDATE` writes `stale = 0`.
So a fact write that commits **between** the reads and that insert is neither in the stored figures
nor reflected in the stored flag: the schema's staleness triggers fire against a row that does not
exist yet (or against the row about to be overwritten), and the insert then clears the flag they set.
The snapshot is silently a snapshot of a state that no longer exists, and — because nothing but the
`stale` flag ever asks for a regeneration — it stays that way forever. The daily `report-snapshot`
job, `POST /report_snapshots/generate`, `regenerate_all` and `regenerate_provisional` all go through
this one function.

**Reproduction** (throwaway database, 6,000 parcels on the exchanged listing so generation takes
1.65 s; every price for 2025-06-30 entered at `10`):

1. `POST /report_snapshots/generate {"date":"2025-06-30"}`
2. 500 ms later, while it is still computing, correct one listing's price:
   `PUT /closing_prices/6/2025-06-30 {"price":"25", …}` → `204` in 2 ms.
3. The snapshot lands at `stale: false`, holding `current_price: "10"` and
   `market_value: "6000000"` for that listing — while the stored closing price for that very day is
   `25`, i.e. a market value of `15000000`. The archived valuation is out by **A$9,000,000** and
   nothing will ever say so.

**Bounded by its controls, both of which are correct.** The same correction applied *entirely
before* generation gives `current_price: "25"` / `15000000`; applied *entirely after* it, the three
snapshots come back `stale: true` and are regenerated by the daily job. Only the interleave is
wrong, which is what identifies the read/write split as the mechanism rather than the trigger set.
**It is not price-specific**: repeating the race with an ordinary `PUT /trades/:id` (a Buy dated
before the snapshot date) leaves the snapshot's quantity at the pre-trade `600000` and, again,
`stale: false` — *any* fact write landing in that window is lost the same way. And it needs no
human: the scheduler's own `price-import` and `report-snapshot` jobs write and read the same tables,
and `rba_fx_rate::true_up_provisional_snapshots` regenerates from inside another job's run.

**Evan's real data is clean** (checked on a copy of `share-tracker-2026-08-22-220812.db`):
regenerating all **2,182** stored snapshot dates reproduced every stored `rows_json` byte for byte,
so the race has not yet corrupted anything live. The window there is ~41 ms per date rather than
1.65 s, which is why.

**The other seven section X scenarios came back correct** — see
[Section X findings](SCENARIOS.md#section-x-findings).

**Options considered** (Evan asked for the pass to pick and proceed rather than stop per finding):

(a) **Generate inside the transaction that stores it** — open `write_tx` first, take every read on
    that connection (`_on(&mut conn)` variants beside the existing `portfolio::db_holdings_on`),
    insert, commit. The inputs and the `stale = 0` claim then come from one serialised point in
    time, which is the rule every other write in the tree already follows. Cost: generation holds
    SQLite's write lock for its duration (~41 ms per date on the real database, 1.65 s on a
    10,000-parcel synthetic), so a concurrent write waits rather than failing — the busy timeout
    covers it, and nothing in `generate` touches the network, so the lock is never held on I/O.
(b) **Detect the interleave and store `stale = 1`** — capture a change marker before the reads,
    re-read it inside the write transaction, and flag the row rather than clearing the flag when it
    moved. Cheaper on lock hold time, but needs a marker the schema does not have
    (`PRAGMA data_version` is per-connection, and the pool hands out a different connection for each
    read), and it *stores a figure known to be wrong* and relies on a later regeneration.
(c) **Leave it, document it as a known limitation.** Rejected: the wrong figure is indistinguishable
    from a right one on the Portfolio Overview graph and in the Snapshots screen, and the archive is
    the only record of a past day's position.

**Chosen: (a).** It removes the window rather than reporting it, it is the convention the rest of
the tree already states (`infra::db::write_tx`'s doc comment, and every entity's `db_upsert`), and
the `_on` split it needs is a pattern the reports already have.

- [x] Generate a report snapshot inside the write transaction that stores it, so a fact write cannot
      land between its inputs and its `stale = 0`
      — `reports::snapshot::generate` now opens `infra::db::write_tx` as its **first** statement and
      takes every input read on that transaction: `aud_prices_for` → `valuation::stored_valuations_on`,
      `portfolio::db_holdings_on` (already existed), `unrealised_gains::db_unrealised_gains_on`,
      `performance::db_performance_on`. The figures and the `stale = 0` they are stored with now come
      from one serialised state, so the write lock is what closes the window rather than a marker
      that detects it. **The mechanism was re-derived before the fix, and the write-up held up in
      full**: the reads each opened and closed their own transaction against the pool, the
      `INSERT … ON CONFLICT … DO UPDATE` sets `stale = 0` on both arms, and both trigger cases behave
      as described (a first generation has no row for the staleness triggers to mark; a regeneration
      has one, marked and then cleared by the same insert).
      **The `_on` split, following `portfolio::db_holdings_on`** — each pool-taking function is kept
      and delegates to its `_on` twin, so the two can never diverge: `reports/valuation.rs`
      (`held_markets_on`, `stored_valuations_on`), `reports/unrealised_gains.rs`
      (`db_unrealised_gains_on`), `reports/performance.rs` (`accumulate_on`, `db_performance_on` —
      `db_performance` now owns the read transaction `accumulate` used to open),
      `entities/closing_price.rs` (`HeldTimeline::load_on`, `db_held_listing_ids_on`, and `db_get_one`
      / `db_latest_ok_price_on_or_before` made executor-generic in the shape `listing::db_get` and
      `FxRates::load` already use, rather than grown a second copy). `load_market_on` was already
      there. **Nothing inside the transaction touches the network**, checked call by call down
      `stored_valuations_on`: it reads `closing_prices`, `listings`, the rename chain, the holiday
      calendar and `rba_fx_rates`, and `Market::latest_complete_trading_day` is pure arithmetic over
      the `now` passed in — snapshot generation values from **stored** prices only, so the lock is
      never held across I/O. `DEFERRED_BEGIN_ALLOWED` is unchanged and still true: the `_on` variants
      begin nothing, and `valuation.rs` reaches for `pool.acquire()` rather than a transaction, so it
      stays off the list.
      **Verified at the HTTP surface, throwaway database, 6,000 parcels (generation ≈ 0.6–1.4 s), the
      finding's own reproduction**: `POST /report_snapshots/generate {"date":"2025-06-30"}` with a
      `PUT /closing_prices/1/2025-06-30 {"price":"25"}` fired 500 ms in. *Before*: the correction
      returned `204` in **1 ms** and the snapshot landed `stale: false` holding `current_price: "10"`
      / `market_value: "6000000"` against a stored price of `25` — A$9,000,000 of archived valuation
      with nothing left to ask for a regeneration. *After*: the same correction returns `204` in
      **81 ms** (it waits for the run) and all three snapshots land **`stale: true`** — the run's own
      figures, correctly flagged, and a following `generate` stores `current_price: "25"` /
      `market_value: "15000000"` fresh. The non-price half of the finding behaves the same way: a
      `PUT /trades/9001` (a Buy dated before the snapshot date) fired 500 ms into a run returns `204`
      in 740 ms and leaves the snapshot `stale: true`, where it used to leave it fresh at the
      pre-trade quantity. **Both controls still hold, unchanged by the fix**: the correction applied
      *entirely before* a run gives `current_price: "25"` / `15000000` fresh; applied *entirely
      after*, all three come back `stale: true`.
      **Tests** (`src/reports/snapshot.rs`):
      `reports::snapshot::tests::a_price_written_during_generation_never_leaves_a_fresh_superseded_snapshot`
      is the invariant — six rounds (the first a first generation, the rest regenerations) each fire
      a corrected price at a run started on another task, and assert the stored snapshot is **either**
      valued at that price **or** flagged stale, plus that its market value still equals its own
      stored price × units. It holds for every ordering, so it cannot flake on the fixed code; it
      fails on round 0 of the old code (3/3 runs).
      `reports::snapshot::tests::generation_reads_only_after_it_holds_the_write_lock` is the same
      guarantee with the race removed: another connection holds `BEGIN IMMEDIATE` with a corrected
      price uncommitted, the run must make no progress, and after the commit it must value at the new
      price — deterministic, and it fails on the old code (which reads before it blocks and stores
      the superseded figure). Both wait on conditions with deadlines, never on yield counts.
      **Both tests build a file-backed pool (`race_pool`, `tempfile` + `infra::db::init`) rather than
      `test_support::test_pool`**, and this is load-bearing rather than incidental: the in-memory pool
      *does* hand out several connections that share one database, but it is **shared-cache**, where a
      reader on a second connection blocks on an open writer — the read/write interleave cannot arise
      at all there, and both tests passed against the *unfixed* code on it. Under WAL (what `main`
      opens) a reader sees the snapshot it began with while another connection commits past it, which
      is the real behaviour. The helper says so, so the next reader does not "simplify" it back.
      **Docs**: `docs/API.md`'s Report snapshots section states the guarantee (reads inside the
      storing transaction, a concurrent fact write waits and then stales, no network call under the
      lock, and the cost — a concurrent write waits tens of ms per date), and README's snapshot
      feature bullet gains the same clause. No schema change, so `docs/SCHEMA.md` is untouched; the
      requirement is code-tested, so no `doc_checks.rs` entry.
