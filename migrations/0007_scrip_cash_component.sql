-- Takeovers with a cash component — partial scrip-for-scrip rollover
-- (REQUIREMENTS 2026-06-10; docs/ato/takeovers-and-scrip-for-scrip.md
-- Example 27, Subdiv 124-M).
--
-- A ScripForScrip action may now carry an optional per-unit cash component:
-- the holder receives, per `scrip_old_units` units exchanged, the scrip ratio
-- PLUS `scrip_cash_per_unit` cash per old unit. The rollover applies only to
-- the scrip portion, so each consumed parcel's remaining reduced cost base is
-- apportioned between cash and scrip by the consideration's market values —
-- which needs the replacement share's market value just after issue
-- (`scrip_market_value`, per NEW unit). The cash side is an ordinary disposal
-- assessed now (the exchange's closing Sell carries the cash as its per-unit
-- price); the scrip side rolls over exactly as before.
--
-- All three columns are present together (a partial-rollover exchange) or all
-- NULL (the all-scrip case, unchanged), and only on ScripForScrip rows. The
-- shared `currency` column stays NULL for ScripForScrip (its CHECK in the
-- original table definition requires that), so the cash/market-value currency
-- is its own column.
--
-- No new snapshot-staleness triggers: corporate_actions already has
-- insert/update/delete staleness triggers (0001), which fire regardless of
-- which columns a row carries.
ALTER TABLE corporate_actions ADD COLUMN scrip_cash_per_unit TEXT
    CHECK (scrip_cash_per_unit IS NULL OR action_type = 'ScripForScrip');
ALTER TABLE corporate_actions ADD COLUMN scrip_market_value TEXT
    CHECK ((scrip_market_value IS NULL) = (scrip_cash_per_unit IS NULL));
ALTER TABLE corporate_actions ADD COLUMN scrip_cash_currency TEXT
    REFERENCES currencies(code)
    CHECK ((scrip_cash_currency IS NULL) = (scrip_cash_per_unit IS NULL));
