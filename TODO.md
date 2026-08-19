# TODO

Items are only marked done when a passing test exists for them.

Completed and decided (out-of-scope / not-reproducible) sections are archived in the topical `DONE/*.md` files, indexed by [DONE.md](DONE.md), to keep this list focused on active work. When a section here is fully done, move it into the matching `DONE/*.md` file rather than leaving it — see CLAUDE.md.

Sections here come from the **2026-07-13 improvement review** (a whole-project pass for
operational and test-strategy gaps, as distinct from the 2026-07-12 programming/domain review whose
findings are all closed in DONE.md), except where a section's heading names another source
(e.g. REQUIREMENTS, SCENARIOS). Each section records one finding; sections land in DONE.md as they
are fixed or decided.

**SCENARIOS.md sections A–O are driven and every finding they raised is closed** in the `DONE/*.md`
archive. Section **O. Net capital gain, losses, and carry-forward** was driven 2026-08-19: eleven of
its seventeen scenarios came back correct outright and the netting arithmetic was right in the other
six too, and the three findings the pass raised — a carried-forward loss invisible in a year with no
CGT activity of its own, both pre-sale tools modelling a disposal dated before the parcels existed,
and the what-if's over-request refusal not naming the account it was scoped to — were all closed the
same day (see [`DONE/reviews.md`](DONE/reviews.md)).

**Nothing is open.** The next work comes from driving **SCENARIOS.md section P. Tax summary, annual
tax report, exports** the same way — walk its 12 scenarios against the running system, and record
each gap here as its own `## ` section.
