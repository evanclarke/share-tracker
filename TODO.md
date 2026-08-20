# TODO

Items are only marked done when a passing test exists for them.

Completed and decided (out-of-scope / not-reproducible) sections are archived in the topical `DONE/*.md` files, indexed by [DONE.md](DONE.md), to keep this list focused on active work. When a section here is fully done, move it into the matching `DONE/*.md` file rather than leaving it — see CLAUDE.md.

Sections here come from the **2026-07-13 improvement review** (a whole-project pass for
operational and test-strategy gaps, as distinct from the 2026-07-12 programming/domain review whose
findings are all closed in DONE.md), except where a section's heading names another source
(e.g. REQUIREMENTS, SCENARIOS). Each section records one finding; sections land in DONE.md as they
are fixed or decided.

**SCENARIOS.md sections A–O are driven and every finding they raised is closed** in the `DONE/*.md`
archive. Section **P. Tax summary, annual tax report, exports** was driven 2026-08-20: seven of its
twelve scenarios came back correct outright — P-01/P-07 (a year carrying franked + unfranked + CFI +
LIC + TFN dividends, a trust distribution, a foreign-company dividend in USD, a full AMMA statement,
Australian and foreign interest, an ESS statement and an expense reconciled line for line across the
tax summary, the annual tax report's income tables and the CSV export), P-03 (30 June and 1 July
income landing in the right years), P-05 (an AMIT held only part of the year, and sold out before
year end, still asked for its AMMA statement; a year it was not held stays silent), P-06 (a demerger,
an off-market buy-back and a rights sale in one year: the buy-back's capital proceeds are market
value less the dividend with the dividend on its own income row, the rights sale is its own
disposal, and the demerger's closing Sell is correctly *not* a disposal), P-09 (a back-dated return
of capital after generation moves the printed gain and itemises itself as a cost-base adjustment),
P-10 (a 300-row disposal schedule returns all 300 rows, and the print renderer has no pager to
truncate them) and P-11 (tickers as at each taxable event's own date across a rename). The five
findings it raised — the annual tax report's year picker omitting years the report has content for,
a converted fund's pre-AMIT income totalled with no rows behind it and never franking-tested, every
investment-expense deduction exported at `D7 / D8` including the ones the ATO routes to 13Y / 20M /
D15, the parcel optimiser ranking strategies on the 50% discount without stating the taxpayer basis,
and `POST /reports/tax-report` panicking on an out-of-range `tax_year` — were all closed the same
day (see [`DONE/reviews.md`](DONE/reviews.md)).

**Nothing is open.** The next work comes from driving **SCENARIOS.md section Q. Prices, valuation,
and snapshots** the same way — walk its 15 scenarios against the running system, and record each gap
here as its own `## ` section.
