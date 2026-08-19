-- The AMMA's *second* foreign-tax line: foreign tax paid on foreign capital
-- gains (SCENARIOS M-12).
--
-- Part C of an AMMA statement reports foreign tax on capital gains separately
-- from foreign tax on other foreign source income, because the two are
-- claimed differently. `foreign_tax_credits` held both, and the tax summary
-- added the whole figure to the year's FITO line — which over-claims, because
-- where only part of a foreign capital gain is assessable (the Division 115
-- discount being the ordinary case) "the foreign tax paid on the gain must be
-- apportioned accordingly" (docs/ato/fito-capital-gains-apportionment.md,
-- QC 104349 *When a FITO applies*; ATO ID 2010/175). The trustee reports the
-- grossed-up figure on purpose — the AMMA guidance notes say FITO is *not*
-- reduced for discount capital gains applied at trust level — so the reduction
-- is the investor's step, and until now nothing did it or asked for it.
--
-- The column is **additional**, not a split of the existing one, and defaults
-- to 0. So every existing row keeps exactly the figures it reports today:
-- `foreign_tax_credits` stays the foreign tax on foreign *income*, claimable
-- in full, and a database that has never separated the two reads forward
-- unchanged. Moving a statement's capital-gains portion across is a
-- deliberate edit against its own Part C detail, which is the only place the
-- split exists — no migration can infer it from a combined figure.
--
-- Maintenance rule (0013): amma_statements is audited, so adding a column
-- means dropping and re-creating its two *_row_history_* triggers with the new
-- column in their json_object lists. Done below — a column the trail drops is
-- a version of the row that can never be recovered.

ALTER TABLE amma_statements ADD COLUMN foreign_tax_credits_capital_gains TEXT NOT NULL DEFAULT '0';

DROP TRIGGER amma_statements_row_history_update;
DROP TRIGGER amma_statements_row_history_delete;

CREATE TRIGGER amma_statements_row_history_update AFTER UPDATE ON amma_statements
BEGIN
    INSERT INTO row_history (table_name, row_id, operation, changed_at, old_row)
    VALUES ('amma_statements', OLD.id, 'UPDATE', strftime('%Y-%m-%dT%H:%M:%fZ', 'now'),
            json_object(
                        'id', OLD.id, 'listing_id', OLD.listing_id, 'tax_year_end_date',
                        OLD.tax_year_end_date, 'units_held', OLD.units_held, 'date_received',
                        OLD.date_received, 'australian_interest', OLD.australian_interest,
                        'australian_dividends_unfranked', OLD.australian_dividends_unfranked,
                        'franked_dividends', OLD.franked_dividends, 'franking_credits',
                        OLD.franking_credits, 'net_rent', OLD.net_rent, 'foreign_income',
                        OLD.foreign_income, 'foreign_tax_credits', OLD.foreign_tax_credits,
                        'foreign_tax_credits_capital_gains',
                        OLD.foreign_tax_credits_capital_gains,
                        'other_income', OLD.other_income, 'cgt_discount_gains',
                        OLD.cgt_discount_gains, 'cgt_indexation_gains',
                        OLD.cgt_indexation_gains, 'cgt_other_gains', OLD.cgt_other_gains,
                        'capital_losses_applied', OLD.capital_losses_applied,
                        'tax_deferred_amount', OLD.tax_deferred_amount, 'tax_free_amount',
                        OLD.tax_free_amount, 'cost_base_adjustment', OLD.cost_base_adjustment,
                        'tfn_withholding_tax', OLD.tfn_withholding_tax, 'currency',
                        OLD.currency, 'holding_account_id', OLD.holding_account_id));
END;

CREATE TRIGGER amma_statements_row_history_delete AFTER DELETE ON amma_statements
BEGIN
    INSERT INTO row_history (table_name, row_id, operation, changed_at, old_row)
    VALUES ('amma_statements', OLD.id, 'DELETE', strftime('%Y-%m-%dT%H:%M:%fZ', 'now'),
            json_object(
                        'id', OLD.id, 'listing_id', OLD.listing_id, 'tax_year_end_date',
                        OLD.tax_year_end_date, 'units_held', OLD.units_held, 'date_received',
                        OLD.date_received, 'australian_interest', OLD.australian_interest,
                        'australian_dividends_unfranked', OLD.australian_dividends_unfranked,
                        'franked_dividends', OLD.franked_dividends, 'franking_credits',
                        OLD.franking_credits, 'net_rent', OLD.net_rent, 'foreign_income',
                        OLD.foreign_income, 'foreign_tax_credits', OLD.foreign_tax_credits,
                        'foreign_tax_credits_capital_gains',
                        OLD.foreign_tax_credits_capital_gains,
                        'other_income', OLD.other_income, 'cgt_discount_gains',
                        OLD.cgt_discount_gains, 'cgt_indexation_gains',
                        OLD.cgt_indexation_gains, 'cgt_other_gains', OLD.cgt_other_gains,
                        'capital_losses_applied', OLD.capital_losses_applied,
                        'tax_deferred_amount', OLD.tax_deferred_amount, 'tax_free_amount',
                        OLD.tax_free_amount, 'cost_base_adjustment', OLD.cost_base_adjustment,
                        'tfn_withholding_tax', OLD.tfn_withholding_tax, 'currency',
                        OLD.currency, 'holding_account_id', OLD.holding_account_id));
END;
