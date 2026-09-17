-- Removing a stored ok closing price stales the snapshots that were valued
-- at it.
--
-- `closing_prices` carried only `closing_prices_stale_snapshots_update` (0001,
-- re-created by 0020, 0021 and 0034): revising a stored ok price — erroring it
-- out, correcting it by hand, or re-basing it when a split is recorded —
-- staled every snapshot dated on or after its `price_date`, but *removing* the
-- row did not.
--
-- The reasoning was that the rows the API's delete guard lets through are ones
-- no stored figure was valued at: an errored row, whose date
-- `reports::valuation` blocks outright, and (since 0037) an ok row inside the
-- listing's `unpriced_before` span, where the marker supersedes the stored
-- rows and the holding is excluded from the date's totals rather than priced.
-- That is a sound statement about the guard, but it made the snapshot flags
-- depend on the guard being airtight rather than on the fact itself — and it
-- was not: `delete_one` read the row and the listing's marker on the pool
-- (`db_get_one`, `listing::db_get`), decided, then deleted in a separate
-- unguarded statement, so a concurrent manual `PUT` or re-fetch could store an
-- ok price for the same `(listing_id, price_date)` in that window and the
-- delete then removed a figure a stored snapshot had been valued at, leaving
-- it `stale = 0` for ever (2026-09-17 review). The handler now reads inside
-- the write transaction it deletes on (`infra::db::write_tx`), so the window
-- is gone; this trigger is the backstop that makes the flag follow the fact
-- instead: any ok row that goes away stales every snapshot dated on or after
-- its `price_date`, in the deleting transaction, whatever removed it.
--
-- Narrowed to `OLD.status = 'ok'`, exactly as the UPDATE arm is: an errored
-- row holds no figure, so no stored valuation can rest on it, and staling from
-- its date would only queue a regeneration that must block. The bulk clear of
-- a superseded span (`db_clear_unpriced_before`) therefore stales the
-- snapshots the marker had already staled — a redundant but idempotent
-- regeneration of figures the excluded holding never entered.

CREATE TRIGGER closing_prices_stale_snapshots_delete AFTER DELETE ON closing_prices
WHEN OLD.status = 'ok'
BEGIN
    UPDATE report_snapshots SET stale = 1 WHERE snapshot_date >= OLD.price_date;
END;
