# TODO

Items are only marked done when a passing test exists for them.

Completed and decided (out-of-scope / not-reproducible) sections are archived in the topical `DONE/*.md` files, indexed by [DONE.md](DONE.md), to keep this list focused on active work. When a section here is fully done, move it into the matching `DONE/*.md` file rather than leaving it — see CLAUDE.md.

Sections here come from the **2026-07-13 improvement review** (a whole-project pass for
operational and test-strategy gaps, as distinct from the 2026-07-12 programming/domain review whose
findings are all closed in DONE.md), except where a section's heading names another source
(e.g. REQUIREMENTS, SCENARIOS). Each section records one finding; sections land in DONE.md as they
are fixed or decided.

**SCENARIOS.md sections A–S are driven and every finding they raised is closed** in the `DONE/*.md`
archive. Section **S. Settlement, holidays, and dates** was driven 2026-08-22 (`d501408`) and its
four findings closed by `67c3096` (a trade dated in the future), `30d0e96` (a trade dated on a day
its exchange was shut), `4a7ef1a` (a stored settlement date that is not a trading day) and
`e453f21` (the settlement dates a completed calendar changes) — all four archived in
[`DONE/trades-income.md`](DONE/trades-income.md), and summarised with the rest of the pass under
[Section S findings](SCENARIOS.md#section-s-findings). Every section's row in SCENARIOS.md's
[Verification status](SCENARIOS.md#verification-status) table names the pass that drove it and where
its findings went; that table is the record of what has been looked at.

**Nothing is open here.** The next work is the next SCENARIOS pass — section **T. Jobs, backup, and
operations** (12 scenarios) — driven the way S was: run every scenario against a throwaway database,
apply the standing probes to each, and log what each raises as a `## SCENARIOS T-nn` section here
with the option Evan chose. Two lessons from S are worth carrying into it. First, **check the live
database read-only before proposing a refusal** (the M lesson, which is what turned S-05 from a
refusal into a flag — trade 9071 would have been bricked). Second, **a decision can rest on a
distinction the schema cannot make**: S-04's "rewrite the auto-computed settlement dates, leave the
stated ones" was unanswerable until a provenance column recorded which path wrote each date, and the
finding's own write-up never mentioned it — so before implementing, check that the data actually
says what the decision assumes it says.
