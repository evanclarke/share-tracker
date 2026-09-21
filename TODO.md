# TODO

Items are only marked done when a passing test exists for them.

This file holds only open / in-flight work. Completed and decided (out-of-scope / not-reproducible)
sections are archived in the topical `DONE/*.md` files, indexed by [DONE.md](DONE.md). When a
section here is fully done, move it into the matching `DONE/*.md` file rather than leaving it — see
CLAUDE.md.

A section records one finding, and its heading names where it came from — a REQUIREMENTS entry, a
[SCENARIOS.md](SCENARIOS.md) section, or a dated review pass.

**Open: the CGT reform from 1 July 2027** — the two sections below, added 2026-09-21 from the ATO
reference mirrored in [`docs/ato/cgt-reform-boosting-home-ownership.md`](docs/ato/cgt-reform-boosting-home-ownership.md)
(QC 107304) and [`docs/ato/cgt-reform-cgt-adjustments.md`](docs/ato/cgt-reform-cgt-adjustments.md)
(Explanatory Memorandum Chapter 1). Nothing is wrong in the meantime: a trade `date` is bounded above
by today (`AmountsError::FutureDate`), so no CGT event dated on or after 1 July 2027 can exist in any
database until that date arrives — the commencement guard that keeps that true once it does has
landed (`src/domain/cgt_reform.rs`; see [`DONE/tax-domain.md`](DONE/tax-domain.md)). Three sections
closed 2026-09-21 and were archived there: cost base indexation (including the Subdivision 112-E boundary
split it needed), the 30 June 2027 reacquisition / deferred gain / pre-CGT assets (whose remaining
s 112-185 and pre-CGT questions were decided as recorded scope cuts, not code), and the assets and
situations outside the data model. The two sections that follow — the seven-step method statement and
Division 119's minimum tax — are the substance.

The 2026-09-17 code review pass — the last recorded here before this — is fully closed: its
two CGT-arithmetic defects (the G1 excess's FX date and the cost-base pipeline's single end-floor)
were archived before the rest, and the 26 sections that followed them were fixed with passing tests
and moved to [`DONE/reviews.md`](DONE/reviews.md) on 2026-09-17. One item among them was closed N/A
rather than applied — the stale comment inside the already-applied migration
`0045_autoincrement_audited_ids.sql` is left byte-identical, because sqlx checksums an applied
migration's whole text and correcting the comment would stop `infra::db::init` against any database
where 0045 has already run; the correction is recorded in that section's archive note instead. The
pass's own summary, and the closing narrative that stood here, are in
[`DONE/verification-passes.md`](DONE/verification-passes.md).

