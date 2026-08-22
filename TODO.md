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

- [ ] The row-history report marks a trail that crosses a `DELETE` on a row that still exists: the
      entries at or before that `DELETE` belonged to a previous occupant of the id, and are labelled
      as such rather than presented as this row's own history
- [ ] The Row History screen surfaces that boundary (not a bare extra column — the reader must not
      have to infer it), and `docs/API.md`'s Row history section states the rule
- [ ] The 20 audited tables that reuse ids are rebuilt with `AUTOINCREMENT` ids by the rename pattern
      0021/0039 established — every FK re-pointed, every `*_row_history_*` and `*_stale_snapshots_*`
      trigger set dropped and re-created, no row dropped or re-scaled (see the 0029 FK-rewrite
      gotcha and `infra::db`'s `migrations_store_decimals_as_text` guard)
- [ ] `docs/SCHEMA.md` records the `AUTOINCREMENT` requirement for an audited table and why
- [ ] Regression tests: a delete-then-recreate on the same id reads back as two occupants, not one
      history; a server-assigned insert after a delete never reuses the freed id; a test pinning
      that every audited table's id column is `AUTOINCREMENT` (so a new audited table cannot be
      added without it)

---

## SCENARIOS U-b: a multi-row operation's trail is only readable one row at a time, by ids you never saw

Driven on 2026-08-22.

`POST /reports/row_history` takes `{table, row_id}` and nothing else, so reading the trail requires
already knowing the numeric id of the row you lost. That is exactly what a user does not have for the
rows the system created or destroyed on their behalf.

Driven concretely: `DELETE /sells/3` on a demerger's closing Sell removed the whole group and wrote
**four entries across two tables** — `trades` 3, 4 and 5 and `parcel_allocations` 1 — all sharing one
`changed_at` (`'now'` is constant across the transaction, so the operation *is* correlatable in the
data). Only trade 3's id was ever named by the user: trades 4 and 5 and the allocation were created
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

- [ ] `POST /reports/row_history` returns a newest-first page of entries across every audited table
      when no `row_id` is given, keeping the existing single-row behaviour unchanged when one is
      — paged, so a large trail stays bounded on this path
- [ ] The Row History screen reaches it without a row id, and the `row_id` field's hint stops
      implying the entity list is the only way in
- [ ] `docs/API.md`'s Row history section documents the browse form and its paging
- [ ] Regression tests: the browse form returns entries across tables newest-first and pages; a
      multi-row operation (a demerger group delete) is findable through it without knowing any of
      the created rows' ids

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
