-- Corporate actions recorded against a listing. One action type is modelled so
-- far: 'ReturnOfCapital' — a non-assessable payment from a company (a
-- shareholder-approved return of share capital, CGT event G1; see
-- docs/cgt-non-assessable-payments.md). The per-unit payment reduces the cost
-- base of every parcel of the listing held on the payment date; where cumulative
-- payments exceed a parcel's per-unit cost base, the cost base floors at nil and
-- the excess is an immediate capital gain (G1 can never produce a capital loss).
-- The action_type CHECK is the extension point for future corporate actions
-- (splits, bonus shares, rights issues, ...), each widening the enum.
CREATE TABLE corporate_actions (
    id              INTEGER PRIMARY KEY,
    action_type     TEXT    NOT NULL CHECK (action_type IN ('ReturnOfCapital')),
    listing_id      INTEGER NOT NULL REFERENCES listings(id),
    -- Payment date: parcels acquired on/before this date are affected.
    date            TEXT    NOT NULL,
    -- Per-unit payment amount in `currency` (TEXT decimal, must be positive).
    amount_per_unit TEXT    NOT NULL,
    currency        TEXT    NOT NULL REFERENCES currencies(code)
);
