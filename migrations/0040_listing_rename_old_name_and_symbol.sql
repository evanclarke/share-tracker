-- Make undoing a rename a real undo (SCENARIOS R-04/R-08, found 2026-08-21).
--
-- `POST /listings/:id/rename` can change four fields on the listing —
-- `ticker`, `exchange_mic`, `name` and `price_symbol` — but `listing_renames`
-- recorded the prior value of only two, so `listing_rename::db_undo` could
-- restore only those two. The other two survived the undo. `price_symbol` is
-- the one that matters: `closing_price::yahoo_symbol_for` uses it verbatim,
-- ahead of the derived ticker/exchange mapping, for every date in the
-- listing's *current* identity — which after an undo is the restored, older
-- identity. So the undo left the listing collecting prices under a symbol
-- that existed only because of the rename that was undone (reproduced: a
-- listing renamed OLD -> NEWER with `"price_symbol":"NEWER.AX"`, undone, then
-- a fetch that still asked the provider for NEWER.AX and stored an errored
-- row saying so). These two columns record what the rename overwrote, so the
-- undo can put all four fields back.
--
-- Both are written from the listing's own row at the moment of the rename,
-- never from the request body — the rule `old_ticker`/`old_exchange_mic` have
-- followed since 0018, for the same reason: the chain must not be
-- falsifiable.
--
-- **Existing rows stay NULL, and NULL there means "nothing recorded".** A
-- rename recorded before this migration did not keep what it overwrote, and
-- neither the name nor the symbol it replaced is recoverable from anything
-- the database holds. Filling the columns in from the listing's *current* row
-- would record today's values as if they were the pre-rename ones, and an
-- undo would then "restore" a listing to exactly what it already is while
-- claiming to have reverted something. So an old row is left unrecorded and
-- its undo behaves exactly as it did before: `ticker` and `exchange_mic` are
-- restored, `name` and `price_symbol` are left alone. This migration changes
-- no existing row's undo behaviour.
--
-- **Which is why "nothing recorded" and "it was NULL before" have to be
-- distinguishable.** `listings.price_symbol` is itself nullable, so a rename
-- that *sets* a symbol on a listing that had none must be undoable back to
-- NULL — and a bare NULL in `old_price_symbol` cannot tell that apart from an
-- unrecorded pre-0040 row. The marker chosen is `old_name`, rather than a
-- third "was recorded" flag column: `listings.name` is NOT NULL (0001), so a
-- rename recorded from 0040 on *always* writes a non-NULL `old_name`, which
-- leaves `old_name IS NULL` meaning one thing only — this row predates the
-- columns. The CHECK below makes that reading enforceable rather than merely
-- conventional: a row with no recorded name can carry no recorded symbol
-- either. `db_undo` reads it exactly that way — all four fields restored when
-- `old_name` is present (`old_price_symbol` NULL then meaning "it was NULL"),
-- `ticker`/`exchange_mic` only when it is not.
--
-- No calculation reads either column: they are consumed by the undo, and
-- served on `GET /listings/:id/renames` so the chain shows what each rename
-- replaced as well as what it set.
--
-- ADD COLUMN, not a rebuild: the CHECK is expressible as a column constraint,
-- so there is no table-level constraint to introduce and nothing to copy.
-- Maintenance rule (0013): `listing_renames` is audited, so both of its
-- `*_row_history_*` triggers are dropped and re-created below with the new
-- columns in their `json_object` lists — a column the trail drops is a
-- version of the row that can never be recovered. There are no
-- `*_stale_snapshots_*` triggers on this table to re-create, and 0018 states
-- why it has none: a ticker is a display label over `listing_id` in every
-- snapshotted report, never a computed figure, so no stored figure can change
-- because a rename row did.

ALTER TABLE listing_renames ADD COLUMN old_name TEXT;
ALTER TABLE listing_renames ADD COLUMN old_price_symbol TEXT
    CHECK (old_name IS NOT NULL OR old_price_symbol IS NULL);

DROP TRIGGER listing_renames_row_history_update;
DROP TRIGGER listing_renames_row_history_delete;

CREATE TRIGGER listing_renames_row_history_update AFTER UPDATE ON listing_renames
BEGIN
    INSERT INTO row_history (table_name, row_id, operation, changed_at, old_row)
    VALUES ('listing_renames', OLD.id, 'UPDATE', strftime('%Y-%m-%dT%H:%M:%fZ', 'now'),
            json_object(
                        'id', OLD.id, 'listing_id', OLD.listing_id,
                        'effective_date', OLD.effective_date,
                        'old_ticker', OLD.old_ticker, 'new_ticker', OLD.new_ticker,
                        'old_exchange_mic', OLD.old_exchange_mic,
                        'new_exchange_mic', OLD.new_exchange_mic,
                        'old_name', OLD.old_name,
                        'old_price_symbol', OLD.old_price_symbol,
                        'note', OLD.note));
END;

CREATE TRIGGER listing_renames_row_history_delete AFTER DELETE ON listing_renames
BEGIN
    INSERT INTO row_history (table_name, row_id, operation, changed_at, old_row)
    VALUES ('listing_renames', OLD.id, 'DELETE', strftime('%Y-%m-%dT%H:%M:%fZ', 'now'),
            json_object(
                        'id', OLD.id, 'listing_id', OLD.listing_id,
                        'effective_date', OLD.effective_date,
                        'old_ticker', OLD.old_ticker, 'new_ticker', OLD.new_ticker,
                        'old_exchange_mic', OLD.old_exchange_mic,
                        'new_exchange_mic', OLD.new_exchange_mic,
                        'old_name', OLD.old_name,
                        'old_price_symbol', OLD.old_price_symbol,
                        'note', OLD.note));
END;
