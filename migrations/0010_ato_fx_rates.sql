-- ATO published monthly foreign exchange rates.
-- `rate` is foreign currency units per 1 AUD (e.g. USD-per-AUD), stored as TEXT
-- to preserve full Decimal precision. `month` is the rate's period as 'YYYY-MM'.
CREATE TABLE ato_fx_rates (
    id        INTEGER PRIMARY KEY AUTOINCREMENT,
    currency  TEXT NOT NULL,            -- ISO 4217 code, e.g. 'USD'
    month     TEXT NOT NULL,            -- 'YYYY-MM'
    rate      TEXT NOT NULL,            -- Decimal: foreign units per 1 AUD
    UNIQUE (currency, month)
);
