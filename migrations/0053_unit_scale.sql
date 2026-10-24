-- Account unit precision (TODO "Unit granularity and registry rounding of
-- fractional entitlements", design review 2026-10-24, first item).
--
-- The unit grid a quantity sits on belongs to the custody, not the exchange:
-- XNYS is whole units at a CHESS broker and 3 dp at a share plan's DRP, a
-- custodial broker sets its own cap, and a crypto wallet holds whatever dust
-- its fees leave. So the scale is the holding account's, with a per-listing
-- override for the odd holding that differs.
--
-- `holding_accounts.unit_scale` is the number of decimal places a quantity
-- held there may carry; NULL is unconstrained. A user-entered quantity finer
-- than its holding's scale is refused 422 at write time (see
-- `entities::holding_unit_scale` for which write paths check, and why the
-- quantities a ratio-driven operation computes are not refused).
--
-- Seeding: an account's scale is the finest quantity it already holds — every
-- trade quantity in it and every allocation drawn on one of its parcels, read
-- with trailing zeros dropped — so it is never below any stored row and no
-- existing record becomes one its own re-save would refuse. Two cases stay
-- NULL: an account holding any Crypto listing (a wallet has no register grid —
-- an account mixing shares and crypto is left unconstrained rather than given
-- a grid its crypto would break), and an account holding nothing (there is no
-- figure to seed from; the owner sets it).
--
-- `holding_unit_scales` is the per-(account, listing) override. Its own
-- `unit_scale` may be NULL too: "this holding is unconstrained", e.g. a share
-- plan that holds one listing's DRP units to many places in an otherwise
-- whole-unit account. One row per pair (UNIQUE). Both foreign keys cascade: an
-- override is configuration of the pair and means nothing once either is gone,
-- and neither a listing nor an account with any recorded fact can be deleted
-- in the first place.
--
-- Audit: neither table joins `row_history`. The scope rule (0013, restated by
-- 0039 for `exchanges`) excludes a table whose values are consumed when a fact
-- is written and persisted onto the fact row, which is itself audited; a unit
-- scale is narrower still — it changes no stored figure at all, it only decides
-- which writes are accepted. `holding_accounts` stays the identity-only table
-- 0013 called it in that sense, so there is no `*_row_history_*` trigger pair to
-- re-create for the new column.
--
-- Snapshot staleness: exempt, both. No report reads a unit scale, so a write to
-- either can invalidate no snapshot (`reports::snapshot`'s classification).

ALTER TABLE holding_accounts ADD COLUMN unit_scale INTEGER
    CHECK (unit_scale IS NULL
           OR (typeof(unit_scale) = 'integer' AND unit_scale BETWEEN 0 AND 28));

CREATE TABLE holding_unit_scales (
    id                 INTEGER PRIMARY KEY AUTOINCREMENT,
    holding_account_id INTEGER NOT NULL REFERENCES holding_accounts(id) ON DELETE CASCADE,
    listing_id         INTEGER NOT NULL REFERENCES listings(id) ON DELETE CASCADE,
    unit_scale         INTEGER
        CHECK (unit_scale IS NULL
               OR (typeof(unit_scale) = 'integer' AND unit_scale BETWEEN 0 AND 28)),
    UNIQUE (holding_account_id, listing_id)
);

-- The decimal places of a stored TEXT quantity, trailing zeros dropped
-- ('100.000' → 0, '1.50' → 1, '7' → 0). Quantities are written by
-- `rust_decimal`'s Display, which never uses an exponent.
UPDATE holding_accounts
SET unit_scale = (
    SELECT MAX(places) FROM (
        SELECT CASE WHEN instr(t.quantity, '.') = 0 THEN 0
                    ELSE length(rtrim(t.quantity, '0')) - instr(t.quantity, '.') END AS places
        FROM trades t
        WHERE t.holding_account_id = holding_accounts.id
        UNION ALL
        SELECT CASE WHEN instr(pa.quantity_allocated, '.') = 0 THEN 0
                    ELSE length(rtrim(pa.quantity_allocated, '0'))
                         - instr(pa.quantity_allocated, '.') END
        FROM parcel_allocations pa
        JOIN trades p ON p.id = pa.purchase_trade_id
        WHERE p.holding_account_id = holding_accounts.id
    )
)
WHERE EXISTS (SELECT 1 FROM trades t WHERE t.holding_account_id = holding_accounts.id)
  AND NOT EXISTS (
      SELECT 1 FROM trades t JOIN listings l ON l.id = t.listing_id
      WHERE t.holding_account_id = holding_accounts.id AND l.security_type = 'Crypto'
  );
