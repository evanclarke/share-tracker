# TODO

Items are only marked done when a passing test exists for them.

Completed and decided (out-of-scope / not-reproducible) sections are archived in the topical `DONE/*.md` files, indexed by [DONE.md](DONE.md), to keep this list focused on active work. When a section here is fully done, move it into the matching `DONE/*.md` file rather than leaving it — see CLAUDE.md.

Sections here come from the **2026-07-13 improvement review** (a whole-project pass for
operational and test-strategy gaps, as distinct from the 2026-07-12 programming/domain review whose
findings are all closed in DONE.md), except where a section's heading names another source
(e.g. REQUIREMENTS, SCENARIOS). Each section records one finding; sections land in DONE.md as they
are fixed or decided.

**SCENARIOS.md sections A–H are driven and every finding they raised is closed** in the `DONE/*.md`
archive. **Section I. DRP** was driven 2026-08-17; the six findings below are its open work. When
they are closed, the next work comes from driving **SCENARIOS.md section J. Employee share schemes**
the same way — walk its scenarios against the running system, and record each gap here as its own
`## ` section.

## A reinvestment paid after its period's unenrolment escapes that period (SCENARIOS I-01, I-02, I-04)
(SCENARIOS.md section I verification pass, 2026-08-17. Eligibility is decided on the **ex date** —
registry practice, and right: participation is fixed at the record date. But every *other* question
about which period a reinvestment belongs to is decided on the **trade date**, which is the payment
date. Those two dates straddle a period boundary whenever a plan is ended between a distribution
going ex and its payment — the ordinary way a DRP is stopped — and then the three trade-date reads
disagree with the ex-date one: the trailing-residual settlement in `drp_enrolment::db_upsert`, the
`residual_brought_forward` chain lookup in `drp_reinvestment::db_reinvest`, and the
`CoversReinvestment` delete guard in `drp_enrolment::db_delete`.)
- [ ] I-01 — reproduced: period `[2020-01-01, 2024-07-01)` CarryForward; a $100 distribution ex
  2024-06-20, paid 2024-07-15, reinvested at $7 → `201`, trade dated **2024-07-15**, 14 units,
  `residual_carried_forward: 2`. That $2 is **stranded**: the period's settlement walk
  (`date >= enrolment_date AND date < unenrolment_date`) never sees the trade, so it is neither
  paid out nor available to any later reinvestment — re-saving the closed period does not reach it
  either. The registry refunds that leftover at termination; the record says it is still carried
- [ ] I-01 — same fixture, re-enrolling on the same day (`[2024-07-01, …)` PayOut): the trade dated
  2024-07-15 now falls inside the **new** period, and the next reinvestment (Sep 2024) brings its
  $2 forward — the carry crosses a period boundary the module doc guarantees it never crosses, and
  the *new* period's residual handling settles money the *old* period's plan left over
- [ ] I-02 — the A-43 guard is defeated by the same mismatch: `DELETE /drp_enrolments/1` on the
  first fixture answers **`204`**, deleting a period that demonstrably produced a reinvestment.
  `db_delete`'s `EXISTS(… trades … date >= ? AND date < ?)` asks the trade-date question, so the
  refusal that exists precisely to keep "the record of why that trade exists" (DONE/reviews.md,
  A-43) never fires for a distribution paid after the unenrolment
- [x] **Decided 2026-08-17 (Evan): (a) match by the distribution's entitlement date.** The three
  reads all want *the period that authorised this reinvestment*, which is knowable exactly:
  `income.reinvestment_trade_id` links the trade back to its distribution, whose `ex_or_pay_date`
  is the date eligibility was decided on. Join `trades → income` in all three places, so period
  membership is the same question everywhere and the trade date stops deciding anything. (Rejected:
  refusing a trade date outside the period — it refuses a genuine registry pattern and mis-dates
  the parcel if the user complies; a `drp_enrolment_id` provenance column — a fourth thing to keep
  in step.)
- [ ] Tests: the ex-in/paid-after fixture settles its residual at unenrolment; the same fixture with
  an immediate re-enrolment does **not** carry into the new period; `DELETE` of the period is
  refused `422` pointing at unenrolment
