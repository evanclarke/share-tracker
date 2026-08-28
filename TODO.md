# TODO

Items are only marked done when a passing test exists for them.

This file holds only open / in-flight work. Completed and decided (out-of-scope / not-reproducible)
sections are archived in the topical `DONE/*.md` files, indexed by [DONE.md](DONE.md). When a
section here is fully done, move it into the matching `DONE/*.md` file rather than leaving it — see
CLAUDE.md.

A section records one finding, and its heading names where it came from — a REQUIREMENTS entry, a
[SCENARIOS.md](SCENARIOS.md) section, or a dated review pass.

**Open: one section** — the 2026-08-28 cyclomatic complexity audit below. Every section recorded
before it is closed and archived. The closing narrative that used to stand here — the pass-by-pass
record of driving SCENARIOS.md sections S through AA, and the last two sections to close (the
distribution calendar and the 2026-08-25 code review) — was moved to
[`DONE/verification-passes.md`](DONE/verification-passes.md) on 2026-08-28. The maintained record of
what has been verified is SCENARIOS.md's [Verification status](SCENARIOS.md#verification-status)
table and its per-section findings blocks; the maintained record of what was built and decided is
the `DONE/*.md` archive.

## Cyclomatic complexity audit (2026-08-28)

Measured with `lizard` (classic cyclomatic complexity, CCN) over all 3,966 Rust functions in `src`,
with functions inside `#[cfg(test)]` items classified out by brace-matching, cross-checked against
`cargo clippy -W clippy::cognitive_complexity` (which scores *nesting* rather than branch count) and
a per-function max-nesting-depth measurement. JS modules measured separately.

**The codebase is healthy and no broad refactor is warranted.** 1,373 production functions,
31,393 production NLOC (against 58,907 NLOC of tests — tests are 65% of all functions and are
uniformly simple). Median CCN 2, mean 3.66; 92% of production functions are under CCN 10.

| Folder | fns | NLOC | mean CCN | max | >15 | >25 |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| `entities/` | 607 | 14,716 | 3.93 | 129 | 33 | 17 |
| `reports/` | 259 | 8,043 | 6.24 | 84 | 30 | 8 |
| `infra/` | 149 | 1,940 | 2.72 | 18 | 1 | 0 |
| `src` root | 281 | 5,523 | 1.37 | 18 | 1 | 0 |
| `domain/` | 77 | 1,171 | 2.99 | 12 | 0 | 0 |

`domain/` and `infra/` — the shared calculation and cross-cutting layers, where a tangle would be
most costly — are the two cleanest folders, `domain/` having no function above CCN 12 at all. The
tail sits where the work is: `entities/` write paths and `reports/` aggregations.

