# TODO

Items are only marked done when a passing test exists for them.

Completed and decided (out-of-scope / not-reproducible) sections are archived in the topical `DONE/*.md` files, indexed by [DONE.md](DONE.md), to keep this list focused on active work. When a section here is fully done, move it into the matching `DONE/*.md` file rather than leaving it — see CLAUDE.md.

Sections here come from the **2026-07-13 improvement review** (a whole-project pass for
operational and test-strategy gaps, as distinct from the 2026-07-12 programming/domain review whose
findings are all closed in DONE.md), except where a section's heading names another source
(e.g. REQUIREMENTS, SCENARIOS). Each section records one finding; sections land in DONE.md as they
are fixed or decided.

**SCENARIOS.md sections A–J are driven and every finding they raised is closed** in the `DONE/*.md`
archive. **Section K. Inherited parcels** was driven 2026-08-18; it raised six findings, and the
two below are the ones still open (the four closed so far — the parcel Buy bypassing the trade
write-time checks, a non-AUD inheritance costed at parity, a currency other than the listing's, and
the missing duplicate-inheritance health check — are archived in
[`DONE/reviews.md`](DONE/reviews.md)). When they are closed, the next work comes from driving **SCENARIOS.md section L. Crypto** the
same way — walk its scenarios against the running system, and record each gap here as its own `## `
section.

## LPR expenditure converts at the parcel's acquisition month, not the month it was incurred (SCENARIOS K-04)
(SCENARIOS.md section K verification pass, 2026-08-18. `db_upsert` folds the LPR expenditure into
the Buy's single `brokerage` figure, so `domain::cost_base` translates the whole parcel — first
element *and* LPR expenditure together — at one rate: the parcel's (possibly deemed) acquisition
month. Under `DeceasedCostBase` that month is the **deceased's acquisition**, while the LPR incurred
the expense after the death, by definition a later month and often a much later one.)
- [ ] Reproduced: a USD listing; deceased acquired 2015-05-05, died 2024-03-01; `cost_base` US$2,000
  and `lpr_expenditure` US$1,000 incurred 2024-06-01. Rates imported: `USD 2015-05 = 2`,
  `USD 2024-06 = 0.5`. `GET /portfolio/open-parcels` reports `original_cost_base 1500` (US$3,000 ÷ 2).
  Translating each element at its own month gives A$1,000 + A$2,000 = **A$3,000** — the LPR element
  is understated 4×, and it moves the reported cost base by 50%
- [ ] The existing Known limitation does not cover it. "Cost-base FX timing" (2026-07-13) is about
  the AMIT/return-of-capital **reductions** and argues the single rate "keeps each parcel's
  cost-base breakdown internally consistent"; it also says the simplification "only bites on a
  non-AUD holding receiving non-AUD AMIT/return-of-capital reductions, which in practice does not
  arise". An LPR expense on an inherited foreign parcel is an **addition**, is dated by the user on
  the row itself, and does arise. `inheritance.rs`'s module doc mentions the single-rate treatment
  ("LPR expenditure translates with the parcel; its own incurral date is provenance only") but ties
  it to indexation, and no user-facing surface says it at all
- [ ] The ATO position: s 960-50(6) translates each amount at its own transaction time
  (`docs/ato/forex-common-transactions.md`, QC 18322 — Lisa's cost base and proceeds each translate
  at their own date), and QC 66053 has the LPR expense "included on the date the LPR incurred it"
- [ ] **Decide the model** (an `AskUserQuestion` for Evan, not a silent call). (a) **Translate the
  LPR element at its own month** — correct per s 960-50(6), but it means the parcel's cost base is
  no longer one currency amount at one rate: either the Buy carries the LPR expenditure in a second
  column the pipeline converts separately, or the inheritance stores the LPR expenditure already in
  AUD. Breaks the "initial − reductions = adjusted holds in the native currency" property the
  pipeline currently guarantees. (b) **Give `lpr_expenditure` its own currency column** and require
  it in AUD (the realistic case: an Australian LPR bills an Australian estate in AUD, whatever the
  shares are denominated in), converting nothing — narrower than (a) and matches how the expense is
  actually incurred. (c) **Document it** as a Known limitation and say so in the field hint —
  cheapest, and consistent with the 2026-07-13 FX-timing cut, but it leaves a wrong figure the user
  has to fix by hand. (b) looks strongest: the mismatch is not really an FX-timing subtlety, it is
  that an AUD fee has nowhere to be recorded as AUD
- [ ] Tests: whichever model is chosen, a foreign inherited parcel with LPR expenditure reports the
  element at its own rate/currency, and the AUD case is unchanged
- [ ] Docs sync: `docs/SCHEMA.md` (`inheritances`), `docs/API.md` (Inheritances, Known limitations,
  the FX-conversion section), README's inherited-parcels feature line

## Nothing states what the deceased's cost-base figure must be net of (SCENARIOS K-02, K-09)
(SCENARIOS.md section K verification pass, 2026-08-18. The G-03/G-04 shape: the figure is entered by
hand, one number, and two ATO rules about what that number must already have had done to it are
recorded nowhere the user will see. `cost_base`'s form field is labelled "Cost base" and carries no
hint, where `lpr_expenditure`, `lpr_expenditure_date` and `deceased_acquisition_date` — the fields
with rules attached — each carry one, and the per-rule `typeDescs` prose describes *which* figure
to enter without saying what it must be net of.)
- [ ] **Indexation recalculated out.** QC 66053 (`docs/ato/inherited-assets-cost-base.md`): where
  the deceased died **on or after 21 September 1999**, indexation is unavailable to the beneficiary
  "and any indexation inside the deceased's cost base must be recalculated out". A deceased who
  acquired before 21 September 1999 may well have been carrying an indexed cost base, so the figure
  copied off the estate's records is the wrong one — silently overstating the cost base and
  understating every later gain. Nothing in the UI, `docs/API.md` or the README says this; the
  mirror says it and is not a user surface. The existing Known-limitations entry covers only the
  indexation *alternative* not being modelled, which is the opposite direction
- [ ] **Apportionment between beneficiaries.** K-09 verified as the documented boundary — one
  taxpayer, so the beneficiary records their own share and there is nowhere to represent the other
  beneficiaries — and entering a part share works cleanly (500 of the estate's 1,000 units at
  $10,000 of its $20,000 cost base gives the expected parcel, fractional quantities included). What
  is missing is that the *cost base* must be apportioned with the units: a user who takes half a
  holding and types the deceased's whole cost base doubles their cost base, and no check can see it
  (a 500-unit inheritance at a $20,000 cost base is a perfectly ordinary row). The Known-limitations
  entry says only that the estate/LPR side is out of scope
- [ ] Fix (documentation, unless Evan wants more): a hint on the `cost_base` field for each rule
  (`typeDescs` already carries per-rule prose to extend), a sentence in `docs/API.md`'s Inheritances
  section, and an extension of the inherited-parcels Known limitation. A `doc_checks.rs` test pins
  the text, per the "a doc-only requirement is done when a test asserts it" rule
- [ ] Tests: `doc_checks.rs` assertions on the API.md/README text, and the served-bundle assertion
  for the field hints
- [ ] Docs sync: `docs/API.md` (Inheritances + Known limitations), README's inherited-parcels
  feature line, `src/web/config.js`
