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
| [`cgt-non-assessable-payments.md`](cgt-non-assessable-payments.md) | Return of capital: **CGT event G1** (company) reduces the cost base, with any excess over cost base a capital gain; E4/E10 for trusts. Worked example (Rob). G1 is **not yet modelled** — see TODO "Corporate actions / additional CGT events". |
| [`you-and-your-shares-dividends.md`](you-and-your-shares-dividends.md) | Dividend assessable income (franked + unfranked + franking-credit gross-up; worked example, John — reproduced in `src/ato_examples.rs`) and the **45-day holding period rule** + **$5,000 small-shareholder exemption** for franking-credit entitlement (worked examples, Matthew and Jessica). The entitlement rules are **not yet modelled** — see TODO "Franking-credit entitlement rules". |

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
