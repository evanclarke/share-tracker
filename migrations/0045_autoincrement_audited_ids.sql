-- no-transaction
-- The 17 audited tables that still reused ids are rebuilt with AUTOINCREMENT
-- ids (SCENARIOS U-a, decided 2026-08-22).
--
-- `row_history` keys a trail on `(table_name, row_id)`, and `row_id` is the
-- audited row's `id`. Nothing binds an id to one row for the life of the
-- database, so an id handed out a second time inherits every entry the
-- previous occupant left. Migration 0021 identified this for closing_prices
-- and 0039 restated it for exchange_holidays, both fixing it the same way:
-- "a plain INTEGER PRIMARY KEY reuses the highest rowid after a delete, so
-- deleting a holiday and later adding another would hand the new row the
-- deleted one's id — and with it the deleted row's audit history. AUTOINCREMENT
-- never reuses an id, so a trail always belongs to exactly one holiday."
--
-- The reasoning was never applied to the other 20 audited tables, and the reuse
-- is not theoretical. In the live database as of 2026-08-22 the trail carries
-- ten `DELETE` entries on ids that hold a row again — eight distinct ids, five
-- in `trades` (9072-9076) and three in `parcel_allocations` (61-63), two of
-- them reused twice. `parcel_allocations` #61 came from an id-less INSERT where
-- SQLite handed back the freed rowid, which is exactly what AUTOINCREMENT
-- prevents, and that is what this migration closes.
--
-- What it does NOT close, stated here so the next reader does not over-read it:
-- AUTOINCREMENT governs only the ids **SQLite** picks, when an INSERT omits the
-- id column. Nine call sites in `src/entities/` compute their own id with
-- `SELECT COALESCE(MAX(id), 0) + 1` and bind it explicitly, and the column
-- definition never gets a say in that; the entity PUTs are upserts on a
-- client-supplied id besides. Reworking those nine call sites to let the
-- database assign the id is the other half of the decision and is still open
-- (TODO, SCENARIOS U-a). The report-side boundary marking, which covers the
-- reuse this migration cannot prevent, already shipped.
--
-- Which tables. Of the 22 audited tables, five are deliberately left alone:
--
--   * closing_prices (0021), rba_fx_rates (0031) and exchange_holidays (0039)
--     are already AUTOINCREMENT — they are where the rule came from.
--   * tax_year_settings is keyed on `tax_year`, the financial year itself. It
--     has no surrogate id, so there is nothing to make AUTOINCREMENT, and
--     0027 decided that re-entering a year's settings is the *same*
--     taxpayer-year fact — inheriting that year's trail is correct, which is
--     why the report exempts it from occupancy marking too.
--   * cgt_settings is `id INTEGER PRIMARY KEY CHECK (id = 1)`, a singleton.
--     The CHECK pins the id, so re-creating the one row after deleting it is
--     legitimate re-entry of the same fact, not reuse of a freed id.
--
-- The remaining 17 are rebuilt below, in the order row_history's table_name
-- CHECK lists them.
--
-- Shape. `ALTER TABLE x RENAME TO x_old` rewrites references to `x` in other
-- tables' REFERENCES clauses whenever foreign keys are enabled, and in trigger
-- bodies. Most of these tables are referenced by another: attachments alone has
-- six ON DELETE CASCADE parents here, and leaving it pointing at
-- `<parent>_old` would cascade every attachment away when that table is
-- dropped. Neither PRAGMA that suppresses the rewrite can be set inside a
-- transaction (`foreign_keys` is a documented no-op there), so this migration
-- runs `-- no-transaction` and brackets its own work in BEGIN/COMMIT — SQLite's
-- own documented procedure for altering a constraint — exactly as 0029 did for
-- the first rebuild of a referenced table. `legacy_alter_table` keeps each
-- rename from rewriting trigger bodies as well.
--
-- Per table: both trigger sets are dropped first (`*_row_history_*` and, where
-- the table has them, `*_stale_snapshots_*`), the table is renamed, re-created
-- with `id INTEGER PRIMARY KEY AUTOINCREMENT` and every other column, index and
-- constraint unchanged, every row copied `ORDER BY id` with its id, the old
-- table dropped, and every index and trigger re-created. Each table's
-- definition and both triggers are reproduced from the live schema, so the
-- column lists the trail records are exactly the ones it recorded before this
-- migration (`reports::row_history`'s
-- `every_audited_column_is_recorded_by_both_triggers` pins that against
-- `pragma_table_info`, and the staleness sets against
-- `reports::snapshot::STALENESS_TRIGGERED_TABLES`). The staleness triggers are
-- re-created *after* the copy so the migration's own INSERTs do not stale every
-- stored snapshot.
--
-- Money and quantity columns are TEXT decimals and are copied column-to-column
-- with no expression around them — nothing is CAST, re-scaled or round-tripped
-- through REAL (`infra::db`'s `migrations_store_decimals_as_text` guard).
-- No row is dropped and no id changes: an id is a link target across this
-- schema (row_history entries, the provenance columns, every foreign key) and
-- re-numbering would break all of them.
--
-- Seeding sqlite_sequence. AUTOINCREMENT never hands out an id at or below the
-- value stored for the table in `sqlite_sequence`, and a plain
-- `INSERT INTO new SELECT ... FROM old` sets that to the largest id copied.
-- That is not enough: an id deleted *before* this migration is above no live
-- row, so it would still be handed out afterwards — re-creating the very bug
-- for historical ids. This is not defensive: in the live database
-- `parcel_allocations` holds 33 rows with a maximum id of **63**, while its
-- trail's highest `row_id` is **65** — 64 and 65 were allocated, audited and
-- deleted. Copying only the live rows would set the sequence to 63, and the
-- next two allocations inserted would be handed 64 and 65, the second of them
-- landing on an id that already has an audit trail. So each table's sequence is
-- seeded to the maximum of its largest live id and the largest `row_id` that
-- table has ever recorded in `row_history`, which is the only surviving record
-- of an id that no longer holds a row. `attachments` is the mirror case (live
-- maximum 140, trail 136) and takes 140. The trail is append-only and
-- keep-forever (0013), so the high-water mark cannot recede.
--
-- A table with no rows and no trail entries seeds to 0, which is the same
-- thing an untouched AUTOINCREMENT table means: the first id it issues is 1.

PRAGMA foreign_keys = OFF;

BEGIN;

-- ---------------------------------------------------------------------------
-- trades
-- ---------------------------------------------------------------------------

DROP TRIGGER trades_stale_snapshots_insert;
DROP TRIGGER trades_stale_snapshots_update;
DROP TRIGGER trades_stale_snapshots_delete;
DROP TRIGGER trades_row_history_update;
DROP TRIGGER trades_row_history_delete;

PRAGMA legacy_alter_table = ON;
ALTER TABLE trades RENAME TO trades_old;
PRAGMA legacy_alter_table = OFF;

CREATE TABLE trades (
    id                  INTEGER PRIMARY KEY AUTOINCREMENT,
    trade_type          TEXT    NOT NULL CHECK(trade_type IN ('Buy', 'Sell', 'DRP')),
    date                TEXT    NOT NULL,
    settlement_date     TEXT    NOT NULL,
    listing_id          INTEGER NOT NULL REFERENCES listings(id),
    average_price       TEXT    NOT NULL,
    quantity            TEXT    NOT NULL,
    currency            TEXT    NOT NULL REFERENCES currencies(code),
    brokerage           TEXT    NOT NULL DEFAULT '0',
    gst_on_brokerage    TEXT    NOT NULL DEFAULT '0',
    brokerage_currency  TEXT    NOT NULL REFERENCES currencies(code),
    fx_rate             TEXT    NOT NULL DEFAULT '1',
    contract_note_ref   TEXT,
    residual_brought_forward TEXT NOT NULL DEFAULT '0',
    residual_carried_forward TEXT NOT NULL DEFAULT '0',
    residual_paid_out        TEXT NOT NULL DEFAULT '0',
    -- Provenance links from the rollover/exercise Buys and Sells back to the
    -- corporate action that produced them (NULL for ordinary trades).
    rights_action_id    INTEGER REFERENCES corporate_actions(id),
    buyback_action_id   INTEGER REFERENCES corporate_actions(id),
    scrip_action_id     INTEGER REFERENCES corporate_actions(id),
    demerger_action_id  INTEGER REFERENCES corporate_actions(id),
    -- Pre-CGT/rollover deemed acquisition date carried onto a rollover Buy.
    deemed_acquisition_date TEXT,
    holding_account_id  INTEGER NOT NULL DEFAULT 1 REFERENCES holding_accounts(id),
    -- Set on the paired Sell/Buys that implement an inter-account transfer.
    transfer_id         INTEGER REFERENCES transfers(id),
    -- 1 if the brokerage amount was *entered* GST-inclusive; the server splits
    -- it at write time so the stored brokerage/gst_on_brokerage keep their
    -- ex-GST semantics. Persisted only so a trade round-trips into the form.
    brokerage_includes_gst INTEGER NOT NULL DEFAULT 0
        CHECK (brokerage_includes_gst IN (0, 1)),
    -- The broker statement's net transaction total in the brokerage currency,
    -- validated at write time against quantity x price +/- (brokerage + GST).
    -- Informational/validation-only: no report or calculation uses it.
    statement_total     TEXT,
    ess_statement_id    INTEGER REFERENCES ess_statements(id),
    -- Provenance link from a worthless-shares closing Sell back to its
    -- WorthlessShares action: set only by POST /corporate_actions/:id/recognise
    -- (NULL for every other trade). The Sell carrying it is rejected by
    -- PUT /sells and PUT/DELETE /trades; DELETE /sells on it removes it and
    -- restores the holding, and the action is frozen while it exists. Unlike
    -- the rollover provenance columns, a Sell carrying it IS counted by the
    -- realised-gains report (its nil proceeds recognise the capital loss).
    worthless_action_id INTEGER REFERENCES corporate_actions(id),
    -- Provenance link from an inherited-parcel Buy back to its inheritance
    -- (NULL for every other trade), set only by PUT /inheritances/:id: the Buy
    -- carrying it is edited and deleted through its inheritance (0005).
    inheritance_id      INTEGER REFERENCES inheritances(id),
    -- Deliberate per-trade spot-rate override, taking precedence over the
    -- monthly RBA rate; NULL keeps the default (monthly rate first, fx_rate as
    -- fallback). Rejected on an AUD trade at write time (0010).
    spot_fx_rate        TEXT,
    -- Which path wrote settlement_date, so the recompute job knows what it may
    -- rewrite: 'computed' (T+n arithmetic), 'stated' (asserted or derived —
    -- never rewritten), 'unrecorded' (the row predates the column) (0041).
    settlement_date_source TEXT NOT NULL DEFAULT 'unrecorded'
        CHECK (settlement_date_source IN ('computed', 'stated', 'unrecorded'))
);

INSERT INTO trades (
    id, trade_type, date, settlement_date, listing_id, average_price, quantity, currency,
    brokerage, gst_on_brokerage, brokerage_currency, fx_rate, contract_note_ref,
    residual_brought_forward, residual_carried_forward, residual_paid_out,
    rights_action_id, buyback_action_id, scrip_action_id, demerger_action_id,
    deemed_acquisition_date, holding_account_id, transfer_id, brokerage_includes_gst,
    statement_total, ess_statement_id, worthless_action_id, inheritance_id, spot_fx_rate,
    settlement_date_source
)
    SELECT
    id, trade_type, date, settlement_date, listing_id, average_price, quantity, currency,
    brokerage, gst_on_brokerage, brokerage_currency, fx_rate, contract_note_ref,
    residual_brought_forward, residual_carried_forward, residual_paid_out,
    rights_action_id, buyback_action_id, scrip_action_id, demerger_action_id,
    deemed_acquisition_date, holding_account_id, transfer_id, brokerage_includes_gst,
    statement_total, ess_statement_id, worthless_action_id, inheritance_id, spot_fx_rate,
    settlement_date_source
    FROM trades_old
    ORDER BY id;

DROP TABLE trades_old;

CREATE INDEX trades_date ON trades (date);
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

CREATE TRIGGER trades_stale_snapshots_insert AFTER INSERT ON trades
BEGIN
    UPDATE report_snapshots SET stale = 1 WHERE snapshot_date >= NEW.date;
END;

CREATE TRIGGER trades_stale_snapshots_update AFTER UPDATE ON trades
BEGIN
    UPDATE report_snapshots SET stale = 1
    WHERE snapshot_date >= MIN(OLD.date, NEW.date);
END;

CREATE TRIGGER trades_stale_snapshots_delete AFTER DELETE ON trades
BEGIN
    UPDATE report_snapshots SET stale = 1 WHERE snapshot_date >= OLD.date;
END;

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

INSERT INTO sqlite_sequence (name, seq)
    SELECT 'trades', 0 WHERE NOT EXISTS (SELECT 1 FROM sqlite_sequence WHERE name = 'trades');
UPDATE sqlite_sequence
    SET seq = MAX(seq, (SELECT COALESCE(MAX(row_id), 0) FROM row_history
                         WHERE table_name = 'trades'))
    WHERE name = 'trades';

-- ---------------------------------------------------------------------------
-- parcel_allocations
-- ---------------------------------------------------------------------------

DROP TRIGGER parcel_allocations_stale_snapshots_insert;
DROP TRIGGER parcel_allocations_stale_snapshots_update;
DROP TRIGGER parcel_allocations_stale_snapshots_delete;
DROP TRIGGER parcel_allocations_row_history_update;
DROP TRIGGER parcel_allocations_row_history_delete;

PRAGMA legacy_alter_table = ON;
ALTER TABLE parcel_allocations RENAME TO parcel_allocations_old;
PRAGMA legacy_alter_table = OFF;

CREATE TABLE parcel_allocations (
    id                INTEGER PRIMARY KEY AUTOINCREMENT,
    sale_trade_id     INTEGER NOT NULL REFERENCES trades(id),
    purchase_trade_id INTEGER NOT NULL REFERENCES trades(id),
    quantity_allocated TEXT    NOT NULL
);

INSERT INTO parcel_allocations (
    id, sale_trade_id, purchase_trade_id, quantity_allocated
)
    SELECT
    id, sale_trade_id, purchase_trade_id, quantity_allocated
    FROM parcel_allocations_old
    ORDER BY id;

DROP TABLE parcel_allocations_old;

CREATE INDEX parcel_allocations_sale_trade_id ON parcel_allocations (sale_trade_id);
CREATE INDEX parcel_allocations_purchase_trade_id ON parcel_allocations (purchase_trade_id);

CREATE TRIGGER parcel_allocations_stale_snapshots_insert AFTER INSERT ON parcel_allocations
BEGIN
    UPDATE report_snapshots SET stale = 1 WHERE snapshot_date >=
        (SELECT date FROM trades WHERE id = NEW.sale_trade_id);
END;

CREATE TRIGGER parcel_allocations_stale_snapshots_update AFTER UPDATE ON parcel_allocations
BEGIN
    UPDATE report_snapshots SET stale = 1 WHERE snapshot_date >=
        MIN((SELECT date FROM trades WHERE id = OLD.sale_trade_id),
            (SELECT date FROM trades WHERE id = NEW.sale_trade_id));
END;

CREATE TRIGGER parcel_allocations_stale_snapshots_delete AFTER DELETE ON parcel_allocations
BEGIN
    UPDATE report_snapshots SET stale = 1 WHERE snapshot_date >=
        (SELECT date FROM trades WHERE id = OLD.sale_trade_id);
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

INSERT INTO sqlite_sequence (name, seq)
    SELECT 'parcel_allocations', 0 WHERE NOT EXISTS (SELECT 1 FROM sqlite_sequence WHERE name = 'parcel_allocations');
UPDATE sqlite_sequence
    SET seq = MAX(seq, (SELECT COALESCE(MAX(row_id), 0) FROM row_history
                         WHERE table_name = 'parcel_allocations'))
    WHERE name = 'parcel_allocations';

-- ---------------------------------------------------------------------------
-- income
-- ---------------------------------------------------------------------------

DROP TRIGGER income_stale_snapshots_insert;
DROP TRIGGER income_stale_snapshots_update;
DROP TRIGGER income_stale_snapshots_delete;
DROP TRIGGER income_row_history_update;
DROP TRIGGER income_row_history_delete;

PRAGMA legacy_alter_table = ON;
ALTER TABLE income RENAME TO income_old;
PRAGMA legacy_alter_table = OFF;

CREATE TABLE income (
    id                          INTEGER PRIMARY KEY AUTOINCREMENT,
    listing_id                  INTEGER NOT NULL REFERENCES listings(id),
    date_paid                   TEXT    NOT NULL,
    ex_date                     TEXT,
    franked_amount              TEXT    NOT NULL DEFAULT '0',
    unfranked_amount            TEXT    NOT NULL DEFAULT '0',
    foreign_source_income       TEXT    NOT NULL DEFAULT '0',
    foreign_tax_paid            TEXT    NOT NULL DEFAULT '0',
    tfn_withholding_tax         TEXT    NOT NULL DEFAULT '0',
    franking_credits            TEXT    NOT NULL DEFAULT '0',
    lic_capital_gain_amount     TEXT    NOT NULL DEFAULT '0',
    conduit_foreign_income      TEXT    NOT NULL DEFAULT '0',
    trust_income                INTEGER NOT NULL DEFAULT 0,
    reinvestment_trade_id       INTEGER REFERENCES trades(id),
    currency                    TEXT    NOT NULL DEFAULT 'AUD' REFERENCES currencies(code),
    buyback_trade_id            INTEGER REFERENCES trades(id),
    holding_account_id          INTEGER NOT NULL DEFAULT 1 REFERENCES holding_accounts(id),
    -- Optional per-share figures from the registry statement, for cross-checking
    -- a distribution against its payment advice: when supplied (always together),
    -- amount_per_security × securities_held cent-rounded must equal the gross
    -- cash components — validated at write time in entities::income.
    -- Informational/validation-only: no report or calculation reads them.
    amount_per_security         TEXT,
    securities_held             TEXT,
    entitlement_date            TEXT
        CHECK (entitlement_date IS NULL OR trust_income = 1),
    tax_deferred_amount         TEXT
        CHECK (tax_deferred_amount IS NULL
               OR (trust_income = 1 AND CAST(tax_deferred_amount AS NUMERIC) >= 0)),
    income_type                 TEXT    NOT NULL DEFAULT 'Dividend'
        CHECK (income_type IN ('Dividend', 'EmploymentIncome', 'OtherIncome'))
);

INSERT INTO income (
    id, listing_id, date_paid, ex_date, franked_amount, unfranked_amount,
    foreign_source_income, foreign_tax_paid, tfn_withholding_tax, franking_credits,
    lic_capital_gain_amount, conduit_foreign_income, trust_income, reinvestment_trade_id,
    currency, buyback_trade_id, holding_account_id, amount_per_security, securities_held,
    entitlement_date, tax_deferred_amount, income_type
)
    SELECT
    id, listing_id, date_paid, ex_date, franked_amount, unfranked_amount,
    foreign_source_income, foreign_tax_paid, tfn_withholding_tax, franking_credits,
    lic_capital_gain_amount, conduit_foreign_income, trust_income, reinvestment_trade_id,
    currency, buyback_trade_id, holding_account_id, amount_per_security, securities_held,
    entitlement_date, tax_deferred_amount, income_type
    FROM income_old
    ORDER BY id;

DROP TABLE income_old;

CREATE INDEX income_date_paid ON income (date_paid);
CREATE INDEX income_listing_id ON income (listing_id);
CREATE INDEX income_reinvestment_trade_id ON income (reinvestment_trade_id);
CREATE INDEX income_buyback_trade_id ON income (buyback_trade_id);
CREATE INDEX income_holding_account_id ON income (holding_account_id);

CREATE TRIGGER income_stale_snapshots_insert AFTER INSERT ON income
BEGIN
    UPDATE report_snapshots SET stale = 1 WHERE snapshot_date >= NEW.date_paid;
END;

CREATE TRIGGER income_stale_snapshots_update AFTER UPDATE ON income
BEGIN
    UPDATE report_snapshots SET stale = 1
    WHERE snapshot_date >= MIN(OLD.date_paid, NEW.date_paid);
END;

CREATE TRIGGER income_stale_snapshots_delete AFTER DELETE ON income
BEGIN
    UPDATE report_snapshots SET stale = 1 WHERE snapshot_date >= OLD.date_paid;
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
                        OLD.franking_credits, 'lic_capital_gain_amount',
                        OLD.lic_capital_gain_amount, 'conduit_foreign_income',
                        OLD.conduit_foreign_income, 'trust_income', OLD.trust_income,
                        'reinvestment_trade_id', OLD.reinvestment_trade_id, 'currency',
                        OLD.currency, 'buyback_trade_id', OLD.buyback_trade_id,
                        'holding_account_id', OLD.holding_account_id, 'amount_per_security',
                        OLD.amount_per_security, 'securities_held', OLD.securities_held,
                        'entitlement_date', OLD.entitlement_date, 'tax_deferred_amount',
                        OLD.tax_deferred_amount, 'income_type', OLD.income_type));
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
                        OLD.franking_credits, 'lic_capital_gain_amount',
                        OLD.lic_capital_gain_amount, 'conduit_foreign_income',
                        OLD.conduit_foreign_income, 'trust_income', OLD.trust_income,
                        'reinvestment_trade_id', OLD.reinvestment_trade_id, 'currency',
                        OLD.currency, 'buyback_trade_id', OLD.buyback_trade_id,
                        'holding_account_id', OLD.holding_account_id, 'amount_per_security',
                        OLD.amount_per_security, 'securities_held', OLD.securities_held,
                        'entitlement_date', OLD.entitlement_date, 'tax_deferred_amount',
                        OLD.tax_deferred_amount, 'income_type', OLD.income_type));
