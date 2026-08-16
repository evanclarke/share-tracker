# TODO

Items are only marked done when a passing test exists for them.

Completed and decided (out-of-scope / not-reproducible) sections are archived in the topical `DONE/*.md` files, indexed by [DONE.md](DONE.md), to keep this list focused on active work. When a section here is fully done, move it into the matching `DONE/*.md` file rather than leaving it — see CLAUDE.md.

The sections below come from the **2026-07-13 improvement review** (a whole-project pass for
operational and test-strategy gaps, as distinct from the 2026-07-12 programming/domain review whose
findings are all closed in DONE.md), except where a section's heading names another source
(e.g. REQUIREMENTS, SCENARIOS). Each section records one finding; sections land in DONE.md as they
are fixed or decided.

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
