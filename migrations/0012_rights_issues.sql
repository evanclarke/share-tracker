-- Rights issues (docs/rights-issues.md): a fourth corporate action type,
-- 'RightsIssue'. On the record/issue `date` every `rights_held_units` units
-- held entitle the holder to acquire `rights_units` new units at
-- `exercise_price` per unit in `currency` (a 1-for-4 rights issue is
-- rights_units=1 / rights_held_units=4). Recording the action has no effect
-- on existing parcels (free rights are non-assessable non-exempt income on
-- issue); exercising it — POST /corporate_actions/:id/exercise — creates a
-- new Buy parcel dated the exercise date (the CGT discount clock runs from
-- exercise) whose cost base is the amount paid to exercise plus any amount
-- paid to acquire the rights. Selling or lapsing the rights themselves is not
-- modelled (see docs/rights-issues.md).
--
-- SQLite cannot widen a CHECK in place, so the table is rebuilt via the
-- rename pattern — no data dropped; DROP TABLE only touches the renamed _old
-- table. Each action type carries exactly its own payload columns, enforced
-- by per-type CHECKs ('currency' is shared by ReturnOfCapital and
-- RightsIssue). NOTE for future rebuilds: trades.rights_action_id (added
-- below) references this table, and ALTER TABLE ... RENAME rewrites that
-- foreign key to follow the rename — a later rebuild of corporate_actions
-- must re-point or rebuild trades as well.
ALTER TABLE corporate_actions RENAME TO corporate_actions_old;

CREATE TABLE corporate_actions (
    id                INTEGER PRIMARY KEY,
    action_type       TEXT    NOT NULL
        CHECK (action_type IN ('ReturnOfCapital', 'ShareSplit', 'BonusIssue', 'RightsIssue')),
    listing_id        INTEGER NOT NULL REFERENCES listings(id),
    -- ReturnOfCapital: payment date — parcels acquired on/before it are affected.
    -- ShareSplit: conversion date — parcels acquired before it are converted
    -- (a trade dated on the conversion date is already in post-split units).
    -- BonusIssue: issue date — parcels acquired before it receive bonus units
    -- (a trade dated on the issue date is ex-bonus).
    -- RightsIssue: record date — units held before it (a trade dated on it is
    -- ex-rights) earn the entitlement; the exercise happens on a later date.
    date              TEXT    NOT NULL,
    -- ReturnOfCapital only: per-unit payment amount in `currency` (TEXT
    -- decimal, must be positive).
    amount_per_unit   TEXT,
    -- ReturnOfCapital: the payment's currency. RightsIssue: the exercise
    -- price's currency.
    currency          TEXT    REFERENCES currencies(code),
    -- ShareSplit only: every split_old_units existing units become
    -- split_new_units units (TEXT decimals, must be positive).
    split_new_units   TEXT,
    split_old_units   TEXT,
    -- BonusIssue only: every bonus_held_units units held receive bonus_units
    -- additional units (TEXT decimals, must be positive).
    bonus_units       TEXT,
    bonus_held_units  TEXT,
    -- RightsIssue only: every rights_held_units units held entitle the holder
    -- to rights_units new units at exercise_price per unit in `currency`
    -- (TEXT decimals, must be positive).
    rights_units      TEXT,
    rights_held_units TEXT,
    exercise_price    TEXT,
    CHECK (action_type <> 'ReturnOfCapital'
           OR (amount_per_unit IS NOT NULL AND currency IS NOT NULL
               AND split_new_units IS NULL AND split_old_units IS NULL
               AND bonus_units IS NULL AND bonus_held_units IS NULL
               AND rights_units IS NULL AND rights_held_units IS NULL
               AND exercise_price IS NULL)),
    CHECK (action_type <> 'ShareSplit'
           OR (split_new_units IS NOT NULL AND split_old_units IS NOT NULL
               AND amount_per_unit IS NULL AND currency IS NULL
               AND bonus_units IS NULL AND bonus_held_units IS NULL
               AND rights_units IS NULL AND rights_held_units IS NULL
               AND exercise_price IS NULL)),
    CHECK (action_type <> 'BonusIssue'
           OR (bonus_units IS NOT NULL AND bonus_held_units IS NOT NULL
               AND amount_per_unit IS NULL AND currency IS NULL
               AND split_new_units IS NULL AND split_old_units IS NULL
               AND rights_units IS NULL AND rights_held_units IS NULL
               AND exercise_price IS NULL)),
    CHECK (action_type <> 'RightsIssue'
           OR (rights_units IS NOT NULL AND rights_held_units IS NOT NULL
               AND exercise_price IS NOT NULL AND currency IS NOT NULL
               AND amount_per_unit IS NULL
               AND split_new_units IS NULL AND split_old_units IS NULL
               AND bonus_units IS NULL AND bonus_held_units IS NULL))
);

INSERT INTO corporate_actions
    (id, action_type, listing_id, date, amount_per_unit, currency,
     split_new_units, split_old_units, bonus_units, bonus_held_units)
SELECT id, action_type, listing_id, date, amount_per_unit, currency,
       split_new_units, split_old_units, bonus_units, bonus_held_units
FROM corporate_actions_old;

DROP TABLE corporate_actions_old;

-- The provenance link from a rights-exercise Buy trade back to its
-- RightsIssue action: set only by POST /corporate_actions/:id/exercise (NULL
-- for every other trade). Drives the entitlement cap (cumulative exercised
-- units may not exceed the holding's entitlement) and the write-integrity
-- restrictions: a trade carrying it is rejected by PUT /trades, and the
-- referenced action cannot be edited or deleted while the trade exists.
ALTER TABLE trades ADD COLUMN rights_action_id INTEGER REFERENCES corporate_actions(id);
