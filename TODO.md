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
was the last of section Q's own findings. **No section-Q scenario is open.** The pass raised two further findings
of its own: that the contemporaneous-price invariant did not hold across a **demerger** — the cause
of an understatement already recorded against Evan's real LAC history — which is **closed** in
`DONE/reviews.md`, and the audit-trail question that closing a fifth finding (Q-05/Q-08) raised,
which is the **one section left below** and is awaiting Evan's decision.

---

## `exchange_holidays` is a user-editable table that changes a reported figure, but is not audited

Raised 2026-08-20 while closing SCENARIOS Q-05/Q-08, which falsified the ground the exclusion rested
on. `docs/SCHEMA.md` excluded `exchanges` and `exchange_holidays` from the audit trail as "tables
that only influence values persisted onto trades at write time". That is true of `exchanges` (its
`settlement_days` is consumed when a trade is written; `timezone`/`close_time` decide only which
dates are *generable*, never what a stored snapshot says) but not of the holiday calendar:
`reports::valuation::stored_valuations` reads it **live** on every snapshot generation, which is why
migration 0033 had to give it staleness triggers.

The audited set's stated criterion (scope decision 2026-07-14) is "every user-entered table whose
values feed a calculation". `exchange_holidays` now visibly meets it, and this is the same shape as
the two tables that already joined on a falsified premise: `closing_prices` in 0021 (once 0020 made
a price hand-enterable) and `rba_fx_rates` in 0031 (once `PUT /rba_fx_rates/:id` made a rate
correctable). Both were "reference data" until a write path existed; this one has had `PUT`/`DELETE`
from the start.

- [ ] Decide whether `exchange_holidays` joins the audited set, and implement it if so. The case
  for: it is hand-editable, there is no import to re-derive it from (the seed is a one-off in
  0001_schema.sql), a deleted holiday is otherwise unrecoverable, and a wrong holiday silently
  changes both recomputed settlement dates and every snapshot valuation from that date. The case
  against: it is a published exchange calendar rather than a taxpayer fact, and the 0033 staleness
  triggers already surface the *effect* of a change on the reports even though they do not retain
  what was changed. `exchanges` should be decided in the same pass (probably staying out, per the
  reasoning above).
- [ ] If audited, it is the four-place change the rule names: a `row_history` rebuild to extend the
  `table_name` CHECK (the rename pattern of 0018/0021/0027/0031, `PRAGMA legacy_alter_table = ON`),
  the `exchange_holidays_row_history_update`/`_delete` trigger pair, an entry in
  `reports::row_history::AUDITED_TABLES`, and one in `config.js`'s table picker — three of which a
  test pins together. Note the composite `(mic, holiday_date)` key: `row_history.row_id` is an
  INTEGER, so the table would need the same AUTOINCREMENT surrogate id `closing_prices` was rebuilt
  with in 0021 (keeping the natural key as a `UNIQUE`), which makes this the larger of the two
  precedents, not the smaller.

## LAC's whole pre-demerger price history is LAR's series — 375 rows say so on their face, 260 say nothing, and the health check sees only the 260

Found in the deployed database (backup `share-tracker-2026-08-16-000000.db`). Listing 7 (`LAC`, held
2021-03-25 → 2023-10-03) carries 635 price rows dated before the demerger. **Every one is
byte-identical to listing 8 (`LAR`/`LAAC`)'s row for the same date** — 635 identical, 0 differing, 0
missing. They split into two halves that must not be confused:

- **375 rows, 2021-03-25 → 2022-09-19, `origin: manual`.** These were a *deliberate, documented*
  stopgap and the provenance says so in full. `sourced_from` reads "listing 8 (LAR) stored close for
  the same date — Yahoo's demerger-adjusted old-LAC series, identical to listing 7's own rows across
  their whole pre-demerger overlap (259/259 days)", and `reason` reads "Yahoo serves no LAC candle
  before 2023-10-02 … leaving 2021-03-25..2022-09-19 unpriceable and 544 snapshots permanently
  stale. Copied to unblock them. NOTE: demerger-adjusted, so about 2.46x below the actual old-LAC
  close of the day — **this period is unblocked, not accurate**." Nothing here is a mistake: the
  manual-price provenance mechanism did exactly its job, and it is the only reason the intent is
  reconstructible three weeks later.
- **260 rows, 2022-09-20 → 2023-10-02, `origin: fetched`.** These came from
  `POST /closing_prices/backfill`'s one-off `symbol` override — Yahoo serves **no** `LAC` history
  before 2023-10-02 (a bare backfill of 2023-09-20..29 answers HTTP 400 on every day, reproduced
  against the live provider), so the override was used to reach them and returned the other entity's
  series. Unlike the 375, these carry **no record of what produced them**: `source` says `yahoo`, and
  the symbol actually used is stored nowhere.

