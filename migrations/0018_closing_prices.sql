-- Daily closing-price history: one stored closing (or reference) price per
-- listing per trading day, collected by the scheduled `price-import` job and
-- on-demand backfill. A failed fetch is stored as an errored row — never
-- silently missing — and is replaced by a successful re-run.

-- Local-time end of the exchange's regular trading session (HH:MM, in the
-- exchange's `timezone`). A trading day's closing price is only collected
-- once this time has passed; both seeded exchanges (XASX, XNYS) close 16:00.
ALTER TABLE exchanges ADD COLUMN close_time TEXT NOT NULL DEFAULT '16:00';

CREATE TABLE closing_prices (
    listing_id INTEGER NOT NULL REFERENCES listings(id),
    -- The trading day the price closes: the date in the exchange's timezone,
    -- or for exchange-less (Crypto) listings the UTC date of the daily candle
    -- that completes at 00:00 UTC at the end of that date.
    price_date TEXT    NOT NULL,
    -- Decimal as TEXT, in the listing's quote currency (NOT AUD-converted —
    -- reports convert via the FX rules). NULL exactly when status = 'error'.
    price      TEXT,
    source     TEXT    NOT NULL,  -- provider that produced the row, e.g. 'yahoo'
    fetched_at TEXT    NOT NULL,  -- RFC 3339 UTC timestamp of the fetch
    status     TEXT    NOT NULL CHECK (status IN ('ok', 'error')),
    error      TEXT,              -- failure detail, NULL exactly when status = 'ok'
    PRIMARY KEY (listing_id, price_date),
    CHECK ((price IS NOT NULL) = (status = 'ok')),
    CHECK ((error IS NOT NULL) = (status = 'error'))
);
