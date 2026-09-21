-- Record the Division 119 minimum-tax inputs per financial year.
--
-- From 1 July 2027 a minimum 30 per cent rate of income tax applies to the
-- year's *minimum tax capital gain* (new Division 119 of the ITAA 1997; EM
-- 1.171-1.193, docs/ato/cgt-reform-cgt-adjustments.md). Whether extra tax is
-- payable, and how much, is the seven-step *minimum tax gap amount* method
-- statement in s 119-10(2): step 1 is a 30 per cent benchmark on the gain;
-- steps 2-4 are the taxpayer's basic income tax liability on their taxable
-- income with and without the gain, which needs a marginal-rate schedule and a
-- taxable-income total this project has never computed (there is no salary or
-- other non-investment income model). The gap is therefore **recorded** rather
-- than computed -- the same shape as the ESS income test (0027) and the FITO
-- cap -- and the report surfaces the working it can derive around it (the
-- step-1 benchmark and the s 12AA rate).
--
-- The second recorded fact is the s 119-15 exemption: no minimum tax is
-- payable where the taxpayer received a payment of a kind the Minister
-- prescribes at any time in the income year. The instrument is not yet made;
-- the intended list is the income-support payments relied on for basic living
-- expenses (Age Pension, Disability Support Pension, JobSeeker, Parenting
-- Payment, Youth Allowance, farm household allowance, ABSTUDY living
-- allowance, special rate disability pension -- EM 1.186-1.190). Eligibility
-- for a payment is a fact about the taxpayer this system cannot see, so it is
-- recorded per year rather than inferred.
--
-- Both columns are stored as the *exception*: an absent row, or a row whose
-- body omits the field, means no gap recorded and not exempt -- the standing
-- assumption, and exactly what an empty table meant before this migration, so
-- no existing database's figures move.
--
-- tax_year_settings is an audited table (0027), so this migration re-creates
-- its two row_history triggers with the widened column list, dropped first: a
-- column added to an audited table without this leaves the audit trail
-- recording a row shape the table no longer has (the live-schema test
-- reports::row_history::every_audited_column_is_recorded_by_both_triggers
-- enforces it). The trigger bodies are reproduced from 0053 apart from the
-- added keys.

ALTER TABLE tax_year_settings
    ADD COLUMN minimum_tax_gap_amount TEXT;

ALTER TABLE tax_year_settings
    ADD COLUMN minimum_tax_income_support_exempt INTEGER NOT NULL DEFAULT 0
        CHECK (minimum_tax_income_support_exempt IN (0, 1));

DROP TRIGGER tax_year_settings_row_history_update;
DROP TRIGGER tax_year_settings_row_history_delete;

CREATE TRIGGER tax_year_settings_row_history_update AFTER UPDATE ON tax_year_settings
BEGIN
    INSERT INTO row_history (table_name, row_id, operation, changed_at, old_row)
    VALUES ('tax_year_settings', OLD.tax_year, 'UPDATE', strftime('%Y-%m-%dT%H:%M:%fZ', 'now'),
            json_object(
                        'tax_year', OLD.tax_year,
                        'ess_taxed_upfront_reduction_eligible',
                        OLD.ess_taxed_upfront_reduction_eligible,
                        'foreign_or_temporary_resident_at_some_time',
                        OLD.foreign_or_temporary_resident_at_some_time,
                        'minimum_tax_gap_amount',
                        OLD.minimum_tax_gap_amount,
                        'minimum_tax_income_support_exempt',
                        OLD.minimum_tax_income_support_exempt));
END;

CREATE TRIGGER tax_year_settings_row_history_delete AFTER DELETE ON tax_year_settings
BEGIN
    INSERT INTO row_history (table_name, row_id, operation, changed_at, old_row)
    VALUES ('tax_year_settings', OLD.tax_year, 'DELETE', strftime('%Y-%m-%dT%H:%M:%fZ', 'now'),
            json_object(
                        'tax_year', OLD.tax_year,
                        'ess_taxed_upfront_reduction_eligible',
                        OLD.ess_taxed_upfront_reduction_eligible,
                        'foreign_or_temporary_resident_at_some_time',
                        OLD.foreign_or_temporary_resident_at_some_time,
                        'minimum_tax_gap_amount',
                        OLD.minimum_tax_gap_amount,
                        'minimum_tax_income_support_exempt',
                        OLD.minimum_tax_income_support_exempt));
END;