- [ ] Docs sync: `docs/API.md` DRP enrolments (which period a reinvestment belongs to, and that it
  is not the trade date), plus the module docs in `entities::drp_enrolment`/`drp_reinvestment` that
  currently state the trade-date rule

## Re-opening or extending an unenrolment does not restore the residual it paid out (SCENARIOS I-01, I-03)
(SCENARIOS.md section I verification pass, 2026-08-17. Closing a period moves the trailing
`residual_carried_forward` to `residual_paid_out` — correct, the registry refunds it. The write is
one-way: nothing restores it if the closure is undone or moved, and `db_upsert`'s own comment calls
the settlement "idempotent — once moved, carried is zero", which is exactly why the reverse edit
cannot recover.)
- [ ] I-03 — reproduced: open period, $100 reinvested at $7 → 14 units, `carried 2`. Unenrol
  (`carried 0 / paid_out 2` — correct). Then correct the mistake by clearing the unenrolment date:
  the trade still reads `carried 0 / paid_out 2`, and the next reinvestment in the re-opened period
  brings forward **0**, buying 14 units off $100 instead of 14 off $102 (`carried 2` instead of
  `carried 4`). The chain has silently lost $2 — and with a smaller price step it loses a *unit*
- [ ] I-01 — the realistic version is a mistyped end date, not a change of mind: closing at
  `2021-01-01` and correcting to `2025-01-01` settles the residual under the first window and never
  un-settles it, leaving a mid-chain trade carrying `paid_out` and every later reinvestment in the
  period funded short
- [x] **Decided 2026-08-17 (Evan): (a) make the settlement a function of the period, not an
  event.** On every upsert, recompute both residual columns for the period's trades from the period
  as it now stands — the trailing trade settles iff the period is closed, every other trade carries
  — which makes the edit reversible by construction. (Rejected: restoring on re-open only, which
  leaves the mistyped-then-extended case wrong; documenting it as one-way.)
- [ ] Tests: unenrol → re-open restores `carried` and the next reinvestment brings it forward;
  unenrol → extend moves the settlement to the new trailing trade and leaves no `paid_out` behind;
  the existing `db_unenrolment_pays_out_trailing_carried_residual` still holds
- [ ] Docs sync: `docs/API.md` DRP enrolments (what editing an unenrolment date does to a residual)

## A whole-number stated allotment can swallow a share's worth of cash (SCENARIOS I-06)
(SCENARIOS.md section I verification pass, 2026-08-17. The optional `units` path exists for broker
plans that allot **fractional** shares: the statement's figure is authoritative, cross-checked
against the available cash to within `1 unit-step at the stated precision × price`, and the residual
columns record zero because a fractional allotment leaves no cash behind. The tolerance scales with
the units' own scale, so at scale 0 it is a *whole unit's* worth of cash — and the discarded
difference is real money, not statement rounding.)
- [x] I-06 — reproduced: $100 available, price $7, `units: "14"` → **`201`**, quantity 14,
  `residual_brought_forward/carried_forward/paid_out` all `0`. The $2 that bought no whole unit is
  neither carried nor paid out; the next reinvestment brings forward nothing. At `units: "14.286"`
  (3 dp, the fractional case the path is for) the tolerance is $0.007 and the same $100 is fully
  spent — the behaviour is right there. A full step off (`14.290`) is correctly refused `422`
  carrying both figures
- [x] The entry path makes this reachable: the reinvest form's units field is offered on every
  distribution, and an ASX registry statement *does* state whole units allotted — keying them in
  is the natural thing to do, and it silently costs the parcel $2 less than the cash applied while
  losing the carry
- [x] **Decided 2026-08-17 (Evan): (a) treat the difference as a residual.** Compute
  `available − units × price` as the leftover and apply the period's residual handling to it,
  cent-rounded so a fractional plan's sub-cent statement rounding still records zero; the tolerance
  check stays as the sanity bound. Records what actually happened rather than discarding it.
  (Rejected: refusing whole-number `units`; a fixed cent tolerance, which would reject the
  fractional case the field exists for.)
- [x] Tests: whole units with a genuine leftover carry it (or are refused, per the decision) and
  the next reinvestment brings it forward; the fractional cases
  (`explicit_units_take_the_statements_fractional_allotment`,
  `explicit_units_tolerate_sub_step_statement_rounding`, `morgan_stanley_ice_fractional_statements_reproduce`)
  are unchanged
