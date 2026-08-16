# Verification scenarios

A catalogue of situations to drive against share-tracker and verify from an
Australian tax professional's point of view. **This file deliberately records no
expected behaviour** — only the situation to construct and the surfaces that
ought to be looked at. Working out what *should* happen (from `docs/ato/`, the
ATO site, `docs/API.md`, and `README.md`) is the verifying agent's job, and
doing that independently is the point: a scenario file that pre-states the
answer can only confirm what the implementer already believed.

## How to use this file

- Each scenario has a stable id (`C-07`). Cite it in findings, TODO items, and
  commit messages so a result can be traced back.
- **Setup** is the situation to construct — enter it through the HTTP API (or
  the web UI where the scenario is about the UI), the way a user would.
- **Probe** lists the surfaces to inspect. It is a starting set, not a closed
  list; every scenario implicitly also asks *"what else moved that shouldn't
  have, and what didn't move that should have?"*
- A scenario is **not** an assertion that something is broken. Many will come
  back "correct, and here is the test that proves it". Some will land on a
  documented [Known limitation](docs/API.md#known-limitations) — in that case
  the verification is that the limitation is *honestly* documented and that the
  system fails safe (refuses, or flags) rather than silently producing a wrong
  figure.
- Anything confirmed wrong becomes a `TODO.md` item; anything confirmed correct
  should leave behind a regression test (`src/ato_examples.rs` when it mirrors
  a published ATO example, otherwise the relevant module's inline tests).
- When a section has been driven end to end, fill in its row in
  [Verification status](#verification-status) below — the commit that did the
  pass, and where its findings went. That row is the only record in this file
  that a section has been looked at; without it the next reader has to go
  spelunking in the commit log.

### Standing probes

Apply these to *every* mutating scenario unless it obviously can't apply. They
are not repeated in the individual entries.

1. **Cost base** — every affected parcel's initial, adjusted, and AUD cost base.
2. **Discount clock** — each parcel's acquisition date and 12-month eligibility.
3. **Realised / unrealised / net capital gain / tax summary** reports for every
   financial year the change can touch, not just the current one.
4. **Report snapshots** — did the write mark the right dates stale (schema
   staleness triggers), and does regeneration produce the changed figure?
5. **Row history** — is the prior row recorded for every UPDATE/DELETE?
6. **Cross-check reports** — AMIT cash, AMIT adjustment, E4, wash sales,
   franking at-risk, settlement coverage, MIC validation, health.
7. **Referential guards** — is the write refused (422) while something still
   draws on the row, and does the refusal name what draws on it?
8. **Atomicity** — if any part of a multi-row operation fails, is *nothing*
   persisted?
9. **Annual tax report** for the affected year, and the CSV exports.
10. **Web UI** — does the affected screen render the change, and does the error
    text surface in the toast rather than a bare status code?

---

## Verification status

453 scenarios in 27 sections. A section counts as **verified** only when every
scenario in it has been driven and each result either left a regression test
behind or became a recorded finding.

| Section | Scenarios | Verified | Findings |
| --- | ---: | --- | --- |
| A. Deletion and mutation ripple effects | 45 | 2026-08-14 (`0bbde4d`) | 5 raised, all closed — see below |
| B. Cost base construction and the adjustment pipeline | 24 | 2026-08-15 | 5 raised, all closed — see below |
| C. The 12-month CGT discount clock | 18 | 2026-08-15 | 1 raised, closed — see below |
| D. Sells and parcel allocation | 20 | 2026-08-15 | 2 raised, both closed — see below |
| E. Corporate actions | 51 | 2026-08-16 | 5 raised, open — see below |
| F. AMIT / AMMA | 25 | — | — |
| G. Dividends, franking, and the holding-period rule | 25 | — | — |
| H. Interest, expenses, and other income | 10 | — | — |
| I. DRP | 14 | — | — |
| J. Employee share schemes | 14 | — | — |
| K. Inherited parcels | 10 | — | — |
| L. Crypto | 15 | — | — |
| M. Foreign currency and FX | 16 | — | — |
| N. Holding accounts and transfers | 12 | — | — |
| O. Net capital gain, losses, and carry-forward | 17 | — | — |
| P. Tax summary, annual tax report, exports | 12 | — | — |
| Q. Prices, valuation, and snapshots | 15 | — | — |
| R. Listing identity and renames | 10 | — | — |
| S. Settlement, holidays, and dates | 10 | — | — |
| T. Jobs, backup, and operations | 12 | — | — |
| U. Audit trail and history | 8 | — | — |
| V. Back-dated and out-of-order entry | 10 | — | — |
| W. Precision, rounding, and scale | 8 | — | — |
| X. Transactional integrity and concurrency | 8 | — | — |
| Y. Web UI | 12 | — | — |
| Z. Composite lifecycle scenarios | 12 | — | — |
| AA. Boundary and out-of-scope scenarios | 20 | — | — |

### Section A findings

Forty of the 45 came back correct. `A-24` and `A-42` cannot arise at all
(`rba_fx_rates` and `currencies` are read-only, `405`). The five findings, each
archived in [`DONE/reviews.md`](DONE/reviews.md) under a heading naming its
scenario ids:

| Finding | Scenarios | Fixed by |
| --- | --- | --- |
| A Buy's `date` and `holding_account_id` escape the Sell-allocation invariants | A-09, A-13 | `408459b` |
| Deleting a split/bonus/return-of-capital silently restates reported gains | A-06, A-20, A-21 | `fca9721`, `6512cb4` |
| A DELETE blocked by an inbound foreign key says the row does not exist | A-18, A-23, A-38, A-41 | `2af8d4f` |
| Deleting a DRP enrolment period strands its trailing residual | A-43 | `cb96f00` |
| A closed financial year can be restated with nothing marking it | A-15, A-21, A-25, A-35, A-40 | `42a6abe` (documented limitation) |

### Section B findings

Nineteen of the 24 came back correct. All five findings are closed, each
archived in [`DONE/reviews.md`](DONE/reviews.md) under a heading naming its
scenario ids, and each naming the commit that closed it:

| Finding | Scenarios | Fixed by |
| --- | --- | --- |
| An AMIT cost-base adjustment over a split applies the statement's per-unit figure to the wrong units | B-24 | `305dda8` |
| A parcel reduced by both an AMIT adjustment and a return of capital loses the excess over its cost base | B-07, B-08 | `83488c2` |
| Brokerage in a currency other than the trade's is added to the cost base unconverted | B-02 | `04bd0e8` |
| A return of capital has no record date, so it reduces parcels bought after the entitlement was fixed | B-09 | `14601f5` |
| Two documentation gaps (sale-side incidental costs; rights bought on-market) | B-17, B-20 | `33a4534` (documented) |

### Section C findings

Seventeen of the 18 came back correct, and the clock itself held up everywhere
it was probed. The one rule lives in `domain::cgt_discount::discount_eligible`
(`event_date > acquired + 12 months`, exclusive of both days) and every
classifier calls it — realised gains, unrealised gains, the net-capital-gain
report's E10/G1 excess gains, the parcel optimiser, the annual tax report — so
there is no second boundary to disagree with the first. What each scenario
turned on:

- **Anchor date.** Every caller passes `ParcelRow::acquired()` — the deemed
  date where a rollover, transfer, or inheritance set one, else the trade date
  — never the raw trade date. That is what makes C-06 through C-15 come out:
  rights exercise starts a *new* clock at exercise (C-06), scrip-for-scrip and
  demerger carry the original's (C-07, C-08), a holding-account transfer does
  not restart it (C-10), an inherited parcel runs from the deceased's
  acquisition or, for a pre-CGT asset, the death (C-11, C-12), an ESS parcel
  runs from the deferred taxing point and no grant date is recorded at all
  (C-13), and each DRP allotment runs its own (C-14).
- **Dates that must *not* move it.** Contract dates decide, never settlement
  (C-03) — no gain report reads `settlement_date` at all. Splits and bonus
  issues re-base quantities only (C-04, C-05), and on a replacement parcel the
  quantity re-base keys off the trade date while the clock keys off the deemed
  date, so the two survive each other (C-15).
- **Boundaries.** Exactly 12 months is not enough; a 29 February parcel first
  discounts on 1 March (C-01, C-02). Crypto is the same rule with same-day
  settlement (C-17). Eligibility is per parcel, so a split allocation carries
  one flag per parcel and never pro-rates across the line (C-16). A sale dated
  before its parcel is refused from both sides — the Sell's allocation check
  and the Buy's date-move guard — so the clock can't run backwards (C-18).

| Finding | Scenarios | Fixed by |
| --- | --- | --- |
| Scrip-for-scrip and demerger rollovers are assumed, not stated as a scope cut | C-09 | documented |

### Section D findings

Eighteen of the 20 came back correct. The allocation invariants are the strong
part: they live in one place (`sell::upsert_sell_in_tx`), every write goes
through it — the `PUT /sells/:id` upsert and each operation-built Sell alike —
and the standalone `parcel_allocations` write routes are disabled, so there is
no second path to disagree with the first. What each group turned on:

- **The sum and the capacity check.** Allocations must sum to the sale's
  quantity by exact `Decimal` equality, so a millionth of a unit is caught like
  a whole one (D-04); each allocation must be positive (D-05); and capacity is
  re-checked per parcel against *every* allocation drawing on it, so two rows
  naming one parcel are capped by their sum (D-05), a second Sell is capped by
  what the first left whether it is the same day or a year later (D-10, D-12),
  an earlier sale cannot be amended up past what a later one consumed (D-17),
  and a parcel a scrip exchange consumed is not sellable again (D-19). 200
  allocation rows go through as one Sell (D-06).
- **What a sale may draw on.** Same listing, same holding account, not dated
  after the sale — all three refused with the reason (D-01, D-02, D-03), and a
  same-day parcel is fine (D-09). A holding spread across three accounts is
  therefore three Sells, not one, which `docs/API.md` states (D-07). A Sell
  naming a parcel that does not exist yet persists nothing at all (D-11).
- **Figures.** Nil proceeds are legitimate and realise the whole cost base as a
  loss — the entry route for a gift under the market-value substitution rule,
  where the user supplies the market value (D-15, D-20); a negative price is
  refused, while costs exceeding the consideration report proceeds below nil
  under the documented sale-side cost convention (D-15). A non-AUD sale in a
  month with no imported rate falls back to the trade's own `fx_rate`, as
  documented, rather than failing or silently passing the figure through
  (D-16). The wash-sale report flags re-acquisitions on both sides of a loss
  sale with signed `days_apart` (D-08), and every parcel-optimiser strategy's
  allocations are accepted by `PUT /sells/:id` verbatim (D-18).

Both findings are closed, each archived in
[`DONE/tax-domain.md`](DONE/tax-domain.md) under a heading naming its scenario
id, and each naming the commit that closed it:

| Finding | Scenarios | Fixed by |
| --- | --- | --- |
| An AMIT adjustment covering part of a parcel is diluted across the whole parcel | D-13 | `04847a4` |
| A return of capital received on units already sold is not recorded anywhere | D-14 | `14c7e16` |

### Section E findings

Forty-four of the 51 came back correct, and the two structural reasons are worth
recording. The five action types that *create* trades (rights exercise, buy-back
participation, scrip exchange, demerge, worthless recognise) all write through
`sell::upsert_sell_in_tx` and `domain::rollover`, so every allocation invariant
section D verified holds for them too, and each group is frozen by its own
`trades.*_action_id` foreign key. The three that apply at *read* time (split,
bonus issue, return of capital) all go through one pipeline —
`corporate_action::adjustments` for the events, `domain::cost_base` for the
money — so a re-basing or entitlement rule is stated once and cannot disagree
with itself between reports. What each group turned on:

- **Return of capital (E-01–E-07).** The payment is per unit of the *listing*,
  so a holding spread across accounts is reduced parcel by parcel with no
  account rule of its own (E-01); six-decimal-place amounts survive to the last
  digit (E-02); a payment reaching nothing (no holding, or a holding entirely
  sold before it) changes nothing and reports nothing (E-06); and a non-AMIT
  trust's tax-deferred amount entered on the income row stays informational —
  the E4 cross-check flags it until the matching action exists, then clears,
  with the reduction counted once (E-05).
- **Splits and bonus issues (E-08–E-15).** A 1-for-1 split is a genuine no-op
  (E-08), a decimal ratio is carried exactly (E-10, 3.5-for-1 → 350 units), and
  a consolidation that doesn't divide keeps the exact fraction and still sells
  out to nothing — the re-base and its inverse agree to the last decimal place
  (E-09). A split *after* an AMMA statement's year end leaves its adjustment
  generation and reduction untouched, because both are stated in the statement
  year's basis (E-13).
- **Rights (E-16–E-24).** Entitlement is fixed by the record-date holding and
  shared by one pool: cumulative exercises stop at it (E-16), a sale of rights
  consumes it too (E-17), and a lapse of free rights is a nil/nil non-event that
  still consumes it (E-18). Selling the *shares* after the record date leaves
  the rights exercisable (E-21), a split between record date and exercise
  re-bases each exercise back into record-date units (E-12), a transfer between
  accounts nets to no change in the entitlement and the exercise lands in the
  chosen account (E-23), an out-of-the-money exercise is allowed with the rights
  cost in the parcel's cost base (E-22), and a renounceable premium is capital
  proceeds on the rights, never income (E-19) — with the non-renounceable case
  entered as unfranked income exactly as documented (E-20).
- **Buy-backs (E-25–E-31).** Capital proceeds are
  `max(price, market value) − dividend` in every combination probed: the
  pre-2022 shape with both components (E-25), the post-25-Oct-2022 shape with
  none — and no income row at all (E-26), a market value above the price (E-27),
  and a dividend equal to the whole price, which is accepted while one above it
  is refused (E-28). A scale-back is delete-and-re-enter, and the dividend
  follows the accepted quantity down (E-29); mixed-eligibility parcels split
  into their own discount buckets (E-30); and the 45-day rule measures the days
  the tendered units were held *before* the buy-back, so a long-held holding
  keeps credits above the small-shareholder threshold while a fortnight-old one
  loses them (E-31).
- **Rollovers (E-32–E-46).** Cost base and acquisition date carry across an
  exchange (E-32), through a second takeover (E-38), across three accounts
  (E-40) and into a demerger's two legs, which always sum exactly to the
  original — at 5.063% and at both extremes the write path allows (E-41, E-42).
  Mixed consideration apportions by market value and assesses only the cash side
  (E-33); fractional replacements are kept exactly (E-36); a trade dated on the
  exchange date is refused with the reason rather than silently mis-handled
  (E-37); an inherited head parcel keeps its death-date clock and market-value
  cost base through the demerge (E-43); and a later consolidation (E-44),
  worthless recognition (E-45) or rename (E-46) of either entity behaves as it
  would on any other parcel. Taking the takeover *without* rollover is the
  documented manual Sell + Buy, which works (E-34).
- **Worthless (E-47–E-51).** The recognise closes only what is still held
  (E-49), in any prior year (E-50); a revived company is simply bought into
  again as a new parcel, the recognised loss standing (E-47); a liquidator's
  final distribution lands as a G1 reduction or a C2 gain depending on which
  side of the cancellation it is paid (E-48); and a suspended listing with no
  declaration is never valued off a stale price — the holding reports
  `price_unavailable` and the health report flags the staleness (E-51).

The five findings, each raised as a `TODO.md` section naming its scenario ids:

| Finding | Scenarios | Status |
| --- | --- | --- |
| A return of capital in a currency other than its parcels' is accepted, then breaks every cost-base report | E-07, E-39 | open |
| A corporate action dated in the future is applied to today's holdings | E-14 | open |
| A return of capital on an AMIT listing double-reduces alongside the AMMA adjustment | E-04 | open |
| A duplicated corporate action is silently compounded | E-03, E-15 | open |
| Fractional entitlements are documented for splits and demergers but not for bonus issues or scrip exchanges | E-11, E-36 | open |

---

## A. Deletion and mutation ripple effects

The example that prompted this file, generalised. Every one of these asks:
what derived, linked, or reported fact silently keeps referring to the thing
that changed?

- **A-01** Delete a Buy trade that an AMIT adjustment was generated against.
  *Probe:* the adjustment row; the AMMA statement's generated set and its
  cross-check report; every open-parcels view; snapshots on and after the buy
  date; row history for both rows.
- **A-02** Delete a Buy that a Sell allocation consumes.
  *Probe:* refusal and its wording; whether the Sell is left under-allocated if
  the guard is bypassed by any other path (transfer, demerger, scrip exchange,
  worthless recognise, buy-back participation).
- **A-03** Delete a Buy that a *transfer-in* parcel descends from, in the
  destination account.
  *Probe:* whether the destination parcel becomes an orphan with a carried cost
  base nothing supports.
- **A-04** Delete a Buy whose parcel anchors a rights sale allocation.
- **A-05** Delete a Buy that a DRP residual chain threads through (the second
  of three chained reinvestments).
- **A-06** Delete the *middle* trade of a chain: buy → split → sell → AMIT
  adjustment against the split-rebased parcel.
- **A-07** *Shrink* (rather than delete) a Buy's quantity below the units
  already allocated to Sells.
- **A-08** Shrink a Buy's quantity below the units an AMIT adjustment was
  computed over.
- **A-09** Change a Buy's `trade_date` to a date after a Sell that allocates
  from it.
- **A-10** Change a Buy's `trade_date` backwards across a corporate action's
  record date, so the parcel now qualifies for a split/bonus/rights/ROC it
  didn't before.
  *Probe:* are the adjustment events recomputed on read, or were they
  materialised at write time and now stale?
- **A-11** Change a Buy's `trade_date` backwards across the 12-month line for
  an already-recorded Sell.
- **A-12** Change a Buy's `listing_id` while allocations, AMIT adjustments, or
  rights sales reference it.
- **A-13** Change a Buy's `holding_account_id` while a Sell in the old account
  allocates from it.
- **A-14** Change a Buy's `quantity` upward after a split has re-based it.
- **A-15** Change a Buy's price/brokerage after the year's tax return has
  notionally been lodged (i.e. after the FY is closed).
  *Probe:* does anything mark the affected prior-year reports as changed, or is
  a silent restatement possible?
- **A-16** Delete an income row that funded a DRP reinvestment.
- **A-17** Delete an income row that carries a residual carried into a *later*
  reinvestment.
- **A-18** Delete an AMMA statement that has generated adjustments.
- **A-19** Delete an AMMA statement whose adjustments have already reduced a
  parcel that has since been sold.
- **A-20** Delete a corporate action (split) after trades were entered on the
  post-split unit basis.
- **A-21** Delete a return-of-capital action after a G1 excess gain has been
  reported in a lodged year.
- **A-22** Delete a holding account that has been emptied by a transfer.
- **A-23** Delete a listing that still has closing prices but no trades.
- **A-24** Delete an FX rate row for a month that a lodged year's report
  converted at.
- **A-25** Delete a `cgt_settings` opening carried-forward loss after later
  years have consumed it.
- **A-26** Delete an ESS statement whose vest Buy has been transferred to
  another account and partly sold.
- **A-27** Delete an inheritance whose parcel has since been split and
  partially sold.
- **A-28** Delete an interest income row after the annual tax report for its
  year was generated and archived.
- **A-29** Delete an investment expense attributed to a listing that has since
  been renamed.
- **A-30** Delete an attachment's owning row and confirm cascade — then check
  the attachments index report and the row-history of the attachment table.
- **A-31** Re-`PUT` a trade with an identical body (no-op update).
  *Probe:* does it write a row-history entry, mark snapshots stale, and
  recompute a settlement date that was previously explicit?
- **A-32** Re-`PUT` a trade omitting `settlement_date` after the listing's
  exchange changed (the documented rename/settlement limitation).
- **A-33** Delete then immediately re-create a trade with the same id.
  *Probe:* do the guards that referenced the old id now silently point at the
  new row (allocations, adjustments, attachments, reinvestment links)?
- **A-34** Delete a Sell and check that every parcel it consumed returns to the
  open pool at the right remaining quantity, in the right account, on the right
  unit basis.
- **A-35** Delete a Sell that was the *only* disposal in a year whose capital
  loss was carried forward into a later year that has since been reported.
- **A-36** Delete a Sell created by a buy-back participation, and confirm the
  paired dividend income row goes with it (and nothing else).
- **A-37** Delete one leg of a transfer group directly via `/trades`.
- **A-38** Delete a demerger's head-parcel closing Sell directly.
- **A-39** Delete a listing rename that price history was fetched under.
- **A-40** Delete an exchange holiday that a stored `settlement_date` was
  computed around.
- **A-41** Delete an exchange that a listing references.
- **A-42** Delete a currency that a trade's `currency` or `brokerage_currency`
  references.
- **A-43** Delete a DRP enrolment period that covers an already-reinvested
  distribution.
- **A-44** Delete a closing price (errored) for a day a snapshot was blocked
  on, then regenerate.
- **A-45** Overwrite a manual closing price with a materially different figure
  and check every snapshot, period-performance window, and performance IRR that
  read the old one.

---

## B. Cost base construction and the adjustment pipeline

- **B-01** Buy with brokerage entered GST-inclusive; verify the element-2
  amount that lands in the cost base and the derived GST split.
- **B-02** Buy with brokerage in a *different currency* from the trade.
- **B-03** Buy with zero brokerage; buy with brokerage larger than the
  consideration.
- **B-04** A `statement_total` that reconciles exactly; one that reconciles
  only after cent-rounding; one that is a cent out; one that is out by the
  sign (Buy total entered as a Sell would compute it).
- **B-05** A parcel receiving an AMIT cost-base *increase* (excess of
  attribution over cash) rather than a decrease.
- **B-06** A parcel receiving AMIT decreases across several years that
  cumulatively exceed its cost base (CGT event E10).
  *Probe:* the nil floor, the E10 gain in the year the excess arises, whether
  the excess is discountable, and whether the *following* year's decrease
  starts from nil rather than going negative.
- **B-07** A return of capital exceeding a parcel's cost base (CGT event G1) in
  the same year as an AMIT E10 excess on a different parcel.
- **B-08** Return of capital *and* an AMIT decrease on the same parcel in the
  same year, in both orders of entry.
  *Probe:* is the result order-independent?
- **B-09** Return of capital on a parcel acquired *after* the record date but
  *before* the payment date.
- **B-10** Return of capital paid on a parcel that was sold between record date
  and payment date.
- **B-11** A split between the return-of-capital record date and its payment
  date (per-unit amount on which unit basis?).
- **B-12** Two splits, then a bonus issue, then a consolidation, on one parcel.
  *Probe:* the parcel's quantity, per-unit cost base, and total cost base at
  each step; the running balance in the listing activity ledger.
- **B-13** A consolidation producing a fractional parcel quantity (e.g. 7-for-10
  on 33 units).
  *Probe:* rounding, and whether fractional units survive to sale.
- **B-14** A bonus issue that is partly assessable (a dividend component) —
  outside the modelled non-assessable case.
- **B-15** A parcel whose cost base is reduced to exactly nil, then sold.
- **B-16** A parcel with a nil cost base receiving a further return of capital.
- **B-17** Incidental costs on the *sale* side (brokerage on a Sell) reducing
  capital proceeds vs adding to cost base — verify which the report does.
- **B-18** A Buy with a negative-after-rounding derived figure (price with more
  decimals than the money precision).
- **B-19** A parcel that has been transferred between accounts twice, then
  AMIT-adjusted, then sold.
- **B-20** Cost base of a rights-exercise parcel where the rights themselves
  were purchased on-market.
- **B-21** Cost base of a DRP parcel funded by a distribution whose cash
  component was in a foreign currency.
- **B-22** Cost base of a demerged parcel where the head entity advised a
  percentage that leaves a rounding remainder across many parcels.
- **B-23** A scrip-for-scrip exchange with a cash component where the cash
  exceeds the parcel's cost base.
- **B-24** Cost base carried through: inheritance → transfer → split → AMIT
  adjustment → partial sale, verifying at every hop.

---

## C. The 12-month CGT discount clock

- **C-01** Sell exactly 365 days after the buy; 366 days; on the anniversary
  date itself; the day after.
- **C-02** Buy on 29 February, sell on 28 February / 1 March of the following
  non-leap year.
- **C-03** Sell where contract date and settlement date fall on opposite sides
  of the 12-month line.
- **C-04** Bonus shares: sold 6 months after issue but 3 years after the
  original parcel.
- **C-05** Split-rebased units sold 6 months after the split, 3 years after the
  original buy.
- **C-06** Rights-exercise parcel sold 13 months after the *original* shares
  were bought but 2 months after exercise.
- **C-07** Scrip-for-scrip replacement parcel sold 6 months after the exchange,
  where the original was held 5 years.
- **C-08** Demerged parcel sold 6 months after the demerger (rollover chosen)
  where the head parcel was held 5 years.
- **C-09** Demerged parcel where the rollover was *not* chosen.
- **C-10** Transfer between holding accounts, then sell 3 months later — clock
  must not restart.
- **C-11** Inherited parcel: deceased acquired 10 years ago, death 2 months
  ago, beneficiary sells 1 month after transfer (s 115-30).
- **C-12** Inherited pre-CGT asset (market value at death) sold 3 months later.
- **C-13** ESS vest parcel sold 6 months after vest, where the grant was 4
  years earlier.
- **C-14** DRP parcel from a distribution reinvested 11 months and 25 days ago.
- **C-15** A parcel whose acquisition date is *deemed* (rollover, inheritance)
  and which also crosses a split — verify the clock survives both.
- **C-16** A partial sale from a parcel where some units are discount-eligible
  and some are not (can't happen within one parcel — verify the report doesn't
  invent it when the optimiser splits an allocation).
- **C-17** Crypto bought and sold either side of the 12-month line, with
  same-day settlement.
- **C-18** A sale dated *before* its parcel's acquisition date (rejected?
  reported? silently negative holding period?).

---

## D. Sells and parcel allocation

- **D-01** Sell allocating from parcels in a different holding account.
- **D-02** Sell allocating from a parcel of a different listing.
- **D-03** Sell allocating from a parcel acquired *after* the sale date.
- **D-04** Sell whose allocations sum to more than / less than the sell
  quantity, by one unit and by a fraction of a unit.
- **D-05** Sell with a zero-quantity allocation; with a negative one; with a
  duplicated parcel in two allocation rows.
- **D-06** Sell with 200 allocation rows (a long-held DRP holding).
- **D-07** Sell of the entire holding across three accounts in one transaction.
- **D-08** Sell at a loss where a Buy of the same listing occurred 3 days
  earlier and another 20 days later (wash-sale window).
- **D-09** Sell dated on the same day as the buy (same-day trade).
- **D-10** Two Sells on the same day drawing on overlapping parcels, entered in
  each order.
- **D-11** Sell entered *before* the Buy it allocates from exists.
- **D-12** Sell of a quantity larger than the total holding at the sale date
  but smaller than the total ever held.
- **D-13** Sell where the parcel was AMIT-adjusted *after* the sale date.
- **D-14** Sell where the parcel is later adjusted by a back-dated return of
  capital whose record date precedes the sale.
- **D-15** Sell with proceeds of exactly nil; with negative proceeds.
- **D-16** Sell in a foreign currency where the FX month has no imported rate.
- **D-17** Partial sale, then the remainder sold in a later financial year,
  then the first sale amended.
- **D-18** Sell allocations that the parcel optimiser produced, submitted
  verbatim — do they still validate?
- **D-19** Sell allocations naming a parcel closed by a scrip exchange the day
  before.
- **D-20** Sell into a market-value-substitution situation (a gift, entered
  manually per the documented limitation) — verify nothing silently uses the
  actual (nil) proceeds.

---

## E. Corporate actions

### Return of capital (CGT event G1)

- **E-01** ROC on a listing held in two accounts with different parcel sets.
- **E-02** ROC where the per-unit amount has 6 decimal places.
- **E-03** ROC recorded twice for the same date (duplicate entry).
- **E-04** ROC on an AMIT listing (should be the AMMA cost-base adjustment
  instead — is it refused?).
- **E-05** ROC where a non-AMIT trust's tax-deferred amount was also recorded
  on the income row (the E4 cross-check).
- **E-06** ROC on a listing with no holdings at the payment date.
- **E-07** ROC in a currency different from the listing's.

### Splits, consolidations, bonus issues

- **E-08** 1-for-1 split (a no-op) and its effect on stored quantities.
- **E-09** A consolidation followed by an immediate sale of the whole holding.
- **E-10** A split with a ratio expressed as a decimal (3.5-for-1).
- **E-11** A bonus issue where the entitlement produces a fraction rounded by
  the registry (cash in lieu of fractions).
- **E-12** A split applied to a listing that also has open rights entitlements.
- **E-13** A split between an AMMA statement's tax-year-end and the generation
  of its adjustments (the documented "different unit bases" refusal).
- **E-14** A split recorded with an effective date in the future.
- **E-15** Two splits recorded with the same effective date.

### Rights issues

- **E-16** Exercise part of the entitlement; then exercise the rest; then
  attempt to exercise one more unit.
- **E-17** Sell part of the entitlement and exercise the rest.
- **E-18** Let rights lapse entirely (nil proceeds).
- **E-19** Renounceable rights sold on-market at a premium; renounceable retail
  premium received from the underwriter (TR 2017/4).
- **E-20** Non-renounceable retail premium (documented as out of scope —
  verify the guidance holds and the alternative entry works).
- **E-21** Rights issue on shares that are sold between record date and
  exercise date.
- **E-22** Rights issue where the exercise price exceeds the market price
  (out-of-the-money) and is exercised anyway.
- **E-23** Rights issue over a parcel that has since been transferred to
  another account.
- **E-24** Rights exercised in a different currency from the underlying.

### Off-market buy-backs

- **E-25** A pre-25-Oct-2022-style buy-back with a franked dividend component
  and a market-value capital component.
- **E-26** A post-25-Oct-2022 listed-company buy-back with **no** dividend
  component (whole price is capital proceeds).
- **E-27** Buy-back where the market value exceeds the buy-back price (the
  market-value substitution).
- **E-28** Buy-back where the dividend component exceeds the price.
- **E-29** Buy-back scaled back by the company (fewer units accepted than
  tendered) — re-participating for the accepted quantity.
- **E-30** Buy-back participation from parcels with mixed discount eligibility.
- **E-31** Buy-back dividend's franking credits and the 45-day rule (units held
  only briefly before tendering).

### Scrip-for-scrip

- **E-32** Straight share-for-share exchange, whole holding.
- **E-33** Mixed consideration (scrip + cash per old unit), with the cash
  apportionment by market value.
- **E-34** Exchange where the taxpayer would prefer *not* to choose the
  rollover (is the choice modelled?).
- **E-35** Exchange where a parcel would make a capital *loss* (rollover is
  unavailable for losses).
- **E-36** Exchange producing fractional replacement units.
- **E-37** Exchange where the original listing traded on the exchange date
  itself.
- **E-38** Exchange followed by a second takeover of the replacement listing.
- **E-39** Exchange where the replacement listing is in a foreign currency.
- **E-40** Exchange of a listing held in three accounts.

### Demergers

- **E-41** Standard Div 125 demerger with an advised cost-base percentage.
- **E-42** Demerger percentage of 0.01% and of 99.99%.
- **E-43** Demerger where the head parcel was pre-CGT in the deceased's hands
  and inherited at market value.
- **E-44** Demerger followed by a consolidation of the demerged entity.
- **E-45** Demerger where the demerged entity is later declared worthless.
- **E-46** Demerger recorded, then the head entity renamed.

### Worthless / delisted

- **E-47** G3 liquidator's declaration, then the company is later revived and
  the shares have value again.
- **E-48** C2 cancellation on deregistration with a small final distribution.
- **E-49** Worthless recognition on a parcel already partly sold.
- **E-50** Worthless recognition dated in a *prior* financial year that has
  been reported.
- **E-51** Shares suspended for three years, never formally declared worthless
  — what does the portfolio/valuation report do?

---

## F. AMIT / AMMA

- **F-01** AMMA statement generated against parcels held at 30 June where a
  trade on 29 June has not yet been entered; enter it and re-generate with
  `replace`.
- **F-02** AMMA statement whose units held disagree with the parcels the system
  can see (the preview/confirm gate) — over and under.
- **F-03** AMMA statement covering a year where the fund was held in two
  accounts.
- **F-04** AMMA statement for a year in which the holding was fully sold before
  30 June.
- **F-05** AMMA statement for a year in which units were bought after the last
  distribution period.
- **F-06** Two AMMA statements for the same fund and year (an amended
  statement).
- **F-07** An AMMA statement with a `tax_year_end_date` that is not 30 June.
- **F-08** AMIT cash distribution rows entered for a year with no AMMA
  statement (the cash cross-check).
- **F-09** AMMA statement entered for a year with no cash rows.
- **F-10** AMIT attributed capital gains: discountable, non-discountable, and
  the "grossed-up" discountable amount — verify which the net capital gain
  report consumes and that it isn't double-discounted.
- **F-11** AMIT attributed *capital losses*.
- **F-12** AMIT foreign income with foreign tax paid, with a discountable
  foreign capital gain requiring FITO apportionment.
- **F-13** AMIT attribution exceeding cash received (cost base increase).
- **F-14** Cash received exceeding attribution (cost base decrease) —
  cumulatively to nil and beyond (E10).
- **F-15** An AMIT holding that is also DRP-enrolled.
- **F-16** AMIT adjustment generated per parcel where one parcel was acquired
  on 30 June itself.
- **F-17** AMIT adjustment where a parcel was transferred to another account
  mid-year.
- **F-18** AMIT adjustment on a parcel that is on a different unit basis (split
  mid-year).
- **F-19** Manual AMIT adjustment rows entered alongside generated ones for the
  same statement (the duplicate-parcel guard).
- **F-20** AMIT adjustment where the trade and the statement are in different
  holding accounts.
- **F-21** AMMA statement components entered on an income row instead (the
  notional-component refusal).
- **F-22** An AMIT listing receiving an ordinary franked dividend income row.
- **F-23** A fund that converts from a non-AMIT MIT to an AMIT mid-history —
  E4 for the earlier years, E10 for the later.
- **F-24** The adjustment cross-check report on a statement whose adjustments
  were generated, then one parcel deleted.
- **F-25** A multi-year AMIT holding sold in year 5: verify the sale's cost
  base reflects all four prior years' adjustments (the "most common ETF tax
  error").

---

## G. Dividends, franking, and the holding-period rule

- **G-01** Fully franked dividend at 30%; at the 25% base-rate-entity rate;
  partially franked; unfranked.
- **G-02** Dividend with TFN amounts withheld.
- **G-03** Dividend with conduit foreign income.
- **G-04** LIC capital gain deduction on a dividend.
- **G-05** Dividend paid on shares bought 40 days before the ex-date and sold
  10 days after (the 45-day rule fails).
- **G-06** The same, but total franking credits for the year are under $5,000
  (small-shareholder exemption).
- **G-07** Franking credits of exactly $5,000; of $5,001.
- **G-08** The 45-day count excluding the acquisition and disposal days, at the
  boundary (exactly 45 days at risk; exactly 44).
- **G-09** Preference shares requiring 90 days (is the distinction modelled?).
- **G-10** A holding built from several parcels where the LIFO rule determines
  which parcels are deemed sold for the holding-period test.
- **G-11** A partial sale between ex-date and payment date.
- **G-12** A dividend on shares that were transferred between holding accounts
  inside the qualification window.
- **G-13** A dividend on shares acquired by DRP inside the window.
- **G-14** A "related payment" obligation (out of scope? undetectable?) — and
  whether the report claims more certainty than it has.
- **G-15** Franking at-risk what-if run before a contemplated sale, then the
  sale actually recorded — do the two agree?
- **G-16** A dividend paid by a foreign company with foreign withholding tax.
- **G-17** A dividend paid in a foreign currency with the FX month missing.
- **G-18** A dividend declared in one FY and paid in the next.
- **G-19** A trust distribution with a present-entitlement date of 30 June paid
  15 July.
- **G-20** A trust distribution with an entitlement date in the *prior* FY to
  its payment, where the units were sold in between.
- **G-21** An `entitlement_date` supplied on a dividend (non-trust) row.
- **G-22** A dividend on a listing that was renamed between ex-date and payment.
- **G-23** `amount_per_security × securities_held` cross-check off by a cent,
  and off by a whole unit's worth.
- **G-24** Two dividends from the same company on the same payment date.
- **G-25** A dividend where the franking credit is entered but the dividend
  amount is nil.

---

## H. Interest, expenses, and other income

- **H-01** Australian-source interest with TFN withholding.
- **H-02** Foreign-source interest (US broker sweep) with foreign tax withheld.
- **H-03** Foreign tax withheld recorded on an Australian-source row and vice
  versa (the classification guard).
- **H-04** Interest received in a foreign currency.
- **H-05** Interest credited on 30 June but not available until 2 July.
- **H-06** Investment expense apportioned between income-producing and private
  use — the user's post-apportionment figure.
- **H-07** Investment expense attributed to a listing that is later renamed,
  demerged, or declared worthless.
- **H-08** Borrowing costs / prepaid interest spanning two financial years.
- **H-09** An expense larger than the year's investment income (a negative net
  position).
