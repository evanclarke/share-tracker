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
raised **six findings**. Two are closed — the three jobs that recorded their failure as a Rust
`Debug` string (T-06), and the startup "no schedule entry" warning that cried wolf on the two
deliberately-manual jobs (T-09/schedule) — both archived in [`DONE/infra.md`](DONE/infra.md); the
other four are open below.
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

The next work after these six is the next SCENARIOS pass — section **U. Audit trail and history**
(8 scenarios) — driven the way S and T were: run every scenario against a throwaway database, apply
the standing probes to each, and log what each raises as a `## SCENARIOS U-nn` section here with the
option Evan chose. Three lessons are worth carrying into it. First, **check the live database
read-only before proposing a refusal** (the M lesson, which is what turned S-05 from a refusal into a
flag — trade 9071 would have been bricked). Second, **a decision can rest on a distinction the schema
cannot make**: S-04's "rewrite the auto-computed settlement dates, leave the stated ones" was
unanswerable until a provenance column recorded which path wrote each date, and the finding's own
write-up never mentioned it. Third, from T: **drive the interruption, don't reason about it** — the
mid-backup restart finding (T-02 below) only became concrete once a 265 MB database made the write
slow enough to kill, and the file it left behind was not the one the code reading predicted.

---

## SCENARIOS T-11/T-02/T-12: nothing notices a job that has stopped running

`reports::health`'s `failed_jobs` fires only when a job's **most recently recorded run failed**. A
job that is not running at all records nothing, so it raises nothing — the last successful run stays
in place and the Jobs screen keeps showing `ok`, indefinitely. Driven on 2026-08-22:

- A schedule line with no future occurrence (`0 0 30 2 *   backup` — 30 February) is **accepted at
  startup**. `run_entry` logs one `ERROR cannot compute next run, stopping` and the task exits. The
  backup will never run again for the life of the process, and `GET /jobs` still answers
  `backup: last_success = true`.
- Same outcome, no ERROR line at all, for the ordinary operational cases: the server was down at
  00:00 every Sunday, or a hand-edited `--schedule` file lost its `backup` line (that one logs a
  single startup `WARN`; it was indistinguishable from the two deliberate ones until T-09/schedule
  marked the manual-only jobs in the registry — closed in [`DONE/infra.md`](DONE/infra.md) — so it
  now fires only for a line that has actually been lost).

Prices and FX each have a *database-derived* freshness signal that catches their job going quiet —
`prices_stale` (latest ok `closing_prices` date more than 3 business days old) and `fx_stale`
(latest `rba_fx_rates` month older than last month). **The backup has none**, and it is the job where
this matters most: nothing in the database changes when a backup does or does not happen, so a backup
that silently stopped a year ago is indistinguishable from one that ran on Sunday. `mic-import` and
`currency-import` have none either.

The Jobs screen compounds it: it shows each job's **last** run and its history, but never the
schedule or the next run — so the one surface an operator would check cannot answer "is this job
still scheduled, and when is it due?". The scheduler already computes that instant every iteration
and logs it (`next run scheduled`), but nothing persists it.

**Live database: no false alarm.** `job_runs` in the 2026-08-16 backup shows every job last
succeeding exactly when its schedule says it should (backup Sunday 00:01 local, rba-fx-import Monday
02:00, mic/currency-import on the 1st, price-import and report-snapshot daily), so an overdue check
would start quiet. Note the thresholds must be **per job** — weekly, monthly and daily jobs sit side
by side.

**Question for Evan — how should an overdue job be detected?**

- **(a) Persist the next scheduled run.** `run_entry` writes the instant it already computes to a
  new `job_schedule` table (job name, cron expression, timezone, `next_run_at`) on every iteration;
  health gains `overdue_jobs` (now past `next_run_at` plus a grace margin) and the Jobs screen gains
  a "next run" column. Catches all three causes — the dead task, the dropped schedule line, and the
  server that was down — because a stopped task stops moving the stored instant. Most work; also the
  only option that answers "when is this due?" in the UI.