END;

INSERT INTO sqlite_sequence (name, seq)
    SELECT 'income', 0 WHERE NOT EXISTS (SELECT 1 FROM sqlite_sequence WHERE name = 'income');
UPDATE sqlite_sequence
    SET seq = MAX(seq, (SELECT COALESCE(MAX(row_id), 0) FROM row_history
                         WHERE table_name = 'income'))
    WHERE name = 'income';

-- ---------------------------------------------------------------------------
-- interest_income
-- ---------------------------------------------------------------------------

DROP TRIGGER interest_income_row_history_update;
DROP TRIGGER interest_income_row_history_delete;

PRAGMA legacy_alter_table = ON;
ALTER TABLE interest_income RENAME TO interest_income_old;
PRAGMA legacy_alter_table = OFF;

CREATE TABLE interest_income (
    id                  INTEGER PRIMARY KEY AUTOINCREMENT,
    -- Date the interest was paid/credited: its month drives the ATO FX rate
    -- used to convert a non-AUD amount to AUD, and the Australian financial
    -- year it is assessed in (July–June; a July date belongs to the next FY).
    date_paid           TEXT    NOT NULL,
    -- Gross interest in `currency`, including any TFN amount withheld (the
    -- ATO's 10L convention: the gross figure is declared, the withheld amount
    -- separately at 10M).
    amount              TEXT    NOT NULL DEFAULT '0',
    -- TFN amount withheld from the gross interest; joins the tax summary's
    -- combined TFN withholding line.
    tfn_withholding_tax TEXT    NOT NULL DEFAULT '0',
    currency            TEXT    NOT NULL DEFAULT 'AUD' REFERENCES currencies(code),
    -- Free-text source description (e.g. "ANZ savings account", "ATO interest
    -- on early payment"). Informational only — no calculation reads it.
    source              TEXT,
    -- Optional link to the holding account the interest was paid on (e.g. a
    -- broker cash account). NULL for interest from outside the portfolio's
    -- accounts (an ordinary bank account). Informational only — no
    -- calculation reads it.
    holding_account_id  INTEGER REFERENCES holding_accounts(id),
    -- Whether the payer is foreign-source: the tax summary then routes the row
    -- to question 20 (20E) instead of Australian gross interest at 10L (0011).
    foreign_source      INTEGER NOT NULL DEFAULT 0
        CHECK (foreign_source IN (0, 1)),
    -- Foreign tax withheld from the gross amount, in `currency`; joins the tax
    -- summary's FITO line. Foreign-source rows only, never negative (0011).
    foreign_tax_paid    TEXT    NOT NULL DEFAULT '0'
        CHECK (CAST(foreign_tax_paid AS NUMERIC) >= 0
               AND (foreign_source = 1 OR CAST(foreign_tax_paid AS NUMERIC) = 0))
);

INSERT INTO interest_income (
    id, date_paid, amount, tfn_withholding_tax, currency, source, holding_account_id,
    foreign_source, foreign_tax_paid
)
    SELECT
    id, date_paid, amount, tfn_withholding_tax, currency, source, holding_account_id,
    foreign_source, foreign_tax_paid
    FROM interest_income_old
    ORDER BY id;

DROP TABLE interest_income_old;

CREATE INDEX interest_income_holding_account_id ON interest_income (holding_account_id);

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

INSERT INTO sqlite_sequence (name, seq)
    SELECT 'interest_income', 0 WHERE NOT EXISTS (SELECT 1 FROM sqlite_sequence WHERE name = 'interest_income');
UPDATE sqlite_sequence
    SET seq = MAX(seq, (SELECT COALESCE(MAX(row_id), 0) FROM row_history
                         WHERE table_name = 'interest_income'))
    WHERE name = 'interest_income';

-- ---------------------------------------------------------------------------
-- amma_statements
-- ---------------------------------------------------------------------------

DROP TRIGGER amma_statements_stale_snapshots_insert;
DROP TRIGGER amma_statements_stale_snapshots_update;
DROP TRIGGER amma_statements_stale_snapshots_delete;
DROP TRIGGER amma_statements_row_history_update;
DROP TRIGGER amma_statements_row_history_delete;

PRAGMA legacy_alter_table = ON;
ALTER TABLE amma_statements RENAME TO amma_statements_old;
PRAGMA legacy_alter_table = OFF;

CREATE TABLE amma_statements (
    id                              INTEGER PRIMARY KEY AUTOINCREMENT,
    listing_id                      INTEGER NOT NULL REFERENCES listings(id),
    tax_year_end_date               TEXT    NOT NULL,
    units_held                      TEXT    NOT NULL DEFAULT '0',
    date_received                   TEXT    NOT NULL,
    australian_interest             TEXT    NOT NULL DEFAULT '0',
    australian_dividends_unfranked  TEXT    NOT NULL DEFAULT '0',
    franked_dividends               TEXT    NOT NULL DEFAULT '0',
    franking_credits                TEXT    NOT NULL DEFAULT '0',
    net_rent                        TEXT    NOT NULL DEFAULT '0',
    foreign_income                  TEXT    NOT NULL DEFAULT '0',
    foreign_tax_credits             TEXT    NOT NULL DEFAULT '0',
    other_income                    TEXT    NOT NULL DEFAULT '0',
    cgt_discount_gains              TEXT    NOT NULL DEFAULT '0',
    cgt_indexation_gains            TEXT    NOT NULL DEFAULT '0',
    cgt_other_gains                 TEXT    NOT NULL DEFAULT '0',
    capital_losses_applied          TEXT    NOT NULL DEFAULT '0',
    tax_deferred_amount             TEXT    NOT NULL DEFAULT '0',
    tax_free_amount                 TEXT    NOT NULL DEFAULT '0',
    cost_base_adjustment            TEXT    NOT NULL DEFAULT '0',
    tfn_withholding_tax             TEXT    NOT NULL DEFAULT '0',
    currency                        TEXT    NOT NULL DEFAULT 'AUD' REFERENCES currencies(code),
    -- The holding account the statement covers.
    holding_account_id              INTEGER NOT NULL DEFAULT 1 REFERENCES holding_accounts(id),
    -- The part of the statement's foreign tax credits attaching to its foreign
    -- *capital gains* rather than its foreign income, so the FITO limit can
    -- treat the two apart. '0' on every pre-0032 row, which is what a database
    -- that never separated them has always meant (0032).
    foreign_tax_credits_capital_gains TEXT NOT NULL DEFAULT '0'
);

INSERT INTO amma_statements (
    id, listing_id, tax_year_end_date, units_held, date_received, australian_interest,
    australian_dividends_unfranked, franked_dividends, franking_credits, net_rent,
    foreign_income, foreign_tax_credits, other_income, cgt_discount_gains,
    cgt_indexation_gains, cgt_other_gains, capital_losses_applied, tax_deferred_amount,
    tax_free_amount, cost_base_adjustment, tfn_withholding_tax, currency,
    holding_account_id, foreign_tax_credits_capital_gains
)
    SELECT
    id, listing_id, tax_year_end_date, units_held, date_received, australian_interest,
    australian_dividends_unfranked, franked_dividends, franking_credits, net_rent,
    foreign_income, foreign_tax_credits, other_income, cgt_discount_gains,
    cgt_indexation_gains, cgt_other_gains, capital_losses_applied, tax_deferred_amount,
    tax_free_amount, cost_base_adjustment, tfn_withholding_tax, currency,
    holding_account_id, foreign_tax_credits_capital_gains
    FROM amma_statements_old
    ORDER BY id;

DROP TABLE amma_statements_old;

CREATE INDEX amma_statements_listing_id ON amma_statements (listing_id);
CREATE INDEX amma_statements_holding_account_id ON amma_statements (holding_account_id);

CREATE TRIGGER amma_statements_stale_snapshots_insert AFTER INSERT ON amma_statements
BEGIN
    UPDATE report_snapshots SET stale = 1
    WHERE snapshot_date >= NEW.tax_year_end_date;
END;

CREATE TRIGGER amma_statements_stale_snapshots_update AFTER UPDATE ON amma_statements
BEGIN
    UPDATE report_snapshots SET stale = 1
    WHERE snapshot_date >= MIN(OLD.tax_year_end_date, NEW.tax_year_end_date);
END;

CREATE TRIGGER amma_statements_stale_snapshots_delete AFTER DELETE ON amma_statements
BEGIN
    UPDATE report_snapshots SET stale = 1
    WHERE snapshot_date >= OLD.tax_year_end_date;
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
                        'foreign_tax_credits_capital_gains',
                        OLD.foreign_tax_credits_capital_gains,
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
                        'foreign_tax_credits_capital_gains',
                        OLD.foreign_tax_credits_capital_gains,
                        'other_income', OLD.other_income, 'cgt_discount_gains',
                        OLD.cgt_discount_gains, 'cgt_indexation_gains',
                        OLD.cgt_indexation_gains, 'cgt_other_gains', OLD.cgt_other_gains,
                        'capital_losses_applied', OLD.capital_losses_applied,
                        'tax_deferred_amount', OLD.tax_deferred_amount, 'tax_free_amount',
                        OLD.tax_free_amount, 'cost_base_adjustment', OLD.cost_base_adjustment,
                        'tfn_withholding_tax', OLD.tfn_withholding_tax, 'currency',
                        OLD.currency, 'holding_account_id', OLD.holding_account_id));
