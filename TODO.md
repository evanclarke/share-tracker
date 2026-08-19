# TODO

Items are only marked done when a passing test exists for them.

Completed and decided (out-of-scope / not-reproducible) sections are archived in the topical `DONE/*.md` files, indexed by [DONE.md](DONE.md), to keep this list focused on active work. When a section here is fully done, move it into the matching `DONE/*.md` file rather than leaving it — see CLAUDE.md.

Sections here come from the **2026-07-13 improvement review** (a whole-project pass for
operational and test-strategy gaps, as distinct from the 2026-07-12 programming/domain review whose
findings are all closed in DONE.md), except where a section's heading names another source
(e.g. REQUIREMENTS, SCENARIOS). Each section records one finding; sections land in DONE.md as they
are fixed or decided.

**SCENARIOS.md sections A–L are driven and every finding they raised is closed** in the `DONE/*.md`
archive — section L. Crypto was driven 2026-08-18, raised six findings, and all six were closed the
same day (see [`DONE/reviews.md`](DONE/reviews.md)).

**Section M. Foreign currency and FX was driven 2026-08-19** and raised the eight findings below.
Once they are closed, the next work comes from driving **SCENARIOS.md section N. Holding accounts
and transfers** the same way — walk its scenarios against the running system, and record each gap
here as its own `## ` section.

## The annual tax report's printed sell-side FX rate is not the rate the proceeds used (SCENARIOS M-01, M-02)
(SCENARIOS.md section M verification pass, 2026-08-19. The annual tax report is the print-to-PDF
document the year is archived as, and each non-AUD disposal row prints `currency`,
`buy_month_fx_rate` and `sell_month_fx_rate` beside its AUD figures so the arithmetic can be
checked. The buy side prints the rate actually applied; the sell side prints the ATO monthly rate
whatever the proceeds used.)
- [x] Reproduced (a): a Sell of US$20,000 carrying `spot_fx_rate: 0.5000` in a month whose ATO rate
  is 0.6800 prints `proceeds_aud: 40000` (= 20000 / 0.50, correct) beside
  `sell_month_fx_rate: 0.6800`, which computes A$29,411.76 — a A$10,588 gap between the printed
  figure and the printed rate, in the document a reader checks the return against
- [x] Reproduced (b): a Sell in a month with no imported ATO rate, resting on its own `fx_rate` of
  0.55, prints `proceeds_aud: 36363.64` beside `sell_month_fx_rate: null` — the fallback rate the
  buy side *does* print (via `fx_override()`) is hidden on the sell side
- [x] Cause: `reports::tax_report`'s `sell_rate` resolves with `FxOverride::None` where `buy_rate`
  resolves with `bt.fx_override()`. It is also keyed on the *buy* trade existing
  (`buy_trade.and_then(|_| …)`), which is unrelated to the sale's own conversion
- [x] Fix: resolve the sell-side rate from the **sale trade's** override, mirroring the buy side, so
  the printed rate is always the rate the printed proceeds were computed at
- [x] Tests: a spot-override Sell and a fallback Sell each print the rate their proceeds used;
  an AUD disposal still prints neither
- [x] Docs sync: `docs/API.md`'s Annual tax report section, where the two rate columns are described

**Resolution (2026-08-19): each side prints the rate its own figure was converted at.**

`reports::tax_report`'s `sell_rate` now resolves from the **disposal's own** override, mirroring
`buy_rate`: a Sell's `fx_override()` (its `spot_fx_rate` when set, else its `fx_rate` fallback), and
for a rights sale — which is not a trade, so it is not in `DisposalInputs::trades` — that row's
`fx_rate` against the issue's currency, loaded into a new `rights_sales` map. The rate is no longer
keyed on the *buy* trade existing.

Tests: `a_disposals_printed_fx_rates_reproduce_its_printed_aud_figures` drives both cases and
asserts each printed AUD figure equals its native amount divided by the rate printed beside it —
the property the document is printed for; `an_aud_disposal_prints_no_fx_rates` keeps the AUD case
printing neither. Docs: `docs/API.md`'s Annual tax report disposal-row description, and the web
document's own note now reads "buy-side rate / sale-side rate" rather than "buy-month / sell-month",
which was only true when no override was in play.

