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

## 2026–27 Budget CGT changes — announced, not law (ATO mirror sweep 2026-10-08)
The ATO's CGT discount, cost base, how-to-calculate and indexation pages have carried a banner
since 29 June 2026: "Recent changes to capital gains tax (CGT) announced in the 2026–27 Federal
Budget don't apply to Tax Time 2026", with "Resources will be available at a later date". As
reported (not yet ATO guidance), the measure replaces the 50% discount with **cost-base indexation
plus a 30% minimum tax** on real gains for assets held 12 months or more, from **1 July 2027**,
with an asset bought before and sold after that date taxed under the existing rules for the gain
accrued to 1 July 2027 and the new rules after it. Nothing is modelled or mirrored yet: there is
no legislation or ATO guidance to implement against, and every disposal this system can hold today
falls under the existing rules.
- [ ] Watch: when the measure is legislated and the ATO publishes guidance, mirror it into
  `docs/ato/` and index it in `docs/ato/OVERVIEW.md` (re-fetch the four bannered pages then too)
- [ ] NEEDS DECISION (owner), once the rules are known: whether and how to model the 1 July 2027
  split. Known shape of the inputs: a per-parcel **market value at 1 July 2027** to divide the gain
  (the closing-price table is the natural source); a CPI series that runs past September 1999
  (`cpi_quarters` is frozen at the 1999 freeze today); and the 30% minimum tax, which turns on the
  taxpayer's marginal rate — outside the data model, like the FITO offset limit
- [ ] Whatever is decided, make sure a closing price is stored for every held listing on the last
  trading day before 1 July 2027 (the scheduled price import plus `closing_price`'s held-days
  backfill), since a missing one cannot be fetched reliably years later

