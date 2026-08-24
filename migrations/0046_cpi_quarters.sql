-- The frozen ATO quarterly CPI series behind the indexation method
-- (SCENARIOS AA-a).
--
-- For a CGT asset whose costs were incurred by 21 September 1999 an individual
-- may index those costs for inflation *instead of* applying the 50% CGT
-- discount — never both, and never on a capital loss
-- (docs/ato/indexing-the-cost-base.md, QC 66024). The indexation factor is
-- 68.7 (the CPI for the quarter ending 30 September 1999, where indexation is
-- frozen) divided by the CPI for the quarter in which the cost was incurred,
-- limited to 3 decimal places with the fourth decimal rounded up from 5.
--
-- This table is that CPI series, seeded verbatim from Appendix 2 of the ATO's
-- Guide to capital gains tax 2025 (QC 104764), mirrored in
-- docs/ato/consumer-price-index.md: 57 rows, the September 1985 quarter (the
-- first one CGT reaches — CGT starts 20 September 1985) through the September
-- 1999 quarter (the freeze). There is deliberately no row after it: a later
-- quarter's CPI is a real number the ABS publishes, but it is not a number the
-- indexation method may use, so storing one would invite a factor the law does
-- not allow. The figures are on the ABS's current (2011-12) index reference
-- base, which is the base the ATO's own stated method reads on — its divisor,
-- 68.7, is the current-base September 1999 figure. The superseded 1989-90-base
-- series is deliberately not stored; it gives a different factor at the third
-- decimal for some quarters (September 1985: 1.730 here against 1.731 there),
-- and the ATO marks it "no longer [usable] for tax and super purposes".
--
-- **No reported tax figure is computed from this table.** The net capital
-- gain, the tax summary, the Annual Tax Report and every CSV export stay on
-- the 50% discount throughout; the indexed cost base exists only so
-- /reports/indexation_cross_check can show a taxpayer which method would have
-- given the better result on each affected disposal, and so realised gains can
-- carry the same figure beside the discount one. Modelling the *election* —
-- choosing per parcel, with the choice interacting with the loss-netting walk
-- — is deliberately out of scope (docs/API.md, Known limitations).
--
-- Snapshot staleness: exempt. Nothing writes to this table after this
-- migration (no entity module, no route, no import job), and no snapshotted
-- report reads it — the three snapshotted reports are the price-dependent ones
-- (valuation, portfolio, unrealised gains) and the indexed cost base reaches
-- none of them. Recorded in reports::snapshot::STALENESS_EXEMPT_TABLES.
--
-- The audit trail: deliberately **not** audited. row_history exists to make an
-- UPDATE or DELETE of a *financial fact the user entered* recoverable; this
-- table holds published reference data with no write path at all, so its
-- triggers could only ever fire for someone editing the database by hand
-- behind the application — and the recovery for that is this migration, which
-- states every figure. Contrast exchange_holidays (0039), which is audited
-- precisely because it has a DELETE route.

CREATE TABLE cpi_quarters (
    -- ISO 'YYYY-MM-DD' date of the quarter's end — the quarter a cost was
    -- incurred in is the one whose end date this row carries. Bounded to the
    -- indexable range at both ends, and to the four quarter-end dates, so a
    -- typo cannot introduce a quarter the method has no CPI for.
    quarter_end TEXT PRIMARY KEY
                CHECK (quarter_end BETWEEN '1985-09-30' AND '1999-09-30')
                CHECK (substr(quarter_end, 6) IN ('03-31', '06-30', '09-30', '12-31')),
    -- Decimal: All groups CPI, weighted average of 8 capital cities, on the
    -- ABS's 2011-12 index reference base (one decimal place, as published).
    cpi         TEXT NOT NULL
);

-- Appendix 2 Consumer price index (CPI), Guide to capital gains tax 2025
-- (QC 104764). March and June 1985 are absent from the ATO's own table: CGT
-- starts 20 September 1985, so no indexable cost predates that quarter.
INSERT INTO cpi_quarters (quarter_end, cpi) VALUES
    ('1985-09-30', '39.7'),
    ('1985-12-31', '40.5'),
    ('1986-03-31', '41.4'),
    ('1986-06-30', '42.1'),
    ('1986-09-30', '43.2'),
    ('1986-12-31', '44.4'),
    ('1987-03-31', '45.3'),
    ('1987-06-30', '46.0'),
    ('1987-09-30', '46.8'),
    ('1987-12-31', '47.6'),
    ('1988-03-31', '48.4'),
    ('1988-06-30', '49.3'),
    ('1988-09-30', '50.2'),
    ('1988-12-31', '51.2'),
    ('1989-03-31', '51.7'),
    ('1989-06-30', '53.0'),
    ('1989-09-30', '54.2'),
    ('1989-12-31', '55.2'),
    ('1990-03-31', '56.2'),
    ('1990-06-30', '57.1'),
    ('1990-09-30', '57.5'),
    ('1990-12-31', '59.0'),
    ('1991-03-31', '58.9'),
    ('1991-06-30', '59.0'),
    ('1991-09-30', '59.3'),
    ('1991-12-31', '59.9'),
    ('1992-03-31', '59.9'),
    ('1992-06-30', '59.7'),
    ('1992-09-30', '59.8'),
    ('1992-12-31', '60.1'),
    ('1993-03-31', '60.6'),
    ('1993-06-30', '60.8'),
    ('1993-09-30', '61.1'),
    ('1993-12-31', '61.2'),
    ('1994-03-31', '61.5'),
    ('1994-06-30', '61.9'),
    ('1994-09-30', '62.3'),
    ('1994-12-31', '62.8'),
    ('1995-03-31', '63.8'),
    ('1995-06-30', '64.7'),
    ('1995-09-30', '65.5'),
    ('1995-12-31', '66.0'),
    ('1996-03-31', '66.2'),
    ('1996-06-30', '66.7'),
    ('1996-09-30', '66.9'),
    ('1996-12-31', '67.0'),
    ('1997-03-31', '67.1'),
    ('1997-06-30', '66.9'),
    ('1997-09-30', '66.6'),
    ('1997-12-31', '66.8'),
    ('1998-03-31', '67.0'),
    ('1998-06-30', '67.4'),
    ('1998-09-30', '67.5'),
    ('1998-12-31', '67.8'),
    ('1999-03-31', '67.8'),
    ('1999-06-30', '68.1'),
    ('1999-09-30', '68.7');
