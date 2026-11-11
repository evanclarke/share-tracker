# TODO

Items are only marked done when a passing test exists for them.

This file holds only open / in-flight work. Completed and decided (out-of-scope / not-reproducible)
sections are archived in the topical `DONE/*.md` files, indexed by [DONE.md](DONE.md). When a
section here is fully done, move it into the matching `DONE/*.md` file rather than leaving it — see
CLAUDE.md.

A section records one finding, and its heading names where it came from — a REQUIREMENTS entry, a
[SCENARIOS.md](SCENARIOS.md) section, or a dated review pass.

The last full pass recorded here — the 2026-09-17 code review — is closed and archived in
[`DONE/reviews.md`](DONE/reviews.md) (26 sections), with its summary and closing narrative in
[`DONE/verification-passes.md`](DONE/verification-passes.md). Everything before it is likewise
archived; the maintained record of what has been verified is SCENARIOS.md's
[Verification status](SCENARIOS.md#verification-status) table, and of what was built and decided,
the [DONE.md](DONE.md) index. The 2026-09-24 REST API audit — its documentation fixes, consistency fixes,
machine-client surface and API improvements — is closed and archived in
[`DONE/api.md`](DONE/api.md), which is the record of what it found and what
answered each finding.

The nine items the 2026-09-26 per-commit review deliberately left — the structural
enforcement it judged out of proportion, and the three decisions it would not make on the
owner's behalf — are closed too, and archived with the review itself in
[`DONE/reviews.md`](DONE/reviews.md).

The 2026-10-04 HTTP API consistency sweep — cross-site write and DNS-rebinding guard, strict lists,
one path casing, readable writable URLs, one rejection shape, and the `200`/`201` rule — is closed
and archived in [`DONE/api.md`](DONE/api.md), and so is the 2026-10-04 agent-usability pass that
followed it (OpenAPI bodies, ids, descriptions and slices, `?dry_run=true`, the live-API skill,
trustworthy `required` lists, and reasoned GET-one `404`s). The Budget section below came from the
2026-10-08 sweep of the ATO mirrors (`docs/ato/OVERVIEW.md`, *Full sweep, 2026-10-08*). The findings
of the 2026-10-10 metric-scored code review (financial correctness 8, error handling 8.5,
concurrency/integrity/performance 8, security 8.5, architecture 6, tests 8, hygiene 9 — out of 10)
are all closed and archived in [`DONE/reviews.md`](DONE/reviews.md), the last of them the
`proptest` item of its test-suite section (2026-10-11), whose properties found and fixed three
defects of their own.

The 2026-10-11 sweep for invariants stated only in comments (moving each into a type) is closed and
archived in [`DONE/reviews.md`](DONE/reviews.md), and so is the re-basing exactness section found
verifying it (closed 2026-10-11: `AcquiredUnits` is now an exact fraction).

The 2026-10-23 Google Rust style guide pass is closed and archived in
[`DONE/reviews.md`](DONE/reviews.md): its correctness-adjacent findings, its mechanical clean-ups,
and its imports, module layout and docs section.

The 2026-10-23 third property-test pass (cross-report and snapshot-staleness invariants, and the
three defects it found) is closed and archived in [`DONE/reviews.md`](DONE/reviews.md).

The 2026-10-24 fourth property-test pass (the tax documents against their own records — the
income drilldown, the disposal schedule's arithmetic, the 45-day walk, period performance — and
the five defects it found, including the parcel-level rounding rule every printed disposal now
shares) is closed and archived in [`DONE/reviews.md`](DONE/reviews.md).

The 2026-10-24 fifth property-test pass (every write operation against its own undo and
`?dry_run=true` preview, the GET → PUT round trip at random scales, and chains of rollovers, and
the one defect it found — the Sell emptying a full-precision parcel) is closed and archived in
[`DONE/reviews.md`](DONE/reviews.md).

The 2026-10-24 sixth property-test pass (each estimate against the record it predicts — the pre-sale
what-if against the worksheet once its Sell is entered, the franking what-if against the denial
recording the sale causes, unrealised gains against selling everything that day — and the seven
defects it found) is closed and archived in [`DONE/reviews.md`](DONE/reviews.md).

The 2026-10-24 seventh property-test pass (pure rules against independent references — the
money-weighted return solver, the 12-month discount rule, backup retention — and cross-check
reports against the consumers they check for — FX coverage against the tax summary, rollover
consistency and the price-collection timeline against the operations and schedule — and the five
defects it found) is closed and archived in [`DONE/reviews.md`](DONE/reviews.md).

The 2026-10-24 design review of unit granularity and registry rounding of fractional entitlements
(account unit precision, a fraction rule on every ratio-driven action, per-holding rounding stored
per parcel, recompute-on-write, and the register at the record date for splits, bonus issues,
scrip exchanges and demergers — `Exact` kept by owner decision) is closed and archived in
[`DONE/reviews.md`](DONE/reviews.md).

The `PUT /trades/:id` missing-listing `500` found 2026-10-25 (and the sweep of every write naming a
listing or holding account it prompted) is closed and archived in [`DONE/api.md`](DONE/api.md).

## 2026–27 Budget CGT changes and the indexation election (ATO mirror sweep 2026-10-08; re-investigated 2026-10-25)
The ATO's CGT discount, cost base, how-to-calculate and indexation pages have carried a banner
since 29 June 2026: "Recent changes to capital gains tax (CGT) announced in the 2026–27 Federal
Budget don't apply to Tax Time 2026", with "Resources will be available at a later date" (still
there on 2026-10-25).

