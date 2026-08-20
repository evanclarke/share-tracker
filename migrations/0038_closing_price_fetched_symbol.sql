-- Record the provider symbol each fetched closing price was actually fetched
-- under, so a fetch made with a one-off `symbol` override is afterwards
-- distinguishable from an ordinary one.
--
-- The incident (found 2026-08-16 in the deployed database, TODO "LAC's whole
-- pre-demerger price history is LAR's series"): 260 rows of listing 7 (LAC)
-- were fetched through `POST /closing_prices/backfill`'s optional `symbol`
-- override — Yahoo serves no LAC candle before 2023-10-02, so the override was
-- the only way to reach those days — and the provider answered with a
-- *different* security's series. The stored rows record `source: 'yahoo'`,
-- `origin: 'fetched'` and nothing else: three weeks later there was no way to
-- tell them from prices fetched under the listing's own ticker. The 375
-- hand-entered rows covering the earlier half of the same span *were*
-- reconstructible, purely because `origin`/`sourced_from`/`reason` (0020) had
-- recorded what was done. This column is the fetched half of that same
-- provenance.
--
-- **Always recorded, not only when it differs.** The alternative — store the
-- symbol only when it is not the one the rename chain would have derived —
-- makes NULL mean two different things ("ordinary fetch" and "not recorded"),
-- which is exactly the ambiguity the incident consisted of. Recorded always,
-- the historical question has one answer: for a fetched ok row, a non-NULL
-- value *is* the symbol the provider was asked for, and NULL means the row
-- predates this column. The derived symbol is not itself stable over time
-- either (a rename recorded later re-derives it), so "differs from the
-- listing's own" is not even a fixed predicate to store against.
--
-- **Existing rows stay NULL, and that is deliberate.** The symbol a stored row
-- was fetched under is not recoverable from anything the database holds — for
-- the 260 LAC rows it is not even the listing's own ticker — so a migration
-- that filled the column in from the rename chain would be inventing the very
-- fact this column exists to record. NULL on a pre-existing row means
-- "unrecorded", never "the derived symbol".
--
-- Nullability. A manual row is never fetched under any symbol, so the CHECK
-- pairs the column with the origin the way 0020 paired `sourced_from`/`reason`
-- — one direction only: manual implies NULL. The converse cannot be a CHECK,
-- because a fetched row is legitimately NULL both for the pre-0038 rows above
-- and for the one live case where no symbol exists to record: a fetch whose
-- symbol could not be resolved at all (an exchange with no provider mapping),
-- which stores an errored row whose `error` says exactly that.
--
-- Informational: no calculation reads it. It is provenance — served by
-- `GET /closing_prices`, shown on the Closing Prices screen, and carried into
-- `row_history` by the triggers re-created below.
--
-- ADD COLUMN, not a rebuild (0020/0021/0034 all rebuilt this table): the CHECK
-- above is expressible as a column constraint, so there is no table-level
-- constraint to introduce and no reason to move 12,000-odd rows and their ids
-- again. Maintenance rule (0013): `closing_prices` is audited, so both of its
-- `*_row_history_*` triggers are dropped and re-created with the new column in
-- their `json_object` lists — a column the trail drops is a version of the row
-- that can never be recovered.
--
-- Snapshot staleness is untouched: the column feeds no valuation (no snapshot
-- figure can change because of it), and the single
-- `closing_prices_stale_snapshots_update` trigger (0001, re-created by 0034)
-- is neither dropped nor re-created here — ADD COLUMN leaves triggers alone.

ALTER TABLE closing_prices ADD COLUMN fetched_symbol TEXT
    CHECK (origin = 'fetched' OR fetched_symbol IS NULL);

DROP TRIGGER closing_prices_row_history_update;
DROP TRIGGER closing_prices_row_history_delete;

CREATE TRIGGER closing_prices_row_history_update AFTER UPDATE ON closing_prices
BEGIN
    INSERT INTO row_history (table_name, row_id, operation, changed_at, old_row)
    VALUES ('closing_prices', OLD.id, 'UPDATE', strftime('%Y-%m-%dT%H:%M:%fZ', 'now'),
            json_object(
                        'id', OLD.id, 'listing_id', OLD.listing_id,
                        'price_date', OLD.price_date, 'price', OLD.price,
                        'price_as_observed', OLD.price_as_observed,
                        'source', OLD.source, 'fetched_at', OLD.fetched_at,
                        'fetched_symbol', OLD.fetched_symbol,
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
                        'fetched_symbol', OLD.fetched_symbol,
                        'status', OLD.status, 'error', OLD.error,
                        'origin', OLD.origin, 'sourced_from', OLD.sourced_from,
                        'reason', OLD.reason));
END;
