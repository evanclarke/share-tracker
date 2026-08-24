-- A rights issue states whether the offer was renounceable (SCENARIOS AA-b).
--
-- The two tax treatments of a **retail premium** — the payment a company makes
-- to a shareholder who did not (or could not) take up an entitlement — turn
-- entirely on this one fact, and nothing recorded it. Under a **renounceable**
-- offer the premium is capital proceeds on rights deemed acquired with the
-- original shares (TR 2017/4, discount-eligible), which is exactly what
-- `POST /corporate_actions/:id/sell_rights` records. Under a
-- **non-renounceable** offer it is an **unfranked dividend** assessable under
-- s 44 ITAA 1936 (TR 2012/1), and any capital gain on it is reduced to nil by
-- s 118-20 — it belongs on the income path, at item 11S, not in a capital
-- gain. See docs/ato/retail-premiums.md.
--
-- The action's payload was `rights_units`, `rights_held_units`,
-- `exercise_price` and `currency`: nothing distinguished the two offers, so a
-- non-renounceable entitlement offer — perfectly legitimate to record, because
-- *exercising* one is identical either way and the exercise path is the whole
-- reason to enter the action — was silently offered the sell-rights operation,
-- which accepted the premium as capital proceeds. Wrong amount (halved again
-- by the discount, since free rights inherit the original shares' acquisition
-- date) and wrong label on the return, with nothing asked and nothing said.
--
-- It is a **boolean**, not an enumerated column: TR 2012/1's scheme is defined
-- by the entitlements being ones that "cannot be traded, transferred, assigned
-- or otherwise dealt with", and the ATO's own definition is the complement of
-- that ("where these conditions aren't met, the rights are considered to be
-- non-renounceable"), so there are exactly two states and no third to name.
-- 0 and 1 are CHECK-constrained all the same.
--
-- **NULL for every other action type**, the per-type payload shape this table
-- has held since 0001: the fact is a term of a rights offer and of nothing
-- else. The table-level per-type CHECKs of 0001/0045 cannot be extended by
-- ALTER, so the rule lives in the column's own CHECK (the shape 0036's
-- stated-close columns already use).
--
-- The other half — a RightsIssue *must* carry the flag — is not expressible
-- here and is deliberately not chased with a table rebuild: SQLite evaluates a
-- CHECK added by `ALTER TABLE ... ADD COLUMN` against the rows already there,
-- so a constraint requiring the flag would reject the very rows this migration
-- exists to backfill. It is enforced at write time instead, where the rest of
-- this table's per-type payload rules that a CHECK cannot express already live
-- (`CorporateActionBody::kind` — which also enforces every ratio's positivity,
-- likewise absent from the CHECKs): the PUT body must state it for a
-- RightsIssue and must not state it for anything else, and `db_upsert` always
-- binds it. A NULL on a rights issue is therefore unreachable through any
-- write path; a row hand-inserted by raw SQL without it reads back as
-- renounceable, the meaning every row recorded before this column carries.
--
-- Existing rows default to **renounceable**: the whole feature was built for
-- the renounceable case (the endpoint's documentation, the UI's action
-- description and docs/ato/rights-issues.md all say so), so 1 is what every
-- stored row already means, not a guess. The backfill is a value the column
-- gains, not a figure that moves, so the triggers are dropped *before* it and
-- re-created after: the trail would otherwise carry one UPDATE entry per
-- rights issue whose `old_row` predates the column it was written for.
--
-- No new snapshot-staleness triggers: corporate_actions already has
-- insert/update/delete staleness triggers (0001, re-created by 0045), which
-- fire regardless of which columns a row carries.

-- corporate_actions is audited (0013), so its two row-history triggers are
-- dropped and re-created with the new column in their JSON column list. The
-- live pair came from 0045 (which rebuilt the table for AUTOINCREMENT ids);
-- this replaces it.
DROP TRIGGER corporate_actions_row_history_update;
DROP TRIGGER corporate_actions_row_history_delete;

ALTER TABLE corporate_actions ADD COLUMN renounceable INTEGER
    CHECK (renounceable IS NULL
           OR (action_type = 'RightsIssue' AND renounceable IN (0, 1)));

UPDATE corporate_actions SET renounceable = 1 WHERE action_type = 'RightsIssue';

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
                        'renounceable', OLD.renounceable));
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
                        'renounceable', OLD.renounceable));
END;
