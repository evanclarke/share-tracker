# TODO

Items are only marked done when a passing test exists for them.

Completed and decided (out-of-scope / not-reproducible) sections are archived in the topical `DONE/*.md` files, indexed by [DONE.md](DONE.md), to keep this list focused on active work. When a section here is fully done, move it into the matching `DONE/*.md` file rather than leaving it — see CLAUDE.md.

Sections here come from the **2026-07-13 improvement review** (a whole-project pass for
operational and test-strategy gaps, as distinct from the 2026-07-12 programming/domain review whose
findings are all closed in DONE.md), except where a section's heading names another source
(e.g. REQUIREMENTS, SCENARIOS). Each section records one finding; sections land in DONE.md as they
are fixed or decided.

**SCENARIOS.md sections A–N are driven and every finding they raised is closed** in the `DONE/*.md`
archive — section N. Holding accounts and transfers was driven 2026-08-19, raised five findings, and
all five were closed the same day (see [`DONE/reviews.md`](DONE/reviews.md)).

**Section O. Net capital gain, losses, and carry-forward was driven 2026-08-19.** All 17 scenarios
were constructed through the HTTP API against throwaway databases and probed. **Eleven came back
correct outright**, and the arithmetic was right in the other six too — each of those surfaced one
of the three findings below, all of them about what a surface *says* or what an input is allowed to
be, none about a figure the netting walk computes. The correct ones include the scenarios that
matter most: O-01/O-13 (losses applied to
non-discountable gains first and *before* the discount — the ATO-optimal order that turns a naive
$7,500 net capital gain into $5,000), O-02/O-03/O-04 (current-year plus brought-forward losses, the
excess carried forward, a `cgt_settings` opening loss consumed across a three-year chain), O-05 (an
AMMA-only year, its discount gains grossed up ×2 before the loss and halved after), O-06 (a G1
excess as a non-discountable gain beside a discountable parcel gain and a loss), O-07 (an E10 excess,
discountable on the holding period as at the statement's year end — probed either side of the
12-month line, and again with the parcel later sold so the G1/disposal pair is counted once each),
O-08 (a demerger contributing nothing in its year, the deferred gain surfacing with the carried
acquisition date), O-09/O-10 (collectables and personal-use assets, honestly documented as out of
scope with the damage named), O-11 (a back-dated Sell restating a prior year and rechaining every
later carry-forward), O-12 (an exactly-nil year still reported with its workings), O-14/O-15 (the
what-if's prior-year loss consumption differing by strategy, and the recorded sale reproducing it
exactly both ways), O-16 (the over-request boundary, exact to 0.0000001 of a unit) and O-17 (the CSV
export matching the JSON field for field, with its ATO label row).

The pass raised three findings. The first — a carried-forward capital loss invisible in a year with
no CGT activity of its own — was closed 2026-08-19 (see [`DONE/reviews.md`](DONE/reviews.md)); the
two below remain open. The next section after these are closed is
**P. Tax summary, annual tax report, exports**.

## The pre-sale what-if and the parcel optimiser model a disposal dated before the parcels existed (SCENARIOS O-14, O-15, O-16)
(SCENARIOS.md section O verification pass, 2026-08-19. Both endpoints read their candidates through
`parcel_optimiser::db_candidate_parcels_on`, which calls `open_parcels::db_open_parcels_on(conn)`
with **no as-at date** — the parcels open *today*, whatever `date` / `sale_date` the request names.)
- [x] Reproduced: a parcel acquired **2022-01-01**, and
  `POST /portfolio/net-capital-gain/what-if` for a disposal dated **2021-12-31** allocating it
  explicitly. Accepted `200`, projecting a $10,000 non-discountable gain into FY2021. The identical
  allocation on `PUT /sells/:id` is refused `422` — *"an allocated parcel is dated after the sale
  date"* — so the what-if answers with figures for a sale that can never be recorded
- [x] Boundary probed: a disposal dated **on** the acquisition date is legitimate on both paths (a
  same-day parcel is fine on a real Sell); the day before is the first refused on the Sell path and
  the first wrongly accepted here. Exactly the Sell's rule, one endpoint short
