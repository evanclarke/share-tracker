-- A run interrupted by a restart leaves a record (SCENARIOS T-11).
--
-- `run_job` recorded a run only *after* the work returned, and the graceful
-- shutdown waits on in-flight HTTP requests, not on the spawned task a
-- scheduled job runs in. So a `service share-tracker restart` (or a reboot, or
-- a pkg upgrade) landing on Sunday 00:00 — the weekly backup's own slot —
-- killed the run mid-write and left no `job_runs` row at all: `GET /jobs` went
-- on showing the *previous* run's result, and nothing distinguished a run that
-- was interrupted from one that never began.
--
-- The row is now written when the run **starts** and updated when it returns,
-- which needs two schema changes:
--
--   * `finished_at` becomes nullable — an in-flight run has not finished;
--   * the boolean `success` becomes a three-valued `status`
--     ('running' / 'ok' / 'failed'). A started-but-unfinished run is neither a
--     success nor a failure, and this project spells a limited set of values as
--     a CHECK-constrained enum rather than leaving it to be inferred from which
--     of two nullable columns happens to be NULL.
--
-- SQLite can neither relax a NOT NULL nor add a table-level CHECK in place, so
-- the table is rebuilt with the established rename-and-rebuild pattern.
-- **Nothing is dropped**: every existing row is carried forward with its id,
-- name, timestamps and error text unchanged, and its success flag translated
-- (1 -> 'ok', 0 -> 'failed'). `success` does not survive as a column of its own
-- because `status` says everything it said and more; keeping both would give
-- one fact two sources of truth, which the codebase treats as a defect.
--
-- The `finished_at`/`status` CHECK below is the invariant the two columns share:
-- a run is 'running' exactly while it has no finish time. It is enforced here,
-- in the write's own transaction, rather than only in the reader.
--
-- No triggers are re-created because the table carries none, and that is
-- unchanged by this migration: `job_runs` is derived operational state, out of
-- scope for the `row_history` audit trail (scope decision 2026-07-14, recorded
-- in docs/SCHEMA.md), and exempt from snapshot staleness because no snapshotted
-- report reads it (the reason 0012 gave, still true — it stays classified as
-- exempt in `reports::snapshot`).

-- Index names are global in SQLite and a renamed table keeps its indexes under
-- their original names, so 0012's index must go before the new table can claim
-- the name. Re-created verbatim against the new table below.
DROP INDEX job_runs_name_id;

ALTER TABLE job_runs RENAME TO job_runs_old;

CREATE TABLE job_runs (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,  -- run order (newest = highest)
    name        TEXT    NOT NULL,          -- registry job name (e.g. backup, rba-fx-import)
    started_at  TEXT    NOT NULL,          -- RFC 3339 timestamp the run began
    finished_at TEXT,                      -- RFC 3339 timestamp the run ended; NULL while it runs
    -- 'running' from the moment the run starts until it returns, then 'ok' or
    -- 'failed'. A row left 'running' is a run that started and never finished.
    status      TEXT    NOT NULL CHECK (status IN ('running', 'ok', 'failed')),
    error       TEXT,                      -- human-readable error when status = 'failed', else NULL
    -- The two columns describe the same instant in the run's life; a finished
    -- run has a finish time and an unfinished one does not.
    CHECK ((status = 'running') = (finished_at IS NULL))
);

-- Per-job lookups: latest run, history listing, and the prune all filter by
-- name and order by id. Unchanged from 0012.
CREATE INDEX job_runs_name_id ON job_runs (name, id);

-- Ids are preserved, so the recorded run order (and the health report's
-- MAX(id)-per-name "latest run" lookup) is exactly what it was.
INSERT INTO job_runs (id, name, started_at, finished_at, status, error)
    SELECT id, name, started_at, finished_at,
           CASE WHEN success THEN 'ok' ELSE 'failed' END,
           error
    FROM job_runs_old
    ORDER BY id;

DROP TABLE job_runs_old;
