-- Interest income (REQUIREMENTS 2026-06-10; docs/ato/tax-return-labels-2026.md
-- question 10 — Gross interest, labels 10L/10M).
--
-- Bank / term-deposit / broker-cash interest has no listing, and the `income`
-- entity is listing-keyed (dividends and trust distributions), so interest is
-- its own table. The tax summary (reports/tax_summary.rs) reports the year's
-- gross interest as its `interest_income` line (10L), includes it in gross
-- assessable investment income, and joins the TFN amount withheld to the
-- combined withholding line (10M).
--
-- No snapshot-staleness triggers: the only report reading this table is the
-- tax summary, which is not snapshotted (only the price-dependent
-- portfolio-overview / unrealised-gains / performance reports are), so no
-- stored snapshot can go stale from an interest write
-- (cf. 0006_rights_sales.sql).
CREATE TABLE interest_income (
    id                  INTEGER PRIMARY KEY,
    -- Date the interest was paid/credited: its month drives the ATO FX rate
    -- used to convert a non-AUD amount to AUD, and the Australian financial
    -- year it is assessed in (July–June; a July date belongs to the next FY).
    date_paid           TEXT    NOT NULL,
    -- Gross interest in `currency`, including any TFN amount withheld (the
    -- ATO's 10L convention: the gross figure is declared, the withheld amount
    -- separately at 10M).
    amount              TEXT    NOT NULL DEFAULT '0',
    -- TFN amount withheld from the gross interest; joins the tax summary's
    -- combined TFN withholding line.
    tfn_withholding_tax TEXT    NOT NULL DEFAULT '0',
    currency            TEXT    NOT NULL DEFAULT 'AUD' REFERENCES currencies(code),
    -- Free-text source description (e.g. "ANZ savings account", "ATO interest
    -- on early payment"). Informational only — no calculation reads it.
    source              TEXT,
    -- Optional link to the holding account the interest was paid on (e.g. a
    -- broker cash account). NULL for interest from outside the portfolio's
    -- accounts (an ordinary bank account). Informational only — no
    -- calculation reads it.
    holding_account_id  INTEGER REFERENCES holding_accounts(id)
);