END;

INSERT INTO sqlite_sequence (name, seq)
    SELECT 'amma_statements', 0 WHERE NOT EXISTS (SELECT 1 FROM sqlite_sequence WHERE name = 'amma_statements');
UPDATE sqlite_sequence
    SET seq = MAX(seq, (SELECT COALESCE(MAX(row_id), 0) FROM row_history
                         WHERE table_name = 'amma_statements'))
    WHERE name = 'amma_statements';

-- ---------------------------------------------------------------------------
-- amit_adjustments
-- ---------------------------------------------------------------------------

DROP TRIGGER amit_adjustments_stale_snapshots_insert;
DROP TRIGGER amit_adjustments_stale_snapshots_update;
DROP TRIGGER amit_adjustments_stale_snapshots_delete;
DROP TRIGGER amit_adjustments_row_history_update;
DROP TRIGGER amit_adjustments_row_history_delete;

PRAGMA legacy_alter_table = ON;
ALTER TABLE amit_adjustments RENAME TO amit_adjustments_old;
PRAGMA legacy_alter_table = OFF;

CREATE TABLE amit_adjustments (
    id                 INTEGER PRIMARY KEY AUTOINCREMENT,
    amma_statement_id  INTEGER NOT NULL REFERENCES amma_statements(id),
    trade_id           INTEGER NOT NULL REFERENCES trades(id),
    quantity           TEXT    NOT NULL
);

