# TODO

Items are only marked done when a passing test exists for them.

Completed and decided (out-of-scope / not-reproducible) sections are archived in [DONE.md](DONE.md) to keep this list focused on active work. When a section here is fully done, move it to DONE.md rather than leaving it — see CLAUDE.md.

## AMIT cash distributions — assessable-income double-count (2026-06-12)

(REQUIREMENTS 2026-06-12, from the full-archive data-entry verification: AMIT cash income rows are needed to drive DRP but their cash inflates `dividends_assessable` ~$12k–22k/FY alongside the AMMA attribution — for an AMIT the AMMA is the only assessable record.)

- [ ] AMIT distribution income rows are cash-only: they fund DRP reinvestment (per-share cross-check, ex-date enrolment check, residual chain unchanged) but contribute nothing to the tax summary's income lines (`dividends_assessable`, `gross_assessable_investment_income`, credits, withholding) — exclusion driven by the listing's `amit` flag or an explicit income kind, validated at write time
- [ ] Non-blocking cross-check report (pattern: E4 cross-check): flag every FY with AMIT cash rows whose listing has no AMMA statement covering that year; an AMMA year with no cash rows is not flagged
- [ ] Non-AMIT trust and ordinary dividend rows unchanged (regression tests)
- [ ] Live-data check: with the 2020–2026 archive entered, each FY's `dividends_assessable` drops to the non-AMIT components only (PLS franked dividends), and the `amma_*` lines still reproduce the AMMA Part A figures
- [ ] Docs sync: `docs/API.md` Income + Tax summary sections, README Features, web UI config if the income form changes

## Fractional-share DRP reinvestment (2026-06-12)

(REQUIREMENTS 2026-06-12: Morgan Stanley reinvests ICE dividends in fractional shares (0.500, 0.434, …) with no residual; the whole-share-only reinvest forced nine plain-Buy workarounds priced net-cash ÷ units.)

- [ ] Reinvest accepts the statement's fractional allotment — explicit `units` (broker figure authoritative, price cross-checked against reinvestable cash) or a per-enrolment whole/fractional mode; the stated units must be representable exactly
- [ ] Whole-share floor + residual carry stays the default; all existing whole-share tests unchanged
- [ ] Live-data check: the nine ICE plain-Buy reinvestments are re-enterable through the reinvest operation with the statements' exact fractional units
- [ ] Docs sync: `docs/API.md` DRP reinvestment section, `docs/SCHEMA.md` if a column is added, README DRP feature bullet, web UI reinvest form

## ESS statement AUD override (2026-06-12)

(REQUIREMENTS 2026-06-12: employer statements convert at release-date spot, the tax summary at the RBA monthly rate — $65–214/yr apart in the live data; the ATO prefill carries the employer's AUD figure.)

- [ ] `ess_statements` gains optional statement-AUD discount amounts (at minimum the total assessable discount); tax summary reports them verbatim when present, RBA-converts as today when absent
- [ ] Live-data check: with the employer AUD figures entered, `ess_discount_assessable` equals the ATO ESS statements exactly (FY2022 10,572; FY2023 9,443; FY2024 11,731; FY2025 13,526)
- [ ] Docs sync: `docs/SCHEMA.md` ess_statements block, `docs/API.md` ESS section + Tax summary, web UI ESS form fields

## statement_total tolerance for cent-rounded contract notes (2026-06-12)

(REQUIREMENTS 2026-06-12: contract notes print the consideration cent-rounded; 3 of 41 archive notes were rejected by the exact comparison and entered without the cross-check.)

- [ ] The cross-check passes when the supplied total equals the computed figure rounded to the cent (half away from zero); exact matches keep passing; larger mismatches still 422 with the computed figure in the body (Buys and Sells)
- [ ] Live-data check: trades 16, 19, 21 (the three entered without the cross-check) accept their contract-note totals
- [ ] Docs sync: `docs/API.md` Trades + Sells statement_total paragraphs, Response codes 422 row

## Known-limitation documentation — RSU dividend equivalents, foreign broker interest (2026-06-12)

(REQUIREMENTS 2026-06-12. Documentation-only; no modelling. Doc-only items are test-pinned via `src/doc_checks.rs`.)

- [ ] Known limitations: dividend equivalents on unvested RSU grants are ordinary income when paid and are not modelled — enterable manually as income if paid out in cash
- [ ] Known limitations: interest income reports at question 10 (10L) regardless of source; foreign broker-cash/money-market income strictly belongs at 20E — state the simplification



