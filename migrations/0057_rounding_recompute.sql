-- Recompute on write instead of freezing (TODO "Unit granularity and registry
-- rounding of fractional entitlements", *Recompute on write instead of
-- freezing*). Supersedes 0055's guard triggers.
--
-- 0055 froze the facts a rounded share split's or bonus issue's stored
-- per-parcel counts (`corporate_action_parcel_counts`) were computed from: a
-- trade, sale or allocation dated before the action in a holding with a unit
-- grid, and that holding's grid, were refused `422` while the counts stood,
-- with "set the action to Exact and back" as the only way through. This
-- migration drops those ten guards and replaces the refusal with
-- recomputation: a write that touches such a fact recomputes the listing's
-- stored counts inside its own write transaction
-- (`corporate_action::rounding::db_settle`), refusing `422` only where the
-- recomputed counts leave a sale that no longer fits its parcel, or a recorded
-- cash-in-lieu payment for a fraction that has moved.
--
-- What makes the recompute structural rather than one more thing each write
-- path must remember is a **deferred foreign key that can never be satisfied**:
--
-- * `rounding_never` is a table that can never hold a row (its CHECK fails
--   every insert);
-- * `rounding_dirty` holds one row per listing awaiting a recompute, and its
--   `unsettled` column references `rounding_never` DEFERRABLE INITIALLY
--   DEFERRED — so every row in it is a foreign-key violation SQLite checks only
--   at COMMIT;
-- * the triggers below insert the written fact's listing into
--   `rounding_dirty` whenever the listing carries a non-`Exact` share split or
--   bonus issue.
--
-- `db_settle` recomputes each listing in `rounding_dirty` and deletes its row.
-- A write path that forgets to call it cannot commit: COMMIT fails with a
-- foreign-key violation and the transaction rolls back, so the forgotten path
-- fails loudly in its first test rather than storing stale counts. (An
-- autocommit statement is checked at its own end the same way.) The table is
-- empty outside a write transaction.
--
-- Marking is deliberately broad: any write to a listing that has a rounded
-- split/bonus, whatever its date or holding — the recompute is what decides
-- whether anything moved, and a recompute that changes nothing writes nothing.
-- Columns a recompute does not read (a trade's price, brokerage, residuals,
-- provenance) do not mark. `settlement_date` does: the counts are decided by
-- trade date today (a Known limitation), but the next item of the same TODO
-- section moves them onto the register's settlement date, and the trigger is
-- then already right.
--
-- Neither table is audited (neither holds a fact; `rounding_dirty` is empty at
-- every commit) and neither stales a snapshot (the recompute's writes to
-- `corporate_action_parcel_counts` carry their own staleness triggers, 0055).
-- The action is matched with equalities rather than an `action_type IN (…)`
-- list, for the reason 0054 gives (the UI picker pin reads the table's one
-- `IN` list).

DROP TRIGGER trades_rounded_entitlement_guard_insert;
DROP TRIGGER trades_rounded_entitlement_guard_update;
DROP TRIGGER trades_rounded_entitlement_guard_delete;
DROP TRIGGER parcel_allocations_rounded_entitlement_guard_insert;
DROP TRIGGER parcel_allocations_rounded_entitlement_guard_update;
DROP TRIGGER parcel_allocations_rounded_entitlement_guard_delete;
DROP TRIGGER holding_accounts_rounded_entitlement_guard;
DROP TRIGGER holding_unit_scales_rounded_entitlement_guard_insert;
DROP TRIGGER holding_unit_scales_rounded_entitlement_guard_update;
DROP TRIGGER holding_unit_scales_rounded_entitlement_guard_delete;

-- The parent no row can ever reference: its CHECK refuses every insert.
CREATE TABLE rounding_never (
    id INTEGER PRIMARY KEY,
    CHECK (0)
);

-- A listing whose rounded counts the open write transaction has yet to
-- recompute. Every row violates the deferred foreign key, so COMMIT succeeds
-- only once `db_settle` has drained the table. Deliberately keyless: a
-- listing written to several times is marked several times. A unique key
-- would need `INSERT OR IGNORE` in the triggers, and SQLite lets the *outer*
-- statement's conflict clause override a trigger's — an `INSERT … ON
-- CONFLICT DO UPDATE` upsert of a trade would turn the trigger's IGNORE into
-- an ABORT on the second mark.
CREATE TABLE rounding_dirty (
    listing_id INTEGER NOT NULL,
    unsettled  INTEGER NOT NULL DEFAULT 0
               REFERENCES rounding_never(id) DEFERRABLE INITIALLY DEFERRED
);