INSERT INTO amit_adjustments (
    id, amma_statement_id, trade_id, quantity
)
    SELECT
    id, amma_statement_id, trade_id, quantity
    FROM amit_adjustments_old
    ORDER BY id;

DROP TABLE amit_adjustments_old;

CREATE INDEX amit_adjustments_amma_statement_id ON amit_adjustments (amma_statement_id);
CREATE INDEX amit_adjustments_trade_id ON amit_adjustments (trade_id);
CREATE UNIQUE INDEX amit_adjustments_statement_trade
    ON amit_adjustments (amma_statement_id, trade_id);

CREATE TRIGGER amit_adjustments_stale_snapshots_insert AFTER INSERT ON amit_adjustments
BEGIN
    UPDATE report_snapshots SET stale = 1 WHERE snapshot_date >=
        (SELECT tax_year_end_date FROM amma_statements WHERE id = NEW.amma_statement_id);
END;

CREATE TRIGGER amit_adjustments_stale_snapshots_update AFTER UPDATE ON amit_adjustments
BEGIN
    UPDATE report_snapshots SET stale = 1 WHERE snapshot_date >=
        MIN((SELECT tax_year_end_date FROM amma_statements WHERE id = OLD.amma_statement_id),
            (SELECT tax_year_end_date FROM amma_statements WHERE id = NEW.amma_statement_id));
END;

CREATE TRIGGER amit_adjustments_stale_snapshots_delete AFTER DELETE ON amit_adjustments
BEGIN
    UPDATE report_snapshots SET stale = 1 WHERE snapshot_date >=
        (SELECT tax_year_end_date FROM amma_statements WHERE id = OLD.amma_statement_id);
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

INSERT INTO sqlite_sequence (name, seq)
    SELECT 'amit_adjustments', 0 WHERE NOT EXISTS (SELECT 1 FROM sqlite_sequence WHERE name = 'amit_adjustments');
UPDATE sqlite_sequence
    SET seq = MAX(seq, (SELECT COALESCE(MAX(row_id), 0) FROM row_history
                         WHERE table_name = 'amit_adjustments'))
    WHERE name = 'amit_adjustments';

-- ---------------------------------------------------------------------------
-- ess_statements
-- ---------------------------------------------------------------------------

DROP TRIGGER ess_statements_row_history_update;
DROP TRIGGER ess_statements_row_history_delete;

PRAGMA legacy_alter_table = ON;
ALTER TABLE ess_statements RENAME TO ess_statements_old;
PRAGMA legacy_alter_table = OFF;

CREATE TABLE ess_statements (
    id                          INTEGER PRIMARY KEY AUTOINCREMENT,
    listing_id                  INTEGER NOT NULL REFERENCES listings(id),
    -- The holding account the ESS interests vest into (an employer-plan
    -- account, typically). Defaults to the seeded default account.
    holding_account_id          INTEGER NOT NULL DEFAULT 1 REFERENCES holding_accounts(id),
    -- The taxing point: the year this date falls in is the assessable year, and
    -- the vest Buy's acquisition/settlement date (the CGT re-acquisition).
    taxing_point_date           TEXT    NOT NULL,
    -- Shares that vest at the taxing point, and their market value per share —
    -- together the cost-base-reset Buy (quantity, price). Positive for a vest.
    quantity                    TEXT    NOT NULL DEFAULT '0',
    market_value_per_share      TEXT    NOT NULL DEFAULT '0',
    -- Item 12 discount labels (all in `currency`): D taxed-upfront eligible for
    -- the $1,000 reduction, E taxed-upfront not eligible, F deferral schemes
    -- (the RSU case), G pre-1 July 2009 cessation discounts assessable this
    -- year. The assessable discount = D + E + F + G − the applied reduction.
    taxed_upfront_eligible      TEXT    NOT NULL DEFAULT '0',
    taxed_upfront_not_eligible  TEXT    NOT NULL DEFAULT '0',
    deferral_discount           TEXT    NOT NULL DEFAULT '0',
    pre_2009_cessation_discount TEXT    NOT NULL DEFAULT '0',
    -- The foreign-source portion of the above discounts (Item 12 label A): a
    -- memo already counted within the discount labels, surfaced separately by
    -- the tax summary for the foreign-income/FITO calculation. Not added on top.
    foreign_source_discount     TEXT    NOT NULL DEFAULT '0',
    -- TFN amounts withheld from the discounts (Item 12 label C); folded into the
    -- tax summary's TFN-withholding line.
    tfn_withholding             TEXT    NOT NULL DEFAULT '0',
    currency                    TEXT    NOT NULL DEFAULT 'AUD' REFERENCES currencies(code),
    -- The employer's own AUD figure for each Item 12 label, converted at the
    -- release-date spot rate. Present, the tax summary reports it verbatim for
    -- that label; NULL, the label keeps converting at the RBA monthly rate.
    -- Non-AUD statements only (0009).
    aud_taxed_upfront_eligible      TEXT,
    aud_taxed_upfront_not_eligible  TEXT,
    aud_deferral_discount           TEXT,
    aud_pre_2009_cessation_discount TEXT,
    aud_foreign_source_discount     TEXT,
    -- The rate the taxpayer states for this statement (foreign-per-AUD, as
    -- trades.fx_rate). NULL means "none stated": the vest then resolves the
    -- taxing-point month's ATO rate and refuses when there is none (0026).
    fx_rate                         TEXT
);

INSERT INTO ess_statements (
    id, listing_id, holding_account_id, taxing_point_date, quantity,
    market_value_per_share, taxed_upfront_eligible, taxed_upfront_not_eligible,
    deferral_discount, pre_2009_cessation_discount, foreign_source_discount,
    tfn_withholding, currency, aud_taxed_upfront_eligible, aud_taxed_upfront_not_eligible,
    aud_deferral_discount, aud_pre_2009_cessation_discount, aud_foreign_source_discount,
    fx_rate
)
    SELECT
    id, listing_id, holding_account_id, taxing_point_date, quantity,
    market_value_per_share, taxed_upfront_eligible, taxed_upfront_not_eligible,
    deferral_discount, pre_2009_cessation_discount, foreign_source_discount,
    tfn_withholding, currency, aud_taxed_upfront_eligible, aud_taxed_upfront_not_eligible,
    aud_deferral_discount, aud_pre_2009_cessation_discount, aud_foreign_source_discount,
    fx_rate
    FROM ess_statements_old
    ORDER BY id;

DROP TABLE ess_statements_old;

CREATE INDEX ess_statements_listing_id ON ess_statements (listing_id);
CREATE INDEX ess_statements_holding_account_id ON ess_statements (holding_account_id);

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
                        OLD.aud_foreign_source_discount, 'fx_rate', OLD.fx_rate));
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
                        OLD.aud_foreign_source_discount, 'fx_rate', OLD.fx_rate));
END;

INSERT INTO sqlite_sequence (name, seq)
    SELECT 'ess_statements', 0 WHERE NOT EXISTS (SELECT 1 FROM sqlite_sequence WHERE name = 'ess_statements');
UPDATE sqlite_sequence
    SET seq = MAX(seq, (SELECT COALESCE(MAX(row_id), 0) FROM row_history
                         WHERE table_name = 'ess_statements'))
    WHERE name = 'ess_statements';

-- ---------------------------------------------------------------------------
-- transfers
-- ---------------------------------------------------------------------------

DROP TRIGGER transfers_row_history_update;
DROP TRIGGER transfers_row_history_delete;

PRAGMA legacy_alter_table = ON;
ALTER TABLE transfers RENAME TO transfers_old;
PRAGMA legacy_alter_table = OFF;

CREATE TABLE transfers (
    id              INTEGER PRIMARY KEY AUTOINCREMENT,
    listing_id      INTEGER NOT NULL REFERENCES listings(id),
    -- The date the holding moves: the transfer-out Sell and transfer-in Buys
    -- are dated on it.
    date            TEXT    NOT NULL,
    from_account_id INTEGER NOT NULL REFERENCES holding_accounts(id),
    to_account_id   INTEGER NOT NULL REFERENCES holding_accounts(id),
    -- The Sell recording the network fee a crypto transfer burns: a real
    -- disposal, created and deleted atomically with the transfer, and counted
    -- by the gains reports — which is why it carries this link rather than
    -- trades.transfer_id (0002).
    fee_sale_trade_id INTEGER REFERENCES trades(id),
    CHECK (from_account_id <> to_account_id)
);

INSERT INTO transfers (
    id, listing_id, date, from_account_id, to_account_id, fee_sale_trade_id
)
    SELECT
    id, listing_id, date, from_account_id, to_account_id, fee_sale_trade_id
    FROM transfers_old
    ORDER BY id;

DROP TABLE transfers_old;

CREATE INDEX transfers_listing_id ON transfers (listing_id);
CREATE INDEX transfers_from_account_id ON transfers (from_account_id);
CREATE INDEX transfers_to_account_id ON transfers (to_account_id);
CREATE INDEX transfers_fee_sale_trade_id ON transfers (fee_sale_trade_id);

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

INSERT INTO sqlite_sequence (name, seq)
    SELECT 'transfers', 0 WHERE NOT EXISTS (SELECT 1 FROM sqlite_sequence WHERE name = 'transfers');
UPDATE sqlite_sequence
    SET seq = MAX(seq, (SELECT COALESCE(MAX(row_id), 0) FROM row_history
                         WHERE table_name = 'transfers'))
    WHERE name = 'transfers';

-- ---------------------------------------------------------------------------
-- corporate_actions
-- ---------------------------------------------------------------------------

DROP TRIGGER corporate_actions_stale_snapshots_insert;
DROP TRIGGER corporate_actions_stale_snapshots_update;
DROP TRIGGER corporate_actions_stale_snapshots_delete;
DROP TRIGGER corporate_actions_row_history_update;
DROP TRIGGER corporate_actions_row_history_delete;

PRAGMA legacy_alter_table = ON;
ALTER TABLE corporate_actions RENAME TO corporate_actions_old;
PRAGMA legacy_alter_table = OFF;

