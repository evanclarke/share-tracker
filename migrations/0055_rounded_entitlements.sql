-- Compute per holding, store per parcel (TODO "Unit granularity and registry
-- rounding of fractional entitlements", design review 2026-10-24, third item).
--
-- A ratio-driven action (`ShareSplit`, `BonusIssue`, `ScripForScrip`,
-- `Demerger`) whose `fraction_rule` (0054) is not `Exact` is applied the way a
-- registry applies it: once per **holding** (holding account × listing), onto
-- the holding's unit grid (`holding_accounts.unit_scale`, or its
-- `holding_unit_scales` override, 0053):
--
--   rounded total = round(ratio × units the action converts, unit_scale, rule)
--
-- and that total is distributed across the holding's parcels by largest
-- remainder on the grid. Each parcel's post-action count is **stored** in
-- `corporate_action_parcel_counts`, rather than re-derived at read time by
-- multiplying the parcel by the ratio — the per-parcel products of a ratio that
-- does not divide are non-terminating, and a sum of their 28-digit roundings
-- has no single right last digit (the CI seed recorded in TODO.md). A holding
-- whose scale is NULL (unconstrained, a crypto wallet) has no grid to round
-- onto, so the action stays exact there and stores nothing for it.
--
-- `corporate_action_parcel_counts`, one row per (action, parcel):
--
-- * For a `ShareSplit`/`BonusIssue` the row's `trade_id` is the re-based parcel
--   itself, `units_before` the units it held immediately before the action (in
--   the unit basis then in force) and `units_after` its stored count after it.
--   `domain::adjustments` re-bases that parcel across the action by exactly
--   units_after / units_before — the stored count, not the announced ratio.
-- * For a `ScripForScrip`/`Demerger` the row's `trade_id` is the replacement
--   (demerged-entity) Buy the operation wrote, whose `quantity` *is* the
--   stored count: `units_after` repeats it, `units_before` is the source
--   parcel's units at the operation date. Informational there (the trade row
--   carries the quantity every reader uses); the row is what the cash-in-lieu
--   operation and the stored-count property read.
-- * `in_lieu_units` is the fraction a `CashInLieu` action's registry sold for
--   cash: the holding's exact count less its rounded-down count, carried in
--   `units_after` of one parcel (the next in largest-remainder order) until
--   the cash-in-lieu Sell disposes of it. 0 on every other row.
--
-- Written inside the action's own write transaction (a split/bonus's PUT, or
-- the exchange/demerge operation), and deleted with it: ON DELETE CASCADE from
-- both the action and the trade (the operation's undo deletes its replacement
-- Buys). Not audited: every row is derived — a function of audited facts (the
-- action, the trades and allocations it counted, the `entitlement_overrides`
-- below) and the holding's unit scale, all of which are frozen while the row
-- exists (the guards below), so the trail of those facts is the trail of these
-- rows; the same ground `report_snapshots` is out of scope on.
--
-- Snapshot staleness: triggered. The open-holdings reports read the stored
-- counts, so a write to either table stales every snapshot dated on or after
-- its action's date — in practice the action write's own trigger has already
-- done so, but no write path is left to rely on that.