## A missing ATO rate answers a tax report with a bare `500` and an empty body (SCENARIOS M-04, M-07)
(SCENARIOS.md section M verification pass, 2026-08-19. A non-AUD income or AMMA record has no
per-record fallback by design, so a month with no imported rate is a loud failure — the right
behaviour. What reaches the user is `500 Internal Server Error` with an empty body, which the web UI
can only show as "HTTP 500".)
- [x] Reproduced: one USD income row in a month with no `rba_fx_rates` row → `GET
  /portfolio/tax-summary` answers `500`, body empty. The cause is named precisely in the server log
  (`no ATO FX rate for USD in 2023-05 and no manual override supplied`) and nowhere else
- [x] The same gap in the *valuation* path already answers well: `POST /report_snapshots/generate`
  returns `422 AAPL: no ATO FX rate for USD in 2024-05 and no manual override supplied`. One class
  of problem, two answers
- [x] This is not an internal detail: it is a data gap the user fixes by running the RBA import (or
  entering the rate the record converted at), and they cannot act on a blank 500
- [x] Cause: `impl From<FxError> for sqlx::Error` turns a `MissingRate` into `sqlx::Error::Decode`
  so it cannot be swallowed, and `ApiError`'s `From<sqlx::Error>` then classifies every Decode as
  `Internal`. The classification is right for a malformed stored decimal and wrong for this
- [x] Fix: carry the missing-rate case through the report error path so it lands as `422` naming the
  currency and month, like the snapshot path
- [x] Tests: the affected reports answer `422` naming the currency and month; a genuine decode
  failure still answers `500`
- [x] Docs sync: `docs/API.md` Response codes (the 422 catalogue) and the FX conversion section,
  which stated the failure as `500`

**Resolution (2026-08-19): the `FxError` is carried, not stringified, and a missing rate answers
`422` naming the currency and month.**

`impl From<FxError> for sqlx::Error` now boxes the `FxError` itself into `sqlx::Error::Decode`
instead of its `to_string()`, so the far end can get it back. `impl From<sqlx::Error> for ApiError`
downcasts a decode error to `FxError` and routes a `MissingRate` through the new
`missing_rate_unprocessable` — a `422` whose body is the error's own sentence plus the remedy
(`import that month's rates with POST /rba_fx_rates/import`), logged at warn with the currency and
month. Every other decode failure — a malformed stored decimal, the case the classification was
written for — stays the `500` it should be, and `FxError::Db` (a failed lookup, a genuine fault)
does too. `impl From<FxError> for ApiError` classifies the same way, so a rate raised directly and
one carried through a report answer identically.

Tests: `infra::http`'s `a_missing_fx_rate_is_a_422_naming_the_currency_and_month` (both routes into
the classification, plus a non-FX decode error still 500 with an empty body) and
`tax_summary`'s `api_a_month_with_no_ato_rate_is_a_422_naming_it_not_a_bare_500` (the tax summary,
its CSV export and the annual tax report end to end, and that importing the month unblocks them).
Docs: the FX conversion section's rule 4 and every "fails loudly with `500`" in `docs/API.md`, plus
the 422 catalogue.

## Nothing lists which (currency, month) rates the recorded data needs (SCENARIOS M-04, M-14)
(SCENARIOS.md section M verification pass, 2026-08-19. `GET /reports/health` reports
`latest_fx_month` and `fx_stale` — the newest imported month across all currencies, and whether it
is old. That answers "has the import run lately", not "is every amount I have recorded convertible".)
- [ ] Reproduced (M-14): an F11 CSV with an empty February cell imports January and March and skips
  February silently (`{"inserted": 2}`); health then reports `latest_fx_month: "2024-03"` — healthy
  by its own measure — while a February USD trade is costed from its own `fx_rate` of 0.99 at
  A$15,151 where the real rate would give A$22,727, and a February income row would `500`
