-- Worthless / delisted shares (docs/ato/worthless-shares.md, QC 52234, TD
-- 2000/52, TD 2000/7): an eighth corporate action type, 'WorthlessShares'. A
-- failed company's shares can produce a capital loss without an ordinary sale,
-- either when a liquidator/administrator declares them worthless (CGT event G3,
-- s 104-145) or when the company is deregistered (CGT event C2, s 104-25). The
-- action records which event the user is invoking (worthless_event:
-- 'G3Declaration' vs 'C2Cancellation') against the failed listing; the event
-- `date` is the declaration / cancellation date. Recording it changes nothing
-- by itself; recognising — POST /corporate_actions/:id/recognise — atomically
-- closes every open parcel held at the event date through a provenance-marked
-- Sell at NIL proceeds, each parcel producing a capital loss equal to its
-- remaining reduced cost base. UNLIKE the scrip-for-scrip and demerger closing
-- Sells (which the rollover excludes from the gains reports), this closing Sell
-- is NOT excluded: its nil proceeds against the cost base *recognise* the loss,
-- which flows through the realised-gains and net-capital-gain reports like any
-- realised loss (never discounted).
--
-- SQLite cannot widen a CHECK in place, so corporate_actions is rebuilt via the
-- rename pattern (as 0010–0015 did). Because foreign keys are enabled,
-- ALTER TABLE ... RENAME rewrites the REFERENCES clauses of referencing tables
-- to follow the rename, so trades follows corporate_actions, and renaming
-- trades drags parcel_allocations, amit_adjustments, income, and attachments
-- with it — the whole FK-connected cluster is rebuilt. No data is dropped:
-- every copy is a straight column-for-column SELECT (TEXT decimals stay TEXT,
-- no CAST), and DROP TABLE only ever touches a renamed _old table.
--
-- This is the first corporate-action migration after 0019 added the
-- report-snapshot staleness triggers, so the triggers on the rebuilt tables
-- (corporate_actions, trades, income, parcel_allocations, amit_adjustments)
-- move to the _old tables on rename and are dropped with them — they are
-- recreated verbatim at the end of this migration.
--
-- New columns added while the tables are open:
--   corporate_actions.worthless_event — 'G3Declaration' | 'C2Cancellation'
--                               (NULL for every other action type).
--   trades.worthless_action_id — set only by the recognise operation on the
--                               closing Sell; freezes the action and makes the
--                               Sell immutable (deleted via DELETE /sells).

-- 1. corporate_actions: widened action_type enum + the WorthlessShares column.
ALTER TABLE corporate_actions RENAME TO corporate_actions_old;

