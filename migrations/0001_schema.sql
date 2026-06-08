-- Consolidated baseline schema + seed data.
--
-- This squashes the entire incremental migration history into the single shape
-- it produced. The application was pre-1.0 with no live data to preserve, so the
-- step-by-step migrations (REAL->TEXT conversions, table rebuilds to add foreign
-- keys, column additions, the snapshot-staleness triggers) were collapsed here.
-- From this point forward, schema changes are added as new numbered migrations.
--
-- All monetary/quantity columns are TEXT (arbitrary-precision Decimal); no column
-- is ever REAL, so the float-imprecision concerns of the old REAL->TEXT
-- conversions cannot recur.
--
-- Layout: reference/parent tables first, then the domain tables (parents before
-- children so REFERENCES resolve cleanly), then the lone partial index, then the
-- snapshot-staleness triggers, then the seed data.

-- ---------------------------------------------------------------------------
-- Reference / parent tables
-- ---------------------------------------------------------------------------

-- Recognised currencies (ISO 4217 fiat + ISO 24165 digital tokens).
-- Every currency code in the model is foreign-keyed to this table.
CREATE TABLE currencies (
    code         TEXT PRIMARY KEY,                                      -- ISO 4217 alpha code (fiat) or ISO 24165 DTI (token)
    kind         TEXT NOT NULL CHECK (kind IN ('Fiat', 'DigitalToken')),
    numeric_code TEXT,                                                  -- ISO 4217 numeric code (fiat only; NULL for tokens)
    name         TEXT NOT NULL,                                         -- currency name (fiat) or token long name
    short_name   TEXT,                                                  -- token short name / ticker (NULL when none)
    minor_units  INTEGER,                                               -- ISO 4217 minor unit / token decimals; informational, NULL when N.A.
    source       TEXT NOT NULL CHECK (source IN ('Iso4217', 'Iso24165'))
);

-- Operational exchanges (the curated, settlement-aware list the app trades against).
CREATE TABLE exchanges (
    mic             TEXT    PRIMARY KEY,
    name            TEXT    NOT NULL,
    country         TEXT    NOT NULL,
    currency        TEXT    NOT NULL REFERENCES currencies(code),
    timezone        TEXT    NOT NULL,
    settlement_days INTEGER NOT NULL,
    -- Local close time 'HH:MM' in the exchange's timezone. A trading day's
    -- closing price is only collected once this time has passed; both seeded
    -- exchanges (XASX, XNYS) close 16:00.
    close_time      TEXT    NOT NULL DEFAULT '16:00'
);

-- ISO 10383 MIC registry (validation list; populated by the monthly import).
CREATE TABLE mic_registry (
    mic           TEXT PRIMARY KEY,           -- the MIC (ISO 10383), e.g. 'XASX'
    operating_mic TEXT NOT NULL,              -- parent operating MIC (== mic for operating entries)
    name          TEXT NOT NULL,              -- MARKET NAME-INSTITUTION DESCRIPTION
    country_code  TEXT NOT NULL,              -- ISO 3166 alpha-2 country code
    city          TEXT,                       -- city (nullable; some entries omit it)
    status        TEXT NOT NULL,              -- ISO STATUS: ACTIVE | UPDATED | EXPIRED
    expiry_date   TEXT                        -- ISO date 'YYYY-MM-DD' when EXPIRED, else NULL
);

-- RBA F11 monthly reference rates used for ATO AUD conversion (populated by import).
CREATE TABLE rba_fx_rates (
    id        INTEGER PRIMARY KEY AUTOINCREMENT,
    currency  TEXT NOT NULL,            -- ISO 4217 code, e.g. 'USD'
    month     TEXT NOT NULL,            -- 'YYYY-MM'
    rate      TEXT NOT NULL,            -- Decimal: foreign units per 1 AUD
    UNIQUE (currency, month)
);

-- Exchange public holidays (full-closure non-trading days).
--
-- Settlement is quoted as T+n *business* days: weekends are skipped, and so are
-- the exchange's public holidays. This table records, per exchange (MIC), the
-- dates on which the market is fully closed, so settlement-date calculation can
-- skip them too (see `entities::trade::add_business_days`). Only full closures
-- are modelled; early-close days (the market still settles) are excluded.
CREATE TABLE exchange_holidays (
    mic          TEXT NOT NULL REFERENCES exchanges(mic),
    holiday_date TEXT NOT NULL,  -- ISO 'YYYY-MM-DD'; a full-closure non-trading day
    name         TEXT NOT NULL,  -- holiday name (informational)
    PRIMARY KEY (mic, holiday_date)
);

-- Last result of each maintenance job (keyed by registry job name).
CREATE TABLE job_runs (
    name        TEXT PRIMARY KEY,          -- registry job name (e.g. backup, rba-fx-import)
    started_at  TEXT    NOT NULL,          -- RFC 3339 timestamp the run began
    finished_at TEXT    NOT NULL,          -- RFC 3339 timestamp the run ended
    success     INTEGER NOT NULL,          -- 1 if the run succeeded, 0 if it failed
    error       TEXT                       -- human-readable error when success = 0, else NULL
);

-- Single-row global CGT settings (id is pinned to 1).
CREATE TABLE cgt_settings (
    id                   INTEGER PRIMARY KEY CHECK (id = 1),
    opening_capital_loss TEXT NOT NULL DEFAULT '0'
);

-- Holding accounts: every parcel/income/statement row belongs to one. The
-- seeded default account (id 1) is the account every row uses out of the box.
CREATE TABLE holding_accounts (
    id   INTEGER PRIMARY KEY,
    name TEXT NOT NULL UNIQUE
);

-- ---------------------------------------------------------------------------
-- Domain tables
-- ---------------------------------------------------------------------------

