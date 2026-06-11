# Wash sales — Part IVA and capital losses (TR 2008/1)

> **Sources:**
> - Taxation Ruling **TR 2008/1** *Income tax: application of Part IVA of the
>   Income Tax Assessment Act 1936 to 'wash sale' arrangements*,
>   https://www.ato.gov.au/law/view/print?DocID=TXR%2FTR20081%2FNAT%2FATO%2F00001&PiT=99991231235958
> - ATO media release **"Wash sales: The ATO is cleaning up dirty laundry"**
>   (QC 69938, last updated 27 June 2022),
>   https://www.ato.gov.au/media-centre/wash-sales-the-ato-is-cleaning-up-dirty-laundry
>
> **Retrieved:** 2026-06-11
> The live ATO site is authoritative; this is a convenience mirror.
> Related: Taxpayer Alert TA 2008/7 (same subject, superseded in detail by the ruling).

## What a wash sale is

TR 2008/1 (paras 2–3): in commerce a wash sale is "the sale and purchase of the
same, or substantially the same, asset within a short period of time of each
other", so that "there is effectively no change in the economic exposure of the
owner to the asset". The ruling is concerned with arrangements where a taxpayer
disposes of, or otherwise deals with, a CGT asset **with no significant change
in economic exposure** (or where the exposure may be reinstated), **in order to
apply the resulting capital loss** (or allowable deduction) against a capital
gain or assessable income already derived or expected.

Fact patterns the ruling lists (para 4) include — alongside trust/associate
transfers and derivative substitutes — the two this project's data can see:

- (a) the taxpayer disposes of the asset and **at the same time, or within a
  short period after, acquires the same or substantially the same asset**;
- (b) **shortly prior to**, or at the time of, disposing of the asset the
  taxpayer acquires the same, or substantially the same, asset.

"Substantially the same" (para 6) means economically equivalent or fungible
with the original asset; shares in a *different* company in the same industry
are **not** substantially the same. (This project flags only re-acquisitions of
the **same listing** — the only case determinable from its data.)

## The consequence

There is **no fixed statutory wash-sale rule in Australia** (unlike the US
30-day rule). Instead the Commissioner may apply **Part IVA** (the general
anti-avoidance provision) where, weighing the eight s 177D factors, the
**sole or dominant purpose** of the sale-and-repurchase was obtaining the tax
benefit of the capital loss. If Part IVA applies, the Commissioner cancels the
tax benefit — the capital loss is denied (s 177F), and the 2022 media release
adds that penalties and interest may follow ("When the ATO identifies this
behaviour, the capital loss is rejected").

Because the test is purposive, **no particular interval between sale and
repurchase is safe or unsafe per se**:

- **Example 1** (paras 18–25): concurrent same-day sell/buy of the same shares,
  engineered to offset a land gain — Part IVA applies.
- **Example 2** (paras 26–34): sale and repurchase **24 hours apart**, planned
  from an end-of-year tax-strategy booklet, no market-driven reason — Part IVA
  applies.
- **Example 6** (paras 61–67): sale of a volatile stock followed by repurchase
  **3 days later** that was explicable by genuine changes in market sentiment
  (independent commercial decisions to sell, then buy) — Part IVA does **not**
  apply.

Indicators pointing toward the dominant tax purpose (para 13) include: the
short period between disposal and acquisition; timing proximate to a derived
gain **or the end of the income year** and not to any market event; the
taxpayer's economic position being essentially unchanged apart from transaction
costs and the tax saved.

## ATO detection (2022 media release)

"Wash sales typically involve the disposal of assets such as crypto and shares
**just before the end of the financial year**, where after a short period of
time, the taxpayer reacquires the same or substantially similar assets." The
ATO's "sophisticated data analytics can identify wash sales through access to
data from **share registries and crypto asset exchanges**".

## How this maps to the project

The wash-sale alert report (`/reports/wash_sales`) surfaces the para 4(a)/(b)
fact pattern: every **loss-realising Sell** with a **Buy of the same listing
within a configurable window either side** (default 30 days — a review
convention, not an ATO bright line; the ruling has no statutory window),
across **all holding accounts** (a repurchase in a different account of the
same beneficial owner changes nothing economically). The report is
**non-blocking and advisory**: the pattern is not illegal per se — Example 6
shows commercially explicable round trips survive Part IVA — so writes are
never rejected and the flagged loss is still counted by every CGT report.
Whether Part IVA could apply is a facts-and-circumstances judgment for the
taxpayer/adviser; the report only makes the pattern visible instead of letting
it be discovered at audit time.
