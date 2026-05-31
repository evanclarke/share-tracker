-- DRP reinvestment residual cash, tracked on DRP trades only (0 for Buy/Sell).
-- When a distribution doesn't divide evenly into whole shares, the leftover is
-- either carried forward to the next reinvestment for the holding or paid out,
-- per the holding's drp_enrolments.residual_handling. See REQUIREMENTS.md > DRP.
-- Stored as TEXT for arbitrary-precision decimals, like the other money columns.
ALTER TABLE trades ADD COLUMN residual_brought_forward TEXT NOT NULL DEFAULT '0';
ALTER TABLE trades ADD COLUMN residual_carried_forward TEXT NOT NULL DEFAULT '0';
ALTER TABLE trades ADD COLUMN residual_paid_out        TEXT NOT NULL DEFAULT '0';
