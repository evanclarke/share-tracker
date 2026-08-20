# TODO

Items are only marked done when a passing test exists for them.

Completed and decided (out-of-scope / not-reproducible) sections are archived in the topical `DONE/*.md` files, indexed by [DONE.md](DONE.md), to keep this list focused on active work. When a section here is fully done, move it into the matching `DONE/*.md` file rather than leaving it — see CLAUDE.md.

Sections here come from the **2026-07-13 improvement review** (a whole-project pass for
operational and test-strategy gaps, as distinct from the 2026-07-12 programming/domain review whose
findings are all closed in DONE.md), except where a section's heading names another source
(e.g. REQUIREMENTS, SCENARIOS). Each section records one finding; sections land in DONE.md as they
are fixed or decided.

**SCENARIOS.md sections A–O are driven and every finding they raised is closed** in the `DONE/*.md`
archive. Section **P. Tax summary, annual tax report, exports** was driven 2026-08-20: seven of its
twelve scenarios came back correct outright — P-01/P-07 (a year carrying franked + unfranked + CFI +
LIC + TFN dividends, a trust distribution, a foreign-company dividend in USD, a full AMMA statement,
Australian and foreign interest, an ESS statement and an expense reconciled line for line across the
tax summary, the annual tax report's income tables and the CSV export), P-03 (30 June and 1 July
income landing in the right years), P-05 (an AMIT held only part of the year, and sold out before
year end, still asked for its AMMA statement; a year it was not held stays silent), P-06 (a demerger,
an off-market buy-back and a rights sale in one year: the buy-back's capital proceeds are market
value less the dividend with the dividend on its own income row, the rights sale is its own
disposal, and the demerger's closing Sell is correctly *not* a disposal), P-09 (a back-dated return
of capital after generation moves the printed gain and itemises itself as a cost-base adjustment),
P-10 (a 300-row disposal schedule returns all 300 rows, and the print renderer has no pager to
truncate them) and P-11 (tickers as at each taxable event's own date across a rename). The five
findings below are open.

---

## SCENARIOS P-02/P-03/P-04: the annual tax report's year picker omits years the report has content for

`GET /reports/tax-report/years` is the *only* way to reach the annual tax report from the web UI —
`taxreport.js`'s `viewTaxReport` renders the response as a `<select>` and posts
`Number(yearSelect.value)`, so a year absent from that list cannot be generated at all. The list is
`db_tax_report_years`, a `UNION` of six fact tables by their most obvious date column
(`trades.date`, `income.date_paid`, `interest_income.date_paid`, `amma_statements.tax_year_end_date`,
`ess_statements.taxing_point_date`, `investment_expenses.date_incurred`). Three kinds of year the
report *does* produce content for are missing from it — each reproduced against the running system:

- [x] **Trust income is bucketed by `date_paid`, not by its assessment date.** A trust distribution
  with a 30 June 2025 entitlement date paid 15 July 2025 (P-04, the exact case that scenario names)
  is assessed in **FY2025** — `Income::assessment_date` / `domain::tax_year::tax_year_for`, the rule
  the tax summary and the report's own `push_income_rows` both apply. `GET /portfolio/tax-summary`
  reports it under `tax_year: 2025`; the year list answers `[2026]`. FY2025 — the year the
  distribution belongs to — is unreachable.
- [x] **A CGT event that is not a trade puts no year on the list.** A return of capital above the
  cost base (CGT event G1) dated 15 September 2025 against a parcel bought in FY2023 gives FY2026 a
  `net_capital_gain` of $200 and a full `cgt_summary`; the year list answers `[2023]`. Likewise a
  rights sale as a year's only fact: a $25 FY2025 net capital gain, year list `[2022]`. `rights_sales`
  and `corporate_actions` are in neither the union nor anything that stands in for them, and the same
  applies to an AMIT E10 excess. `docs/API.md` describes the endpoint as "every Australian financial
  year with any recorded fact touching a tax figure", which these *are* — so the doc and the code
  disagree, not just the code and the need.
- [x] **A quiet year that carries a capital loss forward.** O's fix made
  `net_capital_gain::net_years` emit such a year, and `tax_report`'s own
  `a_quiet_year_still_reports_its_carried_forward_loss` pins that the document prints its label 18V
  figure — but the year list still doesn't offer it (FY2025 loss, `years: [2025]`, FY2026 carries
  $4,000). `docs/API.md` currently resolves this with "request a quiet year by `tax_year` directly",
  which is not a thing the UI can do: the picker is a closed `<select>`.

