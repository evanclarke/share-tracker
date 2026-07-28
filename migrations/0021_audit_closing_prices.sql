-- closing_prices joins the append-only audit trail.
--
-- 0020 made a closing price hand-enterable, which broke the premise the
-- table's audit exclusion rested on (0013: "import-managed and re-importable"
-- reference data). A manual price is a user-entered value that feeds every
-- valuation — exactly what the audit scope decision covers — and overwriting
-- one manual price with another silently discarded the superseded figure and
-- its sourced_from/reason. That is the gap this migration closes.
--
-- Auditing needs an integer row identity: row_history.row_id keys on the
-- audited row's `id`, and closing_prices had only the composite
-- (listing_id, price_date) primary key. So the table is rebuilt once more via
-- the rename pattern (0017/0020 precedent) to add a surrogate `id`, with the
-- old primary key kept as a UNIQUE constraint so one price per (listing, day)
-- is still enforced and db_store's ON CONFLICT target still resolves.
--
-- The id is AUTOINCREMENT, unlike the client-supplied ids elsewhere: it is
-- server-assigned, and a plain INTEGER PRIMARY KEY reuses the highest rowid
-- after a delete. Discarding an errored row and later storing a price for
-- another day would then hand the new row the deleted one's id — and with it
-- the deleted row's audit history. AUTOINCREMENT never reuses an id, so a
-- trail always belongs to exactly one row.
--
-- Writes still address a row by (listing_id, price_date) — the id appears in
-- the API only so a history lookup can be keyed on it (POST /reports/
-- row_history with table = 'closing_prices').
--
-- No new index is needed for the listing_id foreign key: 0019 skipped this
-- table because its composite primary key already led with listing_id, and the
-- UNIQUE constraint that replaces that key below is backed by an index with
-- the same leading column.
--
-- Ordering below matters: closing_prices must have its id before the triggers
-- that record OLD.id, and row_history's table_name CHECK must accept
-- 'closing_prices' before any of those triggers can fire.

-- ---------------------------------------------------------------------------
-- 1. Rebuild closing_prices with a surrogate primary key.
-- ---------------------------------------------------------------------------

-- Dropped before the rename so SQLite's ALTER TABLE trigger-body rewrite has
-- nothing to rewrite; re-created against the new table below. No other
-- trigger in the schema names closing_prices.
DROP TRIGGER closing_prices_stale_snapshots_update;

ALTER TABLE closing_prices RENAME TO closing_prices_old;

CREATE TABLE closing_prices (
    -- Server-assigned surrogate key: the row's identity for the audit trail
    -- (row_history.row_id). Never reused — see the header note.
    id           INTEGER PRIMARY KEY AUTOINCREMENT,
    listing_id   INTEGER NOT NULL REFERENCES listings(id),
    -- The trading day the price closes: the date in the exchange's timezone,
    -- or for exchange-less (Crypto) listings the UTC date of the daily candle
    -- that completes at 00:00 UTC at the end of that date.
    price_date   TEXT    NOT NULL,
    -- Decimal as TEXT, in the listing's quote currency (NOT AUD-converted —
    -- reports convert via the FX rules). NULL exactly when status = 'error'.
    price        TEXT,
    -- Provider that produced the row, e.g. 'yahoo'; 'manual' exactly when the
    -- price was entered by hand (CHECK-paired with origin below).
    source       TEXT    NOT NULL,
    -- RFC 3339 UTC timestamp of the fetch that produced the row — for a manual
    -- row, of the entry that recorded it.
    fetched_at   TEXT    NOT NULL,
    status       TEXT    NOT NULL CHECK (status IN ('ok', 'error')),
    error        TEXT,              -- failure detail, NULL exactly when status = 'ok'
    -- How the row came to be: fetched from the provider, or entered by hand.
    origin       TEXT    NOT NULL DEFAULT 'fetched' CHECK (origin IN ('fetched', 'manual')),
    -- Where a manual price was sourced from (e.g. 'asx.com.au closing report').
    -- NULL exactly when origin = 'fetched'.
    sourced_from TEXT,
    -- Why manual entry was needed (e.g. 'provider serves no candle since the
    -- delisting'). NULL exactly when origin = 'fetched'.
    reason       TEXT,
    -- The former primary key: still one price per (listing, day), and still
    -- the conflict target of closing_price::db_store's upsert.
    UNIQUE (listing_id, price_date),
    CHECK ((price IS NOT NULL) = (status = 'ok')),
    CHECK ((error IS NOT NULL) = (status = 'error')),
    CHECK ((sourced_from IS NOT NULL) = (origin = 'manual')),
    CHECK ((reason IS NOT NULL) = (origin = 'manual')),
    -- A hand-entered row always carries a price: there is no such thing as a
    -- manual fetch failure.
    CHECK (origin = 'fetched' OR status = 'ok'),
    -- The provider slot agrees with the origin, so neither can drift.
    CHECK ((source = 'manual') = (origin = 'manual'))
);

