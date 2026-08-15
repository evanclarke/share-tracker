# TODO

Items are only marked done when a passing test exists for them.

Completed and decided (out-of-scope / not-reproducible) sections are archived in the topical `DONE/*.md` files, indexed by [DONE.md](DONE.md), to keep this list focused on active work. When a section here is fully done, move it into the matching `DONE/*.md` file rather than leaving it — see CLAUDE.md.

The sections below come from the **2026-07-13 improvement review** (a whole-project pass for
operational and test-strategy gaps, as distinct from the 2026-07-12 programming/domain review whose
findings are all closed in DONE.md), except where a section's heading names another source
(e.g. REQUIREMENTS). Each section records one finding; sections land in DONE.md as they are fixed
or decided.

## A closed financial year can be restated with nothing marking it (SCENARIOS A-15, A-21, A-25, A-35)
(SCENARIOS.md section A verification pass, 2026-08-14. Every tax report is computed live from the
current facts, so editing a prior year's inputs silently changes figures that may already have been
lodged. Report snapshots do not cover this — they snapshot the three price-dependent reports only,
never the tax summary, net capital gain, or annual tax report. `row_history` records the change, so
the restatement is *auditable* after the fact, but nothing *surfaces* it.)
- [ ] Reproduced four ways, all `204`/`200` with no flag: changing a lodged year's Buy price
  (FY2023 net capital gain $500 → $1,100, A-15); deleting a `ReturnOfCapital` after its G1 gain was
  reported (A-21 — that *delete* is now refused `422`, see DONE/reviews.md; editing the payment
  amount in place restates the same year, so the finding stands); deleting the `cgt_settings`
  opening carried-forward loss after later years
  consumed it (FY2024 net gain $500 → $1,000, A-25); deleting the only disposal of a loss year that
  a later year's carry-forward drew on (FY2024 net gain $750 → $1,500, A-35). The annual tax report
  keeps reporting `completeness.complete: true` throughout
- [ ] Decide the scope: this may be honest "not modelled" — there is no lodged/closed-year concept
  in the data model, and adding one is a real feature (a lodgement marker per FY, plus a
  "changed since lodgement" flag driven off `row_history` timestamps). If it stays unmodelled it
  needs a **Known limitations** entry saying so plainly, since a user reasonably assumes a prior
  year's numbers are settled. Either way this is a documentation-or-feature decision, not a bug
- [ ] Related, low severity (A-40): `DELETE /exchange_holidays/:mic/:date` has no guard and no flag,
  and a trade re-saved afterwards without an explicit `settlement_date` silently recomputes against
  the changed calendar (reproduced: an ASX trade settling 2024-04-02 recomputed to 2024-03-29 — Good
  Friday itself — once that holiday was deleted). Stored `settlement_date` values are untouched, and
  no CGT figure reads the column (only the settlement-coverage report and the annual tax report's
  display do), so the exposure is a record field, not a tax figure. Worth one line wherever the
  restatement decision above lands
