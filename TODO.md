# TODO

Items are only marked done when a passing test exists for them.

Completed and decided (out-of-scope / not-reproducible) sections are archived in the topical `DONE/*.md` files, indexed by [DONE.md](DONE.md), to keep this list focused on active work. When a section here is fully done, move it into the matching `DONE/*.md` file rather than leaving it — see CLAUDE.md.

The sections below come from the **2026-07-13 improvement review** (a whole-project pass for
operational and test-strategy gaps, as distinct from the 2026-07-12 programming/domain review whose
findings are all closed in DONE.md), except where a section's heading names another source
(e.g. REQUIREMENTS, SCENARIOS). Each section records one finding; sections land in DONE.md as they
are fixed or decided.

## A parcel reduced by both an AMIT adjustment and a return of capital loses the excess over its cost base (SCENARIOS B-07, B-08)
(SCENARIOS.md section B verification pass, 2026-08-15. `reports::net_capital_gain`'s `e10_gains`
and `g1_gains` each walk their own reduction chain from the parcel's **full** initial cost base,
blind to the other — `g1_gains`' doc comment states the assumption outright: "Independent of the
AMIT E10 walk above: E10 applies to trust units, G1 to company shares, so the two reduction chains
never share a parcel in practice." Nothing enforces it: a `ReturnOfCapital` on a listing whose
`amit` flag is set is accepted `204`.)
- [ ] Reproduced: AMIT listing, Buy 100 @ $10 (cost base $1,000); `ReturnOfCapital` $6/unit paid
  2024-09-01; AMMA FY2025 `cost_base_adjustment: "6.00"`. `/portfolio/open-parcels` is right —
  both $600 reductions reported, `remaining_cost_base` floored to 0 — but
  `/portfolio/net-capital-gain` shows FY2025 `cgt_event_e10_gain: 0`, `cgt_event_g1_gain: 0`,
  `net_capital_gain: 0`. The **$200 excess is never reported**
- [ ] It is lost, not deferred: selling the parcel for $15/unit in FY2026 books a $1,500 gain
  against the nil cost base, so the year's grossed figure is $1,500 where the correct total across
  the two years is $1,700
- [ ] The error is always an understatement, never the reverse: each walk reports
  `its own reductions − cost base` where the truth is `both reductions − cost base`, so the
  reported total can only be short (by the cost base, once, whenever both walks fire; by the whole
  excess when neither individually exceeds)
- [ ] Order-independent, at least: entering the action before or after the AMMA statement gives
  identical figures both ways (checked)
- [ ] Decide the fix, and its scope: walk one combined reduction chain per parcel in date order
  (the AMMA statement's `tax_year_end_date` against the payment's `date`), attributing each excess
  to the event that caused it — versus refusing the combination at write time (a `ReturnOfCapital`
  on an `amit` listing, which SCENARIOS E-04 asks about independently: an AMIT's cost-base movement
  is the AMMA `cost_base_adjustment`, and `PUT /income` already refuses a `tax_deferred_amount` on
  an AMIT row for exactly that reason). The refusal is much the smaller change and closes the case
  that arises in practice; the combined walk is what makes the reports correct for a fund that
  converts from a non-AMIT MIT mid-history (SCENARIOS F-23)
- [ ] Tests: `reports::net_capital_gain` — a parcel carrying both reduction kinds, the excess
  reported once and in the right year; and the write-time refusal if that is the chosen route
- [ ] Docs sync: `docs/API.md` net capital gain (the CGT event E10 and G1 paragraphs each describe
  their own walk in isolation) and, if refused at write time, the Income/Corporate actions sections

## Brokerage in a currency other than the trade's is added to the cost base unconverted (SCENARIOS B-02)
(SCENARIOS.md section B verification pass, 2026-08-15. `domain::cost_base`'s `initial_cost` is
`average_price × quantity + brokerage + gst_on_brokerage`, summed in the trade's currency and
converted to AUD as one figure at the acquisition-month rate. `trades.brokerage_currency` is
FK-validated against `currencies` and then read by exactly one thing — `check_statement_total`,
which *refuses* the statement-total cross-check when it differs from `currency`. No calculation
consults it, and the field carries no informational-only comment, so the model invites an entry it
then mis-costs.)
- [ ] Reproduced: USD listing, RBA USD rate 0.50 for 2024-01, Buy 10 @ USD 100 with
  `brokerage: "30"`, `gst_on_brokerage: "3"`, `brokerage_currency: "AUD"` (an Australian broker's
  AUD fee on a US trade). `/portfolio/open-parcels` reports `original_cost_base` **A$2,066**; the
  correct figure is **A$2,033** (USD 1,000 ÷ 0.50 = A$2,000, plus the A$33 already in AUD). The
  A$33 fee was converted as though it were USD
