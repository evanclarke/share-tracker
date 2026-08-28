# When a FITO applies — foreign tax paid on part of an amount included in your income

> **Source:** https://www.ato.gov.au/forms-and-instructions/foreign-income-tax-offset-rules-guide-2025/when-a-fito-applies
> **Retrieved:** 2026-08-19
> Part of "Guide to foreign income tax offset rules 2025" (QC 104349, last updated 29 May 2025).
> The live ATO site is authoritative; this is a convenience mirror of the one section this
> project's calculation rests on. The offset-limit calculation itself is mirrored separately in
> [`fito-limit.md`](fito-limit.md).

## Foreign income tax paid on part of an amount included in your income

In some situations, only part of an amount on which foreign tax has been paid is included in
Australian assessable income. In this situation, only that proportion of the foreign income tax
which is the same as the proportion of foreign income included would be available as a tax offset.

In other situations, the foreign income tax paid relates to only part of an amount included in the
taxpayer's income for Australian income tax purposes. This typically applies where a foreign source
gain on which foreign income tax has been paid is part of a net amount included in a taxpayer's
assessable income. When this occurs, the foreign income tax counts towards the tax offset only to
the extent that it is paid.

For example, this may be relevant where a taxpayer has both a capital gain and a capital loss, and
only the net amount is included in their assessable income. Under the rules applying to capital
gains and capital losses, a taxpayer can choose the order in which capital losses are offset against
gains. In particular, a taxpayer can choose to offset capital losses (whether current year or
prior-year unapplied net capital losses) first against domestic capital gains or foreign gains on
which no foreign tax has been paid. Such an ordering of the losses maximises the foreign source
capital gain component of a net capital gain on which foreign income tax has been paid.

**If only part of a foreign capital gain is assessable in Australia (for example, the gain is
subject to the discount capital gains concessions in Division 115 of the ITAA 1997) the foreign tax
paid on the gain must be apportioned accordingly.** This includes, where a foreign capital gain is
distributed to a unitholder of a managed investment trust (MIT) or attribution managed investment
trust (AMIT). In such circumstances, when calculating your FITO, the 'Foreign tax offset applicable
to discountable capital gains' shown at Part C – Tax offsets of the Attribution Managed Investment
Trust Member Annual (AMMA) statement or Standard Distribution Statement (SDS) must be reduced for
discounted capital gains.

### Example 11: foreign income tax paid on part of an amount included in assessable income

Company C derives the following capital gains and losses on disposals of assets during the year.

| Capital gains or losses | Amount |
| --- | --- |
| Domestic capital gain on land | \$100,000 |
| Foreign capital gain on asset B (no foreign tax paid) | \$50,000 |
| Foreign capital gains on asset C (on which foreign income tax of \$2,000 is paid) | \$20,000 |
| Domestic capital loss on asset A | \$160,000 |
| **Net capital gain** | **\$10,000** |

As the foreign income tax offset can only apply where foreign income tax has been paid on an amount
included in the taxpayer's assessable income, company C chooses to offset its domestic capital loss
on asset A of \$160,000 against: firstly, the domestic gain on land of \$100,000; then \$50,000 against
the foreign capital gain on asset B on which no foreign income tax has been paid; lastly, the
balance of \$10,000 against the foreign capital gain on asset C.

Therefore, the net capital gain of \$10,000 relates to the foreign capital gain on asset C. As this
is the amount included in assessable income on which foreign income tax has been paid, the
proportionate share of tax paid of **\$1,000** (that is, (10,000 ÷ 20,000) × 2,000) counts towards
company C's foreign income tax offset.

### Example 12: no foreign income tax offset — foreign income not included in assessable income

Leslie is an Australian-resident taxpayer. On the sale of an asset, Leslie makes a foreign source
capital gain of \$10,000, on which she paid foreign income tax of \$2,000. Leslie also realises a
capital loss of \$10,000 on the disposal of an Australian asset.

The loss of \$10,000 is offset against the foreign gain of \$10,000, which results in no net capital
gain being included in Leslie's assessable income. As her assessable income does not include an
amount on which foreign income tax has been paid, she is not eligible for a foreign income tax
offset for the foreign income tax paid on the foreign source capital gain.

> Further guidance on this apportionment: [ATO ID 2010/175](https://www.ato.gov.au/law/view/document?DocID=AID/AID2010175/00001&PiT=99991231235958)
> *Foreign income tax offset: entitlement where foreign capital gain is only partly assessable in
> Australia*, cited by the AMMA trustee guidance notes
> ([`amma-statement-guidance-notes.md`](amma-statement-guidance-notes.md)).

## Relevance to this project

The trustee reports the **grossed-up** foreign tax on foreign capital gains — the AMMA guidance
notes are explicit that "FITO is not reduced for discount capital gains applied at the trust level"
and that the investor must gross the discount capital gain up and then work out their own
entitlement. So the Division 115 reduction is the **investor's** step, which is this system's job.

The tax summary therefore takes the AMMA's capital-gains foreign tax
(`amma_statements.foreign_tax_credits_capital_gains`, its own Part C line, distinct from the
foreign tax on foreign *income*) and apportions it to the assessable part of the gains it was paid
on:

```
claimable = capital-gains foreign tax × (assessable gains ÷ grossed-up gains)
```

where the grossed-up gains are `2 × cgt_discount_gains + cgt_indexation_gains + cgt_other_gains`
(the discount component is reported net and must be grossed up) and the assessable gains are
`cgt_discount_gains + cgt_indexation_gains + cgt_other_gains`. With only discount gains present
this is the halving the page describes; with a mix it apportions across the three methods.

**Only the trust path is recordable** (decision 2026-08-19, SCENARIOS M-12): the page's own
framing — a taxpayer disposing of an asset a foreign country taxes, as in Examples 11 and 12 — has
no field in this system, because a Sell carries no foreign-tax column. Under the usual treaty
position a source country taxes a non-resident's gain on **real property** and land-rich
interests, assets this system does not record at all, so the disposal the tax would attach to
could not be entered either. The AMIT/MIT distribution the page goes on to describe is where a
listed-share investor meets a foreign-taxed capital gain, and that is the path implemented above;
the direct case is a documented scope cut (docs/API.md, Known limitations) claimed outside this
tool.

**Not modelled from this page:** the loss-ordering *choice* (Example 11 — offsetting capital losses
first against domestic gains to maximise the foreign-taxed component of the net gain) and the
no-net-gain case (Example 12, where the offset falls to nil). The net-capital-gain report applies
one fixed loss-netting order and does not track which gains carry foreign tax through it, so a
taxpayer with foreign-taxed gains **and** capital losses in the same year must check the outcome
themselves. The A\$1,000 de-minimis ([`fito-limit.md`](fito-limit.md)) bounds the exposure: below it
no limit calculation is required at all.
