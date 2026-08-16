# TODO

Items are only marked done when a passing test exists for them.

Completed and decided (out-of-scope / not-reproducible) sections are archived in the topical `DONE/*.md` files, indexed by [DONE.md](DONE.md), to keep this list focused on active work. When a section here is fully done, move it into the matching `DONE/*.md` file rather than leaving it — see CLAUDE.md.

The sections below come from the **2026-07-13 improvement review** (a whole-project pass for
operational and test-strategy gaps, as distinct from the 2026-07-12 programming/domain review whose
findings are all closed in DONE.md), except where a section's heading names another source
(e.g. REQUIREMENTS, SCENARIOS). Each section records one finding; sections land in DONE.md as they
are fixed or decided.

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