-- trades: a parcel or sale of the listing (and its date, size, kind, holding
-- or register date) — before and after the write, since an update can move a
-- trade between listings.
CREATE TRIGGER trades_rounding_dirty_insert AFTER INSERT ON trades
WHEN EXISTS (SELECT 1 FROM corporate_actions a
             WHERE a.listing_id = NEW.listing_id
               AND (a.action_type = 'ShareSplit' OR a.action_type = 'BonusIssue')
               AND a.fraction_rule <> 'Exact')
BEGIN
    INSERT INTO rounding_dirty (listing_id) VALUES (NEW.listing_id);
END;

CREATE TRIGGER trades_rounding_dirty_update
AFTER UPDATE OF listing_id, date, settlement_date, quantity, trade_type, holding_account_id
ON trades
WHEN OLD.listing_id IS NOT NEW.listing_id OR OLD.date IS NOT NEW.date
  OR OLD.settlement_date IS NOT NEW.settlement_date OR OLD.quantity IS NOT NEW.quantity
  OR OLD.trade_type IS NOT NEW.trade_type OR OLD.holding_account_id IS NOT NEW.holding_account_id
BEGIN
    INSERT INTO rounding_dirty (listing_id)
    SELECT l.id FROM listings l
    WHERE l.id IN (OLD.listing_id, NEW.listing_id)
      AND EXISTS (SELECT 1 FROM corporate_actions a
                  WHERE a.listing_id = l.id
                    AND (a.action_type = 'ShareSplit' OR a.action_type = 'BonusIssue')
                    AND a.fraction_rule <> 'Exact');
END;

CREATE TRIGGER trades_rounding_dirty_delete AFTER DELETE ON trades
WHEN EXISTS (SELECT 1 FROM corporate_actions a
             WHERE a.listing_id = OLD.listing_id
               AND (a.action_type = 'ShareSplit' OR a.action_type = 'BonusIssue')
               AND a.fraction_rule <> 'Exact')
BEGIN
    INSERT INTO rounding_dirty (listing_id) VALUES (OLD.listing_id);
END;

