# TODO

Items are only marked done when a passing test exists for them.

Completed and decided (out-of-scope / not-reproducible) sections are archived in the topical `DONE/*.md` files, indexed by [DONE.md](DONE.md), to keep this list focused on active work. When a section here is fully done, move it into the matching `DONE/*.md` file rather than leaving it — see CLAUDE.md.

Sections here come from the **2026-07-13 improvement review** (a whole-project pass for
operational and test-strategy gaps, as distinct from the 2026-07-12 programming/domain review whose
findings are all closed in DONE.md), except where a section's heading names another source
(e.g. REQUIREMENTS, SCENARIOS). Each section records one finding; sections land in DONE.md as they
are fixed or decided.

**SCENARIOS.md sections A–K are driven and every finding they raised is closed** in the `DONE/*.md`
archive. **Section L. Crypto** was driven 2026-08-18; the six findings below are its open work. When
they are closed, the next work comes from driving **SCENARIOS.md section M. Foreign currency and
FX** the same way — walk its scenarios against the running system, and record each gap here as its
own `## ` section.

## Staking rewards and airdropped tokens are reported as dividends (SCENARIOS L-03, L-04)
(SCENARIOS.md section L verification pass, 2026-08-18. The ATO is explicit: the money value of
staking rewards, and of an **established** token received by airdrop, is **ordinary income at the
time of receipt**, declared "as **other income**" — item 24 of the individual return, not item 11
(QC 69950, "Staking rewards and airdrops"; the same page's Anastasia and Merindah examples). The
documented workaround — README + `docs/API.md` Known limitations, "an income row plus a Buy at
receipt-date market value" — has nowhere to put that income: `income.income_type` is
`Dividend | EmploymentIncome`, so the row is a dividend unless it is remuneration.)
- [ ] Reproduced: 0.5 ETH of staking rewards worth A$2,000 entered as an income row on the ETH
  listing → `GET /portfolio/tax-summary` reports `dividends_assessable: "2000"` against ATO label
  **`11S + 11T`**, and the annual tax report prints it in the **Dividends** table with
  `franking_status: "entitled"` — a franking entitlement on a payment no company made. The total
  assessable income is right; every label on it is wrong
- [ ] `income_type: "EmploymentIncome"` is no better: it reports on the tax summary's
  `employment_income` line, which `docs/API.md` describes as item 1/2 salary and wages. Staking
  rewards are neither a distribution of a holding nor remuneration for services
- [ ] The cost-base half is already right: the reward tokens entered as a Buy at receipt-date market
  value open a parcel at that value with its own 12-month clock, exactly as the ATO states, and the
  later sale reports correctly (verified). This finding is only about where the *income* lands
- [ ] Precedent: J-10 (`1d76d3f`) is this finding one income type earlier — the dividend-equivalent
  workaround reported remuneration at 11S, and Evan chose the `income_type` enum over sharpening the
  wording. The same choice is open here (a third variant reported on its own line and in its own
  annual-tax-report table, against item 24), against the cheaper alternative of documenting that
  crypto income must be carried to item 24 by hand
- [ ] Whichever way it goes, the ATO page has no mirror in `docs/ato/` — see the next section
- [ ] Tests: a staking-reward row reports at its own label, is in no dividend total, and carries no
  franking status; the annual tax report prints it in its own table
- [ ] Docs sync: `docs/API.md` Income (`income_type`) + the tax-summary/annual-tax-report field
  tables, README Features / Known limitations

## The crypto limitations say "not modelled" where the ordinary entry path already gives the ATO's figures (SCENARIOS L-04, L-05, L-06)
(SCENARIOS.md section L verification pass, 2026-08-18. `docs/API.md` Known limitations says "Chain
splits/forks, wrapping, and the personal-use-asset exemption are not modelled", and gives one recipe
for staking rewards *and* airdrops together. Driving the scenarios against the running system, three
of those four are not gaps at all — the machinery already produces the ATO's own worked answers, and
what is missing is the sentence naming the entry path, the way the gift and swap entries do.)
- [ ] **Initial-allocation airdrop** (L-04): the ATO's rule is the opposite of the documented recipe
  — you derive **no** ordinary income and make no capital gain on receipt, and the tokens have a
  **cost base of zero** (or what you paid). Reproduced: a Buy of 800 units at price `0` is accepted,
  opens a nil-cost-base parcel with its clock from receipt, and the later sale reports the ATO's
  Josh example exactly — $4,000 proceeds, $4,000 discount-eligible gain, $2,000 after the discount.
  The documented "an income row plus a Buy at receipt-date market value" is wrong for this half of
  L-04 and would overstate assessable income by the full market value
- [ ] **Chain split** (L-05): the new asset is neither ordinary income nor a capital gain on
  receipt, has a nil cost base, is acquired at the split, and is discountable after 12 months
  (QC 69953, Alex's example: 2 Bitcoin Cash for $1,260 → a $630 discount gain). That is the same
  nil-cost-base Buy dated the split. The page's *other* case — no post-split asset continues the
  original, so a **CGT event C2** happens to it — is representable too: reproduced with a
  `WorthlessShares` corporate action carrying `worthless_event: "C2Cancellation"`, whose recognise
  closed the parcel at nil proceeds for a capital loss of $8,300, Ming's stated figure to the dollar
- [ ] **Wrapping** (L-06): the ATO says wrapping or unwrapping a token *is* a CGT event — you
  exchange one crypto asset for another, with capital proceeds equal to the market value of the
  wrapped token received. That is the documented **swap** recipe, already implemented and already
  covered by `ato_examples.rs`'s Katrina test. Saying "not modelled" beside it invites the opposite
  reading — that no CGT event arises — which is the expensive mistake
- [ ] **Stablecoins** (L-14) belong in the same rewrite: TD 2014/25 rules that bitcoin is not
  "foreign currency" for Division 775, and Schedule 2 to the Treasury Laws Amendment (2022 Measures
  No. 4) Act 2023 excluded digital currency from the definition for income years starting on or
  after 1 July 2021. So the Div 775 deferral in the same limitation entry **never reaches a
  stablecoin**: it is a CGT asset like any other crypto, which is exactly what the system does
  (verified: a stablecoin holding bought at A$1.55 and spent at A$1.60 reports a $500 capital gain).
  The entry should say so rather than leaving a reader to wonder which half applies
- [ ] Nothing in `docs/ato/` mirrors any of this: `crypto-cgt.md` covers only the CGT basics and the
  swap, and `OVERVIEW.md` indexes nothing on staking, airdrops, chain splits, or wrapping
- [ ] Fix: mirror the two ATO pages (`staking-rewards-and-airdrops`, `crypto-chain-splits`, plus the
  wrapped-tokens section of `decentralised-finance-and-wrapping-crypto`) into `docs/ato/` with their
  source URLs and retrieval date, index them in `docs/ato/OVERVIEW.md`, and rewrite the crypto
  Known-limitations entry so each case names its entry path — as the gift entry already does
- [ ] Tests: `ato_examples.rs` for Josh (initial-allocation airdrop), Alex (chain split) and Ming
  (the abandoned original, C2); `doc_checks.rs` for the rewritten limitation text
- [ ] Docs sync: `docs/API.md` Known limitations, README Features (the crypto bullet's parenthetical)

## A trading fee paid in crypto has no stated treatment (SCENARIOS L-08)
(SCENARIOS.md section L verification pass, 2026-08-18. Exchanges commonly bill the trading fee in a
crypto asset — the one being traded, or a third token. `PUT /trades` refuses a `brokerage_currency`
other than the trade's, with the documented "enter it converted into the trade's currency". That is
right for the *incidental-cost* leg, and silent about the other one: crypto spent on a fee is itself
a **disposal**, the very rule the holding-account transfer's `fee_allocations` already implements
for an on-chain network fee.)
- [ ] Reproduced: a 1 BTC buy with `brokerage: "0.001", brokerage_currency: "BTC"` → `422`
  "brokerage_currency must equal the trade's currency…"; the same fee entered as `"50"` AUD is
  accepted and lands in the cost base. Nothing anywhere says whether the 0.001 BTC also had to be
  disposed of — and the answer differs by case
- [ ] The three cases a user actually meets: a fee **netted out of the crypto received** (you simply
  acquired fewer units — enter the net quantity, no disposal); a fee **taken from the crypto sold**
  (its AUD value is an incidental cost of the sale — brokerage in the trade's currency, no second
  disposal); a fee **paid in a third asset you hold** (a disposal of those units at market value,
  entered as a Sell, *and* the same AUD value as the trade's brokerage). Only the middle one is
  what the current sentence describes
- [ ] This is live data, not a hypothetical: the 2026-07-13 crypto reconciliation traced a $4.14
  gap to a Binance trade fee charged in ETH
- [ ] Fix (decision): documentation naming the three cases beside the existing brokerage-currency
  limitation, or an entry path — `fee_allocations` on a Buy/Sell, the shape `transfers` already has,
  which would make the disposal atomic with the trade instead of a second row the user must
  remember
- [ ] Tests: whichever way it goes, a fee-in-crypto entry reports the incidental cost in the cost
  base and the disposal (where there is one) in the gains reports
- [ ] Docs sync: `docs/API.md` Known limitations (the brokerage-currency entry) + Trades, README

## The Crypto/exchange pairing refusals answer with a raw CHECK expression (SCENARIOS L-09)
(SCENARIOS.md section L verification pass, 2026-08-18. `listing::db_upsert` validates the
digital-token ticker itself and returns a sentence for it, but leaves the `exchange_mic`/
`security_type` pairing to the database's CHECK — so the two mistakes a user is most likely to make
while adding a crypto listing are answered in SQL.)
- [ ] Reproduced, both directions, identical body: a `Crypto` listing with `exchange_mic: "XASX"`,
  and a `Share` listing with no exchange, each return `422` `a value falls outside its allowed set
  (CHECK constraint failed: (exchange_mic IS NULL) = (security_type = 'Crypto'))`. The web UI shows
  that string in its toast. It does not say which side is wrong, and the two errors are
  indistinguishable
- [ ] Contrast the sibling refusal on the same write: "a Crypto listing's ticker must be a
  recognised digital-token code". `docs/API.md` documents the pairing as two distinct refusals; the
  server does not distinguish them
- [ ] Precedent: A-18 (`2af8d4f`) — a DELETE blocked by an inbound foreign key used to say the row
  did not exist; the fix classified the violation and named it
- [ ] Fix: two `UpsertError` variants checked in `db_upsert` (the `UnrecognisedDigitalToken` shape),
  each with its own message; the CHECK stays as the backstop it is
- [ ] Tests: each direction returns its own sentence, and the CHECK still holds against a direct DB
  write
- [ ] Docs sync: `docs/API.md` Listings + the 422 catalogue wording

## A Crypto listing can be marked `amit` (SCENARIOS L-09)
(SCENARIOS.md section L verification pass, 2026-08-18. An AMIT is an attribution managed investment
**trust**; a crypto asset is not a trust interest — TD 2014/25 says it is not even currency. Nothing
refuses the flag, and the state it creates is unreachable rather than merely odd.)
- [ ] Reproduced: `PUT /listings/1` with `security_type: "Crypto", amit: true` → `204`. The annual
  tax report then answers `completeness.complete: false` with
  `amma_missing: [{listing_id: 1, ticker: "BTC"}]` — an AMMA statement no coin will ever issue — and
  every income row on the listing is refused `422` ("this listing is an AMIT — its distributions are
  trust income") unless it claims to be trust income
- [ ] `amit_from` (migration 0024) is the same flag dated, so it needs the same refusal
- [ ] Fix: refuse `amit` / `amit_from` on a `Crypto` listing at write time, in `listing::db_upsert`
  beside the digital-token check — a write-time invariant, per CLAUDE.md, not a report-time
  complaint
- [ ] Tests: the write is refused with a sentence saying why; a non-Crypto listing is unaffected
- [ ] Docs sync: `docs/API.md` Listings (the crypto paragraph) + the 422 catalogue

## The recognised digital-token list is BTC and ETH until a credentialed import runs (SCENARIOS L-10)
(SCENARIOS.md section L verification pass, 2026-08-18. A `Crypto` listing's ticker must be a
`DigitalToken` row in `currencies`. `0001_schema.sql` seeds exactly two — BTC and ETH — and the rest
come from the ISO 24165 (DTIF) import, which is **skipped with a log warning** unless
`DTI_REGISTRY_USER_ID` / `DTI_REGISTRY_PASSWORD` are set. Out of the box, therefore, no other crypto
asset can be recorded at all, and the refusal does not say why or what to do.)
- [ ] Reproduced: `DOGE`, `USDT` and `WETH` are each refused with the same sentence — "a Crypto
  listing's ticker must be a recognised digital-token code" — with no hint that the list is two rows
  long, that an import fills it, or that the import needs credentials. The live database confirms
  the shape: 178 fiat rows, 2 digital-token rows
- [ ] It is what blocks two of this section's own scenarios from being entered under their real
  tickers (L-06's WETH, L-14's USDT), and would block any real portfolio holding SOL, USDC or ADA
- [ ] The credential requirement *is* documented, in `docs/API.md`'s Currencies import paragraph —
  which is not where a user meets the problem
- [ ] Fix: name the remedy in the refusal itself (import the ISO 24165 registry; the credentials it
  needs), and consider a `reports::health` line when the token list is still only the seeds, the way
  `prices_stale` / `fx_stale` surface a feed that has not run
- [ ] Tests: the refusal names the import; the health line appears only while the list is unimported
- [ ] Docs sync: `docs/API.md` Listings + Currencies, README (setup — what the crypto feature needs
  before it works)
