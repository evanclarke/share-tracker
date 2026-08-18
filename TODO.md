# TODO

Items are only marked done when a passing test exists for them.

Completed and decided (out-of-scope / not-reproducible) sections are archived in the topical `DONE/*.md` files, indexed by [DONE.md](DONE.md), to keep this list focused on active work. When a section here is fully done, move it into the matching `DONE/*.md` file rather than leaving it — see CLAUDE.md.

Sections here come from the **2026-07-13 improvement review** (a whole-project pass for
operational and test-strategy gaps, as distinct from the 2026-07-12 programming/domain review whose
findings are all closed in DONE.md), except where a section's heading names another source
(e.g. REQUIREMENTS, SCENARIOS). Each section records one finding; sections land in DONE.md as they
are fixed or decided.

**SCENARIOS.md sections A–I are driven and every finding they raised is closed** in the `DONE/*.md`
archive. **Section J. Employee share schemes** was driven 2026-08-18; it raised eight findings, of which
five are closed (the two currency/FX ones, the two write-time-check ones, and the duplicate-statement
health check — all 2026-08-18, see [`DONE/reviews.md`](DONE/reviews.md)) and the three below are its
remaining open work. Each of those three carries a **Decide the model** item awaiting Evan — none can
be finished without that decision. When they are closed, the next work comes from driving
**SCENARIOS.md section K. Inherited parcels** the same way — walk its scenarios against the running system, and record each
gap here as its own `## ` section.

## Nothing on the product side mentions the ESS 30-day rule (SCENARIOS J-04)
(SCENARIOS.md section J verification pass, 2026-08-18. A disposal within 30 days after the deferred
taxing point **moves the taxing point to the disposal date**: the discount is re-measured at the
proceeds and the cost base resets to the same figure, so there is no separate capital gain, and the
discount can move into the next financial year — `docs/ato/ess-30-day-rule.md`, QC 23058 Example 11.
The mirror is indexed in `docs/ato/OVERVIEW.md`, but the words "30-day rule" appear nowhere in
`README.md`, `docs/API.md`, or the ESS screen, and no report flags the pattern.)
- [x] The corrected entry works and is now pinned: `ato_examples::ess_30_day_rule_example_11_wyatt_amended_statement`
  enters the *amended* statement (taxing point = the 20 July 2019 disposal, market value = the
  $3.795 per-share sale price), vests it, and sells the same day — FY2020 discount $1,518, capital
  gain $0, exactly the ATO's answer. `docs/ato/OVERVIEW.md` already claimed this test existed; it
  does now
- [ ] J-04 — the *natural* entry is wrong in two ways at once and nothing says so. Entering the
  employer's original statement (taxing point 23 June 2019, discount $1,400) and then the 20 July
  sale gives `ess_discount_assessable 1400` in **FY2019** and a **$118 capital gain** in FY2020 —
  where the ATO's answer is $1,518 of discount in FY2020 and no capital gain. Both figures are
  wrong, in different years, from an entry the system accepts without comment
- [ ] The trigger is mechanically detectable from data already held: a Sell allocating a parcel
  whose Buy carries `ess_statement_id`, dated within 30 days after that statement's
  `taxing_point_date`. `reports::wash_sales` is the precedent for an advisory, non-blocking
  date-pattern report, and `reports::health` for a banner
- [ ] **Decide the model.** (a) **Documentation only** — a Known-limitations entry plus a hint on
  the ESS screen's taxing-point field saying an amended statement supersedes the original (cheapest,
  and the G-14 precedent for a scope cut honestly stated). (b) **Plus an advisory alert** — a
  `ess_30_day_rule` list in `reports::health` (or its own cross-check report) naming each sale
  within the window and the statement it draws on, so the case is caught rather than remembered.
  (c) **Re-measure automatically** — rejected in advance: the system cannot know whether the
  employer issued an amended statement, and rewriting a user's stated discount would be a
  calculation the ATO puts on the employer
- [ ] Tests: whichever of (a)/(b) is chosen — a `doc_checks` assertion for the wording, and/or an
  alert test with a sale on day 30 and day 31 either side of the boundary
- [ ] Docs sync: `docs/API.md` Known limitations (+ the report, if (b)), README Features