-- parcel_allocations: a sale's draw on a parcel, marked under the parcel's
-- listing (the sale's is the same — `sell` refuses a cross-listing draw).
CREATE TRIGGER parcel_allocations_rounding_dirty_insert AFTER INSERT ON parcel_allocations
BEGIN
    INSERT INTO rounding_dirty (listing_id)
    SELECT p.listing_id FROM trades p
    WHERE p.id = NEW.purchase_trade_id
      AND EXISTS (SELECT 1 FROM corporate_actions a
                  WHERE a.listing_id = p.listing_id
                    AND (a.action_type = 'ShareSplit' OR a.action_type = 'BonusIssue')
                    AND a.fraction_rule <> 'Exact');
END;

CREATE TRIGGER parcel_allocations_rounding_dirty_update AFTER UPDATE ON parcel_allocations
BEGIN
    INSERT INTO rounding_dirty (listing_id)
    SELECT DISTINCT p.listing_id FROM trades p
    WHERE p.id IN (OLD.purchase_trade_id, NEW.purchase_trade_id)
      AND EXISTS (SELECT 1 FROM corporate_actions a
                  WHERE a.listing_id = p.listing_id
                    AND (a.action_type = 'ShareSplit' OR a.action_type = 'BonusIssue')
                    AND a.fraction_rule <> 'Exact');
END;

CREATE TRIGGER parcel_allocations_rounding_dirty_delete AFTER DELETE ON parcel_allocations
BEGIN
    INSERT INTO rounding_dirty (listing_id)
    SELECT p.listing_id FROM trades p
    WHERE p.id = OLD.purchase_trade_id
      AND EXISTS (SELECT 1 FROM corporate_actions a
                  WHERE a.listing_id = p.listing_id
                    AND (a.action_type = 'ShareSplit' OR a.action_type = 'BonusIssue')
                    AND a.fraction_rule <> 'Exact');
END;

-- The grid: an account's scale governs every listing it holds that no
-- override covers (marking an overridden one too is harmless — its recompute
-- moves nothing); an override governs its one pair.
CREATE TRIGGER holding_accounts_rounding_dirty AFTER UPDATE OF unit_scale ON holding_accounts
WHEN OLD.unit_scale IS NOT NEW.unit_scale
BEGIN
    INSERT INTO rounding_dirty (listing_id)
    SELECT DISTINCT t.listing_id FROM trades t
    WHERE t.holding_account_id = NEW.id
      AND EXISTS (SELECT 1 FROM corporate_actions a
                  WHERE a.listing_id = t.listing_id
                    AND (a.action_type = 'ShareSplit' OR a.action_type = 'BonusIssue')
                    AND a.fraction_rule <> 'Exact');
END;

CREATE TRIGGER holding_unit_scales_rounding_dirty_insert AFTER INSERT ON holding_unit_scales
WHEN EXISTS (SELECT 1 FROM corporate_actions a
             WHERE a.listing_id = NEW.listing_id
               AND (a.action_type = 'ShareSplit' OR a.action_type = 'BonusIssue')
               AND a.fraction_rule <> 'Exact')
BEGIN
    INSERT INTO rounding_dirty (listing_id) VALUES (NEW.listing_id);
END;

CREATE TRIGGER holding_unit_scales_rounding_dirty_update AFTER UPDATE ON holding_unit_scales
BEGIN
    INSERT INTO rounding_dirty (listing_id)
    SELECT l.id FROM listings l
    WHERE l.id IN (OLD.listing_id, NEW.listing_id)
      AND EXISTS (SELECT 1 FROM corporate_actions a
                  WHERE a.listing_id = l.id
                    AND (a.action_type = 'ShareSplit' OR a.action_type = 'BonusIssue')
                    AND a.fraction_rule <> 'Exact');
END;

CREATE TRIGGER holding_unit_scales_rounding_dirty_delete AFTER DELETE ON holding_unit_scales
WHEN EXISTS (SELECT 1 FROM corporate_actions a
             WHERE a.listing_id = OLD.listing_id
               AND (a.action_type = 'ShareSplit' OR a.action_type = 'BonusIssue')
               AND a.fraction_rule <> 'Exact')
BEGIN
    INSERT INTO rounding_dirty (listing_id) VALUES (OLD.listing_id);
END;

-- The action itself: a rounded split/bonus written, re-termed, moved, made
-- `Exact` (its counts must go) or deleted.
CREATE TRIGGER corporate_actions_rounding_dirty_insert AFTER INSERT ON corporate_actions
WHEN (NEW.action_type = 'ShareSplit' OR NEW.action_type = 'BonusIssue')
 AND NEW.fraction_rule <> 'Exact'
BEGIN
    INSERT INTO rounding_dirty (listing_id) VALUES (NEW.listing_id);
END;

CREATE TRIGGER corporate_actions_rounding_dirty_update AFTER UPDATE ON corporate_actions
WHEN ((OLD.action_type = 'ShareSplit' OR OLD.action_type = 'BonusIssue')
      AND OLD.fraction_rule <> 'Exact')
  OR ((NEW.action_type = 'ShareSplit' OR NEW.action_type = 'BonusIssue')
      AND NEW.fraction_rule <> 'Exact')
BEGIN
    INSERT INTO rounding_dirty (listing_id) VALUES (OLD.listing_id);
    INSERT INTO rounding_dirty (listing_id) VALUES (NEW.listing_id);
END;

CREATE TRIGGER corporate_actions_rounding_dirty_delete AFTER DELETE ON corporate_actions
WHEN (OLD.action_type = 'ShareSplit' OR OLD.action_type = 'BonusIssue')
 AND OLD.fraction_rule <> 'Exact'
BEGIN
    INSERT INTO rounding_dirty (listing_id) VALUES (OLD.listing_id);
END;

-- A units-received override of a split/bonus replaces a holding's rounded
-- total. (One on a scrip exchange or demerger is read when that operation
-- runs, and is frozen after it — `entitlement_override` — so it marks nothing.)
CREATE TRIGGER entitlement_overrides_rounding_dirty_insert AFTER INSERT ON entitlement_overrides
BEGIN
    INSERT INTO rounding_dirty (listing_id)
    SELECT a.listing_id FROM corporate_actions a
    WHERE a.id = NEW.corporate_action_id
      AND (a.action_type = 'ShareSplit' OR a.action_type = 'BonusIssue')
      AND a.fraction_rule <> 'Exact';
END;

CREATE TRIGGER entitlement_overrides_rounding_dirty_update AFTER UPDATE ON entitlement_overrides
BEGIN
    INSERT INTO rounding_dirty (listing_id)
    SELECT DISTINCT a.listing_id FROM corporate_actions a
    WHERE a.id IN (OLD.corporate_action_id, NEW.corporate_action_id)
      AND (a.action_type = 'ShareSplit' OR a.action_type = 'BonusIssue')
      AND a.fraction_rule <> 'Exact';
END;

CREATE TRIGGER entitlement_overrides_rounding_dirty_delete AFTER DELETE ON entitlement_overrides
BEGIN
    INSERT INTO rounding_dirty (listing_id)
    SELECT a.listing_id FROM corporate_actions a
    WHERE a.id = OLD.corporate_action_id
      AND (a.action_type = 'ShareSplit' OR a.action_type = 'BonusIssue')
      AND a.fraction_rule <> 'Exact';
END;
