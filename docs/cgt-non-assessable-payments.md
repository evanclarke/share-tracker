# Non-assessable payments (return of capital — CGT events G1, E4, E10)

> Source: https://www.ato.gov.au/forms-and-instructions/capital-gains-tax-guide-2016/introduction/investments-in-shares-and-units/non-assessable-payments
> Retrieved: 2026-06-06 from the Australian Taxation Office (ato.gov.au), "Guide to capital gains tax 2016" (the worked example is unchanged in later guides)
> This is a local copy of ATO guidance for reference (excerpt). The ATO site is authoritative.

---

You may need to adjust the cost base of shares or units for CGT calculations if you receive a non-assessable payment without disposing of your shares or units. A payment or distribution can include money and property.

## Non-assessable payments from a company (CGT event G1)

Non-assessable payments to shareholders are not very common and would generally be made only if a company has shareholder approval to reduce its share capital. If you receive a non-assessable payment from a company (that is, a payment that is not a dividend or an amount that is taken to be a dividend for tax purposes), you need to adjust the cost base of the shares at the time of the payment. These payments will often be referred to as a return of capital.

If the amount of the non-assessable payment is not more than the cost base of the shares at the time of payment, you reduce the cost base and reduced cost base by the amount of the payment.

You make a capital gain if the amount of the non-assessable payment is more than the cost base of the shares. The amount of the capital gain is equal to the excess. If you make a capital gain, you reduce the cost base and reduced cost base of the shares to nil. You cannot make a capital loss from the receipt of a non-assessable payment.

### Example 45: Non-assessable payments

Rob bought 1,500 shares in RAP Ltd on 1 July 1994 for $5 each, including brokerage and stamp duty. On 30 November 2007, as part of a shareholder-approved scheme for the reduction of RAP Ltd's share capital, he received a non-assessable payment of 50 cents per share. Just before Rob received the payment, the cost base of each share (without indexation) was $5.

As the amount of the payment is not more than the cost base (without indexation), he reduces the cost base of each share at 30 November 2007 by the amount of the payment to $4.50 ($5.00 – 50 cents). As Rob has chosen not to index the cost base, he can claim the CGT discount if he disposes of the shares in the future.

## Non-assessable payments from a unit trust (CGT event E4 or E10)

Unit trusts often make non-assessable payments to unit holders. When you sell the units, you must adjust their cost base and reduced cost base. The amount of the adjustment is based on the total of the non-assessable payments you received during the income year up to the date of sale. You use the adjusted cost base and reduced cost base to work out your capital gain or capital loss.

If the unit or interest is not in an AMIT, the CGT event is E4, and if the unit or interest is in an AMIT, the CGT event is E10.

> Project note: the AMIT (E10) case is modelled — see `amit-cost-base-adjustments.md` and the
> AMIT adjustment / net-capital-gain implementation. The **company return of capital (G1)**
> case is modelled as a `ReturnOfCapital` corporate action (`src/entities/corporate_action.rs`):
> it reduces the cost base of parcels held on the payment date in the portfolio/open-parcels/
> unrealised/realised reports, and the excess over a parcel's cost base is a G1 capital gain in
> the net-capital-gain report (`g1_gains`). The non-AMIT unit-trust case (**E4**) is not
> distinguished from G1 — both reduce cost base and gain on the excess; the E4 timing nuance
> (adjustment at sale based on the income year's payments) is not separately modelled.
