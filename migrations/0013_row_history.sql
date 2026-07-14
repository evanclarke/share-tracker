-- Append-only audit trail for financial writes (2026-07-13 improvement
-- review; aligns with the ATO record-keeping guidance mirrored in
-- docs/ato/cgt-keeping-records-shares.md).
--
-- Every entity is PUT-upsert-in-place and hard DELETE, so before this
-- migration an accidental edit to a historical row silently changed
-- prior-year cost bases and tax figures with no way to notice it happened.
-- Now every UPDATE or DELETE on an audited table records the prior row (as
-- JSON), the operation, and a UTC timestamp in `row_history` via AFTER
-- UPDATE / AFTER DELETE triggers — enforced in the database per the
-- data-integrity convention, so no write path can bypass it, and a write
-- rejected inside a transaction rolls its history row back with it (no
-- phantom audit entries). INSERTs are not recorded: until a row is first
-- changed, the live row is its own record. A value-identical UPDATE still
-- logs a row — it is a write event, and the audit trail records writes, not
-- diffs. Cascade deletes (a deleted trade's attachments, a deleted rights
-- sale's allocations) fire AFTER DELETE triggers too, so cascade-deleted
-- children are recorded like directly deleted rows.
--
-- Scope (decision 2026-07-14): audited = every user-entered table whose
-- values feed a calculation or report — the financial fact tables (trades,
-- parcel_allocations, income, interest_income, amma_statements,
-- amit_adjustments, ess_statements, transfers, corporate_actions,
-- inheritances, rights_sales, rights_sale_allocations, investment_expenses,
-- drp_enrolments, attachments) plus cgt_settings (the opening capital loss
-- retroactively changes every year's net capital gain) and, of the
-- reference-data tables, listings alone: its amit / security_type /
-- preference flags retroactively change tax calculations. The other
-- reference-data tables are out of scope: import-managed and re-importable
-- (currencies, mic_registry, rba_fx_rates, closing_prices), or they only
-- influence values computed and persisted onto trades at write time, where
-- the trade row itself is audited (exchanges, exchange_holidays), or they
-- are identity-only (holding_accounts) or derived state (report_snapshots,
-- job_runs).
--
-- Retention (decision 2026-07-14): kept forever — it is the audit trail.
-- `row_history` is itself append-only, enforced by its own BEFORE UPDATE /
-- BEFORE DELETE RAISE(ABORT) triggers, and deliberately has no pruning job.
--
-- attachments.content (a BLOB) is the one audited column excluded from the
-- JSON old row — json_object() cannot hold a BLOB — so an attachment's
-- history records filename, byte_size, and checksum, which still identify
-- exactly which file the row held.
--
-- Maintenance: ALTER TABLE ADD COLUMN does not update triggers. A migration
-- adding a column to an audited table must DROP and re-CREATE that table's
-- two *_row_history_* triggers with the new column in the json_object list,
-- and a new financial fact table needs its own trigger pair plus an entry in
-- the table_name CHECK below and in reports::row_history::AUDITED_TABLES
-- (a test pins the three lists to each other). See CLAUDE.md.

CREATE TABLE row_history (
    id         INTEGER PRIMARY KEY AUTOINCREMENT,
    table_name TEXT    NOT NULL CHECK (table_name IN (
                   'trades', 'parcel_allocations', 'income', 'interest_income',
                   'amma_statements', 'amit_adjustments', 'ess_statements',
                   'transfers', 'corporate_actions', 'inheritances',
                   'rights_sales', 'rights_sale_allocations',
                   'investment_expenses', 'drp_enrolments', 'cgt_settings',
                   'attachments', 'listings')),
    row_id     INTEGER NOT NULL,               -- the audited row's `id`
    operation  TEXT    NOT NULL CHECK (operation IN ('UPDATE', 'DELETE')),
    changed_at TEXT    NOT NULL,               -- RFC 3339 UTC, millisecond precision
    old_row    TEXT    NOT NULL                -- the prior row as a JSON object;
                                               -- TEXT decimals stay JSON strings (exact)
);

-- The history endpoint's lookup: one (table, row)'s entries in write order.
CREATE INDEX row_history_row ON row_history (table_name, row_id);

-- The audit trail is append-only: nothing — no handler, no job, no future
-- migration shortcut — may rewrite or erase it.
CREATE TRIGGER row_history_append_only_update BEFORE UPDATE ON row_history
BEGIN
    SELECT RAISE(ABORT, 'row_history is append-only');
END;
CREATE TRIGGER row_history_append_only_delete BEFORE DELETE ON row_history
BEGIN
    SELECT RAISE(ABORT, 'row_history is append-only');
END;

-- ---------------------------------------------------------------------------
-- Per-table audit triggers (generated from the live schema's column lists).
-- ---------------------------------------------------------------------------

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
                        'inheritance_id', OLD.inheritance_id, 'spot_fx_rate', OLD.spot_fx_rate));
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
                        'inheritance_id', OLD.inheritance_id, 'spot_fx_rate', OLD.spot_fx_rate));
