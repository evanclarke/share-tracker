# Taxing retail premiums

> **Source:** https://www.ato.gov.au/individuals-and-families/investments-and-assets/investing-in-shares/owning-shares/taxing-retail-premiums
> (QC 21832, last updated 22 June 2026)
> **Retrieved:** 2026-08-25 (re-fetched driving SCENARIOS AA-b; the previous
> capture was 2026-06-10, before the page's June 2026 update, which split the
> renounceable treatment into *Australian resident shareholders*, *Ineligible
> shareholders* and *Capital gains tax* subsections. The renounceable /
> non-renounceable split itself is unchanged.)
> The live ATO site is authoritative; this is a convenience mirror.
> Underlying rulings: TR 2017/4 (renounceable rights offers, shares held on
> capital account) and TR 2012/1 (non-renounceable share entitlements).

## What is a retail premium?

A retail premium is:

- a payment made by companies to shareholders as a result of offers of
  entitlements or rights to existing shareholders
- paid to shareholders who don't take up the company's offers.

Retail premiums you receive may be taxed in different ways.

The tax outcome for shareholders depends in part on the nature of the offer.
There are different treatments for renounceable and non-renounceable rights
offers.

## Non-participating shareholder

You are a non-participating shareholder if either of the following applies:

- you choose not to take up some or all of your entitlements
- you are not eligible to take up an entitlement.

The entitlements or rights that you didn't take up, couldn't take up or didn't
receive are called unexercised entitlements.

## Amount of retail premium

The retail premium is paid directly to you as a net amount either by cheque or
a direct credit. Generally, there are no incidental expenses. Not all offers to
subscribe for additional shares involve retail premiums.

## Tax and retail premiums

A retail premium payment you receive is taxed in different ways, depending on
whether it is renounceable or non-renounceable.

Renounceable rights offers include situations where the shareholder:

- can choose to take up the entitlement
- let the entitlement lapse
- trades them in the market.

Alternatively, where these conditions aren't met, the rights are considered to
be non-renounceable. These situations have differing tax outcomes for the
shareholders that receive retail premiums.

### Tax treatment for renounceable rights

Generally, where individual retail investors hold shares on capital account and
a resident individual shareholder receives a retail premium, it will constitute
a capital gain.

For foreign resident individual shareholders who are not holding investments
which are taxable Australian property, the receipt of a retail premium amount
won't be taxable.

For more information on the nature of renounceable rights and the tax outcomes
for retail shareholders, see Taxation Ruling TR 2017/4.

#### Australian resident shareholders

A shareholder will make a capital gain if the retail premium amount exceeds the
cost base of the entitlement, generally being incidental costs.

A shareholder is taken to have acquired the rights when they acquired the
original shares. Therefore, any capital gain may represent a discount capital
gain if the eligible shareholder's original shares have been held for 12 months
or more.

Retail premiums paid to shareholders are not dividends.

#### Ineligible shareholders

Retail premiums paid to ineligible shareholders are not dividends.

#### Capital gains tax

A shareholder will make a capital gain if the retail premium amount is more
than the cost base of the entitlement, generally being incidental costs.

Capital gains tax will be disregarded where the shares held are not taxable
Australian assets, for example where the company doesn't own any Australian
real property.

### Tax treatment for non-renounceable rights

A retail premium payment you receive is an unfranked dividend. If you are a
non-resident, the amount is:

- non-assessable non-exempt income
- subject to withholding tax.

For more information, see Taxation Ruling TR 2012/1 *Income tax: retail
premiums paid to shareholders where share entitlements are not taken up or are
not available*.

---

## TR 2012/1, on what a non-renounceable offer is

> **Source:** https://www.ato.gov.au/law/view/print?DocID=TXR%2FTR20121%2FNAT%2FATO%2F00001&PiT=99991231235958
> **Retrieved:** 2026-08-25. Quoted here because the definition and the
> character of the payment are what this system enforces.

The Ruling applies to schemes with, among others, these features (para 2):

