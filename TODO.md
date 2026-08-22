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
raised **six findings**. Five are closed — the three jobs that recorded their failure as a Rust
`Debug` string (T-06), the startup "no schedule entry" warning that cried wolf on the two
deliberately-manual jobs (T-09/schedule), `POST /jobs/:name`'s bare-status-code failures, now
bodied 404/500 replies with an unknown query parameter refused rather than ignored (T-10), the
run interrupted by a restart that left no record and an unverified file wearing a backup's name
(T-11), now a run row opened at the start and a backup staged under `.partial` until it verifies,
and the job that stops running and is never noticed (T-11/T-02/T-12), now a `job_schedule` table the
scheduler rewrites every iteration with a health `overdue_jobs` list and a **next run** column on the
Jobs screen over it — all five archived in [`DONE/infra.md`](DONE/infra.md); the currency
half-import (T-09) is open below.
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

The next work after T-09 is the next SCENARIOS pass — section **U. Audit trail and history**
(8 scenarios) — driven the way S and T were: run every scenario against a throwaway database, apply
the standing probes to each, and log what each raises as a `## SCENARIOS U-nn` section here with the
option Evan chose. Four lessons are worth carrying into it. First, **check the live database
read-only before proposing a refusal** (the M lesson, which is what turned S-05 from a refusal into a
flag — trade 9071 would have been bricked). Second, **a decision can rest on a distinction the schema
cannot make**: S-04's "rewrite the auto-computed settlement dates, leave the stated ones" was
unanswerable until a provenance column recorded which path wrote each date, and the finding's own
write-up never mentioned it. Third, from T: **drive the interruption, don't reason about it** — the
mid-backup restart finding (archived as T-11) only became concrete once a 265 MB database made the
write slow enough to kill, and the file it left behind was not the one the code reading predicted.
Fourth, also from T: **a finding's own summary of what an option covers can be over-generous** —
T-11/T-02/T-12's decision said persisting the next run catches "the server that was down", and it
does not: the schedule is rebuilt at every startup, so a missed run is refreshed forward. Re-derive
what a chosen option actually covers rather than repeating the write-up's claim.

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
