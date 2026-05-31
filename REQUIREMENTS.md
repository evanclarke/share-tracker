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
 - Reporting
   - Current portfolio overview
   - Unrealised gains/losses
   - Realised gains/losses
   - Tax
 - Cost base adjustments
   - AMIT
   - CPI

# Implementation Overview
 - Server process design, written in Rust, using SQLite as the storage
   - Database file can be specified on command line
   - Daily backups are created (named <file>-date.db)
 - Web frontend for all features
 - Features will all have tests
 - Hosted on GitHub, with a hook to run tests when commits are pushed
 - Clear logging with INFO level the default
 - Data model changes implemented in database via data migrations
 - Database seeding with exchange details for ASX and NYSE

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
  - FX Rate
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