- **H-10** An expense dated in a year with no income at all.

---

## I. DRP

- **I-01** Enrol, unenrol, re-enrol across three periods, with distributions in
  each gap.
- **I-02** A distribution whose ex-date falls in an unenrolled gap.
- **I-03** Overlapping enrolment periods; a zero-length period; a period with
  no end date.
- **I-04** Reinvest a distribution where the residual carries into the next
  reinvestment, three times in a row, then undo the middle one.
- **I-05** Undo a reinvestment whose DRP trade has been sold.
- **I-06** Reinvest with `units × price` a cent off the available cash; a full
  unit-step off.
- **I-07** Reinvest a distribution in an account not enrolled, where *another*
  account is enrolled for the same listing.
- **I-08** Reinvest at a DRP price with a 4-decimal per-unit figure.
- **I-09** Partial DRP participation (documented out of scope) — verify the
  workaround produces a defensible cost base.
- **I-10** A DRP allocation that includes a discount to market price.
- **I-11** DRP on an AMIT fund where the distribution is cash-only for tax.
- **I-12** A DRP trade entered by hand via `PUT /trades` (the refusal).
- **I-13** Reinvestment followed by a split, then an undo of the reinvestment.
- **I-14** Reinvestment where the resulting units make the holding
  discount-eligible partway through a later sale.

