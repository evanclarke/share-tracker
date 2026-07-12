# TODO

Items are only marked done when a passing test exists for them.

Completed and decided (out-of-scope / not-reproducible) sections are archived in [DONE.md](DONE.md) to keep this list focused on active work. When a section here is fully done, move it to DONE.md rather than leaving it — see CLAUDE.md.

The sections below come from the **2026-07-12 code review** (a full pass over the reports, entities,
domain pipeline, infra, and migrations for programming and domain issues). Each section records one
finding; sections land in DONE.md as they are fixed.

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
