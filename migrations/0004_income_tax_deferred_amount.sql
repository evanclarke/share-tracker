-- A non-AMIT unit trust's annual statement reports a "tax-deferred amount":
-- a non-assessable payment that is a CGT event E4 cost-base reduction
-- (docs/ato/cgt-non-assessable-payments.md). The reduction itself is modelled
-- as a ReturnOfCapital corporate action — this column only records the
-- statement figure so the E4 cross-check report can flag a trust income row
-- whose tax-deferred amount has no matching same-FY ReturnOfCapital action.
-- Informational: no calculation reads it. Trust rows only, never negative
-- (CHECK; non-trust writes also rejected 422 with a fuller message).
ALTER TABLE income ADD COLUMN tax_deferred_amount TEXT
    CHECK (tax_deferred_amount IS NULL
           OR (trust_income = 1 AND CAST(tax_deferred_amount AS NUMERIC) >= 0));
