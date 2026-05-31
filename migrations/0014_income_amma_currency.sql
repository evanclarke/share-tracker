-- Currency denomination for income and AMMA statement amounts, so the tax summary
-- can convert non-AUD amounts to AUD via the ATO reference rate (infra::fx::to_aud)
-- before aggregating. Defaults to 'AUD' so existing rows keep their current
-- (pass-through) behaviour. The FX month is driven by income.date_paid and
-- amma_statements.tax_year_end_date respectively. See REQUIREMENTS.md > FX.
ALTER TABLE income          ADD COLUMN currency TEXT NOT NULL DEFAULT 'AUD';
ALTER TABLE amma_statements ADD COLUMN currency TEXT NOT NULL DEFAULT 'AUD';