END;

CREATE TRIGGER parcel_allocations_row_history_update AFTER UPDATE ON parcel_allocations
BEGIN
    INSERT INTO row_history (table_name, row_id, operation, changed_at, old_row)
    VALUES ('parcel_allocations', OLD.id, 'UPDATE', strftime('%Y-%m-%dT%H:%M:%fZ', 'now'),
            json_object(
                        'id', OLD.id, 'sale_trade_id', OLD.sale_trade_id, 'purchase_trade_id',
                        OLD.purchase_trade_id, 'quantity_allocated', OLD.quantity_allocated));
END;

CREATE TRIGGER parcel_allocations_row_history_delete AFTER DELETE ON parcel_allocations
BEGIN
    INSERT INTO row_history (table_name, row_id, operation, changed_at, old_row)
    VALUES ('parcel_allocations', OLD.id, 'DELETE', strftime('%Y-%m-%dT%H:%M:%fZ', 'now'),
            json_object(
                        'id', OLD.id, 'sale_trade_id', OLD.sale_trade_id, 'purchase_trade_id',
                        OLD.purchase_trade_id, 'quantity_allocated', OLD.quantity_allocated));
END;

CREATE TRIGGER income_row_history_update AFTER UPDATE ON income
BEGIN
    INSERT INTO row_history (table_name, row_id, operation, changed_at, old_row)
    VALUES ('income', OLD.id, 'UPDATE', strftime('%Y-%m-%dT%H:%M:%fZ', 'now'),
            json_object(
                        'id', OLD.id, 'listing_id', OLD.listing_id, 'date_paid', OLD.date_paid,
                        'ex_date', OLD.ex_date, 'franked_amount', OLD.franked_amount,
                        'unfranked_amount', OLD.unfranked_amount, 'foreign_source_income',
                        OLD.foreign_source_income, 'foreign_tax_paid', OLD.foreign_tax_paid,
                        'tfn_withholding_tax', OLD.tfn_withholding_tax, 'franking_credits',
                        OLD.franking_credits, 'lic_capital_gain_deduction',
                        OLD.lic_capital_gain_deduction, 'conduit_foreign_income',
                        OLD.conduit_foreign_income, 'trust_income', OLD.trust_income,
                        'reinvestment_trade_id', OLD.reinvestment_trade_id, 'currency',
                        OLD.currency, 'buyback_trade_id', OLD.buyback_trade_id,
                        'holding_account_id', OLD.holding_account_id, 'amount_per_security',
                        OLD.amount_per_security, 'securities_held', OLD.securities_held,
                        'entitlement_date', OLD.entitlement_date, 'tax_deferred_amount',
                        OLD.tax_deferred_amount));
END;

CREATE TRIGGER income_row_history_delete AFTER DELETE ON income
BEGIN
    INSERT INTO row_history (table_name, row_id, operation, changed_at, old_row)
    VALUES ('income', OLD.id, 'DELETE', strftime('%Y-%m-%dT%H:%M:%fZ', 'now'),
            json_object(
                        'id', OLD.id, 'listing_id', OLD.listing_id, 'date_paid', OLD.date_paid,
                        'ex_date', OLD.ex_date, 'franked_amount', OLD.franked_amount,
                        'unfranked_amount', OLD.unfranked_amount, 'foreign_source_income',
                        OLD.foreign_source_income, 'foreign_tax_paid', OLD.foreign_tax_paid,
                        'tfn_withholding_tax', OLD.tfn_withholding_tax, 'franking_credits',
                        OLD.franking_credits, 'lic_capital_gain_deduction',
                        OLD.lic_capital_gain_deduction, 'conduit_foreign_income',
                        OLD.conduit_foreign_income, 'trust_income', OLD.trust_income,
                        'reinvestment_trade_id', OLD.reinvestment_trade_id, 'currency',
                        OLD.currency, 'buyback_trade_id', OLD.buyback_trade_id,
                        'holding_account_id', OLD.holding_account_id, 'amount_per_security',
                        OLD.amount_per_security, 'securities_held', OLD.securities_held,
                        'entitlement_date', OLD.entitlement_date, 'tax_deferred_amount',
                        OLD.tax_deferred_amount));