> - A company grants rights (Entitlements) to its existing shareholders
>   (subject to their eligibility) that allow them to subscribe for an
>   allotment of new shares in the company at an amount, often called the
>   'Offer Price';
> - **The Entitlements cannot be traded, transferred, assigned or otherwise
>   dealt with by the shareholder or on behalf of the shareholder or anyone
>   else;**
> - Shareholders can choose not to exercise some or all of their Entitlements
>   to an offered allotment (which Entitlements lapse if not exercised), or may
>   not be eligible to receive an Entitlement or may not be permitted to
>   exercise rights under it.

It does **not** consider arrangements where "Entitlements are assignable by,
tradeable by, or given to a nominee entity for disposal on behalf of, the Non
Participating Shareholders entitled to them", nor entitlement offers to issue
equity in trusts or in stapled groups (para 3).

The Ruling itself:

> 4. A Retail Premium paid to a Non Participating Shareholder is a dividend
>    that is included in assessable income under section 44 of the ITAA 1936,
>    unless the Retail Premium is non-assessable non-exempt income.
>
> 6. A Retail Premium paid to a Non Participating Shareholder is an unfrankable
>    distribution sourced, directly or indirectly, from a company's share
>    capital account pursuant to paragraph 202-45(e) of the ITAA 1997.
>
> 8. In the alternative, a Retail Premium paid to a Non Participating
>    Shareholder is ordinary income that is assessable income under section 6-5
>    of the ITAA 1997 if the Retail Premium is not a dividend.
>
> 9. A CGT asset, being a right, comes into existence when a Non Participating
>    Shareholder becomes entitled to a Retail Premium.
>
> 10. When the Retail Premium is paid to the Non Participating Shareholder, CGT
>     event C2 under section 104-25 of the ITAA 1997 happens.
>
> 11. Any capital gain a Non Participating Shareholder makes from receipt of
>     the Retail Premium is reduced under section 118-20 of the ITAA 1997 to the
>     extent that the amount is otherwise included in the Non Participating
>     Shareholder's assessable income (under section 44 of the ITAA 1936, or
>     alternatively section 6-5 of the ITAA 1997), or is non-assessable
>     non-exempt income (under section 128D of the ITAA 1936).

So the non-renounceable premium is **not** partly capital: a C2 event does
happen on the right to the premium, and the anti-overlap rule in s 118-20
reduces the gain by the amount already assessed as income — to nil where the
whole premium is assessable.

---

## How this maps to this project

A `RightsIssue` corporate action records **`renounceable`** (migration 0047),
and it is required at entry rather than assumed — the two treatments turn
entirely on it:

- **Renounceable** (the rights can be taken up, let lapse, or traded): a retail
  premium is the TR 2017/4 capital treatment — economically identical to
  selling the unexercised rights for the premium amount. Enter it with the
  **sell-rights operation** (`POST /corporate_actions/:id/sell_rights`) using
  the premium as the proceeds. The disposal takes nil cost base (free rights)
  and anchors the CGT discount to the original parcel's acquisition date,
  exactly as this page describes. It is **not** dividend income — do not enter
  it as `income`.
- **Non-renounceable**: the opposite. The premium is an unfranked dividend (TR
  2012/1) — enter it as unfranked dividend `income` against the listing, not as
  a corporate action or rights sale. The sell-rights operation **refuses** any
  positive proceeds (or any cost paid to acquire the rights) against such an
  action with a `422` naming this ruling and the income path: entitlements that
  "cannot be traded, transferred or assigned" can neither be sold nor bought.
  A **lapse** — nil proceeds, nil cost — is still recorded there, which is what
  consumes the entitlement.

**Exercising** is identical under both offers and is unaffected by the flag: the
exercise rules turn on how the rights were acquired and on whether the original
shares are pre- or post-CGT (see [`rights-issues.md`](rights-issues.md)), never
on renounceability. Recording a non-renounceable issue in order to exercise it
is therefore the normal case, not a workaround.

Out of scope, from TR 2012/1's own para 3: entitlement offers over **units in a
trust or a stapled group**, which the Ruling does not consider. The refusal
above still applies to one recorded here (the entitlements are non-tradeable
either way), but the character of the payment is then whatever the
distribution statement says it is, not automatically an unfranked dividend.

An earlier revision of `rights-issues.md` described all retail premiums as
unfranked dividends; that reflected the pre-TR 2017/4 view and is superseded by
the split above.