**Fix — Evan chose 2026-08-20: widen the year list** (over a free-typed year input beside the
picker, and over doing both). `db_tax_report_years` becomes the union of *everything the report can
report on*: income by **assessment date** (not `date_paid`), every year `net_capital_gain`'s own year
walk emits (which already covers realised disposals, rights sales, G1, E10, C2 and the quiet
carry-forward years, so it subsumes the second and third bullets in one read), plus the interest /
AMMA / ESS / expense dates already there. `docs/API.md`'s description of the endpoint and its
"request a quiet year by `tax_year` directly" note both need updating.

**Done 2026-08-20.** `db_tax_report_years` now reads on one transaction and unions: the trade /
interest / AMMA / ESS / expense dates as before; `income` rows decoded and bucketed by
`Income::assessment_date` (no SQL twin of that rule exists and none was added — the read decodes the
rows and asks the model, as `tax_summary` does); and `net_capital_gain::db_cgt_years`, a new
`pub(crate)` entry point running the *same* `gross_buckets`/`net_years` pipeline `db_net_capital_gain`
and `db_cgt_summary_year` run, on the caller's connection. Reusing that walk (rather than re-deriving
CGT-event years from the fact tables) is what makes the picker and `cgt_summary` unable to disagree.
The list is filtered to `MIN_TAX_YEAR..=MAX_TAX_YEAR`, so it can never offer a year `TaxYear::new`
would refuse `422`. **Cost trade-off, stated in the code comment:** the list went from one six-way
date `UNION` to roughly one `cgt_summary`'s worth of work — accepted deliberately, since the picker is
a closed `<select>` and the only way to reach the report, so a missing year is a report that cannot be
generated at all. Tests (all over HTTP through `ApiClient`):
`the_year_list_buckets_trust_income_by_its_assessment_date` (P-04),
`the_year_list_offers_a_g1_excess_year_with_no_trade_in_it` (P-03, which also pins that the two years
with nothing in them stay off the list), `the_year_list_offers_a_rights_sale_only_year` (P-03),
`the_year_list_offers_a_quiet_carry_forward_year` (P-02/O-03, which also posts every listed year and
requires `200`), and the widened `years_handler_lists_every_year_with_a_recorded_fact`. `docs/API.md`'s
endpoint description is rewritten and the "request a quiet year by `tax_year` directly" note is gone,
pinned by `doc_checks::tax_report_year_picker_scope_documented`.

## SCENARIOS P-01/P-07: a converted fund's pre-conversion income is totalled but has no rows behind it, and its franking credits are never tested

`listing::amit_in_tax_year` is the shared rule that a fund which converted to an AMIT part-way through
a holding (`listings.amit_from`, migration 0024, SCENARIOS F-23) was an ordinary trust before its
first AMIT income year — its earlier years' distributions are assessable exactly like any other
trust's. Five readers call it. **Two don't**, and both use a flat `WHERE NOT l.amit` instead:

- [x] `reports::tax_report::push_income_rows` (`src/reports/tax_report.rs:930`). Reproduced: a fund
  with `amit = true`, `amit_from = 2024-07-01` and one FY2023 trust distribution of $600 franked +
  $400 unfranked with $257.14 of credits. `GET /portfolio/tax-summary` reports FY2023
  `dividends_assessable: 1000`, `franking_credits: 257.14`, and the annual tax report's own
  `tax_summary` block echoes `dividends_assessable = 1000` — while its `trust_income` **and**
  `dividends` tables are both `[]`. The archived document states a $1,000 income total with nothing
  behind it, in a report whose stated purpose is "enough detail to hand-check every figure against
  the source contract notes and statements".
- [x] `reports::franking::db_franked_dividends` (`src/reports/franking.rs:334`). The same rows are
  invisible to the holding-period walk: `GET /reports/franking_at_risk` returns `[]` for that
  dividend, and its credits never join `attached_credits_by_year`, so they don't count toward the
  A$5,000 small-shareholder threshold either — meaning in a year near the threshold the *other*
  dividends can be wrongly exempted from the at-risk test as well.

**Fix.** Both call sites read the listing's `amit`/`amit_from` and filter through
`listing::amit_in_tax_year` against the row's own assessed tax year, exactly as
`tax_summary::db_tax_summary_on` already does. No model decision: the tax summary's per-year rule is
already the decided behaviour and these two surfaces simply drifted from it. Regression tests belong
beside the existing F-23 ones.

