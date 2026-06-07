# ATO Reference Documentation — Overview

Local copies of Australian Taxation Office (ATO) guidance, retrieved **2026-06-01**
(worked-example pages marked otherwise were retrieved 2026-06-06), to
support correct implementation of the capital-gains, AMIT/AMMA, and income-attribution
calculations in this project. Each file carries its source URL and retrieval date in a
header block. **The live ATO site (ato.gov.au) is authoritative** — these copies are a
convenience snapshot and may go stale (tax rules and form layouts change yearly).

These pages were captured because several TODO items in
[`../TODO.md`](../TODO.md) need clarification on intended ATO behaviour before
implementation — see "How this maps to open TODO items" at the end.

## Capital gains tax — calculation mechanics

| File | What it covers |
| --- | --- |
| [`cgt-how-to-calculate.md`](cgt-how-to-calculate.md) | The headline method: net capital gain = total capital gains − capital losses − CGT discount. Worked examples. Confirms tax is paid on **net** capital gains and only half the gain survives the 50% discount. |
| [`cgt-discount.md`](cgt-discount.md) | The 50% CGT discount for Australian-resident individuals: the **12-month ownership** rule (acquisition to CGT event, exclusive of both days), what qualifies, and that the discount is applied **after** losses. |
| [`cgt-using-capital-losses.md`](cgt-using-capital-losses.md) | Order of offsetting: subtract capital losses **before** the discount; you choose which gains to apply losses to, and applying them to **non-discountable gains first** minimises tax. Current-year vs carried-forward losses; net capital losses carry forward indefinitely and can't offset ordinary income. |
| [`cgt-cost-base.md`](cgt-cost-base.md) | The **five elements** of the cost base, the **reduced cost base** (used when there's a loss; no indexation; different third element), and what each element includes. Worked reduced-cost-base example. |
| [`cgt-dividend-reinvestment-plans.md`](cgt-dividend-reinvestment-plans.md) | DRP tax treatment: the dividend is assessable income and the new shares are acquired for the dividend amount on the reinvestment date. Worked example (Natalie) — reproduced in `src/ato_examples.rs`. |
| [`cgt-keeping-records-shares.md`](cgt-keeping-records-shares.md) | Parcels bought at different times are **separate CGT assets**; the seller chooses which parcel a sale comes from (specific identification). Worked example (Boris) — reproduced in `src/ato_examples.rs`. |
| [`cgt-non-assessable-payments.md`](cgt-non-assessable-payments.md) | Return of capital: **CGT event G1** (company) reduces the cost base, with any excess over cost base a capital gain; E4/E10 for trusts. Worked example (Rob) — reproduced in `src/ato_examples.rs`. Modelled as the `ReturnOfCapital` corporate action. |
| [`share-splits-and-consolidations.md`](share-splits-and-consolidations.md) | **TD 2000/10** (retrieved 2026-06-06): a share split or consolidation is **no CGT event** — the converted shares keep the **original acquisition date** and section 112-25 attributes a **proportionate cost base** (total unchanged, per-unit scales inversely). Worked examples (John, Examples 1–2) — reproduced in `src/ato_examples.rs`. Modelled as the `ShareSplit` corporate action. |
| [`bonus-shares.md`](bonus-shares.md) | **Bonus shares** (Guide to CGT, retrieved 2026-06-06): a non-assessable bonus issue (the general post-1 July 1998 case) gives the bonus shares the **original parcel's acquisition date** and **apportions the parcel's cost base** over original + bonus shares (total unchanged, per-unit shrinks). Dividend-assessed bonus shares (chosen in lieu of a dividend) form a new parcel at the issue date with cost base = the dividend — a DRP. Worked example (Chris, Example 35) — reproduced in `src/ato_examples.rs`. Modelled as the `BonusIssue` corporate action. |
| [`rights-issues.md`](rights-issues.md) | **Rights or options to acquire shares or units** (Guide to CGT, QC 64895, retrieved 2026-06-06): free rights are NANE income on issue; **exercising is no CGT event** — the new shares are **acquired on the exercise date** (the 12-month discount clock runs from exercise, not from the rights) with first-element cost base = the rights' cost base (nil if issued free) + the amount paid to exercise. Sold/lapsed rights take the **original parcel's acquisition date** when issued free. Worked examples (Shanti, Examples 39–40; Example 40 reproduced in `src/ato_examples.rs`). Modelled as the `RightsIssue` corporate action + exercise operation. |
| [`takeovers-and-scrip-for-scrip.md`](takeovers-and-scrip-for-scrip.md) | **Takeovers and mergers + scrip-for-scrip rollover** (Guide to CGT, QC 64895, retrieved 2026-06-07): a takeover exchange is a CGT event at the market value of the consideration, unless the **scrip-for-scrip rollover** applies — the capital gain is disregarded and the replacement shares are **acquired for the cost base of the original interest**, with the **combined holding period** counting toward the 12-month CGT discount. Partial rollover where cash is received (cost base apportioned by proceeds). Worked examples (Desiree Example 26, Gunther Example 27, Stephanie Example 28 — none reproducible: 26 is the no-rollover election, 27 has a cash component, 28 has two replacement classes; the modelled case is the full-rollover single-class exchange). Modelled as the `ScripForScrip` corporate action + exchange operation. |
| [`demergers.md`](demergers.md) | **Demergers + demerger rollover** (Guide to CGT, QC 64895, retrieved 2026-06-07): under an eligible post-July-2002 demerger, holders of the head entity receive new interests in the demerged entity; with rollover the capital gain/loss is **disregarded** and the original cost base is **apportioned by the head-entity-advised percentages** over the remaining head interests and the new interests (relative market value method). The new interests' 12-month discount clock runs from the **original interests' acquisition date** (rollover case); the head interests' acquisition dates are unchanged. Worked examples (Anita Examples 30–31, discount Examples 32–33; Example 30 reproduced in `src/ato_examples.rs`). Modelled as the `Demerger` corporate action + demerge operation. |
| [`share-buy-backs.md`](share-buy-backs.md) | **Share buy-backs** (QC 66049, retrieved 2026-06-06): selling shares back to the company is a CGT event. An off-market buy-back's capital proceeds can't be less than the market value had the buy-back not been proposed, **less any dividend** paid as part of the buy-back price (the dividend is separately assessable with its franking credits); a listed-company buy-back announced after 7:30 pm AEDT 25 Oct 2022 has **no dividend component** — the whole price is capital proceeds. Worked example (Ranjini) — reproduced in `src/ato_examples.rs`. Modelled as the `BuyBack` corporate action + participate operation. |
| [`you-and-your-shares-dividends.md`](you-and-your-shares-dividends.md) | Dividend assessable income (franked + unfranked + franking-credit gross-up; worked example, John — reproduced in `src/ato_examples.rs`) and the **45-day holding period rule** + **$5,000 small-shareholder exemption** for franking-credit entitlement (worked examples, Matthew and Jessica — both reproduced in `src/ato_examples.rs`). The entitlement rules are implemented in `src/reports/franking.rs`, applied by the tax summary. |

## AMIT / AMMA — attribution and cost-base adjustments

| File | What it covers |
| --- | --- |
| [`amit-cost-base-adjustments.md`](amit-cost-base-adjustments.md) | **The key document for the open cost-base clarification.** Defines the **AMIT cost base net amount** = balance of the cost-base increase amount (assessable + NANE income attributed) and the cost-base decrease amount (actual payments/entitlements + tax offsets), netted annually. Critically: **tax-free and tax-deferred amounts are *not* directly used to adjust cost base** — they are only *broadly reflected* in the single AMIT cost base net amount the trust states on the AMMA statement. Covers upward *and* downward adjustment, reduction-to-nil, and **CGT event E10** (excess net reduction → capital gain). |
| [`amit-reporting-requirements.md`](amit-reporting-requirements.md) | What an AMIT must report to members and the ATO (AMMA statement contents, AIIR alignment, timing — within 3 months of year end), and how member components are characterised. |
| [`amit-calculating-trust-components.md`](amit-calculating-trust-components.md) | How trust components (assessable income, exempt, NANE, tax offsets) are determined and attributed to members under the attribution method. Context for what feeds the cost-base increase/decrease amounts. |
| [`amma-statement-about.md`](amma-statement-about.md) | What the AMMA statement / Standard Distribution Statement (SDS) is, its purpose, and the recommended disclosure format. |
| [`amma-statement-guidance-notes.md`](amma-statement-guidance-notes.md) | **Field-by-field reference for the AMMA statement** (Parts A/B/C). Part C lists each income/CGT/credit component a member receives — the direct analogue of the columns on this project's `amma_statements` table, including the AMIT cost base net amount line. Largest/most detailed file here. |

## Other income components (TODO items needing clarification)

| File | What it covers |
| --- | --- |
| [`lic-capital-gain-deduction.md`](lic-capital-gain-deduction.md) | The **LIC capital gain deduction** (Subdiv 115-D): an individual deducts **50%** of the LIC capital gain amount advised on the dividend statement (33⅓% for super/life; 50% for trusts/partnerships). Drives the `lic_capital_gain_deduction` income field. |
| [`mytax-managed-funds.md`](mytax-managed-funds.md) | How managed-fund/trust distribution components map to tax-return labels: franked/unfranked dividends, **franking credits**, foreign income & **foreign income tax offset**, **conduit foreign income** (NANE — excluded from assessable income), capital gains, AMIT cost base net amount, TFN amounts withheld. Reference for the `income` and `amma_statements` component fields and how each is treated. |
| [`fito-limit.md`](fito-limit.md) | The **FITO offset limit** (Guide to foreign income tax offset rules 2025, retrieved 2026-06-06): up to **A$1,000** of foreign income tax is claimable without a limit calculation; above that the offset limit (steps 1–3 over the taxpayer's full tax position) applies — not computable from this system's data. Worked example (Anna, Example 16). Drives the tax summary's `foreign_tax_offsets` cap + `foreign_tax_offset_excess`. |

## How this maps to open TODO items

- **AMMA cost-base driver** (TODO "Review Findings — Needs Clarification"): the open question is
  whether `tax_deferred_amount`, `tax_free_amount`, or the per-unit `cost_base_adjustment` should
  drive the cost-base adjustment. [`amit-cost-base-adjustments.md`](amit-cost-base-adjustments.md)
  resolves it: for an AMIT the cost base is adjusted by the single **AMIT cost base net amount**
  stated on the AMMA statement (the per-unit `cost_base_adjustment` field), **not** by tax-deferred
  or tax-free amounts directly — those are only "broadly reflected" in that net amount. This supports
  keeping `cost_base_adjustment` as the driver and treating `tax_deferred_amount` / `tax_free_amount`
  as informational-only (or removing them), and points to **CGT event E10** as the not-yet-modelled
  edge case when the net reduction exceeds the remaining cost base.
- **CGT discount + loss netting** (already implemented in `/portfolio/net-capital-gain`):
  [`cgt-using-capital-losses.md`](cgt-using-capital-losses.md) and
  [`cgt-discount.md`](cgt-discount.md) confirm the order the project uses — losses applied to
  non-discountable gains first, then to discount-eligible gains, then halve the remainder.
- **Income components** (LIC deduction, conduit foreign income, foreign tax offset, franking credits):
  [`lic-capital-gain-deduction.md`](lic-capital-gain-deduction.md) and
  [`mytax-managed-funds.md`](mytax-managed-funds.md) document the intended treatment behind the
  `income` / `amma_statements` fields and the tax-summary aggregation rules.
- **FITO cap** (TODO "Foreign income tax offset (FITO) cap"):
  [`fito-limit.md`](fito-limit.md) confirms the A$1,000 de-minimis the tax summary applies, and
  why the full offset-limit calculation (and its Example 16) is out of scope — it needs the
  taxpayer's whole income-tax position.