- [ ] The gap is invisible in both directions: a *silent* one (an amount resting on a per-trade
  `fx_rate` fallback because its month is missing) and a *fatal* one (an income/AMMA amount with no
  fallback at all, which fails the whole report)
- [ ] The analogous reference-data gap already has its own report: `reports::settlement_coverage`
  lists every trade whose settlement window falls outside the seeded exchange-holiday years,
  non-blocking, "an empty report means every settlement window was computed against a complete
  calendar". FX has no twin
- [ ] Fix: a coverage cross-check in `reports/` on that model — for every (currency, month) some
  recorded amount converts at, whether an ATO rate exists, and for each miss what the amount
  currently rests on (a trade's own `fx_rate`, a spot override, or nothing at all → the report will
  fail). Registered like its siblings in `reports::mod`, a `REPORTS` entry in `config.js`, and worth
  a health-banner line since a fatal gap is not something to discover at tax time
- [ ] Tests: a complete series reports empty; a hole in the middle names the currency and month and
  what each affected amount rests on; an AUD-only portfolio reports empty
- [ ] Docs sync: `docs/API.md` (a new report section + the reports list), README Features

## A listing's `currency` is freely editable, silently re-denominating every stored price (SCENARIOS M-08)
(SCENARIOS.md section M verification pass, 2026-08-19. `listing::db_upsert` freezes `ticker` and
`exchange_mic` once a listing has trades, income or closing prices — an identity change must go
through `POST /listings/:id/rename` so it is recorded. `currency` is not in that list, though it is
just as much part of the listing's identity: every stored closing price is denominated in it.)
- [ ] Reproduced: a USD listing with a Buy, a stored price of 200 and a generated snapshot. `PUT
  /listings/1` changing `currency` to EUR → `204`. The stored snapshot still reads `current_price:
  298.51` (200 / 0.67) and — because `listings` has no snapshot-staleness trigger — is still
  `stale: false`. Regenerating the same date silently answers `333.33` (200 / 0.60). The same stored
  fact, two AUD valuations, nothing marked
- [ ] Trades keep their own `currency`, so cost bases are unaffected; the damage is to every
  price-derived figure (the overview, unrealised gains, performance, every snapshot in the series)
- [ ] Two questions for the model, worth asking together:
  - **(a)** What should a currency change *be*? A redenomination is a real event (a listing moving
    quote currency, a currency replaced) — so either it joins the rename path as a recorded event
    with an effective date (prices before it are in the old currency, after it in the new), or it is
    refused outright once there is history and the answer is a new listing plus a transfer
  - **(b)** Regardless of (a), `listings` needs snapshot-staleness triggers: a change to the row
    that a snapshot's figures depend on must stale the snapshots, which is the schema's rule for
    every other dated fact
- [ ] Tests: whichever of (a) — the refusal, or the recorded event with prices resolved per span;
  and for (b) that a listing edit marks snapshots stale
- [ ] Docs sync: `docs/SCHEMA.md` (the triggers), `docs/API.md` Listings + the 422 catalogue,
  README/Known limitations if (a) lands as a refusal

## A trade may be recorded in a currency other than its listing's (SCENARIOS M-08)
(SCENARIOS.md section M verification pass, 2026-08-19. Four entities now refuse a currency that is
not the listing's, each for the same reason: ESS statements — "the per-share market value and the
listed price are the same money" — inheritances, `ReturnOfCapital` corporate actions, and a DRP
reinvestment's distribution. The trade, whose `average_price` *is* the listed price and whose
currency drives the cost base, has no such check.)
- [ ] Reproduced: `PUT /trades/1` with `currency: "USD"` on an AUD-quoted ASX listing → `204`. The
  parcel is then costed by dividing an AUD price by a USD rate. `PUT /income/1`, `PUT
  /amma_statements/1` and `PUT /investment_expenses/1` accept the same mismatch
- [ ] The four cases are not equally strong, which is the decision to take:
  - **Trades** are the strong case, and the same argument the ESS refusal already makes:
    `average_price × quantity` is the security's own price, so it is the listed currency by
    construction. A Sell shares the check via the Sell path
  - **AMMA statements** attribute a distribution of the listed trust — the same money as its price
  - **Income** is weaker: a distribution is normally paid in the listing's currency (which is why
    the DRP reinvest refuses otherwise), but a custodian paying an AUD dividend on a US holding is
    conceivable
  - **Investment expenses** are the weakest and probably should stay free: an Australian adviser's
    AUD fee attributed to a US holding is the ordinary case
