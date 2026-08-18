-- Per-financial-year taxpayer settings (SCENARIOS J-02).
--
-- The $1,000 taxed-upfront ESS reduction is available only where the
-- taxpayer's *adjusted taxable income* for the year is A$180,000 or less
-- (docs/ato/employee-share-schemes.md) — a taxpayer-level test over income
-- this system does not hold. Until now the tool applied min(A$1,000, D)
-- unconditionally and documented the test as the user's responsibility, which
-- left an ineligible taxpayer with no way to say so: the only way to make the
-- summary report the right total was to enter the discount at label E
-- (taxed-upfront *not eligible*), misstating 12D/12E to get 12B right.
--
-- Why per year rather than a flag on the cgt_settings singleton: the income
-- test is answered year by year, and the tax summary reports *every* recorded
-- year in one response. A single global flag would strip the reduction from
-- years that never crossed the threshold — wrong for any taxpayer whose income
-- crosses A$180,000 partway through their recorded history, which is the
-- ordinary case over a working life.
--
-- Absent row = eligible. An empty table therefore behaves exactly as the
-- system did before this migration, and only an explicitly ineligible year
-- changes any figure — so no existing database's numbers move.
--
-- The table joins the append-only audit trail: it carries a taxpayer-level
-- fact that changes an assessable total, which is precisely the audit scope
-- decision (0013), and cgt_settings is audited for the same reason.
--
-- Unlike closing_prices (0021) this table needs no surrogate `id` for
-- row_history.row_id to key on: `tax_year` is already a meaningful integer,
-- and it is never reused for a different fact. Deleting FY2026's settings and
-- entering them again is the *same* taxpayer-year fact, so inheriting that
-- year's own history is right rather than a leak.

CREATE TABLE tax_year_settings (
    -- The Australian financial year, identified by the calendar year of its
    -- 30 June end — the same key domain::tax_year hands every FY-keyed report,
    -- and the row's identity in the audit trail. 1986 is the first year that
    -- can hold anything: CGT starts 20 September 1985, inside FY1986.
    tax_year INTEGER PRIMARY KEY CHECK (tax_year >= 1986),
    -- Whether the taxpayer's adjusted taxable income for the year was within
    -- the A$180,000 limit for the $1,000 taxed-upfront reduction. 1 = eligible
    -- (the default, and what an absent row means); 0 = the tax summary reports
    -- the year's taxed-upfront discount unreduced.
    ess_taxed_upfront_reduction_eligible INTEGER NOT NULL DEFAULT 1
        CHECK (ess_taxed_upfront_reduction_eligible IN (0, 1))
);

-- ---------------------------------------------------------------------------
-- Extend row_history's table_name CHECK to accept 'tax_year_settings'.
--
-- A table-level CHECK SQLite cannot ALTER, so row_history is rebuilt via the
-- rename pattern exactly as 0018 and 0021 did (see 0018's long note):
-- legacy_alter_table suppresses SQLite's rewrite of every trigger body that
-- names row_history — every audited table's trigger pair would otherwise be
-- repointed at row_history_old and break the moment it is dropped.
-- ---------------------------------------------------------------------------

PRAGMA legacy_alter_table = ON;
ALTER TABLE row_history RENAME TO row_history_old;
PRAGMA legacy_alter_table = OFF;

CREATE TABLE row_history (
    id         INTEGER PRIMARY KEY AUTOINCREMENT,
    table_name TEXT    NOT NULL CHECK (table_name IN (
                   'trades', 'parcel_allocations', 'income', 'interest_income',
                   'amma_statements', 'amit_adjustments', 'ess_statements',
                   'transfers', 'corporate_actions', 'inheritances',
                   'rights_sales', 'rights_sale_allocations',
                   'investment_expenses', 'drp_enrolments', 'cgt_settings',
                   'attachments', 'listings', 'listing_renames',
                   'closing_prices', 'tax_year_settings')),
    row_id     INTEGER NOT NULL,
    operation  TEXT    NOT NULL CHECK (operation IN ('UPDATE', 'DELETE')),
    changed_at TEXT    NOT NULL,
    old_row    TEXT    NOT NULL
);

INSERT INTO row_history (id, table_name, row_id, operation, changed_at, old_row)
    SELECT id, table_name, row_id, operation, changed_at, old_row
    FROM row_history_old
    ORDER BY id;

-- Drops row_history_old's index and its own append-only guard triggers, both
-- of which moved with the rename; all three are re-created below.
DROP TABLE row_history_old;

CREATE INDEX row_history_row ON row_history (table_name, row_id);

CREATE TRIGGER row_history_append_only_update BEFORE UPDATE ON row_history
BEGIN
    SELECT RAISE(ABORT, 'row_history is append-only');
END;

CREATE TRIGGER row_history_append_only_delete BEFORE DELETE ON row_history
BEGIN
    SELECT RAISE(ABORT, 'row_history is append-only');
END;

-- ---------------------------------------------------------------------------
-- Audit tax_year_settings. Both triggers record every column; row_id is the
-- financial year itself (see the header note).
-- ---------------------------------------------------------------------------

CREATE TRIGGER tax_year_settings_row_history_update AFTER UPDATE ON tax_year_settings
BEGIN
    INSERT INTO row_history (table_name, row_id, operation, changed_at, old_row)
    VALUES ('tax_year_settings', OLD.tax_year, 'UPDATE', strftime('%Y-%m-%dT%H:%M:%fZ', 'now'),
            json_object(
                        'tax_year', OLD.tax_year,
                        'ess_taxed_upfront_reduction_eligible',
                        OLD.ess_taxed_upfront_reduction_eligible));
END;

CREATE TRIGGER tax_year_settings_row_history_delete AFTER DELETE ON tax_year_settings
BEGIN
    INSERT INTO row_history (table_name, row_id, operation, changed_at, old_row)
    VALUES ('tax_year_settings', OLD.tax_year, 'DELETE', strftime('%Y-%m-%dT%H:%M:%fZ', 'now'),
            json_object(
                        'tax_year', OLD.tax_year,
                        'ess_taxed_upfront_reduction_eligible',
                        OLD.ess_taxed_upfront_reduction_eligible));
END;
