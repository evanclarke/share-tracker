# TODO

Items are only marked done when a passing test exists for them.

This file holds only open / in-flight work. Completed and decided (out-of-scope / not-reproducible)
sections are archived in the topical `DONE/*.md` files, indexed by [DONE.md](DONE.md). When a
section here is fully done, move it into the matching `DONE/*.md` file rather than leaving it — see
CLAUDE.md.

A section records one finding, and its heading names where it came from — a REQUIREMENTS entry, a
[SCENARIOS.md](SCENARIOS.md) section, or a dated review pass.

**Open: the CGT reform from 1 July 2027** — the one section below, added 2026-09-21 from the ATO
reference mirrored in [`docs/ato/cgt-reform-boosting-home-ownership.md`](docs/ato/cgt-reform-boosting-home-ownership.md)
(QC 107304) and [`docs/ato/cgt-reform-cgt-adjustments.md`](docs/ato/cgt-reform-cgt-adjustments.md)
(Explanatory Memorandum Chapter 1). Nothing is wrong in the meantime: a trade `date` is bounded above
by today (`AmountsError::FutureDate`), so no CGT event dated on or after 1 July 2027 can exist in any
database until that date arrives — the commencement guard that keeps that true once it does has
landed (`src/domain/cgt_reform.rs`; see [`DONE/tax-domain.md`](DONE/tax-domain.md)). Every other reform
section closed 2026-09-21 and was archived there: cost base indexation (including the Subdivision 112-E
boundary split it needed), the 30 June 2027 reacquisition / deferred gain / pre-CGT assets (whose
remaining s 112-185 and pre-CGT questions were decided as recorded scope cuts, not code), the Division
119 minimum tax (partly computed, its gap recorded as the taxpayer's own figure) and the assets and
situations outside the data model. The one section that follows — the seven-step method statement —
remains open for a single item, blocked on the ATO's post-reform AMMA guidance.

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
- [x] Replace the two-bucket netting in `reports::net_capital_gain` with the seven-step statement over the four categories, in the statutory order. The walk is one function — `net_years` (`src/reports/net_capital_gain.rs:903`), whose loss ordering is at `:942-954` (losses hit `other_gains` first, then `discount_eligible_gains`) and whose discount is the hard-coded `/ Decimal::TWO` at `:966` — and the parcel optimiser / pre-sale what-if call the same `gross_buckets` + `net_years` pair (`src/reports/parcel_optimiser.rs:1405`), so they follow the new order with no second implementation. **Done**: `net_years` is now the seven-step walk over `GainCategory::ALL` in statutory order, carrying the EM citations and the nil-residential reason in the module doc, with `cgt_reform::governs_tax_year` / `first_reform_tax_year` fixing the regime at the income year (FY2028 onward — the commencement is the year's first day, so no year straddles it and the same walk reproduces the old law exactly); both hypothetical-disposal reports follow it with no second implementation
- [x] Categorise a gain as residential only where a residential dwelling was used to provide residential accommodation (s 102-6). This project holds shares, units and crypto (`listings.security_type IN ('Share','ETF','LIC','Trust','Crypto')`), so every gain is **non-residential** and the four categories collapse to deferred/current. Record that as the stated reason the residential categories and the quarantined-amount steps 3–4 are nil — not an omission — and note what would have to change (an asset class and a dwelling-use record) to make them reachable. **Done**: `deferred_residential_gains` / `residential_gains` / `quarantined_amount` exist as fields and are always nil, with the s 102-6 reason and what would have to change stated in the module doc, the field docs and the steps 3–4 comment
- [x] Keep the existing loss chain working through the new order: `reports::net_capital_gain`'s `capital_loss_brought_forward` / `capital_loss_carried_forward` walk and `cgt_settings.opening_capital_loss` (`migrations/0001_schema.sql:91`, `src/entities/cgt_settings.rs`) remain the loss inputs; only the consumption order changes — deferred gains before current-year gains. **Done**: the chain and its inputs are unchanged; the order moved, pinned by `db_reform_losses_reduce_the_deferred_gain_before_the_current_year_gain`, whose asserted $1,285.60 is the figure the old order would have made $1,185.60
- [x] Apply the discount at step 5 to what remains discountable: the deferred gains (old law), plus the new-residential/affordable-housing arms this app cannot hold; never to a post-2027 indexed gain. **Done**: step 5 reduces each category's remaining discountable slice — the deferred old-law gain alone — and a post-2027 indexed gain's slice is empty (`db_the_reform_discount_falls_only_on_the_deferred_discountable_gain`)
- [x] Update the ATO-label mapping and the exports: `CSV_HEADER` / `CSV_ATO_LABELS` in `src/reports/net_capital_gain.rs`, the Annual Tax Report's CGT summary and its disposal schedule (whose `gain_after_discount_aud` is a notional per-parcel pre-netting working, and changes meaning once some gains are indexed), and `docs/ato/tax-return-labels-2026.md`'s mapping section. Re-verify against the 2028 individual return when it is published — labels shift. **Done**: the CSV carries 23 columns, with the four categories as `18H (category)` (an *alternative* breakdown of the same 18H total, never summed with the two `18H (component)` columns) and the remainders/quarantined amount as `18 (working)`; the Annual Tax Report prints the seven-step layout for a reform year and keeps the question-18 worksheet for a pre-reform one, with `gain_after_discount_aud` left as the split's own arithmetic and relabelled "gain after notional concession"; the 2028-form re-verification is recorded as a standing maintenance trigger in `docs/ato/tax-return-labels-2026.md`
- [ ] NEEDS CLARIFICATION: what a post-2027 AMMA statement reports. The app reads `amma_statements.cgt_discount_gains` / `cgt_indexation_gains` / `cgt_other_gains` into the year's buckets, and Subdivision 115-C reverses cost-base indexation for a beneficiary who cannot index (EM 1.153–1.156, Example 1.15). Re-fetch `docs/ato/amma-statement-guidance-notes.md` and the AMMA form once the ATO publishes the post-reform version, then decide whether new components/labels are needed. **Blocked on ATO publication** — checked 2026-09-21 and no post-reform AMMA form or guidance exists yet (the only published AMMA material is the pre-reform guidance already mirrored), so this is the section's one open item
- [x] Tests: a deferred gain is netted against losses before a current-year gain; the discount falls only on the discountable categories; the loss chain still carries a remainder into the next year. EM Example 1.9 (Asher) is partly reproducible — its rights and ASX-shares arms (`\$600,000` deferred + `\$400,000` current; `\$12,000` current) — while its residential/quarantined arm is N/A per the categorisation item. **Done**: the three order/discount/chain tests named above, plus EM Example 1.9 pinned both ways — `net_years_reproduces_em_example_1_9_ashers_net_capital_gain` (\$212,000, the EM's literal Step 1 amounts) and `net_years_reproduces_em_example_1_9_with_the_rights_current_component` (\$612,000, including the \$400,000 current component the EM's own Step 1 list and Step 7 total omit — an arithmetic gap in the EM, recorded in `docs/API.md`); the residential/quarantined arm stays N/A per the categorisation item
- [x] Docs sync: `docs/API.md`'s net-capital-gain section (the new step order and fields) and README/FEATURES. **Done**: `docs/API.md`'s net-capital-gain section and the reform Known-limitations entry (now stating the method statement is modelled and the residential/quarantined arms are structurally nil), the annual-tax-report `cgt_summary` bullet, README, `docs/FEATURES.md` (including the stale "is not implemented" bullet) and the `docs/ato/OVERVIEW.md` mapping, with every `doc_checks` pin re-pointed
