# TODO

Items are only marked done when a passing test exists for them.

Completed and decided (out-of-scope / not-reproducible) sections are archived in the topical `DONE/*.md` files, indexed by [DONE.md](DONE.md), to keep this list focused on active work. When a section here is fully done, move it into the matching `DONE/*.md` file rather than leaving it — see CLAUDE.md.

The sections below come from the **2026-07-13 improvement review** (a whole-project pass for
operational and test-strategy gaps, as distinct from the 2026-07-12 programming/domain review whose
findings are all closed in DONE.md), except where a section's heading names another source
(e.g. REQUIREMENTS, SCENARIOS). Each section records one finding; sections land in DONE.md as they
are fixed or decided.

## A return of capital in a currency other than its parcels' is accepted, then breaks every cost-base report (SCENARIOS E-07, E-39)
(SCENARIOS.md section E verification pass, 2026-08-16. `RocEvent::per_unit_for`
(`src/entities/corporate_action/adjustments.rs:74`) refuses — correctly — to net a payment against a
parcel in another currency, and raises `sqlx::Error::Decode`. Nothing checks the currency at
**write** time (`corporate_action::db_upsert`, `src/entities/corporate_action/db.rs:153`, validates
the payload's shape and the currency's existence, not its agreement with the listing's parcels), so
the mismatch is only discovered when a report reads it — as an `ApiError::Internal`, i.e. `500` with
an **empty body**.)
- [ ] E-07 — reproduced: listing AAA (AUD), Buy ×100 @ $10, then
  `PUT /corporate_actions/1 {"action_type":"ReturnOfCapital","currency":"USD",…}` → `204`. From that
  moment `GET /portfolio/open-parcels`, `POST /portfolio/overview`, unrealised gains, realised gains,
  net capital gain, the annual tax report and snapshot generation all answer `500` with no body — the
  web UI can only show "HTTP 500". Nothing names the action, and no cross-check or health row points
  at it
- [ ] E-39 — the same trap without a typo: exchange an AUD holding into a **USD-listed** replacement
  (a scrip-for-scrip replacement parcel deliberately keeps the *original's* currency, `docs/API.md`),
  then record the replacement listing's own return of capital in USD — its listed currency, the
  obvious entry — and every parcel report dies the same way
- [ ] The precedent is B-02's brokerage-currency mismatch, refused at write time with a 422 naming
  the reason (`c7d7137`): the same fix shape applies here — compare the payment's currency against
  the currencies of the listing's Buy/DRP parcels inside the write transaction (and, symmetrically,
  refuse a Buy whose currency contradicts an existing payment on the listing, or the hole reopens
  from the other side)
- [ ] `docs/API.md` currently documents the 500 ("A payment's `currency` must match the affected
  trades' currency — the reports never net amounts across currencies and fail loudly (`500`)"). If
  the write-time refusal lands, that sentence becomes the 422 instead

## A corporate action dated in the future is applied to today's holdings (SCENARIOS E-14)
(SCENARIOS.md section E verification pass, 2026-08-16. `domain::open_parcels::load(conn, None)`
resolves its cutoff with `as_of_or_open` (`src/domain/open_parcels.rs:112`), i.e. the `9999-12-31`
sentinel, so the *live* view means "every recorded fact" rather than "everything up to today". A
split or return of capital recorded ahead of its effective date — normal practice, the terms are
announced weeks before they take effect — is therefore already in force in every report built on
that call, while the as-of-dated reports correctly ignore it.)
- [ ] E-14 — reproduced: Buy ×100 on 2023-01-10, `ShareSplit` 2-for-1 dated **2030-03-01**.
  `GET /portfolio/open-parcels` and `POST /portfolio/overview` report **200 units** (market value
  $2,000 at $10) today, in 2026; `POST /portfolio/unrealised-gains` for the same day reports **100**
  ($1,000). Two reports, one database, one day, two answers
- [ ] E-14b — the same with a `ReturnOfCapital` of $1.00/unit dated 2030-03-01: open parcels report
  `return_of_capital_reduction: 100.00` and `remaining_cost_base: 900.00` today, the overview's
  `total_cost_base` follows, and the **parcel optimiser** (`POST /portfolio/parcel-optimiser`,
  `src/reports/parcel_optimiser.rs:109`) prices a contemplated sale off the reduced $9.00/unit,
  overstating the gain on every candidate strategy. Unrealised gains still show $1,000
- [ ] The write paths are consistent with the *correct* reading — a Sell entered today is validated
  and costed against the pre-split basis — so it is only the live read that disagrees, which is what
  makes it silent
- [ ] Fix shape: `load(conn, None)` (and `portfolio::db_holdings(pool, None)`,
  `open_parcels::db_open_parcels`) should bound at today rather than at the sentinel, so "live" means
  "as at today" everywhere; a future-dated fact then appears when it takes effect. Decide whether a
  future-dated Buy/Sell should equally drop out of the live view (it would, under the same change) or
  whether corporate actions alone are bounded
- [ ] Alternative if the bound is unwanted: refuse a corporate action dated after today at write time
  — but that removes a legitimate entry (recording the terms on announcement), so bounding the read
  is the better half

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
- [ ] Decide the model: refuse a `ReturnOfCapital` on a listing with `amit = 1` (422, pointing at the
  AMMA statement), or flag it in a cross-check the way an unmatched tax-deferred amount is flagged
  today, or document it as the user's own call. The E4 cross-check already models the *inverse*
  direction for non-AMIT trusts, so the shape exists either way
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
- [ ] Decide which, or record it as accepted: this is a "flag it" candidate rather than a
  correctness bug in the arithmetic

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
