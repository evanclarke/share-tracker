-- The cash-in-lieu disposal of a rounded corporate action's fraction (TODO
-- "Unit granularity and registry rounding of fractional entitlements",
-- *Compute per holding, store per parcel*).
--
-- A `CashInLieu` action rounds each holding down onto its grid and carries the
-- fraction — the exact count less the rounded-down count — on one parcel
-- (`corporate_action_parcel_counts.in_lieu_units`, 0055) until the registry
-- pays the holding for it. `POST /corporate_actions/:id/cash_in_lieu` records
-- that payment as the disposal it is: a Sell of the fraction, dated the
-- payment date, at the cash the holding was paid, stamped with this column.
--
-- `trades.cash_in_lieu_action_id` is the Sell's provenance link back to its
-- action, the shape `worthless_action_id` has: set only by the operation
-- (NULL for every other trade, and only ever on a Sell — the CHECK), the Sell
-- carrying it is refused by `PUT /sells` and `PUT`/`DELETE /trades`,
-- `DELETE /sells` on it is the operation's undo, and the action is frozen
-- against edits and deletes while it exists. Like a worthless-shares
-- recognise and unlike a rollover's closing Sell, it IS counted by the
-- realised-gains report — the cash is capital proceeds on the fraction.
--
-- `trades` is audited, so its row-history trigger pair (the live pair from
-- 0045) is dropped and re-created with the new column in its JSON. The
-- staleness triggers list no columns and are unchanged. No backfill: every
-- existing row is NULL.

DROP TRIGGER trades_row_history_update;
DROP TRIGGER trades_row_history_delete;

ALTER TABLE trades ADD COLUMN cash_in_lieu_action_id INTEGER REFERENCES corporate_actions(id)
    CHECK (cash_in_lieu_action_id IS NULL OR trade_type = 'Sell');

CREATE INDEX trades_cash_in_lieu_action_id ON trades (cash_in_lieu_action_id);

CREATE TRIGGER trades_row_history_update AFTER UPDATE ON trades
BEGIN
    INSERT INTO row_history (table_name, row_id, operation, changed_at, old_row)
    VALUES ('trades', OLD.id, 'UPDATE', strftime('%Y-%m-%dT%H:%M:%fZ', 'now'),
            json_object(
                        'id', OLD.id, 'trade_type', OLD.trade_type, 'date', OLD.date,
                        'settlement_date', OLD.settlement_date, 'listing_id', OLD.listing_id,
                        'average_price', OLD.average_price, 'quantity', OLD.quantity,
                        'currency', OLD.currency, 'brokerage', OLD.brokerage,
                        'gst_on_brokerage', OLD.gst_on_brokerage, 'brokerage_currency',
                        OLD.brokerage_currency, 'fx_rate', OLD.fx_rate, 'contract_note_ref',
                        OLD.contract_note_ref, 'residual_brought_forward',
                        OLD.residual_brought_forward, 'residual_carried_forward',
                        OLD.residual_carried_forward, 'residual_paid_out',
                        OLD.residual_paid_out, 'rights_action_id', OLD.rights_action_id,
                        'buyback_action_id', OLD.buyback_action_id, 'scrip_action_id',
                        OLD.scrip_action_id, 'demerger_action_id', OLD.demerger_action_id,
                        'deemed_acquisition_date', OLD.deemed_acquisition_date,
                        'holding_account_id', OLD.holding_account_id, 'transfer_id',
                        OLD.transfer_id, 'brokerage_includes_gst', OLD.brokerage_includes_gst,
                        'statement_total', OLD.statement_total, 'ess_statement_id',
                        OLD.ess_statement_id, 'worthless_action_id', OLD.worthless_action_id,
                        'inheritance_id', OLD.inheritance_id, 'spot_fx_rate', OLD.spot_fx_rate,
                        'settlement_date_source', OLD.settlement_date_source,
                        'cash_in_lieu_action_id', OLD.cash_in_lieu_action_id));
END;

CREATE TRIGGER trades_row_history_delete AFTER DELETE ON trades
BEGIN
    INSERT INTO row_history (table_name, row_id, operation, changed_at, old_row)
    VALUES ('trades', OLD.id, 'DELETE', strftime('%Y-%m-%dT%H:%M:%fZ', 'now'),
            json_object(
                        'id', OLD.id, 'trade_type', OLD.trade_type, 'date', OLD.date,
                        'settlement_date', OLD.settlement_date, 'listing_id', OLD.listing_id,
                        'average_price', OLD.average_price, 'quantity', OLD.quantity,
                        'currency', OLD.currency, 'brokerage', OLD.brokerage,
                        'gst_on_brokerage', OLD.gst_on_brokerage, 'brokerage_currency',
                        OLD.brokerage_currency, 'fx_rate', OLD.fx_rate, 'contract_note_ref',
                        OLD.contract_note_ref, 'residual_brought_forward',
                        OLD.residual_brought_forward, 'residual_carried_forward',
                        OLD.residual_carried_forward, 'residual_paid_out',
                        OLD.residual_paid_out, 'rights_action_id', OLD.rights_action_id,
                        'buyback_action_id', OLD.buyback_action_id, 'scrip_action_id',
                        OLD.scrip_action_id, 'demerger_action_id', OLD.demerger_action_id,
                        'deemed_acquisition_date', OLD.deemed_acquisition_date,
                        'holding_account_id', OLD.holding_account_id, 'transfer_id',
                        OLD.transfer_id, 'brokerage_includes_gst', OLD.brokerage_includes_gst,
                        'statement_total', OLD.statement_total, 'ess_statement_id',
                        OLD.ess_statement_id, 'worthless_action_id', OLD.worthless_action_id,
                        'inheritance_id', OLD.inheritance_id, 'spot_fx_rate', OLD.spot_fx_rate,
                        'settlement_date_source', OLD.settlement_date_source,
                        'cash_in_lieu_action_id', OLD.cash_in_lieu_action_id));
END;