CREATE TABLE listings (
    id            INTEGER PRIMARY KEY,
    -- NULL exactly for Crypto listings: a crypto asset trades on no MIC-coded
    -- venue, settles same-day, and has no holiday calendar.
    exchange_mic  TEXT    REFERENCES exchanges(mic),
    ticker        TEXT    NOT NULL,
    name          TEXT    NOT NULL,
    isin          TEXT,
    security_type TEXT    NOT NULL CHECK(security_type IN ('Share', 'ETF', 'LIC', 'Trust', 'Crypto')),
    currency      TEXT    NOT NULL REFERENCES currencies(code),
    amit          INTEGER NOT NULL DEFAULT 0,
    preference    INTEGER NOT NULL DEFAULT 0,
    UNIQUE(exchange_mic, ticker),
    CHECK ((exchange_mic IS NULL) = (security_type = 'Crypto'))
);
-- A crypto listing has NULL exchange_mic, so the UNIQUE(exchange_mic, ticker)
-- constraint can't keep its ticker unique (NULLs compare distinct). This partial
-- index enforces uniqueness of the bare ticker across exchange-less listings.
CREATE UNIQUE INDEX listings_crypto_ticker ON listings(ticker) WHERE exchange_mic IS NULL;

CREATE TABLE transfers (
    id              INTEGER PRIMARY KEY,
    listing_id      INTEGER NOT NULL REFERENCES listings(id),
    -- The date the holding moves: the transfer-out Sell and transfer-in Buys
    -- are dated on it.
    date            TEXT    NOT NULL,
    from_account_id INTEGER NOT NULL REFERENCES holding_accounts(id),
    to_account_id   INTEGER NOT NULL REFERENCES holding_accounts(id),
    CHECK (from_account_id <> to_account_id)
);

CREATE TABLE ess_statements (
    id                          INTEGER PRIMARY KEY,
    listing_id                  INTEGER NOT NULL REFERENCES listings(id),
    -- The holding account the ESS interests vest into (an employer-plan
    -- account, typically). Defaults to the seeded default account.
    holding_account_id          INTEGER NOT NULL DEFAULT 1 REFERENCES holding_accounts(id),
    -- The taxing point: the year this date falls in is the assessable year, and
    -- the vest Buy's acquisition/settlement date (the CGT re-acquisition).
    taxing_point_date           TEXT    NOT NULL,
    -- Shares that vest at the taxing point, and their market value per share —
    -- together the cost-base-reset Buy (quantity, price). Positive for a vest.
    quantity                    TEXT    NOT NULL DEFAULT '0',
    market_value_per_share      TEXT    NOT NULL DEFAULT '0',
    -- Item 12 discount labels (all in `currency`): D taxed-upfront eligible for
    -- the $1,000 reduction, E taxed-upfront not eligible, F deferral schemes
    -- (the RSU case), G pre-1 July 2009 cessation discounts assessable this
    -- year. The assessable discount = D + E + F + G − the applied reduction.
    taxed_upfront_eligible      TEXT    NOT NULL DEFAULT '0',
    taxed_upfront_not_eligible  TEXT    NOT NULL DEFAULT '0',
    deferral_discount           TEXT    NOT NULL DEFAULT '0',
    pre_2009_cessation_discount TEXT    NOT NULL DEFAULT '0',
    -- The foreign-source portion of the above discounts (Item 12 label A): a
    -- memo already counted within the discount labels, surfaced separately by
    -- the tax summary for the foreign-income/FITO calculation. Not added on top.
    foreign_source_discount     TEXT    NOT NULL DEFAULT '0',
    -- TFN amounts withheld from the discounts (Item 12 label C); folded into the
    -- tax summary's TFN-withholding line.
    tfn_withholding             TEXT    NOT NULL DEFAULT '0',
    currency                    TEXT    NOT NULL DEFAULT 'AUD' REFERENCES currencies(code)
);