END;

CREATE TRIGGER interest_income_row_history_update AFTER UPDATE ON interest_income
BEGIN
    INSERT INTO row_history (table_name, row_id, operation, changed_at, old_row)
    VALUES ('interest_income', OLD.id, 'UPDATE', strftime('%Y-%m-%dT%H:%M:%fZ', 'now'),
            json_object(
                        'id', OLD.id, 'date_paid', OLD.date_paid, 'amount', OLD.amount,
                        'tfn_withholding_tax', OLD.tfn_withholding_tax, 'currency',
                        OLD.currency, 'source', OLD.source, 'holding_account_id',
                        OLD.holding_account_id, 'foreign_source', OLD.foreign_source,
                        'foreign_tax_paid', OLD.foreign_tax_paid));
END;

CREATE TRIGGER interest_income_row_history_delete AFTER DELETE ON interest_income
BEGIN
    INSERT INTO row_history (table_name, row_id, operation, changed_at, old_row)
    VALUES ('interest_income', OLD.id, 'DELETE', strftime('%Y-%m-%dT%H:%M:%fZ', 'now'),
            json_object(
                        'id', OLD.id, 'date_paid', OLD.date_paid, 'amount', OLD.amount,
                        'tfn_withholding_tax', OLD.tfn_withholding_tax, 'currency',
                        OLD.currency, 'source', OLD.source, 'holding_account_id',
                        OLD.holding_account_id, 'foreign_source', OLD.foreign_source,
                        'foreign_tax_paid', OLD.foreign_tax_paid));
END;

CREATE TRIGGER amma_statements_row_history_update AFTER UPDATE ON amma_statements
BEGIN
    INSERT INTO row_history (table_name, row_id, operation, changed_at, old_row)
    VALUES ('amma_statements', OLD.id, 'UPDATE', strftime('%Y-%m-%dT%H:%M:%fZ', 'now'),
            json_object(
                        'id', OLD.id, 'listing_id', OLD.listing_id, 'tax_year_end_date',
                        OLD.tax_year_end_date, 'units_held', OLD.units_held, 'date_received',
                        OLD.date_received, 'australian_interest', OLD.australian_interest,
                        'australian_dividends_unfranked', OLD.australian_dividends_unfranked,
                        'franked_dividends', OLD.franked_dividends, 'franking_credits',
                        OLD.franking_credits, 'net_rent', OLD.net_rent, 'foreign_income',
                        OLD.foreign_income, 'foreign_tax_credits', OLD.foreign_tax_credits,
                        'other_income', OLD.other_income, 'cgt_discount_gains',
                        OLD.cgt_discount_gains, 'cgt_indexation_gains',
                        OLD.cgt_indexation_gains, 'cgt_other_gains', OLD.cgt_other_gains,
                        'capital_losses_applied', OLD.capital_losses_applied,
                        'tax_deferred_amount', OLD.tax_deferred_amount, 'tax_free_amount',
                        OLD.tax_free_amount, 'cost_base_adjustment', OLD.cost_base_adjustment,
                        'tfn_withholding_tax', OLD.tfn_withholding_tax, 'currency',
                        OLD.currency, 'holding_account_id', OLD.holding_account_id));
END;

