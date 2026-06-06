-- Preference-share flag on listings (same boolean-flag pattern as `amit`).
-- Drives the franking-credit holding-period rule: preference shares must be
-- held at risk for 90 days instead of 45 to claim attached franking credits
-- (see docs/you-and-your-shares-dividends.md and src/reports/franking.rs).
-- Additive only — no data dropped.
ALTER TABLE listings ADD COLUMN preference INTEGER NOT NULL DEFAULT 0;
