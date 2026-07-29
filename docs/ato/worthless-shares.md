# Worthless / delisted shares — capital loss without a sale (CGT events G3 and C2)

> **Sources:**
> - https://www.ato.gov.au/individuals-and-families/investments-and-assets/capital-gains-tax/shares-and-similar-investments/investments-in-a-company-in-liquidation-or-administration (Investments in a company in liquidation or administration, QC 52234)
> - https://www.ato.gov.au/law/view/print?DocID=TXD%2FTD200052%2FNAT%2FATO%2F00001&PiT=99991231235958 (TD 2000/52 — CGT event G3, liquidator's declaration)
> - https://www.ato.gov.au/law/view/print?DocID=TXD%2FTD20007%2FNAT%2FATO%2F00001&PiT=99991231235958 (TD 2000/7 — CGT event C2 on deregistration)
> **Retrieved:** 2026-06-08 (QC 52234 example re-verified and refreshed 2026-07-29,
> when the ATO rolled its illustrative years forward one year; last updated 22 June 2026)
> The live ATO site is authoritative; this is a convenience mirror.

## What this covers

A failed company's shares can become worthless without ever being sold on a
market — the company goes into liquidation or administration, is delisted, and
is eventually deregistered. The capital loss must be recognisable without an
ordinary disposal, otherwise the dead parcel stays open forever and the loss
never reaches the gains reports. Two CGT events deliver this:

- **CGT event G3** (s 104-145; **TD 2000/52**): a liquidator or administrator
  makes a **written declaration** that they have reasonable grounds to believe
  there is **no likelihood** that shareholders will receive any further
  distribution. The shareholder may then **choose** to make a capital loss
  equal to the **reduced cost base** of the shares at the time of the
  declaration. The cost base and reduced cost base are then **reset to nil**
  just after the declaration (relevant if a later CGT event happens to the
  still-held shares).
- **CGT event C2** (s 104-25; **TD 2000/7**): the company is **deregistered**
  under the Corporations Law and ceases to exist. A CGT event (usually C2)
  happens to the members' shares on the deregistration date — an actual
  cancellation/disposal, normally at **nil capital proceeds**, so the capital
  loss again equals the reduced cost base.

In both cases the result is the same arithmetic: a capital loss equal to the
parcel's **remaining reduced cost base** (cost base after any AMIT and
return-of-capital reductions; i.e. the cost base under the elements 1–2
limitation this system models). It is **always a capital loss, never income**,
and a **capital loss is never discounted** — no 12-month / discount-eligibility
handling applies. The loss flows through the ordinary loss-netting and
carry-forward machinery (see `cgt-using-capital-losses.md`).

## Key rules

- **Opt-in (G3).** The choice to crystallise the loss rests with the
  shareholder; it is made by the amounts shown at the CGT question on the
  return for the year of the declaration. The decision to make a *declaration*,
  and when, rests solely with the liquidator/administrator.
- **Eligibility (G3).** Australian resident; the share/financial instrument was
  acquired after 19 September 1985; a written declaration of worthlessness was
  made; and any gain/loss on the asset is a capital gain/loss (held as an
  investment — not trading stock, not part of a business, not a short-term
  commercial gain). **ESS interests are excluded** — the ESS rules deal with the
  discount before these CGT rules apply.
- **Loss amount.** Capital loss = the **reduced cost base** of the shares at the
  declaration (G3) or cancellation (C2). Under G3 the cost base is then reset to
  nil.
- **Later recovery (informational, out of scope here).** If a payment is later
  received (e.g. a successful court recovery), and the company is dissolved
  **more than 18 months** after the payment, the payment is a capital gain in
  the year received; if dissolved **within 18 months**, the payment is capital
  proceeds on the eventual C2 cancellation. The system does not model the 18-
  month timing rule — a later recovery is entered as the user's own CGT event.

## Worked example — TD 2000/52 (timing of a valid declaration)

A liquidator declares on 25 May 2000 that shareholders will not receive a
distribution of *more than* 2.5% — this is **not** a valid s 104-145(1)
declaration (a distribution, however small, is still expected). On 1 August 2000
the liquidator distributes 1.5% and, having reasonable grounds to believe no
further distribution is likely, declares to that effect on 2 August 2000. Only
the 2 August declaration crystallises the loss; it is used in the 2000–01 income
year. (Timing rule — not reproduced as an acceptance test; the system records
the user-supplied declaration/cancellation date as the event date.)

## Worked example — Dave (capital loss when company dissolves) — reproduced in `src/ato_examples.rs`

On 31 March 2026, the administrators of Company Ltd made a written declaration
that they had reasonable grounds to believe there was no likelihood that
shareholders would receive any distribution. Dave owned 1,000 Company Ltd
shares, acquired in March 2013 for **$1.70 each including brokerage**. He chose
to claim the capital loss in his 2025–26 return.

The reduced cost base — and Dave's capital loss — is **1,000 × $1.70 = $1,700**.
That loss is taken into account in working out his net capital gain or loss for
2025–26.

This is the modelled case: a `WorthlessShares` corporate action (a
`G3Declaration`) whose recognise operation closes every open parcel held at the
declaration date through a provenance-marked Sell at **nil proceeds**, producing
a capital loss equal to each parcel's remaining reduced cost base. The loss
surfaces in the realised-gains report as `capital_loss` and feeds the
net-capital-gain report's loss pool (no discount).

## How this maps to the implementation

- Modelled as the **`WorthlessShares`** corporate action type (discriminator
  `worthless_event`: `G3Declaration` vs `C2Cancellation`, recording which CGT
  event the user is invoking; both produce the identical loss arithmetic). The
  action's `date` is the declaration date (G3) or the deregistration/
  cancellation date (C2).
- The recognise operation (`POST /corporate_actions/:id/recognise`,
  `entities::worthless`) reuses the closing-Sell machinery built for
  scrip-for-scrip and demergers, but — unlike those rollovers — the closing
  Sell is **not** excluded from the realised-gains report: its nil proceeds
  against each parcel's remaining reduced cost base **recognise** the loss.
- Out of scope (documented above): the G3 opt-in eligibility tests (the user's
  determination), the cost-base-reset-to-nil bookkeeping for shares still held
  after a G3 declaration (the operation closes the whole holding), worthless
  *financial instruments* other than shares, and the 18-month later-recovery
  timing rule.
