# Calculate your FITO or offset limit

> **Source:** https://www.ato.gov.au/forms-and-instructions/foreign-income-tax-offset-rules-guide-2025/calculate-your-fito-or-offset-limit
> **Retrieved:** 2026-06-06
> Part of "Guide to foreign income tax offset rules 2025" (QC 104349, last updated 29 May 2025).
> The live ATO site is authoritative; this is a convenience mirror.

## About the FITO limit

You claim the FITO in your tax return. First check if the amount you claim is subject to a
foreign income tax offset limit.

As a non-refundable tax offset, the foreign income tax offset reduces your income tax payable
(including Medicare levy and Medicare levy surcharge).

Under the tax offset ordering rules, the foreign income tax offset is applied after all other
non-refundable tax and non-transferable offsets. Once your tax payable has been reduced to nil,
any unused foreign income tax offset is not refunded to you and can't be carried forward to
later income years.

### FITO up to $1,000

To claim a foreign income tax offset of **up to $1,000**, you only need to record the actual
amount of foreign income tax paid that counts towards the offset (up to $1,000). No offset-limit
calculation is required.

### FITO more than $1,000

If you are claiming a foreign income tax offset of **more than $1,000**, you have to work out
your **foreign income tax offset limit**. This may result in your tax offset being reduced to
the limit. Any foreign income tax paid in excess of the limit is not available to be carried
forward to a later income year and can't be refunded to you.

## Claiming your FITO

Before you calculate your net income, you must convert all foreign income, deductions and
foreign tax paid to Australian dollars.

## How to calculate your offset limit

If you are claiming a foreign income tax offset of more than $1,000, you will first need to work
out your foreign income tax offset limit. The offset limit is based on a comparison between your
tax liability and the tax liability you would have if certain foreign-taxed and foreign-sourced
income and related deductions were disregarded.

- **Step 1:** Work out the income tax payable by you (including Medicare levy and Medicare levy
  surcharge) for the year, excluding penalties and interest and disregarding any tax offsets.
- **Step 2:** Work out the income tax that would be payable under the same exclusions if
  - your assessable income didn't include any amount on which foreign income tax has been paid
    that counts towards your FITO, nor any other income or gains from a non-Australian source; and
  - you were not entitled to: debt deductions attributable to your overseas permanent
    establishment; any other deductions reasonably related to the disregarded income; or the
    foreign loss component of tax losses deducted in the year. Deductions relating to both
    disregarded and other assessable income are apportioned on a reasonable basis. Gifts,
    contributions, superannuation and tax agent's fees are not considered reasonably related to
    the disregarded amounts.
- **Step 3:** Subtract the result of step 2 from step 1. If the result is greater than $1,000,
  this is your offset limit.

### Example 16: foreign income tax offset limit

Anna, an Australian-resident taxpayer for the year ended 30 June 2025, has income and expenses
and pays foreign income tax for the income year as follows:

| Income and deductions | Amount |
| --- | --- |
| Employment income from Australia | A$22,000 |
| Employment income from United States | A$6,000 |
| Employment income from United Kingdom | A$4,000 |
| Rental income from United Kingdom | A$1,000 |
| Dividend income from United Kingdom | A$600 |
| Interest income from United Kingdom | A$400 |
| **Total assessable income** | **A$34,000** |
| Expenses incurred in deriving employment income from Australia | A$2,000 |
| Expenses incurred in deriving employment income from United States | A$450 |
| Expenses incurred in deriving rental income from United Kingdom | A$250 |
| Interest (debt deduction) incurred in deriving dividend income from United Kingdom | A$70 |
| Expenses (debt deduction) incurred in deriving interest income from United Kingdom | A$30 |
| Gift to deductible gift recipient | A$70 |
| **Total allowable deductions** | **A$2,870** |
| **Taxable income** | **A$31,130** |

| Foreign income tax paid on | Amount |
| --- | --- |
| Employment income from United States | A$1,800 |
| Employment income from United Kingdom | A$1,200 |
| Dividend income from United Kingdom | A$60 |
| Interest income from United Kingdom | A$40 |
| Rental income from United Kingdom | A$300 |
| **Total foreign income tax paid** | **A$3,400** |

- Step 1: tax on $31,130 = **$2,581.80** (includes Medicare levy).
- Step 2: disregarding the A$12,000 of foreign income and the A$700 of related expenses
  (the A$100 of debt deductions and the A$70 gift are *not* disregarded), taxable income under
  the assumptions is A$19,830; tax on $19,830 = **$260.80** (below the Medicare low-income
  threshold, so no levy).
- Step 3: $2,581.80 − $260.80 = **$2,321.00**.

This is Anna's foreign income tax offset limit. Although she has paid foreign income tax of
$3,400, her foreign income tax offset is limited to **$2,321.00**. The difference can't be
refunded or carried forward to a future income year.

(The page also covers: a JPDA-income special case; an adjustment to step 2 for deferred
non-commercial business losses, where a zero-or-negative net foreign amount means the offset is
the lower of the foreign tax paid or the default $1,000 limit, with Example 17; and the special
4-year amendment rules where foreign tax is paid, increased or refunded in a different year to
the related income, with Example 18 — all outside this project's scope.)

## Relevance to this project

The offset-limit calculation (steps 1–3) needs the taxpayer's **full income-tax position**
(employment income, deductions, Medicare levy), which is outside this system's data model — so
Example 16 is not reproducible here (see TODO "ATO worked-example acceptance tests"). What *is*
computable from this system's data is the **$1,000 de-minimis rule**: foreign income tax paid up
to A$1,000 in a year is claimable in full with no limit calculation; above A$1,000 the claimable
offset cannot be assumed beyond A$1,000 without the limit calculation. The tax summary therefore
caps the year's foreign tax offset at A$1,000 and separately surfaces the uncapped total, so the
user can claim more where their own offset-limit calculation supports it.
