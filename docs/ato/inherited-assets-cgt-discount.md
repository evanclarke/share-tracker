# Inherited assets — CGT discount clock (s 115-30)

> **Source:**
> https://www.ato.gov.au/individuals-and-families/investments-and-assets/capital-gains-tax/inherited-assets-and-capital-gains-tax/how-cgt-applies-to-inherited-assets
> ("How CGT applies to inherited assets", QC 69713, last updated 6 November 2025), part of
> https://www.ato.gov.au/individuals-and-families/investments-and-assets/capital-gains-tax/inherited-assets-and-capital-gains-tax
> ("Inherited assets and capital gains tax", QC 66052)
> **Retrieved:** 2026-06-10
> The live ATO site is authoritative; this is a convenience mirror.

## The 12-month discount clock for an inherited asset

Australian resident individuals, trusts and super funds can use the CGT
discount to reduce a capital gain on assets owned for 12 months or more.
For the purposes of qualifying for the CGT discount, the beneficiary can
treat an inherited asset as though they have owned it since:

- **the deceased acquired the asset**, if the deceased acquired it on or
  after 20 September 1985 (a post-CGT asset in the deceased's hands), or
- **the deceased died**, if the deceased acquired the asset before
  20 September 1985 (a pre-CGT asset in the deceased's hands).

(This is the ATO's statement of the acquisition-time rule in s 115-30 of the
ITAA 1997 for assets acquired as the beneficiary of a deceased estate.)

## Indexation alternative

If the deceased died **before 21 September 1999**, the beneficiary may
instead index the cost base for inflation up to 21 September 1999; using
indexation, the asset is taken to have been acquired when the deceased
acquired it. (Indexation is not modelled in this project — the 50% discount
is used throughout.)

## Other points from the same page

- A beneficiary inheriting an asset (under a will, intestacy, distribution,
  or a deed of arrangement) has no CGT implications at that time; the LPR
  disregards any gain or loss on an asset that passes to a beneficiary. CGT
  happens when the **beneficiary later disposes** of the asset.
- The deceased's unapplied net capital losses do **not** transfer to the
  beneficiary or LPR.
- Assets passing to a foreign resident, a tax-exempt entity, or a complying
  super fund instead trigger CGT in the deceased's date-of-death return
  (not modelled — the estate side is out of scope).

## Why this matters for this project

Together with the cost-base rules in
[`inherited-assets-cost-base.md`](inherited-assets-cost-base.md): a post-CGT
inherited parcel's discount period runs from the **deceased's acquisition
date** (so the parcel carries a deemed acquisition date), while a pre-CGT
parcel's runs from the **date of death** (the parcel's own date). The
inheritance entry path sets `trades.deemed_acquisition_date` accordingly, and
the realised-gains / net-capital-gain reports' existing 12-month test does the
rest.
