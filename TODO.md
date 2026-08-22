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
throwaway database, with two live backups read read-only to check each finding against real data.
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

It raised **three findings, and all three are now closed** — the id handed out again that inherits
the deleted row's trail (U-a), which was live in the real database and took three commits: the trail
now says whose history it is (`7b915cf`), migration 0045 gives 17 audited tables `AUTOINCREMENT` ids
(`4a3a257`), and every server-created id now comes from the database rather than `MAX(id) + 1`
(`1a0f821`); the multi-row operation readable only by ids the user never saw, now a browse form over
the whole trail (`be64d3d`); and the trigger-column rule enforced only by hand-written per-migration
assertions, now a generic guard derived from the live schema (`57502eb`) — all three archived in
[`DONE/infra.md`](DONE/infra.md).

After U, the next SCENARIOS pass is section **V. Back-dated and out-of-order entry** (10 scenarios),
driven the way S, T and U were: run every scenario against a throwaway database, apply the standing
probes to each, and log what each raises as a `## SCENARIOS V-nn` section here with the option Evan
chose. The lessons worth carrying forward are in the handover memory; U added three. First, **the
standing probes find what the scenario list does not name** — id reuse is not one of U's eight
scenarios; it fell out of asking "what else moved that shouldn't have" about U-01 and U-04, and it is
the section's most serious finding. Second, and again: **check the live database read-only** — U-a
would have read as a theoretical hazard about `INTEGER PRIMARY KEY` if the live DB had not shown it
already firing twice, on a trade Evan actually entered. Third, sharper than either: **re-derive a
fix's mechanism before building it, not just its arithmetic**. U-a's chosen option was
`AUTOINCREMENT`, and `AUTOINCREMENT` governs only the ids SQLite picks — nine call sites bound their
own `MAX(id) + 1` and would have sailed through the whole 17-table migration unchanged, leaving the
headline case live. The same trap sprang twice more in one section: the finding's claim that `'now'`
is constant across a transaction was false (it is per *statement*), and the migration's first draft
justified its `sqlite_sequence` seeding with arithmetic that did not hold. Each was caught by
measuring against the database instead of reasoning about it.

---

## `next_run_log_shows_timezone` flakes under CPU contention

Surfaced 2026-08-22 while closing SCENARIOS U-a, in a run unrelated to the change under test (the
diff touched no scheduler code). `infra::scheduler::tests::next_run_log_shows_timezone` failed once
in roughly ten full-suite runs, only when the machine was loaded; it passed 15/15 in isolation under
the same load.

The test waits for a spawned task to emit its `next run scheduled` log line by calling `yield_now`
about fifty times. That is a bounded spin on the scheduler's goodwill, not a synchronisation
primitive: under contention the spawned task may not have been polled to the point of logging within
those yields, and the assertion then reads an empty buffer. Nothing about the *behaviour* is in
doubt — the same line is asserted by neighbouring tests that do not race for it.

A flaky test in a suite that gates every commit is worse than a missing one: it trains the reader to
re-run rather than read. Worth fixing properly (wait on a signal the logging path actually sets,
rather than counting yields), not by raising the yield count.

- [x] `next_run_log_shows_timezone` waits on something deterministic instead of a bounded `yield_now`
      spin, and no longer fails under CPU contention
- [x] A note in the test says why it cannot go back to counting yields

Done 2026-08-22. It went red in CI first — on the `v0.13.0` release push, at 1953 passed / 1 failed,
in a run whose suite took 140s against ~4s locally, which is exactly the contention the yield count
could not survive. The fixed spin is replaced by `wait_until(cond, what)`, a bounded poll on the
condition itself: it returns on the first poll that sees the line (so it is *faster* in the common
case, not slower), and panics naming what it waited for rather than hanging, so a real regression is
still a failure with a name. Verified by running the test **25 times under full CPU saturation** —
every core pinned by spinners, the condition that produced the CI failure — with 0 failures.
The comment records why counting yields cannot work: a `yield_now()` count bounds how many times
*this* task defers, not whether the spawned task has been polled far enough to log.

The other `yield_now` loop in this file (the in-flight-run wait in the `POST /jobs` tests) is left
alone deliberately: it already spins on the real condition rather than a fixed count. It has no
deadline, which is a smaller wart — a broken condition hangs rather than failing by name — and is
worth tidying only if it ever bites.

---

## A request arriving during startup answers 500 "database is locked"

Found 2026-08-22 by CI going red on the `v0.13.0` push — not on the test suite this time but on
`scripts/ui-smoke.sh`, whose fixture seeding got `PUT /listings/2 -> 500` with an empty body.

Reproduced locally by starting a fresh server and issuing one `PUT` as soon as it answers: **2 in 40
runs** fail with `error returned from database: (code: 517) database is locked`. The server log puts
the error in the middle of the scheduler's startup `next run scheduled` lines, and an **empty
`schedule.cron` makes it vanish (0 in 40)** — so the collision is with the per-entry `job_schedule`
writes that `spawn` performs at startup, concurrently with the server already serving requests.

**It is not migration 0045.** Checked rather than assumed, because the migration had just been
deployed: a build of `7b915cf` (the commit before 0045) fails the same way **8 times in 60**, a
higher rate than the 2 in 40 measured after it. The race arrived with the `job_schedule` table
(migration 0043, SCENARIOS T-11/T-02/T-12) — the first thing in this system to write to the database
from a background task while requests are being served.

