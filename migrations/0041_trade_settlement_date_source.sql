-- SCENARIOS S-04: seeding the calendar the settlement holiday-coverage report
-- asks for silences the report without correcting the settlement dates it
-- flagged. A Buy dated the Thursday before a Good Friday in a year with no
-- seeded `exchange_holidays` rows auto-computes T+2 over weekends only and can
-- land on the Easter Monday nobody has entered yet; seeding that year's
-- calendar extends the coverage span, so the row drops out of
-- `GET /reports/settlement_holiday_coverage` while its stored settlement date
-- stays exactly as wrong as it was.
--
-- The repair Evan chose is a deliberately **unscheduled** `settlement-recompute`
-- job (the `price-rebase` shape from Q-14): re-derive the settlement dates the
-- server itself computed, from the calendar as it now stands, and leave a
-- hand-supplied `settlement_date` alone — an explicit value is the taxpayer's
-- own assertion (S-05: trade 9071, LAC on XNYS, dated 2021-03-25 and settled
-- 2021-05-29, a Saturday two months later, which the coverage report flags and
-- nothing may rewrite).
--
-- That distinction is not answerable from the schema as it stands.
-- `trades.settlement_date` is one plain column written by both paths:
-- `trade::auto_settlement_date` fills it when the PUT body omits it, and the
-- body's own value is stored verbatim when it supplies one. Nothing recorded
-- which of the two wrote the stored date, and no heuristic can recover it — a
-- supplied date that happens to equal T+2 is indistinguishable from a computed
-- one, and a computed date that is now wrong is indistinguishable from a
-- deliberate override. So the provenance is recorded, the way `price_as_observed`
-- (0034) records which basis a stored price arrived in and `domain::rollover`'s
-- provenance columns record which operation wrote a trade.
--
-- Three values, because there are three states and only two of them are
-- knowable:
--
--   'computed'   — `auto_settlement_date` derived this date from the exchange's
--                  calendar at write time (the PUT body omitted
--                  `settlement_date`). The **only** rows the job rewrites.
--   'stated'     — the date was asserted, not derived: supplied in a
--                  `PUT /trades/:id` or `PUT /sells/:id` body, or written by a
--                  derived path that settles same-day by construction (an ESS
--                  vest, an inherited parcel, a DRP reinvestment, a rights
--                  exercise, a rollover's closing Sell and replacement Buys —
--                  a taxing point, a date of death and a corporate action's
--                  date are not market-settled). Never rewritten.
--   'unrecorded' — the row predates this column, so which path wrote its
--                  settlement date is not knowable. Never rewritten either:
--                  guessing would risk overwriting an assertion like trade
--                  9071's, and the cost of never guessing is nil (see below).
--                  A row leaves this state at its next write through
--                  `PUT /trades/:id` or `PUT /sells/:id`, which records the
--                  real answer.
--
-- Existing rows all take 'unrecorded' from the ADD COLUMN default — no UPDATE,
-- so no `row_history` entry and no snapshot staleness is manufactured by the
-- migration itself. That costs nothing on the live database: its 113 trades
-- run 2020-08-31 to 2026-07-16 and **every** settlement window falls inside the
-- seeded 2019–2027 coverage, so not one of them was computed against a missing
-- calendar in the first place (checked read-only against
-- `share-tracker-2026-08-16-000000.db`, 2026-08-22; the job run against a copy
-- of it recomputes nothing). The job is therefore forward-looking: it repairs
-- the dates this server computes from now on, against a calendar completed
-- later.
--
-- (51 of those 113 do settle on a date T+n arithmetic would not produce, and
-- all 51 are right: they are the derived paths' same-day settlements — ESS
-- vests, DRPs, a transfer's Sell — which is exactly why the job may not act on
-- a row whose provenance it does not know, and why it does not report on them
-- either.)
--
-- The default is deliberately the never-rewritten value, not 'computed': a
-- write path that forgets to name the column can only ever under-claim, so no
-- future insert can silently make a date eligible for rewriting.
ALTER TABLE trades
    ADD COLUMN settlement_date_source TEXT NOT NULL DEFAULT 'unrecorded'
        CHECK (settlement_date_source IN ('computed', 'stated', 'unrecorded'));

-- trades is audited (CLAUDE.md rule): ALTER TABLE ADD COLUMN does not update
-- existing triggers, so its two row_history triggers are re-created here with
-- settlement_date_source added to the JSON column list. The job's UPDATEs are
-- ordinary updates of an audited table, so every settlement date it rewrites
-- leaves the superseded value recoverable through `POST /reports/row_history`.
DROP TRIGGER trades_row_history_update;
DROP TRIGGER trades_row_history_delete;

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
                        'settlement_date_source', OLD.settlement_date_source));
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
                        'settlement_date_source', OLD.settlement_date_source));
END;

-- Snapshot staleness: no new triggers. `trades` already carries its
-- `trades_stale_snapshots_{insert,update,delete}` set from 0001, and this
-- column carries no date of its own, so there is nothing new to date a
-- snapshot from.
--
-- Worth recording, since the job's whole output is settlement-date UPDATEs:
-- `trades_stale_snapshots_update` fires on *any* update of the row and marks
-- every snapshot from the trade date on stale, even though no snapshotted
-- report reads `settlement_date` (valuation and the portfolio series key off
-- `date`; the only readers are the settlement-coverage and FX-coverage
-- reports, neither of which is snapshotted). Measured on a copy of the live
-- database (2026-08-22): one settlement-date UPDATE on the oldest US parcel
-- staled 5,904 of its 6,525 snapshots, which would then regenerate to
-- identical figures — and only the daily job's 14-day window regenerates
-- itself, so the rest would sit badged stale until a
-- `POST /report_snapshots/regenerate_all` over the range clears them.
--
-- Deliberately left alone: the trigger is correct-but-broad, narrowing it is a
-- decision about every trade write rather than about this job, and the cost is
-- a regeneration, never a wrong figure. It also cannot arise on the live
-- database as it stands — every row there is 'unrecorded', so the job has
-- nothing to rewrite.
