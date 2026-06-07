-- Holding accounts (REQUIREMENTS "Holding accounts — the same listing held in
-- separate holdings"): a custody/location dimension *within* one taxpayer, so
-- the same listing can be held in more than one place at once with different
-- treatment — e.g. RSU-vested shares sitting in an employer share-plan account
-- (which cannot participate in the DRP) alongside DRP-enrolled shares in the
-- holder's own broker account. Trades, income, AMMA statements (a registry
-- issues one statement per holder account) and DRP enrolment periods each gain
-- a NOT NULL holding_account_id; a default account is seeded and every
-- existing row migrates to it, so current data and API clients are untouched.
--
-- Transfers move a quantity of a listing between two holding accounts of the
-- same owner — not a CGT event, so executing one (entities::transfer) creates
-- a price-0 transfer-out Sell in the source account plus per-parcel
-- transfer-in Buys in the destination carrying each consumed parcel's
-- remaining reduced cost base and acquisition date (the scrip-for-scrip
-- mechanics; both sides carry trades.transfer_id and are excluded from the
-- realised-gains/net-capital-gain reports).
--
-- SQLite cannot add a NOT NULL FK column in place (ADD COLUMN requires a NULL
-- default for REFERENCES columns), so the four tables are rebuilt via the
-- rename pattern. Because foreign keys are enabled, ALTER TABLE ... RENAME
-- rewrites the REFERENCES clauses of referencing tables to follow the rename
-- (the NOTE in 0012): renaming trades drags parcel_allocations,
-- amit_adjustments, income, and attachments with it, and renaming
-- amma_statements drags amit_adjustments and attachments, so the whole
-- FK-connected cluster is rebuilt (same as 0013/0014/0015). No data is
-- dropped: every copy is a straight column-for-column SELECT (TEXT decimals
-- stay TEXT, no CAST), and DROP TABLE only ever touches a renamed _old table.
--
-- New columns added while the tables are open:
--   trades.holding_account_id          — the account the parcel/sale sits in.
--   trades.transfer_id                 — set only by the transfer operation on
--                                        the transfer-out Sell and every
--                                        transfer-in Buy; the group is
--                                        immutable individually and deleted as
--                                        a whole via DELETE /transfers/:id.
--   income.holding_account_id          — the account the distribution was paid
--                                        to (drives DRP eligibility and where
--                                        the reinvestment trade lands).
--   amma_statements.holding_account_id — the account the statement covers.
--   drp_enrolments.holding_account_id  — enrolment periods are per
--                                        (listing, holding account).

-- 1. The new reference table, with the seeded default account every existing
-- row migrates to.
CREATE TABLE holding_accounts (
    id   INTEGER PRIMARY KEY,
    name TEXT NOT NULL UNIQUE
);

INSERT INTO holding_accounts (id, name) VALUES (1, 'Default');

-- 2. Transfers between holding accounts. The per-parcel quantities live on
-- the transfer-out Sell's parcel_allocations rows, not here (single source of
-- truth, same as ordinary Sells).
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

-- 3. trades: add holding_account_id (existing rows → the default account) and
-- the transfer_id provenance column.
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
    -- The holding account the trade sits in. Existing rows migrate to the
    -- seeded default account.
    holding_account_id  INTEGER NOT NULL DEFAULT 1 REFERENCES holding_accounts(id),
    -- Provenance link from a transfer-out Sell / transfer-in Buy back to its
    -- transfer: set only by PUT /transfers/:id (NULL for every other trade).
    -- The trades carrying one transfer id form the transfer group: each is
    -- rejected by PUT /sells and PUT/DELETE /trades and DELETE /sells;
    -- DELETE /transfers/:id removes the whole group.
    transfer_id         INTEGER REFERENCES transfers(id)
);

INSERT INTO trades
    (id, trade_type, date, settlement_date, listing_id, average_price, quantity,
     currency, brokerage, gst_on_brokerage, brokerage_currency, fx_rate,
     contract_note_ref, residual_brought_forward, residual_carried_forward,
     residual_paid_out, rights_action_id, buyback_action_id, scrip_action_id,
     demerger_action_id, deemed_acquisition_date)
