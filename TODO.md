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
seven are closed (all 2026-08-18, see [`DONE/reviews.md`](DONE/reviews.md)) and the one below is its
remaining open work. Evan decided its model on 2026-08-18: J-10 takes the **`income_type` enum**
(option (b)), not the documentation-only cut. When it is closed, the next work comes from driving
**SCENARIOS.md section K. Inherited parcels** the same way — walk its scenarios against the running system, and record each
gap here as its own `## ` section.

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
