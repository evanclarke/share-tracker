# TODO

Items are only marked done when a passing test exists for them.

Completed and decided (out-of-scope / not-reproducible) sections are archived in the topical `DONE/*.md` files, indexed by [DONE.md](DONE.md), to keep this list focused on active work. When a section here is fully done, move it into the matching `DONE/*.md` file rather than leaving it — see CLAUDE.md.

The sections below come from the **2026-07-13 improvement review** (a whole-project pass for
operational and test-strategy gaps, as distinct from the 2026-07-12 programming/domain review whose
findings are all closed in DONE.md), except where a section's heading names another source
(e.g. REQUIREMENTS, SCENARIOS). Each section records one finding; sections land in DONE.md as they
are fixed or decided.

## An AMMA statement for a year with nothing held at 30 June cannot be generated, and its hand-entered set is flagged forever (SCENARIOS F-04, F-17, F-25)
(SCENARIOS.md section F verification pass, 2026-08-16. `db_generate` refuses with `NothingHeld`
when no parcel of the listing is open at the statement's `tax_year_end_date`
(`src/entities/amit_adjustment_generation.rs:141`), and the cross-check's coverage rule compares Σ
of the adjustment quantities against the statement's `units_held`
(`src/reports/amit_adjustment_cross_check.rs:207`). Both are right for the case they were written
for — a statement whose parcels have not been entered yet — and both misfire on the *correct*
holding that was fully sold, or transferred out, during the statement's year. The reduction itself
is right once entered by hand: `AmitReductionEvent::reduction_for_units` spills a whole-parcel row
onto the units sold during the year, which is what LCR 2015/11 para 13 requires.)
- [ ] F-04 — reproduced: Buy ×1000 Aug 2024, sold in full 1 Mar 2025, FY2025 statement stating 0
  units held and 0.20 per unit. `POST /amma_statements/1/generate_adjustments` → `422` "no parcels
  of the statement's listing were held in its holding account at the statement's year end — **enter
  the missing trades first**", which is the one thing the user must not do here: the trades are all
  entered and correct
- [ ] The hand-entered row is accepted (`PUT /amit_adjustments/1` with `quantity` 1000 → `204`) and
  reduces the sale's cost base correctly (49,800 from 50,000 — the sale's gain rises by exactly
  1000 × 0.20). But the cross-check then reports the statement forever: "adjusted units 1000 do not
  match the statement's units held 0 (difference +1000) — a parcel is missing, duplicated, or
  covered for the wrong quantity". An honest, complete entry cannot be made to reconcile
- [ ] F-25 shows the same path is the *normal* one for the year of sale: a multi-year holding sold
  in November has its FY-of-sale AMMA arrive the following September, and that statement always has
  0 units held. F-17 hits it from the other side: after a mid-year transfer, the sending account's
  statement has nothing open in that account at 30 June
- [ ] **Model question for Evan.** (a) Let generation cover parcels held *during* the year when
  none is open at year end — one row per parcel the listing had open at any point in the FY, each
  covering the units it held (this is a real extension: the row quantity for a partly-sold parcel
  would have to be the units held during the year, not the units remaining); (b) keep the refusal
  but re-word it, and teach the coverage check that a statement stating fewer units than were
  adjusted is expected when the difference is units disposed of during the statement's year;
  (c) document the manual path. (b) is the smaller change and fixes both misfires

