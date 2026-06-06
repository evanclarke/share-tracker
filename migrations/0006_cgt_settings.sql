-- Singleton CGT settings row (id is CHECKed to 1 so at most one row can exist).
-- `opening_capital_loss` is the net capital loss carried forward from years
-- before the first year recorded in the system (per docs/cgt-using-capital-losses.md
-- net capital losses carry forward indefinitely). It is an entered, recognised
-- data-model value — not derived — and seeds the brought-forward loss balance the
-- net-capital-gain report chains across its year series. Stored as TEXT to
-- preserve arbitrary Decimal precision, like every monetary column.
CREATE TABLE cgt_settings (
    id                   INTEGER PRIMARY KEY CHECK (id = 1),
    opening_capital_loss TEXT NOT NULL DEFAULT '0'
);
