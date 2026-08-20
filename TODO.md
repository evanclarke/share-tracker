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
was the last of section Q's own findings. **No SCENARIOS finding is open**: the one section below is
the audit-trail question that closing a fifth (Q-05/Q-08) raised, which awaits its own decision.

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
