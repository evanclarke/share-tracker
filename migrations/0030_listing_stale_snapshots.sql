-- Snapshot-staleness triggers for `listings` (SCENARIOS M-08).
--
-- The schema's rule is that a write to a dated fact marks every report
-- snapshot on or after its date stale, inside the write's own transaction
-- (0001_schema.sql, "Snapshot-staleness triggers"). `listings` was never
-- given a pair, on the reasoning that it holds reference data rather than
-- dated facts — but two of its columns decide what a stored snapshot's
-- figures *mean*:
--
--   * `currency` denominates every stored closing price, so changing it
--     re-values the whole price history. The same stored 200 is A$298.51 as
--     USD and A$333.33 as EUR — one stored fact, two AUD valuations, and
--     before this the snapshot holding the first stayed `stale = 0`.
--   * `security_type` decides which days a listing can be valued on: a Crypto
--     listing trades every calendar day, everything else only on its
--     exchange's trading days, and snapshot generation values each holding at
--     its nearest trading day on or before the snapshot date.
--
-- A listing edit carries no date of its own — the change applies to the whole
-- price history — so the trigger stales every snapshot rather than a suffix.
-- It is deliberately narrowed with a WHEN clause to those two columns: an edit
-- to `name`, `isin` or `price_symbol` changes no stored figure, and staling
-- the whole series for one would leave dates outside the daily job's 14-day
-- catch-up window stale until a manual bulk regeneration — noise that would
-- teach a reader to ignore the flag.
--
-- INSERT and DELETE need no trigger: a listing with no trades has nothing held
-- on any snapshot date, and a delete is refused while any row references it.
-- `PUT /listings/:id` now also refuses a currency change on a listing with
-- recorded trades, income or prices, so in practice the currency arm fires
-- only for a listing whose history arrived later; it is kept because the
-- trigger, not the handler, is what the schema's rule rests on.

CREATE TRIGGER listings_stale_snapshots_update AFTER UPDATE ON listings
WHEN OLD.currency <> NEW.currency OR OLD.security_type <> NEW.security_type
BEGIN
    UPDATE report_snapshots SET stale = 1;
END;