CREATE TRIGGER amma_statements_row_history_delete AFTER DELETE ON amma_statements
BEGIN
    INSERT INTO row_history (table_name, row_id, operation, changed_at, old_row)
    VALUES ('amma_statements', OLD.id, 'DELETE', strftime('%Y-%m-%dT%H:%M:%fZ', 'now'),
            json_object(
                        'id', OLD.id, 'listing_id', OLD.listing_id, 'tax_year_end_date',
                        OLD.tax_year_end_date, 'units_held', OLD.units_held, 'date_received',
                        OLD.date_received, 'australian_interest', OLD.australian_interest,
                        'australian_dividends_unfranked', OLD.australian_dividends_unfranked,
                        'franked_dividends', OLD.franked_dividends, 'franking_credits',
                        OLD.franking_credits, 'net_rent', OLD.net_rent, 'foreign_income',
                        OLD.foreign_income, 'foreign_tax_credits', OLD.foreign_tax_credits,
                        'other_income', OLD.other_income, 'cgt_discount_gains',
                        OLD.cgt_discount_gains, 'cgt_indexation_gains',
                        OLD.cgt_indexation_gains, 'cgt_other_gains', OLD.cgt_other_gains,
                        'capital_losses_applied', OLD.capital_losses_applied,
                        'tax_deferred_amount', OLD.tax_deferred_amount, 'tax_free_amount',
                        OLD.tax_free_amount, 'cost_base_adjustment', OLD.cost_base_adjustment,
                        'tfn_withholding_tax', OLD.tfn_withholding_tax, 'currency',
                        OLD.currency, 'holding_account_id', OLD.holding_account_id));
END;

CREATE TRIGGER amit_adjustments_row_history_update AFTER UPDATE ON amit_adjustments
BEGIN
    INSERT INTO row_history (table_name, row_id, operation, changed_at, old_row)
    VALUES ('amit_adjustments', OLD.id, 'UPDATE', strftime('%Y-%m-%dT%H:%M:%fZ', 'now'),
            json_object(
                        'id', OLD.id, 'amma_statement_id', OLD.amma_statement_id, 'trade_id',
                        OLD.trade_id, 'quantity', OLD.quantity));
END;

CREATE TRIGGER amit_adjustments_row_history_delete AFTER DELETE ON amit_adjustments
BEGIN
    INSERT INTO row_history (table_name, row_id, operation, changed_at, old_row)
    VALUES ('amit_adjustments', OLD.id, 'DELETE', strftime('%Y-%m-%dT%H:%M:%fZ', 'now'),
            json_object(
                        'id', OLD.id, 'amma_statement_id', OLD.amma_statement_id, 'trade_id',
                        OLD.trade_id, 'quantity', OLD.quantity));
END;

CREATE TRIGGER ess_statements_row_history_update AFTER UPDATE ON ess_statements
BEGIN
    INSERT INTO row_history (table_name, row_id, operation, changed_at, old_row)
    VALUES ('ess_statements', OLD.id, 'UPDATE', strftime('%Y-%m-%dT%H:%M:%fZ', 'now'),
            json_object(
                        'id', OLD.id, 'listing_id', OLD.listing_id, 'holding_account_id',
                        OLD.holding_account_id, 'taxing_point_date', OLD.taxing_point_date,
                        'quantity', OLD.quantity, 'market_value_per_share',
                        OLD.market_value_per_share, 'taxed_upfront_eligible',
                        OLD.taxed_upfront_eligible, 'taxed_upfront_not_eligible',
                        OLD.taxed_upfront_not_eligible, 'deferral_discount',
                        OLD.deferral_discount, 'pre_2009_cessation_discount',
                        OLD.pre_2009_cessation_discount, 'foreign_source_discount',
                        OLD.foreign_source_discount, 'tfn_withholding', OLD.tfn_withholding,
                        'currency', OLD.currency, 'aud_taxed_upfront_eligible',
                        OLD.aud_taxed_upfront_eligible, 'aud_taxed_upfront_not_eligible',
                        OLD.aud_taxed_upfront_not_eligible, 'aud_deferral_discount',
                        OLD.aud_deferral_discount, 'aud_pre_2009_cessation_discount',
                        OLD.aud_pre_2009_cessation_discount, 'aud_foreign_source_discount',
                        OLD.aud_foreign_source_discount));
END;

