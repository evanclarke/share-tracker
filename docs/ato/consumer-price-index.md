# Appendix 2 Consumer price index (CPI)

> **Source:** https://www.ato.gov.au/forms-and-instructions/guide-to-capital-gains-tax-2025/appendixes/appendix-2-consumer-price-index-cpi
> (Guide to capital gains tax 2025 — "Appendix 2 Consumer price index (CPI)", QC 104764,
> published 29 May 2025)
> **Retrieved:** 2026-08-25
> The live ATO site is authoritative; this is a convenience mirror.

Shows the consumer price index from 1985 to 1999 for each quarter.

## All groups: weighted average of 8 capital cities

| Year | Quarter ending 31 Mar | Quarter ending 30 Jun | Quarter ending 30 Sep | Quarter ending 31 Dec |
| --- | ---: | ---: | ---: | ---: |
| 1985 | – | – | 39.7 | 40.5 |
| 1986 | 41.4 | 42.1 | 43.2 | 44.4 |
| 1987 | 45.3 | 46.0 | 46.8 | 47.6 |
| 1988 | 48.4 | 49.3 | 50.2 | 51.2 |
| 1989 | 51.7 | 53.0 | 54.2 | 55.2 |
| 1990 | 56.2 | 57.1 | 57.5 | 59.0 |
| 1991 | 58.9 | 59.0 | 59.3 | 59.9 |
| 1992 | 59.9 | 59.7 | 59.8 | 60.1 |
| 1993 | 60.6 | 60.8 | 61.1 | 61.2 |
| 1994 | 61.5 | 61.9 | 62.3 | 62.8 |
| 1995 | 63.8 | 64.7 | 65.5 | 66.0 |
| 1996 | 66.2 | 66.7 | 66.9 | 67.0 |
| 1997 | 67.1 | 66.9 | 66.6 | 66.8 |
| 1998 | 67.0 | 67.4 | 67.5 | 67.8 |
| 1999 | 67.8 | 68.1 | 68.7 | n/a (see Note 1) |

For an explanation of indexation and how it applies, see *The indexation method*.

**Note 1:** If you use the indexation method to calculate your capital gain, the
indexation factor is based on increases in the CPI up to September 1999 only.

---

**How this project uses it:** seeded, verbatim and immutable, as the `cpi_quarters`
reference table (migration `0046_cpi_quarters.sql`) — 57 rows, the September 1985
quarter through the September 1999 quarter. It feeds the **indexed cost base**
shown by `/reports/indexation_cross_check` beside the discount figure. **No reported
tax figure is computed from it**: the net capital gain, the tax summary, the Annual
Tax Report and every CSV export stay on the 50% discount throughout (see
[`indexing-the-cost-base.md`](indexing-the-cost-base.md) for the method, and the
*Indexation method* entry in `docs/API.md`'s Known limitations for the scope cut).

**On the reference base.** The ATO's general
[Consumer price index (CPI) rates](https://www.ato.gov.au/tax-rates-and-codes/consumer-price-index)
page (QC 16141) carries the same figures in its current table — the ABS moved the
index reference base from 1989–90 to 2011–12 in September 2012 — and additionally
carries the superseded 1989–90-base series under "Historic rates", marked "You can
no longer use the CPI rates in this table for tax and super purposes". **The two
bases do not always give the same factor to 3 decimal places**: for the September
1985 quarter the current base gives 68.7 ÷ 39.7 = 1.730 while the historic base
gives 123.4 ÷ 71.3 = 1.731. This project uses the figures above — the current base,
which is the one [`indexing-the-cost-base.md`](indexing-the-cost-base.md)'s stated
method reads on (its divisor, 68.7, is the current-base September 1999 figure) and
the one this appendix publishes for CGT.
