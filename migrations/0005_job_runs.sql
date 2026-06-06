-- Last-run record per scheduled/on-demand maintenance job. One row per job name
-- (upserted on every run, scheduled or manual), so the Jobs UI and the
-- `GET /jobs` endpoint can show when each job last ran, whether it succeeded, and
-- the error if it failed. Keyed by job name keeps the table bounded to the
-- registered jobs rather than growing per run.
CREATE TABLE job_runs (
    name        TEXT PRIMARY KEY,          -- registry job name (e.g. backup, rba-fx-import)
    started_at  TEXT    NOT NULL,          -- RFC 3339 timestamp the run began
    finished_at TEXT    NOT NULL,          -- RFC 3339 timestamp the run ended
    success     INTEGER NOT NULL,          -- 1 if the run succeeded, 0 if it failed
    error       TEXT                       -- human-readable error when success = 0, else NULL
);
