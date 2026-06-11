# Capital proceeds from disposing of assets (market-value substitution)

> **Source:** https://www.ato.gov.au/individuals-and-families/investments-and-assets/capital-gains-tax/calculating-your-cgt/capital-proceeds-from-disposing-of-assets
> (QC 66021, last updated 23 June 2025)
> **Retrieved:** 2026-06-11
> The live ATO site is authoritative; this is a convenience mirror.

## Types of capital proceeds

Capital proceeds are what you receive, or are entitled to receive, from a
capital gains tax (CGT) event, such as selling an asset.

For most CGT events your capital proceeds will be money. They can also be the
value of any property you receive or are entitled to receive.

If you receive:

- **foreign currency** – work out the capital proceeds by converting it to
  Australian currency at the time of the CGT event
- **property (including shares) subject to a deed of escrow** – your capital
  proceeds include the market value of the property at the time of the CGT
  event (a deed of escrow imposes a restriction on dealing in that property).

If you give away or sell an asset for less than it's worth, your capital
proceeds equal the market value of the asset.

## Market value substitution

If you receive nothing in exchange for a CGT asset, you are taken to have
received the **market value** of the asset at the time of the CGT event.

This is the **market value substitution rule** for capital proceeds.

You may also be taken to have received the market value if both of the
following apply:

- what you received was more or less than the market value of the CGT asset
- you and the new owner were **not dealing with each other at arm's length**.

You are dealing at "arm's length" with someone when each party acts
independently. This occurs when neither party exercises influence or control
over the other in connection with the transaction.

The law looks at the relationship between the parties and the quality of the
bargaining between them.

The market value substitution rule may apply when transferring property to
family or friends.

### Example: gifting an asset

Martha and Stephen bought a block of land in 2010.

Later, after many years, they complete a transfer form to gift the block to
their son, Paul.

As Martha and Stephen received nothing for it, they are taken to have received
the market value of the land at the time it was transferred to Paul.

*End of example*

## Reducing your capital proceeds

You reduce your capital proceeds from a CGT event if:

- you're not likely to receive some or all of the proceeds
- it's not due to anything you have done or failed to do
- you took all reasonable steps to get payment.

If you repay part of the proceeds and you aren't entitled to a tax deduction
for the repayment, your capital proceeds are reduced by the amount you repaid.
The same applies to compensation you pay that can reasonably be regarded as a
repayment of the proceeds.

If you are registered for GST, any GST payable on the amount you receive is
not part of the capital proceeds.

---

**How this project uses it:** there is no dedicated gift / off-market
related-party transfer entry path — the rule is documented as a Known
limitation (`docs/API.md`). A gift of shares or crypto out of the portfolio is
a CGT disposal at market value, entered as a manual Sell at market-value
proceeds; a gift received is entered as a manual Buy at market-value cost.
