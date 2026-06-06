-- Off-market share buy-backs (docs/share-buy-backs.md, QC 66049): a fifth
-- corporate action type, 'BuyBack'. The action records the offer terms per
-- listing: a per-unit `buyback_price`, the per-unit dividend component of that
-- price (`buyback_dividend`, with `buyback_franking_credit` attached — both 0
-- for a listed-company buy-back announced after 7:30 pm AEDT 25 Oct 2022,
-- which has no dividend component), and optionally `buyback_market_value`
-- (the per-unit market value had the buy-back not been proposed — capital
-- proceeds can't be less than it). `currency` is shared with ReturnOfCapital
-- and RightsIssue. Recording the action changes nothing by itself;
-- participating — POST /corporate_actions/:id/participate — atomically
-- creates the Sell trade (per-unit price = capital proceeds per unit =
-- max(price, market value) − dividend) with its parcel allocations, plus an
-- income row for the dividend component when there is one.
--
-- SQLite cannot widen a CHECK in place, so corporate_actions is rebuilt via
-- the rename pattern. Because foreign keys are enabled, ALTER TABLE ... RENAME
-- rewrites the REFERENCES clauses of referencing tables to follow the rename
-- (the NOTE in 0012): trades.rights_action_id follows corporate_actions to its
-- _old name, so trades must be rebuilt too — and renaming trades drags the
-- REFERENCES of parcel_allocations, amit_adjustments, income, and attachments
-- with it, so the whole FK-connected cluster is rebuilt (the same pattern the
-- pre-consolidation migration 0017 used). No data is dropped: every copy is a
-- straight column-for-column SELECT (TEXT decimals stay TEXT, no CAST), and
-- DROP TABLE only ever touches a renamed _old table.
--
-- New provenance columns added while the tables are open:
--   trades.buyback_action_id  — set only by the participate operation; links a
--                               buy-back Sell to its BuyBack action, freezes
--                               the action and makes the Sell immutable via
--                               PUT /sells (delete and re-participate instead).
--   income.buyback_trade_id   — links the dividend-component income row to its
--                               buy-back Sell; such a row is managed by the
--                               participation (rejected by PUT/DELETE /income,
--                               removed when the Sell is deleted via /sells).

-- 1. corporate_actions: widened action_type enum + the BuyBack payload columns.
ALTER TABLE corporate_actions RENAME TO corporate_actions_old;

CREATE TABLE corporate_actions (
    id                INTEGER PRIMARY KEY,
    action_type       TEXT    NOT NULL
        CHECK (action_type IN ('ReturnOfCapital', 'ShareSplit', 'BonusIssue', 'RightsIssue',
                               'BuyBack')),
    listing_id        INTEGER NOT NULL REFERENCES listings(id),
    -- ReturnOfCapital: payment date — parcels acquired on/before it are affected.
    -- ShareSplit: conversion date — parcels acquired before it are converted
    -- (a trade dated on the conversion date is already in post-split units).
    -- BonusIssue: issue date — parcels acquired before it receive bonus units
    -- (a trade dated on the issue date is ex-bonus).
    -- RightsIssue: record date — units held before it (a trade dated on it is
    -- ex-rights) earn the entitlement; the exercise happens on a later date.
    -- BuyBack: the buy-back date (offer approval); participations are dated
    -- on/after it.
    date              TEXT    NOT NULL,
    -- ReturnOfCapital only: per-unit payment amount in `currency` (TEXT
    -- decimal, must be positive).
    amount_per_unit   TEXT,
    -- ReturnOfCapital: the payment's currency. RightsIssue: the exercise
    -- price's currency. BuyBack: the buy-back price's currency.
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
    -- BuyBack only (TEXT decimals): per-unit buy-back price (positive), the
    -- per-unit dividend component of that price and its attached franking
    -- credit (both non-negative; dividend ≤ price; 0 for a listed-company
    -- buy-back announced after 25 Oct 2022), and the per-unit market value
    -- had the buy-back not been proposed (positive; NULL when the buy-back
    -- price is at or above market value, in which case the price itself is
    -- used). Capital proceeds per unit = max(price, market value) − dividend.
    buyback_price           TEXT,
    buyback_dividend        TEXT,
    buyback_franking_credit TEXT,
    buyback_market_value    TEXT,
    CHECK (action_type <> 'ReturnOfCapital'
           OR (amount_per_unit IS NOT NULL AND currency IS NOT NULL
               AND split_new_units IS NULL AND split_old_units IS NULL
               AND bonus_units IS NULL AND bonus_held_units IS NULL
               AND rights_units IS NULL AND rights_held_units IS NULL
               AND exercise_price IS NULL
               AND buyback_price IS NULL AND buyback_dividend IS NULL
               AND buyback_franking_credit IS NULL AND buyback_market_value IS NULL)),
    CHECK (action_type <> 'ShareSplit'
           OR (split_new_units IS NOT NULL AND split_old_units IS NOT NULL
               AND amount_per_unit IS NULL AND currency IS NULL
               AND bonus_units IS NULL AND bonus_held_units IS NULL
               AND rights_units IS NULL AND rights_held_units IS NULL
               AND exercise_price IS NULL
               AND buyback_price IS NULL AND buyback_dividend IS NULL
               AND buyback_franking_credit IS NULL AND buyback_market_value IS NULL)),
    CHECK (action_type <> 'BonusIssue'
           OR (bonus_units IS NOT NULL AND bonus_held_units IS NOT NULL
               AND amount_per_unit IS NULL AND currency IS NULL
               AND split_new_units IS NULL AND split_old_units IS NULL
               AND rights_units IS NULL AND rights_held_units IS NULL
               AND exercise_price IS NULL
               AND buyback_price IS NULL AND buyback_dividend IS NULL
               AND buyback_franking_credit IS NULL AND buyback_market_value IS NULL)),
    CHECK (action_type <> 'RightsIssue'
           OR (rights_units IS NOT NULL AND rights_held_units IS NOT NULL
               AND exercise_price IS NOT NULL AND currency IS NOT NULL
               AND amount_per_unit IS NULL
               AND split_new_units IS NULL AND split_old_units IS NULL
               AND bonus_units IS NULL AND bonus_held_units IS NULL
               AND buyback_price IS NULL AND buyback_dividend IS NULL
               AND buyback_franking_credit IS NULL AND buyback_market_value IS NULL)),
    CHECK (action_type <> 'BuyBack'
           OR (buyback_price IS NOT NULL AND buyback_dividend IS NOT NULL
               AND buyback_franking_credit IS NOT NULL AND currency IS NOT NULL
               AND amount_per_unit IS NULL
               AND split_new_units IS NULL AND split_old_units IS NULL
               AND bonus_units IS NULL AND bonus_held_units IS NULL
               AND rights_units IS NULL AND rights_held_units IS NULL
               AND exercise_price IS NULL))
);

INSERT INTO corporate_actions
    (id, action_type, listing_id, date, amount_per_unit, currency,
     split_new_units, split_old_units, bonus_units, bonus_held_units,
     rights_units, rights_held_units, exercise_price)
SELECT id, action_type, listing_id, date, amount_per_unit, currency,
       split_new_units, split_old_units, bonus_units, bonus_held_units,
       rights_units, rights_held_units, exercise_price
FROM corporate_actions_old;

-- 2. trades: its rights_action_id REFERENCES clause followed the rename above,
-- so rebuild it pointing at the new corporate_actions; add buyback_action_id.
ALTER TABLE trades RENAME TO trades_old;

CREATE TABLE trades (
    id                  INTEGER PRIMARY KEY,
    trade_type          TEXT    NOT NULL CHECK(trade_type IN ('Buy', 'Sell', 'DRP')),
    date                TEXT    NOT NULL,
    settlement_date     TEXT    NOT NULL,
    listing_id          INTEGER NOT NULL REFERENCES listings(id),
    average_price       TEXT    NOT NULL,
    quantity            TEXT    NOT NULL,
    currency            TEXT    NOT NULL REFERENCES currencies(code),
    brokerage           TEXT    NOT NULL DEFAULT '0',
    gst_on_brokerage    TEXT    NOT NULL DEFAULT '0',
    brokerage_currency  TEXT    NOT NULL REFERENCES currencies(code),
    fx_rate             TEXT    NOT NULL DEFAULT '1',
    contract_note_ref   TEXT,
    residual_brought_forward TEXT NOT NULL DEFAULT '0',
    residual_carried_forward TEXT NOT NULL DEFAULT '0',
    residual_paid_out        TEXT NOT NULL DEFAULT '0',
    -- Provenance link from a rights-exercise Buy trade back to its RightsIssue
    -- action: set only by POST /corporate_actions/:id/exercise (see 0012).
    rights_action_id    INTEGER REFERENCES corporate_actions(id),
    -- Provenance link from a buy-back Sell trade back to its BuyBack action:
    -- set only by POST /corporate_actions/:id/participate (NULL for every
    -- other trade). A trade carrying it is rejected by PUT /sells (delete via
    -- DELETE /sells — which also removes the linked dividend income row — and
    -- re-participate instead), and the referenced action cannot be edited or
    -- deleted while the trade exists.
    buyback_action_id   INTEGER REFERENCES corporate_actions(id)
);

INSERT INTO trades
    (id, trade_type, date, settlement_date, listing_id, average_price, quantity,
     currency, brokerage, gst_on_brokerage, brokerage_currency, fx_rate,
     contract_note_ref, residual_brought_forward, residual_carried_forward,
     residual_paid_out, rights_action_id)
SELECT id, trade_type, date, settlement_date, listing_id, average_price, quantity,
       currency, brokerage, gst_on_brokerage, brokerage_currency, fx_rate,
       contract_note_ref, residual_brought_forward, residual_carried_forward,
       residual_paid_out, rights_action_id
FROM trades_old;

-- 3. income: its reinvestment_trade_id followed the trades rename; rebuild it
-- pointing at the new trades and add buyback_trade_id.
ALTER TABLE income RENAME TO income_old;

CREATE TABLE income (
    id                          INTEGER PRIMARY KEY,
    listing_id                  INTEGER NOT NULL REFERENCES listings(id),
    date_paid                   TEXT    NOT NULL,
    ex_date                     TEXT,
    franked_amount              TEXT    NOT NULL DEFAULT '0',
    unfranked_amount            TEXT    NOT NULL DEFAULT '0',
    foreign_source_income       TEXT    NOT NULL DEFAULT '0',
    foreign_tax_paid            TEXT    NOT NULL DEFAULT '0',
    tfn_withholding_tax         TEXT    NOT NULL DEFAULT '0',
    franking_credits            TEXT    NOT NULL DEFAULT '0',
    lic_capital_gain_deduction  TEXT    NOT NULL DEFAULT '0',
    conduit_foreign_income      TEXT    NOT NULL DEFAULT '0',
    trust_income                INTEGER NOT NULL DEFAULT 0,
    reinvestment_trade_id       INTEGER REFERENCES trades(id),
    currency                    TEXT    NOT NULL DEFAULT 'AUD' REFERENCES currencies(code),
    -- Provenance link from a buy-back dividend-component income row to the
    -- buy-back Sell trade it was created with: set only by the participate
    -- operation (NULL for every other row). A row carrying it is managed by
    -- the participation — PUT/DELETE /income reject it; it is removed when
    -- the Sell is deleted via DELETE /sells.
    buyback_trade_id            INTEGER REFERENCES trades(id)
);

INSERT INTO income
    (id, listing_id, date_paid, ex_date, franked_amount, unfranked_amount,
     foreign_source_income, foreign_tax_paid, tfn_withholding_tax,
     franking_credits, lic_capital_gain_deduction, conduit_foreign_income,
     trust_income, reinvestment_trade_id, currency)
SELECT id, listing_id, date_paid, ex_date, franked_amount, unfranked_amount,
       foreign_source_income, foreign_tax_paid, tfn_withholding_tax,
       franking_credits, lic_capital_gain_deduction, conduit_foreign_income,
       trust_income, reinvestment_trade_id, currency
FROM income_old;

-- 4. parcel_allocations: both trade FKs followed the trades rename; rebuild.
ALTER TABLE parcel_allocations RENAME TO parcel_allocations_old;

CREATE TABLE parcel_allocations (
    id                INTEGER PRIMARY KEY,
    sale_trade_id     INTEGER NOT NULL REFERENCES trades(id),
    purchase_trade_id INTEGER NOT NULL REFERENCES trades(id),
    quantity_allocated TEXT    NOT NULL
);

INSERT INTO parcel_allocations (id, sale_trade_id, purchase_trade_id, quantity_allocated)
SELECT id, sale_trade_id, purchase_trade_id, quantity_allocated
FROM parcel_allocations_old;

DROP TABLE parcel_allocations_old;

-- 5. amit_adjustments: its trade_id followed the trades rename; rebuild.
ALTER TABLE amit_adjustments RENAME TO amit_adjustments_old;

CREATE TABLE amit_adjustments (
    id                 INTEGER PRIMARY KEY,
    amma_statement_id  INTEGER NOT NULL REFERENCES amma_statements(id),
    trade_id           INTEGER NOT NULL REFERENCES trades(id),
    quantity           TEXT    NOT NULL
);

INSERT INTO amit_adjustments (id, amma_statement_id, trade_id, quantity)
SELECT id, amma_statement_id, trade_id, quantity
FROM amit_adjustments_old;

DROP TABLE amit_adjustments_old;

-- 6. attachments: its trade_id/income_id followed the trades/income renames;
-- rebuild (see 0004 for the column semantics).
ALTER TABLE attachments RENAME TO attachments_old;

CREATE TABLE attachments (
    id                INTEGER PRIMARY KEY,
    trade_id          INTEGER REFERENCES trades(id) ON DELETE CASCADE,
    income_id         INTEGER REFERENCES income(id) ON DELETE CASCADE,
    amma_statement_id INTEGER REFERENCES amma_statements(id) ON DELETE CASCADE,
    filename          TEXT    NOT NULL,
    content_type      TEXT    NOT NULL CHECK(content_type IN ('application/pdf', 'image/png', 'image/jpeg')),
    byte_size         INTEGER NOT NULL,
    checksum          TEXT    NOT NULL,
    uploaded_at       TEXT    NOT NULL,
    content           BLOB    NOT NULL,
    CHECK ((trade_id IS NOT NULL) + (income_id IS NOT NULL) + (amma_statement_id IS NOT NULL) = 1)
);

INSERT INTO attachments
    (id, trade_id, income_id, amma_statement_id, filename, content_type,
     byte_size, checksum, uploaded_at, content)
SELECT id, trade_id, income_id, amma_statement_id, filename, content_type,
       byte_size, checksum, uploaded_at, content
FROM attachments_old;

DROP TABLE attachments_old;

-- 7. Drop the renamed parents, children first (every table that referenced
-- them has been rebuilt against the new tables or dropped above).
DROP TABLE income_old;
DROP TABLE trades_old;
DROP TABLE corporate_actions_old;