---

## J. Employee share schemes

- **J-01** Taxed-upfront eligible scheme with the $1,000 reduction applied.
- **J-02** Taxed-upfront where total adjusted taxable income exceeds $180,000
  (the user's responsibility — verify the documentation and that the tool
  doesn't silently claim the reduction is correct).
- **J-03** Tax-deferred scheme vesting, with the ESS discount at the deferred
  taxing point.
- **J-04** The 30-day rule: shares sold within 30 days of the deferred taxing
  point.
- **J-05** Vest, then sell within 12 months (no discount) and after 12 months.
- **J-06** Vest, transfer to the personal broker account, then sell.
- **J-07** ESS statement edited after vesting: a Buy-derived field vs an
  income-side field.
- **J-08** ESS statement with AUD overrides on a foreign-currency grant (an
  ICE-style US RSU release).
- **J-09** ESS statement with TFN withholding.
- **J-10** Dividend equivalents on unvested RSUs paid in cash (documented out
  of scope — verify the manual income-row workaround classifies correctly).
- **J-11** Two vests on the same date from different grants.
- **J-12** A vest whose market value per share is stated in USD with the FX
  month unimported.
- **J-13** Forfeiture of unvested shares after a vest was recorded in error.
- **J-14** ESS discount reaching the tax summary and the annual tax report's
  Item 12 labels, for a year with two vests and a sale.

---

## K. Inherited parcels

- **K-01** Deceased acquired post-CGT: cost base at death, discount clock from
  the deceased's acquisition.
- **K-02** Deceased acquired pre-CGT: market value at death.
- **K-03** Death before 20 September 1985 (the refusal).
- **K-04** LPR expenditure added, with and without its date; date before the
  death.
- **K-05** Deceased's acquisition date after the death.
- **K-06** Inherited parcel sold 6 months after transfer, 8 years after the
  deceased acquired it.
- **K-07** Inherited parcel of an AMIT fund, then an AMMA statement covering
  the year of death.
- **K-08** Inherited parcel that is subsequently demerged.
- **K-09** Two beneficiaries splitting a holding (only one taxpayer is
  modelled — verify the documented boundary).
- **K-10** An inherited parcel edited after a Sell draws on it.

---

## L. Crypto

- **L-01** Buy and sell BTC with same-day settlement, either side of the
  12-month line.
- **L-02** Crypto-to-crypto swap entered as a Sell + Buy at market value.
- **L-03** Staking rewards entered as income + a Buy at receipt-date value,
  then sold.
- **L-04** An airdrop of an established token vs an initial-allocation airdrop.
- **L-05** A chain split / fork (documented out of scope).
- **L-06** Wrapping ETH → WETH.
- **L-07** A transfer between the taxpayer's own exchange accounts (not a
  disposal) entered as a holding-account transfer.
