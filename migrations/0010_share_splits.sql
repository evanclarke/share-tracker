-- Share splits / consolidations (TD 2000/10, docs/share-splits-and-consolidations.md):
-- a second corporate action type, 'ShareSplit'. On the conversion date every
-- `split_old_units` units of the listing become `split_new_units` units (a
-- 2-for-1 split is new=2/old=1; a 1-for-10 consolidation is new=1/old=10).
-- No CGT event: total cost base and the original acquisition date are
-- preserved; only the unit count (and so the per-unit cost base) changes.
--
-- The per-type payload columns become nullable with CHECKs tying each to its
-- action type: ReturnOfCapital carries amount_per_unit + currency (split
-- columns null), ShareSplit carries the ratio columns (payment columns null).
-- SQLite cannot widen a CHECK in place, so the table is rebuilt via the rename
-- pattern — no data dropped; DROP TABLE only touches the renamed _old table.
ALTER TABLE corporate_actions RENAME TO corporate_actions_old;

CREATE TABLE corporate_actions (
    id              INTEGER PRIMARY KEY,
    action_type     TEXT    NOT NULL CHECK (action_type IN ('ReturnOfCapital', 'ShareSplit')),
    listing_id      INTEGER NOT NULL REFERENCES listings(id),
    -- ReturnOfCapital: payment date — parcels acquired on/before it are affected.
    -- ShareSplit: conversion date — parcels acquired before it are converted
    -- (a trade dated on the conversion date is already in post-split units).
    date            TEXT    NOT NULL,
    -- ReturnOfCapital only: per-unit payment amount in `currency` (TEXT
    -- decimal, must be positive).
    amount_per_unit TEXT,
    currency        TEXT    REFERENCES currencies(code),
    -- ShareSplit only: every split_old_units existing units become
    -- split_new_units units (TEXT decimals, must be positive).
    split_new_units TEXT,
    split_old_units TEXT,
    CHECK (action_type <> 'ReturnOfCapital'
           OR (amount_per_unit IS NOT NULL AND currency IS NOT NULL
               AND split_new_units IS NULL AND split_old_units IS NULL)),
    CHECK (action_type <> 'ShareSplit'
           OR (split_new_units IS NOT NULL AND split_old_units IS NOT NULL
               AND amount_per_unit IS NULL AND currency IS NULL))
);

INSERT INTO corporate_actions (id, action_type, listing_id, date, amount_per_unit, currency)
SELECT id, action_type, listing_id, date, amount_per_unit, currency FROM corporate_actions_old;

DROP TABLE corporate_actions_old;
