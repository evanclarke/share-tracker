# TODO

Items are only marked done when a passing test exists for them.

Completed and decided (out-of-scope / not-reproducible) sections are archived in the topical `DONE/*.md` files, indexed by [DONE.md](DONE.md), to keep this list focused on active work. When a section here is fully done, move it into the matching `DONE/*.md` file rather than leaving it — see CLAUDE.md.

The sections below come from the **2026-07-13 improvement review** (a whole-project pass for
operational and test-strategy gaps, as distinct from the 2026-07-12 programming/domain review whose
findings are all closed in DONE.md), except where a section's heading names another source
(e.g. REQUIREMENTS, SCENARIOS). Each section records one finding; sections land in DONE.md as they
are fixed or decided.

## A negative investment expense is accepted and *adds* to assessable income (SCENARIOS H-06, H-09)
(SCENARIOS.md section H verification pass, 2026-08-17. `entities::investment_expense::db_upsert` is
the only `db_upsert` in the tree with no write-time check at all — it has no error enum, returning
`sqlx::Error` — so every figure on the row is whatever was keyed.)
- [ ] H-06 — `PUT /investment_expenses/1` with `expense_type` `Other` and `amount` `-500` answers
  `204`. The tax summary then reports `deductions_other` `-500`, `deductions_total` `-495` (against
  a legitimate `+5` loan-interest row) and `net_assessable_investment_income` **`495`** on a year
  whose `gross_assessable_investment_income` is `0`: a negative deduction is arithmetically income,
  and it inflates the net line above the gross
- [ ] The sibling entity already refuses exactly this. `interest_income::UpsertError::NegativeAmount`
  rejects a negative `amount`/`tfn_withholding_tax`/`foreign_tax_paid` with `422` naming the field,
  "interest figures are the statement's own positive (or zero) amounts" (2026-07-12 review, where
  negatives "silently reduced the year's gross-interest line"). The expense entity is the one that
  was missed — the same class of defect, one line further down the same report
- [ ] `gross_amount` `-100` is accepted too, and `deductible_percentage` takes `150` and `-10` — a
  percentage outside 0–100 is not a percentage
