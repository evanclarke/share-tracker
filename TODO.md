# TODO

Items are only marked done when a passing test exists for them.

This file holds only open / in-flight work. Completed and decided (out-of-scope / not-reproducible)
sections are archived in the topical `DONE/*.md` files, indexed by [DONE.md](DONE.md). When a
section here is fully done, move it into the matching `DONE/*.md` file rather than leaving it — see
CLAUDE.md.

A section records one finding, and its heading names where it came from — a REQUIREMENTS entry, a
[SCENARIOS.md](SCENARIOS.md) section, or a dated review pass.

**Open: nothing.** Every section recorded so far is closed and archived — the last was the
annual tax report's foreign income totals (a REQUIREMENTS entry, closed 2026-08-28 and
moved to [`DONE/reporting.md`](DONE/reporting.md)), and before it the
2026-08-28 cyclomatic complexity audit, whose six items (the `tax_summary` split, the two nesting
outliers, the `rights_sale` anchoring walk, the `corporate_action` presence flags, the
`upsert_sell_in_tx` parameters struct, and the decision not to gate complexity in CI) closed on
2026-08-28 and moved to [`DONE/reviews.md`](DONE/reviews.md). The closing narrative that used to
stand here — the pass-by-pass record of driving SCENARIOS.md sections S through AA, and the last two
sections to close before that audit (the distribution calendar and the 2026-08-25 code review) — was
moved to [`DONE/verification-passes.md`](DONE/verification-passes.md) on 2026-08-28. The maintained
record of what has been verified is SCENARIOS.md's
[Verification status](SCENARIOS.md#verification-status) table and its per-section findings blocks;
the maintained record of what was built and decided is the `DONE/*.md` archive.
