-- The distribution calendar: what a holding *should* have paid, per listing
-- per ex-date (REQUIREMENTS "Distribution calendar and the missing-dividend
-- alert", 2026-08-27).
--
-- Nothing in the system knew when a holding should have paid a distribution. A
-- dividend or trust distribution never entered — or entered with a
-- fat-fingered amount — was invisible: it misstates the year's income and
-- franking credits, and the AMIT cash cross-check can only compare against
-- rows that exist. This table is the external half of that question, collected
-- from the price provider by the `distribution-import` job and read by
-- `reports::health`'s two advisory alerts (a known ex-date with units held and
-- no income row; a matched income row whose gross differs from per unit ×
-- units held).
--
-- **Advisory only, by decision.** No tax figure is computed from this table and
-- none may be: `reports::tax_report`'s `amma_missing` gate stays on recorded
-- facts alone, and the advisory `amma_nothing_recorded` list is never resolved
-- from it. Both exclusions are stated in REQUIREMENTS' "Deliberately out of
-- scope" with their reasons — the short version is that coupling a
-- completeness gate to an external feed would let a provider's coverage gap
-- silently retire a real question.
--
-- **The stored date is the ex-date, and it is candle-joined rather than taken
-- from the provider's action stream.** `yfinance-rs` collapses a corporate
-- action's timestamp to a **UTC** calendar date (`i64_to_date`), discarding the
-- exchange timezone the same response carries. Yahoo stamps the event at the
-- exchange's session start, so for an ASX security the action's own date is the
-- ex-date only in AEST; in AEDT (UTC+11, October–April) it is one day early,
-- where it then lands on a day the market was shut — 2025-01-01, 2024-01-01, a
-- Sunday. `distribution_event::yahoo` recovers the true date by joining the
-- event to the candle sharing its UTC date, both being stamped at session
-- start; verified 10 of 10 against issuer-published dates (REQUIREMENTS, "The
-- one-day ex-date shift"). So an `ex_date` here is a real trading day of the
-- listing's own market, which is what lets the alerts ask "what was held on the
-- last cum-dividend day" and get a meaningful answer.
--
-- **`amount_per_unit` is in the listing's quote currency**, never
-- AUD-converted — the same convention as `closing_prices.price`, and for the
-- same reason: the figure the alert compares against is the income row's own
-- gross, which is recorded in the distribution's currency too. `currency` is
-- the provider's own answer, cross-checked against the listing's before a row
-- is stored, so a mismatch is a rejected fetch rather than a silently
-- mis-scaled expectation.
--
-- Positivity is enforced at write time (`db_store` rejects a non-positive
-- amount) rather than by a CHECK: the column is a TEXT decimal, and the only
-- way a CHECK could compare it numerically is a `CAST(... AS REAL)`, which is
-- exactly the float round-trip the money rules forbid anywhere near a stored
-- figure.
--
-- The natural key is (listing_id, ex_date) — one distribution per listing per
-- ex-date — and it is a UNIQUE constraint rather than the primary key because
-- the trail needs an integer row identity (see below). The surrogate id is
-- AUTOINCREMENT for the reason migration 0045 gave 17 audited tables theirs: a
-- plain INTEGER PRIMARY KEY hands a deleted row's id straight to the next
-- insert, which would let a new row inherit the deleted one's history
-- (SCENARIOS U-a).
--
-- Snapshot staleness: **exempt**. The three snapshotted reports are the
-- price-dependent ones (portfolio overview, unrealised gains, performance) and
-- none of them reads a distribution event; the only reader is
-- `reports::health`, which is computed live on every request and never stored.
-- A write here can therefore invalidate no snapshot. Recorded in
-- `reports::snapshot::STALENESS_EXEMPT_TABLES`.

CREATE TABLE distribution_events (
    -- Surrogate key: the row's identity for the audit trail
    -- (row_history.row_id). AUTOINCREMENT so it is never reused — see the
    -- header note.
    id              INTEGER PRIMARY KEY AUTOINCREMENT,
    listing_id      INTEGER NOT NULL REFERENCES listings(id),
    -- The ex-dividend date in the exchange's own timezone: the first day the
    -- security traded without entitlement to this distribution, so the holders
    -- entitled to it are those holding at the close of the day before.
    ex_date         TEXT    NOT NULL,
    -- Decimal as TEXT, in the listing's quote currency (NOT AUD-converted).
    amount_per_unit TEXT    NOT NULL,
    currency        TEXT    NOT NULL REFERENCES currencies(code),
    -- Provider that produced the row, e.g. 'yahoo'.
    source          TEXT    NOT NULL,
    -- The provider symbol the row was fetched under, in the namespace of
    -- `source` — the same provenance `closing_prices.fetched_symbol` records,
    -- and for the same reason: "what symbol produced this row?" has one
    -- answer. Informational: no calculation reads it; it is served by
    -- GET /distribution_events, shown on the Distribution Calendar screen and
    -- carried into row_history.
    fetched_symbol  TEXT    NOT NULL,
    -- RFC 3339 UTC timestamp of the fetch that produced the row.
    fetched_at      TEXT    NOT NULL,
    UNIQUE (listing_id, ex_date)
);

-- ---------------------------------------------------------------------------
-- Extend row_history's table_name CHECK to accept 'distribution_events'.
--
-- A table-level CHECK SQLite cannot ALTER, so row_history is rebuilt via the
-- rename pattern exactly as 0018, 0021, 0027, 0031 and 0039 did (see 0018's
-- long note): legacy_alter_table suppresses SQLite's rewrite of every trigger
-- body that names row_history — every audited table's trigger pair would
-- otherwise be repointed at row_history_old and break the moment it is
-- dropped.
-- ---------------------------------------------------------------------------

PRAGMA legacy_alter_table = ON;
ALTER TABLE row_history RENAME TO row_history_old;
PRAGMA legacy_alter_table = OFF;

CREATE TABLE row_history (
    id         INTEGER PRIMARY KEY AUTOINCREMENT,
    table_name TEXT    NOT NULL CHECK (table_name IN (
                   'trades', 'parcel_allocations', 'income', 'interest_income',
                   'amma_statements', 'amit_adjustments', 'ess_statements',
                   'transfers', 'corporate_actions', 'inheritances',
                   'rights_sales', 'rights_sale_allocations',
                   'investment_expenses', 'drp_enrolments', 'cgt_settings',
                   'attachments', 'listings', 'listing_renames',
                   'closing_prices', 'tax_year_settings', 'rba_fx_rates',
                   'exchange_holidays', 'distribution_events')),
    row_id     INTEGER NOT NULL,
    operation  TEXT    NOT NULL CHECK (operation IN ('UPDATE', 'DELETE')),
    changed_at TEXT    NOT NULL,
    old_row    TEXT    NOT NULL
);

INSERT INTO row_history (id, table_name, row_id, operation, changed_at, old_row)
    SELECT id, table_name, row_id, operation, changed_at, old_row
    FROM row_history_old
    ORDER BY id;

-- Drops row_history_old's index and its own append-only guard triggers, both
-- of which moved with the rename; all three are re-created below.
DROP TABLE row_history_old;

CREATE INDEX row_history_row ON row_history (table_name, row_id);

CREATE TRIGGER row_history_append_only_update BEFORE UPDATE ON row_history
BEGIN
    SELECT RAISE(ABORT, 'row_history is append-only');
END;

CREATE TRIGGER row_history_append_only_delete BEFORE DELETE ON row_history
BEGIN
    SELECT RAISE(ABORT, 'row_history is append-only');
END;

-- ---------------------------------------------------------------------------
-- Audit distribution_events.
--
-- The table is provider-owned — the import job is the only writer, and there is
-- no PUT or DELETE route — so the UPDATE trigger is the one that earns its
-- keep: it records the figure a re-fetch *changed*. That matters here more than
-- for a price, because the amount is what the cross-check alert reconciles an
-- entered distribution against; a provider that silently revises a per-unit
-- figure would otherwise turn a previously-clean row into a mismatch with no
-- trace of which side moved. The DELETE trigger is there because every audited
-- table in the schema has one and a row removed by hand (or by a future
-- retention step) must still be recoverable.
--
-- Neither is WHEN-narrowed, matching every other audited table: the trail asks
-- "what did this row say before that write?", to which a no-change re-store
-- answers exactly that.
-- ---------------------------------------------------------------------------

CREATE TRIGGER distribution_events_row_history_update AFTER UPDATE ON distribution_events
BEGIN
    INSERT INTO row_history (table_name, row_id, operation, changed_at, old_row)
    VALUES ('distribution_events', OLD.id, 'UPDATE', strftime('%Y-%m-%dT%H:%M:%fZ', 'now'),
            json_object('id', OLD.id, 'listing_id', OLD.listing_id,
                        'ex_date', OLD.ex_date, 'amount_per_unit', OLD.amount_per_unit,
                        'currency', OLD.currency, 'source', OLD.source,
                        'fetched_symbol', OLD.fetched_symbol, 'fetched_at', OLD.fetched_at));
END;

CREATE TRIGGER distribution_events_row_history_delete AFTER DELETE ON distribution_events
BEGIN
    INSERT INTO row_history (table_name, row_id, operation, changed_at, old_row)
    VALUES ('distribution_events', OLD.id, 'DELETE', strftime('%Y-%m-%dT%H:%M:%fZ', 'now'),
            json_object('id', OLD.id, 'listing_id', OLD.listing_id,
                        'ex_date', OLD.ex_date, 'amount_per_unit', OLD.amount_per_unit,
                        'currency', OLD.currency, 'source', OLD.source,
                        'fetched_symbol', OLD.fetched_symbol, 'fetched_at', OLD.fetched_at));
END;
