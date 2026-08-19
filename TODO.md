# TODO

Items are only marked done when a passing test exists for them.

Completed and decided (out-of-scope / not-reproducible) sections are archived in the topical `DONE/*.md` files, indexed by [DONE.md](DONE.md), to keep this list focused on active work. When a section here is fully done, move it into the matching `DONE/*.md` file rather than leaving it — see CLAUDE.md.

Sections here come from the **2026-07-13 improvement review** (a whole-project pass for
operational and test-strategy gaps, as distinct from the 2026-07-12 programming/domain review whose
findings are all closed in DONE.md), except where a section's heading names another source
(e.g. REQUIREMENTS, SCENARIOS). Each section records one finding; sections land in DONE.md as they
are fixed or decided.

**SCENARIOS.md sections A–M are driven and every finding they raised is closed** in the `DONE/*.md`
archive — section M. Foreign currency and FX was driven 2026-08-19, raised eight findings, and all
eight were closed the same day (see [`DONE/reviews.md`](DONE/reviews.md)).

One residual scope question is open below. Beyond it, the next work comes from driving
**SCENARIOS.md section N. Holding accounts and transfers** the same way — walk its scenarios against
the running system, and record each gap here as its own `## ` section.

## Foreign tax on a directly-realised foreign capital gain has nowhere to be recorded (SCENARIOS M-12)
(Split off from the section M finding that added `amma_statements.foreign_tax_credits_capital_gains`,
2026-08-19 — that fix covers the AMIT/MIT distribution path, which is where a listed-share investor
actually meets a foreign-taxed capital gain. This is the other path.)
- [ ] Foreign tax paid on a capital gain the taxpayer realises **themselves** — a disposal the
  foreign country taxes — has no field anywhere: `income.foreign_tax_paid` sits on an income row,
  and a Sell carries no foreign-tax column. So such an amount cannot reach the FITO line at all,
  where the AMMA path now reaches it apportioned
- [ ] Narrower than it looks, which is why it was split rather than fixed: a foreign country rarely
  taxes a non-resident's gain on listed shares (the usual treaty position), so the case arises for
  foreign *real property* and similar assets this system does not record in the first place
- [ ] A model decision either way:
  - **(a)** A `foreign_tax_paid` column on Sells (audited table ⇒ its two `*_row_history_*` triggers
    re-created), apportioned by the same `apportion_capital_gains_foreign_tax` rule against that
    disposal's own discount eligibility. Small, and symmetrical with the AMMA side
  - **(b)** A Known-limitations entry saying the direct path is not recordable, and that such a
    taxpayer claims it outside this tool
- [ ] Tests / docs sync per the option
