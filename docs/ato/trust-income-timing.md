# Trust income — when a beneficiary is assessed (present entitlement)

> **Source:**
> https://www.ato.gov.au/businesses-and-organisations/trusts/trust-income
> ("Trust income", QC 23087, last updated 24 January 2019)
> **Retrieved:** 2026-06-10
> The live ATO site is authoritative; this is a convenience mirror.

## How trust income is taxed

The **net income** of a trust (effectively its taxable income) is its
assessable income for the year less allowable deductions.

Generally, the net income of a trust is taxed in the hands of the
beneficiaries (or the trustee on their behalf) based on their share of the
trust's income — that is, the share they are **'presently entitled'** to —
**regardless of when or whether the income is actually paid to them**.

A beneficiary is **presently entitled** to trust income for an income year
where they have, **by the end of that year**, a present or immediate right to
demand payment from the trustee. The entitlement will depend on the trust deed
and any discretion the trustee has under the deed to allocate income between
beneficiaries.

The trustee will need to provide each beneficiary with details of their share
of the net income, so that the beneficiaries can include this amount in their
tax returns.

## Franked distributions

Unless prevented by the trust deed, a beneficiary may be made specifically
entitled to a franked distribution, resulting in the beneficiary being taxed
on the franked distribution. If no beneficiary is specifically entitled, it is
taxed proportionately to all beneficiaries based on their entitlement to the
trust income.

If the trust is not a family trust, a beneficiary without a fixed entitlement
to the franked distribution is generally not entitled to use the associated
franking credits unless their total franking credits from all sources for a
year is $5,000 or less.

## Losses

A loss made by a trust in an income year can't be distributed to
beneficiaries. It can be carried forward and used to reduce the trust's net
income in a later year.

## Why this matters for this project

A managed fund / unit trust distribution **for** a period ending 30 June is
assessable in the income year of **present entitlement** (the year the
distribution period falls in), even when the cash is paid in July — unlike a
**dividend**, which is assessable when paid or credited. The tax summary
attributes `income` rows by `date_paid` (July ⇒ next FY); that rule is correct
for dividends but attributes a July-paid June trust distribution
(`trust_income = true`) to the **wrong** financial year. AMMA statements are
unaffected (they are attributed by `tax_year_end_date`).