- **L-08** An exchange fee denominated in the crypto asset itself.
- **L-09** A crypto listing given an exchange MIC (the refusal); a non-crypto
  listing without one.
- **L-10** A crypto ticker that is not a recognised digital-token code.
- **L-11** Duplicate exchange-less tickers.
- **L-12** Crypto with 8+ decimal-place quantities through a sale allocation.
- **L-13** Crypto bought on 30 June and sold on 1 July (FY boundary with no
  settlement lag).
- **L-14** A stablecoin holding used as trading cash (Div 775 forex vs CGT —
  the documented deferral).
- **L-15** Crypto valued on a day the price provider has no candle for.

---

## M. Foreign currency and FX

- **M-01** A USD buy and USD sell in different rate months.
- **M-02** A trade with `spot_fx_rate` set, alongside another in the same month
  without one.
- **M-03** A `spot_fx_rate` on an AUD trade (the refusal); a non-positive one.
- **M-04** A trade in a month with no imported RBA rate, with and without a
  per-trade `fx_rate`.
- **M-05** Valuation in a month whose rate is missing but the prior month's is
  present (the 2-month provisional fallback) — and 3 months back (blocked).
- **M-06** A provisional snapshot trued up by a later RBA import — verify the
  `provisional` flag clears and the figures change.
