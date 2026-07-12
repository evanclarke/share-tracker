-- Foreign-source interest classification (REQUIREMENTS 2026-07-13 — resolves
-- the "Foreign broker-cash interest classification" Known limitation).
--
-- docs/ato/tax-return-labels-2026.md: interest-like income on foreign broker
-- cash / money-market sweep funds (e.g. a US broker's Treasury liquidity fund
-- distributions) is assessable foreign source income (question 20, label 20E),
-- not Australian gross interest (question 10, label 10L), and foreign tax
-- withheld from it is claimed via the question 20 FITO (20O). An interest row
-- now records whether its payer is foreign-source and any foreign tax
-- withheld; the tax summary routes the row to the matching lines
-- (`foreign_interest_income` / `foreign_tax_offsets` vs `interest_income`).
--
-- Existing rows default to Australian-source (0) with no foreign tax — the
-- previous behaviour, so nothing is reclassified by the migration itself.
--
-- No snapshot-staleness triggers: as with 0008_interest_income.sql, the only
-- report reading this table is the tax summary, which is not snapshotted.
ALTER TABLE interest_income ADD COLUMN foreign_source INTEGER NOT NULL DEFAULT 0
    CHECK (foreign_source IN (0, 1));
-- Foreign tax withheld from the gross amount, in `currency`; joins the tax
-- summary's FITO line (subject to the A$1,000 de-minimis,
-- docs/ato/fito-limit.md). Foreign-source rows only, never negative (CHECK;
-- Australian-source writes also rejected 422 with a fuller message).
ALTER TABLE interest_income ADD COLUMN foreign_tax_paid TEXT NOT NULL DEFAULT '0'
    CHECK (CAST(foreign_tax_paid AS NUMERIC) >= 0
           AND (foreign_source = 1 OR CAST(foreign_tax_paid AS NUMERIC) = 0));