- [ ] Note this has a data question behind it: the rule must be checked against the live database
  before it lands, since an existing row it would now refuse cannot be edited afterwards
- [ ] Fix: the refusal on whichever set (a) chooses, in each entity's `db_upsert`, naming both
  currencies as the ESS and inheritance refusals do
- [ ] Tests: per entity, the mismatch refused naming both currencies; a matching currency accepted;
  an AUD listing with an AUD row unaffected
- [ ] Docs sync: `docs/API.md` per entity + the 422 catalogue

## A stored RBA rate can never be corrected, and a differing feed value is silently discarded (SCENARIOS M-13)
(SCENARIOS.md section M verification pass, 2026-08-19. `rba_fx_rates` is written only by the import,
which is `INSERT … ON CONFLICT DO NOTHING`, and the resource is read-only over HTTP — no `PUT`, no
`DELETE`. First value wins, permanently.)
- [ ] Reproduced: importing `29-Mar-2024,0.6500` then `29-Mar-2024,0.6512` answers
  `{"inserted": 0, "skipped": 1}` and stores 0.6500. The response cannot distinguish "the feed
  repeated what we had" from "the feed disagreed with what we had"
- [ ] Consequence: a rate that lands wrong — a hand-supplied retry body with a typo (the endpoint
  accepts a pasted CSV precisely for retries), a truncated download, or an upstream revision — can
  be fixed only by editing the database by hand, and every tax figure in that currency-month rests
  on it. `rba_fx_rates` is also **not** in `row_history::AUDITED_TABLES`, so a hand-edit leaves no
  trace either
- [ ] The idempotency the `DO NOTHING` buys is worth keeping: re-running the import must not
  rewrite history unasked, and a silently-changing rate would be worse than a stuck one
- [ ] A model decision, three options:
  - **(a)** Keep the import idempotent but *report* the disagreement: count `conflicted` separately
    from `skipped`, listing each (currency, month, stored, feed), and surface it in the job's
    failure detail and the health report. Nothing changes without the user asking
  - **(b)** (a), plus an explicit correction path — a `PUT /rba_fx_rates/:id` (or an import flag)
    that overwrites, with `rba_fx_rates` added to the audited tables so the old value is recorded
    and the snapshots it fed marked stale
  - **(c)** Documentation only: state in `docs/API.md` that the first imported value for a
    (currency, month) is final and a correction needs direct database access
- [ ] Tests: per the option — a differing re-import is counted and named; an identical one is not;
  a correction (if (b)) restages the affected snapshots and writes a history row
- [ ] Docs sync: `docs/API.md` RBA FX rates + Response codes; `docs/SCHEMA.md` and the three audited
  -table lists if (b)

## Foreign tax on a discountable foreign capital gain is claimed in full, not apportioned (SCENARIOS M-12)
(SCENARIOS.md section M verification pass, 2026-08-19. The FITO guide's "Foreign income tax paid on
part of an amount included in your income" (QC 104349, *When a FITO applies*) states: "If only part
of a foreign capital gain is assessable in Australia (for example, the gain is subject to the
discount capital gains concessions in Division 115 of the ITAA 1997) the foreign tax paid on the
gain must be apportioned accordingly. This includes, where a foreign capital gain is distributed to
a unitholder of a … (AMIT). In such circumstances, when calculating your FITO, the 'Foreign tax
offset applicable to discountable capital gains' shown at Part C … must be reduced for discounted
capital gains." The AMMA guidance notes confirm the trustee reports the **gross** foreign tax and
the reduction is the investor's job — this system's job.)
- [ ] Reproduced: an AMMA statement with `cgt_discount_gains: 5000` and `foreign_tax_credits: 1500`
  reports `foreign_tax_offsets: 1000` and `foreign_tax_offset_excess: 500`. Apportioned to the
  assessable half, the claimable figure is A$750 — so the report's A$1,000 over-claims by A$250,
  and a smaller de-minimis-covered case over-claims by the full apportionment
