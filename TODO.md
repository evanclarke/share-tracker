# TODO

Items are only marked done when a passing test exists for them.

Completed and decided (out-of-scope / not-reproducible) sections are archived in the topical `DONE/*.md` files, indexed by [DONE.md](DONE.md), to keep this list focused on active work. When a section here is fully done, move it into the matching `DONE/*.md` file rather than leaving it — see CLAUDE.md.

The sections below come from the **2026-07-13 improvement review** (a whole-project pass for
operational and test-strategy gaps, as distinct from the 2026-07-12 programming/domain review whose
findings are all closed in DONE.md), except where a section's heading names another source
(e.g. REQUIREMENTS, SCENARIOS). Each section records one finding; sections land in DONE.md as they
are fixed or decided.

## The LIC capital gain deduction field takes the already-halved figure, undocumented (SCENARIOS G-04)
(SCENARIOS.md section G verification pass, 2026-08-16.)
- [ ] G-04 — `lic_capital_gain_deduction` is passed straight through to the tax summary's D8 line.
  What a LIC's dividend statement prints, though, is the **LIC capital gain amount (the attributable
  part)**; an individual deducts **50%** of it (`docs/ato/lic-capital-gain-deduction.md`: Ben's $50
  attributable part is a $25 deduction). The user must halve it before entering
- [ ] Nothing says so: `docs/API.md`'s label table says only "The 50% LIC capital gain deduction is
  claimed at question D8", the UI field is a bare "LIC capital gain deduction", and there is no
  equivalent of the investment-expense field's explicit "enter the deductible figure
  (post-apportionment)" note. Entering the statement's figure doubles the deduction
- [ ] **Needs a decision**: document the entry convention (a `docs/API.md` Income paragraph + a form
  hint naming the 50%), or take the attributable part and compute the 50% — which is what CLAUDE.md's
  "implement a requirement fully" argues for, but would need a migration and a re-reading of every
  existing row
- [ ] Tests: the ATO example (Ben, $50 attributable part → $25 at D8) reproduced through whichever
  entry the decision picks
