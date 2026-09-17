-- no-transaction
-- Three enum-shaped columns were stored as free text, with no CHECK: the
-- project's rule is that a field holding a limited set of values is a
-- CHECK-constrained column (and a typed enum where parsed in Rust), and these
-- three were the last outliers (2026-09-17 review, integrity). Every one of
-- them is written from a compile-time-known set, so the only thing a free-text
-- column buys is a silent typo that no later read can detect:
--
--   * `mic_registry.status` — the ISO 10383 STATUS, `ACTIVE` | `UPDATED` |
--     `EXPIRED`. `entities::mic_registry::parse_registry` reads it from the
--     published feed; the migration's CHECK is the backstop for a feed value
--     the typed parse does not know.
--   * `distribution_events.source` — the provider that produced the row.
--     `DistributionFetcher::source()` makes it a one-value set today
--     (`'yahoo'`, the only implementor), and `db_store` compares
--     `source <> excluded.source` to decide whether a re-fetch is a revision,
--     so a typo silently changes that decision.
--   * `closing_prices.source` — the only prior constraint was
--     `CHECK ((source = 'manual') = (origin = 'manual'))`, which pins the
--     manual rows but leaves a *fetched* row's provider slot free text. The
--     live set is `{'yahoo', 'manual'}`: `PriceFetcher::source()` answers
--     `'yahoo'` and the manual-entry path writes `'manual'`.
--
-- SQLite cannot add a table-level CHECK in place, so each table is rebuilt by
-- the RENAME `_old` + CREATE + `INSERT … SELECT` pattern of 0029/0045. Because
-- `ALTER TABLE x RENAME TO x_old` rewrites other tables' REFERENCES clauses
-- (and trigger bodies) whenever foreign keys are enabled, and `foreign_keys`
-- is a documented no-op inside a transaction, this migration runs
-- `-- no-transaction` and brackets its own work in BEGIN/COMMIT, with
-- `legacy_alter_table` suppressing the trigger-body rewrite around each
-- rename — exactly as 0045 did.
--
-- `distribution_events` and `closing_prices` are **audited**: both carry a
-- `*_row_history_*` trigger pair, which is dropped before the rename and
-- re-created (with the same full column list) after the copy, so a rebuilt
-- table still records every prior version of every column. `closing_prices`
-- additionally carries its two snapshot-staleness triggers (the `AFTER UPDATE`
-- of 0001/0034 and the `AFTER DELETE` of 0050), re-created *after* the copy so
-- the migration's own INSERTs do not stale every stored snapshot. `mic_registry`
-- is not audited (it is import-managed reference data, out of the 2026-07-14
-- audit scope), and carries neither triggers nor a surrogate id.
--
-- Every row keeps its data and — for the two audited tables — its id, which is
-- the link target `row_history.row_id` keys a trail on. The AUTOINCREMENT
-- sequence is reseeded from the larger of the largest live id and the largest
-- `row_id` the table ever recorded in `row_history`, exactly as 0045 did: a
-- plain copy sets the sequence to the largest live id, which would re-issue an
-- id deleted above it and hand the new row the deleted one's history.

PRAGMA foreign_keys = OFF;

BEGIN;

-- ---------------------------------------------------------------------------
-- 1. closing_prices.source — {yahoo, manual}
--
-- All four triggers this table carries are dropped first (the 0034/0050
-- staleness pair and the 0038 audit pair) and re-created against the new
-- table below.
-- ---------------------------------------------------------------------------

DROP TRIGGER closing_prices_stale_snapshots_update;
DROP TRIGGER closing_prices_stale_snapshots_delete;
DROP TRIGGER closing_prices_row_history_update;
DROP TRIGGER closing_prices_row_history_delete;

PRAGMA legacy_alter_table = ON;
ALTER TABLE closing_prices RENAME TO closing_prices_old;
PRAGMA legacy_alter_table = OFF;

