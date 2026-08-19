# TODO

Items are only marked done when a passing test exists for them.

Completed and decided (out-of-scope / not-reproducible) sections are archived in the topical `DONE/*.md` files, indexed by [DONE.md](DONE.md), to keep this list focused on active work. When a section here is fully done, move it into the matching `DONE/*.md` file rather than leaving it — see CLAUDE.md.

Sections here come from the **2026-07-13 improvement review** (a whole-project pass for
operational and test-strategy gaps, as distinct from the 2026-07-12 programming/domain review whose
findings are all closed in DONE.md), except where a section's heading names another source
(e.g. REQUIREMENTS, SCENARIOS). Each section records one finding; sections land in DONE.md as they
are fixed or decided.

**SCENARIOS.md sections A–M are driven and every finding they raised is closed** in the `DONE/*.md`
archive — section M. Foreign currency and FX was driven 2026-08-19, raised eight findings, and all
eight were closed the same day (see [`DONE/reviews.md`](DONE/reviews.md)). The residual M-12 scope
question is decided too (2026-08-19, option (b): the direct foreign-taxed-disposal path is
documented rather than modelled).

**Section N. Holding accounts and transfers was driven 2026-08-19.** All 12 scenarios were
constructed through the HTTP API and probed: N-01 (whole / partial / three parcels in one transfer),
N-02 (the crypto network-fee disposal), N-03, N-04, N-05 (transfer → sell in the destination →
delete), N-07 (both orderings across a split), N-09, N-10 (per-account DRP enrolment), N-11
(portfolio and performance) and N-12 all came back **correct**, along with the standing probes on
snapshot staleness, the annual tax report and `row_history`. The four findings below are what the
pass raised. The next section after these are closed is **O. Net capital gain, losses, and
carry-forward**.

## A cost-base fact dated before a rollover, entered after it, restates only the source side (SCENARIOS N-06, N-07)
(SCENARIOS.md section N verification pass, 2026-08-19. `domain::rollover` **stores** each
replacement parcel's carried cost base — on the replacement Buy's `brokerage` column — computed from
`CostBaseInputs` as they stood when the operation ran. Every later-entered fact dated on or before
the operation restates the *source* parcel, which the reports still walk, but cannot reach the
frozen figure.)
- [x] Reproduced (return of capital): 100 units bought 2023-01-10 for $500, a $1.00/unit return of
  capital on 2023-05-01, transferred to another holding account 2023-08-01, sold 2024-06-01 for
  $900. Entering the ROC **before** the transfer reports cost base $400 / gain $500 (correct — G1
  reduces the cost base by $100). Entering the same ROC **after** the transfer reports cost base
  $500 / gain **$400** — a $100 understated capital gain. No refusal, no flag, and the E4
  cross-check is empty in both runs. The order of *entry* changes the tax figure
- [x] Reproduced (split, worse): the same parcel transferred 2023-08-01, then a 2-for-1 split dated
  2023-05-01 entered afterwards. `db_units_sold` re-bases the transfer-out Sell's allocation into
  post-split units, so it now consumes only 50 as-acquired units — the **source parcel reappears as
  an open holding** (100 current units, cost base $250) beside the untouched destination parcel (100
  units, $500). The portfolio reports 200 units and $750 of cost base where the taxpayer holds 200
  units and $500
- [x] Reproduced on a scrip-for-scrip exchange too (`domain::rollover` is shared): a ROC on the old
  listing dated before the exchange, entered after it, leaves the replacement parcel at $500 instead
  of $400. A demerger has the same shape
- [x] The AMIT half of this is **already guarded** — F-17's `UnitsCarriedIntoReplacement` refuses an
  adjustment covering units a rollover carried away, naming the replacement parcels. Corporate
  actions have no equivalent guard, and the split case is about `quantity`, which no guard covers
