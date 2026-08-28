# Employee share schemes (ESS) — income side

> **Sources:**
> - https://www.ato.gov.au/forms-and-instructions/individual-tax-return-2025-instructions/income-questions-1-12-individual-tax-return-2025/12-employee-share-schemes-2025 (Item 12 instructions, QC 104101, last updated 27 May 2025)
> - https://www.ato.gov.au/businesses-and-organisations/corporate-tax-measures-and-assurance/employee-share-schemes/employers/types-of-ess/concessional-ess/tax-deferred-schemes (Tax-deferred schemes)
> - https://www.ato.gov.au/businesses-and-organisations/corporate-tax-measures-and-assurance/employee-share-schemes/employers/types-of-ess/concessional-ess/taxed-upfront-scheme-1000-dollar-reduction (Taxed-upfront scheme – \$1,000 reduction, QC 47628)
> **Retrieved:** 2026-06-08
> The live ATO site is authoritative; this is a convenience mirror.

## What this covers

The **income side** of an ESS: the assessable **discount** on employee share
scheme interests (ESS interests — shares, stapled securities, or rights to
acquire them) you receive under an employee share scheme, declared at **Item 12**
of the individual tax return in the year of the taxing point.

The **discount** is the difference between the **market value** of the ESS
interests and the amount you paid to acquire them. For a typical RSU the amount
paid is nil, so the discount equals the market value at the taxing point.

This is distinct from — and additional to — the **CGT side**, which this project
already handles: at the taxing point the ESS interest's first-element cost base
is **reset to its market value** and it is taken to be **re-acquired on the
taxing-point date** for CGT (so the 12-month discount clock and the cost base
both run from the taxing point). In this project that re-acquisition is the
cost-base-reset **Buy** the ESS vesting operation creates.

## Taxed-upfront vs deferral (taxing point)

- **Taxed-upfront scheme** — you pay tax on the discount in the **year you
  acquire** the interest.
- **Deferral scheme** (the RSU case) — if you and the scheme meet the
  conditions (e.g. a **real risk of forfeiture**, or a salary-sacrifice
  arrangement), the taxing point is **deferred** to the *deferred taxing point*:
  for a right, the earliest of — no real risk of forfeiture and no genuine
  disposal restriction; exercise (for interests acquired after 30 June 2015,
  where the resulting share is unrestricted); or **15 years** after acquisition.
  Ceasing employment is **no longer** a deferred taxing point (employment ending
  on or after 1 July 2022). The **30-day rule**: a disposal within 30 days after
  the deferred taxing point moves the taxing point to the disposal date.

The discount is assessable in the income year the taxing point occurs.

## Item 12 labels and the assessable discount

Your **Employee share scheme statement** (one per employer) shows the discount
split by scheme type, plus any TFN amounts withheld. The Item 12 steps:

| Step | Item 12 label | Amount |
| --- | --- | --- |
| 1 | **D** | Total discount from **taxed-upfront schemes eligible for reduction** (incl. any foreign-source discounts) |
| 2 | **E** | Total discount from **taxed-upfront schemes not eligible for reduction** (incl. foreign-source) |
| 3 | **F** | Total discount from **deferral schemes** with a deferred taxing point this year (incl. foreign-source) |
| 4–5 | **B** | **Total assessable discount** = D + E + F − the \$1,000 reduction (see below) |
| 6 | **C** | Total **TFN amounts withheld** from discounts |
| 7 | **A** | Of the above, the total discounts for which you're claiming a **foreign income tax offset** (a memo subset of B, for the FITO calculation at Item 20) |

Discounts on **pre-1 July 2009** ESS interests whose cessation time falls in the
year are also assessable (older returns carried these at label **G**); they add
to the assessable discount the same way.

### The \$1,000 reduction (taxed-upfront eligible only)

You may reduce the discounts from **taxed-upfront eligible** schemes (label D) by
**up to \$1,000** — i.e. by `min($1,000, D)`:

- D ≤ \$1,000 → B = E + F (the whole of D is removed)
- D > \$1,000 → B = D + E + F − \$1,000

**Eligibility is income-tested:** you qualify only if your *adjusted taxable
income* (taxable income computed without this reduction, plus reportable fringe
benefits, reportable employer super contributions, net financial-investment
loss, net rental-property loss, and deductible personal super contributions) is
**\$180,000 or less**. The reduction applies to **deferral** (label F) discounts
**not at all** — only to label D.

**Worked example — taxed-upfront eligible (QC 47628).** The ATO's example, verbatim:

> Core Bank Ltd provides its employee Matt 600 shares under an ESS on 4 August
> 2015.
>
> The total market value of the shares is \$3,600. Matt pays Core Bank Ltd
> \$1,200 to purchase the shares, acquiring the shares for a discount of \$2,400
> (\$3,600 less \$1,200).
>
> On 7 July 2016 Core Bank Ltd, gives Matt an ESS statement, with an amount of
> \$2,400 at label D "Discount from taxed upfront schemes – eligible for
> reduction".
>
> Core Bank Ltd lodges an ESS annual report showing all reportable ESS data for
> their employees with us by 14 August 2016.
>
> As Core Bank Ltd will not know Matt's taxable income after adjustments, they
> report the discount as \$2,400, ignoring the \$1,000 concession.

> **Project note — the two figures the ATO's example stops short of.** The
> example ends at what the *employer* reports; the employee's own two outcomes
> follow from rules stated elsewhere and are **not quoted text**:
>
> - **Assessable discount \$1,400.** Applying the \$1,000 reduction described
>   above (Matt being eligible, adjusted taxable income ≤ \$180,000):
>   \$2,400 − \$1,000 = \$1,400. The source page states only the rule ("reduce
>   the amount of the discounts … by up to \$1,000") and that the employer
>   reports the unreduced \$2,400.
> - **CGT cost base \$3,600.** The shares' first-element cost base is their
>   market value at the taxing point (the discount having already been taxed
>   as income), acquired 4 August 2015 — the general ESS/CGT interaction, not
>   a statement of this example.
>
> Both are what `src/ato_examples.rs` asserts, which reproduces this example.

## How this maps to the implementation

- The **`ess_statements`** entity captures one ESS statement: the per-type
  discount labels (`taxed_upfront_eligible` = D, `taxed_upfront_not_eligible` =
  E, `deferral_discount` = F, `pre_2009_cessation_discount` = G), the
  `foreign_source_discount` memo (label A subset), `tfn_withholding` (C), the
  `taxing_point_date`, and the per-share `market_value` and `quantity` that vest.
- The **tax summary** totals the **assessable ESS discount** per Australian
  financial year (`ess_discount_assessable` = D + E + F + G − the applied
  reduction), reported separately from dividend/trust income and in AUD
  (foreign-currency statements converted via the ATO rate for the taxing-point
  month). The applied reduction is surfaced as `ess_taxed_upfront_reduction`.
  The ESS TFN withheld is carried in the existing `tfn_withholding_tax` line.
- The **\$1,000 reduction** mirrors the FITO de-minimis pattern
  ([`fito-limit.md`](fito-limit.md)): the tool applies `min($1,000, D)` per year
  and **flags the ≤\$180,000 adjusted-taxable-income test as the user's
  responsibility** — that test needs the taxpayer's whole income position, which
  is outside this system's data.
- The **vesting operation** ties the income and CGT sides together: from one ESS
  statement it records the discount components and creates the cost-base-reset
  **Buy** (quantity vested, price = the per-share market value at the taxing
  point, acquisition/settlement = the taxing-point date), linked by provenance.
