-- Convert REAL columns to TEXT for arbitrary-precision decimal storage.
-- Uses rename pattern: rename originals to _old, create with correct schema,
-- copy data with CAST, drop _old tables.

-- Step 1: rename originals out of the way (income first since it references trades)
ALTER TABLE income RENAME TO income_old;
ALTER TABLE trades RENAME TO trades_old;

-- Step 2: create new tables with TEXT decimal columns
CREATE TABLE trades (
    id                  INTEGER PRIMARY KEY,
    trade_type          TEXT    NOT NULL CHECK(trade_type IN ('Buy', 'Sell', 'DRP')),
    date                TEXT    NOT NULL,
    settlement_date     TEXT    NOT NULL,
    listing_id          INTEGER NOT NULL REFERENCES listings(id),
    average_price       TEXT    NOT NULL,
    quantity            TEXT    NOT NULL,
    currency            TEXT    NOT NULL,
    brokerage           TEXT    NOT NULL DEFAULT '0',
    gst_on_brokerage    TEXT    NOT NULL DEFAULT '0',
    brokerage_currency  TEXT    NOT NULL,
    fx_rate             TEXT    NOT NULL DEFAULT '1',
    contract_note_ref   TEXT
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
    reinvestment_trade_id       INTEGER REFERENCES trades(id)
);

-- Step 3: copy data, casting REAL columns to TEXT
INSERT INTO trades
SELECT
    id,
    trade_type,
    date,
    settlement_date,
    listing_id,
    CAST(average_price    AS TEXT),
    CAST(quantity         AS TEXT),
    currency,
    CAST(brokerage        AS TEXT),
    CAST(gst_on_brokerage AS TEXT),
    brokerage_currency,
    CAST(fx_rate          AS TEXT),
    contract_note_ref
FROM trades_old;

INSERT INTO income
SELECT
    id,
    listing_id,
    date_paid,
    ex_date,
    CAST(franked_amount             AS TEXT),
    CAST(unfranked_amount           AS TEXT),
    CAST(foreign_source_income      AS TEXT),
    CAST(foreign_tax_paid           AS TEXT),
    CAST(tfn_withholding_tax        AS TEXT),
    CAST(franking_credits           AS TEXT),
    CAST(lic_capital_gain_deduction AS TEXT),
    CAST(conduit_foreign_income     AS TEXT),
    trust_income,
    reinvestment_trade_id
FROM income_old;

-- Step 4: drop _old tables (income_old first due to its FK into trades)
DROP TABLE income_old;
DROP TABLE trades_old;
