-- Deductible investment expenses (REQUIREMENTS "Deductible investment expenses",
-- docs/ato/investment-income-deductions.md + dividend-income-deductions.md): the
-- cost of earning assessable investment income — chiefly interest on money
-- borrowed to buy income-producing shares, plus management/adviser fees,
-- account-keeping fees, and subscriptions. The tax summary nets these against
-- gross assessable investment income per Australian financial year, so the gross
-- assessable totals no longer overstate the net position.
--
-- One row is one expense, incurred on `date_incurred` (its month sets the ATO FX
-- conversion month and the financial year it falls in). `amount` is the
-- **deductible amount** — post-apportionment, the figure that goes on the return:
-- the ATO's apportionment rules (joint accounts, private vs income-producing use,
-- partial seminar/travel/computer use) are the user's determination, not computed
-- here. `gross_amount` and `deductible_percentage` are optional provenance only
-- (informational — no calculation reads them): the pre-apportionment figure and
-- the percentage the user determined deductible. All money is TEXT decimal (never
-- REAL); the currency FK validates the denomination.
--
-- Brokerage and other transaction costs are deliberately NOT expense rows — they
-- form the CGT cost base on the trade — and the LIC capital gain deduction is its
-- own income field (`income.lic_capital_gain_deduction`).
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
