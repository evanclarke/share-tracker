-- no-transaction
-- An income row can be ordinary income that is neither a distribution nor
-- remuneration (SCENARIOS L-03/L-04).
--
-- The ATO taxes a **staking reward**, and an **airdrop of an established
-- token**, as ordinary income at the money value of the tokens when they are
-- received, "declared in your tax return as **other income**" — item 24
-- (QC 69950, docs/ato/crypto-staking-airdrops.md). The documented entry for
-- one was an income row plus a Buy at the receipt-date market value, and the
-- income row had only two kinds to be: `Dividend`, which reported the amount
-- at **11S** with a franking status attached, or `EmploymentIncome`, which
-- reported it at item 1/2 as salary. The total assessable income was right and
-- every label on it was wrong.
--
-- `OtherIncome` is the third kind, and 0028 anticipated exactly this: "the enum
-- is the extension point — a further non-dividend kind is a new value here, not
-- a second flag". It reports on its own tax-summary line against item 24, in
-- its own annual-tax-report table, and — unlike the employment kind, which the
-- ATO prefills from STP reporting at item 1/2 — it **is** counted in gross
-- assessable investment income, because nothing else reports it and it is a
-- return on the holding it is recorded against.
--
-- Like `EmploymentIncome`, the row carries the cash (in `unfranked_amount`) and
-- nothing else: no franking, no foreign-source or LIC component, no CFI, no
-- tax-deferred amount, no ex/entitlement date, and it can never be trust income
-- (entities::income::check_non_distribution_row).
--
-- The value set is a column CHECK, which SQLite cannot ALTER, so income is
-- rebuilt via the rename pattern (0018/0021/0027 precedent). This is the first
-- rebuild of a table another table *references*: `attachments.income_id` is a
-- foreign key into it, and SQLite rewrites such a reference to point at the
-- renamed table whenever `foreign_keys` is on — leaving attachments pointing at
-- `income_old`, whose drop would then cascade every income attachment away.
-- Neither PRAGMA that suppresses the rewrite can be set inside a transaction
-- (`foreign_keys` is a documented no-op there), so this migration runs with
-- `-- no-transaction` and brackets its own work in BEGIN/COMMIT — SQLite's own
-- documented procedure for altering a constraint. `legacy_alter_table` keeps
-- the rename from rewriting trigger bodies naming `income` as well.
--
-- The table's five indexes, three snapshot-staleness triggers and two
-- row-history triggers move with the renamed table and are dropped with it, so
-- all ten objects are re-created below, unchanged apart from being attached to
-- the new table. Ids are copied explicitly — row_history entries key on them,
-- and so do the reinvestment/buy-back/attachment links pointing at this table.

PRAGMA foreign_keys = OFF;

BEGIN;

PRAGMA legacy_alter_table = ON;
ALTER TABLE income RENAME TO income_old;
PRAGMA legacy_alter_table = OFF;

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
    lic_capital_gain_amount     TEXT    NOT NULL DEFAULT '0',
    conduit_foreign_income      TEXT    NOT NULL DEFAULT '0',
    trust_income                INTEGER NOT NULL DEFAULT 0,
    reinvestment_trade_id       INTEGER REFERENCES trades(id),
    currency                    TEXT    NOT NULL DEFAULT 'AUD' REFERENCES currencies(code),
    buyback_trade_id            INTEGER REFERENCES trades(id),
    holding_account_id          INTEGER NOT NULL DEFAULT 1 REFERENCES holding_accounts(id),
    -- Optional per-share figures from the registry statement, for cross-checking
    -- a distribution against its payment advice: when supplied (always together),
    -- amount_per_security × securities_held cent-rounded must equal the gross
    -- cash components — validated at write time in entities::income.
    -- Informational/validation-only: no report or calculation reads them.
    amount_per_security         TEXT,
    securities_held             TEXT,
    entitlement_date            TEXT
        CHECK (entitlement_date IS NULL OR trust_income = 1),
    tax_deferred_amount         TEXT
        CHECK (tax_deferred_amount IS NULL
               OR (trust_income = 1 AND CAST(tax_deferred_amount AS NUMERIC) >= 0)),
    income_type                 TEXT    NOT NULL DEFAULT 'Dividend'
        CHECK (income_type IN ('Dividend', 'EmploymentIncome', 'OtherIncome'))
);

