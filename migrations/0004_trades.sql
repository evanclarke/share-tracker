CREATE TABLE trades (
    id                  INTEGER PRIMARY KEY,
    trade_type          TEXT    NOT NULL CHECK(trade_type IN ('Buy', 'Sell', 'DRP')),
    date                TEXT    NOT NULL,
    settlement_date     TEXT    NOT NULL,
    listing_id          INTEGER NOT NULL REFERENCES listings(id),
    average_price       REAL    NOT NULL,
    quantity            REAL    NOT NULL,
    currency            TEXT    NOT NULL,
    brokerage           REAL    NOT NULL DEFAULT 0,
    gst_on_brokerage    REAL    NOT NULL DEFAULT 0,
    brokerage_currency  TEXT    NOT NULL,
    fx_rate             REAL    NOT NULL DEFAULT 1,
    contract_note_ref   TEXT
);
