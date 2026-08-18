-- Income rows get a kind (SCENARIOS J-10).
--
-- A **dividend equivalent** paid on unvested RSUs is ordinary income as
-- remuneration under s 6-5 — "not a dividend in the employee's hands", not part
-- of the ESS discount, and carrying no franking (TD 2017/26,
-- docs/ato/ess-dividend-equivalents.md). The documented workaround was to enter
-- the cash as an income row "if the user wants it aggregated here", and that
-- row then reported as `dividends_assessable` at **item 11S, unfranked
-- dividends**, counted in gross assessable investment income, and printed in
-- the annual document's **Dividend income** table with a franking status. The
-- accountant's copy called remuneration a dividend, and nothing said otherwise.
--
-- So an income row now states what kind of payment it is. `Dividend` is the
-- default and every existing row is one, so no stored figure moves.
--
-- Why `EmploymentIncome` rather than a generic `Other`: the point of the kind
-- is to say where the amount belongs on the return, and only a named kind can
-- carry that. This one belongs at item 1/2 salary and wages — which the ATO
-- prefills from the employer's STP reporting, so the tax summary reports it on
-- its own **informational** line rather than adding it to any assessable total.
-- The enum is the extension point (as `corporate_actions.action_type` is): a
-- further non-dividend kind is a new value here, not a second flag.
--
-- Orthogonal to `trust_income`, which distinguishes two *investment* income
-- kinds, and deliberately not folded into it: rewriting that boolean would
-- touch the assessability timing, AMIT and franking rules of every existing
-- row for no gain. A write-time rule keeps the two consistent — an
-- EmploymentIncome row can never be trust income, and carries no dividend
-- component at all (entities::income::UpsertError::EmploymentIncomeComponent).

-- income is audited (0013_row_history.sql), so its two row-history triggers are
-- dropped and re-created with the new column list — a column the trail drops is
-- a version that cannot be reconstructed. The live pair came from 0025 (which
-- renamed lic_capital_gain_deduction); this replaces it.
DROP TRIGGER income_row_history_update;
DROP TRIGGER income_row_history_delete;

-- 'Dividend' covers every existing row: before this migration the only income
-- rows enterable were distributions (a dividend, a trust distribution, or a
-- buy-back's dividend component), so the default is a statement of fact rather
-- than a guess.
ALTER TABLE income ADD COLUMN income_type TEXT NOT NULL DEFAULT 'Dividend'
    CHECK (income_type IN ('Dividend', 'EmploymentIncome'));

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
