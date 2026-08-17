# TODO

Items are only marked done when a passing test exists for them.

Completed and decided (out-of-scope / not-reproducible) sections are archived in the topical `DONE/*.md` files, indexed by [DONE.md](DONE.md), to keep this list focused on active work. When a section here is fully done, move it into the matching `DONE/*.md` file rather than leaving it — see CLAUDE.md.

The sections below come from the **2026-07-13 improvement review** (a whole-project pass for
operational and test-strategy gaps, as distinct from the 2026-07-12 programming/domain review whose
findings are all closed in DONE.md), except where a section's heading names another source
(e.g. REQUIREMENTS, SCENARIOS). Each section records one finding; sections land in DONE.md as they
are fixed or decided.

## A deduction's listing attribution never reaches the annual tax report (SCENARIOS H-07)
(SCENARIOS.md section H verification pass, 2026-08-17.)
- [ ] H-07 — the correctness side is fine and now pinned
  (`investment_expense::tests::api_expense_survives_a_rename_and_blocks_deleting_its_listing`): a
  rename keeps `listing_id`, and deleting the listing is refused `422` naming the investment expenses
  that draw on it. What is missing is the print surface. `tax_report::DeductionRow` carries
  `listing_id` and no `ticker` — the only listing-bearing row in the report that doesn't — and
  `taxreport.js` renders the deductions table as `date_incurred, expense_type, amount_aud,
  description`, so the attribution is dropped entirely from the document that gets archived as the
  year's PDF
- [ ] It matters most in exactly the scenario that raised it: after a rename, demerger, or worthless
  declaration, a bare `listing_id` in the JSON is the only trace of which holding the fee was for,
  and the printed page has not even that
- [ ] Fix: carry the ticker on `DeductionRow` the way `DividendIncomeRow`/`ForeignIncomeRow` do (the
  report already loads the listing map), and add the column to the printed table
- [ ] Tests: a listing-attributed expense prints its ticker in the annual tax report, a
  portfolio-wide one prints blank, and the served bundle carries the column
