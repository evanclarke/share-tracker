# TODO

Items are only marked done when a passing test exists for them.

## Infrastructure
- [x] Add dependencies: sqlx (SQLite, tokio, chrono), tokio, chrono, chrono-tz, clap, serde, serde_json, axum (web server)
- [x] CLI arg parsing (`--db <path>`, default: `share-tracker.db`)
- [x] Database initialisation and connection pool
- [x] Daily backup on startup (copy DB to `<file>-YYYY-MM-DD.db`)
- [x] GitHub Actions CI: run tests on push
- [x] CI: verify no migration contains DROP TABLE or DROP COLUMN statements
- [x] Logging setup: tracing subscriber with INFO as default level, configurable via RUST_LOG
- [x] Tests: log output at INFO level; RUST_LOG override works
- [x] Database migration system (sqlx migrate): migrations run on startup, applied once
- [x] Tests: migrations apply cleanly on a fresh in-memory DB
- [x] Add rust_decimal dependency (with sqlx feature) for arbitrary-precision decimal arithmetic

## Reference Data — Exchange
- [x] Exchange model (MIC, name, country, currency, timezone, settlement period)
- [x] DB schema: `exchanges` table
- [x] Seed data for known exchanges (XASX, XNYS at minimum)
- [x] CRUD API endpoints for exchanges
- [x] Tests: insert, retrieve, upsert exchange

## Reference Data — Listing
- [x] Listing model (exchange FK, ticker, name, ISIN, security type, currency, AMIT flag)
- [x] DB schema: `listings` table
- [x] CRUD API endpoints for listings
- [x] Tests: insert, retrieve listing; FK constraint to exchange

## Trade Activity
- [x] Trade model (type, date, settlement date, listing FK, average price, quantity, currency, brokerage, GST on brokerage, brokerage currency, FX rate, contract note reference)
- [x] DB schema: `trades` table
- [x] Auto-populate settlement date from trade date + exchange settlement period (overridable)
- [x] CRUD API endpoints for trades
- [x] Tests: buy, sell, DRP trades; settlement date auto-population; override of settlement date
- [x] Refactor financial fields (average_price, quantity, brokerage, gst_on_brokerage, fx_rate) from f64 to Decimal
- [x] Tests: decimal precision preserved in API round-trip

## Income Activity
- [x] Income model (listing FK, date paid, ex date, franked amount, unfranked amount, foreign source income, foreign tax paid, TFN withholding tax, franking credits, LIC capital gain deduction, conduit foreign income, trust income flag, reinvestment trade FK)
- [x] DB schema: `income` table
- [x] CRUD API endpoints for income
- [x] Tests: dividend income, trust distribution, DRP reinvestment linkage
- [x] Refactor financial fields (franked_amount, unfranked_amount, foreign_source_income, foreign_tax_paid, tfn_withholding_tax, franking_credits, lic_capital_gain_deduction, conduit_foreign_income) from f64 to Decimal
- [x] Tests: decimal precision preserved in API round-trip

## AMMA Statements
- [x] AMMA model (listing FK, tax year end date, units held, date received, australian interest, australian dividends unfranked, franked dividends, franking credits, net rent, foreign income, foreign tax credits, other income, CGT discount gains, CGT indexation gains, CGT other gains, capital losses applied, tax deferred amount, tax free amount, cost base adjustment per unit, TFN withholding tax)
- [x] DB schema: `amma_statements` table
- [x] All financial fields use Decimal (not f64): australian_interest, australian_dividends, franked_dividends, franking_credits, net_rent, foreign_income, foreign_tax_credits, other_income, cgt_discount_gains, cgt_indexation_gains, cgt_other_gains, capital_losses_applied, tax_deferred_amount, tax_free_amount, cost_base_adjustment, tfn_withholding_tax
- [x] CRUD API endpoints for AMMA statements
- [x] Tests: insert and retrieve AMMA statement; cost base adjustment calculation
- [x] Tests: decimal precision preserved in API round-trip

## Share Parcel Allocation
- [x] Parcel allocation model (sale trade FK, purchase trade FK, quantity allocated)
- [x] DB schema: `parcel_allocations` table
- [x] quantity_allocated uses Decimal (not f64)
- [x] Validate quantity allocated does not exceed available quantity on purchase trade
- [x] Validate total allocations for a sale trade do not exceed sale quantity
- [x] CRUD API endpoints for parcel allocations
- [x] Tests: allocation creation, over-allocation rejection
- [x] Validate sale_trade_id references a trade of type Sell
- [x] Validate purchase_trade_id references a trade of type Buy or DRP
- [x] Tests: type constraint violations rejected

## Cost Base Adjustments
- [x] AMIT cost base adjustment: apply AMMA `tax deferred` amounts to reduce cost base of affected parcels
- [x] Tests: AMIT adjustment

## Reporting — Portfolio Overview
- [x] Current holdings: aggregate open parcels by listing (quantity, average cost base)
- [x] Accept current market prices as input to materialise portfolio value
- [x] API endpoint for portfolio overview
- [x] Tests: holdings aggregation after buys and sells

## Reporting — Unrealised Gains/Losses
- [ ] Calculate unrealised gain/loss per holding (market value vs cost base)
- [ ] Apply 50% CGT discount indicator for parcels held > 12 months
- [ ] API endpoint for unrealised gains/losses
- [ ] Tests: gain/loss calculation, discount eligibility

## Reporting — Realised Gains/Losses
- [ ] Calculate capital gain/loss per sale using allocated parcels and adjusted cost bases
- [ ] Apply CGT discount (50%) for parcels held > 12 months
- [ ] API endpoint for realised gains/losses
- [ ] Tests: specific parcel sale, CGT discount eligibility

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
