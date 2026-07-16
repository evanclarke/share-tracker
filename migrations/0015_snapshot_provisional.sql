-- Provisional report snapshots (REQUIREMENTS 2026-07-16): the RBA publishes a
-- month's average FX rate only after the month ends, so a current-month
-- snapshot of a non-AUD holding is valued at the bounded earlier-month
-- fallback rate (infra::fx::resolve_valuation_rate) instead of failing.
-- `provisional` records that any conversion in the generation run used such a
-- fallback rate — distinct from `stale` (a back-dated fact changed the books
-- after generation). Regenerating the snapshot once the real month's rate is
-- imported clears the flag.
--
-- Additive only: no data loss, existing rows are final (0). The snapshot
-- staleness triggers are unaffected (they mark `stale`, not `provisional`),
-- and report_snapshots itself is derived state — not audited, not a fact
-- table, so no new trigger set is needed.
ALTER TABLE report_snapshots
    ADD COLUMN provisional INTEGER NOT NULL DEFAULT 0 CHECK (provisional IN (0, 1));
