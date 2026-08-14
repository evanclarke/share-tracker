# TODO

Items are only marked done when a passing test exists for them.

Completed and decided (out-of-scope / not-reproducible) sections are archived in the topical `DONE/*.md` files, indexed by [DONE.md](DONE.md), to keep this list focused on active work. When a section here is fully done, move it into the matching `DONE/*.md` file rather than leaving it — see CLAUDE.md.

The sections below come from the **2026-07-13 improvement review** (a whole-project pass for
operational and test-strategy gaps, as distinct from the 2026-07-12 programming/domain review whose
findings are all closed in DONE.md), except where a section's heading names another source
(e.g. REQUIREMENTS). Each section records one finding; sections land in DONE.md as they are fixed
or decided.

## A Buy's date and holding account escape the Sell-allocation invariants (SCENARIOS A-09, A-13)
(SCENARIOS.md section A verification pass, 2026-08-14. `PUT /trades/:id` re-checks the dependants
of the two fields it already guards — `listing_id` (`UpsertError::ListingChangeReferenced`) and a
`quantity` shrink (`QuantityBelowAllocated` / `QuantityBelowAmitAdjustment`) — but not `date` or
`holding_account_id`, which are equally load-bearing for a Sell's allocations. Both edits are
accepted `204` and leave a state `PUT /sells/:id` itself refuses, so the invariant holds only on the
Sell side of the pair. This is the failure mode CLAUDE.md's data-integrity rule names: a write path
that can reintroduce a state another path forbids.)
- [ ] A-09 — moving a Buy's `date` after a Sell that allocates from it is accepted. Reproduced:
  Buy 2022-08-01 ×100, Sell 2023-03-01 ×100 allocating all 100, then `PUT /trades/1` with
  `date: 2023-05-01` → `204`. The open-parcels and realised-gains reports then show the sale
  costed against a parcel acquired two months *after* it, `discount_eligible: false` (the discount
  clock runs backwards); the annual tax report prints the same impossible acquisition/sale pair and
  still reports `completeness.complete: true`; no cross-check flags it. Re-`PUT`ting the identical
  Sell body is refused `422 "an allocated parcel is dated after the sale date"`
  (`sell::SellError::PurchaseAfterSale`, `src/entities/sell.rs:623`) — so the guard exists, just not
  on this side. Re-check it in `trade::db_upsert` when `date` moves later, against
  `parcel_allocations JOIN trades s ON s.id = sale_trade_id` (min sale date)
- [ ] A-13 — moving a Buy's `holding_account_id` out of the account a Sell allocating from it sits
  in is accepted. Reproduced: Buy ×100 in account 1, Sell ×60 in account 1, then `PUT /trades/1`
  with `holding_account_id: 2` → `204`; the parcel reports as held in account 2 while the realised
  gain stays costed against it in account 1. Re-`PUT`ting the identical Sell body is refused
  `422 "an allocated parcel is held in a different holding account from the Sell"`. Same fix shape:
  re-check in `trade::db_upsert` when the account changes and any allocation references the parcel
- [ ] Tests: `entities::trade::tests` — a date move past an allocating Sell and an account move away
  from one are each refused `422` naming the rule, and the pre-edit state is unchanged (nothing
  persisted); the same edits stay allowed while no allocation references the parcel
- [ ] Docs sync: `docs/API.md` Trades section + the Response codes 422 list (the two new refusals,
  alongside the existing listing-change and quantity-shrink wording)

