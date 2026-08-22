# TODO

Items are only marked done when a passing test exists for them.

Completed and decided (out-of-scope / not-reproducible) sections are archived in the topical `DONE/*.md` files, indexed by [DONE.md](DONE.md), to keep this list focused on active work. When a section here is fully done, move it into the matching `DONE/*.md` file rather than leaving it — see CLAUDE.md.

Sections here come from the **2026-07-13 improvement review** (a whole-project pass for
operational and test-strategy gaps, as distinct from the 2026-07-12 programming/domain review whose
findings are all closed in DONE.md), except where a section's heading names another source
(e.g. REQUIREMENTS, SCENARIOS). Each section records one finding; sections land in DONE.md as they
are fixed or decided.

**SCENARIOS.md sections A–R are driven and every finding they raised is closed** in the `DONE/*.md`
archive. Section **S. Settlement, holidays, and dates** was driven 2026-08-22 against a throwaway
database, and **the T+n arithmetic itself was right in every scenario**: S-01 (Thu 2026-04-02 before
Good Friday on the ASX → 2026-04-08, skipping Good Friday, the weekend *and* Easter Monday), S-02
(2026-12-24 → 2026-12-30 over Christmas and the observed Boxing Day; 2026-12-31 → 2027-01-05 across
the year boundary), S-03 (the exchange-change calendar drift reproduced to the day on a re-save —
2026-06-10 recomputed to 2026-06-09 once the listing moved to an exchange without that holiday —
which is the [documented live-exchange limitation](docs/API.md#known-limitations), pinned by
`doc_checks::known_limitations_document_exchange_change_recomputation`), S-05 (an explicit
`settlement_date` before the trade date is refused `422 settlement_date cannot be before the trade
date`), S-06 (a Crypto trade settles same-day with no calendar and no coverage warning), S-07 (NYSE
2026-11-25 → 2026-11-30 over Thanksgiving; 2026-06-18 → 2026-06-23 over Juneteenth), and S-09 (the
pre-CGT boundary is exact — 1985-09-19 refused on both `PUT /trades/:id` and `PUT /sells/:id`,
1985-09-20 accepted and costed). The coverage machinery works on its own terms too: an out-of-range
window logs the `WARN` and is listed by `GET /reports/settlement_holiday_coverage` as
`outside_holiday_coverage` with the seeded span, an exchange with no seeded holidays at all reports
`no_holiday_coverage`, a window that *starts* inside coverage and settles outside it is caught
(2027-12-30 → 2028-01-03), and the `exchange_holidays` audit trail records the prior row on a
delete.

**Every finding below is about a date the system accepts, never about the arithmetic it then does.**
Four are open, and each records the option Evan chose on 2026-08-22 along with what was rejected.
Each was checked read-only against the live database (`share-tracker-2026-08-16-000000.db`) before
any write-time refusal was proposed, per the section-M lesson — that check is what turned the S-05
decision away from a refusal.

---

## SCENARIOS S-08: a trade may be dated on a day its exchange did not trade, and nothing refuses or flags it

`PUT /trades/:id` and `PUT /sells/:id` accept any date from 1985-09-20 on. Driven against the
running system:

- Buy on **Saturday 2026-05-16** (XASX) → `204`, settlement 2026-05-19.
- Buy on **Good Friday 2026-04-03** (a seeded `exchange_holidays` row for XASX) → `204`,
  settlement 2026-04-08.
- Buy on **Christmas Day 2026-12-25** (seeded) → `204`, settlement 2026-12-30.
- Sell on **Saturday 2026-08-15** → `204`.

None of these days exists on the exchange's own calendar, which the database already holds and
which the settlement calculation reads on the very next line. The same calendar is *already*
enforced one entity away: `PUT /closing_prices/:listing/:date` refuses exactly this with
`422 2026-06-06 is not a trading day` (`closing_price::validate_complete_trading_day` over
`Market::is_trading_day`), and that helper resolves the calendar **as at the date** through the
rename chain and returns `true` unconditionally for an exchange-less (Crypto) listing, so it is
already the right shape for a trade too.

What rides on the trade date makes this more than a tidiness point: it is the CGT event date, so it
sets the 12-month discount clock, the financial year the gain falls in, and the day the T+n count
starts from. A date the market was shut is a data-entry error by construction.

**Live database: zero rows disagree** — no trade in Evan's 113 is dated on a weekend or on a seeded
holiday for its exchange, so a write-time refusal would leave every existing row editable.

- [x] Decide the shape (see the two options below) and implement it — (c) both. The refusal lives in
      `trade::db_upsert` and `sell::db_upsert_sell`, on the transaction each already opens (the
      calendar is a DB read, so it cannot go in the pure `check_amounts`), over the *existing*
      `closing_price` machinery: a new `non_trading_day(&Market, date)` beside
      `validate_complete_trading_day`, resolving the calendar as at the date through the rename
      chain and exempting exchange-less (Crypto) listings. `load_market` gained a
      `load_market_on(conn, …)` twin so the write path can read it inside its own transaction
      (`http::crud_get`, `listing::db_get`, `exchange::db_get` and `db_holiday_dates_for` are
      executor-generic for it). Deliberately **not** in `check_amounts` and **not** in
      `upsert_sell_in_tx`: a corporate action's own date may legitimately fall on a closed day
- [x] The derived Buy paths must be settled either way: `ess_vest` and `inheritance` **INSERT their
      Buy directly** rather than through `trade::db_upsert`, each carrying a module-doc list of
      "which `check_amounts` rejection is satisfied where" and an explicit instruction that *a new
      check added to `trade::check_amounts` needs a line here*. Both are dated by facts that are
      routinely **not** trading days — an ESS taxing point, and a date of death — so a check added to
      `check_amounts` must exempt them (and say why in those lists), or live outside it
      — done the second way: the check lives outside `check_amounts` entirely, and both module-doc
      lists carry a paragraph saying so and why (a taxing point is set by the scheme, a death keeps
      no exchange's hours), pointing at the health alert that covers them instead
- [x] `docs/API.md` — the 422 catalogue row and the trades/sells sections, or the health section
      — all of them: the catalogue row beside the pre-CGT/future-date bounds, a Trades paragraph
      (with the derived-path exemption), a Sells paragraph, and the `non_trading_day_trades` health
      entry; plus `docs/SCHEMA.md`'s `trades.date` comment and the README health bullet
- [x] Regression tests: weekend and seeded-holiday dates on `PUT /trades/:id` and `PUT /sells/:id`,
      the same Saturday accepted for a Crypto listing (the `L-15` shape), and a date in a year with
      no seeded calendar still accepted
- [x] The non-blocking half: `reports::health`'s `non_trading_day_trades`, over *every* trade rather
      than only the ones the refusal sees, naming the reason (weekend / holiday), the exchange whose
      calendar was in force on the date, and the write path that created the row

**Decision (Evan, 2026-08-22): (c) both.** Refuse on `PUT /trades/:id` and `PUT /sells/:id`
(Crypto and the derived ESS-vest / inheritance paths exempt), **and** carry a non-blocking
`reports::health` alert so a non-trading-day row a derived path writes is still surfaced.
Rejected: (a) refusal alone (nothing would surface the derived paths' rows), (b) the alert alone
(weaker than the rule `closing_prices` already enforces on the same calendar). The accepted cost is
that an off-market allotment dated on a closed day can no longer be entered through `/trades`.

---

## SCENARIOS S-10: a trade may be dated in the future, and a financial year that has not happened then appears in the annual tax report's year picker

`PUT /trades/:id` and `PUT /sells/:id` accept any future date. Driven against the running system on
2026-08-22:

- Buy dated **2027-06-01** → `204`, settlement 2027-06-03.
- Buy dated **2028-03-01** → `204`; Buy dated **2028-04-13** → `204`.
- Sell dated **2027-06-01** → `204`, allocating a real parcel.
- Crypto Buy dated **2030-01-01** → `204`, settlement same day.
- `GET /reports/tax-report/years` then answers **`[1986, 2026, 2027, 2028, 2030]`**, and
  `POST /reports/tax-report {"tax_year": 2030}` is inside `TaxYear`'s accepted range, so the annual
  tax report will render a financial year that has not begun.

The rest of the system is consistent the other way: `POST /listings/:id/rename` refuses a future
`effective_date` (`RenameError::FutureDated`, closed as SCENARIOS R-02 in `59bb595`),
`PUT /closing_prices/:listing/:date` refuses one with `the close of <date> is not final yet`, and
`net_capital_gain`'s quiet-carry-forward year is deliberately bounded at `tax_year_for(today())`
(SCENARIOS O-x, `319b159`). A trade is the only dated fact with no upper bound at all.

Reports read as at today are **not** corrupted — `domain::open_parcels::load` filters on `as_of`, so
the future parcels are correctly absent from `GET /portfolio/open-parcels` and the portfolio
overview. The damage is confined to the year-keyed surfaces and to the typo going unnoticed (a
2027-for-2026 slip on a July trade is exactly the shape this catches).

**Live database: zero rows disagree** — no trade is dated after today (latest is 2026-07-16), so a
write-time refusal would leave every existing row editable.

**Decision (Evan, 2026-08-22): refuse it.** Rejected: a health alert alone, and capping the year
picker while still accepting the trade.

- [x] Refuse a `date` after the server's current date on `PUT /trades/:id` and `PUT /sells/:id`,
      via `check_amounts` (a new `FutureDate` variant beside `PreCgtDate`, its natural twin — one
      bounds the date below, the other above)
- [x] Settle the two direct-INSERT paths the way `PreCgtDate` was settled (refused on the *statement*
      in `ess_vest`, the earlier and better place): an ESS taxing point and a date of death are both
      already-happened facts, so the same argument holds, but each module-doc list needs its line
      — done: neither `ess_statement::db_upsert` nor `inheritance::db_upsert` bounded its date above,
      so each got the bound (`UpsertError::FutureTaxingPoint` / `UpsertError::DeathInFuture`) beside
      its pre-CGT twin, and both module-doc lists carry the `FutureDate` line
- [x] `settlement_date` is **not** in scope — a T+2 settlement of a trade dated today is legitimately
      in the future (pinned: the boundary tests assert the accepted trade dated *today* stores a
      settlement date after today)
- [x] `docs/API.md` 422 catalogue + the trades/sells sections; `docs/SCHEMA.md` if the column comment
      needs it — also the ESS-statement and inheritance sections, the `tax-report/years` paragraph,
      and the `trades.date` / `taxing_point_date` / `date_of_death` column comments
- [x] Regression tests: tomorrow refused on `PUT /trades/:id` and `PUT /sells/:id`, today accepted
      (the boundary), and `GET /reports/tax-report/years` never offering a year beyond
      `tax_year_for(today())`

Consequences found while implementing, all settled in the same commit:

- `check_amounts` is shared with `sell::upsert_sell_in_tx`, which every parcel-substituting
  operation writes its closing Sell through — so a scrip exchange, demerger, transfer, buy-back
  participation or worthless-shares recognise **performed before its own date** is now refused too.
  That is the right answer (its replacement parcels would be dated in the future and so absent from
  the live view), but three of those five answered a generic "the … parcel allocations are invalid"
  and `transfer`'s catch-all logged `tracing::error!` "unexpected sell rejection" — each now names
  the future date instead.
- The year picker needed its **own** bound: `db_tax_report_years` unions every dated fact, and
  interest income / AMMA / ESS / investment expenses are not date-bounded, so it filters at
  `tax_year_for(today())` rather than inheriting the trade write path's ceiling.

---

## SCENARIOS S-05: a stored settlement date is never checked against the trading calendar, and the live database has one that falls on a Saturday

The only rule an explicitly supplied `settlement_date` has to satisfy is that it is not before the
trade date (`AmountsError::SettlementBeforeTrade`). It is never checked against the exchange's
calendar, so a hand-entered settlement can land on a day the exchange is closed — and one has:

```
trade 9071  LAC (XNYS)  date 2021-03-25  settlement_date 2021-05-29
```

**2021-05-29 is a Saturday**, and the two dates are two months apart, so this is a hand-entered
value rather than anything `auto_settlement_date` produced. It is in
`share-tracker-2026-08-16-000000.db` today and no surface anywhere mentions it:
`GET /reports/settlement_holiday_coverage` only asks whether the window is inside the *seeded
coverage span*, never whether the settlement day itself is a trading day, and `reports::health` has
no settlement check at all.

The auto path can produce one too — see S-04 below, where a settlement computed with no seeded
calendar landed on **2028-04-17, Easter Monday**.

A settlement date that is not a trading day on the listing's own calendar is wrong by construction,
whoever wrote it, and the check needs no bookkeeping about *when* it was computed —
`closing_price::Market::is_trading_day` already answers it, as at the date, with Crypto exempt.

- [x] Flag every stored `settlement_date` that is not a trading day on the listing's calendar.
      `GET /reports/settlement_holiday_coverage` is the natural home — a third `coverage_status`
      (or a sibling field) beside `outside_holiday_coverage` / `no_holiday_coverage` — so the one
      report that exists to answer "is this settlement date trustworthy" answers the whole question
      — done as **both**, because the two questions are independent and one trade can answer both
      badly: a sibling `settlement_non_trading_reason` (`weekend` / `holiday` / null, over
      `closing_price::non_trading_day`, one `Market` load per listing on the report's own read
      transaction) carries the new answer, and `coverage_status` gained a third value
      `inside_holiday_coverage` for the rows now listed for the settlement question alone. The row
      filter changed with it: a trade inside coverage is emitted when its settlement is not a
      trading day, so the report no longer omits every in-coverage trade
**Decision (Evan, 2026-08-22): flag it, do not refuse a supplied value.** An explicit
`settlement_date` is a deliberate override the user is asserting, so trade 9071 stays editable and
untouched; only the *auto-computed* path is guaranteed to land on a trading day. Rejected: refusing a
supplied non-trading-day settlement (it would brick trade 9071 until it was corrected), and flagging
with no guarantee on the auto path.

Note what that leaves to build: with S-08 refusing a non-trading-day **trade** date, the auto path
already lands on a trading day wherever the calendar is complete — `add_business_days` skips seeded
holidays by construction. The only way it produces a closed day is a *missing* calendar, which is
S-04, and a trade cannot be refused for the calendar being incomplete. So this section's work is the
**flag**, and the auto-path guarantee is delivered by S-04's recompute job rather than by a new
refusal. Add a test pinning that the auto path cannot produce a non-trading day under a complete
calendar, so the guarantee is asserted rather than assumed.

- [x] A supplied `settlement_date` is **not** refused — the coverage-report status is what surfaces
      it (trade 9071 stays editable; correct it separately if it turns out to be a typo, checking the
      deployed database at `bigbrain.lan:3000` as well, since the copy in the repo is the 2026-08-16
      backup rather than the live file)
- [x] `docs/API.md` — the settlement-holiday-coverage section's contract sentence — rewritten as
      the two questions the report answers, saying what an empty report does *and does not* mean
      (it is not a claim that each stored date is what today's calendar would compute — that is
      S-04's, still open); plus the Trades section (a supplied value is stored as given), the
      `non_trading_day_trades` health entry's cross-reference, `docs/SCHEMA.md`'s
      `trades.settlement_date` comment, the README feature line, and the report's `desc` /
      third-status badge in the web UI. Pinned by
      `doc_checks::settlement_coverage_documents_both_questions_it_answers`
- [x] Regression tests: a supplied weekend settlement flagged, a supplied holiday settlement flagged,
      a Crypto same-day settlement on a Saturday **not** flagged — plus a trade that is *both*
      outside coverage and settling on a weekend, reported so both facts stay legible, and the
      auto-path guarantee this section asks for:
      `trade::tests::auto_settlement_never_lands_on_a_non_trading_day_under_a_complete_calendar`
      walks every trading day of both seeded calendars (2019–2027, ~4,500 settlements) and asserts
      each computed settlement is itself a trading day, skipping the windows that run past the end
      of coverage (the incomplete-calendar case, which is S-04's). The weekend test reproduces
      trade 9071 exactly; run against a copy of the 2026-08-16 backup, the report returns that one
      row and nothing else

---

## SCENARIOS S-04: seeding the calendar the coverage report asks for silences the report without correcting the settlement dates it flagged

`GET /reports/settlement_holiday_coverage` documents its own contract in `docs/API.md`:

> Trades fully inside coverage are omitted — an empty report means every settlement window was
> computed against a complete calendar.

That sentence stops being true the moment the user does the thing the report exists to prompt.
Driven against the running system:

1. `exchange_holidays` is seeded 2019–2027. A Buy dated **2028-04-13** (the Thursday before the 2028
   Good Friday) auto-computes to **2028-04-17**, skipping weekends only, and is correctly listed as
   `outside_holiday_coverage`.
2. The user seeds the 2028 XASX calendar — Good Friday 2028-04-14, Easter Monday 2028-04-17, and the
   rest.
3. The report now returns **nothing** for that trade. The coverage span covers 2028, so the window
   is inside it.
4. The stored settlement date is **still 2028-04-17**, which is now a seeded **Easter Monday**. The
   correct answer on the completed calendar is 2028-04-19.

So the report's guarantee inverts: it is honest only while the calendar is *incomplete*, and goes
quiet exactly when the missing calendar is supplied. Nothing recomputes the affected trades, nothing
records that a stored settlement was computed against a calendar that has since changed, and the
same hole is already documented one door down for a holiday **deletion** ("a trade re-saved
afterwards without an explicit `settlement_date` silently recomputes against the changed calendar")
— but that note is about a re-save *changing* a date, not about a stale date staying put.

The S-05 trading-day check above catches this particular instance (2028-04-17 is a seeded holiday),
but not the general one: a settlement computed one day early because the window contained a holiday
that is not the settlement day itself lands on a perfectly good trading day and stays wrong.

- [x] Decide the shape (see the options below) and implement it — the `settlement-recompute` job
      (`POST /jobs/settlement-recompute`, registered in `infra::scheduler::registry`, deliberately
      absent from `schedule.cron`, the `price-rebase` shape). It re-derives each settlement date
      through `auto_settlement_date` itself — the write path's own function, over the listing's
      **live** `exchange_mic` — so the job's answer is exactly where a re-save would put the date
      and the two can never disagree; that inherits the documented live-exchange limitation
      deliberately rather than quietly resolving the calendar differently. One transaction,
      idempotent, and the UPDATEs go through the ordinary audited-table triggers, so each
      superseded date stays in `row_history`.

      **What the finding's write-up does not say, and is the whole of the work:** "rewrite the
      auto-computed ones and leave the stated ones alone" was *not answerable from the schema*.
      `trades.settlement_date` is one plain column written by both paths and nothing recorded
      which, and no heuristic recovers it (a supplied date that happens to equal T+2 is
      indistinguishable from a computed one). So migration **0041** adds
      `trades.settlement_date_source` — `computed` / `stated` / `unrecorded`, CHECK-constrained,
      the project's provenance-column idiom (`price_as_observed`, `domain::rollover::Provenance`).
      Three values because there are three states: every **existing** row takes `unrecorded` from
      the ADD COLUMN default (no UPDATE, so the migration writes no audit rows and stales no
      snapshots), and the job never rewrites `stated` or `unrecorded` — guessing could overwrite an
      assertion like trade 9071's. The default is the never-rewritten value, so a write path that
      forgets the column can only under-claim. The derived paths (ESS vest, inherited parcel, DRP
      reinvestment, rights exercise, every rollover trade) name `'stated'` in their INSERT: their
      same-day settlement is asserted by construction. One qualification keeps the provenance
      meaningful: **re-supplying the date already stored keeps the recorded source**, because a GET
      body PUT back verbatim is what the web UI's edit form sends, and treating that as an
      assertion would opt every edited trade out of the repair (pinned by
      `entities::tests::what_a_get_returns_can_be_put_back_unchanged`, which caught it).

      Cost on the live database: nil and provable. All 113 rows become `unrecorded`, and none of
      them needs recomputing anyway — every settlement window is inside the seeded 2019–2027
      coverage. Run against a copy of `share-tracker-2026-08-16-000000.db`: migration 0041 applies,
      the job answers `204` logging `trades=113 candidates=0 recomputed=0`, and every settlement
      date, every `row_history` row and every snapshot's `stale` flag is byte-identical to the
      original — trade 9071 included
- [x] `docs/API.md` — the coverage-report contract sentence is currently false and must be corrected
      whichever option is taken — it now names the repair ("**Run the `settlement-recompute` job**
      after seeding a calendar"), the Jobs list documents the job as unscheduled and says what it
      will not rewrite, the Trades section documents the read-only `settlement_date_source` field
      and the re-supply rule, and the live-exchange Known-limitation says the job inherits it.
      Plus `docs/SCHEMA.md`'s new column line, the README feature line and its unscheduled-job
      paragraph, the Jobs-screen description and the Trades screen (the new column, its
      `COLUMN_LABELS` heading, and the settlement-date field hint). Pinned by
      `doc_checks::settlement_recompute_job_documented`
- [x] Regression tests: the four-step reproduction above, and a trade whose stored settlement still
      matches a recomputation staying silent — five, in `entities::trade::tests`:
      `seeding_a_missing_calendar_and_recomputing_corrects_the_settlement_it_left_wrong` is the
      four-step reproduction end to end through the API (transposed to the 2018 Easter, because
      S-10 now refuses a trade dated 2028 — same shape, missing calendar at the other end of the
      seeded span: settles on the unseeded Easter Monday, the year is seeded, the stored date does
      not move, the job re-derives it to 2018-04-04, the report empties, and the superseded date is
      in `row_history`); `recompute_corrects_a_settlement_left_a_day_early_by_a_missing_holiday` is
      the *general* case S-05 cannot catch (only the in-window holiday missing, so the stored date
      is a perfectly good trading day and the report is silent);
      `recompute_leaves_a_settlement_that_already_matches_the_calendar_untouched` (no write at all,
      so no audit row — which is also what makes the job idempotent);
      `recompute_leaves_a_hand_supplied_settlement_untouched` reproduces trade 9071's shape (LAC on
      XNYS, 2021-03-25 → 2021-05-29) and asserts it survives the job, still flagged, unaudited; and
      `recompute_leaves_a_row_from_before_the_provenance_column_untouched` pins the `unrecorded`
      default, including that a verbatim re-save keeps it and entering a different date does not

**Decision (Evan, 2026-08-22): (b) a `settlement-recompute` job** — registered in
`infra::scheduler::registry` and deliberately **unscheduled** (the `price-rebase` shape from Q-14),
rewriting auto-computed settlement dates from the current calendar, with the docs saying to run it
after seeding a calendar. Rejected: (a) recompute-and-compare inside the report (it would have to
distinguish a deliberate override such as trade 9071 from a stale computation, and it reports rather
than repairs), and (c) documentation only. The report's contract sentence still has to be corrected
either way, and the job needs to leave a hand-supplied `settlement_date` alone — see S-05, where the
supplied value is the user's own assertion.
