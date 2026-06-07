-- Crypto-asset holdings (REQUIREMENTS "New Requirements — Crypto-asset
-- holdings"): the ATO treats investment crypto as a CGT asset
-- (docs/ato/crypto-cgt.md), so the existing parcel-level CGT machinery applies
-- unchanged — the schema work is letting a listing exist without an exchange.
--
-- listings changes:
--   * security_type gains 'Crypto' in its CHECK enum.
--   * exchange_mic becomes nullable, with a CHECK that it is NULL exactly when
--     security_type = 'Crypto' (every other type still requires an exchange).
--   * UNIQUE(exchange_mic, ticker) does not constrain NULL-mic rows (SQLite
--     treats NULLs as distinct), so a partial unique index makes an
--     exchange-less listing unique by ticker.
--
-- SQLite cannot widen a CHECK or drop NOT NULL in place, so listings is
-- rebuilt via the rename pattern. Because foreign keys are enabled,
-- ALTER TABLE ... RENAME rewrites the REFERENCES clauses of referencing tables
-- to follow the rename (the NOTE in 0012): corporate_actions, transfers,
-- trades, amma_statements, income, and drp_enrolments follow listings, and
-- renaming trades/amma_statements/income drags parcel_allocations,
-- amit_adjustments, and attachments with them, so the whole FK-connected
-- cluster is rebuilt (same as 0013/0014/0015/0016). No data is dropped: every
-- copy is a straight column-for-column SELECT (TEXT decimals stay TEXT, no
-- CAST), existing rows migrate forward unchanged, and DROP TABLE only ever
-- touches a renamed _old table.

-- 1. listings: nullable exchange_mic + the widened security_type enum.
ALTER TABLE listings RENAME TO listings_old;

CREATE TABLE listings (
    id            INTEGER PRIMARY KEY,
    -- NULL exactly for Crypto listings: a crypto asset trades on no MIC-coded
    -- venue, settles same-day, and has no holiday calendar.
    exchange_mic  TEXT    REFERENCES exchanges(mic),
    ticker        TEXT    NOT NULL,
    name          TEXT    NOT NULL,
    isin          TEXT,
    security_type TEXT    NOT NULL CHECK(security_type IN ('Share', 'ETF', 'LIC', 'Trust', 'Crypto')),
    currency      TEXT    NOT NULL REFERENCES currencies(code),
    amit          INTEGER NOT NULL DEFAULT 0,
    preference    INTEGER NOT NULL DEFAULT 0,
    UNIQUE(exchange_mic, ticker),
    CHECK ((exchange_mic IS NULL) = (security_type = 'Crypto'))
);

INSERT INTO listings (id, exchange_mic, ticker, name, isin, security_type, currency, amit, preference)
SELECT id, exchange_mic, ticker, name, isin, security_type, currency, amit, preference
FROM listings_old;

-- UNIQUE(exchange_mic, ticker) treats NULL mics as distinct, so exchange-less
-- (Crypto) listings are made unique by ticker here instead.
CREATE UNIQUE INDEX listings_crypto_ticker ON listings(ticker) WHERE exchange_mic IS NULL;

-- 2. corporate_actions: its three listing FKs followed the rename; rebuild
-- against the new listings (see 0015 for the column semantics).
ALTER TABLE corporate_actions RENAME TO corporate_actions_old;

CREATE TABLE corporate_actions (
    id                INTEGER PRIMARY KEY,
    action_type       TEXT    NOT NULL
        CHECK (action_type IN ('ReturnOfCapital', 'ShareSplit', 'BonusIssue', 'RightsIssue',
                               'BuyBack', 'ScripForScrip', 'Demerger')),
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
    -- ScripForScrip: the exchange (implementation) date — every parcel of
    -- listing_id still open on it is exchanged; the closing Sell and the
    -- replacement Buys are dated on it.
    -- Demerger: the demerger (implementation) date — every parcel of
    -- listing_id (the head entity) still open on it participates; the closing
    -- Sell, head replacement Buys, and demerged-entity Buys are dated on it.
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
    -- ScripForScrip only: the replacement listing (must differ from the
    -- original listing_id) and the exchange ratio — every scrip_old_units
    -- units of listing_id held at the exchange date become scrip_new_units
    -- units of scrip_listing_id (TEXT decimals, must be positive).
    scrip_listing_id  INTEGER REFERENCES listings(id),
    scrip_new_units   TEXT,
    scrip_old_units   TEXT,
    -- Demerger only: the demerged entity's listing (must differ from the head
    -- listing_id), the entitlement ratio — every demerger_held_units units of
    -- listing_id held at the demerger date receive demerger_new_units units
    -- of demerger_listing_id — and the percentage of the original cost base
    -- apportioned to the new interests (the head-entity-advised step 2
    -- percentage; TEXT decimals, ratio units positive, 0 < pct < 100).
    demerger_listing_id   INTEGER REFERENCES listings(id),
    demerger_new_units    TEXT,
    demerger_held_units   TEXT,
    demerger_cost_base_pct TEXT,
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
               AND demerger_held_units IS NULL AND demerger_cost_base_pct IS NULL)),
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
               AND demerger_held_units IS NULL AND demerger_cost_base_pct IS NULL)),
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
               AND demerger_held_units IS NULL AND demerger_cost_base_pct IS NULL)),
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
               AND demerger_held_units IS NULL AND demerger_cost_base_pct IS NULL)),
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
               AND demerger_held_units IS NULL AND demerger_cost_base_pct IS NULL)),
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
               AND demerger_held_units IS NULL AND demerger_cost_base_pct IS NULL)),
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
               AND scrip_old_units IS NULL))
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