## Deleting a split/bonus/return-of-capital silently restates reported gains (SCENARIOS A-06, A-20)
(SCENARIOS.md section A verification pass, 2026-08-14. `RightsIssue`, `BuyBack`, `ScripForScrip`,
`Demerger`, and `WorthlessShares` are all frozen while the trades they produced exist. The three
action types that re-base parcels instead of creating trades — `ShareSplit`, `BonusIssue`,
`ReturnOfCapital` — carry no delete guard at all, even though every open-parcel quantity, cost base,
and realised gain is computed from them at read time.)
- [ ] A-20 — deleting a `ShareSplit` after trades were entered on the post-split basis can leave
  allocations the write path rejects. Reproduced: Buy 2023-01-10 ×100, 2-for-1 split 2023-03-01,
  Sell 2023-06-01 ×200 (post-split, the whole holding) → realised **gain $200**. `DELETE
  /corporate_actions/1` → `204`, and the same Sell now reports a **loss of $800** against a parcel
  it over-consumes: re-`PUT`ting the identical Sell body is refused `422 "the allocations exceed a
  purchase parcel's available quantity"`. A `BonusIssue` delete has the same shape
- [ ] A-06 — the same delete leaves a generated AMIT adjustment covering more units than the parcel
  has (quantity materialised in as-acquired units at generation time). The AMIT adjustment
  cross-check does flag the resulting statement, so this half fails visibly rather than silently
- [ ] A-21 — deleting a `ReturnOfCapital` cannot break a quantity invariant (it changes no unit
  count), but it does silently drop an already-reported CGT event G1 gain from a prior year's net
  capital gain (reproduced: G1 gain $50.00 in FY2023 → the year disappears from the report entirely)
- [ ] Decide the guard: refuse the delete while any trade of the listing is dated on/after the
  action's `date` (the rule the `demerge`/`exchange`/`recognise` operations already apply at the
  other end), or require an explicit override. Whichever way, a delete must not be able to leave
  allocations exceeding a parcel — the invariant `PUT /sells/:id` enforces
- [ ] Tests: `entities::corporate_action::tests` — deleting each of the three re-basing types is
  refused while dependent trades exist, and still allowed when none do
- [ ] Docs sync: `docs/API.md` Corporate actions (the new refusal, per type) + Response codes 422

## A DELETE blocked by an inbound foreign key says the row does not exist (SCENARIOS A-18, A-23, A-38, A-41)
(SCENARIOS.md section A verification pass, 2026-08-14. `ApiError`'s `From<sqlx::Error>` maps
`ErrorKind::ForeignKeyViolation` to `"the request refers to a record that does not exist"`
(`src/infra/http.rs:295`) — correct for an *outgoing* FK (a write naming an unknown listing or
currency), but the same SQLite error kind covers the *incoming* case, where a DELETE is blocked
because something still references the row. `delete_handler`'s own doc comment (`src/infra/http.rs:266`)
records that this is the path such deletes take. For a delete the message states the opposite of
the truth: the row exists, and what is missing is the name of whatever depends on it. It also
breaks the error-bodies contract in `docs/API.md` ("saying *why* it failed — the failed invariant").)
- [ ] Reproduced on every entity whose delete has no hand-written guard: `DELETE
  /amma_statements/:id` with generated AMIT adjustments (A-18/A-19 — and the statement is
  undeletable until they are removed one by one, which the message never says), `DELETE
  /listings/:id` with stored closing prices (A-23), `DELETE /exchanges/:mic` referenced by a listing
  or its own holidays (A-41), and `DELETE /corporate_actions/:id` frozen by its trade group (A-38 —
  `docs/API.md` promises a `422` here, and the status is right, only the reason is wrong)
- [ ] Fix shape: keep the outgoing wording for writes, and give deletes a message that names the
  dependant — either by parsing the constraint's table out of the SQLite detail (it names the child
  table) or by adding hand-written guards like `trade`'s and `holding_account`'s. Entities with an
  explicit guard already answer well ("this account still has trades, income, AMMA statements …")
- [ ] A-23 follow-on to document either way: a listing that has ever had a **manual** closing price
  entered can never be deleted — the manual price is `status: ok`, so `DELETE /closing_prices/…`
  refuses it (the documented one-way rule), and the listing's FK refuses while it stands. That
  dead-end is a consequence of two documented rules but is not itself stated anywhere
