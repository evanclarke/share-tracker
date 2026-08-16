-- SCENARIOS F-23: the `amit` flag had no time dimension, so flipping it on a
-- fund that *converted* to an AMIT part-way through a holding rewrote every
-- earlier year — the tax summary dropped the pre-conversion cash income
-- entirely (its exclusion is `WHERE NOT l.amit`, whole-listing), the AMIT cash
-- cross-check demanded AMMA statements for years the fund was an ordinary
-- trust, and the E4 mechanism those years need was refused on the write.
--
-- `amit_from` dates the status: the listing is an AMIT for records **on or
-- after** this date, and an ordinary trust before it. NULL keeps the old,
-- undated meaning — the flag applies to the whole history — so every existing
-- row is already correct and nothing is migrated. The pairing is the CHECK
-- below: a date only means something on a listing that is an AMIT at all.
ALTER TABLE listings ADD COLUMN amit_from TEXT;

-- SQLite cannot add a table-level CHECK to an existing table, and a column
-- CHECK cannot reference another column, so the pairing (amit_from is only
-- ever set on an `amit` listing) is enforced in `entities::listing::db_upsert`
-- — the same place the rest of the listing's write-time invariants live.
-- Recorded here so the schema and the code agree on where the rule lives.

-- listings is audited (CLAUDE.md rule): ALTER TABLE ADD COLUMN does not update
-- existing triggers, so its two row_history triggers are re-created here with
-- amit_from added to the JSON column list.
DROP TRIGGER listings_row_history_update;
DROP TRIGGER listings_row_history_delete;

CREATE TRIGGER listings_row_history_update AFTER UPDATE ON listings
BEGIN
    INSERT INTO row_history (table_name, row_id, operation, changed_at, old_row)
    VALUES ('listings', OLD.id, 'UPDATE', strftime('%Y-%m-%dT%H:%M:%fZ', 'now'),
            json_object(
                        'id', OLD.id, 'exchange_mic', OLD.exchange_mic, 'ticker', OLD.ticker,
                        'name', OLD.name, 'isin', OLD.isin, 'security_type', OLD.security_type,
                        'currency', OLD.currency, 'amit', OLD.amit, 'preference', OLD.preference,
                        'price_symbol', OLD.price_symbol, 'amit_from', OLD.amit_from));
END;

CREATE TRIGGER listings_row_history_delete AFTER DELETE ON listings
BEGIN
    INSERT INTO row_history (table_name, row_id, operation, changed_at, old_row)
    VALUES ('listings', OLD.id, 'DELETE', strftime('%Y-%m-%dT%H:%M:%fZ', 'now'),
            json_object(
                        'id', OLD.id, 'exchange_mic', OLD.exchange_mic, 'ticker', OLD.ticker,
                        'name', OLD.name, 'isin', OLD.isin, 'security_type', OLD.security_type,
                        'currency', OLD.currency, 'amit', OLD.amit, 'preference', OLD.preference,
                        'price_symbol', OLD.price_symbol, 'amit_from', OLD.amit_from));
END;

-- No snapshot staleness triggers: `listings` carries no dated financial fact
-- that a snapshotted report reads as a figure (the reports read it for the
-- ticker and the AMIT status, both of which move only with a deliberate edit,
-- and a snapshot is regenerated from the fact tables' own triggers).
