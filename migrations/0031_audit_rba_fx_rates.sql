-- rba_fx_rates joins the append-only audit trail, and gains its own
-- snapshot-staleness trigger (SCENARIOS M-13).
--
-- 0013 excluded the table as "import-managed and re-importable" reference
-- data. That premise held only while nothing could change a stored rate — and
-- nothing could, which was itself the problem: the import is
-- `INSERT … ON CONFLICT DO NOTHING` and the resource is read-only over HTTP,
-- so the first value imported for a (currency, month) was final. A rate that
-- landed wrong — a typo in a hand-pasted retry CSV (the endpoint accepts one
-- precisely for retries), a truncated download, an upstream revision — could
-- be corrected only by editing the database by hand, and every tax figure in
-- that currency-month rested on it. `PUT /rba_fx_rates/:id` now corrects one,
-- which is exactly the closing_prices story of 0021: a reference row that
-- became hand-correctable is a user-entered value feeding every conversion,
-- so it is audited.
--
-- The id is already AUTOINCREMENT (0001), so a history trail always belongs to
-- exactly one row — no rebuild is needed here, unlike 0021's.
--
-- The staleness trigger is new too, and only for UPDATE. 0001's note that "FX
-- imports fire none" was right about INSERTs and stays true: an import filling
-- a month that had no rate is what the provisional true-up handles (a
-- provisional snapshot is regenerated, not staled — the facts were never
-- wrong, only the rate was interim). *Changing* a stored rate is different:
-- every snapshot from that month on was valued at the old figure, so they are
-- stale in the ordinary sense. `month` is 'YYYY-MM', so the suffix starts at
-- its first day. DELETE needs no counterpart — there is no delete route, and
-- the audit trigger below covers one arriving later.
--
-- Ordering: row_history's table_name CHECK must accept 'rba_fx_rates' before
-- the trigger that writes it can fire.

-- ---------------------------------------------------------------------------
-- 1. Extend row_history's table_name CHECK.
--
-- A table-level CHECK SQLite cannot ALTER, so row_history is rebuilt via the
-- rename pattern exactly as 0018 and 0021 did: legacy_alter_table suppresses
-- SQLite's rewrite of every trigger body that names row_history — every
-- audited table's trigger pair would otherwise be repointed at
-- row_history_old and break the moment it is dropped.
-- ---------------------------------------------------------------------------

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
                   'attachments', 'listings', 'listing_renames',
                   'closing_prices', 'tax_year_settings', 'rba_fx_rates')),
    row_id     INTEGER NOT NULL,
    operation  TEXT    NOT NULL CHECK (operation IN ('UPDATE', 'DELETE')),
    changed_at TEXT    NOT NULL,
    old_row    TEXT    NOT NULL
);

INSERT INTO row_history (id, table_name, row_id, operation, changed_at, old_row)
    SELECT id, table_name, row_id, operation, changed_at, old_row
    FROM row_history_old
    ORDER BY id;

-- Drops row_history_old's index and its own append-only guard triggers, both
-- of which moved with the rename; all three are re-created below.
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

-- ---------------------------------------------------------------------------
-- 2. Audit rba_fx_rates.
--
-- The UPDATE trigger is what retains a superseded rate: correcting one is an
-- UPDATE, so the figure every earlier report was computed at stays recoverable
-- rather than being overwritten out of existence.
-- ---------------------------------------------------------------------------

CREATE TRIGGER rba_fx_rates_row_history_update AFTER UPDATE ON rba_fx_rates
BEGIN
    INSERT INTO row_history (table_name, row_id, operation, changed_at, old_row)
    VALUES ('rba_fx_rates', OLD.id, 'UPDATE', strftime('%Y-%m-%dT%H:%M:%fZ', 'now'),
            json_object('id', OLD.id, 'currency', OLD.currency,
                        'month', OLD.month, 'rate', OLD.rate));
END;

CREATE TRIGGER rba_fx_rates_row_history_delete AFTER DELETE ON rba_fx_rates
BEGIN
    INSERT INTO row_history (table_name, row_id, operation, changed_at, old_row)
    VALUES ('rba_fx_rates', OLD.id, 'DELETE', strftime('%Y-%m-%dT%H:%M:%fZ', 'now'),
            json_object('id', OLD.id, 'currency', OLD.currency,
                        'month', OLD.month, 'rate', OLD.rate));
END;

-- ---------------------------------------------------------------------------
-- 3. A corrected rate stales every snapshot from its month on.
-- ---------------------------------------------------------------------------

CREATE TRIGGER rba_fx_rates_stale_snapshots_update AFTER UPDATE ON rba_fx_rates
WHEN OLD.rate <> NEW.rate
BEGIN
    UPDATE report_snapshots SET stale = 1 WHERE snapshot_date >= OLD.month || '-01';
END;
