# Project Overview

This project is a comprehensive share tracker, where facts about the investing activity are recorded
and an overview of the portfolio can be materialised for given market prices from these facts.  Reporting
and cost basis calculations are done with the Australian tax view in mind.

# Status

Everything specified here through 2026-06-07 is implemented (or explicitly resolved out of scope) and
is documented in `README.md`: the Features, Database schema, and HTTP API sections describe the
implemented behaviour, and the Known limitations section records the resolved out-of-scope decisions
(taxpayer entity type, cost-base elements 3–5 / reduced cost base, taxpayer-level accounts, DRP partial
participation, ESS income). The full historical requirement text and each item's resolution are
preserved in git history and in `TODO.md`'s entries. Ongoing engineering rules (Decimal-only money,
non-destructive migrations, enum constraints, write-time invariants, …) live in `CLAUDE.md`.

# New Requirements

New requirements are written below and then folded into `TODO.md`.

## Crypto-asset holdings (BTC, ETH, …)

Investment holdings of crypto assets (e.g. Bitcoin, Ether) can be recorded and reported alongside
share holdings. The ATO treats investment crypto as a CGT asset, so the existing parcel-level CGT
machinery (parcel allocations, reduced cost base, the 12-month 50% discount, loss netting) applies
unchanged — the work is letting an asset exist without an exchange, not new tax rules.

- Crypto assets as listings
  - A new listing security type `Crypto`; a Crypto listing has **no exchange** (`exchange_mic` is
    absent for it, and only for it — the existing security types must still require an exchange,
    enforced by database constraint and at write time)
  - A Crypto listing's ticker must be a recognised digital-token code in the currencies reference
    table (kind DigitalToken, from the ISO 24165 / DTIF import) — an unrecognised token is rejected
  - Exchange-less listings are unique by ticker (the existing per-exchange uniqueness can't cover
    them)
  - The listing's currency is the pricing currency for its trades (e.g. AUD for BTC bought on an
    AUD market); quantities are token units — the existing arbitrary-precision Decimal storage
    already covers satoshi/wei-scale fractions
- Settlement
  - Trades on an exchange-less listing settle same-day: the auto-calculated settlement date is the
    trade date (no T+N, no holiday calendar)
- Reference-data reports
  - The exchange MIC validation report and the settlement-holiday coverage report skip
    exchange-less listings (nothing to validate; no holiday calendar applies)
- Tax and reporting
  - Crypto parcels flow through the portfolio, unrealised, realised, performance, and net capital
    gain reports exactly like share parcels: cost base and proceeds in AUD, discount eligibility
    after 12 months, ATO-ordered loss netting
  - Market prices for valuation continue to be supplied in the report request (no crypto price
    feed)
  - Holding-account transfers of crypto (e.g. wallet to wallet) work like any listing transfer:
    not a CGT event, cost base and acquisition date carried
- ATO documentation
  - Mirror the ATO's crypto-as-CGT-asset guidance into `docs/ato/` (source URL + retrieval date,
    indexed in `OVERVIEW.md`); add `ato_examples.rs` acceptance tests for any worked examples the
    feature makes representable
- Web UI
  - The listings screen supports the Crypto type with the exchange field optional accordingly;
    crypto trades/parcels/reports need no special casing beyond the existing config-driven screens

Out of scope (record as Known limitations):
- **Foreign-currency cash balances** — deferred. Holding FX itself is generally not CGT (Division
  775 forex realisation gains/losses are ordinary income, with the $250k limited-balance election),
  so it needs its own tax engine and will be specified separately
- **Crypto-to-crypto swaps** (e.g. BTC→ETH) as an atomic operation — a swap is a disposal at market
  value and can be entered manually as a Sell plus a Buy; a dedicated swap operation (like the
  buy-back/scrip participations) is deferred
- **Staking rewards and airdrops** — ordinary income at receipt with a cost base equal to market
  value at receipt; can be entered manually (income row + Buy at the receipt-date market value),
  a linked operation is deferred
- **Chain splits/forks, wrapping, and the personal-use-asset exemption** — not modelled

## Daily closing prices and scheduled report snapshots

Closing prices for every listing currently held are collected automatically at the end of each
exchange's trading day and kept as a price history. Once the day's last close is in, the
price-dependent reports are run against the stored prices and their results stored as a daily
snapshot series — viewable and graphable over time. Recording a back-dated fact invalidates the
affected snapshots so they can be regenerated against the new facts.

