# Taxing retail premiums

> **Source:** https://www.ato.gov.au/individuals-and-families/investments-and-assets/investing-in-shares/owning-shares/taxing-retail-premiums
> (QC 21832, last updated 23 June 2025)
> **Retrieved:** 2026-06-10
> The live ATO site is authoritative; this is a convenience mirror.
> Underlying rulings: TR 2017/4 (renounceable rights offers, shares held on
> capital account) and TR 2012/1 (non-renounceable share entitlements).

## What is a retail premium?

A retail premium is:

- a payment made by companies to shareholders as a result of offers of
  entitlements or rights to existing shareholders
- paid to shareholders who don't take up the company's offers.

Retail premiums you receive may be taxed in different ways. The tax outcome
for shareholders depends in part on the nature of the offer. There are
**different treatments for renounceable and non-renounceable rights offers**.

## Non-participating shareholder

You are a non-participating shareholder if either of the following applies:

- you choose not to take up some or all of your entitlements
- you are not eligible to take up an entitlement.

The entitlements or rights that you did not take up, could not take up or did
not receive are called unexercised entitlements.

## Amount of retail premium

The retail premium is paid directly to you as a net amount either by cheque or
a direct credit. Generally, there are no incidental expenses. Not all offers
to subscribe for additional shares involve retail premiums.

## Tax and retail premiums

Renounceable rights offers include situations where the shareholder:

- can choose to take up the entitlement
- let the entitlement lapse
- trades them in the market.

Alternatively, where these conditions aren't met, the rights are considered to
be non-renounceable. These situations have differing tax outcomes for the
shareholders that receive retail premiums.

### Tax treatment for renounceable rights (TR 2017/4)

Generally, where individual retail investors hold shares on capital account
and a resident individual shareholder receives a retail premium, it will
constitute a **capital gain**.

**Australian resident shareholders** — a shareholder will make a capital gain
if the retail premium amount exceeds the **cost base of the entitlement,
generally being incidental costs**. A shareholder is **taken to have acquired
the rights when they acquired the original shares**. Therefore, any capital
gain may represent a **discount capital gain** if the eligible shareholder's
original shares have been held for 12 months or more.

**Retail premiums paid to shareholders are not dividends.** The same applies
to ineligible shareholders: the premium is capital proceeds, not a dividend.

For foreign resident individual shareholders not holding taxable Australian
property, the receipt of a retail premium amount won't be taxable.

### Tax treatment for non-renounceable rights (TR 2012/1)

A retail premium payment you receive is an **unfranked dividend**. If you are
a non-resident, the amount is non-assessable non-exempt income, subject to
withholding tax.

---

## How this maps to this project

The `RightsIssue` corporate action models a **renounceable** offer (the rights
can be exercised, sold, or left to lapse), so a retail premium received under
it is the TR 2017/4 capital treatment — economically identical to selling the
unexercised rights for the premium amount:

- enter it with the **sell-rights operation** (`POST
  /corporate_actions/:id/sell_rights`) using the premium as the proceeds. The
  disposal takes nil cost base (free rights) and anchors the CGT discount to
  the original parcel's acquisition date, exactly as this page describes.
- it is **not** dividend income — do not enter it as `income`.

A retail premium under a **non-renounceable** entitlement offer is the
opposite: an unfranked dividend (TR 2012/1) — enter it as unfranked dividend
`income` against the listing, not as a corporate action or rights sale.

An earlier revision of `rights-issues.md` described all retail premiums as
unfranked dividends; that reflected the pre-TR 2017/4 view and is superseded
by the split above.
