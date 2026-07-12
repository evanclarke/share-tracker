# TODO

Items are only marked done when a passing test exists for them.

Completed and decided (out-of-scope / not-reproducible) sections are archived in [DONE.md](DONE.md) to keep this list focused on active work. When a section here is fully done, move it to DONE.md rather than leaving it — see CLAUDE.md.

The sections below come from the **2026-07-12 code review** (a full pass over the reports, entities,
domain pipeline, infra, and migrations for programming and domain issues). Each section records one
finding; sections land in DONE.md as they are fixed.

## Sell allocations: listing and acquisition-date invariants missing (2026-07-12 review, domain + integrity)

`upsert_sell_in_tx` (`src/entities/sell.rs:515-598`) validates that each allocated parcel exists,
is a Buy/DRP, sits in the right holding account, and is not over-allocated — but never that:

- the parcel's `listing_id` equals the Sell's `listing_id` (it is read at
  `src/entities/sell.rs:546` and used only for the splits lookup), so a Sell of listing A can
  consume parcels of listing B and the CGT reports will happily cost them cross-listing
- the parcel's trade date is on or before the sale date, so a Sell can draw on a parcel acquired
  *after* it, producing a negative holding period (the discount test just says "not eligible" and
  the reports emit nonsense figures instead of rejecting the entry)

Similarly, `trade::db_upsert` (`src/entities/trade.rs:476+`) lets an existing Buy's `listing_id`
be edited while Sell allocations reference it, silently re-associating those allocations across
listings (its capacity re-check even fetches splits for the *new* listing).

- [ ] Reject (422) an allocation whose parcel belongs to a different listing than the Sell, in the
      shared transactional core so every caller (sells, buy-back, scrip, demerger, transfer,
      worthless) inherits it
- [ ] Reject (422) an allocation whose parcel is dated after the sale date
- [ ] Reject (422, or validate against the allocations) editing a Buy's `listing_id` while
      allocations/AMIT adjustments reference it
- [ ] Tests for each rejection and message text

## PUT /trades can silently rewrite a reinvest-created DRP (2026-07-12 review, integrity)

The `PUT /trades/:id` handler rejects a body with `trade_type = DRP`
(`src/entities/trade.rs:869-874`), but nothing stops a **Buy body targeting an existing DRP row**
created by `POST /income/:id/reinvest`: `db_upsert` checks every provenance column except the
reinvestment link, which lives on `income.reinvestment_trade_id`, not on the trade. The write
re-types the trade to Buy and (because the body's residual fields default to 0) silently zeroes
the residual carry-forward chain, while the income row keeps pointing at it. `DELETE /trades`
already guards this exact reference (`src/entities/trade.rs:727-737`); the upsert path doesn't.

Related: `PUT /income/:id` accepts an arbitrary client-supplied `reinvestment_trade_id`
(`src/entities/income.rs:130`, bound at 387) with no validation that the trade exists as a DRP of
the same listing/account — and an income edit that omits the field silently clears an existing
link.

- [ ] `trade::db_upsert` rejects (422) an update to a trade referenced by
      `income.reinvestment_trade_id` (mirror the delete guard's message: delete the reinvestment
      via the income row instead)
- [ ] Decide the `PUT /income` contract for `reinvestment_trade_id` (reject client-set values, or
      validate the target is an unclaimed DRP trade of the same listing) and enforce it
- [ ] Tests: a Buy body over a reinvest-created DRP is rejected; the income-side rule is pinned

## Net-capital-gain report reads without a transaction (2026-07-12 review, programming)

`db_net_capital_gain` / `gross_buckets` / `e10_gains` / `g1_gains`
(`src/reports/net_capital_gain.rs:404-511`) run many separate queries directly on the pool: the
realised-gains rows come from `db_realised_gains`'s own (correct) snapshot, then AMMA rows, AMIT
adjustments, ROC/split events, allocations, FX rates, and the opening loss are each read at later
instants. CLAUDE.md's report rule requires one `pool.begin()` read transaction per multi-query
report so an interleaved write can't produce inconsistent inputs (e.g. an AMMA row arriving
between the realised read and the E10 walk double- or under-counting a year). The what-if handler
(`what_if_handler`) has the same shape.

- [ ] Restructure the report to read every input on one read transaction (likely: extend
      `realised_gains::load_report_data`-style loading, or take a `&mut SqliteConnection` through
      `gross_buckets`/`e10_gains`/`g1_gains`), keeping the computation pure
- [ ] Same for the what-if path
- [ ] A test proving the report still reproduces its fixtures (existing tests should carry this)

## Tax summary: franking holding-period test runs post-commit, per dividend (2026-07-12 review, programming)

`db_tax_summary` reads its inputs on one transaction, commits it, then calls
`franking::holding_period_test(pool, …)` **per franked dividend**
(`src/reports/tax_summary.rs:551-565`), each call issuing three more queries (listing preference,
trade walk, splits) on the raw pool. That both breaks the single-snapshot rule (a trade written
after the commit changes the denial outcome for a summary computed from older facts) and is an
N+1 on a report that already pre-loads everything else.

