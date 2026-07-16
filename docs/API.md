# HTTP API

The REST JSON API of [share-tracker](../README.md). The tables behind it are documented in [SCHEMA.md](SCHEMA.md).

All data endpoints return JSON. Write endpoints accept `Content-Type: application/json`.

## Web frontend

The server also hosts a built-in web UI — a no-build-step single-page app (plain HTML/CSS/JS, shipped as native ES modules) embedded in the binary and served from the same origin as the API:

| Method | Path | Description |
|--------|------|-------------|
| `GET` | `/` | The SPA shell (HTML) |
| `GET` | `/static/app.js` | The app entry module: the rendering engine and router (JavaScript) |
| `GET` | `/static/config.js` | The entity/report/action configuration the engine renders (JavaScript) |
| `GET` | `/static/forms.js` | Field constructors and form wiring (JavaScript) |
| `GET` | `/static/util.js` | Shared utilities: API client, formatting, decimal arithmetic (JavaScript) |
| `GET` | `/static/style.css` | Stylesheet (CSS) |

Open `http://localhost:<port>/` in a browser. The app is hash-routed (`#/e/<entity>`, `#/sells`, `#/jobs`, `#/attachments/<owner>/<id>`, `#/r/<report>`) and drives the JSON API below — it provides CRUD screens for every entity (exchanges, listings, holding accounts, trades, income, investment expenses, AMMA statements, AMIT adjustments, DRP enrolments, exchange holidays, CGT settings, corporate actions), a dedicated Sell screen that captures parcel allocations atomically, a Transfers screen that moves parcels between holding accounts (`PUT /transfers/:id`) and deletes a transfer to restore the pre-transfer holding, a simple-first income form (payment amount + franking selector mapped onto the component fields, the per-share cross-check pair with a live computed-product hint, and a "Reinvested under DRP" tick that chains the reinvest call after the save, with the full tax-component field set behind an advanced toggle), a DRP reinvest action on income rows (with an Undo reinvest action on reinvested rows driving `DELETE /income/:id/reinvest`), an Exercise and a Sell rights action on RightsIssue corporate-action rows (`POST /corporate_actions/:id/exercise` / `POST /corporate_actions/:id/sell_rights` — the latter with an anchoring-parcel allocation editor; recorded sales are listed under a delete-only Rights Sales view whose Delete undoes the sale and frees the entitlement), a Participate action on BuyBack corporate-action rows (`POST /corporate_actions/:id/participate`), an Exchange action on ScripForScrip corporate-action rows (`POST /corporate_actions/:id/exchange`), a Demerge action on Demerger corporate-action rows (`POST /corporate_actions/:id/demerge`), a Recognise action on WorthlessShares corporate-action rows (`POST /corporate_actions/:id/recognise`), an ESS Statements screen with a Vest action on unvested rows that creates the cost-base-reset Buy for vested shares (`POST /ess_statements/:id/vest`; a vested row shows its linked Buy instead), an Attachments action on each trade/income/AMMA/ESS-statement/interest-income row that uploads, lists, downloads, and deletes its documents (a trade created from another record — a DRP trade, an ESS vest Buy, a buy-back Sell — also lists that record's [linked documents](#attachments), labelled with their owner; delete stays on the owner's view), read-only views of the import-managed reference tables (currencies, MIC registry, RBA FX rates, parcel allocations), a Maintenance → Jobs screen that lists the scheduled jobs with each one's last run (when it finished, whether it succeeded, and any error), expands each job to its stored run history (the newest 20 runs, so an intermittent failure is diagnosable), and runs any of them on demand (`POST /jobs/:name`), and a view for each report (portfolio overview, open parcels, unrealised/realised gains, performance, net capital gain, tax summary, exchange MIC validation, settlement holiday coverage). The net capital gain and tax summary report views carry an **Export CSV** action that downloads the report via its `/export` endpoint. A **Snapshots** view lists the stored [report snapshots](#report-snapshots) with stale and provisional ones badged, opens any day's stored rows, generates/regenerates a day on demand, offers **Regenerate all** and **Regenerate provisional** bulk-repair buttons (`POST /report_snapshots/regenerate_all` / `regenerate_provisional`), and graphs market value and unrealised gain over time as an inline-SVG time series (no chart library — the no-build-step rule holds) with stale points hollow and provisional points dash-ringed.

