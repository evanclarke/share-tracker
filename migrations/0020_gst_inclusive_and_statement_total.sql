-- GST-inclusive brokerage entry and statement-total cross-check.
--
-- brokerage_includes_gst records that the brokerage amount was *entered*
-- GST-inclusive; the server splits it at write time (gst_on_brokerage =
-- amount / 11 rounded to the cent, brokerage = remainder), so the stored
-- columns keep their existing ex-GST semantics and every report's
-- cost-base arithmetic (brokerage + gst_on_brokerage) is unchanged. The
-- flag persists only so a trade round-trips back into the entry form.
--
-- statement_total is the broker statement's net transaction total in the
-- brokerage currency, validated at write time against
-- quantity x price +/- (brokerage + GST). Informational/validation-only:
-- no report or calculation uses it.
--
-- Plain ADD COLUMNs (constant defaults, no FK) - no rebuild, no data
-- dropped; existing rows get flag 0 / total NULL.
ALTER TABLE trades ADD COLUMN brokerage_includes_gst INTEGER NOT NULL DEFAULT 0
    CHECK (brokerage_includes_gst IN (0, 1));
ALTER TABLE trades ADD COLUMN statement_total TEXT;
