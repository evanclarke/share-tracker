-- Moving a listing to another exchange stales the snapshots its calendar
-- valued.
--
-- `reports::valuation` values a holding on a snapshot date at the stored close
-- of the nearest trading day on or before it, on the calendar of the exchange
-- in force *then* (`closing_price::Market`, which reads the listing's own
-- `exchange_mic` for the span since its newest rename and each
-- `listing_renames.old_exchange_mic` for the spans before). So a different
-- exchange can make a different day the valuation day: a holding snapshotted
-- on Australia Day was valued at the Friday close on the XASX calendar, and
-- once the listing is on an exchange that trades that Monday the same date
-- regenerates at the Monday close. `listings_stale_snapshots_update` (0030,
-- re-created by 0035, 0037 and 0045) watched only `currency`, `security_type`,
-- `unpriced_from` and `unpriced_before`, and `listing_renames` is
-- staleness-exempt as a display-only change, so recording such a move — a
-- rename that changes the exchange, or its undo — left every snapshot it
-- re-valued `stale = 0`.
--
-- Both paths write `listings.exchange_mic` (a plain `PUT /listings/:id`
-- refuses an exchange change once the listing has trades, income or prices,
-- so it can move only a listing no snapshot holds), so the column joins the
-- trigger's list. The dates whose calendar changes are those under the
-- listing's *current* identity: on or after its newest rename's
-- `effective_date`, or every date when it has none. The rename writes its row
-- before updating the listing and the undo updates the listing before
-- deleting its row, so in both the newest rename at trigger time is the one
-- being recorded or undone; had either order been the other way round, the
-- newest would be an earlier rename, which stales a superset — so the bound is
-- never too narrow.
--
-- No data changes; `listings` keeps its audit triggers (`listings_row_history_*`,
-- untouched here, since no column is added).

DROP TRIGGER listings_stale_snapshots_update;

CREATE TRIGGER listings_stale_snapshots_update AFTER UPDATE ON listings
WHEN OLD.currency <> NEW.currency
  OR OLD.security_type <> NEW.security_type
  OR OLD.unpriced_from IS NOT NEW.unpriced_from
  OR OLD.unpriced_before IS NOT NEW.unpriced_before
  OR OLD.exchange_mic IS NOT NEW.exchange_mic
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

    -- exchange_mic: the snapshots under the current identity — on or after
    -- the newest rename, or all of them when there is none.
    UPDATE report_snapshots SET stale = 1
    WHERE OLD.exchange_mic IS NOT NEW.exchange_mic
      AND snapshot_date >= IFNULL(
          (SELECT MAX(effective_date) FROM listing_renames WHERE listing_id = NEW.id),
          '0000-01-01');
END;
