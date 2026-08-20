-- exchange_holidays joins the append-only audit trail (SCENARIOS Q-05/Q-08,
-- decided 2026-08-21).
--
-- 0013 excluded the table — with `exchanges` — as one that "only influences
-- values persisted onto trades at write time": its holidays are read when a
-- trade's settlement_date is computed, and the computed date is then stored.
-- 0033 falsified that half of the sentence for the calendar:
-- `reports::valuation::stored_valuations` values every held listing at
-- `market.latest_trading_day_on_or_before(date)`, and that walk reads this
-- table **live**, on every snapshot generation — which is why 0033 had to give
-- it staleness triggers. The audited set's own criterion (scope decision
-- 2026-07-14) is "every user-entered table whose values feed a calculation",
-- and the holiday calendar visibly meets it.
--
-- The rest of the case, which 0033 stated and left open: the table is
-- hand-editable by `PUT`/`DELETE` and has been from the start (unlike
-- closing_prices in 0020 and rba_fx_rates in 0031, whose write paths arrived
-- later and retired their exclusions with them); there is no import to
-- re-derive it from, the seed being a one-off in 0001_schema.sql; so a deleted
-- holiday is unrecoverable from anything the database holds. And a wrong
-- holiday is silent twice over — it changes every settlement date recomputed
-- afterwards, and every snapshot valuation from that date on. 0033 flags the
-- second effect (the snapshots go stale) but retains nothing about *what* was
-- changed, which is the trail's job, not the flag's.
--
-- `exchanges` deliberately stays out, on the reasoning 0013 gave, which is
-- still true of it: `settlement_days` is consumed when a trade is written and
-- persisted onto the trade, and `timezone`/`close_time` decide only which
-- dates are *generable*, never what a stored snapshot says.
--
-- Auditing needs an integer row identity: row_history.row_id keys on the
-- audited row's `id`, and exchange_holidays had only the composite
-- (mic, holiday_date) primary key. So the table is rebuilt via the rename
-- pattern to add a surrogate `id` — exactly what 0021 did to closing_prices,
-- for exactly this reason — with the old primary key kept as a UNIQUE
-- constraint, so one holiday per (exchange, day) is still enforced and
-- db_upsert's ON CONFLICT target still resolves.
--
-- The id is AUTOINCREMENT, for 0021's reason: it is server-assigned, and a
-- plain INTEGER PRIMARY KEY reuses the highest rowid after a delete, so
-- deleting a holiday and later adding another would hand the new row the
-- deleted one's id — and with it the deleted row's audit history. AUTOINCREMENT
-- never reuses an id, so a trail always belongs to exactly one holiday.
--
-- Writes still address a holiday by (mic, holiday_date): every route is keyed
-- on the natural key and none changes shape. The id appears in the API only so
-- a history lookup can be keyed on it (POST /reports/row_history with
-- table = 'exchange_holidays'), which is why the GET responses and the
-- Exchange Holidays list now carry it.
--
-- No new index: 0019 skipped this table because its composite primary key
-- already led with `mic`, and the UNIQUE constraint replacing that key below is
-- backed by an index with the same leading column. Nothing else in the schema
-- references exchange_holidays — no foreign key points at it, and the only
-- other objects on it were the three 0033 triggers dropped and re-created here
-- — so the rename needs neither `PRAGMA foreign_keys = OFF` nor the
-- out-of-transaction shape 0029 required.
--
-- Ordering below matters: exchange_holidays must have its id before the
-- triggers that record OLD.id, and row_history's table_name CHECK must accept
-- 'exchange_holidays' before any of those triggers can fire.

-- ---------------------------------------------------------------------------
-- 1. Rebuild exchange_holidays with a surrogate primary key.
-- ---------------------------------------------------------------------------

-- Dropped before the rename so SQLite's ALTER TABLE trigger-body rewrite has
-- nothing to rewrite; re-created verbatim against the new table below. No
-- other trigger in the schema names exchange_holidays.
DROP TRIGGER exchange_holidays_stale_snapshots_insert;
DROP TRIGGER exchange_holidays_stale_snapshots_update;
DROP TRIGGER exchange_holidays_stale_snapshots_delete;