SELECT id, trade_type, date, settlement_date, listing_id, average_price, quantity,
       currency, brokerage, gst_on_brokerage, brokerage_currency, fx_rate,
       contract_note_ref, residual_brought_forward, residual_carried_forward,
       residual_paid_out, rights_action_id, buyback_action_id, scrip_action_id,
       demerger_action_id, deemed_acquisition_date
FROM trades_old;

-- 4. amma_statements: add holding_account_id.
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
    -- The holding account the statement covers (a registry issues one AMMA
    -- statement per holder account). Existing rows migrate to the default.
    holding_account_id              INTEGER NOT NULL DEFAULT 1 REFERENCES holding_accounts(id)
);

INSERT INTO amma_statements
    (id, listing_id, tax_year_end_date, units_held, date_received,
     australian_interest, australian_dividends_unfranked, franked_dividends,
     franking_credits, net_rent, foreign_income, foreign_tax_credits,
     other_income, cgt_discount_gains, cgt_indexation_gains, cgt_other_gains,
     capital_losses_applied, tax_deferred_amount, tax_free_amount,
     cost_base_adjustment, tfn_withholding_tax, currency)
SELECT id, listing_id, tax_year_end_date, units_held, date_received,
       australian_interest, australian_dividends_unfranked, franked_dividends,
       franking_credits, net_rent, foreign_income, foreign_tax_credits,
       other_income, cgt_discount_gains, cgt_indexation_gains, cgt_other_gains,
       capital_losses_applied, tax_deferred_amount, tax_free_amount,
       cost_base_adjustment, tfn_withholding_tax, currency
FROM amma_statements_old;

-- 5. income: add holding_account_id; its trade FKs followed the trades
-- rename, so it had to be rebuilt anyway.
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
    -- The holding account the distribution was paid to: decides whose DRP
    -- enrolment applies and which account the reinvestment trade lands in.
    -- Existing rows migrate to the default account.
    holding_account_id          INTEGER NOT NULL DEFAULT 1 REFERENCES holding_accounts(id)
);

INSERT INTO income
    (id, listing_id, date_paid, ex_date, franked_amount, unfranked_amount,
     foreign_source_income, foreign_tax_paid, tfn_withholding_tax,
     franking_credits, lic_capital_gain_deduction, conduit_foreign_income,
     trust_income, reinvestment_trade_id, currency, buyback_trade_id)
SELECT id, listing_id, date_paid, ex_date, franked_amount, unfranked_amount,
       foreign_source_income, foreign_tax_paid, tfn_withholding_tax,
       franking_credits, lic_capital_gain_deduction, conduit_foreign_income,
       trust_income, reinvestment_trade_id, currency, buyback_trade_id
FROM income_old;

-- 6. drp_enrolments: add holding_account_id — enrolment periods (and their
-- overlap invariants and residual chains) are per (listing, holding account).
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
    -- The holding account the enrolment applies to: the same listing may be
    -- enrolled in one account and unenrolled in another (e.g. an employer
    -- share-plan account that cannot DRP alongside an enrolled personal
    -- account). Existing rows migrate to the default account.
    holding_account_id INTEGER NOT NULL DEFAULT 1 REFERENCES holding_accounts(id),
    CHECK (unenrolment_date IS NULL OR unenrolment_date > enrolment_date)
);

INSERT INTO drp_enrolments (id, listing_id, enrolment_date, unenrolment_date, residual_handling)
SELECT id, listing_id, enrolment_date, unenrolment_date, residual_handling
FROM drp_enrolments_old;

DROP TABLE drp_enrolments_old;

-- 7. parcel_allocations: both trade FKs followed the trades rename; rebuild.
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

-- 8. amit_adjustments: its trade_id/amma_statement_id followed the renames;
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

-- 9. attachments: its trade_id/income_id/amma_statement_id followed the
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

-- 10. Drop the renamed parents, children first (every table that referenced
-- them has been rebuilt against the new tables or dropped above).
DROP TABLE income_old;
DROP TABLE amma_statements_old;
DROP TABLE trades_old;
