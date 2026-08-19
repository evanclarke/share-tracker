# ESS — takeovers and restructures (ITAA 1997 s 83A-130)

> Source: https://www.ato.gov.au/law/view/print?DocID=PAC/19970038/83A-130&PiT=99991231235958
> Retrieved: 2026-08-19 from the Australian Taxation Office legal database (ato.gov.au),
> *Income Tax Assessment Act 1997* s 83A-130, Subdivision 83A-C (deferred inclusion of gain).
> This is a local copy of the provision for reference. The ATO site is authoritative.
> Excerpted: subsections (1), (2), (4), (5), (6) and (9) — the object, the continuation rule,
> the unmatched-consideration disposal, and the exceptions.

---

## Why this is here

`docs/ato/ess-30-day-rule.md` states the rule this project acts on: a **disposal** within 30 days
after the deferred taxing point moves the taxing point to the disposal date. What counts as a
disposal therefore decides whether the rule bites. s 83A-130 answers that for a takeover or a
demerger: the replacement interests are treated as a **continuation** of the old ones, so Division
83A keeps applying and no taxing point crystallises — except to the extent the holder receives
something that is *not* a matching continuation (subsection (5)).

## The provision

### Object and scope

> **83A-130(1)** The object of this section is to allow this Division to continue to apply if:
>
> (a) at least one of the following applies:
>
>   (i) an \*arrangement (the **takeover**) is entered into that is intended to result in a company
>   (the **old company**) becoming a \*100% subsidiary of another company;
>
>   (ii) \*ESS interests in a company (the **old company**) acquired under \*employee share schemes
>   can reasonably be regarded as having been replaced, wholly or partly, by ESS interests in one or
>   more other companies as a result of a change (the **restructure**) in the ownership (including
>   the structure of the ownership) of the old company or a \*demerger subsidiary of the old
>   company; and
>
> (b) just before the takeover or restructure, you held ESS interests (the **old interests**) in the
> old company that you acquired under an employee share scheme.

### Treat new interests as continuations of old interests

> **83A-130(2)** For the purposes of this Division, treat any \*ESS interests (the **new interests**)
> in a company (the **new company**) that you acquire in connection with the takeover or restructure
> as a continuation of the old interests, to the extent that:
>
> (a) as a result of the arrangement or change, you stop holding the old interests; and
>
> (b) the new interests can reasonably be regarded as matching any of the old interests.
>
> **Note:** In determining to what extent something can reasonably be regarded as matching any of
> the old interests, one of the factors to consider is the respective market values of that thing and
> of the old interests.

> **83A-130(4)** Subsections (2) and (3) only apply if the new interests relate to ordinary \*shares.

### Old interest not matched by new interests

> **83A-130(5)** For the purposes of this Division, treat yourself as having disposed of the old
> interests to the extent that, in connection with the takeover or restructure, you acquire anything
> that:
>
> (a) can reasonably be regarded as matching any of the old interests; but
>
> (b) is not treated by subsection (2) as a continuation of those interests.

### Continuation of your employment

> **83A-130(6)** For the purposes of this Division, treat your employment by:
>
> (a) the new company; or (b) a \*subsidiary of the new company; or (c) a holding company (within the
> meaning of the *Corporations Act 2001*) of the new company; or (d) a subsidiary of a holding
> company (within the meaning of the *Corporations Act 2001*) of the new company;
>
> as a continuation of the employment in respect of which you acquired the old interests.

### Apportionment of cost base of old interests

> **83A-130(7)** Treat yourself as having given, as consideration for the assets mentioned in
> subsection (8), the amount worked out by apportioning among those assets, according to their
> respective \*market values immediately after the takeover or restructure, the total of:
>
> (a) the \*cost bases of the old interests when you stop holding them; and
>
> (b) the cost bases of the assets mentioned in paragraph (8)(b) immediately after the takeover or
> restructure (ignoring the effect of this subsection).

### Exceptions

> **83A-130(9)** This section only applies if:
>
> (a) at or about the time you acquire the new interests, you are employed as mentioned in
> subsection (6); and
>
> (b) at the time you acquire the new interests:
>
>   (i) you do not hold a beneficial interest in more than 10% of the \*shares in the new company;
>   and
>
>   (ii) you are not in a position to cast, or to control the casting of, more than 10% of the
>   maximum number of votes that might be cast at a general meeting of the new company.

---

> **Project note** (not from the source): what this means for the 30-day-rule health alert
> (`reports::health`'s `ess_30_day_rule`), which pairs a Sell allocation drawing on a vest parcel
> with the statement whose taxing point it falls within 30 days of.
>
> - A **holding-account transfer** (`entities::transfer`) is not within s 83A-130 at all, because it
>   is not a takeover or a restructure and nothing is disposed of: the same beneficial owner holds
>   the same interests before and after, which is why no CGT event arises either. Its transfer-out
>   Sell is excluded from the alert.
> - A **scrip-for-scrip exchange** (a takeover, (1)(a)(i)) or a **demerger** ((1)(a)(ii), which names
>   a demerger subsidiary expressly) does close the old parcels, but subsection (2) treats the
>   replacement interests as a continuation of them where they match — so the taxing point does not
>   move, *provided* subsection (9)'s conditions (continuing employment, the two 10% tests) and
>   subsection (4)'s ordinary-shares requirement hold. None of those is recorded here, so the alert
>   keeps the row and labels it `TakeoverOrRestructure`: the answer turns on facts outside this
>   system.
> - Subsection (5) is why the label is not simply "ignore me": a partial-rollover scrip exchange's
>   **cash** component is consideration that is not a continuation, so the old interests are disposed
>   of to that extent and the rule can bite on it.
