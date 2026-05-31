-- Seed the currencies reference table with a baseline of common fiat currencies so
-- the currency foreign keys added in 0017 are satisfied without first running an
-- import (the import later upserts authoritative names / minor units in place).
-- Mirrors the seeded exchanges (XASX→AUD, XNYS→USD). `INSERT OR IGNORE` keeps this
-- idempotent and avoids clobbering rows an import may already have refined.
INSERT OR IGNORE INTO currencies (code, kind, numeric_code, name, short_name, minor_units, source) VALUES
    ('AUD', 'Fiat', '036', 'Australian Dollar',     NULL,  2, 'Iso4217'),
    ('USD', 'Fiat', '840', 'US Dollar',             NULL,  2, 'Iso4217'),
    ('NZD', 'Fiat', '554', 'New Zealand Dollar',    NULL,  2, 'Iso4217'),
    ('GBP', 'Fiat', '826', 'Pound Sterling',        NULL,  2, 'Iso4217'),
    ('EUR', 'Fiat', '978', 'Euro',                  NULL,  2, 'Iso4217'),
    ('JPY', 'Fiat', '392', 'Yen',                   NULL,  0, 'Iso4217'),
    ('CAD', 'Fiat', '124', 'Canadian Dollar',       NULL,  2, 'Iso4217'),
    ('CHF', 'Fiat', '756', 'Swiss Franc',           NULL,  2, 'Iso4217'),
    ('HKD', 'Fiat', '344', 'Hong Kong Dollar',      NULL,  2, 'Iso4217'),
    ('SGD', 'Fiat', '702', 'Singapore Dollar',      NULL,  2, 'Iso4217'),
    ('CNY', 'Fiat', '156', 'Yuan Renminbi',         NULL,  2, 'Iso4217'),
    -- Common digital tokens, keyed by their ticker (the code users record on
    -- trades). The ISO 24165 import additionally catalogues tokens under their
    -- formal Digital Token Identifier; minor_units are the token's decimals.
    ('BTC', 'DigitalToken', NULL, 'Bitcoin',  'BTC',  8, 'Iso24165'),
    ('ETH', 'DigitalToken', NULL, 'Ether',    'ETH', 18, 'Iso24165');
