# TODO

Items are only marked done when a passing test exists for them.

Completed and decided (out-of-scope / not-reproducible) sections are archived in [DONE.md](DONE.md) to keep this list focused on active work. When a section here is fully done, move it to DONE.md rather than leaving it — see CLAUDE.md.

## Deductible investment expenses
(REQUIREMENTS "Deductible investment expenses" 2026-06-08. The tax summary reports gross assessable income with no deductions side, overstating the net position. Add a place to record investment-expense deductions — chiefly interest on money borrowed to buy income-producing shares, plus management/adviser fees, account-keeping fees, subscriptions — and net them in the tax summary. Distinct from the existing LIC capital gain deduction. Not present anywhere in DONE.md.)
- [ ] Mirror the ATO guidance into `docs/ato/` ("Interest, dividend and other investment income deductions" + "Dividend income deductions"; source URL + retrieval date), indexed in `docs/ato/OVERVIEW.md`
- [ ] New `investment_expenses` entity + migration: id, date incurred, expense-type enum (`LoanInterest`/`ManagementFee`/`AdviceFee`/`AccountKeepingFee`/`Subscription`/`Other`, CHECK-constrained), amount (TEXT Decimal), `currency` (FK→currencies, AUD default), description, optional `listing_id` + `holding_account_id` FKs (both nullable — portfolio-wide expense). CRUD per the entity module pattern
- [ ] Apportionment: store the **deductible amount** (post-apportionment, the figure that goes on the return) as the totalled value; optionally keep gross + deductible-percentage for provenance (informational). The tool does not rule on correct apportionment — the user's determination
- [ ] Tax summary deductions side per Australian financial year: total by expense type + overall, and a **net assessable investment income** field (existing gross totals − deductions), gross figures retained. Non-AUD expenses converted to AUD via the ATO rate at the month incurred (`infra::fx::to_aud`), fail loudly when no rate (never mix currencies / never silent zero). Tax-return CSV export carries the new columns
- [ ] Web UI: CRUD screen via the `ENTITIES` config; new tax-summary columns surface automatically (report columns derive from response keys); asserted in the served bundle
- [ ] Tests: entity CRUD round-trip with decimal precision; enum/FK constraints (422 on unknown currency/listing/account); a non-AUD expense converts to AUD (fails loudly with no rate); the tax summary nets deductions by type and overall and computes net assessable income; CSV export columns; web bundle assertion
- [ ] Docs: `docs/SCHEMA.md` (new table + Relationships), `docs/API.md` (endpoints, new tax-summary fields, 422 causes), README Features list (investment-expense deductions)