INSERT INTO income (id, listing_id, date_paid, ex_date, franked_amount, unfranked_amount,
                    foreign_source_income, foreign_tax_paid, tfn_withholding_tax,
                    franking_credits, lic_capital_gain_amount, conduit_foreign_income,
                    trust_income, reinvestment_trade_id, currency, buyback_trade_id,
                    holding_account_id, amount_per_security, securities_held,
                    entitlement_date, tax_deferred_amount, income_type)
    SELECT id, listing_id, date_paid, ex_date, franked_amount, unfranked_amount,
           foreign_source_income, foreign_tax_paid, tfn_withholding_tax,
           franking_credits, lic_capital_gain_amount, conduit_foreign_income,
           trust_income, reinvestment_trade_id, currency, buyback_trade_id,
           holding_account_id, amount_per_security, securities_held,
           entitlement_date, tax_deferred_amount, income_type
    FROM income_old
    ORDER BY id;

DROP TABLE income_old;

CREATE INDEX income_date_paid ON income (date_paid);
CREATE INDEX income_listing_id ON income (listing_id);
CREATE INDEX income_reinvestment_trade_id ON income (reinvestment_trade_id);
CREATE INDEX income_buyback_trade_id ON income (buyback_trade_id);
CREATE INDEX income_holding_account_id ON income (holding_account_id);

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

CREATE TRIGGER income_row_history_update AFTER UPDATE ON income
BEGIN
    INSERT INTO row_history (table_name, row_id, operation, changed_at, old_row)
    VALUES ('income', OLD.id, 'UPDATE', strftime('%Y-%m-%dT%H:%M:%fZ', 'now'),
            json_object(
                        'id', OLD.id, 'listing_id', OLD.listing_id, 'date_paid', OLD.date_paid,
                        'ex_date', OLD.ex_date, 'franked_amount', OLD.franked_amount,
                        'unfranked_amount', OLD.unfranked_amount, 'foreign_source_income',
                        OLD.foreign_source_income, 'foreign_tax_paid', OLD.foreign_tax_paid,
                        'tfn_withholding_tax', OLD.tfn_withholding_tax, 'franking_credits',
                        OLD.franking_credits, 'lic_capital_gain_amount',
                        OLD.lic_capital_gain_amount, 'conduit_foreign_income',
                        OLD.conduit_foreign_income, 'trust_income', OLD.trust_income,
                        'reinvestment_trade_id', OLD.reinvestment_trade_id, 'currency',
                        OLD.currency, 'buyback_trade_id', OLD.buyback_trade_id,
                        'holding_account_id', OLD.holding_account_id, 'amount_per_security',
                        OLD.amount_per_security, 'securities_held', OLD.securities_held,
                        'entitlement_date', OLD.entitlement_date, 'tax_deferred_amount',
                        OLD.tax_deferred_amount, 'income_type', OLD.income_type));
END;

CREATE TRIGGER income_row_history_delete AFTER DELETE ON income
BEGIN
    INSERT INTO row_history (table_name, row_id, operation, changed_at, old_row)
    VALUES ('income', OLD.id, 'DELETE', strftime('%Y-%m-%dT%H:%M:%fZ', 'now'),
            json_object(
                        'id', OLD.id, 'listing_id', OLD.listing_id, 'date_paid', OLD.date_paid,
                        'ex_date', OLD.ex_date, 'franked_amount', OLD.franked_amount,
                        'unfranked_amount', OLD.unfranked_amount, 'foreign_source_income',
                        OLD.foreign_source_income, 'foreign_tax_paid', OLD.foreign_tax_paid,
                        'tfn_withholding_tax', OLD.tfn_withholding_tax, 'franking_credits',
                        OLD.franking_credits, 'lic_capital_gain_amount',
                        OLD.lic_capital_gain_amount, 'conduit_foreign_income',
                        OLD.conduit_foreign_income, 'trust_income', OLD.trust_income,
                        'reinvestment_trade_id', OLD.reinvestment_trade_id, 'currency',
                        OLD.currency, 'buyback_trade_id', OLD.buyback_trade_id,
                        'holding_account_id', OLD.holding_account_id, 'amount_per_security',
                        OLD.amount_per_security, 'securities_held', OLD.securities_held,
                        'entitlement_date', OLD.entitlement_date, 'tax_deferred_amount',
                        OLD.tax_deferred_amount, 'income_type', OLD.income_type));
END;

COMMIT;

-- Restored for the connection this ran on; every other pooled connection opens
-- with foreign keys on (infra::db).
PRAGMA foreign_keys = ON;