- Closing-price collection
  - For each listing with a non-zero holding, fetch the closing price after that listing's
    exchange closes for the day (per-exchange timing via the cron scheduler in
    `infra/scheduler.rs` + `schedule.cron`). Only trading days: skip weekends and the exchange's
    seeded holidays — a non-trading day has no price row, it is not an error
  - Exchange-less (Crypto) listings trade continuously: collect one daily reference price at a
    fixed cut-off (convention defined at implementation, e.g. UTC midnight). This supersedes the
    earlier "no crypto price feed" limitation for *stored history*; report requests with
    explicitly supplied prices keep working unchanged
  - Prices are stored in the listing's quote currency (TEXT Decimal) with provenance: source,
    fetch timestamp, and a status. A failed fetch is recorded as an errored row for that
    (listing, day) — never silently missing — and can be re-run on demand for just that
    day/listing (manual trigger endpoint)
  - One price per (listing, date), enforced by constraint; a re-run replaces the errored row
  - The price source is a pluggable fetcher (trait) so providers can be swapped; the concrete
    provider(s) — and their key handling and rate limits — are chosen and documented at
    implementation time
- Price history
  - The history is viewable: an endpoint listing stored prices (filterable by listing and date
    range, including errored rows) and a web UI screen via the existing `filterableTable`
- Backfill
  - Price history can be backfilled on demand for a listing and date range — e.g. after importing
    an old trade, backfill from the trade date to today. Backfill fetches only trading days and
    skips dates already stored successfully
- Scheduled report snapshots
  - After the last relevant exchange close of the day, run the price-dependent reports (portfolio
    overview / valuation, unrealised gains, performance) using that day's stored closing prices —
    converted to AUD per the existing FX rules, since stored prices are in the listing's quote
    currency — and store the results as that date's snapshot
  - Snapshots are viewable (endpoint + UI) and graphable in the web UI as time series (e.g.
    market value and unrealised gain over time), without introducing a build step
  - Where prices have been backfilled, snapshots for those past dates can be generated on demand
    too
- Staleness and regeneration
  - Adding, changing, or deleting any back-dated fact (trade, corporate action, income,
    transfer, …) marks every stored snapshot on or after that date as out of date. Stale
    snapshots are visibly flagged wherever they are shown, and can be regenerated on demand —
    regeneration re-runs the report with the stored prices and the new facts, replacing the
    stale result
  - A snapshot is missing-vs-stale distinguishable: a day whose prices failed to fetch has no
    trustworthy snapshot and shows as such until the price re-run succeeds

Out of scope (record as Known limitations):
- **Intraday prices** — only one closing/reference price per listing per day is stored
- **Automatic backfill triggered by entering a back-dated trade** — backfill is on demand; a
  back-dated fact flags snapshots stale but does not itself fetch missing past prices

## GST-inclusive brokerage entry and statement-total cross-check (2026-06-07)

Trade entry (both Buys and Sells) is made faster and safer to reconcile against a broker
statement/contract note.

- GST-inclusive brokerage
  - A trade carries a boolean flag, `brokerage_includes_gst` (default false), recording that the
    brokerage amount was entered GST-inclusive
  - When the flag is set, the entered brokerage amount is GST-inclusive and the server splits it
    at write time: `gst_on_brokerage` = amount × 1/11 rounded to the cent (half away from zero,
    matching statements), `brokerage` = amount − GST. The stored ex-GST brokerage and GST still
    sum exactly to the amount paid, so the existing cost-base arithmetic
    (`brokerage + gst_on_brokerage`) is unchanged everywhere. Any `gst_on_brokerage` supplied in
    the input is ignored when the flag is set
  - When the flag is not set, behaviour is unchanged: brokerage is entered ex-GST and
    `gst_on_brokerage` is entered manually (zero for no-GST trades, e.g. foreign brokers)
  - The flag is persisted so a trade round-trips: reading the trade back shows the split values
    plus the flag, and the web form re-presents a single GST-inclusive brokerage amount
    (brokerage + GST) with the box ticked
