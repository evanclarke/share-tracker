-- Bounded per-job run history.
--
-- `job_runs` previously kept exactly one row per job (name as primary key,
-- upserted on every run), so an intermittent failure that later succeeded left
-- no trace. Rebuild it as an append-per-run history table: one row per run,
-- ordered by the autoincrement id. The writer (`scheduler::db_record_run`)
-- prunes each job's rows to the newest 20 in the same transaction, so the
-- table stays bounded without a background sweep.
--
-- The existing one-row-per-job data is migrated forward verbatim: each job's
-- last recorded run becomes its first history row (nothing is dropped).
--
-- No snapshot-staleness triggers are needed: no snapshotted report reads
-- job_runs (it is operational metadata, not a financial fact table).

ALTER TABLE job_runs RENAME TO job_runs_old;

CREATE TABLE job_runs (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,  -- run order (newest = highest)
    name        TEXT    NOT NULL,          -- registry job name (e.g. backup, rba-fx-import)
    started_at  TEXT    NOT NULL,          -- RFC 3339 timestamp the run began
    finished_at TEXT    NOT NULL,          -- RFC 3339 timestamp the run ended
    success     INTEGER NOT NULL,          -- 1 if the run succeeded, 0 if it failed
    error       TEXT                       -- human-readable error when success = 0, else NULL
);

-- Per-job lookups: latest run, history listing, and the prune all filter by
-- name and order by id.
CREATE INDEX job_runs_name_id ON job_runs (name, id);

INSERT INTO job_runs (name, started_at, finished_at, success, error)
    SELECT name, started_at, finished_at, success, error
    FROM job_runs_old
    ORDER BY name;

DROP TABLE job_runs_old;
