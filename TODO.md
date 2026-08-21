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

**One section is open below**, on two items — one needing a figure Evan must source, one a code
question. The LAC demerger was **re-modelled on the deployed database on 2026-08-21** so that
listing 8 (the continuing entity) is the head, which was the root cause of a long price/valuation
saga; **no tax figure moved**, proven by a byte-level diff of the FY2024 and FY2025 documents.

Everything else the 2026-08-20 price/valuation pass raised is closed. The `exchange_holidays`
audit-trail question was decided and implemented (migration 0039; `exchanges` stays out), archived in
`DONE/reviews.md`. The **LAC borrowed-price section** and the **production-cleanup runbook** are
archived in `DONE/reference-data.md` — the runbook was run on 2026-08-21, clearing 634 audited rows
and regenerating 1,128 snapshots. The host upgrade they waited on shipped the same day as **v0.12.0**
(`54f4012`), 137 commits and 18 migrations on from v0.11.0.

---

## The LAC demerger was modelled with the head and the new entity swapped — re-modelled 2026-08-21

Established 2026-08-21 from the company's own demerger notice (see the closed stated-close item in
`DONE/reference-data.md` for the quotations). The notice is unambiguous: the **continuing** entity is
Lithium Americas (Argentina) Corp — listing 8, `LAAC`/`LAR` — and Lithium Americas Corp (`LAC`,
listing 7) is **"a new corporation"** created by the Arrangement. Corporate action 1 recorded the
opposite: listing 7 as the head, listing 8 as `demerger_listing_id`. Evan asked for it to be fixed,
and it was, against the deployed database on 2026-08-21.

**No tax figure was ever wrong, and this corrects an earlier worry that one might be.** The
apportionment lands exactly where the notice puts it — LAC keeps 64%, LAAC receives 36%, and the
percentages attach to the *ticker*, not to the head/demerged role, so re-modelling moved nothing.
Both parcels inherit the 2021-03-25 acquisition date and were discount-eligible on the 2025-01-13
disposals. Proven rather than assumed: the whole FY2025 tax document, before and after, is identical
but for two `purchase_trade_id` values swapping roles (9073↔9074), and FY2024 is byte-identical.
LAC cost base A$12,716.33 / proceeds A$5,309.35 / loss A$7,406.98; LAAC A$7,152.93 / A$4,756.64 /
loss A$2,396.29, over a combined A$19,869.26. **Nothing needed re-filing.**

- [x] **Re-model so listing 8 is the head.** Done 2026-08-21 on the deployed database, entirely
  through the supported API — **no code change was needed**, and no raw SQL was used. Rehearsed first
  on a copy of a fresh `pre-remodel` backup, which predicted every figure. The four documented
  refusals all fire as designed and dictate the order (a Buy's listing is frozen while allocations
  reference it; demerge-group trades cannot be edited or individually deleted; the action is frozen
  while its trades exist; the group cannot go while its parcels are drawn on), so the sequence is:
  save both 2025 contract-note PDFs → `DELETE /sells/9075`, `/sells/9076`, then `/sells/9072` (which
  takes the whole demerge group) → `PUT /trades/9071` with `listing_id: 8` → `PUT
  /corporate_actions/1` as a `Demerger` on listing 8, `demerger_listing_id: 7`,
  `demerger_cost_base_pct: 64` → `POST /corporate_actions/1/demerge` → `PUT` both Sells back with
  their allocations crossed over → re-upload the PDFs. Trade ids were preserved throughout (the
  deletes returned the max id to 9071, so the rebuild reclaimed 9072–9076); the *roles* of 9073/9074
  swap, which is the whole point.
- [x] **Point the stated close at the series that was actually restated.** With listing 8 as head the
  existing mechanism reaches it: `demerger_close_date 2023-10-02` / `demerger_close_price 16.85`
  (old Lithium Americas' actual close that day, supplied by Evan and corroborated by the notice's own
  64/36 split against the observed post-separation closes) derives a factor of
  16.85 ÷ 6.453301 = **2.6111** and re-based **653 rows**, 2021-03-01 → 2023-10-02, in the write's own
  transaction. The recovered series is unmistakably the real undivided company: 2021-03-25 **15.24**
  (against Evan's own fill of 14.39 that day — a 5.9% intraday gap, where the old stored figure was
  an absurd 5.84), 2021-11-29 peak **43.06**, 2023-06-30 **21.24**. `demergers_missing_close` clears,
  and `GET /reports/health` is now **entirely clean**.
