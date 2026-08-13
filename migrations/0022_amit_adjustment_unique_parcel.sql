-- One AMIT adjustment per (AMMA statement, parcel).
--
-- An AMMA statement's per-unit `cost_base_adjustment` is applied parcel by
-- parcel through `amit_adjustments`, and `db_cost_base_reductions` sums
-- `quantity × cost_base_adjustment` over every linked row. Two rows naming
-- the same parcel on the same statement therefore reduce that parcel's cost
-- base twice — and because CGT event E10 floors the reduced cost base at nil
-- and treats the excess as a capital gain, an over-reduction does not merely
-- understate the cost base: it can manufacture a gain that was never made.
-- Each row was already validated in isolation (Buy/DRP, matching listing and
-- holding account, quantity within the parcel), but nothing stopped the same
-- parcel appearing twice.
--
-- Purely additive: an index, no table rebuild, no column change, so the
-- table's `row_history` triggers are untouched. Both the repo copy of the
-- database and the deployed one (bigbrain.lan, 149 rows) were checked for
-- existing duplicate pairs on 2026-08-13 and had none, so the index applies
-- to live data without a migration step.

CREATE UNIQUE INDEX amit_adjustments_statement_trade
    ON amit_adjustments (amma_statement_id, trade_id);
