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

## A one-off `symbol` override stores another security's whole history under a listing, unguarded and undetected — and Evan's live LAC rows are the case

`POST /closing_prices/backfill`'s optional `symbol` is documented as "a one-off override for this
fetch only ... for a provider spelling the rename chain doesn't record", and the stored rows "land
under the listing's own `listing_id` either way, so history stays unified regardless of which symbol
fetched it". Nothing checks that the symbol names *the same security*, and nothing afterwards
notices that it didn't.

Found in the deployed database (backup `share-tracker-2026-08-16-000000.db`, read-only), and it is
not hypothetical:

- Listing 7 (`LAC`, held 2021-03-25 → 2023-10-03) carries **634** price rows for 2021-03-25 →
  2023-09-29, all `fetched_at` 2026-07-28.
- Every one of those 634 rows is **byte-identical** to listing 8 (`LAR`/`LAAC`)'s row for the same
  date — 634 identical, 0 differing, 0 missing. LAC's pre-demerger price history *is* LAR's price
  series.
- The cause is the override: Yahoo serves **no** `LAC` history before 2023-10-02 (a bare backfill of
  2023-09-20..29 answers HTTP 400 on every day — reproduced on a scratch DB against the live
  provider), so a previous session backfilled under the pre-separation symbol to unblock the dates.
  What came back was the other entity's series.
- Impact: **922 snapshot dates / 2,766 stored snapshot rows** value LAC at the wrong company's price.
  At 2023-09-29 the stored `portfolio_overview` row reads `market_value` A$11,123.21 against a
  `total_cost_base` of A$19,869.26 — a 44% unrealised loss — where old Lithium Americas closed near
  US$16.85, i.e. roughly A$27,400. **No tax figure is affected**: closing prices feed valuation only.
- The memory note "LAC pre-demerger prices unblocked but still ~2.46x understated" (2026-07-28) is
  this, and the 2.46 is the LAR-vs-old-LAC ratio, not a demerger adjustment factor.

- [ ] Nothing detects it. No cross-check asks whether two listings hold an identical price series,
  which is the one signal this leaves — 634 consecutive byte-identical closes between two listings is
  not something that happens to two real securities. A `reports::health` check in the
  `duplicate_*` family is the obvious shape.
- [ ] Nothing guards the write. Consider what is checkable at fetch time: the returned candles'
  **currency** against the listing's, and whether the override's series is already stored in full
  under *another* listing. Neither is conclusive, so this may be a warn-and-record rather than a
  refusal — but the override currently records nothing at all: the stored row's `source` says
  `yahoo` and the symbol that actually produced it is not kept anywhere, so after the fact there is
  no way to tell an overridden fetch from an ordinary one. That is the first thing to fix, and it is
  small: record the symbol the row was fetched under.
- [ ] The rows cannot be removed. `DELETE /closing_prices/:listing_id/:price_date` refuses an `ok`
  row by design, so 634 rows known to be the wrong security's are unremovable through the API;
  `POST /closing_prices/fetch` would replace them one day at a time, but for these dates it now
  errors, so it converts them to errored rows rather than clearing them. Whatever the repair path is,
  it needs to exist — the one-way rule was written for *real price data*, and these are not.

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

- [ ] Decide whether it deserves the counterpart. The two are not symmetric: carrying a close
  *forward* substitutes a real, once-observed price, while carrying one *backward* would invent a
  valuation for a period before any price existed. The honest options are probably (a) an
  `unpriced_before` date that makes valuation **exclude** the holding and flag the snapshot (so the
  portfolio total is explicitly partial rather than blocked or wrong), or (b) leave it blocked and
  document that a pre-listing period is un-snapshottable, which is what happens today.
- [ ] Whichever way, it interacts with the section above: for LAC the *correct* answer to
  2021-03-25 → 2023-10-01 is that no price is obtainable, and the wrong answer currently stored is
  another company's.

