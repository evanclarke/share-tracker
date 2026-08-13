# TODO

Items are only marked done when a passing test exists for them.

Completed and decided (out-of-scope / not-reproducible) sections are archived in the topical `DONE/*.md` files, indexed by [DONE.md](DONE.md), to keep this list focused on active work. When a section here is fully done, move it into the matching `DONE/*.md` file rather than leaving it — see CLAUDE.md.

The sections below come from the **2026-07-13 improvement review** (a whole-project pass for
operational and test-strategy gaps, as distinct from the 2026-07-12 programming/domain review whose
findings are all closed in DONE.md), except where a section's heading names another source
(e.g. REQUIREMENTS). Each section records one finding; sections land in DONE.md as they are fixed
or decided.

## AMIT adjustment cross-check and generation (REQUIREMENTS 2026-08-13)
Entering an AMMA statement creates nothing else: the per-parcel `amit_adjustments` rows that apply
its per-unit `cost_base_adjustment` are hand-entered afterwards (FY2025 VDHG needs 30). Each row is
validated in isolation by `amit_adjustment::db_upsert` (Buy/DRP, listing, holding account, quantity
cap) but the *set* is never checked against its statement, so a missed parcel silently overstates
cost base and a duplicated one over-reduces it — and because CGT event E10 floors at nil, an
over-reduction can manufacture a capital gain. Two halves: a cross-check report that verifies a set,
and a generation action so the set need not be typed at all. No schema change beyond the optional
UNIQUE index below.

- [ ] New non-blocking report `GET /reports/amit_adjustment_cross_check`
      (`src/reports/amit_adjustment_cross_check.rs` + `pub mod` / `.merge(...)` in
      `src/reports/mod.rs`), following `reports::amit_cash_cross_check`/`e4_cross_check`: all inputs
      on one `pool.begin()` read transaction, empty result = everything reconciles. One row per
      flagged AMMA statement carrying `amma_statement_id`, `listing_id`, `ticker`, `tax_year`
      (via `domain::tax_year::tax_year_for`), `holding_account_id`, `units_held`, `units_adjusted`,
      `parcel_count`, and the list of problems found
