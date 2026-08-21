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

**One section is open below**, on **one** item, and that item needs a fact rather than code.

The audit-trail question that closing a fifth finding (Q-05/Q-08) raised — whether the hand-editable
`exchange_holidays` calendar, which valuation reads live, belongs in the audit trail — was **decided
and implemented 2026-08-21**: it does (migration 0039, with the surrogate id the composite key
needed), while `exchanges` stays out because its half of the original exclusion wording is genuinely
still true; that section is archived in `DONE/reviews.md`.

The **production-cleanup runbook** for LAC's borrowed price rows was **run against the deployed
database on 2026-08-21 and is complete** — 634 rows cleared, all audited, 1,128 snapshots
regenerated with 0 blocked, 2023-09-29's total down by exactly the A$11,123.21 that was another
company's price. It is archived in `DONE/reference-data.md`. The host upgrade it had been waiting on
was released the same day as **v0.12.0** (`54f4012`), 137 commits and 18 migrations on from v0.11.0.

That leaves the LAC price-history section open on the **demerger stated close** alone. Everything in
the repository that it was waiting for is now done: the `ok`-row delete rule was relaxed inside an
`unpriced_before` span (with a bulk form), every fetched row records the provider symbol it was
fetched under (migration 0038, so an overridden backfill can no longer pass for an ordinary one),
`GET /reports/health`'s `duplicate_price_series` detects the result itself, and the borrowed rows are
gone from the live database — leaving exactly one pre-demerger row to re-base. What is missing is
**what old Lithium Americas actually closed at on 2023-10-02**, a real-world observation the price
provider does not serve under any symbol; it has to come from a source Evan can cite.

---

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
- [x] **Nothing detects the result.** 635 consecutive byte-identical closes across two listings is
  not something two real securities do. A `reports::health` check in the `duplicate_*` family is the
  obvious shape, and it is the only signal this leaves for the 260 rows that carry no provenance.
  - Closed 2026-08-21 with `duplicate_price_series` on `GET /reports/health`. **The predicate is a
    run, not a total**, and that distinction is not academic: LAC and LAR really did both close at
    4.12 on 2024-02-08, four months after their series parted, so a count of matching days over all
    history would report the accident and the incident in the same breath. `identical_days` is the
    longest unbroken sequence of *comparisons* — days on which both listings hold an `ok` price —
    and the threshold is **30** (about six weeks of trading; `DUPLICATE_PRICE_SERIES_RUN_DAYS`). Two
    securities at a similar price level coincide to the cent on maybe one day in fifty; thirty in a
    row is that raised to the thirtieth power, so the rule is not a judgement call about how close is
    too close — no pair of distinct securities produces it. A day only one listing has a price for is
    **not a comparison**: it neither breaks the run nor counts towards it (so a run's span can exceed
    its length), while a day both hold and disagree on ends it. Prices compare as `Decimal`s, not as
    stored text, matching the rest of the family. One deliberate exemption: **a run whose closes
    never move is never reported**, however long — two instruments pinned to one figure (a pair of
    stablecoins at 1.00, two funds at a constant unit price) match forever without either series
    being a copy, and that would be a permanent alarm nobody could clear, which is the one failure
    mode this file has repeatedly judged worse than silence.
  - **Reported per pair with each side's rows split by origin**, the same call
    `demergers_missing_close` made and for the same reason — the two halves need different remedies:
    `fetched_days`/`manual_days` for the pair's lower listing id and `other_fetched_days`/
    `other_manual_days` for the higher. A hand-entered row states where it came from and why; a
    fetched row from before migration 0038 states nothing, and that half is what this check exists
    for.
  - **No suppression mechanism was invented**, consistent with the whole `duplicate_*` family: it
    clears when the borrowed rows go (the single-date `DELETE`, or the bulk
    `POST /closing_prices/clear_unpriced_before`) or are replaced by the listing's own series.
    `unpriced_before` deliberately does **not** quiet it — that marker explains an *absence*, which
    is why `errored_prices` honours it, whereas here the rows are present and are another security's,
    so the marker is a reason to delete them rather than to accept them.
  - Verified read-only against the deployed backup (`share-tracker-2026-08-16-000000.db`, upgraded on
    a scratch copy, served on a throwaway port): **one** row across all 28 listing pairs — listings 7
    (`LAC`) and 8 (`LAR`), `identical_days: 634`, 2021-03-25 → 2023-09-29, LAC `manual_days: 375` /
    `fetched_days: 259`, LAR 634 fetched / 0 manual. Every other pair scored a longest run of **0**.
    The 635th matching day is 2023-10-02, which is LAC's own demerger-adjusted close and differs from
    LAR's, so 634 — not 635 — is the run; the 2024-02-08 coincidence sits outside it, as intended.
    Rehearsed the remedy on the same copy: setting `unpriced_before = 2023-10-02` alone left the
    warning standing, and `clear_unpriced_before` (634 rows) cleared it.
  - Tests: `health::tests::two_listings_holding_one_price_series_are_reported` (the incident's shape,
    mixed origins, the later coincidence excluded, and nothing else in health seeing it),
    `…::scattered_identical_closes_and_a_short_run_are_not_reported` (the near-miss that matters: 49
    matching days that never run 30 deep), `…::a_day_only_one_listing_has_a_price_for_does_not_break_the_run`,
    `…::two_listings_pinned_to_one_price_are_not_reported` (both directions — silent while pinned,
    reported the moment the shared series moves),
    `…::api_the_warning_clears_when_the_borrowed_rows_go_not_when_the_marker_is_set` (over
    `GET /reports/health`), the addition to `…::empty_database_reports_nothing_stale`,
    `doc_checks::duplicate_price_series_check_documented` and the banner assertions in
    `web::tests::health_banner_ui_present`.
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
  - **Unblocked 2026-08-21**, but still open and still needing a fact this repository does not hold.
    The borrowed rows are gone from the deployed database (see the production-cleanup section below,
    which is now complete), so the objection above no longer stands: listing 7 has exactly **one**
    pre-demerger stored row left, 2023-10-02 at `10.13`, which is genuinely LAC's own
    demerger-adjusted figure, and `demergers_missing_close` now reads `adjusted_days: 1,
    manual_days: 0`. A stated close would therefore re-base that one row and nothing else, which is
    precisely what the mechanism is for.
  - What it still needs is `demerger_close_price`: **what old Lithium Americas actually closed at on
    2023-10-02**, the last pre-demerger trading day, in USD. That is a real-world fact, not something
    to derive or estimate — the whole point of the field is that it is a stated observation — and the
    price provider does not serve it under any symbol (which is what caused this entire section). It
    has to come from a source Evan trusts and can cite in `demerger_close_sourced_from`: a broker
    statement, a contract note, an exchange or financial-press record of the 2 October 2023 NYSE
    close. Once that figure is in hand, recording it on corporate action 1 re-bases the 2023-10-02
    row from `10.13` by the stated-close ÷ provider-figure factor, and clears the last
    `demergers_missing_close` entry.
