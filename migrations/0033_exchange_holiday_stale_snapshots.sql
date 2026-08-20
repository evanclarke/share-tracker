-- Snapshot-staleness triggers for `exchange_holidays` (SCENARIOS Q-05/Q-08).
--
-- The schema's rule is that a write which changes what a stored report
-- snapshot should say marks it stale, inside the write's own transaction
-- (0001_schema.sql, "Snapshot-staleness triggers"). `exchange_holidays` was
-- never given a set, on the reasoning that the calendar only influences a
-- value persisted onto a trade at write time — its `settlement_date`. That
-- reasoning was wrong: `reports::valuation::stored_valuations` values every
-- held listing at `market.latest_trading_day_on_or_before(date)`, and that
-- walk reads this table **live**, on every snapshot generation. The calendar
-- is not a write-time input; it is an input to the valuation itself.
--
-- So a holiday write silently re-values stored snapshots:
--
--   * Adding a holiday closes a day the snapshot was valued on, moving the
--     valuation back to the prior close. Observed on a two-holding portfolio:
--     seeding 2025-06-05 as an XASX holiday moved that date's market value
--     from A$5,073.08 to A$4,443.08 — 12.4% — while every snapshot stayed
--     `stale = 0` and the series kept reporting the old figure.
--   * Deleting a seeded holiday is worse in kind: the day becomes a trading
--     day, so the stored snapshot's valuation day is one that was never
--     priced at all. The stored figure stands, unflagged, while a manual
--     regeneration of the same date is *blocked* for want of a price.
--
-- The daily `report-snapshot` job only regenerates dates that are stale or
-- provisional within its window, so an unflagged wrong figure stands
-- indefinitely.
--
-- A holiday only affects snapshots dated on or after it — an earlier snapshot
-- values at an earlier trading day and can never reach it — so all three arms
-- stale the suffix from the holiday's own date, and the UPDATE arm from the
-- earlier of the old and new dates, exactly as the 0001 fact-table triggers
-- do. They are deliberately **not** narrowed to the listings that trade on
-- `mic`: which listings were held on a snapshot date is a per-date question a
-- trigger cannot answer cheaply, and 0030 set the precedent of staling from
-- the date rather than narrowing inside the trigger.
--
-- The UPDATE arm carries a `WHEN` clause for the same reason 0030's does: the
-- only column an UPDATE can reach through the API is `name` (the primary key
-- is `(mic, holiday_date)`, and `db_upsert`'s ON CONFLICT sets `name` alone),
-- and a holiday's name is informational — no calculation reads it. Without
-- the clause, correcting "Queen's Birthday" to "King's Birthday", or
-- re-PUTting a published calendar over itself, would stale years of snapshots
-- for a change to no figure at all, which teaches a reader to ignore the flag.
-- Re-dating or re-exchanging a holiday in place is not reachable through the
-- API either (that is a delete plus an insert, both of which fire above), but
-- the arm is kept because the trigger, not the handler, is what the schema's
-- rule rests on.

CREATE TRIGGER exchange_holidays_stale_snapshots_insert AFTER INSERT ON exchange_holidays
BEGIN
    UPDATE report_snapshots SET stale = 1 WHERE snapshot_date >= NEW.holiday_date;
END;

CREATE TRIGGER exchange_holidays_stale_snapshots_update AFTER UPDATE ON exchange_holidays
WHEN OLD.holiday_date <> NEW.holiday_date OR OLD.mic <> NEW.mic
BEGIN
    UPDATE report_snapshots SET stale = 1
    WHERE snapshot_date >= MIN(OLD.holiday_date, NEW.holiday_date);
END;

CREATE TRIGGER exchange_holidays_stale_snapshots_delete AFTER DELETE ON exchange_holidays
BEGIN
    UPDATE report_snapshots SET stale = 1 WHERE snapshot_date >= OLD.holiday_date;
END;
