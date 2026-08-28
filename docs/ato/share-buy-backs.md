# Share buy-backs

> **Source:**
> https://www.ato.gov.au/individuals-and-families/investments-and-assets/capital-gains-tax/shares-and-similar-investments/share-buy-backs
> ("Share buy-backs", QC 66049, last updated 23 June 2025)
> **Retrieved:** 2026-06-06
> The live ATO site is authoritative; this is a convenience mirror.

How your tax is affected if you sell your shares back to the company.

## Effect on capital gains tax

If you dispose of shares you hold on capital account back to the company, it
is a **capital gains tax (CGT) event**. This means you must:

- calculate your capital gain or loss by subtracting the cost of the shares
  from your capital proceeds
- report your capital gain or loss in your tax return.

If it's an off-market buy-back arrangement, your capital proceeds may be based
on the **market value** of the shares, rather than the amount you receive.

## Time of capital gain or loss

The point at which you make a capital gain or loss depends on the conditions
of the buy-back offer. For example, it may be the time the company accepts
your application to participate in the buy-back or, if it's a conditional
offer of buy-back, the time you accept the offer.

You report your capital gain or loss in your tax return for the year in which
the CGT event happens.

## Capital proceeds from an off-market share buy-back

An off-market share buy-back is when a company offers to buy its shares back
from you directly, rather than buying them through a stock exchange in the
open market. Usually the company writes to you with the offer.

For CGT purposes, your capital proceeds **can't be less than what the market
value of your shares would have been if the buy-back had not been proposed**.

### Listed public company

Any off-market share buy-back announced by listed public companies **after
7:30 pm AEDT on 25 October 2022** will **not contain a dividend component**
in the price. The entire buy-back price will be treated as capital proceeds.

### Company that is not a listed public company

If the buy-back price is **equal to or more than** the market value, your
capital proceeds are the amount paid, **excluding any dividend** paid as part
of the buy-back.

If the buy-back price is **less than** the market value, your capital
proceeds are:

- what the market value of your shares would have been if the buy-back had
  not been proposed
- **less** any dividend paid under the buy-back.

In this situation, the company may tell you the market value or obtain a
class ruling from the ATO.

There are some further adjustments in circumstances where the participating
shareholder is itself a company.

Where a share buy-back affects a large shareholder group, the ATO may publish
a class ruling setting out the tax consequences for shareholders.

## Example: off-market buy-back

Ranjini bought 10,000 shares in a company that was not a listed public
company at a cost of \$6 per share, including brokerage.

A few years later, the company wrote to its shareholders offering to buy back
10% of their shares for \$9.60 each. The buy-back price included a **franked
dividend of \$1.40 per share**, with each dividend to carry a **franking
credit of \$0.60**.

Ranjini applied to participate in the buy-back to sell 1,000 of her shares.
The company approved the buy-back on the same terms as its earlier letter of
offer.

The **market value** of the company's shares at the time of the buy-back,
assuming the buy-back had not been proposed, was **\$10.20**.

Ranjini received a cheque for \$9,600 (1,000 shares × \$9.60 = \$9,600).

Ranjini must work out her capital gain using the market value of the shares
because:

- it is an off-market share buy-back
- the buy-back price is less than what the market value of the shares would
  have been if the buy-back had not been proposed.

Ranjini works out her capital gain as follows:

| Step | Amount |
| --- | --- |
| Market value of shares: \$10.20 × 1,000 | \$10,200 |
| Dividend: \$1.40 × 1,000 | \$1,400 |
| Capital proceeds: \$10,200 − \$1,400 | \$8,800 |
| Cost base: \$6.00 × 1,000 | \$6,000 |
| Capital gain (before applying any discount): \$8,800 − \$6,000 | **\$2,800** |

Ranjini must report her capital gain as well as her **dividend of \$1,400 and
franking credit of \$600** in her tax return.

## Further references

- **TD 2004/22** — for off-market share buy-backs of listed shares, what is
  the market value of the share for the purposes of subsection 159GZZZQ(2) of
  the ITAA 1936.
- **PS LA 2007/9** — Share buy-backs.

## How this project models it

A `BuyBack` corporate action records the offer terms per listing: the
per-unit buy-back price, the per-unit dividend component and its franking
credit (both zero for a listed-company buy-back announced after 25 October
2022), the per-unit market value had the buy-back not been proposed (optional
— omitted when the buy-back price is at or above market value), and the
currency. Participating (`POST /corporate_actions/:id/participate`)
atomically creates:

- a **Sell trade** for the bought-back units with the chosen parcel
  allocations (specific identification applies as for any disposal); its
  per-unit price is the capital proceeds per unit =
  `max(buy-back price, market value) − dividend`, so the realised-gains and
  net-capital-gain reports compute the CGT outcome with no special casing;
- an **Income row** for the dividend component (franked amount + franking
  credits), when there is one, so the dividend is assessable in the tax
  summary and subject to the ordinary franking-credit entitlement rules
  (45-day holding period / small-shareholder exemption).

Not modelled: the further adjustments where the participating shareholder is
itself a company, and the on-market buy-back's treatment for share traders
(shares held on revenue account).
