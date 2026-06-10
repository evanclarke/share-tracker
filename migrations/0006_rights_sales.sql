-- Selling or lapsing renounceable rights (REQUIREMENTS 2026-06-10;
-- docs/ato/rights-issues.md Example 39, docs/ato/retail-premiums.md TR 2017/4).
--
-- A rights sale disposes of the rights themselves — a CGT event on the
-- rights, not on the original shares, so it is its own table rather than a
-- Sell trade: a Sell would consume share parcels and shrink the holding,
-- which selling rights must never do. The disposal reaches the realised-gains
-- and net-capital-gain reports from here (reports/realised_gains.rs reads
-- both tables alongside trades).
--
-- Rows are written only by POST /corporate_actions/:id/sell_rights, which
-- validates (in one transaction) the entitlement cap shared with exercises
-- and the per-parcel anchoring below; rows are immutable (no PUT) — delete
-- and re-enter to amend. The referenced RightsIssue action is frozen while
-- rows exist (entities::corporate_action), like exercise trades.
CREATE TABLE rights_sales (
    id                 INTEGER PRIMARY KEY,
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

-- Which original parcels the sold rights are anchored to. Free rights are
-- taken to have been acquired when the original shares were acquired
-- (docs/ato/rights-issues.md), so each allocation's 12-month discount clock
-- runs from its parcel's (possibly deemed) acquisition date. Unlike
-- parcel_allocations these do NOT consume the parcel's units — the original
-- shares are still held; the link only anchors dates and caps how many
-- rights each parcel can have earned.
CREATE TABLE rights_sale_allocations (
    id                INTEGER PRIMARY KEY,
    rights_sale_id    INTEGER NOT NULL REFERENCES rights_sales(id) ON DELETE CASCADE,
    purchase_trade_id INTEGER NOT NULL REFERENCES trades(id),
    -- Rights anchored to this parcel, in record-date rights units; the sale's
    -- allocations sum exactly to its `units` (validated in Rust).
    units             TEXT    NOT NULL
);

-- No snapshot-staleness triggers: the snapshotted reports (portfolio
-- overview, unrealised gains, performance) never read these tables — a
-- rights sale changes no holding quantity and no parcel cost base, so no
-- stored snapshot's figures can be invalidated by one (cf. the 0005
-- inheritances rationale; the CGT-side reports that do read them are
-- computed live, never snapshotted).
