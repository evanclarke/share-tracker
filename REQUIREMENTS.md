# Project Overview

This project is a comprehensive share tracker, where facts about the investing activity are recorded 
and an overview of the portfolio can be materialised for given market prices from these facts.  Reporting
and cost basis calculations are done with the Australian tax view in mind.

# Features Overview
 - Recording and maintenance for:
   - Trade activity
   - Income activity
   - AMMA statements
   - Share parcel allocation for sales
 - FX rate reference data
   - Weekly automated import of the ATO's published monthly foreign exchange rates
   - Manual trigger of the same import via an HTTP endpoint (for retries / missed runs)
   - AUD conversions use these rates, falling back to a per-trade FX rate only when no ATO rate
     exists yet for the trade's currency and month
 - Reporting
   - Current portfolio overview
   - Unrealised gains/losses
   - Realised gains/losses
   - Tax
 - Cost base adjustments
   - AMIT

# Implementation Overview and Rules
 - Server process design, written in Rust, using SQLite as the storage
   - Database file can be specified on command line
   - Daily backups are created (named <file>-date.db)
   - Weekly scheduled task fetches the ATO's published monthly foreign exchange rates and stores any
     new periods (idempotent - re-fetching an already-stored month must not create duplicates or
     alter existing rows)
   - The same fetch is exposed as an HTTP endpoint so it can be triggered manually (e.g. to retry
     after a failed or missed scheduled run); manual and scheduled runs share the same idempotent
     import logic
 - Web frontend for all features
 - Features will all have tests
 - Hosted on GitHub, with a hook to run tests when commits are pushed
 - Clear logging with INFO level the default
 - Data model changes implemented in database via data migrations
 - Database seeding with exchange details for ASX and NYSE
 - For financial values, always use a BigDecimal type as not to introduce errors due to inexact floats
 - Database migrations must not drop data
 - If a field will only contain a limited set of values, always create an enum and constraints in the database

# Data Model
## Reference Data
- Exchange
  - MIC (Market Identifier Code, ISO 10383 - primary identifier)
  - Name
  - Country
  - Currency (default trading currency for the exchange)
  - Timezone
  - Settlement Period (T+n days, e.g. T+2 - used to auto-populate Settlement Date on trades)
- Listing
  - Exchange (FK)
  - Ticker / Code
  - Name
  - ISIN (optional, International Securities Identification Number)
  - Security Type (Share, ETF, LIC, Trust)
  - Currency (may differ from exchange default for cross-listed securities)
  - AMIT Flag (whether the security is subject to the AMIT regime and will have AMMA statements)
- ATO FX Rate
  - Source: the ATO's published monthly foreign exchange rates, imported by the weekly scheduled task
  - Currency (ISO 4217 code of the foreign currency, e.g. USD)
  - Month (the rate period - year and month, matching the ATO monthly rates table)
  - Rate (units of the foreign currency per 1 AUD, exactly as published by the ATO)
  - Unique per (Currency, Month)

### FX Conversion
- All reports take the Australian-tax view: every non-AUD amount is converted to AUD before it is
  aggregated or compared
- The rate used is the ATO FX Rate for the amount's currency and the month of the relevant date
  (e.g. trade date for trades). AUD amounts need no conversion (rate = 1)
- If no ATO FX Rate exists yet for that (currency, month), fall back to the trade's manual FX Rate
  override (same foreign-per-AUD convention). The ATO rate always takes precedence once available -
  the override is only consulted in its absence
- The ATO publishes rates as foreign-per-AUD, so: AUD amount = foreign amount / Rate
- If neither an ATO FX Rate nor a manual override is available for a required conversion, it must
  fail loudly rather than silently substitute a default - it must not produce a zero or unconverted
  figure

## Facts
The facts that are recorded by the user
- Trade Activity
  - Type (Buy, Sell, DRP)
  - Date
  - Settlement Date (automatically populated from Date and exchange settlement rules - can be overridden)
  - Listing
  - Average Price
  - Quantity
  - Currency
  - Brokerage
  - GST on Brokerage
  - Brokerage Currency
  - FX Rate (optional manual override, foreign-per-AUD - used as a fallback only when no ATO FX Rate
    exists for the trade's currency and month; the ATO rate takes precedence once available. See
    Reference Data > FX Conversion)
  - Contract Note Reference
- Income Activity
  - Listing
  - Date Paid
  - Ex Date
  - Franked Amount
  - Unfranked Amount
  - Foreign Source Income
  - Foreign Tax Paid
  - TFN Withholding Tax
  - Franking Credits
  - LIC Capital Gain Deduction
  - Conduit Foreign Income
  - Trust Income Flag
  - Reinvestment Trade (FK to Trade Activity, optional - populated when income was reinvested via DRP)
- AMMA Statements
  - Listing
  - Tax Year End Date
  - Units Held (at year end)
  - Date Received
  - Australian Interest
  - Australian Dividends (unfranked)
  - Franked Dividends
  - Franking Credits
  - Net Rent
  - Foreign Income
  - Foreign Tax Credits
  - Other Income
  - CGT Discount Gains (gross, pre-discount)
  - CGT Indexation Gains
  - CGT Other Gains (no concession)
  - Capital Losses Applied
  - Tax Deferred Amount (reduces cost base)
  - Tax Free Amount (NANE, no cost base impact)
  - Cost Base Adjustment (net per-unit)
  - TFN Withholding Tax
- Share Parcel Allocation
  - Sale Trade (FK to Trade Activity, Type: Sell)
  - Purchase Trade (FK to Trade Activity, Type: Buy or DRP)
  - Quantity Allocated