# TODO

Items are only marked done when a passing test exists for them.

Completed and decided (out-of-scope / not-reproducible) sections are archived in the topical `DONE/*.md` files, indexed by [DONE.md](DONE.md), to keep this list focused on active work. When a section here is fully done, move it into the matching `DONE/*.md` file rather than leaving it — see CLAUDE.md.

Sections here come from the **2026-07-13 improvement review** (a whole-project pass for
operational and test-strategy gaps, as distinct from the 2026-07-12 programming/domain review whose
findings are all closed in DONE.md), except where a section's heading names another source
(e.g. REQUIREMENTS, SCENARIOS). Each section records one finding; sections land in DONE.md as they
are fixed or decided.

**SCENARIOS.md sections A–S are driven and every finding they raised is closed** in the `DONE/*.md`
archive. Section **S. Settlement, holidays, and dates** was driven 2026-08-22 (`d501408`) and its
four findings closed by `67c3096` (a trade dated in the future), `30d0e96` (a trade dated on a day
its exchange was shut), `4a7ef1a` (a stored settlement date that is not a trading day) and
`e453f21` (the settlement dates a completed calendar changes) — all four archived in
[`DONE/trades-income.md`](DONE/trades-income.md), and summarised with the rest of the pass under
[Section S findings](SCENARIOS.md#section-s-findings). Every section's row in SCENARIOS.md's
[Verification status](SCENARIOS.md#verification-status) table names the pass that drove it and where
its findings went; that table is the record of what has been looked at.

Section **T. Jobs, backup, and operations** (12 scenarios) was driven on **2026-08-22** against
throwaway databases (a small one for the HTTP surface, a 265 MB one to catch a backup mid-write) and
raised **six findings**, and all six are now closed — the three jobs that recorded their failure as a
Rust `Debug` string (T-06), the startup "no schedule entry" warning that cried wolf on the two
deliberately-manual jobs (T-09/schedule), `POST /jobs/:name`'s bare-status-code failures, now
bodied 404/500 replies with an unknown query parameter refused rather than ignored (T-10), the
run interrupted by a restart that left no record and an unverified file wearing a backup's name
(T-11), now a run row opened at the start and a backup staged under `.partial` until it verifies,
the job that stops running and is never noticed (T-11/T-02/T-12), now a `job_schedule` table the
scheduler rewrites every iteration with a health `overdue_jobs` list and a **next run** column on the
Jobs screen over it, and the currency import that skipped the whole ISO 24165 half and reported an
unqualified success (T-09), now a per-feed import summary and a `job_runs.note` the Jobs screen shows
beside a still-`ok` status — all six archived in [`DONE/infra.md`](DONE/infra.md).
Everything else in the section came back correct: the
per-job lock serialises overlapping triggers (T-01), the run history bounds itself at 20 while
keeping a fail-then-succeed sequence readable (T-02), a corrupt backup is quarantined (T-03),
retention pruning over 20 backups spanning 18 months keeps exactly the newest 8 plus the first of
each of the 12 most recent months and touches nothing else in the directory (T-04), the DST-pinned
price-import entries fire at 17:30 local on both sides of every transition in both hemispheres and
handle the skipped and repeated hours exactly as the README states (T-05), the manual CSV retry
imports what the unreachable feed would have (T-06), an expiring MIC flips the validation report to
`expired` without blocking anything (T-08), and a stale price date, a stale FX month and a failed job
all surface at once and independently on the health report and its banner (T-12).

Section **U. Audit trail and history** (8 scenarios) was driven on **2026-08-22** against a
throwaway database, with the live database read read-only to check each finding against real data.
**The trail's machinery came back correct**: every one of the 22 audited tables' triggers records
every column of the live schema (U-01, checked by diffing `PRAGMA table_info` against each trigger's
`json_object` keys, which is stronger than the name-list pin the tests carry); `row_history` is
append-only and no `REPLACE INTO` path exists anywhere in the tree, while migration 0025 — the one
migration that rewrites an audited table's data in place — deliberately drops the triggers first, so
no migration forges an entry (U-02); a cascade-deleted attachment is recorded like a directly deleted
row (U-04); a superseded manual closing price's `sourced_from`, `reason` and `origin` are fully
recoverable exactly as `docs/API.md` claims (U-06); a non-audited table is refused 422 naming the
audited list (U-07); and the report is unbounded by design, which is the safe direction for an audit
trail — 10,000 entries served in 0.42 s, against a live maximum of 2 entries on any single row
(U-08).

It raised **three findings**, all open below: an id handed out again inherits the deleted row's
trail — live in the real database, where the LAC demerger's closing Sell wears a 2025 sale's history
because `trades.id` reuses freed rowids (U-a); a multi-row operation's trail is complete but readable
only one row at a time, by ids the user never saw (U-b); and the "re-create both triggers when you add
a column" rule is enforced only by hand-written per-migration assertions (U-c).

After U, the next SCENARIOS pass is section **V. Back-dated and out-of-order entry** (10 scenarios),
driven the way S, T and U were: run every scenario against a throwaway database, apply the standing
probes to each, and log what each raises as a `## SCENARIOS V-nn` section here with the option Evan
chose. The lessons worth carrying forward are in the handover memory; U added two. First, **the
standing probes find what the scenario list does not name** — id reuse is not one of U's eight
scenarios; it fell out of asking "what else moved that shouldn't have" about U-01 and U-04, and it is
the section's most serious finding. Second, and again: **check the live database read-only** — U-a
would have read as a theoretical hazard about `INTEGER PRIMARY KEY` if the live DB had not shown it
already firing twice, on a trade Evan actually entered.

---

## SCENARIOS U-a: a reused id inherits the deleted row's audit trail

Driven on 2026-08-22 against a throwaway database, then confirmed **live** in
`share-tracker-2026-08-16-000000.db` (read-only).

`reports::row_history` keys a trail on `(table_name, row_id)`, and `row_id` is the audited row's
`id`. Nothing binds an `id` to one row for the life of the database, so when an id is handed out
again the new occupant inherits every entry the previous one left.

**It has already happened twice in the live database, and not by mistyping an id.** Trade **9072**
was a real Sell — 2025-01-13, 1049 units at USD 3.16, brokerage 8.64, contract note `2501852833` —
deleted on 2026-07-26. Id 9072 is now the **LAC demerger's closing Sell**: 2023-10-03, price 0,
`demerger_action_id = 1`. `POST /reports/row_history {"table":"trades","row_id":9072}` answers with
the 2025 sale, presented as this trade's own past. `parcel_allocations` **#61** was reused in the
same session (there the re-created row is byte-identical, so the trail happens to read correctly).

The reuse is *server*-assigned. `trades.id` is a plain `INTEGER PRIMARY KEY`, which is an alias for
the rowid, and SQLite reuses the largest freed rowid on the next insert — so `POST
/corporate_actions/1/demerge`, inserting its closing Sell with no id of its own, was handed the
deleted trade's. No user chose it and nothing reported it.

This is the exact hazard migration **0021** identified for `closing_prices` and **0039** restated for
`exchange_holidays`, both fixed with `AUTOINCREMENT`, in 0039's words: *"a plain INTEGER PRIMARY KEY
reuses the highest rowid after a delete, so deleting a holiday and later adding another would hand
the new row the deleted one's id — and with it the deleted row's audit history. AUTOINCREMENT never
reuses an id, so a trail always belongs to exactly one holiday."* The reasoning was never applied to
the rest: of the 22 audited tables, **only those two are `AUTOINCREMENT`; the other 20 reuse ids.**

`AUTOINCREMENT` alone does not close it. It governs only what SQLite picks; every entity is
PUT-upsert on a client-supplied id, so `PUT /trades/9072` after deleting trade 9072 reuses the id
whatever the column says. The trail does hold the evidence in both cases — a trail whose newest entry
is a `DELETE` on a row that currently exists can only mean the id was recycled — but nothing says so,
and INSERTs record nothing to mark the boundary.

**Question for Evan — how far to take it?**

- **(a) Mark the boundary in the report.** No migration; covers server-assigned and hand-entered
  reuse alike; fixes what the trail *claims* without touching 20 tables.
- **(b) Mark the boundary, and make the remaining audited tables `AUTOINCREMENT`.** Ends accidental
  server-side reuse permanently — an id then means one row forever, which matters beyond the trail
  (an export or a note citing "trade 9072"). Large: rename-pattern rebuilds, every FK and trigger set
  re-created.
- **(c) Refuse to reuse an id** — reject a PUT on an id carrying a `DELETE` entry. Blocks the
  legitimate "delete a mis-entered row, re-enter it under the same id" workflow, and cannot cover
  server-assigned inserts without (b) anyway.

**Decision (Evan, 2026-08-22): (b), both halves.** Rejected: marking the boundary alone, and
refusing the reuse.

**Revision (2026-08-22), after the fix's premise was re-derived and found false.** `AUTOINCREMENT`
governs only the ids **SQLite** picks, when an INSERT omits the id column. It would **not** have
prevented the headline case. Nine call sites in `src/entities/` assign ids themselves —
`SELECT COALESCE(MAX(id), 0) + 1 FROM <table>`, seven for `trades` (`demerger`, `transfer`,
`scrip_exchange`, `worthless`, `ess_vest`, `inheritance`, `buyback_participation`), one for `income`
and one for `amit_adjustments` — and bind the result explicitly, so the column definition never gets
a say. The live sequence confirms it: 9072 was the highest trade id, deleting it made `MAX(id)` 9071,
`POST /corporate_actions/1/demerge` computed 9071 + 1 and took 9072 back, and the re-entered 2025 sale
became 9075.

The migration is still worth doing — the *other* live reuse, `parcel_allocations` #61, came from an
id-less INSERT where SQLite reused the freed rowid, which is exactly what `AUTOINCREMENT` prevents,
and both flavours of INSERT are common across the audited tables. But it is only half the prevention.
Dropping the nine `MAX(id) + 1` queries is worth doing on its own account too: computing an id by
reading the table's maximum is a race between concurrent operations, independent of the audit trail.

**Revised decision (Evan, 2026-08-22): the full fix — boundary marking, `AUTOINCREMENT`, *and*
the nine call sites reworked to let the database assign the id.** Rejected: allocating above the
trail's high-water mark instead of migrating (leaves SQLite's own rowid reuse unfixed), and boundary
marking alone.

- [x] The row-history report marks a trail that crosses a `DELETE` on a row that still exists: the
      entries at or before that `DELETE` belonged to a previous occupant of the id, and are labelled
      as such rather than presented as this row's own history
- [x] The Row History screen surfaces that boundary (not a bare extra column — the reader must not
      have to infer it), and `docs/API.md`'s Row history section states the rule
- [x] The nine `SELECT COALESCE(MAX(id), 0) + 1` call sites stop assigning ids: the INSERT omits the
      id and the server reads `last_insert_rowid()`, so a never-reused id comes from the database
      (this, not the migration, is what fixes the trade 9072 case)
- [x] The audited tables that reuse ids are rebuilt with `AUTOINCREMENT` ids by the rename pattern
      0021/0039 established — every FK re-pointed, every `*_row_history_*` and `*_stale_snapshots_*`
      trigger set dropped and re-created, no row dropped or re-scaled (see the 0029 FK-rewrite
      gotcha and `infra::db`'s `migrations_store_decimals_as_text` guard)
- [x] `docs/SCHEMA.md` records the `AUTOINCREMENT` requirement for an audited table and why
- [x] Regression tests: a delete-then-recreate on the same id reads back as two occupants, not one
      history; a server-assigned insert after a delete never reuses the freed id; a test pinning
      that every audited table's id column is `AUTOINCREMENT` (so a new audited table cannot be
      added without it)

**Boundary marking done 2026-08-22** (the first two items; the id-assignment rework, the
`AUTOINCREMENT` migration, the SCHEMA.md note and the AUTOINCREMENT pin test are still open).

**`AUTOINCREMENT` migration done 2026-08-22** (`0045_autoincrement_audited_ids.sql`). Still open:
the nine `SELECT COALESCE(MAX(id), 0) + 1` call sites, `docs/SCHEMA.md`'s statement of the
*requirement* and why, and the regression tests — including the pin that every audited table's id
column is `AUTOINCREMENT`, so a new audited table cannot be added without it.

**Id assignment reworked, and the section closed — done 2026-08-22.** All nine
`SELECT COALESCE(MAX(id), 0) + 1` queries are gone: every server-created row now omits the id
column (or binds NULL, which for an `INTEGER PRIMARY KEY AUTOINCREMENT` column is the same thing)
and reads back what the database assigned. One shared write core covers five of the nine sites:
`sell::upsert_sell_in_tx`, whose `id` is now `Option<i64>` — `Some` only on the client-supplied-id
path `PUT /sells/{id}`, which stays an upsert — and which answers the id written, for the
scrip-exchange, demerger, transfer (and its network-fee), worthless and buy-back closing Sells. The
other four are the ESS vest parcel, the inherited parcel (NULL on first entry; an edit still keeps
the linked Buy's id through the `ON CONFLICT` arm), the buy-back's dividend `income` row, and the
generated AMIT adjustments — `amit_adjustment` gained `db_insert_on` over the same validation core
as `db_upsert_on`, and its duplicate-parcel check now reads `id IS NOT ?` so a NULL id excludes
nothing. Alongside the nine, `rollover::insert_replacement_buy` (no `id` field on `ReplacementBuy`
any more, answers `last_insert_rowid()`) was the same bug in another shape: the demerger, transfer
and scrip exchange numbered their replacement Buys `sell_id + 1 + i`. Each Buy now takes the id its
own INSERT was given. A *preview* generation writes nothing, so its rows
carry no id at all (`UNASSIGNED_ID` = 0) rather than a prediction; `docs/API.md` says so.

Proved behaviourally: `reports::row_history`'s
`a_server_assigned_insert_never_takes_a_deleted_trades_id` (the old
`a_server_assigned_id_taking_a_deleted_trades_place_is_marked`, inverted) deletes the highest-id
trade and drives a real `POST /corporate_actions/{id}/demerge` — the closing Sell and both
replacement Buys take three fresh ids, none of them the freed one, each with an empty trail, while
the deleted Buy's own trail still reads as one occupant and its id holds no row.
`amit_adjustment_generation`'s `db_generation_never_reuses_a_deleted_adjustments_id` does the same
for the adjustments table and checks each reported id is the row really stored. The
`AUTOINCREMENT` pin is `every_audited_tables_id_is_autoincrement`: derived from the live schema and
`AUDITED_TABLES`, so a new audited table cannot be added without it (verified non-vacuous — adding
a plain-PK table to the loop fails it). Its two exemptions are *checked*, not skipped —
`tax_year_settings` must still have no `id` column, `cgt_settings` must still carry
`CHECK (id = 1)`. On a copy of the live database, deleting the highest trade (9076) and inserting
an id-less trade takes **9077**, not the freed id.

Exactly **17** of the 22 audited tables reused ids and are rebuilt: `trades`, `parcel_allocations`,
`income`, `interest_income`, `amma_statements`, `amit_adjustments`, `ess_statements`, `transfers`,
`corporate_actions`, `inheritances`, `rights_sales`, `rights_sale_allocations`,
`investment_expenses`, `drp_enrolments`, `attachments`, `listings`, `listing_renames`. Five are
deliberately left alone, and the migration header says why: `closing_prices` (0021),
`rba_fx_rates` (0031) and `exchange_holidays` (0039) are already `AUTOINCREMENT`;
`tax_year_settings` is keyed on the financial year itself, with no surrogate id to make one (0027,
and the boundary marking exempts it for the same reason); and `cgt_settings` is
`id INTEGER PRIMARY KEY CHECK (id = 1)`, a singleton whose CHECK pins the id, so re-creating its one
row is re-entry of the same fact, not reuse.

The shape is 0029's, because most of these tables are referenced by another — `attachments` alone
has six `ON DELETE CASCADE` parents among them, and a rename that repointed it at `<parent>_old`
would have cascaded every attachment away when that table was dropped. `-- no-transaction` with the
migration's own `BEGIN`/`COMMIT` around `PRAGMA foreign_keys = OFF` (a no-op inside a transaction),
plus `legacy_alter_table` per rename so no trigger body is rewritten either. Per table: both trigger
sets dropped, rename, re-create with `id INTEGER PRIMARY KEY AUTOINCREMENT` and every other column,
constraint and index unchanged, copy `ORDER BY id`, drop the old table, re-create the indexes and
then the triggers — the staleness triggers last, so the migration's own copy does not stale every
stored snapshot. Each table's definition and both triggers are reproduced from the **live** schema
rather than from the migration that first created them, since several had been re-created since
(`trades`' pair comes from 0041, not 0013).

**Seeding `sqlite_sequence` is load-bearing, not defensive.** `AUTOINCREMENT` never issues an id at
or below the table's stored sequence, and a plain copy sets that to the largest *live* id — leaving
an id freed before the migration still issuable. In the live database `parcel_allocations` holds 33
rows with a maximum id of **63**, while its trail's highest `row_id` is **65**: a plain copy would
have handed the next two allocations 64 and 65, and 65 already has an audit trail — the bug would
have reproduced on the first write after the migration. So each table's sequence is seeded to
`MAX(largest live id, largest row_id that table has ever recorded in row_history)`, the trail being
the only surviving record of an id that no longer holds a row (append-only and keep-forever, 0013,
so the mark cannot recede). `attachments` is the mirror case (live 140, trail 136) and takes 140; an
empty table with no trail seeds to 0, which is what an untouched `AUTOINCREMENT` table means anyway.

Acceptance-tested against a copy of the live database (`share-tracker-2026-08-22-205530.db`, 45 MB,
1,329 trail entries, migrated to head): for all 30 tables the row count, the full id set and an
all-columns checksum are **byte-identical** before and after — `PRAGMA integrity_check` `ok`,
`PRAGMA foreign_key_check` empty. All 155 schema objects compare equal after comment/whitespace
normalisation *modulo* the 17 `AUTOINCREMENT` keywords, and a DB built from the migrations from
scratch produces the same schema. Post-migration sequences: `trades` 9076, `parcel_allocations`
**65** (not 63), `attachments` 140, `income` 47, `amit_adjustments` 149, `interest_income` 25,
`transfers` 10, `amma_statements`/`listings` 8, `ess_statements` 5, `drp_enrolments` 3,
`corporate_actions`/`listing_renames` 1, the four empty tables 0. Behaviourally, two inserts into
the migrated copy take ids **66 and 67** — not the freed 64 and 65 — and a new trade takes 9077.

The reuse this fixes is measurable in that database: the trail carries ten `DELETE` entries on ids
that hold a row again, across eight distinct ids (`trades` 9072-9076, `parcel_allocations` 61-63,
two of them reused twice).

The single-row form now segments a trail into the successive **occupants** of the id and says which
is which, because the trail already holds the evidence: INSERTs are not recorded, so a `DELETE` on an
id that **still holds a row** can only mean the id was handed out again. Every `DELETE` therefore
closes an occupancy — the `DELETE` and everything older belong to an earlier occupant — with one
exception: the newest entry of a trail whose id holds no row now is that occupant's own death, an
ordinary deleted row. Segmenting rather than splitting once was deliberate: delete/recreate twice
reads as three occupants. Each entry carries `occupant` (`1` = the id's most recent occupant) and
`current_occupant` (`true` when it belongs to the record holding the id now); both are additive, so
no existing field changed meaning. "Does the id hold a row?" is read on the **same transaction** as
the trail, or a concurrent delete would label the boundary against a row that had just gone.

It is honest about the two things it cannot know, both stated in `docs/API.md` and on screen: *when*
the id was taken again (the re-insert recorded nothing), and whether the new occupant is a re-entry
of the same record. `tax_year_settings` is exempt — its `row_id` is the financial year itself, and
0027 already decided that re-entering a year's settings is the *same* taxpayer-year fact, so it stays
one occupant.

The screen (`sections`, a new generic `viewReport` hook for an array response whose rows are not all
one thing) renders a headed section per occupant — a previous occupant's named with the timestamp of
the `DELETE` that ended it — under a boxed `.section-notice` warning; the record holding the id with
no entries of its own gets an explicit empty section rather than silently missing one. A trail with
one occupant renders exactly as before: one plain table, no heading, no notice, and neither marking
field as a column. The browse form deliberately carries no marking (it lists the trail in write
order, where no entry is presented as any row's own history; the drill-through link lands on the
single-row form, which does).

Tests: `reports::row_history::tests::a_reused_id_splits_into_two_occupants`,
`a_reused_ids_new_occupant_may_have_no_history_of_its_own`,
`an_id_reused_twice_segments_into_three_occupants`,
`a_server_assigned_id_taking_a_deleted_trades_place_is_marked` (the live shape reproduced end to end
— deleting the highest trade id then demerging, whose `MAX(id) + 1` hands the freed id straight to a
server-created Sell, exactly as trade 9072 became the LAC demerger's closing Sell), the two
non-reuse cases (`an_edited_row_that_still_exists_is_one_occupant`,
`a_deleted_row_is_one_occupant_not_a_reuse`), `a_natural_key_re_entered_is_still_one_occupant`,
`api_entries_carry_the_occupant_they_belong_to`, `browse_entries_carry_no_occupant_marking`,
`web::tests::row_history_ui_present` and `doc_checks::row_history_audit_trail_documented`. Rendering
verified with `scripts/ui-check.sh` over all three shapes (re-use, re-use with no own history, plain
trail).

---

## SCENARIOS U-b: a multi-row operation's trail is only readable one row at a time, by ids you never saw

Driven on 2026-08-22.

`POST /reports/row_history` takes `{table, row_id}` and nothing else, so reading the trail requires
already knowing the numeric id of the row you lost. That is exactly what a user does not have for the
rows the system created or destroyed on their behalf.

Driven concretely: `DELETE /sells/3` on a demerger's closing Sell removed the whole group and wrote
**four entries across two tables** — `trades` 3, 4 and 5 and `parcel_allocations` 1. Only trade 3's
id was ever named by the user: trades 4 and 5 and the allocation were created
by `POST /corporate_actions/7/demerge` and are now gone from every list endpoint. The same shape is
in the live database at `2026-07-26T07:39:44.222Z`, spanning `trades`, `attachments` and
`parcel_allocations`. The cascade case (U-04) is identical — deleting a trade takes its attachments
with it, and their ids appear nowhere afterwards.

The UI states the gap without meaning to: the Row ID field's hint reads *"The record's id as shown in
its entity list"*, and a deleted row is by definition not in its entity list. The trail is complete
and correct; it is simply not reachable.

Also confirmed while driving this: `POST /clear_unpriced_before` deletes hundreds of price rows in
one transaction (the documented case is 635), every one recorded and every one keyed on an id the
user never saw.

**Question for Evan — how should the trail become discoverable?**

- **(a) A recent-changes browse mode** — list entries newest-first across all tables, paged, with no
  `row_id`, so an operation is found by when it happened and drilled into. Covers every unknown-id
  case at once and adds no new concept: the trail is already ordered and indexed.
- **(b) Group by transaction timestamp** — return every entry sharing one `changed_at`. Answers the
  multi-row question exactly, but still needs one known id for a foothold.
- **(c) Both.**
- **(d) Document it as a known limitation.**

**Decision (Evan, 2026-08-22): (a), the browse mode.** Rejected: timestamp grouping (it needs a
foothold the user does not have), both, and documenting it.

- [x] `POST /reports/row_history` returns a newest-first page of entries across every audited table
      when no `row_id` is given, keeping the existing single-row behaviour unchanged when one is
      — paged, so a large trail stays bounded on this path
- [x] The Row History screen reaches it without a row id, and the `row_id` field's hint stops
      implying the entity list is the only way in
- [x] `docs/API.md`'s Row history section documents the browse form and its paging
- [x] Regression tests: the browse form returns entries across tables newest-first and pages; a
      multi-row operation (a demerger group delete) is findable through it without knowing any of
      the created rows' ids

Done 2026-08-22. `POST /reports/row_history` now answers two shapes, chosen by whether the body
names a `row_id`:

- **One row's trail** — unchanged, byte for byte: the same flat array of prior versions, each
  flattening the audited table's own columns behind `history_id`/`operation`/`changed_at`. `table`
  is still required alongside a `row_id` (a row id means nothing without the table it is an id in),
  and the existing tests were not touched.
- **Recent changes** (`{}`) — an object, `{entries, page_size, next_before_id}`, whose entries are
  **uniform across tables**: `history_id`, `table_name`, `row_id`, `operation`, `changed_at`, and
  nothing else. The flat shape could not be reused — rows of `trades` and `parcel_allocations` have
  different columns and the UI renders every data table through one `filterableTable` with one
  column set — and `old_row` is deliberately neither flattened nor summarised: a summary would have
  to choose what to show, and could misrepresent what changed. The prior values stay one drill-down
  away, through the `(table_name, row_id)` each entry names in full.

Paging is a **cursor**, not an offset: `before_id` returns the entries older than that trail id,
`limit` defaults to 100 and is bounded at 1000 (outside that is a 422 naming the cap, never a
silently truncated page), and `next_before_id` is null *exactly* when the page reached the end of
the trail — so "more remains" is stated, not inferred from a full-looking page. The trail is
append-only, so new entries land at the top and an offset page would shift under a concurrent
write. `table` without a `row_id` filters the page to one audited table (still 422 if it is not
one); `before_id`/`limit` alongside a `row_id` are refused rather than ignored (one row's trail is
returned whole).

UI: the same screen, still one config-driven `REPORTS` entry. Both params became optional, a
`before_id` param joined them, and the screen `autoRun`s — every field is optional, so it opens on
the browse page and the form narrows it. The browse object renders through the existing `tables`
mechanism (`viewReport` now applies `tables` to object responses only, so the single-row array
falls through to the plain table as before), each browse row carries a **Trail** link to
`#/r/row-history/<table>/<row_id>` (report hash routes now take extra path segments that prefill a
params form positionally and run it), and a paged response renders a "Load older →" button that
fills `before_id` and re-runs — field-driven on `next_before_id`, like the existing taxpayer-basis
note. `dataTable` grew one guard: the Actions column appears only where some row has an action, so
the single-row trail (whose rows have no `table_name`) does not carry an empty column. The Row ID
hint no longer says "as shown in its entity list" — the wording the finding called out, since a
deleted row is not in one; it now names the browse form as the way in for a row no list shows.

Tests: `reports::row_history::tests::browse_returns_entries_across_tables_newest_first`,
`browse_pages_by_cursor_and_says_when_more_remain`,
`browse_filters_to_one_table_and_refuses_a_bad_request`, and
`a_demerger_group_delete_is_findable_without_knowing_any_ids` — which drives the finding's own
case: demerge, delete the group's closing Sell, then find the two demerge-created Buys and the
allocation from the browse page alone and drill into one of them by the `row_id` its entry carries.
Plus `web::tests::row_history_ui_present` (the cursor param, the drill-in link, and the absence of
the old hint) and `doc_checks::row_history_audit_trail_documented` (the browse section, the response
shape, the cursor, the bound, the ordering rule). Rendered end to end with
`scripts/ui-check.sh --seed … '#/r/row-history'`: the browse table, the Trail links, and — over 100
entries — the "Load older" affordance.

**A claim in the finding is wrong, and it mattered.** "`'now'` is constant across a transaction in
SQLite" is not so: it is fixed for one *statement*. Measured (2026-08-22): two `strftime('now')`
reads in one transaction, a long query between them, came back 227 ms apart — and a first draft of
the demerger test, which grouped the operation's entries by shared `changed_at`, failed
intermittently because the delete's four rows span three statements. So the timestamps of one
operation *tie* where a single statement wrote them and *differ* where it did not: `changed_at` is
neither unique nor a total order, and ordering/paging on it would skip or repeat rows. Ordering is
on the trail's own `id` throughout, which is what the decision asked for. It also retires option (b)
(group by `changed_at`) as more than merely needing a foothold: it would have been unreliable.

---

## SCENARIOS U-c: nothing pins an audited table's trigger column list against the live schema

Driven on 2026-08-22 — and the machinery came back **correct**: diffing every audited table's
`PRAGMA table_info` against the `json_object` keys of both its `*_row_history_*` triggers found all
22 tables complete, with `attachments.content` the single documented exclusion (a BLOB `json_object`
cannot hold).

What is missing is the guard. "A migration that adds a column to an audited table must DROP and
re-CREATE that table's two `*_row_history_*` triggers with the new column list" is stated in
CLAUDE.md, in 0013's header and in `docs/SCHEMA.md`, and is enforced only by **hand-written
per-migration assertions** — `audited_tables_match_migration_check_and_triggers` pins the *lists* to
each other and then checks specific migrations by name (0026 re-creating the `ess_statements` pair
with `fx_rate`, and so on). A future `ALTER TABLE ... ADD COLUMN` that forgets the rebuild adds no
failing test: the column would simply stop being recorded, silently, and the trail would keep looking
healthy. The check that catches it is about twenty lines and derives everything from the live schema.

**Question for Evan — add the generic guard?**

- **(a) Add the generic test.**
- **(b) Leave the per-migration assertions.**

**Decision (Evan, 2026-08-22): (a).**

- [x] A test walks every table in `AUDITED_TABLES`, reads its columns from the live schema and both
      its triggers' `json_object` keys, and fails on any column the trail would drop — with
      `attachments.content` allowlisted as the documented BLOB exclusion, and a missing trigger
      reported as a failure rather than a skip
- [x] The comment says it supersedes the bespoke per-migration column assertions for future
      migrations, so the next one is not written by hand

Done 2026-08-22. `reports::row_history::tests::every_audited_column_is_recorded_by_both_triggers`
walks all 22 `AUDITED_TABLES` over a `test_pool()` (every migration applied), reads each table's
columns from `pragma_table_info` and each of its two `*_row_history_*` triggers' recorded keys from
`sqlite_master`, and asserts no column is missing from either. A trigger that is absent panics
naming it rather than being skipped, so an audited table with no pair fails just as loudly as one
with a dropped column. `attachments.content` is the single allowlisted exclusion, carrying 0013's
reason (a BLOB is not something a `json_object` can hold).

The key list is parsed by a `json_object_keys` helper that scans only the text between the
`json_object(` call's own parentheses — the enclosing `INSERT INTO row_history ... VALUES
('<table>', OLD.id, ...)` has a quoted-string/`OLD.`-value pair of its own that a whole-body regex
reads as a column — and takes every quoted string in it, so identifiers with digits
(`pre_2009_cessation_discount`) are matched whole. No new dependency: plain string scanning, since
`regex` is not in the tree. All 22 tables pass on `main` as it stands; deleting the allowlist makes
it fail on `attachments.content`, which is how the check was confirmed to have teeth.

The doc comment says it supersedes the per-migration column assertions in
`audited_tables_match_migration_check_and_triggers` for *future* migrations — the existing ones stay,
because they pin something derived checking cannot: which migration the live trigger pair came from.
