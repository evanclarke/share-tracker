-- Document attachments: a supporting file (trade confirmation PDF, dividend or
-- AMMA statement scan) stored against exactly one activity. The file bytes live
-- in the database as a BLOB so the weekly backup captures them with no separate
-- file store to back up.
CREATE TABLE attachments (
    id                INTEGER PRIMARY KEY,
    -- Owner: exactly one of these three FKs is non-null (enforced by the CHECK
    -- below). A real per-activity foreign key keeps referential integrity to the
    -- owning row; ON DELETE CASCADE removes an activity's attachments when the
    -- activity is deleted, so no orphaned blobs remain.
    trade_id          INTEGER REFERENCES trades(id) ON DELETE CASCADE,
    income_id         INTEGER REFERENCES income(id) ON DELETE CASCADE,
    amma_statement_id INTEGER REFERENCES amma_statements(id) ON DELETE CASCADE,
    filename          TEXT    NOT NULL,                                              -- original upload filename, preserved for download
    content_type      TEXT    NOT NULL CHECK(content_type IN ('application/pdf', 'image/png', 'image/jpeg')),
    byte_size         INTEGER NOT NULL,                                              -- size of content in bytes (informational, for display)
    checksum          TEXT    NOT NULL,                                              -- SHA-256 of content, hex (integrity / duplicate detection)
    uploaded_at       TEXT    NOT NULL,                                              -- RFC 3339 timestamp the attachment was stored
    content           BLOB    NOT NULL,                                              -- the file bytes
    CHECK ((trade_id IS NOT NULL) + (income_id IS NOT NULL) + (amma_statement_id IS NOT NULL) = 1)
);
