# ESS dividend equivalent payments — ordinary income when paid (TD 2017/26)

> **Source:**
> https://www.ato.gov.au/law/view/print?DocID=TXD%2FTD201726%2FNAT%2FATO%2F00001&PiT=99991231235958
> (Taxation Determination TD 2017/26, *Income tax: employee share schemes — when a
> dividend equivalent payment is assessable to an employee as remuneration*,
> consolidated version)
> **Retrieved:** 2026-06-12
> The live ATO site is authoritative; this is a convenience mirror.

## What a dividend equivalent payment is

A **dividend equivalent payment** is a cash payment paid by the trustee of an
employee-share-scheme trust to an employee, ordinarily calculated by reference
to the dividends (less trustee tax) the employee would have earned had they
owned the shares from the day they received their interest in the trust —
i.e. the dividends accrued on **unvested** grants over the vesting period.

## Ruling

A dividend equivalent payment is assessable to the employee **as remuneration
(and therefore ordinary income) under section 6-5** ITAA 1997 when it is
received for, or in respect of, services provided as an employee — or where
the payment has a sufficient connection with the employment.

It is **not** a dividend in the employee's hands, and it is not part of the
ESS discount: "while the quantum of the payment reflects a dividend equivalent
that may have been received had the employee acquired the shares at the outset
of the arrangement, this is merely a calculation mechanism and does not
reflect the character of the payment in the recipient's hands. The character
of the payment in the employee's hands is remuneration."

## Examples (abridged)

- **Example 1 — assessable as remuneration.** The ESS agreement entitles an
  employee who satisfies performance/continuous-employment conditions to the
  shares **plus** an amount reflecting the post-tax dividends the shares earned
  during vesting. Because the payment is made for satisfying those conditions,
  it is in substance a reward for performance — assessable under s 6-5 when
  received.
- **Example 2 — not remuneration.** The trustee has an absolute discretion
  (independent of the employer, service, or performance conditions) to pay a
  dividend equivalent to a beneficiary, even one no longer employed. Such a
  payment is received in the capacity of trust beneficiary, has only a distant
  causal connection to employment, and is not assessable as remuneration.

## Date of effect

Applies to dividend equivalent payments paid under the terms attached to ESS
interests granted **on or after 1 January 2018**. For interests granted before
that date, the Commissioner's general administrative practice is to treat such
payments as not assessable (conditional on the underlying dividends having
been assessed to the trustee under s 99A ITAA 1936).

## How this project uses it

Dividend equivalents accrue on **unvested** RSU grants, which this system does
not track (unvested grants are not shares — see Known limitations in
`docs/API.md`). They are therefore **not modelled**: no entry path computes or
classifies them. A dividend equivalent paid out in cash is ordinary
employment-connected income in the year received and can be entered manually
as an income row if the user wants it aggregated here at all — it is not an
ESS discount, not a dividend, and carries no franking.