## Unit granularity and registry rounding of fractional entitlements (design review 2026-10-24)
Every rounding defect the 2026-10-11 and 2026-10-24 passes found has one root: a ratio-driven
action (`ShareSplit`, `BonusIssue`, `ScripForScrip`, `Demerger`) whose ratio does not divide a
holding produces a count no register holds (200 post-split units of a 3-for-1 split are 66.666…
as-acquired units), and every later re-base, sum and read-back has to keep it exact —
`AcquiredUnits` as an exact fraction, `as_acquired_quantity`'s shortest-count read-back,
`sold_in_acquired_units`' remainder matching, `parcel_optimiser`'s `adding_up_to` residue. The
model is also unfaithful: `docs/API.md`'s [Fractional entitlements](docs/API.md#fractional-entitlements)
says registry rounding and cash in lieu are "not modelled". What registries actually do:
- **The rounding rule is the action's.** ASX Listing Rule 7.20 has the issuer disclose its
  treatment of fractional entitlements, and it varies: rounded up (Wesfarmers' 0.9827
  consolidation, 2014), rounded down with no cash (Stellar Resources' 10-for-1, 2026), or
  aggregated, sold and paid out as cash in lieu
- **It is applied once per holding** (per HIN/SRN), not per parcel — parcels are the taxpayer's
  record, the register holds one number. Two 50-unit parcels after a 1-for-3 consolidation are 33
  or 34 units, never 16.67 + 16.67
- **No CGT event** (TD 2000/10, s112-25): the original cost base spreads over the resulting units
  and the acquisition date carries; cash in lieu disposes of the fraction (already documented as a
  Sell). A round-up is left to each company's class ruling
- **The unit grid belongs to the custody, not the exchange:** XNYS is whole units at SelfWealth
  (CHESS) and 3 dp at the Morgan Stanley share plan (DRP); a custodial broker sets its own cap
  (Questrade 4 dp); crypto has no exchange, and Binance's `LOT_SIZE` `stepSize` bounds *orders*
  only — fee dust leaves balances finer than it, and ETH is natively 18 dp. So the key is the
  holding account, with a per-listing override, not (account, exchange)

A live instance (found by CI's random seed, 2026-10-24, and present in v0.32.0):
`unrealised_gains`' `an_unrealised_gain_is_what_selling_everything_that_day_realises` fails
intermittently at its `total(&allocs) == row.quantity` assertion after a bonus issue and a split
that do not divide the holding — a holding row of `898.9999999999999999999999999` against its
parcels' printed remainders summing to `899.0000000000000000000000000` (seed
`cc e7cf5465c7025e26b37463289788999765705b7b94fae9c6957331b62bbb105d`: three Buys of 220/529/615,
transfers of 68% and 35% to account 2, a 1-for-4 bonus issue, a 2-for-3 split), and the reverse
(`449.50000000000000000000000001` against `449.5`) on another. Each parcel's remainder is a 28-digit
rounding of a non-terminating count, and a sum of such roundings has no single right last digit, so
this cannot be fixed inside the exact-fraction model — the per-parcel stored grid counts below are
what fix it. The seed is deliberately not committed to `proptest-regressions/` (it would fail every
run until then); add it, and see it pass, as part of closing this section.

The live portfolio's only ratio-driven action is a 1-for-1 demerger, so no stored figure is wrong
today; this is a model-correctness change. DRP's `residual_handling` (`CarryForward | PayOut`) is
the precedent: the rule is on the plan, the grid on the holding.

- [x] **Account unit precision.** Add `unit_scale` to `holding_accounts` (INTEGER, nullable: the
  number of decimal places a quantity held there may carry; NULL = unconstrained, for crypto
  wallets), plus an optional per-(account, listing) override table for the odd holding that
  differs. A Buy/Sell/DRP/transfer/allocation quantity finer than its holding's scale is refused
  `422` naming the scale. Migration seeds the scale from the finest quantity each account already
  holds (never below it — no existing row may become invalid) and leaves crypto accounts NULL.
  `docs/SCHEMA.md`, `docs/API.md`, `config.js` (holding-account form), the audited-table trigger
  pair for `holding_accounts` (new column), and a staleness-trigger classification for the
  override table. Useful on its own (it catches mistyped quantities) but does not by itself stop
  the defect class
  *Done 2026-10-24 (migration 0053, `entities::holding_unit_scale`).* Two corrections to the
  item as written: `holding_accounts` was never audited (0013 scoped it out as identity-only), so
  there was no trigger pair to re-create — neither it nor the new `holding_unit_scales` override
  joins `row_history`, since a scale changes no stored figure, only which writes are accepted; and
  the check applies to quantities the taxpayer **states** (trades, Sells and their allocations incl.
  transfer/buy-back ones, transfer-ins against the destination, ESS vest, inheritance, rights
  exercise, DRP reinvestment), not to the quantities a scrip exchange/demerger/worthless recognise
  computes — those are this section's later items' to round
- [ ] **A fraction rule on every ratio-driven action.** Add `fraction_rule` to `corporate_actions`
  (`Exact | Down | Up | Nearest | CashInLieu`, CHECK-constrained, typed enum in `model.rs`; required
  for the four ratio-driven types, absent for the rest). Existing rows migrate to `Exact`, so no
  history moves; `Exact` stays available for a registry that genuinely holds fractions. A
  `CashInLieu` action also takes the cash amount per holding (or total, apportioned) and writes the
  fraction's disposal as a Sell with its own provenance variant (`sell::SellProvenance`), dated the
  payment date. `docs/API.md` (corporate actions + rewrite *Fractional entitlements*), the
  `config.js` `fieldGroups`, and the `sel()` pin in `select_option_lists_are_pinned_to_their_server_side_source`
- [ ] **Compute per holding, store per parcel.** For a non-`Exact` action, the post-action count of
  each holding account is `round(ratio × units on the register at the record date, unit_scale,
  fraction_rule)` — register by **settlement** date, per Financial correctness — with an optional
  per-account "units received" override for a statement that disagrees. The rounded total is
  distributed across that holding's open parcels by largest remainder on the scale's grid (ties by
  acquisition date, then trade id, so it is deterministic), and each parcel's post-action count is
  **stored** (a new child table of `corporate_actions`, keyed by action + parcel), written inside
  the action's `write_tx`, rather than re-derived by multiplying each parcel by the ratio at read
  time. Each parcel keeps its whole cost base; scrip/demerger legs still split it through
  `infra::decimal::apportion`. Needs: `domain::adjustments`' `split_adjusted_quantity`/`rebased` to
  read the stored count for a non-`Exact` action; the action's undo and `?dry_run=true` preview to
  cover the child table (the operation properties in `ca5bcc0` extended to it); staleness triggers
  on it (snapshotted reports read it); audit triggers + `AUDITED_TABLES` + `config.js` picker if it
  is a financial fact table; and a re-computation (or refusal, `422`) when a trade the stored
  result depended on — a parcel settled on or before the record date — is edited or deleted
  afterwards. Property: for every non-`Exact` action, the stored parcel counts sum to the rounded
  holding total, every count is on the grid, and each parcel's count differs from its exact share
  by less than one grid step
- [ ] **Retire the exact-fraction machinery where it is no longer reached.** Once every ratio-driven
  action a holding can carry rounds onto a grid, a quantity is a terminating decimal on it
  everywhere. Decide (owner) whether `Exact` stays a permitted rule; if not, migrate existing
  `Exact` rows only where their stored results already lie on the account's grid, and simplify
  `AcquiredUnits` back to a `Decimal`, `as_acquired_quantity`'s shortest-count read-back,
  `sold_in_acquired_units`' remainder matching and `adding_up_to`'s residue — each removal with the
  property that found its defect still passing. If `Exact` stays, this item is N/A and the
  machinery remains for it alone
