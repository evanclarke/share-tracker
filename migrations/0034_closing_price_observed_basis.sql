-- A stored closing price is recorded in its own trading day's unit basis, and
-- the provider's figure it came from is kept alongside it.
--
-- The provider (Yahoo) restates a security's whole close series into the
-- *current* unit basis the moment it splits: after NVDA's 10-for-1 on
-- 2024-06-10 the chart API answers 120.888 for 2024-06-07, a day NVDA actually
-- closed at 1208.88. `auto_adjust(false)` turns off *dividend* adjustment only.
-- The reports go the other way — `domain::open_parcels` re-bases each parcel's
-- quantity into the snapshot date's own basis — so a price backfilled after a
-- split was multiplied by units in the historical basis and the product came
-- out by the split ratio (SCENARIOS Q-14: a tenfold step in the valuation
-- series at the split date, and an 89.5% "unrealised loss" on a holding that
-- was up). No tax figure reads a closing price, so nothing but valuation was
-- affected.
--
-- The invariant chosen (2026-08-20): **a stored `price` is the price the
-- security traded at on its own `price_date`, in the unit basis in force that
-- day** — which is what a row fetched before the split already held, and what
-- the Closing Prices screen implies.
--
-- Restating a figure needs to know the basis it arrived in, and that is fixed
-- by *when* it was observed, not by when it is read: a figure observed at time
-- T is in the basis in force at T. So the row keeps the figure as observed and
-- derives the stored one:
--
--     price = price_as_observed × (the split ratio over (price_date, fetched_at])
--
-- Keeping the observation means every restatement is a recompute from source
-- rather than a delta applied to an already-adjusted number: recording,
-- editing or deleting a ShareSplit/BonusIssue re-derives the same answer, in
-- any order, with nothing to un-apply and no division to lose digits to. That
-- is what makes `entities::closing_price::db_rebase_listing_prices` idempotent
-- and lets the daily job's contemporaneously-fetched history survive a split
-- being recorded years later untouched (its fetch predates the split, so its
-- ratio is 1 — the case a "multiply every earlier price by the ratio" rule
-- would have corrupted wholesale).
--
-- For a hand-entered price the two columns are equal: the operator states the
-- figure for that day, so it is contemporaneous by declaration and is never
-- normalised or re-based (`docs/API.md`, Closing prices).
--
-- Existing rows: every stored figure to date *is* the raw observation (no
-- normalisation has ever been applied), so the copy below carries `price` into
-- `price_as_observed` unchanged and leaves `price` alone. That is already the
-- right answer for any database with no ShareSplit/BonusIssue recorded —
-- including the live one, which has none (checked 2026-08-20: 12,249 stored
-- prices, zero re-basing actions). A database that *does* have one gets its
-- one-off repair by running the `price-rebase` maintenance job
-- (`POST /jobs/price-rebase`), which re-derives every listing's prices by the
-- rule above. The repair is deliberately not attempted here: the ratio is a
-- product of TEXT decimals, and SQLite cannot multiply and divide those
-- exactly without going through REAL, which this project bans outright.
--
-- The table is rebuilt (rather than ALTER TABLE ADD COLUMN) so the new column
-- can carry its nullability CHECK — pairing it with `status` exactly as
-- `price` is — which is what makes the re-basing pass total: no ok row can
-- exist without the observation it must be re-derived from. Same rename
-- pattern as 0020 and 0021; ids are carried over explicitly so every row keeps
-- the audit trail already recorded against it.

-- ---------------------------------------------------------------------------
-- 1. Rebuild closing_prices with the observed-figure column.
-- ---------------------------------------------------------------------------

-- Dropped before the rename so SQLite's ALTER TABLE trigger-body rewrite has
-- nothing to rewrite; all three are re-created against the new table below.
-- No other trigger in the schema names closing_prices.
DROP TRIGGER closing_prices_stale_snapshots_update;
DROP TRIGGER closing_prices_row_history_update;
DROP TRIGGER closing_prices_row_history_delete;

ALTER TABLE closing_prices RENAME TO closing_prices_old;

