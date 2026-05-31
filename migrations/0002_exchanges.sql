CREATE TABLE exchanges (
    mic             TEXT    PRIMARY KEY,
    name            TEXT    NOT NULL,
    country         TEXT    NOT NULL,
    currency        TEXT    NOT NULL,
    timezone        TEXT    NOT NULL,
    settlement_days INTEGER NOT NULL
);

INSERT OR IGNORE INTO exchanges (mic, name, country, currency, timezone, settlement_days) VALUES
    ('XASX', 'Australian Securities Exchange', 'Australia',     'AUD', 'Australia/Sydney',  2),
    ('XNYS', 'New York Stock Exchange',        'United States', 'USD', 'America/New_York',  2);