## The $1,000 taxed-upfront reduction is always applied, with no way to record failing the income test (SCENARIOS J-02)
(SCENARIOS.md section J verification pass, 2026-08-18. The reduction is available only if *adjusted
taxable income* is ≤ A$180,000 — a taxpayer-level test outside this system's data
(`docs/ato/employee-share-schemes.md`). The tool applies `min(A$1,000, D)` unconditionally and
documents the test as the user's responsibility in `README.md`, `docs/API.md` (both the tax-summary
section and Known limitations) and the ESS screen description — thorough, and the applied amount is
surfaced as its own `ess_taxed_upfront_reduction` line so it can be added back by hand.)
- [ ] J-02 — the gap is that "add it back by hand" has no home in the system: there is no
  per-taxpayer or per-year flag, and the only way to make the summary report the right figure is to
  enter the discount at label **E** (taxed-upfront *not eligible*), which misstates 12D/12E to get
  12B right. An ineligible taxpayer's every stored figure and export stays $1,000 light
- [ ] J-02 — the printed archival document (`/reports/tax-report`, the PDF the accountant gets)
  prints `ess_taxed_upfront_reduction 1,000` as a bare line with an empty ATO label and no statement
  of the condition it assumes. `taxreport.js` already carries the precedent for exactly this: the
  CFI footnote (`cfiFootnote`) explains a figure the reader would otherwise misread
- [ ] **Decide the model.** (a) **A footnote only** — print the ≤A$180,000 condition under the ESS
  table whenever a reduction was applied (cheap, honest, matches the CFI precedent). (b) **Plus a
  `cgt_settings` flag** — the singleton settings entity already carries a taxpayer-level fact (the
  opening capital loss); an `ess_taxed_upfront_reduction_eligible` boolean (default true) would let
  the summary report the ineligible position and keep the exports right. (c) **Per-year** rather
  than singleton, since the income test is answered year by year — more faithful, and the only one
  that survives a year where the taxpayer crosses $180,000; costs a new dated settings table
- [ ] Tests: whichever is chosen — a `doc_checks`/bundle assertion for the footnote wording, and a
  summary test that an ineligible year reports the unreduced discount
- [ ] Docs sync: `docs/API.md` tax summary + Known limitations, README

## The documented dividend-equivalent workaround reports remuneration as a dividend (SCENARIOS J-10)
(SCENARIOS.md section J verification pass, 2026-08-18. A dividend equivalent paid on unvested RSUs
is **ordinary income as remuneration** under s 6-5 — "not a dividend in the employee's hands", not
part of the ESS discount, and carrying no franking (TD 2017/26,
`docs/ato/ess-dividend-equivalents.md`). `docs/API.md` Known limitations tells the user it is
"enterable manually as an [income](#income) row if the user wants it aggregated here".)
- [ ] J-10 — reproduced: that row (`unfranked_amount 250` against the employer's listing) reports as
  `dividends_assessable 250` — **item 11S, unfranked dividends** — counts in
  `gross_assessable_investment_income`, and prints in the annual document's **Dividend income**
  table with `franking_status "entitled"`. The one place the amount belongs (salary and wages,
  item 1/2) is not somewhere this system reports at all
- [ ] The workaround is not wrong so much as unlabelled: aggregating the cash here is fine, but
  nothing tells the reader the row will be **called a dividend** by every surface it reaches, and
  the printed document is the one that goes to an accountant
- [ ] **Decide the model.** (a) **Sharpen the documentation** — say plainly that an income row
  reports at 11S and that the amount must be moved to salary/wages in the return, or say don't enter
  it here at all (cheapest; keeps the data model unchanged). (b) **Give income rows a kind** — an
  `income_type` enum (dividend / other) whose non-dividend value reports on its own tax-summary line
  and prints in its own table; correct, but it touches an audited table, the tax summary, the export
  header and the printed document. (a) looks proportionate for a payment the system deliberately
  does not model
- [ ] Tests: a `doc_checks` assertion for the wording (the H/G precedent for documentation-only
  requirements)
- [ ] Docs sync: `docs/API.md` Known limitations (the RSU dividend-equivalents entry), README