- [ ] The fix is the sibling's, verbatim in shape: an `UpsertError` with `NegativeAmount(&'static str)`
  (plus a percentage-range variant), the `From<UpsertError> for ApiError` arm carrying the
  user-facing wording, and the new `422` causes in `docs/API.md`'s catalogue
- [ ] Tests: a negative `amount`/`gross_amount` and an out-of-range `deductible_percentage` are each
  refused `422` naming the field with nothing persisted, zero stays acceptable, and the tax summary's
  net line can no longer exceed its gross line from a deduction alone

## An investment expense's apportionment provenance is never checked against what is claimed (SCENARIOS H-06)
(SCENARIOS.md section H verification pass, 2026-08-17.)
- [ ] H-06 — the scenario is the ordinary one: a fee that is part income-producing and part private,
  where the user works out the deductible share. The row records all three figures — `gross_amount`,
  `deductible_percentage`, `amount` — and nothing relates them. `gross_amount` `100` with
  `deductible_percentage` `50` and `amount` `900` answers `204`; so does an `amount` nine times a
  `gross_amount` with no percentage at all
- [ ] Both fields are documented "optional provenance (informational only)", so this is a deliberate
  starting point, not an oversight — but the system has the opposite precedent for a supplied pair:
  `income.amount_per_security × securities_held` must equal the components to the cent or the write
  is refused `422` naming the computed product (G-23), and `trades.statement_total` reconciles the
  same way. A user who keys 50% and then the *gross* figure as the amount over-claims, and the two
  fields that record the mistake sit inertly beside it
- [ ] **Decided 2026-08-17: cross-check it, the `amount_per_security` way.** When both provenance
  fields are supplied, `gross × pct` cent-rounded must equal `amount` or the write is refused `422`
  naming the computed figure. (The alternatives put aside: a health-report warning like the
  `duplicate_*` lists, or documenting the pair as a note to self that nothing verifies.)
- [ ] Tests: whichever way it lands, an inconsistent triple is refused/flagged and a consistent one
  (including the exactly-100% and no-percentage cases) is accepted

## Nothing states that interest belongs to the year it is *credited* (SCENARIOS H-05)
(SCENARIOS.md section H verification pass, 2026-08-17.)
- [ ] H-05 — a term deposit credits $500 of interest on 30 June 2026; the funds are only reachable
  on 2 July. The ATO rule is the credit: "You must declare interest income in the year it is
  credited, received or applied or dealt with in any way on your behalf or as you direct … For term
  deposits this usually means you should declare interest in the year the investment matures"
  (*Investment income*, QC 72101, retrieved 2026-08-17)
- [ ] The calculation is right — `tax_summary::tests::db_interest_is_assessed_in_the_year_it_is_credited`
  now pins that a 30 June `date_paid` lands in FY2026 and 2 July in FY2027. The gap is that
  `date_paid` is the *only* date the row carries, and nothing tells the user which of the two dates
  it wants: the field is labelled "Date paid" in the UI with the hint "Sets the financial year the
  interest falls in", `docs/SCHEMA.md` says "Date paid/credited", and there is no `docs/ato/` mirror
  of the rule at all. Key the availability date and a whole year's interest moves
- [ ] The system already models this distinction where it decided to: a trust distribution carries
  `entitlement_date` beside `date_paid` precisely so a 30 June entitlement paid 15 July is assessed
  in the year just ended (G-19). Interest has one date because it needs one — but only if that date
  is named unambiguously
- [ ] **Decided 2026-08-17: state the convention**, the shape the conduit-foreign-income convention
  took (G-03) — relabel toward "date credited" in the UI hint, `docs/SCHEMA.md` and `docs/API.md`,
  plus a mirrored `docs/ato/investment-income-timing.md` (QC 72101) indexed in OVERVIEW. No second
  column: the availability date is not a tax fact, and recording it would invite keying it here
- [ ] Tests: `doc_checks` pins the stated convention against the mirrored ATO wording

## An expense covering more than one financial year has nowhere to be apportioned (SCENARIOS H-08)
(SCENARIOS.md section H verification pass, 2026-08-17.)
- [ ] H-08 — one `investment_expenses` row is one `date_incurred`, one financial year, deducted in
  full in that year (`tax_summary`'s deduction loop buckets by `tax_year_for(date_incurred)`). Two
  ordinary share-investor expenses do not work that way:
  - **Borrowing expenses** — loan establishment fees, legal expenses, stamp duty on the loan: "If
    your expenses total more than $100, apportion them over 5 years or the loan term, whichever is
    shorter. If your expenses are $100 or less, you can claim a deduction for the full amount in the
    year you incur them" (ATO, *Dividend income deductions*, QC 104069, retrieved 2026-08-17; s 25-25)
  - **Prepaid interest** — a prepayment whose eligible service period runs over 12 months, or ends
    after the last day of the next income year, is apportioned by days across the years it covers
    (ATO, *Deductions for prepaid expenses*, the Martin example: $1,250 over 397 days → $573 in the
    first year, $677 in the second). Inside the 12-month rule it *is* immediately deductible, which
    is the case the current model gets right by construction
- [ ] So a $2,000 loan establishment fee entered as one row claims 5× the first year's deduction, and
  nothing refuses it, flags it, or documents the alternative. `gross_amount`/`deductible_percentage`
  are no help: they describe the private-vs-income-producing split, not a split across time, so there
  is not even a provenance field saying "this row is one year of five"
- [ ] Nothing in `docs/API.md`'s Known limitations, the entity's UI description, or
  `docs/ato/investment-income-deductions.md` (which lists borrowing costs as claimable without saying
  over what period) mentions time apportionment
- [ ] **Decided 2026-08-17: document the workaround** — one row per financial year carrying that
  year's apportioned share, stated as a Known limitation naming both ATO rules (QC 104069 for the
  5-years-or-loan-term borrowing expenses, the prepaid-expenses guide for the 12-month rule and the
  day-count formula), a UI hint on the entity, and a mirrored `docs/ato/` doc indexed in OVERVIEW.
  Not modelled: a `service_period_start`/`service_period_end` pair the tax summary apportions by
  days is the honest version but a real feature — new columns, the day-count split, the annual tax
  report's rows, and the "which year is this row in" question every report answers with one date
- [ ] Tests: whichever way it lands, a multi-year expense reaches the right per-year deduction (or is
  refused/documented), and `doc_checks` pins the stated rule

## Duplicate interest and expense rows are silently double-counted (SCENARIOS H-01, H-06)
(SCENARIOS.md section H verification pass, 2026-08-17, standing probe 6 — the `interest_income` /
`investment_expenses` counterpart of the closed E-03 `duplicate_actions`, F-06
`duplicate_amma_statements` and G-24 `duplicate_income` findings.)
- [ ] Two `interest_income` rows with the same `date_paid`, `amount` `250` and `source`
  "ANZ savings" report `interest_income` `500`; two identical `investment_expenses` rows report
  `deductions_advice_fee` `200`. `GET /reports/health` says nothing — its duplicate checks cover
  corporate actions, AMMA statements and income rows only
- [ ] Same cause as the other three (a re-submitted form, a statement keyed twice), and the same
  shape of fix: a **warning, not a constraint** — two interest credits of the same amount on one day
  from different accounts are legitimate, which is why `source` (interest) and
  `expense_type` + `listing_id`/`holding_account_id` + `description` (expenses) belong in the key
  alongside the date and the amount
- [ ] Group in Rust, not SQL: the amounts are TEXT decimals SQL would compare as strings — the
  `duplicate_income` pass (G-24) already had to do this, so follow it exactly, banner included
- [ ] Tests: a duplicated pair of each kind is reported with its ids, rows differing in any key field
  are not, and the web banner names both new lists

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
