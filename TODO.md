# TODO

Items are only marked done when a passing test exists for them.

Completed and decided (out-of-scope / not-reproducible) sections are archived in [DONE.md](DONE.md) to keep this list focused on active work. When a section here is fully done, move it to DONE.md rather than leaving it — see CLAUDE.md.

The sections below come from the **2026-07-12 code review** (a full pass over the reports, entities,
domain pipeline, infra, and migrations for programming and domain issues). Each section records one
finding; sections land in DONE.md as they are fixed.

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
