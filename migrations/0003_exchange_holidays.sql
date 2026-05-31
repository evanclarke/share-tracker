-- Exchange public holidays (full-closure non-trading days).
--
-- Settlement is quoted as T+n *business* days: weekends are skipped, and so are
-- the exchange's public holidays. This table records, per exchange (MIC), the
-- dates on which the market is fully closed, so settlement-date calculation can
-- skip them too (see `entities::trade::add_business_days`).
--
-- Only full closures are modelled; early-close days (the market still settles)
-- are not non-trading days and are intentionally excluded. Holidays that fall on
-- a weekend are listed where the official exchange calendar names them on that
-- date; they are harmless for settlement (already skipped as a weekend), and a
-- weekday substitute (where the exchange grants one) is listed on its own date.
--
-- Seeded from the official published calendars: NYSE (NYSE Group 2025-2027
-- holiday schedule, plus 2024 historical) and ASX (cash-market trading calendar,
-- NSW public holidays). Extend as later years are published.

CREATE TABLE exchange_holidays (
    mic          TEXT NOT NULL REFERENCES exchanges(mic),
    holiday_date TEXT NOT NULL,  -- ISO 'YYYY-MM-DD'; a full-closure non-trading day
    name         TEXT NOT NULL,  -- holiday name (informational)
    PRIMARY KEY (mic, holiday_date)
);

-- ASX (XASX) — NSW public holidays observed by the ASX cash market.
INSERT INTO exchange_holidays (mic, holiday_date, name) VALUES
    ('XASX', '2024-01-01', 'New Year''s Day'),
    ('XASX', '2024-01-26', 'Australia Day'),
    ('XASX', '2024-03-29', 'Good Friday'),
    ('XASX', '2024-04-01', 'Easter Monday'),
    ('XASX', '2024-04-25', 'Anzac Day'),
    ('XASX', '2024-06-10', 'King''s Birthday'),
    ('XASX', '2024-12-25', 'Christmas Day'),
    ('XASX', '2024-12-26', 'Boxing Day'),
    ('XASX', '2025-01-01', 'New Year''s Day'),
    ('XASX', '2025-01-27', 'Australia Day'),               -- 26 Jan is a Sunday; observed Mon 27
    ('XASX', '2025-04-18', 'Good Friday'),
    ('XASX', '2025-04-21', 'Easter Monday'),
    ('XASX', '2025-04-25', 'Anzac Day'),
    ('XASX', '2025-06-09', 'King''s Birthday'),
    ('XASX', '2025-12-25', 'Christmas Day'),
    ('XASX', '2025-12-26', 'Boxing Day'),
    ('XASX', '2026-01-01', 'New Year''s Day'),
    ('XASX', '2026-01-26', 'Australia Day'),
    ('XASX', '2026-04-03', 'Good Friday'),
    ('XASX', '2026-04-06', 'Easter Monday'),
    ('XASX', '2026-04-25', 'Anzac Day'),                   -- a Saturday; named on its actual date
    ('XASX', '2026-06-08', 'King''s Birthday'),
    ('XASX', '2026-12-25', 'Christmas Day'),
    ('XASX', '2026-12-28', 'Boxing Day'),                  -- 26 Dec is a Saturday; observed Mon 28
    ('XASX', '2027-01-01', 'New Year''s Day'),
    ('XASX', '2027-01-26', 'Australia Day'),
    ('XASX', '2027-03-26', 'Good Friday'),
    ('XASX', '2027-03-29', 'Easter Monday'),
    ('XASX', '2027-04-25', 'Anzac Day'),                   -- a Sunday; named on its actual date
    ('XASX', '2027-06-14', 'King''s Birthday'),
    ('XASX', '2027-12-27', 'Christmas Day'),               -- 25 Dec is a Saturday; observed Mon 27
    ('XASX', '2027-12-28', 'Boxing Day');                  -- 26 Dec is a Sunday; observed Tue 28

-- NYSE (XNYS) — full market closures (early-close days excluded). A holiday on a
-- Saturday is not observed (no weekday closure); a Sunday holiday is observed the
-- following Monday.
INSERT INTO exchange_holidays (mic, holiday_date, name) VALUES
    ('XNYS', '2024-01-01', 'New Year''s Day'),
    ('XNYS', '2024-01-15', 'Martin Luther King, Jr. Day'),
    ('XNYS', '2024-02-19', 'Washington''s Birthday'),
    ('XNYS', '2024-03-29', 'Good Friday'),
    ('XNYS', '2024-05-27', 'Memorial Day'),
    ('XNYS', '2024-06-19', 'Juneteenth National Independence Day'),
    ('XNYS', '2024-07-04', 'Independence Day'),
    ('XNYS', '2024-09-02', 'Labor Day'),
    ('XNYS', '2024-11-28', 'Thanksgiving Day'),
    ('XNYS', '2024-12-25', 'Christmas Day'),
    ('XNYS', '2025-01-01', 'New Year''s Day'),
    ('XNYS', '2025-01-09', 'National Day of Mourning'),    -- President Jimmy Carter
    ('XNYS', '2025-01-20', 'Martin Luther King, Jr. Day'),
    ('XNYS', '2025-02-17', 'Washington''s Birthday'),
    ('XNYS', '2025-04-18', 'Good Friday'),
    ('XNYS', '2025-05-26', 'Memorial Day'),
    ('XNYS', '2025-06-19', 'Juneteenth National Independence Day'),
    ('XNYS', '2025-07-04', 'Independence Day'),
    ('XNYS', '2025-09-01', 'Labor Day'),
    ('XNYS', '2025-11-27', 'Thanksgiving Day'),
    ('XNYS', '2025-12-25', 'Christmas Day'),
    ('XNYS', '2026-01-01', 'New Year''s Day'),
    ('XNYS', '2026-01-19', 'Martin Luther King, Jr. Day'),
    ('XNYS', '2026-02-16', 'Washington''s Birthday'),
    ('XNYS', '2026-04-03', 'Good Friday'),
    ('XNYS', '2026-05-25', 'Memorial Day'),
    ('XNYS', '2026-06-19', 'Juneteenth National Independence Day'),
    ('XNYS', '2026-09-07', 'Labor Day'),                   -- 4 Jul is a Saturday; not observed
    ('XNYS', '2026-11-26', 'Thanksgiving Day'),
    ('XNYS', '2026-12-25', 'Christmas Day'),
    ('XNYS', '2027-01-01', 'New Year''s Day'),
    ('XNYS', '2027-01-18', 'Martin Luther King, Jr. Day'),
    ('XNYS', '2027-02-15', 'Washington''s Birthday'),
    ('XNYS', '2027-03-26', 'Good Friday'),
    ('XNYS', '2027-05-31', 'Memorial Day'),
    ('XNYS', '2027-06-18', 'Juneteenth National Independence Day'),  -- 19 Jun is a Saturday; observed Fri 18
    ('XNYS', '2027-07-05', 'Independence Day'),            -- 4 Jul is a Sunday; observed Mon 5
    ('XNYS', '2027-09-06', 'Labor Day'),
    ('XNYS', '2027-11-25', 'Thanksgiving Day'),
    ('XNYS', '2027-12-24', 'Christmas Day');               -- 25 Dec is a Saturday; observed Fri 24
