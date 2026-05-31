CREATE TABLE income (
    id                        INTEGER PRIMARY KEY,
    listing_id                INTEGER NOT NULL REFERENCES listings(id),
    date_paid                 TEXT    NOT NULL,
    ex_date                   TEXT,
    franked_amount            REAL    NOT NULL DEFAULT 0,
    unfranked_amount          REAL    NOT NULL DEFAULT 0,
    foreign_source_income     REAL    NOT NULL DEFAULT 0,
    foreign_tax_paid          REAL    NOT NULL DEFAULT 0,
    tfn_withholding_tax       REAL    NOT NULL DEFAULT 0,
    franking_credits          REAL    NOT NULL DEFAULT 0,
    lic_capital_gain_deduction REAL   NOT NULL DEFAULT 0,
    conduit_foreign_income    REAL    NOT NULL DEFAULT 0,
    trust_income              INTEGER NOT NULL DEFAULT 0,
    reinvestment_trade_id     INTEGER REFERENCES trades(id)
);