**Most of that tail is deliberate and should stay.** Of the 39 production functions above CCN 20,
29 are flat (max nesting ≤ 3 counting the fn body as 1) — sequential write-time guard chains and
one-arm-per-type decision tables, each arm carrying a comment stating the invariant it defends.
That is the shape CLAUDE.md mandates ("Enforce data-model invariants at write time inside a
transaction"), and the gap between the two tools confirms it: `corporate_action::model::kind` scores
CCN 129 but only 29 on clippy's cognitive scale, because its branching is breadth, not depth.
Dispersing those guards into helpers would scatter a correctness argument that currently reads top
to bottom. The items below are only the cases where the complexity is *accidental* — where a
decomposition exists that moves no invariant.

- [x] Split `reports::tax_summary::db_tax_summary_on` (`src/reports/tax_summary.rs:558`, CCN 84 —
  second-highest in the codebase, 276 NLOC). Strongest and lowest-risk candidate: after the input
  loads it is four *independent* accumulation loops (income `:661`, interest `:739`, AMMA `:760`,
  ESS `:823`), each folding a different row type into the same `HashMap<i32, TaxYearSummary>` and
  interacting with each other only through that map. Each becomes an `accumulate_*(&mut map, &rows,
  &fx, …)` free function with no shared mutable state beyond the map and no invariant spanning the
  cut. Behaviour-preserving, so the existing tax-summary and `ato_examples` tests are the proof

  **Done 2026-08-28.** `db_tax_summary_on` is CCN 20 / 80 NLOC, down from 84 / 276; nothing in the
  file is above CCN 18. The audit's count of four loops was low — there were **six** row-driven
  accumulations and two whole-map post-passes, all eight now named:

  - `accumulate_income` (12), `accumulate_interest` (9), `accumulate_amma` (18), `accumulate_ess`
    (14), `accumulate_expenses` (10) — one per kind of record, each folding only its own rows in
  - `apply_franking_denials`, `apply_fito_de_minimis`, `total_assessable_income` — the ordered part,
    which is now visible *as* the ordered part rather than being three more loops in a run of eight

  What made the cut safe is that the five `accumulate_*` passes touch disjoint summary lines and
  compose in any order, so no invariant spans a boundary; the three `apply_*`/total passes are the
  ones that must run after everything is aggregated, and the orchestrator's doc comment now says so
  — a sequencing constraint that was previously implicit in the order of a 276-line body. Two
  supporting pieces: a `Summaries` type alias for the shared map, and `year_entry`, which replaces
  the five copies of `map.entry(ty).or_insert_with(|| zero_summary(ty))`.

  `accumulate_ess` deliberately keeps *two* loops: the per-year $1,000 taxed-upfront reduction reads
  `ess_eligible_by_year`, which the row loop builds and which now never escapes the function — the
  encapsulation the split bought. Every comment moved with its code, the ones describing a whole
  pass becoming that function's doc comment. Proof is the unchanged suite: 91 tax-summary tests and
  the 38 `ato_examples` acceptance tests (which assert the ATO's own stated figures end to end via
  HTTP), 2,336 total, plus fmt and `clippy -D warnings` clean.
- [x] Extract the two nesting outliers, each one contained inner block (these are the only two
  production functions nested deeper than 6):
  - `entities::currencies::parse_iso4217` (`src/entities/currencies.rs:234`, nest 8, CCN 19) — the
    depth is the standard `loop { match event { … } }` SAX shape, which should stay; the extractable
    piece is the `CcyNtry` End arm that parses minor units and builds the `Currency`
  - `reports::valuation::valuations_of_markets` (`src/reports/valuation.rs:220`, nest 7, CCN 16) —
    the per-market guard sequence is readable as is; the depth comes from the `_ if unpriced` arm's
    nested `match` on the carry-forward lookup, which lifts out as a `carry_forward_price` helper

  **Done 2026-08-28.** Both are now nested no deeper than 6, so no production function in the tree
  is. Neither cut moves an invariant, and the audit's judgement that the surrounding shapes should
  stay was upheld — the SAX event loop and the per-market guard sequence are untouched.

  - `parse_iso4217` (nest 8 → 6, CCN 19 → 17, 98 → 75 NLOC). The four in-flight `Option<String>`
    field variables became a `CcyNtry` struct, so the entry reset is one `CcyNtry::default()` rather
    than a four-tuple, and the End arm's body is `CcyNtry::into_currency(seen)` (CCN 4) — the
    minor-unit parse and the dedup-and-build. What the extraction buys beyond depth is that the
    entry-level rules now have somewhere to be stated: which entries are skipped (no `<Ccy>`, or a
    code already seen), and that `N.A.` means no minor units while a malformed value fails loudly,
    were three sentences in the *function's* doc comment describing something two nesting levels
    down; they are `into_currency`'s doc comment now, and `parse_iso4217`'s says only what the walk
    itself does
  - `valuations_of_markets` (nest 7 → 6, 141 → 131 NLOC; CCN stays 16 — the arm's two-way branch
    remains at the call site, which is the point: the *blocker* it pushes belongs with every other
    blocker in the loop). The `_ if unpriced` arm's inner `match` on the carry-forward lookup is now
    `carry_forward_price(conn, market, valuation_day)`, returning `Option<Decimal>`; the SCENARIOS
    Q-02 rationale stays on the arm, and the reason `None` is a safety net rather than a live path
    (`db_upsert` guarantees an earlier ok price) moved onto the helper, which is what guarantees it

  Both are behaviour-preserving, so the existing suite is the proof: the currency-import parse tests
  (incl. the minor-units/dedup/missing-code and entity-reference cases) and the valuation,
  snapshot and period-performance tests, 2,336 total, with fmt and `clippy -D warnings` clean.
- [x] Review `entities::rights_sale::db_sell_rights` (`src/entities/rights_sale.rs:314`, CCN 47,
  nest 6, 176 NLOC) — the highest-CCN function that is *not* flat, so unlike the guard chains its
  branch count is not explained by breadth. Decide whether the record-date anchoring walk separates
  from the write, or record here that it does not and why

  **Verdict 2026-08-28: it separates, cleanly.** The walk is a *validation pass and nothing else* —
  it computes no figure the write goes on to store, so its only output is Ok-or-refuse and the cut
  moves no invariant. `db_sell_rights` is now CCN 21 / 98 NLOC / nest 3, down from 47 / 176 / 6, and
  nothing in the file is above CCN 21. Four named pieces came out:

  - `check_parcel_anchoring` (6) — the per-parcel cap loop, over `parcel_units_at_record_date` (15,
    the anchoring walk proper: the parcel is an original Buy/DRP of the listing, less the units sold
    before the record date, re-based to record-date units) and `prior_anchored_units` (4)
  - `check_body` (6) — the checks that need nothing but the request, which is why they run *before*
    the transaction is opened; it returns the two defaulted amounts as a `SaleAmounts`
  - `resolve_fx_rate` (5) — the stated-rate / ATO-rate / nil-lapse-parity decision, with
    `SaleAmounts::converts_nothing` naming the condition its comment had to spell out inline

  What the orchestrator reads as now is the order the refusals are owed in — request figures, the
  offer's terms, the total cap, the per-parcel cap, then the rate and the insert — and its doc
  comment says so. Unlike the guard chains the audit ruled should stay, none of these are guards in
  one sequence: they are three different *kinds* of question (the request alone, one parcel at a
  time, the sale month's rate), which is why the branch count wasn't breadth.

  **The extraction found a live bug, now fixed.** The per-parcel cap read each parcel's *stored*
  prior anchoring inside the allocation loop and added it to the in-request running total on every
  iteration, so a request naming one parcel in two allocations counted that parcel's earlier sales
  twice and refused a request that was inside the cap purely for having been split — the very shape
  the loop's own comment says is allowed ("a request can't split one parcel over two allocations to
  dodge the cap"). The prior figure is now read once per distinct parcel and carried forward by the
  running total. It was over-strict, never permissive, so nothing invalid was ever accepted and no
  stored figure is wrong. Pinned by `splitting_one_parcel_over_allocations_counts_prior_sales_once`,
  which fails with `ExceedsParcelEntitlement` against the old logic, and which re-asserts the cap
  itself still bites afterwards so the fix can't be read as having loosened it. The rest is
  behaviour-preserving, so the file's other 27 tests are the proof: 2,337 total, fmt and
  `clippy -D warnings` clean.
- [ ] Consider a `Presence` flags struct for `entities::corporate_action::model::kind`
  (`src/entities/corporate_action/model.rs:498`, CCN 129 — the codebase maximum, 3.5× the next).
  The one-arm-per-action-type table itself should **not** be broken up. But 84 of its ~129 decision
  points are `&&`/`||` in the repeated "every other type's fields absent" negation chains; naming
  the presence flags in a small struct with an `only(&[…])` helper would collapse the repetition
  without moving the table or its comments. Judgement call — leave it alone if the rewrite reads
  worse than what it replaces, and record that verdict here
- [ ] Give `entities::sell::upsert_sell_in_tx` (`src/entities/sell.rs:572`) a parameters struct — 9
  parameters, the widest signature in the tree (only 6 production functions exceed 6). The
  neighbouring `checks.rs` already uses this shape (`AmountsCheck`, `StatementTotalCheck`)
- [ ] Decide whether to gate complexity in CI. `clippy::cognitive_complexity` is nursery-level and
  currently flags 16 functions at its default threshold of 25 — 5 of them test functions in
  `doc_checks.rs` plus `row_history.rs:1623`, which are assertion sequences rather than logic.
  Turning it on under the existing `-D warnings` policy would therefore need those allowed
  individually first. Worth it only if the allow-list stays small; if not, record the decision here
  so it is not re-litigated

**Not findings, recorded so this ground is not re-audited:** the JS modules are clean (max CCN 46 in
`app.js`'s `refreshHealthBanner`, whose breadth is *mandated* by
`health_banner_renders_every_field_of_the_health_report` — every health field must be read there;
then 31 in `util.js`'s `decParts`, exact decimal arithmetic). The long flat entity write paths
(`trade/db.rs:316` CCN 57, `corporate_action/db.rs:447`, `income.rs:672`, `sell.rs:572`) and the
per-owner dispatch in `reports::attachments::db_attachments` (`:48`, CCN 35, zero boolean operators
— six `else if` arms, one per owner FK) were each read and judged correct as they stand.
