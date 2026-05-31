# share-tracker

A personal Australian share portfolio tracker with a REST JSON API. Records trades, dividends, and trust distributions, then produces portfolio and tax reports aligned with Australian tax rules (CGT discount, franking credits, AMIT/AMMA).

## Features

- **Trade recording** — buys, sells, and dividend reinvestment plan (DRP) acquisitions, with automatic settlement date calculation per exchange
- **Income recording** — dividends and trust distributions with full Australian tax component breakdown (franked/unfranked amounts, foreign source income, franking credits, conduit foreign income, TFN withholding, LIC capital gain deductions)
- **AMIT/AMMA support** — annual tax statements for Attribution Managed Investment Trusts (AMITs), with cost base adjustments applied per purchase parcel
- **Parcel-level CGT** — explicit parcel allocations link sell trades to the parcels they came from; cost bases are pro-rated and AMIT-reduced at the parcel level
- **Portfolio overview** — open holdings per security with total cost base and optional market value (supply current prices in the request body)
- **Unrealised gains report** — per-holding gain/loss and CGT-discount-eligible quantity as at a given date
- **Realised gains report** — per-sale capital gain/loss and CGT-discount-eligible gain (parcels held strictly more than 12 months)
- **Tax summary** — income aggregated by Australian financial year (July–June), combining dividends, trust distributions, and AMMA components
- **FX rate import** — monthly RBA F11 foreign exchange rates (the rates the ATO directs taxpayers to use) fetched and stored as foreign-per-AUD, refreshed weekly and via a manual trigger, ready for AUD tax conversion
- **MIC registry import** — the ISO 10383 Market Identifier Code list imported monthly (and via a manual trigger), used by a non-blocking report to flag curated exchanges whose MIC is unknown or expired

## Building and running

```bash
cargo build --release
./target/release/share-tracker [--db share-tracker.db] [--port 3000] [--schedule schedule.cron]
```

| Flag | Default | Description |
|------|---------|-------------|
| `--db` | `share-tracker.db` | SQLite database file path |
| `--port` | `3000` | HTTP port to listen on |
| `--schedule` | built-in `schedule.cron` | Path to a cron file overriding the built-in maintenance schedule |

The database is created automatically on first run. Migrations are applied in order at startup.

### Scheduled maintenance

Recurring maintenance jobs — the database backup, the RBA FX rate import, and the ISO MIC registry import — are scheduled from a cron file rather than hard-coded intervals. Each line is a 5-field Vixie cron expression (`min hour dom mon dow`) followed by a job name; `#` starts a comment. The built-in default is embedded in the binary (`schedule.cron`); pass `--schedule <path>` to use your own file instead:

```
0 0 * * *   backup          # daily at midnight
0 2 * * 1   rba-fx-import   # weekly, Monday 02:00
0 3 1 * *   mic-import      # monthly, 1st at 03:00 (ISO publishes monthly)
```

A schedule line naming an unknown job is rejected at startup; a registered job with no schedule line is allowed but logged as a `WARN` (it will then only run via its endpoint). Jobs run only at their scheduled times (not at startup); after each run (and at startup) the next scheduled run is logged at INFO. The backup writes `<stem>-YYYY-MM-DD.db` beside the main database file (skipped if one already exists for the day). Any job can be run on demand with `POST /jobs/{name}` (see HTTP API).

Logging is controlled by the `RUST_LOG` environment variable (default: `info`).

## Database schema

```
exchanges
├── mic          TEXT PK          ISO 10383 Market Identifier Code (e.g. XASX)
├── name         TEXT
├── country      TEXT
├── currency     TEXT             Default trading currency
├── timezone     TEXT             IANA timezone string
└── settlement_days INTEGER      T+N settlement (e.g. 2 for ASX)

listings
├── id           INTEGER PK
├── exchange_mic TEXT FK→exchanges.mic
├── ticker       TEXT
├── name         TEXT
├── isin         TEXT (nullable)
├── security_type TEXT           Share | ETF | LIC | Trust
├── currency     TEXT
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

trades
├── id                INTEGER PK
├── trade_type        TEXT         Buy | Sell | DRP
├── date              DATE
├── settlement_date   DATE
├── listing_id        INTEGER FK→listings.id
├── average_price     TEXT (decimal)
├── quantity          TEXT (decimal)
├── currency          TEXT
├── brokerage         TEXT (decimal)
├── gst_on_brokerage  TEXT (decimal)
├── brokerage_currency TEXT
├── fx_rate           TEXT (decimal)  1.0 for AUD trades
└── contract_note_ref TEXT (nullable)

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
└── reinvestment_trade_id     INTEGER FK→trades.id (nullable, for DRP linkage)

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
├── tax_deferred_amount             TEXT (decimal)
├── tax_free_amount                 TEXT (decimal)
├── cost_base_adjustment            TEXT (decimal)  Per-unit cost base reduction
└── tfn_withholding_tax             TEXT (decimal)

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
```

### Relationships

```
exchanges ──< listings ──< trades >──────────────< parcel_allocations
                                \                         /
                                 └──────────────────────-/
                       trades ──< amit_adjustments >──── amma_statements
                       listings ──< amma_statements
                       listings ──< income
                       trades (DRP) ──< income (reinvestment_trade_id)
```

`rba_fx_rates` is standalone reference data (no foreign keys); it is looked up by `(currency, month)`.

