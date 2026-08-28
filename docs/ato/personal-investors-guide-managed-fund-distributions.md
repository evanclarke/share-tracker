# Personal investors guide to CGT 2025 — Part C: Distributions from managed funds

> Source: https://www.ato.gov.au/forms-and-instructions/capital-gains-tax-personal-investors-guide-2025/part-c-distributions-from-managed-funds
> Retrieved: 2026-07-12 from the Australian Taxation Office (ato.gov.au) (QC 104651, published 29 May 2025)
> This is a local copy of ATO guidance for reference. The ATO site is authoritative.

The investor-side worksheet for capital gains distributed/attributed by a managed fund or AMIT —
the counterpart to the trustee-side AMMA reporting notes
([`amma-statement-guidance-notes.md`](amma-statement-guidance-notes.md)). The key point for this
project: the losses subtracted in the investor's calculation (Step 4) are **the investor's own**
current-year capital losses and unapplied net capital losses from earlier years. Capital losses
applied **at the trust level** never enter the investor's worksheet — per the trustee guidance
notes, the attributed gains on the statement are *already reduced* for them, and the losses-applied
figure is disclosed only so the investor can understand the statement (and apportion FITO). A trust
cannot pass its capital losses through to members.

---

## Attribution managed investment trusts

A managed investment trust (MIT) may choose the attribution rules of Division 276 ITAA 1997,
becoming an AMIT. The rules attribute amounts for tax purposes to each member based on their
interest in the AMIT (rather than present entitlement); attributed amounts keep their tax
character, flow through, and are treated as received directly. For capital gains this means the
member treats the capital gains component of trust income as a capital gain the member makes. The
member statement for an AMIT is the AMIT member annual statement (AMMA statement); the cost base
of AMIT units may also be subject to annual upward or downward adjustments (see below).

## C1: How to work out your capital gains tax for a managed fund distribution

If a managed fund distribution includes a capital gain amount, it is included at question 18
Capital gains (not question 13 Partnerships and trusts).

- **Step 1 — Work out the capital gain received from the fund.** The statement shows which
  method the fund used: indexation, discount, or 'other'. The investor must use the same methods
  as the fund. (Funds may call indexation/'other' gains "non-discount gains".)
- **Step 2 — Gross up any discounted capital gain.** A distributed gain the fund has already
  discounted is grossed up by **multiplying by 2**; the grossed-up amount is the capital gain
  from the fund.
  - *Example 21*: Tim receives a discounted capital gain of \$400 → grosses up to \$800 (\$400 × 2).
- **Step 3 — Total current-year capital gains (label 18H).** Add all fund gains (grossed up where
  necessary) and gains from other assets; write the total at label H. Capital losses are **not**
  deducted before label H.
  - *Example 22*: Tim's fund also distributes a \$100 'other'-method gain → 18H is \$900
    (\$800 + \$100).
- **Step 4 — Apply capital losses against capital gains.** "If you make capital losses this year,
  deduct them from the amount you wrote at label H. If you have unapplied net capital losses from
  earlier income years, deduct them from the amount remaining after you deduct capital losses made
  this year. Deduct both types of losses in the manner that gives you the greatest benefit" —
  usually against 'other'/indexation gains first, then discount gains.
  - *Example 23*: Tim has a \$200 capital loss selling another CGT asset → \$900 − \$200 = \$700,
    applied against the 'other' gain first, leaving the whole \$700 discountable.
- **Step 5 — Apply the CGT discount.** Remaining grossed-up discount gains are reduced by 50%.
  The discount is never applied to indexation/'other'-method gains.
  - *Example 24*: \$700 × 50% = \$350.
- **Step 6 — Net capital gain (label 18A).** The remainder after steps 1–5.
  - *Example 25*: Tim writes \$350 at question 18 – label A.

  (Examples 21–25 are one continuous worked example, reproduced in
  `src/ato_examples.rs` as `pig_managed_funds_examples_21_25_tim_*`. The same
  step order stated for the individual return — losses before the discount,
  current-year losses before earlier-year ones — is mirrored in
  [`capital-gains-question-18.md`](capital-gains-question-18.md).)
