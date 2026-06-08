-- Employee share scheme (ESS) income (REQUIREMENTS "Employee share scheme (ESS)
-- income", docs/ato/employee-share-schemes.md): the assessable discount on ESS
-- interests, declared at Item 12 in the year of the taxing point. The CGT side
-- (a cost-base-reset Buy at the taxing-point market value) was already
-- representable; this adds the income side and ties the two together.
--
-- One row captures one Employee share scheme statement, attributed to a
-- (listing, holding account): the per-type discount labels, the foreign-source
-- memo, the TFN withheld, the taxing-point date, and the per-share market value
-- and quantity that vest (driving the cost-base-reset Buy the vesting operation
-- creates). All money/quantity columns are TEXT decimals (never REAL); the
-- currency FK validates the denomination. The discount-test ($1,000 reduction,
-- ≤$180,000 income test) is applied in the tax summary, not constrained here.
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

-- Provenance link from the cost-base-reset Buy back to its ESS statement: set
-- only by POST /ess_statements/:id/vest (NULL for every other trade). A trade
-- carrying it is the statement's vest: it is rejected by PUT /trades (the
-- figures derive from the statement — delete and re-vest) and is never deleted
-- individually (DELETE /ess_statements/:id removes it, refused while it is drawn
-- on by a Sell allocation or AMIT adjustment). The statement is frozen against
-- edits while its vest exists. Plain ADD COLUMN — existing rows get NULL, no
-- data dropped (the referenced table is created above).
ALTER TABLE trades ADD COLUMN ess_statement_id INTEGER REFERENCES ess_statements(id);
