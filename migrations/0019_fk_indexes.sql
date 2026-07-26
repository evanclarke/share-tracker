-- Supporting indexes for foreign-key/join columns that had none (DBA review,
-- 2026-07-26). SQLite does not auto-index foreign keys the way some engines
-- do, so every join a report runs against these columns — and every
-- write-time "is this row still referenced" check before a delete — was an
-- unindexed table scan. Purely additive: no table rebuild, no data change,
-- no Rust change. At today's row counts (low hundreds per table, bar
-- closing_prices/report_snapshots which are already covered by their own
-- composite primary keys) this buys nothing yet — it's future-proofing as
-- trades/income/row_history grow, not a fix for a live slowdown.
--
-- Currency-code FK columns (currency, brokerage_currency, scrip_cash_currency
-- etc.) are deliberately excluded: low-cardinality reference data (a handful
-- of distinct codes over hundreds of rows) where an index buys negligible
-- selectivity over a full scan of the small table it's on.

CREATE INDEX trades_listing_id ON trades (listing_id);
CREATE INDEX trades_holding_account_id ON trades (holding_account_id);
CREATE INDEX trades_rights_action_id ON trades (rights_action_id);
CREATE INDEX trades_buyback_action_id ON trades (buyback_action_id);
CREATE INDEX trades_scrip_action_id ON trades (scrip_action_id);
CREATE INDEX trades_demerger_action_id ON trades (demerger_action_id);
CREATE INDEX trades_transfer_id ON trades (transfer_id);
CREATE INDEX trades_ess_statement_id ON trades (ess_statement_id);
CREATE INDEX trades_worthless_action_id ON trades (worthless_action_id);
CREATE INDEX trades_inheritance_id ON trades (inheritance_id);

CREATE INDEX income_listing_id ON income (listing_id);
CREATE INDEX income_reinvestment_trade_id ON income (reinvestment_trade_id);
CREATE INDEX income_buyback_trade_id ON income (buyback_trade_id);
CREATE INDEX income_holding_account_id ON income (holding_account_id);

CREATE INDEX parcel_allocations_sale_trade_id ON parcel_allocations (sale_trade_id);
CREATE INDEX parcel_allocations_purchase_trade_id ON parcel_allocations (purchase_trade_id);

CREATE INDEX amit_adjustments_amma_statement_id ON amit_adjustments (amma_statement_id);
CREATE INDEX amit_adjustments_trade_id ON amit_adjustments (trade_id);

CREATE INDEX amma_statements_listing_id ON amma_statements (listing_id);
CREATE INDEX amma_statements_holding_account_id ON amma_statements (holding_account_id);

CREATE INDEX ess_statements_listing_id ON ess_statements (listing_id);
CREATE INDEX ess_statements_holding_account_id ON ess_statements (holding_account_id);

CREATE INDEX corporate_actions_listing_id ON corporate_actions (listing_id);
CREATE INDEX corporate_actions_scrip_listing_id ON corporate_actions (scrip_listing_id);
CREATE INDEX corporate_actions_demerger_listing_id ON corporate_actions (demerger_listing_id);

CREATE INDEX transfers_listing_id ON transfers (listing_id);
CREATE INDEX transfers_from_account_id ON transfers (from_account_id);
CREATE INDEX transfers_to_account_id ON transfers (to_account_id);
CREATE INDEX transfers_fee_sale_trade_id ON transfers (fee_sale_trade_id);

CREATE INDEX drp_enrolments_listing_id ON drp_enrolments (listing_id);
CREATE INDEX drp_enrolments_holding_account_id ON drp_enrolments (holding_account_id);

CREATE INDEX investment_expenses_listing_id ON investment_expenses (listing_id);
CREATE INDEX investment_expenses_holding_account_id ON investment_expenses (holding_account_id);

CREATE INDEX inheritances_listing_id ON inheritances (listing_id);
CREATE INDEX inheritances_holding_account_id ON inheritances (holding_account_id);

CREATE INDEX rights_sales_rights_action_id ON rights_sales (rights_action_id);
CREATE INDEX rights_sales_holding_account_id ON rights_sales (holding_account_id);

CREATE INDEX rights_sale_allocations_rights_sale_id ON rights_sale_allocations (rights_sale_id);
CREATE INDEX rights_sale_allocations_purchase_trade_id ON rights_sale_allocations (purchase_trade_id);

CREATE INDEX interest_income_holding_account_id ON interest_income (holding_account_id);

CREATE INDEX attachments_trade_id ON attachments (trade_id);
CREATE INDEX attachments_income_id ON attachments (income_id);
CREATE INDEX attachments_amma_statement_id ON attachments (amma_statement_id);
CREATE INDEX attachments_ess_statement_id ON attachments (ess_statement_id);
CREATE INDEX attachments_interest_income_id ON attachments (interest_income_id);
CREATE INDEX attachments_corporate_action_id ON attachments (corporate_action_id);

CREATE INDEX listing_renames_listing_id ON listing_renames (listing_id);
CREATE INDEX listing_renames_old_exchange_mic ON listing_renames (old_exchange_mic);
CREATE INDEX listing_renames_new_exchange_mic ON listing_renames (new_exchange_mic);
