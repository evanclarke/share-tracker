-- Attachment owner expansion (2026-07-15 requirement):
-- 1. Two new owner links: an attachment can now belong to an ESS statement
--    (the annual employee-share-scheme statement) or an interest income
--    record (e.g. a broker statement whose only activity is cash interest) —
--    previously only Trade / Income / AMMA Statement rows could own documents.
-- 2. text/plain joins the content-type allowlist: parts of the document
--    archive (crypto exchange trade records, registry DRP advices) are
--    plain-text files, which previously could not be attached at all.
--
-- Both rules live in table-level CHECK constraints, which SQLite cannot
-- ALTER, so the table is rebuilt via the rename pattern (0012 precedent):
-- rename out, create the new shape, copy every row, drop the _old table.
-- Ids are copied explicitly — row_history entries key on them.
--
-- attachments is an audited table: dropping the old table drops its two
-- row_history triggers with it, so they are re-created below with the new
-- column list (the CLAUDE.md audited-table rule). content (a BLOB) stays
-- excluded from the JSON old row — json_object() cannot hold a BLOB.
--
-- No snapshot-staleness triggers are needed: no snapshotted report reads
-- attachments (documents are provenance, not financial facts).

ALTER TABLE attachments RENAME TO attachments_old;

CREATE TABLE attachments (
    id                 INTEGER PRIMARY KEY,
    trade_id           INTEGER REFERENCES trades(id) ON DELETE CASCADE,
    income_id          INTEGER REFERENCES income(id) ON DELETE CASCADE,
    amma_statement_id  INTEGER REFERENCES amma_statements(id) ON DELETE CASCADE,
    ess_statement_id   INTEGER REFERENCES ess_statements(id) ON DELETE CASCADE,
    interest_income_id INTEGER REFERENCES interest_income(id) ON DELETE CASCADE,
    filename           TEXT    NOT NULL,
    content_type       TEXT    NOT NULL CHECK(content_type IN ('application/pdf', 'image/png', 'image/jpeg', 'text/plain')),
    byte_size          INTEGER NOT NULL,
    checksum           TEXT    NOT NULL,
    uploaded_at        TEXT    NOT NULL,
    content            BLOB    NOT NULL,
    CHECK ((trade_id IS NOT NULL) + (income_id IS NOT NULL) + (amma_statement_id IS NOT NULL)
         + (ess_statement_id IS NOT NULL) + (interest_income_id IS NOT NULL) = 1)
);

INSERT INTO attachments (id, trade_id, income_id, amma_statement_id, filename, content_type, byte_size, checksum, uploaded_at, content)
    SELECT id, trade_id, income_id, amma_statement_id, filename, content_type, byte_size, checksum, uploaded_at, content
    FROM attachments_old
    ORDER BY id;

DROP TABLE attachments_old;

CREATE TRIGGER attachments_row_history_update AFTER UPDATE ON attachments
BEGIN
    INSERT INTO row_history (table_name, row_id, operation, changed_at, old_row)
    VALUES ('attachments', OLD.id, 'UPDATE', strftime('%Y-%m-%dT%H:%M:%fZ', 'now'),
            json_object(
                        'id', OLD.id, 'trade_id', OLD.trade_id, 'income_id', OLD.income_id,
                        'amma_statement_id', OLD.amma_statement_id,
                        'ess_statement_id', OLD.ess_statement_id,
                        'interest_income_id', OLD.interest_income_id,
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
                        'filename', OLD.filename, 'content_type', OLD.content_type,
                        'byte_size', OLD.byte_size, 'checksum', OLD.checksum,
                        'uploaded_at', OLD.uploaded_at));
END;