CREATE TABLE closing_prices (
    -- Server-assigned surrogate key: the row's identity for the audit trail
    -- (row_history.row_id). Never reused — see 0021.
    id           INTEGER PRIMARY KEY AUTOINCREMENT,
    listing_id   INTEGER NOT NULL REFERENCES listings(id),
    -- The trading day the price closes: the date in the exchange's timezone,
    -- or for exchange-less (Crypto) listings the UTC date of the daily candle
    -- that completes at 00:00 UTC at the end of that date.
    price_date   TEXT    NOT NULL,
    -- Decimal as TEXT, in the listing's quote currency (NOT AUD-converted —
    -- reports convert via the FX rules), and in the unit basis in force on
    -- price_date. NULL exactly when status = 'error'.
    price        TEXT,
    -- Decimal as TEXT: the figure exactly as the provider served it (or as the
    -- operator entered it), in the unit basis in force when it was observed —
    -- which fetched_at dates. `price` is derived from it by the ratio of the
    -- ShareSplit/BonusIssue actions dated in (price_date, fetched_at]; for a
    -- hand-entered row the two are equal. NULL exactly when status = 'error'.
    price_as_observed TEXT,
    -- Provider that produced the row: 'yahoo' for every fetched row, 'manual'
    -- exactly when the price was entered by hand (CHECK-paired with origin
    -- below). The CHECK is what keeps the fetched half of the slot from being
    -- free text (0051).
    source       TEXT    NOT NULL CHECK (source IN ('yahoo', 'manual')),
    -- RFC 3339 UTC timestamp of the fetch that produced the row — for a manual
    -- row, of the entry that recorded it. Also dates the unit basis
    -- price_as_observed arrived in (see the 0034 header note).
    fetched_at   TEXT    NOT NULL,
    -- The provider symbol the row was fetched under (0038), in the namespace
    -- of `source`. Informational: no calculation reads it.
    fetched_symbol TEXT  CHECK (origin = 'fetched' OR fetched_symbol IS NULL),
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
    -- The observation is present exactly when the price is, so a re-base can
    -- never find an ok row it has nothing to re-derive from.
    CHECK ((price_as_observed IS NOT NULL) = (status = 'ok')),
    CHECK ((error IS NOT NULL) = (status = 'error')),
    CHECK ((sourced_from IS NOT NULL) = (origin = 'manual')),
    CHECK ((reason IS NOT NULL) = (origin = 'manual')),
    -- A hand-entered row always carries a price: there is no such thing as a
    -- manual fetch failure.
    CHECK (origin = 'fetched' OR status = 'ok'),
    -- The provider slot agrees with the origin, so neither can drift.
    CHECK ((source = 'manual') = (origin = 'manual'))
);

INSERT INTO closing_prices
    (id, listing_id, price_date, price, price_as_observed, source, fetched_at,
     fetched_symbol, status, error, origin, sourced_from, reason)
    SELECT id, listing_id, price_date, price, price_as_observed, source, fetched_at,
           fetched_symbol, status, error, origin, sourced_from, reason
    FROM closing_prices_old
    ORDER BY id;

DROP TABLE closing_prices_old;