Impact: 922 snapshot dates / 2,766 stored snapshot rows value LAC at LAR's price. At 2023-09-29 the
stored `portfolio_overview` row reads `market_value` A$11,123.21 against `total_cost_base`
A$19,869.26 — a 44% unrealised loss — where old Lithium Americas closed near US$16.85 (≈ A$27,400).
**No tax figure is affected**: closing prices feed valuation only.

- [ ] **`reports::health`'s `demergers_missing_close` (added `16db704`) under-reports this by more
  than half, and excludes exactly the rows documented as inaccurate.** Its query filters
  `cp.origin = 'fetched'`, so on this data it answers `adjusted_days: 260, earliest_date:
  2022-09-20` where the affected span is 635 rows from 2021-03-25. Excluding manual rows is right
  for its *stated* purpose (a manual price is contemporaneous by declaration and is never re-based),
  but that makes the count read as the size of the problem when it is not. Either the check needs a
  second figure for manual rows in the span, or its wording must say what it is counting.
- [ ] **The `symbol` override records nothing.** It is documented as "a one-off override for this
  fetch only … for a provider spelling the rename chain doesn't record", and the stored rows "land
  under the listing's own `listing_id` either way". Nothing checks that the symbol names the *same
  security*, and — the fixable part — nothing keeps the symbol a row was fetched under, so an
  overridden fetch is indistinguishable from an ordinary one afterwards. Recording it is small and
  is the first thing to do; a returned-currency check against the listing's is a cheap second.
- [ ] **Nothing detects the result.** 635 consecutive byte-identical closes across two listings is
  not something two real securities do. A `reports::health` check in the `duplicate_*` family is the
  obvious shape, and it is the only signal this leaves for the 260 rows that carry no provenance.
- [ ] **The rows cannot be cleared.** `DELETE /closing_prices/:listing_id/:price_date` refuses an
  `ok` row by design. The 375 manual ones can only be overwritten by another manual `PUT`; the 260
  fetched ones would be replaced one day at a time by `POST /closing_prices/fetch`, which for these
  dates now errors — converting them to errored rows rather than clearing them. The one-way rule was
  written for real price data; there is no path for data known to be another security's.
- [ ] **The demerger stated-close fix (`16db704`) does not repair this listing, and this corrects
  the record.** Its factor is derived from the last pre-demerger stored row; here that row
  (2023-10-02, 10.13) is LAC's own demerger-adjusted figure while the 635 behind it are LAR's, so one
  factor cannot serve both. The mechanism is right and the 10.13 row is genuinely demerger-adjusted —
  but a stated close should not be recorded against this action until the 635 are dealt with, or it
  will scale the wrong series by a plausible-looking factor.

## There is no "unpriced *before*" counterpart to `unpriced_from`, which is the shape a spun-off entity actually has

SCENARIOS Q-02 (closed `c71e1f9`) added `listings.unpriced_from`: the provider *stopped* serving a
security from a date, so the last stored close is carried forward. The mirror image is real and
Evan has it: a security whose provider series **begins** at a date, with everything earlier
unavailable at any price.

New Lithium Americas' Yahoo series starts **2023-10-02** — a bare backfill of any earlier range
answers HTTP 400 (reproduced against the live provider). The Q-02 pass already met this and refused
it correctly rather than mis-handling it: its own note records that LAC's block "sits *before* its ok
series begins, i.e. an unpriced-**before** hole a carry-forward cannot reach", and `unpriced_from`'s
validation refuses to be used for it. So the system knows the shape exists and declines to answer it.

**The strongest argument for the counterpart is what happened in its absence.** The 375 manual rows
in the section above were entered because there was no way to say "unpriceable": their own `reason`
names the pressure exactly — "leaving 2021-03-25..2022-09-19 unpriceable and 544 snapshots
permanently stale. Copied to unblock them … this period is unblocked, not accurate." Offered a
choice between a permanently stale run of snapshots and a knowingly wrong number, a careful operator
took the wrong number and documented it. That is a missing feature, not a lapse.

- [ ] Decide whether it deserves the counterpart. The two are not symmetric: carrying a close
  *forward* substitutes a real, once-observed price, while carrying one *backward* would invent a
  valuation for a period before any price existed. The honest options are probably (a) an
  `unpriced_before` date that makes valuation **exclude** the holding and flag the snapshot (so the
  portfolio total is explicitly partial rather than blocked or wrong), or (b) leave it blocked and
  document that a pre-listing period is un-snapshottable, which is what happens today.
- [ ] Whichever way, it interacts with the section above: for LAC the *correct* answer to
  2021-03-25 → 2023-10-01 is that no price is obtainable, and the wrong answer currently stored is
  another company's.