`mic_registry` is standalone reference data (no foreign keys), keyed by `mic`. It is populated from the ISO 10383 list and used only to validate curated `exchanges` (see the [exchange MIC validation report](#exchange-mic-validation)); it is *not* the operational exchange table and carries no currency/timezone/settlement data.

Decimal values are stored as TEXT to preserve arbitrary precision.

## HTTP API

All endpoints return JSON. Write endpoints accept `Content-Type: application/json`.

### Exchanges

| Method | Path | Description |
|--------|------|-------------|
| `GET` | `/exchanges` | List all exchanges |
| `GET` | `/exchanges/:mic` | Get one exchange |
| `PUT` | `/exchanges/:mic` | Create or update an exchange |
| `DELETE` | `/exchanges/:mic` | Delete an exchange |

Seed data includes `XASX` (ASX, T+2) and `XNYS` (NYSE, T+2).

### Listings

| Method | Path | Description |
|--------|------|-------------|
| `GET` | `/listings` | List all listings |
| `GET` | `/listings/:id` | Get one listing |
| `PUT` | `/listings/:id` | Create or update a listing |
| `DELETE` | `/listings/:id` | Delete a listing |

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

### Jobs

Recurring maintenance jobs scheduled from the cron file (see [Scheduled maintenance](#scheduled-maintenance)). These endpoints inspect the registered jobs and trigger them on demand.

| Method | Path | Description |
|--------|------|-------------|
| `GET` | `/jobs` | List registered job names (JSON array, sorted) |
| `POST` | `/jobs/:name` | Run the named job now |

`POST /jobs/:name` runs the job synchronously and returns `204 No Content` on success, `404 Not Found` if no job has that name, or `500 Internal Server Error` if the job fails. Registered jobs are `backup`, `rba-fx-import`, and `mic-import`.

### Trades

| Method | Path | Description |
|--------|------|-------------|
| `GET` | `/trades` | List all trades |
| `GET` | `/trades/:id` | Get one trade |
| `PUT` | `/trades/:id` | Create or update a trade |
| `DELETE` | `/trades/:id` | Delete a trade |

If `settlement_date` is omitted from the PUT body, it is auto-calculated by advancing `date` by `exchange.settlement_days` **business days** (weekends are skipped; public holidays are not yet modelled).

`PUT /trades/:id` rejects `trade_type: "Sell"` with `422` — Sells must be created via `PUT /sells/:id` (see below) so they are always persisted with a full set of parcel allocations.

### Income

| Method | Path | Description |
|--------|------|-------------|
| `GET` | `/income` | List all income records |
| `GET` | `/income/:id` | Get one income record |
| `PUT` | `/income/:id` | Create or update an income record |
| `DELETE` | `/income/:id` | Delete an income record |

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

#### Overview

```
POST /portfolio/overview
```

Returns open holdings per listing. Request body (optional):

```json
{ "prices": { "<listing_id>": "<price>" } }
```

Response fields per holding: `listing_id`, `quantity`, `avg_cost_base_per_unit`, `total_cost_base`, `current_price` (nullable), `market_value` (nullable).

Cost base is calculated as `(price × quantity + brokerage + GST) − AMIT reductions`, pro-rated to remaining (unsold) units.

#### Unrealised gains

```
POST /portfolio/unrealised-gains
```

Request body (all optional):

```json
{ "prices": { "<listing_id>": "<price>" }, "as_of_date": "YYYY-MM-DD" }
```

`as_of_date` defaults to today. Response fields per holding: `listing_id`, `quantity`, `total_cost_base`, `current_price`, `market_value`, `unrealised_gain_loss`, `cgt_discount_eligible_quantity` (units from parcels held strictly more than 12 months as at `as_of_date`).

#### Realised gains

```
GET /portfolio/realised-gains
```

Returns one record per sale trade that has at least one parcel allocation. Response fields: `sale_trade_id`, `listing_id`, `sale_date`, `proceeds`, `cost_base`, `capital_gain_loss`, `discount_eligible_gain` (gain attributable to parcels held strictly more than 12 months; losses are excluded).

Sorted by `sale_date` ascending.

#### Tax summary

```
GET /portfolio/tax-summary
```

Returns one record per Australian financial year (identified by the calendar year of 30 June), sorted ascending. Aggregates dividend income by `date_paid` (July = next FY) and AMMA statements by `tax_year_end_date`. Response fields include all income and AMMA components as separate fields for direct transfer to a tax return.

#### Exchange MIC validation

```
GET /reports/exchange_mic_validation
```

Validates each curated exchange's MIC against the `mic_registry` (the imported ISO 10383 list) — **non-blocking**: writes to `exchanges` are never rejected, this only surfaces MICs worth a second look. Returns one record per exchange (sorted by MIC) with fields: `mic`, `exchange_name`, `registry_status` (`ok` = active in the registry, `expired` = present but EXPIRED, `unknown` = no registry entry, i.e. a typo or the registry hasn't been imported yet), `iso_status` (raw ISO `ACTIVE`/`UPDATED`/`EXPIRED`, or null when unknown), and `expiry_date`. With an empty registry every exchange is `unknown`.

## Response codes

| Code | Meaning |
|------|---------|
| `200 OK` | Successful GET |
| `204 No Content` | Successful PUT or DELETE, or a job run via `POST /jobs/:name` |
| `404 Not Found` | Resource does not exist |
| `405 Method Not Allowed` | Write attempted on a read-only path (e.g. `parcel_allocations`) |
| `422 Unprocessable Entity` | Business rule violation (e.g. over-allocation, wrong trade type, under-allocated Sell, unparseable FX or MIC feed) |
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
