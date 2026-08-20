-- SCENARIOS Q-02: a still-held listing the provider stopped quoting — a
-- delisting, or a suspension that can run for years — blocked the *whole*
-- portfolio's snapshots indefinitely.
--
-- Every trading day after its last quote stored an errored row, and
-- `reports::valuation::stored_valuations` fails the whole date if any held
-- listing is unpriced (deliberately: no partial result). So one suspended
-- holding stopped the daily `report-snapshot` job for every listing, every
-- day, while `GET /reports/health` nagged with a growing `errored_days`
-- count. The documented way out — a hand-entered price — is one entry per
-- listing per trading day, forever.
--
-- `unpriced_from` dates the condition on the listing itself: the *provider*
-- serves nothing for this security from this date on. NULL — every existing
-- row — keeps today's behaviour exactly, so nothing is migrated. From the
-- date on:
--
--   * price collection stops fetching the listing (and an explicit re-fetch
--     or backfill of such a date is refused, so no fetched row can appear
--     after it),
--   * `GET /reports/health` stops reporting its errored rows and unpriced
--     days — they are expected, not a to-do,
--   * valuation stops blocking the whole date on it: the last stored ok
--     close is carried forward and the snapshot is flagged
--     `price_carried_forward`, the way a fallback-month FX rate is flagged
--     `provisional`, so the substitution is never silent.
--
-- The two pairings SQLite cannot express here — a table-level CHECK cannot be
-- ALTERed in, and a column CHECK cannot reference another table — are
-- enforced in `entities::listing::db_upsert`, the same place `amit_from`'s
-- are (0024):
--
--   * the date must have a stored **ok** price before it, since that is what
--     valuation carries forward; without one the holding is unvaluable and
--     the marker would only hide the alarm saying so,
--   * no **fetched ok** price may be stored on or after it, since that is the
--     provider serving exactly what the column says it does not. (A *manual*
--     price on or after it is fine and is preferred over the carried-forward
--     figure — an administrator's valuation during a suspension.)
ALTER TABLE listings ADD COLUMN unpriced_from TEXT;

-- Which report snapshots were valued with a carried-forward close.
-- Deliberately **not** `provisional`: that flag means an interim FX rate that
-- a later RBA import trues up, and the true-up runs
-- (`POST /report_snapshots/regenerate_provisional`, the post-import
-- regeneration) target provisional dates. A carried-forward price never
-- clears — the provider is never going to quote the day — so folding it into
-- `provisional` would turn a bounded true-up into one that regenerates the
-- same dates forever and never finishes clearing them.
ALTER TABLE report_snapshots
    ADD COLUMN price_carried_forward INTEGER NOT NULL DEFAULT 0
        CHECK (price_carried_forward IN (0, 1));

-- listings is audited (CLAUDE.md rule): ALTER TABLE ADD COLUMN does not
-- update existing triggers, so its two row_history triggers are re-created
-- here with unpriced_from added to the JSON column list.
DROP TRIGGER listings_row_history_update;
DROP TRIGGER listings_row_history_delete;

CREATE TRIGGER listings_row_history_update AFTER UPDATE ON listings
BEGIN
    INSERT INTO row_history (table_name, row_id, operation, changed_at, old_row)
    VALUES ('listings', OLD.id, 'UPDATE', strftime('%Y-%m-%dT%H:%M:%fZ', 'now'),
            json_object(
                        'id', OLD.id, 'exchange_mic', OLD.exchange_mic, 'ticker', OLD.ticker,
                        'name', OLD.name, 'isin', OLD.isin, 'security_type', OLD.security_type,
                        'currency', OLD.currency, 'amit', OLD.amit, 'preference', OLD.preference,
                        'price_symbol', OLD.price_symbol, 'amit_from', OLD.amit_from,
                        'unpriced_from', OLD.unpriced_from));
END;

CREATE TRIGGER listings_row_history_delete AFTER DELETE ON listings
BEGIN
    INSERT INTO row_history (table_name, row_id, operation, changed_at, old_row)
    VALUES ('listings', OLD.id, 'DELETE', strftime('%Y-%m-%dT%H:%M:%fZ', 'now'),
            json_object(
                        'id', OLD.id, 'exchange_mic', OLD.exchange_mic, 'ticker', OLD.ticker,
                        'name', OLD.name, 'isin', OLD.isin, 'security_type', OLD.security_type,
                        'currency', OLD.currency, 'amit', OLD.amit, 'preference', OLD.preference,
                        'price_symbol', OLD.price_symbol, 'amit_from', OLD.amit_from,
                        'unpriced_from', OLD.unpriced_from));
END;

-- Snapshot staleness: `unpriced_from` joins `currency`/`security_type` as a
-- listings column that changes what a *stored* snapshot figure means, so
-- 0030's narrowed trigger is re-created with it in the WHEN clause.
--
-- Unlike those two it carries a date of its own, so it stales a suffix rather
-- than the whole series: setting it makes every snapshot from that date on
-- carry-forward-valued, clearing it (the security relists, the suspension
-- lifts) makes every snapshot from that date on valuable at real prices
-- again, and moving it affects everything from the earlier of the two dates.
-- Clearing it is exactly the case that must not be missed: the stored figures
-- are a flat line at the last close, and only a regeneration replaces them
-- with the prices that have since been collected.
DROP TRIGGER listings_stale_snapshots_update;

CREATE TRIGGER listings_stale_snapshots_update AFTER UPDATE ON listings
WHEN OLD.currency <> NEW.currency
  OR OLD.security_type <> NEW.security_type
  OR OLD.unpriced_from IS NOT NEW.unpriced_from
BEGIN
    -- currency / security_type: no date of their own, so the whole series.
    UPDATE report_snapshots SET stale = 1
    WHERE OLD.currency <> NEW.currency OR OLD.security_type <> NEW.security_type;

    -- unpriced_from: only the snapshots dated on or after the earlier of the
    -- old and new dates (IFNULL both ways, so a set uses NEW's date and a
    -- clear uses OLD's).
    UPDATE report_snapshots SET stale = 1
    WHERE OLD.unpriced_from IS NOT NEW.unpriced_from
      AND snapshot_date >= MIN(IFNULL(OLD.unpriced_from, NEW.unpriced_from),
                               IFNULL(NEW.unpriced_from, OLD.unpriced_from));
END;
