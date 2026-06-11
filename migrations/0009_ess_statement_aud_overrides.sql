-- Statement-AUD overrides for the ESS discount labels.
--
-- The employer's annual Employee share scheme statement (and the matching ATO
-- prefill) states each discount label in AUD, converted at the release-date
-- spot rate — not the RBA monthly rate the tax summary otherwise applies to a
-- foreign-currency statement. When the employer's AUD figure is recorded here
-- the tax summary reports it verbatim for that label; absent, the label keeps
-- converting via the RBA monthly rate as before. Only meaningful on a non-AUD
-- statement — write-time validation rejects overrides when currency = 'AUD'
-- (entities::ess_statement).
--
-- No snapshot-staleness triggers: no snapshotted report reads ess_statements
-- (the tax summary is not snapshotted), matching the table's existing columns.

ALTER TABLE ess_statements ADD COLUMN aud_taxed_upfront_eligible      TEXT;
ALTER TABLE ess_statements ADD COLUMN aud_taxed_upfront_not_eligible  TEXT;
ALTER TABLE ess_statements ADD COLUMN aud_deferral_discount           TEXT;
ALTER TABLE ess_statements ADD COLUMN aud_pre_2009_cessation_discount TEXT;
ALTER TABLE ess_statements ADD COLUMN aud_foreign_source_discount     TEXT;