CREATE TABLE corporate_actions (
    id                INTEGER PRIMARY KEY,
    action_type       TEXT    NOT NULL
        CHECK (action_type IN ('ReturnOfCapital', 'ShareSplit', 'BonusIssue', 'RightsIssue',
                               'BuyBack', 'ScripForScrip', 'Demerger', 'WorthlessShares')),
    listing_id        INTEGER NOT NULL REFERENCES listings(id),
    -- ReturnOfCapital: payment date. ShareSplit: conversion date. BonusIssue:
    -- issue date. RightsIssue: record date (exercise is later). BuyBack: the
    -- buy-back date. ScripForScrip: exchange date. Demerger: demerger date.
    -- WorthlessShares: the declaration date (G3) or deregistration/cancellation
    -- date (C2) — every parcel of listing_id still open on it is closed.
    date              TEXT    NOT NULL,
    -- ReturnOfCapital only: per-unit payment amount in `currency`.
    amount_per_unit   TEXT,
    -- ReturnOfCapital / RightsIssue / BuyBack: the relevant amount's currency.
    currency          TEXT    REFERENCES currencies(code),
    -- ShareSplit only.
    split_new_units   TEXT,
    split_old_units   TEXT,
    -- BonusIssue only.
    bonus_units       TEXT,
    bonus_held_units  TEXT,
    -- RightsIssue only.
    rights_units      TEXT,
    rights_held_units TEXT,
    exercise_price    TEXT,
    -- BuyBack only.
    buyback_price           TEXT,
    buyback_dividend        TEXT,
    buyback_franking_credit TEXT,
    buyback_market_value    TEXT,
    -- ScripForScrip only.
    scrip_listing_id  INTEGER REFERENCES listings(id),
    scrip_new_units   TEXT,
    scrip_old_units   TEXT,
    -- Demerger only.
    demerger_listing_id   INTEGER REFERENCES listings(id),
    demerger_new_units    TEXT,
    demerger_held_units   TEXT,
    demerger_cost_base_pct TEXT,
    -- WorthlessShares only: which CGT event the user is invoking. Both produce
    -- the same loss arithmetic (close every open parcel at nil proceeds); the
    -- discriminator records the legal basis (G3 declaration vs C2
    -- deregistration) for the user's records and the recognise operation's
    -- description. CHECK-constrained enum; NULL for every other action type.
    worthless_event   TEXT    CHECK (worthless_event IN ('G3Declaration', 'C2Cancellation')),
    CHECK (action_type <> 'ReturnOfCapital'
           OR (amount_per_unit IS NOT NULL AND currency IS NOT NULL
               AND split_new_units IS NULL AND split_old_units IS NULL
               AND bonus_units IS NULL AND bonus_held_units IS NULL
               AND rights_units IS NULL AND rights_held_units IS NULL
               AND exercise_price IS NULL
               AND buyback_price IS NULL AND buyback_dividend IS NULL
               AND buyback_franking_credit IS NULL AND buyback_market_value IS NULL
               AND scrip_listing_id IS NULL AND scrip_new_units IS NULL
               AND scrip_old_units IS NULL
               AND demerger_listing_id IS NULL AND demerger_new_units IS NULL
               AND demerger_held_units IS NULL AND demerger_cost_base_pct IS NULL
               AND worthless_event IS NULL)),
    CHECK (action_type <> 'ShareSplit'
           OR (split_new_units IS NOT NULL AND split_old_units IS NOT NULL
               AND amount_per_unit IS NULL AND currency IS NULL
               AND bonus_units IS NULL AND bonus_held_units IS NULL
               AND rights_units IS NULL AND rights_held_units IS NULL
               AND exercise_price IS NULL
               AND buyback_price IS NULL AND buyback_dividend IS NULL
               AND buyback_franking_credit IS NULL AND buyback_market_value IS NULL
               AND scrip_listing_id IS NULL AND scrip_new_units IS NULL
               AND scrip_old_units IS NULL
               AND demerger_listing_id IS NULL AND demerger_new_units IS NULL
               AND demerger_held_units IS NULL AND demerger_cost_base_pct IS NULL
               AND worthless_event IS NULL)),
    CHECK (action_type <> 'BonusIssue'
           OR (bonus_units IS NOT NULL AND bonus_held_units IS NOT NULL
               AND amount_per_unit IS NULL AND currency IS NULL
               AND split_new_units IS NULL AND split_old_units IS NULL
               AND rights_units IS NULL AND rights_held_units IS NULL
               AND exercise_price IS NULL
               AND buyback_price IS NULL AND buyback_dividend IS NULL
               AND buyback_franking_credit IS NULL AND buyback_market_value IS NULL
               AND scrip_listing_id IS NULL AND scrip_new_units IS NULL
               AND scrip_old_units IS NULL
               AND demerger_listing_id IS NULL AND demerger_new_units IS NULL
               AND demerger_held_units IS NULL AND demerger_cost_base_pct IS NULL
               AND worthless_event IS NULL)),
    CHECK (action_type <> 'RightsIssue'
           OR (rights_units IS NOT NULL AND rights_held_units IS NOT NULL
               AND exercise_price IS NOT NULL AND currency IS NOT NULL
               AND amount_per_unit IS NULL
               AND split_new_units IS NULL AND split_old_units IS NULL
               AND bonus_units IS NULL AND bonus_held_units IS NULL
               AND buyback_price IS NULL AND buyback_dividend IS NULL
               AND buyback_franking_credit IS NULL AND buyback_market_value IS NULL
               AND scrip_listing_id IS NULL AND scrip_new_units IS NULL
               AND scrip_old_units IS NULL
               AND demerger_listing_id IS NULL AND demerger_new_units IS NULL
               AND demerger_held_units IS NULL AND demerger_cost_base_pct IS NULL
               AND worthless_event IS NULL)),
    CHECK (action_type <> 'BuyBack'
           OR (buyback_price IS NOT NULL AND buyback_dividend IS NOT NULL
               AND buyback_franking_credit IS NOT NULL AND currency IS NOT NULL
               AND amount_per_unit IS NULL
               AND split_new_units IS NULL AND split_old_units IS NULL
               AND bonus_units IS NULL AND bonus_held_units IS NULL
               AND rights_units IS NULL AND rights_held_units IS NULL
               AND exercise_price IS NULL
               AND scrip_listing_id IS NULL AND scrip_new_units IS NULL
               AND scrip_old_units IS NULL
               AND demerger_listing_id IS NULL AND demerger_new_units IS NULL
               AND demerger_held_units IS NULL AND demerger_cost_base_pct IS NULL
               AND worthless_event IS NULL)),
    CHECK (action_type <> 'ScripForScrip'
           OR (scrip_listing_id IS NOT NULL AND scrip_listing_id <> listing_id
               AND scrip_new_units IS NOT NULL AND scrip_old_units IS NOT NULL
               AND amount_per_unit IS NULL AND currency IS NULL
               AND split_new_units IS NULL AND split_old_units IS NULL
               AND bonus_units IS NULL AND bonus_held_units IS NULL
               AND rights_units IS NULL AND rights_held_units IS NULL
               AND exercise_price IS NULL
               AND buyback_price IS NULL AND buyback_dividend IS NULL
               AND buyback_franking_credit IS NULL AND buyback_market_value IS NULL
               AND demerger_listing_id IS NULL AND demerger_new_units IS NULL
               AND demerger_held_units IS NULL AND demerger_cost_base_pct IS NULL
               AND worthless_event IS NULL)),
    CHECK (action_type <> 'Demerger'
           OR (demerger_listing_id IS NOT NULL AND demerger_listing_id <> listing_id
               AND demerger_new_units IS NOT NULL AND demerger_held_units IS NOT NULL
               AND demerger_cost_base_pct IS NOT NULL
               AND amount_per_unit IS NULL AND currency IS NULL
               AND split_new_units IS NULL AND split_old_units IS NULL
               AND bonus_units IS NULL AND bonus_held_units IS NULL
               AND rights_units IS NULL AND rights_held_units IS NULL
               AND exercise_price IS NULL
               AND buyback_price IS NULL AND buyback_dividend IS NULL
               AND buyback_franking_credit IS NULL AND buyback_market_value IS NULL
               AND scrip_listing_id IS NULL AND scrip_new_units IS NULL
               AND scrip_old_units IS NULL
               AND worthless_event IS NULL)),
    -- WorthlessShares: only the event discriminator; every numeric payload NULL.
    CHECK (action_type <> 'WorthlessShares'
           OR (worthless_event IS NOT NULL
               AND amount_per_unit IS NULL AND currency IS NULL
               AND split_new_units IS NULL AND split_old_units IS NULL
               AND bonus_units IS NULL AND bonus_held_units IS NULL
               AND rights_units IS NULL AND rights_held_units IS NULL
               AND exercise_price IS NULL
               AND buyback_price IS NULL AND buyback_dividend IS NULL
               AND buyback_franking_credit IS NULL AND buyback_market_value IS NULL
               AND scrip_listing_id IS NULL AND scrip_new_units IS NULL
               AND scrip_old_units IS NULL
               AND demerger_listing_id IS NULL AND demerger_new_units IS NULL
               AND demerger_held_units IS NULL AND demerger_cost_base_pct IS NULL))
);

