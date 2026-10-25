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
- [x] **A fraction rule on every ratio-driven action.** Add `fraction_rule` to `corporate_actions`
  (`Exact | Down | Up | Nearest | CashInLieu`, CHECK-constrained, typed enum in `model.rs`; required
  for the four ratio-driven types, absent for the rest). Existing rows migrate to `Exact`, so no
  history moves; `Exact` stays available for a registry that genuinely holds fractions. A
  `CashInLieu` action also takes the cash amount per holding (or total, apportioned) and writes the
  fraction's disposal as a Sell with its own provenance variant (`sell::SellProvenance`), dated the
  payment date. `docs/API.md` (corporate actions + rewrite *Fractional entitlements*), the
  `config.js` `fieldGroups`, and the `sel()` pin in `select_option_lists_are_pinned_to_their_server_side_source`
  *Done 2026-10-24 (migration 0054, `corporate_action::model::FractionRule`).* The rule is stored,
  CHECK-constrained, required on the four ratio-driven types and refused on the rest (`422` naming
  which), audited (0054 re-creates the trigger pair; the backfill itself writes no trail entry),
  on the form and in the docs; existing ratio-driven rows read `Exact`. One part **moved to the next
  item** rather than built here: the `CashInLieu` Sell. Its quantity is the holding's exact count
  minus its rounded-down count, which after a 1-for-3 consolidation is a non-terminating figure
  that only the per-parcel stored counts below make exact, and which parcel(s) give up the fraction
  is that item's largest-remainder distribution — written now, it would be reworked by it. The
  cash-input shape is decided: **per holding**, not a total apportioned — the registry pays each
  holding its own cheque for its own aggregated fraction, which is the figure the holder's
  statement prints, so it is an operation's input on the action (one call per holding, at
  payment), not a column on it. Until then every rule is computed as `Exact` (`docs/API.md`
  *Fractional entitlements* says so), and a received cash-in-lieu payment is entered as an
  ordinary Sell as before
- [x] **Compute per holding, store per parcel.** For a non-`Exact` action, the post-action count of
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
  by less than one grid step. Also, moved here from the item above (2026-10-24): a `CashInLieu`
  action's disposal of the fraction — per holding, an operation on the action (e.g.
  `POST /corporate_actions/{id}/cash_in_lieu`) taking the holding account and the cash that holding
  was paid, writing a Sell of the remainder (the exact count minus its rounded-down count on the
  holding's `unit_scale`) with its own `sell::SellProvenance` variant (a new `trades` provenance
  column, so the `trades` audit trigger pair is re-created and the action is frozen while it
  exists), dated the payment date; with undo (delete) and `?dry_run=true`
  *Done 2026-10-24 (migrations 0055/0056; `domain::entitlement_rounding`,
  `corporate_action::rounding`, `entities::entitlement_override`, `entities::cash_in_lieu`).*
  Decisions made in building it: **a NULL `unit_scale` keeps the action exact** for that holding
  (no grid to round onto — a crypto wallet holds what the ratio produces; refusing would make an
  action with one such holding unrecordable), storing nothing for it. **The units an action
  converts are decided by trade date, not settlement date**: the parcels traded before its `date`
  less the sales dated before it, because that is the set every quantity re-base converts (a
  split/bonus has no record date here, and its `date` decides which basis a trade is stated in) —
  rounding any other set would disagree with the re-base; a Known limitation says so. The
  **units-received override** is its own audited table/entity (`/entitlement_overrides`, one per
  action × account), recomputing a split/bonus's listing on write and read by an exchange/demerger
  when it runs (frozen after). The distribution gives the left-over grid steps first to a parcel the
  round-down would empty, then by largest remainder, acquisition date, trade id — every count stays
  within a step of its exact share; a total too small to give each parcel a step is **refused 422**
  (a zero-unit parcel would strand its cost base). **Dependent-fact edits are refused, not
  recomputed**: 0055's guard triggers refuse a trade/sale/allocation dated before a rounded
  split/bonus (in a holding with a grid) and a change to that holding's scale/override, answered
  422 via an `infra::http` classifier; the action's own edits (ratio, rule, date, an earlier
  action) and override writes recompute the listing in their transaction. The child table
  `corporate_action_parcel_counts` is **not audited** (derived from audited, frozen inputs — the
  `report_snapshots` ground) but is staleness-triggered; `entitlement_overrides` is audited. For a
  scrip exchange/demerger the stored count is the replacement Buy's quantity (row keyed on that
  Buy, cascading with the operation's undo). `CashInLieu` carries the fraction on one parcel (to 12
  places beyond the grid) until `POST /corporate_actions/{id}/cash_in_lieu` sells it
  (`SellProvenance::CashInLieuAction`, `trades.cash_in_lieu_action_id`, trades audit pair re-created,
  action frozen, undo = `DELETE /sells`, `?dry_run=true`). Readers convert a parcel through
  `ListingSplits::for_parcel` (the loaders no longer return a slice); the activity ledger's running
  balance applies the stored sums. Properties: `domain::entitlement_rounding`'s pure property, and
  `entities::tests::operations`' new rounded-world property (every rounded operation incl. cash in
  lieu previews and undoes exactly, and the stored counts of every rounded holding sum to its rounded
  total, on the grid, each within a step of its exact share, after every step). **The CI seed** is
  committed — to `proptest-regressions/entities/corporate_action/tests/rounding.txt`, against
  `a_rounded_unrealised_gain_is_what_selling_everything_that_day_realises`, which runs
  `unrealised_gains`' property body and strategy unchanged with every ratio action rounded on
  whole-unit accounts — and passes there; a second instance this work's own run found
  (`d841d284…`, a 2-for-3 exchange) is committed beside it. Both still fail the `Exact` property in
  `reports::unrealised_gains`, so they are not in that file: that run fails intermittently on random
  seeds until item 4 decides `Exact`'s fate
