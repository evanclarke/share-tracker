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

The pass raised three findings. Two were closed 2026-08-19 (see [`DONE/reviews.md`](DONE/reviews.md))
— a carried-forward capital loss invisible in a year with no CGT activity of its own, and both
pre-sale tools modelling a disposal dated before the parcels existed — and the one below remains
open. The next section after it is closed is **P. Tax summary, annual tax report, exports**.

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
