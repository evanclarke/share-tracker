# TODO

Items are only marked done when a passing test exists for them.

## Infrastructure
- [x] Add dependencies: sqlx (SQLite, tokio, chrono), tokio, chrono, chrono-tz, clap, serde, serde_json, axum (web server)
- [x] CLI arg parsing (`--db <path>`, default: `share-tracker.db`)
- [x] Database initialisation and connection pool
- [x] Daily backup on startup (copy DB to `<file>-YYYY-MM-DD.db`)
- [ ] GitHub Actions CI: run tests on push
- [x] Logging setup: tracing subscriber with INFO as default level, configurable via RUST_LOG
- [x] Tests: log output at INFO level; RUST_LOG override works
- [x] Database migration system (sqlx migrate): migrations run on startup, applied once
- [x] Tests: migrations apply cleanly on a fresh in-memory DB

## Reference Data — Exchange
- [ ] Exchange model (MIC, name, country, currency, timezone, settlement period)
- [ ] DB schema: `exchanges` table
- [ ] Seed data for known exchanges (XASX, XNYS at minimum)
- [ ] CRUD API endpoints for exchanges
- [ ] Tests: insert, retrieve, upsert exchange

## Reference Data — Listing
- [ ] Listing model (exchange FK, ticker, name, ISIN, security type, currency, AMIT flag)
- [ ] DB schema: `listings` table
- [ ] CRUD API endpoints for listings
- [ ] Tests: insert, retrieve listing; FK constraint to exchange

## Trade Activity
- [ ] Trade model (type, date, settlement date, listing FK, average price, quantity, currency, brokerage, GST on brokerage, brokerage currency, FX rate, contract note reference)
- [ ] DB schema: `trades` table
- [ ] Auto-populate settlement date from trade date + exchange settlement period (overridable)
- [ ] CRUD API endpoints for trades
- [ ] Tests: buy, sell, DRP trades; settlement date auto-population; override of settlement date

## Income Activity
- [ ] Income model (listing FK, date paid, ex date, franked amount, unfranked amount, foreign source income, foreign tax paid, TFN withholding tax, franking credits, LIC capital gain deduction, conduit foreign income, trust income flag, reinvestment trade FK)
- [ ] DB schema: `income` table
- [ ] CRUD API endpoints for income
- [ ] Tests: dividend income, trust distribution, DRP reinvestment linkage

## AMMA Statements
- [ ] AMMA model (listing FK, tax year end date, units held, date received, australian interest, australian dividends unfranked, franked dividends, franking credits, net rent, foreign income, foreign tax credits, other income, CGT discount gains, CGT indexation gains, CGT other gains, capital losses applied, tax deferred amount, tax free amount, cost base adjustment per unit, TFN withholding tax)
- [ ] DB schema: `amma_statements` table
- [ ] CRUD API endpoints for AMMA statements
- [ ] Tests: insert and retrieve AMMA statement; cost base adjustment calculation

## Share Parcel Allocation
- [ ] Parcel allocation model (sale trade FK, purchase trade FK, quantity allocated)
- [ ] DB schema: `parcel_allocations` table
- [ ] Validate quantity allocated does not exceed available quantity on purchase trade
- [ ] Validate total allocations for a sale trade do not exceed sale quantity
- [ ] CRUD API endpoints for parcel allocations
- [ ] Tests: allocation creation, over-allocation rejection

## Cost Base Adjustments
- [ ] AMIT cost base adjustment: apply AMMA `tax deferred` amounts to reduce cost base of affected parcels
- [ ] CPI indexation: apply CPI adjustment to cost base for assets acquired before 21 Sep 1999
- [ ] Tests: AMIT adjustment, CPI adjustment

## Reporting — Portfolio Overview
- [ ] Current holdings: aggregate open parcels by listing (quantity, average cost base)
- [ ] Accept current market prices as input to materialise portfolio value
- [ ] API endpoint for portfolio overview
- [ ] Tests: holdings aggregation after buys and sells

## Reporting — Unrealised Gains/Losses
- [ ] Calculate unrealised gain/loss per holding (market value vs cost base)
- [ ] Apply 50% CGT discount indicator for parcels held > 12 months
- [ ] API endpoint for unrealised gains/losses
- [ ] Tests: gain/loss calculation, discount eligibility

## Reporting — Realised Gains/Losses
- [ ] Calculate capital gain/loss per sale using allocated parcels and adjusted cost bases
- [ ] Apply CGT discount (50%) for parcels held > 12 months
- [ ] Apply indexation for eligible parcels (pre-21 Sep 1999)
- [ ] API endpoint for realised gains/losses
- [ ] Tests: FIFO sale, specific parcel sale, discount vs indexation choice

## Reporting — Tax
- [ ] Aggregate all assessable income components by tax year
- [ ] Aggregate franking credits, foreign tax offsets, TFN withholding tax by tax year
- [ ] Include AMMA attributed income components in tax year totals
- [ ] Include LIC capital gain deductions
- [ ] Exclude conduit foreign income from assessable totals
- [ ] API endpoint for tax summary by year
- [ ] Tests: full-year tax summary with mixed income types

## Web Frontend
- [ ] Serve frontend from the Rust server (axum)
- [ ] Exchange management UI
- [ ] Listing management UI
- [ ] Trade entry and listing UI
- [ ] Income entry and listing UI
- [ ] AMMA statement entry and listing UI
- [ ] Share parcel allocation UI
- [ ] Portfolio overview UI
- [ ] Gains/losses report UI
- [ ] Tax summary UI
