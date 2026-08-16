# TODO

Items are only marked done when a passing test exists for them.

Completed and decided (out-of-scope / not-reproducible) sections are archived in the topical `DONE/*.md` files, indexed by [DONE.md](DONE.md), to keep this list focused on active work. When a section here is fully done, move it into the matching `DONE/*.md` file rather than leaving it — see CLAUDE.md.

The sections below come from the **2026-07-13 improvement review** (a whole-project pass for
operational and test-strategy gaps, as distinct from the 2026-07-12 programming/domain review whose
findings are all closed in DONE.md), except where a section's heading names another source
(e.g. REQUIREMENTS, SCENARIOS). Each section records one finding; sections land in DONE.md as they
are fixed or decided.

## A return of capital on an AMIT listing double-reduces alongside the AMMA adjustment (SCENARIOS E-04)
(SCENARIOS.md section E verification pass, 2026-08-16. For an AMIT the cost-base movement is driven
solely by the AMMA statement's per-unit `cost_base_adjustment` — `docs/API.md` says so in the E4
cross-check section — but nothing stops the same money being entered *again* as a `ReturnOfCapital`
action on the same listing, and the two reductions simply add.)
- [ ] E-04 — reproduced: AMIT listing VDHG, Buy ×100 @ $10, AMMA FY2024 with
  `cost_base_adjustment: 0.50` generated onto the parcel (`amit_cost_base_reduction: 50.00`,
  remaining cost base 950.00), then a `ReturnOfCapital` of $0.50/unit dated 2024-05-01 → `204`, and
  the parcel's remaining cost base drops to **900.00**. `e4_cross_check`, `amit_cash_cross_check`,
  `amit_adjustment_cross_check` and `health` are all empty: nothing sees it
- [ ] **Decided 2026-08-16 (Evan): refuse it at write time** — a `ReturnOfCapital` on a listing with
  `amit = 1` answers `422` pointing at the AMMA statement's `cost_base_adjustment` as the place the
  reduction belongs. (The alternatives considered and rejected: a non-blocking cross-check row, or
  documenting it as the user's own call.) The refusal needs the usual sweep: the error variant and
  its 422 body beside `WriteError`, `docs/API.md`'s corporate-actions 422 catalogue, and a note in
  the AMIT/AMMA sections saying the two paths are mutually exclusive
- [ ] Note the asymmetry that makes the refusal tempting: the income-row path already refuses the
  same double entry — `tax_deferred_amount` on a non-trust income row is a 422 telling the user to
  record a `ReturnOfCapital` instead — so the corporate-action side is the only unguarded door

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