CREATE TABLE corporate_actions (
    id                INTEGER PRIMARY KEY,
    action_type       TEXT    NOT NULL
        CHECK (action_type IN ('ReturnOfCapital', 'ShareSplit', 'BonusIssue', 'RightsIssue',
                               'BuyBack', 'ScripForScrip', 'Demerger', 'WorthlessShares')),
    listing_id        INTEGER NOT NULL REFERENCES listings(id),
    -- ReturnOfCapital: payment date. ShareSplit: conversion date. BonusIssue:
    -- issue date. RightsIssue: record date (exercise is later). BuyBack: the
    -- buy-back date. ScripForScrip: exchange date. Demerger: demerger date.
    -- WorthlessShares: the declaration date (G3) or deregistration/cancellation
    -- date (C2) — every parcel of listing_id still open on it is closed.
    date              TEXT    NOT NULL,
    -- ReturnOfCapital only: per-unit payment amount in `currency`.
    amount_per_unit   TEXT,
    -- ReturnOfCapital / RightsIssue / BuyBack: the relevant amount's currency.
    currency          TEXT    REFERENCES currencies(code),
    -- ShareSplit only.
    split_new_units   TEXT,
    split_old_units   TEXT,
    -- BonusIssue only.
    bonus_units       TEXT,
    bonus_held_units  TEXT,
    -- RightsIssue only.
    rights_units      TEXT,
    rights_held_units TEXT,
    exercise_price    TEXT,
    -- BuyBack only.
    buyback_price           TEXT,
    buyback_dividend        TEXT,
    buyback_franking_credit TEXT,
    buyback_market_value    TEXT,
    -- ScripForScrip only.
    scrip_listing_id  INTEGER REFERENCES listings(id),
    scrip_new_units   TEXT,
    scrip_old_units   TEXT,
    -- Demerger only.
    demerger_listing_id   INTEGER REFERENCES listings(id),
    demerger_new_units    TEXT,
    demerger_held_units   TEXT,
    demerger_cost_base_pct TEXT,
    -- WorthlessShares only: which CGT event the user is invoking. Both produce
    -- the same loss arithmetic (close every open parcel at nil proceeds); the
    -- discriminator records the legal basis (G3 declaration vs C2
    -- deregistration) for the user's records and the recognise operation's
    -- description. CHECK-constrained enum; NULL for every other action type.
    worthless_event   TEXT    CHECK (worthless_event IN ('G3Declaration', 'C2Cancellation')),
    CHECK (action_type <> 'ReturnOfCapital'
           OR (amount_per_unit IS NOT NULL AND currency IS NOT NULL
               AND split_new_units IS NULL AND split_old_units IS NULL
               AND bonus_units IS NULL AND bonus_held_units IS NULL
               AND rights_units IS NULL AND rights_held_units IS NULL
               AND exercise_price IS NULL
               AND buyback_price IS NULL AND buyback_dividend IS NULL
               AND buyback_franking_credit IS NULL AND buyback_market_value IS NULL
               AND scrip_listing_id IS NULL AND scrip_new_units IS NULL
               AND scrip_old_units IS NULL
               AND demerger_listing_id IS NULL AND demerger_new_units IS NULL
               AND demerger_held_units IS NULL AND demerger_cost_base_pct IS NULL
               AND worthless_event IS NULL)),
    CHECK (action_type <> 'ShareSplit'
           OR (split_new_units IS NOT NULL AND split_old_units IS NOT NULL
               AND amount_per_unit IS NULL AND currency IS NULL
               AND bonus_units IS NULL AND bonus_held_units IS NULL
               AND rights_units IS NULL AND rights_held_units IS NULL
               AND exercise_price IS NULL
               AND buyback_price IS NULL AND buyback_dividend IS NULL
               AND buyback_franking_credit IS NULL AND buyback_market_value IS NULL
               AND scrip_listing_id IS NULL AND scrip_new_units IS NULL
               AND scrip_old_units IS NULL
               AND demerger_listing_id IS NULL AND demerger_new_units IS NULL
               AND demerger_held_units IS NULL AND demerger_cost_base_pct IS NULL
               AND worthless_event IS NULL)),
    CHECK (action_type <> 'BonusIssue'
           OR (bonus_units IS NOT NULL AND bonus_held_units IS NOT NULL
               AND amount_per_unit IS NULL AND currency IS NULL
               AND split_new_units IS NULL AND split_old_units IS NULL
               AND rights_units IS NULL AND rights_held_units IS NULL
               AND exercise_price IS NULL
               AND buyback_price IS NULL AND buyback_dividend IS NULL
               AND buyback_franking_credit IS NULL AND buyback_market_value IS NULL
               AND scrip_listing_id IS NULL AND scrip_new_units IS NULL
               AND scrip_old_units IS NULL
               AND demerger_listing_id IS NULL AND demerger_new_units IS NULL
               AND demerger_held_units IS NULL AND demerger_cost_base_pct IS NULL
               AND worthless_event IS NULL)),
    CHECK (action_type <> 'RightsIssue'
           OR (rights_units IS NOT NULL AND rights_held_units IS NOT NULL
               AND exercise_price IS NOT NULL AND currency IS NOT NULL
               AND amount_per_unit IS NULL
               AND split_new_units IS NULL AND split_old_units IS NULL
               AND bonus_units IS NULL AND bonus_held_units IS NULL
               AND buyback_price IS NULL AND buyback_dividend IS NULL
               AND buyback_franking_credit IS NULL AND buyback_market_value IS NULL
               AND scrip_listing_id IS NULL AND scrip_new_units IS NULL
               AND scrip_old_units IS NULL
               AND demerger_listing_id IS NULL AND demerger_new_units IS NULL
               AND demerger_held_units IS NULL AND demerger_cost_base_pct IS NULL
               AND worthless_event IS NULL)),
    CHECK (action_type <> 'BuyBack'
           OR (buyback_price IS NOT NULL AND buyback_dividend IS NOT NULL
               AND buyback_franking_credit IS NOT NULL AND currency IS NOT NULL
               AND amount_per_unit IS NULL
               AND split_new_units IS NULL AND split_old_units IS NULL
               AND bonus_units IS NULL AND bonus_held_units IS NULL
               AND rights_units IS NULL AND rights_held_units IS NULL
               AND exercise_price IS NULL
               AND scrip_listing_id IS NULL AND scrip_new_units IS NULL
               AND scrip_old_units IS NULL
               AND demerger_listing_id IS NULL AND demerger_new_units IS NULL
               AND demerger_held_units IS NULL AND demerger_cost_base_pct IS NULL
               AND worthless_event IS NULL)),
    CHECK (action_type <> 'ScripForScrip'
           OR (scrip_listing_id IS NOT NULL AND scrip_listing_id <> listing_id
               AND scrip_new_units IS NOT NULL AND scrip_old_units IS NOT NULL
               AND amount_per_unit IS NULL AND currency IS NULL
               AND split_new_units IS NULL AND split_old_units IS NULL
               AND bonus_units IS NULL AND bonus_held_units IS NULL
               AND rights_units IS NULL AND rights_held_units IS NULL
               AND exercise_price IS NULL
               AND buyback_price IS NULL AND buyback_dividend IS NULL
               AND buyback_franking_credit IS NULL AND buyback_market_value IS NULL
               AND demerger_listing_id IS NULL AND demerger_new_units IS NULL
               AND demerger_held_units IS NULL AND demerger_cost_base_pct IS NULL
               AND worthless_event IS NULL)),
    CHECK (action_type <> 'Demerger'
           OR (demerger_listing_id IS NOT NULL AND demerger_listing_id <> listing_id
               AND demerger_new_units IS NOT NULL AND demerger_held_units IS NOT NULL
               AND demerger_cost_base_pct IS NOT NULL
               AND amount_per_unit IS NULL AND currency IS NULL
               AND split_new_units IS NULL AND split_old_units IS NULL
               AND bonus_units IS NULL AND bonus_held_units IS NULL
               AND rights_units IS NULL AND rights_held_units IS NULL
               AND exercise_price IS NULL
               AND buyback_price IS NULL AND buyback_dividend IS NULL
               AND buyback_franking_credit IS NULL AND buyback_market_value IS NULL
               AND scrip_listing_id IS NULL AND scrip_new_units IS NULL
               AND scrip_old_units IS NULL
               AND worthless_event IS NULL)),
    -- WorthlessShares: only the event discriminator; every numeric payload NULL.
    CHECK (action_type <> 'WorthlessShares'
           OR (worthless_event IS NOT NULL
               AND amount_per_unit IS NULL AND currency IS NULL
               AND split_new_units IS NULL AND split_old_units IS NULL
               AND bonus_units IS NULL AND bonus_held_units IS NULL
               AND rights_units IS NULL AND rights_held_units IS NULL
               AND exercise_price IS NULL
               AND buyback_price IS NULL AND buyback_dividend IS NULL
               AND buyback_franking_credit IS NULL AND buyback_market_value IS NULL
               AND scrip_listing_id IS NULL AND scrip_new_units IS NULL
               AND scrip_old_units IS NULL
               AND demerger_listing_id IS NULL AND demerger_new_units IS NULL
               AND demerger_held_units IS NULL AND demerger_cost_base_pct IS NULL))
);

