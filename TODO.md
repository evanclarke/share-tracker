# TODO

Items are only marked done when a passing test exists for them.

Completed and decided (out-of-scope / not-reproducible) sections are archived in the topical `DONE/*.md` files, indexed by [DONE.md](DONE.md), to keep this list focused on active work. When a section here is fully done, move it into the matching `DONE/*.md` file rather than leaving it — see CLAUDE.md.

Sections here come from the **2026-07-13 improvement review** (a whole-project pass for
operational and test-strategy gaps, as distinct from the 2026-07-12 programming/domain review whose
findings are all closed in DONE.md), except where a section's heading names another source
(e.g. REQUIREMENTS, SCENARIOS). Each section records one finding; sections land in DONE.md as they
are fixed or decided.

**SCENARIOS.md sections A–P are driven and every finding they raised is closed** in the `DONE/*.md`
archive. Section **Q. Prices, valuation, and snapshots** was driven 2026-08-20: ten of its fifteen
scenarios came back correct outright — Q-01 (a provider gap on a real trading day is stored as an
errored row, blocks that date's snapshot with the error text quoted back, and is surfaced by
`GET /reports/health`'s `errored_prices`), Q-03/Q-04 (a hand-entered price is one-way exactly as
documented: `POST /closing_prices/fetch` refuses it 422 quoting the row's own `reason`, a backfill
counts it `already_stored`, and the validation refuses a non-trading day, a close that is not final,
a blank `sourced_from`/`reason` and a non-positive price), Q-06 (a date blocked by two missing prices
reports both blockers, then one, then generates — and the `report-snapshot` job's 14-day window
leaves an older date alone, which `regenerate_all` reaches with `regenerate_range` widening its
default `from` to the first-ever-held date), Q-07 (a back-dated Buy triggers no price backfill and
shows up as `unpriced_days` in health, as documented), Q-08 (**every** fact write stales the right
dates — trade insert, parcel allocation via a Sell, income, AMMA statement by its
`tax_year_end_date`, corporate action, a manual price replacing another, an
`rba_fx_rates` correction from the first of its month, and the `listings` `amit` flip — bar the one
table that was missing, `exchange_holidays`, since given its own trigger set by migration 0033),
Q-10/Q-11 (period performance refuses `from == to` and `from > to` with the same message, and names
the endpoint date it has no stored price for), Q-12/Q-13 (live valuation of
two holdings where one listing's quote fails: the failing row is left unvalued carrying
`price_unavailable` while the other still values, converts at the quote-month rate and flags
`fx_provisional` when it falls back), and Q-15 (a rename leaves the stored `performance` snapshot's
`ticker` label untouched and unstaled, and regeneration picks the new one up — the documented
display-only drift). A fourth, Q-14 — the price provider serves a split-adjusted history, so every
valuation of a pre-split date was out by the split ratio — is closed in the archive, as is Q-02 (a
still-held delisted or suspended listing blocked the whole portfolio's snapshots indefinitely), which
was the last of section Q's own findings. **No section-Q scenario is open.** The pass raised three further findings
of its own, and **all three are now closed** in `DONE/reviews.md`: that the contemporaneous-price
invariant did not hold across a **demerger** — the cause of an understatement already recorded
against Evan's real LAC history — and that there was no "unpriced *before*" counterpart to
`unpriced_from` for a security whose provider series *begins* at a date, closed 2026-08-20 by
`listings.unpriced_before` (migration 0037), which excludes the holding from the date's totals and
names what it omits; and that the hand-editable `exchange_holidays` calendar changed a reported
figure without being audited, closed 2026-08-21 by migration 0039.

**One section is open below**, and it is new — opened 2026-08-21 by the company demerger notice Evan
produced, which settled an entity question the whole LAC investigation had been guessing at.

Everything the 2026-08-20 price/valuation pass raised is now closed. The `exchange_holidays`
audit-trail question was **decided and implemented** (migration 0039, with the surrogate id the
composite key needed; `exchanges` stays out), archived in `DONE/reviews.md`. The **LAC borrowed-price
section** is closed and archived in `DONE/reference-data.md`, along with the **production-cleanup
runbook**, which was run against the deployed database on 2026-08-21: 634 rows cleared and audited,
1,128 snapshots regenerated with 0 blocked, and 2023-09-29's total down by exactly the A$11,123.21
that was another company's price. The host upgrade those waited on was released the same day as
**v0.12.0** (`54f4012`), 137 commits and 18 migrations on from v0.11.0.

The last item of that section — the demerger stated close — closed as **"do not record one"**. The
notice shows listing 7 (`LAC`) is *a new corporation* created by the Arrangement, not the continuing
entity, so it has no pre-demerger series for a stated close to un-adjust. That answer, rather than
the fix it was expected to be, is what opened the section below: the entity whose series the provider
*did* restate is listing 8, which the mechanism cannot be pointed at. **No tax figure is affected** —
the notice's 64/36 apportionment matches what is recorded, and FY2025 stands as reported.

---

## The tracker models the LAC demerger with the head and the new entity swapped, so the series that *was* restated cannot be reached

Established 2026-08-21 from the company's own demerger notice (see the closed stated-close item
above for the quotations). The notice is unambiguous: the **continuing** entity is Lithium Americas
(Argentina) Corp — listing 8, `LAAC`/`LAR` — and Lithium Americas Corp (`LAC`, listing 7) is **"a new
corporation"** created by the Arrangement. Corporate action 1 records the opposite: listing 7 as the
head, listing 8 as `demerger_listing_id`.