- **Step 7 — Carry-forward losses (label 18V).** If total losses (current-year + unapplied prior)
  exceed the year's gains: nothing at 18A; the excess at 18V, carried forward against later years.

## C2: Non-assessable payments from a managed fund

Non-assessable payments may appear on the statement as tax-free amounts, CGT-concession amounts,
tax-exempted amounts, or tax-deferred amounts. Slightly different rules apply to AMITs.

- **Tax-free amounts** adjust the reduced cost base only (not the cost base).
- **CGT-concession amounts** (the discount component of an actual distribution) do not affect
  cost base or reduced cost base if received after 30 June 2001.
- **Tax-exempted amounts** do not affect cost base or reduced cost base.
- **Tax-deferred amounts** reduce both the cost base and the reduced cost base. If a tax-deferred
  amount exceeds the cost base of the units, the excess is a capital gain. "You can't make a
  capital loss from a non-assessable payment."

### Cost-base adjustments for AMIT members

The annual adjustment is driven by the **AMIT cost base net amount** (the balance of the AMIT
cost base reduction amount — cash payments plus tax offsets — against the AMIT cost base increase
amount — amounts attributed as assessable/NANE income plus attributed trust capital gains).
A net reduction reduces the cost base; if it exceeds the cost base it reduces it to nil and **any
remaining amount is a capital gain** (CGT event E10). A net shortfall (increase) raises the cost
base and reduced cost base. Tax-free/tax-deferred amounts are not separately applied for an AMIT —
they are reflected within the AMIT cost base net amount. See LCR 2015/11.

## C3: Worked examples for managed fund distributions

### Example 26: receiving a non-assessable amount from a managed fund (Bob)

Bob's OZ Investments Fund statement shows his distribution included: \$100 discount-method gain
(grossed-up \$200), \$75 indexation-method gain, \$28 'other'-method gain, and a \$105 tax-deferred
amount. His units' cost base is \$1,200 (reduced cost base \$1,050). He has no other gains or losses.

- Total current-year gains: \$200 + \$75 + \$28 = **\$303 at 18H**
- No losses; the discount reduces the grossed-up gain back to \$100 → **\$203 at 18A**
- (Non-AMIT) the \$105 tax-deferred amount reduces cost base \$1,200 → \$1,095 and reduced cost base
  \$1,050 → \$945. If the fund is an AMIT, the AMIT cost base net amount governs instead.

### Example 27: a capital loss greater than the indexation + 'other' gains (Ilena)

Ilena's XYZ Managed Fund statement shows: \$65 discounted capital gain, \$50 'other'-method gain,
\$40 indexation-method gain (plus a \$30 tax-deferred and \$35 tax-free amount). She has no other
capital gain but made a **\$100 capital loss of her own** selling shares during the year. Her
units' cost base is \$5,000 (reduced cost base \$4,700).

- Gross up the discounted gain: \$65 × 2 = \$130
- Total current-year gains: \$130 + \$50 + \$40 = **\$220 at 18H**
- Apply her own \$100 loss, best-first: \$90 against the indexation + 'other' gains (\$90 → \$0),
  the remaining \$10 against the grossed-up discount gain (\$130 → \$120)
- Apply the 50% discount: \$120 × 50% = \$60
- Net capital gain: \$0 + \$60 = **\$60 at 18A**
- (Non-AMIT) the tax-deferred amount reduces the cost base \$5,000 → \$4,970; tax-deferred +
  tax-free reduce the reduced cost base \$4,700 → \$4,635.

### Example 28: notified of an AMIT cost base net adjustment (Miriam)

Miriam's AMIT units have a cost base of \$55 each. The fund attributes \$13 of assessable income
per unit but pays only \$3 cash: increase \$13 vs reduction \$3 → a \$10 shortfall AMIT cost base net
amount, **increasing** her cost base to \$65 (protecting the retained amount from double taxation
on later sale). In the alternative (attributes \$3, pays \$13 cash), the \$10 excess **reduces** the
cost base to \$45.