CREATE TABLE amma_statements (
    id                              INTEGER PRIMARY KEY,
    listing_id                      INTEGER NOT NULL REFERENCES listings(id),
    tax_year_end_date               TEXT    NOT NULL,
    units_held                      TEXT    NOT NULL DEFAULT '0',
    date_received                   TEXT    NOT NULL,
    australian_interest             TEXT    NOT NULL DEFAULT '0',
    australian_dividends_unfranked  TEXT    NOT NULL DEFAULT '0',
    franked_dividends               TEXT    NOT NULL DEFAULT '0',
    franking_credits                TEXT    NOT NULL DEFAULT '0',
    net_rent                        TEXT    NOT NULL DEFAULT '0',
    foreign_income                  TEXT    NOT NULL DEFAULT '0',
    foreign_tax_credits             TEXT    NOT NULL DEFAULT '0',
    other_income                    TEXT    NOT NULL DEFAULT '0',
    cgt_discount_gains              TEXT    NOT NULL DEFAULT '0',
    cgt_indexation_gains            TEXT    NOT NULL DEFAULT '0',
    cgt_other_gains                 TEXT    NOT NULL DEFAULT '0',
    capital_losses_applied          TEXT    NOT NULL DEFAULT '0',
    tax_deferred_amount             TEXT    NOT NULL DEFAULT '0',
    tax_free_amount                 TEXT    NOT NULL DEFAULT '0',
    cost_base_adjustment            TEXT    NOT NULL DEFAULT '0',
    tfn_withholding_tax             TEXT    NOT NULL DEFAULT '0',
    currency                        TEXT    NOT NULL DEFAULT 'AUD' REFERENCES currencies(code),
    -- The holding account the statement covers.
    holding_account_id              INTEGER NOT NULL DEFAULT 1 REFERENCES holding_accounts(id)
);

CREATE TABLE drp_enrolments (
    id                INTEGER PRIMARY KEY,
    listing_id        INTEGER NOT NULL REFERENCES listings(id),
    -- First day of the period (inclusive): distributions with an ex date (or
    -- pay date when no ex date is recorded) from this day reinvest.
    enrolment_date    TEXT NOT NULL,
    -- Day the unenrolment takes effect (exclusive): distributions with an ex
    -- date on or after it no longer reinvest. NULL = open-ended (currently
    -- enrolled). Periods for a (listing, holding account) must not overlap
    -- and at most one may be open at a time — a multi-row invariant enforced
    -- at write time inside a transaction (entities::drp_enrolment).
    unenrolment_date  TEXT,
    residual_handling TEXT NOT NULL DEFAULT 'CarryForward'
        CHECK(residual_handling IN ('CarryForward', 'PayOut')),
    -- The holding account the enrolment applies to.
    holding_account_id INTEGER NOT NULL DEFAULT 1 REFERENCES holding_accounts(id),
    CHECK (unenrolment_date IS NULL OR unenrolment_date > enrolment_date)
);

