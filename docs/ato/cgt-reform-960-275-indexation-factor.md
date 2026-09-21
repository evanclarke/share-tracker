# Section 960-275, Income Tax Assessment Act 1997 — indexation factor

**Source:** ATO Legal database, *Income Tax Assessment Act 1997*, section 960-275 —
<https://www.ato.gov.au/law/view/print?DocID=PAC%2F19970038%2F960-275&PiT=99991231235958>
(print view, whole section as in force on and after 1 July 2027).
**Retrieved:** 2026-09-21.

Mirrored because the enacted section is what the post-1 July 2027 cost base indexation is
implemented against: the Explanatory Memorandum states the new factor (`docs/ato/cgt-reform-cgt-adjustments.md`,
EM 1.67–1.74) but not the rounding rule the section keeps for it, and the implementation must not
assume the pre-2027 rule carries across unstated. It does: subsection (5) is general — "You work out
the \*indexation factor to 3 decimal places" — and the Bill inserted subsections (1B) and (1C)
without amending it, so the new factors take the same 3-decimal-place rounding the frozen method's
does. `domain::cgt_indexation` implements it; `domain::indexation` keeps the frozen method's own.

Only the subsections this project reads are mirrored verbatim; the remaining subsections
(1)/(1A)/(2)/(3), which set the annual-indexation factors and the frozen pre-21-September-1999
method, are unchanged by the reform and are described in `docs/ato/indexing-the-cost-base.md` and
`docs/ato/consumer-price-index.md`.

---

## 960-275(1B)

For indexation under subsection 110-36(1A) of the \*cost base of a \*CGT asset (except the first
element of the cost base of an asset covered by subsection (3)), the **_indexation factor_** for
expenditure:

**(a)** in an element of the cost base; and

**(b)** incurred on or after 1 July 2027;

is:

| Index number for the quarter in which the \*CGT event happens |
| --- |
| Index number for the quarter in which the expenditure is incurred |

The expenditure can include giving property: see section 103-5.

**Note 1:** This includes expenditure taken to have been incurred on 1 July 2027 as mentioned in
paragraph 112-155(2)(b), 112-165(2)(b) or 112-175(2)(b).

**Note 2:** There are rules affecting when the expenditure was incurred: see Division 114.

## 960-275(1C)

For indexation under subsection 110-36(1A) of the first element of the \*cost base of a \*CGT asset
that is a \*share in a company, or unit in a unit trust, the **_indexation factor_** for an amount
in that first element that was paid to the company or trust at a time:

**(a)** after the asset was \*acquired; and

**(b)** on or after 1 July 2027;

is:

| Index number for the quarter in which the \*CGT event happens |
| --- |
| Index number for the quarter in which the amount is paid |

The payment can include giving property: see section 103-5.

> **Example:**
>
> Peter acquires shares in a company. The shares are partly-paid, and the company makes a call on
> the shares. Peter sells the shares to Narina before Peter is liable to pay the call.
>
> The amount Narina paid to Peter for the shares is indexed under subsection 960-275(1B) from the
> quarter in which she incurred the expenditure to acquire the shares.
>
> The amount Narina later pays for the call on the shares is indexed in accordance with this
> subsection from the quarter in which she made that later payment.

## 960-275(4)

However, you cannot index expenditure in the third element of the \*cost base of a CGT asset (costs
of ownership).

## 960-275(5)

You work out the \*indexation factor to 3 decimal places (rounding up if the fourth decimal place is
5 or more).

> **Example:**
>
> If the factor is 1.102795, it would be rounded up to 1.103.

---

## What this project takes from it

- **The factor** (1B)/(1C): event quarter ÷ expenditure quarter, from `current_cpi_quarters`
  (`domain::cgt_indexation::IndexationFactor`). This project's data model holds one expenditure date
  per parcel, so (1B) and (1C) coincide today; the separate first-element rule becomes reachable only
  with a call-payment fact, which does not exist (see the *Partly paid shares* Known limitation).
- **The range**: subsection (1B)(b) reaches only expenditure incurred on or after 1 July 2027, and
  Note 1 puts the deemed reacquisition's expenditure at 1 July 2027 — so the earliest quarter the
  factor can name is the one ending 30 September 2027 (EM 1.72). That is the lower bound of the
  `current_cpi_quarters` table (migration 0052) and of what the `cpi-import` job stores.
- **The third element** (4): never indexed. This project records no costs of owning an asset, so the
  rule is structural.
- **The rounding** (5): 3 decimal places, fourth decimal up from 5 — the rule
  `domain::cgt_indexation::indexation_factor` applies, and the one the clarification this file exists
  for settled.