- [x] Deleting a folded-in ROC *is* refused (A-06's guard), so only the *adding* direction is open
- [x] The live database has no exposure today: the only rollovers are 10 transfers (ICE / BTC / ETH,
  listings with no corporate actions at all) and the LAC demerger, whose only same-listing action is
  the demerger itself, dated the same day. Checked read-only
- [ ] A model decision, four options:
  - **(a)** Extend F-17's guard to corporate actions: refuse a `ReturnOfCapital` / `ShareSplit` /
    `BonusIssue` write dated on or before an existing rollover of that listing whose closing Sell
    consumed parcels the action would restate (excluding the action that *created* the rollover),
    with the message F-17 already uses — delete the operation, enter the action, re-run it. Fails
    safe, matches the guard that exists, refuses nothing in the live DB
  - **(b)** A cross-check report (and a health alert) that recomputes each rollover's carried cost
    base and quantity from today's facts and lists every replacement parcel whose stored figure no
    longer matches — entry stays unrestricted, and the stale rollover is named until it is redone
  - **(c)** Both (a) and (b): the guard for the writes it can see, the cross-check for the states
    that predate it or arrive by another route
  - **(d)** Documentation only: a Known limitation stating that a rollover freezes its carried cost
    base, so any fact dated before it must be entered first
- [ ] Not proposed: making the rollover *derive* the carried cost base instead of storing it. It is
  the right model, but the cost base lives on the replacement Buy's `brokerage` column in the
  parcel's own currency, threaded through `domain::cost_base` and all three operations — the K-pass
  lesson is to measure that before choosing it, and it is a much larger change than this finding
- [ ] Tests: per the option — the two reproductions above as regression tests, plus the refusal or
  the cross-check row
- [ ] Docs sync: `docs/API.md` (the 422 catalogue and/or the new report), README Known limitations,
  `docs/SCHEMA.md` if a report is added

## An AMMA statement whose units a transfer has moved can be recorded nowhere (SCENARIOS N-06)
(SCENARIOS.md section N verification pass, 2026-08-19. The sibling of the finding above, on the
guarded path: F-17 refuses the adjustment against the source parcel, and the per-account rule
refuses it against the replacement. AMMA statements arrive in August–September for a year ended
30 June, so a transfer between the year end and data entry is the ordinary case, not an edge.)
- [x] Reproduced: 100 VDHG units in account 2 from 2022-08-10, transferred to account 3 on
  2023-08-01, then the FY2023 AMMA statement (issued for account 2, received 2023-09-15) entered.
  Every route is refused — `POST /amma_statements/1/generate_adjustments` → 422
  `UnitsCarriedIntoReplacement`; the same by hand against the source parcel → the same 422; by hand
  against the replacement parcel → 422 "the trade sits in a different holding account from the AMMA
  statement"
- [x] Re-filing the statement under the destination account is **accepted silently** (204) even
  though account 3 held nothing at the statement's year end, so generation then answers
  `NothingHeld` and the statement is attributed to an account the registry never issued it for
- [x] Two surfaces advise the entry the guard refuses: the `NothingHeld` message ("if the holding
  was sold or transferred away during the year … enter one AMIT adjustment by hand against each
  parcel those units came from") and the generate-adjustments action description in
  `src/web/config.js`. Only the F-17 refusal names the workable recovery — delete the transfer,
  enter the adjustment, re-transfer — and that path does work end to end
- [x] The AMIT adjustment cross-check does keep the statement flagged throughout ("no AMIT
  adjustments entered, so the statement's 1.5 per-unit cost base adjustment reaches no parcel"), so
  nothing is silently wrong here — the figure is unrecordable, not misreported
- [ ] A model decision, three options:
  - **(a)** Let the adjustment reach through the rollover chain: accept a row against a replacement
    parcel when the parcel descends (via `trades.transfer_id` / the rollover provenance columns)
    from a parcel that was in the statement's holding account at the statement's year end, and
    apply it to the replacement's carried cost base. Records the fact where the units now are, and
    is the only option that leaves the statement attributed as the registry issued it
  - **(b)** Fix the advice and add the recovery as an operation: make the two misleading messages
    name the delete-and-redo path, and refuse re-filing a statement under an account that held
    nothing of its listing at its year end
  - **(c)** Documentation only: a Known limitation stating that a rollover must be deleted and
    re-run to record a statement covering units it moved
- [ ] Tests: per the option — the reproduction above ends with the adjustment recorded (a) or with
  every refusal naming the recovery (b)
- [ ] Docs sync: `docs/API.md` 422 catalogue, README Known limitations, `src/web/config.js`'s action
  description

## The ESS 30-day-rule alert fires on a holding-account transfer, which is not a disposal (SCENARIOS N-08)
(SCENARIOS.md section N verification pass, 2026-08-19. `reports::health`'s `db_ess_30_day_rule`
pairs every `parcel_allocations` row whose Sell falls 1..=30 days after a statement's taxing point
with that statement. It does not exclude the transfer-out Sell, which carries `transfer_id` and is
not a disposal — the same filter `realised_gains`, `net_capital_gain`, `wash_sales` and
`franking_at_risk` all apply.)
- [x] Reproduced: an ESS statement with a 2024-03-01 taxing point, 100 shares at $20 with a $2,000
  deferral discount, vested into holding account 2, then transferred to account 3 on 2024-03-11 —
  the RSU-plan-to-broker move `entities::transfer`'s own module doc gives as the feature's purpose.
  `GET /reports/health` reports one `ess_30_day_rule` row (`days_after: 10`, the full $2,000
  discount) while `GET /portfolio/realised-gains` correctly reports nothing
- [x] Consequence: the alert says the taxing point moves to the "disposal" date and the capital gain
  is cancelled — advice that, followed, re-measures an assessable discount and amends a return over
  a change of custody. The 30-day rule is `docs/ato/employee-share-schemes.md`'s "a **disposal**
  within 30 days after the deferred taxing point"; beneficial ownership is unchanged by a transfer,
  which is why no CGT event arises
- [ ] Fix: `AND s.transfer_id IS NULL` in `db_ess_30_day_rule`, with a regression test that the
  alert stays silent for a transfer inside the window and still fires for a real sale in it
- [ ] Open question in the same area, for the same fix: a scrip-for-scrip exchange or demerger
  closing Sell also reaches the alert. Those *are* CGT events, but ESS interests replaced under a
  takeover or restructure are treated as continuing the originals — if that is right they should be
  excluded too, and the ATO source for it needs mirroring into `docs/ato/` before the code claims it
- [ ] Docs sync: `docs/API.md`'s health-report entry if the alert's stated scope changes

## A transfer's parcel rejection lists five causes and names none of the real ones (SCENARIOS N-04, N-12)
(SCENARIOS.md section N verification pass, 2026-08-19. `TransferError::Sell` collapses every
`sell::SellError` variant into one sentence, where `PUT /sells` answers a precise message per
variant.)
- [x] Reproduced (N-12): a transfer dated 2023-01-01 of a parcel acquired 2023-02-10 is correctly
  refused with nothing persisted (`422`, no transfer row, no trades) — but the body reads "the
  selected parcels are invalid: missing, over-allocated, not a Buy/DRP, or held in a different
  account from the source". The parcel is none of those; the actual cause is
  `SellError::PurchaseAfterSale`, whose own message is "an allocated parcel is dated after the sale
  date" and which the sentence does not list at all
- [x] Reproduced (N-04): transferring a parcel a Sell already consumed, and back-dating a transfer
  over a later sale of the same units, are both correctly refused and both answer that same
  sentence — the only true clause for them being "over-allocated"
- [ ] Fix: map the Sell-side causes the transfer can actually hit to their own messages (the parcel
  is dated after the transfer date; the units are already consumed; the parcel is not a Buy/DRP;
  the parcel is in another account; the quantity is not positive), phrased for a transfer rather
  than a sale, keeping the catch-all only for the variants a transfer cannot reach
- [ ] Tests: one rejection test per mapped cause asserting the message names it, in
  `entities::transfer`'s inline module
- [ ] Docs sync: `docs/API.md`'s 422 catalogue row for `PUT /transfers/:id`