- [x] Docs sync: `docs/API.md` reinvest `units` semantics + the Response-codes `422` catalogue if a
  refusal is added; the units hint in `config.js`


**Resolution (2026-08-17): the leftover is the period's residual on both paths, and which kind of
difference it is follows from how the units were stated.**

The stated-units branch of `db_reinvest` no longer returns `(units, ZERO, ZERO)`: it computes
`available − units × price` like the whole-share branch, and the two share one `match handling`
that carries or pays it out. Cent-rounding the difference (the first attempt) turned out to be the
wrong discriminator — the real Morgan Stanley statements in
`morgan_stanley_ice_fractional_statements_reproduce` miss the cash by up to **5 cents**, because
0.500 units printed to 3 dp is a *rounded* allotment whose true fraction already spent that cash;
carrying it would double-count it. The discriminator is the units' own **scale**: a whole number is
an exact count (the plan bought whole units and left the rest over — cash), a figure stated to
decimals is a rounded one (the plan applied everything — printing, not money, so zero as before).
The one-unit-step tolerance is unchanged and is what bounds the whole-unit leftover below one
unit's price.

Not fixed, and deliberately: the *overspend* direction is still bounded only by that tolerance, so
stated units costing up to a unit's price **more** than the available cash are accepted with no
residual (15 units at $7 against $100). Tightening it needs a bound that does not reject a genuine
fractional statement — a separate question, noted here rather than guessed at.

Tests: `stated_whole_units_carry_the_cash_they_left_over` (14 units at $7 against $100 carries $2,
and the next reinvestment brings it forward), `stated_whole_units_pay_out_the_leftover_where_the_period_says_so`,
and the three fractional tests unchanged. `docs/API.md`'s "Stated allotments (`units`)" paragraph
and the reinvest form's units hint now state both halves.
## A reinvested distribution can be edited afterwards with nothing re-checked (SCENARIOS I-01, I-04, I-07)
(SCENARIOS.md section I verification pass, 2026-08-17. `income::db_upsert` deliberately never writes
`reinvestment_trade_id` — a client can't forge or drop the link — but it also never *looks* at it,
so every field the reinvest operation validated against can be changed underneath the DRP trade. This
is A-09/A-13's failure mode on the income side: a write path that reintroduces a state the operation
itself refuses.)
- [ ] I-07 — reproduced, all four accepted `204` with the DRP trade untouched:
  **listing** moved to another listing (the link now crosses listings — the trade is a parcel of the
  old one, and `POST …/reinvest` would have refused the new one for want of an enrolment);
  **holding account** moved to an account with no enrolment (the trade stays in the old account's
  chain, and enrolment is per (listing, account));
  **ex date** moved outside every enrolment period (the reinvestment now rests on an enrolment that
  does not cover it — the very check that gated its creation);
  **cash amounts** changed from $100 to $200 (the trade still says 14 units and `carried 2`, figures
  computed from a distribution that no longer exists)
- [ ] I-01/I-04 — the cash edit is the one that reaches a report: the parcel's cost base stays at
  the old cash while the assessable dividend becomes the new figure, so the ATO identity the whole
  operation rests on — "the acquisition cost is the amount of the dividends used to acquire them"
  (`docs/ato/cgt-dividend-reinvestment-plans.md`) — quietly stops holding, with no cross-check
  flagging it
- [ ] Fix shape is A-09's, verbatim: re-check in `income::db_upsert`, in its own transaction, when
  the stored row has a `reinvestment_trade_id` — refuse a change to `listing_id`,
  `holding_account_id`, the entitlement inputs (`ex_date`/`entitlement_date`/`trust_income`) or any
  cash component, naming the field and pointing at `DELETE /income/:id/reinvest` (the operation's
  own undo, which already exists and is the documented way to redo a reinvestment). Non-load-bearing
  fields (`amount_per_security`, memo columns) stay editable
- [ ] Tests: each of the four edits is refused `422` naming the rule with nothing persisted; the
  same edits stay allowed on a distribution with no reinvestment; undo → edit → re-reinvest works
- [ ] Docs sync: `docs/API.md` Income (what is frozen while a distribution is reinvested) + the
  Response-codes `422` catalogue