- **M-07** A tax report that would need the provisional fallback (must stay on
  the strict path).
- **M-08** A holding in a currency that is redenominated / replaced.
- **M-09** A settlement-window forex movement (K10/K11, documented out of
  scope) on a large USD disposal.
- **M-10** A non-AUD parcel receiving a non-AUD AMIT reduction (the documented
  acquisition-month FX asymmetry).
- **M-11** FITO where total foreign tax is under $1,000; over $1,000 requiring
  the offset limit.
- **M-12** Foreign tax on a discountable foreign capital gain requiring
  apportionment.
- **M-13** An RBA import that re-imports an existing month with a *different*
  rate.
- **M-14** An RBA CSV with a missing month in the middle of the series.
- **M-15** Three currencies in one financial year's report.
- **M-16** Currency conversion in the period-performance FX attribution for a
  holding opened and closed inside the window (documented approximation).

---

## N. Holding accounts and transfers

- **N-01** Transfer a whole parcel; a partial parcel; several parcels at once.
- **N-02** Transfer with a network fee paid in units (a fee sale).
- **N-03** Transfer to the same account (the refusal).
- **N-04** Transfer of a parcel that a Sell in the source account already
  consumed.
- **N-05** Transfer, then sell in the destination, then delete the transfer.
- **N-06** Transfer of an AMIT-adjusted parcel, then a new AMMA statement in
  the destination account.
