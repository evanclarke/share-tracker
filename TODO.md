# TODO

Items are only marked done when a passing test exists for them.

Completed and decided (out-of-scope / not-reproducible) sections are archived in the topical `DONE/*.md` files, indexed by [DONE.md](DONE.md), to keep this list focused on active work. When a section here is fully done, move it into the matching `DONE/*.md` file rather than leaving it — see CLAUDE.md.

Sections here come from the **2026-07-13 improvement review** (a whole-project pass for
operational and test-strategy gaps, as distinct from the 2026-07-12 programming/domain review whose
findings are all closed in DONE.md), except where a section's heading names another source
(e.g. REQUIREMENTS, SCENARIOS). Each section records one finding; sections land in DONE.md as they
are fixed or decided.

**SCENARIOS.md sections A–R are driven and every finding they raised is closed** in the `DONE/*.md`
archive. Section **R. Listing identity and renames** was driven 2026-08-21 against a throwaway
database with the real price provider: all ten scenarios behaved as designed on their own terms, and
the eight findings the pass raised — a rename applied before its effective date, an undo that put
back only two of the four fields it overwrote, a rename onto an exchange quoting a currency the
listing could no longer be given, a ticker an exchange reissues to another company (documented as a
Known limitation), a dead provider symbol diagnosed only on the empty-candle path, a ledger reading
today's ticker where the docs say it reads the row's own date, the whole feature having no web UI,
and a ticker collision answered with the raw SQLite constraint text — are all now closed (see
[`DONE/reference-data.md`](DONE/reference-data.md),
[`DONE/reviews.md`](DONE/reviews.md) and [`DONE/web-frontend.md`](DONE/web-frontend.md)).

**Nothing is open.** The next work comes from driving **SCENARIOS.md section S. Settlement,
holidays, and dates** the same way — walk its scenarios against the running system, and record each
gap here as its own `## ` section.
