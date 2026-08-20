-- A demerger restates the provider's price series too, and nothing re-based it
-- back (found 2026-08-20, checking the live database against the invariant
-- SCENARIOS Q-14 had just established).
--
-- 0034 made a stored closing price the price the security actually traded at on
-- its own date, derived from `price_as_observed` by the ShareSplit/BonusIssue
-- ratios dated in (price_date, fetched_at]. A **Demerger** breaks that same
-- invariant by the same mechanism: the provider applies a spin-off
-- price-adjustment factor to the whole pre-demerger series exactly as it does
-- for a split — but a demerger changes no unit count on the original listing
-- (it issues units of a *different* listing), so there is no ratio to read and
-- nothing re-bases the price back. Evan's LAC history is the live case: the
-- one stored pre-demerger close is the current adjusted level, ~2.46x below
-- what the security actually closed at that day.
--
-- The factor cannot be derived from the action's existing terms:
-- `demerger_cost_base_pct` is an ATO cost-base apportionment, not a price
-- ratio, and the provider's factor is set by the two entities' market values
-- at the spin-off. So the action gains a **stated fact** instead — what the
-- security actually closed at on the last pre-demerger trading day — carrying
-- the same `sourced_from` / `reason` provenance a hand-entered closing price
-- does (0020). The factor is *derived* from it against the provider's own
-- adjusted figure for that same day, so the entry is auditable and the
-- arithmetic is the system's.
--
-- The two sides of the factor are stored, never the quotient: the provider
-- side is looked up lazily at re-base time, so the close can be stated before
-- any history is backfilled (and re-derives itself if that history is
-- re-fetched), exactly the order-freedom 0034's `price_as_observed` buys.
--
-- **Optional**, not required. The live database already holds a Demerger row
-- (LAC → LAR, 2023-10-03) entered before this existed; a NOT NULL column would
-- make it uneditable until the close was supplied. A demerger whose head
-- listing has no fetched pre-demerger prices needs no statement at all. All
-- four columns are present together or all absent — the all-or-none shape
-- 0007's `scrip_cash_*` columns already use — and only on a Demerger row. The
-- close date is the last *pre*-demerger trading day, so it is strictly before
-- the action's own date (the per-type payload CHECKs of 0001 are table-level
-- and cannot be extended by ALTER, so both rules live in these columns' own
-- CHECKs).
--
-- The stated close is a money amount in the listing's quote currency, so it is
-- TEXT like every other decimal here — never REAL, which would round-trip the
-- figure the factor is derived from.
--
-- No new snapshot-staleness triggers: corporate_actions already has
-- insert/update/delete staleness triggers (0001), which fire regardless of
-- which columns a row carries — and a stated close moves stored prices, whose
-- own update trigger stales the snapshots valued at them.
ALTER TABLE corporate_actions ADD COLUMN demerger_close_date TEXT
    CHECK (demerger_close_date IS NULL
           OR (action_type = 'Demerger' AND demerger_close_date < date));
ALTER TABLE corporate_actions ADD COLUMN demerger_close_price TEXT
    CHECK ((demerger_close_price IS NULL) = (demerger_close_date IS NULL));
ALTER TABLE corporate_actions ADD COLUMN demerger_close_sourced_from TEXT
    CHECK ((demerger_close_sourced_from IS NULL) = (demerger_close_date IS NULL));
ALTER TABLE corporate_actions ADD COLUMN demerger_close_reason TEXT
    CHECK ((demerger_close_reason IS NULL) = (demerger_close_date IS NULL));

-- corporate_actions is audited (0013), so its two row-history triggers are
-- dropped and re-created with the new columns in their JSON column list.
DROP TRIGGER corporate_actions_row_history_update;
DROP TRIGGER corporate_actions_row_history_delete;

