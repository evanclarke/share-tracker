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
   - DRP (Dividend Reinvestment Plan) enrolments
 - DRP (Dividend Reinvestment Plan) handling
   - Record which holdings are enrolled in a DRP
   - For a distribution on an enrolled holding, generate the reinvestment Trade (Type DRP) from the
     distribution's reinvestable cash and the reinvestment price, linked back to the distribution
   - Track the residual cash left over when the reinvestable amount doesn't buy a whole number of
     shares; per enrolment this is either carried forward to the next reinvestment for that holding
     or treated as paid out
 - FX rate reference data
   - Weekly automated import of the ATO's published monthly foreign exchange rates
   - Manual trigger of the same import via an HTTP endpoint (for retries / missed runs)
   - AUD conversions use these rates, falling back to a per-trade FX rate only when no ATO rate
     exists yet for the trade's currency and month
 - Currency reference data (fiat and digital tokens)
   - Monthly automated import of the ISO 4217 fiat currency list and the ISO 24165 digital token
     (DTI) registry into a single currencies reference table
   - Manual trigger of the same import via an HTTP endpoint (for retries / missed runs)
   - Validates that currency codes used on trades, income and AMMA records are recognised codes
 - Document attachments
   - Attach supporting documents (e.g. a trade confirmation / contract note PDF, a dividend
     statement, an AMMA statement scan) to a Trade, Income, or AMMA Statement record
   - File contents are stored inside the database (as a BLOB), not on the filesystem, so the
     existing weekly DB backup captures the documents too - no separate file store to back up
   - Upload, download (original filename + content type preserved), list, and delete attachments;
     an activity may have many attachments, each attachment belongs to exactly one activity
   - Removing an activity removes its attachments in the same transaction (no orphaned blobs)
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
   - Weekly backups are created (named <file>-date-time.db)
   - Weekly scheduled task fetches the ATO's published monthly foreign exchange rates and stores any
     new periods (idempotent - re-fetching an already-stored month must not create duplicates or
     alter existing rows)
   - The same fetch is exposed as an HTTP endpoint so it can be triggered manually (e.g. to retry
     after a failed or missed scheduled run); manual and scheduled runs share the same idempotent
     import logic
   - Monthly scheduled task imports the currencies reference table from the ISO 4217 (SIX Group
     List One) and ISO 24165 (DTIF registry) sources, storing new/changed currencies (idempotent -
     re-importing must not create duplicates or alter unchanged rows); also exposed as an HTTP
     endpoint for manual retries, sharing the same idempotent import logic
 - Web frontend for all features
 - Features will all have tests
 - Hosted on GitHub, with a hook to run tests when commits are pushed
 - Clear logging with INFO level the default
 - Data model changes implemented in database via data migrations
 - Database seeding with exchange details for ASX and NYSE
 - For financial values, always use a BigDecimal type as not to introduce errors due to inexact floats
 - Database migrations must not drop data
 - If a field will only contain a limited set of values, always create an enum and constraints in the database
 - Jobs that are scheduled will log an info when started and finished
 - Tables in the Web UI should be filterable and sortable
 - Document attachment contents are stored as a SQLite BLOB column (binary, not a TEXT/Decimal
   column); only the content payload is binary - all metadata stays in typed columns. A SHA-256
   checksum of the content is computed and stored on upload for integrity verification
 - Accepted attachment content types are restricted to a defined allowlist (proposed: application/pdf,
   image/png, image/jpeg) enforced by a database enum/constraint; an unsupported type is rejected
   with 422
 - A per-file maximum upload size is enforced (proposed: 25 MB); an oversized upload is rejected
   (proposed: 413 Payload Too Large)

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
- Currency
  - A single reference table of recognised currencies covering both fiat currencies and digital
    tokens, used to validate the currency codes recorded on trades, income and AMMA records
  - Kind (enum: Fiat, DigitalToken)
  - Code (primary identifier): for Fiat, the ISO 4217 alphabetic code (e.g. AUD, USD); for
    DigitalToken, the ISO 24165 Digital Token Identifier (DTI - a 9-character identifier)
  - Numeric Code (ISO 4217 numeric code, fiat only - null for digital tokens)
  - Name (currency/entity name for fiat, or the token's long name)
  - Short Name (the token's short name / common ticker; optional for fiat)
  - Minor Units (number of decimal places - ISO 4217 minor unit for fiat, or the token's decimals;
    informational reference only, not used to round stored amounts, which remain arbitrary-precision
    Decimal)
  - Source (enum: Iso4217, Iso24165 - which feed the row came from)
  - Sources, imported by the monthly scheduled task (idempotent - re-importing a period must not
    create duplicates or alter unchanged rows):
    - Fiat (ISO 4217): SIX Group, the official ISO 4217 Maintenance Agency (on behalf of the Swiss
      Association for Standardization, SNV), free machine-readable "List One" XML
      (https://www.six-group.com/dam/download/financial-information/data-center/iso-currrency/lists/list-one.xml)
    - Digital tokens (ISO 24165): the Digital Token Identifier Foundation (DTIF), the ISO 24165
      Registration Authority, free JSON registry snapshot which is itself refreshed monthly
      (download service: https://dtif.org/download-dti-data/; REST API: https://dtif-api-docs.dtif.org/)

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
  - Residual Brought Forward (DRP trades only - leftover cash carried in from the prior reinvestment
    for this holding; 0 when none. See DRP > Reinvestment)
  - Residual Carried Forward (DRP trades only - leftover cash from this reinvestment carried to the
    next one; 0 for non-carry-forward enrolments)
  - Residual Paid Out (DRP trades only - leftover cash paid out rather than carried; 0 for
    carry-forward enrolments)
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
- Attachment (a supporting document for one activity)
  - Owner: exactly one of Trade (FK), Income (FK), or AMMA Statement (FK) - the other two are null.
    Modelled as three nullable FK columns with a database CHECK that exactly one is set (rather than
    a polymorphic type+id pair, so referential integrity to the owning activity is enforced by a
    real foreign key). Deleting the owning activity deletes its attachments (ON DELETE CASCADE)
  - Filename (the original upload filename, preserved for download)
  - Content Type (enum: the accepted MIME allowlist - e.g. application/pdf, image/png, image/jpeg)
  - Byte Size (size of the stored content in bytes; informational, for display)
  - Checksum (SHA-256 of the content, hex; for integrity verification and duplicate detection)
  - Uploaded At (timestamp the attachment was stored)
  - Content (the file bytes, stored as a BLOB)

### Attachment API
- Distinct from the JSON CRUD convention because the payload is binary:
  - Upload via multipart/form-data (the file plus the target activity), returning 201 with the
    created attachment's metadata
  - List/get return metadata only (id, owner, filename, content type, size, checksum, uploaded at) -
    never the blob - so listings stay light and route through the shared filterable table
  - A dedicated content endpoint streams the raw bytes with the stored Content-Type and a
    Content-Disposition filename for download
  - Delete removes the attachment (204, or 404 if unknown)
  - Attachments for a given activity are listable by that activity (e.g. filtered by trade/income/
    AMMA id) so the Web UI can show and manage them from the activity's view

## DRP (Dividend Reinvestment Plan)
- DRP Enrolment
  - Listing (FK, unique - at most one enrolment per holding; its presence means the holding is
    enrolled for full reinvestment)
  - Residual Handling (enum: CarryForward, PayOut - what to do with leftover cash that doesn't buy a
    whole share; default CarryForward)
  - Note: partial participation (only a portion of units reinvested) is out of scope for now -
    enrolment is all-or-nothing

### Reinvestment
- Creating a DRP trade from a distribution is a single operation that, given a distribution (Income
  Activity) on an enrolled holding and the reinvestment price per share:
  - Reinvestable cash = the distribution's cash component (franking credits are not cash and are
    excluded)
  - Residual Brought Forward = the most recent prior DRP trade's Residual Carried Forward for the
    same listing, or 0 if there is none
  - Available = Reinvestable cash + Residual Brought Forward
  - Quantity = floor(Available / reinvestment price) - whole shares only; the registry rounds down
  - Cost = Quantity × reinvestment price
  - Leftover = Available − Cost; if the enrolment's Residual Handling is CarryForward it becomes the
    new trade's Residual Carried Forward (and is picked up by the next reinvestment), otherwise it is
    recorded as Residual Paid Out
- The operation is atomic: it creates the Trade Activity (Type DRP, Listing and Currency from the
  distribution, Date = distribution pay date, Quantity, Average Price = reinvestment price, residual
  fields as above) and sets the distribution's Reinvestment Trade FK to the new trade in one
  transaction - a distribution may have at most one reinvestment trade
- The current carried-forward residual for a holding is therefore derivable as the Residual Carried
  Forward of its latest DRP trade (single source of truth - no separate running balance is stored)