-- Never hand a deleted row's id to a new one: the sequence follows the largest
-- id the table, or its trail, has ever seen (0045's rule; see the header).
INSERT INTO sqlite_sequence (name, seq)
    SELECT 'closing_prices', 0
    WHERE NOT EXISTS (SELECT 1 FROM sqlite_sequence WHERE name = 'closing_prices');
UPDATE sqlite_sequence
    SET seq = MAX(seq, (SELECT COALESCE(MAX(row_id), 0) FROM row_history
                         WHERE table_name = 'closing_prices'))
    WHERE name = 'closing_prices';

-- Re-created after the copy so the migration's own INSERTs stale nothing.
-- Unchanged from 0001/0034 and 0050.
CREATE TRIGGER closing_prices_stale_snapshots_update AFTER UPDATE ON closing_prices
WHEN OLD.status = 'ok' AND (NEW.status <> 'ok' OR OLD.price <> NEW.price)
BEGIN
    UPDATE report_snapshots SET stale = 1 WHERE snapshot_date >= OLD.price_date;
END;

CREATE TRIGGER closing_prices_stale_snapshots_delete AFTER DELETE ON closing_prices
WHEN OLD.status = 'ok'
BEGIN
    UPDATE report_snapshots SET stale = 1 WHERE snapshot_date >= OLD.price_date;
END;

-- Unchanged from 0038: both record every column, including the new CHECK on
-- source (the column list is identical — 0051 changed no column).
CREATE TRIGGER closing_prices_row_history_update AFTER UPDATE ON closing_prices
BEGIN
    INSERT INTO row_history (table_name, row_id, operation, changed_at, old_row)
    VALUES ('closing_prices', OLD.id, 'UPDATE', strftime('%Y-%m-%dT%H:%M:%fZ', 'now'),
            json_object(
                        'id', OLD.id, 'listing_id', OLD.listing_id,
                        'price_date', OLD.price_date, 'price', OLD.price,
                        'price_as_observed', OLD.price_as_observed,
                        'source', OLD.source, 'fetched_at', OLD.fetched_at,
                        'fetched_symbol', OLD.fetched_symbol,
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
                        'price_as_observed', OLD.price_as_observed,
                        'source', OLD.source, 'fetched_at', OLD.fetched_at,
                        'fetched_symbol', OLD.fetched_symbol,
                        'status', OLD.status, 'error', OLD.error,
                        'origin', OLD.origin, 'sourced_from', OLD.sourced_from,
                        'reason', OLD.reason));
END;

-- ---------------------------------------------------------------------------
-- 2. distribution_events.source — {'yahoo'}
--
-- The provider trait makes this a one-value set today. Both audit triggers are
-- dropped and re-created with the same column list after the copy.
-- ---------------------------------------------------------------------------

DROP TRIGGER distribution_events_row_history_update;
DROP TRIGGER distribution_events_row_history_delete;

PRAGMA legacy_alter_table = ON;
ALTER TABLE distribution_events RENAME TO distribution_events_old;
PRAGMA legacy_alter_table = OFF;

CREATE TABLE distribution_events (
    -- Surrogate key: the row's identity for the audit trail
    -- (row_history.row_id). AUTOINCREMENT so it is never reused.
    id              INTEGER PRIMARY KEY AUTOINCREMENT,
    listing_id      INTEGER NOT NULL REFERENCES listings(id),
    -- The ex-dividend date in the exchange's own timezone.
    ex_date         TEXT    NOT NULL,
    -- Decimal as TEXT, in the listing's quote currency (NOT AUD-converted).
    amount_per_unit TEXT    NOT NULL,
    currency        TEXT    NOT NULL REFERENCES currencies(code),
    -- Provider that produced the row. One value today: the only
    -- DistributionFetcher is the Yahoo adapter, and db_store's revision guard
    -- compares this column, so a typo would silently change that decision.
    source          TEXT    NOT NULL CHECK (source IN ('yahoo')),
    -- The provider symbol the row was fetched under, in the namespace of
    -- `source`. Informational: no calculation reads it.
    fetched_symbol  TEXT    NOT NULL,
    -- RFC 3339 UTC timestamp of the fetch that produced the row.
    fetched_at      TEXT    NOT NULL,
    UNIQUE (listing_id, ex_date)
);

INSERT INTO distribution_events
    (id, listing_id, ex_date, amount_per_unit, currency, source, fetched_symbol, fetched_at)
    SELECT id, listing_id, ex_date, amount_per_unit, currency, source, fetched_symbol, fetched_at
    FROM distribution_events_old
    ORDER BY id;

DROP TABLE distribution_events_old;

INSERT INTO sqlite_sequence (name, seq)
    SELECT 'distribution_events', 0
    WHERE NOT EXISTS (SELECT 1 FROM sqlite_sequence WHERE name = 'distribution_events');
UPDATE sqlite_sequence
    SET seq = MAX(seq, (SELECT COALESCE(MAX(row_id), 0) FROM row_history
                         WHERE table_name = 'distribution_events'))
    WHERE name = 'distribution_events';

CREATE TRIGGER distribution_events_row_history_update AFTER UPDATE ON distribution_events
BEGIN
    INSERT INTO row_history (table_name, row_id, operation, changed_at, old_row)
    VALUES ('distribution_events', OLD.id, 'UPDATE', strftime('%Y-%m-%dT%H:%M:%fZ', 'now'),
            json_object('id', OLD.id, 'listing_id', OLD.listing_id,
                        'ex_date', OLD.ex_date, 'amount_per_unit', OLD.amount_per_unit,
                        'currency', OLD.currency, 'source', OLD.source,
                        'fetched_symbol', OLD.fetched_symbol, 'fetched_at', OLD.fetched_at));
END;

CREATE TRIGGER distribution_events_row_history_delete AFTER DELETE ON distribution_events
BEGIN
    INSERT INTO row_history (table_name, row_id, operation, changed_at, old_row)
    VALUES ('distribution_events', OLD.id, 'DELETE', strftime('%Y-%m-%dT%H:%M:%fZ', 'now'),
            json_object('id', OLD.id, 'listing_id', OLD.listing_id,
                        'ex_date', OLD.ex_date, 'amount_per_unit', OLD.amount_per_unit,
                        'currency', OLD.currency, 'source', OLD.source,
                        'fetched_symbol', OLD.fetched_symbol, 'fetched_at', OLD.fetched_at));
END;

-- ---------------------------------------------------------------------------
-- 3. mic_registry.status — {ACTIVE, UPDATED, EXPIRED}
--
-- Import-managed reference data, not audited: no triggers and no surrogate id,
-- so the rebuild is the rename/create/copy/drop alone.
-- ---------------------------------------------------------------------------

PRAGMA legacy_alter_table = ON;
ALTER TABLE mic_registry RENAME TO mic_registry_old;
PRAGMA legacy_alter_table = OFF;

CREATE TABLE mic_registry (
    mic           TEXT PRIMARY KEY,           -- the MIC (ISO 10383), e.g. 'XASX'
    operating_mic TEXT NOT NULL,              -- parent operating MIC (== mic for operating entries)
    name          TEXT NOT NULL,              -- MARKET NAME-INSTITUTION DESCRIPTION
    country_code  TEXT NOT NULL,              -- ISO 3166 alpha-2 country code
    city          TEXT,                       -- city (nullable; some entries omit it)
    status        TEXT NOT NULL               -- ISO STATUS: ACTIVE | UPDATED | EXPIRED
                  CHECK (status IN ('ACTIVE', 'UPDATED', 'EXPIRED')),
    expiry_date   TEXT                        -- ISO date 'YYYY-MM-DD' when EXPIRED, else NULL
);

INSERT INTO mic_registry (mic, operating_mic, name, country_code, city, status, expiry_date)
    SELECT mic, operating_mic, name, country_code, city, status, expiry_date
    FROM mic_registry_old;

DROP TABLE mic_registry_old;

COMMIT;

-- Restored for the connection this ran on; every other pooled connection opens
-- with foreign keys on (infra::db).
PRAGMA foreign_keys = ON;
