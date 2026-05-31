-- Consolidated baseline schema.
--
-- This squashes the original incremental migrations (0001-0017) into a single
-- schema definition. The application was still pre-1.0 with little live data, so
-- the historical step-by-step migrations (REAL->TEXT conversions, table rebuilds
-- to add foreign keys, column additions) were collapsed into the final shape they
-- produced. All monetary/quantity columns are TEXT (arbitrary-precision Decimal);
-- no column is ever REAL, so the float-imprecision concerns of the old REAL->TEXT
-- conversions cannot recur.
--
-- Tables are created parents-before-children so REFERENCES resolve cleanly.

-- Reference data: recognised currencies (ISO 4217 fiat + ISO 24165 digital tokens).
-- Every currency code in the model is foreign-keyed to this table.
CREATE TABLE currencies (
    code         TEXT PRIMARY KEY,                                      -- ISO 4217 alpha code (fiat) or ISO 24165 DTI (token)
    kind         TEXT NOT NULL CHECK (kind IN ('Fiat', 'DigitalToken')),
    numeric_code TEXT,                                                  -- ISO 4217 numeric code (fiat only; NULL for tokens)
    name         TEXT NOT NULL,                                         -- currency name (fiat) or token long name
    short_name   TEXT,                                                  -- token short name / ticker (NULL when none)
    minor_units  INTEGER,                                               -- ISO 4217 minor unit / token decimals; informational, NULL when N.A.
    source       TEXT NOT NULL CHECK (source IN ('Iso4217', 'Iso24165'))
);

-- Operational exchanges (the curated, settlement-aware list the app trades against).
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

-- Reference/import data with no outgoing foreign keys.

-- ISO 10383 MIC registry (validation list; populated by the monthly import).
CREATE TABLE mic_registry (
    mic           TEXT PRIMARY KEY,           -- the MIC (ISO 10383), e.g. 'XASX'
    operating_mic TEXT NOT NULL,              -- parent operating MIC (== mic for operating entries)
    name          TEXT NOT NULL,              -- MARKET NAME-INSTITUTION DESCRIPTION
    country_code  TEXT NOT NULL,              -- ISO 3166 alpha-2 country code
    city          TEXT,                       -- city (nullable; some entries omit it)
    status        TEXT NOT NULL,              -- ISO STATUS: ACTIVE | UPDATED | EXPIRED
    expiry_date   TEXT                        -- ISO date 'YYYY-MM-DD' when EXPIRED, else NULL
);

-- RBA F11 monthly reference rates used for ATO AUD conversion (populated by import).
CREATE TABLE rba_fx_rates (
    id        INTEGER PRIMARY KEY AUTOINCREMENT,
    currency  TEXT NOT NULL,            -- ISO 4217 code, e.g. 'USD'
    month     TEXT NOT NULL,            -- 'YYYY-MM'
    rate      TEXT NOT NULL,            -- Decimal: foreign units per 1 AUD
    UNIQUE (currency, month)
);
