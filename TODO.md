# TODO

Items are only marked done when a passing test exists for them.

Completed and decided (out-of-scope / not-reproducible) sections are archived in the topical `DONE/*.md` files, indexed by [DONE.md](DONE.md), to keep this list focused on active work. When a section here is fully done, move it into the matching `DONE/*.md` file rather than leaving it — see CLAUDE.md.

Sections here come from the **2026-07-13 improvement review** (a whole-project pass for
operational and test-strategy gaps, as distinct from the 2026-07-12 programming/domain review whose
findings are all closed in DONE.md), except where a section's heading names another source
(e.g. REQUIREMENTS, SCENARIOS). Each section records one finding; sections land in DONE.md as they
are fixed or decided.

**SCENARIOS.md sections A–I are driven and every finding they raised is closed** in the `DONE/*.md`
archive. **Section J. Employee share schemes** was driven 2026-08-18; the eight findings below are
its open work. When they are closed, the next work comes from driving **SCENARIOS.md section
K. Inherited parcels** the same way — walk its scenarios against the running system, and record each
gap here as its own `## ` section.

## The ESS vest Buy's FX rate is a hard-coded 1, so a foreign-currency vest can cost at parity (SCENARIOS J-08, J-12)
(SCENARIOS.md section J verification pass, 2026-08-18. `entities::ess_vest::db_vest` INSERTs the
cost-base-reset Buy with `fx_rate` literal `'1'`. On the trade that column is **not** a constant —
`infra::fx::pick_rate` treats it as `FxOverride::Fallback`, the rate used *when no ATO rate exists
for the month*. So the placeholder becomes a real answer exactly when the RBA rate is missing, and
the answer is 1 AUD per USD.)
- [ ] J-12 — reproduced: a USD listing, statement `taxing_point_date 2024-09-01`, 100 shares at
  US$150, no `rba_fx_rates` row for `USD 2024-09`. `POST /ess_statements/1/vest` → `201` with
  `currency USD, fx_rate 1`, and `POST /portfolio/overview` answers `total_cost_base 15000` — a
  **US$15,000 parcel costed at A$15,000**. Importing the month's rate (0.65) moves it to
  A$23,076.92, so the figure was ~35% understated with nothing marked provisional