CREATE TRIGGER ess_statements_row_history_delete AFTER DELETE ON ess_statements
BEGIN
    INSERT INTO row_history (table_name, row_id, operation, changed_at, old_row)
    VALUES ('ess_statements', OLD.id, 'DELETE', strftime('%Y-%m-%dT%H:%M:%fZ', 'now'),
            json_object(
                        'id', OLD.id, 'listing_id', OLD.listing_id, 'holding_account_id',
                        OLD.holding_account_id, 'taxing_point_date', OLD.taxing_point_date,
                        'quantity', OLD.quantity, 'market_value_per_share',
                        OLD.market_value_per_share, 'taxed_upfront_eligible',
                        OLD.taxed_upfront_eligible, 'taxed_upfront_not_eligible',
                        OLD.taxed_upfront_not_eligible, 'deferral_discount',
                        OLD.deferral_discount, 'pre_2009_cessation_discount',
                        OLD.pre_2009_cessation_discount, 'foreign_source_discount',
                        OLD.foreign_source_discount, 'tfn_withholding', OLD.tfn_withholding,
                        'currency', OLD.currency, 'aud_taxed_upfront_eligible',
                        OLD.aud_taxed_upfront_eligible, 'aud_taxed_upfront_not_eligible',
                        OLD.aud_taxed_upfront_not_eligible, 'aud_deferral_discount',
                        OLD.aud_deferral_discount, 'aud_pre_2009_cessation_discount',
                        OLD.aud_pre_2009_cessation_discount, 'aud_foreign_source_discount',
                        OLD.aud_foreign_source_discount));
END;

CREATE TRIGGER transfers_row_history_update AFTER UPDATE ON transfers
BEGIN
    INSERT INTO row_history (table_name, row_id, operation, changed_at, old_row)
    VALUES ('transfers', OLD.id, 'UPDATE', strftime('%Y-%m-%dT%H:%M:%fZ', 'now'),
            json_object(
                        'id', OLD.id, 'listing_id', OLD.listing_id, 'date', OLD.date,
                        'from_account_id', OLD.from_account_id, 'to_account_id',
                        OLD.to_account_id, 'fee_sale_trade_id', OLD.fee_sale_trade_id));
END;

CREATE TRIGGER transfers_row_history_delete AFTER DELETE ON transfers
BEGIN
    INSERT INTO row_history (table_name, row_id, operation, changed_at, old_row)
    VALUES ('transfers', OLD.id, 'DELETE', strftime('%Y-%m-%dT%H:%M:%fZ', 'now'),
            json_object(
                        'id', OLD.id, 'listing_id', OLD.listing_id, 'date', OLD.date,
                        'from_account_id', OLD.from_account_id, 'to_account_id',
                        OLD.to_account_id, 'fee_sale_trade_id', OLD.fee_sale_trade_id));
END;

CREATE TRIGGER corporate_actions_row_history_update AFTER UPDATE ON corporate_actions
BEGIN
    INSERT INTO row_history (table_name, row_id, operation, changed_at, old_row)
    VALUES ('corporate_actions', OLD.id, 'UPDATE', strftime('%Y-%m-%dT%H:%M:%fZ', 'now'),
            json_object(
                        'id', OLD.id, 'action_type', OLD.action_type, 'listing_id',
                        OLD.listing_id, 'date', OLD.date, 'amount_per_unit',
                        OLD.amount_per_unit, 'currency', OLD.currency, 'split_new_units',
                        OLD.split_new_units, 'split_old_units', OLD.split_old_units,
                        'bonus_units', OLD.bonus_units, 'bonus_held_units',
                        OLD.bonus_held_units, 'rights_units', OLD.rights_units,
                        'rights_held_units', OLD.rights_held_units, 'exercise_price',
                        OLD.exercise_price, 'buyback_price', OLD.buyback_price,
                        'buyback_dividend', OLD.buyback_dividend, 'buyback_franking_credit',
                        OLD.buyback_franking_credit, 'buyback_market_value',
                        OLD.buyback_market_value, 'scrip_listing_id', OLD.scrip_listing_id,
                        'scrip_new_units', OLD.scrip_new_units, 'scrip_old_units',
                        OLD.scrip_old_units, 'demerger_listing_id', OLD.demerger_listing_id,
                        'demerger_new_units', OLD.demerger_new_units, 'demerger_held_units',
                        OLD.demerger_held_units, 'demerger_cost_base_pct',
                        OLD.demerger_cost_base_pct, 'worthless_event', OLD.worthless_event,
                        'scrip_cash_per_unit', OLD.scrip_cash_per_unit, 'scrip_market_value',
                        OLD.scrip_market_value, 'scrip_cash_currency', OLD.scrip_cash_currency));
END;

