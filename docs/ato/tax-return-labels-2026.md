# Individual tax return 2026 — label reference for this project's figures

> **Form year: 2026** (Individual tax return 2026 + supplementary tax return, covering FY2025–26,
> the year ended 30 June 2026). Labels shift year to year — re-verify against the next year's
> instructions before reusing this mapping.
>
> **Sources** (all retrieved 2026-06-11 from ato.gov.au, instructions "Last updated 30 May 2026"):
>
> - 10 Gross interest 2026 — https://www.ato.gov.au/forms-and-instructions/individual-tax-return-2026-instructions/income-questions-1-12-individual-tax-return-2026/10-gross-interest-2026
> - 11 Dividends 2026 — https://www.ato.gov.au/forms-and-instructions/individual-tax-return-2026-instructions/income-questions-1-12-individual-tax-return-2026/11-dividends-2026
> - 12 Employee share schemes 2026 — https://www.ato.gov.au/forms-and-instructions/individual-tax-return-2026-instructions/income-questions-1-12-individual-tax-return-2026/12-employee-share-schemes-2026
> - 13 Partnerships and trusts 2026 — https://www.ato.gov.au/forms-and-instructions/individual-supplementary-tax-return-2026-instructions/income-questions-13-24-supplementary-tax-return-2026/13-partnerships-and-trusts-2026
> - 18 Capital gains 2026 — https://www.ato.gov.au/forms-and-instructions/individual-supplementary-tax-return-2026-instructions/income-questions-13-24-supplementary-tax-return-2026/18-capital-gains-2026
> - 20 Foreign source income and foreign assets or property 2026 — https://www.ato.gov.au/forms-and-instructions/individual-supplementary-tax-return-2026-instructions/income-questions-13-24-supplementary-tax-return-2026/20-foreign-source-income-and-foreign-assets-or-property-2026
> - D7 Interest income deductions 2026 — https://www.ato.gov.au/forms-and-instructions/individual-tax-return-2026-instructions/deduction-questions-d1-d10-individual-tax-return-2026/d7-interest-income-deductions-2026
> - D8 Dividend deductions 2026 — https://www.ato.gov.au/forms-and-instructions/individual-tax-return-2026-instructions/deduction-questions-d1-d10-individual-tax-return-2026/d8-dividend-deductions-2026
> - D15 Other deductions not claimable elsewhere in your tax return 2026 — https://www.ato.gov.au/forms-and-instructions/individual-supplementary-tax-return-2026-instructions/deduction-questions-d11-d15-supplementary-tax-return-2026/d15-other-deductions-not-claimable-elsewhere-in-your-tax-return-2026 (retrieved 2026-08-20, "Last updated 30 May 2026")
> - myTax 2026 Managed funds (SDS label cross-reference) — https://www.ato.gov.au/individuals-and-families/your-tax-return/instructions-to-complete-your-tax-return/mytax-instructions/2026/income/managed-fund-or-trust-distributions/managed-funds
>
> This is a local copy of ATO guidance for reference. The ATO site is authoritative.

myTax uses the same labels: a pre-filled or statement-sourced amount is shown in myTax under
its paper label and field name (e.g. "13U Total non-primary production income" on the myTax
managed-funds screen), so one mapping serves both lodgment paths.

The two CSV exports (`/portfolio/tax-summary/export`, `/portfolio/net-capital-gain/export`)
carry this mapping as their second header row; the full per-column table is in
[`../API.md`](../API.md).

## Question 10 — Gross interest

| Label | Field |
| --- | --- |
| 10L | Gross interest (include TFN amounts withheld in the gross figure) |
| 10M | TFN amounts withheld from gross interest (show cents) |

(Exported by the tax summary: `interest_income` → 10L; its TFN amount withheld joins the combined `tfn_withholding_tax` column → 10M.)

## Question 11 — Dividends

