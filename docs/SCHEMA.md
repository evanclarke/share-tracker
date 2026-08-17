# Database schema

The SQLite schema behind [share-tracker](../README.md). The HTTP endpoints over these tables are documented in [API.md](API.md).

```
exchanges
├── mic          TEXT PK          ISO 10383 Market Identifier Code (e.g. XASX)
├── name         TEXT
├── country      TEXT
├── currency     TEXT FK→currencies.code   Default trading currency
├── timezone     TEXT             IANA timezone string
├── settlement_days INTEGER      T+N settlement (e.g. 2 for ASX)
└── close_time   TEXT             'HH:MM' local end of the regular session; a day's closing price is only collected after it (default 16:00)

exchange_holidays             Full-closure non-trading days per exchange (settlement skips them)
├── mic          TEXT FK→exchanges.mic   Part of PK
├── holiday_date TEXT             'YYYY-MM-DD'; part of PK
└── name         TEXT             Holiday name (informational)

listings
├── id           INTEGER PK
├── exchange_mic TEXT FK→exchanges.mic (nullable)  NULL exactly when security_type = Crypto (CHECK); exchange-less listings are unique by ticker (partial unique index), the rest by UNIQUE(exchange_mic, ticker)
├── ticker       TEXT             For Crypto: must be a recognised digital-token code in currencies (kind DigitalToken), validated at write time
├── name         TEXT
├── isin         TEXT (nullable)
├── security_type TEXT           Share | ETF | LIC | Trust | Crypto
├── currency     TEXT FK→currencies.code
├── amit         BOOLEAN          True if the security is an AMIT
├── amit_from    TEXT (nullable)  'YYYY-MM-DD' — the 1 July the fund's first AMIT income year began (0024), for a MIT that *converted*: every reader of `amit` compares its record's own financial year against this, so the earlier years stay ordinary trust income (SCENARIOS F-23). NULL = the flag applies to the whole history. Only set on an `amit` listing, and only a 1 July date — both enforced in entities::listing::db_upsert (SQLite can neither ALTER in a table-level CHECK nor reference another column from a column CHECK)
├── preference   BOOLEAN          Preference share: franking credits need 90 (not 45) at-risk days
└── price_symbol TEXT (nullable)  Provider-symbol override (0018): used verbatim by closing_price::yahoo_symbol ahead of its derived ticker/exchange mapping, for a symbol the provider spells differently or an exchange with no mapping

listing_renames               A ticker or exchange-code rename, as a dated event (0018; see API.md Ticker or name changes)
├── id               INTEGER PK
├── listing_id       INTEGER FK→listings.id
├── effective_date   TEXT             'YYYY-MM-DD' — first trading day under the new identity; UNIQUE with listing_id
├── old_ticker       TEXT             Written from the listing's row at the moment of the rename, never trusted from the request
├── new_ticker       TEXT
├── old_exchange_mic TEXT FK→exchanges.mic (nullable)
├── new_exchange_mic TEXT FK→exchanges.mic (nullable)
└── note             TEXT (nullable)
                       CHECK: old_ticker <> new_ticker OR old_exchange_mic IS NOT new_exchange_mic (no no-op rename)

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

holding_accounts             Custody/location accounts within the one taxpayer (e.g. employer share plan vs personal broker)
├── id           INTEGER PK       Account 1 ('Default') is seeded; writes that omit an account fall back to it
└── name         TEXT UNIQUE

transfers                    Moves of a listing between two holding accounts of the same owner (not a CGT event)
├── id              INTEGER PK
├── listing_id      INTEGER FK→listings.id
├── date            TEXT          The transfer-out Sell and transfer-in Buys are dated on it
├── from_account_id INTEGER FK→holding_accounts.id
├── to_account_id   INTEGER FK→holding_accounts.id   CHECK: differs from from_account_id
└── fee_sale_trade_id INTEGER FK→trades.id (nullable)  The network-fee disposal Sell, when a crypto wallet transfer burned an on-chain fee paid in the crypto (NULL otherwise). Unlike the transfer-out Sell (trades.transfer_id), this Sell is a real disposal and IS counted by the gains reports — linked here, not via transfer_id, so it stays visible to them. Set by PUT /transfers/:id, removed with the transfer by DELETE /transfers/:id; immutable via PUT /sells, PUT /trades, DELETE /sells
                    The per-parcel quantities live on the transfer-out Sell's parcel_allocations rows

trades
├── id                INTEGER PK
├── trade_type        TEXT         Buy | Sell | DRP
├── date              DATE   Indexed — every as-of report (open holdings, unrealised gains, performance) filters trades by date <= as_of
├── settlement_date   DATE
├── listing_id        INTEGER FK→listings.id
├── average_price     TEXT (decimal)
├── quantity          TEXT (decimal)
├── currency          TEXT FK→currencies.code
├── brokerage         TEXT (decimal)  Always stored ex-GST: a GST-inclusive entry is split at write time (gst = amount/11 rounded to the cent, brokerage = remainder), so cost base = brokerage + gst_on_brokerage unconditionally
├── gst_on_brokerage  TEXT (decimal)
├── brokerage_includes_gst INTEGER CHECK (0,1)  Records that the brokerage amount was *entered* GST-inclusive and server-split; persisted only so the entry form round-trips (the money columns are already split — nothing else reads it). 0 on operation-created trades
├── brokerage_currency TEXT FK→currencies.code  The currency the fee was billed in; write-time validated to equal `currency` (422 otherwise) — the cost base, a Sell's net proceeds and the activity ledger's transaction total are all single-currency sums, so a foreign fee is entered converted into the trade's currency (SCENARIOS B-02)
├── fx_rate           TEXT (decimal)  Manual foreign-per-AUD override; fallback when no ATO rate exists (1.0 for AUD trades)
├── spot_fx_rate      TEXT (decimal, nullable)  Deliberate transaction-date spot rate (same foreign-per-AUD convention): when set it wins over the monthly RBA rate everywhere this trade converts to AUD (QC 18020 — an average rate is not appropriate for a one-off purchase/sale of a large capital asset). NULL = unchanged default (monthly rate first, fx_rate fallback). Write-time validated: positive, non-AUD trades only (422 otherwise); carried onto scrip/demerger/transfer replacement Buys with fx_rate
├── contract_note_ref TEXT (nullable)
├── statement_total   TEXT (decimal, nullable)  The broker statement's net transaction total in the brokerage currency, for cross-referencing against the contract note. Validated at write time (quantity × price + brokerage + GST for a Buy/DRP, − for a Sell); informational-only after that — no report or calculation uses it. NULL on operation-created trades
├── residual_brought_forward TEXT (decimal)  DRP trades only: leftover cash carried in from the prior reinvestment (else 0)
├── residual_carried_forward TEXT (decimal)  DRP trades only: leftover carried to the next reinvestment (else 0)
├── residual_paid_out        TEXT (decimal)  DRP trades only: leftover paid out instead of carried, incl. the trailing residual refunded at DRP unenrolment (else 0)
├── rights_action_id  INTEGER FK→corporate_actions.id (nullable)  Rights-exercise Buys only: the RightsIssue action exercised, set by POST /corporate_actions/:id/exercise (caps cumulative exercised units at the entitlement; the trade is immutable via PUT /trades and blocks editing/deleting the action)
├── buyback_action_id INTEGER FK→corporate_actions.id (nullable)  Buy-back participation Sells only: the BuyBack action sold into, set by POST /corporate_actions/:id/participate (the trade is immutable via PUT /sells, carries a linked dividend income row removed with it by DELETE /sells, and blocks editing/deleting the action)
├── scrip_action_id   INTEGER FK→corporate_actions.id (nullable)  Scrip-for-scrip exchange trades only (the closing Sell + every replacement Buy): the ScripForScrip action exchanged, set by POST /corporate_actions/:id/exchange. The trades carrying one action id form the exchange group: each is immutable via PUT /sells and PUT/DELETE /trades, DELETE /sells on the closing Sell removes the whole group, and the action is frozen while any exists
├── demerger_action_id INTEGER FK→corporate_actions.id (nullable)  Demerger trades only (the closing Sell + every head replacement and demerged-entity Buy): the Demerger action demerged, set by POST /corporate_actions/:id/demerge. Same group rules as scrip_action_id: each trade is immutable via PUT /sells and PUT/DELETE /trades, DELETE /sells on the closing Sell removes the whole group, and the action is frozen while any exists
├── deemed_acquisition_date TEXT (nullable)  Scrip-for-scrip replacement, demerger head/demerged, and transfer-in Buys only: the consumed parcel's acquisition date, carried by the rollover/transfer (the combined holding period; an own-account transfer is not a disposal at all). Drives the 12-month CGT discount clock and the AUD translation month of the cost base in the reports; split/return-of-capital applicability stays on the actual trade date. NULL = the trade's own date
├── holding_account_id INTEGER FK→holding_accounts.id  The account the trade sits in (defaults to the seeded default account when omitted from a write)
├── transfer_id       INTEGER FK→transfers.id (nullable)  Transfer trades only (the transfer-out Sell + every transfer-in Buy): the transfer that created them, set by PUT /transfers/:id. The trades carrying one transfer id form the transfer group: each is immutable via PUT /sells, PUT/DELETE /trades, and DELETE /sells; DELETE /transfers/:id removes the whole group (with the transfer record), restoring the pre-transfer holding
├── ess_statement_id  INTEGER FK→ess_statements.id (nullable)  ESS vest Buys only: the ESS statement whose discount this parcel resets the cost base for, set by POST /ess_statements/:id/vest. The Buy is immutable via PUT /trades and never deleted individually — DELETE /ess_statements/:id removes it (refused while it is drawn on by a Sell allocation or AMIT adjustment), and the statement is frozen against edits while it exists
├── worthless_action_id INTEGER FK→corporate_actions.id (nullable)  Worthless-shares recognise closing Sells only: the WorthlessShares action recognised, set by POST /corporate_actions/:id/recognise. The Sell is immutable via PUT /sells and PUT/DELETE /trades; DELETE /sells removes it and restores the holding; the action is frozen while it exists. Unlike scrip_action_id/demerger_action_id it does NOT exclude the Sell from the realised-gains report — its nil proceeds recognise the capital loss
└── inheritance_id    INTEGER FK→inheritances.id (nullable)  Inherited-parcel Buys only: the inheritance that created them, set by PUT /inheritances/:id. The Buy carries the inheritance's cost base (on the brokerage column, price 0) and s 115-30 discount clock (deemed_acquisition_date), so it is immutable via PUT/DELETE /trades — it is edited and deleted through its inheritance, both refused while a Sell allocation or AMIT adjustment draws on it

inheritances                 Inherited share parcels from a deceased estate — not a CGT event on transfer (docs/ato/inherited-assets-cost-base.md QC 66053, docs/ato/inherited-assets-cgt-discount.md QC 69713 / s 115-30)
├── id                        INTEGER PK
├── listing_id                INTEGER FK→listings.id
├── holding_account_id        INTEGER FK→holding_accounts.id  Defaults to the seeded default account
├── quantity                  TEXT (decimal)  Units inherited, in date-of-death terms (> 0, validated at write time)
├── date_of_death             TEXT          The linked Buy is dated (and settled) on it — an estate transmission is not market-settled
├── cost_base_rule            TEXT          DeceasedCostBase | MarketValueAtDeath (CHECK) — which QC 66053 rule produced the first-element figure: the deceased's cost base at death (asset acquired by the deceased on/after 20 Sep 1985) or the market value at death (pre-CGT asset; the user supplies the valuation)
├── cost_base                 TEXT (decimal)  The whole-parcel first-element cost base per that rule, in `currency`
├── lpr_expenditure           TEXT (decimal)  LPR expenditure the beneficiary may include (conveyancing on transfer, legal costs of proving the will) — added to the linked Buy's cost base
├── lpr_expenditure_date      TEXT (nullable)  When the LPR incurred it (on or after the death; present exactly when lpr_expenditure is non-zero — validated at write time). Provenance only: indexation, where the date would matter, is not modelled
├── deceased_acquisition_date TEXT (nullable)  CHECK: present exactly when cost_base_rule = DeceasedCostBase, and not after date_of_death; ≥ 20 Sep 1985 validated at write time. Starts the s 115-30 discount clock, carried as the Buy's deemed_acquisition_date; a pre-CGT asset's clock runs from the date of death instead
├── currency                  TEXT FK→currencies.code  Default AUD
└── fx_rate                   TEXT (decimal)  Manual foreign-per-AUD fallback (same convention as trades.fx_rate)
                    The entry path (PUT /inheritances/:id) upserts this row and its linked Buy atomically; like transfers, no report reads this table directly — the parcel side lives on the Buy

income
├── id                        INTEGER PK
├── listing_id                INTEGER FK→listings.id
├── date_paid                 DATE   Indexed — every as-of report filters income by date_paid <= as_of
├── ex_date                   DATE (nullable)
├── franked_amount            TEXT (decimal)
├── unfranked_amount          TEXT (decimal)
├── foreign_source_income     TEXT (decimal)
├── foreign_tax_paid          TEXT (decimal)
├── tfn_withholding_tax       TEXT (decimal)
├── franking_credits          TEXT (decimal)
├── lic_capital_gain_amount   TEXT (decimal)  The LIC capital gain amount (the attributable part) a listed investment company advises on its dividend statement, entered as printed — not the deduction claimed from it. An individual deducts 50% of it at question D8, computed by the tax summary and the annual tax report (entities::income::Income::lic_capital_gain_deduction, docs/ato/lic-capital-gain-deduction.md). Renamed from lic_capital_gain_deduction, which took the already-halved figure: migration 0025 doubles existing rows back to the advised amount
├── conduit_foreign_income    TEXT (decimal)  Memo: the part of unfranked_amount the payer declared to be conduit foreign income (CFI) — recorded within that amount, never in addition to it, and rejected 422 if it exceeds it. To the Australian resident this system reports for, an unfranked dividend declared to be CFI is assessable (docs/ato/amma-statement-guidance-notes.md, Part B item 13U), and it is counted through unfranked_amount; every report reads this column for reference only (the annual tax report prints it as a memo column) so the dollars are never counted twice. CFI is NANE only for a foreign resident
├── trust_income              BOOLEAN
├── entitlement_date          DATE (nullable)  Trust distributions only (CHECK: NULL unless trust_income; non-trust writes also rejected 422): the date the holder became presently entitled — usually the distribution period's end. Trust income is assessed in the year of present entitlement regardless of payment (ATO QC 23087, docs/ato/trust-income-timing.md), so when set the tax summary attributes every component of the row (FY bucket and AUD-conversion month) by this date; absent, date_paid behaviour is unchanged
├── reinvestment_trade_id     INTEGER FK→trades.id (nullable, for DRP linkage)  Read-only provenance managed by the reinvest operation: set by POST /income/:id/reinvest, cleared by DELETE /income/:id/reinvest — PUT /income never writes it (an edit preserves the link); while set, the DRP trade is frozen (PUT/DELETE /trades reject it), DELETE /income is refused, and the income row's own listing, holding account, currency, dates and cash components are frozen too (PUT /income rejects a change to any of them, naming the field)
├── currency                  TEXT FK→currencies.code   ISO 4217; tax summary converts to AUD by date_paid month — entitlement_date month when that governs (default AUD)
├── buyback_trade_id          INTEGER FK→trades.id (nullable)  Buy-back dividend components only: the participation Sell this row was created with (the row is managed by the participation — PUT/DELETE /income reject it; DELETE /sells on the Sell removes it)
├── holding_account_id        INTEGER FK→holding_accounts.id  The account the distribution was paid to — decides whose DRP enrolment applies and where a reinvestment trade lands (defaults to the seeded default account)
├── amount_per_security       TEXT (decimal, nullable)  Optional statement cross-check, supplied only together with securities_held: their product, cent-rounded, must equal franked + unfranked + foreign source income (422 otherwise). Informational/validation-only — no report uses it (mirrors trades.statement_total)
├── securities_held           TEXT (decimal, nullable)  See amount_per_security — the statement's securities-held count
└── tax_deferred_amount       TEXT (decimal, nullable)  Non-AMIT trust statements only (CHECK: NULL unless trust_income and the value is ≥ 0; non-trust/negative writes also rejected 422): the statement's tax-deferred amount — a CGT event E4 cost-base reduction (docs/ato/cgt-non-assessable-payments.md). Informational: the reduction itself is entered as a ReturnOfCapital corporate action and no calculation reads this figure — the E4 cross-check report flags a row whose non-zero amount has no same-FY action on the listing

interest_income              Interest income — bank, term-deposit, or broker-cash interest (no listing, so not an income row). The tax summary reports an Australian-source row's gross as its interest_income line (question 10 label L, TFN withholding joining the combined withholding line, label M) and a foreign-source row's gross as its foreign_interest_income line (question 20 label E — assessable foreign source income, its foreign tax withheld joining the FITO line); both count in gross assessable investment income
├── id                    INTEGER PK
├── date_paid             DATE   The date the interest was **credited** — the day it was credited, received, or applied or dealt with on the holder's behalf or as they direct (docs/ato/investment-income-timing.md, QC 72101), which for a term deposit run to maturity is the maturity date. Never the date the funds became reachable: a 30 June credit withdrawable on 2 July is FY2026 interest. Its month sets the ATO FX conversion month and the Australian financial year the interest is assessed in (a July date belongs to the next FY)
├── amount                TEXT (decimal)  Gross interest including any amount withheld (the gross figure is declared)
├── tfn_withholding_tax   TEXT (decimal)  TFN amount withheld from the gross interest; joins the tax summary's withholding line. Australian-source rows only (TFN amounts are withheld by Australian investment bodies; a foreign-source write with one is rejected 422)
├── foreign_source        INTEGER (0/1, CHECK)  Whether the payer is foreign (e.g. a US broker's cash / money-market sweep fund): routes the row to the tax summary's foreign_interest_income (20E) line instead of interest_income (10L). Default 0 (Australian) — existing rows keep their pre-migration classification
├── foreign_tax_paid      TEXT (decimal)  Foreign tax withheld from the gross amount; joins the tax summary's FITO line (A$1,000 de-minimis). Foreign-source rows only, never negative (CHECK; Australian-source/negative writes also rejected 422)
├── currency              TEXT FK→currencies.code   ISO 4217; tax summary converts to AUD by date_paid month (default AUD)
├── source                TEXT (nullable)  Free-text source description (e.g. "ANZ savings account"); informational only
└── holding_account_id    INTEGER FK→holding_accounts.id (nullable)  The holding account the interest was paid on (e.g. a broker cash account); NULL for interest from outside the portfolio's accounts; informational only

investment_expenses          Deductible investment expenses — the cost of earning assessable investment income (interest on borrowed money, management/adviser fees, account-keeping fees, subscriptions). The tax summary nets these against gross assessable investment income per financial year
├── id                    INTEGER PK
├── date_incurred         DATE   Its month sets the ATO FX conversion month and the Australian financial year the deduction falls in (a July date belongs to the next FY). One row is one year: an expense apportioned across years (borrowing expenses over 5 years or the loan term; a prepayment failing the 12-month rule, split by days) is entered as one row per financial year carrying that year's share — docs/ato/expense-time-apportionment.md, a documented Known limitation rather than a modelled split
├── expense_type          TEXT   LoanInterest | ManagementFee | AdviceFee | AccountKeepingFee | Subscription | Other (CHECK-enforced enum)
├── amount                TEXT (decimal)  The deductible amount — post-apportionment, the figure that goes on the return and the value the tax summary totals; never negative (write-time 422)
├── gross_amount          TEXT (decimal, nullable)  Optional provenance (no calculation reads it): the pre-apportionment gross expense; never negative (write-time 422)
├── deductible_percentage TEXT (decimal, nullable)  Optional provenance (no calculation reads it): the percentage of gross_amount the user determined deductible; within 0–100, and — supplied with gross_amount — gross × pct must cent-round to amount (write-time 422)
├── currency              TEXT FK→currencies.code   ISO 4217; tax summary converts to AUD by date_incurred month (default AUD)
├── description           TEXT (nullable)  Free-text note
├── listing_id            INTEGER FK→listings.id (nullable)  The listing the expense relates to; NULL = portfolio-wide
└── holding_account_id    INTEGER FK→holding_accounts.id (nullable)  The holding account the expense relates to; NULL = portfolio-wide
                          Brokerage is not recorded here (it forms a trade's CGT cost base); the LIC capital gain deduction comes from the income row's own lic_capital_gain_amount field

amma_statements              Annual AMIT Member Annual (AMMA) statements
├── id                              INTEGER PK
├── listing_id                      INTEGER FK→listings.id
├── tax_year_end_date               DATE         Always a 30 June date (write-time 422 otherwise) — e.g. 2024-06-30 for FY2024; reports bucket by its calendar year
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
├── capital_losses_applied          TEXT (decimal)  Informational only — trust-level losses the statement's CGT gains are already net of; not the member's loss (no calculation reads it)
├── tax_deferred_amount             TEXT (decimal)  Informational only — not a cost-base driver (reflected in cost_base_adjustment)
├── tax_free_amount                 TEXT (decimal)  Informational only — not a cost-base driver (reflected in cost_base_adjustment)
├── cost_base_adjustment            TEXT (decimal)  Per-unit AMIT cost base net amount; sole cost-base driver (+ reduces, − increases)
├── tfn_withholding_tax             TEXT (decimal)
├── currency                        TEXT FK→currencies.code   ISO 4217; tax summary converts to AUD by tax_year_end_date month (default AUD)
└── holding_account_id              INTEGER FK→holding_accounts.id  The account the statement covers — a registry issues one AMMA statement per holder account (defaults to the seeded default account)

amit_adjustments             Links a purchase parcel to an AMMA statement
├── id                   INTEGER PK
├── amma_statement_id    INTEGER FK→amma_statements.id
├── trade_id             INTEGER FK→trades.id  Must be Buy or DRP
└── quantity             TEXT (decimal)       Units of the parcel covered by the adjustment, in the parcel's as-acquired units (the basis trades.quantity caps).
                                              A split/bonus issue between the parcel's acquisition and the statement's tax_year_end_date is re-based
                                              automatically: the reduction is (this quantity re-based into the year-end basis) × cost_base_adjustment,
                                              since the statement's per-unit figure is per unit as its own tax year saw them
UNIQUE (amma_statement_id, trade_id)  One adjustment per statement per parcel: two rows for the same parcel would
                                      apply the statement's per-unit cost_base_adjustment to it twice, and CGT event
                                      E10's nil floor turns an over-reduction into a capital gain never made

ess_statements               Employee share scheme statements — the income side of an ESS interest (Item 12 discount, declared in the year of the taxing point)
├── id                          INTEGER PK
├── listing_id                  INTEGER FK→listings.id
├── holding_account_id          INTEGER FK→holding_accounts.id  The account the interests vest into (defaults to the seeded default account)
├── taxing_point_date           DATE   The taxing point: sets the assessable financial year and the vest Buy's acquisition/settlement date
├── quantity                    TEXT (decimal)  Shares that vest — the vest Buy's quantity
├── market_value_per_share      TEXT (decimal)  Market value per share at the taxing point — the vest Buy's price (the reset cost base)
├── taxed_upfront_eligible      TEXT (decimal)  Item 12 label D: taxed-upfront discount eligible for the $1,000 reduction
├── taxed_upfront_not_eligible  TEXT (decimal)  Item 12 label E: taxed-upfront discount not eligible for the reduction
├── deferral_discount           TEXT (decimal)  Item 12 label F: deferral-scheme discount (the RSU case)
├── pre_2009_cessation_discount TEXT (decimal)  Pre-1 July 2009 cessation discounts assessable this year (label G)
├── foreign_source_discount     TEXT (decimal)  Item 12 label A: the foreign-source portion of the above discounts (a memo already within them; surfaced by the tax summary for the FITO calc, not added on top)
├── tfn_withholding             TEXT (decimal)  Item 12 label C: TFN amounts withheld from the discounts; folded into the tax summary's TFN line
├── currency                    TEXT FK→currencies.code   ISO 4217; tax summary converts to AUD by taxing_point_date month (default AUD)
├── aud_taxed_upfront_eligible      TEXT (decimal, nullable)  Statement-AUD override for label D: the employer statement's stated AUD figure (release-date spot rate — what the ATO prefill carries); when present the tax summary reports it verbatim instead of RBA-converting, when absent behaviour is unchanged. Only accepted on a non-AUD statement (422 otherwise)
├── aud_taxed_upfront_not_eligible  TEXT (decimal, nullable)  Statement-AUD override for label E (same semantics)
├── aud_deferral_discount           TEXT (decimal, nullable)  Statement-AUD override for label F (same semantics; the RSU case)
├── aud_pre_2009_cessation_discount TEXT (decimal, nullable)  Statement-AUD override for label G (same semantics)
└── aud_foreign_source_discount     TEXT (decimal, nullable)  Statement-AUD override for the label A memo (same semantics)
                             The assessable discount (D+E+F+G − the applied $1,000 reduction) reaches the tax summary; the vest Buy is created by POST /ess_statements/:id/vest (entities::ess_vest). While the vest Buy exists, the fields it was created from (listing, account, taxing point, quantity, market value, currency) are frozen — the income-side fields (discount labels, TFN withheld, statement-AUD overrides) stay editable, since the employer's annual ESS statement arrives after the vest

parcel_allocations           Links sell parcels to the purchase parcels they consume
├── id                   INTEGER PK
├── sale_trade_id        INTEGER FK→trades.id  Must be Sell
├── purchase_trade_id    INTEGER FK→trades.id  Must be Buy or DRP
└── quantity_allocated   TEXT (decimal)

drp_enrolments               Dated DRP enrolment periods per (listing, holding account) — a holding can enrol, unenrol, and re-enrol, and the same listing may be enrolled in one account and not another
├── id                   INTEGER PK
├── listing_id           INTEGER FK→listings.id
├── holding_account_id   INTEGER FK→holding_accounts.id  The account the enrolment applies to (defaults to the seeded default account)
├── enrolment_date       TEXT   First day of the period (inclusive)
├── unenrolment_date     TEXT (nullable)  Day the unenrolment takes effect (exclusive); NULL = open-ended (currently enrolled)
└── residual_handling    TEXT   CarryForward | PayOut  Leftover-cash policy for the period (default CarryForward)
                         CHECK: unenrolment_date (when set) is after enrolment_date
                         Write-time invariant: a (listing, holding account)'s periods must not overlap (so at most one is open per account)

cgt_settings                 Singleton CGT settings row (CHECK id = 1)
├── id                   INTEGER PK  Always 1 (CHECK-enforced singleton)
└── opening_capital_loss TEXT (decimal)  Net capital loss carried forward from before the first recorded year (AUD, non-negative); starting balance for the net-capital-gain loss chain

corporate_actions            Corporate actions per listing (company returns of capital — CGT event G1 — share splits/consolidations — TD 2000/10 — non-assessable bonus issues, rights issues, off-market buy-backs, scrip-for-scrip takeovers, demergers, and worthless/delisted shares — CGT events G3/C2)
├── id                INTEGER PK
├── action_type       TEXT   ReturnOfCapital | ShareSplit | BonusIssue | RightsIssue | BuyBack | ScripForScrip | Demerger | WorthlessShares (CHECK-enforced enum; the extension point for future actions). Per-type CHECKs tie each payload below to its type
├── listing_id        INTEGER FK→listings.id
├── date              TEXT   ReturnOfCapital: payment date — parcels still held then and entitled to the payment (see record_date) are affected; with no record_date, entitlement falls back to "acquired on/before this date". ShareSplit: conversion date — parcels acquired before it are converted (a trade dated on it is already in post-split units). BonusIssue: issue date — parcels acquired before it receive bonus units (a trade dated on it is ex-bonus). RightsIssue: record date — units held before it earn the entitlement (a trade dated on it is ex-rights). BuyBack: the buy-back date — participations are dated on/after it. ScripForScrip: the exchange date — every parcel still open on it is exchanged; the closing Sell and replacement Buys are dated on it. Demerger: the demerger date — every head parcel still open on it participates; the closing Sell and the head/demerged Buys are dated on it. WorthlessShares: the declaration date (G3) or deregistration/cancellation date (C2) — every parcel still open on it is closed at nil proceeds by the recognise operation
├── amount_per_unit   TEXT (decimal, nullable)  ReturnOfCapital only: per-unit non-assessable payment (positive); reduces affected parcels' cost bases
├── record_date       TEXT (date, nullable)  ReturnOfCapital only: the date entitlement to the payment was fixed (CHECK, 0023: only on ReturnOfCapital rows, and never after `date`). Parcels acquired before it earn the payment; one acquired on/after it is ex-entitlement and is not reduced. NULL = not recorded, and the payment date decides instead
├── currency          TEXT FK→currencies.code (nullable)  ReturnOfCapital: write-time validated to equal the currency of the parcels the payment reaches (422 otherwise) — the reduction is per parcel, in the parcel's own currency, and amounts are never netted across currencies. RightsIssue: the exercise price's currency. BuyBack: the buy-back price's currency
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
├── scrip_old_units   TEXT (decimal, nullable)  ScripForScrip only: see scrip_new_units
├── scrip_cash_per_unit TEXT (decimal, nullable)  ScripForScrip only: optional cash component (the partial rollover, Example 27 in docs/ato/takeovers-and-scrip-for-scrip.md) — cash received per OLD unit exchanged, in scrip_cash_currency (positive). CHECKs (0007): only on ScripForScrip rows, and the three cash columns are all present or all NULL (all NULL = the all-scrip full rollover)
├── scrip_market_value  TEXT (decimal, nullable)  ScripForScrip only: market value of one NEW (replacement) unit just after issue, in scrip_cash_currency (positive) — the scrip side of the market-value cost-base apportionment. Present exactly when scrip_cash_per_unit is (CHECK)
├── scrip_cash_currency TEXT FK→currencies.code (nullable)  ScripForScrip only: currency of the cash and market value (the shared currency column stays NULL for ScripForScrip). Present exactly when scrip_cash_per_unit is (CHECK)
├── demerger_listing_id INTEGER FK→listings.id (nullable)  Demerger only: the demerged entity's listing (CHECK: differs from listing_id, the head entity)
├── demerger_new_units  TEXT (decimal, nullable)  Demerger only: every demerger_held_units units of listing_id held at the demerger date receive demerger_new_units units of demerger_listing_id (both positive)
├── demerger_held_units TEXT (decimal, nullable)  Demerger only: see demerger_new_units
├── demerger_cost_base_pct TEXT (decimal, nullable)  Demerger only: the percentage of each parcel's cost base apportioned to the new interests in the demerged entity (the head-entity-advised step 2 percentage, 0 < pct < 100); the head parcels keep the rest
└── worthless_event   TEXT (nullable)  WorthlessShares only: which CGT event the loss is recognised under — G3Declaration (s 104-145, liquidator/administrator declaration) or C2Cancellation (s 104-25, deregistration); CHECK-enforced enum. Both close every open parcel at nil proceeds (the recognise operation); the discriminator records the legal basis

rights_sales                 Disposals of renounceable rights — sold on-market, lapsed, or compensated by a retail premium (TR 2017/4) — recorded by POST /corporate_actions/:id/sell_rights against a RightsIssue action. A CGT event on the rights themselves: the share holding is untouched, and the realised-gains report surfaces each row as a source = RightsSale disposal
├── id                 INTEGER PK
├── rights_action_id   INTEGER FK→corporate_actions.id   The RightsIssue disposed against; the action is frozen (no edit/delete) while rows reference it. Cumulative units — together with exercise trades — are capped at the record-date entitlement (write-time check shared with the exercise operation)
├── date               TEXT   Sale (or lapse/expiry) date; never before the issue's record date (write-time check)
├── units              TEXT (decimal)  Rights disposed of, in record-date (as-issued) rights units (positive)
├── proceeds_per_right TEXT (decimal)  Per-right capital proceeds in the issue's currency (the action's currency column — no column here; non-negative, default 0 = a lapse). A renounceable-offer retail premium is entered here
├── rights_cost        TEXT (decimal)  Total paid to acquire the disposed rights (the purchased-rights case; non-negative, default 0 = rights issued free, nil cost base) — apportioned over the allocations by the realised-gains report, so nil proceeds on a paid right realises a capital loss
├── fx_rate            TEXT (decimal)  Manual foreign-per-AUD fallback rate (reports prefer the ATO/RBA rate for the sale month)
└── holding_account_id INTEGER FK→holding_accounts.id  The account the disposal is reported under (defaults to the seeded default account)
                       Rows are immutable (no PUT) — delete and re-enter to amend; deleting frees the entitlement

rights_sale_allocations      Which original parcels the sold rights are anchored to: free rights are deemed acquired with the original shares, so each allocation's 12-month discount clock runs from its parcel's (possibly deemed) acquisition date. Unlike parcel_allocations these consume no parcel units — the shares are still held
├── id                INTEGER PK
├── rights_sale_id    INTEGER FK→rights_sales.id (ON DELETE CASCADE)
├── purchase_trade_id INTEGER FK→trades.id   A Buy/DRP of the issue's listing dated before the record date; the parcel is frozen against PUT/DELETE /trades while referenced. Cumulative rights anchored to a parcel (across the action's sales) are capped at the entitlement its record-date units earned
└── units             TEXT (decimal)  Rights anchored to this parcel (positive); a sale's allocations sum exactly to its units

attachments                  Supporting documents for an activity; bytes stored in the DB (captured by the weekly backup)
├── id                 INTEGER PK
├── trade_id           INTEGER FK→trades.id (nullable, ON DELETE CASCADE)            Owner (exactly one of the six is set)
├── income_id          INTEGER FK→income.id (nullable, ON DELETE CASCADE)            Owner (exactly one of the six is set)
├── amma_statement_id  INTEGER FK→amma_statements.id (nullable, ON DELETE CASCADE)   Owner (exactly one of the six is set)
├── ess_statement_id   INTEGER FK→ess_statements.id (nullable, ON DELETE CASCADE)    Owner (exactly one of the six is set) — e.g. the annual ESS statement document (0014)
├── interest_income_id INTEGER FK→interest_income.id (nullable, ON DELETE CASCADE)   Owner (exactly one of the six is set) — e.g. a broker statement whose only activity is cash interest (0014)
├── corporate_action_id INTEGER FK→corporate_actions.id (nullable, ON DELETE CASCADE) Owner (exactly one of the six is set) — e.g. a demerger booklet or scrip-exchange offer document (0017)
├── filename           TEXT             Original upload filename, preserved for download
├── content_type       TEXT             application/pdf | image/png | image/jpeg | text/plain (allowlist, CHECK-enforced; text/plain since 0014 — plain-text records like crypto exchange exports and DRP advices)
├── byte_size          INTEGER          Size of content in bytes (informational)
├── checksum           TEXT             SHA-256 of content, hex (integrity / duplicate detection)
├── uploaded_at        TEXT             RFC 3339 timestamp the attachment was stored
└── content            BLOB             The file bytes
                       CHECK: exactly one of trade_id / income_id / amma_statement_id / ess_statement_id / interest_income_id / corporate_action_id is non-null

closing_prices               Daily closing-price history per listing (collected by the price-import job; see API.md Closing prices)
├── id          INTEGER PK (AUTOINCREMENT, 0021)  Server-assigned surrogate key — the row's identity for the audit trail (row_history.row_id). AUTOINCREMENT so a discarded row's id is never reused by a later row, which would graft its history onto the new one. Writes address a row by (listing_id, price_date), never by id
├── listing_id  INTEGER FK→listings.id   Part of UNIQUE(listing_id, price_date)
├── price_date  TEXT             'YYYY-MM-DD'; part of UNIQUE(listing_id, price_date) — the trading day in the exchange's timezone (for Crypto: the UTC date whose daily candle completes at 00:00 UTC at its end). One row per (listing, date), the former primary key before 0021 added the surrogate id
├── price       TEXT (decimal, nullable)  Closing price in the listing's quote currency (never AUD-converted; reports convert at read time). NULL exactly when status = error (CHECK)
├── source      TEXT             Provider that produced the row (e.g. yahoo); 'manual' exactly when origin = manual (CHECK), so the provider slot and the origin cannot drift
├── fetched_at  TEXT             RFC 3339 UTC timestamp of the fetch — for a manual row, of the entry that recorded it
├── status      TEXT             ok | error (CHECK-enforced enum)
├── error       TEXT (nullable)  Failure detail; NULL exactly when status = ok (CHECK). A failed fetch is stored, never silently missing, and a re-run replaces it
├── origin      TEXT             fetched | manual (CHECK-enforced enum, 0020). A manual row is always status = ok (CHECK) — there is no hand-entered fetch failure
├── sourced_from TEXT (nullable) Where a manual price was taken from (e.g. 'asx.com.au closing report'); NULL exactly when origin = fetched (CHECK)
└── reason      TEXT (nullable)  Why manual entry was needed (e.g. 'provider serves no candle since the delisting'); NULL exactly when origin = fetched (CHECK)

report_snapshots             Stored daily results of the price-dependent reports (written by the report-snapshot job or on demand; see API.md Report snapshots)
├── report        TEXT             portfolio_overview | unrealised_gains | performance (CHECK-enforced enum); part of PK
├── snapshot_date TEXT             'YYYY-MM-DD'; part of PK — one stored result per (report, date)
├── generated_at  TEXT             RFC 3339 UTC timestamp of the run that produced the stored result
├── stale         INTEGER          0 | 1 (CHECK): 1 = a back-dated fact was recorded after generation, set by the staleness triggers (below) in the same transaction as the fact; cleared by regeneration
├── provisional   INTEGER          0 | 1 (CHECK, 0015): 1 = some price conversion in the generation run used a fallback-month FX rate (the valuation month's RBA rate was not published yet). Distinct from stale (no trigger sets it — FX imports fire none); cleared when regeneration converts every price at a real-month rate (the RBA-import true-up and the snapshot job's window do this automatically)
└── rows_json     TEXT             The report's response rows as JSON; money values inside are Decimal strings (the API's serialisation), kept in TEXT — never a REAL/float

job_runs                     Run history of the scheduled/on-demand maintenance jobs (one row per run, appended each run; pruned to the newest 20 per job in the same write)
├── id          INTEGER PK       Autoincrement — run order (newest run = highest id); indexed with name for per-job lookups
├── name        TEXT             Registry job name (e.g. backup, rba-fx-import)
├── started_at  TEXT             RFC 3339 timestamp the run began
├── finished_at TEXT             RFC 3339 timestamp the run ended
├── success     INTEGER          1 if the run succeeded, 0 if it failed
└── error       TEXT (nullable)  Human-readable error when success = 0, else NULL

row_history                  Append-only audit trail of the financial fact tables (written by database triggers on every UPDATE/DELETE; inspected via POST /reports/row_history — see API.md Row history)
├── id          INTEGER PK       Autoincrement — write order (newest entry = highest id); indexed with (table_name, row_id) for per-row lookups
├── table_name  TEXT             The audited table (CHECK-enforced enum: trades, parcel_allocations, income, interest_income, amma_statements, amit_adjustments, ess_statements, transfers, corporate_actions, inheritances, rights_sales, rights_sale_allocations, investment_expenses, drp_enrolments, cgt_settings, attachments, listings, listing_renames)
├── row_id      INTEGER          The audited row's id
├── operation   TEXT             UPDATE | DELETE (CHECK-enforced enum) — INSERTs are not recorded (until first changed, the live row is its own record)
├── changed_at  TEXT             RFC 3339 UTC timestamp of the write, millisecond precision
└── old_row     TEXT             The prior row as a JSON object; TEXT decimal columns stay JSON strings (exact). attachments.content (a BLOB) is the one excluded column — filename/byte_size/checksum still identify the file
```

