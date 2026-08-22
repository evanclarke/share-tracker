-- Nothing notices a job that has stopped running (SCENARIOS T-11/T-02/T-12).
--
-- `reports::health`'s `failed_jobs` fires only when a job's most recently
-- recorded run *failed*. A job that is not running at all records nothing, so
-- it raises nothing: the last successful run stays in place and the Jobs screen
-- keeps showing `ok`, indefinitely. Driven on 2026-08-22 — a schedule line with
-- no future occurrence (`0 0 30 2 *`, 30 February) is accepted at startup,
-- `run_entry` logs one ERROR and its task exits, and `GET /jobs` went on
-- answering `backup: ok` for the life of the process. Prices and FX each have a
-- database-derived freshness signal that catches their job going quiet
-- (`prices_stale`, `fx_stale`); the backup has none, and it is the job where it
-- matters most — nothing in the database changes when a backup does or does not
-- happen, so one that silently stopped a year ago is indistinguishable from one
-- that ran on Sunday.
--
-- The scheduler already computes the next fire instant every iteration and logs
-- it (`next run scheduled`). This table is that instant **persisted**: a task
-- that has stopped stops moving its row, so a report reading only the database
-- can see it, and the Jobs screen can finally answer "is this still scheduled,
-- and when is it due?".
--
-- One row per *schedule entry*, not per job: `price-import` has three lines
-- (Sydney, New York, UTC), and one of the three dying while the others carry on
-- is exactly the kind of half-failure this exists to catch. The row is keyed by
-- a surrogate id because the table is **the schedule the running process is
-- executing**, rebuilt at every startup: `scheduler::spawn` clears it before
-- spawning the entry tasks, and each task inserts its own row and then updates
-- that row every iteration. That ordering is what settles the removed-line
-- case — a job whose `schedule.cron` line was deleted simply gets no row, so it
-- is not reported overdue for ever after. A lost line is reported by the
-- startup WARN that exists for it (SCENARIOS T-09/schedule); a permanent alarm
-- nobody can clear is the pattern this project has repeatedly had to undo
-- (`unpriced_from`, the duplicate-income key, that same T-09 warning).
--
-- `next_run_at` is stored as a UTC instant so it is comparable with `now`
-- whatever zone the entry is pinned to, with the entry's own IANA zone kept
-- beside it for display — the cron expression alone does not say when
-- `30 17 * * 1-5` fires.
--
-- Not audited by `row_history`: derived operational state, the same scope
-- decision as `job_runs` (2026-07-14, recorded in docs/SCHEMA.md) — every row
-- here is rewritten by the scheduler seconds after any hand edit would land.
-- No staleness triggers either, and none are needed: it holds no financial fact
-- and no snapshotted report reads it, so no write to it can invalidate a stored
-- snapshot. It is therefore classified **exempt** in
-- `reports::snapshot::tests::every_table_is_classified_for_snapshot_staleness`,
-- the way 0006_rights_sales.sql states its own reason.
CREATE TABLE job_schedule (
    id          INTEGER PRIMARY KEY,
    -- Registry job name (e.g. backup, price-import); not a foreign key — the
    -- job list lives in code. Several rows may share a name, one per line.
    name        TEXT    NOT NULL,
    -- The 5-field cron expression exactly as written in the schedule file.
    cron        TEXT    NOT NULL,
    -- The entry's IANA timezone (e.g. Australia/Sydney); NULL when the line
    -- carries none, meaning the expression is in server-local time.
    timezone    TEXT,
    -- RFC 3339 **UTC** instant of the next scheduled run, as the entry's own
    -- timer computed it. Rewritten every iteration; a row whose instant is in
    -- the past by more than the health report's grace margin is a scheduler
    -- that has stopped.
    next_run_at TEXT    NOT NULL,
    -- RFC 3339 UTC instant this row was last written. Informational: the
    -- overdue check reads `next_run_at`, and this is what says how long ago the
    -- task last drew breath when a row looks wrong.
    updated_at  TEXT    NOT NULL
);