CREATE TABLE trades (
    id                  INTEGER PRIMARY KEY,
    trade_type          TEXT    NOT NULL CHECK(trade_type IN ('Buy', 'Sell', 'DRP')),
    date                TEXT    NOT NULL,
    settlement_date     TEXT    NOT NULL,
    listing_id          INTEGER NOT NULL REFERENCES listings(id),
    average_price       TEXT    NOT NULL,
    quantity            TEXT    NOT NULL,
    currency            TEXT    NOT NULL REFERENCES currencies(code),
    brokerage           TEXT    NOT NULL DEFAULT '0',
    gst_on_brokerage    TEXT    NOT NULL DEFAULT '0',
    brokerage_currency  TEXT    NOT NULL REFERENCES currencies(code),
    fx_rate             TEXT    NOT NULL DEFAULT '1',
    contract_note_ref   TEXT,
    residual_brought_forward TEXT NOT NULL DEFAULT '0',
    residual_carried_forward TEXT NOT NULL DEFAULT '0',
    residual_paid_out        TEXT NOT NULL DEFAULT '0',
    -- Provenance links from the rollover/exercise Buys and Sells back to the
    -- corporate action that produced them (NULL for ordinary trades).
    rights_action_id    INTEGER REFERENCES corporate_actions(id),
    buyback_action_id   INTEGER REFERENCES corporate_actions(id),
    scrip_action_id     INTEGER REFERENCES corporate_actions(id),
    demerger_action_id  INTEGER REFERENCES corporate_actions(id),
    -- Pre-CGT/rollover deemed acquisition date carried onto a rollover Buy.
    deemed_acquisition_date TEXT,
    holding_account_id  INTEGER NOT NULL DEFAULT 1 REFERENCES holding_accounts(id),
    -- Set on the paired Sell/Buys that implement an inter-account transfer.
    transfer_id         INTEGER REFERENCES transfers(id),
    -- 1 if the brokerage amount was *entered* GST-inclusive; the server splits
    -- it at write time so the stored brokerage/gst_on_brokerage keep their
    -- ex-GST semantics. Persisted only so a trade round-trips into the form.
    brokerage_includes_gst INTEGER NOT NULL DEFAULT 0
        CHECK (brokerage_includes_gst IN (0, 1)),
    -- The broker statement's net transaction total in the brokerage currency,
    -- validated at write time against quantity x price +/- (brokerage + GST).
    -- Informational/validation-only: no report or calculation uses it.
    statement_total     TEXT,
    ess_statement_id    INTEGER REFERENCES ess_statements(id),
    -- Provenance link from a worthless-shares closing Sell back to its
    -- WorthlessShares action: set only by POST /corporate_actions/:id/recognise
    -- (NULL for every other trade). The Sell carrying it is rejected by
    -- PUT /sells and PUT/DELETE /trades; DELETE /sells on it removes it and
    -- restores the holding, and the action is frozen while it exists. Unlike
    -- the rollover provenance columns, a Sell carrying it IS counted by the
    -- realised-gains report (its nil proceeds recognise the capital loss).
    worthless_action_id INTEGER REFERENCES corporate_actions(id)
);

CREATE TABLE income (
    id                          INTEGER PRIMARY KEY,
    listing_id                  INTEGER NOT NULL REFERENCES listings(id),
    date_paid                   TEXT    NOT NULL,
    ex_date                     TEXT,
    franked_amount              TEXT    NOT NULL DEFAULT '0',
    unfranked_amount            TEXT    NOT NULL DEFAULT '0',
    foreign_source_income       TEXT    NOT NULL DEFAULT '0',
    foreign_tax_paid            TEXT    NOT NULL DEFAULT '0',
    tfn_withholding_tax         TEXT    NOT NULL DEFAULT '0',
    franking_credits            TEXT    NOT NULL DEFAULT '0',
    lic_capital_gain_deduction  TEXT    NOT NULL DEFAULT '0',
    conduit_foreign_income      TEXT    NOT NULL DEFAULT '0',
    trust_income                INTEGER NOT NULL DEFAULT 0,
    reinvestment_trade_id       INTEGER REFERENCES trades(id),
    currency                    TEXT    NOT NULL DEFAULT 'AUD' REFERENCES currencies(code),
    buyback_trade_id            INTEGER REFERENCES trades(id),
    holding_account_id          INTEGER NOT NULL DEFAULT 1 REFERENCES holding_accounts(id),
    -- Optional per-share figures from the registry statement, for cross-checking
    -- a distribution against its payment advice: when supplied (always together),
    -- amount_per_security × securities_held cent-rounded must equal the gross
    -- cash components — validated at write time in entities::income.
    -- Informational/validation-only: no report or calculation reads them.
    amount_per_security         TEXT,
    securities_held             TEXT
);

CREATE TABLE parcel_allocations (
    id                INTEGER PRIMARY KEY,
    sale_trade_id     INTEGER NOT NULL REFERENCES trades(id),
    purchase_trade_id INTEGER NOT NULL REFERENCES trades(id),
    quantity_allocated TEXT    NOT NULL
);

CREATE TABLE amit_adjustments (
    id                 INTEGER PRIMARY KEY,
    amma_statement_id  INTEGER NOT NULL REFERENCES amma_statements(id),
    trade_id           INTEGER NOT NULL REFERENCES trades(id),
    quantity           TEXT    NOT NULL
);

CREATE TABLE attachments (
    id                INTEGER PRIMARY KEY,
    trade_id          INTEGER REFERENCES trades(id) ON DELETE CASCADE,
    income_id         INTEGER REFERENCES income(id) ON DELETE CASCADE,
    amma_statement_id INTEGER REFERENCES amma_statements(id) ON DELETE CASCADE,
    filename          TEXT    NOT NULL,
    content_type      TEXT    NOT NULL CHECK(content_type IN ('application/pdf', 'image/png', 'image/jpeg')),
    byte_size         INTEGER NOT NULL,
    checksum          TEXT    NOT NULL,
    uploaded_at       TEXT    NOT NULL,
    content           BLOB    NOT NULL,
    CHECK ((trade_id IS NOT NULL) + (income_id IS NOT NULL) + (amma_statement_id IS NOT NULL) = 1)
);

