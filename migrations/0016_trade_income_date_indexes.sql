-- `trades.date` and `income.date_paid` are queried with `<= as_of` by every
-- as-of report (`reports::performance::db_performance` in particular, run
-- once per date by snapshot generation) with no supporting index, so every
-- call does a full table scan. Purely additive — no data change.

CREATE INDEX trades_date ON trades (date);
CREATE INDEX income_date_paid ON income (date_paid);
