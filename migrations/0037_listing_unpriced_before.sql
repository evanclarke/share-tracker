-- The mirror image of 0035's `unpriced_from`: a security whose provider
-- series *begins* at a date, with everything earlier unavailable at any
-- price.
--
-- 0035 recorded the date a provider **stopped** serving a security and had
-- valuation carry the last observed close forward. The opposite shape is just
-- as real and Evan has it: New Lithium Americas' Yahoo series starts
-- 2023-10-02 and a backfill of any earlier range answers HTTP 400 on every
-- day. The 0035 pass met this case and correctly refused to mis-handle it —
-- an unpriced hole *before* the series begins is one a carry-forward cannot
-- reach, because there is no earlier close to carry.
--
-- What happened in the column's absence is the argument for it. Offered a
-- choice between a permanently stale run of snapshots and a knowingly wrong
-- number, the operator entered 375 closing prices by hand whose own `reason`
-- text says what they are: "Yahoo serves no LAC candle before 2023-10-02 …
-- leaving 2021-03-25..2022-09-19 unpriceable and 544 snapshots permanently
-- stale. Copied to unblock them. NOTE: demerger-adjusted, so about 2.46x
-- below the actual old-LAC close of the day — this period is unblocked, not
-- accurate."
--
-- `unpriced_before` dates the condition on the listing itself: **no price is
-- obtainable for this security before this date**. NULL — every existing row
-- — keeps today's behaviour exactly, so nothing is migrated. Before the date:
--
--   * price collection never fetches the listing (and an explicit fetch is
--     refused, a backfill clamped up to the date),
--   * `GET /reports/health` stops reporting its errored rows and unpriced
--     days there — they are expected, not a to-do,
--   * valuation **excludes the holding** from the date's portfolio total and
--     says so, rather than blocking the whole date.
--
-- The two directions are deliberately **not** symmetric, which is the whole
-- decision (Evan, 2026-08-20). Carrying a close forward substitutes a real,
-- once-observed price. Nothing before the provider's series begins was ever
-- observed, so no figure is invented: the holding leaves the total and the
-- total says which holding left it. Two consequences are accepted, both
-- visible rather than silent — the Portfolio Overview graph **steps** when
-- the listing's own series begins, and a portfolio total for the excluded
-- span omits a real holding.
--
-- The marker is a listing-level **declaration**, made later in time than any
-- stored row, and it therefore supersedes the per-day rows for that span:
-- valuation excludes the holding even where a stored ok price exists for the
-- day, whatever its origin. That is exactly the live case — LAC's 635
-- pre-2023-10-02 rows are byte-identical to another listing's series, 375 of
-- them hand-entered and 260 fetched under a one-off `symbol` override — and
-- it is what lets those rows be retired instead of quietly continuing to
-- price the span. To value a day inside the span from a source you do have,
-- move `unpriced_before` back to the earliest day you can price, or clear it.
--
-- The one pairing SQLite cannot express here — a table-level CHECK cannot be
-- ALTERed in — is enforced in `entities::listing::db_upsert`, beside
-- `amit_from`'s and `unpriced_from`'s:
--
--   * `unpriced_before` must be strictly before `unpriced_from` when both are
--     set. Both may be: a security the provider quotes only between the two
--     dates (a spun-off entity later delisted) is an ordinary shape. Equal or
--     crossed dates would leave no day the provider serves at all, which is
--     not a listing anyone can hold a valuation of.
--
-- There is deliberately **no** refusal based on prices already stored before
-- the date, in either direction:
--
--   * requiring one would invert 0035's rule for no reason — nothing is
--     carried, so nothing needs to exist;
--   * refusing one (the literal mirror of 0035's "no *fetched* ok price on or
--     after it") would make the feature unreachable for the only case it
--     exists for. Checked read-only against the deployed database first: LAC
--     carries 635 ok rows before 2023-10-02, 260 of them `origin: fetched`.
--     A fetched row is not proof the provider serves the listing's own
--     symbol — `POST /closing_prices/backfill` takes a one-off `symbol`
--     override, which is how those 260 arrived — so it cannot contradict the
--     column the way a post-`unpriced_from` fetch contradicts 0035's.
ALTER TABLE listings ADD COLUMN unpriced_before TEXT;

-- Whether this stored snapshot's totals **omit a held holding** because no
-- price is obtainable for it at that date (`listings.unpriced_before`).
--
-- A third flag rather than a reuse of either existing one, for the reason
-- 0035 gave for keeping `price_carried_forward` out of `provisional`, plus
-- one of its own:
--
--   * `provisional` means an interim FX rate a later import trues up, and the
--     true-up runs (`POST /report_snapshots/regenerate_provisional`, the
--     post-import regeneration) select on it. An excluded holding never
--     clears, so folding it in would turn a bounded true-up into one that
--     regenerates the same dates forever.
--   * `price_carried_forward` is a weaker statement than this one: it says
--     the figure rests on an interim input, while this says the figure is
--     **missing a holding**. They must be readable apart.
ALTER TABLE report_snapshots
    ADD COLUMN holding_excluded INTEGER NOT NULL DEFAULT 0
        CHECK (holding_excluded IN (0, 1));

-- Which holdings this snapshot's totals omit, and why: a JSON array of
-- `{"listing_id", "ticker", "reason"}` exactly as the generation run resolved
-- it. The boolean above says the total is incomplete; a reader needs to know
-- *which* holding is absent to judge the figure, and the answer has to travel
-- with the stored result (the listing's marker may have moved since). '[]'
-- on every existing row, which is what "nothing was excluded" has always
-- meant.
ALTER TABLE report_snapshots
    ADD COLUMN excluded_holdings TEXT NOT NULL DEFAULT '[]';

-- listings is audited (CLAUDE.md rule): ALTER TABLE ADD COLUMN does not
-- update existing triggers, so its two row_history triggers are re-created
-- here with unpriced_before added to the JSON column list.
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
                        'unpriced_from', OLD.unpriced_from,
                        'unpriced_before', OLD.unpriced_before));
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
                        'unpriced_from', OLD.unpriced_from,
                        'unpriced_before', OLD.unpriced_before));
END;

-- Snapshot staleness: `unpriced_before` joins `unpriced_from` as a dated
-- listings column that changes what a *stored* snapshot figure means, so
-- 0035's trigger is re-created with it — a third body rather than a parallel
-- mechanism, since `reports::snapshot`'s
-- `every_table_is_classified_for_snapshot_staleness` pins the exact trigger
-- set `listings` carries.
--
-- It stales a *prefix* where `unpriced_from` stales a suffix: setting it
-- makes every snapshot before that date exclude the holding, clearing it
-- makes every snapshot before that date valuable again, and moving it
-- affects everything before the later of the two dates. Clearing it is the
-- case that must not be missed — the stored figures omit a real holding, and
-- only a regeneration puts it back.
DROP TRIGGER listings_stale_snapshots_update;

CREATE TRIGGER listings_stale_snapshots_update AFTER UPDATE ON listings
WHEN OLD.currency <> NEW.currency
  OR OLD.security_type <> NEW.security_type
  OR OLD.unpriced_from IS NOT NEW.unpriced_from
  OR OLD.unpriced_before IS NOT NEW.unpriced_before
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

    -- unpriced_before: the mirror — only the snapshots dated *before* the
    -- later of the old and new dates.
    UPDATE report_snapshots SET stale = 1
    WHERE OLD.unpriced_before IS NOT NEW.unpriced_before
      AND snapshot_date < MAX(IFNULL(OLD.unpriced_before, NEW.unpriced_before),
                              IFNULL(NEW.unpriced_before, OLD.unpriced_before));
END;
