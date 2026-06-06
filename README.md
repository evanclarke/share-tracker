# share-tracker

A personal Australian share portfolio tracker with a REST JSON API. Records trades, dividends, and trust distributions, then produces portfolio and tax reports aligned with Australian tax rules (CGT discount, franking credits, AMIT/AMMA).

## Features

- **Trade recording** — buys, sells, and dividend reinvestment plan (DRP) acquisitions, with automatic settlement date calculation per exchange
- **Income recording** — dividends and trust distributions with full Australian tax component breakdown (franked/unfranked amounts, foreign source income, franking credits, conduit foreign income, TFN withholding, LIC capital gain deductions)
- **DRP reinvestment** — enrol holdings in a Dividend Reinvestment Plan over dated enrolment periods (enrol, unenrol, re-enrol), then turn a distribution into a linked DRP trade; reinvestability is checked as at the distribution's ex date, and leftover cash that can't buy a whole share is carried forward to the next reinvestment in the period or paid out, per the period — unenrolling pays out the trailing carried residual
- **AMIT/AMMA support** — annual tax statements for Attribution Managed Investment Trusts (AMITs), with cost base adjustments applied per purchase parcel
- **Parcel-level CGT** — explicit parcel allocations link sell trades to the parcels they came from; cost bases are pro-rated and AMIT-reduced at the parcel level
- **Return of capital (CGT event G1)** — record a company's non-assessable payment as a corporate action; the per-unit amount reduces the cost base of every parcel held on the payment date across all reports, and a payment in excess of a parcel's cost base becomes a capital gain in the net capital gain report (the cost base floors at nil — G1 never produces a loss)
- **Share splits and consolidations (TD 2000/10)** — record a conversion of a listing's shares into a larger or smaller number as a corporate action; no CGT event arises: parcels keep their total cost base and original acquisition date (the 12-month discount clock keeps running) while quantities and per-unit cost bases are re-based across the conversion in every report and in the Sell/trade capacity checks (a post-split sale allocates post-split units against pre-split parcels)
- **Bonus issues (non-assessable)** — record a bonus share issue as a corporate action; the ATO apportions each parcel's cost base over the original + bonus shares and the bonus shares take the original acquisition date, so the issue is the same no-CGT-event quantity re-base as a split: parcels grow by `bonus/held` per unit with their total cost base and acquisition date untouched (bonus shares received *in lieu of a dividend* are assessed as a dividend and entered as a DRP trade instead)
- **Rights issues** — record a rights issue (free rights to acquire new shares at a set price) as a corporate action, then exercise it: the exercise creates a new Buy parcel **acquired on the exercise date** (the 12-month discount clock runs from exercise, not from the rights or the original shares) with a cost base of the exercise payment plus any amount paid to acquire the rights; cumulative exercised units are capped at the entitlement the holding earned at the record date (selling or lapsing the rights themselves is not modelled; see `docs/rights-issues.md`)
- **Off-market share buy-backs** — record the buy-back offer (per-unit price, the dividend component of that price with its franking credit, and the market value had the buy-back not been proposed) as a corporate action, then sell units into it: the participation atomically creates the Sell at the **capital proceeds** per unit (`max(price, market value) − dividend`, per the ATO's market-value rule) with the chosen parcel allocations, plus the dividend component as franked income with its credits — so the CGT and dividend sides land in the right reports with no special casing; a listed-company buy-back announced after 25 Oct 2022 has no dividend component and the whole price is capital proceeds (see `docs/share-buy-backs.md`)
- **Takeovers and mergers (scrip-for-scrip rollover)** — record an all-scrip takeover (every `old` units of the original listing become `new` units of the replacement listing) as a corporate action, then exchange it: the exchange atomically closes every open parcel of the original listing through a provenance-marked Sell — the rollover **disregards the capital gain**, so the disposal never reaches the realised-gains or net-capital-gain reports — and creates one replacement parcel per consumed parcel carrying its **remaining reduced cost base** and its **acquisition date** (the combined holding period counts toward the 12-month discount, and a non-AUD cost base keeps its original AUD translation). Takeovers without rollover, partial cash consideration, and multiple replacement share classes are not modelled (see `docs/takeovers-and-scrip-for-scrip.md`)
- **Portfolio overview** — open holdings per security with total cost base and optional market value (supply current prices in the request body)
- **Unrealised gains report** — per-holding gain/loss and CGT-discount-eligible quantity as at a given date
- **Realised gains report** — per-sale capital gain/loss split into discount-eligible (parcels held strictly more than 12 months), non-discountable, and loss buckets
- **Net capital gain report** — the overall CGT position per financial year: combines realised parcel gains with AMMA-attributed CGT gains and capital losses, applies losses ATO-optimally (non-discountable gains first), carries unused net capital losses forward across years (seeded by an enterable opening carried-forward loss), and applies the 50% discount to produce the assessable net capital gain
- **Tax summary** — income aggregated by Australian financial year (July–June), combining dividends, trust distributions, and AMMA components; franking credits are reported as claimable only, applying the 45-day at-risk holding-period rule (90 days for preference shares, LIFO share identification) with the A$5,000 small-shareholder exemption; the foreign income tax offset is capped at the A$1,000 FITO de-minimis, with the excess surfaced separately
- **Tax-return CSV export** — the tax summary and net capital gain reports download as tax-return-ready CSV (`GET <report>/export`), one record per financial year with the same columns as the JSON response
- **FX rate import** — monthly RBA F11 foreign exchange rates (the rates the ATO directs taxpayers to use) fetched and stored as foreign-per-AUD, refreshed weekly and via a manual trigger
- **AUD conversion** — cost base and proceeds in the portfolio, unrealised, and realised reports are converted to AUD at the ATO reference rate (with a per-trade manual `fx_rate` fallback); see [FX conversion](#fx-conversion)
- **MIC registry import** — the ISO 10383 Market Identifier Code list imported monthly (and via a manual trigger), used by a non-blocking report to flag curated exchanges whose MIC is unknown or expired
- **Settlement-holiday coverage alerting** — exchange holiday calendars are seeded for a finite range of years; auto-calculating a settlement date outside that range logs a warning, and a non-blocking report flags every trade whose settlement window falls outside its exchange's seeded coverage (see [Settlement holiday coverage](#settlement-holiday-coverage))
- **Web UI** — a built-in browser frontend (no build step, served from the same binary) with CRUD screens for every entity, atomic Sell + parcel-allocation entry, DRP reinvestment, and a view for each report; see [Web frontend](#web-frontend)

## Building and running

```bash
cargo build --release
./target/release/share-tracker [--db share-tracker.db] [--host 0.0.0.0] [--port 3000] [--schedule schedule.cron]
```

| Flag | Default | Description |
|------|---------|-------------|
| `--db` | `share-tracker.db` | SQLite database file path |
| `--host` | `0.0.0.0` | IP address to bind. `0.0.0.0` listens on all interfaces (reachable from other machines); use `127.0.0.1` to restrict to localhost |
| `--port` | `3000` | HTTP port to listen on |
| `--schedule` | built-in `schedule.cron` | Path to a cron file overriding the built-in maintenance schedule |

> **Note:** the default `--host 0.0.0.0` makes the server reachable from other machines on the network, and it has no authentication. Run it only on trusted networks, or pass `--host 127.0.0.1` to keep it local.

The database is created automatically on first run. Migrations are applied in order at startup.

### Scheduled maintenance

Recurring maintenance jobs — the database backup, the RBA FX rate import, the ISO MIC registry import, and the currencies import — are scheduled from a cron file rather than hard-coded intervals. Each line is a 5-field Vixie cron expression (`min hour dom mon dow`) followed by a job name; `#` starts a comment. The built-in default is embedded in the binary (`schedule.cron`); pass `--schedule <path>` to use your own file instead:

```
0 0 * * 0   backup          # weekly, Sunday 00:00
0 2 * * 1   rba-fx-import   # weekly, Monday 02:00
0 3 1 * *   mic-import      # monthly, 1st at 03:00 (ISO publishes monthly)
0 4 1 * *   currency-import # monthly, 1st at 04:00 (ISO 4217 + ISO 24165 / DTIF)
```

A schedule line naming an unknown job is rejected at startup; a registered job with no schedule line is allowed but logged as a `WARN` (it will then only run via its endpoint). Jobs run only at their scheduled times (not at startup); after each run (and at startup) the next scheduled run is logged at INFO. The backup writes `<stem>-YYYY-MM-DD-HHMMSS.db` beside the main database file (the date-time component keeps each weekly run distinct; skipped only if a file with that exact name already exists). Any job can be run on demand with `POST /jobs/{name}` (see HTTP API).

Logging is controlled by the `RUST_LOG` environment variable (default: `info`).

## Database schema

```
exchanges
├── mic          TEXT PK          ISO 10383 Market Identifier Code (e.g. XASX)
├── name         TEXT
├── country      TEXT
├── currency     TEXT FK→currencies.code   Default trading currency
├── timezone     TEXT             IANA timezone string
└── settlement_days INTEGER      T+N settlement (e.g. 2 for ASX)

exchange_holidays             Full-closure non-trading days per exchange (settlement skips them)
├── mic          TEXT FK→exchanges.mic   Part of PK
├── holiday_date TEXT             'YYYY-MM-DD'; part of PK
└── name         TEXT             Holiday name (informational)

listings
├── id           INTEGER PK
├── exchange_mic TEXT FK→exchanges.mic
├── ticker       TEXT
├── name         TEXT
├── isin         TEXT (nullable)
├── security_type TEXT           Share | ETF | LIC | Trust
├── currency     TEXT FK→currencies.code
├── amit         BOOLEAN          True if the security is an AMIT
└── preference   BOOLEAN          Preference share: franking credits need 90 (not 45) at-risk days

rba_fx_rates                  RBA F11 monthly FX rates (the rate used for ATO conversion)
├── id           INTEGER PK
├── currency     TEXT             ISO 4217 code (e.g. USD)
├── month        TEXT             'YYYY-MM'
└── rate         TEXT (decimal)   Foreign units per 1 AUD; UNIQUE (currency, month)

mic_registry                  ISO 10383 MIC reference list (validation only; not the operational exchange table)
├── mic           TEXT PK          ISO 10383 Market Identifier Code (e.g. XASX)
├── operating_mic TEXT             Parent operating MIC (== mic for operating entries)
├── name          TEXT             Market name / institution description
├── country_code  TEXT             ISO 3166 alpha-2 country code
├── city          TEXT (nullable)
├── status        TEXT             ISO STATUS: ACTIVE | UPDATED | EXPIRED
└── expiry_date   TEXT (nullable)  'YYYY-MM-DD' when EXPIRED, else NULL

currencies                    Recognised currencies: fiat (ISO 4217) + digital tokens (ISO 24165)
├── code          TEXT PK          ISO 4217 alpha code (fiat) or ISO 24165 DTI (token)
├── kind          TEXT             Fiat | DigitalToken
├── numeric_code  TEXT (nullable)  ISO 4217 numeric code (fiat only)
├── name          TEXT             Currency name (fiat) or token long name
├── short_name    TEXT (nullable)  Token short name / ticker
├── minor_units   INTEGER (nullable)  ISO 4217 minor unit / token decimals; informational only
└── source        TEXT             Iso4217 | Iso24165 (which feed the row came from)

trades
├── id                INTEGER PK
├── trade_type        TEXT         Buy | Sell | DRP
├── date              DATE
├── settlement_date   DATE
├── listing_id        INTEGER FK→listings.id
├── average_price     TEXT (decimal)
├── quantity          TEXT (decimal)
├── currency          TEXT FK→currencies.code
├── brokerage         TEXT (decimal)
├── gst_on_brokerage  TEXT (decimal)
├── brokerage_currency TEXT FK→currencies.code
├── fx_rate           TEXT (decimal)  Manual foreign-per-AUD override; fallback when no ATO rate exists (1.0 for AUD trades)
├── contract_note_ref TEXT (nullable)
├── residual_brought_forward TEXT (decimal)  DRP trades only: leftover cash carried in from the prior reinvestment (else 0)
├── residual_carried_forward TEXT (decimal)  DRP trades only: leftover carried to the next reinvestment (else 0)
├── residual_paid_out        TEXT (decimal)  DRP trades only: leftover paid out instead of carried, incl. the trailing residual refunded at DRP unenrolment (else 0)
├── rights_action_id  INTEGER FK→corporate_actions.id (nullable)  Rights-exercise Buys only: the RightsIssue action exercised, set by POST /corporate_actions/:id/exercise (caps cumulative exercised units at the entitlement; the trade is immutable via PUT /trades and blocks editing/deleting the action)
├── buyback_action_id INTEGER FK→corporate_actions.id (nullable)  Buy-back participation Sells only: the BuyBack action sold into, set by POST /corporate_actions/:id/participate (the trade is immutable via PUT /sells, carries a linked dividend income row removed with it by DELETE /sells, and blocks editing/deleting the action)
├── scrip_action_id   INTEGER FK→corporate_actions.id (nullable)  Scrip-for-scrip exchange trades only (the closing Sell + every replacement Buy): the ScripForScrip action exchanged, set by POST /corporate_actions/:id/exchange. The trades carrying one action id form the exchange group: each is immutable via PUT /sells and PUT/DELETE /trades, DELETE /sells on the closing Sell removes the whole group, and the action is frozen while any exists
└── deemed_acquisition_date TEXT (nullable)  Scrip-for-scrip replacement Buys only: the consumed parcel's acquisition date, carried by the rollover (the combined holding period). Drives the 12-month CGT discount clock and the AUD translation month of the cost base in the reports; split/return-of-capital applicability stays on the actual trade date. NULL = the trade's own date

income
├── id                        INTEGER PK
├── listing_id                INTEGER FK→listings.id
├── date_paid                 DATE
├── ex_date                   DATE (nullable)
├── franked_amount            TEXT (decimal)
├── unfranked_amount          TEXT (decimal)
├── foreign_source_income     TEXT (decimal)
├── foreign_tax_paid          TEXT (decimal)
├── tfn_withholding_tax       TEXT (decimal)
├── franking_credits          TEXT (decimal)
├── lic_capital_gain_deduction TEXT (decimal)
├── conduit_foreign_income    TEXT (decimal)  Excluded from assessable income
├── trust_income              BOOLEAN
├── reinvestment_trade_id     INTEGER FK→trades.id (nullable, for DRP linkage)
├── currency                  TEXT FK→currencies.code   ISO 4217; tax summary converts to AUD by date_paid month (default AUD)
└── buyback_trade_id          INTEGER FK→trades.id (nullable)  Buy-back dividend components only: the participation Sell this row was created with (the row is managed by the participation — PUT/DELETE /income reject it; DELETE /sells on the Sell removes it)

amma_statements              Annual AMIT Member Annual (AMMA) statements
├── id                              INTEGER PK
├── listing_id                      INTEGER FK→listings.id
├── tax_year_end_date               DATE         e.g. 2024-06-30 for FY2024
├── units_held                      TEXT (decimal)
├── date_received                   DATE
├── australian_interest             TEXT (decimal)
├── australian_dividends_unfranked  TEXT (decimal)
├── franked_dividends               TEXT (decimal)
├── franking_credits                TEXT (decimal)
├── net_rent                        TEXT (decimal)
├── foreign_income                  TEXT (decimal)
├── foreign_tax_credits             TEXT (decimal)
├── other_income                    TEXT (decimal)
├── cgt_discount_gains              TEXT (decimal)
├── cgt_indexation_gains            TEXT (decimal)
├── cgt_other_gains                 TEXT (decimal)
├── capital_losses_applied          TEXT (decimal)
├── tax_deferred_amount             TEXT (decimal)  Informational only — not a cost-base driver (reflected in cost_base_adjustment)
├── tax_free_amount                 TEXT (decimal)  Informational only — not a cost-base driver (reflected in cost_base_adjustment)
├── cost_base_adjustment            TEXT (decimal)  Per-unit AMIT cost base net amount; sole cost-base driver (+ reduces, − increases)
├── tfn_withholding_tax             TEXT (decimal)
└── currency                        TEXT FK→currencies.code   ISO 4217; tax summary converts to AUD by tax_year_end_date month (default AUD)

amit_adjustments             Links a purchase parcel to an AMMA statement
├── id                   INTEGER PK
├── amma_statement_id    INTEGER FK→amma_statements.id
├── trade_id             INTEGER FK→trades.id  Must be Buy or DRP
└── quantity             TEXT (decimal)       Units of the parcel covered by the adjustment

parcel_allocations           Links sell parcels to the purchase parcels they consume
├── id                   INTEGER PK
├── sale_trade_id        INTEGER FK→trades.id  Must be Sell
├── purchase_trade_id    INTEGER FK→trades.id  Must be Buy or DRP
└── quantity_allocated   TEXT (decimal)

drp_enrolments               Dated DRP enrolment periods per holding (a holding can enrol, unenrol, and re-enrol)
├── id                   INTEGER PK
├── listing_id           INTEGER FK→listings.id
├── enrolment_date       TEXT   First day of the period (inclusive)
├── unenrolment_date     TEXT (nullable)  Day the unenrolment takes effect (exclusive); NULL = open-ended (currently enrolled)
└── residual_handling    TEXT   CarryForward | PayOut  Leftover-cash policy for the period (default CarryForward)
                         CHECK: unenrolment_date (when set) is after enrolment_date
                         Write-time invariant: a listing's periods must not overlap (so at most one is open)

cgt_settings                 Singleton CGT settings row (CHECK id = 1)
├── id                   INTEGER PK  Always 1 (CHECK-enforced singleton)
└── opening_capital_loss TEXT (decimal)  Net capital loss carried forward from before the first recorded year (AUD, non-negative); starting balance for the net-capital-gain loss chain

corporate_actions            Corporate actions per listing (company returns of capital — CGT event G1 — share splits/consolidations — TD 2000/10 — non-assessable bonus issues, rights issues, off-market buy-backs, and scrip-for-scrip takeovers)
├── id                INTEGER PK
├── action_type       TEXT   ReturnOfCapital | ShareSplit | BonusIssue | RightsIssue | BuyBack | ScripForScrip (CHECK-enforced enum; the extension point for future actions). Per-type CHECKs tie each payload below to its type
├── listing_id        INTEGER FK→listings.id
├── date              TEXT   ReturnOfCapital: payment date — parcels acquired on/before it and still held then are affected. ShareSplit: conversion date — parcels acquired before it are converted (a trade dated on it is already in post-split units). BonusIssue: issue date — parcels acquired before it receive bonus units (a trade dated on it is ex-bonus). RightsIssue: record date — units held before it earn the entitlement (a trade dated on it is ex-rights). BuyBack: the buy-back date — participations are dated on/after it. ScripForScrip: the exchange date — every parcel still open on it is exchanged; the closing Sell and replacement Buys are dated on it
├── amount_per_unit   TEXT (decimal, nullable)  ReturnOfCapital only: per-unit non-assessable payment (positive); reduces affected parcels' cost bases
├── currency          TEXT FK→currencies.code (nullable)  ReturnOfCapital: must match the affected trades' currency (reports fail loudly on a mismatch). RightsIssue: the exercise price's currency. BuyBack: the buy-back price's currency
├── split_new_units   TEXT (decimal, nullable)  ShareSplit only: every split_old_units existing units become split_new_units units (both positive; a consolidation has new < old)
├── split_old_units   TEXT (decimal, nullable)  ShareSplit only: see split_new_units
├── bonus_units       TEXT (decimal, nullable)  BonusIssue only: every bonus_held_units units held receive bonus_units additional units (both positive; a 1-for-10 issue is bonus_units=1 / bonus_held_units=10)
├── bonus_held_units  TEXT (decimal, nullable)  BonusIssue only: see bonus_units
├── rights_units      TEXT (decimal, nullable)  RightsIssue only: every rights_held_units units held at the record date entitle the holder to rights_units new units (both positive; a 1-for-4 issue is rights_units=1 / rights_held_units=4)
├── rights_held_units TEXT (decimal, nullable)  RightsIssue only: see rights_units
├── exercise_price    TEXT (decimal, nullable)  RightsIssue only: per-new-unit price paid on exercise, in currency (positive)
├── buyback_price           TEXT (decimal, nullable)  BuyBack only: per-unit buy-back price in currency (positive)
├── buyback_dividend        TEXT (decimal, nullable)  BuyBack only: per-unit dividend component of the price (non-negative, ≤ the price; 0 for a listed-company buy-back announced after 25 Oct 2022); assessable income, excluded from capital proceeds
├── buyback_franking_credit TEXT (decimal, nullable)  BuyBack only: per-unit franking credit attached to the dividend component (non-negative; 0 when there is no dividend)
├── buyback_market_value    TEXT (decimal, nullable)  BuyBack only: per-unit market value had the buy-back not been proposed (positive); capital proceeds can't be less than it. NULL when the price is at or above market value (the price is used)
├── scrip_listing_id  INTEGER FK→listings.id (nullable)  ScripForScrip only: the replacement listing the original holding converts into (CHECK: differs from listing_id)
├── scrip_new_units   TEXT (decimal, nullable)  ScripForScrip only: every scrip_old_units units of listing_id held at the exchange date become scrip_new_units units of scrip_listing_id (both positive)
└── scrip_old_units   TEXT (decimal, nullable)  ScripForScrip only: see scrip_new_units

attachments                  Supporting documents for an activity; bytes stored in the DB (captured by the weekly backup)
├── id                INTEGER PK
├── trade_id          INTEGER FK→trades.id (nullable, ON DELETE CASCADE)            Owner (exactly one of the three is set)
├── income_id         INTEGER FK→income.id (nullable, ON DELETE CASCADE)            Owner (exactly one of the three is set)
├── amma_statement_id INTEGER FK→amma_statements.id (nullable, ON DELETE CASCADE)   Owner (exactly one of the three is set)
├── filename          TEXT             Original upload filename, preserved for download
├── content_type      TEXT             application/pdf | image/png | image/jpeg (allowlist, CHECK-enforced)
├── byte_size         INTEGER          Size of content in bytes (informational)
├── checksum          TEXT             SHA-256 of content, hex (integrity / duplicate detection)
├── uploaded_at       TEXT             RFC 3339 timestamp the attachment was stored
└── content           BLOB             The file bytes
                       CHECK: exactly one of trade_id / income_id / amma_statement_id is non-null

job_runs                     Last run of each scheduled/on-demand maintenance job (one row per job, upserted each run)
├── name        TEXT PK          Registry job name (e.g. backup, rba-fx-import)
├── started_at  TEXT             RFC 3339 timestamp the run began
├── finished_at TEXT             RFC 3339 timestamp the run ended
├── success     INTEGER          1 if the run succeeded, 0 if it failed
└── error       TEXT (nullable)  Human-readable error when success = 0, else NULL
```

### Relationships

```
exchanges ──< exchange_holidays
exchanges ──< listings ──< trades >──────────────< parcel_allocations
                                \                         /
                                 └──────────────────────-/
                       trades ──< amit_adjustments >──── amma_statements
                       listings ──< amma_statements
                       listings ──< income
                       listings ──< drp_enrolments
                       listings ──< corporate_actions
                       trades (DRP) ──< income (reinvestment_trade_id)
                       corporate_actions (RightsIssue) ──< trades (rights_action_id)
                       corporate_actions (BuyBack) ──< trades (buyback_action_id)
                       corporate_actions (ScripForScrip) ──< trades (scrip_action_id)
                       corporate_actions (ScripForScrip) >── listings (scrip_listing_id)
                       trades (buy-back Sell) ──< income (buyback_trade_id)
                       trades, income, amma_statements ──< attachments (exactly one owner; ON DELETE CASCADE)

currencies ──< exchanges, listings, trades (currency + brokerage_currency), income, amma_statements, corporate_actions
```

Each `attachments` row belongs to exactly one activity via one of three nullable foreign keys (`trade_id` / `income_id` / `amma_statement_id`), with a `CHECK` enforcing that exactly one is set — a real foreign key keeps referential integrity to the owning row, and `ON DELETE CASCADE` removes an activity's attachments when it is deleted. File contents live in the `content` BLOB so the weekly DB backup captures the documents with no separate file store.

`rba_fx_rates` is standalone reference data (no foreign keys); it is looked up by `(currency, month)`. `job_runs` is likewise standalone: it is keyed by the in-code job name (not a foreign key), and each scheduled or manual run upserts the job's row so only its last run is kept. `cgt_settings` is also standalone: a singleton row (`CHECK (id = 1)`) holding the entered opening carried-forward capital loss consumed by the [net capital gain report](#net-capital-gain).

`mic_registry` is standalone reference data (no foreign keys), keyed by `mic`. It is populated from the ISO 10383 list and used only to validate curated `exchanges` (see the [exchange MIC validation report](#exchange-mic-validation)); it is *not* the operational exchange table and carries no currency/timezone/settlement data.

`currencies` is reference data keyed by `code` (it has no outgoing foreign keys). It is populated from the ISO 4217 (SIX Group) and ISO 24165 (DTIF) feeds and seeded with a baseline of common currencies (the seed migration), and is the recognised list that **every** currency code in the model is foreign-keyed to: `exchanges.currency`, `listings.currency`, `trades.currency`, `trades.brokerage_currency`, `income.currency`, `amma_statements.currency`, and `corporate_actions.currency` all reference `currencies.code`, so an unrecognised currency is rejected at write time. `minor_units` is informational only — stored amounts remain arbitrary-precision Decimal and are never rounded to it.

Decimal values are stored as TEXT to preserve arbitrary precision.

## HTTP API

All data endpoints return JSON. Write endpoints accept `Content-Type: application/json`.

### Web frontend

The server also hosts a built-in web UI — a no-build-step single-page app (plain HTML/CSS/JS) embedded in the binary and served from the same origin as the API:

| Method | Path | Description |
|--------|------|-------------|
| `GET` | `/` | The SPA shell (HTML) |
| `GET` | `/static/app.js` | The app bundle (JavaScript) |
| `GET` | `/static/style.css` | Stylesheet (CSS) |

Open `http://localhost:<port>/` in a browser. The app is hash-routed (`#/e/<entity>`, `#/sells`, `#/jobs`, `#/attachments/<owner>/<id>`, `#/r/<report>`) and drives the JSON API below — it provides CRUD screens for every entity (exchanges, listings, trades, income, AMMA statements, AMIT adjustments, DRP enrolments, exchange holidays, CGT settings, corporate actions), a dedicated Sell screen that captures parcel allocations atomically, a DRP reinvest action on income rows, an Exercise action on RightsIssue corporate-action rows (`POST /corporate_actions/:id/exercise`), a Participate action on BuyBack corporate-action rows (`POST /corporate_actions/:id/participate`), an Exchange action on ScripForScrip corporate-action rows (`POST /corporate_actions/:id/exchange`), an Attachments action on each trade/income/AMMA row that uploads, lists, downloads, and deletes its documents, read-only views of the import-managed reference tables (currencies, MIC registry, RBA FX rates, parcel allocations), a Maintenance → Jobs screen that lists the scheduled jobs with each one's last run (when it finished, whether it succeeded, and any error) and runs any of them on demand (`POST /jobs/:name`), and a view for each report (portfolio overview, open parcels, unrealised/realised gains, net capital gain, tax summary, exchange MIC validation, settlement holiday coverage). The net capital gain and tax summary report views carry an **Export CSV** action that downloads the report via its `/export` endpoint.

### Exchanges

| Method | Path | Description |
|--------|------|-------------|
| `GET` | `/exchanges` | List all exchanges |
| `GET` | `/exchanges/:mic` | Get one exchange |
| `PUT` | `/exchanges/:mic` | Create or update an exchange |
| `DELETE` | `/exchanges/:mic` | Delete an exchange |

Seed data includes `XASX` (ASX, T+2) and `XNYS` (NYSE, T+2). `PUT` returns `422` if `currency` is not a recognised code in `currencies`.

### Exchange holidays

Full-closure non-trading days per exchange, keyed by `(mic, holiday_date)`. Settlement-date calculation skips these in addition to weekends (see [Trades](#trades)). Seeded from the published NYSE and ASX calendars for 2024–2027 (extend as later years are published).

Coverage is finite: an exchange's calendar is considered to cover the whole calendar years spanned by its seeded holidays (1 Jan of the earliest holiday's year to 31 Dec of the latest's). Outside that span, settlement calculation degrades to weekend-only skipping — this is never an error, but it is surfaced rather than silent: the write logs a `WARN` and the [Settlement holiday coverage](#settlement-holiday-coverage) report flags the affected trades until the missing years are entered.

| Method | Path | Description |
|--------|------|-------------|
| `GET` | `/exchange_holidays` | List all holidays (ordered by MIC, then date) |
| `GET` | `/exchange_holidays/:mic` | List one exchange's holidays (ordered by date) |
| `GET` | `/exchange_holidays/:mic/:date` | Get one holiday (`:date` is `YYYY-MM-DD`) |
| `PUT` | `/exchange_holidays/:mic/:date` | Create or update a holiday (body: `{ "name": "..." }`) |
| `DELETE` | `/exchange_holidays/:mic/:date` | Delete a holiday |

`PUT` returns `422` if `:mic` is not a known exchange, and `400` if `:date` is not a valid `YYYY-MM-DD` date.

### Listings

| Method | Path | Description |
|--------|------|-------------|
| `GET` | `/listings` | List all listings |
| `GET` | `/listings/:id` | Get one listing |
| `PUT` | `/listings/:id` | Create or update a listing |
| `DELETE` | `/listings/:id` | Delete a listing |

`PUT` returns `422` if `exchange_mic` is not a known exchange or `currency` is not a recognised code in `currencies`. The same currency check applies to the `currency` (and `brokerage_currency`) fields on trades, income, and AMMA writes.

**Ticker or name changes:** a renamed security is the *same* security — record the change by editing the existing listing in place (`PUT /listings/:id` with the same id, new `ticker`/`name`). The listing's `id` is the identity everything references (trades, income, AMMA statements, DRP enrolments, corporate actions), and nothing is keyed by ticker, so the full history — parcels, cost bases, and acquisition dates (the 12-month discount clock) — stays attached across the rename. Don't create a new listing for a renamed security: that would start a second, unrelated history. (A relisting under a new entity via merger/takeover is a different event — a CGT parcel substitution, recorded as a [`ScripForScrip` corporate action](#corporate-actions) — not a rename.)

### RBA FX rates

Monthly foreign exchange rates from the RBA's F11 table, stored as foreign-currency units per 1 AUD (so `AUD = foreign / rate`). Rows are written only by the import, so the resource is read-only via `GET`.

| Method | Path | Description |
|--------|------|-------------|
| `GET` | `/rba_fx_rates` | List all FX rates (ordered by currency, then month) |
| `GET` | `/rba_fx_rates/:id` | Get one FX rate |
| `POST` | `/rba_fx_rates/import` | Trigger an import (see below) |

`POST /rba_fx_rates/import` is idempotent: it inserts new `(currency, month)` rows and leaves existing rows unchanged (re-running creates no duplicates). With an **empty body** it fetches the live RBA F11 CSV; with a **non-empty body** it imports that supplied CSV (useful for retries when the RBA endpoint is unreachable). Returns `200 OK` with `{ "inserted": N, "skipped": M }`, `422` if the feed can't be parsed, or `502 Bad Gateway` if the RBA fetch fails. The same import also runs on the cron schedule as the `rba-fx-import` job (see Jobs).

### MIC registry

The ISO 10383 Market Identifier Code list, imported from the official ISO20022 `ISO10383_MIC.csv`. Reference data only — used to validate curated exchanges; rows are written only by the import, so the resource is read-only via `GET`.

| Method | Path | Description |
|--------|------|-------------|
| `GET` | `/mic_registry` | List all MIC entries (ordered by MIC) |
| `GET` | `/mic_registry/:mic` | Get one MIC entry |
| `POST` | `/mic_registry/import` | Trigger an import (see below) |

`POST /mic_registry/import` upserts every row in the feed in one transaction, tracking the latest ISO publication (a MIC's status/expiry can change), so re-running creates no duplicates and refreshes changed entries. With an **empty body** it fetches the live ISO CSV; with a **non-empty body** it imports that supplied CSV (useful for retries when ISO is unreachable). Returns `200 OK` with `{ "imported": N }`, `422` if the feed can't be parsed, or `502 Bad Gateway` if the ISO fetch fails. The same import also runs on the cron schedule as the `mic-import` job (see Jobs).

### Currencies

The recognised currencies list — fiat (ISO 4217) and digital tokens (ISO 24165) in one table. Rows are written only by the import, so the resource is read-only via `GET`.

| Method | Path | Description |
|--------|------|-------------|
| `GET` | `/currencies` | List all currencies (ordered by code) |
| `GET` | `/currencies/:code` | Get one currency |
| `POST` | `/currencies/import` | Trigger an import (see below) |

`POST /currencies/import` upserts every row from the feed in one transaction (idempotent — re-running creates no duplicates). The feed format is detected from its content: an **ISO 4217 XML** body (the SIX Group "List One") imports fiat currencies, an **ISO 24165 JSON** body (the DTIF registry snapshot) imports digital tokens. With an **empty body** it fetches the live sources: the ISO 4217 list (free), plus the ISO 24165 registry when the `DTI_REGISTRY_USER_ID` / `DTI_REGISTRY_PASSWORD` environment variables are set (the DTIF download requires Basic-auth credentials; the token fetch is skipped with a warning when they are absent, and fiat still imports). Returns `200 OK` with `{ "imported": N }`, `422` if the feed can't be parsed, or `502 Bad Gateway` if a fetch fails. The same import also runs on the cron schedule as the `currency-import` job (see Jobs).

### Jobs

Recurring maintenance jobs scheduled from the cron file (see [Scheduled maintenance](#scheduled-maintenance)). These endpoints inspect the registered jobs and trigger them on demand.

| Method | Path | Description |
|--------|------|-------------|
| `GET` | `/jobs` | List registered jobs (sorted) with each one's last run |
| `POST` | `/jobs/:name` | Run the named job now |

`GET /jobs` returns a JSON array (sorted by job name); each element is `{ "name", "last_started_at", "last_finished_at", "last_success", "last_error" }`. The four `last_*` fields are `null` for a job that has never run; otherwise they carry the RFC 3339 start/finish timestamps, a boolean success flag, and the error text (`null` on success) of the job's most recent run. Every run — scheduled or manual — upserts the job's `job_runs` row, so this reflects the latest run only.

`POST /jobs/:name` runs the job synchronously and returns `204 No Content` on success, `404 Not Found` if no job has that name, or `500 Internal Server Error` if the job fails. Either way the run is recorded (see `GET /jobs`). Registered jobs are `backup`, `rba-fx-import`, `mic-import`, and `currency-import`.

### Trades

| Method | Path | Description |
|--------|------|-------------|
| `GET` | `/trades` | List all trades |
| `GET` | `/trades/:id` | Get one trade |
| `PUT` | `/trades/:id` | Create or update a trade |
| `DELETE` | `/trades/:id` | Delete a trade |

If `settlement_date` is omitted from the PUT body, it is auto-calculated by advancing `date` by `exchange.settlement_days` **business days** — both weekends and the exchange's seeded public holidays (see [Exchange holidays](#exchange-holidays)) are skipped. If the trade's settlement window falls outside the exchange's seeded holiday coverage, the calculation skips weekends only; the write still succeeds but logs a `WARN`, and the trade is flagged by the [Settlement holiday coverage](#settlement-holiday-coverage) report (the same applies to Sells entered via `PUT /sells/:id`).

`PUT /trades/:id` rejects `trade_type: "Sell"` with `422` — Sells must be created via `PUT /sells/:id` (see below) so they are always persisted with a full set of parcel allocations.

Buy/DRP trades carry the same write-time integrity as Sells (validated atomically in a transaction):

- `DELETE /trades/:id` returns `422` if the trade is still referenced — as the purchase parcel of a Sell's allocation, by an AMIT adjustment, as a distribution's reinvestment trade, or by a buy-back dividend income row (`income.buyback_trade_id`) — or if it belongs to a scrip-for-scrip exchange group (`scrip_action_id` set: the group is only ever deleted as a whole, via `DELETE /sells/:id` on its closing Sell) — instead of surfacing the FK error as `500`. Remove the dependants first (e.g. delete the Sell via `DELETE /sells/:id`).
- `PUT /trades/:id` returns `422` if the edit would shrink the trade's `quantity` below what its dependants rely on: the total already allocated out to Sells (each allocation re-based to the parcel's as-acquired units across any [share splits/consolidations or bonus issues](#corporate-actions)), or any linked AMIT adjustment's covered quantity (AMIT adjustment quantities are expressed in the parcel's as-acquired units).
- `PUT /trades/:id` returns `422` if the existing trade is a rights exercise (`rights_action_id` set): its figures were validated against the rights issue's entitlement, which a free-form edit could exceed. Delete it (`DELETE /trades/:id`, which frees the entitlement) and re-exercise instead — see [Corporate actions](#corporate-actions).
- `PUT /trades/:id` returns `422` if the existing trade belongs to a scrip-for-scrip exchange group (`scrip_action_id` set): its figures carry the rollover's cost base and deemed acquisition date, which a free-form edit would corrupt. Delete the group (`DELETE /sells/:id` on its closing Sell) and re-exchange instead — see [Corporate actions](#corporate-actions).

An unreferenced trade edits and deletes freely.

### Income

| Method | Path | Description |
|--------|------|-------------|
| `GET` | `/income` | List all income records |
| `GET` | `/income/:id` | Get one income record |
| `PUT` | `/income/:id` | Create or update an income record |
| `DELETE` | `/income/:id` | Delete an income record |
| `POST` | `/income/:id/reinvest` | Create the DRP reinvestment trade for this distribution (see [DRP reinvestment](#drp-reinvestment)) |

`PUT /income/:id` and `DELETE /income/:id` return `422` for a buy-back dividend-component row (`buyback_trade_id` set): its figures derive from the buy-back's terms and it belongs with its participation Sell. Delete the Sell via `DELETE /sells/:id` (which removes this row too) and re-participate instead — see [Corporate actions](#corporate-actions).

### AMMA statements

| Method | Path | Description |
|--------|------|-------------|
| `GET` | `/amma_statements` | List all AMMA statements |
| `GET` | `/amma_statements/:id` | Get one AMMA statement |
| `PUT` | `/amma_statements/:id` | Create or update an AMMA statement |
| `DELETE` | `/amma_statements/:id` | Delete an AMMA statement |

### AMIT adjustments

| Method | Path | Description |
|--------|------|-------------|
| `GET` | `/amit_adjustments` | List all AMIT adjustments |
| `GET` | `/amit_adjustments/:id` | Get one AMIT adjustment |
| `PUT` | `/amit_adjustments/:id` | Create or update an AMIT adjustment |
| `DELETE` | `/amit_adjustments/:id` | Delete an AMIT adjustment |

Returns `422 Unprocessable Entity` if the referenced trade is not a Buy/DRP, the trade and AMMA statement reference different listings, or the quantity exceeds the trade quantity.

### Attachments

Supporting documents (a trade confirmation / contract note PDF, a dividend statement, an AMMA statement scan) attached to exactly one activity — a Trade, an Income record, or an AMMA Statement. The file bytes are stored in the database (a BLOB), so the weekly DB backup captures the documents with no separate file store. Because the payload is binary, these endpoints depart from the JSON-CRUD convention used elsewhere: upload is `multipart/form-data`, list/get return metadata only, and a dedicated endpoint streams the raw content.

| Method | Path | Description |
|--------|------|-------------|
| `GET` | `/attachments` | List attachment metadata (never the blob); filter by owner with `?trade_id=`, `?income_id=`, or `?amma_statement_id=` |
| `GET` | `/attachments/:id` | Get one attachment's metadata |
| `GET` | `/attachments/:id/content` | Download the raw file bytes (stored `Content-Type` + `Content-Disposition` filename) |
| `POST` | `/attachments` | Upload a file (`multipart/form-data`) |
| `DELETE` | `/attachments/:id` | Delete one attachment |

`POST /attachments` takes a `multipart/form-data` body with the file in a `file` part and **exactly one** owner field — `trade_id`, `income_id`, or `amma_statement_id`. The server computes `byte_size` and the SHA-256 `checksum`, and returns `201 Created` with the stored metadata as JSON. It returns `422 Unprocessable Entity` if no owner or more than one owner is given, the owner id doesn't reference an existing activity, the `file` part is missing, or its content type is outside the allowlist (`application/pdf`, `image/png`, `image/jpeg`); and `413 Payload Too Large` if the file exceeds 25 MB. Deleting the owning Trade / Income / AMMA Statement removes its attachments automatically (`ON DELETE CASCADE`).

### DRP enrolments

Records when each holding reinvests its distributions, as **dated enrolment periods**: `enrolment_date` (inclusive) to `unenrolment_date` (exclusive; omitted = open-ended, i.e. currently enrolled). A holding can start unenrolled, enrol, unenrol, and re-enrol — one row per period, each with its own residual handling.

| Method | Path | Description |
|--------|------|-------------|
| `GET` | `/drp_enrolments` | List all enrolment periods |
| `GET` | `/drp_enrolments/:id` | Get one enrolment period |
| `PUT` | `/drp_enrolments/:id` | Create or update an enrolment period |
| `DELETE` | `/drp_enrolments/:id` | Remove an enrolment period |

```
PUT /drp_enrolments/1
{ "listing_id": 1, "enrolment_date": "2024-01-01", "unenrolment_date": "2025-01-01",
  "residual_handling": "CarryForward" }   // or "PayOut"; defaults to CarryForward if omitted
```

`residual_handling` decides what happens to leftover cash a reinvestment can't spend on whole shares: `CarryForward` adds it to the next reinvestment in the period, `PayOut` records it as paid out.

A listing's periods must not overlap, and at most one may be open at a time — validated atomically at write time (touching periods, where one ends the day the next starts, are allowed). Closing a period (unenrolling) settles its trailing residual: the leftover the period's last reinvestment carried forward is moved to `residual_paid_out` on that DRP trade in the same transaction, since the registry refunds it at termination; it is **not** picked up after a re-enrolment.

Returns `204 No Content`, or `422 Unprocessable Entity` if `listing_id` doesn't reference a listing, the period overlaps another period for the listing (or would be a second open period), or `unenrolment_date` is not after `enrolment_date`.

### CGT settings

A singleton row (the id is always `1`) holding the **opening carried-forward capital loss**: the net capital loss carried forward from years before the first year recorded in the system, so a user migrating mid-history doesn't have to re-enter pre-system loss years. The [net capital gain report](#net-capital-gain) uses it as the starting brought-forward balance when chaining unused losses across years. When no row exists, the opening loss is zero.

| Method | Path | Description |
|--------|------|-------------|
| `GET` | `/cgt_settings` | List the settings row (empty array if never set) |
| `GET` | `/cgt_settings/:id` | Get the settings row (id is always 1) |
| `PUT` | `/cgt_settings/:id` | Set the opening carried-forward capital loss |
| `DELETE` | `/cgt_settings/:id` | Remove the settings row (opening loss reverts to zero) |

```
PUT /cgt_settings/1
{ "opening_capital_loss": "1500.25" }
```

Returns `204 No Content`, or `422 Unprocessable Entity` if the amount is negative or the id is not `1` (the table CHECKs the singleton).

### Corporate actions

Corporate actions recorded against a listing. Six action types are modelled:

- `ReturnOfCapital` — a non-assessable payment from a company (a shareholder-approved return of share capital, CGT event G1; see `docs/cgt-non-assessable-payments.md`). The per-unit payment reduces the cost base of every parcel of the listing held on the payment date (units sold before the payment were not held for it and are unaffected) in the [portfolio](#overview), [open parcels](#open-parcels), [unrealised](#unrealised-gains), and [realised](#realised-gains) reports. Where cumulative payments exceed a parcel's per-unit cost base, the cost base floors at nil and the excess is a capital gain in the payment's income year — G1 never produces a capital loss — reported by the [net capital gain report](#net-capital-gain).
- `ShareSplit` — a share split or consolidation (TD 2000/10; see `docs/share-splits-and-consolidations.md`): on the conversion `date`, every `split_old_units` units of the listing become `split_new_units` units (a 2-for-1 split is new=2/old=1; a 1-for-10 consolidation is new=1/old=10). **No CGT event happens**: the converted parcels keep their total cost base and their original acquisition date (the 12-month discount clock keeps running) — only the unit count, and so the per-unit cost base, changes. Trade rows keep the quantities as originally transacted; the reports and the Sell/trade write-time capacity checks re-base quantities between unit bases (a trade dated on or after the conversion date is already in post-split units, so after a 2-for-1 split a 100-share parcel covers a 200-share sale). Open-holdings reports show quantities in current units (the unrealised report in the units of its `as_of_date`); a `ReturnOfCapital` payment after a split is per post-split unit. A consolidation that doesn't divide a holding evenly keeps the exact fractional quantity (company rounding / cash-in-lieu arrangements are not modelled). AMIT adjustment quantities remain expressed in the parcel's as-acquired units.
- `BonusIssue` — a non-assessable bonus share issue (the general post-1 July 1998 case; see `docs/bonus-shares.md`): on the issue `date`, every `bonus_held_units` units held receive `bonus_units` additional units (a 1-for-10 issue is bonus=1/held=10). **No CGT event happens**: the ATO apportions each parcel's cost base over the original + bonus shares and the bonus shares take the original acquisition date — the same quantity re-base as a `ShareSplit` with new = held + bonus and old = held, and the reports and write-time checks treat it identically (a trade dated on or after the issue date is ex-bonus and receives nothing). Bonus shares received **in lieu of a dividend** (a bonus share plan) are assessed as a dividend — enter those as a distribution plus a DRP reinvestment trade (the new parcel is acquired at the issue date with the dividend as its cost base), not as this action. Partly paid bonus shares and call payments are not modelled.
- `RightsIssue` — rights to acquire new shares, issued free to existing holders (see `docs/rights-issues.md`): on the record `date`, every `rights_held_units` units held entitle the holder to acquire `rights_units` new units at `exercise_price` per unit in `currency` (a 1-for-4 issue is rights=1/held=4; a trade dated on or after the record date is ex-rights). Recording the action changes nothing by itself — the rights' market value is non-assessable non-exempt income on issue. Exercising it (`POST /corporate_actions/:id/exercise`, below) creates the new parcel. Selling or letting the rights themselves lapse (a CGT event on the rights, whose deemed acquisition date is inherited from the original shares), pre-CGT originals, and retail premiums (entered as unfranked dividend income) are not modelled.
- `BuyBack` — an off-market share buy-back (see `docs/share-buy-backs.md`, QC 66049): the company offers to buy shares back directly from holders. On/after the buy-back `date`, each unit bought back is paid `buyback_price` in `currency`, of which `buyback_dividend` is an assessable franked dividend carrying `buyback_franking_credit` per unit (both 0 for a listed-company buy-back announced after 7:30 pm AEDT 25 Oct 2022 — those have no dividend component); `buyback_market_value` is the per-unit market value had the buy-back not been proposed (capital proceeds can't be less than it; omit it when the price is at or above market value). Recording the action changes nothing by itself; participating (`POST /corporate_actions/:id/participate`, below) creates the disposal and the dividend income together. The further adjustments where the participating shareholder is itself a company, and shares held on revenue account, are not modelled.
- `ScripForScrip` — a takeover or merger completed as an all-scrip exchange with scrip-for-scrip rollover (Subdiv 124-M; see `docs/takeovers-and-scrip-for-scrip.md`): on the exchange `date`, every `scrip_old_units` units of the listing become `scrip_new_units` units of `scrip_listing_id` (the replacement listing, which must differ; a 1-for-1 merger is new=1/old=1). Recording the action changes nothing by itself; exchanging (`POST /corporate_actions/:id/exchange`, below) substitutes every open parcel. Takeovers **without** rollover are an ordinary market-value disposal — enter the Sell and Buy manually; partial rollover with a cash component, multiple replacement share classes, pre-CGT originals, and rolling over a capital loss (not permitted by law) are not modelled.

| Method | Path | Description |
|--------|------|-------------|
| `GET` | `/corporate_actions` | List corporate actions |
| `GET` | `/corporate_actions/:id` | Get one corporate action |
| `PUT` | `/corporate_actions/:id` | Create or update a corporate action |
| `DELETE` | `/corporate_actions/:id` | Delete a corporate action |
| `POST` | `/corporate_actions/:id/exercise` | Exercise a `RightsIssue` into a new Buy parcel |
| `POST` | `/corporate_actions/:id/participate` | Sell units into a `BuyBack` (Sell + dividend income, atomic) |
| `POST` | `/corporate_actions/:id/exchange` | Exchange a `ScripForScrip` takeover (closing Sell + replacement parcels, atomic) |

```
PUT /corporate_actions/1
{
  "action_type": "ReturnOfCapital",
  "listing_id": 1,
  "date": "2024-11-30",
  "amount_per_unit": "0.50",
  "currency": "AUD"
}

PUT /corporate_actions/2
{
  "action_type": "ShareSplit",
  "listing_id": 1,
  "date": "2025-03-01",
  "split_new_units": "2",
  "split_old_units": "1"
}

PUT /corporate_actions/3
{
  "action_type": "BonusIssue",
  "listing_id": 1,
  "date": "2025-09-01",
  "bonus_units": "1",
  "bonus_held_units": "10"
}

PUT /corporate_actions/4
{
  "action_type": "RightsIssue",
  "listing_id": 1,
  "date": "2025-10-01",
  "rights_units": "1",
  "rights_held_units": "4",
  "exercise_price": "1.80",
  "currency": "AUD"
}

PUT /corporate_actions/5
{
  "action_type": "BuyBack",
  "listing_id": 1,
  "date": "2025-11-30",
  "buyback_price": "9.60",
  "buyback_dividend": "1.40",
  "buyback_franking_credit": "0.60",
  "buyback_market_value": "10.20",
  "currency": "AUD"
}

PUT /corporate_actions/6
{
  "action_type": "ScripForScrip",
  "listing_id": 1,
  "date": "2026-02-01",
  "scrip_listing_id": 2,
  "scrip_new_units": "2",
  "scrip_old_units": "1"
}
```

Each action type carries exactly its own payload: a `ReturnOfCapital` has `amount_per_unit` + `currency`, a `ShareSplit` has `split_new_units` + `split_old_units`, a `BonusIssue` has `bonus_units` + `bonus_held_units`, a `RightsIssue` has `rights_units` + `rights_held_units` + `exercise_price` + `currency`, a `BuyBack` has `buyback_price` + `buyback_dividend` + `buyback_franking_credit` + an optional `buyback_market_value` + `currency` (the dividend and credit default to 0 when omitted), a `ScripForScrip` has `scrip_listing_id` + `scrip_new_units` + `scrip_old_units` — the other types' columns are null in the table (enforced by CHECKs and the PUT handler), and GET responses omit them, returning only the action's own fields. Returns `204 No Content`, or `422 Unprocessable Entity` when `amount_per_unit` is not positive, a split/bonus/rights/scrip ratio or `exercise_price` is missing or not positive, `buyback_price` is missing or not positive, `buyback_dividend` is negative or exceeds the price, `buyback_franking_credit` is negative or attached to a zero dividend, `buyback_market_value` is not positive, `scrip_listing_id` is missing, unknown, or the same as `listing_id`, the payload mixes the per-type fields, the listing or currency is unknown, or the action type is unrecognised. A payment's `currency` must match the affected trades' currency — the reports never net amounts across currencies and fail loudly (`500`) on a mismatch.

#### Exercising a rights issue

```
POST /corporate_actions/4/exercise
{
  "date": "2025-11-01",
  "units": "250",
  "rights_cost": "0",
  "fx_rate": "1"
}
```

Exercising rights is no CGT event (`docs/rights-issues.md`): the endpoint atomically creates a Buy trade — the new parcel — dated the exercise `date` (the parcel's acquisition date, so **the 12-month CGT discount clock runs from exercise**, not from the rights or the original shares; the company allots the shares, so the settlement date is the exercise date too). The parcel's cost base is the amount paid to exercise (`units × exercise_price`, carried as the trade's quantity × average price) plus `rights_cost` — the total paid to acquire the exercised rights, 0 (the default) for rights issued free — carried on the trade's `brokerage` column (both are components of the single cost base every report computes). `fx_rate` is the optional manual foreign-per-AUD fallback (defaults to 1).

Cumulative exercised units are capped at the entitlement: units held when the record date arrived (trades dated before the action's `date`, re-based to record-date units across any splits/consolidations) × `rights_units / rights_held_units`, with a fractional entitlement rounded **up** to a whole unit (registry practice). The created trade carries `rights_action_id` linking it to the action; to keep the cap honest the trade is immutable via `PUT /trades` (delete it — which frees the entitlement — and re-exercise instead), and the action itself returns `422` on `PUT`/`DELETE` while exercise trades reference it.

Returns `201 Created` with the created trade as JSON, `404 Not Found` if no corporate action has that id, or `422 Unprocessable Entity` if the action is not a `RightsIssue`, `units` is not positive, `rights_cost` is negative, the exercise date precedes the record date, or the exercise would exceed the remaining entitlement.

#### Participating in a buy-back

```
POST /corporate_actions/5/participate
{
  "date": "2025-12-10",
  "units": "1000",
  "allocations": [ { "purchase_trade_id": 1, "quantity_allocated": "1000" } ],
  "fx_rate": "1"          // optional manual foreign-per-AUD override, default 1
}
```

Sells `units` into the `BuyBack`, creating both sides of the split atomically:

- a **Sell trade** dated `date` (the CGT-event date — when the company accepts the application; it may not precede the action's buy-back `date`) for the chosen parcel `allocations` (specific identification — the same shape and write-time invariants as [`PUT /sells/:id`](#sells): allocations must sum exactly to `units` and each parcel must be a valid, not-over-allocated Buy/DRP). Its per-unit price is the **capital proceeds per unit** = `max(buyback_price, buyback_market_value) − buyback_dividend`, so the [realised gains](#realised-gains) and [net capital gain](#net-capital-gain) reports compute the CGT outcome with no special casing. Proceeds are paid by the company, not market-settled, so the settlement date is the participation date.
- an **income row** for the dividend component (`buyback_dividend × units` franked, `buyback_franking_credit × units` credits, paid on `date`), when the price carries one — assessable in the [tax summary](#tax-summary) and subject to the ordinary franking-credit entitlement rules. A zero-dividend buy-back creates no income row.

The created rows carry provenance links (`trades.buyback_action_id`, `income.buyback_trade_id`) that keep the split consistent: the Sell is rejected by `PUT /sells/:id` (`422`) and removed — together with its income row — by `DELETE /sells/:id`; the income row is rejected by `PUT`/`DELETE /income/:id` (`422`); and the action itself returns `422` on `PUT`/`DELETE` while participation trades reference it.

Returns `201 Created` with `{ "trade": …, "income": …|null }` as JSON, `404 Not Found` if no corporate action has that id, or `422 Unprocessable Entity` if the action is not a `BuyBack`, `units` is not positive, the participation date precedes the buy-back date, or a Sell-side invariant fails.

#### Exchanging a scrip-for-scrip takeover

```
POST /corporate_actions/6/exchange
```

Takes no parameters — the action's terms and the holdings at its `date` determine everything. The rollover disregards the capital gain on the original shares and deems the replacement shares acquired *for the cost base of the original interest*, with the combined holding period counting toward the 12-month CGT discount (`docs/takeovers-and-scrip-for-scrip.md`). The exchange therefore creates, in one transaction:

- a **closing Sell** on the original listing dated the exchange date — price 0, with parcel allocations consuming every open parcel (through the same write-time invariants as [`PUT /sells/:id`](#sells)). It carries `scrip_action_id`, which **excludes it from the [realised gains](#realised-gains) and [net capital gain](#net-capital-gain) reports**: the disposal happens, but its gain is disregarded and the zero proceeds never surface as a loss.
- one **replacement Buy** per consumed parcel on the replacement listing, dated the exchange date (so later splits and returns of capital on the replacement listing apply only from then), with quantity = the parcel's remaining units at the exchange date × `scrip_new_units / scrip_old_units`. The parcel's remaining reduced cost base (AMIT- and return-of-capital-adjusted, floored at nil) is carried on the trade's `brokerage` column with a zero price — numerically part of the single cost base every report computes — and the parcel's acquisition date (chained through any earlier exchange) is carried as `deemed_acquisition_date`, which drives the discount clock, the reported acquisition date in the [open parcels report](#open-parcels), and the AUD translation month of the cost base. The parcel's `currency` and manual `fx_rate` fallback carry over too, so a non-AUD parcel's AUD cost base is unchanged by the exchange.

The created trades form the exchange group (`trades.scrip_action_id`): each is rejected by `PUT /sells/:id` and `PUT`/`DELETE /trades/:id` (`422`); `DELETE /sells/:id` on the closing Sell removes the whole group, restoring the pre-exchange holding (refused with `422` while a replacement Buy is consumed by later allocations or AMIT adjustments); and the action itself returns `422` on `PUT`/`DELETE` while the group exists.

Returns `201 Created` with `{ "sell": …, "replacements": […] }` as JSON, `404 Not Found` if no corporate action has that id, or `422 Unprocessable Entity` if the action is not a `ScripForScrip`, it has already been exchanged, nothing of the original listing is held at the exchange date, or the original listing has a trade dated on/after the exchange date (the takeover delisted it — fix the data first).

### DRP reinvestment

```
POST /income/:id/reinvest
{ "reinvestment_price": "1.50", "fx_rate": "0.65", "date": "2024-03-31" }
```

Creates the DRP reinvestment trade for a distribution and links it back (`income.reinvestment_trade_id`) in one transaction. `fx_rate` (default 1) and `date` (default the distribution's `date_paid`) are optional.

Reinvestability is decided as at the distribution's **ex date** (registry practice: DRP participation is fixed at the record date), falling back to `date_paid` when no ex date is recorded. That date must fall inside one of the holding's [enrolment periods](#drp-enrolments) — a distribution dated before enrolment, or in a gap between unenrolment and re-enrolment, is rejected — and the matching period's `residual_handling` applies.

The reinvestable cash — `franked_amount + unfranked_amount + foreign_source_income − foreign_tax_paid − tfn_withholding_tax` (franking credits are notional and excluded) — plus the residual brought forward from the holding's most recent prior DRP trade *within the same enrolment period* is spent on whole shares at `reinvestment_price`. The leftover is carried forward or paid out per the period's `residual_handling` and recorded on the new trade's residual columns. The carried-forward chain never crosses periods: a period's trailing residual is paid out at unenrolment.

Returns `201 Created` with the created trade as JSON, `404 Not Found` if no income record has that id, or `422 Unprocessable Entity` if no enrolment period covers the distribution's ex date (or pay date when no ex date is recorded), the distribution was already reinvested, or `reinvestment_price` is not positive.

### Sells

```
PUT /sells/:id
```

Creates or replaces a Sell trade **together with all of its parcel allocations** in a single transaction. This is the only write path for Sell trades and their allocations, which guarantees that a Sell can never be persisted under- or over-allocated.

Request body — the Sell trade fields (no `trade_type`; it is always `Sell`) plus an `allocations` array:

```json
{
  "date": "2024-06-03",
  "settlement_date": "2024-06-05",
  "listing_id": 1,
  "average_price": "15.00",
  "quantity": "100",
  "currency": "AUD",
  "brokerage": "9.95",
  "gst_on_brokerage": "0.995",
  "brokerage_currency": "AUD",
  "fx_rate": "1",
  "contract_note_ref": null,
  "allocations": [
    { "purchase_trade_id": 1, "quantity_allocated": "100" }
  ]
}
```

`settlement_date` is optional and auto-calculated as for trades. Re-`PUT`ting the same id replaces the Sell row and *all* of its allocations with the submitted set.

Returns `204 No Content` on success, or `422 Unprocessable Entity` if the allocations do not sum exactly to `quantity`, a referenced purchase trade is missing or is not a Buy/DRP, an allocation would over-allocate a purchase parcel, or the existing trade is a buy-back participation Sell or a scrip-for-scrip exchange closing Sell (`buyback_action_id` / `scrip_action_id` set — its figures derive from its action's terms; delete it and re-participate/re-exchange instead, see [Corporate actions](#corporate-actions)). On any failure the whole transaction is rolled back — nothing is persisted. Allocation quantities are in the sale date's unit basis: the over-allocation check re-bases them across any [share splits/consolidations or bonus issues](#corporate-actions) between the purchase and the sale, so after a 2-for-1 split a 100-share parcel covers a 200-share sale.

```
DELETE /sells/:id
```

Deletes a Sell trade and all of its parcel allocations in one transaction, freeing the purchase parcels those allocations had consumed. A buy-back participation Sell also takes its linked dividend-component income row (`income.buyback_trade_id`) with it, so the capital and dividend sides are always removed together. A scrip-for-scrip exchange closing Sell takes the exchange's replacement Buys (`trades.scrip_action_id`) with it, restoring the pre-exchange holding. Returns `204 No Content` on success, `404 Not Found` if no trade has that id, or `422 Unprocessable Entity` if the id refers to a trade that is not a Sell (use `DELETE /trades/:id` for Buy/DRP trades) or a replacement Buy of the exchange group is still consumed by later allocations or AMIT adjustments (remove those first).

### Parcel allocations

Parcel allocations are **read-only** over HTTP; they are created and replaced atomically with their Sell trade via `PUT /sells/:id`. Allowing standalone writes would let a Sell become under-covered (e.g. by deleting or shrinking an allocation).

| Method | Path | Description |
|--------|------|-------------|
| `GET` | `/parcel_allocations` | List all parcel allocations |
| `GET` | `/parcel_allocations/:id` | Get one parcel allocation |

`PUT` and `DELETE` on these paths return `405 Method Not Allowed`.

### Portfolio reports

#### FX conversion

Reports take the Australian-tax view, so every non-AUD trade amount is converted to AUD before it is aggregated. The rate is the ATO reference rate — the RBA F11 monthly rate (foreign units per 1 AUD) for the amount's currency and the month of the relevant trade date — so `AUD = foreign / rate`. AUD amounts pass through unchanged. When no ATO rate has been imported for that `(currency, month)`, the trade's manual `fx_rate` is used as a fallback; the ATO rate takes precedence once available. If neither is available the report fails loudly (`500`) rather than leaving an amount unconverted. Cost base and proceeds in the portfolio, unrealised, and realised reports are converted this way. Income and AMMA amounts are also converted in the tax summary, using each record's `currency` and the month of `date_paid` (income) or `tax_year_end_date` (AMMA); these records have no manual `fx_rate`, so a non-AUD amount with no ATO rate fails loudly (`500`) rather than being passed through unconverted.

#### Overview

```
POST /portfolio/overview
```

Returns open holdings per listing. Request body (optional):

```json
{ "prices": { "<listing_id>": "<price>" } }
```

Response fields per holding: `listing_id`, `quantity`, `avg_cost_base_per_unit`, `total_cost_base`, `current_price` (nullable), `market_value` (nullable).

Cost base is calculated as `(price × quantity + brokerage + GST) − AMIT reductions`, pro-rated to remaining (unsold) units, less [return-of-capital](#corporate-actions) payments received on those units — flooring at nil (CGT events E10 and G1) — then converted to AUD (see [FX conversion](#fx-conversion)). Supplied prices are expected in AUD, so `market_value` is AUD. The unrealised-gains report computes its cost base the same way. `quantity` is in *current* units — re-based across any [share splits/consolidations or bonus issues](#corporate-actions) — so it lines up with a current market price; the re-basing never changes the cost base totals.

#### Open parcels

```
GET /portfolio/open-parcels
```

Returns every open parcel — a Buy/DRP trade whose quantity is not fully consumed by parcel allocations — the per-parcel cost-base schedule to reconcile against a broker statement and the input to a sell decision (the [overview](#overview) aggregates the same parcels per listing). Response fields per parcel: `trade_id`, `listing_id`, `ticker`, `acquisition_date`, `original_quantity`, `remaining_quantity` (units not yet allocated to a Sell), `original_cost_base` (price × quantity + brokerage + GST for the whole parcel), `amit_cost_base_reduction` (cumulative AMIT reductions to date — the full amount, even where CGT event E10 has floored the cost base), `return_of_capital_reduction` (cumulative [return-of-capital](#corporate-actions) payments received on the remaining units since acquisition — likewise the full amount, even where CGT event G1 has floored the cost base), and `remaining_cost_base` (`max(original − AMIT, 0)` pro-rated to the remaining units, less the return-of-capital payments on those units, floored at nil). All monetary fields are AUD, converted at the parcel's buy-month rate (see [FX conversion](#fx-conversion)). `remaining_quantity` is in *current* units — re-based across any [share splits/consolidations or bonus issues](#corporate-actions) so it reconciles with a broker statement — while `original_quantity` stays as transacted; `acquisition_date` is preserved across a split or bonus issue (TD 2000/10; `docs/bonus-shares.md`). A [scrip-for-scrip](#corporate-actions) replacement parcel reports the consumed parcel's acquisition date (the rollover's combined holding period) and carries its remaining reduced cost base; its monetary fields convert at the *original* acquisition month's rate, so the AUD cost base is unchanged by the exchange.

Sorted by `listing_id`, then `acquisition_date`, then `trade_id`.

#### Unrealised gains

```
POST /portfolio/unrealised-gains
```

Request body (all optional):

```json
{ "prices": { "<listing_id>": "<price>" }, "as_of_date": "YYYY-MM-DD" }
```

`as_of_date` defaults to today. Response fields per holding: `listing_id`, `quantity`, `total_cost_base`, `current_price`, `market_value`, `unrealised_gain_loss`, `cgt_discount_eligible_quantity` (units from parcels held strictly more than 12 months as at `as_of_date`). `total_cost_base` is in AUD (see [FX conversion](#fx-conversion)); supplied prices are expected in AUD, so `market_value` and `unrealised_gain_loss` are AUD. Quantities are in the unit basis of `as_of_date` — re-based across any [share splits/consolidations or bonus issues](#corporate-actions) up to that date — and neither a split nor a bonus issue restarts the 12-month discount clock (the converted/bonus shares keep the original acquisition date; TD 2000/10, `docs/bonus-shares.md`). A [scrip-for-scrip](#corporate-actions) replacement parcel's discount clock likewise runs from its deemed (carried) acquisition date — the rollover's combined holding period.

#### Realised gains

```
GET /portfolio/realised-gains
```

Returns one record per sale trade that has at least one parcel allocation. Response fields: `sale_trade_id`, `listing_id`, `sale_date`, `proceeds`, `cost_base`, `capital_gain_loss`, `discount_eligible_gain` (gross gain from parcels held strictly more than 12 months), `non_discountable_gain` (gross gain from parcels held 12 months or less — the "other" method), and `capital_loss` (total losses from allocations sold below cost, as a positive amount). The three buckets satisfy `capital_gain_loss == discount_eligible_gain + non_discountable_gain − capital_loss`. `proceeds`, `cost_base`, and `capital_gain_loss` are in AUD: proceeds are converted at the sale's FX rate and cost base at the purchase's FX rate (see [FX conversion](#fx-conversion)). The cost base of the sold units is reduced by [return-of-capital](#corporate-actions) payments received while they were held — from acquisition up to the sale date — flooring at nil; payments after the sale don't touch them. An allocation's quantity is in the sale date's unit basis: a [share split/consolidation or bonus issue](#corporate-actions) between purchase and sale re-bases it back to as-acquired units for the cost-base pro-rating, and the discount holding period still runs from the original acquisition date (TD 2000/10; `docs/bonus-shares.md`). A [scrip-for-scrip](#corporate-actions) exchange's closing Sell is **excluded** — the rollover disregards its gain — and a sale of a replacement parcel uses the carried cost base (converted at the original acquisition month's rate) with the discount clock running from the deemed (carried) acquisition date, the rollover's combined holding period.

Sorted by `sale_date` ascending.

#### Net capital gain

```
GET /portfolio/net-capital-gain
```

Returns one record per Australian financial year (identified by the calendar year of 30 June), sorted ascending — the overall CGT position combining realised parcel gains with the CGT components attributed on AMMA statements. Realised gains are attributed by the sale's tax year (July = next FY); AMMA components by `tax_year_end_date`. A [scrip-for-scrip](#corporate-actions) exchange contributes nothing in the exchange year — the rollover disregards the gain; the deferred gain surfaces when the replacement parcel is eventually sold.

The assessable net capital gain is computed the ATO way:

1. Total the year's gross capital gains, split into **discount-eligible** (realised parcels held > 12 months + AMMA discount-method gains grossed up ×2 — the AMMA `cgt_discount_gains` value is the already-halved "discounted capital gain", so doubling it restores the gross gain + any **CGT event E10/G1** gain whose parcel was held > 12 months at the event date) and **non-discountable** (realised parcels held ≤ 12 months + AMMA indexation-method and other-method gains, neither of which gets the discount + any CGT event E10/G1 gain held ≤ 12 months).
2. Total the year's capital losses: realised losses + AMMA `capital_losses_applied`, **plus the net capital loss brought forward from earlier years** — unused losses chain across the year series indefinitely (per the ATO), starting from the entered [opening carried-forward loss](#cgt-settings) (losses from before the first recorded year).
3. Apply losses against non-discountable gains first, then discount-eligible gains (taxpayer-favourable: the 50% discount falls on the largest possible remaining gain). Losses always apply before the discount.
4. **Net capital gain** = remaining non-discountable gain + 50% of the remaining discount-eligible gain. Unused losses are carried forward into the next year in the series.

**CGT event E10**: when the cumulative AMIT cost base reductions (`amit_adjustments` × the AMMA per-unit `cost_base_adjustment`) on a parcel exceed its cost base, the cost base is floored at nil (in the portfolio, unrealised, and realised reports) and the excess is a capital gain in the income year the reducing AMMA statement applies to — added to the gain buckets above (discount-eligible vs not, per the holding period as at the statement's `tax_year_end_date`). The excess is converted to AUD at the parcel's buy-month rate. See `docs/amit-cost-base-adjustments.md`.

**CGT event G1**: when a company's cumulative [return-of-capital](#corporate-actions) payments exceed a parcel's per-unit cost base, the cost base is floored at nil and the excess is a capital gain in the payment's income year — covering only the units still held at the payment date, and never producing a capital loss. The gain is added to the gain buckets above (discount-eligible vs not, per the holding period as at the payment date) and converted to AUD at the payment month's ATO rate (no manual fallback: a non-AUD payment with no rate fails loudly with `500`). See `docs/cgt-non-assessable-payments.md`.

Response fields: `tax_year`, `discount_eligible_gains`, `other_gains`, `capital_losses` (all gross; `capital_losses` is only the losses arising that year), `capital_loss_brought_forward` (unused losses chained from earlier years, seeded by the `cgt_settings` opening balance), `net_discount_eligible_gain` and `net_other_gain` (after losses), `cgt_discount` (the 50% reduction applied = `net_discount_eligible_gain / 2`), `net_capital_gain`, `capital_loss_carried_forward` (losses left unused after offsetting all gains — the next year's brought-forward balance), `cgt_event_e10_gain`, and `cgt_event_g1_gain` (informational: gross E10/G1 gains already included in the gain buckets). All amounts are AUD (AMMA amounts converted via the ATO rate for the month of `tax_year_end_date`, so a non-AUD amount with no rate fails loudly with `500`; see [FX conversion](#fx-conversion)).

```
GET /portfolio/net-capital-gain/export
```

The same per-year records as a downloadable tax-return-ready CSV (`Content-Type: text/csv; charset=utf-8`, `Content-Disposition: attachment; filename="net-capital-gain.csv"`): a header row naming the columns (the response fields above, in that order), then one record per financial year. An empty report still returns the header row.

#### Tax summary

```
GET /portfolio/tax-summary
```

Returns one record per Australian financial year (identified by the calendar year of 30 June), sorted ascending. Aggregates dividend income by `date_paid` (July = next FY) and AMMA statements by `tax_year_end_date`. All amounts are converted to AUD via the ATO rate (see [FX conversion](#fx-conversion)) before aggregating, using each record's `currency` and the month of `date_paid` (income) or `tax_year_end_date` (AMMA). Response fields include all income and AMMA components as separate fields for direct transfer to a tax return.

**Franking-credit entitlement** (the at-risk holding-period rule, `docs/you-and-your-shares-dividends.md`): `franking_credits` reports only *claimable* credits. In a year whose total attached credits (income + AMMA) reach A$5,000, each dividend's shares must have been held at risk for at least 45 days — 90 for a listing flagged `preference` — not counting the acquisition or disposal day; which shares were sold is identified **last-in first-out** (as the ATO mandates for this rule), regardless of the CGT parcel allocation chosen on the sale. Credits on entitled units that fail the test are reported in `franking_credits_denied` and excluded from `franking_credits`. Below A$5,000 the small-shareholder exemption applies and nothing is denied. The test anchors on the income record's `ex_date` (falling back to `date_paid` when absent); AMMA-attributed credits count toward the threshold but are never themselves denied (an annual AMMA statement carries no per-distribution ex-date).

**Foreign income tax offset (FITO) cap** (`docs/fito-limit.md`): `foreign_tax_offsets` (income `foreign_tax_paid` + AMMA `foreign_tax_credits`, in AUD) reports the offset claimable without the ATO's offset-limit calculation — up to the A$1,000 de-minimis per year. A year's foreign tax above A$1,000 is reported in `foreign_tax_offset_excess` and excluded from `foreign_tax_offsets`: the limit calculation needs the taxpayer's full income-tax position (employment income, deductions, Medicare levy), which is outside this system's data, so the excess is claimable only to the extent the taxpayer's own offset-limit calculation supports it.

```
GET /portfolio/tax-summary/export
```

The same per-year records as a downloadable tax-return-ready CSV (`Content-Type: text/csv; charset=utf-8`, `Content-Disposition: attachment; filename="tax-summary.csv"`): a header row naming the columns (the response fields, in field order, from `tax_year` through `tfn_withholding_tax`), then one record per financial year. An empty report still returns the header row.

#### Exchange MIC validation

```
GET /reports/exchange_mic_validation
```

Validates each curated exchange's MIC against the `mic_registry` (the imported ISO 10383 list) — **non-blocking**: writes to `exchanges` are never rejected, this only surfaces MICs worth a second look. Returns one record per exchange (sorted by MIC) with fields: `mic`, `exchange_name`, `registry_status` (`ok` = active in the registry, `expired` = present but EXPIRED, `unknown` = no registry entry, i.e. a typo or the registry hasn't been imported yet), `iso_status` (raw ISO `ACTIVE`/`UPDATED`/`EXPIRED`, or null when unknown), and `expiry_date`. With an empty registry every exchange is `unknown`.

#### Settlement holiday coverage

```
GET /reports/settlement_holiday_coverage
```

Flags every trade whose `[date, settlement_date]` window is not fully inside its exchange's seeded holiday coverage (see [Exchange holidays](#exchange-holidays)) — **non-blocking**: trade writes are never rejected, this only surfaces settlement dates that may have been computed against an incomplete calendar (weekend-only skipping). Returns one record per affected trade (sorted by ticker, then date, then trade id) with fields: `trade_id`, `listing_id`, `ticker`, `mic`, `trade_type`, `date`, `settlement_date`, `coverage_status` (`outside_holiday_coverage` = the window extends beyond the seeded years, `no_holiday_coverage` = the exchange has no seeded holidays at all), and the exchange's coverage span `coverage_start`/`coverage_end` (1 Jan of the earliest seeded holiday's year to 31 Dec of the latest's; null when there is no coverage). Trades fully inside coverage are omitted — an empty report means every settlement window was computed against a complete calendar. Entering the missing holiday years clears the corresponding alerts.

## Response codes

| Code | Meaning |
|------|---------|
| `200 OK` | Successful GET (JSON; the report `/export` endpoints return `text/csv`, an attachment content download returns its stored content type) |
| `201 Created` | DRP reinvestment trade created via `POST /income/:id/reinvest`, a rights-exercise trade created via `POST /corporate_actions/:id/exercise`, a buy-back participation (Sell + dividend income) created via `POST /corporate_actions/:id/participate`, a scrip-for-scrip exchange (closing Sell + replacement parcels) created via `POST /corporate_actions/:id/exchange`, or an attachment uploaded via `POST /attachments` |
| `204 No Content` | Successful PUT or DELETE, or a job run via `POST /jobs/:name` |
| `400 Bad Request` | Malformed path parameter (e.g. an `exchange_holidays` `:date` that is not `YYYY-MM-DD`) |
| `404 Not Found` | Resource does not exist |
| `405 Method Not Allowed` | Write attempted on a read-only path (e.g. `parcel_allocations`) |
| `413 Payload Too Large` | Uploaded attachment exceeds the 25 MB per-file limit |
| `422 Unprocessable Entity` | Business rule or constraint violation (e.g. over-allocation, wrong trade type, under-allocated Sell, deleting or shrinking a Buy/DRP that a parcel allocation, AMIT adjustment, or reinvestment link still relies on, unparseable FX or MIC feed, a write referencing an unrecognised currency / unknown exchange / listing, an attachment upload with no/multiple owners or an unsupported content type, a negative / non-singleton `cgt_settings` opening capital loss, an overlapping or empty DRP enrolment period, reinvesting a distribution no enrolment period covers, or a corporate action with a non-positive `amount_per_unit`, a missing/non-positive split/bonus/rights ratio, exercise price, or buy-back price, a buy-back dividend that is negative or exceeds the price, a franking credit without a dividend, a non-positive market value, a payload mixing the per-type fields, or an unrecognised `action_type`; a rights exercise that is not against a RightsIssue, has non-positive units or a negative rights cost, is dated before the record date, or exceeds the remaining entitlement; a buy-back participation that is not against a BuyBack, has non-positive units, is dated before the buy-back date, or fails a Sell-side invariant; a scrip-for-scrip exchange that is not against a ScripForScrip, is already exchanged, has nothing held, or whose original listing traded on/after the exchange date — or a ScripForScrip whose replacement listing is missing, unknown, or the same as the original; editing a rights-exercise trade, a buy-back participation Sell, a buy-back dividend income row, or any scrip-for-scrip exchange trade, deleting an exchange trade individually or an exchange group whose replacement parcels are still drawn on, or editing/deleting a RightsIssue, BuyBack, or ScripForScrip that exercise/participation/exchange trades still reference) |
| `500 Internal Server Error` | Unexpected database error, or a job triggered via `POST /jobs/:name` failed |
| `502 Bad Gateway` | Upstream fetch failed (e.g. the RBA FX or ISO MIC import could not reach its source) |

## Tech stack

- **Rust** (edition 2024)
- **axum 0.8** — HTTP framework
- **sqlx 0.8** — async SQLite driver with compile-time migration support
- **SQLite** with WAL journal mode and foreign key enforcement
- **rust_decimal** — arbitrary-precision decimal arithmetic for all monetary values
- **tokio** — async runtime
- **reqwest** — HTTP client for fetching the RBA F11 FX rate CSV
- **chrono / chrono-tz** — date handling