CREATE TABLE corporate_actions (
    id                INTEGER PRIMARY KEY AUTOINCREMENT,
    action_type       TEXT    NOT NULL
        CHECK (action_type IN ('ReturnOfCapital', 'ShareSplit', 'BonusIssue', 'RightsIssue',
                               'BuyBack', 'ScripForScrip', 'Demerger', 'WorthlessShares')),
    listing_id        INTEGER NOT NULL REFERENCES listings(id),
    -- ReturnOfCapital: payment date. ShareSplit: conversion date. BonusIssue:
    -- issue date. RightsIssue: record date (exercise is later). BuyBack: the
    -- buy-back date. ScripForScrip: exchange date. Demerger: demerger date.
    -- WorthlessShares: the declaration date (G3) or deregistration/cancellation
    -- date (C2) — every parcel of listing_id still open on it is closed.
    date              TEXT    NOT NULL,
    -- ReturnOfCapital only: per-unit payment amount in `currency`.
    amount_per_unit   TEXT,
    -- ReturnOfCapital / RightsIssue / BuyBack: the relevant amount's currency.
    currency          TEXT    REFERENCES currencies(code),
    -- ShareSplit only.
    split_new_units   TEXT,
    split_old_units   TEXT,
    -- BonusIssue only.
    bonus_units       TEXT,
    bonus_held_units  TEXT,
    -- RightsIssue only.
    rights_units      TEXT,
    rights_held_units TEXT,
    exercise_price    TEXT,
    -- BuyBack only.
    buyback_price           TEXT,
    buyback_dividend        TEXT,
    buyback_franking_credit TEXT,
    buyback_market_value    TEXT,
    -- ScripForScrip only.
    scrip_listing_id  INTEGER REFERENCES listings(id),
    scrip_new_units   TEXT,
    scrip_old_units   TEXT,
    -- Demerger only.
    demerger_listing_id   INTEGER REFERENCES listings(id),
    demerger_new_units    TEXT,
    demerger_held_units   TEXT,
    demerger_cost_base_pct TEXT,
    -- WorthlessShares only: which CGT event the user is invoking. Both produce
    -- the same loss arithmetic (close every open parcel at nil proceeds); the
    -- discriminator records the legal basis (G3 declaration vs C2
    -- deregistration) for the user's records and the recognise operation's
    -- description. CHECK-constrained enum; NULL for every other action type.
    worthless_event   TEXT    CHECK (worthless_event IN ('G3Declaration', 'C2Cancellation')),
    -- ScripForScrip partial-rollover cash: the cash per unit, the market value
    -- of the scrip received, and the currency both are in. All three present
    -- together or all NULL, and only on a ScripForScrip row (0007).
    scrip_cash_per_unit TEXT
        CHECK (scrip_cash_per_unit IS NULL OR action_type = 'ScripForScrip'),
    scrip_market_value  TEXT
        CHECK ((scrip_market_value IS NULL) = (scrip_cash_per_unit IS NULL)),
    scrip_cash_currency TEXT
        REFERENCES currencies(code)
        CHECK ((scrip_cash_currency IS NULL) = (scrip_cash_per_unit IS NULL)),
    -- ReturnOfCapital only: the date entitlement was fixed, which is the date a
    -- parcel must be held on. NULL keeps the payment-date test (0023).
    record_date         TEXT
        CHECK (record_date IS NULL
               OR (action_type = 'ReturnOfCapital' AND record_date <= date)),
    -- Demerger only: the stated pre-demerger close the cost-base apportionment
    -- is derived from, with its provenance. All four present together or all
    -- NULL, and the close predates the action's own date (0036).
    demerger_close_date TEXT
        CHECK (demerger_close_date IS NULL
               OR (action_type = 'Demerger' AND demerger_close_date < date)),
    demerger_close_price TEXT
        CHECK ((demerger_close_price IS NULL) = (demerger_close_date IS NULL)),
    demerger_close_sourced_from TEXT
        CHECK ((demerger_close_sourced_from IS NULL) = (demerger_close_date IS NULL)),
    demerger_close_reason TEXT
        CHECK ((demerger_close_reason IS NULL) = (demerger_close_date IS NULL)),
    CHECK (action_type <> 'ReturnOfCapital'
           OR (amount_per_unit IS NOT NULL AND currency IS NOT NULL
               AND split_new_units IS NULL AND split_old_units IS NULL
               AND bonus_units IS NULL AND bonus_held_units IS NULL
               AND rights_units IS NULL AND rights_held_units IS NULL
               AND exercise_price IS NULL
               AND buyback_price IS NULL AND buyback_dividend IS NULL
               AND buyback_franking_credit IS NULL AND buyback_market_value IS NULL
               AND scrip_listing_id IS NULL AND scrip_new_units IS NULL
               AND scrip_old_units IS NULL
               AND demerger_listing_id IS NULL AND demerger_new_units IS NULL
               AND demerger_held_units IS NULL AND demerger_cost_base_pct IS NULL
               AND worthless_event IS NULL)),
    CHECK (action_type <> 'ShareSplit'
           OR (split_new_units IS NOT NULL AND split_old_units IS NOT NULL
               AND amount_per_unit IS NULL AND currency IS NULL
               AND bonus_units IS NULL AND bonus_held_units IS NULL
               AND rights_units IS NULL AND rights_held_units IS NULL
               AND exercise_price IS NULL
               AND buyback_price IS NULL AND buyback_dividend IS NULL
               AND buyback_franking_credit IS NULL AND buyback_market_value IS NULL
               AND scrip_listing_id IS NULL AND scrip_new_units IS NULL
               AND scrip_old_units IS NULL
               AND demerger_listing_id IS NULL AND demerger_new_units IS NULL
               AND demerger_held_units IS NULL AND demerger_cost_base_pct IS NULL
               AND worthless_event IS NULL)),
    CHECK (action_type <> 'BonusIssue'
           OR (bonus_units IS NOT NULL AND bonus_held_units IS NOT NULL
               AND amount_per_unit IS NULL AND currency IS NULL
               AND split_new_units IS NULL AND split_old_units IS NULL
               AND rights_units IS NULL AND rights_held_units IS NULL
               AND exercise_price IS NULL
               AND buyback_price IS NULL AND buyback_dividend IS NULL
               AND buyback_franking_credit IS NULL AND buyback_market_value IS NULL
               AND scrip_listing_id IS NULL AND scrip_new_units IS NULL
               AND scrip_old_units IS NULL
               AND demerger_listing_id IS NULL AND demerger_new_units IS NULL
               AND demerger_held_units IS NULL AND demerger_cost_base_pct IS NULL
               AND worthless_event IS NULL)),
    CHECK (action_type <> 'RightsIssue'
           OR (rights_units IS NOT NULL AND rights_held_units IS NOT NULL
               AND exercise_price IS NOT NULL AND currency IS NOT NULL
               AND amount_per_unit IS NULL
               AND split_new_units IS NULL AND split_old_units IS NULL
               AND bonus_units IS NULL AND bonus_held_units IS NULL
               AND buyback_price IS NULL AND buyback_dividend IS NULL
               AND buyback_franking_credit IS NULL AND buyback_market_value IS NULL
               AND scrip_listing_id IS NULL AND scrip_new_units IS NULL
               AND scrip_old_units IS NULL
               AND demerger_listing_id IS NULL AND demerger_new_units IS NULL
               AND demerger_held_units IS NULL AND demerger_cost_base_pct IS NULL
               AND worthless_event IS NULL)),
    CHECK (action_type <> 'BuyBack'
           OR (buyback_price IS NOT NULL AND buyback_dividend IS NOT NULL
               AND buyback_franking_credit IS NOT NULL AND currency IS NOT NULL
               AND amount_per_unit IS NULL
               AND split_new_units IS NULL AND split_old_units IS NULL
               AND bonus_units IS NULL AND bonus_held_units IS NULL
               AND rights_units IS NULL AND rights_held_units IS NULL
               AND exercise_price IS NULL
               AND scrip_listing_id IS NULL AND scrip_new_units IS NULL
               AND scrip_old_units IS NULL
               AND demerger_listing_id IS NULL AND demerger_new_units IS NULL
               AND demerger_held_units IS NULL AND demerger_cost_base_pct IS NULL
               AND worthless_event IS NULL)),
    CHECK (action_type <> 'ScripForScrip'
           OR (scrip_listing_id IS NOT NULL AND scrip_listing_id <> listing_id
               AND scrip_new_units IS NOT NULL AND scrip_old_units IS NOT NULL
               AND amount_per_unit IS NULL AND currency IS NULL
               AND split_new_units IS NULL AND split_old_units IS NULL
               AND bonus_units IS NULL AND bonus_held_units IS NULL
               AND rights_units IS NULL AND rights_held_units IS NULL
               AND exercise_price IS NULL
               AND buyback_price IS NULL AND buyback_dividend IS NULL
               AND buyback_franking_credit IS NULL AND buyback_market_value IS NULL
               AND demerger_listing_id IS NULL AND demerger_new_units IS NULL
               AND demerger_held_units IS NULL AND demerger_cost_base_pct IS NULL
               AND worthless_event IS NULL)),
    CHECK (action_type <> 'Demerger'
           OR (demerger_listing_id IS NOT NULL AND demerger_listing_id <> listing_id
               AND demerger_new_units IS NOT NULL AND demerger_held_units IS NOT NULL
               AND demerger_cost_base_pct IS NOT NULL
               AND amount_per_unit IS NULL AND currency IS NULL
               AND split_new_units IS NULL AND split_old_units IS NULL
               AND bonus_units IS NULL AND bonus_held_units IS NULL
               AND rights_units IS NULL AND rights_held_units IS NULL
               AND exercise_price IS NULL
               AND buyback_price IS NULL AND buyback_dividend IS NULL
               AND buyback_franking_credit IS NULL AND buyback_market_value IS NULL
               AND scrip_listing_id IS NULL AND scrip_new_units IS NULL
               AND scrip_old_units IS NULL
               AND worthless_event IS NULL)),
    -- WorthlessShares: only the event discriminator; every numeric payload NULL.
    CHECK (action_type <> 'WorthlessShares'
           OR (worthless_event IS NOT NULL
               AND amount_per_unit IS NULL AND currency IS NULL
               AND split_new_units IS NULL AND split_old_units IS NULL
               AND bonus_units IS NULL AND bonus_held_units IS NULL
               AND rights_units IS NULL AND rights_held_units IS NULL
               AND exercise_price IS NULL
               AND buyback_price IS NULL AND buyback_dividend IS NULL
               AND buyback_franking_credit IS NULL AND buyback_market_value IS NULL
               AND scrip_listing_id IS NULL AND scrip_new_units IS NULL
               AND scrip_old_units IS NULL
               AND demerger_listing_id IS NULL AND demerger_new_units IS NULL
               AND demerger_held_units IS NULL AND demerger_cost_base_pct IS NULL))
);

INSERT INTO corporate_actions (
    id, action_type, listing_id, date, amount_per_unit, currency, split_new_units,
    split_old_units, bonus_units, bonus_held_units, rights_units, rights_held_units,
    exercise_price, buyback_price, buyback_dividend, buyback_franking_credit,
    buyback_market_value, scrip_listing_id, scrip_new_units, scrip_old_units,
    demerger_listing_id, demerger_new_units, demerger_held_units, demerger_cost_base_pct,
    worthless_event, scrip_cash_per_unit, scrip_market_value, scrip_cash_currency,
    record_date, demerger_close_date, demerger_close_price, demerger_close_sourced_from,
    demerger_close_reason
)
    SELECT
    id, action_type, listing_id, date, amount_per_unit, currency, split_new_units,
    split_old_units, bonus_units, bonus_held_units, rights_units, rights_held_units,
    exercise_price, buyback_price, buyback_dividend, buyback_franking_credit,
    buyback_market_value, scrip_listing_id, scrip_new_units, scrip_old_units,
    demerger_listing_id, demerger_new_units, demerger_held_units, demerger_cost_base_pct,
    worthless_event, scrip_cash_per_unit, scrip_market_value, scrip_cash_currency,
    record_date, demerger_close_date, demerger_close_price, demerger_close_sourced_from,
    demerger_close_reason
    FROM corporate_actions_old
    ORDER BY id;

