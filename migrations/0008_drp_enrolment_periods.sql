-- DRP enrolment becomes dated periods per listing (REQUIREMENTS "DRP enrolment
-- and unenrolment over time"): a holding can start unenrolled, enrol, unenrol,
-- and re-enrol, each period choosing its own residual handling. The old model
-- (one row per listing, presence = enrolled) cannot represent that history.
--
-- Existing rows migrate to an open-ended period starting 0001-01-01 (enrolled
-- "since forever"), preserving the old behaviour where every distribution on an
-- enrolled holding was reinvestable. Rebuilt via the rename pattern — no data
-- dropped; DROP TABLE only touches the renamed _old table.
ALTER TABLE drp_enrolments RENAME TO drp_enrolments_old;

CREATE TABLE drp_enrolments (
    id                INTEGER PRIMARY KEY,
    listing_id        INTEGER NOT NULL REFERENCES listings(id),
    -- First day of the period (inclusive): distributions with an ex date (or
    -- pay date when no ex date is recorded) from this day reinvest.
    enrolment_date    TEXT NOT NULL,
    -- Day the unenrolment takes effect (exclusive): distributions with an ex
    -- date on or after it no longer reinvest. NULL = open-ended (currently
    -- enrolled). Periods for a listing must not overlap and at most one may be
    -- open at a time — a multi-row invariant enforced at write time inside a
    -- transaction (entities::drp_enrolment), like the Sell-allocation invariant.
    unenrolment_date  TEXT,
    residual_handling TEXT NOT NULL DEFAULT 'CarryForward'
        CHECK(residual_handling IN ('CarryForward', 'PayOut')),
    CHECK (unenrolment_date IS NULL OR unenrolment_date > enrolment_date)
);

INSERT INTO drp_enrolments (listing_id, enrolment_date, unenrolment_date, residual_handling)
SELECT listing_id, '0001-01-01', NULL, residual_handling FROM drp_enrolments_old;

DROP TABLE drp_enrolments_old;