- **N-07** Transfer across a split date.
- **N-08** Transfer of an ESS vest parcel before the 30-day point.
- **N-09** Delete a holding account with only closed (fully sold) parcels.
- **N-10** Two accounts holding the same listing, one DRP-enrolled, receiving
  the same distribution.
- **N-11** Portfolio overview and performance with transfers in the window
  (must not distort portfolio-level figures).
- **N-12** A transfer dated before the parcel's acquisition date.

---

## O. Net capital gain, losses, and carry-forward

- **O-01** Current-year losses applied against non-discountable gains before
  discountable ones (the ATO-optimal order).
- **O-02** Prior-year carried-forward losses plus current-year losses in the
  same year.
- **O-03** Losses exceeding all gains — the excess carried forward.
- **O-04** An opening carried-forward loss entered in `cgt_settings`, consumed
  across three years.
- **O-05** A year with only AMIT-attributed gains and a carried-forward loss.
- **O-06** A year with a G1 excess gain (non-discountable) and a discountable
  parcel gain and a loss.
- **O-07** An E10 excess gain — verify whether it is discountable and where it
  lands.
- **O-08** A demerger-disregarded gain not entering the net figure.
- **O-09** A collectable loss entered as an ordinary listing (documented out of
  scope — verify the warning and the damage it would do).