DROP TABLE corporate_actions_old;

CREATE INDEX corporate_actions_listing_id ON corporate_actions (listing_id);
CREATE INDEX corporate_actions_scrip_listing_id ON corporate_actions (scrip_listing_id);
CREATE INDEX corporate_actions_demerger_listing_id ON corporate_actions (demerger_listing_id);

CREATE TRIGGER corporate_actions_stale_snapshots_insert AFTER INSERT ON corporate_actions
BEGIN
    UPDATE report_snapshots SET stale = 1 WHERE snapshot_date >= NEW.date;
END;

CREATE TRIGGER corporate_actions_stale_snapshots_update AFTER UPDATE ON corporate_actions
BEGIN
    UPDATE report_snapshots SET stale = 1
    WHERE snapshot_date >= MIN(OLD.date, NEW.date);
END;

CREATE TRIGGER corporate_actions_stale_snapshots_delete AFTER DELETE ON corporate_actions
BEGIN
    UPDATE report_snapshots SET stale = 1 WHERE snapshot_date >= OLD.date;
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
                        OLD.scrip_market_value, 'scrip_cash_currency', OLD.scrip_cash_currency,
                        'record_date', OLD.record_date,
                        'demerger_close_date', OLD.demerger_close_date,
                        'demerger_close_price', OLD.demerger_close_price,
                        'demerger_close_sourced_from', OLD.demerger_close_sourced_from,
                        'demerger_close_reason', OLD.demerger_close_reason));
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
                        OLD.scrip_market_value, 'scrip_cash_currency', OLD.scrip_cash_currency,
                        'record_date', OLD.record_date,
                        'demerger_close_date', OLD.demerger_close_date,
                        'demerger_close_price', OLD.demerger_close_price,
                        'demerger_close_sourced_from', OLD.demerger_close_sourced_from,
                        'demerger_close_reason', OLD.demerger_close_reason));
END;

INSERT INTO sqlite_sequence (name, seq)
    SELECT 'corporate_actions', 0 WHERE NOT EXISTS (SELECT 1 FROM sqlite_sequence WHERE name = 'corporate_actions');
UPDATE sqlite_sequence
    SET seq = MAX(seq, (SELECT COALESCE(MAX(row_id), 0) FROM row_history
                         WHERE table_name = 'corporate_actions'))
    WHERE name = 'corporate_actions';

-- ---------------------------------------------------------------------------
-- inheritances
-- ---------------------------------------------------------------------------

DROP TRIGGER inheritances_row_history_update;
DROP TRIGGER inheritances_row_history_delete;

PRAGMA legacy_alter_table = ON;
ALTER TABLE inheritances RENAME TO inheritances_old;
PRAGMA legacy_alter_table = OFF;

CREATE TABLE inheritances (
    id                        INTEGER PRIMARY KEY AUTOINCREMENT,
    listing_id                INTEGER NOT NULL REFERENCES listings(id),
    holding_account_id        INTEGER NOT NULL DEFAULT 1 REFERENCES holding_accounts(id),
    -- Units inherited, in date-of-death terms (validated > 0 in Rust).
    quantity                  TEXT    NOT NULL,
    date_of_death             TEXT    NOT NULL,
    -- Which QC 66053 rule produced the first-element figure (CHECK-constrained
    -- enum): DeceasedCostBase = the deceased acquired the asset on or after
    -- 20 September 1985, so their cost base on the day they died carries
    -- over; MarketValueAtDeath = a pre-CGT asset in the deceased's hands, so
    -- the first element is the asset's market value on the day they died
    -- (the user supplies the valuation figure).
    cost_base_rule            TEXT    NOT NULL
        CHECK (cost_base_rule IN ('DeceasedCostBase', 'MarketValueAtDeath')),
    -- The whole-parcel first-element cost base per that rule, in `currency`.
    cost_base                 TEXT    NOT NULL,
    -- Expenditure of the legal personal representative the beneficiary may
    -- include in the cost base (QC 66053 — e.g. conveyancing on the
    -- transfer, legal costs of proving the will), dated when the LPR
    -- incurred it (on or after the date of death; validated in Rust). Added
    -- to the linked Buy's cost base; the date is provenance (indexation,
    -- where it would matter, is not modelled).
    lpr_expenditure           TEXT    NOT NULL DEFAULT '0',
    lpr_expenditure_date      TEXT,
    -- The deceased's acquisition date — recorded exactly when the
    -- DeceasedCostBase rule applies (it starts the 12-month discount clock
    -- per s 115-30, carried as the Buy's deemed_acquisition_date); a
    -- pre-CGT asset's clock runs from the date of death instead, so the
    -- date is not recorded.
    deceased_acquisition_date TEXT,
    currency                  TEXT    NOT NULL DEFAULT 'AUD' REFERENCES currencies(code),
    -- Manual foreign-per-AUD fallback rate (same convention as
    -- trades.fx_rate; reports prefer the ATO/RBA rate for the acquisition
    -- month).
    fx_rate                   TEXT    NOT NULL DEFAULT '1',
    CHECK ((deceased_acquisition_date IS NOT NULL) = (cost_base_rule = 'DeceasedCostBase')),
    CHECK (deceased_acquisition_date IS NULL OR deceased_acquisition_date <= date_of_death)
);

INSERT INTO inheritances (
    id, listing_id, holding_account_id, quantity, date_of_death, cost_base_rule, cost_base,
    lpr_expenditure, lpr_expenditure_date, deceased_acquisition_date, currency, fx_rate
)
    SELECT
    id, listing_id, holding_account_id, quantity, date_of_death, cost_base_rule, cost_base,
    lpr_expenditure, lpr_expenditure_date, deceased_acquisition_date, currency, fx_rate
    FROM inheritances_old
    ORDER BY id;

DROP TABLE inheritances_old;

CREATE INDEX inheritances_listing_id ON inheritances (listing_id);
CREATE INDEX inheritances_holding_account_id ON inheritances (holding_account_id);

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

INSERT INTO sqlite_sequence (name, seq)
    SELECT 'inheritances', 0 WHERE NOT EXISTS (SELECT 1 FROM sqlite_sequence WHERE name = 'inheritances');
UPDATE sqlite_sequence
    SET seq = MAX(seq, (SELECT COALESCE(MAX(row_id), 0) FROM row_history
                         WHERE table_name = 'inheritances'))
    WHERE name = 'inheritances';

-- ---------------------------------------------------------------------------
-- rights_sales
-- ---------------------------------------------------------------------------

DROP TRIGGER rights_sales_row_history_update;
DROP TRIGGER rights_sales_row_history_delete;

PRAGMA legacy_alter_table = ON;
ALTER TABLE rights_sales RENAME TO rights_sales_old;
PRAGMA legacy_alter_table = OFF;

CREATE TABLE rights_sales (
    id                 INTEGER PRIMARY KEY AUTOINCREMENT,
    rights_action_id   INTEGER NOT NULL REFERENCES corporate_actions(id),
    -- Sale (or lapse/expiry) date; never before the issue's record date
    -- (validated in Rust).
    date               TEXT    NOT NULL,
    -- Rights disposed of, in record-date (as-issued) rights units
    -- (validated > 0 in Rust).
    units              TEXT    NOT NULL,
    -- Per-right capital proceeds in the issue's currency (the action's
    -- `currency` column — no column here, one source of truth). 0 = the
    -- rights lapsed or a free right expired worthless; a renounceable-offer
    -- retail premium is entered as the premium per right (TR 2017/4).
    proceeds_per_right TEXT    NOT NULL DEFAULT '0',
    -- Total paid to acquire the disposed rights (the purchased-rights case),
    -- in the issue's currency: the rights' cost base, apportioned over
    -- `units` by the realised-gains report. 0 for rights issued free (nil
    -- cost base) — so nil proceeds on a paid right realises a capital loss.
    rights_cost        TEXT    NOT NULL DEFAULT '0',
    -- Manual foreign-per-AUD fallback rate (same convention as
    -- trades.fx_rate; reports prefer the ATO/RBA rate).
    fx_rate            TEXT    NOT NULL DEFAULT '1',
    -- The account the proceeds are reported under (informational grouping on
    -- the realised row; anchoring parcels may sit in any account, matching
    -- the exercise operation's account freedom).
    holding_account_id INTEGER NOT NULL DEFAULT 1 REFERENCES holding_accounts(id)
);

INSERT INTO rights_sales (
    id, rights_action_id, date, units, proceeds_per_right, rights_cost, fx_rate,
    holding_account_id
)
    SELECT
    id, rights_action_id, date, units, proceeds_per_right, rights_cost, fx_rate,
    holding_account_id
    FROM rights_sales_old
    ORDER BY id;

DROP TABLE rights_sales_old;

CREATE INDEX rights_sales_rights_action_id ON rights_sales (rights_action_id);
CREATE INDEX rights_sales_holding_account_id ON rights_sales (holding_account_id);

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

INSERT INTO sqlite_sequence (name, seq)
    SELECT 'rights_sales', 0 WHERE NOT EXISTS (SELECT 1 FROM sqlite_sequence WHERE name = 'rights_sales');
UPDATE sqlite_sequence
    SET seq = MAX(seq, (SELECT COALESCE(MAX(row_id), 0) FROM row_history
                         WHERE table_name = 'rights_sales'))
    WHERE name = 'rights_sales';

-- ---------------------------------------------------------------------------
-- rights_sale_allocations
-- ---------------------------------------------------------------------------

DROP TRIGGER rights_sale_allocations_row_history_update;
DROP TRIGGER rights_sale_allocations_row_history_delete;

PRAGMA legacy_alter_table = ON;
ALTER TABLE rights_sale_allocations RENAME TO rights_sale_allocations_old;
PRAGMA legacy_alter_table = OFF;

CREATE TABLE rights_sale_allocations (
    id                INTEGER PRIMARY KEY AUTOINCREMENT,
    rights_sale_id    INTEGER NOT NULL REFERENCES rights_sales(id) ON DELETE CASCADE,
    purchase_trade_id INTEGER NOT NULL REFERENCES trades(id),
    -- Rights anchored to this parcel, in record-date rights units; the sale's
    -- allocations sum exactly to its `units` (validated in Rust).
    units             TEXT    NOT NULL
);

INSERT INTO rights_sale_allocations (
    id, rights_sale_id, purchase_trade_id, units
)
    SELECT
    id, rights_sale_id, purchase_trade_id, units
    FROM rights_sale_allocations_old
    ORDER BY id;

DROP TABLE rights_sale_allocations_old;