- [x] The discount clock runs backwards with it: every parcel acquired after the disposal date is
  classified `discount_eligible: false`. `POST /portfolio/parcel-optimiser` with `sale_date`
  2021-01-01 returned **all four strategies identical** (`fifo`, `min_gain`, `max_discount`,
  `harvest_losses` — same parcels, same $7,000), because nothing was discountable and the orderings
  collapsed. The screen exists to show the choice, and it silently shows none
- [x] The other half of the same read: a parcel that *was* open at the disposal date but has since
  been sold is **excluded** from the candidates, so a past-dated what-if also under-reports what
  could have been sold. One cause, both directions
- [x] Reachable from the UI: the Pre-Sale What-If's `Sale date` field is `required` with **no
  default** (`config.js`), so nothing steers the user to today, and O-14's own question — would
  selling in June instead of July consume a different year's losses — invites a date in the past
- [x] A *future*-dated request is unaffected: every currently-open parcel is a legitimate candidate
- [ ] **Decided 2026-08-19 (Evan): option (a)** — read the candidates as at the request's date.
- [ ] A model decision, four options:
  - **(a)** ← **chosen.** Read the candidates **as at the request's date** — `db_open_parcels_on` already takes an
    as-of date everywhere else (`docs/API.md`'s [As-at date](docs/API.md#as-at-date) section), so
    both endpoints would model the holding as it actually stood. The fullest answer, and the one
    that makes a past-dated what-if mean "what if I had sold then"; note it then offers parcels that
    have since been sold, which for a *contemplated* sale is advice the user cannot act on
  - **(b)** Keep "open today" and add the Sell path's own refusal: reject an allocation naming a
    parcel dated after the disposal date (`422`, the Sell's wording), and drop such parcels from a
    strategy's candidate list. Fails safe, no semantic change, no new as-at reads
  - **(c)** Refuse a past-dated request outright — these are *pre-sale* tools; `422` naming the date
  - **(d)** Documentation only: state that both endpoints value a contemplated sale against today's
    holding and that a past `date` is only meaningful for the tax-year and discount-clock arithmetic
- [ ] Tests: the reproduction above (what-if and optimiser), the same-day boundary staying accepted,
  and — per the option — the refusal or the as-at candidate set
- [ ] Docs sync: `docs/API.md`'s [Parcel-selection optimiser](docs/API.md#parcel-selection-optimiser)
  and [Pre-sale what-if](docs/API.md#pre-sale-what-if) sections, and the 422 catalogue row if (b)/(c)

## The what-if's over-request refusal does not name the account it was scoped to (SCENARIOS O-16)
(SCENARIOS.md section O verification pass, 2026-08-19. `what_if_handler`'s strategy branch formats
*"only {open} unit(s) of {listing} are open"* without the `holding_account_id` the request scoped it
to — while its own explicit-allocations branch, and the optimiser, both name the account.)
- [x] Reproduced: 2,000 units of TSTG open in the default account and 5,000 in a second account.
  `POST /portfolio/net-capital-gain/what-if` for 3,000 units with `"holding_account_id": 1` answers
  `422 only 2000 unit(s) of TSTG are open` — a statement that is simply false of the 7,000 units
  held. `POST /portfolio/parcel-optimiser` with the same body answers
  `only 2000 unit(s) of TSTG are open in account 'Default'`, which is right
- [x] The same handler's allocations branch already gets it right: it appends
  `" in {account}"` via `reports::account_label` when the request named one
- [x] Nothing computes wrongly — this is the refusal's wording only, and it is the message the web
  UI shows in its toast
- [ ] Fix: reuse the allocations branch's `account_label` suffix in the strategy branch, so one
  endpoint gives one answer. No decision needed
- [ ] Tests: the account-scoped over-request naming the account, and the unscoped one not
- [ ] Docs sync: none expected (the 422 catalogue already covers "more units than the listing's open
  quantity"); confirm the wording quoted in `docs/API.md` still matches