CREATE TRIGGER corporate_actions_row_history_delete AFTER DELETE ON corporate_actions
BEGIN
    INSERT INTO row_history (table_name, row_id, operation, changed_at, old_row)
    VALUES ('corporate_actions', OLD.id, 'DELETE', strftime('%Y-%m-%dT%H:%M:%fZ', 'now'),
            json_object(
                        'id', OLD.id, 'action_type', OLD.action_type, 'listing_id',
                        OLD.listing_id, 'date', OLD.date, 'amount_per_unit',
                        OLD.amount_per_unit, 'currency', OLD.currency, 'split_new_units',
                        OLD.split_new_units, 'split_old_units', OLD.split_old_units,
                        'bonus_units', OLD.bonus_units, 'bonus_held_units',
                        OLD.bonus_held_units, 'rights_units', OLD.rights_units,
                        'rights_held_units', OLD.rights_held_units, 'exercise_price',
                        OLD.exercise_price, 'buyback_price', OLD.buyback_price,
                        'buyback_dividend', OLD.buyback_dividend, 'buyback_franking_credit',
                        OLD.buyback_franking_credit, 'buyback_market_value',
                        OLD.buyback_market_value, 'scrip_listing_id', OLD.scrip_listing_id,
                        'scrip_new_units', OLD.scrip_new_units, 'scrip_old_units',
                        OLD.scrip_old_units, 'demerger_listing_id', OLD.demerger_listing_id,
                        'demerger_new_units', OLD.demerger_new_units, 'demerger_held_units',
                        OLD.demerger_held_units, 'demerger_cost_base_pct',
                        OLD.demerger_cost_base_pct, 'worthless_event', OLD.worthless_event,
                        'scrip_cash_per_unit', OLD.scrip_cash_per_unit, 'scrip_market_value',
                        OLD.scrip_market_value, 'scrip_cash_currency', OLD.scrip_cash_currency));
END;

CREATE TRIGGER inheritances_row_history_update AFTER UPDATE ON inheritances
BEGIN
    INSERT INTO row_history (table_name, row_id, operation, changed_at, old_row)
    VALUES ('inheritances', OLD.id, 'UPDATE', strftime('%Y-%m-%dT%H:%M:%fZ', 'now'),
            json_object(
                        'id', OLD.id, 'listing_id', OLD.listing_id, 'holding_account_id',
                        OLD.holding_account_id, 'quantity', OLD.quantity, 'date_of_death',
                        OLD.date_of_death, 'cost_base_rule', OLD.cost_base_rule, 'cost_base',
                        OLD.cost_base, 'lpr_expenditure', OLD.lpr_expenditure,
                        'lpr_expenditure_date', OLD.lpr_expenditure_date,
                        'deceased_acquisition_date', OLD.deceased_acquisition_date, 'currency',
                        OLD.currency, 'fx_rate', OLD.fx_rate));
END;

CREATE TRIGGER inheritances_row_history_delete AFTER DELETE ON inheritances
BEGIN
    INSERT INTO row_history (table_name, row_id, operation, changed_at, old_row)
    VALUES ('inheritances', OLD.id, 'DELETE', strftime('%Y-%m-%dT%H:%M:%fZ', 'now'),
            json_object(
                        'id', OLD.id, 'listing_id', OLD.listing_id, 'holding_account_id',
                        OLD.holding_account_id, 'quantity', OLD.quantity, 'date_of_death',
                        OLD.date_of_death, 'cost_base_rule', OLD.cost_base_rule, 'cost_base',
                        OLD.cost_base, 'lpr_expenditure', OLD.lpr_expenditure,
                        'lpr_expenditure_date', OLD.lpr_expenditure_date,
                        'deceased_acquisition_date', OLD.deceased_acquisition_date, 'currency',
                        OLD.currency, 'fx_rate', OLD.fx_rate));
END;

CREATE TRIGGER rights_sales_row_history_update AFTER UPDATE ON rights_sales
BEGIN
    INSERT INTO row_history (table_name, row_id, operation, changed_at, old_row)
    VALUES ('rights_sales', OLD.id, 'UPDATE', strftime('%Y-%m-%dT%H:%M:%fZ', 'now'),
            json_object(
                        'id', OLD.id, 'rights_action_id', OLD.rights_action_id, 'date',
                        OLD.date, 'units', OLD.units, 'proceeds_per_right',
                        OLD.proceeds_per_right, 'rights_cost', OLD.rights_cost, 'fx_rate',
                        OLD.fx_rate, 'holding_account_id', OLD.holding_account_id));
