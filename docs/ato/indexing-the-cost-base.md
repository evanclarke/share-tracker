# Indexing the cost base

> **Source:** https://www.ato.gov.au/individuals-and-families/investments-and-assets/capital-gains-tax/calculating-your-cgt/indexing-the-cost-base
> (QC 66024, last updated 23 June 2025)
> **Retrieved:** 2026-06-11
> The live ATO site is authoritative; this is a convenience mirror.

## How indexation works

The indexation method adjusts the amount of an asset's costs by the rate of
inflation. The adjustment is based on the consumer price index (CPI).

The increased cost amounts will reduce your capital gain on the asset.

However, you:

- must have incurred the costs **by 21 September 1999**
- can index for inflation **only up to 30 September 1999** (the indexation is
  frozen at that quarter's CPI)
- cannot index the third element of the cost base (costs of owning the asset).

## When to use indexation

If your asset is eligible for indexation, it is probably also eligible for the
50% CGT discount for individuals.

You can use **whichever of these methods gives you the best result** (the
lowest capital gain), **but not both**.

In most cases the discount will give you the best result. Indexation may give
you a better result in some situations, such as if you also have capital
losses.

Companies cannot use the CGT discount. They should use indexation for assets
acquired before 21 September 1999.

If you have had a **capital loss** on an asset, you **cannot use indexation**.

## How to apply indexation

1. **Identify your eligible capital costs** — incurred no later than
   21 September 1999; third-element (ownership) costs cannot be indexed.
2. **For each eligible cost, identify the CPI rate** for the quarter in which
   the cost was incurred. (A call on partly paid shares or units acquired
   after 15 August 1989 is indexed from the date of the later payment.)
3. **Calculate the indexation factor**: divide **68.7** (the CPI for
   30 September 1999) by the CPI from step 2, limited to 3 decimal places
   (round the fourth decimal up from 5, e.g. 1.4125 → 1.413).
4. **Multiply the cost by the indexation factor.**
5. **Total your indexed eligible costs and any non-indexed capital costs** —
   this is your indexed cost base.
6. **Subtract the indexed cost base from your capital proceeds** — this is
   your capital gain.

Remember, if you index the cost base you cannot apply the CGT discount.

### Example: indexing the cost base (Val)

Val bought an investment property for $150,000 under a contract dated
24 June 1991. She paid:

- a deposit of $15,000 on 24 June 1991
- the balance of $135,000 on settlement on 5 August 1991
- stamp duty of $5,000 on 20 July 1991
- solicitor's fees of $2,000 on 5 August 1991 as part of settlement.

Val sold the property on 15 October 2024 (the day contracts were exchanged)
for $600,000. She incurred costs of $1,500 in solicitor's fees and $15,000 in
agent's commission.

The costs of buying the property were incurred before 21 September 1999, so
they are eligible for indexation. The CPI rates are 59.0 (June 1991 quarter:
deposit and balance — although the balance was paid in the September quarter,
it is indexed from the date of contract) and 59.3 (September 1991 quarter:
stamp duty and solicitor's fees).

The indexation factors are 68.7 ÷ 59.0 = **1.164** and 68.7 ÷ 59.3 = **1.159**.

| Cost | Indexed |
|------|---------|
| Deposit $15,000 × 1.164 | $17,460 |
| Balance $135,000 × 1.164 | $157,140 |
| Stamp duty $5,000 × 1.159 | $5,795 |
| Solicitor's fees (purchase) $2,000 × 1.159 | $2,318 |

Val's total cost base is **$199,213**: indexed costs $182,713 + $1,500
solicitor's fees (sale) + $15,000 agent's commission (neither eligible for
indexation).

Using indexation, Val's capital gain is $600,000 − $199,213 = **$400,787**.

Val is eligible to use the CGT discount instead of indexation. Unless she has
significant capital losses to apply, she will get a better result by using the
CGT discount.

*End of example*

---

**How this project uses it:** not modelled — documented as a Known limitation
(`docs/API.md`). The indexation method only ever applies to assets acquired
before 21 September 1999, the discount is almost always the better result for
an individual, and the net-capital-gain pipeline applies the 50% discount
throughout.