-- 3. transfers: its listing FK followed the rename; rebuild (see 0016).
ALTER TABLE transfers RENAME TO transfers_old;

CREATE TABLE transfers (
    id              INTEGER PRIMARY KEY,
    listing_id      INTEGER NOT NULL REFERENCES listings(id),
    -- The date the holding moves: the transfer-out Sell and transfer-in Buys
    -- are dated on it.
    date            TEXT    NOT NULL,
    from_account_id INTEGER NOT NULL REFERENCES holding_accounts(id),
    to_account_id   INTEGER NOT NULL REFERENCES holding_accounts(id),
    CHECK (from_account_id <> to_account_id)
);

INSERT INTO transfers (id, listing_id, date, from_account_id, to_account_id)
SELECT id, listing_id, date, from_account_id, to_account_id
FROM transfers_old;

DROP TABLE transfers_old;

-- 4. trades: its listing/corporate-action/transfer FKs followed the renames;
-- rebuild against the new tables (see 0016 for the column semantics).
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
    -- set only by POST /corporate_actions/:id/participate (see 0013).
    buyback_action_id   INTEGER REFERENCES corporate_actions(id),
    -- Provenance link from a scrip-for-scrip closing Sell / replacement Buy
    -- back to its ScripForScrip action: set only by
    -- POST /corporate_actions/:id/exchange (see 0014).
    scrip_action_id     INTEGER REFERENCES corporate_actions(id),
    -- Provenance link from a demerger closing Sell / head replacement Buy /
    -- demerged-entity Buy back to its Demerger action: set only by
    -- POST /corporate_actions/:id/demerge (see 0015).
    demerger_action_id  INTEGER REFERENCES corporate_actions(id),
    -- The CGT acquisition date deemed for this parcel when it differs from
    -- `date`: set on scrip-for-scrip replacement Buys, demerger head/demerged
    -- Buys, and transfer-in Buys, carrying the consumed parcel's acquisition
    -- date (rollovers count the combined holding period; an own-account
    -- transfer is not a disposal at all). Drives the 12-month discount clock
    -- and the AUD translation month of the cost base; NULL = the trade's own
    -- date.
    deemed_acquisition_date TEXT,
    -- The holding account the trade sits in (see 0016).
    holding_account_id  INTEGER NOT NULL DEFAULT 1 REFERENCES holding_accounts(id),
    -- Provenance link from a transfer-out Sell / transfer-in Buy back to its
    -- transfer: set only by PUT /transfers/:id (see 0016).
    transfer_id         INTEGER REFERENCES transfers(id)
);

INSERT INTO trades
    (id, trade_type, date, settlement_date, listing_id, average_price, quantity,
     currency, brokerage, gst_on_brokerage, brokerage_currency, fx_rate,
     contract_note_ref, residual_brought_forward, residual_carried_forward,
     residual_paid_out, rights_action_id, buyback_action_id, scrip_action_id,
     demerger_action_id, deemed_acquisition_date, holding_account_id, transfer_id)
SELECT id, trade_type, date, settlement_date, listing_id, average_price, quantity,
       currency, brokerage, gst_on_brokerage, brokerage_currency, fx_rate,
       contract_note_ref, residual_brought_forward, residual_carried_forward,
       residual_paid_out, rights_action_id, buyback_action_id, scrip_action_id,
       demerger_action_id, deemed_acquisition_date, holding_account_id, transfer_id
FROM trades_old;

-- 5. amma_statements: its listing FK followed the rename; rebuild (see 0016).
ALTER TABLE amma_statements RENAME TO amma_statements_old;

CREATE TABLE amma_statements (
    id                              INTEGER PRIMARY KEY,
    listing_id                      INTEGER NOT NULL REFERENCES listings(id),
    tax_year_end_date               TEXT    NOT NULL,
    units_held                      TEXT    NOT NULL DEFAULT '0',
    date_received                   TEXT    NOT NULL,
    australian_interest             TEXT    NOT NULL DEFAULT '0',
    australian_dividends_unfranked  TEXT    NOT NULL DEFAULT '0',
    franked_dividends               TEXT    NOT NULL DEFAULT '0',
    franking_credits                TEXT    NOT NULL DEFAULT '0',
    net_rent                        TEXT    NOT NULL DEFAULT '0',
    foreign_income                  TEXT    NOT NULL DEFAULT '0',
    foreign_tax_credits             TEXT    NOT NULL DEFAULT '0',
    other_income                    TEXT    NOT NULL DEFAULT '0',
    cgt_discount_gains              TEXT    NOT NULL DEFAULT '0',
    cgt_indexation_gains            TEXT    NOT NULL DEFAULT '0',
    cgt_other_gains                 TEXT    NOT NULL DEFAULT '0',
    capital_losses_applied          TEXT    NOT NULL DEFAULT '0',
    tax_deferred_amount             TEXT    NOT NULL DEFAULT '0',
    tax_free_amount                 TEXT    NOT NULL DEFAULT '0',
    cost_base_adjustment            TEXT    NOT NULL DEFAULT '0',
    tfn_withholding_tax             TEXT    NOT NULL DEFAULT '0',
    currency                        TEXT    NOT NULL DEFAULT 'AUD' REFERENCES currencies(code),
    -- The holding account the statement covers (see 0016).
    holding_account_id              INTEGER NOT NULL DEFAULT 1 REFERENCES holding_accounts(id)
);

