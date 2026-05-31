CREATE TABLE parcel_allocations (
    id                INTEGER PRIMARY KEY,
    sale_trade_id     INTEGER NOT NULL REFERENCES trades(id),
    purchase_trade_id INTEGER NOT NULL REFERENCES trades(id),
    quantity_allocated TEXT    NOT NULL
);