ALTER TABLE exchange_holidays RENAME TO exchange_holidays_old;

CREATE TABLE exchange_holidays (
    -- Server-assigned surrogate key: the row's identity for the audit trail
    -- (row_history.row_id). Never reused — see the header note.
    id           INTEGER PRIMARY KEY AUTOINCREMENT,
    mic          TEXT NOT NULL REFERENCES exchanges(mic),
    holiday_date TEXT NOT NULL,  -- ISO 'YYYY-MM-DD'; a full-closure non-trading day
    name         TEXT NOT NULL,  -- holiday name (informational)
    -- The former primary key: still one holiday per (exchange, day), and still
    -- the conflict target of exchange_holiday::db_upsert's upsert.
    UNIQUE (mic, holiday_date)
);

-- Ids are assigned here for the first time, earliest holiday first, so they
-- ascend with the calendar they describe.
INSERT INTO exchange_holidays (mic, holiday_date, name)
    SELECT mic, holiday_date, name FROM exchange_holidays_old
    ORDER BY holiday_date, mic;

DROP TABLE exchange_holidays_old;

-- Unchanged from 0033 (see its long note for why the calendar stales
-- snapshots at all, and why the UPDATE arm is WHEN-narrowed): a holiday only
-- affects snapshots dated on or after it, so all three arms stale the suffix
-- from its date, and a name-only correction stales nothing.
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

-- ---------------------------------------------------------------------------
-- 2. Extend row_history's table_name CHECK to accept 'exchange_holidays'.
--
-- A table-level CHECK SQLite cannot ALTER, so row_history is rebuilt via the
-- rename pattern exactly as 0018, 0021, 0027 and 0031 did (see 0018's long
-- note): legacy_alter_table suppresses SQLite's rewrite of every trigger body
-- that names row_history — every audited table's trigger pair would otherwise
-- be repointed at row_history_old and break the moment it is dropped.
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
                   'closing_prices', 'tax_year_settings', 'rba_fx_rates',
                   'exchange_holidays')),
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
-- 3. Audit exchange_holidays.
--
-- Both triggers record every column of the row, `name` included: the name is
-- informational to the *calculations*, but a trail that dropped it could not
-- say which holiday a deleted date was. The DELETE trigger is the one that
-- matters most here — a deleted holiday is recoverable from nothing else, the
-- seed being a one-off — and the UPDATE trigger covers both a re-dating (not
-- reachable through the API, which re-dates by delete + insert) and the
-- name correction the upsert performs.
--
-- Unlike the 0033 staleness trigger above, neither is WHEN-narrowed. The two
-- answer different questions: staleness asks "did a stored figure change?",
-- to which a name correction answers no, while the trail asks "what did this
-- row say before that write?" — and every audited table in the schema records
-- every UPDATE, so re-PUTting a published calendar over itself leaves an
-- entry per row saying exactly that.
-- ---------------------------------------------------------------------------

CREATE TRIGGER exchange_holidays_row_history_update AFTER UPDATE ON exchange_holidays
BEGIN
    INSERT INTO row_history (table_name, row_id, operation, changed_at, old_row)
    VALUES ('exchange_holidays', OLD.id, 'UPDATE', strftime('%Y-%m-%dT%H:%M:%fZ', 'now'),
            json_object('id', OLD.id, 'mic', OLD.mic,
                        'holiday_date', OLD.holiday_date, 'name', OLD.name));
END;

CREATE TRIGGER exchange_holidays_row_history_delete AFTER DELETE ON exchange_holidays
BEGIN
    INSERT INTO row_history (table_name, row_id, operation, changed_at, old_row)
    VALUES ('exchange_holidays', OLD.id, 'DELETE', strftime('%Y-%m-%dT%H:%M:%fZ', 'now'),
            json_object('id', OLD.id, 'mic', OLD.mic,
                        'holiday_date', OLD.holiday_date, 'name', OLD.name));
END;