END;

CREATE TRIGGER rights_sales_row_history_delete AFTER DELETE ON rights_sales
BEGIN
    INSERT INTO row_history (table_name, row_id, operation, changed_at, old_row)
    VALUES ('rights_sales', OLD.id, 'DELETE', strftime('%Y-%m-%dT%H:%M:%fZ', 'now'),
            json_object(
                        'id', OLD.id, 'rights_action_id', OLD.rights_action_id, 'date',
                        OLD.date, 'units', OLD.units, 'proceeds_per_right',
                        OLD.proceeds_per_right, 'rights_cost', OLD.rights_cost, 'fx_rate',
                        OLD.fx_rate, 'holding_account_id', OLD.holding_account_id));
END;

CREATE TRIGGER rights_sale_allocations_row_history_update AFTER UPDATE ON rights_sale_allocations
BEGIN
    INSERT INTO row_history (table_name, row_id, operation, changed_at, old_row)
    VALUES ('rights_sale_allocations', OLD.id, 'UPDATE', strftime('%Y-%m-%dT%H:%M:%fZ', 'now'),
            json_object(
                        'id', OLD.id, 'rights_sale_id', OLD.rights_sale_id, 'purchase_trade_id',
                        OLD.purchase_trade_id, 'units', OLD.units));
END;

CREATE TRIGGER rights_sale_allocations_row_history_delete AFTER DELETE ON rights_sale_allocations
BEGIN
    INSERT INTO row_history (table_name, row_id, operation, changed_at, old_row)
    VALUES ('rights_sale_allocations', OLD.id, 'DELETE', strftime('%Y-%m-%dT%H:%M:%fZ', 'now'),
            json_object(
                        'id', OLD.id, 'rights_sale_id', OLD.rights_sale_id, 'purchase_trade_id',
                        OLD.purchase_trade_id, 'units', OLD.units));
END;

CREATE TRIGGER investment_expenses_row_history_update AFTER UPDATE ON investment_expenses
BEGIN
    INSERT INTO row_history (table_name, row_id, operation, changed_at, old_row)
    VALUES ('investment_expenses', OLD.id, 'UPDATE', strftime('%Y-%m-%dT%H:%M:%fZ', 'now'),
            json_object(
                        'id', OLD.id, 'date_incurred', OLD.date_incurred, 'expense_type',
                        OLD.expense_type, 'amount', OLD.amount, 'gross_amount',
                        OLD.gross_amount, 'deductible_percentage', OLD.deductible_percentage,
                        'currency', OLD.currency, 'description', OLD.description, 'listing_id',
                        OLD.listing_id, 'holding_account_id', OLD.holding_account_id));
END;

CREATE TRIGGER investment_expenses_row_history_delete AFTER DELETE ON investment_expenses
BEGIN
    INSERT INTO row_history (table_name, row_id, operation, changed_at, old_row)
    VALUES ('investment_expenses', OLD.id, 'DELETE', strftime('%Y-%m-%dT%H:%M:%fZ', 'now'),
            json_object(
                        'id', OLD.id, 'date_incurred', OLD.date_incurred, 'expense_type',
                        OLD.expense_type, 'amount', OLD.amount, 'gross_amount',
                        OLD.gross_amount, 'deductible_percentage', OLD.deductible_percentage,
                        'currency', OLD.currency, 'description', OLD.description, 'listing_id',
                        OLD.listing_id, 'holding_account_id', OLD.holding_account_id));
END;

CREATE TRIGGER drp_enrolments_row_history_update AFTER UPDATE ON drp_enrolments
BEGIN
    INSERT INTO row_history (table_name, row_id, operation, changed_at, old_row)
    VALUES ('drp_enrolments', OLD.id, 'UPDATE', strftime('%Y-%m-%dT%H:%M:%fZ', 'now'),
            json_object(
                        'id', OLD.id, 'listing_id', OLD.listing_id, 'enrolment_date',
                        OLD.enrolment_date, 'unenrolment_date', OLD.unenrolment_date,
                        'residual_handling', OLD.residual_handling, 'holding_account_id',
                        OLD.holding_account_id));
END;

