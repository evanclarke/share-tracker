# myTax 2025 Dividend deductions

> **Source:** https://www.ato.gov.au/individuals-and-families/your-tax-return/instructions-to-complete-your-tax-return/mytax-instructions/2025/deductions/deductions-for-donations-investments-and-managing-your-tax-affairs/dividend-deductions
> **Retrieved:** 2026-06-08
> Part of the myTax 2025 instructions (parent QC 104207), last updated 2 June 2025.
> The live ATO site is authoritative; this is a convenience mirror.

How to claim dividend deductions when lodging your tax return using myTax.

## Things to know

To claim a deduction for dividend expenses, you must have **incurred the expenses in earning
income** included in the Dividends section. Dividend and distribution income are amounts paid or
credited to you by Australian companies you held shares in, and include dividends applied under a
**dividend reinvestment plan** or dealt with on your behalf.

If a listed investment company (LIC) pays you a dividend that included a capital gain amount, you
can claim a deduction of **50%** of the LIC capital gain at this section. (Modelled separately: the
`income.lic_capital_gain_amount` field records the advised LIC capital gain amount and the 50% is
computed for the tax summary's D8 line.)

## What you can claim as a dividend deduction

Dividend expenses you can claim a deduction for may include:

- **management fees and fees for investment advice** relating to changes in the mix of your
  investments
- **interest** you paid on money you borrowed to buy shares or similar investments
- **costs relating to managing your investments**, such as travel and buying specialist
  investment journals or subscriptions.

If you borrowed money to buy assets for both private use and income-producing investments, you can
claim only the **portion** of the interest expenses relating to the income-producing investments.

Interest you incurred on investments you made using a **capital-protected borrowing** may not be
fully deductible.

You can claim part of the **decline in value of your computer** using the percentage of your total
computer use that related to managing your investments. Where the same computer manages both
interest-producing and share investments, claim the related decline in value **once**, in either
*Interest income deductions* or *Dividend deductions* (don't double-count).

Deductions for some expenses (such as interest and borrowing costs) may be affected by the **thin
capitalisation rules** if they relate to certain overseas investments, or to Australian
investments held by a foreign resident. These rules may apply when total debt deductions exceed
\$2 million for the year.

## What you can't claim as a dividend deduction

You can't claim expenses for:

- financial advice received from someone who isn't either a tax agent with a current Tax
  Practitioners Board registration, or a qualified tax relevant provider with a current ASIC
  registration
- some interest expenses where you borrow under a capital-protected product or borrowing
- **brokerage fees and other transaction costs** (these go to the CGT cost base instead).

## Don't show at this section

- expenses incurred earning **trust and partnership** distributions (go to Partnerships or Trusts)
- expenses incurred earning **foreign-source dividends** (go to Other foreign income or Other
  deductions)
- expenses for an investment proposal **before** acquiring an asset (unless carrying on an
  investment business).

## How this maps to the project

Confirms the deductible expense categories the `investment_expenses` entity records and the tax
summary nets against assessable dividend/distribution income per Australian financial year: the
expenses must be incurred in **earning assessable dividend income**, the deductible figure is
**post-apportionment** (private vs income-producing use is the user's determination), and
**brokerage is excluded** (it belongs to the CGT cost base). The optional `listing_id` /
`holding_account_id` on an expense row tie a deduction to the holding it relates to; a
portfolio-wide expense leaves both NULL.

The **"Don't show at this section"** list above is what routes each deduction to its question:
`domain::deduction_destination` derives the destination from the holding the expense is attributed
to — a trust or AMIT holding at **13Y**, a foreign-source holding at **20M** (or **D15** for a debt
deduction), everything else at D7/D8 — and the tax summary carries a line per destination. The
labels and the instruction text behind them are in
[`tax-return-labels-2026.md`](tax-return-labels-2026.md), *Where an investment-expense deduction
goes*.