- [x] **The valuation history it was all for.** `regenerate_all` rebuilt **2,181 dates, 0 blocked**.
  Where 2,763 snapshot rows across **921 dates** (2021-03-25 → 2023-10-01) had the holding *excluded*
  as unvaluable, there are now **zero** excluded holdings and zero stale snapshots. 2022-09-19 went
  from a portfolio total of A$404,812.63 with LAC excluded to **A$454,637.56** with the holding
  valued at A$49,824.93; 2023-09-29 from A$484,306.31 to **A$513,349.75**. The 2021-03-25 holding
  values at A$21,032.06 against a cost base of A$19,869.26 — the first time the position's own
  history has been valuable at all.

Two things remain, one of them needing a figure from Evan:

- [x] **The 2023-10-03 row on listing 8 is in the provider's adjusted basis — and that is correct.
  This corrects the finding as first written, which had the arithmetic wrong.** The original claim
  was that the re-base walk is off by a day for demergers (the provider's adjustment runs through
  2023-10-03 inclusive, its ex-date being 2023-10-04) and that the row understates listing 8 by
  ~A$1,373. Both halves are wrong, and extending the walk would have been a real corruption.
  - **Un-adjusting the row yields the *combined* entity, which the model already counts twice over.**
    6.517713 × 2.611067 = **17.01818** is what the whole undivided company closed at on 3 October —
    NewCo included. But the demerge Buy is dated 2023-10-03, so listing 7 is *already held* that day
    at 9.67. Extending the walk would value the holding at 17.01818 + 9.67 = **26.69/unit** against
    the previous day's 16.85 — a 58% overnight jump, from double-counting the NewCo side. As it
    stands the two listings sum to **16.19/unit**, a 3.9% move, which is ordinary.
  - **The proposed repair figure was also too high.** 17.01818 − 9.67 = 7.348 assumed the when-issued
    9.67 exactly captures NewCo's value; the market priced standalone Lithium Argentina at **6.01**
    the next trading day and 6.09 the day after. The stored `6.517713` — Yahoo's spin-off factor
    applied to the combined close, i.e. its own standalone-equivalent estimate — is *closer to
    reality* than the repair. Entering 7.348 would have overstated the holding.
  - **So the boundary is right, and for a statable reason**: the walk stops strictly before the
    demerger date because **on that date the model already holds the demerged parcel**, so the head
    listing's price must be the standalone stub rather than the combined entity. A split reaches the
    same boundary by a different route (the price is already in the new basis on the effective date).
    It is not an off-by-one, and `price_basis_ratio`'s `e.date <= from` skip should be left alone.
  - **The residue, which is real but small and has no better answer**: on 2023-10-03 the stored price
    is not strictly "in its own trading day's unit basis" as the documented invariant has it, because
    standalone Lithium Argentina did not trade that day — the combined entity did, and what is stored
    is a derived standalone-equivalent. Every alternative is worse, so this is a documented exception
    rather than something to code around. The general shape is worth stating in `docs/API.md` beside
    the contemporaneous-basis rule: **on a demerger's own date the head listing's stored price is the
    provider's standalone-equivalent, not an observed close**, and that is deliberate. **Done
    2026-08-21**: `docs/API.md`'s Closing prices section now carries the exception as its own
    paragraph beside the "Which corporate actions restate the series" list — the strictly-before
    boundary, why leaving the row alone is right (the demerged parcel is already held that day, so
    un-adjusting would recover the combined entity), the LAC worked numbers, that a split reaches the
    same boundary by the other route, and that `demergers_missing_close` counts `adjusted_days` by
    the same boundary so the check and the walk agree. `docs/SCHEMA.md`'s `closing_prices.price`
    commentary, which asserts the invariant from the other side, carries the same qualification.
    Pinned by `doc_checks::demerger_date_price_basis_exception_documented`. No code changed.

- [ ] **Nothing detects a demerger modelled with the new entity as head** — the mis-modelling this
  section fixed. It was invisible for three years and was only caught because the price history was
  impossible; `demergers_missing_close` reported it as a missing stated close, which is a symptom
  rather than the defect. The signature is cheap to test for: the head listing has **no provider
  series before the demerger date**, which is what a newly created entity looks like and what
  `unpriced_before` now records explicitly. A health check in that shape would have named this
  directly. Note the general lesson for SCENARIOS section R (listing identity and renames): a
  demerger where the *new* entity keeps the original ticker breaks the "ticker continuity = entity
  continuity" assumption, and nothing in the model records which side of a demerger is the continuing
  legal entity.