CREATE TRIGGER drp_enrolments_row_history_delete AFTER DELETE ON drp_enrolments
BEGIN
    INSERT INTO row_history (table_name, row_id, operation, changed_at, old_row)
    VALUES ('drp_enrolments', OLD.id, 'DELETE', strftime('%Y-%m-%dT%H:%M:%fZ', 'now'),
            json_object(
                        'id', OLD.id, 'listing_id', OLD.listing_id, 'enrolment_date',
                        OLD.enrolment_date, 'unenrolment_date', OLD.unenrolment_date,
                        'residual_handling', OLD.residual_handling, 'holding_account_id',
                        OLD.holding_account_id));
END;

CREATE TRIGGER cgt_settings_row_history_update AFTER UPDATE ON cgt_settings
BEGIN
    INSERT INTO row_history (table_name, row_id, operation, changed_at, old_row)
    VALUES ('cgt_settings', OLD.id, 'UPDATE', strftime('%Y-%m-%dT%H:%M:%fZ', 'now'),
            json_object(
                        'id', OLD.id, 'opening_capital_loss', OLD.opening_capital_loss));
END;

CREATE TRIGGER cgt_settings_row_history_delete AFTER DELETE ON cgt_settings
BEGIN
    INSERT INTO row_history (table_name, row_id, operation, changed_at, old_row)
    VALUES ('cgt_settings', OLD.id, 'DELETE', strftime('%Y-%m-%dT%H:%M:%fZ', 'now'),
            json_object(
                        'id', OLD.id, 'opening_capital_loss', OLD.opening_capital_loss));
END;

CREATE TRIGGER attachments_row_history_update AFTER UPDATE ON attachments
BEGIN
    INSERT INTO row_history (table_name, row_id, operation, changed_at, old_row)
    VALUES ('attachments', OLD.id, 'UPDATE', strftime('%Y-%m-%dT%H:%M:%fZ', 'now'),
            json_object(
                        'id', OLD.id, 'trade_id', OLD.trade_id, 'income_id', OLD.income_id,
                        'amma_statement_id', OLD.amma_statement_id, 'filename', OLD.filename,
                        'content_type', OLD.content_type, 'byte_size', OLD.byte_size,
                        'checksum', OLD.checksum, 'uploaded_at', OLD.uploaded_at));
END;

CREATE TRIGGER attachments_row_history_delete AFTER DELETE ON attachments
BEGIN
    INSERT INTO row_history (table_name, row_id, operation, changed_at, old_row)
    VALUES ('attachments', OLD.id, 'DELETE', strftime('%Y-%m-%dT%H:%M:%fZ', 'now'),
            json_object(
                        'id', OLD.id, 'trade_id', OLD.trade_id, 'income_id', OLD.income_id,
                        'amma_statement_id', OLD.amma_statement_id, 'filename', OLD.filename,
                        'content_type', OLD.content_type, 'byte_size', OLD.byte_size,
                        'checksum', OLD.checksum, 'uploaded_at', OLD.uploaded_at));
END;

CREATE TRIGGER listings_row_history_update AFTER UPDATE ON listings
BEGIN
    INSERT INTO row_history (table_name, row_id, operation, changed_at, old_row)
    VALUES ('listings', OLD.id, 'UPDATE', strftime('%Y-%m-%dT%H:%M:%fZ', 'now'),
            json_object(
                        'id', OLD.id, 'exchange_mic', OLD.exchange_mic, 'ticker', OLD.ticker,
                        'name', OLD.name, 'isin', OLD.isin, 'security_type', OLD.security_type,
                        'currency', OLD.currency, 'amit', OLD.amit, 'preference', OLD.preference));
END;

CREATE TRIGGER listings_row_history_delete AFTER DELETE ON listings
BEGIN
    INSERT INTO row_history (table_name, row_id, operation, changed_at, old_row)
    VALUES ('listings', OLD.id, 'DELETE', strftime('%Y-%m-%dT%H:%M:%fZ', 'now'),
            json_object(
                        'id', OLD.id, 'exchange_mic', OLD.exchange_mic, 'ticker', OLD.ticker,
                        'name', OLD.name, 'isin', OLD.isin, 'security_type', OLD.security_type,
                        'currency', OLD.currency, 'amit', OLD.amit, 'preference', OLD.preference));
END;

