-- The LIC capital gain field now records what the dividend statement prints.
--
-- A listed investment company advises how much of a dividend is attributable to
-- a LIC capital gain (the **attributable part**); an individual deducts **50%**
-- of it at question D8 (docs/ato/lic-capital-gain-deduction.md: Ben's $50
-- attributable part is a $25 deduction). The column took the already-halved
-- deduction and the tax summary passed it straight through, so entering the
-- statement's own figure — the natural reading of a field named after the LIC
-- capital gain — silently doubled the deduction (SCENARIOS G-04). Nothing said
-- otherwise: no docs, no form hint.
--
-- So the column becomes the statement figure, `lic_capital_gain_amount`, and
-- `reports::tax_summary` computes the 50% for D8 (via
-- `entities::income::Income::lic_capital_gain_deduction`, shared with the annual
-- tax report's per-dividend column so the two cannot disagree).
--
-- Existing rows hold a deduction under the old convention, so they are read
-- forward by doubling: deduction × 2 = the attributable part it was halved from.
-- The doubling is exact, done on the decimal's own digits as an integer (money
-- is TEXT decimal — a value must never round-trip through REAL): the digit
-- string without its point, doubled, then re-pointed at the same scale.
-- Zero rows (the column default, i.e. every non-LIC distribution) are skipped.

-- income is audited (0013_row_history.sql), so its two row-history triggers are
-- dropped and re-created with the new column list — the JSON key is a string
-- literal a RENAME COLUMN would leave saying `lic_capital_gain_deduction` while
-- carrying the attributable part. The doubling therefore records no row-history
-- entry: it happens while the triggers are down, deliberately, because it is a
-- schema re-reading of a stored figure rather than an edit of a fact, and it is
-- exactly invertible (halve) if a row turns out to have been keyed with the
-- statement's amount already.
DROP TRIGGER income_row_history_update;
DROP TRIGGER income_row_history_delete;

ALTER TABLE income RENAME COLUMN lic_capital_gain_deduction TO lic_capital_gain_amount;

UPDATE income SET lic_capital_gain_amount = (
    WITH doubled(scale, digits) AS (
        SELECT CASE WHEN instr(lic_capital_gain_amount, '.') > 0
                    THEN length(lic_capital_gain_amount)
                         - instr(lic_capital_gain_amount, '.')
                    ELSE 0 END,
               CAST(replace(lic_capital_gain_amount, '.', '') AS INTEGER) * 2
    )
    SELECT CASE
        -- printf, not CAST(... AS TEXT): the digits are an exact integer here,
        -- but a CAST to TEXT in a migration is banned outright as a
        -- float-imprecision risk (infra::db's migrations_store_decimals_as_text
        -- guard), and the rule is worth keeping blunt.
        WHEN scale = 0 THEN printf('%d', digits)
        -- Pad to at least scale + 1 digits so the point always has an integer
        -- digit before it ('0.07' -> digits 14, scale 2 -> '014' -> '0.14').
        ELSE substr(printf('%0*d', scale + 1, digits),
                    1, length(printf('%0*d', scale + 1, digits)) - scale)
             || '.' ||
             substr(printf('%0*d', scale + 1, digits),
                    length(printf('%0*d', scale + 1, digits)) - scale + 1)
    END
    FROM doubled
)
WHERE CAST(lic_capital_gain_amount AS NUMERIC) <> 0;

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
                        OLD.tax_deferred_amount));
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
                        OLD.tax_deferred_amount));
END;
