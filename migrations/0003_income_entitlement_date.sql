-- Trust distributions are assessed in the income year of *present entitlement*,
-- regardless of when the cash is paid (ATO QC 23087, docs/ato/trust-income-timing.md)
-- — a June distribution paid in mid-July belongs to the FY just ended. Dividends
-- stay assessed by payment, so the date is only meaningful on trust rows (CHECK).
-- When present on a trust row, the tax summary attributes every component of the
-- row (FY bucket and AUD-conversion month) by this date instead of date_paid;
-- absent, date_paid behaviour is unchanged.
ALTER TABLE income ADD COLUMN entitlement_date TEXT
    CHECK (entitlement_date IS NULL OR trust_income = 1);
