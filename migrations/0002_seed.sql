-- Seed data: a baseline of recognised currencies and the operational exchanges.
--
-- Currencies are seeded first so the exchange currency foreign keys resolve. The
-- currency import (ISO 4217 / ISO 24165) and MIC/FX imports later refine and extend
-- these; the baseline only needs to be enough for the app to run out of the box.

INSERT INTO currencies (code, kind, numeric_code, name, short_name, minor_units, source) VALUES
    ('AUD', 'Fiat', '036', 'Australian Dollar',  NULL, 2, 'Iso4217'),
    ('USD', 'Fiat', '840', 'US Dollar',          NULL, 2, 'Iso4217'),
    ('GBP', 'Fiat', '826', 'Pound Sterling',      NULL, 2, 'Iso4217'),
    ('EUR', 'Fiat', '978', 'Euro',                NULL, 2, 'Iso4217'),
    ('JPY', 'Fiat', '392', 'Yen',                 NULL, 0, 'Iso4217'),
    ('NZD', 'Fiat', '554', 'New Zealand Dollar',  NULL, 2, 'Iso4217'),
    ('HKD', 'Fiat', '344', 'Hong Kong Dollar',    NULL, 2, 'Iso4217'),
    ('SGD', 'Fiat', '702', 'Singapore Dollar',    NULL, 2, 'Iso4217'),
    ('CAD', 'Fiat', '124', 'Canadian Dollar',     NULL, 2, 'Iso4217'),
    ('CHF', 'Fiat', '756', 'Swiss Franc',         NULL, 2, 'Iso4217'),
    ('CNY', 'Fiat', '156', 'Yuan Renminbi',       NULL, 2, 'Iso4217'),
    ('BTC', 'DigitalToken', NULL, 'Bitcoin', 'BTC',  8, 'Iso24165'),
    ('ETH', 'DigitalToken', NULL, 'Ether',   'ETH', 18, 'Iso24165');

INSERT INTO exchanges (mic, name, country, currency, timezone, settlement_days) VALUES
    ('XASX', 'Australian Securities Exchange', 'Australia',     'AUD', 'Australia/Sydney',  2),
    ('XNYS', 'New York Stock Exchange',        'United States', 'USD', 'America/New_York',  2);