## A distribution in a currency other than its listing's is reinvested without conversion (SCENARIOS I-06, I-08)
(SCENARIOS.md section I verification pass, 2026-08-17. `db_reinvest` takes the cash from the income
row — in the income row's currency — and divides it by a price it stamps with the **listing's**
currency. Nothing checks the two agree. CLAUDE.md's rule is explicit: "Convert every non-AUD amount
to AUD using the record's `fx_rate` before aggregating or comparing — never mix currencies in one
calculation".)
- [x] I-08 — reproduced: AUD listing, income row `currency: "USD"` with `foreign_source_income: 100`
  → reinvest at `7` answers `201` with quantity **14** and `residual_carried_forward: 2` on an
  **AUD** trade. US$100 was divided by A$7; the parcel is costed A$98 for cash that was US$100
- [x] The mismatch is reachable because an income row's currency is free-form (the currencies FK
  aside) and is not tied to its listing's. Whether *that* should be constrained in general is a
  wider question than this section — but the reinvest operation is a single calculation over the
  two, and is where the mixing actually happens
- [x] **Decided 2026-08-17 (Evan): (a) refuse the reinvestment** when the distribution's currency
  differs from the listing's, naming both. Fails safe, one check, no FX policy invented: a registry
  paying a foreign-currency distribution into a plan converts it itself, and the converted figure is
  what the statement shows, so the user has it. (Rejected: converting at the ATO rate, which invents
  an FX policy the statement already settled; constraining `income.currency` to its listing's
  everywhere — the widest fix, noted as a question for a later pass rather than this section.)
- [x] Tests: a distribution whose currency differs from its listing's is refused `422` naming both
  currencies with nothing persisted; the matching-currency USD case
  (`morgan_stanley_ice_fractional_statements_reproduce`) is unchanged
- [x] Docs sync: `docs/API.md` reinvest + the Response-codes `422` catalogue


**Resolution (2026-08-17): refused, naming both currencies.**

`ReinvestError::CurrencyMismatch { distribution, listing }` is raised in `db_reinvest` beside the
listing-currency read it already does, before any arithmetic; the 422 body names both currencies
and says where to correct the entry (a registry reinvesting a foreign-currency payment converts it
and prints the converted figure). The check also caught the module's own fixtures: three tests set a
USD listing and left the distribution at the builder's default AUD, so `insert_distribution_dated`
now stamps the listing's currency and the ICE statements' rows are USD, as the statements are.

Tests: `a_distribution_in_another_currency_than_its_listing_is_refused` — the error variant, the 422
naming both currencies, and nothing persisted (no trade, no link).
## The partial-participation limitation names no workaround (SCENARIOS I-09)
(SCENARIOS.md section I verification pass, 2026-08-17. The Known limitation is honest — "enrolment
is all-or-nothing per (listing, holding account): a registry plan that reinvests only a portion of a
holding's units is not modelled" — and the system fails safe: stating the partial units is refused
`422` with both figures. What it doesn't say is what to do instead, which the scenario asks to
verify.)
- [ ] I-09 — reproduced: a $100 distribution half reinvested, entered as `units: "7"` at $7 → `422`
  "the stated units at the reinvestment price spend 49, but the reinvestable cash … is 100". Good
  refusal, no guidance
- [ ] I-09 — the workaround does produce a defensible cost base, verified end to end: split the
  distribution into two income rows — the reinvested $50 and the cash $50 — and reinvest the first.
  The parcel costs $49 for 7 units with $1 carried (the dividends actually applied, per
  `docs/ato/cgt-dividend-reinvestment-plans.md`), and the tax summary still declares the full $100
  as assessable dividend income. The per-share cross-check (`amount_per_security`/`securities_held`)
  has to be left off the split rows, since neither half reconciles against the whole holding
- [ ] Caveat worth stating with it: an exactly half-and-half split trips the `duplicate_income`
  health warning (same listing, account, `date_paid` and *identical* amounts — G-24's key), so the
  banner reports a duplicate that is deliberate. Uneven splits don't
- [ ] Fix: documentation only — extend the Known-limitations entry with the two-row workaround, the
  per-share-cross-check caveat and the duplicate-income note, and pin it in `doc_checks.rs` the way
  the other doc-only requirements are