INSERT INTO corporate_actions
    (id, action_type, listing_id, date, amount_per_unit, currency,
     split_new_units, split_old_units, bonus_units, bonus_held_units,
     rights_units, rights_held_units, exercise_price,
     buyback_price, buyback_dividend, buyback_franking_credit, buyback_market_value,
     scrip_listing_id, scrip_new_units, scrip_old_units,
     demerger_listing_id, demerger_new_units, demerger_held_units, demerger_cost_base_pct)
SELECT id, action_type, listing_id, date, amount_per_unit, currency,
       split_new_units, split_old_units, bonus_units, bonus_held_units,
       rights_units, rights_held_units, exercise_price,
       buyback_price, buyback_dividend, buyback_franking_credit, buyback_market_value,
       scrip_listing_id, scrip_new_units, scrip_old_units,
       demerger_listing_id, demerger_new_units, demerger_held_units, demerger_cost_base_pct
FROM corporate_actions_old;

-- 2. trades: its *_action_id REFERENCES clauses followed the rename above, so
-- rebuild it pointing at the new corporate_actions; add worthless_action_id.
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
    rights_action_id    INTEGER REFERENCES corporate_actions(id),
    buyback_action_id   INTEGER REFERENCES corporate_actions(id),
    scrip_action_id     INTEGER REFERENCES corporate_actions(id),
    demerger_action_id  INTEGER REFERENCES corporate_actions(id),
    deemed_acquisition_date TEXT,
    holding_account_id  INTEGER NOT NULL DEFAULT 1 REFERENCES holding_accounts(id),
    transfer_id         INTEGER REFERENCES transfers(id),
    brokerage_includes_gst INTEGER NOT NULL DEFAULT 0
        CHECK (brokerage_includes_gst IN (0, 1)),
    statement_total     TEXT,
    ess_statement_id    INTEGER REFERENCES ess_statements(id),
    -- Provenance link from a worthless-shares closing Sell back to its
    -- WorthlessShares action: set only by POST /corporate_actions/:id/recognise
    -- (NULL for every other trade). The Sell carrying it is rejected by
    -- PUT /sells and PUT/DELETE /trades; DELETE /sells on it removes it and
    -- restores the holding, and the action is frozen while it exists. Unlike
    -- the rollover provenance columns, a Sell carrying it IS counted by the
    -- realised-gains report (its nil proceeds recognise the capital loss).
    worthless_action_id INTEGER REFERENCES corporate_actions(id)
);

