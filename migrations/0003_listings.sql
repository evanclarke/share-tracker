CREATE TABLE listings (
    id            INTEGER PRIMARY KEY,
    exchange_mic  TEXT    NOT NULL REFERENCES exchanges(mic),
    ticker        TEXT    NOT NULL,
    name          TEXT    NOT NULL,
    isin          TEXT,
    security_type TEXT    NOT NULL CHECK(security_type IN ('Share', 'ETF', 'LIC', 'Trust')),
    currency      TEXT    NOT NULL,
    amit          INTEGER NOT NULL DEFAULT 0,
    UNIQUE(exchange_mic, ticker)
);