- **O-10** A personal-use asset loss (disregarded).
- **O-11** Losses from a prior year that has since been amended (a back-dated
  Sell inserted).
- **O-12** A year where the net capital gain is exactly nil.
- **O-13** Discount applied *after* losses, not before (the ordering trap).
- **O-14** The pre-sale what-if for a disposal that would change which prior
  year's losses are consumed.
- **O-15** The parcel optimiser's "minimise assessable gain" vs "maximise
  discount-eligible" strategies producing different allocations — then verify
  the actual sale recorded either way.
- **O-16** The optimiser asked for more units than are open.
- **O-17** Net capital gain export CSV against the JSON, label by label.

---

## P. Tax summary, annual tax report, exports

- **P-01** A year with every income type present at once (franked, unfranked,
  trust, AMIT, interest AU + foreign, ESS, LIC, CFI, TFN withholding, foreign
  tax, expenses, capital gains and losses).
- **P-02** A year with no activity at all.
- **P-03** A year with activity only in June, and only in July.
- **P-04** The FY boundary: facts dated 30 June and 1 July, including a trust
  distribution with a 30 June entitlement date paid 15 July.
- **P-05** The annual tax report's completeness check with an AMIT fund held
  for part of the year and no AMMA statement.
- **P-06** The annual tax report for a year that also had a demerger, a
  buy-back, and a rights issue.
- **P-07** Tax summary vs annual tax report vs CSV export — the same figure in
  three places.
- **P-08** The CSV's second header row's myTax label mapping for every column.
- **P-09** A report generated, then a back-dated fact entered, then
  re-generated — is the difference visible?
- **P-10** The annual tax report printed to PDF with a 300-row realised gains
  table (no client-side pager truncation).
- **P-11** Tax figures for a year straddling a listing rename.
- **P-12** `taxpayer_basis` on every row (the single-individual assumption).

---

## Q. Prices, valuation, and snapshots

- **Q-01** A held listing whose provider has no candle for a real trading day.
- **Q-02** A delisted listing after its last trading day.
- **Q-03** A manual price entered for the wrong date (documented one-way).
- **Q-04** A manual price entered, then the provider starts serving that day.
- **Q-05** A price fetch on a public holiday not in the seeded calendar.
- **Q-06** A snapshot blocked by a missing price, then unblocked, then
  backfilled outside the 14-day window.
- **Q-07** A back-dated trade for a date with no price history (documented
  no-auto-backfill).
- **Q-08** Snapshot staleness triggered by each kind of fact write in turn
  (trade, allocation, income, AMMA, corporate action, price, FX).
- **Q-09** A new dated fact table added without staleness triggers (a
  code-review scenario, not a data one).
- **Q-10** Period performance over a window whose `from` has no valuation.
- **Q-11** Period performance where `from == to`; where `from > to`.
- **Q-12** Live valuation when the quote provider fails for one listing of ten.
- **Q-13** Live valuation of a foreign listing needing both a quote and an FX
  conversion.
- **Q-14** The portfolio overview graph across a period containing a split (the
  market value should not step).
- **Q-15** A snapshot regenerated after a rename (the documented display-only
  ticker drift).

---

## R. Listing identity and renames

- **R-01** Rename a ticker; rename an exchange; rename both.
- **R-02** A rename whose effective date precedes the newest existing rename.
- **R-03** A rename whose resulting ticker collides with another listing.
- **R-04** Undo a rename that is not the newest.
- **R-05** A `PUT` changing the ticker on a listing with trades (the refusal
  pointing at `/rename`).
- **R-06** Price backfill across a rename boundary (symbol resolved as at each
  date).
- **R-07** A rename followed by a demerger of the renamed entity.
- **R-08** A rename on a listing with an explicit `price_symbol`.
- **R-09** Settlement dates on trades before an exchange change (the documented
  live-exchange limitation), including a re-save.
- **R-10** The same ticker reused by a different company years later.

---

## S. Settlement, holidays, and dates

- **S-01** A trade on the Thursday before Good Friday on the ASX.
- **S-02** A trade on 24 December; on 31 December.
- **S-03** A trade whose settlement window crosses a holiday only on the
  *other* exchange the listing was formerly on.
- **S-04** A trade dated outside the seeded holiday coverage (the warning + the
  coverage report).
- **S-05** An explicit `settlement_date` before the trade date.
- **S-06** A crypto trade (same-day settlement, no calendar).
- **S-07** A trade on a US exchange around Thanksgiving / Juneteenth.
- **S-08** A trade dated on a weekend.
- **S-09** A trade dated 19 September 1985 and 20 September 1985 (the pre-CGT
  boundary).
- **S-10** A trade dated in the future.

---

## T. Jobs, backup, and operations

- **T-01** Two triggers of the same job overlapping (the per-job lock).
- **T-02** A job that fails, then succeeds — the bounded run history.
- **T-03** The backup verification failing (a corrupt copy quarantined).
- **T-04** Backup retention pruning with 20 backups spanning 18 months.
- **T-05** The price-import job for a market whose close moves with DST.
- **T-06** The RBA import when the feed is unreachable (502) and the manual
  CSV retry.
- **T-07** The RBA import's snapshot true-up when some snapshots are blocked.
- **T-08** The MIC import when a MIC's status changes to expired.
- **T-09** The currency import without DTIF credentials.
- **T-10** A `POST /jobs/:name` with an invalid suffix.
- **T-11** Server restart mid-job.
- **T-12** The health endpoint with a stale price date, a stale FX month, and a
  failed job at once.

---

## U. Audit trail and history

- **U-01** Every audited table's UPDATE and DELETE recording a prior row.
- **U-02** An attempt to UPDATE or DELETE `row_history` itself.
- **U-03** A migration adding a column to an audited table (trigger re-create).
- **U-04** Row history for a row deleted by cascade (an attachment).
- **U-05** Row history for a multi-row operation (a transfer, a demerger).
- **U-06** Reconstructing a superseded manual closing price and its provenance.
- **U-07** The row-history report for a table that is not audited.
- **U-08** Row history volume after 10,000 edits (report performance).

---

## V. Back-dated and out-of-order entry

The realistic pattern: a user enters history months later, out of order.

- **V-01** Enter a full year of trades in reverse chronological order.
- **V-02** Enter a Sell, then the Buy it should have allocated from, then fix
  the allocation.
- **V-03** Enter a corporate action *after* the trades it re-bases.
- **V-04** Enter an AMMA statement before any trades exist for the fund.
- **V-05** Enter a rename after price history was collected under the new
  symbol.
- **V-06** Enter an inheritance dated three years ago, after later trades exist
  on the same listing.
