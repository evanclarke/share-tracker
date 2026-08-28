# Deductions spread over more than one income year

Two ordinary share-investor expenses are **not** deductible in full in the year
they are incurred — they are apportioned across the years they cover. Both are
mirrored here because `investment_expenses` records one expense as one
`date_incurred` in one financial year, so a multi-year expense has to be
entered as one row per year (see *Why this matters for this project*).

## 1. Borrowing expenses — 5 years or the loan term, whichever is shorter

> **Source:** https://www.ato.gov.au/individuals-and-families/investments-and-assets/shares-funds-and-trusts/investing-in-shares/owning-shares/dividend-income-deductions
> **Retrieved:** 2026-08-17
> QC 104069, last updated 22 June 2026.
> The live ATO site is authoritative; this is a convenience mirror.

### Borrowing expenses

> You may be able to claim expenses for taking out a loan to buy shares that
> are expected to produce dividend income. The expenses may include
> establishment fees, legal expenses and stamp duty on the loan. If your
> expenses total more than \$100, apportion them over 5 years or the loan term,
> whichever is shorter. If your expenses are \$100 or less, you can claim a
> deduction for the full amount in the year you incur them.

(Statutory rule: s 25-25 ITAA 1997.) Note that this is the *cost of taking out
the loan*, not the **interest** on it:

> **Interest** — If you borrow money to buy shares, you can claim a deduction
> for the loan interest, provided it is reasonable to expect that you'll
> receive assessable dividends. Where the loan was also used for private
> purposes, only claim interest on the part used to acquire the shares.

Ordinary loan interest is deductible when incurred, so an ordinary monthly
interest charge is one row in one year and needs none of this.

## 2. Prepaid expenses — the 12-month rule, then apportion by days

> **Source:** https://www.ato.gov.au/forms-and-instructions/deductions-for-prepaid-expenses-2026/deductible-non-business-expenditure
> **Retrieved:** 2026-08-17
> QC 106556, published 30 May 2026 (part of *Deductions for prepaid expenses 2026*).
> The live ATO site is authoritative; this is a convenience mirror.

A passive share investor's expenditure is **non-business expenditure**: "Other
examples include certain expenditure made for a rental property or shares held
purely as a passive investment."

### Summary of rules including the 12-month rule

> If you're an individual, your prepaid non-business expenditure is immediately
> deductible under the 12-month rule if either
>
> - the eligible service period for the expenditure is 12 months or less
> - the period ends no later than the last day of the income year following the
>   year in which the expenditure was incurred.
>
> When the eligible service period is more than 12 months or it ends after the
> last day of the next income year, you apportion your deduction for prepaid
> non-business expenditure over the lesser of either
>
> - the eligible service period
> - 10 years.

### Calculating your deduction if the 12-month rule isn't satisfied

> If you incur prepaid non-business expenditure and the eligible service period
> is more than 12 months or it ends after the last day of the next income year,
> you must use the following formula to work out your deduction:
>
> **A multiplied by (B divided by C)**
>
> Where:
> **A** is expenditure.
> **B** is the number of days of the eligible service period in the income year.
> **C** is the total number of days of the eligible service period.

### Example: eligible service period of more than 12 months

> On 1 January 2026, Martin, a senior clerk employed by a legal firm, paid
> \$1,250 for a subscription for a monthly professional journal. The
> subscription is for 1 January 2026 to 31 January 2027 (396 days). As the
> eligible service period is more than 12 months, Martin must apportion his
> deduction over the income years 2025–26 and 2026–27. Martin's deductions are:
>
> - 2025–26 (1 January 2026 to 30 June 2026): \$1,250 × (182 ÷ 396) = \$572
> - 2026–27 (1 July 2026 to 31 January 2027): \$1,250 × (215 ÷ 396) = \$678
>
> The total deduction allowed proportionately over the income years 2025–26 and
> 2026–27 is \$1,250.

*(The ATO rolls this example's dates forward each year — the 2025 edition ran
1 January 2025 to 31 January 2026 over 397 days, giving \$573 / \$677. The rule
and the formula are the same; only the day counts move.)*

### Example: the 12-month rule satisfied (immediately deductible)

> On 1 June 2026 Jasmin, an employed solicitor, paid \$1,750 for a subscription
> for a monthly professional journal for 1 June 2026 to 31 May 2027. The
> provision of the journal is the 'service to be done under the agreement'. The
> period of subscription is wholly within a 12-month period ending before the
> last day of the next income year. So, Jasmin is entitled to a deduction for
> the expenditure in 2025–26.

## Why this matters for this project

One `investment_expenses` row is one `date_incurred`, one financial year, and
one deduction claimed in full in that year: the tax summary's deduction loop
buckets each row by `tax_year_for(date_incurred)` and totals it there. That is
correct for the ordinary case — a management fee, an account-keeping fee, a
month's loan interest, and any prepayment *inside* the 12-month rule, which is
immediately deductible exactly as the model already treats it.

It is wrong for the two rules above if a multi-year expense is entered as a
single row: a \$2,000 loan establishment fee keyed once claims the whole \$2,000
in the first year instead of \$400 in each of five, and a prepayment failing the
12-month rule claims the whole amount a year early.

Neither `gross_amount` nor `deductible_percentage` helps — they record the
private-vs-income-producing split, which is a split of the *amount*, not of the
*time*.

**The documented entry convention is one row per financial year**, each
carrying that year's apportioned share (`amount` = this year's deduction,
`date_incurred` = a date inside that year, `description` naming the whole
expense and the year of the sequence — "loan establishment fee, year 2 of 5").
The per-year figures then flow through the tax summary and the annual tax
report unchanged, and the working that produced them is the taxpayer's, the
same as the apportionment percentage already is. This is stated as a Known
limitation in `docs/API.md`; modelling it properly would mean a
`service_period_start`/`service_period_end` pair the tax summary apportions by
days, which changes what "the year a row is in" means for every report that
answers it with one date.