**Status, corrected 2026-10-25.** This section's old heading called the measure "announced, not
law". That is out of date. The *Treasury Laws Amendment (Tax Reform No. 1) Bill 2026* (introduced
28 May 2026) and its Imposition Bill passed the Senate on 25 June 2026. Royal Assent is not yet
confirmed here. The Bill is only the **framework, the first tranche**; a second tranche of detail
is to follow. Source: the Parliamentary Library Bills Digest
(https://www.aph.gov.au/Parliamentary_Business/Bills_Legislation/bd/bd2526/26bd067), read
2026-10-25. That is not ATO guidance, so nothing is implemented from it beyond the shape below.

What the framework settles:
- **Deemed sale (Subdiv 112-E):** from 1 July 2027 every CGT asset of an individual or trust is
  treated as sold just before 1 July 2027 and bought back on that date. No tax is due on that
  deemed sale; the gain is split when the asset is actually sold.
  - The **pre-1 July 2027 part** is deferred and taxed under the existing rules (the 50% discount
    where eligible).
  - The **post-1 July 2027 part** gets cost-base indexation (assets held 12 months or more) plus
    the 30% minimum tax (Div 119; the tax itself is imposed by the separate Imposition Bill,
    ITRA s 12AA).
- **The split** is set by either the **market value at 1 July 2027** or an apportionment method
  the Minister will set later (s 112-185, not yet published).
- **Capital losses** are calculated the same way as today; the Bill does not change that.
- **Pre-CGT assets** (bought before 20 Sep 1985) stop being exempt: they get a cost base equal to
  their 1 July 2027 market value.
- **Gains are sorted into four categories (s 102-6):** residential or non-residential, each either
  current or deferred (deferred = the pre-2027 part held back until the actual sale). Shares and
  trust units are non-residential.

Still unknown:
- the apportionment method
- which CPI series and rounding the new indexation uses
- whether the 12-month test runs from the original purchase or from the deemed buy-back
- how the pre-2027 and post-2027 parts net against each other and against losses
- the new AMMA statement components
- the new return labels

**Approach (owner-approved 2026-10-25).** The pre-1999 **indexation election** and the 2027 split
have the same shape. Each parcel's share of a sale breaks into **typed gain components**: discount
gross, indexed, other, loss, plus the minimum-tax base after 2027. The loss-netting walk then takes
losses from each pile in the ATO's Step 8 order. So the election is built first, against rules
already in force with ATO worked examples (Val; Clare, Example 18), and the 2027 split plugs into
the same layer once its detail is published. The comparison side of indexation already exists
(SCENARIOS AA-a and the loss-aware optimum, archived in
[`DONE/tax-domain.md`](DONE/tax-domain.md)). Only the election into the reported figures is out of
scope today (the *Indexation method* entry in
[`docs/API.md`'s Known limitations](docs/API.md#known-limitations)). As far as is recorded, the
live portfolio holds no pre-1999 parcel, so Phase 1 changes no figure for the owner today. Its value
is the layer Phase 3 needs.

### Phase 1 — the indexation election (current law)
- [ ] `domain::cgt_method`: one shared function that turns a parcel's share of a sale into typed
  gain components (how each is taxed: discounted / indexed / other / loss / minimum-tax base).
  `reports::realised_gains` and `net_capital_gain::gross_buckets` both read from it, so no caller
  re-classifies a gain inline.
- [ ] `net_years` gains an explicit **indexed** pile, netted in Step 8 order (other → indexed →
  discount, with the 50% discount on what is left of the last). The trust's indexation gains (an
  AMMA statement's `cgt_indexation_gains`) move into it out of `other`. Pin that no figure moves for
  a database with no elections; a byte-identical before/after drive, as AA-a did.
- [ ] Election storage: a new audited table (e.g. `cgt_method_elections`, keyed by sale trade +
  purchase trade, `units_indexed`). Kept out of `parcel_allocations` because a Sell edit replaces
  its allocations and would silently wipe the election.
  - With no row, the parcel takes the discount.
  - Checked at write time inside the transaction, `422` otherwise: the parcel's cost was incurred
    by 21 Sep 1999 (its own trade date); the allocation is a gain; the units don't exceed what was
    sold.
  - Needs: snapshot-staleness triggers, the three audited-table lists, `docs/SCHEMA.md`,
    `docs/API.md`, and a `config.js` entity.
  - A health-report entry names an election a later Sell edit no longer fits (instead of refusing
    the Sell edit).
- [ ] Reported figures honour the election: realised gains, net capital gain (and its
  carried-forward loss chain into later years), tax summary, the annual tax report and its CSV
  exports. ATO acceptance tests in `ato_examples.rs`: Val (indexing-the-cost-base.md) and Clare's
  395/396-share split (Part B Example 18).
- [ ] "Apply the optimum" action (an `ACTIONS` entry): writes a year's loss-aware split from
  `GET /reports/indexation_cross_check` as elections. Once elections are recorded facts, the
  cross-check's brought-forward balance follows the figures as chosen, so drop its "assumes the
  discount in every earlier year" qualifier.
- [ ] Rewrite the *Indexation method* Known limitation (the election is now modelled); keep the
  `doc_checks` pins in step.

### Phase 2 — capture the 1 July 2027 market value (must land before June 2027)
- [ ] Whatever is decided, make sure a closing price is stored for every held listing on the last
  trading day before 1 July 2027 (the scheduled price import plus `closing_price`'s held-days
  backfill), since a missing one cannot be fetched reliably years later
- [ ] Health-report entry naming every listing held on 30 June 2027 that has no closing price for
  its last trading day before 1 July 2027 (computable now against a test clock). Also a per-listing
  override value for anything without a fetchable price (an unlisted unit, a delisted holding).

### Phase 3 — the 2027 split (after the second tranche / ATO guidance)
- [ ] Watch: when the measure is legislated and the ATO publishes guidance, mirror it into
  `docs/ato/` and index it in `docs/ato/OVERVIEW.md` (re-fetch the four bannered pages then too).
  Until then, mirror the Bills Digest as a non-ATO legislative source, marked as such in
  `OVERVIEW.md`.
- [ ] NEEDS DECISION (owner), once the rules are known: whether and how to model the 1 July 2027
  split. Known shape of the inputs: a per-parcel **market value at 1 July 2027** to divide the gain
  (the closing-price table is the natural source); a CPI series that runs past September 1999
  (`cpi_quarters` is frozen at the 1999 freeze today); and the 30% minimum tax, which turns on the
  taxpayer's marginal rate — outside the data model, like the FITO offset limit.
  **DECIDED 2026-10-25 (owner): model it**, as the items below. This stays open until they close.
- [ ] `cgt_method` splits a parcel's gain at the deemed sale (market value or the Minister's
  apportionment method, whichever the rules allow and the owner picks).
  - The pre-2027 part goes through the existing rules, including a Phase 1 indexation election
    for a pre-1999 parcel.
  - The post-2027 part is indexed from the deemed buy-back.
  - Pre-CGT (pre-20 Sep 1985) parcels enter at their 1 July 2027 market value. Today the
    `trades.date` floor refuses them outright, so revisit that floor.
- [ ] A live, unfrozen CPI series, as new `cpi_quarters` rows or a parallel table (the frozen
  68.7 numerator stays the old rule's). If the ATO publishes it quarterly, add an import job through
  `infra/scheduler/registry.rs` + `schedule.cron`.
- [ ] Per-year **minimum-tax capital gain** figure. Informational only: the tax turns on the
  taxpayer's marginal rate, outside the data model like the FITO limit, and the Div 119 income
  support exemption is likewise not modelled.
- [ ] The new AMMA statement components (deferred / indexed gains) and the new return labels on
  the annual tax report, once published.

## Remove the RUSTSEC-2026-0459 advisory ignore (raised 2026-11-11)
The advisory (bundled SQLite < 3.54.0) failed CI's `cargo deny` gate on 2026-11-11 and went on
`deny.toml`'s ignore list (expires 2027-01-31): the fix is `libsqlite3-sys` 0.39, which
`sqlx-sqlite` 0.9.0 caps below 0.38.
- [ ] When a sqlx release allows `libsqlite3-sys` >= 0.39 (watch Dependabot's cargo group PRs):
  upgrade sqlx, bump the `libsqlite3-sys` dev-dependency in `Cargo.toml` in step with it (its raw
  handle backs the busy-probe FFI), remove the ignore entry from `deny.toml`, and confirm
  `cargo deny check advisories` passes without it. If no release lands by 2027-01-31,
  `doc_checks::advisory_ignores_expire` fails the suite: re-justify the entry with a new expiry.