- [ ] Tests: `infra::http::tests` (or per-entity) — a delete blocked by a dependant answers `422`
  naming the dependant, and a write naming an unknown row keeps the existing wording
- [ ] Docs sync: `docs/API.md` Response codes `422` row + the AMMA statements and Listings sections
  (what blocks a delete and how to clear it)

## Deleting a DRP enrolment period strands its trailing residual (SCENARIOS A-43)
(SCENARIOS.md section A verification pass, 2026-08-14. Closing a period by setting
`unenrolment_date` settles the trailing residual — the leftover the period's last reinvestment
carried forward moves to `residual_paid_out` on that DRP trade, in the same transaction, because the
registry refunds it at termination (`db_unenrolment_pays_out_trailing_carried_residual` pins this).
`DELETE /drp_enrolments/:id` ends the period just as finally and does none of it.)
- [ ] Reproduced: enrol open-ended, reinvest $100 at $10.50 → DRP trade with
  `residual_carried_forward: 5.5`, `residual_paid_out: 0`. Unenrolling → `carried 0 / paid_out 5.5`.
  Deleting the period instead → `carried 5.5 / paid_out 0` — cash recorded as carrying forward into
  a period that no longer exists, and nothing can pick it up (a later reinvestment is refused
  outright, `"account 'Default' is not enrolled …"`)
- [ ] Decide: settle the trailing residual on delete the same way unenrolment does, or refuse the
  delete while the period covers a reinvestment (pointing at unenrolment instead). The second is
  probably right — deleting a period that already produced DRP trades erases the record of why they
  exist, and the reinvestment cannot be re-created afterwards
- [ ] Tests: `entities::drp_enrolment::tests`, mirroring
  `db_unenrolment_pays_out_trailing_carried_residual` for the delete path
- [ ] Docs sync: `docs/API.md` DRP enrolments (what deleting a period does to a trailing residual)

## A closed financial year can be restated with nothing marking it (SCENARIOS A-15, A-21, A-25, A-35)
(SCENARIOS.md section A verification pass, 2026-08-14. Every tax report is computed live from the
current facts, so editing a prior year's inputs silently changes figures that may already have been
lodged. Report snapshots do not cover this — they snapshot the three price-dependent reports only,
never the tax summary, net capital gain, or annual tax report. `row_history` records the change, so
the restatement is *auditable* after the fact, but nothing *surfaces* it.)
- [ ] Reproduced four ways, all `204`/`200` with no flag: changing a lodged year's Buy price
  (FY2023 net capital gain $500 → $1,100, A-15); deleting a `ReturnOfCapital` after its G1 gain was
  reported (A-21); deleting the `cgt_settings` opening carried-forward loss after later years
  consumed it (FY2024 net gain $500 → $1,000, A-25); deleting the only disposal of a loss year that
  a later year's carry-forward drew on (FY2024 net gain $750 → $1,500, A-35). The annual tax report
  keeps reporting `completeness.complete: true` throughout
- [ ] Decide the scope: this may be honest "not modelled" — there is no lodged/closed-year concept
  in the data model, and adding one is a real feature (a lodgement marker per FY, plus a
  "changed since lodgement" flag driven off `row_history` timestamps). If it stays unmodelled it
  needs a **Known limitations** entry saying so plainly, since a user reasonably assumes a prior
  year's numbers are settled. Either way this is a documentation-or-feature decision, not a bug
- [ ] Related, low severity (A-40): `DELETE /exchange_holidays/:mic/:date` has no guard and no flag,
  and a trade re-saved afterwards without an explicit `settlement_date` silently recomputes against
  the changed calendar (reproduced: an ASX trade settling 2024-04-02 recomputed to 2024-03-29 — Good
  Friday itself — once that holiday was deleted). Stored `settlement_date` values are untouched, and
  no CGT figure reads the column (only the settlement-coverage report and the annual tax report's
  display do), so the exposure is a record field, not a tax figure. Worth one line wherever the
  restatement decision above lands
