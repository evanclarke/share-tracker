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

**There are no open sections below — this list is empty.** The last one closed on 2026-08-21 and is
archived in `DONE/reference-data.md`: the LAC demerger had been recorded with the head and the new
entity swapped, was **re-modelled on the deployed database** so that listing 8 (the continuing
entity) is the head — the root cause of a long price/valuation saga, with **no tax figure moved**,
proven by a byte-level diff of the FY2024 and FY2025 documents — and the last item of it added the
health check that names that mis-modelling directly (`demergers_head_not_continuing`) instead of
leaving it to be inferred from an impossible price history. New work comes from driving the next
**SCENARIOS.md** section (R. Listing identity and renames) or from a new REQUIREMENTS entry.

Everything else the 2026-08-20 price/valuation pass raised is closed. The `exchange_holidays`
audit-trail question was decided and implemented (migration 0039; `exchanges` stays out), archived in
`DONE/reviews.md`. The **LAC borrowed-price section** and the **production-cleanup runbook** are
archived in `DONE/reference-data.md` — the runbook was run on 2026-08-21, clearing 634 audited rows
and regenerating 1,128 snapshots. The host upgrade they waited on shipped the same day as **v0.12.0**
(`54f4012`), 137 commits and 18 migrations on from v0.11.0.
