# TODO

Items are only marked done when a passing test exists for them.

Completed and decided (out-of-scope / not-reproducible) sections are archived in the topical `DONE/*.md` files, indexed by [DONE.md](DONE.md), to keep this list focused on active work. When a section here is fully done, move it into the matching `DONE/*.md` file rather than leaving it — see CLAUDE.md.

Sections here come from the **2026-07-13 improvement review** (a whole-project pass for
operational and test-strategy gaps, as distinct from the 2026-07-12 programming/domain review whose
findings are all closed in DONE.md), except where a section's heading names another source
(e.g. REQUIREMENTS, SCENARIOS). Each section records one finding; sections land in DONE.md as they
are fixed or decided.

**SCENARIOS.md sections A–K are driven and every finding they raised is closed** in the `DONE/*.md`
archive. **Section L. Crypto** was driven 2026-08-18; the six findings below are its open work. When
they are closed, the next work comes from driving **SCENARIOS.md section M. Foreign currency and
FX** the same way — walk its scenarios against the running system, and record each gap here as its
own `## ` section.

## Staking rewards and airdropped tokens are reported as dividends (SCENARIOS L-03, L-04)
(SCENARIOS.md section L verification pass, 2026-08-18. The ATO is explicit: the money value of
staking rewards, and of an **established** token received by airdrop, is **ordinary income at the
time of receipt**, declared "as **other income**" — item 24 of the individual return, not item 11
(QC 69950, "Staking rewards and airdrops"; the same page's Anastasia and Merindah examples). The
documented workaround — README + `docs/API.md` Known limitations, "an income row plus a Buy at
receipt-date market value" — has nowhere to put that income: `income.income_type` is
`Dividend | EmploymentIncome`, so the row is a dividend unless it is remuneration.)
- [ ] Reproduced: 0.5 ETH of staking rewards worth A$2,000 entered as an income row on the ETH
  listing → `GET /portfolio/tax-summary` reports `dividends_assessable: "2000"` against ATO label
  **`11S + 11T`**, and the annual tax report prints it in the **Dividends** table with
  `franking_status: "entitled"` — a franking entitlement on a payment no company made. The total
  assessable income is right; every label on it is wrong
- [ ] `income_type: "EmploymentIncome"` is no better: it reports on the tax summary's
  `employment_income` line, which `docs/API.md` describes as item 1/2 salary and wages. Staking
  rewards are neither a distribution of a holding nor remuneration for services
- [ ] The cost-base half is already right: the reward tokens entered as a Buy at receipt-date market
  value open a parcel at that value with its own 12-month clock, exactly as the ATO states, and the
  later sale reports correctly (verified). This finding is only about where the *income* lands
- [ ] Precedent: J-10 (`1d76d3f`) is this finding one income type earlier — the dividend-equivalent
  workaround reported remuneration at 11S, and Evan chose the `income_type` enum over sharpening the
  wording. The same choice is open here (a third variant reported on its own line and in its own
  annual-tax-report table, against item 24), against the cheaper alternative of documenting that
  crypto income must be carried to item 24 by hand
- [x] The ATO page is now mirrored: `docs/ato/crypto-staking-airdrops.md`, indexed in `docs/ato/OVERVIEW.md` (closed 2026-08-18 with the crypto entry-path documentation finding)
- [ ] **Decided 2026-08-18: Evan chose the third `income_type` variant** — reported on its own tax-summary line against item 24 and in its own annual-tax-report table, out of every dividend total
- [ ] Tests: a staking-reward row reports at its own label, is in no dividend total, and carries no
  franking status; the annual tax report prints it in its own table
- [ ] Docs sync: `docs/API.md` Income (`income_type`) + the tax-summary/annual-tax-report field
  tables, README Features / Known limitations
