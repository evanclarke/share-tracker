-- A record date on a scrip-for-scrip exchange or demerger (TODO "Unit
-- granularity and registry rounding of fractional entitlements", *Register by
-- record date for rounded scrip exchanges and demergers*).
--
-- 0058 gave a rounded share split / bonus issue a record date and rounded the
-- holding **registered** at it — Buys settled on or before it, Sells settled
-- after it, per CLAUDE.md's Financial correctness rule. A rounded
-- (non-`Exact`) `ScripForScrip` / `Demerger` still rounded the parcels open on
-- its operation date by trade date. It now rounds its register the same way
-- (`corporate_action::rounding::db_round_replacements`): a Sell straddling the
-- record date (traded before the operation, settled after it) is rounded with
-- its seller's holding and its own converted count delivered out of it, and a
-- Buy settled after it is converted alone as a settlement obligation. The
-- parcels the operation consumes do not change — every one open on its date,
-- by trade date — so the record date decides only each replacement's count.
--
-- `rollover_record_date` is that optional date. NULL — every row recorded
-- before this column, and any action entered without one — is the timetable's
-- register, every trade dated before `date`: what every exchange and demerger
-- has always rounded, so no stored count moves. It is read only under a
-- non-`Exact` rule and only when the operation runs (the action is frozen while
-- its group exists, so it cannot change under a stored count).
--
-- A new column rather than 0058's `rebase_record_date`, whose CHECK confines it
-- to ShareSplit / BonusIssue — a table CHECK cannot be changed by ALTER (0047,
-- 0054, 0058). The API names it `record_date` on the `ScripForScrip` /
-- `Demerger` payloads too. Like a split's, it may fall on either side of `date`
-- (a demerger recorded at its ex-date has its record date the business day
-- after; one recorded at its implementation date, the week before).
--
-- corporate_actions is audited (0013): its row-history pair (the live one from
-- 0058) is re-created with the new column in its JSON list. No new staleness
-- trigger: the stored counts an exchange or demerger writes belong to its
-- replacement Buys, whose own trades triggers stale snapshots when the
-- operation runs, and corporate_actions' update trigger stales every snapshot
-- from the action's `date` on any edit. No `rounding_dirty` marking: that
-- recompute covers only splits and bonus issues; an exchange's or demerger's
-- counts are written once, by the operation. The ScripForScrip / Demerger
-- equalities follow 0054's reason (the UI picker pin reads the table's one
-- `IN` list).

DROP TRIGGER corporate_actions_row_history_update;
DROP TRIGGER corporate_actions_row_history_delete;

ALTER TABLE corporate_actions ADD COLUMN rollover_record_date TEXT
    CHECK (rollover_record_date IS NULL
           OR action_type = 'ScripForScrip' OR action_type = 'Demerger');

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
                        'demerger_close_reason', OLD.demerger_close_reason,
                        'renounceable', OLD.renounceable,
                        'fraction_rule', OLD.fraction_rule,
                        'rebase_record_date', OLD.rebase_record_date,
                        'rollover_record_date', OLD.rollover_record_date));
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
                        'demerger_close_reason', OLD.demerger_close_reason,
                        'renounceable', OLD.renounceable,
                        'fraction_rule', OLD.fraction_rule,
                        'rebase_record_date', OLD.rebase_record_date,
                        'rollover_record_date', OLD.rollover_record_date));
END;