- **(b) A per-job maximum age.** Health alerts when a job's last *successful* run is older than a
  constant declared beside the job in `registry()` (e.g. backup 10 days, rba-fx-import 10 days,
  mic/currency-import 40 days). No new table; duplicates the schedule's knowledge in a second place,
  and stays silent if the schedule is edited to something slower.
- **(c) Backup only.** A `backup_stale` flag on the health report, mirroring `prices_stale`, derived
  from `job_runs`. Smallest fix, covers the job with no other signal, leaves the rest as they are.
- **(d) Documentation only** — a Known limitation saying job liveness is the operator's business.

**Decision (Evan, 2026-08-22): (a), persist the next scheduled run.** Rejected: a per-job maximum
age, a `backup_stale` flag alone, and documentation only.

- [ ] A `job_schedule` table (migration): job name, cron expression, timezone, `next_run_at`,
      `updated_at`. Written by `run_entry` every iteration, from the instant it already computes for
      its `next run scheduled` log line — so a task that has stopped stops moving its row
- [ ] `reports::health` gains `overdue_jobs`: jobs whose `next_run_at` is now in the past by more
      than a grace margin. Per-job by construction (the stored instant carries the job's own cadence),
      so weekly, monthly and daily jobs need no separate thresholds
- [ ] The health banner surfaces it, linking to Jobs, with wording that names the job and how long it
      is overdue
- [ ] The Jobs screen gains a **next run** column from `GET /jobs`, so the one surface an operator
      checks can answer "is this still scheduled, and when is it due?"
- [ ] A **manual-only** job (`GET /jobs`'s `trigger` — SCENARIOS T-09/schedule) is never reported
      overdue and gets no `job_schedule` row: it has no schedule by design, so an overdue check must
      leave it alone rather than treating "never ran" as "late"
- [ ] Classify `job_schedule` for snapshot staleness (exempt, with the reason in the migration) —
      `every_table_is_classified_for_snapshot_staleness` fails otherwise
- [ ] `docs/SCHEMA.md` (new table + Relationships), `docs/API.md` (`GET /jobs` shape, health report's
      new field, Response codes if any), README "Scheduled maintenance"
- [ ] Regression tests: a schedule with no future occurrence leaves a stored instant that goes stale
      and health reports the job overdue; a job that ran on time is not overdue; the grace margin's
      boundary

---

## SCENARIOS T-11: a run interrupted by a restart leaves no record, and an unverified file that looks like a good backup

`run_job` records the run **after** the work returns, and `main`'s graceful shutdown waits only for
in-flight *HTTP requests* — a scheduled job runs in a spawned task the process does not wait for.
Reproduced on 2026-08-22 against a 265 MB throwaway database, with a schedule entry firing at a known
second and a `SIGTERM` 0.6 s into it:

```
16:51:00.301  INFO job started job=backup
16:51:00.302  INFO starting backup path=".../big/t-2026-08-22-165100.db"
16:51:00.649  INFO shutting down
```

and on disk afterwards, `t-2026-08-22-165100.db` — 265,175,040 bytes against the live database's
265,199,616. What that leaves behind:

- **No `job_runs` row at all.** `GET /jobs` still shows the *previous* run's result; there is no
  "started but never finished" record, so nothing distinguishes an interrupted run from one that
  never began.
- **A file that was never verified.** `verify_or_quarantine` runs after `VACUUM INTO` returns, in the
  process that just died. The file matches the backup naming pattern exactly, so it is counted by
  retention pruning, can become a first-of-month keeper, and is a restore candidate indistinguishable
  from a verified one. (This particular truncation happened to open and pass `integrity_check` —
  which is the point: whether an interrupted copy is restorable is luck, and nothing checks.)
- Nothing ever re-verifies an existing backup, so the only verification a file gets is the one it
  missed.

The plain operational trigger is `service share-tracker restart` (or a host reboot, or a `pkg`
upgrade) landing on Sunday 00:00 — the weekly backup's own slot.

Related, same directory: a quarantined `<name>.db.bad` is deliberately never pruned ("kept for
diagnosis"), so a failing disk — the likely cause of a verification failure — leaves a full-size copy
per weekly run until the volume fills. Worth settling with whatever this finding takes.

**Question for Evan — which half to fix, and how?**

- **(a) Both: record the start, and never leave an unverified file looking good.** `run_job` inserts
  the `job_runs` row when the job *starts* (finished_at/success NULL) and updates it on completion,
  so an interrupted run is visible as one that started and never finished; and the backup writes to a
  temporary name (`<name>.db.partial`) that is renamed into place only after verification passes, so
  an interrupted copy can never be mistaken for a backup. Startup could additionally sweep leftover
  `.partial` files.
- **(b) The file only.** Write-then-rename as above; leave run recording as it is.
- **(c) The record only.** Start/finish rows as above; leave the file naming as it is (an
  unverified leftover keeps a backup's name).
- **(d) Wait for the job instead.** Hold shutdown until in-flight jobs finish, bounded by a timeout.
  Fixes the common case but not a `SIGKILL`, a power cut, or a timeout expiring.

**Decision (Evan, 2026-08-22): (a), both halves.** Rejected: fixing only the file, only the record,
and waiting for the job on shutdown (it cannot cover `SIGKILL`, a power cut, or the timeout expiring).
The quarantined-file question was settled alongside: **bound the `.bad` files** to the newest few
rather than leaving them unbounded or only documenting them.

- [ ] `run_job` inserts the `job_runs` row when the job **starts** (`finished_at`/`success` NULL) and
      updates that row on completion — an interrupted run is then visible as one that started and
      never finished. Needs a migration relaxing `job_runs.finished_at`/`success` to nullable, and
      `JobStatus`/`JobRunRecord` to carry the in-flight state honestly
- [ ] `GET /jobs` and the Jobs screen show an unfinished run as such (not as a success, not as a
      failure), and the history pruning still bounds the table
- [ ] The backup writes to a staging name (`<name>.db.partial`) and renames it into place **only
      after verification passes**, so an interrupted copy can never carry a backup's name, be counted
      by pruning, or be picked for a restore
- [ ] Startup sweeps leftover `.partial` files for this database (an interrupted run's debris), and
      pruning bounds quarantined `<name>.db.bad` files to the newest few
- [ ] `docs/API.md` (`GET /jobs` shape and the in-flight state, the backup job's paragraph),
      `docs/SCHEMA.md` (`job_runs` columns), README "Scheduled maintenance" (staging + `.bad` bound)
- [ ] Regression tests: a run recorded at start is visible before it finishes; a verification failure
      leaves no file under the backup name (the `.partial` is quarantined instead); a leftover
      `.partial` is swept at startup and never counted by pruning; `.bad` files are bounded

---

## SCENARIOS T-10: `POST /jobs/:name` answers bare status codes with no body

Driven on 2026-08-22:

- `POST /jobs/nope` → **404 with an empty body**. The Jobs screen's `api()` helper turns that into the
  toast `HTTP 404`. CLAUDE.md's own rule for the delete routes — "never a bare `StatusCode::NOT_FOUND`,
  which the web UI can only show as 'HTTP 404'" — is the same rule, and `deleted(found, noun)` exists
  to satisfy it.
- A job that fails → **500 with an empty body**, toasted as `HTTP 500`, even though `run_job` has just
  returned the reason as a `String`. It is recoverable — `viewJobs()` reloads and the row's Error
  column then shows it — but the toast the user reads first says nothing.

The suffix validation, by contrast, is exemplary: `?suffix=../../etc/x`, a leading `-`, an empty value
and a 41-character value each answer **422 with a plain-text reason**, and are rejected *before* the
registry lookup so a malformed request never records a run.

One inconsistency found alongside: an unknown query parameter is silently ignored (`?sufix=pre-0.5.1`
answers 204 and takes an **unlabelled** backup), because `JobParams` derives `Deserialize` without
`deny_unknown_fields`.

**Question for Evan — how far to take it?**

- **(a) Both bodies, and reject unknown query params.** 404 names the job and lists the registered
  names; 500 carries `run_job`'s error text; `JobParams` gets `#[serde(deny_unknown_fields)]` so a
  typo'd `suffix` is a 422 rather than a silently unlabelled backup.
- **(b) Both bodies only** — leave the typo'd parameter silently ignored.
- **(c) The 404 only** — a failed run's reason is already one reload away in the table.

**Decision (Evan, 2026-08-22): (a), all three.** Rejected: the two bodies alone, and the 404 alone.

- [ ] `POST /jobs/:name` for an unknown name answers 404 with a plain-text body naming the job and
      the registered names (the `deleted(found, noun)` convention, one endpoint over)
- [ ] A failed run answers 500 carrying `run_job`'s error text, so the toast the user reads first
      says what went wrong
- [ ] `JobParams` gets `#[serde(deny_unknown_fields)]`, so `?sufix=pre-0.5.1` is a 422 rather than a
      204 taking a silently unlabelled backup
- [ ] `docs/API.md` Jobs section + the Response codes table
- [ ] Regression tests: the 404 body names the job; a failing job's 500 carries its reason; a
      misspelt query parameter is refused rather than ignored

---

## SCENARIOS T-09: a currency-import that skipped the whole ISO 24165 half reports unqualified success

Without `DTI_REGISTRY_USER_ID` / `DTI_REGISTRY_PASSWORD`, `currencies::run_import` fetches the ISO 4217
fiat list, logs `WARN ... skipping ISO 24165 digital token import`, and returns
`ImportSummary { imported: 178 }`. Driven on 2026-08-22: the job records **success**, `GET /jobs` shows
a clean run, and the Jobs screen shows `ok` with no error — nothing in the operational surface says
that half the reference data the job exists to import was never fetched.

The consequence is already known to the project: `listing::UNRECOGNISED_DIGITAL_TOKEN` (closed as
SCENARIOS L-10) exists precisely because "the seeded list is just BTC and ETH" is otherwise a dead end,
and it names the credentials as the remedy. So the *point of use* is well handled; what is missing is
that the job the user would check first reports the gap as a clean success. A green Jobs screen is
evidence the reference data is complete, and here it is not.

**Question for Evan — what should a half-import report?**

- **(a) Say what ran.** `ImportSummary` gains a per-feed breakdown (`fiat`, `tokens: Option<usize>`)
  and the job's INFO line and the `/currencies/import` response carry it; a skipped feed is named in
  the summary rather than only in a WARN. Health/Jobs then have something to show.
- **(b) Fail the job.** A run that could not do half its work is not a success — the operator either
  configures the credentials or accepts a permanently red job.
- **(c) A health alert** — a `reference_data_incomplete` entry while the token feed has never been
  imported, cleared by a successful token import.
- **(d) Documentation only** — the credentials are optional by design and L-10 already names them at
  the point of use.

**Decision (Evan, 2026-08-22): (a), say what ran.** Rejected: failing the job, a health alert, and
documentation only.

- [ ] `currencies::ImportSummary` gains a per-feed breakdown (the fiat count, and the token count as
      an `Option` — `None` meaning the feed was not attempted) rather than one total
- [ ] The job's INFO line and the `POST /currencies/import` response carry it, so a skipped feed is
      named in the summary rather than only in a `WARN` nobody reads
- [ ] The Jobs surface shows a half-import as what it is (the run is still a success — the
      credentials are optional by design; it just no longer *reads* as complete)
- [ ] `docs/API.md` (the currencies import response shape), README where the credentials are described
- [ ] Regression tests: an import with no credentials reports the fiat count and `None` tokens; one
      with both feeds reports both