**Health banner:** every view carries a cross-page warning strip driven by `GET /reports/health` (see [Health](#health)). It appears only when something needs attention — stale closing prices, stale RBA FX rates, or a job whose latest run failed — names the problem, and links to the Jobs page; it refreshes on each navigation, so fixing the cause (e.g. re-running the failed job) clears it. A failing price or FX import is therefore visible from any screen, not only when the Jobs page happens to be opened.

**Names, never raw ids:** every foreign-key id shown in the UI renders the referenced row's name, not the bare number — in entity-list tables, report tables, `<select>` option labels, the post-record action pages, and the toast that confirms a created row. A listing shows as `MIC:TICKER` (`Crypto:TICKER` for crypto), a holding account by its name, a trade/parcel as a side/quantity/listing/date description (e.g. "DRP 45 XASX:VDHG on 2024-12-20"), and an AMMA statement as its listing + tax year. The raw id stays reachable on the cell's tooltip and appears only as secondary detail (e.g. "Reinvested into DRP 45 XASX:VDHG on 2024-12-20 (trade #12)"). This is display-only — the JSON API is unchanged and still keyed by id.

**Human-friendly headings:** every heading, table column header, and form field label reads as a human name, never the raw database/JSON field name — `amount_per_security` shows as "Amount per security", `fx_rate` as "FX rate", `exchange_mic` as "Exchange", `holding_account_id` as "Account". A field with no explicit label is humanised by default: a trailing `_id` is dropped (the cell already shows the referenced row's name, so `listing_id` → "Listing"), the snake_case becomes sentence case, and known acronyms keep their canonical casing (AUD, FX, MIC, DRP, CGT, AMIT, GST, LIC, FITO) rather than "Aud"/"Fx"/"Drp". The always-AUD report aggregates carry an "(AUD)" qualifier (e.g. "Market value (AUD)"); per-row entity tables get none, because their amounts are in the row's own currency. This is the labelling counterpart to *Names, never raw ids* (that fixed raw id values; this fixes raw field names in the headers around them) and is likewise display-only and config-driven — keyed by column name, so a column reused on a new screen inherits its heading automatically.

**Amounts round, rates don't:** in every table (entity lists, the Sells/Transfers lists, and report tables) a numeric column is classified by name as a monetary amount, a per-unit rate, or a quantity. Monetary amounts are shown rounded to the cent (2 decimal places, half away from zero) with thousands grouping (e.g. `123,476.78`); per-unit rates (a trade's average price, an income amount-per-security, a DRP reinvestment price, FX rates, crypto/closing prices) and quantities keep their full entered precision, because rounding a rate would break statement reconciliation; a derived per-unit figure a report computes (e.g. average cost base per unit) shows at least 4 decimal places rather than cent-rounded. This is display-only and exact (decimal-string arithmetic, never floating point): the JSON API and the CSV exports still return full-precision decimals, and the underlying value drives column sorting (filters match the displayed text). When rounding a money cell drops precision, the full value is shown on its hover tooltip. A new screen inherits the rule automatically — the classification is keyed by column name, shared across the API.

## Exchanges

| Method | Path | Description |
|--------|------|-------------|
| `GET` | `/exchanges` | List all exchanges |
| `GET` | `/exchanges/:mic` | Get one exchange |
| `PUT` | `/exchanges/:mic` | Create or update an exchange |
| `DELETE` | `/exchanges/:mic` | Delete an exchange |

Seed data includes `XASX` (ASX, T+2) and `XNYS` (NYSE, T+2). `PUT` returns `422` if `currency` is not a recognised code in `currencies`. `close_time` (`HH:MM` local in the exchange's `timezone`, default `16:00`) is the end of the regular trading session: [closing-price collection](#closing-prices) only stores a day's price once it has passed.

## Exchange holidays

Full-closure non-trading days per exchange, keyed by `(mic, holiday_date)`. Settlement-date calculation skips these in addition to weekends (see [Trades](#trades)). Seeded from the published NYSE and ASX calendars for 2019–2027 (extend as later years are published).

Coverage is finite: an exchange's calendar is considered to cover the whole calendar years spanned by its seeded holidays (1 Jan of the earliest holiday's year to 31 Dec of the latest's). Outside that span, settlement calculation degrades to weekend-only skipping — this is never an error, but it is surfaced rather than silent: the write logs a `WARN` and the [Settlement holiday coverage](#settlement-holiday-coverage) report flags the affected trades until the missing years are entered.

| Method | Path | Description |
|--------|------|-------------|
| `GET` | `/exchange_holidays` | List all holidays (ordered by MIC, then date) |
| `GET` | `/exchange_holidays/:mic` | List one exchange's holidays (ordered by date) |
| `GET` | `/exchange_holidays/:mic/:date` | Get one holiday (`:date` is `YYYY-MM-DD`) |
| `PUT` | `/exchange_holidays/:mic/:date` | Create or update a holiday (body: `{ "name": "..." }`) |
| `DELETE` | `/exchange_holidays/:mic/:date` | Delete a holiday |

`PUT` returns `422` if `:mic` is not a known exchange, and `400` if `:date` is not a valid `YYYY-MM-DD` date.

## Listings

| Method | Path | Description |
|--------|------|-------------|
| `GET` | `/listings` | List all listings |
| `GET` | `/listings/:id` | Get one listing |
| `PUT` | `/listings/:id` | Create or update a listing |
| `DELETE` | `/listings/:id` | Delete a listing |

`PUT` returns `422` if `exchange_mic` is not a known exchange or `currency` is not a recognised code in `currencies`. The same currency check applies to the `currency` (and `brokerage_currency`) fields on trades, income, and AMMA writes.

**Crypto listings:** a crypto asset held as an investment is a CGT asset like any share (`docs/ato/crypto-cgt.md`), recorded as a listing with `security_type: "Crypto"` and **no `exchange_mic`** (omit it or send null — a crypto asset trades on no MIC-coded venue). Its `ticker` must be a recognised digital-token code in [`currencies`](#currencies) (kind `DigitalToken` — the seeded BTC/ETH codes or anything the ISO 24165 import recognises, matched on the DTI code or short name). Exchange-less listings are unique by ticker. `PUT` returns `422` for an unrecognised token ticker, a Crypto listing *with* an exchange, a non-Crypto listing *without* one, or a duplicate exchange-less ticker. Trades and Sells on a Crypto listing auto-populate `settlement_date` as the trade date itself (same-day settlement — no T+n, no holiday calendar, no coverage warning), and crypto parcels flow through every report exactly like share parcels (AUD cost base/proceeds, the 12-month 50% discount, loss netting, holding-account transfers). Crypto closing prices are collected daily at the UTC-midnight cut-off into the [closing-price history](#closing-prices) (which feeds the [report snapshots](#report-snapshots)); ad-hoc report requests can still supply their own prices.

**Ticker or name changes:** a renamed security is the *same* security — record the change by editing the existing listing in place (`PUT /listings/:id` with the same id, new `ticker`/`name`). The listing's `id` is the identity everything references (trades, income, AMMA statements, DRP enrolments, corporate actions), and nothing is keyed by ticker, so the full history — parcels, cost bases, and acquisition dates (the 12-month discount clock) — stays attached across the rename. Don't create a new listing for a renamed security: that would start a second, unrelated history. (A relisting under a new entity via merger/takeover is a different event — a CGT parcel substitution, recorded as a [`ScripForScrip` corporate action](#corporate-actions) — not a rename.)

## Holding accounts

Custody/location accounts within the one taxpayer — e.g. an employer share-plan account holding RSU-vested shares alongside a personal broker account — so the same listing can be held in several places at once with different treatment (notably [DRP enrolment](#drp-enrolments)). Account 1 (`Default`) is seeded by the migrations: every write that omits `holding_account_id` lands in it, so single-account users never see the dimension.

| Method | Path | Description |
|--------|------|-------------|
| `GET` | `/holding_accounts` | List all holding accounts |
| `GET` | `/holding_accounts/:id` | Get one holding account |
| `PUT` | `/holding_accounts/:id` | Create or rename a holding account |
| `DELETE` | `/holding_accounts/:id` | Delete a holding account |

`PUT` returns `422` for a duplicate `name` (UNIQUE). `DELETE` returns `422` if the account still holds data (trades, income, AMMA statements, DRP enrolment periods, or a transfer references it) or is the seeded default account — move or remove the data first.

## RBA FX rates

Monthly foreign exchange rates from the RBA's F11 table, stored as foreign-currency units per 1 AUD (so `AUD = foreign / rate`). Rows are written only by the import, so the resource is read-only via `GET`.

| Method | Path | Description |
|--------|------|-------------|
| `GET` | `/rba_fx_rates` | List all FX rates (ordered by currency, then month) |
| `GET` | `/rba_fx_rates/:id` | Get one FX rate |
| `POST` | `/rba_fx_rates/import` | Trigger an import (see below) |

`POST /rba_fx_rates/import` is idempotent: it inserts new `(currency, month)` rows and leaves existing rows unchanged (re-running creates no duplicates). With an **empty body** it fetches the live RBA F11 CSV; with a **non-empty body** it imports that supplied CSV (useful for retries when the RBA endpoint is unreachable). Returns `200 OK` with `{ "inserted": N, "skipped": M }`, `422` if the feed can't be parsed, or `502 Bad Gateway` if the RBA fetch fails. An import that inserted new rows also **trues up** the [provisional report snapshots](#report-snapshots) in the same run — regenerating each one, so a snapshot valued at a fallback-month rate is finalised the moment its real rate arrives — and the response then carries the regeneration summary as `"snapshot_true_up": { "regenerated": […], "blocked": […] }` (absent when nothing was inserted). The same import (and true-up) also runs on the cron schedule as the `rba-fx-import` job (see Jobs).

## MIC registry

The ISO 10383 Market Identifier Code list, imported from the official ISO20022 `ISO10383_MIC.csv`. Reference data only — used to validate curated exchanges; rows are written only by the import, so the resource is read-only via `GET`.

| Method | Path | Description |
|--------|------|-------------|
| `GET` | `/mic_registry` | List all MIC entries (ordered by MIC) |
| `GET` | `/mic_registry/:mic` | Get one MIC entry |
| `POST` | `/mic_registry/import` | Trigger an import (see below) |

`POST /mic_registry/import` upserts every row in the feed in one transaction, tracking the latest ISO publication (a MIC's status/expiry can change), so re-running creates no duplicates and refreshes changed entries. With an **empty body** it fetches the live ISO CSV; with a **non-empty body** it imports that supplied CSV (useful for retries when ISO is unreachable). Returns `200 OK` with `{ "imported": N }`, `422` if the feed can't be parsed, or `502 Bad Gateway` if the ISO fetch fails. The same import also runs on the cron schedule as the `mic-import` job (see Jobs).

## Currencies

The recognised currencies list — fiat (ISO 4217) and digital tokens (ISO 24165) in one table. Rows are written only by the import, so the resource is read-only via `GET`.

| Method | Path | Description |
|--------|------|-------------|
| `GET` | `/currencies` | List all currencies (ordered by code) |
| `GET` | `/currencies/:code` | Get one currency |
| `POST` | `/currencies/import` | Trigger an import (see below) |

`POST /currencies/import` upserts every row from the feed in one transaction (idempotent — re-running creates no duplicates). The feed format is detected from its content: an **ISO 4217 XML** body (the SIX Group "List One") imports fiat currencies, an **ISO 24165 JSON** body (the DTIF registry snapshot) imports digital tokens. With an **empty body** it fetches the live sources: the ISO 4217 list (free), plus the ISO 24165 registry when the `DTI_REGISTRY_USER_ID` / `DTI_REGISTRY_PASSWORD` environment variables are set (the DTIF download requires Basic-auth credentials; the token fetch is skipped with a warning when they are absent, and fiat still imports). Returns `200 OK` with `{ "imported": N }`, `422` if the feed can't be parsed, or `502 Bad Gateway` if a fetch fails. The same import also runs on the cron schedule as the `currency-import` job (see Jobs).

## Closing prices

Daily closing-price history per listing, in the **listing's quote currency** (never AUD-converted — reports convert via the [FX rules](#fx-conversion) at read time). Rows are collected automatically by the scheduled `price-import` job (see Jobs): after each exchange's `close_time`, it stores the close of every trading day in the **last 7 trading days** (per held listing, one provider call spanning the window) whose stored row is missing or errored — so a day lost to a host or provider outage is backfilled by the following runs instead of becoming a permanent hole. Trading days only (weekends and the exchange's seeded [holidays](#exchange-holidays) store no row and are not an error), and days already stored ok are never re-fetched, so runs are idempotent. Exchange-less ([Crypto](#listings)) listings trade continuously, so their daily cut-off is **UTC midnight**: the stored price for date *D* is the daily candle completing at 00:00 UTC at the end of *D* (~10–11 am Sydney the next morning).

The provider behind the pluggable fetcher is **Yahoo Finance** (unofficial chart API, via the `yfinance-rs` crate): free and keyless, covering ASX (`.AX` suffix), NYSE/Nasdaq (plain ticker), and crypto (`<TICKER>-<currency>`) in one source. A **failed fetch is stored as an errored row** (`status: "error"`, `price: null`, `error` text) — never silently missing — and is replaced by a later successful re-fetch. Yahoo serves float32-precision values, so prices are rounded to 7 significant digits before storing. Prices for other exchanges need a symbol mapping added to the fetcher first (until then their fetches store errored rows naming the exchange).

| Method | Path | Description |
|--------|------|-------------|
| `GET` | `/closing_prices` | List stored prices, newest first, **including errored rows**; filter with `?listing_id=`, `?from=`, `?to=` (dates inclusive) |
| `POST` | `/closing_prices/fetch` | Re-fetch one day for one listing (body: `{ "listing_id": 1, "price_date": "YYYY-MM-DD" }`) |
| `POST` | `/closing_prices/backfill` | Backfill a listing over a date range (body: `{ "listing_id": 1, "from": "...", "to": "..." }`) |

`POST /closing_prices/fetch` replaces whatever is stored for that (listing, day) — its purpose is re-running a failed fetch once the provider recovers. It returns `200` with the freshly stored row (which is itself errored if the provider failed again), `404` for an unknown listing, or `422` if the day's close is not final yet or the date is not a trading day.

`POST /closing_prices/backfill` fills a listing's history (e.g. after importing an old trade: trade date to today): trading days only, days already stored ok are skipped, and the missing days are fetched in one provider call — an expected trading day the provider returns no candle for (e.g. a historical holiday outside the seeded calendar) is stored as an errored row. The `to` date is clamped to the latest complete trading day. Returns `200` with `{ "trading_days", "already_stored", "fetched_ok", "errored" }`, `404` for an unknown listing, or `422` if `from` is after `to` or the range contains no complete trading day.

## Report snapshots

Stored daily results of the three price-dependent reports — [portfolio overview](#overview), [unrealised gains](#unrealised-gains), and [performance](#performance) — one stored row per (report, date). The scheduled `report-snapshot` job (see Jobs) runs daily after the last relevant close and **catches up over a bounded window**: it computes the latest calendar date every held listing can be valued at with final prices (typically yesterday, once the prior NYSE close and the crypto UTC-midnight cut-off are in), then generates every missing snapshot date — including a hole an earlier blocked date left behind — from the series' first stored snapshot, capped at 14 calendar days back, up to that latest date, and regenerates any stale or provisional snapshot in the window. A date still blocked (a missing or errored price) is skipped with its blocker in the job's failure detail and retried on later runs, so a late price *delays* that date's snapshot instead of losing it. Each listing is valued at its nearest trading day on or before the snapshot date — a weekend or holiday date uses the prior close — with the stored quote-currency price converted to AUD per the [FX rules](#fx-conversion).

A snapshot records the report *as at its date*: facts dated after the snapshot date (trades, sales, income, corporate actions, AMIT adjustments by their statement's year end) are excluded, and recording a **back-dated fact** marks every snapshot dated on or after it **stale** — atomically with the fact write, via database triggers no write path can bypass. A stale snapshot keeps returning its stored rows, flagged, until regenerated (the job regenerates stale window dates itself); regeneration re-runs the report with the stored prices and the new facts. A day on which any held listing's price fetch failed (or was never fetched) has **no snapshot at all** — missing, distinguishable from stale — until the price re-run or backfill succeeds and the day is generated on demand or by the job's window.

**Provisional snapshots:** the RBA publishes a month's FX rate only after the month ends, so a current-month snapshot of a non-AUD holding cannot use its own month's rate. Instead of failing, generation values it at the most recent earlier month's rate (at most 2 months back — beyond that it fails loudly) and stores the snapshot flagged **`provisional`**, with the affected report rows carrying `fx_provisional: true`. Provisional is distinct from stale: the facts are right, the rate is an interim one. When an [FX import](#rba-fx-rates) lands new rates it regenerates every provisional snapshot in the same run (the true-up), and the daily job regenerates provisional window dates too — a regeneration whose conversions all used real-month rates clears the flag. Only these valuation paths ever use a fallback-month rate; tax calculations and FY reports keep the strict [FX rules](#fx-conversion).

| Method | Path | Description |
|--------|------|-------------|
| `GET` | `/report_snapshots` | List stored snapshots (metadata only: `report`, `snapshot_date`, `generated_at`, `stale`, `provisional`), oldest first; filter with `?report=`, `?from=`, `?to=` |
| `GET` | `/report_snapshots/series` | The graphable time series: per snapshot date, the portfolio's AUD totals (`market_value`, `total_cost_base`, `unrealised_gain`) plus the `stale` and `provisional` flags, oldest first |
| `GET` | `/report_snapshots/:report/:date` | One snapshot's metadata plus its stored report rows (exactly what the live report returned at generation) |
| `POST` | `/report_snapshots/generate` | Generate — or regenerate a stale/provisional — day's snapshots (body: `{ "date": "YYYY-MM-DD" }`; omit the date for the latest fully-valuable day) |
| `POST` | `/report_snapshots/regenerate_all` | Regenerate every stored snapshot date (bulk repair after back-dated edits); per-date blockers are reported, unblocked dates still regenerate |
| `POST` | `/report_snapshots/regenerate_provisional` | Regenerate only the provisional snapshot dates — the manual counterpart of the post-import true-up |

`report` is `portfolio_overview`, `unrealised_gains`, or `performance`. `POST /report_snapshots/generate` returns `200` with the three stored snapshots' metadata, or `422` with the blocker detail when the day cannot be trusted: a held listing's price is missing (backfill it) or errored (re-fetch it), the date's close is not final yet, an FX-rate gap too old for the valuation fallback, or nothing was held on the date. The two regeneration endpoints return `200` with `{ "regenerated": ["YYYY-MM-DD", …], "blocked": [{ "date", "reason" }, …] }` — a blocked date is reported there rather than failing the request, and its stored snapshot is left as it was.

## Jobs

Recurring maintenance jobs scheduled from the cron file (see [Scheduled maintenance](../README.md#scheduled-maintenance)). These endpoints inspect the registered jobs and trigger them on demand.

| Method | Path | Description |
|--------|------|-------------|
| `GET` | `/jobs` | List registered jobs (sorted) with each one's run history |
| `POST` | `/jobs/:name` | Run the named job now |

`GET /jobs` returns a JSON array (sorted by job name); each element is `{ "name", "last_started_at", "last_finished_at", "last_success", "last_error", "runs" }`. `runs` is the job's stored run history, most recent first; each entry is `{ "started_at", "finished_at", "success", "error" }` — the RFC 3339 start/finish timestamps, a boolean success flag, and the error text (`null` on success). Every run — scheduled or manual — appends a `job_runs` row, and the same write prunes that job's history to the newest 20 runs, so an intermittent failure that later succeeded stays diagnosable here without unbounded growth. The four `last_*` fields duplicate the newest entry for at-a-glance reading; they are `null` (and `runs` empty) for a job that has never run.

`POST /jobs/:name` runs the job synchronously and returns `204 No Content` on success, `404 Not Found` if no job has that name, or `500 Internal Server Error` if the job fails. Either way the run is recorded (see `GET /jobs`). Runs of the same job are serialised: a trigger that overlaps an in-flight run of that job (scheduled or manual) waits for it to finish and then runs — the same job never executes concurrently. Registered jobs are `backup`, `rba-fx-import` (which also trues up provisional [report snapshots](#report-snapshots) when new rates land), `mic-import`, `currency-import`, `price-import` (see [Closing prices](#closing-prices); scheduled per market close — each run re-attempts, per held listing, every trading day in the last 7 whose stored row is missing or errored, never re-fetching a day stored ok, so the runs are idempotent and are each other's outage backfill), and `report-snapshot` (see [Report snapshots](#report-snapshots); daily after the second price import, backfilling missing dates and regenerating stale/provisional ones over its 14-day window).

The `backup` job writes a timestamped `VACUUM INTO` copy of the database, **verifies** it (the produced file must pass `PRAGMA integrity_check` and carry the same applied migrations as the live database), and then **prunes** the backup destination to the retention policy — the newest 8 backups plus the first backup of each of the 12 most recent months; only files matching the backup naming pattern are ever deleted (see [Scheduled maintenance](../README.md#scheduled-maintenance)). A backup that fails verification is quarantined as `<name>.db.bad` and the run fails: `POST /jobs/backup` returns `500` and the verification failure is recorded as the run's `last_error` in `GET /jobs`. A verified backup whose pruning step fails also fails the run (the backup file itself is good and keeps its name) — so a `500` from this endpoint always means the Jobs page has the reason.

## Trades

| Method | Path | Description |
|--------|------|-------------|
| `GET` | `/trades` | List all trades |
| `GET` | `/trades/:id` | Get one trade |
| `PUT` | `/trades/:id` | Create or update a trade |
| `DELETE` | `/trades/:id` | Delete a trade |

If `settlement_date` is omitted from the PUT body, it is auto-calculated by advancing `date` by `exchange.settlement_days` **business days** — both weekends and the exchange's seeded public holidays (see [Exchange holidays](#exchange-holidays)) are skipped. If the trade's settlement window falls outside the exchange's seeded holiday coverage, the calculation skips weekends only; the write still succeeds but logs a `WARN`, and the trade is flagged by the [Settlement holiday coverage](#settlement-holiday-coverage) report (the same applies to Sells entered via `PUT /sells/:id`). A trade on an exchange-less ([Crypto](#listings)) listing settles **same-day** instead: the auto-populated `settlement_date` is the trade date, with no holiday lookup and no coverage warning.

`PUT /trades/:id` rejects `trade_type: "Sell"` with `422` — Sells must be created via `PUT /sells/:id` (see below) so they are always persisted with a full set of parcel allocations.

`PUT /trades/:id` likewise rejects `trade_type: "DRP"` with `422` — DRP trades are created only via `POST /income/:id/reinvest` (see [Income](#income)), which links the reinvested shares back to their funding distribution (`income.reinvestment_trade_id`) and threads the residual carry-forward chain. A free-form DRP would be an orphan parcel (no income link, zero residuals) that could shadow that chain, and a re-`PUT` of a reinvest-created DRP would silently zero its residual columns; both are refused. A **`Buy` body targeting a reinvest-created DRP** is refused with `422` too — the distribution's link lives on the income row, so without the guard the write would silently re-type the trade and wipe its residual chain while the income row keeps pointing at it. Undo the reinvestment via its distribution (`DELETE /income/:id/reinvest`, see [DRP reinvestment](#drp-reinvestment)) and re-reinvest instead. (The endpoint still creates and edits plain `Buy` trades.)

`holding_account_id` is optional on the body and defaults to the seeded default account (1) — see [Holding accounts](#holding-accounts). The same default applies to the `holding_account_id` field on income, AMMA statement, DRP enrolment, and Sell writes, and on the rights-exercise, sell-rights, and buy-back participation operations.

Brokerage can be entered GST-inclusive, as broker statements quote it: set `brokerage_includes_gst: true` and put the inclusive amount in `brokerage` — the server splits it at write time (`gst_on_brokerage` = amount × 1/11 rounded to the cent, half away from zero; `brokerage` keeps the exact remainder, so the stored pair always sums back to the amount paid). Any `gst_on_brokerage` supplied alongside the flag is ignored — deriving it is the point. **Reads present the same shape writes expect (lossless round-trip):** with the flag set, `GET /trades` and `GET /trades/:id` return `brokerage` as the one GST-inclusive amount — the stored ex-GST brokerage and GST recombined, exactly what was entered — with `gst_on_brokerage` carrying the derived GST component (informational on reads; a flagged write ignores it). `brokerage` therefore means the GST-inclusive amount **on both reads and writes** whenever the flag is set, and `PUT`ting a `GET` response body back verbatim re-splits the same amount to the **identical stored pair** — a GET→edit→PUT client never shifts the figures. This holds for flagged Sells too (read via `GET /trades/:id`, written via `PUT /sells/:id`). With the flag off (the default), `brokerage` is ex-GST, `gst_on_brokerage` is entered manually, and reads return the stored values as-is.

An optional `statement_total` (decimal, in the brokerage currency) cross-checks the entry against the broker statement's net transaction total: when supplied it must equal `quantity × price + brokerage + GST` (the amount payable; compared numerically, so `1009.95` matches `1009.9500`) either exactly or rounded to the cent (half away from zero — contract notes print the consideration cent-rounded, so `1,302 × 37.585914 + 9.50 = 48,946.360028` accepts the note's `48,946.36`), and a mismatch beyond that sub-cent rounding is rejected with `422` whose body carries the figure the trade computes to. It may only be supplied when the trade and brokerage currencies match — `422` otherwise; no FX conversion is invented for it. The stored value is informational-only: no report or calculation uses it. Both fields apply to Sells too (see [Sells](#sells)), where the total is the **net proceeds**, `quantity × price − brokerage − GST`. Operation-created trades (DRP reinvestment, rights exercise, buy-back participation, scrip exchange, demerger, transfer, worthless-shares recognise) never carry the flag or a total.

An optional `spot_fx_rate` (decimal, foreign units per 1 AUD — the same convention as `fx_rate`) records the transaction-date spot rate as a deliberate override: when set it wins over the imported monthly RBA rate everywhere the trade's amounts convert to AUD (see [FX conversion](#fx-conversion) for when the ATO says to use it). It is rejected with `422` when non-positive or supplied on an AUD-currency trade (an AUD amount never converts, so the override could only be a mistake silently ignored). Absent (`null`), conversion is unchanged: monthly rate first, `fx_rate` fallback. The field applies to Sells too (see [Sells](#sells)); the scrip-for-scrip exchange, demerger, and transfer operations carry a consumed parcel's override onto its replacement Buys so the carried AUD cost base is unchanged.

Core figures are sanity-checked at write time — a degenerate value would silently corrupt every downstream report: `quantity` must be positive, `average_price` / `brokerage` / `gst_on_brokerage` cannot be negative (a zero price is legitimate — e.g. nil proceeds), `fx_rate` must be a positive foreign-per-AUD rate (1 for AUD), `settlement_date` cannot be before `date`, and `date` cannot be before **20 September 1985** — a pre-CGT holding is outside CGT and not modelled, so every report would wrongly compute a capital gain or loss on it (see [Known limitations](#known-limitations)). A violation returns `422` naming the rule. The same checks apply to Sells (see [Sells](#sells)).

Buy/DRP trades carry the same write-time integrity as Sells (validated atomically in a transaction):

- `DELETE /trades/:id` returns `422` if the trade is still referenced — as the purchase parcel of a Sell's allocation, by an AMIT adjustment, as a distribution's reinvestment trade, or by a buy-back dividend income row (`income.buyback_trade_id`) — or if it belongs to a scrip-for-scrip exchange or demerger group (`scrip_action_id` / `demerger_action_id` set: the group is only ever deleted as a whole, via `DELETE /sells/:id` on its closing Sell), it is an [ESS vest Buy](#ess-statements) (`ess_statement_id` set: removed via `DELETE /ess_statements/:id`), or it is a [worthless-shares recognise closing Sell](#recognising-worthless-shares) (`worthless_action_id` set: removed via `DELETE /sells/:id`, which restores the holding) — instead of surfacing the FK error as `500`. Remove the dependants first (e.g. delete the Sell via `DELETE /sells/:id`).
- `PUT /trades/:id` returns `422` if the edit would shrink the trade's `quantity` below what its dependants rely on: the total already allocated out to Sells (each allocation re-based to the parcel's as-acquired units across any [share splits/consolidations or bonus issues](#corporate-actions)), or any linked AMIT adjustment's covered quantity (AMIT adjustment quantities are expressed in the parcel's as-acquired units).
- `PUT /trades/:id` returns `422` if the edit would change the trade's `listing_id` while Sell allocations or AMIT adjustments reference the parcel — accepting it would silently re-associate those dependants to the new listing, costing them cross-listing in the CGT reports. Remove the dependants first (e.g. delete the Sell via `DELETE /sells/:id`), or leave the listing unchanged.
- `PUT /trades/:id` returns `422` if the existing trade is a rights exercise (`rights_action_id` set): its figures were validated against the rights issue's entitlement, which a free-form edit could exceed. Delete it (`DELETE /trades/:id`, which frees the entitlement) and re-exercise instead — see [Corporate actions](#corporate-actions).
- `PUT /trades/:id` and `DELETE /trades/:id` return `422` if the trade is an original parcel anchoring a [rights sale](#selling-or-lapsing-rights) (`rights_sale_allocations.purchase_trade_id`): the sale's record-date anchoring caps were validated against its date and quantity. Delete the rights sale (`DELETE /rights_sales/:id`), make the change, then re-enter it.
- `PUT /trades/:id` returns `422` if the existing trade belongs to a scrip-for-scrip exchange or demerger group (`scrip_action_id` / `demerger_action_id` set): its figures carry the rollover's cost base and deemed acquisition date, which a free-form edit would corrupt. Delete the group (`DELETE /sells/:id` on its closing Sell) and re-exchange/re-demerge instead — see [Corporate actions](#corporate-actions).
- `PUT /trades/:id` and `DELETE /trades/:id` return `422` if the existing trade belongs to a holding-account transfer group (`transfer_id` set, or the crypto network-fee disposal Sell linked via `transfers.fee_sale_trade_id`) — the group only ever changes as a whole, via `DELETE /transfers/:id` (see [Transfers](#transfers)) — or it is an [ESS vest Buy](#ess-statements) (`ess_statement_id` set): edit by deleting the [ESS statement](#ess-statements) and re-vesting.

An unreferenced trade edits and deletes freely.

## Income

| Method | Path | Description |
|--------|------|-------------|
| `GET` | `/income` | List all income records |
| `GET` | `/income/:id` | Get one income record |
| `PUT` | `/income/:id` | Create or update an income record |
| `DELETE` | `/income/:id` | Delete an income record |
| `POST` | `/income/:id/reinvest` | Create the DRP reinvestment trade for this distribution (see [DRP reinvestment](#drp-reinvestment)) |
| `DELETE` | `/income/:id/reinvest` | Undo this distribution's reinvestment — delete the DRP trade and clear the link (see [DRP reinvestment](#drp-reinvestment)) |

`PUT /income/:id` and `DELETE /income/:id` return `422` for a buy-back dividend-component row (`buyback_trade_id` set): its figures derive from the buy-back's terms and it belongs with its participation Sell. Delete the Sell via `DELETE /sells/:id` (which removes this row too) and re-participate instead — see [Corporate actions](#corporate-actions).

**`reinvestment_trade_id` is read-only provenance**, managed solely by the reinvest operation: `POST /income/:id/reinvest` sets it, `DELETE /income/:id/reinvest` clears it. `PUT /income/:id` accepts no such field — a value in the body is ignored (a client can't forge a link to an arbitrary trade), and an edit of a reinvested row **preserves** the existing link rather than clearing it. `DELETE /income/:id` on a reinvested row returns `422` (deleting the distribution alone would orphan its DRP trade): undo the reinvestment first, then delete the row.

**No negative amounts:** every money figure on the row — `franked_amount`, `unfranked_amount`, `foreign_source_income`, `foreign_tax_paid`, `tfn_withholding_tax`, `franking_credits`, `lic_capital_gain_deduction`, `conduit_foreign_income`, and the per-share cross-check pair — is the statement's own positive (or zero) amount. A negative value returns `422` naming the field: it would silently reduce the year's totals in every report. (`tax_deferred_amount` carries the same rule; see below.)

**Per-share cross-check:** a record can optionally carry the statement's per-share figures, `amount_per_security` and `securities_held` (both decimal). They must be supplied together — exactly one present returns `422`. When both are present, the write is validated inside the write transaction: amount_per_security × securities_held, rounded to the cent (half away from zero, matching statements), must equal the gross cash components `franked_amount + unfranked_amount + foreign_source_income` (franking credits are notional and TFN withholding is deducted from — not part of — the gross); a mismatch returns `422` with the computed product in the body. Omitting both skips the check. The stored values are validation/cross-reference only — no report or calculation uses them.

**Entitlement date (trust distributions):** a `trust_income` record can optionally carry `entitlement_date` — the date the holder became presently entitled, usually the distribution period's end printed on the statement. Trust income is assessed in the income year of **present entitlement** regardless of when the cash is paid (ATO QC 23087, `docs/ato/trust-income-timing.md`), so a June distribution paid in mid-July belongs to the FY just ended: when the date is set, the [tax summary](#tax-summary) attributes **every** component of the row (the financial-year bucket and the AUD-conversion month) by it instead of `date_paid`. Absent, `date_paid` behaviour is unchanged. A dividend is assessed when paid or credited, so supplying `entitlement_date` on a non-trust row returns `422` (also CHECK-enforced in the schema). The franking 45-day at-risk test keeps anchoring on `ex_date`/`date_paid` — the at-risk window is about holding the shares — while the A$5,000 small-shareholder threshold year follows the row's assessment year.

**Tax-deferred amount (non-AMIT trust distributions):** a `trust_income` record can optionally carry `tax_deferred_amount` (decimal, ≥ 0) — the statement's tax-deferred amount, which for a non-AMIT unit trust is a CGT event E4 cost-base reduction (`docs/ato/cgt-non-assessable-payments.md`). The field is **informational**: no calculation reads it, and recording it changes nothing by itself — the reduction is entered as a `ReturnOfCapital` [corporate action](#corporate-actions) on the listing, exactly as before. Its purpose is the [E4 cross-check report](#tax-deferred-e4-cross-check), which flags every row whose non-zero amount has no same-FY action, so a faithfully keyed statement can't silently leave the cost base overstated. Supplying it on a non-trust row returns `422` (a company's non-assessable payment is entered as the corporate action directly; also CHECK-enforced in the schema), as does a negative value.

**AMIT cash distributions (cash-only rows):** an income row on a listing whose `amit` flag is set records the fund's **cash distribution advice only**. It drives the cash machinery exactly like any other row — [DRP reinvestment](#drp-reinvestment) (the gross cash less the amounts withheld at source), the per-share cross-check, the ex-date enrolment check — but contributes **nothing** to the [tax summary](#tax-summary)'s income lines: for an AMIT the AMMA statement's attribution is the only assessable record (the cash advice is not a tax document), so counting the cash alongside the [AMMA components](#amma-statements) would double the year's income. The exclusion is whole-row (cash, withholding, offsets). Write-time validation keeps these rows cash-only: `PUT /income/:id` on an AMIT listing returns `422` unless `trust_income` is set (an AMIT is an attribution managed investment *trust*), for a non-zero notional component (`franking_credits`, `lic_capital_gain_deduction`, `conduit_foreign_income` — the fund's attribution belongs on its AMMA statement, where the tax summary reads it), and for a `tax_deferred_amount` (an AMIT's cost-base movement is the AMMA `cost_base_adjustment`, entered as [AMIT adjustments](#amit-adjustments) — CGT event E10, not E4). The cash components (`franked_amount`, `unfranked_amount`, `foreign_source_income`) and the source-withholding fields (`foreign_tax_paid`, `tfn_withholding_tax` — they reduce the DRP-reinvestable cash) stay recordable. The [AMIT cash cross-check report](#amit-cash-cross-check) flags any year whose cash rows have no covering AMMA statement, so cash-only entry can't silently drop a year's income from the return.

## Interest income

Interest income (`docs/ato/tax-return-labels-2026.md`): bank, term-deposit, or broker-cash interest. Interest has no listing, so it is its own entity rather than an [income](#income) record. The [tax summary](#tax-summary) reports an **Australian-source** row's gross as its `interest_income` line (question 10, label 10L), joining the TFN amount withheld to the combined withholding line (10M); a **foreign-source** row (interest-like income from a foreign payer — e.g. a US broker's Treasury liquidity / money-market sweep fund) instead reports as assessable foreign source income on the `foreign_interest_income` line (question 20, label 20E), with its foreign tax withheld joining the FITO line (20O, subject to the A$1,000 de-minimis). Both classifications count in gross assessable investment income.

| Method | Path | Description |
|--------|------|-------------|
| `GET` | `/interest_income` | List all interest income records |
| `GET` | `/interest_income/:id` | Get one interest income record |
| `PUT` | `/interest_income/:id` | Create or update an interest income record |
| `DELETE` | `/interest_income/:id` | Delete an interest income record |

Fields: `date_paid` (its month sets the financial year and the ATO FX conversion month), `amount` (the **gross** interest including any amount withheld — the gross figure is declared), `tfn_withholding_tax` (the TFN amount withheld, 10M — Australian-source rows only), `foreign_source` (boolean, default false: whether the payer is foreign, routing the row to 20E instead of 10L), `foreign_tax_paid` (foreign tax withheld from the gross amount, joining the FITO line — foreign-source rows only), `currency` (defaults to AUD), an optional free-text `source` description (e.g. the bank account — informational only), and an optional `holding_account_id` link for interest paid on a portfolio account such as a broker cash account (NULL for an ordinary bank account; informational only). `PUT` returns `422` for an unrecognised `currency` or `holding_account_id`, a negative `amount`, `tfn_withholding_tax`, or `foreign_tax_paid` (interest figures are the statement's positive — or zero — amounts), a `foreign_tax_paid` on an Australian-source row (foreign tax needs a foreign payer, or the FITO line would claim an offset the row can't support), or a `tfn_withholding_tax` on a foreign-source row (TFN amounts are withheld by Australian investment bodies).

## Investment expenses

Deductible investment expenses (`docs/ato/investment-income-deductions.md`, `docs/ato/dividend-income-deductions.md`): the cost of earning assessable investment income — interest on money borrowed to buy income-producing shares, management/adviser fees, account-keeping fees, and subscriptions. The [tax summary](#tax-summary) nets these against gross assessable investment income per Australian financial year.

| Method | Path | Description |
|--------|------|-------------|
| `GET` | `/investment_expenses` | List all investment expenses |
| `GET` | `/investment_expenses/:id` | Get one investment expense |
| `PUT` | `/investment_expenses/:id` | Create or update an investment expense |
| `DELETE` | `/investment_expenses/:id` | Delete an investment expense |

Fields: `date_incurred` (its month sets the financial year and the ATO FX conversion month), `expense_type` (`LoanInterest` | `ManagementFee` | `AdviceFee` | `AccountKeepingFee` | `Subscription` | `Other`), `amount` (the **deductible amount** — post-apportionment, the figure that goes on the return and the value the tax summary totals), the optional provenance `gross_amount` and `deductible_percentage` (informational only — no calculation reads them), `currency` (defaults to AUD), an optional `description`, and the optional `listing_id` / `holding_account_id` links (both nullable — leave blank for a portfolio-wide expense). `PUT` returns `422` for an unrecognised `currency`, `listing_id`, or `holding_account_id`, or an `expense_type` outside the enum set. Apportionment (joint accounts, private vs income-producing use) is the user's determination, not computed here; brokerage is not recorded here (it forms a trade's CGT cost base) and the LIC capital gain deduction is its own income field.

## AMMA statements

| Method | Path | Description |
|--------|------|-------------|
| `GET` | `/amma_statements` | List all AMMA statements |
| `GET` | `/amma_statements/:id` | Get one AMMA statement |
| `PUT` | `/amma_statements/:id` | Create or update an AMMA statement |
| `DELETE` | `/amma_statements/:id` | Delete an AMMA statement |

`tax_year_end_date` must be a **30 June** date (any year): an AMMA statement attributes a full Australian financial year, and every report that reads it — the [tax summary](#tax-summary), [net capital gain](#net-capital-gain), and franking reports — buckets the statement into the FY named by that date's calendar year. `PUT` returns `422` for any other date, naming the rejected date.

## AMIT adjustments

| Method | Path | Description |
|--------|------|-------------|
| `GET` | `/amit_adjustments` | List all AMIT adjustments |
| `GET` | `/amit_adjustments/:id` | Get one AMIT adjustment |
| `PUT` | `/amit_adjustments/:id` | Create or update an AMIT adjustment |
| `DELETE` | `/amit_adjustments/:id` | Delete an AMIT adjustment |

Returns `422 Unprocessable Entity` if the referenced trade is not a Buy/DRP, the trade and AMMA statement reference different listings **or different holding accounts** (a registry issues one statement per holder account, so a statement only adjusts its own account's parcels), or the quantity exceeds the trade quantity.

## ESS statements

The income side of an employee share scheme interest (`docs/ato/employee-share-schemes.md`): one row per Employee share scheme statement, carrying the Item 12 discount labels declared in the year of the taxing point. The assessable discount reaches the [tax summary](#tax-summary); the [Vesting](#vesting-an-ess-statement) operation creates the cost-base-reset Buy for the vested shares.

| Method | Path | Description |
|--------|------|-------------|
| `GET` | `/ess_statements` | List all ESS statements |
| `GET` | `/ess_statements/:id` | Get one ESS statement |
| `PUT` | `/ess_statements/:id` | Create or update an ESS statement |
| `DELETE` | `/ess_statements/:id` | Delete an ESS statement (and its vest Buy, if any) |
| `POST` | `/ess_statements/:id/vest` | Create the cost-base-reset Buy for this statement (see [Vesting](#vesting-an-ess-statement)) |

Fields: `listing_id`, `holding_account_id` (defaults to the seeded default account), `taxing_point_date`, `quantity` and `market_value_per_share` (the vested shares and their per-share market value — the vest Buy's quantity and price), the discount labels `taxed_upfront_eligible` (D), `taxed_upfront_not_eligible` (E), `deferral_discount` (F), `pre_2009_cessation_discount` (G), the `foreign_source_discount` memo (A — already within the above, surfaced for the FITO calc), `tfn_withholding` (C), and `currency` (defaults to AUD). An unrecognised currency/listing/account is rejected `422`. Responses additionally carry the read-only `vest_trade_id` — the id of the statement's vest Buy, `null` while unvested (derived from the Buy's back-link, ignored if supplied on `PUT`) — which is how the web UI offers the Vest action only on unvested rows.

**Statement-AUD overrides:** a non-AUD statement can additionally carry the employer statement's stated **AUD** figure for each discount label — `aud_taxed_upfront_eligible`, `aud_taxed_upfront_not_eligible`, `aud_deferral_discount`, `aud_pre_2009_cessation_discount`, `aud_foreign_source_discount` (all optional). Employer statements convert at the **release-date spot rate**, while the [tax summary](#tax-summary) otherwise converts foreign-currency amounts at the **RBA monthly rate** — the figures differ, and the ATO-prefilled return carries the employer's. When an override is present the tax summary reports it **verbatim** for that label (including the $1,000 taxed-upfront reduction calculation); absent, the label converts via the RBA rate as before. Supplying an override on an AUD-denominated statement is rejected `422` (its labels are already the AUD figures, and a second figure could silently disagree).

`PUT /ess_statements/:id` returns `422` once the statement has been vested (a vest Buy carries its `ess_statement_id`) **and the edit changes a field the Buy was created from** — `listing_id`, `holding_account_id`, `taxing_point_date`, `quantity`, `market_value_per_share`, or `currency` — which would desync it; delete the statement (which removes the vest) and re-enter to change those. The income-side fields (the discount labels, `tfn_withholding`, the statement-AUD overrides) stay editable after vesting, since the employer's annual ESS statement arrives after each vest is recorded. `DELETE /ess_statements/:id` removes the statement and its vest Buy together, returning `422` while that Buy is **drawn on** by a Sell allocation or AMIT adjustment (remove those first).

### Vesting an ESS statement

```
POST /ess_statements/6/vest
```

Creates the cost-base-reset **Buy** for the vested shares and links it back (`trades.ess_statement_id`) in one transaction — no request body. At the taxing point the ESS interest's first-element cost base is reset to its market value and it is taken to be re-acquired on that date for CGT, so the Buy is dated (and settled) on `taxing_point_date`, with `quantity` shares at `average_price` = `market_value_per_share`, zero brokerage, in the statement's currency. The discount clock runs from the taxing point (no `deemed_acquisition_date`). The income side (the assessable discount) is already on the statement and reaches the tax summary directly.

Returns `201 Created` with the created Buy as JSON, `404 Not Found` if no statement has that id, or `422 Unprocessable Entity` if the statement was already vested (delete it first to redo) or its `quantity`/`market_value_per_share` is not positive (nothing to create). The created Buy is immutable via `PUT /trades/:id` and never deleted individually (`DELETE /trades/:id` → `422`) — `DELETE /ess_statements/:id` removes it.

## Attachments

Supporting documents (a trade confirmation / contract note PDF, a dividend statement, an AMMA statement scan, an annual employee-share-scheme statement, a plain-text exchange record) attached to exactly one activity — a Trade, an Income record, an AMMA Statement, an ESS Statement, or an Interest Income record. The file bytes are stored in the database (a BLOB), so the weekly DB backup captures the documents with no separate file store. Because the payload is binary, these endpoints depart from the JSON-CRUD convention used elsewhere: upload is `multipart/form-data`, list/get return metadata only, and a dedicated endpoint streams the raw content.

| Method | Path | Description |
|--------|------|-------------|
| `GET` | `/attachments` | List attachment metadata (never the blob); filter by owner with `?trade_id=`, `?income_id=`, `?amma_statement_id=`, `?ess_statement_id=`, or `?interest_income_id=`; a trade filter can add `&include_linked=true` to also return the linked source record's documents (below) |
| `GET` | `/attachments/:id` | Get one attachment's metadata |
| `GET` | `/attachments/:id/content` | Download the raw file bytes (stored `Content-Type` + `Content-Disposition` filename) |
| `POST` | `/attachments` | Upload a file (`multipart/form-data`) |
| `DELETE` | `/attachments/:id` | Delete one attachment |

**Linked documents:** a provenance-created trade's paperwork usually lives on its source record — one registry advice documents both a distribution and its reinvestment, so it is attached to the income row the Reinvest action was run from, and the DRP trade's own attachment list would otherwise always be empty. `GET /attachments?trade_id=…&include_linked=true` therefore also returns the attachments owned by the record the trade was created from, traversing the provenance link at read time: the funding distribution of a DRP trade (`income.reinvestment_trade_id`), the dividend-component income row of a buy-back participation Sell (`income.buyback_trade_id`), and the annual statement of an ESS vest Buy (`trades.ess_statement_id`). The other provenance-created trades (transfers, rights exercises, scrip exchanges, demergers, inheritances, worthless-shares sells) trace to records that cannot own attachments, so there is nothing to traverse. Ownership is unchanged — each returned row still carries its true owner's id field (a linked row has `income_id`/`ess_statement_id` set and `trade_id` null), uploads and deletes stay with the owning record, and the web UI labels linked rows and routes Delete to the owner's own Attachments view. `include_linked=true` with no `trade_id`, or combined with another owner filter, returns `422 Unprocessable Entity`.

`POST /attachments` takes a `multipart/form-data` body with the file in a `file` part and **exactly one** owner field — `trade_id`, `income_id`, `amma_statement_id`, `ess_statement_id`, or `interest_income_id`. The server computes `byte_size` and the SHA-256 `checksum`, and returns `201 Created` with the stored metadata as JSON. It returns `422 Unprocessable Entity` if no owner or more than one owner is given, the owner id doesn't reference an existing activity, the `file` part is missing, or its content type is outside the allowlist (`application/pdf`, `image/png`, `image/jpeg`, `text/plain` — the last so plain-text records like crypto exchange exports and registry DRP advices are attachable); and `413 Payload Too Large` if the file exceeds 25 MB. Deleting the owning Trade / Income record / AMMA Statement / ESS Statement / Interest Income record removes its attachments automatically (`ON DELETE CASCADE`).

## DRP enrolments

Records when each holding reinvests its distributions, as **dated enrolment periods**: `enrolment_date` (inclusive) to `unenrolment_date` (exclusive; omitted = open-ended, i.e. currently enrolled). A holding can start unenrolled, enrol, unenrol, and re-enrol — one row per period, each with its own residual handling. Enrolment is per **(listing, holding account)**: the same listing may be enrolled in one account and not another (e.g. an employer share-plan account that cannot DRP alongside an enrolled personal account); `holding_account_id` defaults to the seeded default account when omitted.

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

A (listing, holding account)'s periods must not overlap, and at most one may be open at a time per account — validated atomically at write time (touching periods, where one ends the day the next starts, are allowed; the same listing's periods in another account are independent). Closing a period (unenrolling) settles its trailing residual: the leftover the period's last reinvestment carried forward is moved to `residual_paid_out` on that DRP trade (in the period's account) in the same transaction, since the registry refunds it at termination; it is **not** picked up after a re-enrolment.

Returns `204 No Content`, or `422 Unprocessable Entity` if `listing_id` doesn't reference a listing, the period overlaps another period for the same (listing, holding account) (or would be a second open period in that account), or `unenrolment_date` is not after `enrolment_date`.

## CGT settings

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

## Corporate actions

Corporate actions recorded against a listing. Seven action types are modelled:

- `ReturnOfCapital` — a non-assessable payment from a company (a shareholder-approved return of share capital, CGT event G1; see `docs/ato/cgt-non-assessable-payments.md`). The per-unit payment reduces the cost base of every parcel of the listing held on the payment date (units sold before the payment were not held for it and are unaffected) in the [portfolio](#overview), [open parcels](#open-parcels), [unrealised](#unrealised-gains), and [realised](#realised-gains) reports. Where cumulative payments exceed a parcel's per-unit cost base, the cost base floors at nil and the excess is a capital gain in the payment's income year — G1 never produces a capital loss — reported by the [net capital gain report](#net-capital-gain).
- `ShareSplit` — a share split or consolidation (TD 2000/10; see `docs/ato/share-splits-and-consolidations.md`): on the conversion `date`, every `split_old_units` units of the listing become `split_new_units` units (a 2-for-1 split is new=2/old=1; a 1-for-10 consolidation is new=1/old=10). **No CGT event happens**: the converted parcels keep their total cost base and their original acquisition date (the 12-month discount clock keeps running) — only the unit count, and so the per-unit cost base, changes. Trade rows keep the quantities as originally transacted; the reports and the Sell/trade write-time capacity checks re-base quantities between unit bases (a trade dated on or after the conversion date is already in post-split units, so after a 2-for-1 split a 100-share parcel covers a 200-share sale). Open-holdings reports show quantities in current units (the unrealised report in the units of its `as_of_date`); a `ReturnOfCapital` payment after a split is per post-split unit. A consolidation that doesn't divide a holding evenly keeps the exact fractional quantity (company rounding / cash-in-lieu arrangements are not modelled). AMIT adjustment quantities remain expressed in the parcel's as-acquired units.
- `BonusIssue` — a non-assessable bonus share issue (the general post-1 July 1998 case; see `docs/ato/bonus-shares.md`): on the issue `date`, every `bonus_held_units` units held receive `bonus_units` additional units (a 1-for-10 issue is bonus=1/held=10). **No CGT event happens**: the ATO apportions each parcel's cost base over the original + bonus shares and the bonus shares take the original acquisition date — the same quantity re-base as a `ShareSplit` with new = held + bonus and old = held, and the reports and write-time checks treat it identically (a trade dated on or after the issue date is ex-bonus and receives nothing). Bonus shares received **in lieu of a dividend** (a bonus share plan) are assessed as a dividend — enter those as a distribution plus a DRP reinvestment trade (the new parcel is acquired at the issue date with the dividend as its cost base), not as this action. Partly paid bonus shares and call payments are not modelled.
- `RightsIssue` — rights to acquire new shares, issued free to existing holders (see `docs/ato/rights-issues.md`): on the record `date`, every `rights_held_units` units held entitle the holder to acquire `rights_units` new units at `exercise_price` per unit in `currency` (a 1-for-4 issue is rights=1/held=4; a trade dated on or after the record date is ex-rights). Recording the action changes nothing by itself — the rights' market value is non-assessable non-exempt income on issue. Exercising it (`POST /corporate_actions/:id/exercise`, below) creates the new parcel; selling or lapsing the rights themselves — including a renounceable-offer retail premium (TR 2017/4, `docs/ato/retail-premiums.md`) — is `POST /corporate_actions/:id/sell_rights` (below), a CGT event on the rights whose deemed acquisition dates are inherited from the original parcels. Pre-CGT originals and non-renounceable-offer retail premiums (an unfranked dividend — enter as income, TR 2012/1) are not modelled.
- `BuyBack` — an off-market share buy-back (see `docs/ato/share-buy-backs.md`, QC 66049): the company offers to buy shares back directly from holders. On/after the buy-back `date`, each unit bought back is paid `buyback_price` in `currency`, of which `buyback_dividend` is an assessable franked dividend carrying `buyback_franking_credit` per unit (both 0 for a listed-company buy-back announced after 7:30 pm AEDT 25 Oct 2022 — those have no dividend component); `buyback_market_value` is the per-unit market value had the buy-back not been proposed (capital proceeds can't be less than it; omit it when the price is at or above market value). Recording the action changes nothing by itself; participating (`POST /corporate_actions/:id/participate`, below) creates the disposal and the dividend income together. The further adjustments where the participating shareholder is itself a company, and shares held on revenue account, are not modelled.
- `ScripForScrip` — a takeover or merger completed as a scrip exchange with scrip-for-scrip rollover (Subdiv 124-M; see `docs/ato/takeovers-and-scrip-for-scrip.md`): on the exchange `date`, every `scrip_old_units` units of the listing become `scrip_new_units` units of `scrip_listing_id` (the replacement listing, which must differ; a 1-for-1 merger is new=1/old=1) — plus, when the offer pays mixed consideration (the **partial rollover**, the guide's Example 27), `scrip_cash_per_unit` cash per old unit, with `scrip_market_value` (one replacement unit's market value just after issue) and `scrip_cash_currency` (the three come together or not at all; the rollover then applies only to the scrip portion and the cash side's market-value-apportioned gain is assessed now). Recording the action changes nothing by itself; exchanging (`POST /corporate_actions/:id/exchange`, below) substitutes every open parcel. Takeovers **without** rollover are an ordinary market-value disposal — enter the Sell and Buy manually (a pure-cash takeover is an ordinary Sell); multiple replacement share classes, pre-CGT originals, and rolling over a capital loss (not permitted by law) are not modelled.
- `Demerger` — an eligible demerger with the Div 125 rollover chosen (see `docs/ato/demergers.md`, QC 64895): on the demerger `date`, every `demerger_held_units` units of the listing (the head entity) held receive `demerger_new_units` units of `demerger_listing_id` (the demerged entity's listing, which must differ; BHP Billiton's 1-for-5 demerger of BHP Steel is new=1/held=5), and `demerger_cost_base_pct` percent of each parcel's cost base is apportioned to the new interests (the head-entity-advised percentage — e.g. 5.063 for BHP Steel; the head parcels keep the rest). Recording the action changes nothing by itself; demerging (`POST /corporate_actions/:id/demerge`, below) apportions every open parcel. Demergers **without** rollover (the new interests are then acquired at the demerger date under the ordinary cost-base rules — enter the Buy manually), pre-CGT original interests, assessable demerger dividends / separate capital returns (enter income or a `ReturnOfCapital`), and registry cash-in-lieu of fractional entitlements are not modelled.
- `WorthlessShares` — a capital loss on a failed company without an ordinary sale (CGT events G3 and C2; see `docs/ato/worthless-shares.md`, QC 52234 / TD 2000/52 / TD 2000/7): `worthless_event` records which event the loss is recognised under — `G3Declaration` (s 104-145: a liquidator/administrator made a written declaration of no-likely-distribution and the shareholder chooses to crystallise the loss) or `C2Cancellation` (s 104-25: the company was deregistered and the shares cancelled) — and the `date` is the declaration or cancellation date. Recording the action changes nothing by itself; recognising (`POST /corporate_actions/:id/recognise`, below) closes every open parcel at nil proceeds, each producing a capital loss equal to its remaining reduced cost base (never income, never discounted). The G3 opt-in eligibility tests (the user's call), the cost-base-reset-to-nil for shares still held after a G3 declaration (the operation closes the whole holding), worthless *financial instruments* other than shares, and the 18-month later-recovery timing rule are not modelled.

| Method | Path | Description |
|--------|------|-------------|
| `GET` | `/corporate_actions` | List corporate actions |
| `GET` | `/corporate_actions/:id` | Get one corporate action |
| `PUT` | `/corporate_actions/:id` | Create or update a corporate action |
| `DELETE` | `/corporate_actions/:id` | Delete a corporate action |
| `POST` | `/corporate_actions/:id/exercise` | Exercise a `RightsIssue` into a new Buy parcel |
| `POST` | `/corporate_actions/:id/sell_rights` | Sell or lapse a `RightsIssue`'s rights (a disposal of the rights themselves) |
| `GET` | `/rights_sales` | List recorded rights sales (allocations embedded) |
| `GET` | `/rights_sales/:id` | Get one rights sale |
| `DELETE` | `/rights_sales/:id` | Delete a rights sale (undo — frees the entitlement) |
| `POST` | `/corporate_actions/:id/participate` | Sell units into a `BuyBack` (Sell + dividend income, atomic) |
| `POST` | `/corporate_actions/:id/exchange` | Exchange a `ScripForScrip` takeover (closing Sell + replacement parcels, atomic) |
| `POST` | `/corporate_actions/:id/demerge` | Demerge a `Demerger` (closing Sell + head and demerged parcels, atomic) |
| `POST` | `/corporate_actions/:id/recognise` | Recognise a `WorthlessShares` loss (closing Sell at nil proceeds, atomic) |

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
  "scrip_old_units": "1",
  "scrip_cash_per_unit": "10",
  "scrip_market_value": "20",
  "scrip_cash_currency": "AUD"
}
```

The three `scrip_cash_*` fields are the optional cash component (partial rollover) — omit all three for an all-scrip exchange.

```

PUT /corporate_actions/7
{
  "action_type": "Demerger",
  "listing_id": 1,
  "date": "2026-05-01",
  "demerger_listing_id": 3,
  "demerger_new_units": "1",
  "demerger_held_units": "5",
  "demerger_cost_base_pct": "5.063"
}
```

```
PUT /corporate_actions/8
{
  "action_type": "WorthlessShares",
  "listing_id": 1,
  "date": "2026-05-01",
  "worthless_event": "G3Declaration"
}
```

Each action type carries exactly its own payload: a `ReturnOfCapital` has `amount_per_unit` + `currency`, a `ShareSplit` has `split_new_units` + `split_old_units`, a `BonusIssue` has `bonus_units` + `bonus_held_units`, a `RightsIssue` has `rights_units` + `rights_held_units` + `exercise_price` + `currency`, a `BuyBack` has `buyback_price` + `buyback_dividend` + `buyback_franking_credit` + an optional `buyback_market_value` + `currency` (the dividend and credit default to 0 when omitted), a `ScripForScrip` has `scrip_listing_id` + `scrip_new_units` + `scrip_old_units` + an optional cash component `scrip_cash_per_unit` + `scrip_market_value` + `scrip_cash_currency` (all three present or all absent), a `Demerger` has `demerger_listing_id` + `demerger_new_units` + `demerger_held_units` + `demerger_cost_base_pct`, a `WorthlessShares` has `worthless_event` (`G3Declaration` | `C2Cancellation`) — the other types' columns are null in the table (enforced by CHECKs and the PUT handler), and GET responses omit them, returning only the action's own fields. Returns `204 No Content`, or `422 Unprocessable Entity` when `amount_per_unit` is not positive, a split/bonus/rights/scrip/demerger ratio or `exercise_price` is missing or not positive, `buyback_price` is missing or not positive, `buyback_dividend` is negative or exceeds the price, `buyback_franking_credit` is negative or attached to a zero dividend, `buyback_market_value` is not positive, `scrip_listing_id` or `demerger_listing_id` is missing, unknown, or the same as `listing_id`, the scrip cash component is partial (`scrip_cash_per_unit`, `scrip_market_value`, and `scrip_cash_currency` come together) or its amounts are not positive or its currency unknown, `demerger_cost_base_pct` is missing or not strictly between 0 and 100, `worthless_event` is missing or not one of `G3Declaration`/`C2Cancellation`, the payload mixes the per-type fields, the listing or currency is unknown, or the action type is unrecognised. A payment's `currency` must match the affected trades' currency — the reports never net amounts across currencies and fail loudly (`500`) on a mismatch.

### Exercising a rights issue

```
POST /corporate_actions/4/exercise
{
  "date": "2025-11-01",
  "units": "250",
  "rights_cost": "0",
  "fx_rate": "1"
}
```

Exercising rights is no CGT event (`docs/ato/rights-issues.md`): the endpoint atomically creates a Buy trade — the new parcel — dated the exercise `date` (the parcel's acquisition date, so **the 12-month CGT discount clock runs from exercise**, not from the rights or the original shares; the company allots the shares, so the settlement date is the exercise date too). The parcel's cost base is the amount paid to exercise (`units × exercise_price`, carried as the trade's quantity × average price) plus `rights_cost` — the total paid to acquire the exercised rights, 0 (the default) for rights issued free — carried on the trade's `brokerage` column (both are components of the single cost base every report computes). `fx_rate` is the optional manual foreign-per-AUD fallback (defaults to 1).

Cumulative exercised units are capped at the entitlement: units held when the record date arrived (trades dated before the action's `date`, re-based to record-date units across any splits/consolidations) × `rights_units / rights_held_units`, with a fractional entitlement rounded **up** to a whole unit (registry practice). The created trade carries `rights_action_id` linking it to the action; to keep the cap honest the trade is immutable via `PUT /trades` (delete it — which frees the entitlement — and re-exercise instead), and the action itself returns `422` on `PUT`/`DELETE` while exercise trades reference it.

Returns `201 Created` with the created trade as JSON, `404 Not Found` if no corporate action has that id, or `422 Unprocessable Entity` if the action is not a `RightsIssue`, `units` is not positive, `rights_cost` is negative, the exercise date precedes the record date, or the exercise would exceed the remaining entitlement.

### Selling or lapsing rights

```
POST /corporate_actions/4/sell_rights
{
  "date": "1998-07-15",
  "units": "250",
  "proceeds_per_right": "0.20",
  "rights_cost": "0",
  "fx_rate": "1",
  "holding_account_id": 1,
  "allocations": [
    { "purchase_trade_id": 12, "units": "250" }
  ]
}
```

Disposing of the rights themselves — selling them on-market, letting them lapse, or receiving a **retail premium** for not taking them up under this (renounceable) offer — is a CGT event on the **rights**, not on the original shares (`docs/ato/rights-issues.md` Example 39; `docs/ato/retail-premiums.md`, TR 2017/4). The endpoint atomically records a `rights_sales` row with its anchoring allocations; **no trade is created and the share holding is untouched**. The disposal surfaces in the [realised gains](#realised-gains) report as a `source = "RightsSale"` row — proceeds `units × proceeds_per_right`, cost base `rights_cost` (`0`, the default, for rights issued free — so a free right that lapses is a nil/nil non-event, while nil proceeds on a *paid* right realises a capital loss) — and flows into the [net capital gain](#net-capital-gain) buckets from there. A retail premium is entered as the premium per right in `proceeds_per_right`; it is **not** dividend income. Both legs convert to AUD at the sale month's ATO rate with `fx_rate` (default 1) as the manual fallback; amounts are in the issue's `currency`.

Free rights are taken to have been **acquired when the original shares were acquired**, so each sale carries `allocations` anchoring the sold rights to original parcels — Buy/DRP trades of the issue's listing dated before the record date — and each allocation's 12-month discount clock runs from its parcel's (possibly deemed) acquisition date. Unlike a Sell's parcel allocations these consume nothing. Two caps are validated at write time: **total** — rights sold plus rights exercised against the action may not exceed the record-date entitlement (the same shared cap as [exercising](#exercising-a-rights-issue)); **per parcel** — rights anchored to a parcel, cumulatively across the action's sales, may not exceed the entitlement that parcel's record-date units earned (its units held at the record date × `rights_units / rights_held_units`, rounded up), so a sale can't borrow an older parcel's acquisition date for the discount.

Rows are immutable (no `PUT`) — `DELETE /rights_sales/:id` removes one (its allocations cascade), freeing the entitlement, and re-entry amends. While sales reference the action it returns `422` on `PUT`/`DELETE`, and an anchoring parcel Buy returns `422` on `PUT`/`DELETE /trades` (see [Trades](#trades)).

Returns `201 Created` with the created rights sale (allocations embedded) as JSON, `404 Not Found` if no corporate action has that id, or `422 Unprocessable Entity` if the action is not a `RightsIssue`, `units` is not positive, `proceeds_per_right` or `rights_cost` is negative, the date precedes the record date, the allocations are empty / non-positive / don't sum to `units`, an allocated parcel is not a Buy/DRP of the issue's listing held before the record date, a parcel's anchoring cap is exceeded, or the sale would exceed the remaining shared entitlement.

`GET /rights_sales` and `GET /rights_sales/:id` return the recorded sales with their allocations embedded (`[{ purchase_trade_id, units }]`).

### Participating in a buy-back

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

Returns `201 Created` with `{ "trade": …, "income": …|null }` as JSON, `404 Not Found` if no corporate action has that id, or `422 Unprocessable Entity` if the action is not a `BuyBack`, `units` is not positive, the participation date precedes the buy-back date, or a Sell-side invariant fails — a Sell-side rejection carries the same per-invariant body as [`PUT /sells/:id`](#sells) (allocation mismatch, over-allocation, wrong holding account, …), so the response says which invariant failed.

### Exchanging a scrip-for-scrip takeover

```
POST /corporate_actions/6/exchange
```

Takes no parameters — the action's terms and the holdings at its `date` determine everything. The rollover disregards the capital gain on the original shares and deems the replacement shares acquired *for the cost base of the original interest*, with the combined holding period counting toward the 12-month CGT discount (`docs/ato/takeovers-and-scrip-for-scrip.md`). With a cash component on the action (the **partial rollover**, the guide's Example 27), the rollover applies only to the scrip portion: each parcel's remaining reduced cost base is apportioned between cash and scrip by the consideration's market values — `cash×old / (cash×old + mv×new)` to the cash side — and the cash side is an ordinary disposal assessed now. The exchange therefore creates, in one transaction:

- a **closing Sell** on the original listing dated the exchange date — priced at `scrip_cash_per_unit` in `scrip_cash_currency` (price 0 when all-scrip), with parcel allocations consuming every open parcel (through the same write-time invariants as [`PUT /sells/:id`](#sells)). It carries `scrip_action_id`: when all-scrip that **excludes it from the [realised gains](#realised-gains) and [net capital gain](#net-capital-gain) reports** (the disposal happens, but its gain is disregarded and the zero proceeds never surface as a loss); with cash those reports assess its proceeds against the cash-apportioned share of each parcel's reduced cost base, discount-classified by the parcel's original (or deemed) acquisition date. The [performance report](#performance) counts the cash as real external proceeds on top of the carried cost.
- one **replacement Buy** per consumed parcel on the replacement listing, dated the exchange date (so later splits and returns of capital on the replacement listing apply only from then), with quantity = the parcel's remaining units at the exchange date × `scrip_new_units / scrip_old_units`. The parcel's remaining reduced cost base (AMIT- and return-of-capital-adjusted, floored at nil; its scrip-apportioned share when there is cash) is carried on the trade's `brokerage` column with a zero price — numerically part of the single cost base every report computes — and the parcel's acquisition date (chained through any earlier exchange) is carried as `deemed_acquisition_date`, which drives the discount clock, the reported acquisition date in the [open parcels report](#open-parcels), and the AUD translation month of the cost base. The parcel's `currency` and manual `fx_rate` fallback carry over too, so a non-AUD parcel's AUD cost base is unchanged by the exchange.

The created trades form the exchange group (`trades.scrip_action_id`): each is rejected by `PUT /sells/:id` and `PUT`/`DELETE /trades/:id` (`422`); `DELETE /sells/:id` on the closing Sell removes the whole group, restoring the pre-exchange holding (refused with `422` while a replacement Buy is consumed by later allocations or AMIT adjustments); and the action itself returns `422` on `PUT`/`DELETE` while the group exists.

Returns `201 Created` with `{ "sell": …, "replacements": […] }` as JSON, `404 Not Found` if no corporate action has that id, or `422 Unprocessable Entity` if the action is not a `ScripForScrip`, it has already been exchanged, nothing of the original listing is held at the exchange date, or the original listing has a trade dated on/after the exchange date (the takeover delisted it — fix the data first).

### Demerging

```
POST /corporate_actions/7/demerge
```

Takes no parameters — the action's terms and the holdings at its `date` determine everything. With the Div 125 rollover chosen, any capital gain or loss under the demerger is disregarded, the cost base of each parcel is spread over the remaining head interests and the new demerged-entity interests by the advised percentages, the head interests' acquisition dates are unchanged, and the new interests' 12-month discount clock runs from the date the corresponding original interests were acquired (`docs/ato/demergers.md`, the ATO's Example 32). The demerge therefore creates, in one transaction:

- a **closing Sell** on the head listing dated the demerger date — price 0, with parcel allocations consuming every open parcel (through the same write-time invariants as [`PUT /sells/:id`](#sells)). It carries `demerger_action_id`, which **excludes it from the [realised gains](#realised-gains) and [net capital gain](#net-capital-gain) reports** — no gain or loss is recognised, and the zero proceeds never surface as a loss — and, per consumed parcel:
- a **head replacement Buy** on the same listing for the parcel's remaining units, carrying `(100 − demerger_cost_base_pct)%` of its remaining reduced cost base (AMIT- and return-of-capital-adjusted, floored at nil), and
- a **demerged-entity Buy** on `demerger_listing_id` for those units × `demerger_new_units / demerger_held_units` (exact fractional entitlements are kept — registry rounding / cash-in-lieu is not modelled), carrying the other `demerger_cost_base_pct`% — the two legs always sum exactly to the parcel's cost base.

Both Buys are dated the demerger date (so later splits and returns of capital on either listing apply only from then), carry the cost base on the `brokerage` column with a zero price, and carry the parcel's acquisition date (chained through any earlier rollover) as `deemed_acquisition_date` — it drives the discount clock, the reported acquisition date in the [open parcels report](#open-parcels), and the AUD translation month of the cost base; the parcel's `currency` and manual `fx_rate` fallback carry over, so a non-AUD parcel's AUD cost base is unchanged by the demerger.

The head shares are never actually disposed of in a demerger, so the closing Sell and head replacement Buys are also **excluded from the franking-credit 45-day walk** (see the [tax summary](#tax-summary)) — the original parcels' at-risk days keep running, and a dividend going ex around the demerger is not spuriously disqualified. The demerged-entity Buys are included (they are the only record of those holdings).

The created trades form the demerger group (`trades.demerger_action_id`): each is rejected by `PUT /sells/:id` and `PUT`/`DELETE /trades/:id` (`422`); `DELETE /sells/:id` on the closing Sell removes the whole group, restoring the pre-demerger holding (refused with `422` while a replacement Buy is consumed by later allocations or AMIT adjustments); and the action itself returns `422` on `PUT`/`DELETE` while the group exists.

Returns `201 Created` with `{ "sell": …, "head_replacements": […], "demerged_replacements": […] }` as JSON (the two replacement lists pair up per consumed parcel), `404 Not Found` if no corporate action has that id, or `422 Unprocessable Entity` if the action is not a `Demerger`, it has already been demerged, nothing of the head listing is held at the demerger date, or the head listing has a trade dated on/after the demerger date (the demerge closes the holding as at that date — enter later activity after demerging).

### Recognising worthless shares

```
POST /corporate_actions/8/recognise
```

Takes no parameters — the action and the holdings at its `date` determine everything. Closes every open parcel of the listing held at the event date through a single **closing Sell at price 0**, with parcel allocations consuming every open parcel (through the same write-time invariants as [`PUT /sells/:id`](#sells)), across every holding account. It carries `worthless_action_id`.

Unlike the scrip-for-scrip and demerger closing Sells (which their provenance columns *exclude* from the gains reports because the rollover disregards the gain), this Sell is **counted by the [realised gains](#realised-gains) and [net capital gain](#net-capital-gain) reports**: its nil proceeds against each consumed parcel's remaining reduced cost base (AMIT- and return-of-capital-adjusted, floored at nil) **recognise** the capital loss — never income, never discounted — which then flows through the net-capital-gain loss pool and carry-forward like any realised loss. There are no replacement Buys (the shares are simply gone).

The Sell is immutable: `PUT /sells/:id` and `PUT`/`DELETE /trades/:id` return `422`; `DELETE /sells/:id` removes it and restores the pre-event holding; and the action itself returns `422` on `PUT`/`DELETE` while the Sell exists.

Returns `201 Created` with `{ "sell": … }` as JSON, `404 Not Found` if no corporate action has that id, or `422 Unprocessable Entity` if the action is not a `WorthlessShares`, it has already been recognised, nothing of the listing is held at the event date, or the listing has a trade dated on/after the event date (the company has failed — fix the data first).

## DRP reinvestment

```
POST /income/:id/reinvest
{ "reinvestment_price": "1.50", "units": "0.500", "fx_rate": "0.65", "date": "2024-03-31" }
```

Creates the DRP reinvestment trade for a distribution and links it back (`income.reinvestment_trade_id`) in one transaction. `units` (see below), `fx_rate` (default 1), and `date` (default the distribution's `date_paid`) are optional.

Reinvestability is decided as at the distribution's **ex date** (registry practice: DRP participation is fixed at the record date), falling back to `date_paid` when no ex date is recorded. That date must fall inside one of the [enrolment periods](#drp-enrolments) **for the distribution's holding account** — a distribution dated before enrolment, in a gap between unenrolment and re-enrolment, or paid to an account that isn't enrolled (e.g. an employer-plan account while only the personal account is enrolled) is rejected — and the matching period's `residual_handling` applies. The created DRP trade lands in the distribution's holding account.

The reinvestable cash — `franked_amount + unfranked_amount + foreign_source_income − foreign_tax_paid − tfn_withholding_tax` (franking credits are notional and excluded) — plus the residual brought forward from the most recent prior DRP trade *within the same enrolment period and holding account* is spent on whole shares at `reinvestment_price`. The leftover is carried forward or paid out per the period's `residual_handling` and recorded on the new trade's residual columns. The carried-forward chain never crosses periods or accounts: a period's trailing residual is paid out at unenrolment, and each account runs its own chain.

**Fractional allotments (`units`):** a broker plan that reinvests in fractional shares (e.g. a US broker DRP, unlike the whole-share ASX registries) states the allotted units on its statement and leaves no residual. Supply the statement's figure as the optional `units` and it is taken **exactly** as the trade's quantity (stored as stated, trailing zeros included) — whole-share flooring is bypassed. The price is cross-checked against the available cash (the reinvestable cash plus any residual brought forward): `units × reinvestment_price` must agree with it to within **one unit-step at the units' stated precision** (a figure stated to 3 decimals must be within `0.001 × price`) — the property any broker-computed allotment has whatever its rounding direction — and a larger mismatch is rejected with `422` whose body carries both figures. The sub-step difference is statement rounding, not cash: the trade's residual columns record zero. Omitting `units` keeps the whole-share default above.

Returns `201 Created` with the created trade as JSON, `404 Not Found` if no income record has that id, or `422 Unprocessable Entity` if no enrolment period for the distribution's holding account covers its ex date (or pay date when no ex date is recorded), the distribution was already reinvested, `reinvestment_price` or `units` is not positive, or `units × reinvestment_price` is a full unit-step or more off the available cash.

```
DELETE /income/:id/reinvest
```

Undoes a reinvestment: deletes the DRP trade and clears the distribution's `reinvestment_trade_id` in one transaction, so the distribution can be reinvested again (e.g. to correct a mistyped price). This is the **only** path that clears the link — `PUT /income` never touches it and a reinvest-created DRP is immutable via `PUT`/`DELETE /trades` — so an orphaned DRP trade can never exist. The web UI offers it as the **Undo reinvest** action on a reinvested income row.

Returns `204 No Content` on success, `404 Not Found` if no income record has that id, or `422 Unprocessable Entity` if the distribution has no reinvestment trade, the DRP trade is drawn on by a Sell allocation or AMIT adjustment (remove those first, e.g. delete the Sell via `DELETE /sells/:id`), or a **later** DRP trade exists for the same listing and holding account — the residual chain reads each reinvestment's brought-forward cash back from the most recent prior DRP trade, so removing a mid-chain trade would falsify the later trade's residuals; undo runs last-in-first-out (undo the later reinvestments first).

## Sells

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
  "brokerage_includes_gst": false,
  "brokerage_currency": "AUD",
  "fx_rate": "1",
  "contract_note_ref": null,
  "statement_total": null,
  "holding_account_id": 1,
  "allocations": [
    { "purchase_trade_id": 1, "quantity_allocated": "100" }
  ]
}
```

`settlement_date` is optional and auto-calculated as for trades. `holding_account_id` is optional and defaults to the seeded default account; the Sell's allocations may only consume parcels held in that account (see [Holding accounts](#holding-accounts)). An optional `spot_fx_rate` behaves as on [Trades](#trades): a deliberate transaction-date spot rate that wins over the monthly RBA rate when converting the proceeds to AUD, rejected with `422` when non-positive or on an AUD Sell. `brokerage_includes_gst` and `statement_total` behave as on [Trades](#trades) — including the cent-rounding tolerance (a total matching the computed figure rounded to the cent, half away from zero, passes) and the lossless GST-inclusive round-trip (a flagged Sell reads back via `GET /trades/:id` with `brokerage` as the one inclusive amount, and re-`PUT`ting that body to `PUT /sells/:id` re-splits it to the identical stored pair) — except that the statement total is the **net proceeds** — `quantity × price − brokerage − GST` (the statement nets costs out of what you receive). Re-`PUT`ting the same id replaces the Sell row and *all* of its allocations with the submitted set.

Returns `204 No Content` on success, or `422 Unprocessable Entity` if the allocations do not sum exactly to `quantity`, a referenced purchase trade is missing or is not a Buy/DRP, an allocation would over-allocate a purchase parcel, an allocation consumes a parcel of a **different listing** than the Sell's `listing_id` (a sale can only dispose of the security actually sold — a cross-listing allocation would be costed against the wrong security in the CGT reports), an allocation consumes a parcel **dated after the sale date** (units can't be sold before they were acquired; a same-day parcel is fine), an allocation consumes a parcel held in a different holding account from the Sell's (move it first via a [Transfer](#transfers), or fix the Sell's `holding_account_id`; the mechanically constructed scrip-for-scrip/demerger/worthless-shares closing Sells are exempt — they close the whole holding across every account), or the existing trade is a buy-back participation Sell, a scrip-for-scrip exchange or demerger closing Sell, a worthless-shares recognise closing Sell, a holding-account transfer-out Sell, or a crypto network-fee disposal Sell (`buyback_action_id` / `scrip_action_id` / `demerger_action_id` / `worthless_action_id` / `transfer_id` set, or linked via `transfers.fee_sale_trade_id` — its figures derive from its action's or transfer's terms; delete it and re-participate/re-exchange/re-demerge/re-recognise/re-transfer instead, see [Corporate actions](#corporate-actions) and [Transfers](#transfers)). The Sell's core figures are sanity-checked exactly as on [Trades](#trades) — positive `quantity` and `fx_rate`, non-negative price/brokerage/GST, settlement not before the sale date — and every allocation's `quantity_allocated` must be **positive**: a zero or negative allocation returns `422` (a negative one would pass the sum and per-parcel capacity checks while quietly shifting capacity between parcels — e.g. −5 on parcel A and +105 on parcel B "sums" to a 100-unit Sell). On any failure the whole transaction is rolled back — nothing is persisted. Allocation quantities are in the sale date's unit basis: the over-allocation check re-bases them across any [share splits/consolidations or bonus issues](#corporate-actions) between the purchase and the sale, so after a 2-for-1 split a 100-share parcel covers a 200-share sale.

```
DELETE /sells/:id
```

Deletes a Sell trade and all of its parcel allocations in one transaction, freeing the purchase parcels those allocations had consumed. A buy-back participation Sell also takes its linked dividend-component income row (`income.buyback_trade_id`) with it, so the capital and dividend sides are always removed together. A scrip-for-scrip exchange or demerger closing Sell takes its group's replacement Buys (`trades.scrip_action_id` / `trades.demerger_action_id`) with it, restoring the pre-exchange/pre-demerger holding. A worthless-shares recognise closing Sell (`trades.worthless_action_id`) deletes on its own — there are no replacement Buys — restoring the pre-event holding and thawing the action. Returns `204 No Content` on success, `404 Not Found` if no trade has that id, or `422 Unprocessable Entity` if the id refers to a trade that is not a Sell (use `DELETE /trades/:id` for Buy/DRP trades), a replacement Buy of the group is still consumed by later allocations or AMIT adjustments (remove those first), or the Sell is a holding-account transfer-out Sell or a crypto network-fee disposal Sell (its group — and the transfer record — is deleted as a whole via `DELETE /transfers/:id`, see [Transfers](#transfers)).

## Transfers

Moves a quantity of one listing between two holding accounts of the same owner — e.g. vested plan shares to the personal account. **Not a CGT event**: the same beneficial owner holds the shares before and after, so nothing is disposed of, nothing reaches the [realised gains](#realised-gains) or [net capital gain](#net-capital-gain) reports, and the franking 45-day at-risk clock keeps running across the move.

| Method | Path | Description |
|--------|------|-------------|
| `GET` | `/transfers` | List all transfers |
| `GET` | `/transfers/:id` | Get one transfer |
| `PUT` | `/transfers/:id` | Record **and execute** a transfer, atomically |
| `DELETE` | `/transfers/:id` | Delete the transfer and its whole trade group, restoring the pre-transfer holding |

```
PUT /transfers/1
{
  "listing_id": 1,
  "date": "2024-06-01",
  "from_account_id": 2,
  "to_account_id": 1,
  "allocations": [ { "purchase_trade_id": 1, "quantity_allocated": "100" } ]
}
```

The `allocations` say which parcels move and how many units of each (in transfer-date units, like a Sell's allocations); partial parcels are allowed. Executing creates, in one transaction (mirroring the scrip-for-scrip mechanics):

- a **transfer-out Sell** in the source account dated the transfer date — price 0, consuming the chosen quantity from each parcel via parcel allocations (through the same write-time invariants as [`PUT /sells/:id`](#sells), including the same-account rule). It carries `transfer_id`, which excludes it from every gains report and the franking at-risk walk, and
- one **transfer-in Buy** per consumed parcel in the destination account — the moved units at price 0, carrying the moved units' share of the parcel's remaining reduced cost base (AMIT- and return-of-capital-adjusted, floored at nil; pro-rated for a partial move) on the `brokerage` column, the parcel's `currency` and manual `fx_rate` fallback, and the parcel's acquisition date (chained through any earlier rollover or transfer) as `deemed_acquisition_date` — the 12-month discount clock and the AUD translation month of the cost base are unchanged by the move.

**Crypto network fee (optional).** A crypto wallet-to-wallet transfer can burn an on-chain network fee paid in the crypto itself. Per ATO guidance (`docs/ato/crypto-cgt.md`), the move stays a non-CGT event, but the crypto consumed to cover the fee **is a disposal** with capital-gain consequences. Add a fee by sending:

```
{
  …,
  "allocations": [ { "purchase_trade_id": 1, "quantity_allocated": "0.5" } ],
  "fee_allocations": [ { "purchase_trade_id": 1, "quantity_allocated": "0.001" } ],
  "fee_market_price": "80000"
}
```

`fee_allocations` are the source parcels (and units) disposed of to pay the fee — not moved — and `fee_market_price` is the fee crypto's per-unit market value at the transfer date, in the listing's currency (AUD for an AUD-priced crypto; an optional `fee_fx_rate`, default 1, converts a non-AUD listing's price to AUD). They become an ordinary **disposal Sell** in the source account at that market value: no `transfer_id`, so it **is counted by the realised-gains, net-capital-gain, and performance reports** with the 12-month discount, while still being linked to the transfer (`transfers.fee_sale_trade_id`) so it is created and removed atomically with it. The moved units and the fee units are validated together against each source parcel's capacity. Omit `fee_allocations` (or send an empty list) for no fee.

The created trades form the transfer group: the transfer-out Sell and transfer-in Buys (`trades.transfer_id`) and the fee Sell (`transfers.fee_sale_trade_id`) are each rejected by `PUT /sells/:id`, `PUT`/`DELETE /trades/:id`, and `DELETE /sells/:id` (`422`). A recorded transfer is immutable — re-`PUT`ting its id returns `422`; delete it and re-transfer instead. `DELETE /transfers/:id` removes the whole group and the record together (including the fee Sell), restoring the pre-transfer holding (refused with `422` while a transfer-in Buy is consumed by later allocations, AMIT adjustments, or income links — remove those first).

`PUT` returns `201 Created` with `{ "transfer": …, "sell": …, "transfer_ins": […], "fee_sale": … | null }` as JSON (the created trade ids matter to the client, so this operation departs from the bare-`204` PUT convention), or `422 Unprocessable Entity` if the source and destination accounts are the same, the transfer id already exists, there are no allocations, a parcel (moved or fee) belongs to a different listing, a fee was specified without a positive `fee_market_price`, an account or the listing is unknown (FK), or a Sell-side invariant fails (missing/over-allocated/non-Buy-DRP parcel, or a parcel outside the source account).

## Inheritances

Inherited parcels from a deceased estate (`docs/ato/inherited-assets-cost-base.md` QC 66053, `docs/ato/inherited-assets-cgt-discount.md` QC 69713 / s 115-30). Receiving the parcel is **not a CGT event** — recording the inheritance creates the parcel so CGT applies correctly when the beneficiary later disposes of it.

| Method | Path | Description |
|--------|------|-------------|
| `GET` | `/inheritances` | List all inheritances |
| `GET` | `/inheritances/:id` | Get one inheritance |
| `PUT` | `/inheritances/:id` | Record an inheritance **and create/update its parcel Buy**, atomically |
| `DELETE` | `/inheritances/:id` | Delete the inheritance and its parcel Buy together |

```
PUT /inheritances/1
{
  "listing_id": 1,
  "quantity": "100",
  "date_of_death": "2025-01-10",
  "cost_base_rule": "DeceasedCostBase",
  "cost_base": "3000",
  "deceased_acquisition_date": "2020-02-01",
  "lpr_expenditure": "200",
  "lpr_expenditure_date": "2025-03-01"
}
```

`cost_base_rule` records which QC 66053 rule produced the `cost_base` figure:

- `DeceasedCostBase` — the deceased acquired the asset **on or after 20 September 1985**: the first element is the deceased's cost base on the day they died, and `deceased_acquisition_date` is required — per s 115-30 the 12-month discount clock runs from the **deceased's acquisition**.
- `MarketValueAtDeath` — a pre-CGT asset in the deceased's hands: the first element is the asset's market value on the day they died (the user supplies the valuation figure), `deceased_acquisition_date` must be omitted, and the discount clock runs from the **date of death**.

`lpr_expenditure` is expenditure of the legal personal representative the beneficiary may include (e.g. conveyancing on the transfer, legal costs of proving the will), added to the parcel's cost base and dated (`lpr_expenditure_date`, required together with a non-zero figure, on or after the death) when the LPR incurred it. `holding_account_id` defaults to the seeded default account; `currency` defaults to AUD with `fx_rate` (foreign-per-AUD manual fallback) defaulting to 1.

The upsert writes the inheritance and its **parcel Buy** in one transaction: a Buy dated (and settled) on the date of death — an estate transmission is not market-settled — at price 0 with the whole cost base (`cost_base` + `lpr_expenditure`) on the brokerage column, carrying `trades.inheritance_id` and, under `DeceasedCostBase`, the deceased's acquisition date as `deemed_acquisition_date`. The parcel then flows through every report and write-time capacity check like any Buy, with the discount clock and the AUD translation month of a non-AUD cost base following the (deemed) acquisition date exactly as for rollover parcels.

`PUT` returns `204 No Content`, or `422 Unprocessable Entity` if the quantity is not positive, the cost base or LPR expenditure is negative, the date of death is before 20 September 1985 (the parcel would be pre-CGT in the *beneficiary's* own hands — outside CGT and not modelled, whichever cost-base rule was chosen), `deceased_acquisition_date` is missing under `DeceasedCostBase` or present under `MarketValueAtDeath`, the deceased's acquisition is before 20 September 1985 (that is the pre-CGT case — use `MarketValueAtDeath`) or after the death, the LPR expenditure and its date are not supplied together or the date precedes the death, or the listing/account/currency is unknown (FK). The linked Buy is immutable individually (`PUT`/`DELETE /trades/:id` → `422`); editing the inheritance updates it in place, and both `PUT` and `DELETE /inheritances/:id` return `422` while the parcel is **drawn on** by a Sell allocation or AMIT adjustment (remove those first).

## Parcel allocations

Parcel allocations are **read-only** over HTTP; they are created and replaced atomically with their Sell trade via `PUT /sells/:id`. Allowing standalone writes would let a Sell become under-covered (e.g. by deleting or shrinking an allocation).

| Method | Path | Description |
|--------|------|-------------|
| `GET` | `/parcel_allocations` | List all parcel allocations |
| `GET` | `/parcel_allocations/:id` | Get one parcel allocation |

`PUT` and `DELETE` on these paths return `405 Method Not Allowed`.

## Portfolio reports

### FX conversion

Reports take the Australian-tax view, so every non-AUD trade amount is converted to AUD before it is aggregated (`AUD = foreign / rate`, rates in foreign units per 1 AUD; AUD amounts pass through unchanged). One precedence rule applies everywhere a trade's amounts convert — cost base, proceeds, every report and the snapshot pipeline:

1. the trade's **`spot_fx_rate` override**, when set — a deliberate transaction-date spot rate that wins over the monthly rate;
2. else the **ATO reference rate** — the RBA F11 monthly rate for the amount's currency and the month of the relevant trade date;
3. else the trade's manual **`fx_rate` fallback** — used only when no ATO rate has been imported for that `(currency, month)`;
4. if none is available the report fails loudly (`500`) rather than leaving an amount unconverted.

The monthly rate is the ATO-published convenience default: ATO guidance (`docs/ato/forex-average-rates.md`, QC 18020) permits an average rate only where it is a **reasonable approximation of the spot rates** at the statutory translation times — fine for recurring or small amounts, but its Examples 5 and 7 state an average rate is **not appropriate for a one-off purchase or sale of a large capital asset**. Such a trade should carry the transaction-date spot rate in `spot_fx_rate` (`docs/ato/forex-common-transactions.md`: Lisa translates each leg at the day's rate). Entering the override is deliberate — absent it, behaviour is unchanged (monthly rate first, `fx_rate` fallback), and the silent fallback semantics of existing `fx_rate` values never flip.

Cost base and proceeds in the portfolio, unrealised, and realised reports are converted this way (a rollover/transfer replacement parcel carries its consumed parcel's `fx_rate` *and* `spot_fx_rate`, so the carried AUD cost base is unchanged by the substitution). A parcel's cost-base breakdown converts as one unit at the acquisition-month rate — including the AMIT/return-of-capital reductions that arose in later rate months (see [Known limitations](#known-limitations)). Income and AMMA amounts are also converted in the tax summary, using each record's `currency` and the month of `date_paid` (income) or `tax_year_end_date` (AMMA); these records have no manual `fx_rate` or spot override, so a non-AUD amount with no ATO rate fails loudly (`500`) rather than being passed through unconverted.

**Valuation-only fallback:** the RBA publishes a month's rate only after the month ends, so *valuing* a holding in the current month (a [report snapshot](#report-snapshots), a [live quote](#live-valuation)) would otherwise be blocked all month. Exactly those two valuation paths may fall back to the most recent earlier month's rate — at most 2 months back, beyond which they fail loudly as above — and the result is always flagged (the snapshot's `provisional` flag, the row's `fx_provisional`), never silently substituted. No tax calculation or FY report can reach a fallback-month rate: cost base, proceeds, and income keep the strict precedence rule above.

### Overview

```
POST /portfolio/overview
```

Returns open holdings per **(listing, holding account)** — the same listing held in two accounts (e.g. an employer share plan and a personal broker account) reports as two holdings. Request body (optional):

```json
{ "live": true, "prices": { "<listing_id>": "<price>" } }
```

Response fields per holding: `listing_id`, `holding_account_id`, `quantity`, `avg_cost_base_per_unit`, `total_cost_base`, `current_price` (nullable), `market_value` (nullable), `price_as_of` (nullable), `price_unavailable` (nullable).

Cost base is calculated as `(price × quantity + brokerage + GST) − AMIT reductions`, pro-rated to remaining (unsold) units, less [return-of-capital](#corporate-actions) payments received on those units — flooring at nil (CGT events E10 and G1) — then converted to AUD (see [FX conversion](#fx-conversion)). Supplied prices are expected in AUD, so `market_value` is AUD. The unrealised-gains report computes its cost base the same way. `quantity` is in *current* units — re-based across any [share splits/consolidations or bonus issues](#corporate-actions) — so it lines up with a current market price; the re-basing never changes the cost base totals.

**Live valuation (`live`):** see [Live valuation](#live-valuation) — with `live: true` each held listing with no explicit price is valued from the [price source](#closing-prices)'s latest quote (converted to AUD), and `price_as_of` carries the provider's quote timestamp; an unavailable quote sets `price_unavailable` and leaves the holding unvalued. The web UI defaults to live; the API defaults to off (so existing callers are unchanged).

The three price-taking reports (overview, [unrealised gains](#unrealised-gains), [performance](#performance)) are also run automatically against the stored [closing prices](#closing-prices) each day and persisted as [report snapshots](#report-snapshots); the request-supplied `prices` remain for ad-hoc what-if runs.

### Live valuation

The three price-taking reports — [overview](#overview), [unrealised gains](#unrealised-gains), and [performance](#performance) — accept `"live": true` to value holdings from the **current** price at the [price source](#closing-prices) (Yahoo) instead of returning empty valuations when no `prices` are supplied. Each held listing without an explicit price is valued from the provider's latest available quote, in the listing's quote currency, **converted to AUD** at the quote-month ATO rate — currencies are never mixed.

- **As-of time:** every live-valued row carries `price_as_of`, the provider's quote timestamp (RFC 3339 UTC) — how fresh the valuation is. The UI rolls the per-row times up into one "as at …" line; an explicitly supplied price has no `price_as_of`.
- **Explicit override:** a price in `prices` always wins and is never fetched — what-if valuations and the deterministic acceptance tests keep working unchanged.
- **Provisional FX:** early in a month the quote month's ATO rate cannot exist yet (the RBA publishes after month end), so the conversion falls back to the most recent earlier month's rate (at most 2 months back) and the row is flagged `fx_provisional: true` — never silently substituted. The UI rolls the flags up into a "valued at a provisional FX rate" note on the "as at" line. This valuation-only fallback never reaches a tax calculation (see [FX conversion](#fx-conversion)).
- **Graceful failure:** a per-listing fetch failure (provider error, a currency mismatch, or an FX-rate gap older than the fallback bound) does not zero the holding or fail the request — that row is left unvalued (`current_price`/`market_value` null) with the reason in `price_unavailable`, while the rest of the report still values (consistent with the never-silent-zero rule).

`live` defaults to **off** so existing API callers and the deterministic ATO acceptance tests never hit the network; the web UI sets it on by default. This is on-demand live valuation only — it does not write to the [closing-price history](#closing-prices) or the daily [report snapshots](#report-snapshots), which remain sourced from stored closing prices.

### Open parcels

```
GET /portfolio/open-parcels
```

Returns every open parcel — a Buy/DRP trade whose quantity is not fully consumed by parcel allocations — the per-parcel cost-base schedule to reconcile against a broker statement and the input to a sell decision (the [overview](#overview) aggregates the same parcels per listing). Response fields per parcel: `trade_id`, `listing_id`, `holding_account_id` (the account the parcel sits in), `ticker`, `acquisition_date`, `original_quantity`, `remaining_quantity` (units not yet allocated to a Sell), `original_cost_base` (price × quantity + brokerage + GST for the whole parcel), `amit_cost_base_reduction` (cumulative AMIT reductions to date — the full amount, even where CGT event E10 has floored the cost base), `return_of_capital_reduction` (cumulative [return-of-capital](#corporate-actions) payments received on the remaining units since acquisition — likewise the full amount, even where CGT event G1 has floored the cost base), and `remaining_cost_base` (`max(original − AMIT, 0)` pro-rated to the remaining units, less the return-of-capital payments on those units, floored at nil). All monetary fields are AUD, converted at the parcel's buy-month rate (see [FX conversion](#fx-conversion)). `remaining_quantity` is in *current* units — re-based across any [share splits/consolidations or bonus issues](#corporate-actions) so it reconciles with a broker statement — while `original_quantity` stays as transacted; `acquisition_date` is preserved across a split or bonus issue (TD 2000/10; `docs/ato/bonus-shares.md`). A [scrip-for-scrip](#corporate-actions) replacement parcel reports the consumed parcel's acquisition date (the rollover's combined holding period) and carries its remaining reduced cost base; its monetary fields convert at the *original* acquisition month's rate, so the AUD cost base is unchanged by the exchange. A [demerger's](#corporate-actions) head and demerged parcels likewise report the consumed parcel's acquisition date, each carrying its percentage share of that cost base.

A [transfer's](#transfers) transfer-in parcel likewise reports the moved parcel's acquisition date and carries its share of the remaining reduced cost base, in the destination account.

Sorted by `listing_id`, then `holding_account_id`, then `acquisition_date`, then `trade_id`.

### Listing activity

```
POST /portfolio/activity
```

Everything ever recorded against **one listing**, in chronological order, ending in the final holding summary (units held, cost base, current value). Request body:

```json
{ "listing_id": 1, "price": "12.50" }
```

`listing_id` (required) must exist (`404` otherwise). `price` (optional) is the current per-unit price in AUD for the holding summary; absent, it is live-fetched per the [live-valuation rules](#live-valuation), degrading per holding (`price_unavailable`) when no quote is obtainable — the ledger itself never needs a price.

Response: `listing_id`, `events` (the ledger), and `holdings` (the final summary). Every input is read on one read transaction, so the ledger and the summary come from a single consistent snapshot.

**`events`** — one row per recorded fact, sorted by date (a corporate action orders before same-dated trades: a trade dated on a split's conversion date is already in post-split units). Row kinds:

- every **trade**, labelled with the operation that created it — a plain `Buy`/`Sell`, `DRP reinvestment`, or `Buy (rights exercise)`, `Sell (buy-back)`, `Buy/Sell (scrip exchange)`, `Buy/Sell (demerger)`, `Sell (worthless shares)`, `Buy (ESS vest)`, `Buy (inheritance)`, `Sell (transfer network fee)` — **except** a [transfer](#transfers) group's own Sell/Buys, which collapse into the transfer's one row (a transfer is not a disposal and nets to nothing)
- **transfers between accounts** (one row: moved units, source and destination accounts, any network-fee disposal noted)
- **income** (`Dividend`, `Trust distribution`, `Dividend (buy-back component)`, with franking credits and any reinvestment link in the detail)
- **corporate actions** (return of capital, share split/consolidation, bonus issue, rights issue, buy-back offer, scrip-for-scrip takeover, demerger, worthless shares — each with its per-unit amount / ratio / terms)
- **AMMA statements** (dated by their 30 June year end), **ESS statements** (dated by the taxing point), **rights sales/lapses**, **DRP enrolment/unenrolment**, and **listing-scoped [investment expenses](#investment-expenses)**

Fields per row: `date`, `event`, `detail` (human-readable specifics in the record's own currency), `holding_account_id` (nullable — corporate actions and transfers span accounts), `quantity` (nullable: the row's signed unit effect, in its own date's unit basis), `units_after` (the whole-listing running balance after the row — a split/bonus issue re-bases it in place, a transfer leaves it unchanged; the last row's balance equals the holding summary's total quantity), and `amount_aud` (nullable: the row's own money figure in AUD — a trade's whole consideration `quantity × price ± brokerage and GST` converted with the trade's own [FX precedence](#fx-conversion); an income row's gross cash converted by the month the [tax summary](#tax-summary) uses; a rights sale's proceeds; an expense's deductible amount — absent where the row has no single amount, e.g. a per-unit return of capital).

**`holdings`** — the [overview](#overview)'s rows for the listing, one per holding account (`quantity`, `avg_cost_base_per_unit`, `total_cost_base`, and the `current_price` / `market_value` / `price_as_of` / `price_unavailable` valuation fields). An explicit `price` wins over the live fetch, exactly as in the overview.

### Parcel-selection optimiser

```
POST /portfolio/parcel-optimiser
```

Candidate parcel selections for a **contemplated** sale — which parcels a sale comes from is the taxpayer's choice (`docs/ato/cgt-keeping-records-shares.md`: Boris nominates the parcel that realises a loss), and it changes the tax outcome. **Read-only**: nothing is persisted; the user picks a candidate's allocations and enters them on the real [Sell](#sells). Request body:

```json
{ "listing_id": 1, "holding_account_id": 1, "units": "1500",
  "sale_date": "2026-06-15", "price": "8.00" }
```

`units` (required, positive) may not exceed the listing's open quantity in the account (`422` otherwise — a Sell may only consume its own account's parcels). `sale_date` (optional, default today) drives the 12-month discount clock. `price` is the per-unit sale price in AUD; absent, it is **live-fetched** per the [live-valuation rules](#live-valuation) — but unlike the valuation reports, which leave a row unvalued, no obtainable price rejects the request with `422` and the reason (the candidates can't be valued without one).

The candidate parcels are the [open-parcels](#open-parcels) rows (current units, adjusted AUD cost base), so every cost-base rule — AMIT/E10, return of capital/G1, splits, rollover carried dates and cost bases — flows through unchanged. The hypothetical sale carries no brokerage (it isn't known yet). Four strategies are returned, each allocating the units greedily in its preference order (ties broken FIFO):

- `fifo` — oldest acquisition first; the no-choice baseline
- `min_gain` — smallest **assessable** contribution per unit first: losses (in full), then discount-eligible gains at half weight, then non-discountable gains — minimises the current-year assessable gain
- `max_discount` — discount-eligible gain parcels first, then losses, then non-discountable gains last — maximises the proportion of the realised gain that gets the 50% discount
- `harvest_losses` — loss parcels first (largest per-unit loss first), then FIFO

Response: the echoed inputs (`listing_id`, `holding_account_id`, `units`, `sale_date`), the `price` used with `price_as_of` (the provider quote timestamp, only when live-fetched), `strategies` — one record per strategy with the [realised-gains](#realised-gains) buckets for its candidate disposal (`proceeds`, `cost_base`, `capital_gain_loss`, `discount_eligible_gain`, `non_discountable_gain`, `capital_loss`; the identity `capital_gain_loss == discount_eligible_gain + non_discountable_gain − capital_loss` holds per strategy) — and `allocations`, the per-parcel rows keyed back by `strategy`: `purchase_trade_id`, `holding_account_id`, `acquisition_date`, `units`, `cost_base`, `proceeds`, `capital_gain_loss`, `discount_eligible` (held strictly more than 12 months at the sale date). All amounts are AUD; proceeds are spread over a strategy's allocations by quantity as a cumulative difference, so they sum exactly to `price × units`.

### Unrealised gains

```
POST /portfolio/unrealised-gains
```

Request body (all optional):

```json
{ "live": true, "prices": { "<listing_id>": "<price>" }, "as_of_date": "YYYY-MM-DD" }
```

`as_of_date` defaults to today, and the report is the position **as at** that date: trades, sales, corporate actions, and AMIT adjustments (by their statement's year end) dated after it are excluded. One row per (listing, holding account), like the [overview](#overview). Response fields per holding: `listing_id`, `holding_account_id`, `quantity`, `total_cost_base`, `current_price`, `market_value`, `unrealised_gain_loss`, `cgt_discount_eligible_quantity` (units from parcels held strictly more than 12 months as at `as_of_date`), `price_as_of` (nullable), `price_unavailable` (nullable). With `live: true`, unpriced holdings are valued from the price source's latest quote — see [Live valuation](#live-valuation). `total_cost_base` is in AUD (see [FX conversion](#fx-conversion)); supplied prices are expected in AUD, so `market_value` and `unrealised_gain_loss` are AUD. Quantities are in the unit basis of `as_of_date` — re-based across any [share splits/consolidations or bonus issues](#corporate-actions) up to that date — and neither a split nor a bonus issue restarts the 12-month discount clock (the converted/bonus shares keep the original acquisition date; TD 2000/10, `docs/ato/bonus-shares.md`). A [scrip-for-scrip](#corporate-actions) replacement or [demerger](#corporate-actions) head/demerged parcel's discount clock likewise runs from its deemed (carried) acquisition date — the rollover's combined holding period.

### Realised gains

```
GET /portfolio/realised-gains
```

Returns one record per disposal: each sale trade that has at least one parcel allocation, plus each [rights sale](#selling-or-lapsing-rights). Response fields: `source` (`"Sell"` — an ordinary Sell trade — or `"RightsSale"`), `sale_trade_id` (the disposal's row id in its source's table: `trades` for `Sell`, `rights_sales` for `RightsSale`), `listing_id`, `holding_account_id` (the account the Sell happened in — one taxpayer either way, so totals are account-independent), `sale_date`, `proceeds`, `cost_base`, `capital_gain_loss`, `discount_eligible_gain` (gross gain from parcels held strictly more than 12 months), `non_discountable_gain` (gross gain from parcels held 12 months or less — the "other" method), and `capital_loss` (total losses from allocations sold below cost, as a positive amount). The three buckets satisfy `capital_gain_loss == discount_eligible_gain + non_discountable_gain − capital_loss`. `proceeds`, `cost_base`, and `capital_gain_loss` are in AUD: proceeds are converted at the sale's FX rate and cost base at the purchase's FX rate (see [FX conversion](#fx-conversion)). The cost base of the sold units is reduced by [return-of-capital](#corporate-actions) payments received while they were held — from acquisition up to the sale date — flooring at nil; payments after the sale don't touch them. An allocation's quantity is in the sale date's unit basis: a [share split/consolidation or bonus issue](#corporate-actions) between purchase and sale re-bases it back to as-acquired units for the cost-base pro-rating, and the discount holding period still runs from the original acquisition date (TD 2000/10; `docs/ato/bonus-shares.md`). A `RightsSale` row is the disposal of the rights themselves: proceeds `units × proceeds_per_right` and cost base `rights_cost` (nil for free rights), both converted at the sale month, with each anchoring allocation's discount clock running from its original parcel's (possibly deemed) acquisition date — sold rights inherit the original shares' acquisition date, unlike an exercised parcel whose clock restarts at exercise. A [scrip-for-scrip](#corporate-actions) exchange's or [demerger's](#corporate-actions) closing Sell is **excluded** — the rollover disregards its gain — as is a [transfer's](#transfers) transfer-out Sell (an own-account move is no disposal at all; a later sale of the transfer-in parcel uses the carried cost base and deemed acquisition date) — and a sale of a replacement (or demerger head/demerged) parcel uses the carried (apportioned) cost base, converted at the original acquisition month's rate, with the discount clock running from the deemed (carried) acquisition date, the rollover's combined holding period.

Each disposal also carries `parcels`: the individual parcel allocations behind its totals — `purchase_trade_id` (the anchor parcel for a `RightsSale`), `acquisition_date`, `units`, `cost_base`, `proceeds`, `capital_gain_loss`, and `discount_eligible` (held strictly more than 12 months at the sale date). These are the same per-allocation figures the disposal's own totals are summed from, sorted by acquisition date then `purchase_trade_id`, so a UI can drill from the disposal into which parcel contributed what without a separate request.

Sorted by `sale_date` ascending (then source, then id).

### Performance

```
POST /portfolio/performance
Body (optional): { "live": true, "prices": { "<listing_id>": "12.34", ... }, "as_of_date": "2026-06-30" }
```

Investment performance (not tax) per holding (listing × holding account) plus a final **OVERALL** row, valued at `as_of_date` (default: today; trades and income dated after it are ignored) with the supplied AUD prices. The report is cash-flow based: **out** — each Buy/DRP parcel's AUD cost on its trade date (converted at the acquisition month, or the deemed acquisition month for a rollover-created parcel); **in** — each Sell's AUD net proceeds, each distribution's cash (franked + unfranked + foreign source − foreign tax − TFN withholding; franking credits are not cash), and the holding's market value at `as_of_date`. Internal movements — [transfers](#transfers), [scrip-for-scrip exchanges, and demergers](#corporate-actions) — are valued **at the carried cost** within each holding (the source exits without gain; the destination carries the cost base, so the gain shows up where the parcels now sit) and are skipped entirely in the OVERALL row, which sees only external cash. AMMA statements attribute taxable income, not cash, and are excluded; a DRP reinvestment is both cash income and a same-sized purchase, so it nets out.

Response fields per row: `listing_id`, `ticker`, `holding_account_id`, `quantity_held` (as-of units; all three `null`/`"OVERALL"` on the total row), `invested`, `proceeds`, `income` (lifetime AUD figures), `market_value` (`quantity_held` × the supplied or live price), `total_return` (proceeds + income + market value − invested, AUD), `total_return_pct` (of invested), `money_weighted_return_pct` (annualised internal rate of return over the dated flows, actual/365, % p.a.), `income_yield_pct` (trailing 12 months' income / market value), `price_as_of` (nullable; absent on the OVERALL row), and `price_unavailable` (nullable). A still-open holding with no supplied price reports `null` for every market-dependent metric rather than a silently wrong figure; the OVERALL row likewise reports them only when every open holding is priced. With `live: true`, unpriced open holdings are valued from the price source's latest quote — see [Live valuation](#live-valuation).

### Net capital gain

```
GET /portfolio/net-capital-gain
```

Returns one record per Australian financial year (identified by the calendar year of 30 June), sorted ascending — the overall CGT position combining realised parcel gains with the CGT components attributed on AMMA statements. Realised gains are attributed by the sale's tax year (July = next FY); AMMA components by `tax_year_end_date`. A [scrip-for-scrip](#corporate-actions) exchange or [demerger](#corporate-actions) contributes nothing in the exchange/demerger year — the rollover disregards the gain; the deferred gain surfaces when the replacement (or head/demerged) parcels are eventually sold.

The assessable net capital gain is computed the ATO way:

1. Total the year's gross capital gains, split into **discount-eligible** (realised parcels held > 12 months + AMMA discount-method gains grossed up ×2 — the AMMA `cgt_discount_gains` value is the already-halved "discounted capital gain", so doubling it restores the gross gain + any **CGT event E10/G1** gain whose parcel was held > 12 months at the event date) and **non-discountable** (realised parcels held ≤ 12 months + AMMA indexation-method and other-method gains, neither of which gets the discount + any CGT event E10/G1 gain held ≤ 12 months).
2. Total the year's capital losses: realised losses, **plus the net capital loss brought forward from earlier years** — unused losses chain across the year series indefinitely (per the ATO), starting from the entered [opening carried-forward loss](#cgt-settings) (losses from before the first recorded year). An AMMA statement's `capital_losses_applied` is deliberately **not** counted: those losses were applied at the *trust* level before attribution — the statement's CGT amounts are already net of them, and a trust cannot distribute capital losses to members, so only the investor's own losses enter the netting (`docs/ato/amma-statement-guidance-notes.md`; `docs/ato/personal-investors-guide-managed-fund-distributions.md`, Step 4).
3. Apply losses against non-discountable gains first, then discount-eligible gains (taxpayer-favourable: the 50% discount falls on the largest possible remaining gain). Losses always apply before the discount.
4. **Net capital gain** = remaining non-discountable gain + 50% of the remaining discount-eligible gain. Unused losses are carried forward into the next year in the series.

The 50% rate is the **Australian-resident-individual** CGT discount — other taxpayer entity types (SMSF/complying super 33⅓%, company 0%, trust/partnership flow-through) are not modelled (see [Known limitations](#known-limitations)). Every row states this in its informational `taxpayer_basis` field rather than leaving the assumption implicit.

**CGT event E10**: when the cumulative AMIT cost base reductions (`amit_adjustments` × the AMMA per-unit `cost_base_adjustment`) on a parcel exceed its cost base, the cost base is floored at nil (in the portfolio, unrealised, and realised reports) and the excess is a capital gain in the income year the reducing AMMA statement applies to — added to the gain buckets above (discount-eligible vs not, per the holding period as at the statement's `tax_year_end_date`). The excess is converted to AUD at the parcel's buy-month rate. See `docs/ato/amit-cost-base-adjustments.md`.

**CGT event G1**: when a company's cumulative [return-of-capital](#corporate-actions) payments exceed a parcel's per-unit cost base, the cost base is floored at nil and the excess is a capital gain in the payment's income year — covering only the units still held at the payment date, and never producing a capital loss. The gain is added to the gain buckets above (discount-eligible vs not, per the holding period as at the payment date) and converted to AUD at the payment month's ATO rate (no manual fallback: a non-AUD payment with no rate fails loudly with `500`). See `docs/ato/cgt-non-assessable-payments.md`.

Response fields: `tax_year`, `discount_eligible_gains`, `other_gains`, `capital_losses` (all gross; `capital_losses` is only the losses arising that year), `capital_loss_brought_forward` (unused losses chained from earlier years, seeded by the `cgt_settings` opening balance), `net_discount_eligible_gain` and `net_other_gain` (after losses), `cgt_discount` (the 50% reduction applied = `net_discount_eligible_gain / 2`), `net_capital_gain`, `capital_loss_carried_forward` (losses left unused after offsetting all gains — the next year's brought-forward balance), `cgt_event_e10_gain`, `cgt_event_g1_gain` (informational: gross E10/G1 gains already included in the gain buckets), `taxpayer_basis` (informational: the individual-resident rate assumption above), and `disposals` — the year's realised disposals (per the [realised-gains report](#realised-gains), each with its own nested `parcels`) that fed the discount-eligible/non-discountable/loss buckets above, so a UI can drill from a year down through its disposals to the individual parcels. AMMA-attributed and CGT event E10/G1 gains have no parcel-allocation record and so are folded into the year's totals only, not into `disposals`. All amounts are AUD (AMMA amounts converted via the ATO rate for the month of `tax_year_end_date`, so a non-AUD amount with no rate fails loudly with `500`; see [FX conversion](#fx-conversion)).

```
GET /portfolio/net-capital-gain/export
```

The same per-year records as a downloadable tax-return-ready CSV (`Content-Type: text/csv; charset=utf-8`, `Content-Disposition: attachment; filename="net-capital-gain.csv"`): a header row naming the columns (the response fields above, **excluding `disposals`** — the csv writer rejects a record with a nested sequence, and the export was already the flat per-year record before the JSON drilldown was added), a **second header row carrying each column's ATO tax-return label**, then one record per financial year. An empty report still returns both header rows.

The label row's first cell is `ato_labels_2026`, naming the form year the mapping targets — the **Individual tax return 2026** (paper supplementary section; myTax shows the same labels). Labels shift year to year; the verified reference is `docs/ato/tax-return-labels-2026.md`. The mapping (columns not listed report at no label — intermediate workings or informational figures):

| Column | Label | Meaning |
|--------|-------|---------|
| `discount_eligible_gains` + `other_gains` | `18H (component)` | The two gross-gain columns **sum** to label 18H, *Total current year capital gains* (AMMA discount gains already grossed up ×2 in `discount_eligible_gains`) |
| `capital_losses`, `net_discount_eligible_gain`, `net_other_gain`, `cgt_discount` | `18 (working)` | Steps of question 18's calculation with no label of their own |
| `capital_loss_brought_forward` | `18V (prior year)` | Equals label 18V from the **previous** year's return |
| `net_capital_gain` | `18A` | *Net capital gain* |
| `capital_loss_carried_forward` | `18V` | *Net capital losses carried forward to later income years* |

### Pre-sale what-if

```
POST /portfolio/net-capital-gain/what-if
```

Dry-runs a **hypothetical** disposal through the [net capital gain](#net-capital-gain) computation and returns the disposal year's figures with and without it — **no rows are written**. The whole-of-income tax estimate is out of scope (consistent with the FITO decision); this is the CGT-side delta only. Request body:

```json
{ "listing_id": 1, "holding_account_id": 1, "units": "1500", "proceeds": "12000",
  "date": "2026-06-15",
  "allocations": [ { "purchase_trade_id": 2, "units": "1500" } ] }
```

`units` must be positive and `proceeds` (the total capital proceeds, **AUD**) non-negative. The sold units are drawn from the listing's [open parcels](#open-parcels) (restricted to `holding_account_id`'s parcels when supplied) via **exactly one** of:

- `allocations` — explicit per-parcel choices; each must name an open parcel of the listing with enough remaining units, and they must sum to `units` (`422` otherwise, as on a real [Sell](#sells))
- `strategy` — one of the [parcel-selection optimiser](#parcel-selection-optimiser)'s strategy names (`fifo`, `min_gain`, `max_discount`, `harvest_losses`); the allocations are derived the optimiser's way at the implied per-unit price `proceeds ÷ units`, rejecting `units` beyond the open quantity with `422`

The hypothetical's gain/loss buckets (classified per parcel exactly as the [realised-gains](#realised-gains) report would: discount when held strictly more than 12 months at `date`, the cost base from the adjusted open-parcel figures) are injected into the disposal's tax year and the full loss-chaining walk re-run, so earlier years' carried-forward losses (and the [opening balance](#cgt-settings)) apply in both scenarios.

Response fields: `tax_year` (the disposal's financial year), `strategy` (echoed when one was named), `hypothetical` (the disposal's own buckets: `proceeds`, `cost_base`, `capital_gain_loss`, `discount_eligible_gain`, `non_discountable_gain`, `capital_loss`), `allocations` (its per-parcel rows, derived or as supplied — the optimiser's allocation fields), and `years` — exactly two [net-capital-gain year records](#net-capital-gain) for the disposal year, each tagged `scenario` = `"without"` / `"with"`; a year with no recorded activity still yields both rows (zeros plus the correct brought-forward chain). Later years' changed carry-forward is **not** returned — re-run the main report with the disposal entered for the full series.

### Tax summary

```
GET /portfolio/tax-summary
```

Returns one record per Australian financial year (identified by the calendar year of 30 June), sorted ascending. Aggregates dividend income by `date_paid` (July = next FY) — except a trust distribution carrying an `entitlement_date`, which is attributed by that date instead (trust income is assessed in the year of present entitlement regardless of payment: ATO QC 23087, `docs/ato/trust-income-timing.md`; see [Income](#income)) — and AMMA statements by `tax_year_end_date`. Income rows on an **AMIT listing are excluded entirely** (every component — cash, withholding, offsets): they are [cash-only rows](#income) that fund the DRP chain, and for an AMIT the AMMA attribution is the only assessable record, reported on the `amma_*` lines — counting the cash too would double the year's income (the [AMIT cash cross-check](#amit-cash-cross-check) flags a cash year with no covering AMMA statement). All amounts are converted to AUD via the ATO rate (see [FX conversion](#fx-conversion)) before aggregating, using each record's `currency` and the month of the attribution date — `date_paid` (income; the governing `entitlement_date` for a trust row that has one) or `tax_year_end_date` (AMMA). Response fields include all income and AMMA components as separate fields for direct transfer to a tax return, plus the informational `taxpayer_basis` field stating the **Australian-resident-individual** assumption behind the hard-wired rates (the LIC capital gain deduction passed through here is the individual 50% figure; other entity types are not modelled — see [Known limitations](#known-limitations)).

**Franking-credit entitlement** (the at-risk holding-period rule, `docs/ato/you-and-your-shares-dividends.md`): `franking_credits` reports only *claimable* credits. In a year whose total attached credits (income + AMMA) reach A$5,000, each dividend's shares must have been held at risk for at least 45 days — 90 for a listing flagged `preference` — not counting the acquisition or disposal day; which shares were sold is identified **last-in first-out** (as the ATO mandates for this rule), regardless of the CGT parcel allocation chosen on the sale. Credits on entitled units that fail the test are reported in `franking_credits_denied` and excluded from `franking_credits` (the [franking at-risk report](#franking-at-risk) explains each denial — the failing window and units — and its what-if mode tests a contemplated sale before it is recorded). Below A$5,000 the small-shareholder exemption applies and nothing is denied. The test anchors on the income record's `ex_date` (falling back to `date_paid` when absent); AMMA-attributed credits count toward the threshold but are never themselves denied (an annual AMMA statement carries no per-distribution ex-date). A [demerger's](#corporate-actions) closing Sell and head replacement Buys are excluded from the walk — the head shares were never actually disposed of, so their at-risk days keep running across the demerger.

**Foreign income tax offset (FITO) cap** (`docs/ato/fito-limit.md`): `foreign_tax_offsets` (income `foreign_tax_paid` + AMMA `foreign_tax_credits`, in AUD) reports the offset claimable without the ATO's offset-limit calculation — up to the A$1,000 de-minimis per year. A year's foreign tax above A$1,000 is reported in `foreign_tax_offset_excess` and excluded from `foreign_tax_offsets`: the limit calculation needs the taxpayer's full income-tax position (employment income, deductions, Medicare levy), which is outside this system's data, so the excess is claimable only to the extent the taxpayer's own offset-limit calculation supports it.

**Employee share scheme discount** (`docs/ato/employee-share-schemes.md`): [ESS statements](#ess-statements) are aggregated by `taxing_point_date` (July = next FY) into `ess_discount_assessable` — the Item 12 assessable discount (labels D + E + F + G) **net of** the applied $1,000 taxed-upfront reduction — reported separately from dividend/trust income and in AUD (foreign-currency statements converted via the ATO rate for the taxing-point month; no rate ⇒ fails loudly with `500`). A label carrying a [statement-AUD override](#ess-statements) (the employer statement's stated AUD figure, converted at the release-date spot rate — what the ATO prefill carries) is reported **verbatim** instead of converted, so the summary matches the lodged return; labels without one keep the RBA conversion. `ess_taxed_upfront_reduction` surfaces the reduction applied (`min(A$1,000, the year's taxed-upfront-eligible discount)`); like the FITO cap, the tool applies the de-minimis but the **≤A$180,000 adjusted-taxable-income eligibility test is the user's responsibility** (an ineligible taxpayer adds the reduction back). `ess_foreign_source_discount` is the foreign-source portion (label A), a memo already within `ess_discount_assessable`. The ESS TFN amounts withheld join the existing `tfn_withholding_tax` line.

**Interest income** (`docs/ato/tax-return-labels-2026.md`): [interest income](#interest-income) records are aggregated by `date_paid` (July = next FY), each year's gross in AUD (foreign-currency amounts converted via the ATO rate for the month paid; no rate ⇒ fails loudly with `500`). An **Australian-source** row lands in `interest_income` — question 10's gross interest (10L), including any TFN amount withheld, the withheld amount itself joining the existing `tfn_withholding_tax` line. A row marked **`foreign_source`** (a foreign broker's cash / money-market fund) lands in `foreign_interest_income` instead — assessable foreign source income (20E) — with its `foreign_tax_paid` joining the `foreign_tax_offsets` FITO line (counting toward the A$1,000 de-minimis like any other foreign tax). Write-time validation keeps each withholding kind on the matching source, so the routing can't claim an offset a row doesn't support.

**Investment-expense deductions** (`docs/ato/investment-income-deductions.md`, `docs/ato/dividend-income-deductions.md`): [investment expenses](#investment-expenses) are aggregated by `date_incurred` (July = next FY) into per-type lines — `deductions_loan_interest`, `deductions_management_fee`, `deductions_advice_fee`, `deductions_account_keeping_fee`, `deductions_subscription`, `deductions_other` — and `deductions_total`, each the recorded post-apportionment deductible amount in AUD (foreign-currency expenses converted via the ATO rate for the month incurred; no rate ⇒ fails loudly with `500`). `gross_assessable_investment_income` sums the report's existing assessable income lines (`dividends_assessable` + `interest_income` + `foreign_interest_income` + `foreign_source_income` + the six AMMA income components), and `net_assessable_investment_income` = `gross_assessable_investment_income − deductions_total`. The gross figures are retained unchanged. The gross deliberately excludes the franking-credit gross-up and FITO (offset lines), conduit foreign income (NANE), the ESS discount (employment income), and capital gains (the [net capital gain](#net-capital-gain) report); the LIC capital gain deduction is distinct and is not folded into the net figure.

```
GET /portfolio/tax-summary/export
```

The same per-year records as a downloadable tax-return-ready CSV (`Content-Type: text/csv; charset=utf-8`, `Content-Disposition: attachment; filename="tax-summary.csv"`): a header row naming the columns (the response fields, in field order, from `tax_year` through `taxpayer_basis`), a **second header row carrying each column's ATO tax-return label**, then one record per financial year. An empty report still returns both header rows.

The label row's first cell is `ato_labels_2026`, naming the form year the mapping targets — the **Individual tax return 2026** (paper return + supplementary section; myTax shows the same labels). Labels shift year to year; the verified reference is `docs/ato/tax-return-labels-2026.md`. The mapping:

| Column | Label | Meaning |
|--------|-------|---------|
| `dividends_assessable` | `11S + 11T` | The single column is unfranked (11S) + franked (11T) dividends summed; split per the underlying income records |
| `interest_income` | `10L` | Australian gross interest, including any TFN amount withheld (the withheld amount is inside the `tfn_withholding_tax` column) |
| `foreign_interest_income`, `foreign_source_income`, `amma_foreign_income` | `20E + 20M` | Assessable foreign source income (20E gross; 20M is its net-of-expenses counterpart — with no foreign-side expenses recorded the two are equal); foreign-source interest reports here, never at 10L |
| `lic_capital_gain_deduction` | `D8` | The 50% LIC capital gain deduction is claimed at question D8 *Dividend deductions* |
| `amma_australian_interest`, `amma_dividends_unfranked`, `amma_net_rent`, `amma_other_income` | `13U` | Non-primary production trust income components |
| `amma_franked_dividends` | `13C` | *Franked distributions from trusts* — on the return 13C **includes** the attached franking credits; this column is the statement's franked-distribution component |
| `amma_cgt_*` | `18 (working)` | Inputs to question 18 — the [net-capital-gain export](#net-capital-gain) carries the final 18H/18A/18V figures |
| `franking_credits` | `11U / 13Q` | Claimable credits from direct dividends (11U) and trust distributions (13Q), summed |
| `foreign_tax_offsets` | `20O` | FITO within the A$1,000 de-minimis (the excess column is unlabelled — claimable only per the taxpayer's own offset-limit calculation) |
| `tfn_withholding_tax` | `10M / 11V / 13R / 12C` | TFN credits from interest, dividends, trust distributions, and ESS discounts, summed |
| `ess_discount_assessable` | `12B` | *Total assessable discount amount*, already net of the applied $1,000 taxed-upfront reduction |
| `ess_foreign_source_discount` | `12A` | Foreign-source ESS discount memo (for the question 20 FITO claim) |
| `deductions_*`, `deductions_total` | `D7 / D8` | Expenses of earning interest income at D7, dividend/distribution income at D8 — the per-type split between the two questions is the taxpayer's, per where the income belongs |

Unlabelled columns are informational or derived: `franking_credits_denied` and `foreign_tax_offset_excess` (amounts *excluded* from the claimable lines), `ess_taxed_upfront_reduction` (already inside 12B), `amma_capital_losses_applied` (trust-level losses the AMMA's gains are already net of — not a loss of the taxpayer's, so it feeds no question-18 figure), `gross_assessable_investment_income` / `net_assessable_investment_income` (derived totals), `taxpayer_basis`.

### Exchange MIC validation

```
GET /reports/exchange_mic_validation
```

Validates each curated exchange's MIC against the `mic_registry` (the imported ISO 10383 list) — **non-blocking**: writes to `exchanges` are never rejected, this only surfaces MICs worth a second look. Returns one record per exchange (sorted by MIC) with fields: `mic`, `exchange_name`, `registry_status` (`ok` = active in the registry, `expired` = present but EXPIRED, `unknown` = no registry entry, i.e. a typo or the registry hasn't been imported yet), `iso_status` (raw ISO `ACTIVE`/`UPDATED`/`EXPIRED`, or null when unknown), and `expiry_date`. With an empty registry every exchange is `unknown`.

### Settlement holiday coverage

```
GET /reports/settlement_holiday_coverage
```

Flags every trade whose `[date, settlement_date]` window is not fully inside its exchange's seeded holiday coverage (see [Exchange holidays](#exchange-holidays)) — **non-blocking**: trade writes are never rejected, this only surfaces settlement dates that may have been computed against an incomplete calendar (weekend-only skipping). Returns one record per affected trade (sorted by ticker, then date, then trade id) with fields: `trade_id`, `listing_id`, `ticker`, `mic`, `trade_type`, `date`, `settlement_date`, `coverage_status` (`outside_holiday_coverage` = the window extends beyond the seeded years, `no_holiday_coverage` = the exchange has no seeded holidays at all), and the exchange's coverage span `coverage_start`/`coverage_end` (1 Jan of the earliest seeded holiday's year to 31 Dec of the latest's; null when there is no coverage). Trades fully inside coverage are omitted — an empty report means every settlement window was computed against a complete calendar. Entering the missing holiday years clears the corresponding alerts. Trades on exchange-less ([Crypto](#listings)) listings are skipped — they settle same-day with no holiday calendar, so there is no coverage to be outside of (the [exchange MIC validation report](#exchange-mic-validation) likewise has nothing to validate for them: it checks curated exchanges, and a Crypto listing has none).

### Tax-deferred E4 cross-check

```
GET /reports/e4_cross_check
```

Flags every trust [income](#income) row carrying a non-zero `tax_deferred_amount` whose listing has **no** `ReturnOfCapital` [corporate action](#corporate-actions) dated in the row's financial year — for a non-AMIT unit trust the statement's tax-deferred amount is a CGT event E4 cost-base reduction (`docs/ato/cgt-non-assessable-payments.md`), modelled as the `ReturnOfCapital` action, so a recorded amount with no matching action means the cost base is silently overstated. **Non-blocking**: income writes are never rejected — this only surfaces reductions still to be entered. The row's financial year is its assessment year (the governing `entitlement_date` when set, else `date_paid` — the [tax summary](#tax-summary)'s attribution rule). Returns one record per affected row (sorted by ticker, then date paid, then income id) with fields: `income_id`, `listing_id`, `ticker`, `date_paid`, `tax_deferred_amount` (verbatim, in the record's currency), and `tax_year` (the FY the action is expected in, identified by the calendar year of its 30 June end). Rows whose listing has a same-FY action, and rows with no or a zero amount, are omitted — an empty report means every recorded tax-deferred amount has its E4 reduction entered; entering the matching action clears a row. (AMIT funds are unaffected: their cost-base movement is driven solely by the AMMA `cost_base_adjustment` — see [AMMA statements](#amma-statements) — and any tax-deferred figure on an AMMA statement stays informational.)

### AMIT cash cross-check

```
GET /reports/amit_cash_cross_check
```

Flags every financial year in which an AMIT listing received cash distributions ([cash-only income rows](#income)) but has **no [AMMA statement](#amma-statements)** whose `tax_year_end_date` falls in that year. AMIT cash rows are excluded from the [tax summary](#tax-summary) — the AMMA attribution is the assessable record — so a cash year with no AMMA entered would silently drop that income from the return. **Non-blocking**: income writes are never rejected — this only surfaces statements still to be entered; entering the fund's AMMA clears the row. The converse is not flagged: an AMMA year with no cash rows is fine (the fund can be held without receiving or recording cash that year), and non-AMIT listings are never this report's business. Each row's financial year is its assessment year (the governing `entitlement_date` when set, else `date_paid` — the tax summary's attribution rule). Returns one record per affected (listing, year) pair (sorted by ticker, then year) with fields: `listing_id`, `ticker`, `tax_year` (the FY the AMMA is expected for, identified by the calendar year of its 30 June end), `cash_rows` (how many cash income rows the year has), and `cash_total_aud` (the year's gross cash components in AUD — the income that would go unreported). An empty report means every AMIT cash year has its attribution entered.

### Wash sales

```
POST /reports/wash_sales
```

Flags the wash-sale fact pattern the ATO warns may have the capital loss cancelled under Part IVA (TR 2008/1, `docs/ato/wash-sales.md`): every **loss-realising Sell** — any of its allocations realised a capital loss per the [realised gains](#realised-gains) report, so the rollover/transfer exclusions and the full adjusted-cost-base pipeline apply unchanged — with a **Buy or DRP of the same listing dated within the window either side**, across **all holding accounts** (a repurchase in another account of the same beneficial owner changes nothing economically). **Non-blocking and advisory**: writes are never rejected, the flagged loss still counts in every CGT report, and the pattern is not unlawful per se — TR 2008/1's own Example 6 is a repurchase 3 days later that survives Part IVA because it was market-driven, while Example 2's planned 24-hour round trip fails. Whether Part IVA could apply is a facts-and-circumstances judgment the report leaves to the taxpayer; it only makes the pattern visible.

The optional JSON body `{"window_days": n}` sets the scan window (`n ≥ 1`; `422` otherwise); absent or an empty body defaults to **30 days** — a review convention, not an ATO bright line (the ruling has no statutory window). Returns one record per loss-Sell × nearby-Buy pair (a Sell with several nearby Buys yields several rows; no pairs ⇒ an empty report) with fields: `sale_trade_id`, `listing_id`, `ticker`, `sale_holding_account_id`, `sale_date`, `capital_loss` (the sale's realised loss across its loss-making allocations, AUD, positive), `buy_trade_id`, `buy_holding_account_id`, `buy_date`, `buy_quantity`, and `days_apart` (`buy_date − sale_date`; negative = acquired before the sale). Provenance Buys that merely continue or relocate an existing holding never match (holding-account transfer-ins, scrip-for-scrip replacements, demerger Buys, inheritance Buys); rights-exercise and ESS-vesting Buys do — they are genuine new acquisitions. Symmetrically, a [transfer](#transfers)'s network-fee disposal Sell is never a candidate — the disposal is compelled by the transfer (a dominant non-tax purpose, timed by the transfer, with the fee units never re-acquired), so it is not the TR 2008/1 fact pattern; its capital loss still counts in every CGT report. Matching is otherwise by date pattern only: the Buy a Sell drew its own allocations from can match, and the report does not judge purpose.

### Franking at-risk

```
GET /reports/franking_at_risk
```

Explains the [tax summary](#tax-summary)'s otherwise-silent franking-credit denials: one record per dividend whose entitled shares fail the at-risk holding-period walk (45 days, 90 for a `preference` listing; LIFO identification — the same `docs/ato/you-and-your-shares-dividends.md` rule the tax summary applies, computed by the same shared code so the two cannot disagree). **Non-blocking**: nothing is rejected; an empty report means every attached credit is claimable. Fields: `income_id`, `listing_id`, `ticker`, `tax_year`, `date_paid`, `ex_date` (the payment date when no ex-date was recorded), `required_days` (45/90), `window_end` (the qualification window runs from `ex_date` to here — a disposal inside it with fewer than `required_days` at-risk days disqualifies; one after it cannot), `entitled_units`, `disqualified_units`, `credits_attached` (AUD), `credits_at_risk` (the disqualified share of the credits per the walk), `credits_denied` (what the tax summary actually excludes — equal to `credits_at_risk`, or zero under the exemption), and `status`: `denied`, or `exempt_small_shareholder` (the year's attached credits are under A$5,000, so the rule doesn't apply — the row still explains the failing walk, since the exemption is year-wide and lapses as more credits arrive). The `denied` rows sum, per financial year, to the tax summary's `franking_credits_denied`.

### Health

```
GET /reports/health
```

Operational health / data-freshness in one read (a single read transaction), driving the web UI's cross-view banner: a failing import or silently aging price/FX data is surfaced on every screen rather than only when the Jobs page is opened. Returns `{ "latest_price_date", "prices_stale", "latest_fx_month", "fx_stale", "failed_jobs" }`:

- `latest_price_date` — the newest [closing price](#closing-prices) date stored with status `ok` across every listing (`null` when none). `prices_stale` is true when it is more than **3 business days** (a coarse Mon–Fri count — deliberately not the per-exchange holiday calendar; this is a freshness alarm, not a settlement calculation) before the server's current date.
- `latest_fx_month` — the newest imported [RBA FX rate](#rba-fx-rates) month, `YYYY-MM` (`null` when none). `fx_stale` is true when it is older than the previous calendar month (the RBA publishes month M's F11 rates shortly after M ends, so an older latest month means the weekly import has stopped landing new data).
- `failed_jobs` — every job whose **most recent** recorded run failed, as `{ "name", "finished_at", "error" }` (sorted by name); a job that failed and then succeeded is recovered and not listed.

A database with no prices or FX rates at all reports `stale = false` for that series — nothing has decayed, so a fresh install shows no banner; an import that breaks before ever succeeding surfaces through `failed_jobs` instead.

```
POST /reports/franking_at_risk/what-if
```

The contemplated-sale mode, surfaced next to the Sell flow in the web UI: body `{"listing_id": n, "sale_date": "YYYY-MM-DD", "units": "n"}` (all required; non-positive units ⇒ `422`) re-runs the holding-period walk for every franked dividend of the listing with that hypothetical Sell injected (in date order, after any recorded same-day trades), and returns the dividends whose denial would **grow** — nothing is written. Fields: `income_id`, `listing_id`, `ticker`, `tax_year`, `ex_date`, `required_days`, `window_end` (selling after this date can no longer disqualify the dividend — the lever for timing the disposal), `credits_attached`, `disqualified_units_now`, `disqualified_units_after_sale`, `additional_credits_at_risk` (the walk's denial with the sale minus without it, AUD), and the same `status` as above. A sale dated after every window end, or one that disqualifies nothing, returns an empty report.

### Row history

```
POST /reports/row_history
```

The read side of the **append-only audit trail** (`row_history` in [SCHEMA.md](SCHEMA.md); aligns with the ATO record-keeping guidance in `docs/ato/cgt-keeping-records-shares.md`): database triggers record the prior row whenever an audited table's row is updated or deleted — in the writing transaction itself, so no write path can bypass it, a rejected write leaves no phantom entry, and cascade deletes are recorded too. Entries are kept forever and the trail itself cannot be rewritten (append-only, database-enforced), so an accidental edit to a historical fact — which would silently change prior-year cost bases and tax figures — can be noticed and reconstructed.

Body `{"table": "trades", "row_id": 1}` (both required): `table` must be one of the audited tables (the `row_history.table_name` enum in SCHEMA.md; anything else ⇒ `422` naming the valid list). Returns the row's recorded history **newest first**, one JSON object per prior version: `history_id` (the trail entry's own id), `operation` (`UPDATE` | `DELETE`), `changed_at` (RFC 3339 UTC, millisecond precision), followed by every column of the audited table with its pre-write value (for `attachments` the `content` BLOB is excluded — `filename`/`byte_size`/`checksum` still identify the file). A row never updated or deleted returns `[]` — INSERTs are not recorded, so an empty trail means the row still reads exactly as first entered.

# Known limitations

Deliberate scope decisions (2026-06-07), documented rather than modelled:

- **Taxpayer entity type** — all tax figures assume an **Australian-resident individual**: the 50% CGT discount and the 50% LIC capital gain deduction. The rates for other entity types (SMSF/complying super 33⅓%, company 0%, trust/partnership flow-through taxation) are not modelled. Every tax-summary and net-capital-gain row carries the assumption in its `taxpayer_basis` field.
- **Cost base elements** — only cost-base elements 1 (acquisition) and 2 (incidental costs: brokerage + GST) are captured. Element 3 (ownership/holding costs), element 4 (capital improvements), and element 5 (title/defence costs) are not recordable — for listed shares they rarely apply, and element-3 borrowing costs are typically claimed as deductions instead (which excludes them from the cost base anyway). Consequently the ATO **reduced cost base** (used for capital losses; excludes element 3) is identical to the cost base by construction and is not modelled separately. See `docs/ato/cgt-cost-base.md`.
- **One taxpayer** — all holdings belong to a single taxpayer. [Holding accounts](#holding-accounts) partition custody/location (e.g. employer share plan vs personal broker) within that one taxpayer; a taxpayer-level ownership dimension (Individual / Joint / SMSF / Family Trust, each a separate CGT taxpayer) is not modelled.
- **Rights issues** — the modelled case is rights **issued free to the holder over post-CGT original shares**, exercised ([exercise](#exercising-a-rights-issue)) or disposed of ([sell rights](#selling-or-lapsing-rights), which also covers a renounceable-offer retail premium per TR 2017/4). **Pre-CGT original shares** (the market-value uplift on exercise; the disregarded gain on sale) and **non-renounceable-offer retail premiums** (an unfranked dividend per TR 2012/1 — enter as [income](#income)) are not modelled. See `docs/ato/rights-issues.md`, `docs/ato/retail-premiums.md`.
- **DRP partial participation** — [enrolment](#drp-enrolments) is all-or-nothing per (listing, holding account): a registry plan that reinvests only a portion of a holding's units is not modelled.
- **Employee share schemes** — both sides of an ESS interest are modelled: the assessable discount via [ESS statements](#ess-statements) (the Item 12 labels, reaching the [tax summary](#tax-summary)) and the cost-base-reset Buy via the [Vest](#vesting-an-ess-statement) operation (or entered manually as a Buy at the vest-date market value). The residual limits are: **unvested grants are not tracked** (they are not shares), and the $1,000 taxed-upfront reduction's **≤A$180,000 income-test eligibility is the user's responsibility** (the tool applies the de-minimis but can't see the taxpayer's whole income position — see the [tax summary](#tax-summary)).
- **Inherited parcels** — the beneficiary's side of a deceased-estate transfer is modelled via [Inheritances](#inheritances) (cost base per QC 66053, discount clock per s 115-30). The **estate/LPR side is not**: the executor's own return, assets the executor sells to pay debts (the Maria example's shares), and assets passing to a foreign resident / charity / super fund (CGT in the deceased's date-of-death return) are out of scope — only parcels that pass to the beneficiary are recorded. The **market value at death** for a pre-CGT asset is user-supplied (as valuations are elsewhere), and the pre-21-September-1999 **indexation alternative** is not modelled (the 50% discount is used throughout).
- **Crypto assets** — investment crypto is modelled as the exchange-less [`Crypto` listing](#listings) flowing through the ordinary CGT machinery (`docs/ato/crypto-cgt.md`). A **crypto-to-crypto swap** is a CGT event entered manually as a Sell at the market-value proceeds plus a Buy of the acquired asset at the same value; **staking rewards and airdrops** are entered manually (an income row plus a Buy at receipt-date market value). Chain splits/forks, wrapping, and the personal-use-asset exemption are not modelled. **Foreign-currency cash balances** (Div 775 forex gains — ordinary income, not CGT) are deferred to a separate specification.
- **Intraday prices** — the [closing-price history](#closing-prices) stores one closing/reference price per listing per day; intraday prices are not stored. A back-dated fact does not auto-backfill price history — it only marks the affected [report snapshots](#report-snapshots) stale; the daily jobs self-heal only their bounded windows (7 trading days of prices, 14 calendar days of snapshots), so older history is backfilled on demand via `POST /closing_prices/backfill` (then regenerate the affected snapshots via `POST /report_snapshots/generate` or `regenerate_all`).
- **Statement entry** — the income form's franking selector auto-computes franking credits at the **30% corporate rate** printed on typical statements only; 25% base-rate-entity dividends and partially franked payments are entered via the advanced component fields. Statement figures are keyed in manually — there is no statement parsing/import.
- **Gifts / off-market related-party transfers** (2026-06-10) — a gift of shares or crypto (or any nil-consideration / non-arm's-length transfer) is a **CGT disposal at market value** under the market-value substitution rule (`docs/ato/capital-proceeds-market-value-substitution.md`): the giver's capital proceeds are the asset's market value at the time of the gift, and the recipient's first-element cost base is likewise that market value. There is no dedicated gift entry path — enter a gift out as a manual Sell at market-value proceeds, and a gift in as a manual Buy at market-value cost.
- **Pre-CGT holdings** (2026-06-10; enforced at write time 2026-07-13) — a parcel acquired before **20 September 1985** is outside CGT entirely (gains and losses on it are disregarded); pre-CGT holdings are not modelled, and they **cannot be entered**: a [trade](#trades) or [Sell](#sells) dated before 20 September 1985, and an [inheritance](#inheritances) whose date of death pre-dates it (the parcel would be pre-CGT in the beneficiary's own hands under s 115-30), are rejected with `422` — so the system can no longer be handed a parcel it would wrongly compute a capital gain or loss on. (The one modelled pre-CGT interaction is an inherited parcel that was pre-CGT *in the deceased's hands*: [Inheritances](#inheritances) applies the market-value-at-death cost base, and the parcel is post-CGT in the beneficiary's hands.)
- **Indexation method** (2026-06-10) — for an asset acquired before **21 September 1999** an individual may index the cost base for inflation (frozen at the 30 September 1999 CPI) *instead of* applying the 50% discount (`docs/ato/indexing-the-cost-base.md`). The discount almost always gives an individual the better result, so indexation is not modelled — the 50% discount is used throughout (the inherited-parcels entry above states the same for the deceased-estate case).
- **RSU dividend equivalents** (2026-06-12) — employer share plans accrue **dividend equivalents on unvested RSU grants** (cash reflecting the dividends the shares earned during vesting, paid at release). Under TD 2017/26 a dividend equivalent payment is **ordinary income when paid** (remuneration, s 6-5) — not a dividend, not part of the ESS discount, and carrying no franking (`docs/ato/ess-dividend-equivalents.md`). They are not modelled: unvested grants are not tracked (see the employee-share-schemes entry above), so no entry path computes or classifies them — a dividend equivalent **paid out in cash is enterable manually as an [income](#income) row** if the user wants it aggregated here.
- **Settlement-window forex on foreign-currency trades — CGT events K10/K11** (2026-06-12) — under the default forex 12-month rule (`docs/ato/forex-cgt-12-month-rule.md`, QC 17062), the currency movement between a foreign-currency trade's contract date and its settlement payment is not ignored: on an **acquisition** it adjusts the parcel's cost base (the Art Ltd example), and on a **disposal** it is a separate **non-discountable capital gain (CGT event K10) or capital loss (CGT event K11)** (the Eleanor example). This system computes neither — settlement-window forex outcomes are the taxpayer's manual adjustment. The interaction with the [spot-rate override](#fx-conversion) is what sizes the omission: with monthly rates, a T+2 settlement inside the same rate month translates both legs at the same rate, so the component is **nil by construction**; entering per-leg spot rates (`spot_fx_rate`) is what makes a settlement-window movement visible, and a settlement crossing a rate month silently drops it either way. A material K10/K11 outcome can be approximated manually — fold a K10/K11 into the entered figures, or record the K10 gain's cost-base effect on the Buy — but no entry path computes or classifies it.
- **Cost-base FX timing — AMIT/return-of-capital reductions convert at the acquisition-month rate** (2026-07-13) — the cost-base pipeline converts a non-AUD parcel's entire cost-base breakdown to AUD at **one rate: the parcel's (possibly deemed) acquisition month** (see [FX conversion](#fx-conversion)) — including the AMIT (CGT event E10) and return-of-capital (CGT event G1) **reductions**, which arose in later, possibly very different, rate months. Strictly, the s 960-50(6) translation rules convert each amount at its own transaction time (`docs/ato/forex-common-transactions.md`, QC 18322 — Lisa's cost base and proceeds each translate at their own date), so each reduction would convert at its own payment/period month. This leaves a deliberate asymmetry in the [net capital gain](#net-capital-gain) report: a return-of-capital payment's *excess* over the cost base (the G1 capital gain) converts at the **payment month**, while the same payment's *reduction* inside the cost base converts at the **acquisition month** (the E10 excess uses the buy-month rate, consistent with the cost base). The single rate keeps each parcel's cost-base breakdown internally consistent — initial − reductions = adjusted holds in AUD exactly as in the native currency — and the simplification only bites on a **non-AUD holding receiving non-AUD AMIT/return-of-capital reductions**, which in practice does not arise (E10/G1 events occur on AUD funds). A taxpayer with a material foreign-currency reduction adjusts the cost base manually at the payment-period rate.
- **Server-side pagination** (2026-06-08) — the list and report endpoints always return the **full** result set as one JSON array; there is no server-side paging (`limit`/`offset`/cursor) of the payload. The web UI paginates **client-side** (the shared table renders one 50-row page at a time over the whole fetched set), so this addresses rendering/usability, not payload size — a very large table still transfers the entire array.

# Response codes

| Code | Meaning |
|------|---------|
| `200 OK` | Successful GET or report POST (JSON; the report `/export` endpoints return `text/csv`, an attachment content download returns its stored content type) |
| `201 Created` | DRP reinvestment trade created via `POST /income/:id/reinvest`, a rights-exercise trade created via `POST /corporate_actions/:id/exercise`, a rights sale recorded via `POST /corporate_actions/:id/sell_rights`, a buy-back participation (Sell + dividend income) created via `POST /corporate_actions/:id/participate`, a scrip-for-scrip exchange (closing Sell + replacement parcels) created via `POST /corporate_actions/:id/exchange`, a demerger (closing Sell + head and demerged parcels) created via `POST /corporate_actions/:id/demerge`, a worthless-shares loss (closing Sell at nil proceeds) recognised via `POST /corporate_actions/:id/recognise`, a holding-account transfer (transfer-out Sell + transfer-in parcels) created via `PUT /transfers/:id`, or an attachment uploaded via `POST /attachments` |
| `204 No Content` | Successful PUT or DELETE, or a job run via `POST /jobs/:name` |
| `400 Bad Request` | Malformed path parameter (e.g. an `exchange_holidays` `:date` that is not `YYYY-MM-DD`) |
| `404 Not Found` | Resource does not exist |
| `405 Method Not Allowed` | Write attempted on a read-only path (e.g. `parcel_allocations`) |
| `413 Payload Too Large` | Uploaded attachment exceeds the 25 MB per-file limit |
| `422 Unprocessable Entity` | Business rule or constraint violation (e.g. over-allocation, wrong trade type, under-allocated Sell, a degenerate trade/Sell figure (non-positive `quantity` or `fx_rate`, negative `average_price`/`brokerage`/`gst_on_brokerage`, a `settlement_date` before the trade date, a pre-CGT trade date — before 20 September 1985), a Sell allocation with a zero or negative `quantity_allocated`, a negative money amount on an income or interest-income row, an interest-income row whose withholding doesn't match its source classification (foreign tax on an Australian-source row, or TFN withholding on a foreign-source row), deleting or shrinking a Buy/DRP that a parcel allocation, AMIT adjustment, or reinvestment link still relies on, editing a reinvest-created DRP trade (undo the reinvestment via `DELETE /income/:id/reinvest` instead), changing a Buy/DRP's listing while Sell allocations or AMIT adjustments reference it, unparseable FX or MIC feed, a write referencing an unrecognised currency / unknown exchange / listing, a Crypto listing with an exchange or an unrecognised digital-token ticker, a non-Crypto listing without an exchange, a duplicate exchange-less ticker, an attachment upload with no/multiple owners or an unsupported content type, an attachment list combining `include_linked` with no `trade_id` filter or with another owner filter, a negative / non-singleton `cgt_settings` opening capital loss, an overlapping or empty DRP enrolment period, reinvesting a distribution no enrolment period covers, a reinvest `units` that is not positive or whose `units × price` is a full unit-step (at the stated precision) or more off the available cash (the response body carries both figures), undoing a reinvestment that doesn't exist, whose DRP trade is drawn on by a Sell allocation or AMIT adjustment, or that is not the residual chain's latest trade (undo runs last-in-first-out), deleting a reinvested distribution before undoing its reinvestment, or a corporate action with a non-positive `amount_per_unit`, a missing/non-positive split/bonus/rights/demerger ratio, exercise price, or buy-back price, a buy-back dividend that is negative or exceeds the price, a franking credit without a dividend, a non-positive market value, a demerger cost-base percentage missing or outside (0, 100), a scrip cash component that is partial (cash per old unit, market value, and currency come together) or non-positive, a payload mixing the per-type fields, or an unrecognised `action_type`; a rights exercise that is not against a RightsIssue, has non-positive units or a negative rights cost, is dated before the record date, or exceeds the remaining entitlement (shared with rights sales); a rights sale that is not against a RightsIssue, has non-positive units or negative proceeds/rights cost, is dated before the record date, whose allocations are empty or don't sum to the units, anchor to a parcel that is not a Buy/DRP of the issue's listing held before the record date, or exceed a parcel's or the holding's entitlement; editing or individually deleting a parcel Buy that anchors a rights sale; a buy-back participation that is not against a BuyBack, has non-positive units, is dated before the buy-back date, or fails a Sell-side invariant; a scrip-for-scrip exchange that is not against a ScripForScrip, is already exchanged, has nothing held, or whose original listing traded on/after the exchange date — or a ScripForScrip/Demerger whose replacement/demerged listing is missing, unknown, or the same as the original; a demerge that is not against a Demerger, is already demerged, has nothing held, or whose head listing traded on/after the demerger date; a worthless-shares recognise that is not against a WorthlessShares, is already recognised, has nothing held, or whose listing traded on/after the event date; editing a rights-exercise trade, a buy-back participation Sell, a buy-back dividend income row, any scrip-for-scrip exchange or demerger trade, or a worthless-shares recognise closing Sell, deleting a group trade individually or a group whose replacement parcels are still drawn on, or editing/deleting a RightsIssue, BuyBack, ScripForScrip, Demerger, or WorthlessShares that exercise/participation/exchange/demerge/recognise trades or rights sales still reference; a Sell allocation consuming a parcel in a different holding account from the Sell's, of a different listing than the Sell's, or dated after the sale date, an AMIT adjustment whose trade and statement sit in different holding accounts, an AMMA statement whose `tax_year_end_date` is not a 30 June date, a duplicate holding-account name, deleting a holding account that still holds data (or the seeded default account), a transfer whose source and destination accounts are the same, whose id already exists, with no allocations or a wrong-listing parcel, editing or individually deleting a transfer-group trade, or deleting a transfer whose transfer-in parcels are still drawn on; an inheritance with a non-positive quantity or negative cost base / LPR expenditure, whose date of death is pre-CGT (before 20 September 1985 — the parcel would be pre-CGT in the beneficiary's hands), whose deceased-acquisition date is missing/extra for its cost-base rule, pre-CGT under `DeceasedCostBase`, or after the death, whose LPR expenditure and date are not supplied together or pre-date the death, editing or individually deleting its linked parcel Buy via `/trades`, or editing/deleting an inheritance whose parcel is still drawn on; a closing-price re-fetch for a day whose close is not final or that is not a trading day, or a backfill whose `from` is after `to` or whose range has no complete trading day; a report-snapshot generation blocked by a missing/errored stored price, a close that is not final yet, an FX-rate gap too old for the 2-month valuation fallback, or a date nothing was held on; a `statement_total` that does not reconcile with the trade's own figures, neither exactly nor cent-rounded — `quantity × price + brokerage + GST` for a Buy/DRP, `−` for a Sell (the response body carries the computed figure) — or one supplied when the trade and brokerage currencies differ; a `spot_fx_rate` that is not positive or is supplied on an AUD-currency trade or Sell; an income `amount_per_security` / `securities_held` supplied without the other, or whose cent-rounded product does not equal the gross cash components (the response body carries the computed product); an income `entitlement_date` supplied on a non-trust row (a dividend is assessed when paid — present entitlement only shifts trust distributions); an income `tax_deferred_amount` that is negative or supplied on a non-trust row (a company's non-assessable payment is entered as a ReturnOfCapital corporate action directly); an ESS statement-AUD override supplied on an AUD-denominated statement, or a vested ESS statement edit that changes a field its vest Buy was created from; an income row on an AMIT listing that is not trust income, carries a non-zero notional component — `franking_credits`, `lic_capital_gain_deduction`, `conduit_foreign_income` (the AMMA statement is the assessable record) — or carries a `tax_deferred_amount` (an AMIT's cost-base movement is the AMMA `cost_base_adjustment`, CGT event E10, not E4); a parcel-optimiser or pre-sale what-if request with non-positive units, negative proceeds, more units than the listing's open quantity, no obtainable price (the optimiser without an explicit price), allocations that name a non-open parcel, exceed its remaining units, or don't sum to the units sold, or neither/both of allocations and a strategy; a wash-sales report `window_days` under 1; a franking what-if with non-positive units; a row-history request naming a table that is not audited) |
| `500 Internal Server Error` | Unexpected database error, or a job triggered via `POST /jobs/:name` failed |
| `502 Bad Gateway` | Upstream fetch failed (e.g. the RBA FX or ISO MIC import could not reach its source) |

**Error bodies.** A rejected write (`400`, `404`-with-a-cause, `409`, `413`, `422`, `502`) carries a short, plain-text body saying *why* it failed — the failed invariant and, where relevant, the actual values involved (e.g. "account 'Default' is not enrolled in a DRP for VDHG at 2026-03-04 — enrol it on the DRP enrolments screen first", or "the parcel allocations do not sum to the sell quantity"). The web UI surfaces this text in its toast, so a rejection is actionable rather than a bare "HTTP 422". Messages name entities by name/ticker, never by raw foreign-key id. A constraint violation surfaces the database's own message (which names the offending column/constraint, never a client-supplied value). `5xx` responses stay generic — the internal error goes to the server log, not the response body.