**Why `busy_timeout` does not already cover it.** sqlx sets a 5-second `busy_timeout` by default, so
plain `SQLITE_BUSY` waits. But code 517 is `SQLITE_BUSY_SNAPSHOT`, which `sqlite3_busy_timeout()`
deliberately does **not** retry: it is returned when a transaction that began deferred — as a reader
— tries to upgrade to a writer after another connection has committed since its read snapshot was
taken. SQLite returns it immediately and expects the application to roll back and retry, or to have
taken the write lock up front. `pool.begin()` issues a deferred `BEGIN`, so every write transaction
in the tree is exposed; there are 26 files beginning transactions on the write side and 21 on the
read-only report side.

The impact is small but real and user-visible: a 500 with an empty body, which the web UI can only
show as `HTTP 500` — the same complaint SCENARIOS T-10 raised about the jobs endpoint. It is not a
correctness risk (the transaction fails atomically; nothing partial is written), and it needs a
concurrent writer, which for a single-user tool means startup or a job running as you click.

**Question for Evan — how to fix it?**

- **(a) Write transactions take the lock up front** — `pool.begin_with("BEGIN IMMEDIATE")` on the
  write paths, leaving the read-only report snapshots deferred. This is what SQLite documents for
  exactly this error, and it makes the existing 5-second `busy_timeout` effective: a concurrent
  writer waits instead of failing. Touches the write-side `begin()` sites and needs a shared helper
  so a new write path cannot quietly go back to a deferred `BEGIN`.
- **(b) Retry the transaction on 517** in a wrapper, leaving the `BEGIN`s deferred. Keeps the
  transaction shapes but puts retry logic on every write path, and a retried financial write must
  re-run the whole transaction, not resume it.
- **(c) Keep the scheduler off the database while the server is starting** — write `job_schedule`
  before binding, or serialise it. Narrows this trigger but leaves the general race live (a manual
  `POST /jobs/{name}` while you are entering a trade collides just the same).
- **(d) Accept it** — a rare startup-only 500, already atomic, and document it.

**Decision (Evan, 2026-08-22): (a)**, write transactions take the lock up front. Rejected: retrying
on 517 (b), narrowing the scheduler's startup writes (c), and accepting it (d).

- [x] Decision recorded and implemented

      Done 2026-08-23. `infra::db::write_tx(pool)` is the one way a write transaction is begun —
      `pool.begin_with("BEGIN IMMEDIATE")`, with the reasoning in its doc comment — and all 39
      write-side `pool.begin()` sites across 27 files now go through it: every file under
      `src/entities/`, `src/infra/scheduler/db.rs`, and **`src/reports/snapshot.rs`**, the one
      report that writes (it persists the price-dependent reports to `report_snapshots`; a
      reports-are-readers split would have missed it). The other 20 report files stay deferred:
      they never upgrade, so they cannot hit this, and taking the write lock up front would
      serialise every report against every other for nothing. `src/domain/` turned out to begin no
      transactions at all — it composes onto the caller's connection — so it needed no change.

      `StoredSchedule::record` (`infra/scheduler/run.rs`), the `job_schedule` write that triggered
      the bug, needed no change either: it writes through `db_insert_schedule`/`db_update_schedule`,
      single statements executed straight on the pool. A lone statement is its own implicit
      transaction, which takes the write lock immediately and gets plain `SQLITE_BUSY` — the one
      the 5-second `busy_timeout` *does* retry. It was only ever the other side of the race.

      Pinned by `infra::db::tests::write_side_modules_never_begin_a_deferred_transaction`, a source
      scan in the spirit of the `.bind(x.to_string())` one: a deferred `BEGIN` anywhere under `src`
      fails the test unless the file is named in `DEFERRED_BEGIN_ALLOWED`, which lists the 20
      read-only report files one at a time (not `src/reports/`, so a *new* report is an offender
      until someone decides which side it is on) and rejects an entry that has gone stale.

      Measured on the reproduction from the diagnosis above — fresh server on a temp DB with the
      real `schedule.cron`, poll until it answers, then immediately `PUT /listings/2`: **2 failures
      in 160 runs before** (one `(code: 517)`, one `(code: 5)` — the same failed upgrade surfaces
      as either), **0 in 200 after**.
- [x] `scripts/ui-smoke.sh` dumps the server log when a **seed** request fails, not only when the
      server fails to start — the cause was logged and CI threw it away, which is what made this a
      half-hour diagnosis instead of a one-line one

      Done 2026-08-23, in `scripts/ui-check.sh`, which is where ui-smoke's seeding happens: the
      seed step's exit status is captured and a non-zero one prints the server log before exiting.
      A failed seed reached a *running* server, so a 500's cause is in the log and nowhere else —
      `ApiError::Internal` answers with an empty body by design. Verified by seeding a deliberately
      invalid fixture: the run exits 1 and the log is printed.
- [x] A regression test: a write arriving concurrently with the scheduler's startup writes succeeds
      rather than answering 500

      Done 2026-08-23, as a pair. `infra::scheduler::tests::
      a_write_arriving_during_scheduler_startup_is_served_not_locked_out` is the end-to-end one:
      `spawn` over the real `schedule.cron` (repeated, so ~64 entry tasks claim `job_schedule` rows
      at once), 15 concurrent `PUT /listings/…` fired immediately behind it, five startups, every
      request required to answer 204. It is a race, not a scripted interleaving, so its power is
      measured rather than assumed: against a build with `write_tx` reverted to a deferred `BEGIN`
      it caught the regression **29 times in 30** (`PUT … -> 500`), and passed **30 in 30** with
      the fix. The deterministic half is in `infra::db`:
      `a_deferred_transaction_cannot_upgrade_after_a_concurrent_write` pins the failure itself
      (immediate, not after the busy timeout), and
      `write_tx_holds_off_a_concurrent_writer_instead_of_failing_to_upgrade` pins the fix — the
      concurrent writer must still be blocked while the transaction holds the lock, which is
      exactly what a deferred `BEGIN` cannot do, so it fails 100% of the time on a regressed build.
