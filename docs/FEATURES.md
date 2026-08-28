# Features

The detail behind [the README's feature summary](../README.md#features). Every entry here is a feature that exists and is tested; deliberate omissions are in [Known limitations](API.md#known-limitations) and summarised under [Scope](#deliberate-scope-cuts) at the foot of this page.

## Contents

- [Recording what you hold](#recording-what-you-hold)
- [Capital gains and corporate actions](#capital-gains-and-corporate-actions)
- [Managed funds (AMIT / AMMA)](#managed-funds-amit--amma)
- [Other holdings and income](#other-holdings-and-income)
- [Reports](#reports)
- [Prices and foreign exchange](#prices-and-foreign-exchange)
- [Cross-checks and alerts](#cross-checks-and-alerts)
- [The application itself](#the-application-itself)
- [Deliberate scope cuts](#deliberate-scope-cuts)

## Recording what you hold

Every figure in every report comes from these entries — the tool computes, it never guesses.

### Trade recording

Buys, sells, and dividend reinvestment plan (DRP) acquisitions, with automatic settlement date calculation per exchange

### Statement-friendly trade entry

Brokerage can be entered GST-inclusive as the contract note quotes it (the server derives the 1/11 GST component, rounded to the cent), and an optional statement total cross-checks the entry against the note's net transaction total at write time — a figure that doesn't reconcile is rejected with the computed amount in the response (see [Trades](API.md#trades))

### Income recording

Dividends and trust distributions with full Australian tax component breakdown (franked/unfranked amounts, foreign source income, franking credits, conduit foreign income, TFN withholding, and the LIC capital gain amount a listed investment company advises — entered as printed, with the individual's 50% deduction computed for question D8); a trust distribution can carry its **present-entitlement date** (usually the distribution period's end) so a June distribution paid in mid-July is assessed in the financial year just ended, as the ATO requires (QC 23087, `docs/ato/trust-income-timing.md`) — dividends stay assessed by payment date

### Interest income

Bank, term-deposit, and broker-cash interest recorded as the statement shows it (the gross amount including any amount withheld, with the withheld amount alongside), classified by payer: the [tax summary](API.md#tax-summary) reports an Australian-source row as the year's gross interest (tax-return question 10 label L, its TFN amount joining the combined withholding line, label M) and a **foreign-source** row (e.g. a US broker's money-market sweep fund) as **20E assessable foreign source income** with its foreign tax withheld joining the FITO line (`docs/ato/tax-return-labels-2026.md`); both count in gross assessable investment income

### Statement-friendly income entry

The income form captures what a registry payment advice prints: the amount with a franking selector (fully franked auto-computes the credit at amount × 30/70; trust distributions record as unfranked trust income), the statement's per-share figures as an optional write-time cross-check (amount per security × securities held must equal the gross, rejected with the computed product otherwise — see [Income](API.md#income)), and a DRP statement's reinvestment entered in the same form (the save chains the linked DRP trade); the full component field set sits behind an advanced toggle

### DRP reinvestment

Enrol holdings in a Dividend Reinvestment Plan over dated enrolment periods (enrol, unenrol, re-enrol), per (listing, holding account) — the same listing may be enrolled in one account and not another — then turn a distribution into a linked DRP trade in the distribution's account; reinvestability is checked as at the distribution's ex date, and leftover cash that can't buy a whole share is carried forward to the next reinvestment in the period (the chain never crosses accounts) or paid out, per the period — unenrolling pays out the trailing carried residual. A plan that prints the units it allotted enters that figure instead: taken exactly as the trade quantity and cross-checked against the reinvestable cash — a **fractional** allotment (e.g. a US broker DRP) spends the whole distribution and leaves no residual, while whole stated units leave the cash they did not buy as the period's residual, like the computed path.

**Partial** participation is out of scope; the workaround is to split the distribution into a reinvested row and a cash row (see [Known limitations](API.md#known-limitations)). Because each reinvestment reads its brought-forward cash from the one before it, they are entered in **payment order**: one the period already has a later DRP trade for is refused. A reinvestment can be **undone** (the DRP trade deleted and the link cleared, atomically — last-in-first-out along the residual chain, the same rule from the other end) to redo it at a corrected price; the link itself is provenance the CRUD endpoints can neither forge nor silently clear (see [DRP reinvestment](API.md#drp-reinvestment))

### Holding accounts

The same listing can be held in several accounts of the one taxpayer at once (e.g. RSU-vested shares in an employer share-plan account that cannot DRP alongside DRP-enrolled shares in a personal broker account); trades, income, AMMA statements, and DRP enrolment periods each carry a holding account, every write defaults to the seeded default account so single-account users never see the dimension, and the holdings reports show the same listing once per account

### Transfers between holding accounts

Move parcels (whole or partial) between two accounts of the same owner, e.g. vested plan shares to the personal account: **not a CGT event** — the transfer atomically closes the moved units in the source account and re-creates them in the destination carrying each parcel's remaining reduced cost base and its acquisition date (the 12-month discount clock and AUD translation month are unchanged), and nothing appears in the gains reports; deleting the transfer restores the pre-transfer holding. A crypto wallet-to-wallet transfer can carry an optional **network fee** paid in the transferred crypto: the move stays a non-CGT event, but the crypto burned to cover the on-chain fee **is a disposal** (ATO guidance) — recorded at its AUD market value so it flows through the gains reports with the 12-month discount, created and deleted atomically with the transfer

### Document attachments

Attach a supporting file (trade confirmation, dividend/AMMA/ESS statement, demerger booklet, plain-text exchange record) to a trade, income row, AMMA/ESS statement, interest-income row, or corporate action; stored as a database BLOB (so the weekly backup covers it, no separate file store), one file per row shown from that row's own Attachments action. An **Attachments report** lists every stored document portfolio-wide against the activity and listing it belongs to, with Download, View-in-new-tab, and a link back to the owning record (see [Attachments](API.md#attachments), [Attachments index](API.md#attachments-index))

## Capital gains and corporate actions

Parcel-level CGT, and the corporate actions that re-shape a parcel without a sale.

### Parcel-level CGT

Explicit parcel allocations link sell trades to the parcels they came from; cost bases are pro-rated and AMIT-reduced at the parcel level

### Return of capital (CGT event G1)

Record a company's non-assessable payment as a corporate action; the per-unit amount reduces the cost base of every parcel held on the payment date across all reports, and a payment in excess of a parcel's cost base becomes a capital gain in the net capital gain report (the cost base floors at nil — G1 never produces a loss)

### Share splits and consolidations (TD 2000/10)

Record a conversion of a listing's shares into a larger or smaller number as a corporate action; no CGT event arises: parcels keep their total cost base and original acquisition date (the 12-month discount clock keeps running) while quantities and per-unit cost bases are re-based across the conversion in every report and in the Sell/trade capacity checks (a post-split sale allocates post-split units against pre-split parcels)

### Bonus issues (non-assessable)

Record a bonus share issue as a corporate action; the ATO apportions each parcel's cost base over the original + bonus shares and the bonus shares take the original acquisition date, so the issue is the same no-CGT-event quantity re-base as a split: parcels grow by `bonus/held` per unit with their total cost base and acquisition date untouched (bonus shares received *in lieu of a dividend* are assessed as a dividend and entered as a DRP trade instead)

### Rights issues

Record a rights issue (free rights to acquire new shares at a set price) as a corporate action, then exercise it: the exercise creates a new Buy parcel **acquired on the exercise date** (the 12-month discount clock runs from exercise, not from the rights or the original shares) with a cost base of the exercise payment plus any amount paid to acquire the rights (nil for free rights, and necessarily nil under a non-renounceable offer, whose entitlements cannot be bought — a cost there is refused). Each issue records whether the offer was **renounceable**, because a **retail premium** is taxed on that fact alone: under a renounceable offer it is a capital gain (not a dividend) per TR 2017/4, and under a non-renounceable one an **unfranked dividend** per TR 2012/1 — entered as income, with the rights-sale path refusing it so it can't be booked as a gain (a nil-proceeds lapse is still recorded).

**Selling or lapsing the rights themselves** — including that renounceable-offer retail premium — is its own operation: a disposal of the rights that leaves the share holding untouched, with a **nil cost base for free rights** (the carried cost for purchased rights, so a paid right lapsing at nil proceeds realises a capital loss) and the discount anchored to **each original parcel's acquisition date** (free rights are deemed acquired with the original shares), reaching the realised-gains and net-capital-gain reports. Rights used either way — exercised plus sold — are capped at the entitlement the holding earned at the record date, and each sale's parcel anchoring is capped at what that parcel earned (see `docs/ato/rights-issues.md`, `docs/ato/retail-premiums.md`)

### Off-market share buy-backs

Record the buy-back offer (per-unit price, the dividend component of that price with its franking credit, and the market value had the buy-back not been proposed) as a corporate action, then sell units into it: the participation atomically creates the Sell at the **capital proceeds** per unit (`max(price, market value) − dividend`, per the ATO's market-value rule) with the chosen parcel allocations, plus the dividend component as franked income with its credits — so the CGT and dividend sides land in the right reports with no special casing; a listed-company buy-back announced after 25 Oct 2022 has no dividend component and the whole price is capital proceeds (see `docs/ato/share-buy-backs.md`)

### Takeovers and mergers (scrip-for-scrip rollover)

Record a takeover (every `old` units of the original listing become `new` units of the replacement listing, optionally plus **cash per old unit** — the mixed-consideration offer) as a corporate action, then exchange it: the exchange atomically closes every open parcel of the original listing through a provenance-marked Sell and creates one replacement parcel per consumed parcel carrying its **remaining reduced cost base** and its **acquisition date** (the combined holding period counts toward the 12-month discount, and a non-AUD cost base keeps its original AUD translation).

All-scrip, the rollover **disregards the capital gain** — the disposal never reaches the realised-gains or net-capital-gain reports. With cash (a **partial rollover**, the ATO's Example 27), each parcel's cost base is apportioned between cash and scrip by the consideration's **market values**: the cash side is a capital gain assessed now (discount per the original holding period) and only the scrip side's share rolls over into the replacement parcels.

Takeovers without rollover (a pure-cash takeover is an ordinary Sell) and multiple replacement share classes are not modelled (see `docs/ato/takeovers-and-scrip-for-scrip.md`)

### Demergers (Div 125 rollover)

Record an eligible demerger (every `held` units of the head listing receive `new` units of the demerged listing, with the head-entity-advised percentage of the cost base apportioned to the new interests) as a corporate action, then demerge it: the demerge atomically closes every open head parcel through a provenance-marked Sell — any gain is **disregarded**, so nothing reaches the realised-gains or net-capital-gain reports — and recreates each as a head replacement parcel plus a demerged-entity parcel splitting its **remaining reduced cost base** by the percentage, both keeping the parcel's **acquisition date** (the head dates are unchanged by law; the new interests' 12-month discount clock runs from the original acquisition, per the ATO's Example 32). Demergers without rollover and pre-CGT original interests are not modelled (see `docs/ato/demergers.md`)

### Worthless / delisted shares (CGT events G3 and C2)

Recognise a capital loss on a failed company without an ordinary sale: record a worthless-shares corporate action (a liquidator/administrator's `G3Declaration` of no-likely-distribution, or a `C2Cancellation` on deregistration), then recognise it: the recognise atomically closes every open parcel through a provenance-marked Sell at **nil proceeds**, each parcel producing a capital loss equal to its **remaining reduced cost base**. Unlike the scrip-for-scrip/demerger rollovers, the loss is **recognised** (never disregarded, never income, never discounted) — it flows through the realised-gains report and the net-capital-gain loss pool and carries forward like any realised loss. The G3 opt-in eligibility tests, worthless financial instruments other than shares, and the 18-month later-recovery timing rule are not modelled (see `docs/ato/worthless-shares.md`)

## Managed funds (AMIT / AMMA)

Attribution Managed Investment Trusts, whose annual statement adjusts the cost base of every parcel held.

### AMIT/AMMA support

Annual tax statements for Attribution Managed Investment Trusts (AMITs), with cost base adjustments applied per purchase parcel. A fund that **converted** to an AMIT records the 1 July its first AMIT year began, and every rule that keys off the flag follows the record's own financial year: the earlier years stay ordinary trust income — franking credits, tax-deferred amounts and E4 return-of-capital reductions intact, no AMMA statement expected, still reported in the tax summary. An AMIT's quarterly cash distributions are entered as **cash-only income rows**: they fund DRP reinvestment and the statement cross-checks but contribute nothing to the tax summary's income lines — the AMMA attribution is the only assessable record, so the cash is never double-counted (write-time validation keeps the notional tax components off the cash rows; see [Income](API.md#income))

### AMIT cash cross-check

A non-blocking report flags every financial year with AMIT cash distribution rows but no AMMA statement covering the year, so cash-only entry can't silently drop a year's income from the return; coverage is asked **per holding account**, since a registry issues one statement per holder account, and an AMMA year with no cash rows is fine (see [AMIT cash cross-check](API.md#amit-cash-cross-check))

### AMIT adjustment generation and cross-check

An AMMA statement's per-parcel cost base adjustments are generated from the parcels actually held at its tax year end (one transaction, previewed and confirmed against the statement's units held before anything is written, re-runnable after a missed trade is entered) rather than typed a row at a time; a non-blocking report flags every statement whose set doesn't reconcile — none entered at all, adjusted units outside the band the year allows — the units held at its end, plus the units disposed of during it, so a statement for the year a holding was sold (nil units held, rows covering the units the fund attributed to before the sale) reconciles rather than being flagged forever — the same parcel adjusted twice, or a parcel that can't have been held in the statement's year.

Generation itself covers the parcels open at the year end, so a holding closed during the year is entered by hand: its refusal says so, instead of sending you looking for trades that are already there. Where a **transfer has moved the units since** the year end — the ordinary case, since a statement for a year ended 30 June arrives in spring — generation follows it and writes the row against the transfer-in parcel, wherever the units went: that parcel carries exactly the units moved under the source parcel's own acquisition date, so it is identified without guessing, and the statement's reduction lands where those units' cost base now lives (a scrip-for-scrip exchange or demerger scales its replacements by a ratio and links none of them to the parcel it replaced, so those units are listed in the response for hand entry against the replacement instead).

A missed parcel overstates its cost base; a duplicated one over-reduces it, and CGT event E10's nil floor can turn that into a capital gain that was never made — so the duplicate is refused at write time outright — as is an adjustment against a parcel a transfer, scrip-for-scrip exchange or demerger has already carried into a replacement parcel, where the reduction would reach nothing (the refusal names the replacement, which is where the row goes instead) (see [Generating AMIT adjustments](API.md#generating-amit-adjustments) and [AMIT adjustment cross-check](API.md#amit-adjustment-cross-check))

## Other holdings and income

### Employee share scheme (ESS) income

Record an Employee share scheme statement (the Item 12 discount labels: taxed-upfront eligible/not-eligible, deferral, pre-2009 cessation, plus the foreign-source memo and TFN withheld) attributed to a listing and holding account, then vest it: the **assessable discount** reaches the [tax summary](API.md#tax-summary) per financial year — net of the $1,000 taxed-upfront reduction — whose ≤A$180,000 adjusted-taxable-income test is over income the tool cannot see, so it is **recorded per financial year** rather than computed: mark a year ineligible in **Tax Year Settings** and its discount is reported unreduced, while recording nothing applies the reduction as before, and the printed annual document footnotes the condition wherever a reduction was applied and reported separately from dividend income — and the **Vest** action atomically creates the cost-base-reset Buy for the vested shares (market value at the taxing point, acquired on the taxing-point date), tying the income and CGT sides together (see `docs/ato/employee-share-schemes.md`).

A foreign-currency statement can carry the employer's stated **AUD** figure per discount label (employer statements convert at the release-date spot rate — what the ATO prefill carries), which the tax summary reports verbatim instead of RBA-converting so the summary matches the lodged return; it is entered in its listing's own currency (the per-share market value and the listed price are the same money) and can state the **FX rate** the employer used, which both sides then convert at where the taxing point's month has no imported RBA rate — a foreign vest with neither rate is refused rather than costing the parcel at parity; the discount labels stay editable after vesting, since the employer's annual statement arrives later

### Inherited share parcels

Record a parcel passing from a deceased estate (not a CGT event on transfer): the entry captures the date of death, which ATO cost-base rule applies — the **deceased's cost base at death** (asset acquired by the deceased on/after 20 Sep 1985) or the **market value at death** (pre-CGT asset) — plus any legal-personal-representative expenditure (added to the cost base, dated when incurred; AUD holdings only — see Known limitations), and atomically creates a provenance-linked parcel Buy that flows through every report and capacity check like any Buy; the 12-month discount clock follows s 115-30 (from the deceased's acquisition for a post-CGT asset, from the death for a pre-CGT one). The estate/LPR side is not modelled and the market value at death is user-supplied; the cost base you enter is your own share of it, with any indexation recalculated out where the death was on or after 21 September 1999 (see `docs/ato/inherited-assets-cost-base.md`, `docs/ato/inherited-assets-cgt-discount.md`, [Known limitations](API.md#known-limitations))

### Deductible investment expenses

Record the cost of earning investment income (interest on money borrowed to buy income-producing shares, management/adviser fees, account-keeping fees, subscriptions) as the **post-apportionment deductible amount** (apportionment is your determination — brokerage isn't an expense here, it forms the CGT cost base); the [tax summary](API.md#tax-summary) totals them by type per financial year and nets them against gross assessable investment income to a **net assessable investment income** figure (see `docs/ato/investment-income-deductions.md`). The same total is also cut by **the question each deduction is claimed at**, derived from the holding it is attributed to: expenses of earning a trust or AMIT distribution at **13Y** (interest on money borrowed to buy the units included — question 13 takes debt deductions), expenses of earning foreign-source income netted into **20M**, a debt deduction against foreign income at **D15**, and everything else at **D7/D8**; a portfolio-wide expense can't be routed from what's recorded and reports at D7/D8 (`docs/ato/tax-return-labels-2026.md`).

The annual tax report prints each deduction's destination beside it. Each row is deducted in full in its own financial year, so an expense the ATO spreads across years — borrowing costs over $100 (5 years or the loan term) or a prepayment running past 12 months — is entered as one row per year carrying that year's share (`docs/ato/expense-time-apportionment.md`, [Known limitations](API.md#known-limitations))

### Crypto-asset holdings

Investment crypto (BTC, ETH, …) is a CGT asset per the ATO (`docs/ato/crypto-cgt.md`), recorded as an exchange-less `Crypto` listing whose ticker must be a recognised digital-token code; crypto trades settle same-day (no T+n, no holiday calendar) and crypto parcels flow through every report and holding-account transfer exactly like share parcels — AUD cost base/proceeds, the 12-month 50% discount, and loss netting included (crypto-to-crypto swaps — wrapping included — chain splits, airdrops and staking rewards have no operation of their own: each is entered as the ordinary trade the CGT event already is — a staking reward or established-token airdrop adding an `OtherIncome` income row at **item 24** for the money value received — and [Known limitations](API.md#known-limitations) gives the entry for each)

## Reports

All AUD, all computed live from the recorded facts.

### Portfolio overview

The app's home screen (`#/`): open holdings per security with total cost base and market value, plus a market-value/unrealised-gain graph over the stored daily [report snapshots](API.md#report-snapshots) — **one series at a time**, chosen by a selector above the graph and remembered across reloads (the two figures are an order of magnitude apart, so sharing one axis drew the smaller as a flat line), with the y axis scaled to the plotted series (its extremes plus 10% of their span as headroom, never anchored at zero, and a dashed zero baseline drawn where the series crosses it), an x axis of weekly gridlines counted back from the latest snapshot and dated on as many of them as the plot is wide enough to carry (weekly, then fortnightly, then four-weekly), and each point's value shown on hover — the graph is as wide as the browser window rather than capped, built at its measured width so the extra width becomes horizontal room for the series instead of a scaled-up drawing — with quick date-range presets (1M/3M/6M/1Y/2Y/3Y/FY-to-date/all — the last-picked preset is remembered across reloads via `localStorage`, so the panel reopens on it instead of resetting to All

A custom range is ad-hoc and isn't remembered) and a custom range, and a [period performance](API.md#period-performance) summary for the selected range: the period's return attributed into **capital growth**, **FX movement**, and **income** (always summing exactly to the return), plus per-holding contributions (a default-checked "hide holdings with no activity in this period" checkbox, also remembered across reloads, filters out holdings that were fully closed before the range began) and a per-currency FX breakdown

Shortcut buttons (**New trade**, **New income**, **New sell**, **New transfer**) sit above the graph for the most common data-entry paths

### Listing activity ledger

Everything ever recorded against one listing in chronological order (trades labelled with the operation that created them, transfers, income, corporate actions, AMMA/ESS statements, rights sales, DRP enrolment periods, listing-scoped expenses), each row with its AUD amount and a running units-held balance that splits and bonus issues re-base, ending in the final holding summary per account: units held, cost base, and current market value (live-priced; an explicit price wins) — see [Listing activity](API.md#listing-activity)

### Unrealised gains report

Per-holding gain/loss and CGT-discount-eligible quantity as at a given date

### Realised gains report

Per-sale capital gain/loss split into discount-eligible (parcels held strictly more than 12 months), non-discountable, and loss buckets; expandable in the web UI to the individual parcels sold and each one's own cost base, proceeds, gain/loss, and discount eligibility (with an "expand all" control)

### Performance report

Investment performance (not tax) per holding and overall: total return (AUD and % of invested), annualised money-weighted return (IRR over the dated cash flows), and trailing-12-month income yield, valued with live or supplied prices as at a chosen date; holding-account transfers and rollover exchanges don't distort the portfolio-level figures

### Net capital gain report

The overall CGT position per financial year: combines realised parcel gains with AMMA-attributed CGT gains and capital losses, applies losses ATO-optimally (non-discountable gains first), carries unused net capital losses forward across years (seeded by an enterable opening carried-forward loss), and applies the 50% discount to produce the assessable net capital gain; expandable in the web UI from a year down to its realised disposals and, within each, its individual parcels

### Tax summary

Income aggregated by Australian financial year (July–June; trust distributions by their present-entitlement date when recorded, dividends by payment date), combining dividends, trust distributions, interest, AMMA components, and the assessable ESS discount (net of the $1,000 taxed-upfront reduction, reported separately); AMIT cash distribution rows are excluded — the AMMA attribution is the assessable record, so the cash is never counted alongside it; deductible investment expenses are totalled by type and netted against gross assessable investment income to a net assessable figure; franking credits are reported as claimable only, applying the 45-day at-risk holding-period rule (90 days for preference shares, LIFO share identification) with the A$5,000 small-shareholder exemption; the foreign income tax offset is capped at the A$1,000 FITO de-minimis, with the excess surfaced separately

### Tax-return CSV export

The tax summary and net capital gain reports download as tax-return-ready CSV (`GET <report>/export`), one record per financial year with the same columns as the JSON response (money columns rounded to the cent, as on screen; the JSON keeps the exact figure), plus a second header row mapping each column to its myTax/paper tax-return label (e.g. net capital gain → 18A, franked dividends → 11T, franking credits → 11U/13Q; the row's first cell names the form year it targets — currently the 2026 return, verified in `docs/ato/tax-return-labels-2026.md`, full mapping tables in [docs/API.md](API.md#tax-summary))

### Annual tax report

A printable, per-year tax document distinct from the multi-year tax summary above: pick a financial year and Generate, then Print / Save as PDF to archive it. Computes nothing new — every figure is sourced from the existing reports — and shows:

- a data-completeness check (every AMIT fund held during the year has a covering AMMA statement, checked by **holdings**, not just cash rows, so a fund-year with no cash entered at all is still caught — and every such statement's per-parcel AMIT adjustments reconcile to it, since an adjustment gap distorts the disposal schedule's cost base, the document's central figure)
- the trading-activity schedule, per disposed parcel, with the adjusted cost base itemised into one row per AMIT/return-of-capital/split adjustment
- the ATO gain/loss worksheet (short-term/long-term, the grossed-up AMMA discount distribution, losses offset, the 50% concession, to the final Capital Gain)
- income broken out by category (trust, dividend — each with its franking entitlement status, foreign — with non-AMMA and AMMA subtotals and a total, so question 20's gross reads off the page — interest, ESS, deductions)
- the overall tax summary with each line's ATO label

See [Annual tax report](API.md#annual-tax-report).

### Parcel-selection optimiser

Decision support for a contemplated sale: which parcels a sale comes from is the taxpayer's choice (`docs/ato/cgt-keeping-records-shares.md`), so given a listing, account, units, sale date, and a price (live-fetched by default; an explicit price wins) the report returns candidate strategies — minimise current-year assessable gain, maximise the discount-eligible proportion, harvest losses first, FIFO baseline — each with its per-parcel allocations and gross gain / discountable split, ready to enter on the real Sell; read-only, nothing recorded; each strategy expands in the web UI to its own per-parcel allocations (see [Parcel-selection optimiser](API.md#parcel-selection-optimiser))

### Pre-sale what-if

Dry-run a hypothetical disposal (units, proceeds, date, with explicit allocations or an optimiser strategy) through the net capital gain computation and see the disposal year's figures with and without it, earlier years' carried-forward losses included; no rows are written, and the whole-of-income tax estimate stays out of scope — the CGT-side delta only; the hypothetical disposal expands in the web UI to its per-parcel allocations (see [Pre-sale what-if](API.md#pre-sale-what-if))

## Prices and foreign exchange

### Live valuation

The price-dependent reports (portfolio overview, unrealised gains, performance) value holdings from the **current** price at the price source on demand, each quote converted to AUD and carrying its provider as-of time (a "as at …" freshness line); explicitly supplied prices still override (what-if), and a per-listing fetch failure leaves that holding unvalued with a reason rather than a silent zero. Every listing needing a quote is fetched in one request rather than one per holding, and a quote is reused for 60 seconds, so revisiting the home screen or moving between the three reports costs no round trip — the as-of time shown is always the provider’s own, never the moment it was served (see [Live valuation](API.md#live-valuation))

### Daily closing prices

Every held listing's closing price is collected after its exchange's close (crypto at the UTC-midnight cut-off) and stored as history in the listing's quote currency, via a pluggable fetcher backed by Yahoo Finance; each run self-heals the last 14 calendar days (missing or errored days are re-attempted, ok days never re-fetched) — the same window the snapshot job retries, so a blocked date is always still reachable, failed fetches are stored as errored rows, and history is backfillable over a date range on demand. A day the provider can never serve — a delisted or mis-served symbol, or a permanent hole in its series — is **priced by hand**, recorded with where the figure was sourced from and why manual entry was needed, and valued by the reports exactly like a fetched price.

An **unbounded run** of such days — the provider stops quoting the security altogether, at a delisting or a suspension that can run for years — is not hand-priced day by day: the listing records the date it stopped being quoted (**unpriced from**), which stops collection fetching it, quiets the health alarm for those days, and has valuation carry the last stored close forward — flagged, never silently — instead of one suspended holding blocking the whole portfolio's snapshots every day. The mirror is recorded too (**unpriced before**): the day a provider's series *begins*, before which nothing is obtainable at any price (a spun-off entity). There nothing is substituted — the holding is **excluded** from that date's portfolio totals, which name which holding is absent and why, so the total is smaller by a real holding and the value graph steps where the listing's own series begins.

That marker supersedes whatever is stored for those days, so it is also the one span in which a stored price may be **deleted** — one date at a time, or a whole span at once — the acknowledgement that a figure nothing values never was a valuation; every cleared row stays in the audit trail (see [Closing prices](API.md#closing-prices). Each stored price is held in **its own trading day's unit basis** — what the security actually traded at that day — which the provider does not serve: it restates a security's whole close history into the current basis when the security splits. The figure as observed is kept beside the stored one, so a price fetched after a split is normalised on entry, and recording, editing or deleting a share split or bonus issue re-derives the listing's stored prices in the same transaction — the valuation series no longer steps by the split ratio, and the order the split and the prices were entered in cannot matter (a hand-entered price is contemporaneous by declaration and is never rewritten).

A **demerger** restates the provider's series the same way while changing no unit count, so it has no ratio to read: record what the security **actually closed at** on the last pre-demerger trading day on the demerger action (with the same sourced-from/why provenance a hand-entered price carries) and the factor is derived from it against the provider's own figure for that day — the Health report names any demerger whose pre-demerger prices still need it)

### Daily report snapshots

Once the day's last close is in, the price-dependent reports (portfolio overview, unrealised gains, performance) are run against the stored closing prices (AUD-converted) and persisted as a daily series, feeding the Portfolio Overview screen's graph and period-performance summary; each run also backfills missing dates over a 14-day window, so a blocked date is delayed, not lost. A snapshot valued before the month's RBA FX rate is published is stored flagged **provisional** (valued at an earlier month's rate, at most 2 months back) and finalised automatically once the rate import lands the real rate

One valued with a holding whose listing is marked **unpriced from** a date carries a separate **carried-forward price** flag — the last stored close stood in for a security the provider no longer quotes, which nothing ever trues up, so clearing the marker when it relists stales those dates instead — and one dated before a held listing's **unpriced before** carries an **excluded holding** flag with the list of what the total omits, since nothing there was ever quoted and nothing is substituted (a date on which *every* holding is excluded is blocked rather than stored as a false zero)

Recording a back-dated fact marks the affected snapshots stale — atomically, via database triggers, and a fact recorded while a run is in flight waits for that run rather than being lost between its figures and its freshness flag (a write that waits past the 30-second busy timeout is answered `503` saying the database was busy and the request can be sent again, never a bare error; a **bulk regenerate-all** holds the writer effectively continuously for its whole run, so treat it as an exclusive maintenance operation) — and the daily run regenerates stale/provisional window dates itself, with a date-ranged regenerate-all action (defaulting to the whole history — first-ever holding through the latest fully-valuable date — so it can also backfill dates that never had a snapshot) and a regenerate-provisional action to bulk-repair the series (see [Report snapshots](API.md#report-snapshots))

### FX rate import

Monthly RBA F11 foreign exchange rates (the rates the ATO directs taxpayers to use) fetched and stored as foreign-per-AUD, refreshed weekly and via a manual trigger

### AUD conversion

Cost base and proceeds in the portfolio, unrealised, and realised reports are converted to AUD at the ATO reference rate (with a per-trade manual `fx_rate` fallback), and a trade can carry a deliberate **transaction-date spot-rate override** (`spot_fx_rate`) that wins over the monthly rate — the monthly rate is the ATO-published convenience default, reasonable for recurring/small amounts, but per QC 18020 an average rate is not appropriate for a one-off purchase or sale of a large capital asset; see [FX conversion](API.md#fx-conversion)

### MIC registry import

The ISO 10383 Market Identifier Code list imported monthly (and via a manual trigger), used by a non-blocking report to flag curated exchanges whose MIC is unknown or expired

### Ticker and exchange-code renames

A rename (e.g. LAAC → LAR) is recorded as a dated, audited event (`POST /listings/:id/rename`) rather than a bare field edit once a listing has any recorded trades, income, or prices: parcels, cost bases, and the 12-month discount clock stay attached across it, price history fetches under the symbol in force on each date (so pre-rename days are recovered under the pre-rename symbol automatically, with a `price_symbol` override still available when the provider simply spells a symbol differently), and the Annual Tax Report and listing activity ledger show the ticker as it stood at each row's own date (see [Listings](API.md#listings))

## Cross-checks and alerts

Non-blocking reports, every one of them: they name what looks wrong and never refuse a write. Each exists because the mistake it catches is individually plausible and silently wrong downstream.

### FX coverage alerting

A non-blocking report naming every recorded amount whose ATO monthly rate has not been imported (and what each converts at meanwhile — a deliberate spot rate, the record's own silent fallback, or nothing at all, which fails the report), plus the two documented FX simplifications where they actually bite: a settlement window crossing a rate month (CGT event K10/K11) and a cost-base reduction converted at the parcel's acquisition month (see [FX coverage](API.md#fx-coverage))

### Settlement-holiday coverage alerting

A non-blocking report answering both halves of "is this settlement date trustworthy": exchange holiday calendars are seeded for a finite range of years, so auto-calculating a settlement date outside that range logs a warning and flags every trade whose settlement **window** falls outside its exchange's seeded coverage; and every stored settlement **date** is put to the listing's own trading calendar, so a hand-entered settlement landing on a weekend or a public holiday is surfaced too (never refused — an explicit `settlement_date` is a deliberate override, so the row stays editable); seeding the missing calendar the report asks for is what clears the first half, and the unscheduled `settlement-recompute` job re-derives the settlement dates that were computed while it was missing, so the report goes quiet because the dates are right rather than because the alert is hidden (an entered `settlement_date` is never rewritten) (see [Settlement holiday coverage](API.md#settlement-holiday-coverage))

### Rollover consistency cross-check

A non-blocking report flags every holding-account transfer, scrip-for-scrip exchange and demerger whose **stored** carried cost base (or units) no longer matches what the units it consumed are worth today, and every scrip-for-scrip exchange, demerger and worthless-shares recognise that left a parcel of its listing **unconsumed** — those three take the whole holding as at their date as a matter of law, so a parcel still open then is units the operation could never reach. Those operations compute the replacement parcels' cost base once, when they run, so a later change behind them — a source parcel's price or brokerage edited, an AMMA statement's per-unit figure corrected — would otherwise leave the same holding reporting a different cost base depending only on the order things were entered.

Each row names what was carried, what it should be, the difference, and the fix (delete the operation and run it again); a partial-rollover scrip exchange is listed as *not checked*, because how much of each cost base went to its cash side is the exchange's own apportionment. The annual tax report's completeness section carries these too, unfiltered by year — a rollover from an earlier year is the one this year's disposals are costed on. The loudest ways in are refused outright at write time instead: a split, bonus issue or return of capital dated on or before an operation that has already run, an AMIT adjustment covering units one carried away, and any parcel-creating write — a Buy, an inherited parcel, an ESS vest, a rights exercise, a DRP reinvestment, a rollover's own replacement parcels — dated on or before one of those three whole-holding operations (see [Rollover consistency](API.md#rollover-consistency))

### Tax-deferred E4 cross-check

A non-AMIT trust statement's tax-deferred amount can be recorded on the income row (informational — the CGT event E4 cost-base reduction itself is entered as a Return of capital corporate action), and a non-blocking report flags every row whose amount has no same-financial-year action on the listing, so a faithfully keyed statement can't silently leave the cost base overstated (see [Tax-deferred E4 cross-check](API.md#tax-deferred-e4-cross-check))

### Indexation cross-check

For an asset whose costs were incurred by 21 September 1999 an individual may index the cost base for inflation *instead of* taking the 50% discount, and this tool applies the discount throughout. A non-blocking report names every disposal the other method was available on and sets the two side by side: the CPI quarter, the factor (68.7 ÷ that quarter's CPI, to 3 decimal places), the indexed cost base against the one actually used, and both methods' assessable gains, per parcel and per year. Advisory only — no reported tax figure is computed from it, and taking the other method is your own adjustment. Each row is a comparison made **before capital losses**, which is stated on its year's row: losses are applied before the 50% discount but come straight off an indexed gain, so applying them can only move the comparison further toward indexation — the rows are a floor on its case, not the whole answer. A parcel disposed of at a loss is excluded rather than shown as "the discount wins", because indexation cannot be used on a capital loss at all (see [Indexation cross-check](API.md#indexation-cross-check))

### Wash-sale alerting

A non-blocking report flags every loss-realising Sell with a Buy of the same listing within a configurable window either side (default 30 days), across all holding accounts — the sell-and-repurchase pattern the ATO warns may have the loss cancelled under Part IVA (TR 2008/1); advisory only, writes are never rejected (see [Wash sales](API.md#wash-sales))

### Franking at-risk foresight

The tax summary's 45-day-rule denials, explained: a report lists each dividend whose credits fail the holding-period walk with the failing qualification window and units (and whether the small-shareholder exemption currently shields it) — plus every dividend the rule could not be applied to, because no ex date (or, on a trust distribution, entitlement date) was recorded to anchor the window, so an empty report really does mean every credit the walk can test is claimable (the other two qualified-person tests — the 30%-at-risk test and the related payments rule — are not modelled, see [Known limitations](API.md#known-limitations)) — and a what-if mode — linked from the Sell screens — tests a contemplated sale against the same walk before anything is recorded, showing the credits it would cost and the window end to wait for (see [Franking at-risk](API.md#franking-at-risk))

### Distribution calendar and the missing-dividend alert

A weekly job collects, per held listing, the price provider's own record of what it distributed and when, over the span the listing was held. The [health report](API.md#health) then answers a question every other completeness check here cannot: those compare recorded facts against **each other**, so a dividend or trust distribution that was simply *never entered* is invisible to all of them — it understates the year's income and franking credits, and the AMIT cash cross-check can only compare against rows that exist. Two alerts, both on the cross-view banner: a **known ex-date where units were held and no income row matches it** (carrying the ticker, the ex-date and the gross the missing row would carry), and an **entered distribution whose gross does not match** per unit × units held — the likelier of the two errors, and the one the provider's agreement with the registry to six decimal places shows it is accurate enough to catch.

Held is measured on the **last cum-dividend day**, the day before the ex-date, which is what entitles a holder; matching is per **holding account**, since the same listing held in two accounts pays two distributions; and the gross alone is compared, never the components, which come from the registry statement and nowhere else. **Advisory by decision**: no tax figure is computed from the feed and the annual report's own completeness gate stays on recorded facts alone — an alert firing is worth acting on, an alert not firing is not proof the books are complete (see [Distribution calendar](API.md#distribution-calendar) and the coverage limitation in [Known limitations](API.md#known-limitations))

### Job and data-freshness monitoring

Every maintenance job run (scheduled or manual) is recorded as a bounded per-job history (the newest 20 runs), so an intermittent failure that later succeeded stays diagnosable from `GET /jobs` and the Jobs screen; the row is written when the run *starts*, so a run a restart interrupted shows as one that started and never finished rather than leaving no trace at all; a health endpoint reports the latest stored closing-price date, the latest RBA FX rate month, any job whose latest run failed, any job that is **overdue** (its scheduler's own stored next-run instant came and went more than six hours ago — the signal that catches a job which has *stopped running*, which no run history can, because a job that is not running records nothing at all), and any run left `running` far longer than any run takes (a process that died mid-run rather than one in flight) in one read, alongside the two price-gap lists that between them catch every day a holding cannot be valued.

What it names, each one a mistake that is individually plausible and silently wrong downstream:

- listings with an errored price row, and listings with a **held day nothing ever fetched** (no row at all, otherwise silent until a snapshot sticks stale, and surfaced on the Closing Prices screen with a Backfill action over exactly the hole)
- any two listings holding **one price series between them** (the same close on a long run of consecutive trading days — thirty is the threshold, counted as an unbroken run of comparisons rather than a total, so a chance coincidence on a handful of scattered days is not flagged and a pair pinned to one unchanging figure never is — which is what a series fetched or copied under the wrong symbol looks like afterwards, and the only signal such a fetch leaves behind, since every row of it is ok and its figures look plausible; each side's rows are split into fetched and hand-entered, because a hand-entered row states where it came from and a fetched one may not)
- **demerger recorded the wrong way round** (its head listing is the entity the demerger *created* rather than the one that continued — accepted in full today, since the apportionment percentages attach to the tickers rather than to the roles, so no tax figure moves and no write-time check can see it, while the head parcel sits on a listing that did not exist when it was bought and cannot be valued before the demerger; caught by the asymmetry rather than an absence — the head has no stored price at all before the event while the listing it demerged carries a fetched series running back before it, and the head was held earlier still — so a database that has collected nothing, and a history that was simply never backfilled, are both left alone)
- **duplicated corporate action** (two rows of the same type on one listing and date, whose effect is otherwise silently compounded — reported, never rejected, since a genuine same-day pair is possible)
- **duplicated AMMA statement** (two statements for one fund, financial year and holding account — an amended statement entered as a new row instead of over the original, which doubles that year's attributed income, gains and cost-base reductions alike; likewise reported, never rejected)
- **duplicated distribution** (two income rows of identical amounts for one listing, holding account and payment date, which declares the dividend and its franking credits twice — the amounts are part of the key so that a genuine ordinary + special dividend paid on one day is not flagged; likewise reported, never rejected)
- **duplicated interest credit or deductible expense** (identical rows on the two listing-less sides of the tax summary, each counted once per row — keyed on the payer identity interest has instead of a listing, its free-text source and holding account, and on everything that identifies an expense including its description and attributions; likewise reported, never rejected)
- **duplicated ESS statement** (two statements of identical figures for one listing, holding account and taxing point, which compounds on both sides at once — the year's Item 12 discount assessed twice *and* the parcel vested twice; the accident the 30-day rule invites, an amended employer statement entered as a new row instead of over the original, and again reported, never rejected, since two vests on one date from different grants are ordinary)
- **duplicated trade** (two trades of one listing quoting the same broker **contract note reference** — one confirmation entered twice, the duplication a bulk back-entry of history produces most easily and pays for most dearly, since a doubled Buy inflates the holding *and* its cost base while a doubled Sell inflates the realised gain and consumes a second parcel, and both rows are individually valid so nothing else can see it; keyed on the reference rather than on the figures, which is what makes it a list with **no false positives** — a repeated document id has no innocent reading, where identical figures do (an order filled in equal clips, a regular fixed-dollar purchase) — at the honest price that it only catches trades whose entry recorded the reference, and per listing, since one note can cover a multi-line order; likewise reported, never rejected)
- **duplicated inheritance** (two inherited parcels of identical figures for one listing, holding account and date of death — the one duplicate that doubles a *holding* rather than a year's income, since each inheritance creates its own parcel and no financial year bounds the error; likewise reported, never rejected, since two inheritances from one death are ordinary)
- **ESS sale inside the 30-day rule's window** (a disposal within 30 days after the taxing point, where the rule moves the taxing point to the sale date — the discount is re-measured at the proceeds, can move into the next financial year, and leaves no separate capital gain; flagged with both years named, never re-measured, because the amended statement is the employer's to issue)
- **trade dated on a day its exchange was shut** (a weekend or a seeded public holiday on the calendar in force that day — the trade date is the CGT event date, so it sets the discount clock, the financial year and the settlement count, and a day the market never traded is a data-entry error by construction; the two hand-entry routes, `PUT /trades` and `PUT /sells`, refuse such a date outright, so what this list catches is the rows written by the derived paths — a vest, an inherited parcel, a DRP, a corporate action — which are exempt on purpose because a taxing point, a death or an action's effective date need not be a market day, plus anything entered before the rule existed)
- **disposal recorded at nil proceeds** (a Sell at a zero price, or a disposal of rights that were *paid for* at nothing per right — the shape a **gift** takes when what was entered is what was actually received, which is nothing: under the market-value substitution rule the proceeds are the asset's **market value** at the time of the event, so entering the nothing fabricates a capital loss the size of the whole cost base, nets it against the year's gains and carries it forward until it is absorbed, and every figure downstream is individually valid; flagged rather than refused, because a crypto burn, an abandonment or paid-for rights left to lapse each realise a real loss at genuinely nil proceeds and no stored fact says which this is — while the Sells an *operation* writes at nil proceeds, a worthless-shares recognise above all, are never flagged, and neither is a free right that lapses, which is nil against nil and fabricates nothing)

The web UI shows a cross-view warning banner (linking to the Jobs page) whenever data goes stale, a job fails, or a job's schedule stops moving, and the Jobs screen carries a **next run** column so it can answer whether a job is still scheduled and when it is due — a broken price source is visible from any screen, not only when the Jobs page is opened (see [Health](API.md#health))

## The application itself

### Web UI

A built-in browser frontend (no build step, served from the same binary) navigated by a top menu bar (Activity, Reports, Reference Data, Jobs — each expanding a panel on hover/focus/click, Reports as a mega-menu of titled columns) rather than a sidebar, opening on the Portfolio Overview home screen with New trade/income/sell/transfer shortcut buttons.

What it puts on screen:

- CRUD screens for every entity, atomic Sell + parcel-allocation entry, holding-account transfers, DRP reinvestment
- an Origin column on the Trades/Sells lists labelling operation-created rows (so a transfer-in Buy's cost-base-carrying brokerage figure never reads as a real fee)
- a closing-price history screen with re-fetch and backfill actions
- a snapshots maintenance screen with generate/regenerate and bulk regenerate-all (date-ranged, defaulting to the whole history) / regenerate-provisional actions
- a listing name in any table — report, entity list, or nested breakdown row — linking straight into that listing's activity ledger
- every table opening newest-first on its own date column (the activity ledger included: ties reverse with the sort, so its running units-held balance still steps down the page)
- an inline-SVG time-series graph on the Portfolio Overview screen with date-range presets (stale points hollow, provisional points dash-ringed, carried-forward-price points amber-ringed) and a period-performance summary
- a view for each report, and a printable Annual Tax Report view (plain semantic tables, not the shared filterable/paginated table — a print document has no business with a filter row or a pager) with a Print / Save as PDF button
- a **light/dark toggle** in the top bar whose choice is remembered across reloads (a first visit follows the operating system's own light/dark setting; printing always reverts to the light scheme, so an archived tax-report PDF looks the same whichever was on screen)

See [Web frontend](API.md#web-frontend).

### Append-only audit trail

Every edit or deletion of a financial fact (trades, allocations, income, statements, corporate actions, expenses, hand-entered closing prices, the exchange holiday calendar every valuation reads, and the rest of the audited tables) records the prior row with a UTC timestamp, written by database triggers inside the same transaction so no write path can bypass it; entries are kept forever, the trail itself cannot be rewritten (database-enforced append-only), and any record's history is inspectable via the API and the web UI's Row History screen — which also **browses the recent changes across every audited table**, newest first and cursor-paged, so an operation that changed rows you never named (a demerger group, a cascade delete, a bulk price clear) is found by *when it happened* and drilled into from there, rather than needing an id that appears in no list once the row is gone.

A trail says **which record's history each entry actually is**: nothing binds an id to one row for life, so a record entered under a deleted row's id would otherwise inherit its past (it has happened — a server-assigned id handed a demerger's closing Sell the trail of a share sale deleted minutes earlier), and every entry now names the occupant of the id it belongs to, headed and warned about on screen rather than left to be inferred — so an accidental edit that would silently change prior-year cost bases or tax figures can be noticed and reconstructed (aligns with the ATO record-keeping guidance, `docs/ato/cgt-keeping-records-shares.md`; see [Row history](API.md#row-history))

### Authentication (optional)

A single shared username + Argon2 password hash, set in `[auth]` in the config file, gates the whole application behind a `/login` page and a signed session cookie (access control only, not per-user data — the app stays single-taxpayer); a bearer token can be accepted alongside it for the deployment scripts that call the HTTP API without a browser. Off by default: the server serves exactly as it always has until `[auth]` is set (see [Authentication](API.md#authentication))

## Deliberate scope cuts

What this tool deliberately does not do. Each is a decision, not a gap waiting to be filled, and
each is documented in [Known limitations](API.md#known-limitations) as well:

- Every figure assumes a share **investor** holding CGT assets on capital account, not a **share trader** carrying on a business — for a trader the same holdings are trading stock (profits ordinary income, purchase price and brokerage deductible in the year incurred, losses deductible against any income, and a year-end trading-stock valuation instead of a per-parcel gain), and nothing here can tell the two apart, so the assumption is documented rather than detected.
- **one taxpayer per database** — a second taxpayer (a spouse, a family trust) is a second database and a second server instance (`--db`, `--port`), never a second holding account, which carries no taxpayer identity and would silently aggregate two people into one net capital gain, one capital-loss pool, one A$5,000 small-shareholder franking threshold and one A$1,000 FITO de-minimis, while a **jointly held** parcel is entered as your own share of it (500 units of a 1,000-unit joint holding), the convention inherited parcels already use.
- **gifts / off-market related-party transfers** are a CGT disposal at **market value** (the market-value substitution rule) with no dedicated entry path — enter a manual Sell (gift out) or Buy (gift in) at market value.
- **pre-CGT holdings** (acquired before 20 September 1985) are outside CGT and not modelled — a trade dated before 20 September 1985 (or an inheritance from a pre-CGT death) is rejected at write time, so a parcel the system would wrongly compute gains on can't be entered.
- the **indexation method** (costs incurred by 21 September 1999, frozen at September 1999) has its **election** out of scope — the 50% discount is applied throughout and choosing indexation on a parcel is your own adjustment, though both figures are now shown (the Indexation Cross-Check report above), because the discount does *not* "almost always" win: on a parcel bought in the September 1985 quarter the factor is 1.730 and indexation assesses less whenever the proceeds are below 2.460 × cost.
- **dividend equivalents on unvested RSU grants** are ordinary income when paid (TD 2017/26), and while the accrual is not modelled the payment is recordable as an income row of type `EmploymentIncome` — no surface then calls it a dividend, though salary and wages (item 1/2), where it belongs, is not something this tool reports.
- **settlement-window forex outcomes on foreign-currency trades (CGT events K10/K11)** are not computed — the contract-to-settlement currency movement is the taxpayer's manual adjustment (nil by construction under same-rate-month monthly-rate entry; per-leg spot rates are what make it visible).
- **foreign-currency cash balances** (Division 775 forex gains and losses — ordinary income and deductions, not CGT) are out of scope with **no entry path at all** — there is no cash-balance record, and an income row requires a listing a currency balance does not have — so the figure is worked out and lodged outside this tool.
- a foreign-currency parcel's **AMIT/return-of-capital cost-base reductions convert at the acquisition-month rate**, not each reduction's own payment/period month (s 960-50 translation timing) — material only for a non-AUD holding receiving non-AUD reductions.
- **brokerage is billed in the trade's own currency** — a `brokerage_currency` differing from the trade's `currency` is rejected at write time, since the cost base, net proceeds and transaction total are single-currency sums, so an Australian broker's AUD fee on a US trade is entered converted into the trade's currency.
- **foreign tax on a capital gain you realise yourself** has no field — a Sell carries no foreign-tax column, so only the trust path (an AMMA statement's capital-gains foreign tax, apportioned to its assessable part) reaches the FITO line, the direct case being confined to foreign real property this tool does not record.
- **no financial year is ever closed** — there is no lodgement marker, and every tax figure is computed live from the current facts, so editing a prior year's inputs restates figures that may already have been lodged with nothing flagging it (the change is recorded in the audit trail, but you have to go looking — save the Annual Tax Report as a PDF at lodgement and compare against it).