INSERT INTO trades
    (id, trade_type, date, settlement_date, listing_id, average_price, quantity,
     currency, brokerage, gst_on_brokerage, brokerage_currency, fx_rate,
     contract_note_ref, residual_brought_forward, residual_carried_forward,
     residual_paid_out, rights_action_id, buyback_action_id, scrip_action_id,
     demerger_action_id, deemed_acquisition_date, holding_account_id, transfer_id,
     brokerage_includes_gst, statement_total, ess_statement_id)
SELECT id, trade_type, date, settlement_date, listing_id, average_price, quantity,
       currency, brokerage, gst_on_brokerage, brokerage_currency, fx_rate,
       contract_note_ref, residual_brought_forward, residual_carried_forward,
       residual_paid_out, rights_action_id, buyback_action_id, scrip_action_id,
       demerger_action_id, deemed_acquisition_date, holding_account_id, transfer_id,
       brokerage_includes_gst, statement_total, ess_statement_id
FROM trades_old;

-- 3. income: its reinvestment_trade_id/buyback_trade_id followed the trades
-- rename; rebuild it pointing at the new trades.
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
    buyback_trade_id            INTEGER REFERENCES trades(id),
    holding_account_id          INTEGER NOT NULL DEFAULT 1 REFERENCES holding_accounts(id),
    amount_per_security         TEXT,
    securities_held             TEXT
);

INSERT INTO income
    (id, listing_id, date_paid, ex_date, franked_amount, unfranked_amount,
     foreign_source_income, foreign_tax_paid, tfn_withholding_tax,
     franking_credits, lic_capital_gain_deduction, conduit_foreign_income,
     trust_income, reinvestment_trade_id, currency, buyback_trade_id,
     holding_account_id, amount_per_security, securities_held)
SELECT id, listing_id, date_paid, ex_date, franked_amount, unfranked_amount,
       foreign_source_income, foreign_tax_paid, tfn_withholding_tax,
       franking_credits, lic_capital_gain_deduction, conduit_foreign_income,
       trust_income, reinvestment_trade_id, currency, buyback_trade_id,
       holding_account_id, amount_per_security, securities_held
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
-- rebuild.
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

-- 7. Drop the renamed parents, children first (every table that referenced them
-- has been rebuilt against the new tables or dropped above).
DROP TABLE income_old;
DROP TABLE trades_old;
DROP TABLE corporate_actions_old;

