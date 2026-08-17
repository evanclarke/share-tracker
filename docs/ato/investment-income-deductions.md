# Interest, dividend and other investment income deductions

> **Source:** https://www.ato.gov.au/individuals-and-families/income-deductions-offsets-and-records/deductions-you-can-claim/investments-insurance-and-super/interest-dividend-and-other-investment-income-deductions
> **Retrieved:** 2026-06-08
> QC 72187, last updated 8 June 2026.
> The live ATO site is authoritative; this is a convenience mirror.

Deductions you can claim for the costs of earning interest, share dividends, or income from
other investments.

## Interest income expenses

You can claim a deduction for **account-keeping fees** you incur on an account held for
investment purposes, such as bank accounts or income bonds. You will find these fees on your
statements.

If you have a joint account, you can only claim **your share** of the fees, charges or taxes on
the account. For example, if you hold an equal share in an account with your spouse, you can only
claim half of any allowable account-keeping fees. (Apportionment is the taxpayer's
determination — this tool stores the post-apportionment deductible amount.)

## Investment seminars

If you attend an investment seminar about an **existing** investment, you may be entitled to
claim a deduction for the **portion** of your expenses that relate to earning investment income.

You **can't** claim a deduction to attend a seminar about something you're *considering* investing
in, even if you subsequently invest in it.

## Interest you pay on borrowed money

If you borrow money to buy shares or other investments from which you earn dividends or other
**assessable income**, you can claim a deduction for the **interest** you pay.

Only interest expenses you incur for an **income-producing purpose** are deductible.

If you use the money you borrow for both private and income-producing purposes, you must
**apportion** the interest between each purpose.

You **can't** claim a deduction if you receive an **exempt** dividend or other exempt income.

## Dividend and share income expenses

### What you can claim

You can claim a deduction for costs you incur to invest in shares, including:

- limited **financial advice fees** — for example, ongoing management fees or advice about
  changes in your investment mix
- the **portion** of your costs that are for managing your investments — for example, some
  travel expenses, such as to attend the annual general meeting of a company you hold shares in
- the cost of **specialist investment journals and subscriptions**
- **borrowing costs and interest expenses**
- the cost of internet access
- the decline in value of your computer
- 50% of the **listed investment company (LIC) capital gain amount** — if you were an Australian
  resident when a LIC paid you a dividend that included a LIC capital gain amount (modelled
  separately: the statement's advised amount is the `income.lic_capital_gain_amount` field and
  the 50% is computed for question D8, not an investment expense).

### What you can't claim

When you invest in shares, you can't claim:

- **financial advice fees** about your *proposed* investments or future income-earning structure,
  or where there is no connection with income-earning activities
- some interest expenses where you borrow money under a **capital-protected borrowing**
  arrangement to buy shares, units in unit trusts and stapled securities (the interest is treated
  as the cost of the capital-protection feature)
- **brokerage fees and other transaction costs** (but you can include these costs to work out
  your capital gains tax when you sell the shares — in this tool, brokerage is part of a trade's
  cost base, never an investment-expense deduction).

## How this maps to the project

Drives the `investment_expenses` entity and the tax summary's deductions side. The expense-type
enum mirrors the deductible categories above: `LoanInterest` (interest on borrowed money),
`ManagementFee` / `AdviceFee` (financial-advice and management fees), `AccountKeepingFee`,
`Subscription` (specialist journals/subscriptions), and `Other`. The tool stores the
**post-apportionment deductible amount** (the figure that goes on the return) — the ATO's
apportionment rules (joint accounts, private vs income-producing use, partial seminar/travel)
are the user's determination, not computed here. Brokerage and the LIC capital gain deduction are
deliberately *not* expense rows: brokerage is a cost-base element on the trade, and the LIC
deduction is its own income field.
