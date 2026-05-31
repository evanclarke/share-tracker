-- Enforce that every currency code recorded in the model is a recognised currency
-- (i.e. exists in `currencies`). SQLite cannot add a foreign key to an existing
-- table, so each currency-bearing table is rebuilt via the rename pattern (rename
-- to _old, recreate with the FK, copy data, drop _old) — the same approach as
-- migration 0006. No data is dropped: every row is copied into the new table.
--
-- A RENAME rewrites the foreign-key references in *other* tables to point at the
-- renamed `_old` table, so the whole FK-connected cluster is rebuilt together and
-- the `_old` tables are dropped in child→parent order (a child's rows are removed
-- before the parent they reference). The three tables without a currency column
-- (parcel_allocations, amit_adjustments, drp_enrolments) are rebuilt too, only to
-- reset their references onto the new tables. Foreign keys stay enforced
-- throughout (sqlx runs the migration in a transaction, so toggling `foreign_keys`
-- here would be a no-op); the copy therefore checks each currency against
-- `currencies` as it goes.

-- Safety net: backfill any currency code present in existing data but not seeded in
-- 0016 (e.g. a database using currencies beyond the seeded set) so the FK-checked
-- copies below can never fail. Placeholder rows (name = code) are refined in place
-- by the next currency import.
INSERT OR IGNORE INTO currencies (code, kind, name, source)
    SELECT DISTINCT currency, 'Fiat', currency, 'Iso4217' FROM exchanges       WHERE currency IS NOT NULL;
INSERT OR IGNORE INTO currencies (code, kind, name, source)
    SELECT DISTINCT currency, 'Fiat', currency, 'Iso4217' FROM listings        WHERE currency IS NOT NULL;
INSERT OR IGNORE INTO currencies (code, kind, name, source)
    SELECT DISTINCT currency, 'Fiat', currency, 'Iso4217' FROM trades          WHERE currency IS NOT NULL;
INSERT OR IGNORE INTO currencies (code, kind, name, source)
    SELECT DISTINCT brokerage_currency, 'Fiat', brokerage_currency, 'Iso4217' FROM trades WHERE brokerage_currency IS NOT NULL;
INSERT OR IGNORE INTO currencies (code, kind, name, source)
    SELECT DISTINCT currency, 'Fiat', currency, 'Iso4217' FROM income          WHERE currency IS NOT NULL;
INSERT OR IGNORE INTO currencies (code, kind, name, source)
    SELECT DISTINCT currency, 'Fiat', currency, 'Iso4217' FROM amma_statements WHERE currency IS NOT NULL;

-- Step 1: rename every table in the FK cluster out of the way.
ALTER TABLE parcel_allocations RENAME TO parcel_allocations_old;
ALTER TABLE amit_adjustments   RENAME TO amit_adjustments_old;
ALTER TABLE drp_enrolments     RENAME TO drp_enrolments_old;
ALTER TABLE income             RENAME TO income_old;
ALTER TABLE amma_statements    RENAME TO amma_statements_old;
ALTER TABLE trades             RENAME TO trades_old;
ALTER TABLE listings           RENAME TO listings_old;
ALTER TABLE exchanges          RENAME TO exchanges_old;

-- Step 2: create the new tables (parents before children so REFERENCES resolve).
-- The five currency-bearing tables gain `REFERENCES currencies(code)`; the rest are
-- recreated unchanged so their references bind to the new tables.
CREATE TABLE exchanges (
    mic             TEXT    PRIMARY KEY,
    name            TEXT    NOT NULL,
    country         TEXT    NOT NULL,
    currency        TEXT    NOT NULL REFERENCES currencies(code),
    timezone        TEXT    NOT NULL,
    settlement_days INTEGER NOT NULL
);

CREATE TABLE listings (
    id            INTEGER PRIMARY KEY,
    exchange_mic  TEXT    NOT NULL REFERENCES exchanges(mic),
    ticker        TEXT    NOT NULL,
    name          TEXT    NOT NULL,
    isin          TEXT,
    security_type TEXT    NOT NULL CHECK(security_type IN ('Share', 'ETF', 'LIC', 'Trust')),
    currency      TEXT    NOT NULL REFERENCES currencies(code),
    amit          INTEGER NOT NULL DEFAULT 0,
    UNIQUE(exchange_mic, ticker)
);

CREATE TABLE trades (
    id                  INTEGER PRIMARY KEY,
    trade_type          TEXT    NOT NULL CHECK(trade_type IN ('Buy', 'Sell', 'DRP')),
    date                TEXT    NOT NULL,
    settlement_date     TEXT    NOT NULL,
    listing_id          INTEGER NOT NULL REFERENCES listings(id),
    average_price       TEXT    NOT NULL,
    quantity            TEXT    NOT NULL,
    currency            TEXT    NOT NULL REFERENCES currencies(code),
    brokerage           TEXT    NOT NULL DEFAULT '0',
    gst_on_brokerage    TEXT    NOT NULL DEFAULT '0',
    brokerage_currency  TEXT    NOT NULL REFERENCES currencies(code),
    fx_rate             TEXT    NOT NULL DEFAULT '1',
    contract_note_ref   TEXT,
    residual_brought_forward TEXT NOT NULL DEFAULT '0',
    residual_carried_forward TEXT NOT NULL DEFAULT '0',
    residual_paid_out        TEXT NOT NULL DEFAULT '0'
);

