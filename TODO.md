# TODO

Items are only marked done when a passing test exists for them.

Completed and decided (out-of-scope / not-reproducible) sections are archived in [DONE.md](DONE.md) to keep this list focused on active work. When a section here is fully done, move it to DONE.md rather than leaving it — see CLAUDE.md.

## Inherited share parcels (2026-06-10)

(REQUIREMENTS 2026-06-10; `docs/ato/inherited-assets-cost-base.md`, QC 66053.)

- [ ] Mirror the s 115-30 discount-clock rule for inherited assets into `docs/ato/` (confirm from the ATO source: post-CGT asset → discount period runs from the deceased's acquisition; pre-CGT asset → from the date of death) and index it in `OVERVIEW.md` — read before implementing
- [ ] Entry path for an inherited parcel: listing, holding account, units, date of death, cost base (recording which rule produced it), deceased's acquisition date (post-CGT case), LPR expenditure dated when incurred; provenance visible (not a market Buy)
- [ ] The parcel flows through every report and write-time capacity check like a Buy; the discount clock follows the mirrored s 115-30 rule
- [ ] Web UI via the existing config-driven entity/action patterns
- [ ] Tests: cost-base and discount-clock cases (post-CGT and pre-CGT deceased); `ato_examples.rs` acceptance test for any representable worked example
- [ ] Docs: `docs/SCHEMA.md`, `docs/API.md`, README Features; Known limitations: estate/LPR side not modelled, market value at death is user-supplied

## Renounceable rights — selling, lapsing, retail premiums (2026-06-10)

(REQUIREMENTS 2026-06-10; `docs/ato/rights-issues.md` already documents the sold/lapsed treatment. Supersedes the README line saying selling/lapsing rights is not modelled — update that text as part of this work.)

- [ ] Sell-rights operation against a `RightsIssue`: units (capped, together with exercises, at the entitlement), proceeds per right, sale date → provenance-marked disposal taking the **original parcel's acquisition date** for the discount; nil cost base for free rights, carried cost for paid rights (nil proceeds = lapse of a paid right → capital loss); reaches realised + net-capital-gain reports
- [ ] NEEDS CLARIFICATION: retail premiums — fetch and mirror the ATO retail-premiums guidance into `docs/ato/`, resolve the income character, and only then model (or record out of scope)
- [ ] Web UI: `ACTIONS` entry
- [ ] Tests: entitlement cap shared with exercises; discount anchoring; `ato_examples.rs` Example 39 (sold-rights case)
- [ ] Docs: `docs/SCHEMA.md`, `docs/API.md` (operation + 422 cases), README Features + Known limitations text

## Takeovers with a cash component — partial scrip-for-scrip rollover (2026-06-10)

(REQUIREMENTS 2026-06-10; `docs/ato/takeovers-and-scrip-for-scrip.md` Example 27 (Gunther). Supersedes the README "partial cash consideration not modelled" note — update it as part of this work.)

- [ ] Extend `ScripForScrip` with an optional per-unit cash component; the exchange operation apportions each consumed parcel's remaining reduced cost base between cash and scrip by the consideration's market values, recognises the cash-side gain/loss (discount per the original holding period) in the realised + net-capital-gain reports, and creates replacement parcels for the scrip side exactly as today
- [ ] All-scrip behaviour unchanged; pure-cash takeovers remain ordinary Sells
- [ ] Tests: apportionment arithmetic; `ato_examples.rs` Example 27 acceptance test
- [ ] Web UI: the action/operation config gains the cash field
- [ ] Docs: `docs/SCHEMA.md` (new column + CHECKs), `docs/API.md`, README Features + Known limitations (no-rollover and multi-class cases stay out)

## CGT decision support — parcel-selection optimiser and pre-sale what-if (2026-06-10)

(REQUIREMENTS 2026-06-10; `docs/ato/cgt-keeping-records-shares.md` — parcel choice is the taxpayer's. Read-only; nothing persisted.)

- [ ] Parcel-selection optimiser report: given listing, account, units, sale date, price (live-fetched default per the live-valuation rules; explicit override wins) → candidate strategies (minimise current-year gain, maximise discount-eligible proportion, harvest losses first, FIFO baseline), each with per-parcel allocations and gross gain / discountable split
- [ ] Pre-sale what-if: net-capital-gain accepts a hypothetical disposal (units, proceeds, date, allocations or a named strategy) and returns the year's figures with and without it — dry run, no rows written; whole-of-income tax estimate stays out of scope
- [ ] Web UI: screens via the existing `REPORTS`/action config
- [ ] Tests: each strategy's allocation choice; what-if leaves the DB untouched; API tests
- [ ] Docs: `docs/API.md`, README Features

## Compliance alert reports — wash sales and franking at-risk foresight (2026-06-10)

(REQUIREMENTS 2026-06-10; non-blocking, pattern: MIC validation / settlement coverage.)

- [ ] Mirror the ATO wash-sale guidance (TR 2008/1 / current ATO page) into `docs/ato/` + `OVERVIEW.md` — read before implementing
- [ ] Wash-sale report: every loss-realising Sell with a Buy of the same listing within a configurable window (default 30 days), either side, across all holding accounts; writes never rejected
- [ ] Franking at-risk foresight report: each dividend whose credits are denied by the 45-day walk (with the failing window/dates), plus a contemplated-sale mode reusing the holding-period walk; surfaced near the Sell flow in the UI
- [ ] Tests: window edges; cross-account detection; denied-credit explanation matches the tax summary's denial
- [ ] Docs: `docs/API.md` (both reports), README Features

## Tax-return label mapping on the CSV exports (2026-06-10)

(REQUIREMENTS 2026-06-10.)

- [ ] Verify the current year's myTax/paper labels from the ATO instructions and mirror the label reference into `docs/ato/` (+ `OVERVIEW.md`), recording which year's form the mapping targets
- [ ] Carry the mapping on the exports themselves (second header row or label column) without changing existing columns; document the full mapping in `docs/API.md`
- [ ] Tests: export carries the labels; existing column assertions unchanged
- [ ] Docs: `docs/API.md`, README

## Interest income (2026-06-10)

(REQUIREMENTS 2026-06-10. The `income` entity is listing-keyed, so interest needs its own entity.)

- [ ] New entity `interest_income` (standard module pattern + migration): date paid, amount, currency (AUD default; ATO-rate conversion at the month paid), TFN withholding, optional `holding_account_id`, source description
- [ ] Tax summary: `interest_income` line per FY, included in `gross_assessable_investment_income` (and so netted by deductions); TFN amount joins the existing withholding line; CSV export updated
- [ ] Web UI: `ENTITIES` entry
- [ ] Tests: entity CRUD; FY aggregation + FX conversion + fail-loudly; gross/net identity
- [ ] Docs: `docs/SCHEMA.md` (incl. Relationships), `docs/API.md`, README Features

## Operational hardening — restore, off-disk backups, localhost default (2026-06-10)

(REQUIREMENTS 2026-06-10.)

- [ ] `--backup-dir` option (default: beside the DB, as today) so backups can land on another volume; scheduler/backup job honours it
- [ ] Document the restore procedure in the README and prove it with a test (backup → mutate → restore → assert pre-mutation state)
- [ ] Default `--host` changes to `127.0.0.1`; `0.0.0.0` remains opt-in and the README security note inverts accordingly
- [ ] Tests: backup lands in the configured dir; restore round-trip; default-bind assertion
- [ ] Docs: README flags table + Scheduled maintenance section

## Known-limitation documentation — gifts, pre-CGT holdings, indexation (2026-06-10)

(REQUIREMENTS 2026-06-10. Documentation-only; no modelling.)

- [ ] Known limitations (docs/API.md + README): gifts / off-market related-party transfers are a disposal at market value (market-value substitution) — enterable today as a manual Sell or Buy at market value
- [ ] Known limitations: pre-CGT holdings (acquired before 20 September 1985) are outside CGT and not modelled — the system would wrongly compute gains on such a parcel
- [ ] Known limitations: the indexation method (pre-21 September 1999 acquisitions, frozen at Sep 1999) is not modelled; the 50% discount is used throughout