CREATE TABLE investment_expenses (
    id                    INTEGER PRIMARY KEY,
    -- Date the expense was incurred: its month drives the ATO FX rate used to
    -- convert a non-AUD amount to AUD, and the Australian financial year the
    -- deduction is attributed to (July–June; a July date belongs to the next FY).
    date_incurred         TEXT    NOT NULL,
    -- Expense category (CHECK-constrained enum). LoanInterest = interest on money
    -- borrowed to buy income-producing shares; ManagementFee = ongoing investment
    -- management fees; AdviceFee = financial-advice fees about an existing
    -- investment mix; AccountKeepingFee = investment-account fees; Subscription =
    -- specialist investment journals/subscriptions; Other = any other deductible
    -- investment expense.
    expense_type          TEXT    NOT NULL CHECK (expense_type IN (
                              'LoanInterest', 'ManagementFee', 'AdviceFee',
                              'AccountKeepingFee', 'Subscription', 'Other')),
    -- The deductible amount (post-apportionment) — the figure that goes on the
    -- return and the value the tax summary totals.
    amount                TEXT    NOT NULL DEFAULT '0',
    -- Optional provenance (informational only — no calculation reads these): the
    -- pre-apportionment gross expense and the percentage the user determined was
    -- deductible. Stored so a row's `amount` is auditable back to its source.
    gross_amount          TEXT,
    deductible_percentage TEXT,
    currency              TEXT    NOT NULL DEFAULT 'AUD' REFERENCES currencies(code),
    -- Free-text note (e.g. "margin loan interest Q3", "adviser annual fee").
    description           TEXT,
    -- Optional links tying the expense to the holding it relates to. Both NULL for
    -- a portfolio-wide expense (e.g. an adviser's whole-of-portfolio fee).
    listing_id            INTEGER REFERENCES listings(id),
    holding_account_id    INTEGER REFERENCES holding_accounts(id)
);

-- ---------------------------------------------------------------------------
-- Price snapshots
-- ---------------------------------------------------------------------------

CREATE TABLE closing_prices (
    listing_id INTEGER NOT NULL REFERENCES listings(id),
    -- The trading day the price closes: the date in the exchange's timezone,
    -- or for exchange-less (Crypto) listings the UTC date of the daily candle
    -- that completes at 00:00 UTC at the end of that date.
    price_date TEXT    NOT NULL,
    -- Decimal as TEXT, in the listing's quote currency (NOT AUD-converted —
    -- reports convert via the FX rules). NULL exactly when status = 'error'.
    price      TEXT,
    source     TEXT    NOT NULL,  -- provider that produced the row, e.g. 'yahoo'
    fetched_at TEXT    NOT NULL,  -- RFC 3339 UTC timestamp of the fetch
    status     TEXT    NOT NULL CHECK (status IN ('ok', 'error')),
    error      TEXT,              -- failure detail, NULL exactly when status = 'ok'
    PRIMARY KEY (listing_id, price_date),
    CHECK ((price IS NOT NULL) = (status = 'ok')),
    CHECK ((error IS NOT NULL) = (status = 'error'))
);

