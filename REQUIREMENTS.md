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

Everything specified through 2026-06-08 (worthless / delisted shares, deductible investment
expenses, ESS income, the crypto network fee) is now also implemented and documented in
`README.md` / `DONE.md`.

The sections dated 2026-06-10 at the end come from the **2026-06-10 business-analysis gap review**
(an individual-investor gap analysis against the ATO guidance mirrored in `docs/ato/`). They are
newly specified and folded into `TODO.md` as open work.

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

## No raw foreign keys shown in the web UI (2026-06-07)

Everywhere the web UI presents a reference to another entity, it shows a useful name, never a bare
foreign-key id. List tables were largely fixed already; this requirement extends the rule to every
surface and makes it an audited invariant rather than a per-screen fix.

- Audit every web surface for raw ids standing in for an entity: entity list tables, report
  tables, form `<select>` option labels, read-only/derived fields on forms, post-record action
  dialogues (reinvest, exercise, participate, scrip, demerge, transfer) and their parcel/option
  labels, confirmation prompts, and toast messages (e.g. "Reinvested into trade #12")
- Naming convention per referenced entity:
  - Listing → ticker (plus name where space allows)
  - Holding account → account name
  - Trade/parcel → a human description (side, quantity, ticker, date) — a parcel or trade id
    alone is meaningless to the user
  - Other entities (corporate actions, transfers, …) → their most recognisable label
- An id may appear *alongside* the name where it helps cross-referencing, but never instead of it
- Toasts that today report only a created row's id should name what was created (e.g. the
  reinvestment Buy's ticker, quantity, and date), with the id as secondary detail
- Verified the existing way: UI tests assert the rendering path in the served bundle; where a
  lookup map is built for a table/select, a test covers it

## Currency rounding in lists and reports; precision where it matters (2026-06-07)

Displayed money is rounded to the currency's minor unit; stored and computed money keeps full
precision. Today some screens show raw decimal strings (e.g. a franking credit of
`1181.7000000000`), and the rule for which figures legitimately carry sub-cent precision is
implicit.

- Display rounding (presentation only)
  - Every monetary amount shown in a list table or report table is formatted to 2 decimal places
    (the minor unit of AUD and the other supported currencies), with thousands grouping if cheap
    to add
  - Aggregation always happens at full precision; rounding is applied only at the formatting
    step, never to intermediate values (consistent with the existing Decimal-only rule)
  - The JSON API keeps returning full-precision strings — rounding lives in the web layer's
    formatters, so API consumers and cross-checks are unaffected
