-- Ticker/exchange-code renames as an explicit, dated, audited event (REQUIREMENTS
-- "Ticker and exchange-code changes", 2026-07-26), replacing a bare `PUT
-- /listings/:id` field edit with a record of *when* a security's identity
-- changed and what it changed from — LAAC -> LAR being the prompting case.
--
-- One row per rename. old_ticker/old_exchange_mic are always written from the
-- listing's row at the moment of the rename (never trusted from a request
-- body), so the chain cannot be falsified. The CHECK mirrors the write-time
-- rejection of a no-op rename (same ticker, same exchange).
CREATE TABLE listing_renames (
    id               INTEGER PRIMARY KEY,
    listing_id       INTEGER NOT NULL REFERENCES listings(id),
    effective_date   TEXT    NOT NULL,   -- first trading day under the new identity
    old_ticker       TEXT    NOT NULL,
    new_ticker       TEXT    NOT NULL,
    old_exchange_mic TEXT    REFERENCES exchanges(mic),
    new_exchange_mic TEXT    REFERENCES exchanges(mic),
    note             TEXT,
    UNIQUE (listing_id, effective_date),
    CHECK (old_ticker <> new_ticker OR old_exchange_mic IS NOT new_exchange_mic)
);

-- Deliberately no *_stale_snapshots_* trigger pair here (contrast
-- 0001_schema.sql's fact-table triggers): the snapshotted reports (portfolio
-- overview, unrealised gains, performance) carry a listing's ticker only as a
-- display label over `listing_id`, never as a computed figure, and `listings`
-- itself already carries no staleness trigger for the same reason. A stored
-- snapshot showing a pre-rename ticker label is display-only drift, not a
-- wrong figure, so nothing needs to mark it stale.

-- The provider-symbol override (closing_price::yahoo_symbol consults this
-- ahead of its derived ticker/exchange mapping): fixes a symbol Yahoo spells
-- differently, or one on an exchange with no mapping. Not part of the CHECK
-- pairing above -- it's independent of the rename chain, and cross-listing
-- renames should generally leave it alone (an override that matched the old
-- ticker rarely matches the new one).
ALTER TABLE listings ADD COLUMN price_symbol TEXT;

-- listings is audited (CLAUDE.md rule): ALTER TABLE ADD COLUMN does not
-- update existing triggers, so its two row_history triggers are re-created
-- here with price_symbol added to the JSON column list.
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
                        'price_symbol', OLD.price_symbol));
END;

CREATE TRIGGER listings_row_history_delete AFTER DELETE ON listings
BEGIN
    INSERT INTO row_history (table_name, row_id, operation, changed_at, old_row)
    VALUES ('listings', OLD.id, 'DELETE', strftime('%Y-%m-%dT%H:%M:%fZ', 'now'),
            json_object(
                        'id', OLD.id, 'exchange_mic', OLD.exchange_mic, 'ticker', OLD.ticker,
                        'name', OLD.name, 'isin', OLD.isin, 'security_type', OLD.security_type,
                        'currency', OLD.currency, 'amit', OLD.amit, 'preference', OLD.preference,
                        'price_symbol', OLD.price_symbol));
END;

-- listing_renames joins the audited set too, which means the row_history
-- table's own table_name CHECK must accept it -- and SQLite cannot ALTER a
-- table-level CHECK, so row_history is rebuilt via the rename pattern (0012
-- precedent, most recently 0017): rename out, create the new shape, copy
-- every row (explicit ids -- AUTOINCREMENT continues from the copied max, it
-- does not reuse ids), drop the _old table. Dropping row_history_old also
-- drops its own append-only guard triggers (they moved with the rename), so
-- they are re-created below; every *other* audited table's trigger pair
-- lives on that table, not on row_history, but its body still names
-- row_history by string -- and SQLite's default ALTER TABLE RENAME rewrites
-- every trigger/view body that references the renamed name, wherever it
-- lives, to the new name. Left alone, every trigger inserting into
-- row_history (trades_row_history_update, listings_row_history_update, ...)
-- would silently start targeting row_history_old and break the moment
-- row_history_old is dropped below. legacy_alter_table suppresses that
-- rewrite so those bodies still read plain "row_history", which resolves
-- correctly once the name is recreated a few statements down.
PRAGMA legacy_alter_table = ON;
ALTER TABLE row_history RENAME TO row_history_old;
PRAGMA legacy_alter_table = OFF;

CREATE TABLE row_history (
    id         INTEGER PRIMARY KEY AUTOINCREMENT,
    table_name TEXT    NOT NULL CHECK (table_name IN (
                   'trades', 'parcel_allocations', 'income', 'interest_income',
                   'amma_statements', 'amit_adjustments', 'ess_statements',
                   'transfers', 'corporate_actions', 'inheritances',
                   'rights_sales', 'rights_sale_allocations',
                   'investment_expenses', 'drp_enrolments', 'cgt_settings',
                   'attachments', 'listings', 'listing_renames')),
    row_id     INTEGER NOT NULL,
    operation  TEXT    NOT NULL CHECK (operation IN ('UPDATE', 'DELETE')),
    changed_at TEXT    NOT NULL,
    old_row    TEXT    NOT NULL
);

INSERT INTO row_history (id, table_name, row_id, operation, changed_at, old_row)
    SELECT id, table_name, row_id, operation, changed_at, old_row
    FROM row_history_old
    ORDER BY id;

-- row_history_old's own row_history_row index (it moved with the rename,
-- keeping its original name) must go before the new one can be created.
DROP TABLE row_history_old;

CREATE INDEX row_history_row ON row_history (table_name, row_id);

CREATE TRIGGER row_history_append_only_update BEFORE UPDATE ON row_history
BEGIN
    SELECT RAISE(ABORT, 'row_history is append-only');
END;

CREATE TRIGGER row_history_append_only_delete BEFORE DELETE ON row_history
BEGIN
    SELECT RAISE(ABORT, 'row_history is append-only');
END;

-- listing_renames' own trigger pair. There is no UPDATE path for a rename
-- record today (the entity only supports insert-and-undo-the-newest), so the
-- update trigger never fires in practice -- created anyway for symmetry with
-- every other audited table, and so a future update path is covered from day
-- one rather than silently unaudited.
CREATE TRIGGER listing_renames_row_history_update AFTER UPDATE ON listing_renames
BEGIN
    INSERT INTO row_history (table_name, row_id, operation, changed_at, old_row)
    VALUES ('listing_renames', OLD.id, 'UPDATE', strftime('%Y-%m-%dT%H:%M:%fZ', 'now'),
            json_object(
                        'id', OLD.id, 'listing_id', OLD.listing_id,
                        'effective_date', OLD.effective_date,
                        'old_ticker', OLD.old_ticker, 'new_ticker', OLD.new_ticker,
                        'old_exchange_mic', OLD.old_exchange_mic,
                        'new_exchange_mic', OLD.new_exchange_mic, 'note', OLD.note));
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
                        'new_exchange_mic', OLD.new_exchange_mic, 'note', OLD.note));
END;