CREATE TABLE corporate_action_parcel_counts (
    id                  INTEGER PRIMARY KEY AUTOINCREMENT,
    corporate_action_id INTEGER NOT NULL REFERENCES corporate_actions(id) ON DELETE CASCADE,
    trade_id            INTEGER NOT NULL REFERENCES trades(id) ON DELETE CASCADE,
    -- Decimal as TEXT: the parcel's units immediately before the action, in
    -- the unit basis then in force.
    units_before        TEXT    NOT NULL,
    -- Decimal as TEXT: the parcel's stored count after the action (on the
    -- holding's grid, plus `in_lieu_units`).
    units_after         TEXT    NOT NULL,
    -- Decimal as TEXT: the cash-in-lieu fraction carried in `units_after`
    -- ('0' unless the action's rule is CashInLieu).
    in_lieu_units       TEXT    NOT NULL DEFAULT '0',
    UNIQUE (corporate_action_id, trade_id)
);

CREATE INDEX corporate_action_parcel_counts_trade_id
    ON corporate_action_parcel_counts (trade_id);

-- The "units received" override: the count a holding's statement says it
-- received, where it disagrees with the rule's arithmetic. It replaces that
-- holding's rounded total, which is then distributed across its parcels by
-- largest remainder exactly as a computed one is. One per (action, account);
-- user-entered, so audited.
CREATE TABLE entitlement_overrides (
    id                  INTEGER PRIMARY KEY AUTOINCREMENT,
    corporate_action_id INTEGER NOT NULL REFERENCES corporate_actions(id) ON DELETE CASCADE,
    holding_account_id  INTEGER NOT NULL REFERENCES holding_accounts(id),
    -- Decimal as TEXT: units the holding received from the action (of the
    -- replacement / demerged listing for a ScripForScrip / Demerger).
    units_received      TEXT    NOT NULL,
    UNIQUE (corporate_action_id, holding_account_id)
);

CREATE INDEX entitlement_overrides_holding_account_id
    ON entitlement_overrides (holding_account_id);

CREATE TRIGGER corporate_action_parcel_counts_stale_snapshots_insert
AFTER INSERT ON corporate_action_parcel_counts
BEGIN
    UPDATE report_snapshots SET stale = 1 WHERE snapshot_date >=
        (SELECT date FROM corporate_actions WHERE id = NEW.corporate_action_id);
END;

CREATE TRIGGER corporate_action_parcel_counts_stale_snapshots_update
AFTER UPDATE ON corporate_action_parcel_counts
BEGIN
    UPDATE report_snapshots SET stale = 1 WHERE snapshot_date >=
        MIN((SELECT date FROM corporate_actions WHERE id = OLD.corporate_action_id),
            (SELECT date FROM corporate_actions WHERE id = NEW.corporate_action_id));
END;

CREATE TRIGGER corporate_action_parcel_counts_stale_snapshots_delete
AFTER DELETE ON corporate_action_parcel_counts
BEGIN
    UPDATE report_snapshots SET stale = 1 WHERE snapshot_date >=
        (SELECT date FROM corporate_actions WHERE id = OLD.corporate_action_id);
END;

CREATE TRIGGER entitlement_overrides_stale_snapshots_insert
AFTER INSERT ON entitlement_overrides
BEGIN
    UPDATE report_snapshots SET stale = 1 WHERE snapshot_date >=
        (SELECT date FROM corporate_actions WHERE id = NEW.corporate_action_id);
END;

CREATE TRIGGER entitlement_overrides_stale_snapshots_update
AFTER UPDATE ON entitlement_overrides
BEGIN
    UPDATE report_snapshots SET stale = 1 WHERE snapshot_date >=
        MIN((SELECT date FROM corporate_actions WHERE id = OLD.corporate_action_id),
            (SELECT date FROM corporate_actions WHERE id = NEW.corporate_action_id));
END;

CREATE TRIGGER entitlement_overrides_stale_snapshots_delete
AFTER DELETE ON entitlement_overrides
BEGIN
    UPDATE report_snapshots SET stale = 1 WHERE snapshot_date >=
        (SELECT date FROM corporate_actions WHERE id = OLD.corporate_action_id);
END;

-- ---------------------------------------------------------------------------
-- The facts a stored count depends on are frozen while it stands.
--
-- A split/bonus's stored counts are computed from every parcel and sale of the
-- listing dated before the action (the units it converts — a trade dated on or
-- after it is stated in the post-action basis), in every holding whose unit
-- scale is not NULL. A trade or allocation written behind such an action would
-- change the holding it rounded, so it is refused, and so is a change to the
-- scale itself; editing the action (its rule to `Exact`, say) recomputes or
-- removes its counts, which is the way to make such a change. The refusal is a
-- trigger rather than a check in each write path so that no path — present or
-- future — can bypass it; `infra::http`'s classifier answers the
-- `rounded entitlement:` message `422` with the message as its body.
--
-- The action is matched on `fraction_rule <> 'Exact'` and spelt with
-- equalities rather than an `action_type IN (…)` list, for the reason 0054
-- gives (the UI picker pin reads the table's one `IN` list).
-- ---------------------------------------------------------------------------

-- An `INSERT … ON CONFLICT DO UPDATE` (every upsert here) fires the BEFORE
-- INSERT trigger even when it resolves to an update, so the insert arms let
-- through a row that is already stored with the same counted columns — the
-- update arm judges what actually changes.
CREATE TRIGGER trades_rounded_entitlement_guard_insert BEFORE INSERT ON trades
WHEN NOT EXISTS (
    SELECT 1 FROM trades t
    WHERE t.id = NEW.id AND t.listing_id IS NEW.listing_id AND t.date IS NEW.date
      AND t.quantity IS NEW.quantity AND t.trade_type IS NEW.trade_type
      AND t.holding_account_id IS NEW.holding_account_id)
 AND EXISTS (
    SELECT 1 FROM corporate_actions a
    WHERE a.listing_id = NEW.listing_id
      AND (a.action_type = 'ShareSplit' OR a.action_type = 'BonusIssue')
      AND a.fraction_rule <> 'Exact'
      AND NEW.date < a.date
      AND (SELECT CASE WHEN o.id IS NULL THEN ha.unit_scale ELSE o.unit_scale END
           FROM holding_accounts ha
           LEFT JOIN holding_unit_scales o
                  ON o.holding_account_id = ha.id AND o.listing_id = NEW.listing_id
           WHERE ha.id = NEW.holding_account_id) IS NOT NULL)
BEGIN
    SELECT RAISE(ABORT, 'rounded entitlement: this trade is dated before a share split or bonus issue of the listing that rounds each holding onto its unit grid, so it would change the units that action rounded — set that action''s fraction_rule to Exact, make this change, then set the rule back');
END;

CREATE TRIGGER trades_rounded_entitlement_guard_update BEFORE UPDATE ON trades
WHEN (OLD.listing_id IS NOT NEW.listing_id OR OLD.date IS NOT NEW.date
      OR OLD.quantity IS NOT NEW.quantity OR OLD.trade_type IS NOT NEW.trade_type
      OR OLD.holding_account_id IS NOT NEW.holding_account_id)
 AND EXISTS (
    SELECT 1 FROM corporate_actions a
    WHERE (a.action_type = 'ShareSplit' OR a.action_type = 'BonusIssue')
      AND a.fraction_rule <> 'Exact'
      AND ((a.listing_id = OLD.listing_id AND OLD.date < a.date
            AND (SELECT CASE WHEN o.id IS NULL THEN ha.unit_scale ELSE o.unit_scale END
                 FROM holding_accounts ha
                 LEFT JOIN holding_unit_scales o
                        ON o.holding_account_id = ha.id AND o.listing_id = OLD.listing_id
                 WHERE ha.id = OLD.holding_account_id) IS NOT NULL)
        OR (a.listing_id = NEW.listing_id AND NEW.date < a.date
            AND (SELECT CASE WHEN o.id IS NULL THEN ha.unit_scale ELSE o.unit_scale END
                 FROM holding_accounts ha
                 LEFT JOIN holding_unit_scales o
                        ON o.holding_account_id = ha.id AND o.listing_id = NEW.listing_id
                 WHERE ha.id = NEW.holding_account_id) IS NOT NULL)))
BEGIN
    SELECT RAISE(ABORT, 'rounded entitlement: this trade is dated before a share split or bonus issue of the listing that rounds each holding onto its unit grid, so changing it would change the units that action rounded — set that action''s fraction_rule to Exact, make this change, then set the rule back');
END;

CREATE TRIGGER trades_rounded_entitlement_guard_delete BEFORE DELETE ON trades
WHEN EXISTS (
    SELECT 1 FROM corporate_actions a
    WHERE a.listing_id = OLD.listing_id
      AND (a.action_type = 'ShareSplit' OR a.action_type = 'BonusIssue')
      AND a.fraction_rule <> 'Exact'
      AND OLD.date < a.date
      AND (SELECT CASE WHEN o.id IS NULL THEN ha.unit_scale ELSE o.unit_scale END
           FROM holding_accounts ha
           LEFT JOIN holding_unit_scales o
                  ON o.holding_account_id = ha.id AND o.listing_id = OLD.listing_id
           WHERE ha.id = OLD.holding_account_id) IS NOT NULL)
BEGIN
    SELECT RAISE(ABORT, 'rounded entitlement: this trade is dated before a share split or bonus issue of the listing that rounds each holding onto its unit grid, so deleting it would change the units that action rounded — set that action''s fraction_rule to Exact, make this change, then set the rule back');
END;

-- An allocation is dated by its sale; the holding it changes is its parcel's.
CREATE TRIGGER parcel_allocations_rounded_entitlement_guard_insert
BEFORE INSERT ON parcel_allocations
WHEN EXISTS (
    SELECT 1 FROM trades p
    JOIN trades s ON s.id = NEW.sale_trade_id
    JOIN corporate_actions a ON a.listing_id = p.listing_id
    WHERE p.id = NEW.purchase_trade_id
      AND (a.action_type = 'ShareSplit' OR a.action_type = 'BonusIssue')
      AND a.fraction_rule <> 'Exact'
      AND s.date < a.date
      AND (SELECT CASE WHEN o.id IS NULL THEN ha.unit_scale ELSE o.unit_scale END
           FROM holding_accounts ha
           LEFT JOIN holding_unit_scales o
                  ON o.holding_account_id = ha.id AND o.listing_id = p.listing_id
           WHERE ha.id = p.holding_account_id) IS NOT NULL)
BEGIN
    SELECT RAISE(ABORT, 'rounded entitlement: this sale is dated before a share split or bonus issue of the listing that rounds each holding onto its unit grid, so it would change the units that action rounded — set that action''s fraction_rule to Exact, make this change, then set the rule back');
END;

CREATE TRIGGER parcel_allocations_rounded_entitlement_guard_update
BEFORE UPDATE ON parcel_allocations
WHEN EXISTS (
    SELECT 1 FROM trades p
    JOIN trades s ON s.id IN (OLD.sale_trade_id, NEW.sale_trade_id)
    JOIN corporate_actions a ON a.listing_id = p.listing_id
    WHERE p.id IN (OLD.purchase_trade_id, NEW.purchase_trade_id)
      AND (a.action_type = 'ShareSplit' OR a.action_type = 'BonusIssue')
      AND a.fraction_rule <> 'Exact'
      AND s.date < a.date
      AND (SELECT CASE WHEN o.id IS NULL THEN ha.unit_scale ELSE o.unit_scale END
           FROM holding_accounts ha
           LEFT JOIN holding_unit_scales o
                  ON o.holding_account_id = ha.id AND o.listing_id = p.listing_id
           WHERE ha.id = p.holding_account_id) IS NOT NULL)
BEGIN
    SELECT RAISE(ABORT, 'rounded entitlement: this sale is dated before a share split or bonus issue of the listing that rounds each holding onto its unit grid, so changing it would change the units that action rounded — set that action''s fraction_rule to Exact, make this change, then set the rule back');
END;

CREATE TRIGGER parcel_allocations_rounded_entitlement_guard_delete
BEFORE DELETE ON parcel_allocations
WHEN EXISTS (
    SELECT 1 FROM trades p
    JOIN trades s ON s.id = OLD.sale_trade_id
    JOIN corporate_actions a ON a.listing_id = p.listing_id
    WHERE p.id = OLD.purchase_trade_id
      AND (a.action_type = 'ShareSplit' OR a.action_type = 'BonusIssue')
      AND a.fraction_rule <> 'Exact'
      AND s.date < a.date
      AND (SELECT CASE WHEN o.id IS NULL THEN ha.unit_scale ELSE o.unit_scale END
           FROM holding_accounts ha
           LEFT JOIN holding_unit_scales o
                  ON o.holding_account_id = ha.id AND o.listing_id = p.listing_id
           WHERE ha.id = p.holding_account_id) IS NOT NULL)
BEGIN
    SELECT RAISE(ABORT, 'rounded entitlement: this sale is dated before a share split or bonus issue of the listing that rounds each holding onto its unit grid, so deleting it would change the units that action rounded — set that action''s fraction_rule to Exact, make this change, then set the rule back');
END;

-- The grid itself. An account's scale governs every listing no override
-- covers; an override governs its one pair.
CREATE TRIGGER holding_accounts_rounded_entitlement_guard
BEFORE UPDATE OF unit_scale ON holding_accounts
WHEN OLD.unit_scale IS NOT NEW.unit_scale AND EXISTS (
    SELECT 1 FROM trades t
    JOIN corporate_actions a ON a.listing_id = t.listing_id
    WHERE t.holding_account_id = OLD.id
      AND (a.action_type = 'ShareSplit' OR a.action_type = 'BonusIssue')
      AND a.fraction_rule <> 'Exact'
      AND t.date < a.date
      AND NOT EXISTS (SELECT 1 FROM holding_unit_scales o
                      WHERE o.holding_account_id = OLD.id AND o.listing_id = t.listing_id))
BEGIN
    SELECT RAISE(ABORT, 'rounded entitlement: this account holds units that a share split or bonus issue rounded onto its unit grid, so its unit_scale cannot change — set that action''s fraction_rule to Exact, change the scale, then set the rule back');
END;

CREATE TRIGGER holding_unit_scales_rounded_entitlement_guard_insert
BEFORE INSERT ON holding_unit_scales
WHEN NOT EXISTS (
    SELECT 1 FROM holding_unit_scales o
    WHERE o.id = NEW.id AND o.holding_account_id IS NEW.holding_account_id
      AND o.listing_id IS NEW.listing_id AND o.unit_scale IS NEW.unit_scale)
 AND EXISTS (
    SELECT 1 FROM trades t
    JOIN corporate_actions a ON a.listing_id = t.listing_id
    WHERE t.holding_account_id = NEW.holding_account_id AND t.listing_id = NEW.listing_id
      AND (a.action_type = 'ShareSplit' OR a.action_type = 'BonusIssue')
      AND a.fraction_rule <> 'Exact'
      AND t.date < a.date)
BEGIN
    SELECT RAISE(ABORT, 'rounded entitlement: this holding has units that a share split or bonus issue rounded onto its unit grid, so its unit scale cannot change — set that action''s fraction_rule to Exact, change the scale, then set the rule back');
END;

CREATE TRIGGER holding_unit_scales_rounded_entitlement_guard_update
BEFORE UPDATE ON holding_unit_scales
WHEN (OLD.unit_scale IS NOT NEW.unit_scale
      OR OLD.holding_account_id IS NOT NEW.holding_account_id
      OR OLD.listing_id IS NOT NEW.listing_id)
 AND EXISTS (
    SELECT 1 FROM trades t
    JOIN corporate_actions a ON a.listing_id = t.listing_id
    WHERE ((t.holding_account_id = OLD.holding_account_id AND t.listing_id = OLD.listing_id)
        OR (t.holding_account_id = NEW.holding_account_id AND t.listing_id = NEW.listing_id))
      AND (a.action_type = 'ShareSplit' OR a.action_type = 'BonusIssue')
      AND a.fraction_rule <> 'Exact'
      AND t.date < a.date)
BEGIN
    SELECT RAISE(ABORT, 'rounded entitlement: this holding has units that a share split or bonus issue rounded onto its unit grid, so its unit scale cannot change — set that action''s fraction_rule to Exact, change the scale, then set the rule back');
END;

CREATE TRIGGER holding_unit_scales_rounded_entitlement_guard_delete
BEFORE DELETE ON holding_unit_scales
WHEN EXISTS (
    SELECT 1 FROM trades t
    JOIN corporate_actions a ON a.listing_id = t.listing_id
    WHERE t.holding_account_id = OLD.holding_account_id AND t.listing_id = OLD.listing_id
      AND (a.action_type = 'ShareSplit' OR a.action_type = 'BonusIssue')
      AND a.fraction_rule <> 'Exact'
      AND t.date < a.date)
BEGIN
    SELECT RAISE(ABORT, 'rounded entitlement: this holding has units that a share split or bonus issue rounded onto its unit grid, so its unit scale cannot change — set that action''s fraction_rule to Exact, change the scale, then set the rule back');
END;

-- ---------------------------------------------------------------------------
-- Audit entitlement_overrides: extend row_history's table_name CHECK (the
-- rename pattern of 0018/0021/0027/0031/0039/0048 — see 0018's note on
-- legacy_alter_table), then its trigger pair.
-- ---------------------------------------------------------------------------

PRAGMA legacy_alter_table = ON;
ALTER TABLE row_history RENAME TO row_history_old;
PRAGMA legacy_alter_table = OFF;

CREATE TABLE row_history (
    id         INTEGER PRIMARY KEY AUTOINCREMENT,
    table_name TEXT    NOT NULL CHECK (table_name IN (
                   'trades', 'parcel_allocations', 'income', 'interest_income',
                   'amma_statements', 'amit_adjustments', 'ess_statements',
                   'transfers', 'corporate_actions', 'inheritances',
                   'rights_sales', 'rights_sale_allocations',
                   'investment_expenses', 'drp_enrolments', 'cgt_settings',
                   'attachments', 'listings', 'listing_renames',
                   'closing_prices', 'tax_year_settings', 'rba_fx_rates',
                   'exchange_holidays', 'distribution_events',
                   'entitlement_overrides')),
    row_id     INTEGER NOT NULL,
    operation  TEXT    NOT NULL CHECK (operation IN ('UPDATE', 'DELETE')),
    changed_at TEXT    NOT NULL,
    old_row    TEXT    NOT NULL
);

INSERT INTO row_history (id, table_name, row_id, operation, changed_at, old_row)
    SELECT id, table_name, row_id, operation, changed_at, old_row
    FROM row_history_old
    ORDER BY id;

DROP TABLE row_history_old;

CREATE INDEX row_history_row ON row_history (table_name, row_id);

CREATE TRIGGER row_history_append_only_update BEFORE UPDATE ON row_history
BEGIN
    SELECT RAISE(ABORT, 'row_history is append-only');
END;

CREATE TRIGGER row_history_append_only_delete BEFORE DELETE ON row_history
BEGIN
    SELECT RAISE(ABORT, 'row_history is append-only');
END;

CREATE TRIGGER entitlement_overrides_row_history_update AFTER UPDATE ON entitlement_overrides
BEGIN
    INSERT INTO row_history (table_name, row_id, operation, changed_at, old_row)
    VALUES ('entitlement_overrides', OLD.id, 'UPDATE', strftime('%Y-%m-%dT%H:%M:%fZ', 'now'),
            json_object('id', OLD.id, 'corporate_action_id', OLD.corporate_action_id,
                        'holding_account_id', OLD.holding_account_id,
                        'units_received', OLD.units_received));
END;

CREATE TRIGGER entitlement_overrides_row_history_delete AFTER DELETE ON entitlement_overrides
BEGIN
    INSERT INTO row_history (table_name, row_id, operation, changed_at, old_row)
    VALUES ('entitlement_overrides', OLD.id, 'DELETE', strftime('%Y-%m-%dT%H:%M:%fZ', 'now'),
            json_object('id', OLD.id, 'corporate_action_id', OLD.corporate_action_id,
                        'holding_account_id', OLD.holding_account_id,
                        'units_received', OLD.units_received));
END;