| Label | Field |
| --- | --- |
| 11S | Unfranked amount (include TFN amounts withheld; include amounts treated as dividends) |
| 11T | Franked amount (a statement that doesn't split franked/unfranked goes wholly at T) |
| 11U | Franking credit (exclude credits the taxpayer isn't entitled to claim — holding-period rule etc.) |
| 11V | TFN amounts withheld from dividends (show cents) |

## Question 12 — Employee share schemes

| Label | Field |
| --- | --- |
| 12D | Discount from taxed upfront schemes — eligible for reduction |
| 12E | Discount from taxed upfront schemes — not eligible for reduction |
| 12F | Discount from deferral schemes |
| 12B | Total assessable discount amount = D + E + F, less the up-to-$1,000 taxed-upfront reduction where the ≤$180,000 income test is met |
| 12C | TFN amounts withheld from discounts |
| 12A | Foreign-source discounts for which a foreign income tax offset is claimed (memo for question 20) |

The pre-1 July 2009 cessation label (G on earlier forms) no longer appears on the 2026 form.

## Question 13 — Partnerships and trusts (non-primary production)

| Label | Field |
| --- | --- |
| 13U | Share of net income from trusts, less capital gains, foreign income and franked distributions (the SDS "non-primary production income": Australian interest, unfranked dividends, other income, net rent) |
| 13C | Franked distributions from trusts, **including** the share of attached franking credits |
| 13Q | Share of franking credit from franked dividends (the offset entitlement; may differ from the grossed-up credit inside 13C where trust deductions were allocated to it) |
| 13R | Share of credit for TFN amounts withheld from interest, dividends and unit trust distributions |
| 13S | Credit for tax paid by trustee |
| 13X / 13Y | Other deductions relating to distributions (X primary production, Y non-primary production) |

## Question 18 — Capital gains

| Label | Field |
| --- | --- |
| 18G | Did you have a CGT event during the year? (X in Yes/No) |
| 18H | Total current year capital gains — gross gains before losses and discount; trust/AMMA discount gains grossed up (×2) before inclusion |
| 18A | Net capital gain — after current + prior-year losses and the CGT discount (zero, not blank, when losses reduce gains to nil) |
| 18V | Net capital losses carried forward to later income years (next year's brought-forward amount) |
| 18M | Exemption or rollover applied (X + code) |
| 18X | Credit for foreign resident capital gains withholding |

## Question 20 — Foreign source income

| Label | Field |
| --- | --- |
| 20E | Assessable foreign source income (gross) |
| 20M | Other net foreign source income (the net amount of 20E income after expenses) |
| 20O | Foreign income tax offset — up to $1,000 without the offset-limit calculation, confirming the FITO de-minimis in `fito-limit.md` (show cents) |

## Deductions D7 / D8 / D15

| Label | Field |
| --- | --- |
| D7 (label I) | Interest income deductions — expenses of earning interest income |
| D8 (label H) | Dividend deductions — expenses of earning dividend/distribution income, **plus the 50% LIC capital gain deduction** ("you can claim a deduction of 50% of the LIC capital gain amount at this question") |
| D15 (label J) | Other deductions not claimable elsewhere — including **debt deductions** (interest, borrowing costs) incurred earning assessable income that are claimed nowhere else, "for example, foreign source income that you include at question 20" |

Expenses of earning trust/partnership distributions belong at 13X/13Y, not D7/D8.

## Where an investment-expense deduction goes

The deductible *amount* of an investment expense is one figure, but the question it is claimed at
follows the income it was incurred earning. myTax 2025 *Dividend deductions* (mirrored in
[`dividend-income-deductions.md`](dividend-income-deductions.md)) keeps two classes off D7/D8 under
**"Don't show at this section"** — trust and partnership distribution expenses ("go to Partnerships
or Trusts") and foreign-source dividend expenses ("go to Other foreign income or Other deductions").
The three instruction pages resolve those to labels (all three re-fetched 2026-08-20, unchanged from
the 2026-06-11 capture apart from the D15 page, added then):

**Trust and partnership distributions → 13Y** (13X for primary production). Question 13, *Non-primary
production deductions*, step 2:

> Write at question 13 – label **Y** the total of other deductions (including non-commercial business
> losses deferred from a prior income year) you can claim in relation to your share of both:
> non-primary production income or loss from a partnership; non-primary production income of a
> trust, including deductions relating to franked distributions from trusts.

**Interest on money borrowed to buy units in a trust follows the trust**, rather than staying at D7:
question 13's Part C says so by treating debt deductions as claimed at X/Y —

> If you incurred debt deductions, the thin capitalisation rules may affect the amount you can deduct
> at labels **X** and **Y**. For example, debt deductions such as interest and borrowing costs, in
> deriving assessable income from a trust or partnership, of more than $2,000,000 …

**Foreign-source income → netted into 20M, except debt deductions, which go to D15.** Question 20
Part E (*other foreign source income*, which is where a foreign company's dividends are shown),
step 2:

> Add up all the deductible expenses that you incurred in earning the foreign income you show at
> Part E, Step 1, **excluding any debt deductions**. Write the total at Worksheet 1 – row **s**.
>
> Debt deductions, such as interest and borrowing costs, aren't deductible for the purposes of this
> calculation unless they related to income you earned through a permanent establishment in an
> overseas country. If you incurred debt deductions in earning your foreign income … see question
> **D15** Other deductions not claimable elsewhere in your tax return 2026.

Row s is subtracted from the gross (row r) on the way to **20M** *Other net foreign source income*,
so a foreign-income expense is claimed by reducing 20M rather than at a deduction label of its own;
the gross at **20E** is unreduced. D15 confirms the other half:

> You may claim 'debt deductions' you incurred in earning assessable income (for example, foreign
> source income that you include at question 20 in your supplementary tax return) at this question,
> if you didn't claim them elsewhere in your tax return.

**Everything else → D7 / D8**, above. D15 explicitly is *not* the place for "expenses relating to
investment planning and advice involving shares, unit trusts and interest-bearing deposits" —
"Other questions deal with these matters."

> **Project note.** `domain::deduction_destination` implements this routing over what the data model
> records, and the tax summary carries a line per destination (`deductions_trust_distributions` →
> 13Y, `deductions_foreign_income` → 20M, `deductions_foreign_debt` → D15,
> `deductions_dividend_and_interest` → D7/D8). Only `LoanInterest` is treated as a debt deduction.
> Primary production (13X) is not modelled: a managed fund's primary-production component has no
> field, so a trust deduction always routes to 13Y.
