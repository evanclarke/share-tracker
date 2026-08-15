-- Return-of-capital entitlement is fixed at the record date, not the payment
-- date (SCENARIOS B-09).
--
-- `corporate_actions.date` for a ReturnOfCapital is the *payment* date, and the
-- cost-base pipeline tested entitlement by it: every parcel acquired on or
-- before the payment had its cost base reduced. A parcel bought after the
-- shares went ex-entitlement receives nothing, so reducing it understated its
-- cost base and overstated every later gain on it.
--
-- A ReturnOfCapital may now carry an optional `record_date`: units held
-- *before* it earn the payment (a trade dated on it is ex-entitlement), the
-- same convention a RightsIssue's own `date` already uses. NULL keeps the
-- previous payment-date test, so every existing row is unchanged — a
-- correction is entered by adding the record date to the action.
--
-- The record date can never follow the payment it fixes entitlement for, and
-- no other action type carries one (the per-type payload CHECKs of 0001 are
-- table-level and can't be extended by ALTER, so both rules live in this
-- column's own CHECK).
--
-- No new snapshot-staleness triggers: corporate_actions already has
-- insert/update/delete staleness triggers (0001), which fire regardless of
-- which columns a row carries.
ALTER TABLE corporate_actions ADD COLUMN record_date TEXT
    CHECK (record_date IS NULL
           OR (action_type = 'ReturnOfCapital' AND record_date <= date));

-- corporate_actions is audited (0013), so its two row-history triggers are
-- dropped and re-created with the new column in their JSON column list.
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
                        'record_date', OLD.record_date));
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
                        'record_date', OLD.record_date));
END;
