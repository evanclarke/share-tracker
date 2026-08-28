# Share splits and consolidations (TD 2000/10)

> Source: https://www.ato.gov.au/law/view/print?DocID=TXD%2FTD200010%2FNAT%2FATO%2F00001&PiT=99991231235958
> (Taxation Determination TD 2000/10, legally binding public ruling)
> Retrieved: 2026-06-06

**Taxation Determination TD 2000/10** — Income tax: capital gains: what are the
CGT consequences for a shareholder if a company converts its shares into a
larger (a *share split*) or smaller (a *share consolidation*) number of shares?

## The determination

1. If a company converts its shares into a larger or smaller number of shares
   ("the converted shares") in accordance with section 254H of the Corporations
   Law in that:
   - (a) the original shares are not cancelled or redeemed;
   - (b) there is no change in the total amount allocated to the share capital
     account of the company; and
   - (c) the proportion of equity owned by each shareholder in the share
     capital account is maintained;

   **no CGT event happens** to the shareholder's original shares. While there
   is a change in the form of the original shares, there is no change in their
   beneficial ownership. Roll-over relief under section 124-240 ITAA 1997 does
   not arise because no CGT event happens to the shares.

2. **The converted shares have the same date of acquisition as the original
   shares to which they relate.** For example, if the original shares were
   acquired before 20 September 1985 (pre-CGT shares), the converted shares
   have the same acquisition date.

3. For post-CGT shares, **section 112-25 ITAA 1997 applies to attribute a
   proportionate cost base to the converted shares** — the parcel's total cost
   base is unchanged and is spread over the new number of shares.

4. Cancelling original share certificates and replacing them with new
   certificates as part of the conversion does not change this result, unless
   the original shares are in fact cancelled or redeemed under the Corporations
   Law (in which case CGT event C2 happens and roll-over relief under
   section 124-240 may be available). That cancellation/redemption case is
   **not** what this project's ShareSplit corporate action models.

## Example 1 — share split (2-for-1)

XYZ Ltd's share capital account of \$100,000 consists of 100,000 shares. The
company converts its share capital into 200,000 ordinary shares on 1 July 1992.
The original shares are not cancelled or redeemed, the share capital account is
unaltered, and each shareholder's proportion of equity is maintained.

John acquired 2,000 ordinary shares in XYZ Ltd in September 1984 (pre-CGT) and
3,000 ordinary shares on 30 April 1988. Before the conversion, the 1988 shares
had a cost base of **\$1.00 each**.

On conversion, no CGT event happens to any of John's original shares. John now
has **4,000** ordinary shares with an acquisition date before 20 September 1985,
and **6,000** ordinary shares with a cost base of **\$0.50 each** and an
acquisition date of **30 April 1988**.

## Example 2 — share consolidation (1-for-2)

If XYZ Ltd instead converts its original share capital into 50,000 ordinary
shares (all other facts unchanged), no CGT event happens to John's original
shares. John would now have **1,000** ordinary shares with a pre-CGT
acquisition date, and **1,500** ordinary shares with a cost base of **\$2.00
each** and an acquisition date of 30 April 1988.

## How this maps to this project

- A split/consolidation is recorded as a **ShareSplit corporate action** on the
  listing: on the conversion `date`, every `split_old_units` units held become
  `split_new_units` units (a 2-for-1 split is new=2/old=1; a 1-for-10
  consolidation is new=1/old=10).
- **No CGT event**: the action creates no gain/loss and no new parcel. Parcels
  acquired before the conversion date keep their trade row (quantities as
  originally transacted); reports re-base those quantities to the unit basis of
  the date being reported and spread the unchanged total cost base over the
  converted number of units (the per-unit cost base scales inversely).
- **Acquisition date preserved**: the 12-month CGT-discount clock keeps running
  from the original acquisition date of the converted parcel.
- Trades dated on or after a conversion date are already expressed in
  post-conversion units (you can only transact the units that exist on the
  day), so a Sell entered after a split allocates against pre-split parcels by
  re-basing — e.g. a 100-share pre-split parcel covers a 200-share post-split
  sale after a 2-for-1 split.
- Fractional entitlements from a consolidation that does not divide a holding
  evenly are kept exact as decimal quantities; company-specific rounding or
  cash-in-lieu arrangements are not modelled.
