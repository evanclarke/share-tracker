# Inherited assets — cost base and CGT

> **Source:**
> https://www.ato.gov.au/individuals-and-families/investments-and-assets/capital-gains-tax/inherited-assets-and-capital-gains-tax/cost-base-of-inherited-assets
> ("Cost base of inherited assets", QC 66053, last updated 23 June 2025), part of
> https://www.ato.gov.au/individuals-and-families/investments-and-assets/capital-gains-tax/inherited-assets-and-capital-gains-tax
> ("Inherited assets and capital gains tax", QC 66052)
> **Retrieved:** 2026-06-10
> The live ATO site is authoritative; this is a convenience mirror.

Inheriting an asset is not itself a CGT event for the beneficiary — the legal
personal representative (LPR) disregards any capital gain or loss on
transferring an asset to a beneficiary. CGT applies when the **beneficiary
later disposes** of the asset, using the cost base rules below.

## Asset acquired by the deceased before 20 September 1985 (pre-CGT)

The asset was a pre-CGT asset while the deceased owned it. The first element
of the beneficiary's cost base is the **market value of the asset on the day
the deceased died**.

## Asset acquired by the deceased on or after 20 September 1985

The first element of the beneficiary's cost base is generally **the
deceased's cost base for the asset on the day they died** (the cost base
carries over; special market-value rules exist for a main residence and
special disability trusts — not relevant to shares).

## Expenses the beneficiary can include

The beneficiary can include in their cost base (and reduced cost base) any
expenditure the LPR would have included had the LPR sold the asset instead of
distributing it (e.g. conveyancing on transfer; certain legal costs of
proving the will), included on the date the LPR incurred it.

Example (Maria/Antonio): shares sold by the executor to pay debts stay in the
**estate's** return; the land transferred to the beneficiary carries Maria's
cost base at death plus the executor's $5,000 conveyancing fee.

## Indexation

If the deceased died **before 21 September 1999**, the beneficiary may index
the first element of the cost base for inflation up to 21 September 1999
instead of claiming the CGT discount (usually the discount is better). If the
deceased died on or after that date, indexation is unavailable, and any
indexation inside the deceased's cost base must be recalculated out.

## Why this matters for this project

Shares inherited from a deceased estate enter the portfolio as parcels whose
**first-element cost base is not what was paid on any market** — it is the
deceased's cost base at death (post-CGT assets) or market value at death
(pre-CGT assets), with the acquisition-date rules in s 115-30 (mirrored in
[`inherited-assets-cgt-discount.md`](inherited-assets-cgt-discount.md))
governing the 12-month discount clock. Modelled as the `inheritances` entity:
its entry path records which rule produced the figure plus any LPR
expenditure, and creates the provenance-linked parcel Buy.