**Done 2026-08-20.** Both call sites now select `l.amit`/`l.amit_from` and filter through
`listing::amit_in_tax_year` against `tax_year_for(income.assessment_date())`, the same shape
`tax_summary::db_tax_summary_on` uses. A sweep for other flat AMIT filters found a **third** drift in
the same family: `tax_report::amma_missing`'s net-units walk read `WHERE listing_id IN (SELECT id
FROM listings WHERE amit)` and never re-filtered to the per-year AMIT set, so a converted fund held
beside a lifelong AMIT was flagged as missing a statement it could never have (the existing F-23 test
missed it because with one AMIT listing the empty-`tickers` early-out fires first). Fixed the same
way. The remaining `l.amit` filters — `amit_cash_cross_check` (an AMIT-side report; per-year checked
inside) and the two write-time checks (`income`, `corporate_action`) — are correct as they stand, so
`amit_in_tax_year` now has **seven** call sites. Tests:
`reports::tax_report::tests::a_converted_funds_pre_amit_income_prints_behind_its_tax_summary_line`
(the row prints *and* equals its tax-summary line; the AMIT year still excludes the cash row),
`reports::tax_report::tests::amma_missing_ignores_a_converted_fund_beside_a_lifelong_amit`,
`reports::franking_at_risk::tests::db_a_converted_funds_pre_amit_dividend_is_tested_and_counts_toward_the_threshold`
(the credits are walked *and* tip the year's other dividend out of the small-shareholder exemption),
and `reports::franking::tests::db_franked_dividends_follow_the_funds_conversion_year` (the AMIT
year's legacy row stays out of both the candidates and `attached_by_year`). `docs/API.md`'s
converted-fund paragraph, annual-tax-report `income` bullet and franking at-risk section state the
per-year rule, pinned by `doc_checks::dated_amit_status_documented`.

## SCENARIOS P-08: every investment-expense deduction is exported at `D7 / D8`, including the ones the ATO says don't go there

The tax summary's CSV maps all seven deduction columns (`deductions_loan_interest` …
`deductions_total`) to the label `D7 / D8`. The project's own mirrored ATO reference says that is
wrong for two common cases — `docs/ato/dividend-income-deductions.md`, **"Don't show at this
section"**:

> - expenses incurred earning **trust and partnership** distributions (go to Partnerships or Trusts)
> - expenses incurred earning **foreign-source dividends** (go to Other foreign income or Other deductions)

and `docs/ato/tax-return-labels-2026.md` closes with "Expenses of earning trust/partnership
distributions belong at 13X/13Y, not D7/D8", carrying the 13X/13Y row in its question-13 table.

- [x] An `investment_expenses` row already carries an optional `listing_id`, so the destination
  question is derivable: a fee on a trust or AMIT listing belongs at **13Y**, a fee on a
  foreign-currency / foreign-source-dividend holding at question **20**, everything else at D7/D8.
  Nothing anywhere says so — `docs/API.md` and `README.md` contain no occurrence of `13X` or `13Y` at
  all, so the wrong label is neither corrected nor disclosed.
- [x] This is Evan's live case, not a hypothetical: a management fee on **VDHG/HNDQ** (both AMITs)
  exports at D8, and a fee attributable to **ICE** (USD, foreign-source) does too.

The total deduction is unaffected, so nothing here is a wrong *figure* — it is a wrong destination on
the return, which myTax cross-checks against the income each question carries.

**Fix — Evan chose 2026-08-20: split the lines by destination question** (over keeping one set of
lines and surfacing the routing, and over a Known-limitations entry). Each expense's destination is
derived from its listing — trust or AMIT ⇒ **13Y**, a foreign-source-dividend holding ⇒ question
**20**, everything else ⇒ **D7/D8** — and the tax summary carries separate columns per destination,
since the CSV carries one label per *column*, not per row. That reaches `TaxYearSummary`, `CSV_HEADER`
/ `CSV_ATO_LABELS`, the annual tax report (which reads the same label mapping), `docs/API.md`'s
column table, and `config.js`.

**Done 2026-08-20.** The ATO was re-checked first, and it splits the foreign case in two the
write-up didn't: question 20's worksheet nets an expense of earning foreign income into **20M**
(rows r − s) but *expressly excludes debt deductions*, which go to **D15** (label J) instead — while
question 13 Part C *does* take debt deductions at X/Y, so interest on money borrowed to buy units in
a trust follows the trust to 13Y rather than staying at D7. The instruction text for all three (and
D15's own page) is quoted in `docs/ato/tax-return-labels-2026.md`'s new *Where an investment-expense
deduction goes* section, indexed in `docs/ato/OVERVIEW.md`. `domain::deduction_destination` is the
one routing rule both readers call: a listing flagged `amit` (an AMIT and the ordinary trust it was
before its `amit_from` year both report at question 13 — read through `listing::amit_in_tax_year`,
not a flat `l.amit`) or carrying any `trust_income` row is a trust; else any `foreign_source_income`
row, or a non-AUD listing with no income recorded at all, is foreign; else D7/D8. A portfolio-wide
expense and an AUD listing with no income recorded are genuinely undecidable and take the D7/D8
default, stated in the module doc, `docs/API.md`, the report description and the printed footnote.
`TaxYearSummary` gains four destination lines (`deductions_trust_distributions` → 13Y,
`deductions_foreign_income` → 20M, `deductions_foreign_debt` → D15,
`deductions_dividend_and_interest` → D7/D8) beside the six per-*kind* lines, which are a different
cut of the same total and so lost their (wrong) `D7 / D8` label rather than being dropped;
`deductions_total` is the sum of either group, never both. The annual tax report prints each
deduction's `destination` + `ato_label`. Tests:
`tax_summary::tests::db_deductions_are_cut_by_the_question_each_is_claimed_at`,
`db_trust_loan_interest_reports_at_13y_not_d15`,
`db_a_converted_funds_pre_amit_year_deduction_still_reports_at_13y`,
`db_a_holding_with_no_income_recorded_routes_on_its_currency`,
`api_export_labels_each_deduction_destination_with_its_question`,
`tax_report::tests::deduction_rows_print_the_question_each_is_claimed_at`,
`deduction_destination::tests::*`, and `doc_checks::investment_expense_deduction_destinations_documented`.
No `docs/ato/` worked example became representable (the question 13/20 instructions carry no
numbers for this), so nothing was added to `ato_examples.rs`.

## SCENARIOS P-12: the parcel optimiser ranks strategies on the 50% discount without stating the taxpayer basis

`reports::TAXPAYER_BASIS` ("individual resident: 50% CGT discount; 50% LIC deduction") is the stated
single-taxpayer assumption every hard-wired rate rests on. It is reported on `TaxYearSummary`, on
`NetCapitalGainYear` (and the what-if's scenario rows), and in the annual tax report's `meta`.

- [x] `POST /portfolio/parcel-optimiser` does not carry it, and it is the surface where the
  assumption bites hardest: it halves each candidate parcel's gain (`g / Decimal::from(2)` where
  `discount_eligible`) and then **ranks four strategies against each other on the result** — so for a
  taxpayer the 50% rate doesn't apply to (a company, a super fund at 33⅓%, a non-resident), it does
  not merely report a wrong number, it recommends the wrong parcels. Verified: a 200 response with no
  `taxpayer_basis` anywhere in it.
- [x] `GET /portfolio/realised-gains` likewise carries `discount_eligible` / `discount_eligible_gain`
  per disposal with no statement of the basis. Lower stakes (it reports rather than advises) but the
  same gap.

**Fix.** Add the `taxpayer_basis` field to the optimiser's response (and realised gains'), sourced
from the same `reports::TAXPAYER_BASIS` constant, and surface it in the UI the way the other reports
do; `docs/API.md` gains the field on both endpoints.

## SCENARIOS P-02 (boundary): `POST /reports/tax-report` panics on an out-of-range `tax_year` instead of refusing it

`tax_report::period_for` builds the period with
`NaiveDate::from_ymd_opt(tax_year - 1, 7, 1).expect("valid period start")`. `TaxReportRequest`
validates nothing, so a year `chrono` cannot represent aborts the handler:

- [x] `POST /reports/tax-report {"tax_year": 300000}` **panics** at `src/reports/tax_report.rs:75`
  (`valid period start`) rather than answering `422`. A panicking handler bypasses `infra::http`'s
  one-error-type contract entirely — no classified status, no `tracing::error!` with the cause, the
  connection simply drops.
- [x] Below that, nonsense is accepted silently: `tax_year: 0` returns `200` with
  `period_start: "-0001-07-01"`, and `tax_year: -1` a period ending `-0001-06-30`.

**Fix.** Validate `tax_year` in the handler and answer `422` naming the accepted range — an
`ApiError::Unprocessable`, like every other rejected request field — with the range chosen so no
representable financial year a user could legitimately want is refused. `docs/API.md`'s response-codes
422 catalogue gains the new cause.
