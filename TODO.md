# TODO

Items are only marked done when a passing test exists for them.

Completed and decided (out-of-scope / not-reproducible) sections are archived in the topical `DONE/*.md` files, indexed by [DONE.md](DONE.md), to keep this list focused on active work. When a section here is fully done, move it into the matching `DONE/*.md` file rather than leaving it — see CLAUDE.md.

The sections below come from the **2026-07-13 improvement review** (a whole-project pass for
operational and test-strategy gaps, as distinct from the 2026-07-12 programming/domain review whose
findings are all closed in DONE.md), except where a section's heading names another source
(e.g. REQUIREMENTS, SCENARIOS). Each section records one finding; sections land in DONE.md as they
are fixed or decided.

## A duplicated corporate action is silently compounded (SCENARIOS E-03, E-15)
(SCENARIOS.md section E verification pass, 2026-08-16. Two actions of the same type, listing and
date are two independent events to every reader: `db_return_of_capital_events` and
`db_share_split_events` load both, and the pipeline sums / multiplies them.)
- [ ] E-03 — two identical `ReturnOfCapital` rows ($0.50/unit, same date, same listing) reduce a
  100-unit parcel by **$100.00**, not $50.00
- [ ] E-15 — two identical 2-for-1 `ShareSplit` rows on one date turn 100 units into **400**
- [ ] Both are plausible double entries (a re-submitted form, a re-imported statement), both restate
  every cost base and quantity of the listing, and nothing — not the health report, not any
  cross-check — mentions it. Genuine same-day pairs exist in principle (two tranches of a capital
  return), so a hard uniqueness constraint would be wrong; the fit is a health-report warning naming
  the duplicated (listing, type, date), or a confirm step on the second write
- [ ] **Decided 2026-08-16 (Evan): a health-report warning** — one row per duplicated
  (listing, action type, date), non-blocking, so a genuine same-day pair stays enterable. (A UI
  confirm step on the second write, and accepting it as a non-issue, were both considered and
  rejected.) `reports::health` gains the check and `docs/API.md`'s health section the field

## Fractional entitlements are documented for splits and demergers but not for bonus issues or scrip exchanges (SCENARIOS E-11, E-36)
(SCENARIOS.md section E verification pass, 2026-08-16. The convention is consistent in the code —
exact fractional unit counts are kept everywhere, registry rounding and cash-in-lieu are never
modelled — but `docs/API.md` states it only for `ShareSplit` ("a consolidation that doesn't divide a
holding evenly keeps the exact fractional quantity") and `Demerger` ("registry cash-in-lieu of
fractional entitlements are not modelled").)
- [ ] E-11 — a 1-for-10 bonus issue on 105 units reports **115.50** units held, where the registry
  issues 10 and pays cash for the half. The `BonusIssue` bullet lists only partly paid bonus shares
  and call payments as unmodelled
- [ ] E-36 — a 1-for-3 exchange of 101 units creates a replacement parcel of
  **33.666666666666666666666666667** units (now pinned by a test). The `ScripForScrip` bullet lists
  multiple share classes, pre-CGT originals and loss rollovers as unmodelled, but not the fraction
- [ ] Add the same sentence to both bullets, and say what to do with the cash actually received for
  a fraction (it is its own small CGT event on the disposed fraction — the honest answer may be
  "enter it as a Sell of the fractional units", which is worth stating rather than leaving to the
  reader)