CREATE INDEX rights_sale_allocations_rights_sale_id ON rights_sale_allocations (rights_sale_id);
CREATE INDEX rights_sale_allocations_purchase_trade_id ON rights_sale_allocations (purchase_trade_id);

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

INSERT INTO sqlite_sequence (name, seq)
    SELECT 'rights_sale_allocations', 0 WHERE NOT EXISTS (SELECT 1 FROM sqlite_sequence WHERE name = 'rights_sale_allocations');
UPDATE sqlite_sequence
    SET seq = MAX(seq, (SELECT COALESCE(MAX(row_id), 0) FROM row_history
                         WHERE table_name = 'rights_sale_allocations'))
    WHERE name = 'rights_sale_allocations';

-- ---------------------------------------------------------------------------
-- investment_expenses
-- ---------------------------------------------------------------------------

DROP TRIGGER investment_expenses_row_history_update;
DROP TRIGGER investment_expenses_row_history_delete;

PRAGMA legacy_alter_table = ON;
ALTER TABLE investment_expenses RENAME TO investment_expenses_old;
PRAGMA legacy_alter_table = OFF;

CREATE TABLE investment_expenses (
    id                    INTEGER PRIMARY KEY AUTOINCREMENT,
    -- Date the expense was incurred: its month drives the ATO FX rate used to
    -- convert a non-AUD amount to AUD, and the Australian financial year the
    -- deduction is attributed to (July–June; a July date belongs to the next FY).
    date_incurred         TEXT    NOT NULL,
    -- Expense category (CHECK-constrained enum). LoanInterest = interest on money
    -- borrowed to buy income-producing shares; ManagementFee = ongoing investment
    -- management fees; AdviceFee = financial-advice fees about an existing
    -- investment mix; AccountKeepingFee = investment-account fees; Subscription =
    -- specialist investment journals/subscriptions; Other = any other deductible
    -- investment expense.
    expense_type          TEXT    NOT NULL CHECK (expense_type IN (
                              'LoanInterest', 'ManagementFee', 'AdviceFee',
                              'AccountKeepingFee', 'Subscription', 'Other')),
    -- The deductible amount (post-apportionment) — the figure that goes on the
    -- return and the value the tax summary totals.
    amount                TEXT    NOT NULL DEFAULT '0',
    -- Optional provenance (informational only — no calculation reads these): the
    -- pre-apportionment gross expense and the percentage the user determined was
    -- deductible. Stored so a row's `amount` is auditable back to its source.
    gross_amount          TEXT,
    deductible_percentage TEXT,
    currency              TEXT    NOT NULL DEFAULT 'AUD' REFERENCES currencies(code),
    -- Free-text note (e.g. "margin loan interest Q3", "adviser annual fee").
    description           TEXT,
    -- Optional links tying the expense to the holding it relates to. Both NULL for
    -- a portfolio-wide expense (e.g. an adviser's whole-of-portfolio fee).
    listing_id            INTEGER REFERENCES listings(id),
    holding_account_id    INTEGER REFERENCES holding_accounts(id)
);

INSERT INTO investment_expenses (
    id, date_incurred, expense_type, amount, gross_amount, deductible_percentage, currency,
    description, listing_id, holding_account_id
)
    SELECT
    id, date_incurred, expense_type, amount, gross_amount, deductible_percentage, currency,
    description, listing_id, holding_account_id
    FROM investment_expenses_old
    ORDER BY id;

DROP TABLE investment_expenses_old;

CREATE INDEX investment_expenses_listing_id ON investment_expenses (listing_id);
CREATE INDEX investment_expenses_holding_account_id ON investment_expenses (holding_account_id);

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

INSERT INTO sqlite_sequence (name, seq)
    SELECT 'investment_expenses', 0 WHERE NOT EXISTS (SELECT 1 FROM sqlite_sequence WHERE name = 'investment_expenses');
UPDATE sqlite_sequence
    SET seq = MAX(seq, (SELECT COALESCE(MAX(row_id), 0) FROM row_history
                         WHERE table_name = 'investment_expenses'))
    WHERE name = 'investment_expenses';

-- ---------------------------------------------------------------------------
-- drp_enrolments
-- ---------------------------------------------------------------------------

DROP TRIGGER drp_enrolments_row_history_update;
DROP TRIGGER drp_enrolments_row_history_delete;

PRAGMA legacy_alter_table = ON;
ALTER TABLE drp_enrolments RENAME TO drp_enrolments_old;
PRAGMA legacy_alter_table = OFF;

CREATE TABLE drp_enrolments (
    id                INTEGER PRIMARY KEY AUTOINCREMENT,
    listing_id        INTEGER NOT NULL REFERENCES listings(id),
    -- First day of the period (inclusive): distributions with an ex date (or
    -- pay date when no ex date is recorded) from this day reinvest.
    enrolment_date    TEXT NOT NULL,
    -- Day the unenrolment takes effect (exclusive): distributions with an ex
    -- date on or after it no longer reinvest. NULL = open-ended (currently
    -- enrolled). Periods for a (listing, holding account) must not overlap
    -- and at most one may be open at a time — a multi-row invariant enforced
    -- at write time inside a transaction (entities::drp_enrolment).
    unenrolment_date  TEXT,
    residual_handling TEXT NOT NULL DEFAULT 'CarryForward'
        CHECK(residual_handling IN ('CarryForward', 'PayOut')),
    -- The holding account the enrolment applies to.
    holding_account_id INTEGER NOT NULL DEFAULT 1 REFERENCES holding_accounts(id),
    CHECK (unenrolment_date IS NULL OR unenrolment_date > enrolment_date)
);

INSERT INTO drp_enrolments (
    id, listing_id, enrolment_date, unenrolment_date, residual_handling, holding_account_id
)
    SELECT
    id, listing_id, enrolment_date, unenrolment_date, residual_handling, holding_account_id
    FROM drp_enrolments_old
    ORDER BY id;

DROP TABLE drp_enrolments_old;

CREATE INDEX drp_enrolments_listing_id ON drp_enrolments (listing_id);
CREATE INDEX drp_enrolments_holding_account_id ON drp_enrolments (holding_account_id);

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

INSERT INTO sqlite_sequence (name, seq)
    SELECT 'drp_enrolments', 0 WHERE NOT EXISTS (SELECT 1 FROM sqlite_sequence WHERE name = 'drp_enrolments');
UPDATE sqlite_sequence
    SET seq = MAX(seq, (SELECT COALESCE(MAX(row_id), 0) FROM row_history
                         WHERE table_name = 'drp_enrolments'))
    WHERE name = 'drp_enrolments';

-- ---------------------------------------------------------------------------
-- attachments
-- ---------------------------------------------------------------------------

DROP TRIGGER attachments_row_history_update;
DROP TRIGGER attachments_row_history_delete;

PRAGMA legacy_alter_table = ON;
ALTER TABLE attachments RENAME TO attachments_old;
PRAGMA legacy_alter_table = OFF;

CREATE TABLE attachments (
    id                  INTEGER PRIMARY KEY AUTOINCREMENT,
    trade_id            INTEGER REFERENCES trades(id) ON DELETE CASCADE,
    income_id           INTEGER REFERENCES income(id) ON DELETE CASCADE,
    amma_statement_id   INTEGER REFERENCES amma_statements(id) ON DELETE CASCADE,
    ess_statement_id    INTEGER REFERENCES ess_statements(id) ON DELETE CASCADE,
    interest_income_id  INTEGER REFERENCES interest_income(id) ON DELETE CASCADE,
    corporate_action_id INTEGER REFERENCES corporate_actions(id) ON DELETE CASCADE,
    filename            TEXT    NOT NULL,
    content_type        TEXT    NOT NULL CHECK(content_type IN ('application/pdf', 'image/png', 'image/jpeg', 'text/plain')),
    byte_size           INTEGER NOT NULL,
    checksum            TEXT    NOT NULL,
    uploaded_at         TEXT    NOT NULL,
    content             BLOB    NOT NULL,
    CHECK ((trade_id IS NOT NULL) + (income_id IS NOT NULL) + (amma_statement_id IS NOT NULL)
         + (ess_statement_id IS NOT NULL) + (interest_income_id IS NOT NULL) + (corporate_action_id IS NOT NULL) = 1)
);

INSERT INTO attachments (
    id, trade_id, income_id, amma_statement_id, ess_statement_id, interest_income_id,
    corporate_action_id, filename, content_type, byte_size, checksum, uploaded_at, content
)
    SELECT
    id, trade_id, income_id, amma_statement_id, ess_statement_id, interest_income_id,
    corporate_action_id, filename, content_type, byte_size, checksum, uploaded_at, content
    FROM attachments_old
    ORDER BY id;

DROP TABLE attachments_old;

CREATE INDEX attachments_trade_id ON attachments (trade_id);
CREATE INDEX attachments_income_id ON attachments (income_id);
CREATE INDEX attachments_amma_statement_id ON attachments (amma_statement_id);
CREATE INDEX attachments_ess_statement_id ON attachments (ess_statement_id);
CREATE INDEX attachments_interest_income_id ON attachments (interest_income_id);
CREATE INDEX attachments_corporate_action_id ON attachments (corporate_action_id);

CREATE TRIGGER attachments_row_history_update AFTER UPDATE ON attachments
BEGIN
    INSERT INTO row_history (table_name, row_id, operation, changed_at, old_row)
    VALUES ('attachments', OLD.id, 'UPDATE', strftime('%Y-%m-%dT%H:%M:%fZ', 'now'),
            json_object(
                        'id', OLD.id, 'trade_id', OLD.trade_id, 'income_id', OLD.income_id,
                        'amma_statement_id', OLD.amma_statement_id,
                        'ess_statement_id', OLD.ess_statement_id,
                        'interest_income_id', OLD.interest_income_id,
                        'corporate_action_id', OLD.corporate_action_id,
                        'filename', OLD.filename, 'content_type', OLD.content_type,
                        'byte_size', OLD.byte_size, 'checksum', OLD.checksum,
                        'uploaded_at', OLD.uploaded_at));
END;

CREATE TRIGGER attachments_row_history_delete AFTER DELETE ON attachments
BEGIN
    INSERT INTO row_history (table_name, row_id, operation, changed_at, old_row)
    VALUES ('attachments', OLD.id, 'DELETE', strftime('%Y-%m-%dT%H:%M:%fZ', 'now'),
            json_object(
                        'id', OLD.id, 'trade_id', OLD.trade_id, 'income_id', OLD.income_id,
                        'amma_statement_id', OLD.amma_statement_id,
                        'ess_statement_id', OLD.ess_statement_id,
                        'interest_income_id', OLD.interest_income_id,
                        'corporate_action_id', OLD.corporate_action_id,
                        'filename', OLD.filename, 'content_type', OLD.content_type,
                        'byte_size', OLD.byte_size, 'checksum', OLD.checksum,
                        'uploaded_at', OLD.uploaded_at));
END;

INSERT INTO sqlite_sequence (name, seq)
    SELECT 'attachments', 0 WHERE NOT EXISTS (SELECT 1 FROM sqlite_sequence WHERE name = 'attachments');
UPDATE sqlite_sequence
    SET seq = MAX(seq, (SELECT COALESCE(MAX(row_id), 0) FROM row_history
                         WHERE table_name = 'attachments'))
    WHERE name = 'attachments';

-- ---------------------------------------------------------------------------
-- listings
-- ---------------------------------------------------------------------------

DROP TRIGGER listings_row_history_update;
DROP TRIGGER listings_row_history_delete;
DROP TRIGGER listings_stale_snapshots_update;

PRAGMA legacy_alter_table = ON;
ALTER TABLE listings RENAME TO listings_old;
PRAGMA legacy_alter_table = OFF;

CREATE TABLE listings (
    id            INTEGER PRIMARY KEY AUTOINCREMENT,
    -- NULL exactly for Crypto listings: a crypto asset trades on no MIC-coded
    -- venue, settles same-day, and has no holiday calendar.
    exchange_mic  TEXT    REFERENCES exchanges(mic),
    ticker        TEXT    NOT NULL,
    name          TEXT    NOT NULL,
    isin          TEXT,
    security_type TEXT    NOT NULL CHECK(security_type IN ('Share', 'ETF', 'LIC', 'Trust', 'Crypto')),
    currency      TEXT    NOT NULL REFERENCES currencies(code),
    amit          INTEGER NOT NULL DEFAULT 0,
    preference    INTEGER NOT NULL DEFAULT 0,
    -- Provider symbol override for the price fetch, where it differs from what
    -- ticker + exchange would produce (0020).
    price_symbol  TEXT,
    -- The listing is an AMIT for records on or after this date and an ordinary
    -- trust before it; NULL means the `amit` flag applies to the whole history.
    -- Only meaningful on an `amit` listing — a pairing SQLite cannot express
    -- here, so entities::listing::db_upsert enforces it (0024).
    amit_from     TEXT,
    -- No closing price is obtainable from this date on (a suspension or
    -- delisting): valuation carries the last stored close forward and flags the
    -- snapshot price_carried_forward (0035).
    unpriced_from TEXT,
    -- No closing price is obtainable *before* this date — the mirror of
    -- unpriced_from, and it deliberately requires no stored price on either
    -- side of the date (0037).
    unpriced_before TEXT,
    UNIQUE(exchange_mic, ticker),
    CHECK ((exchange_mic IS NULL) = (security_type = 'Crypto'))
);

INSERT INTO listings (
    id, exchange_mic, ticker, name, isin, security_type, currency, amit, preference,
    price_symbol, amit_from, unpriced_from, unpriced_before
)
    SELECT
    id, exchange_mic, ticker, name, isin, security_type, currency, amit, preference,
    price_symbol, amit_from, unpriced_from, unpriced_before
    FROM listings_old
    ORDER BY id;

DROP TABLE listings_old;

CREATE UNIQUE INDEX listings_crypto_ticker ON listings(ticker) WHERE exchange_mic IS NULL;

CREATE TRIGGER listings_row_history_update AFTER UPDATE ON listings
BEGIN
    INSERT INTO row_history (table_name, row_id, operation, changed_at, old_row)
    VALUES ('listings', OLD.id, 'UPDATE', strftime('%Y-%m-%dT%H:%M:%fZ', 'now'),
            json_object(
                        'id', OLD.id, 'exchange_mic', OLD.exchange_mic, 'ticker', OLD.ticker,
                        'name', OLD.name, 'isin', OLD.isin, 'security_type', OLD.security_type,
                        'currency', OLD.currency, 'amit', OLD.amit, 'preference', OLD.preference,
                        'price_symbol', OLD.price_symbol, 'amit_from', OLD.amit_from,
                        'unpriced_from', OLD.unpriced_from,
                        'unpriced_before', OLD.unpriced_before));
END;

CREATE TRIGGER listings_row_history_delete AFTER DELETE ON listings
BEGIN
    INSERT INTO row_history (table_name, row_id, operation, changed_at, old_row)
    VALUES ('listings', OLD.id, 'DELETE', strftime('%Y-%m-%dT%H:%M:%fZ', 'now'),
            json_object(
                        'id', OLD.id, 'exchange_mic', OLD.exchange_mic, 'ticker', OLD.ticker,
                        'name', OLD.name, 'isin', OLD.isin, 'security_type', OLD.security_type,
                        'currency', OLD.currency, 'amit', OLD.amit, 'preference', OLD.preference,
                        'price_symbol', OLD.price_symbol, 'amit_from', OLD.amit_from,
                        'unpriced_from', OLD.unpriced_from,
                        'unpriced_before', OLD.unpriced_before));
END;

CREATE TRIGGER listings_stale_snapshots_update AFTER UPDATE ON listings
WHEN OLD.currency <> NEW.currency
  OR OLD.security_type <> NEW.security_type
  OR OLD.unpriced_from IS NOT NEW.unpriced_from
  OR OLD.unpriced_before IS NOT NEW.unpriced_before
BEGIN
    -- currency / security_type: no date of their own, so the whole series.
    UPDATE report_snapshots SET stale = 1
    WHERE OLD.currency <> NEW.currency OR OLD.security_type <> NEW.security_type;

    -- unpriced_from: only the snapshots dated on or after the earlier of the
    -- old and new dates (IFNULL both ways, so a set uses NEW's date and a
    -- clear uses OLD's).
    UPDATE report_snapshots SET stale = 1
    WHERE OLD.unpriced_from IS NOT NEW.unpriced_from
      AND snapshot_date >= MIN(IFNULL(OLD.unpriced_from, NEW.unpriced_from),
                               IFNULL(NEW.unpriced_from, OLD.unpriced_from));

    -- unpriced_before: the mirror — only the snapshots dated *before* the
    -- later of the old and new dates.
    UPDATE report_snapshots SET stale = 1
    WHERE OLD.unpriced_before IS NOT NEW.unpriced_before
      AND snapshot_date < MAX(IFNULL(OLD.unpriced_before, NEW.unpriced_before),
                              IFNULL(NEW.unpriced_before, OLD.unpriced_before));
END;

INSERT INTO sqlite_sequence (name, seq)
    SELECT 'listings', 0 WHERE NOT EXISTS (SELECT 1 FROM sqlite_sequence WHERE name = 'listings');
UPDATE sqlite_sequence
    SET seq = MAX(seq, (SELECT COALESCE(MAX(row_id), 0) FROM row_history
                         WHERE table_name = 'listings'))
    WHERE name = 'listings';

-- ---------------------------------------------------------------------------
-- listing_renames
-- ---------------------------------------------------------------------------

DROP TRIGGER listing_renames_row_history_update;
DROP TRIGGER listing_renames_row_history_delete;

PRAGMA legacy_alter_table = ON;
ALTER TABLE listing_renames RENAME TO listing_renames_old;
PRAGMA legacy_alter_table = OFF;

CREATE TABLE listing_renames (
    id               INTEGER PRIMARY KEY AUTOINCREMENT,
    listing_id       INTEGER NOT NULL REFERENCES listings(id),
    effective_date   TEXT    NOT NULL,   -- first trading day under the new identity
    old_ticker       TEXT    NOT NULL,
    new_ticker       TEXT    NOT NULL,
    old_exchange_mic TEXT    REFERENCES exchanges(mic),
    new_exchange_mic TEXT    REFERENCES exchanges(mic),
    note             TEXT,
    -- What the rename replaced, so the chain shows the old identity as well as
    -- the new one and db_undo can restore it. A NULL old_name means only that
    -- the row predates 0040, and the CHECK makes that reading enforceable: a
    -- row with no recorded name can carry no recorded symbol either. Read by
    -- the undo and GET /listings/:id/renames; no calculation reads them (0040).
    old_name         TEXT,
    old_price_symbol TEXT
        CHECK (old_name IS NOT NULL OR old_price_symbol IS NULL),
    UNIQUE (listing_id, effective_date),
    CHECK (old_ticker <> new_ticker OR old_exchange_mic IS NOT new_exchange_mic)
);

INSERT INTO listing_renames (
    id, listing_id, effective_date, old_ticker, new_ticker, old_exchange_mic,
    new_exchange_mic, note, old_name, old_price_symbol
)
    SELECT
    id, listing_id, effective_date, old_ticker, new_ticker, old_exchange_mic,
    new_exchange_mic, note, old_name, old_price_symbol
    FROM listing_renames_old
    ORDER BY id;

DROP TABLE listing_renames_old;

CREATE INDEX listing_renames_listing_id ON listing_renames (listing_id);
CREATE INDEX listing_renames_old_exchange_mic ON listing_renames (old_exchange_mic);
CREATE INDEX listing_renames_new_exchange_mic ON listing_renames (new_exchange_mic);

CREATE TRIGGER listing_renames_row_history_update AFTER UPDATE ON listing_renames
BEGIN
    INSERT INTO row_history (table_name, row_id, operation, changed_at, old_row)
    VALUES ('listing_renames', OLD.id, 'UPDATE', strftime('%Y-%m-%dT%H:%M:%fZ', 'now'),
            json_object(
                        'id', OLD.id, 'listing_id', OLD.listing_id,
                        'effective_date', OLD.effective_date,
                        'old_ticker', OLD.old_ticker, 'new_ticker', OLD.new_ticker,
                        'old_exchange_mic', OLD.old_exchange_mic,
                        'new_exchange_mic', OLD.new_exchange_mic,
                        'old_name', OLD.old_name,
                        'old_price_symbol', OLD.old_price_symbol,
                        'note', OLD.note));
END;

CREATE TRIGGER listing_renames_row_history_delete AFTER DELETE ON listing_renames
BEGIN
    INSERT INTO row_history (table_name, row_id, operation, changed_at, old_row)
    VALUES ('listing_renames', OLD.id, 'DELETE', strftime('%Y-%m-%dT%H:%M:%fZ', 'now'),
            json_object(
                        'id', OLD.id, 'listing_id', OLD.listing_id,
                        'effective_date', OLD.effective_date,
                        'old_ticker', OLD.old_ticker, 'new_ticker', OLD.new_ticker,
                        'old_exchange_mic', OLD.old_exchange_mic,
                        'new_exchange_mic', OLD.new_exchange_mic,
                        'old_name', OLD.old_name,
                        'old_price_symbol', OLD.old_price_symbol,
                        'note', OLD.note));
END;

INSERT INTO sqlite_sequence (name, seq)
    SELECT 'listing_renames', 0 WHERE NOT EXISTS (SELECT 1 FROM sqlite_sequence WHERE name = 'listing_renames');
UPDATE sqlite_sequence
    SET seq = MAX(seq, (SELECT COALESCE(MAX(row_id), 0) FROM row_history
                         WHERE table_name = 'listing_renames'))
    WHERE name = 'listing_renames';

COMMIT;

-- Restored for the connection this ran on; every other pooled connection opens
-- with foreign keys on (infra::db).
PRAGMA foreign_keys = ON;
