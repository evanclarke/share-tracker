-- Inherited share parcels (REQUIREMENTS 2026-06-10;
-- docs/ato/inherited-assets-cost-base.md QC 66053,
-- docs/ato/inherited-assets-cgt-discount.md QC 69713 / s 115-30).
--
-- An inheritance records the facts of a parcel passing to the beneficiary
-- from a deceased estate — not a CGT event. Its entry path
-- (entities::inheritance) creates one provenance-linked Buy carrying the
-- cost base (on the brokerage column, price 0 — the rollover convention) and
-- the s 115-30 discount clock (trades.deemed_acquisition_date), so the
-- parcel flows through every report and write-time capacity check like any
-- Buy. Like `transfers`, this table is provenance only — no report reads it
-- directly, and every write also writes the linked Buy in the same
-- transaction (firing the trades snapshot-staleness triggers), so it needs
-- no trigger set of its own.
CREATE TABLE inheritances (
    id                        INTEGER PRIMARY KEY,
    listing_id                INTEGER NOT NULL REFERENCES listings(id),
    holding_account_id        INTEGER NOT NULL DEFAULT 1 REFERENCES holding_accounts(id),
    -- Units inherited, in date-of-death terms (validated > 0 in Rust).
    quantity                  TEXT    NOT NULL,
    date_of_death             TEXT    NOT NULL,
    -- Which QC 66053 rule produced the first-element figure (CHECK-constrained
    -- enum): DeceasedCostBase = the deceased acquired the asset on or after
    -- 20 September 1985, so their cost base on the day they died carries
    -- over; MarketValueAtDeath = a pre-CGT asset in the deceased's hands, so
    -- the first element is the asset's market value on the day they died
    -- (the user supplies the valuation figure).
    cost_base_rule            TEXT    NOT NULL
        CHECK (cost_base_rule IN ('DeceasedCostBase', 'MarketValueAtDeath')),
    -- The whole-parcel first-element cost base per that rule, in `currency`.
    cost_base                 TEXT    NOT NULL,
    -- Expenditure of the legal personal representative the beneficiary may
    -- include in the cost base (QC 66053 — e.g. conveyancing on the
    -- transfer, legal costs of proving the will), dated when the LPR
    -- incurred it (on or after the date of death; validated in Rust). Added
    -- to the linked Buy's cost base; the date is provenance (indexation,
    -- where it would matter, is not modelled).
    lpr_expenditure           TEXT    NOT NULL DEFAULT '0',
    lpr_expenditure_date      TEXT,
    -- The deceased's acquisition date — recorded exactly when the
    -- DeceasedCostBase rule applies (it starts the 12-month discount clock
    -- per s 115-30, carried as the Buy's deemed_acquisition_date); a
    -- pre-CGT asset's clock runs from the date of death instead, so the
    -- date is not recorded.
    deceased_acquisition_date TEXT,
    currency                  TEXT    NOT NULL DEFAULT 'AUD' REFERENCES currencies(code),
    -- Manual foreign-per-AUD fallback rate (same convention as
    -- trades.fx_rate; reports prefer the ATO/RBA rate for the acquisition
    -- month).
    fx_rate                   TEXT    NOT NULL DEFAULT '1',
    CHECK ((deceased_acquisition_date IS NOT NULL) = (cost_base_rule = 'DeceasedCostBase')),
    CHECK (deceased_acquisition_date IS NULL OR deceased_acquisition_date <= date_of_death)
);

-- Provenance link from the inherited-parcel Buy back to its inheritance
-- (NULL for every other trade): set only by PUT /inheritances/:id. The Buy
-- carrying it is rejected by PUT/DELETE /trades — it is edited and deleted
-- through its inheritance instead.
ALTER TABLE trades ADD COLUMN inheritance_id INTEGER REFERENCES inheritances(id);