**No tax figure is wrong, and this corrects an earlier worry that one might be.** The apportionment
lands exactly where the notice puts it — listing 7 keeps 64%, listing 8 receives 36% — and both
parcels correctly inherit the 2021-03-25 acquisition date, so both were discount-eligible on the
2025-01-13 disposals. FY2025 stands as reported: LAC cost base A$12,716.33 / proceeds A$5,309.35 /
loss A$7,406.98, and LAAC A$7,152.93 / A$4,756.64 / loss A$2,396.29, over a combined A$19,869.26
cost base. Nothing here needs re-filing.

What is wrong is the **valuation history**, and two things follow from it:

- [ ] **The series the provider actually restated is listing 8's, and the stated-close mechanism
  cannot be pointed at it.** Listing 8 holds continuous history from 2021-03-01 through the
  separation, and its pre-separation rows carry the provider's spin-off adjustment on their face:
  `6.453301` at 2023-10-02 against clean quotes (`6.01`, `6.09`) from 2023-10-04. Old Lithium
  Americas' real closes are recoverable from it — the implied factor is 6.453301 ÷ 16.85 = 0.38299
  (against the notice's 36%, the difference being that the provider uses the realised market ratio,
  not the butterfly percentage), so the un-adjust multiplier is **× 2.6111**. That is precisely what
  `demerger_close_*` computes, but the four fields live on the *head* listing's action, and here the
  restated entity is the *demerged* one. The mechanism as built cannot reach it. Decide whether the
  right answer is to let a demerger state a close for its demerged listing too, or to fix the
  modelling below (which would make listing 8 the head and the existing mechanism sufficient).
- [ ] **`GET /reports/health`'s `demergers_missing_close` reports action 1 as needing a stated close
  when a stated close is exactly the wrong thing to record.** The check's rule is "a pre-demerger
  `ok` row the provider served after the demerger", which a *newly created* head listing's own first
  close satisfies while carrying no spin-off adjustment at all. Distinguishing them needs something
  the check does not currently know — e.g. that the listing has no provider history before the
  demerger date, which is the signature of a new entity rather than a restated one, and which
  `unpriced_before` now records explicitly. Until then this row is a standing false positive on
  Evan's data.
- [ ] **The deeper modelling question, which is the cause of both.** The tracker asserts Evan held
  listing 7 (`LAC`) from 2021-03-25; in fact he held old Lithium Americas, whose identity continued
  as listing 8. That is why LAC had no obtainable price before the separation, why 635 rows were
  borrowed from listing 8 to fill the gap, and why 922 snapshot dates now exclude the holding
  entirely. The truthful shape is the mirror image: listing 8 carries the 2021 parcel and the
  continuous price series, listing 7 begins at 2023-10-03 as the new interests. Re-modelling a real,
  already-disposed holding is not a small change and must not disturb the FY2025 figures above, which
  are correct as they stand — so it needs its own design pass, not an edit. Note the general lesson
  for SCENARIOS section R (listing identity and renames): a demerger where the *new* entity keeps the
  original ticker is the case that breaks the "ticker continuity = entity continuity" assumption, and
  nothing in the model currently records which side of a demerger is the continuing legal entity.
