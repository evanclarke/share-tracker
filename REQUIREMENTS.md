# Project Overview

This project is a comprehensive share tracker, where facts about the investing activity are recorded
and an overview of the portfolio can be materialised for given market prices from these facts.  Reporting
and cost basis calculations are done with the Australian tax view in mind.

# Status

Everything specified here through 2026-06-07 is implemented (or explicitly resolved out of scope) and
is documented in `README.md`: the Features, Database schema, and HTTP API sections describe the
implemented behaviour, and the Known limitations section records the resolved out-of-scope decisions
(taxpayer entity type, cost-base elements 3–5 / reduced cost base, taxpayer-level accounts, DRP partial
participation, ESS income). The full historical requirement text and each item's resolution are
preserved in git history and in `TODO.md`'s entries. Ongoing engineering rules (Decimal-only money,
non-destructive migrations, enum constraints, write-time invariants, …) live in `CLAUDE.md`.

# New Requirements

New requirements are written below and then folded into `TODO.md`.

## Crypto-asset holdings (BTC, ETH, …)

Investment holdings of crypto assets (e.g. Bitcoin, Ether) can be recorded and reported alongside
share holdings. The ATO treats investment crypto as a CGT asset, so the existing parcel-level CGT
machinery (parcel allocations, reduced cost base, the 12-month 50% discount, loss netting) applies
unchanged — the work is letting an asset exist without an exchange, not new tax rules.

- Crypto assets as listings
  - A new listing security type `Crypto`; a Crypto listing has **no exchange** (`exchange_mic` is
    absent for it, and only for it — the existing security types must still require an exchange,
    enforced by database constraint and at write time)
  - A Crypto listing's ticker must be a recognised digital-token code in the currencies reference
    table (kind DigitalToken, from the ISO 24165 / DTIF import) — an unrecognised token is rejected
  - Exchange-less listings are unique by ticker (the existing per-exchange uniqueness can't cover
    them)
  - The listing's currency is the pricing currency for its trades (e.g. AUD for BTC bought on an
    AUD market); quantities are token units — the existing arbitrary-precision Decimal storage
    already covers satoshi/wei-scale fractions
- Settlement
  - Trades on an exchange-less listing settle same-day: the auto-calculated settlement date is the
    trade date (no T+N, no holiday calendar)
- Reference-data reports
  - The exchange MIC validation report and the settlement-holiday coverage report skip
    exchange-less listings (nothing to validate; no holiday calendar applies)
- Tax and reporting
  - Crypto parcels flow through the portfolio, unrealised, realised, performance, and net capital
    gain reports exactly like share parcels: cost base and proceeds in AUD, discount eligibility
    after 12 months, ATO-ordered loss netting
  - Market prices for valuation continue to be supplied in the report request (no crypto price
    feed)
  - Holding-account transfers of crypto (e.g. wallet to wallet) work like any listing transfer:
    not a CGT event, cost base and acquisition date carried
- ATO documentation
  - Mirror the ATO's crypto-as-CGT-asset guidance into `docs/ato/` (source URL + retrieval date,
    indexed in `OVERVIEW.md`); add `ato_examples.rs` acceptance tests for any worked examples the
    feature makes representable
- Web UI
  - The listings screen supports the Crypto type with the exchange field optional accordingly;
    crypto trades/parcels/reports need no special casing beyond the existing config-driven screens

Out of scope (record as Known limitations):
- **Foreign-currency cash balances** — deferred. Holding FX itself is generally not CGT (Division
  775 forex realisation gains/losses are ordinary income, with the $250k limited-balance election),
  so it needs its own tax engine and will be specified separately
- **Crypto-to-crypto swaps** (e.g. BTC→ETH) as an atomic operation — a swap is a disposal at market
  value and can be entered manually as a Sell plus a Buy; a dedicated swap operation (like the
  buy-back/scrip participations) is deferred
- **Staking rewards and airdrops** — ordinary income at receipt with a cost base equal to market
  value at receipt; can be entered manually (income row + Buy at the receipt-date market value),
  a linked operation is deferred
- **Chain splits/forks, wrapping, and the personal-use-asset exemption** — not modelled