## An AMIT adjustment on a parcel closed by a transfer is accepted and reduces nothing (SCENARIOS F-17)
(SCENARIOS.md section F verification pass, 2026-08-16. A transfer closes the source parcel and
writes a replacement Buy carrying the cost base forward as a frozen figure
(`domain::rollover::insert_replacement_buy`, `src/domain/rollover.rs:255`) — so an AMIT adjustment
written against the *original* parcel afterwards reaches nothing: the parcel is fully consumed, so
no open-holdings report shows it, and the transfer's closing Sell is not a disposal, so no realised
gain nets it off. `amit_adjustment::db_upsert_on` checks the trade type, listing, holding account,
quantity and duplication — not whether the parcel still exists in any reachable form.)
- [ ] F-17 — reproduced: Buy ×1000 @ $50 in account 1, transferred whole to account 2 on
  1 Feb 2025, then the sending account's FY2025 statement (0.20/unit) applied by hand to the
  original parcel (trade 10) → `204`. `GET /portfolio/open-parcels` still shows the replacement
  parcel at `amit_cost_base_reduction` 0 and `remaining_cost_base` 50,000; realised gains is empty;
  net capital gain is all zeroes. The $200 reduction is simply gone
- [ ] The receiving account's own statement is fine — it covers the replacement parcel, which is
  the case pinned by
  `amit_adjustment_generation::db_a_parcel_transferred_mid_year_is_covered_in_its_new_account`
- [ ] The same shape applies to any parcel-substituting operation (`domain::rollover` also backs
  scrip-for-scrip and demergers), and to any AMIT adjustment entered *after* one of them: the
  replacement's cost base was fixed when the operation ran
- [ ] **Model question for Evan.** (a) Refuse an adjustment against a parcel that a rollover has
  closed, naming the replacement parcel to use instead (cheap, and makes the state
  unrepresentable); (b) carry a later adjustment through to the replacement parcel (correct in
  substance, but re-opens the "cost base frozen at operation time" decision the rollover design
  rests on); (c) flag it in the AMIT adjustment cross-check as an unreachable row

## The `amit` listing flag is retroactive and rewrites every earlier year (SCENARIOS F-23)
(SCENARIOS.md section F verification pass, 2026-08-16. `listings.amit` is a plain boolean with no
time dimension, and three readers key off it as though it had always been true: the tax summary
excludes *every* income row of an AMIT listing regardless of year (`src/reports/tax_summary.rs:352`
— `WHERE NOT l.amit`), the AMIT cash cross-check demands an AMMA statement for every year the
listing has cash rows (`src/reports/amit_cash_cross_check.rs:43`), and the `ReturnOfCapital` write
refuses the action outright once the flag is set (`src/entities/corporate_action/db.rs:337`,
E-04's fix). `docs/API.md` states conversion is supported — "a fund that converts to an AMIT
part-way through a holding keeps the payments recorded while it was an ordinary trust … record the
pre-conversion payments before flagging the listing" — which holds for the *cost base* but not for
the income side.)
- [ ] F-23 — reproduced: an ordinary unit trust with an FY2023 distribution (franked 200,
  unfranked 300, franking credits 85). Tax summary FY2023: `dividends_assessable` 500,
  `franking_credits` 85. `PUT /listings/1` with `"amit": true` → `204`, and the tax summary is now
  **empty** — the whole pre-conversion year of assessable income vanished from the return, with no
  refusal, warning or health row. Only the AMIT cash cross-check notices, and it says the wrong
  thing: "FY2023 has cash rows with no covering AMMA statement", for a year in which there was no
  AMMA statement to have
- [ ] The E-04 refusal is also broader than the documented advice: after the flip, *editing* an
  existing pre-conversion `ReturnOfCapital` (correcting the amount on a payment recorded years
  earlier) is refused `422` too, not only creating one. The stored reduction keeps applying, so
  the cost base is right until someone needs to correct it
- [ ] **Model question for Evan.** (a) Date the status — an `amit_from` date on the listing (or a
  small `listing_amit_periods` table), with every reader comparing the record's year against it;
  (b) drive the AMIT/non-AMIT decision off *which years have an AMMA statement* rather than off
  the flag, so the flag stays a UI hint; (c) declare mid-history conversion out of scope, document
  that a converted fund is entered as two listings, and have the write refuse the flag flip while
  the listing has income rows or a return of capital in an earlier year. Whichever is chosen, the
  income-side silence is the part that must not survive
