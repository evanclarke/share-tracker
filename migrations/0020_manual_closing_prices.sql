-- Manually entered closing prices.
--
-- Every row in this table was produced by a PriceFetcher until now: a day the
-- provider cannot serve (a delisted or mis-served symbol, a permanent hole in
-- the series) is stored errored, and reports::valuation blocks that date
-- outright, so no snapshot exists for it and no re-fetch will ever fix it.
-- A price entered by hand fills such a day, and is read by valuation exactly
-- like a fetched one.
--
-- A hand-entered figure is only auditable with its provenance, so the two
-- questions it must answer are stored as their own columns rather than one
-- free-text blob: sourced_from (where the figure came from) and reason (why
-- manual entry was needed). origin is the typed marker the code branches on
-- — the fetch endpoint refuses a manual row, and the UI badges it — with the
-- source column (the provider slot) held to 'manual' in step with it, so the
-- existing Source column of the prices screen stays truthful.
--
-- The pairings below are table-level CHECKs, which SQLite cannot ALTER in, so
-- the table is rebuilt via the rename pattern (0014/0017 precedent): rename
-- out, create the new shape, copy every row, drop the _old table. Existing
-- rows are all provider-fetched, so they stamp origin = 'fetched' with both
-- text columns NULL.
--
-- No new snapshot-staleness triggers, and none are needed:
--   * replacing a stored ok price with a manual one is an UPDATE that changes
--     price, which the existing closing_prices_stale_snapshots_update trigger
--     (re-created below) already catches;
--   * filling a missing or errored date is an INSERT or an error -> ok UPDATE,
--     and such a date was blocked for valuation, so there is no snapshot on it
--     to stale — the reasoning 0001_schema.sql already records for this table;
--   * a manual row is status = 'ok', and closing_price::delete_one rejects ok
--     rows, so a valued manual price can never be deleted out from under a
--     snapshot. That is also the whole escape-hatch story: a manual price is
--     corrected by entering another one, never by handing the day back to the
--     provider.
--
-- closing_prices is deliberately outside the row_history audit trail (0013:
-- import-managed, re-importable reference data), so there are no audit
-- triggers to re-create — only the staleness trigger.

DROP TRIGGER closing_prices_stale_snapshots_update;

ALTER TABLE closing_prices RENAME TO closing_prices_old;

CREATE TABLE closing_prices (
    listing_id   INTEGER NOT NULL REFERENCES listings(id),
    -- The trading day the price closes: the date in the exchange's timezone,
    -- or for exchange-less (Crypto) listings the UTC date of the daily candle
    -- that completes at 00:00 UTC at the end of that date.
    price_date   TEXT    NOT NULL,
    -- Decimal as TEXT, in the listing's quote currency (NOT AUD-converted —
    -- reports convert via the FX rules). NULL exactly when status = 'error'.
    price        TEXT,
    -- Provider that produced the row, e.g. 'yahoo'; 'manual' exactly when the
    -- price was entered by hand (CHECK-paired with origin below).
    source       TEXT    NOT NULL,
    -- RFC 3339 UTC timestamp of the fetch that produced the row — for a manual
    -- row, of the entry that recorded it.
    fetched_at   TEXT    NOT NULL,
    status       TEXT    NOT NULL CHECK (status IN ('ok', 'error')),
    error        TEXT,              -- failure detail, NULL exactly when status = 'ok'
    -- How the row came to be: fetched from the provider, or entered by hand.
    origin       TEXT    NOT NULL DEFAULT 'fetched' CHECK (origin IN ('fetched', 'manual')),
    -- Where a manual price was sourced from (e.g. 'asx.com.au closing report').
    -- NULL exactly when origin = 'fetched'.
    sourced_from TEXT,
    -- Why manual entry was needed (e.g. 'provider serves no candle since the
    -- delisting'). NULL exactly when origin = 'fetched'.
    reason       TEXT,
    PRIMARY KEY (listing_id, price_date),
    CHECK ((price IS NOT NULL) = (status = 'ok')),
    CHECK ((error IS NOT NULL) = (status = 'error')),
    CHECK ((sourced_from IS NOT NULL) = (origin = 'manual')),
    CHECK ((reason IS NOT NULL) = (origin = 'manual')),
    -- A hand-entered row always carries a price: there is no such thing as a
    -- manual fetch failure.
    CHECK (origin = 'fetched' OR status = 'ok'),
    -- The provider slot agrees with the origin, so neither can drift.
    CHECK ((source = 'manual') = (origin = 'manual'))
);

INSERT INTO closing_prices
    (listing_id, price_date, price, source, fetched_at, status, error, origin, sourced_from, reason)
    SELECT listing_id, price_date, price, source, fetched_at, status, error, 'fetched', NULL, NULL
    FROM closing_prices_old
    ORDER BY listing_id, price_date;

DROP TABLE closing_prices_old;

CREATE TRIGGER closing_prices_stale_snapshots_update AFTER UPDATE ON closing_prices
WHEN OLD.status = 'ok' AND (NEW.status <> 'ok' OR OLD.price <> NEW.price)
BEGIN
    UPDATE report_snapshots SET stale = 1 WHERE snapshot_date >= OLD.price_date;
END;
