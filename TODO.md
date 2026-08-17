# TODO

Items are only marked done when a passing test exists for them.

Completed and decided (out-of-scope / not-reproducible) sections are archived in the topical `DONE/*.md` files, indexed by [DONE.md](DONE.md), to keep this list focused on active work. When a section here is fully done, move it into the matching `DONE/*.md` file rather than leaving it — see CLAUDE.md.

The sections below come from the **2026-07-13 improvement review** (a whole-project pass for
operational and test-strategy gaps, as distinct from the 2026-07-12 programming/domain review whose
findings are all closed in DONE.md), except where a section's heading names another source
(e.g. REQUIREMENTS, SCENARIOS). Each section records one finding; sections land in DONE.md as they
are fixed or decided.

## A franking credit is accepted with no dividend behind it (SCENARIOS G-25)
(SCENARIOS.md section G verification pass, 2026-08-16.)
- [ ] G-25 — `PUT /income/1` with `franking_credits` 300 and every other amount zero returns `204`,
  and the tax summary reports a $300 offset against $0 of dividend income
- [ ] The same write accepts a credit ten times the dividend ($700 franked, $7,000 credits), which
  is arithmetically impossible: a company can attach at most `franked_amount × 30/70` (a base-rate
  entity's 25% gives less). It is the transposed-column / wrong-line data-entry error, and it
  inflates a *refundable* offset
- [ ] Scope it to `trust_income = false` rows. A trust row's credit legitimately exceeds the ratio:
  the "franked distributions from trusts" component can be reduced by the trust's own deductions
  while the member still claims the full franking credit
  (`docs/ato/amma-statement-guidance-notes.md`, Part B item 13Q). AMIT rows already reject credits
  outright
- [ ] **Needs a decision**: a write-time `422` naming the ceiling (the shape of the per-share
  cross-check and the no-negative-amounts rule), or a health-report warning (the shape of
  `duplicate_actions`)
- [ ] Tests: a non-trust row with credits above `franked_amount × 30/70` is refused/flagged, a
  fully franked 30% row and a base-rate 25% row are both accepted, and a trust row above the ratio
  is left alone

## Duplicate income rows are silently double-counted (SCENARIOS G-24)
(SCENARIOS.md section G verification pass, 2026-08-16 — the `income` counterpart of the closed
E-03 `duplicate_actions` and F-06 `duplicate_amma_statements` findings.)
- [ ] G-24 — two `income` rows for the same listing, holding account and `date_paid`, with identical
  amounts, report twice the dividend income and twice the franking credits. `GET /reports/health`
  says nothing: its duplicate checks cover corporate actions and AMMA statements only
- [ ] The cause is the same as those two (a re-submitted form, a re-imported statement) and so is
  the shape of the fix: a **warning, not a constraint** — two dividends from one company on one day
  are legitimate in principle (an ordinary and a special dividend), so the pair must stay enterable
- [ ] Open question: the key. (listing, account, `date_paid`) alone flags the legitimate
  ordinary + special pair; adding "identical gross amounts" flags only what is almost certainly a
  double entry
- [ ] Tests: a duplicated pair is reported with its ids (as `duplicate_actions` is), rows differing
  in listing/account/date/amount are not, and the web banner names it

## The related-payments rule and the 30%-at-risk test are not modelled and nowhere documented (SCENARIOS G-14)
(SCENARIOS.md section G verification pass, 2026-08-16.)
- [ ] G-14 — being a "qualified person" needs more than the 45/90-day count: days on which 30% or
  less of the ordinary financial risk of loss and opportunity for gain is retained do not count
  (hedges, options, futures), and the **related payments rule** applies separately — the
  small-shareholder exemption itself only exempts a holder "entitled to franking credits for all
  shares that satisfy the related payments rule"
  (`docs/ato/you-and-your-shares-dividends.md`)
- [ ] Neither is modelled (there is nowhere to record a hedge or a related payment) and neither is
  mentioned in `docs/API.md`'s Known limitations, the franking at-risk section, or the tax summary's
  `franking_credits` field — while that section states "an empty report means every attached credit
  is claimable", which claims more certainty than the recorded data can support
- [ ] Documentation-only, like the C-09 rollover scope cut: state the two unmodelled tests, and
  qualify the empty-report sentence with them. Note that G-11's fix has since made that sentence
  *true for what the report does test* (a dividend the walk cannot anchor is now listed as
  `untested_no_ex_date`), so the qualification to add is about the tests that are not modelled at
  all, not about the walk's coverage
- [ ] Tests: `doc_checks` pins the Known-limitations entry and the reworded report section

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
