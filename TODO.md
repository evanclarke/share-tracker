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
of its own, of which two are **closed** in `DONE/reviews.md`: that the contemporaneous-price
invariant did not hold across a **demerger** — the cause of an understatement already recorded
against Evan's real LAC history — and that there was no "unpriced *before*" counterpart to
`unpriced_from` for a security whose provider series *begins* at a date, closed 2026-08-20 by
`listings.unpriced_before` (migration 0037), which excludes the holding from the date's totals and
names what it omits.

**Three sections are open below.** The audit-trail question that closing a fifth finding (Q-05/Q-08)
raised is awaiting Evan's decision. The LAC price-history section is open on two items — nothing
detects two listings sharing a byte-identical series, and the demerger stated close must wait for
the borrowed rows to be cleared on the deployed database (clearing them is now *possible*: the
`ok`-row delete rule was relaxed inside an `unpriced_before` span 2026-08-21, with a bulk form; and
since 2026-08-21 every fetched row records the provider symbol it was fetched under, migration
0038, so an overridden backfill can no longer pass for an ordinary one). And the production-cleanup runbook for those rows is recorded
last: it is a procedure to run against the deployed database, not code to write here, and it names
the repo work it still waits on — now only the host upgrade.

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

- [x] **`reports::health`'s `demergers_missing_close` (added `16db704`) under-reports this by more
  than half, and excludes exactly the rows documented as inaccurate.** Its query filters
  `cp.origin = 'fetched'`, so on this data it answers `adjusted_days: 260, earliest_date:
  2022-09-20` where the affected span is 635 rows from 2021-03-25. Excluding manual rows is right
  for its *stated* purpose (a manual price is contemporaneous by declaration and is never re-based),
  but that makes the count read as the size of the problem when it is not. Either the check needs a
  second figure for manual rows in the span, or its wording must say what it is counting.
  - Closed with the **second figure**, matching the `duplicate_*` checks (which already publish a
    count alongside the ids): the two halves need different remedies, so hiding one serves nobody.
    One pass over the listing's pre-demerger ok rows now splits by `FILTER` into `adjusted_days` /
    `earliest_date` / `latest_date` (fetched, observed on or after the demerger — what a stated
    close re-bases) and `manual_days` / `manual_earliest_date` / `manual_latest_date` (hand-entered,
    whenever entered — what it does not). The observed-on-or-after test stays on the fetched half
    only: a manual row's `fetched_at` records when it was typed in. The row-existence rule is
    unchanged by a `HAVING` on the fetched count, so a demerger with only manual pre-demerger rows
    is still not listed — the manual figure is context on this warning, not a warning of its own
    (that is the "nothing detects the result" item below). Struct docs, `docs/API.md`'s Health
    section and the web banner all state what is counted and what is not. Verified on the deployed
    backup: LAC now answers `adjusted_days: 260, earliest_date: 2022-09-20` **plus**
    `manual_days: 375, manual_earliest_date: 2021-03-25` — the full 635-row span from March 2021.
    Tests: `health::tests::a_demerger_reports_hand_entered_prices_in_the_span_separately` (both
    kinds in one span, over `GET /reports/health`, asserting the earlier start), the no-manual
    assertions added to `a_demerger_with_provider_adjusted_prices_and_no_stated_close_is_reported`,
    a manual row added to `a_demerger_with_no_adjusted_prices_is_not_reported`, and the banner
    bundle assertions in `web.rs`.