- [ ] Run the holding-period walks inside the same read transaction as the rest of the report
      (thread a `&mut SqliteConnection` through `holding_period_test`, which
      `franking_at_risk` can share), and batch the per-listing lookups (preference, trades,
      splits) instead of re-querying per dividend
- [ ] Existing denial tests keep passing; add one covering two dividends on one listing reusing
      the loaded walk

## No positivity/sanity validation on ordinary trade, Sell, allocation, and income amounts (2026-07-12 review, integrity)

The linked operations validate their inputs (`units <= 0` rejected in buy-back, rights exercise,
ESS vest, DRP reinvest, inheritance…), but the plain CRUD paths accept degenerate values, and the
schema has no CHECKs on them (`migrations/0001_schema.sql:369+`, `:444`):

- `PUT /trades` / `PUT /sells`: zero or negative `quantity` and `average_price`, negative
  brokerage/GST, and a `settlement_date` before the trade date are all accepted
- Sell `allocations`: a zero or **negative** `quantity_allocated` passes both the sum check and
  the per-parcel capacity check (e.g. −5 on parcel A and +105 on parcel B "sums" to a 100-unit
  Sell), quietly increasing another parcel's capacity
- `PUT /income` / `PUT /interest_income`: negative amounts accepted on every money column

A negative or zero quantity corrupts every downstream report without failing anything, which is
exactly what the write-time-invariant rule exists to prevent.

- [ ] Decide the exact rule set (quantity > 0, price ≥ 0, brokerage/GST ≥ 0, allocation units > 0,
      income components ≥ 0, settlement ≥ trade date) and enforce it at write time with clear 422
      bodies
- [ ] Tests per rejected shape; docs/API.md 422 causes updated

## Cost-base FX timing: AMIT/ROC reductions convert at the acquisition-month rate (2026-07-12 review, domain — decide)

`CostBase::into_aud_with` (`src/domain/cost_base.rs:204-239`) deliberately resolves **one** rate —
the parcel's acquisition month — and applies it to every component, including the AMIT
(`amit_reduction`) and return-of-capital (`roc_reduction`) reductions that happened in later,
possibly very different rate months. The translation rules (s 960-50; `docs/ato/
forex-common-transactions.md` translates each leg at its own transaction time) point at
translating each reduction at the rate of the period/payment it belongs to. The codebase is also
internally split: `g1_gains` converts a payment's *excess* at the **payment month**
(`src/reports/net_capital_gain.rs:389-390`) while the same payment's *reduction* inside the cost
base converts at the acquisition month.

In practice this only bites on non-AUD holdings with non-AUD ROC/AMMA reductions (none in the
live data — E10/G1 events are on AUD funds), so it may be acceptable to resolve out of scope.

- [ ] Decide explicitly: convert each reduction at its own event/period month (extending the
      pipeline to carry per-event rates), or record the single-rate simplification as a Known
      limitation with the citation, noting the g1_gains asymmetry either way

## AMMA tax_year_end_date is assumed to be 30 June but never validated (2026-07-12 review, integrity)

Every AMMA-keyed report buckets the statement by `tax_year_end_date.year()`
(`src/reports/tax_summary.rs:422`, `src/reports/net_capital_gain.rs:477`,
`src/reports/franking.rs:305`), which equals the Australian FY only when the date is in
January–June (in practice, 30 June). Nothing validates that at write time
(`src/entities/amma.rs`), so a statement keyed e.g. `2024-12-31` lands in FY2024 while
`domain::tax_year::tax_year_for` — the rule CLAUDE.md says every FY-keyed report must use — would
put it in FY2025.

- [ ] Either validate at write time that `tax_year_end_date` is a 30 June date (422 otherwise),
      or bucket AMMA rows through `tax_year_for` everywhere; pick one and pin it with a test

## Scheduler nits: wrong line number in UnknownJob; no overlap guard (2026-07-12 review, programming)

- `spawn` reports `ScheduleError::UnknownJob { line: idx + 1 }` where `idx` indexes the *parsed
  entries*, not the schedule file — comments and blank lines shift the reported line
  (`src/infra/scheduler.rs:331-338`; `parse` carries the real line number but drops it)
- [ ] Carry the source line through `ScheduleEntry` so the error points at the real line; test
      with a schedule containing comments
- Nothing prevents the same job running concurrently: `POST /jobs/{name}` executes inline in the
  handler and can overlap the scheduled run (or a second manual trigger) — e.g. two simultaneous
  `backup`s race the same destination second, two `price-import`s double-fetch
- [ ] Serialise per-job execution (a per-job async mutex around `run_job`, or reject a trigger
      while the job is running with 409) and test it

## Buy-back participation collapses all Sell-side rejections into one message (2026-07-12 review, UX)

`ParticipationError::Sell` maps every sell invariant failure to the generic "the holding cannot
cover the units participated (over-allocated parcels)"
(`src/entities/buyback_participation.rs:127-132`), so e.g. an allocation in the wrong holding
account (`PurchaseInDifferentAccount`) or an allocation-sum mismatch is misreported. This
contradicts the useful-error-messages convention (every 422 says which invariant failed).

- [ ] Pass the underlying `SellError`'s own 422 body through (it already has one per variant) and
      assert the distinct texts in tests
