-- A fraction rule on every ratio-driven action (TODO "Unit granularity and
-- registry rounding of fractional entitlements", design review 2026-10-24,
-- second item).
--
-- A ratio that does not divide a holding produces a count no register holds,
-- and what the register holds instead is the issuer's choice: ASX Listing Rule
-- 7.20 has it disclose its treatment of fractional entitlements, and it varies
-- — rounded up (Wesfarmers' 0.9827 consolidation, 2014), rounded down with no
-- cash, or aggregated, sold and paid out as cash in lieu. So the rule is a
-- term of the action, as DRP's `residual_handling` is a term of the plan.
--
-- `fraction_rule` is one of `Exact` (the registry holds the exact fractional
-- count — what every row recorded before this column means), `Down`, `Up`,
-- `Nearest` (rounded onto the holding's unit grid, 0053) or `CashInLieu`
-- (rounded down, the fraction sold and the proceeds paid out). It belongs to
-- the four ratio-driven types — `ShareSplit`, `BonusIssue`, `ScripForScrip`,
-- `Demerger` — and is NULL on every other one, the per-type payload shape this
-- table has always had. As 0047 explains for `renounceable`, the table-level
-- per-type CHECKs cannot be extended by ALTER, so the shape lives in the
-- column's own CHECK; and the other half — a ratio-driven row *must* carry a
-- rule — is enforced at write time (`CorporateActionBody::kind` /
-- `fraction_rule_refusal`, a 422), because a CHECK added by ADD COLUMN is
-- evaluated against the rows already there, before the backfill below.
--
-- Existing ratio-driven rows migrate to `Exact`: it is what the arithmetic has
-- always done with them, so no stored figure, quantity or report moves. The
-- rule is stored and validated here; the per-holding rounding it describes is
-- applied by a later item of the same TODO section, until which every action
-- is computed as `Exact`.
--
-- corporate_actions is audited (0013), so its two row-history triggers (the
-- live pair from 0047) are dropped *before* the backfill — which is a value
-- the column gains, not a figure that moves, and would otherwise put one
-- UPDATE entry per ratio-driven action into the trail — and re-created after
-- it with the new column in their JSON column list.
--
-- No new snapshot-staleness triggers: corporate_actions' insert/update/delete
-- staleness triggers (0045) list no columns and fire whatever a row carries.

DROP TRIGGER corporate_actions_row_history_update;
DROP TRIGGER corporate_actions_row_history_delete;

-- The ratio-driven types are spelt as equalities rather than an
-- `action_type IN (…)` list: the table's own `action_type` CHECK is the one
-- `IN` list the UI's action-type picker is pinned to
-- (`web::tests::select_option_lists_are_pinned_to_their_server_side_source`),
-- and a second would make that pin ambiguous.
ALTER TABLE corporate_actions ADD COLUMN fraction_rule TEXT
    CHECK (fraction_rule IS NULL
           OR (fraction_rule IN ('Exact', 'Down', 'Up', 'Nearest', 'CashInLieu')
               AND (action_type = 'ShareSplit' OR action_type = 'BonusIssue'
                    OR action_type = 'ScripForScrip' OR action_type = 'Demerger')));

UPDATE corporate_actions SET fraction_rule = 'Exact'
WHERE action_type IN ('ShareSplit', 'BonusIssue', 'ScripForScrip', 'Demerger');

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
                        'fraction_rule', OLD.fraction_rule));
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
                        'fraction_rule', OLD.fraction_rule));
END;