- [x] **The `symbol` override records nothing.** It is documented as "a one-off override for this
  fetch only … for a provider spelling the rename chain doesn't record", and the stored rows "land
  under the listing's own `listing_id` either way". Nothing checks that the symbol names the *same
  security*, and — the fixable part — nothing keeps the symbol a row was fetched under, so an
  overridden fetch is indistinguishable from an ordinary one afterwards. Recording it is small and
  is the first thing to do; a returned-currency check against the listing's is a cheap second.
  - Closed 2026-08-21 with `closing_prices.fetched_symbol` (migration **0038**), the provider symbol
    each fetched row was actually fetched under. Two decisions worth stating. **Recorded always, not
    only when it differs** from the symbol the rename chain derives: the only-on-a-difference design
    makes NULL mean two things at once — "ordinary fetch" and "not recorded" — which *is* the
    ambiguity the incident consisted of, and the derived symbol isn't a fixed predicate to compare
    against anyway (a rename recorded later re-derives it). **Existing rows stay NULL**: the symbol
    a stored row was fetched under is recoverable from nothing the database holds — for the 260 LAC
    rows it is not even the listing's own ticker — so back-filling it from the rename chain would
    have invented the very fact the column exists to record. The migration comment, `docs/SCHEMA.md`
    and `docs/API.md` all say so. The value comes from the fetcher itself (a new
    `PriceFetcher::symbol`, the counterpart of `source`) rather than from `yahoo_symbol` beside it,
    so the symbol and the `source` it is stored next to are always in one namespace. Errored rows
    carry it too — a wrong symbol is the usual *cause* of the failure — and `db_store`'s upsert
    moves it with the figure, so a re-fetch under the right symbol cannot leave a row asserting the
    wrong one. A manual row carries none (nothing was fetched), CHECK-paired with `origin` the way
    0020 paired `sourced_from`/`reason` the other way round; the converse cannot be a CHECK, because
    a fetched row is legitimately NULL both for pre-0038 rows and for a fetch whose symbol could not
    be resolved at all. `ALTER TABLE ADD COLUMN`, not a fourth rebuild of this table (0020/0021/0034
    each rebuilt it) — the CHECK is expressible as a column constraint — but both
    `closing_prices_row_history_*` triggers were still dropped and re-created with the new column,
    and the staleness trigger is untouched (ADD COLUMN leaves triggers alone; the column feeds no
    valuation). The Closing Prices screen shows it as "Fetched under symbol".
  - **The "cheap second" was already there, and this verified rather than built it**: `fetch_and_store`
    has cross-checked the provider's returned currency against the listing's since the first price
    commit (`781b8cf`), and a mismatch is an **errored row for that date**, not a 422 — the same
    treatment as any other provider failure for a day, so the wrong figure is never stored and the
    reason is on the record. That is the right shape and was left as is; what was missing was a test
    on the *override* path, now added. Its limit is stated plainly in `docs/API.md`'s Known
    limitations: the currency check catches a symbol that reached another **market**, never one that
    reached another **security quoted in the same currency** — which is exactly the LAC/LAR case, and
    is why the symbol is recorded rather than merely checked. Detecting *that* is the next item in
    this section, not this one.
  - Tests: `entities::closing_price::tests::db_a_fetched_row_records_the_symbol_it_was_fetched_under`,
    `…::db_a_failed_fetch_records_the_symbol_it_was_attempted_under`,
    `…::db_each_row_records_its_own_segments_symbol_across_a_rename`,
    `…::api_backfill_records_the_overriding_symbol_on_every_stored_row` (the incident's own shape,
    including the re-fetch that replaces the record), `…::api_list_serves_the_symbol_a_row_was_fetched_under`,
    `…::api_a_manual_price_records_no_fetched_symbol`,
    `…::api_backfill_under_an_override_stores_a_currency_mismatch_as_an_error`,
    `infra::db::tests::migration_0038_leaves_existing_rows_unrecorded_and_pairs_the_symbol_with_the_origin`
    (existing rows unrecorded, the CHECK both ways, both audit triggers back with the new column, the
    staleness trigger still there), `doc_checks::fetched_symbol_provenance_documented` and
    `web::tests::closing_prices_ui_present`.
- [ ] **Nothing detects the result.** 635 consecutive byte-identical closes across two listings is
  not something two real securities do. A `reports::health` check in the `duplicate_*` family is the
  obvious shape, and it is the only signal this leaves for the 260 rows that carry no provenance.