- **V-07** Enter a return of capital dated inside a period that has already
  been snapshotted and reported.
- **V-08** Enter a DRP enrolment period retroactively covering distributions
  already entered as cash.
- **V-09** Import a whole portfolio's history in one session and reconcile the
  final holdings against a registry statement.
- **V-10** A back-dated fact that changes a *prior* year's carried-forward loss
  and therefore every later year.

---

## W. Precision, rounding, and scale

- **W-01** A per-unit cost-base adjustment quoted to 10 decimal places (the
  Vanguard AMMA pattern).
- **W-02** A quantity with 8 decimals (crypto) allocated across 5 parcels.
- **W-03** A cent-rounding difference accumulating across 200 DRP parcels.
- **W-04** A money value that would lose precision through a `REAL` round-trip
  (a migration scenario).
- **W-05** A holding worth $10M and one worth $0.01 in the same report.
- **W-06** A split ratio producing a repeating decimal per-unit cost base.
- **W-07** Sum-of-parts vs total: parcel gains summing to the reported total
  exactly, at 4 and 2 decimal display precision.
- **W-08** A portfolio with 5,000 trades — report latency and payload size (no
  server-side pagination).

---

## X. Transactional integrity and concurrency

- **X-01** A Sell whose last allocation violates an invariant (nothing
  persisted).
- **X-02** A transfer whose fee sale fails after the transfer-out succeeded.
- **X-03** A scrip exchange interrupted between the closing Sell and the
  replacement Buys.
- **X-04** An AMIT generation interrupted midway (partial adjustment set).
- **X-05** A report read interleaved with a write that would make the snapshot
  inconsistent (an allocation whose parcel isn't in the same read).
- **X-06** Two concurrent Sells of the same parcel racing for the last units.
- **X-07** A snapshot generation racing a price correction.
- **X-08** A backup taken mid-transaction.

---

## Y. Web UI

- **Y-01** Every entity's list, form, and validation error toast.
- **Y-02** A 422 with a long, specific message (does the toast show it?).
- **Y-03** The allocation editor with 50 parcels.
- **Y-04** The corporate-action form switching between the 8 action types.
- **Y-05** The AMIT generation confirm gate showing a mismatch.
- **Y-06** The tax report printed with `@media print` (no filter row, no pager).
- **Y-07** The overview graph's date-range presets across a FY boundary.
- **Y-08** A table with 500 rows (client-side pager) sorted and filtered.
- **Y-09** Every hash route rendering with seeded demo data (the smoke check).
- **Y-10** A number rendered at the wrong precision for its column kind.
- **Y-11** The row-history screen for each audited table.
- **Y-12** The nav mega-menu with a newly added report.

---

## Z. Composite lifecycle scenarios

The hard ones: long chains where an error at step 2 only surfaces at step 9.

- **Z-01** **The 10-year ETF.** Buy VDHG in 2016; DRP-enrol in 2018; receive
  AMMA statements with cost-base decreases every year; a split in 2021; a
  partial sale in 2023 at a discountable gain; the balance sold in 2026.
  Verify the 2026 sale's cost base reflects all ten years of adjustments, that
  the DRP parcels each carry their own date and cost, and that the 2023 sale's
  figures did not change retrospectively.
- **Z-02** **The takeover chain.** Company A held 5 years → scrip-for-scrip
  into B with a cash component → B demerges C → C is declared worthless → B is
  consolidated 1-for-10 → the remaining B sold. Track the cost base and
  discount clock end to end.
- **Z-03** **The employee.** US-listed RSUs vest quarterly for 4 years into an
  employer plan account; dividends are paid on vested shares; some vests are
  transferred to a personal broker; a sell-to-cover happens within 30 days of
  one vest; the employer's ESS statement arrives after year end with figures
  differing from the vest entries; the whole holding is sold in year 5 across a
  USD/AUD move.
- **Z-04** **The estate.** A parent dies holding pre-CGT shares, post-CGT
  shares, an AMIT fund, and crypto; the beneficiary inherits all four; one is
  sold within 3 months, one after 18 months, the AMIT receives an AMMA
  statement covering the year of death, and the crypto is transferred between
  wallets.
- **Z-05** **The correction cascade.** A year is fully entered and its tax
  report archived; then a missed Buy from 14 months earlier is discovered.
  Enter it and follow the blast radius: allocations, AMIT adjustments, the
  discount split of the sale, the year's net capital gain, the carried-forward
  loss, every later year, the snapshots, and the archived report.
- **Z-06** **The multi-account, multi-currency year.** Three accounts, four
  currencies, a transfer between accounts mid-year, a foreign dividend with
  withholding, an AMIT fund with foreign capital gains, and a FITO limit
  calculation.
- **Z-07** **The wash sale.** Sell at a loss on 20 June, re-buy on 3 July;
  the loss is claimed in the earlier year. Verify the report flags it across
  the FY boundary and across holding accounts, and that nothing is silently
  rejected.
- **Z-08** **The rights round trip.** A 1-for-4 renounceable rights issue:
  exercise half, sell a quarter on-market, let a quarter lapse, then a bonus
  issue, then sell everything — with the original shares held in two accounts.
- **Z-09** **The buy-back before and after the law change.** The same company
  runs a franked off-market buy-back in 2021 and a capital-only one in 2024;
  the taxpayer participates in both from overlapping parcels.
- **Z-10** **The delisted fund.** An unlisted/suspended holding with no price
  for 18 months: snapshots, valuation, the overview graph, the health report,
  and eventually a G3 declaration.
- **Z-11** **The full financial year, reconciled.** Enter one complete FY of a
  realistic portfolio and reconcile every figure in the annual tax report
  against a hand-computed return: 11T/11U, 13Q/13U, 18A/18H/18V, 20E/20O,
  Item 12, question 10 L/M, and D7/D8.
- **Z-12** **The restatement.** Take Z-11's year, change one input fact
  (a corrected AMMA statement), and diff every report before and after.

---

## AA. Boundary and out-of-scope scenarios

Each of these is a documented limitation. The verification is that the system
**fails safe** — refuses, or flags, or documents — rather than silently
producing a wrong number, and that the documented workaround actually works.

- **AA-01** A pre-CGT parcel entered directly (the write-time refusal).
- **AA-02** Indexation elected instead of the discount for a pre-1999 asset.
- **AA-03** A gift of shares (market-value substitution) entered as a manual
  Sell.
- **AA-04** A collectable or personal-use asset entered as a listing.
- **AA-05** An SMSF or company taxpayer basis (33⅓% / 0% discount).
- **AA-06** Joint ownership of a parcel between two taxpayers.
- **AA-07** A share *trader* rather than investor (trading stock, not CGT).
- **AA-08** Cost-base elements 3, 4, and 5.
- **AA-09** A reduced cost base differing from the cost base.
- **AA-10** Partial DRP participation.
- **AA-11** A K10/K11 settlement-window forex outcome on a large USD trade.
- **AA-12** A Div 775 forex gain on a foreign-currency cash balance.
- **AA-13** A non-renounceable retail premium (an unfranked dividend).
- **AA-14** Rights over pre-CGT original shares.
- **AA-15** The estate/LPR side of a deceased estate.
- **AA-16** Unvested ESS grants and dividend equivalents.
- **AA-17** A chain split, wrapping, or the personal-use crypto exemption.
- **AA-18** Small business CGT concessions, main residence, or any non-listed
  asset class arriving in the data by mistake.
- **AA-19** A second taxpayer's holdings entered into the same database.
- **AA-20** An entity-level franking deficit or the related-payments rule.
