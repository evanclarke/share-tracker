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
display-only drift). Two of those findings are open below, alongside the audit-trail question that
closing a fourth (Q-05/Q-08) raised.

---

## SCENARIOS Q-14: the price provider serves split-adjusted history, so every valuation of a pre-split date is wrong by the split ratio

`YahooFetcher::daily_closes` asks `yfinance_rs` for `auto_adjust(false)`
(`src/entities/closing_price.rs:566`), which turns off *dividend* adjustment only. Yahoo's `close`
series is still **split-adjusted retroactively**: once a security splits, its whole history is
restated into the post-split basis. The reports go the other way — `domain::open_parcels` re-bases
each parcel's quantity into the **snapshot date's own** unit basis across any recorded `ShareSplit` /
`BonusIssue`. So a price backfilled after a split is in the *current* basis while the units it is
multiplied by are in the *historical* one, and the product is out by the split ratio.

Reproduced end to end against the running system (throwaway DB, real provider):

- Buy 100 NVDA on 2024-06-04 at US$1,150; `ShareSplit` 10-for-1 on 2024-06-10 (the real one);
  `POST /closing_prices/backfill` over 2024-06-05..2024-06-12; snapshots generated for each day.
- Yahoo returns **120.888** for 2024-06-07. NVDA's actual close that day was **US$1,208.88**.
- `GET /report_snapshots/series` then reads:
  `2024-06-07 = A$18,132.29`, `2024-06-10 = A$182,675.87` — a **tenfold step at the split date**,
  which is precisely what Q-14 says must not happen.
- The pre-split figure is the wrong one: the holding was worth 100 × US$1,208.88 ÷ 0.6667 =
  **A$181,322.93** on 2024-06-07, and the stored cost base for the same date is A$172,491.38 — so
  the `unrealised_gains` snapshot reports an **89.5% unrealised loss on a holding that was up**.

- [ ] Nothing about this is documented. `docs/API.md`'s [Closing prices](docs/API.md#closing-prices)
  section describes the stored figure only as "the close of every trading day", and there is no
  Known limitation for it — unlike the neighbouring price/valuation caveats (*Intraday prices*, *A
  manually entered price is one-way*, *Snapshot ticker labels…*), which are all recorded.
- [ ] **The stored series is internally inconsistent, which is what makes this more than a one-line
  fix.** A day fetched *before* the split keeps the contemporaneous (unadjusted) close forever —
  `run_collection` and `backfill` both skip a day already stored `ok` — while a day fetched *after*
  it holds the adjusted one. So one listing's history can hold both bases with nothing on the row
  saying which. Two ordinary paths introduce the mix on their own: an **errored** day before a split
  that the daily `price-import` job re-attempts after it, and `POST /closing_prices/fetch` on a
  pre-split day (which, being an `ok`→`ok` price change, *stales* the snapshots on and after it, so
  they regenerate at the wrong figure).
- [ ] Scope: valuation only. Closing prices feed `reports::valuation::stored_valuations` and the
  live-quote path, so this reaches `portfolio_overview` / `unrealised_gains` / `performance` (live,
  stored and snapshotted), `report_snapshots/series` and the Portfolio Overview graph, and
  `period_performance`'s `capital_growth`. **No tax figure reads a closing price** — cost base and
  proceeds come from the trades — so no CGT or income figure is affected. A consolidation (reverse
  split) fails the same way in the opposite direction, and a `BonusIssue` too, since Yahoo restates
  for those as well.

**Fix — Evan chose 2026-08-20: option (a), the contemporaneous basis** (over storing the provider's
current-basis figure and re-basing units forward at valuation, and over the documentation-only cut).
A stored closing price is *the price the security traded at on its own date*, which is what a
pre-split-fetched row already holds and what the Closing Prices screen implies. So: normalise on the
way in, re-base already-stored earlier prices when a split or bonus issue is recorded, and leave
`reports::valuation` alone. Existing rows need a one-off repair, and a re-fetch is no longer
byte-identical to the provider — both accepted.

**Options as put:**

- **(a) Stored prices are in the price date's own contemporaneous basis** (what a pre-split-fetched
  row already holds, and what the Closing Prices screen implies). Normalise on the way in: multiply
  the provider's figure by the cumulative ratio of every recorded `ShareSplit`/`BonusIssue` dated
  *after* the price date, and re-base every stored price dated before an action when that action is
  recorded, so entry order can't matter. Valuation then multiplies as-at-date units by an
  as-at-date price and needs no change.
- **(b) Stored prices are in the listing's current basis** (what the provider serves, so a re-fetch
  is always idempotent). Re-base the *stored prices before* a split when the split is recorded, and
  change valuation to multiply the price by units re-based into the **current** basis. Smaller
  arithmetic surface, but the Closing Prices screen then shows a figure that is not what the security
  traded at that day.
- **(c) Document it as a Known limitation** and add a `reports::health` warning when a listing has a
  recorded split or bonus issue with stored prices spanning it — no re-basing, the operator prices
  the affected days by hand.

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

## SCENARIOS Q-02: a still-held delisted or suspended listing blocks the whole portfolio's snapshots indefinitely, undocumented

A listing whose provider serves nothing after its last trading day stores an errored row for every
subsequent trading day, and `stored_valuations` fails the **whole** date if any held listing is
unpriced — deliberately, the no-partial-result rule. So one suspended holding stops
`report-snapshot` for the entire portfolio, every day, and `GET /reports/health` nags with a growing
`errored_days` count.

Reproduced: `POST /closing_prices/backfill` for a delisted US ticker (`ATVI`, 2024-06-05..07) stores
three errored rows (`yahoo fetch for ATVI failed: Not found`), and a held listing in that state
blocks `POST /report_snapshots/generate` for every date.

The system fails safe and the way out exists — the manual price, whose own `reason` placeholder in
`app.js` is literally "provider serves no candle since the delisting". But:

- [ ] That way out is **one hand-entered price per listing per trading day, forever**, for a
  suspension that can run for years, and nothing says so. There is no listing-level "no longer
  priced" fact (only `WorthlessShares`, which ends the holding — wrong for a suspended-but-valuable
  security), and `DELETE /closing_prices/:listing_id/:price_date` explicitly does *not* unblock the
  date, only the health alarm.
- [ ] Not in the Known limitations, and not in the [Closing prices](docs/API.md#closing-prices)
  section, which describes hand-pricing as the answer to "a day the provider can never serve" —
  singular — without saying what an unbounded run of such days costs.

**Fix — Evan chose 2026-08-20: a per-listing dated "unpriced from" fact** (over documenting the cost
alone, and over silently carrying the last stored close forward for *any* unpriced day, which would
weaken the no-partial-result rule everywhere rather than at the one listing that needs it). The
listing records the date from which the provider serves nothing: collection stops fetching it,
`GET /reports/health` stops nagging, and valuation stops blocking the whole date on it — carrying
the last stored close forward and flagging the snapshot, the way the provisional-FX fallback already
works, so the substitution is never silent. Migration + listing field + collection/valuation/health
branches + `docs/API.md`, `docs/SCHEMA.md` and README.
