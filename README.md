# share-tracker

A personal Australian share portfolio tracker with a REST JSON API. Records trades, dividends, and trust distributions, then produces portfolio and tax reports aligned with Australian tax rules (CGT discount, franking credits, AMIT/AMMA).

## Features

- **Trade recording** — buys, sells, and dividend reinvestment plan (DRP) acquisitions, with automatic settlement date calculation per exchange
- **Income recording** — dividends and trust distributions with full Australian tax component breakdown (franked/unfranked amounts, foreign source income, franking credits, conduit foreign income, TFN withholding, LIC capital gain deductions)
- **DRP reinvestment** — enrol holdings in a Dividend Reinvestment Plan, then turn a distribution into a linked DRP trade; leftover cash that can't buy a whole share is carried forward to the next reinvestment or paid out, per the enrolment
- **AMIT/AMMA support** — annual tax statements for Attribution Managed Investment Trusts (AMITs), with cost base adjustments applied per purchase parcel
- **Parcel-level CGT** — explicit parcel allocations link sell trades to the parcels they came from; cost bases are pro-rated and AMIT-reduced at the parcel level
- **Portfolio overview** — open holdings per security with total cost base and optional market value (supply current prices in the request body)
- **Unrealised gains report** — per-holding gain/loss and CGT-discount-eligible quantity as at a given date
- **Realised gains report** — per-sale capital gain/loss split into discount-eligible (parcels held strictly more than 12 months), non-discountable, and loss buckets
- **Net capital gain report** — the overall CGT position per financial year: combines realised parcel gains with AMMA-attributed CGT gains and capital losses, applies losses ATO-optimally (non-discountable gains first), carries unused net capital losses forward across years (seeded by an enterable opening carried-forward loss), and applies the 50% discount to produce the assessable net capital gain
- **Tax summary** — income aggregated by Australian financial year (July–June), combining dividends, trust distributions, and AMMA components
- **FX rate import** — monthly RBA F11 foreign exchange rates (the rates the ATO directs taxpayers to use) fetched and stored as foreign-per-AUD, refreshed weekly and via a manual trigger
- **AUD conversion** — cost base and proceeds in the portfolio, unrealised, and realised reports are converted to AUD at the ATO reference rate (with a per-trade manual `fx_rate` fallback); see [FX conversion](#fx-conversion)
- **MIC registry import** — the ISO 10383 Market Identifier Code list imported monthly (and via a manual trigger), used by a non-blocking report to flag curated exchanges whose MIC is unknown or expired
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
└── amit         BOOLEAN          True if the security is an AMIT

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
└── residual_paid_out        TEXT (decimal)  DRP trades only: leftover paid out instead of carried (else 0)

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
└── currency                  TEXT FK→currencies.code   ISO 4217; tax summary converts to AUD by date_paid month (default AUD)

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

drp_enrolments               DRP enrolment per holding (presence = reinvest in full)
├── listing_id           INTEGER PK, FK→listings.id  One enrolment per holding
└── residual_handling    TEXT   CarryForward | PayOut  Leftover-cash policy (default CarryForward)

cgt_settings                 Singleton CGT settings row (CHECK id = 1)
├── id                   INTEGER PK  Always 1 (CHECK-enforced singleton)
└── opening_capital_loss TEXT (decimal)  Net capital loss carried forward from before the first recorded year (AUD, non-negative); starting balance for the net-capital-gain loss chain

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
                       trades (DRP) ──< income (reinvestment_trade_id)
                       trades, income, amma_statements ──< attachments (exactly one owner; ON DELETE CASCADE)

currencies ──< exchanges, listings, trades (currency + brokerage_currency), income, amma_statements
```

Each `attachments` row belongs to exactly one activity via one of three nullable foreign keys (`trade_id` / `income_id` / `amma_statement_id`), with a `CHECK` enforcing that exactly one is set — a real foreign key keeps referential integrity to the owning row, and `ON DELETE CASCADE` removes an activity's attachments when it is deleted. File contents live in the `content` BLOB so the weekly DB backup captures the documents with no separate file store.

`rba_fx_rates` is standalone reference data (no foreign keys); it is looked up by `(currency, month)`. `job_runs` is likewise standalone: it is keyed by the in-code job name (not a foreign key), and each scheduled or manual run upserts the job's row so only its last run is kept. `cgt_settings` is also standalone: a singleton row (`CHECK (id = 1)`) holding the entered opening carried-forward capital loss consumed by the [net capital gain report](#net-capital-gain).

`mic_registry` is standalone reference data (no foreign keys), keyed by `mic`. It is populated from the ISO 10383 list and used only to validate curated `exchanges` (see the [exchange MIC validation report](#exchange-mic-validation)); it is *not* the operational exchange table and carries no currency/timezone/settlement data.

`currencies` is reference data keyed by `code` (it has no outgoing foreign keys). It is populated from the ISO 4217 (SIX Group) and ISO 24165 (DTIF) feeds and seeded with a baseline of common currencies (the seed migration), and is the recognised list that **every** currency code in the model is foreign-keyed to: `exchanges.currency`, `listings.currency`, `trades.currency`, `trades.brokerage_currency`, `income.currency`, and `amma_statements.currency` all reference `currencies.code`, so an unrecognised currency is rejected at write time. `minor_units` is informational only — stored amounts remain arbitrary-precision Decimal and are never rounded to it.

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

Open `http://localhost:<port>/` in a browser. The app is hash-routed (`#/e/<entity>`, `#/sells`, `#/jobs`, `#/attachments/<owner>/<id>`, `#/r/<report>`) and drives the JSON API below — it provides CRUD screens for every entity (exchanges, listings, trades, income, AMMA statements, AMIT adjustments, DRP enrolments, exchange holidays, CGT settings), a dedicated Sell screen that captures parcel allocations atomically, a DRP reinvest action on income rows, an Attachments action on each trade/income/AMMA row that uploads, lists, downloads, and deletes its documents, read-only views of the import-managed reference tables (currencies, MIC registry, RBA FX rates, parcel allocations), a Maintenance → Jobs screen that lists the scheduled jobs with each one's last run (when it finished, whether it succeeded, and any error) and runs any of them on demand (`POST /jobs/:name`), and a view for each report (portfolio overview, unrealised/realised gains, net capital gain, tax summary, exchange MIC validation).

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

If `settlement_date` is omitted from the PUT body, it is auto-calculated by advancing `date` by `exchange.settlement_days` **business days** — both weekends and the exchange's seeded public holidays (see [Exchange holidays](#exchange-holidays)) are skipped.

`PUT /trades/:id` rejects `trade_type: "Sell"` with `422` — Sells must be created via `PUT /sells/:id` (see below) so they are always persisted with a full set of parcel allocations.

### Income

| Method | Path | Description |
|--------|------|-------------|
| `GET` | `/income` | List all income records |
| `GET` | `/income/:id` | Get one income record |
| `PUT` | `/income/:id` | Create or update an income record |
| `DELETE` | `/income/:id` | Delete an income record |
| `POST` | `/income/:id/reinvest` | Create the DRP reinvestment trade for this distribution (see [DRP reinvestment](#drp-reinvestment)) |

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

Records which holdings reinvest their distributions. Keyed by `listing_id` (one enrolment per holding); the path id is the listing id.

| Method | Path | Description |
|--------|------|-------------|
| `GET` | `/drp_enrolments` | List all DRP enrolments |
| `GET` | `/drp_enrolments/:listing_id` | Get one holding's enrolment |
| `PUT` | `/drp_enrolments/:listing_id` | Enrol a holding (or update its residual handling) |
| `DELETE` | `/drp_enrolments/:listing_id` | Remove an enrolment |

```
PUT /drp_enrolments/1
{ "residual_handling": "CarryForward" }   // or "PayOut"; defaults to CarryForward if omitted
```

`residual_handling` decides what happens to leftover cash a reinvestment can't spend on whole shares: `CarryForward` adds it to the next reinvestment for the holding, `PayOut` records it as paid out. Returns `204 No Content`, or `422 Unprocessable Entity` if `listing_id` doesn't reference a listing.

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

### DRP reinvestment

```
POST /income/:id/reinvest
{ "reinvestment_price": "1.50", "fx_rate": "0.65", "date": "2024-03-31" }
```

Creates the DRP reinvestment trade for a distribution and links it back (`income.reinvestment_trade_id`) in one transaction. `fx_rate` (default 1) and `date` (default the distribution's `date_paid`) are optional.

The reinvestable cash — `franked_amount + unfranked_amount + foreign_source_income − foreign_tax_paid − tfn_withholding_tax` (franking credits are notional and excluded) — plus the residual brought forward from the holding's most recent prior DRP trade is spent on whole shares at `reinvestment_price`. The leftover is carried forward or paid out per the enrolment's `residual_handling` and recorded on the new trade's residual columns.

Returns `201 Created` with the created trade as JSON, `404 Not Found` if no income record has that id, or `422 Unprocessable Entity` if the holding isn't enrolled, the distribution was already reinvested, or `reinvestment_price` is not positive.

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

Returns `204 No Content` on success, or `422 Unprocessable Entity` if the allocations do not sum exactly to `quantity`, a referenced purchase trade is missing or is not a Buy/DRP, or an allocation would over-allocate a purchase parcel. On any failure the whole transaction is rolled back — nothing is persisted.

```
DELETE /sells/:id
```

Deletes a Sell trade and all of its parcel allocations in one transaction, freeing the purchase parcels those allocations had consumed. Returns `204 No Content` on success, `404 Not Found` if no trade has that id, or `422 Unprocessable Entity` if the id refers to a trade that is not a Sell (use `DELETE /trades/:id` for Buy/DRP trades).

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

Cost base is calculated as `(price × quantity + brokerage + GST) − AMIT reductions`, pro-rated to remaining (unsold) units, then converted to AUD (see [FX conversion](#fx-conversion)). Supplied prices are expected in AUD, so `market_value` is AUD.

#### Unrealised gains

```
POST /portfolio/unrealised-gains
```

Request body (all optional):

```json
{ "prices": { "<listing_id>": "<price>" }, "as_of_date": "YYYY-MM-DD" }
```

`as_of_date` defaults to today. Response fields per holding: `listing_id`, `quantity`, `total_cost_base`, `current_price`, `market_value`, `unrealised_gain_loss`, `cgt_discount_eligible_quantity` (units from parcels held strictly more than 12 months as at `as_of_date`). `total_cost_base` is in AUD (see [FX conversion](#fx-conversion)); supplied prices are expected in AUD, so `market_value` and `unrealised_gain_loss` are AUD.

#### Realised gains

```
GET /portfolio/realised-gains
```

Returns one record per sale trade that has at least one parcel allocation. Response fields: `sale_trade_id`, `listing_id`, `sale_date`, `proceeds`, `cost_base`, `capital_gain_loss`, `discount_eligible_gain` (gross gain from parcels held strictly more than 12 months), `non_discountable_gain` (gross gain from parcels held 12 months or less — the "other" method), and `capital_loss` (total losses from allocations sold below cost, as a positive amount). The three buckets satisfy `capital_gain_loss == discount_eligible_gain + non_discountable_gain − capital_loss`. `proceeds`, `cost_base`, and `capital_gain_loss` are in AUD: proceeds are converted at the sale's FX rate and cost base at the purchase's FX rate (see [FX conversion](#fx-conversion)).

Sorted by `sale_date` ascending.

#### Net capital gain

```
GET /portfolio/net-capital-gain
```

Returns one record per Australian financial year (identified by the calendar year of 30 June), sorted ascending — the overall CGT position combining realised parcel gains with the CGT components attributed on AMMA statements. Realised gains are attributed by the sale's tax year (July = next FY); AMMA components by `tax_year_end_date`.

The assessable net capital gain is computed the ATO way:

1. Total the year's gross capital gains, split into **discount-eligible** (realised parcels held > 12 months + AMMA discount-method gains grossed up ×2 — the AMMA `cgt_discount_gains` value is the already-halved "discounted capital gain", so doubling it restores the gross gain + any **CGT event E10** gain held > 12 months at year end) and **non-discountable** (realised parcels held ≤ 12 months + AMMA indexation-method and other-method gains, neither of which gets the discount + any CGT event E10 gain held ≤ 12 months).
2. Total the year's capital losses: realised losses + AMMA `capital_losses_applied`, **plus the net capital loss brought forward from earlier years** — unused losses chain across the year series indefinitely (per the ATO), starting from the entered [opening carried-forward loss](#cgt-settings) (losses from before the first recorded year).
3. Apply losses against non-discountable gains first, then discount-eligible gains (taxpayer-favourable: the 50% discount falls on the largest possible remaining gain). Losses always apply before the discount.
4. **Net capital gain** = remaining non-discountable gain + 50% of the remaining discount-eligible gain. Unused losses are carried forward into the next year in the series.

**CGT event E10**: when the cumulative AMIT cost base reductions (`amit_adjustments` × the AMMA per-unit `cost_base_adjustment`) on a parcel exceed its cost base, the cost base is floored at nil (in the portfolio, unrealised, and realised reports) and the excess is a capital gain in the income year the reducing AMMA statement applies to — added to the gain buckets above (discount-eligible vs not, per the holding period as at the statement's `tax_year_end_date`). The excess is converted to AUD at the parcel's buy-month rate. See `docs/amit-cost-base-adjustments.md`.

Response fields: `tax_year`, `discount_eligible_gains`, `other_gains`, `capital_losses` (all gross; `capital_losses` is only the losses arising that year), `capital_loss_brought_forward` (unused losses chained from earlier years, seeded by the `cgt_settings` opening balance), `net_discount_eligible_gain` and `net_other_gain` (after losses), `cgt_discount` (the 50% reduction applied = `net_discount_eligible_gain / 2`), `net_capital_gain`, `capital_loss_carried_forward` (losses left unused after offsetting all gains — the next year's brought-forward balance), and `cgt_event_e10_gain` (informational: gross E10 gains already included in the gain buckets). All amounts are AUD (AMMA amounts converted via the ATO rate for the month of `tax_year_end_date`, so a non-AUD amount with no rate fails loudly with `500`; see [FX conversion](#fx-conversion)).

#### Tax summary

```
GET /portfolio/tax-summary
```

Returns one record per Australian financial year (identified by the calendar year of 30 June), sorted ascending. Aggregates dividend income by `date_paid` (July = next FY) and AMMA statements by `tax_year_end_date`. All amounts are converted to AUD via the ATO rate (see [FX conversion](#fx-conversion)) before aggregating, using each record's `currency` and the month of `date_paid` (income) or `tax_year_end_date` (AMMA). Response fields include all income and AMMA components as separate fields for direct transfer to a tax return.

#### Exchange MIC validation

```
GET /reports/exchange_mic_validation
```

Validates each curated exchange's MIC against the `mic_registry` (the imported ISO 10383 list) — **non-blocking**: writes to `exchanges` are never rejected, this only surfaces MICs worth a second look. Returns one record per exchange (sorted by MIC) with fields: `mic`, `exchange_name`, `registry_status` (`ok` = active in the registry, `expired` = present but EXPIRED, `unknown` = no registry entry, i.e. a typo or the registry hasn't been imported yet), `iso_status` (raw ISO `ACTIVE`/`UPDATED`/`EXPIRED`, or null when unknown), and `expiry_date`. With an empty registry every exchange is `unknown`.

## Response codes

| Code | Meaning |
|------|---------|
| `200 OK` | Successful GET |
| `201 Created` | DRP reinvestment trade created via `POST /income/:id/reinvest`, or an attachment uploaded via `POST /attachments` |
| `204 No Content` | Successful PUT or DELETE, or a job run via `POST /jobs/:name` |
| `400 Bad Request` | Malformed path parameter (e.g. an `exchange_holidays` `:date` that is not `YYYY-MM-DD`) |
| `404 Not Found` | Resource does not exist |
| `405 Method Not Allowed` | Write attempted on a read-only path (e.g. `parcel_allocations`) |
| `413 Payload Too Large` | Uploaded attachment exceeds the 25 MB per-file limit |
| `422 Unprocessable Entity` | Business rule or constraint violation (e.g. over-allocation, wrong trade type, under-allocated Sell, unparseable FX or MIC feed, a write referencing an unrecognised currency / unknown exchange / listing, an attachment upload with no/multiple owners or an unsupported content type, or a negative / non-singleton `cgt_settings` opening capital loss) |
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
