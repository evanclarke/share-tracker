# Bonus shares

> **Source:** https://www.ato.gov.au/forms-and-instructions/capital-gains-tax-guide-2021/whats-new/investments-in-shares-and-units/bonus-shares
> (Guide to capital gains tax — "Bonus shares", QC 64895, last updated 20 July 2021)
> **Retrieved:** 2026-06-06
> The live ATO site is authoritative; this is a convenience mirror.

Bonus shares are additional shares a shareholder receives for an existing
holding of shares in a company. If you dispose of bonus shares received on or
after 20 September 1985, you may make a capital gain. You may also have to
modify the cost base and reduced cost base of your existing shares in the
company if you receive bonus shares.

The cost base and reduced cost base of bonus shares depend on whether the
bonus shares are **assessable as a dividend**.

As a result of changes to company and taxation laws, the paid-up value of
bonus shares is now generally **not** assessable as a dividend. An exception
to this rule is where you have the choice of being paid a cash dividend or of
being issued shares under a dividend reinvestment plan. These shares are
treated as dividends and the amount of the dividend is included in your
assessable income.

## Table 3: timing of the bonus share issue

| Date | Implications of timing of bonus shares |
| --- | --- |
| From 20 September 1985 to 30 June 1987 inclusive | Many bonus shares issued were paid out of a company's asset revaluation reserve or from a share premium account. These bonus shares are not usually assessable dividends. |
| From 1 July 1987 to 30 June 1998 inclusive | The paid-up value of bonus shares issued is assessed as a dividend unless paid from a share premium account. |
| From 1 July 1998 | The paid-up value of bonus shares issued is generally **not** assessed as a dividend unless you have the choice of being paid a dividend or being issued shares and you chose to be issued with shares. |

There are other, less common, circumstances where bonus shares will be
assessed as a dividend, for example, where:

- the bonus shares are being substituted for a dividend to give a tax advantage
- the company directs bonus shares to some shareholders and dividends to
  others to give them a tax benefit.

## Bonus shares issued where no amount is assessed as a dividend

### Original shares acquired on or after 20 September 1985

If your bonus shares relate to other shares that you acquired on or after
20 September 1985 (referred to as your original shares) your bonus shares are
taken to have been **acquired on the date you acquired your original shares**.
If you acquired your original shares at different times, you will have to work
out how many of your bonus shares are taken to have been acquired at each of
those times.

Calculate the cost base and reduced cost base of the bonus shares by
**apportioning the cost base and reduced cost base of the original shares over
both the original and the bonus shares**. Effectively, this results in a
reduction of the cost base and reduced cost base of the original shares. You
also include any calls paid on partly paid bonus shares as part of the cost
base and reduced cost base that is apportioned between the original and the
bonus shares.

### Original shares acquired before 20 September 1985

Your CGT obligations depend on when the bonus shares were issued and whether
they are fully paid or partly paid. (Pre-CGT assets — out of scope for this
project, which models post-CGT holdings only.)

### Example 35: Fully paid bonus shares

> Chris bought 100 shares in MAC Ltd for $1 each on 1 June 1985. He bought 300
> more shares for $1 each on 27 May 1986. On 15 November 1986, MAC Ltd issued
> Chris with 400 bonus shares from its capital profits reserve, fully paid to
> $1. Chris did not pay anything to acquire the bonus shares and no part of the
> value of the bonus shares was assessed as a dividend.
>
> For CGT purposes, the acquisition date of 100 of the bonus shares is
> 1 June 1985 (pre-CGT). Therefore, those bonus shares are not subject to CGT.
>
> The acquisition date of the other 300 bonus shares is 27 May 1986. Their
> cost base is worked out by spreading the cost of the 300 shares Chris bought
> on that date over both those original shares and the remaining 300 bonus
> shares. As the 300 original shares cost $300, **the cost base of each share
> will now be 50 cents**.

### Example 36: Partly paid bonus shares