-- Ids are assigned here for the first time, oldest price first, so they
-- ascend with the history they describe.
INSERT INTO closing_prices
    (listing_id, price_date, price, source, fetched_at, status, error, origin, sourced_from, reason)
    SELECT listing_id, price_date, price, source, fetched_at, status, error, origin, sourced_from, reason
    FROM closing_prices_old
    ORDER BY price_date, listing_id;

DROP TABLE closing_prices_old;

-- Unchanged from 0001/0020: revising a stored ok price stales the snapshots
-- that were valued at it. Still no INSERT or DELETE counterpart — a new price
-- fills a date that was blocked (no snapshot exists to stale), and an ok row
-- (fetched or manual) is never deletable.
CREATE TRIGGER closing_prices_stale_snapshots_update AFTER UPDATE ON closing_prices
WHEN OLD.status = 'ok' AND (NEW.status <> 'ok' OR OLD.price <> NEW.price)
BEGIN
    UPDATE report_snapshots SET stale = 1 WHERE snapshot_date >= OLD.price_date;
END;

-- ---------------------------------------------------------------------------
-- 2. Extend row_history's table_name CHECK to accept 'closing_prices'.
--
-- A table-level CHECK SQLite cannot ALTER, so row_history is rebuilt via the
-- rename pattern exactly as 0018 did (see its long note): legacy_alter_table
-- suppresses SQLite's rewrite of every trigger body that names row_history —
-- every audited table's trigger pair would otherwise be repointed at
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
                   'closing_prices')),
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
-- 3. Audit closing_prices.
--
-- Both triggers record every column of the row. The UPDATE trigger is what
-- retains a superseded manual price with its sourced_from/reason: the upsert
-- that replaces it is an UPDATE, so the old figure and its provenance land in
-- the trail instead of being lost. The DELETE trigger covers discarding an
-- errored row — the only delete the API allows.
-- ---------------------------------------------------------------------------

CREATE TRIGGER closing_prices_row_history_update AFTER UPDATE ON closing_prices
BEGIN
    INSERT INTO row_history (table_name, row_id, operation, changed_at, old_row)
    VALUES ('closing_prices', OLD.id, 'UPDATE', strftime('%Y-%m-%dT%H:%M:%fZ', 'now'),
            json_object(
                        'id', OLD.id, 'listing_id', OLD.listing_id,
                        'price_date', OLD.price_date, 'price', OLD.price,
                        'source', OLD.source, 'fetched_at', OLD.fetched_at,
                        'status', OLD.status, 'error', OLD.error,
                        'origin', OLD.origin, 'sourced_from', OLD.sourced_from,
                        'reason', OLD.reason));
END;

CREATE TRIGGER closing_prices_row_history_delete AFTER DELETE ON closing_prices
BEGIN
    INSERT INTO row_history (table_name, row_id, operation, changed_at, old_row)
    VALUES ('closing_prices', OLD.id, 'DELETE', strftime('%Y-%m-%dT%H:%M:%fZ', 'now'),
            json_object(
                        'id', OLD.id, 'listing_id', OLD.listing_id,
                        'price_date', OLD.price_date, 'price', OLD.price,
                        'source', OLD.source, 'fetched_at', OLD.fetched_at,
                        'status', OLD.status, 'error', OLD.error,
                        'origin', OLD.origin, 'sourced_from', OLD.sourced_from,
                        'reason', OLD.reason));
END;
