-- Recognised currencies reference table covering both fiat currencies (ISO 4217,
-- imported from the SIX Group "List One" XML) and digital tokens (ISO 24165,
-- imported from the DTIF registry) by the `currency-import` maintenance job. It is
-- standalone reference data keyed by `code` (no foreign keys): its role is to let
-- the currency codes recorded on trades/income/AMMA be validated against a real,
-- recognised list. `minor_units` is informational only — stored amounts remain
-- arbitrary-precision Decimal and are never rounded to it.
CREATE TABLE currencies (
    code         TEXT PRIMARY KEY,                                      -- ISO 4217 alpha code (fiat) or ISO 24165 DTI (token)
    kind         TEXT NOT NULL CHECK (kind IN ('Fiat', 'DigitalToken')),
    numeric_code TEXT,                                                  -- ISO 4217 numeric code (fiat only; NULL for tokens)
    name         TEXT NOT NULL,                                         -- currency name (fiat) or token long name
    short_name   TEXT,                                                  -- token short name / ticker (NULL when none)
    minor_units  INTEGER,                                               -- ISO 4217 minor unit / token decimals; informational, NULL when N.A.
    source       TEXT NOT NULL CHECK (source IN ('Iso4217', 'Iso24165'))
);