- [ ] Check: **no adjustments at all** on a statement whose `cost_base_adjustment` is non-zero
      (highest signal — the whole statement's cost-base effect is missing). A statement with a zero
      per-unit figure is not flagged
- [ ] Check: **coverage mismatch** — Σ `amit_adjustments.quantity` ≠ `amma_statements.units_held`,
      reported with the signed difference. Must re-base through the listing's splits before
      comparing (`corporate_action::adjustments::split_adjusted_quantity` / `as_acquired_quantity`):
      adjustment quantities are as-acquired units, `units_held` is the statement year's basis, so a
      naive comparison false-positives on any split
- [ ] Check: **duplicate parcel** — the same (`amma_statement_id`, `trade_id`) pair more than once
- [ ] Check: **parcel outside the statement's year** — the two unambiguous cases only: trade `date`
      after `tax_year_end_date`, or the parcel fully consumed by allocations whose sale trades all
      predate 1 July of that FY. A parcel disposed of *during* the year is legitimate and must not
      be flagged
- [ ] Write-time: duplicate (`amma_statement_id`, `trade_id`) pairs rejected `422` from
      `amit_adjustment::db_upsert` — a new `UpsertError` variant with its arm in the existing
      `From<UpsertError> for ApiError` impl. Unlike the other checks this is a real data-model
      invariant. Verify the **deployed** DB (bigbrain.lan, not the repo copy) has no existing
      duplicate pairs before adding a UNIQUE index in a migration; the repo copy was clean as at
      2026-08-13
- [ ] `POST /amma_statements/:id/generate_adjustments` — creates one `amit_adjustment` per open
      parcel as at `tax_year_end_date`, sourced from `domain::open_parcels::load(conn, as_of)` with
      each `remaining_as_of` converted back to the as-acquired basis the quantity column stores, and
      filtered to the statement's own `listing_id` + `holding_account_id`. All rows in one
      transaction (no partial set can persist), each written **through
      `amit_adjustment::db_upsert`** rather than a bulk INSERT so the per-row invariants and the
      `row_history` audit trail apply to generated rows exactly as to typed ones
- [ ] Generation response echoes `created` (the rows), `units_adjusted`, `units_held` and their
      difference. A mismatch does **not** block the write — it is a reconciliation, not an invariant
      (a statement may state units at a date other than year end) — it is surfaced in the response
      and stays flagged by the cross-check report until resolved
- [ ] Generation refuses `422` when: the statement already has adjustments (unless `replace: true`,
      which deletes and regenerates in the same transaction); there are no open parcels as at that
      date (a statement for a position the system doesn't have is itself the error, and an empty set
      would hide it); or the listing has a split between the earliest covered parcel's acquisition
      and `tax_year_end_date` leaving covered parcels on different unit bases — a single per-unit
      `cost_base_adjustment` cannot correctly scale both sides of a split. Pre-existing modelling
      limit (hand entry has it too, with no error message); a guard, not a blocker — neither AMIT
      listing held today has a split
- [ ] Web UI: saving an AMMA statement offers generation as the next step, the same
      chain-after-save shape the income form's "Reinvested under DRP" tick uses. The confirm step
      previews the parcels and quantities it will create and shows Σ against the statement's
      `units_held`, so "are the current positions correct?" is checkable rather than assumed; a
      mismatch is shown prominently and the user can still proceed
- [ ] Web UI: a standing `ACTIONS` entry in `config.js` on the AMMA statement row runs generation
      later, or re-runs it with `replace` after correcting a missed trade (the common repair path —
      a missing parcel usually means a trade was entered after the statement); plus the `REPORTS`
      entry for the cross-check under Reports → Cross-checks & alerts beside the AMIT Cash
      Cross-Check, with its numeric columns classified in `util.js`'s `COLUMN_KINDS`
- [ ] Annual tax report picks the cross-check up: `reports::tax_report::Completeness` gains a
      fourth list beside `amma_missing`/`amit_cash_alerts`/`e4_alerts`, filtered to the report's
      year on the row's `tax_year` exactly as those two are, and `complete` becomes "all four
      empty". Read via the new report's own pool-based `db_*` function on its own snapshot, not
      folded into the report's main transaction — the same advisory-note reasoning the module
      header already documents for the other two (that header's "two existing cross-checks" becomes
      three). This is the answer to "verify before the annual tax statement is run": the
      completeness section is exactly that gate, and an AMIT adjustment gap distorts the disposal
      schedule's cost base, which is the report's central figure
- [ ] `taxreport.js`'s `completenessSection` renders the new alerts as a fourth bullet type; the
      existing ✓/⚠ badge and its "this report may understate income or the cost base until they are
      resolved" wording already cover them. Deliberately **not** a hard gate on generating the
      report — completeness stays non-blocking (`docs/API.md`: "never rejects the request"). A
      warning printed onto the archived PDF is a stronger safeguard than a refusal, since it travels
      with the document, and the report is often generated precisely to find out what is wrong
- [ ] Tests: per-check report tests (each flag fires; a correct set flags nothing; a split does not
      false-positive the coverage check; a mid-year disposal is not flagged); `db_upsert` duplicate
      rejection at DB and API level; generation reproduces the hand-entered HNDQ FY2024/FY2025 sets
      exactly (509+1302 = 1811, and the five parcels totalling 2620 with the 2025-07-16 DRP
      excluded — the empirical case the requirement is built on); each `422` refusal; the annual tax
      report's `completeness` flags an adjustment gap, drops `complete` to false, and clears once
      the adjustments are entered (mirroring the existing `amma_missing` tests at
      `tax_report.rs:1526`); the new delete route (if any) added to
      `entities::tests::deleting_a_missing_row_is_404_naming_what_was_missing`
- [ ] Docs: `docs/API.md` gains the new report and the generation endpoint (request/response shapes,
      each `422`, and the Response-codes section) and updates the Annual tax report `completeness`
      bullet (currently "true only when all three are empty"); README's Features list alongside the
      other cross-checks, and its Annual tax report bullet's completeness wording;
      `docs/SCHEMA.md` only if the UNIQUE index lands

