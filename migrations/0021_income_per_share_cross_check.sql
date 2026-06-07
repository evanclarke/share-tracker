-- Optional per-share figures from the registry statement, recorded for
-- cross-checking a distribution against its payment advice: when supplied
-- (always together), amount_per_security × securities_held cent-rounded must
-- equal the gross cash components (franked + unfranked + foreign source
-- income) — validated at write time in entities::income. Informational /
-- validation-only: no report or calculation reads them (mirrors
-- trades.statement_total). Plain ADD COLUMN — existing rows get NULL,
-- no data dropped.
ALTER TABLE income ADD COLUMN amount_per_security TEXT;
ALTER TABLE income ADD COLUMN securities_held TEXT;