Before this pass, every section recorded here was closed and archived — the last was the annual tax
report's version stamp, and before it its foreign income totals (both REQUIREMENTS entries, closed
2026-08-28 and moved to [`DONE/reporting.md`](DONE/reporting.md)), and before it the 2026-08-28
cyclomatic complexity audit, whose six items (the `tax_summary` split, the two nesting outliers, the
`rights_sale` anchoring walk, the `corporate_action` presence flags, the `upsert_sell_in_tx`
parameters struct, and the decision not to gate complexity in CI) closed on 2026-08-28 and moved to
[`DONE/reviews.md`](DONE/reviews.md). The closing narrative that used to stand here — the
pass-by-pass record of driving SCENARIOS.md sections S through AA, and the last two sections to
close before that audit (the distribution calendar and the 2026-08-25 code review) — was moved to
[`DONE/verification-passes.md`](DONE/verification-passes.md) on 2026-08-28. The maintained record of
what has been verified is SCENARIOS.md's
[Verification status](SCENARIOS.md#verification-status) table and its per-section findings blocks;
the maintained record of what was built and decided is the `DONE/*.md` archive.



## CGT reform from 1 July 2027 — the seven-step net capital gain method statement
(EM 1.75–1.107, new subsection 102-5(1) and s 102-6. The method statement becomes seven steps over four categories — deferred non-residential, deferred residential, non-residential, residential — with current-year losses applied in that order (step 1), carried-forward losses next (step 2), quarantined rental amounts at the two new steps 3 and 4, the discount at step 5, small business concessions at step 6.)
- [ ] Replace the two-bucket netting in `reports::net_capital_gain` with the seven-step statement over the four categories, in the statutory order. The walk is one function — `net_years` (`src/reports/net_capital_gain.rs:903`), whose loss ordering is at `:942-954` (losses hit `other_gains` first, then `discount_eligible_gains`) and whose discount is the hard-coded `/ Decimal::TWO` at `:966` — and the parcel optimiser / pre-sale what-if call the same `gross_buckets` + `net_years` pair (`src/reports/parcel_optimiser.rs:1405`), so they follow the new order with no second implementation
- [ ] Categorise a gain as residential only where a residential dwelling was used to provide residential accommodation (s 102-6). This project holds shares, units and crypto (`listings.security_type IN ('Share','ETF','LIC','Trust','Crypto')`), so every gain is **non-residential** and the four categories collapse to deferred/current. Record that as the stated reason the residential categories and the quarantined-amount steps 3–4 are nil — not an omission — and note what would have to change (an asset class and a dwelling-use record) to make them reachable
- [ ] Keep the existing loss chain working through the new order: `reports::net_capital_gain`'s `capital_loss_brought_forward` / `capital_loss_carried_forward` walk and `cgt_settings.opening_capital_loss` (`migrations/0001_schema.sql:91`, `src/entities/cgt_settings.rs`) remain the loss inputs; only the consumption order changes — deferred gains before current-year gains
- [ ] Apply the discount at step 5 to what remains discountable: the deferred gains (old law), plus the new-residential/affordable-housing arms this app cannot hold; never to a post-2027 indexed gain
- [ ] Update the ATO-label mapping and the exports: `CSV_HEADER` / `CSV_ATO_LABELS` in `src/reports/net_capital_gain.rs`, the Annual Tax Report's CGT summary and its disposal schedule (whose `gain_after_discount_aud` is a notional per-parcel pre-netting working, and changes meaning once some gains are indexed), and `docs/ato/tax-return-labels-2026.md`'s mapping section. Re-verify against the 2028 individual return when it is published — labels shift
- [ ] NEEDS CLARIFICATION: what a post-2027 AMMA statement reports. The app reads `amma_statements.cgt_discount_gains` / `cgt_indexation_gains` / `cgt_other_gains` into the year's buckets, and Subdivision 115-C reverses cost-base indexation for a beneficiary who cannot index (EM 1.153–1.156, Example 1.15). Re-fetch `docs/ato/amma-statement-guidance-notes.md` and the AMMA form once the ATO publishes the post-reform version, then decide whether new components/labels are needed
- [ ] Tests: a deferred gain is netted against losses before a current-year gain; the discount falls only on the discountable categories; the loss chain still carries a remainder into the next year. EM Example 1.9 (Asher) is partly reproducible — its rights and ASX-shares arms (`\$600,000` deferred + `\$400,000` current; `\$12,000` current) — while its residential/quarantined arm is N/A per the categorisation item
- [ ] Docs sync: `docs/API.md`'s net-capital-gain section (the new step order and fields) and README/FEATURES

## CGT reform from 1 July 2027 — the 30 per cent minimum tax on capital gains (Division 119)
(EM 1.171–1.193, new Division 119 of the ITAA 1997, Division 119 of the ITTP Act and s 12AA of the Income Tax Rates Act 1986. An Australian-resident individual pays extra income tax on the year's *minimum tax capital gain* where a seven-step *minimum tax gap amount* is positive — the benchmark 30 per cent less the tax the gain already bears as the top slice of taxable income. Recipients of prescribed income-support payments are exempt.)
- [ ] NEEDS CLARIFICATION: decide how far Division 119 can be implemented, because steps 2–4 need a **basic income tax liability** on taxable income with and without the minimum tax capital gain — and this project computes no tax payable at all. There is no taxable-income total (`TaxYearSummary`'s `net_assessable_investment_income` is investment-only), no home for salary or other non-investment income (`income.listing_id` is `NOT NULL`, and `IncomeType` is Dividend/EmploymentIncome/OtherIncome with `EmploymentIncome` informational only), and no marginal-rate schedule anywhere in `src`. Options: (a) record the year's other taxable income in `tax_year_settings` and implement the resident rate schedule; (b) surface the *minimum tax capital gain* only and leave the gap to the taxpayer; (c) record the gap amount as a per-year user figure. (b)/(c) follow the FITO and ESS precedents; (a) makes the figure computable but adds a tax-payable engine this project has never had
- [ ] If (a): extend `tax_year_settings` with the per-year taxpayer facts — the year's taxable income from sources outside the app, the income-support-payment exemption, and the residency answer the s 114-25 testing period needs — as a migration that DROPs and re-CREATEs the table's two `*_row_history_*` triggers with the new column list (it is an audited table; `migrations/0027_tax_year_settings.sql` is the pattern). Update `COLUMNS`/body in `src/entities/tax_year_settings.rs`, `docs/API.md`, `docs/SCHEMA.md` and the `tax_year_settings` SPA entry in `src/web/config.js`
- [ ] If (a): the resident marginal-rate schedule and a `basic_income_tax_liability(taxable_income, tax_year)` function in a cited `domain` module (step 2's "basic income tax liability" under s 4-10(3), before offsets and before the Medicare levy). This is a new class of figure for the project — it needs its own tests against the ATO's published rates and a maintenance note: the schedule changes with each Budget, so a new rate year is a code change
- [ ] Compute the **minimum tax capital gain**: the covered gains remaining after step 6 (s 119-5) — the post-1 July 2027, non-residential gains left after losses. Excluded: the deferred pre-2027 gains (EM 1.175), and new-residential/affordable-housing gains that chose the discount (N/A here)
- [ ] Compute the seven-step **minimum tax gap amount** (s 119-10(2)): step 1 the 30 per cent benchmark; steps 2–4 the tax the gain already bears as the top slice; steps 5–6 the difference, rounded down to whole dollars; step 7 positive or nil. Then the extra rate is `minimum tax gap amount ÷ minimum tax capital gain` (Rates Act s 12AA)
- [ ] Apply the exemptions and exclusions: no minimum tax where the year carries a payment the Minister prescribes (intended to include the Age Pension, Disability Support Pension, JobSeeker, Parenting Payment, Youth Allowance, farm household allowance, ABSTUDY living allowance and the special rate disability pension — the instrument is not yet made), or where the taxpayer already pays at least 30 per cent on the gain. Record the exemption per year rather than inferring it (the ESS-eligibility pattern)
- [ ] Decide where the figure surfaces: `reports::tax_summary` has no net-capital-gain field at all and its `total_assessable_income` (`tax_summary.rs:1127`) deliberately excludes capital gains, so there is no existing seam. Either add the reform fields to `TaxYearSummary` (and its CSV/label rows) or keep them on `reports::net_capital_gain` / `tax_report`'s `cgt_summary` alone — decide once, and say in `docs/API.md` where a reader should look
- [ ] Surface the result as an informational figure with its working — the minimum tax capital gain, the 30 per cent benchmark, the tax on the gain as top slice, the gap and the effective extra rate — so a reader can check it and no liability the app does not otherwise compute is silently asserted. `reports::TAXPAYER_BASIS` must state the resident-individual and post-reform basis
- [ ] Tests: EM Example 1.17 (Genevieve — a `\$50,000` minimum tax capital gain and `\$40,000` of other taxable income give step 1 `\$15,000`, step 4 `\$14,500`, a `\$500` gap and a 1 per cent extra rate = `\$500`). The EM's example states rounded tax figures against an illustrative schedule, so pin the test either to the rates its figures imply or to the enacted schedule for the year once legislated — and state which
- [ ] Tests: no gap where the taxpayer's marginal rate already reaches 30 per cent; a prescribed income-support payment zeroes the gap; the rate formula reproduces the gap exactly
- [ ] Docs sync: `docs/API.md` (the new fields, the settings columns, and a Known-limitations entry for whatever Division 119 input remains the taxpayer's), README/FEATURES, and the `docs/ato/OVERVIEW.md` mapping