- Statement-total cross-check
  - A trade can optionally record the statement's transaction total (`statement_total`, decimal,
    nullable, in the brokerage currency)
  - When provided, it is validated at write time inside the write transaction: it must equal
    quantity × price + brokerage + GST for a Buy, or quantity × price − brokerage − GST for a
    Sell (net amount payable/receivable). Comparison is numeric (1234.50 matches 1234.5).
    A mismatch is rejected with 422 — this catches data-entry errors against the contract note
  - A total may only be supplied when the trade currency equals the brokerage currency (the
    normal statement case); supplying one on a mixed-currency trade is rejected with 422 — no
    FX conversion is invented for it
  - The stored total is validation/cross-reference only — no report or calculation uses it
    (informational column)
  - Trades created by linked operations (DRP reinvestment, rights exercise, buy-back, scrip,
    demerger, transfer) are unaffected: flag false, no total
- Web UI
  - The Buy and Sell forms gain the GST-included checkbox (when ticked the GST field is hidden
    and the brokerage field is labelled as GST-inclusive) and the optional statement-total field
  - The trades and Sells lists show the statement total so it can be eyeballed against statements

## Simpler income entry, per-share cross-check, and combined income + DRP form (2026-06-07)

Entering a dividend/distribution from a registry statement (e.g. a Computershare payment advice
or DRP advice) is made faster: the common cases need only the figures printed on the statement,
the rarer tax components are tucked behind an advanced toggle, and a distribution that was
reinvested under a DRP is entered in the same form as the income itself.

- Simple-first income form (web UI only; the income API and its component model are unchanged)
  - The income form opens in a simple mode showing: listing, date paid, the payment amount, and a
    franking selector — **Fully franked (30%)** / **Unfranked** / **Trust distribution**
  - Fully franked: the amount is submitted as `franked_amount` and the franking credit is
    auto-computed as amount × 30/70 rounded to the cent (PLS example: $2,757.30 → $1,181.70).
    Unfranked: the amount is `unfranked_amount`. Trust distribution: the amount is
    `unfranked_amount` with `trust_income` set (component breakdown arrives later via the AMMA
    statement for AMIT funds)
  - An "advanced" toggle reveals the full existing field set (ex date, the individual
    franked/unfranked/credit amounts, foreign income and tax, TFN withholding, LIC deduction,
    conduit foreign income, currency, holding account). Partially franked dividends and
    non-30% corporate tax rates (e.g. 25% base-rate entities) are entered via advanced mode —
    the selector covers only the common statement cases
  - Editing an existing row opens in advanced mode whenever any advanced-only field holds a
    non-default value (so nothing is hidden); otherwise it opens simple with the selector
    reflecting the stored shape
- Per-share cross-check (server-side, like the trades statement-total check)
  - Income gains two optional columns: `amount_per_security` and `securities_held` (TEXT
    decimals, nullable). They must be supplied together — one without the other is rejected
    with 422
  - When supplied, the write is validated inside the write transaction: amount_per_security ×
    securities_held, rounded to the cent (half away from zero), must equal the gross cash
    components `franked_amount + unfranked_amount + foreign_source_income` (franking credits are
    notional and TFN withholding is deducted from, not part of, the gross). Mismatch → 422.
    Both statements reconcile: 0.14 × 19,695 = $2,757.30 (PLS); 0.89891492 × 866 = $778.46
    (VDHG, cent-rounded)
  - The stored values are validation/cross-reference only — no report uses them (informational
    columns, mirroring `trades.statement_total`)
  - The web form exposes both fields in simple mode (they are printed prominently on statements)
    and shows the computed product as a hint against the entered amount
- Combined income + DRP entry (web UI; chains the two existing API calls)
  - When the income being entered has no reinvestment yet, the form offers a "Reinvested under
    DRP" tick; ticking it reveals the reinvestment fields of the existing Reinvest action
    (reinvestment price, optional trade date defaulting to the pay date, FX rate)
  - Submit saves the income (`PUT /income/{id}`) and then posts the existing
    `POST /income/{id}/reinvest`. If the reinvest step fails (e.g. not DRP-enrolled), the saved
    income row stands, the error is shown, and the row's existing Reinvest action remains the
    fallback — no new endpoint, no change to reinvestment semantics (whole units + residual are
    still computed server-side; the VDHG advice reconciles: $778.46 at $52.0017 → 14 units =
    $728.02, residual $50.44 carried forward)
- Docs: the new columns update `docs/SCHEMA.md`; the new 422 validations update `docs/API.md`;
  the simpler entry flow is a user-visible feature for the README

Out of scope (record as Known limitations):
- **Auto-computing franking credits at non-30% corporate tax rates** — the selector assumes the
  30% rate printed on typical statements; 25% base-rate-entity dividends and partially franked
  payments are entered via the advanced fields
- **Statement parsing/import** — figures are still keyed in manually from the statement