CREATE TABLE report_snapshots (
    report        TEXT NOT NULL CHECK (report IN
                      ('portfolio_overview', 'unrealised_gains', 'performance')),
    snapshot_date TEXT NOT NULL,
    -- RFC 3339 UTC timestamp of the run that produced this result.
    generated_at  TEXT NOT NULL,
    -- 1 = a back-dated fact was recorded after generation; the stored result
    -- no longer reflects the books and should be regenerated.
    stale         INTEGER NOT NULL DEFAULT 0 CHECK (stale IN (0, 1)),
    -- The report's response rows as JSON. Money fields inside are Decimal
    -- strings (the API's serialisation), kept in TEXT — never a REAL/float.
    rows_json     TEXT NOT NULL,
    PRIMARY KEY (report, snapshot_date)
);

-- ---------------------------------------------------------------------------
-- Snapshot-staleness triggers
--
-- A back-dated fact write must mark any snapshot on/after its date stale so the
-- price-dependent reports regenerate. Each fact table gets insert/update/delete
-- triggers keyed on the relevant date column; a new dated fact table needs a
-- matching set here (see CLAUDE.md / reports/snapshot.rs).
-- ---------------------------------------------------------------------------

CREATE TRIGGER amma_statements_stale_snapshots_insert AFTER INSERT ON amma_statements
BEGIN
    UPDATE report_snapshots SET stale = 1
    WHERE snapshot_date >= NEW.tax_year_end_date;
END;
CREATE TRIGGER amma_statements_stale_snapshots_update AFTER UPDATE ON amma_statements
BEGIN
    UPDATE report_snapshots SET stale = 1
    WHERE snapshot_date >= MIN(OLD.tax_year_end_date, NEW.tax_year_end_date);
END;
CREATE TRIGGER amma_statements_stale_snapshots_delete AFTER DELETE ON amma_statements
BEGIN
    UPDATE report_snapshots SET stale = 1
    WHERE snapshot_date >= OLD.tax_year_end_date;
END;

CREATE TRIGGER closing_prices_stale_snapshots_update AFTER UPDATE ON closing_prices
WHEN OLD.status = 'ok' AND (NEW.status <> 'ok' OR OLD.price <> NEW.price)
BEGIN
    UPDATE report_snapshots SET stale = 1 WHERE snapshot_date >= OLD.price_date;
END;

CREATE TRIGGER corporate_actions_stale_snapshots_insert AFTER INSERT ON corporate_actions
BEGIN
    UPDATE report_snapshots SET stale = 1 WHERE snapshot_date >= NEW.date;
END;
CREATE TRIGGER corporate_actions_stale_snapshots_update AFTER UPDATE ON corporate_actions
BEGIN
    UPDATE report_snapshots SET stale = 1
    WHERE snapshot_date >= MIN(OLD.date, NEW.date);
END;
CREATE TRIGGER corporate_actions_stale_snapshots_delete AFTER DELETE ON corporate_actions
BEGIN
    UPDATE report_snapshots SET stale = 1 WHERE snapshot_date >= OLD.date;
END;

CREATE TRIGGER trades_stale_snapshots_insert AFTER INSERT ON trades
BEGIN
    UPDATE report_snapshots SET stale = 1 WHERE snapshot_date >= NEW.date;
END;
CREATE TRIGGER trades_stale_snapshots_update AFTER UPDATE ON trades
BEGIN
    UPDATE report_snapshots SET stale = 1
    WHERE snapshot_date >= MIN(OLD.date, NEW.date);
END;
CREATE TRIGGER trades_stale_snapshots_delete AFTER DELETE ON trades
BEGIN
    UPDATE report_snapshots SET stale = 1 WHERE snapshot_date >= OLD.date;
END;

CREATE TRIGGER income_stale_snapshots_insert AFTER INSERT ON income
BEGIN
    UPDATE report_snapshots SET stale = 1 WHERE snapshot_date >= NEW.date_paid;
END;
CREATE TRIGGER income_stale_snapshots_update AFTER UPDATE ON income
BEGIN
    UPDATE report_snapshots SET stale = 1
    WHERE snapshot_date >= MIN(OLD.date_paid, NEW.date_paid);
END;
CREATE TRIGGER income_stale_snapshots_delete AFTER DELETE ON income
BEGIN
    UPDATE report_snapshots SET stale = 1 WHERE snapshot_date >= OLD.date_paid;
END;

CREATE TRIGGER parcel_allocations_stale_snapshots_insert AFTER INSERT ON parcel_allocations
BEGIN
    UPDATE report_snapshots SET stale = 1 WHERE snapshot_date >=
        (SELECT date FROM trades WHERE id = NEW.sale_trade_id);
END;
CREATE TRIGGER parcel_allocations_stale_snapshots_update AFTER UPDATE ON parcel_allocations
BEGIN
    UPDATE report_snapshots SET stale = 1 WHERE snapshot_date >=
        MIN((SELECT date FROM trades WHERE id = OLD.sale_trade_id),
            (SELECT date FROM trades WHERE id = NEW.sale_trade_id));
END;
CREATE TRIGGER parcel_allocations_stale_snapshots_delete AFTER DELETE ON parcel_allocations
BEGIN
    UPDATE report_snapshots SET stale = 1 WHERE snapshot_date >=
        (SELECT date FROM trades WHERE id = OLD.sale_trade_id);
END;

CREATE TRIGGER amit_adjustments_stale_snapshots_insert AFTER INSERT ON amit_adjustments
BEGIN
    UPDATE report_snapshots SET stale = 1 WHERE snapshot_date >=
        (SELECT tax_year_end_date FROM amma_statements WHERE id = NEW.amma_statement_id);
END;
CREATE TRIGGER amit_adjustments_stale_snapshots_update AFTER UPDATE ON amit_adjustments
BEGIN
    UPDATE report_snapshots SET stale = 1 WHERE snapshot_date >=
        MIN((SELECT tax_year_end_date FROM amma_statements WHERE id = OLD.amma_statement_id),
            (SELECT tax_year_end_date FROM amma_statements WHERE id = NEW.amma_statement_id));
END;
CREATE TRIGGER amit_adjustments_stale_snapshots_delete AFTER DELETE ON amit_adjustments
BEGIN
    UPDATE report_snapshots SET stale = 1 WHERE snapshot_date >=
        (SELECT tax_year_end_date FROM amma_statements WHERE id = OLD.amma_statement_id);
END;

-- ---------------------------------------------------------------------------
-- Seed data
--
-- A baseline of recognised currencies and operational exchanges, the seeded
-- exchange holiday calendars, and the default holding account every row uses out
-- of the box. The currency/MIC/FX imports later refine and extend the reference
-- data; this baseline is only enough for the app to run out of the box.
-- ---------------------------------------------------------------------------

INSERT INTO currencies (code, kind, numeric_code, name, short_name, minor_units, source) VALUES
    ('AUD', 'Fiat', '036', 'Australian Dollar',  NULL, 2, 'Iso4217'),
    ('USD', 'Fiat', '840', 'US Dollar',          NULL, 2, 'Iso4217'),
    ('GBP', 'Fiat', '826', 'Pound Sterling',      NULL, 2, 'Iso4217'),
    ('EUR', 'Fiat', '978', 'Euro',                NULL, 2, 'Iso4217'),
    ('JPY', 'Fiat', '392', 'Yen',                 NULL, 0, 'Iso4217'),
    ('NZD', 'Fiat', '554', 'New Zealand Dollar',  NULL, 2, 'Iso4217'),
    ('HKD', 'Fiat', '344', 'Hong Kong Dollar',    NULL, 2, 'Iso4217'),
    ('SGD', 'Fiat', '702', 'Singapore Dollar',    NULL, 2, 'Iso4217'),
    ('CAD', 'Fiat', '124', 'Canadian Dollar',     NULL, 2, 'Iso4217'),
    ('CHF', 'Fiat', '756', 'Swiss Franc',         NULL, 2, 'Iso4217'),
    ('CNY', 'Fiat', '156', 'Yuan Renminbi',       NULL, 2, 'Iso4217'),
    ('BTC', 'DigitalToken', NULL, 'Bitcoin', 'BTC',  8, 'Iso24165'),
    ('ETH', 'DigitalToken', NULL, 'Ether',   'ETH', 18, 'Iso24165');

INSERT INTO exchanges (mic, name, country, currency, timezone, settlement_days) VALUES
    ('XASX', 'Australian Securities Exchange', 'Australia',     'AUD', 'Australia/Sydney',  2),
    ('XNYS', 'New York Stock Exchange',        'United States', 'USD', 'America/New_York',  2);

-- ASX (XASX) — NSW public holidays observed by the ASX cash market.
INSERT INTO exchange_holidays (mic, holiday_date, name) VALUES
    ('XASX', '2024-01-01', 'New Year''s Day'),
    ('XASX', '2024-01-26', 'Australia Day'),
    ('XASX', '2024-03-29', 'Good Friday'),
    ('XASX', '2024-04-01', 'Easter Monday'),
    ('XASX', '2024-04-25', 'Anzac Day'),
    ('XASX', '2024-06-10', 'King''s Birthday'),
    ('XASX', '2024-12-25', 'Christmas Day'),
    ('XASX', '2024-12-26', 'Boxing Day'),
    ('XASX', '2025-01-01', 'New Year''s Day'),
    ('XASX', '2025-01-27', 'Australia Day'),               -- 26 Jan is a Sunday; observed Mon 27
    ('XASX', '2025-04-18', 'Good Friday'),
    ('XASX', '2025-04-21', 'Easter Monday'),
    ('XASX', '2025-04-25', 'Anzac Day'),
    ('XASX', '2025-06-09', 'King''s Birthday'),
    ('XASX', '2025-12-25', 'Christmas Day'),
    ('XASX', '2025-12-26', 'Boxing Day'),
    ('XASX', '2026-01-01', 'New Year''s Day'),
    ('XASX', '2026-01-26', 'Australia Day'),
    ('XASX', '2026-04-03', 'Good Friday'),
    ('XASX', '2026-04-06', 'Easter Monday'),
    ('XASX', '2026-04-25', 'Anzac Day'),                   -- a Saturday; named on its actual date
    ('XASX', '2026-06-08', 'King''s Birthday'),
    ('XASX', '2026-12-25', 'Christmas Day'),
    ('XASX', '2026-12-28', 'Boxing Day'),                  -- 26 Dec is a Saturday; observed Mon 28
    ('XASX', '2027-01-01', 'New Year''s Day'),
    ('XASX', '2027-01-26', 'Australia Day'),
    ('XASX', '2027-03-26', 'Good Friday'),
    ('XASX', '2027-03-29', 'Easter Monday'),
    ('XASX', '2027-04-25', 'Anzac Day'),                   -- a Sunday; named on its actual date
    ('XASX', '2027-06-14', 'King''s Birthday'),
    ('XASX', '2027-12-27', 'Christmas Day'),               -- 25 Dec is a Saturday; observed Mon 27
    ('XASX', '2027-12-28', 'Boxing Day');                  -- 26 Dec is a Sunday; observed Tue 28

-- NYSE (XNYS) — full market closures (early-close days excluded). A holiday on a
-- Saturday is not observed (no weekday closure); a Sunday holiday is observed the
-- following Monday.
INSERT INTO exchange_holidays (mic, holiday_date, name) VALUES
    ('XNYS', '2024-01-01', 'New Year''s Day'),
    ('XNYS', '2024-01-15', 'Martin Luther King, Jr. Day'),
    ('XNYS', '2024-02-19', 'Washington''s Birthday'),
    ('XNYS', '2024-03-29', 'Good Friday'),
    ('XNYS', '2024-05-27', 'Memorial Day'),
    ('XNYS', '2024-06-19', 'Juneteenth National Independence Day'),
    ('XNYS', '2024-07-04', 'Independence Day'),
    ('XNYS', '2024-09-02', 'Labor Day'),
    ('XNYS', '2024-11-28', 'Thanksgiving Day'),
    ('XNYS', '2024-12-25', 'Christmas Day'),
    ('XNYS', '2025-01-01', 'New Year''s Day'),
    ('XNYS', '2025-01-09', 'National Day of Mourning'),    -- President Jimmy Carter
    ('XNYS', '2025-01-20', 'Martin Luther King, Jr. Day'),
    ('XNYS', '2025-02-17', 'Washington''s Birthday'),
    ('XNYS', '2025-04-18', 'Good Friday'),
    ('XNYS', '2025-05-26', 'Memorial Day'),
    ('XNYS', '2025-06-19', 'Juneteenth National Independence Day'),
    ('XNYS', '2025-07-04', 'Independence Day'),
    ('XNYS', '2025-09-01', 'Labor Day'),
    ('XNYS', '2025-11-27', 'Thanksgiving Day'),
    ('XNYS', '2025-12-25', 'Christmas Day'),
    ('XNYS', '2026-01-01', 'New Year''s Day'),
    ('XNYS', '2026-01-19', 'Martin Luther King, Jr. Day'),
    ('XNYS', '2026-02-16', 'Washington''s Birthday'),
    ('XNYS', '2026-04-03', 'Good Friday'),
    ('XNYS', '2026-05-25', 'Memorial Day'),
    ('XNYS', '2026-06-19', 'Juneteenth National Independence Day'),
    ('XNYS', '2026-09-07', 'Labor Day'),                   -- 4 Jul is a Saturday; not observed
    ('XNYS', '2026-11-26', 'Thanksgiving Day'),
    ('XNYS', '2026-12-25', 'Christmas Day'),
    ('XNYS', '2027-01-01', 'New Year''s Day'),
    ('XNYS', '2027-01-18', 'Martin Luther King, Jr. Day'),
    ('XNYS', '2027-02-15', 'Washington''s Birthday'),
    ('XNYS', '2027-03-26', 'Good Friday'),
    ('XNYS', '2027-05-31', 'Memorial Day'),
    ('XNYS', '2027-06-18', 'Juneteenth National Independence Day'),  -- 19 Jun is a Saturday; observed Fri 18
    ('XNYS', '2027-07-05', 'Independence Day'),            -- 4 Jul is a Sunday; observed Mon 5
    ('XNYS', '2027-09-06', 'Labor Day'),
    ('XNYS', '2027-11-25', 'Thanksgiving Day'),
    ('XNYS', '2027-12-24', 'Christmas Day');               -- 25 Dec is a Saturday; observed Fri 24

-- The default holding account every row migrates to / uses out of the box.
INSERT INTO holding_accounts (id, name) VALUES (1, 'Default');
