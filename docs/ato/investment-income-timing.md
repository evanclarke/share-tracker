# Investment income — the year interest is declared in

> **Source:** https://www.ato.gov.au/individuals-and-families/income-deductions-offsets-and-records/income-you-must-declare/investment-income
> **Retrieved:** 2026-08-17
> QC 72101, last updated 8 June 2026.
> The live ATO site is authoritative; this is a convenience mirror.

Work out which investment income you must declare, such as interest, dividends,
rental income or other capital gains.

## When to declare investment income

You must declare income you earn from investments and assets in your tax
return. Investment income may include amounts from interest, dividends, rental
income, managed investment trust, crypto assets and capital gains.

You need to declare investment income when you receive payments directly or
through a distribution for a partnership (such as a share club) or trust.

## Interest income

If you're an Australian resident and you receive interest, you must declare it
as income. Interest income includes:

- interest you earn from financial institution accounts and term deposits
- interest you earn from any other source including penalty interest you
  receive on an investment
- interest you earn from children's savings accounts, if you
  - open or operate an account for a child and the funds in the account belong
    to you
  - spent or use the funds in the account
- interest we pay or credit to you – for example, interest on early payments,
  interest on overpayments and delayed refunds
- life insurance bonuses (you may be entitled to a tax offset equal to 30% of
  any bonus amounts you include in your income)
- interest from foreign sources (you can claim a foreign income tax offset for
  any tax paid on this income).

## Term deposits

> You must declare interest income in the year it is credited, received or
> applied or dealt with in any way on your behalf or as you direct. For term
> deposits this usually means you should declare interest in the year the
> investment matures.

If you elect to rollover your investment or if the financial institution
automatically reinvests the term deposit at maturity, you will need to declare
the interest earned as at the rollover or reinvestment date. This is the amount
you would have received if the investment was not rolled over or reinvested.

Similarly, you may choose to have the interest from a term deposit, held for
more than 12 months, credited to a different account periodically throughout
the life of the investment. In this case, the interest is assessable at the
dates of payment (which is before the date of maturity).

## Dividends

Dividend payments can be money or other property, including shares. If you
receive bonus shares instead of money, the company issuing the shares should
give you a statement that shows if the bonus shares are a dividend.

Dividend income may come from a listed investment company, public trading
trust, corporate unit trust, or corporate limited partnership (in the form of a
distribution). Some dividends have imputation or franking credits attached.

## Managed investment trusts

You must show any income you receive or credits you are entitled to from any
managed investment trust in your tax return. This includes income or credits
from a cash management trust, money market trust, mortgage trust, unit trust,
or managed fund — such as a property trust, share trust, equity trust, growth
trust, imputation trust or balanced trust.

## Why this matters for this project

An `interest_income` row carries **one** date, `date_paid`, and the tax summary
buckets the row into the Australian financial year that date falls in
(`tax_year_for`). The rule above says which date that must be: the day the
interest was **credited, received, or otherwise applied or dealt with** on the
holder's behalf or as they direct — **not** the day the money first became
reachable.

The two come apart on exactly the case this doc was mirrored for: a term
deposit that credits its interest on 30 June with the funds only withdrawable
on 2 July is declared in the year ended 30 June, not the next one. The date to
key is the statement's credit date; keying the availability date instead moves
a whole year's interest into the wrong return. For a term deposit that runs to
maturity with no interim credits, the credit date **is** the maturity date,
which is why the ATO's shorthand is "the year the investment matures". A
long-dated deposit that pays interest away to another account periodically is
assessed at those payment dates instead — again, each one is a credit date.

The distinction the project models explicitly elsewhere does **not** apply
here. A trust distribution carries `entitlement_date` beside `date_paid`
because trust income is assessed in the year of **present entitlement**
regardless of when it is paid (`trust-income-timing.md`) — two genuinely
different tax facts, so two columns. Interest has one tax fact, so one column:
there is no "available on" date in the assessment rule, and recording one would
only invite keying it as the assessment date.