## Relationships

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
                       listings ──< transfers
                       listings ──< ess_statements
                       listings ──< inheritances
                       listings ──< investment_expenses (nullable; portfolio-wide expense leaves it NULL)
                       holding_accounts ──< trades, income, amma_statements, drp_enrolments, ess_statements, inheritances
                       holding_accounts ──< investment_expenses (nullable; portfolio-wide expense leaves it NULL)
                       holding_accounts ──< interest_income (nullable; interest from outside the portfolio's accounts leaves it NULL)
                       holding_accounts ──< transfers (from_account_id + to_account_id)
                       transfers ──< trades (transfer_id)
                       transfers >── trades (fee_sale_trade_id; the crypto network-fee disposal Sell)
                       ess_statements ──< trades (ess_statement_id; the cost-base-reset vest Buy)
                       inheritances ──< trades (inheritance_id; the inherited-parcel Buy)
                       trades (DRP) ──< income (reinvestment_trade_id)
                       corporate_actions (RightsIssue) ──< trades (rights_action_id)
                       corporate_actions (RightsIssue) ──< rights_sales ──< rights_sale_allocations >── trades (purchase_trade_id; date-anchoring only — consumes no units)
                       holding_accounts ──< rights_sales
                       corporate_actions (BuyBack) ──< trades (buyback_action_id)
                       corporate_actions (ScripForScrip) ──< trades (scrip_action_id)
                       corporate_actions (ScripForScrip) >── listings (scrip_listing_id)
                       corporate_actions (Demerger) ──< trades (demerger_action_id)
                       corporate_actions (Demerger) >── listings (demerger_listing_id)
                       corporate_actions (WorthlessShares) ──< trades (worthless_action_id; the recognise closing Sell)
                       trades (buy-back Sell) ──< income (buyback_trade_id)
                       trades, income, amma_statements, ess_statements, interest_income, corporate_actions ──< attachments (exactly one owner; ON DELETE CASCADE)
                       listings ──< closing_prices (one row per listing per trading day)
                       listings ──< listing_renames
                       exchanges ──< listing_renames (old_exchange_mic + new_exchange_mic, nullable)
                       trades, parcel_allocations, income, amma_statements, amit_adjustments,
                       corporate_actions, closing_prices ──> report_snapshots.stale (staleness triggers)
                       every audited table (see row_history.table_name) ──> row_history (audit triggers on UPDATE/DELETE; table_name/row_id, not FKs — entries outlive their row)

currencies ──< exchanges, listings, trades (currency + brokerage_currency), income, interest_income, amma_statements, corporate_actions (currency + scrip_cash_currency), ess_statements, investment_expenses, inheritances
```

Each `attachments` row belongs to exactly one activity via one of six nullable foreign keys (`trade_id` / `income_id` / `amma_statement_id` / `ess_statement_id` / `interest_income_id` — the last two added by 0014 — / `corporate_action_id` — added by 0017, e.g. a demerger booklet or scrip-exchange offer document — each rebuild of the table re-creating its audit triggers), with a `CHECK` enforcing that exactly one is set — a real foreign key keeps referential integrity to the owning row, and `ON DELETE CASCADE` removes an activity's attachments when it is deleted. File contents live in the `content` BLOB so the weekly DB backup captures the documents with no separate file store.

`report_snapshots` has no foreign keys of its own (`report` is an in-code enum, the rows are a JSON payload), but every dated fact table writes to it through **staleness triggers** (0001_schema.sql): inserting, updating, or deleting a row in `trades`, `parcel_allocations` (dated by its sale trade), `income` (by `date_paid`), `amma_statements` / `amit_adjustments` (by the statement's `tax_year_end_date`), or `corporate_actions` sets `stale = 1` on every snapshot dated on or after the fact — an update from the earlier of the old and new dates — atomically with the fact write, so no write path (entity CRUD, Sells, transfers, corporate-action operations, DRP reinvestment) can bypass the invalidation. Revising a stored ok closing price (or erroring it out) likewise stales snapshots from its date, since they were valued at it. `closing_prices` carries only that one `AFTER UPDATE` trigger — no INSERT or DELETE counterpart — because neither can invalidate a snapshot: a new price fills a date that was blocked (so no snapshot exists to stale), and the only deletable row is an errored one (`DELETE /closing_prices/:listing_id/:price_date` rejects an ok row 422), whose date was never valued. A **manually entered** price (0020) changes neither premise: it is stored `status = ok`, so it is undeletable like any other price, and replacing a stored ok price with one is an ordinary UPDATE the trigger already catches. `rights_sales` / `rights_sale_allocations` and `interest_income` are the deliberate exceptions: no snapshotted report reads them (a rights sale changes no holding quantity and no parcel cost base — its effect is confined to the live-computed CGT reports; interest reaches only the tax summary, which is not snapshotted), so they carry no trigger set.

`row_history` is the **append-only audit trail** (0013_row_history.sql). `AFTER UPDATE` / `AFTER DELETE` triggers on every audited table record the prior row (as JSON), the operation, and a UTC timestamp — in the writing transaction itself, so no write path can bypass it and a rejected (rolled-back) write leaves no phantom entry, while cascade deletes (an activity's attachments, a rights sale's allocations) are recorded like direct ones. `table_name`/`row_id` are deliberately *not* foreign keys: a history entry must outlive its row (the DELETE case). Audited (scope decision 2026-07-14): every user-entered table whose values feed a calculation — the financial fact tables plus `cgt_settings` and, of the reference data, `listings` alone (its `amit`/`amit_from`/`security_type`/`preference` flags retroactively change tax calculations). `listing_renames` (0018) joined the audited set too, which required rebuilding `row_history` itself to extend its `table_name` CHECK (a table-level CHECK SQLite cannot `ALTER`) — the *live* `row_history_append_only_*` guard triggers, and the `listings` trigger pair carrying the new `price_symbol` column, come from 0018, not 0013. The listings pair was re-created once more in 0024, which added `amit_from`. `closing_prices` joined the audited set in 0021: 0020 made a price **hand-enterable**, which retired the "import-managed and re-importable" ground its original exclusion rested on — a manual price is a user-entered value feeding every valuation, and overwriting one with another used to discard the superseded figure and the `sourced_from`/`reason` given for it. Auditing it needed a row identity `row_history.row_id` could key on, so 0021 rebuilt the table with an AUTOINCREMENT surrogate `id` (the old composite key kept as `UNIQUE(listing_id, price_date)`) and rebuilt `row_history` again to extend the `table_name` CHECK. Because `closing_price::db_store` upserts on the natural key, a replacing write is an UPDATE and lands in the trail; the DELETE trigger covers discarding an errored row, the only delete the API allows. The remaining import-managed reference data (`currencies`, `mic_registry`, `rba_fx_rates`), tables that only influence values persisted onto trades at write time (`exchanges`, `exchange_holidays`), the identity-only `holding_accounts`, and derived state (`report_snapshots`, `job_runs`) are out of scope. Retention (decision 2026-07-14): entries are kept forever — the table is itself append-only, enforced by its own `BEFORE UPDATE`/`BEFORE DELETE` `RAISE(ABORT)` triggers, and deliberately has no pruning job. A migration adding a column to an audited table must recreate that table's two `*_row_history_*` triggers with the new column list.

`rba_fx_rates` is standalone reference data (no foreign keys); it is looked up by `(currency, month)`. `job_runs` is likewise standalone: `name` is the in-code job name (not a foreign key), and each scheduled or manual run appends a row, pruning that job's history to the newest 20 rows in the same transaction — so an intermittent failure that later succeeds stays visible (surfaced by `GET /jobs` and the [health report](API.md#health)) without the table growing unboundedly. `cgt_settings` is also standalone: a singleton row (`CHECK (id = 1)`) holding the entered opening carried-forward capital loss consumed by the [net capital gain report](API.md#net-capital-gain).

`mic_registry` is standalone reference data (no foreign keys), keyed by `mic`. It is populated from the ISO 10383 list and used only to validate curated `exchanges` (see the [exchange MIC validation report](API.md#exchange-mic-validation)); it is *not* the operational exchange table and carries no currency/timezone/settlement data.

`currencies` is reference data keyed by `code` (it has no outgoing foreign keys). It is populated from the ISO 4217 (SIX Group) and ISO 24165 (DTIF) feeds and seeded with a baseline of common currencies (the seed migration), and is the recognised list that **every** currency code in the model is foreign-keyed to: `exchanges.currency`, `listings.currency`, `trades.currency`, `trades.brokerage_currency`, `income.currency`, `interest_income.currency`, `amma_statements.currency`, `corporate_actions.currency`, `corporate_actions.scrip_cash_currency`, `ess_statements.currency`, `investment_expenses.currency`, and `inheritances.currency` all reference `currencies.code`, so an unrecognised currency is rejected at write time. `minor_units` is informational only — stored amounts remain arbitrary-precision Decimal and are never rounded to it. A Crypto listing's ticker is additionally validated against the `DigitalToken` rows at write time (matched on `code` or `short_name`).

`listings.exchange_mic` is nullable for exactly the Crypto security type (CHECK-enforced both ways): a crypto asset trades on no MIC-coded venue, settles same-day, and has no holiday calendar. Because `UNIQUE(exchange_mic, ticker)` treats NULLs as distinct, a partial unique index makes exchange-less listings unique by ticker.

Decimal values are stored as TEXT to preserve arbitrary precision.