-- 8. Recreate the report-snapshot staleness triggers (0019) on the rebuilt
-- tables: the originals moved to the _old tables on rename and were dropped
-- with them above. The new tables had no triggers during the bulk
-- INSERT...SELECT copies (so no spurious staleness), and all _old tables are
-- now gone, freeing the trigger names.
CREATE TRIGGER corporate_actions_stale_snapshots_insert AFTER INSERT ON corporate_actions
BEGIN
    UPDATE report_snapshots SET stale = 1 WHERE snapshot_date >= NEW.date;
END;
CREATE TRIGGER corporate_actions_stale_snapshots_update AFTER UPDATE ON corporate_actions
BEGIN
    UPDATE report_snapshots SET stale = 1
    WHERE snapshot_date >= MIN(OLD.date, NEW.date);
END;
CREATE TRIGGER corporate_actions_stale_snapshots_delete AFTER DELETE ON corporate_actions
BEGIN
    UPDATE report_snapshots SET stale = 1 WHERE snapshot_date >= OLD.date;
END;

CREATE TRIGGER trades_stale_snapshots_insert AFTER INSERT ON trades
BEGIN
    UPDATE report_snapshots SET stale = 1 WHERE snapshot_date >= NEW.date;
END;
CREATE TRIGGER trades_stale_snapshots_update AFTER UPDATE ON trades
BEGIN
    UPDATE report_snapshots SET stale = 1
    WHERE snapshot_date >= MIN(OLD.date, NEW.date);
END;
CREATE TRIGGER trades_stale_snapshots_delete AFTER DELETE ON trades
BEGIN
    UPDATE report_snapshots SET stale = 1 WHERE snapshot_date >= OLD.date;
END;

CREATE TRIGGER income_stale_snapshots_insert AFTER INSERT ON income
BEGIN
    UPDATE report_snapshots SET stale = 1 WHERE snapshot_date >= NEW.date_paid;
END;
CREATE TRIGGER income_stale_snapshots_update AFTER UPDATE ON income
BEGIN
    UPDATE report_snapshots SET stale = 1
    WHERE snapshot_date >= MIN(OLD.date_paid, NEW.date_paid);
END;
CREATE TRIGGER income_stale_snapshots_delete AFTER DELETE ON income
BEGIN
    UPDATE report_snapshots SET stale = 1 WHERE snapshot_date >= OLD.date_paid;
END;

CREATE TRIGGER parcel_allocations_stale_snapshots_insert AFTER INSERT ON parcel_allocations
BEGIN
    UPDATE report_snapshots SET stale = 1 WHERE snapshot_date >=
        (SELECT date FROM trades WHERE id = NEW.sale_trade_id);
END;
CREATE TRIGGER parcel_allocations_stale_snapshots_update AFTER UPDATE ON parcel_allocations
BEGIN
    UPDATE report_snapshots SET stale = 1 WHERE snapshot_date >=
        MIN((SELECT date FROM trades WHERE id = OLD.sale_trade_id),
            (SELECT date FROM trades WHERE id = NEW.sale_trade_id));
END;
CREATE TRIGGER parcel_allocations_stale_snapshots_delete AFTER DELETE ON parcel_allocations
BEGIN
    UPDATE report_snapshots SET stale = 1 WHERE snapshot_date >=
        (SELECT date FROM trades WHERE id = OLD.sale_trade_id);
END;

CREATE TRIGGER amit_adjustments_stale_snapshots_insert AFTER INSERT ON amit_adjustments
BEGIN
    UPDATE report_snapshots SET stale = 1 WHERE snapshot_date >=
        (SELECT tax_year_end_date FROM amma_statements WHERE id = NEW.amma_statement_id);
END;
CREATE TRIGGER amit_adjustments_stale_snapshots_update AFTER UPDATE ON amit_adjustments
BEGIN
    UPDATE report_snapshots SET stale = 1 WHERE snapshot_date >=
        MIN((SELECT tax_year_end_date FROM amma_statements WHERE id = OLD.amma_statement_id),
            (SELECT tax_year_end_date FROM amma_statements WHERE id = NEW.amma_statement_id));
END;
CREATE TRIGGER amit_adjustments_stale_snapshots_delete AFTER DELETE ON amit_adjustments
BEGIN
    UPDATE report_snapshots SET stale = 1 WHERE snapshot_date >=
        (SELECT tax_year_end_date FROM amma_statements WHERE id = OLD.amma_statement_id);
END;