CREATE TABLE closing_prices (
    -- Server-assigned surrogate key: the row's identity for the audit trail
    -- (row_history.row_id). Never reused — see 0021.
    id           INTEGER PRIMARY KEY AUTOINCREMENT,
    listing_id   INTEGER NOT NULL REFERENCES listings(id),
    -- The trading day the price closes: the date in the exchange's timezone,
    -- or for exchange-less (Crypto) listings the UTC date of the daily candle
    -- that completes at 00:00 UTC at the end of that date.
    price_date   TEXT    NOT NULL,
    -- Decimal as TEXT, in the listing's quote currency (NOT AUD-converted —
    -- reports convert via the FX rules), and in the unit basis in force on
    -- price_date. NULL exactly when status = 'error'.
    price        TEXT,
    -- Decimal as TEXT: the figure exactly as the provider served it (or as the
    -- operator entered it), in the unit basis in force when it was observed —
    -- which fetched_at dates. `price` is derived from it by the ratio of the
    -- ShareSplit/BonusIssue actions dated in (price_date, fetched_at]; for a
    -- hand-entered row the two are equal. NULL exactly when status = 'error'.
    price_as_observed TEXT,
    -- Provider that produced the row, e.g. 'yahoo'; 'manual' exactly when the
    -- price was entered by hand (CHECK-paired with origin below).
    source       TEXT    NOT NULL,
    -- RFC 3339 UTC timestamp of the fetch that produced the row — for a manual
    -- row, of the entry that recorded it. Also dates the unit basis
    -- price_as_observed arrived in (see the header note).
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
    -- The former primary key: still one price per (listing, day), and still
    -- the conflict target of closing_price::db_store's upsert.
    UNIQUE (listing_id, price_date),
    CHECK ((price IS NOT NULL) = (status = 'ok')),
    -- The observation is present exactly when the price is, so a re-base can
    -- never find an ok row it has nothing to re-derive from.
    CHECK ((price_as_observed IS NOT NULL) = (status = 'ok')),
    CHECK ((error IS NOT NULL) = (status = 'error')),
    CHECK ((sourced_from IS NOT NULL) = (origin = 'manual')),
    CHECK ((reason IS NOT NULL) = (origin = 'manual')),
    -- A hand-entered row always carries a price: there is no such thing as a
    -- manual fetch failure.
    CHECK (origin = 'fetched' OR status = 'ok'),
    -- The provider slot agrees with the origin, so neither can drift.
    CHECK ((source = 'manual') = (origin = 'manual'))
);

-- Ids are carried over, not reassigned: each row keeps the row_history trail
-- already recorded against it. Every stored figure to date is the raw
-- observation, so price_as_observed is a straight copy of price (NULL for an
-- errored row, which the CHECK above requires).
INSERT INTO closing_prices
    (id, listing_id, price_date, price, price_as_observed, source, fetched_at,
     status, error, origin, sourced_from, reason)
    SELECT id, listing_id, price_date, price, price, source, fetched_at,
           status, error, origin, sourced_from, reason
    FROM closing_prices_old
    ORDER BY id;

DROP TABLE closing_prices_old;

-- Unchanged from 0021: revising a stored ok price stales the snapshots that
-- were valued at it — which now also covers a re-base, so recording a split
-- regenerates the valuations its price correction moved. Still no INSERT or
-- DELETE counterpart (see 0021).
CREATE TRIGGER closing_prices_stale_snapshots_update AFTER UPDATE ON closing_prices
WHEN OLD.status = 'ok' AND (NEW.status <> 'ok' OR OLD.price <> NEW.price)
BEGIN
    UPDATE report_snapshots SET stale = 1 WHERE snapshot_date >= OLD.price_date;
END;

-- ---------------------------------------------------------------------------
-- 2. Re-create the audit triggers with the new column list.
--
-- A re-base is an UPDATE of an audited table, so the superseded figure lands
-- in the trail like any other price revision — and the observation it was
-- derived from travels with it.
-- ---------------------------------------------------------------------------

CREATE TRIGGER closing_prices_row_history_update AFTER UPDATE ON closing_prices
BEGIN
    INSERT INTO row_history (table_name, row_id, operation, changed_at, old_row)
    VALUES ('closing_prices', OLD.id, 'UPDATE', strftime('%Y-%m-%dT%H:%M:%fZ', 'now'),
            json_object(
                        'id', OLD.id, 'listing_id', OLD.listing_id,
                        'price_date', OLD.price_date, 'price', OLD.price,
                        'price_as_observed', OLD.price_as_observed,
                        'source', OLD.source, 'fetched_at', OLD.fetched_at,
                        'status', OLD.status, 'error', OLD.error,
                        'origin', OLD.origin, 'sourced_from', OLD.sourced_from,
                        'reason', OLD.reason));
END;

CREATE TRIGGER closing_prices_row_history_delete AFTER DELETE ON closing_prices
BEGIN
    INSERT INTO row_history (table_name, row_id, operation, changed_at, old_row)
    VALUES ('closing_prices', OLD.id, 'DELETE', strftime('%Y-%m-%dT%H:%M:%fZ', 'now'),
            json_object(
                        'id', OLD.id, 'listing_id', OLD.listing_id,
                        'price_date', OLD.price_date, 'price', OLD.price,
                        'price_as_observed', OLD.price_as_observed,
                        'source', OLD.source, 'fetched_at', OLD.fetched_at,
                        'status', OLD.status, 'error', OLD.error,
                        'origin', OLD.origin, 'sourced_from', OLD.sourced_from,
                        'reason', OLD.reason));
END;
