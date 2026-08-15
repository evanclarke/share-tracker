# TODO

Items are only marked done when a passing test exists for them.

Completed and decided (out-of-scope / not-reproducible) sections are archived in the topical `DONE/*.md` files, indexed by [DONE.md](DONE.md), to keep this list focused on active work. When a section here is fully done, move it into the matching `DONE/*.md` file rather than leaving it — see CLAUDE.md.

The sections below come from the **2026-07-13 improvement review** (a whole-project pass for
operational and test-strategy gaps, as distinct from the 2026-07-12 programming/domain review whose
findings are all closed in DONE.md), except where a section's heading names another source
(e.g. REQUIREMENTS, SCENARIOS). Each section records one finding; sections land in DONE.md as they
are fixed or decided.

## An AMIT adjustment covering part of a parcel is diluted across the whole parcel (SCENARIOS D-13)
(SCENARIOS.md section D verification pass, 2026-08-15. `amit_adjustment::db_cost_base_reductions`
computes each row's reduction as **covered quantity × the statement's per-unit figure**
(`reduction_for`, `src/entities/amit_adjustment.rs:190`) — the row's own arithmetic says the amount
belongs to those units — but `domain::cost_base::adjusted_cost_base` then subtracts it from the
*whole parcel's* initial cost and pro-rates the remainder over every as-acquired unit:
`(initial − amit) × units / parcel.quantity` (`src/domain/cost_base.rs:211`). While the row covers
the whole parcel the two agree exactly. They diverge the moment it covers less — which is not an
exotic hand entry: `amit_adjustment_generation` writes `quantity = parcel.remaining_as_of`, the
units still open at the statement's year end, so **every parcel partly sold during the year**
generates one.)
- [ ] D-13 — reduction meant for the units still held is spread onto units already sold.
  Reproduced: Buy 2022-01-10 ×100 @ $10 (cost base 1000), Sell 2024-03-01 ×40, AMMA year ended
  2024-06-30 with `units_held: 60` and `cost_base_adjustment: 0.50`, one adjustment row covering 60
  units (reduction 30.00 — what generation itself would write). Realised gains report the 40 sold
  in March at cost base **388.00** (1000 − 30 pro-rated: (970 × 40/100)), and open parcels report
  the remaining 60 at `remaining_cost_base` **582.00** with `amit_cost_base_reduction: 30.00` —
  where 60 units each reduced by the stated $0.50 is 600 − 30 = **570.00**. 12.00 of reduction has
  moved from the units the statement covers to units it does not, understating the March sale's
  cost base and overstating what the open parcel carries into its own future disposal. The total is
  preserved, so the lifetime gain is unchanged — only its split across units and years is wrong
- [ ] The AMIT adjustment cross-check does not see it (`units_adjusted: 60` equals the statement's
  `units_held: 60` — the set reconciles; it is the *application* of the reduction that doesn't), so
  nothing surfaces the figure
- [ ] Contrast with the other cost-base reduction: a return of capital is applied **per unit** and
  bounded by `up_to` (`RocEvent::per_unit_for`, `src/entities/corporate_action/adjustments.rs:60`),
  so units sold before the payment are untouched and units held take the full per-unit amount. The
  two adjustment types answer the same question — which units does this reduction reach — in
  different ways, and only one of them matches the amount the row was computed from
- [ ] Decide the model, which is the part that needs a call rather than code: a per-unit reduction
  applied to the covered units only (matching ROC, and matching `reduction_for`'s own multiplication)
  needs a rule for *which* units of a parcel a partial row covers — the units open at the year end
  is what generation means, but an entry covering the whole parcel after a mid-year disposal (the
  fund attributing to units held during the year, correct under s 104-107B: the adjustment is made
  "just before the end of the income year, **or just before the time of a relevant CGT event**",
  LCR 2015/11 para 13) must keep reaching the sold units, as it does today and as
  `reports::realised_gains::tests::db_amit_statement_for_the_sale_year_adjusts_the_parcel_already_sold`
  now pins
- [ ] Tests: the partly-sold case above, asserting the sold allocation and the open remainder each
  carry the stated per-unit reduction and no more; plus the existing whole-parcel cases unchanged
- [ ] Docs sync: `docs/API.md` AMIT adjustments (what `quantity` means for the units it does *not*
  cover) and, if the pooling stays, a Known-limitations entry saying so

## A return of capital received on units already sold is not recorded anywhere (SCENARIOS D-14)
(SCENARIOS.md section D verification pass, 2026-08-15. Selling between a return of capital's record
date and its payment date leaves the seller entitled to the payment — that is what the record date
fixes — but the tool reduces nothing and records nothing, and correctly so as far as CGT event G1
goes: the units were not owned when the payment was made. The gap is what happens *instead*. The
ATO's own class rulings on returns of capital put it as CGT event **C2**, happening on the payment
date to the *right to receive* the payment, with a nil cost base for that right where the share's
cost base was fully applied in working out the gain or loss on the disposal — so the whole payment
is a capital gain in the payment's income year, not discountable (the right is held from the record
date). Nothing in the model can hold it.)
- [ ] D-14 — reproduced: Buy 2023-01-10 ×100 @ $10, Sell 2023-10-03 ×100 @ $15 (gain 500.00), then
  a `ReturnOfCapital` of $0.50/unit dated 2023-11-01 with `record_date: 2023-09-25`. The sale's
  realised figures are unchanged (cost base 1000, gain 500) and the net-capital-gain report shows no
  `cgt_event_g1_gain` — right for G1, but the $50.00 actually received is nowhere: no capital gain,
  no income row, no cross-check flag. (A payment dated *before* the sale does reduce the sold
  parcel's cost base, back-dated entry included — that half is correct and pinned by
  `reports::realised_gains::tests::db_return_of_capital_needs_both_entitlement_and_holding_at_payment`)
- [ ] `docs/API.md`'s `ReturnOfCapital` bullet states the two conditions precisely and says such
  parcels are left alone, which reads as *nothing to do* — the one place a user in this position
  would look. At minimum it should name the C2 event and the manual entry route; Known limitations
  has no entry for it either
- [ ] Decide: document only (a Known-limitations entry plus the `ReturnOfCapital` bullet, with the
  entry route — there is no path that records a gain on a right, so it would be a manual note), or
  model it (the payment's units × per-unit as a C2 capital gain in the payment year for parcels
  entitled at the record date but disposed of before payment, which the existing record-date and
  allocation data is enough to derive)
- [ ] Tests: `doc_checks` for the documentation route, or `reports::net_capital_gain` for the
  modelled one
