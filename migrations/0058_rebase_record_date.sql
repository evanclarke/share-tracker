-- A record date on a share split or bonus issue (TODO "Unit granularity and
-- registry rounding of fractional entitlements", *Register by settlement
-- date*).
--
-- A rounded (non-`Exact`) split or bonus issue rounds each holding as the
-- registry does: the units **on the register at the record date**, read off
-- settlement dates — a Buy settled on or before it, a Sell settled after it —
-- per CLAUDE.md's Financial correctness rule. Until now those actions carried
-- no record date and rounded the units *traded* before the action's `date`,
-- which is also what decides the unit basis a trade's quantity is stated in.
-- Under the market timetable the two sets coincide for every normally settled
-- trade (ASX: the record date is the business day after the ex-date and
-- post-ex trades settle on a deferred basis after it; NYSE: a due bill carries
-- the new shares to a buyer between the record date and the ex-date), so they
-- differ only for a trade that straddles them — a pre-ex trade settling after
-- the record date. `corporate_action::rounding` rounds the register and
-- converts each straddling trade as its own settlement obligation
-- (docs/API.md *Fractional entitlements*).
--
-- `rebase_record_date` is that optional date. NULL — every row recorded
-- before this column, and any action entered without one — means the
-- timetable's register: every trade dated before `date` was on it, which is
-- exactly what the arithmetic has always done, so no stored count moves. It is
-- read only under a non-`Exact` rule; on an `Exact` action it is informational.
--
-- A new column rather than the existing `record_date`: that one belongs to
-- `ReturnOfCapital` alone and is CHECKed no later than `date` (0023), while a
-- split's record date may fall on either side of its ex-date (after it on the
-- ASX, before it for a NYSE forward split) — and a table CHECK cannot be
-- changed by ALTER (0047, 0054). The API names it `record_date` on the
-- `ShareSplit` / `BonusIssue` payload, as it does on `ReturnOfCapital`'s.
--
-- corporate_actions is audited (0013): its row-history pair (the live one from
-- 0054) is re-created with the new column in its JSON list. No new staleness
-- trigger: corporate_actions' update trigger stales every snapshot from the
-- action's `date`, which is where a changed register can first move a stored
-- count (and the counts' own triggers, 0055, stale from there too). The
-- `rounding_dirty` marking trigger (0057) fires on any update of a rounded
-- split/bonus, so a changed record date recomputes it. The ShareSplit /
-- BonusIssue equalities follow 0054's reason (the UI picker pin reads the
-- table's one `IN` list).

DROP TRIGGER corporate_actions_row_history_update;
DROP TRIGGER corporate_actions_row_history_delete;

ALTER TABLE corporate_actions ADD COLUMN rebase_record_date TEXT
    CHECK (rebase_record_date IS NULL
           OR action_type = 'ShareSplit' OR action_type = 'BonusIssue');

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
                        'rebase_record_date', OLD.rebase_record_date));
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
                        'rebase_record_date', OLD.rebase_record_date));
END;