- [ ] **Retire the exact-fraction machinery where it is no longer reached.** Once every ratio-driven
  action a holding can carry rounds onto a grid, a quantity is a terminating decimal on it
  everywhere. Decide (owner) whether `Exact` stays a permitted rule; if not, migrate existing
  `Exact` rows only where their stored results already lie on the account's grid, and simplify
  `AcquiredUnits` back to a `Decimal`, `as_acquired_quantity`'s shortest-count read-back,
  `sold_in_acquired_units`' remainder matching and `adding_up_to`'s residue — each removal with the
  property that found its defect still passing. If `Exact` stays, this item is N/A and the
  machinery remains for it alone
  - **N/A — decided (owner, 2026-10-25): `Exact` stays.** Investigation showed retiring it removes
    nothing: `AcquiredUnits`' exact fraction is still reached under rounding (a partial sale of a
    parcel re-based by stored after/before counts — 100 → 300 rounded, sell 200 — is 66.666…
    as-acquired units, the `a20c26d` defect), legacy `Exact` split/bonus rows whose per-parcel
    counts do not terminate cannot migrate to any rule without moving history (so the other three
    pieces stay reachable for them), and `Exact` is the only valve for 0055's freeze guards. The
    items below replace the valve and close the remaining gaps instead
- [x] **Recompute on write instead of freezing.** Replace 0055's freeze guards with recomputation: a
  trade/sale/allocation/settlement-date/unit-scale write that touches a holding before a rounded
  split/bonus recomputes that listing's stored counts inside its own `write_tx` (the existing
  `db_recompute_listing`, then the representability and `allocations_fit_parcels` checks), refusing
  `422` only a write whose sales genuinely no longer fit. Make it structural rather than per-path:
  triggers mark the listing dirty and a write path that forgets to settle fails at COMMIT (e.g. a
  `rounding_dirty` table with a deferred FK to a parent that never exists). **Stable distribution**:
  a recompute keeps each existing parcel's stored count where the rounded total allows, and a
  back-dated parcel absorbs the difference — a registry does not redistribute, and otherwise a
  back-dated parcel can take a grid step from a sibling that has already sold it (parcel A 100, 1-for-3,
  A stored 34 and sold; a back-dated B of 50 would redistribute 50 as A 33 / B 17). Docs (API.md
  limitations + response codes, SCHEMA.md, 0055's narrative superseded); property: the rounded-world
  operations property gains back-dated trade/sale writes and scale changes, with preview, undo and
  the stored-count invariants holding after each, and no existing parcel's count moving when a
  back-dated parcel is added unless the total forces it
  *Done 2026-10-25 (migration 0057, `corporate_action::rounding::db_settle`).* 0055's ten guards
  are dropped. **Structural:** 0057's triggers mark a listing with a non-`Exact` split/bonus in
  `rounding_dirty` on any trade (listing/date/settlement date/quantity/type/account),
  allocation, unit-scale, override or action write; each row violates a deferred FK to the
  never-populated `rounding_never`, so COMMIT (or an autocommit statement) fails until `db_settle`
  recomputes and drains it — verified by a test that commits unsettled. `db_settle` is called on
  every write path (trade upsert/delete, Sell write/delete, transfer and its delete, DRP
  reinvest/undo, ESS vest/statement delete, inheritance write/delete, rights exercise, exchange,
  demerger, buy-back, worthless, cash in lieu, holding-account and unit-scale writes — the
  unit-scale delete is now hand-written — overrides, action upsert/delete, the
  `settlement-recompute` job) and carries its `422`s back as an `infra::http::Refusal` inside
  `sqlx::Error`. **Stable distribution** (`round_holding`'s `Prior`): a stored count is kept while
  it is still valid for its parcel (on the grid, its share's floor or one step over); kept counts
  are released — changed parcels first, then latest acquisition, then higher trade id — only while
  the free parcels cannot absorb the total within a step each (zero floors taking one). Every count
  thus stays within a step of its exact share (the old invariant still holds); the new invariant is
  that an unchanged parcel's count moves only when keeping all of them is infeasible. An action's
  own term edit and an override write distribute afresh. Remaining `422`s: a later sale no longer
  fitting, a paid cash-in-lieu fraction moved (names the Sell), an emptied parcel, an override off
  the grid. Properties: `entitlement_rounding`'s recompute property, and the rounded operations
  property with back-dated Buys/Sells and grid changes (stability after every write and undo; an
  undo is exact when the write moved no count, else restores every holding's parcels, units and
  total — stability makes the rest history-dependent, a Known limitation)
- [x] **Register by settlement date.** Item 3 rounds the units held by **trade** date before a
  split/bonus's date (a Known limitation), against Financial correctness's rule that the register is
  the settlement date. Round the units **registered** (Buys settled on/before, Sells settled after,
  by `settlement_date`) at the action's record date, giving splits/bonuses a record date if the
  model needs one, and reconcile that with the unit basis a trade straddling the action is stated
  in. Remove the Known limitation; property over trades straddling the action's date
  *Done 2026-10-25 (migration 0058, `corporate_action::rounding`,
  `domain::entitlement_rounding::round_registered_holding`).* **Record date:** an optional
  `record_date` on `ShareSplit`/`BonusIssue`, either side of `date` (ASX: ex + 1 business day;
  NYSE forward split: before the ex-date), stored in a new audited column `rebase_record_date`
  (the existing `record_date` is ROC-only and CHECKed `<= date`, and a CHECK cannot be altered);
  read only under a non-`Exact` rule. NULL = the timetable's register (every trade dated before
  `date`), which is what every existing row rounded, so no stored count moves. **Straddling
  trades:** the action's `date` stays the ex-date that decides each trade's unit basis, and the
  stored counts stay keyed on the parcels traded before it, so re-base and rounding always cover
  the same units. Under the market timetable (ASX deferred settlement; US due bills) the register
  equals that set for every normally settled trade, so only a trade traded before the ex-date and
  settled after the record date straddles; it is a settlement obligation converted on its own —
  a straddling **Buy**'s parcel is rounded alone (the rule; `CashInLieu` → down, the fraction
  being the registered seller's), a straddling **Sell**'s units are rounded with the holding and
  its own converted count delivered out of the rounded total, the remaining parcels sharing the
  rest pro rata (within a step each, stable). A trade dated on/after the ex-date takes no part
  however it settles. The trade-date Known limitation is replaced by a narrower one: a rounded
  scrip exchange/demerger still converts the parcels open on its operation date by trade date (no
  record date of its own). Properties: `entitlement_rounding`'s registered-holding property, and
  `a_rounded_split_rounds_the_register_at_its_record_date` (straddling Buys/Sells, record date
  ±3 days or absent, every rule, then a settlement-date move) against an independent model of
  the register, checking totals, grid, within-a-step and that the re-base reads the same counts
- [ ] **Fix the `Exact`-mode `unrealised_gains` property's intermittent failure.** Under `Exact` a
  holding row (`898.9999999999999999999999999`) and its parcels' printed remainders (`899`) can
  disagree in the 28th digit (seeds `e7cf5465…`, `d841d284…`). Since `Exact` stays, decide what the
  assertion should mean for a non-terminating holding — compare within the rounding the printed
  figures carry, or make the holding row the sum of the printed parcel remainders so the two agree
  by construction (preferred if it is what a reader of the report expects) — and commit both seeds
  to `proptest-regressions/reports/unrealised_gains.txt`, passing
- [ ] **Two latent property failures found while closing "Recompute on write" (2026-10-25)**, both
  present before it: (1) `a_rounded_unrealised_gain_is_what_selling_everything_that_day_realises`
  can generate a rounded split that empties a parcel (five 1-unit Buys, a 1-for-2 split, rule
  `Down`) and fails on the documented `422` — the strategy, or the property, needs to treat that
  refusal as a legitimate outcome; (2) an exchange/demerger of a holding left exact (no grid, so a
  non-terminating count such as 1000 × 5/3) into a listing with a grid, rounded `Up`, overshoots by
  a grid step when the 28-digit `units_before` rounds an exact count (1250) to 1250.000…0002
  (reachable before it through an unconstrained override entered ahead of the action) — the
  rounded operations property's grid-change op is limited to a grid for now because of it

