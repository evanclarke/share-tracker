-- The price-change alert send log (REQUIREMENTS "Emailed portfolio reports —
-- weekly summary and price-change alerts", 2026-08-30).
--
-- The `price-alert` job runs once per market close — three times a day for a
-- portfolio spanning the ASX, the NYSE and a 24h crypto market — and each run
-- walks *every* held listing, because it cannot know which market the close it
-- was scheduled for belongs to (a listing's market is a property of the
-- listing, not of the cron line). Without this table an ASX holding that fell
-- 8% at Thursday's close would be emailed again at the New York close, again at
-- the crypto cut-off, and again every run until a newer close displaced it. The
-- table is what makes a move alert exactly once: a row is written for each
-- alerted move, keyed (listing_id, price_date), and a move already recorded is
-- never re-sent.
--
-- So the send log *is* the deduplication rather than a record kept beside it.
-- That is deliberate: a separate "already alerted" marker could drift from what
-- was actually sent, and this way the question "did it send, and what did it
-- say" has one answer. Every column is read by the job — the key by the
-- suppression check, the rest as the alert's own evidence of what was compared
-- (the operator's first question about a surprising alert is which two prices
-- produced it, and the threshold in force when it fired, since that is
-- configuration and can change).
--
-- **Prices are in the listing's quote currency**, never AUD-converted — the
-- same convention as `closing_prices.price`, which is where both figures come
-- from. The alert is about the security's own move, and converting would fold
-- an FX movement into a price change.
--
-- **The comparison is basis-safe.** `closing_prices.price` is stored in the
-- unit basis in force on its own `price_date`, so a pair straddling a share
-- split, consolidation or demerger restatement is quoted in two different
-- units: a 1-for-2 consolidation would read as a 50% crash. The job skips such
-- a pair rather than alerting on it (and says so in the run's note), so no row
-- here ever spans a price-basis event.
--
-- No HTTP route and no UI screen, by decision — the same call `cpi_quarters`
-- and `job_schedule` made. This is the alert job's own operational state: it is
-- written by one job, read by that job alone, and carries no financial fact any
-- report computes with. The emails themselves are the surface it exists for.
-- `id` is a plain INTEGER PRIMARY KEY rather than AUTOINCREMENT for the same
-- reason: migration 0045 gave the audited tables theirs so a deleted row's id
-- could not be handed to a new row and let it inherit a history, and this table
-- has no history to inherit (see below).
--
-- Audit trail: **not audited**. `row_history` records the taxpayer's own
-- financial facts so an edit or deletion is recoverable; nothing here is
-- entered, nothing is edited, and a row's whole content is derivable again from
-- the two `closing_prices` rows it names. There is no UPDATE or DELETE path at
-- all — the job only ever inserts.
--
-- Snapshot staleness: **exempt**. The three snapshotted reports are the
-- price-dependent ones (portfolio overview, unrealised gains, performance) and
-- none of them reads an alert; writing one changes no figure any stored
-- snapshot holds. Recorded with that reason in `reports::snapshot`'s
-- STALENESS_EXEMPT_TABLES.

CREATE TABLE price_alerts (
    id             INTEGER PRIMARY KEY,
    listing_id     INTEGER NOT NULL REFERENCES listings(id),
    -- The close that moved, and the stored close it moved from. Together with
    -- listing_id these identify the comparison exactly.
    price_date     TEXT    NOT NULL,
    previous_date  TEXT    NOT NULL,
    -- Both prices in the listing's quote currency, as stored.
    price          TEXT    NOT NULL,
    previous_price TEXT    NOT NULL,
    -- The move, signed: negative for a fall. Percent of the previous close.
    change_pct     TEXT    NOT NULL,
    -- The threshold in force when this alert fired. Configuration, and it can
    -- change — without it, an old alert cannot be read back against the rule
    -- that produced it.
    threshold_pct  TEXT    NOT NULL,
    -- RFC 3339 UTC instant the alert email was accepted by the relay. Written
    -- only after the send succeeds: a failed send leaves no row, so the next
    -- run retries the move rather than silently swallowing it.
    sent_at        TEXT    NOT NULL,
    -- One alert per listing per close. This is the suppression.
    UNIQUE(listing_id, price_date)
);

-- The suppression check asks "which of these listings have I already alerted
-- for this close?", so the UNIQUE index above serves it directly; this one
-- serves the other direction — the most recent alerts, for a listing — which is
-- how a run reports what it suppressed.
CREATE INDEX price_alerts_listing_sent ON price_alerts(listing_id, sent_at);