- [ ] Same on the disposal side: a Sell's proceeds net the brokerage before conversion, so a
  foreign-currency fee on a foreign-currency sale is netted at the wrong scale
- [ ] Not covered by any Known limitation, and no test pins a cost base with a mixed
  brokerage/trade currency (`brokerage_currency` appears in `src/` only in fixtures and the
  statement-total guard)
- [ ] Decide the fix: convert the brokerage leg separately at its own currency's rate (element 2 is
  an amount actually incurred, translated at its own time per s 960-50 — `docs/ato/
  forex-common-transactions.md`), or refuse a `brokerage_currency` that differs from `currency` at
  write time the way `statement_total` already does for the same pair. Refusing is honest and
  cheap; converting is what the field promises
- [ ] Tests: `domain::cost_base` / `reports::open_parcels` for whichever route, plus the Sell side
- [ ] Docs sync: `docs/API.md` Trades (what `brokerage_currency` means for the cost base) and
  `docs/SCHEMA.md`

## A return of capital has no record date, so it reduces parcels bought after the entitlement was fixed (SCENARIOS B-09)
(SCENARIOS.md section B verification pass, 2026-08-15. `corporate_actions.date` for a
`ReturnOfCapital` is the **payment** date, and both the cost-base pipeline and `g1_gains` test
entitlement by it: every parcel with `t.date <= ca.date` is reduced. Entitlement to a return of
capital is fixed at the **record date**, weeks earlier — shares bought after the ex date carry no
entitlement.)
- [ ] Reproduced: parcel bought 2025-02-15, `ReturnOfCapital` of $0.50/unit paid 2025-03-01 — the
  parcel's cost base is reduced by $50 although it was bought ex-entitlement and received nothing.
  Its cost base is understated, so every later gain on it is overstated
- [ ] The converse is right and stays right: a parcel **sold** between the record date and the
  payment is unaffected (checked), matching G1's own "own the shares at the time of the payment"
  test in `docs/ato/cgt-non-assessable-payments.md`
- [ ] `docs/API.md` states the payment-date test as though it were the rule ("reduces the cost base
  of every parcel of the listing held on the payment date"), so nothing warns the user
- [ ] Decide the fix: add an optional record/ex date to the `ReturnOfCapital` payload and test
  entitlement by it (falling back to the payment date when absent, so existing rows are unchanged),
  or document the approximation and the manual correction. Note `income.ex_date` already models
  exactly this distinction for distributions, and the `RightsIssue` action's own `date` **is** its
  record date — the concept is present in the model everywhere but here
- [ ] Tests: `reports::open_parcels` / `reports::net_capital_gain` (a parcel inside the window),
  or `doc_checks` for the documentation-only route
- [ ] Docs sync: `docs/API.md` Corporate actions (`ReturnOfCapital`), `docs/SCHEMA.md`'s
  `corporate_actions.date` comment, and Known limitations if it is documented rather than modelled

## Two documentation gaps found alongside the section B pass (SCENARIOS B-17, B-20)
(SCENARIOS.md section B verification pass, 2026-08-15. Neither produces a wrong figure; both leave
a reader unable to tell what the system did.)
- [ ] B-17 — a Sell's brokerage and GST are **netted off `proceeds`** rather than added to the
  cost base: a 100-unit sale at $12 with $10.945 of costs reports `proceeds: 1189.055` /
  `cost_base: 1010.945`, where the ATO's own presentation is capital proceeds $1,200 against a cost
  base including the disposal's incidental costs (`docs/ato/cgt-cost-base.md`, second element:
  costs "that relate to the CGT event"). The capital gain is identical either way — only the two
  reported components differ — but `docs/API.md`'s realised-gains section defines neither, so a
  user reconciling against an ATO worksheet finds two figures that don't match and a gain that
  does. Document which convention the report uses, and why
- [ ] B-20 — rights **bought on-market** can be exercised only up to the holding's own record-date
  entitlement: `POST /corporate_actions/:id/exercise` caps cumulative units at the entitlement and
  answers `the units exercised exceed the entitlement earned by the holding at the record date`.
  That is a safe refusal, and the cost-base side works (`rights_cost` lands in the parcel's cost
  base and the discount clock runs from exercise, both checked) — but `rights_cost`'s documentation
  ("the total paid to acquire the exercised rights, 0 … for rights issued free") implies purchased
  rights are supported, while Known limitations names only pre-CGT originals and non-renounceable
  retail premiums. Say that rights acquired beyond the holding's own entitlement are not recordable