- Sub-cent precision is deliberate, not accidental, for per-unit rates:
  - Trade price per share/unit, income `amount_per_security` (e.g. VDHG's 0.89891492), DRP
    reinvestment price, FX rates, and crypto quantities/prices keep their entered precision in
    storage and display — these are not "money shown to the user" but rates, and rounding them
    breaks reconciliation against statements
  - Derived per-unit figures a report may show (e.g. average cost per share) display with enough
    precision to be useful (at least 4 decimal places), not cent-rounded
- Quantities are unaffected: they display at their natural precision (whole shares stay whole,
  token fractions keep their places)
- The distinction (amounts round, rates don't) is documented once (README or docs/API.md) so new
  screens follow it

## Useful error messages in the web UI (2026-06-07)

A rejected write must tell the user *why*. Today most handlers return a bare status code, so the
UI toast reads just "HTTP 422" — e.g. reinvesting a distribution for an account not enrolled in
the DRP gives no hint that enrolment is the problem. The toast plumbing already displays the
response body when present (`api()` appends it), so the work is server-side.

- Every 4xx response that a user action can trigger carries a short, human-readable, plain-text
  body explaining the rejection — the existing per-share cross-check 422 detail is the model
- Audit every handler returning a bare `StatusCode` for 422/409/404-with-a-cause and attach the
  reason: which invariant failed, with the actual values involved (e.g. "allocations sum to 95
  but the sell quantity is 100", "account 'Broker' is not DRP-enrolled for VDHG —
  enrol it on the holding-accounts screen first")
- Messages name entities by name/ticker, not by foreign-key id (per the no-raw-ids requirement)
- 404s from a stale UI (row deleted elsewhere) say what wasn't found; plain "not found" body is
  acceptable where there is nothing more to say
- 5xx responses stay generic — internal details belong in the server log, not the toast
- The error-body convention (plain text, when present, what it contains) is documented in
  `docs/API.md`'s Response codes section; each endpoint's 422 causes are listed where they are
  non-obvious
- Tests assert the body text (or a distinctive fragment of it) for each validated rejection, not
  just the status code

## Live current prices from the price source, with an as-of time (2026-06-08)

Now that there is a real price source (Yahoo, via the `PriceFetcher` trait), any report or screen
that needs *current* market prices should fetch them from the source rather than requiring the
user to type them into the request. Today the price-dependent reports (portfolio/valuation,
unrealised gains, performance) only value holdings if the caller supplies a `prices` map in the
request body — otherwise `market_value`/`current_price` come back empty. That should no longer be
the default experience.

- When a price-dependent report or screen is requested without explicitly supplied prices, fetch
  the latest available price for each held listing from the price source (the existing
  `PriceFetcher` / `YahooFetcher`), instead of returning empty valuations
  - Prices come from the source in the listing's quote currency and are converted to AUD per the
    existing FX rules before valuation — never mix currencies (per `CLAUDE.md`)
  - Exchange-less (Crypto) listings are quoted continuously; their latest quote is fetched the
    same way
- Every fetched price carries, and the report/screen surfaces, the **as-of time** the price is
  for (the provider's quote timestamp, not the time we fetched) — so the user can see how fresh
  the valuation is and that different listings may be as-of different moments (e.g. a closed
  market vs an open one). The web UI shows this near the valuation (per-row and/or a summary "as
  at …" line)
- Explicitly supplied prices still override the fetched ones (the existing `prices` request body
  keeps working unchanged) — used for what-if valuations and to keep the ATO acceptance tests
  deterministic
- A failed/unavailable live fetch for a listing is surfaced, not silently zeroed: that holding
  shows no current value with a reason, while the rest of the report still values (consistent with
  the "never silent zero" rule)
- Stored closing-price history and daily snapshots are unchanged — this is about *current/live*
  valuation on demand; the snapshot series remains the historical record
- `docs/API.md` documents the new behaviour (default = live-fetched, the as-of time field, the
  override), and the Features list notes live valuation. Tests cover: live fetch fills valuations,
  the as-of time is returned, an explicit override wins, and a per-listing fetch failure degrades
  gracefully

## Human-friendly headings and field labels throughout the web UI (2026-06-08)

Every heading, table column header, and form field label shown to the user must read as a
human-friendly name, not the raw database/JSON field name — `amount_per_security` shows as
"Amount per security", `exchange_mic` as "Exchange", `fx_rate` as "FX rate", `holding_account_id`
as "Account", and so on. This is the labelling counterpart to the no-raw-foreign-keys requirement:
that one fixed raw *id values*; this one fixes raw *field names* in the chrome around them.

- All column headers in the shared `filterableTable`, all form input labels, all report table
  headers, and all section/screen headings use human-readable names
- The mapping is config-driven and lives with the existing per-entity/report config in `app.js`
  (the `ENTITIES`/`REPORTS`/`ACTIONS` descriptors) — labels are declared once per field, not
  hand-written per view; generic list/form/table code reads them. A field with no explicit label
  falls back to a humanised form of its name (e.g. snake_case → "Title case") so nothing ever
  renders a raw identifier by default
- Units/qualifiers belong in the label where they aid reading (e.g. "Price (AUD)", "Quantity
  (units)") without changing the underlying field
- Acronyms keep their canonical casing (AUD, FX, MIC, DRP, CGT, AMIT, GST, LIC, FITO), not
  title-cased into "Aud"/"Drp"
- Tested by asserting the served bundle renders the friendly labels and no raw field name leaks
  into a heading/label (consistent with the existing "UI items tested against the served bundle"
  approach)

## Client-side pagination for large tables (2026-06-08)

Tables that can grow large (entity lists, the Sells list, report tables — trades, closing-price
history, snapshots, parcels) should paginate so a long result set is not dumped as one enormous
table. At this stage pagination is done client-side: the existing JSON endpoints keep returning
the full array and the web layer pages through it.

- The shared `filterableTable` gains pagination: a page size and page navigation (next/prev and/or
  page numbers), so only one page of rows is in the DOM at a time
- Pagination composes correctly with the existing filtering and sorting: filtering/sorting apply
  to the **whole** result set, then the result is paged — never page-then-filter. Changing a
  filter resets to the first page; the row/result count reflects the filtered total, and the
  control shows e.g. "showing 1–50 of 320"
- Applied uniformly through `filterableTable` (per the "route new tables through it" rule), so
  every table benefits without bespoke per-table paging
- The default page size is 50 rows; small tables (50 rows or fewer) show no pagination control
- Server-side pagination of the JSON API is explicitly out of scope for now (record as a Known
  limitation): the full set is still fetched, so this addresses rendering/usability, not payload
  size. If result sets later outgrow a client-side fetch, server paging becomes a follow-up
- Tested by asserting the paging controls and behaviour are present in the served bundle and that
  filtering still reflects the full set (per the served-bundle UI test approach)

## Worthless / delisted shares — capital loss on a company in liquidation (CGT events G3 and C2) (2026-06-08)

A company an individual holds can fail: it goes into liquidation or administration, or is
deregistered, often with no disposal a broker can settle (the shares are suspended/delisted and
worthless). The ATO lets the holder recognise the capital loss without an ordinary sale, but today
the dead parcel sits as an open holding forever and the real, deductible capital loss never reaches
the realised-gains or net-capital-gain reports. This requirement lets that loss be recorded. It is
a **capital loss** (never income, never discounted), so once recognised it flows through the
existing loss-netting order and indefinite carry-forward unchanged.

- The mechanism — two ATO events, modelled as a new corporate action against the listing (the
  natural extension point: the `corporate_actions` `action_type` enum):
  - **CGT event G3** (s104-145; TD 2000/52): the liquidator or administrator declares **in
    writing** that they have reasonable grounds to believe there is no likelihood shareholders
    will receive any further distribution. The holder may **choose** (it is opt-in) to make a
    capital loss equal to the **reduced cost base** of the shares at the declaration date. The
    shares are not cancelled — but on making the choice their cost base and reduced cost base are
    reset to nil, so a later actual cancellation cannot double-count the loss (and any later
    distribution becomes a capital gain)
  - **CGT event C2** (s104-25; TD 2000/7): the shares are actually cancelled/redeemed or the
    company is deregistered — an ordinary disposal at the capital proceeds (usually nil), so the
    capital loss is the reduced cost base if not already crystallised under G3
  - The action records the listing, the event date, and which event it is (a `G3Declaration` vs a
    `C2Cancellation` subtype, or one type with an event-kind field). Recording it is itself the
    G3 opt-in choice
- The operation closes every open parcel of the listing held at the event date through a
  provenance-marked Sell at **nil proceeds** (reusing the shared sell core and the
  closing-Sell/group mechanics already built for scrip-for-scrip and demergers) — but, unlike a
  rollover, the loss **is** recognised: each parcel produces a capital loss equal to its remaining
  reduced cost base (cost base after any AMIT/return-of-capital reductions), reaching the
  realised-gains report (as a `capital_loss`) and the net-capital-gain report's loss pool. Because
  the project captures only cost-base elements 1–2, the reduced cost base equals the cost base (the
  existing Known limitation), so the loss is the remaining cost base
- A capital loss is never discounted, so there is no discount-eligibility/12-month concern; the
  acquisition date is irrelevant to the loss amount
- Write-time integrity, mirroring the existing group operations: the closing Sells are immutable
  individually (PUT/DELETE on a group trade → 422), deleting the operation restores the pre-event
  holding, the action is frozen while referenced, and the operation rejects (422) a wrong action
  type, an already-recognised action, or nothing held at the event date
- ATO documentation: mirror the G3/C2 worthless-shares guidance into `docs/ato/` (the
  "Investments in a company in liquidation or administration" page + TD 2000/52 and TD 2000/7;
  source URL + retrieval date, indexed in `OVERVIEW.md`); add an `ato_examples.rs` acceptance test
  for any worked example the feature makes representable
- Web UI: the action and its operation render through the existing config-driven Corporate Actions
  view + `ACTIONS` descriptor — no bespoke screen
- Docs: a new corporate-action type updates `docs/SCHEMA.md` (columns + CHECKs) and `docs/API.md`
  (the action, the operation endpoint, the 201/404/422 cases); the user-visible capital-loss
  recognition updates the README Features list

Out of scope (record as Known limitations):
- **The choice *not* to crystallise a G3 loss** — recording the action is the opt-in; a holder who
  does not record it simply keeps the parcel open (no modelling needed)
- **Declarations limited to a class of shares** — handled at the listing granularity; a per-class
  partial declaration is not separately modelled
- **Pre-CGT original shares** — consistent with the other corporate actions
- **An unexpected later distribution after a G3 choice** (reduced cost base already reset to nil →
  the whole distribution is a capital gain) — entered manually as a return-of-capital/G1 if it
  arises

## Deductible investment expenses (2026-06-08)

The tax summary today reports **gross** assessable investment income with no deductions side, so it
overstates the net assessable position. An individual share investor can deduct the expenses
incurred in earning that income — most materially **interest on money borrowed to buy income-
producing shares** (margin/investment loans), plus ongoing management and adviser fees, account-
keeping fees on investment accounts, and specialist investment subscriptions/data services. This
requirement adds a place to record those deductions and nets them in the tax summary.

- A new entity (`investment_expenses`): id, date incurred, an expense-type enum
  (`LoanInterest`, `ManagementFee`, `AdviceFee`, `AccountKeepingFee`, `Subscription`, `Other`),
  amount (Decimal), `currency` (AUD default; non-AUD converted to AUD via the existing ATO-rate
  rules at the month incurred, like income), a free-text description, and optional attribution to a
  `listing_id` and/or `holding_account_id` (an expense may be portfolio-wide — both null)
- Apportionment for part-private use (e.g. internet): the **deductible amount** is what is stored
  and totalled (post-apportionment, the figure that goes on the return); the gross amount and the
  deductible percentage may be stored alongside for provenance, but the tool does not itself rule
  on the correct apportionment — that is the user's determination (consistent with how it treats
  the FITO income test)
- The tax summary gains a deductions side per Australian financial year: a total by expense type
  and overall, and a **net assessable investment income** figure (existing gross income totals
  minus the deductions), without removing the gross figures. AUD throughout, per the existing
  never-mix-currencies rule. This is distinct from, and additional to, the existing LIC capital
  gain deduction (a different statutory mechanism, kept as-is)
- The tax-return CSV export carries the new deduction columns and the net figure
- ATO documentation: mirror the "Interest, dividend and other investment income deductions" and
  "Dividend income deductions" guidance into `docs/ato/` (source URL + retrieval date, indexed in
  `OVERVIEW.md`)
- Web UI: a CRUD screen via the existing `ENTITIES` config; the new tax-summary columns surface
  automatically (report columns derive from the response keys)
- Docs: the new table updates `docs/SCHEMA.md`; the new tax-summary fields and any 422 validations
  update `docs/API.md`; the deductions feature updates the README Features list

Out of scope (record as Known limitations):
- **Non-deductible costs** are not the tool's job to police: acquisition brokerage/stamp duty stay
  as cost-base element 2 (already handled), and the user is responsible for not entering exempt-
  income or capital expenses as deductions
- **Prepaid-interest timing rules** (the 12-month prepayment rule, capital-protected borrowing
  interest apportionment, split-loan arrangements) — the expense is recorded in the year the user
  attributes it to; these timing/character apportionments are the user's determination
- **The deductibility determination itself** — the tool records and totals what the user enters; it
  does not rule on whether a given expense is deductible

## Employee share scheme (ESS) income (2026-06-08)

The project already models the **CGT side** of an employee share scheme correctly: an RSU vest is
entered as an ordinary Buy at the **market value at the deferred taxing point** with the **vest
date** as the acquisition date — which is exactly the ATO's cost-base reset (at the deferred taxing
point the ESS interest is taken to be re-acquired at market value, and the 50% discount clock
restarts from that date). What is missing is the **income side**: the assessable ESS discount must
be declared in the year of the taxing point and surfaced in the tax summary. This requirement adds
that, and links it to the cost-base-reset Buy so the income and CGT sides are entered once and stay
consistent. It **supersedes the current "ESS income reporting is out of scope" Known limitation.**

- A new ESS-income record capturing the figures an employer's **ESS statement** prints (the
  individual tax return's Item 12 labels), per statement, attributed to a `listing_id` and
  `holding_account_id`:
  - `taxed_upfront_eligible` — discount from taxed-upfront schemes **eligible** for the $1,000
    reduction (label D)
  - `taxed_upfront_not_eligible` — discount from taxed-upfront schemes **not** eligible (label E)
  - `deferral_discount` — discount from tax-deferral schemes (label F; the RSU case)
  - `pre_2009_cessation_discount` — discount on pre-1 July 2009 interests whose cessation time fell
    in the year (label G)
  - `foreign_source_discount` — the foreign-source portion of the above (label B)
  - `tfn_withholding` — TFN amounts withheld from ESS discounts (label C), where no TFN/ABN was
    given to the employer
  - the taxing-point date and the market value at the taxing point (drives both the assessable
    discount and the linked Buy's cost base)
- The **$1,000 reduction** (taxed-upfront eligible schemes): reduce the assessable discount by up
  to the lesser of $1,000 and `taxed_upfront_eligible`, and surface the reduction applied — but the
  full eligibility test needs the taxpayer's **adjusted taxable income ≤ $180,000**, which is
  outside this system's data. So apply the up-to-$1,000 de-minimis and flag the income-test caveat
  as the user's responsibility, exactly mirroring the FITO $1,000 cap pattern (a
  `taxed_upfront_reduction` field + an informational caveat in the row)
- The tax summary gains an **assessable ESS discount** total per Australian financial year (total
  of the labels, net of the applied reduction), reported separately from dividend/trust income, in
  AUD (foreign-source discounts converted via the ATO rate), with the TFN-withholding total carried
  alongside the existing TFN line. The CSV export carries the new fields
- An **ESS vesting operation** ties the two sides together atomically (like the buy-back/scrip
  participations): from one entry it records the ESS-income discount components **and** creates the
  cost-base-reset Buy parcel (quantity vested, price = market value at the taxing point, zero
  brokerage, acquisition/settlement date = the taxing point), linked by provenance. Editing/deleting
  is symmetric (deleting the ESS record removes its linked vest Buy unless that parcel is already
  drawn on by a Sell/allocation, per the existing group-integrity rules)
- ATO documentation: mirror the ESS guidance into `docs/ato/` (the "tax-deferred schemes",
  "taxed-upfront $1,000 reduction", "ESS and capital gains tax", and Item 12 pages; source URL +
  retrieval date, indexed in `OVERVIEW.md`); add an `ato_examples.rs` acceptance test for any
  worked example the feature makes representable
- Web UI: a CRUD screen + the vesting operation via the existing `ENTITIES`/`ACTIONS` config; the
  new tax-summary columns surface automatically
- Docs: the new table updates `docs/SCHEMA.md`; the new endpoint/operation, fields, and 422 cases
  update `docs/API.md`; remove the superseded ESS-income Known limitation and add the ESS-income
  feature to the README Features list

Out of scope (record as Known limitations):
- **Determining the deferred taxing point** — the user enters the taxing-point date and market
  value from their ESS statement; the tool does not compute the earliest-of test (no real risk of
  forfeiture and no disposal restriction, or 15 years). The **30-day rule** (a sale within 30 days
  of the taxing point moves the taxing point to the sale date) is likewise reflected by the user
  entering the correct taxing-point date/value
- **The $180,000 adjusted-taxable-income test** for the $1,000 reduction — needs the taxpayer's
  whole income position (as with FITO); the de-minimis is applied and the caveat surfaced
- **Unvested grants and forfeiture before the taxing point** — there is no ESS interest to value
  yet, so nothing is tracked until vesting (consistent with the prior limitation)
- **Start-up concession schemes** (no upfront discount is assessable; the interest is taxed only
  under CGT on disposal) and **the employer's ESS annual report** lodgement (an employer
  obligation, not the individual's) are not modelled

## Crypto wallet-to-wallet transfer with a network fee (2026-06-08)

Moving a crypto asset between two wallets you own is already modelled as a holding-account transfer
(not a CGT event). What's missing is the **on-chain network fee**: an on-chain transfer reduces the
holding to pay a fee in the crypto itself, and per the ATO ("Crypto asset investments and tax",
QC 69952, mirrored in `docs/ato/crypto-cgt.md`): *"Transferring crypto assets from one digital
wallet to another digital wallet is not considered as a disposal as long as you maintain ownership
of it. If your crypto holding reduces during a transfer to cover a network fee, the transaction fee
is a disposal and has capital gain consequences."* So the move stays a non-CGT event, but the crypto
burned to cover the fee **is a disposal** that must surface in the gains reports.

- The holding-account transfer operation (`PUT /transfers/:id`) gains an **optional network fee**:
  - `fee_allocations` — the source parcels (and units) consumed to pay the fee, in the same shape as
    the moved `allocations`; empty/absent means no fee. They must belong to the transfer's listing
    and the source account, and the moved units plus the fee units are validated together against
    each parcel's capacity
  - `fee_market_price` — the fee crypto's per-unit market value at the transfer date, in the
    listing's currency (AUD for an AUD-priced crypto; an optional `fee_fx_rate`, default 1, converts
    a non-AUD listing's price to AUD). Required when `fee_allocations` is non-empty; a fee without a
    positive market value is rejected (422) — the disposal needs its capital proceeds
- The fee is recorded as an **ordinary disposal Sell** in the source account at that market value:
  it carries **no `transfer_id`** (so the realised-gains, net-capital-gain, and performance reports
  count it, with the 12-month discount where the fee units were held ≥ 12 months), but is linked to
  the transfer so the two are created and deleted atomically and the fee Sell is individually
  immutable (rejected by `PUT /sells`, `PUT`/`DELETE /trades`, `DELETE /sells`; undo by deleting the
  transfer). Deleting the transfer removes the fee disposal and restores the whole source parcel
- ATO documentation: the wallet-transfer/network-fee guidance is mirrored into
  `docs/ato/crypto-cgt.md` (source URL + retrieval date, indexed in `OVERVIEW.md`)
- Web UI: the transfer form gains an optional fee-parcel allocation editor + the per-unit market
  value field, via the shared `allocationEditor`
- Docs: the new `transfers.fee_sale_trade_id` column updates `docs/SCHEMA.md` (incl. Relationships);
  the changed `PUT /transfers/:id` request/response and 422 cases update `docs/API.md`; the README
  Transfers feature notes the crypto network fee

Out of scope (already recorded as Known limitations, unchanged): crypto-to-crypto swaps, staking
rewards/airdrops, chain splits/forks, wrapping, the personal-use-asset exemption, and Div 775
foreign-currency balances. A transfer fee charged in **fiat** by an exchange is not a crypto
disposal — it is a (non-deductible) transaction cost, not modelled here.

## Trust distribution income year — present entitlement (2026-06-10)

The tax summary attributes `income` rows to a financial year by `date_paid` (July ⇒ next FY).
That is correct for dividends (assessable when paid or credited) but **wrong for trust
distributions**: per ATO QC 23087 (mirrored in `docs/ato/trust-income-timing.md`) a beneficiary
is assessed in the year they are **presently entitled**, regardless of when the cash is paid —
and managed funds routinely pay the June distribution in mid-July. Today a `trust_income` row
paid 15 July 2026 for the June 2026 period lands in FY 2027; it belongs in FY 2026. AMMA
statements are unaffected (attributed by `tax_year_end_date`).

- `income` gains an optional `entitlement_date` (TEXT date, nullable): the date the beneficiary
  became presently entitled (in practice the distribution period's end, printed on the statement)
  - Only meaningful on trust distributions: supplying it on a non-trust row (`trust_income`
    false) is rejected with 422 — a dividend is always assessed by payment
  - When present on a trust row, the tax summary (and its CSV export) attributes **every**
    component of that row by `entitlement_date` instead of `date_paid`; absent, behaviour is
    unchanged (`date_paid`), so existing rows are unaffected
  - The franking 45-day at-risk test keeps anchoring on `ex_date`/`date_paid` (the at-risk window
    is about holding the shares, not the assessment year); the A$5,000 threshold year follows the
    row's assessment year
- Web UI: the income form's **Trust distribution** selection reveals the entitlement-date field
  (defaulting to the pay date); the advanced field set includes it
- Docs: `docs/SCHEMA.md` (new column), `docs/API.md` (the 422 and the attribution rule), README
  tax-summary feature text; an `ato_examples.rs`-style test asserts a July-paid June trust
  distribution reaches the earlier FY

## Non-AMIT trust tax-deferred amounts — CGT event E4 cross-check (2026-06-10)

For a **non-AMIT** unit trust, a tax-deferred amount on the annual statement is a CGT event E4
cost-base reduction (`docs/ato/cgt-non-assessable-payments.md`). The model handles it via a
`ReturnOfCapital` corporate action, but nothing connects the income entry to that obligation — a
user who faithfully keys the statement will silently overstate cost base. (For an AMIT the
per-unit `cost_base_adjustment` is the sole driver and tax-deferred is informational —
`docs/ato/amit-cost-base-adjustments.md` — that stays unchanged.)

- `income` gains an optional informational `tax_deferred_amount` (TEXT decimal, nullable, ≥ 0;
  only valid on `trust_income` rows, 422 otherwise) — recorded from the statement, used by no
  calculation (the E4 reduction itself remains the `ReturnOfCapital` action)
- A new non-blocking report (pattern: settlement-holiday coverage) flags every trust income row
  with a non-zero `tax_deferred_amount` whose listing has **no** `ReturnOfCapital` action dated
  within that row's financial year — "tax-deferred amount recorded but no cost-base reduction
  entered". Entering the matching action clears the flag; rows whose listing has a same-FY action
  are omitted
- Web UI: the field joins the advanced income fields; the report renders via the standard
  `REPORTS` config
- Docs: `docs/SCHEMA.md`, `docs/API.md` (new report + 422), README

## Inherited share parcels (2026-06-10)

Shares inherited from a deceased estate enter the portfolio with a cost base that is not a market
purchase (ATO QC 66053, mirrored in `docs/ato/inherited-assets-cost-base.md`): the transfer from
the estate is no CGT event; the beneficiary's first-element cost base is the **deceased's cost
base at death** (asset acquired by the deceased on/after 20 Sep 1985) or the **market value at
death** (pre-CGT asset), plus any LPR expenditure. Today the only entry path is a synthetic Buy
with hand-computed figures and no provenance.

- A way to enter an inherited parcel recording: listing, holding account, units, **date of
  death**, the cost base per the rule above (entered as a figure, with which rule applied), the
  **deceased's acquisition date** where the asset was post-CGT in their hands, and any LPR
  expenditure (added to cost base, dated when the LPR incurred it)
- The 12-month discount clock follows s 115-30: confirm the rule from the ATO source during
  implementation and mirror the page into `docs/ato/` — the intended outcome is that a post-CGT
  inherited parcel's discount period runs from the **deceased's** acquisition and a pre-CGT
  parcel's from the **date of death**
- The parcel flows through every report and write-time capacity check like any Buy; provenance is
  visible (it is an inherited parcel, not a market trade)
- Web UI via the existing config-driven entity/action patterns; docs per the standard sync rule;
  an `ato_examples.rs` acceptance test for any worked example the feature makes representable

Out of scope (record as Known limitations):
- **The estate/LPR side** (the executor's own return, assets sold by the executor) — only parcels
  that pass to the beneficiary are modelled
- **Market valuation at death** — the user supplies the figure (as elsewhere)

## Renounceable rights — selling, lapsing, and retail premiums (2026-06-10)

The `RightsIssue` action models exercise only. ASX retail entitlement offers constantly produce
the other outcomes, already documented in the mirrored guidance (`docs/ato/rights-issues.md`):
**selling** free rights is a CGT event with the rights taking the **original parcel's acquisition
date** and a nil cost base (so the proceeds are essentially all gain, discount-eligible off the
original holding date); **lapse** of free rights is a non-event (nil cost base, nil proceeds).

- A **sell-rights** operation against a `RightsIssue`: units of rights sold (capped, together
  with exercises, at the entitlement), proceeds per right, sale date; produces a provenance-marked
  disposal whose acquisition date for the discount is the original parcel's, reaching the realised
  and net-capital-gain reports. Free rights have nil cost base; rights paid for carry that cost
- **Lapse** needs no operation for free rights (no gain, no loss); a paid-for right that lapses
  is a capital loss of its cost — supported by the same operation at nil proceeds
- **Retail premiums** (the payment a non-participating holder receives when the shortfall is
  placed): the ATO treats these as assessable — fetch and mirror the ATO's retail-premiums
  guidance before implementing, and resolve the exact character (the needs-clarification step);
  model only after the doc is mirrored
- Web UI via the existing `ACTIONS` config; docs per the standard sync rule; `ato_examples.rs`
  tests for the worked examples this makes representable (Example 39's sold-rights case)

## Takeovers with a cash component — partial scrip-for-scrip rollover (2026-06-10)

`ScripForScrip` models only the all-scrip full rollover, yet most real takeovers pay cash or
mixed consideration. The mirrored guidance (`docs/ato/takeovers-and-scrip-for-scrip.md`, Example
27 — Gunther) covers the partial case: rollover applies only to the scrip portion; the cost base
is apportioned between cash and scrip by the **market values of the consideration**; the cash
portion is an ordinary disposal whose gain is assessed now (discount per the original holding
period), and the replacement parcels carry the scrip-apportioned cost base and original
acquisition dates.

- Extend the `ScripForScrip` action with an optional per-unit cash component alongside the scrip
  ratio; the exchange operation then splits each consumed parcel's remaining reduced cost base by
  the consideration's market values, recognises the cash-side gain/loss in the realised and
  net-capital-gain reports, and creates the replacement parcels exactly as today for the scrip
  side
- A pure-cash takeover stays an ordinary Sell (unchanged); the all-scrip case is unchanged
- `ato_examples.rs`: Example 27 becomes representable — add the acceptance test
- Web UI via the existing action config; docs per the standard sync rule

Out of scope (record as Known limitations, unchanged): takeovers without rollover eligibility
(enter as ordinary Sells) and multiple replacement share classes (Example 28)

## CGT decision support — parcel-selection optimiser and pre-sale what-if (2026-06-10)

Everything in the system is retrospective, but parcel selection is the taxpayer's **choice**
(`docs/ato/cgt-keeping-records-shares.md`) — the largest legal CGT lever an individual has. The
tool records the choice; it should help make it. Both features are **read-only reports** over
data the open-parcels report already has; nothing is persisted.

- **Parcel-selection optimiser**: given a listing, holding account, unit quantity, sale date, and
  a price (live-fetched by default, per the live-valuation rules), return candidate allocation
  strategies — at least: minimise current-year assessable gain, maximise discount-eligible
  proportion, harvest losses first, FIFO as the baseline — each with its per-parcel allocation
  and the resulting gross gain / discountable split, so the user can pick allocations for the
  real Sell
- **Pre-sale what-if**: the net-capital-gain report accepts a hypothetical disposal (listing,
  units, proceeds, date, chosen allocations or a strategy from the optimiser) and returns the
  year's figures with and without it — a dry run, no rows written. The whole-of-income tax
  estimate stays out of scope (consistent with the FITO decision); this is the CGT-side delta
  only
- Web UI: a screen per report via the existing `REPORTS`/action config; docs per the standard
  sync rule

## Compliance alert reports — wash sales and franking at-risk foresight (2026-06-10)

Two non-blocking alert reports in the established pattern (MIC validation, settlement coverage):

- **Wash-sale flag**: list every loss-realising Sell followed (or preceded) by a Buy of the same
  listing within a configurable window (default 30 days) in any holding account — the fact
  pattern the ATO warns may attract Part IVA. Fetch and mirror the ATO's wash-sale guidance
  (TR 2008/1 / the current ATO page) into `docs/ato/` before implementing. Non-blocking: writes
  are never rejected; the report only surfaces the pattern with the dates and amounts
- **Franking at-risk foresight**: the 45-day rule is currently applied silently at tax-summary
  time. Add a report listing each dividend whose credits are denied (or would be denied by a
  contemplated sale — reusing the holding-period walk) with the failing window, so the user sees
  *why* credits disappear and can time disposals; surfaced in the UI near the Sell flow
- Docs per the standard sync rule; both reports render via the standard `REPORTS` config

## Tax-return label mapping on the CSV exports (2026-06-10)

The tax-summary and net-capital-gain exports carry the right figures but not the tax-return
labels, so every June the user re-derives the mapping. Map each exported column to its
myTax/paper-return label (e.g. net capital gain → 18A, total current-year gains → 18H, franked
dividends/credits → 11T/11U, trust income → 13U, foreign income/FITO → 20E/20O, deductions →
D7/D8, ESS → Item 12 D/E/F/G/A/C).

- Verify the current year's labels from the ATO instructions at implementation time and mirror
  the label reference into `docs/ato/` (labels shift year to year — the mirror records which
  year's form the mapping targets)
- The mapping appears in `docs/API.md` and on the export itself (e.g. a second header row or a
  label column) without changing the existing columns
- Existing user-responsibility caveats (`taxpayer_basis`, FITO, ESS income test) are unchanged

## Interest income (2026-06-10)

The deductions side of investment income is modelled (`investment_expenses`), but assessable
**interest income** (broker cash hub accounts, margin-loan offset interest) is not recordable —
`net_assessable_investment_income` is structurally understated. The `income` entity is
listing-keyed, so interest needs its own small entity.

- A new entity `interest_income`: id, date paid, amount (Decimal), currency (AUD default,
  converted per the existing ATO-rate rules at the month paid), TFN withholding, optional
  `holding_account_id`, free-text source description
- The tax summary gains an `interest_income` line per financial year, included in
  `gross_assessable_investment_income` (and so netted by the existing deductions); the TFN amount
  joins the existing withholding line; CSV export updated
- Standard entity module pattern, web UI `ENTITIES` entry, docs per the standard sync rule

## Operational hardening — restore, off-disk backups, localhost default (2026-06-10)

- **Backups**: the weekly backup writes beside the live database — same disk, same failure
  domain — and no restore procedure is documented or tested. Add an optional `--backup-dir`
  (default: beside the DB, as today) so backups can land on another volume; document the restore
  procedure in the README and prove it with a test (back up, mutate, restore, assert the
  pre-mutation state)
- **Bind address**: the server defaults to `0.0.0.0` with no authentication while holding a
  near-complete tax position. Change the default `--host` to `127.0.0.1`; `--host 0.0.0.0`
  remains available and the README note inverts accordingly (opt into network exposure rather
  than out of it)

## Known-limitation documentation — gifts, pre-CGT holdings, indexation (2026-06-10)

Three scope cuts that are currently silent; document each in the Known limitations list (no
modelling):

- **Gifts / off-market related-party transfers**: a gift is a disposal at **market value**
  (market-value substitution); enterable today as a manual Sell (gift out) or Buy (gift in) at
  market value — say so
- **Pre-CGT holdings**: a parcel acquired before 20 September 1985 is outside CGT entirely; the
  system would wrongly compute gains on it — state that pre-CGT parcels are not modelled
- **The indexation method**: for assets acquired before 21 September 1999 an individual may
  index the cost base (frozen at Sep 1999) instead of the 50% discount; the discount is almost
  always better for individuals and indexation is not modelled — state it

## AMIT cash distributions — assessable-income double-count (2026-06-12)

Found entering the full 2020–2026 statement archive: an AMIT fund's (VDHG, HNDQ) quarterly
distribution must be entered as an `income` row to drive the DRP reinvestment machinery, but every
income row's cash components land in the tax summary's `dividends_assessable` line — while the
fund's AMMA statement attributes the same income to 13U/13C/18/20E. With both entered (both are
needed: the cash rows for the DRP chain, the AMMA for the return figures) the year's assessable
dividends are inflated by the full trust cash (~$12k–22k per FY in the live data). For an AMIT the
**AMMA attribution is the only assessable record**; the cash advice is not a tax document.

- An AMIT distribution income row must be recordable as **cash-only**: it funds DRP reinvestment
  (and the per-share cross-check, ex-date enrolment check, residual chain) but contributes
  **nothing** to the tax summary's income lines (`dividends_assessable`,
  `gross_assessable_investment_income`, withholding, credits) — the listing's existing `amit` flag
  or an explicit income kind may drive the exclusion; write-time-validated, not report-special-cased
- A non-blocking cross-check report (pattern: E4 cross-check) flags every FY with AMIT cash rows
  whose listing has **no AMMA statement covering that year** — cash-only entry must not let a
  missing AMMA silently drop the income from the return — and conversely an AMMA year with no cash
  rows is fine (fund held without DRP)
- Non-AMIT trust and ordinary dividend rows are unchanged

## Fractional-share DRP reinvestment (2026-06-12)

`POST /income/:id/reinvest` spends the distribution on **whole shares** only — correct for ASX
registries (Computershare/MUFG; all 26 archive advices reproduce exactly) but wrong for US broker
DRPs: Morgan Stanley reinvests ICE dividends in fractional shares (0.500, 0.434, …) with no residual
carry. Those nine reinvestments had to be entered as plain Buys (price = net cash ÷ units), losing
the income→trade link and the DRP trade type.

- The reinvest operation accepts the statement's **fractional allotment**: an optional explicit
  `units` (the broker's stated figure, authoritative) with the price cross-checked against the
  reinvestable cash, or a per-enrolment whole/fractional mode — design open, but the registry
  statement's units must be representable exactly
- Whole-share behaviour (floor + residual carry) stays the default and is unchanged for existing
  enrolments

## ESS statement AUD override (2026-06-12)

Employer ESS statements convert the discount at the **release-date spot rate** (e.g. 1.4034 for
Feb-2022); the tax summary converts the recorded foreign-currency discount at the **RBA monthly
rate** — the live data differs by $65–214/yr (0.6–2.3%). The ATO-prefilled return carries the
employer statement's AUD figure, so the tool's figure disagrees with what the user must lodge.

- `ess_statements` gains optional statement-AUD amounts for the discount labels (at minimum the
  total assessable discount); when present the tax summary reports them verbatim instead of
  converting, when absent behaviour is unchanged (RBA monthly conversion)

## statement_total tolerance for cent-rounded contract notes (2026-06-12)

The `statement_total` cross-check compares exactly against `quantity × price ± brokerage + GST`,
but contract notes print the consideration **rounded to the cent**: 3 of 41 archive notes
(e.g. 1,302 × 37.585914 = 48,936.860028, note says 48,936.86) were rejected with `422` and had to
be entered without the cross-check.

- The cross-check passes when the supplied total equals the computed figure **rounded to the cent**
  (half away from zero, as statements round); an exact match keeps passing; mismatches beyond the
  sub-cent still reject with the computed figure in the body

## Known-limitation documentation — RSU dividend equivalents, foreign broker interest (2026-06-12)

Two scope cuts surfaced by the archive entry; document in Known limitations (no modelling):

- **Dividend equivalents on unvested RSUs**: employer plans accrue dividend equivalents on unvested
  grants (the archive's release confirmations show $77.88–$193.44); they are ordinary income when
  paid and are not modelled — enterable manually as income if paid out in cash
- **Foreign broker-cash interest classification**: interest income is reported at question 10
  (gross interest, 10L) regardless of source; foreign broker-cash/money-market income (the archive's
  USD Treasury Liquidity Fund dividends, ~$12–17/yr) strictly belongs at 20E assessable foreign
  source income — state the 10L simplification

## FX conversion granularity — spot-rate override for one-off capital transactions (2026-06-12)

Every non-AUD trade converts at the monthly RBA rate, and the per-trade `fx_rate` is only a
*fallback* — once the monthly rate is imported it takes precedence, so a deliberate spot rate can
never win. ATO guidance (`docs/ato/forex-average-rates.md`, QC 18020) permits average rates only
where they are a **reasonable approximation of the spot rates** at the statutory translation
times, and its Examples 5 and 7 state an average rate is **not appropriate for a one-off purchase
or sale of a large capital asset** — the spot rate at the transaction date should be used
(`docs/ato/forex-common-transactions.md` Lisa translates each leg at the day's rate). The monthly
simplification is fine as the default; it must stop being compulsory.

- A trade (Buy, DRP, Sell) can carry an explicit **spot-rate override** that wins over the
  imported monthly RBA rate everywhere the trade's amounts convert to AUD (cost base, proceeds,
  every report and the snapshot pipeline) — whether by promoting `fx_rate` from fallback to
  override via an explicit flag, or a separate column, is design-open; entry must be deliberate
  (the silent fallback semantics of `fx_rate` must not simply flip, which would change the meaning
  of existing rows)
- Absent an override, behaviour is unchanged: monthly RBA rate first, `fx_rate` fallback, loud
  failure when neither exists
- The FX conversion documentation states the rule honestly: monthly rates are the ATO-published
  convenience default, reasonable for recurring/small amounts; a one-off large foreign disposal
  should carry the transaction-date spot rate per QC 18020

## Settlement-window forex on foreign-currency trades — CGT events K10/K11 (2026-06-12)

Under the default forex 12-month rule (`docs/ato/forex-cgt-12-month-rule.md`, QC 17062), the
currency movement between a foreign-currency trade's contract date and its settlement payment is
not ignored: on an **acquisition** it adjusts the parcel's cost base (Art Ltd example); on a
**disposal** it is a separate **non-discountable capital gain (CGT event K10) or capital loss
(K11)** (Eleanor example). The system computes neither — trades convert at the trade-date monthly
rate, so a same-rate-month T+2 settlement nets to nil by construction, but a settlement crossing a
rate month (or entered with spot rates) silently drops the forex component.

- Decide the scope explicitly: either **model it** — for a non-AUD trade, compute the forex
  movement between the trade-date and settlement-date translations of the consideration; fold it
  into the parcel's cost base on a Buy/DRP, and surface it as a separate non-discountable
  K10 gain / K11 capital loss feeding the realised-gains and net-capital-gain reports on a Sell —
  or **resolve it out of scope** as a Known limitation stating that settlement-window forex
  outcomes are the taxpayer's manual adjustment, with the OVERVIEW.md citation
- Whichever way it resolves, the decision must note the interaction with the spot-rate override
  above (with monthly rates and same-month T+2 settlement the component is nil by construction;
  spot-rate-per-leg entry is what makes it visible)

## Known-limitations review against the ATO reference docs (2026-07-13)

A pass over the `docs/API.md` Known-limitations list asking which entries the ATO guidance
mirrored in `docs/ato/` can actually resolve. Most entries stay deliberate scope decisions
(taxpayer entity types, one taxpayer, cost-base elements 3–5, the indexation method, K10/K11
settlement forex — decided 2026-06-12, cost-base FX timing — decided 2026-07-13, unvested ESS
grants, the estate/LPR side of inheritances). Two resolve into work:

### Foreign broker-cash interest reports at 20E

`docs/ato/tax-return-labels-2026.md` gives the exact treatment the limitation deferred:
interest-like income from a foreign payer (a US broker's Treasury liquidity / money-market sweep
fund) is assessable foreign source income (question 20, label 20E), not Australian gross
interest (question 10, label 10L), and foreign tax withheld from it is claimed via the question
20 FITO (20O).

- An interest-income row records whether its payer is foreign (`foreign_source`, defaulting to
  Australian so existing rows keep their meaning) and any foreign tax withheld
  (`foreign_tax_paid`)
- The tax summary reports foreign-source interest on its own `foreign_interest_income` line
  mapped to 20E (never 10L), with the foreign tax joining the FITO line under the existing
  A$1,000 de-minimis; both classifications count in gross assessable investment income
- Withholding must match the classification at write time: foreign tax on an Australian-source
  row (or a TFN amount on a foreign-source row) is rejected 422 — otherwise the FITO line could
  claim an offset the row can't support
- The Known-limitations entry is removed; entity docs, tax-summary docs, CSV label mapping, and
  the web UI form/columns carry the classification

### Pre-CGT holdings cannot be entered

The limitation warned "pre-CGT holdings are not modelled and should not be entered — the system
would wrongly compute a capital gain or loss on such a parcel". Enforce it at write time (the
data-integrity rule: invariants are enforced on write, not hoped for):

- Any trade or Sell dated before 20 September 1985 is rejected 422 via the shared core-figure
  check, naming the pre-CGT rule
- An inheritance whose date of death is before 20 September 1985 is rejected 422: the parcel
  would be pre-CGT in the beneficiary's own hands (s 115-30 deems acquisition at the death at
  latest), whichever cost-base rule was chosen
- The ATO worked examples that include a pre-CGT parcel alongside a post-CGT one (TD 2000/10
  Examples 1–2, bonus shares Example 35) enter that parcel with the first post-CGT date as a
  stand-in — the re-basing arithmetic under test is date-independent, and each test notes the
  substitution

## Lossless trade round-trip for GST-inclusive brokerage (2026-07-13)

Found scripting against the API during the crypto reconciliation: on a trade stored with
`brokerage_includes_gst` set, `GET /trades/:id` returns the stored **ex-GST split**
(`brokerage` + `gst_on_brokerage`) alongside the flag, but `PUT /trades/:id` with the flag set
interprets `brokerage` as the **one GST-inclusive amount** and re-splits it. So a faithful
GET→edit→PUT round-trip — the natural way any API client edits a row — silently shrinks the
brokerage by the GST each time (0.99 stored → read back as 0.90 + 0.09 → re-split to 0.82 + 0.08).
The web form escapes only because it recombines the pair before saving; every other client hits a
silent data corruption with no 422.

- A trade read back and PUT back unchanged must store identical values — the round-trip is
  lossless for every field, including the GST-inclusive brokerage case
- How is design-open: make reads present `brokerage` as the same GST-inclusive amount the write
  path expects when the flag is set (the web form recombining today shows the read contract
  already means "form value", but it must then be changed in the same step so it doesn't
  double-recombine), or make the write path accept the stored split pair as-is when it is
  supplied intact — either way the asymmetry goes
- Whichever shape wins, a regression test PUTs a GST-inclusive trade, GETs it, PUTs the response
  body back verbatim, and asserts the stored brokerage/GST are unchanged
- `docs/API.md`'s GST-inclusive brokerage section states the round-trip semantics explicitly

## Listing activity ledger (2026-07-13)

A per-listing report showing all recorded activity for one listing in chronological order — buys,
sells, DRP reinvestments, transfers between accounts, dividends/distributions, corporate actions,
AMMA and ESS statements, rights sales, DRP enrolment changes, listing-scoped investment expenses —
each row dated and labelled (including how an operation-created trade came to be: rights exercise,
buy-back, scrip exchange, demerger, ESS vest, inheritance, transfer network fee), with a running
units-held balance, and finishing with a final holding summary: units held, cost base, and current
market value (live-priced by default; an explicit price wins).

## FreeBSD packaging, versioned releases, and a configuration file (2026-07-13)

The server is deployed on a FreeBSD 15.1 (amd64) host. Pushing to GitHub must produce an
installable FreeBSD package without manual build steps, releases must carry proper version
numbers, and the server must be configurable from a file so the rc.d service does not need a
pile of CLI flags.

- Versioned releases from CI
  - `Cargo.toml`'s `version` is the single source of truth for the version number; the binary
    reports it via `--version`
  - On push to `main`, when no release exists yet for the current version, CI builds the
    FreeBSD package, tags the commit that produced it as `vX.Y.Z`, and publishes a GitHub
    release with the `.pkg` attached; a push without a version bump publishes nothing (CI
    still runs)
- FreeBSD package
  - Built natively in a FreeBSD 15.1 VM so the pkg ABI matches the deployment host; contains
    the release binary, an rc.d service script (`share_tracker`, daemon(8)-supervised under a
    dedicated non-login user created on install), and a `@sample` configuration file at
    `/usr/local/etc/share-tracker.toml.sample` (preserved user edits on upgrade)
  - The freshly built package is installed and smoke-tested inside the VM (service script
    loads, `--version` runs, the server starts and answers over HTTP) before anything is
    released
- Configuration file
  - A TOML file controlling the DB path, backup directory, bind host, port, and schedule path
  - Loaded automatically from `/usr/local/etc/share-tracker.toml` when present; `--config PATH`
    overrides the location and must exist when given; CLI flags override config-file values;
    built-in defaults (today's behaviour) apply when neither is set
  - Unknown keys and invalid TOML fail startup loudly — a typo must not silently fall back to a
    default (this is a financial-records server; starting against the wrong database is worse
    than not starting)

## Release notes from the commits between tags (2026-07-13)

Each published release's notes are generated from the commit history at build time: every commit
subject between the previous release tag and the commit being released (newest first, each with
its abbreviated SHA), followed by a full-changelog compare link. The first release — no previous
tag — lists every commit. GitHub's `--generate-notes` is built from merged pull requests, which a
commit-directly-to-main solo workflow doesn't have, so the notes must come from the commits
themselves.

## Attachment coverage: more owners, plain-text files (2026-07-15)

The document archive holds records the attachments feature could not yet store against their
entries (found while attaching the archive to the recorded activity):

- Plain-text records (crypto exchange trade records, registry DRP advices, an early plain-text
  ESS statement) are valid supporting documents — `.txt` uploads (`text/plain`) join the accepted
  attachment content types
- An annual employee share scheme statement documents an `ess_statements` row, and a broker
  statement whose only activity is cash interest documents an `interest_income` row — both entity
  types accept attachments exactly like trades, income, and AMMA statements (upload, list,
  download, delete, cascade on owner delete, audit trail, web UI Attachments action)