- [ ] The de-minimis cap bounds the damage at A$1,000 but does not remove it, and the excess figure
  the user is told they *may* claim with their own limit calculation is overstated by the whole
  un-apportioned amount
- [ ] The blocker is the data model: `amma_statements.foreign_tax_credits` is one field for both
  "foreign tax on foreign income" and "foreign tax on foreign capital gains", which the AMMA's own
  Part C reports as separate lines. The apportionment applies only to the second, so it cannot be
  computed from what is stored
- [ ] The same is true of a *direct* foreign-taxed disposal: foreign tax paid on a capital gain the
  taxpayer realises themselves has nowhere to be recorded at all
- [ ] A model decision, two options:
  - **(a)** Split the field — a new `foreign_tax_credits_capital_gains` column (migration; the AMMA
    is audited, so its two `*_row_history_*` triggers must be dropped and re-created) — and apply
    the Division 115 reduction to that half in the tax summary, with the AMMA screen's field hint
    naming which Part C line each takes. The system then computes the ATO's figure
  - **(b)** Documentation only: a Known-limitations entry stating that a `foreign_tax_credits`
    figure attributable to a discountable foreign capital gain must be entered already reduced, with
    the ATO citation, and the AMMA field hint saying so
- [ ] Mirror the ATO page into `docs/ato/` with its source URL and retrieval date and index it in
  `docs/ato/OVERVIEW.md` either way — nothing there covers the FITO apportionment rule today
  (`fito-limit.md` mirrors only the offset-limit page)
- [ ] Tests: per the option — the apportioned offset computed, or `doc_checks.rs` for the entry
- [ ] Docs sync: `docs/SCHEMA.md` + `config.js` if (a); `docs/API.md` Known limitations either way

## The two documented FX simplifications are silent where their sibling is refused (SCENARIOS M-09, M-10)
(SCENARIOS.md section M verification pass, 2026-08-19. Both simplifications are honestly documented
and both behave exactly as documented — the verification confirmed each. What neither has is a
surface telling a user that *their* data has hit it, though in both cases the affected rows are
identifiable from stored facts. The third member of the family, LPR expenditure on a foreign
inherited parcel, was refused outright at write time in the section K pass for the same reason.)
- [ ] **K10/K11 (M-09)**: reproduced with a US$1.5m disposal contracted 27 March (rate 0.66) and
  settled 2 April (rate 0.60). Proceeds convert at the contract month, correctly; the A$227,272 of
  settlement-window movement is a CGT event K10 gain or K11 loss the system does not compute, per
  the Known-limitations entry. A trade at risk is exactly identifiable: non-AUD, and
  `date`'s month ≠ `settlement_date`'s month
- [ ] **Cost-base FX timing (M-10)**: reproduced with a USD parcel acquired at 0.70 taking a USD
  AMIT reduction whose own month is 0.60 — the reduction converts at 0.70 (A$2,857 where its own
  month gives A$3,333), keeping `initial − reductions = adjusted` exact in AUD. Affected rows are
  likewise identifiable: a non-AUD parcel with a non-AUD AMIT or return-of-capital reduction. The
  limitation says this "in practice does not arise"; nothing checks whether it has
- [ ] Fix: surface both, non-blocking, on the `reports::settlement_coverage` model — one alert per
  affected trade/parcel naming the two months and what the omission is. Natural home is the FX
  coverage report proposed in the sibling finding above, as a second alert kind, or the health
  report if that one is not taken
- [ ] Tests: a same-month settlement and an AUD trade produce no alert; a cross-month non-AUD
  settlement does; a non-AUD parcel with a non-AUD reduction does; an AUD fund with an AMIT
  reduction does not
- [ ] Docs sync: `docs/API.md` — both Known-limitations entries gain the sentence naming where the
  affected rows are listed
