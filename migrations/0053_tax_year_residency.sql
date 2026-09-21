-- Record the taxpayer's residency per financial year (s 114-25).
--
-- From 1 July 2027 cost base indexation replaces the 50 per cent CGT discount,
-- and s 114-25 makes it unavailable where the individual was a **foreign
-- resident or a temporary resident at any time** between 1 July 2027 (or the
-- day the asset was acquired, if later) and the day of the CGT event
-- (EM 1.59, 1.62; docs/ato/cgt-reform-cgt-adjustments.md). The app's standing
-- assumption is one Australian-resident individual (reports::TAXPAYER_BASIS),
-- which is right for the ordinary case and was, until now, the only answer it
-- could give: `domain::cgt_indexation::Residency`'s foreign/temporary arm
-- existed but nothing in the server could construct it, so the denial was
-- unreachable through the API and untestable end to end.
--
-- Per year, not one global flag, and for the same reason 0027 is per year: the
-- residency answer belongs to a period of the taxpayer's life, and the app
-- reports every recorded year at once. Absent row = the standing assumption
-- (Australian resident throughout), so an empty table behaves exactly as the
-- system did before this column existed and no existing database's figures
-- move; only an explicitly recorded year changes one.
--
-- The answer is deliberately **year-granular** and stored as the *exception*:
-- 1 means the taxpayer was a foreign or temporary resident at some time in
-- that financial year. The s 114-25 testing period runs from 1 July 2027 (or
-- acquisition, if later) to the CGT event, so a disposal is denied indexation
-- when any financial year the period runs through carries the flag
-- (`domain::cgt_indexation::residency_for_testing_period`). A foreign period
-- inside the event's own financial year but *after* the event date therefore
-- denies indexation where strictly it would not — the conservative direction
-- for an indexation benefit, and stated in docs/API.md's Known limitations
-- rather than left implicit.
--
-- tax_year_settings is an audited table (0027), so this migration re-creates
-- its two row_history triggers with the widened column list. The trigger
-- bodies are reproduced verbatim from 0027 apart from the added key, and both
-- are dropped first: a column added to an audited table without this leaves
-- the audit trail recording a row shape the table no longer has.

ALTER TABLE tax_year_settings
    ADD COLUMN foreign_or_temporary_resident_at_some_time INTEGER NOT NULL DEFAULT 0
        CHECK (foreign_or_temporary_resident_at_some_time IN (0, 1));

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
                        OLD.foreign_or_temporary_resident_at_some_time));
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
                        OLD.foreign_or_temporary_resident_at_some_time));
END;
