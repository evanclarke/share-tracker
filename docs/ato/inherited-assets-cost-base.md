# Inherited assets — cost base and CGT

> **Source:**
> https://www.ato.gov.au/individuals-and-families/investments-and-assets/capital-gains-tax/inherited-assets-and-capital-gains-tax/cost-base-of-inherited-assets
> ("Cost base of inherited assets", QC 66053, last updated 22 June 2026), part of
> https://www.ato.gov.au/individuals-and-families/investments-and-assets/capital-gains-tax/inherited-assets-and-capital-gains-tax
> ("Inherited assets and capital gains tax", QC 66052)
> **Retrieved:** 2026-06-10; **re-fetched and expanded 2026-08-18** (the page had
> been updated: the Maria example's dates rolled forward, and a *Legal costs
> incurred by a legal personal representative* section with two examples was
> added; the earlier capture also summarised several rules that are now quoted).
> The live ATO site is authoritative; this is a convenience mirror.

Inheriting an asset is not itself a CGT event for the beneficiary — the legal
personal representative (LPR) disregards any capital gain or loss on
transferring an asset to a beneficiary. CGT applies when the **beneficiary
later disposes** of the asset, using the cost base rules below.

## Asset acquired by deceased before 20 September 1985

> If the deceased acquired the asset before 20 September 1985, it was a pre-CGT
> asset while they owned it. The first element of your cost base is the market
> value of the asset on the day the deceased died.
>
> If the deceased made a major improvement to the asset on or after
> 20 September 1985, the improvement isn't treated as a separate asset. You are
> taken to have acquired a single asset.
>
> The cost base of this single asset is the market value of the asset,
> including the improvement, on the day the deceased died.

## Asset acquired by deceased on or after 20 September 1985

> If the deceased acquired the asset on or after 20 September 1985, the first
> element of your cost base is generally what the deceased's cost base for the
> asset was on the day they died.
>
> The first element of your cost base is the market value of the asset on the
> day the deceased died if the asset either:
>
> - is a property that passed to you after 20 August 1996 (but not as a joint
>   tenant), and just before the deceased died it was their main residence and
>   was not being used to produce income
> - passed to you as the trustee of a special disability trust.

(Neither market-value exception can apply to listed shares.)

## Expenses the beneficiary includes in the cost base

> As a beneficiary, you can include in your cost base (and reduced cost base)
> any expenditure a legal personal representative (LPR) would have included in
> their cost base if they had sold the asset instead of distributing it to you.
>
> You include the expenditure on the date the LPR incurred it.

> **Example: transfer of an asset from executor (LPR) to beneficiary**
>
> Maria died on 13 October 2025 leaving 2 assets:
>
> - a parcel of 2,000 shares
> - a vacant block of land.
>
> The executor of the estate:
>
> - disregarded any capital gain or loss on the transfer of the assets
> - sold the shares to pay Maria's outstanding debts
> - transferred the land to Maria's beneficiary, Antonio, and paid the
>   conveyancing fee of $5,000 upon payment of all debts and tax.
>
> The shares were not transferred to a beneficiary. Therefore, the executor
> must include any capital gain or loss on this disposal in the tax return for
> Maria's deceased estate.
>
> The land was transferred to a beneficiary. Any capital gain or loss on this
> transfer is disregarded by the LPR.
>
> The first element of Antonio's cost base is Maria's cost base on the date of
> her death. Antonio can include the $5,000 the executor spent on the
> conveyancing in his cost base.

## Legal costs incurred by a legal personal representative

> As the LPR, in some circumstances, legal costs you incur may form part of the
> cost base of the estate's assets.
>
> For example, if a LPR incurs costs to confirm the validity of the deceased's
> will or defend a claim for control of the estate, these costs form part of
> the cost base of the estate's assets.

> **Example: legal costs incurred to prove the validity of a will**
>
> Annie is the executor (LPR) of a deceased estate.
>
> The deceased had more than one will prepared prior to their death:
>
> - The final will left the estate's assets to Max.
> - Prior wills had left the estate's assets to family members.
>
> The family members challenged the validity of the deceased's will in Court.
> As a result, Annie incurred legal costs on behalf of the deceased estate to
> defend this action.
>
> The Court held that the final will was valid and granted probate.
>
> The legal costs that Annie incurred to confirm the validity of the will and
> obtain probate were incurred to preserve or defend the rights over the
> estate's assets.
>
> Annie can't claim a deduction for these costs in her capacity as LPR as they
> are capital in nature. She can, however, include these legal costs in the
> cost base of the estate's assets.

> However, not all costs incurred by a LPR having a connection to estate assets
> will form part of the cost base of the estate's assets.

> **Example: legal costs incurred prior to the deceased's death**
>
> Cassie is the executor (LPR) of a deceased estate.
>
> Shortly prior to and in anticipation of the deceased's death, Cassie acted as
> the solicitor for the deceased.
>
> Cassie prepared an agreement for the transfer of interests in an asset of the
> deceased.
>
> These actions were undertaken by Cassie prior to the deceased's death and the
> start of Cassie's duties as the LPR of the estate.
>
> Any charges for Cassie's solicitor services prior to the deceased's death
> can't be included in the cost base of the estate's assets. Cassie's solicitor
> charges for the administration of the estate once she starts her duties as
> the LPR can be included in the cost base of the estate assets.

## Indexing the cost base of an inherited asset

> If the deceased died before 21 September 1999, you have the option of
> indexing the cost base when you dispose of the asset. Alternatively, you can
> claim the CGT discount. Usually, the discount will give you a better result.
>
> With indexation, you calculate your capital gain by using the first element
> of the asset's cost base indexed for inflation up until 21 September 1999.
> You don't apply the discount.
>
> If the deceased died on or after 21 September 1999, you can't use indexation.
> If the deceased's cost base includes indexation, you must recalculate the
> first element of your cost base to exclude it.

## Why this matters for this project

Shares inherited from a deceased estate enter the portfolio as parcels whose
**first-element cost base is not what was paid on any market** — it is the
deceased's cost base at death (post-CGT assets) or market value at death
(pre-CGT assets), with the acquisition-date rules in s 115-30 (mirrored in
[`inherited-assets-cgt-discount.md`](inherited-assets-cgt-discount.md))
governing the 12-month discount clock. Modelled as the `inheritances` entity:
its entry path records which rule produced the figure plus any LPR
expenditure, and creates the provenance-linked parcel Buy.

Two consequences for what the user types into the `cost_base` field, neither
of which any stored fact can check (see `docs/API.md`, Inheritances):

- **Indexation must be recalculated out** of a deceased's cost base where the
  death was on or after 21 September 1999. A deceased who acquired before
  21 September 1999 may have been carrying an indexed cost base, and copying
  that figure off the estate's records overstates the beneficiary's cost base.
- **The LPR-expenditure test is what the LPR incurred administering the
  estate**, per the Annie/Cassie examples: probate and will-validity costs are
  in, anything the same solicitor billed *before* the death is out. The
  `lpr_expenditure_date` this project requires with the figure is the ATO's own
  "on the date the LPR incurred it".