CREATE TRIGGER corporate_actions_row_history_update AFTER UPDATE ON corporate_actions
BEGIN
    INSERT INTO row_history (table_name, row_id, operation, changed_at, old_row)
    VALUES ('corporate_actions', OLD.id, 'UPDATE', strftime('%Y-%m-%dT%H:%M:%fZ', 'now'),
            json_object(
                        'id', OLD.id, 'action_type', OLD.action_type, 'listing_id',
                        OLD.listing_id, 'date', OLD.date, 'amount_per_unit',
                        OLD.amount_per_unit, 'currency', OLD.currency, 'split_new_units',
                        OLD.split_new_units, 'split_old_units', OLD.split_old_units,
                        'bonus_units', OLD.bonus_units, 'bonus_held_units',
                        OLD.bonus_held_units, 'rights_units', OLD.rights_units,
                        'rights_held_units', OLD.rights_held_units, 'exercise_price',
                        OLD.exercise_price, 'buyback_price', OLD.buyback_price,
                        'buyback_dividend', OLD.buyback_dividend, 'buyback_franking_credit',
                        OLD.buyback_franking_credit, 'buyback_market_value',
                        OLD.buyback_market_value, 'scrip_listing_id', OLD.scrip_listing_id,
                        'scrip_new_units', OLD.scrip_new_units, 'scrip_old_units',
                        OLD.scrip_old_units, 'demerger_listing_id', OLD.demerger_listing_id,
                        'demerger_new_units', OLD.demerger_new_units, 'demerger_held_units',
                        OLD.demerger_held_units, 'demerger_cost_base_pct',
                        OLD.demerger_cost_base_pct, 'worthless_event', OLD.worthless_event,
                        'scrip_cash_per_unit', OLD.scrip_cash_per_unit, 'scrip_market_value',
                        OLD.scrip_market_value, 'scrip_cash_currency', OLD.scrip_cash_currency,
                        'record_date', OLD.record_date,
                        'demerger_close_date', OLD.demerger_close_date,
                        'demerger_close_price', OLD.demerger_close_price,
                        'demerger_close_sourced_from', OLD.demerger_close_sourced_from,
                        'demerger_close_reason', OLD.demerger_close_reason));
END;

CREATE TRIGGER corporate_actions_row_history_delete AFTER DELETE ON corporate_actions
BEGIN
    INSERT INTO row_history (table_name, row_id, operation, changed_at, old_row)
    VALUES ('corporate_actions', OLD.id, 'DELETE', strftime('%Y-%m-%dT%H:%M:%fZ', 'now'),
            json_object(
                        'id', OLD.id, 'action_type', OLD.action_type, 'listing_id',
                        OLD.listing_id, 'date', OLD.date, 'amount_per_unit',
                        OLD.amount_per_unit, 'currency', OLD.currency, 'split_new_units',
                        OLD.split_new_units, 'split_old_units', OLD.split_old_units,
                        'bonus_units', OLD.bonus_units, 'bonus_held_units',
                        OLD.bonus_held_units, 'rights_units', OLD.rights_units,
                        'rights_held_units', OLD.rights_held_units, 'exercise_price',
                        OLD.exercise_price, 'buyback_price', OLD.buyback_price,
                        'buyback_dividend', OLD.buyback_dividend, 'buyback_franking_credit',
                        OLD.buyback_franking_credit, 'buyback_market_value',
                        OLD.buyback_market_value, 'scrip_listing_id', OLD.scrip_listing_id,
                        'scrip_new_units', OLD.scrip_new_units, 'scrip_old_units',
                        OLD.scrip_old_units, 'demerger_listing_id', OLD.demerger_listing_id,
                        'demerger_new_units', OLD.demerger_new_units, 'demerger_held_units',
                        OLD.demerger_held_units, 'demerger_cost_base_pct',
                        OLD.demerger_cost_base_pct, 'worthless_event', OLD.worthless_event,
                        'scrip_cash_per_unit', OLD.scrip_cash_per_unit, 'scrip_market_value',
                        OLD.scrip_market_value, 'scrip_cash_currency', OLD.scrip_cash_currency,
                        'record_date', OLD.record_date,
                        'demerger_close_date', OLD.demerger_close_date,
                        'demerger_close_price', OLD.demerger_close_price,
                        'demerger_close_sourced_from', OLD.demerger_close_sourced_from,
                        'demerger_close_reason', OLD.demerger_close_reason));
END;
