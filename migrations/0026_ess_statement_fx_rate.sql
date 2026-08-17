-- SCENARIOS J-08/J-12: the ESS vest Buy's FX rate was a hard-coded '1'. On a
-- trade that column is not a constant — it is the *fallback* rate applied when
-- no ATO/RBA monthly rate exists for the amount's month (infra::fx::pick_rate,
-- FxOverride::Fallback) — so the placeholder became a real answer exactly when
-- the month's rate was missing, costing a US$15,000 vest parcel at A$15,000.
-- Every other parcel-creating operation takes a rate from the user
-- (inheritances.fx_rate, the rights-exercise and DRP-reinvest body fields) or
-- carries the consumed parcel's forward (domain::rollover); the ESS vest was
-- the only one that invented one.
--
-- `fx_rate` is the rate the taxpayer states for this statement: the same
-- foreign-per-AUD convention as `trades.fx_rate`/`inheritances.fx_rate`
-- (AUD = foreign / rate). NULL — every existing row — means "none stated", and
-- the vest then resolves the taxing-point month's ATO rate and refuses (422)
-- when there is none, rather than binding a parity placeholder. Write-time
-- validation (entities::ess_statement::db_upsert) rejects a non-positive value
-- and a rate on an AUD statement, where it could never apply.
ALTER TABLE ess_statements ADD COLUMN fx_rate TEXT;

-- ess_statements is audited (CLAUDE.md rule): ALTER TABLE ADD COLUMN does not
-- update existing triggers, so its two row_history triggers are re-created here
-- with fx_rate added to the JSON column list.
DROP TRIGGER ess_statements_row_history_update;
DROP TRIGGER ess_statements_row_history_delete;

CREATE TRIGGER ess_statements_row_history_update AFTER UPDATE ON ess_statements
BEGIN
    INSERT INTO row_history (table_name, row_id, operation, changed_at, old_row)
    VALUES ('ess_statements', OLD.id, 'UPDATE', strftime('%Y-%m-%dT%H:%M:%fZ', 'now'),
            json_object(
                        'id', OLD.id, 'listing_id', OLD.listing_id, 'holding_account_id',
                        OLD.holding_account_id, 'taxing_point_date', OLD.taxing_point_date,
                        'quantity', OLD.quantity, 'market_value_per_share',
                        OLD.market_value_per_share, 'taxed_upfront_eligible',
                        OLD.taxed_upfront_eligible, 'taxed_upfront_not_eligible',
                        OLD.taxed_upfront_not_eligible, 'deferral_discount',
                        OLD.deferral_discount, 'pre_2009_cessation_discount',
                        OLD.pre_2009_cessation_discount, 'foreign_source_discount',
                        OLD.foreign_source_discount, 'tfn_withholding', OLD.tfn_withholding,
                        'currency', OLD.currency, 'aud_taxed_upfront_eligible',
                        OLD.aud_taxed_upfront_eligible, 'aud_taxed_upfront_not_eligible',
                        OLD.aud_taxed_upfront_not_eligible, 'aud_deferral_discount',
                        OLD.aud_deferral_discount, 'aud_pre_2009_cessation_discount',
                        OLD.aud_pre_2009_cessation_discount, 'aud_foreign_source_discount',
                        OLD.aud_foreign_source_discount, 'fx_rate', OLD.fx_rate));
END;

CREATE TRIGGER ess_statements_row_history_delete AFTER DELETE ON ess_statements
BEGIN
    INSERT INTO row_history (table_name, row_id, operation, changed_at, old_row)
    VALUES ('ess_statements', OLD.id, 'DELETE', strftime('%Y-%m-%dT%H:%M:%fZ', 'now'),
            json_object(
                        'id', OLD.id, 'listing_id', OLD.listing_id, 'holding_account_id',
                        OLD.holding_account_id, 'taxing_point_date', OLD.taxing_point_date,
                        'quantity', OLD.quantity, 'market_value_per_share',
                        OLD.market_value_per_share, 'taxed_upfront_eligible',
                        OLD.taxed_upfront_eligible, 'taxed_upfront_not_eligible',
                        OLD.taxed_upfront_not_eligible, 'deferral_discount',
                        OLD.deferral_discount, 'pre_2009_cessation_discount',
                        OLD.pre_2009_cessation_discount, 'foreign_source_discount',
                        OLD.foreign_source_discount, 'tfn_withholding', OLD.tfn_withholding,
                        'currency', OLD.currency, 'aud_taxed_upfront_eligible',
                        OLD.aud_taxed_upfront_eligible, 'aud_taxed_upfront_not_eligible',
                        OLD.aud_taxed_upfront_not_eligible, 'aud_deferral_discount',
                        OLD.aud_deferral_discount, 'aud_pre_2009_cessation_discount',
                        OLD.aud_pre_2009_cessation_discount, 'aud_foreign_source_discount',
                        OLD.aud_foreign_source_discount, 'fx_rate', OLD.fx_rate));
END;

-- No snapshot-staleness triggers, for the reason 0009 gave when it added the
-- statement-AUD override columns: no snapshotted report reads ess_statements
-- (the snapshotted reports are the price-dependent portfolio/unrealised ones;
-- the ESS discount reaches the tax summary, which is not snapshotted). The
-- vest *Buy* this column feeds is a trades row, and the trades staleness
-- triggers (0001_schema.sql) already cover it.