> Klaus owns 200 shares in MAC Ltd, which he bought on 31 October 1984, and
> 200 shares in PUP Ltd, which he bought on 31 January 1985.
>
> On 1 January 1987, both MAC Ltd and PUP Ltd made their shareholders a
> one-for-one bonus share offer of $1 shares partly paid to 50 cents. Klaus
> elected to accept the offer and acquired 200 new partly paid shares in each
> company. No part of the value of the bonus shares was taxed as a dividend.
>
> On 1 April 1989, PUP Ltd made a call for the balance of 50 cents outstanding
> on the partly paid shares, payable on 30 June 1989. Klaus paid the call
> payment on that date. MAC Ltd has not yet made any calls on its partly paid
> shares.
>
> For CGT purposes, Klaus is treated as having acquired his bonus PUP Ltd
> shares on the date he became liable to pay the call (1 April 1989). The cost
> base of the bonus shares in PUP Ltd includes the amount of the call payment
> (50 cents) plus the market value of the shares immediately before the call
> was made.
>
> The MAC Ltd bonus shares will continue to have the same acquisition date as
> the original shares (31 October 1984) and are therefore not subject to CGT.
> However, this will not be the case if Klaus makes any more payments to the
> company on calls made by the company for any part of the unpaid amount on
> the bonus shares. In this case, the acquisition date of the bonus shares
> will be when the liability to pay the call arises and the bonus shares will
> then be subject to CGT.

## Bonus shares issued where the paid-up value is assessed as a dividend

If the paid-up value of bonus shares is assessed as a dividend, you may have
to pay CGT when you dispose of the bonus shares, regardless of when you
acquired the original shares.

### Original shares acquired on or after 20 September 1985

If your bonus shares relate to original shares that you acquired on or after
20 September 1985, the **acquisition date of the bonus shares is the date they
were issued**. Their cost base and reduced cost base includes the **amount of
the dividend**, plus any call payments you made to the company if they were
only partly paid.

*Exception – bonus shares received before 1 July 1987:* they are taken to be
acquired on the date you acquired your original shares; their cost base is
calculated as if the amount was not taxed as a dividend.

### Example 37: Cost base of bonus shares

> Mark owns 1,000 shares in RIM Ltd, which he bought on 30 September 1984 for
> $1 each.
>
> On 1 February 1997, the company issued him with 500 bonus shares partly paid
> to 50 cents. The paid-up value of bonus shares ($250) is an assessable
> dividend to Mark.
>
> On 1 May 1997, the company made a call for the 50 cents outstanding on each
> bonus share, which Mark paid on 1 July 1997.
>
> The total cost base of the bonus shares is $500, consisting of the $250
> dividend received on the issue of the bonus shares on 1 February 1997 plus
> the $250 call payment made on 1 July 1997.
>
> The bonus shares were acquired on 1 February 1997.

## How this project models bonus shares

- **Non-assessable bonus issue** (the general post-1 July 1998 case): the
  `BonusIssue` corporate action — on the issue date, every `bonus_held_units`
  units held receive `bonus_units` additional units. Per parcel this is the
  quantity re-base `(held + bonus) / held` with total cost base and the
  original acquisition date preserved — exactly the ATO apportionment above
  (Example 35: 300 @ $1 + 300 bonus → 50c each), and the same mechanics as a
  `ShareSplit` (TD 2000/10, [`share-splits-and-consolidations.md`](share-splits-and-consolidations.md)),
  so the two action types share the quantity re-basing machinery.
- **Dividend-assessed bonus shares** (shares chosen in lieu of a dividend —
  a bonus share plan / DRP): already representable as a distribution plus a
  DRP reinvestment trade — the new parcel is acquired on the issue date with
  cost base equal to the dividend amount, matching the rule above. See
  [`cgt-dividend-reinvestment-plans.md`](cgt-dividend-reinvestment-plans.md).
- **Partly paid bonus shares / call payments** (Examples 36–37) and pre-CGT
  originals are not modelled.