INSERT INTO amma_statements
    (id, listing_id, tax_year_end_date, units_held, date_received,
     australian_interest, australian_dividends_unfranked, franked_dividends,
     franking_credits, net_rent, foreign_income, foreign_tax_credits,
     other_income, cgt_discount_gains, cgt_indexation_gains, cgt_other_gains,
     capital_losses_applied, tax_deferred_amount, tax_free_amount,
     cost_base_adjustment, tfn_withholding_tax, currency, holding_account_id)
SELECT id, listing_id, tax_year_end_date, units_held, date_received,
       australian_interest, australian_dividends_unfranked, franked_dividends,
       franking_credits, net_rent, foreign_income, foreign_tax_credits,
       other_income, cgt_discount_gains, cgt_indexation_gains, cgt_other_gains,
       capital_losses_applied, tax_deferred_amount, tax_free_amount,
       cost_base_adjustment, tfn_withholding_tax, currency, holding_account_id
FROM amma_statements_old;

-- 6. income: its listing/trade FKs followed the renames; rebuild (see 0016).
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
    -- The holding account the distribution was paid to (see 0016).
    holding_account_id          INTEGER NOT NULL DEFAULT 1 REFERENCES holding_accounts(id)
);

INSERT INTO income
    (id, listing_id, date_paid, ex_date, franked_amount, unfranked_amount,
     foreign_source_income, foreign_tax_paid, tfn_withholding_tax,
     franking_credits, lic_capital_gain_deduction, conduit_foreign_income,
     trust_income, reinvestment_trade_id, currency, buyback_trade_id,
     holding_account_id)
SELECT id, listing_id, date_paid, ex_date, franked_amount, unfranked_amount,
       foreign_source_income, foreign_tax_paid, tfn_withholding_tax,
       franking_credits, lic_capital_gain_deduction, conduit_foreign_income,
       trust_income, reinvestment_trade_id, currency, buyback_trade_id,
       holding_account_id
FROM income_old;

-- 7. drp_enrolments: its listing FK followed the rename; rebuild (see 0016).
ALTER TABLE drp_enrolments RENAME TO drp_enrolments_old;

CREATE TABLE drp_enrolments (
    id                INTEGER PRIMARY KEY,
    listing_id        INTEGER NOT NULL REFERENCES listings(id),
    -- First day of the period (inclusive): distributions with an ex date (or
    -- pay date when no ex date is recorded) from this day reinvest.
    enrolment_date    TEXT NOT NULL,
    -- Day the unenrolment takes effect (exclusive): distributions with an ex
    -- date on or after it no longer reinvest. NULL = open-ended (currently
    -- enrolled). Periods for a (listing, holding account) must not overlap
    -- and at most one may be open at a time — a multi-row invariant enforced
    -- at write time inside a transaction (entities::drp_enrolment).
    unenrolment_date  TEXT,
    residual_handling TEXT NOT NULL DEFAULT 'CarryForward'
        CHECK(residual_handling IN ('CarryForward', 'PayOut')),
    -- The holding account the enrolment applies to (see 0016).
    holding_account_id INTEGER NOT NULL DEFAULT 1 REFERENCES holding_accounts(id),
    CHECK (unenrolment_date IS NULL OR unenrolment_date > enrolment_date)
);

INSERT INTO drp_enrolments
    (id, listing_id, enrolment_date, unenrolment_date, residual_handling, holding_account_id)
SELECT id, listing_id, enrolment_date, unenrolment_date, residual_handling, holding_account_id
FROM drp_enrolments_old;

DROP TABLE drp_enrolments_old;

-- 8. parcel_allocations: both trade FKs followed the trades rename; rebuild.
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

-- 9. amit_adjustments: its trade_id/amma_statement_id followed the renames;
-- rebuild.
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

-- 10. attachments: its trade_id/income_id/amma_statement_id followed the
-- renames; rebuild (see 0004 for the column semantics).
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

-- 11. Drop the renamed parents, children first (every table that referenced
-- them has been rebuilt against the new tables or dropped above).
DROP TABLE income_old;
DROP TABLE amma_statements_old;
DROP TABLE trades_old;
DROP TABLE corporate_actions_old;
DROP TABLE listings_old;
