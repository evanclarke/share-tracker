-- Bonus shares (docs/bonus-shares.md): a third corporate action type,
-- 'BonusIssue'. On the issue date every `bonus_held_units` units held receive
-- `bonus_units` additional units (a 1-for-10 bonus issue is bonus_units=1 /
-- bonus_held_units=10). The general post-1 July 1998 case is not assessed as
-- a dividend: the bonus shares take the original parcel's acquisition date
-- and the parcel's cost base is apportioned over original + bonus shares —
-- total cost base unchanged, per-unit cost base shrinks. (Bonus shares chosen
-- in lieu of a dividend ARE a dividend — entered as a DRP trade, not here.)
--
-- SQLite cannot widen a CHECK in place, so the table is rebuilt via the
-- rename pattern — no data dropped; DROP TABLE only touches the renamed _old
-- table. Each action type carries exactly its own payload columns, enforced
-- by per-type CHECKs.
ALTER TABLE corporate_actions RENAME TO corporate_actions_old;

CREATE TABLE corporate_actions (
    id               INTEGER PRIMARY KEY,
    action_type      TEXT    NOT NULL
        CHECK (action_type IN ('ReturnOfCapital', 'ShareSplit', 'BonusIssue')),
    listing_id       INTEGER NOT NULL REFERENCES listings(id),
    -- ReturnOfCapital: payment date — parcels acquired on/before it are affected.
    -- ShareSplit: conversion date — parcels acquired before it are converted
    -- (a trade dated on the conversion date is already in post-split units).
    -- BonusIssue: issue date — parcels acquired before it receive bonus units
    -- (a trade dated on the issue date is ex-bonus).
    date             TEXT    NOT NULL,
    -- ReturnOfCapital only: per-unit payment amount in `currency` (TEXT
    -- decimal, must be positive).
    amount_per_unit  TEXT,
    currency         TEXT    REFERENCES currencies(code),
    -- ShareSplit only: every split_old_units existing units become
    -- split_new_units units (TEXT decimals, must be positive).
    split_new_units  TEXT,
    split_old_units  TEXT,
    -- BonusIssue only: every bonus_held_units units held receive bonus_units
    -- additional units (TEXT decimals, must be positive).
    bonus_units      TEXT,
    bonus_held_units TEXT,
    CHECK (action_type <> 'ReturnOfCapital'
           OR (amount_per_unit IS NOT NULL AND currency IS NOT NULL
               AND split_new_units IS NULL AND split_old_units IS NULL
               AND bonus_units IS NULL AND bonus_held_units IS NULL)),
    CHECK (action_type <> 'ShareSplit'
           OR (split_new_units IS NOT NULL AND split_old_units IS NOT NULL
               AND amount_per_unit IS NULL AND currency IS NULL
               AND bonus_units IS NULL AND bonus_held_units IS NULL)),
    CHECK (action_type <> 'BonusIssue'
           OR (bonus_units IS NOT NULL AND bonus_held_units IS NOT NULL
               AND amount_per_unit IS NULL AND currency IS NULL
               AND split_new_units IS NULL AND split_old_units IS NULL))
);

INSERT INTO corporate_actions
    (id, action_type, listing_id, date, amount_per_unit, currency,
     split_new_units, split_old_units)
SELECT id, action_type, listing_id, date, amount_per_unit, currency,
       split_new_units, split_old_units
FROM corporate_actions_old;

DROP TABLE corporate_actions_old;