- [x] **The rows cannot be cleared.** `DELETE /closing_prices/:listing_id/:price_date` refuses an
  `ok` row by design. The 375 manual ones can only be overwritten by another manual `PUT`; the 260
  fetched ones would be replaced one day at a time by `POST /closing_prices/fetch`, which for these
  dates now errors — converting them to errored rows rather than clearing them. The one-way rule was
  written for real price data; there is no path for data known to be another security's.
  - Closed 2026-08-21 by relaxing the refusal **inside an `unpriced_before` span only**, plus a bulk
    form. The one-way rule exists so the endpoint can never punch a hole in a *valued* series; a date
    before the marker is by declaration not valued — `stored_valuations` excludes the holding there
    whatever is stored, and the `unpriced_from` carry-forward is floored at the marker — so within
    the span the rule protects nothing and deleting is the acknowledgement that the stored figure
    never was a valuation. Everywhere else the refusal stands word for word, and `unpriced_from` is
    deliberately **not** relaxed: a date on or after *that* marker **is** valued, at the last stored
    ok close carried forward, so a delete there could remove the very figure being carried (pinned by
    `api_delete_still_rejects_an_ok_row_inside_an_unpriced_from_run`). No staleness handling was
    needed and none was added: setting or moving the marker is itself what stales the prefix, so the
    dates have already been regenerated without the rows by the time they go — and clearing the
    marker later stales the prefix again, after which regeneration reports each date blocked for want
    of a price, which is the truth once the rows are gone
    (`snapshot::db_clearing_the_superseded_prices_changes_no_stored_snapshot`).
    `POST /closing_prices/clear_unpriced_before` is the bulk form — 634 single-date DELETEs is not a
    runbook — and takes **no date range**: the span is read from the listing's own marker by the
    DELETE itself, so it cannot become a general bulk-delete of price history. One transaction, the
    row count reported, idempotent on re-run, and audited per row (the `AFTER DELETE` trigger fires
    once per row, so every figure and the manual half's `reason` stay in `row_history`). The Closing
    Prices screen offers Discard on a superseded row — manual ones included — and a "Clear superseded
    prices" card for listings carrying the marker. Rehearsed end to end on an upgraded copy of the
    deployed backup: marker set on listing 7, **634** rows cleared (375 manual + 259 fetched), 0 on a
    re-run, `row_history` up from 13 to 647 with the 375 rows' "unblocked, not accurate" note intact,
    2023-09-29's total falling from A$495,429.52 to A$484,306.31 — smaller by exactly the
    A$11,123.21 that was another company's price — with LAC named as excluded, and 2023-10-02 onward
    valuing LAC at its own price. Tests: the six new ones in `entities::closing_price::tests`
    (delete inside the span for both origins, refused on/after the marker, refused inside an
    `unpriced_from` run, the audited deletion, the bulk clear's exact span + idempotence, its two
    refusals, the per-row audit trail, the carry-forward left intact), the snapshot test above,
    `doc_checks::clearing_superseded_closing_prices_documented` and
    `web::tests::clear_superseded_prices_ui_present`.
- [ ] **The demerger stated-close fix (`16db704`) does not repair this listing, and this corrects
  the record.** Its factor is derived from the last pre-demerger stored row; here that row
  (2023-10-02, 10.13) is LAC's own demerger-adjusted figure while the 635 behind it are LAR's, so one
  factor cannot serve both. The mechanism is right and the 10.13 row is genuinely demerger-adjusted —
  but a stated close should not be recorded against this action until the 635 are dealt with, or it
  will scale the wrong series by a plausible-looking factor.

## PRODUCTION CLEANUP (not a development task): clear LAC's 635 borrowed price rows on the deployed database

**This section is an operational runbook, not code to write.** It is recorded here so the work is
not lost, but nothing in it is done by editing this repository — it is a procedure to run against
Evan's deployed database once the prerequisites below are released. Evan asked (2026-08-20) to clear
the rows and to resume the job another day.

### What is wrong with the data

Listing 7 (`LAC`, held 2021-03-25 → 2023-10-03) carries **635** price rows dated before its
demerger, and every one is byte-identical to listing 8 (`LAR`/`LAAC`)'s row for the same date — 635
identical, 0 differing, 0 missing. They are another security's prices. Yahoo serves no `LAC` candle
before 2023-10-02 at all (HTTP 400 on every earlier day), so there was nothing else to reach for at
the time. See the `## LAC's whole pre-demerger price history is LAR's series` section above for the
full evidence; in brief:

| Rows | Dates | `origin` | How they got there |
| ---: | --- | --- | --- |
| 375 | 2021-03-25 → 2022-09-19 | `manual` | Hand-copied from listing 8, with `sourced_from`/`reason` stating plainly that the period is "unblocked, not accurate" |
| 260 | 2022-09-20 → 2023-10-02 | `fetched` | `POST /closing_prices/backfill`'s one-off `symbol` override, which recorded nothing about the symbol used |

Effect: **922 snapshot dates / 2,766 stored snapshot rows** value LAC at LAR's price. At 2023-09-29
the stored `portfolio_overview` row reads `market_value` A$11,123.21 against `total_cost_base`
A$19,869.26 — a 44% unrealised loss — where old Lithium Americas closed near US$16.85 (≈ A$27,400).
**No tax figure is affected**: closing prices feed valuation only, never cost base or proceeds.

### Development prerequisites (these *are* repo work, and are not all done)

- [x] `listings.unpriced_before` — a date before which the provider has no series, excluding the
  holding from valuation and flagging the snapshot partial. Decided 2026-08-20 and **in progress**;
  see the `## There is no "unpriced *before*" counterpart` section for the decision and its
  reasoning. Tick this only once that section is closed and archived.
  - Done 2026-08-20 (migration 0037); the section is closed and archived in `DONE/reviews.md`. The
    snapshot carries `holding_excluded` plus an `excluded_holdings` list naming the absent holding
    and why, and setting it on listing 7 was rehearsed against an upgraded copy of the deployed
    database: 2023-09-29's total drops from A$495,429.52 to A$484,306.31, LAC's row unvalued with the
    reason. Note that the marker **supersedes** the stored rows for the span, so step 4 of the
    procedure is no longer what makes the totals honest — it is now housekeeping on rows nothing
    reads.
- [x] **An `ok` row must become deletable inside an `unpriced_before` span.** Today
  `DELETE /closing_prices/:listing_id/:price_date` refuses every `ok` row, so all 635 are
  unremovable through the API and this cleanup cannot be performed at all. The relaxation is
  principled and narrow rather than a general loosening: once a listing declares `unpriced_before`,
  dates in that span are *by declaration* not read by valuation, so deleting a stored price there
  cannot punch a hole in a valued series — which is the only reason the rule exists. A bulk form is
  probably wanted too; 635 single-date DELETEs is not a runbook.
  - Done 2026-08-21; see the "The rows cannot be cleared" item in the section above for the
    reasoning and the tests. The refusal is relaxed inside an `unpriced_before` span only (and
    deliberately **not** inside an `unpriced_from` run, where the last stored close *is* read),
    and `POST /closing_prices/clear_unpriced_before` is the bulk form — body `{ "listing_id": 7 }`,
    no date range, one transaction, idempotent, reporting the row count. The whole procedure below
    was rehearsed on an upgraded copy of the 2026-08-16 backup and its numbers are this section's.
- [ ] The deployed host must be upgraded first. It is at **migration 21**; this repo is past 36. The
  upgrade was rehearsed clean against a copy (21 → 36, every row count preserved,
  `integrity_check` and `foreign_key_check` clean, all seven annual tax reports and every
  cross-check report 200) — but none of it has been released.

### The procedure, once the above are released

1. Take a fresh backup (`POST /jobs/backup?suffix=pre-lac-cleanup`) — the deployed host's own job.
2. Rehearse the whole sequence against a **copy** of that backup before touching the live database.
3. `PUT /listings/7` with `unpriced_before: 2023-10-02`.
4. Clear the superseded rows in one request: `POST /closing_prices/clear_unpriced_before` with
   `{ "listing_id": 7 }`. It clears exactly the span the marker set in step 3 supersedes and answers
   `{ "listing_id", "unpriced_before", "deleted" }`; on the rehearsal `deleted` was **634**, and a
   second call reported 0. Note the count: 634, not 635 — the 635th is 2023-10-02 itself, which is
   *not* in the span (`price_date < unpriced_before`) and is the one row that is genuinely LAC's own
   (10.13, against listing 8's 6.453301 for the same day). That is the row a stated close would
   re-base, and it is meant to stay.
5. Regenerate the affected snapshots (`POST /report_snapshots/regenerate_all` with
   `{ "to": "2023-10-02" }`) and confirm the totals come back flagged partial rather than blocked.
   Leave `from` out rather than passing 2021-03-25: setting the marker stales the whole **prefix**
   before its date, which reaches back past LAC's own first holding to the first-ever-held date
   (2020-08-31 on the rehearsal — 206 dates a 2021-03-25 start would have left stale). The rehearsal
   regenerated 1,128 dates, 0 blocked, leaving only the 3 rows already stale in the backup.
6. Spot-check 2023-09-29: the LAC row should be absent and the total lower by A$11,123.21, with the
   snapshot naming LAC as excluded. On the rehearsal the stored `portfolio_overview` total went from
   A$495,429.52 to A$484,306.31, LAC's row carrying `price_unavailable` and the snapshot
   `holding_excluded` with `excluded_holdings` naming it; 2023-10-02 valued LAC at its own
   A$16,744.99. `GET /reports/health`'s `demergers_missing_close` then reads `adjusted_days: 1,
   manual_days: 0` — the single genuine row, which is what the stated-close item above is waiting
   for.

### Things to know before running it

- **Nothing is destroyed.** `closing_prices` is an [audited table](docs/SCHEMA.md), so every deleted
  row lands in `row_history` with its figure and its `sourced_from`/`reason` intact. The 375 manual
  rows' careful note about what they were survives; it just stops being read as a valuation.
- **The trade Evan accepted**, worth re-reading before running: 922 dates currently report a
  wrong-but-present total, and afterwards report a total that omits a holding he really owned. The
  true figures are not obtainable — old Lithium Americas' pre-separation closes are not served by the
  provider under any symbol. Neither state is right; the second is honest about it.
- **Do not record a demerger stated close on action 1 until this is done.** Its factor derives from
  the last pre-demerger stored row (2023-10-02, `10.13`), which is LAC's own demerger-adjusted figure,
  while the 635 behind it are LAR's — one factor cannot serve both, and stating one first would scale
  the wrong series by a plausible-looking number.
- **Where the data is.** The deployed database is on `bigbrain.lan:3000`. The copy in this repo
  (`share-tracker.db`) is **stale** — last written 2026-07-30, still at migration 21 — and is not the
  live data. The backup Evan fetched on 2026-08-20 is `share-tracker-2026-08-16-000000.db`; an
  upgraded scratch copy used for the rehearsals was at
  `scratchpad/upgrade-rehearsal.db` (session-local, will not survive).