CREATE TABLE amma_statements (
    id                              INTEGER PRIMARY KEY,
    listing_id                      INTEGER NOT NULL REFERENCES listings(id),
    tax_year_end_date               TEXT    NOT NULL,
    units_held                      TEXT    NOT NULL DEFAULT '0',
    date_received                   TEXT    NOT NULL,
    australian_interest             TEXT    NOT NULL DEFAULT '0',
    australian_dividends_unfranked  TEXT    NOT NULL DEFAULT '0',
    franked_dividends               TEXT    NOT NULL DEFAULT '0',
    franking_credits                TEXT    NOT NULL DEFAULT '0',
    net_rent                        TEXT    NOT NULL DEFAULT '0',
    foreign_income                  TEXT    NOT NULL DEFAULT '0',
    foreign_tax_credits             TEXT    NOT NULL DEFAULT '0',
    other_income                    TEXT    NOT NULL DEFAULT '0',
    cgt_discount_gains              TEXT    NOT NULL DEFAULT '0',
    cgt_indexation_gains            TEXT    NOT NULL DEFAULT '0',
    cgt_other_gains                 TEXT    NOT NULL DEFAULT '0',
    capital_losses_applied          TEXT    NOT NULL DEFAULT '0',
    tax_deferred_amount             TEXT    NOT NULL DEFAULT '0',
    tax_free_amount                 TEXT    NOT NULL DEFAULT '0',
    cost_base_adjustment            TEXT    NOT NULL DEFAULT '0',
    tfn_withholding_tax             TEXT    NOT NULL DEFAULT '0',
    currency                        TEXT    NOT NULL DEFAULT 'AUD' REFERENCES currencies(code)
);

CREATE TABLE income (
    id                          INTEGER PRIMARY KEY,
    listing_id                  INTEGER NOT NULL REFERENCES listings(id),
    date_paid                   TEXT    NOT NULL,
    ex_date                     TEXT,
    franked_amount              TEXT    NOT NULL DEFAULT '0',
    unfranked_amount            TEXT    NOT NULL DEFAULT '0',
    foreign_source_income       TEXT    NOT NULL DEFAULT '0',
    foreign_tax_paid            TEXT    NOT NULL DEFAULT '0',
    tfn_withholding_tax         TEXT    NOT NULL DEFAULT '0',
    franking_credits            TEXT    NOT NULL DEFAULT '0',
    lic_capital_gain_deduction  TEXT    NOT NULL DEFAULT '0',
    conduit_foreign_income      TEXT    NOT NULL DEFAULT '0',
    trust_income                INTEGER NOT NULL DEFAULT 0,
    reinvestment_trade_id       INTEGER REFERENCES trades(id),
    currency                    TEXT    NOT NULL DEFAULT 'AUD' REFERENCES currencies(code)
);

CREATE TABLE parcel_allocations (
    id                INTEGER PRIMARY KEY,
    sale_trade_id     INTEGER NOT NULL REFERENCES trades(id),
    purchase_trade_id INTEGER NOT NULL REFERENCES trades(id),
    quantity_allocated TEXT    NOT NULL
);

CREATE TABLE amit_adjustments (
    id                 INTEGER PRIMARY KEY,
    amma_statement_id  INTEGER NOT NULL REFERENCES amma_statements(id),
    trade_id           INTEGER NOT NULL REFERENCES trades(id),
    quantity           TEXT    NOT NULL
);

CREATE TABLE drp_enrolments (
    listing_id        INTEGER PRIMARY KEY REFERENCES listings(id),
    residual_handling TEXT NOT NULL DEFAULT 'CarryForward'
        CHECK(residual_handling IN ('CarryForward', 'PayOut'))
);

-- Step 3: copy data (parents before children so each FK-checked insert resolves).
INSERT INTO exchanges          SELECT * FROM exchanges_old;
INSERT INTO listings           SELECT * FROM listings_old;
INSERT INTO trades             SELECT * FROM trades_old;
INSERT INTO amma_statements    SELECT * FROM amma_statements_old;
INSERT INTO income             SELECT * FROM income_old;
INSERT INTO parcel_allocations SELECT * FROM parcel_allocations_old;
INSERT INTO amit_adjustments   SELECT * FROM amit_adjustments_old;
INSERT INTO drp_enrolments     SELECT * FROM drp_enrolments_old;

-- Step 4: drop the _old tables in child→parent order (each child's rows are gone
-- before the parent it referenced, so no foreign-key action is triggered).
DROP TABLE parcel_allocations_old;
DROP TABLE amit_adjustments_old;
DROP TABLE drp_enrolments_old;
DROP TABLE income_old;
DROP TABLE amma_statements_old;
DROP TABLE trades_old;
DROP TABLE listings_old;
DROP TABLE exchanges_old;