- [ ] J-12 — the two sides disagree about the same missing month: `GET /portfolio/tax-summary`
  **500s** (`FxError::MissingRate`, documented in `docs/API.md` as "no rate ⇒ fails loudly with
  `500`") while the price-free CGT reports keep answering off the parity cost base. A user in this
  state sees the income report break and the capital-gains reports look fine
- [ ] J-08 — an ICE-style US RSU release has nowhere to put the release-date spot rate on the CGT
  side at all. The statement-AUD overrides (`aud_deferral_discount` &c.) cover only the **income**
  labels; every other parcel-creating operation takes a rate — `inheritance.fx_rate` (its own
  column), `rights_exercise`'s `fx_rate` body field, `drp_reinvestment`'s `fx_rate` body field, and
  `domain::rollover` carries the consumed parcel's forward. The ESS vest is the only one that
  invents one
- [ ] **Decide the model** (an `AskUserQuestion` for Evan, not a silent call). (a) **Give the
  statement an `fx_rate` column** the vest binds (default 1, refused ≤ 0, and — like
  `trades.spot_fx_rate` — only accepted on a non-AUD statement), so the taxpayer states the rate
  they used, matching `inheritance`. (b) **Bind `NULL`/no fallback** so a missing month fails loudly
  on the CGT side too, the way the income side already does — smallest change, but it leaves a
  correct-rate month with no way to record the spot rate the employer used. (c) **Bind
  `spot_fx_rate`** (the existing column, which *outranks* the ATO monthly rate) from a new statement
  field — the honest mapping for a release-date rate, but it changes the reported cost base for
  every month, not only missing ones. (a)+(b) together look right: a stated rate when the user has
  one, a loud failure when neither exists
- [ ] Tests: a USD vest with the month's rate missing does not answer a parity cost base; with a
  stated rate it converts at it; an AUD statement rejects the rate field
- [ ] Docs sync: `docs/SCHEMA.md` (`ess_statements`, `trades.fx_rate` on a vest Buy), `docs/API.md`
  (ESS statements + the 422 catalogue), README's ESS feature line

## An ESS statement in a currency other than its listing's is vested without conversion (SCENARIOS J-08, J-12)
(SCENARIOS.md section J verification pass, 2026-08-18. The I-06/I-08 pattern on the ESS side:
`ess_statement::db_upsert` never compares `currency` with the listing's. `market_value_per_share` is
the market value of *that listed share*, so a statement whose currency is not the listing's is
either a data-entry slip or two currencies in one row — and the vest copies the statement's currency
onto the parcel regardless.)
- [ ] J-08 — reproduced: an **AUD** ASX listing, statement `currency USD`, 100 shares at 150 →
  `204`, vest `201` with a **USD** parcel on an AUD-priced security. With `USD 2024-09` imported at
  0.65 the overview reports `total_cost_base 23076.92` for what the listing says is a A$15,000
  holding, and a later closing price (AUD, from the exchange) values a USD-costed parcel
- [ ] Precedent: the DRP side already refuses this (`450b887`, "reinvesting … a distribution
  recorded in a currency other than its listing's (the cash and the per-unit price are one
  division, so they must be the same money)"). The same argument holds here: the per-share market
  value and the listed price are the same money
- [ ] Fix: refuse at write time in `db_upsert` (`422` naming both currencies), the way the income
  reinvest path does — no model decision needed unless Evan wants the check on the *vest* instead
  (a statement can be entered before the listing exists in the right currency; the vest is the
  first point the currency reaches a parcel)
- [ ] Tests: a statement whose currency differs from its listing's is refused `422`; the matching
  case is unaffected; an AUD listing with an AUD statement still vests
- [ ] Docs sync: `docs/API.md` ESS statements + the 422 catalogue

## The ESS vest bypasses the trade write-time checks, and creates a Buy `PUT /trades` refuses (SCENARIOS J-03, J-13)
(SCENARIOS.md section J verification pass, 2026-08-18. `db_vest` writes its Buy with a raw
`INSERT INTO trades`, not through `trade::db_upsert`, so `checks::check_amounts` never runs. Most of
that check is satisfied by construction — the vest enforces positive quantity and price itself, sets
`brokerage_currency = currency`, `fx_rate = 1`, and `settlement_date = date` — with exactly one
exception: `AmountsError::PreCgtDate`.)
- [ ] J-13 — reproduced: a statement with `taxing_point_date 1985-01-01` is accepted (`204`) and
  vests (`201`) a Buy dated 1985-01-01. `PUT /trades/1` with that date answers `422`: "the trade is
  dated before 20 September 1985 — a pre-CGT holding is outside CGT and not modelled, so recording
  it would wrongly compute a capital gain or loss". The vest creates precisely the row the trade
  entity refuses, and the tax summary grows a `tax_year 1985` row
- [ ] Nothing about ESS can genuinely predate 20 September 1985 (Division 83A dates from 2009, its
  predecessor from 1995), so this is a typo guard rather than a live case — but it is the one place
  a parcel can enter the system below the CGT floor, and A-series work has consistently closed those
- [ ] Fix: reject a pre-CGT `taxing_point_date` in `ess_statement::db_upsert` (the earlier, better
  place: the statement is what the user typed), and state in `ess_vest`'s module doc which trade
  checks the vest satisfies by construction so the next reader can see the list is deliberate
- [ ] Tests: a pre-CGT taxing point is refused `422`; 1985-09-20 itself is accepted
- [ ] Docs sync: `docs/API.md` ESS statements + the 422 catalogue

## An ESS statement has no write-time checks on what it may say (SCENARIOS J-01, J-09, J-11)
(SCENARIOS.md section J verification pass, 2026-08-18. Section H's `investment_expenses` finding,
again: apart from the statement-AUD-override rule, `ess_statement::db_upsert` validates **nothing**
about its amounts. Every discount label, the foreign-source memo, the TFN withheld, the quantity and
the market value are taken as typed and reach the tax summary and the printed annual document
unchallenged.)
- [ ] J-09 — reproduced: `deferral_discount -1000` with `tfn_withholding -50` → `204`. The tax
  summary reports `tfn_withholding_tax: "-50"` — negative withholding is a refund from nowhere,
  and the negative discount silently nets against the other statements' discounts in the same year
  (four statements totalling A$17,000 of positive labels reported `ess_discount_assessable 16000`)
- [ ] J-01 — reproduced: `quantity -100`, `market_value_per_share -10` → `204` (the vest then
  refuses, `NothingToVest`, so the nonsense row simply sits there claiming income)
- [ ] J-01 — reproduced: 100 shares at $10 (A$1,000 of market value) with `deferral_discount 15000`
  → `204`. The discount is *by definition* market value less what the employee paid
  (`docs/ato/employee-share-schemes.md`), so a discount above the vested shares' market value
  implies a negative payment. The obvious cause is a transposed column or a foreign-currency figure
  against an AUD market value — the check must only apply when both `quantity` and
  `market_value_per_share` are positive, since an income-only statement (no vest recorded) leaves
  them zero and is legitimate
- [ ] J-11 — reproduced: `foreign_source_discount 5000` against `deferral_discount 1000` → `204`,
  and the tax summary reports `ess_foreign_source_discount 5000` inside a `ess_discount_assessable`
  of 1000. Label A is a **memo subset** of D+E+F+G (`docs/API.md`: "a memo already within
  `ess_discount_assessable`"), so a memo larger than what it is a memo of is a contradiction — the
  same shape as the CFI-within-unfranked check the income entity already enforces (`0a4e198`)
- [ ] Fix (the H-section pattern, `db81aab`): refuse at write time, `422` per cause — negative
  discount label / TFN / quantity / market value; label A above D+E+F+G; and total discount above
  `quantity × market_value_per_share` when both are positive
- [ ] Tests: one rejection test per cause, plus the income-only (zero quantity) statement still
  accepted and the exact-equality boundary (discount == market value, an RSU with nil consideration)
  accepted
- [ ] Docs sync: `docs/API.md` ESS statements + the 422 catalogue, and the field hints in
  `src/web/config.js`

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

## A duplicated ESS statement is caught by nothing (SCENARIOS J-11)
(SCENARIOS.md section J verification pass, 2026-08-18. `reports::health` warns on duplicate
corporate actions (E-03), AMMA statements (F), income (G-24), interest and expenses (H) —
`ess_statements` is the one income-bearing fact table with no such check.)
- [ ] J-11 — reproduced: the same statement entered twice (same listing, account, taxing point,
  quantity, market value and discount) is accepted, vests **two** parcels, and doubles both the
  Item 12 discount (`ess_discount_assessable 2000` for a $1,000 grant) and the holding
  (`quantity 200`). The health report answers with every list empty
- [ ] The 30-day rule makes this the *expected* accident rather than a hypothetical: the employer
  issues an **amended** statement for the same vest (`docs/ato/ess-30-day-rule.md` — an amended 2019
  statement and a new 2020 one for one grant), and a user who enters both has exactly this shape
- [ ] J-11 — the legitimate case must stay silent: two vests on the same date from different grants
  are ordinary. The G-24 key (identical amounts as part of the key, grouped in Rust because the
  amounts are TEXT decimals SQL would compare as strings) already handles that — differing
  quantities or discounts are not duplicates
- [ ] Fix: `duplicate_ess_statements` in `reports::health` + the UI banner, keyed on listing,
  holding account, `taxing_point_date` *and* identical quantity / market value / discount labels
- [ ] Tests: the doubled statement is reported with both ids; two same-date statements from
  different grants (different quantity or discount) are not
- [ ] Docs sync: `docs/API.md` health report, README

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
