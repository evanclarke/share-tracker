-- DRP (Dividend Reinvestment Plan) enrolments.
-- At most one enrolment per holding (listing_id is the primary key); its
-- presence means the holding reinvests its distributions in full. Partial
-- participation is out of scope. residual_handling decides what happens to
-- leftover cash that doesn't buy a whole share. See REQUIREMENTS.md > DRP.
CREATE TABLE drp_enrolments (
    listing_id        INTEGER PRIMARY KEY REFERENCES listings(id),
    residual_handling TEXT NOT NULL DEFAULT 'CarryForward'
        CHECK(residual_handling IN ('CarryForward', 'PayOut'))
);
