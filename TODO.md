# TODO

Items are only marked done when a passing test exists for them.

Completed and decided (out-of-scope / not-reproducible) sections are archived in the topical `DONE/*.md` files, indexed by [DONE.md](DONE.md), to keep this list focused on active work. When a section here is fully done, move it into the matching `DONE/*.md` file rather than leaving it — see CLAUDE.md.

The sections below come from the **2026-07-13 improvement review** (a whole-project pass for
operational and test-strategy gaps, as distinct from the 2026-07-12 programming/domain review whose
findings are all closed in DONE.md), except where a section's heading names another source
(e.g. REQUIREMENTS, SCENARIOS). Each section records one finding; sections land in DONE.md as they
are fixed or decided.

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
